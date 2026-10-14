//! The client: a document's text as nodes, the prelude's environment,
//! the files the graph asks for, edits, and the outputs read back (DVI
//! bytes, the log).
//!
//! Nodes come from the syntactic pass ([`phitex_source::pass`]) sliced by
//! `hints::slices_of`: one per paragraph region or loading command of a
//! document, one for a whole format. Files a run reads come in place
//! from the environment provider (`Graph::provide`).

use phitex_defuse::FxHashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{
    Engine, Field, NodeId, OpId, Report, SeqMonoid, SumMonoid, UnitId, Value, ValueId, Values,
};
use phitex_source::pass;

use crate::hints::Defs;
use crate::incr::Incr;
use crate::ops::{Dvi, Exec, Files, Mark, Put, Shared};
use crate::run::{Ops, slice_args, slice_args_in_file};
use crate::val::{field_name, int_of, list_of, str_of};

/// Finds a file by the name TeX asks for: the name to print and the
/// bytes.
pub type Finder = Arc<dyn Fn(&str) -> Option<(String, Vec<u8>)> + Send + Sync>;

/// Whether the job may write a file under a name (`\openout`): the
/// client's `kpse_out_name_ok` (its `openout_any`, its output
/// directories).
pub type OutNameOk = Arc<dyn Fn(&str) -> bool + Send + Sync>;

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
    /// Where the prelude (a format) finds its files, if not `finder`: a
    /// format is built away from the document's directory (fmtutil runs
    /// it in a scratch directory), so a document named `plain.tex` is not
    /// what `\input plain` reads while the format is made.
    pub prelude_finder: Option<Finder>,
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
    /// The round-one key sets of the read-back files that only define
    /// (`.aux`: `wkeys:<name>`), as [`Build::wkeys`] gave them in an
    /// earlier session; a file not listed is guessed from the text
    /// ([`guess_wkeys`]). A right guess saves the loop-back's second
    /// round; a wrong one costs nothing more than no guess.
    pub wkeys: Vec<(String, Vec<String>)>,
    /// The round-one values of the page builder's loop-backs
    /// (`pagefx:<j>`, what page `j`'s output routine leaves for the
    /// next), as [`Build::export_pagefx`] gave them in an earlier
    /// session of the same document; empty: none left (the declared
    /// value).
    pub pagefx: Vec<u8>,
    /// The round-one values of the page chain's loop-backs (`pgst:<j>`
    /// and `pgdef:<j>`, the state each page's routine leaves for the
    /// next), as [`Build::export_pgst`] gave them in an earlier session
    /// of the same document; empty: the cold guesses.
    pub pgst: Vec<u8>,
    /// The package loads' records ([`Build::export_pkgrec`]): kept
    /// machine-wide beside the format, keyed by the load and the files
    /// it read, never by the document, so a new document's first build
    /// uses what any earlier one learned (`crate::pkgrec`). Empty: none
    /// (every load unbounded, or optimistic under speculation).
    pub pkgrec: Vec<u8>,
    /// Whether the records carry the values each load published, and a
    /// load whose key matches is given them as the prediction of its
    /// outputs (also `PHITEX_PKG_PREDICT=1`).
    pub pkg_predict: bool,
    /// The loop-back rounds an evaluation may take before it ends with
    /// a no-fixpoint error naming the loop-back fields (a document whose
    /// written files oscillate), instead of running on.
    pub max_rounds: u64,
    /// Whether `\openout` may write a file under a name (pdfTeX asks
    /// kpathsea, §1374; a refused name ends a nonstop job); `None`:
    /// TeX Live's default, `openout_any = p` with no output directory
    /// (`control::out_name_ok_p`).
    pub out_name_ok: Option<OutNameOk>,
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
    nodes: &'a mut Vec<(u64, NodeId, Src)>,
    exec: OpId,
    /// The first node after the text's ([`Build::end`]).
    end: NodeId,
    /// The document's units and their analyses (`None`: a format's
    /// text, one node).
    incr: Option<&'a mut Incr>,
    /// The latest version taken whose text was the one the nodes have
    /// already: nothing applied, so not reported to the engine (its
    /// contract: `None` when nothing was applied); the result is for it.
    unchanged: Option<u64>,
    /// The finder `\\input` files are sliced with, and the text applied.
    finder: Finder,
    text: &'a mut String,
    /// The package loads' hints (`crate::pkgrec`).
    loads: Option<&'a crate::pkgrec::LoadHints>,
}

impl phitex_defuse::EditSource for Absorb<'_> {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        let (text, version) = self.edits.take()?;
        let changed = apply_text(
            eng,
            self.exec,
            self.end,
            self.nodes,
            &text,
            self.incr.as_deref_mut(),
            Some(&self.finder),
            self.loads,
        );
        *self.text = text;
        if changed {
            self.unchanged = None;
            Some(version)
        } else {
            self.unchanged = Some(version);
            None
        }
    }
}

/// Declares node `n`'s `marks:before:<n>` loop-back: the marks its
/// `\topmark` and its kin read, which the page builder's fold
/// publishes; UNIT (no page fired yet) until it does.
fn declare_marks_before(eng: &mut Engine, n: NodeId) {
    let f = eng
        .values()
        .field(&crate::etex::marks_before_field(n.raw()));
    eng.declare_loopback(f, ValueId::UNIT);
    // how many pages fired before the node's marker (the writer's
    // read-time writes after a `\shipout`, `etex::page_shipped_before`)
    let f = eng
        .values()
        .field(&crate::etex::fired_before_field(n.raw()));
    let zero = eng.values().int(0);
    eng.declare_loopback(f, zero);
}

/// Edited texts of `\\input` files by name: the text and its bytes.
type Overrides = Arc<Mutex<FxHashMap<String, (String, Vec<u8>)>>>;

/// `PHITEX_TEX_TIME`: an edit's apply, its phases (`t`: slices, hints,
/// diff, as elapsed since `t_all`) and slice counts (`n`: old, new, kept,
/// replaced, removed, inserted, text bytes).
fn edit_apply_note(t_all: std::time::Instant, t: [std::time::Duration; 3], n: [usize; 7]) {
    if !crate::exec::env_flag!("PHITEX_TEX_TIME") {
        return;
    }
    let [t_slices, t_hints, t_diff] = t;
    eprintln!(
        "edit apply: {:?} total; hint pass (units spliced, the chunks analysed again) {t_slices:?}, nodes and keys {:?}, slice-key diff {:?}, engine replace/insert/remove {:?}; {} old / {} new slices, {} kept, {} replaced, {} removed, {} inserted, text {} bytes",
        t_all.elapsed(),
        t_hints.checked_sub(t_slices).unwrap_or_default(),
        t_diff.checked_sub(t_hints).unwrap_or_default(),
        t_all.elapsed().checked_sub(t_diff).unwrap_or_default(),
        n[0],
        n[1],
        n[2],
        n[3],
        n[4],
        n[5],
        n[6]
    );
}

/// The lengths of the key prefix and suffix `old` and `new` share
/// (not overlapping).
fn common_ends(old: &[(u64, NodeId, Src)], new: &[(u64, DocNode)]) -> (usize, usize) {
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
    (a, b)
}

/// The text's nodes and their keys: a document's (`incr`) per chunk of
/// its units ([`doc_nodes`]), a format's the whole text as one.
fn slice_keys(
    v: &Values,
    text: &str,
    incr: Option<&Incr>,
    finder: Option<&Finder>,
    read: &mut Vec<String>,
) -> Vec<(u64, DocNode)> {
    match incr {
        Some(inc) => {
            register_doc(v, inc);
            doc_nodes(v, text, inc, finder, read)
        }
        None if text.is_empty() => Vec::new(),
        None => vec![(key(text, true), DocNode::Doc(0, text.len()))],
    }
}

/// Edits the document's nodes `nodes` to the text `text`: the slices
/// both texts share at the start and the end stay, those between are
/// replaced, removed or inserted. Whether anything changed (a text
/// whose slices are the nodes' already changes nothing).
// one call site (Absorb::absorb), which passes its fields apart
#[allow(clippy::too_many_arguments)]
fn apply_text(
    eng: &mut Engine,
    exec: OpId,
    end: NodeId,
    nodes_now: &mut Vec<(u64, NodeId, Src)>,
    text: &str,
    mut incr: Option<&mut Incr>,
    finder: Option<&Finder>,
    loads: Option<&crate::pkgrec::LoadHints>,
) -> bool {
    let t_all = std::time::Instant::now();
    let text = as_file(text);
    // (the units spliced, the chunks whose text or incoming definitions
    // changed analysed again)
    if let Some(inc) = incr.as_deref_mut() {
        inc.set_text(&text);
    }
    let incr = incr.as_deref();
    let t_slices = t_all.elapsed();
    let mut read = Vec::new();
    let new = slice_keys(eng.values(), &text, incr, finder, &mut read);
    let t_hints = t_all.elapsed();
    let old = &*nodes_now;
    if old.len() == new.len() && old.iter().zip(&new).all(|(o, n)| o.0 == n.0) {
        return false;
    }
    let args_of = |eng: &Engine, k: usize| node_args(eng.values(), &text, incr, &new[k].1, loads);
    let (a, b) = common_ends(old, &new);
    let old_len = old.len();
    let (old_mid, new_mid) = (old.len() - a - b, new.len() - a - b);
    let common = old_mid.min(new_mid);
    let t_diff = t_all.elapsed();
    for k in common..old_mid {
        eng.remove(old[a + k].1);
    }
    let mut nodes: Vec<(u64, NodeId, Src)> = Vec::with_capacity(new.len());
    for i in 0..new.len() {
        let n = if i < a || i >= a + new_mid {
            let oi = if i < a { i } else { i - new_mid + old_mid };
            old[oi].1
        } else if i - a < common {
            let n = old[i].1;
            let args = args_of(eng, i);
            eng.replace(n, exec, args);
            declare_marks_before(eng, n);
            n
        } else {
            let args = args_of(eng, i);
            let n = if let Some(&(_, p, _)) = nodes.last() {
                eng.insert_after(p, exec, args)
            } else {
                let first = old.get(a + old_mid).map_or(end, |x| x.1);
                eng.insert_before(first, exec, args)
            };
            declare_marks_before(eng, n);
            n
        };
        nodes.push((new[i].0, n, Src::of(&new[i].1)));
    }
    *nodes_now = nodes;
    edit_apply_note(
        t_all,
        [t_slices, t_hints, t_diff],
        [
            old_len,
            new.len(),
            a + b,
            common,
            old_mid - common,
            new_mid - common,
            text.len(),
        ],
    );
    true
}

impl Default for Options {
    fn default() -> Self {
        Options {
            name: "./texput.tex".to_owned(),
            jobname: "texput".to_owned(),
            comment: String::new(),
            finder: Arc::new(|_| None),
            prelude_finder: None,
            epoch: None,
            fixpoint: false,
            wkeys: Vec::new(),
            pagefx: Vec::new(),
            pgst: Vec::new(),
            pkgrec: Vec::new(),
            pkg_predict: false,
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
            max_rounds: 64,
            out_name_ok: None,
        }
    }
}

/// A value outside any table, to carry an environment between engines.
#[derive(Clone, Debug)]
pub(crate) enum Tree {
    Leaf(Value),
    List(Vec<Tree>),
    Seq(Vec<Tree>),
}

pub(crate) fn export(v: &Values, x: ValueId) -> Tree {
    match &*v.get(x) {
        Value::List(xs) => Tree::List(xs.iter().map(|&y| export(v, y)).collect()),
        Value::Seq(_) => Tree::Seq(v.seq_to_vec(x).into_iter().map(|y| export(v, y)).collect()),
        other => Tree::Leaf(other.clone()),
    }
}

