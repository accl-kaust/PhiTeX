//! The client: a document's text as nodes, the prelude's environment,
//! the files the graph asks for, edits, and the outputs read back (DVI
//! bytes, the log).
//!
//! Nodes come from the syntactic pass ([`phitex_source::pass`]): one per
//! control sequence, one per run of characters, one per space, brace or
//! blank line, each holding its slice of the text (and the blanks after
//! it). A node reads its own slice and the siblings its command claims.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{
    Engine, Field, NodeId, OpId, Report, SeqMonoid, SumMonoid, UnitId, Value, ValueId, Values,
};
use phitex_source::pass::{self, AtomKind};

use crate::ops::{Dvi, Exec, Files, Mark, OutPage, Put, Shared};
use crate::run::{Ops, slice_args};
use crate::val::{field_name, int_of, list_of, str_of};

/// Finds a file by the name TeX asks for: the name to print and the
/// bytes.
pub type Finder = Arc<dyn Fn(&str) -> Option<(String, Vec<u8>)> + Send + Sync>;

/// How a document is run.
#[derive(Clone)]
pub struct Options {
    /// The name `(…` prints for the document (`./story.tex`).
    pub name: String,
    /// `\jobname`.
    pub jobname: String,
    /// The DVI preamble's comment (`" TeX output YYYY.MM.DD:HHMM"`).
    pub comment: String,
    /// Where `\input`, `\openin` and fonts come from.
    pub finder: Finder,
    /// The job's start (`SOURCE_DATE_EPOCH` with `FORCE_SOURCE_DATE`):
    /// `\time`, `\day`, `\month`, `\year` (§241) and the PDF file's
    /// date; `None` keeps the prelude's.
    pub epoch: Option<i64>,
    /// Whether the files the job writes and reads back (`.aux`, `.toc`,
    /// ...) go round to their fixpoint (as `latexmk` runs LaTeX until
    /// they settle), or are absent before the job writes them (one run
    /// from nothing).
    pub fixpoint: bool,
    /// `\jobname` while the prelude runs (`latex` for `latex.ini`).
    pub prelude_jobname: String,
    /// A prelude's environment from [`Build::export_prelude`]: used
    /// instead of evaluating the prelude when its key matches (the same
    /// prelude text, every file it read unchanged).
    pub prelude_bytes: Option<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            name: "./texput.tex".to_owned(),
            jobname: "texput".to_owned(),
            comment: String::new(),
            finder: Arc::new(|_| None),
            epoch: None,
            fixpoint: false,
            prelude_jobname: "texput".to_owned(),
            prelude_bytes: None,
        }
    }
}

/// A value outside any table, to carry an environment between engines.
#[derive(Clone, Debug)]
enum Tree {
    Leaf(Value),
    List(Vec<Tree>),
    Seq(Vec<Tree>),
}

fn export(v: &Values, x: ValueId) -> Tree {
    match &*v.get(x) {
        Value::List(xs) => Tree::List(xs.iter().map(|&y| export(v, y)).collect()),
        Value::Seq(_) => Tree::Seq(v.seq_to_vec(x).into_iter().map(|y| export(v, y)).collect()),
        other => Tree::Leaf(other.clone()),
    }
}

fn import(v: &Values, t: &Tree) -> ValueId {
    match t {
        Tree::Leaf(x) => v.intern(x.clone()),
        Tree::List(xs) => {
            let ys = xs.iter().map(|y| import(v, y)).collect();
            v.list(ys)
        }
        Tree::Seq(xs) => {
            let mut s = ValueId::NIL;
            for y in xs {
                let e = import(v, y);
                s = v.seq_concat(s, v.seq_unit(e));
            }
            s
        }
    }
}

type Env = Arc<Vec<(String, Tree)>>;

/// A prelude's environment with what it read: each file's name and the
/// hash of what the finder gave (0: absent).
struct Cached {
    key: [u8; 32],
    text_key: u64,
    reads: Vec<(String, u64)>,
    env: Env,
}

/// Prelude environments by the hash of the prelude's text; each is good
/// while the files it read are unchanged.
fn env_cache() -> &'static Mutex<HashMap<u64, Vec<Arc<Cached>>>> {
    static C: OnceLock<Mutex<HashMap<u64, Vec<Arc<Cached>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The hash of a file as the finder gives it.
