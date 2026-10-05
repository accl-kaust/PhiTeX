//! spc_pdfm.c, spc_pdfm.h: dvipdfmx's `pdf:` specials.
//!
//! C's single `struct spc_pdf_ _pdf_stat` is this module's `State`
//! (`self.pdfm`); the `void *dp` of its init/clean is dropped.
//! `pdf_foreach_dict` callbacks are methods taking the key, the value
//! and what C's `dp` really is.

use crate::pdfdev::{PdfRect, TransformInfo};
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

/// `SPC_PDFM_SUPPORT_ANNOT_TRANS`.
pub const SPC_PDFM_SUPPORT_ANNOT_TRANS: bool = true;

/// `STRING_STREAM`, the `type` of `spc_handler_pdfm_stream_with_type`.
pub const STRING_STREAM: i32 = 0;
/// `FILE_STREAM`.
pub const FILE_STREAM: i32 = 1;

/// `struct tounicode`: for the to-UTF16-BE conversion of text strings.
#[derive(Clone, Debug)]
pub struct Tounicode {
    pub cmap_id: i32,
    pub unescape_backslash: i32,
    /// An array of PDF names.
    pub taintkeys: Option<Obj>,
}

impl Default for Tounicode {
    fn default() -> Self {
        Tounicode {
            cmap_id: -1,
            unescape_backslash: 0,
            taintkeys: None,
        }
    }
}

/// `struct spc_pdf_` (`_pdf_stat`).
#[derive(Clone, Debug)]
pub struct State {
    /// Pending annotation dict.
    pub annot_dict: Option<Obj>,
    /// Current min level of outlines.
    pub lowest_level: i32,
    pub cd: Tounicode,
    /// Added to all page resource dicts.
    pub pageresources: Option<Obj>,
}

impl Default for State {
    fn default() -> Self {
        State {
            annot_dict: None,
            lowest_level: 255,
            cd: Tounicode::default(),
            pageresources: None,
        }
    }
}

/// `default_taintkeys` of `spc_handler_pdfm__init`.
pub const DEFAULT_TAINTKEYS: [&[u8]; 11] = [
    b"Title",
    b"Author",
    b"Subject",
    b"Keywords",
    b"Creator",
    b"Producer",
    b"Contents",
    b"Subj",
    b"TU",
    b"T",
    b"TM",
];

/// `calculate_size_utf16` (static): the UTF-16 size of UTF-8 `p`.
fn calculate_size_utf16(p: &[u8]) -> usize {
    todo!()
}

impl Dpx {
    /// `spc_pdfm_at_begin_document`.
    pub fn spc_pdfm_at_begin_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_pdfm_at_end_document`.
    pub fn spc_pdfm_at_end_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_pdfm_at_end_page`: adds `pageresources` to the page.
    pub fn spc_pdfm_at_end_page(&mut self) -> i32 {
        todo!()
    }
}

/// `parse_pdf_reference` (static): the parser's `@name` callback.
fn parse_pdf_reference(dpx: &mut Dpx, s: &[u8], pp: &mut usize) -> Option<Obj> {
    todo!()
}

/// `spc_handler_pdfm__init`.
fn spc_handler_pdfm__init(dpx: &mut Dpx) -> i32 {
    todo!()
}

/// `spc_handler_pdfm__clean`.
fn spc_handler_pdfm__clean(dpx: &mut Dpx) -> i32 {
    todo!()
}

/// `safeputresdent` (foreach callback; `dp` a dict).
fn safeputresdent(dpx: &mut Dpx, kp: Obj, vp: Obj, dp: Obj) -> i32 {
    todo!()
}

/// `safeputresdict` (foreach callback; `dp` a dict).
fn safeputresdict(dpx: &mut Dpx, kp: Obj, vp: Obj, dp: Obj) -> i32 {
    todo!()
}

/// `putpageresources` (foreach callback; `dp` the category name).
fn putpageresources(dpx: &mut Dpx, kp: Obj, vp: Obj, category: &[u8]) -> i32 {
    todo!()
}

/// `forallresourcecategory` (foreach callback; `dp` unused).
fn forallresourcecategory(dpx: &mut Dpx, kp: Obj, vp: Obj) -> i32 {
    todo!()
}

/// `reencode_string_from_utf8_to_utf16be`.
fn reencode_string_from_utf8_to_utf16be(dpx: &mut Dpx, instring: Obj) -> i32 {
    todo!()
}

/// `reencode_string`: `cmap_id` is the CMap cache id (C's `CMap *`).
fn reencode_string(dpx: &mut Dpx, cmap_id: i32, instring: Obj) -> i32 {
    todo!()
}

/// `need_reencode`.
fn need_reencode(dpx: &mut Dpx, kp: Obj, vp: Obj, cd: &Tounicode) -> bool {
    todo!()
}

