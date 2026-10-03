//! writet1.c: embedding Type 1 fonts, whole or subsetted.
//!
//! The font file is read line by line as pdfTeX reads it (the same
//! whitespace normalization, eexec decryption, charstring parsing), so
//! the embedded bytes — and the `/Length1`, `/Length2` of the stream —
//! come out the same. Everything is a function of the file and the glyph
//! set; nothing touches the engine but the warnings it returns.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::enc::NOTDEF;

/// The standard encoding's glyph names (`standard_glyph_names`).
const STANDARD_GLYPH_NAMES: [&str; 256] = {
    let mut n = [".notdef"; 256];
    let names: [(usize, &str); 149] = [
        (0x20, "space"),
        (0x21, "exclam"),
        (0x22, "quotedbl"),
        (0x23, "numbersign"),
        (0x24, "dollar"),
        (0x25, "percent"),
        (0x26, "ampersand"),
        (0x27, "quoteright"),
        (0x28, "parenleft"),
        (0x29, "parenright"),
        (0x2a, "asterisk"),
        (0x2b, "plus"),
        (0x2c, "comma"),
        (0x2d, "hyphen"),
        (0x2e, "period"),
        (0x2f, "slash"),
        (0x30, "zero"),
        (0x31, "one"),
        (0x32, "two"),
        (0x33, "three"),
        (0x34, "four"),
        (0x35, "five"),
        (0x36, "six"),
        (0x37, "seven"),
        (0x38, "eight"),
        (0x39, "nine"),
        (0x3a, "colon"),
        (0x3b, "semicolon"),
        (0x3c, "less"),
        (0x3d, "equal"),
        (0x3e, "greater"),
        (0x3f, "question"),
        (0x40, "at"),
        (0x41, "A"),
        (0x42, "B"),
        (0x43, "C"),
        (0x44, "D"),
        (0x45, "E"),
        (0x46, "F"),
        (0x47, "G"),
        (0x48, "H"),
        (0x49, "I"),
        (0x4a, "J"),
        (0x4b, "K"),
        (0x4c, "L"),
        (0x4d, "M"),
        (0x4e, "N"),
        (0x4f, "O"),
        (0x50, "P"),
        (0x51, "Q"),
        (0x52, "R"),
        (0x53, "S"),
        (0x54, "T"),
        (0x55, "U"),
        (0x56, "V"),
        (0x57, "W"),
        (0x58, "X"),
        (0x59, "Y"),
        (0x5a, "Z"),
        (0x5b, "bracketleft"),
        (0x5c, "backslash"),
        (0x5d, "bracketright"),
        (0x5e, "asciicircum"),
        (0x5f, "underscore"),
        (0x60, "quoteleft"),
        (0x61, "a"),
        (0x62, "b"),
        (0x63, "c"),
        (0x64, "d"),
        (0x65, "e"),
        (0x66, "f"),
        (0x67, "g"),
        (0x68, "h"),
        (0x69, "i"),
        (0x6a, "j"),
        (0x6b, "k"),
        (0x6c, "l"),
        (0x6d, "m"),
        (0x6e, "n"),
        (0x6f, "o"),
        (0x70, "p"),
        (0x71, "q"),
        (0x72, "r"),
        (0x73, "s"),
        (0x74, "t"),
        (0x75, "u"),
        (0x76, "v"),
        (0x77, "w"),
        (0x78, "x"),
        (0x79, "y"),
        (0x7a, "z"),
        (0x7b, "braceleft"),
        (0x7c, "bar"),
        (0x7d, "braceright"),
        (0x7e, "asciitilde"),
        (0xa1, "exclamdown"),
        (0xa2, "cent"),
        (0xa3, "sterling"),
        (0xa4, "fraction"),
        (0xa5, "yen"),
        (0xa6, "florin"),
        (0xa7, "section"),
        (0xa8, "currency"),
        (0xa9, "quotesingle"),
        (0xaa, "quotedblleft"),
        (0xab, "guillemotleft"),
        (0xac, "guilsinglleft"),
        (0xad, "guilsinglright"),
        (0xae, "fi"),
        (0xaf, "fl"),
        (0xb1, "endash"),
        (0xb2, "dagger"),
        (0xb3, "daggerdbl"),
        (0xb4, "periodcentered"),
        (0xb6, "paragraph"),
        (0xb7, "bullet"),
        (0xb8, "quotesinglbase"),
        (0xb9, "quotedblbase"),
        (0xba, "quotedblright"),
        (0xbb, "guillemotright"),
        (0xbc, "ellipsis"),
        (0xbd, "perthousand"),
        (0xbf, "questiondown"),
        (0xc1, "grave"),
        (0xc2, "acute"),
        (0xc3, "circumflex"),
        (0xc4, "tilde"),
        (0xc5, "macron"),
        (0xc6, "breve"),
        (0xc7, "dotaccent"),
        (0xc8, "dieresis"),
        (0xca, "ring"),
        (0xcb, "cedilla"),
        (0xcd, "hungarumlaut"),
        (0xce, "ogonek"),
        (0xcf, "caron"),
        (0xd0, "emdash"),
        (0xe1, "AE"),
        (0xe3, "ordfeminine"),
        (0xe8, "Lslash"),
        (0xe9, "Oslash"),
        (0xea, "OE"),
        (0xeb, "ordmasculine"),
        (0xf1, "ae"),
        (0xf5, "dotlessi"),
        (0xf8, "lslash"),
        (0xf9, "oslash"),
        (0xfa, "oe"),
        (0xfb, "germandbls"),
    ];
    let mut i = 0;
    while i < names.len() {
        n[names[i].0] = names[i].1;
        i += 1;
    }
    n
};

/// The font dimension keys (`font_key`): what the font descriptor calls
/// them, and what the font file does.
pub(crate) const ASCENT: usize = 0;
pub(crate) const CAPHEIGHT: usize = 1;
pub(crate) const DESCENT: usize = 2;
pub(crate) const ITALIC_ANGLE: usize = 3;
pub(crate) const STEMV: usize = 4;
pub(crate) const XHEIGHT: usize = 5;
pub(crate) const FONTBBOX1: usize = 6;
pub(crate) const FONTNAME: usize = 10;
pub(crate) const FONT_KEYS: [(&str, &str); 11] = [
    ("Ascent", "Ascender"),
    ("CapHeight", "CapHeight"),
    ("Descent", "Descender"),
    ("ItalicAngle", "ItalicAngle"),
    ("StemV", "StdVW"),
    ("XHeight", "XHeight"),
    ("FontBBox", "FontBBox"),
    ("", ""),
    ("", ""),
    ("", ""),
    ("FontName", "FontName"),
];

/// What `writet1` needs of the font descriptor, and what it changes.
#[derive(Debug)]
pub(crate) struct T1Job<'a> {
    pub data: &'a [u8],
    pub subsetted: bool,
    pub slant: i32,
    pub extend: i32,
    /// `fd->gl_tree` (grows by `seac` components).
    pub glyphs: BTreeSet<Vec<u8>>,
    /// `fd->tx_tree`: character codes of non-reencoded fonts.
    pub codes: BTreeSet<u8>,
    /// `fd->all_glyphs`: every glyph kept (an included PDF's font).
    pub all_glyphs: bool,
    pub fontname: Vec<u8>,
    pub font_dim: [(i32, bool); 11],
    /// Subset tags already given (`st_tree`).
    pub tags: &'a mut BTreeSet<[u8; 6]>,
    /// `lastargOtherSubr3` (static in pdfTeX).
    pub last_arg_other_subr3: &'a mut i32,
}

