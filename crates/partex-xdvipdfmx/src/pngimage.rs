//! pngimage.c, pngimage.h: PNG images.
//!
//! C reads PNGs with libpng; here [`PngInfo`] stands for libpng's
//! `png_structp` + `png_infop` (the chunks read, the transformations
//! asked for). libpng's behavior must be reproduced exactly: the chunk
//! values it reports, `png_set_strip_16`, `png_set_gamma` (gamma
//! correction of the samples), `png_read_update_info`'s rowbytes, and
//! the decoded rows (IDAT inflated — needs an inflater, zlib's output —
//! unfiltered, de-interlaced). `PNG_MAXIMUM_INFLATE_WINDOW` is on.

use crate::prelude::*;

/// `PNG_DEBUG_STR`.
pub const PNG_DEBUG_STR: &str = "PNG";
/// `PNG_DEBUG`.
pub const PNG_DEBUG: i32 = 3;
/// `DPX_PNG_DEFAULT_GAMMA`.
pub const DPX_PNG_DEFAULT_GAMMA: f64 = 2.2;

/// `PDF_TRANS_TYPE_NONE`.
pub const PDF_TRANS_TYPE_NONE: i32 = 0;
/// `PDF_TRANS_TYPE_BINARY`.
pub const PDF_TRANS_TYPE_BINARY: i32 = 1;
/// `PDF_TRANS_TYPE_ALPHA`.
pub const PDF_TRANS_TYPE_ALPHA: i32 = 2;

/// libpng's `PNG_COLOR_MASK_PALETTE`.
pub const PNG_COLOR_MASK_PALETTE: u8 = 1;
/// libpng's `PNG_COLOR_MASK_COLOR`.
pub const PNG_COLOR_MASK_COLOR: u8 = 2;
/// libpng's `PNG_COLOR_MASK_ALPHA`.
pub const PNG_COLOR_MASK_ALPHA: u8 = 4;
/// libpng's `PNG_COLOR_TYPE_GRAY`.
pub const PNG_COLOR_TYPE_GRAY: u8 = 0;
/// libpng's `PNG_COLOR_TYPE_PALETTE`.
pub const PNG_COLOR_TYPE_PALETTE: u8 = 3;
/// libpng's `PNG_COLOR_TYPE_RGB`.
pub const PNG_COLOR_TYPE_RGB: u8 = 2;
/// libpng's `PNG_COLOR_TYPE_RGB_ALPHA`.
pub const PNG_COLOR_TYPE_RGB_ALPHA: u8 = 6;
/// libpng's `PNG_COLOR_TYPE_GRAY_ALPHA`.
pub const PNG_COLOR_TYPE_GRAY_ALPHA: u8 = 4;

/// libpng's `PNG_sRGB_INTENT_PERCEPTUAL`.
pub const PNG_SRGB_INTENT_PERCEPTUAL: i32 = 0;
/// libpng's `PNG_sRGB_INTENT_RELATIVE`.
pub const PNG_SRGB_INTENT_RELATIVE: i32 = 1;
/// libpng's `PNG_sRGB_INTENT_SATURATION`.
pub const PNG_SRGB_INTENT_SATURATION: i32 = 2;
/// libpng's `PNG_sRGB_INTENT_ABSOLUTE`.
pub const PNG_SRGB_INTENT_ABSOLUTE: i32 = 3;

/// libpng's `PNG_RESOLUTION_METER`.
pub const PNG_RESOLUTION_METER: i32 = 1;
/// libpng's `PNG_ITXT_COMPRESSION_NONE`.
pub const PNG_ITXT_COMPRESSION_NONE: i32 = 1;

/// libpng's `png_color_16` (the tRNS color key).
#[derive(Clone, Copy, Debug, Default)]
pub struct PngColor16 {
    pub index: u8,
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub gray: u16,
}

/// libpng's `png_text` (tEXt, zTXt, iTXt chunks).
#[derive(Clone, Debug, Default)]
pub struct PngText {
    /// `PNG_TEXT_COMPRESSION_*` / `PNG_ITXT_COMPRESSION_*`.
    pub compression: i32,
    pub key: Vec<u8>,
    pub text: Vec<u8>,
    pub text_length: usize,
    pub itxt_length: usize,
    pub lang: Vec<u8>,
    pub lang_key: Vec<u8>,
}

