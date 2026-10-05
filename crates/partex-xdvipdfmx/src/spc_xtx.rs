//! spc_xtx.c, spc_xtx.h: XeTeX's `x:` specials.

use crate::pdfdev::PdfCoord;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

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
    ) -> i32 {
        todo!()
    }
}

/// `spc_handler_xtx_scale`.
fn spc_handler_xtx_scale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_bscale`: scale without gsave/grestore.
fn spc_handler_xtx_bscale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_escale`.
fn spc_handler_xtx_escale(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_rotate`.
fn spc_handler_xtx_rotate(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_gsave` (also used by the dvips specials).
pub fn spc_handler_xtx_gsave(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_grestore` (also used by the dvips specials).
pub fn spc_handler_xtx_grestore(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_papersize` (does nothing).
fn spc_handler_xtx_papersize(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_backgroundcolor`.
fn spc_handler_xtx_backgroundcolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_fontmapline`.
fn spc_handler_xtx_fontmapline(dpx: &mut Dpx, spe: &mut SpcEnv, ap: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_fontmapfile`.
fn spc_handler_xtx_fontmapfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_initoverlay`.
fn spc_handler_xtx_initoverlay(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_clipoverlay`.
fn spc_handler_xtx_clipoverlay(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_renderingmode`.
fn spc_handler_xtx_renderingmode(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_unsupportedcolor`.
fn spc_handler_xtx_unsupportedcolor(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_xtx_unsupported`.
fn spc_handler_xtx_unsupported(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
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
    todo!()
}

/// `spc_xtx_setup_handler`.
pub fn spc_xtx_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
