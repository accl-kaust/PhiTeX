//! Pieces: the source cut at blank lines (brace depth 0), and a piece's
//! run, a pure function of the reaching definitions it reads.
//!
//! Inside a piece, tokens are read in order: each one's meaning can depend
//! on the command before it (a real dependency). Its scopes are its own
//! (a piece starts and ends at brace depth 0). What it shares is only what
//! it reads (through [`Env`], recorded with versions) and what it exports
//! (its top-level and global definitions) and its paragraphs.

use std::collections::HashMap;
use std::sync::Arc;

use crate::arith::{round_decimals, xn_over_d};
use crate::hash::hash_of;
use crate::tfm::Font;
use crate::val::{Key, Macro, Meaning, Param, Prim, Tok, Val, Ver};

/// Where a piece's reads of names from outside it are answered.
pub trait Env {
    /// The reaching definition of `key` at the piece's start.
    fn reach(&mut self, key: &Key) -> Val;
}

/// An item of a horizontal list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HItem {
    Char { c: u8, w: i32, h: i32, d: i32 },
    /// An empty box (the paragraph indentation).
    Box { w: i32, h: i32, d: i32 },
    Glue { w: i32, st: i32, st_ord: u8, sh: i32, sh_ord: u8 },
    Penalty(i32),
}

/// A paragraph: its list and the parameters it was read with. Line
/// breaking is a pure function of this, memoised by its hash.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Para {
    pub hlist: Vec<HItem>,
    pub hsize: i32,
    pub tolerance: i32,
    pub linepenalty: i32,
    pub baselineskip: i32,
    pub parskip: i32,
    pub vsize: i32,
    pub topskip: i32,
    pub maxdepth: i32,
}

/// An explicit φ at a `\fi`: the test's result, the keys either arm
/// defined (value after, value before the `\if`), and how many tokens the
/// untaken arm held.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Phi {
    pub test: bool,
    pub skipped: usize,
    pub keys: Vec<(Key, Ver, Ver)>,
}

/// What a piece's run produced.
#[derive(Clone, Debug, Default)]
pub struct PieceRun {
    /// Names read from outside the piece, with the versions read.
    pub reads: Vec<(Key, Ver)>,
    /// Definitions visible after the piece (top level and global).
    pub exports: Vec<(Key, Ver, Val)>,
    pub paras: Vec<Arc<Para>>,
    pub para_hashes: Vec<u64>,
    pub phis: Vec<Phi>,
    pub errors: Vec<String>,
    /// Tokens processed (the run's work).
    pub steps: u64,
}

/// Cuts the source into pieces at blank lines at brace depth 0. Lines lose
/// their trailing spaces and end in `\r` (TeX's end of line). The cuts are
/// hints: a piece is read with the real category codes.
pub fn cut(src: &str) -> Vec<Arc<[char]>> {
    let mut pieces: Vec<Arc<[char]>> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut depth: i32 = 0;
    let mut blank_run = false;
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        let line = line.trim_end_matches([' ', '\r']);
        let blank = line.trim_start_matches(' ').is_empty();
        if blank {
            blank_run = true;
        } else {
            if blank_run && depth == 0 && cur.iter().any(|c| !c.is_whitespace()) {
                pieces.push(Arc::from(std::mem::take(&mut cur)));
            }
            blank_run = false;
            depth = brace_depth(line, depth);
        }
        cur.extend(line.chars());
        cur.push('\r');
    }
    if !cur.is_empty() {
        pieces.push(Arc::from(cur));
    }
    pieces
}

fn brace_depth(line: &str, mut depth: i32) -> i32 {
    let mut it = line.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                it.next();
            }
            '%' => break,
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    depth
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    N,
    M,
    S,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    V,
    H,
}

struct Cond {
    test: bool,
    defined: Vec<(Key, Ver)>,
    skipped: usize,
}

struct Reader<'a> {
    env: &'a mut dyn Env,
    text: &'a [char],
    pos: usize,
    state: State,
    /// Tokens the stream was rewritten with (read next, last first).
    pending: Vec<Tok>,
    /// The piece's own scopes: innermost last.
    frames: Vec<HashMap<Key, Val>>,
    /// Names read from outside, as read.
    imports: HashMap<Key, Val>,
    run: PieceRun,
    conds: Vec<Cond>,
    mode: Mode,
    hlist: Vec<HItem>,
    parskip: i32,
}