fn file_hash(f: Option<&(String, Vec<u8>)>) -> u64 {
    f.map_or(0, |(path, bytes)| {
        let mut b = path.as_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(bytes);
        phitex_defuse::hash_bytes(3, &b).max(1)
    })
}

/// The texts of the files written: the `streams` entries (`[name]` opens
/// a file empty, `[name, text]` adds to it), in the order first opened.
pub(crate) fn stream_texts(v: &Values, entries: &[ValueId]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for &e in entries {
        let xs = list_of(v, e);
        let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
        let k = if let Some(k) = out.iter().position(|(n, _)| *n == name) {
            k
        } else {
            out.push((name, String::new()));
            out.len() - 1
        };
        match xs.get(1) {
            Some(&t) => out[k].1.push_str(&str_of(v, t)),
            None => out[k].1.clear(),
        }
    }
    out
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_tree(out: &mut Vec<u8>, t: &Tree) {
    match t {
        Tree::Leaf(Value::Unit) => out.push(0),
        Tree::Leaf(Value::Bool(b)) => out.extend([1, u8::from(*b)]),
        Tree::Leaf(Value::Int(i)) => {
            out.push(2);
            out.extend_from_slice(&i.to_le_bytes());
        }
        Tree::Leaf(Value::Str(x)) => {
            out.push(3);
            put_bytes(out, x.as_bytes());
        }
        Tree::Leaf(_) => out.push(4),
        Tree::List(xs) | Tree::Seq(xs) => {
            out.push(if matches!(t, Tree::List(_)) { 5 } else { 6 });
            out.extend_from_slice(&(xs.len() as u64).to_le_bytes());
            for x in xs {
                put_tree(out, x);
            }
        }
    }
}

/// A reader over serialised bytes.
struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<String> {
        let n = usize::try_from(self.u64()?).ok()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn tree(&mut self) -> Option<Tree> {
        let tag = self.take(1)?[0];
        Some(match tag {
            0 => Tree::Leaf(Value::Unit),
            1 => Tree::Leaf(Value::Bool(self.take(1)?[0] != 0)),
            2 => Tree::Leaf(Value::Int(i64::from_le_bytes(
                self.take(8)?.try_into().ok()?,
            ))),
            3 => Tree::Leaf(Value::Str(self.string()?.into_boxed_str())),
            4 => Tree::Leaf(Value::Nil),
            5 | 6 => {
                let n = usize::try_from(self.u64()?).ok()?;
                let mut xs = Vec::with_capacity(n.min(1 << 20));
                for _ in 0..n {
                    xs.push(self.tree()?);
                }
                if tag == 5 {
                    Tree::List(xs)
                } else {
                    Tree::Seq(xs)
                }
            }
            _ => return None,
        })
    }
}

/// The key of a prelude: its text and jobname, and what it read.
fn prelude_key_of(text_key: u64, reads: &[(String, u64)]) -> [u8; 32] {
    let mut b = text_key.to_le_bytes().to_vec();
    for (n, h) in reads {
        b.extend_from_slice(n.as_bytes());
        b.push(0);
        b.extend_from_slice(&h.to_le_bytes());
    }
    let mut out = [0u8; 32];
    for (k, chunk) in out.chunks_mut(8).enumerate() {
        let seed = 11 + u64::try_from(k).unwrap_or(0);
        chunk.copy_from_slice(&phitex_defuse::hash_bytes(seed, &b).to_le_bytes());
    }
    out
}

/// `[key][reads][env]`, as [`Build::export_prelude`] writes it.
fn serialise(c: &Cached) -> Vec<u8> {
    let mut out = c.key.to_vec();
    out.extend_from_slice(&c.text_key.to_le_bytes());
    out.extend_from_slice(&(c.reads.len() as u64).to_le_bytes());
    for (n, h) in &c.reads {
        put_bytes(&mut out, n.as_bytes());
        out.extend_from_slice(&h.to_le_bytes());
    }
    out.extend_from_slice(&(c.env.len() as u64).to_le_bytes());
    for (k, t) in c.env.iter() {
        put_bytes(&mut out, k.as_bytes());
        put_tree(&mut out, t);
    }
    out
}

fn deserialise(b: &[u8]) -> Option<Cached> {
    let mut r = Rd(b);
    let key: [u8; 32] = r.take(32)?.try_into().ok()?;
    let text_key = r.u64()?;
    let n = usize::try_from(r.u64()?).ok()?;
    let mut reads = Vec::new();
    for _ in 0..n {
        let name = r.string()?;
        reads.push((name, r.u64()?));
    }
    let n = usize::try_from(r.u64()?).ok()?;
    let mut env = Vec::new();
    for _ in 0..n {
        let k = r.string()?;
        env.push((k, r.tree()?));
    }
    Some(Cached {
        key,
        text_key,
        reads,
        env: Arc::new(env),
    })
}

/// `\time`, `\day`, `\month`, `\year` at `t` seconds since 1970 (UTC,
/// as with `SOURCE_DATE_EPOCH`).
fn date_params(t: i64) -> [(&'static str, i64); 4] {
    let d = phitex_pdf::pdf_date(t);
    let n = |a: usize, b: usize| d.get(a..b).and_then(|x| x.parse().ok()).unwrap_or(0);
    [
        ("param:time", n(10, 12) * 60 + n(12, 14)),
        ("param:day", n(8, 10)),
        ("param:month", n(6, 8)),
        ("param:year", n(2, 6)),
    ]
}

/// The nodes of a text: slices starting at the pass's atoms (characters
/// run together), each with the blanks before the next.
pub(crate) fn slices(text: &str) -> Vec<(usize, usize)> {
    let defs = pass::Definitions::default();
    let scope = pass::Scope {
        document: &defs,
        file: 0,
        prelude: &defs,
    };
    let p = pass::analyze(text, pass::FileKind::Document, scope);
    let mut starts: Vec<usize> = Vec::new();
    let mut prev_char = false;
    for a in &p.atoms {
        let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
        if !(ch && prev_char) {
            starts.push(a.start);
        }
        prev_char = ch;
    }
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts.dedup();
    let mut out = Vec::with_capacity(starts.len());
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(text.len());
        if e > s {
            out.push((s, e));
        }
    }
    out
}

/// What the evaluations of one build or edit did.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// The engine's counters, summed over the evaluations.
    pub nodes_run: u64,
    pub runs: u64,
    pub cutoffs: u64,
    pub created: u64,
    pub freed: u64,
    /// Evaluations (one, plus one per round of files supplied).
    pub rounds: u64,
    /// Live nodes after.
    pub live: usize,
    /// Expansions whose tokens read on into the text after them, and how
    /// many siblings they claimed for it.
    pub tails: u64,
    pub tail_siblings: u64,
    /// Children that ran out of text (a claim measured short: a bug).
    pub starved: u64,
    /// Loop-back rounds the engine ran (the files read back, to their
    /// fixpoint).
    pub loop_rounds: u64,
    /// Nodes that waited on a value not known yet, and stops.
    pub pending_waits: u64,
    pub stops: u64,
}

