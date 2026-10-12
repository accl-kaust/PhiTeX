//! A small sequential interpreter for the example language: the reference
//! the engine is checked against. It shares only the parser and the pure
//! value helpers with the engine's operations; binding, scopes, claims,
//! unfolding, accumulators and loop-back rounds are its own.
//!
//! It runs statements once each, in text order. A scope is a frame on a
//! stack; every definition carries its text position, and a use takes the
//! latest visible one: the innermost frame holding the name (frames above
//! it were opened later), unless a global was defined after that. Unfolded
//! text runs in place. A claim takes following statements of the same list.
//! A function body runs in a fresh unit seeded with its captures and
//! parameters. Loop-back fields are iterated in whole rounds.

use super::{
    DOC_KIND, FOLD, FOLD_STATE, LIST_KIND, Lang, OUT_KIND, binop, fold_init, fold_step, fold_text, has_proc, items,
    parse, parse_stmt, proc_field, tag, text, truthy,
};
use defuse::{Field, FxHashMap, FxHashSet, Value, ValueId, Values};

type Binding = (u64, ValueId);

/// One unit of text: the program, or one function application's body.
#[derive(Default, Clone)]
struct Unit {
    /// Local frames; the first is the unit's own (never closed).
    frames: Vec<FxHashMap<Field, Binding>>,
    /// Each frame's scope kind.
    kinds: Vec<Option<String>>,
    /// Each frame's `items` contributions, in text order.
    seqs: Vec<Vec<ValueId>>,
    globals: FxHashMap<Field, Binding>,
    env: FxHashMap<Field, ValueId>,
    /// `out` contributions, in text order.
    contribs: Vec<(u64, ValueId)>,
}

impl Unit {
    fn new() -> Unit {
        Unit {
            frames: vec![FxHashMap::default()],
            kinds: vec![None],
            seqs: vec![Vec::new()],
            ..Unit::default()
        }
    }

    fn kind(&self) -> Option<&str> {
        self.kinds.last().and_then(Option::as_deref)
    }
}

/// A stream claim being looked ahead (see `collect_ahead`).
#[derive(Clone)]
struct Collect {
    class: String,
    field: String,
    outs: Vec<ValueId>,
    done: bool,
}

#[derive(Clone)]
struct Interp<'v> {
    v: &'v Values,
    pos: u64,
    loops: FxHashMap<Field, ValueId>,
    pubs: FxHashMap<Field, ValueId>,
    apps: FxHashMap<ValueId, ValueId>,
    running: FxHashSet<ValueId>,
    error: Option<String>,
    collect: Option<Collect>,
    /// Nodes visited so far (to tell stream elements from statements that
    /// unfolded).
    visits: u64,
}

/// Runs `lines` and returns [`super::Program::result`]'s vector: each line's
/// output id (0 if claimed) and the `out` accumulator.
///
/// # Errors
///
/// A cycle (a function applied to itself), conflicting publications of a
/// loop-back field, or no convergence within the engine's round limit.
pub fn interpret(lines: &[String]) -> Result<Vec<u64>, String> {
    interpret_env(lines, &[])
}

