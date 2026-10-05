//! spc_util.c, spc_util.h: reading colors, lengths and dimension/transform
//! keys in specials.
//!
//! In/out parameters (a color or a `transform_info` the caller set up,
//! left alone on some errors) stay `&mut`.

use crate::pdfcolor::PdfColor;
use crate::pdfdev::{PdfTmatrix, TransformInfo};
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv};

/// `PDF_COLORSPACE_TYPE_GRAY` (pdfcolor.h).
const GRAY: i32 = -1;
/// `PDF_COLORSPACE_TYPE_CMYK` (pdfcolor.h).
const CMYK: i32 = -4;

/// An entry of `colordefs`: a named color (`res_id` -1, no spot name,
/// `pattern_id` -1; `num_components` 1 for gray, 4 for CMYK).
#[derive(Clone, Copy, Debug)]
pub struct ColorDef {
    pub key: &'static [u8],
    /// `PDF_COLORSPACE_TYPE_GRAY` or `PDF_COLORSPACE_TYPE_CMYK`.
    pub ty: i32,
    pub values: [f64; 4],
}

/// `colordefs`: the dvips color names, in C's order.
pub static COLORDEFS: [ColorDef; 68] = [
    ColorDef {
        key: b"GreenYellow",
        ty: CMYK,
        values: [0.15, 0.00, 0.69, 0.00],
    },
    ColorDef {
        key: b"Yellow",
        ty: CMYK,
        values: [0.00, 0.00, 1.00, 0.00],
    },
    ColorDef {
        key: b"Goldenrod",
        ty: CMYK,
        values: [0.00, 0.10, 0.84, 0.00],
    },
    ColorDef {
        key: b"Dandelion",
        ty: CMYK,
        values: [0.00, 0.29, 0.84, 0.00],
    },
    ColorDef {
        key: b"Apricot",
        ty: CMYK,
        values: [0.00, 0.32, 0.52, 0.00],
    },
    ColorDef {
        key: b"Peach",
        ty: CMYK,
        values: [0.00, 0.50, 0.70, 0.00],
    },
    ColorDef {
        key: b"Melon",
        ty: CMYK,
        values: [0.00, 0.46, 0.50, 0.00],
    },
    ColorDef {
        key: b"YellowOrange",
        ty: CMYK,
        values: [0.00, 0.42, 1.00, 0.00],
    },
    ColorDef {
        key: b"Orange",
        ty: CMYK,
        values: [0.00, 0.61, 0.87, 0.00],
    },
    ColorDef {
        key: b"BurntOrange",
        ty: CMYK,
        values: [0.00, 0.51, 1.00, 0.00],
    },
    ColorDef {
        key: b"Bittersweet",
        ty: CMYK,
        values: [0.00, 0.75, 1.00, 0.24],
    },
    ColorDef {
        key: b"RedOrange",
        ty: CMYK,
        values: [0.00, 0.77, 0.87, 0.00],
    },
    ColorDef {
        key: b"Mahogany",
        ty: CMYK,
        values: [0.00, 0.85, 0.87, 0.35],
    },
    ColorDef {
        key: b"Maroon",
        ty: CMYK,
        values: [0.00, 0.87, 0.68, 0.32],
    },
    ColorDef {
        key: b"BrickRed",
        ty: CMYK,
        values: [0.00, 0.89, 0.94, 0.28],
    },
    ColorDef {
        key: b"Red",
        ty: CMYK,
        values: [0.00, 1.00, 1.00, 0.00],
    },
    ColorDef {
        key: b"OrangeRed",
        ty: CMYK,
        values: [0.00, 1.00, 0.50, 0.00],
    },
    ColorDef {
        key: b"RubineRed",
        ty: CMYK,
        values: [0.00, 1.00, 0.13, 0.00],
    },
    ColorDef {
        key: b"WildStrawberry",
        ty: CMYK,
        values: [0.00, 0.96, 0.39, 0.00],
    },
    ColorDef {
        key: b"Salmon",
        ty: CMYK,
        values: [0.00, 0.53, 0.38, 0.00],
    },
    ColorDef {
        key: b"CarnationPink",
        ty: CMYK,
        values: [0.00, 0.63, 0.00, 0.00],
    },
    ColorDef {
        key: b"Magenta",
        ty: CMYK,
        values: [0.00, 1.00, 0.00, 0.00],
    },
    ColorDef {
        key: b"VioletRed",
        ty: CMYK,
        values: [0.00, 0.81, 0.00, 0.00],
    },
    ColorDef {
        key: b"Rhodamine",
        ty: CMYK,
        values: [0.00, 0.82, 0.00, 0.00],
    },
    ColorDef {
        key: b"Mulberry",
        ty: CMYK,
        values: [0.34, 0.90, 0.00, 0.02],
    },
    ColorDef {
        key: b"RedViolet",
        ty: CMYK,
        values: [0.07, 0.90, 0.00, 0.34],
    },
    ColorDef {
        key: b"Fuchsia",
        ty: CMYK,
        values: [0.47, 0.91, 0.00, 0.08],
    },
    ColorDef {
        key: b"Lavender",
        ty: CMYK,
        values: [0.00, 0.48, 0.00, 0.00],
    },
    ColorDef {
        key: b"Thistle",
        ty: CMYK,
        values: [0.12, 0.59, 0.00, 0.00],
    },
    ColorDef {
        key: b"Orchid",
        ty: CMYK,
        values: [0.32, 0.64, 0.00, 0.00],
    },
    ColorDef {
        key: b"DarkOrchid",
        ty: CMYK,
        values: [0.40, 0.80, 0.20, 0.00],
    },
    ColorDef {
        key: b"Purple",
        ty: CMYK,
        values: [0.45, 0.86, 0.00, 0.00],
    },
    ColorDef {
        key: b"Plum",
        ty: CMYK,
        values: [0.50, 1.00, 0.00, 0.00],
    },
    ColorDef {
        key: b"Violet",
        ty: CMYK,
        values: [0.79, 0.88, 0.00, 0.00],
    },
    ColorDef {
        key: b"RoyalPurple",
        ty: CMYK,
        values: [0.75, 0.90, 0.00, 0.00],
    },
    ColorDef {
        key: b"BlueViolet",
        ty: CMYK,
        values: [0.86, 0.91, 0.00, 0.04],
    },
    ColorDef {
        key: b"Periwinkle",
        ty: CMYK,
        values: [0.57, 0.55, 0.00, 0.00],
    },
    ColorDef {
        key: b"CadetBlue",
        ty: CMYK,
        values: [0.62, 0.57, 0.23, 0.00],
    },
    ColorDef {
        key: b"CornflowerBlue",
        ty: CMYK,
        values: [0.65, 0.13, 0.00, 0.00],
    },
    ColorDef {
        key: b"MidnightBlue",
        ty: CMYK,
        values: [0.98, 0.13, 0.00, 0.43],
    },
    ColorDef {
        key: b"NavyBlue",
        ty: CMYK,
        values: [0.94, 0.54, 0.00, 0.00],
    },
    ColorDef {
        key: b"RoyalBlue",
        ty: CMYK,
        values: [1.00, 0.50, 0.00, 0.00],
    },
    ColorDef {
        key: b"Blue",
        ty: CMYK,
        values: [1.00, 1.00, 0.00, 0.00],
    },
    ColorDef {
        key: b"Cerulean",
        ty: CMYK,
        values: [0.94, 0.11, 0.00, 0.00],
    },
    ColorDef {
        key: b"Cyan",
        ty: CMYK,
        values: [1.00, 0.00, 0.00, 0.00],
    },
    ColorDef {
        key: b"ProcessBlue",
        ty: CMYK,
        values: [0.96, 0.00, 0.00, 0.00],
    },
    ColorDef {
        key: b"SkyBlue",
        ty: CMYK,
        values: [0.62, 0.00, 0.12, 0.00],
    },
    ColorDef {
        key: b"Turquoise",
        ty: CMYK,
        values: [0.85, 0.00, 0.20, 0.00],
    },
    ColorDef {
        key: b"TealBlue",
        ty: CMYK,
        values: [0.86, 0.00, 0.34, 0.02],
    },
    ColorDef {
        key: b"Aquamarine",
        ty: CMYK,
        values: [0.82, 0.00, 0.30, 0.00],
    },
    ColorDef {
        key: b"BlueGreen",
        ty: CMYK,
        values: [0.85, 0.00, 0.33, 0.00],
    },
    ColorDef {
        key: b"Emerald",
        ty: CMYK,
        values: [1.00, 0.00, 0.50, 0.00],
    },
    ColorDef {
        key: b"JungleGreen",
        ty: CMYK,
        values: [0.99, 0.00, 0.52, 0.00],
    },
    ColorDef {
        key: b"SeaGreen",
        ty: CMYK,
        values: [0.69, 0.00, 0.50, 0.00],
    },
    ColorDef {
        key: b"Green",
        ty: CMYK,
        values: [1.00, 0.00, 1.00, 0.00],
    },
    ColorDef {
        key: b"ForestGreen",
        ty: CMYK,
        values: [0.91, 0.00, 0.88, 0.12],
    },
    ColorDef {
        key: b"PineGreen",
        ty: CMYK,
        values: [0.92, 0.00, 0.59, 0.25],
    },
    ColorDef {
        key: b"LimeGreen",
        ty: CMYK,
        values: [0.50, 0.00, 1.00, 0.00],
    },
    ColorDef {
        key: b"YellowGreen",
        ty: CMYK,
        values: [0.44, 0.00, 0.74, 0.00],
    },
    ColorDef {
        key: b"SpringGreen",
        ty: CMYK,
        values: [0.26, 0.00, 0.76, 0.00],
    },
    ColorDef {
        key: b"OliveGreen",
        ty: CMYK,
        values: [0.64, 0.00, 0.95, 0.40],
    },
    ColorDef {
        key: b"RawSienna",
        ty: CMYK,
        values: [0.00, 0.72, 1.00, 0.45],
    },
    ColorDef {
        key: b"Sepia",
        ty: CMYK,
        values: [0.00, 0.83, 1.00, 0.70],
    },
    ColorDef {
        key: b"Brown",
        ty: CMYK,
        values: [0.00, 0.81, 1.00, 0.60],
    },
    ColorDef {
        key: b"Tan",
        ty: CMYK,
        values: [0.14, 0.42, 0.56, 0.00],
    },
    ColorDef {
        key: b"Gray",
        ty: GRAY,
        values: [0.5, 0.0, 0.0, 0.0],
    },
    ColorDef {
        key: b"Black",
        ty: GRAY,
        values: [0.0, 0.0, 0.0, 0.0],
    },
    ColorDef {
        key: b"White",
        ty: GRAY,
        values: [1.0, 0.0, 0.0, 0.0],
    },
];

