//! A toy TeX-like language over the φ core (not TeX): the acceptance
//! client of DESIGN 7.13.
//!
//! Tokens are words, control sequences, `{` and `}`. The document is an
//! unfold over them, a step per command:
//! - `\def\x{…}` / `\gdef\x{…}` define macros; `\x` rewrites the input;
//! - `\defname{w}{…}` and `\use{w}` make and use names spelled at run time;
//! - `{` and `}` are groups (local definitions end at the close);
//! - `\step` advances `\count` (a leaf per step, chained through the
//!   name), `\the\count` prints it;
//! - `\ifzero\x A \else B \fi`: the test is a node, only the taken arm is
//!   read, and at the join each name the arm defined gets a φ;
//! - words fill a paragraph (`\par`), broken by a DP scan into lines,
//!   which go on the `VLIST` chain; a page scan reads that chain;
//! - `\hbox{…}` is a nested unfold;
//! - `\write{w}` and `\message{w}` are effects on two chains, `\barrier`
//!   a barrier;
//! - `\label{k}` publishes `\count` to slot `k`, `\ref{k}` reads it (the
//!   next run's value, predicted).

#![allow(dead_code, clippy::pedantic)]

use std::sync::Arc;

use phi::{
    Arg, Args, Chain, Class, ElemId, Fam, Graph, Lang, NameId, NodeId, Sel, Seq, Slot, Step,
    StepCx, Value, Ver,
};

pub const OUT: Chain = Chain(1);
pub const LOG: Chain = Chain(2);
pub const VLIST: Chain = Chain(3);
pub const TOC: Fam = Fam(1);
/// The line width and the page height.
pub const WIDTH: i64 = 24;
pub const HEIGHT: i64 = 5;
/// The line breaker's lookback, in words.
pub const K: usize = 6;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Tok {
    Word(Arc<str>),
    Cs(Arc<str>),
    Open,
    Close,
}

impl Tok {
    pub fn text(&self) -> String {
        match self {
            Tok::Word(w) => w.to_string(),
            Tok::Cs(c) => format!("\\{c}"),
            Tok::Open => "{".into(),
            Tok::Close => "}".into(),
        }
    }
}

/// The step state: the pending input and the open conditionals (what
/// has no name; DESIGN 7.4's rule).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct St {
    pub pending: Vec<Tok>,
    /// Per open conditional: the names its arm defined, and whether
    /// globally.
    pub conds: Vec<Vec<(String, bool)>>,
    /// In a box: its width so far.
    pub width: i64,
    /// Macro expansions since the input last advanced (a bound against
    /// recursive macros: past it a macro expands to nothing).
    pub exp: u32,
}

#[derive(Clone, Debug, Default)]
pub enum TV {
    #[default]
    Unit,
    Int(i64),
    Tok(Tok),
    Toks(Arc<Vec<Tok>>),
    Seq(Seq<TV>),
    Word(Arc<str>, i64),
    St(Arc<St>, Ver),
    /// A record with fields (its version from theirs).
    Rec(Arc<Vec<TV>>, Ver),
    /// A line breaker's window: the last K words' widths and best costs.
    Dp(Arc<(Vec<i64>, Vec<i64>)>),
    /// Line widths of a paragraph.
    Lines(Arc<Vec<i64>>),
}

impl TV {
    pub fn st(s: St) -> TV {
        let v = Ver::of(&format!(
            "{:?}|{:?}|{}|{}",
            s.pending, s.conds, s.width, s.exp
        ));
        TV::St(Arc::new(s), v)
    }
    pub fn rec(xs: Vec<TV>) -> TV {
        let vs: Vec<Ver> = xs.iter().map(Value::ver).collect();
        TV::Rec(Arc::new(xs), Ver::node(0x0072_6563, &vs))
    }
    pub fn int(&self) -> i64 {
        match self {
            TV::Int(i) => *i,
            TV::Word(_, w) => *w,
            _ => 0,
        }
    }
}

