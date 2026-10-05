//! specials.c, specials.h: dispatching `\special`s to the spc_* modules,
//! named references, the coordinate stacks.
//!
//! A handler is a plain `fn(&mut Dpx, &mut SpcEnv, &mut SpcArg) -> i32`
//! ([`SpcHandlerFn`]); each spc_* module has its table of them.
//! `spc_tpic` and `spc_dvips` (PostScript through ghostscript) are not
//! ported. Their entries of `known_specials` are kept, with C's check
//! functions (so they claim the same specials, and in the same order),
//! no hooks, and a setup that fails: such a special is an error (-1),
//! reported and skipped, as a failing handler is in C.

use crate::obj::PdfOut;
use crate::parse::{Unknown, skip_white};
use crate::pdfdev::{PdfCoord, PdfRect, TransformInfo};
use crate::prelude::*;

/// `THEBUFFLENGTH`.
pub const THEBUFFLENGTH: usize = 1024;

/// specials.c's `static` state.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `coords`: the `pdf:bt`/`pdf:et` coordinate stack (top = last).
    pub coords: crate::dpxutil::DpxStack<PdfCoord>,
    /// `pt_fixee`: XeTeX's fixed points.
    pub pt_fixee: crate::dpxutil::DpxStack<PdfCoord>,
}

/// `spc_env.info`.
#[derive(Clone, Debug, Default)]
pub struct SpcEnvInfo {
    pub is_drawable: i32,
    pub rect: PdfRect,
}

/// `struct spc_env`.
#[derive(Clone, Debug, Default)]
pub struct SpcEnv {
    pub x_user: f64,
    pub y_user: f64,
    pub mag: f64,
    /// Current page in PDF.
    pub pg: i32,
    pub info: SpcEnvInfo,
}

/// `struct spc_arg`: owns the special's bytes; C's pointers are indices
/// into `buf` (`base` is where the special starts, 0). Parsers take
/// `(&ap.buf[..ap.endptr], &mut ap.curptr)`.
#[derive(Clone, Debug, Default)]
pub struct SpcArg {
    pub buf: Vec<u8>,
    pub curptr: usize,
    pub endptr: usize,
    pub base: usize,
    /// The command name (C's `const char *command`, always a static
    /// string: a handler table key or a literal).
    pub command: Option<&'static [u8]>,
}

impl SpcArg {
    /// `ap->curptr[0]`: 0 at (or past) the end, where C reads the byte
    /// after the special.
    #[must_use]
    pub fn cur(&self) -> u8 {
        if self.curptr < self.endptr {
            self.buf[self.curptr]
        } else {
            0
        }
    }
    /// The bytes from `curptr` to `endptr`.
    #[must_use]
    pub fn rest(&self) -> &[u8] {
        &self.buf[self.curptr.min(self.endptr)..self.endptr]
    }
    /// `skip_white(&ap->curptr, ap->endptr)` (pdfparse's).
    pub fn skip_white(&mut self) {
        skip_white(&self.buf[..self.endptr], &mut self.curptr);
    }
    /// C's `(&ap->curptr, ap->endptr)`, for the parsers.
    pub fn parts(&mut self) -> (&[u8], &mut usize) {
        (&self.buf[..self.endptr], &mut self.curptr)
    }
}

/// A C string's bytes: up to the first NUL.
#[must_use]
pub fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `spc_handler_fn_ptr`.
pub type SpcHandlerFn = fn(&mut Dpx, &mut SpcEnv, &mut SpcArg) -> i32;

/// `struct spc_handler`. `key` empty = C's NULL.
#[derive(Clone, Copy)]
pub struct SpcHandler {
    pub key: &'static [u8],
    pub exec: SpcHandlerFn,
}

