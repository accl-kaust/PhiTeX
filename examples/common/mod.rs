//! A small example language built on `defuse`, shared by the examples, the
//! benchmark and the tests. One statement per line at top level.
//!
//! ```text
//! x = 1 + y                 assignment (local to the enclosing scope)
//! global g = 2              global assignment
//! p = {x: 1, w: 10, c: 3}   record: fields p.x, p.w, p.c
//! r = layout(p)             reads only p.x and p.w
//! { a = 1  b = a }          block (unfolds into begin .. end)
//! begin / end               scope opened and closed by different statements
//! if c { .. } else { .. }   only the selected branch is created
//! f = fn(a) { t = a * 2 return t + 1 }   lexical function value
//! y = f(2)                  memoized application
//! proc p(a) { global q = a }             procedure (text)
//! do p(5)                   unfolds the body at the call site
//! eval "z = 4"              runtime text, unfolded in place
//! sumnext s 2               claims the next 2 lines; s = sum of them
//! listto l .. endlist       claims up to `endlist`; l = their values
//! listuntil l v .. mark k   claims up to `mark <value of v>` (pending)
//! meaning put = "set"       what `~put` does is a field value
//! ~put x 5                  defines x (set/double), shows, or emits
//! emit e                    contributes to the `out` accumulator
//! page                      reads `out` since the previous page
//! loop later = 0            loop-back field (later = .. publishes)
//! print x
//! ```
#![allow(dead_code, clippy::many_single_char_names)] // AST code reads best with short names

pub mod interp;

use std::sync::{Arc, OnceLock, RwLock};

use defuse::{
    Claim, Ctx, Effect, Engine, Executor, Field, FxHashMap, Locality, MayDefine, NodeId, OpId, Operation, RunResult,
    SeqMonoid, Stop, UnitId, Value, ValueId, Values,
};

// ------------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    Str(String),
    Sym(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() || c == ';' {
            i += 1;
        } else if c == '#' {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            let s = i;
            while i < cs.len() && cs[i].is_ascii_digit() {
                i += 1;
            }
            let t: String = cs[s..i].iter().collect();
            out.push(Tok::Int(t.parse().map_err(|_| "bad int".to_string())?));
        } else if c.is_alphabetic() || c == '_' {
            let s = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(cs[s..i].iter().collect()));
        } else if c == '"' {
            let s = i + 1;
            i += 1;
            while i < cs.len() && cs[i] != '"' {
                i += 1;
            }
            if i >= cs.len() {
                return Err("unterminated string".into());
            }
            out.push(Tok::Str(cs[s..i].iter().collect()));
            i += 1;
        } else if c == '=' && cs.get(i + 1) == Some(&'=') {
            out.push(Tok::Sym("=="));
            i += 2;
        } else {
            let sym = match c {
                '=' => "=",
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '<' => "<",
                '(' => "(",
                ')' => ")",
                '{' => "{",
                '}' => "}",
                ',' => ",",
                ':' => ":",
                '.' => ".",
                '~' => "~",
                _ => return Err(format!("unexpected {c:?}")),
            };
            out.push(Tok::Sym(sym));
            i += 1;
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ parser

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    v: &'a Values,
}