impl Stats {
    fn add(&mut self, r: &Report) {
        self.nodes_run += r.nodes_run;
        self.runs += r.runs;
        self.cutoffs += r.cutoffs;
        self.created += r.created;
        self.freed += r.freed;
        self.rounds += 1;
        self.loop_rounds += r.rounds;
        self.pending_waits += r.pending_waits;
        self.stops += r.stops;
    }
}

/// An engine with the operations registered.
struct Graph {
    eng: Engine,
    ops: Ops,
    mark: OpId,
    dvi: OpId,
    files_op: OpId,
    /// The files supplied so far, with the hash of what the finder gave.
    files: Vec<(Field, ValueId)>,
    reads: Vec<(String, u64)>,
    /// The finder's answers since the last evaluation began.
    queries: Vec<(String, bool)>,
    /// The files the finder does not have.
    absent: Vec<Field>,
    /// The files the job writes and reads back: loop-back fields
    /// (`fixpoint`), or absent before the job writes them.
    written: Vec<String>,
}

impl Graph {
    fn new() -> Graph {
        let mut eng = Engine::new();
        eng.set_limits(phitex_defuse::Limits {
            // a node's children are bounded by the text it reads (one per
            // `\hyphenation` word, per character of a long run), not by a
            // constant
            children_per_node: 1 << 24,
            // TeX's own nesting limits are far below this
            unfold_depth: 4096,
            ..phitex_defuse::Limits::default()
        });
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let outpage = eng.register(Arc::new(OutPage(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi));
        let mark = eng.register(Arc::new(Mark));
        let files_op = eng.register(Arc::new(Files));
        let ops = Ops { exec, put, outpage };
        let _ = shared.set(ops);
        for f in [
            "items", "log", "needs", "defs", "pages", "writes", "tails", "streams",
        ] {
            let fld = eng.values().field(f);
            eng.declare_accumulator(fld, Arc::new(SeqMonoid));
        }
        let lines = eng.values().field("lines");
        eng.declare_accumulator(lines, Arc::new(SumMonoid));
        Graph {
            eng,
            ops,
            mark,
            dvi,
            files_op,
            files: Vec::new(),
            reads: Vec::new(),
            queries: Vec::new(),
            absent: Vec::new(),
            written: Vec::new(),
        }
    }

    /// An evaluation error as a reader can act on it: the error, and
    /// each node it names with what it is.
    fn diagnose(&self, e: &phitex_defuse::EvalError) -> String {
        use phitex_defuse::EvalError as E;
        let mut ids: Vec<NodeId> = Vec::new();
        match e {
            E::Cycle(ns) => ids.extend(ns.iter().copied()),
            E::RunawayNode { node, .. }
            | E::RunawayCreation { node, .. }
            | E::UndeclaredDefinition { node, .. } => ids.push(*node),
            E::NoProgress { waiters, .. } | E::Deadline { waiters, .. } => {
                ids.extend(waiters.iter().take(20).map(|w| w.node));
            }
            _ => {}
        }
        let mut out = format!("evaluation: {e}\n{e:?}");
        let v = self.eng.values();
        for id in ids {
            let what = self.eng.node_info(id).map_or_else(
                || "gone".to_owned(),
                |i| {
                    let args: Vec<String> = i
                        .args
                        .iter()
                        .map(|&a| v.render(a).chars().take(200).collect())
                        .collect();
                    format!(
                        "{} {:?}, parent {:?}, args {}",
                        i.op,
                        i.state,
                        i.parent,
                        args.join(" ")
                    )
                },
            );
            let _ = write!(out, "\n  {id:?}: {what}");
        }
        out
    }

    /// The names of the files the job opened for writing.
    fn opened(&self, unit: UnitId) -> Vec<String> {
        let v = self.eng.values();
        let f = v.field("streams");
        let entries = self
            .eng
            .acc_value(unit, f)
            .map(|s| v.seq_to_vec(s))
            .unwrap_or_default();
        stream_texts(v, &entries)
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    }

    /// A file the job writes is read back: from the previous round
    /// (`fixpoint`: a loop-back field, absent in round one, as LaTeX's
    /// `\@input` finds no `.aux` on a first run), or absent.
    fn written_file(&mut self, name: &str, fixpoint: bool) {
        let v = self.eng.values();
        let f = v.field(&format!("file:{name}"));
        self.files.retain(|(g, _)| *g != f);
        self.absent.retain(|g| *g != f);
        self.reads.retain(|(n, _)| n != name);
        if fixpoint {
            let absent = v.list(Vec::new());
            self.eng.declare_loopback(f, absent);
        } else {
            self.absent.push(f);
        }
        self.written.push(name.to_owned());
    }

    /// The environment: the files supplied, and the loop-back files.
    fn environment(&self, fixpoint: bool) -> Vec<(Field, ValueId)> {
        let v = self.eng.values();
        let mut env = self.files.clone();
        if fixpoint {
            let names = self.written.iter().map(|n| v.str(n)).collect();
            env.push((v.field("loopfiles"), v.list(names)));
        }
        env
    }

    /// Evaluates; when the graph asks for files, supplies them and
    /// evaluates again.
    fn settle(&mut self, unit: UnitId, finder: &Finder, stats: &mut Stats, fixpoint: bool) {
        let needs_f = self.eng.values().field("needs");
        loop {
            let t0 = std::time::Instant::now();
            let r = match self.eng.evaluate_sequential() {
                Ok(r) => r,
                Err(e) => panic!("{}", self.diagnose(&e)),
            };
            if std::env::var_os("PHITEX_TEX_TIME").is_some() {
                eprintln!("round: {:?}, {} nodes run", t0.elapsed(), r.nodes_run);
            }
            stats.add(&r);
            let opened = self.opened(unit);
            let mut asked = false;
            // a file supplied from the finder that the job turns out to
            // write is read back instead
            for name in &opened {
                if self.written.contains(name) {
                    continue;
                }
                let f = self.eng.values().field(&format!("file:{name}"));
                if self.files.iter().any(|(g, _)| *g == f) || self.absent.contains(&f) {
                    self.written_file(name, fixpoint);
                    asked = true;
                }
            }
            let asked_names: Vec<String> = {
                let v = self.eng.values();
                r.env_waits
                    .iter()
                    .filter_map(|&(_, f)| field_name(v, f).strip_prefix("file:").map(str::to_owned))
                    .collect()
            };
            if std::env::var_os("PHITEX_TEX_TIME").is_some() {
                eprintln!("  waits: {} {:?}", r.env_stalled, asked_names);
            }
            let v = self.eng.values();
            let needs = self
                .eng
                .acc_value(unit, needs_f)
                .map(|n| v.seq_to_vec(n))
                .unwrap_or_default();
            let needs: Vec<(String, String)> = needs
                .into_iter()
                .map(|x| {
                    let xs = list_of(v, x);
                    (
                        xs.first().map(|&k| str_of(v, k)).unwrap_or_default(),
                        xs.get(2).map(|&n| str_of(v, n)).unwrap_or_default(),
                    )
                })
                .collect();
            for (kind, _) in &needs {
                if kind == "starve" {
                    stats.starved += 1;
                }
            }
            for name in asked_names {
                let f = self.eng.values().field(&format!("file:{name}"));
                if self.files.iter().any(|(g, _)| *g == f)
                    || self.absent.contains(&f)
                    || self.written.contains(&name)
                {
                    continue;
                }
                if opened.contains(&name) {
                    self.written_file(&name, fixpoint);
                    asked = true;
                    continue;
                }
                let found = finder(&name);
                self.queries.push((name.clone(), found.is_some()));
                let v = self.eng.values();
                self.reads.push((name.clone(), file_hash(found.as_ref())));
                if let Some((path, bytes)) = found {
                    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
                    let val = v.list(vec![v.str(&path), v.str(&text)]);
                    self.files.push((f, val));
                } else {
                    self.absent.push(f);
                }
                asked = true;
            }
            if !asked {
                break;
            }
            let env = self.environment(fixpoint);
            self.eng.set_environment(env);
            self.eng.set_environment_absent(self.absent.clone());
        }
        let v = self.eng.values();
        let tails_f = v.field("tails");
        if let Some(t) = self.eng.acc_value(unit, tails_f) {
            for x in v.seq_to_vec(t) {
                stats.tails += 1;
                stats.tail_siblings += u64::try_from(int_of(v, x)).unwrap_or(0);
            }
        }
    }

    /// The text's nodes pushed onto `unit`.
    fn push_text(&mut self, unit: UnitId, text: &str) -> Vec<(String, NodeId)> {
        let mut out = Vec::new();
        let t0 = std::time::Instant::now();
        let sl = slices(text);
        let t1 = t0.elapsed();
        for (s, e) in sl {
            let piece = &text[s..e];
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            if std::env::var_os("PHITEX_TEX_TRACE").is_some() {
                eprintln!("node {piece:?}");
            }
            let args = slice_args(self.eng.values(), piece, starts);
            let n = self.eng.push(unit, self.ops.exec, args);
            out.push((key(piece, starts), n));
        }
        if std::env::var_os("PHITEX_TEX_TIME").is_some() {
            eprintln!(
                "push_text: slices {t1:?}, push {:?}, {} nodes",
                t0.elapsed().checked_sub(t1).unwrap(),
                out.len()
            );
        }
        out
    }
}

/// A node's identity for edits: its text and whether it begins a line.
fn key(piece: &str, starts: bool) -> String {
    format!("{}{piece}", u8::from(starts))
}

/// A text as TeX reads a file: every line ends (§31).
fn as_file(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

/// INITEX's state, and the prelude's on top of it: a format, `\dump`
/// ending it (or its text's end). Cached by the prelude's text, and good
/// while every file it read is what the finder gives now.
fn prelude_env(prelude: Option<&str>, opts: &Options) -> Arc<Cached> {
    let finder = &opts.finder;
    let key = prelude.map_or(0, |p| {
        let mut b = p.as_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(opts.prelude_jobname.as_bytes());
        phitex_defuse::hash_bytes(7, &b)
    });
    let candidates: Vec<Arc<Cached>> = env_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let given = opts
        .prelude_bytes
        .as_deref()
        .and_then(deserialise)
        .filter(|c| c.text_key == key)
        .map(Arc::new);
    for c in given.into_iter().chain(candidates) {
        if c.reads
            .iter()
            .all(|(n, h)| file_hash(finder(n).as_ref()) == *h)
        {
            return c;
        }
    }
    let mut g = Graph::new();
    let unit = g.eng.new_unit();
    let mut env: Vec<(String, ValueId)> = crate::val::initex(g.eng.values());
    for (name, x) in &env {
        let f = g.eng.values().field(name);
        g.eng.set_env(unit, f, *x);
    }
    if let Some(p) = prelude {
        {
            let v = g.eng.values();
            let (f, x) = (v.field("jobname"), v.str(&opts.prelude_jobname));
            g.eng.set_env(unit, f, x);
        }
        g.push_text(unit, &as_file(p));
        // the point the prelude's final bindings are read at
        let end = g.eng.push(unit, g.mark, Vec::new());
        let mut stats = Stats::default();
        let t0 = std::time::Instant::now();
        g.settle(unit, finder, &mut stats, false);
        if std::env::var_os("PHITEX_TEX_TIME").is_some() {
            eprintln!("prelude settle {:?}", t0.elapsed());
        }
        let v = g.eng.values();
        let defs_f = v.field("defs");
        let mut seen = std::collections::HashSet::new();
        if let Some(d) = g.eng.acc_value(unit, defs_f) {
            for l in v.seq_to_vec(d) {
                for n in list_of(v, l) {
                    if !seen.insert(n) {
                        continue;
                    }
                    let f = Field(n);
                    let name = field_name(v, f);
                    // open files are not dumped
                    // nor is the PDF writer's state (pdfTeX dumps no objects)
                    if ["wopen:", "write:", "file:", "read:", "pdf:state"]
                        .iter()
                        .any(|p| name.starts_with(p))
                    {
                        continue;
                    }
                    let x = g
                        .eng
                        .reaching_def(end, f)
                        .and_then(|d| g.eng.def_value(d, f));
                    if let Some(x) = x {
                        env.retain(|(k, _)| *k != name);
                        env.push((name, x));
                    }
                }
            }
        }
    }
    let v = g.eng.values();
    let out: Vec<(String, Tree)> = env.into_iter().map(|(k, x)| (k, export(v, x))).collect();
    let e = Arc::new(out);
    let c = Arc::new(Cached {
        key: prelude_key_of(key, &g.reads),
        text_key: key,
        reads: g.reads.clone(),
        env: e,
    });
    env_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key)
        .or_default()
        .push(c.clone());
    c
}

/// A document built on the graph, kept for edits.
pub struct Build {
    g: Graph,
    unit: UnitId,
    opts: Options,
    /// The nodes of the text, with their keys.
    nodes: Vec<(String, NodeId)>,
    dvi: NodeId,
    prelude: Arc<Cached>,
    /// What the last build or edit ran.
    pub stats: Stats,
}

impl Build {
    /// A cold build of `document` after `prelude` (`None`: INITEX).
    #[must_use]
    pub fn new(prelude: Option<&str>, document: &str) -> Build {
        Build::with(Options::default(), prelude, document)
    }

