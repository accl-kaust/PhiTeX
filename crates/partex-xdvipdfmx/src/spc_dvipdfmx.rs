//! spc_dvipdfmx.c, spc_dvipdfmx.h: `dvipdfmx:` specials.

use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

/// `spc_handler_null`: skips the rest.
fn spc_handler_null(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_dvipdfmx_catch_phantom`.
fn spc_handler_dvipdfmx_catch_phantom(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `dvipdfmx_handlers`.
pub static DVIPDFMX_HANDLERS: [SpcHandler; 2] = [
    // handled at bop
    SpcHandler {
        key: b"config",
        exec: spc_handler_null,
    },
    SpcHandler {
        key: b"catch_phantom",
        exec: spc_handler_dvipdfmx_catch_phantom,
    },
];

/// `spc_dvipdfmx_check_special`.
pub fn spc_dvipdfmx_check_special(buf: &[u8]) -> bool {
    todo!()
}

/// `spc_dvipdfmx_setup_handler`.
pub fn spc_dvipdfmx_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
