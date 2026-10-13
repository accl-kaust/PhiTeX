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
use phitex_source::pass;

use crate::hints::Defs;
use crate::ops::{Dvi, Exec, Files, Mark, Put, Shared};
use crate::run::{slice_args_in_file, Ops, slice_args};
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
    /// The wall time one evaluation (the prelude, a build, an edit) may
    /// take; past it the build ends with the engine's deadline error,
    /// naming what waits, in the log. `None`: no limit.
    pub time_limit: Option<std::time::Duration>,
    /// The threads the engine's batches run on: `None` takes
    /// `PHITEX_THREADS`, else the available parallelism; `Some(1)` runs
    /// them in order. The output is the same at any count.
    pub threads: Option<usize>,
    /// Whether the PDF file's objects are numbered as pdfTeX numbers
    /// them (the `pdf:state` fold, byte-identical), or each page and
    /// each object TeX makes is written on its own and numbered at the
    /// end (`crates/output/pdf/RELAXED.md`: the same file up to object
    /// numbers and resource names). A document the relaxed mode does
    /// not cover is built in exact mode, with a warning in the log.
    pub exact_objects: bool,
    /// Where the build reports what it runs ([`Build::progress`]), for
    /// a reader that polls it during the cold build; `None` makes one.
    pub progress: Option<Arc<crate::exec::Progress>>,
    /// Where texts for the build come from while it runs
    /// ([`EditSender`]): the engine takes the latest between its steps,
    /// and the build ends with the result for it. `None` makes one
    /// ([`Build::edit_sender`]).
    pub edits: Option<EditSender>,
    /// Called after each build or edit for every page whose hash
    /// ([`Build::pages`]) is new or changed: the page's index (0 the
    /// first), its hash, and the version of the text it is for.
    pub on_page_settled: Option<PageSettled>,
    /// Whether the engine runs nodes ahead of their inputs
    /// (`PHITEX_SPEC=off|predicted|all` overrides; default Predicted), and on how
    /// many
    /// candidates per step. The output is the same either way.
    pub speculation: Speculation,
    pub speculation_budget: usize,
}

/// When the engine speculates ([`Options::speculation`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speculation {
    /// Never.
    Off,
    /// On nodes a previous run predicts.
    Predicted,
    /// On any candidate.
    All,
}

impl Speculation {
    /// The engine's scheduler for `self` and `budget`, `PHITEX_SPEC`
    /// overriding.
    fn scheduler(self, budget: usize) -> phitex_defuse::Greedy {
        use phitex_defuse::Speculation as S;
        let mode = match std::env::var("PHITEX_SPEC").ok().as_deref() {
            Some("off") => S::Off,
            Some("predicted") => S::Predicted,
            Some("all") => S::All,
            _ => match self {
                Speculation::Off => S::Off,
                Speculation::Predicted => S::Predicted,
                Speculation::All => S::All,
            },
        };
        phitex_defuse::Greedy {
            outstanding: 4096,
            fuel: 10_000,
            ..phitex_defuse::Greedy::speculative(mode, budget)
        }
    }
}

/// The callback of [`Options::on_page_settled`].
pub type PageSettled = Arc<dyn Fn(usize, u64, u64) + Send + Sync>;

/// A handle to send a build the document's new texts from any thread.
/// Each text gets the next version number; a text sent while the build
/// runs is taken between the engine's steps (only the latest counts:
/// each is the whole document), and [`Build::version`] says which text
/// a result is for.
#[derive(Clone, Default)]
pub struct EditSender(Arc<Mutex<Pending>>);

#[derive(Default)]
struct Pending {
    text: Option<String>,
    version: u64,
}

impl EditSender {
    /// Sends the document's text: its version.
    pub fn send(&self, text: &str) -> u64 {
        let mut p = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        p.version += 1;
        p.text = Some(text.to_owned());
        p.version
    }

    /// Whether a text waits that no build has taken.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .text
            .is_some()
    }

    fn take(&self) -> Option<(String, u64)> {
        let mut p = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let v = p.version;
        p.text.take().map(|t| (t, v))
    }
}

/// The engine's source of edits during an evaluation: the latest text
/// sent, applied to the document's nodes.
struct Absorb<'a> {
    edits: &'a EditSender,
    nodes: &'a mut Vec<(String, NodeId)>,
    exec: OpId,
    dvi: NodeId,
    /// The format's definitions, for the new slices' hints and regions.
    defs: Defs,
}

impl phitex_defuse::EditSource for Absorb<'_> {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        let (text, version) = self.edits.take()?;
        apply_text(eng, self.exec, self.dvi, self.nodes, &text, &self.defs);
        Some(version)
    }
}

/// Edits the document's nodes `nodes` to the text `text`: the slices
/// both texts share at the start and the end stay, those between are
/// replaced, removed or inserted.
fn apply_text(
    eng: &mut Engine,
    exec: OpId,
    dvi: NodeId,
    nodes_now: &mut Vec<(String, NodeId)>,
    text: &str,
    defs: &Defs,
) {
    let text = as_file(text);
    let new: Vec<(String, (usize, usize))> = slices(&text, defs)
        .into_iter()
        .map(|(s, e)| {
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            (key(&text[s..e], starts), (s, e))
        })
        .collect();
    let old = &*nodes_now;
    let hints = crate::hints::text_hints(&text, pass::FileKind::Document, defs);
    crate::hints::register_text(eng.values().str(DOC_NAME), &hints);
    let args_of = |eng: &Engine, k: usize| {
        let (s, e) = new[k].1;
        let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
        let v = eng.values();
        let mut args = slice_args_in_file(v, &text[s..e], starts, v.str(DOC_NAME), s);
        // (a document node is a top-level node)
        args[1] = v.int(1);
        args.push(crate::hints::hint_arg(v, &hints, s));
        args
    };
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
    for k in common..old_mid {
        eng.remove(old[a + k].1);
    }
    let mut nodes: Vec<(String, NodeId)> = Vec::with_capacity(new.len());
    for i in 0..new.len() {
        let n = if i < a || i >= a + new_mid {
            let oi = if i < a { i } else { i - new_mid + old_mid };
            old[oi].1
        } else if i - a < common {
            let n = old[i].1;
            let args = args_of(eng, i);
            eng.replace(n, exec, args);
            n
        } else {
            let args = args_of(eng, i);
            if let Some(&(_, p)) = nodes.last() {
                eng.insert_after(p, exec, args)
            } else {
                let first = old.get(a + old_mid).map_or(dvi, |x| x.1);
                eng.insert_before(first, exec, args)
            }
        };
        nodes.push((new[i].0.clone(), n));
    }
    *nodes_now = nodes;
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
            time_limit: Some(std::time::Duration::from_secs(600)),
            threads: None,
            exact_objects: true,
            progress: None,
            edits: None,
            on_page_settled: None,
            // free on a cold build, helps edits; All (the engine's advice
            // for cold builds) mispredicts most of its runs while the
            // commands answer `Any`
            speculation: Speculation::Predicted,
            speculation_budget: 256,
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
    /// The prelude's evaluation errors: such a prelude is not cached.
    errors: Vec<String>,
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

/// The layout of an exported prelude and of what it binds. Bump when
/// the serialised environment's layout, the node args, the `Fields` set,
/// the hint args or the state fields' names change: a prelude exported
/// under another layout is refused (rebuilt), never loaded.
pub const PRELUDE_LAYOUT: u32 = 3;

/// The exported prelude's magic, before the layout and the engine's
/// identity.
const PRELUDE_MAGIC: &[u8; 8] = b"PHITEXPR";

/// The vendored engine's identity: a hash of its sources, so vendoring
/// another engine invalidates every exported prelude.
fn engine_identity() -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        let srcs: [&[u8]; 9] = [
            include_bytes!("../../../vendor/defuse/Cargo.toml"),
            include_bytes!("../../../vendor/defuse/src/lib.rs"),
            include_bytes!("../../../vendor/defuse/src/engine.rs"),
            include_bytes!("../../../vendor/defuse/src/value.rs"),
            include_bytes!("../../../vendor/defuse/src/ctx.rs"),
            include_bytes!("../../../vendor/defuse/src/sched.rs"),
            include_bytes!("../../../vendor/defuse/src/tree.rs"),
            include_bytes!("../../../vendor/defuse/src/hash.rs"),
            include_bytes!("../../../vendor/defuse/src/obs.rs"),
        ];
        let mut h = 0u64;
        for s in srcs {
            h = phitex_defuse::hash_bytes(h ^ 0x9e37_79b9, s);
        }
        h
    })
}