/// A begin/end-of-document/page/form hook (`int (*)(void)`).
pub type SpcHookFn = fn(&mut Dpx) -> i32;
/// `check_func`: does this module take the special?
pub type SpcCheckFn = fn(&[u8]) -> bool;
/// `setup_func`: sets `sph.exec` (and `ap.command`), advances `ap`.
pub type SpcSetupFn = fn(&mut Dpx, &mut SpcHandler, &mut SpcEnv, &mut SpcArg) -> i32;

/// An entry of `known_specials`.
#[derive(Clone, Copy)]
pub struct KnownSpecial {
    pub key: &'static [u8],
    pub bodhk_func: Option<SpcHookFn>,
    pub eodhk_func: Option<SpcHookFn>,
    pub bophk_func: Option<SpcHookFn>,
    pub eophk_func: Option<SpcHookFn>,
    pub bofhk_func: Option<SpcHookFn>,
    pub eofhk_func: Option<SpcHookFn>,
    pub check_func: SpcCheckFn,
    pub setup_func: SpcSetupFn,
}

/// `known_specials`, in C's order (the first that checks wins).
pub static KNOWN_SPECIALS: [KnownSpecial; 8] = [
    KnownSpecial {
        key: b"pdf:",
        bodhk_func: Some(Dpx::spc_pdfm_at_begin_document),
        eodhk_func: Some(Dpx::spc_pdfm_at_end_document),
        bophk_func: None,
        eophk_func: Some(Dpx::spc_pdfm_at_end_page),
        bofhk_func: None,
        eofhk_func: None,
        check_func: crate::spc_pdfm::spc_pdfm_check_special,
        setup_func: crate::spc_pdfm::spc_pdfm_setup_handler,
    },
    KnownSpecial {
        key: b"x:",
        bodhk_func: None,
        eodhk_func: None,
        bophk_func: None,
        eophk_func: None,
        bofhk_func: None,
        eofhk_func: None,
        check_func: crate::spc_xtx::spc_xtx_check_special,
        setup_func: crate::spc_xtx::spc_xtx_setup_handler,
    },
    KnownSpecial {
        key: b"dvipdfmx:",
        bodhk_func: None,
        eodhk_func: None,
        bophk_func: None,
        eophk_func: None,
        bofhk_func: None,
        eofhk_func: None,
        check_func: crate::spc_dvipdfmx::spc_dvipdfmx_check_special,
        setup_func: crate::spc_dvipdfmx::spc_dvipdfmx_setup_handler,
    },
    // spc_dvips.c is not ported (ghostscript): C's check, no hooks, and
    // a setup that fails.
    KnownSpecial {
        key: b"ps:",
        bodhk_func: None,
        eodhk_func: None,
        bophk_func: None,
        eophk_func: None,
        bofhk_func: None,
        eofhk_func: None,
        check_func: spc_dvips_check_special,
        setup_func: spc_unsupported_setup_handler,
    },
    KnownSpecial {
        key: b"color",
        bodhk_func: None,
        eodhk_func: None,
        bophk_func: None,
        eophk_func: None,
        bofhk_func: None,
        eofhk_func: None,
        check_func: crate::spc_color::spc_color_check_special,
        setup_func: crate::spc_color::spc_color_setup_handler,
    },
    // spc_tpic.c is not ported: C's check, no hooks, a setup that fails.
    KnownSpecial {
        key: b"tpic",
        bodhk_func: None,
        eodhk_func: None,
        bophk_func: None,
        eophk_func: None,
        bofhk_func: None,
        eofhk_func: None,
        check_func: spc_tpic_check_special,
        setup_func: spc_unsupported_setup_handler,
    },
    KnownSpecial {
        key: b"html:",
        bodhk_func: Some(Dpx::spc_html_at_begin_document),
        eodhk_func: Some(Dpx::spc_html_at_end_document),
        bophk_func: Some(Dpx::spc_html_at_begin_page),
        eophk_func: Some(Dpx::spc_html_at_end_page),
        bofhk_func: None,
        eofhk_func: None,
        check_func: crate::spc_html::spc_html_check_special,
        setup_func: crate::spc_html::spc_html_setup_handler,
    },
    KnownSpecial {
        key: b"compat",
        bodhk_func: Some(Dpx::spc_misc_at_begin_document),
        eodhk_func: Some(Dpx::spc_misc_at_end_document),
        bophk_func: Some(Dpx::spc_misc_at_begin_page),
        eophk_func: None,
        bofhk_func: Some(Dpx::spc_misc_at_begin_form),
        eofhk_func: Some(Dpx::spc_misc_at_end_form),
        check_func: crate::spc_misc::spc_misc_check_special,
        setup_func: crate::spc_misc::spc_misc_setup_handler,
    },
];