/// The result: the stream's bytes and lengths.
#[derive(Clone, Debug, Default)]
pub(crate) struct T1Out {
    pub bytes: Vec<u8>,
    pub length1: usize,
    pub length2: usize,
    pub subset_tag: Option<[u8; 6]>,
    /// The font's own encoding (`builtin_glyph_names`), when subsetted.
    pub builtin: Option<Vec<Vec<u8>>>,
    pub warnings: Vec<Vec<u8>>,
}

const T1_C1: u16 = 52845;
const T1_C2: u16 = 22719;

const CS_HSTEM: usize = 1;
const CS_VSTEM: usize = 3;
const CS_VMOVETO: usize = 4;
const CS_RLINETO: usize = 5;
const CS_HLINETO: usize = 6;
const CS_VLINETO: usize = 7;
const CS_RRCURVETO: usize = 8;
const CS_CLOSEPATH: usize = 9;
const CS_CALLSUBR: usize = 10;
const CS_RETURN: usize = 11;
const CS_ESCAPE: usize = 12;
const CS_HSBW: usize = 13;
const CS_ENDCHAR: usize = 14;
const CS_RMOVETO: usize = 21;
const CS_HMOVETO: usize = 22;
const CS_VHCURVETO: usize = 30;
const CS_HVCURVETO: usize = 31;
const CS_1BYTE_MAX: usize = CS_HVCURVETO + 1;
const CS_DOTSECTION: usize = CS_1BYTE_MAX;
const CS_VSTEM3: usize = CS_1BYTE_MAX + 1;
const CS_HSTEM3: usize = CS_1BYTE_MAX + 2;
const CS_SEAC: usize = CS_1BYTE_MAX + 6;
const CS_SBW: usize = CS_1BYTE_MAX + 7;
const CS_DIV: usize = CS_1BYTE_MAX + 12;
const CS_CALLOTHERSUBR: usize = CS_1BYTE_MAX + 16;
const CS_POP: usize = CS_1BYTE_MAX + 17;
const CS_SETCURRENTPOINT: usize = CS_1BYTE_MAX + 33;
const CS_MAX: usize = CS_SETCURRENTPOINT + 1;

/// `cc_tab`: (number of arguments, from the bottom, clears the stack).
fn cc(b: usize) -> Option<(usize, bool, bool)> {
    Some(match b {
        CS_HSTEM | CS_VSTEM | CS_RLINETO | CS_HSBW | CS_RMOVETO | CS_SETCURRENTPOINT => {
            (2, true, true)
        }
        CS_VMOVETO | CS_HLINETO | CS_VLINETO | CS_HMOVETO => (1, true, true),
        CS_RRCURVETO | CS_VSTEM3 | CS_HSTEM3 => (6, true, true),
        CS_CLOSEPATH | CS_ENDCHAR | CS_DOTSECTION => (0, false, true),
        CS_CALLSUBR => (1, false, false),
        CS_RETURN | CS_CALLOTHERSUBR | CS_POP => (0, false, false),
        CS_VHCURVETO | CS_HVCURVETO | CS_SBW => (4, true, true),
        CS_SEAC => (5, true, true),
        CS_DIV => (2, false, false),
        _ => return None,
    })
}

/// A charstring or subroutine (`cs_entry`).
#[derive(Clone, Debug, Default)]
struct Cs {
    name: Vec<u8>,
    data: Vec<u8>,
    cslen: usize,
    used: bool,
    valid: bool,
}

/// A fatal error (`pdftex_fail`'s message).
pub(crate) type Fail = Vec<u8>;

fn fail<T>(m: String) -> Result<T, Fail> {
    Err(m.into_bytes())
}

