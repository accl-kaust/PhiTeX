//! xpdf's font file readers (`fofi/`, xpdf 4.05 as TeX Live has it), as
//! far as `Gfx8BitFont` uses them for an 8-bit font's encoding: what kind
//! of file an embedded font is (`FoFiIdentifier::identifyStream`), and
//! the encoding a Type 1 file (`FoFiType1::getEncoding`) or a CFF file
//! (`FoFiType1C::getEncoding`) has. pdfTeX's font replacement in PDF
//! inclusion writes that encoding as the font's `/Differences`.
//!
//! An encoding is 256 glyph names, `None` where xpdf's table has `NULL`
//! (a name may be empty: a CFF string that is).

use alloc::vec;
use alloc::vec::Vec;

use crate::fofi_tables as tables;

/// An encoding by code.
pub type Encoding = Vec<Option<Vec<u8>>>;

/// A table of [`fofi_tables`](crate::fofi_tables) as an encoding.
#[must_use]
pub fn table(t: &[&str; 256]) -> Encoding {
    t.iter()
        .map(|s| (!s.is_empty()).then(|| s.as_bytes().to_vec()))
        .collect()
}

/// `FoFiIdentifierType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Type1Pfa,
    Type1Pfb,
    Cff8Bit,
    CffCid,
    TrueType,
    TrueTypeCollection,
    OpenTypeCff8Bit,
    OpenTypeCffCid,
    Unknown,
}

/// `StreamReader`: a stream read forward through a buffer of 1,024
/// bytes. A position before the buffer cannot be read again, and a read
/// past the stream's end consumes it (what the identification can see
/// depends on the order of its reads, as xpdf's).
struct Reader<'a> {
    data: &'a [u8],
    buf_pos: i32,
    buf_len: i32,
}

const READER_BUF: i32 = 1024;

impl Reader<'_> {
    /// The stream's next byte (`getChar`), at `buf_pos + buf_len`.
    fn next(&self) -> Option<u8> {
        usize::try_from(self.buf_pos + self.buf_len)
            .ok()
            .and_then(|i| self.data.get(i).copied())
    }

    /// `fillBuf`.
    fn fill(&mut self, pos: i32, len: i32) -> bool {
        if pos < 0 || !(0..=READER_BUF).contains(&len) || pos > i32::MAX - READER_BUF {
            return false;
        }
        if pos < self.buf_pos {
            return false;
        }
        if pos + len > self.buf_pos + READER_BUF {
            if pos < self.buf_pos + self.buf_len {
                self.buf_len -= pos - self.buf_pos;
                self.buf_pos = pos;
            } else {
                self.buf_pos += self.buf_len;
                self.buf_len = 0;
                while self.buf_pos < pos {
                    if self.next().is_none() {
                        return false;
                    }
                    self.buf_pos += 1;
                }
            }
        }
        while self.buf_pos + self.buf_len < pos + len {
            if self.next().is_none() {
                return false;
            }
            self.buf_len += 1;
        }
        true
    }

    /// The bytes at `pos` (filled).
    fn at(&self, pos: i32, n: i32) -> &[u8] {
        let a = usize::try_from(pos).unwrap_or(0);
        &self.data[a..a + usize::try_from(n).unwrap_or(0)]
    }

    fn byte(&mut self, pos: i32) -> i32 {
        if self.fill(pos, 1) {
            i32::from(self.at(pos, 1)[0])
        } else {
            -1
        }
    }

    fn u16be(&mut self, pos: i32) -> Option<i32> {
        self.fill(pos, 2).then(|| {
            let b = self.at(pos, 2);
            i32::from(b[0]) << 8 | i32::from(b[1])
        })
    }

    fn u32be(&mut self, pos: i32) -> Option<u32> {
        self.uvar_be(pos, 4)
    }

    fn u32le(&mut self, pos: i32) -> Option<u32> {
        self.fill(pos, 4)
            .then(|| u32::from_le_bytes(self.at(pos, 4).try_into().unwrap_or([0; 4])))
    }

    fn uvar_be(&mut self, pos: i32, size: i32) -> Option<u32> {
        if !(1..=4).contains(&size) || !self.fill(pos, size) {
            return None;
        }
        Some(
            self.at(pos, size)
                .iter()
                .fold(0u32, |v, &b| v << 8 | u32::from(b)),
        )
    }

    fn cmp(&mut self, pos: i32, s: &[u8]) -> bool {
        let n = i32::try_from(s.len()).unwrap_or(0);
        self.fill(pos, n) && self.at(pos, n) == s
    }
}

