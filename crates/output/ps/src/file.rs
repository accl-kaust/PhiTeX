//! Files: the program being read, strings read as files, and decoding
//! filters over them. A filter decodes its source to its end of data at
//! once and moves the source past what it read, as Ghostscript's
//! read-ahead does in effect (an image's data ends with the filter's EOD,
//! the program goes on after it).

use alloc::rc::Rc;
use alloc::vec::Vec;

/// A readable file.
pub struct File {
    pub data: Rc<Vec<u8>>,
    pub pos: usize,
    pub closed: bool,
    /// What a `DCTDecode` filter read: its JPEG data as it is (passed
    /// through to the PDF, `PassThroughJPEGImages`).
    pub dct: Option<Rc<Vec<u8>>>,
    /// Bytes written (`%stdout`, `%stderr`, written files): dropped.
    pub write: bool,
    /// DSC comments are given to the interpreter (the EPS file in EPS
    /// mode: gs_epsf.ps's ProcessDSCComment).
    pub dsc: bool,
}

impl File {
    pub fn new(data: Rc<Vec<u8>>) -> File {
        File {
            data,
            pos: 0,
            closed: false,
            dct: None,
            write: false,
            dsc: false,
        }
    }
    pub fn writer() -> File {
        File {
            data: Rc::new(Vec::new()),
            pos: 0,
            closed: false,
            dct: None,
            write: true,
            dsc: false,
        }
    }
    pub fn next(&mut self) -> Option<u8> {
        if self.closed {
            return None;
        }
        let c = self.data.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
    pub fn peek(&self) -> Option<u8> {
        if self.closed {
            return None;
        }
        self.data.get(self.pos).copied()
    }
    pub fn rest(&self) -> &[u8] {
        if self.closed {
            return &[];
        }
        &self.data[self.pos.min(self.data.len())..]
    }
    pub fn bytes_available(&self) -> i64 {
        if self.closed {
            -1
        } else {
            (self.data.len() - self.pos.min(self.data.len())) as i64
        }
    }
}

/// A decoding filter's name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    AsciiHex,
    Ascii85,
    RunLength,
    Lzw,
    Flate,
    Dct,
    SubFile,
    Null,
}

pub struct FilterParams {
    /// SubFileDecode's EOD count and string.
    pub eod_count: i64,
    pub eod_string: Vec<u8>,
    /// LZWDecode's EarlyChange.
    pub early_change: bool,
    /// Predictor parameters (Flate/LZW).
    pub predictor: i64,
    pub colors: i64,
    pub bpc: i64,
    pub columns: i64,
}

impl Default for FilterParams {
    fn default() -> Self {
        FilterParams {
            eod_count: 0,
            eod_string: Vec::new(),
            early_change: true,
            predictor: 1,
            colors: 1,
            bpc: 8,
            columns: 1,
        }
    }
}

fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

