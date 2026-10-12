//! Type 1 fonts embedded, whole or subset (`writet1.c`): the font file is
//! read line by line as pdfTeX reads it, the eexec part decrypted and
//! encrypted again, and for a subset only the glyphs used (and the
//! subroutines they call) are kept.

use std::collections::{BTreeMap, BTreeSet};

use crate::PdfError;
use crate::enc::{GlyphNames, append_char, append_eol};
use crate::map::MapEntry;

/// The `font_dim` codes (`ptexmac.h`).
pub mod dim {
    /// `ASCENT_CODE`.
    pub const ASCENT: usize = 0;
    /// `CAPHEIGHT_CODE`.
    pub const CAPHEIGHT: usize = 1;
    /// `DESCENT_CODE`.
    pub const DESCENT: usize = 2;
    /// `ITALIC_ANGLE_CODE`.
    pub const ITALIC_ANGLE: usize = 3;
    /// `STEMV_CODE`.
    pub const STEMV: usize = 4;
    /// `XHEIGHT_CODE`.
    pub const XHEIGHT: usize = 5;
    /// `FONTBBOX1_CODE`.
    pub const FONTBBOX1: usize = 6;
    /// `FONTNAME_CODE`.
    pub const FONTNAME: usize = 10;
    /// `GEN_KEY_NUM`.
    pub const GEN_KEY_NUM: usize = 6;
    /// `INT_KEYS_NUM`.
    pub const INT_KEYS_NUM: usize = 10;
}

/// `font_key` (`ptexlib.h`): the PDF and the Type 1 names of the keys.
pub const FONT_KEY: [(&str, &str); 11] = [
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

const NOTDEF: &[u8] = b".notdef";

/// `standard_glyph_names` (`writet1.c`): Adobe's `StandardEncoding`.
#[rustfmt::skip]
const STANDARD: [&str; 256] = [
    "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
    "space", "exclam", "quotedbl", "numbersign", "dollar", "percent",
    "ampersand", "quoteright", "parenleft", "parenright", "asterisk",
    "plus", "comma", "hyphen", "period", "slash",
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight",
    "nine", "colon", "semicolon", "less", "equal", "greater", "question",
    "at", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N",
    "O",
    "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z", "bracketleft",
    "backslash", "bracketright", "asciicircum", "underscore",
    "quoteleft", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    "m", "n", "o",
    "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "braceleft", "bar",
    "braceright", "asciitilde", "",
    "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
    "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
    "", "exclamdown", "cent", "sterling", "fraction", "yen", "florin",
    "section", "currency", "quotesingle", "quotedblleft", "guillemotleft",
    "guilsinglleft", "guilsinglright", "fi", "fl",
    "", "endash", "dagger", "daggerdbl", "periodcentered", "",
    "paragraph", "bullet", "quotesinglbase", "quotedblbase",
    "quotedblright", "guillemotright", "ellipsis", "perthousand", "",
    "questiondown",
    "", "grave", "acute", "circumflex", "tilde", "macron", "breve",
    "dotaccent", "dieresis", "",
    "ring", "cedilla", "", "hungarumlaut", "ogonek", "caron",
    "emdash", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
    "", "AE", "", "ordfeminine", "", "", "", "",
    "Lslash", "Oslash", "OE", "ordmasculine", "", "", "", "",
    "", "ae", "", "", "", "dotlessi", "", "", "lslash",
    "oslash", "oe", "germandbls", "", "", "", "",
];

/// A standard glyph name, `.notdef` for the empty slots.
fn standard(i: usize) -> &'static [u8] {
    match STANDARD.get(i) {
        Some(s) if !s.is_empty() => s.as_bytes(),
        _ => NOTDEF,
    }
}

// charstring commands (`writet1.c`)
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

/// `cc_tab` (`cc_init`): arguments, from the bottom, clears the stack.
fn cc_entry(b: usize) -> Option<(usize, bool, bool)> {
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

/// The begin/end tokens of `/Subrs` entries (`cs_token_pairs_list`).
const CS_TOKEN_PAIRS: [(&[u8], &[u8]); 4] = [
    (b" RD", b"NP"),
    (b" -|", b"|"),
    (b" RD", b"noaccess put"),
    (b" -|", b"noaccess put"),
];

/// `POST_SUBRS_SCAN`: lines looked at for `/CharStrings` after `/Subrs`.
const POST_SUBRS_SCAN: usize = 5;

const T1_C1: u16 = 52845;
const T1_C2: u16 = 22719;

/// `cs_entry`.
#[derive(Clone, Debug, Default)]
struct CsEntry {
    name: Vec<u8>,
    data: Vec<u8>,
    cslen: usize,
    used: bool,
    valid: bool,
}

/// What a font descriptor gets from the font file and gives to it.
#[derive(Clone, Debug)]
pub struct FontDesc {
    /// `fontname`, replaced by `/FontName` of the file.
    pub fontname: Vec<u8>,
    /// `subset_tag`.
    pub subset_tag: Option<[u8; 6]>,
    /// `font_dim`: value and whether it is set.
    pub font_dim: [(i32, bool); 11],
    /// `gl_tree`: the glyphs used, by name.
    pub gl_tree: BTreeSet<Vec<u8>>,
    /// `tx_tree`: the character codes used (non-reencoded fonts).
    pub tx_tree: Option<BTreeSet<usize>>,
    /// `builtin_glyph_names`.
    pub builtin: Option<GlyphNames>,
}

/// What `writet1` leaves in `fb` and the lengths.
pub struct Embedded {
    /// The font program.
    pub bytes: Vec<u8>,
    /// `t1_length1`.
    pub length1: usize,
    /// `t1_length2`.
    pub length2: usize,
    /// `t1_length3`.
    pub length3: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Standard,
    Builtin,
}

#[allow(clippy::struct_excessive_bools)]
struct T1<'a> {
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
    len_iv: i32,
    line: Vec<u8>,
    cslen: usize,
    cs_start: usize,
    last_hexbyte: i32,
    fb: Vec<u8>,
    save_offset: usize,
    length1: usize,
    length2: usize,
    fontname_offset: usize,
    encoding: Encoding,
    fm: &'a MapEntry,
    fd: &'a mut FontDesc,
    // the CharStrings and Subrs
    cs_tab: Vec<CsEntry>,
    cs_size: usize,
    cs_count: usize,
    cs_dict_start: Vec<u8>,
    cs_dict_end: Vec<u8>,
    cs_size_pos: usize,
    cs_notdef: Option<usize>,
    subr_tab: Vec<CsEntry>,
    subr_size: i32,
    subr_array_start: Vec<u8>,
    subr_array_end: Vec<u8>,
    subr_max: i32,
    subr_size_pos: usize,
    cs_token_pair: Option<usize>,
    stack: Vec<i32>,
    last_arg_other_subr3: &'a mut i32,
}

