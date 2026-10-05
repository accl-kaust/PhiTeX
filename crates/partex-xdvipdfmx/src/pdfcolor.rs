//! pdfcolor.c, pdfcolor.h: colors, the color stack, ICC colorspaces.
//!
//! Color operations on plain `PdfColor` data are its methods. The color
//! stack and the colorspace cache (`cspc_id` indexes `cspc_cache`) are in
//! `self.color`. `pdf_color_get_current` returns copies of the current
//! stroke and fill colors (C returns pointers into the stack, only read).
//! No named-color table here: those are spc_color.c / spc_util.c's.
//! (`pdf_colorspace_load_ICCBased`, `iccp_load_file_stream`,
//! `pdf_get_colorspace_num_components`/`_subtype` and
//! `pdf_dev_preserve_color` are `#if 0` in C: not ported.)

use crate::fmt::Buf;
use crate::prelude::*;

pub const PDF_COLORSPACE_TYPE_CMYK: i32 = -4;
pub const PDF_COLORSPACE_TYPE_RGB: i32 = -3;
pub const PDF_COLORSPACE_TYPE_SPOT: i32 = -2;
pub const PDF_COLORSPACE_TYPE_GRAY: i32 = -1;
pub const PDF_COLORSPACE_TYPE_INVALID: i32 = 0;
pub const PDF_COLORSPACE_TYPE_DEVICEGRAY: i32 = 1;
pub const PDF_COLORSPACE_TYPE_DEVICERGB: i32 = 2;
pub const PDF_COLORSPACE_TYPE_DEVICECMYK: i32 = 3;
pub const PDF_COLORSPACE_TYPE_CALGRAY: i32 = 4;
pub const PDF_COLORSPACE_TYPE_CALRGB: i32 = 5;
pub const PDF_COLORSPACE_TYPE_LAB: i32 = 6;
pub const PDF_COLORSPACE_TYPE_ICCBASED: i32 = 7;
pub const PDF_COLORSPACE_TYPE_SEPARATION: i32 = 8;
pub const PDF_COLORSPACE_TYPE_DEVICEN: i32 = 9;
pub const PDF_COLORSPACE_TYPE_INDEXED: i32 = 10;
pub const PDF_COLORSPACE_TYPE_PATTERN: i32 = 11;

/// `PDF_COLOR_COMPONENT_MAX`.
pub const PDF_COLOR_COMPONENT_MAX: usize = 32;

/// `PDF_COLOR_SOURCE_DVIPS`: "color push" / "color pop".
pub const PDF_COLOR_SOURCE_DVIPS: i32 = 0;
/// `PDF_COLOR_SOURCE_BCOLOR`: "pdf:bcolor" / "pdf:ecolor".
pub const PDF_COLOR_SOURCE_BCOLOR: i32 = 1;

/// `DEV_COLOR_STACK_MAX`.
pub const DEV_COLOR_STACK_MAX: usize = 128;

pub const ICC_INTENT_PERCEPTUAL: i32 = 0;
pub const ICC_INTENT_RELATIVE: i32 = 1;
pub const ICC_INTENT_SATURATION: i32 = 2;
pub const ICC_INTENT_ABSOLUTE: i32 = 3;

pub const ICC_HEAD_SECT1_START: usize = 0;
pub const ICC_HEAD_SECT1_LENGTH: usize = 56;
pub const ICC_HEAD_SECT2_START: usize = 68;
pub const ICC_HEAD_SECT2_LENGTH: usize = 16;
pub const ICC_HEAD_SECT3_START: usize = 100;
pub const ICC_HEAD_SECT3_LENGTH: usize = 28;

pub const PDF_COLORSPACE_FAMILY_DEVICE: i32 = 0;
pub const PDF_COLORSPACE_FAMILY_CIEBASED: i32 = 1;
pub const PDF_COLORSPACE_FAMILY_SPECIAL: i32 = 2;

/// `iccNullSig`.
pub const ICC_NULL_SIG: IccSig = 0;

