//! spc_xtx.c, spc_xtx.h: XeTeX's `x:` specials.

use crate::dpxutil::parse_c_ident;
use crate::pdfcolor::PdfColor;
use crate::pdfdev::{PdfCoord, PdfTmatrix};
use crate::prelude::*;
use crate::spc_util::spc_util_read_numbers;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, cstr};

/// spc_xtx.c's `static` state.
#[derive(Clone, Debug)]
pub struct State {
    /// `scaleFactors`: the `bscale` stack (C's array; `escale` pops).
    pub scale_factors: Vec<PdfCoord>,
    /// `scaleFactorCount` (C starts it at -1: the index of the top).
    pub scale_factor_count: i32,
    /// `overlay_name` (C's `char[256]`, without the NUL).
    pub overlay_name: Vec<u8>,
}

impl Default for State {
    fn default() -> Self {
        State {
            scale_factors: Vec::new(),
            scale_factor_count: -1,
            overlay_name: Vec::new(),
        }
    }
}

/// `M_PI`.
const M_PI: f64 = core::f64::consts::PI;

impl Dpx {
    /// `spc_handler_xtx_do_transform`: concatenates the matrix around
    /// (`x_user`, `y_user`) and moves the fixed point.
    pub fn spc_handler_xtx_do_transform(
        &mut self,
        spe: &mut SpcEnv,
        x_user: f64,
        y_user: f64,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        e: f64,
        f: f64,
    ) -> Result<i32> {
        // Create transformation matrix
        let mut m = PdfTmatrix {
            a,
            b,
            c,
            d,
            e: 0.0,
            f: 0.0,
        };
        m.e = ((1.0 - m.a) * x_user - m.c * y_user) + e;
        m.f = ((1.0 - m.d) * y_user - m.b * x_user) + f;

        self.pdf_dev_concat(&m)?;
        let (ptx, pty) = self.spc_get_fixed_point(spe);
        self.spc_set_fixed_point(spe, x_user - ptx, y_user - pty);

        Ok(0)
    }
}

/// `spc_handler_xtx_scale`.
fn spc_handler_xtx_scale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut values = [0.0f64; 2];

    if spc_util_read_numbers(&mut values, args) < 2 {
        return Ok(-1);
    }
    args.curptr = args.endptr;

    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_handler_xtx_do_transform(spe, x, y, values[0], 0.0, 0.0, values[1], 0.0, 0.0)
}

/// `spc_handler_xtx_bscale`: scale without gsave/grestore.
fn spc_handler_xtx_bscale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut values = [0.0f64; 2];

    dpx.xtx.scale_factor_count += 1;
    let count = dpx.xtx.scale_factor_count;
    if count & 0x0f == 0 {
        // realloc to scaleFactorCount + 16 entries
        dpx.xtx
            .scale_factors
            .resize((count + 16) as usize, PdfCoord::default());
    }
    if spc_util_read_numbers(&mut values, args) < 2 {
        return Ok(-1);
    }
    if libm::fabs(values[0]) < 1.0e-7 || libm::fabs(values[1]) < 1.0e-7 {
        return Ok(-1);
    }
    dpx.xtx.scale_factors[count as usize] = PdfCoord {
        x: 1.0 / values[0],
        y: 1.0 / values[1],
    };
    args.curptr = args.endptr;

    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_handler_xtx_do_transform(spe, x, y, values[0], 0.0, 0.0, values[1], 0.0, 0.0)
}

/// `spc_handler_xtx_escale`.
fn spc_handler_xtx_escale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let count = dpx.xtx.scale_factor_count;
    if count < 0 || count as usize >= dpx.xtx.scale_factors.len() {
        // C reads outside its array here.
        crate::fatal!("x:escale without x:bscale");
    }
    let factor = dpx.xtx.scale_factors[count as usize];
    dpx.xtx.scale_factor_count -= 1;

    args.curptr = args.endptr;

    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_handler_xtx_do_transform(spe, x, y, factor.x, 0.0, 0.0, factor.y, 0.0, 0.0)
}

