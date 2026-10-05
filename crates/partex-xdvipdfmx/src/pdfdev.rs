//! pdfdev.c, pdfdev.h: the output device (text, rules, images, units).
//!
//! C's single `static pdf_dev pdev` is `self.dev.pdev`; the static
//! functions that take `pdf_dev *p` are `Dpx` methods without it. The
//! `pdf_sprint_*` writers append to a [`Buf`] and return the bytes
//! written (C's `char *buf` + length). `p_itoa`/`p_dtoa` are in
//! [`crate::fmt`].

use crate::fmt::Buf;
use crate::fontmap::FontmapRec;
use crate::prelude::*;

/// `spt_t`: a length in DVI units (scaled points).
pub type Spt = i32;

/// `pdf_tmatrix`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfTmatrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

/// `pdf_rect`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfRect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

/// `pdf_coord`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfCoord {
    pub x: f64,
    pub y: f64,
}

/// `transform_info`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransformInfo {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    /// Transform matrix.
    pub matrix: PdfTmatrix,
    /// User bbox.
    pub bbox: PdfRect,
    pub flags: i32,
}

pub const INFO_HAS_USER_BBOX: i32 = 1 << 0;
pub const INFO_HAS_WIDTH: i32 = 1 << 1;
pub const INFO_HAS_HEIGHT: i32 = 1 << 2;
pub const INFO_DO_CLIP: i32 = 1 << 3;
pub const INFO_DO_HIDE: i32 = 1 << 4;

/// `PDF_DEV_PARAM_AUTOROTATE`.
pub const PDF_DEV_PARAM_AUTOROTATE: i32 = 1;
/// `PDF_DEV_PARAM_COLORMODE`.
pub const PDF_DEV_PARAM_COLORMODE: i32 = 2;

/// Motion state: not within BT/ET nor in a string.
pub const GRAPHICS_MODE: i32 = 1;
/// Motion state: in BT/ET, not in a string.
pub const TEXT_MODE: i32 = 2;
/// Motion state: in a string.
pub const STRING_MODE: i32 = 3;

pub const TEXT_WMODE_HH: i32 = 0;
pub const TEXT_WMODE_HV: i32 = 1;
pub const TEXT_WMODE_VH: i32 = 4;
pub const TEXT_WMODE_VV: i32 = 5;
pub const TEXT_WMODE_HD: i32 = 3;
pub const TEXT_WMODE_VD: i32 = 7;

pub const PDF_FONTTYPE_SIMPLE: i32 = 1;
pub const PDF_FONTTYPE_BITMAP: i32 = 2;
pub const PDF_FONTTYPE_COMPOSITE: i32 = 3;

/// `TEX_ONE_HUNDRED_BP`.
pub const TEX_ONE_HUNDRED_BP: i32 = 6578176;
/// `FORMAT_BUF_SIZE`.
pub const FORMAT_BUF_SIZE: usize = 4096;
/// `DEV_PRECISION_MAX`.
pub const DEV_PRECISION_MAX: i32 = 8;
/// `PDF_LINE_THICKNESS_MAX`.
pub const PDF_LINE_THICKNESS_MAX: f64 = 5.0;

/// `ten_pow`.
pub const TEN_POW: [u32; 10] = [
    1, 10, 100, 1000, 10000, 100000, 1000000, 10000000, 100000000, 1000000000,
];
/// `ten_pow_inv`.
pub const TEN_POW_INV: [f64; 10] = [
    1.0,
    0.1,
    0.01,
    0.001,
    0.0001,
    0.00001,
    0.000001,
    0.0000001,
    0.00000001,
    0.000000001,
];

/// `ANGLE_CHANGES(m1, m2)`.
#[must_use]
pub fn angle_changes(m1: i32, m2: i32) -> bool {
    todo!()
}

/// `ROTATE_TEXT(m)`.
#[must_use]
pub fn rotate_text(m: i32) -> bool {
    todo!()
}

/// `struct dev_param`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevParam {
    pub autorotate: i32,
    /// 0: ignore colors.
    pub colormode: i32,
}

/// `text_state.matrix`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextMatrix {
    pub slant: f64,
    pub extend: f64,
    /// `TEXT_WMODE_XX`.
    pub rotate: i32,
}

/// `struct text_state`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextState {
    /// Index into `PdfDev::fonts` (-1: none).
    pub font_id: i32,
    pub offset: Spt,
    pub ref_x: Spt,
    pub ref_y: Spt,
    pub raise: Spt,
    pub leading: Spt,
    pub matrix: TextMatrix,
    pub bold_param: f64,
    pub dir_mode: i32,
    pub force_reset: i32,
    pub is_mb: i32,
}

