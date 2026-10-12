//! The client: a document's lines as nodes, the prelude's environment,
//! the needs the graph reports (files, more input), edits, and the
//! outputs read back (DVI bytes, the log).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Engine, NodeId, OpId, Report, SeqMonoid, SumMonoid, UnitId, Value, ValueId, Values};

use crate::ops::{Dvi, Exec, Fin, Mark, OutPage, Put, Shared};
use crate::run::{Ops, line_args};
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

/// The lines of a text, as TeX reads a file (§31).
pub(crate) fn lines_of(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if text.ends_with('\n') {
        v.pop();
    }
    v
}

/// What the evaluations of one build or edit did.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// The engine's counters, summed over the client's rounds.
    pub nodes_run: u64,
    pub runs: u64,
    pub created: u64,
    /// Client rounds (needs answered).
    pub rounds: u64,
    /// Parked nodes woken by the client.
    pub pokes: u64,
    /// Of those, wakes that had to re-read every node.
    pub full_pokes: u64,
    /// Live nodes after.
    pub live: usize,
}

impl Stats {
    fn add(&mut self, r: &Report) {
        self.nodes_run += r.nodes_run;
        self.runs += r.runs;
        self.created += r.created;
        self.rounds += 1;
    }
}

/// An engine with the operations registered.
struct Graph {
    eng: Engine,
    ops: Ops,
    mark: OpId,
    dvi: OpId,
    /// The hints given, by node.
    hints: HashMap<u64, i64>,
    /// The page builder's root, in a document.
    fin: Option<NodeId>,
    /// The client's nodes' texts.
    args: HashMap<u64, Vec<ValueId>>,
    /// Files answered.
    answered: std::collections::HashSet<String>,
}