    /// [`Build::new`] with options.
    ///
    /// # Panics
    ///
    /// If the engine reports an evaluation error (a cycle).
    #[must_use]
    pub fn with(opts: Options, prelude: Option<&str>, document: &str) -> Build {
        let t0 = std::time::Instant::now();
        let cached = prelude_env(prelude, &opts);
        let env = cached.env.clone();
        let t1 = t0.elapsed();
        let mut g = Graph::new();
        let unit = g.eng.new_unit();
        {
            let v = g.eng.values();
            let mut sets: Vec<(Field, ValueId)> = Vec::new();
            for (k, t) in env.iter() {
                sets.push((v.field(k), import(v, t)));
            }
            sets.push((v.field("jobname"), v.str(&opts.jobname)));
            sets.push((v.field("dvicomment"), v.str(&opts.comment)));
            sets.push((
                v.field("pdfdate"),
                v.str(&phitex_pdf::pdf_date(opts.epoch.unwrap_or(0))),
            ));
            if let Some(t) = opts.epoch {
                // §241 fix_date_and_time, at the job's start
                for (k, x) in date_params(t) {
                    sets.push((v.field(k), v.int(x)));
                }
            }
            for (f, x) in sets {
                g.eng.set_env(unit, f, x);
            }
        }
        // §1337: `\everyjob`, before the job's first line
        let every = env
            .iter()
            .find(|(k, _)| k == "param:everyjob")
            .map(|(_, t)| import(g.eng.values(), t));
        if let Some(t) = every {
            let v = g.eng.values();
            let toks = crate::val::toks_of(v, Some(t));
            if !toks.is_empty() {
                let frame = v.list(vec![v.str("t"), crate::val::toks_value(v, &toks)]);
                let args = vec![v.list(vec![frame]), v.int(1)];
                let exec = g.ops.exec;
                g.eng.push(unit, exec, args);
            }
        }
        let nodes = g.push_text(unit, &as_file(document));
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        g.eng.add_root(dvi);
        let files_op = g.files_op;
        let files = g.eng.push(unit, files_op, Vec::new());
        g.eng.add_root(files);
        let mut b = Build {
            g,
            unit,
            opts,
            nodes,
            dvi,
            prelude: cached,
            stats: Stats::default(),
        };
        b.settle();
        if std::env::var_os("PHITEX_TEX_TIME").is_some() {
            eprintln!(
                "time: prelude {t1:?}, document {:?}",
                t0.elapsed().checked_sub(t1).unwrap()
            );
        }
        b
    }