/// `FoFiIdentifier::identifyStream` on a stream's (decoded) bytes.
#[must_use]
pub fn identify(data: &[u8]) -> Kind {
    let mut r = Reader {
        data,
        buf_pos: 0,
        buf_len: 0,
    };
    // PFA
    if r.cmp(0, b"%!PS-AdobeFont-1") || r.cmp(0, b"%!FontType1") {
        return Kind::Type1Pfa;
    }
    // PFB
    if r.byte(0) == 0x80
        && r.byte(1) == 0x01
        && let Some(n) = r.u32le(2)
        && ((n >= 16 && r.cmp(6, b"%!PS-AdobeFont-1")) || (n >= 11 && r.cmp(6, b"%!FontType1")))
    {
        return Kind::Type1Pfb;
    }
    // TrueType
    let b = [r.byte(0), r.byte(1), r.byte(2), r.byte(3)];
    if b == [0x00, 0x01, 0x00, 0x00] || b == [0x74, 0x72, 0x75, 0x65] {
        return Kind::TrueType;
    }
    if b == [0x74, 0x74, 0x63, 0x66] {
        return Kind::TrueTypeCollection;
    }
    // OpenType
    if b == [0x4f, 0x54, 0x54, 0x4f] {
        return identify_open_type(&mut r);
    }
    // CFF
    if b[0] == 0x01 && b[1] == 0x00 {
        return identify_cff(&mut r, 0);
    }
    // (some tools embed CFF fonts with an extra whitespace char at the
    // beginning)
    if r.byte(1) == 0x01 && r.byte(2) == 0x00 {
        return identify_cff(&mut r, 1);
    }
    Kind::Unknown
}

/// `identifyOpenType`.
fn identify_open_type(r: &mut Reader<'_>) -> Kind {
    let Some(n_tables) = r.u16be(4) else {
        return Kind::Unknown;
    };
    for i in 0..n_tables {
        if r.cmp(12 + i * 16, b"CFF ") {
            if let Some(offset) = r.u32be(12 + i * 16 + 8)
                && offset < 0x7fff_ffff
            {
                return match identify_cff(r, offset.cast_signed()) {
                    Kind::Cff8Bit => Kind::OpenTypeCff8Bit,
                    Kind::CffCid => Kind::OpenTypeCffCid,
                    k => k,
                };
            }
            return Kind::Unknown;
        }
    }
    Kind::Unknown
}

/// `identifyCFF` (C's `int` arithmetic, wrapping).
fn identify_cff(r: &mut Reader<'_>, start: i32) -> Kind {
    if r.byte(start) != 0x01 || r.byte(start.wrapping_add(1)) != 0x00 {
        return Kind::Unknown;
    }
    let hdr_size = r.byte(start.wrapping_add(2));
    if hdr_size < 0 {
        return Kind::Unknown;
    }
    let off_size0 = r.byte(start.wrapping_add(3));
    if !(1..=4).contains(&off_size0) {
        return Kind::Unknown;
    }
    let mut pos = start.wrapping_add(hdr_size);
    if pos < 0 {
        return Kind::Unknown;
    }
    // the name index
    let Some(n) = r.u16be(pos) else {
        return Kind::Unknown;
    };
    if n == 0 {
        pos = pos.wrapping_add(2);
    } else {
        let off_size1 = r.byte(pos.wrapping_add(2));
        if !(1..=4).contains(&off_size1) {
            return Kind::Unknown;
        }
        let Some(offset1) = r.uvar_be(
            pos.wrapping_add(3).wrapping_add(n.wrapping_mul(off_size1)),
            off_size1,
        ) else {
            return Kind::Unknown;
        };
        if offset1 > 0x7fff_ffff {
            return Kind::Unknown;
        }
        pos = pos
            .wrapping_add(3)
            .wrapping_add((n + 1).wrapping_mul(off_size1))
            .wrapping_add(offset1.cast_signed())
            .wrapping_sub(1);
    }
    if pos < 0 {
        return Kind::Unknown;
    }
    // the top dict index
    let n = match r.u16be(pos) {
        Some(n) if n >= 1 => n,
        _ => return Kind::Unknown,
    };
    let off_size1 = r.byte(pos.wrapping_add(2));
    if !(1..=4).contains(&off_size1) {
        return Kind::Unknown;
    }
    let Some(offset0) = r.uvar_be(pos.wrapping_add(3), off_size1) else {
        return Kind::Unknown;
    };
    if offset0 > 0x7fff_ffff {
        return Kind::Unknown;
    }
    let Some(offset1) = r.uvar_be(pos.wrapping_add(3).wrapping_add(off_size1), off_size1) else {
        return Kind::Unknown;
    };
    if offset1 > 0x7fff_ffff || offset0 > offset1 {
        return Kind::Unknown;
    }
    let base = pos
        .wrapping_add(3)
        .wrapping_add((n + 1).wrapping_mul(off_size1))
        .wrapping_sub(1);
    let mut pos = base.wrapping_add(offset0.cast_signed());
    let end_pos = pos
        .wrapping_add(3)
        .wrapping_add((n + 1).wrapping_mul(off_size1))
        .wrapping_add(offset1.cast_signed())
        .wrapping_sub(1);
    if pos < 0 || end_pos < 0 || pos > end_pos {
        return Kind::Unknown;
    }
    // the top dict: a CID font's starts with <int> <int> <int> ROS
    while pos >= 0 && pos < end_pos {
        let b0 = r.byte(pos);
        pos = match b0 {
            0x1c => pos.wrapping_add(3),
            0x1d => pos.wrapping_add(5),
            0xf7..=0xfe => pos.wrapping_add(2),
            0x20..=0xf6 => pos.wrapping_add(1),
            _ => break,
        };
    }
    if pos.wrapping_add(1) < end_pos && r.byte(pos) == 12 && r.byte(pos.wrapping_add(1)) == 30 {
        Kind::CffCid
    } else {
        Kind::Cff8Bit
    }
}

