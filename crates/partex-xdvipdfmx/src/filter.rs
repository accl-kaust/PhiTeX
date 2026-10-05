//! Stream filters (pdfobj.c): the predictors applied before compression,
//! and the decoders used to read streams of included PDF files.

use alloc::vec;
use alloc::vec::Vec;

use crate::obj::DecodeParms;

/// `filter_PNG15_apply_filter`: PNG's per-row filter chosen by the
/// minimum sum of absolute differences.
#[must_use]
pub fn png15_apply(raster: &[u8], columns: i32, rows: i32, bpc: i32, colors: i32) -> Vec<u8> {
    let bits_per_pixel = colors * bpc;
    let bpp = ((bits_per_pixel + 7) / 8) as isize;
    let rowbytes = (columns as isize) * bpp;
    let rows = rows.max(0) as isize;
    let mut dst = vec![0u8; ((rowbytes + 1) * rows) as usize];
    let at = |i: isize| -> i32 { i32::from(raster[i as usize]) };
    for j in 0..rows {
        let pp = (j * (rowbytes + 1)) as usize;
        let p = j * rowbytes;
        let mut sum = [0u32; 5];
        for i in 0..rowbytes {
            let left = if i - bpp >= 0 { at(p + i - bpp) } else { 0 };
            let up = if j > 0 { at(p + i - rowbytes) } else { 0 };
            let uplft = if j > 0 && i - bpp >= 0 {
                at(p + i - rowbytes - bpp)
            } else {
                0
            };
            let cur = at(p + i);
            sum[0] = sum[0].wrapping_add(cur as u32);
            sum[1] = sum[1].wrapping_add((cur - left).unsigned_abs());
            sum[2] = sum[2].wrapping_add((cur - up).unsigned_abs());
            let tmp = (up + left) / 2;
            sum[3] = sum[3].wrapping_add((cur - tmp).unsigned_abs());
            let q = left + up - uplft;
            let (qa, qb, qc) = ((q - left).abs(), (q - up).abs(), (q - uplft).abs());
            let v = if qa <= qb && qa <= qc {
                left
            } else if qb <= qc {
                up
            } else {
                uplft
            };
            sum[4] = sum[4].wrapping_add((cur - v).unsigned_abs());
        }
        // (`int min = sum[0]`: compared as unsigned)
        let mut min = sum[0];
        let mut min_idx = 0;
        for (i, &s) in sum.iter().enumerate() {
            if s < min {
                min = s;
                min_idx = i;
            }
        }
        dst[pp] = min_idx as u8;
        for i in 0..rowbytes {
            let cur = at(p + i);
            let left = if i - bpp >= 0 { at(p + i - bpp) } else { 0 };
            let up = if j > 0 { at(p + i - rowbytes) } else { 0 };
            let v = match min_idx {
                0 => 0,
                1 => left,
                2 => up,
                3 => (up + left) / 2,
                _ => {
                    let uplft = if j > 0 && i - bpp >= 0 {
                        at(p + i - rowbytes - bpp)
                    } else {
                        0
                    };
                    let q = left + up - uplft;
                    let (qa, qb, qc) = ((q - left).abs(), (q - up).abs(), (q - uplft).abs());
                    if qa <= qb && qa <= qc {
                        left
                    } else if qb <= qc {
                        up
                    } else {
                        uplft
                    }
                }
            };
            dst[pp + 1 + i as usize] = (cur - v) as u8;
        }
    }
    dst
}

