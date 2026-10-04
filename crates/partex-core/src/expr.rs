//! e-TeX's expressions: `\numexpr`, `\dimexpr`, `\glueexpr`, `\muexpr`
//! (pdfTeX part 53a), and the other e-TeX quantities `\the` can fetch.
//!
//! An expression is evaluated left to right with the usual precedence,
//! parenthesized subexpressions on an explicit stack. Results are exactly
//! e-TeX's: products and quotients round half away from zero, `a*b/c`
//! uses the exact product, and a value out of range makes the whole
//! expression an arithmetic overflow (zero, after an error).

use crate::pdf::PdfLast;
use alloc::vec::Vec;

use partex_engine::node::{GlueSpec, Node, Order};

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// The state of an expression or a term so far.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Op {
    /// `(` seen, or `( expr )`
    None,
    Add,
    Sub,
    Mult,
    Div,
    /// `term * factor /` seen
    Scale,
}

/// A value of an expression: an integer or dimension, or glue (with
/// the glue's origin while it is a register's value unchanged: e-TeX
/// then assigns the same glue, see `Tex::glue_origin`).
#[derive(Clone, Copy)]
enum Val {
    N(i32),
    G(GlueSpec, Option<u64>),
}

impl Val {
    fn n(self) -> i32 {
        match self {
            Val::N(n) => n,
            Val::G(g, _) => g.width,
        }
    }
    fn g(self) -> GlueSpec {
        match self {
            Val::G(g, _) => g,
            Val::N(_) => GlueSpec::ZERO_GLUE,
        }
    }
}

/// A suspended expression, while a parenthesized one is evaluated.
struct Frame {
    level: i32,
    s: Op,
    r: Op,
    e: Val,
    t: Val,
    n: i32,
}

/// e-TeX's `add_or_sub`: `x±y` if its absolute value is at most `max`.
fn add_or_sub(x: i32, y: i32, max: i32, negative: bool, error: &mut bool) -> i32 {
    let y = if negative {
        -i64::from(y)
    } else {
        i64::from(y)
    };
    let a = i64::from(x) + y;
    if a.abs() > i64::from(max) {
        *error = true;
        0
    } else {
        i32::try_from(a).unwrap_or(0)
    }
}

/// e-TeX's `quotient`: `n/d` rounded, halves away from zero.
fn quotient(n: i32, d: i32, error: &mut bool) -> i32 {
    if d == 0 {
        *error = true;
        return 0;
    }
    let negative = (n < 0) != (d < 0);
    let (n, d) = (i64::from(n).abs(), i64::from(d).abs());
    let mut a = n / d;
    if 2 * (n - a * d) >= d {
        a += 1;
    }
    i32::try_from(if negative { -a } else { a }).unwrap_or(0)
}

use partex_engine::scaled::fract;