/// `FoFiType1::undoPFB`: a PFB's segments joined.
fn undo_pfb(file: &[u8]) -> alloc::borrow::Cow<'_, [u8]> {
    if file.first() != Some(&0x80) {
        return alloc::borrow::Cow::Borrowed(file);
    }
    let mut out = Vec::with_capacity(file.len());
    let mut pos = 0usize;
    while file.get(pos) == Some(&0x80) {
        let Some(&kind) = file.get(pos + 1) else {
            break;
        };
        if !(1..=2).contains(&kind) {
            break;
        }
        let Some(len) = file.get(pos + 2..pos + 6) else {
            break;
        };
        let seg = u32::from_le_bytes(len.try_into().unwrap_or([0; 4]));
        pos += 6;
        let Some(end) = usize::try_from(seg).ok().and_then(|s| pos.checked_add(s)) else {
            break;
        };
        // (checkRegion: the segment within the file, its offset an int)
        if end > file.len() || seg > 0x7fff_ffff {
            break;
        }
        out.extend_from_slice(&file[pos..end]);
        pos = end;
    }
    alloc::borrow::Cow::Owned(out)
}

/// `FoFiType1::getNextLine`: the next line's start, `None` at the end.
fn next_line(file: &[u8], mut line: usize) -> Option<usize> {
    while line < file.len() && file[line] != b'\n' && file[line] != b'\r' {
        line += 1;
    }
    if line < file.len() && file[line] == b'\r' {
        line += 1;
    }
    if line < file.len() && file[line] == b'\n' {
        line += 1;
    }
    (line < file.len()).then_some(line)
}

/// `strtok`'s next token of `s` from `*at`, its delimiters `delims`.
fn strtok<'a>(s: &'a [u8], at: &mut usize, delims: &[u8]) -> Option<&'a [u8]> {
    let mut i = *at;
    while i < s.len() && delims.contains(&s[i]) {
        i += 1;
    }
    if i == s.len() {
        *at = i;
        return None;
    }
    let start = i;
    while i < s.len() && !delims.contains(&s[i]) {
        i += 1;
    }
    // (the delimiter is replaced by NUL, the next call starts after it)
    *at = if i < s.len() { i + 1 } else { i };
    Some(&s[start..i])
}

