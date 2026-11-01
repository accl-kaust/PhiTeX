//! XeTeX in the interpreter (docs/spec-xetex.md): the engine chosen per
//! build ([`crate::Options::xetex`]), its primitive table (XeTeX's names
//! for pdfTeX's primitives it keeps, the `\U…` and `\XeTeX…` ones), its
//! character codes up to `0x10FFFF` (§2), its input decoding (UTF-8,
//! `\XeTeXinputencoding`) and its printing (UTF-8 past `0x9F`).
//!
//! Every branch here sits behind the engine's choice: a pdfTeX build
//! never reaches one (spec §8, "keeping pdfTeX's output and speed").

use std::sync::Arc;

use phitex_defuse::ValueId;

use crate::run::{R, Run};
use crate::scan::IVal;
use crate::val::{Tok, cmd};

/// A build's XeTeX ([`crate::Options::xetex`]): where its native fonts
/// come from.
#[derive(Clone, Default)]
pub struct XeTeX {
    /// The font files: kpathsea's lookups (`[file]` names, `TECkit`
    /// mappings) and the bytes of a path (a face the index names).
    pub fonts: Option<FontFiles>,
    /// The fonts a name (`"Latin Modern Roman/B"`) is looked up in
    /// (fontconfig's list as the oracle sees it, `phitex_otf::index`).
    pub font_index: Option<Arc<phitex_otf::index::FontIndex>>,
    /// The PDF written from the pages, as `xelatex` makes it (`crate::xdvpdf`);
    /// none: the XDV file (`xelatex -no-pdf`).
    pub pdf: Option<crate::xdvpdf::XdvPdf>,
    /// The native fonts loaded through these font files and this index
    /// ([`crate::native::NativeFonts`]): made with them, shared by the
    /// clones; a host that makes new font files makes a new one
    /// (`..XeTeX::default()`).
    pub native: Arc<crate::native::NativeFonts>,
}

/// [`XeTeX::fonts`].
pub type FontFiles = Arc<dyn phitex_otf::FontSource + Send + Sync>;

/// `biggest_usv` (xetex.web §12).
pub(crate) const BIGGEST_USV: u32 = 0x10_FFFF;

/// `char_class_boundary` and `char_class_ignored` (xetex.web).
/// `\XeTeXinterchartoks`'s code (`XeTeX_inter_char_loc`).
pub(crate) const INTER_CHAR_LOC: i32 = XI;

pub(crate) const CLASS_BOUNDARY: i32 = CHAR_CLASS_LIMIT - 1;
pub(crate) const CLASS_IGNORED: i32 = CHAR_CLASS_LIMIT;

/// `char_class_limit` (xetex.web): `\XeTeXcharclass` is 0–4096, kept in
/// a character's `\sfcode` from bit 16 up.
pub(crate) const CHAR_CLASS_LIMIT: i32 = 0x1000;

/// `last_item` modifiers of XeTeX's integer queries.
pub(crate) mod li {
    pub const VERSION: i32 = 300;
    pub const COUNT_GLYPHS: i32 = 301;
    pub const COUNT_VARIATIONS: i32 = 302;
    pub const VARIATION: i32 = 303;
    pub const FIND_VARIATION_BY_NAME: i32 = 304;
    pub const VARIATION_MIN: i32 = 305;
    pub const VARIATION_MAX: i32 = 306;
    pub const VARIATION_DEFAULT: i32 = 307;
    pub const COUNT_FEATURES: i32 = 308;
    pub const FEATURE_CODE: i32 = 309;
    pub const FIND_FEATURE_BY_NAME: i32 = 310;
    pub const IS_EXCLUSIVE_FEATURE: i32 = 311;
    pub const COUNT_SELECTORS: i32 = 312;
    pub const SELECTOR_CODE: i32 = 313;
    pub const FIND_SELECTOR_BY_NAME: i32 = 314;
    pub const IS_DEFAULT_SELECTOR: i32 = 315;
    pub const OT_COUNT_SCRIPTS: i32 = 316;
    pub const OT_COUNT_LANGUAGES: i32 = 317;
    pub const OT_COUNT_FEATURES: i32 = 318;
    pub const OT_SCRIPT: i32 = 319;
    pub const OT_LANGUAGE: i32 = 320;
    pub const OT_FEATURE: i32 = 321;
    pub const MAP_CHAR_TO_GLYPH: i32 = 322;
    pub const GLYPH_INDEX: i32 = 323;
    pub const GLYPH_BOUNDS: i32 = 324;
    pub const FONT_TYPE: i32 = 325;
    pub const FIRST_CHAR: i32 = 326;
    pub const LAST_CHAR: i32 = 327;
    pub const PDF_PAGE_COUNT: i32 = 328;
}

/// `convert` modifiers of XeTeX's.
pub(crate) mod cv {
    pub const UCHAR: i32 = 300;
    pub const UCHARCAT: i32 = 301;
    pub const REVISION: i32 = 302;
    pub const VARIATION_NAME: i32 = 303;
    pub const FEATURE_NAME: i32 = 304;
    pub const SELECTOR_NAME: i32 = 305;
    pub const GLYPH_NAME: i32 = 306;
}

/// `extension` modifiers of XeTeX's.
pub(crate) mod ex {
    pub const PIC_FILE: i32 = 300;
    pub const PDF_FILE: i32 = 301;
    pub const GLYPH: i32 = 302;
    pub const LINEBREAK_LOCALE: i32 = 303;
    pub const INPUT_ENCODING: i32 = 304;
    pub const DEFAULT_ENCODING: i32 = 305;
}

/// The integer parameters of XeTeX's (`param:<name>`), past pdfTeX's.
const XI: i32 = 1300;

/// `XeTeX_def_code` (xetex.web §1232's XeTeX part) is `def_code` with
/// its modifier past this: `\XeTeXcharclass` (`sf`), `\Umathcodenum`
/// (`mc`), `\Umathcode` (`mc`+1), `\Udelcodenum` (`dc`), `\Udelcode`
/// (`dc`+1).
pub(crate) const XDC: i32 = 0x10_0000;

/// `\Umathchardef` and `\Umathcharnumdef` (`shorthand_def`).
pub(crate) const UMATHCHAR_DEF: i32 = 20;
pub(crate) const UMATHCHARNUM_DEF: i32 = 21;

