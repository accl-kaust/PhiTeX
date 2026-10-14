//! The paragraph-start token lists (`\everypar`), evaluated abstractly
//! along a document's slices.
//!
//! A paragraph's start inserts the list in force (tex.web §1091); a
//! heading's `\@afterheading` assigns one that defines `\if@nobreak`,
//! `\clubpenalty` and, on its second run, `\everypar` itself. What a
//! slice's paragraph start defines therefore depends on the slices
//! before it. The lists the definitions assign are found by text (the
//! bodies that assign `\everypar`, the *setters*), compiled to a small
//! program ([`Op`]: conditionals on `\newif` flags, the flags' setters,
//! assignments of `\everypar`, what each straight run defines), and run
//! over a set of abstract states ([`ParState`]): the list in force and
//! the flags known. A slice compiles to the same program (its paragraph
//! starts and ends, the setters its calls may run); [`ParMachine::slice`]
//! runs it and answers what the lists run in it may define. A flag the
//! text cannot know forks the state; too many states give up
//! ([`ParState::top`]: any list, every list's definitions).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::FileKind;
use super::atoms::{self, Atom, AtomKind};
use super::defs::{Definitions, skip};
use super::hints::{Closure, DefSet, effects, implicit_of, implicit_of_char};
use super::lex;
use super::{Implicit, Shape};

/// The most abstract states kept: beyond them, the state is unknown.
const CAP: usize = 16;

/// TeX's conditionals (§487), e-TeX's and pdfTeX's.
const CONDITIONALS: &[&str] = &[
    "if",
    "ifcat",
    "ifnum",
    "ifdim",
    "ifodd",
    "ifvmode",
    "ifhmode",
    "ifmmode",
    "ifinner",
    "ifvoid",
    "ifhbox",
    "ifvbox",
    "ifx",
    "ifeof",
    "iftrue",
    "iffalse",
    "ifcase",
    "ifdefined",
    "ifcsname",
    "iffontchar",
    "ifincsname",
    "ifpdfprimitive",
    "ifpdfabsnum",
    "ifpdfabsdim",
];

/// How deep setters may run setters.
const DEPTH: u8 = 6;

/// One step of a compiled text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Horizontal material in vertical mode: a paragraph starts (§1091).
    Start,
    /// A call that may start one (its body has horizontal material).
    MayStart,
    /// `\par` or a blank line: vertical mode again.
    End,
    /// A conditional: its flag when the text knows it (`if@nobreak`),
    /// its branches.
    If(Option<String>, Vec<Op>, Vec<Op>),
    /// A flag set (`\@nobreakfalse`).
    Set(String, bool),
    /// Flags a call may set to a value the text cannot know.
    Unknown(Vec<String>),
    /// `\everypar{…}`: the list (its text), or `None` when computed.
    Assign(Option<Arc<str>>),
    /// A setter's body runs.
    Run(String),
    /// A call may run these setters, or none of them.
    MayRun(Vec<String>),
    /// What a straight run of commands defines.
    Defines(DefSet),
}

/// An abstract state: the list in force (`Some("")`: none, `None`:
/// unknown), the flags known, and the mode.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParState {
    list: Option<Arc<str>>,
    flags: BTreeMap<String, bool>,
    horizontal: bool,
}

impl ParState {
    /// A document's start: no list, the flags unknown, vertical mode.
    #[must_use]
    pub fn start() -> ParState {
        ParState {
            list: Some(Arc::from("")),
            flags: BTreeMap::new(),
            horizontal: false,
        }
    }

    /// Nothing known: any list may be in force.
    #[must_use]
    pub fn top() -> ParState {
        ParState {
            list: None,
            flags: BTreeMap::new(),
            horizontal: false,
        }
    }
}

/// The setters and lists of a set of definitions, compiled.
#[derive(Debug)]
pub struct ParMachine {
    setters: HashMap<String, Vec<Op>>,
    lists: HashMap<Arc<str>, Vec<Op>>,
    /// Every list's definitions: what an unknown list may define.
    all: DefSet,
}