/// C's `strtod` prefix, as `sscanf("%g")` reads it: the value and the
/// length read.
fn scan_float(s: &[u8]) -> Option<(f64, usize)> {
    let mut i = 0;
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r' | 11 | 12) {
        i += 1;
    }
    let start = i;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let digits = i;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    let mut n = i - digits;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let f = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        n += i - f;
    }
    if n == 0 {
        return None;
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        if j < s.len() && s[j].is_ascii_digit() {
            while j < s.len() && s[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let text = core::str::from_utf8(&s[start..i]).ok()?;
    Some((text.parse().ok()?, i))
}

/// C's `%i` (base from the prefix) after optional whitespace.
fn scan_c_int(s: &[u8]) -> Option<(i64, usize)> {
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let neg = if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
        s[i - 1] == b'-'
    } else {
        false
    };
    let (base, mut j) = if s[i..].starts_with(b"0x") || s[i..].starts_with(b"0X") {
        (16, i + 2)
    } else if s.get(i) == Some(&b'0') {
        (8, i)
    } else {
        (10, i)
    };
    let d0 = j;
    let mut v: i64 = 0;
    while let Some(d) = s.get(j).and_then(|&c| char::from(c).to_digit(base)) {
        v = v
            .saturating_mul(i64::from(base))
            .saturating_add(i64::from(d));
        j += 1;
    }
    if j == d0 {
        return None;
    }
    Some((if neg { -v } else { v }, j))
}

/// A tiny `sscanf`: `%i` and `%s` conversions, whitespace directives and
/// literals. Returns the conversions made.
enum Conv {
    Int(i64),
    Str(Vec<u8>),
}

fn sscanf(s: &[u8], fmt: &[u8]) -> Vec<Conv> {
    let mut out = Vec::new();
    let (mut i, mut f) = (0, 0);
    while f < fmt.len() {
        let c = fmt[f];
        if c == b'%' {
            let conv = fmt.get(f + 1).copied();
            // (`%255s`: the width is 255, more than any line here)
            let (conv, skip) = if conv == Some(b'2') {
                (Some(b's'), 5)
            } else {
                (conv, 2)
            };
            f += skip;
            match conv {
                Some(b'i') => match scan_c_int(&s[i..]) {
                    Some((v, n)) => {
                        out.push(Conv::Int(v));
                        i += n;
                    }
                    None => return out,
                },
                Some(b's') => {
                    while i < s.len() && s[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    let st = i;
                    while i < s.len() && !s[i].is_ascii_whitespace() && s[i] != 0 && i - st < 255 {
                        i += 1;
                    }
                    if i == st {
                        return out;
                    }
                    out.push(Conv::Str(s[st..i].to_vec()));
                }
                _ => return out,
            }
        } else if c.is_ascii_whitespace() {
            while i < s.len() && s[i].is_ascii_whitespace() {
                i += 1;
            }
            f += 1;
        } else {
            if s.get(i) != Some(&c) {
                return out;
            }
            i += 1;
            f += 1;
        }
    }
    out
}

/// `%g` of a C `float`.
fn fmt_g(v: f32) -> Vec<u8> {
    crate::fontmap::percent_g(f64::from(v))
}

struct T1<'a, 'j> {
    job: &'j mut T1Job<'a>,
    data: &'a [u8],
    pos: usize,
    eof: bool,
    pfa: bool,
    block_length: i64,
    dr: u16,
    er: u16,
    in_eexec: u8,
    cs: bool,
    scan: bool,
    synthetic: bool,
    eexec_encrypt: bool,
    len_iv: usize,
    last_hexbyte: i32,
    line: Vec<u8>,
    cslen: usize,
    cs_start: usize,
    fb: Vec<u8>,
    save_offset: usize,
    length1: usize,
    length2: usize,
    fontname_offset: usize,
    standard_encoding: bool,
    cs_tab: Vec<Cs>,
    cs_size: usize,
    cs_notdef: Option<usize>,
    cs_dict_start: Vec<u8>,
    cs_dict_end: Vec<u8>,
    cs_size_pos: usize,
    cs_token_pair: Option<(&'static [u8], &'static [u8])>,
    subr_tab: Vec<Cs>,
    subr_size: usize,
    subr_max: isize,
    subr_array_start: Vec<u8>,
    subr_array_end: Vec<u8>,
    subr_size_pos: usize,
    stack: Vec<i32>,
    warnings: Vec<Vec<u8>>,
    tag: Option<[u8; 6]>,
}

const CS_TOKEN_PAIRS: [(&[u8], &[u8]); 4] = [
    (b" RD", b"NP"),
    (b" -|", b"|"),
    (b" RD", b"noaccess put"),
    (b" -|", b"noaccess put"),
];

/// `str_suffix`: `buf` ends with `s` (a final newline aside).
fn suffix(buf: &[u8], s: &[u8]) -> bool {
    let b = buf.strip_suffix(b"\n").unwrap_or(buf);
    b.ends_with(s)
}

/// The C string in `buf` (up to a null).
fn c_str(buf: &[u8]) -> &[u8] {
    buf.iter().position(|&c| c == 0).map_or(buf, |n| &buf[..n])
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `eol`: a line ends with a newline (if longer than one byte).
fn eol(line: &mut Vec<u8>) {
    if line.len() > 1 && line.last() != Some(&10) {
        line.push(10);
    }
}

impl T1<'_, '_> {
    fn getchar(&mut self) -> i32 {
        if let Some(&c) = self.data.get(self.pos) {
            self.pos += 1;
            i32::from(c)
        } else {
            self.eof = true;
            -1
        }
    }

    fn check_pfa(&mut self) {
        self.pfa = self.data.first() != Some(&128);
    }

    fn getbyte(&mut self) -> Result<i32, Fail> {
        let mut c = self.getchar();
        if self.pfa {
            return Ok(c);
        }
        if self.block_length == 0 {
            if c != 128 {
                return fail(String::from("invalid marker"));
            }
            c = self.getchar();
            if c == 3 {
                while !self.eof {
                    self.getchar();
                }
                return Ok(-1);
            }
            let mut l = i64::from(self.getchar() & 0xff);
            l |= i64::from(self.getchar() & 0xff) << 8;
            l |= i64::from(self.getchar() & 0xff) << 16;
            l |= i64::from(self.getchar() & 0xff) << 24;
            self.block_length = l;
            c = self.getchar();
        }
        self.block_length -= 1;
        Ok(c)
    }

    fn edecrypt(&mut self, cipher: i32) -> Result<u8, Fail> {
        let mut cipher = cipher;
        if self.pfa {
            while cipher == 10 || cipher == 13 {
                cipher = self.getbyte()?;
            }
            let hi = hexval(cipher);
            let lo = hexval(self.getbyte()?);
            cipher = (hi << 4) + lo;
            self.last_hexbyte = cipher;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "C's (byte)"
        )]
        let cb = cipher as u8;
        let plain = cb ^ (self.dr >> 8) as u8;
        self.dr = (u16::from(cb).wrapping_add(self.dr))
            .wrapping_mul(T1_C1)
            .wrapping_add(T1_C2);
        Ok(plain)
    }

    fn eencrypt(&mut self, plain: u8) -> u8 {
        let cipher = plain ^ (self.er >> 8) as u8;
        self.er = (u16::from(cipher).wrapping_add(self.er))
            .wrapping_mul(T1_C1)
            .wrapping_add(T1_C2);
        cipher
    }

    fn scan_num(&self, from: usize) -> Result<(f32, usize), Fail> {
        let mut p = from;
        if self.line.get(p) == Some(&b' ') {
            p += 1;
        }
        let Some((v, _)) = scan_float(&self.line[p.min(self.line.len())..]) else {
            let mut l = self.line.clone();
            if l.last() == Some(&10) {
                l.pop();
            }
            return fail(format!(
                "a number expected: `{}'",
                String::from_utf8_lossy(&l)
            ));
        };
        while p < self.line.len()
            && (self.line[p].is_ascii_digit()
                || matches!(self.line[p], b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            p += 1;
        }
        #[expect(clippy::cast_possible_truncation, reason = "C's float")]
        Ok((v as f32, p))
    }

    /// `append_char_to_buf`.
    fn append(&mut self, c: i32) -> i32 {
        let mut c = c;
        if c == 9 {
            c = 32;
        }
        if c == 13 || c == -1 {
            c = 10;
        }
        if c != 32 || self.line.last().is_some_and(|&p| p != 32) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a byte"
            )]
            self.line.push(c as u8);
        }
        c
    }

    fn getline(&mut self) -> Result<(), Fail> {
        const EEXEC: &[u8] = b"currentfile eexec";
        loop {
            if self.eof {
                return fail(String::from("unexpected end of file"));
            }
            self.line.clear();
            self.cslen = 0;
            let mut eexec_scan: isize = 0;
            let mut c = self.getbyte()?;
            if c == -1 {
                return Ok(());
            }
            while !self.eof {
                if self.in_eexec == 1 {
                    c = i32::from(self.edecrypt(c)?);
                }
                c = self.append(c);
                if self.in_eexec == 0 && (0..17).contains(&eexec_scan) {
                    let k = usize::try_from(eexec_scan).unwrap_or(0);
                    if self.line.get(k) == Some(&EEXEC[k]) {
                        eexec_scan += 1;
                    } else {
                        eexec_scan = -1;
                    }
                }
                if c == 10 || (self.pfa && eexec_scan == 17 && c == 32) {
                    break;
                }
                if self.cs
                    && self.cslen == 0
                    && self.line.len() > 4
                    && (suffix(&self.line, b" RD ") || suffix(&self.line, b" -| "))
                {
                    let mut p = self.line.len() - 5;
                    while self.line[p] != b' ' {
                        p -= 1;
                    }
                    let (l, _) = self.scan_num(p + 1)?;
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "C's (int)"
                    )]
                    let l = l as i32 as usize;
                    self.cslen = l;
                    self.cs_start = self.line.len();
                    for _ in 0..l {
                        let b = self.getbyte()?;
                        let d = self.edecrypt(b)?;
                        self.line.push(d);
                    }
                }
                c = self.getbyte()?;
            }
            // `append_eol`
            if self.line.len() > 1 && self.line.last() != Some(&10) {
                self.line.push(10);
            }
            let n = self.line.len();
            if n > 2 && self.line[n - 2] == 32 {
                self.line[n - 2] = 10;
                self.line.pop();
            }
            if self.line.len() < 2 {
                continue;
            }
            if eexec_scan == 17 {
                self.in_eexec = 1;
            }
            return Ok(());
        }
    }

    fn putline(&mut self) {
        if self.line.len() <= 1 {
            return;
        }
        if self.eexec_encrypt {
            for i in 0..self.line.len() {
                let c = self.eencrypt(self.line[i]);
                self.fb.push(c);
            }
        } else {
            self.fb.extend_from_slice(&self.line);
        }
    }

    fn puts(&mut self, s: &[u8]) {
        self.line = s.to_vec();
        self.putline();
    }

    fn prefix(&self, s: &[u8]) -> bool {
        self.line.starts_with(s)
    }

    fn charstrings(&self) -> bool {
        find(c_str(&self.line), b"/CharStrings").is_some()
    }

    fn subrs(&self) -> bool {
        self.prefix(b"/Subrs")
    }

    fn end_eexec(&self) -> bool {
        suffix(&self.line, b"mark currentfile closefile")
    }

    fn line_text(&self) -> String {
        let mut l = c_str(&self.line).to_vec();
        if l.last() == Some(&10) {
            l.pop();
        }
        String::from_utf8_lossy(&l).into_owned()
    }

    fn check_block_len(&mut self, decrypt: bool) -> Result<(), Fail> {
        if self.block_length == 0 {
            return Ok(());
        }
        let mut c = self.getbyte()?;
        if decrypt {
            c = i32::from(self.edecrypt(c)?);
        }
        let l = self.block_length;
        if !(l == 0 && (c == 10 || c == 13)) {
            return fail(format!("{} bytes more than expected", l + 1));
        }
        Ok(())
    }

    fn start_eexec(&mut self) -> Result<(), Fail> {
        self.length1 = self.fb.len() - self.save_offset;
        self.save_offset = self.fb.len();
        if !self.pfa {
            self.check_block_len(false)?;
        }
        self.line.clear();
        for _ in 0..4 {
            let b = self.getbyte()?;
            self.edecrypt(b)?;
            self.line.push(0);
        }
        self.eexec_encrypt = true;
        self.putline();
        Ok(())
    }

    fn stop_eexec(&mut self) -> Result<(), Fail> {
        self.length2 = self.fb.len() - self.save_offset;
        self.save_offset = self.fb.len();
        self.eexec_encrypt = false;
        if self.pfa {
            let b = self.getbyte()?;
            let c = self.edecrypt(b)?;
            if !(c == 10 || c == 13) {
                if self.last_hexbyte == 0 {
                    self.puts(b"00");
                } else {
                    return fail(String::from("unexpected data after eexec"));
                }
            }
        } else {
            self.check_block_len(true)?;
        }
        self.cs = false;
        self.in_eexec = 2;
        Ok(())
    }

    fn modify_fm(&mut self) -> Result<(), Fail> {
        let open = self.line.iter().position(|&c| c == b'[' || c == b'{');
        let Some(open) = open else {
            return fail(format!(
                "FontMatrix: an array expected: `{}'",
                self.line_text()
            ));
        };
        let c = self.line[open];
        let mut out = self.line[..=open].to_vec();
        let mut p = open + 1;
        let mut a = [0f32; 6];
        for x in &mut a {
            let (v, q) = self.scan_num(p)?;
            *x = v;
            p = q;
        }
        #[expect(clippy::cast_possible_truncation, reason = "C's float arithmetic")]
        if self.job.slant != 0 {
            let s = (f64::from(self.job.slant) * 1e-3) as f32;
            a[0] += a[1] * s;
            a[2] += a[3] * s;
            a[4] += a[5] * s;
        }
        #[expect(clippy::cast_possible_truncation, reason = "C's float arithmetic")]
        if self.job.extend != 0 {
            let e = (f64::from(self.job.extend) * 1e-3) as f32;
            a[0] *= e;
            a[2] *= e;
            a[4] *= e;
        }
        for x in a {
            out.extend_from_slice(&fmt_g(x));
            out.push(b' ');
        }
        let close = if c == b'[' { b']' } else { b'}' };
        while p < self.line.len() && self.line[p] != close && self.line[p] != 0 {
            p += 1;
        }
        if p >= self.line.len() || self.line[p] == 0 {
            return fail(format!(
                "FontMatrix: cannot find the corresponding character to '{}': `{}'",
                char::from(c),
                self.line_text()
            ));
        }
        out.extend_from_slice(&self.line[p..]);
        self.line = out;
        eol(&mut self.line);
        Ok(())
    }

    fn modify_italic(&mut self) -> Result<(), Fail> {
        if self.job.slant == 0 {
            return Ok(());
        }
        let sp = self.line.iter().position(|&c| c == b' ').unwrap_or(0);
        let mut out = self.line[..=sp].to_vec();
        let (a, r) = self.scan_num(sp + 1)?;
        #[expect(clippy::cast_possible_truncation, reason = "C's float arithmetic")]
        let a = (f64::from(a)
            - atan(f64::from(self.job.slant) * 1e-3) * (180.0 / core::f64::consts::PI))
            as f32;
        out.extend_from_slice(&fmt_g(a));
        out.extend_from_slice(&self.line[r..]);
        self.line = out;
        eol(&mut self.line);
        #[expect(clippy::cast_possible_truncation, reason = "C's round")]
        let v = round_f(f64::from(a)) as i32;
        self.job.font_dim[ITALIC_ANGLE] = (v, true);
        Ok(())
    }

    fn scan_keys(&mut self) -> Result<(), Fail> {
        if self.job.extend != 0 || self.job.slant != 0 {
            if self.prefix(b"/FontMatrix") {
                return self.modify_fm();
            }
            if self.prefix(b"/ItalicAngle") {
                return self.modify_italic();
            }
        }
        if self.prefix(b"/FontType") {
            let (i, _) = self.scan_num(9)?;
            #[expect(clippy::cast_possible_truncation, reason = "C's (int)")]
            let i = i as i32;
            if i != 1 {
                return fail(format!("Type{i} fonts unsupported by pdfTeX"));
            }
            return Ok(());
        }
        let Some(k) = FONT_KEYS
            .iter()
            .position(|(_, t1)| !t1.is_empty() && self.line[1..].starts_with(t1.as_bytes()))
        else {
            return Ok(());
        };
        let mut p = FONT_KEYS[k].1.len() + 1;
        if self.line.get(p) == Some(&b' ') {
            p += 1;
        }
        if k == FONTNAME {
            if self.line.get(p) != Some(&b'/') {
                return fail(format!("a name expected: `{}'", self.line_text()));
            }
            p += 1;
            let r = p;
            let mut q = p;
            while q < self.line.len() && self.line[q] != b' ' && self.line[q] != 10 {
                q += 1;
            }
            let mut name = self.line[r..q].to_vec();
            if self.job.slant != 0 {
                name.extend_from_slice(format!("-Slant_{}", self.job.slant).as_bytes());
            }
            if self.job.extend != 0 {
                name.extend_from_slice(format!("-Extend_{}", self.job.extend).as_bytes());
            }
            self.job.fontname.clone_from(&name);
            if self.job.subsetted {
                self.fontname_offset = self.fb.len() + r;
                let rest = self.line[q..].to_vec();
                self.line.truncate(r);
                self.line.extend_from_slice(b"ABCDEF+");
                self.line.extend_from_slice(&name);
                self.line.extend_from_slice(c_str(&rest));
                eol(&mut self.line);
            }
            return Ok(());
        }
        if (k == STEMV || k == FONTBBOX1) && matches!(self.line.get(p), Some(&(b'[' | b'{'))) {
            p += 1;
        }
        if k == FONTBBOX1 {
            for i in 0..4 {
                let (v, r) = self.scan_num(p)?;
                #[expect(clippy::cast_possible_truncation, reason = "C's (int)")]
                let v = v as i32;
                self.job.font_dim[k + i] = (v, true);
                p = r;
            }
            return Ok(());
        }
        let (v, _) = self.scan_num(p)?;
        #[expect(clippy::cast_possible_truncation, reason = "C's (int)")]
        let v = v as i32;
        self.job.font_dim[k] = (v, true);
        Ok(())
    }

    fn scan_param(&mut self) -> Result<(), Fail> {
        if !self.scan || self.line.first() != Some(&b'/') {
            return Ok(());
        }
        if self.prefix(b"/lenIV") {
            let (v, _) = self.scan_num(6)?;
            if v < 0.0 {
                return fail(String::from("negative value of lenIV is not supported"));
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C's short"
            )]
            let v = v as i32 as usize;
            self.len_iv = v;
            return Ok(());
        }
        self.scan_keys()
    }

    /// `t1_builtin_enc`.
    fn builtin_enc(&mut self) -> Result<Vec<Vec<u8>>, Fail> {
        let mut names = vec![NOTDEF.to_vec(); 256];
        if suffix(&self.line, b"def") {
            let convs = sscanf(&self.line[9..], b"%255s");
            if let Some(Conv::Str(s)) = convs.first()
                && s == b"StandardEncoding"
            {
                self.standard_encoding = true;
                for (i, n) in STANDARD_GLYPH_NAMES.iter().enumerate() {
                    names[i] = n.as_bytes().to_vec();
                }
                return Ok(names);
            }
            let s = match convs.first() {
                Some(Conv::Str(s)) => String::from_utf8_lossy(s).into_owned(),
                _ => String::new(),
            };
            return fail(format!(
                "cannot subset font (unknown predefined encoding `{s}')"
            ));
        }
        self.standard_encoding = false;
        if self.prefix(b"/Encoding [") || self.prefix(b"/Encoding[") {
            let mut r = self.line.iter().position(|&c| c == b'[').unwrap_or(0) + 1;
            if self.line.get(r) == Some(&b' ') {
                r += 1;
            }
            let mut counter = 0;
            loop {
                while self.line.get(r) == Some(&b'/') {
                    r += 1;
                    let st = r;
                    while r < self.line.len() && !matches!(self.line[r], 32 | 10 | b']' | b'/') {
                        r += 1;
                    }
                    let name = self.line[st..r].to_vec();
                    if self.line.get(r) == Some(&b' ') {
                        r += 1;
                    }
                    if counter > 255 {
                        return fail(String::from("encoding vector contains more than 256 names"));
                    }
                    if name != NOTDEF {
                        names[counter] = name;
                    }
                    counter += 1;
                }
                let c = self.line.get(r).copied().unwrap_or(10);
                if c != 10 && c != b'%' {
                    if self.line[r..].starts_with(b"] def")
                        || self.line[r..].starts_with(b"] readonly def")
                    {
                        break;
                    }
                    return fail(format!(
                        "a name or `] def' or `] readonly def' expected: `{}'",
                        self.line_text()
                    ));
                }
                self.getline()?;
                r = 0;
            }
            return Ok(names);
        }
        let mut p = self
            .line
            .iter()
            .position(|&c| c == 10)
            .unwrap_or(self.line.len());
        let valid = |v: i64| (0..256).contains(&v);
        let ix = |v: i64| usize::try_from(v).unwrap_or(0);
        loop {
            if self.line.get(p).is_none_or(|&c| c == 10) {
                self.getline()?;
                p = 0;
            }
            let rest = self.line[p..].to_vec();
            let a = sscanf(&rest, b"dup %i%255s put");
            if let [Conv::Int(i), Conv::Str(g), ..] = a.as_slice()
                && g.first() == Some(&b'/')
                && valid(*i)
            {
                if &g[1..] != NOTDEF {
                    names[ix(*i)] = g[1..].to_vec();
                }
                let Some(at) = find(&rest, b" put") else {
                    return fail(String::from("invalid pfb, no put found in dup"));
                };
                p += at + 4;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
                continue;
            }
            let b2 = sscanf(&rest, b"dup dup %i exch %i get put");
            if let [Conv::Int(b), Conv::Int(a), ..] = b2.as_slice()
                && valid(*a)
                && valid(*b)
            {
                let g = names[ix(*a)].clone();
                names[ix(*b)] = g;
                let Some(at) = find(&rest, b" get put") else {
                    return fail(String::from("invalid pfb, no get put found in dup dup"));
                };
                p += at + 8;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
                continue;
            }
            let c3 = sscanf(&rest, b"dup dup %i %i getinterval %i exch putinterval");
            if let [Conv::Int(a), Conv::Int(c), Conv::Int(b), ..] = c3.as_slice()
                && valid(*a)
                && valid(*b)
                && valid(*c)
            {
                for i in 0..*c {
                    if valid(*a + i) && valid(*b + i) {
                        let g = names[ix(*a + i)].clone();
                        names[ix(*b + i)] = g;
                    }
                }
                let Some(at) = find(&rest, b" putinterval") else {
                    return fail(String::from("invalid pfb, no putinterval found in dup dup"));
                };
                p += at + 12;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
                continue;
            }
            if (p == 0 || self.line.get(p - 1) == Some(&b' ')) && c_str(&rest) == b"def\n" {
                return Ok(names);
            }
            while p < self.line.len() && self.line[p] != b' ' && self.line[p] != 10 {
                p += 1;
            }
            if self.line.get(p) == Some(&b' ') {
                p += 1;
            }
        }
    }

    fn open_prefix(&mut self) {
        self.len_iv = 4;
        self.dr = 55665;
        self.er = 55665;
        self.in_eexec = 0;
        self.cs = false;
        self.scan = true;
        self.synthetic = false;
        self.eexec_encrypt = false;
        self.block_length = 0;
        self.check_pfa();
    }

    fn include(&mut self) -> Result<(), Fail> {
        loop {
            self.getline()?;
            self.scan_param()?;
            self.putline();
            if self.in_eexec != 0 {
                break;
            }
        }
        self.start_eexec()?;
        loop {
            self.getline()?;
            self.scan_param()?;
            self.putline();
            if self.charstrings() || self.subrs() {
                break;
            }
        }
        self.cs = true;
        loop {
            self.getline()?;
            self.putline();
            if self.end_eexec() {
                break;
            }
        }
        self.stop_eexec()
    }

    /// `cs_store`.
    fn cs_store(&mut self, is_subr: bool) -> Result<(), Fail> {
        let sp = self.line.iter().position(|&c| c == b' ').unwrap_or(0);
        let first = self.line[..sp].to_vec();
        let mut data = self.line[self.cs_start - 4..self.cs_start + self.cslen].to_vec();
        let mut p = self.cs_start + self.cslen;
        while p < self.line.len() && self.line[p] != 10 {
            data.push(self.line[p]);
            p += 1;
        }
        data.push(10);
        let entry = Cs {
            name: Vec::new(),
            data,
            cslen: self.cslen,
            used: false,
            valid: true,
        };
        if is_subr {
            let (v, _) = self.scan_num(sp + 1)?;
            #[expect(clippy::cast_possible_truncation, reason = "C's (int)")]
            let subr = v as i32;
            if subr < 0 || usize::try_from(subr).unwrap_or(usize::MAX) >= self.subr_size {
                return fail(format!("Subrs array: entry index out of range ({subr})"));
            }
            let s = usize::try_from(subr).unwrap_or(0);
            if self.cs_token_pair.is_none() {
                let buf = &entry.data;
                self.cs_token_pair = CS_TOKEN_PAIRS
                    .iter()
                    .find(|(a, b)| buf.starts_with(a) && suffix(buf, b))
                    .copied();
            }
            self.subr_tab[s] = entry;
        } else {
            if self.cs_tab.len() + 1 > self.cs_size {
                return fail(format!(
                    "CharStrings dict: more entries than dict size ({})",
                    self.cs_size
                ));
            }
            let mut e = entry;
            e.name = first.get(1..).unwrap_or_default().to_vec();
            self.cs_tab.push(e);
        }
        Ok(())
    }

    /// `cs_mark` of a charstring (`name`) or subroutine.
    fn cs_mark(&mut self, name: Option<&[u8]>, subr: i32) -> Result<(), Fail> {
        let what = |name: Option<&[u8]>| match name {
            None => format!("Subr ({subr})"),
            Some(n) => format!("CharString (/{})", String::from_utf8_lossy(n)),
        };
        let (is_subr, idx) = match name {
            None => {
                if subr < 0 || usize::try_from(subr).unwrap_or(usize::MAX) >= self.subr_size {
                    return fail(format!("Subrs array: entry index out of range ({subr})"));
                }
                let i = usize::try_from(subr).unwrap_or(0);
                if !self.subr_tab[i].valid {
                    return Ok(());
                }
                (true, i)
            }
            Some(n) => {
                if let (Some(i), true) = (self.cs_notdef, n == NOTDEF) {
                    (false, i)
                } else {
                    let Some(i) = self.cs_tab.iter().position(|c| c.name == n) else {
                        self.warnings.push(
                            format!("glyph `{}' undefined", String::from_utf8_lossy(n))
                                .into_bytes(),
                        );
                        return Ok(());
                    };
                    if self.cs_tab[i].name == NOTDEF {
                        self.cs_notdef = Some(i);
                    }
                    (false, i)
                }
            }
        };
        let e = if is_subr {
            &self.subr_tab[idx]
        } else {
            &self.cs_tab[idx]
        };
        if !e.valid || (e.used && !is_subr) {
            return Ok(());
        }
        let data = e.data.clone();
        let cslen = e.cslen;
        if is_subr {
            self.subr_tab[idx].used = true;
        } else {
            self.cs_tab[idx].used = true;
        }
        let mut cr: u16 = 4330;
        let mut k = 4;
        let mut next = |k: &mut usize| {
            let c = data[*k];
            *k += 1;
            let plain = c ^ (cr >> 8) as u8;
            cr = (u16::from(c).wrapping_add(cr))
                .wrapping_mul(T1_C1)
                .wrapping_add(T1_C2);
            plain
        };
        let mut cs_len = cslen.cast_signed();
        for _ in 0..self.len_iv {
            next(&mut k);
            cs_len -= 1;
        }
        let mut last_cmd = 0;
        while cs_len > 0 {
            cs_len -= 1;
            let b = usize::from(next(&mut k));
            if b >= 32 {
                let a: i32 = if b <= 246 {
                    i32::try_from(b).unwrap_or(0) - 139
                } else if b <= 250 {
                    cs_len -= 1;
                    ((i32::try_from(b).unwrap_or(0) - 247) << 8) + 108 + i32::from(next(&mut k))
                } else if b <= 254 {
                    cs_len -= 1;
                    -((i32::try_from(b).unwrap_or(0) - 251) << 8) - 108 - i32::from(next(&mut k))
                } else {
                    cs_len -= 4;
                    let mut a = u32::from(next(&mut k)) << 24;
                    a |= u32::from(next(&mut k)) << 16;
                    a |= u32::from(next(&mut k)) << 8;
                    a |= u32::from(next(&mut k));
                    a.cast_signed()
                };
                self.stack.push(a);
                continue;
            }
            let mut b = b;
            if b == CS_ESCAPE {
                b = usize::from(next(&mut k)) + CS_1BYTE_MAX;
                cs_len -= 1;
            }
            if b >= CS_MAX {
                return fail(format!("{}: command value out of range: {b}", what(name)));
            }
            let Some((nargs, bottom, clear)) = cc(b) else {
                return fail(format!("{}: command not valid: {b}", what(name)));
            };
            if bottom {
                let n = self.stack.len();
                if n < nargs {
                    return fail(format!(
                        "{}: less arguments on stack ({n}) than required ({nargs})",
                        what(name)
                    ));
                } else if n > nargs {
                    return fail(format!(
                        "{}: more arguments on stack ({n}) than required ({nargs})",
                        what(name)
                    ));
                }
            }
            last_cmd = b;
            let pop = |s: &mut Vec<i32>, n: usize| -> Result<(), Fail> {
                if s.len() < n {
                    return fail(format!(
                        "CharString: invalid access ({n}) to stack ({} entries)",
                        s.len()
                    ));
                }
                s.truncate(s.len() - n);
                Ok(())
            };
            let top = |s: &Vec<i32>, n: usize| {
                s.len()
                    .checked_sub(n)
                    .and_then(|i| s.get(i))
                    .copied()
                    .unwrap_or(0)
            };
            match b {
                CS_CALLSUBR => {
                    let a1 = top(&self.stack, 1);
                    pop(&mut self.stack, 1)?;
                    self.cs_mark(None, a1)?;
                    let ok = usize::try_from(a1)
                        .ok()
                        .and_then(|i| self.subr_tab.get(i))
                        .is_some_and(|s| s.valid);
                    if !ok {
                        return fail(format!("{}: cannot call subr ({a1})", what(name)));
                    }
                }
                CS_DIV => {
                    pop(&mut self.stack, 2)?;
                    self.stack.push(0);
                }
                CS_CALLOTHERSUBR => {
                    if top(&self.stack, 1) == 3 {
                        *self.job.last_arg_other_subr3 = top(&self.stack, 3);
                    }
                    let a1 = top(&self.stack, 2) + 2;
                    pop(&mut self.stack, usize::try_from(a1).unwrap_or(0))?;
                }
                CS_POP => self.stack.push(*self.job.last_arg_other_subr3),
                CS_SEAC => {
                    let a1 = self.stack.get(3).copied().unwrap_or(0);
                    let a2 = self.stack.get(4).copied().unwrap_or(0);
                    self.stack.clear();
                    let g1 =
                        STANDARD_GLYPH_NAMES[usize::try_from(a1).unwrap_or(0) & 255].as_bytes();
                    let g2 =
                        STANDARD_GLYPH_NAMES[usize::try_from(a2).unwrap_or(0) & 255].as_bytes();
                    self.cs_mark(Some(g1), 0)?;
                    self.cs_mark(Some(g2), 0)?;
                    self.job.glyphs.insert(g1.to_vec());
                    self.job.glyphs.insert(g2.to_vec());
                }
                _ => {
                    if clear {
                        self.stack.clear();
                    }
                }
            }
        }
        if is_subr && last_cmd != CS_RETURN {
            self.warnings.push(
                format!(
                    "last command in subr `{subr}' is not a RETURN; I will add it now but please consider fixing the font"
                )
                .into_bytes(),
            );
            self.append_cs_return(idx);
        }
        Ok(())
    }

    /// `append_cs_return`.
    fn append_cs_return(&mut self, idx: usize) {
        let e = &self.subr_tab[idx];
        let mut plain = Vec::with_capacity(e.cslen + 1);
        let mut cr: u16 = 4330;
        for &c in &e.data[4..4 + e.cslen] {
            plain.push(c ^ (cr >> 8) as u8);
            cr = (u16::from(c).wrapping_add(cr))
                .wrapping_mul(T1_C1)
                .wrapping_add(T1_C2);
        }
        #[expect(clippy::cast_possible_truncation, reason = "a command byte")]
        plain.push(CS_RETURN as u8);
        let mut data = e.data[..4].to_vec();
        let mut cr: u16 = 4330;
        for p in plain {
            let c = p ^ (cr >> 8) as u8;
            cr = (u16::from(c).wrapping_add(cr))
                .wrapping_mul(T1_C1)
                .wrapping_add(T1_C2);
            data.push(c);
        }
        data.extend_from_slice(&e.data[4 + e.cslen..]);
        let e = &mut self.subr_tab[idx];
        e.data = data;
        e.cslen += 1;
    }

    fn subset_ascii_part(&mut self) -> Result<Vec<Vec<u8>>, Fail> {
        self.getline()?;
        while !self.prefix(b"/Encoding") {
            self.scan_param()?;
            let s = c_str(&self.line);
            let unique_def =
                self.prefix(b"/UniqueID") && s.len() >= 4 && s[s.len() - 4..].starts_with(b"def");
            if !unique_def {
                self.putline();
            }
            self.getline()?;
        }
        let names = self.builtin_enc()?;
        if self.job.subsetted {
            for &c in &self.job.codes.clone() {
                self.job.glyphs.insert(names[usize::from(c)].clone());
            }
            let tag = make_subset_tag(&self.job.glyphs, &self.job.fontname, self.job.tags);
            let at = self.fontname_offset;
            self.fb[at..at + 6].copy_from_slice(&tag);
            self.tag = Some(tag);
        }
        if self.standard_encoding {
            self.puts(b"/Encoding StandardEncoding def\n");
        } else {
            self.puts(b"/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n");
            // the glyph tree: each name at its first code
            let mut first: alloc::collections::BTreeMap<&[u8], usize> =
                alloc::collections::BTreeMap::new();
            for (i, n) in names.iter().enumerate() {
                if n != NOTDEF {
                    first.entry(n.as_slice()).or_insert(i);
                }
            }
            let mut j = 0;
            let glyphs = self.job.glyphs.clone();
            for g in &glyphs {
                if let Some(&i) = first.get(g.as_slice()) {
                    let l = [
                        b"dup ".as_slice(),
                        format!("{i}").as_bytes(),
                        b" /",
                        g,
                        b" put\n",
                    ]
                    .concat();
                    self.puts(&l);
                    j += 1;
                }
            }
            if j == 0 {
                self.puts(b"dup 0 /.notdef put\n");
            }
            self.puts(b"readonly def\n");
        }
        loop {
            self.getline()?;
            self.scan_param()?;
            if !self.prefix(b"/UniqueID") {
                self.putline();
            }
            if self.in_eexec != 0 {
                break;
            }
        }
        Ok(names)
    }

    fn read_subrs(&mut self) -> Result<(), Fail> {
        self.getline()?;
        while !(self.charstrings() || self.subrs()) {
            self.scan_param()?;
            if !self.prefix(b"/UniqueID") {
                self.putline();
            }
            self.getline()?;
        }
        loop {
            self.cs = true;
            self.scan = false;
            if !self.subrs() {
                return Ok(());
            }
            self.subr_size_pos = 7;
            let (v, _) = self.scan_num(self.subr_size_pos)?;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "C's (int)"
            )]
            let n = v as i32 as usize;
            self.subr_size = n;
            if n == 0 {
                while !self.charstrings() {
                    self.getline()?;
                }
                return Ok(());
            }
            self.subr_tab = vec![Cs::default(); n];
            self.subr_array_start = self.line.clone();
            self.getline()?;
            while self.cslen > 0 {
                self.cs_store(true)?;
                self.getline()?;
            }
            for s in self.subr_tab.iter_mut().take(4) {
                s.used = true;
            }
            let mut end = Vec::new();
            let mut i = 0;
            while i < 5 {
                if self.charstrings() {
                    break;
                }
                end.extend_from_slice(c_str(&self.line));
                self.getline()?;
                i += 1;
            }
            self.subr_array_end = end;
            if i < 5 {
                return Ok(());
            }
            // CharStrings not found: a synthetic font
            self.subr_tab.clear();
            self.subr_size = 0;
            self.subr_max = 0;
            self.subr_array_start.clear();
            self.subr_array_end.clear();
            self.cs_token_pair = None;
            self.cs = false;
            self.synthetic = true;
            while !(self.charstrings() || self.subrs()) {
                self.getline()?;
            }
        }
    }

    fn flush_cs(&mut self, is_subr: bool) {
        let (start_line, line_end, size_pos, count) = if is_subr {
            (
                self.subr_array_start.clone(),
                self.subr_array_end.clone(),
                self.subr_size_pos,
                usize::try_from(self.subr_max + 1).unwrap_or(0),
            )
        } else {
            let n = self.cs_tab.iter().filter(|c| c.used).count();
            (
                self.cs_dict_start.clone(),
                self.cs_dict_end.clone(),
                self.cs_size_pos,
                n,
            )
        };
        let mut l = start_line[..size_pos.min(start_line.len())].to_vec();
        let mut p = size_pos;
        while p < start_line.len() && start_line[p].is_ascii_digit() {
            p += 1;
        }
        l.extend_from_slice(format!("{count}").as_bytes());
        l.extend_from_slice(c_str(&start_line[p.min(start_line.len())..]));
        self.line = l;
        eol(&mut self.line);
        self.putline();
        let mut return_cs = Vec::new();
        if is_subr {
            let mut cr: u16 = 4330;
            let mut enc = |p: u8| {
                let c = p ^ (cr >> 8) as u8;
                cr = (u16::from(c).wrapping_add(cr))
                    .wrapping_mul(T1_C1)
                    .wrapping_add(T1_C2);
                c
            };
            for _ in 0..self.len_iv {
                return_cs.push(enc(0));
            }
            #[expect(clippy::cast_possible_truncation, reason = "a command byte")]
            return_cs.push(enc(CS_RETURN as u8));
        }
        let entries = if is_subr {
            core::mem::take(&mut self.subr_tab)
        } else {
            core::mem::take(&mut self.cs_tab)
        };
        let n = if is_subr { count } else { entries.len() };
        for (i, e) in entries.iter().enumerate().take(n) {
            if e.used {
                let head = if is_subr {
                    format!("dup {i} {}", e.cslen).into_bytes()
                } else {
                    [b"/".as_slice(), &e.name, format!(" {}", e.cslen).as_bytes()].concat()
                };
                self.line = [head.as_slice(), &e.data].concat();
                self.putline();
            } else if is_subr {
                let (open, close) = self.cs_token_pair.unwrap_or((b" RD", b"NP"));
                let head = format!("dup {i} {}", return_cs.len()).into_bytes();
                self.line = [head.as_slice(), open, b" ", &return_cs].concat();
                self.putline();
                self.line = [b" ".as_slice(), close].concat();
                eol(&mut self.line);
                self.putline();
            }
        }
        self.line = c_str(&line_end).to_vec();
        eol(&mut self.line);
        self.putline();
    }

    fn mark_glyphs(&mut self) -> Result<(), Fail> {
        if self.synthetic || self.job.all_glyphs {
            for c in &mut self.cs_tab {
                if c.valid {
                    c.used = true;
                }
            }
            for c in &mut self.subr_tab {
                if c.valid {
                    c.used = true;
                }
            }
            self.subr_max = isize::try_from(self.subr_size).unwrap_or(0) - 1;
            return Ok(());
        }
        self.cs_mark(Some(NOTDEF), 0)?;
        for g in self.job.glyphs.clone() {
            self.cs_mark(Some(&g), 0)?;
        }
        if !self.subr_tab.is_empty() {
            self.subr_max = -1;
            for (i, s) in self.subr_tab.iter().enumerate() {
                if s.used {
                    self.subr_max = isize::try_from(i).unwrap_or(0);
                }
            }
        }
        Ok(())
    }

    fn check_unusual_charstring(&mut self) -> Result<(), Fail> {
        let at = find(c_str(&self.line), b"/CharStrings").unwrap_or(0) + 12;
        if scan_c_int(&self.line[at..]).is_none() {
            let mut buf = c_str(&self.line).to_vec();
            if let Some(l) = buf.last_mut() {
                *l = b' ';
            }
            self.getline()?;
            buf.extend_from_slice(c_str(&self.line));
            self.line = buf;
            eol(&mut self.line);
        }
        Ok(())
    }

    fn subset_charstrings(&mut self) -> Result<(), Fail> {
        self.check_unusual_charstring()?;
        self.cs_size_pos = find(c_str(&self.line), b"/CharStrings").unwrap_or(0) + 12 + 1;
        let (v, _) = self.scan_num(self.cs_size_pos)?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "C's (int)"
        )]
        let n = v as i32 as usize;
        self.cs_size = n;
        self.cs_tab = Vec::with_capacity(n);
        self.cs_notdef = None;
        self.cs_dict_start = self.line.clone();
        self.getline()?;
        while self.cslen > 0 {
            self.cs_store(false)?;
            self.getline()?;
        }
        self.cs_dict_end = self.line.clone();
        self.mark_glyphs()?;
        if !self.subr_tab.is_empty() {
            if self.cs_token_pair.is_none() {
                return fail(String::from(
                    "This Type 1 font uses mismatched subroutine begin/end token pairs.",
                ));
            }
            self.flush_cs(true);
        }
        self.flush_cs(false);
        Ok(())
    }

    fn subset_end(&mut self) -> Result<(), Fail> {
        if self.synthetic {
            while find(c_str(&self.line), b"definefont").is_none() {
                self.getline()?;
                self.putline();
            }
            while !self.end_eexec() {
                self.getline()?;
            }
            self.putline();
        } else {
            while !self.end_eexec() {
                self.getline()?;
                self.putline();
            }
        }
        self.stop_eexec()
    }
}

