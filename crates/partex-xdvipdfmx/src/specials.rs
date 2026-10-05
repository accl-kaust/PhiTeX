//! specials.c, specials.h: dispatching `\special`s to the spc_* modules,
//! named references, the coordinate stacks.
//!
//! A handler is a plain `fn(&mut Dpx, &mut SpcEnv, &mut SpcArg) -> i32`
//! ([`SpcHandlerFn`]); each spc_* module has its table of them.
//! `spc_tpic`, `spc_dvips` (and mpost) are not ported: their entries of
//! `known_specials` are left out (comments in [`KNOWN_SPECIALS`]).

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
pub static KNOWN_SPECIALS: [KnownSpecial; 6] = [
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
    // {"ps:", spc_dvips_*}: spc_dvips.c is not ported.
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
    // {"tpic", spc_tpic_*}: spc_tpic.c is not ported.
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
    todo!()
}

/// `spc_handler_unknown`.
fn spc_handler_unknown(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `check_garbage`.
fn check_garbage(args: &mut SpcArg) {
    todo!()
}

impl Dpx {
    /// `spc_warn` (the message preformatted).
    pub fn spc_warn(&mut self, spe: &SpcEnv, msg: core::fmt::Arguments<'_>) {
        todo!()
    }

    /// `spc_lookup_reference`: a reference (new link) for a named object
    /// or a reserved name (`@thispage`, `@xpos`…).
    pub fn spc_lookup_reference(&mut self, ident: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `spc_lookup_object`: the object itself (not linked).
    pub fn spc_lookup_object(&mut self, ident: &[u8]) -> Option<Obj> {
        todo!()
    }

    /// `spc_begin_annot`.
    pub fn spc_begin_annot(&mut self, spe: &mut SpcEnv, annot_dict: Obj) -> i32 {
        todo!()
    }
    /// `spc_end_annot`.
    pub fn spc_end_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        todo!()
    }
    /// `spc_resume_annot`.
    pub fn spc_resume_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        todo!()
    }
    /// `spc_suspend_annot`.
    pub fn spc_suspend_annot(&mut self, spe: &mut SpcEnv) -> i32 {
        todo!()
    }

    /// `spc_begin_form`.
    pub fn spc_begin_form(
        &mut self,
        spe: &mut SpcEnv,
        ident: &[u8],
        cp: PdfCoord,
        cropbox: &PdfRect,
    ) -> i32 {
        todo!()
    }
    /// `spc_end_form`.
    pub fn spc_end_form(&mut self, spe: &mut SpcEnv, attr: Option<Obj>) -> i32 {
        todo!()
    }

    /// `spc_is_tracking_boxes`.
    pub fn spc_is_tracking_boxes(&mut self, spe: &mut SpcEnv) -> bool {
        todo!()
    }
    /// `spc_set_linkmode`: 0 normal, 1 capture phantom texts.
    pub fn spc_set_linkmode(&mut self, spe: &mut SpcEnv, mode: i32) {
        todo!()
    }
    /// `spc_set_phantom`.
    pub fn spc_set_phantom(&mut self, spe: &mut SpcEnv, height: f64, depth: f64) {
        todo!()
    }

    /// `spc_push_object`: to `global_names` (takes `value`).
    pub fn spc_push_object(&mut self, spe: &mut SpcEnv, key: &[u8], value: Obj) {
        todo!()
    }
    /// `spc_flush_object`.
    pub fn spc_flush_object(&mut self, spe: &mut SpcEnv, key: &[u8]) {
        todo!()
    }
    /// `spc_clear_objects` (does nothing).
    pub fn spc_clear_objects(&mut self, spe: &mut SpcEnv) {
        todo!()
    }

    /// `spc_put_image`.
    pub fn spc_put_image(
        &mut self,
        spe: &mut SpcEnv,
        res_id: i32,
        ti: &mut TransformInfo,
        xpos: f64,
        ypos: f64,
    ) {
        todo!()
    }
    /// `spc_get_current_point`.
    pub fn spc_get_current_point(&mut self, spe: &mut SpcEnv) -> PdfCoord {
        todo!()
    }

    /// `spc_get_coord`: the top of `coords`, or (0, 0).
    pub fn spc_get_coord(&mut self, spe: &mut SpcEnv) -> (f64, f64) {
        todo!()
    }
    /// `spc_push_coord`.
    pub fn spc_push_coord(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        todo!()
    }
    /// `spc_pop_coord`.
    pub fn spc_pop_coord(&mut self, spe: &mut SpcEnv) {
        todo!()
    }
    /// `spc_set_fixed_point`.
    pub fn spc_set_fixed_point(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        todo!()
    }
    /// `spc_get_fixed_point`.
    pub fn spc_get_fixed_point(&mut self, spe: &mut SpcEnv) -> (f64, f64) {
        todo!()
    }
    /// `spc_put_fixed_point`.
    pub fn spc_put_fixed_point(&mut self, spe: &mut SpcEnv, x: f64, y: f64) {
        todo!()
    }
    /// `spc_dup_fixed_point`.
    pub fn spc_dup_fixed_point(&mut self, spe: &mut SpcEnv) {
        todo!()
    }
    /// `spc_pop_fixed_point`.
    pub fn spc_pop_fixed_point(&mut self, spe: &mut SpcEnv) {
        todo!()
    }
    /// `spc_clear_fixed_point`.
    pub fn spc_clear_fixed_point(&mut self, spe: &mut SpcEnv) {
        todo!()
    }

    /// `spc_exec_at_begin_page`.
    pub fn spc_exec_at_begin_page(&mut self) -> i32 {
        todo!()
    }
    /// `spc_exec_at_end_page`.
    pub fn spc_exec_at_end_page(&mut self) -> i32 {
        todo!()
    }
    /// `spc_exec_at_begin_document`.
    pub fn spc_exec_at_begin_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_exec_at_end_document`.
    pub fn spc_exec_at_end_document(&mut self) -> i32 {
        todo!()
    }

    /// `spc_exec_special`: error, and C's out-parameters `is_drawable`
    /// and `rect` (set only when no error).
    pub fn spc_exec_special(
        &mut self,
        buffer: &[u8],
        x_user: f64,
        y_user: f64,
        mag: f64,
    ) -> (i32, i32, PdfRect) {
        todo!()
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
    todo!()
}

/// `print_error`.
fn print_error(dpx: &mut Dpx, name: &[u8], spe: &mut SpcEnv, ap: &mut SpcArg) {
    todo!()
}