/// What a run of a slice found: the states after it, and what the lists
/// its paragraph starts ran may define.
pub struct Ran {
    pub states: Vec<ParState>,
    pub defines: DefSet,
}

impl ParMachine {
    /// The machine of the definitions `document` over `prelude`: `None`
    /// when no body they give assigns `\everypar`.
    #[must_use]
    pub fn new(document: &Definitions, prelude: &Definitions) -> Option<ParMachine> {
        let mut bodies: HashMap<&str, &str> = HashMap::new();
        for (n, t) in prelude.last_bodies().chain(document.last_bodies()) {
            bodies.insert(n, t);
        }
        let setter_names: HashSet<String> = bodies
            .iter()
            .filter(|(n, t)| !n.starts_with('\0') && t.contains("\\everypar"))
            .map(|(n, _)| (*n).to_owned())
            .collect();
        if setter_names.is_empty() {
            return None;
        }
        let mut c = Compiler {
            closure: Closure::new(document, prelude),
            setters: &setter_names,
            lists: Vec::new(),
            top: false,
        };
        let mut setters = HashMap::new();
        for n in &setter_names {
            let ops = c.compile_text(bodies[n.as_str()]);
            setters.insert(n.clone(), ops);
        }
        let mut lists: HashMap<Arc<str>, Vec<Op>> = HashMap::new();
        let mut k = 0;
        while k < c.lists.len() {
            let t = c.lists[k].clone();
            k += 1;
            if lists.contains_key(&t) {
                continue;
            }
            let ops = c.compile_text(&t);
            lists.insert(t, ops);
        }
        let mut all = DefSet::default();
        for ops in lists.values() {
            defines_of(ops, &mut all);
        }
        Some(ParMachine {
            setters,
            lists,
            all,
        })
    }

    /// The program of a slice (atoms `lo..hi` of `text`, read with the
    /// definitions `document` over `prelude`): its paragraph starts and
    /// ends, the setters and flags its calls may run or set.
    #[must_use]
    pub fn slice_ops(
        &self,
        text: &str,
        atoms: &[Atom],
        lo: usize,
        hi: usize,
        document: &Definitions,
        prelude: &Definitions,
    ) -> Vec<Op> {
        let names: HashSet<String> = self.setters.keys().cloned().collect();
        let mut c = Compiler {
            closure: Closure::new(document, prelude),
            setters: &names,
            lists: Vec::new(),
            top: true,
        };
        c.compile(text, atoms, lo, hi)
    }

    /// Runs a slice's program from `states`.
    #[must_use]
    pub fn slice(&self, ops: &[Op], states: &[ParState]) -> Ran {
        let mut cx = Cx {
            m: self,
            defines: DefSet::default(),
            collect: false,
        };
        let out = cx.run_all(ops, states.to_vec(), 0);
        Ran {
            states: out,
            defines: cx.defines,
        }
    }
}

/// What a program defines in every branch.
fn defines_of(ops: &[Op], out: &mut DefSet) {
    for op in ops {
        match op {
            Op::Defines(d) => out.add(d),
            Op::If(_, a, b) => {
                defines_of(a, out);
                defines_of(b, out);
            }
            _ => {}
        }
    }
}

struct Cx<'m> {
    m: &'m ParMachine,
    defines: DefSet,
    /// Whether a list runs: its definitions are the answer.
    collect: bool,
}