type PResult<T> = Result<T, String>;

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.pos + k)
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn is_kw(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(x)) if x == s)
    }

    fn eat(&mut self, s: &str) -> PResult<()> {
        if self.is_sym(s) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected {s:?} at token {}", self.pos))
        }
    }

    fn ident(&mut self) -> PResult<String> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            _ => Err(format!("expected identifier at token {}", self.pos)),
        }
    }

    fn int(&mut self) -> PResult<i64> {
        match self.peek() {
            Some(Tok::Int(i)) => {
                let i = *i;
                self.pos += 1;
                Ok(i)
            }
            _ => Err(format!("expected integer at token {}", self.pos)),
        }
    }

    /// `a` or `a.b`.
    fn name(&mut self) -> PResult<String> {
        let mut s = self.ident()?;
        if self.is_sym(".") {
            self.pos += 1;
            s.push('.');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }

    fn l(&self, xs: Vec<ValueId>) -> ValueId {
        self.v.list(xs)
    }

    fn s(&self, x: &str) -> ValueId {
        self.v.str(x)
    }

    fn tagged(&self, tag: &str, mut rest: Vec<ValueId>) -> ValueId {
        rest.insert(0, self.s(tag));
        self.l(rest)
    }

    fn stmts_until_brace(&mut self) -> PResult<Vec<ValueId>> {
        let mut out = Vec::new();
        while !self.is_sym("}") {
            if self.peek().is_none() {
                return Err("missing }".into());
            }
            out.push(self.stmt()?);
        }
        Ok(out)
    }

    fn block(&mut self) -> PResult<ValueId> {
        self.eat("{")?;
        let ss = self.stmts_until_brace()?;
        self.eat("}")?;
        Ok(self.l(ss))
    }

    fn params(&mut self) -> PResult<ValueId> {
        self.eat("(")?;
        let mut ps = Vec::new();
        while !self.is_sym(")") {
            let p = self.ident()?;
            ps.push(self.s(&p));
            if self.is_sym(",") {
                self.pos += 1;
            }
        }
        self.eat(")")?;
        Ok(self.l(ps))
    }

    #[allow(clippy::too_many_lines)]
    fn stmt(&mut self) -> PResult<ValueId> {
        if self.is_sym("{") {
            let b = self.block()?;
            return Ok(self.tagged("block", vec![b]));
        }
        if self.is_sym("~") {
            self.pos += 1;
            let n = self.ident()?;
            let t = self.ident()?;
            let e = self.expr()?;
            return Ok(self.tagged("invoke", vec![self.s(&n), self.s(&t), e]));
        }
        if let Some(Tok::Ident(k)) = self.peek().cloned() {
            let simple = |p: &mut Self, tag: &str| {
                p.pos += 1;
                Ok(p.tagged(tag, vec![]))
            };
            match k.as_str() {
                "begin" => return simple(self, "begin"),
                "end" => return simple(self, "end"),
                "endlist" => return simple(self, "endlist"),
                "page" => return simple(self, "page"),
                "if" => {
                    self.pos += 1;
                    let c = self.expr()?;
                    let t = self.block()?;
                    let e = if self.is_kw("else") {
                        self.pos += 1;
                        self.block()?
                    } else {
                        self.l(vec![])
                    };
                    return Ok(self.tagged("if", vec![c, t, e]));
                }
                "eval" | "print" | "emit" => {
                    self.pos += 1;
                    let e = self.expr()?;
                    return Ok(self.tagged(&k, vec![e]));
                }
                "loop" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    self.eat("=")?;
                    let e = self.expr()?;
                    return Ok(self.tagged("loop", vec![self.s(&n), e]));
                }
                "sumnext" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    let c = self.int()?;
                    return Ok(self.tagged("sumnext", vec![self.s(&n), self.v.int(c)]));
                }
                "listto" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    return Ok(self.tagged("listto", vec![self.s(&n)]));
                }
                "listuntil" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    let var = self.ident()?;
                    return Ok(self.tagged("listuntil", vec![self.s(&n), self.s(&var)]));
                }
                "mark" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    return Ok(self.tagged("mark", vec![self.s(&n)]));
                }
                "meaning" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    self.eat("=")?;
                    let e = self.expr()?;
                    let g = self.v.bool(false);
                    return Ok(self.tagged("assign", vec![self.s(&format!("meaning.{n}")), e, g]));
                }
                "proc" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    let ps = self.params()?;
                    let body = self.block()?;
                    return Ok(self.tagged("proc", vec![self.s(&n), ps, body]));
                }
                "do" => {
                    self.pos += 1;
                    let n = self.ident()?;
                    self.eat("(")?;
                    let mut parts = vec![self.s(&n)];
                    while !self.is_sym(")") {
                        parts.push(self.expr()?);
                        if self.is_sym(",") {
                            self.pos += 1;
                        }
                    }
                    self.eat(")")?;
                    return Ok(self.tagged("do", parts));
                }
                _ => {}
            }
        }
        let global = self.is_kw("global");
        if global {
            self.pos += 1;
        }
        let is_assign = matches!(self.peek(), Some(Tok::Ident(_)))
            && (matches!(self.peek_at(1), Some(Tok::Sym("=")))
                || (matches!(self.peek_at(1), Some(Tok::Sym("."))) && matches!(self.peek_at(3), Some(Tok::Sym("=")))));
        if is_assign {
            let n = self.name()?;
            self.eat("=")?;
            let g = self.v.bool(global);
            if self.is_sym("{") {
                self.pos += 1;
                let mut fields = Vec::new();
                while !self.is_sym("}") {
                    let f = self.ident()?;
                    self.eat(":")?;
                    let e = self.expr()?;
                    fields.push(self.l(vec![self.s(&f), e]));
                    if self.is_sym(",") {
                        self.pos += 1;
                    }
                }
                self.eat("}")?;
                let fs = self.l(fields);
                return Ok(self.tagged("record", vec![self.s(&n), fs, g]));
            }
            let e = self.expr()?;
            return Ok(self.tagged("assign", vec![self.s(&n), e, g]));
        }
        if global {
            return Err("expected assignment after global".into());
        }
        let e = self.expr()?;
        Ok(self.tagged("expr", vec![e]))
    }

    fn expr(&mut self) -> PResult<ValueId> {
        let mut a = self.sum()?;
        while self.is_sym("<") || self.is_sym("==") {
            let op = if self.is_sym("<") { "<" } else { "==" };
            self.pos += 1;
            let b = self.sum()?;
            a = self.tagged("op", vec![self.s(op), a, b]);
        }
        Ok(a)
    }

    fn sum(&mut self) -> PResult<ValueId> {
        let mut a = self.product()?;
        while self.is_sym("+") || self.is_sym("-") {
            let op = if self.is_sym("+") { "+" } else { "-" };
            self.pos += 1;
            let b = self.product()?;
            a = self.tagged("op", vec![self.s(op), a, b]);
        }
        Ok(a)
    }

    fn product(&mut self) -> PResult<ValueId> {
        let mut a = self.call()?;
        while self.is_sym("*") {
            self.pos += 1;
            let b = self.call()?;
            a = self.tagged("op", vec![self.s("*"), a, b]);
        }
        Ok(a)
    }

    fn call(&mut self) -> PResult<ValueId> {
        let mut f = self.atom()?;
        while self.is_sym("(") {
            self.pos += 1;
            let mut parts = vec![f];
            while !self.is_sym(")") {
                parts.push(self.expr()?);
                if self.is_sym(",") {
                    self.pos += 1;
                }
            }
            self.eat(")")?;
            f = self.tagged("call", parts);
        }
        Ok(f)
    }

    fn atom(&mut self) -> PResult<ValueId> {
        match self.peek().cloned() {
            Some(Tok::Int(i)) => {
                self.pos += 1;
                Ok(self.v.int(i))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(self.tagged("str", vec![self.s(&s)]))
            }
            Some(Tok::Sym("(")) => {
                self.pos += 1;
                let e = self.expr()?;
                self.eat(")")?;
                Ok(e)
            }
            Some(Tok::Sym("-")) => {
                self.pos += 1;
                let e = self.atom()?;
                let z = self.v.int(0);
                Ok(self.tagged("op", vec![self.s("-"), z, e]))
            }
            Some(Tok::Ident(k)) if k == "fn" => {
                self.pos += 1;
                let ps = self.params()?;
                self.eat("{")?;
                let mut body = Vec::new();
                while !self.is_kw("return") {
                    if self.peek().is_none() || self.is_sym("}") {
                        return Err("function body needs return".into());
                    }
                    body.push(self.stmt()?);
                }
                self.pos += 1;
                let ret = self.expr()?;
                self.eat("}")?;
                let body = self.l(body);
                Ok(self.tagged("fn", vec![ps, body, ret]))
            }
            Some(Tok::Ident(k)) if k == "layout" => {
                self.pos += 1;
                self.eat("(")?;
                let n = self.ident()?;
                self.eat(")")?;
                Ok(self.tagged("layout", vec![self.s(&n)]))
            }
            Some(Tok::Ident(k)) if k == "true" || k == "false" => {
                self.pos += 1;
                Ok(self.tagged("lit", vec![self.v.bool(k == "true")]))
            }
            Some(Tok::Ident(_)) => {
                let n = self.name()?;
                Ok(self.tagged("var", vec![self.s(&n)]))
            }
            t => Err(format!("unexpected {t:?}")),
        }
    }
}

/// Parses program text into statement ASTs.
pub fn parse(v: &Values, src: &str) -> Result<Vec<ValueId>, String> {
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
        v,
    };
    let mut out = Vec::new();
    while p.peek().is_some() {
        out.push(p.stmt()?);
    }
    Ok(out)
}

/// Parses one statement; a parse error becomes an `error` statement.
pub fn parse_stmt(v: &Values, src: &str) -> ValueId {
    match parse(v, src) {
        Ok(mut ss) if ss.len() == 1 => ss.pop().expect("one"),
        Ok(ss) => v.list(vec![v.str("seq"), v.list(ss)]),
        Err(e) => v.list(vec![v.str("error"), v.str(&e)]),
    }
}