/// The encoding `FoFiType1::parse` finds in the first 100 lines: `None`
/// for none (xpdf's `NULL`), [`tables::FOFI_TYPE1_STANDARD`] for
/// `/Encoding StandardEncoding def`, else what an `/Encoding 256 array`'s
/// `dup` lines put.
#[must_use]
pub fn type1_encoding(file: &[u8]) -> Option<Encoding> {
    let file = undo_pfb(file);
    let file = &file[..];
    let len = file.len();
    let starts = |at: usize, s: &[u8]| at + s.len() <= len && &file[at..at + s.len()] == s;
    let mut got_matrix = false;
    let mut line = Some(0);
    let mut i = 1;
    // (the loop also stops when the name, encoding and matrix are all
    // found; with no encoding it goes on, whatever the name)
    while i <= 100 {
        let Some(l) = line else {
            break;
        };
        if starts(l, b"/Encoding StandardEncoding def") {
            return Some(table(&tables::FOFI_TYPE1_STANDARD));
        } else if starts(l, b"/Encoding 256 array") {
            return Some(type1_array(file, l));
        } else if !got_matrix && starts(l, b"/FontMatrix") {
            // (read where the line is; the next round moves on)
            got_matrix = true;
        } else {
            line = next_line(file, l);
        }
        i += 1;
    }
    None
}

/// An `/Encoding 256 array`'s `dup <code> /<name> put` lines, up to the
/// line ending in `def` (300 lines at most).
fn type1_array(file: &[u8], l: usize) -> Encoding {
    let mut enc: Encoding = vec![None; 256];
    let mut line = next_line(file, l);
    let mut j = 0;
    while j < 300 {
        let Some(ln) = line else {
            break;
        };
        let Some(line1) = next_line(file, ln) else {
            break;
        };
        // (strncpy: 255 bytes at most, to the first NUL)
        let raw = &file[ln..ln + (line1 - ln).min(255)];
        let buf = &raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())];
        let at = |p: usize| buf.get(p).copied().unwrap_or(0);
        let mut p = 0;
        while matches!(at(p), b' ' | b'\t') {
            p += 1;
        }
        let ends_with_dup = &file[ln - 4..ln] == b"dup\n" || &file[ln - 5..ln - 1] == b"dup\r";
        let starts_with_dup = buf[p..].starts_with(b"dup");
        if ends_with_dup || starts_with_dup {
            if starts_with_dup {
                p += 3;
            }
            loop {
                while matches!(at(p), b' ' | b'\t') {
                    p += 1;
                }
                let base: u32 = if at(p) == b'8' && at(p + 1) == b'#' {
                    p += 2;
                    8
                } else if at(p).is_ascii_digit() {
                    10
                } else {
                    break;
                };
                let mut code: u32 = 0;
                while at(p) >= b'0' && u32::from(at(p)) < u32::from(b'0') + base {
                    code = code
                        .wrapping_mul(base)
                        .wrapping_add(u32::from(at(p) - b'0'));
                    p += 1;
                }
                while matches!(at(p), b' ' | b'\t') {
                    p += 1;
                }
                if at(p) != b'/' {
                    break;
                }
                p += 1;
                let mut p2 = p;
                while !matches!(at(p2), 0 | b' ' | b'\t') {
                    p2 += 1;
                }
                if code < 256 {
                    enc[code as usize] = Some(buf[p..p2].to_vec());
                }
                p = p2;
                while matches!(at(p), b' ' | b'\t') {
                    p += 1;
                }
                if !buf[p..].starts_with(b"put") {
                    break;
                }
                p += 3;
                while matches!(at(p), b' ' | b'\t') {
                    p += 1;
                }
                if !buf[p..].starts_with(b"dup") {
                    break;
                }
                p += 3;
            }
        } else {
            let mut t = 0;
            if strtok(buf, &mut t, b" \t").is_some()
                && strtok(buf, &mut t, b" \t\n\r") == Some(b"def".as_slice())
            {
                break;
            }
        }
        j += 1;
        line = Some(line1);
    }
    enc
}

/// A CFF dict's operand or operator (`Type1COp`).
#[derive(Clone, Copy)]
enum Op {
    Operator(i32),
    Int(i32),
    Float(f64),
}

impl Op {
    /// `Type1COp::toInt`.
    fn to_int(self) -> i32 {
        match self {
            Op::Int(i) => i,
            #[allow(
                clippy::cast_possible_truncation,
                reason = "C's (int) of a double in range"
            )]
            Op::Float(f) if (-2e9..=2e9).contains(&f) => f as i32,
            _ => 0,
        }
    }
}

/// An INDEX (`Type1CIndex`).
#[derive(Clone, Copy, Default)]
struct Index {
    pos: i32,
    len: i32,
    off_size: i32,
    start: i32,
    end: i32,
}

/// `FoFiType1C`'s parse, as far as the encoding: its byte accessors
/// (`FoFiBase`) failing out of the file, `parsedOk` the flag they clear.
struct Cff<'a> {
    file: &'a [u8],
    ok: bool,
    ops: [Op; 49],
    n_ops: usize,
}

