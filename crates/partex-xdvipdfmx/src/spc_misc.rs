//! spc_misc.c, spc_misc.h: pdfTeX-compatible and other small specials
//! (`postscriptbox`, `pdfcolorstack`, `pdffontattr`, `landscape`, …).

use crate::dpxutil::DpxStack;
use crate::pdfdev::PdfCoord;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

/// `PDFCOLORSTACK_MAX_STACK`.
pub const PDFCOLORSTACK_MAX_STACK: usize = 256;

/// `struct stack`: one pdfcolorstack.
#[derive(Clone, Debug, Default)]
pub struct Stack {
    pub page: i32,
    pub direct: i32,
    /// The literal strings (PDF string objects, owned: released on pop).
    pub stack: DpxStack<Obj>,
}

/// `struct fontattr`: a `pdffontattr` applied at the end of the document.
#[derive(Clone, Debug, Default)]
pub struct Fontattr {
    pub ident: Vec<u8>,
    pub size: f64,
    pub attr: Option<Obj>,
}

/// spc_misc.c's `static` state.
#[derive(Clone, Debug)]
pub struct State {
    /// `spc_stack.stacks` (`PDFCOLORSTACK_MAX_STACK` of them).
    pub stacks: Vec<Stack>,
    /// `fontattrs` (none = C's NULL; `num_fontattrs` is its length,
    /// `max_fontattrs` its capacity).
    pub fontattrs: Option<Vec<Fontattr>>,
}

impl Default for State {
    fn default() -> Self {
        let mut stacks = Vec::with_capacity(PDFCOLORSTACK_MAX_STACK);
        stacks.resize_with(PDFCOLORSTACK_MAX_STACK, Stack::default);
        State {
            stacks,
            fontattrs: None,
        }
    }
}

impl Dpx {
    /// `spc_misc_at_begin_document`.
    pub fn spc_misc_at_begin_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_misc_at_end_document`.
    pub fn spc_misc_at_end_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_misc_at_begin_page`.
    pub fn spc_misc_at_begin_page(&mut self) -> i32 {
        todo!()
    }
    /// `spc_misc_at_begin_form`.
    pub fn spc_misc_at_begin_form(&mut self) -> i32 {
        todo!()
    }
    /// `spc_misc_at_end_form`.
    pub fn spc_misc_at_end_form(&mut self) -> i32 {
        todo!()
    }
}

/// `pdfcolorstack__get_id`: status and the id.
fn pdfcolorstack__get_id(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> (i32, i32) {
    todo!()
}

/// `pdfcolorstack__init` (on `self.misc.stacks`).
fn pdfcolorstack__init(dpx: &mut Dpx) -> i32 {
    todo!()
}

/// `pdfcolorstack__clean` (releases the strings).
fn pdfcolorstack__clean(dpx: &mut Dpx) -> i32 {
    todo!()
}

/// `pdfcolorstack__set_litstr`.
fn pdfcolorstack__set_litstr(dpx: &mut Dpx, cp: PdfCoord, litstr: Option<Obj>, direct: i32) {
    todo!()
}

/// `pdfcolorstack__set`: `st` is the stack's id (index of
/// `self.misc.stacks`).
fn pdfcolorstack__set(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `pdfcolorstack__push`.
fn pdfcolorstack__push(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `pdfcolorstack__current`.
fn pdfcolorstack__current(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `pdfcolorstack__pop`.
fn pdfcolorstack__pop(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    st: usize,
    cp: PdfCoord,
    args: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `parse_pdf_reference` (static; the `@name` callback of the parser).
fn parse_pdf_reference(dpx: &mut Dpx, s: &[u8], pp: &mut usize) -> Option<Obj> {
    todo!()
}

/// `process_fontattr`: merges `attr` into the font's resource dict.
fn process_fontattr(dpx: &mut Dpx, ident: &[u8], size: f64, attr: Obj) -> i32 {
    todo!()
}

/// `spc_handler_pdfcolorstackinit`.
fn spc_handler_pdfcolorstackinit(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdfcolorstack`.
fn spc_handler_pdfcolorstack(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_pdffontattr`.
fn spc_handler_pdffontattr(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_postscriptbox`.
fn spc_handler_postscriptbox(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_null`: skips the rest.
fn spc_handler_null(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `misc_handlers`.
pub static MISC_HANDLERS: [SpcHandler; 9] = [
    SpcHandler {
        key: b"postscriptbox",
        exec: spc_handler_postscriptbox,
    },
    SpcHandler {
        key: b"pdfcolorstackinit",
        exec: spc_handler_pdfcolorstackinit,
    },
    SpcHandler {
        key: b"pdfcolorstack",
        exec: spc_handler_pdfcolorstack,
    },
    SpcHandler {
        key: b"pdffontattr",
        exec: spc_handler_pdffontattr,
    },
    // handled at bop
    SpcHandler {
        key: b"landscape",
        exec: spc_handler_null,
    },
    // handled at bop
    SpcHandler {
        key: b"papersize",
        exec: spc_handler_null,
    },
    // simply ignored
    SpcHandler {
        key: b"src:",
        exec: spc_handler_null,
    },
    SpcHandler {
        key: b"pos:",
        exec: spc_handler_null,
    },
    SpcHandler {
        key: b"om:",
        exec: spc_handler_null,
    },
];

/// `spc_misc_check_special`.
pub fn spc_misc_check_special(buffer: &[u8]) -> bool {
    todo!()
}

/// `spc_misc_setup_handler` (sets `handle.key` to `"???:"`).
pub fn spc_misc_setup_handler(
    dpx: &mut Dpx,
    handle: &mut SpcHandler,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> i32 {
    todo!()
}
