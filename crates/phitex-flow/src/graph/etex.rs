//! e-TeX's extensions (etex.web), in extended mode (`Options::etex`: the
//! primitives are named only then, `val::initial`), as steps and nodes of
//! the same kinds as TeX's own:
//!
//! - expansion steps: `\unexpanded`, `\detokenize` (`the` with chr 1 and
//!   5), `\scantokens` (the text, shown, rewrites the input as a text read
//!   again with the category codes in force: `RState::texts`), `\unless`,
//!   `\ifdefined`, `\ifcsname` (defines nothing), `\iffontchar`;
//! - internal quantities: `\numexpr`, `\dimexpr`, `\glueexpr`, `\muexpr`
//!   (§1511–1544: the expression grammar, rounding, overflow), the glue
//!   and font-character items, `\parshape…`, and the reads of the reader
//!   state — `\currentgrouplevel/type` from `RState::groups`,
//!   `\currentiflevel/type/branch` from `RState::conds`, `\lastnodetype`
//!   from the material emitted before (`Exec::last_item`, CORE.md
//!   Conflict 13);
//! - assignments: `\protected` (a flag of `Macro`, honoured where `\edef`
//!   and `\write` expand), `\interactionmode` (`Key::Interaction`), the new
//!   parameters (plain `IntPar`s and `\everyeof`), `\readline`'s parsing;
//! - material: `\marks` (`Item::Marks`), TeXXeT's `\beginL`… (`Item::Math`).

use std::fmt::Write as _;
use std::sync::Arc;

use super::material::{BoxContext, BoxKind};
use super::math::MathKind;
use super::scan::{IVal, Lv, MAX_DIMEN, mult_and_add, nx_plus_y};
use super::{EOL_UNREAD, Exec, GroupKind, Mode, PendingText, TState, TextKind};
use crate::item::Item;
use crate::print;
use crate::pack::Spec;
use crate::show::{LINE, LN, NL, RAW};
use crate::val::{Glue, IntPar, Key, Meaning, Prim, Tok, Val};

/// `\eTeXversion` and `\eTeXrevision` (etex.web §2).
pub(super) const ETEX_VERSION: i32 = 2;
pub(super) const ETEX_REVISION: &str = ".6";

/// tex.web's `infinity`: the largest integer.
const INFINITY: i32 = 0x7fff_ffff;

/// An expression's state (etex.web §1521): what was seen before the next
/// factor.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Op {
    None,
    Add,
    Sub,
    Mult,
    Div,
    /// `term * factor /` seen.
    Scale,
}

/// A value of an expression: an integer or a dimension, or glue.
#[derive(Clone, Copy, Debug)]
enum EVal {
    N(i32),
    G(Glue),
}

impl EVal {
    fn n(self) -> i32 {
        match self {
            EVal::N(n) => n,
            EVal::G(g) => g.width,
        }
    }
    fn g(self) -> Glue {
        match self {
            EVal::G(g) => g,
            EVal::N(_) => Glue::default(),
        }
    }
}

/// An expression waiting while a parenthesized one is evaluated.
struct Frame {
    l: Lv,
    s: Op,
    r: Op,
    e: EVal,
    t: EVal,
    n: i32,
}

/// etex.web §1537, `add_or_sub`: `x ± y` if its magnitude is at most
/// `max`.
fn add_or_sub(x: i32, y: i32, max: i32, negative: bool, err: &mut bool) -> i32 {
    let y = if negative { -i64::from(y) } else { i64::from(y) };
    let a = i64::from(x) + y;
    if a.abs() > i64::from(max) {
        *err = true;
        0
    } else {
        a as i32
    }
}

/// etex.web §1538, `quotient`: `n / d` rounded, halves away from zero.
fn quotient(n: i32, d: i32, err: &mut bool) -> i32 {
    if d == 0 {
        *err = true;
        return 0;
    }
    let negative = (n < 0) != (d < 0);
    let (n, d) = (i64::from(n).abs(), i64::from(d).abs());
    let mut a = n / d;
    if 2 * (n - a * d) >= d {
        a += 1;
    }
    (if negative { -a } else { a }) as i32
}