impl Cff<'_> {
    fn len(&self) -> i32 {
        i32::try_from(self.file.len()).unwrap_or(i32::MAX)
    }

    fn u8(&mut self, pos: i32) -> i32 {
        if let Some(&b) = usize::try_from(pos).ok().and_then(|p| self.file.get(p)) {
            i32::from(b)
        } else {
            self.ok = false;
            0
        }
    }

    fn u16be(&mut self, pos: i32) -> i32 {
        if !(0..i32::MAX).contains(&pos) || pos + 1 >= self.len() {
            self.ok = false;
            return 0;
        }
        let p = pos.cast_unsigned() as usize;
        i32::from(self.file[p]) << 8 | i32::from(self.file[p + 1])
    }

    fn uvar_be(&mut self, pos: i32, size: i32) -> u32 {
        if pos < 0 || pos > i32::MAX - size || pos + size > self.len() {
            self.ok = false;
            return 0;
        }
        let p = pos.cast_unsigned() as usize;
        (0..size.cast_unsigned() as usize).fold(0u32, |x, i| {
            (x << 8).wrapping_add(u32::from(self.file[p + i]))
        })
    }

    /// `getIndex`.
    fn index(&mut self, pos: i32) -> Index {
        let mut idx = Index {
            pos,
            len: self.u16be(pos),
            ..Index::default()
        };
        if idx.len == 0 {
            idx.start = pos.wrapping_add(2);
            idx.end = idx.start;
        } else {
            idx.off_size = self.u8(pos.wrapping_add(2));
            if !(1..=4).contains(&idx.off_size) {
                self.ok = false;
            }
            idx.start = pos
                .wrapping_add(3)
                .wrapping_add((idx.len + 1).wrapping_mul(idx.off_size))
                .wrapping_sub(1);
            if idx.start < 0 || idx.start >= self.len() {
                self.ok = false;
            }
            let v = self.uvar_be(
                pos.wrapping_add(3)
                    .wrapping_add(idx.len.wrapping_mul(idx.off_size)),
                idx.off_size,
            );
            idx.end = idx.start.cast_unsigned().wrapping_add(v).cast_signed();
            if idx.end < idx.start || idx.end > self.len() {
                self.ok = false;
            }
        }
        idx
    }

    /// `getIndexVal`: the `i`th value's position and length.
    fn index_val(&mut self, idx: &Index, i: i32) -> (i32, i32) {
        if i < 0 || i >= idx.len {
            self.ok = false;
            return (0, 0);
        }
        let at = |k: i32| {
            idx.pos
                .wrapping_add(3)
                .wrapping_add(k.wrapping_mul(idx.off_size))
        };
        let v0 = self.uvar_be(at(i), idx.off_size);
        let v1 = self.uvar_be(at(i + 1), idx.off_size);
        let pos0 = idx.start.cast_unsigned().wrapping_add(v0).cast_signed();
        let pos1 = idx.start.cast_unsigned().wrapping_add(v1).cast_signed();
        if pos0 < idx.start || pos0 > idx.end || pos1 <= idx.start || pos1 > idx.end || pos1 < pos0
        {
            self.ok = false;
        }
        (pos0, pos1.wrapping_sub(pos0))
    }

    /// The byte at `*pos`, `*pos` moved past it (`getU8(pos++, ok)`).
    fn next(&mut self, pos: &mut i32) -> i32 {
        let b = self.u8(*pos);
        *pos = pos.wrapping_add(1);
        b
    }

    /// `getOp` (in a dict, not a charstring).
    fn op(&mut self, mut pos: i32) -> i32 {
        let b0 = self.next(&mut pos);
        let op = match b0 {
            28 => {
                let x = self.next(&mut pos) << 8;
                let x = x | self.next(&mut pos);
                Op::Int(if x & 0x8000 != 0 { x | !0xffff } else { x })
            }
            29 => {
                let mut x: u32 = 0;
                for _ in 0..4 {
                    x = x << 8 | self.next(&mut pos).cast_unsigned();
                }
                Op::Int(x.cast_signed())
            }
            30 => {
                const NYB: &[u8; 15] = b"0123456789.ee -";
                let mut buf = Vec::with_capacity(65);
                let nyb = |n: i32| NYB[usize::try_from(n).unwrap_or(0)];
                loop {
                    let b1 = self.next(&mut pos);
                    let (nyb0, nyb1) = (b1 >> 4, b1 & 0x0f);
                    if nyb0 == 0xf {
                        break;
                    }
                    buf.push(nyb(nyb0));
                    if buf.len() == 64 {
                        break;
                    }
                    if nyb0 == 0xc {
                        buf.push(b'-');
                    }
                    if buf.len() == 64 || nyb1 == 0xf {
                        break;
                    }
                    buf.push(nyb(nyb1));
                    if buf.len() == 64 {
                        break;
                    }
                    if nyb1 == 0xc {
                        buf.push(b'-');
                    }
                    if buf.len() >= 64 {
                        break;
                    }
                }
                Op::Float(crate::pdfread::atof(&buf))
            }
            32..=246 => Op::Int(b0 - 139),
            247..=250 => Op::Int(((b0 - 247) << 8) + self.next(&mut pos) + 108),
            251..=254 => Op::Int(-((b0 - 251) << 8) - self.next(&mut pos) - 108),
            12 => Op::Operator(0x0c00 + self.next(&mut pos)),
            _ => Op::Operator(b0),
        };
        if self.n_ops < 49 {
            self.ops[self.n_ops] = op;
            self.n_ops += 1;
        }
        pos
    }

    /// The operator just read, if the last op read is one (`ops[nOps -
    /// 1]`), dropped from the ops.
    fn operator(&mut self) -> Option<i32> {
        match self.ops[self.n_ops.saturating_sub(1)] {
            Op::Operator(o) => {
                self.n_ops = self.n_ops.saturating_sub(1);
                Some(o)
            }
            _ => None,
        }
    }

    /// `getString`.
    fn string(&mut self, strings: &Index, sid: i32) -> Vec<u8> {
        if sid < 0 {
            Vec::new()
        } else if let Some(s) = usize::try_from(sid)
            .ok()
            .and_then(|s| tables::CFF_STD_STRINGS.get(s))
        {
            s.as_bytes().to_vec()
        } else {
            let (pos, len) = self.index_val(strings, sid - 391);
            if !self.ok {
                return Vec::new();
            }
            let n = usize::try_from(len.min(255)).unwrap_or(0);
            let p = usize::try_from(pos).unwrap_or(0);
            let s = &self.file[p..p + n];
            s[..s.iter().position(|&c| c == 0).unwrap_or(n)].to_vec()
        }
    }
}