/// `filter_TIFF2_apply_filter`.
#[must_use]
pub fn tiff2_apply(raster: &[u8], columns: i32, rows: i32, bpc: i32, colors: i32) -> Vec<u8> {
    let rowbytes = ((bpc * colors * columns + 7) / 8) as usize;
    let rows = rows.max(0) as usize;
    let n = rowbytes * rows;
    let mut dst = raster[..n.min(raster.len())].to_vec();
    dst.resize(n, 0);
    let colors = colors.max(0) as usize;
    let columns = columns.max(0) as usize;
    match bpc {
        1 | 2 | 4 => {
            let mask: u8 = ((1u16 << bpc) - 1) as u8;
            let bpc = bpc as u32;
            let mut prev = vec![0u16; colors];
            for j in 0..rows {
                prev.iter_mut().for_each(|x| *x = 0);
                let (mut inbuf, mut outbuf) = (0u16, 0u16);
                let (mut inbits, mut outbits) = (0u32, 0u32);
                let mut l = j * rowbytes;
                let mut k = j * rowbytes;
                for _ in 0..columns {
                    for c in 0..colors {
                        if inbits < bpc {
                            inbuf = (inbuf << 8) | u16::from(dst[l]);
                            l += 1;
                            inbits += 8;
                        }
                        let cur = ((inbuf >> (inbits - bpc)) as u8) & mask;
                        inbits -= bpc;
                        let mut sub = (i16::from(cur) - prev[c] as i16) as i8;
                        prev[c] = u16::from(cur);
                        if sub < 0 {
                            sub = (i16::from(sub) + (1 << bpc)) as i8;
                        }
                        outbuf = (outbuf << bpc) | u16::from(sub as u8);
                        outbits += bpc;
                        if outbits >= 8 {
                            dst[k] = (outbuf >> (outbits - 8)) as u8;
                            k += 1;
                            outbits -= 8;
                        }
                    }
                }
                if outbits > 0 {
                    dst[k] = (outbuf << (8 - outbits)) as u8;
                }
            }
        }
        8 => {
            let mut prev = vec![0u16; colors];
            for j in 0..rows {
                prev.iter_mut().for_each(|x| *x = 0);
                for i in 0..columns {
                    let pos = colors * (columns * j + i);
                    for c in 0..colors {
                        let cur = u16::from(raster[pos + c]);
                        let sub = i32::from(cur) - i32::from(prev[c]);
                        prev[c] = cur;
                        dst[pos + c] = sub as u8;
                    }
                }
            }
        }
        16 => {
            let mut prev = vec![0u16; colors];
            for j in 0..rows {
                prev.iter_mut().for_each(|x| *x = 0);
                for i in 0..columns {
                    let pos = 2 * colors * (columns * j + i);
                    for c in 0..colors {
                        let cur = u16::from(raster[pos + 2 * c]) * 256
                            + u16::from(raster[pos + 2 * c + 1]);
                        let sub = cur.wrapping_sub(prev[c]);
                        prev[c] = cur;
                        dst[pos + 2 * c] = (sub >> 8) as u8;
                        dst[pos + 2 * c + 1] = sub as u8;
                    }
                }
            }
        }
        _ => {}
    }
    dst
}

fn row_tiff2(dst: &mut [u8], src: &[u8], parms: &DecodeParms) {
    let bpc = parms.bits_per_component as u32;
    let mask: i32 = (1 << bpc) - 1;
    let mut col = vec![0u8; parms.colors.max(0) as usize];
    let (mut inbuf, mut outbuf) = (0i32, 0i32);
    let (mut inbits, mut outbits) = (0u32, 0u32);
    let (mut j, mut k) = (0usize, 0usize);
    for _ in 0..parms.columns {
        for c in &mut col {
            if inbits < bpc {
                inbuf = (inbuf << 8) | i32::from(src[j]);
                j += 1;
                inbits += 8;
            }
            *c = ((i32::from(*c) + (inbuf >> (inbits - bpc))) & mask) as u8;
            inbits -= bpc;
            outbuf = (outbuf << bpc) | i32::from(*c);
            outbits += bpc;
            if outbits >= 8 {
                dst[k] = (outbuf >> (outbits - 8)) as u8;
                k += 1;
                outbits -= 8;
            }
        }
    }
    if outbits > 0 {
        dst[k] = (outbuf << (8 - outbits)) as u8;
    }
}

/// `filter_stream_decode_Predictor`.
#[must_use]
pub fn decode_predictor(src: &[u8], parms: &DecodeParms) -> Option<Vec<u8>> {
    let bits_per_pixel = parms.colors * parms.bits_per_component;
    let bpp = ((bits_per_pixel + 7) / 8) as usize;
    let length = ((parms.columns * bits_per_pixel + 7) / 8) as usize;
    let mut dst = Vec::new();
    let mut prev = vec![0u8; length];
    let mut buf = vec![0u8; length];
    let mut p = 0usize;
    let end = src.len();
    match parms.predictor {
        1 => dst.extend_from_slice(src),
        2 => {
            if parms.bits_per_component == 8 {
                while p + length < end {
                    for i in 0..length {
                        let pv = if i >= bpp { buf[i - bpp] } else { 0 };
                        buf[i] = src[p + i].wrapping_add(pv);
                    }
                    dst.extend_from_slice(&buf);
                    p += length;
                }
            } else if parms.bits_per_component == 16 {
                while p + length < end {
                    let mut i = 0;
                    while i < length {
                        // (`char hi`, `char lo`: signed)
                        let (hi, lo) = if i >= bpp {
                            (
                                i32::from(buf[i - bpp] as i8),
                                i32::from(buf[i - bpp + 1] as i8),
                            )
                        } else {
                            (0, 0)
                        };
                        let pv = (hi << 8) | lo;
                        let cv = (i32::from(src[p + i]) << 8) | i32::from(src[p + i + 1]);
                        let c = pv + cv;
                        buf[i] = (c >> 8) as u8;
                        buf[i + 1] = c as u8;
                        i += 2;
                    }
                    dst.extend_from_slice(&buf);
                    p += length;
                }
            } else {
                while p + length < end {
                    row_tiff2(&mut buf, &src[p..], parms);
                    dst.extend_from_slice(&buf);
                    p += length;
                }
            }
        }
        10..=15 => {
            let mut kind = parms.predictor - 10;
            while p + length < end {
                if parms.predictor == 15 {
                    kind = i32::from(src[p]);
                } else if i32::from(src[p]) != kind {
                    return None;
                }
                p += 1;
                let row = &src[p..p + length];
                match kind {
                    0 => buf.copy_from_slice(row),
                    1 => {
                        for i in 0..length {
                            let pv = if i >= bpp { buf[i - bpp] } else { 0 };
                            buf[i] = row[i].wrapping_add(pv);
                        }
                    }
                    2 => {
                        for i in 0..length {
                            buf[i] = row[i].wrapping_add(prev[i]);
                        }
                    }
                    3 => {
                        for i in 0..length {
                            let up = i32::from(prev[i]);
                            let left = if i >= bpp { i32::from(buf[i - bpp]) } else { 0 };
                            buf[i] = (i32::from(row[i]) + (up + left) / 2) as u8;
                        }
                    }
                    4 => {
                        for i in 0..length {
                            let a = if i >= bpp { i32::from(buf[i - bpp]) } else { 0 };
                            let b = i32::from(prev[i]);
                            let c = if i >= bpp {
                                i32::from(prev[i - bpp])
                            } else {
                                0
                            };
                            let q = a + b - c;
                            let (qa, qb, qc) = ((q - a).abs(), (q - b).abs(), (q - c).abs());
                            let v = if qa <= qb && qa <= qc {
                                a
                            } else if qb <= qc {
                                b
                            } else {
                                c
                            };
                            buf[i] = (i32::from(row[i]) + v) as u8;
                        }
                    }
                    _ => return None,
                }
                dst.extend_from_slice(&buf);
                prev.copy_from_slice(&buf);
                p += length;
            }
        }
        _ => return None,
    }
    Some(dst)
}