/// etex.web §1539, `fract` (`arith::fract`), `err` set on an error.
fn fract(x: i32, n: i32, d: i32, max: i32, err: &mut bool) -> i32 {
    crate::arith::fract(x, n, d, max).unwrap_or_else(|| {
        *err = true;
        0
    })
}

/// `normalize_glue` (etex.web §1529): no order for a zero component.
fn normalize(mut g: Glue) -> Glue {
    if g.stretch == 0 {
        g.stretch_order = 0;
    }
    if g.shrink == 0 {
        g.shrink_order = 0;
    }
    g
}

/// `n * x` within `max` (tex.web 105), `err` set on overflow.
fn mul(n: i32, x: i32, max: i32, err: &mut bool) -> i32 {
    mult_and_add(n, x, 0, max).unwrap_or_else(|| {
        *err = true;
        0
    })
}

/// The tokens `\readline` makes of a line (etex.web §1445): each character
/// of category 12, a space of category 10; the line's end-of-line
/// character, if any, ends it.
pub(super) fn readline_toks(line: &str, eol: Option<char>) -> Vec<Tok> {
    let s: String = line.trim_end_matches(' ').chars().chain(eol).collect();
    print::str_toks(&s)
}

impl Exec<'_> {
    /// Whether the format is e-TeX's extended mode (a read of the format's
    /// value).
    pub(super) fn etex_mode(&mut self) -> bool {
        matches!(self.get(&Key::ETeXMode), Val::Int(1))
    }

    // ------------------------------------------------------------ steps

    /// `\unexpanded`, `\detokenize` (etex.web §1418): a balanced text, as it
    /// is or shown as characters.
    pub(super) fn etex_the(&mut self, p: Prim) -> Vec<Tok> {
        let toks = self.scan_toks_raw();
        if p == Prim::Unexpanded {
            return toks;
        }
        let mut s = String::new();
        self.show_toks(&mut s, &toks);
        print::str_toks(&s)
    }

    /// `\scantokens` (etex.web §1489–1491): the balanced text, shown, cut
    /// into lines at `\newlinechar` (each without its trailing spaces),
    /// becomes a text the input is rewritten with, read on as the piece's
    /// own is.
    pub(super) fn scan_tokens(&mut self) {
        let toks = self.scan_toks_raw();
        let mut s = String::new();
        self.show_toks(&mut s, &toks);
        let nl = u32::try_from(self.int_par(IntPar::Newlinechar)).ok().and_then(char::from_u32);
        let chars: Vec<char> = s.chars().collect();
        let mut text: Vec<char> = Vec::with_capacity(chars.len() + 1);
        let mut l = 0;
        while l < chars.len() {
            let m = l;
            while l < chars.len() && Some(chars[l]) != nl {
                l += 1;
            }
            let mut line = &chars[m..l];
            while let [rest @ .., ' '] = line {
                line = rest;
            }
            text.extend_from_slice(line);
            text.push('\r');
            l += 1;
        }
        let at = self.st.pending.len();
        self.st.texts.push(PendingText {
            text: Arc::from(text),
            at,
            pos: 0,
            tstate: TState::N,
            eol: EOL_UNREAD,
            eof: false,
            kind: TextKind::Scan,
            endinput: false,
        });
    }

    /// `\ifcsname … \endcsname` (etex.web §1451): whether the name is
    /// defined; unlike `\csname`, it defines nothing.
    pub(super) fn if_csname(&mut self) -> bool {
        let mut name = String::new();
        loop {
            let Some((t, m)) = self.next_x_m(false) else {
                self.err("Missing \\endcsname inserted".into());
                break;
            };
            if let Tok::Char(_, c) = t {
                name.push(c);
            } else {
                if m != Meaning::Prim(Prim::EndCsName) {
                    self.err("Missing \\endcsname inserted".into());
                    self.back(t);
                }
                break;
            }
        }
        let t = self.cs(&name);
        let key = t.key().expect("a name");
        self.meaning(&key) != Meaning::Undefined
    }

    // ------------------------------------------------------------ internal quantities

    /// e-TeX's items of `last_item` (etex.web §1381–1408, §1510).
    pub(super) fn etex_item(&mut self, k: u8) -> IVal {
        match k {
            5 => IVal::Int(ETEX_VERSION),
            6 => IVal::Int(self.st.groups.len() as i32),
            7 => IVal::Int(self.group_type()),
            8 => IVal::Int(self.st.conds.len() as i32),
            9 => IVal::Int(match self.st.conds.last() {
                None => 0,
                Some(c) if c.code < 32 => i32::from(c.code) + 1,
                Some(c) => -(i32::from(c.code) - 32 + 1),
            }),
            10 => IVal::Int(match self.st.conds.last().map(|c| c.limit) {
                // (`or_code`, `else_code`: the then arm, a case; `fi_code`:
                // the else arm; the test being read, or no conditional: 0)
                Some(3 | 4) => 1,
                Some(2) => -1,
                _ => 0,
            }),
            11 => IVal::Int(self.last_node_type()),
            12 | 13 | 21 | 22 => {
                let g = self.scan_glue(false);
                match k {
                    12 => IVal::Int(i32::from(g.stretch_order)),
                    13 => IVal::Int(i32::from(g.shrink_order)),
                    21 => IVal::Dimen(g.stretch),
                    _ => IVal::Dimen(g.shrink),
                }
            }
            14..=17 => {
                // (etex.web §1402: a character in the font's range; one it
                // lacks has zero metrics)
                let f = self.scan_font_ident();
                let c = self.scan_char_num();
                IVal::Dimen(f.glyph(c).map_or(0, |g| match k {
                    14 => g.width,
                    15 => g.height,
                    16 => g.depth,
                    _ => g.italic,
                }))
            }
            18..=20 => {
                // (etex.web §1406: `mem[par_shape_ptr+2k-q]`)
                let mut q = i32::from(k - 18);
                let mut n = self.scan_int();
                let shape = match self.get(&Key::Parshape) {
                    Val::Parshape(s) => s,
                    _ => Arc::new(Vec::new()),
                };
                IVal::Dimen(if shape.is_empty() || n <= 0 {
                    0
                } else {
                    if q == 2 {
                        q = n % 2;
                        n = i32::midpoint(n, q);
                    }
                    let (indent, length) = shape[(n as usize).min(shape.len()) - 1];
                    if q == 1 { indent } else { length }
                })
            }
            23 => IVal::Glue(self.scan_glue(true)),
            24 => IVal::Mu(self.scan_glue(false)),
            25 => self.scan_expr(Lv::Int),
            26 => self.scan_expr(Lv::Dimen),
            27 => self.scan_expr(Lv::Glue),
            28 => self.scan_expr(Lv::Mu),
            _ => IVal::Int(0),
        }
    }

    /// `\currentgrouptype` (etex.web §1393, tex.web 269's group codes) of
    /// the innermost open group.
    fn group_type(&self) -> i32 {
        self.st.groups.last().map_or(0, Self::type_of)
    }

    /// A group's code (tex.web 269).
    fn type_of(g: &super::Group) -> i32 {
        match g.kind {
            GroupKind::Simple => 1,
            GroupKind::SemiSimple => 14,
            GroupKind::Output => 8,
            GroupKind::Box | GroupKind::Math => match g.boxg.as_deref().map(|b| (&b.kind, b.adjusted)) {
                Some((BoxKind::H, true)) => 3,
                Some((BoxKind::V, _)) => 4,
                Some((BoxKind::Vtop, _)) => 5,
                Some((BoxKind::Disc(..), _)) => 10,
                Some((BoxKind::Vcenter, _)) => 12,
                Some((BoxKind::Math(k), _)) => match **k {
                    MathKind::Sub(_) => 9,
                    MathKind::Choice(..) => 13,
                    MathKind::Left { .. } => 16,
                    // (`$`, `$$`, `\eqno`: `math_shift_group`)
                    _ => 15,
                },
                _ => 2,
            },
        }
    }

    /// `\lastnodetype` (etex.web §424): the type of the current list's last
    /// item plus one; 0 for a character, -1 for none (and with no mode).
    fn last_node_type(&mut self) -> i32 {
        if self.in_write {
            return -1;
        }
        match self.last_item() {
            None => -1,
            Some(it) => match it {
                Item::Char(..) => 0,
                Item::Box(b) => {
                    if b.vertical {
                        2
                    } else {
                        1
                    }
                }
                Item::Rule { .. } => 3,
                Item::Ins(_) => 4,
                Item::Mark(_) | Item::Marks(..) => 5,
                Item::Adjust(_) => 6,
                Item::Lig(_) => 7,
                Item::Disc(_) => 8,
                Item::Whatsit(_) | Item::Lang { .. } => 9,
                Item::Math { .. } => 10,
                Item::Glue { .. } | Item::Leaders(_) => 11,
                Item::Kern { .. } => 12,
                Item::Penalty(_) => 13,
                _ => -1,
            },
        }
    }

    /// The next token that is not a space, expanded (etex.web §1530:
    /// "get the next non-blank non-call token").
    fn nonblank_x(&mut self) -> Option<(Tok, Meaning)> {
        loop {
            let x = self.next_x_m(false)?;
            if !matches!(x.1, Meaning::Char(10, _)) {
                return Some(x);
            }
        }
    }

    /// An expression of level `level` (etex.web §1520–1536): factors and
    /// terms left to right, `*` and `/` before `+` and `-`, parentheses on a
    /// stack of their own; ends at a token that continues none, which is
    /// put back unless it is `\relax`. Any overflow (or a division by zero)
    /// makes it zero, after "Arithmetic overflow".
    #[allow(clippy::too_many_lines, reason = "etex.web's one procedure")]
    pub(super) fn scan_expr(&mut self, level: Lv) -> IVal {
        let zero = |l: Lv| if l >= Lv::Glue { EVal::G(Glue::default()) } else { EVal::N(0) };
        let mut l = level;
        let mut b = false;
        let mut stack: Vec<Frame> = Vec::new();
        let e = 'restart: loop {
            let (mut r, mut s, mut e, mut t, mut n) = (Op::None, Op::None, zero(l), zero(l), 0i32);
            'cont: loop {
                // (scan a factor `f` of level `o`, or begin a subexpression)
                let o = if s == Op::None { l } else { Lv::Int };
                let x = self.nonblank_x();
                if let Some((tk, _)) = &x
                    && tk.is_char(12, '(')
                {
                    stack.push(Frame { l, s, r, e, t, n });
                    l = o;
                    continue 'restart;
                }
                if let Some((tk, _)) = x {
                    self.back(tk);
                }
                let mut f = match o {
                    Lv::Int => EVal::N(self.scan_int()),
                    Lv::Dimen => EVal::N(self.scan_dimen()),
                    Lv::Glue => EVal::G(self.scan_glue(false)),
                    _ => EVal::G(self.scan_glue(true)),
                };
                loop {
                    // (found: the next operator)
                    let next = self.nonblank_x();
                    let mut op = match &next {
                        Some((Tok::Char(12, '+'), _)) => Op::Add,
                        Some((Tok::Char(12, '-'), _)) => Op::Sub,
                        Some((Tok::Char(12, '*'), _)) => Op::Mult,
                        Some((Tok::Char(12, '/'), _)) => Op::Div,
                        _ => Op::None,
                    };
                    if op == Op::None {
                        if stack.is_empty() {
                            if let Some((tk, m)) = next
                                && m != Meaning::Prim(Prim::Relax)
                            {
                                self.back(tk);
                            }
                        } else if !matches!(&next, Some((Tok::Char(12, ')'), _))) {
                            self.err("Missing ) inserted for expression".into());
                            if let Some((tk, _)) = next {
                                self.back(tk);
                            }
                        }
                    }
                    let mut err = b;
                    // (§1527: the factor in range)
                    if l == Lv::Int || s > Op::Sub {
                        if f.n() == i32::MIN {
                            err = true;
                            f = EVal::N(0);
                        }
                    } else if l == Lv::Dimen {
                        if f.n().unsigned_abs() > MAX_DIMEN as u32 {
                            err = true;
                            f = EVal::N(0);
                        }
                    } else {
                        let g = f.g();
                        if [g.width, g.stretch, g.shrink].iter().any(|c| c.unsigned_abs() > MAX_DIMEN as u32) {
                            err = true;
                            f = EVal::G(Glue::default());
                        }
                    }
                    // (§1528: the term so far)
                    match s {
                        Op::None => {
                            t = match f {
                                EVal::G(g) if op != Op::None => EVal::G(normalize(g)),
                                f => f,
                            };
                        }
                        Op::Mult if op == Op::Div => {
                            n = f.n();
                            op = Op::Scale;
                        }
                        Op::Mult => {
                            let f = f.n();
                            t = match (l, t) {
                                (Lv::Int, t) => EVal::N(mul(t.n(), f, INFINITY, &mut err)),
                                (Lv::Dimen, t) => EVal::N(nx_plus_y(t.n(), f, 0).unwrap_or_else(|| {
                                    err = true;
                                    0
                                })),
                                (_, t) => {
                                    let mut g = t.g();
                                    g.width = mul(g.width, f, MAX_DIMEN, &mut err);
                                    g.stretch = mul(g.stretch, f, MAX_DIMEN, &mut err);
                                    g.shrink = mul(g.shrink, f, MAX_DIMEN, &mut err);
                                    EVal::G(g)
                                }
                            };
                        }
                        Op::Div => {
                            let f = f.n();
                            t = match t {
                                EVal::N(t) => EVal::N(quotient(t, f, &mut err)),
                                EVal::G(mut g) => {
                                    g.width = quotient(g.width, f, &mut err);
                                    g.stretch = quotient(g.stretch, f, &mut err);
                                    g.shrink = quotient(g.shrink, f, &mut err);
                                    EVal::G(g)
                                }
                            };
                        }
                        Op::Scale => {
                            let f = f.n();
                            let max = if l == Lv::Int { INFINITY } else { MAX_DIMEN };
                            t = match t {
                                EVal::N(t) => EVal::N(fract(t, n, f, max, &mut err)),
                                EVal::G(mut g) => {
                                    g.width = fract(g.width, n, f, max, &mut err);
                                    g.stretch = fract(g.stretch, n, f, max, &mut err);
                                    g.shrink = fract(g.shrink, n, f, max, &mut err);
                                    EVal::G(g)
                                }
                            };
                        }
                        Op::Add | Op::Sub => {}
                    }
                    if op > Op::Sub {
                        s = op;
                    } else {
                        // (§1529: the expression so far)
                        s = Op::None;
                        let neg = r == Op::Sub;
                        e = if r == Op::None {
                            t
                        } else if l == Lv::Int {
                            EVal::N(add_or_sub(e.n(), t.n(), INFINITY, neg, &mut err))
                        } else if l == Lv::Dimen {
                            EVal::N(add_or_sub(e.n(), t.n(), MAX_DIMEN, neg, &mut err))
                        } else {
                            let (mut x, y) = (e.g(), t.g());
                            x.width = add_or_sub(x.width, y.width, MAX_DIMEN, neg, &mut err);
                            if x.stretch_order == y.stretch_order {
                                x.stretch = add_or_sub(x.stretch, y.stretch, MAX_DIMEN, neg, &mut err);
                            } else if x.stretch_order < y.stretch_order && y.stretch != 0 {
                                x.stretch = y.stretch;
                                x.stretch_order = y.stretch_order;
                            }
                            if x.shrink_order == y.shrink_order {
                                x.shrink = add_or_sub(x.shrink, y.shrink, MAX_DIMEN, neg, &mut err);
                            } else if x.shrink_order < y.shrink_order && y.shrink != 0 {
                                x.shrink = y.shrink;
                                x.shrink_order = y.shrink_order;
                            }
                            EVal::G(normalize(x))
                        };
                        r = op;
                    }
                    b = err;
                    if op != Op::None {
                        continue 'cont;
                    }
                    let Some(p) = stack.pop() else {
                        break 'restart e;
                    };
                    // (the subexpression is the factor of the one around it)
                    f = e;
                    (l, s, r, e, t, n) = (p.l, p.s, p.r, p.e, p.t, p.n);
                }
            }
        };
        let e = if b {
            self.err("Arithmetic overflow".into());
            zero(l)
        } else {
            e
        };
        match (l, e) {
            (Lv::Int, e) => IVal::Int(e.n()),
            (Lv::Dimen, e) => IVal::Dimen(e.n()),
            (Lv::Glue, e) => IVal::Glue(e.g()),
            (_, e) => IVal::Mu(e.g()),
        }
    }

    // ------------------------------------------------------------ commands

    /// e-TeX's commands that are neither assignments nor expandable.
    pub(super) fn etex_command(&mut self, p: Prim) {
        let t = self.cmd.clone();
        match p {
            Prim::Marks => {
                // (etex.web §1500: `\marks n {…}`; class 0 is `\mark`'s)
                let n = self.scan_eight_bit_int();
                let toks = Arc::new(self.scan_toks_x());
                self.emit_here(if n == 0 { Item::Mark(toks) } else { Item::Marks(n, toks) });
            }
            Prim::Discards(_) => {
                // (etex.web §1514: the discards saved by the page builder or
                // `\vsplit`, which keep none while `\savingvdiscards` is not
                // positive — and the page fold keeps none yet: CORE.md)
                match self.st.mode {
                    Mode::H => self.head_for_vmode(&t, false),
                    Mode::M => self.insert_dollar_sign(&t),
                    Mode::V => {}
                }
            }
            Prim::Lr(sub) => {
                // (etex.web §1434: TeXXeT's math nodes, if enabled)
                if self.st.mode == Mode::V {
                    self.back_new_graf(&t);
                } else if self.st.mode == Mode::M {
                    self.insert_dollar_sign(&t);
                } else if self.int_par(IntPar::TeXXeTstate) > 0 {
                    self.emit_here(Item::Math { w: 0, after: sub % 2 == 1, lr: sub & !1 });
                } else {
                    self.err(format!("Improper {t}"));
                }
            }
            Prim::ShowE(k) => self.show_etex(k),
            _ => {}
        }
    }

    /// `\batchmode` … `\errorstopmode`, `\interactionmode=n` (tex.web 1265:
    /// `new_interaction`): the log's line ends, the mode is defined (always
    /// globally: it is not in TeX's tables).
    pub(super) fn new_interaction(&mut self, k: i32) {
        self.log = Some(format!("{RAW}{LN}").into());
        self.define(Key::Interaction, Val::Int(k), true);
    }

    // ------------------------------------------------------------ penalties arrays

    /// `\interlinepenalties n p1 … pn` and friends (etex.web §1248): `n`
    /// values, none if `n` is not positive.
    pub(super) fn penalties_assign(&mut self, k: u8, global: bool) {
        self.optional_equals();
        let mut n = self.scan_int();
        // (TeX's memory holds at most a few million words)
        if n > 1 << 20 {
            self.err("TeX capacity exceeded, sorry [main memory size]".into());
            n = 0;
        }
        let mut v = Vec::with_capacity(usize::try_from(n).unwrap_or(0).min(1024));
        for _ in 0..n.max(0) {
            v.push(self.scan_int());
        }
        self.define(Key::PenaltiesArray(k), Val::Penalties(Arc::from(v)), global);
    }

    /// A penalties array (empty: TeX's null).
    pub(super) fn penalties(&mut self, k: u8) -> Arc<[i32]> {
        match self.get(&Key::PenaltiesArray(k)) {
            Val::Penalties(p) => p,
            _ => Arc::from([]),
        }
    }

    /// `\the\interlinepenalties n` (etex.web's "Fetch a penalties array
    /// element"): element `n` (the last past the end), the count for 0, 0
    /// for a negative `n` or no array.
    pub(super) fn penalties_item(&mut self, k: u8) -> i32 {
        let n = self.scan_int();
        let p = self.penalties(k);
        if p.is_empty() || n < 0 {
            0
        } else if n == 0 {
            p.len() as i32
        } else {
            p[(n as usize).min(p.len()) - 1]
        }
    }

    // ------------------------------------------------------------ \showgroups, \showifs, \showtokens

    /// The error context of the command just read (tex.web 311–318, as
    /// `\showbox`'s: `show_context`, laid out by the log's gatherer once
    /// the line number is known), then the end of a help-less error in
    /// batch mode (two `print_ln`s).
    fn file_context(&mut self) -> String {
        let mut s = String::new();
        self.show_context(&mut s);
        s.push(LN);
        s.push(LN);
        s
    }

    /// Where the `k`-th open group (`group`) or conditional began, as a
    /// log placeholder: the line of the node that opened it, found through
    /// this node's state when the log is laid out (`graph::opener`).
    fn entry_line(group: bool, k: usize) -> String {
        format!("{LINE}{}{k}{LINE}", if group { 'E' } else { 'C' })
    }

    /// `\showgroups` (4), `\showtokens` (5), `\showifs` (6): etex.web's
    /// cases of `show_whatever`, in batch mode: the display, then `! OK.`
    /// (but for `\showtokens`) and the error's context, in the log.
    fn show_etex(&mut self, k: u8) {
        let mut s = String::new();
        s.push(RAW);
        match k {
            5 => {
                // (`the_toks` with `show_tokens`: a general text, shown)
                let toks = self.scan_toks_raw();
                let mut t = String::new();
                self.show_toks(&mut t, &toks);
                s.push(NL);
                s.push_str("> ");
                s.push_str(&t);
                s.push('.');
            }
            4 => {
                s.push(NL);
                s.push(LN);
                self.show_save_groups(&mut s);
                s.extend([NL, LN, NL]);
                s.push_str("! OK.");
            }
            _ => {
                s.push(NL);
                s.push(LN);
                self.show_ifs(&mut s);
                s.extend([NL, LN, NL]);
                s.push_str("! OK.");
            }
        }
        let cx = self.file_context();
        s.push_str(&cx);
        self.log_raw(&s);
    }

    /// etex.web's `show_save_groups`: each open group, innermost first,
    /// with where it was entered and what began it.
    fn show_save_groups(&mut self, s: &mut String) {
        let groups: Vec<super::Group> = self.st.groups.0.as_deref().cloned().unwrap_or_default();
        for (i, g) in groups.iter().enumerate().rev() {
            let level = i + 1;
            let ty = Self::type_of(g);
            let name = match ty {
                1 => "simple",
                14 => "semi simple",
                2 => "hbox",
                3 => "adjusted hbox",
                4 => "vbox",
                5 => "vtop",
                10 => "disc",
                12 => "vcenter",
                9 => "math",
                13 => "math choice",
                15 => "math shift",
                _ => "math left",
            };
            s.push(NL);
            let _ = write!(s, "### {name} group (level {level}) entered at line {} (", Self::entry_line(true, i));
            let bg = g.boxg.as_deref();
            let mut out = String::new();
            match ty {
                1 | 9 => out.push('{'),
                14 => self.print_esc(&mut out, "begingroup"),
                2..=5 | 12 => {
                    if let Some(b) = bg {
                        if ty != 12 {
                            // (the box context)
                            match b.context {
                                BoxContext::Shift(0) => {}
                                BoxContext::Shift(d) => {
                                    let cmd = match (b.mode == Mode::V, d > 0) {
                                        (true, true) => "moveright",
                                        (true, false) => "moveleft",
                                        (false, true) => "lower",
                                        (false, false) => "raise",
                                    };
                                    self.print_esc(&mut out, cmd);
                                    print::scaled(&mut out, d.abs());
                                    out.push_str("pt");
                                }
                                BoxContext::Set(n, global) => {
                                    if global {
                                        self.print_esc(&mut out, "global");
                                    }
                                    self.print_esc(&mut out, "setbox");
                                    let _ = write!(out, "{n}=");
                                }
                                BoxContext::Ship => self.print_esc(&mut out, "shipout"),
                                // (etex.web §1411: `\leaders`, `\cleaders`, `\xleaders`)
                                BoxContext::Leaders(k) => self.print_esc(
                                    &mut out,
                                    ["leaders", "cleaders", "xleaders"][usize::from(k.saturating_sub(100)).min(2)],
                                ),
                            }
                        }
                        let box_name = match ty {
                            4 => "vbox",
                            5 => "vtop",
                            12 => "vcenter",
                            _ => "hbox",
                        };
                        self.print_esc(&mut out, box_name);
                        // (the box's packaging)
                        let (word, d) = match b.spec {
                            Spec::Exactly(w) => ("to", w),
                            Spec::Additional(w) => ("spread", w),
                        };
                        if d != 0 {
                            out.push(' ');
                            out.push_str(word);
                            print::scaled(&mut out, d);
                            out.push_str("pt");
                        }
                    }
                    out.push('{');
                }
                10 | 13 => {
                    let done = match bg.map(|b| &b.kind) {
                        Some(BoxKind::Disc(k, _)) => usize::from(*k),
                        Some(BoxKind::Math(m)) => match &**m {
                            MathKind::Choice(k, _) => usize::from(*k),
                            _ => 0,
                        },
                        _ => 0,
                    };
                    self.print_esc(&mut out, if ty == 10 { "discretionary" } else { "mathchoice" });
                    for _ in 0..done.min(3) {
                        out.push_str("{}");
                    }
                    out.push('{');
                }
                15 => match bg.map(|b| &b.kind) {
                    Some(BoxKind::Math(m)) => match &**m {
                        MathKind::EqNo { left } => self.print_esc(&mut out, if *left { "leqno" } else { "eqno" }),
                        MathKind::Inline => out.push('$'),
                        _ => out.push_str("$$"),
                    },
                    _ => out.push('$'),
                },
                _ => {
                    let middle = matches!(bg.map(|b| &b.kind), Some(BoxKind::Math(m)) if matches!(**m, MathKind::Left { middle: true }));
                    self.print_esc(&mut out, if middle { "middle" } else { "left" });
                }
            }
            s.push_str(&out);
            s.push(')');
        }
        s.push(NL);
        s.push_str("### bottom level");
    }

    /// etex.web's `show_ifs`: each open conditional, innermost first.
    fn show_ifs(&mut self, s: &mut String) {
        const NAMES: [&str; 20] = [
            "if", "ifcat", "ifnum", "ifdim", "ifodd", "ifvmode", "ifhmode", "ifmmode", "ifinner", "ifvoid", "ifhbox",
            "ifvbox", "ifx", "ifeof", "iftrue", "iffalse", "ifcase", "ifdefined", "ifcsname", "iffontchar",
        ];
        let conds: Vec<super::Cond> = self.st.conds.0.as_deref().cloned().unwrap_or_default();
        if conds.is_empty() {
            s.push(NL);
            s.push_str("### no active conditionals");
            return;
        }
        for (i, c) in conds.iter().enumerate().rev() {
            let mut out = String::new();
            if c.code >= 32 {
                self.print_esc(&mut out, "unless");
            }
            self.print_esc(&mut out, NAMES[usize::from(c.code % 32).min(19)]);
            if c.limit == 2 {
                self.print_esc(&mut out, "else");
            }
            s.push(NL);
            let _ = write!(s, "### level {}: {out} entered on line {}", i + 1, Self::entry_line(false, i));
        }
    }

    /// `\interactionmode=n` (etex.web §1428).
    pub(super) fn interaction_mode_assign(&mut self) {
        self.optional_equals();
        let v = self.scan_int();
        if (0..=3).contains(&v) {
            self.new_interaction(v);
        } else {
            self.err(format!("Bad interaction mode ({v})"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_rounds_as_etex_does() {
        let mut e = false;
        assert_eq!(quotient(7, 2, &mut e), 4);
        assert_eq!(quotient(-7, 2, &mut e), -4);
        assert_eq!(quotient(5, 3, &mut e), 2);
        assert_eq!(quotient(4, 3, &mut e), 1);
        assert_eq!(fract(7, 3, 2, INFINITY, &mut e), 11);
        assert_eq!(fract(INFINITY, 2, 2, INFINITY, &mut e), INFINITY);
        assert!(!e);
        let _ = quotient(1, 0, &mut e);
        assert!(e);
        let mut e = false;
        let _ = add_or_sub(INFINITY, 1, INFINITY, false, &mut e);
        assert!(e);
    }

    #[test]
    fn readline_makes_other_characters() {
        let t = readline_toks("a b  ", Some('\r'));
        assert_eq!(t, vec![Tok::Char(12, 'a'), Tok::Char(10, ' '), Tok::Char(12, 'b'), Tok::Char(12, '\r')]);
    }
}