/// The top dict's entries the encoding needs (`readTopDict`).
#[derive(Default)]
struct TopDict {
    first_op: Option<i32>,
    charset: i32,
    encoding: i32,
    char_strings: i32,
    private_size: i32,
    private_offset: i32,
}

/// The encoding `FoFiType1C::make` and `getEncoding` give a CFF file: an
/// 8-bit font's; `None` for a file that does not parse (`make` gives
/// `NULL`), a CID-keyed or synthetic font's (which has none). Either way
/// `Gfx8BitFont` then starts from `StandardEncoding`.
#[must_use]
pub fn type1c_encoding(file: &[u8]) -> Option<Encoding> {
    // (some tools embed Type 1C fonts with an extra whitespace char at
    // the beginning)
    let file = match file.first() {
        Some(&c) if c != 1 => &file[1..],
        _ => file,
    };
    let mut c = Cff {
        file,
        ok: true,
        ops: [Op::Int(0); 49],
        n_ops: 0,
    };
    let hdr = c.u8(2);
    let names = c.index(hdr);
    let top = c.index(names.end);
    let strings = c.index(top.end);
    c.index(strings.end);
    if !c.ok {
        return None;
    }
    c.index_val(&names, 0);
    if !c.ok {
        return None;
    }
    // readTopDict
    let mut d = TopDict::default();
    let (mut pos, len) = c.index_val(&top, 0);
    let end = pos.wrapping_add(len);
    c.n_ops = 0;
    while pos < end {
        pos = c.op(pos);
        if !c.ok {
            break;
        }
        if let Some(o) = c.operator() {
            d.first_op.get_or_insert(o);
            let int = |k: usize| c.ops[k].to_int();
            match o {
                0x000f => d.charset = int(0),
                0x0010 => d.encoding = int(0),
                0x0011 => d.char_strings = int(0),
                0x0012 => (d.private_size, d.private_offset) = (int(0), int(1)),
                _ => {}
            }
            c.n_ops = 0;
        }
    }
    if matches!(d.first_op, Some(0x0c1e | 0x0c14)) {
        return None;
    }
    // readPrivateDict: only whether its ops read within the file
    if d.private_offset != 0 && d.private_size != 0 {
        let mut pos = d.private_offset;
        let end = d.private_offset.wrapping_add(d.private_size);
        c.n_ops = 0;
        while pos < end {
            pos = c.op(pos);
            if !c.ok {
                break;
            }
            if c.operator().is_some() {
                c.n_ops = 0;
            }
        }
    }
    if !c.ok || d.char_strings <= 0 {
        return None;
    }
    let char_strings = c.index(d.char_strings);
    if !c.ok {
        return None;
    }
    let charset = cff_charset(&mut c, d.charset, char_strings.len)?;
    let n_glyphs = i32::try_from(charset.len()).unwrap_or(0);
    let enc = cff_encoding(&mut c, d.encoding, &charset, n_glyphs, &strings);
    c.ok.then_some(enc)
}

