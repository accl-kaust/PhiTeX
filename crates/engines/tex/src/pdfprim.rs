//! pdfTeX's primitives (`pdftex.web`'s `primitive` calls past e-TeX's):
//! the parameters, the expandable commands, the font codes, the whatsits
//! and objects, and what `\shipout` resolves of them.
//!
//! What the backend makes of a whatsit (links, annotations, objects,
//! images, outlines, threads) is `phitex-pdf`'s; this crate makes the
//! node and its data, and resolves at `\shipout` what TeX reads back
//! (`\pdflastxpos`, `\pdflastypos`). Where `phitex-pdf` has no support
//! yet, the data waits behind `PDF-BACKEND-GAP` notes.
//!
//! Allocation-order globals (F10): one field per family, defined by each
//! allocating command and read by the next, `\advance`-shaped
//! (`pdf:objnum`, `pdf:lastobj`, …); the random generator's state is one
//! field (`pdf:random`) read and defined by every draw.

use std::fmt::Write as _;
use std::rc::Rc;

use phitex_defuse::ValueId;
use phitex_typeset::glue::{GlueSign, Order};
use phitex_typeset::node::{BoxKind, BoxNode, GlueNode, Node};

use crate::build::Mode;
use crate::expand::str_toks;
use crate::nodes;
use crate::run::{R, Run};
use crate::scan::IVal;
use crate::val::{Mean, Tok, cmd, int_of, list_of, str_of, toks_of};

/// `last_item` modifiers.
mod li {
    pub const PDFTEX_VERSION: i32 = 140;
    pub const LAST_OBJ: i32 = 141;
    pub const LAST_XFORM: i32 = 142;
    pub const LAST_XIMAGE: i32 = 143;
    pub const LAST_XIMAGE_PAGES: i32 = 144;
    pub const LAST_ANNOT: i32 = 145;
    pub const LAST_X_POS: i32 = 146;
    pub const LAST_Y_POS: i32 = 147;
    pub const RETVAL: i32 = 148;
    pub const LAST_XIMAGE_COLORDEPTH: i32 = 149;
    pub const ELAPSED_TIME: i32 = 150;
    pub const SHELL_ESCAPE: i32 = 151;
    pub const RANDOM_SEED: i32 = 152;
    pub const LAST_LINK: i32 = 153;
}

/// `convert` modifiers.
mod cv {
    pub const EXPANDED: i32 = 101;
    pub const PDFTEX_REVISION: i32 = 102;
    pub const PDFTEX_BANNER: i32 = 103;
    pub const FONT_NAME: i32 = 104;
    pub const FONT_OBJNUM: i32 = 105;
    pub const FONT_SIZE: i32 = 106;
    pub const PAGE_REF: i32 = 107;
    pub const LEFT_MARGIN_KERN: i32 = 108;
    pub const RIGHT_MARGIN_KERN: i32 = 109;
    pub const XFORM_NAME: i32 = 110;
    pub const ESCAPE_STRING: i32 = 111;
    pub const ESCAPE_NAME: i32 = 112;
    pub const ESCAPE_HEX: i32 = 113;
    pub const UNESCAPE_HEX: i32 = 114;
    pub const CREATION_DATE: i32 = 115;
    pub const FILE_MOD_DATE: i32 = 116;
    pub const FILE_SIZE: i32 = 117;
    pub const MDFIVE_SUM: i32 = 118;
    pub const FILE_DUMP: i32 = 119;
    pub const MATCH: i32 = 120;
    pub const LAST_MATCH: i32 = 121;
    pub const STRCMP: i32 = 122;
    pub const COLORSTACK_INIT: i32 = 123;
    pub const UNIFORM_DEVIATE: i32 = 124;
    pub const NORMAL_DEVIATE: i32 = 125;
    pub const INSERT_HT: i32 = 126;
    pub const XIMAGE_BBOX: i32 = 127;
}

/// `extension` modifiers (pdfTeX §1344 ff.).
mod ex {
    pub const LITERAL: i32 = 6;
    pub const COLORSTACK: i32 = 7;
    pub const SETMATRIX: i32 = 8;
    pub const SAVE: i32 = 9;
    pub const RESTORE: i32 = 10;
    pub const OBJ: i32 = 11;
    pub const REFOBJ: i32 = 12;
    pub const XFORM: i32 = 13;
    pub const REFXFORM: i32 = 14;
    pub const XIMAGE: i32 = 15;
    pub const REFXIMAGE: i32 = 16;
    pub const ANNOT: i32 = 17;
    pub const START_LINK: i32 = 18;
    pub const END_LINK: i32 = 19;
    pub const OUTLINE: i32 = 20;
    pub const DEST: i32 = 21;
    pub const THREAD: i32 = 22;
    pub const START_THREAD: i32 = 23;
    pub const END_THREAD: i32 = 24;
    pub const SAVE_POS: i32 = 25;
    pub const SNAP_REF_POINT: i32 = 26;
    pub const SNAPY: i32 = 27;
    pub const SNAPY_COMP: i32 = 28;
    pub const INFO: i32 = 29;
    pub const CATALOG: i32 = 30;
    pub const NAMES: i32 = 31;
    pub const INCLUDE_CHARS: i32 = 32;
    pub const FONT_ATTR: i32 = 33;
    pub const MAP_FILE: i32 = 34;
    pub const MAP_LINE: i32 = 35;
    pub const TRAILER: i32 = 36;
    pub const TRAILER_ID: i32 = 37;
    pub const RESET_TIMER: i32 = 38;
    pub const SET_RANDOM_SEED: i32 = 39;
    pub const FONT_EXPAND: i32 = 40;
    pub const GLYPH_TO_UNICODE: i32 = 41;
    pub const NOBUILTIN_TOUNICODE: i32 = 42;
    pub const INTERWORD_SPACE_ON: i32 = 43;
    pub const INTERWORD_SPACE_OFF: i32 = 44;
    pub const FAKE_SPACE: i32 = 45;
    pub const RUNNING_LINK_OFF: i32 = 46;
    pub const RUNNING_LINK_ON: i32 = 47;
    pub const SPACE_FONT: i32 = 48;
}

/// The font codes of `assign_font_int` (pdfTeX §1253), after
/// `\hyphenchar` and `\skewchar`.
const FONT_CODES: [&str; 12] = [
    "", "", "lp", "rp", "ef", "tag", "knbs", "stbs", "shbs", "knbc", "knac", "nolig",
];
const NO_LIG: i32 = 11;
const TAG: i32 = 5;

const I: i32 = 1100;
const D: i32 = 1100;
const T: i32 = 1100;