/// `nullbytes16`.
pub const NULLBYTES16: [u8; 16] = [0; 16];

/// `icc_versions[]` (major, minor), indexed by PDF version - 10.
pub const ICC_VERSIONS: [(i32, i32); 11] = [
    (0, 0),
    (0, 0),
    (0, 0),
    (0x02, 0x10),
    (0x02, 0x20),
    (0x04, 0x00),
    (0x04, 0x00),
    (0x04, 0x20),
    (0x04, 0x20),
    (0x04, 0x20),
    (0x04, 0x20),
];

/// `pdf_color`.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfColor {
    pub res_id: i32,
    /// `PDF_COLORSPACE_TYPE_*`.
    pub r#type: i32,
    pub num_components: i32,
    pub spot_color_name: Option<Vec<u8>>,
    pub values: [f64; PDF_COLOR_COMPONENT_MAX],
    pub pattern_id: i32,
}

impl Default for PdfColor {
    /// All zero, as a C static is (`pdf_color_black` gives black gray).
    fn default() -> Self {
        PdfColor {
            res_id: 0,
            r#type: 0,
            num_components: 0,
            spot_color_name: None,
            values: [0.0; PDF_COLOR_COMPONENT_MAX],
            pattern_id: 0,
        }
    }
}

/// C's initial `current_fill`/`current_stroke`/`default_color`: gray 0,
/// `res_id` and `pattern_id` -1.
#[must_use]
pub fn pdf_color_initial() -> PdfColor {
    PdfColor {
        res_id: -1,
        r#type: PDF_COLORSPACE_TYPE_GRAY,
        num_components: 1,
        spot_color_name: None,
        values: [0.0; PDF_COLOR_COMPONENT_MAX],
        pattern_id: -1,
    }
}

/// `iccSig`.
pub type IccSig = u32;

/// `iccXYZNumber` (s15Fixed16Number).
#[derive(Clone, Copy, Debug, Default)]
pub struct IccXyzNumber {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// `iccHeader`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccHeader {
    pub size: i32,
    pub cmm_type: IccSig,
    pub version: i32,
    pub dev_class: IccSig,
    pub color_space: IccSig,
    /// Profile Connection Space.
    pub pcs: IccSig,
    pub creation_date: [u8; 12],
    pub acsp: IccSig,
    pub platform: IccSig,
    pub flags: [u8; 4],
    pub dev_mnfct: IccSig,
    pub dev_model: IccSig,
    pub dev_attr: [u8; 8],
    pub intent: i32,
    pub illuminant: IccXyzNumber,
    pub creator: IccSig,
    /// MD5 checksum.
    pub id: [u8; 16],
}

/// `struct iccbased_cdata`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccbasedCdata {
    /// 'i' 'c' 'c' 'b'.
    pub sig: i32,
    /// MD5 checksum.
    pub checksum: [u8; 16],
    /// Input colorspace (RGB, Gray, CMYK).
    pub colorspace: i32,
    /// Alternate colorspace (id), unused.
    pub alternate: i32,
}

/// `pdf_colorspace` (`cdata` is only ever an ICC-based one).
#[derive(Clone, Debug, Default)]
pub struct PdfColorspace {
    pub ident: Option<Vec<u8>>,
    pub subtype: i32,
    pub resource: Option<Obj>,
    pub reference: Option<Obj>,
    pub cdata: Option<IccbasedCdata>,
}

/// `color_stack` (`stroke`/`fill`/`source` have `DEV_COLOR_STACK_MAX`
/// entries).
#[derive(Clone, Debug)]
pub struct ColorStack {
    pub current: i32,
    pub stroke: Vec<PdfColor>,
    pub fill: Vec<PdfColor>,
    /// `PDF_COLOR_SOURCE_*`.
    pub source: Vec<i32>,
}

