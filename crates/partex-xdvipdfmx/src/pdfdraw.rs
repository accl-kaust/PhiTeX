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
    m.a = n.a;
    m.b = n.b;
    m.c = n.c;
    m.d = n.d;
    m.e = n.e;
    m.f = n.f;
}

/// `pdf_setmatrix`.
pub fn pdf_setmatrix(m: &mut PdfTmatrix, p: f64, q: f64, r: f64, s: f64, t: f64, u: f64) {
    m.a = p;
    m.b = q;
    m.c = r;
    m.d = s;
    m.e = t;
    m.f = u;
}

/// `pdf_concatmatrix(m, n)`: m -> n x m.
pub fn pdf_concatmatrix(m: &mut PdfTmatrix, n: &PdfTmatrix) {
    let (tmp_a, tmp_b, tmp_c, tmp_d) = (m.a, m.b, m.c, m.d);
    m.a = n.a * tmp_a + n.b * tmp_c;
    m.b = n.a * tmp_b + n.b * tmp_d;
    m.c = n.c * tmp_a + n.d * tmp_c;
    m.d = n.c * tmp_b + n.d * tmp_d;
    m.e += n.e * tmp_a + n.f * tmp_c;
    m.f += n.e * tmp_b + n.f * tmp_d;
}

/// `detP(M)`.
#[must_use]
pub fn det_p(m: &PdfTmatrix) -> f64 {
    m.a * m.d - m.b * m.c
}

/// `inversematrix`: status (-1 not invertible) and W.
pub fn inversematrix(m: &PdfTmatrix) -> (i32, PdfTmatrix) {
    let det = det_p(m);
    if det.abs() < OUR_EPSILON {
        warn!("Inverting matrix with zero determinant...");
        return (-1, PdfTmatrix::default()); /* result is undefined. */
    }

    let mut w = PdfTmatrix {
        a: m.d / det,
        b: -m.b / det,
        c: -m.c / det,
        d: m.a / det,
        e: m.c * m.f - m.d * m.e,
        f: m.b * m.e - m.a * m.f,
    };
    w.e /= det;
    w.f /= det;

    (0, w)
}

/// `pdf_coord__equal` (`COORD_EQUAL`).
#[must_use]
pub fn pdf_coord__equal(p1: &PdfCoord, p2: &PdfCoord) -> i32 {
    if (p1.x - p2.x).abs() < 1.0e-7 && (p1.y - p2.y).abs() < 1.0e-7 {
        return 1;
    }
    0
}

fn coord_equal(p: &PdfCoord, q: &PdfCoord) -> bool {
    pdf_coord__equal(p, q) != 0
}

/// `pdf_coord__transform`.
pub fn pdf_coord__transform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    let (x, y) = (p.x, p.y);
    p.x = x * m.a + y * m.c + m.e;
    p.y = x * m.b + y * m.d + m.f;

    0
}

/// `pdf_coord__itransform`.
pub fn pdf_coord__itransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    let (error, w) = inversematrix(m);
    if error != 0 {
        return error;
    }

    let (x, y) = (p.x, p.y);
    p.x = x * w.a + y * w.c + w.e;
    p.y = x * w.b + y * w.d + w.f;

    0
}

/// `pdf_coord__dtransform`.
pub fn pdf_coord__dtransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    let (x, y) = (p.x, p.y);
    p.x = x * m.a + y * m.c;
    p.y = x * m.b + y * m.d;

    0
}

/// `pdf_coord__idtransform`.
pub fn pdf_coord__idtransform(p: &mut PdfCoord, m: &PdfTmatrix) -> i32 {
    let (error, w) = inversematrix(m);
    if error != 0 {
        return error;
    }

    let (x, y) = (p.x, p.y);
    p.x = x * w.a + y * w.c;
    p.y = x * w.b + y * w.d;

    0
}

/// `pdf_invertmatrix`.
pub fn pdf_invertmatrix(m: &mut PdfTmatrix) {
    let det = det_p(m);
    let w = if det.abs() < OUR_EPSILON {
        warn!("Inverting matrix with zero determinant...");
        PdfTmatrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    } else {
        let mut w = PdfTmatrix {
            a: m.d / det,
            b: -m.b / det,
            c: -m.c / det,
            d: m.a / det,
            e: m.c * m.f - m.d * m.e,
            f: m.b * m.e - m.a * m.f,
        };
        w.e /= det;
        w.f /= det;
        w
    };

    pdf_copymatrix(m, &w);
}

/// `INVERTIBLE_MATRIX`: 0, or -1 (with warnings).
pub fn invertible_matrix(m: &PdfTmatrix) -> i32 {
    if det_p(m).abs() < OUR_EPSILON {
        warn!("Transformation matrix not invertible.");
        warn!("--- M = [{} {} {} {} {} {}]", m.a, m.b, m.c, m.d, m.e, m.f);
        return -1;
    }
    0
}

/// `PT_OP_VALID(c)`.
#[must_use]
pub fn pt_op_valid(c: u8) -> bool {
    matches!(c, b'f' | b'F' | b's' | b'S' | b'b' | b'B' | b'W')
}

