//! mpost.c: the inline PostScript dvips-style specials carry (`ps:`,
//! `" `), interpreted as dvipdfmx does: a stack of numbers and PDF objects,
//! the path, graphics state and transformation operators (`mps_exec_inline`,
//! `mp_parse_body`, `do_operator`).
//!
//! Not ported: MetaPost file inclusion (`mps_include_page`,
//! `mps_do_page`), the font operators (`findfont`, `scalefont`,
//! `setfont`, `currentfont`, `show`, `fshow`) and `startTexFig` /
//! `endTexFig`: they fail as an unknown operator does (the special's
//! "Interpreting PS code failed" warning). graphicx's and color's `dvips`
//! drivers use none of them.

use crate::parse::{parse_ident, skip_white};
use crate::pdfcolor::PdfColor;
use crate::pdfdev::{PDF_DEV_PARAM_AUTOROTATE, PdfCoord, PdfTmatrix, pdf_setmatrix};
use crate::pdfdraw::{PDF_DASH_SIZE_MAX, PDF_FILL_RULE_NONZERO};
use crate::prelude::*;

/// `MP_CMODE_MPOST`.
const MP_CMODE_MPOST: i32 = 0;
/// `MP_CMODE_DVIPSK`.
const MP_CMODE_DVIPSK: i32 = 1;
/// `MP_CMODE_PTEXVERT`.
const MP_CMODE_PTEXVERT: i32 = 2;

/// `PS_STACK_SIZE`.
const PS_STACK_SIZE: usize = 1024;

/// `DEVICE_RESOLUTION`.
const DEVICE_RESOLUTION: f64 = 1000.0;

/// mpost.c's `static` state.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `stack` (`top_stack` is its length).
    pub stack: Vec<Obj>,
    /// `mp_cmode`.
    pub cmode: i32,
}

/// The operators (`ps_operators`, then `mps_operators`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Neg,
    Truncate,
    Clear,
    Exch,
    Pop,
    Newpath,
    Closepath,
    Moveto,
    Rmoveto,
    Curveto,
    Rcurveto,
    Lineto,
    Rlineto,
    Arc,
    Arcn,
    Fill,
    Stroke,
    Show,
    Clip,
    Eoclip,
    Showpage,
    Gsave,
    Grestore,
    Concat,
    Scale,
    Translate,
    Rotate,
    Setlinewidth,
    Setdash,
    Setlinecap,
    Setlinejoin,
    Setmiterlimit,
    Setgray,
    Setrgbcolor,
    Setcmykcolor,
    Currentpoint,
    Idtransform,
    Dtransform,
    Findfont,
    Scalefont,
    Setfont,
    Currentfont,
    Stringwidth,
    Def,
    Fshow,
    Stexfig,
    Etexfig,
    Hlw,
    Vlw,
    Rd,
    B,
}

/// `ps_operators`.
const PS_OPERATORS: [(&[u8], Op); 46] = [
    (b"add", Op::Add),
    (b"mul", Op::Mul),
    (b"div", Op::Div),
    (b"neg", Op::Neg),
    (b"sub", Op::Sub),
    (b"truncate", Op::Truncate),
    (b"clear", Op::Clear),
    (b"exch", Op::Exch),
    (b"pop", Op::Pop),
    (b"clip", Op::Clip),
    (b"eoclip", Op::Eoclip),
    (b"closepath", Op::Closepath),
    (b"concat", Op::Concat),
    (b"newpath", Op::Newpath),
    (b"moveto", Op::Moveto),
    (b"rmoveto", Op::Rmoveto),
    (b"lineto", Op::Lineto),
    (b"rlineto", Op::Rlineto),
    (b"curveto", Op::Curveto),
    (b"rcurveto", Op::Rcurveto),
    (b"arc", Op::Arc),
    (b"arcn", Op::Arcn),
    (b"stroke", Op::Stroke),
    (b"fill", Op::Fill),
    (b"show", Op::Show),
    (b"showpage", Op::Showpage),
    (b"gsave", Op::Gsave),
    (b"grestore", Op::Grestore),
    (b"translate", Op::Translate),
    (b"rotate", Op::Rotate),
    (b"scale", Op::Scale),
    (b"setlinecap", Op::Setlinecap),
    (b"setlinejoin", Op::Setlinejoin),
    (b"setlinewidth", Op::Setlinewidth),
    (b"setmiterlimit", Op::Setmiterlimit),
    (b"setdash", Op::Setdash),
    (b"setgray", Op::Setgray),
    (b"setrgbcolor", Op::Setrgbcolor),
    (b"setcmykcolor", Op::Setcmykcolor),
    (b"currentpoint", Op::Currentpoint),
    (b"dtransform", Op::Dtransform),
    (b"idtransform", Op::Idtransform),
    (b"findfont", Op::Findfont),
    (b"scalefont", Op::Scalefont),
    (b"setfont", Op::Setfont),
    (b"currentfont", Op::Currentfont),
];

