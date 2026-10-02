//! A small language that exercises everything TeX will (`DESIGN.md`
//! §7.17.10, step 1), and its oracle.
//!
//! The source is a persistent sequence of lines; `main` calls the
//! tokenizer per line (bytes → words; `%` starts a comment and runs of
//! spaces collapse), gathers a paragraph's words up to a blank line and
//! calls `para` over them. A paragraph's words are text, or commands:
//!
//! | command | does |
//! |---|---|
//! | `set X N`, `add X Y` | write a variable (a state slot); `Y` a number or a variable |
//! | `print X` | the variable's value as a word |
//! | `work N` | `N` units of cost |
//! | `emit W`, `alloc T` | an output effect; objects are numbered at the link |
//! | `begin`, `end` | a group: a save-stack value; `end` restores what it saved |
//! | `label L`, `ref L` | `label` stores `L p` into `aux` when its page ships; `ref` loads `aux`: `??`, or `p` in roman |
//! | `lineno` | the paragraph's first line number (a derived value) |
//! | `setbox B W`, `wd B` | a box register, and a read of its width field |
//! | `cite K`, `printbib` | `cite` stores into `bcf`; `biber` maps `bcf` to `bbl`; `printbib` loads `bbl` |
//! | `note W` | a store into `log`, which nothing loads |
//!
//! A paragraph's box is `ceil(width / 16)` lines high; the page builder
//! is a fold call per box that breaks pages at 4 lines, and `ship` is a
//! call per page. [`oracle`] is a plain interpreter over `BTreeMap`s
//! that iterates whole trips (Jacobi) and traces every call naively.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Write as _;

use crate::hash::{Version, hash64};
use crate::machine::{Cx, Loc, Machine, Stream, name_of};
use crate::pstack::PStack;
use crate::pvec::{PVec, Poly};
use crate::value::{Value, version_opt};

pub const LINE_W: i64 = 16;
pub const PAGE_H: i64 = 4;

/// A string with its version.
pub struct Str {
    ver: Version,
    s: String,
}

/// A tuple with its version.
pub struct Tup {
    ver: Version,
    items: Vec<Val>,
}

/// Sequences end to end, versioned as the one sequence they make:
/// the version comes from the parts' polynomial hashes (`Poly::then`),
/// so a paragraph's tokens are named without being copied, and only a
/// body that runs flattens them.
pub struct Cat {
    ver: Version,
    len: usize,
    parts: Vec<PVec<Val>>,
}

/// The stub's values.
#[derive(Clone)]
pub enum Val {
    Nil,
    Int(i64),
    Str(Arc<Str>),
    Seq(PVec<Val>),
    /// A sequence in parts; equal to the `Seq` of its elements.
    Cat(Arc<Cat>),
    Stack(PStack<Val>),
    Tup(Arc<Tup>),
}

impl Val {
    #[must_use]
    pub fn str(s: &str) -> Val {
        Val::Str(Arc::new(Str {
            ver: Version::of(s),
            s: s.to_string(),
        }))
    }
    #[must_use]
    pub fn tup(items: Vec<Val>) -> Val {
        let vs: Vec<Version> = items.iter().map(Value::version).collect();
        Val::Tup(Arc::new(Tup {
            ver: Version::node(5, &vs),
            items,
        }))
    }
    /// The sequences `parts` end to end.
    #[must_use]
    pub fn cat(mut parts: Vec<PVec<Val>>) -> Val {
        parts.retain(|p| !p.is_empty());
        if parts.len() <= 1 {
            return Val::Seq(parts.pop().unwrap_or_default());
        }
        let len = parts.iter().map(PVec::len).sum();
        let poly = parts.iter().fold(Poly::EMPTY, |h, p| h.then(p.poly()));
        Val::Cat(Arc::new(Cat {
            ver: Version::node(3, &[poly.version(len)]),
            len,
            parts,
        }))
    }
    #[must_use]
    pub fn int(&self) -> i64 {
        match self {
            Val::Int(n) => *n,
            _ => 0,
        }
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Val::Str(s) => &s.s,
            _ => "",
        }
    }
    #[must_use]
    pub fn seq(&self) -> PVec<Val> {
        match self {
            Val::Seq(s) => s.clone(),
            Val::Cat(c) => PVec::from_vec(c.parts.iter().flat_map(PVec::iter).cloned().collect()),
            _ => PVec::new(),
        }
    }
    fn get(&self, i: u32) -> Val {
        self.field(i).unwrap_or(Val::Nil)
    }
}