/// The primitives XeTeX adds (xetex.web's `primitive` calls), with the
/// codes they have here. The `XeTeX…` aliases of the `U…` names come
/// first: the `U…` name is the one printed.
pub(crate) const PRIMS: &[(&str, u8, i32)] = &[
    ("XeTeXlinebreakskip", cmd::ASSIGN_GLUE, XI),
    ("XeTeXlinebreakpenalty", cmd::ASSIGN_INT, XI),
    ("XeTeXprotrudechars", cmd::ASSIGN_INT, XI + 1),
    ("suppressfontnotfounderror", cmd::ASSIGN_INT, XI + 2),
    ("XeTeXupwardsmode", cmd::ASSIGN_INT, XI + 3),
    ("XeTeXuseglyphmetrics", cmd::ASSIGN_INT, XI + 4),
    ("XeTeXinterchartokenstate", cmd::ASSIGN_INT, XI + 5),
    ("XeTeXdashbreakstate", cmd::ASSIGN_INT, XI + 6),
    ("XeTeXinputnormalization", cmd::ASSIGN_INT, XI + 7),
    ("XeTeXtracingfonts", cmd::ASSIGN_INT, XI + 8),
    ("XeTeXinterwordspaceshaping", cmd::ASSIGN_INT, XI + 9),
    ("XeTeXgenerateactualtext", cmd::ASSIGN_INT, XI + 10),
    ("XeTeXhyphenatablelength", cmd::ASSIGN_INT, XI + 11),
    ("XeTeXinterchartoks", cmd::ASSIGN_TOKS, XI),
    ("XeTeXdelimiter", cmd::DELIM_NUM, 1),
    ("Udelimiter", cmd::DELIM_NUM, 1),
    ("XeTeXmathaccent", cmd::MATH_ACCENT, 1),
    ("Umathaccent", cmd::MATH_ACCENT, 1),
    ("XeTeXmathcharnum", cmd::MATH_CHAR_NUM, 1),
    ("Umathcharnum", cmd::MATH_CHAR_NUM, 1),
    ("XeTeXmathchar", cmd::MATH_CHAR_NUM, 2),
    ("Umathchar", cmd::MATH_CHAR_NUM, 2),
    ("XeTeXradical", cmd::RADICAL, 1),
    ("Uradical", cmd::RADICAL, 1),
    ("Uchar", cmd::CONVERT, cv::UCHAR),
    ("Ucharcat", cmd::CONVERT, cv::UCHARCAT),
    ("XeTeXmathcharnumdef", cmd::SHORTHAND_DEF, UMATHCHARNUM_DEF),
    ("Umathcharnumdef", cmd::SHORTHAND_DEF, UMATHCHARNUM_DEF),
    ("XeTeXmathchardef", cmd::SHORTHAND_DEF, UMATHCHAR_DEF),
    ("Umathchardef", cmd::SHORTHAND_DEF, UMATHCHAR_DEF),
    (
        "XeTeXmathcodenum",
        cmd::DEF_CODE,
        XDC + crate::val::MATH_CODE_BASE,
    ),
    (
        "Umathcodenum",
        cmd::DEF_CODE,
        XDC + crate::val::MATH_CODE_BASE,
    ),
    (
        "XeTeXmathcode",
        cmd::DEF_CODE,
        XDC + crate::val::MATH_CODE_BASE + 1,
    ),
    (
        "Umathcode",
        cmd::DEF_CODE,
        XDC + crate::val::MATH_CODE_BASE + 1,
    ),
    (
        "XeTeXcharclass",
        cmd::DEF_CODE,
        XDC + crate::val::SF_CODE_BASE,
    ),
    (
        "XeTeXdelcodenum",
        cmd::DEF_CODE,
        XDC + crate::val::DEL_CODE_BASE,
    ),
    (
        "Udelcodenum",
        cmd::DEF_CODE,
        XDC + crate::val::DEL_CODE_BASE,
    ),
    (
        "XeTeXdelcode",
        cmd::DEF_CODE,
        XDC + crate::val::DEL_CODE_BASE + 1,
    ),
    (
        "Udelcode",
        cmd::DEF_CODE,
        XDC + crate::val::DEL_CODE_BASE + 1,
    ),
    ("XeTeXpicfile", cmd::EXTENSION, ex::PIC_FILE),
    ("XeTeXpdffile", cmd::EXTENSION, ex::PDF_FILE),
    ("XeTeXglyph", cmd::EXTENSION, ex::GLYPH),
    ("XeTeXlinebreaklocale", cmd::EXTENSION, ex::LINEBREAK_LOCALE),
    ("XeTeXinputencoding", cmd::EXTENSION, ex::INPUT_ENCODING),
    ("XeTeXdefaultencoding", cmd::EXTENSION, ex::DEFAULT_ENCODING),
    ("XeTeXversion", cmd::LAST_ITEM, li::VERSION),
    ("XeTeXrevision", cmd::CONVERT, cv::REVISION),
    ("XeTeXcountglyphs", cmd::LAST_ITEM, li::COUNT_GLYPHS),
    ("XeTeXcountvariations", cmd::LAST_ITEM, li::COUNT_VARIATIONS),
    ("XeTeXvariation", cmd::LAST_ITEM, li::VARIATION),
    (
        "XeTeXfindvariationbyname",
        cmd::LAST_ITEM,
        li::FIND_VARIATION_BY_NAME,
    ),
    ("XeTeXvariationmin", cmd::LAST_ITEM, li::VARIATION_MIN),
    ("XeTeXvariationmax", cmd::LAST_ITEM, li::VARIATION_MAX),
    (
        "XeTeXvariationdefault",
        cmd::LAST_ITEM,
        li::VARIATION_DEFAULT,
    ),
    ("XeTeXcountfeatures", cmd::LAST_ITEM, li::COUNT_FEATURES),
    ("XeTeXfeaturecode", cmd::LAST_ITEM, li::FEATURE_CODE),
    (
        "XeTeXfindfeaturebyname",
        cmd::LAST_ITEM,
        li::FIND_FEATURE_BY_NAME,
    ),
    (
        "XeTeXisexclusivefeature",
        cmd::LAST_ITEM,
        li::IS_EXCLUSIVE_FEATURE,
    ),
    ("XeTeXcountselectors", cmd::LAST_ITEM, li::COUNT_SELECTORS),
    ("XeTeXselectorcode", cmd::LAST_ITEM, li::SELECTOR_CODE),
    (
        "XeTeXfindselectorbyname",
        cmd::LAST_ITEM,
        li::FIND_SELECTOR_BY_NAME,
    ),
    (
        "XeTeXisdefaultselector",
        cmd::LAST_ITEM,
        li::IS_DEFAULT_SELECTOR,
    ),
    ("XeTeXvariationname", cmd::CONVERT, cv::VARIATION_NAME),
    ("XeTeXfeaturename", cmd::CONVERT, cv::FEATURE_NAME),
    ("XeTeXselectorname", cmd::CONVERT, cv::SELECTOR_NAME),
    ("XeTeXOTcountscripts", cmd::LAST_ITEM, li::OT_COUNT_SCRIPTS),
    (
        "XeTeXOTcountlanguages",
        cmd::LAST_ITEM,
        li::OT_COUNT_LANGUAGES,
    ),
    (
        "XeTeXOTcountfeatures",
        cmd::LAST_ITEM,
        li::OT_COUNT_FEATURES,
    ),
    ("XeTeXOTscripttag", cmd::LAST_ITEM, li::OT_SCRIPT),
    ("XeTeXOTlanguagetag", cmd::LAST_ITEM, li::OT_LANGUAGE),
    ("XeTeXOTfeaturetag", cmd::LAST_ITEM, li::OT_FEATURE),
    ("XeTeXcharglyph", cmd::LAST_ITEM, li::MAP_CHAR_TO_GLYPH),
    ("XeTeXglyphindex", cmd::LAST_ITEM, li::GLYPH_INDEX),
    ("XeTeXglyphbounds", cmd::LAST_ITEM, li::GLYPH_BOUNDS),
    ("XeTeXglyphname", cmd::CONVERT, cv::GLYPH_NAME),
    ("XeTeXfonttype", cmd::LAST_ITEM, li::FONT_TYPE),
    ("XeTeXfirstfontchar", cmd::LAST_ITEM, li::FIRST_CHAR),
    ("XeTeXlastfontchar", cmd::LAST_ITEM, li::LAST_CHAR),
    ("XeTeXpdfpagecount", cmd::LAST_ITEM, li::PDF_PAGE_COUNT),
];