/// `struct dev_font`.
///
/// C's `used_chars` (a pointer into the pdf_font's usedchars) is not a
/// field: mark glyphs through `font_id` (`pdf_get_font_usedchars`
/// semantics, the descendant's for a Type0 font).
#[derive(Clone, Debug, Default)]
pub struct DevFont {
    /// Resource name ("F12"; C's `char short_name[16]`).
    pub short_name: Vec<u8>,
    pub used_on_this_page: i32,
    pub tex_name: Vec<u8>,
    pub sptsize: Spt,
    /// The pdf_font id.
    pub font_id: i32,
    /// The encoding (CMap) id.
    pub enc_id: i32,
    pub resource: Option<Obj>,
    /// `PDF_FONTTYPE_*`.
    pub format: i32,
    /// Non-zero for vertical.
    pub wmode: i32,
    pub extend: f64,
    pub slant: f64,
    pub bold: f64,
}

/// `struct dev_unit`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevUnit {
    /// DVI unit to bp multiplier.
    pub dvi2pts: f64,
    /// Shortest resolvable distance in the output (DVI units).
    pub min_bp_val: i32,
    /// Decimal digits kept.
    pub precision: i32,
}

/// `struct pdf_dev`.
#[derive(Clone, Debug, Default)]
pub struct PdfDev {
    pub motion_state: i32,
    pub param: DevParam,
    pub unit: DevUnit,
    pub text_state: TextState,
    /// `fonts` / `num_dev_fonts` / `max_dev_fonts`.
    pub fonts: Vec<DevFont>,
    /// `format_buffer` (scratch).
    pub format_buffer: Buf,
}

/// pdfdev.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `pdev`.
    pub pdev: PdfDev,
}

impl TransformInfo {
    /// `transform_info_clear`.
    pub fn transform_info_clear(&mut self) {
        todo!()
    }
}

