//! spc_util.c, spc_util.h: reading colors, lengths and dimension/transform
//! keys in specials.
//!
//! In/out parameters (a color or a `transform_info` the caller set up,
//! left alone on some errors) stay `&mut`.

use crate::dpxutil::{parse_c_ident, parse_c_string, parse_float_decimal};
use crate::fmt::atof;
use crate::parse::{parse_opt_ident, skip_white};
use crate::pdfcolor::*;
use crate::pdfdev::{
    INFO_DO_CLIP, INFO_DO_HIDE, INFO_HAS_HEIGHT, INFO_HAS_USER_BBOX, INFO_HAS_WIDTH, PdfTmatrix,
    TransformInfo,
};
use crate::pdfdoc::PdfPageBoundary;
use crate::pdfdraw::pdf_setmatrix;
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
    while *pp < s.len() && (s[*pp] == b' ' || s[*pp] == b'\t') {
        *pp += 1;
    }
}

/// `ap->curptr[0]`: the byte at the current position (C may read the
/// byte at `endptr`, the buffer's byte there, or 0 past the buffer).
fn cur(ap: &SpcArg) -> u8 {
    ap.buf.get(ap.curptr).copied().unwrap_or(0)
}

/// `skip_blank(&ap->curptr, ap->endptr)`.
fn ap_skip_blank(ap: &mut SpcArg) {
    let end = ap.endptr;
    skip_blank(&ap.buf[..end], &mut ap.curptr);
}

/// `parse_c_ident(&ap->curptr, ap->endptr)`.
fn ap_c_ident(ap: &mut SpcArg) -> Option<Vec<u8>> {
    let end = ap.endptr;
    parse_c_ident(&ap.buf[..end], &mut ap.curptr)
}

/// `parse_float_decimal(&ap->curptr, ap->endptr)`.
fn ap_float(ap: &mut SpcArg) -> Option<Vec<u8>> {
    let end = ap.endptr;
    parse_float_decimal(&ap.buf[..end], &mut ap.curptr)
}

/// `parse_opt_ident(&ap->curptr, ap->endptr)` (C goes on with a null
/// ident when there is none; here an empty one).
fn ap_opt_ident(ap: &mut SpcArg) -> Vec<u8> {
    let end = ap.endptr;
    parse_opt_ident(&ap.buf[..end], &mut ap.curptr).unwrap_or_default()
}

/// `pdf_color_namedcolor` (static): 0, or -1 for an unknown name.
fn pdf_color_namedcolor(color: &mut PdfColor, colorname: &[u8]) -> i32 {
    for def in &COLORDEFS {
        if def.key == colorname {
            let mut values = [0.0; PDF_COLOR_COMPONENT_MAX];
            values[..4].copy_from_slice(&def.values);
            color.pdf_color_copycolor(&PdfColor {
                res_id: -1,
                r#type: def.ty,
                num_components: if def.ty == GRAY { 1 } else { 4 },
                spot_color_name: None,
                values,
                pattern_id: -1,
            });
            return 0;
        }
    }
    -1
}

/// `rgb_color_from_hsv` (static).
fn rgb_color_from_hsv(color: &mut PdfColor, h: f64, s: f64, v: f64) {
    let (mut r, mut g, mut b) = (v, v, v);
    if s != 0.0 {
        let h6 = h * 6.0; /* 360 / 60 */
        let i = h6 as i32;
        let f = h6 - f64::from(i);
        let v1 = v * (1.0 - s);
        let v2 = v * (1.0 - s * f);
        let v3 = v * (1.0 - s * (1.0 - f));
        match i {
            0 => (r, g, b) = (v, v3, v1),
            1 => (r, g, b) = (v2, v, v1),
            2 => (r, g, b) = (v1, v, v3),
            3 => (r, g, b) = (v1, v2, v),
            4 => (r, g, b) = (v3, v1, v),
            5 | 6 => (r, g, b) = (v, v1, v2),
            _ => {}
        }
    }
    color.pdf_color_rgbcolor(r, g, b);
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
    let c = libm::cos(rotate);
    let s = libm::sin(rotate);

    m.a = xscale * c;
    m.b = xscale * s;
    m.c = -yscale * s;
    m.d = yscale * c;
    m.e = xoffset;
    m.f = yoffset;
}