/// The pdfTeX primitives (`pdftex.web`, past e-TeX's).
pub(crate) const PRIMS: &[(&str, u8, i32)] = &[
    ("pdfpagesattr", cmd::ASSIGN_TOKS, T),
    ("pdfpageattr", cmd::ASSIGN_TOKS, T + 1),
    ("pdfpageresources", cmd::ASSIGN_TOKS, T + 2),
    ("pdfpkmode", cmd::ASSIGN_TOKS, T + 3),
    ("pdfoutput", cmd::ASSIGN_INT, I),
    ("pdfcompresslevel", cmd::ASSIGN_INT, I + 1),
    ("pdfobjcompresslevel", cmd::ASSIGN_INT, I + 2),
    ("pdfdecimaldigits", cmd::ASSIGN_INT, I + 3),
    ("pdfmovechars", cmd::ASSIGN_INT, I + 4),
    ("pdfimageresolution", cmd::ASSIGN_INT, I + 5),
    ("pdfpkresolution", cmd::ASSIGN_INT, I + 6),
    ("pdfuniqueresname", cmd::ASSIGN_INT, I + 7),
    // the same parameters as \pdfminorversion, \pdfforcepagebox and
    // \pdfinclusionerrorlevel under their old names
    ("pdfoptionpdfminorversion", cmd::ASSIGN_INT, I + 8),
    ("pdfoptionalwaysusepdfpagebox", cmd::ASSIGN_INT, I + 9),
    ("pdfoptionpdfinclusionerrorlevel", cmd::ASSIGN_INT, I + 10),
    ("pdfmajorversion", cmd::ASSIGN_INT, I + 11),
    ("pdfminorversion", cmd::ASSIGN_INT, I + 8),
    ("pdfforcepagebox", cmd::ASSIGN_INT, I + 9),
    ("pdfpagebox", cmd::ASSIGN_INT, I + 12),
    ("pdfinclusionerrorlevel", cmd::ASSIGN_INT, I + 10),
    ("pdfgamma", cmd::ASSIGN_INT, I + 13),
    ("pdfimagegamma", cmd::ASSIGN_INT, I + 14),
    ("pdfimagehicolor", cmd::ASSIGN_INT, I + 15),
    ("pdfimageapplygamma", cmd::ASSIGN_INT, I + 16),
    ("pdfadjustspacing", cmd::ASSIGN_INT, I + 17),
    ("pdfprotrudechars", cmd::ASSIGN_INT, I + 18),
    ("pdftracingfonts", cmd::ASSIGN_INT, I + 19),
    ("pdfadjustinterwordglue", cmd::ASSIGN_INT, I + 20),
    ("pdfprependkern", cmd::ASSIGN_INT, I + 21),
    ("pdfappendkern", cmd::ASSIGN_INT, I + 22),
    ("pdfgentounicode", cmd::ASSIGN_INT, I + 23),
    ("pdfdraftmode", cmd::ASSIGN_INT, I + 24),
    ("pdfinclusioncopyfonts", cmd::ASSIGN_INT, I + 25),
    ("pdfsuppresswarningdupdest", cmd::ASSIGN_INT, I + 26),
    ("pdfsuppresswarningdupmap", cmd::ASSIGN_INT, I + 27),
    ("pdfsuppresswarningpagegroup", cmd::ASSIGN_INT, I + 28),
    ("pdfinfoomitdate", cmd::ASSIGN_INT, I + 29),
    ("pdfsuppressptexinfo", cmd::ASSIGN_INT, I + 30),
    ("pdfomitcharset", cmd::ASSIGN_INT, I + 31),
    ("pdfomitinfodict", cmd::ASSIGN_INT, I + 32),
    ("pdfomitprocset", cmd::ASSIGN_INT, I + 33),
    ("pdfptexuseunderscore", cmd::ASSIGN_INT, I + 34),
    ("pdfhorigin", cmd::ASSIGN_DIMEN, D),
    ("pdfvorigin", cmd::ASSIGN_DIMEN, D + 1),
    ("pdfpagewidth", cmd::ASSIGN_DIMEN, D + 2),
    ("pdfpageheight", cmd::ASSIGN_DIMEN, D + 3),
    ("pdflinkmargin", cmd::ASSIGN_DIMEN, D + 4),
    ("pdfdestmargin", cmd::ASSIGN_DIMEN, D + 5),
    ("pdfthreadmargin", cmd::ASSIGN_DIMEN, D + 6),
    ("pdffirstlineheight", cmd::ASSIGN_DIMEN, D + 7),
    ("pdflastlinedepth", cmd::ASSIGN_DIMEN, D + 8),
    ("pdfeachlineheight", cmd::ASSIGN_DIMEN, D + 9),
    ("pdfeachlinedepth", cmd::ASSIGN_DIMEN, D + 10),
    ("pdfignoreddimen", cmd::ASSIGN_DIMEN, D + 11),
    ("pdfpxdimen", cmd::ASSIGN_DIMEN, D + 12),
    ("letterspacefont", cmd::SET_INTERACTION, 11),
    ("pdfcopyfont", cmd::SET_INTERACTION, 12),
    ("pdfprimitive", cmd::NO_EXPAND, 1),
    ("pdftexversion", cmd::LAST_ITEM, li::PDFTEX_VERSION),
    ("pdflastobj", cmd::LAST_ITEM, li::LAST_OBJ),
    ("pdflastxform", cmd::LAST_ITEM, li::LAST_XFORM),
    ("pdflastximage", cmd::LAST_ITEM, li::LAST_XIMAGE),
    ("pdflastximagepages", cmd::LAST_ITEM, li::LAST_XIMAGE_PAGES),
    ("pdflastannot", cmd::LAST_ITEM, li::LAST_ANNOT),
    ("pdflastxpos", cmd::LAST_ITEM, li::LAST_X_POS),
    ("pdflastypos", cmd::LAST_ITEM, li::LAST_Y_POS),
    ("pdfretval", cmd::LAST_ITEM, li::RETVAL),
    (
        "pdflastximagecolordepth",
        cmd::LAST_ITEM,
        li::LAST_XIMAGE_COLORDEPTH,
    ),
    ("pdfelapsedtime", cmd::LAST_ITEM, li::ELAPSED_TIME),
    ("pdfshellescape", cmd::LAST_ITEM, li::SHELL_ESCAPE),
    ("pdfrandomseed", cmd::LAST_ITEM, li::RANDOM_SEED),
    ("pdflastlink", cmd::LAST_ITEM, li::LAST_LINK),
    ("expanded", cmd::CONVERT, cv::EXPANDED),
    ("pdftexrevision", cmd::CONVERT, cv::PDFTEX_REVISION),
    ("pdftexbanner", cmd::CONVERT, cv::PDFTEX_BANNER),
    ("pdffontname", cmd::CONVERT, cv::FONT_NAME),
    ("pdffontobjnum", cmd::CONVERT, cv::FONT_OBJNUM),
    ("pdffontsize", cmd::CONVERT, cv::FONT_SIZE),
    ("pdfpageref", cmd::CONVERT, cv::PAGE_REF),
    ("leftmarginkern", cmd::CONVERT, cv::LEFT_MARGIN_KERN),
    ("rightmarginkern", cmd::CONVERT, cv::RIGHT_MARGIN_KERN),
    ("pdfxformname", cmd::CONVERT, cv::XFORM_NAME),
    ("pdfescapestring", cmd::CONVERT, cv::ESCAPE_STRING),
    ("pdfescapename", cmd::CONVERT, cv::ESCAPE_NAME),
    ("pdfescapehex", cmd::CONVERT, cv::ESCAPE_HEX),
    ("pdfunescapehex", cmd::CONVERT, cv::UNESCAPE_HEX),
    ("pdfcreationdate", cmd::CONVERT, cv::CREATION_DATE),
    ("pdffilemoddate", cmd::CONVERT, cv::FILE_MOD_DATE),
    ("pdffilesize", cmd::CONVERT, cv::FILE_SIZE),
    ("pdfmdfivesum", cmd::CONVERT, cv::MDFIVE_SUM),
    ("pdffiledump", cmd::CONVERT, cv::FILE_DUMP),
    ("pdfmatch", cmd::CONVERT, cv::MATCH),
    ("pdflastmatch", cmd::CONVERT, cv::LAST_MATCH),
    ("pdfstrcmp", cmd::CONVERT, cv::STRCMP),
    ("pdfcolorstackinit", cmd::CONVERT, cv::COLORSTACK_INIT),
    ("pdfuniformdeviate", cmd::CONVERT, cv::UNIFORM_DEVIATE),
    ("pdfnormaldeviate", cmd::CONVERT, cv::NORMAL_DEVIATE),
    ("pdfinsertht", cmd::CONVERT, cv::INSERT_HT),
    ("pdfximagebbox", cmd::CONVERT, cv::XIMAGE_BBOX),
    ("ifpdfprimitive", cmd::IF_TEST, crate::etex::IF_PDFPRIMITIVE),
    ("ifpdfabsnum", cmd::IF_TEST, crate::etex::IF_PDFABS_NUM),
    ("ifpdfabsdim", cmd::IF_TEST, crate::etex::IF_PDFABS_DIM),
    ("quitvmode", cmd::START_PAR, 2),
    ("lpcode", cmd::ASSIGN_FONT_INT, 2),
    ("rpcode", cmd::ASSIGN_FONT_INT, 3),
    ("efcode", cmd::ASSIGN_FONT_INT, 4),
    ("tagcode", cmd::ASSIGN_FONT_INT, TAG),
    ("knbscode", cmd::ASSIGN_FONT_INT, 6),
    ("stbscode", cmd::ASSIGN_FONT_INT, 7),
    ("shbscode", cmd::ASSIGN_FONT_INT, 8),
    ("knbccode", cmd::ASSIGN_FONT_INT, 9),
    ("knaccode", cmd::ASSIGN_FONT_INT, 10),
    ("pdfnoligatures", cmd::ASSIGN_FONT_INT, NO_LIG),
    ("pdfliteral", cmd::EXTENSION, ex::LITERAL),
    ("pdfcolorstack", cmd::EXTENSION, ex::COLORSTACK),
    ("pdfsetmatrix", cmd::EXTENSION, ex::SETMATRIX),
    ("pdfsave", cmd::EXTENSION, ex::SAVE),
    ("pdfrestore", cmd::EXTENSION, ex::RESTORE),
    ("pdfobj", cmd::EXTENSION, ex::OBJ),
    ("pdfrefobj", cmd::EXTENSION, ex::REFOBJ),
    ("pdfxform", cmd::EXTENSION, ex::XFORM),
    ("pdfrefxform", cmd::EXTENSION, ex::REFXFORM),
    ("pdfximage", cmd::EXTENSION, ex::XIMAGE),
    ("pdfrefximage", cmd::EXTENSION, ex::REFXIMAGE),
    ("pdfannot", cmd::EXTENSION, ex::ANNOT),
    ("pdfstartlink", cmd::EXTENSION, ex::START_LINK),
    ("pdfendlink", cmd::EXTENSION, ex::END_LINK),
    ("pdfoutline", cmd::EXTENSION, ex::OUTLINE),
    ("pdfdest", cmd::EXTENSION, ex::DEST),
    ("pdfthread", cmd::EXTENSION, ex::THREAD),
    ("pdfstartthread", cmd::EXTENSION, ex::START_THREAD),
    ("pdfendthread", cmd::EXTENSION, ex::END_THREAD),
    ("pdfsavepos", cmd::EXTENSION, ex::SAVE_POS),
    ("pdfsnaprefpoint", cmd::EXTENSION, ex::SNAP_REF_POINT),
    ("pdfsnapy", cmd::EXTENSION, ex::SNAPY),
    ("pdfsnapycomp", cmd::EXTENSION, ex::SNAPY_COMP),
    ("pdfinfo", cmd::EXTENSION, ex::INFO),
    ("pdfcatalog", cmd::EXTENSION, ex::CATALOG),
    ("pdfnames", cmd::EXTENSION, ex::NAMES),
    ("pdfincludechars", cmd::EXTENSION, ex::INCLUDE_CHARS),
    ("pdffontattr", cmd::EXTENSION, ex::FONT_ATTR),
    ("pdfmapfile", cmd::EXTENSION, ex::MAP_FILE),
    ("pdfmapline", cmd::EXTENSION, ex::MAP_LINE),
    ("pdftrailer", cmd::EXTENSION, ex::TRAILER),
    ("pdftrailerid", cmd::EXTENSION, ex::TRAILER_ID),
    ("pdfresettimer", cmd::EXTENSION, ex::RESET_TIMER),
    ("pdfsetrandomseed", cmd::EXTENSION, ex::SET_RANDOM_SEED),
    ("pdffontexpand", cmd::EXTENSION, ex::FONT_EXPAND),
    ("pdfglyphtounicode", cmd::EXTENSION, ex::GLYPH_TO_UNICODE),
    (
        "pdfnobuiltintounicode",
        cmd::EXTENSION,
        ex::NOBUILTIN_TOUNICODE,
    ),
    (
        "pdfinterwordspaceon",
        cmd::EXTENSION,
        ex::INTERWORD_SPACE_ON,
    ),
    (
        "pdfinterwordspaceoff",
        cmd::EXTENSION,
        ex::INTERWORD_SPACE_OFF,
    ),
    ("pdffakespace", cmd::EXTENSION, ex::FAKE_SPACE),
    ("pdfrunninglinkoff", cmd::EXTENSION, ex::RUNNING_LINK_OFF),
    ("pdfrunninglinkon", cmd::EXTENSION, ex::RUNNING_LINK_ON),
    ("pdfspacefont", cmd::EXTENSION, ex::SPACE_FONT),
];

/// `one_hundred_inch` (pdfTeX §695).
const ONE_HUNDRED_INCH: i32 = 473_628_672;
/// `one_bp`.
const ONE_BP: i32 = 65782;

/// INITEX's values of pdfTeX's parameters (pdfTeX §1128, "\pdfTeX\
/// basic").
pub(crate) fn initex(v: &phitex_defuse::Values) -> Vec<(String, ValueId)> {
    let origin = (ONE_HUNDRED_INCH + 50) / 100;
    [
        ("pdfhorigin", origin),
        ("pdfvorigin", origin),
        ("pdfcompresslevel", 9),
        ("pdfdecimaldigits", 3),
        ("pdfimageresolution", 72),
        ("pdfmajorversion", 1),
        ("pdfminorversion", 4),
        ("pdfgamma", 1000),
        ("pdfimagegamma", 2200),
        ("pdfimagehicolor", 1),
        ("pdfpxdimen", ONE_BP),
    ]
    .iter()
    .map(|&(n, x)| (format!("param:{n}"), v.int(i64::from(x))))
    .collect()
}

// ------------------------------------------------------- `\pdfprimitive`

/// `\pdfprimitive` (pdfTeX §1683): the next control sequence's
/// primitive meaning, whatever it means now.
pub(crate) fn pdf_primitive(r: &mut Run<'_, '_>) -> R<()> {
    r.get_token()?;
    let Tok::S(name) = r.cur.tok.clone() else {
        r.log("error", "Missing primitive name")?;
        r.back_cur();
        return Ok(());
    };
    let Some((n, c, h)) = crate::val::prim(&name) else {
        r.log("error", "Missing primitive name")?;
        r.back_cur();
        return Ok(());
    };
    // the frozen copy of the primitive
    let t = Tok::S(Rc::from(format!("\u{10FFFE}{n}")));
    let f = t.field_name().unwrap_or_default();
    if r.get(&f)?.is_none() {
        let m = Mean::Prim(n, c, h).encode(r.v);
        r.set(&f, m, true);
    }
    r.back_input(t);
    Ok(())
}

/// `\ifpdfprimitive` (pdfTeX §1684).
pub(crate) fn if_pdf_primitive(r: &mut Run<'_, '_>) -> R<bool> {
    r.get_token()?;
    let t = r.cur.tok.clone();
    let Tok::S(name) = &t else { return Ok(false) };
    let Some((n, _, _)) = crate::val::prim(name) else {
        return Ok(false);
    };
    Ok(matches!(&r.cur.mean, Mean::Prim(m, _, _) if *m == n))
}

// ------------------------------------------------------- the conversions

/// The token list of a `{...}` argument, expanded (`scan_pdf_ext_toks`),
/// as a string (`tokens_to_string`).
fn ext_string(r: &mut Run<'_, '_>) -> R<String> {
    let (_, body) = r.scan_toks(false, true)?;
    let mut s = String::new();
    r.show_toks_into(&mut s, &body)?;
    Ok(s)
}