// --------------------------------------------------------------- AST access

fn items(v: &Values, id: ValueId) -> Vec<ValueId> {
    match &*v.get(id) {
        Value::List(xs) => xs.to_vec(),
        _ => Vec::new(),
    }
}

/// Procedures are named in their own namespace.
fn proc_field(v: &Values, name: ValueId) -> Field {
    v.field(&format!("proc.{}", text(v, name)))
}

fn has_proc(v: &Values, id: ValueId) -> bool {
    has_tag(v, id, &["proc"])
}

fn has_tag(v: &Values, id: ValueId, tags: &[&str]) -> bool {
    let xs = items(v, id);
    xs.first().is_some_and(|&t| tags.contains(&text(v, t).as_str())) || xs.iter().any(|&x| has_tag(v, x, tags))
}

/// Statements whose scope effect or definitions only show at run time.
const OPAQUE: [&str; 6] = ["begin", "end", "do", "eval", "invoke", "seq"];

/// Every procedure declared anywhere in a program, by name: what each
/// declaration's body may define visibly after a `do` (`None`: unknowable).
/// `tracked` is set once a [`Program`] maintains it; until then `do` stays
/// unbounded.
#[derive(Default)]
pub struct Procs {
    tracked: bool,
    defs: FxHashMap<String, Vec<Option<Vec<String>>>>,
}

pub type ProcTable = Arc<RwLock<Procs>>;

impl Procs {
    /// The names a `do NAME` may define: the union over its declarations.
    fn union(&self, name: &str) -> Option<Vec<String>> {
        let mut out = Vec::new();
        for e in self.defs.get(name).into_iter().flatten() {
            out.extend(e.clone()?);
        }
        out.sort();
        out.dedup();
        Some(out)
    }

    fn add(&mut self, decls: Vec<(String, Option<Vec<String>>)>) {
        for (n, e) in decls {
            self.defs.entry(n).or_default().push(e);
        }
    }

    fn remove(&mut self, decls: &[(String, Option<Vec<String>>)]) {
        for (n, e) in decls {
            let Some(es) = self.defs.get_mut(n) else { continue };
            if let Some(i) = es.iter().position(|x| x == e) {
                es.swap_remove(i);
            }
            if es.is_empty() {
                self.defs.remove(n);
            }
        }
    }
}

/// The procedure declarations in `ast`, with what each body may export.
fn proc_decls(v: &Values, ast: ValueId, out: &mut Vec<(String, Option<Vec<String>>)>) {
    let (t, a) = tag(v, ast);
    if t == "proc" {
        let body = a[2];
        // The body unfolds inside its own scope: only globals escape, unless
        // it opens, closes or unfolds further.
        let e = if has_tag(v, body, &OPAQUE) {
            None
        } else {
            exports(v, &items(v, body))
        };
        out.push((text(v, a[0]), e));
    }
    for x in items(v, ast) {
        proc_decls(v, x, out);
    }
}

/// The literal text an expression's value starts with, and whether that is
/// all of it (string literals joined by `+`).
fn literal_prefix(v: &Values, e: ValueId) -> (String, bool) {
    let (t, a) = tag(v, e);
    match t.as_str() {
        "str" => (text(v, a[0]), true),
        "op" if text(v, a[0]) == "+" => {
            let (l, full) = literal_prefix(v, a[1]);
            if !full {
                return (l, false);
            }
            let (r, full) = literal_prefix(v, a[2]);
            (l + &r, full)
        }
        _ => (String::new(), false),
    }
}

const KEYWORDS: [&str; 21] = [
    "begin",
    "end",
    "endlist",
    "page",
    "if",
    "else",
    "eval",
    "print",
    "emit",
    "loop",
    "sumnext",
    "listto",
    "listuntil",
    "mark",
    "meaning",
    "proc",
    "do",
    "global",
    "return",
    "fn",
    "seq",
];

/// What `eval TEXT` with a computed part may define, from the literal text
/// before the first computed piece: names starting with its first token.
/// Advisory (the computed rest could start more statements; the engine
/// re-binds if it does).
fn eval_prefix(v: &Values, e: ValueId) -> MayDefine {
    let (p, _) = literal_prefix(v, e);
    let p = p.trim_start();
    let id: String = p
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let complete = id.len() < p.len();
    let keyword = if complete {
        KEYWORDS.contains(&id.as_str())
    } else {
        KEYWORDS.iter().any(|k| k.starts_with(id.as_str()))
    };
    if id.is_empty() || keyword || id.starts_with(|c: char| c.is_ascii_digit()) {
        return MayDefine::Any;
    }
    // `id = {..}` also defines `id.x`: a prefix covers both.
    MayDefine::Prefixes(vec![id])
}

/// What a literal `eval "TEXT"` may define, read statically: `None` if its
/// text is not a literal, or opens, closes or unfolds further.
fn eval_literal(v: &Values, expr: ValueId) -> Option<Vec<String>> {
    let (t, a) = tag(v, expr);
    if t != "str" {
        return None;
    }
    let Ok(ss) = parse(v, &text(v, a[0])) else {
        return Some(Vec::new()); // an error: defines nothing
    };
    if ss.iter().any(|&s| has_proc(v, s)) {
        return Some(Vec::new());
    }
    if ss.iter().any(|&s| has_tag(v, s, &OPAQUE)) {
        return None;
    }
    // No wrapper scope: its top-level assignments define in place.
    let mut out = exports(v, &ss)?;
    for &s in &ss {
        let (t, a) = tag(v, s);
        if t == "assign" || t == "record" {
            out.push(text(v, a[0]));
        }
    }
    Some(out)
}

fn text(v: &Values, id: ValueId) -> String {
    match &*v.get(id) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
}

fn tag(v: &Values, id: ValueId) -> (String, Vec<ValueId>) {
    let xs = items(v, id);
    match xs.split_first() {
        Some((t, rest)) => (text(v, *t), rest.to_vec()),
        None => (String::new(), Vec::new()),
    }
}

fn truthy(v: &Values, id: ValueId) -> bool {
    match *v.get(id) {
        Value::Bool(b) => b,
        Value::Int(i) => i != 0,
        _ => false,
    }
}