pub(crate) fn import(v: &Values, t: &Tree) -> ValueId {
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

/// Value `x` as bytes, outside any table ([`tree_from_bytes`]).
pub(crate) fn tree_bytes(v: &Values, x: ValueId) -> Vec<u8> {
    let mut out = Vec::new();
    put_tree(&mut out, &export(v, x));
    out
}

/// The value [`tree_bytes`] wrote, in `v`.
pub(crate) fn tree_from_bytes(v: &Values, b: &[u8]) -> Option<ValueId> {
    let mut r = Rd(b);
    let t = r.tree()?;
    r.0.is_empty().then(|| import(v, &t))
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
    /// TeX's errors in the prelude (a format whose text stops on them),
    /// at their places in its files: reported with every build on it,
    /// and such a prelude is never exported.
    diags: Vec<Diagnostic>,
}

/// Prelude environments by the hash of the prelude's text; each is good
/// while the files it read are unchanged.
fn env_cache() -> &'static Mutex<FxHashMap<u64, Vec<Arc<Cached>>>> {
    static C: OnceLock<Mutex<FxHashMap<u64, Vec<Arc<Cached>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
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
        let xs = list_of(v, stream_entry(v, e));
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

/// A `streams` entry without its page tag (`[j, entry]` from page
/// step `j`, [`crate::run::Run::contribute_stream`]).
pub(crate) fn stream_entry(v: &Values, e: ValueId) -> ValueId {
    let xs = list_of(v, e);
    match xs.as_slice() {
        [j, x] if v.as_int(*j).is_some() => *x,
        _ => e,
    }
}

/// The `streams` entries in the order TeX writes them: the page
/// builder's plan (`wplan`: an immediate write's entry where its marker
/// stood, page `j` where it fired) with each page's entries in place.
/// Without a plan entry for them, the document nodes' entries go first.
pub(crate) fn ordered_entries(v: &Values, plan: &[ValueId], streams: &[ValueId]) -> Vec<ValueId> {
    let mut by_page: std::collections::BTreeMap<i64, Vec<ValueId>> =
        std::collections::BTreeMap::default();
    let mut imm = Vec::new();
    for &e in streams {
        let xs = list_of(v, e);
        match xs.as_slice() {
            [j, x] if v.as_int(*j).is_some() => {
                by_page
                    .entry(v.as_int(*j).unwrap_or(0))
                    .or_default()
                    .push(*x);
            }
            _ => imm.push(e),
        }
    }
    let mut out = Vec::new();
    if !plan.iter().any(|&p| v.as_int(p).is_none()) {
        out.extend(imm);
    }
    for &p in plan {
        match v.as_int(p) {
            Some(j) => out.extend(by_page.remove(&j).unwrap_or_default()),
            None => out.push(p),
        }
    }
    for (_, es) in by_page {
        out.extend(es);
    }
    out
}

/// The texts of the files the job wrote, from the accumulated `wplan`
/// and `streams` sequences, in the order first opened.
pub(crate) fn written_texts(
    v: &Values,
    wplan: Option<ValueId>,
    streams: Option<ValueId>,
) -> Vec<(String, String)> {
    let plan: Vec<ValueId> = wplan
        .map(|p| v.seq_to_vec(p))
        .unwrap_or_default()
        .into_iter()
        .flat_map(|p| list_of(v, p))
        .collect();
    let streams = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
    stream_texts(v, &ordered_entries(v, &plan, &streams))
}

/// The definitions the read-back of each job-written file would make
/// (the third element of its `streams` entries, [`Run::interpret_line`]),
/// in TeX's order: per file, `(field, [value, global])`, each field once,
/// where it was first defined, with its last value.
pub(crate) fn written_defs(
    v: &Values,
    wplan: Option<ValueId>,
    streams: Option<ValueId>,
) -> Vec<(String, Vec<(String, ValueId)>)> {
    let plan: Vec<ValueId> = wplan
        .map(|p| v.seq_to_vec(p))
        .unwrap_or_default()
        .into_iter()
        .flat_map(|p| list_of(v, p))
        .collect();
    let streams = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
    let mut out: Vec<(String, Vec<(String, ValueId)>)> = Vec::new();
    for e in ordered_entries(v, &plan, &streams) {
        let xs = list_of(v, stream_entry(v, e));
        let Some(&made) = xs.get(2) else { continue };
        let name = str_of(v, xs[0]);
        let k = if let Some(k) = out.iter().position(|(n, _)| *n == name) {
            k
        } else {
            out.push((name, Vec::new()));
            out.len() - 1
        };
        for d in list_of(v, made) {
            let ds = list_of(v, d);
            let [f, x, g] = ds.as_slice() else { continue };
            let f = str_of(v, *f);
            let val = v.list(vec![*x, *g]);
            match out[k].1.iter_mut().find(|(n, _)| *n == f) {
                Some(slot) => slot.1 = val,
                None => out[k].1.push((f, val)),
            }
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

/// The vendored engine's identity: a hash of its sources and of this
/// crate's (`build.rs`), so vendoring another engine or changing how
/// this crate encodes its state invalidates every exported prelude.
fn engine_identity() -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        let own = u64::from_str_radix(env!("PHITEX_TEX_SRC_HASH"), 16).unwrap_or(0);
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
        let mut h = own;
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
        diags: Vec::new(),
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

/// What the evaluations of one build or edit did.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Page-step runs that reached their finish in this evaluation, by
    /// page index (the attempts that stopped or were cut off are not
    /// counted).
    pub page_steps_completed: Vec<u64>,
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
    /// Waits on a node whose declared set was pending, and how many of
    /// those sets then defined the field (the necessary ones).
    pub pending_sets: u64,
    pub pending_sets_defining: u64,
    pub stops: u64,
    /// Attempts that resumed a stopped run at its blocked read, and that
    /// started over though one was kept (a read changed).
    pub resumed: u64,
    pub restarted: u64,
    /// Stopped runs restarted instead of resumed, by the first read that
    /// refused the resume.
    pub restarted_by: Vec<(&'static str, u64)>,
    /// Environment reads the engine's provider answered in place.
    pub env_provided: u64,
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
    /// The engine's realised critical path (`Report::realised_critical_path`:
    /// the longest chain of runs that waited for each other, counted from
    /// 1; reads a guess or a prediction answered are no link), the most
    /// over this build's evaluations.
    pub realised_critical_path: u32,
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
    /// The texts taken while the engine evaluated (the engine's
    /// `edits_applied`): each one's version and the step it came before.
    pub absorbed: Vec<(u64, u64)>,
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
    /// The page chain's loop-back guesses the evaluation refuted, with
    /// the fields that refuted them (`refuted: <fields> at page <n>
    /// (<loop-back>)`).
    pub refuted_guesses: Vec<String>,
    /// The engine's speculative runs: kept with outstanding
    /// assumptions, confirmed, and whose assumptions failed; and nodes
    /// re-done higher after running below a finished input (raises).
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    /// Predicted definitions ([`Options::pkg_predict`]) the runs
    /// confirmed, and refuted (their readers re-ran).
    pub predictions_confirmed: u64,
    pub predictions_refuted: u64,
    /// Speculative runs dropped at a stop (their guess is then blocked
    /// until its node leaves work).
    pub spec_stops: u64,
    /// Speculative runs dropped at apply because they changed the
    /// node's shape (`Report::spec_shape_changes`).
    pub spec_shape_changes: u64,
    /// Results dropped at apply because the node went away or was
    /// re-bound (`dead_result`), or no longer takes a result
    /// (`state_result`): the engine's probe counters, totals since their
    /// last reset, with the `profile` feature only (0 without it).
    pub dead_result: u64,
    pub state_result: u64,
    /// The engine's parallel-apply probe counters (`profile` feature,
    /// totals since their last reset): tail passes, use records, and
    /// slices aborted.
    pub tail: u64,
    pub use_record: u64,
    pub slice_abort: u64,
    pub raises: u64,
    /// The most nodes ready at one step, and the speculative runs that
    /// ran out of fuel.
    pub max_ready: usize,
    pub fuel_outs: u64,
    /// Declared definition sets the engine refuted (a run defined
    /// outside its node's hint): a package load's record among them.
    pub declarations_refuted: u64,
    /// The document's package loads (`crate::pkgrec`), those given a
    /// record and those given the optimistic hint (since the build
    /// began), their finished runs (speculative ones included), the
    /// most at once, the span from the first's start to the last's
    /// end, and how many are on the critical chain.
    pub pkg_loads: u64,
    /// Of those, the loads another node's run took (claimed: not run
    /// as their own node).
    pub pkg_claimed: u64,
    pub pkg_recorded: u64,
    pub pkg_optimistic: u64,
    pub pkg_runs: u64,
    pub pkg_max_concurrent: usize,
    pub pkg_wall: std::time::Duration,
    pub pkg_on_critical: u64,
    /// Loads whose record predicted their outputs
    /// ([`Options::pkg_predict`]), of those whose run published exactly
    /// the predicted values, and the fields predicted wrong (a value
    /// that differs, or a field one side lacks), as `key: field`.
    pub pkg_predicted: u64,
    pub pkg_predict_right: u64,
    pub pkg_predict_wrong: Vec<String>,
}

/// A round's counters (`PHITEX_TEX_TIME`).
fn print_round(t: std::time::Duration, r: &Report) {
    eprintln!(
        "round: {t:?}, {} nodes run, {} runs, {} gated, {} cutoffs, {} rebinds, {} restarts, {} loop rounds, {} stops, {} env_provided, {} raises, {} pending, {} pending sets ({} defining), {} created, {} freed, spec {}/{}/{}, by op {:?}",
        r.nodes_run,
        r.runs,
        r.gated,
        r.cutoffs,
        r.rebinds,
        r.restarts,
        r.rounds,
        r.stops,
        r.env_provided,
        r.raises,
        r.pending_waits,
        r.pending_sets,
        r.pending_sets_defining,
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
        if phitex_defuse::obs::enabled() {
            for &(name, n) in &phitex_defuse::obs::snapshot().counters {
                match name {
                    "dead_result" => self.dead_result = n,
                    "state_result" => self.state_result = n,
                    "tail" => self.tail = n,
                    "use_record" => self.use_record = n,
                    "slice_abort" => self.slice_abort = n,
                    _ => {}
                }
            }
        }
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
        self.pending_sets += r.pending_sets;
        self.pending_sets_defining += r.pending_sets_defining;
        self.stops += r.stops;
        self.resumed += r.resumed;
        self.restarted += r.restarted;
        for &(k, n) in &r.restarted_by {
            match self.restarted_by.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += n,
                None => self.restarted_by.push((k, n)),
            }
        }
        self.env_provided += r.env_provided;
        self.version = r.version;
        self.absorbed.extend_from_slice(&r.edits_applied);
        self.steps += r.steps;
        self.ready_total += r.ready_total;
        self.speculations += r.speculations;
        self.confirmations += r.confirmations;
        self.mispredictions += r.mispredictions;
        self.predictions_confirmed += r.predictions_confirmed;
        self.predictions_refuted += r.predictions_refuted;
        self.realised_critical_path = self.realised_critical_path.max(r.realised_critical_path);
        self.spec_stops += r.spec_stops;
        self.spec_shape_changes += r.spec_shape_changes;
        self.raises += r.raises;
        self.max_ready = self.max_ready.max(r.max_ready);
        self.fuel_outs += r.fuel_outs;
        self.declarations_refuted += r.declarations_refuted;
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
    /// Why the exact PDF writer folds the pages (`pdf:fold`), if it does.
    pdf_fold: Option<String>,
    /// The `marks:before:<node>` loop-backs declared for nodes the graph
    /// made (they asked through `marksdecl:<node>`), as the
    /// environment's answers.
    marks_decl: Vec<(Field, ValueId)>,
    /// The `wdef:<field>` loop-backs declared: the fields the read-back
    /// of a job-written file defines.
    wdefs: Vec<String>,
    /// The round-one guesses of the `wkeys:<name>` loop-backs.
    wkeys_guess: Vec<(String, Vec<String>)>,
    /// What the executor runs now, for readers on other threads.
    watch: crate::exec::Watch,
    /// The engine's scheduler ([`Options::speculation`]).
    sched: phitex_defuse::Greedy,
    /// The files the environment provider found or missed during runs
    /// (name, field, value, hash), merged into `files`/`absent`/`reads`
    /// before the environment is set again (`Graph::provide`).
    provided: Arc<std::sync::Mutex<Vec<Provided>>>,
    /// The document files sliced into the document's regions
    /// (`crate::inputs`), and their texts as edited (`Build::edit_file`)
    /// over the finder's.
    inputs_read: Vec<String>,
    overrides: Overrides,
    /// The package loads' records and hints (`crate::pkgrec`), once the
    /// build has set them.
    loads: Option<Arc<crate::pkgrec::LoadHints>>,
}

impl Graph {
    /// The page-step runs that reached their finish since the last call,
    /// by page index ([`Stats::page_steps_completed`]).
    fn take_refuted(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .ops
                .lblog
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
        .refuted()
    }

    fn take_steps_done(&self) -> Vec<u64> {
        std::mem::take(
            &mut *self
                .ops
                .steps_done
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

/// A file the environment provider answered: name, field, the value
/// (`None`: absent), and the finder's hash.
type Provided = (String, Field, Option<ValueId>, u64);

impl Graph {
    #[allow(clippy::too_many_lines)]
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
        // our loop-backs (pagefx, marks:before) have a unique fixpoint, so an
        // edit starts from the last values
        eng.set_loop_start(phitex_defuse::LoopStart::Previous);
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let [pagestep, pagestep_any] = [false, true].map(|any| {
            eng.register(Arc::new(crate::page::PageStep {
                shared: shared.clone(),
                any,
            }))
        });
        let pageplan = eng.register(Arc::new(crate::page::PagePlan(shared.clone())));
        let brk = eng.register(Arc::new(crate::page::Brk(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi(shared.clone())));
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
            pageplan,
            brk,
            fields,
            defs: Arc::new(Mutex::new(crate::hints::no_defs().clone())),
            steps_done: Arc::new(Mutex::new(Vec::new())),
            lblog: Arc::new(Mutex::new(crate::page::LbLog::default())),
            pkg_log: Arc::new(Mutex::new(Vec::new())),
            pkg_loads: Arc::new(Mutex::new(None)),
            out_ok: Arc::new(Mutex::new(None)),
        };
        let _ = shared.set(ops.clone());
        declare_fields(&mut eng);
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
            pdf_fold: None,
            marks_decl: Vec::new(),
            wdefs: Vec::new(),
            wkeys_guess: Vec::new(),
            watch: crate::exec::Watch::new(crate::exec::threads(threads), None),
            sched: phitex_defuse::Greedy::default(),
            eval_errors: Vec::new(),
            provided: Arc::default(),
            inputs_read: Vec::new(),
            overrides: Arc::default(),
            loads: None,
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
        // (the engine does not say which loop-backs kept changing: the
        // job's read-back fields are the candidates)
        if let E::NonConvergence { .. } = e {
            for name in &self.written {
                let _ = write!(out, "\n  written file {name}");
            }
            for f in &self.wdefs {
                let _ = write!(out, "\n  read-back field {f}");
            }
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
        self.reads
            .push((name.to_owned(), file_hash(found.as_ref())));
        if let Some((path, bytes)) = found {
            let text: String = bytes.iter().map(|&b| char::from(b)).collect();
            let val = v.list(vec![v.str(&path), v.str(&text)]);
            self.files.push((f, val));
        } else {
            self.absent.push(f);
        }
    }

    /// Installs the environment provider: a run reading a file the
    /// environment does not hold gets it from the finder in place, as
    /// [`Graph::supply`] would have supplied it, instead of stopping;
    /// a file the job writes is not answered (it is read back).
    fn provide(&mut self, finder: &Finder) {
        let finder = Arc::clone(finder);
        let log = Arc::clone(&self.provided);
        let written = self.written.clone();
        let t0 = std::time::Instant::now();
        let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let spent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        // a file asked for again in the evaluation (by every page step,
        // say) is answered from here, not read and converted again; its
        // first asker reads it, concurrent askers wait for that answer
        let answered: std::sync::Mutex<
            FxHashMap<String, Arc<std::sync::OnceLock<Option<ValueId>>>>,
        > = std::sync::Mutex::new(FxHashMap::default());
        self.eng.set_env_provider(Box::new(move |v, f| {
            let name = v.as_str(f.0)?;
            let name = name.strip_prefix("file:")?;
            if written.iter().any(|w| w == name) {
                return None;
            }
            let cell = answered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(name.to_owned())
                .or_default()
                .clone();
            let answer = || {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let t1 = std::time::Instant::now();
                let found = finder(name);
                let hash = file_hash(found.as_ref());
                let val = found.map(|(path, bytes)| {
                    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
                    v.list(vec![v.str(&path), v.str(&text)])
                });
                if let Ok(mut l) = log.lock() {
                    l.push((name.to_owned(), f, val, hash));
                }
                let ns = u64::try_from(t1.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let all = spent.fetch_add(ns, std::sync::atomic::Ordering::Relaxed) + ns;
                if crate::exec::env_flag!("PHITEX_TEX_TIME") {
                    #[allow(clippy::cast_precision_loss)]
                    let all = all as f64 / 1e9;
                    eprintln!(
                        "provider: at {:.1} s, {n} answers, {all:.3} s in the provider, last {name}",
                        t0.elapsed().as_secs_f64()
                    );
                }
                val
            };
            Some(*cell.get_or_init(answer))
        }));
    }

    /// Merges what the provider answered into the supplied files, so the
    /// next environment set is the whole environment.
    fn absorb_provided(&mut self) {
        let got = self
            .provided
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default();
        for (name, f, val, hash) in got {
            if self.files.iter().any(|(g, _)| *g == f)
                || self.absent.contains(&f)
                || self.written.contains(&name)
            {
                continue;
            }
            self.queries.push((name.clone(), val.is_some()));
            self.reads.push((name, hash));
            match val {
                Some(x) => self.files.push((f, x)),
                None => self.absent.push(f),
            }
        }
    }

    /// The finder the document's `\\input` files are sliced with: the
    /// edited texts first ([`Build::edit_file`]).
    fn input_finder(&self, base: &Finder) -> Finder {
        let base = Arc::clone(base);
        let over = Arc::clone(&self.overrides);
        Arc::new(move |n: &str| {
            let o = over
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(n)
                .cloned();
            o.or_else(|| base(n))
        })
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

    /// Round one's guesses for the loop-backs: the last session's
    /// (`opts`), else from the text `document`.
    fn guesses(&mut self, opts: &Options, document: &str) {
        // the last session's page-builder loop-backs, round one's guess
        {
            let mut rd = Rd(&opts.pagefx);
            while let (Some(j), Some(t)) = (rd.u64(), rd.tree()) {
                if usize::try_from(j).is_ok_and(|j| j < crate::page::MAX_PAGES) {
                    let v = self.eng.values();
                    let f = v.field(&format!("pagefx:{j}"));
                    let x = import(v, &t);
                    self.eng.declare_loopback(f, x);
                }
            }
        }
        let aux = format!("{}.aux", opts.jobname);
        self.wkeys_guess.clone_from(&opts.wkeys);
        if !self.wkeys_guess.iter().any(|(n, _)| *n == aux) {
            self.wkeys_guess.push((aux, guess_wkeys(document)));
        }
    }

    /// A file the job writes is read back: from the previous round
    /// (`fixpoint`: a loop-back field, absent in round one, as LaTeX's
    /// `\@input` finds no `.aux` on a first run), or absent.
    ///
    /// With `fixpoint` the file is an in-memory value, `wfile:<name>`: a
    /// loop-back the writer (`Files`) publishes, its first value the
    /// file on disk if there is one (the last build's fixpoint), else
    /// absent; `file:<name>` is absent, so a run that read it from the
    /// finder before this runs again and reads the loop-back.
    fn written_file(&mut self, name: &str, fixpoint: bool, _finder: &Finder) {
        if self.written.iter().any(|w| w == name) {
            return;
        }
        let f = self.eng.values().field(&format!("file:{name}"));
        self.files.retain(|(g, _)| *g != f);
        self.absent.retain(|g| *g != f);
        self.reads.retain(|(n, _)| n != name);
        self.absent.push(f);
        if fixpoint {
            // nothing reads the bytes on disk: a read-back that only
            // defines (`.aux`) is its key set `wkeys:<name>` and one
            // `wdef:<field>` per field; another file is its text in
            // memory, `wfile:<name>`; both start empty (absent)
            let v = self.eng.values();
            let none = v.list(Vec::new());
            let k = if defines_only(name) { "wkeys" } else { "wfile" };
            let w = v.field(&format!("{k}:{name}"));
            // a key set guessed (the last session's, or from the text):
            // the read-back runs once when the guess is right
            let guess = self
                .wkeys_guess
                .iter()
                .find(|(n, _)| n == name && defines_only(name))
                .map_or(none, |(_, ks)| {
                    v.list(ks.iter().map(|k| v.str(k)).collect())
                });
            self.eng.declare_loopback(w, guess);
            // the guessed keys' `wdef:<field>` loop-backs declared with
            // it: a read-back of a guessed key never waits for them
            let keys: Vec<String> = self
                .wkeys_guess
                .iter()
                .filter(|(n, _)| n == name && defines_only(name))
                .flat_map(|(_, ks)| ks.iter().cloned())
                .collect();
            for k in keys {
                if !self.wdefs.contains(&k) {
                    let v = self.eng.values();
                    let none = v.list(Vec::new());
                    let wd = v.field(&format!("wdef:{k}"));
                    self.eng.declare_loopback(wd, none);
                    self.wdefs.push(k);
                }
            }
        }
        self.written.push(name.to_owned());
    }

    /// The environment: the files supplied, and the loop-back files.
    fn environment(&self, fixpoint: bool) -> Vec<(Field, ValueId)> {
        let v = self.eng.values();
        let mut env = self.files.clone();
        env.extend(self.marks_decl.iter().copied());
        for n in &self.written {
            env.push((v.field(&format!("wfiledecl:{n}")), ValueId::UNIT));
        }
        for f in &self.wdefs {
            env.push((v.field(&format!("wdefdecl:{f}")), ValueId::UNIT));
        }
        let set = self.wdefs.iter().map(|f| v.str(f)).collect();
        env.push((v.field("wdefset"), v.list(set)));
        if self.pdf_exact.is_some() {
            env.push((v.field("pdf:exact"), v.int(1)));
        }
        if self.pdf_fold.is_some() {
            env.push((v.field("pdf:fold"), v.int(1)));
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
        let tm = crate::exec::env_flag!("PHITEX_TEX_TIME");
        let (mut t_prov, mut t_eval, mut t_after) = (
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
        );
        let mut t_mark = std::time::Instant::now();
        loop {
            t_after += t_mark.elapsed();
            let tp = std::time::Instant::now();
            if !crate::exec::env_flag!("PHITEX_NO_PROVIDER") {
                self.provide(finder);
            }
            t_prov += tp.elapsed();
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
            t_eval += t0.elapsed();
            t_mark = std::time::Instant::now();
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
                    self.written_file(name, fixpoint, finder);
                    asked = true;
                }
            }
            // nodes made in the graph that read the marks: their
            // `marks:before:<node>` loop-backs declared
            let decl: Vec<(NodeId, Field)> = {
                let v = self.eng.values();
                r.env_waits
                    .iter()
                    .filter(|&&(_, f)| field_name(v, f).starts_with("marksdecl:"))
                    .copied()
                    .collect()
            };
            for (n, f) in decl {
                if self.marks_decl.iter().any(|&(g, _)| g == f) {
                    continue;
                }
                declare_marks_before(&mut self.eng, n);
                self.marks_decl.push((f, ValueId::UNIT));
                asked = true;
            }
            // files a run opened for writing: their `wfile:` loop-backs
            // declared (`wfiledecl:<name>`)
            let wdecl: Vec<String> = {
                let v = self.eng.values();
                r.env_waits
                    .iter()
                    .filter_map(|&(_, f)| {
                        field_name(v, f)
                            .strip_prefix("wfiledecl:")
                            .map(str::to_owned)
                    })
                    .collect()
            };
            for name in wdecl {
                if !self.written.contains(&name) {
                    self.written_file(&name, fixpoint, finder);
                }
                asked = true;
            }
            // the fields a job-written file's read-back defines: their
            // `wdef:<field>` loop-backs declared
            if fixpoint {
                let v = self.eng.values();
                let acc = |n: &str| self.eng.acc_value(unit, v.field(n));
                let defs = written_defs(v, acc("wplan"), acc("streams"));
                let mut new = Vec::new();
                for (_, ds) in defs {
                    for (f, _) in ds {
                        if !self.wdefs.contains(&f) && !new.contains(&f) {
                            new.push(f);
                        }
                    }
                }
                for f in new {
                    let none = self.eng.values().list(Vec::new());
                    let w = self.eng.values().field(&format!("wdef:{f}"));
                    self.eng.declare_loopback(w, none);
                    self.wdefs.push(f);
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
                // the exact writer's pages cannot be written apart:
                // the build again, with the pages in the `pdf:state` fold
                if kind == "pdf:fold" && self.pdf_fold.is_none() {
                    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
                        eprintln!("pdf: the pages folded again: {why}");
                    }
                    self.pdf_fold = Some(why.clone());
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
                    self.written_file(&name, fixpoint, finder);
                    asked = true;
                    continue;
                }
                self.supply(finder, &name);
                asked = true;
            }
            // what the provider answered in place joins the supplied
            // files (an edit of one replaces it, [`Build::edit_file`]),
            // whether or not this round asked for more
            self.absorb_provided();
            if !asked {
                t_after += t_mark.elapsed();
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
        if tm {
            eprintln!(
                "settle phases: provider {t_prov:?}, evaluate (rounds, the writer's nodes among them) {t_eval:?}, after the rounds (files, loop-backs, stats) {t_after:?}"
            );
        }
    }

    /// The document's nodes pushed onto `unit`, with its units kept for
    /// its edits.
    fn push_document(
        &mut self,
        unit: UnitId,
        text: &str,
        defs: Defs,
        doc: bool,
        finder: Option<&Finder>,
    ) -> (Vec<(u64, NodeId, Src)>, Incr) {
        let mut incr = Incr::new(defs);
        incr.threads = self.threads;
        let nodes = self.push_text(unit, &as_file(text), doc.then_some(&mut incr), finder);
        (nodes, incr)
    }

    /// The text's nodes pushed onto `unit`: a document's (`incr`) its
    /// paragraph regions, an `\\input` file's in place of the command
    /// (`crate::inputs`); a format's one node.
    fn push_text(
        &mut self,
        unit: UnitId,
        text: &str,
        mut incr: Option<&mut Incr>,
        finder: Option<&Finder>,
    ) -> Vec<(u64, NodeId, Src)> {
        let t0 = std::time::Instant::now();
        if let Some(inc) = incr.as_deref_mut() {
            inc.set_text(text);
        }
        let incr = incr.as_deref();
        // a format's text is one node: a sequential run over the whole
        // file, its `\input`s inline; what it may define is unknowable
        // from its parts, so anything (it runs once, serially, cached)
        let nodes: Vec<(u64, DocNode)> = match incr {
            Some(inc) => {
                register_doc(self.eng.values(), inc);
                let mut read = Vec::new();
                let n = doc_nodes(self.eng.values(), text, inc, finder, &mut read);
                self.inputs_read = read;
                n
            }
            None if text.is_empty() => Vec::new(),
            None => vec![(key(text, true), DocNode::Doc(0, text.len()))],
        };
        let t1 = t0.elapsed();
        // (the nodes' arguments made on the pool: values are named by
        // their content, whichever worker makes them)
        let args = {
            let v = self.eng.values();
            let loads = self.loads.as_deref();
            crate::exec::map(self.threads, nodes.len(), &|k| {
                node_args(v, text, incr, &nodes[k].1, loads)
            })
        };
        let t2 = t0.elapsed();
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            for (k, _) in &nodes {
                eprintln!("node {k:?}");
            }
        }
        // (one append of every node, in order)
        let ids = self.eng.push_many(unit, self.ops.exec, args);
        let out: Vec<(u64, NodeId, Src)> = nodes
            .iter()
            .zip(ids)
            .map(|((k, n), id)| (*k, id, Src::of(n)))
            .collect();
        let t3 = t0.elapsed();
        for &(_, id, _) in &out {
            declare_marks_before(&mut self.eng, id);
        }
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "push_text: slices {t1:?}, push {:?} (arguments {:?}, engine push {:?}, loop-back declarations {:?}), {} nodes",
                t0.elapsed().checked_sub(t1).unwrap(),
                t2.checked_sub(t1).unwrap(),
                t3.checked_sub(t2).unwrap(),
                t0.elapsed().checked_sub(t3).unwrap(),
                out.len()
            );
        }
        out
    }
}

/// Whether a job-written file's read-back only defines (its lines are
/// read as definitions per field, never as text): `.aux`.
/// The fields a LaTeX job's `.aux` read-back defines, guessed from the
/// text, in the order the file's lines define them: `\r@<x>` for each
/// `\label{x}` and `\b@<k>` for each `\bibitem{k}` (their `\newlabel`
/// and `\bibcite` lines, in text order), then the kernel's
/// `\@abspage@last` (the line `\enddocument` writes). Nothing without
/// `\documentclass`: another job's `.aux` is its own.
#[must_use]
pub fn guess_wkeys(text: &str) -> Vec<String> {
    if !text.contains("\\documentclass") {
        return Vec::new();
    }
    let mut keys: Vec<String> = Vec::new();
    let mut add = |k: String| {
        if !keys.contains(&k) {
            keys.push(k);
        }
    };
    let mut rest = text;
    while let Some(i) = rest.find('\\') {
        rest = &rest[i + 1..];
        let (prefix, after) = if let Some(a) = rest.strip_prefix("label{") {
            ("r@", a)
        } else if let Some(a) = rest.strip_prefix("bibitem") {
            // `\bibitem[label]{key}`
            let a = a.trim_start();
            let a = if a.starts_with('[') {
                a.find(']').map_or(a, |j| &a[j + 1..])
            } else {
                a
            };
            match a.trim_start().strip_prefix('{') {
                Some(a) => ("b@", a),
                None => continue,
            }
        } else {
            continue;
        };
        if let Some(j) = after.find('}') {
            let key = &after[..j];
            if !key.is_empty() && !key.contains(['\\', '{', '#', '%']) {
                add(format!("cs:{prefix}{key}"));
            }
        }
    }
    add("cs:@abspage@last".to_owned());
    keys
}

fn defines_only(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("aux"))
}

/// The extensions of the files a job is taken to write and read back
/// (`\jobname.<ext>`), declared before its first evaluation.
const WRITTEN_EXTS: [&str; 8] = ["aux", "toc", "lof", "lot", "out", "idx", "nav", "snm"];

/// The hint of a format's node (`hints::Hint` encoded): unbounded.
const FORMAT_HINT: &str = "A\x1eformat: the whole text, run once\x1e";

/// Whether the job's text is a document's (paragraph regions): always
/// after a format; without one, unless it reads as a format's source.
fn is_doc(prelude: Option<&str>, text: &str) -> bool {
    prelude.is_some()
        || crate::hints::kind_of(text, pass::FileKind::Document) == pass::FileKind::Document
}

/// The name a document's own text is registered under for the hints
/// of nodes made from parts of it (`hints::register_text`).
const DOC_NAME: &str = "\u{10FFFE}document";

/// Registers the document's analysis for the hints of nodes made from
/// parts of its slices (`hints::register_text`, by the document's
/// offsets): when one chunk is the whole document. A document of several
/// chunks has no whole analysis; such a node's hint answers `Any`.
fn register_doc(v: &Values, inc: &Incr) {
    if let [c] = inc.chunks.as_slice() {
        crate::hints::register_text(v.str(DOC_NAME), &c.th);
    }
}

/// A document node's text: a slice of the document (its bytes), or one
/// of an `\\input` file sliced in its place (`crate::inputs`).
enum DocNode {
    Doc(usize, usize),
    File(crate::inputs::Piece),
}

/// The document's nodes, keyed: per chunk of its units, its slices; an
/// `\\input` slice replaced by its file's ([`crate::inputs::pieces`],
/// for the chunks that have one). `read` gets the files sliced in.
fn doc_nodes(
    v: &Values,
    text: &str,
    inc: &Incr,
    finder: Option<&Finder>,
    read: &mut Vec<String>,
) -> Vec<(u64, DocNode)> {
    // (a text redefining `\\input` reads its files in its runs)
    let finder = finder.filter(|_| !inc.chunks.iter().any(|c| c.input.defines));
    let mut out = Vec::new();
    for c in &inc.chunks {
        let end = c.start + c.th.slices.last().map_or(0, |x| x.1);
        let slices =
            c.th.slices
                .iter()
                .zip(c.keys.iter())
                .map(|(&(s, e), &k)| (c.start + s, c.start + e, k));
        // (the chunk's keys, made when it was analysed; a slice at its
        // start not beginning a line keyed again)
        let doc_slice = |(s, e, k): (usize, usize, u64)| {
            let k = if s == c.start && s > 0 && text.as_bytes()[s - 1] != b'\n' {
                key(&text[s..e], false)
            } else {
                k
            };
            (k, DocNode::Doc(s, e))
        };
        let Some(f) = finder.filter(|_| c.input.names) else {
            out.extend(slices.map(doc_slice));
            continue;
        };
        let mut slices = slices;
        let chunk = &text[c.start..end];
        for p in crate::inputs::pieces(v, chunk, &c.th, inc.defs(), Some(f), read) {
            if p.is_file() {
                out.push((p.key, DocNode::File(p)));
                continue;
            }
            // (the document's own slice it is: the next whose key it has)
            for sl in slices.by_ref() {
                let (k, n) = doc_slice(sl);
                if k == p.key {
                    out.push((k, n));
                    break;
                }
            }
        }
    }
    out
}

/// A document node's arguments: its text with its origin, and its hint;
/// a format's (`incr` `None`): its text, and the format's hint.
fn node_args(
    v: &Values,
    text: &str,
    incr: Option<&Incr>,
    n: &DocNode,
    loads: Option<&crate::pkgrec::LoadHints>,
) -> Vec<ValueId> {
    match (n, incr) {
        (DocNode::File(p), _) => p.args(v, DOC_NAME),
        (&DocNode::Doc(s, e), Some(inc)) => {
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            let mut a = slice_args_in_file(v, &text[s..e], starts, v.str(DOC_NAME), s);
            // (a document node is a top-level node)
            a[1] = v.int(3);
            // (a package load: its record's hint, or the optimistic one,
            // `crate::pkgrec`)
            if let Some(h) = loads.and_then(|l| l.hint_for(v, &text[s..e])) {
                a.push(v.str(&h));
            } else {
                let (c, at) = inc.chunk_at(s);
                a.push(crate::hints::hint_arg(v, &c.th, at));
            }
            a
        }
        (&DocNode::Doc(s, e), None) => {
            let mut a = slice_args(v, &text[s..e], true);
            a.push(v.str(FORMAT_HINT));
            a
        }
    }
}

/// Where a text node's text is: the document's bytes, or a file's
/// (a slice of a file the document inputs).
#[derive(Clone)]
enum Src {
    Doc(usize, usize),
    File(Arc<str>, usize, usize),
}

impl Src {
    fn of(n: &DocNode) -> Src {
        match n {
            &DocNode::Doc(s, e) => Src::Doc(s, e),
            DocNode::File(p) => {
                let (t, s, e) = p.span();
                Src::File(t, s, e)
            }
        }
    }

    /// Its text, the document's being `doc`.
    fn text<'a>(&'a self, doc: &'a str) -> &'a str {
        match self {
            &Src::Doc(s, e) => doc.get(s..e.min(doc.len())).unwrap_or(""),
            Src::File(t, s, e) => t.get(*s..*e).unwrap_or(""),
        }
    }
}

/// A node's identity for edits: the hash of its text and whether it
/// begins a line.
pub(crate) fn key(piece: &str, starts: bool) -> u64 {
    phitex_defuse::hash_bytes(1 + u64::from(starts), piece.as_bytes())
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
/// The names a text loads (`\\input name`, `\\input{name}`,
/// `\\RequirePackage{a,b}`, `\\LoadClass{c}`, `\\documentclass{c}`),
/// each with the extensions to try.
fn loaded_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (cmd, ext) in [
        ("\\input", ""),
        ("\\RequirePackage", ".sty"),
        ("\\usepackage", ".sty"),
        ("\\LoadClass", ".cls"),
        ("\\documentclass", ".cls"),
    ] {
        let mut from = 0;
        while let Some(k) = text[from..].find(cmd) {
            let at = from + k + cmd.len();
            from = at;
            let rest = &text[at..];
            // (a longer name, `\\inputlineno`)
            if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            // (the line is not a comment)
            let line = &text[text[..at].rfind('\n').map_or(0, |x| x + 1)..at];
            if line.contains('%') {
                continue;
            }
            let mut r = rest.trim_start();
            if r.starts_with('[') {
                r = r[r.find(']').map_or(r.len(), |x| x + 1)..].trim_start();
            }
            let names: Vec<&str> = if let Some(b) = r.strip_prefix('{') {
                b[..b.find('}').unwrap_or(0)].split(',').collect()
            } else {
                vec![
                    r.split(|c: char| c.is_whitespace() || c == '\\' || c == '}')
                        .next()
                        .unwrap_or(""),
                ]
            };
            for n in names {
                let n = n.trim();
                if n.is_empty() || n.contains('#') {
                    continue;
                }
                out.push(if n.contains('.') {
                    n.to_owned()
                } else {
                    format!("{n}{}", if ext.is_empty() { ".tex" } else { ext })
                });
            }
        }
    }
    out
}

/// Harvests the files `text` loads that the finder resolves, then
/// theirs, to depth 8, each file once; when the prelude's reads are
/// known (`read`), only those it read (a name in a body never run,
/// `\\documentstyle`'s `latex209.def`, is not the format's).
#[allow(clippy::too_many_arguments)]
fn harvest_inputs(
    text: &str,
    depth: u32,
    finder: &Finder,
    read: &[String],
    defs: &mut pass::Definitions,
    order: &mut Vec<u32>,
    seen: &mut phitex_defuse::FxHashSet<String>,
    next_id: &mut u32,
) {
    if depth >= 8 {
        return;
    }
    for name in loaded_names(text) {
        if !read.is_empty()
            && !read
                .iter()
                .any(|n| *n == name || n.ends_with(&format!("/{name}")))
        {
            continue;
        }
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some((_, bytes)) = finder(&name) else {
            continue;
        };
        let t: String = bytes.iter().map(|&b| char::from(b)).collect();
        let id = *next_id;
        *next_id += 1;
        let kind = crate::hints::kind_of(&t, pass::FileKind::from_path(&name));
        // (what it loads comes first: its definitions precede the rest)
        harvest_inputs(&t, depth + 1, finder, read, defs, order, seen, next_id);
        defs.harvest(&t, kind, id);
        order.push(id);
    }
}

fn prelude_defs(c: &Cached, prelude: Option<&str>, finder: &Finder) -> Defs {
    static C: OnceLock<Mutex<FxHashMap<[u8; 32], Defs>>> = OnceLock::new();
    let cache = C.get_or_init(|| Mutex::new(FxHashMap::default()));
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
        // (the files the prelude reads, transitively: `\\input tex.ini`
        // reads plain.tex; depth 8, each file once)
        let mut seen = phitex_defuse::FxHashSet::default();
        let mut next_id = 1_000_000u32;
        let read: Vec<String> = c
            .reads
            .iter()
            .filter(|(_, h)| *h != 0)
            .map(|(n, _)| n.clone())
            .collect();
        harvest_inputs(
            p,
            0,
            finder,
            &read,
            &mut defs,
            &mut order,
            &mut seen,
            &mut next_id,
        );
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

/// The cached prelude of text key `key` still good under the finder, or
/// the files stale entries read (to supply up front).
impl Options {
    /// The finder the prelude reads its files with.
    fn prelude_finder(&self) -> &Finder {
        self.prelude_finder.as_ref().unwrap_or(&self.finder)
    }
}

fn cached_prelude(key: u64, opts: &Options) -> Result<Arc<Cached>, Vec<String>> {
    let finder = opts.prelude_finder();
    let candidates: Vec<Arc<Cached>> = env_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let t0 = std::time::Instant::now();
    let given = opts
        .prelude_bytes
        .as_deref()
        .and_then(deserialise)
        .filter(|c| c.text_key == key)
        .map(Arc::new);
    if crate::exec::env_flag!("PHITEX_TEX_TIME") && opts.prelude_bytes.is_some() {
        eprintln!(
            "format deserialise: {:?} ({} bytes, {})",
            t0.elapsed(),
            opts.prelude_bytes.as_ref().map_or(0, Vec::len),
            if given.is_some() {
                "accepted"
            } else {
                "rejected"
            }
        );
    }
    let mut known: Vec<String> = Vec::new();
    for c in given.into_iter().chain(candidates) {
        let t1 = std::time::Instant::now();
        let fresh = c
            .reads
            .iter()
            .all(|(n, h)| file_hash(finder(n).as_ref()) == *h);
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "format check: {} files found and hashed in {:?} ({})",
                c.reads.len(),
                t1.elapsed(),
                if fresh { "fresh" } else { "stale" }
            );
        }
        if fresh {
            return Ok(c);
        }
        // (a stale cache entry still names the files the prelude reads)
        for (n, _) in &c.reads {
            if !known.contains(n) {
                known.push(n.clone());
            }
        }
    }
    Err(known)
}

/// INITEX's state, and the prelude's on top of it: a format, `\dump`
/// ending it (or its text's end). Cached by the prelude's text, and good
/// while every file it read is what the finder gives now.
// (its phases timed and measured inline, PHITEX_TEX_TIME)
#[allow(clippy::too_many_lines)]
fn prelude_env(prelude: Option<&str>, opts: &Options) -> Arc<Cached> {
    let key = prelude.map_or(0, |p| {
        let mut b = p.as_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(opts.prelude_jobname.as_bytes());
        phitex_defuse::hash_bytes(7, &b)
    });
    let t_start = std::time::Instant::now();
    let time = crate::exec::env_flag!("PHITEX_TEX_TIME");
    let known = match cached_prelude(key, opts) {
        Ok(c) => {
            if time {
                eprintln!(
                    "prelude: cached ({:?} to find and check it)",
                    t_start.elapsed()
                );
            }
            return c;
        }
        Err(known) => known,
    };
    let t_check = t_start.elapsed();
    let m_check = mem_mb();
    let finder = opts.prelude_finder();
    let mut g = Graph::new(opts.time_limit, opts.threads);
    g.eng.set_max_rounds(opts.max_rounds);
    g.sched = opts.speculation.scheduler(opts.speculation_budget);
    let unit = g.eng.new_unit();
    // the files a previous build of this prelude read, supplied before
    // the first evaluation: no run stops on them, no round per file
    for n in &known {
        g.supply(finder, n);
    }
    if !known.is_empty() {
        g.absorb_provided();
        let env = g.environment(false);
        g.eng.set_environment(env);
        g.eng.set_environment_absent(g.absent.clone());
    }
    let t_supply = t_start.elapsed();
    let m_supply = mem_mb();
    let mut m_settle = m_supply;
    let mut t_settle = t_supply;
    let mut diags = Vec::new();
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
        g.push_text(unit, &as_file(p), None, None);
        // the point the prelude's final bindings are read at
        let end = g.eng.push(unit, g.mark, Vec::new());
        let mut stats = Stats::default();
        let t0 = std::time::Instant::now();
        g.settle(unit, finder, &mut stats, false, &mut phitex_defuse::NoEdits);
        if time {
            eprintln!("prelude settle {:?}", t0.elapsed());
        }
        diags = prelude_diagnostics(&g, unit, &as_file(p), finder);
        t_settle = t_start.elapsed();
        m_settle = mem_mb();
        env = final_bindings(&g, unit, end, env);
    }
    let t_bindings = t_start.elapsed();
    let m_bindings = mem_mb();
    let v = g.eng.values();
    let out: Vec<(String, Tree)> = env.into_iter().map(|(k, x)| (k, export(v, x))).collect();
    let e = Arc::new(out);
    if time {
        eprintln!(
            "prelude: freshness check {t_check:?}, supply known files {:?}, run (settle) {:?}, final bindings {:?}, export {:?}; {} bindings",
            t_supply.checked_sub(t_check).unwrap_or_default(),
            t_settle.checked_sub(t_supply).unwrap_or_default(),
            t_bindings.checked_sub(t_settle).unwrap_or_default(),
            t_start
                .elapsed()
                .checked_sub(t_bindings)
                .unwrap_or_default(),
            e.len()
        );
        let m_export = mem_mb();
        eprintln!(
            "prelude memory (MB resident/peak): before {m_check:?}, after supply {m_supply:?}, after the run {m_settle:?}, after final bindings {m_bindings:?}, after export {m_export:?}"
        );
    }
    let c = Arc::new(Cached {
        key: prelude_key_of(key, &g.reads),
        text_key: key,
        reads: g.reads.clone(),
        env: e,
        errors: g.eval_errors.clone(),
        diags,
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

/// The name a diagnostic in the prelude's own text carries (its
/// `\\input`s carry their files' names).
pub const PRELUDE_NAME: &str = "(format)";

/// TeX's errors in the prelude (`text`, its own text) at their places:
/// the file and line, TeX's context (§318), as [`Build::diagnostics`]
/// places a document's.
fn prelude_diagnostics(g: &Graph, unit: UnitId, text: &str, finder: &Finder) -> Vec<Diagnostic> {
    let v = g.eng.values();
    let entries = g
        .eng
        .acc_value(unit, v.field("diags"))
        .map(|s| v.seq_to_vec(s))
        .unwrap_or_default();
    let mut out = Vec::new();
    for e in entries {
        let xs = list_of(v, e);
        let int = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
        if int(0) == 0 {
            continue;
        }
        let message = xs.get(1).map(|&x| str_of(v, x)).unwrap_or_default();
        let loc = usize::try_from(int(6)).unwrap_or(0);
        let name = xs
            .get(2)
            .filter(|&&n| n != ValueId::UNIT)
            .map(|&n| str_of(v, n));
        // (the file's characters are its bytes; the prelude's own text
        // is its characters)
        let (file, chars, at): (String, Vec<char>, usize) = match name {
            Some(n) if n != DOC_NAME && int(3) >= 0 => {
                let bytes = finder(n.trim_start_matches("./"))
                    .map(|(_, b)| b)
                    .unwrap_or_default();
                let at = usize::try_from(int(3)).unwrap_or(0) + loc;
                (n, bytes.iter().map(|&b| char::from(b)).collect(), at)
            }
            _ => {
                let chars: Vec<char> = text.chars().collect();
                let at = if int(3) >= 0 {
                    let b = usize::try_from(int(3)).unwrap_or(0);
                    text.get(..b.min(text.len()))
                        .map_or(0, |t| t.chars().count())
                        + loc
                } else {
                    0
                };
                (PRELUDE_NAME.to_owned(), chars, at)
            }
        };
        let ci = at.min(chars.len());
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
        let start = if message == "Undefined control sequence" {
            before
                .rfind('\\')
                .map_or(ci, |k| ls + before[..k].chars().count())
        } else {
            ci
        };
        let lead = format!("l.{line} ");
        let context = format!(
            "{lead}{before}\n{}{after}",
            " ".repeat(lead.len() + before.chars().count())
        );
        out.push(Diagnostic {
            file,
            range: (start, ci),
            line,
            severity: Severity::Error,
            message,
            context,
            help: None,
        });
    }
    out
}

/// The prelude's bindings at its end (`end`, the mark after its text):
/// the INITEX ones `env`, each field it defines bound to its final
/// value instead (open files and the PDF writer's state are not
/// dumped).
fn final_bindings(
    g: &Graph,
    unit: UnitId,
    end: NodeId,
    mut env: Vec<(String, ValueId)>,
) -> Vec<(String, ValueId)> {
    let v = g.eng.values();
    let defs_f = v.field("defs");
    let mut seen = phitex_defuse::FxHashSet::default();
    // (each name's last binding replaces its earlier one, which keeps
    // its place empty: the order is the one remove-then-push gives)
    let mut at: phitex_defuse::FxHashMap<String, usize> = env
        .iter()
        .enumerate()
        .map(|(k, (n, _))| (n.clone(), k))
        .collect();
    let mut slots: Vec<Option<(String, ValueId)>> = env.drain(..).map(Some).collect();
    let t0 = std::time::Instant::now();
    let z = std::time::Duration::ZERO;
    let (mut t_name, mut t_reach, mut t_value) = (z, z, z);
    let mut raw = 0usize;
    let acc = g.eng.acc_value(unit, defs_f).map(|d| v.seq_to_vec(d));
    let t_acc = t0.elapsed();
    if let Some(ls) = acc {
        for l in ls {
            for n in list_of(v, l) {
                raw += 1;
                if !seen.insert(n) {
                    continue;
                }
                let f = Field(n);
                let t = std::time::Instant::now();
                let name = field_name(v, f);
                t_name += t.elapsed();
                // open files are not dumped
                // nor is the PDF writer's state (pdfTeX dumps no objects)
                if ["wopen:", "write:", "file:", "read:", "pdf:state"]
                    .iter()
                    .any(|p| name.starts_with(p))
                {
                    continue;
                }
                let t = std::time::Instant::now();
                let d = g.eng.reaching_def(end, f);
                t_reach += t.elapsed();
                let t = std::time::Instant::now();
                let x = d.and_then(|d| g.eng.def_value(d, f));
                t_value += t.elapsed();
                if let Some(x) = x {
                    if let Some(k) = at.insert(name.clone(), slots.len()) {
                        slots[k] = None;
                    }
                    slots.push(Some((name, x)));
                }
            }
        }
    }
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        eprintln!(
            "final bindings: defs fold {t_acc:?}, {raw} entries, {} fields; names {t_name:?}, reaching_def {t_reach:?}, def_value {t_value:?}, total {:?}",
            seen.len(),
            t0.elapsed()
        );
    }
    slots.into_iter().flatten().collect()
}

/// The process's resident and peak resident memory now, in MB
/// (`PHITEX_TEX_TIME`'s phase lines), from `/proc/self/status` (zeros
/// elsewhere).
pub(crate) fn mem_mb() -> (u64, u64) {
    let t = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |k: &str| {
        t.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            / 1024
    };
    (kb("VmRSS:"), kb("VmHWM:"))
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
    nodes: Vec<(u64, NodeId, Src)>,
    dvi: NodeId,
    /// The first node after the text's (the page plan, which folds the
    /// list the text's nodes make): new text nodes go before it, also
    /// when the text has none left.
    end: NodeId,
    prelude: Arc<Cached>,
    /// What the last build or edit ran.
    pub stats: Stats,
    /// Where new texts come from ([`Options::edits`]).
    edits: EditSender,
    /// The pages' hashes the callback last saw.
    settled: Vec<u64>,
    /// Whether the job's text is a document's ([`is_doc`]).
    doc: bool,
    /// The document's text last applied (`edit_file` applies it again
    /// when a file sliced into it changed).
    text: String,
    /// The document's units and their analyses ([`Incr`]).
    incr: Incr,
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
    #[allow(clippy::too_many_lines)]
    pub fn with(opts: Options, prelude: Option<&str>, document: &str) -> Build {
        let t0 = std::time::Instant::now();
        let cached = prelude_env(prelude, &opts);
        let t_env = t0.elapsed();
        let env = cached.env.clone();
        let defs = prelude_defs(&cached, prelude, opts.prelude_finder());
        let t1 = t0.elapsed();
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: prelude env (format load or build) {t_env:?}, prelude definitions {:?}",
                t1.checked_sub(t_env).unwrap_or_default()
            );
        }
        let mut g = Graph::new(opts.time_limit, opts.threads);
        g.eng.set_max_rounds(opts.max_rounds);
        (*g.ops.out_ok.lock().unwrap_or_else(PoisonError::into_inner))
            .clone_from(&opts.out_name_ok);
        g.sched = opts.speculation.scheduler(opts.speculation_budget);
        if let Some(p) = &opts.progress {
            g.watch = crate::exec::Watch::new(g.threads, Some(p.clone()));
        }
        let unit = g.eng.new_unit();
        // the files the format read (fonts, data) are supplied before the
        // document's first evaluation: a run reading one never stops
        let t_sup = std::time::Instant::now();
        for (n, _) in cached.reads.clone() {
            g.supply(&opts.finder, &n);
        }
        g.guesses(&opts, document);
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: supply the format's {} files (fonts, data) {:?}",
                cached.reads.len(),
                t_sup.elapsed()
            );
        }
        // the files a LaTeX job writes and reads back: in-memory values
        // from the start (`.bbl` and `.ind` stay files until their
        // BibTeX and makeindex nodes publish them)
        for ext in WRITTEN_EXTS {
            g.written_file(
                &format!("{}.{ext}", opts.jobname),
                opts.fixpoint,
                &opts.finder,
            );
        }
        {
            g.absorb_provided();
            let env = g.environment(opts.fixpoint);
            g.eng.set_environment(env);
            g.eng.set_environment_absent(g.absent.clone());
        }
        job_env(&mut g, unit, &env, &opts);
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
        *g.ops.defs.lock().unwrap_or_else(PoisonError::into_inner) = defs.clone();
        let doc = is_doc(prelude, document);
        let inputs = g.input_finder(&opts.finder);
        // the package loads' records (`crate::pkgrec`); a load never seen
        // runs optimistically under speculation
        let optimistic = opts.speculation != Speculation::Off
            && std::env::var("PHITEX_PKG_OPTIMISTIC").map_or(true, |x| x != "0");
        let predict = opts.pkg_predict || crate::exec::env_flag!("PHITEX_PKG_PREDICT");
        g.loads = Some(Arc::new(crate::pkgrec::LoadHints::new(
            crate::pkgrec::parse(&opts.pkgrec),
            inputs.clone(),
            optimistic,
            predict,
        )));
        // (the loads' predictions, asked by the engine per load node)
        g.ops
            .pkg_loads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&g.loads);
        let (nodes, incr) = g.push_document(unit, document, defs, doc, Some(&inputs));
        // (the files sliced in are read: listed, and watched)
        for n in g.inputs_read.clone() {
            g.supply(&opts.finder, &n);
        }
        // the page builder's fold, after the text: one node choosing every
        // page break, its children the pages
        let end = g.eng.push(unit, g.ops.pageplan, Vec::new());
        page_guesses(&mut g, &env);
        chain_guesses(&mut g, &opts.pgst);
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
            end,
            prelude: cached,
            incr,
            stats: Stats::default(),
            edits,
            settled: Vec::new(),
            doc,
            text: document.to_owned(),
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

    /// `PHITEX_CLAIMS=<text>`: the debugging dump after a settle.
    fn dump_claims(&self) {
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
            for (_, n, src) in &self.nodes {
                if src.text(&self.text).contains(&want) {
                    dump(eng, v, *n, 0);
                }
            }
        }
    }

    fn settle(&mut self) {
        let t_all = std::time::Instant::now();
        // the text nodes' offsets in the document, for the progress
        let mut offsets = FxHashMap::default();
        let mut off = 0u64;
        for (_, n, src) in &self.nodes {
            offsets.insert(n.raw(), off);
            off += u64::try_from(src.text(&self.text).len()).unwrap_or(0);
        }
        self.g.watch.offsets = Arc::new(offsets);
        let mut stats = Stats::default();
        self.g.take_steps_done();
        self.g.take_refuted();
        self.g.queries.clear();
        // (the files sliced into the document's regions were read)
        let sliced: Vec<(String, bool)> = self
            .g
            .inputs_read
            .iter()
            .map(|n| (n.clone(), true))
            .collect();
        self.g.queries.extend(sliced);
        self.g
            .ops
            .pkg_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let loads = self.g.loads.clone();
        let mut absorb = Absorb {
            edits: &self.edits,
            nodes: &mut self.nodes,
            exec: self.g.ops.exec,
            end: self.end,
            incr: self.doc.then_some(&mut self.incr),
            unchanged: None,
            finder: self.g.input_finder(&self.opts.finder),
            text: &mut self.text,
            loads: loads.as_deref(),
        };
        let t_before = t_all.elapsed();
        // a text sent before the evaluation starts is the one it is for
        if let Some(v) = phitex_defuse::EditSource::absorb(&mut absorb, &mut self.g.eng) {
            self.g.eng.set_version(v);
        }
        let t_apply = t_all.elapsed();
        self.g.settle(
            self.unit,
            &self.opts.finder,
            &mut stats,
            self.opts.fixpoint,
            &mut absorb,
        );
        // (a text taken last that changed nothing: the result is for it)
        let t_settle = t_all.elapsed();
        if let Some(v) = absorb.unchanged {
            stats.version = stats.version.max(v);
        }
        stats.live = self.g.eng.live_nodes();
        stats.page_steps_completed = self.g.take_steps_done();
        stats.refuted_guesses = self.g.take_refuted();
        self.pkg_stats(&mut stats);
        // the loads' records as this evaluation left them: a load
        // inserted by a later edit uses them
        if let Some(l) = &self.g.loads {
            l.merge(self.fresh_records());
        }
        // the chain's last values become its round-one guesses: an edit
        // whose rounds restart begins from them, not from the cold guess
        let kept = self.export_pgst();
        chain_guesses(&mut self.g, &kept);
        self.stats = stats;
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "edit phases: before the apply (offsets, inputs) {t_before:?}, apply (intake, hint pass, slice-key diff, engine replace/insert/remove: `edit apply`) {:?}, settle (`settle phases`) {:?}, after (stats, guesses kept) {:?}",
                t_apply.saturating_sub(t_before),
                t_settle.saturating_sub(t_apply),
                t_all.elapsed().saturating_sub(t_settle)
            );
            let (nodes, units, values) = self.sizes();
            let (node, use_) = Engine::record_sizes();
            let rss = std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|t| {
                    t.lines()
                        .find_map(|l| l.strip_prefix("VmRSS:"))
                        .map(|v| v.trim().to_owned())
                })
                .unwrap_or_default();
            eprintln!(
                "sizes: {nodes} live nodes ({node} B), {units} live units, {values} values (use {use_} B), rss {rss}"
            );
        }
        self.dump_claims();
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

    /// Why the exact PDF writer folded the pages in order (`pdf:state`
    /// threaded through the page steps) instead of writing each on its
    /// own, if it did.
    #[must_use]
    pub fn pdf_fold_reason(&self) -> Option<&str> {
        self.g.pdf_fold.as_deref()
    }

    /// Frees the engine's memoized applications nothing reaches any
    /// more (a long session calls it between edits); returns the nodes
    /// freed.
    pub fn collect_garbage(&mut self) -> usize {
        self.g.eng.collect_garbage()
    }

    /// The engine's live nodes, live units and values in its table.
    #[must_use]
    pub fn sizes(&self) -> (usize, usize, usize) {
        (
            self.g.eng.live_nodes(),
            self.g.eng.live_units(),
            self.g.eng.values().len(),
        )
    }

    /// The document changed to `text`: the nodes whose slices differ are
    /// replaced, inserted or removed, and the graph re-evaluated.
    pub fn edit(&mut self, text: &str) {
        let t0 = std::time::Instant::now();
        self.edits.send(text);
        self.settle();
        if crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!("time: edit {:?}", t0.elapsed());
        }
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
        // a file sliced into the document's regions (`crate::inputs`):
        // the document is applied again with the new text, so only the
        // regions whose slices changed are replaced
        let sliced = self.g.inputs_read.iter().any(|n| n == name);
        {
            let mut o = self
                .g
                .overrides
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match (&found, text) {
                (Some(x), Some(_)) => {
                    o.insert(name.to_owned(), x.clone());
                }
                _ => {
                    o.remove(name);
                }
            }
        }
        if sliced {
            let t = self.text.clone();
            self.edits.send(&t);
        }
        // a package load whose record lists the file: its key no longer
        // matches, so its hint goes back to the unrecorded one
        // (`crate::pkgrec`)
        if let Some(l) = self.g.loads.clone() {
            l.forget(name);
            let text = as_file(&self.text);
            let incr = self.doc.then_some(&self.incr);
            let redo: Vec<(NodeId, Vec<ValueId>)> = self
                .nodes
                .iter()
                .filter_map(|(_, n, src)| match *src {
                    Src::Doc(s, e) if l.reads_file(src.text(&text), name) => Some((
                        *n,
                        node_args(
                            self.g.eng.values(),
                            &text,
                            incr,
                            &DocNode::Doc(s, e),
                            Some(&l),
                        ),
                    )),
                    _ => None,
                })
                .collect();
            for (n, args) in redo {
                self.g.eng.replace(n, self.g.ops.exec, args);
                declare_marks_before(&mut self.g.eng, n);
            }
        }
        self.g.absorb_provided();
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
        self.prelude.errors.is_empty() && self.prelude.diags.is_empty()
    }

    /// TeX's errors in the prelude, at their places in its files (a
    /// format that stopped on them): every build on it reports them.
    #[must_use]
    pub fn prelude_diagnostics(&self) -> &[Diagnostic] {
        &self.prelude.diags
    }

    /// The prelude's environment, serialised, for
    /// [`Options::prelude_bytes`]; `None` when its evaluation did not
    /// finish ([`Self::prelude_complete`]): a part of a format is never
    /// stored.
    #[must_use]
    pub fn export_prelude(&self) -> Option<Vec<u8>> {
        self.prelude_complete().then(|| serialise(&self.prelude))
    }

    /// The package loads' records ([`Options::pkgrec`]): those the build
    /// was given, each load this build ran replacing its own
    /// (`crate::pkgrec`). Kept machine-wide, beside the format.
    #[must_use]
    pub fn export_pkgrec(&self) -> Vec<u8> {
        match &self.g.loads {
            Some(l) => {
                l.merge(self.fresh_records());
                l.bytes()
            }
            None => self.opts.pkgrec.clone(),
        }
    }

    /// The predicted publications of the package load `text` (what the
    /// engine's hook is given, `crate::pkgrec::LoadHints::prediction`):
    /// each field, whether global, and its value rendered; `None` when
    /// not predicting or the load's key does not match.
    #[doc(hidden)]
    #[must_use]
    pub fn pkg_prediction(&self, text: &str) -> Option<Vec<(String, bool, String)>> {
        let v = self.g.eng.values();
        let p = self.g.loads.as_ref()?.prediction(v, text)?;
        Some(p.into_iter().map(|(n, g, x)| (n, g, v.render(x))).collect())
    }

    /// The records the loads' last runs left (`pkgrecs`), by load key.
    fn fresh_records(&self) -> Vec<(String, crate::pkgrec::Rec)> {
        let v = self.g.eng.values();
        let Some(acc) = self.g.eng.acc_value(self.unit, v.field("pkgrecs")) else {
            return Vec::new();
        };
        let keys: FxHashMap<u64, String> = self
            .nodes
            .iter()
            .filter_map(|(_, n, src)| {
                Some((n.raw(), crate::pkgrec::load_key(src.text(&self.text))?))
            })
            .collect();
        v.seq_to_vec(acc)
            .into_iter()
            .filter_map(|e| {
                let (node, r) = crate::pkgrec::decode_entry(v, e)?;
                Some((keys.get(&node)?.clone(), r))
            })
            .collect()
    }

    /// The package loads' counters ([`Stats::pkg_loads`] and the rest).
    #[allow(clippy::too_many_lines)]
    fn pkg_stats(&self, stats: &mut Stats) {
        let loads: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, _, src)| crate::pkgrec::load_key(src.text(&self.text)).is_some())
            .map(|(_, n, _)| n.raw())
            .collect();
        stats.pkg_loads = loads.len() as u64;
        stats.pkg_claimed = self
            .nodes
            .iter()
            .filter(|(_, n, src)| {
                crate::pkgrec::load_key(src.text(&self.text)).is_some()
                    && self
                        .g
                        .eng
                        .node_info(*n)
                        .is_some_and(|i| i.state == phitex_defuse::NodeState::Inert)
            })
            .count() as u64;
        if crate::exec::env_flag!("PHITEX_PKG_TRACE") {
            let fresh = self.fresh_records();
            for (_, n, src) in &self.nodes {
                let Some(k) = crate::pkgrec::load_key(src.text(&self.text)) else {
                    continue;
                };
                let st = self.g.eng.node_info(*n).map(|i| i.state);
                let r = fresh.iter().find(|(f, _)| *f == k).map(|(_, r)| {
                    (
                        r.files
                            .iter()
                            .map(|(n, h)| format!("{n}:{h:x}"))
                            .collect::<Vec<_>>(),
                        r.writes.len(),
                        r.reads.len(),
                    )
                });
                eprintln!(
                    "pkgrec: load {k} node {} state {st:?}: fresh {r:?}",
                    n.raw()
                );
            }
            // per load: its runs (speculative ones), and the fields its
            // record reads whose definition reaching it is another load's
            let v = self.g.eng.values();
            let keyed: Vec<(NodeId, String)> = self
                .nodes
                .iter()
                .filter_map(|(_, n, src)| {
                    Some((*n, crate::pkgrec::load_key(src.text(&self.text))?))
                })
                .collect();
            let log = self
                .g
                .ops
                .pkg_log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let mut chain: FxHashMap<String, u64> = FxHashMap::default();
            for (n, k) in &keyed {
                let runs = log.iter().filter(|r| r.0 == n.raw()).count();
                let spec = log.iter().filter(|r| r.0 == n.raw() && r.3).count();
                let mut deps: Vec<String> = Vec::new();
                if let Some((_, r)) = fresh.iter().find(|(f, _)| f == k) {
                    for f in &r.reads {
                        if let Some(p) = self.g.eng.reaching_def(*n, v.field(f))
                            && let Some((_, pk)) = keyed.iter().find(|(m, _)| *m == p)
                        {
                            *chain.entry(f.clone()).or_default() += 1;
                            deps.push(format!("{f} <- {pk}"));
                        }
                    }
                }
                let nd = deps.len();
                eprintln!(
                    "pkgrec: runs of {k}: {runs} ({spec} speculative); reads {nd} fields another load defined: {deps:?}"
                );
            }
            // the fields that still chain loads, by the number of loads
            // reading one from another
            let mut chain: Vec<(String, u64)> = chain.into_iter().collect();
            chain.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            eprintln!(
                "pkgrec: chaining fields ({}): {:?}",
                chain.len(),
                chain.iter().take(40).collect::<Vec<_>>()
            );
            eprintln!(
                "pkgrec: refuted by field {:?}; restarted by {:?}; speculations {} confirmed {} mispredicted {}",
                stats.refuted_by_field.iter().take(20).collect::<Vec<_>>(),
                stats.restarted_by,
                stats.speculations,
                stats.confirmations,
                stats.mispredictions
            );
        }
        if let Some(l) = &self.g.loads {
            stats.pkg_recorded = l.given[0].load(std::sync::atomic::Ordering::Relaxed);
            stats.pkg_optimistic = l.given[1].load(std::sync::atomic::Ordering::Relaxed);
            // each prediction against what the load's last run published
            let predicted = l
                .predicted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if !predicted.is_empty() {
                let fresh: FxHashMap<String, crate::pkgrec::Rec> =
                    self.fresh_records().into_iter().collect();
                stats.pkg_predicted = predicted.len() as u64;
                stats.pkg_predict_wrong.clear();
                let mut keys: Vec<&String> = predicted.keys().collect();
                keys.sort();
                for k in keys {
                    let want = &predicted[k];
                    let Some(got) = fresh.get(k) else {
                        stats
                            .pkg_predict_wrong
                            .push(format!("{k}: (no record of its run)"));
                        continue;
                    };
                    let a: FxHashMap<&str, (bool, &[u8])> = want
                        .iter()
                        .map(|(n, g, b)| (n.as_str(), (*g, b.as_slice())))
                        .collect();
                    let b: FxHashMap<&str, (bool, &[u8])> = got
                        .values
                        .iter()
                        .map(|(n, g, b)| (n.as_str(), (*g, b.as_slice())))
                        .collect();
                    let mut wrong: Vec<&str> = a
                        .iter()
                        .filter(|(n, x)| b.get(*n) != Some(*x))
                        .map(|(n, _)| *n)
                        .chain(b.keys().filter(|n| !a.contains_key(*n)).copied())
                        .collect();
                    wrong.sort_unstable();
                    if wrong.is_empty() {
                        stats.pkg_predict_right += 1;
                    }
                    stats
                        .pkg_predict_wrong
                        .extend(wrong.into_iter().map(|f| format!("{k}: {f}")));
                }
            }
        }
        let log = self
            .g
            .ops
            .pkg_log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        stats.pkg_runs = log.len() as u64;
        // the most load runs at once, and the span from the first's start
        // to the last's end
        let mut ev: Vec<(std::time::Instant, i64)> = Vec::with_capacity(2 * log.len());
        for &(_, a, b, _) in &log {
            ev.push((a, 1));
            ev.push((b, -1));
        }
        ev.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.cmp(&y.1)));
        let (mut cur, mut max) = (0i64, 0i64);
        for &(_, d) in &ev {
            cur += d;
            max = max.max(cur);
        }
        stats.pkg_max_concurrent = usize::try_from(max).unwrap_or(0);
        stats.pkg_wall = match (ev.first(), ev.last()) {
            (Some(a), Some(b)) => b.0.duration_since(a.0),
            _ => std::time::Duration::ZERO,
        };
        // the loads on the critical chain (a walk of it: asked for)
        if !crate::exec::env_flag!("PHITEX_PKG_STATS") {
            return;
        }
        let on: phitex_defuse::FxHashSet<u64> = self
            .g
            .eng
            .critical_chain()
            .iter()
            .map(|(n, _)| n.raw())
            .collect();
        stats.pkg_on_critical = loads.iter().filter(|n| on.contains(n)).count() as u64;
    }

    /// The files the document's runs opened for reading, with a hash of
    /// the text each read, in the order read (the format's are in its
    /// key, [`Self::prelude_key`]).
    #[must_use]
    pub fn files_read(&self) -> Vec<(String, u64)> {
        let v = self.g.eng.values();
        self.g
            .eng
            .acc_value(self.unit, v.field("fread"))
            .map(|s| v.seq_to_vec(s))
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                (
                    str_of(v, crate::val::list_of(v, e)[0]),
                    crate::val::int_at(v, e, 1).cast_unsigned(),
                )
            })
            .collect()
    }

    /// The files the job wrote (`\openout`, `\write`), with their text,
    /// in the order first opened: what TeX leaves in them.
    #[must_use]
    pub fn files(&self) -> Vec<(String, String)> {
        let v = self.g.eng.values();
        let acc = |n: &str| self.g.eng.acc_value(self.unit, v.field(n));
        written_texts(v, acc("wplan"), acc("streams"))
    }

    /// The key sets of the read-back files that only define (`.aux`),
    /// as the last build published them: [`Options::wkeys`] for a later
    /// session of the same document.
    #[must_use]
    pub fn wkeys(&self) -> Vec<(String, Vec<String>)> {
        let v = self.g.eng.values();
        let acc = |n: &str| self.g.eng.acc_value(self.unit, v.field(n));
        written_defs(v, acc("wplan"), acc("streams"))
            .into_iter()
            .filter(|(n, _)| defines_only(n))
            .map(|(n, ds)| (n, ds.into_iter().map(|(f, _)| f).collect()))
            .collect()
    }

    /// What each page's output routine left for the next as the last
    /// build published it (`pagefx:<j>`), serialised: [`Options::pagefx`]
    /// for a later session of the same document.
    #[must_use]
    pub fn export_pagefx(&self) -> Vec<u8> {
        let v = self.g.eng.values();
        let mut last: Vec<(u64, ValueId)> = Vec::new();
        if let Some(s) = self.g.eng.acc_value(self.unit, v.field("pagefxs")) {
            for e in v.seq_to_vec(s) {
                let xs = list_of(v, e);
                let (Some(&j), Some(&x)) = (xs.first(), xs.get(1)) else {
                    continue;
                };
                let j = u64::try_from(crate::val::int_of(v, j)).unwrap_or(0);
                match last.iter_mut().find(|(k, _)| *k == j) {
                    Some(slot) => slot.1 = x,
                    None => last.push((j, x)),
                }
            }
        }
        let mut out = Vec::new();
        for (j, x) in last {
            out.extend_from_slice(&j.to_le_bytes());
            put_tree(&mut out, &export(v, x));
        }
        out
    }

    /// The page chain's loop-backs as the last build's writer folded
    /// them (`pgst:<j>`, `pgdef:<j>`), serialised: [`Options::pgst`] for
    /// a later session of the same document.
    #[must_use]
    pub fn export_pgst(&self) -> Vec<u8> {
        let v = self.g.eng.values();
        let mut last: Vec<((u64, u64), ValueId)> = Vec::new();
        if let Some(s) = self.g.eng.acc_value(self.unit, v.field("pgsts")) {
            for e in v.seq_to_vec(s) {
                let xs = list_of(v, e);
                let (Some(&k), Some(&j), Some(&x)) = (xs.first(), xs.get(1), xs.get(2)) else {
                    continue;
                };
                let n = |y| u64::try_from(crate::val::int_of(v, y)).unwrap_or(0);
                let key = (n(k), n(j));
                match last.iter_mut().find(|(q, _)| *q == key) {
                    Some(slot) => slot.1 = x,
                    None => last.push((key, x)),
                }
            }
        }
        let mut out = Vec::new();
        for ((k, j), x) in last {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&j.to_le_bytes());
            put_tree(&mut out, &export(v, x));
        }
        out
    }

    /// Whether the output is a PDF file (`\pdfoutput` at the first
    /// `\shipout`); else DVI.
    #[must_use]
    pub fn is_pdf(&self) -> bool {
        self.output().0.starts_with(b"%PDF")
    }

    /// Whether the job ended on a fatal error (§93, `history =
    /// fatal_error_stop`).
    #[must_use]
    pub fn fatal(&self) -> bool {
        let v = self.g.eng.values();
        self.g
            .eng
            .acc_value(self.unit, v.field("log"))
            .is_some_and(|e| {
                v.seq_to_vec(e).into_iter().any(|x| {
                    list_of(v, x)
                        .first()
                        .is_some_and(|&k| str_of(v, k) == "fatal")
                })
            })
    }

    /// Whether the job ended on a fatal error with `\pdfoutput` positive
    /// (pdfTeX then says no PDF file was produced, pages or none).
    fn fatal_pdf(&self) -> bool {
        let v = self.g.eng.values();
        self.g
            .eng
            .acc_value(self.unit, v.field("log"))
            .is_some_and(|e| {
                v.seq_to_vec(e).into_iter().any(|x| {
                    let xs = list_of(v, x);
                    xs.first().is_some_and(|&k| str_of(v, k) == "fatal")
                        && xs.get(1).is_some_and(|&t| str_of(v, t) == "pdf")
                })
            })
    }

    /// The output file (DVI or PDF) and the log.
    #[must_use]
    pub fn output(&self) -> (Vec<u8>, String) {
        let v = self.g.eng.values();
        let mut dvi: Vec<u8> = self
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
        // (pdfTeX §794: after a fatal error the PDF file is removed)
        let end = if self.fatal() && (dvi.starts_with(b"%PDF") || self.fatal_pdf()) {
            dvi.clear();
            "!  ==> Fatal error occurred, no output PDF file produced!".to_owned()
        } else if n == 0 {
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
            // TeX82's primitives live in the primitive table, not the
            // environment (INITEX's §226 `primitive` calls)
            names.extend(crate::prims::PRIMS.iter().map(|p| p.0.to_owned()));
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
        for (_, _, src) in &self.nodes {
            let piece = src.text(&self.text);
            spans.push(doc.len());
            doc.push_str(piece);
        }
        // every node under a node of the text, to that node
        let mut top: phitex_defuse::FxHashMap<u64, usize> = phitex_defuse::FxHashMap::default();
        for (i, (_, n, _)) in self.nodes.iter().enumerate() {
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
            let in_doc = xs.get(2).is_some_and(|&n| str_of(v, n) == DOC_NAME);
            let (file, text, at) = if int(3) >= 0 && in_doc {
                // a line of the document's own text: the position the
                // node's frame carries is the byte in the document
                let at = usize::try_from(int(3)).unwrap_or(0)
                    + seg.chars().take(loc).map(char::len_utf8).sum::<usize>();
                (docname.clone(), doc.clone(), at)
            } else if int(3) >= 0
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
        out.extend(self.prelude.diags.iter().cloned());
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
                    Cause::Woken { by, field, prefix } => format!(
                        "Woken by [{}] for {} (prefix {})",
                        d(Some(by)),
                        field.map_or_else(|| "?".to_owned(), |f| field_name(v, f)),
                        prefix.as_deref().unwrap_or("-")
                    ),
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

    /// Every document node's declaration (a measure): its text's start,
    /// what it may define (`Any`, or the kind and size of its bound),
    /// its hint's cause, and how many reads it declares.
    #[doc(hidden)]
    #[must_use]
    pub fn node_declarations(&self) -> Vec<(String, String, String, usize)> {
        let v = self.g.eng.values();
        self.nodes
            .iter()
            .filter_map(|(_, n, _)| {
                let i = self.g.eng.node_info(*n)?;
                if i.op != "exec" {
                    return None;
                }
                let text: String = i
                    .args
                    .first()
                    .map(|&x| v.render(x))
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect();
                let md = match crate::hints::may_define(v, &i.args) {
                    phitex_defuse::MayDefine::Any => "Any".to_owned(),
                    other => format!("{other:?}").chars().take(12).collect(),
                };
                let h = crate::hints::hint_of_args(v, &i.args);
                let reads = crate::hints::reads(v, &i.args).map_or(0, |r| r.len());
                Some((text, md, h.cause.clone(), reads))
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
                Cause::Woken { by, field, prefix } => out.push(format!(
                    "Woken for {} (prefix {})\n    waiter: {}\n    waited: {}",
                    field.map_or_else(|| "?".to_owned(), |f| field_name(v, f)),
                    prefix.as_deref().unwrap_or("-"),
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

    /// The critical chain's links whose reader and writer are both page
    /// steps (the target graph forbids any): reader, writer, the cause.
    #[must_use]
    pub fn page_step_links(&self) -> Vec<String> {
        use phitex_defuse::Cause;
        let eng = &self.g.eng;
        let v = eng.values();
        let step = |n: phitex_defuse::NodeId| {
            eng.node_info(n)
                .is_some_and(|i| i.op.starts_with("pagestep"))
        };
        let mut out = Vec::new();
        for (n, c) in eng.critical_chain() {
            let (w, f) = match c {
                Cause::Changed { field, site } => (site, Some(field)),
                Cause::Rebind { field, to, .. } => (to, Some(field)),
                Cause::Woken { by, field, .. } => (Some(by), field),
                Cause::Output { of } => (Some(of), None),
                _ => (None, None),
            };
            if let Some(w) = w
                && step(n)
                && step(w)
            {
                out.push(format!(
                    "[{}] <- [{}] {}",
                    describe(eng, n),
                    describe(eng, w),
                    f.map_or_else(String::new, |f| field_name(v, f))
                ));
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
                    Cause::Woken { field, .. } => (
                        "Woken",
                        field.map_or_else(String::new, |f| field_name(v, f)),
                    ),
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

/// The job's environment: the format's parameters, the job name, the
/// PDF options and the date (§241), set on `unit` before the first line.
fn job_env(g: &mut Graph, unit: UnitId, env: &Env, opts: &Options) {
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

/// The last session's page-chain loop-backs ([`Build::export_pgst`]):
/// `[kind, j, value]`, kind 0 `pgst:<j>`, 1 `pgdef:<j>`; declared after
/// the cold guesses, which they replace.
///
/// A page past the last one given guesses the last `pgst` (the state a
/// routine left settles: a first-page hook clears itself once).
fn chain_guesses(g: &mut Graph, bytes: &[u8]) {
    let mut rd = Rd(bytes);
    let mut last: Option<(usize, ValueId)> = None;
    while let (Some(k), Some(j), Some(t)) = (rd.u64(), rd.u64(), rd.tree()) {
        let Ok(j) = usize::try_from(j) else { continue };
        if j < crate::page::MAX_PAGES {
            let v = g.eng.values();
            let name = if k == 0 { "pgst" } else { "pgdef" };
            let f = v.field(&format!("{name}:{j}"));
            let x = import(v, &t);
            g.eng.declare_loopback(f, x);
            if k == 0 && last.is_none_or(|(l, _)| j > l) {
                last = Some((j, x));
            }
        }
    }
    if let Some((l, x)) = last {
        for j in l + 1..crate::page::MAX_PAGES {
            let f = g.eng.values().field(&format!("pgst:{j}"));
            g.eng.declare_loopback(f, x);
        }
    }
}

/// The graph's accumulators and loop-backs (their round-one values).
fn declare_fields(eng: &mut Engine) {
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
        "wplan",
        "fplan",
        "oplan",
        "fread",
        "pagefxs",
        "pgdeltas",
        "pgsts",
        "pkgrecs",
        crate::pdfrelaxed::PIECES,
        crate::pdfout::XPAGES,
    ] {
        let fld = eng.values().field(f);
        eng.declare_accumulator(fld, Arc::new(SeqMonoid));
    }
    let lines = eng.values().field("lines");
    eng.declare_accumulator(lines, Arc::new(LinesMonoid));
    // the fonts' names as the writer gives them where they are not the
    // engine's numbers (`pdfout::font_tags`), for `\pdffontname`
    let f = eng.values().field(crate::pdfout::FONT_TAGS);
    let none = eng.values().list(Vec::new());
    eng.declare_loopback(f, none);
    // what each page's output routine leaves for the next: a
    // loop-back the page builder's fold reads from the previous round
    for j in 0..crate::page::MAX_PAGES {
        let f = eng.values().field(&format!("pagefx:{j}"));
        let none = eng.values().list(Vec::new());
        eng.declare_loopback(f, none);
        // what `\count0..9` are after page j's step (the build's
        // guess in round one, `page_guesses`)
        let f = eng.values().field(&format!("pgdef:{j}"));
        eng.declare_loopback(f, ValueId::UNIT);
        // what the routines up to page j left changed, besides
        // `\count0..9` (the chain's cumulative delta, folded by the
        // writer; guess: none)
        let f = eng.values().field(&format!("pgst:{j}"));
        eng.declare_loopback(f, ValueId::UNIT);
        // whether page j's step shipped a page (guess: no)
        let f = eng.values().field(&crate::etex::shipped_field(j));
        let zero = eng.values().int(0);
        eng.declare_loopback(f, zero);
    }
    for f in crate::hints::COUNTERS {
        let fld = eng.values().field(f);
        eng.declare_accumulator(fld, Arc::new(SumMonoid));
    }
}

/// The cold guess of the page steps' loop-back `pgdef:<j>` (`\count0..9`
/// after page j's step), declared as its round-one value before the
/// first evaluation: `\count0` advanced by one per page (plain's and
/// LaTeX's routines), the others as the format left them. A right guess
/// costs nothing; a wrong one re-runs the steps after it.
fn page_guesses(g: &mut Graph, env: &Env) {
    let v = g.eng.values();
    let int = |k: &str| {
        env.iter()
            .find(|(n, _)| n == k)
            .map_or(0, |(_, t)| crate::val::int_of(v, import(v, t)))
    };
    let base: Vec<i64> = (0..10).map(|k| int(&format!("count:{k}"))).collect();
    let mut decl = Vec::new();
    for j in 0..crate::page::MAX_PAGES {
        let d = i64::try_from(j).unwrap_or(0) + 1;
        let mut cs: Vec<ValueId> = base.iter().map(|&c| v.int(c)).collect();
        cs[0] = v.int(if base[0] >= 0 {
            base[0] + d
        } else {
            base[0] - d
        });
        decl.push((v.field(&format!("pgdef:{j}")), v.list(cs)));
    }
    for (f, x) in decl {
        g.eng.declare_loopback(f, x);
    }
}

/// The `lines` accumulator: the lines of documents' pieces begun, by
/// file name, as `[[name, count], ...]` sorted by name
/// ([`crate::run::Run::doc_lines`]).
struct LinesMonoid;

impl phitex_defuse::Monoid for LinesMonoid {
    fn empty(&self, v: &Values) -> ValueId {
        v.list(Vec::new())
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        let (xs, ys) = (list_of(v, a), list_of(v, b));
        if xs.is_empty() {
            return b;
        }
        if ys.is_empty() {
            return a;
        }
        let mut m: std::collections::BTreeMap<String, (ValueId, i64)> =
            std::collections::BTreeMap::new();
        for e in xs.iter().chain(ys.iter()) {
            let p = list_of(v, *e);
            if let (Some(&n), Some(&k)) = (p.first(), p.get(1)) {
                let x = m.entry(str_of(v, n)).or_insert((n, 0));
                x.1 += int_of(v, k);
            }
        }
        v.list(
            m.into_values()
                .map(|(n, k)| v.list(vec![n, v.int(k)]))
                .collect(),
        )
    }
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
            diags: Vec::new(),
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