/// The rest of `ps_operators` (`stringwidth`, `def`) and `mps_operators`.
const MPS_OPERATORS: [(&[u8], Op); 30] = [
    (b"stringwidth", Op::Stringwidth),
    (b"def", Op::Def),
    (b"fshow", Op::Fshow),
    (b"startTexFig", Op::Stexfig),
    (b"endTexFig", Op::Etexfig),
    (b"hlw", Op::Hlw),
    (b"vlw", Op::Vlw),
    (b"l", Op::Lineto),
    (b"r", Op::Rlineto),
    (b"c", Op::Curveto),
    (b"m", Op::Moveto),
    (b"p", Op::Closepath),
    (b"n", Op::Newpath),
    (b"C", Op::Setcmykcolor),
    (b"G", Op::Setgray),
    (b"R", Op::Setrgbcolor),
    (b"lj", Op::Setlinejoin),
    (b"ml", Op::Setmiterlimit),
    (b"lc", Op::Setlinecap),
    (b"S", Op::Stroke),
    (b"F", Op::Fill),
    (b"q", Op::Gsave),
    (b"Q", Op::Grestore),
    (b"s", Op::Scale),
    (b"t", Op::Concat),
    (b"sd", Op::Setdash),
    (b"rd", Op::Rd),
    (b"P", Op::Showpage),
    (b"B", Op::B),
    (b"W", Op::Clip),
];

/// `get_opcode`.
fn get_opcode(token: &[u8]) -> Option<Op> {
    PS_OPERATORS
        .iter()
        .chain(MPS_OPERATORS.iter())
        .find(|(t, _)| *t == token)
        .map(|&(_, op)| op)
}

impl Dpx {
    /// `PUSH_STACK`: false on overflow (C's `*error = 1`).
    fn mps_push(&mut self, o: Obj) -> bool {
        if self.mpost.stack.len() < PS_STACK_SIZE {
            self.mpost.stack.push(o);
            true
        } else {
            warn!("PS stack overflow including MetaPost file or inline PS code");
            false
        }
    }

    /// `PUSH(pdf_new_number(v))`: the error (1 on overflow).
    fn mps_push_number(&mut self, v: f64) -> i32 {
        let o = self.o.new_number(v);
        i32::from(!self.mps_push(o))
    }

    /// `pop_get_numbers`: `count` numbers into `values` (the deepest
    /// first); 0, or C's `count + 1` where it stopped.
    fn pop_get_numbers(&mut self, values: &mut [f64], count: usize) -> Result<i32> {
        let mut count = count as i32;
        while count > 0 {
            count -= 1;
            let Some(tmp) = self.mpost.stack.pop() else {
                warn!("mpost: Stack underflow.");
                return Ok(count + 1);
            };
            if !self.o.is_number(Some(tmp)) {
                warn!("mpost: Not a number!");
                self.o.release(tmp)?;
                return Ok(count + 1);
            }
            values[count as usize] = self.o.number_value(tmp)?;
            self.o.release(tmp)?;
        }
        Ok(0)
    }

    /// `cvr_array`: `count` numbers of `array` (released) into `values`;
    /// 0, or C's `count + 1` where it stopped.
    fn cvr_array(&mut self, array: Option<Obj>, values: &mut [f64], count: usize) -> Result<i32> {
        let mut count = count as i32;
        let mut stopped = None;
        if let Some(a) = array.filter(|&a| self.o.is_array(Some(a))) {
            while count > 0 {
                count -= 1;
                let tmp = self.o.get_array(a, count)?;
                let Some(t) = tmp.filter(|&t| self.o.is_number(Some(t))) else {
                    warn!("mpost: Not a number!");
                    stopped = Some(count + 1);
                    break;
                };
                values[count as usize] = self.o.number_value(t)?;
            }
        } else {
            warn!("mpost: Not an array!");
            stopped = Some(count + 1);
        }
        if let Some(a) = array {
            self.o.release(a)?;
        }
        Ok(stopped.unwrap_or(0))
    }