/// libpng's `png_structp` + `png_infop`: the chunks read (`None` = not
/// `png_get_valid`) and the transformations set.
#[derive(Clone, Debug, Default)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    /// After `png_read_update_info`, the transformed depth.
    pub bit_depth: u8,
    pub color_type: u8,
    pub compression_type: u8,
    pub filter_type: u8,
    pub interlace_type: u8,
    /// `png_get_rowbytes` (after `png_read_update_info`).
    pub rowbytes: u32,
    /// PLTE.
    pub palette: Vec<[u8; 3]>,
    /// tRNS: the palette alphas (`trans_alpha`, `num_trans`).
    pub trans_alpha: Vec<u8>,
    /// tRNS: the color key.
    pub trans_color: Option<PngColor16>,
    /// gAMA (`png_get_gAMA`, the file gamma).
    pub gamma: Option<f64>,
    /// sRGB intent.
    pub srgb_intent: Option<i32>,
    /// cHRM: white x, y, red x, y, green x, y, blue x, y.
    pub chrm: Option<[f64; 8]>,
    /// iCCP: name, compression type, the profile (decompressed).
    pub iccp: Option<(Vec<u8>, i32, Vec<u8>)>,
    /// pHYs: x and y pixels per unit, unit type.
    pub phys: Option<(u32, u32, i32)>,
    /// tEXt / zTXt / iTXt, in file order.
    pub text: Vec<PngText>,
    /// The IDAT chunks' data, concatenated.
    pub idat: Vec<u8>,
    /// `png_set_strip_16` asked.
    pub strip_16: bool,
    /// `png_set_gamma(screen_gamma, file_gamma)` asked.
    pub set_gamma: Option<(f64, f64)>,
}

/// `check_for_png`: 1 if the signature is PNG's.
pub fn check_for_png(fp: &mut MemFile) -> i32 {
    todo!()
}

/// libpng's `png_read_info`: the chunks up to the first IDAT (and
/// collects the IDATs and the chunks after, as libpng reads them by the
/// end); none on an error (C's `setjmp` path).
pub fn png_read_info(fp: &mut MemFile) -> Option<PngInfo> {
    todo!()
}

/// libpng's `png_read_update_info`: applies the transformations to the
/// reported depth and rowbytes.
pub fn png_read_update_info(png: &mut PngInfo) {
    todo!()
}

/// `check_transparency` (static): a `PDF_TRANS_TYPE_*`.
pub fn check_transparency(png: &PngInfo) -> i32 {
    todo!()
}

/// `read_image_data` (static): the rows, decoded and transformed, into
/// `dest` (`height * rowbytes` bytes); libpng's `png_read_image`.
pub fn read_image_data(png: &mut PngInfo, dest: &mut [u8], height: u32, rowbytes: u32) {
    todo!()
}

impl Dpx {
    /// `png_include_image`: fills XObject `xobj_id`; 0 or -1.
    pub fn png_include_image(&mut self, xobj_id: i32, fp: &mut MemFile) -> i32 {
        todo!()
    }
    /// `png_get_bbox`: status, width, height, xdensity, ydensity.
    pub fn png_get_bbox(&mut self, fp: &mut MemFile) -> (i32, u32, u32, f64, f64) {
        todo!()
    }
    /// `create_cspace_Indexed` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_Indexed(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `create_cspace_CalRGB` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_CalRGB(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `create_cspace_CalGray` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_CalGray(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `make_param_Cal` (static).
    #[allow(non_snake_case)]
    pub fn make_param_Cal(
        &mut self,
        color_type: u8,
        g: f64,
        xw: f64,
        yw: f64,
        xr: f64,
        yr: f64,
        xg: f64,
        yg: f64,
        xb: f64,
        yb: f64,
    ) -> Option<Obj> {
        todo!()
    }
    /// `create_cspace_sRGB` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_sRGB(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `get_rendering_intent` (static).
    pub fn get_rendering_intent(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `create_cspace_ICCBased` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_ICCBased(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `create_ckey_mask` (static).
    pub fn create_ckey_mask(&mut self, png: &PngInfo) -> Option<Obj> {
        todo!()
    }
    /// `create_soft_mask` (static): for palette images.
    pub fn create_soft_mask(
        &mut self,
        png: &PngInfo,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> Option<Obj> {
        todo!()
    }
    /// `strip_soft_mask` (static): the alpha channel as an SMask; strips it
    /// from `image_data` in place and updates `rowbytes`.
    pub fn strip_soft_mask(
        &mut self,
        png: &PngInfo,
        image_data: &mut Vec<u8>,
        rowbytes: &mut u32,
        width: u32,
        height: u32,
    ) -> Option<Obj> {
        todo!()
    }
}