impl Cx<'_> {
    fn run_all(&mut self, ops: &[Op], mut states: Vec<ParState>, depth: u8) -> Vec<ParState> {
        for op in ops {
            let mut next = Vec::new();
            for s in states {
                next.extend(self.step(op, s, depth));
            }
            next.sort();
            next.dedup();
            if next.len() > CAP {
                // (too many: the lists' every definition from here on)
                self.defines.add(&self.m.all);
                next = vec![ParState::top()];
            }
            states = next;
        }
        states
    }

    fn step(&mut self, op: &Op, mut s: ParState, depth: u8) -> Vec<ParState> {
        match op {
            Op::Start => self.start(s, depth),
            Op::MayStart => {
                let mut v = self.start(s.clone(), depth);
                v.push(s);
                v
            }
            Op::End => {
                s.horizontal = false;
                vec![s]
            }
            Op::If(flag, a, b) => match flag.as_ref().and_then(|f| s.flags.get(f)) {
                Some(true) => self.run_all(a, vec![s], depth),
                Some(false) => self.run_all(b, vec![s], depth),
                None => {
                    let mut v = self.run_all(a, vec![s.clone()], depth);
                    v.extend(self.run_all(b, vec![s], depth));
                    v
                }
            },
            Op::Set(f, v) => {
                s.flags.insert(f.clone(), *v);
                vec![s]
            }
            Op::Unknown(fs) => {
                for f in fs {
                    s.flags.remove(f);
                }
                vec![s]
            }
            Op::Assign(l) => {
                s.list.clone_from(l);
                vec![s]
            }
            Op::Run(m) => self.run_setter(m, s, depth),
            Op::MayRun(ms) => {
                let mut v = vec![s.clone()];
                for m in ms {
                    v.extend(self.run_setter(m, s.clone(), depth));
                }
                v
            }
            Op::Defines(d) => {
                if self.collect {
                    self.defines.add(d);
                }
                vec![s]
            }
        }
    }

    fn run_setter(&mut self, m: &str, s: ParState, depth: u8) -> Vec<ParState> {
        match self.m.setters.get(m) {
            Some(ops) if depth < DEPTH => self.run_all(ops, vec![s], depth + 1),
            _ => vec![ParState { list: None, ..s }],
        }
    }

    /// A paragraph starts: the list in force runs (§1091).
    fn start(&mut self, mut s: ParState, depth: u8) -> Vec<ParState> {
        if s.horizontal {
            return vec![s];
        }
        s.horizontal = true;
        let Some(l) = s.list.clone() else {
            self.defines.add(&self.m.all);
            return vec![s];
        };
        if l.is_empty() {
            return vec![s];
        }
        let Some(ops) = self.m.lists.get(&l) else {
            self.defines.add(&self.m.all);
            return vec![ParState { list: None, ..s }];
        };
        if depth >= DEPTH {
            self.defines.add(&self.m.all);
            return vec![ParState { list: None, ..s }];
        }
        let collect = std::mem::replace(&mut self.collect, true);
        let out = self.run_all(ops, vec![s], depth + 1);
        self.collect = collect;
        out
    }
}

struct Compiler<'a, 'd> {
    closure: Closure<'d>,
    setters: &'a HashSet<String>,
    /// The lists met (texts of `\everypar{…}`).
    lists: Vec<Arc<str>>,
    /// A slice's text (its paragraph starts count), not a body.
    top: bool,
}

impl Compiler<'_, '_> {
    fn compile_text(&mut self, text: &str) -> Vec<Op> {
        let lexed = lex::lex(text, FileKind::Package, None);
        let atoms = atoms::atoms(text, &lexed.tokens);
        self.compile(text, &atoms, 0, atoms.len())
    }

    fn is_flag(&self, name: &str) -> bool {
        let sh = self
            .closure
            .document
            .shapes(name)
            .or_else(|| self.closure.prelude.shapes(name));
        sh.is_some_and(|v| v.iter().any(|s| matches!(s, Shape::Cond)))
    }

    /// The flag a name sets: `@nobreaktrue` sets `if@nobreak`.
    fn flag_set(&self, name: &str) -> Option<(String, bool)> {
        let (base, v) = if let Some(b) = name.strip_suffix("true") {
            (b, true)
        } else {
            (name.strip_suffix("false")?, false)
        };
        let f = format!("if{base}");
        self.is_flag(&f).then_some((f, v))
    }

    /// Whether `name` is a conditional: a `\\newif` flag or TeX's.
    fn is_conditional(&self, name: &str) -> bool {
        CONDITIONALS.contains(&name) || self.is_flag(name)
    }