/// `readCharset`: the charset (its length the glyph count, cut to a
/// predefined charset's), `None` if it fails.
fn cff_charset(c: &mut Cff<'_>, offset: i32, n_glyphs: i32) -> Option<Vec<u16>> {
    let n = usize::try_from(n_glyphs).unwrap_or(0);
    match offset {
        0 => return Some(tables::CFF_ISO_ADOBE_CHARSET[..n.min(229)].to_vec()),
        1 => return Some(tables::CFF_EXPERT_CHARSET[..n.min(166)].to_vec()),
        2 => return Some(tables::CFF_EXPERT_SUBSET_CHARSET[..n.min(87)].to_vec()),
        _ => {}
    }
    let mut charset = vec![0u16; n];
    let mut pos = offset;
    let format = c.u8(pos);
    pos = pos.wrapping_add(1);
    let mut i = 1;
    match format {
        0 => {
            while i < n {
                charset[i] = u16::try_from(c.u16be(pos) & 0xffff).unwrap_or(0);
                pos = pos.wrapping_add(2);
                if !c.ok {
                    break;
                }
                i += 1;
            }
        }
        1 | 2 => {
            while i < n {
                let mut first = c.u16be(pos);
                pos = pos.wrapping_add(2);
                let n_left = if format == 1 {
                    let v = c.u8(pos);
                    pos = pos.wrapping_add(1);
                    v
                } else {
                    let v = c.u16be(pos);
                    pos = pos.wrapping_add(2);
                    v
                };
                if !c.ok {
                    break;
                }
                let mut j = 0;
                while j <= n_left && i < n {
                    charset[i] = u16::try_from(first & 0xffff).unwrap_or(0);
                    i += 1;
                    first += 1;
                    j += 1;
                }
            }
        }
        _ => {}
    }
    c.ok.then_some(charset)
}

/// `encoding[code] = copyString(getString(sid))`.
fn put(c: &mut Cff<'_>, enc: &mut Encoding, strings: &Index, code: i32, sid: i32) {
    let name = c.string(strings, sid);
    if let Some(e) = usize::try_from(code).ok().and_then(|k| enc.get_mut(k)) {
        *e = Some(name);
    }
}