impl Default for ColorStack {
    fn default() -> Self {
        ColorStack {
            current: 0,
            stroke: vec![PdfColor::default(); DEV_COLOR_STACK_MAX],
            fill: vec![PdfColor::default(); DEV_COLOR_STACK_MAX],
            source: vec![0; DEV_COLOR_STACK_MAX],
        }
    }
}

/// pdfcolor.c's globals and statics.
#[derive(Clone, Debug)]
pub struct State {
    pub current_fill: PdfColor,
    pub current_stroke: PdfColor,
    pub default_color: PdfColor,
    pub color_stack: ColorStack,
    /// `cspc_cache` (count/capacity: the Vec).
    pub cspc_cache: Vec<PdfColorspace>,
}

impl Default for State {
    fn default() -> Self {
        State {
            current_fill: pdf_color_initial(),
            current_stroke: pdf_color_initial(),
            default_color: pdf_color_initial(),
            color_stack: ColorStack::default(),
            cspc_cache: Vec::new(),
        }
    }
}

impl PdfColor {
    /// `pdf_color_type`.
    #[must_use]
    pub fn pdf_color_type(&self) -> i32 {
        todo!()
    }
    /// `pdf_color_rgbcolor`.
    pub fn pdf_color_rgbcolor(&mut self, r: f64, g: f64, b: f64) -> i32 {
        todo!()
    }
    /// `pdf_color_cmykcolor`.
    pub fn pdf_color_cmykcolor(&mut self, c: f64, m: f64, y: f64, k: f64) -> i32 {
        todo!()
    }
    /// `pdf_color_graycolor`.
    pub fn pdf_color_graycolor(&mut self, g: f64) -> i32 {
        todo!()
    }
    /// `pdf_color_spotcolor`.
    pub fn pdf_color_spotcolor(&mut self, name: &[u8], c: f64) -> i32 {
        todo!()
    }
    /// `pdf_color_copycolor`: `self` (color1) = color2, deep copy.
    pub fn pdf_color_copycolor(&mut self, color2: &PdfColor) {
        todo!()
    }
    /// `pdf_color_black`.
    pub fn pdf_color_black(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_color_white`.
    pub fn pdf_color_white(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_color_brighten_color`: `self` is dst.
    pub fn pdf_color_brighten_color(&mut self, src: &PdfColor, f: f64) {
        todo!()
    }
    /// `pdf_color_is_white`.
    #[must_use]
    pub fn pdf_color_is_white(&self) -> i32 {
        todo!()
    }
    /// `pdf_color_set_color`: appends the operators (`mask` 0 or 0x20);
    /// bytes written. (C checks an estimate against `buffer_len`; callers
    /// pass 1024, `FORMAT_BUFF_LEN`.)
    pub fn pdf_color_set_color(&self, buf: &mut Buf, buffer_len: usize, mask: u8) -> usize {
        todo!()
    }
    /// `pdf_color_compare`.
    #[must_use]
    pub fn pdf_color_compare(&self, color2: &PdfColor) -> i32 {
        todo!()
    }
}

/// `str2iccSig`: C's `char` is signed.
#[must_use]
pub fn str2icc_sig(s: &[u8]) -> IccSig {
    todo!()
}

/// `check_sig(d, p, q, r, s)`.
#[must_use]
pub fn check_sig(d: Option<&IccbasedCdata>, p: u8, q: u8, r: u8, s: u8) -> bool {
    todo!()
}

/// `ICC_INTENT_TYPE(n)`.
#[must_use]
pub fn icc_intent_type(n: i32) -> i32 {
    todo!()
}

impl IccHeader {
    /// `iccp_init_iccHeader`.
    pub fn iccp_init_icc_header(&mut self) {
        todo!()
    }
}

impl PdfColorspace {
    /// `pdf_init_colorspace_struct`.
    pub fn pdf_init_colorspace_struct(&mut self) {
        todo!()
    }
}

impl IccbasedCdata {
    /// `init_iccbased_cdata`.
    pub fn init_iccbased_cdata(&mut self) {
        todo!()
    }
}

/// `get_num_components_iccbased`.
#[must_use]
pub fn get_num_components_iccbased(cdata: &IccbasedCdata) -> i32 {
    todo!()
}

/// `compare_iccbased`.
#[must_use]
pub fn compare_iccbased(
    ident1: Option<&[u8]>,
    cdata1: Option<&IccbasedCdata>,
    ident2: Option<&[u8]>,
    cdata2: Option<&IccbasedCdata>,
) -> i32 {
    todo!()
}

/// `iccp_get_checksum`: MD5 of the profile with the rendering intent,
/// header attributes and profile ID zeroed.
#[must_use]
pub fn iccp_get_checksum(profile: &[u8]) -> [u8; 16] {
    todo!()
}

/// `iccp_devClass_allowed`.
#[must_use]
pub fn iccp_dev_class_allowed(dev_class: i32) -> i32 {
    todo!()
}

impl Dpx {
    /// `iccp_version_supported` (reads the output PDF version).
    fn iccp_version_supported(&self, major: i32, minor: i32) -> i32 {
        todo!()
    }
    /// `iccp_unpack_header`: status (0 ok, -1 error).
    fn iccp_unpack_header(&mut self, icch: &mut IccHeader, profile: &[u8], check_size: i32) -> i32 {
        todo!()
    }
    /// `print_iccp_header` (verbose output only).
    fn print_iccp_header(&mut self, icch: &IccHeader, checksum: &[u8; 16]) {
        todo!()
    }
    /// `iccp_check_colorspace`.
    pub fn iccp_check_colorspace(&mut self, colortype: i32, profile: &[u8]) -> i32 {
        todo!()
    }
    /// `iccp_get_rendering_intent`.
    pub fn iccp_get_rendering_intent(&mut self, profile: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `iccp_load_profile`: the colorspace id, or -1.
    pub fn iccp_load_profile(&mut self, ident: Option<&[u8]>, profile: &[u8]) -> i32 {
        todo!()
    }
    /// `pdf_colorspace_findresource`: the id, or -1.
    fn pdf_colorspace_findresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: Option<&IccbasedCdata>,
    ) -> i32 {
        todo!()
    }
    /// `pdf_clean_colorspace_struct`: releases its objects.
    fn pdf_clean_colorspace_struct(&mut self, colorspace: &mut PdfColorspace) {
        todo!()
    }
    /// `pdf_flush_colorspace`: releases its objects.
    fn pdf_flush_colorspace(&mut self, colorspace: &mut PdfColorspace) {
        todo!()
    }
    /// `pdf_colorspace_defineresource`: the new id.
    fn pdf_colorspace_defineresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: Option<IccbasedCdata>,
        resource: Obj,
    ) -> i32 {
        todo!()
    }
    /// `pdf_get_colorspace_reference`.
    pub fn pdf_get_colorspace_reference(&mut self, cspc_id: i32) -> Obj {
        todo!()
    }
    /// `pdf_init_colors`.
    pub fn pdf_init_colors(&mut self) {
        todo!()
    }
    /// `pdf_close_colors`.
    pub fn pdf_close_colors(&mut self) {
        todo!()
    }
    /// `pdf_color_clear_stack`.
    pub fn pdf_color_clear_stack(&mut self) {
        todo!()
    }
    /// `pdf_color_set`.
    pub fn pdf_color_set(&mut self, sc: &PdfColor, fc: &PdfColor) {
        todo!()
    }
    /// `pdf_color_push`.
    pub fn pdf_color_push(&mut self, sc: &PdfColor, fc: &PdfColor, source: i32) {
        todo!()
    }
    /// `pdf_color_pop`.
    pub fn pdf_color_pop(&mut self, source: i32) {
        todo!()
    }
    /// `pdf_color_get_current`: copies of (stroke, fill).
    pub fn pdf_color_get_current(&self) -> (PdfColor, PdfColor) {
        todo!()
    }
}