    fn settle(&mut self) {
        let mut stats = Stats::default();
        self.g.queries.clear();
        self.g
            .settle(self.unit, &self.opts.finder, &mut stats, self.opts.fixpoint);
        stats.live = self.g.eng.live_nodes();
        self.stats = stats;
    }

    /// The document changed to `text`: the nodes whose slices differ are
    /// replaced, inserted or removed, and the graph re-evaluated.
    pub fn edit(&mut self, text: &str) {
        let text = as_file(text);
        let new: Vec<(String, (usize, usize))> = slices(&text)
            .into_iter()
            .map(|(s, e)| {
                let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
                (key(&text[s..e], starts), (s, e))
            })
            .collect();
        let old = &self.nodes;
        let mut a = 0;
        while a < old.len() && a < new.len() && old[a].0 == new[a].0 {
            a += 1;
        }
        let mut b = 0;
        while b < old.len() - a
            && b < new.len() - a
            && old[old.len() - 1 - b].0 == new[new.len() - 1 - b].0
        {
            b += 1;
        }
        let (old_mid, new_mid) = (old.len() - a - b, new.len() - a - b);
        let common = old_mid.min(new_mid);
        let exec = self.g.ops.exec;
        let mut nodes = self.nodes[..a].to_vec();
        let args_of = |g: &Graph, k: usize| {
            let (s, e) = new[k].1;
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            slice_args(g.eng.values(), &text[s..e], starts)
        };
        for k in 0..common {
            let n = self.nodes[a + k].1;
            let args = args_of(&self.g, a + k);
            self.g.eng.replace(n, exec, args);
            nodes.push((new[a + k].0.clone(), n));
        }
        for k in common..old_mid {
            self.g.eng.remove(self.nodes[a + k].1);
        }
        for k in common..new_mid {
            let args = args_of(&self.g, a + k);
            let n = if let Some(&(_, p)) = nodes.last() {
                self.g.eng.insert_after(p, exec, args)
            } else {
                let first = self.nodes.get(a + old_mid).map_or(self.dvi, |x| x.1);
                self.g.eng.insert_before(first, exec, args)
            };
            nodes.push((new[a + k].0.clone(), n));
        }
        nodes.extend_from_slice(&self.nodes[a + old_mid..]);
        self.nodes = nodes;
        self.settle();
    }