fn fail<T>(m: impl Into<String>) -> Result<T, PdfError> {
    Err(PdfError::Font(m.into()))
}

/// `str_suffix`: whether `buf` ends with `s`, a final newline ignored.
fn str_suffix(buf: &[u8], s: &[u8]) -> bool {
    let mut b = buf;
    if b.last() == Some(&10) {
        b = &b[..b.len() - 1];
    }
    b.ends_with(s)
}

/// The text of a C string: up to the first NUL.
fn cstr(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(n) => &b[..n],
        None => b,
    }
}

/// `strstr` over a C string.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    cstr(hay).windows(needle.len()).position(|w| w == needle)
}

/// A C `%g` (strtod) number at the start of `s` after blanks, and the
/// bytes taken.
fn c_float(s: &[u8]) -> Option<(f64, usize)> {
    let mut i = 0;
    while s.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let st = i;
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let mut n = 0;
    while s.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        n += 1;
    }
    if s.get(i) == Some(&b'.') {
        i += 1;
        while s.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if s.get(j).is_some_and(u8::is_ascii_digit) {
            i = j;
            while s.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
    }
    let v = std::str::from_utf8(&s[st..i]).ok()?.parse().ok()?;
    Some((v, i))
}

/// A C `%i` (strtol, base 0) at the start of `s` after blanks.
fn c_int(s: &[u8]) -> Option<(i64, usize)> {
    let mut i = 0;
    while s.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let mut neg = false;
    if let Some(&c @ (b'+' | b'-')) = s.get(i) {
        neg = c == b'-';
        i += 1;
    }
    let (base, mut j) = if s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X')) {
        (16, i + 2)
    } else if s.get(i) == Some(&b'0') {
        (8, i)
    } else {
        (10, i)
    };
    let st = j;
    let mut v: i64 = 0;
    while let Some(d) = s.get(j).and_then(|&c| char::from(c).to_digit(base)) {
        v = v.wrapping_mul(i64::from(base)).wrapping_add(i64::from(d));
        j += 1;
    }
    if j == st {
        if base == 16 {
            // "0x" without digits is the number 0
            return Some((0, i + 1));
        }
        return None;
    }
    Some((if neg { -v } else { v }, j))
}

/// A C `%255s` at the start of `s` after blanks.
fn c_word(s: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut i = 0;
    while s.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let st = i;
    while i - st < 255
        && s.get(i)
            .is_some_and(|c| !c.is_ascii_whitespace() && *c != 0)
    {
        i += 1;
    }
    (i > st).then(|| (s[st..i].to_vec(), i))
}

/// A literal word of a `sscanf` format: blanks then the word.
fn c_lit(s: &[u8], w: &[u8]) -> Option<usize> {
    let mut i = 0;
    while s.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    s[i..].starts_with(w).then_some(i + w.len())
}

fn valid_code(c: i64) -> bool {
    (0..256).contains(&c)
}