/// Names that statements may define visibly after them: `Some(globals)`, or
/// `None` if unknowable from the text (runtime code).
fn exports(v: &Values, stmts: &[ValueId]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for &s in stmts {
        let (t, a) = tag(v, s);
        match t.as_str() {
            "assign" | "record" if truthy(v, a[2]) => out.push(text(v, a[0])),
            "eval" | "do" | "invoke" | "seq" => return None,
            "block" => out.extend(exports(v, &items(v, a[0]))?),
            "if" => {
                out.extend(exports(v, &items(v, a[1]))?);
                out.extend(exports(v, &items(v, a[2]))?);
            }
            _ => {}
        }
    }
    Some(out)
}

fn free_vars(v: &Values, e: ValueId, bound: &mut Vec<String>, out: &mut Vec<String>) {
    if let Value::Int(_) = *v.get(e) {
        return;
    }
    let (t, a) = tag(v, e);
    let add = |n: String, bound: &Vec<String>, out: &mut Vec<String>| {
        if !bound.contains(&n) && !out.contains(&n) {
            out.push(n);
        }
    };
    match t.as_str() {
        "var" => add(text(v, a[0]), bound, out),
        "layout" => {
            let n = text(v, a[0]);
            add(format!("{n}.x"), bound, out);
            add(format!("{n}.w"), bound, out);
        }
        "op" => {
            free_vars(v, a[1], bound, out);
            free_vars(v, a[2], bound, out);
        }
        "call" => {
            for &x in &a {
                free_vars(v, x, bound, out);
            }
        }
        "fn" => {
            let n = bound.len();
            bound.extend(items(v, a[0]).iter().map(|&p| text(v, p)));
            for s in items(v, a[1]) {
                free_vars(v, s, bound, out);
            }
            free_vars(v, a[2], bound, out);
            bound.truncate(n);
        }
        "assign" | "publish" => {
            free_vars(v, a[1], bound, out);
            bound.push(text(v, a[0]));
        }
        "record" => {
            for fe in items(v, a[1]) {
                free_vars(v, items(v, fe)[1], bound, out);
            }
        }
        "expr" | "print" | "eval" | "return" | "emit" => free_vars(v, a[0], bound, out),
        "block" | "seq" => {
            for s in items(v, a[0]) {
                free_vars(v, s, bound, out);
            }
        }
        "if" => {
            free_vars(v, a[0], bound, out);
            for s in items(v, a[1]).into_iter().chain(items(v, a[2])) {
                free_vars(v, s, bound, out);
            }
        }
        "invoke" => {
            add(format!("meaning.{}", text(v, a[0])), bound, out);
            free_vars(v, a[2], bound, out);
        }
        "do" => {
            add(text(v, a[0]), bound, out);
            for &x in &a[1..] {
                free_vars(v, x, bound, out);
            }
        }
        _ => {}
    }
}

// --------------------------------------------------------------- operations

#[derive(Copy, Clone, Debug)]
pub struct Ops {
    pub stmt: OpId,
    pub apply: OpId,
}

/// One operation for every statement kind; the AST is its argument.
struct StmtOp(Arc<OnceLock<Ops>>, ProcTable);
/// Lexical function application: arguments `[closure, arg...]`.
struct ApplyOp(Arc<OnceLock<Ops>>);

fn ops(o: &OnceLock<Ops>) -> Ops {
    *o.get().expect("installed")
}

pub fn out_field(v: &Values) -> Field {
    v.field("out")
}

fn eval_expr(cx: &mut Ctx<'_>, ops: Ops, e: ValueId) -> Result<ValueId, Stop> {
    cx.tick(1)?;
    let v = cx.values();
    if let Value::Int(_) = *v.get(e) {
        return Ok(e);
    }
    let (t, a) = tag(v, e);
    Ok(match t.as_str() {
        "str" | "lit" => a[0],
        "var" => cx.use_field(v.field(&text(v, a[0])))?.unwrap_or(ValueId::UNIT),
        "op" => {
            let x = eval_expr(cx, ops, a[1])?;
            let y = eval_expr(cx, ops, a[2])?;
            binop(v, &text(v, a[0]), x, y)
        }
        "call" => {
            let f = eval_expr(cx, ops, a[0])?;
            let mut args = vec![f];
            for &x in &a[1..] {
                args.push(eval_expr(cx, ops, x)?);
            }
            if tag(v, f).0 != "closure" {
                return Ok(v.str("error: not a function"));
            }
            cx.apply(ops.apply, args)?
        }
        "fn" => {
            let mut bound: Vec<String> = items(v, a[0]).iter().map(|&p| text(v, p)).collect();
            let mut free = Vec::new();
            for s in items(v, a[1]) {
                free_vars(v, s, &mut bound, &mut free);
            }
            free_vars(v, a[2], &mut bound, &mut free);
            free.sort();
            let mut caps = Vec::new();
            for n in free {
                if let Some(val) = cx.use_field(v.field(&n))? {
                    caps.push(v.list(vec![v.str(&n), val]));
                }
            }
            let caps = v.list(caps);
            v.list(vec![v.str("closure"), a[0], a[1], a[2], caps])
        }
        "layout" => {
            // Reads only x and w of the record.
            let n = text(v, a[0]);
            let x = cx.use_field(v.field(&format!("{n}.x")))?;
            let w = cx.use_field(v.field(&format!("{n}.w")))?;
            let x = x.and_then(|x| v.as_int(x)).unwrap_or(0);
            let w = w.and_then(|w| v.as_int(w)).unwrap_or(0);
            v.int(x + w)
        }
        _ => v.str("error: bad expression"),
    })
}

fn binop(v: &Values, op: &str, x: ValueId, y: ValueId) -> ValueId {
    let (a, b) = (v.get(x), v.get(y));
    match (op, &*a, &*b) {
        ("+", Value::Int(a), Value::Int(b)) => v.int(a.wrapping_add(*b)),
        ("-", Value::Int(a), Value::Int(b)) => v.int(a.wrapping_sub(*b)),
        ("*", Value::Int(a), Value::Int(b)) => v.int(a.wrapping_mul(*b)),
        ("<", Value::Int(a), Value::Int(b)) => v.bool(a < b),
        ("+", Value::Str(a), Value::Str(b)) => v.str(&format!("{a}{b}")),
        ("+", Value::Str(a), Value::Int(b)) => v.str(&format!("{a}{b}")),
        ("==", _, _) => v.bool(x == y),
        _ => v.str("error: type"),
    }
}

/// An AST expression that evaluates to `val`.
fn literal(v: &Values, val: ValueId) -> ValueId {
    match *v.get(val) {
        Value::Int(_) => val,
        _ => v.list(vec![v.str("lit"), val]),
    }
}

