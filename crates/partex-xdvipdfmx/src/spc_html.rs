//! spc_html.c, spc_html.h: `html:` specials (hypertex anchors, `<img>`,
//! `<base>`).
//!
//! C's single `struct spc_html_ _html_state` is this module's `State`
//! (`self.html`); the C functions' `sd` parameter is dropped.

use crate::pdfdev::{PdfTmatrix, TransformInfo};
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

/// `ENABLE_HTML_IMG_SUPPORT` etc. are all on.
pub const ANCHOR_TYPE_HREF: i32 = 0;
pub const ANCHOR_TYPE_NAME: i32 = 1;

/// `HTML_TAG_NAME_MAX`.
pub const HTML_TAG_NAME_MAX: usize = 127;
/// `HTML_TAG_TYPE_EMPTY`.
pub const HTML_TAG_TYPE_EMPTY: i32 = 1;
/// `HTML_TAG_TYPE_OPEN`.
pub const HTML_TAG_TYPE_OPEN: i32 = 1;
/// `HTML_TAG_TYPE_CLOSE`.
pub const HTML_TAG_TYPE_CLOSE: i32 = 2;

/// `struct spc_html_` (`_html_state`).
#[derive(Clone, Debug)]
pub struct State {
    /// `opts.extensions`.
    pub extensions: i32,
    pub link_dict: Option<Obj>,
    pub baseurl: Option<Vec<u8>>,
    /// -1 when no anchor is pending.
    pub pending_type: i32,
}

impl Default for State {
    fn default() -> Self {
        State {
            extensions: 0,
            link_dict: None,
            baseurl: None,
            pending_type: -1,
        }
    }
}

/// `parse_key_val` (static): status, key and value (`key="value"`).
fn parse_key_val(s: &[u8], pp: &mut usize) -> (i32, Option<Vec<u8>>, Option<Vec<u8>>) {
    todo!()
}

/// `fqurl` (static): `name` resolved against `baseurl`.
fn fqurl(baseurl: Option<&[u8]>, name: &[u8]) -> Vec<u8> {
    todo!()
}

/// `atopt` (static): a length with its unit, in bp.
fn atopt(a: &[u8]) -> f64 {
    todo!()
}

/// `cvt_a_to_tmatrix` (static): an SVG `transform` attribute; status and
/// the matrix (`pp` is C's `nextptr`).
fn cvt_a_to_tmatrix(s: &[u8], pp: &mut usize) -> (i32, PdfTmatrix) {
    todo!()
}

impl Dpx {
    /// `spc_html_at_begin_page`.
    pub fn spc_html_at_begin_page(&mut self) -> i32 {
        todo!()
    }
    /// `spc_html_at_end_page`.
    pub fn spc_html_at_end_page(&mut self) -> i32 {
        todo!()
    }
    /// `spc_html_at_begin_document`.
    pub fn spc_html_at_begin_document(&mut self) -> i32 {
        todo!()
    }
    /// `spc_html_at_end_document`.
    pub fn spc_html_at_end_document(&mut self) -> i32 {
        todo!()
    }
}

/// `read_html_tag` (static): status, the tag name (downcased), and
/// its type (`HTML_TAG_TYPE_*`); attributes go into `attr`.
fn read_html_tag(dpx: &mut Dpx, attr: Obj, s: &[u8], pp: &mut usize) -> (i32, Vec<u8>, i32) {
    todo!()
}

/// `spc_handler_html__init`.
fn spc_handler_html__init(dpx: &mut Dpx) -> i32 {
    todo!()
}

/// `spc_handler_html__clean` (`spe` may be none).
fn spc_handler_html__clean(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    todo!()
}

/// `spc_handler_html__bophook`.
fn spc_handler_html__bophook(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    todo!()
}

/// `spc_handler_html__eophook`.
fn spc_handler_html__eophook(dpx: &mut Dpx, spe: Option<&mut SpcEnv>) -> i32 {
    todo!()
}

/// `html_open_link`.
fn html_open_link(dpx: &mut Dpx, spe: &mut SpcEnv, name: &[u8]) -> i32 {
    todo!()
}

/// `html_open_dest`.
fn html_open_dest(dpx: &mut Dpx, spe: &mut SpcEnv, name: &[u8]) -> i32 {
    todo!()
}

/// `spc_html__anchor_open`.
fn spc_html__anchor_open(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    todo!()
}

/// `spc_html__anchor_close`.
fn spc_html__anchor_close(dpx: &mut Dpx, spe: &mut SpcEnv) -> i32 {
    todo!()
}

/// `spc_html__base_empty`.
fn spc_html__base_empty(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    todo!()
}

/// `create_xgstate`: an ExtGState with `CA`/`ca` = `a`.
fn create_xgstate(dpx: &mut Dpx, a: f64) -> Obj {
    todo!()
}

/// `spc_html__img_empty` (the `ENABLE_HTML_IMG_SUPPORT` one).
fn spc_html__img_empty(dpx: &mut Dpx, spe: &mut SpcEnv, attr: Obj) -> i32 {
    todo!()
}

/// `spc_handler_html_default`.
fn spc_handler_html_default(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_html_check_special`.
pub fn spc_html_check_special(buffer: &[u8]) -> bool {
    todo!()
}

/// `spc_html_setup_handler`.
pub fn spc_html_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