impl<'a> T1<'a> {
    fn new(
        data: &'a [u8],
        fm: &'a MapEntry,
        fd: &'a mut FontDesc,
        last_arg: &'a mut i32,
    ) -> T1<'a> {
        // t1_init_params, t1_check_pfa
        let pfa = data.first() != Some(&128);
        T1 {
            data,
            pos: 0,
            eof: false,
            pfa,
            block_length: 0,
            dr: 55665,
            er: 55665,
            in_eexec: 0,
            cs: false,
            scan: true,
            synthetic: false,
            eexec_encrypt: false,
            len_iv: 4,
            line: Vec::new(),
            cslen: 0,
            cs_start: 0,
            last_hexbyte: 0,
            fb: Vec::new(),
            save_offset: 0,
            length1: 0,
            length2: 0,
            fontname_offset: 0,
            encoding: Encoding::Builtin,
            fm,
            fd,
            cs_tab: Vec::new(),
            cs_size: 0,
            cs_count: 0,
            cs_dict_start: Vec::new(),
            cs_dict_end: Vec::new(),
            cs_size_pos: 0,
            cs_notdef: None,
            subr_tab: Vec::new(),
            subr_size: 0,
            subr_array_start: Vec::new(),
            subr_array_end: Vec::new(),
            subr_max: -1,
            subr_size_pos: 0,
            cs_token_pair: None,
            stack: Vec::new(),
            last_arg_other_subr3: last_arg,
        }
    }

    /// `getc`.
    fn getc(&mut self) -> i32 {
        if let Some(&b) = self.data.get(self.pos) {
            self.pos += 1;
            i32::from(b)
        } else {
            self.eof = true;
            -1
        }
    }

    /// `t1_getbyte`: a byte of the font, PFB segment headers skipped.
    fn getbyte(&mut self) -> Result<i32, PdfError> {
        let mut c = self.getc();
        if self.pfa {
            return Ok(c);
        }
        if self.block_length == 0 {
            if c != 128 {
                return fail("invalid marker");
            }
            c = self.getc();
            if c == 3 {
                while !self.eof {
                    self.getc();
                }
                return Ok(-1);
            }
            let mut l = i64::from(self.getc() & 0xff);
            l |= i64::from(self.getc() & 0xff) << 8;
            l |= i64::from(self.getc() & 0xff) << 16;
            l |= i64::from(self.getc() & 0xff) << 24;
            self.block_length = l;
            c = self.getc();
        }
        self.block_length -= 1;
        Ok(c)
    }

    /// `edecrypt`.
    fn edecrypt(&mut self, cipher: u8) -> Result<u8, PdfError> {
        let mut cipher = cipher;
        if self.pfa {
            let mut c = i32::from(cipher);
            while c == 10 || c == 13 {
                c = self.getbyte()?;
            }
            let hi = hexval(c);
            let lo = hexval(self.getbyte()?);
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            {
                cipher = ((hi << 4) + lo) as u8;
            }
            self.last_hexbyte = i32::from(cipher);
        }
        let plain = cipher ^ (self.dr >> 8) as u8;
        self.dr = (u16::from(cipher).wrapping_add(self.dr))
            .wrapping_mul(T1_C1)
            .wrapping_add(T1_C2);
        Ok(plain)
    }

    /// `eencrypt`.
    fn eencrypt(&mut self, plain: u8) -> u8 {
        let cipher = plain ^ (self.er >> 8) as u8;
        self.er = (u16::from(cipher).wrapping_add(self.er))
            .wrapping_mul(T1_C1)
            .wrapping_add(T1_C2);
        cipher
    }

    /// `t1_getline`.
    fn getline(&mut self) -> Result<(), PdfError> {
        const EEXEC: &[u8] = b"currentfile eexec";
        loop {
            if self.eof {
                return fail("unexpected end of file");
            }
            self.line.clear();
            self.cslen = 0;
            let mut eexec_scan: i32 = 0;
            let mut c = self.getbyte()?;
            if c == -1 {
                return Ok(());
            }
            while !self.eof {
                if self.in_eexec == 1 {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    let b = c as u8;
                    c = i32::from(self.edecrypt(b)?);
                }
                c = append_char(c, &mut self.line);
                if self.in_eexec == 0 && (0..17).contains(&eexec_scan) {
                    #[allow(clippy::cast_sign_loss)]
                    let k = eexec_scan as usize;
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
                    && (str_suffix(&self.line, b" RD ") || str_suffix(&self.line, b" -| "))
                {
                    let mut p = self.line.len() - 5;
                    while self.line[p] != b' ' {
                        p -= 1;
                    }
                    let Some((l, _)) = c_float(&self.line[p + 1..]) else {
                        return fail("a number expected");
                    };
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let l = l as usize;
                    self.cslen = l;
                    self.cs_start = self.line.len();
                    for _ in 0..l {
                        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                        let b = self.getbyte()? as u8;
                        let d = self.edecrypt(b)?;
                        self.line.push(d);
                    }
                }
                c = self.getbyte()?;
            }
            append_eol(&mut self.line);
            if self.line.len() < 2 {
                continue;
            }
            if eexec_scan == 17 {
                self.in_eexec = 1;
            }
            return Ok(());
        }
    }

    /// `t1_putline`.
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

    /// `t1_puts`.
    fn puts(&mut self, s: &[u8]) {
        self.line = s.to_vec();
        self.putline();
    }

    fn prefix(&self, s: &[u8]) -> bool {
        self.line.starts_with(s)
    }

    fn charstrings(&self) -> bool {
        find(&self.line, b"/CharStrings").is_some()
    }

    fn subrs(&self) -> bool {
        self.prefix(b"/Subrs")
    }

    fn end_eexec(&self) -> bool {
        str_suffix(&self.line, b"mark currentfile closefile")
    }

    /// `t1_check_block_len`.
    fn check_block_len(&mut self, decrypt: bool) -> Result<(), PdfError> {
        if self.block_length == 0 {
            return Ok(());
        }
        let mut c = self.getbyte()?;
        if decrypt {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let b = c as u8;
            c = i32::from(self.edecrypt(b)?);
        }
        let l = self.block_length;
        if !(l == 0 && (c == 10 || c == 13)) {
            return fail(format!("{} bytes more than expected", l + 1));
        }
        Ok(())
    }

    /// `t1_start_eexec`.
    fn start_eexec(&mut self) -> Result<(), PdfError> {
        self.length1 = self.fb.len() - self.save_offset;
        self.save_offset = self.fb.len();
        if !self.pfa {
            self.check_block_len(false)?;
        }
        self.line.clear();
        for _ in 0..4 {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let b = self.getbyte()? as u8;
            self.edecrypt(b)?;
            self.line.push(0);
        }
        self.eexec_encrypt = true;
        self.putline();
        Ok(())
    }

    /// `t1_stop_eexec`.
    fn stop_eexec(&mut self) -> Result<(), PdfError> {
        self.length2 = self.fb.len() - self.save_offset;
        self.save_offset = self.fb.len();
        self.eexec_encrypt = false;
        if self.pfa {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let b = self.getbyte()? as u8;
            let c = self.edecrypt(b)?;
            if !(c == 10 || c == 13) {
                if self.last_hexbyte == 0 {
                    self.puts(b"00");
                } else {
                    return fail("unexpected data after eexec");
                }
            }
        } else {
            self.check_block_len(true)?;
        }
        self.cs = false;
        self.in_eexec = 2;
        Ok(())
    }

    /// `t1_scan_keys`.
    #[allow(clippy::cast_possible_truncation)]
    fn scan_keys(&mut self) -> Result<(), PdfError> {
        if self.fm.extend != 0 || self.fm.slant != 0 {
            if self.prefix(b"/FontMatrix") {
                return self.modify_fm();
            }
            if self.prefix(b"/ItalicAngle") {
                self.modify_italic();
                return Ok(());
            }
        }
        if self.prefix(b"/FontType") {
            let p = &self.line[9..];
            let Some((i, _)) = c_float(p.strip_prefix(b" ").unwrap_or(p)) else {
                return fail("a number expected");
            };
            if i as i32 != 1 {
                return fail(format!("Type{} fonts unsupported by pdfTeX", i as i32));
            }
            return Ok(());
        }
        let Some(k) = FONT_KEY
            .iter()
            .position(|(_, t1)| !t1.is_empty() && self.line[1..].starts_with(t1.as_bytes()))
        else {
            return Ok(());
        };
        let mut p = FONT_KEY[k].1.len() + 1;
        if self.line.get(p) == Some(&b' ') {
            p += 1;
        }
        if k == dim::FONTNAME {
            if self.line.get(p) != Some(&b'/') {
                return fail("a name expected");
            }
            p += 1;
            let r = p;
            while !matches!(self.line.get(p), Some(b' ' | 10) | None) {
                p += 1;
            }
            let mut name = self.line[r..p].to_vec();
            if self.fm.slant != 0 {
                name.extend_from_slice(format!("-Slant_{}", self.fm.slant).as_bytes());
            }
            if self.fm.extend != 0 {
                name.extend_from_slice(format!("-Extend_{}", self.fm.extend).as_bytes());
            }
            self.fd.fontname.clone_from(&name);
            if self.fm.is_subsetted() {
                self.fontname_offset = self.fb.len() + r;
                let rest = self.line[p..].to_vec();
                self.line.truncate(r);
                self.line.extend_from_slice(b"ABCDEF+");
                self.line.extend_from_slice(&name);
                self.line.extend_from_slice(cstr(&rest));
                // eol(r)
                if self.line.len() - r > 1 && self.line.last() != Some(&10) {
                    self.line.push(10);
                }
            }
            return Ok(());
        }
        if (k == dim::STEMV || k == dim::FONTBBOX1) && matches!(self.line.get(p), Some(b'[' | b'{'))
        {
            p += 1;
        }
        if k == dim::FONTBBOX1 {
            for i in 0..4 {
                let s = &self.line[p..];
                let s = s.strip_prefix(b" ").unwrap_or(s);
                let skipped = self.line.len() - p - s.len();
                let Some((v, _)) = c_float(s) else {
                    return fail("a number expected");
                };
                self.fd.font_dim[k + i] = (v as f32 as i32, true);
                // the number's own characters
                let mut q = p + skipped;
                while self.line.get(q).is_some_and(|c| {
                    c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')
                }) {
                    q += 1;
                }
                p = q;
            }
            return Ok(());
        }
        let s = &self.line[p..];
        let Some((v, _)) = c_float(s.strip_prefix(b" ").unwrap_or(s)) else {
            return fail("a number expected");
        };
        self.fd.font_dim[k] = (v as f32 as i32, true);
        Ok(())
    }

    /// `t1_modify_fm`: the font matrix slanted, then extended.
    #[allow(clippy::cast_possible_truncation)]
    fn modify_fm(&mut self) -> Result<(), PdfError> {
        let Some(open) = self.line.iter().position(|&c| c == b'[' || c == b'{') else {
            return fail("FontMatrix: an array expected");
        };
        let c = self.line[open];
        let mut p = open + 1;
        let mut a = [0f32; 6];
        for x in &mut a {
            let (v, q) = scan_num(&self.line, p)?;
            *x = v;
            p = q;
        }
        if self.fm.slant != 0 {
            let s = f64::from(self.fm.slant) * 1e-3;
            a[0] = (f64::from(a[0]) + f64::from(a[1]) * s) as f32;
            a[2] = (f64::from(a[2]) + f64::from(a[3]) * s) as f32;
            a[4] = (f64::from(a[4]) + f64::from(a[5]) * s) as f32;
        }
        if self.fm.extend != 0 {
            let e = f64::from(self.fm.extend) * 1e-3;
            a[0] = (f64::from(a[0]) * e) as f32;
            a[2] = (f64::from(a[2]) * e) as f32;
            a[4] = (f64::from(a[4]) * e) as f32;
        }
        let close = if c == b'[' { b']' } else { b'}' };
        while p < self.line.len() && self.line[p] != close {
            p += 1;
        }
        if p == self.line.len() {
            return fail(format!(
                "FontMatrix: cannot find the corresponding character to '{}'",
                char::from(c)
            ));
        }
        let mut l = self.line[..=open].to_vec();
        for x in a {
            l.extend_from_slice(fmt_g(f64::from(x)).as_bytes());
            l.push(b' ');
        }
        l.extend_from_slice(&self.line[p..]);
        self.line = l;
        eol(&mut self.line);
        Ok(())
    }

    /// `t1_modify_italic`.
    #[allow(clippy::cast_possible_truncation)]
    fn modify_italic(&mut self) {
        if self.fm.slant == 0 {
            return;
        }
        let Some(sp) = self.line.iter().position(|&c| c == b' ') else {
            return;
        };
        let Ok((a, r)) = scan_num(&self.line, sp + 1) else {
            return;
        };
        let a = (f64::from(a)
            - (f64::from(self.fm.slant) * 1e-3).atan() * (180.0 / std::f64::consts::PI))
            as f32;
        let mut l = self.line[..=sp].to_vec();
        l.extend_from_slice(fmt_g(f64::from(a)).as_bytes());
        l.extend_from_slice(&self.line[r..]);
        self.line = l;
        eol(&mut self.line);
        self.fd.font_dim[dim::ITALIC_ANGLE] = (f64::from(a).round() as i32, true);
    }

    /// `t1_scan_param`.
    fn scan_param(&mut self) -> Result<(), PdfError> {
        if !self.scan || self.line.first() != Some(&b'/') {
            return Ok(());
        }
        if self.prefix(b"/lenIV") {
            let s = &self.line[6..];
            let Some((v, _)) = c_float(s.strip_prefix(b" ").unwrap_or(s)) else {
                return fail("a number expected");
            };
            #[allow(clippy::cast_possible_truncation)]
            {
                self.len_iv = i32::from(v as f32 as i16);
            }
            if self.len_iv < 0 {
                return fail("negative value of lenIV is not supported");
            }
            return Ok(());
        }
        self.scan_keys()
    }

    /// `t1_builtin_enc`.
    #[allow(clippy::too_many_lines)]
    fn builtin_enc(&mut self) -> Result<GlyphNames, PdfError> {
        let mut names: GlyphNames = vec![None; 256];
        if str_suffix(&self.line, b"def") {
            if let Some((w, _)) = c_word(&self.line[9..])
                && w == b"StandardEncoding"
            {
                self.encoding = Encoding::Standard;
                for (i, n) in names.iter_mut().enumerate() {
                    let s = standard(i);
                    if s != NOTDEF {
                        *n = Some(s.to_vec());
                    }
                }
                return Ok(names);
            }
            return fail("cannot subset font (unknown predefined encoding)");
        }
        self.encoding = Encoding::Builtin;
        if self.prefix(b"/Encoding [") || self.prefix(b"/Encoding[") {
            let mut counter = 0usize;
            let mut r = self.line.iter().position(|&c| c == b'[').unwrap_or(0) + 1;
            if self.line.get(r) == Some(&b' ') {
                r += 1;
            }
            loop {
                while self.line.get(r) == Some(&b'/') {
                    r += 1;
                    let st = r;
                    while !matches!(self.line.get(r), Some(b' ' | 10 | b']' | b'/') | None) {
                        r += 1;
                    }
                    let name = self.line[st..r].to_vec();
                    if self.line.get(r) == Some(&b' ') {
                        r += 1;
                    }
                    if counter > 255 {
                        return fail("encoding vector contains more than 256 names");
                    }
                    if name != NOTDEF {
                        names[counter] = Some(name);
                    }
                    counter += 1;
                }
                if !matches!(self.line.get(r), Some(10 | b'%')) {
                    let rest = &self.line[r.min(self.line.len())..];
                    if rest.starts_with(b"] def") || rest.starts_with(b"] readonly def") {
                        break;
                    }
                    return fail("a name or `] def' or `] readonly def' expected");
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
        loop {
            if self.line.get(p).is_none_or(|&c| c == 10) {
                self.getline()?;
                p = 0;
            }
            let s = &self.line[p..];
            if let Some((i, j)) =
                c_lit(s, b"dup").and_then(|a| c_int(&s[a..]).map(|(i, b)| (i, a + b)))
                && let Some((w, _)) = c_word(&s[j..])
                && w[0] == b'/'
                && valid_code(i)
            {
                if &w[1..] != NOTDEF {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    {
                        names[i as usize] = Some(w[1..].to_vec());
                    }
                }
                let Some(k) = find(s, b" put") else {
                    return fail("invalid pfb, no put found in dup");
                };
                p += k + 4;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
            } else if let Some((b, a)) = c_lit(s, b"dup")
                .and_then(|x| c_lit(&s[x..], b"dup").map(|y| x + y))
                .and_then(|x| c_int(&s[x..]).map(|(b, y)| (b, x + y)))
                .and_then(|(b, x)| c_lit(&s[x..], b"exch").map(|y| (b, x + y)))
                .and_then(|(b, x)| c_int(&s[x..]).map(|(a, _)| (b, a)))
                && valid_code(a)
                && valid_code(b)
            {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                copy_glyph_names(&mut names, a as usize, b as usize);
                let Some(k) = find(s, b" get put") else {
                    return fail("invalid pfb, no get put found in dup dup");
                };
                p += k + 8;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
            } else if let Some((a, c, b)) = scan_getinterval(s)
                && valid_code(a)
                && valid_code(b)
                && valid_code(c)
            {
                for i in 0..c {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    copy_glyph_names(&mut names, (a + i) as usize, (b + i) as usize);
                }
                let Some(k) = find(s, b" putinterval") else {
                    return fail("invalid pfb, no putinterval found in dup dup");
                };
                p += k + 12;
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
            } else if (p == 0 || self.line[p - 1] == b' ') && cstr(s) == b"def\n" {
                return Ok(names);
            } else {
                while !matches!(self.line.get(p), Some(b' ' | 10) | None) {
                    p += 1;
                }
                if self.line.get(p) == Some(&b' ') {
                    p += 1;
                }
            }
        }
    }

    /// `t1_include`: the whole font.
    fn include(&mut self) -> Result<(), PdfError> {
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

    fn is_unique_id_def(&self) -> bool {
        let l = cstr(&self.line);
        self.prefix(b"/UniqueID") && l.len() >= 4 && l[l.len() - 4..].starts_with(b"def")
    }

    /// `t1_subset_ascii_part`.
    fn subset_ascii_part(&mut self, tags: &mut BTreeSet<[u8; 6]>) -> Result<(), PdfError> {
        self.getline()?;
        while !self.prefix(b"/Encoding") {
            self.scan_param()?;
            if !self.is_unique_id_def() {
                self.putline();
            }
            self.getline()?;
        }
        let names = self.builtin_enc()?;
        if self.fm.is_subsetted() {
            if let Some(tx) = &self.fd.tx_tree {
                for &c in tx {
                    let g = names[c].clone().unwrap_or_else(|| NOTDEF.to_vec());
                    self.fd.gl_tree.insert(g);
                }
            }
            let tag = make_subset_tag(&self.fd.gl_tree, &self.fd.fontname, tags);
            self.fd.subset_tag = Some(tag);
            self.fb[self.fontname_offset..self.fontname_offset + 6].copy_from_slice(&tag);
        }
        if self.encoding == Encoding::Standard {
            self.puts(b"/Encoding StandardEncoding def\n");
        } else {
            self.puts(b"/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n");
            // create_t1_glyph_tree: each name at its first code
            let mut first: BTreeMap<&[u8], usize> = BTreeMap::new();
            for (i, n) in names.iter().enumerate() {
                if let Some(n) = n {
                    first.entry(n.as_slice()).or_insert(i);
                }
            }
            let mut lines = Vec::new();
            for g in &self.fd.gl_tree {
                if let Some(&i) = first.get(g.as_slice()) {
                    let mut l = format!("dup {i} /").into_bytes();
                    l.extend_from_slice(g);
                    l.extend_from_slice(b" put\n");
                    lines.push(l);
                }
            }
            let j = lines.len();
            for l in lines {
                self.puts(&l);
            }
            if j == 0 {
                self.puts(b"dup 0 /.notdef put\n");
            }
            self.puts(b"readonly def\n");
        }
        self.fd.builtin = Some(names);
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
        Ok(())
    }

    /// `cs_store`.
    fn cs_store(&mut self, is_subr: bool) -> Result<(), PdfError> {
        let sp = self
            .line
            .iter()
            .position(|&c| c == b' ')
            .unwrap_or(self.line.len());
        let token = self.line[..sp].to_vec();
        let idx;
        if is_subr {
            let s = &self.line[sp + 1..];
            let Some((v, _)) = c_float(s) else {
                return fail("a number expected");
            };
            #[allow(clippy::cast_possible_truncation)]
            let subr = v as f32 as i32;
            if subr >= self.subr_size || subr < 0 {
                return fail(format!("Subrs array: entry index out of range ({subr})"));
            }
            #[allow(clippy::cast_sign_loss)]
            {
                idx = subr as usize;
            }
        } else {
            idx = self.cs_tab.len();
            if idx + 1 > self.cs_size {
                return fail(format!(
                    "CharStrings dict: more entries than dict size ({})",
                    self.cs_size
                ));
            }
            self.cs_tab.push(CsEntry {
                name: token[1..].to_vec(),
                ..CsEntry::default()
            });
        }
        // " RD " + cs data + the rest of the line
        let mut data = self.line[self.cs_start - 4..self.cs_start + self.cslen].to_vec();
        let mut p = self.cs_start + self.cslen;
        while self.line.get(p).is_some_and(|&c| c != 10) {
            data.push(self.line[p]);
            p += 1;
        }
        data.push(10);
        if is_subr && self.cs_token_pair.is_none() {
            self.cs_token_pair = CS_TOKEN_PAIRS
                .iter()
                .position(|(a, b)| data.starts_with(a) && str_suffix(&data, b));
        }
        let e = if is_subr {
            &mut self.subr_tab[idx]
        } else {
            &mut self.cs_tab[idx]
        };
        e.data = data;
        e.cslen = self.cslen;
        e.valid = true;
        Ok(())
    }

    /// `t1_read_subrs`.
    fn read_subrs(&mut self) -> Result<(), PdfError> {
        self.getline()?;
        while !(self.charstrings() || self.subrs()) {
            self.scan_param()?;
            if !self.prefix(b"/UniqueID") {
                self.putline();
            }
            self.getline()?;
        }
        loop {
            // found:
            self.cs = true;
            self.scan = false;
            if !self.subrs() {
                return Ok(());
            }
            self.subr_size_pos = 7;
            let Some((v, _)) = c_float(&self.line[7..]) else {
                return fail("a number expected");
            };
            #[allow(clippy::cast_possible_truncation)]
            {
                self.subr_size = v as f32 as i32;
            }
            if self.subr_size == 0 {
                while !self.charstrings() {
                    self.getline()?;
                }
                return Ok(());
            }
            self.subr_tab = vec![CsEntry::default(); usize::try_from(self.subr_size).unwrap_or(0)];
            self.subr_array_start = cstr(&self.line).to_vec();
            self.getline()?;
            while self.cslen != 0 {
                self.cs_store(true)?;
                self.getline()?;
            }
            for e in self.subr_tab.iter_mut().take(4) {
                e.used = true;
            }
            let mut end = Vec::new();
            let mut i = 0;
            while i < POST_SUBRS_SCAN {
                if self.charstrings() {
                    break;
                }
                end.extend_from_slice(cstr(&self.line));
                self.getline()?;
                i += 1;
            }
            self.subr_array_end = end;
            if i < POST_SUBRS_SCAN {
                return Ok(());
            }
            // CharStrings not found: a synthetic font
            self.subr_tab.clear();
            self.subr_array_start.clear();
            self.subr_array_end.clear();
            self.subr_size = 0;
            self.subr_size_pos = 0;
            self.subr_max = 0;
            self.cs_token_pair = None;
            self.cs = false;
            self.synthetic = true;
            while !(self.charstrings() || self.subrs()) {
                self.getline()?;
            }
        }
    }

    /// `cs_mark(cs_name, subr)`.
    #[allow(clippy::too_many_lines)]
    fn cs_mark(&mut self, cs_name: Option<&[u8]>, subr: i32) -> Result<(), PdfError> {
        let is_subr = cs_name.is_none();
        let idx = if let Some(name) = cs_name {
            if self.cs_notdef.is_some() && name == NOTDEF {
                self.cs_notdef.unwrap_or(0)
            } else {
                let Some(i) = self.cs_tab.iter().position(|e| e.name == name) else {
                    // "glyph `%s' undefined": a warning
                    return Ok(());
                };
                if self.cs_tab[i].name == NOTDEF {
                    self.cs_notdef = Some(i);
                }
                i
            }
        } else {
            if subr >= self.subr_size || subr < 0 {
                return fail(format!("Subrs array: entry index out of range ({subr})"));
            }
            #[allow(clippy::cast_sign_loss)]
            let i = subr as usize;
            if !self.subr_tab[i].valid {
                return Ok(());
            }
            i
        };
        let e = if is_subr {
            &mut self.subr_tab[idx]
        } else {
            &mut self.cs_tab[idx]
        };
        if !e.valid || (e.used && !is_subr) {
            return Ok(());
        }
        e.used = true;
        let cslen = e.cslen;
        // the charstring decrypted (cr = 4330), and what follows it
        let mut cr: u16 = 4330;
        let plain: Vec<u8> = e.data[4..].iter().map(|&c| cdecrypt(c, &mut cr)).collect();
        let name_for = |m: &str| match cs_name {
            None => format!("Subr ({subr}): {m}"),
            Some(n) => format!("CharString (/{}): {m}", String::from_utf8_lossy(n)),
        };
        let mut k = 0usize;
        let mut get = || {
            let b = plain.get(k).copied().unwrap_or(0);
            k += 1;
            i32::from(b)
        };
        #[allow(clippy::cast_possible_wrap)]
        let mut cs_len = cslen as i64;
        for _ in 0..self.len_iv {
            get();
            cs_len -= 1;
        }
        let mut last_cmd = 0usize;
        while cs_len > 0 {
            cs_len -= 1;
            let b = get();
            if b >= 32 {
                let a = if b <= 246 {
                    b - 139
                } else if b <= 250 {
                    cs_len -= 1;
                    ((b - 247) << 8) + 108 + get()
                } else if b <= 254 {
                    cs_len -= 1;
                    -((b - 251) << 8) - 108 - get()
                } else {
                    cs_len -= 4;
                    let mut a = (get() & 0xff) << 24;
                    a |= (get() & 0xff) << 16;
                    a |= (get() & 0xff) << 8;
                    a |= get() & 0xff;
                    a
                };
                self.stack.push(a);
                continue;
            }
            #[allow(clippy::cast_sign_loss)]
            let mut b = b as usize;
            if b == CS_ESCAPE {
                #[allow(clippy::cast_sign_loss)]
                {
                    b = get() as usize + CS_1BYTE_MAX;
                }
                cs_len -= 1;
            }
            if b >= CS_MAX {
                return fail(name_for(&format!("command value out of range: {b}")));
            }
            let Some((nargs, bottom, clear)) = cc_entry(b) else {
                return fail(name_for(&format!("command not valid: {b}")));
            };
            if bottom {
                let n = self.stack.len();
                if n < nargs {
                    return fail(name_for(&format!(
                        "less arguments on stack ({n}) than required ({nargs})"
                    )));
                } else if n > nargs {
                    return fail(name_for(&format!(
                        "more arguments on stack ({n}) than required ({nargs})"
                    )));
                }
            }
            last_cmd = b;
            let stack_error = |n: usize, have: usize| {
                PdfError::Font(format!(
                    "CharString: invalid access ({n}) to stack ({have} entries)"
                ))
            };
            match b {
                CS_CALLSUBR => {
                    let Some(a1) = self.stack.pop() else {
                        return Err(stack_error(1, 0));
                    };
                    self.cs_mark(None, a1)?;
                    #[allow(clippy::cast_sign_loss)]
                    if !self.subr_tab[a1 as usize].valid {
                        return fail(name_for(&format!("cannot call subr ({a1})")));
                    }
                }
                CS_DIV => {
                    let n = self.stack.len();
                    if n < 2 {
                        return Err(stack_error(2, n));
                    }
                    self.stack.truncate(n - 2);
                    self.stack.push(0);
                }
                CS_CALLOTHERSUBR => {
                    let n = self.stack.len();
                    if n < 2 {
                        return Err(stack_error(2, n));
                    }
                    if self.stack[n - 1] == 3 && n >= 3 {
                        *self.last_arg_other_subr3 = self.stack[n - 3];
                    }
                    let a1 = self.stack[n - 2] + 2;
                    let a1 = usize::try_from(a1).unwrap_or(usize::MAX);
                    if n < a1 {
                        return Err(stack_error(a1, n));
                    }
                    self.stack.truncate(n - a1);
                }
                CS_POP => self.stack.push(*self.last_arg_other_subr3),
                CS_SEAC => {
                    let a1 = usize::try_from(self.stack[3]).unwrap_or(usize::MAX);
                    let a2 = usize::try_from(self.stack[4]).unwrap_or(usize::MAX);
                    self.stack.clear();
                    let (g1, g2) = (standard(a1), standard(a2));
                    self.cs_mark(Some(g1), 0)?;
                    self.cs_mark(Some(g2), 0)?;
                    self.fd.gl_tree.insert(g1.to_vec());
                    self.fd.gl_tree.insert(g2.to_vec());
                }
                _ => {
                    if clear {
                        self.stack.clear();
                    }
                }
            }
        }
        if is_subr && last_cmd != CS_RETURN {
            // "last command in subr is not a RETURN": append_cs_return
            let e = &mut self.subr_tab[idx];
            let mut cr: u16 = 4330;
            let mut plain: Vec<u8> = e.data[4..4 + e.cslen]
                .iter()
                .map(|&c| cdecrypt(c, &mut cr))
                .collect();
            #[allow(clippy::cast_possible_truncation)]
            plain.push(CS_RETURN as u8);
            let mut cr: u16 = 4330;
            let mut new = e.data[..4].to_vec();
            new.extend(plain.iter().map(|&p| cencrypt(p, &mut cr)));
            new.extend_from_slice(&e.data[4 + e.cslen..]);
            e.data = new;
            e.cslen += 1;
        }
        Ok(())
    }

    /// `t1_flush_cs`.
    fn flush_cs(&mut self, is_subr: bool) {
        let (start_line, line_end, size_pos, count) = if is_subr {
            (
                std::mem::take(&mut self.subr_array_start),
                std::mem::take(&mut self.subr_array_end),
                self.subr_size_pos,
                usize::try_from(self.subr_max + 1).unwrap_or(0),
            )
        } else {
            (
                std::mem::take(&mut self.cs_dict_start),
                std::mem::take(&mut self.cs_dict_end),
                self.cs_size_pos,
                self.cs_count,
            )
        };
        let mut l = start_line[..size_pos.min(start_line.len())].to_vec();
        let mut p = size_pos.min(start_line.len());
        while start_line.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        l.extend_from_slice(count.to_string().as_bytes());
        l.extend_from_slice(&start_line[p..]);
        eol(&mut l);
        self.line = l;
        self.putline();
        let (open, close) = CS_TOKEN_PAIRS[self.cs_token_pair.unwrap_or(0)];
        let mut return_cs = Vec::new();
        if is_subr {
            let mut cr: u16 = 4330;
            for _ in 0..self.len_iv {
                return_cs.push(cencrypt(0, &mut cr));
            }
            #[allow(clippy::cast_possible_truncation)]
            return_cs.push(cencrypt(CS_RETURN as u8, &mut cr));
        }
        let tab = if is_subr {
            std::mem::take(&mut self.subr_tab)
        } else {
            std::mem::take(&mut self.cs_tab)
        };
        let end = if is_subr {
            count.min(tab.len())
        } else {
            tab.len()
        };
        for (i, e) in tab[..end].iter().enumerate() {
            if e.used {
                let mut l = if is_subr {
                    format!("dup {i} {}", e.cslen).into_bytes()
                } else {
                    let mut l = b"/".to_vec();
                    l.extend_from_slice(&e.name);
                    l.extend_from_slice(format!(" {}", e.cslen).as_bytes());
                    l
                };
                l.extend_from_slice(&e.data);
                self.line = l;
                self.putline();
            } else if is_subr {
                let mut l = format!("dup {i} {}", return_cs.len()).into_bytes();
                l.extend_from_slice(open);
                l.push(b' ');
                l.extend_from_slice(&return_cs);
                self.line = l;
                self.putline();
                let mut l = b" ".to_vec();
                l.extend_from_slice(close);
                eol(&mut l);
                self.line = l;
                self.putline();
            }
        }
        let mut l = line_end;
        eol(&mut l);
        self.line = l;
        self.putline();
    }

    /// `t1_mark_glyphs`.
    fn mark_glyphs(&mut self) -> Result<(), PdfError> {
        if self.synthetic {
            for e in &mut self.cs_tab {
                if e.valid {
                    e.used = true;
                }
            }
            for e in &mut self.subr_tab {
                if e.valid {
                    e.used = true;
                }
            }
            self.subr_max = self.subr_size - 1;
            return Ok(());
        }
        self.cs_mark(Some(NOTDEF), 0)?;
        let glyphs: Vec<Vec<u8>> = self.fd.gl_tree.iter().cloned().collect();
        for g in &glyphs {
            self.cs_mark(Some(g), 0)?;
        }
        if !self.subr_tab.is_empty() {
            self.subr_max = -1;
            for (i, e) in self.subr_tab.iter().enumerate() {
                let i = i32::try_from(i).unwrap_or(i32::MAX);
                if e.used && i > self.subr_max {
                    self.subr_max = i;
                }
            }
        }
        Ok(())
    }

    /// `t1_check_unusual_charstring`.
    fn check_unusual_charstring(&mut self) -> Result<(), PdfError> {
        let p = find(&self.line, b"/CharStrings").unwrap_or(0) + 12;
        if c_int(&self.line[p..]).is_none() {
            let mut buf = cstr(&self.line).to_vec();
            if let Some(last) = buf.last_mut() {
                *last = b' ';
            }
            self.getline()?;
            buf.extend_from_slice(cstr(&self.line));
            eol(&mut buf);
            self.line = buf;
        }
        Ok(())
    }

    /// `t1_subset_charstrings`.
    fn subset_charstrings(&mut self) -> Result<(), PdfError> {
        self.check_unusual_charstring()?;
        self.cs_size_pos = find(&self.line, b"/CharStrings").unwrap_or(0) + 12 + 1;
        let Some((v, _)) = c_float(&self.line[self.cs_size_pos.min(self.line.len())..]) else {
            return fail("a number expected");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            self.cs_size = v as f32 as usize;
        }
        self.cs_tab.clear();
        self.cs_notdef = None;
        self.cs_dict_start = cstr(&self.line).to_vec();
        self.getline()?;
        while self.cslen != 0 {
            self.cs_store(false)?;
            self.getline()?;
        }
        self.cs_dict_end = cstr(&self.line).to_vec();
        self.mark_glyphs()?;
        if !self.subr_tab.is_empty() {
            if self.cs_token_pair.is_none() {
                return fail("This Type 1 font uses mismatched subroutine begin/end token pairs.");
            }
            self.flush_cs(true);
        }
        self.cs_count = self.cs_tab.iter().filter(|e| e.used).count();
        self.flush_cs(false);
        Ok(())
    }

    /// `t1_subset_end`.
    fn subset_end(&mut self) -> Result<(), PdfError> {
        if self.synthetic {
            while find(&self.line, b"definefont").is_none() {
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

/// `t1_scan_num` at `p`: the number (a `float`) and where it ends.
#[allow(clippy::cast_possible_truncation)]
fn scan_num(line: &[u8], mut p: usize) -> Result<(f32, usize), PdfError> {
    if line.get(p) == Some(&b' ') {
        p += 1;
    }
    let Some((v, _)) = c_float(&line[p.min(line.len())..]) else {
        return fail("a number expected");
    };
    while line
        .get(p)
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        p += 1;
    }
    Ok((v as f32, p))
}

/// C's `%g`.
pub(crate) fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let e = format!("{x:.5e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if (-4..6).contains(&exp) {
        let prec = usize::try_from(5 - exp).unwrap_or(0);
        strip(&format!("{x:.prec$}"))
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mant), exp.abs())
    }
}

/// `eol` (`writet1.c`): a newline at the end of a line of more than one
/// character.
fn eol(l: &mut Vec<u8>) {
    if l.len() > 1 && l.last() != Some(&10) {
        l.push(10);
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

/// `cdecrypt`.
fn cdecrypt(cipher: u8, cr: &mut u16) -> u8 {
    let plain = cipher ^ (*cr >> 8) as u8;
    *cr = (u16::from(cipher).wrapping_add(*cr))
        .wrapping_mul(T1_C1)
        .wrapping_add(T1_C2);
    plain
}

/// `cencrypt`.
fn cencrypt(plain: u8, cr: &mut u16) -> u8 {
    let cipher = plain ^ (*cr >> 8) as u8;
    *cr = (u16::from(cipher).wrapping_add(*cr))
        .wrapping_mul(T1_C1)
        .wrapping_add(T1_C2);
    cipher
}

/// `copy_glyph_names`.
fn copy_glyph_names(names: &mut GlyphNames, a: usize, b: usize) {
    names[b] = names[a].clone();
}

/// `sscanf(p, "dup dup %i %i getinterval %i exch putinterval")`.
fn scan_getinterval(s: &[u8]) -> Option<(i64, i64, i64)> {
    let mut i = c_lit(s, b"dup")?;
    i += c_lit(&s[i..], b"dup")?;
    let (a, j) = c_int(&s[i..])?;
    i += j;
    let (c, j) = c_int(&s[i..])?;
    i += j;
    i += c_lit(&s[i..], b"getinterval")?;
    let (b, _) = c_int(&s[i..])?;
    Some((a, c, b))
}

/// `make_subset_tag` (`utils.c`): six letters from the MD5 of the glyph
/// names and the font name, unique in the document.
#[must_use]
pub fn make_subset_tag(
    glyphs: &BTreeSet<Vec<u8>>,
    fontname: &[u8],
    tags: &mut BTreeSet<[u8; 6]>,
) -> [u8; 6] {
    let mut j: i32 = 0;
    loop {
        let mut m = Vec::new();
        for g in glyphs {
            m.extend_from_slice(g);
            m.push(b' ');
        }
        m.extend_from_slice(fontname);
        m.extend_from_slice(&j.to_le_bytes());
        let d = phitex_codecs::md5::md5(&m);
        let mut a = [0i32; 6];
        a[0] = d[..13].iter().map(|&x| i32::from(x)).sum();
        for i in 1..6 {
            a[i] = a[i - 1] - i32::from(d[i - 1]) + i32::from(d[(i + 12) % 16]);
        }
        let mut tag = [0u8; 6];
        for (t, x) in tag.iter_mut().zip(a) {
            *t = u8::try_from(x % 26).unwrap_or(0) + b'A';
        }
        j += 1;
        if tags.insert(tag) {
            return tag;
        }
    }
}

/// `writet1`: the font program to embed, whole or subset, and the font
/// descriptor's values read from it.
pub fn write_t1(
    data: &[u8],
    fm: &MapEntry,
    fd: &mut FontDesc,
    tags: &mut BTreeSet<[u8; 6]>,
    last_arg: &mut i32,
) -> Result<Embedded, PdfError> {
    let mut t = T1::new(data, fm, fd, last_arg);
    if fm.is_subsetted() {
        t.subset_ascii_part(tags)?;
        t.start_eexec()?;
        t.read_subrs()?;
        t.subset_charstrings()?;
        t.subset_end()?;
    } else {
        t.include()?;
    }
    Ok(Embedded {
        length1: t.length1,
        length2: t.length2,
        length3: 0,
        bytes: t.fb,
    })
}