/// `spc_util_read_numbers`: how many numbers were read into `values`
/// (at most `values.len()`, C's `num_values`).
pub fn spc_util_read_numbers(values: &mut [f64], args: &mut SpcArg) -> i32 {
    let mut count = 0;

    ap_skip_blank(args);
    while count < values.len() && args.curptr < args.endptr {
        let Some(q) = ap_float(args) else {
            break;
        };
        values[count] = atof(&q);
        ap_skip_blank(args);
        count += 1;
    }

    count as i32
}

/// `strcasecmp(a, b) == 0`.
fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
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
    ) -> Result<i32> {
        ap_skip_blank(args);
        if args.curptr >= args.endptr {
            return Ok(-1);
        }

        colorspec.pdf_color_black(); /* As initialization... */
        if syntax != 0 {
            Ok(spc_read_color_color(self, spe, colorspec, args))
        } else {
            spc_read_color_pdf(self, spe, colorspec, args)
        }
    }
    /// `spc_util_read_pdfcolor`.
    pub fn spc_util_read_pdfcolor(
        &mut self,
        spe: &mut SpcEnv,
        colorspec: &mut PdfColor,
        args: &mut SpcArg,
        defaultcolor: Option<&PdfColor>,
    ) -> Result<i32> {
        ap_skip_blank(args);
        if args.curptr >= args.endptr {
            return Ok(-1);
        }
        let mut error = spc_read_color_pdf(self, spe, colorspec, args)?;
        if error < 0
            && let Some(d) = defaultcolor
        {
            colorspec.pdf_color_copycolor(d);
            error = 0;
        }
        Ok(error)
    }
    /// `spc_util_read_dimtrns`: `syntax` nonzero for dvips keys.
    pub fn spc_util_read_dimtrns(
        &mut self,
        spe: &mut SpcEnv,
        dimtrns: &mut TransformInfo,
        args: &mut SpcArg,
        syntax: i32,
    ) -> i32 {
        if syntax != 0 {
            spc_read_dimtrns_dvips(self, spe, dimtrns, args)
        } else {
            spc_read_dimtrns_pdfm(self, spe, dimtrns, args)
        }
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
        read_dimtrns_keys(
            self,
            spe,
            dimtrns,
            args,
            Some((page_no, bbox_type, page_name)),
        )
    }
}