/// pdfTeX's primitives XeTeX has under other names (xetex.web: the
/// `pdf` dropped): XeTeX's name, pdfTeX's.
pub(crate) const RENAMED: &[(&str, &str)] = &[
    ("creationdate", "pdfcreationdate"),
    ("elapsedtime", "pdfelapsedtime"),
    ("filedump", "pdffiledump"),
    ("filemoddate", "pdffilemoddate"),
    ("filesize", "pdffilesize"),
    ("ifprimitive", "ifpdfprimitive"),
    ("mdfivesum", "pdfmdfivesum"),
    ("normaldeviate", "pdfnormaldeviate"),
    ("primitive", "pdfprimitive"),
    ("randomseed", "pdfrandomseed"),
    ("resettimer", "pdfresettimer"),
    ("setrandomseed", "pdfsetrandomseed"),
    ("shellescape", "pdfshellescape"),
    ("strcmp", "pdfstrcmp"),
    ("uniformdeviate", "pdfuniformdeviate"),
];

/// pdfTeX's primitives XeTeX has by the same name.
pub(crate) const KEPT: &[&str] = &[
    "expanded",
    "leftmarginkern",
    "lpcode",
    "pdflastxpos",
    "pdflastypos",
    "pdfpageheight",
    "pdfpagewidth",
    "pdfsavepos",
    "rightmarginkern",
    "rpcode",
];

/// XeTeX's own entries of the primitive table: the renamed ones with
/// pdfTeX's codes, then [`PRIMS`].
pub(crate) fn own_prims() -> impl Iterator<Item = (&'static str, u8, i32)> {
    RENAMED
        .iter()
        .filter_map(|&(x, p)| {
            crate::pdfprim::PRIMS
                .iter()
                .find(|q| q.0 == p)
                .map(|q| (x, q.1, q.2))
        })
        .chain(PRIMS.iter().copied())
}

/// Every primitive a XeTeX format starts with (INITEX in extended mode):
/// `TeX82`'s, e-TeX's (with web2c's), the pdfTeX ones XeTeX keeps, and
/// its own.
pub(crate) fn initex_prims() -> impl Iterator<Item = (&'static str, u8, i32)> {
    crate::prims::PRIMS
        .iter()
        .copied()
        .chain(crate::etex::PRIMS.iter().copied())
        .chain(
            crate::pdfprim::PRIMS
                .iter()
                .copied()
                .filter(|p| KEPT.contains(&p.0)),
        )
        .chain(own_prims())
}

// -------------------------------------------------------------- reading

/// The input modes of a file (`XeTeX_ext.h`): how its bytes become
/// characters.
pub(crate) mod mode {
    pub const AUTO: i64 = 0;
    pub const UTF8: i64 = 1;
    pub const UTF16BE: i64 = 2;
    pub const UTF16LE: i64 = 3;
    pub const RAW: i64 = 4;
}

/// `getencodingmodeandinfo`: the mode a name stands for (ICU's
/// converters are not built in: any other name reads as bytes, as
/// XeTeX reads a name ICU does not know; TeX Live 2026's xetex fails
/// opening any ICU converter at all).
pub(crate) fn mode_of_name(name: &str) -> i64 {
    match name.to_ascii_lowercase().as_str() {
        "auto" => mode::AUTO,
        "utf8" => mode::UTF8,
        "utf16" | "utf16le" => mode::UTF16LE,
        "utf16be" => mode::UTF16BE,
        _ => mode::RAW,
    }
}

/// A line's characters as XeTeX's `input_line` reads them from its
/// bytes (each a character 0–255 of `buf`) in `mode`: UTF-8 as
/// `get_uni_c` decodes it (an ill-formed sequence is U+FFFD, the byte
/// that broke it read again; a lone continuation byte is its own code),
/// or the bytes themselves. `first`: the file's first line, where a
/// UTF-8 byte-order mark is skipped (the mode was `auto`). True when a
/// sequence was ill-formed ("Invalid UTF-8 byte or sequence").
pub(crate) fn decode_line(buf: &mut Vec<u32>, m: i64, first: bool) -> bool {
    const OFFSETS: [u32; 4] = [0, 0x3080, 0xE_2080, 0x3C8_2080];
    if m == mode::RAW || buf.iter().all(|&c| c < 0x80) {
        return false;
    }
    let src = std::mem::take(buf);
    let mut i = 0;
    if first && m == mode::AUTO && src.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
    }
    let mut bad = false;
    let mut out = Vec::with_capacity(src.len());
    while i < src.len() {
        let b = src[i];
        i += 1;
        let extra = match b {
            0xC0..=0xDF => 1,
            0xE0..=0xEF => 2,
            0xF0..=0xF7 => 3,
            0xF8..=0xFF => {
                bad = true;
                out.push(0xFFFD);
                continue;
            }
            _ => 0,
        };
        let mut r = b;
        let mut ok = true;
        for _ in 0..extra {
            match src.get(i) {
                Some(&c) if (0x80..0xC0).contains(&c) => {
                    r = (r << 6) + c;
                    i += 1;
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            bad = true;
            out.push(0xFFFD);
            continue;
        }
        let r = r.wrapping_sub(OFFSETS[extra]);
        if r > BIGGEST_USV {
            bad = true;
            out.push(0xFFFD);
        } else {
            out.push(r);
        }
    }
    *buf = out;
    bad
}

// ------------------------------------------------------------- printing

/// The bytes of a character as XeTeX's `print_char` writes it to a file
/// or the terminal (selector `log_only`..`write_file`): controls in `^^`
/// form, printable ASCII as itself, C1 controls as `^^xx`, the rest
/// UTF-8. Each byte is one character of `out` (a file's text holds its
/// bytes as characters 0–255).
pub(crate) fn print_file_char(out: &mut String, c: u32) {
    let hex = |d: u32| char::from_digit(d, 16).unwrap_or('0');
    if c < 32 {
        out.push('^');
        out.push('^');
        out.push(char::from_u32(c + 64).unwrap_or('?'));
    } else if c < 127 {
        out.push(char::from_u32(c).unwrap_or('?'));
    } else if c == 127 {
        out.push_str("^^?");
    } else if c < 0xA0 {
        out.push('^');
        out.push('^');
        out.push(hex((c % 0x100) / 0x10));
        out.push(hex(c % 0x10));
    } else {
        utf8_bytes(out, c);
    }
}

/// `c` as its UTF-8 bytes, each one character 0–255 (`print_raw_char`'s
/// bytes).
pub(crate) fn utf8_bytes(out: &mut String, c: u32) {
    let b = |x: u32| char::from_u32(x & 0xFF).unwrap_or('?');
    if c < 0x80 {
        out.push(b(c));
    } else if c < 0x800 {
        out.push(b(0xC0 + c / 0x40));
        out.push(b(0x80 + c % 0x40));
    } else if c < 0x1_0000 {
        out.push(b(0xE0 + c / 0x1000));
        out.push(b(0x80 + (c % 0x1000) / 0x40));
        out.push(b(0x80 + c % 0x40));
    } else {
        out.push(b(0xF0 + c / 0x4_0000));
        out.push(b(0x80 + (c % 0x4_0000) / 0x1000));
        out.push(b(0x80 + (c % 0x1000) / 0x40));
        out.push(b(0x80 + c % 0x40));
    }
}

/// A written line (`\write` to a file: characters as TeX printed them,
/// the new-line character already a line end) as the file's bytes.
pub(crate) fn file_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\n' {
            out.push('\n');
        } else {
            print_file_char(&mut out, u32::from(c));
        }
    }
    out
}