/// spc_dvips.c's `dvips_handlers` keys (for its check).
const DVIPS_KEYS: [&[u8]; 10] = [
    b"header",
    b"PSfile",
    b"psfile",
    b"ps: plotfile ",
    b"PS: plotfile ",
    b"PS:",
    b"ps:",
    b"PST:",
    b"pst:",
    b"\" ",
];

/// `spc_dvips_check_special` (spc_dvips.c).
fn spc_dvips_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    skip_white(buf, &mut p);
    if p >= buf.len() {
        return false;
    }
    let rest = &buf[p..];
    DVIPS_KEYS.iter().any(|k| rest.starts_with(k))
}

/// spc_tpic.c's `tpic_handlers` keys (for its check).
const TPIC_KEYS: [&[u8]; 13] = [
    b"pn", b"pa", b"fp", b"ip", b"da", b"dt", b"sp", b"ar", b"ia", b"sh", b"wh", b"bk", b"tx",
];

/// `spc_tpic_check_special` (spc_tpic.c, `ENABLE_SPC_NAMESPACE`; its
/// `skip_blank` skips spaces and tabs).
fn spc_tpic_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    while p < buf.len() && (buf[p] == b' ' || buf[p] == b'\t') {
        p += 1;
    }
    let mut hasnsp = false;
    if p + 5 < buf.len() && &buf[p..p + 5] == b"tpic:" {
        p += 5;
        hasnsp = true;
    }
    match crate::dpxutil::parse_c_ident(buf, &mut p) {
        None => false,
        // `__setopt__` is a special only with DEBUG.
        Some(q) if hasnsp && q == b"__setopt__" => false,
        Some(q) => TPIC_KEYS.iter().any(|k| q == *k),
    }
}

/// The setup of a module that is not ported (spc_dvips, spc_tpic): the
/// special fails.
fn spc_unsupported_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    -1
}

/// `_rkeys`: reserved names of `spc_lookup_reference`/`spc_lookup_object`.
pub const RKEYS: [&[u8]; 10] = [
    b"xpos",
    b"ypos",
    b"thispage",
    b"prevpage",
    b"nextpage",
    b"resources",
    b"pages",
    b"names",
    b"catalog",
    b"docinfo",
];
pub const K_OBJ__XPOS: usize = 0;
pub const K_OBJ__YPOS: usize = 1;
pub const K_OBJ__THISPAGE: usize = 2;
pub const K_OBJ__PREVPAGE: usize = 3;
pub const K_OBJ__NEXTPAGE: usize = 4;
pub const K_OBJ__RESOURCES: usize = 5;
pub const K_OBJ__PAGES: usize = 6;
pub const K_OBJ__NAMES: usize = 7;
pub const K_OBJ__CATALOG: usize = 8;
pub const K_OBJ__DOCINFO: usize = 9;

/// `ispageref`: `pageN`, N a positive integer.
fn ispageref(key: &[u8]) -> bool {
    if key.len() <= 4 || &key[..4] != b"page" {
        return false;
    }
    key[4..].iter().all(u8::is_ascii_digit)
}

/// `spc_handler_unknown`.
fn spc_handler_unknown(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    args.curptr = args.endptr;
    -1
}

