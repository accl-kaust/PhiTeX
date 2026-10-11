//! spc_dvips.c: dvips's specials (`PSfile=`, `ps:`, `" `), as graphicx's
//! and color's `dvips` drivers write them.
//!
//! EPS files go through `pdf_ximage_load_image` (the host's distiller);
//! inline PostScript through mpost's interpreter ([`crate::mpost`]).
//! Not ported: PSTricks's `PST:` and `pst:` (they run ghostscript as a
//! calculator): they fail, as a special C cannot handle does. `header=`
//! only matters to them, so it is taken and ignored.

use crate::parse::skip_white;
use crate::pdfdev::{PdfTmatrix, TransformInfo};
use crate::pdfximage::LoadOptions;
use crate::prelude::*;
use crate::specials::{SpcArg, SpcEnv, SpcHandler, SpcHandlerFn};

/// spc_dvips.c's `static` state (what the ported handlers use).
#[derive(Clone, Debug, Default)]
pub struct State {
    pub block_pending: i32,
    pub pending_x: f64,
    pub pending_y: f64,
    pub position_set: bool,
}

/// `spc_handler_ps_header`: `header=FILE` (only PSTricks's calculator
/// reads the headers).
fn spc_handler_ps_header(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr + 1 >= args.endptr || args.buf[args.curptr] != b'=' {
        dpx.spc_warn(spe, format_args!("No filename specified for PSfile special."));
        return Ok(-1);
    }
    args.curptr = args.endptr;
    Ok(0)
}

/// `parse_filename`: quoted with `"` or `'`, or up to a space.
fn parse_filename(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    let mut p = *pp;
    if p >= s.len() {
        return None;
    }
    let qchar = if s[p] == b'"' || s[p] == b'\'' {
        p += 1;
        s[p - 1]
    } else {
        b' '
    };
    let q = p;
    while p < s.len() && s[p] != qchar {
        p += 1;
    }
    let n = p - q;
    if qchar != b' ' {
        // (C reads the byte after the special: never the quote)
        if p >= s.len() {
            return None;
        }
        p += 1;
    }
    if n == 0 {
        return None;
    }
    *pp = p;
    Some(s[q..q + n].to_vec())
}

/// The options C's `load_options options = {1, 0, NULL}` sets.
fn first_page() -> LoadOptions {
    LoadOptions {
        page_no: 1,
        bbox_type: 0,
        dict: None,
        page_name: None,
    }
}

/// `spc_handler_ps_file`: `PSfile=FILE llx=.. lly=.. ...`.
fn spc_handler_ps_file(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    args.skip_white();
    if args.curptr + 1 >= args.endptr || args.buf[args.curptr] != b'=' {
        dpx.spc_warn(spe, format_args!("No filename specified for PSfile special."));
        return Ok(-1);
    }
    args.curptr += 1;
    let filename = {
        let (s, pp) = args.parts();
        parse_filename(s, pp)
    };
    let Some(filename) = filename else {
        dpx.spc_warn(spe, format_args!("No filename specified for PSfile special."));
        return Ok(-1);
    };
    let mut ti = TransformInfo::default();
    ti.transform_info_clear();
    if dpx.spc_util_read_dimtrns(spe, &mut ti, args, 1) < 0 {
        return Ok(-1);
    }
    let form_id = dpx.pdf_ximage_load_image(None, &filename, first_page())?;
    if form_id < 0 {
        dpx.spc_warn(
            spe,
            format_args!("Failed to read image file: {}", String::from_utf8_lossy(&filename)),
        );
        return Ok(-1);
    }
    let (x, y) = (spe.x_user, spe.y_user);
    dpx.spc_put_image(spe, form_id, &mut ti, x, y)?;
    Ok(0)
}