fn bytes(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

fn string(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

fn hex_upper(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        let _ = write!(s, "{x:02X}");
    }
    s
}

/// A file's bytes, or the need for it.
fn file_bytes(r: &mut Run<'_, '_>, name: &str) -> R<Option<Vec<u8>>> {
    let Some(x) = r.file(name)? else {
        return Ok(None);
    };
    let xs = list_of(r.v, x);
    Ok(xs.get(1).map(|&t| bytes(&str_of(r.v, t))))
}

/// The conversions pdfTeX adds to `conv_toks` (pdfTeX §470 ff.).
#[allow(clippy::too_many_lines)]
pub(crate) fn convert(r: &mut Run<'_, '_>, c: i32) -> R<()> {
    let v = r.v;
    let out: String = match c {
        cv::EXPANDED => {
            let (_, body) = r.scan_toks(false, true)?;
            r.push_toks(body);
            return Ok(());
        }
        cv::PDFTEX_REVISION => "29".to_owned(),
        cv::PDFTEX_BANNER => "This is pdfTeX, Version 3.141592653-2.6-1.40.29".to_owned(),
        cv::FONT_NAME | cv::FONT_OBJNUM => {
            let f = r.scan_font_ident()?;
            let (name, obj) = font_num(r, f)?;
            let fid = u32::try_from(f).unwrap_or(0);
            let input = pdf_input(r, "fontobjnum", f, ValueId::UNIT)?;
            let got = crate::pdfout::command(r, input, &[fid], |st, fs| {
                let n = st.font_name(fid, fs).ok()?;
                let o = st.font_objnum(fid, fs).ok()?;
                Some((n, o))
            })?;
            let (name, obj) = got.unwrap_or((name, obj));
            (if c == cv::FONT_NAME { name } else { obj }).to_string()
        }
        cv::FONT_SIZE => {
            let f = r.scan_font_ident()?;
            let info = r.get(&format!("fontinfo:{f}"))?;
            let size = crate::fonts::info(v, info).map_or(0, |i| i.size);
            let mut s = String::new();
            crate::print::print_scaled(&mut s, size);
            s.push_str("pt");
            s
        }
        cv::PAGE_REF => {
            let n = r.scan_int()?;
            if n <= 0 {
                r.log("error", "pdfTeX error (pageref): invalid page number")?;
            }
            let f = format!("pdf:pageref:{n}");
            let obj = if let Some(x) = r.get(&f)? {
                int_of(v, x)
            } else {
                let o = alloc_obj(r)?;
                r.set(&f, v.int(o), true);
                o
            };
            obj.to_string()
        }
        cv::LEFT_MARGIN_KERN | cv::RIGHT_MARGIN_KERN => {
            // margin kerns come from \pdfprotrudechars, which the line
            // breaker does not make: none to find
            let n = r.scan_register_num()?;
            if r.box_reg(n)?.is_none_or(|b| !nodes::box_is_h(v, b)) {
                r.log(
                    "error",
                    "pdfTeX error (marginkern): a non-empty hbox expected",
                )?;
            }
            "0pt".to_owned()
        }
        cv::XFORM_NAME => {
            let n = r.scan_int()?;
            let x = r.get(&format!("pdf:xform:{n}"))?;
            x.map_or(0, |x| crate::val::int_at(v, x, 3)).to_string()
        }
        cv::ESCAPE_STRING => {
            let s = bytes(&ext_string(r)?);
            let mut o = String::new();
            for ch in s {
                if !(b'!'..=b'~').contains(&ch) {
                    let _ = write!(o, "\\{ch:03o}");
                    continue;
                }
                if matches!(ch, b'(' | b')' | b'\\') {
                    o.push('\\');
                }
                o.push(char::from(ch));
            }
            o
        }
        cv::ESCAPE_NAME => {
            let s = bytes(&ext_string(r)?);
            let mut o = String::new();
            for ch in s {
                match ch {
                    0 => {}
                    1..=32 | 127..=255 | 35 | 37 | 40 | 41 | 47 | 60 | 62 | 91 | 93 | 123 | 125 => {
                        let _ = write!(o, "#{ch:02X}");
                    }
                    _ => o.push(char::from(ch)),
                }
            }
            o
        }
        cv::ESCAPE_HEX => hex_upper(&bytes(&ext_string(r)?)),
        cv::UNESCAPE_HEX => {
            let s = bytes(&ext_string(r)?);
            let mut o = Vec::new();
            let mut first: Option<u8> = None;
            for ch in s {
                let d = match ch {
                    b'0'..=b'9' => ch - b'0',
                    b'A'..=b'F' => ch - b'A' + 10,
                    b'a'..=b'f' => ch - b'a' + 10,
                    _ => continue,
                };
                match first.take() {
                    None => first = Some(d << 4),
                    Some(a) => o.push(a + d),
                }
            }
            o.extend(first);
            string(&o)
        }
        cv::CREATION_DATE => creation_date(r)?,
        cv::FILE_MOD_DATE => {
            // PDF-BACKEND-GAP: the files the client supplies carry no
            // modification time; pdfTeX prints nothing for a file it
            // cannot stat
            let name = ext_string(r)?;
            let _ = file_bytes(r, &name)?;
            String::new()
        }
        cv::FILE_SIZE => {
            let name = ext_string(r)?;
            file_bytes(r, &name)?.map_or_else(String::new, |b| b.len().to_string())
        }
        cv::MDFIVE_SUM => {
            let file = r.scan_keyword("file")?;
            let s = ext_string(r)?;
            let data = if file {
                file_bytes(r, &s)?
            } else {
                Some(bytes(&s))
            };
            data.map_or_else(String::new, |d| hex_upper(&phitex_codecs::md5::md5(&d)))
        }
        cv::FILE_DUMP => {
            let mut off = 0;
            if r.scan_keyword("offset")? {
                off = r.scan_int()?;
                if off < 0 {
                    r.log("error", "Bad file offset")?;
                    off = 0;
                }
            }
            let mut len = 0;
            if r.scan_keyword("length")? {
                len = r.scan_int()?;
                if len < 0 {
                    r.log("error", "Bad dump length")?;
                    len = 0;
                }
            }
            let name = ext_string(r)?;
            match file_bytes(r, &name)? {
                Some(b) => {
                    let o = usize::try_from(off).unwrap_or(0).min(b.len());
                    let e = (o + usize::try_from(len).unwrap_or(0)).min(b.len());
                    hex_upper(&b[o..e])
                }
                None => String::new(),
            }
        }
        cv::MATCH => pdf_match(r)?,
        cv::LAST_MATCH => {
            let mut n = r.scan_int()?;
            if n < 0 {
                r.log("error", "Bad match number")?;
                n = 0;
            }
            last_match(r, n)?
        }
        cv::STRCMP => {
            let a = bytes(&ext_string(r)?);
            let b = bytes(&ext_string(r)?);
            match a.cmp(&b) {
                std::cmp::Ordering::Less => "-1",
                std::cmp::Ordering::Equal => "0",
                std::cmp::Ordering::Greater => "1",
            }
            .to_owned()
        }
        cv::COLORSTACK_INIT => {
            let page = r.scan_keyword("page")?;
            let mode = if r.scan_keyword("direct")? {
                2
            } else {
                i64::from(r.scan_keyword("page")?)
            };
            let s = ext_string(r)?;
            let mut stacks = r
                .get("pdf:colorstacks")?
                .map(|x| list_of(v, x))
                .unwrap_or_default();
            // stack 0 is pdfTeX's own ("0 g 0 G")
            let input = v.list(vec![
                v.str("colorstackinit"),
                v.int(i64::from(page)),
                v.int(mode),
                v.str(&s),
            ]);
            let init = bytes(&s);
            let got = crate::pdfout::command(r, input, &[], |st, _| {
                st.colorstack_init(page, mode == 2, &init).ok()
            })?;
            let n = got.unwrap_or(stacks.len() + 1);
            stacks.push(v.list(vec![v.int(i64::from(page)), v.int(mode), v.str(&s)]));
            r.set("pdf:colorstacks", v.list(stacks), true);
            n.to_string()
        }
        cv::UNIFORM_DEVIATE => {
            let x = r.scan_int()?;
            let mut st = random_state(r)?;
            let y = st.unif_rand(x);
            put_random(r, &st);
            y.to_string()
        }
        cv::NORMAL_DEVIATE => {
            let mut st = random_state(r)?;
            let y = st.norm_rand();
            put_random(r, &st);
            y.to_string()
        }
        cv::INSERT_HT => {
            // PDF-BACKEND-GAP: phitex-typeset's page builder keeps its
            // insertion records private; the class's height so far is
            // not readable, so this prints 0pt
            let _ = r.scan_register_num()?;
            "0pt".to_owned()
        }
        cv::XIMAGE_BBOX => {
            let n = r.scan_int()?;
            let j = r.scan_int()?;
            if !(1..=4).contains(&j) {
                r.log("error", "pdfTeX error (pdfximagebbox): invalid parameter")?;
            }
            let x = r.get(&format!("pdf:ximage:{n}"))?;
            let k = usize::try_from(j.clamp(1, 4) + 5).unwrap_or(6);
            let d = x.map_or(0, |x| crate::val::int_at(v, x, k));
            let mut s = String::new();
            crate::print::print_scaled(&mut s, i32::try_from(d).unwrap_or(0));
            s.push_str("pt");
            s
        }
        _ => String::new(),
    };
    r.push_toks(str_toks(&out));
    Ok(())
}

/// `\pdfcreationdate` (`getcreationdate`): `SOURCE_DATE_EPOCH` when the
/// environment gives it (F15: an environment input), as UTC.
fn creation_date(r: &mut Run<'_, '_>) -> R<String> {
    let t = match r.get("env:SOURCE_DATE_EPOCH")? {
        Some(x) => int_of(r.v, x),
        None => std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0),
    };
    Ok(pdf_date(t))
}

