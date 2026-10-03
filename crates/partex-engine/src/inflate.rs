//! zlib's `inflate` (RFC 1950 and 1951), as xpdf's `FlateStream` reads
//! the streams of an included PDF (`pdfread.rs`): its object streams and
//! cross-reference streams, and contents copied decoded. A stream that
//! goes wrong ends there, with what was decoded before, as xpdf's does
//! (it reports the error and hands out the bytes it has); the zlib
//! trailer's checksum is not checked, as xpdf does not.
//!
//! Also the predictors of `/DecodeParms` (PNG's and TIFF's), which
//! cross-reference streams use.

use alloc::vec::Vec;

/// The bits of a deflate stream, least significant first.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl Bits<'_> {
    /// The next `n` bits (at most 16).
    fn take(&mut self, n: u32) -> Option<u32> {
        while self.cnt < n {
            let b = *self.d.get(self.pos)?;
            self.pos += 1;
            self.buf |= u32::from(b) << self.cnt;
            self.cnt += 8;
        }
        let v = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.cnt -= n;
        Some(v)
    }
}

/// A canonical Huffman code: the number of codes of each length, and the
/// symbols in code order (RFC 1951 §3.2.2).
struct Huffman {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huffman {
    /// The code of these code lengths, or `None` if they oversubscribe
    /// it. (An incomplete code is allowed, as zlib allows a single
    /// distance code.)
    fn new(lengths: &[u8]) -> Option<Huffman> {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[usize::from(l)] += 1;
        }
        count[0] = 0;
        let mut left: i32 = 1;
        for &c in &count[1..] {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                return None;
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = alloc::vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                let o = &mut offs[usize::from(l)];
                symbol[usize::from(*o)] = u16::try_from(s).ok()?;
                *o += 1;
            }
        }
        Some(Huffman { count, symbol })
    }

    /// The next symbol of `bits` in this code.
    fn decode(&self, bits: &mut Bits<'_>) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= i32::try_from(bits.take(1)?).ok()?;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return self
                    .symbol
                    .get(usize::try_from(index + (code - first)).ok()?)
                    .copied();
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// The literals and lengths, then the distances, of a block, into `out`.
fn codes(bits: &mut Bits<'_>, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Option<()> {
    loop {
        let s = lit.decode(bits)?;
        match s {
            0..=255 => out.push(u8::try_from(s).ok()?),
            256 => return Some(()),
            _ => {
                let i = usize::from(s - 257);
                let len = usize::from(*LENGTH_BASE.get(i)?)
                    + usize::try_from(bits.take(u32::from(LENGTH_EXTRA[i]))?).ok()?;
                let d = usize::from(dist.decode(bits)?);
                let back = usize::from(*DIST_BASE.get(d)?)
                    + usize::try_from(bits.take(u32::from(DIST_EXTRA[d]))?).ok()?;
                if back > out.len() {
                    return None;
                }
                let from = out.len() - back;
                for k in 0..len {
                    let b = out[from + k];
                    out.push(b);
                }
            }
        }
    }
}

/// The order of the code length codes' lengths (RFC 1951 §3.2.7).
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A dynamic block's codes.
fn dynamic(bits: &mut Bits<'_>) -> Option<(Huffman, Huffman)> {
    let nlen = usize::try_from(bits.take(5)?).ok()? + 257;
    let ndist = usize::try_from(bits.take(5)?).ok()? + 1;
    let ncode = usize::try_from(bits.take(4)?).ok()? + 4;
    if nlen > 286 || ndist > 30 {
        return None;
    }
    let mut lengths = [0u8; 19];
    for &o in &ORDER[..ncode] {
        lengths[o] = u8::try_from(bits.take(3)?).ok()?;
    }
    let lencode = Huffman::new(&lengths)?;
    let mut l = alloc::vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < nlen + ndist {
        let s = lencode.decode(bits)?;
        if s < 16 {
            l[i] = u8::try_from(s).ok()?;
            i += 1;
        } else {
            let (v, n) = match s {
                16 => (*l.get(i.checked_sub(1)?)?, 3 + bits.take(2)?),
                17 => (0, 3 + bits.take(3)?),
                _ => (0, 11 + bits.take(7)?),
            };
            let n = usize::try_from(n).ok()?;
            if i + n > nlen + ndist {
                return None;
            }
            l[i..i + n].fill(v);
            i += n;
        }
    }
    if l[256] == 0 {
        return None;
    }
    Some((Huffman::new(&l[..nlen])?, Huffman::new(&l[nlen..])?))
}

/// The fixed codes (RFC 1951 §3.2.6).
fn fixed() -> (Huffman, Huffman) {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    let lit = Huffman::new(&l).unwrap_or(Huffman {
        count: [0; 16],
        symbol: Vec::new(),
    });
    let dist = Huffman::new(&[5u8; 30]).unwrap_or(Huffman {
        count: [0; 16],
        symbol: Vec::new(),
    });
    (lit, dist)
}

/// The raw deflate data `d` decoded into `out`, as far as it goes; whether
/// it went to its last block's end.
pub fn inflate_raw(d: &[u8], out: &mut Vec<u8>) -> bool {
    let mut bits = Bits {
        d,
        pos: 0,
        buf: 0,
        cnt: 0,
    };
    let mut block = || -> Option<bool> {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => {
                // (stored: from the next byte)
                bits.buf = 0;
                bits.cnt = 0;
                let p = bits.pos;
                let len = usize::from(u16::from_le_bytes([*d.get(p)?, *d.get(p + 1)?]));
                let nlen = u16::from_le_bytes([*d.get(p + 2)?, *d.get(p + 3)?]);
                if usize::from(!nlen) != len {
                    return None;
                }
                out.extend_from_slice(d.get(p + 4..p + 4 + len)?);
                bits.pos = p + 4 + len;
            }
            1 => {
                let (lit, dist) = fixed();
                codes(&mut bits, out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic(&mut bits)?;
                codes(&mut bits, out, &lit, &dist)?;
            }
            _ => return None,
        }
        Some(last)
    };
    loop {
        match block() {
            Some(true) => return true,
            Some(false) => {}
            None => return false,
        }
    }
}

/// A zlib stream (`/FlateDecode`) decoded, as far as it goes: `None` if
/// its header is not zlib's (xpdf's "Unknown compression method" and
/// "Bad FCHECK", "FDICT bit set": nothing decoded).
#[must_use]
pub fn inflate(d: &[u8]) -> Option<Vec<u8>> {
    let (&cmf, &flg) = (d.first()?, d.get(1)?);
    if cmf & 0x0f != 8 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 || flg & 0x20 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(d.len() * 4);
    inflate_raw(&d[2..], &mut out);
    Some(out)
}

/// `/DecodeParms` of a stream: its predictor (1: none, 2: TIFF's, 10-15:
/// PNG's), and the row's colors, bits per component and columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Predictor {
    pub predictor: i32,
    pub colors: i32,
    pub bits: i32,
    pub columns: i32,
}