/// `skip_blank` (static): spaces and tabs.
fn skip_blank(s: &[u8], pp: &mut usize) {
    todo!()
}

/// `pdf_color_namedcolor` (static): 0, or -1 for an unknown name.
fn pdf_color_namedcolor(color: &mut PdfColor, colorname: &[u8]) -> i32 {
    todo!()
}

/// `rgb_color_from_hsv` (static).
fn rgb_color_from_hsv(color: &mut PdfColor, h: f64, s: f64, v: f64) {
    todo!()
}

/// `make_transmatrix` (static).
fn make_transmatrix(
    m: &mut PdfTmatrix,
    xoffset: f64,
    yoffset: f64,
    xscale: f64,
    yscale: f64,
    rotate: f64,
) {
    todo!()
}

/// `spc_util_read_numbers`: how many numbers were read into `values`
/// (at most `values.len()`, C's `num_values`).
pub fn spc_util_read_numbers(values: &mut [f64], args: &mut SpcArg) -> i32 {
    todo!()
}

impl Dpx {
    /// `spc_util_read_colorspec`: `syntax` nonzero for the color
    /// special's syntax, zero for pdf: specials'.
    pub fn spc_util_read_colorspec(
        &mut self,
        spe: &mut SpcEnv,
        colorspec: &mut PdfColor,
        args: &mut SpcArg,
        syntax: i32,
    ) -> i32 {
        todo!()
    }
    /// `spc_util_read_pdfcolor`.
    pub fn spc_util_read_pdfcolor(
        &mut self,
        spe: &mut SpcEnv,
        colorspec: &mut PdfColor,
        args: &mut SpcArg,
        defaultcolor: Option<&PdfColor>,
    ) -> i32 {
        todo!()
    }
    /// `spc_util_read_dimtrns`: `syntax` nonzero for dvips keys.
    pub fn spc_util_read_dimtrns(
        &mut self,
        spe: &mut SpcEnv,
        dimtrns: &mut TransformInfo,
        args: &mut SpcArg,
        syntax: i32,
    ) -> i32 {
        todo!()
    }
    /// `spc_util_read_blahblah`: dimensions, `page`, `pagebox`
    /// (`bbox_type` is an `enum pdf_page_boundary` value), `named`.
    pub fn spc_util_read_blahblah(
        &mut self,
        spe: &mut SpcEnv,
        dimtrns: &mut TransformInfo,
        page_no: &mut i32,
        bbox_type: &mut i32,
        page_name: &mut Option<Vec<u8>>,
        args: &mut SpcArg,
    ) -> i32 {
        todo!()
    }
}

/// `spc_read_color_color` (static).
fn spc_read_color_color(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    colorspec: &mut PdfColor,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `spc_read_color_pdf` (static).
fn spc_read_color_pdf(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    colorspec: &mut PdfColor,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `spc_util_read_length` (static): status and the length in bp
/// (`true` units divided by `spe.mag`).
fn spc_util_read_length(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> (i32, f64) {
    todo!()
}

/// `spc_read_dimtrns_dvips` (static).
fn spc_read_dimtrns_dvips(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    t: &mut TransformInfo,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}

/// `spc_read_dimtrns_pdfm` (static).
fn spc_read_dimtrns_pdfm(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    p: &mut TransformInfo,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