/// An integer's version: a bijective mix of the number (an odd
/// multiplier and a shift, twice, after a tag), not the hasher.
fn int_version(n: i64) -> Version {
    const TAG: u128 = 0x696e_7400_0000_0000_5d1c_8e3f_a9b2_4c71;
    const K: u128 = 0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835;
    let mut x = u128::from(n.cast_unsigned()) ^ TAG;
    x = x.wrapping_mul(K);
    x ^= x >> 67;
    x = x.wrapping_mul(K);
    Version(x ^ (x >> 59))
}

const NIL: Version = Version(0x6e69_6c00_0000_0000_0000_0000_0000_0002);

impl Value for Val {
    fn version(&self) -> Version {
        match self {
            Val::Nil => NIL,
            Val::Int(n) => int_version(*n),
            Val::Str(s) => s.ver,
            Val::Seq(s) => Version::node(3, &[s.version()]),
            Val::Cat(c) => c.ver,
            Val::Stack(s) => Version::node(4, &[s.version()]),
            Val::Tup(t) => t.ver,
        }
    }
    fn field(&self, field: u32) -> Option<Self> {
        match self {
            Val::Tup(t) => t.items.get(field as usize).cloned(),
            Val::Seq(s) => s.get(field as usize).cloned(),
            Val::Cat(c) => {
                let mut i = field as usize;
                if i >= c.len {
                    return None;
                }
                for p in &c.parts {
                    if i < p.len() {
                        return p.get(i).cloned();
                    }
                    i -= p.len();
                }
                None
            }
            _ => None,
        }
    }
}

impl fmt::Debug for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Nil => write!(f, "nil"),
            Val::Int(n) => write!(f, "{n}"),
            Val::Str(s) => write!(f, "{:?}", s.s),
            Val::Seq(s) => f.debug_list().entries(s.iter()).finish(),
            Val::Cat(c) => f
                .debug_list()
                .entries(c.parts.iter().flat_map(PVec::iter))
                .finish(),
            Val::Stack(s) => f.debug_list().entries(s.iter()).finish(),
            Val::Tup(t) => {
                write!(f, "(")?;
                for (i, x) in t.items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x:?}")?;
                }
                write!(f, ")")
            }
        }
    }
}

/// A name with its hash, made once: an address hashes as one word,
/// not its characters on every map operation.
#[derive(Clone)]
pub struct Name {
    h: u64,
    s: Arc<str>,
}

impl Name {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.s
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Name {
        Name {
            h: hash64(s),
            s: Arc::from(s),
        }
    }
}

impl PartialEq for Name {
    fn eq(&self, o: &Name) -> bool {
        self.h == o.h && (Arc::ptr_eq(&self.s, &o.s) || self.s == o.s)
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, o: &Name) -> Option<core::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Name {
    fn cmp(&self, o: &Name) -> core::cmp::Ordering {
        self.s.cmp(&o.s)
    }
}

impl core::hash::Hash for Name {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.h);
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.s)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.s, f)
    }
}

/// The stub's addresses.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Addr {
    Var(Name),
    Reg(Name),
    Save,
    Line,
    Stream(Name),
}

impl Addr {
    #[must_use]
    pub fn stream(s: &str) -> Addr {
        Addr::Stream(Name::from(s))
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Addr::Var(x) => write!(f, "var:{x}"),
            Addr::Reg(x) => write!(f, "reg:{x}"),
            Addr::Save => write!(f, "save"),
            Addr::Line => write!(f, "lineno"),
            Addr::Stream(x) => write!(f, "stream:{x}"),
        }
    }
}

/// The stub's output effects.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Effect {
    Emit(Arc<str>),
    Alloc(Arc<str>),
    /// (shared: the link copies every page's effect out of its record)
    Page(i64, Arc<str>),
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Emit(w) => write!(f, "emit {w}"),
            Effect::Alloc(t) => write!(f, "alloc {t}"),
            Effect::Page(n, t) => write!(f, "page {n} {t}"),
        }
    }
}