/// A written file's text (its bytes as characters) as XeTeX reads it
/// back: each line's UTF-8 decoded ([`decode_line`]).
pub(crate) fn reread(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (k, l) in text.split('\n').enumerate() {
        if k > 0 {
            out.push('\n');
        }
        let mut b: Vec<u32> = l.chars().map(u32::from).collect();
        decode_line(&mut b, mode::UTF8, false);
        out.extend(
            b.into_iter()
                .map(|c| char::from_u32(c).unwrap_or('\u{FFFD}')),
        );
    }
    out
}

/// A `\special`'s bytes (`special_out`, `doing_special`: every character
/// past 127 in UTF-8, nothing in `^^` form).
pub(crate) fn special_bytes(s: &str) -> Vec<u8> {
    let mut t = String::new();
    for c in s.chars() {
        utf8_bytes(&mut t, u32::from(c));
    }
    t.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
        .collect()
}

// ------------------------------------------------------- the primitives

/// XeTeX's `convert` commands (§470, xetex.web's cases).
pub(crate) fn convert(r: &mut Run<'_, '_>, c: i32) -> R<()> {
    match c {
        cv::UCHAR => {
            // `\Uchar`: a character token of category 12 (10 for a
            // space)
            let n = r.scan_usv_num()?;
            let ch = u32::try_from(n).unwrap_or(0);
            let cat = if ch == 32 { 10 } else { 12 };
            r.push_toks([Tok::ch(cat, ch)]);
        }
        cv::UCHARCAT => {
            let n = r.scan_usv_num()?;
            let ch = u32::try_from(n).unwrap_or(0);
            let cat = r.scan_int()?;
            // (`illegal_Ucharcat_catcode`: 1–4, 6–8, 10–13 only; 13 an
            // active character)
            let ok = matches!(cat, 1..=4 | 6..=8 | 10..=13);
            if !ok {
                r.log("error", "Invalid code")?;
            }
            let cat = u8::try_from(if ok { cat } else { 12 }).unwrap_or(12);
            r.push_toks([Tok::ch(cat, ch)]);
        }
        cv::REVISION => r.push_toks(crate::expand::str_toks(".999998")),
        cv::GLYPH_NAME => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)? {
                let g = r.scan_int()?;
                let name = nf.font.glyph_name(g.cast_unsigned());
                let s: String = name.iter().map(|&b| char::from(b)).collect();
                r.push_toks(crate::expand::str_toks(&s));
            } else {
                font_error(r, cmd::CONVERT, c, f, NOT_NATIVE)?;
            }
        }
        cv::VARIATION_NAME => {
            let f = r.scan_font_ident()?;
            font_error(r, cmd::CONVERT, c, f, NOT_AAT)?;
        }
        cv::FEATURE_NAME | cv::SELECTOR_NAME => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| is_gr(nf)) {
                let a1 = r.scan_int()?;
                let a2 = if c == cv::SELECTOR_NAME {
                    r.scan_int()?
                } else {
                    0
                };
                let what = if c == cv::FEATURE_NAME {
                    phitex_otf::xetex::what::FEATURE_NAME
                } else {
                    phitex_otf::xetex::what::SELECTOR_NAME
                };
                let s = nf.font.gr_font_name(what, a1, a2).unwrap_or_default();
                r.push_toks(crate::expand::str_toks(&s));
            } else {
                font_error(r, cmd::CONVERT, c, f, NOT_AAT_GR)?;
            }
        }
        _ => {
            r.log("error", "this XeTeX primitive is not supported")?;
        }
    }
    Ok(())
}

/// The name `\print_cmd_chr` gives XeTeX's primitive `(c, code)`.
fn prim_name(c: u8, code: i32) -> &'static str {
    PRIMS
        .iter()
        .rev()
        .find(|p| p.1 == c && p.2 == code)
        .map_or("", |p| p.0)
}

/// Whether a native font shapes with Graphite (`is_gr_font`; else it is
/// an OpenType one, `is_ot_font`).
fn is_gr(nf: &crate::native::NativeFont) -> bool {
    nf.font.req_engine == b'G' && nf.font.face.graphite().is_some()
}

/// `not_native_font_error` and its kin: "Cannot use \x with
/// <font>; not ...".
fn font_error(r: &mut Run<'_, '_>, c: u8, code: i32, f: i64, what: &str) -> R<()> {
    let info = r.font_get(f, crate::fields::FontCell::Info)?;
    let name = crate::fonts::info(r.v, info)
        .map(|i| i.name)
        .unwrap_or_default();
    let p = prim_name(c, code);
    r.log("error", &format!("Cannot use \\{p} with {name}; {what}"))
}

const NOT_NATIVE: &str = "not a native platform font";
const NOT_OT: &str = "not an OpenType Layout font";
const NOT_AAT_GR: &str = "not an AAT or Graphite font";
const NOT_AAT: &str = "not an AAT font";