/// `D:YYYYMMDDHHmmSSZ` for UTC seconds `t`.
fn pdf_date(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// A font's resource number and object number (`pdf_init_font`),
/// allocated when first asked.
fn font_num(r: &mut Run<'_, '_>, f: i64) -> R<(i64, i64)> {
    let v = r.v;
    if f == 0 {
        r.log("error", "pdfTeX error (font): invalid font identifier")?;
    }
    let field = format!("pdf:fontnum:{f}");
    if let Some(x) = r.get(&field)? {
        return Ok((crate::val::int_at(v, x, 0), crate::val::int_at(v, x, 1)));
    }
    let n = r.int("pdf:fontcount")? + 1;
    r.set("pdf:fontcount", v.int(n), true);
    let o = alloc_obj(r)?;
    r.set(&field, v.list(vec![v.int(n), v.int(o)]), true);
    Ok((n, o))
}

/// The next object number (`pdf_create_obj`): allocation order.
fn alloc_obj(r: &mut Run<'_, '_>) -> R<i64> {
    let n = r.int("pdf:objnum")? + 1;
    let x = r.v.int(n);
    r.set("pdf:objnum", x, true);
    Ok(n)
}

// ------------------------------------------------------------ `\pdfmatch`

/// `\pdfmatch` (utils.c `matchstrings`): POSIX extended expressions.
fn pdf_match(r: &mut Run<'_, '_>) -> R<String> {
    let icase = r.scan_keyword("icase")?;
    let mut sub = -1;
    if r.scan_keyword("subcount")? {
        sub = r.scan_int()?;
    }
    let pat = bytes(&ext_string(r)?);
    let text = bytes(&ext_string(r)?);
    let v = r.v;
    let Some(re) = regex::Re::parse(&pat, icase) else {
        r.log(
            "wlog",
            "pdfTeX warning: \\pdfmatch: invalid regular expression",
        )?;
        return Ok("-1".to_owned());
    };
    let count = if sub < 0 { 10 } else { sub };
    let found = re.find(&text);
    let mut groups = Vec::new();
    if let Some(g) = &found {
        for k in 0..usize::try_from(count).unwrap_or(0) {
            let (s, e) = g.get(k).copied().flatten().unwrap_or((-1, -1));
            groups.push(v.list(vec![v.int(s), v.int(e)]));
        }
    }
    let st = v.list(vec![
        v.int(i64::from(found.is_some())),
        v.int(i64::from(count)),
        v.str(&string(&text)),
        v.list(groups),
    ]);
    r.set("pdf:match", st, true);
    Ok(if found.is_some() { "1" } else { "0" }.to_owned())
}

/// `\pdflastmatch` (utils.c `getmatch`).
fn last_match(r: &mut Run<'_, '_>, n: i32) -> R<String> {
    let v = r.v;
    let Some(st) = r.get("pdf:match")? else {
        return Ok("-1->".to_owned());
    };
    let xs = list_of(v, st);
    let ok = xs.first().is_some_and(|&x| int_of(v, x) != 0);
    let count = xs.get(1).map_or(0, |&x| int_of(v, x));
    let text = xs.get(2).map(|&x| bytes(&str_of(v, x))).unwrap_or_default();
    let groups = xs.get(3).map(|&x| list_of(v, x)).unwrap_or_default();
    if ok && i64::from(n) < count {
        let k = usize::try_from(n).unwrap_or(0);
        if let Some(&g) = groups.get(k) {
            let (s, e) = (crate::val::int_at(v, g, 0), crate::val::int_at(v, g, 1));
            if s >= 0 && e >= s {
                let (su, eu) = (
                    usize::try_from(s).unwrap_or(0),
                    usize::try_from(e).unwrap_or(0),
                );
                return Ok(format!("{s}->{}", string(&text[su..eu.min(text.len())])));
            }
        }
    }
    Ok("-1->".to_owned())
}

mod regex {
    //! A backtracking matcher for POSIX extended regular expressions:
    //! the leftmost match, its alternatives and repetitions tried longest
    //! first, the groups of that match.

    #[derive(Debug, Clone)]
    enum Atom {
        Char(u8),
        Any,
        Class(Vec<(u8, u8)>, bool),
        Group(Box<Alt>, usize),
        Bol,
        Eol,
    }

    #[derive(Debug, Clone)]
    struct Piece {
        atom: Atom,
        min: usize,
        max: Option<usize>,
    }

    #[derive(Debug, Clone)]
    struct Alt(Vec<Vec<Piece>>);

    pub(super) struct Re {
        root: Alt,
        groups: usize,
        icase: bool,
    }

    type Caps = Vec<Option<(i64, i64)>>;

    struct Parser<'a> {
        p: &'a [u8],
        i: usize,
        groups: usize,
        icase: bool,
    }

    impl Parser<'_> {
        fn alt(&mut self) -> Option<Alt> {
            let mut alts = vec![self.seq()?];
            while self.i < self.p.len() && self.p[self.i] == b'|' {
                self.i += 1;
                alts.push(self.seq()?);
            }
            Some(Alt(alts))
        }

        fn seq(&mut self) -> Option<Vec<Piece>> {
            let mut out = Vec::new();
            while self.i < self.p.len() && self.p[self.i] != b'|' && self.p[self.i] != b')' {
                let atom = self.atom()?;
                let (mut min, mut max) = (1, Some(1));
                if self.i < self.p.len() {
                    match self.p[self.i] {
                        b'*' => {
                            (min, max) = (0, None);
                            self.i += 1;
                        }
                        b'+' => {
                            (min, max) = (1, None);
                            self.i += 1;
                        }
                        b'?' => {
                            (min, max) = (0, Some(1));
                            self.i += 1;
                        }
                        b'{' => {
                            let save = self.i;
                            self.i += 1;
                            let a = self.num();
                            if let Some(a) = a {
                                min = a;
                                max = Some(a);
                                if self.p.get(self.i) == Some(&b',') {
                                    self.i += 1;
                                    max = self.num();
                                }
                                if self.p.get(self.i) == Some(&b'}') {
                                    self.i += 1;
                                } else {
                                    return None;
                                }
                            } else {
                                self.i = save;
                            }
                        }
                        _ => {}
                    }
                }
                out.push(Piece { atom, min, max });
            }
            Some(out)
        }

        fn num(&mut self) -> Option<usize> {
            let s = self.i;
            while self.i < self.p.len() && self.p[self.i].is_ascii_digit() {
                self.i += 1;
            }
            std::str::from_utf8(&self.p[s..self.i]).ok()?.parse().ok()
        }

        fn atom(&mut self) -> Option<Atom> {
            let c = self.p[self.i];
            self.i += 1;
            Some(match c {
                b'.' => Atom::Any,
                b'^' => Atom::Bol,
                b'$' => Atom::Eol,
                b'(' => {
                    self.groups += 1;
                    let k = self.groups;
                    let a = self.alt()?;
                    if self.p.get(self.i) != Some(&b')') {
                        return None;
                    }
                    self.i += 1;
                    Atom::Group(Box::new(a), k)
                }
                b'[' => self.class()?,
                b'\\' => {
                    let d = *self.p.get(self.i)?;
                    self.i += 1;
                    self.lit(d)
                }
                b'*' | b'+' | b'?' => return None,
                _ => self.lit(c),
            })
        }

        fn lit(&self, c: u8) -> Atom {
            if self.icase && c.is_ascii_alphabetic() {
                let (l, u) = (c.to_ascii_lowercase(), c.to_ascii_uppercase());
                Atom::Class(vec![(l, l), (u, u)], false)
            } else {
                Atom::Char(c)
            }
        }

        fn class(&mut self) -> Option<Atom> {
            let mut neg = false;
            if self.p.get(self.i) == Some(&b'^') {
                neg = true;
                self.i += 1;
            }
            let mut ranges = Vec::new();
            let mut first = true;
            loop {
                let c = *self.p.get(self.i)?;
                if c == b']' && !first {
                    self.i += 1;
                    break;
                }
                first = false;
                if c == b'[' && self.p.get(self.i + 1) == Some(&b':') {
                    let rest = &self.p[self.i + 2..];
                    let end = rest.windows(2).position(|w| w == b":]")?;
                    let name = &rest[..end];
                    self.i += 2 + end + 2;
                    let add: &[(u8, u8)] = match name {
                        b"digit" => &[(b'0', b'9')],
                        b"alpha" => &[(b'a', b'z'), (b'A', b'Z')],
                        b"alnum" => &[(b'a', b'z'), (b'A', b'Z'), (b'0', b'9')],
                        b"upper" => &[(b'A', b'Z')],
                        b"lower" => &[(b'a', b'z')],
                        b"space" => &[(b' ', b' '), (9, 13)],
                        b"blank" => &[(b' ', b' '), (9, 9)],
                        b"punct" => &[(33, 47), (58, 64), (91, 96), (123, 126)],
                        b"xdigit" => &[(b'0', b'9'), (b'a', b'f'), (b'A', b'F')],
                        b"cntrl" => &[(0, 31), (127, 127)],
                        b"print" => &[(32, 126)],
                        b"graph" => &[(33, 126)],
                        _ => return None,
                    };
                    ranges.extend_from_slice(add);
                    continue;
                }
                self.i += 1;
                if self.p.get(self.i) == Some(&b'-')
                    && self.p.get(self.i + 1).is_some_and(|&x| x != b']')
                {
                    let hi = self.p[self.i + 1];
                    self.i += 2;
                    ranges.push((c, hi));
                } else {
                    ranges.push((c, c));
                }
            }
            if self.icase {
                let extra: Vec<(u8, u8)> = ranges
                    .iter()
                    .flat_map(|&(a, b)| {
                        (a..=b)
                            .filter(u8::is_ascii_alphabetic)
                            .map(|c| {
                                let o = if c.is_ascii_lowercase() {
                                    c.to_ascii_uppercase()
                                } else {
                                    c.to_ascii_lowercase()
                                };
                                (o, o)
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                ranges.extend(extra);
            }
            Some(Atom::Class(ranges, neg))
        }
    }

    impl Re {
        pub(super) fn parse(p: &[u8], icase: bool) -> Option<Re> {
            let mut ps = Parser {
                p,
                i: 0,
                groups: 0,
                icase,
            };
            let root = ps.alt()?;
            if ps.i != p.len() {
                return None;
            }
            Some(Re {
                root,
                groups: ps.groups,
                icase,
            })
        }

        /// The leftmost match: group 0 and the others.
        pub(super) fn find(&self, s: &[u8]) -> Option<Caps> {
            let _ = self.icase;
            for start in 0..=s.len() {
                let mut caps: Caps = vec![None; self.groups + 1];
                let mut best: Option<(usize, Caps)> = None;
                m_alt(&self.root, s, start, &mut caps, &mut |e, c| {
                    if best.as_ref().is_none_or(|(b, _)| e > *b) {
                        best = Some((e, c.clone()));
                    }
                    false
                });
                if let Some((e, mut c)) = best {
                    c[0] = Some((
                        i64::try_from(start).unwrap_or(0),
                        i64::try_from(e).unwrap_or(0),
                    ));
                    return Some(c);
                }
            }
            None
        }
    }

    /// Calls `k` with every end the alternatives can reach from `i`
    /// (longest-first search, all of them); `k` returns true to stop.
    fn m_alt(
        a: &Alt,
        s: &[u8],
        i: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &Caps) -> bool,
    ) -> bool {
        for seq in &a.0 {
            if m_seq(seq, 0, s, i, caps, k) {
                return true;
            }
        }
        false
    }

    fn m_seq(
        seq: &[Piece],
        j: usize,
        s: &[u8],
        i: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &Caps) -> bool,
    ) -> bool {
        let Some(p) = seq.get(j) else {
            return k(i, caps);
        };
        m_rep(p, 0, seq, j, s, i, caps, k)
    }

    #[allow(clippy::too_many_arguments)]
    fn m_rep(
        p: &Piece,
        n: usize,
        seq: &[Piece],
        j: usize,
        s: &[u8],
        i: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &Caps) -> bool,
    ) -> bool {
        // one more repetition first (greedy), then the rest of the
        // sequence
        if p.max.is_none_or(|m| n < m) {
            let stop = m_atom(&p.atom, s, i, caps, &mut |e, c| {
                if e == i && n >= p.min {
                    return false;
                }
                let mut c2 = c.clone();
                m_rep(p, n + 1, seq, j, s, e, &mut c2, k)
            });
            if stop {
                return true;
            }
        }
        if n >= p.min {
            return m_seq(seq, j + 1, s, i, caps, k);
        }
        false
    }

    fn m_atom(
        a: &Atom,
        s: &[u8],
        i: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &Caps) -> bool,
    ) -> bool {
        match a {
            Atom::Char(c) => i < s.len() && s[i] == *c && k(i + 1, caps),
            Atom::Any => i < s.len() && k(i + 1, caps),
            Atom::Class(rs, neg) => {
                i < s.len()
                    && (rs.iter().any(|&(a, b)| (a..=b).contains(&s[i])) != *neg)
                    && k(i + 1, caps)
            }
            Atom::Bol => i == 0 && k(i, caps),
            Atom::Eol => i == s.len() && k(i, caps),
            Atom::Group(alt, g) => {
                let g = *g;
                m_alt(alt, s, i, caps, &mut |e, c| {
                    let mut c2 = c.clone();
                    c2[g] = Some((i64::try_from(i).unwrap_or(0), i64::try_from(e).unwrap_or(0)));
                    k(e, &c2)
                })
            }
        }
    }
}

// -------------------------------------------------------- random numbers

const FRACTION_ONE: i32 = 0o2000000000;
const FRACTION_HALF: i32 = 0o1000000000;
const FRACTION_FOUR: i32 = 0o10000000000;
const EL_GORDO: i32 = 0o17777777777;

/// The generator (pdfTeX §2560 ff., from \MP): `randoms`, `j_random`,
/// `random_seed`.
struct Random {
    seed: i32,
    j: usize,
    a: [i32; 55],
}

fn make_frac(mut p: i32, mut q: i32) -> i32 {
    let mut negative = false;
    if p < 0 {
        p = -p;
        negative = true;
    }
    if q <= 0 {
        q = -q;
        negative = !negative;
    }
    let n = p / q;
    p %= q;
    if n >= 8 {
        return if negative { -EL_GORDO } else { EL_GORDO };
    }
    let n = (n - 1) * FRACTION_ONE;
    let mut f = 1;
    loop {
        let be_careful = p - q;
        p += be_careful;
        if p >= 0 {
            f = f + f + 1;
        } else {
            f += f;
            p += q;
        }
        if f >= FRACTION_ONE {
            break;
        }
    }
    let be_careful = p - q;
    if be_careful + p >= 0 {
        f += 1;
    }
    if negative { -(f + n) } else { f + n }
}