/// `spc_handler_ps_plotfile` ("not correct implementation but dvipdfm
/// supports...").
fn spc_handler_ps_plotfile(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.spc_warn(spe, format_args!("\"ps: plotfile\" found (not properly implemented)"));
    args.skip_white();
    let filename = {
        let (s, pp) = args.parts();
        parse_filename(s, pp)
    };
    let Some(filename) = filename else {
        dpx.spc_warn(spe, format_args!("Expecting filename but not found..."));
        return Ok(-1);
    };
    let form_id = dpx.pdf_ximage_load_image(None, &filename, first_page())?;
    if form_id < 0 {
        dpx.spc_warn(
            spe,
            format_args!("Could not open PS file: {}", String::from_utf8_lossy(&filename)),
        );
        return Ok(-1);
    }
    let mut p = TransformInfo::default();
    p.transform_info_clear();
    // (xscale = 1.0, yscale = -1.0)
    p.matrix.d = -1.0;
    dpx.spc_put_image(spe, form_id, &mut p, 0.0, 0.0)?;
    Ok(0)
}

/// The warnings after inline PostScript, and the graphics state back
/// where it was if it failed (with `restore`).
fn after_inline(dpx: &mut Dpx, spe: &SpcEnv, error: i32, st_depth: usize, gs_depth: i32, restore: bool) -> Result<()> {
    if error != 0 {
        dpx.spc_warn(spe, format_args!("Interpreting PS code failed!!! Output might be broken!!!"));
        if restore {
            dpx.pdf_dev_grestore_to(gs_depth)?;
        }
    } else if st_depth != dpx.mps_stack_depth() {
        dpx.spc_warn(spe, format_args!("Stack not empty after execution of inline PostScript code."));
        dpx.spc_warn(
            spe,
            format_args!(">> Your macro package makes some assumption on internal behaviour of DVI drivers."),
        );
        dpx.spc_warn(spe, format_args!(">> It may not compatible with dvipdfmx."));
    }
    Ok(())
}

/// `spc_handler_ps_literal`: `ps:CODE`, `ps::CODE`, `ps::[begin]CODE`,
/// `ps::[end]CODE`.
fn spc_handler_ps_literal(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    let cp = dpx.spc_get_current_point(spe);
    let rest = args.rest();
    let st = &mut dpx.dvips;
    let (x_user, y_user);
    if rest.starts_with(b":[begin]") {
        st.block_pending += 1;
        st.position_set = true;
        st.pending_x = cp.x;
        st.pending_y = cp.y;
        (x_user, y_user) = (cp.x, cp.y);
        args.curptr += b":[begin]".len();
    } else if rest.starts_with(b":[end]") {
        if st.block_pending <= 0 {
            dpx.spc_warn(spe, format_args!("No corresponding ::[begin] found."));
            return Ok(-1);
        }
        st.block_pending -= 1;
        st.position_set = false;
        (x_user, y_user) = (st.pending_x, st.pending_y);
        args.curptr += b":[end]".len();
    } else if rest.first() == Some(&b':') {
        (x_user, y_user) = if st.position_set { (st.pending_x, st.pending_y) } else { (cp.x, cp.y) };
        args.curptr += 1;
    } else {
        st.position_set = true;
        st.pending_x = cp.x;
        st.pending_y = cp.y;
        (x_user, y_user) = (cp.x, cp.y);
    }
    args.skip_white();
    let mut error = 0;
    if args.curptr < args.endptr {
        let st_depth = dpx.mps_stack_depth();
        let gs_depth = dpx.pdf_dev_current_depth();
        let (s, pp) = args.parts();
        error = dpx.mps_exec_inline(s, pp, x_user, y_user)?;
        after_inline(dpx, spe, error, st_depth, gs_depth, true)?;
    }
    Ok(error)
}

/// `spc_handler_ps_trickscmd`, `spc_handler_ps_tricksobj`: not ported
/// (ghostscript).
fn spc_handler_ps_tricks(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.spc_warn(spe, format_args!("PSTricks specials are not supported."));
    args.curptr = args.endptr;
    Ok(-1)
}