/// The key of a prelude: the layout and the engine, its text and
/// jobname, and what it read.
fn prelude_key_of(text_key: u64, reads: &[(String, u64)]) -> [u8; 32] {
    let mut b = PRELUDE_LAYOUT.to_le_bytes().to_vec();
    b.extend_from_slice(&engine_identity().to_le_bytes());
    b.extend_from_slice(&text_key.to_le_bytes());
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

/// `[magic][layout][engine][key][reads][env]`, as
/// [`Build::export_prelude`] writes it.
fn serialise(c: &Cached) -> Vec<u8> {
    serialise_as(c, PRELUDE_LAYOUT, engine_identity())
}

fn serialise_as(c: &Cached, layout: u32, engine: u64) -> Vec<u8> {
    let mut out = PRELUDE_MAGIC.to_vec();
    out.extend_from_slice(&layout.to_le_bytes());
    out.extend_from_slice(&engine.to_le_bytes());
    out.extend_from_slice(&c.key);
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

/// A prelude exported under this layout and engine; `None` (rebuild)
/// for any other.
fn deserialise(b: &[u8]) -> Option<Cached> {
    let c = deserialise_as(b, PRELUDE_LAYOUT, engine_identity());
    if c.is_none() && !b.is_empty() {
        eprintln!("format cache rebuilt (layout changed)");
    }
    c
}

fn deserialise_as(b: &[u8], layout: u32, engine: u64) -> Option<Cached> {
    let mut r = Rd(b);
    if r.take(8)? != PRELUDE_MAGIC
        || u32::from_le_bytes(r.take(4)?.try_into().ok()?) != layout
        || r.u64()? != engine
    {
        return None;
    }
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
        errors: Vec::new(),
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

/// The nodes of a text: its paragraph regions for a document, one
/// command per slice for a format ([`crate::hints::slices_of`]); the
/// pass over the whole text also gives each slice's hint.
pub(crate) fn slices(text: &str, defs: &Defs) -> Vec<(usize, usize)> {
    crate::hints::text_hints(text, pass::FileKind::Document, defs)
        .slices
        .clone()
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
    /// Always 0: no expansion reads on into another node any more.
    pub tails: u64,
    /// Paragraph-ending items after which the list was not vertical
    /// (OUTPUT.md §5). Always 0: a region runs on its own nest.
    pub par_boundary_violations: u64,
    /// Outer `\fi`s that found no conditional at the depth the text's
    /// nesting gave their node (OUTPUT.md §6).
    pub if_depth_violations: u64,
    /// Always 0 (see `tails`).
    pub tail_siblings: u64,
    /// Children that ran out of text (a claim measured short: a bug).
    pub starved: u64,
    /// Loop-back rounds the engine ran (the files read back, to their
    /// fixpoint).
    pub loop_rounds: u64,
    /// Nodes that waited on a value not known yet, and stops.
    pub pending_waits: u64,
    pub stops: u64,
    /// Attempts that resumed a stopped run at its blocked read, and that
    /// started over though one was kept (a read changed).
    pub resumed: u64,
    pub restarted: u64,
    /// The executor's threads.
    pub threads: usize,
    /// The evaluate calls' wall time, summed (zero without a clock).
    pub wall: std::time::Duration,
    /// The node runs' time, summed over every job (zero without a
    /// clock): parallelism is `busy / wall`.
    pub busy: std::time::Duration,
    /// The most jobs that ran at once.
    pub peak_running: usize,
    /// The longest chain of dependent waits among the live nodes after
    /// the last evaluation (the engine's `critical_path`), in nodes.
    /// The batches of runs the executor was given, those it ran on
    /// the calling thread (one job, or one thread), and the largest.
    pub batches: u64,
    pub batches_inline: u64,
    pub jobs_max_batch: usize,
    pub critical_path: Option<usize>,
    /// The light page steps run (OUTPUT.md §4.2a), and those of them
    /// with no output routine, which may define anything.
    pub page_steps: u64,
    pub page_steps_any: u64,
    /// The engine's pending waits by kind, summed over the evaluations.
    pub waits_by_kind: Vec<(&'static str, u64)>,
    /// The nodes most waited on in the last evaluation (raw ids, waits).
    pub top_waited: Vec<(u64, u64)>,
    /// Starts the engine held back until a node's inputs were known.
    pub gated: u64,
    /// The version of the text the result is for ([`Build::version`]).
    pub version: u64,
    /// Scheduler steps, and the nodes run in them summed: `ready_mean`
    /// is the mean batch.
    pub steps: u64,
    pub ready_total: u64,
    /// Nodes whose text bounded what they may define ([`crate::hints`]),
    /// and those that may define anything, among the live nodes that
    /// ran.
    pub nodes_bounded: u64,
    pub nodes_any: u64,
    /// Nodes whose hint's assumptions (catcodes, meanings) did not hold
    /// when they ran, and nodes that defined outside their bound: both
    /// diagnostics, never a silent fallback.
    pub regime_violations: u64,
    pub undeclared_definitions: u64,
    /// Nodes answering `Any` because their class is demoted
    /// (`crate::hints`; `PHITEX_HINTS=1` names the class).
    pub hints_demoted: u64,
    /// Nodes whose text is a token list (`["t", ...]`), and nodes
    /// reading source text (a document slice, a line, a file).
    pub nodes_token_frames: u64,
    pub nodes_text_frames: u64,
    /// The fields whose speculative reads were refuted, with counts,
    /// most first.
    pub refuted_by_field: Vec<(String, u64)>,
    /// The engine's speculative runs: kept with outstanding
    /// assumptions, confirmed, and whose assumptions failed; and nodes
    /// re-done higher after running below a finished input (raises).
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    pub raises: u64,
    /// The most nodes ready at one step, and the speculative runs that
    /// ran out of fuel.
    pub max_ready: usize,
    pub fuel_outs: u64,
}

/// A round's counters (`PHITEX_TEX_TIME`).
fn print_round(t: std::time::Duration, r: &Report) {
    eprintln!(
        "round: {t:?}, {} nodes run, {} runs, {} gated, {} cutoffs, {} rebinds, {} restarts, {} loop rounds, {} stops, {} raises, {} pending, {} created, {} freed, spec {}/{}/{}, by op {:?}",
        r.nodes_run,
        r.runs,
        r.gated,
        r.cutoffs,
        r.rebinds,
        r.restarts,
        r.rounds,
        r.stops,
        r.raises,
        r.pending_waits,
        r.created,
        r.freed,
        r.speculations,
        r.confirmations,
        r.mispredictions,
        r.runs_by_op
    );
}

impl Stats {
    /// The mean number of nodes run per scheduler step.
    #[must_use]
    pub fn ready_mean(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let m = self.ready_total as f64 / self.steps.max(1) as f64;
        m
    }

    /// The executor's counters of one evaluation begun at `c0`.
    fn add_exec(&mut self, ex: &crate::exec::Threads, c0: Option<u64>, eng: &Engine) {
        if let (Some(a), Some(b)) = (c0, crate::exec::clock().now()) {
            self.wall += std::time::Duration::from_nanos(b.saturating_sub(a));
        }
        self.threads = ex.n;
        self.busy += ex.busy();
        self.peak_running = self.peak_running.max(ex.peak());
        let (b, bi, m) = ex.batches();
        self.batches += b;
        self.batches_inline += bi;
        self.jobs_max_batch = self.jobs_max_batch.max(m);
        self.critical_path = usize::try_from(eng.critical_path()).ok();
    }

    /// The hint counters of `unit` (the live nodes' accumulators), and
    /// the engine's undeclared-definition errors so far.
    fn add_hints(&mut self, eng: &Engine, unit: UnitId, errors: &[String]) {
        let v = eng.values();
        let get = |name: &str| -> u64 {
            eng.acc_value(unit, v.field(name))
                .and_then(|x| v.as_int(x))
                .and_then(|x| u64::try_from(x).ok())
                .unwrap_or(0)
        };
        let c = crate::hints::COUNTERS;
        self.nodes_bounded = get(c[0]);
        self.nodes_any = get(c[1]);
        self.regime_violations = get(c[2]);
        self.hints_demoted = get(c[4]);
        self.nodes_token_frames = get(c[5]);
        self.nodes_text_frames = get(c[6]);
        self.undeclared_definitions = get(c[3])
            + u64::try_from(
                errors
                    .iter()
                    .filter(|e| e.contains("UndeclaredDefinition"))
                    .count(),
            )
            .unwrap_or(u64::MAX);
    }

    fn add_refuted(&mut self, eng: &Engine, r: &Report) {
        let v = eng.values();
        for &(f, n) in &r.refuted_by_field {
            let name = field_name(v, f);
            match self.refuted_by_field.iter_mut().find(|e| e.0 == name) {
                Some(e) => e.1 += n,
                None => self.refuted_by_field.push((name, n)),
            }
        }
        self.refuted_by_field
            .sort_by_key(|e| std::cmp::Reverse(e.1));
    }

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
        self.resumed += r.resumed;
        self.restarted += r.restarted;
        self.version = r.version;
        self.steps += r.steps;
        self.ready_total += r.ready_total;
        self.speculations += r.speculations;
        self.confirmations += r.confirmations;
        self.mispredictions += r.mispredictions;
        self.raises += r.raises;
        self.max_ready = self.max_ready.max(r.max_ready);
        self.fuel_outs += r.fuel_outs;
        self.page_steps +=
            Report::by_op(&r.runs_by_op, "pagestep") + Report::by_op(&r.runs_by_op, "pagestep_any");
        self.page_steps_any += Report::by_op(&r.runs_by_op, "pagestep_any");
        self.gated += r.gated;
        self.top_waited = r.top_waited.iter().map(|&(n, w)| (n.raw(), w)).collect();
        for &(k, n) in &r.waits_by_kind {
            match self.waits_by_kind.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += n,
                None => self.waits_by_kind.push((k, n)),
            }
        }
    }
}

/// An engine with the operations registered.
struct Graph {
    eng: Engine,
    /// The evaluation errors met (the engine's), as messages.
    eval_errors: Vec<String>,
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
    /// The executor's threads ([`Options::threads`]).
    threads: usize,
    /// Why the relaxed PDF mode gave way to exact mode, once it has.
    pdf_exact: Option<String>,
    /// What the executor runs now, for readers on other threads.
    watch: crate::exec::Watch,
    /// The engine's scheduler ([`Options::speculation`]).
    sched: phitex_defuse::Greedy,
}

impl Graph {
    fn new(time: Option<std::time::Duration>, threads: Option<usize>) -> Graph {
        let mut eng = Engine::new();
        eng.set_limits(phitex_defuse::Limits {
            // a node's children are bounded by the text it reads (one per
            // `\hyphenation` word, per character of a long run), not by a
            // constant
            children_per_node: 1 << 24,
            // TeX's own nesting limits are far below this
            unfold_depth: 4096,
            time,
            ..phitex_defuse::Limits::default()
        });
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let [pagestep, pagestep_any] = [false, true].map(|any| {
            eng.register(Arc::new(crate::page::PageStep {
                shared: shared.clone(),
                any,
            }))
        });
        let breaks = eng.register(Arc::new(crate::page::Breaks(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi));
        let mark = eng.register(Arc::new(Mark));
        let files_op = eng.register(Arc::new(Files));
        let fields = Arc::new(crate::fields::Fields::new(eng.values()));
        // (handles held outside the graph: pinned, or a value sweep
        // frees them while a run still defines through them)
        for f in fields.handles() {
            eng.pin(f.0);
        }
        let ops = Ops {
            exec,
            put,
            pagestep,
            pagestep_any,
            breaks,
            fields,
        };
        let _ = shared.set(ops.clone());
        for f in [
            "items",
            "log",
            "diags",
            "needs",
            "defs",
            "pages",
            "pagehash",
            "writes",
            "ifviol",
            "streams",
            crate::pdfrelaxed::PIECES,
        ] {
            let fld = eng.values().field(f);
            eng.declare_accumulator(fld, Arc::new(SeqMonoid));
        }
        let lines = eng.values().field("lines");
        eng.declare_accumulator(lines, Arc::new(SumMonoid));
        // what each page's output routine leaves for the next: a
        // loop-back the page builder's fold reads from the previous round
        for j in 0..crate::page::MAX_PAGES {
            let f = eng.values().field(&format!("pagefx:{j}"));
            let none = eng.values().list(Vec::new());
            eng.declare_loopback(f, none);
        }
        for f in crate::hints::COUNTERS {
            let fld = eng.values().field(f);
            eng.declare_accumulator(fld, Arc::new(SumMonoid));
        }
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
            threads: crate::exec::threads(threads),
            pdf_exact: None,
            watch: crate::exec::Watch::new(crate::exec::threads(threads), None),
            sched: phitex_defuse::Greedy::default(),
            eval_errors: Vec::new(),
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
        // (the field and the nodes' texts first: a harness shows the
        // head of the message; the error's full form last)
        let mut out = format!("evaluation: {e}");
        let v = self.eng.values();
        if let E::UndeclaredDefinition { field, .. } = e {
            let _ = write!(out, "\n  field {}", field_name(v, *field));
        }
        for id in ids {
            let what = self.eng.node_info(id).map_or_else(
                || "gone".to_owned(),
                |i| {
                    let args: Vec<String> = i
                        .args
                        .iter()
                        .map(|&a| v.render(a).chars().take(200).collect())
                        .collect();
                    // (and the node's hint: what its text was taken to be)
                    let h = crate::hints::hint_of_args(v, &i.args);
                    format!(
                        "{} {:?}, parent {:?}, hint {:?} ({}), args {}",
                        i.op,
                        i.state,
                        i.parent,
                        h.excerpt,
                        h.cause,
                        args.join(" ")
                    )
                },
            );
            let _ = write!(out, "\n  {id:?}: {what}");
        }
        let _ = write!(out, "\n  {e:?}");
        out
    }

    /// Supplies file `name` to the environment now, through the finder,
    /// unless it is supplied, absent or written already: a read of it
    /// then hits on the run's first attempt instead of stopping the run
    /// and costing a round (every file a previous build of this prelude
    /// or document read is supplied up front, [`Build::with`]).
    fn supply(&mut self, finder: &Finder, name: &str) {
        let f = self.eng.values().field(&format!("file:{name}"));
        if self.files.iter().any(|(g, _)| *g == f)
            || self.absent.contains(&f)
            || self.written.iter().any(|w| w == name)
        {
            return;
        }
        let found = finder(name);
        self.queries.push((name.to_owned(), found.is_some()));
        let v = self.eng.values();
        self.reads.push((name.to_owned(), file_hash(found.as_ref())));
        if let Some((path, bytes)) = found {
            let text: String = bytes.iter().map(|&b| char::from(b)).collect();
            let val = v.list(vec![v.str(&path), v.str(&text)]);
            self.files.push((f, val));
        } else {
            self.absent.push(f);
        }
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
        if self.pdf_exact.is_some() {
            env.push((v.field("pdf:exact"), v.int(1)));
        }
        if fixpoint {
            let names = self.written.iter().map(|n| v.str(n)).collect();
            env.push((v.field("loopfiles"), v.list(names)));
        }
        env
    }

    /// Evaluates; when the graph asks for files, supplies them and
    /// evaluates again.
    #[allow(clippy::too_many_lines)]
    fn settle(
        &mut self,
        unit: UnitId,
        finder: &Finder,
        stats: &mut Stats,
        fixpoint: bool,
        edits: &mut dyn phitex_defuse::EditSource,
    ) {
        let needs_f = self.eng.values().field("needs");
        loop {
            let t0 = std::time::Instant::now();
            let mut ex = crate::exec::Threads::new(self.threads).watched(&self.watch);
            let c0 = crate::exec::clock().now();
            let r = match self.eng.evaluate_absorbing(&mut self.sched, &mut ex, edits) {
                Ok(r) => r,
                Err(e) => {
                    // the build ends here; the error is a diagnostic
                    let m = self.diagnose(&e);
                    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
                        eprintln!("{m}");
                    }
                    self.eval_errors.push(m);
                    stats.add_hints(&self.eng, unit, &self.eval_errors);
                    return;
                }
            };
            if crate::exec::env_flag!("PHITEX_TEX_TIME") {
                print_round(t0.elapsed(), &r);
                eprintln!("  waits by kind {:?}", r.waits_by_kind);
                for &(n, w) in &r.top_waited {
                    eprintln!("  waited on {w}x: {}", describe(&self.eng, n));
                }
            }
            stats.add(&r);
            stats.add_refuted(&self.eng, &r);
            stats.add_exec(&ex, c0, &self.eng);
            stats.add_hints(&self.eng, unit, &self.eval_errors);
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
            if crate::exec::env_flag!("PHITEX_TEX_TIME") {
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
            for (kind, why) in &needs {
                if kind == "starve" {
                    stats.starved += 1;
                }
                // the relaxed PDF mode cannot write this document: the
                // build again, in exact mode
                if kind == "pdf:exact" && self.pdf_exact.is_none() {
                    self.pdf_exact = Some(why.clone());
                    asked = true;
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
                self.supply(finder, &name);
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
        if let Some(t) = self.eng.acc_value(unit, v.field("ifviol")) {
            stats.if_depth_violations += u64::try_from(v.seq_to_vec(t).len()).unwrap_or(0);
        }
    }

    /// The text's nodes pushed onto `unit`.
    fn push_text(&mut self, unit: UnitId, text: &str, defs: &Defs) -> Vec<(String, NodeId)> {
        let mut out = Vec::new();
        let t0 = std::time::Instant::now();
        let sl = slices(text, defs);
        let t1 = t0.elapsed();
        let hints = crate::hints::text_hints(text, pass::FileKind::Document, defs);
        crate::hints::register_text(self.eng.values().str(DOC_NAME), &hints);
        for (s, e) in sl {
            let piece = &text[s..e];
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
                eprintln!("node {piece:?}");
            }
            let v = self.eng.values();
            let mut args = slice_args_in_file(v, piece, starts, v.str(DOC_NAME), s);
            args[1] = v.int(1);
            args.push(crate::hints::hint_arg(v, &hints, s));
            let n = self.eng.push(unit, self.ops.exec, args);
            out.push((key(piece, starts), n));
        }
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "push_text: slices {t1:?}, push {:?}, {} nodes",
                t0.elapsed().checked_sub(t1).unwrap(),
                out.len()
            );
        }
        out
    }
}

/// The name a document's own text is registered under for the hints
/// of nodes made from parts of it (`hints::register_text`).
const DOC_NAME: &str = "\u{10FFFE}document";

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

/// Whether a file the prelude read is TeX text worth harvesting (fonts,
/// encodings and maps are not).
fn is_tex_text(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rsplit_once('.') {
        None => true,
        Some((_, ext)) => matches!(
            ext,
            "tex" | "ltx" | "sty" | "cls" | "def" | "fd" | "clo" | "cfg" | "ini" | "dtx" | "ldf"
        ),
    }
}

/// The definitions the prelude's text and the files it read make, in
/// load order: the format's macros, which the hints bound a document's
/// calls with ([`crate::hints`]). Harvested once per prelude key (a text pass, no
/// evaluation), for a prelude evaluated here or imported from bytes.
fn prelude_defs(c: &Cached, prelude: Option<&str>, finder: &Finder) -> Defs {
    static C: OnceLock<Mutex<HashMap<[u8; 32], Defs>>> = OnceLock::new();
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(d) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&c.key)
    {
        return d.clone();
    }
    let t0 = std::time::Instant::now();
    let mut defs = pass::Definitions::new();
    let mut order: Vec<u32> = Vec::new();
    if let Some(p) = prelude {
        defs.harvest(p, crate::hints::kind_of(p, pass::FileKind::Package), 0);
        order.push(0);
    }
    for (k, (name, h)) in c.reads.iter().enumerate() {
        if *h == 0 || !is_tex_text(name) {
            continue;
        }
        let Some((_, bytes)) = finder(name) else {
            continue;
        };
        let text: String = bytes.iter().map(|&b| char::from(b)).collect();
        let file = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let kind = crate::hints::kind_of(&text, pass::FileKind::from_path(name));
        defs.harvest(&text, kind, file);
        order.push(file);
    }
    defs.set_load_order(&order);
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "prelude definitions: {} names from {} files in {:?}",
            defs.len(),
            order.len(),
            t0.elapsed()
        );
    }
    let d: Defs = Arc::new(defs);
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(c.key, d.clone());
    d
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
    let mut known: Vec<String> = Vec::new();
    for c in given.into_iter().chain(candidates) {
        if c.reads
            .iter()
            .all(|(n, h)| file_hash(finder(n).as_ref()) == *h)
        {
            return c;
        }
        // (a stale cache entry still names the files the prelude reads)
        for (n, _) in &c.reads {
            if !known.contains(n) {
                known.push(n.clone());
            }
        }
    }
    let mut g = Graph::new(opts.time_limit, opts.threads);
    g.sched = opts.speculation.scheduler(opts.speculation_budget);
    let unit = g.eng.new_unit();
    // the files a previous build of this prelude read, supplied before
    // the first evaluation: no run stops on them, no round per file
    for n in &known {
        g.supply(finder, n);
    }
    if !known.is_empty() {
        let env = g.environment(false);
        g.eng.set_environment(env);
        g.eng.set_environment_absent(g.absent.clone());
    }
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
        // (the prelude's own slices: INITEX knows no definitions yet)
        g.push_text(unit, &as_file(p), crate::hints::no_defs());
        // the point the prelude's final bindings are read at
        let end = g.eng.push(unit, g.mark, Vec::new());
        let mut stats = Stats::default();
        let t0 = std::time::Instant::now();
        g.settle(unit, finder, &mut stats, false, &mut phitex_defuse::NoEdits);
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
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
        errors: g.eval_errors.clone(),
    });
    if c.errors.is_empty() {
        env_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default()
            .push(c.clone());
    }
    c
}