/// XeTeX's `last_item` queries (xetex.web "Cases for fetching an
/// integer value", `\XeTeXglyphbounds` a dimension).
#[allow(clippy::too_many_lines)]
pub(crate) fn last_item(r: &mut Run<'_, '_>, m: i32) -> R<IVal> {
    // (`m - XeTeX_int`: the font library's query codes)
    let what = m - li::VERSION;
    let li_cmd = crate::val::cmd::LAST_ITEM;
    let n = match m {
        li::VERSION => 0,
        li::COUNT_GLYPHS => {
            let f = r.scan_font_ident()?;
            r.native_font(f)?.map_or(0, |nf| nf.font.ot_font_get(what))
        }
        li::COUNT_FEATURES => {
            let f = r.scan_font_ident()?;
            r.native_font(f)?
                .filter(|nf| is_gr(nf))
                .map_or(0, |nf| nf.font.ot_font_get(what))
        }
        li::VARIATION
        | li::VARIATION_MIN
        | li::VARIATION_MAX
        | li::VARIATION_DEFAULT
        | li::COUNT_VARIATIONS => {
            r.scan_font_ident()?;
            0
        }
        li::FEATURE_CODE | li::IS_EXCLUSIVE_FEATURE | li::COUNT_SELECTORS => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| is_gr(nf)) {
                let k = r.scan_int()?;
                nf.font.ot_font_get1(what, k)
            } else {
                font_error(r, li_cmd, m, f, NOT_AAT_GR)?;
                -1
            }
        }
        li::SELECTOR_CODE | li::IS_DEFAULT_SELECTOR => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| is_gr(nf)) {
                let k = r.scan_int()?;
                let n = r.scan_int()?;
                nf.font.ot_font_get2(what, k, n)
            } else {
                font_error(r, li_cmd, m, f, NOT_AAT_GR)?;
                -1
            }
        }
        li::FIND_VARIATION_BY_NAME => {
            let f = r.scan_font_ident()?;
            font_error(r, li_cmd, m, f, NOT_AAT)?;
            -1
        }
        li::FIND_FEATURE_BY_NAME => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| is_gr(nf)) {
                let name = r.scan_file_name()?;
                nf.font.gr_font_get_named(what, name.as_bytes())
            } else {
                font_error(r, li_cmd, m, f, NOT_AAT_GR)?;
                -1
            }
        }
        li::FIND_SELECTOR_BY_NAME => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| is_gr(nf)) {
                let k = r.scan_int()?;
                let name = r.scan_file_name()?;
                nf.font.gr_font_get_named1(what, k, name.as_bytes())
            } else {
                font_error(r, li_cmd, m, f, NOT_AAT_GR)?;
                -1
            }
        }
        li::OT_COUNT_SCRIPTS => {
            let f = r.scan_font_ident()?;
            r.native_font(f)?
                .filter(|nf| !is_gr(nf))
                .map_or(0, |nf| nf.font.ot_font_get(what))
        }
        li::OT_COUNT_LANGUAGES | li::OT_SCRIPT => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| !is_gr(nf)) {
                let k = r.scan_int()?;
                nf.font.ot_font_get1(what, k)
            } else {
                font_error(r, li_cmd, m, f, NOT_OT)?;
                -1
            }
        }
        li::OT_COUNT_FEATURES | li::OT_LANGUAGE => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| !is_gr(nf)) {
                let k = r.scan_int()?;
                let n = r.scan_int()?;
                nf.font.ot_font_get2(what, k, n)
            } else {
                font_error(r, li_cmd, m, f, NOT_OT)?;
                -1
            }
        }
        li::OT_FEATURE => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)?.filter(|nf| !is_gr(nf)) {
                let k = r.scan_int()?;
                let kk = r.scan_int()?;
                let n = r.scan_int()?;
                nf.font.ot_font_get3(what, k, kk, n)
            } else {
                font_error(r, li_cmd, m, f, NOT_OT)?;
                -1
            }
        }
        li::MAP_CHAR_TO_GLYPH => {
            let f = r.cint(crate::fields::cell::FONT)?;
            if let Some(nf) = r.native_font(f)? {
                let n = r.scan_int()?;
                nf.font.map_char_to_glyph(n)
            } else {
                font_error(r, li_cmd, m, f, NOT_NATIVE)?;
                0
            }
        }
        li::GLYPH_INDEX => {
            let f = r.cint(crate::fields::cell::FONT)?;
            if let Some(nf) = r.native_font(f)? {
                let name = r.scan_file_name()?;
                nf.font.map_glyph_to_index(name.as_bytes())
            } else {
                font_error(r, li_cmd, m, f, NOT_NATIVE)?;
                0
            }
        }
        li::GLYPH_BOUNDS => {
            let f = r.cint(crate::fields::cell::FONT)?;
            let Some(nf) = r.native_font(f)? else {
                font_error(r, li_cmd, m, f, NOT_NATIVE)?;
                return Ok(IVal::Dimen(0));
            };
            let edge = r.scan_int()?;
            if !(1..=4).contains(&edge) {
                r.log(
                    "error",
                    &format!(
                        "\\XeTeXglyphbounds requires an edge index from 1 to 4; I don't know anything about edge {edge}"
                    ),
                )?;
                return Ok(IVal::Dimen(0));
            }
            let g = r.scan_int()?;
            return Ok(IVal::Dimen(
                nf.font.glyph_bounds_edge(edge, g.cast_unsigned()),
            ));
        }
        li::PDF_PAGE_COUNT => r.pdf_page_count()?,
        li::FONT_TYPE => {
            let f = r.scan_font_ident()?;
            match r.native_font(f)? {
                Some(nf) if is_gr(&nf) => 3,
                Some(_) => 2,
                None => 0,
            }
        }
        li::FIRST_CHAR | li::LAST_CHAR => {
            let f = r.scan_font_ident()?;
            if let Some(nf) = r.native_font(f)? {
                nf.font.font_char_range(m == li::FIRST_CHAR)
            } else {
                let mut view = crate::fonts::View::default();
                let fu = u32::try_from(f).unwrap_or(0);
                r.add_font(&mut view, fu)?;
                // (the null font: `font_bc` 1, `font_ec` 0)
                view.fonts
                    .get(fu)
                    .map_or(i32::from(m == li::FIRST_CHAR), |(l, _)| {
                        i32::from(if m == li::FIRST_CHAR {
                            l.tfm.bc
                        } else {
                            l.tfm.ec
                        })
                    })
            }
        }
        _ => {
            r.log("error", "this XeTeX primitive is not supported")?;
            0
        }
    };
    Ok(IVal::Int(n))
}

/// `\fontcharwd`, `\fontcharht`, `\fontchardp`, `\fontcharic` of a
/// native font (`getnativecharwd` and the others; `None` for a TFM
/// font).
pub(crate) fn native_char_dim(r: &mut Run<'_, '_>, f: i64, c: i32, m: i32) -> R<Option<i32>> {
    let Some(nf) = r.native_font(f)? else {
        return Ok(None);
    };
    Ok(Some(match m {
        crate::etex::li::FONT_CHAR_WD => nf.font.char_wd(c),
        crate::etex::li::FONT_CHAR_IC => nf.font.char_ic(c, nf.font.letter_space),
        _ => {
            let quad = r.font_dimen(f, 6)?;
            let x_height = r.font_dimen(f, 5)?;
            let cap = r.font_dimen(f, 8)?;
            let (h, d) = nf.font.char_height_depth(c, quad, x_height, cap);
            if m == crate::etex::li::FONT_CHAR_HT {
                h
            } else {
                d
            }
        }
    }))
}

/// `\iffontchar` of a native font: whether the character maps to a
/// glyph (`None` for a TFM font).
pub(crate) fn native_has_char(r: &mut Run<'_, '_>, f: i64, c: i32) -> R<Option<bool>> {
    Ok(r.native_font(f)?.map(|nf| nf.font.map_char_to_glyph(c) > 0))
}

/// `\XeTeXglyph` (xetex.web's "Implement `\XeTeXglyph`").
fn xetex_glyph(r: &mut Run<'_, '_>, md: &crate::build::Mode) -> R<()> {
    match md.m.abs() {
        1 => {
            r.back_cur();
            r.new_graf(md, true)?;
        }
        203 => r.report_illegal_case(md)?,
        _ => {
            let f = r.cint(crate::fields::cell::FONT)?;
            let Some(nf) = r.native_font(f)? else {
                return font_error(r, crate::val::cmd::EXTENSION, ex::GLYPH, f, NOT_NATIVE);
            };
            let mut g = r.scan_int()?;
            if !(0..=0xFFFF).contains(&g) {
                r.log("error", "Bad glyph number")?;
                g = 0;
            }
            let n = r.native_glyph(&nf, f, u16::try_from(g).unwrap_or(0))?;
            let x = crate::nodes::encode(
                r.v,
                &phitex_typeset::node::Node::Native(Box::new(n)),
                &crate::nodes::Side::default(),
            );
            r.append(md.lid, x);
        }
    }
    Ok(())
}

