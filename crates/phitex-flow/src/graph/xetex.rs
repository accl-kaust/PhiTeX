//! XeTeX on the graph (xetex.web; CORE.md, "XeTeX on the graph"): the
//! reader's Unicode (the input decoded as the encoding in force says,
//! a read like the catcodes), XeTeX's codes and their scanning, native
//! fonts (a node defining the font as a value: its face, features, size
//! and metrics), the characters of a native font set as words (shaping is
//! a pure function of the font value, the text and the default
//! direction, `Key::NativeDir`), `\XeTeXinterchartoks`,
//! `\XeTeXlinebreaklocale` (ICU's line breaks, copied: `icu_linebreak`),
//! the font queries (pure functions of the font value), pictures (a value
//! measured from the file's bytes) and the encodings.

use std::sync::Arc;

use super::scan::{IVal, Lv};
use super::{Exec, Kind, Mode};
use crate::item::{Disc, Item, Whatsit};
use crate::native::{self, NativeWord};
use crate::tfm::Font;
use crate::val::{Code, GluePar, IntPar, Key, Meaning, Prim, Tok, Val, Xe};

/// `XeTeX_revision`.
pub const XETEX_REVISION: &str = ".999998";

/// The input modes (xetex.web: `XeTeX_input_mode_*`); an ICU converter
/// is `ICU | table << 8`. `DECODED`: a text already of characters (a
/// `\scantokens` pseudo file, a line typed at the terminal), not decoded.
pub const AUTO: u32 = 0;
pub const UTF8: u32 = 1;
pub const UTF16BE: u32 = 2;
pub const UTF16LE: u32 = 3;
pub const RAW: u32 = 4;
pub const ICU: u32 = 5;
pub const DECODED: u32 = u32::MAX;

/// `char_class_boundary`, `char_class_ignored`.
pub const BOUNDARY: u16 = 4095;
pub const IGNORED: u16 = 4096;

/// The character of code `c` (a surrogate, which Rust's `char` cannot
/// hold: U+FFFD; CORE.md).
pub fn usv(c: i32) -> char {
    char::from_u32(c as u32).unwrap_or('\u{FFFD}')
}

/// The character at `pos` of a text of bytes (one `char` each, below 256),
/// decoded as encoding `enc` says, and the bytes it took: `get_uni_c`
/// (UTF-8 as the Unicode consortium's `ConvertUTF`: a lone continuation
/// byte is itself; a bad lead byte or continuation is U+FFFD, the bad
/// byte read again), the bytes as they are, or an ICU single-byte
/// converter's table. A line's end (`\r`) is never part of a character.
pub fn decode(t: &[char], pos: usize, enc: u32) -> (char, usize) {
    let raw = t[pos];
    let byte = |i: usize| -> Option<u8> {
        t.get(i).copied().filter(|&c| c != '\r').and_then(|c| u8::try_from(u32::from(c)).ok())
    };
    match enc {
        DECODED => (raw, 1),
        RAW | UTF16BE | UTF16LE => (raw, 1),
        _ if enc & 0xFF == ICU => {
            let conv = (enc >> 8) as usize;
            if conv == usize::from(crate::icu_tables::UTF8) {
                let (c, n, ok) = utf8(&byte, pos);
                // (ICU's converter: a malformed sequence is one substitute)
                return if ok { (c, n) } else { ('\u{FFFD}', n.max(1)) };
            }
            let b = usize::from(byte(pos).unwrap_or(0));
            let v = crate::icu_tables::SBCS.get(conv.wrapping_sub(1)).map_or(b as u32, |tb| u32::from(tb[b]));
            (char::from_u32(v).unwrap_or('\u{FFFD}'), 1)
        }
        _ => {
            let (c, n, _) = utf8(&byte, pos);
            (c, n)
        }
    }
}

/// XeTeX's UTF-8 decoding of the bytes from `pos`.
fn utf8(byte: &dyn Fn(usize) -> Option<u8>, pos: usize) -> (char, usize, bool) {
    const OFFSETS: [u32; 4] = [0, 0x3080, 0x000E_2080, 0x03C8_2080];
    let Some(c0) = byte(pos) else { return ('\u{FFFD}', 1, false) };
    let extra = match c0 {
        0..=0xBF => 0,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        _ => return ('\u{FFFD}', 1, false),
    };
    let mut v = u32::from(c0);
    for k in 1..=extra {
        match byte(pos + k) {
            Some(c) if (0x80..0xC0).contains(&c) => v = (v << 6) + u32::from(c),
            _ => return ('\u{FFFD}', k, false),
        }
    }
    let v = v.wrapping_sub(OFFSETS[extra]);
    match char::from_u32(v) {
        Some(c) => (c, extra + 1, true),
        None => ('\u{FFFD}', extra + 1, false),
    }
}

/// `get_encoding_mode_and_info`: the mode an encoding's name opens
/// (`Err(())`: an ICU converter not made here; an unknown name is read as
/// bytes, `Ok((RAW, true))`, with a diagnostic).
pub fn encoding_mode(name: &str) -> Result<(u32, bool), ()> {
    let builtin = [("auto", AUTO), ("utf8", UTF8), ("utf16", UTF16LE), ("utf16be", UTF16BE), ("utf16le", UTF16LE), ("bytes", RAW)];
    if let Some(&(_, m)) = builtin.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Ok((m, false));
    }
    let key = norm_encoding_name(name.as_bytes());
    let found = crate::icu_tables::NAMES
        .binary_search_by(|(n, _)| n.as_bytes().cmp(&key))
        .ok()
        .map(|k| crate::icu_tables::NAMES[k].1);
    match found {
        Some(crate::icu_tables::OTHER) => Err(()),
        Some(c) => {
            let conv = if c == crate::icu_tables::UTF8 { u32::from(c) } else { u32::from(c) + 1 };
            Ok((ICU | (conv << 8), false))
        }
        None => Ok((RAW, true)),
    }
}