/// `spc_handler_xtx_rotate`.
fn spc_handler_xtx_rotate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let mut value = [0.0f64; 1];

    if spc_util_read_numbers(&mut value, args) < 1 {
        return Ok(-1);
    }
    args.curptr = args.endptr;

    let value = value[0];
    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_handler_xtx_do_transform(
        spe,
        x,
        y,
        libm::cos(value * M_PI / 180.0),
        libm::sin(value * M_PI / 180.0),
        -libm::sin(value * M_PI / 180.0),
        libm::cos(value * M_PI / 180.0),
        0.0,
        0.0,
    )
}

/// `spc_handler_xtx_gsave` (also used by the dvips specials).
pub fn spc_handler_xtx_gsave(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_gsave()?;
    dpx.spc_dup_fixed_point(spe);
    Ok(0)
}

/// `spc_handler_xtx_grestore` (also used by the dvips specials).
pub fn spc_handler_xtx_grestore(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_grestore()?;
    dpx.spc_pop_fixed_point(spe);

    // Unfortunately, the following line is necessary in case of a font
    // or color change inside of the save/restore pair. We act like we
    // are starting a new page.
    dpx.pdf_dev_reset_fonts(0);
    dpx.pdf_dev_reset_color(0)?;
    dpx.pdf_dev_reset_xgstate(0)?;

    Ok(0)
}

/// `spc_handler_xtx_papersize` (does nothing).
fn spc_handler_xtx_papersize(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    Ok(0)
}

/// `spc_handler_xtx_backgroundcolor`.
fn spc_handler_xtx_backgroundcolor(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> Result<i32> {
    let mut colorspec = PdfColor::default();

    let error = dpx.spc_util_read_colorspec(spe, &mut colorspec, args, 0)?;
    if error != 0 {
        dpx.spc_warn(spe, format_args!("No valid color specified?"));
    } else {
        dpx.pdf_doc_set_bgcolor(Some(&colorspec));
    }

    Ok(error)
}

/// `spc_handler_xtx_fontmapline` (C's copy of `pdf:mapline`).
fn spc_handler_xtx_fontmapline(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> Result<i32> {
    crate::spc_pdfm::spc_fontmapline(dpx, spe, ap, "fontmapline")
}

/// `spc_handler_xtx_fontmapfile` (C's copy of `pdf:mapfile`).
fn spc_handler_xtx_fontmapfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    crate::spc_pdfm::spc_fontmapfile(dpx, spe, args)
}

/// `spc_handler_xtx_initoverlay`.
fn spc_handler_xtx_initoverlay(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr >= args.endptr {
        return Ok(-1);
    }
    // strncpy: up to a NUL.
    dpx.xtx.overlay_name = cstr(args.rest()).to_vec();

    args.curptr = args.endptr;
    Ok(0)
}

/// `strncmp(a, b, n) != 0` on C strings (`b` may end before a NUL).
fn strncmp_ne(a: &[u8], b: &[u8], n: usize) -> bool {
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return true;
        }
        if x == 0 {
            return false;
        }
    }
    false
}