impl Graph {
    fn new() -> Graph {
        let mut eng = Engine::new();
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let fin = eng.register(Arc::new(Fin(shared.clone())));
        let outpage = eng.register(Arc::new(OutPage(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi));
        let mark = eng.register(Arc::new(Mark));
        let ops = Ops { exec, put, fin, outpage };
        let _ = shared.set(ops);
        for f in ["items", "log", "needs", "defs", "pages", "writes"] {
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
            hints: HashMap::new(),
            fin: None,
            args: HashMap::new(),
            answered: std::collections::HashSet::new(),
        }
    }

    /// Evaluates, answering the graph's needs until it has none.
    fn settle(&mut self, unit: UnitId, top: &[NodeId], finder: &Finder, stats: &mut Stats) {
        let needs_f = self.eng.values().field("needs");
        loop {
            let r = match self.eng.evaluate_sequential() {
                Ok(r) => r,
                Err(e) => {
                    if let phitex_defuse::EvalError::Cycle(ids) = &e
                        && stats.pokes < 10_000
                    {
                        // REDUCE-GAP: a node parked on a node that a claim
                        // then makes inert is never woken (reported as a
                        // cycle); the client wakes it through its
                        // `hint:` field.
                        stats.pokes += 1;
                        let mut texts: Vec<Vec<ValueId>> = Vec::new();
                        let mut all = false;
                        for id in ids {
                            if let Some(a) = self.args.get(&id.raw()) {
                                texts.push(a.clone());
                            } else if !self.fin.is_some_and(|f| f == *id) {
                                all = true;
                            }
                        }
                        let (exec, keep) = (self.ops.exec, [self.ops.fin, self.ops.outpage, self.dvi, self.mark]);
                        if all {
                            stats.full_pokes += 1;
                        }
                        self.eng.refresh_where(&|op, args, _| {
                            keep.contains(&op) || (op == exec && (all || texts.iter().any(|t| t[..] == *args)))
                        });
                        continue;
                    }
                    if let phitex_defuse::EvalError::Cycle(ids) = &e {
                        for id in ids {
                            let at = top.iter().position(|t| t == id);
                            eprintln!("cycle node {} top {at:?} kids {:?}", id.raw(), self.eng.children(*id).len());
                        }
                    }
                    panic!("evaluation: {e:?}");
                }
            };
            stats.add(&r);
            let v = self.eng.values();
            let Some(needs) = self.eng.acc_value(unit, needs_f) else { break };
            let needs = v.seq_to_vec(needs);
            if needs.is_empty() {
                break;
            }
            let trace = std::env::var_os("PHITEX_TEX_TRACE").is_some();
            if trace {
                eprintln!("round {}: {} needs (ran {})", stats.rounds, needs.len(), r.nodes_run);
            }
            // every need of the round is answered: a need after another
            // may be spurious (its node ran on what the first one had not
            // yet done), and answering it is harmless
            let mut starving = Vec::new();
            let mut files = Vec::new();
            let mut fatal = false;
            for x in needs {
                let xs = list_of(v, x);
                let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
                let raw = xs.get(1).map_or(0, |&r| int_of(v, r));
                match kind.as_str() {
                    "fatal" => fatal = true,
                    "file" => files.push(xs.get(2).map(|&n| str_of(v, n)).unwrap_or_default()),
                    _ => starving.push(raw),
                }
            }
            if fatal && starving.is_empty() && files.is_empty() {
                break;
            }
            let mut sets = Vec::new();
            for name in files {
                let f = v.field(&format!("file:{name}"));
                if self.eng.values().get(f.0).as_ref() != &Value::Unit && self.answered.contains(&name) {
                    continue;
                }
                self.answered.insert(name.clone());
                let val = match finder(&name) {
                    Some((path, bytes)) => {
                        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
                        v.list(vec![v.str(&path), v.str(&text)])
                    }
                    None => v.list(Vec::new()),
                };
                sets.push((f, val));
            }
            let progress = !sets.is_empty() || !starving.is_empty();
            for (f, x) in sets {
                self.eng.set_env(unit, f, x);
            }
            if !starving.is_empty() {
                self.feed(unit, top, &starving, stats.rounds);
            }
            if !progress {
                break;
            }
        }
    }

    fn feed(&mut self, unit: UnitId, top: &[NodeId], raws: &[i64], round: u64) {
        let nonce = i64::try_from(round).unwrap_or(0);
        let mut parent: HashMap<u64, Option<NodeId>> = HashMap::new();
        let mut sibs: HashMap<u64, (Vec<NodeId>, usize)> = HashMap::new();
        let mut stack: Vec<(Vec<NodeId>, Option<NodeId>)> = vec![(top.to_vec(), None)];
        if let Some(f) = self.fin {
            // the page builder's children (output routines): a list of
            // their own, nothing after it to claim
            stack.push((vec![f], None));
        }
        let mut targets: HashMap<u64, NodeId> = HashMap::new();
        let want: std::collections::HashSet<u64> = raws.iter().map(|r| u64::from_ne_bytes(r.to_ne_bytes())).collect();
        while let Some((list, p)) = stack.pop() {
            for (i, &n) in list.iter().enumerate() {
                parent.insert(n.raw(), p);
                sibs.insert(n.raw(), (list.clone(), i));
                if want.contains(&n.raw()) {
                    targets.insert(n.raw(), n);
                }
                let kids = self.eng.children(n);
                if !kids.is_empty() {
                    stack.push((kids, Some(n)));
                }
            }
        }
        let mut sets = Vec::new();
        let mut given: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for &raw in raws {
        let Some(&start) = targets.get(&u64::from_ne_bytes(raw.to_ne_bytes())) else { continue };
        let v = self.eng.values();
        let mut chain = Vec::new();
        let mut cur = Some(start);
        let mut found = false;
        while let Some(n) = cur {
            chain.push(n);
            let claimed = self.eng.claimed(n).len();
            let (list, i) = sibs.get(&n.raw()).cloned().unwrap_or_default();
            let after = list.len().saturating_sub(i + 1);
            if std::env::var_os("PHITEX_TEX_TRACE").is_some() {
                eprintln!("  node {} claimed {claimed} index {i} of {} kids {}", n.raw(), list.len(), self.eng.children(n).len());
            }
            let asked = usize::try_from(self.hints.get(&n.raw()).copied().unwrap_or(0)).unwrap_or(0);
            if after > claimed.max(asked) {
                found = true;
                break;
            }
            cur = parent.get(&n.raw()).copied().flatten();
        }
        if !found {
            let f = v.field(&format!("hint:{raw}"));
            let m1 = v.list(vec![v.int(-1), v.int(nonce)]);
            sets.push((f, m1));
            continue;
        }
        for n in chain {
            if !given.insert(n.raw()) {
                continue;
            }
            let claimed = self.eng.claimed(n).len();
            let r = i64::from_ne_bytes(n.raw().to_ne_bytes());
            let f = v.field(&format!("hint:{r}"));
            // the round makes the value new: a node whose sibling has
            // only now appeared runs again
            let prev = self.hints.get(&n.raw()).copied().unwrap_or(0);
            // doubling: a construct over many lines takes few rounds
            let k = prev.max(i64::try_from(claimed).unwrap_or(0)) + prev.max(1);
            self.hints.insert(n.raw(), k);
            let h = v.list(vec![v.int(k), v.int(nonce)]);
            sets.push((f, h));
        }
        }
        for (f, h) in sets {
            self.eng.set_env(unit, f, h);
        }
    }
}

/// A document built on the graph, kept for edits.
pub struct Build {
    g: Graph,
    unit: UnitId,
    opts: Options,
    lines: Vec<String>,
    nodes: Vec<NodeId>,
    fin: NodeId,
    dvi: NodeId,
    hints_set: Vec<String>,
    /// What the last build or edit ran.
    pub stats: Stats,
}

/// INITEX's state, and the prelude's on top of it.
fn prelude_env(prelude: Option<&str>, finder: &Finder) -> Env {
    let key = prelude.map_or(0, |p| phitex_defuse::hash_bytes(7, p.as_bytes()));
    if let Some(e) = env_cache().lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
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
        // REDUCE-GAP: see `Build::with`: a no-op node heads the unit.
        let _ = g.eng.push(unit, g.ops.exec, Vec::new());
        let mut top = Vec::new();
        for l in lines_of(p) {
            let args = line_args(g.eng.values(), &l);
            let n = g.eng.push(unit, g.ops.exec, args.clone());
            g.args.insert(n.raw(), args);
            top.push(n);
        }
        let end = g.eng.push(unit, g.mark, Vec::new());
        let mut stats = Stats::default();
        g.settle(unit, &top, finder, &mut stats);
        let v = g.eng.values();
        let defs_f = v.field("defs");
        let mut seen = std::collections::HashSet::new();
        if let Some(d) = g.eng.acc_value(unit, defs_f) {
            for l in v.seq_to_vec(d) {
                for n in list_of(v, l) {
                    if !seen.insert(n) {
                        continue;
                    }
                    let f = phitex_defuse::Field(n);
                    let name = field_name(v, f);
                    if name.starts_with("hint:") || name == "ended" {
                        continue;
                    }
                    let x = match g.eng.reaching_def(end, f) {
                        Some(d) => g.eng.def_value(d, f),
                        None => None,
                    };
                    if let Some(x) = x {
                        env.retain(|(k, _)| *k != name);
                        env.push((name, x));
                    }
                }
            }
        }
        // the files the prelude read stay found
        let _ = top;
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

impl Build {
    /// A cold build of `document` after `prelude` (`None`: INITEX).
    #[must_use]
    pub fn new(prelude: Option<&str>, document: &str) -> Build {
        Build::with(Options::default(), prelude, document)
    }

    /// [`Build::new`] with options.
    #[must_use]
    pub fn with(opts: Options, prelude: Option<&str>, document: &str) -> Build {
        let env = prelude_env(prelude, &opts.finder);
        let mut g = Graph::new();
        let unit = g.eng.new_unit();
        {
            let v = g.eng.values();
            let mut sets: Vec<(phitex_defuse::Field, ValueId)> = Vec::new();
            for (k, t) in env.iter() {
                sets.push((v.field(k), import(v, t)));
            }
            sets.push((v.field("jobname"), v.str(&opts.jobname)));
            sets.push((v.field("dvicomment"), v.str(&opts.comment)));
            for (f, x) in sets {
                g.eng.set_env(unit, f, x);
            }
        }
        let lines = lines_of(document);
        // REDUCE-GAP: when a unit's first node resolves a pending claim,
        // the nodes after its claim stay parked (reported as a cycle);
        // a no-op node heads every unit.
        let head = g.eng.push(unit, g.ops.exec, Vec::new());
        let _ = head;
        let mut nodes = Vec::new();
        for l in &lines {
            let args = line_args(g.eng.values(), l);
            let n = g.eng.push(unit, g.ops.exec, args.clone());
            g.args.insert(n.raw(), args);
            nodes.push(n);
        }
        // REDUCE-GAP: a range read (`acc_range`) that parks while a node
        // in its range is still to be claimed is not woken when the claim
        // makes that node inert; a no-op node between the text and the
        // page builder finishes after every claim and wakes it.
        let _ = g.eng.push(unit, g.mark, Vec::new());
        let fin = g.eng.push(unit, g.ops.fin, Vec::new());
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        g.eng.add_root(fin);
        g.fin = Some(fin);
        g.eng.add_root(dvi);
        let mut b = Build {
            g,
            unit,
            opts,
            lines,
            nodes,
            fin,
            dvi,
            hints_set: Vec::new(),
            stats: Stats::default(),
        };
        b.settle();
        b
    }

    fn settle(&mut self) {
        let mut stats = Stats::default();
        let top = self.nodes.clone();
        self.g.settle(self.unit, &top, &self.opts.finder, &mut stats);
        stats.live = self.g.eng.live_nodes();
        self.stats = stats;
    }

    /// The document changed to `text`: the lines that differ are
    /// replaced, inserted or removed, and the graph re-evaluated.
    pub fn edit(&mut self, text: &str) {
        let new = lines_of(text);
        let old = &self.lines;
        let mut a = 0;
        while a < old.len() && a < new.len() && old[a] == new[a] {
            a += 1;
        }
        let mut b = 0;
        while b < old.len() - a && b < new.len() - a && old[old.len() - 1 - b] == new[new.len() - 1 - b] {
            b += 1;
        }
        let (old_mid, new_mid) = (old.len() - a - b, new.len() - a - b);
        let common = old_mid.min(new_mid);
        let exec = self.g.ops.exec;
        let mut nodes = self.nodes[..a].to_vec();
        for k in 0..common {
            let n = self.nodes[a + k];
            let args = line_args(self.g.eng.values(), &new[a + k]);
            self.g.args.insert(n.raw(), args.clone());
            self.g.eng.replace(n, exec, args);
            nodes.push(n);
        }
        for k in common..old_mid {
            self.g.args.remove(&self.nodes[a + k].raw());
            self.g.eng.remove(self.nodes[a + k]);
        }
        for k in common..new_mid {
            let args = line_args(self.g.eng.values(), &new[a + k]);
            let n = match nodes.last() {
                Some(&p) => self.g.eng.insert_after(p, exec, args.clone()),
                None => {
                    let first = self.nodes.get(a + old_mid).copied().unwrap_or(self.fin);
                    self.g.eng.insert_before(first, exec, args.clone())
                }
            };
            self.g.args.insert(n.raw(), args);
            nodes.push(n);
        }
        nodes.extend_from_slice(&self.nodes[a + old_mid..]);
        self.nodes = nodes;
        self.lines = new;
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
            .map(|x| str_of(v, x).chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(0)).collect())
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

    #[must_use]
    pub fn hints(&self) -> usize {
        self.hints_set.len()
    }
}