fn hexval(c: i32) -> i32 {
    match u8::try_from(c) {
        Ok(c @ b'A'..=b'F') => i32::from(c - b'A' + 10),
        Ok(c @ b'a'..=b'f') => i32::from(c - b'a' + 10),
        Ok(c @ b'0'..=b'9') => i32::from(c - b'0'),
        _ => -1,
    }
}

/// C's `round`.
fn round_f(x: f64) -> f64 {
    let t = trunc(x);
    if (x - t).abs() >= 0.5 {
        t + x.signum()
    } else {
        t
    }
}

fn trunc(x: f64) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "in range"
    )]
    let r = (x as i64) as f64;
    r
}

/// `atan` (no `std`): argument reduction and a series.
pub(crate) fn atan(x: f64) -> f64 {
    if x < 0.0 {
        return -atan(-x);
    }
    if x > 1.0 {
        return core::f64::consts::FRAC_PI_2 - atan(1.0 / x);
    }
    // atan(x) = 2 atan(x / (1 + sqrt(1 + x^2)))
    let y = x / (1.0 + sqrt(1.0 + x * x));
    let y = y / (1.0 + sqrt(1.0 + y * y));
    // |y| < 0.2: the series converges fast
    let y2 = y * y;
    let mut term = y;
    let mut sum = 0.0;
    let mut k = 0;
    while k < 40 {
        sum += term / f64::from(2 * k + 1);
        term *= -y2;
        k += 1;
    }
    4.0 * sum
}

