//! pdfdraw.c, pdfdraw.h: graphics state, paths, ExtGState stack.
//!
//! The gstate stack (`gs_stack`) and the ExtGState stack (`xgs_stack`)
//! are `DpxStack`s in `self.draw`; their top is the current state. Path
//! operations on plain `PdfPath` data are its methods; those that write
//! page content or read the gstate are `Dpx` methods. A C `pa_elem *`
//! into a path is an index into `PdfPath::path`.

use crate::dpxutil::DpxStack;
use crate::fmt::Buf;
use crate::pdfcolor::PdfColor;
use crate::pdfdev::{PdfCoord, PdfRect, PdfTmatrix};
use crate::prelude::*;

/// `PDF_DASH_SIZE_MAX`.
pub const PDF_DASH_SIZE_MAX: usize = 16;
/// `PDF_GSAVE_MAX`.
pub const PDF_GSAVE_MAX: i32 = 256;

pub const PDF_FILL_RULE_NONZERO: i32 = 0;
pub const PDF_FILL_RULE_EVENODD: i32 = 1;

/// `OUR_EPSILON`.
pub const OUR_EPSILON: f64 = 2.5e-16;

pub const PE_TYPE__INVALID: i32 = -1;
pub const PE_TYPE__MOVETO: i32 = 0;
pub const PE_TYPE__LINETO: i32 = 1;
pub const PE_TYPE__CURVETO: i32 = 2;
pub const PE_TYPE__CURVETO_V: i32 = 3;
pub const PE_TYPE__CURVETO_Y: i32 = 4;
pub const PE_TYPE__CLOSEPATH: i32 = 5;
pub const PE_TYPE__TERMINATE: i32 = 6;

/// `GS_FLAG_CURRENTPOINT_SET`.
pub const GS_FLAG_CURRENTPOINT_SET: i32 = 1 << 0;
/// `FORMAT_BUFF_LEN`.
pub const FORMAT_BUFF_LEN: usize = 1024;
/// `QB_TWO_THIRD`, `QB_ONE_THIRD` (quadratic Bezier).
pub const QB_TWO_THIRD: f64 = 2.0 / 3.0;
pub const QB_ONE_THIRD: f64 = 1.0 / 3.0;

/// An entry of `petypes[]`.
#[derive(Clone, Copy, Debug)]
pub struct PeType {
    /// PDF operator char.
    pub opchr: u8,
    /// Number of points.
    pub n_pts: i32,
    pub strkey: Option<&'static [u8]>,
}

/// `petypes[]`, indexed by `PE_TYPE__*`.
pub const PETYPES: [PeType; 7] = [
    PeType {
        opchr: b'm',
        n_pts: 1,
        strkey: Some(b"moveto"),
    },
    PeType {
        opchr: b'l',
        n_pts: 1,
        strkey: Some(b"lineto"),
    },
    PeType {
        opchr: b'c',
        n_pts: 3,
        strkey: Some(b"curveto"),
    },
    PeType {
        opchr: b'v',
        n_pts: 2,
        strkey: Some(b"vcurveto"),
    },
    PeType {
        opchr: b'y',
        n_pts: 2,
        strkey: Some(b"ycurveto"),
    },
    PeType {
        opchr: b'h',
        n_pts: 0,
        strkey: Some(b"closepath"),
    },
    PeType {
        opchr: b' ',
        n_pts: 0,
        strkey: None,
    },
];

/// `default_xgs`.
pub const DEFAULT_XGS: &[u8] = b"<< /Type /ExtGState   /LW 1 /LC 0 /LJ 0 /ML 10 /D [[] 0]   /RI /RelativeColorimetric /SA false /BM /Normal /SMask /None   /AIS false /TK false /CA 1 /ca 1   /OP false /op false /OPM 0 /FL 1>>";

/// `pa_elem`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaElem {
    /// `PE_TYPE__*`.
    pub r#type: i32,
    pub p: [PdfCoord; 3],
}

/// `struct pdf_path_` (`num_paths`/`max_paths`: the Vec's length).
#[derive(Clone, Debug, Default)]
pub struct PdfPath {
    pub path: Vec<PaElem>,
}

/// `pdf_gstate.linedash`.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineDash {
    pub num_dash: i32,
    pub pattern: [f64; PDF_DASH_SIZE_MAX],
    pub offset: f64,
}