/// `modify_strings` (foreach callback; `dp` the `struct tounicode`).
fn modify_strings(dpx: &mut Dpx, kp: Obj, vp: Obj, cd: Option<&Tounicode>) -> i32 {
    todo!()
}

/// `parse_pdf_dict_with_tounicode`.
fn parse_pdf_dict_with_tounicode(
    dpx: &mut Dpx,
    s: &[u8],
    pp: &mut usize,
    cd: &Tounicode,
) -> Option<Obj> {
    todo!()
}

/// `set_rect_for_annot`.
fn set_rect_for_annot(dpx: &mut Dpx, spe: &mut SpcEnv, ti: TransformInfo) -> PdfRect {
    todo!()
}

/// `spc_handler_pdfm_stream_with_type` (`STRING_STREAM`/`FILE_STREAM`).
fn spc_handler_pdfm_stream_with_type(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
    ty: i32,
) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bop`.
fn spc_handler_pdfm_bop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_eop`.
fn spc_handler_pdfm_eop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_put`.
fn spc_handler_pdfm_put(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_annot`.
fn spc_handler_pdfm_annot(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bann`.
fn spc_handler_pdfm_bann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_eann`.
fn spc_handler_pdfm_eann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_xann`.
fn spc_handler_pdfm_xann(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bcolor`.
fn spc_handler_pdfm_bcolor(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_scolor`.
fn spc_handler_pdfm_scolor(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_ecolor`.
fn spc_handler_pdfm_ecolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_btrans`.
fn spc_handler_pdfm_btrans(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_etrans`.
fn spc_handler_pdfm_etrans(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_outline`.
fn spc_handler_pdfm_outline(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_article`.
fn spc_handler_pdfm_article(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bead`.
fn spc_handler_pdfm_bead(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_image`.
fn spc_handler_pdfm_image(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_dest`.
fn spc_handler_pdfm_dest(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_names`.
fn spc_handler_pdfm_names(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_docinfo`.
fn spc_handler_pdfm_docinfo(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_docview`.
fn spc_handler_pdfm_docview(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_close`.
fn spc_handler_pdfm_close(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_object`.
fn spc_handler_pdfm_object(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_content`.
fn spc_handler_pdfm_content(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_literal`.
fn spc_handler_pdfm_literal(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bcontent`.
fn spc_handler_pdfm_bcontent(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_econtent`.
fn spc_handler_pdfm_econtent(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_code`.
fn spc_handler_pdfm_code(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_do_nothing`.
fn spc_handler_pdfm_do_nothing(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_stream`.
fn spc_handler_pdfm_stream(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_fstream`.
fn spc_handler_pdfm_fstream(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bform`.
fn spc_handler_pdfm_bform(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_eform`.
fn spc_handler_pdfm_eform(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_uxobj`.
fn spc_handler_pdfm_uxobj(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_link`.
fn spc_handler_pdfm_link(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_nolink`.
fn spc_handler_pdfm_nolink(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_pagesize`.
fn spc_handler_pdfm_pagesize(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bgcolor`.
fn spc_handler_pdfm_bgcolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_mapline`.
fn spc_handler_pdfm_mapline(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_mapfile`.
fn spc_handler_pdfm_mapfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_tounicode`.
fn spc_handler_pdfm_tounicode(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_pageresources`.
fn spc_handler_pdfm_pageresources(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_bxgstate`.
fn spc_handler_pdfm_bxgstate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfm_exgstate`.
fn spc_handler_pdfm_exgstate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdft_compat_page`.
fn spc_handler_pdft_compat_page(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}
/// `pdfm_handlers`, in C's order (the first match wins).
pub static PDFM_HANDLERS: [SpcHandler; 88] = [
    SpcHandler {
        key: b"annotation",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"annotate",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"annot",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"ann",
        exec: spc_handler_pdfm_annot,
    },
    SpcHandler {
        key: b"outline",
        exec: spc_handler_pdfm_outline,
    },
    SpcHandler {
        key: b"out",
        exec: spc_handler_pdfm_outline,
    },
    SpcHandler {
        key: b"article",
        exec: spc_handler_pdfm_article,
    },
    SpcHandler {
        key: b"art",
        exec: spc_handler_pdfm_article,
    },
    SpcHandler {
        key: b"bead",
        exec: spc_handler_pdfm_bead,
    },
    SpcHandler {
        key: b"thread",
        exec: spc_handler_pdfm_bead,
    },
    SpcHandler {
        key: b"destination",
        exec: spc_handler_pdfm_dest,
    },
    SpcHandler {
        key: b"dest",
        exec: spc_handler_pdfm_dest,
    },
    SpcHandler {
        key: b"object",
        exec: spc_handler_pdfm_object,
    },
    SpcHandler {
        key: b"obj",
        exec: spc_handler_pdfm_object,
    },
    SpcHandler {
        key: b"docinfo",
        exec: spc_handler_pdfm_docinfo,
    },
    SpcHandler {
        key: b"docview",
        exec: spc_handler_pdfm_docview,
    },
    SpcHandler {
        key: b"content",
        exec: spc_handler_pdfm_content,
    },
    SpcHandler {
        key: b"put",
        exec: spc_handler_pdfm_put,
    },
    SpcHandler {
        key: b"close",
        exec: spc_handler_pdfm_close,
    },
    SpcHandler {
        key: b"bop",
        exec: spc_handler_pdfm_bop,
    },
    SpcHandler {
        key: b"eop",
        exec: spc_handler_pdfm_eop,
    },
    SpcHandler {
        key: b"image",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"img",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"epdf",
        exec: spc_handler_pdfm_image,
    },
    SpcHandler {
        key: b"link",
        exec: spc_handler_pdfm_link,
    },
    SpcHandler {
        key: b"nolink",
        exec: spc_handler_pdfm_nolink,
    },
    SpcHandler {
        key: b"begincolor",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bcolor",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bc",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"setcolor",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"scolor",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"sc",
        exec: spc_handler_pdfm_scolor,
    },
    SpcHandler {
        key: b"endcolor",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"ecolor",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"ec",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"begingray",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bgray",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"bg",
        exec: spc_handler_pdfm_bcolor,
    },
    SpcHandler {
        key: b"endgray",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"egray",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"eg",
        exec: spc_handler_pdfm_ecolor,
    },
    SpcHandler {
        key: b"bgcolor",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bgc",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bbc",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"bbg",
        exec: spc_handler_pdfm_bgcolor,
    },
    SpcHandler {
        key: b"pagesize",
        exec: spc_handler_pdfm_pagesize,
    },
    SpcHandler {
        key: b"bannot",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"beginann",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"bann",
        exec: spc_handler_pdfm_bann,
    },
    SpcHandler {
        key: b"eannot",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"endann",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"eann",
        exec: spc_handler_pdfm_eann,
    },
    SpcHandler {
        key: b"btrans",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"begintransform",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"begintrans",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"bt",
        exec: spc_handler_pdfm_btrans,
    },
    SpcHandler {
        key: b"etrans",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"endtransform",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"endtrans",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"et",
        exec: spc_handler_pdfm_etrans,
    },
    SpcHandler {
        key: b"bform",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"beginxobj",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"bxobj",
        exec: spc_handler_pdfm_bform,
    },
    SpcHandler {
        key: b"eform",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"endxobj",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"exobj",
        exec: spc_handler_pdfm_eform,
    },
    SpcHandler {
        key: b"usexobj",
        exec: spc_handler_pdfm_uxobj,
    },
    SpcHandler {
        key: b"uxobj",
        exec: spc_handler_pdfm_uxobj,
    },
    SpcHandler {
        key: b"tounicode",
        exec: spc_handler_pdfm_tounicode,
    },
    SpcHandler {
        key: b"literal",
        exec: spc_handler_pdfm_literal,
    },
    SpcHandler {
        key: b"stream",
        exec: spc_handler_pdfm_stream,
    },
    SpcHandler {
        key: b"fstream",
        exec: spc_handler_pdfm_fstream,
    },
    SpcHandler {
        key: b"names",
        exec: spc_handler_pdfm_names,
    },
    SpcHandler {
        key: b"mapline",
        exec: spc_handler_pdfm_mapline,
    },
    SpcHandler {
        key: b"mapfile",
        exec: spc_handler_pdfm_mapfile,
    },
    SpcHandler {
        key: b"bcontent",
        exec: spc_handler_pdfm_bcontent,
    },
    SpcHandler {
        key: b"econtent",
        exec: spc_handler_pdfm_econtent,
    },
    SpcHandler {
        key: b"code",
        exec: spc_handler_pdfm_code,
    },
    SpcHandler {
        key: b"minorversion",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"majorversion",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"encrypt",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"pageresources",
        exec: spc_handler_pdfm_pageresources,
    },
    SpcHandler {
        key: b"trailerid",
        exec: spc_handler_pdfm_do_nothing,
    },
    SpcHandler {
        key: b"xannot",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"extendann",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"xann",
        exec: spc_handler_pdfm_xann,
    },
    SpcHandler {
        key: b"bxgstate",
        exec: spc_handler_pdfm_bxgstate,
    },
    SpcHandler {
        key: b"exgstate",
        exec: spc_handler_pdfm_exgstate,
    },
];

/// `pdft_compat_handlers` (`pdf:direct`, `pdf:page`).
pub static PDFT_COMPAT_HANDLERS: [SpcHandler; 2] = [
    SpcHandler {
        key: b"direct",
        exec: spc_handler_pdft_compat_page,
    },
    SpcHandler {
        key: b"page",
        exec: spc_handler_pdft_compat_page,
    },
];

/// `spc_pdfm_check_special`.
pub fn spc_pdfm_check_special(buf: &[u8]) -> bool {
    todo!()
}

/// `spc_pdfm_setup_handler`.
pub fn spc_pdfm_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