/// An encoding's name as ICU compares names (`ucnv_io_stripASCIIForCompare`).
fn norm_encoding_name(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(name.len());
    let mut after_digit = false;
    for (i, &c) in name.iter().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {
                out.push(c.to_ascii_lowercase());
                after_digit = false;
            }
            b'0' if !after_digit && name.get(i + 1).is_some_and(u8::is_ascii_digit) => {}
            b'0' => out.push(c),
            b'1'..=b'9' => {
                out.push(c);
                after_digit = true;
            }
            _ => after_digit = false,
        }
    }
    out
}

/// What `check_for_inter_char_toks` did.
pub(super) enum Inter {
    None,
    /// The boundary's list went in before the run's first character.
    BigSwitch,
    /// A list went in between two characters.
    Found,
}

/// `node_is_invisible_to_interword_space`.
fn invisible(it: &Item) -> bool {
    matches!(it, Item::Penalty(_) | Item::Ins(_) | Item::Mark(_) | Item::Marks(..) | Item::Adjust(_) | Item::Lang { .. })
        || matches!(it, Item::Whatsit(w) if !matches!(**w, Whatsit::SavePos))
}

/// The length of `text` up to and including its first hyphen (or dash,
/// with `dash`), if it has one.
fn break_after(text: &[u16], hyph: i32, dash: bool) -> Option<usize> {
    text.iter()
        .position(|&u| {
            let u = i32::from(u);
            u == hyph || (dash && (u == 0x2014 || u == 0x2013))
        })
        .map(|i| i + 1)
}