/// `pdf_gstate`.
#[derive(Clone, Debug, Default)]
pub struct PdfGstate {
    pub cp: PdfCoord,
    /// cm.
    pub matrix: PdfTmatrix,
    pub strokecolor: PdfColor,
    pub fillcolor: PdfColor,
    /// d, D.
    pub linedash: LineDash,
    /// w, LW.
    pub linewidth: f64,
    /// J, LC.
    pub linecap: i32,
    /// j, LJ.
    pub linejoin: i32,
    /// M, ML.
    pub miterlimit: f64,
    /// i, FL (0 to 100).
    pub flatness: i32,
    pub path: PdfPath,
    pub flags: i32,
    pub extgstate: Option<Obj>,
}

/// `struct xgs_res`.
#[derive(Clone, Debug, Default)]
pub struct XgsRes {
    pub object: Option<Obj>,
    pub accumlated: Option<Obj>,
}

/// pdfdraw.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `fmt_buf` (scratch).
    pub fmt_buf: Buf,
    pub xgs_stack: DpxStack<XgsRes>,
    pub xgs_count: i32,
    pub gs_stack: DpxStack<PdfGstate>,
}

/// `pdf_copymatrix(m, n)`: `*m = *n`.
pub fn pdf_copymatrix(m: &mut PdfTmatrix, n: &PdfTmatrix) {
    todo!()
}

/// `pdf_setmatrix`.
pub fn pdf_setmatrix(m: &mut PdfTmatrix, p: f64, q: f64, r: f64, s: f64, t: f64, u: f64) {
    todo!()
}

/// `pdf_concatmatrix(m, n)`: m -> n x m.
pub fn pdf_concatmatrix(m: &mut PdfTmatrix, n: &PdfTmatrix) {
    todo!()
}

/// `detP(M)`.
#[must_use]
pub fn det_p(m: &PdfTmatrix) -> f64 {
    todo!()
}

/// `inversematrix`: status (-1 not invertible) and W.
pub fn inversematrix(m: &PdfTmatrix) -> (i32, PdfTmatrix) {
    todo!()
}

/// `pdf_coord__equal` (`COORD_EQUAL`).
#[must_use]
pub fn pdf_coord__equal(p1: &PdfCoord, p2: &PdfCoord) -> i32 {
    todo!()
}

/// `pdf_coord__transform`.
pub fn pdf_coord__transform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    todo!()
}

/// `pdf_coord__itransform`.
pub fn pdf_coord__itransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    todo!()
}

/// `pdf_coord__dtransform`.
pub fn pdf_coord__dtransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    todo!()
}

/// `pdf_coord__idtransform`.
pub fn pdf_coord__idtransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    todo!()
}

/// `pdf_invertmatrix`.
pub fn pdf_invertmatrix(m: &mut PdfTmatrix) {
    todo!()
}

/// `INVERTIBLE_MATRIX`: 0, or -1 (with warnings).
pub fn invertible_matrix(m: &PdfTmatrix) -> i32 {
    todo!()
}

/// `PT_OP_VALID(c)`.
#[must_use]
pub fn pt_op_valid(c: u8) -> bool {
    todo!()
}

/// `PE_VALID`, `PE_N_PTS`, `PE_OPCHR` for an element.
impl PaElem {
    #[must_use]
    pub fn pe_valid(&self) -> bool {
        todo!()
    }
    #[must_use]
    pub fn pe_n_pts(&self) -> i32 {
        todo!()
    }
    #[must_use]
    pub fn pe_opchr(&self) -> u8 {
        todo!()
    }
}