/// `PE_VALID`, `PE_N_PTS`, `PE_OPCHR` for an element.
impl PaElem {
    #[must_use]
    pub fn pe_valid(&self) -> bool {
        self.r#type > PE_TYPE__INVALID && self.r#type < PE_TYPE__TERMINATE
    }
    #[must_use]
    pub fn pe_n_pts(&self) -> i32 {
        if self.pe_valid() {
            PETYPES[self.r#type as usize].n_pts
        } else {
            0
        }
    }
    #[must_use]
    pub fn pe_opchr(&self) -> u8 {
        if self.pe_valid() {
            PETYPES[self.r#type as usize].opchr
        } else {
            b' '
        }
    }
}

impl PdfPath {
    /// `init_a_path`.
    pub fn init_a_path(&mut self) {
        self.path = Vec::new();
    }
    /// `pdf_path__clearpath`.
    pub fn pdf_path__clearpath(&mut self) {
        self.path.clear();
    }
    /// `pdf_path__growpath` (the Vec grows as elements are added).
    pub fn pdf_path__growpath(&mut self, max_pe: i32) -> i32 {
        if max_pe > 0 {
            self.path
                .reserve((max_pe as usize).saturating_sub(self.path.len()));
        }
        0
    }
    /// `clear_a_path`.
    pub fn clear_a_path(&mut self) {
        self.path = Vec::new();
    }
    /// `pdf_path__copypath`: `self = p0`.
    pub fn pdf_path__copypath(&mut self, p0: &PdfPath) -> i32 {
        self.path.clear();
        self.path.extend_from_slice(&p0.path);
        0
    }
    /// `PA_LENGTH`.
    #[must_use]
    pub fn pa_length(&self) -> i32 {
        self.path.len() as i32
    }
    /// `pdf_path__moveto`.
    pub fn pdf_path__moveto(&mut self, cp: &mut PdfCoord, p0: &PdfCoord) -> i32 {
        self.pdf_path__growpath(self.pa_length() + 1);
        if let Some(pe) = self.path.last_mut()
            && pe.r#type == PE_TYPE__MOVETO
        {
            cp.x = p0.x;
            cp.y = p0.y;
            pe.p[0] = *cp;
            return 0;
        }
        cp.x = p0.x;
        cp.y = p0.y;
        let mut pe = PaElem {
            r#type: PE_TYPE__MOVETO,
            ..PaElem::default()
        };
        pe.p[0] = *cp;
        self.path.push(pe);

        0
    }
    /// `pdf_path__next_pe`: the index of the new element (its type and
    /// points are the caller's to set).
    pub fn pdf_path__next_pe(&mut self, cp: &PdfCoord) -> usize {
        let moveto = |cp: &PdfCoord| {
            let mut pe = PaElem {
                r#type: PE_TYPE__MOVETO,
                ..PaElem::default()
            };
            pe.p[0] = *cp;
            pe
        };

        self.pdf_path__growpath(self.pa_length() + 2);
        if self.path.is_empty() {
            self.path.push(moveto(cp));
            self.path.push(PaElem::default());
            return self.path.len() - 1;
        }

        let pe = self.path.last_mut().unwrap();
        match pe.r#type {
            PE_TYPE__MOVETO => {
                pe.p[0] = *cp;
            }
            PE_TYPE__LINETO => {
                if !coord_equal(&pe.p[0], cp) {
                    self.path.push(moveto(cp));
                }
            }
            PE_TYPE__CURVETO => {
                if !coord_equal(&pe.p[2], cp) {
                    self.path.push(moveto(cp));
                }
            }
            PE_TYPE__CURVETO_Y | PE_TYPE__CURVETO_V => {
                if !coord_equal(&pe.p[1], cp) {
                    self.path.push(moveto(cp));
                }
            }
            PE_TYPE__CLOSEPATH => {
                self.path.push(moveto(cp));
            }
            _ => {}
        }

        self.path.push(PaElem::default());
        self.path.len() - 1
    }
    /// `pdf_path__transform`.
    pub fn pdf_path__transform(&mut self, m: &PdfTmatrix) -> i32 {
        for pe in &mut self.path {
            let mut n = pe.pe_n_pts();
            while n > 0 {
                n -= 1;
                pdf_coord__transform(&mut pe.p[n as usize], m);
            }
        }

        0
    }
    /// `pdf_path__lineto`.
    pub fn pdf_path__lineto(&mut self, cp: &mut PdfCoord, p0: &PdfCoord) -> i32 {
        let i = self.pdf_path__next_pe(cp);
        let pe = &mut self.path[i];
        pe.r#type = PE_TYPE__LINETO;
        cp.x = p0.x;
        cp.y = p0.y;
        pe.p[0] = *cp;

        0
    }
    /// `pdf_path__curveto`.
    pub fn pdf_path__curveto(
        &mut self,
        cp: &mut PdfCoord,
        p0: &PdfCoord,
        p1: &PdfCoord,
        p2: &PdfCoord,
    ) -> i32 {
        let i = self.pdf_path__next_pe(cp);
        let pe = &mut self.path[i];
        if coord_equal(cp, p0) {
            pe.r#type = PE_TYPE__CURVETO_V;
            pe.p[0] = *p1;
            cp.x = p2.x;
            cp.y = p2.y;
            pe.p[1] = *cp;
        } else if coord_equal(p1, p2) {
            pe.r#type = PE_TYPE__CURVETO_Y;
            pe.p[0] = *p0;
            cp.x = p1.x;
            cp.y = p1.y;
            pe.p[1] = *cp;
        } else {
            pe.r#type = PE_TYPE__CURVETO;
            pe.p[0] = *p0;
            pe.p[1] = *p1;
            cp.x = p2.x;
            cp.y = p2.y;
            pe.p[2] = *cp;
        }

        0
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
        let (mut xar, mut a_0, mut a_1) = (xar, a_0, a_1);
        let mut error = 0;

        if r_x.abs() < OUR_EPSILON || r_y.abs() < OUR_EPSILON {
            return -1;
        }

        if a_d < 0 {
            while a_1 > a_0 {
                a_1 -= 360.0;
            }
        } else {
            while a_1 < a_0 {
                a_0 -= 360.0;
            }
        }

        let mut d_a = a_1 - a_0;
        let mut n_c: i32 = 1;
        while d_a.abs() > 90.0 * f64::from(n_c) {
            n_c += 1;
        }
        d_a /= f64::from(n_c);
        if d_a.abs() < OUR_EPSILON {
            return -1;
        }

        a_0 *= core::f64::consts::PI / 180.0;
        a_1 *= core::f64::consts::PI / 180.0;
        d_a *= core::f64::consts::PI / 180.0;
        xar *= core::f64::consts::PI / 180.0;
        let _ = a_1;
        let mut t = PdfTmatrix::default();
        t.a = libm::cos(xar);
        t.c = -libm::sin(xar);
        t.b = -t.c;
        t.d = t.a;
        t.e = 0.0;
        t.f = 0.0;

        /* A parameter that controls cb-curve (off-curve) points */
        let b = 4.0 * (1.0 - libm::cos(0.5 * d_a)) / (3.0 * libm::sin(0.5 * d_a));
        let b_x = r_x * b;
        let b_y = r_y * b;

        let mut p0 = PdfCoord {
            x: r_x * libm::cos(a_0),
            y: r_y * libm::sin(a_0),
        };
        pdf_coord__transform(&mut p0, &t);
        p0.x += ca.x;
        p0.y += ca.y;
        if self.pa_length() == 0 {
            self.pdf_path__moveto(cp, &p0);
        } else if !coord_equal(cp, &p0) {
            self.pdf_path__lineto(cp, &p0); /* add line seg */
        }
        let mut i = 0;
        while error == 0 && i < n_c {
            let q = a_0 + f64::from(i) * d_a;
            let e0 = PdfCoord {
                x: libm::cos(q),
                y: libm::sin(q),
            };
            let e1 = PdfCoord {
                x: libm::cos(q + d_a),
                y: libm::sin(q + d_a),
            };

            let mut p0 = PdfCoord {
                x: r_x * e0.x,
                y: r_y * e0.y,
            }; /* s.p. */
            let mut p3 = PdfCoord {
                x: r_x * e1.x,
                y: r_y * e1.y,
            }; /* e.p. */

            let mut p1 = PdfCoord {
                x: -b_x * e0.y,
                y: b_y * e0.x,
            };
            let mut p2 = PdfCoord {
                x: b_x * e1.y,
                y: -b_y * e1.x,
            };

            pdf_coord__transform(&mut p0, &t);
            pdf_coord__transform(&mut p1, &t);
            pdf_coord__transform(&mut p2, &t);
            pdf_coord__transform(&mut p3, &t);
            p0.x += ca.x;
            p0.y += ca.y;
            p3.x += ca.x;
            p3.y += ca.y;
            p1.x += p0.x;
            p1.y += p0.y;
            p2.x += p3.x;
            p2.y += p3.y;

            error = self.pdf_path__curveto(&mut p0, &p1, &p2, &p3);
            cp.x = p3.x;
            cp.y = p3.y;
            i += 1;
        }

        error
    }
    /// `pdf_path__closepath`.
    pub fn pdf_path__closepath(&mut self, cp: &mut PdfCoord) -> i32 {
        /* search for start point of the last subpath */
        let Some(i) = self
            .path
            .iter()
            .rposition(|pe| pe.r#type == PE_TYPE__MOVETO)
        else {
            return -1; /* No path or no start point(!) */
        };

        cp.x = self.path[i].p[0].x;
        cp.y = self.path[i].p[0].y;

        self.pdf_path__growpath(self.pa_length() + 1);

        self.path.push(PaElem {
            r#type: PE_TYPE__CLOSEPATH,
            ..PaElem::default()
        });

        0
    }
    /// `pdf_path__isarect`: `f_ir` fill-rule is ignorable.
    #[must_use]
    pub fn pdf_path__isarect(&self, f_ir: i32) -> i32 {
        if self.pa_length() == 5 {
            let pe0 = &self.path[0];
            let pe1 = &self.path[1];
            let pe2 = &self.path[2];
            let pe3 = &self.path[3];
            let pe4 = &self.path[4];
            if pe0.r#type == PE_TYPE__MOVETO
                && pe1.r#type == PE_TYPE__LINETO
                && pe2.r#type == PE_TYPE__LINETO
                && pe3.r#type == PE_TYPE__LINETO
                && pe4.r#type == PE_TYPE__CLOSEPATH
            {
                if pe1.p[0].y - pe0.p[0].y == 0.0
                    && pe2.p[0].x - pe1.p[0].x == 0.0
                    && pe3.p[0].y - pe2.p[0].y == 0.0
                {
                    if pe1.p[0].x - pe0.p[0].x == pe2.p[0].x - pe3.p[0].x {
                        return 1;
                    }
                    /* Winding number is different but ignore it here. */
                } else if f_ir != 0
                    && pe1.p[0].x - pe0.p[0].x == 0.0
                    && pe2.p[0].y - pe1.p[0].y == 0.0
                    && pe3.p[0].x - pe2.p[0].x == 0.0
                    && pe1.p[0].y - pe0.p[0].y == pe2.p[0].y - pe3.p[0].y
                {
                    return 1;
                }
            }
        }

        0
    }
}

impl PdfGstate {
    /// `init_a_gstate`.
    pub fn init_a_gstate(&mut self) {
        self.cp.x = 0.0;
        self.cp.y = 0.0;

        pdf_setmatrix(&mut self.matrix, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

        self.strokecolor.pdf_color_black();
        self.fillcolor.pdf_color_black();

        self.linedash.num_dash = 0;
        self.linedash.offset = 0.0;
        self.linecap = 0;
        self.linejoin = 0;
        self.linewidth = 1.0;
        self.miterlimit = 10.0;

        self.flatness = 1; /* default to 1 in PDF */

        /* Internal variables */
        self.flags = 0;
        self.path.init_a_path();

        self.extgstate = None;
    }
    /// `copy_a_gstate`: `self` (gs1) = `gs2`; the caller links
    /// `extgstate` (C does `pdf_link_obj`). `flags` is not copied.
    pub fn copy_a_gstate(&mut self, gs2: &PdfGstate) {
        self.cp.x = gs2.cp.x;
        self.cp.y = gs2.cp.y;

        pdf_copymatrix(&mut self.matrix, &gs2.matrix);

        self.path.pdf_path__copypath(&gs2.path);

        self.linedash.num_dash = gs2.linedash.num_dash;
        for i in 0..gs2.linedash.num_dash.max(0) as usize {
            self.linedash.pattern[i] = gs2.linedash.pattern[i];
        }
        self.linedash.offset = gs2.linedash.offset;

        self.linecap = gs2.linecap;
        self.linejoin = gs2.linejoin;
        self.linewidth = gs2.linewidth;
        self.miterlimit = gs2.miterlimit;
        self.flatness = gs2.flatness;

        self.fillcolor.pdf_color_copycolor(&gs2.fillcolor);
        self.strokecolor.pdf_color_copycolor(&gs2.strokecolor);

        self.extgstate = gs2.extgstate;
    }
}

/// `parse_pdf_dict` of `default_xgs`.
fn default_xgs(dpx: &mut Dpx) -> Result<Obj> {
    let mut p = 0;
    Ok(dpx
        .o
        .parse_pdf_dict(DEFAULT_XGS, &mut p, None)?
        .expect("default ExtGState"))
}

impl Dpx {
    /// The current graphics state (the gstate stack's top).
    fn pdfdraw_gs(&self) -> &PdfGstate {
        self.draw
            .gs_stack
            .dpx_stack_top()
            .expect("graphics state stack empty")
    }
    fn pdfdraw_gs_mut(&mut self) -> &mut PdfGstate {
        self.draw
            .gs_stack
            .dpx_stack_top_mut()
            .expect("graphics state stack empty")
    }

    /// `pdf_dev__rectshape`: `m` optional.
    fn pdf_dev__rectshape(&mut self, r: &PdfRect, m: Option<&PdfTmatrix>, opchr: u8) -> i32 {
        assert!(pt_op_valid(opchr));

        let isclip = opchr == b'W';

        /* disallow matrix for clipping.
         * q ... clip Q does nothing and
         * n M cm ... clip n alter CTM.
         * (C's test: a matrix given and either clipping or invertible.)
         */
        if let Some(m) = m
            && (isclip || invertible_matrix(m) == 0)
        {
            return -1;
        }

        self.graphics_mode();

        if !isclip {
            self.pdf_dev_gsave();
        }
        self.pdf_dev_newpath();
        if !isclip && let Some(m) = m {
            self.pdf_dev_concat(m);
        }

        let p = PdfCoord { x: r.llx, y: r.lly };
        let wd = r.urx - r.llx;
        let ht = r.ury - r.lly;
        let mut buf = Buf::new();
        buf.push(b' ');
        self.pdf_sprint_coord(&mut buf, &p);
        buf.push(b' ');
        self.pdf_sprint_length(&mut buf, wd);
        buf.push(b' ');
        self.pdf_sprint_length(&mut buf, ht);
        buf.push(b' ');
        buf.extend(b"re");

        buf.push(b' ');
        buf.push(opchr);

        self.pdf_doc_add_page_content(buf.as_bytes());

        if isclip {
            self.pdf_dev_newpath();
        } else {
            self.pdf_dev_grestore();
        }

        0
    }
    /// `pdf_dev__flushpath` on the current gstate's path.
    fn pdf_dev__flushpath(&mut self, opchr: u8, rule: i32, ignore_rule: i32) -> i32 {
        assert!(pt_op_valid(opchr));
        let b_len = FORMAT_BUFF_LEN;

        if self.pdfdraw_gs().path.pa_length() <= 0 {
            return 0;
        }
        let pa = self.pdfdraw_gs().path.clone();

        self.graphics_mode();
        let mut b = Buf::new();
        let isrect = pa.pdf_path__isarect(ignore_rule);
        if isrect != 0 {
            let pe = &pa.path[0];
            let pe1 = &pa.path[2];

            let r = PdfRect {
                llx: pe.p[0].x,
                lly: pe.p[0].y,
                urx: pe1.p[0].x - pe.p[0].x, /* width...  */
                ury: pe1.p[0].y - pe.p[0].y, /* height... */
            };

            b.push(b' ');
            self.pdf_sprint_rect(&mut b, &r);
            b.push(b' ');
            b.push(b'r');
            b.push(b'e');
            self.pdf_doc_add_page_content(b.as_bytes()); /* op: re */
            b = Buf::new();
        } else {
            for pe in &pa.path {
                let n_pts = pe.pe_n_pts();
                for pt in &pe.p[..n_pts as usize] {
                    b.push(b' ');
                    self.pdf_sprint_coord(&mut b, pt);
                }
                b.push(b' ');
                b.push(pe.pe_opchr());
                if b.len() + 128 > b_len {
                    self.pdf_doc_add_page_content(b.as_bytes()); /* op: m l c v y h */
                    b = Buf::new();
                }
            }
            if !b.is_empty() {
                self.pdf_doc_add_page_content(b.as_bytes()); /* op: m l c v y h */
                b = Buf::new();
            }
        }

        b.push(b' ');
        b.push(opchr);
        if rule == PDF_FILL_RULE_EVENODD {
            b.push(b'*');
        }

        self.pdf_doc_add_page_content(b.as_bytes()); /* op: f F s S b B W f* F* s* S* b* B* W* */

        0
    }
    /// `init_xgstate`.
    fn init_xgstate(&mut self) {
        self.draw.xgs_stack = DpxStack::dpx_stack_init();
        self.draw.xgs_count = 0;
    }
    /// `clear_xgstate`.
    fn clear_xgstate(&mut self) {
        while let Some(xgs) = self.draw.xgs_stack.dpx_stack_pop() {
            self.o.release_opt(xgs.object);
            self.o.release_opt(xgs.accumlated);
        }
    }
    /// `clear_a_gstate`: releases its objects.
    fn clear_a_gstate(&mut self, gs: &mut PdfGstate) {
        gs.path.clear_a_path();
        if let Some(x) = gs.extgstate {
            self.o.release(x);
        }
        *gs = PdfGstate::default();
    }
    /// `pdf_dev_set_xgstate`.
    fn pdf_dev_set_xgstate(&mut self, diff: Obj, accumlated: Obj) -> Result<i32> {
        let id = self.draw.xgs_count;
        let mut res_name = Buf::new();
        res_name.extend(b"DPX_GS");
        res_name.int(id);
        res_name.0.truncate(15);
        let mut buf = Buf::new();
        buf.extend(b" /");
        buf.extend(res_name.as_bytes());
        buf.extend(b" gs");
        buf.0.truncate(63);
        self.pdf_doc_add_page_content(buf.as_bytes());
        let d = self.o.link(diff);
        self.pdf_doc_add_page_resource(b"ExtGState", res_name.as_bytes(), d)?;
        if let Some(x) = self.pdfdraw_gs().extgstate {
            self.o.release(x);
        }
        let a = self.o.link(accumlated);
        self.pdfdraw_gs_mut().extgstate = Some(a);
        self.draw.xgs_count += 1;

        Ok(0)
    }
    /// `pdf_dev_reset_xgstate`.
    pub fn pdf_dev_reset_xgstate(&mut self, force: i32) -> Result<i32> {
        let mut need_reset = false;

        let target = if let Some(xgs) = self.draw.xgs_stack.dpx_stack_top() {
            let a = xgs.accumlated.expect("ExtGState");
            self.o.link(a)
        } else {
            if self.pdfdraw_gs().extgstate.is_none() && force == 0 {
                return Ok(0);
            }
            default_xgs(self)?
        };
        let current = if let Some(x) = self.pdfdraw_gs().extgstate {
            self.o.link(x)
        } else {
            default_xgs(self)?
        };

        let diff = self.o.new_dict();
        let keys = self.o.dict_keys(target);
        for i in 0..self.o.array_length(keys) {
            let key = self.o.get_array(keys, i as i32).unwrap();
            let name = self.o.name_value(key).to_vec();
            let value1 = self.o.lookup_dict(target, &name);
            let value2 = self.o.lookup_dict(current, &name);
            let is_diff = self.o.compare_object(value1, value2);
            if is_diff != 0 {
                let k = self.o.link(key);
                let v = self.o.link_opt(value1);
                self.o.add_dict(diff, k, v);
                need_reset = true;
            }
        }
        self.o.release(keys);
        if need_reset {
            self.pdf_dev_set_xgstate(diff, target)?;
        }
        self.o.release(diff);
        self.o.release(current);
        self.o.release(target);

        Ok(0)
    }
    /// `pdf_dev_xgstate_push`.
    pub fn pdf_dev_xgstate_push(&mut self, object: Obj) -> Result<()> {
        let accumlated = if let Some(current) = self.draw.xgs_stack.dpx_stack_top() {
            let ca = current.accumlated.expect("ExtGState");
            let a = self.o.new_dict();
            self.o.merge_dict(a, ca);
            a
        } else {
            default_xgs(self)?
        };
        self.o.merge_dict(accumlated, object);
        self.draw.xgs_stack.dpx_stack_push(XgsRes {
            object: Some(object),
            accumlated: Some(accumlated),
        });

        self.pdf_dev_set_xgstate(object, accumlated)?;
        Ok(())
    }
    /// `pdf_dev_xgstate_pop`.
    pub fn pdf_dev_xgstate_pop(&mut self) -> Result<()> {
        let current = self.draw.xgs_stack.dpx_stack_pop();
        let Some(current) = current else {
            warn!("Too many pop operation for ExtGState!");
            return Ok(());
        };
        let accumlated = if let Some(target) = self.draw.xgs_stack.dpx_stack_top() {
            let a = target.accumlated.expect("ExtGState");
            self.o.link(a)
        } else {
            default_xgs(self)?
        };
        let cobject = current.object.expect("ExtGState");
        let keys = self.o.dict_keys(cobject);
        let revert = self.o.new_dict();
        for i in 0..self.o.array_length(keys) {
            let key = self.o.get_array(keys, i as i32).unwrap();
            let name = self.o.name_value(key).to_vec();
            let value = self.o.lookup_dict(accumlated, &name);
            if let Some(value) = value {
                let k = self.o.link(key);
                let v = self.o.link(value);
                self.o.add_dict(revert, k, Some(v));
            } else {
                warn!("No previous ExtGState entry known, ignoring...");
            }
        }
        self.pdf_dev_set_xgstate(revert, accumlated)?;
        self.o.release(revert);
        self.o.release(keys);
        self.o.release(accumlated);

        self.o.release(cobject);
        self.o.release_opt(current.accumlated);
        Ok(())
    }
    /// `pdf_dev_init_gstates`.
    pub fn pdf_dev_init_gstates(&mut self) {
        self.draw.gs_stack = DpxStack::dpx_stack_init();

        let mut gs = PdfGstate::default();
        gs.init_a_gstate();

        self.draw.gs_stack.dpx_stack_push(gs); /* Initial state */
        self.init_xgstate();
    }
    /// `pdf_dev_clear_gstates`.
    pub fn pdf_dev_clear_gstates(&mut self) {
        if self.draw.gs_stack.dpx_stack_depth() > 1 {
            /* at least 1 elem. */
            warn!("GS stack depth is not zero at the end of the document.");
        }

        while let Some(mut gs) = self.draw.gs_stack.dpx_stack_pop() {
            self.clear_a_gstate(&mut gs);
        }

        self.clear_xgstate();
    }
    /// `pdf_dev_gsave`.
    pub fn pdf_dev_gsave(&mut self) -> i32 {
        let mut gs1 = PdfGstate::default();
        gs1.init_a_gstate();
        gs1.copy_a_gstate(self.pdfdraw_gs());
        gs1.extgstate = self.o.link_opt(gs1.extgstate);
        self.draw.gs_stack.dpx_stack_push(gs1);

        self.pdf_doc_add_page_content(b" q"); /* op: q */

        0
    }
    /// `pdf_dev_grestore`.
    pub fn pdf_dev_grestore(&mut self) -> i32 {
        if self.draw.gs_stack.dpx_stack_depth() <= 1 {
            /* Initial state at bottom */
            warn!("Too many grestores.");
            return -1;
        }

        let mut gs = self.draw.gs_stack.dpx_stack_pop().unwrap();
        self.clear_a_gstate(&mut gs);

        self.pdf_doc_add_page_content(b" Q"); /* op: Q */

        self.pdf_dev_reset_fonts(0);

        0
    }
    /// `pdf_dev_push_gstate`.
    pub fn pdf_dev_push_gstate(&mut self) -> i32 {
        let mut gs0 = PdfGstate::default();

        gs0.init_a_gstate();

        self.draw.gs_stack.dpx_stack_push(gs0);

        0
    }
    /// `pdf_dev_pop_gstate`.
    pub fn pdf_dev_pop_gstate(&mut self) -> i32 {
        if self.draw.gs_stack.dpx_stack_depth() <= 1 {
            /* Initial state at bottom */
            warn!("Too many grestores.");
            return -1;
        }

        let mut gs = self.draw.gs_stack.dpx_stack_pop().unwrap();
        self.clear_a_gstate(&mut gs);

        0
    }
    /// `pdf_dev_current_depth`.
    #[must_use]
    pub fn pdf_dev_current_depth(&self) -> i32 {
        self.draw.gs_stack.dpx_stack_depth() - 1 /* 0 means initial state */
    }
    /// `pdf_dev_grestore_to`.
    pub fn pdf_dev_grestore_to(&mut self, depth: i32) {
        assert!(depth >= 0);

        if self.draw.gs_stack.dpx_stack_depth() > depth + 1 {
            warn!("Closing pending transformations at end of page/XObject.");
        }

        while self.draw.gs_stack.dpx_stack_depth() > depth + 1 {
            self.pdf_doc_add_page_content(b" Q"); /* op: Q */
            let mut gs = self.draw.gs_stack.dpx_stack_pop().unwrap();
            self.clear_a_gstate(&mut gs);
        }
        self.pdf_dev_reset_fonts(0);
    }
    /// `pdf_dev_currentpoint`: status and the point.
    #[must_use]
    pub fn pdf_dev_currentpoint(&self) -> (i32, PdfCoord) {
        let cpt = &self.pdfdraw_gs().cp;
        (0, PdfCoord { x: cpt.x, y: cpt.y })
    }
    /// `pdf_dev_currentmatrix`: status and the matrix.
    #[must_use]
    pub fn pdf_dev_currentmatrix(&self) -> (i32, PdfTmatrix) {
        let mut m = PdfTmatrix::default();
        pdf_copymatrix(&mut m, &self.pdfdraw_gs().matrix);
        (0, m)
    }
    /// `pdf_dev_set_color`: `mask` 0 stroking, 0x20 non-stroking.
    pub fn pdf_dev_set_color(&mut self, color: &PdfColor, mask: u8, force: i32) -> Result<()> {
        let differs = {
            let gs = self.pdfdraw_gs();
            let current = if mask != 0 {
                &gs.fillcolor
            } else {
                &gs.strokecolor
            };
            force != 0 || color.pdf_color_compare(current) != 0
        };
        if !(self.pdf_dev_get_param(crate::pdfdev::PDF_DEV_PARAM_COLORMODE) != 0 && differs) {
            /* If "color" is already the current color, then do nothing
             * unless a color operator is forced
             */
            return Ok(());
        }

        self.graphics_mode();
        let mut buf = Buf::new();
        let len = self.pdf_color_set_color(color, &mut buf, FORMAT_BUFF_LEN, mask)?;
        self.pdf_doc_add_page_content(&buf.as_bytes()[..len]); /* op: RG K G rg k g etc. */
        let gs = self.pdfdraw_gs_mut();
        let current = if mask != 0 {
            &mut gs.fillcolor
        } else {
            &mut gs.strokecolor
        };
        current.pdf_color_copycolor(color);
        Ok(())
    }
    /// `pdf_dev_set_strokingcolor(c)`.
    pub fn pdf_dev_set_strokingcolor(&mut self, color: &PdfColor) -> Result<()> {
        self.pdf_dev_set_color(color, 0, 0)?;
        Ok(())
    }
    /// `pdf_dev_set_nonstrokingcolor(c)`.
    pub fn pdf_dev_set_nonstrokingcolor(&mut self, color: &PdfColor) -> Result<()> {
        self.pdf_dev_set_color(color, 0x20, 0)?;
        Ok(())
    }
    /// `pdf_dev_concat`.
    pub fn pdf_dev_concat(&mut self, m: &PdfTmatrix) -> i32 {
        /* Adobe Reader erases page content if there are
         * non invertible transformation.
         */
        if det_p(m).abs() < OUR_EPSILON {
            warn!("Transformation matrix not invertible.");
            warn!("--- M = [{} {} {} {} {} {}]", m.a, m.b, m.c, m.d, m.e, m.f);
            return -1;
        }

        if (m.a - 1.0).abs() > OUR_EPSILON
            || m.b.abs() > OUR_EPSILON
            || m.c.abs() > OUR_EPSILON
            || (m.d - 1.0).abs() > OUR_EPSILON
            || m.e.abs() > OUR_EPSILON
            || m.f.abs() > OUR_EPSILON
        {
            let mut buf = Buf::new();
            buf.push(b' ');
            self.pdf_sprint_matrix(&mut buf, m);
            buf.push(b' ');
            buf.push(b'c');
            buf.push(b'm');
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: cm */

            pdf_concatmatrix(&mut self.pdfdraw_gs_mut().matrix, m);
        }
        let (_, w) = inversematrix(m);

        let gs = self.pdfdraw_gs_mut();
        gs.path.pdf_path__transform(&w);
        pdf_coord__transform(&mut gs.cp, &w);

        0
    }
    /// `pdf_dev_setmiterlimit`.
    pub fn pdf_dev_setmiterlimit(&mut self, mlimit: f64) -> i32 {
        if self.pdfdraw_gs().miterlimit != mlimit {
            let mut buf = Buf::new();
            buf.push(b' ');
            self.pdf_sprint_length(&mut buf, mlimit);
            buf.push(b' ');
            buf.push(b'M');
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: M */
            self.pdfdraw_gs_mut().miterlimit = mlimit;
        }

        0
    }
    /// `pdf_dev_setlinecap`.
    pub fn pdf_dev_setlinecap(&mut self, capstyle: i32) -> i32 {
        if self.pdfdraw_gs().linecap != capstyle {
            let mut buf = Buf::new();
            buf.push(b' ');
            buf.int(capstyle);
            buf.extend(b" J");
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: J */
            self.pdfdraw_gs_mut().linecap = capstyle;
        }

        0
    }
    /// `pdf_dev_setlinejoin`.
    pub fn pdf_dev_setlinejoin(&mut self, joinstyle: i32) -> i32 {
        if self.pdfdraw_gs().linejoin != joinstyle {
            let mut buf = Buf::new();
            buf.push(b' ');
            buf.int(joinstyle);
            buf.extend(b" j");
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: j */
            self.pdfdraw_gs_mut().linejoin = joinstyle;
        }

        0
    }
    /// `pdf_dev_setlinewidth`.
    pub fn pdf_dev_setlinewidth(&mut self, width: f64) -> i32 {
        if self.pdfdraw_gs().linewidth != width {
            let mut buf = Buf::new();
            buf.push(b' ');
            self.pdf_sprint_length(&mut buf, width);
            buf.push(b' ');
            buf.push(b'w');
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: w */
            self.pdfdraw_gs_mut().linewidth = width;
        }

        0
    }
    /// `pdf_dev_setdash` (`count` = `pattern.len()`; C writes past the
    /// gstate's `PDF_DASH_SIZE_MAX` entries for a longer pattern, here
    /// they are printed but not kept).
    pub fn pdf_dev_setdash(&mut self, pattern: &[f64], offset: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        gs.linedash.num_dash = pattern.len() as i32;
        gs.linedash.offset = offset;
        self.pdf_doc_add_page_content(b" ["); /* op: */
        for (i, &p) in pattern.iter().enumerate() {
            let mut buf = Buf::new();
            buf.push(b' ');
            self.pdf_sprint_length(&mut buf, p);
            self.pdf_doc_add_page_content(buf.as_bytes()); /* op: */
            if i < PDF_DASH_SIZE_MAX {
                self.pdfdraw_gs_mut().linedash.pattern[i] = p;
            }
        }
        self.pdf_doc_add_page_content(b"] "); /* op: */
        let mut buf = Buf::new();
        self.pdf_sprint_length(&mut buf, offset);
        self.pdf_doc_add_page_content(buf.as_bytes()); /* op: */
        self.pdf_doc_add_page_content(b" d"); /* op: d */

        0
    }
    /// `pdf_dev_clip`.
    pub fn pdf_dev_clip(&mut self) -> i32 {
        self.pdf_dev__flushpath(b'W', PDF_FILL_RULE_NONZERO, 0)
    }
    /// `pdf_dev_eoclip`.
    pub fn pdf_dev_eoclip(&mut self) -> i32 {
        self.pdf_dev__flushpath(b'W', PDF_FILL_RULE_EVENODD, 0)
    }
    /// `pdf_dev_flushpath`.
    pub fn pdf_dev_flushpath(&mut self, p_op: u8, fill_rule: i32) -> i32 {
        /* last arg 'ignore_rule' is only for single object
         * that can be converted to a rect where fill rule
         * is inessential.
         */
        let error = self.pdf_dev__flushpath(p_op, fill_rule, 1);
        let gs = self.pdfdraw_gs_mut();
        gs.path.pdf_path__clearpath();

        gs.flags &= !GS_FLAG_CURRENTPOINT_SET;

        error
    }
    /// `pdf_dev_newpath`.
    pub fn pdf_dev_newpath(&mut self) -> i32 {
        let p = &mut self.pdfdraw_gs_mut().path;
        if p.pa_length() > 0 {
            p.pdf_path__clearpath();
        }
        /* The following is required for "newpath" operator in mpost.c. */
        self.pdf_doc_add_page_content(b" n"); /* op: n */

        0
    }
    /// `pdf_dev_moveto`.
    pub fn pdf_dev_moveto(&mut self, x: f64, y: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p = PdfCoord { x, y };
        gs.path.pdf_path__moveto(&mut gs.cp, &p) /* cpt updated */
    }
    /// `pdf_dev_rmoveto`.
    pub fn pdf_dev_rmoveto(&mut self, x: f64, y: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p = PdfCoord {
            x: gs.cp.x + x,
            y: gs.cp.y + y,
        };
        gs.path.pdf_path__moveto(&mut gs.cp, &p) /* cpt updated */
    }
    /// `pdf_dev_lineto`.
    pub fn pdf_dev_lineto(&mut self, x: f64, y: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p0 = PdfCoord { x, y };

        gs.path.pdf_path__lineto(&mut gs.cp, &p0)
    }
    /// `pdf_dev_rlineto`.
    pub fn pdf_dev_rlineto(&mut self, x: f64, y: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p0 = PdfCoord {
            x: x + gs.cp.x,
            y: y + gs.cp.y,
        };

        gs.path.pdf_path__lineto(&mut gs.cp, &p0)
    }
    /// `pdf_dev_curveto`.
    pub fn pdf_dev_curveto(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p0 = PdfCoord { x: x0, y: y0 };
        let p1 = PdfCoord { x: x1, y: y1 };
        let p2 = PdfCoord { x: x2, y: y2 };

        gs.path.pdf_path__curveto(&mut gs.cp, &p0, &p1, &p2)
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
        let gs = self.pdfdraw_gs_mut();
        let p0 = PdfCoord {
            x: x0 + gs.cp.x,
            y: y0 + gs.cp.y,
        };
        let p1 = PdfCoord {
            x: x1 + gs.cp.x,
            y: y1 + gs.cp.y,
        };
        let p2 = PdfCoord {
            x: x2 + gs.cp.x,
            y: y2 + gs.cp.y,
        };

        gs.path.pdf_path__curveto(&mut gs.cp, &p0, &p1, &p2)
    }
    /// `pdf_dev_closepath`.
    pub fn pdf_dev_closepath(&mut self) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        gs.path.pdf_path__closepath(&mut gs.cp)
    }
    /// `pdf_dev_dtransform`: `m` none means the current CTM.
    pub fn pdf_dev_dtransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        match m {
            Some(m) => pdf_coord__dtransform(p, m),
            None => pdf_coord__dtransform(p, &self.pdfdraw_gs().matrix),
        };
    }
    /// `pdf_dev_idtransform`.
    pub fn pdf_dev_idtransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        match m {
            Some(m) => pdf_coord__idtransform(p, m),
            None => pdf_coord__idtransform(p, &self.pdfdraw_gs().matrix),
        };
    }
    /// `pdf_dev_transform`.
    pub fn pdf_dev_transform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        match m {
            Some(m) => pdf_coord__transform(p, m),
            None => pdf_coord__transform(p, &self.pdfdraw_gs().matrix),
        };
    }
    /// `pdf_dev_itransform`.
    pub fn pdf_dev_itransform(&self, p: &mut PdfCoord, m: Option<&PdfTmatrix>) {
        match m {
            Some(m) => pdf_coord__itransform(p, m),
            None => pdf_coord__itransform(p, &self.pdfdraw_gs().matrix),
        };
    }
    /// `pdf_dev_arc`.
    pub fn pdf_dev_arc(&mut self, c_x: f64, c_y: f64, r: f64, a_0: f64, a_1: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let c = PdfCoord { x: c_x, y: c_y };

        gs.path
            .pdf_path__elliptarc(&mut gs.cp, &c, r, r, 0.0, a_0, a_1, 1)
    }
    /// `pdf_dev_arcn`.
    pub fn pdf_dev_arcn(&mut self, c_x: f64, c_y: f64, r: f64, a_0: f64, a_1: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let c = PdfCoord { x: c_x, y: c_y };

        gs.path
            .pdf_path__elliptarc(&mut gs.cp, &c, r, r, 0.0, a_0, a_1, -1)
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
        let gs = self.pdfdraw_gs_mut();
        let c = PdfCoord { x: c_x, y: c_y };

        gs.path
            .pdf_path__elliptarc(&mut gs.cp, &c, r_x, r_y, xar, a_0, a_1, a_d)
    }
    /// `pdf_dev_bspline`.
    pub fn pdf_dev_bspline(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> i32 {
        let gs = self.pdfdraw_gs_mut();
        let p1 = PdfCoord {
            x: x0 + 2.0 * (x1 - x0) / 3.0,
            y: y0 + 2.0 * (y1 - y0) / 3.0,
        };
        let p2 = PdfCoord {
            x: x1 + (x2 - x1) / 3.0,
            y: y1 + (y2 - y1) / 3.0,
        };
        let p3 = PdfCoord { x: x2, y: y2 };

        gs.path.pdf_path__curveto(&mut gs.cp, &p1, &p2, &p3)
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
        let r = PdfRect {
            llx: x,
            lly: y,
            urx: x + w,
            ury: y + h,
        };

        self.pdf_dev__rectshape(&r, m, b'S')
    }
    /// `pdf_dev_rectfill`.
    pub fn pdf_dev_rectfill(&mut self, x: f64, y: f64, w: f64, h: f64) -> i32 {
        let r = PdfRect {
            llx: x,
            lly: y,
            urx: x + w,
            ury: y + h,
        };

        self.pdf_dev__rectshape(&r, None, b'f')
    }
    /// `pdf_dev_rectclip`.
    pub fn pdf_dev_rectclip(&mut self, x: f64, y: f64, w: f64, h: f64) -> i32 {
        let r = PdfRect {
            llx: x,
            lly: y,
            urx: x + w,
            ury: y + h,
        };

        self.pdf_dev__rectshape(&r, None, b'W')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> PdfCoord {
        PdfCoord { x, y }
    }

    #[test]
    fn path_compression() {
        let mut pa = PdfPath::default();
        let mut cp = PdfCoord::default();
        // moveto twice: one moveto
        pa.pdf_path__moveto(&mut cp, &pt(1.0, 1.0));
        pa.pdf_path__moveto(&mut cp, &pt(2.0, 2.0));
        assert_eq!(pa.pa_length(), 1);
        pa.pdf_path__lineto(&mut cp, &pt(3.0, 2.0));
        // the curve starts at the current point: v
        pa.pdf_path__curveto(&mut cp, &pt(3.0, 2.0), &pt(4.0, 4.0), &pt(5.0, 5.0));
        assert_eq!(pa.path[2].r#type, PE_TYPE__CURVETO_V);
        pa.pdf_path__closepath(&mut cp);
        assert_eq!(cp, pt(2.0, 2.0));
        // a lineto after closepath starts with a moveto
        pa.pdf_path__lineto(&mut cp, &pt(0.0, 0.0));
        let types: Vec<i32> = pa.path.iter().map(|p| p.r#type).collect();
        assert_eq!(
            types,
            [
                PE_TYPE__MOVETO,
                PE_TYPE__LINETO,
                PE_TYPE__CURVETO_V,
                PE_TYPE__CLOSEPATH,
                PE_TYPE__MOVETO,
                PE_TYPE__LINETO
            ]
        );
    }

    #[test]
    fn rectangle() {
        let mut pa = PdfPath::default();
        let mut cp = PdfCoord::default();
        pa.pdf_path__moveto(&mut cp, &pt(0.0, 0.0));
        pa.pdf_path__lineto(&mut cp, &pt(2.0, 0.0));
        pa.pdf_path__lineto(&mut cp, &pt(2.0, 3.0));
        pa.pdf_path__lineto(&mut cp, &pt(0.0, 3.0));
        pa.pdf_path__closepath(&mut cp);
        assert_eq!(pa.pdf_path__isarect(0), 1);
    }

    #[test]
    fn matrices() {
        let m = PdfTmatrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 4.0,
            e: 1.0,
            f: 1.0,
        };
        let (e, w) = inversematrix(&m);
        assert_eq!(e, 0);
        let mut p = pt(3.0, 9.0);
        pdf_coord__transform(&mut p, &w);
        assert_eq!(p, pt(1.0, 2.0));
        let mut n = m;
        pdf_concatmatrix(&mut n, &w);
        assert_eq!(
            n,
            PdfTmatrix {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                e: 0.0,
                f: 0.0
            }
        );
    }

    #[test]
    fn quarter_arc() {
        let mut pa = PdfPath::default();
        let mut cp = PdfCoord::default();
        pa.pdf_path__elliptarc(&mut cp, &pt(0.0, 0.0), 1.0, 1.0, 0.0, 0.0, 90.0, 1);
        assert_eq!(pa.pa_length(), 2);
        assert_eq!(pa.path[1].r#type, PE_TYPE__CURVETO);
        // the control points of a quarter circle: 4/3 (sqrt 2 - 1)
        let k = 4.0 / 3.0 * (core::f64::consts::SQRT_2 - 1.0);
        assert!((pa.path[1].p[0].y - k).abs() < 1e-12);
        assert!((cp.x).abs() < 1e-12 && (cp.y - 1.0).abs() < 1e-12);
    }
}