/// How bad a [`Diagnostic`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// A message about the document, at a place in one of its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// The file: the document's name, or an `\input` file's path.
    pub file: String,
    /// Bytes of that file.
    pub range: (usize, usize),
    /// The line of `range.0`, from 1.
    pub line: usize,
    pub severity: Severity,
    pub message: String,
    /// TeX's two-line context (§318): the line up to the error point,
    /// then the rest of it below.
    pub context: String,
    pub help: Option<String>,
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
    /// The format's definitions ([`prelude_defs`]).
    defs: Defs,
    /// What the last build or edit ran.
    pub stats: Stats,
    /// Where new texts come from ([`Options::edits`]).
    edits: EditSender,
    /// The pages' hashes the callback last saw.
    settled: Vec<u64>,
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
        let defs = prelude_defs(&cached, prelude, &opts.finder);
        let t1 = t0.elapsed();
        let mut g = Graph::new(opts.time_limit, opts.threads);
        g.sched = opts.speculation.scheduler(opts.speculation_budget);
        g.sched = opts.speculation.scheduler(opts.speculation_budget);
        if let Some(p) = &opts.progress {
            g.watch = crate::exec::Watch::new(g.threads, Some(p.clone()));
        }
        let unit = g.eng.new_unit();
        // the files the format read (fonts, data) are supplied before the
        // document's first evaluation: a run reading one never stops
        for (n, _) in cached.reads.clone() {
            g.supply(&opts.finder, &n);
        }
        if !cached.reads.is_empty() {
            let env = g.environment(opts.fixpoint);
            g.eng.set_environment(env);
            g.eng.set_environment_absent(g.absent.clone());
        }
        {
            let v = g.eng.values();
            let mut sets: Vec<(Field, ValueId)> = Vec::new();
            for (k, t) in env.iter() {
                sets.push((v.field(k), import(v, t)));
            }
            sets.push((v.field("jobname"), v.str(&opts.jobname)));
            sets.push((v.field("dvicomment"), v.str(&opts.comment)));
            sets.push((
                v.field("pdf:relaxed"),
                v.int(i64::from(!opts.exact_objects)),
            ));
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
        let nodes = g.push_text(unit, &as_file(document), &defs);
        // the page builder's fold, after the text: one node choosing every
        // page break, its children the pages
        let breaks = g.ops.breaks;
        let _ = g.eng.push(unit, breaks, Vec::new());
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        g.eng.add_root(dvi);
        let files_op = g.files_op;
        let files = g.eng.push(unit, files_op, Vec::new());
        g.eng.add_root(files);
        let edits = opts.edits.clone().unwrap_or_default();
        let mut b = Build {
            g,
            unit,
            opts,
            nodes,
            dvi,
            prelude: cached,
            defs,
            stats: Stats::default(),
            edits,
            settled: Vec::new(),
        };
        b.settle();
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: prelude {t1:?}, document {:?}",
                t0.elapsed().checked_sub(t1).unwrap()
            );
        }
        b
    }

    fn settle(&mut self) {
        // the text nodes' offsets in the document, for the progress
        let mut offsets = std::collections::HashMap::new();
        let mut off = 0u64;
        for (k, n) in &self.nodes {
            offsets.insert(n.raw(), off);
            off += u64::try_from(k.len().saturating_sub(1)).unwrap_or(0);
        }
        self.g.watch.offsets = Arc::new(offsets);
        let mut stats = Stats::default();
        self.g.queries.clear();
        let mut absorb = Absorb {
            edits: &self.edits,
            nodes: &mut self.nodes,
            exec: self.g.ops.exec,
            dvi: self.dvi,
            defs: self.defs.clone(),
        };
        // a text sent before the evaluation starts is the one it is for
        if let Some(v) = phitex_defuse::EditSource::absorb(&mut absorb, &mut self.g.eng) {
            self.g.eng.set_version(v);
        }
        self.g.settle(
            self.unit,
            &self.opts.finder,
            &mut stats,
            self.opts.fixpoint,
            &mut absorb,
        );
        stats.live = self.g.eng.live_nodes();
        self.stats = stats;
        // `PHITEX_CLAIMS=<text>`: the nodes whose slice holds the text,
        // their claims and their descendants' (a debugging dump)
        if let Ok(want) = std::env::var("PHITEX_CLAIMS") {
            fn dump(eng: &Engine, v: &Values, n: NodeId, depth: usize) {
                let Some(i) = eng.node_info(n) else { return };
                let a = i
                    .args
                    .first()
                    .map(|&x| v.render(x).chars().take(60).collect::<String>())
                    .unwrap_or_default();
                let claimed: Vec<String> = eng
                    .claimed(n)
                    .iter()
                    .map(|&c| {
                        eng.node_info(c)
                            .and_then(|ci| ci.args.first().map(|&x| v.render(x)))
                            .map_or_else(
                                || "?".to_owned(),
                                |t| t.chars().take(30).collect::<String>(),
                            )
                    })
                    .collect();
                eprintln!(
                    "{}{} {:?} {} claims {:?}",
                    "  ".repeat(depth),
                    n.raw() % 100_000,
                    i.state,
                    a,
                    claimed
                );
                if depth < 4 {
                    for c in eng.children(n) {
                        dump(eng, v, c, depth + 1);
                    }
                }
            }
            let v = self.g.eng.values();
            let eng = &self.g.eng;
            for (k, n) in &self.nodes {
                if k.contains(&want) {
                    dump(eng, v, *n, 0);
                }
            }
        }
        if let Some(cb) = self.opts.on_page_settled.clone() {
            let pages = self.pages();
            for (k, &h) in pages.iter().enumerate() {
                if self.settled.get(k) != Some(&h) {
                    cb(k, h, self.stats.version);
                }
            }
            self.settled = pages;
        }
    }

    /// A handle to send this build new texts from another thread
    /// ([`EditSender`]); a text it sends while an edit or the cold
    /// build runs is taken by that evaluation.
    #[must_use]
    pub fn edit_sender(&self) -> EditSender {
        self.edits.clone()
    }

    /// The version of the text the last result is for ([`EditSender`];
    /// 0: the text the build began with).
    #[must_use]
    pub fn version(&self) -> u64 {
        self.stats.version
    }

    /// Builds the latest text sent, if one waits that no evaluation took:
    /// whether there was one.
    pub fn absorb_pending(&mut self) -> bool {
        if !self.edits.pending() {
            return false;
        }
        self.settle();
        true
    }

    /// What the build runs now, readable from another thread while
    /// [`Build::with`] or [`Build::edit`] runs (the CLI polls it).
    #[must_use]
    pub fn progress(&self) -> Arc<crate::exec::Progress> {
        self.g.watch.progress.clone()
    }

    /// Why the PDF file was written in exact mode though
    /// [`Options::exact_objects`] was off (the relaxed mode does not
    /// cover the document), if it was.
    #[must_use]
    pub fn pdf_exact_reason(&self) -> Option<&str> {
        self.g.pdf_exact.as_deref()
    }

    /// The document changed to `text`: the nodes whose slices differ are
    /// replaced, inserted or removed, and the graph re-evaluated.
    pub fn edit(&mut self, text: &str) {
        self.edits.send(text);
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

    /// Whether the prelude's evaluation finished: no deadline, no
    /// evaluation error (a node left waiting ends the evaluation with
    /// one).
    #[must_use]
    pub fn prelude_complete(&self) -> bool {
        self.prelude.errors.is_empty()
    }

    /// The prelude's environment, serialised, for
    /// [`Options::prelude_bytes`]; `None` when its evaluation did not
    /// finish ([`Self::prelude_complete`]): a part of a format is never
    /// stored.
    #[must_use]
    pub fn export_prelude(&self) -> Option<Vec<u8>> {
        self.prelude_complete().then(|| serialise(&self.prelude))
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
        let dvi: Vec<u8> = self
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
        let n = self.pages().len();
        let end = if n == 0 {
            "No pages of output.".to_owned()
        } else {
            // §642 (pdfTeX §794): the file's name, its pages and bytes
            let ext = if dvi.starts_with(b"%PDF") {
                "pdf"
            } else {
                "dvi"
            };
            format!(
                "Output written on {}.{ext} ({n} page{}, {} bytes).",
                self.opts.jobname,
                if n == 1 { "" } else { "s" },
                dvi.len()
            )
        };
        let log_f = v.field("log");
        let log = crate::log::layout(
            v,
            &self.opts.name,
            self.g.eng.acc_value(self.unit, log_f),
            &end,
        );
        (dvi, log)
    }

    /// A node by its raw id ([`Stats::top_waited`]): its operation and
    /// the start of its first argument.
    #[must_use]
    pub fn describe_node(&self, raw: u64) -> String {
        describe(&self.g.eng, phitex_defuse::NodeId::from_raw(raw))
    }

    /// Up to three defined control sequences within two edits of `name`,
    /// nearest first, for an undefined one's help.
    fn near_names(&self, name: &str, known: &mut Option<Vec<String>>) -> Vec<String> {
        if name.is_empty() {
            return Vec::new();
        }
        let known = known.get_or_insert_with(|| {
            let v = self.g.eng.values();
            let mut names: Vec<String> = self
                .prelude
                .env
                .iter()
                .filter_map(|(k, _)| k.strip_prefix("cs:").map(str::to_owned))
                .collect();
            let f = v.field("defs");
            if let Some(d) = self.g.eng.acc_value(self.unit, f) {
                for l in v.seq_to_vec(d) {
                    for n in list_of(v, l) {
                        if let Some(c) = field_name(v, Field(n)).strip_prefix("cs:") {
                            names.push(c.to_owned());
                        }
                    }
                }
            }
            names.sort_unstable();
            names.dedup();
            names
        });
        let mut near: Vec<(usize, &String)> = known
            .iter()
            .filter(|k| k.as_str() != name)
            .filter_map(|k| {
                let d = edit_distance(name, k);
                (d <= 2).then_some((d, k))
            })
            .collect();
        near.sort();
        near.into_iter().take(3).map(|(_, k)| k.clone()).collect()
    }

    #[allow(clippy::too_many_lines)]
    /// The diagnostics of the last build or edit: TeX's errors and
    /// warnings (box reports, missing characters) where the input was,
    /// and the engine's evaluation errors; by file, then byte.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let v = self.g.eng.values();
        // the document's text and its nodes' spans
        let mut doc = String::new();
        let mut spans = Vec::new();
        for (k, _) in &self.nodes {
            let piece = k.get(1..).unwrap_or("");
            spans.push(doc.len());
            doc.push_str(piece);
        }
        // every node under a node of the text, to that node
        let mut top: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, (_, n)) in self.nodes.iter().enumerate() {
            let mut stack = vec![*n];
            while let Some(x) = stack.pop() {
                if top.insert(x.raw(), i).is_none() {
                    stack.extend(self.g.eng.children(x));
                }
            }
        }
        let docname = self.opts.name.clone();
        let mut out = Vec::new();
        // the control sequences defined, gathered at the first undefined one
        let mut known: Option<Vec<String>> = None;
        let f = v.field("diags");
        let entries = self
            .g
            .eng
            .acc_value(self.unit, f)
            .map(|s| v.seq_to_vec(s))
            .unwrap_or_default();
        for e in entries {
            let xs = list_of(v, e);
            let int = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
            let error = int(0) != 0;
            let message = xs.get(1).map(|&x| str_of(v, x)).unwrap_or_default();
            let seg = xs.get(5).map(|&x| str_of(v, x)).unwrap_or_default();
            let loc = usize::try_from(int(6)).unwrap_or(0);
            let before: String = seg.chars().take(loc).collect();
            let (file, text, at) = if int(3) >= 0
                && let Some(&n) = xs.get(2)
                && n != ValueId::UNIT
            {
                // a line of an `\input` file: its characters are bytes
                let name = str_of(v, n);
                let bytes = (self.opts.finder)(name.trim_start_matches("./"))
                    .map(|(_, b)| b)
                    .unwrap_or_default();
                let text: String = bytes.iter().map(|&b| char::from(b)).collect();
                let at = usize::try_from(int(3)).unwrap_or(0) + loc;
                (name, text, at)
            } else {
                let node = u64::from_ne_bytes(int(4).to_ne_bytes());
                let i = top.get(&node).copied().unwrap_or(0);
                let from = spans.get(i).copied().unwrap_or(0);
                let at = doc
                    .get(from..)
                    .and_then(|d| (!seg.is_empty()).then(|| d.find(&seg)).flatten())
                    .map_or(from, |k| from + k + before.len());
                let at = if self.nodes.is_empty() { 0 } else { at };
                (docname.clone(), doc.clone(), at)
            };
            // the line around `at`, as characters; `at` in the file's
            // bytes (the document's text is UTF-8, a file's characters
            // are its bytes)
            let chars: Vec<char> = text.chars().collect();
            let ci = if file == docname {
                text.get(..at.min(text.len()))
                    .map_or(0, |t| t.chars().count())
            } else {
                at.min(chars.len())
            };
            let line = chars[..ci].iter().filter(|&&c| c == '\n').count() + 1;
            let ls = chars[..ci]
                .iter()
                .rposition(|&c| c == '\n')
                .map_or(0, |k| k + 1);
            let le = chars[ci..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |k| ci + k);
            let before: String = chars[ls..ci].iter().collect();
            let after: String = chars[ci..le]
                .iter()
                .collect::<String>()
                .trim_end_matches('\r')
                .to_owned();
            let undefined = message == "Undefined control sequence";
            let start = if undefined {
                before.rfind('\\').map_or(at, |k| at - (before.len() - k))
            } else {
                at
            };
            let lead = format!("l.{line} ");
            let context = format!(
                "{lead}{before}\n{}{after}",
                " ".repeat(lead.len() + before.chars().count())
            );
            let help = undefined.then(|| {
                let mut h = "The control sequence at the end of the top line\n\
                 of your error message was never \\def'ed. If you have\n\
                 misspelled it (e.g., `\\hobx'), type `I' and the correct\n\
                 spelling (e.g., `I\\hbox'). Otherwise just continue,\n\
                 and I'll forget about whatever was undefined."
                    .to_owned();
                let name = before.rfind('\\').map_or("", |k| &before[k + 1..]);
                let near = self.near_names(name, &mut known);
                if !near.is_empty() {
                    let l: Vec<String> = near.iter().map(|n| format!("\\{n}")).collect();
                    let _ = std::fmt::Write::write_fmt(
                        &mut h,
                        format_args!("\nDid you mean {}?", l.join(", ")),
                    );
                }
                h
            });
            out.push(Diagnostic {
                file,
                range: (start, at),
                line,
                severity: if error {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                message,
                context,
                help,
            });
        }
        for m in self.prelude.errors.iter().chain(&self.g.eval_errors) {
            out.push(Diagnostic {
                file: docname.clone(),
                range: (0, 0),
                line: 1,
                severity: Severity::Error,
                message: m.clone(),
                context: String::new(),
                help: None,
            });
        }
        out.sort_by(|a, b| (&a.file, a.range.0).cmp(&(&b.file, b.range.0)));
        out
    }

    /// The critical chain in full, one line per link, last node first,
    /// with each node's height:
    /// the link's cause with what it names (the site that defined the
    /// field read, the node waited on, the creator, the height a raise
    /// went to) and the node, each as its operation and text.
    #[must_use]
    pub fn critical_chain_lines(&self) -> Vec<String> {
        use phitex_defuse::Cause;
        let eng = &self.g.eng;
        let v = eng.values();
        let d = |n: Option<phitex_defuse::NodeId>| {
            n.map_or_else(|| "(unit start)".to_owned(), |n| describe(eng, n))
        };
        eng.critical_chain_heights()
            .into_iter()
            .enumerate()
            .map(|(i, (n, h, c))| {
                let why = match c {
                    Cause::Rebind { field, to, .. } => {
                        format!("Rebind {} to [{}]", field_name(v, field), d(to))
                    }
                    Cause::Changed { field, site } => {
                        format!("Changed {} at [{}]", field_name(v, field), d(site))
                    }
                    Cause::Output { of } => format!("Output of [{}]", d(Some(of))),
                    Cause::Woken { by } => format!("Woken by [{}]", d(Some(by))),
                    Cause::Loop { field } => format!("Loop {}", field_name(v, field)),
                    Cause::Environment { field } => {
                        format!("Environment {}", field_name(v, field))
                    }
                    Cause::Structure => "Structure".to_owned(),
                    Cause::Range { field, from } => {
                        format!("Range {} from [{}]", field_name(v, field), d(from))
                    }
                    Cause::Created { by, from_hint } => format!(
                        "Created by [{}]{}",
                        d(by),
                        from_hint.map_or_else(String::new, |(f, t)| format!(
                            ", hint {} at [{}]",
                            field_name(v, f),
                            d(Some(t))
                        ))
                    ),
                    Cause::Raised { to, by, how } => format!(
                        "Raised to height {to} by {:?} ({how:?})",
                        by.map(|n| n.raw() % 100_000)
                    ),
                };
                format!("{i:4} h{h:<5} {why}\n       node [{}]", describe(eng, n))
            })
            .collect()
    }

    /// The critical chain's waits in detail (a measure): for each
    /// `Woken` link, the waiter and the node it waited on (text, the
    /// hint's class and cause, `may_define`, effect, the hinted reads);
    /// for each `Raised` link, the node and its hinted reads.
    #[must_use]
    pub fn chain_waits(&self) -> Vec<String> {
        use phitex_defuse::Cause;
        let v = self.g.eng.values();
        let describe = |n: phitex_defuse::NodeId| -> String {
            let Some(i) = self.g.eng.node_info(n) else {
                return "gone".to_owned();
            };
            let text: String = i
                .args
                .first()
                .map(|&x| v.render(x))
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect();
            if i.op != "exec" {
                return format!("{} {text}", i.op);
            }
            let h = crate::hints::hint_of_args(v, &i.args);
            let md = match crate::hints::may_define(v, &i.args) {
                phitex_defuse::MayDefine::Any => "Any".to_owned(),
                other => format!("{other:?}").chars().take(160).collect(),
            };
            let eff = if crate::hints::effect_none(v, &i.args) {
                "None"
            } else {
                "Pending"
            };
            let reads: Vec<String> = crate::hints::reads(v, &i.args)
                .unwrap_or_default()
                .iter()
                .map(|f| field_name(v, *f))
                .collect();
            format!(
                "{text:?} [{}] may_define {md} effect {eff} reads {reads:?}",
                h.cause
            )
        };
        let mut out = Vec::new();
        for (n, c) in self.g.eng.critical_chain() {
            match c {
                Cause::Woken { by } => out.push(format!(
                    "Woken\n    waiter: {}\n    waited: {}",
                    describe(n),
                    describe(by)
                )),
                Cause::Raised { to, .. } => {
                    out.push(format!("Raised to {to}\n    node: {}", describe(n)));
                }
                _ => {}
            }
        }
        out
    }

    /// The engine's critical chain, from the highest finished node back:
    /// for each link the node's operation and first argument (shortened),
    /// the cause's kind, and its field if any ([`Engine::critical_chain`]).
    #[must_use]
    pub fn critical_chain(&self) -> Vec<(String, String, String)> {
        use phitex_defuse::Cause;
        let v = self.g.eng.values();
        self.g
            .eng
            .critical_chain()
            .into_iter()
            .map(|(n, c)| {
                let what = self.g.eng.node_info(n).map_or_else(String::new, |i| {
                    let a: String = i
                        .args
                        .first()
                        .map(|&x| v.render(x))
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect();
                    format!("{} {a}", i.op)
                });
                let (kind, field) = match c {
                    Cause::Rebind { field, .. } => ("Rebind", field_name(v, field)),
                    Cause::Changed { field, .. } => ("Changed", field_name(v, field)),
                    Cause::Output { .. } => ("Output", String::new()),
                    Cause::Woken { .. } => ("Woken", String::new()),
                    Cause::Loop { field } => ("Loop", field_name(v, field)),
                    Cause::Environment { field } => ("Environment", field_name(v, field)),
                    Cause::Structure => ("Structure", String::new()),
                    Cause::Range { field, .. } => ("Range", field_name(v, field)),
                    Cause::Created { from_hint, .. } => (
                        "Created",
                        from_hint.map_or_else(String::new, |(f, _)| field_name(v, f)),
                    ),
                    // the height it was raised to is in the dump, not the key
                    Cause::Raised { .. } => ("Raised", String::new()),
                };
                (what, kind.to_owned(), field)
            })
            .collect()
    }

    /// The pages shipped, first first: a hash of what each shows (a PDF
    /// page's content stream, uncompressed, with its size and
    /// `\pdfpageattr`; a DVI page's `bop`..`eop` bytes, the back pointer
    /// left out), with no object numbers or file offsets, so a page
    /// an edit leaves alone keeps its hash.
    #[must_use]
    pub fn pages(&self) -> Vec<u64> {
        let v = self.g.eng.values();
        let f = v.field("pagehash");
        self.g
            .eng
            .acc_value(self.unit, f)
            .map(|s| {
                v.seq_to_vec(s)
                    .into_iter()
                    .map(|x| u64::from_ne_bytes(int_of(v, x).to_ne_bytes()))
                    .collect()
            })
            .unwrap_or_default()
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

/// The Levenshtein distance between `a` and `b`, by characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// A node: its operation and the start of its first argument.
fn describe(eng: &Engine, n: phitex_defuse::NodeId) -> String {
    eng.node_info(n).map_or_else(String::new, |i| {
        let a = i.args.first().map(|&x| eng.values().render(x));
        let a: String = a.unwrap_or_default().chars().take(80).collect();
        format!("{} {a}", i.op)
    })
}

#[cfg(test)]
mod prelude_tests {
    use super::*;

    #[test]
    fn another_layout_or_engine_is_refused() {
        let c = Cached {
            key: [3; 32],
            text_key: 5,
            reads: vec![("a.tex".to_owned(), 9)],
            env: Arc::new(vec![("x".to_owned(), Tree::Leaf(Value::Int(1)))]),
            errors: Vec::new(),
        };
        let b = serialise(&c);
        assert!(deserialise_as(&b, PRELUDE_LAYOUT, engine_identity()).is_some());
        let other = serialise_as(&c, PRELUDE_LAYOUT + 1, engine_identity());
        assert!(deserialise(&other).is_none());
        let other = serialise_as(&c, PRELUDE_LAYOUT, engine_identity() ^ 1);
        assert!(deserialise(&other).is_none());
        // the old layout, with no header
        assert!(deserialise(&b[20..]).is_none());
    }
}