fn sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = if x > 1.0 { x } else { 1.0 };
    for _ in 0..60 {
        r = f64::midpoint(r, x / r);
    }
    r
}

/// utils.c's `make_subset_tag`.
pub(crate) fn make_subset_tag(
    glyphs: &BTreeSet<Vec<u8>>,
    fontname: &[u8],
    tags: &mut BTreeSet<[u8; 6]>,
) -> [u8; 6] {
    let mut j: i32 = 0;
    loop {
        let mut data = Vec::new();
        for g in glyphs {
            data.extend_from_slice(g);
            data.push(b' ');
        }
        data.extend_from_slice(fontname);
        data.extend_from_slice(&j.to_le_bytes());
        let d = partex_engine::md5::md5(&data);
        let mut a = [0i32; 6];
        a[0] = d[..13].iter().map(|&b| i32::from(b)).sum();
        for i in 1..6 {
            a[i] = a[i - 1] - i32::from(d[i - 1]) + i32::from(d[(i + 12) % 16]);
        }
        let mut tag = [0u8; 6];
        for i in 0..6 {
            tag[i] = b'A' + u8::try_from(a[i] % 26).unwrap_or(0);
        }
        j += 1;
        if tags.insert(tag) {
            return tag;
        }
    }
}

/// writet1.c's `writet1`: the font file stream's contents, or the
/// message of `pdftex_fail`.
pub(crate) fn writet1(job: &mut T1Job<'_>) -> Result<T1Out, (Fail, Vec<Vec<u8>>)> {
    let data = job.data;
    let subsetted = job.subsetted;
    let mut t = T1 {
        job,
        data,
        pos: 0,
        eof: false,
        pfa: false,
        block_length: 0,
        dr: 55665,
        er: 55665,
        in_eexec: 0,
        cs: false,
        scan: true,
        synthetic: false,
        eexec_encrypt: false,
        len_iv: 4,
        last_hexbyte: 0,
        line: Vec::new(),
        cslen: 0,
        cs_start: 0,
        fb: Vec::new(),
        save_offset: 0,
        length1: 0,
        length2: 0,
        fontname_offset: 0,
        standard_encoding: false,
        cs_tab: Vec::new(),
        cs_size: 0,
        cs_notdef: None,
        cs_dict_start: Vec::new(),
        cs_dict_end: Vec::new(),
        cs_size_pos: 0,
        cs_token_pair: None,
        subr_tab: Vec::new(),
        subr_size: 0,
        subr_max: -1,
        subr_array_start: Vec::new(),
        subr_array_end: Vec::new(),
        subr_size_pos: 0,
        stack: Vec::new(),
        warnings: Vec::new(),
        tag: None,
    };
    t.open_prefix();
    let run = |t: &mut T1| -> Result<Option<Vec<Vec<u8>>>, Fail> {
        if !subsetted {
            t.include()?;
            return Ok(None);
        }
        let names = t.subset_ascii_part()?;
        t.start_eexec()?;
        t.read_subrs()?;
        t.subset_charstrings()?;
        t.subset_end()?;
        Ok(Some(names))
    };
    match run(&mut t) {
        Ok(builtin) => Ok(T1Out {
            length1: t.length1,
            length2: t.length2,
            subset_tag: t.tag,
            builtin,
            warnings: t.warnings,
            bytes: t.fb,
        }),
        Err(m) => Err((m, t.warnings)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_c() {
        assert_eq!(fmt_g(0.001), b"0.001");
        assert_eq!(fmt_g(-0.000_167), b"-0.000167");
        assert!((atan(1.0) - core::f64::consts::FRAC_PI_4).abs() < 1e-12);
        assert!((atan(0.167) - 0.165_472_984_197_484_55).abs() < 1e-15);
    }

    #[test]
    fn scans_like_c() {
        let c = sscanf(b"dup 32 /space put\n", b"dup %i%255s put");
        assert!(matches!(c.as_slice(), [Conv::Int(32), Conv::Str(s)] if s == b"/space"));
        let c = sscanf(
            b"dup dup 161 10 getinterval 0 exch putinterval",
            b"dup dup %i %i getinterval %i exch putinterval",
        );
        assert_eq!(c.len(), 3);
    }
}