    /// A file other than the document changed (`None`: as the finder
    /// gives it again): its `file:` field is replaced, and the nodes that
    /// read it run again.
    pub fn edit_file(&mut self, name: &str, text: Option<&[u8]>) {
        let v = self.g.eng.values();
        let f = v.field(&format!("file:{name}"));
        let k = if let Some(k) = self.g.files.iter().position(|(g, _)| *g == f) {
            k
        } else if self.g.absent.contains(&f) {
            self.g.absent.retain(|g| *g != f);
            self.g.files.push((f, v.list(Vec::new())));
            self.g.files.len() - 1
        } else {
            return;
        };
        let old = list_of(v, self.g.files[k].1);
        let found = match text {
            Some(t) => Some((
                old.first()
                    .map_or_else(|| format!("./{name}"), |&p| str_of(v, p)),
                t.to_vec(),
            )),
            None => (self.opts.finder)(name),
        };
        let val = match &found {
            Some((path, bytes)) => {
                let t: String = bytes.iter().map(|&b| char::from(b)).collect();
                v.list(vec![v.str(path), v.str(&t)])
            }
            None => v.list(Vec::new()),
        };
        if found.is_none() {
            self.g.files.remove(k);
            self.g.absent.push(f);
        } else {
            self.g.files[k].1 = val;
        }
        if let Some(r) = self.g.reads.iter_mut().find(|(n, _)| n == name) {
            r.1 = file_hash(found.as_ref());
        }
        let env = self.g.environment(self.opts.fixpoint);
        self.g.eng.set_environment(env);
        let absent = self.g.absent.clone();
        self.g.eng.set_environment_absent(absent);
        self.settle();
    }