/// Values of the expression lines among claimed text.
fn claimed_values(cx: &mut Ctx<'_>, ops: Ops, lines: &[(OpId, Vec<ValueId>)]) -> Result<Vec<ValueId>, Stop> {
    let v = cx.values();
    let mut out = Vec::new();
    for (op, args) in lines {
        if *op != ops.stmt {
            continue;
        }
        let (t, a) = tag(v, args[0]);
        if t == "expr" || t == "print" {
            out.push(eval_expr(cx, ops, a[0])?);
        }
    }
    Ok(out)
}

impl Operation for StmtOp {
    fn name(&self) -> &'static str {
        "stmt"
    }

    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let (t, a) = tag(v, args[0]);
        let loc = |g: ValueId| {
            if truthy(v, g) {
                Locality::Global
            } else {
                Locality::Local
            }
        };
        match t.as_str() {
            "assign" => Some(vec![(v.field(&text(v, a[0])), loc(a[2]))]),
            "record" => {
                let base = text(v, a[0]);
                Some(
                    items(v, a[1])
                        .into_iter()
                        .map(|fe| (v.field(&format!("{base}.{}", text(v, items(v, fe)[0]))), loc(a[2])))
                        .collect(),
                )
            }
            "sumnext" | "listto" | "listuntil" => Some(vec![(v.field(&text(v, a[0])), Locality::Local)]),
            "proc" => Some(vec![(proc_field(v, a[0]), Locality::Local)]),
            "invoke" | "do" | "eval" | "seq" => None,
            "block" if exports(v, &items(v, a[0])).is_none_or(|e| !e.is_empty()) => None,
            "if" => {
                let then = exports(v, &items(v, a[1]));
                let other = exports(v, &items(v, a[2]));
                match (then, other) {
                    (Some(x), Some(y)) if x.is_empty() && y.is_empty() => Some(Vec::new()),
                    _ => None,
                }
            }
            _ => Some(Vec::new()),
        }
    }

    fn may_define_field(&self, args: &[ValueId], v: &Values, f: Field) -> bool {
        // Unfolded or evaluated text never defines a procedure name.
        !matches!(tag(v, args[0]).0.as_str(), "do" | "eval") || !text(v, f.0).starts_with("proc.")
    }

    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let names = |e: Option<Vec<String>>| match e {
            Some(e) if e.is_empty() => MayDefine::Nothing,
            Some(e) => MayDefine::Names(e.iter().map(|n| v.field(n)).collect()),
            None => MayDefine::Any,
        };
        let (t, a) = tag(v, args[0]);
        match t.as_str() {
            // Whatever `~name` means, it can only define its target.
            "invoke" => MayDefine::Names(vec![v.field(&text(v, a[1]))]),
            "do" => {
                let p = self.1.read().expect("procs");
                names(if p.tracked { p.union(&text(v, a[0])) } else { None })
            }
            "eval" => match eval_literal(v, a[0]) {
                Some(e) => names(Some(e)),
                None => eval_prefix(v, a[0]),
            },
            "block" => names(exports(v, &items(v, a[0]))),
            "if" => names(exports(v, &items(v, a[1])).and_then(|mut e| {
                e.extend(exports(v, &items(v, a[2]))?);
                Some(e)
            })),
            _ => MayDefine::Any,
        }
    }

    fn effect(&self, args: &[ValueId], v: &Values) -> Effect {
        match tag(v, args[0]).0.as_str() {
            "begin" => Effect::Open,
            "end" => Effect::Close,
            // Runtime code may open or close scopes; literal text shows it.
            "eval" if eval_literal(v, tag(v, args[0]).1[0]).is_some() => Effect::None,
            "eval" => Effect::Pending,
            _ => Effect::None,
        }
    }

    fn claim_limit(
        &self,
        args: &[ValueId],
        v: &Values,
        following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        // A delimiter claim never extends past the enclosing block's close.
        if tag(v, args[0]).0 != "listuntil" {
            return None;
        }
        for (n, (_, a)) in following.enumerate() {
            if tag(v, a[0]).0 == "end" {
                return Some(n);
            }
        }
        None
    }

    fn claim(&self, args: &[ValueId], v: &Values, following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        let (t, a) = tag(v, args[0]);
        match t.as_str() {
            "sumnext" => Claim::Count(usize::try_from(v.as_int(a[1]).unwrap_or(0).max(0)).unwrap_or(0)),
            "listto" => {
                // Syntactic: up to and including the next `endlist`.
                let mut n = 0;
                for (_, args) in following {
                    n += 1;
                    if tag(v, args[0]).0 == "endlist" {
                        return Claim::Count(n);
                    }
                }
                Claim::Count(n)
            }
            // The delimiter is a value.
            "listuntil" => Claim::Pending,
            _ => Claim::None,
        }
    }

    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        let (t, a) = tag(v, args[0]);
        let mut out = Vec::new();
        let mut bound = Vec::new();
        match t.as_str() {
            "assign" | "publish" => free_vars(v, a[1], &mut bound, &mut out),
            // Text substitution: a call reads only the procedure.
            "do" => out.push(format!("proc.{}", text(v, a[0]))),
            "record" | "expr" | "print" | "emit" | "eval" | "invoke" => {
                free_vars(v, args[0], &mut bound, &mut out);
            }
            "if" => free_vars(v, a[0], &mut bound, &mut out),
            "listuntil" => out.push(text(v, a[1])),
            "block" | "begin" | "end" | "endlist" | "mark" | "loop" | "proc" => {}
            _ => return None,
        }
        Some(out.iter().map(|n| v.field(n)).collect())
    }

    #[allow(clippy::too_many_lines)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ops = ops(&self.0);
        let v = cx.values();
        let (t, a) = tag(v, cx.args()[0]);
        match t.as_str() {
            "assign" => {
                let val = eval_expr(cx, ops, a[1])?;
                cx.define(v.field(&text(v, a[0])), val);
                Ok(val)
            }
            "publish" => {
                let val = eval_expr(cx, ops, a[1])?;
                cx.publish(v.field(&text(v, a[0])), val);
                Ok(val)
            }
            "record" => {
                let base = text(v, a[0]);
                let mut vals = Vec::new();
                for fe in items(v, a[1]) {
                    let fe = items(v, fe);
                    let val = eval_expr(cx, ops, fe[1])?;
                    cx.define(v.field(&format!("{base}.{}", text(v, fe[0]))), val);
                    vals.push(val);
                }
                Ok(v.list(vals))
            }
            "block" => {
                cx.open_scope();
                for s in items(v, a[0]) {
                    cx.child(ops.stmt, vec![s]);
                }
                cx.close_scope();
                Ok(ValueId::UNIT)
            }
            "seq" => {
                for s in items(v, a[0]) {
                    cx.child(ops.stmt, vec![s]);
                }
                Ok(ValueId::UNIT)
            }
            "if" => {
                let c = eval_expr(cx, ops, a[0])?;
                let branch = if truthy(v, c) { a[1] } else { a[2] };
                cx.open_scope();
                for s in items(v, branch) {
                    cx.child(ops.stmt, vec![s]);
                }
                cx.close_scope();
                Ok(v.bool(truthy(v, c)))
            }
            "eval" => {
                let src = eval_expr(cx, ops, a[0])?;
                cx.set_effect(Effect::None);
                match parse(v, &text(v, src)) {
                    Ok(ss) if ss.iter().any(|&s| has_proc(v, s)) => Ok(v.str("error: procedure in evaluated text")),
                    Ok(ss) => {
                        for s in ss {
                            cx.child(ops.stmt, vec![s]);
                        }
                        Ok(ValueId::UNIT)
                    }
                    Err(e) => Ok(v.str(&format!("error: {e}"))),
                }
            }
            "proc" => {
                let val = v.list(vec![v.str("proc"), a[1], a[2]]);
                cx.define(proc_field(v, a[0]), val);
                Ok(val)
            }
            "do" => {
                // Text substitution: the body becomes program text here, in
                // a scope, with the parameters bound to the argument *text*
                // (evaluated at the call site, after the unfolding). The text
                // is shared between equal calls; each call site binds its own
                // copy. The structure depends only on the procedure's value.
                let p = cx.use_field(proc_field(v, a[0]))?.unwrap_or(ValueId::UNIT);
                let (pt, pa) = tag(v, p);
                if pt != "proc" {
                    return Ok(v.str("error: not a procedure"));
                }
                if has_proc(v, pa[1]) {
                    // Procedures live in their own namespace, which unfolded
                    // text never defines (see `may_define_field`).
                    return Ok(v.str("error: procedure inside a procedure"));
                }
                let argt = a[1..].to_vec();
                let key = v.list(argt.clone());
                let body = cx.memo_text(p, key, |v| {
                    let mut ss = vec![v.list(vec![v.str("begin")])];
                    for (par, arg) in items(v, pa[0]).iter().zip(&argt) {
                        ss.push(v.list(vec![v.str("assign"), *par, *arg, v.bool(false)]));
                    }
                    ss.extend(items(v, pa[1]));
                    ss.push(v.list(vec![v.str("end")]));
                    v.list(ss)
                });
                for s in items(v, body) {
                    cx.child(ops.stmt, vec![s]);
                }
                Ok(ValueId::UNIT)
            }
            "invoke" => {
                let m = cx.use_field(v.field(&format!("meaning.{}", text(v, a[0]))))?;
                let val = eval_expr(cx, ops, a[2])?;
                let target = v.field(&text(v, a[1]));
                match m.map(|m| text(v, m)).as_deref() {
                    Some("set") => cx.define_with(target, val, Locality::Local),
                    Some("double") => {
                        let d = binop(v, "*", val, v.int(2));
                        cx.define_with(target, d, Locality::Local);
                    }
                    Some("emit") => cx.contribute(out_field(v), v.seq_unit(val)),
                    Some("show") => {}
                    _ => return Ok(v.str("error: no meaning")),
                }
                Ok(val)
            }
            "sumnext" => {
                let lines = cx.claimed();
                let vals = claimed_values(cx, ops, &lines)?;
                let sum = vals
                    .iter()
                    .map(|&x| v.as_int(x).unwrap_or(0))
                    .fold(0i64, i64::wrapping_add);
                let s = v.int(sum);
                cx.define(v.field(&text(v, a[0])), s);
                Ok(s)
            }
            "listto" => {
                let lines = cx.claimed();
                let vals = claimed_values(cx, ops, &lines)?;
                let l = v.list(vals);
                cx.define(v.field(&text(v, a[0])), l);
                Ok(l)
            }
            "listuntil" => {
                let stop = cx.use_field(v.field(&text(v, a[1])))?.map_or_default(|x| text(v, x));
                // Up to the mark, never past the block's close.
                let mut n = 0;
                while let Some((op, args)) = cx.following(n) {
                    let (lt, la) = tag(v, args[0]);
                    if op == ops.stmt && lt == "end" {
                        break;
                    }
                    n += 1;
                    if op == ops.stmt && lt == "mark" && text(v, la[0]) == stop {
                        break;
                    }
                }
                cx.claim(n);
                let lines = cx.claimed();
                let vals = claimed_values(cx, ops, &lines)?;
                let l = v.list(vals);
                cx.define(v.field(&text(v, a[0])), l);
                Ok(l)
            }
            "emit" => {
                let val = eval_expr(cx, ops, a[0])?;
                cx.contribute(out_field(v), v.seq_unit(val));
                Ok(val)
            }
            "page" => {
                // A layout consumer: reads only its segment of the output.
                let stmt = ops.stmt;
                let prev = cx.prev_sibling_where(&|op, args| op == stmt && tag(v, args[0]).0 == "page");
                let seg = cx.acc_range(out_field(v), prev)?;
                let xs = v.seq_to_vec(seg);
                let sum = xs
                    .iter()
                    .map(|&x| v.as_int(x).unwrap_or(0))
                    .fold(0i64, i64::wrapping_add);
                Ok(v.list(vec![v.int(i64::try_from(xs.len()).unwrap_or(0)), v.int(sum)]))
            }
            "print" | "expr" | "return" => eval_expr(cx, ops, a[0]),
            "loop" | "begin" | "end" | "endlist" | "mark" => Ok(ValueId::UNIT),
            "error" => Ok(v.str(&format!("error: {}", text(v, a[0])))),
            _ => Ok(v.str("error: bad statement")),
        }
    }
}