/// `check_garbage`.
fn check_garbage(args: &mut SpcArg) {
    if args.curptr >= args.endptr {
        return;
    }
    args.skip_white();
    if args.curptr < args.endptr {
        crate::warn!("Unparsed material at end of special ignored.");
    }
}

/// A parser's handler of unknown tokens that needs the whole program
/// (spc_pdfm's and spc_misc's `parse_pdf_reference`).
pub type DpxUnknown = fn(&mut Dpx, &[u8], &mut usize) -> Option<Obj>;

impl Dpx {
    /// Runs `f` on the objects with an `Unknown` handler that calls
    /// `unknown` on the whole program: `self.o` is lent to `f`, and
    /// swapped back into the program (with what `take` left) around
    /// each call of `unknown`.
    pub fn with_dpx_unknown<R>(
        &mut self,
        unknown: DpxUnknown,
        f: impl FnOnce(&mut PdfOut, Unknown<'_>) -> R,
    ) -> R {
        let mut o = core::mem::take(&mut self.o);
        let r = {
            let mut cb = |o2: &mut PdfOut, s: &[u8], pp: &mut usize| {
                core::mem::swap(&mut self.o, o2);
                let r = unknown(self, s, pp);
                core::mem::swap(&mut self.o, o2);
                r
            };
            f(&mut o, &mut cb)
        };
        self.o = o;
        r
    }

    /// `parse_pdf_object_extended(pp, endptr, NULL, unknown, …)`.
    pub fn parse_pdf_object_extended(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        unknown: DpxUnknown,
    ) -> Option<Obj> {
        self.with_dpx_unknown(unknown, |o, u| o.parse_pdf_object_ext(s, pp, None, Some(u)))
    }

    /// `parse_pdf_tainted_dict(pp, endptr, unknown, …)`.
    pub fn parse_pdf_tainted_dict_extended(
        &mut self,
        s: &[u8],
        pp: &mut usize,
        unknown: DpxUnknown,
    ) -> Option<Obj> {
        self.with_dpx_unknown(unknown, |o, u| o.parse_pdf_tainted_dict(s, pp, Some(u)))
    }

    /// `spc_warn` (the message preformatted).
    pub fn spc_warn(&mut self, spe: &SpcEnv, msg: core::fmt::Arguments<'_>) {
        crate::warn!("{}", msg);
    }

    /// The device position (`dvi_dev_xpos`, `dvi_dev_ypos`) in PDF
    /// space, rounded to 0.01: `@xpos`, `@ypos`.
    fn spc_dev_pos(&mut self) -> PdfCoord {
        let mut cp = PdfCoord {
            x: self.dvi_dev_xpos(),
            y: self.dvi_dev_ypos(),
        };
        self.pdf_dev_transform(&mut cp, None);
        cp
    }

    /// `spc_lookup_reference`: a reference (new link) for a named object
    /// or a reserved name (`@thispage`, `@xpos`…).
    pub fn spc_lookup_reference(&mut self, ident: &[u8]) -> Option<Obj> {
        let key = cstr(ident);
        let k = RKEYS.iter().position(|r| *r == key).unwrap_or(RKEYS.len());
        let value = match k {
            // xpos and ypos must be position in device space here.
            K_OBJ__XPOS => {
                let cp = self.spc_dev_pos();
                Some(self.o.new_number(crate::fmt::round_acc(cp.x, 0.01)))
            }
            K_OBJ__YPOS => {
                let cp = self.spc_dev_pos();
                Some(self.o.new_number(crate::fmt::round_acc(cp.y, 0.01)))
            }
            K_OBJ__THISPAGE => self.pdf_doc_this_page_ref(),
            K_OBJ__PREVPAGE => self.pdf_doc_prev_page_ref(),
            K_OBJ__NEXTPAGE => self.pdf_doc_next_page_ref(),
            K_OBJ__PAGES => {
                let o = self.pdf_doc_page_tree();
                Some(self.o.ref_obj(o))
            }
            K_OBJ__NAMES => {
                let o = self.pdf_doc_names();
                Some(self.o.ref_obj(o))
            }
            K_OBJ__RESOURCES => {
                let o = self
                    .pdf_doc_current_page_resources()
                    .expect("page resources");
                Some(self.o.ref_obj(o))
            }
            K_OBJ__CATALOG => {
                let o = self.pdf_doc_catalog();
                Some(self.o.ref_obj(o))
            }
            K_OBJ__DOCINFO => {
                let o = self.pdf_doc_docinfo();
                Some(self.o.ref_obj(o))
            }
            _ => {
                if ispageref(key) {
                    let n = crate::fmt::atoi(&key[4..]) as i32;
                    self.pdf_doc_ref_page(n as u32)
                } else {
                    let names = self.doc.global_names.as_mut().expect("global_names");
                    self.o.pdf_names_lookup_reference(names, key)
                }
            }
        };
        if value.is_none() {
            crate::error!(
                "Object reference {} not exist.",
                String::from_utf8_lossy(key)
            );
        }
        value
    }

    /// `spc_lookup_object`: the object itself (not linked).
    pub fn spc_lookup_object(&mut self, ident: &[u8]) -> Option<Obj> {
        let key = cstr(ident);
        let k = RKEYS.iter().position(|r| *r == key).unwrap_or(RKEYS.len());
        match k {
            K_OBJ__XPOS => {
                let cp = self.spc_dev_pos();
                Some(self.o.new_number(crate::fmt::round_acc(cp.x, 0.01)))
            }
            K_OBJ__YPOS => {
                let cp = self.spc_dev_pos();
                Some(self.o.new_number(crate::fmt::round_acc(cp.y, 0.01)))
            }
            K_OBJ__THISPAGE => Some(self.pdf_doc_this_page()),
            K_OBJ__PAGES => Some(self.pdf_doc_page_tree()),
            K_OBJ__NAMES => Some(self.pdf_doc_names()),
            K_OBJ__RESOURCES => self.pdf_doc_current_page_resources(),
            K_OBJ__CATALOG => Some(self.pdf_doc_catalog()),
            K_OBJ__DOCINFO => Some(self.pdf_doc_docinfo()),
            // prevpage and nextpage are names here, as in C.
            _ => {
                let names = self.doc.global_names.as_mut().expect("global_names");
                self.o.pdf_names_lookup_object(names, key)
            }
        }
    }

    /// `spc_begin_annot`.
    pub fn spc_begin_annot(&mut self, spe: &mut SpcEnv, annot_dict: Obj) -> i32 {
        self.pdf_doc_begin_annot(annot_dict);
        // Tell dvi interpreter to handle line-break.
        self.dvi_tag_depth();
        0
    }
    /// `spc_end_annot`.
    pub fn spc_end_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        self.dvi_untag_depth();
        self.pdf_doc_end_annot();
        0
    }
    /// `spc_resume_annot`.
    pub fn spc_resume_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        self.dvi_link_annot(1);
        0
    }
    /// `spc_suspend_annot`.
    pub fn spc_suspend_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        self.dvi_link_annot(0);
        0
    }

    /// `spc_begin_form`.
    pub fn spc_begin_form(
        &mut self,
        spe: &mut SpcEnv,
        ident: &[u8],
        cp: PdfCoord,
        cropbox: &PdfRect,
    ) -> i32 {
        let mut error = 0;
        let xobj_id = self.pdf_doc_begin_grabbing(ident, cp.x, cp.y, cropbox);
        if xobj_id < 0 {
            error = -1;
        } else {
            for ks in &KNOWN_SPECIALS {
                if let Some(f) = ks.bofhk_func {
                    error = f(self);
                }
            }
        }
        error
    }
    /// `spc_end_form`.
    pub fn spc_end_form(&mut self, spe: &mut SpcEnv, attr: Option<Obj>) -> i32 {
        let mut error = 0;
        self.pdf_doc_end_grabbing(attr);
        for ks in &KNOWN_SPECIALS {
            if let Some(f) = ks.eofhk_func {
                error = f(self);
            }
        }
        error
    }

    /// `spc_is_tracking_boxes`.
    pub fn spc_is_tracking_boxes(&mut self, spe: &mut SpcEnv) -> bool {
        self.dvi_is_tracking_boxes()
    }
    /// `spc_set_linkmode`: 0 normal, 1 capture phantom texts.
    pub fn spc_set_linkmode(&mut self, spe: &mut SpcEnv, mode: i32) {
        self.dvi_set_linkmode(mode);
    }
    /// `spc_set_phantom`.
    pub fn spc_set_phantom(&mut self, spe: &mut SpcEnv, height: f64, depth: f64) {
        self.dvi_set_phantom_height(height, depth);
    }

    /// `spc_push_object`: to `global_names` (takes `value`).
    pub fn spc_push_object(&mut self, spe: &mut SpcEnv, key: &[u8], value: Obj) {
        let key = cstr(key);
        let names = self.doc.global_names.as_mut().expect("global_names");
        self.o.pdf_names_add_object(names, key, value);
    }
    /// `spc_flush_object`.
    pub fn spc_flush_object(&mut self, spe: &mut SpcEnv, key: &[u8]) {
        let key = cstr(key);
        let names = self.doc.global_names.as_mut().expect("global_names");
        self.o.pdf_names_close_object(names, key);
    }
    /// `spc_clear_objects` (does nothing).
    pub fn spc_clear_objects(&mut self, spe: &mut SpcEnv) {}

    /// `spc_put_image`.
    pub fn spc_put_image(
        &mut self,
        spe: &mut SpcEnv,
        res_id: i32,
        ti: &mut TransformInfo,
        xpos: f64,
        ypos: f64,
    ) {
        let (xoff, yoff) = self.spc_get_coord(spe);
        let (_, rect) = self.pdf_dev_put_image(res_id, ti, xpos - xoff, ypos - yoff);
        spe.info.rect = rect;
        spe.info.is_drawable = 1;
    }
    /// `spc_get_current_point`.
    pub fn spc_get_current_point(&mut self, spe: &mut SpcEnv) -> PdfCoord {
        let (xoff, yoff) = self.spc_get_coord(spe);
        PdfCoord {
            x: spe.x_user - xoff,
            y: spe.y_user - yoff,
        }
    }

    /// `spc_get_coord`: the top of `coords`, or (0, 0).
    pub fn spc_get_coord(&mut self, spe: &mut SpcEnv) -> (f64, f64) {
        match self.spc.coords.dpx_stack_top() {
            Some(p) => (p.x, p.y),
            None => (0.0, 0.0),
        }
    }
    /// `spc_push_coord`.
    pub fn spc_push_coord(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        self.spc.coords.dpx_stack_push(PdfCoord { x, y });
        self.dvi_set_compensation(x, y);
    }
    /// `spc_pop_coord`.
    pub fn spc_pop_coord(&mut self, spe: &mut SpcEnv) {
        self.spc.coords.dpx_stack_pop();
        let (x, y) = self.spc_get_coord(spe);
        self.dvi_set_compensation(x, y);
    }
    /// `spc_set_fixed_point`.
    pub fn spc_set_fixed_point(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        if let Some(p) = self.spc.pt_fixee.dpx_stack_top_mut() {
            p.x = x;
            p.y = y;
        } else {
            self.spc.pt_fixee.dpx_stack_push(PdfCoord { x, y });
        }
    }
    /// `spc_get_fixed_point`.
    pub fn spc_get_fixed_point(&mut self, spe: &mut SpcEnv) -> (f64, f64) {
        match self.spc.pt_fixee.dpx_stack_top() {
            Some(p) => (p.x, p.y),
            None => (0.0, 0.0),
        }
    }
    /// `spc_put_fixed_point`.
    pub fn spc_put_fixed_point(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        self.spc.pt_fixee.dpx_stack_push(PdfCoord { x, y });
    }
    /// `spc_dup_fixed_point`.
    pub fn spc_dup_fixed_point(&mut self, spe: &mut SpcEnv) {
        let p2 = match self.spc.pt_fixee.dpx_stack_top() {
            Some(p1) => *p1,
            None => PdfCoord { x: 0.0, y: 0.0 },
        };
        self.spc.pt_fixee.dpx_stack_push(p2);
    }
    /// `spc_pop_fixed_point`.
    pub fn spc_pop_fixed_point(&mut self, spe: &mut SpcEnv) {
        self.spc.pt_fixee.dpx_stack_pop();
    }
    /// `spc_clear_fixed_point`.
    pub fn spc_clear_fixed_point(&mut self, spe: &mut SpcEnv) {
        while self.spc.pt_fixee.dpx_stack_pop().is_some() {}
    }

    /// `spc_exec_at_begin_page`.
    pub fn spc_exec_at_begin_page(&mut self) -> i32 {
        let mut error = 0;
        for ks in &KNOWN_SPECIALS {
            if let Some(f) = ks.bophk_func {
                error = f(self);
            }
        }
        error
    }
    /// `spc_exec_at_end_page`.
    pub fn spc_exec_at_end_page(&mut self) -> i32 {
        let mut error = 0;
        for ks in &KNOWN_SPECIALS {
            if let Some(f) = ks.eophk_func {
                error = f(self);
            }
        }
        error
    }
    /// `spc_exec_at_begin_document`.
    pub fn spc_exec_at_begin_document(&mut self) -> i32 {
        let mut error = 0;
        for ks in &KNOWN_SPECIALS {
            if let Some(f) = ks.bodhk_func {
                error = f(self);
            }
        }
        self.spc.coords = crate::dpxutil::DpxStack::dpx_stack_init();
        self.spc.pt_fixee = crate::dpxutil::DpxStack::dpx_stack_init();
        error
    }
    /// `spc_exec_at_end_document`.
    pub fn spc_exec_at_end_document(&mut self) -> i32 {
        let mut error = 0;
        for ks in &KNOWN_SPECIALS {
            if let Some(f) = ks.eodhk_func {
                error = f(self);
            }
        }
        while self.spc.coords.dpx_stack_pop().is_some() {}
        while self.spc.pt_fixee.dpx_stack_pop().is_some() {}
        error
    }

    /// `spc_exec_special`: error, and C's out-parameters `is_drawable`
    /// and `rect` (set only when no error; else 0 and an empty rect, the
    /// values dvi.c initializes them to).
    pub fn spc_exec_special(
        &mut self,
        buffer: &[u8],
        x_user: f64,
        y_user: f64,
        mag: f64,
    ) -> (i32, i32, PdfRect) {
        let mut error = -1;
        let mut is_drawable = 0;
        let mut rect = PdfRect::default();

        let (mut special, mut spe, mut args) = init_special(self, buffer, x_user, y_user, mag);

        for ks in &KNOWN_SPECIALS {
            let found = (ks.check_func)(buffer);
            if found {
                error = (ks.setup_func)(self, &mut special, &mut spe, &mut args);
                if error == 0 {
                    error = (special.exec)(self, &mut spe, &mut args);
                }
                if error != 0 {
                    print_error(self, ks.key, &mut spe, &mut args);
                } else {
                    is_drawable = spe.info.is_drawable;
                    rect = spe.info.rect;
                }
                break;
            }
        }

        check_garbage(&mut args);

        (error, is_drawable, rect)
    }
}