/// XeTeX's extensions (`do_extension`).
pub(crate) fn extension(r: &mut Run<'_, '_>, md: &crate::build::Mode) -> R<()> {
    match r.cur.chr {
        ex::GLYPH => xetex_glyph(r, md)?,
        ex::PIC_FILE => r.implement_picture(md, false)?,
        ex::PDF_FILE => r.implement_picture(md, true)?,
        ex::INPUT_ENCODING => {
            let name = r.scan_file_name()?;
            let m = mode_of_name(&name);
            if m == mode::AUTO {
                r.log(
                    "error",
                    "Encoding mode `auto' is not valid for \\XeTeXinputencoding",
                )?;
                return Ok(());
            }
            r.set_input_mode(m);
        }
        ex::LINEBREAK_LOCALE => {
            // (an assignment of eqtb's own, never restored: the locale
            // stays past the group)
            let name = r.scan_file_name()?;
            let v = r.v;
            r.set(LINEBREAK_LOCALE_FIELD, v.str(&name), true);
        }
        ex::DEFAULT_ENCODING => {
            let name = r.scan_file_name()?;
            let m = mode_of_name(&name);
            let v = r.v;
            r.set("xdefenc", v.int(m), true);
        }
        _ => {
            r.log("error", "this XeTeX primitive is not supported")?;
        }
    }
    Ok(())
}

/// The field of `\XeTeXlinebreaklocale`'s name ("" none).
pub(crate) const LINEBREAK_LOCALE_FIELD: &str = "xlbloc";

/// `XeTeX_linebreak_skip_code`, the parameter of `\XeTeXlinebreakskip`'s
/// glue between a word's parts.
pub(crate) const LINEBREAK_SKIP_CODE: u8 = 15;

/// The line break opportunities ICU's line break iterator gives in
/// `text` (UTF-16) for locale `loc` (`ubrk_open(UBRK_LINE, loc)`): 0,
/// each break, the length. The `lb` keyword (`@lb=loose`, `-u-lb-loose`)
/// is the strictness, ICU's default strict; Japanese and Chinese default
/// to ICU's `line_normal` rules (their `line` entry: small kana and `ー`
/// break as ideographs) and take the `_cj` rules only under a keyword
/// (`line_normal_cj`, `line_loose_cj`: a break before `〜` and `゠`, a
/// `content_locale` of theirs to ICU4X).
pub(crate) fn line_breaks(loc: &str, text: &[u16]) -> Vec<usize> {
    use icu_segmenter::LineSegmenter;
    use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness};
    let lower = loc.to_ascii_lowercase();
    let lang_end = lower.find(['_', '-', '@']).unwrap_or(lower.len());
    let lang = &lower[..lang_end];
    let lb = lower
        .split_once("lb=")
        .map(|x| x.1)
        .or_else(|| lower.split_once("-u-lb-").map(|x| x.1))
        .map(|s| s.split([';', '-', '_', '@']).next().unwrap_or(""));
    let cj = lang == "ja" || lang == "zh";
    let strictness = match lb {
        Some("loose") => LineBreakStrictness::Loose,
        Some("normal") => LineBreakStrictness::Normal,
        Some("strict") => LineBreakStrictness::Strict,
        // (ICU's Japanese and Chinese default to their normal rules)
        _ if cj => LineBreakStrictness::Normal,
        _ => LineBreakStrictness::Strict,
    };
    let ja: icu_locale_core::LanguageIdentifier = icu_locale_core::langid!("ja");
    let zh: icu_locale_core::LanguageIdentifier = icu_locale_core::langid!("zh");
    let mut options = LineBreakOptions::default();
    options.strictness = Some(strictness);
    options.content_locale = match lang {
        _ if lb.is_none() => None,
        "ja" => Some(&ja),
        "zh" => Some(&zh),
        _ => None,
    };
    LineSegmenter::new_dictionary(options)
        .segment_utf16(text)
        .collect()
}

impl Run<'_, '_> {
    /// `scan_usv_num` (xetex.web): a character code, 0 to `0x10FFFF`
    /// ("Bad character code" past them).
    pub(crate) fn scan_usv_num(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        if (0..=BIGGEST_USV.cast_signed()).contains(&n) {
            Ok(n)
        } else {
            self.log("error", "Bad character code")?;
            Ok(0)
        }
    }

    /// The input mode of the file named `name` (its `xenc:` field, set by
    /// `\XeTeXinputencoding` or at its opening from
    /// `\XeTeXdefaultencoding`; `auto` where none was).
    fn input_mode(&mut self, name: ValueId) -> R<i64> {
        if let Some(&(_, m)) = self.xenc.iter().find(|e| e.0 == name) {
            return Ok(m);
        }
        let v = self.v;
        let f = v.field(&format!("xenc:{}", crate::val::str_of(v, name)));
        let m = self
            .read_f(f)?
            .map_or(mode::AUTO, |x| crate::val::int_of(v, x));
        self.xenc.push((name, m));
        Ok(m)
    }

    /// `set_input_file_encoding(input_file[in_open], m)`: the innermost
    /// file's lines from the next on are read in mode `m`.
    pub(crate) fn set_input_mode(&mut self, m: i64) {
        let name = self
            .input
            .levels
            .iter()
            .rev()
            .filter(|l| l.kind == crate::input::kind::LINE)
            .map(|&l| self.input.line(l))
            .find(|line| !line.rescanned)
            // (the document's own text has no origin: `UNIT` names it)
            .map(|line| line.origin.map_or(ValueId::UNIT, |o| o.0));
        let Some(name) = name else { return };
        let v = self.v;
        self.set(
            &format!("xenc:{}", crate::val::str_of(v, name)),
            v.int(m),
            true,
        );
        self.xenc.retain(|e| e.0 != name);
        self.xenc.push((name, m));
    }

    /// XeTeX's `input_line` for the line on top, just reached: its bytes
    /// made characters in its file's mode ([`decode_line`]). A line no
    /// file holds (a token list's, a pseudo file's, `\read`'s, which are
    /// characters already) is left as it is.
    pub(crate) fn xe_decode_top(&mut self) -> R<()> {
        let Some(s) = self.input.top_line() else {
            return Ok(());
        };
        if s.rescanned || s.chars || s.buf.iter().all(|&c| c < 0x80) {
            return Ok(());
        }
        let first = s.starts && s.origin.is_some_and(|o| o.1 == 0);
        let m = self.input_mode(s.origin.map_or(ValueId::UNIT, |o| o.0))?;
        if let Some(s) = self.input.top_line_mut() {
            decode_line(&mut s.buf, m, first);
            s.chars = true;
        }
        Ok(())
    }

    /// [`Run::xe_decode_top`] for a segment joining the reached line on
    /// top (its continuation).
    pub(crate) fn xe_decode_joined(
        &mut self,
        buf: &mut Vec<u32>,
        origin: Option<ValueId>,
    ) -> R<()> {
        if buf.iter().all(|&c| c < 0x80) {
            return Ok(());
        }
        let m = self.input_mode(origin.unwrap_or(ValueId::UNIT))?;
        decode_line(buf, m, false);
        Ok(())
    }
}