    /// `do_exch`.
    fn mps_do_exch(&mut self) -> i32 {
        let n = self.mpost.stack.len();
        if n < 2 {
            return -1;
        }
        self.mpost.stack.swap(n - 1, n - 2);
        0
    }

    /// `do_clear`.
    fn mps_do_clear(&mut self) -> Result<i32> {
        while let Some(tmp) = self.mpost.stack.pop() {
            self.o.release(tmp)?;
        }
        Ok(0)
    }

    /// `mps_stack_clear_to`.
    pub fn mps_stack_clear_to(&mut self, depth: usize) -> Result<()> {
        while self.mpost.stack.len() > depth {
            let tmp = self.mpost.stack.pop().expect("an object");
            self.o.release(tmp)?;
        }
        Ok(())
    }

    /// `ps_dev_CTM`.
    fn ps_dev_ctm(&self) -> PdfTmatrix {
        let (_, mut m) = self.pdf_dev_currentmatrix();
        m.a *= DEVICE_RESOLUTION;
        m.b *= DEVICE_RESOLUTION;
        m.c *= DEVICE_RESOLUTION;
        m.d *= DEVICE_RESOLUTION;
        m.e *= DEVICE_RESOLUTION;
        m.f *= DEVICE_RESOLUTION;
        m
    }

    /// `do_mpost_bind_def`: `ps_code` interpreted.
    fn do_mpost_bind_def(&mut self, ps_code: &[u8], x_user: f64, y_user: f64) -> Result<i32> {
        let mut p = 0;
        self.mp_parse_body(ps_code, &mut p, x_user, y_user)
    }

    /// `DTRANSFORM` and `IDTRANSFORM`: the point (and an optional matrix)
    /// off the stack, transformed, pushed back; the error.
    fn mps_transform_op(&mut self, inverse: bool) -> Result<i32> {
        let mut values = [0.0f64; 12];
        let mut matrix = PdfTmatrix::default();
        let mut has_matrix = false;
        let mut tmp = self.mpost.stack.pop();
        if self.o.is_array(tmp) {
            let error = self.cvr_array(tmp, &mut values, 6)?;
            if error != 0 {
                return Ok(error);
            }
            pdf_setmatrix(
                &mut matrix,
                values[0],
                values[1],
                values[2],
                values[3],
                values[4],
                values[5],
            );
            tmp = self.mpost.stack.pop();
            has_matrix = true;
        }
        if !self.o.is_number(tmp) {
            return Ok(1);
        }
        let t = tmp.expect("a number");
        let y = self.o.number_value(t)?;
        self.o.release(t)?;
        let tmp = self.mpost.stack.pop();
        if !self.o.is_number(tmp) {
            return Ok(1);
        }
        let t = tmp.expect("a number");
        let x = self.o.number_value(t)?;
        self.o.release(t)?;
        if !has_matrix {
            // (here, we need the real PostScript CTM)
            matrix = self.ps_dev_ctm();
        }
        let mut cp = PdfCoord { x, y };
        if inverse {
            self.pdf_dev_idtransform(&mut cp, Some(&matrix));
        } else {
            self.pdf_dev_dtransform(&mut cp, Some(&matrix));
        }
        let mut error = self.mps_push_number(cp.x);
        if error == 0 {
            error = self.mps_push_number(cp.y);
        }
        Ok(error)
    }