/// Interprets `lines` with the environment `env` (integer fields defined
/// before the first line).
pub fn interpret_env(lines: &[String], env: &[(&str, i64)]) -> Result<Vec<u64>, String> {
    let v = Values::new();
    let asts: Vec<ValueId> = lines.iter().map(|l| parse_stmt(&v, l)).collect();
    let mut loops = FxHashMap::default();
    let mut names = Vec::new();
    for &s in &asts {
        let (t, a) = tag(&v, s);
        if t == "loop" {
            let name = text(&v, a[0]);
            loops.insert(v.field(&name), a[1]);
            names.push(name);
        }
    }
    let asts: Vec<ValueId> = asts.into_iter().map(|s| Lang::rewrite(&v, &names, s)).collect();
    let mut it = Interp {
        v: &v,
        pos: 0,
        loops,
        pubs: FxHashMap::default(),
        apps: FxHashMap::default(),
        running: FxHashSet::default(),
        error: None,
        collect: None,
        visits: 0,
    };
    for _round in 0..64 {
        it.pos = 0;
        it.pubs.clear();
        // Applications may read loop-back fields.
        it.apps.clear();
        let mut u = Unit::new();
        for &(f, x) in env {
            u.env.insert(v.field(f), v.int(x));
        }
        let outs = it.run_list(&mut u, &asts);
        if let Some(e) = it.error.take() {
            return Err(e);
        }
        let mut changed = false;
        for (f, &p) in &it.pubs {
            if it.loops.get(f) != Some(&p) {
                changed = true;
            }
        }
        if !changed {
            let mut r: Vec<u64> = outs.iter().map(|o| o.map_or(0, ValueId::raw)).collect();
            let acc = u.contribs.iter().fold(ValueId::NIL, |a, &(_, x)| v.seq_concat(a, x));
            r.push(acc.raw());
            return Ok(r);
        }
        let pubs = std::mem::take(&mut it.pubs);
        it.loops.extend(pubs);
    }
    Err("no convergence".into())
}