fn take_frac(mut q: i32, mut f: i32) -> i32 {
    let mut negative = false;
    if f < 0 {
        f = -f;
        negative = true;
    }
    if q < 0 {
        q = -q;
        negative = !negative;
    }
    let mut n;
    if f < FRACTION_ONE {
        n = 0;
    } else {
        n = f / FRACTION_ONE;
        f %= FRACTION_ONE;
        if q <= EL_GORDO / n {
            n *= q;
        } else {
            n = EL_GORDO;
        }
    }
    f += FRACTION_ONE;
    let mut p = FRACTION_HALF;
    if q < FRACTION_FOUR {
        loop {
            p = if f % 2 == 1 {
                i32::midpoint(p, q)
            } else {
                p / 2
            };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    } else {
        loop {
            p = if f % 2 == 1 { p + (q - p) / 2 } else { p / 2 };
            f /= 2;
            if f == 1 {
                break;
            }
        }
    }
    let be_careful = n - EL_GORDO;
    if be_careful + p > 0 {
        n = EL_GORDO - p;
    }
    if negative { -(n + p) } else { n + p }
}

fn m_log(mut x: i32) -> i32 {
    const SPEC_LOG: [i32; 29] = [
        0, 93_032_640, 38_612_034, 17_922_280, 8_662_214, 4_261_238, 2_113_709, 1_052_693, 525_315,
        262_400, 131_136, 65_552, 32_772, 16_385, 8192, 4096, 2048, 1024, 512, 256, 128, 64, 32,
        16, 8, 4, 2, 1, 1,
    ];
    if x <= 0 {
        return 0;
    }
    let mut y: i32 = 1_302_456_956 + 4 - 100;
    let mut z: i32 = 27595 + 6_553_600;
    while x < FRACTION_FOUR {
        x += x;
        y -= 93_032_639;
        z -= 48782;
    }
    y += z / 65536;
    let mut k = 2usize;
    while x > FRACTION_FOUR + 4 {
        let mut zz = ((x - 1) / (1 << k)) + 1;
        while x < FRACTION_FOUR + zz {
            zz = (zz + 1) / 2;
            k += 1;
        }
        y += SPEC_LOG[k];
        x -= zz;
    }
    y / 8
}

#[allow(clippy::bool_to_int_with_if)]
fn ab_vs_cd(mut a: i32, mut b: i32, mut c: i32, mut d: i32) -> i32 {
    if a < 0 {
        a = -a;
        b = -b;
    }
    if c < 0 {
        c = -c;
        d = -d;
    }
    if d <= 0 {
        if b >= 0 {
            return if (a == 0 || b == 0) && (c == 0 || d == 0) {
                0
            } else {
                1
            };
        }
        if d == 0 {
            return if a == 0 { 0 } else { -1 };
        }
        std::mem::swap(&mut a, &mut c);
        let q = -b;
        b = -d;
        d = q;
    } else if b <= 0 {
        if b < 0 && a > 0 {
            return -1;
        }
        return if c == 0 { 0 } else { -1 };
    }
    loop {
        let (q, r) = (a / d, c / b);
        if q != r {
            return if q > r { 1 } else { -1 };
        }
        let (q, r) = (a % d, c % b);
        if r == 0 {
            return if q == 0 { 0 } else { 1 };
        }
        if q == 0 {
            return -1;
        }
        a = b;
        b = q;
        c = d;
        d = r;
    }
}

impl Random {
    fn new(seed: i32) -> Random {
        let mut r = Random {
            seed,
            j: 0,
            a: [0; 55],
        };
        let mut j = seed.saturating_abs();
        while j >= FRACTION_ONE {
            j /= 2;
        }
        let mut k = 1;
        for i in 0..55 {
            let jj = k;
            k = j - k;
            j = jj;
            if k < 0 {
                k += FRACTION_ONE;
            }
            r.a[(i * 21) % 55] = j;
        }
        r.new_randoms();
        r.new_randoms();
        r.new_randoms();
        r
    }

    fn new_randoms(&mut self) {
        for k in 0..24 {
            let mut x = self.a[k] - self.a[k + 31];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.a[k] = x;
        }
        for k in 24..55 {
            let mut x = self.a[k] - self.a[k - 24];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.a[k] = x;
        }
        self.j = 54;
    }

    fn next(&mut self) -> i32 {
        if self.j == 0 {
            self.new_randoms();
        } else {
            self.j -= 1;
        }
        self.a[self.j]
    }

    fn unif_rand(&mut self, x: i32) -> i32 {
        let r = self.next();
        let y = take_frac(x.saturating_abs(), r);
        if y == x.saturating_abs() {
            0
        } else if x > 0 {
            y
        } else {
            -y
        }
    }

    fn norm_rand(&mut self) -> i32 {
        loop {
            let (mut x, mut u);
            loop {
                let r = self.next();
                x = take_frac(112_429, r - FRACTION_HALF);
                u = self.next();
                if x.saturating_abs() < u {
                    break;
                }
            }
            x = make_frac(x, u);
            let l = 139_548_960 - m_log(u);
            if ab_vs_cd(1024, l, x, x) >= 0 {
                return x;
            }
        }
    }
}

/// The generator's state where this node is; first seeded as pdfTeX
/// seeds it at its start (F15: the time is an environment input).
fn random_state(r: &mut Run<'_, '_>) -> R<Random> {
    let v = r.v;
    if let Some(x) = r.get("pdf:random")? {
        let xs = list_of(v, x);
        let n = |k: usize| {
            xs.get(k)
                .map_or(0, |&y| i32::try_from(int_of(v, y)).unwrap_or(0))
        };
        let mut a = [0; 55];
        for (k, s) in a.iter_mut().enumerate() {
            *s = n(k + 2);
        }
        return Ok(Random {
            seed: n(0),
            j: usize::try_from(n(1)).unwrap_or(0),
            a,
        });
    }
    let t = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Ok(Random::new(i32::try_from(t % 1_000_000).unwrap_or(0)))
}

fn put_random(r: &mut Run<'_, '_>, st: &Random) {
    let v = r.v;
    let mut xs = vec![
        v.int(i64::from(st.seed)),
        v.int(i64::try_from(st.j).unwrap_or(0)),
    ];
    xs.extend(st.a.iter().map(|&x| v.int(i64::from(x))));
    r.set("pdf:random", v.list(xs), true);
}

// ----------------------------------------------------------- `last_item`

#[allow(clippy::match_same_arms)]
pub(crate) fn last_item(r: &mut Run<'_, '_>, m: i32) -> R<IVal> {
    let field = match m {
        li::PDFTEX_VERSION => return Ok(IVal::Int(140)),
        li::SHELL_ESCAPE => return Ok(IVal::Int(0)),
        // F15: the clock is not on the graph; \pdfresettimer is when
        // the run starts
        li::ELAPSED_TIME => return Ok(IVal::Int(0)),
        li::RANDOM_SEED => return Ok(IVal::Int(random_state(r)?.seed)),
        li::LAST_OBJ => "pdf:lastobj",
        li::LAST_XFORM => "pdf:lastxform",
        li::LAST_XIMAGE => "pdf:lastximage",
        li::LAST_XIMAGE_PAGES => "pdf:lastximagepages",
        li::LAST_XIMAGE_COLORDEPTH => "pdf:lastximagecolordepth",
        li::LAST_ANNOT => "pdf:lastannot",
        li::LAST_LINK => "pdf:lastlink",
        li::LAST_X_POS => "pdf:lastxpos",
        li::LAST_Y_POS => "pdf:lastypos",
        li::RETVAL => "pdf:retval",
        _ => return Ok(IVal::Int(0)),
    };
    Ok(IVal::Int(r.int32(field)?))
}

// ------------------------------------------------------------ font codes

fn code_field(m: i32, f: i64, c: i32) -> String {
    let name = FONT_CODES[usize::try_from(m).unwrap_or(0).min(11)];
    format!("fcode:{name}:{f}:{c}")
}

/// `\lpcode` and the other font codes, read (pdfTeX §426).
pub(crate) fn font_code(r: &mut Run<'_, '_>, m: i32) -> R<IVal> {
    let f = r.scan_font_ident()?;
    if m == NO_LIG {
        return Ok(IVal::Int(r.int32(&format!("fcode:nolig:{f}"))?));
    }
    let c = r.scan_char_num()?;
    let x = r.get(&code_field(m, f, c))?;
    if let Some(x) = x {
        return Ok(IVal::Int(i32::try_from(int_of(r.v, x)).unwrap_or(0)));
    }
    Ok(IVal::Int(match m {
        4 => 1000,
        TAG => {
            // the character's tag in the font (0 none, 1 lig/kern, 2
            // list, 3 extensible), -1 if it does not exist
            tag_of(r, f, c)?
        }
        _ => 0,
    }))
}

fn tag_of(r: &mut Run<'_, '_>, f: i64, c: i32) -> R<i32> {
    let Ok(fu) = u32::try_from(f) else {
        return Ok(-1);
    };
    let mut view = crate::fonts::View::default();
    r.add_font(&mut view, fu)?;
    let Some((l, _)) = view.fonts.get(&fu) else {
        return Ok(-1);
    };
    let Ok(cu) = u8::try_from(c) else {
        return Ok(-1);
    };
    Ok(match l.tfm.char_info(cu) {
        Some(ci) if ci.exists() => match ci.tag {
            phitex_tfm::Tag::None => 0,
            phitex_tfm::Tag::Lig => 1,
            phitex_tfm::Tag::List => 2,
            phitex_tfm::Tag::Ext => 3,
        },
        _ => -1,
    })
}

/// The pdfTeX assignments of `prefixed_command`.
pub(crate) fn prefixed(r: &mut Run<'_, '_>, global: bool) -> R<bool> {
    let v = r.v;
    let (c, chr) = (r.cur.cmd, r.cur.chr);
    if c == cmd::ASSIGN_FONT_INT && chr >= 2 {
        let f = r.scan_font_ident()?;
        if chr == NO_LIG {
            // \pdfnoligatures (pdfTeX §1253)
            r.set(&format!("fcode:nolig:{f}"), v.int(1), true);
            return Ok(true);
        }
        let ch = r.scan_char_num()?;
        r.scan_optional_equals()?;
        let n = r.scan_int()?;
        let n = if chr == 4 {
            n.clamp(0, 1000)
        } else if chr == TAG {
            n
        } else {
            n.clamp(-1000, 1000)
        };
        r.set(&code_field(chr, f, ch), v.int(i64::from(n)), true);
        return Ok(true);
    }
    if c == cmd::SET_INTERACTION && (chr == 11 || chr == 12) {
        // \letterspacefont, \pdfcopyfont (pdfTeX §1257 ff.)
        let u = r.get_r_tok()?;
        let t: String = match &u {
            Tok::S(n) => n.to_string(),
            Tok::C(_) => format!("FONT{}", char::from_u32(u.chr()).unwrap_or('?')),
        };
        r.define_cs(&u, &Mean::Font(0), global);
        r.scan_optional_equals()?;
        let f = r.scan_font_ident()?;
        let mut suffix = String::new();
        if chr == 11 {
            let ls = r.scan_int()?.clamp(-1000, 1000);
            let nolig = r.scan_keyword("nolig")?;
            suffix = format!("{}{ls}ls", if ls >= 0 { "+" } else { "" });
            let _ = nolig;
            // PDF-BACKEND-GAP: a letter-spaced font is a virtual font
            // whose characters carry the spacing; the copy here has the
            // original's metrics
        }
        let nf = copy_font(r, f, &suffix)?;
        r.define_cs(&u, &Mean::Font(nf), global);
        let id = Tok::S(Rc::from(format!("\u{10FFFE}F{nf}\u{2}{t}")));
        r.define_cs(&id, &Mean::Font(nf), true);
        r.set(&format!("fontid:{nf}"), v.str(&t), true);
        return Ok(true);
    }
    Ok(false)
}

/// A new font with `f`'s metrics (and its name plus `suffix`).
fn copy_font(r: &mut Run<'_, '_>, f: i64, suffix: &str) -> R<i64> {
    let v = r.v;
    let info = r.get(&format!("fontinfo:{f}"))?;
    let Some(i) = crate::fonts::info(v, info) else {
        return Ok(0);
    };
    let nf = r.int("fontcount")? + 1;
    r.set("fontcount", v.int(nf), true);
    let mut xs = vec![
        v.str(&format!("{}{suffix}", i.name)),
        v.int(i64::from(i.size)),
        v.int(i64::from(i.dsize)),
    ];
    xs.extend(i.file);
    xs.push(v.int(i64::from(i.s)));
    r.set(&format!("fontinfo:{nf}"), v.list(xs), true);
    let n = r.int32(&format!("fdn:{f}"))?;
    for k in 1..=n {
        let d = r.int(&format!("fd:{f}:{k}"))?;
        r.set(&format!("fd:{nf}:{k}"), v.int(d), true);
    }
    r.set(&format!("fdn:{nf}"), v.int(i64::from(n)), true);
    let hc = r.int(&format!("hc:{f}"))?;
    let sc = r.int(&format!("sc:{f}"))?;
    r.set(&format!("hc:{nf}"), v.int(hc), true);
    r.set(&format!("sc:{nf}"), v.int(sc), true);
    Ok(nf)
}

impl Run<'_, '_> {
    /// `get_r_token` (§1215).
    fn get_r_tok(&mut self) -> R<Tok> {
        loop {
            self.get_token()?;
            if self.cur.tok != crate::val::space_tok() {
                break;
            }
        }
        if !self.cur.is_cs() {
            self.back_cur();
            return Ok(Tok::cs("inaccessible"));
        }
        Ok(self.cur.tok.clone())
    }
}

// ------------------------------------------------- whatsits and objects

/// The input of a font command's writer step: the command, the font
/// as loaded (its `fontinfo`), and the data.
fn pdf_input(r: &mut Run<'_, '_>, kind: &str, f: i64, data: ValueId) -> R<ValueId> {
    let v = r.v;
    let info = r.get(&format!("fontinfo:{f}"))?.unwrap_or(ValueId::UNIT);
    Ok(v.list(vec![v.str(kind), v.int(f), info, data]))
}

fn whatsit(r: &mut Run<'_, '_>, md: &Mode, data: Vec<ValueId>) {
    let v = r.v;
    let node = v.list(vec![v.int(nodes::T_WHATSIT), v.list(data)]);
    r.append(md.lid, node);
}

/// An empty box of the given dimensions: what a reference to a form or
/// an image takes in its list (pdfTeX §1365 packs them as rules).
fn space_box(r: &mut Run<'_, '_>, md: &Mode, w: i32, h: i32, d: i32) -> R<()> {
    let v = r.v;
    let b = BoxNode {
        kind: BoxKind::H,
        width: w,
        height: h,
        depth: d,
        ..BoxNode::default()
    };
    let x = nodes::encode_box(v, &b, &nodes::Side::default());
    if md.m.abs() == 1 {
        r.append_to_vlist(md, x)?;
    } else {
        r.append(md.lid, x);
        if md.m.abs() == 102 {
            r.set_space_factor(md, 1000);
        }
    }
    Ok(())
}

/// `scan_pdf_ext_toks` as a value.
fn ext_value(r: &mut Run<'_, '_>) -> R<ValueId> {
    let s = ext_string(r)?;
    Ok(r.v.str(&s))
}

/// An optional `attr {...}`.
fn attr(r: &mut Run<'_, '_>) -> R<ValueId> {
    if r.scan_keyword("attr")? {
        ext_value(r)
    } else {
        Ok(r.v.str(""))
    }
}

/// `scan_alt_rule` (pdfTeX §1348): `width`, `height`, `depth` in any
/// order, running where not given.
fn alt_rule(r: &mut Run<'_, '_>) -> R<(i32, i32, i32)> {
    let running = phitex_typeset::node::RUNNING;
    let (mut w, mut h, mut d) = (running, running, running);
    loop {
        if r.scan_keyword("width")? {
            w = r.scan_normal_dimen()?;
        } else if r.scan_keyword("height")? {
            h = r.scan_normal_dimen()?;
        } else if r.scan_keyword("depth")? {
            d = r.scan_normal_dimen()?;
        } else {
            return Ok((w, h, d));
        }
    }
}

/// `scan_action` (pdfTeX §1360): `user {…}`, `goto …`, `thread …`.
fn action(r: &mut Run<'_, '_>) -> R<ValueId> {
    let v = r.v;
    let kind = if r.scan_keyword("user")? {
        let s = ext_value(r)?;
        return Ok(v.list(vec![v.str("user"), s]));
    } else if r.scan_keyword("goto")? {
        "goto"
    } else if r.scan_keyword("thread")? {
        "thread"
    } else {
        r.log("error", "pdfTeX error (ext1): action type missing")?;
        return Ok(v.list(vec![v.str("none")]));
    };
    let file = if r.scan_keyword("file")? {
        ext_value(r)?
    } else {
        v.str("")
    };
    let page = if kind == "goto" && r.scan_keyword("page")? {
        let n = r.scan_int()?;
        let s = ext_value(r)?;
        Some(v.list(vec![v.int(i64::from(n)), s]))
    } else {
        None
    };
    let id = if page.is_some() {
        v.str("")
    } else if r.scan_keyword("name")? {
        let s = ext_value(r)?;
        v.list(vec![v.str("name"), s])
    } else if r.scan_keyword("num")? {
        let n = r.scan_int()?;
        v.list(vec![v.str("num"), v.int(i64::from(n))])
    } else if kind == "goto" && r.scan_keyword("struct")? {
        let s = ext_value(r)?;
        v.list(vec![v.str("struct"), s])
    } else {
        v.str("")
    };
    let mut window = 0;
    if r.scan_keyword("newwindow")? {
        window = 1;
    } else if r.scan_keyword("nonewwindow")? {
        window = 2;
    }
    Ok(v.list(vec![
        v.str(kind),
        file,
        page.unwrap_or(ValueId::UNIT),
        id,
        v.int(window),
    ]))
}

/// `\pdfdest`'s and `\pdfthread`'s identifier: `num n` or `name {…}`.
fn identifier(r: &mut Run<'_, '_>) -> R<ValueId> {
    let v = r.v;
    if r.scan_keyword("num")? {
        let n = r.scan_int()?;
        if n <= 0 {
            r.log(
                "error",
                "pdfTeX error (ext1): num identifier must be positive",
            )?;
        }
        return Ok(v.list(vec![v.str("num"), v.int(i64::from(n))]));
    }
    if r.scan_keyword("name")? {
        let s = ext_value(r)?;
        return Ok(v.list(vec![v.str("name"), s]));
    }
    r.log("error", "pdfTeX error (ext1): identifier type missing")?;
    Ok(v.list(vec![v.str("num"), v.int(0)]))
}

/// An optional `reserveobjnum` or `useobjnum n` (pdfTeX §1349): the
/// object number, and whether it is only reserved.
fn obj_num_option(r: &mut Run<'_, '_>) -> R<(i64, bool)> {
    if r.scan_keyword("reserveobjnum")? {
        return Ok((alloc_obj(r)?, true));
    }
    if r.scan_keyword("useobjnum")? {
        let n = r.scan_int()?;
        return Ok((i64::from(n), false));
    }
    Ok((alloc_obj(r)?, false))
}

/// Appends `text` to the accumulated string field `name`.
fn append_text(r: &mut Run<'_, '_>, name: &str, text: ValueId) -> R<()> {
    let v = r.v;
    let mut xs = r.get(name)?.map(|x| list_of(v, x)).unwrap_or_default();
    xs.push(text);
    r.set(name, v.list(xs), true);
    Ok(())
}

/// The `extension` commands of pdfTeX (pdfTeX §1348 ff.).
#[allow(clippy::too_many_lines)]
#[allow(clippy::match_same_arms)]
pub(crate) fn extension(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let v = r.v;
    let s = |x: &str| v.str(x);
    match r.cur.chr {
        ex::LITERAL => {
            // pdfTeX §1353 ("Implement \\pdfliteral")
            let late = r.scan_keyword("shipout")?;
            let mode = if r.scan_keyword("direct")? {
                "direct"
            } else if r.scan_keyword("page")? {
                "page"
            } else {
                "origin"
            };
            let text = if late {
                // expanded when shipped out
                let (_, body) = r.scan_toks(false, false)?;
                crate::val::toks_value(v, &body)
            } else {
                ext_value(r)?
            };
            whatsit(
                r,
                md,
                vec![s("pdf:literal"), s(mode), text, v.int(i64::from(late))],
            );
        }
        ex::COLORSTACK => {
            let n = r.scan_int()?;
            let count = r.get("pdf:colorstacks")?.map_or(0, |x| list_of(v, x).len());
            if n < 0 || usize::try_from(n).unwrap_or(usize::MAX) > count {
                r.log("error", "pdfTeX error (colorstack): invalid identifier")?;
            }
            let (act, text) = if r.scan_keyword("set")? {
                ("set", ext_value(r)?)
            } else if r.scan_keyword("push")? {
                ("push", ext_value(r)?)
            } else if r.scan_keyword("pop")? {
                ("pop", s(""))
            } else if r.scan_keyword("current")? {
                ("current", s(""))
            } else {
                r.log("error", "pdfTeX error (colorstack): action missing")?;
                ("none", s(""))
            };
            whatsit(
                r,
                md,
                vec![s("pdf:colorstack"), v.int(i64::from(n)), s(act), text],
            );
        }
        ex::SETMATRIX => {
            let text = ext_value(r)?;
            whatsit(r, md, vec![s("pdf:setmatrix"), text]);
        }
        ex::SAVE => whatsit(r, md, vec![s("pdf:save")]),
        ex::RESTORE => whatsit(r, md, vec![s("pdf:restore")]),
        ex::OBJ => {
            // pdfTeX §1355: not a node; the object is made now
            let (n, reserved) = obj_num_option(r)?;
            if !reserved {
                let stream = r.scan_keyword("stream")?;
                let at = if stream { attr(r)? } else { s("") };
                let file = r.scan_keyword("file")?;
                let data = ext_value(r)?;
                // PDF-BACKEND-GAP: the object's text, written by
                // phitex-pdf when it supports objects
                let o = v.list(vec![
                    v.int(i64::from(stream)),
                    at,
                    v.int(i64::from(file)),
                    data,
                ]);
                r.set(&format!("pdf:obj:{n}"), o, true);
            }
            r.set("pdf:lastobj", v.int(n), true);
        }
        ex::REFOBJ => {
            let n = r.scan_int()?;
            whatsit(r, md, vec![s("pdf:refobj"), v.int(i64::from(n))]);
        }
        ex::XFORM => {
            // pdfTeX §1357: the box register's contents become a form
            let at = attr(r)?;
            let res = if r.scan_keyword("resources")? {
                ext_value(r)?
            } else {
                s("")
            };
            let k = r.scan_register_num()?;
            let b = r.box_reg(k)?;
            let o = alloc_obj(r)?;
            let fx = r.int("pdf:xformcount")? + 1;
            r.set("pdf:xformcount", v.int(fx), true);
            let bx = b.unwrap_or(ValueId::UNIT);
            let (w, h, d) = b.map_or((0, 0, 0), |b| {
                (
                    nodes::box_dim(v, b, 1),
                    nodes::box_dim(v, b, 3),
                    nodes::box_dim(v, b, 2),
                )
            });
            r.set(
                &format!("pdf:xform:{o}"),
                v.list(vec![
                    bx,
                    at,
                    res,
                    v.int(fx),
                    v.int(i64::from(w)),
                    v.int(i64::from(h)),
                    v.int(i64::from(d)),
                ]),
                true,
            );
            r.modify_box_reg(k, None)?;
            r.set("pdf:lastxform", v.int(o), true);
        }
        ex::REFXFORM => {
            let n = r.scan_int()?;
            let x = r.get(&format!("pdf:xform:{n}"))?;
            let Some(x) = x else {
                return r.log(
                    "error",
                    "pdfTeX error (ext1): cannot find referenced object",
                );
            };
            let g = |k: usize| i32::try_from(crate::val::int_at(v, x, k)).unwrap_or(0);
            whatsit(r, md, vec![s("pdf:refxform"), v.int(i64::from(n))]);
            space_box(r, md, g(4), g(5), g(6))?;
        }
        ex::XIMAGE => ximage(r)?,
        ex::REFXIMAGE => {
            let n = r.scan_int()?;
            let x = r.get(&format!("pdf:ximage:{n}"))?;
            let Some(x) = x else {
                return r.log(
                    "error",
                    "pdfTeX error (ext1): cannot find referenced object",
                );
            };
            let g = |k: usize| i32::try_from(crate::val::int_at(v, x, k)).unwrap_or(0);
            // PDF-BACKEND-GAP: phitex-pdf does not place images; the
            // node carries the object and the box its dimensions
            whatsit(r, md, vec![s("pdf:refximage"), v.int(i64::from(n))]);
            space_box(r, md, g(0), g(1), g(2))?;
        }
        ex::ANNOT => {
            let (n, reserved) = obj_num_option(r)?;
            if !reserved {
                let (w, h, d) = alt_rule(r)?;
                let data = ext_value(r)?;
                // PDF-BACKEND-GAP: annotations
                whatsit(
                    r,
                    md,
                    vec![
                        s("pdf:annot"),
                        v.int(n),
                        v.int(i64::from(w)),
                        v.int(i64::from(h)),
                        v.int(i64::from(d)),
                        data,
                    ],
                );
            }
            r.set("pdf:lastannot", v.int(n), true);
        }
        ex::START_LINK => {
            if md.m.abs() == 1 {
                return r.log(
                    "error",
                    "pdfTeX error (ext1): \\pdfstartlink cannot be used in vertical mode",
                );
            }
            let (w, h, d) = alt_rule(r)?;
            let at = attr(r)?;
            let act = action(r)?;
            let n = alloc_obj(r)?;
            // PDF-BACKEND-GAP: links
            whatsit(
                r,
                md,
                vec![
                    s("pdf:startlink"),
                    v.int(n),
                    v.int(i64::from(w)),
                    v.int(i64::from(h)),
                    v.int(i64::from(d)),
                    at,
                    act,
                ],
            );
            r.set("pdf:lastlink", v.int(n), true);
        }
        ex::END_LINK => {
            if md.m.abs() == 1 {
                return r.log(
                    "error",
                    "pdfTeX error (ext1): \\pdfendlink cannot be used in vertical mode",
                );
            }
            whatsit(r, md, vec![s("pdf:endlink")]);
        }
        ex::OUTLINE => {
            let at = attr(r)?;
            let act = action(r)?;
            let count = if r.scan_keyword("count")? {
                r.scan_int()?
            } else {
                0
            };
            let title = ext_value(r)?;
            // PDF-BACKEND-GAP: outlines
            let e = v.list(vec![at, act, v.int(i64::from(count)), title]);
            append_text(r, "pdf:outlines", e)?;
        }
        ex::DEST => {
            let id = identifier(r)?;
            let kind = if r.scan_keyword("xyz")? {
                if r.scan_keyword("zoom")? {
                    let z = r.scan_int()?;
                    v.list(vec![s("xyz"), v.int(i64::from(z))])
                } else {
                    v.list(vec![s("xyz")])
                }
            } else if r.scan_keyword("fitbh")? {
                v.list(vec![s("fitbh")])
            } else if r.scan_keyword("fitbv")? {
                v.list(vec![s("fitbv")])
            } else if r.scan_keyword("fitb")? {
                v.list(vec![s("fitb")])
            } else if r.scan_keyword("fith")? {
                v.list(vec![s("fith")])
            } else if r.scan_keyword("fitv")? {
                v.list(vec![s("fitv")])
            } else if r.scan_keyword("fitr")? {
                let (w, h, d) = alt_rule(r)?;
                v.list(vec![
                    s("fitr"),
                    v.int(i64::from(w)),
                    v.int(i64::from(h)),
                    v.int(i64::from(d)),
                ])
            } else if r.scan_keyword("fit")? {
                v.list(vec![s("fit")])
            } else {
                r.log("error", "pdfTeX error (ext1): destination type missing")?;
                v.list(vec![s("xyz")])
            };
            // PDF-BACKEND-GAP: destinations
            whatsit(r, md, vec![s("pdf:dest"), id, kind]);
        }
        ex::THREAD | ex::START_THREAD => {
            let (w, h, d) = alt_rule(r)?;
            let at = attr(r)?;
            let id = identifier(r)?;
            let k = if r.cur.chr == ex::THREAD {
                "pdf:thread"
            } else {
                "pdf:startthread"
            };
            // PDF-BACKEND-GAP: threads and beads
            whatsit(
                r,
                md,
                vec![
                    s(k),
                    id,
                    v.int(i64::from(w)),
                    v.int(i64::from(h)),
                    v.int(i64::from(d)),
                    at,
                ],
            );
        }
        ex::END_THREAD => whatsit(r, md, vec![s("pdf:endthread")]),
        ex::SAVE_POS => whatsit(r, md, vec![s("pdf:savepos")]),
        ex::SNAP_REF_POINT => whatsit(r, md, vec![s("pdf:snaprefpoint")]),
        ex::SNAPY => {
            let g = r.scan_glue(2)?;
            whatsit(r, md, vec![s("pdf:snapy"), crate::val::glue_value(v, &g)]);
        }
        ex::SNAPY_COMP => {
            let n = r.scan_int()?.clamp(0, 1000);
            whatsit(r, md, vec![s("pdf:snapycomp"), v.int(i64::from(n))]);
        }
        ex::INFO => {
            let t = ext_value(r)?;
            append_text(r, "pdf:info", t)?;
            let tb = bytes(&str_of(v, t));
            let input = v.list(vec![s("info"), t]);
            crate::pdfout::command(r, input, &[], |st, _| st.info(&tb))?;
        }
        ex::CATALOG => {
            let t = ext_value(r)?;
            append_text(r, "pdf:catalog", t)?;
            // PDF-BACKEND-GAP: `openaction`'s action to the writer
            // (`Action` from the scanned value)
            if r.scan_keyword("openaction")? {
                let a = action(r)?;
                r.set("pdf:openaction", a, true);
            }
            let tb = bytes(&str_of(v, t));
            let input = v.list(vec![s("catalog"), t]);
            crate::pdfout::command(r, input, &[], |st, _| {
                let _ = st.catalog(&tb, None);
            })?;
        }
        ex::NAMES => {
            let t = ext_value(r)?;
            append_text(r, "pdf:names", t)?;
            let tb = bytes(&str_of(v, t));
            let input = v.list(vec![s("names"), t]);
            crate::pdfout::command(r, input, &[], |st, _| st.names(&tb))?;
        }
        ex::TRAILER => {
            let t = ext_value(r)?;
            append_text(r, "pdf:trailer", t)?;
            let tb = bytes(&str_of(v, t));
            let input = v.list(vec![s("trailer"), t]);
            crate::pdfout::command(r, input, &[], |st, _| st.trailer(&tb))?;
        }
        ex::TRAILER_ID => {
            let t = ext_value(r)?;
            r.set("pdf:trailerid", t, true);
            let tb = bytes(&str_of(v, t));
            let input = v.list(vec![s("trailerid"), t]);
            crate::pdfout::command(r, input, &[], |st, _| st.trailer_id(&tb))?;
        }
        ex::INCLUDE_CHARS | ex::FONT_ATTR => {
            let attr = r.cur.chr == ex::FONT_ATTR;
            let f = r.scan_font_ident()?;
            let t = ext_value(r)?;
            let name = if attr {
                format!("pdf:fontattr:{f}")
            } else {
                format!("pdf:includechars:{f}")
            };
            r.set(&name, t, true);
            let fid = u32::try_from(f).unwrap_or(0);
            let tb = bytes(&str_of(v, t));
            let input = pdf_input(r, if attr { "fontattr" } else { "includechars" }, f, t)?;
            crate::pdfout::command(r, input, &[fid], |st, fs| {
                if attr {
                    st.font_attr(fid, &tb, fs);
                } else {
                    let _ = st.include_chars(fid, &tb, fs);
                }
            })?;
        }
        ex::MAP_FILE | ex::MAP_LINE => {
            let t = ext_value(r)?;
            let k = if r.cur.chr == ex::MAP_FILE {
                "file"
            } else {
                "line"
            };
            let e = v.list(vec![s(k), t]);
            append_text(r, "pdf:maps", e)?;
            let input = v.list(vec![s("map"), s(k), t]);
            let tb = bytes(&str_of(v, t));
            let file = k == "file";
            crate::pdfout::command(r, input, &[], |st, fs| {
                if file {
                    st.map_file(&tb, fs);
                } else {
                    st.map_line(&tb);
                }
            })?;
        }
        ex::RESET_TIMER => {}
        ex::SET_RANDOM_SEED => {
            let n = r.scan_int()?;
            let st = Random::new(n);
            put_random(r, &st);
        }
        ex::FONT_EXPAND => {
            // pdfTeX §1365: \pdffontexpand f stretch shrink step [autoexpand]
            let f = r.scan_font_ident()?;
            let st = r.scan_int()?;
            let sh = r.scan_int()?;
            let step = r.scan_int()?;
            let auto = r.scan_keyword("autoexpand")?;
            let e = v.list(vec![
                v.int(i64::from(st)),
                v.int(i64::from(sh)),
                v.int(i64::from(step)),
                v.int(i64::from(auto)),
            ]);
            r.set(&format!("pdf:fontexpand:{f}"), e, true);
        }
        ex::GLYPH_TO_UNICODE => {
            let a = ext_value(r)?;
            let b = ext_value(r)?;
            append_text(r, "pdf:glyphtounicode", v.list(vec![a, b]))?;
            let (ab, bb) = (bytes(&str_of(v, a)), bytes(&str_of(v, b)));
            let input = v.list(vec![v.str("glyphtounicode"), a, b]);
            crate::pdfout::command(r, input, &[], |st, _| st.glyph_to_unicode(&ab, &bb))?;
        }
        ex::NOBUILTIN_TOUNICODE => {
            let f = r.scan_font_ident()?;
            r.set(&format!("pdf:nobuiltintounicode:{f}"), v.int(1), true);
            let fid = u32::try_from(f).unwrap_or(0);
            let input = pdf_input(r, "nobuiltintounicode", f, ValueId::UNIT)?;
            crate::pdfout::command(r, input, &[fid], |st, fs| st.no_builtin_tounicode(fid, fs))?;
        }
        ex::INTERWORD_SPACE_ON => whatsit(r, md, vec![s("pdf:interwordspaceon")]),
        ex::INTERWORD_SPACE_OFF => whatsit(r, md, vec![s("pdf:interwordspaceoff")]),
        ex::FAKE_SPACE => whatsit(r, md, vec![s("pdf:fakespace")]),
        ex::RUNNING_LINK_OFF => whatsit(r, md, vec![s("pdf:runninglinkoff")]),
        ex::RUNNING_LINK_ON => whatsit(r, md, vec![s("pdf:runninglinkon")]),
        ex::SPACE_FONT => {
            let f = r.scan_font_ident()?;
            r.set("pdf:spacefont", v.int(f), true);
        }
        _ => {}
    }
    Ok(())
}

/// `\pdfximage` (pdfTeX §1358, `scale_image` §1359): the image's size
/// from its header.
fn ximage(r: &mut Run<'_, '_>) -> R<()> {
    let v = r.v;
    let (mut w, mut h, mut d) = alt_rule(r)?;
    let at = attr(r)?;
    let page = if r.scan_keyword("page")? {
        r.scan_int()?
    } else {
        1
    };
    let _cs = if r.scan_keyword("colorspace")? {
        r.scan_int()?
    } else {
        0
    };
    for k in ["mediabox", "cropbox", "bleedbox", "trimbox", "artbox"] {
        if r.scan_keyword(k)? {
            break;
        }
    }
    let name = ext_string(r)?;
    let data = file_bytes(r, &name)?.unwrap_or_default();
    let img = image_info(&data, page);
    let running = phitex_typeset::node::RUNNING;
    let (x, y, mut xr, mut yr, pdf, pages, depth) = img;
    let one_inch = 226;
    if (xr > 65535)
        || (yr > 65535)
        || (xr != 0 && yr != 0 && (x / one_inch >= xr || y / one_inch >= yr))
    {
        xr = 0;
        yr = 0;
    }
    let (iw, ih) = if pdf {
        (x, y)
    } else {
        let def = r.param("pdfimageresolution")?.clamp(0, 65535);
        if def > 0 && (xr == 0 || yr == 0) {
            xr = def;
            yr = def;
        }
        if xr > 0 && yr > 0 {
            (
                ext_xn_over_d(ONE_HUNDRED_INCH, x, 100 * xr),
                ext_xn_over_d(ONE_HUNDRED_INCH, y, 100 * yr),
            )
        } else {
            (
                ext_xn_over_d(ONE_HUNDRED_INCH, x, 7200),
                ext_xn_over_d(ONE_HUNDRED_INCH, y, 7200),
            )
        }
    };
    if w == running && h == running && d == running {
        w = iw;
        h = ih;
        d = 0;
    } else if w == running {
        if h == running {
            w = ext_xn_over_d(ih, x, y);
            h = ih - d;
        } else if d == running {
            w = ext_xn_over_d(h, x, y);
            d = 0;
        } else {
            w = ext_xn_over_d(h + d, x, y);
        }
    } else if h == running && d == running {
        h = ext_xn_over_d(w, y, x);
        d = 0;
    } else if h == running {
        h = ext_xn_over_d(w, y, x) - d;
    } else if d == running {
        d = 0;
    }
    let n = alloc_obj(r)?;
    // PDF-BACKEND-GAP: the image's stream, written by phitex-pdf when it
    // supports images
    r.set(
        &format!("pdf:ximage:{n}"),
        v.list(vec![
            v.int(i64::from(w)),
            v.int(i64::from(h)),
            v.int(i64::from(d)),
            v.str(&name),
            v.int(i64::from(page)),
            at,
            v.int(0),
            v.int(0),
            v.int(i64::from(x)),
            v.int(i64::from(y)),
        ]),
        true,
    );
    r.set("pdf:lastximage", v.int(n), true);
    r.set("pdf:lastximagepages", v.int(i64::from(pages)), true);
    r.set("pdf:lastximagecolordepth", v.int(i64::from(depth)), true);
    Ok(())
}

/// `ext_xn_over_d` (utils.c): `x*n/d` in doubles, rounded.
#[allow(clippy::cast_possible_truncation)]
fn ext_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    if d == 0 {
        return 0;
    }
    let mut r = f64::from(x) * f64::from(n) / f64::from(d);
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

/// An image's size (pixels, or scaled points for a PDF page), its
/// resolution, whether it is a PDF, its page count and colour depth.
#[allow(clippy::type_complexity)]
fn image_info(data: &[u8], page: i32) -> (i32, i32, i32, i32, bool, i32, i32) {
    if data.starts_with(b"\x89PNG")
        && let Ok(i) = phitex_codecs::png::read_info(data)
    {
        {
            let (xr, yr) = match i.phys {
                // pixels per metre: dots per inch, rounded as pdfTeX's
                // writepng does
                Some((x, y, 1)) => (
                    i32::try_from((u64::from(x) * 254 + 5000) / 10000).unwrap_or(0),
                    i32::try_from((u64::from(y) * 254 + 5000) / 10000).unwrap_or(0),
                ),
                _ => (0, 0),
            };
            return (
                i32::try_from(i.width).unwrap_or(0),
                i32::try_from(i.height).unwrap_or(0),
                xr,
                yr,
                false,
                1,
                i32::from(i.bit_depth),
            );
        }
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return jpeg_info(data);
    }
    if data.starts_with(b"%PDF") {
        let arc: std::sync::Arc<[u8]> = std::sync::Arc::from(data);
        if let Ok(doc) = phitex_codecs::pdfread::Doc::open(&arc) {
            let n = doc.num_pages();
            let k = usize::try_from(page.max(1) - 1).unwrap_or(0);
            if let Some(p) = doc.page(k) {
                let m = p.media;
                let sp = |bp: f64| -> i32 {
                    #[allow(clippy::cast_possible_truncation)]
                    let x = (bp * 65781.76 + 0.5).floor() as i32;
                    x
                };
                return (
                    sp(m[2] - m[0]),
                    sp(m[3] - m[1]),
                    0,
                    0,
                    true,
                    i32::try_from(n).unwrap_or(0),
                    0,
                );
            }
        }
    }
    (0, 0, 0, 0, false, 0, 0)
}

/// A JPEG's size from its frame header and its resolution from JFIF.
fn jpeg_info(d: &[u8]) -> (i32, i32, i32, i32, bool, i32, i32) {
    let (mut w, mut h, mut xr, mut yr, mut depth) = (0, 0, 0, 0, 8);
    let mut i = 2;
    while i + 4 <= d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        let len = usize::from(d[i + 2]) << 8 | usize::from(d[i + 3]);
        let seg = &d[(i + 4).min(d.len())..(i + 2 + len).min(d.len())];
        match m {
            0xE0 if seg.starts_with(b"JFIF\0") && seg.len() >= 12 => {
                let units = seg[7];
                let xd = i32::from(seg[8]) << 8 | i32::from(seg[9]);
                let yd = i32::from(seg[10]) << 8 | i32::from(seg[11]);
                match units {
                    1 => {
                        xr = xd;
                        yr = yd;
                    }
                    2 => {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            xr = (f64::from(xd) * 2.54 + 0.5) as i32;
                            yr = (f64::from(yd) * 2.54 + 0.5) as i32;
                        }
                    }
                    _ => {}
                }
            }
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF if seg.len() >= 5 => {
                depth = i32::from(seg[0]);
                h = i32::from(seg[1]) << 8 | i32::from(seg[2]);
                w = i32::from(seg[3]) << 8 | i32::from(seg[4]);
                break;
            }
            0xDA | 0xD9 => break,
            _ => {}
        }
        i += 2 + len;
    }
    (w, h, xr, yr, false, 1, depth)
}