impl PdfPath {
    /// `init_a_path`.
    pub fn init_a_path(&mut self) {
        todo!()
    }
    /// `pdf_path__clearpath`.
    pub fn pdf_path__clearpath(&mut self) {
        todo!()
    }
    /// `pdf_path__growpath`.
    pub fn pdf_path__growpath(&mut self, max_pe: i32) -> i32 {
        todo!()
    }
    /// `clear_a_path`.
    pub fn clear_a_path(&mut self) {
        todo!()
    }
    /// `pdf_path__copypath`: `self = p0`.
    pub fn pdf_path__copypath(&mut self, p0: &PdfPath) -> i32 {
        todo!()
    }
    /// `PA_LENGTH`.
    #[must_use]
    pub fn pa_length(&self) -> i32 {
        todo!()
    }
    /// `pdf_path__moveto`.
    pub fn pdf_path__moveto(&mut self, cp: &mut PdfCoord, p0: &PdfCoord) -> i32 {
        todo!()
    }
    /// `pdf_path__next_pe`: the index of the new element.
    pub fn pdf_path__next_pe(&mut self, cp: &PdfCoord) -> usize {
        todo!()
    }
    /// `pdf_path__transform`.
    pub fn pdf_path__transform(&mut self, m: &PdfTmatrix) -> i32 {
        todo!()
    }
    /// `pdf_path__lineto`.
    pub fn pdf_path__lineto(&mut self, cp: &mut PdfCoord, p0: &PdfCoord) -> i32 {
        todo!()
    }
    /// `pdf_path__curveto`.
    pub fn pdf_path__curveto(
        &mut self,
        cp: &mut PdfCoord,
        p0: &PdfCoord,
        p1: &PdfCoord,
        p2: &PdfCoord,
    ) -> i32 {
        todo!()
    }
    /// `pdf_path__elliptarc`: `ca` center, `xar` x-axis rotation
    /// (degrees), `a_d` orientation.
    pub fn pdf_path__elliptarc(
        &mut self,
        cp: &mut PdfCoord,
        ca: &PdfCoord,
        r_x: f64,
        r_y: f64,
        xar: f64,
        a_0: f64,
        a_1: f64,
        a_d: i32,
    ) -> i32 {
        todo!()
    }
    /// `pdf_path__closepath`.
    pub fn pdf_path__closepath(&mut self, cp: &mut PdfCoord) -> i32 {
        todo!()
    }
    /// `pdf_path__isarect`: `f_ir` fill-rule is ignorable.
    #[must_use]
    pub fn pdf_path__isarect(&self, f_ir: i32) -> i32 {
        todo!()
    }
}

impl PdfGstate {
    /// `init_a_gstate`.
    pub fn init_a_gstate(&mut self) {
        todo!()
    }
    /// `copy_a_gstate`: `self` (gs1) = `gs2`; the caller links
    /// `extgstate` (C does `pdf_link_obj`).
    pub fn copy_a_gstate(&mut self, gs2: &PdfGstate) {
        todo!()
    }
}

