//! spc_dvipdfmx.c, spc_dvipdfmx.h: `dvipdfmx:` specials.
//!
//! `dvipdfmx:config` is read at BOP by dvi.c (`read_config_special`);
//! here it is skipped.

use crate::dpxutil::parse_c_ident;
use crate::pdfdev::{INFO_HAS_HEIGHT, TransformInfo};
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, cstr};

/// `spc_handler_null`: skips the rest.
fn spc_handler_null(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.curptr = args.endptr;
    Ok(0)
}

/// `spc_handler_dvipdfmx_catch_phantom`.
fn spc_handler_dvipdfmx_catch_phantom(
    dpx: &mut Dpx,
    spe: &mut SpcEnv,
    args: &mut SpcArg,
) -> Result<i32> {
    args.skip_white();
    let mode;
    {
        let b = {
            let (s, pp) = args.parts();
            dpx.o.parse_pdf_boolean(s, pp)
        };
        let Some(b) = b else {
            crate::warn!("A boolean value expected but not found...");
            return Ok(-1);
        };
        mode = i32::from(dpx.o.boolean_value(b));
        dpx.o.release(b);
    }
    dpx.spc_set_linkmode(spe, mode);

    args.skip_white();
    if mode == 1 && args.curptr < args.endptr {
        let mut ti = TransformInfo::default();
        ti.transform_info_clear();
        let error = dpx.spc_util_read_dimtrns(spe, &mut ti, args, 0);
        if error != 0 {
            return Ok(-1);
        }
        if ti.flags & INFO_HAS_HEIGHT != 0 {
            dpx.spc_set_phantom(spe, ti.height, ti.depth);
        }
        args.skip_white();
    }

    Ok(0)
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
    let mut p = 0;
    crate::parse::skip_white(buf, &mut p);
    buf[p..].starts_with(b"dvipdfmx:")
}

/// `spc_dvipdfmx_setup_handler`.
pub fn spc_dvipdfmx_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> Result<i32> {
    let mut error = -1;

    ap.skip_white();
    if ap.curptr + 9 >= ap.endptr || !ap.rest().starts_with(b"dvipdfmx:") {
        dpx.spc_warn(spe, format_args!("Not dvipdfmx: special???"));
        return Ok(-1);
    }
    ap.curptr += 9;

    ap.skip_white();
    let q = {
        let (s, pp) = ap.parts();
        parse_c_ident(s, pp)
    };
    if let Some(q) = q {
        let q = cstr(&q);
        for h in &DVIPDFMX_HANDLERS {
            if q == h.key {
                ap.command = Some(h.key);
                sph.key = b"dvipdfmx:";
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
        assert!(spc_dvipdfmx_check_special(b"dvipdfmx:config C 0x0010"));
        assert!(spc_dvipdfmx_check_special(b"  dvipdfmx:"));
        assert!(!spc_dvipdfmx_check_special(b"dvipdfmx"));
    }
}