// ------------------------------------------------------------ `\shipout`

/// Where each whatsit of a shipped box is (`pdf_hlist_out`,
/// `pdf_vlist_out`: pdfTeX §729–§746), by its handle: `(cur_h, cur_v)`
/// from the page's corner, and the page's height.
pub(crate) struct Positions {
    pub at: std::collections::HashMap<u32, (i32, i32)>,
    pub page_height: i32,
    pub pdf: bool,
}

/// The positions of a page's whatsits (F9c: a fold over the shipped
/// page).
pub(crate) fn positions(r: &mut Run<'_, '_>, b: &BoxNode) -> R<Positions> {
    let pdf = r.param("pdfoutput")? > 0;
    let view = r.font_view(std::slice::from_ref(&Node::Box(b.clone())))?;
    let (h0, v0) = if pdf {
        (
            r.param("pdfhorigin")? + r.param("hoffset")?,
            r.param("pdfvorigin")? + r.param("voffset")?,
        )
    } else {
        (r.param("hoffset")?, r.param("voffset")?)
    };
    let ph = r.param("pdfpageheight")?;
    let page_height = if pdf {
        if ph != 0 {
            ph
        } else {
            b.height + b.depth + 2 * v0
        }
    } else {
        b.height + b.depth + 2 * v0 + 2 * 4_736_286
    };
    let mut at = std::collections::HashMap::new();
    let width = |f: u32, c: u16| view.char_dims(f, u32::from(c)).map_or(0, |d| d.width);
    let mut w = Walk {
        at: &mut at,
        width: &width,
    };
    let cur_v = b.height + v0;
    if b.kind == BoxKind::V {
        w.vlist(b, h0, cur_v - b.height);
    } else {
        w.hlist(b, h0, cur_v);
    }
    Ok(Positions {
        at,
        page_height,
        pdf,
    })
}