impl Operation for ApplyOp {
    fn name(&self) -> &'static str {
        "apply"
    }

    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ops = ops(&self.0);
        let v = cx.values();
        let args = cx.args();
        let c = tag(v, args[0]).1; // [params, body, ret, captures]
        let params = items(v, c[0]);
        if params.len() != args.len() - 1 {
            return Ok(v.str("error: arity"));
        }
        for cap in items(v, c[3]) {
            let cap = items(v, cap);
            cx.set_env(v.field(&text(v, cap[0])), cap[1]);
        }
        for (p, &a) in params.iter().zip(&args[1..]) {
            cx.set_env(v.field(&text(v, *p)), a);
        }
        for s in items(v, c[1]) {
            cx.child(ops.stmt, vec![s]);
        }
        let ret = cx.child(ops.stmt, vec![v.list(vec![v.str("return"), c[2]])]);
        cx.output_of(ret)
    }
}

/// The language installed into an engine.
pub struct Lang {
    pub ops: Ops,
    pub procs: ProcTable,
}

impl Lang {
    pub fn install(eng: &mut Engine) -> Lang {
        let cell = Arc::new(OnceLock::new());
        let procs = ProcTable::default();
        let stmt = eng.register(Arc::new(StmtOp(cell.clone(), procs.clone())));
        let apply = eng.register(Arc::new(ApplyOp(cell.clone())));
        let ops = Ops { stmt, apply };
        cell.set(ops).expect("fresh");
        let out = out_field(eng.values());
        eng.declare_accumulator(out, Arc::new(SeqMonoid));
        Lang { ops, procs }
    }

