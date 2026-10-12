//! The client: a document's text as nodes, the prelude's environment,
//! the files the graph asks for, edits, and the outputs read back (DVI
//! bytes, the log).
//!
//! Nodes come from the syntactic pass ([`phitex_source::pass`]): one per
//! control sequence, one per run of characters, one per space, brace or
//! blank line, each holding its slice of the text (and the blanks after
//! it). A node reads its own slice and the siblings its command claims.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{
    Engine, Field, NodeId, OpId, Report, SeqMonoid, SumMonoid, UnitId, Value, ValueId, Values,
};
use phitex_source::pass::{self, AtomKind};

use crate::ops::{Dvi, Exec, Mark, OutPage, Put, Shared};
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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            name: "./texput.tex".to_owned(),
            jobname: "texput".to_owned(),
            comment: String::new(),
            finder: Arc::new(|_| None),
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

/// Prelude environments by the hash of the prelude's text.
fn env_cache() -> &'static Mutex<HashMap<u64, Env>> {
    static C: OnceLock<Mutex<HashMap<u64, Env>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
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
}

impl Stats {
    fn add(&mut self, r: &Report) {
        self.nodes_run += r.nodes_run;
        self.runs += r.runs;
        self.cutoffs += r.cutoffs;
        self.created += r.created;
        self.freed += r.freed;
        self.rounds += 1;
    }
}

/// An engine with the operations registered.
struct Graph {
    eng: Engine,
    ops: Ops,
    mark: OpId,
    dvi: OpId,
    /// The files supplied so far.
    files: Vec<(Field, ValueId)>,
}

impl Graph {
    fn new() -> Graph {
        let mut eng = Engine::new();
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let outpage = eng.register(Arc::new(OutPage(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi));
        let mark = eng.register(Arc::new(Mark));
        let ops = Ops { exec, put, outpage };
        let _ = shared.set(ops);
        for f in ["items", "log", "needs", "defs", "pages", "writes", "tails"] {
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
            files: Vec::new(),
        }
    }

    /// Evaluates; when the graph asks for files, supplies them and
    /// evaluates again.
    fn settle(&mut self, unit: UnitId, finder: &Finder, stats: &mut Stats) {
        let needs_f = self.eng.values().field("needs");
        loop {
            let t0 = std::time::Instant::now();
            let r = self
                .eng
                .evaluate_sequential()
                .unwrap_or_else(|e| panic!("evaluation: {e:?}"));
            if std::env::var_os("PHITEX_TEX_TIME").is_some() {
                eprintln!("round: {:?}, {} nodes run", t0.elapsed(), r.nodes_run);
            }
            stats.add(&r);
            let v = self.eng.values();
            let needs = self
                .eng
                .acc_value(unit, needs_f)
                .map(|n| v.seq_to_vec(n))
                .unwrap_or_default();
            let mut asked = false;
            for x in needs {
                let xs = list_of(v, x);
                let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
                if kind == "starve" {
                    stats.starved += 1;
                }
                if kind != "file" {
                    continue;
                }
                let name = xs.get(2).map(|&n| str_of(v, n)).unwrap_or_default();
                let f = v.field(&format!("file:{name}"));
                if self.files.iter().any(|(g, _)| *g == f) {
                    continue;
                }
                let val = match finder(&name) {
                    Some((path, bytes)) => {
                        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
                        v.list(vec![v.str(&path), v.str(&text)])
                    }
                    None => v.list(Vec::new()),
                };
                self.files.push((f, val));
                asked = true;
            }
            if !asked {
                break;
            }
            let env = self.files.clone();
            self.eng.set_environment(env);
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

/// INITEX's state, and the prelude's on top of it.
fn prelude_env(prelude: Option<&str>, finder: &Finder) -> Env {
    let key = prelude.map_or(0, |p| phitex_defuse::hash_bytes(7, p.as_bytes()));
    if let Some(e) = env_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return e.clone();
    }
    let mut g = Graph::new();
    let unit = g.eng.new_unit();
    let mut env: Vec<(String, ValueId)> = crate::val::initex(g.eng.values());
    for (name, x) in &env {
        let f = g.eng.values().field(name);
        g.eng.set_env(unit, f, *x);
    }
    if let Some(p) = prelude {
        g.push_text(unit, &as_file(p));
        // the point the prelude's final bindings are read at
        let end = g.eng.push(unit, g.mark, Vec::new());
        let mut stats = Stats::default();
        let t0 = std::time::Instant::now();
        g.settle(unit, finder, &mut stats);
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
    env_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, e.clone());
    e
}

/// A document built on the graph, kept for edits.
pub struct Build {
    g: Graph,
    unit: UnitId,
    opts: Options,
    /// The nodes of the text, with their keys.
    nodes: Vec<(String, NodeId)>,
    dvi: NodeId,
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
        let env = prelude_env(prelude, &opts.finder);
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
            for (f, x) in sets {
                g.eng.set_env(unit, f, x);
            }
        }
        let nodes = g.push_text(unit, &as_file(document));
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        g.eng.add_root(dvi);
        let mut b = Build {
            g,
            unit,
            opts,
            nodes,
            dvi,
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
        self.g.settle(self.unit, &self.opts.finder, &mut stats);
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

    /// The DVI file and the log.
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