/// `init_special`: the handler (`spc_handler_unknown`), env and arg.
fn init_special(
    dpx: &mut Dpx,
    p: &[u8],
    x_user: f64,
    y_user: f64,
    mag: f64,
) -> (SpcHandler, SpcEnv, SpcArg) {
    let special = SpcHandler {
        key: b"",
        exec: spc_handler_unknown,
    };
    let spe = SpcEnv {
        x_user,
        y_user,
        mag,
        pg: dpx.pdf_doc_current_page_number(), // _FIXME_
        info: SpcEnvInfo {
            is_drawable: 0,
            rect: PdfRect {
                llx: 0.0,
                lly: 0.0,
                urx: 0.0,
                ury: 0.0,
            },
        },
    };
    let args = SpcArg {
        buf: p.to_vec(),
        curptr: 0,
        endptr: p.len(),
        base: 0,
        command: None,
    };
    (special, spe, args)
}

/// `print_error`'s `ebuf`: at most 63 bytes from `from`, non-printables
/// as `\xNN`, dots at the end when the special was not read through.
fn error_buf(ap: &SpcArg, from: usize) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut ebuf = Vec::new();
    let mut p = from;
    while ebuf.len() < 63 && p < ap.endptr {
        let c = ap.buf[p];
        if (0x20..0x7f).contains(&c) {
            ebuf.push(c);
        } else if ebuf.len() + 4 < 63 {
            ebuf.extend_from_slice(&[b'\\', b'x', HEX[(c >> 4) as usize], HEX[(c & 15) as usize]]);
        } else {
            break;
        }
        p += 1;
    }
    if ap.curptr < ap.endptr {
        let mut i = ebuf.len();
        while i > 60 {
            i -= 1;
            ebuf[i] = b'.';
        }
    }
    ebuf
}