/// Runs a piece: reads its text with the reaching definitions `env`
/// answers, and returns what it produced.
pub fn run_piece(text: &[char], env: &mut dyn Env) -> PieceRun {
    let mut r = Reader {
        env,
        text,
        pos: 0,
        state: State::N,
        pending: Vec::new(),
        frames: vec![HashMap::new()],
        imports: HashMap::new(),
        run: PieceRun::default(),
        conds: Vec::new(),
        mode: Mode::V,
        hlist: Vec::new(),
        parskip: 0,
    };
    let mut global = false;
    while let Some(t) = r.next_x() {
        r.run.steps += 1;
        let was_global = std::mem::take(&mut global);
        match &t {
            Tok::Char(cat, c) => r.char_tok(*cat, *c),
            Tok::Param(_) => r.err("Illegal parameter number in a paragraph".into()),
            Tok::Cs(_) | Tok::Active(_) => {
                let key = t.key().expect("a name");
                match r.meaning(&key) {
                    Meaning::Prim(Prim::Global) => global = true,
                    Meaning::Prim(p) => r.prim(p, was_global),
                    Meaning::Char(cat, c) => r.char_tok(cat, c),
                    Meaning::Undefined => r.err(format!("Undefined control sequence {t}")),
                    Meaning::Macro(_) => {}
                }
            }
        }
    }
    r.end_par();
    while r.frames.len() > 1 {
        r.frames.pop();
        r.err("Missing } inserted at the end of a piece".into());
    }
    while !r.conds.is_empty() {
        r.err("Incomplete \\if at the end of a piece".into());
        r.end_cond(0);
    }
    let mut exports: Vec<(Key, Ver, Val)> = r
        .frames
        .pop()
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, v.ver(), v))
        .collect();
    exports.sort_by(|a, b| a.0.cmp(&b.0));
    let mut run = r.run;
    run.exports = exports;
    run.para_hashes = run.paras.iter().map(|p| hash_of(&**p)).collect();
    run
}