    /// The finder's answers in the last evaluation: each name asked for,
    /// and whether it was found.
    #[must_use]
    pub fn reads(&self) -> Vec<(String, bool)> {
        self.g.queries.clone()
    }

    /// The prelude's key: its text and every file it read.
    #[must_use]
    pub fn prelude_key(&self) -> [u8; 32] {
        self.prelude.key
    }

    /// The prelude's environment, serialised, for
    /// [`Options::prelude_bytes`].
    #[must_use]
    pub fn export_prelude(&self) -> Vec<u8> {
        serialise(&self.prelude)
    }

    /// The files the job wrote (`\openout`, `\write`), with their text,
    /// in the order first opened: what TeX leaves in them.
    #[must_use]
    pub fn files(&self) -> Vec<(String, String)> {
        let v = self.g.eng.values();
        let f = v.field("streams");
        let entries = self
            .g
            .eng
            .acc_value(self.unit, f)
            .map(|s| v.seq_to_vec(s))
            .unwrap_or_default();
        stream_texts(v, &entries)
    }

    /// Whether the output is a PDF file (`\pdfoutput` at the first
    /// `\shipout`); else DVI.
    #[must_use]
    pub fn is_pdf(&self) -> bool {
        self.output().0.starts_with(b"%PDF")
    }

    /// The output file (DVI or PDF) and the log.
    #[must_use]
    pub fn output(&self) -> (Vec<u8>, String) {
        let v = self.g.eng.values();
        let dvi = self
            .g
            .eng
            .output(self.dvi)
            .map(|x| {
                str_of(v, x)
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        let log_f = v.field("log");
        let log = crate::log::layout(v, &self.opts.name, self.g.eng.acc_value(self.unit, log_f));
        (dvi, log)
    }

    /// The `\write` streams' lines: `(stream, text)`.
    #[must_use]
    pub fn writes(&self) -> Vec<(i64, String)> {
        let v = self.g.eng.values();
        let f = v.field("writes");
        self.g
            .eng
            .acc_value(self.unit, f)
            .map(|s| {
                v.seq_to_vec(s)
                    .into_iter()
                    .map(|e| {
                        let xs = list_of(v, e);
                        (
                            xs.first().map_or(0, |&k| int_of(v, k)),
                            xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Nodes live in the graph.
    #[must_use]
    pub fn live_nodes(&self) -> usize {
        self.g.eng.live_nodes()
    }
}