impl Interp<'_> {
    fn define(&mut self, u: &mut Unit, f: Field, val: ValueId, global: bool) {
        let b = (self.pos, val);
        if global {
            u.globals.insert(f, b);
        } else {
            u.frames.last_mut().expect("unit frame").insert(f, b);
        }
    }

    fn lookup(&self, u: &Unit, f: Field) -> Option<ValueId> {
        if let Some(&x) = self.loops.get(&f) {
            return Some(x);
        }
        let local = u.frames.iter().rev().find_map(|fr| fr.get(&f)).copied();
        let global = u.globals.get(&f).copied();
        match (local, global) {
            (Some(l), Some(g)) => Some(if l.0 > g.0 { l.1 } else { g.1 }),
            (Some(b), None) | (None, Some(b)) => Some(b.1),
            (None, None) => u.env.get(&f).copied(),
        }
    }

    fn open(u: &mut Unit) {
        Self::open_kind(u, None);
    }

    fn open_kind(u: &mut Unit, kind: Option<String>) {
        u.frames.push(FxHashMap::default());
        u.kinds.push(kind);
        u.seqs.push(Vec::new());
    }

    fn close(u: &mut Unit) {
        // An unmatched close is ignored.
        if u.frames.len() > 1 {
            u.frames.pop();
            u.kinds.pop();
            u.seqs.pop();
        }
    }

    /// Runs a statement list; each statement's output, `None` if claimed.
    fn run_list(&mut self, u: &mut Unit, stmts: &[ValueId]) -> Vec<Option<ValueId>> {
        let v = self.v;
        let mut outs = vec![None; stmts.len()];
        let mut last_page = 0;
        let mut i = 0;
        while i < stmts.len() {
            if self.collect.as_ref().is_some_and(|c| c.done) {
                break;
            }
            self.pos += 1;
            let (t, a) = tag(v, stmts[i]);
            // A stream claim tests each candidate before it runs.
            let fires = self.fires(u, &t);
            self.visits += 1;
            let visits = self.visits;
            let rest = &stmts[i + 1..];
            let claim = match t.as_str() {
                "sumnext" => usize::try_from(v.as_int(a[1]).unwrap_or(0).max(0))
                    .unwrap_or(0)
                    .min(rest.len()),
                "listto" => rest
                    .iter()
                    .position(|&s| tag(v, s).0 == "endlist")
                    .map_or(rest.len(), |k| k + 1),
                "listuntil" => {
                    let stop = self
                        .lookup(u, v.field(&text(v, a[1])))
                        .map_or_else(String::new, |x| text(v, x));
                    let mut n = 0;
                    for &s in rest {
                        let (lt, la) = tag(v, s);
                        if lt == "end" {
                            break;
                        }
                        n += 1;
                        if lt == "mark" && text(v, la[0]) == stop {
                            break;
                        }
                    }
                    n
                }
                _ => 0,
            };
            let out = if claim > 0 || matches!(t.as_str(), "sumnext" | "listto" | "listuntil") {
                let vals = self.claimed_values(u, &rest[..claim]);
                let val = if t == "sumnext" {
                    v.int(
                        vals.iter()
                            .map(|&x| v.as_int(x).unwrap_or(0))
                            .fold(0i64, i64::wrapping_add),
                    )
                } else {
                    v.list(vals)
                };
                self.define(u, v.field(&text(v, a[0])), val, false);
                val
            } else if t == "collect" {
                let outs = self.collect_ahead(u, &a, &stmts[i + 1..]);
                let l = v.list(outs);
                self.define(u, v.field(&text(v, a[0])), l, false);
                l
            } else if t == "page" {
                let xs: Vec<ValueId> = u
                    .contribs
                    .iter()
                    .filter(|c| c.0 > last_page)
                    .flat_map(|c| v.seq_to_vec(c.1))
                    .collect();
                last_page = self.pos;
                let sum = xs
                    .iter()
                    .map(|&x| v.as_int(x).unwrap_or(0))
                    .fold(0i64, i64::wrapping_add);
                v.list(vec![v.int(i64::try_from(xs.len()).unwrap_or(0)), v.int(sum)])
            } else {
                self.stmt(u, &t, &a)
            };
            outs[i] = Some(out);
            if self.visits == visits {
                self.element(fires, out);
            }
            // Claimed text keeps its positions but does not run.
            self.pos += claim as u64;
            i += 1 + claim;
        }
        outs
    }

    /// Whether the claim being looked ahead ends before a node of `class`
    /// here.
    fn fires(&self, u: &Unit, class: &str) -> bool {
        let v = self.v;
        self.collect.as_ref().is_some_and(|c| {
            c.class == class
                && (c.field.is_empty()
                    || c.field
                        .split(' ')
                        .all(|f| self.lookup(u, v.field(f)).is_some_and(|x| truthy(v, x))))
        })
    }

    /// A node that unfolded nothing: an element of the stream.
    fn element(&mut self, fires: bool, out: ValueId) {
        if let Some(c) = &mut self.collect
            && !c.done
        {
            if fires {
                c.done = true;
            } else {
                c.outs.push(out);
            }
        }
    }

    /// A node the engine has that is no statement here (a fold output's
    /// scope opener, output and resume nodes).
    fn visit(&mut self, u: &Unit, class: &str, out: ValueId) {
        let fires = self.fires(u, class);
        self.visits += 1;
        self.element(fires, out);
    }

    /// `collect NAME until CLASS [if FIELD]`: runs the following text on a
    /// copy of the state until the claim ends, and returns the outputs of
    /// the elements before it.
    fn collect_ahead(&self, u: &Unit, a: &[ValueId], rest: &[ValueId]) -> Vec<ValueId> {
        let v = self.v;
        let mut it = self.clone();
        let mut cu = u.clone();
        it.collect = Some(Collect {
            class: text(v, a[1]),
            field: text(v, a[2]),
            outs: Vec::new(),
            done: false,
        });
        it.run_list(&mut cu, rest);
        it.collect.map_or_else(Vec::new, |c| c.outs)
    }

    fn claimed_values(&mut self, u: &mut Unit, lines: &[ValueId]) -> Vec<ValueId> {
        let mut out = Vec::new();
        for &s in lines {
            let (t, a) = tag(self.v, s);
            if t == "expr" || t == "print" {
                out.push(self.expr(u, a[0]));
            }
        }
        out
    }

    #[allow(clippy::too_many_lines)]
    fn stmt(&mut self, u: &mut Unit, t: &str, a: &[ValueId]) -> ValueId {
        let v = self.v;
        match t {
            "assign" => {
                let val = self.expr(u, a[1]);
                self.define(u, v.field(&text(v, a[0])), val, truthy(v, a[2]));
                val
            }
            "publish" => {
                let val = self.expr(u, a[1]);
                let f = v.field(&text(v, a[0]));
                if self.pubs.insert(f, val).is_some_and(|p| p != val) {
                    self.error.get_or_insert_with(|| "conflicting publication".into());
                }
                val
            }
            "record" => {
                let base = text(v, a[0]);
                let mut vals = Vec::new();
                for fe in items(v, a[1]) {
                    let fe = items(v, fe);
                    let val = self.expr(u, fe[1]);
                    self.define(u, v.field(&format!("{base}.{}", text(v, fe[0]))), val, truthy(v, a[2]));
                    vals.push(val);
                }
                v.list(vals)
            }
            "block" => {
                // The scope markers: never an element.
                self.visits += 1;
                Self::open(u);
                self.run_list(u, &items(v, a[0]));
                Self::close(u);
                ValueId::UNIT
            }
            "seq" => {
                self.run_list(u, &items(v, a[0]));
                ValueId::UNIT
            }
            "if" => {
                let c = self.expr(u, a[0]);
                self.visits += 1;
                Self::open(u);
                self.run_list(u, &items(v, if truthy(v, c) { a[1] } else { a[2] }));
                Self::close(u);
                v.bool(truthy(v, c))
            }
            "eval" => {
                let src = self.expr(u, a[0]);
                match parse(v, &text(v, src)) {
                    Ok(ss) if ss.iter().any(|&s| has_proc(v, s)) => v.str("error: procedure in evaluated text"),
                    Ok(ss) if ss.iter().any(|&s| super::has_tag(v, s, &["item"])) => {
                        v.str("error: item in evaluated text")
                    }
                    Ok(ss) => {
                        self.run_list(u, &ss);
                        ValueId::UNIT
                    }
                    Err(e) => v.str(&format!("error: {e}")),
                }
            }
            "proc" => {
                let val = v.list(vec![v.str("proc"), a[1], a[2]]);
                self.define(u, proc_field(v, a[0]), val, false);
                val
            }
            "do" => {
                let p = self.lookup(u, proc_field(v, a[0])).unwrap_or(ValueId::UNIT);
                let (pt, pa) = tag(v, p);
                if pt != "proc" {
                    return v.str("error: not a procedure");
                }
                if has_proc(v, pa[1]) {
                    return v.str("error: procedure inside a procedure");
                }
                let mut ss = vec![v.list(vec![v.str("begin")])];
                for (par, arg) in items(v, pa[0]).iter().zip(&a[1..]) {
                    ss.push(v.list(vec![v.str("assign"), *par, *arg, v.bool(false)]));
                }
                ss.extend(items(v, pa[1]));
                ss.push(v.list(vec![v.str("end")]));
                self.run_list(u, &ss);
                ValueId::UNIT
            }
            "invoke" => {
                let m = self.lookup(u, v.field(&format!("meaning.{}", text(v, a[0]))));
                let val = self.expr(u, a[2]);
                let target = v.field(&text(v, a[1]));
                match m.map(|m| text(v, m)).as_deref() {
                    Some("set") => self.define(u, target, val, false),
                    Some("double") => {
                        let d = binop(v, "*", val, v.int(2));
                        self.define(u, target, d, false);
                    }
                    Some("emit") => u.contribs.push((self.pos, v.seq_unit(val))),
                    Some("show") => {}
                    _ => return v.str("error: no meaning"),
                }
                val
            }
            "emit" => {
                let val = self.expr(u, a[0]);
                u.contribs.push((self.pos, v.seq_unit(val)));
                val
            }
            "print" | "expr" | "return" => self.expr(u, a[0]),
            "begin" => {
                Self::open(u);
                ValueId::UNIT
            }
            "end" => {
                // Also closes an implicit scope open inside the one it ends.
                if u.kind() == Some(LIST_KIND) {
                    Self::close(u);
                }
                Self::close(u);
                ValueId::UNIT
            }
            "scope" => {
                Self::open_kind(u, Some(text(v, a[0])));
                ValueId::UNIT
            }
            "item" => {
                if u.kind() == Some(DOC_KIND) {
                    Self::open_kind(u, Some(LIST_KIND.into()));
                }
                let val = self.expr(u, a[1]);
                self.define(u, v.field(&text(v, a[0])), val, false);
                val
            }
            "para" => {
                if u.kind() == Some(LIST_KIND) {
                    Self::close(u);
                }
                ValueId::UNIT
            }
            "add" => {
                let val = self.expr(u, a[0]);
                u.seqs.last_mut().expect("unit frame").push(val);
                let state = v.field(FOLD_STATE);
                let decl = if u.kind() == Some(OUT_KIND) {
                    None
                } else {
                    self.lookup(u, v.field(FOLD))
                };
                let Some(src) = decl else {
                    self.define(u, state, ValueId::UNIT, false);
                    return val;
                };
                let st = self
                    .lookup(u, state)
                    .filter(|&s| items(v, s).len() == 2)
                    .unwrap_or_else(|| fold_init(v));
                let (st, out) = fold_step(v, st, val, self.cap(u));
                self.define(u, state, st, false);
                if let Some((h, k)) = out {
                    self.output(u, src, st, h, k, Vec::new());
                }
                val
            }
            "fold" => {
                let src = self.expr(u, a[0]);
                self.define(u, v.field(FOLD), src, false);
                self.define(u, v.field(FOLD_STATE), fold_init(v), false);
                src
            }
            "when" => {
                let c = self.expr(u, a[0]);
                if truthy(v, c) {
                    self.run_list(u, &[a[1]]);
                }
                v.bool(truthy(v, c))
            }
            "take" | "replace" => {
                let r = (t == "replace").then(|| self.expr(u, a[0]));
                let s = u.seqs.last_mut().expect("unit frame");
                let taken = s.pop();
                s.extend(r);
                taken.unwrap_or_else(|| v.str("none"))
            }
            "loop" | "endlist" | "mark" => ValueId::UNIT,
            "error" => v.str(&format!("error: {}", text(v, a[0]))),
            _ => v.str("error: bad statement"),
        }
    }

    fn expr(&mut self, u: &mut Unit, e: ValueId) -> ValueId {
        let v = self.v;
        if let Value::Int(_) = *v.get(e) {
            return e;
        }
        let (t, a) = tag(v, e);
        match t.as_str() {
            "str" | "lit" => a[0],
            "kind" => v.str(u.kind().unwrap_or("none")),
            "prev" => u
                .seqs
                .last()
                .and_then(|s| s.last().copied())
                .unwrap_or_else(|| v.str("none")),
            "before" => v.list(u.seqs.last().cloned().unwrap_or_default()),
            "var" => self.lookup(u, v.field(&text(v, a[0]))).unwrap_or(ValueId::UNIT),
            "op" => {
                let x = self.expr(u, a[1]);
                let y = self.expr(u, a[2]);
                binop(v, &text(v, a[0]), x, y)
            }
            "call" => {
                let f = self.expr(u, a[0]);
                let mut args = vec![f];
                for &x in &a[1..] {
                    args.push(self.expr(u, x));
                }
                if tag(v, f).0 != "closure" {
                    return v.str("error: not a function");
                }
                self.apply(&args)
            }
            "fn" => {
                let mut bound: Vec<String> = items(v, a[0]).iter().map(|&p| text(v, p)).collect();
                let mut free = Vec::new();
                for s in items(v, a[1]) {
                    super::free_vars(v, s, &mut bound, &mut free);
                }
                super::free_vars(v, a[2], &mut bound, &mut free);
                free.sort();
                let caps: Vec<ValueId> = free
                    .iter()
                    .filter_map(|n| self.lookup(u, v.field(n)).map(|val| v.list(vec![v.str(n), val])))
                    .collect();
                v.list(vec![v.str("closure"), a[0], a[1], a[2], v.list(caps)])
            }
            "layout" => {
                let n = text(v, a[0]);
                let x = self
                    .lookup(u, v.field(&format!("{n}.x")))
                    .and_then(|x| v.as_int(x))
                    .unwrap_or(0);
                let w = self
                    .lookup(u, v.field(&format!("{n}.w")))
                    .and_then(|w| v.as_int(w))
                    .unwrap_or(0);
                v.int(x + w)
            }
            _ => v.str("error: bad expression"),
        }
    }

    fn cap(&self, u: &Unit) -> i64 {
        self.lookup(u, self.v.field("cap"))
            .and_then(|c| self.v.as_int(c))
            .unwrap_or(50)
    }

    /// Runs emitted fold output `k` in place: an `out` scope with `out`,
    /// `k` and `state` defined, the fold text, then the fold continues with
    /// what the text added and the elements still carried over.
    fn output(&mut self, u: &mut Unit, src: ValueId, st: ValueId, h: ValueId, k: ValueId, rest: Vec<ValueId>) {
        let v = self.v;
        self.pos += 1;
        self.visit(u, "scope", ValueId::UNIT);
        Self::open_kind(u, Some(OUT_KIND.into()));
        self.pos += 1;
        self.visit(u, "output", h);
        for (n, x) in [("out", h), ("k", k), ("state", st)] {
            self.define(u, v.field(n), x, false);
        }
        u.contribs.push((self.pos, v.seq_unit(h)));
        match fold_text(v, src) {
            Ok(ss) => {
                self.run_list(u, &ss);
            }
            Err(e) => {
                self.pos += 1;
                self.visit(u, "error", v.str(&format!("error: {e}")));
            }
        }
        // The resume node (an element unless it emits).
        self.pos += 1;
        let fires = self.fires(u, "resume");
        self.visits += 1;
        let visits = self.visits;
        let mut input = u.seqs.last().cloned().unwrap_or_default();
        input.extend(rest);
        let state = v.field(FOLD_STATE);
        if let Some(src) = self.lookup(u, v.field(FOLD)) {
            let cap = self.cap(u);
            let mut st = st;
            for (j, &e) in input.iter().enumerate() {
                let (s2, out) = fold_step(v, st, e, cap);
                st = s2;
                if let Some((h, k)) = out {
                    self.define(u, state, st, true);
                    self.output(u, src, st, h, k, input[j + 1..].to_vec());
                    self.pos += 1;
                    Self::close(u);
                    return;
                }
            }
            self.define(u, state, st, true);
            if self.visits == visits {
                self.element(fires, st);
            }
        } else {
            self.define(u, state, st, true);
            self.element(fires, st);
        }
        self.pos += 1;
        Self::close(u);
    }

    /// Applies a closure (memoized: applications are pure).
    fn apply(&mut self, args: &[ValueId]) -> ValueId {
        let v = self.v;
        let key = v.list(args.to_vec());
        if let Some(&r) = self.apps.get(&key) {
            return r;
        }
        if !self.running.insert(key) {
            self.error.get_or_insert_with(|| "cycle".into());
            return ValueId::UNIT;
        }
        let c = tag(v, args[0]).1; // [params, body, ret, captures]
        let params = items(v, c[0]);
        let r = if params.len() == args.len() - 1 {
            let mut u = Unit::new();
            for cap in items(v, c[3]) {
                let cap = items(v, cap);
                u.env.insert(v.field(&text(v, cap[0])), cap[1]);
            }
            for (p, &x) in params.iter().zip(&args[1..]) {
                u.env.insert(v.field(&text(v, *p)), x);
            }
            let pos = self.pos;
            let (collect, visits) = (self.collect.take(), self.visits);
            self.run_list(&mut u, &items(v, c[1]));
            let r = self.expr(&mut u, c[2]);
            (self.collect, self.visits) = (collect, visits);
            self.pos = pos;
            r
        } else {
            v.str("error: arity")
        };
        self.running.remove(&key);
        self.apps.insert(key, r);
        r
    }
}