impl Reader<'_> {
    fn err(&mut self, s: String) {
        self.run.errors.push(s);
    }

    // ------------------------------------------------------------ names

    /// The meaning of `key` here: the piece's own scopes, else the reaching
    /// definition from outside (read once, recorded with its version).
    fn get(&mut self, key: &Key) -> Val {
        for f in self.frames.iter().rev() {
            if let Some(v) = f.get(key) {
                return v.clone();
            }
        }
        if let Some(v) = self.imports.get(key) {
            return v.clone();
        }
        let v = self.env.reach(key);
        self.run.reads.push((key.clone(), v.ver()));
        self.imports.insert(key.clone(), v.clone());
        v
    }

    fn define(&mut self, key: Key, v: Val, global: bool) {
        if !self.conds.is_empty() {
            let before = self.get(&key).ver();
            for c in &mut self.conds {
                if !c.defined.iter().any(|(k, _)| *k == key) {
                    c.defined.push((key.clone(), before));
                }
            }
        }
        if global {
            for f in &mut self.frames {
                f.remove(&key);
            }
            self.frames[0].insert(key, v);
        } else {
            self.frames.last_mut().expect("a frame").insert(key, v);
        }
    }

    fn meaning(&mut self, key: &Key) -> Meaning {
        match self.get(key) {
            Val::Meaning(m) => m,
            _ => Meaning::Undefined,
        }
    }

    fn catcode(&mut self, c: char) -> u8 {
        match self.get(&Key::Cat(c)) {
            Val::Cat(k) => k,
            _ => 12,
        }
    }

    fn param(&mut self, p: Param) -> i32 {
        match self.get(&Key::Param(p)) {
            Val::Int(i) => i,
            _ => 0,
        }
    }

    fn font(&mut self) -> Arc<Font> {
        match self.get(&Key::Font) {
            Val::Font(f) => f,
            other => panic!("the font key holds {other:?}"),
        }
    }

    fn tok_meaning(&mut self, t: &Tok) -> Meaning {
        match t {
            Tok::Char(c, x) => Meaning::Char(*c, *x),
            _ => t.key().map_or(Meaning::Undefined, |k| self.meaning(&k)),
        }
    }

    // ------------------------------------------------------------ the stream

    fn back(&mut self, t: Tok) {
        self.pending.push(t);
    }

    /// Rewrites the stream: `toks`, then the rest.
    fn rewrite(&mut self, toks: Vec<Tok>) {
        self.pending.extend(toks.into_iter().rev());
    }

    /// The next token, unexpanded (tex.web 343-357, without `^^`).
    fn next_raw(&mut self) -> Option<Tok> {
        if let Some(t) = self.pending.pop() {
            return Some(t);
        }
        loop {
            let c = *self.text.get(self.pos)?;
            let cat = self.catcode(c);
            self.pos += 1;
            match cat {
                0 => {
                    let Some(&c2) = self.text.get(self.pos) else {
                        self.state = State::S;
                        return Some(Tok::Cs(Arc::from("")));
                    };
                    let cat2 = self.catcode(c2);
                    if cat2 == 11 {
                        let start = self.pos;
                        while self.pos < self.text.len() {
                            let c3 = self.text[self.pos];
                            if self.catcode(c3) != 11 {
                                break;
                            }
                            self.pos += 1;
                        }
                        let name: String = self.text[start..self.pos].iter().collect();
                        self.state = State::S;
                        return Some(Tok::Cs(Arc::from(name)));
                    }
                    self.pos += 1;
                    self.state = if cat2 == 10 { State::S } else { State::M };
                    return Some(Tok::Cs(Arc::from(c2.to_string())));
                }
                5 => {
                    let st = self.state;
                    self.state = State::N;
                    match st {
                        State::N => return Some(Tok::Cs(Arc::from("par"))),
                        State::M => return Some(Tok::Char(10, ' ')),
                        State::S => {}
                    }
                }
                10 => {
                    if self.state == State::M {
                        self.state = State::S;
                        return Some(Tok::Char(10, ' '));
                    }
                }
                14 => {
                    while self.pos < self.text.len() && self.text[self.pos] != '\r' {
                        self.pos += 1;
                    }
                    self.pos += 1;
                    self.state = State::N;
                }
                9 => {}
                15 => self.err("Text line contains an invalid character".into()),
                13 => {
                    self.state = State::M;
                    return Some(Tok::Active(c));
                }
                _ => {
                    self.state = State::M;
                    return Some(Tok::Char(cat, c));
                }
            }
        }
    }

    /// The next token, expanded: macro calls rewrite the stream,
    /// conditionals choose an arm.
    fn next_x(&mut self) -> Option<Tok> {
        loop {
            let t = self.next_raw()?;
            let Some(key) = t.key() else { return Some(t) };
            match self.meaning(&key) {
                Meaning::Macro(m) => self.call(&t, &m),
                Meaning::Prim(p) if p.expandable() => self.expand_prim(p),
                _ => return Some(t),
            }
        }
    }

    // ------------------------------------------------------------ macros

    /// A macro call: matches the arguments and rewrites the stream with the
    /// body (tex.web 389-399).
    fn call(&mut self, name: &Tok, m: &Macro) {
        let mut args: Vec<Vec<Tok>> = vec![Vec::new(); 10];
        let p = &m.params;
        let mut i = 0;
        while i < p.len() {
            if let Tok::Param(n) = p[i] {
                let mut j = i + 1;
                while j < p.len() && !matches!(p[j], Tok::Param(_)) {
                    j += 1;
                }
                let delim = &p[i + 1..j];
                args[usize::from(n)] = if delim.is_empty() {
                    self.undelimited()
                } else {
                    self.delimited(delim)
                };
                i = j;
            } else {
                match self.next_raw() {
                    Some(x) if x == p[i] => {}
                    other => {
                        self.err(format!("Use of {name} doesn't match its definition"));
                        if let Some(x) = other {
                            self.back(x);
                        }
                        return;
                    }
                }
                i += 1;
            }
        }
        let mut out = Vec::with_capacity(m.body.len());
        for t in &m.body {
            match t {
                Tok::Param(n) => out.extend(args[usize::from(*n)].iter().cloned()),
                _ => out.push(t.clone()),
            }
        }
        self.rewrite(out);
    }

    fn undelimited(&mut self) -> Vec<Tok> {
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning an argument".into());
                return Vec::new();
            };
            match t.cat() {
                Some(10) => {}
                Some(1) => return self.group_body(),
                _ => return vec![t],
            }
        }
    }

    /// The tokens up to the brace closing an open group (not included).
    fn group_body(&mut self) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(t) = self.next_raw() {
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
                _ => {}
            }
            out.push(t);
        }
        self.err("File ended while scanning an argument".into());
        out
    }

    fn delimited(&mut self, delim: &[Tok]) -> Vec<Tok> {
        let mut buf: Vec<Tok> = Vec::new();
        let mut depth = 0;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a delimited argument".into());
                break;
            };
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => depth -= 1,
                _ => {}
            }
            buf.push(t);
            if depth == 0 && buf.len() >= delim.len() && buf[buf.len() - delim.len()..] == *delim {
                buf.truncate(buf.len() - delim.len());
                break;
            }
        }
        // (one pair of braces around the whole argument goes)
        if buf.len() >= 2 && buf[0].cat() == Some(1) && buf[buf.len() - 1].cat() == Some(2) {
            let mut d = 0;
            let mut whole = true;
            for (k, t) in buf.iter().enumerate() {
                match t.cat() {
                    Some(1) => d += 1,
                    Some(2) => d -= 1,
                    _ => {}
                }
                if d == 0 && k + 1 < buf.len() {
                    whole = false;
                    break;
                }
            }
            if whole {
                buf.pop();
                buf.remove(0);
            }
        }
        buf
    }

    // ------------------------------------------------------------ conditionals

    fn expand_prim(&mut self, p: Prim) {
        match p {
            Prim::Ifx => {
                let a = self.next_raw();
                let b = self.next_raw();
                let t = match (a, b) {
                    (Some(a), Some(b)) => self.tok_meaning(&a) == self.tok_meaning(&b),
                    _ => false,
                };
                self.begin_cond(t);
            }
            Prim::Iftrue => self.begin_cond(true),
            Prim::Iffalse => self.begin_cond(false),
            Prim::Else => {
                if self.conds.is_empty() {
                    self.err("Extra \\else".into());
                    return;
                }
                let (n, _) = self.skip(false);
                self.end_cond(n);
            }
            Prim::Fi => {
                if self.conds.is_empty() {
                    self.err("Extra \\fi".into());
                    return;
                }
                self.end_cond(0);
            }
            _ => {}
        }
    }

    /// The test decided: read the taken arm (skip the other).
    fn begin_cond(&mut self, test: bool) {
        if test {
            self.conds.push(Cond { test, defined: Vec::new(), skipped: 0 });
            return;
        }
        let (n, at_else) = self.skip(true);
        if at_else {
            self.conds.push(Cond { test, defined: Vec::new(), skipped: n });
        } else {
            self.run.phis.push(Phi { test, skipped: n, keys: Vec::new() });
        }
    }

    /// Skips an arm: up to the matching `\fi`, or `\else` if asked
    /// (tex.web 494). Returns the tokens skipped and whether it stopped at
    /// `\else`.
    fn skip(&mut self, stop_at_else: bool) -> (usize, bool) {
        let mut depth = 0usize;
        let mut n = 0;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("Incomplete \\if".into());
                return (n, false);
            };
            n += 1;
            let Some(k) = t.key() else { continue };
            match self.meaning(&k) {
                Meaning::Prim(Prim::Ifx | Prim::Iftrue | Prim::Iffalse) => depth += 1,
                Meaning::Prim(Prim::Fi) => {
                    if depth == 0 {
                        return (n, false);
                    }
                    depth -= 1;
                }
                Meaning::Prim(Prim::Else) if depth == 0 && stop_at_else => return (n, true),
                _ => {}
            }
        }
    }

    /// `\fi`: the explicit φ for each key the arm defined.
    fn end_cond(&mut self, skipped: usize) {
        let Some(c) = self.conds.pop() else { return };
        let keys = c
            .defined
            .into_iter()
            .map(|(k, before)| {
                let after = self.get(&k).ver();
                (k, after, before)
            })
            .collect();
        self.run.phis.push(Phi { test: c.test, skipped: c.skipped + skipped, keys });
    }

    // ------------------------------------------------------------ commands

    fn char_tok(&mut self, cat: u8, c: char) {
        match cat {
            1 => self.frames.push(HashMap::new()),
            2 => {
                if self.frames.len() > 1 {
                    self.frames.pop();
                } else {
                    self.err("Too many }'s".into());
                }
            }
            10 => {
                if self.mode == Mode::H {
                    self.space();
                }
            }
            11 | 12 => self.letter(c),
            _ => self.err(format!("Character {c} of category {cat} is not supported yet")),
        }
    }

    fn prim(&mut self, p: Prim, global: bool) {
        match p {
            Prim::Def => self.def(global),
            Prim::Gdef => self.def(true),
            Prim::Let => self.let_(global),
            Prim::Catcode => self.catcode_assign(global),
            Prim::Param(q) => self.param_assign(q, global),
            Prim::Par => self.end_par(),
            Prim::Indent => {
                if self.mode == Mode::V {
                    self.begin_par(true);
                } else {
                    let w = self.param(Param::Parindent);
                    self.hlist.push(HItem::Box { w, h: 0, d: 0 });
                }
            }
            Prim::Noindent => {
                if self.mode == Mode::V {
                    self.begin_par(false);
                }
            }
            Prim::Space => {
                if self.mode == Mode::V {
                    self.begin_par(true);
                }
                self.space();
            }
            Prim::Relax | Prim::Global => {}
            Prim::Ifx | Prim::Iftrue | Prim::Iffalse | Prim::Else | Prim::Fi => {}
        }
    }

    fn def(&mut self, global: bool) {
        let Some(name) = self.next_raw() else { return };
        let Some(key) = name.key() else {
            self.err("Missing control sequence inserted".into());
            return;
        };
        let mut params = Vec::new();
        loop {
            let Some(t) = self.next_raw() else { return };
            match t.cat() {
                Some(1) => break,
                Some(6) => match self.next_raw() {
                    Some(Tok::Char(_, d)) if d.is_ascii_digit() && d != '0' => {
                        params.push(Tok::Param(d as u8 - b'0'));
                    }
                    _ => self.err("Parameters must be numbered consecutively".into()),
                },
                _ => params.push(t),
            }
        }
        let mut body = Vec::new();
        let mut depth = 1;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a definition".into());
                break;
            };
            match t.cat() {
                Some(1) => {
                    depth += 1;
                    body.push(t);
                }
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    body.push(t);
                }
                Some(6) => match self.next_raw() {
                    Some(Tok::Char(_, d)) if d.is_ascii_digit() && d != '0' => {
                        body.push(Tok::Param(d as u8 - b'0'));
                    }
                    Some(x) if x.cat() == Some(6) => body.push(x),
                    other => {
                        self.err("Illegal parameter number in a definition".into());
                        if let Some(x) = other {
                            body.push(x);
                        }
                    }
                },
                _ => body.push(t),
            }
        }
        self.define(key, Val::Meaning(Meaning::Macro(Arc::new(Macro { params, body }))), global);
    }

    fn let_(&mut self, global: bool) {
        let Some(name) = self.next_raw() else { return };
        let Some(key) = name.key() else {
            self.err("Missing control sequence inserted".into());
            return;
        };
        let mut t = self.next_raw();
        while matches!(t, Some(Tok::Char(10, _))) {
            t = self.next_raw();
        }
        if matches!(t, Some(Tok::Char(12, '='))) {
            t = self.next_raw();
            if matches!(t, Some(Tok::Char(10, _))) {
                t = self.next_raw();
            }
        }
        let Some(t) = t else { return };
        let m = self.tok_meaning(&t);
        self.define(key, Val::Meaning(m), global);
    }

    fn catcode_assign(&mut self, global: bool) {
        let c = self.scan_int();
        self.optional_equals();
        let v = self.scan_int();
        let Some(ch) = char::from_u32(c as u32) else {
            self.err("Bad character code".into());
            return;
        };
        if !(0..=15).contains(&v) {
            self.err("Invalid code".into());
            return;
        }
        self.define(Key::Cat(ch), Val::Cat(v as u8), global);
    }

    fn param_assign(&mut self, p: Param, global: bool) {
        self.optional_equals();
        let v = if p.is_int() { self.scan_int() } else { self.scan_dimen() };
        self.define(Key::Param(p), Val::Int(v), global);
    }

    // ------------------------------------------------------------ scanning

    fn skip_spaces_x(&mut self) -> Option<Tok> {
        loop {
            let t = self.next_x()?;
            if t.cat() != Some(10) {
                return Some(t);
            }
        }
    }

    fn optional_equals(&mut self) {
        if let Some(t) = self.skip_spaces_x() {
            if !t.is_char(12, '=') {
                self.back(t);
            }
        }
    }

    fn optional_space(&mut self) {
        if let Some(t) = self.next_x() {
            if t.cat() != Some(10) {
                self.back(t);
            }
        }
    }

    fn signs(&mut self) -> (bool, Option<Tok>) {
        let mut neg = false;
        loop {
            let t = self.skip_spaces_x();
            match &t {
                Some(x) if x.is_char(12, '-') => neg = !neg,
                Some(x) if x.is_char(12, '+') => {}
                _ => return (neg, t),
            }
        }
    }

    /// An internal quantity a token names: (value, is an integer).
    fn internal(&mut self, t: &Tok) -> Option<(i32, bool)> {
        let k = t.key()?;
        match self.meaning(&k) {
            Meaning::Prim(Prim::Param(p)) => Some((self.param(p), p.is_int())),
            _ => None,
        }
    }

    /// An integer (tex.web 440-447).
    fn scan_int(&mut self) -> i32 {
        let (neg, t) = self.signs();
        let Some(t) = t else {
            self.err("Missing number, treated as zero".into());
            return 0;
        };
        let v: i64 = if t.is_char(12, '`') {
            let c = match self.next_raw() {
                Some(Tok::Char(_, c) | Tok::Active(c)) => i64::from(u32::from(c)),
                Some(Tok::Cs(n)) => {
                    let mut it = n.chars();
                    if let (Some(c), None) = (it.next(), it.next()) {
                        i64::from(u32::from(c))
                    } else {
                        self.err("Improper alphabetic constant".into());
                        0
                    }
                }
                _ => 0,
            };
            self.optional_space();
            c
        } else if let Some((v, _)) = self.internal(&t) {
            i64::from(v)
        } else {
            let (radix, mut first) = if t.is_char(12, '\'') {
                (8u32, None)
            } else if t.is_char(12, '"') {
                (16, None)
            } else {
                (10, Some(t))
            };
            let mut v: i64 = 0;
            let mut any = false;
            loop {
                let x = match first.take() {
                    Some(x) => Some(x),
                    None => self.next_x(),
                };
                let Some(x) = x else { break };
                let d = match &x {
                    Tok::Char(12, c) if c.is_ascii_digit() => c.to_digit(10),
                    Tok::Char(11 | 12, c) if radix == 16 && ('A'..='F').contains(c) => c.to_digit(16),
                    _ => None,
                };
                match d {
                    Some(d) if d < radix => {
                        v = (v * i64::from(radix) + i64::from(d)).min(i64::from(i32::MAX));
                        any = true;
                    }
                    _ => {
                        if x.cat() != Some(10) {
                            self.back(x);
                        }
                        break;
                    }
                }
            }
            if !any {
                self.err("Missing number, treated as zero".into());
            }
            v
        };
        let v = v as i32;
        if neg { -v } else { v }
    }

    /// A keyword, letters of either case and category (tex.web 407).
    fn keyword(&mut self, kw: &str) -> bool {
        let kws: Vec<char> = kw.chars().collect();
        let mut buf = Vec::new();
        let mut i = 0;
        while i < kws.len() {
            let Some(t) = self.next_x() else {
                for x in buf.into_iter().rev() {
                    self.back(x);
                }
                return false;
            };
            let hit = matches!(&t, Tok::Char(_, c) if c.to_ascii_lowercase() == kws[i]);
            if hit {
                buf.push(t);
                i += 1;
            } else if buf.is_empty() && t.cat() == Some(10) {
            } else {
                self.back(t);
                for x in buf.into_iter().rev() {
                    self.back(x);
                }
                return false;
            }
        }
        true
    }

    /// A dimension (tex.web 448-461).
    fn scan_dimen(&mut self) -> i32 {
        let (neg, t) = self.signs();
        let Some(t) = t else {
            self.err("Missing number, treated as zero".into());
            return 0;
        };
        if let Some((v, is_int)) = self.internal(&t) {
            if !is_int {
                return if neg { -v } else { v };
            }
            return self.units(neg, v, 0);
        }
        let mut ip: i64 = 0;
        let mut digits: Vec<u8> = Vec::new();
        let mut point = false;
        let mut any = false;
        let mut first = Some(t);
        loop {
            let x = match first.take() {
                Some(x) => Some(x),
                None => self.next_x(),
            };
            let Some(x) = x else { break };
            match &x {
                Tok::Char(12, c) if c.is_ascii_digit() => {
                    let d = c.to_digit(10).unwrap_or(0);
                    any = true;
                    if point {
                        if digits.len() < 17 {
                            digits.push(d as u8);
                        }
                    } else {
                        ip = (ip * 10 + i64::from(d)).min(i64::from(i32::MAX));
                    }
                }
                Tok::Char(12, '.' | ',') if !point => point = true,
                _ => {
                    self.back(x);
                    break;
                }
            }
        }
        if !any && !point {
            self.err("Missing number, treated as zero".into());
        }
        let f = if digits.is_empty() { 0 } else { round_decimals(&digits) };
        self.units(neg, ip as i32, f)
    }

    fn units(&mut self, neg: bool, mut v: i32, mut f: i32) -> i32 {
        let font_unit = if self.keyword("em") {
            Some(6)
        } else if self.keyword("ex") {
            Some(5)
        } else {
            None
        };
        if let Some(n) = font_unit {
            let q = self.font().param(n);
            let r = i64::from(v) * i64::from(q) + i64::from(xn_over_d(q, f, 65536).0);
            self.optional_space();
            let r = crate::arith::clamp(r);
            return if neg { -r } else { r };
        }
        let _ = self.keyword("true");
        let (num, den) = if self.keyword("pt") {
            (1, 1)
        } else if self.keyword("in") {
            (7227, 100)
        } else if self.keyword("pc") {
            (12, 1)
        } else if self.keyword("cm") {
            (7227, 254)
        } else if self.keyword("mm") {
            (7227, 2540)
        } else if self.keyword("bp") {
            (7227, 7200)
        } else if self.keyword("dd") {
            (1238, 1157)
        } else if self.keyword("cc") {
            (14856, 1157)
        } else if self.keyword("sp") {
            self.optional_space();
            return if neg { -v } else { v };
        } else {
            self.err("Illegal unit of measure (pt inserted)".into());
            (1, 1)
        };
        if num != 1 || den != 1 {
            let (q, rem) = xn_over_d(v, num, den);
            f = ((i64::from(num) * i64::from(f) + 65536 * i64::from(rem)) / i64::from(den)) as i32;
            v = q + f / 65536;
            f %= 65536;
        }
        if v >= 0o40000 {
            self.err("Dimension too large".into());
            v = 0o37777;
            f = 0o177_777;
        }
        let r = v * 65536 + f;
        self.optional_space();
        if neg { -r } else { r }
    }

    // ------------------------------------------------------------ material

    fn begin_par(&mut self, indent: bool) {
        self.mode = Mode::H;
        self.parskip = self.param(Param::Parskip);
        self.hlist.clear();
        if indent {
            let w = self.param(Param::Parindent);
            self.hlist.push(HItem::Box { w, h: 0, d: 0 });
        }
    }

    fn letter(&mut self, c: char) {
        if self.mode == Mode::V {
            self.begin_par(true);
        }
        let font = self.font();
        match font.char(u32::from(c)) {
            Some(d) if u32::from(c) < 256 => {
                self.hlist.push(HItem::Char { c: c as u8, w: d.w, h: d.h, d: d.d });
            }
            _ => self.err(format!("Missing character {c} in font {}", font.name)),
        }
    }

    fn space(&mut self) {
        let f = self.font();
        self.hlist.push(HItem::Glue { w: f.param(2), st: f.param(3), st_ord: 0, sh: f.param(4), sh_ord: 0 });
    }

    /// `\par`: the paragraph, with the parameters line breaking and pages
    /// read (tex.web 1096).
    fn end_par(&mut self) {
        if self.mode != Mode::H {
            return;
        }
        self.mode = Mode::V;
        if matches!(self.hlist.last(), Some(HItem::Glue { .. })) {
            self.hlist.pop();
        }
        self.hlist.push(HItem::Penalty(10000));
        self.hlist.push(HItem::Glue { w: 0, st: 65536, st_ord: 1, sh: 0, sh_ord: 0 });
        let para = Para {
            hlist: std::mem::take(&mut self.hlist),
            hsize: self.param(Param::Hsize),
            tolerance: self.param(Param::Tolerance),
            linepenalty: self.param(Param::Linepenalty),
            baselineskip: self.param(Param::Baselineskip),
            parskip: self.parskip,
            vsize: self.param(Param::Vsize),
            topskip: self.param(Param::Topskip),
            maxdepth: self.param(Param::Maxdepth),
        };
        self.run.paras.push(Arc::new(para));
    }
}
