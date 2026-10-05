//! spc_color.c, spc_color.h: the `color` and `background` specials.
//!
//! No handler table in C: `spc_color_setup_handler` picks the handler.
//! The color stack itself is pdfcolor.c's (the color is reinstalled
//! after grestore and the like).

use crate::dpxutil::parse_c_ident;
use crate::pdfcolor::PdfColor;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, cstr};

/// `ISBLANK`: spaces, tabs and `\v`.
fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == 0x0b
}

/// `skip_blank` (static): spaces, tabs and `\v`.
fn skip_blank(s: &[u8], pp: &mut usize) {
    while *pp < s.len() && is_blank(s[*pp]) {
        *pp += 1;
    }
}

/// `spc_handler_color_push` (`color push …`).
fn spc_handler_color_push(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    let mut colorspec = PdfColor::default();
    let error = dpx.spc_util_read_colorspec(spe, &mut colorspec, args, 1);
    if error == 0 {
        dpx.pdf_color_push(&colorspec, &colorspec);
    }
    error
}

/// `spc_handler_color_pop`.
fn spc_handler_color_pop(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    dpx.pdf_color_pop();
    0
}

/// `spc_handler_color_default` (`color <spec>`: as dvips, clear the
/// stack, then set).
fn spc_handler_color_default(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    let mut colorspec = PdfColor::default();
    let error = dpx.spc_util_read_colorspec(spe, &mut colorspec, args, 1);
    if error == 0 {
        dpx.pdf_color_clear_stack();
        dpx.pdf_color_set(&colorspec, &colorspec);
    }
    error
}

/// `spc_handler_background`.
fn spc_handler_background(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> i32 {
    let mut colorspec = PdfColor::default();
    let error = dpx.spc_util_read_colorspec(spe, &mut colorspec, args, 1);
    if error == 0 {
        dpx.pdf_doc_set_bgcolor(Some(&colorspec));
    }
    error
}

/// `spc_color_check_special`.
pub fn spc_color_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    skip_blank(buf, &mut p);
    match parse_c_ident(buf, &mut p) {
        None => false,
        Some(q) => {
            let q = cstr(&q);
            q == b"color" || q == b"background"
        }
    }
}

/// `spc_color_setup_handler`.
pub fn spc_color_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> i32 {
    {
        let (s, pp) = ap.parts();
        skip_blank(s, pp);
    }
    let q = {
        let (s, pp) = ap.parts();
        parse_c_ident(s, pp)
    };
    let Some(q) = q else {
        return -1;
    };
    {
        let (s, pp) = ap.parts();
        skip_blank(s, pp);
    }

    let q = cstr(&q);
    if q == b"background" {
        ap.command = Some(b"background");
        sph.exec = spc_handler_background;
    } else if q == b"color" {
        // color
        let mut p = ap.curptr;
        let q = parse_c_ident(&ap.buf[..ap.endptr], &mut p);
        let Some(q) = q else {
            return -1;
        };
        let q = cstr(&q);
        if q == b"push" {
            ap.command = Some(b"push");
            sph.exec = spc_handler_color_push;
            ap.curptr = p;
        } else if q == b"pop" {
            ap.command = Some(b"pop");
            sph.exec = spc_handler_color_pop;
            ap.curptr = p;
        } else {
            // cmyk, rgb, ...
            ap.command = Some(b"");
            sph.exec = spc_handler_color_default;
        }
    } else {
        dpx.spc_warn(spe, format_args!("Not color/background special?"));
        return -1;
    }

    {
        let (s, pp) = ap.parts();
        skip_blank(s, pp);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank() {
        let s = b" \t\x0b\ncolor";
        let mut p = 0;
        skip_blank(s, &mut p);
        assert_eq!(p, 3);
    }
}