    /// The end of the conditional whose branches begin at `k`: the
    /// `\else` (if any) and the `\fi`, at its own level.
    fn branches(&self, text: &str, atoms: &[Atom], k: usize, hi: usize) -> (Option<usize>, usize) {
        let mut level = 0usize;
        let mut els = None;
        for (j, a) in atoms.iter().enumerate().take(hi).skip(k) {
            if a.kind != AtomKind::Cs {
                continue;
            }
            let n = a.name(text);
            if n == "fi" {
                if level == 0 {
                    return (els, j);
                }
                level -= 1;
            } else if n == "else" && level == 0 {
                els = Some(j);
            } else if self.is_conditional(n) {
                level += 1;
            }
        }
        (els, hi)
    }

    #[allow(clippy::too_many_lines)]
    fn compile(&mut self, text: &str, atoms: &[Atom], lo: usize, hi: usize) -> Vec<Op> {
        let mut out = Vec::new();
        let mut seg = lo;
        let mut depth = 0i64;
        let mut k = lo;
        while k < hi {
            let a = atoms[k];
            match a.kind {
                AtomKind::Open => {
                    depth += 1;
                    k += 1;
                    continue;
                }
                AtomKind::Close => {
                    depth -= 1;
                    k += 1;
                    continue;
                }
                AtomKind::Par => {
                    if self.top {
                        out.push(Op::End);
                    }
                    k += 1;
                    continue;
                }
                AtomKind::Char(c) => {
                    if self.top && depth == 0 && implicit_of_char(c) == Implicit::StartsParagraph {
                        out.push(Op::Start);
                    }
                    k += 1;
                    continue;
                }
                AtomKind::Cs => {}
                _ => {
                    k += 1;
                    continue;
                }
            }
            let n = a.name(text);
            if super::hints::is_definer(n) {
                // (a definition: its body runs when that macro is called)
                k = super::hints::past_definition(atoms, k + 1, text);
                continue;
            }
            if self.is_conditional(n) {
                self.flush(text, atoms, seg, k, &mut out);
                let (els, fi) = self.branches(text, atoms, k + 1, hi);
                // (`\ifx`, `\if`, `\ifcat` compare the two tokens after
                // them, §507: no command)
                let first = if matches!(n, "ifx" | "ifcat" | "if") {
                    (skip(atoms, skip(atoms, k + 1) + 1) + 1).min(hi)
                } else {
                    k + 1
                };
                let then = self.compile(text, atoms, first, els.unwrap_or(fi));
                let other = match els {
                    Some(e) => self.compile(text, atoms, e + 1, fi),
                    None => Vec::new(),
                };
                match n {
                    "iftrue" => out.extend(then),
                    "iffalse" => out.extend(other),
                    _ => out.push(Op::If(self.is_flag(n).then(|| n.to_owned()), then, other)),
                }
                k = fi + 1;
                seg = k;
                continue;
            }
            if let Some((f, v)) = self.flag_set(n) {
                self.flush(text, atoms, seg, k + 1, &mut out);
                // (`\newif`'s setters `\let` the flag, §1221)
                if !self.top {
                    let mut d = DefSet::default();
                    d.names.insert(super::hints::cs(&f), false);
                    out.push(Op::Defines(d));
                }
                out.push(Op::Set(f, v));
                k += 1;
                seg = k;
                continue;
            }
            if n == "everypar" {
                let mut j = skip(atoms, k + 1);
                if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('=')) {
                    j = skip(atoms, j + 1);
                }
                match atoms.get(j) {
                    Some(o) if o.kind == AtomKind::Open && o.mate != usize::MAX && o.mate < hi => {
                        let t: Arc<str> = Arc::from(&text[o.end..atoms[o.mate].start]);
                        self.flush(text, atoms, seg, o.mate + 1, &mut out);
                        let known = !t.contains('#');
                        if known {
                            self.lists.push(t.clone());
                        }
                        out.push(Op::Assign(known.then_some(t)));
                        k = o.mate + 1;
                    }
                    _ => {
                        self.flush(text, atoms, seg, k + 1, &mut out);
                        out.push(Op::Assign(None));
                        k += 1;
                    }
                }
                seg = k;
                continue;
            }
            if self.setters.contains(n) {
                self.flush(text, atoms, seg, k, &mut out);
                out.push(Op::Run(n.to_owned()));
                k += 1;
                seg = k;
                continue;
            }
            match implicit_of(n, false) {
                Implicit::StartsParagraph if self.top && depth == 0 => out.push(Op::Start),
                Implicit::EndsParagraph if self.top && depth == 0 => out.push(Op::End),
                _ => {}
            }
            // (a macro: the setters it may run, the flags it may set)
            let set = self.closure.call(n);
            let ms: Vec<String> = set
                .relied
                .iter()
                .filter(|r| self.setters.contains(r.as_str()))
                .cloned()
                .collect();
            let flags: Vec<String> = set
                .names
                .keys()
                .filter_map(|f| f.strip_prefix("cs:\\"))
                .filter(|f| self.is_flag(f))
                .map(str::to_owned)
                .collect();
            if !flags.is_empty() {
                out.push(Op::Unknown(flags));
            }
            if !ms.is_empty() {
                out.push(Op::MayRun(ms));
            }
            if self.top
                && depth == 0
                && set
                    .calls
                    .iter()
                    .any(|c| implicit_of(c, false) == Implicit::StartsParagraph)
            {
                out.push(Op::MayStart);
            }
            k += 1;
        }
        self.flush(text, atoms, seg, hi, &mut out);
        out
    }

    /// What the atoms `lo..hi` define, as one step (bodies only: a
    /// slice's own definitions are its hint's).
    fn flush(&mut self, text: &str, atoms: &[Atom], lo: usize, hi: usize, out: &mut Vec<Op>) {
        if self.top || lo >= hi {
            return;
        }
        let e = effects(text, atoms, lo, hi, false);
        let d = self.closure.of(&e);
        if !d.names.is_empty()
            || !d.prefixes.is_empty()
            || !d.unbounded.is_empty()
            || !d.calls.is_empty()
        {
            out.push(Op::Defines(d));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A heading's list along the paragraphs after it: the first
    /// defines the flag and `\clubpenalty`, the second `\clubpenalty`
    /// and `\everypar` (the list empties itself), the third nothing.
    #[test]
    fn heading_list_runs_out() {
        let prelude_text = "\\newif\\if@nobreak\n\
            \\def\\@afterheading{\\@nobreaktrue\\everypar{\\if@nobreak\\@nobreakfalse\\clubpenalty\\@M\\else\\clubpenalty\\@clubpenalty\\everypar{}\\fi}}\n\
            \\def\\section#1{\\par\\@afterheading}\n";
        let mut prelude = Definitions::new();
        prelude.harvest(prelude_text, FileKind::Package, 1);
        let doc = Definitions::new();
        let m = ParMachine::new(&doc, &prelude).expect("a setter");
        let slices = ["\\section{A}\n", "One.\n\n", "Two.\n\n", "Three.\n\n"];
        let mut states = vec![ParState::start()];
        let mut got = Vec::new();
        for t in slices {
            let lexed = lex::lex(t, FileKind::Document, None);
            let atoms = atoms::atoms(t, &lexed.tokens);
            let ops = m.slice_ops(t, &atoms, 0, atoms.len(), &doc, &prelude);
            let r = m.slice(&ops, &states);
            states = r.states;
            got.push(r.defines.names.keys().cloned().collect::<Vec<_>>());
        }
        assert!(got[0].is_empty(), "{got:?}");
        assert!(got[1].iter().any(|n| n.contains("if@nobreak")), "{got:?}");
        assert!(got[1].iter().any(|n| n.contains("clubpenalty")), "{got:?}");
        assert!(!got[1].iter().any(|n| n.contains("everypar")), "{got:?}");
        assert!(got[2].iter().any(|n| n.contains("everypar")), "{got:?}");
        assert!(got[3].is_empty(), "{got:?}");
    }
}