impl Exec<'_> {
    // ------------------------------------------------------------ scanning

    /// An integer in `0..=max`, else `int_error` with XeTeX's help and
    /// `bad` (xetex.web §435–§440's scanners).
    fn scan_ranged(&mut self, max: i32, what: &str, help: &[&str], bad: i32) -> i32 {
        let v = self.scan_int();
        if !(0..=max).contains(&v) {
            self.error(&format!("{what} ({v})"), help);
            return bad;
        }
        v
    }

    /// `scan_usv_num`.
    pub(super) fn scan_usv_num(&mut self) -> i32 {
        self.scan_ranged(
            0x10_FFFF,
            "Bad character code",
            &["A Unicode scalar value must be between 0 and \"10FFFF.", "I changed this one to zero."],
            0,
        )
    }

    /// XeTeX's `scan_char_num` (0–65535).
    pub(super) fn scan_char_num_xetex(&mut self) -> i32 {
        self.scan_ranged(
            65535,
            "Bad character code",
            &["A character number must be between 0 and 65535.", "I changed this one to zero."],
            0,
        )
    }

    /// `scan_char_class` (0–4096) and `scan_char_class_not_ignored`
    /// (0–4095).
    fn scan_char_class(&mut self, ignored_ok: bool) -> i32 {
        if ignored_ok {
            self.scan_ranged(
                4096,
                "Bad character class",
                &["A character class must be between 0 and 4096.", "I changed this one to zero."],
                0,
            )
        } else {
            self.scan_ranged(
                4095,
                "Bad character class",
                &["A class for inter-character transitions must be between 0 and 4095.", "I changed this one to zero."],
                0,
            )
        }
    }

    pub(super) fn scan_math_class_int(&mut self) -> i32 {
        self.scan_ranged(
            7,
            "Bad math class",
            &["Since I expected to read a number between 0 and 7,", "I changed this one to zero."],
            0,
        )
    }

    pub(super) fn scan_math_fam_int(&mut self) -> i32 {
        self.scan_ranged(
            255,
            "Bad math family",
            &["Since I expected to read a number between 0 and 255,", "I changed this one to zero."],
            0,
        )
    }

    /// `scan_xetex_math_char_int`.
    pub(super) fn scan_xetex_math_char_int(&mut self) -> i32 {
        let v = self.scan_int();
        if v & 0x1F_FFFF == 0x1F_FFFF {
            if v != 0x1F_FFFF {
                self.error(
                    &format!("Bad active XeTeX math code ({v})"),
                    &["Since I ignore class and family for active math chars,", "I changed this one to \"1FFFFF."],
                );
                return 0x1F_FFFF;
            }
        } else if (v & 0x1F_FFFF) > 0x10_FFFF {
            self.error(
                &format!("Bad XeTeX math character code ({v})"),
                &["Since I expected a character number between 0 and \"10FFFF,", "I changed this one to zero."],
            );
            return 0;
        }
        v
    }

    /// A code as `\the` gives it in XeTeX (xetex.web's "Fetch a character
    /// code from some table"): a math code back in TeX's 15 bits, a space
    /// factor code without its class, a delimiter code unless extended.
    pub(super) fn xe_code_out(&mut self, c: Code, v: i32) -> i32 {
        match c {
            Code::Math => {
                if v & 0x1F_FFFF == 0x1F_FFFF {
                    return 0x8000;
                }
                let (class, fam, ch) = ((v >> 21) & 7, (v as u32 >> 24) as i32, v & 0x1F_FFFF);
                if class > 7 || fam > 15 || ch > 255 {
                    self.error(
                        &format!("Extended mathchar used as mathchar ({v})"),
                        &["A mathchar number must be between 0 and \"7FFF.", "I changed this one to zero."],
                    );
                    return 0;
                }
                class * 0x1000 + fam * 0x100 + ch
            }
            Code::Sf => v & 0xFFFF,
            Code::Del if v >= 0x4000_0000 => {
                self.error(
                    "Extended delcode used as delcode",
                    &["A delimiter code must be between 0 and \"7FFFFFF.", "I changed this one to zero."],
                );
                0
            }
            _ => v,
        }
    }

    /// A code table assignment in XeTeX (xetex.web's `def_code`): a space
    /// factor code keeps the character's class; a math code is stored in
    /// XeTeX's 32 bits.
    pub(super) fn xe_code_in(&mut self, c: Code, ch: char, v: i32) -> i32 {
        match c {
            Code::Sf => (self.int(&Key::Sfcode(ch)) >> 16 << 16) + v,
            Code::Math if v == 0x8000 => 0x1F_FFFF,
            Code::Math => ((v / 0x1000) << 21) + (((v % 0x1000) / 0x100) << 24) + (v % 0x100),
            _ => v,
        }
    }

    /// A math family's number: TeX's four bits, XeTeX's
    /// `scan_math_fam_int` (0–255).
    pub(super) fn scan_fam_num(&mut self) -> u8 {
        if self.xetex { self.scan_math_fam_int() as u8 } else { self.scan_four_bit_int() as u8 }
    }

    /// `\Umathcharnum n` (1) or `\Umathchar class fam usv` (2).
    pub(super) fn scan_xe_math_char(&mut self, k: u8) -> i32 {
        if k == 1 {
            return self.scan_xetex_math_char_int();
        }
        let class = self.scan_math_class_int();
        let fam = self.scan_math_fam_int();
        let u = self.scan_usv_num();
        ((fam as u32) << 24) as i32 + (class << 21) + u
    }

    // ------------------------------------------------------------ assignments

    /// `XeTeX_def_code` (xetex.web's Assignments): `\XeTeXcharclass` (0),
    /// `\Umathcodenum` (1), `\Umathcode` (2), `\Udelcodenum` (3),
    /// `\Udelcode` (4).
    pub(super) fn xe_def_code(&mut self, k: u8, global: bool) {
        let c = self.scan_usv_num();
        let ch = usv(c);
        match k {
            0 => {
                let n = self.int(&Key::Sfcode(ch)) & 0xFFFF;
                self.optional_equals();
                let cls = self.scan_char_class(true);
                self.define(Key::Sfcode(ch), Val::Int(cls * 0x1_0000 + n), global);
            }
            1 => {
                self.optional_equals();
                let v = self.scan_xetex_math_char_int();
                self.define(Key::Mathcode(ch), Val::Int(v), global);
            }
            2 => {
                self.optional_equals();
                let class = self.scan_math_class_int();
                let fam = self.scan_math_fam_int();
                let u = self.scan_usv_num();
                self.define(Key::Mathcode(ch), Val::Int(((fam as u32) << 24) as i32 + (class << 21) + u), global);
            }
            3 => {
                self.optional_equals();
                let v = self.scan_int();
                self.define(Key::Delcode(ch), Val::Int(v), global);
            }
            _ => {
                self.optional_equals();
                let fam = self.scan_math_fam_int();
                let u = self.scan_usv_num();
                self.define(Key::Delcode(ch), Val::Int(0x4000_0000 + fam * 0x20_0000 + u), global);
            }
        }
    }

    /// `\Umathchardef` (`full`: class, family, character) and
    /// `\Umathcharnumdef` (one number).
    pub(super) fn xe_math_char_def(&mut self, full: bool, global: bool) {
        let Some(key) = self.r_token() else { return };
        self.define(key.clone(), Val::Meaning(Meaning::Prim(Prim::Relax)), global);
        self.optional_equals();
        let v = if full {
            let class = self.scan_math_class_int();
            let fam = self.scan_math_fam_int();
            let u = self.scan_usv_num();
            ((fam as u32) << 24) as i32 + (class << 21) + u
        } else {
            self.scan_xetex_math_char_int()
        };
        self.define(key, Val::Meaning(Meaning::XMathCharGiven(v as u32)), global);
    }

    /// `\XeTeXinterchartoks a b = {…}`.
    pub(super) fn xe_interchar_assign(&mut self, global: bool) {
        let a = self.scan_char_class(false) as u16;
        let b = self.scan_char_class(false) as u16;
        self.toks_assign(Key::InterChar(a, b), false, global);
    }

    // ------------------------------------------------------------ internal quantities

    /// XeTeX's internal quantities (`last_item` codes, the code tables'
    /// numbers, `\XeTeXinterchartoks`).
    pub(super) fn xe_internal(&mut self, t: &Tok, x: Xe, level: Lv) -> Option<IVal> {
        let v = match x {
            Xe::InterCharToks => {
                let a = self.scan_char_class(false) as u16;
                let b = self.scan_char_class(false) as u16;
                if level < Lv::Ident {
                    self.err("Missing number, treated as zero".into());
                    self.back(t.clone());
                    return Some(IVal::Dimen(0));
                }
                match self.get(&Key::InterChar(a, b)) {
                    Val::Toks(l) => IVal::Toks(l),
                    _ => IVal::Toks(Arc::new(Vec::new())),
                }
            }
            Xe::DefCode(k) => {
                let c = self.scan_usv_num();
                let ch = usv(c);
                IVal::Int(match k {
                    0 => self.int(&Key::Sfcode(ch)) >> 16,
                    1 => self.int(&Key::Mathcode(ch)),
                    3 => self.int(&Key::Delcode(ch)),
                    2 => {
                        self.error(
                            "Can't use \\Umathcode as a number (try \\Umathcodenum)",
                            &[
                                "\\Umathcode is for setting a mathcode from separate values;",
                                "use \\Umathcodenum to access them as single values.",
                            ],
                        );
                        0
                    }
                    _ => {
                        self.error(
                            "Can't use \\Udelcode as a number (try \\Udelcodenum)",
                            &[
                                "\\Udelcode is for setting a delcode from separate values;",
                                "use \\Udelcodenum to access them as single values.",
                            ],
                        );
                        0
                    }
                })
            }
            Xe::Item(k) => self.xe_item(k),
            _ => return None,
        };
        Some(v)
    }

    /// The escape character, then `n`.
    fn esc_name(&mut self, n: &str) -> String {
        let mut s = String::new();
        self.print_esc(&mut s, n);
        s
    }

    /// "Cannot use \X with font; not ..." (`not_aat_font_error` and kin).
    fn font_kind_error(&mut self, what: &str, f: &Font, not: &str) {
        // (`print_cmd_chr`, then `font_name[f]`; no help)
        let esc = self.esc_name(what);
        self.error(&format!("Cannot use {esc} with {}{not}", f.name), &[]);
    }

    /// XeTeX's `last_item` codes (xetex.web §1450–1457).
    fn xe_item(&mut self, k: u8) -> IVal {
        use partex_otf::xetex::what;
        const NOT_NATIVE: &str = "; not a native platform font";
        const NOT_OT: &str = "; not an OpenType Layout font";
        const NOT_AAT_GR: &str = "; not an AAT or Graphite font";
        let name = |k: u8| Prim::Xe(Xe::Item(k)).name();
        IVal::Int(match k {
            0 => 0,
            1 | 16 => {
                let f = self.scan_font_ident();
                native::native(&f).map_or(0, |nf| nf.xf.ot_font_get(i32::from(k)))
            }
            2 | 3 | 5 | 6 | 7 | 8 => {
                let _ = self.scan_font_ident();
                0
            }
            9..=15 => {
                let f = self.scan_font_ident();
                self.font_kind_error(name(k), &f, NOT_AAT_GR);
                -1
            }
            4 => {
                let f = self.scan_font_ident();
                self.font_kind_error(name(k), &f, "; not an AAT font");
                -1
            }
            17..=21 => {
                let f = self.scan_font_ident();
                match native::native(&f).cloned() {
                    None => {
                        self.font_kind_error(name(k), &f, NOT_OT);
                        -1
                    }
                    Some(nf) => {
                        let a = self.scan_int();
                        let w = i32::from(k);
                        match w {
                            what::OT_COUNT_LANGUAGES | what::OT_SCRIPT_CODE => nf.xf.ot_font_get1(w, a),
                            what::OT_COUNT_FEATURES | what::OT_LANGUAGE_CODE => {
                                let b = self.scan_int();
                                nf.xf.ot_font_get2(w, a, b)
                            }
                            _ => {
                                let b = self.scan_int();
                                let c = self.scan_int();
                                nf.xf.ot_font_get3(w, a, b, c)
                            }
                        }
                    }
                }
            }
            22 | 23 => {
                let f = self.font();
                match native::native(&f).cloned() {
                    None => {
                        self.font_kind_error(name(k), &f, NOT_NATIVE);
                        0
                    }
                    Some(nf) if k == 22 => {
                        let c = self.scan_int();
                        nf.xf.map_char_to_glyph(c)
                    }
                    Some(nf) => {
                        let n = self.scan_file_name();
                        nf.xf.map_glyph_to_index(n.as_bytes())
                    }
                }
            }
            24 => {
                let f = self.scan_font_ident();
                if native::native(&f).is_some() { 2 } else { 0 }
            }
            25 | 26 => {
                let f = self.scan_font_ident();
                match native::native(&f) {
                    Some(nf) => nf.xf.font_char_range(k == 25),
                    None if k == 25 => f.bc,
                    None => f.ec,
                }
            }
            27 => {
                let n = self.scan_file_name();
                match self.get(&Key::File(Arc::from(n.as_str()))) {
                    Val::Bytes(b) => crate::xpic::pdf_page_count(&b),
                    _ => 0,
                }
            }
            28 => {
                let f = self.font();
                let v = match native::native(&f).cloned() {
                    None => {
                        self.font_kind_error(name(k), &f, NOT_NATIVE);
                        0
                    }
                    Some(nf) => {
                        let edge = self.scan_int();
                        if (1..=4).contains(&edge) {
                            let g = self.scan_int();
                            nf.xf.glyph_bounds_edge(edge, g.max(0) as u32)
                        } else {
                            // (the message's two backslashes are XeTeX's)
                            self.error(
                                &format!(
                                    "\\\\XeTeXglyphbounds requires an edge index from 1 to 4;{}I don't know anything about edge {edge}",
                                    crate::show::NL
                                ),
                                &[],
                            );
                            0
                        }
                    }
                };
                return IVal::Dimen(v);
            }
            // (`\shellescape`: TeX Live's restricted `\write18`)
            40 => 2,
            _ => 0,
        })
    }

    /// e-TeX's `\fontcharwd` and kin of a native font (`getnativecharwd`
    /// …): `None` if `f` is not one. `m`: 0 wd, 1 ht, 2 dp, 3 ic.
    pub(super) fn native_char_dimen(&mut self, m: u8, f: &Font, c: i32) -> Option<i32> {
        let nf = native::native(f)?;
        let k = self.font_key(f);
        let p = |x: &mut Self, n: u32| x.int(&Key::FontDimen(k.clone(), n));
        Some(match m {
            0 => nf.xf.char_wd(c),
            1 | 2 => {
                let (q, xh, ch) = (p(self, 6), p(self, 5), p(self, 8));
                let (h, d) = nf.xf.char_height_depth(c, q, xh, ch);
                if m == 1 { h } else { d }
            }
            _ => nf.xf.char_ic(c, nf.xf.letter_space),
        })
    }

    // ------------------------------------------------------------ convert

    /// XeTeX's `convert` codes: `\XeTeXrevision`, the AAT/Graphite names
    /// (errors here), `\XeTeXglyphname`, `\Uchar`, `\Ucharcat`.
    pub(super) fn xe_convert(&mut self, x: Xe) {
        let mut toks: Vec<Tok> = Vec::new();
        let other = |c: char| if c == ' ' { Tok::Char(10, ' ') } else { Tok::Char(12, c) };
        match x {
            Xe::Revision => toks.extend(XETEX_REVISION.chars().map(other)),
            Xe::VariationName | Xe::FeatureName | Xe::SelectorName => {
                let f = self.scan_font_ident();
                let not = if x == Xe::VariationName { "; not an AAT font" } else { "; not an AAT or Graphite font" };
                self.font_kind_error(Prim::Xe(x).name(), &f, not);
            }
            Xe::GlyphName => {
                let f = self.scan_font_ident();
                match native::native(&f).cloned() {
                    Some(nf) => {
                        let g = self.scan_int();
                        for b in nf.xf.glyph_name(g.max(0) as u32) {
                            toks.push(other(char::from(b)));
                        }
                    }
                    None => self.font_kind_error("XeTeXglyphname", &f, "; not a native platform font"),
                }
            }
            Xe::Uchar => {
                let c = self.scan_usv_num();
                toks.push(other(usv(c)));
            }
            _ => {
                let c = self.scan_usv_num();
                let mut cat = self.scan_int();
                if !(1..=13).contains(&cat) || cat == 5 || cat == 9 {
                    self.error(
                        &format!("Invalid code ({cat}), should be in the ranges 1..4, 6..8, 10..13"),
                        &["I'm going to use 12 instead of that illegal code value."],
                    );
                    cat = 12;
                }
                let ch = usv(c);
                toks.push(if cat == 13 { Tok::Active(ch) } else { Tok::Char(cat as u8, ch) });
            }
        }
        self.rewrite(toks);
    }

    // ------------------------------------------------------------ commands

    /// XeTeX's commands that are neither expanded nor assignments.
    pub(super) fn xe_command(&mut self, x: Xe) {
        match x {
            Xe::PicFile | Xe::PdfFile => {
                if self.st.mode == Mode::M {
                    let m = self.mode_name();
                    self.err(format!("You can't use `\\{}' in {m} mode", Prim::Xe(x).name()));
                } else {
                    self.load_picture(x == Xe::PdfFile);
                }
            }
            Xe::Glyph => self.implement_glyph(),
            Xe::LinebreakLocale => {
                let n = self.scan_file_name();
                // (XeTeX keeps the name only: no area, no extension)
                let base = n.rsplit('/').next().unwrap_or("");
                self.define(Key::LinebreakLocale, Val::Name(Arc::from(base)), true);
            }
            Xe::InputEncoding | Xe::DefaultEncoding => {
                let n = self.scan_file_name();
                let mode = match encoding_mode(&n) {
                    Ok((m, unknown)) => {
                        if unknown {
                            self.log_raw(&format!(
                                "{}{}Unknown encoding `{n}'; reading as raw bytes",
                                crate::show::RAW,
                                crate::show::NL
                            ));
                        }
                        m
                    }
                    Err(()) => {
                        self.err(format!("\\{} \"{n}\": this ICU converter is not supported yet", Prim::Xe(x).name()));
                        return;
                    }
                };
                if x == Xe::DefaultEncoding {
                    self.define(Key::DefaultEncoding, Val::Int(mode as i32), true);
                } else if mode == AUTO {
                    self.error(
                        "Encoding mode `auto' is not valid for \\XeTeXinputencoding",
                        &[
                            "You can't use `auto' encoding here, only for \\XeTeXdefaultencoding.",
                            "I'll ignore this and leave the current encoding unchanged.",
                        ],
                    );
                } else {
                    self.define(Key::InputEncoding, Val::Int(mode as i32), true);
                }
            }
            Xe::SavePos => self.emit_here(Item::Whatsit(Arc::new(Whatsit::SavePos))),
            _ => self.err(format!("\\{} is not supported yet", Prim::Xe(x).name())),
        }
    }

    /// xetex.web §1444: `\XeTeXglyph n`, a glyph of the current font (a
    /// paragraph begins in vertical mode).
    fn implement_glyph(&mut self) {
        match self.st.mode {
            Mode::V => {
                let t = self.cmd.clone();
                self.back_new_graf(&t);
            }
            Mode::M => {
                let m = self.mode_name();
                self.err(format!("You can't use `\\XeTeXglyph' in {m} mode"));
            }
            Mode::H => {
                let f = self.font();
                if native::native(&f).is_none() {
                    self.font_kind_error("XeTeXglyph", &f, "; not a native platform font");
                    return;
                }
                let mut g = self.scan_int();
                if !(0..=65535).contains(&g) {
                    self.error(
                        &format!("Bad glyph number ({g})"),
                        &["A glyph number must be between 0 and 65535.", "I changed this one to zero."],
                    );
                    g = 0;
                }
                let gm = self.int_par(IntPar::XeTeXuseglyphmetrics) > 0;
                self.emit.push(Item::Glyph(Arc::new(native::glyph(&f, g as u16, gm))));
            }
        }
    }

    /// `load_picture` (xetex.web §1446, `XeTeX_pic.c`): a picture whose
    /// size is a pure function of the file's bytes (a read of
    /// `Key::File`) and the keywords.
    fn load_picture(&mut self, pdf: bool) {
        let name = self.scan_file_name();
        let mut pdf_box = 0u8;
        let mut page = 0;
        if pdf {
            if self.keyword("page") {
                page = self.scan_int();
            }
            pdf_box = crate::xpic::PDFBOX_NONE;
            for (k, b) in [
                ("crop", crate::xpic::PDFBOX_CROP),
                ("media", crate::xpic::PDFBOX_MEDIA),
                ("bleed", crate::xpic::PDFBOX_BLEED),
                ("trim", crate::xpic::PDFBOX_TRIM),
                ("art", crate::xpic::PDFBOX_ART),
            ] {
                if self.keyword(k) {
                    pdf_box = b;
                    break;
                }
            }
        }
        let mut kws = Vec::new();
        loop {
            if self.keyword("scaled") {
                kws.push(crate::xpic::Kw::Scaled(self.scan_int()));
            } else if self.keyword("xscaled") {
                kws.push(crate::xpic::Kw::XScaled(self.scan_int()));
            } else if self.keyword("yscaled") {
                kws.push(crate::xpic::Kw::YScaled(self.scan_int()));
            } else if self.keyword("width") {
                let d = self.scan_dimen();
                if d <= 0 {
                    self.improper_image_size(d);
                } else {
                    kws.push(crate::xpic::Kw::Width(d));
                }
            } else if self.keyword("height") {
                let d = self.scan_dimen();
                if d <= 0 {
                    self.improper_image_size(d);
                } else {
                    kws.push(crate::xpic::Kw::Height(d));
                }
            } else if self.keyword("rotated") {
                kws.push(crate::xpic::Kw::Rotated(self.scan_decimal()));
            } else {
                break;
            }
        }
        let found = match self.get(&Key::File(Arc::from(name.as_str()))) {
            Val::Bytes(b) => Some(b),
            _ => None,
        };
        let pic = found.as_ref().and_then(|b| crate::xpic::picture(b, pdf_box, page, &kws));
        let Some((t, w, h)) = pic else {
            let why = if found.is_none() { "the file was not found." } else { "it was not a recognized image format." };
            self.error(
                &format!("Unable to load picture or PDF file '{}'", quoted(&name)),
                &["The requested image couldn't be read because", why],
            );
            return;
        };
        let path = self.env.file_path(&name);
        let path = path.strip_prefix("./").unwrap_or(&path).to_string();
        let p = native::Pic {
            pdf,
            path: Arc::from(path.as_bytes()),
            page: i32::from(page as i16),
            pdf_box,
            t,
            w,
            h,
            d: 0,
        };
        self.emit_here(Item::Pic(Arc::new(p)));
    }

    /// "Improper image size (…pt) will be ignored".
    fn improper_image_size(&mut self, d: i32) {
        let mut v = String::new();
        crate::print::scaled(&mut v, d);
        self.error(
            &format!("Improper image size ({v}pt) will be ignored"),
            &["I can't scale images to zero or negative sizes,", "so I'm ignoring this."],
        );
    }

    // ------------------------------------------------------------ native fonts

    /// xetex.web §1257's `read_font_info` for a native font: the font a
    /// quoted name (or an unquoted one no TFM file has) names at size `s`
    /// (negative: scaled), loaded through the build (a pure function of
    /// the name, the size and the index of the installed fonts), with
    /// what loading prints.
    pub(super) fn load_native(&mut self, name: &str, s: i32) -> Option<Arc<Font>> {
        let tracing = self.int_par(IntPar::XeTeXtracingfonts);
        let (f, diags) = self.env.native_font(name, s, tracing);
        for d in diags {
            let line = match d {
                partex_otf::xetex::Diagnostic::UnknownFeature { feature, font } => {
                    format!("Unknown feature `{feature}' in font `{font}'.")
                }
                partex_otf::xetex::Diagnostic::MappingLoaded { mapping, font } => {
                    format!("Loaded mapping `{mapping}' for font `{font}'.")
                }
                partex_otf::xetex::Diagnostic::MappingNotFound { mapping, font } => {
                    format!("Font mapping `{mapping}' for font `{font}' not found.")
                }
                partex_otf::xetex::Diagnostic::MappingNotUsable { mapping, font } => {
                    format!("Font mapping `{mapping}' for font `{font}' not usable;\nbad mapping file or incorrect mapping type.")
                }
                partex_otf::xetex::Diagnostic::FontPath(p) => format!(" -> {p}"),
            };
            self.log_raw(&format!("{}{}{line}", crate::show::RAW, crate::show::NL));
        }
        f
    }

    /// The default direction of the next word in native font `f`: its last
    /// word's script (`Key::NativeDir`), as the node setting words reads it.
    fn native_rtl(&mut self, f: &Font) -> bool {
        let k = self.font_key(f);
        match self.get(&Key::NativeDir(k)) {
            Val::Int(s) => native::script_rtl(s as u32),
            _ => false,
        }
    }

    /// `set_native_metrics` of a word the main loop makes: the word, its
    /// font's direction read and defined anew (global: XeTeX's shaping
    /// state is never local to a group).
    pub(super) fn native_word(&mut self, f: &Arc<Font>, text: &[u16]) -> NativeWord {
        let at = self.int_par(IntPar::XeTeXgenerateactualtext) > 0;
        self.native_word_at(f, text, at)
    }

    fn native_word_at(&mut self, f: &Arc<Font>, text: &[u16], at: bool) -> NativeWord {
        let gm = self.int_par(IntPar::XeTeXuseglyphmetrics) > 0;
        let rtl = self.native_rtl(f);
        let (w, script) = native::word(f, Arc::from(text), at, gm, rtl);
        let k = self.font_key(f);
        let old = match self.get(&Key::NativeDir(k.clone())) {
            Val::Int(s) => s as u32,
            _ => 0,
        };
        if script != old {
            self.define(Key::NativeDir(k), Val::Int(script as i32), true);
        }
        w
    }

    /// xetex.web §745 `new_native_character(f, c)`: the character in the
    /// font (through its mapping), warning of characters it lacks.
    pub(super) fn native_character(&mut self, f: &Arc<Font>, c: u32) -> NativeWord {
        let text = native::mapped(f, c);
        let lost = self.int_par(IntPar::Tracinglostchars) > 0;
        if lost {
            for ch in native::chars_of(&text) {
                if !native::has_char(f, ch) {
                    self.char_warning(f, ch as i32);
                }
            }
        }
        self.native_word_at(f, &text, false)
    }

    // ------------------------------------------------------------ interchar tokens

    /// `check_for_inter_char_toks` for character `c` (category `cat`),
    /// read again after a list (`backed`).
    pub(super) fn check_inter_char(&mut self, c: u32, cat: u8, backed: bool) -> Inter {
        let enabled = self.int_par(IntPar::XeTeXinterchartokenstate) > 0;
        if !enabled {
            // (`space_class` is read only where the lists are on)
            return Inter::None;
        }
        let sf = self.int(&Key::Sfcode(usv(c as i32)));
        let space = (sf >> 16) as u16;
        let mut prev = self.st.xclass.0;
        self.st.xclass.1 = space;
        if enabled && space != IGNORED {
            if prev == BOUNDARY {
                if !backed && let Some(t) = self.inter_char_toks(BOUNDARY, space) {
                    self.back_char_then(c, cat, t);
                    return Inter::BigSwitch;
                }
            } else if let Some(t) = self.inter_char_toks(prev, space) {
                self.back_char_then(c, cat, t);
                self.st.xclass.0 = BOUNDARY;
                return Inter::Found;
            }
            prev = space;
        }
        self.st.xclass.0 = prev;
        Inter::None
    }

    /// The list between classes `a` and `b`, if one is set.
    fn inter_char_toks(&mut self, a: u16, b: u16) -> Option<Vec<Tok>> {
        match self.get(&Key::InterChar(a, b)) {
            Val::Toks(l) if !l.is_empty() => Some(l.to_vec()),
            _ => None,
        }
    }

    /// Character `c` read again after `toks` (`backed_up_char`: marked
    /// by `\0xbackedchar`, so the class check is not made again).
    fn back_char_then(&mut self, c: u32, cat: u8, toks: Vec<Tok>) {
        let cat = if cat == 11 { 11 } else { 12 };
        let mark = self.cs("\u{0}xbackedchar");
        self.rewrite(vec![mark, Tok::Char(cat, usv(c as i32))]);
        self.rewrite(toks);
    }

    /// `check_for_post_char_toks` before command `t` in horizontal mode:
    /// the list from the last character's class to the boundary, `t` read
    /// again after it. Whether one went in.
    pub(super) fn check_post_char(&mut self, t: &Tok) -> bool {
        let (prev, space) = self.st.xclass;
        if space == IGNORED || prev == BOUNDARY || self.int_par(IntPar::XeTeXinterchartokenstate) <= 0 {
            return false;
        }
        self.st.xclass.0 = BOUNDARY;
        let Some(toks) = self.inter_char_toks(space, BOUNDARY) else { return false };
        self.back(t.clone());
        self.rewrite(toks);
        true
    }

    // ------------------------------------------------------------ the main loop

    /// The current horizontal list's last item and whether the item before
    /// it (walking from the list's head, a discretionary's replacement
    /// skipped) is one a native word may join (not a character, not a
    /// discretionary, not the head): xetex.web's `main_pp`, `main_ppp`.
    /// The one read of material (CORE.md, Conflict 13).
    fn native_tail(&mut self) -> Option<(Arc<NativeWord>, bool)> {
        let own = self.emit.len() >= 2 && !self.emit.iter().any(|i| matches!(i, Item::Remove(_)));
        let list = if own {
            self.emit.clone()
        } else {
            if self.kind == Kind::Other {
                self.kind = Kind::Par;
            }
            self.current_list(false)
        };
        let Some(Item::Native(w)) = list.last() else { return None };
        let n = list.len();
        if n < 2 {
            return Some((w.clone(), false));
        }
        // (the walk from the head: a discretionary's replaced items are
        // passed over)
        let mut i = 0usize;
        let mut prev: Option<usize> = None;
        while i < n - 1 {
            prev = Some(i);
            if let Item::Disc(d) = &list[i] {
                let r = usize::from(d.replace);
                if i + r >= n - 1 {
                    // (the tail is replaced text: `main_ppp` reaches it)
                    return Some((w.clone(), false));
                }
                i += r;
            }
            i += 1;
        }
        let ok = prev.is_some_and(|p| !matches!(list[p], Item::Char(..) | Item::Disc(_)));
        Some((w.clone(), ok))
    }

    /// The previous native word goes from the current list (merged into
    /// the one now made).
    fn drop_native_tail(&mut self) {
        if matches!(self.emit.last(), Some(Item::Native(_))) {
            self.emit.pop();
        } else {
            self.emit.push(Item::Remove(4));
        }
    }

    /// xetex.web §1034's native branch: the characters of native font `f`
    /// from `c0` on, gathered, mapped, set as words (with discretionaries
    /// after hyphens, ICU's breaks with `\XeTeXlinebreaklocale`), joined
    /// to the word before when nothing separates them.
    pub(super) fn main_loop_native(&mut self, f: Arc<Font>, c0: u32, cat0: u8, backed: bool) {
        self.st.xclass.0 = BOUNDARY;
        if !self.st.inner {
            self.fix_language();
        }
        let k = self.font_key(&f);
        let hyph = self.int(&Key::HyphenChar(k));
        let dash = self.int_par(IntPar::XeTeXdashbreakstate) > 0;
        let mut text: Vec<u16> = Vec::new();
        let mut main_h = 0usize;
        let mut is_hyph = false;
        let (mut c, mut cat, mut back) = (c0, cat0, backed);
        let mut big_switch = false;
        loop {
            self.adjust_space_factor(c as i32);
            match self.check_inter_char(c, cat, back) {
                Inter::BigSwitch => return,
                Inter::Found => {
                    big_switch = true;
                    break;
                }
                Inter::None => {}
            }
            back = false;
            text.extend(native::utf16_of(c));
            is_hyph = c as i32 == hyph || (dash && matches!(c, 0x2013 | 0x2014));
            if main_h == 0 && is_hyph {
                main_h = text.len();
            }
            match self.next_x_m(false) {
                Some((_, Meaning::Char(ct @ (11 | 12), ch))) => {
                    c = u32::from(ch);
                    cat = ct;
                }
                Some((_, Meaning::CharGiven(g))) => {
                    c = g;
                    cat = 12;
                }
                Some((_, Meaning::Prim(Prim::Char))) => {
                    c = self.scan_usv_num() as u32;
                    cat = 12;
                }
                Some((t, _)) => {
                    big_switch = self.check_post_char(&t);
                    if !big_switch {
                        self.back(t);
                    }
                    break;
                }
                None => break,
            }
        }
        let _ = big_switch;
        // (collected:)
        if let Some(nf) = native::native(&f)
            && nf.xf.mapping.is_some()
        {
            text = nf.xf.apply_mapping(&text);
            main_h = break_after(&text, hyph, dash).unwrap_or(0);
        }
        if self.int_par(IntPar::Tracinglostchars) > 0 {
            for ch in native::chars_of(&text) {
                if !native::has_char(&f, ch) {
                    self.char_warning(&f, ch as i32);
                }
            }
        }
        if !self.st.inner {
            let mut main_k = text.len();
            let mut temp = 0usize;
            let mut first = true;
            loop {
                if main_h == 0 {
                    main_h = main_k;
                }
                let merge = if first {
                    match self.native_tail() {
                        Some((w, true)) if *w.font == *f => Some(w),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(old) = merge {
                    self.drop_native_tail();
                    let mut s: Vec<u16> = old.text.to_vec();
                    s.extend_from_slice(&text[temp..temp + main_h]);
                    self.locale_linebreaks(&f, &s);
                    main_k = text.len() - main_h - temp;
                    temp = main_h;
                } else {
                    let s = text[temp..temp + main_h].to_vec();
                    self.locale_linebreaks(&f, &s);
                    temp += main_h;
                    main_k -= main_h;
                }
                first = false;
                let rest = &text[temp..temp + main_k];
                main_h = break_after(rest, hyph, dash).unwrap_or(rest.len());
                if main_k > 0 || is_hyph {
                    self.emit.push(Item::Disc(Arc::new(Disc { pre: Vec::new(), post: Vec::new(), replace: 0 })));
                }
                if main_k == 0 {
                    break;
                }
            }
        } else {
            let w = match self.native_tail() {
                Some((old, true)) if *old.font == *f => {
                    self.drop_native_tail();
                    let mut s: Vec<u16> = old.text.to_vec();
                    s.extend_from_slice(&text);
                    self.native_word(&f, &s)
                }
                _ => self.native_word(&f, &text),
            };
            self.emit.push(Item::Native(Arc::new(w)));
        }
        if self.int_par(IntPar::XeTeXinterwordspaceshaping) > 0 {
            self.interword_space_shaping(&f);
        }
    }

    /// `do_locale_linebreaks`: `text` appended as a native word; with
    /// `\XeTeXlinebreaklocale` set, as the words between ICU's line breaks
    /// for the locale, each break a `\XeTeXlinebreakpenalty` (if not zero,
    /// or there is no skip) and `\XeTeXlinebreakskip` (unless TeX's zero
    /// glue).
    fn locale_linebreaks(&mut self, f: &Arc<Font>, text: &[u16]) {
        let locale = match self.get(&Key::LinebreakLocale) {
            Val::Name(n) => n,
            _ => Arc::from(""),
        };
        if locale.is_empty() || text.len() == 1 {
            let w = self.native_word(f, text);
            self.emit.push(Item::Native(Arc::new(w)));
            return;
        }
        let skip = self.glue_par(GluePar::XeTeXlinebreakskip);
        let use_skip = !crate::item::is_zero(&skip);
        let penalty = self.int_par(IntPar::XeTeXlinebreakpenalty);
        let use_penalty = penalty != 0 || !use_skip;
        let Some(set) = crate::icu_linebreak::rule_set(locale.as_bytes()) else {
            self.err("\\XeTeXlinebreaklocale: ICU's phrase breaking (lw=phrase) is not supported yet".into());
            let w = self.native_word(f, text);
            self.emit.push(Item::Native(Arc::new(w)));
            return;
        };
        let rules = crate::icu_linebreak::Rules::new(set);
        let Some(breaks) = rules.breaks(text) else {
            self.err("\\XeTeXlinebreaklocale: ICU's dictionary breaking (Thai, Lao, Khmer, Myanmar) is not supported yet".into());
            let w = self.native_word(f, text);
            self.emit.push(Item::Native(Arc::new(w)));
            return;
        };
        let mut prev = 0usize;
        for offs in breaks {
            if prev != 0 {
                if use_penalty {
                    self.emit.push(Item::Penalty(penalty));
                }
                if use_skip {
                    self.emit.push(crate::item::param_glue(&skip, GluePar::XeTeXlinebreakskip as u8));
                }
            }
            let w = self.native_word(f, &text[prev..offs]);
            self.emit.push(Item::Native(Arc::new(w)));
            prev = offs;
        }
    }

    /// `\XeTeXinterwordspaceshaping`: the space between the word just
    /// appended and a word before it in the same font, measured in
    /// context; where it differs from the font's, a kern (subtype 3,
    /// `space_adjustment`) after the glue makes up the difference.
    fn interword_space_shaping(&mut self, f: &Arc<Font>) {
        let own = !self.emit.iter().any(|i| matches!(i, Item::Remove(_)));
        let list = if own && self.emit.iter().filter(|i| matches!(i, Item::Native(_))).count() >= 2 {
            self.emit.clone()
        } else {
            if self.kind == Kind::Other {
                self.kind = Kind::Par;
            }
            self.current_list(false)
        };
        let n = list.len();
        let Some(tail) = n.checked_sub(1) else { return };
        let Some(Item::Native(last)) = list.get(tail).cloned() else { return };
        let Some(pp) = (0..tail).rev().find(|&i| matches!(list[i], Item::Native(_))) else { return };
        let Item::Native(word) = list[pp].clone() else { return };
        if *word.font != **f {
            return;
        }
        let mut p = pp + 1;
        while list.get(p).is_some_and(invisible) {
            p += 1;
        }
        if !matches!(list.get(p), Some(Item::Glue { .. })) {
            return;
        }
        let mut q = p + 1;
        while list.get(q).is_some_and(invisible) {
            q += 1;
        }
        if q != tail {
            return;
        }
        let mut s = word.text.to_vec();
        s.push(u16::from(b' '));
        s.extend_from_slice(&last.text);
        let both = self.native_word(f, &s);
        let t = both.w - word.w - last.w;
        let space = native::native(f).map_or(0, |nf| nf.glue.width);
        if t != space {
            // (the kern goes after the glue: here, the items after the
            // glue are taken off and put back after it)
            let after = list.len() - (p + 1);
            let tail_items: Vec<Item> = list[p + 1..].to_vec();
            if own && self.emit.len() >= after {
                self.emit.truncate(self.emit.len() - after);
            } else {
                // (an earlier node's items: removed one by one is not
                // possible; the kern goes before the word instead)
                self.emit.pop();
                self.emit.push(Item::Kern { w: t - space, sub: 3 });
                self.emit.push(Item::Native(last));
                return;
            }
            self.emit.push(Item::Kern { w: t - space, sub: 3 });
            self.emit.extend(tail_items);
        }
    }
}

/// A file name as `print_file_name` prints it (quoted if it has a
/// space).
fn quoted(n: &str) -> String {
    if n.contains(' ') { format!("\"{n}\"") } else { n.to_string() }
}