/// The stub's functions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Func {
    Main,
    Tokenize,
    Para,
    Page,
    Ship,
    Biber,
}

impl fmt::Display for Func {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Func::Main => "main",
            Func::Tokenize => "tokenize",
            Func::Para => "para",
            Func::Page => "page",
            Func::Ship => "ship",
            Func::Biber => "biber",
        })
    }
}

/// The stub language.
#[derive(Clone, Copy, Default, Debug)]
pub struct Stub;

/// The source value of `lines`.
#[must_use]
pub fn source(lines: &[String]) -> Val {
    Val::Seq(PVec::from_vec(lines.iter().map(|l| Val::str(l)).collect()))
}

/// Render the output: objects numbered at the link, in program order.
#[must_use]
pub fn link(effects: &[Effect]) -> String {
    let mut out = String::new();
    let mut obj = 0;
    for e in effects {
        match e {
            Effect::Emit(w) => {
                let _ = writeln!(out, "emit {w}");
            }
            Effect::Alloc(t) => {
                obj += 1;
                let _ = writeln!(out, "obj {obj} {t}");
            }
            Effect::Page(n, t) => {
                let _ = writeln!(out, "page {n}: {t}");
            }
        }
    }
    out
}

fn roman(mut n: i64) -> String {
    let mut s = String::new();
    for (v, r) in [(10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

impl Machine for Stub {
    type Addr = Addr;
    type Val = Val;
    type Effect = Effect;
    type Func = Func;

    fn run<C: Cx<Self>>(&self, f: Func, args: &[Val], cx: &mut C) -> Val {
        match f {
            Func::Tokenize => tokenize(args[0].as_str(), cx),
            Func::Main => main(&args[0].seq(), cx),
            Func::Para => para(&args[0].seq(), cx),
            Func::Page => page_step(&args[0], args[1].int()),
            Func::Ship => ship(args[0].int(), &args[1].seq(), cx),
            Func::Biber => biber(&args[0].seq(), cx),
        }
    }
}

fn tokenize<C: Cx<Stub>>(line: &str, cx: &mut C) -> Val {
    cx.cost(line.len() as u64);
    let text = line.split('%').next().unwrap_or("");
    Val::Seq(PVec::from_vec(
        text.split_whitespace().map(Val::str).collect(),
    ))
}

struct Pager {
    /// The paragraph's lines' tokens, joined only by version.
    toks: Vec<PVec<Val>>,
    first: usize,
    acc: Val,
    page: Vec<Val>,
}

fn main<C: Cx<Stub>>(src: &PVec<Val>, cx: &mut C) -> Val {
    cx.open(&Addr::stream("aux"));
    let mut p = Pager {
        toks: Vec::new(),
        first: 0,
        acc: Val::tup(alloc::vec![Val::Int(0), Val::Int(1)]),
        page: Vec::new(),
    };
    for (i, line) in src.iter().enumerate() {
        let t = cx.call(Func::Tokenize, core::slice::from_ref(line)).seq();
        if t.is_empty() {
            flush(&mut p, cx);
            continue;
        }
        if p.toks.is_empty() {
            p.first = i + 1;
        }
        p.toks.push(t);
    }
    flush(&mut p, cx);
    if !p.page.is_empty() {
        let boxes = Val::Seq(PVec::from_vec(core::mem::take(&mut p.page)));
        cx.call(Func::Ship, &[p.acc.get(1), boxes]);
    }
    if let Some(bcf) = cx.load(&Addr::stream("bcf")) {
        cx.call(Func::Biber, &[Val::Seq(bcf)]);
    }
    p.acc.get(1)
}

fn flush<C: Cx<Stub>>(p: &mut Pager, cx: &mut C) {
    if p.toks.is_empty() {
        return;
    }
    cx.write(
        &Addr::Line,
        Some(Val::Int(i64::try_from(p.first).unwrap_or(0))),
    );
    let toks = Val::cat(core::mem::take(&mut p.toks));
    let bx = cx.call(Func::Para, &[toks]);
    let r = cx.call(Func::Page, &[p.acc.clone(), bx.get(0)]);
    if r.get(1).int() != 0 {
        let boxes = Val::Seq(PVec::from_vec(core::mem::take(&mut p.page)));
        cx.call(Func::Ship, &[p.acc.get(1), boxes]);
    }
    p.acc = r.get(0);
    p.page.push(bx);
}

fn page_step(acc: &Val, h: i64) -> Val {
    let (ah, no) = (acc.get(0).int(), acc.get(1).int());
    if ah > 0 && ah + h > PAGE_H {
        Val::tup(alloc::vec![
            Val::tup(alloc::vec![Val::Int(h), Val::Int(no + 1)]),
            Val::Int(1)
        ])
    } else {
        Val::tup(alloc::vec![
            Val::tup(alloc::vec![Val::Int(ah + h), Val::Int(no)]),
            Val::Int(0)
        ])
    }
}

fn ship<C: Cx<Stub>>(no: i64, boxes: &PVec<Val>, cx: &mut C) -> Val {
    let mut text = String::new();
    for (i, b) in boxes.iter().enumerate() {
        if i > 0 {
            text.push_str(" | ");
        }
        text.push_str(b.get(2).as_str());
    }
    cx.cost(text.len() as u64);
    cx.effect(Effect::Page(no, Arc::from(text)));
    let aux = Addr::stream("aux");
    for b in boxes {
        for l in &b.get(3).seq() {
            cx.store(&aux, Val::str(&format!("{} {no}", l.as_str())));
        }
    }
    Val::Nil
}

fn biber<C: Cx<Stub>>(bcf: &PVec<Val>, cx: &mut C) -> Val {
    let keys: BTreeSet<&str> = bcf.iter().map(Val::as_str).collect();
    cx.cost(bcf.len() as u64 * 10);
    let bbl = Addr::stream("bbl");
    cx.open(&bbl);
    for k in keys {
        cx.store(&bbl, Val::str(&format!("[{k}]")));
    }
    Val::Nil
}

fn var(x: &str) -> Addr {
    Addr::Var(Name::from(x))
}

/// Save `x`'s value in the innermost group's frame, once per group.
fn save<C: Cx<Stub>>(x: &str, cx: &mut C) {
    let Some(Val::Stack(st)) = cx.read(&Addr::Save) else {
        return;
    };
    let Some((top, rest)) = st.pop() else {
        return;
    };
    let mut frame = top.seq();
    if frame.iter().any(|e| e.get(0).as_str() == x) {
        return;
    }
    let old = cx.read(&var(x)).unwrap_or(Val::Nil);
    frame.push(Val::tup(alloc::vec![Val::str(x), old]));
    cx.write(&Addr::Save, Some(Val::Stack(rest.push(Val::Seq(frame)))));
}

fn value<C: Cx<Stub>>(w: &str, cx: &mut C) -> i64 {
    w.parse()
        .unwrap_or_else(|_| cx.read(&var(w)).map_or(0, |v| v.int()))
}

// One arm per command: splitting the match would scatter the language.
#[allow(clippy::too_many_lines)]
fn para<C: Cx<Stub>>(toks: &PVec<Val>, cx: &mut C) -> Val {
    let t: Vec<&str> = toks.iter().map(Val::as_str).collect();
    let mut words: Vec<String> = Vec::new();
    let mut labels = Vec::new();
    let mut i = 0;
    let has = |i: usize, k: usize| i + k < t.len();
    while i < t.len() {
        let w = t[i];
        match w {
            "set" if has(i, 2) => {
                save(t[i + 1], cx);
                let n = t[i + 2].parse().unwrap_or(0);
                cx.write(&var(t[i + 1]), Some(Val::Int(n)));
                i += 3;
            }
            "add" if has(i, 2) => {
                save(t[i + 1], cx);
                let v = value(t[i + 1], cx).wrapping_add(value(t[i + 2], cx));
                cx.write(&var(t[i + 1]), Some(Val::Int(v)));
                i += 3;
            }
            "print" if has(i, 1) => {
                words.push(value(t[i + 1], cx).to_string());
                i += 2;
            }
            "work" if has(i, 1) => {
                let n: u64 = t[i + 1].parse().unwrap_or(0);
                cx.cost(n);
                let mut x = 0u64;
                for k in 0..n.min(100_000) {
                    x = core::hint::black_box(x.wrapping_mul(31).wrapping_add(k));
                }
                core::hint::black_box(x);
                i += 2;
            }
            "emit" if has(i, 1) => {
                cx.effect(Effect::Emit(Arc::from(t[i + 1])));
                i += 2;
            }
            "alloc" if has(i, 1) => {
                cx.effect(Effect::Alloc(Arc::from(t[i + 1])));
                i += 2;
            }
            "begin" => {
                let st = match cx.read(&Addr::Save) {
                    Some(Val::Stack(s)) => s,
                    _ => PStack::new(),
                };
                cx.write(
                    &Addr::Save,
                    Some(Val::Stack(st.push(Val::Seq(PVec::new())))),
                );
                i += 1;
            }
            "end" => {
                let st = match cx.read(&Addr::Save) {
                    Some(Val::Stack(s)) => s,
                    _ => PStack::new(),
                };
                if let Some((frame, rest)) = st.pop() {
                    for e in &frame.seq() {
                        let old = e.get(1);
                        let v = if matches!(old, Val::Nil) {
                            None
                        } else {
                            Some(old)
                        };
                        cx.write(&var(e.get(0).as_str()), v);
                    }
                    let rest = if rest.is_empty() {
                        None
                    } else {
                        Some(Val::Stack(rest))
                    };
                    cx.write(&Addr::Save, rest);
                } else {
                    words.push("!end".to_string());
                }
                i += 1;
            }
            "label" if has(i, 1) => {
                labels.push(Val::str(t[i + 1]));
                i += 2;
            }
            "ref" if has(i, 1) => {
                let aux = cx.load(&Addr::stream("aux"));
                let key = t[i + 1];
                let found = aux.and_then(|a| {
                    a.iter()
                        .filter_map(|l| {
                            let (k, p) = l.as_str().split_once(' ')?;
                            (k == key).then(|| p.parse::<i64>().unwrap_or(0))
                        })
                        .last()
                });
                words.push(found.map_or_else(|| "??".to_string(), roman));
                i += 2;
            }
            "lineno" => {
                words.push(cx.read(&Addr::Line).map_or(0, |v| v.int()).to_string());
                i += 1;
            }
            "setbox" if has(i, 2) => {
                let w = t[i + 2];
                let b = Val::tup(alloc::vec![
                    Val::Int(1),
                    Val::Int(i64::try_from(w.len()).unwrap_or(0)),
                    Val::str(w)
                ]);
                cx.write(&Addr::Reg(Name::from(t[i + 1])), Some(b));
                i += 3;
            }
            "wd" if has(i, 1) => {
                let w = cx.read_field(&Addr::Reg(Name::from(t[i + 1])), 1);
                words.push(w.map_or(0, |v| v.int()).to_string());
                i += 2;
            }
            "cite" if has(i, 1) => {
                cx.store(&Addr::stream("bcf"), Val::str(t[i + 1]));
                i += 2;
            }
            "printbib" => {
                match cx.load(&Addr::stream("bbl")) {
                    Some(s) => words.extend(s.iter().map(|l| l.as_str().to_string())),
                    None => words.push("[nobib]".to_string()),
                }
                i += 1;
            }
            "note" if has(i, 1) => {
                cx.store(&Addr::stream("log"), Val::str(t[i + 1]));
                i += 2;
            }
            _ => {
                words.push(w.to_string());
                i += 1;
            }
        }
    }
    let text = words.join(" ");
    cx.cost(text.len() as u64);
    let width = i64::try_from(text.len()).unwrap_or(0);
    let height = ((width + LINE_W - 1) / LINE_W).max(1);
    Val::tup(alloc::vec![
        Val::Int(height),
        Val::Int(width),
        Val::str(&text),
        Val::Seq(PVec::from_vec(labels))
    ])
}

/// The oracle: a plain interpreter over `BTreeMap`s that iterates whole
/// trips (Jacobi) and traces every call's name and external reads
/// naively, with sets.
pub mod oracle {
    use super::{
        Addr, BTreeMap, BTreeSet, Effect, Func, Loc, PVec, Stream, String, Stub, Val, Vec, Version,
        link, name_of, source, version_opt,
    };
    use crate::machine::{Cx, Machine};
    use crate::value::Value;

    /// A call as the oracle saw it.
    #[derive(Clone, Debug)]
    pub struct OCall {
        pub func: Func,
        pub name: Version,
        pub reads: Vec<(Loc<Addr>, Version)>,
        pub children: Vec<OCall>,
    }

    pub type Streams = BTreeMap<Addr, Vec<Val>>;

    pub struct OTrip {
        pub streams: Streams,
        pub loaded: BTreeSet<Addr>,
        /// The root's calls (one: `main`).
        pub calls: Vec<OCall>,
        pub output: String,
        pub converged: bool,
    }

    pub struct OBuild {
        pub trips: Vec<OTrip>,
        pub output: String,
        pub converged: bool,
        pub streams: Streams,
    }

    struct Frame {
        func: Func,
        name: Version,
        reads: Vec<(Loc<Addr>, Version)>,
        seen: BTreeSet<Loc<Addr>>,
        written: BTreeSet<Addr>,
        children: Vec<OCall>,
    }

    impl Frame {
        fn note(&mut self, loc: Loc<Addr>, v: Version) {
            let internal = match &loc {
                Loc::State(a) | Loc::Field(a, _) => self.written.contains(a),
                Loc::Phi(_) => false,
            };
            if !internal && self.seen.insert(loc.clone()) {
                self.reads.push((loc, v));
            }
        }
    }

    struct O<'a> {
        state: BTreeMap<Addr, Val>,
        phi: &'a Streams,
        effects: Vec<Effect>,
        stores: Streams,
        loaded: BTreeSet<Addr>,
        frames: Vec<Frame>,
    }

    fn sver(s: Option<&Vec<Val>>) -> Version {
        s.map_or(Version::ABSENT, |v| PVec::from_vec(v.clone()).version())
    }

    impl Cx<Stub> for O<'_> {
        fn read(&mut self, a: &Addr) -> Option<Val> {
            let v = self.state.get(a).cloned();
            self.frames
                .last_mut()
                .expect("frame")
                .note(Loc::State(a.clone()), version_opt(v.as_ref()));
            v
        }
        fn read_field(&mut self, a: &Addr, field: u32) -> Option<Val> {
            let v = self.state.get(a).and_then(|x| x.field(field));
            self.frames
                .last_mut()
                .expect("frame")
                .note(Loc::Field(a.clone(), field), version_opt(v.as_ref()));
            v
        }
        fn write(&mut self, a: &Addr, v: Option<Val>) {
            match v {
                Some(v) => {
                    self.state.insert(a.clone(), v);
                }
                None => {
                    self.state.remove(a);
                }
            }
            self.frames
                .last_mut()
                .expect("frame")
                .written
                .insert(a.clone());
        }
        fn effect(&mut self, e: Effect) {
            self.effects.push(e);
        }
        fn call(&mut self, f: Func, args: &[Val]) -> Val {
            let vs: Vec<Version> = args.iter().map(Value::version).collect();
            self.frames.push(Frame {
                func: f,
                name: name_of(f, &vs),
                reads: Vec::new(),
                seen: BTreeSet::new(),
                written: BTreeSet::new(),
                children: Vec::new(),
            });
            let r = Stub.run(f, args, self);
            let c = self.frames.pop().expect("frame");
            let p = self.frames.last_mut().expect("parent");
            for (l, v) in &c.reads {
                p.note(l.clone(), *v);
            }
            p.written.extend(c.written);
            p.children.push(OCall {
                func: c.func,
                name: c.name,
                reads: c.reads,
                children: c.children,
            });
            r
        }
        fn open(&mut self, s: &Addr) {
            self.stores.entry(s.clone()).or_default();
        }
        fn store(&mut self, s: &Addr, line: Val) {
            self.stores.entry(s.clone()).or_default().push(line);
        }
        fn load(&mut self, s: &Addr) -> Option<Stream<Val>> {
            self.loaded.insert(s.clone());
            let v = self.phi.get(s).cloned();
            self.frames
                .last_mut()
                .expect("frame")
                .note(Loc::Phi(s.clone()), sver(v.as_ref()));
            v.map(PVec::from_vec)
        }
        fn cost(&mut self, _units: u64) {}
    }

    /// Build `lines` from the previous build's streams, trip by trip.
    #[must_use]
    pub fn build(lines: &[String], prev: &Streams, max_trips: usize) -> OBuild {
        let src = source(lines);
        let mut phi = prev.clone();
        let mut trips: Vec<OTrip> = Vec::new();
        loop {
            let mut o = O {
                state: BTreeMap::new(),
                phi: &phi,
                effects: Vec::new(),
                stores: BTreeMap::new(),
                loaded: BTreeSet::new(),
                frames: alloc::vec![Frame {
                    func: Func::Main,
                    name: Version::ABSENT,
                    reads: Vec::new(),
                    seen: BTreeSet::new(),
                    written: BTreeSet::new(),
                    children: Vec::new(),
                }],
            };
            o.call(Func::Main, core::slice::from_ref(&src));
            let converged = o
                .loaded
                .iter()
                .all(|s| sver(phi.get(s)) == sver(o.stores.get(s)));
            let root = o.frames.pop().expect("root");
            let t = OTrip {
                streams: o.stores,
                loaded: o.loaded,
                calls: root.children,
                output: link(&o.effects),
                converged,
            };
            phi = t.streams.clone();
            trips.push(t);
            if converged || trips.len() >= max_trips {
                let last = trips.last().expect("a trip");
                return OBuild {
                    output: last.output.clone(),
                    converged,
                    streams: phi,
                    trips,
                };
            }
        }
    }
}

/// Random programs and edits for the property tests.
pub mod generate {
    use super::{String, ToString, Vec, format};

    /// splitmix64.
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        pub fn below(&mut self, n: usize) -> usize {
            // Truncation intended.
            #[allow(clippy::cast_possible_truncation)]
            let r = (self.next_u64() % n.max(1) as u64) as usize;
            r
        }
    }

    /// Which commands a program may use.
    #[derive(Clone, Copy, Debug)]
    #[allow(clippy::struct_excessive_bools)]
    pub struct Feats {
        pub vars: bool,
        pub refs: bool,
        pub cites: bool,
        pub lineno: bool,
    }

    impl Feats {
        pub const ALL: Feats = Feats {
            vars: true,
            refs: true,
            cites: true,
            lineno: true,
        };
        pub const TEXT: Feats = Feats {
            vars: false,
            refs: false,
            cites: false,
            lineno: false,
        };
    }

    const WORDS: &[&str] = &[
        "a",
        "to",
        "the",
        "word",
        "quick",
        "brown",
        "lazy",
        "paragraph",
        "xy",
    ];

    fn item(r: &mut Rng, f: Feats) -> String {
        let v = |r: &mut Rng| format!("x{}", r.below(4));
        let k = r.below(if f.vars || f.refs || f.cites || f.lineno {
            40
        } else {
            10
        });
        match k {
            10 | 11 if f.vars => format!("set {} {}", v(r), r.below(9)),
            12 if f.vars => format!("add {} {}", v(r), v(r)),
            13 | 14 if f.vars => format!("print {}", v(r)),
            15 if f.vars => "begin".to_string(),
            16 if f.vars => "end".to_string(),
            17 | 18 if f.refs => format!("label L{}", r.below(3)),
            19 | 20 if f.refs => format!("ref L{}", r.below(3)),
            21 if f.lineno => "lineno".to_string(),
            22 => format!("alloc t{}", r.below(3)),
            23 => format!("emit e{}", r.below(3)),
            24 if f.vars => format!("setbox b{} {}", r.below(2), WORDS[r.below(WORDS.len())]),
            25 if f.vars => format!("wd b{}", r.below(2)),
            26 if f.cites => format!("cite k{}", r.below(4)),
            27 if f.cites => "printbib".to_string(),
            28 => format!("note n{}", r.below(3)),
            29 => format!("work {}", r.below(50)),
            _ => WORDS[r.below(WORDS.len())].to_string(),
        }
    }

    /// A random line of 1–5 items.
    pub fn line(r: &mut Rng, f: Feats) -> String {
        let n = 1 + r.below(5);
        (0..n).map(|_| item(r, f)).collect::<Vec<_>>().join(" ")
    }

    /// A random program of `paras` paragraphs of 1–3 lines.
    pub fn program(r: &mut Rng, paras: usize, f: Feats) -> Vec<String> {
        let mut out = Vec::new();
        for p in 0..paras {
            if p > 0 {
                out.push(String::new());
            }
            for _ in 0..=r.below(3) {
                out.push(line(r, f));
            }
        }
        out
    }

    /// The paragraphs, as ranges of lines.
    #[must_use]
    pub fn paragraphs(lines: &[String]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = None;
        for (i, l) in lines.iter().enumerate() {
            let blank = l.split('%').next().unwrap_or("").trim().is_empty();
            match (blank, start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    out.push((s, i));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push((s, lines.len()));
        }
        out
    }

    /// Move paragraph `from` before paragraph `to` (by index).
    pub fn move_paragraph(lines: &mut Vec<String>, from: usize, to: usize) {
        let ps = paragraphs(lines);
        if ps.len() < 2 || from == to {
            return;
        }
        let (s, e) = ps[from];
        let block: Vec<String> = lines[s..e].to_vec();
        let mut rest: Vec<String> = Vec::new();
        for (i, (a, b)) in ps.iter().enumerate() {
            if i == from {
                continue;
            }
            if i == to {
                if !rest.is_empty() {
                    rest.push(String::new());
                }
                rest.extend(block.iter().cloned());
            }
            if !rest.is_empty() {
                rest.push(String::new());
            }
            rest.extend(lines[*a..*b].iter().cloned());
        }
        if to >= ps.len() {
            rest.push(String::new());
            rest.extend(block);
        }
        *lines = rest;
    }

    /// Double one space of `l` (the same tokens).
    #[must_use]
    pub fn double_space(l: &str) -> String {
        match l.find(' ') {
            Some(i) => format!("{}  {}", &l[..i], &l[i + 1..]),
            None => format!("{l}  "),
        }
    }

    /// A random edit; returns its kind.
    pub fn edit(r: &mut Rng, lines: &mut Vec<String>, f: Feats) -> &'static str {
        let n = lines.len();
        match r.below(8) {
            0 => {
                let i = r.below(n + 1);
                lines.insert(i, line(r, f));
                "insert"
            }
            1 if n > 1 => {
                lines.remove(r.below(n));
                "delete"
            }
            2 | 3 if n > 0 => {
                let i = r.below(n);
                lines[i] = line(r, f);
                "change"
            }
            4 if n > 0 => {
                let i = r.below(n);
                lines[i] = double_space(&lines[i]);
                "retype"
            }
            5 => {
                let k = paragraphs(lines).len();
                let (a, b) = (r.below(k), r.below(k + 1));
                move_paragraph(lines, a, b);
                "move"
            }
            6 => {
                // Push a label along: a filler paragraph before a random one.
                let ps = paragraphs(lines);
                let at = ps.get(r.below(ps.len().max(1))).map_or(0, |p| p.0);
                lines.insert(at, String::new());
                lines.insert(at, "paragraph paragraph paragraph".to_string());
                "push"
            }
            _ => {
                if n > 0 {
                    let i = r.below(n);
                    lines[i] = format!("{} % comment {}", lines[i], r.below(9));
                }
                "comment"
            }
        }
    }
}