    /// Declares the loop-back fields of `asts` and returns their names.
    pub fn declare_loops(eng: &mut Engine, asts: &[ValueId]) -> Vec<String> {
        let mut names = Vec::new();
        for &s in asts {
            let v = eng.values();
            let (t, a) = tag(v, s);
            if t == "loop" {
                let name = text(v, a[0]);
                let f = v.field(&name);
                let init = a[1];
                eng.declare_loopback(f, init);
                names.push(name);
            }
        }
        names
    }

    /// Assignments to loop-back fields publish instead of define.
    pub fn rewrite(v: &Values, loops: &[String], ast: ValueId) -> ValueId {
        let (t, a) = tag(v, ast);
        if t == "assign" && loops.contains(&text(v, a[0])) {
            return v.list(vec![v.str("publish"), a[0], a[1]]);
        }
        ast
    }
}

/// A program held as one top-level statement per line, mirrored into a unit.
pub struct Program {
    pub lang: Lang,
    pub loops: Vec<String>,
    pub unit: UnitId,
    pub lines: Vec<String>,
    pub nodes: Vec<NodeId>,
}

impl Program {
    /// Loads `lines` into a fresh engine.
    pub fn load(lines: Vec<String>) -> (Engine, Program) {
        let mut eng = Engine::new();
        let lang = Lang::install(&mut eng);
        let unit = eng.new_unit();
        let asts: Vec<ValueId> = lines.iter().map(|l| parse_stmt(eng.values(), l)).collect();
        let loops = Lang::declare_loops(&mut eng, &asts);
        {
            let mut p = lang.procs.write().expect("procs");
            for &ast in &asts {
                let mut d = Vec::new();
                proc_decls(eng.values(), ast, &mut d);
                p.add(d);
            }
            p.tracked = true;
        }
        let mut nodes = Vec::with_capacity(lines.len());
        for ast in asts {
            let ast = Lang::rewrite(eng.values(), &loops, ast);
            let n = eng.push(unit, lang.ops.stmt, vec![ast]);
            eng.add_root(n);
            nodes.push(n);
        }
        (
            eng,
            Program {
                lang,
                loops,
                unit,
                lines,
                nodes,
            },
        )
    }

    /// Replaces line `i`.
    pub fn set_line(&mut self, eng: &mut Engine, i: usize, text: String) {
        let ast = Lang::rewrite(eng.values(), &self.loops, parse_stmt(eng.values(), &text));
        let old = self.lines[i].clone();
        self.procs_edit(eng, Some(&old), Some(&text));
        eng.replace(self.nodes[i], self.lang.ops.stmt, vec![ast]);
        self.lines[i] = text;
    }

    /// Keeps the procedure table in step with a line edit; re-reads the
    /// `do` statements whose procedures may now define something else.
    fn procs_edit(&self, eng: &mut Engine, old: Option<&str>, new: Option<&str>) {
        let decls = |eng: &Engine, l: Option<&str>| {
            let mut d = Vec::new();
            if let Some(l) = l {
                proc_decls(eng.values(), parse_stmt(eng.values(), l), &mut d);
            }
            d
        };
        let (od, nd) = (decls(eng, old), decls(eng, new));
        if od.is_empty() && nd.is_empty() {
            return;
        }
        let names: Vec<String> = od.iter().chain(&nd).map(|d| d.0.clone()).collect();
        let changed: Vec<String> = {
            let mut p = self.lang.procs.write().expect("procs");
            let before: Vec<_> = names.iter().map(|n| p.union(n)).collect();
            p.remove(&od);
            p.add(nd);
            names
                .iter()
                .zip(before)
                .filter(|(n, b)| p.union(n) != *b)
                .map(|(n, _)| n.clone())
                .collect()
        };
        if changed.is_empty() {
            return;
        }
        let stmt = self.lang.ops.stmt;
        eng.refresh_where(&|op, args, v| {
            op == stmt && {
                let (t, a) = tag(v, args[0]);
                t == "do" && changed.contains(&text(v, a[0]))
            }
        });
    }

    /// Inserts a new line before line `i`.
    pub fn insert_line(&mut self, eng: &mut Engine, i: usize, text: String) {
        let ast = Lang::rewrite(eng.values(), &self.loops, parse_stmt(eng.values(), &text));
        self.procs_edit(eng, None, Some(&text));
        let n = if i == 0 {
            eng.insert_before(self.nodes[0], self.lang.ops.stmt, vec![ast])
        } else {
            eng.insert_after(self.nodes[i - 1], self.lang.ops.stmt, vec![ast])
        };
        eng.add_root(n);
        self.nodes.insert(i, n);
        self.lines.insert(i, text);
    }

    /// Removes line `i`.
    pub fn remove_line(&mut self, eng: &mut Engine, i: usize) {
        let n = self.nodes.remove(i);
        eng.remove(n);
        let old = self.lines.remove(i);
        self.procs_edit(eng, Some(&old), None);
    }

    /// The result as raw content ids: every top-level output (0 if the line
    /// is claimed) and the `out` accumulator. Compared bytewise.
    pub fn result(&self, eng: &Engine) -> Vec<u64> {
        let mut r: Vec<u64> = self
            .nodes
            .iter()
            .map(|&n| eng.output(n).map_or(0, ValueId::raw))
            .collect();
        r.push(
            eng.acc_value(self.unit, out_field(eng.values()))
                .map_or(0, ValueId::raw),
        );
        r
    }

    /// The result rendered (for display).
    pub fn render(&self, eng: &Engine) -> Vec<String> {
        let mut r: Vec<String> = self
            .nodes
            .iter()
            .map(|&n| eng.output(n).map_or_else(|| "-".into(), |v| eng.values().render(v)))
            .collect();
        if let Some(acc) = eng.acc_value(self.unit, out_field(eng.values())) {
            r.push(eng.values().render(acc));
        }
        r
    }
}