/// `spc_handler_xtx_clipoverlay`.
fn spc_handler_xtx_clipoverlay(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr >= args.endptr {
        return Ok(-1);
    }
    dpx.pdf_dev_grestore()?;
    dpx.pdf_dev_gsave()?;
    let name = &dpx.xtx.overlay_name;
    if strncmp_ne(name, args.rest(), name.len()) && strncmp_ne(b"all", args.rest(), 3) {
        dpx.pdf_doc_add_page_content(b" 0 0 m W n")?;
    }

    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_xtx_renderingmode`.
fn spc_handler_xtx_renderingmode(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> Result<i32> {
    let mut value = [0.0f64; 1];

    if spc_util_read_numbers(&mut value, args) < 1 {
        return Ok(-1);
    }
    let mode = value[0] as i32;
    if mode < 0 || mode > 7 {
        dpx.spc_warn(spe, format_args!("Invalid text rendering mode {}.\n", mode));
        return Ok(-1);
    }
    let work_buffer = format!(" {} Tr", mode);
    dpx.pdf_doc_add_page_content(work_buffer.as_bytes())?;
    args.skip_white();
    if args.curptr < args.endptr {
        dpx.pdf_doc_add_page_content(b" ")?;
        let content = args.rest().to_vec();
        dpx.pdf_doc_add_page_content(&content)?;
    }

    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_xtx_unsupportedcolor`.
fn spc_handler_xtx_unsupportedcolor(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> Result<i32> {
    dpx.spc_warn(
        spe,
        format_args!(
            "xetex-style \\special{{x:{:?}}} is not supported by this driver;\nupdate document or driver to use \\special{{color}} instead.",
            args.command
        ),
    );

    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_xtx_unsupported`.
fn spc_handler_xtx_unsupported(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.spc_warn(
        spe,
        format_args!(
            "xetex-style \\special{{x:{:?}}} is not supported by this driver.",
            args.command
        ),
    );

    args.curptr = args.endptr;
    Ok(0)
}

/// `xtx_handlers`.
pub static XTX_HANDLERS: [SpcHandler; 21] = [
    SpcHandler {
        key: b"textcolor",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"textcolorpush",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"textcolorpop",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"rulecolor",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"rulecolorpush",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"rulecolorpop",
        exec: spc_handler_xtx_unsupportedcolor,
    },
    SpcHandler {
        key: b"papersize",
        exec: spc_handler_xtx_papersize,
    },
    SpcHandler {
        key: b"backgroundcolor",
        exec: spc_handler_xtx_backgroundcolor,
    },
    SpcHandler {
        key: b"gsave",
        exec: spc_handler_xtx_gsave,
    },
    SpcHandler {
        key: b"grestore",
        exec: spc_handler_xtx_grestore,
    },
    SpcHandler {
        key: b"scale",
        exec: spc_handler_xtx_scale,
    },
    SpcHandler {
        key: b"bscale",
        exec: spc_handler_xtx_bscale,
    },
    SpcHandler {
        key: b"escale",
        exec: spc_handler_xtx_escale,
    },
    SpcHandler {
        key: b"rotate",
        exec: spc_handler_xtx_rotate,
    },
    SpcHandler {
        key: b"fontmapline",
        exec: spc_handler_xtx_fontmapline,
    },
    SpcHandler {
        key: b"fontmapfile",
        exec: spc_handler_xtx_fontmapfile,
    },
    SpcHandler {
        key: b"shadow",
        exec: spc_handler_xtx_unsupported,
    },
    SpcHandler {
        key: b"colorshadow",
        exec: spc_handler_xtx_unsupported,
    },
    SpcHandler {
        key: b"renderingmode",
        exec: spc_handler_xtx_renderingmode,
    },
    SpcHandler {
        key: b"initoverlay",
        exec: spc_handler_xtx_initoverlay,
    },
    SpcHandler {
        key: b"clipoverlay",
        exec: spc_handler_xtx_clipoverlay,
    },
];

/// `spc_xtx_check_special`.
pub fn spc_xtx_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    crate::parse::skip_white(buf, &mut p);
    buf[p..].starts_with(b"x:")
}

/// `spc_xtx_setup_handler`.
pub fn spc_xtx_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> Result<i32> {
    let mut error = -1;

    ap.skip_white();
    if ap.curptr + 2 >= ap.endptr || !ap.rest().starts_with(b"x:") {
        dpx.spc_warn(spe, format_args!("Not x: special???"));
        return Ok(-1);
    }
    ap.curptr += 2;

    ap.skip_white();
    let q = {
        let (s, pp) = ap.parts();
        parse_c_ident(s, pp)
    };
    if let Some(q) = q {
        let q = cstr(&q);
        for h in &XTX_HANDLERS {
            if q == h.key {
                ap.command = Some(h.key);
                sph.key = b"x:";
                sph.exec = h.exec;
                ap.skip_white();
                error = 0;
                break;
            }
        }
    }

    Ok(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check() {
        assert!(spc_xtx_check_special(b" x:gsave"));
        assert!(!spc_xtx_check_special(b"pdf:x"));
    }

    #[test]
    fn overlay_compare() {
        assert!(!strncmp_ne(b"on", b"on", 2));
        assert!(!strncmp_ne(b"on", b"onx", 2));
        assert!(strncmp_ne(b"on", b"o", 2));
        assert!(!strncmp_ne(b"", b"anything", 0));
        assert!(!strncmp_ne(b"all", b"all", 3));
    }
}