/// `active_math_char` (xetex.web): a math code whose character field is
/// all ones is "active", whatever its class and family.
pub(crate) const ACTIVE_MATH_CHAR: i32 = 0x1F_FFFF;

/// A XeTeX math code's class, family and character (`math_class_field`,
/// `math_fam_field`, `math_char_field`, xetex.h).
pub(crate) fn math_fields(x: i32) -> (i32, i32, i32) {
    let u = x.cast_unsigned();
    (
        ((u >> 21) & 7).cast_signed(),
        ((u >> 24) & 0xFF).cast_signed(),
        (u & 0x1F_FFFF).cast_signed(),
    )
}

/// A math code from its class, family and character
/// (`set_class_field`, `set_family_field`).
pub(crate) fn math_code(class: i32, fam: i32, ch: i32) -> i32 {
    (((class.cast_unsigned() & 7) << 21) | ((fam.cast_unsigned() & 0xFF) << 24)).cast_signed() + ch
}

/// A XeTeX math code in `TeX82`'s fifteen bits (`"8000` active): the
/// class, the family's low four bits and the character's low eight.
pub(crate) fn math15(x: i32) -> i32 {
    let (class, fam, ch) = math_fields(x);
    if ch == ACTIVE_MATH_CHAR {
        return 0x8000;
    }
    class * 0x1000 + (fam % 16) * 0x100 + ch % 0x100
}

/// The flag of a `\\Umathchardef`'s value ([`crate::val::Mean::MathCharDef`]):
/// past its code's 32 bits.
pub(crate) const U_MATH: i64 = 1 << 32;

impl Run<'_, '_> {
    /// A XeTeX math code: class, family and character
    /// (`\\Umathchar`, `\\Umathchardef`: `separate`), or one number
    /// (`\\Umathcharnum`, `\\Umathcharnumdef`).
    pub(crate) fn scan_umath(&mut self, separate: bool) -> R<i32> {
        if !separate {
            return self.scan_xetex_math_char_int();
        }
        let class = self.scan_bounded(7, "Bad math class")?;
        let fam = self.scan_bounded(255, "Bad math family")?;
        let ch = self.scan_usv_num()?;
        Ok(math_code(class, fam, ch))
    }

    fn set_code(&mut self, name: &'static str, c: i32, x: i32, global: bool) {
        let l = self.code_loc(name, c);
        self.set_loc(l, crate::state::Entry::int(x), global);
    }

    /// `scan_math_class_int`, `scan_math_fam_int`, `scan_char_class`:
    /// an integer in `0..=max`, else 0 (with the error).
    pub(crate) fn scan_bounded(&mut self, max: i32, what: &str) -> R<i32> {
        let n = self.scan_int()?;
        if (0..=max).contains(&n) {
            Ok(n)
        } else {
            self.log("error", what)?;
            Ok(0)
        }
    }

    /// `scan_xetex_math_char_int`.
    fn scan_xetex_math_char_int(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        let (_, _, ch) = math_fields(n);
        if ch == ACTIVE_MATH_CHAR {
            if n != ACTIVE_MATH_CHAR {
                self.log("error", "Bad active XeTeX math code")?;
                return Ok(ACTIVE_MATH_CHAR);
            }
        } else if ch.cast_unsigned() > BIGGEST_USV {
            self.log("error", "Bad XeTeX math character code")?;
            return Ok(0);
        }
        Ok(n)
    }

    /// XeTeX's `def_code` and `XeTeX_def_code` assignments (xetex.web
    /// §1232).
    pub(crate) fn xe_def_code(&mut self, global: bool) -> R<()> {
        use crate::val::{CAT_CODE_BASE, DEL_CODE_BASE, MATH_CODE_BASE, SF_CODE_BASE};
        let chr = self.cur.chr;
        if chr >= XDC {
            let base = chr - XDC;
            let c = self.scan_usv_num()?;
            match base {
                b if b == SF_CODE_BASE => {
                    let n = self.code_slow(2, c.cast_unsigned())? % 0x1_0000;
                    self.scan_optional_equals()?;
                    let k = self.scan_bounded(CHAR_CLASS_LIMIT, "Bad character class")?;
                    self.set_code("sf", c, k * 0x1_0000 + n, global);
                }
                b if b == MATH_CODE_BASE => {
                    self.scan_optional_equals()?;
                    let n = self.scan_xetex_math_char_int()?;
                    self.set_code("mc", c, n, global);
                }
                b if b == MATH_CODE_BASE + 1 => {
                    self.scan_optional_equals()?;
                    let class = self.scan_bounded(7, "Bad math class")?;
                    let fam = self.scan_bounded(255, "Bad math family")?;
                    let ch = self.scan_usv_num()?;
                    self.set_code("mc", c, math_code(class, fam, ch), global);
                }
                b if b == DEL_CODE_BASE => {
                    self.scan_optional_equals()?;
                    let n = self.scan_int()?;
                    self.set_code("dc", c, n, global);
                }
                _ => {
                    self.scan_optional_equals()?;
                    let fam = self.scan_bounded(255, "Bad math family")?;
                    let ch = self.scan_usv_num()?;
                    self.set_code("dc", c, 0x4000_0000 + fam * 0x20_0000 + ch, global);
                }
            }
            return Ok(());
        }
        let n: i32 = match chr {
            CAT_CODE_BASE => 15,
            MATH_CODE_BASE => 0x8000,
            SF_CODE_BASE => 0x7FFF,
            DEL_CODE_BASE => 0x7FF_FFFF,
            _ => BIGGEST_USV.cast_signed(),
        };
        let c = self.scan_usv_num()?;
        self.scan_optional_equals()?;
        let mut val = self.scan_int()?;
        if (val < 0 && chr < DEL_CODE_BASE) || val > n {
            self.log("error", "Invalid code")?;
            val = 0;
        }
        match chr {
            CAT_CODE_BASE => self.set_code("cat", c, val, global),
            SF_CODE_BASE => {
                let k = self.code_slow(2, c.cast_unsigned())? / 0x1_0000;
                self.set_code("sf", c, k * 0x1_0000 + val, global);
            }
            MATH_CODE_BASE => {
                let x = if val == 0x8000 {
                    ACTIVE_MATH_CHAR
                } else {
                    math_code(val / 0x1000, (val % 0x1000) / 0x100, val % 0x100)
                };
                self.set_code("mc", c, x, global);
            }
            DEL_CODE_BASE => self.set_code("dc", c, val, global),
            crate::val::LC_CODE_BASE => self.set_code("lc", c, val, global),
            _ => self.set_code("uc", c, val, global),
        }
        Ok(())
    }