struct Walk<'a> {
    at: &'a mut std::collections::HashMap<u32, (i32, i32)>,
    width: &'a dyn Fn(u32, u16) -> i32,
}

/// The glue setting state of a box (§625).
struct GlueRun {
    sign: GlueSign,
    order: Order,
    set: f64,
    cur_g: i32,
    cur_glue: f64,
}

impl GlueRun {
    fn new(b: &BoxNode) -> GlueRun {
        GlueRun {
            sign: b.glue.sign,
            order: b.glue.order,
            set: b.glue.ratio,
            cur_g: 0,
            cur_glue: 0.0,
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn glue(&mut self, g: &GlueNode) -> i32 {
        let spec = g.spec;
        let mut size = spec.width - self.cur_g;
        let round = |t: f64| -> i32 {
            let t = t.clamp(-1_000_000_000.0, 1_000_000_000.0);
            if t >= 0.0 {
                (t + 0.5) as i32
            } else {
                (t - 0.5) as i32
            }
        };
        match self.sign {
            GlueSign::Stretching if spec.stretch_order == self.order => {
                self.cur_glue += f64::from(spec.stretch);
                self.cur_g = round(self.set * self.cur_glue);
            }
            GlueSign::Shrinking if spec.shrink_order == self.order => {
                self.cur_glue -= f64::from(spec.shrink);
                self.cur_g = round(self.set * self.cur_glue);
            }
            _ => {}
        }
        size += self.cur_g;
        size
    }
}

impl Walk<'_> {
    /// `pdf_hlist_out`: `cur_h` from the left edge, on the baseline.
    fn hlist(&mut self, b: &BoxNode, left: i32, base: i32) {
        let mut g = GlueRun::new(b);
        let mut h = left;
        for n in &b.list {
            match n {
                Node::Char { font, ch } => h += (self.width)(*font, *ch),
                Node::Lig(l) => h += (self.width)(l.font, l.ch),
                Node::Box(x) => {
                    if !x.list.is_empty() {
                        if x.kind == BoxKind::V {
                            self.vlist(x, h, base + x.shift - x.height);
                        } else {
                            self.hlist(x, h, base + x.shift);
                        }
                    }
                    h += x.width;
                }
                Node::Rule(x) => h += x.width,
                Node::Whatsit(k) => {
                    self.at.insert(*k, (h, base));
                }
                Node::Glue(x) => h += g.glue(x),
                Node::Kern(w, _) | Node::Math(_, w) => h += *w,
                _ => {}
            }
        }
    }

    /// `pdf_vlist_out`: `cur_v` from the top edge.
    fn vlist(&mut self, b: &BoxNode, left: i32, top: i32) {
        let mut g = GlueRun::new(b);
        let mut v = top;
        for n in &b.list {
            match n {
                Node::Box(x) => {
                    if x.list.is_empty() {
                        v += x.height + x.depth;
                    } else {
                        v += x.height;
                        if x.kind == BoxKind::V {
                            self.vlist(x, left + x.shift, v - x.height);
                        } else {
                            self.hlist(x, left + x.shift, v);
                        }
                        v += x.depth;
                    }
                }
                Node::Rule(x) => {
                    let ht = if x.height == phitex_typeset::node::RUNNING {
                        0
                    } else {
                        x.height
                    };
                    let dp = if x.depth == phitex_typeset::node::RUNNING {
                        0
                    } else {
                        x.depth
                    };
                    v += ht + dp;
                }
                Node::Whatsit(k) => {
                    self.at.insert(*k, (left, v));
                }
                Node::Glue(x) => v += g.glue(x),
                Node::Kern(w, _) => v += *w,
                _ => {}
            }
        }
    }
}

/// A pdfTeX whatsit met by `\shipout` (pdfTeX §1366 ff.): what TeX reads
/// back of it.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn at_shipout(
    r: &mut Run<'_, '_>,
    kind: &str,
    pos: Option<(i32, i32)>,
    p: &Positions,
) -> R<()> {
    let v = r.v;
    if kind == "pdf:savepos" {
        // pdfTeX §1408, and in DVI mode §1407
        let (h, vv) = pos.unwrap_or((0, 0));
        let (x, y) = if p.pdf {
            (h, p.page_height - vv)
        } else {
            (h + 4_736_286, p.page_height - vv - 4_736_286)
        };
        r.set("pdf:lastxpos", v.int(i64::from(x)), true);
        r.set("pdf:lastypos", v.int(i64::from(y)), true);
    }
    Ok(())
}