/// `print_error`.
fn print_error(dpx: &mut Dpx, name: &[u8], spe: &mut SpcEnv, ap: &mut SpcArg) {
    let pg = spe.pg;
    let mut c = PdfCoord {
        x: spe.x_user,
        y: spe.y_user,
    };
    dpx.pdf_dev_transform(&mut c, None);

    if let Some(command) = ap.command {
        crate::warn!(
            "Interpreting special command {:?} ({:?}) failed.",
            command,
            name
        );
        crate::warn!(
            ">> at page=\"{}\" position=\"({}, {})\" (in PDF)",
            pg,
            c.x,
            c.y
        );
    }
    let ebuf = error_buf(ap, ap.base);
    crate::warn!(">> xxx \"{:?}\"", ebuf);

    if ap.curptr < ap.endptr {
        let ebuf = error_buf(ap, ap.curptr);
        crate::warn!(">> Reading special command stopped around >>{:?}<<", ebuf);
        ap.curptr = ap.endptr;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pageref() {
        assert!(ispageref(b"page1"));
        assert!(ispageref(b"page12"));
        assert!(!ispageref(b"page"));
        assert!(!ispageref(b"pages"));
        assert!(!ispageref(b"page1a"));
        assert!(!ispageref(b"thispage"));
    }

    #[test]
    fn dvips_check() {
        assert!(spc_dvips_check_special(b"  ps: 1 0 0 setrgbcolor"));
        assert!(spc_dvips_check_special(b"PSfile=foo.eps"));
        assert!(spc_dvips_check_special(b"\" newpath"));
        assert!(!spc_dvips_check_special(b"pdf:literal"));
        assert!(!spc_dvips_check_special(b"   "));
    }

    #[test]
    fn error_buf_cut() {
        let ap = SpcArg {
            buf: b"pdf:x\x01y".to_vec(),
            curptr: 8,
            endptr: 7,
            base: 0,
            command: None,
        };
        assert_eq!(error_buf(&ap, 0), b"pdf:x\\x01y".to_vec());
        let ap = SpcArg {
            buf: vec![b'a'; 100],
            curptr: 0,
            endptr: 100,
            base: 0,
            command: None,
        };
        let mut want = vec![b'a'; 60];
        want.extend_from_slice(b"...");
        assert_eq!(error_buf(&ap, 0), want);
    }
}