impl Value for TV {
    fn ver(&self) -> Ver {
        match self {
            TV::Unit => Ver::of(&0u8),
            TV::Int(i) => Ver::of(&(1u8, *i)),
            TV::Tok(t) => Ver::of(&(2u8, t)),
            TV::Toks(ts) => Ver::of(&(3u8, &**ts)),
            TV::Seq(s) => Ver::node(4, &[s.ver()]),
            TV::Word(w, n) => Ver::of(&(5u8, &**w, *n)),
            TV::St(_, v) => *v,
            TV::Rec(_, v) => *v,
            TV::Dp(d) => Ver::of(&(7u8, &d.0, &d.1)),
            TV::Lines(l) => Ver::of(&(8u8, &**l)),
        }
    }
    fn field(&self, f: u32) -> Option<TV> {
        match self {
            TV::Rec(xs, _) => xs.get(f as usize).cloned(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Op {
    #[default]
    Nop,
    /// The document (an unfold).
    Doc,
    /// `\hbox{…}` (a nested unfold).
    Box,
    /// A word as a box (from a box's width).
    BoxWord,
    /// Append operand 1 to the sequence operand 0, with this identity.
    Push(u64),
    Add1,
    /// The value as a word.
    Show,
    /// Whether operand 0 is zero (or nothing).
    IsZero,
    /// φ(test, taken, other): the taken side.
    Phi,
    /// Operand 0 (an effect's payload, a definition's copy).
    Id,
    /// The line breaker (a scan).
    Break,
    /// The page builder (a scan).
    Page,
    /// Sum of operands (tests).
    Sum,
    /// Operand's field 1 times 2 (tests).
    Double,
    /// A record of the operands.
    Pair,
    /// A table of contents line per entry (a scan).
    TocLine,
    /// A page label: (the page, its digits' count).
    PageRec,
    /// Operand 0, counted (`USEW`): what reads a width.
    UseW,
}

thread_local! {
    /// `Op::UseW` evaluations on this thread.
    pub static USEW: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub struct Toy;

fn dp_init() -> TV {
    TV::Dp(Arc::new((Vec::new(), Vec::new())))
}

impl Lang for Toy {
    type Val = TV;
    type Op = Op;

    fn eval(op: Op, a: &Args<'_, Self>) -> TV {
        match op {
            Op::Push(id) => {
                let mut s = match &*a.get(0) {
                    TV::Seq(s) => s.clone(),
                    _ => Seq::new(),
                };
                s.push(ElemId(id), a.get(1).clone());
                TV::Seq(s)
            }
            Op::Add1 => TV::Int(a.get(0).int() + 1),
            Op::Show => {
                let t = match &*a.get(0) {
                    TV::Unit => "?".to_string(),
                    v => v.int().to_string(),
                };
                let n = t.len() as i64;
                TV::Word(t.into(), n)
            }
            Op::BoxWord => TV::Word("[box]".into(), a.get(0).int()),
            Op::IsZero => TV::Int(i64::from(a.get(0).int() == 0)),
            Op::Phi | Op::Id => {
                if op == Op::Phi {
                    a.get(1).clone()
                } else {
                    a.get(0).clone()
                }
            }
            Op::Sum => TV::Int((0..a.len()).map(|i| a.get(i).int()).sum()),
            Op::Double => TV::Int(a.get(0).int() * 2),
            Op::Nop => TV::Unit,
            Op::Pair => TV::rec((0..a.len()).map(|i| a.get(i).clone()).collect()),
            Op::PageRec => {
                let p = a.get(0).int();
                TV::rec(vec![TV::Int(p), TV::Int(p.to_string().len() as i64)])
            }
            Op::UseW => {
                USEW.with(|c| c.set(c.get() + 1));
                a.get(0).clone()
            }
            _ => unreachable!("{op:?}"),
        }
    }

    fn step(op: Op, st: &TV, _args: &Args<'_, Self>, cx: &mut StepCx<'_, Self>) -> Step<TV> {
        let st = match st {
            TV::St(s, _) => (**s).clone(),
            _ => St::default(),
        };
        step(op, st, cx)
    }

    fn scan(op: Op, st: &TV, x: &TV, a: &Args<'_, Self>) -> (TV, TV) {
        match op {
            Op::Break => break_step(st, x, a.get(0).int()),
            Op::TocLine => {
                // (a line from the entry; the state is nothing, so a changed
                // entry changes its line only)
                let line = format!("{} {:?}", x.field(0).map_or(0, |n| n.int()), x.field(1));
                let n = line.len() as i64;
                (TV::Unit, TV::Word(line.into(), n))
            }
            Op::Page => {
                // (state: lines on the current page; output: breaks inside)
                let h = a.get(0).int();
                let mut used = st.int();
                let n = match x {
                    TV::Lines(l) => l.len() as i64,
                    _ => 0,
                };
                let mut breaks = 0;
                for _ in 0..n {
                    if used == h {
                        used = 0;
                        breaks += 1;
                    }
                    used += 1;
                }
                (TV::Int(used), TV::Int(breaks))
            }
            _ => unreachable!("{op:?}"),
        }
    }

    fn scan_result(op: Op, st: &TV, outs: &Seq<TV>, _a: &Args<'_, Self>) -> TV {
        match op {
            Op::Break => traceback(outs),
            Op::TocLine => TV::Int(outs.iter().map(|(_, o)| o.int()).sum()),
            Op::Page => {
                let breaks: i64 = outs.iter().map(|(_, o)| o.int()).sum();
                TV::Int(if st.int() == 0 && breaks == 0 {
                    0
                } else {
                    breaks + 1
                })
            }
            _ => st.clone(),
        }
    }

    fn as_seq(v: &TV) -> Option<&Seq<TV>> {
        match v {
            TV::Seq(s) => Some(s),
            _ => None,
        }
    }

    fn chain_val(items: Seq<TV>) -> TV {
        TV::Seq(items)
    }

    fn entries(op: Op, input: &TV, _a: &Args<'_, Self>) -> Vec<phi::Entry<TV>> {
        // (after each `\par` of the document: a paragraph starts with
        // nothing pending)
        let (Op::Doc, TV::Seq(s)) = (op, input) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, (_, t)) in s.iter().enumerate() {
            if let TV::Tok(Tok::Cs(c)) = t
                && &**c == "par"
                && let Some((id, _)) = s.get(i + 1)
            {
                let key = phi::ver::hash64(&(id.0, &Vec::<Tok>::new()));
                out.push(phi::Entry {
                    at: i + 1,
                    key,
                    guess: Some(TV::st(St::default())),
                });
            }
        }
        out
    }

    fn fmt_op(op: Op) -> String {
        match op {
            Op::Push(_) => "Push".into(),
            op => format!("{op:?}"),
        }
    }

    fn parse_val(s: &str) -> Option<TV> {
        if s == "Unit" {
            return Some(TV::Unit);
        }
        if let Some(n) = s.strip_prefix("Int(").and_then(|r| r.strip_suffix(')')) {
            return n.parse().ok().map(TV::Int);
        }
        let r = s.strip_prefix("Word(\"")?.strip_suffix(')')?;
        let (w, n) = r.rsplit_once("\", ")?;
        if w.contains(['\\', '"']) {
            return None;
        }
        Some(TV::Word(w.into(), n.parse().ok()?))
    }

    fn op_tag(op: Op) -> u64 {
        phi::ver::hash64(&format!("{op:?}"))
    }
}

/// One word through the line breaker: the window of the last K words'
/// widths and the best cost of a break after each, relative to their
/// least (so equal windows mean equal decisions from here on).
fn break_step(st: &TV, x: &TV, width: i64) -> (TV, TV) {
    let (ws, cs) = match st {
        TV::Dp(d) => (d.0.clone(), d.1.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    let w = x.int();
    let mut ws = ws;
    ws.push(w);
    // best[i]: cost of a break after word i of the window (before the
    // window's first word: 0)
    let n = ws.len();
    let mut best = i64::MAX;
    let mut back = 1;
    let mut len = -1;
    for k in 1..=n.min(K) {
        len += ws[n - k] + 1;
        if len > width && k > 1 {
            break;
        }
        let before = if n - k == 0 {
            0
        } else {
            cs.get(n - k - 1).copied().unwrap_or(0)
        };
        let slack = (width - len).max(0);
        let c = before.saturating_add(slack * slack);
        if c < best {
            best = c;
            back = k;
        }
    }
    let mut cs = cs;
    cs.push(best);
    if ws.len() > K {
        ws.remove(0);
        cs.remove(0);
    }
    let m = *cs.iter().min().unwrap_or(&0);
    let cs: Vec<i64> = cs.iter().map(|c| c - m).collect();
    (TV::Dp(Arc::new((ws, cs))), TV::Int(back as i64))
}

/// The lines, from each word's best back step, from the end.
fn traceback(outs: &Seq<TV>) -> TV {
    let mut lines = Vec::new();
    let mut i = outs.len();
    let words: Vec<i64> = outs.iter().map(|(_, o)| o.int()).collect();
    while i > 0 {
        let k = words[i - 1].max(1) as usize;
        lines.push(k as i64);
        i = i.saturating_sub(k);
    }
    lines.reverse();
    TV::Lines(Arc::new(lines))
}

/// The next token: pending first, then the input.
struct Rd<'a, 's> {
    pending: Vec<Tok>,
    at: usize,
    cx: &'a mut StepCx<'s, Toy>,
}

impl Rd<'_, '_> {
    fn next(&mut self) -> Option<Tok> {
        if self.at < self.pending.len() {
            self.at += 1;
            return Some(self.pending[self.at - 1].clone());
        }
        match self.cx.next()? {
            TV::Tok(t) => Some(t.clone()),
            _ => None,
        }
    }
    fn peek(&mut self) -> Option<Tok> {
        if self.at < self.pending.len() {
            return Some(self.pending[self.at].clone());
        }
        match self.cx.peek(0)? {
            TV::Tok(t) => Some(t.clone()),
            _ => None,
        }
    }
    /// A balanced `{…}` group's tokens (without the braces), or one token.
    fn group(&mut self) -> Vec<Tok> {
        match self.next() {
            Some(Tok::Open) => {
                let mut depth = 1;
                let mut out = Vec::new();
                while let Some(t) = self.next() {
                    match t {
                        Tok::Open => depth += 1,
                        Tok::Close => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    out.push(t);
                }
                out
            }
            Some(t) => vec![t],
            None => Vec::new(),
        }
    }
    fn word(&mut self) -> String {
        self.group()
            .iter()
            .map(Tok::text)
            .collect::<Vec<_>>()
            .join(" ")
    }
    /// The arms of a conditional: up to `\else` and `\fi` at this level.
    fn arms(&mut self) -> (Vec<Tok>, Vec<Tok>) {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut in_else = false;
        let mut depth = 0;
        while let Some(t) = self.next() {
            if let Tok::Cs(c) = &t {
                match &**c {
                    "ifzero" => depth += 1,
                    "fi" if depth == 0 => break,
                    "fi" => depth -= 1,
                    "else" if depth == 0 => {
                        in_else = true;
                        continue;
                    }
                    _ => {}
                }
            }
            if in_else { b.push(t) } else { a.push(t) }
        }
        (a, b)
    }
    fn rest(&self) -> Vec<Tok> {
        self.pending[self.at..].to_vec()
    }
}

fn step(op: Op, mut st: St, cx: &mut StepCx<'_, Toy>) -> Step<TV> {
    let mut rd = Rd {
        pending: std::mem::take(&mut st.pending),
        at: 0,
        cx,
    };
    let Some(t) = rd.next() else {
        return if op == Op::Box {
            Step::Done(TV::Int(st.width))
        } else {
            Step::Done(TV::Unit)
        };
    };
    let par = rd.cx.name(b"par@");
    let count = rd.cx.name(b"count");
    // a word into the paragraph (or the box)
    fn word<'s>(op: Op, par: NameId, rd: &mut Rd<'_, 's>, w: Arg<'s>, id: u64) {
        if op == Op::Box {
            return;
        }
        let p = rd.cx.leaf(Op::Push(id), Class::Pure, &[Arg::Name(par), w]);
        rd.cx.define(par, Arg::Local(p), true);
    }
    let here = rd.cx.cursor().0 ^ (rd.at as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    match t {
        Tok::Word(w) => {
            if op == Op::Box {
                st.width += w.len() as i64 + 1;
            } else {
                let n = w.len() as i64;
                let l = rd.cx.lit(TV::Word(w, n));
                word(op, par, &mut rd, Arg::Local(l), here);
            }
        }
        Tok::Open => rd.cx.open_group(),
        Tok::Close => rd.cx.close_group(),
        Tok::Cs(c) => match &*c {
            "par" => {
                if op == Op::Doc {
                    let init = rd.cx.lit(dp_init());
                    let w = rd.cx.lit(TV::Int(WIDTH));
                    let lines = rd.cx.scan(
                        Op::Break,
                        Arg::Name(par),
                        Arg::Local(init),
                        &[Arg::Local(w)],
                    );
                    rd.cx
                        .leaf(Op::Id, Class::Effect(VLIST), &[Arg::Local(lines)]);
                    let e = rd.cx.lit(TV::Seq(Seq::new()));
                    rd.cx.define(par, Arg::Local(e), true);
                }
            }
            "def" | "gdef" => {
                let name = match rd.next() {
                    Some(Tok::Cs(n)) => n,
                    _ => "bad".into(),
                };
                let body = rd.group();
                let m = rd.cx.name(name.as_bytes());
                let l = rd.cx.lit(TV::Toks(Arc::new(body)));
                let global = &*c == "gdef";
                rd.cx.define(m, Arg::Local(l), global);
                if let Some(f) = st.conds.last_mut() {
                    f.push((name.to_string(), global));
                }
            }
            "defname" => {
                let name = rd.word();
                let body = rd.group();
                let m = rd.cx.name(name.as_bytes());
                let l = rd.cx.lit(TV::Toks(Arc::new(body)));
                rd.cx.define(m, Arg::Local(l), false);
                if let Some(f) = st.conds.last_mut() {
                    f.push((name, false));
                }
            }
            "use" => {
                let name = rd.word();
                let m = rd.cx.name(name.as_bytes());
                expand(&mut rd, &mut st, m);
            }
            "step" => {
                let l = rd.cx.leaf(Op::Add1, Class::Pure, &[Arg::Name(count)]);
                rd.cx.define(count, Arg::Local(l), false);
                if let Some(f) = st.conds.last_mut() {
                    f.push(("count".into(), false));
                }
            }
            "the" => {
                let name = match rd.next() {
                    Some(Tok::Cs(n)) => n,
                    _ => "bad".into(),
                };
                let m = rd.cx.name(name.as_bytes());
                let s = rd.cx.leaf(Op::Show, Class::Pure, &[Arg::Name(m)]);
                if op == Op::Box {
                    // (a box reads names too: its width from the shown value)
                    st.width += 2;
                } else {
                    word(op, par, &mut rd, Arg::Local(s), here);
                }
            }
            "hbox" => {
                let body = rd.group();
                let items: Vec<(ElemId, TV)> = body
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (ElemId(i as u64 + 1), TV::Tok(t)))
                    .collect();
                let input = rd.cx.lit(TV::Seq(Seq::from_vec(items)));
                let init = rd.cx.lit(TV::st(St::default()));
                let b = rd
                    .cx
                    .unfold(Op::Box, Arg::Local(input), Arg::Local(init), &[]);
                let w = rd.cx.leaf(Op::BoxWord, Class::Pure, &[Arg::Local(b)]);
                word(op, par, &mut rd, Arg::Local(w), here);
            }
            "ifzero" => {
                let name = match rd.next() {
                    Some(Tok::Cs(n)) => n,
                    _ => "bad".into(),
                };
                let m = rd.cx.name(name.as_bytes());
                let test = rd.cx.leaf(Op::IsZero, Class::Pure, &[Arg::Name(m)]);
                let k = st.conds.len();
                let hidden = rd.cx.name(format!("if@{k}").as_bytes());
                rd.cx.define(hidden, Arg::Local(test), true);
                // (the step decides: it reads the name itself)
                let v = rd.cx.read(m).map(|v| v.int()).unwrap_or(0);
                let (a, b) = rd.arms();
                let (taken, other) = if v == 0 { (a, b) } else { (b, a) };
                let mut p = taken;
                p.push(Tok::Cs("fi@".into()));
                let other_l = rd.cx.lit(TV::Toks(Arc::new(other)));
                let _ = other_l;
                p.extend(rd.rest());
                rd.pending = p;
                rd.at = 0;
                st.conds.push(Vec::new());
            }
            "fi@" => {
                let k = st.conds.len().saturating_sub(1);
                let hidden = rd.cx.name(format!("if@{k}").as_bytes());
                let names = st.conds.pop().unwrap_or_default();
                let mut seen: Vec<String> = Vec::new();
                for (n, global) in names.iter().rev() {
                    if seen.contains(n) {
                        continue;
                    }
                    seen.push(n.clone());
                    let m = rd.cx.name(n.as_bytes());
                    let other = rd.cx.lit(TV::Unit);
                    let phi = rd.cx.leaf(
                        Op::Phi,
                        Class::Pure,
                        &[Arg::Name(hidden), Arg::Name(m), Arg::Local(other)],
                    );
                    rd.cx.define(m, Arg::Local(phi), *global);
                    if let Some(f) = st.conds.last_mut() {
                        f.push((n.clone(), *global));
                    }
                }
            }
            "else" | "fi" => {}
            "write" | "message" => {
                let w = rd.word();
                let n = w.len() as i64;
                let l = rd.cx.lit(TV::Word(w.into(), n));
                let ch = if &*c == "write" { OUT } else { LOG };
                rd.cx.leaf(Op::Id, Class::Effect(ch), &[Arg::Local(l)]);
            }
            "barrier" => {
                rd.cx.leaf(Op::Nop, Class::Barrier, &[]);
            }
            "label" => {
                let k = rd.word();
                rd.cx.leaf(
                    Op::Id,
                    Class::Publish(Slot(phi::ver::hash64(&k))),
                    &[Arg::Name(count)],
                );
            }
            "section" => {
                let title = rd.word();
                let sec = rd.cx.name(b"sec");
                let n = rd.cx.leaf(Op::Add1, Class::Pure, &[Arg::Name(sec)]);
                rd.cx.define(sec, Arg::Local(n), true);
                let w = title.len() as i64;
                let t = rd.cx.lit(TV::Word(title.clone().into(), w));
                rd.cx.leaf(
                    Op::Pair,
                    Class::Entry(TOC, Slot(phi::ver::hash64(&title))),
                    &[Arg::Local(n), Arg::Local(t)],
                );
                word(op, par, &mut rd, Arg::Local(t), here);
            }
            "toc" => {
                let f = rd.cx.family(TOC);
                let init = rd.cx.lit(TV::Unit);
                let s = rd
                    .cx
                    .scan(Op::TocLine, Arg::Local(f), Arg::Local(init), &[]);
                let w = rd.cx.leaf(Op::BoxWord, Class::Pure, &[Arg::Local(s)]);
                word(op, par, &mut rd, Arg::Local(w), here);
            }
            "pagelabel" => {
                // (\label with the page's width beside it)
                let k = rd.word();
                rd.cx.leaf(
                    Op::PageRec,
                    Class::Publish(Slot(phi::ver::hash64(&k))),
                    &[Arg::Name(count)],
                );
            }
            "pageref" => {
                // (shows the page; `pr` is the whole record)
                let k = rd.word();
                let c = rd.cx.cross(Slot(phi::ver::hash64(&k)));
                let s = rd.cx.leaf(Op::Show, Class::Pure, &[Arg::Field(c, 0)]);
                let pr = rd.cx.name(b"pr");
                rd.cx.define(pr, Arg::Local(c), true);
                word(op, par, &mut rd, Arg::Local(s), here);
            }
            "copypr" => {
                // (`pr2`: a copy of `pr`, the whole record)
                let pr = rd.cx.name(b"pr");
                let x = rd.cx.leaf(Op::Id, Class::Pure, &[Arg::Name(pr)]);
                let pr2 = rd.cx.name(b"pr2");
                rd.cx.define(pr2, Arg::Local(x), true);
            }
            "usew" => {
                // (reads the width only: field 1 of `pr2`)
                let pr2 = rd.cx.name(b"pr2");
                let y = rd.cx.leaf(Op::UseW, Class::Pure, &[Arg::NameField(pr2, 1)]);
                let w2 = rd.cx.name(b"w2");
                rd.cx.define(w2, Arg::Local(y), true);
            }
            "ref" => {
                let k = rd.word();
                let c = rd.cx.cross(Slot(phi::ver::hash64(&k)));
                let s = rd.cx.leaf(Op::Show, Class::Pure, &[Arg::Local(c)]);
                word(op, par, &mut rd, Arg::Local(s), here);
            }
            _ => {
                let m = rd.cx.name(c.as_bytes());
                expand(&mut rd, &mut st, m);
            }
        },
    }
    if rd.cx.consumed() > 0 {
        st.exp = 0;
    }
    let rest = rd.rest();
    let key = {
        let cur = rd.cx.cursor().0;
        phi::ver::hash64(&(cur, &rest))
    };
    st.pending = rest;
    Step::Next {
        st: TV::st(st),
        key,
    }
}

/// A macro call: its body before the rest of the input.
fn expand(rd: &mut Rd<'_, '_>, st: &mut St, m: NameId) {
    if let Some(v) = rd.cx.read(m)
        && let TV::Toks(body) = &*v
        && st.exp < 64
    {
        st.exp += 1;
        let mut p: Vec<Tok> = (**body).clone();
        p.extend(rd.rest());
        rd.pending = p;
        rd.at = 0;
    }
}

/// A document: its tokens with identities, and the graph built over it.
pub struct Doc {
    pub toks: Vec<(ElemId, Tok)>,
    pub g: Graph<Toy>,
    pub input: NodeId,
    pub doc: NodeId,
    pub pages: NodeId,
}

pub fn seq_of(toks: &[(ElemId, Tok)]) -> Seq<TV> {
    Seq::from_vec(toks.iter().map(|(i, t)| (*i, TV::Tok(t.clone()))).collect())
}

impl Doc {
    pub fn new(toks: Vec<(ElemId, Tok)>) -> Doc {
        Self::with(toks, Graph::new())
    }

    pub fn with(toks: Vec<(ElemId, Tok)>, mut g: Graph<Toy>) -> Doc {
        let input = g.input(TV::Seq(seq_of(&toks)));
        let init = g.input(TV::st(St::default()));
        let doc = g.unfold(Op::Doc, input, init, &[]);
        let vl = g.chain_read(VLIST);
        let h = g.input(TV::Int(HEIGHT));
        let z = g.input(TV::Int(0));
        let pages = g.scan(Op::Page, (vl, Sel::WHOLE), z, &[h]);
        Doc {
            toks,
            g,
            input,
            doc,
            pages,
        }
    }

    /// Replace `del` tokens at `at` with `ins`, identities between the
    /// neighbours' (all renumbered when there is no room).
    pub fn splice(&mut self, at: usize, del: usize, ins: Vec<Tok>) {
        let lo = if at == 0 { 0 } else { self.toks[at - 1].0.0 };
        let hi = self.toks.get(at + del).map_or(u64::MAX / 2, |t| t.0.0);
        let room = hi.saturating_sub(lo) / (ins.len() as u64 + 1);
        let new: Vec<(ElemId, Tok)> = if room > 0 {
            ins.into_iter()
                .enumerate()
                .map(|(k, t)| (ElemId(lo + room * (k as u64 + 1)), t))
                .collect()
        } else {
            ins.into_iter().map(|t| (ElemId(0), t)).collect()
        };
        self.toks.splice(at..at + del, new);
        if room == 0 {
            for (k, t) in self.toks.iter_mut().enumerate() {
                t.0 = ElemId((k as u64 + 1) << 20);
            }
        }
        self.g.set(self.input, TV::Seq(seq_of(&self.toks)));
    }

    /// What a reader of the document sees.
    pub fn observe(&self) -> String {
        let g = &self.g;
        let mut s = String::new();
        s += &format!("doc={:?}\n", g.value(self.doc).ver());
        s += &format!("pages={}\n", g.value(self.pages).int());
        for (name, c) in [("out", OUT), ("log", LOG), ("vlist", VLIST)] {
            let items: Vec<String> = g
                .chain(c)
                .iter()
                .map(|v| match v {
                    TV::Word(w, _) => w.to_string(),
                    TV::Lines(l) => format!("{l:?}"),
                    v => format!("{v:?}"),
                })
                .collect();
            s += &format!("{name}={items:?}\n");
        }
        for n in ["count", "a", "b", "x", "y", "par@"] {
            let v = g
                .name_id(n.as_bytes())
                .and_then(|id| g.name_value(id))
                .map(|v| v.ver());
            s += &format!("{n}={v:?}\n");
        }
        s
    }
}

/// Tokens from text: words, `\cs`, `{`, `}` (blank-separated).
pub fn lex(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for w in src.split_whitespace() {
        let mut rest = w;
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix('{') {
                out.push(Tok::Open);
                rest = r;
            } else if let Some(r) = rest.strip_prefix('}') {
                out.push(Tok::Close);
                rest = r;
            } else if let Some(r) = rest.strip_prefix('\\') {
                let n = r
                    .find(|c: char| !c.is_alphanumeric() && c != '@')
                    .unwrap_or(r.len())
                    .max(1);
                out.push(Tok::Cs(r[..n].into()));
                rest = &r[n..];
            } else {
                let n = rest.find(['{', '}', '\\']).unwrap_or(rest.len());
                out.push(Tok::Word(rest[..n].into()));
                rest = &rest[n..];
            }
        }
    }
    out
}

pub fn ids(toks: Vec<Tok>) -> Vec<(ElemId, Tok)> {
    toks.into_iter()
        .enumerate()
        .map(|(k, t)| (ElemId((k as u64 + 1) << 20), t))
        .collect()
}