/// What the PDF writer makes of a pdfTeX whatsit when the page is
/// shipped (`pdf_hlist_out`'s whatsit cases, pdfTeX §1366 ff.): a literal
/// `["lit", mode, text]` (`literal`, pdfTeX §693: 0 `set_origin`, 1
/// `direct_page`, 2 `direct_always`), or a colour stack command
/// `["cs", n, action, text]` whose literal depends on the stacks'
/// state, the writer's fold over the pages.
pub(crate) fn writer_data(r: &mut Run<'_, '_>, xs: &[ValueId]) -> R<Option<ValueId>> {
    let v = r.v;
    let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
    let text = |k: usize| xs.get(k).map(|&t| str_of(v, t)).unwrap_or_default();
    let lit = |mode: i64, t: &str| Some(v.list(vec![v.str("lit"), v.int(mode), v.str(t)]));
    Ok(match kind.as_str() {
        "pdf:literal" => {
            // pdf_out_literal
            let mode = match text(1).as_str() {
                "direct" => 2,
                "page" => 1,
                _ => 0,
            };
            let late = xs.get(3).is_some_and(|&l| int_of(v, l) != 0);
            let t = if late {
                let toks = toks_of(v, xs.get(2).copied());
                r.expand_late(&toks)?
            } else {
                text(2)
            };
            lit(mode, &t)
        }
        // pdf_out_setmatrix (the writer keeps the matrix stack)
        "pdf:setmatrix" => Some(v.list(vec![v.str("mx"), v.str(&text(1))])),
        // pdf_out_save, pdf_out_restore
        "pdf:save" => Some(v.list(vec![v.str("save")])),
        "pdf:restore" => Some(v.list(vec![v.str("restore")])),
        "pdf:colorstack" => Some(v.list(vec![
            v.str("cs"),
            xs.get(1).copied().unwrap_or(ValueId::UNIT),
            v.str(&text(2)),
            v.str(&text(3)),
        ])),
        "pdf:interwordspaceon" => Some(v.list(vec![v.str("iwon")])),
        "pdf:interwordspaceoff" => Some(v.list(vec![v.str("iwoff")])),
        "pdf:fakespace" => Some(v.list(vec![v.str("fake")])),
        "pdf:runninglinkoff" => Some(v.list(vec![v.str("rloff")])),
        "pdf:runninglinkon" => Some(v.list(vec![v.str("rlon")])),
        "pdf:refobj" => backend_gap_object(),
        "pdf:refxform" => backend_gap_form(),
        "pdf:refximage" => backend_gap_image(),
        "pdf:annot" => backend_gap_annot(),
        "pdf:startlink" | "pdf:endlink" => backend_gap_link(),
        "pdf:dest" => backend_gap_dest(),
        "pdf:thread" | "pdf:startthread" | "pdf:endthread" => backend_gap_thread(),
        _ => None,
    })
}

// PDF-BACKEND-GAP: `\pdfrefobj` (pdfTeX §1366 `pdf_append_list` to the
// page's object list): the writer would take the object number and write
// `pdf:obj:<n>` (stream flag, attr, file flag, data) at the end.
fn backend_gap_object() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfrefxform` (`pdf_place_form`): the form's box
// (`pdf:xform:<n>`: box, attr, resources, number, w, h, d) shipped as an
// XObject and placed at the whatsit's position.
fn backend_gap_form() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfrefximage` (`pdf_place_image`): the image
// (`pdf:ximage:<n>`) read and written, placed at the whatsit's position.
fn backend_gap_image() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfannot` (`do_annot`): object number, width,
// height, depth, data; the rectangle from the whatsit's position.
fn backend_gap_annot() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfstartlink`/`\pdfendlink` (`do_link`,
// `end_link`): object number, w, h, d, attr, action; rectangles per line.
fn backend_gap_link() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfdest` (`do_dest`): identifier, kind, position;
// the names tree at the end.
fn backend_gap_dest() -> Option<ValueId> {
    None
}

// PDF-BACKEND-GAP: `\pdfthread`, `\pdfstartthread`, `\pdfendthread`
// (`do_thread`, beads): identifier, w, h, d, attr.
fn backend_gap_thread() -> Option<ValueId> {
    None
}