/// Runs batches on scoped OS threads (examples and tests only: the library
/// never spawns threads).
pub struct Threads(pub usize);

impl Executor for Threads {
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        if self.0 <= 1 || n < 64 {
            out.extend((0..n).map(run));
            return;
        }
        // Dynamic distribution in small grains: job costs vary a lot (a gated
        // job is a few lookups), so equal static chunks leave threads idle.
        // The engine orders the results itself.
        let grain = (n / (self.0 * 8)).clamp(4, 64);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let workers = self.0.min(n.div_ceil(grain));
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    s.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let start = next.fetch_add(grain, std::sync::atomic::Ordering::Relaxed);
                            if start >= n {
                                break;
                            }
                            mine.extend((start..(start + grain).min(n)).map(run));
                        }
                        mine
                    })
                })
                .collect();
            for h in handles {
                out.extend(h.join().expect("worker panicked"));
            }
        });
    }
}

// --------------------------------------------------------------- generator

/// Sections of this many lines; dependencies stay inside a section or go to
/// the header, so the inherent dependency depth is bounded by a constant.
pub const SECTION: usize = 24;

/// Generates a random program of about `n` lines using every feature.
pub fn generate(seed: u64, n: usize) -> Vec<String> {
    generate_with(seed, n, true)
}

/// [`generate`], optionally without evaluated text that starts with a
/// computed piece (the only unbounded definition sets it makes).
#[allow(clippy::too_many_lines)]
pub fn generate_with(seed: u64, n: usize, unbounded: bool) -> Vec<String> {
    let mut rng = defuse::Rng::new(seed);
    let mut lines: Vec<String> = vec![
        "loop fwd = 0".into(),
        "early = fwd + 1".into(),
        "meaning put = \"set\"".into(),
        "meaning dbl = \"double\"".into(),
        "meaning say = \"emit\"".into(),
        "stopword = \"zz\"".into(),
        "proc bump(a) { global pb = a + 1 }".into(),
        "proc show(a) { emit a * 3 }".into(),
        "f = fn(a) { t = a * 2 return t + g1 }".into(),
    ];
    for g in 0..32 {
        lines.push(format!("g{g} = {}", rng.below(100)));
    }
    let mut sec = 0;
    while lines.len() + 4 < n {
        sec += 1;
        lines.push("begin".into());
        let mut locals: Vec<String> = Vec::new();
        if rng.chance(1, 8) {
            lines.push(format!(
                "meaning put = \"{}\"",
                if rng.chance(1, 2) { "double" } else { "set" }
            ));
        }
        let pick = |rng: &mut defuse::Rng, locals: &[String]| -> String {
            if locals.is_empty() || rng.chance(1, 4) {
                format!("g{}", rng.below(32))
            } else {
                locals[locals.len() - 1 - rng.below(locals.len().min(4))].clone()
            }
        };
        let mut i = 0;
        while i < SECTION {
            let k = rng.below(100);
            let name = format!("x{sec}_{i}");
            let src = pick(&mut rng, &locals);
            // Computed definitions (evaluated text) on 0.3% of lines: 9 in
            // 10 with a literal prefix, 1 in 10 starting with a computed
            // piece (unbounded).
            if rng.chance(3, 1000) {
                let named = match rng.below(10) {
                    0 if unbounded => {
                        lines.push(format!("s{sec}_{i} = \"e{sec}_{i} = \""));
                        lines.push(format!("eval s{sec}_{i} + {src}"));
                        true
                    }
                    1 | 2 => {
                        lines.push(format!("eval \"u{sec}_\" + {src} + \" = 1\""));
                        false
                    }
                    _ => {
                        lines.push(format!("eval \"e{sec}_{i} = \" + {src}"));
                        true
                    }
                };
                if named {
                    lines.push(format!("{name} = e{sec}_{i} + 1"));
                    locals.push(name);
                }
                i += 1;
                continue;
            }
            match rng.below(16) {
                0..=3 => {
                    lines.push(format!("{name} = {src} + {k}"));
                    locals.push(name);
                }
                4 => {
                    lines.push(format!("{name} = {k}"));
                    locals.push(name);
                }
                5 => {
                    lines.push(format!(
                        "p{sec}_{i} = {{x: {k}, w: {}, c: {}}}",
                        rng.below(50),
                        rng.below(9)
                    ));
                    lines.push(format!("{name} = layout(p{sec}_{i})"));
                    locals.push(name);
                }
                6 => {
                    lines.push(format!("{name} = f({src})"));
                    locals.push(name);
                }
                7 => {
                    lines.push(format!("~put {name} {src} + {k}"));
                    locals.push(name);
                }
                8 => {
                    lines.push(format!("~dbl {name} {src}"));
                    locals.push(name);
                }
                9 => lines.push(format!("emit {src} + {k}")),
                10 => lines.push(format!("~say {name} {src}")),
                11 => {
                    lines.push(format!("sumnext {name} 2"));
                    lines.push(format!("{src} + {k}"));
                    lines.push(format!("{}", rng.below(9)));
                    locals.push(name);
                    i += 2;
                }
                12 => {
                    lines.push(format!("listto {name}"));
                    for _ in 0..=rng.below(3) {
                        lines.push(format!("{src} * {}", rng.below(5)));
                        i += 1;
                    }
                    lines.push("endlist".into());
                    locals.push(name);
                }
                // 95% of procedure bodies define nothing.
                13 if rng.chance(1, 20) => {
                    lines.push(format!("do bump({src})"));
                    lines.push(format!("{name} = pb + {k}"));
                    locals.push(name);
                }
                13 => lines.push(format!("do show({src})")),
                14 => {
                    lines.push(format!(
                        "if {src} < {k} {{ global h{sec} = 1 }} else {{ global h{sec} = 2 }}"
                    ));
                    lines.push(format!("{name} = h{sec} + 1"));
                    locals.push(name);
                }
                _ if sec % 16 == 0 && i == 3 => {
                    lines.push(format!("listuntil {name} stopword"));
                    lines.push(format!("{src} + 1"));
                    lines.push("mark zz".into());
                    locals.push(name);
                    i += 2;
                }
                _ => {
                    lines.push(format!("{name} = {src} * 2"));
                    locals.push(name);
                }
            }
            i += 1;
        }
        lines.push("end".into());
        if sec % 4 == 0 {
            lines.push("page".into());
        }
    }
    lines.push("page".into());
    lines.push("fwd = g0 * 0 + 3".into());
    lines.push("print early".into());
    lines
}