/// `spc_read_color_color` (static).
fn spc_read_color_color(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    colorspec: &mut PdfColor,
    ap: &mut SpcArg,
) -> i32 {
    let mut cv = [0.0; 4];
    let mut error = 0;

    let Some(q) = ap_c_ident(ap) else {
        dpx.spc_warn(spe, format_args!("No valid color specified?"));
        return -1;
    };
    ap_skip_blank(ap);

    match &q[..] {
        b"rgb" => {
            /* Handle rgb color */
            let nc = spc_util_read_numbers(&mut cv[..3], ap);
            if nc != 3 {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid value for RGB color specification."),
                );
                error = -1;
            } else {
                colorspec.pdf_color_rgbcolor(cv[0], cv[1], cv[2]);
            }
        }
        b"cmyk" => {
            /* Handle cmyk color */
            let nc = spc_util_read_numbers(&mut cv[..4], ap);
            if nc != 4 {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid value for CMYK color specification."),
                );
                error = -1;
            } else {
                colorspec.pdf_color_cmykcolor(cv[0], cv[1], cv[2], cv[3]);
            }
        }
        b"gray" => {
            /* Handle gray */
            let nc = spc_util_read_numbers(&mut cv[..1], ap);
            if nc != 1 {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid value for gray color specification."),
                );
                error = -1;
            } else {
                colorspec.pdf_color_graycolor(cv[0]);
            }
        }
        b"spot" => {
            /* Handle spot colors */
            let Some(color_name) = ap_c_ident(ap) else {
                dpx.spc_warn(spe, format_args!("No valid spot color name specified?"));
                return -1;
            };
            ap_skip_blank(ap);
            let nc = spc_util_read_numbers(&mut cv[..1], ap);
            if nc != 1 {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid value for spot color specification."),
                );
                error = -1;
            } else {
                colorspec.pdf_color_spotcolor(&color_name, cv[0]);
            }
        }
        b"hsb" => {
            let nc = spc_util_read_numbers(&mut cv[..3], ap);
            if nc != 3 {
                dpx.spc_warn(
                    spe,
                    format_args!("Invalid value for HSB color specification."),
                );
                error = -1;
            } else {
                rgb_color_from_hsv(colorspec, cv[0], cv[1], cv[2]);
                dpx.spc_warn(
                    spe,
                    format_args!(
                        "HSB color converted to RGB: hsb: <{}, {}, {}> ==> rgb: <{}, {}, {}>",
                        cv[0],
                        cv[1],
                        cv[2],
                        colorspec.values[0],
                        colorspec.values[1],
                        colorspec.values[2]
                    ),
                );
            }
        }
        _ => {
            /* Must be a "named" color */
            error = pdf_color_namedcolor(colorspec, &q);
            if error != 0 {
                dpx.spc_warn(spe, format_args!("Unrecognized color name"));
            }
        }
    }

    error
}

/// The pattern reference after a Pattern color (`@name`): its resource
/// id, or none for C's `return -1`.
fn read_pattern_ref(dpx: &mut Dpx, ap: &mut SpcArg) -> Result<Option<i32>> {
    /* reference appears */
    ap_skip_blank(ap);
    if cur(ap) != b'@' {
        warn!("An object reference expected but not found for Pattern!");
        return Ok(None);
    }
    let ident = ap_opt_ident(ap);
    let mut res_id = dpx.pdf_findresource(b"Pattern", &ident)?;
    if res_id < 0 {
        let Some(pattern) = dpx.spc_lookup_object(&ident)? else {
            // (C links the NULL it found)
            fatal!("pdf_link_obj(): passed invalid object.");
        };
        /* Skip checking. /Type entry is optional... */
        let p = dpx.o.link(pattern)?;
        res_id = dpx.pdf_defineresource(b"Pattern", Some(&ident), p, 0)?;
    }
    Ok(Some(res_id))
}

/// The closing `]` of a color in brackets; the error, or 0.
fn close_bracket(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg, msg: &str) -> i32 {
    ap_skip_blank(ap);
    if ap.curptr >= ap.endptr || cur(ap) != b']' {
        dpx.spc_warn(spe, format_args!("{msg}"));
        -1
    } else {
        ap.curptr += 1;
        0
    }
}

