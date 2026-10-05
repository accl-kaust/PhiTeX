//! A file read in memory (C's `FILE *` as dvipdfm-x uses it), with
//! numbers.c's readers and mfileio.c's line readers.

use alloc::sync::Arc;
use alloc::vec::Vec;

/// A file's bytes and a position.
#[derive(Clone, Debug)]
pub struct MemFile {
    pub data: Arc<[u8]>,
    pub pos: usize,
    /// The name it was found as (the resolved path).
    pub name: Vec<u8>,
}

impl MemFile {
    #[must_use]
    pub fn new(data: Arc<[u8]>, name: &[u8]) -> Self {
        MemFile {
            data,
            pos: 0,
            name: name.to_vec(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// `fgetc`: the next byte, or -1 at the end.
    pub fn getc(&mut self) -> i32 {
        if self.pos < self.data.len() {
            let c = self.data[self.pos];
            self.pos += 1;
            i32::from(c)
        } else {
            -1
        }
    }
    /// `ungetc` of the byte just read.
    pub fn ungetc(&mut self) {
        if self.pos > 0 {
            self.pos -= 1;
        }
    }
    /// `feof` after a read past the end.
    #[must_use]
    pub fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// `fread`: up to `n` bytes.
    pub fn read(&mut self, n: usize) -> &[u8] {
        let a = self.pos.min(self.data.len());
        let b = (self.pos + n).min(self.data.len());
        self.pos = b;
        &self.data[a..b]
    }
    /// `fread` into `buf`: how many bytes were read.
    pub fn read_into(&mut self, buf: &mut [u8]) -> usize {
        let s = self.read(buf.len());
        let n = s.len();
        buf[..n].copy_from_slice(&self.data[self.pos - n..self.pos]);
        n
    }

    pub fn seek_absolute(&mut self, pos: usize) {
        self.pos = pos;
    }
    pub fn seek_relative(&mut self, d: isize) {
        self.pos = self.pos.saturating_add_signed(d);
    }
    pub fn seek_end(&mut self) {
        self.pos = self.data.len();
    }
    pub fn rewind(&mut self) {
        self.pos = 0;
    }
    #[must_use]
    pub fn tell(&self) -> usize {
        self.pos
    }

    /// `get_unsigned_byte`: panics at the end ("File ended prematurely").
    pub fn get_unsigned_byte(&mut self) -> u8 {
        let c = self.getc();
        assert!(c >= 0, "File ended prematurely");
        c as u8
    }
    pub fn skip_bytes(&mut self, n: usize) {
        for _ in 0..n {
            self.get_unsigned_byte();
        }
    }
    pub fn get_signed_byte(&mut self) -> i8 {
        self.get_unsigned_byte() as i8
    }
    pub fn get_unsigned_pair(&mut self) -> u16 {
        let a = u16::from(self.get_unsigned_byte());
        (a << 8) | u16::from(self.get_unsigned_byte())
    }
    pub fn get_signed_pair(&mut self) -> i16 {
        self.get_unsigned_pair() as i16
    }
    pub fn get_unsigned_triple(&mut self) -> u32 {
        let mut t = 0u32;
        for _ in 0..3 {
            t = (t << 8) | u32::from(self.get_unsigned_byte());
        }
        t
    }
    pub fn get_signed_triple(&mut self) -> i32 {
        let mut t = i32::from(self.get_signed_byte());
        for _ in 0..2 {
            t = (t << 8) | i32::from(self.get_unsigned_byte());
        }
        t
    }
    pub fn get_signed_quad(&mut self) -> i32 {
        self.get_unsigned_quad() as i32
    }
    pub fn get_unsigned_quad(&mut self) -> u32 {
        let mut q = 0u32;
        for _ in 0..4 {
            q = (q << 8) | u32::from(self.get_unsigned_byte());
        }
        q
    }
    /// `get_unsigned_num(file, num)`: `num + 1` bytes; four are signed.
    pub fn get_unsigned_num(&mut self, num: u8) -> i32 {
        let mut v = i32::from(self.get_unsigned_byte());
        if num == 3 && v > 0x7f {
            v -= 0x100;
        }
        for _ in 0..num.min(3) {
            v = (v << 8) | i32::from(self.get_unsigned_byte());
        }
        v
    }
    /// `get_positive_quad`.
    pub fn get_positive_quad(&mut self, kind: &str, name: &str) -> u32 {
        let v = self.get_signed_quad();
        assert!(v >= 0, "Bad {kind}: negative {name}: {v}");
        v as u32
    }

    /// `mfgets`: a line without its end (at most `length - 1` bytes),
    /// or none at the end of the file.
    pub fn mfgets(&mut self, length: usize) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        let mut ch = 0;
        while line.len() + 1 < length {
            ch = self.getc();
            if ch < 0 || ch == i32::from(b'\n') || ch == i32::from(b'\r') {
                break;
            }
            line.push(ch as u8);
        }
        if ch < 0 && line.is_empty() {
            return None;
        }
        if ch == i32::from(b'\r') {
            let d = self.getc();
            if d >= 0 && d != i32::from(b'\n') {
                self.ungetc();
            }
        }
        Some(line)
    }

    /// `mfreadln`: a line without its end; `None` at the end of the file
    /// (C's -1), `Some(Err(()))` for a line longer than `size` (C's -2).
    pub fn mfreadln(&mut self, size: usize) -> Option<Result<Vec<u8>, ()>> {
        let mut line = Vec::new();
        let mut c;
        loop {
            c = self.getc();
            if c < 0 || c == i32::from(b'\n') || c == i32::from(b'\r') {
                break;
            }
            if line.len() >= size {
                return Some(Err(()));
            }
            line.push(c as u8);
        }
        if c < 0 && line.is_empty() {
            return None;
        }
        if c == i32::from(b'\r') {
            let d = self.getc();
            if d >= 0 && d != i32::from(b'\n') {
                self.ungetc();
            }
        }
        Some(Ok(line))
    }
}

/// `sget_unsigned_pair`.
#[must_use]
pub fn sget_unsigned_pair(s: &[u8]) -> u16 {
    (u16::from(s[0]) << 8) | u16::from(s[1])
}

/// `sqxfw` (numbers.c): a scaled times a fix word, as TeX does it.
#[must_use]
pub fn sqxfw(sq: i32, fw: i32) -> i32 {
    let mut sign = 1;
    let (mut sq, mut fw) = (sq, fw);
    if sq < 0 {
        sign = -sign;
        sq = -sq;
    }
    if fw < 0 {
        sign = -sign;
        fw = -fw;
    }
    let a = (sq as u32) >> 16;
    let b = (sq as u32) & 0xffff;
    let c = (fw as u32) >> 16;
    let d = (fw as u32) & 0xffff;
    let ad = a.wrapping_mul(d);
    let bd = b.wrapping_mul(d);
    let bc = b.wrapping_mul(c);
    let ac = a.wrapping_mul(c);
    let e = bd >> 16;
    let f = ad >> 16;
    let g = ad & 0xffff;
    let h = bc >> 16;
    let i = bc & 0xffff;
    let j = ac >> 16;
    let k = ac & 0xffff;
    let mut result = (e.wrapping_add(g).wrapping_add(i).wrapping_add(1 << 3)) >> 4;
    result = result.wrapping_add((f.wrapping_add(h).wrapping_add(k)) << 12);
    result = result.wrapping_add(j << 28);
    let result = result as i32;
    if sign > 0 {
        result
    } else {
        result.wrapping_neg()
    }
}