    /// XeTeX's §414: a code fetched (`def_code`, `XeTeX_def_code`).
    pub(crate) fn xe_fetch_code(&mut self, m: i32) -> R<i32> {
        use crate::val::{
            CAT_CODE_BASE, DEL_CODE_BASE, LC_CODE_BASE, MATH_CODE_BASE, SF_CODE_BASE, UC_CODE_BASE,
        };
        let c = self.scan_usv_num()?;
        if m >= XDC {
            let base = m - XDC;
            return Ok(match base {
                b if b == SF_CODE_BASE => self.code_slow(2, c.cast_unsigned())? / 0x1_0000,
                b if b == MATH_CODE_BASE => self.code(3, c)?,
                b if b == DEL_CODE_BASE => self.code(4, c)?,
                b if b == MATH_CODE_BASE + 1 => {
                    self.log(
                        "error",
                        "Can't use \\Umathcode as a number (try \\Umathcodenum)",
                    )?;
                    0
                }
                _ => {
                    self.log(
                        "error",
                        "Can't use \\Udelcode as a number (try \\Udelcodenum)",
                    )?;
                    0
                }
            });
        }
        Ok(match m {
            CAT_CODE_BASE => i32::from(self.cat(c.cast_unsigned())?),
            LC_CODE_BASE => self.code(0, c)?,
            UC_CODE_BASE => self.code(1, c)?,
            SF_CODE_BASE => self.code_slow(2, c.cast_unsigned())? % 0x1_0000,
            MATH_CODE_BASE => {
                let x = self.code(3, c)?;
                let (class, fam, ch) = math_fields(x);
                if ch == ACTIVE_MATH_CHAR {
                    0x8000
                } else if class > 7 || fam > 15 || ch > 255 {
                    self.log("error", "Extended mathchar used as mathchar")?;
                    0
                } else {
                    class * 0x1000 + fam * 0x100 + ch
                }
            }
            DEL_CODE_BASE => {
                let x = self.code(4, c)?;
                if x >= 0x4000_0000 {
                    self.log("error", "Extended delcode used as delcode")?;
                    0
                } else {
                    x
                }
            }
            _ => 0,
        })
    }
}

impl Run<'_, '_> {
    /// `\XeTeXinterchartoks`'s field for the two classes scanned
    /// (`scan_char_class_not_ignored` twice): `ict:<a*4096+b>`.
    pub(crate) fn ic_field(&mut self) -> R<phitex_defuse::Field> {
        let mut k = [0; 2];
        for x in &mut k {
            let n = self.scan_int()?;
            *x = if (0..=CHAR_CLASS_LIMIT).contains(&n) {
                n
            } else {
                self.log("error", "Bad character class")?;
                0
            };
        }
        Ok(self
            .v
            .field(&format!("ict:{}", k[0] * CHAR_CLASS_LIMIT + k[1])))
    }

    /// The tokens between classes `a` and `b`, where there are any.
    fn ic_toks(&mut self, a: i32, b: i32) -> R<Option<Vec<Tok>>> {
        let x = self.get(&format!("ict:{}", a * CHAR_CLASS_LIMIT + b))?;
        let t = crate::val::toks_of(self.v, x);
        Ok((!t.is_empty()).then_some(t))
    }

    /// `back_input` of character `ch` as `backed_up_char` (a letter's
    /// category kept, anything else other), then the tokens `t` ahead
    /// of it (`inter_char_text`).
    fn ic_insert(&mut self, ch: u32, letter: bool, t: &[Tok]) {
        let c = Tok::ch(if letter { 11 } else { 12 }, ch);
        let r = self.input.arena.alloc(&[c]);
        self.input.push_toks(r, crate::input::ty::BACKED_UP_CHAR);
        let r = self.input.arena.alloc(t);
        self.input.push_toks(r, crate::input::ty::INTER_CHAR);
    }

    /// `check_for_inter_char_toks` (xetex.web) for character `ch`, just
    /// read (`letter`: of category 11): where tokens go between the
    /// class before it and its own, the character is put back after
    /// them and true is returned (`goto` the label: the characters so far
    /// end there).
    pub(crate) fn ic_check(&mut self, ch: u32, letter: bool) -> R<bool> {
        if self.param("XeTeXinterchartokenstate")? <= 0 {
            return Ok(false);
        }
        let class = self.code_slow(2, ch)? / 0x1_0000;
        if class == CLASS_IGNORED {
            return Ok(false);
        }
        self.ic_space = class;
        if self.ic_prev == CLASS_BOUNDARY {
            // (a character put back here is past its boundary already)
            let backed = self.input.top().is_some_and(|l| {
                l.kind == crate::input::kind::TOKS && l.ty == crate::input::ty::BACKED_UP_CHAR
            });
            if !backed && let Some(t) = self.ic_toks(CLASS_BOUNDARY, class)? {
                self.ic_insert(ch, letter, &t);
                return Ok(true);
            }
        } else if let Some(t) = self.ic_toks(self.ic_prev, class)? {
            self.ic_insert(ch, letter, &t);
            self.ic_prev = CLASS_BOUNDARY;
            return Ok(true);
        }
        self.ic_prev = class;
        Ok(false)
    }

    /// `check_for_post_char_toks` (xetex.web): after a run of characters,
    /// at the token `cur` that is not one, the tokens between the last
    /// character's class and the boundary go ahead of `cur` (put back);
    /// true where they did.
    pub(crate) fn ic_post(&mut self) -> R<bool> {
        if self.ic_prev == CLASS_BOUNDARY
            || self.ic_space == CLASS_IGNORED
            || self.param("XeTeXinterchartokenstate")? <= 0
        {
            return Ok(false);
        }
        self.ic_prev = CLASS_BOUNDARY;
        let Some(t) = self.ic_toks(self.ic_space, CLASS_BOUNDARY)? else {
            return Ok(false);
        };
        self.back_cur();
        let r = self.input.arena.alloc(&t);
        self.input.push_toks(r, crate::input::ty::INTER_CHAR);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(bytes: &[u8], m: i64, first: bool) -> (Vec<u32>, bool) {
        let mut b: Vec<u32> = bytes.iter().map(|&x| u32::from(x)).collect();
        let bad = decode_line(&mut b, m, first);
        (b, bad)
    }

    #[test]
    fn utf8_is_decoded_as_get_uni_c_decodes_it() {
        assert_eq!(
            dec("déjà".as_bytes(), mode::AUTO, false),
            (vec![0x64, 0xE9, 0x6A, 0xE0], false)
        );
        assert_eq!(
            dec("𝐀".as_bytes(), mode::UTF8, false),
            (vec![0x1D400], false)
        );
        // (a lead byte cut short: U+FFFD, the next byte read again)
        assert_eq!(
            dec(b"x\xC3y", mode::UTF8, false),
            (vec![0x78, 0xFFFD, 0x79], true)
        );
        // (a lone continuation byte: its own code)
        assert_eq!(dec(b"\xA9", mode::UTF8, false), (vec![0xA9], false));
        // (bytes: each its code)
        assert_eq!(
            dec(b"\xC3\xA9", mode::RAW, false),
            (vec![0xC3, 0xA9], false)
        );
        // (a byte-order mark on the first line, in auto mode)
        assert_eq!(dec(b"\xEF\xBB\xBFa", mode::AUTO, true), (vec![0x61], false));
    }

    #[test]
    fn files_get_utf8_and_hat_forms() {
        let s: String = ['a', '\u{1}', '\u{85}', 'é', '\u{1D400}'].iter().collect();
        let t = file_text(&s);
        let bytes: Vec<u8> = t
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap())
            .collect();
        assert_eq!(bytes, b"a^^A^^85\xC3\xA9\xF0\x9D\x90\x80");
    }
}