impl Dpx {
    /// `pdf_dev__rectshape`: `m` optional.
    fn pdf_dev__rectshape(&mut self, r: &PdfRect, m: Option<&PdfTmatrix>, opchr: u8) -> i32 {
        todo!()
    }
    /// `pdf_dev__flushpath` on the current gstate's path.
    fn pdf_dev__flushpath(&mut self, opchr: u8, rule: i32, ignore_rule: i32) -> i32 {
        todo!()
    }
    /// `init_xgstate`.
    fn init_xgstate(&mut self) {
        todo!()
    }
    /// `clear_xgstate`.
    fn clear_xgstate(&mut self) {
        todo!()
    }
    /// `clear_a_gstate`: releases its objects.
    fn clear_a_gstate(&mut self, gs: &mut PdfGstate) {
        todo!()
    }
    /// `pdf_dev_set_xgstate`.
    fn pdf_dev_set_xgstate(&mut self, diff: Obj, accumlated: Obj) -> i32 {
        todo!()
    }
    /// `pdf_dev_reset_xgstate`.
    pub fn pdf_dev_reset_xgstate(&mut self, force: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_xgstate_push`.
    pub fn pdf_dev_xgstate_push(&mut self, object: Obj) {
        todo!()
    }
    /// `pdf_dev_xgstate_pop`.
    pub fn pdf_dev_xgstate_pop(&mut self) {
        todo!()
    }
    /// `pdf_dev_init_gstates`.
    pub fn pdf_dev_init_gstates(&mut self) {
        todo!()
    }
    /// `pdf_dev_clear_gstates`.
    pub fn pdf_dev_clear_gstates(&mut self) {
        todo!()
    }
    /// `pdf_dev_gsave`.
    pub fn pdf_dev_gsave(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_grestore`.
    pub fn pdf_dev_grestore(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_push_gstate`.
    pub fn pdf_dev_push_gstate(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_pop_gstate`.
    pub fn pdf_dev_pop_gstate(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_current_depth`.
    pub fn pdf_dev_current_depth(&self) -> i32 {
        todo!()
    }
    /// `pdf_dev_grestore_to`.
    pub fn pdf_dev_grestore_to(&mut self, depth: i32) {
        todo!()
    }
    /// `pdf_dev_currentpoint`: status and the point.
    pub fn pdf_dev_currentpoint(&self) -> (i32, PdfCoord) {
        todo!()
    }
    /// `pdf_dev_currentmatrix`: status and the matrix.
    pub fn pdf_dev_currentmatrix(&self) -> (i32, PdfTmatrix) {
        todo!()
    }
    /// `pdf_dev_set_color`: `mask` 0 stroking, 0x20 non-stroking.
    pub fn pdf_dev_set_color(&mut self, color: &PdfColor, mask: u8, force: i32) {
        todo!()
    }
    /// `pdf_dev_set_strokingcolor(c)`.
    pub fn pdf_dev_set_strokingcolor(&mut self, color: &PdfColor) {
        todo!()
    }
    /// `pdf_dev_set_nonstrokingcolor(c)`.
    pub fn pdf_dev_set_nonstrokingcolor(&mut self, color: &PdfColor) {
        todo!()
    }
    /// `pdf_dev_concat`.
    pub fn pdf_dev_concat(&mut self, m: &PdfTmatrix) -> i32 {
        todo!()
    }
    /// `pdf_dev_setmiterlimit`.
    pub fn pdf_dev_setmiterlimit(&mut self, mlimit: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_setlinecap`.
    pub fn pdf_dev_setlinecap(&mut self, capstyle: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_setlinejoin`.
    pub fn pdf_dev_setlinejoin(&mut self, joinstyle: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_setlinewidth`.
    pub fn pdf_dev_setlinewidth(&mut self, width: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_setdash` (`count` = `pattern.len()`).
    pub fn pdf_dev_setdash(&mut self, pattern: &[f64], offset: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_clip`.
    pub fn pdf_dev_clip(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_eoclip`.
    pub fn pdf_dev_eoclip(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_flushpath`.
    pub fn pdf_dev_flushpath(&mut self, p_op: u8, fill_rule: i32) -> i32 {
        todo!()
    }
    /// `pdf_dev_newpath`.
    pub fn pdf_dev_newpath(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_moveto`.
    pub fn pdf_dev_moveto(&mut self, x: f64, y: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_rmoveto`.
    pub fn pdf_dev_rmoveto(&mut self, x: f64, y: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_lineto`.
    pub fn pdf_dev_lineto(&mut self, x: f64, y: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_rlineto`.
    pub fn pdf_dev_rlineto(&mut self, x: f64, y: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_curveto`.
    pub fn pdf_dev_curveto(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_rcurveto`.
    pub fn pdf_dev_rcurveto(
        &mut self,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> i32 {
        todo!()
    }
    /// `pdf_dev_closepath`.
    pub fn pdf_dev_closepath(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_dev_dtransform`: `m` none means the current CTM.
    pub fn pdf_dev_dtransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        todo!()
    }
    /// `pdf_dev_idtransform`.
    pub fn pdf_dev_idtransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        todo!()
    }
    /// `pdf_dev_transform`.
    pub fn pdf_dev_transform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        todo!()
    }
    /// `pdf_dev_itransform`.
    pub fn pdf_dev_itransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        todo!()
    }
    /// `pdf_dev_arc`.
    pub fn pdf_dev_arc(&mut self, c_x: f64, c_y: f64, r: f64, a_0: f64, a_1: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_arcn`.
    pub fn pdf_dev_arcn(&mut self, c_x: f64, c_y: f64, r: f64, a_0: f64, a_1: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_arcx`: `a_d` arc direction, `xar` x-axis rotation.
    pub fn pdf_dev_arcx(
        &mut self,
        c_x: f64,
        c_y: f64,
        r_x: f64,
        r_y: f64,
        a_0: f64,
        a_1: f64,
        a_d: i32,
        xar: f64,
    ) -> i32 {
        todo!()
    }
    /// `pdf_dev_bspline`.
    pub fn pdf_dev_bspline(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_rectstroke`: `m` optional.
    pub fn pdf_dev_rectstroke(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        m: Option<&PdfTmatrix>,
    ) -> i32 {
        todo!()
    }
    /// `pdf_dev_rectfill`.
    pub fn pdf_dev_rectfill(&mut self, x: f64, y: f64, w: f64, h: f64) -> i32 {
        todo!()
    }
    /// `pdf_dev_rectclip`.
    pub fn pdf_dev_rectclip(&mut self, x: f64, y: f64, w: f64, h: f64) -> i32 {
        todo!()
    }
}