/// `normalize_glue`: no order for a zero component.
fn normalize(mut g: GlueSpec) -> GlueSpec {
    if g.stretch == 0 {
        g.stretch_order = Order::Normal;
    }
    if g.shrink == 0 {
        g.shrink_order = Order::Normal;
    }
    g
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// e-TeX: scan an expression of type `cur_val_level`; its value goes to
    /// `cur_val` (or `cur_glue`).
    pub(crate) fn scan_expr(&mut self) -> Result<(), Jump> {
        let mut l = self.cur_val_level;
        let a = self.arith_error;
        let mut b = false;
        self.expand_depth_count += 1;
        if self.expand_depth_count >= self.params.expand_depth {
            return self.overflow(b"expansion depth", self.params.expand_depth);
        }
        let zero = |l: i32| {
            if l >= GLUE_VAL {
                Val::G(GlueSpec::ZERO_GLUE, None)
            } else {
                Val::N(0)
            }
        };
        let mut stack: Vec<Frame> = Vec::new();
        let (mut r, mut s, mut e, mut t, mut n);
        let e = 'done: loop {
            // restart:
            (r, s, e, t, n) = (Op::None, Op::None, zero(l), zero(l), 0);
            'term: loop {
                // continue: scan a factor `f` of type `o`, or start a
                // subexpression
                let o = if s == Op::None { l } else { INT_VAL };
                self.get_nonblank_noncall()?;
                if self.cur_tok == OTHER_TOKEN + i32::from(b'(') {
                    stack.push(Frame {
                        level: l,
                        s,
                        r,
                        e,
                        t,
                        n,
                    });
                    l = o;
                    continue 'done;
                }
                self.back_input()?;
                let mut f = match o {
                    INT_VAL => {
                        self.scan_int()?;
                        Val::N(self.cur_val)
                    }
                    DIMEN_VAL => {
                        self.scan_normal_dimen()?;
                        Val::N(self.cur_val)
                    }
                    GLUE_VAL => {
                        self.scan_glue(GLUE_VAL)?;
                        Val::G(self.cur_glue, self.glue_origin)
                    }
                    _ => {
                        self.scan_glue(MU_VAL)?;
                        Val::G(self.cur_glue, self.glue_origin)
                    }
                };
                loop {
                    // found: scan the next operator and set `o`
                    self.get_nonblank_noncall()?;
                    let mut o = match u8::try_from(self.cur_tok - OTHER_TOKEN).unwrap_or(0) {
                        _ if !(OTHER_TOKEN..OTHER_TOKEN + 256).contains(&self.cur_tok) => Op::None,
                        b'+' => Op::Add,
                        b'-' => Op::Sub,
                        b'*' => Op::Mult,
                        b'/' => Op::Div,
                        _ => Op::None,
                    };
                    if o == Op::None {
                        if stack.is_empty() {
                            if self.cur_cmd != RELAX {
                                self.back_input()?;
                            }
                        } else if self.cur_tok != OTHER_TOKEN + i32::from(b')') {
                            self.print_err(b"Missing ) inserted for expression");
                            self.help(&[
                                b"I was expecting to see `+', `-', `*', `/', or `)'. Didn't.",
                            ]);
                            self.back_error()?;
                        }
                    }
                    self.arith_error = b;
                    // make sure that `f` is in the proper range
                    if l == INT_VAL || s > Op::Sub {
                        if f.n() == i32::MIN {
                            self.arith_error = true;
                            f = Val::N(0);
                        }
                    } else if l == DIMEN_VAL {
                        if f.n().unsigned_abs() > MAX_DIMEN.unsigned_abs() {
                            self.arith_error = true;
                            f = Val::N(0);
                        }
                    } else {
                        let g = f.g();
                        if [g.width, g.stretch, g.shrink]
                            .iter()
                            .any(|c| c.unsigned_abs() > MAX_DIMEN.unsigned_abs())
                        {
                            self.arith_error = true;
                            f = Val::G(GlueSpec::ZERO_GLUE.copy(), None);
                        }
                    }
                    // evaluate the current term
                    let mut overflow = false;
                    let err = &mut overflow;
                    match s {
                        Op::None => {
                            t = match f {
                                Val::G(g, _) if o != Op::None => Val::G(normalize(g), None),
                                f => f,
                            };
                        }
                        Op::Mult if o == Op::Div => {
                            n = f.n();
                            o = Op::Scale;
                        }
                        Op::Mult => {
                            let f = f.n();
                            t = match (l, t) {
                                (INT_VAL, t) => Val::N(self.mult_integers(t.n(), f)),
                                (DIMEN_VAL, t) => Val::N(self.nx_plus_y(t.n(), f, 0)),
                                (_, t) => {
                                    let mut g = t.g();
                                    g.width = self.nx_plus_y(g.width, f, 0);
                                    g.stretch = self.nx_plus_y(g.stretch, f, 0);
                                    g.shrink = self.nx_plus_y(g.shrink, f, 0);
                                    Val::G(g, None)
                                }
                            };
                        }
                        Op::Div => {
                            let f = f.n();
                            t = match t {
                                Val::N(t) => Val::N(quotient(t, f, err)),
                                Val::G(mut g, _) => {
                                    g.width = quotient(g.width, f, err);
                                    g.stretch = quotient(g.stretch, f, err);
                                    g.shrink = quotient(g.shrink, f, err);
                                    Val::G(g, None)
                                }
                            };
                        }
                        Op::Scale => {
                            let f = f.n();
                            let max = if l == INT_VAL { INFINITY } else { MAX_DIMEN };
                            t = match t {
                                Val::N(t) => Val::N(fract(t, n, f, max, err)),
                                Val::G(mut g, _) => {
                                    g.width = fract(g.width, n, f, max, err);
                                    g.stretch = fract(g.stretch, n, f, max, err);
                                    g.shrink = fract(g.shrink, n, f, max, err);
                                    Val::G(g, None)
                                }
                            };
                        }
                        Op::Add | Op::Sub => {} // (not a term state)
                    }
                    self.arith_error |= overflow;
                    if o > Op::Sub {
                        s = o;
                    } else {
                        // evaluate the current expression
                        s = Op::None;
                        let neg = r == Op::Sub;
                        let mut overflow = false;
                        let err = &mut overflow;
                        e = if r == Op::None {
                            t
                        } else if l == INT_VAL {
                            Val::N(add_or_sub(e.n(), t.n(), INFINITY, neg, err))
                        } else if l == DIMEN_VAL {
                            Val::N(add_or_sub(e.n(), t.n(), MAX_DIMEN, neg, err))
                        } else {
                            // the sum or difference of two glue specs
                            let (mut x, y) = (e.g(), t.g());
                            x.width = add_or_sub(x.width, y.width, MAX_DIMEN, neg, err);
                            if x.stretch_order == y.stretch_order {
                                x.stretch = add_or_sub(x.stretch, y.stretch, MAX_DIMEN, neg, err);
                            } else if x.stretch_order < y.stretch_order && y.stretch != 0 {
                                x.stretch = y.stretch;
                                x.stretch_order = y.stretch_order;
                            }
                            if x.shrink_order == y.shrink_order {
                                x.shrink = add_or_sub(x.shrink, y.shrink, MAX_DIMEN, neg, err);
                            } else if x.shrink_order < y.shrink_order && y.shrink != 0 {
                                x.shrink = y.shrink;
                                x.shrink_order = y.shrink_order;
                            }
                            Val::G(normalize(x), None)
                        };
                        self.arith_error |= overflow;
                        r = o;
                    }
                    b = self.arith_error;
                    if o != Op::None {
                        continue 'term;
                    }
                    let Some(p) = stack.pop() else {
                        break 'done e;
                    };
                    // pop the expression stack and `goto found`
                    f = e;
                    (l, s, r, e, t, n) = (p.level, p.s, p.r, p.e, p.t, p.n);
                }
            }
        };
        self.expand_depth_count -= 1;
        let mut e = e;
        if b {
            self.print_err(b"Arithmetic overflow");
            self.help(&[
                b"I can't evaluate this expression,",
                b"since the result is out of range.",
            ]);
            self.error()?;
            e = zero(l);
        }
        self.arith_error = a;
        match e {
            Val::N(v) => self.cur_val = v,
            Val::G(g, origin) => {
                self.cur_glue = g;
                self.glue_origin = origin;
            }
        }
        self.cur_val_level = l;
        Ok(())
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// e-TeX: the last node of the current list as `\lastpenalty` and its
    /// kin see it (a final `\endM` is skipped), and whether there is none.
    pub(crate) fn effective_tail(&self) -> (Option<Node>, bool) {
        if self.mode().abs() == MMODE {
            return match self.mlist().last() {
                None => (None, true),
                Some(_) => (self.tail_item().cloned(), false),
            };
        }
        let list = self.nodes();
        let end = match list.last() {
            Some(Node::Math { subtype, .. }) if i32::from(*subtype) == END_M_CODE => list.len() - 1,
            _ => list.len(),
        };
        if end == 0 {
            return (None, true);
        }
        let n = match &list[end - 1] {
            Node::Disc(d) if !d.replace.is_empty() => d.replace.last(),
            n => Some(n),
        };
        (n.cloned(), false)
    }

    /// e-TeX's and pdfTeX's integers of `last_item` (`m` is the code).
    pub(crate) fn fetch_etex_int(&mut self, m: i32) -> Result<i32, Jump> {
        Ok(match m {
            PDFTEX_VERSION_CODE => PDFTEX_VERSION,
            ELAPSED_TIME_CODE => self.elapsed_time(),
            RANDOM_SEED_CODE => {
                self.random_read();
                self.random.random_seed
            }
            PDF_SHELL_ESCAPE_CODE => match (self.params.shell_escape, self.params.restricted_shell)
            {
                (false, _) => 0,
                (true, true) => 2,
                (true, false) => 1,
            },
            ETEX_VERSION_CODE => ETEX_VERSION,
            CURRENT_GROUP_LEVEL_CODE => self.cur_level() - LEVEL_ONE,
            CURRENT_GROUP_TYPE_CODE => self.cur_group(),
            CURRENT_IF_LEVEL_CODE => {
                self.cond_read();
                i32::try_from(self.cond_stack.len()).unwrap_or(i32::MAX)
            }
            CURRENT_IF_TYPE_CODE => {
                self.cond_read();
                if self.cond_stack.is_empty() {
                    0
                } else if self.cur_if < UNLESS_CODE {
                    self.cur_if + 1
                } else {
                    -(self.cur_if - UNLESS_CODE + 1)
                }
            }
            CURRENT_IF_BRANCH_CODE => match self.if_limit {
                OR_CODE | ELSE_CODE => 1,
                FI_CODE => -1,
                _ => 0,
            },
            GLUE_STRETCH_ORDER_CODE | GLUE_SHRINK_ORDER_CODE => {
                self.scan_glue(GLUE_VAL)?;
                let g = self.cur_glue;
                let o = if m == GLUE_STRETCH_ORDER_CODE {
                    g.stretch_order
                } else {
                    g.shrink_order
                };
                i32::from(o as u8)
            }
            // pdfTeX §447
            // (each value is read through its cell; a virtual number's
            // final one also reads the numbering, `final_num`)
            PDF_LAST_OBJ_CODE => {
                let k = self.pdf_last(PdfLast::Obj);
                self.obj_observe();
                self.pdf.objs.final_num(k)
            }
            PDF_LAST_XFORM_CODE => {
                let k = self.pdf_last(PdfLast::XForm);
                self.obj_observe();
                self.pdf.objs.final_num(k)
            }
            PDF_LAST_XIMAGE_CODE => {
                let k = self.pdf_last(PdfLast::XImage);
                self.obj_observe();
                self.pdf.objs.final_num(k)
            }
            PDF_LAST_XIMAGE_PAGES_CODE => self.pdf_last(PdfLast::XImagePages),
            PDF_LAST_ANNOT_CODE => {
                let k = self.pdf_last(PdfLast::Annot);
                self.obj_observe();
                self.pdf.objs.final_num(k)
            }
            PDF_LAST_X_POS_CODE => self.pdf_last(PdfLast::XPos),
            PDF_LAST_Y_POS_CODE => self.pdf_last(PdfLast::YPos),
            PDF_RETVAL_CODE => self.pdf_last(PdfLast::Retval),
            PDF_LAST_XIMAGE_COLORDEPTH_CODE => self.pdf_last(PdfLast::XImageColordepth),
            PDF_LAST_LINK_CODE => {
                let k = self.pdf_last(PdfLast::Link);
                self.obj_observe();
                self.pdf.objs.final_num(k)
            }
            _ => 0,
        })
    }

    /// e-TeX's dimensions of `last_item`.
    pub(crate) fn fetch_etex_dimen(&mut self, m: i32) -> Result<(), Jump> {
        match m {
            FONT_CHAR_WD_CODE | FONT_CHAR_HT_CODE | FONT_CHAR_DP_CODE | FONT_CHAR_IC_CODE => {
                self.scan_font_ident()?;
                let f = self.cur_val;
                self.scan_char_num()?;
                self.cur_val = self
                    .fonts
                    .get(f)
                    .glyph(self.cur_val)
                    .map_or(0, |g| match m {
                        FONT_CHAR_WD_CODE => g.width,
                        FONT_CHAR_HT_CODE => g.height,
                        FONT_CHAR_DP_CODE => g.depth,
                        _ => g.italic,
                    });
            }
            PAR_SHAPE_LENGTH_CODE | PAR_SHAPE_INDENT_CODE | PAR_SHAPE_DIMEN_CODE => {
                let mut q = m - PAR_SHAPE_LENGTH_CODE;
                self.scan_int()?;
                let shape = self.par_shape().cloned();
                self.cur_val = match shape {
                    Some(s) if self.cur_val > 0 => {
                        let mut k = self.cur_val;
                        if q == 2 {
                            q = k % 2;
                            k = i32::midpoint(k, q);
                        }
                        let k = usize::try_from(k).unwrap_or(0).min(s.len());
                        let (indent, width) = s[k - 1];
                        if q == 1 { indent } else { width }
                    }
                    _ => 0,
                };
            }
            _ => {
                // \gluestretch, \glueshrink
                self.scan_glue(GLUE_VAL)?;
                let g = self.cur_glue;
                self.cur_val = if m == GLUE_STRETCH_CODE {
                    g.stretch
                } else {
                    g.shrink
                };
            }
        }
        Ok(())
    }

    /// e-TeX's glue of `last_item`, and expressions.
    pub(crate) fn fetch_etex_glue(&mut self, m: i32) -> Result<(), Jump> {
        if m < ETEX_MU {
            // \mutoglue
            self.scan_glue(MU_VAL)?;
            self.cur_val_level = GLUE_VAL;
        } else if m < ETEX_EXPR {
            // \gluetomu
            self.scan_glue(GLUE_VAL)?;
            self.cur_val_level = MU_VAL;
        } else {
            self.cur_val_level = m - ETEX_EXPR + INT_VAL;
            self.scan_expr()?;
        }
        Ok(())
    }

    /// pdfTeX's `\pdfelapsedtime`: scaled seconds since the start (or
    /// `\pdfresettimer`).
    pub(crate) fn elapsed_time(&mut self) -> i32 {
        let (s, m) = self.host.seconds_and_micros();
        self.clock_read(crate::track::Query::Timer, &(s, m));
        let (es, em) = self.epoch();
        if s - es > 32767 {
            i32::MAX
        } else if em > m {
            (s - 1 - es) * 65536 + (((m + 1_000_000 - em) / 100) * 65536) / 10000
        } else {
            (s - es) * 65536 + (((m - em) / 100) * 65536) / 10000
        }
    }
}