/// Decodes `src` (from its start) with `f`: the decoded bytes and how
/// many bytes of `src` were read.
pub fn decode(f: Filter, src: &[u8], p: &FilterParams) -> (Vec<u8>, usize) {
    match f {
        Filter::AsciiHex => {
            let mut out = Vec::new();
            let mut hi: Option<u8> = None;
            let mut i = 0;
            while i < src.len() {
                let c = src[i];
                i += 1;
                let d = match c {
                    b'0'..=b'9' => c - b'0',
                    b'a'..=b'f' => c - b'a' + 10,
                    b'A'..=b'F' => c - b'A' + 10,
                    b'>' => break,
                    _ => continue,
                };
                match hi.take() {
                    None => hi = Some(d),
                    Some(h) => out.push(h << 4 | d),
                }
            }
            if let Some(h) = hi {
                out.push(h << 4);
            }
            (out, i)
        }
        Filter::Ascii85 => {
            let mut out = Vec::new();
            let mut g = [0u32; 5];
            let mut n = 0;
            let mut i = 0;
            while i < src.len() {
                let c = src[i];
                i += 1;
                if is_ws(c) {
                    continue;
                }
                if c == b'~' {
                    // (`~>`: the end)
                    if src.get(i) == Some(&b'>') {
                        i += 1;
                    }
                    break;
                }
                if c == b'z' && n == 0 {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                    continue;
                }
                if !(b'!'..=b'u').contains(&c) {
                    continue;
                }
                g[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    let v = g.iter().fold(0u64, |a, &d| a * 85 + u64::from(d)) as u32;
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            if n > 1 {
                for k in g.iter_mut().skip(n) {
                    *k = 84;
                }
                let v = g.iter().fold(0u64, |a, &d| a * 85 + u64::from(d)) as u32;
                out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
            }
            (out, i)
        }
        Filter::RunLength => {
            let mut out = Vec::new();
            let mut i = 0;
            while i < src.len() {
                let l = src[i];
                i += 1;
                if l == 128 {
                    break;
                }
                if l < 128 {
                    let n = usize::from(l) + 1;
                    let e = (i + n).min(src.len());
                    out.extend_from_slice(&src[i..e]);
                    i = e;
                } else {
                    let n = 257 - usize::from(l);
                    if let Some(&b) = src.get(i) {
                        out.extend(core::iter::repeat_n(b, n));
                    }
                    i += 1;
                }
            }
            (out, i.min(src.len()))
        }
        Filter::Flate => {
            let mut out = Vec::new();
            let used = if src.len() >= 2 {
                phitex_codecs::inflate::inflate_raw_len(&src[2..], &mut out)
                    .map_or(src.len(), |n| (n + 2 + 4).min(src.len()))
            } else {
                src.len()
            };
            (predict(out, p), used)
        }
        Filter::Lzw => {
            let (out, used) = lzw(src, p.early_change);
            (predict(out, p), used)
        }
        Filter::Dct => {
            // (to the JPEG's end-of-image marker)
            let mut i = 2;
            while i + 1 < src.len() {
                if src[i] == 0xff && src[i + 1] == 0xd9 {
                    i += 2;
                    break;
                }
                i += 1;
            }
            let i = i.min(src.len());
            (src[..i].to_vec(), i)
        }
        Filter::SubFile => {
            if p.eod_string.is_empty() {
                if p.eod_count == 0 {
                    return (src.to_vec(), src.len());
                }
                let n = usize::try_from(p.eod_count).unwrap_or(0).min(src.len());
                return (src[..n].to_vec(), n);
            }
            // (the data before the eod_count'th occurrence of the string)
            let s = &p.eod_string;
            let mut count = 0;
            let mut i = 0;
            while i + s.len() <= src.len() {
                if &src[i..i + s.len()] == s.as_slice() {
                    if count == p.eod_count {
                        return (src[..i].to_vec(), i + s.len());
                    }
                    count += 1;
                    i += s.len();
                } else {
                    i += 1;
                }
            }
            (src.to_vec(), src.len())
        }
        Filter::Null => (src.to_vec(), src.len()),
    }
}

/// The PNG and TIFF predictors of Flate and LZW data.
fn predict(d: Vec<u8>, p: &FilterParams) -> Vec<u8> {
    if p.predictor < 2 {
        return d;
    }
    let colors = p.colors.max(1) as usize;
    let bpc = p.bpc.max(1) as usize;
    let cols = p.columns.max(1) as usize;
    let bpp = (colors * bpc).div_ceil(8);
    let row = (colors * bpc * cols).div_ceil(8);
    if p.predictor == 2 {
        let mut out = d;
        if bpc == 8 {
            for r in out.chunks_mut(row) {
                for i in bpp..r.len() {
                    r[i] = r[i].wrapping_add(r[i - bpp]);
                }
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(d.len());
    let mut prev = alloc::vec![0u8; row];
    for chunk in d.chunks(row + 1) {
        if chunk.is_empty() {
            break;
        }
        let t = chunk[0];
        let mut cur: Vec<u8> = chunk[1..].to_vec();
        cur.resize(row, 0);
        for i in 0..row {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            cur[i] = match t {
                1 => cur[i].wrapping_add(a),
                2 => cur[i].wrapping_add(b),
                3 => cur[i].wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                4 => {
                    let pp = i16::from(a) + i16::from(b) - i16::from(c);
                    let (pa, pb, pc) = (
                        (pp - i16::from(a)).abs(),
                        (pp - i16::from(b)).abs(),
                        (pp - i16::from(c)).abs(),
                    );
                    let pr = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    cur[i].wrapping_add(pr)
                }
                _ => cur[i],
            };
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    out
}

fn lzw(src: &[u8], early: bool) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut table: Vec<Vec<u8>> = (0..=255u8).map(|b| alloc::vec![b]).collect();
    table.push(Vec::new());
    table.push(Vec::new());
    let mut bits = 9;
    let mut buf: u32 = 0;
    let mut nbuf = 0;
    let mut i = 0;
    let mut prev: Option<Vec<u8>> = None;
    loop {
        while nbuf < bits {
            let Some(&b) = src.get(i) else {
                return (out, i);
            };
            i += 1;
            buf = buf << 8 | u32::from(b);
            nbuf += 8;
        }
        let code = ((buf >> (nbuf - bits)) & ((1 << bits) - 1)) as usize;
        nbuf -= bits;
        if code == 256 {
            table.truncate(258);
            bits = 9;
            prev = None;
            continue;
        }
        if code == 257 {
            return (out, i);
        }
        let entry = if code < table.len() {
            table[code].clone()
        } else if let Some(p) = &prev {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            return (out, i);
        };
        out.extend_from_slice(&entry);
        if let Some(p) = prev {
            let mut e = p;
            e.push(entry[0]);
            table.push(e);
        }
        prev = Some(entry);
        let n = table.len() + usize::from(early);
        bits = if n >= 2048 {
            12
        } else if n >= 1024 {
            11
        } else if n >= 512 {
            10
        } else {
            9
        };
    }
}