/// `filter_stream_decode_FlateDecode`.
#[must_use]
pub fn decode_flate(data: &[u8], parms: Option<&DecodeParms>) -> Option<Vec<u8>> {
    let raw = partex_engine::inflate::inflate(data).or_else(|| {
        // (zlib's inflate stops quietly at a truncated stream)
        let body = data.get(2..)?;
        let mut out = Vec::new();
        partex_engine::inflate::inflate_raw(body, &mut out);
        Some(out)
    })?;
    match parms {
        Some(p) => decode_predictor(&raw, p),
        None => Some(raw),
    }
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0)
}

/// `filter_stream_decode_ASCIIHexDecode`.
#[must_use]
pub fn decode_ascii_hex(data: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    crate::parse::skip_white(data, &mut p);
    let mut buf = Vec::new();
    let (mut ch, mut pos, mut eod) = (0u8, 0usize, false);
    while p < data.len() && !eod {
        let c1 = data[p];
        let val = match c1 {
            b'A'..=b'F' => c1 - b'A' + 10,
            b'a'..=b'f' => c1 - b'a' + 10,
            b'0'..=b'9' => c1 - b'0',
            b'>' => {
                eod = true;
                if pos % 2 == 0 {
                    break;
                }
                0
            }
            _ => return None,
        };
        if pos % 2 == 1 {
            buf.push(ch.wrapping_add(val));
            ch = 0;
        } else {
            ch = val << 4;
        }
        pos += 1;
        p += 1;
        crate::parse::skip_white(data, &mut p);
    }
    if !eod {
        return None;
    }
    Some(buf)
}

/// `filter_stream_decode_ASCII85Decode`.
#[must_use]
pub fn decode_ascii85(data: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    let skip = |p: &mut usize| {
        while *p < data.len() && is_space(data[*p]) {
            *p += 1;
        }
    };
    skip(&mut p);
    let mut buf: Vec<u8> = Vec::new();
    let mut eod = false;
    while p < data.len() && !eod {
        let mut q = [b'u'; 5];
        let mut ch = data[p];
        p += 1;
        skip(&mut p);
        if ch == b'z' {
            buf.extend_from_slice(&[0; 4]);
            continue;
        } else if ch == b'~' {
            if p < data.len() && data[p] == b'>' {
                eod = true;
                p += 1;
            } else {
                return None;
            }
            break;
        }
        q[0] = ch;
        let mut m = 1;
        while m < 5 && p < data.len() {
            ch = data[p];
            p += 1;
            skip(&mut p);
            if ch == b'~' {
                if p < data.len() && data[p] == b'>' {
                    eod = true;
                    p += 1;
                } else {
                    return None;
                }
                break;
            } else if !(b'!'..=b'u').contains(&ch) {
                return None;
            }
            q[m] = ch;
            m += 1;
        }
        if m <= 1 {
            return None;
        }
        let d = |c: u8| u32::from(c - b'!');
        let mut val = 85 * 85 * 85 * d(q[0]) + 85 * 85 * d(q[1]) + 85 * d(q[2]) + d(q[3]);
        if val > u32::MAX / 85 {
            return None;
        }
        val *= 85;
        if val > u32::MAX - d(q[4]) {
            return None;
        }
        val += d(q[4]);
        let bytes = val.to_be_bytes();
        buf.extend_from_slice(&bytes[..m - 1]);
    }
    if !eod {
        return None;
    }
    Some(buf)
}