/// `buildEncoding`.
fn cff_encoding(
    c: &mut Cff<'_>,
    offset: i32,
    charset: &[u16],
    n_glyphs: i32,
    strings: &Index,
) -> Encoding {
    match offset {
        0 => return table(&tables::FOFI_TYPE1_STANDARD),
        1 => return table(&tables::FOFI_TYPE1_EXPERT),
        _ => {}
    }
    let mut enc: Encoding = vec![None; 256];
    let mut pos = offset;
    let format = c.next(&mut pos);
    if !c.ok {
        return enc;
    }
    let kind = format & 0x7f;
    if kind == 0 {
        let n_codes = (1 + c.next(&mut pos)).min(n_glyphs);
        if !c.ok {
            return enc;
        }
        for i in 1..n_codes {
            let code = c.next(&mut pos);
            if !c.ok {
                return enc;
            }
            put(
                c,
                &mut enc,
                strings,
                code,
                i32::from(charset[usize::try_from(i).unwrap_or(0)]),
            );
        }
    } else if kind == 1 {
        let n_ranges = c.next(&mut pos);
        if !c.ok {
            return enc;
        }
        let mut n_codes = 1;
        for _ in 0..n_ranges {
            let mut code = c.next(&mut pos);
            let n_left = c.next(&mut pos);
            if !c.ok {
                return enc;
            }
            let mut j = 0;
            while j <= n_left && n_codes < n_glyphs {
                if code < 256 {
                    put(
                        c,
                        &mut enc,
                        strings,
                        code,
                        i32::from(charset[usize::try_from(n_codes).unwrap_or(0)]),
                    );
                }
                n_codes += 1;
                code += 1;
                j += 1;
            }
        }
    }
    if format & 0x80 != 0 {
        let n_sups = c.next(&mut pos);
        if !c.ok {
            return enc;
        }
        for _ in 0..n_sups {
            let code = c.next(&mut pos);
            if !c.ok {
                return enc;
            }
            let sid = c.u16be(pos);
            pos = pos.wrapping_add(2);
            if !c.ok {
                return enc;
            }
            put(c, &mut enc, strings, code, sid);
        }
    }
    enc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(e: &Encoding, c: usize) -> Option<&[u8]> {
        e[c].as_deref()
    }

    #[test]
    fn reads_a_type1_encoding_array() {
        let f = b"%!PS-AdobeFont-1.0: CMR10 003.002\n/FontName /CMR10 def\n\
/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n\
dup 65 /A put\n dup 8#102 /B put dup 67 /C put\ndup\n68 /D put\nreadonly def\n/X 1 def\n";
        assert_eq!(identify(f), Kind::Type1Pfa);
        let e = type1_encoding(f).unwrap();
        assert_eq!(name(&e, 65), Some(&b"A"[..]));
        assert_eq!(name(&e, 66), Some(&b"B"[..]));
        assert_eq!(name(&e, 67), Some(&b"C"[..]));
        // (a code on the line after `dup`)
        assert_eq!(name(&e, 68), Some(&b"D"[..]));
        assert_eq!(name(&e, 0), None);
    }

    #[test]
    fn reads_the_standard_encoding_and_none() {
        let f = b"%!FontType1\n/Encoding StandardEncoding def\n";
        let e = type1_encoding(f).unwrap();
        assert_eq!(name(&e, 65), Some(&b"A"[..]));
        assert_eq!(name(&e, 0xe1), Some(&b"AE"[..]));
        assert!(type1_encoding(b"%!FontType1\n/FontName /X def\n").is_none());
    }

    #[test]
    fn identifies_font_files() {
        assert_eq!(
            identify(b"\x80\x01\x20\0\0\0%!PS-AdobeFont-1.0"),
            Kind::Type1Pfb
        );
        assert_eq!(identify(b"\0\x01\0\0rest"), Kind::TrueType);
        assert_eq!(identify(b"OTTO\0\0"), Kind::Unknown);
        assert_eq!(identify(b"nothing"), Kind::Unknown);
    }

    #[test]
    fn reads_a_cff_encoding() {
        // a CFF of one glyph besides .notdef, "A" (SID 34) at code 65:
        // header, Name INDEX ("X"), Top DICT INDEX, String INDEX (empty),
        // Global Subr INDEX (empty), CharStrings INDEX, charset, encoding
        let mut f = vec![1u8, 0, 4, 1];
        f.extend_from_slice(&[0, 1, 1, 1, 2, b'X']);
        // the top dict: the charset's, the encoding's and the
        // CharStrings' offsets (no private dict)
        let (cs_off, charset_off, enc_off) = (40u8, 50u8, 60u8);
        let dict = [139 + charset_off, 15, 139 + enc_off, 16, 139 + cs_off, 17];
        f.extend_from_slice(&[0, 1, 1, 1, 7]);
        f.extend_from_slice(&dict);
        f.extend_from_slice(&[0, 0]); // strings
        f.extend_from_slice(&[0, 0]); // gsubrs
        f.resize(usize::from(cs_off), 0);
        // CharStrings: two glyphs of one byte each (endchar)
        f.extend_from_slice(&[0, 2, 1, 1, 2, 3, 14, 14]);
        f.resize(usize::from(charset_off), 0);
        f.extend_from_slice(&[0, 0, 34]); // format 0: glyph 1 is SID 34 ("A")
        f.resize(usize::from(enc_off), 0);
        f.extend_from_slice(&[0, 1, 65]); // format 0: code 65 for glyph 1
        assert_eq!(identify(&f), Kind::Cff8Bit);
        let e = type1c_encoding(&f).unwrap();
        assert_eq!(name(&e, 65), Some(&b"A"[..]));
        assert_eq!(name(&e, 66), None);
    }
}