/// The oracle's streams as the runtime's.
#[must_use]
pub fn to_pmap(s: &oracle::Streams) -> crate::pmap::PMap<Addr, Stream<Val>> {
    let mut m = crate::pmap::PMap::new();
    for (k, v) in s {
        m.insert(k.clone(), PVec::from_vec(v.clone()));
    }
    m
}

/// A stream's lines as text.
#[must_use]
pub fn lines_of(s: &Stream<Val>) -> Vec<String> {
    s.iter().map(|v| v.as_str().to_string()).collect()
}

/// A set of calls by name and reads.
pub type Seen = BTreeSet<(Version, Vec<(Loc<Addr>, Version)>)>;

/// The oracle's traces flattened, for exactness: the calls a memo
/// holding `seen` would re-run for `calls`, in program order, adding
/// each call to `seen` when it ends.
pub fn expected_misses(seen: &mut Seen, calls: &[oracle::OCall], out: &mut Vec<(Func, Version)>) {
    for c in calls {
        let key = (c.name, c.reads.clone());
        if seen.contains(&key) {
            continue;
        }
        out.push((c.func, c.name));
        expected_misses(seen, &c.children, out);
        seen.insert(key);
    }
}

/// Lines of a stream map as text, for comparisons.
#[must_use]
pub fn streams_text(s: &BTreeMap<Addr, Stream<Val>>) -> BTreeMap<String, Vec<String>> {
    s.iter()
        .map(|(k, v)| (k.to_string(), lines_of(v)))
        .collect()
}
