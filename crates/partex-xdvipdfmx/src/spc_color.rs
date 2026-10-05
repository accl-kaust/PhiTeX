//! spc_color.c, spc_color.h: the `color` and `background` specials.
//!
//! No handler table in C: `spc_color_setup_handler` picks the handler.

use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler};

/// `skip_blank` (static): spaces, tabs and `\v`.
fn skip_blank(s: &[u8], pp: &mut usize) {
    todo!()
}

/// `spc_handler_color_push` (`color push …`).
fn spc_handler_color_push(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_color_pop`.
fn spc_handler_color_pop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_color_default` (`color <spec>`: clear the stack, set).
fn spc_handler_color_default(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_handler_background`.
fn spc_handler_background(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    todo!()
}

/// `spc_color_check_special`.
pub fn spc_color_check_special(buf: &[u8]) -> bool {
    todo!()
}

/// `spc_color_setup_handler`.
pub fn spc_color_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    todo!()
}