/// `spc_read_color_pdf` (static).
fn spc_read_color_pdf(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    colorspec: &mut PdfColor,
    ap: &mut SpcArg,
) -> Result<i32> {
    let mut cv = [0.0; PDF_COLOR_COMPONENT_MAX]; /* dvipdfmx limit */
    let mut nc: i32 = -1;
    let mut ty = PDF_COLORSPACE_TYPE_INVALID;
    let mut isarry = false;
    let mut error = 0;

    ap_skip_blank(ap);
    if cur(ap) == b'@' {
        let ident = ap_opt_ident(ap);
        ap_skip_blank(ap);
        let mut res_id = dpx.pdf_findresource(b"ColorSpace", &ident)?;
        {
            let cspace = dpx.spc_lookup_object(&ident)?;
            let Some(cspace) = cspace.filter(|&c| dpx.o.is_array(Some(c))) else {
                warn!("Couldn't find ColorSpace resource (or not an array object?)");
                return Ok(-1);
            };
            let csname = dpx.o.get_array(cspace, 0)?;
            let Some(csname) = csname.filter(|&c| dpx.o.is_name(Some(c))) else {
                warn!("Invalid ColorSpace resource found...");
                return Ok(-1);
            };

            match dpx.o.name_value(csname)? {
                b"Separation" => ty = PDF_COLORSPACE_TYPE_SEPARATION,
                b"CalGray" => {
                    ty = PDF_COLORSPACE_TYPE_CALGRAY;
                    nc = 1;
                }
                b"CalRGB" => {
                    ty = PDF_COLORSPACE_TYPE_CALRGB;
                    nc = 3;
                }
                b"LAB" => {
                    ty = PDF_COLORSPACE_TYPE_LAB;
                    nc = 3;
                }
                b"ICCBased" => {
                    ty = PDF_COLORSPACE_TYPE_ICCBASED;
                    nc = -1;
                }
                b"DeviceN" => {
                    ty = PDF_COLORSPACE_TYPE_DEVICEN;
                    nc = -1;
                }
                b"Indexed" => {
                    ty = PDF_COLORSPACE_TYPE_INDEXED;
                    nc = 1;
                }
                b"Pattern" => {
                    ty = PDF_COLORSPACE_TYPE_PATTERN;
                    nc = -1;
                }
                _ => {
                    warn!("Specified object not a ColorSpace???");
                    return Ok(-1);
                }
            }
            if res_id < 0 {
                let c = dpx.o.link(cspace)?;
                res_id = dpx.pdf_defineresource(b"ColorSpace", Some(&ident), c, 0)?;
            }
        }

        colorspec.res_id = res_id;
        colorspec.r#type = ty;
        if cur(ap) == b'[' {
            ap.curptr += 1;
            ap_skip_blank(ap);
            isarry = true;
        }
        if nc > 0 {
            let n = spc_util_read_numbers(&mut cv[..nc as usize], ap);
            if n != nc {
                warn!("Wrong number of color components...");
                return Ok(-1);
            }
        } else {
            nc = spc_util_read_numbers(&mut cv[..PDF_COLOR_COMPONENT_MAX], ap);
        }
        colorspec.num_components = nc;
        while nc > 0 {
            nc -= 1;
            colorspec.values[nc as usize] = cv[nc as usize];
        }
        if ty == PDF_COLORSPACE_TYPE_PATTERN {
            let Some(res_id) = read_pattern_ref(dpx, ap)? else {
                return Ok(-1);
            };
            colorspec.pattern_id = res_id;
        }
        if isarry {
            error = close_bracket(
                dpx,
                spe,
                ap,
                "Unbalanced '[' and ']' or wrong number of color components in color specification?",
            );
        }
    } else if cur(ap) == b'/' {
        /* /DeviceGray... */
        let end = ap.endptr;
        let csname = dpx.o.parse_pdf_name(&ap.buf[..end], &mut ap.curptr);
        let Some(csname) = csname else {
            warn!("Failed to read a name object while parsing colorspecification...");
            return Ok(-1);
        };
        match dpx.o.name_value(csname)? {
            b"DeviceGray" => {
                ty = PDF_COLORSPACE_TYPE_DEVICEGRAY;
                nc = 1;
            }
            b"DeviceRGB" => {
                ty = PDF_COLORSPACE_TYPE_DEVICERGB;
                nc = 3;
            }
            b"DeviceCMYK" => {
                ty = PDF_COLORSPACE_TYPE_DEVICECMYK;
                nc = 4;
            }
            b"Pattern" => {
                ty = PDF_COLORSPACE_TYPE_PATTERN;
                nc = 0;
            }
            _ => {
                warn!("Unknown ColorSpace name specified");
                return Ok(-1);
            }
        }
        dpx.o.release(csname)?;
        ap_skip_blank(ap);
        colorspec.res_id = -1;
        colorspec.r#type = ty;
        if cur(ap) == b'[' {
            ap.curptr += 1;
            ap_skip_blank(ap);
            isarry = true;
        }
        {
            let n = spc_util_read_numbers(&mut cv[..nc as usize], ap);
            if n != nc {
                warn!("Wrong number of color components for this ColorSpace...");
                return Ok(-1);
            }
        }
        colorspec.num_components = nc;
        while nc > 0 {
            nc -= 1;
            colorspec.values[nc as usize] = cv[nc as usize];
        }
        if ty == PDF_COLORSPACE_TYPE_PATTERN {
            let Some(res_id) = read_pattern_ref(dpx, ap)? else {
                return Ok(-1);
            };
            colorspec.pattern_id = res_id;
        }
        if isarry {
            error = close_bracket(
                dpx,
                spe,
                ap,
                "Unbalanced '[' and ']' or wrong number of color components in color specification?",
            );
        }
    } else {
        if cur(ap) == b'[' {
            ap.curptr += 1;
            ap_skip_blank(ap);
            isarry = true;
        }
        nc = spc_util_read_numbers(&mut cv[..4], ap);
        match nc {
            1 => {
                colorspec.pdf_color_graycolor(cv[0]);
            }
            3 => {
                colorspec.pdf_color_rgbcolor(cv[0], cv[1], cv[2]);
            }
            4 => {
                colorspec.pdf_color_cmykcolor(cv[0], cv[1], cv[2], cv[3]);
            }
            _ => {
                /* Try to read the color names defined in dvipsname.def */
                let Some(q) = ap_c_ident(ap) else {
                    dpx.spc_warn(spe, format_args!("No valid color specified?"));
                    return Ok(-1);
                };
                error = pdf_color_namedcolor(colorspec, &q);
                if error != 0 {
                    dpx.spc_warn(
                        spe,
                        format_args!("Unrecognized color name, keep the current color"),
                    );
                }
            }
        }
        if isarry {
            error = close_bracket(
                dpx,
                spe,
                ap,
                "Unbalanced '[' and ']' in color specification.",
            );
        }
    }

    Ok(error)
}