/// `spc_handler_ps_default`: `" CODE`, at the current point.
fn spc_handler_ps_default(dpx: &mut Dpx, spe: &mut SpcEnv, args: &mut SpcArg) -> Result<i32> {
    dpx.pdf_dev_gsave()?;
    let st_depth = dpx.mps_stack_depth();
    let gs_depth = dpx.pdf_dev_current_depth();
    let cp = dpx.spc_get_current_point(spe);
    let mut m = PdfTmatrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: cp.x, f: cp.y };
    dpx.pdf_dev_concat(&m)?;
    let error = {
        let (s, pp) = args.parts();
        dpx.mps_exec_inline(s, pp, cp.x, cp.y)?
    };
    m.e = -cp.x;
    m.f = -cp.y;
    dpx.pdf_dev_concat(&m)?;
    after_inline(dpx, spe, error, st_depth, gs_depth, false)?;
    dpx.pdf_dev_grestore_to(gs_depth)?;
    dpx.pdf_dev_grestore()?;
    Ok(error)
}

/// `dvips_handlers`.
const DVIPS_HANDLERS: [(&[u8], SpcHandlerFn); 10] = [
    (b"header", spc_handler_ps_header),
    (b"PSfile", spc_handler_ps_file),
    (b"psfile", spc_handler_ps_file),
    (b"ps: plotfile ", spc_handler_ps_plotfile),
    (b"PS: plotfile ", spc_handler_ps_plotfile),
    (b"PS:", spc_handler_ps_literal),
    (b"ps:", spc_handler_ps_literal),
    (b"PST:", spc_handler_ps_tricks),
    (b"pst:", spc_handler_ps_tricks),
    (b"\" ", spc_handler_ps_default),
];

/// `spc_dvips_check_special`.
pub fn spc_dvips_check_special(buf: &[u8]) -> bool {
    let mut p = 0;
    skip_white(buf, &mut p);
    if p >= buf.len() {
        return false;
    }
    let rest = &buf[p..];
    DVIPS_HANDLERS.iter().any(|(k, _)| rest.starts_with(k))
}

/// `spc_dvips_setup_handler`.
pub fn spc_dvips_setup_handler(
    dpx: &mut Dpx,
    sph: &mut SpcHandler,
    spe: &mut SpcEnv,
    ap: &mut SpcArg,
) -> Result<i32> {
    ap.skip_white();
    let key = ap.curptr;
    while ap.curptr < ap.endptr && ap.buf[ap.curptr].is_ascii_alphabetic() {
        ap.curptr += 1;
    }
    // (test for "ps:"; the "ps::" special is subsumed under this case)
    if ap.curptr < ap.endptr && ap.buf[ap.curptr] == b':' {
        ap.curptr += 1;
        if ap.rest().starts_with(b" plotfile ") {
            ap.curptr += b" plotfile ".len();
        }
    } else if ap.curptr + 1 < ap.endptr && ap.buf[ap.curptr] == b'"' && ap.buf[ap.curptr + 1] == b' ' {
        ap.curptr += 2;
    }
    if ap.curptr == key {
        dpx.spc_warn(spe, format_args!("Not ps: special???"));
        return Ok(-1);
    }
    let k = &ap.buf[key..ap.curptr];
    let Some(&(name, exec)) = DVIPS_HANDLERS.iter().find(|(name, _)| *name == k) else {
        return Ok(-1);
    };
    ap.skip_white();
    ap.command = Some(name);
    sph.key = b"ps:";
    sph.exec = exec;
    Ok(0)
}

impl Dpx {
    /// `spc_dvips_at_end_page`.
    pub fn spc_dvips_at_end_page(&mut self) -> Result<i32> {
        self.mps_eop_cleanup()?;
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        let mut p = 0;
        assert_eq!(parse_filename(b"\"a b.eps\" llx=0", &mut p).as_deref(), Some(&b"a b.eps"[..]));
        assert_eq!(p, 9);
        let mut p = 0;
        assert_eq!(parse_filename(b"x.eps llx=0", &mut p).as_deref(), Some(&b"x.eps"[..]));
        assert_eq!(p, 5);
        let mut p = 0;
        assert_eq!(parse_filename(b"\"x.eps", &mut p), None);
    }

    #[test]
    fn check() {
        assert!(spc_dvips_check_special(b"  ps: 1 0 0 setrgbcolor"));
        assert!(spc_dvips_check_special(b"PSfile=foo.eps"));
        assert!(spc_dvips_check_special(b"\" newpath"));
        assert!(!spc_dvips_check_special(b"pdf:literal"));
        assert!(!spc_dvips_check_special(b"   "));
    }
}