impl Default for Predictor {
    fn default() -> Self {
        Predictor {
            predictor: 1,
            colors: 1,
            bits: 8,
            columns: 1,
        }
    }
}

/// `data` with predictor `p` undone (xpdf's `StreamPredictor`): PNG rows
/// each led by their filter type; TIFF's horizontal differencing (8 bits
/// per component; other widths are left as they are). `None` if the
/// parameters are out of xpdf's range (it then decodes nothing).
#[must_use]
pub fn unpredict(data: &[u8], p: Predictor) -> Option<Vec<u8>> {
    if p.predictor <= 1 {
        return Some(data.to_vec());
    }
    if !(1..=32).contains(&p.colors)
        || !matches!(p.bits, 1 | 2 | 4 | 8 | 16)
        || p.columns <= 0
        || p.columns > i32::MAX / p.colors / p.bits
    {
        return None;
    }
    let pix_bits = usize::try_from(p.colors * p.bits).ok()?;
    let row = (usize::try_from(p.columns).ok()? * pix_bits).div_ceil(8);
    let bpp = pix_bits.div_ceil(8);
    let mut out = Vec::with_capacity(data.len());
    if p.predictor == 2 {
        if p.bits != 8 {
            return Some(data.to_vec());
        }
        for r in data.chunks(row) {
            let start = out.len();
            for (i, &b) in r.iter().enumerate() {
                let left = if i >= bpp { out[start + i - bpp] } else { 0 };
                out.push(b.wrapping_add(left));
            }
        }
        return Some(out);
    }
    // (PNG: a row of zeros above the first)
    let mut prev = alloc::vec![0u8; row];
    let mut cur = alloc::vec![0u8; row];
    let mut i = 0;
    while i < data.len() {
        let ty = data[i];
        i += 1;
        let n = row.min(data.len() - i);
        cur[..n].copy_from_slice(&data[i..i + n]);
        cur[n..].fill(0);
        i += n;
        for k in 0..row {
            let a = if k >= bpp { cur[k - bpp] } else { 0 };
            let b = prev[k];
            let c = if k >= bpp { prev[k - bpp] } else { 0 };
            let x = cur[k];
            cur[k] = match ty {
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(
                    u8::try_from(u16::midpoint(u16::from(a), u16::from(b))).unwrap_or(0),
                ),
                4 => {
                    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
                    let pp = ia + ib - ic;
                    let (pa, pb, pc) = ((pp - ia).abs(), (pp - ib).abs(), (pp - ic).abs());
                    let pr = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    x.wrapping_add(pr)
                }
                _ => x,
            };
        }
        out.extend_from_slice(&cur[..n]);
        core::mem::swap(&mut prev, &mut cur);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// zlib's own streams (Python's `zlib.compress` at levels 1 and 9:
    /// fixed and dynamic blocks), decoded to their texts.
    #[test]
    fn inflates_zlib_streams() {
        let vecs: &[(&str, usize, u32, &str)] = &include!("inflate_vectors.rs");
        for &(name, len, crc, h) in vecs {
            let out = inflate(&hex(h)).unwrap();
            assert_eq!(out.len(), len, "{name}");
            assert_eq!(crate::zlib::crc32(&out), crc, "{name}");
        }
    }

    #[test]
    fn inflates_stored_blocks() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(inflate(&crate::zlib::stored(&data)).unwrap(), data);
    }

    #[test]
    fn a_broken_stream_keeps_what_it_decoded() {
        let mut z = hex("78dacb48cdc9c90700062c0215");
        z.truncate(6);
        let out = inflate(&z).unwrap();
        assert!(b"hello".starts_with(&out));
        assert!(inflate(b"\x00\x00").is_none());
    }

    #[test]
    fn undoes_png_up_rows() {
        // (a cross-reference stream's rows: /Columns 3 /Predictor 12)
        let rows = [2u8, 1, 0, 16, 2, 0, 1, 5];
        let p = Predictor {
            predictor: 12,
            columns: 3,
            ..Predictor::default()
        };
        assert_eq!(unpredict(&rows, p).unwrap(), [1, 0, 16, 1, 1, 21]);
    }
}