/// `spc_util_read_length` (static): status and the length in bp
/// (`true` units divided by `spe.mag`); no length when no number was
/// read (C leaves `*vp` alone then).
fn spc_util_read_length(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> (i32, Option<f64>) {
    const UKEYS: [&[u8]; 9] = [
        b"pt", b"in", b"cm", b"mm", b"bp", b"pc", b"dd", b"cc", b"sp",
    ];
    let mut u = 1.0;
    let mut error = 0;

    let Some(q) = ap_float(ap) else {
        return (-1, None);
    };

    let v = atof(&q);

    let end = ap.endptr;
    skip_white(&ap.buf[..end], &mut ap.curptr);
    if let Some(qq) = ap_c_ident(ap) {
        let mut q = Some(qq);
        if let Some(t) = &q
            && t.len() >= 4
            && t.starts_with(b"true")
        {
            u /= if spe.mag != 0.0 { spe.mag } else { 1.0 }; /* inverse magnify */
            let rest = t[4..].to_vec();

            if rest.is_empty() {
                skip_white(&ap.buf[..end], &mut ap.curptr);
                q = ap_c_ident(ap);
            } else {
                q = Some(rest);
            }
        }

        if let Some(q) = q {
            match UKEYS.iter().position(|k| *k == &q[..]) {
                Some(0) => u *= 72.0 / 72.27,
                Some(1) => u *= 72.0,
                Some(2) => u *= 72.0 / 2.54,
                Some(3) => u *= 72.0 / 25.4,
                Some(4) => u *= 1.0,
                Some(5) => u *= 12.0 * 72.0 / 72.27,
                Some(6) => u *= 1238.0 / 1157.0 * 72.0 / 72.27,
                Some(7) => u *= 12.0 * 1238.0 / 1157.0 * 72.0 / 72.27,
                Some(8) => u *= 72.0 / (72.27 * 65536.0),
                _ => {
                    dpx.spc_warn(spe, format_args!("Unknown unit of measure"));
                    error = -1;
                }
            }
        } else {
            dpx.spc_warn(spe, format_args!("Missing unit of measure after \"true\""));
            error = -1;
        }
    }

    (error, Some(v * u))
}

/// `spc_read_dimtrns_dvips` (static).
fn spc_read_dimtrns_dvips(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    t: &mut TransformInfo,
    ap: &mut SpcArg,
) -> i32 {
    const DTKEYS: [&[u8]; 14] = [
        b"hoffset", b"voffset", b"hsize", b"vsize", b"hscale", b"vscale", b"angle", b"clip",
        b"llx", b"lly", b"urx", b"ury", b"rwi", b"rhi",
    ];
    const K__CLIP: usize = 7;
    let (mut xoffset, mut yoffset, mut rotate) = (0.0, 0.0, 0.0);
    let (mut xscale, mut yscale) = (1.0, 1.0);
    let mut error = 0;

    ap_skip_blank(ap);
    while error == 0 && ap.curptr < ap.endptr {
        let Some(kp) = ap_c_ident(ap) else {
            break;
        };

        let Some(k) = DTKEYS.iter().position(|key| *key == &kp[..]) else {
            dpx.spc_warn(
                spe,
                format_args!("Unrecognized dimension/transformation key"),
            );
            error = -1;
            break;
        };

        ap_skip_blank(ap);
        if k == K__CLIP {
            t.flags |= INFO_DO_CLIP;
            continue; /* not key-value */
        }

        if ap.curptr < ap.endptr && cur(ap) == b'=' {
            ap.curptr += 1;
            ap_skip_blank(ap);
        }

        let mut vp;
        if cur(ap) == b'\'' || cur(ap) == b'"' {
            let qchr = cur(ap);
            ap.curptr += 1;
            ap_skip_blank(ap);
            vp = ap_float(ap);
            ap_skip_blank(ap);
            if vp.is_some() && qchr != cur(ap) {
                dpx.spc_warn(
                    spe,
                    format_args!("Syntax error in dimension/transformation specification."),
                );
                error = -1;
                vp = None;
            }
            ap.curptr += 1;
        } else {
            vp = ap_float(ap);
        }
        if error == 0 && vp.is_none() {
            dpx.spc_warn(
                spe,
                format_args!("Missing value for dimension/transformation"),
            );
            error = -1;
        }
        let Some(vp) = vp else {
            break;
        };
        if error != 0 {
            break;
        }

        let v = atof(&vp);
        match k {
            0 => xoffset = v,
            1 => yoffset = v,
            2 => {
                t.width = v;
                t.flags |= INFO_HAS_WIDTH;
            }
            3 => {
                t.height = v;
                t.flags |= INFO_HAS_HEIGHT;
            }
            4 => xscale = v / 100.0,
            5 => yscale = v / 100.0,
            6 => rotate = core::f64::consts::PI * v / 180.0,
            8 => {
                t.bbox.llx = v;
                t.flags |= INFO_HAS_USER_BBOX;
            }
            9 => {
                t.bbox.lly = v;
                t.flags |= INFO_HAS_USER_BBOX;
            }
            10 => {
                t.bbox.urx = v;
                t.flags |= INFO_HAS_USER_BBOX;
            }
            11 => {
                t.bbox.ury = v;
                t.flags |= INFO_HAS_USER_BBOX;
            }
            12 => {
                t.width = v / 10.0;
                t.flags |= INFO_HAS_WIDTH;
            }
            13 => {
                t.height = v / 10.0;
                t.flags |= INFO_HAS_HEIGHT;
            }
            _ => {}
        }
        ap_skip_blank(ap);
    }
    make_transmatrix(&mut t.matrix, xoffset, yoffset, xscale, yscale, rotate);

    error
}

/// `spc_read_dimtrns_pdfm` (static).
fn spc_read_dimtrns_pdfm(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    p: &mut TransformInfo,
    ap: &mut SpcArg,
) -> i32 {
    read_dimtrns_keys(dpx, spe, p, ap, None)
}

/// The body of `spc_read_dimtrns_pdfm` and, with its `page`, `pagebox`
/// and `named` keys, of `spc_util_read_blahblah` (the same code twice
/// in C).
fn read_dimtrns_keys(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    p: &mut TransformInfo,
    ap: &mut SpcArg,
    mut extra: Option<(&mut i32, &mut i32, &mut Option<Vec<u8>>)>,
) -> i32 {
    const DTKEYS: [&[u8]; 14] = [
        b"width", b"height", b"depth", b"scale", b"xscale", b"yscale", b"rotate", b"bbox",
        b"matrix", b"clip", b"hide", b"page", b"pagebox", b"named",
    ];
    let nkeys = if extra.is_some() { 14 } else { 11 };
    let (mut has_scale, mut has_xscale, mut has_yscale, mut has_rotate, mut has_matrix) =
        (false, false, false, false, false);
    let (mut xscale, mut yscale, mut rotate) = (1.0, 1.0, 0.0);
    let mut error = 0;

    p.flags |= INFO_DO_CLIP; /* default: do clipping */
    p.flags &= !INFO_DO_HIDE; /* default: do clipping */

    ap_skip_blank(ap);

    while error == 0 && ap.curptr < ap.endptr {
        let Some(kp) = ap_c_ident(ap) else {
            break;
        };

        ap_skip_blank(ap);
        let k = DTKEYS[..nkeys]
            .iter()
            .position(|key| *key == &kp[..])
            .unwrap_or(usize::MAX);
        match k {
            0 | 1 | 2 => {
                let (e, v) = spc_util_read_length(dpx, spe, ap);
                error = e;
                if let Some(v) = v {
                    match k {
                        0 => p.width = v,
                        1 => p.height = v,
                        _ => p.depth = v,
                    }
                }
                p.flags |= if k == 0 {
                    INFO_HAS_WIDTH
                } else {
                    INFO_HAS_HEIGHT
                };
            }
            3 | 4 | 5 | 6 => match ap_float(ap) {
                None => error = -1,
                Some(vp) => {
                    let v = atof(&vp);
                    match k {
                        3 => {
                            xscale = v;
                            yscale = v;
                            has_scale = true;
                        }
                        4 => {
                            xscale = v;
                            has_xscale = true;
                        }
                        5 => {
                            yscale = v;
                            has_yscale = true;
                        }
                        _ => {
                            rotate = core::f64::consts::PI * v / 180.0;
                            has_rotate = true;
                        }
                    }
                }
            },
            7 => {
                let mut v = [0.0; 4];
                if spc_util_read_numbers(&mut v, ap) != 4 {
                    error = -1;
                } else {
                    p.bbox.llx = v[0];
                    p.bbox.lly = v[1];
                    p.bbox.urx = v[2];
                    p.bbox.ury = v[3];
                    p.flags |= INFO_HAS_USER_BBOX;
                }
            }
            8 => {
                let mut v = [0.0; 6];
                if spc_util_read_numbers(&mut v, ap) != 6 {
                    error = -1;
                } else {
                    pdf_setmatrix(&mut p.matrix, v[0], v[1], v[2], v[3], v[4], v[5]);
                    has_matrix = true;
                }
            }
            9 => match ap_float(ap) {
                None => error = -1,
                Some(vp) => {
                    if atof(&vp) != 0.0 {
                        p.flags |= INFO_DO_CLIP;
                    } else {
                        p.flags &= !INFO_DO_CLIP;
                    }
                }
            },
            10 => {
                p.flags |= INFO_DO_HIDE;
            }
            11 => {
                let mut page = [0.0];
                if spc_util_read_numbers(&mut page, ap) == 1 {
                    if let Some((page_no, _, _)) = &mut extra {
                        **page_no = page[0] as i32;
                    }
                } else {
                    error = -1;
                }
            }
            12 => {
                let bbox_type = &mut extra.as_mut().unwrap().1;
                if let Some(q) = ap_c_ident(ap) {
                    if eq_ignore_case(&q, b"cropbox") {
                        **bbox_type = PdfPageBoundary::CropBox as i32;
                    } else if eq_ignore_case(&q, b"mediabox") {
                        **bbox_type = PdfPageBoundary::MediaBox as i32;
                    } else if eq_ignore_case(&q, b"artbox") {
                        **bbox_type = PdfPageBoundary::ArtBox as i32;
                    } else if eq_ignore_case(&q, b"trimbox") {
                        **bbox_type = PdfPageBoundary::TrimBox as i32;
                    } else if eq_ignore_case(&q, b"bleedbox") {
                        **bbox_type = PdfPageBoundary::BleedBox as i32;
                    }
                } else {
                    **bbox_type = PdfPageBoundary::Auto as i32;
                }
            }
            13 => {
                let end = ap.endptr;
                let name = parse_c_string(&ap.buf[..end], &mut ap.curptr);
                *extra.as_mut().unwrap().2 = name;
            }
            _ => {
                error = -1;
            }
        }
        if error != 0 {
            dpx.spc_warn(
                spe,
                format_args!("Unrecognized key or invalid value for dimension/transformation"),
            );
        } else {
            ap_skip_blank(ap);
        }
    }

    if error == 0 {
        /* Check consistency */
        if has_xscale && p.flags & INFO_HAS_WIDTH != 0 {
            dpx.spc_warn(
                spe,
                format_args!("Can't supply both width and xscale. Ignore xscale."),
            );
            xscale = 1.0;
        } else if has_yscale && p.flags & INFO_HAS_HEIGHT != 0 {
            dpx.spc_warn(
                spe,
                format_args!("Can't supply both height/depth and yscale. Ignore yscale."),
            );
            yscale = 1.0;
        } else if has_scale && (has_xscale || has_yscale) {
            dpx.spc_warn(
                spe,
                format_args!("Can't supply overall scale along with axis scales."),
            );
            error = -1;
        } else if has_matrix && (has_scale || has_xscale || has_yscale || has_rotate) {
            dpx.spc_warn(spe, format_args!("Can't supply transform matrix along with scales or rotate. Ignore scales and rotate."));
        }
    }

    if !has_matrix {
        make_transmatrix(&mut p.matrix, 0.0, 0.0, xscale, yscale, rotate);
    }

    if p.flags & INFO_HAS_USER_BBOX == 0 {
        p.flags &= !INFO_DO_CLIP; /* no clipping needed */
    }

    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors() {
        let mut c = PdfColor::default();
        assert_eq!(pdf_color_namedcolor(&mut c, b"Orange"), 0);
        assert_eq!(c.r#type, CMYK);
        assert_eq!(c.num_components, 4);
        assert_eq!(&c.values[..4], &[0.0, 0.61, 0.87, 0.0]);
        assert_eq!(pdf_color_namedcolor(&mut c, b"Gray"), 0);
        assert_eq!((c.r#type, c.num_components, c.values[0]), (GRAY, 1, 0.5));
        assert_eq!(pdf_color_namedcolor(&mut c, b"Grey"), -1);
    }

    #[test]
    fn hsb() {
        let mut c = PdfColor::default();
        rgb_color_from_hsv(&mut c, 0.0, 1.0, 1.0);
        assert_eq!(&c.values[..3], &[1.0, 0.0, 0.0]);
        rgb_color_from_hsv(&mut c, 0.5, 0.0, 0.25);
        assert_eq!(&c.values[..3], &[0.25, 0.25, 0.25]);
    }

    #[test]
    fn transmatrix() {
        let mut m = PdfTmatrix::default();
        make_transmatrix(&mut m, 1.0, 2.0, 2.0, 3.0, 0.0);
        assert_eq!(
            m,
            PdfTmatrix {
                a: 2.0,
                b: 0.0,
                c: -0.0,
                d: 3.0,
                e: 1.0,
                f: 2.0
            }
        );
    }
}