    /// `do_operator`: the error.
    #[allow(clippy::too_many_lines)]
    fn do_operator(&mut self, token: &[u8], x_user: f64, y_user: f64) -> Result<i32> {
        let mut values = [0.0f64; 12];
        let mut matrix = PdfTmatrix::default();
        let Some(op) = get_opcode(token) else {
            // (`is_fontname`: no MetaPost fonts here)
            warn!("Unknown token \"{}\"", String::from_utf8_lossy(token));
            return Ok(1);
        };
        let mut error = 0;
        match op {
            Op::Add | Op::Mul | Op::Sub | Op::Div => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    let v = match op {
                        Op::Add => values[0] + values[1],
                        Op::Mul => values[0] * values[1],
                        Op::Sub => values[0] - values[1],
                        _ => values[0] / values[1],
                    };
                    error = self.mps_push_number(v);
                }
            }
            Op::Neg => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    error = self.mps_push_number(-values[0]);
                }
            }
            Op::Truncate => {
                // (round toward zero)
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    let v = if values[0] > 0.0 {
                        libm::floor(values[0])
                    } else {
                        libm::ceil(values[0])
                    };
                    error = self.mps_push_number(v);
                }
            }
            Op::Clear => error = self.mps_do_clear()?,
            Op::Pop => {
                if let Some(tmp) = self.mpost.stack.pop() {
                    self.o.release(tmp)?;
                }
            }
            Op::Exch => error = self.mps_do_exch(),
            Op::Moveto => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    error = self.pdf_dev_moveto(values[0], values[1]);
                }
            }
            Op::Rmoveto => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    error = self.pdf_dev_rmoveto(values[0], values[1]);
                }
            }
            Op::Lineto => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    error = self.pdf_dev_lineto(values[0], values[1]);
                }
            }
            Op::Rlineto => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    error = self.pdf_dev_rlineto(values[0], values[1]);
                }
            }
            Op::Curveto => {
                error = self.pop_get_numbers(&mut values, 6)?;
                if error == 0 {
                    error = self.pdf_dev_curveto(
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    );
                }
            }
            Op::Rcurveto => {
                error = self.pop_get_numbers(&mut values, 6)?;
                if error == 0 {
                    error = self.pdf_dev_rcurveto(
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    );
                }
            }
            Op::Closepath => error = self.pdf_dev_closepath(),
            Op::Arc | Op::Arcn => {
                error = self.pop_get_numbers(&mut values, 5)?;
                if error == 0 {
                    error = if op == Op::Arc {
                        self.pdf_dev_arc(values[0], values[1], values[2], values[3], values[4])
                    } else {
                        self.pdf_dev_arcn(values[0], values[1], values[2], values[3], values[4])
                    };
                }
            }
            Op::Newpath => {
                self.pdf_dev_newpath()?;
            }
            Op::Stroke => {
                // (fill rule not supported yet)
                self.pdf_dev_flushpath(b'S', PDF_FILL_RULE_NONZERO)?;
            }
            Op::Fill => {
                self.pdf_dev_flushpath(b'f', PDF_FILL_RULE_NONZERO)?;
            }
            Op::Clip => error = self.pdf_dev_clip()?,
            Op::Eoclip => error = self.pdf_dev_eoclip()?,
            // (`save_font`, `restore_font`: no MetaPost fonts here)
            Op::Gsave => error = self.pdf_dev_gsave()?,
            Op::Grestore => error = self.pdf_dev_grestore()?,
            Op::Concat => {
                let tmp = self.mpost.stack.pop();
                error = self.cvr_array(tmp, &mut values, 6)?;
                if error != 0 {
                    warn!("Missing array before \"concat\".");
                } else {
                    pdf_setmatrix(
                        &mut matrix,
                        values[0],
                        values[1],
                        values[2],
                        values[3],
                        values[4],
                        values[5],
                    );
                    error = self.pdf_dev_concat(&matrix)?;
                }
            }
            Op::Scale => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    if self.mpost.cmode == MP_CMODE_PTEXVERT {
                        pdf_setmatrix(&mut matrix, values[1], 0.0, 0.0, values[0], 0.0, 0.0);
                    } else {
                        pdf_setmatrix(&mut matrix, values[0], 0.0, 0.0, values[1], 0.0, 0.0);
                    }
                    error = self.pdf_dev_concat(&matrix)?;
                }
            }
            // (positive angle means clockwise in graphicx-dvips)
            Op::Rotate => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    let a = values[0] * core::f64::consts::PI / 180.0;
                    let (c, s) = (libm::cos(a), libm::sin(a));
                    match self.mpost.cmode {
                        MP_CMODE_DVIPSK | MP_CMODE_MPOST | MP_CMODE_PTEXVERT => {
                            pdf_setmatrix(&mut matrix, c, -s, s, c, 0.0, 0.0);
                        }
                        _ => pdf_setmatrix(&mut matrix, c, s, -s, c, 0.0, 0.0),
                    }
                    error = self.pdf_dev_concat(&matrix)?;
                }
            }
            Op::Translate => {
                error = self.pop_get_numbers(&mut values, 2)?;
                if error == 0 {
                    pdf_setmatrix(&mut matrix, 1.0, 0.0, 0.0, 1.0, values[0], values[1]);
                    error = self.pdf_dev_concat(&matrix)?;
                }
            }
            Op::Setdash => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    let offset = values[0];
                    let pattern = self.mpost.stack.pop();
                    if !self.o.is_array(pattern) {
                        if let Some(p) = pattern {
                            self.o.release(p)?;
                        }
                        return Ok(1);
                    }
                    let pattern = pattern.expect("an array");
                    let num_dashes = self.o.array_length(pattern)?;
                    if num_dashes > PDF_DASH_SIZE_MAX {
                        warn!("Too many dashes...");
                        self.o.release(pattern)?;
                        return Ok(1);
                    }
                    let mut dash_values = Vec::with_capacity(num_dashes);
                    for i in 0..num_dashes {
                        let dash = self.o.get_array(pattern, i as i32)?;
                        if !self.o.is_number(dash) {
                            error = 1;
                            break;
                        }
                        dash_values.push(self.o.number_value(dash.expect("a number"))?);
                    }
                    self.o.release(pattern)?;
                    if error == 0 {
                        error = self.pdf_dev_setdash(&dash_values, offset)?;
                    }
                }
            }
            Op::Setlinecap => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    error = self.pdf_dev_setlinecap(values[0] as i32)?;
                }
            }
            Op::Setlinejoin => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    error = self.pdf_dev_setlinejoin(values[0] as i32)?;
                }
            }
            Op::Setlinewidth => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    error = self.pdf_dev_setlinewidth(values[0])?;
                }
            }
            Op::Setmiterlimit => {
                error = self.pop_get_numbers(&mut values, 1)?;
                if error == 0 {
                    error = self.pdf_dev_setmiterlimit(values[0])?;
                }
            }
            // (not handled properly, as in C)
            Op::Setcmykcolor | Op::Setgray | Op::Setrgbcolor => {
                let n = match op {
                    Op::Setcmykcolor => 4,
                    Op::Setgray => 1,
                    _ => 3,
                };
                error = self.pop_get_numbers(&mut values, n)?;
                if error == 0 {
                    let mut color = PdfColor::default();
                    match op {
                        Op::Setcmykcolor => {
                            color.pdf_color_cmykcolor(values[0], values[1], values[2], values[3]);
                        }
                        Op::Setgray => {
                            color.pdf_color_graycolor(values[0]);
                        }
                        _ => {
                            color.pdf_color_rgbcolor(values[0], values[1], values[2]);
                        }
                    }
                    self.pdf_dev_set_strokingcolor(&color)?;
                    self.pdf_dev_set_nonstrokingcolor(&color)?;
                }
            }
            // (ignored for now, as in C)
            Op::Showpage => {}
            Op::Currentpoint => {
                let (e, cp) = self.pdf_dev_currentpoint();
                error = e;
                if error == 0 {
                    error = self.mps_push_number(cp.x);
                    if error == 0 {
                        error = self.mps_push_number(cp.y);
                    }
                }
            }
            Op::Dtransform => error = self.mps_transform_op(false)?,
            Op::Idtransform => error = self.mps_transform_op(true)?,
            // (not ported: MetaPost's fonts, `stringwidth`, texfig)
            Op::Findfont
            | Op::Scalefont
            | Op::Setfont
            | Op::Currentfont
            | Op::Show
            | Op::Stringwidth
            | Op::Fshow
            | Op::Stexfig
            | Op::Etexfig => {
                warn!(
                    "Unsupported PostScript operator \"{}\"",
                    String::from_utf8_lossy(token)
                );
                error = 1;
            }
            Op::Hlw => {
                error = self.do_mpost_bind_def(
                    b"0 dtransform exch truncate exch idtransform pop setlinewidth",
                    x_user,
                    y_user,
                )?
            }
            Op::Vlw => {
                error = self.do_mpost_bind_def(
                    b"0 exch dtransform truncate idtransform setlinewidth pop",
                    x_user,
                    y_user,
                )?
            }
            Op::Rd => error = self.do_mpost_bind_def(b"[] 0 setdash", x_user, y_user)?,
            Op::B => error = self.do_mpost_bind_def(b"gsave fill grestore", x_user, y_user)?,
            Op::Def => {
                // (not implemented in C: two objects dropped, not released)
                self.mpost.stack.pop();
                self.mpost.stack.pop();
            }
        }
        Ok(error)
    }

    /// `mp_parse_body`: the PostScript in `s` from `*p`; the error.
    pub fn mp_parse_body(
        &mut self,
        s: &[u8],
        p: &mut usize,
        x_user: f64,
        y_user: f64,
    ) -> Result<i32> {
        let end = s.len();
        let mut error = 0;
        skip_white(s, p);
        while *p < end && error == 0 {
            let c = s[*p];
            if c.is_ascii_digit() || (*p < end - 1 && (c == b'+' || c == b'-' || c == b'.')) {
                let (value, n) = crate::fmt::strtod(&s[*p..]);
                let next = *p + n;
                if next < end && !b"<([{/%".contains(&s[next]) && !crate::fmt::is_c_space(s[next]) {
                    warn!("Unkown PostScript operator.");
                    error = 1;
                } else {
                    error = self.mps_push_number(value);
                    *p = next;
                }
            } else if c == b'['
                && let Some(obj) = {
                    let mut q = *p;
                    let o = self.o.parse_pdf_array(s, &mut q, None)?;
                    if o.is_some() {
                        *p = q;
                    }
                    o
                }
            {
                error = i32::from(!self.mps_push(obj));
            } else if *p < end - 1
                && c == b'<'
                && s[*p + 1] == b'<'
                && let Some(obj) = {
                    let mut q = *p;
                    let o = self.o.parse_pdf_dict(s, &mut q, None)?;
                    if o.is_some() {
                        *p = q;
                    }
                    o
                }
            {
                error = i32::from(!self.mps_push(obj));
            } else if (c == b'(' || c == b'<')
                && let Some(obj) = {
                    let mut q = *p;
                    let o = self.o.parse_pdf_string(s, &mut q);
                    if o.is_some() {
                        *p = q;
                    }
                    o
                }
            {
                error = i32::from(!self.mps_push(obj));
            } else if c == b'/'
                && let Some(obj) = {
                    let mut q = *p;
                    let o = self.o.parse_pdf_name(s, &mut q);
                    if o.is_some() {
                        *p = q;
                    }
                    o
                }
            {
                error = i32::from(!self.mps_push(obj));
            } else {
                match parse_ident(s, p) {
                    None => error = 1,
                    Some(token) => error = self.do_operator(&token, x_user, y_user)?,
                }
            }
            skip_white(s, p);
        }
        Ok(error)
    }

    /// `mps_eop_cleanup`.
    pub fn mps_eop_cleanup(&mut self) -> Result<()> {
        self.mps_do_clear()?;
        Ok(())
    }

    /// `mps_stack_depth`.
    #[must_use]
    pub fn mps_stack_depth(&self) -> usize {
        self.mpost.stack.len()
    }

    /// `mps_exec_inline`: the PostScript in `s` from `*p`, at the user's
    /// point; the error.
    pub fn mps_exec_inline(
        &mut self,
        s: &[u8],
        p: &mut usize,
        x_user: f64,
        y_user: f64,
    ) -> Result<i32> {
        // (compatibility for dvipsk)
        let dirmode = self.pdf_dev_get_dirmode();
        self.mpost.cmode = if dirmode != 0 {
            MP_CMODE_PTEXVERT
        } else {
            MP_CMODE_DVIPSK
        };
        let autorotate = self.pdf_dev_get_param(PDF_DEV_PARAM_AUTOROTATE);
        self.pdf_dev_set_param(PDF_DEV_PARAM_AUTOROTATE, 0);
        // (dvipdfm: x_user and y_user are off by 0.02 %)
        self.pdf_dev_moveto(x_user, y_user);
        let error = self.mp_parse_body(s, p, x_user, y_user)?;
        self.pdf_dev_set_param(PDF_DEV_PARAM_AUTOROTATE, autorotate);
        self.pdf_dev_set_dirmode(dirmode);
        Ok(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcodes() {
        assert_eq!(get_opcode(b"currentpoint"), Some(Op::Currentpoint));
        assert_eq!(get_opcode(b"q"), Some(Op::Gsave));
        assert_eq!(get_opcode(b"stringwidth"), Some(Op::Stringwidth));
        assert_eq!(get_opcode(b"nothing"), None);
    }
}