impl Dpx {
    /// `dev_out`: appends to the page content (`pdf_doc_add_page_content`).
    fn dev_out(&mut self, s: &[u8]) {
        todo!()
    }
    /// `pdf_dev_unit_dviunit` (static).
    fn pdf_dev_unit_dviunit(&self) -> f64 {
        todo!()
    }
    /// `bpt2spt`.
    fn bpt2spt(&self, b: f64) -> Spt {
        todo!()
    }
    /// `spt2bpt`.
    fn spt2bpt(&self, s: Spt) -> f64 {
        todo!()
    }
    /// `dev_sprint_bp`: bytes written, and the rounding error (`*error`).
    fn dev_sprint_bp(&self, buf: &mut Buf, value: Spt) -> (usize, Spt) {
        todo!()
    }
    /// `pdf_dev_sprint_matrix` (static).
    fn pdf_dev_sprint_matrix(&self, buf: &mut Buf, m: &PdfTmatrix) -> usize {
        todo!()
    }
    /// `pdf_dev_sprint_rect` (static).
    fn pdf_dev_sprint_rect(&self, buf: &mut Buf, rect: &PdfRect) -> usize {
        todo!()
    }
    /// `pdf_dev_sprint_coord` (static).
    fn pdf_dev_sprint_coord(&self, buf: &mut Buf, c: &PdfCoord) -> usize {
        todo!()
    }
    /// `pdf_dev_sprint_length` (static).
    fn pdf_dev_sprint_length(&self, buf: &mut Buf, value: f64) -> usize {
        todo!()
    }
    /// `pdf_dev_sprint_number` (static).
    fn pdf_dev_sprint_number(&self, buf: &mut Buf, value: f64) -> usize {
        todo!()
    }
    /// `dev_set_text_matrix`.
    fn dev_set_text_matrix(&mut self, xpos: Spt, ypos: Spt, slant: f64, extend: f64, rotate: i32) {
        todo!()
    }
    /// `reset_text_state`.
    fn reset_text_state(&mut self) {
        todo!()
    }
    /// `pdf_dev_text_mode`.
    fn pdf_dev_text_mode(&mut self) {
        todo!()
    }
    /// `pdf_dev_graphics_mode`.
    fn pdf_dev_graphics_mode(&mut self) {
        todo!()
    }
    /// `start_string`.
    fn start_string(&mut self, xpos: Spt, ypos: Spt, slant: f64, extend: f64, rotate: i32) {
        todo!()
    }
    /// `pdf_dev_string_mode`.
    fn pdf_dev_string_mode(&mut self, xpos: Spt, ypos: Spt, slant: f64, extend: f64, rotate: i32) {
        todo!()
    }
    /// `pdf_dev_set_font` (static): `font_id` indexes `pdev.fonts`.
    fn pdf_dev_set_font(&mut self, font_id: i32) -> i32 {
        todo!()
    }
    /// `handle_multibyte_string`: status (0, or -1 when the CMap
    /// conversion failed) and the string to show (decoded through the
    /// dev font's CMap when it has one; C's `sbuf0`).
    fn handle_multibyte_string(&mut self, dev_font: usize, s: &[u8]) -> (i32, Vec<u8>) {
        todo!()
    }
    /// `pdf_dev_set_string`: `font_id` is a dev font index.
    pub fn pdf_dev_set_string(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        instr: &[u8],
        width: Spt,
        font_id: i32,
    ) {
        todo!()
    }
    /// `pdf_init_device`.
    pub fn pdf_init_device(&mut self, dvi2pts: f64, precision: i32, black_and_white: i32) {
        todo!()
    }
    /// `pdf_close_device`.
    pub fn pdf_close_device(&mut self) {
        todo!()
    }
    /// `pdf_dev_reset_fonts`.
    pub fn pdf_dev_reset_fonts(&mut self, newpage: i32) {
        todo!()
    }
    /// `pdf_dev_reset_color`.
    pub fn pdf_dev_reset_color(&mut self, force: i32) {
        todo!()
    }
    /// `pdf_dev_bop`.
    pub fn pdf_dev_bop(&mut self, m: &PdfTmatrix) {
        todo!()
    }
    /// `pdf_dev_eop`.
    pub fn pdf_dev_eop(&mut self) {
        todo!()
    }
    /// `print_fontmap` (verbose output only).
    fn print_fontmap(&mut self, font_name: &[u8], mrec: Option<&FontmapRec>) {
        todo!()
    }
    /// `pdf_dev_locate_font`: the dev font index, or -1.
    pub fn pdf_dev_locate_font(&mut self, font_name: &[u8], ptsize: Spt) -> i32 {
        todo!()
    }
    /// `dev_sprint_line`.
    fn dev_sprint_line(
        &self,
        buf: &mut Buf,
        width: Spt,
        p0_x: Spt,
        p0_y: Spt,
        p1_x: Spt,
        p1_y: Spt,
    ) -> usize {
        todo!()
    }
    /// `pdf_dev_set_rule`.
    pub fn pdf_dev_set_rule(&mut self, xpos: Spt, ypos: Spt, width: Spt, height: Spt) {
        todo!()
    }
    /// `pdf_dev_set_rect`: the rectangle in device space (C's `rect`).
    pub fn pdf_dev_set_rect(
        &mut self,
        x_pos: Spt,
        y_pos: Spt,
        width: Spt,
        height: Spt,
        depth: Spt,
    ) -> PdfRect {
        todo!()
    }
    /// `pdf_dev_get_dirmode`.
    pub fn pdf_dev_get_dirmode(&self) -> i32 {
        todo!()
    }
    /// `pdf_dev_set_dirmode`.
    pub fn pdf_dev_set_dirmode(&mut self, text_dir: i32) {
        todo!()
    }
    /// `dev_set_param_autorotate`.
    fn dev_set_param_autorotate(&mut self, auto_rotate: i32) {
        todo!()
    }
    /// `pdf_dev_get_param`.
    pub fn pdf_dev_get_param(&self, param_type: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_set_param`.
    pub fn pdf_dev_set_param(&mut self, param_type: i32, value: i32) {
        todo!()
    }
    /// `pdf_dev_set_autorotate(v)`.
    pub fn pdf_dev_set_autorotate(&mut self, v: i32) {
        todo!()
    }
    /// `pdf_dev_put_image`: status and the rectangle (C's `rect` out
    /// parameter; callers passing NULL ignore it).
    pub fn pdf_dev_put_image(
        &mut self,
        id: i32,
        ti: &mut TransformInfo,
        ref_x: f64,
        ref_y: f64,
    ) -> (i32, PdfRect) {
        todo!()
    }
    /// `pdf_dev_begin_actualtext`.
    pub fn pdf_dev_begin_actualtext(&mut self, unicodes: &[u16]) {
        todo!()
    }
    /// `pdf_dev_end_actualtext`.
    pub fn pdf_dev_end_actualtext(&mut self) {
        todo!()
    }
    /// `graphics_mode`.
    pub fn graphics_mode(&mut self) {
        todo!()
    }
    /// `dev_unit_dviunit`: 1/dvi2pts.
    pub fn dev_unit_dviunit(&self) -> f64 {
        todo!()
    }
    /// `pdf_sprint_matrix`.
    pub fn pdf_sprint_matrix(&self, buf: &mut Buf, m: &PdfTmatrix) -> usize {
        todo!()
    }
    /// `pdf_sprint_rect`.
    pub fn pdf_sprint_rect(&self, buf: &mut Buf, rect: &PdfRect) -> usize {
        todo!()
    }
    /// `pdf_sprint_coord`.
    pub fn pdf_sprint_coord(&self, buf: &mut Buf, c: &PdfCoord) -> usize {
        todo!()
    }
    /// `pdf_sprint_length`.
    pub fn pdf_sprint_length(&self, buf: &mut Buf, value: f64) -> usize {
        todo!()
    }
    /// `pdf_sprint_number`.
    pub fn pdf_sprint_number(&self, buf: &mut Buf, value: f64) -> usize {
        todo!()
    }
    /// `pdf_dev_get_font_wmode`: `font_id` is a dev font index.
    pub fn pdf_dev_get_font_wmode(&self, font_id: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_font_minbytes`: `font_id` is a dev font index.
    pub fn pdf_dev_font_minbytes(&self, font_id: i32) -> i32 {
        todo!()
    }
}
