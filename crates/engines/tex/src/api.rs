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

/// The searches a tool's support files are found by ([`Options::tool_finder`]):
/// kpathsea's formats for them, as `bibtex`, `biber` and `makeindex` ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolSearch {
    /// A bibliography style (kpathsea's `bst` format).
    BibStyle,
    /// A bibliography database (kpathsea's `bib` format).
    BibData,
    /// An index style (kpathsea's `ist` format).
    IndexStyle,
}

impl ToolSearch {
    /// The search's code in its environment field (`tools::support_field`).
    pub(crate) fn code(self) -> u8 {
        match self {
            ToolSearch::BibStyle => 0,
            ToolSearch::BibData => 1,
            ToolSearch::IndexStyle => 2,
        }
    }

    fn of_code(c: &str) -> Option<ToolSearch> {
        match c {
            "0" => Some(ToolSearch::BibStyle),
            "1" => Some(ToolSearch::BibData),
            "2" => Some(ToolSearch::IndexStyle),
            _ => None,
        }
    }
}

/// Finds a tool's support file by its search and the name the tool asks
/// for: the name to print and the bytes.
pub type ToolFinder = Arc<dyn Fn(ToolSearch, &str) -> Option<(String, Vec<u8>)> + Send + Sync>;

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
    /// earlier session (the last build's values); a file not listed has
    /// no guess. A right guess saves the loop-back's second round; a
    /// wrong one costs nothing more than no guess.
    pub wkeys: Vec<(String, Vec<String>)>,
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
    /// Whether a page's finisher is given the definitions the last
    /// finisher running its routine made as its prediction (on by
    /// default; off, the readers after it wait for its run: a test that
    /// the build's bytes do not hang on the predictions).
    pub finisher_predictions: bool,
    /// `\\pdfshellescape` (pdfTeX's `shellenabledp`): 0 off, 1 on,
    /// 2 restricted (TeX Live's `shell_escape = p`, the default). No
    /// command ever runs as a process: an allowed `makeindex` runs in
    /// process (`phitex-makeindex`); the others are refused with a
    /// diagnostic.
    pub shell_escape: i32,
    /// The loop-back rounds an evaluation may take, as latexmk's
    /// `max_repeat` bounds pdflatex's runs (5): a document whose files
    /// read back still change then is finished on the values its last
    /// round ran with, and a warning names the fields still moving
    /// ([`Build::diagnostics`]).
    pub max_rounds: u64,
    /// Where a tool's support files come from (bibliography styles and
    /// databases, index styles: [`ToolSearch`]); `None` asks
    /// [`Options::finder`] by name.
    pub tool_finder: Option<ToolFinder>,
    /// BibTeX's options and capacities (TeX Live's `texmf.cnf`:
    /// `max_strings`, `ent_str_size`, ...), for the BibTeX runs a read
    /// makes (`crate::tools`).
    pub bibtex: phitex_bibtex::Options,
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
    /// The engine's scheduler for `self` and `budget`.
    /// (debug, `observe` builds: `PHITEX_SCHED_SEED=n` runs the engine's
    /// random schedule with seed `n`, speculating: results must not
    /// depend on it)
    fn scheduler(self, budget: usize) -> Box<dyn phitex_defuse::Scheduler + Send> {
        use phitex_defuse::Speculation as S;
        if let Some(seed) =
            crate::exec::obs_str!("PHITEX_SCHED_SEED").and_then(|x| x.parse::<u64>().ok())
        {
            return Box::new(phitex_defuse::RandomScheduler::new(seed, true));
        }
        let mode = match self {
            Speculation::Off => S::Off,
            Speculation::Predicted => S::Predicted,
            Speculation::All => S::All,
        };
        // (the step limit of a guessing run)
        Box::new(phitex_defuse::Greedy {
            outstanding: 4096,
            fuel: 10_000,
            ..phitex_defuse::Greedy::speculative(mode, budget)
        })
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
    text: Option<(Arc<str>, Option<Hint>)>,
    version: u64,
}

/// What a sender knows of a text against the one it sent before: that
/// text, and the lengths of the prefix and suffix they share (a check
/// the engine then skips when that is the text it has).
pub type Hint = (Arc<str>, usize, usize);

impl EditSender {
    /// Sends the document's text: its version.
    #[allow(
        clippy::must_use_candidate,
        reason = "a send; the version is for whoever wants it"
    )]
    pub fn send(&self, text: &str) -> u64 {
        self.send_shared(Arc::from(text))
    }

    /// [`EditSender::send`], the text shared (not copied).
    #[allow(
        clippy::must_use_candidate,
        reason = "a send; the version is for whoever wants it"
    )]
    pub fn send_shared(&self, text: Arc<str>) -> u64 {
        self.send_edit(text, None)
    }

    /// [`EditSender::send_shared`] with what the sender knows of the
    /// change ([`Hint`]).
    pub fn send_edit(&self, text: Arc<str>, hint: Option<Hint>) -> u64 {
        let mut p = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        p.version += 1;
        // (a hint against a text the engine has not taken is no hint)
        let hint = if p.text.is_some() { None } else { hint };
        p.text = Some((text, hint));
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

    fn take(&self) -> Option<(Arc<str>, Option<Hint>, u64)> {
        let mut p = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let v = p.version;
        p.text.take().map(|(t, h)| (t, h, v))
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
    /// The text applied.
    text: &'a mut String,
    /// The text sent that `text` is the canonical text of (the next
    /// one's is made where they differ only, [`canonical_splice`]).
    raw: &'a mut Option<Arc<str>>,
    /// `text`'s line starts where known ([`Build::lines`]).
    lines: &'a Mutex<Option<Vec<usize>>>,
    /// Whether the nodes hold the document's canonical text
    /// ([`canonical_text`]; off for good once a text cannot be), and its
    /// comments by line, for the environment (`doc:marks`).
    canon: &'a mut bool,
    marks: Marks,
    /// The package loads' hints (`crate::pkgrec`).
    loads: Option<&'a crate::pkgrec::LoadHints>,
    /// The build's hint registry (`crate::hints::Registry`).
    hints: Arc<crate::hints::Registry>,
}

impl phitex_defuse::EditSource for Absorb<'_> {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        let (raw, hint, version) = self.edits.take()?;
        crate::timeline::lap("engine: the edit taken");
        // (the sender's prefix and suffix, where its last text is the one
        // the canonical text is of: what changed is looked at alone)
        let ends = hint
            .filter(|h| self.raw.as_ref().is_some_and(|r| Arc::ptr_eq(r, &h.0)))
            .map(|h| (h.1, h.2));
        let t0 = std::time::Instant::now();
        let raw_marks = match ends {
            Some((p, s)) => {
                let b = raw.as_bytes();
                let (a, z) = (
                    p.saturating_sub(1),
                    (b.len() - s.min(b.len()) + 1).min(b.len()),
                );
                let mid = &b[a.min(z)..z];
                mid.contains(&0) || mid.windows(2).any(|w| w == b"^^")
            }
            None => has_raw_marks(&raw),
        };
        crate::timeline::note("canonical: NUL and ^^ looked for", t0.elapsed());
        // (the canonical text made in place where the last raw text is
        // known: the range of it replaced)
        let mut range = None;
        let canon = if *self.canon && !raw_marks {
            match self.raw.as_deref() {
                Some(old) => {
                    let ms = self.marks.get();
                    let (ms, r) = canonical_splice(old, self.text, &ms, &raw, ends);
                    // (`PHITEX_CANON_CHECK`: against the whole text's)
                    if crate::exec::obs_flag!("PHITEX_CANON_CHECK") {
                        assert!(
                            canonical_text(&raw).as_ref() == Some(&(self.text.clone(), ms.clone())),
                            "canonical_splice differs from canonical_text"
                        );
                    }
                    range = Some(r);
                    // (the line starts: those in the range made again,
                    // those after it moved)
                    let mut ls = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(v) = ls.as_mut() {
                        let (a, b, c) = r;
                        let i = v.partition_point(|&k| k <= a);
                        let j = v.partition_point(|&k| k <= b);
                        let made: Vec<usize> = self.text.as_bytes()[a..c]
                            .iter()
                            .enumerate()
                            .filter(|x| *x.1 == b'\n')
                            .map(|x| a + x.0 + 1)
                            .collect();
                        let n = made.len();
                        v.splice(i..j, made);
                        for k in &mut v[i + n..] {
                            *k = *k + c - b;
                        }
                        if crate::exec::obs_flag!("PHITEX_CANON_CHECK") {
                            assert!(*v == line_starts(self.text), "the line starts moved wrong");
                        }
                    }
                    Some((None, ms))
                }
                None => canonical_text(&raw).map(|(t, ms)| (Some(t), ms)),
            }
        } else {
            None
        };
        if let Some((t, ms)) = canon {
            self.marks.set(ms);
            *self.raw = Some(raw);
            if let Some(t) = t {
                *self.text = t;
                *self.lines.lock().unwrap_or_else(PoisonError::into_inner) = None;
            }
        } else {
            *self.lines.lock().unwrap_or_else(PoisonError::into_inner) = None;
            *self.canon = false;
            self.marks.set(Vec::new());
            *self.raw = None;
            *self.text = raw.to_string();
        }
        crate::timeline::lap("engine: the canonical text (comments marked)");
        let changed = apply_text(
            &self.hints,
            eng,
            self.exec,
            self.end,
            self.nodes,
            self.text,
            range,
            self.incr.as_deref_mut(),
            self.loads,
        );
        if changed {
            self.unchanged = None;
            Some(version)
        } else {
            self.unchanged = Some(version);
            None
        }
    }
}

/// `PHITEX_TEX_TIME`: an edit's apply, its phases (`t`: slices, hints,
/// diff, as elapsed since `t_all`) and slice counts (`n`: old, new, kept,
/// replaced, removed, inserted, text bytes).
fn edit_apply_note(t_all: std::time::Instant, t: [std::time::Duration; 3], n: [usize; 7]) {
    if !crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
    reg: &crate::hints::Registry,
    v: &Values,
    text: &str,
    incr: Option<&Incr>,
) -> Vec<(u64, DocNode)> {
    match incr {
        Some(inc) => {
            register_doc(reg, v, inc);
            doc_nodes(text, inc)
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
    reg: &crate::hints::Registry,
    eng: &mut Engine,
    exec: OpId,
    end: NodeId,
    nodes_now: &mut Vec<(u64, NodeId, Src)>,
    text: &str,
    range: Option<(usize, usize, usize)>,
    mut incr: Option<&mut Incr>,
    loads: Option<&crate::pkgrec::LoadHints>,
) -> bool {
    let t_all = std::time::Instant::now();
    let text = as_file(text);
    crate::timeline::lap("engine: the text as a file");
    // (the units spliced, the chunks whose text or incoming definitions
    // changed analysed again; the range that changed where it is known
    // and the text is the file as it is)
    if let Some(inc) = incr.as_deref_mut() {
        match range.filter(|_| matches!(text, std::borrow::Cow::Borrowed(_))) {
            Some(r) => inc.set_text_edit(&text, r),
            None => inc.set_text(&text),
        }
    }
    let incr = incr.as_deref();
    let t_slices = t_all.elapsed();
    crate::timeline::lap("engine: the units spliced and walked");
    let new = slice_keys(reg, eng.values(), &text, incr);
    crate::timeline::lap("engine: the slices and their keys");
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
    crate::timeline::lap("engine: the slice-key diff");
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
            n
        } else {
            let args = args_of(eng, i);

            if let Some(&(_, p, _)) = nodes.last() {
                eng.insert_after(p, exec, args)
            } else {
                let first = old.get(a + old_mid).map_or(end, |x| x.1);
                eng.insert_before(first, exec, args)
            }
        };
        nodes.push((new[i].0, n, Src::of(&new[i].1)));
    }
    *nodes_now = nodes;
    crate::timeline::lap("engine: replace/insert/remove, the node list made");
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
            pkgrec: Vec::new(),
            pkg_predict: false,
            finisher_predictions: true,
            shell_escape: 2,
            prelude_jobname: "texput".to_owned(),
            prelude_bytes: None,
            time_limit: Some(std::time::Duration::from_secs(600)),
            threads: None,
            progress: None,
            edits: None,
            on_page_settled: None,
            // free on a cold build, helps edits; All (the engine's advice
            // for cold builds) mispredicts most of its runs while the
            // commands answer `Any`
            speculation: Speculation::Predicted,
            speculation_budget: 256,
            max_rounds: 5,
            tool_finder: None,
            bibtex: phitex_bibtex::Options::default(),
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
/// a file empty, `[name, text]` adds to it, `[name, "", "close"]` closes
/// it: what is written to it until it is opened again goes nowhere, as
/// TeX's §1370 sends a closed stream's writes to the log), in the order
/// first opened.
pub(crate) fn stream_texts(v: &Values, entries: &[ValueId]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for &e in entries {
        if is_cand_marker(v, e) {
            continue;
        }
        let xs = list_of(v, stream_entry(v, e));
        let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
        let k = if let Some(k) = out.iter().position(|(n, _)| *n == name) {
            k
        } else {
            out.push((name.clone(), String::new()));
            out.len() - 1
        };
        if xs
            .get(2)
            .is_some_and(|&c| v.as_str(c).as_deref() == Some("close"))
        {
            if !closed.contains(&name) {
                closed.push(name);
            }
            continue;
        }
        if let Some(&t) = xs.get(1) {
            if !closed.contains(&name) {
                out[k].1.push_str(&str_of(v, t));
            }
        } else {
            closed.retain(|n| *n != name);
            out[k].1.clear();
        }
    }
    out
}

/// The first element of a candidate marker among a stream's entries
/// ([`crate::run::Run::contribute_stream`]).
pub(crate) const CAND: &str = "cand";

/// Whether `e` is a candidate marker `["cand", piece, c]`, not an entry.
pub(crate) fn is_cand_marker(v: &Values, e: ValueId) -> bool {
    let xs = list_of(v, e);
    xs.len() == 3 && v.as_str(xs[0]).as_deref() == Some(CAND)
}

/// A `streams` entry without its page tag (`[j, entry]` or
/// `[j, entry, piece, candidate]` from page step `j`,
/// [`crate::run::Run::contribute_stream`]).
pub(crate) fn stream_entry(v: &Values, e: ValueId) -> ValueId {
    let xs = list_of(v, e);
    match xs.as_slice() {
        [j, x] | [j, x, _, _] if v.as_int(*j).is_some() => *x,
        _ => e,
    }
}

/// A page-tagged entry's page and, for a page whose break is inside a
/// piece, that piece and candidate.
fn page_tag(v: &Values, e: ValueId) -> Option<(i64, Option<(i64, i64)>)> {
    let xs = list_of(v, e);
    match xs.as_slice() {
        [j, _] => Some((v.as_int(*j)?, None)),
        [j, _, p, k] => Some((v.as_int(*j)?, Some((v.as_int(*p)?, v.as_int(*k)?)))),
        _ => None,
    }
}

/// The `streams` entries (an accumulator's, in the nodes' order) in the
/// order TeX writes them, the candidate markers dropped: a page's
/// entries where its finisher stands (right after the piece it fires
/// in), the pages placed after one piece in their order, and a piece's
/// entries after its candidate `c` after the entries of a page whose
/// break is inside that piece at a candidate before `c` (the page shipped
/// there first: its deferred `\write`s before a later `\closeout`,
/// §1370, §1374). Nothing is dropped but the markers; no read depends on
/// how the finishers placed after one piece are ordered among
/// themselves.
///
/// Example: piece P writes a, passes its candidate 1, closes the file
/// (`m1, close`); page 3 breaks in P at candidate 0 and writes b: the
/// entries `a, m1, close, [3, b, P, 0]` are written `a, b, close`.
pub(crate) fn doc_order(v: &Values, entries: &[ValueId]) -> Vec<ValueId> {
    // (runs of consecutive page entries: the finishers placed after one
    // piece, sorted by page, stable within one page)
    let mut es: Vec<ValueId> = Vec::with_capacity(entries.len());
    let mut i = 0;
    while i < entries.len() {
        if page_tag(v, entries[i]).is_none() {
            es.push(entries[i]);
            i += 1;
            continue;
        }
        let a = i;
        while i < entries.len() && page_tag(v, entries[i]).is_some() {
            i += 1;
        }
        let mut run: Vec<ValueId> = entries[a..i].to_vec();
        run.sort_by_key(|&e| page_tag(v, e).map_or(0, |t| t.0));
        es.extend(run);
    }
    // (each page's last entry, by the piece its break is inside of)
    let mut pages_of: FxHashMap<i64, Vec<(usize, i64)>> = FxHashMap::default();
    for (p, &e) in es.iter().enumerate() {
        if let Some((_, Some((pid, k)))) = page_tag(v, e) {
            let last = es
                .get(p + 1)
                .and_then(|&n| page_tag(v, n))
                .is_none_or(|t| t.0 != page_tag(v, e).map_or(-1, |u| u.0));
            if last {
                pages_of.entry(pid).or_default().push((p, k));
            }
        }
    }
    let mut anchor: Vec<Option<usize>> = vec![None; es.len()];
    if !pages_of.is_empty() {
        let mut i = 0;
        while i < es.len() {
            let xs = list_of(v, es[i]);
            if !is_cand_marker(v, es[i]) {
                i += 1;
                continue;
            }
            let (pid, c) = (int_of(v, xs[1]), int_of(v, xs[2]));
            let to = pages_of.get(&pid).and_then(|ps| {
                ps.iter()
                    .filter(|&&(p, k)| p > i && k < c)
                    .map(|&(p, _)| p)
                    .max()
            });
            if to.is_some() {
                anchor[i] = to;
                if let Some(&n) = es.get(i + 1)
                    && !is_cand_marker(v, n)
                {
                    anchor[i + 1] = to;
                    i += 2;
                    continue;
                }
            }
            i += 1;
        }
    }
    let mut after: FxHashMap<usize, Vec<ValueId>> = FxHashMap::default();
    for (i, a) in anchor.iter().enumerate() {
        if let Some(p) = a {
            after.entry(*p).or_default().push(es[i]);
        }
    }
    let mut out = Vec::with_capacity(es.len());
    for (i, &x) in es.iter().enumerate() {
        if anchor[i].is_none() && !is_cand_marker(v, x) {
            out.push(x);
        }
        if let Some(m) = after.remove(&i) {
            out.extend(m.into_iter().filter(|&y| !is_cand_marker(v, y)));
        }
    }
    out
}

/// The texts of the files the job wrote, from an accumulated `streams`
/// sequence, in the order first opened.
pub(crate) fn written_texts(v: &Values, streams: Option<ValueId>) -> Vec<(String, String)> {
    let streams = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
    stream_texts(v, &doc_order(v, &streams))
}

/// The definitions the read-back of each job-written file would make
/// (the third element of its `streams` entries, [`Run::interpret_line`]),
/// in TeX's order: per file, `(field, [value, global])`, each field once,
/// where it was first defined, with its last value.
pub(crate) fn written_defs(
    v: &Values,
    streams: Option<ValueId>,
) -> Vec<(String, Vec<(String, ValueId)>)> {
    let streams = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
    let mut out: Vec<(String, Vec<(String, ValueId)>)> = Vec::new();
    for e in doc_order(v, &streams) {
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
            // (`[field]`: a definition a read-back nested in the line
            // made, the other file's: a key of this file whose value that
            // file publishes, `UNIT` here)
            let (f, val) = match ds.as_slice() {
                [f, x, g] => (str_of(v, *f), v.list(vec![*x, *g])),
                [f] => (str_of(v, *f), ValueId::UNIT),
                _ => continue,
            };
            match out[k].1.iter_mut().find(|(n, _)| *n == f) {
                Some(slot) if val != ValueId::UNIT => slot.1 = val,
                Some(_) => {}
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
        let srcs: [&[u8]; 15] = [
            include_bytes!("../../../vendor/defuse/Cargo.toml"),
            include_bytes!("../../../vendor/defuse/src/lib.rs"),
            include_bytes!("../../../vendor/defuse/src/api.rs"),
            include_bytes!("../../../vendor/defuse/src/values.rs"),
            include_bytes!("../../../vendor/defuse/src/sync.rs"),
            include_bytes!("../../../vendor/defuse/src/obs.rs"),
            include_bytes!("../../../vendor/defuse/src/core/mod.rs"),
            include_bytes!("../../../vendor/defuse/src/core/commit.rs"),
            include_bytes!("../../../vendor/defuse/src/core/ctx.rs"),
            include_bytes!("../../../vendor/defuse/src/core/edit.rs"),
            include_bytes!("../../../vendor/defuse/src/core/eval.rs"),
            include_bytes!("../../../vendor/defuse/src/core/order.rs"),
            include_bytes!("../../../vendor/defuse/src/core/query.rs"),
            include_bytes!("../../../vendor/defuse/src/core/report.rs"),
            include_bytes!("../../../vendor/defuse/src/core/run.rs"),
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
    /// Cuts the finishers asked for (`crate::pages`: a routine run after
    /// the piece its page finished inside of changed what the piece's
    /// commands after the candidate saw): each costs a round.
    pub cuts: u64,
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
    /// The operations' run time, summed over every run, timed inside
    /// the run (zero without a clock): useful work, so `busy / wall`
    /// is the parallelism achieved. A worker waiting for work counts
    /// nothing.
    pub busy: std::time::Duration,
    /// The most operation runs at once (not worker loops).
    pub peak_running: usize,
    /// The batches of tasks the executor was given, those it ran on
    /// the calling thread (one task, or one thread), and the largest.
    /// A task is a worker's loop over runs, so a batch's size is the
    /// engine's task count, not the runs ready.
    pub batches: u64,
    pub batches_inline: u64,
    pub jobs_max_batch: usize,
    /// The longest chain of dependent waits among the live nodes after
    /// the last evaluation (the engine's `critical_path`), in nodes.
    pub critical_path: Option<usize>,
    /// The engine's realised critical path (`Report::realised_critical_path`:
    /// the longest chain of runs that waited for each other, counted from
    /// 1; reads a guess or a prediction answered are no link), the most
    /// over this build's evaluations.
    pub realised_critical_path: u32,
    /// The engine's realised critical path by time
    /// (`Report::realised_critical_time`: the same chain, summed by run
    /// time), the most over this build's evaluations, and that
    /// evaluation's run times by node (`Report::run_times`, longest
    /// first, the first ten): where the chain's time goes.
    pub realised_critical_time: std::time::Duration,
    pub run_times: Vec<(u64, std::time::Duration)>,
    /// The light page steps run (OUTPUT.md §4.2a), and those of them
    /// with no output routine, which may define anything.
    pub page_steps: u64,
    /// The engine's runs by operation (`exec`, `fold`, `finish`,
    /// `resume`, `place`, ...), every attempt, summed over the evaluations.
    pub runs_by_op: Vec<(&'static str, u64)>,
    /// Every node made to run again in the last build or edit: its raw
    /// id, its operation, and why (the engine's `Rerun` kind and the
    /// field's name).
    pub reruns: Vec<(u64, &'static str, String)>,
    /// (measure, with `PHITEX_RUN_LOG` set) every run attempt in the last
    /// build or edit, in order: the node's raw id, its operation, whether
    /// it resumed a parked run, whether it ran speculatively, how it
    /// ended (`o` finished, `s` stopped, `x` other), and the evaluation's
    /// index in the build or edit (its round).
    pub run_log: Vec<(u64, &'static str, bool, bool, u8, u64)>,
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
    /// when they ran: a diagnostic, never a silent fallback.
    pub regime_violations: u64,
    /// The engine's undeclared-definition errors (a definition outside
    /// a node's `may_define` bound). A static definition list is a
    /// guess the engine widens: a run defining outside it is counted in
    /// [`Stats::defs_refuted`], not here.
    pub undeclared_definitions: u64,
    /// The engine's guess counters (spec-redesign §0.2): declared
    /// claims, effects and definition lists a run refuted, and the
    /// siblings a refuted claim took or released.
    pub claims_refuted: u64,
    pub effects_refuted: u64,
    pub claim_siblings: u64,
    pub defs_refuted: u64,
    /// Nodes answering `Any` because their class is demoted
    /// (`crate::hints`; `PHITEX_HINTS=1` names the class).
    pub hints_demoted: u64,
    /// Nodes whose text is a token list (`["t", ...]`), and nodes
    /// reading source text (a document slice, a line, a file).
    pub nodes_token_frames: u64,
    pub nodes_text_frames: u64,
    /// Nodes whose own slice is a paragraph region (`hints::slices_of`):
    /// a paragraph another node's run took is not one.
    pub nodes_paragraphs: u64,
    /// Paragraph-region slices run inside another node's run (pulled).
    pub paragraphs_taken: u64,
    /// Runs that read on with their one group open because leaving it
    /// to the engine as a scope would define a field both at the run's
    /// level and in the scope: 0 since the engine takes both entries
    /// (kept as a check).
    pub scope_both_levels: u64,
    /// The fields whose speculative reads were refuted, with counts,
    /// most first.
    pub refuted_by_field: Vec<(String, u64)>,
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
    /// spec-final §1: where each loop-back round's time went (the
    /// engine's `Report::round_times`), over the evaluations.
    pub round_times: Vec<phitex_defuse::RoundTimes>,
    /// The operations' run time and runs by node kind
    /// (`crate::exec::KINDS`), over the evaluations.
    pub op_by_kind: Vec<(&'static str, std::time::Duration, u64)>,
    /// The environment pushes made to the engine (spec-final §4.5).
    pub env_calls: u64,
}

/// The cold accounting of an evaluation (spec-final §1), with
/// `PHITEX_TEX_TIME`: per round the engine's split of its time, the
/// runs per node, the parallel shape (§9b: raises, pending waits, the
/// finishers waited on), the op time by node kind, the realised
/// critical path in nodes and time and the kinds on the engine's
/// critical chain, and the environment pushes.
#[allow(clippy::too_many_lines)]
fn print_accounting(eng: &Engine, r: &Report, ex: &crate::exec::Threads, env_calls: u64) {
    let ms = |ns: u64| {
        #[allow(clippy::cast_precision_loss)]
        let x = ns as f64 / 1e6;
        x
    };
    let (mut wall, mut op) = (0u64, 0u64);
    for (i, t) in r.round_times.iter().enumerate() {
        eprintln!(
            "acct: round {i}: wall {:.3} ms, op {:.3} ms, commit {:.3} ms, check {:.3} ms, idle {:.3} ms, runs {}, memo hits {}",
            ms(t.wall_ns),
            ms(t.op_ns),
            ms(t.commit_ns),
            ms(t.check_ns),
            ms(t.idle_ns),
            t.runs,
            t.memo_hits
        );
        wall += t.wall_ns;
        op += t.op_ns;
    }
    let runs = r.runs.max(1);
    let live = eng.live_nodes().max(1);
    #[allow(clippy::cast_precision_loss)]
    let pct = |n: u64| 100.0 * n as f64 / runs as f64;
    #[allow(clippy::cast_precision_loss)]
    let per_run_us = (wall.saturating_sub(op)) as f64 / 1e3 / runs as f64;
    #[allow(clippy::cast_precision_loss)]
    let rpn = r.runs as f64 / live as f64;
    eprintln!(
        "acct: memo {} bytes (the previous evaluation's round results)",
        r.memo_bytes
    );
    eprintln!(
        "acct: {} rounds, {} runs, {} live nodes, {rpn:.2} runs/node, raises {} ({:.2}% of runs), pending waits {} ({:.2}% of runs), round wall minus op {per_run_us:.1} us/run at {} threads",
        r.round_times.len(),
        r.runs,
        live,
        r.raises,
        pct(r.raises),
        r.pending_waits,
        pct(r.pending_waits),
        ex.n
    );
    let kinds: Vec<String> = ex
        .by_kind()
        .into_iter()
        .filter(|k| k.2 > 0)
        .map(|(k, d, n)| format!("{k} {:.3} ms/{n}", d.as_secs_f64() * 1e3))
        .collect();
    eprintln!("acct: op time by kind: {}", kinds.join(", "));
    // (the time inside the engine's reads by kind, and the op bodies:
    // the op time less it)
    let read_total: u64 = r.read_ns.iter().map(|&(_, n)| n).sum();
    let reads: Vec<String> = r
        .read_ns
        .iter()
        .filter(|&&(_, n)| n > 0)
        .map(|&(k, n)| format!("{k} {:.3} ms", ms(n)))
        .collect();
    eprintln!(
        "acct: read_ns by kind: {}; total {:.3} ms; op bodies (op minus reads) {:.3} ms",
        reads.join(", "),
        ms(read_total),
        ms(op.saturating_sub(read_total))
    );
    // (what stays parked at the end: the nodes whose wait chains end at
    // the environment, and each chain's head with the field it asks)
    let waits: Vec<String> = r
        .env_waits
        .iter()
        .take(20)
        .map(|&(n, f)| {
            format!(
                "{} on {}",
                describe(eng, n),
                crate::val::field_name(eng.values(), f)
            )
        })
        .collect();
    eprintln!(
        "acct: parked at the end: {} nodes stalled on the environment, {} chain heads: {waits:?}",
        r.env_stalled,
        r.env_waits.len()
    );
    // (the progress guarantee: nodes run conservatively after a refutation,
    // and the refutations by field, most first)
    let mut by_field: Vec<(String, u64)> = r
        .refuted_by_field
        .iter()
        .map(|&(f, n)| (crate::val::field_name(eng.values(), f), n))
        .collect();
    by_field.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    by_field.truncate(20);
    eprintln!(
        "acct: {} nodes conservative, {} nodes refuted (at most {} times); refuted by field {by_field:?}",
        r.nodes_conservative, r.refuted_nodes, r.refutations_max
    );
    // (the kinds on the engine's critical chain, and the finishers waited
    // on: §9b, only the link and a cut's resume wait on one)
    let mut on_chain: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let chain = eng.critical_chain();
    // (the chain's links by the node's kind and why the next one waited
    // on it, and its first links in order)
    let mut links: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut first = Vec::new();
    for (n, c) in &chain {
        let op = eng.node_info(*n).map_or("?", |i| i.op);
        *on_chain.entry(op).or_default() += 1;
        let c = format!("{c:?}");
        let c = c.split([' ', '(', '{']).next().unwrap_or("").to_owned();
        *links.entry(format!("{op} {c}")).or_default() += 1;
        if first.len() < 40 {
            first.push(format!("{op}:{c}"));
        }
    }
    eprintln!(
        "acct: critical chain {} links by kind and cause {links:?}; first: {}",
        chain.len(),
        first.join(" ")
    );
    let waited_fin = r
        .top_waited
        .iter()
        .filter(|(n, _)| eng.node_info(*n).is_some_and(|i| i.op == "finish"))
        .count();
    eprintln!(
        "acct: critical path {} nodes, {:.3} ms; chain kinds {on_chain:?}; finishers waited on {waited_fin}; env calls {env_calls}",
        r.realised_critical_path,
        r.realised_critical_time.as_secs_f64() * 1e3
    );
    let ld = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "acct: commands {}; exec claims declared: none {}, a count {}, pending {}",
        ld(&crate::ops::COMMANDS),
        ld(&crate::ops::EXEC_CLAIMS[0]),
        ld(&crate::ops::EXEC_CLAIMS[1]),
        ld(&crate::ops::EXEC_CLAIMS[2])
    );
    // (the nodes waited on most: the definers the waits are on, by
    // operation, with the start of the text or arguments)
    let waited: Vec<String> = r
        .top_waited
        .iter()
        .map(|&(n, w)| {
            let info = eng.node_info(n);
            let op = info.as_ref().map_or("gone", |i| i.op);
            let a = info
                .and_then(|i| i.args.first().map(|&x| eng.values().render(x)))
                .unwrap_or_default()
                .chars()
                .take(48)
                .collect::<String>();
            format!("{w}x {op} {a}")
        })
        .collect();
    eprintln!("acct: waited on (top {}): {waited:?}", waited.len());
    let longest: Vec<String> = r
        .run_times
        .iter()
        .take(8)
        .map(|&(n, t)| {
            let info = eng.node_info(n);
            let op = info.as_ref().map_or("gone", |i| i.op);
            let a = info
                .and_then(|i| i.args.first().map(|&x| eng.values().render(x)))
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect::<String>();
            format!("{:.1} ms {op} {a}", t.as_secs_f64() * 1e3)
        })
        .collect();
    eprintln!("acct: longest runs: {longest:?}");
    let refuted: Vec<(String, u64)> = r
        .refuted_by_field
        .iter()
        .take(12)
        .map(|&(f, n)| (crate::val::field_name(eng.values(), f), n))
        .collect();
    eprintln!("acct: predictions refuted by field (top 12): {refuted:?}");
    eprintln!(
        "acct: stops by reader and field (top 15): {:?}; waits by kind {:?}; by reader {:?}",
        crate::run::take_stops(eng.values(), 15),
        r.waits_by_kind,
        r.waits_by_op
    );
}

/// A round's counters (`PHITEX_TEX_TIME`).
fn print_round(t: std::time::Duration, r: &Report) {
    // (why nodes ran again, by kind: `Report::rerun_causes`)
    let mut causes = std::collections::BTreeMap::<String, usize>::new();
    if crate::exec::obs_flag!("PHITEX_RERUN_SHOW") {
        for (n, c) in &r.rerun_causes {
            eprintln!("rerun: node {} {c:?}", n.raw());
        }
    }
    for (_, c) in &r.rerun_causes {
        let k = format!("{c:?}");
        let k = k.split([' ', '(', '{']).next().unwrap_or("").to_owned();
        *causes.entry(k).or_default() += 1;
    }
    eprintln!(
        "finishers: {} created with a prediction, {} without; speculations {} confirmations {} mispredictions {}",
        crate::pages::FIN_PREDICTED.load(std::sync::atomic::Ordering::Relaxed),
        crate::pages::FIN_UNPREDICTED.load(std::sync::atomic::Ordering::Relaxed),
        r.speculations,
        r.confirmations,
        r.mispredictions
    );
    eprintln!(
        "rounds: {}, final nodes run {}, capped at the round limit: {}",
        r.rounds,
        r.final_nodes_run,
        r.capped.as_ref().map_or(0, Vec::len)
    );
    eprintln!(
        "round guesses: {} claims refuted, {} effects refuted, {} claim siblings, {} definitions refuted",
        r.claims_refuted, r.effects_refuted, r.claim_siblings, r.defs_refuted
    );
    eprintln!(
        "round rerun causes: {} nodes, {causes:?}, realised cp {} nodes {:?}, longest runs {:?}",
        r.rerun_causes.len(),
        r.realised_critical_path,
        r.realised_critical_time,
        r.run_times
            .iter()
            .take(5)
            .map(|&(n, t)| (n.raw(), t))
            .collect::<Vec<_>>()
    );
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
        for (k, d, n) in ex.by_kind() {
            match self.op_by_kind.iter_mut().find(|x| x.0 == k) {
                Some(x) => {
                    x.1 += d;
                    x.2 += n;
                }
                None => self.op_by_kind.push((k, d, n)),
            }
        }
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
        self.hints_demoted = get(c[3]);
        self.nodes_token_frames = get(c[4]);
        self.nodes_text_frames = get(c[5]);
        self.nodes_paragraphs = get(c[6]);
        self.paragraphs_taken = get(c[7]);
        self.scope_both_levels = get(c[8]);
        self.undeclared_definitions = u64::try_from(
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

    fn add_reruns(&mut self, eng: &Engine, r: &Report) {
        use phitex_defuse::Rerun as Rr;
        let v = eng.values();
        for &(n, c) in &r.rerun_causes {
            let op = eng.node_info(n).map_or("?", |i| i.op);
            let f = match c {
                Rr::Changed { field, .. }
                | Rr::Rebind { field }
                | Rr::Loop { field }
                | Rr::Environment { field } => Some(field),
                Rr::Refuted { field } | Rr::Woken { field } | Rr::Range { field } => field,
                _ => None,
            };
            let kind = format!("{c:?}");
            let kind = kind.split([' ', '(', '{']).next().unwrap_or("").to_owned();
            let mut why =
                f.map_or_else(|| kind.clone(), |f| format!("{kind} {}", field_name(v, f)));
            if crate::exec::prof_flag!("PHITEX_RUN_LOG") {
                // (measure: where the value read differs from the one the
                // run had read, and the round)
                if let Rr::Changed { old, new, .. } = c {
                    match structural_diff(v, old, new, String::new()) {
                        Some((path, a, b)) => {
                            let cut = |x: String| x.chars().take(90).collect::<String>();
                            why = format!("{why} @{path}: {} | {}", cut(a), cut(b));
                        }
                        None => why = format!("{why} (EQUAL value)"),
                    }
                }
                why = format!("{why} #r{}", self.rounds);
            }
            self.reruns.push((n.raw(), op, why));
        }
        if crate::exec::prof_flag!("PHITEX_RUN_LOG") {
            let log = std::mem::take(
                &mut *crate::ops::RUN_LOG
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            let round = self.rounds;
            for (n, resumed, spec, how) in log {
                let op = eng
                    .node_info(phitex_defuse::NodeId::from_raw(n))
                    .map_or("?", |i| i.op);
                self.run_log.push((n, op, resumed, spec, how, round));
            }
        }
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
        self.claims_refuted += r.claims_refuted;
        self.effects_refuted += r.effects_refuted;
        self.claim_siblings += r.claim_siblings;
        self.defs_refuted += r.defs_refuted;
        self.version = r.version;
        self.absorbed.extend_from_slice(&r.edits_applied);
        self.steps += r.steps;
        self.ready_total += r.ready_total;
        self.speculations += r.speculations;
        self.confirmations += r.confirmations;
        self.mispredictions += r.mispredictions;
        self.predictions_confirmed += r.predictions_confirmed;
        self.predictions_refuted += r.predictions_refuted;
        self.round_times.extend_from_slice(&r.round_times);
        self.realised_critical_path = self.realised_critical_path.max(r.realised_critical_path);
        if r.realised_critical_time >= self.realised_critical_time {
            self.realised_critical_time = r.realised_critical_time;
            self.run_times = r
                .run_times
                .iter()
                .take(10)
                .map(|&(n, t)| (n.raw(), t))
                .collect();
        }
        self.spec_stops += r.spec_stops;
        self.spec_shape_changes += r.spec_shape_changes;
        self.raises += r.raises;
        self.max_ready = self.max_ready.max(r.max_ready);
        self.fuel_outs += r.fuel_outs;
        self.declarations_refuted += r.declarations_refuted;
        self.page_steps += Report::by_op(&r.runs_by_op, "finish");
        self.gated += r.gated;
        self.top_waited = r.top_waited.iter().map(|&(n, w)| (n.raw(), w)).collect();
        for &(k, n) in &r.waits_by_kind {
            match self.waits_by_kind.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += n,
                None => self.waits_by_kind.push((k, n)),
            }
        }
        for &(k, n) in &r.runs_by_op {
            match self.runs_by_op.iter_mut().find(|x| x.0 == k) {
                Some(x) => x.1 += n,
                None => self.runs_by_op.push((k, n)),
            }
        }
    }
}

/// An engine with the operations registered.
struct Graph {
    eng: Engine,
    /// The values the job's environment sets at the start of the
    /// document ([`job_env`]), for `PHITEX_RECORD_CHECK`
    /// ([`crate::run::record_check`]).
    start_env: FxHashMap<Field, ValueId>,
    /// The evaluation errors met (the engine's), as messages.
    eval_errors: Vec<String>,
    /// The warnings an evaluation ended with (the read-back files still
    /// changing at the round cap), as messages.
    eval_warnings: Vec<String>,
    /// [`Options::tool_finder`].
    tool_finder: Option<ToolFinder>,
    ops: Ops,
    mark: OpId,
    dvi: OpId,
    /// The link (`crate::ops::Link`), pushed before the end node.
    link: OpId,
    /// The environment and the absent fields as last pushed to the
    /// engine (sorted), and the engine calls the pushes made
    /// ([`Graph::push_env`], spec-final §4.5).
    env_pushed: Vec<(Field, ValueId)>,
    absent_pushed: Vec<Field>,
    pub(crate) env_calls: u64,
    /// The job's bindings (the format's state, the job's names and
    /// dates: [`job_env`]), pushed with the environment.
    job: Vec<(Field, ValueId)>,
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
    /// The files the last evaluation's runs opened for writing: their
    /// nodes are placed from the streams (`ops::Files`), so `loopfiles`
    /// lists only the files written before and not opened now.
    opened_last: Vec<String>,
    /// The executor's threads ([`Options::threads`]).
    threads: usize,
    /// The comments of the document's canonical text by line
    /// ([`canonical_text`]): `doc:marks` in the environment, read by a
    /// run that reads a comment's mark as text (`Run::splice_mark`).
    marks: Marks,
    /// The `wdef:<field>` loop-backs declared: the fields the read-back
    /// of a job-written file defines.
    wdefs: Vec<String>,
    /// The round-one guesses of the `wkeys:<name>` loop-backs.
    wkeys_guess: Vec<(String, Vec<String>)>,
    /// What the executor runs now, for readers on other threads.
    watch: crate::exec::Watch,
    /// The engine's scheduler ([`Options::speculation`]).
    sched: Box<dyn phitex_defuse::Scheduler + Send>,
    /// The files the environment provider found or missed during runs
    /// (name, field, value, hash), merged into `files`/`absent`/`reads`
    /// before the environment is set again (`Graph::provide`).
    provided: Arc<std::sync::Mutex<Vec<Provided>>>,
    /// The fields of the files the provider cut into units
    /// ([`file_units`]), merged into `files` with `provided`.
    provided_units: Arc<std::sync::Mutex<Vec<ProvidedUnit>>>,
    /// The package loads' records and hints (`crate::pkgrec`), once the
    /// build has set them.
    loads: Option<Arc<crate::pkgrec::LoadHints>>,
}

impl Graph {}

/// A file the environment provider answered: name, field, the value
/// (`None`: absent), and the finder's hash.
type Provided = (String, Field, Option<ValueId>, u64);

/// A file the provider cut into units ([`file_units`]): its name, the
/// finder's hash, and its fields with their values (none: the file is
/// absent).
type ProvidedUnit = (String, u64, Vec<(Field, ValueId)>);

/// The files the provider cut into units, by name: each cut once.
type UnitCuts = std::sync::Mutex<FxHashMap<String, Arc<OnceLock<FxHashMap<u64, ValueId>>>>>;

/// The file a units field names: `units:<name>` (the order) or
/// `unit:<name>:<i>` (one unit).
fn units_file(field: &str) -> Option<&str> {
    if let Some(n) = field.strip_prefix("units:") {
        return Some(n);
    }
    field
        .strip_prefix("unit:")?
        .rsplit_once(':')
        .map(|(n, _)| n)
}

/// A file a document's `\input` reads, as units (spec-redesign §1
/// `unit:<file>:<i>`): the file cut by the same analysis a document's
/// text is cut by, each slice a field of the environment,
/// `unit:<name>:<i>` = `[text, offset]`, and the order `units:<name>` =
/// `[path, UNIT, [sliceable, [[unit field, starts], ...]]]` (a written
/// file's shape, `ops::Files`). The reader of the file reads the order
/// only, so an edit inside a unit re-runs the unit's piece, not the
/// node that read the file; each unit's hint is registered by its field
/// (`hints::register_unit`). A file that is not read as a document's
/// text (`inputs::document_file`) is not cut: its order is empty and
/// not sliceable, and the reader reads its text.
fn file_units(
    v: &Values,
    reg: &crate::hints::Registry,
    defs: &Defs,
    name: &str,
    path: &str,
    bytes: &[u8],
) -> Vec<(Field, ValueId)> {
    let text = latin1(bytes);
    let pv = v.str(path);
    let order = |sliceable: bool, units: Vec<ValueId>| {
        v.list(vec![
            pv,
            ValueId::UNIT,
            v.list(vec![v.int(i64::from(sliceable)), v.list(units)]),
        ])
    };
    let uf = v.field(&format!("units:{name}"));
    if !crate::inputs::document_file(path, &text) {
        return vec![(uf, order(false, Vec::new()))];
    }
    let t = crate::hints::text_hints(&text, pass::FileKind::Document, defs);
    crate::hints::register_text(reg, pv, &t);
    let mut out = Vec::with_capacity(t.slices.len() + 1);
    let mut units = Vec::with_capacity(t.slices.len());
    for (i, &(s, e)) in t.slices.iter().enumerate() {
        let Some(u) = text.get(s..e) else { continue };
        let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
        let key = v.str(&format!("unit:{name}:{i}"));
        crate::hints::register_unit(reg, key, crate::hints::slice_hint_of(&t, s));
        out.push((
            v.field(&format!("unit:{name}:{i}")),
            v.list(vec![v.str(u), v.int(i64::try_from(s).unwrap_or(0))]),
        ));
        units.push(v.list(vec![key, v.int(i64::from(starts))]));
    }
    out.insert(0, (uf, order(true, units)));
    out
}

impl Graph {
    #[allow(clippy::too_many_lines)]
    fn new(time: Option<std::time::Duration>, threads: Option<usize>) -> Graph {
        let mut eng = Engine::new();
        // (the report's critical path and chain, a serial pass: `profile`
        // builds only)
        // (`PHITEX_NO_HEIGHTS`: a profile build without them, as a
        // release build runs, for its timings)
        eng.set_heights(cfg!(feature = "profile") && !crate::exec::prof_flag!("PHITEX_NO_HEIGHTS"));
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
        // the read-back files can have more than one fixpoint (a page
        // number that moves a break that moves it back), so the fixpoint
        // an edit reaches from the last values can differ from the cold
        // build's: every evaluation's rounds start from the initial
        // loop-back values, as a fresh engine's do, and a node is offered
        // its result from the previous evaluation's same round, rechecked
        // through the commit path (the memo saves work, never decides a
        // value)
        eng.set_loop_start(phitex_defuse::LoopStart::PerRound);
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let scan = eng.register(Arc::new(crate::pages::Scan(shared.clone())));
        let finish = eng.register(Arc::new(crate::pages::Finish(shared.clone())));
        let resume = eng.register(Arc::new(crate::pages::Resume(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi(shared.clone())));
        let link = eng.register(Arc::new(crate::ops::Link(shared.clone())));
        let mark = eng.register(Arc::new(Mark));
        let files_op = eng.register(Arc::new(Files(shared.clone())));
        let tool = eng.register(Arc::new(crate::tools::ToolRun(shared.clone())));
        let toolnode = eng.register(Arc::new(crate::tools::ToolNode(shared.clone())));
        let pdfpage = eng.register(Arc::new(crate::pdfout::PdfPage));
        let fields = Arc::new(crate::fields::Fields::new(eng.values()));
        let class_proj = eng.register_projection(Arc::new(crate::val::class_projection));
        // (handles held outside the graph: pinned, or a value sweep
        // frees them while a run still defines through them)
        for f in fields.handles() {
            eng.pin(f.0);
        }
        let ops = Ops {
            exec,
            put,
            scan,
            finish,
            resume,
            tool,
            toolnode,
            pdfpage,
            tool_opts: Arc::default(),
            fields,
            defs: Arc::new(Mutex::new(crate::hints::no_defs().clone())),
            hints: Arc::default(),
            pkg_log: Arc::new(Mutex::new(Vec::new())),
            pkg_loads: Arc::new(Mutex::new(None)),
            out_ok: Arc::new(Mutex::new(None)),
            fin_defs: Arc::default(),
            scan_guess: Arc::default(),
            fin_defs_start: Arc::default(),
            scan_guess_start: Arc::default(),
            fin_predict: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            pdf_cmds: Arc::default(),
            class_proj,
        };
        let _ = shared.set(ops.clone());
        declare_fields(&mut eng);
        Graph {
            start_env: FxHashMap::default(),
            eng,
            ops,
            mark,
            dvi,
            link,
            env_pushed: Vec::new(),
            absent_pushed: Vec::new(),
            env_calls: 0,
            job: Vec::new(),
            files_op,
            files: Vec::new(),
            reads: Vec::new(),
            queries: Vec::new(),
            absent: Vec::new(),
            written: Vec::new(),
            threads: crate::exec::threads(threads),
            marks: Marks::default(),
            wdefs: Vec::new(),
            wkeys_guess: Vec::new(),
            opened_last: Vec::new(),
            watch: crate::exec::Watch::new(crate::exec::threads(threads), None),
            sched: Box::new(phitex_defuse::Greedy::default()),
            eval_errors: Vec::new(),
            eval_warnings: Vec::new(),
            tool_finder: None,
            provided: Arc::default(),
            provided_units: Arc::default(),
            loads: None,
        }
    }

    /// An evaluation error as a reader can act on it: the error, and
    /// each node it names with what it is.
    fn diagnose(&self, e: &phitex_defuse::EvalError, unit: UnitId) -> String {
        use phitex_defuse::EvalError as E;
        let mut ids: Vec<NodeId> = Vec::new();
        match e {
            E::Cycle(ns) => ids.extend(ns.iter().copied()),
            E::RunawayNode { node, .. }
            | E::RunawayCreation { node, .. }
            | E::UndeclaredDefinition { node, .. }
            | E::OpPanicked { node, .. }
            | E::FinalMovedLoop { node, .. } => ids.push(*node),
            E::HookPanicked { node, .. } => ids.extend(*node),
            E::NoProgress { waiters, .. } | E::Deadline { waiters, .. } => {
                ids.extend(waiters.iter().take(20).map(|w| w.node));
            }
            _ => {}
        }
        // (the field and the nodes' texts first: a harness shows the
        // head of the message; the error's full form last)
        let mut out = match e {
            // (a static hook of ours panicked: which hook of which
            // operation, first, then the engine's whole message)
            E::HookPanicked {
                op, hook, message, ..
            } => format!(
                "internal error: the {hook} hook of operation {op} panicked: {message}\nevaluation: {e}"
            ),
            _ => format!("evaluation: {e}"),
        };
        let v = self.eng.values();
        if let E::UndeclaredDefinition { field, .. } | E::ConflictingPublication { field } = e {
            let _ = write!(out, "\n  field {}", field_name(v, *field));
        }
        // (a runaway node's causes, their fields named)
        if let E::RunawayNode { causes, .. } = e {
            use phitex_defuse::Cause as C;
            let named: Vec<String> = causes
                .iter()
                .map(|c| match c {
                    C::Rebind { field, .. } => format!("Rebind {}", field_name(v, *field)),
                    C::Changed { field, site } => {
                        format!("Changed {} at {site:?}", field_name(v, *field))
                    }
                    C::Loop { field } => format!("Loop {}", field_name(v, *field)),
                    C::Environment { field } => format!("Environment {}", field_name(v, *field)),
                    C::Range { field, .. } => format!("Range {}", field_name(v, *field)),
                    C::Woken { field, .. } => format!(
                        "Woken {}",
                        field.map_or_else(|| "-".to_owned(), |f| field_name(v, f))
                    ),
                    other => format!("{other:?}"),
                })
                .collect();
            let _ = write!(out, "\n  causes named: {}", named.join(", "));
        }
        if let E::FinalMovedLoop { field, .. } = e {
            let _ = write!(
                out,
                "\n  the loop-back moved: {}",
                field_name(self.eng.values(), *field)
            );
        }
        // (the engine does not say which loop-backs kept changing: the
        // job's read-back fields are the candidates)
        if let E::NonConvergence { .. } = e {
            // (the files the job wrote: its own runs declared their
            // loop-backs, `Run::written_decl`, the client may not have
            // seen them yet)
            let mut names = self.written.clone();
            for n in self.opened(unit) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            for name in &names {
                let _ = write!(out, "\n  written file {name}");
            }
            for f in &self.wdefs {
                let _ = write!(out, "\n  read-back field {}", f.replace('\u{1}', " "));
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
                    let h = crate::hints::hint_of_args(&self.ops.hints, v, &i.args);
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
            let text = latin1(&bytes);
            let val = v.list(vec![v.str(&path), v.str(&text)]);
            self.files.push((f, val));
        } else {
            self.absent.push(f);
        }
    }

    /// [`Graph::supply`] of every file of `names` in order, the files
    /// found, read, hashed and made values on the pool (spec-final §9b:
    /// the cold build's serial part), recorded in the same order.
    fn supply_all(&mut self, finder: &Finder, names: &[String]) {
        let mut seen = phitex_defuse::FxHashSet::default();
        let todo: Vec<(&str, Field)> = names
            .iter()
            .filter(|n| seen.insert(n.as_str()))
            .map(|n| (n.as_str(), self.eng.values().field(&format!("file:{n}"))))
            .filter(|&(n, f)| {
                !(self.files.iter().any(|(g, _)| *g == f)
                    || self.absent.contains(&f)
                    || self.written.iter().any(|w| w == n))
            })
            .collect();
        let v = self.eng.values();
        let mut ex = crate::exec::Coarse(crate::exec::Threads::new(self.threads));
        let got: Vec<(u64, Option<ValueId>)> = crate::exec::map_on(&mut ex, todo.len(), &|k| {
            let found = finder(todo[k].0);
            let hash = file_hash(found.as_ref());
            let val = found.map(|(path, bytes)| {
                let text = latin1(&bytes);
                v.list(vec![v.str(&path), v.str(&text)])
            });
            (hash, val)
        });
        for (&(name, f), (hash, val)) in todo.iter().zip(got) {
            self.queries.push((name.to_owned(), val.is_some()));
            self.reads.push((name.to_owned(), hash));
            match val {
                Some(x) => self.files.push((f, x)),
                None => self.absent.push(f),
            }
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
        // (a file read as units, [`file_units`]: cut once, each of its
        // fields answered from the cut)
        let cut: UnitCuts = std::sync::Mutex::new(FxHashMap::default());
        let units_log = Arc::clone(&self.provided_units);
        let (reg, defs) = (Arc::clone(&self.ops.hints), Arc::clone(&self.ops.defs));
        let finder_u = Arc::clone(&finder);
        let finder_t = Arc::clone(&finder);
        let tools = self.tool_finder.clone();
        self.eng.set_env_provider(Box::new(move |v, f| {
            let field = v.as_str(f.0)?;
            if let Some(name) = units_file(&field) {
                if written.iter().any(|w| w == name) {
                    return None;
                }
                let cell = cut
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .entry(name.to_owned())
                    .or_default()
                    .clone();
                let m = cell.get_or_init(|| {
                    let found = finder_u(name);
                    let hash = file_hash(found.as_ref());
                    let defs = defs.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let es = found
                        .map(|(path, bytes)| file_units(v, &reg, &defs, name, &path, &bytes))
                        .unwrap_or_default();
                    let m = es.iter().map(|&(g, x)| (g.0.raw(), x)).collect();
                    if let Ok(mut l) = units_log.lock() {
                        l.push((name.to_owned(), hash, es));
                    }
                    m
                });
                return Some(m.get(&f.0.raw()).copied());
            }
            // (a tool's support file, `tfile:<search>:<name>`: the
            // client's lookup for tools, else its finder by name)
            if let Some(rest) = field.strip_prefix("tfile:") {
                let (code, name) = rest.split_once(':')?;
                let search = ToolSearch::of_code(code)?;
                let found = match &tools {
                    Some(t) => t(search, name),
                    None => finder_t(name),
                };
                let hash = file_hash(found.as_ref());
                let val = found.map(|(path, bytes)| {
                    let text = latin1(&bytes);
                    v.list(vec![v.str(&path), v.str(&text)])
                });
                if let Ok(mut l) = log.lock() {
                    l.push((field.clone(), f, val, hash));
                }
                return Some(val);
            }
            let name = field.strip_prefix("file:")?;
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
                    let text = latin1(&bytes);
                    v.list(vec![v.str(&path), v.str(&text)])
                });
                if let Ok(mut l) = log.lock() {
                    l.push((name.to_owned(), f, val, hash));
                }
                let ns = u64::try_from(t1.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let all = spent.fetch_add(ns, std::sync::atomic::Ordering::Relaxed) + ns;
                if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
        self.absorb_units();
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
            // (a tool's support file: in the environment, not among the
            // names the finder is asked for again)
            if !name.starts_with("tfile:") {
                self.queries.push((name.clone(), val.is_some()));
                self.reads.push((name, hash));
            }
            match val {
                Some(x) => self.files.push((f, x)),
                None => self.absent.push(f),
            }
        }
    }

    /// Merges the units the provider cut ([`file_units`]) into the
    /// supplied files (a file's, absent: into `absent`).
    fn absorb_units(&mut self) {
        let got = self
            .provided_units
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default();
        if got.is_empty() {
            return;
        }
        let have: phitex_defuse::FxHashSet<Field> = self
            .files
            .iter()
            .map(|&(g, _)| g)
            .chain(self.absent.iter().copied())
            .collect();
        for (name, hash, es) in got {
            // (a file read as units is a file the build read: listed, its
            // hash kept, as [`Graph::supply`] keeps a file's)
            if !self.reads.iter().any(|(n, _)| *n == name) {
                self.queries.push((name.clone(), !es.is_empty()));
                self.reads.push((name.clone(), hash));
            }
            if es.is_empty() {
                let uf = self.eng.values().field(&format!("units:{name}"));
                if !have.contains(&uf) {
                    self.absent.push(uf);
                }
            }
            for (f, x) in es {
                if !have.contains(&f) {
                    self.files.push((f, x));
                }
            }
        }
    }

    /// A unit's text and offset in its file, as the environment holds it
    /// (`unit:<file>:<i>`, [`file_units`]).
    fn unit_value(&self, key: &str) -> Option<(String, usize)> {
        let v = self.eng.values();
        let f = v.field(key);
        let &(_, x) = self.files.iter().find(|&&(g, _)| g == f)?;
        let xs = list_of(v, x);
        let text = str_of(v, *xs.first()?);
        let at = usize::try_from(int_of(v, *xs.get(1)?)).ok()?;
        Some((text, at))
    }

    /// A file's whole text as the environment holds it: its `file:` value
    /// (by its name or its path), else its units joined in order.
    fn file_text(&self, name: &str) -> Option<Arc<str>> {
        let v = self.eng.values();
        for &(g, x) in &self.files {
            let fname = field_name(v, g);
            let xs = list_of(v, x);
            if let Some(n) = fname.strip_prefix("file:")
                && let [p, t] = xs[..]
                && (n == name || str_of(v, p) == name)
            {
                return Some(str_of(v, t).into());
            }
        }
        for &(g, x) in &self.files {
            let fname = field_name(v, g);
            let xs = list_of(v, x);
            let Some(n) = fname.strip_prefix("units:") else {
                continue;
            };
            if n != name && xs.first().is_none_or(|&p| str_of(v, p) != name) {
                continue;
            }
            let order = xs.get(2).map(|&o| list_of(v, o)).unwrap_or_default();
            let units = order.get(1).map(|&l| list_of(v, l)).unwrap_or_default();
            let mut text = String::new();
            for u in units {
                let key = list_of(v, u).first().map(|&k| str_of(v, k))?;
                text.push_str(&self.unit_value(&key)?.0);
            }
            return Some(text.into());
        }
        None
    }

    /// A file's units ([`file_units`]) dropped from the environment:
    /// the file the job writes, or one cut again.
    fn drop_units(&mut self, name: &str) {
        let v = self.eng.values();
        let own = |f: Field| units_file(&field_name(v, f)).is_some_and(|n| n == name);
        self.files.retain(|&(g, _)| !own(g));
        self.absent.retain(|&g| !own(g));
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

    /// Round one's guesses for the loop-backs: the last build's values
    /// (`opts`); none without one.
    fn guesses(&mut self, opts: &Options) {
        self.wkeys_guess.clone_from(&opts.wkeys);
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
        self.drop_units(name);
        // (with `fixpoint`, `file:<name>` given a value of its own, the
        // empty list, which a run reads as no file: a run that read it
        // absent before the `\openout` declared the loop-back, LaTeX's
        // `\@input` of the `.aux` in `\document` and `\@starttoc`'s
        // `.toc`, runs again and reads the loop-back; left absent,
        // nothing it read changed and it kept the empty read. Without
        // it the file stays absent, as the reader found it: the empty
        // list would be a change of the environment at the next edit,
        // and every reader of the file would run again on it)
        if fixpoint {
            let none = self.eng.values().list(Vec::new());
            self.files.push((f, none));
        } else {
            self.absent.push(f);
        }
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
            {
                decl_trace(&self.eng, w, guess);
                self.eng.declare_loopback(w, guess);
            }
            // the guessed keys' `wdef:<field>` loop-backs declared with
            // it: a read-back of a guessed key never waits for them
            let keys: Vec<String> = self
                .wkeys_guess
                .iter()
                .filter(|(n, _)| n == name && defines_only(name))
                .flat_map(|(_, ks)| ks.iter().map(|k| wdef_id(name, k)))
                .collect();
            for k in keys {
                if !self.wdefs.contains(&k) {
                    let v = self.eng.values();
                    let none = v.list(Vec::new());
                    let wd = v.field(&format!("wdef:{k}"));
                    {
                        decl_trace(&self.eng, wd, none);
                        self.eng.declare_loopback(wd, none);
                    }
                    self.wdefs.push(k);
                }
            }
        }
        self.written.push(name.to_owned());
    }

    /// Pushes the environment (spec-final §4.5): once per evaluation,
    /// and only when it changed since the last push (the entries and the
    /// absent fields compared as sorted lists; an unchanged one is no
    /// call to the engine).
    fn push_env(&mut self, env: Vec<(Field, ValueId)>) {
        // (a field given twice: the later entry, as the engine's map has
        // it; the job's bindings first)
        let map: FxHashMap<Field, ValueId> = self.job.iter().copied().chain(env).collect();
        let mut env: Vec<(Field, ValueId)> = map.into_iter().collect();
        env.sort_unstable_by_key(|e| e.0.0.raw());
        let mut absent = self.absent.clone();
        absent.sort_unstable_by_key(|f| f.0.raw());
        absent.dedup();
        if env != self.env_pushed {
            self.eng.set_environment(env.iter().copied());
            self.env_calls += 1;
            self.env_pushed = env;
        }
        if absent != self.absent_pushed {
            self.eng.set_environment_absent(absent.iter().copied());
            self.env_calls += 1;
            self.absent_pushed = absent;
        }
    }

    /// The environment: the files supplied, and the loop-back files.
    fn environment(&self, fixpoint: bool) -> Vec<(Field, ValueId)> {
        let v = self.eng.values();
        let mut env = self.files.clone();
        env.push((v.field(DOC_MARKS), self.marks.value(v)));
        // (the files the job writes kept in memory, a fixpoint's; none
        // otherwise: `UNIT`, never a wait on the client. Listed: the files
        // written before that the last evaluation did not open, each
        // published absent; a file the runs open is placed from its
        // stream, so learning its name after the evaluation changes
        // nothing here)
        if fixpoint {
            let names = self
                .written
                .iter()
                .filter(|n| !self.opened_last.contains(n))
                .map(|n| v.str(n))
                .collect();
            env.push((v.field("loopfiles"), v.list(names)));
        } else {
            env.push((v.field("loopfiles"), ValueId::UNIT));
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
        let tm = crate::exec::prof_flag!("PHITEX_TEX_TIME");
        debug_nodes(&self.eng, "before the evaluation");
        let (mut t_prov, mut t_eval, mut t_after) = (
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
        );
        let mut t_mark = std::time::Instant::now();
        loop {
            t_after += t_mark.elapsed();
            let tp = std::time::Instant::now();
            self.provide(finder);
            t_prov += tp.elapsed();
            // (the comments of a text applied before this evaluation)
            if self.marks.dirty() {
                let env = self.environment(fixpoint);
                env_trace(&self.eng, &env);
                self.push_env(env);
            }
            crate::timeline::lap("engine: the provider, the comments' environment");
            // (the finishers' table as the evaluation begins: what the
            // routines' guesses read, [`crate::run::Ops::fin_defs_start`])
            {
                let o = &self.ops;
                let fd = o
                    .fin_defs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                *o.fin_defs_start
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Arc::new(fd);
                let sg = o
                    .scan_guess
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                *o.scan_guess_start
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Arc::new(sg);
            }
            let t0 = std::time::Instant::now();
            let mut ex = crate::exec::Threads::new(self.threads).watched(&self.watch);
            let c0 = crate::exec::clock().now();
            let r = match self
                .eng
                .evaluate_absorbing(&mut *self.sched, &mut ex, edits)
            {
                Ok(r) => r,
                Err(e) => {
                    // the build ends here; the error is a diagnostic
                    let m = self.diagnose(&e, unit);
                    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                        eprintln!("{m}");
                    }
                    // (what the evaluation did before it ended counts: its
                    // runs, its wall and busy time, the round; a deadline's
                    // stats were all zero, as if nothing had run)
                    if let phitex_defuse::EvalError::Deadline { runs, .. } = &e {
                        stats.runs += *runs;
                    }
                    stats.rounds += 1;
                    stats.add_exec(&ex, c0, &self.eng);
                    stats.live = self.eng.live_nodes();
                    self.eval_errors.push(m);
                    stats.add_hints(&self.eng, unit, &self.eval_errors);
                    return;
                }
            };
            t_eval += t0.elapsed();
            // (latexmk's `max_repeat` reached: the output is the last
            // round's, and the fields still moving are said)
            self.eval_warnings.clear();
            if let Some(fs) = &r.capped {
                let v = self.eng.values();
                let mut names: Vec<String> = fs.iter().map(|&f| field_name(v, f)).collect();
                names.sort();
                self.eval_warnings.push(format!(
                    "the files read back still changed after {} rounds (latexmk's limit); the output is the last round's; still changing: {}",
                    r.rounds,
                    names.join(", ")
                ));
            }
            for (k, d, n) in ex.by_kind() {
                if n > 0 {
                    crate::timeline::note(&format!("runs: {k} ×{n} (summed over threads)"), d);
                }
            }
            for (i, t) in r.round_times.iter().enumerate() {
                let ns = |x: u64| std::time::Duration::from_nanos(x);
                crate::timeline::note(&format!("round {i}: wall"), ns(t.wall_ns));
                crate::timeline::note(&format!("round {i}: commit (summed)"), ns(t.commit_ns));
                crate::timeline::note(&format!("round {i}: check (summed)"), ns(t.check_ns));
            }
            {
                let rounds: u64 = r.round_times.iter().map(|t| t.wall_ns).sum();
                crate::timeline::note(
                    "the evaluation outside its rounds (its end: the engine's watcher's sleep)",
                    t0.elapsed()
                        .saturating_sub(std::time::Duration::from_nanos(rounds)),
                );
            }
            crate::timeline::lap("engine: evaluate (the runs, their commits)");
            t_mark = std::time::Instant::now();
            if tm {
                // (the scans a stable field's definition made through its
                // readers, `STABLE_FIELDS`: about none, or the list is wrong)
                eprintln!("stable fields: {} scans", r.stable_scans);
            }
            crate::timeline::lap("engine: the evaluation's report");
            if crate::exec::obs_flag!("PHITEX_CHECK_READS") {
                // (debug: every committed node's reads checked again
                // against the store after this evaluation; `=fail`: a
                // stale one fails, the tests' setting)
                eprintln!(
                    "check_reads: counters claims_refuted {} effects_refuted {} claim_siblings {} defs_refuted {}",
                    r.claims_refuted, r.effects_refuted, r.claim_siblings, r.defs_refuted
                );
                let tc = std::time::Instant::now();
                let lines = self.eng.check_reads();
                for l in &lines {
                    eprintln!("check_reads: {l}");
                }
                // (the check's own time: the gate's walls include it)
                eprintln!(
                    "check_reads: {} stale (checked in {:?})",
                    lines.len(),
                    tc.elapsed()
                );
                assert!(
                    lines.is_empty() || crate::exec::obs_str!("PHITEX_CHECK_READS") != Some("fail"),
                    "{} committed reads no longer hold",
                    lines.len()
                );
                crate::timeline::lap("engine: the reads checked again (PHITEX_CHECK_READS only)");
            }
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                print_round(t0.elapsed(), &r);
                crate::exec::flush_later();
                print_accounting(&self.eng, &r, &ex, self.env_calls);
                {
                    // (the reruns by cause and field, most first)
                    use phitex_defuse::Rerun as Rr;
                    let v = self.eng.values();
                    let mut by = FxHashMap::<String, usize>::default();
                    let mut shown = 0;
                    if crate::exec::obs_flag!("PHITEX_RERUN_OPS") {
                        // (each node run again: its operation, why, the field by name)
                        for (n, c) in &r.rerun_causes {
                            let f = match *c {
                                Rr::Changed { field, .. }
                                | Rr::Rebind { field }
                                | Rr::Loop { field }
                                | Rr::Environment { field }
                                | Rr::Refuted { field: Some(field) }
                                | Rr::Woken { field: Some(field) }
                                | Rr::Range { field: Some(field) } => {
                                    crate::val::field_name(v, field)
                                }
                                _ => String::new(),
                            };
                            let d: String = describe(&self.eng, *n).chars().take(60).collect();
                            eprintln!("rerunop: {} [{d}] {c:?} {f}", n.raw());
                        }
                    }
                    for (n, c) in &r.rerun_causes {
                        if let Rr::Changed { field, old, new } = *c
                            && crate::exec::obs_str!("PHITEX_RERUN_DIFF")
                                .and_then(|n| n.parse::<usize>().ok())
                                .is_some_and(|n| shown < n)
                            && crate::val::field_name(v, field) == "ps"
                        {
                            shown += 1;
                            let (o, w) = (v.render(old), v.render(new));
                            let i = o.bytes().zip(w.bytes()).take_while(|(a, b)| a == b).count();
                            let cut = |x: &str| {
                                x.get(i.saturating_sub(80)..(i + 240).min(x.len()))
                                    .unwrap_or("")
                                    .to_owned()
                            };
                            eprintln!(
                                "  changed ps at {} (len {} vs {}, first diff at {i}):\n    old ..{}\n    new ..{}",
                                describe(&self.eng, *n),
                                o.len(),
                                w.len(),
                                cut(&o),
                                cut(&w)
                            );
                            if let Some((path, a, b)) = structural_diff(v, old, new, String::new())
                            {
                                eprintln!("    leaf diff at {path}: old {a} | new {b}");
                            }
                        }
                        let (k, f) = match *c {
                            Rr::Changed { field, .. } => ("Changed", Some(field)),
                            Rr::Rebind { field } => ("Rebind", Some(field)),
                            Rr::Refuted { field } => ("Refuted", field),
                            Rr::Woken { field } => ("Woken", field),
                            _ => ("other", None),
                        };
                        let name =
                            f.map_or_else(|| "-".to_owned(), |f| crate::val::field_name(v, f));
                        *by.entry(format!("{k} {name}")).or_default() += 1;
                    }
                    if let Some(list) = crate::exec::obs_str!("PHITEX_DEBUG_NODES") {
                        let ids: Vec<u64> = list
                            .split(',')
                            .filter_map(|s| s.trim().parse().ok())
                            .collect();
                        for (n, c) in &r.rerun_causes {
                            if ids.contains(&n.raw()) {
                                let f = match *c {
                                    Rr::Changed { field, .. }
                                    | Rr::Rebind { field }
                                    | Rr::Environment { field } => crate::val::field_name(v, field),
                                    _ => String::new(),
                                };
                                eprintln!("  rerun cause of {}: {c:?} ({f})", n.raw());
                            }
                        }
                        eprintln!("  restarted by {:?}", r.restarted_by);
                    }
                    let mut by: Vec<_> = by.into_iter().collect();
                    by.sort_by_key(|a| std::cmp::Reverse(a.1));
                    by.truncate(12);
                    eprintln!("  reruns by field {by:?}");
                }
                eprintln!("  waits by kind {:?}", r.waits_by_kind);
                for &(n, w) in &r.top_waited {
                    eprintln!("  waited on {w}x: {}", describe(&self.eng, n));
                }
            }
            crate::timeline::lap("engine: the accounting printed (PHITEX_TEX_TIME only)");
            stats.add(&r);
            stats.env_calls = self.env_calls;
            stats.add_reruns(&self.eng, &r);
            stats.add_refuted(&self.eng, &r);
            stats.add_exec(&ex, c0, &self.eng);
            stats.add_hints(&self.eng, unit, &self.eval_errors);
            crate::timeline::lap("engine: the evaluation's stats");
            let opened = self.opened(unit);
            let mut asked = false;
            // (a file written before that no run opens any more: listed
            // in `loopfiles` from now on, its node publishes it absent;
            // an evaluation more where that happens)
            let dropped = self
                .opened_last
                .iter()
                .any(|n| !opened.contains(n) && self.written.contains(n));
            self.opened_last.clone_from(&opened);
            if fixpoint && dropped {
                asked = true;
            }
            // (`PHITEX_TEX_TIME`: why the build evaluates again)
            let ask_why = |why: &str| {
                if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                    eprintln!("  evaluate again: {why}");
                }
            };
            // a file supplied from the finder that the job turns out to
            // write is read back instead
            // (every file a run opened for writing is the job's: in a
            // fixpoint the run declared its loop-back and its readers ran
            // again on it within the evaluation, `Run::written_decl`; a
            // file read from the finder before is read again only where
            // there is no fixpoint, as absent)
            // (what the provider answered counts as supplied: a file it
            // found is read again as absent; one it did not find was read
            // as absent already)
            self.absorb_provided();
            for name in &opened {
                if self.written.contains(name) {
                    continue;
                }
                let f = self.eng.values().field(&format!("file:{name}"));
                let supplied = self
                    .files
                    .iter()
                    .any(|&(g, x)| g == f && !list_of(self.eng.values(), x).is_empty());
                self.written_file(name, fixpoint, finder);
                if supplied && !fixpoint {
                    asked = true;
                    ask_why(&format!("supplied file written: {name}"));
                }
            }
            // the fields a job-written file's read-back defines: their
            // `wdef:<field>` loop-backs declared
            if fixpoint {
                let v = self.eng.values();
                let acc = |n: &str| self.eng.acc_value(unit, v.field(n));
                let defs = written_defs(v, acc("streams"));
                let mut new = Vec::new();
                for (name, ds) in defs {
                    for (f, _) in ds {
                        let f = wdef_id(&name, &f);
                        if !self.wdefs.contains(&f) && !new.contains(&f) {
                            new.push(f);
                        }
                    }
                }
                for f in new {
                    let none = self.eng.values().list(Vec::new());
                    let w = self.eng.values().field(&format!("wdef:{f}"));
                    {
                        decl_trace(&self.eng, w, none);
                        self.eng.declare_loopback(w, none);
                    }
                    // (declared by the files node within the
                    // evaluation, its readers run again there: no
                    // evaluation more)
                    self.wdefs.push(f);
                }
            }
            let asked_names: Vec<String> = {
                let v = self.eng.values();
                r.env_waits
                    .iter()
                    .filter_map(|&(_, f)| field_name(v, f).strip_prefix("file:").map(str::to_owned))
                    .collect()
            };
            // (a file asked for as units, [`file_units`], with no
            // provider: cut here)
            let asked_units: Vec<String> = {
                let v = self.eng.values();
                let mut ns: Vec<String> = r
                    .env_waits
                    .iter()
                    .filter_map(|&(_, f)| units_file(&field_name(v, f)).map(str::to_owned))
                    .collect();
                ns.sort_unstable();
                ns.dedup();
                ns
            };
            for name in asked_units {
                let uf = self.eng.values().field(&format!("units:{name}"));
                if self.files.iter().any(|&(g, _)| g == uf)
                    || self.absent.contains(&uf)
                    || self.written.contains(&name)
                {
                    continue;
                }
                let defs = self
                    .ops
                    .defs
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let found = finder(&name);
                if !self.reads.iter().any(|(n, _)| *n == name) {
                    self.queries.push((name.clone(), found.is_some()));
                    self.reads.push((name.clone(), file_hash(found.as_ref())));
                }
                match found {
                    Some((path, bytes)) => {
                        let es = file_units(
                            self.eng.values(),
                            &self.ops.hints,
                            &defs,
                            &name,
                            &path,
                            &bytes,
                        );
                        self.files.extend(es);
                    }
                    None => self.absent.push(uf),
                }
                asked = true;
                ask_why(&format!("units supplied: {name}"));
            }
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
                    self.written_file(&name, fixpoint, finder);
                    asked = true;
                    ask_why(&format!("opened file: {name}"));
                    continue;
                }
                self.supply(finder, &name);
                asked = true;
                ask_why(&format!("file supplied: {name}"));
            }
            // what the provider answered in place joins the supplied
            // files (an edit of one replaces it, [`Build::edit_file`]),
            // whether or not this round asked for more
            self.absorb_provided();
            // (an edit taken during the evaluation changed the comments:
            // the environment holds them before it ends)
            asked |= self.marks.dirty();
            if !asked {
                t_after += t_mark.elapsed();
                break;
            }
            let env = self.environment(fixpoint);
            let env: Vec<(Field, ValueId)> = (env).into_iter().collect();
            env_trace(&self.eng, &env);
            self.push_env(env);
        }
        let v = self.eng.values();
        if let Some(t) = self.eng.acc_value(unit, v.field("ifviol")) {
            stats.if_depth_violations += u64::try_from(v.seq_to_vec(t).len()).unwrap_or(0);
        }
        crate::timeline::lap("engine: after the rounds (files, loop-backs, stats)");
        if tm {
            eprintln!(
                "settle phases: provider {t_prov:?}, evaluate (rounds, the writer's nodes among them) {t_eval:?}, after the rounds (files, loop-backs, stats) {t_after:?}"
            );
        }
        debug_nodes(&self.eng, "after the evaluation");
    }

    /// The document's nodes pushed onto `unit`, with its units kept for
    /// its edits.
    fn push_document(
        &mut self,
        unit: UnitId,
        text: &str,
        defs: Defs,
        doc: bool,
    ) -> (Vec<(u64, NodeId, Src)>, Incr) {
        let mut incr = Incr::new(defs);
        incr.threads = self.threads;
        let nodes = self.push_text(unit, &as_file(text), doc.then_some(&mut incr));
        (nodes, incr)
    }

    /// The text's nodes pushed onto `unit`: a document's (`incr`) its
    /// paragraph regions (an `\\input` file's are its run's, split where
    /// the run reaches it, `Run::leftover`); a format's one node.
    fn push_text(
        &mut self,
        unit: UnitId,
        text: &str,
        mut incr: Option<&mut Incr>,
    ) -> Vec<(u64, NodeId, Src)> {
        let t0 = std::time::Instant::now();
        if let Some(inc) = incr.as_deref_mut() {
            inc.set_text(text);
        }
        crate::timeline::lap("hints: the units and the hint pass");
        let incr = incr.as_deref();
        // a format's text is one node: a sequential run over the whole
        // file, its `\input`s inline; what it may define is unknowable
        // from its parts, so anything (it runs once, serially, cached)
        let nodes: Vec<(u64, DocNode)> = match incr {
            Some(inc) => {
                register_doc(&self.ops.hints, self.eng.values(), inc);
                doc_nodes(text, inc)
            }
            None if text.is_empty() => Vec::new(),
            None => vec![(key(text, true), DocNode::Doc(0, text.len()))],
        };
        let t1 = t0.elapsed();
        crate::timeline::lap("push: the nodes' slices and keys");
        // (the nodes' arguments made on the pool: values are named by
        // their content, whichever worker makes them)
        let args = {
            let v = self.eng.values();
            let loads = self.loads.as_deref();
            let reg = &*self.ops.hints;
            let mut ex = crate::exec::Threads::new(self.threads);
            crate::exec::map_on(&mut ex, nodes.len(), &|k| {
                let a = node_args(v, text, incr, &nodes[k].1, loads);
                crate::hints::prepare(reg, v, &a);
                a
            })
        };
        let t2 = t0.elapsed();
        crate::timeline::lap("push: the nodes' arguments (on the pool)");
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
            for (k, _) in &nodes {
                eprintln!("node {k:?}");
            }
        }
        // (one append of every node, in order)
        let ids = self.eng.push_many(unit, self.ops.exec, args);
        crate::hints::prepared_done(&self.ops.hints);
        crate::timeline::lap("push: the engine's push");
        let out: Vec<(u64, NodeId, Src)> = nodes
            .iter()
            .zip(ids)
            .map(|((k, n), id)| (*k, id, Src::of(n)))
            .collect();
        let t3 = t0.elapsed();
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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

/// A file's bytes as TeX's characters, one each (Latin-1): ASCII bytes
/// are their characters' UTF-8 already, copied at once.
fn latin1(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes)
        && s.is_ascii()
    {
        return s.to_owned();
    }
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// The id of file `file`'s read-back definition of field `f`
/// (`wdef:<id>`): per file, as two files that define one field (LaTeX's
/// scratch macros, `\reserved@a`) each give their own value, the last
/// file read the one in force.
/// A key already naming its file (a read-back nested in another file's
/// line, [`crate::run::Run::read_back`]) is its own id.
pub(crate) fn wdef_id(file: &str, f: &str) -> String {
    if f.contains('\u{1}') {
        f.to_owned()
    } else {
        format!("{file}\u{1}{f}")
    }
}

/// The field a read-back key defines (a key naming its file: after it).
pub(crate) fn wdef_field(k: &str) -> &str {
    k.rsplit('\u{1}').next().unwrap_or(k)
}

/// Whether a job-written file's read-back only defines (its lines are
/// read as definitions per field, never as text): `.aux`.
pub(crate) fn defines_only(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("aux"))
}

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
pub(crate) const DOC_NAME: &str = "\u{10FFFE}document";

/// Registers the document's analysis for the hints of nodes made from
/// parts of its slices (`hints::register_text`, by the document's
/// offsets): when one chunk is the whole document. A document of several
/// chunks has no whole analysis; such a node's hint answers `Any` (an
/// analysis registered for an earlier text of the build is withdrawn).
fn register_doc(reg: &crate::hints::Registry, v: &Values, inc: &Incr) {
    if let [c] = inc.chunks.as_slice() {
        crate::hints::register_text(reg, v.str(DOC_NAME), &c.th);
    } else {
        crate::hints::unregister_text(reg, v.str(DOC_NAME));
    }
}

/// The fields the engine is told are stable (`Engine::set_stable`):
/// read by more than half of para4000's nodes and defined only by the
/// format or the job's environment (`PHITEX_READERS=1` counted them:
/// 4,000 to 5,204 readers of 6,677 nodes, every read from the
/// environment). A wrong one costs speed, never bytes.
const STABLE_FIELDS: &[&str] = &[
    "cattab",
    "sftab",
    "lctab",
    "patterns",
    "font",
    "fontinfo:1",
    "hc:1",
    "fd:1:2",
    "fd:1:3",
    "fd:1:4",
    "fd:1:7",
    "cs:par",
    "param:everypar",
];

/// A document node's text: a slice of the document (its bytes).
enum DocNode {
    Doc(usize, usize),
}

/// The document's nodes, keyed: per chunk of its units, its slices. (An
/// `\\input` file's regions are made where a run reaches the file,
/// whatever route reaches it: `Run::leftover`.)
fn doc_nodes(text: &str, inc: &Incr) -> Vec<(u64, DocNode)> {
    let mut out = Vec::new();
    for c in &inc.chunks {
        let slices =
            c.th.slices
                .iter()
                .zip(c.keys.iter())
                .map(|(&(s, e), &k)| (c.start + s, c.start + e, k));
        // (the chunk's keys, made when it was analysed; a slice at its
        // start not beginning a line keyed again)
        out.extend(slices.map(|(s, e, k)| {
            let k = if s == c.start && s > 0 && text.as_bytes()[s - 1] != b'\n' {
                key(&text[s..e], false)
            } else {
                k
            };
            (k, DocNode::Doc(s, e))
        }));
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
}

impl Src {
    fn of(n: &DocNode) -> Src {
        match n {
            &DocNode::Doc(s, e) => Src::Doc(s, e),
        }
    }

    /// Its text, the document's being `doc`.
    fn text<'a>(&'a self, doc: &'a str) -> &'a str {
        match self {
            &Src::Doc(s, e) => doc.get(s..e.min(doc.len())).unwrap_or(""),
        }
    }
}

/// A node's identity for edits: the hash of its text and whether it
/// begins a line.
pub(crate) fn key(piece: &str, starts: bool) -> u64 {
    phitex_defuse::hash_bytes(1 + u64::from(starts), piece.as_bytes())
}

/// The environment field of the canonical text's comments
/// ([`canonical_text`]): `[[line, comment], ...]`.
pub(crate) const DOC_MARKS: &str = "doc:marks";

/// The comments of the document's canonical text by line, shared with
/// the edits a running evaluation takes ([`Absorb`]); `dirty` while the
/// environment does not hold them yet.
#[derive(Clone, Default)]
pub(crate) struct Marks(Arc<Mutex<MarksState>>);

/// [`Build`]'s memo of its output and pages, by the values they come
/// from.
#[derive(Default)]
struct OutMemo {
    #[allow(clippy::type_complexity)]
    output: Option<(
        (Option<ValueId>, Option<ValueId>, Option<ValueId>, usize),
        (Arc<Vec<u8>>, String),
    )>,
    pages: Option<(Option<ValueId>, Vec<u64>)>,
    log: crate::log::LayoutMemo,
}

/// The comments by line, and whether the environment lacks them.
type MarksState = (Vec<(u32, String)>, bool);

impl Marks {
    fn get(&self) -> Vec<(u32, String)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .clone()
    }

    fn set(&self, ms: Vec<(u32, String)>) {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if m.0 != ms {
            *m = (ms, true);
        }
    }

    /// The `doc:marks` value; the environment holds them from here.
    fn value(&self, v: &Values) -> ValueId {
        let mut m = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        m.1 = false;
        v.list(
            m.0.iter()
                .map(|(n, c)| v.list(vec![v.int(i64::from(*n)), v.str(c)]))
                .collect(),
        )
    }

    /// Whether marks changed since the environment was set.
    fn dirty(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).1
    }
}

/// The document's text as its tokens need it (spec-redesign §8: a unit's
/// value is its tokens, not its text): each comment's text after its
/// `%` replaced by one NUL, the comment's mark, and the comments by line
/// (1-based). Under the catcodes a document is read with (`\\` an
/// escape, `%` a comment) the tokens are the same, and a comment-only
/// edit changes no node. A run that reads a mark as text (`%` no
/// comment there, a verbatim's) puts the comment back in its line from
/// `doc:marks` (`Run::splice_mark`): the tokens are TeX's either way.
/// `None` for a text this cannot mark (one holding NUL, or `^^`, which
/// can spell an escape or a `%`).
pub(crate) fn canonical_text(text: &str) -> Option<(String, Vec<(u32, String)>)> {
    if has_raw_marks(text) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut marks = Vec::new();
    canonical_lines(text, 0, &mut out, &mut marks);
    Some((out, marks))
}

/// Whether `text` has what [`canonical_text`] cannot stand for (a NUL,
/// `^^`).
fn has_raw_marks(text: &str) -> bool {
    text.contains('\0') || text.contains("^^")
}

/// The line ends among eight bytes `w` (little-endian): the top bit of
/// each such byte set, exactly.
fn nl_mask(w: [u8; 8]) -> u64 {
    let x = u64::from_le_bytes(w) ^ 0x0a0a_0a0a_0a0a_0a0a;
    let t = (x & 0x7f7f_7f7f_7f7f_7f7f).wrapping_add(0x7f7f_7f7f_7f7f_7f7f);
    !(t | x | 0x7f7f_7f7f_7f7f_7f7f)
}

/// The number of line ends in `b`, eight bytes at a time.
fn newlines(b: &[u8]) -> usize {
    let (words, rest) = b.as_chunks::<8>();
    let n: usize = words
        .iter()
        .map(|w| nl_mask(*w).count_ones() as usize)
        .sum();
    n + rest.iter().map(|&c| usize::from(c == b'\n')).sum::<usize>()
}

/// Where the `k`-th line after byte `from` of `b` begins (past its `k`
/// line ends; `b.len()` if there are fewer), eight bytes at a time.
fn skip_lines(b: &[u8], from: usize, mut k: usize) -> usize {
    if k == 0 {
        return from;
    }
    let (words, rest) = b[from..].as_chunks::<8>();
    for (i, w) in words.iter().enumerate() {
        let mut m = nl_mask(*w);
        let c = m.count_ones() as usize;
        if c < k {
            k -= c;
            continue;
        }
        while k > 1 {
            m &= m - 1;
            k -= 1;
        }
        return from + i * 8 + (m.trailing_zeros() / 8) as usize + 1;
    }
    let base = from + words.len() * 8;
    for (i, &c) in rest.iter().enumerate() {
        if c == b'\n' {
            k -= 1;
            if k == 0 {
                return base + i + 1;
            }
        }
    }
    b.len()
}

/// The starts of `s`'s lines, eight bytes at a time.
fn line_starts(s: &str) -> Vec<usize> {
    let b = s.as_bytes();
    let mut v = vec![0];
    let (words, rest) = b.as_chunks::<8>();
    for (i, w) in words.iter().enumerate() {
        let mut m = nl_mask(*w);
        while m != 0 {
            v.push(i * 8 + (m.trailing_zeros() / 8) as usize + 1);
            m &= m - 1;
        }
    }
    let base = words.len() * 8;
    v.extend(
        rest.iter()
            .enumerate()
            .filter(|c| *c.1 == b'\n')
            .map(|c| base + c.0 + 1),
    );
    v
}

/// [`canonical_text`] of `new`, made in `canon` from that of `old` (the
/// text `canon` and its `marks` are made from): the lines where the
/// texts differ made again in place, the rest kept (each line's
/// canonical form is its own; the marks after them renumbered). The new
/// marks, and the range of `canon` replaced, `(start, old end, new end)`.
fn canonical_splice(
    old: &str,
    canon: &mut String,
    marks: &[(u32, String)],
    new: &str,
    ends: Option<(usize, usize)>,
) -> (Vec<(u32, String)>, (usize, usize, usize)) {
    let (o, n) = (old.as_bytes(), new.as_bytes());
    let (p, s) = ends.unwrap_or_else(|| byte_ends(o, n));
    canonical_lines_splice(old, canon, marks, new, p, s)
}

/// The lengths of the prefix and the suffix `o` and `n` share (not
/// overlapping), compared a block at a time.
#[must_use]
pub fn byte_ends(o: &[u8], n: &[u8]) -> (usize, usize) {
    const B: usize = 4096;
    // (the bytes both begin and end with, a block at a time)
    let mut p = 0;
    let m = o.len().min(n.len());
    while p + B <= m && o[p..p + B] == n[p..p + B] {
        p += B;
    }
    while p < m && o[p] == n[p] {
        p += 1;
    }
    let mut s = 0;
    let room = m - p;
    while s + B <= room && o[o.len() - s - B..o.len() - s] == n[n.len() - s - B..n.len() - s] {
        s += B;
    }
    while s < room && o[o.len() - 1 - s] == n[n.len() - 1 - s] {
        s += 1;
    }
    (p, s)
}

/// [`canonical_splice`] where `old` and `new` share `p` bytes at their
/// start and `s` at their end.
fn canonical_lines_splice(
    old: &str,
    canon: &mut String,
    marks: &[(u32, String)],
    new: &str,
    p: usize,
    s: usize,
) -> (Vec<(u32, String)>, (usize, usize, usize)) {
    let (o, n) = (old.as_bytes(), new.as_bytes());
    // (whole lines: from the start of the line the first difference is
    // on to the end of the line the last one is on)
    let t0 = std::time::Instant::now();
    let ls = n[..p]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |k| k + 1);
    let q = n.len() - s;
    let le = if q == n.len() || (q > 0 && n[q - 1] == b'\n') {
        q
    } else {
        n[q..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(n.len(), |k| q + k + 1)
    };
    let le_old = le + o.len() - n.len();
    let old_lines = newlines(&o[ls..le_old]);
    // (those lines in the canonical text: the same lines, by number;
    // without comments it is the text itself)
    let (first, cs, ce) = if marks.is_empty() {
        (None, ls, le_old)
    } else {
        let first = newlines(&o[..ls]);
        let cb = canon.as_bytes();
        let cs = skip_lines(cb, 0, first);
        let ce = if le_old == o.len() {
            cb.len()
        } else {
            skip_lines(cb, cs, old_lines)
        };
        (Some(first), cs, ce)
    };
    crate::timeline::note(
        &format!("canonical: the lines found ({} bytes)", le - ls),
        t0.elapsed(),
    );
    let t0 = std::time::Instant::now();
    let mut out = String::with_capacity(le - ls + 16);
    let mut mid = Vec::new();
    canonical_lines(&new[ls..le], 0, &mut out, &mut mid);
    // (the changed lines' marks numbered from their first line's)
    let first = match first {
        Some(f) => f,
        None if mid.is_empty() => 0,
        None => newlines(&o[..ls]),
    };
    let lift = u32::try_from(first).unwrap_or(u32::MAX);
    for m in &mut mid {
        m.0 = m.0.saturating_add(lift);
    }
    canon.replace_range(cs..ce, &out);
    crate::timeline::note("canonical: spliced in place", t0.elapsed());
    // (the marks: before the lines, theirs made again, after them moved)
    let new_lines = newlines(&n[ls..le]);
    let lo = u32::try_from(first).unwrap_or(u32::MAX);
    let hi = u32::try_from(first + old_lines).unwrap_or(u32::MAX);
    let shift = |l: u32| -> u32 {
        let l = i64::from(l) + i64::try_from(new_lines).unwrap_or(0)
            - i64::try_from(old_lines).unwrap_or(0);
        u32::try_from(l).unwrap_or(u32::MAX)
    };
    let mut ms: Vec<(u32, String)> = marks.iter().filter(|m| m.0 <= lo).cloned().collect();
    ms.extend(mid);
    ms.extend(
        marks
            .iter()
            .filter(|m| m.0 > hi && le_old < o.len())
            .map(|(l, c)| (shift(*l), c.clone())),
    );
    (ms, (cs, ce, cs + out.len()))
}

/// [`canonical_text`]'s lines of `text`, the first numbered `first`
/// (from 0), onto `out` and `marks`.
fn canonical_lines(text: &str, first: usize, out: &mut String, marks: &mut Vec<(u32, String)>) {
    // (only a line with a `%` can change: the lines between two such
    // lines are copied at once, their ends counted, eight bytes at a time)
    let b = text.as_bytes();
    let (mut done, mut n, mut from) = (0, first, 0);
    while let Some(p) = text[from..].find('%').map(|k| from + k) {
        let ls = text[done..p].rfind('\n').map_or(done, |k| done + k + 1);
        n += newlines(&b[done..ls]);
        out.push_str(&text[done..ls]);
        let le = text[p..].find('\n').map_or(text.len(), |k| p + k);
        canonical_line(&text[ls..le], n, out, marks);
        if le < text.len() {
            out.push('\n');
            n += 1;
            done = le + 1;
        } else {
            done = le;
        }
        from = done;
    }
    out.push_str(&text[done..]);
}

/// [`canonical_lines`] of one line `body` (its end not in it), line `n`
/// (0-based) of the text.
fn canonical_line(body: &str, n: usize, out: &mut String, marks: &mut Vec<(u32, String)>) {
    let b = body.as_bytes();
    let mut i = 0;
    let mut cut = None;
    while i < b.len() {
        match b[i] {
            // (a control symbol's character, or a control word's first
            // letter: never a comment)
            b'\\' => i += 2,
            b'%' => {
                cut = Some(i);
                break;
            }
            _ => i += 1,
        }
    }
    match cut {
        Some(k) if k + 1 < body.len() => {
            out.push_str(&body[..=k]);
            out.push('\0');
            // (the comment as web2c's `input_line` keeps it: the
            // line's end, then its trailing blanks removed, §31)
            let c = body[k + 1..]
                .trim_end_matches(['\r', '\n'])
                .trim_end_matches([' ', '\t']);
            marks.push((u32::try_from(n + 1).unwrap_or(u32::MAX), c.to_owned()));
        }
        _ => out.push_str(body),
    }
}

/// A text as TeX reads a file: every line ends (§31).
fn as_file(text: &str) -> std::borrow::Cow<'_, str> {
    if text.is_empty() || text.ends_with('\n') {
        std::borrow::Cow::Borrowed(text)
    } else {
        std::borrow::Cow::Owned(format!("{text}\n"))
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
/// load order (the format's recorded reads, in the order it read them):
/// the format's macros, which the hints bound a document's calls with
/// ([`crate::hints`]). Harvested once per prelude key (a text pass, no
/// evaluation), for a prelude evaluated here or imported from bytes.
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
    }
    for (k, (name, h)) in c.reads.iter().enumerate() {
        if *h == 0 || !is_tex_text(name) {
            continue;
        }
        let Some((_, bytes)) = finder(name) else {
            continue;
        };
        let text = latin1(&bytes);
        let file = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let kind = crate::hints::kind_of(&text, pass::FileKind::from_path(name));
        defs.harvest(&text, kind, file);
        order.push(file);
    }
    defs.set_load_order(&order);
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") && opts.prelude_bytes.is_some() {
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
        // (the files found, read and hashed on the pool)
        let mut ex = crate::exec::Coarse(crate::exec::Threads::new(crate::exec::threads(
            opts.threads,
        )));
        let fresh = crate::exec::map_on(&mut ex, c.reads.len(), &|k| {
            let (n, h) = &c.reads[k];
            file_hash(finder(n).as_ref()) == *h
        })
        .into_iter()
        .all(|x| x);
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
    let time = crate::exec::prof_flag!("PHITEX_TEX_TIME");
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
    g.ops.fin_predict.store(
        opts.finisher_predictions,
        std::sync::atomic::Ordering::Relaxed,
    );
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
        let env: Vec<(Field, ValueId)> = (env).into_iter().collect();
        env_trace(&g.eng, &env);
        g.push_env(env);
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
        g.push_text(unit, &as_file(p), None);
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
        // (TeX's errors only: the engine's own checks are not the format's)
        if int(0) != 1 {
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
                if [
                    "wopen:",
                    "write:",
                    "file:",
                    "read:",
                    "pdf:formfonts",
                    "pdf:imagefiles",
                ]
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
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
    /// TeX's error (`! ...` in the log, in TeX's error count).
    Error,
    Warning,
    /// The engine's own check (a hint's regime violated, an undeclared
    /// definition): not TeX's error, counted apart; it decides neither
    /// the output nor the exit status.
    Diagnostic,
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
    /// The first node after the text's (the writer): new text nodes go
    /// before it, also when the text has none left.
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
    /// when a file sliced into it changed): its canonical text
    /// ([`canonical_text`]) while `canon`.
    text: String,
    /// Whether `text` is the document's canonical text.
    canon: bool,
    /// The text `text` is the canonical text of, while `canon`.
    raw: Option<Arc<str>>,
    /// The output and the pages as last asked, by the values they come
    /// from (asked again with those unchanged: given again, not made).
    memo: Mutex<OutMemo>,
    /// `text`'s line starts, made when a diagnostic asks and kept
    /// through edits (`Absorb` moves them).
    lines: Mutex<Option<Vec<usize>>>,
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
        crate::timeline::lap("engine: the format loaded (its environment)");
        let env = cached.env.clone();
        let defs = prelude_defs(&cached, prelude, opts.prelude_finder());
        let t1 = t0.elapsed();
        crate::timeline::lap("engine: the format's definitions");
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: prelude env (format load or build) {t_env:?}, prelude definitions {:?}",
                t1.checked_sub(t_env).unwrap_or_default()
            );
        }
        let mut g = Graph::new(opts.time_limit, opts.threads);
        g.ops.fin_predict.store(
            opts.finisher_predictions,
            std::sync::atomic::Ordering::Relaxed,
        );
        g.eng.set_max_rounds(opts.max_rounds);
        // (latexmk's `max_repeat`: the document finished on its last
        // round's values, the fields still moving a warning)
        g.eng
            .set_round_cap_policy(phitex_defuse::CapPolicy::Finalize);
        (*g.ops.out_ok.lock().unwrap_or_else(PoisonError::into_inner))
            .clone_from(&opts.out_name_ok);
        g.ops
            .tool_opts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bibtex
            .clone_from(&opts.bibtex);
        g.tool_finder.clone_from(&opts.tool_finder);
        g.sched = opts.speculation.scheduler(opts.speculation_budget);
        if let Some(p) = &opts.progress {
            g.watch = crate::exec::Watch::new(g.threads, Some(p.clone()));
        }
        let unit = g.eng.new_unit();
        crate::timeline::lap("engine: the graph made");
        // the files the format read (fonts, data) are supplied before the
        // document's first evaluation: a run reading one never stops
        let t_sup = std::time::Instant::now();
        g.supply_all(
            &opts.finder,
            &cached
                .reads
                .iter()
                .map(|(n, _)| n.clone())
                .collect::<Vec<_>>(),
        );
        crate::timeline::lap("engine: the format's files supplied (fonts, data)");
        g.guesses(&opts);
        crate::timeline::lap("engine: the guesses (read-back keys)");
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: supply the format's {} files (fonts, data) {:?}",
                cached.reads.len(),
                t_sup.elapsed()
            );
        }
        // (a document's nodes hold its canonical text, [`canonical_text`],
        // unless it has none: a comment-only edit changes no node)
        let canon_text = (is_doc(prelude, document))
            .then(|| canonical_text(document))
            .flatten();
        let g_canon = canon_text.is_some();
        let text = match canon_text {
            Some((t, ms)) => {
                g.marks.set(ms);
                t
            }
            None => document.to_owned(),
        };
        crate::timeline::lap("engine: the canonical text");
        // (the files the job writes and reads back are loop-backs the
        // `\openout` that opens them declares, `Run::written_decl`: a read
        // before it runs again on the loop-back)
        {
            g.absorb_provided();
            let env = g.environment(opts.fixpoint);
            let env: Vec<(Field, ValueId)> = (env).into_iter().collect();
            env_trace(&g.eng, &env);
            g.push_env(env);
        }
        crate::timeline::lap("engine: the environment pushed");
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
        // the package loads' records (`crate::pkgrec`); a load never seen
        // runs optimistically under speculation
        let optimistic = opts.speculation != Speculation::Off;
        let predict = opts.pkg_predict;
        g.loads = Some(Arc::new(crate::pkgrec::LoadHints::new(
            crate::pkgrec::parse(&opts.pkgrec),
            opts.finder.clone(),
            optimistic,
            predict,
        )));
        // (the loads' predictions, asked by the engine per load node)
        g.ops
            .pkg_loads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&g.loads);
        crate::timeline::lap("engine: the job's environment, the loads' records");
        let (nodes, incr) = g.push_document(unit, &text, defs, doc);
        // (the pages: each piece places the finisher of a page that ends
        // in it, `crate::pages`; nothing of them is pushed here)
        // (the files' node, then the link and the end node: new text
        // pieces go before all three; the files' node, whose run gives the
        // loop-backs, before the link and the end node, which run once
        // the loop-backs settle (`final_only`): no range it reads holds
        // them)
        let files_op = g.files_op;
        let files = g.eng.push(unit, files_op, Vec::new());
        g.eng.add_root(files);
        let link = g.eng.push(unit, g.link, Vec::new());
        g.eng.add_root(link);
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        g.eng.add_root(dvi);
        let end = files;
        crate::timeline::lap("engine: the link, end and files nodes");
        // (the fields nearly every node reads and only the format or the
        // job's start defines: their reads kept per unit, not filed by
        // scope, [`STABLE_FIELDS`])
        for f in STABLE_FIELDS {
            let f = g.eng.values().field(f);
            g.eng.set_stable(f);
        }
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
            text,
            canon: g_canon,
            raw: g_canon.then(|| Arc::from(document)),
            memo: Mutex::default(),
            lines: Mutex::default(),
        };
        b.settle();
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "time: prelude {t1:?}, document {:?}",
                t0.elapsed().checked_sub(t1).unwrap()
            );
        }
        b
    }

    /// `PHITEX_READERS=1`: after a settle, the fields most nodes read,
    /// each with its readers, the nodes in all, and where the reads found
    /// it (the environment, or the nodes defining it): the data the
    /// stable fields ([`STABLE_FIELDS`]) are chosen from.
    fn dump_readers(&self) {
        if !crate::exec::obs_flag!("PHITEX_READERS") {
            return;
        }
        let eng = &self.g.eng;
        let mut todo: Vec<NodeId> = self.nodes.iter().map(|(_, n, _)| *n).collect();
        todo.extend([self.end, self.dvi]);
        let mut seen = phitex_defuse::FxHashSet::default();
        // field -> (readers, reads from the environment, definers)
        let mut by: FxHashMap<String, (usize, usize, phitex_defuse::FxHashSet<String>)> =
            FxHashMap::default();
        while let Some(n) = todo.pop() {
            if !seen.insert(n.raw()) {
                continue;
            }
            todo.extend(eng.children(n));
            let mut mine = phitex_defuse::FxHashSet::default();
            for l in eng.debug_reads(n).lines() {
                let Some((name, rest)) = l.split_once(" (") else {
                    continue;
                };
                let from = rest
                    .split_once("): from ")
                    .map_or("?", |(_, r)| r.split(' ').next().unwrap_or("?"));
                let e = by.entry(name.to_owned()).or_default();
                if mine.insert(name.to_owned()) {
                    e.0 += 1;
                }
                if from == "env" {
                    e.1 += 1;
                } else {
                    e.2.insert(from.to_owned());
                }
            }
        }
        let mut rows: Vec<_> = by.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
        for (f, (readers, env, defs)) in rows.iter().take(60) {
            let mut d: Vec<&String> = defs.iter().collect();
            d.sort();
            d.truncate(4);
            eprintln!(
                "readers: {f} read by {readers} of {} nodes; {env} reads from the environment, {} definers {d:?}",
                seen.len(),
                defs.len()
            );
        }
    }

    /// `PHITEX_CLAIMS=<text>`: the debugging dump after a settle.
    fn dump_claims(&self) {
        // `PHITEX_CLAIMS=<text>`: the nodes whose slice holds the text,
        // their claims and their descendants' (a debugging dump)
        if let Some(want) = crate::exec::obs_str!("PHITEX_CLAIMS") {
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
                if src.text(&self.text).contains(want) {
                    dump(eng, v, *n, 0);
                }
            }
        }
    }

    fn settle(&mut self) {
        let t_all = std::time::Instant::now();
        // the text nodes' offsets in the document, for the progress (an
        // edit of a long document keeps the last ones: a place shown
        // while it runs is off by the edit's length at most, and the map
        // is not made again for every edit)
        if self.g.watch.offsets.is_empty() || self.nodes.len() < 4096 {
            let mut offsets = FxHashMap::with_capacity_and_hasher(
                self.nodes.len(),
                std::hash::BuildHasherDefault::default(),
            );
            for (_, n, src) in &self.nodes {
                let Src::Doc(s, _) = *src;
                offsets.insert(n.raw(), u64::try_from(s).unwrap_or(0));
            }
            self.g.watch.offsets = Arc::new(offsets);
        }
        crate::timeline::lap("engine: the nodes' offsets (progress)");
        let mut stats = Stats::default();
        self.g.queries.clear();
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
            hints: self.g.ops.hints.clone(),
            end: self.end,
            incr: self.doc.then_some(&mut self.incr),
            unchanged: None,
            text: &mut self.text,
            raw: &mut self.raw,
            lines: &self.lines,
            canon: &mut self.canon,
            marks: self.g.marks.clone(),
            loads: loads.as_deref(),
        };
        let t_before = t_all.elapsed();
        crate::timeline::lap("engine: the edit's inputs");
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
        crate::timeline::lap("engine: live nodes counted");
        if crate::exec::obs_flag!("PHITEX_RECORD_CHECK") {
            crate::run::record_check(&self.g.eng, &self.g.start_env);
        }
        self.pkg_stats(&mut stats);
        crate::timeline::lap("engine: package load stats");
        // the loads' records as this evaluation left them: a load
        // inserted by a later edit uses them
        if let Some(l) = &self.g.loads {
            l.merge(self.fresh_records());
        }
        crate::timeline::lap("engine: package load records merged");
        self.stats = stats;
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
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
        self.dump_readers();
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
        let _ = self.edits.send(text);
        self.settle();
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!("time: edit {:?}", t0.elapsed());
        }
    }

    /// A file other than the document changed (`None`: as the finder
    /// gives it again): its `file:` field is replaced, and the nodes that
    /// read it run again.
    #[allow(clippy::too_many_lines)]
    pub fn edit_file(&mut self, name: &str, text: Option<&[u8]>) {
        // (the units the provider cut so far: a file read only as units
        // is edited by its units)
        self.g.absorb_units();
        let v = self.g.eng.values();
        let f = v.field(&format!("file:{name}"));
        let uf = v.field(&format!("units:{name}"));
        let unit_path = self
            .g
            .files
            .iter()
            .find(|&&(g, _)| g == uf)
            .and_then(|&(_, x)| list_of(v, x).first().map(|&p| str_of(v, p)));
        let cut = unit_path.is_some() || self.g.absent.contains(&uf);
        let k = if let Some(k) = self.g.files.iter().position(|(g, _)| *g == f) {
            Some(k)
        } else if self.g.absent.contains(&f) {
            self.g.absent.retain(|g| *g != f);
            self.g.files.push((f, v.list(Vec::new())));
            Some(self.g.files.len() - 1)
        } else {
            None
        };
        if k.is_none() && !cut {
            return;
        }
        let old = k.map(|k| list_of(v, self.g.files[k].1)).unwrap_or_default();
        let found = match text {
            Some(t) => Some((
                old.first()
                    .map(|&p| str_of(v, p))
                    .or(unit_path)
                    .unwrap_or_else(|| format!("./{name}")),
                t.to_vec(),
            )),
            None => (self.opts.finder)(name),
        };
        if let Some(k) = k {
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
        }
        // (a file read as units: cut again, each unit's field its new
        // slice; a unit whose text is the same keeps its value)
        if cut {
            self.g.drop_units(name);
            let v = self.g.eng.values();
            let defs = self
                .g
                .ops
                .defs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            match &found {
                Some((path, bytes)) => {
                    let es = file_units(v, &self.g.ops.hints, &defs, name, path, bytes);
                    self.g.files.extend(es);
                }
                None => self.g.absent.push(uf),
            }
        }
        if let Some(r) = self.g.reads.iter_mut().find(|(n, _)| n == name) {
            r.1 = file_hash(found.as_ref());
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
                    Src::Doc(..) => None,
                })
                .collect();
            for (n, args) in redo {
                self.g.eng.replace(n, self.g.ops.exec, args);
            }
        }
        self.g.absorb_provided();
        let env = self.g.environment(self.opts.fixpoint);
        let env: Vec<(Field, ValueId)> = (env).into_iter().collect();
        env_trace(&self.g.eng, &env);
        self.g.push_env(env);
        self.settle();
    }

    /// The document's nodes' texts: each node's raw id, its text (`None`:
    /// the document's, as given to [`Self::edit`]; else the file's text
    /// it is a slice of) and its byte range in it.
    #[must_use]
    pub fn node_spans(&self) -> Vec<(u64, Option<Arc<str>>, usize, usize)> {
        let mut out: Vec<(u64, Option<Arc<str>>, usize, usize)> = self
            .nodes
            .iter()
            .map(|(_, n, src)| match src {
                &Src::Doc(s, e) => (n.raw(), None, s, e),
            })
            .collect();
        // (the nodes a run made of a file's slices or units, under the
        // document's nodes: each in its file's text, at the slice's
        // place, [`Run::doc_file_regions`])
        let v = self.g.eng.values();
        let mut texts: FxHashMap<String, Option<Arc<str>>> = FxHashMap::default();
        let mut seen: phitex_defuse::FxHashSet<u64> = phitex_defuse::FxHashSet::default();
        let mut stack: Vec<NodeId> = self.nodes.iter().map(|(_, n, _)| *n).collect();
        while let Some(x) = stack.pop() {
            if !seen.insert(x.raw()) {
                continue;
            }
            stack.extend(self.g.eng.children(x));
            let Some(i) = self.g.eng.node_info(x) else {
                continue;
            };
            let Some(&fs) = i.args.first() else { continue };
            let mut span: Option<(String, usize, usize)> = None;
            for f in list_of(v, fs) {
                let xs = list_of(v, f);
                let Some(&t) = xs.first() else { continue };
                let (name, s, e) = match (str_of(v, t).as_str(), &xs[..]) {
                    ("s", &[_, seg, _, _, _, name, pos]) => {
                        let s = usize::try_from(int_of(v, pos)).unwrap_or(0);
                        (str_of(v, name), s, s + str_of(v, seg).len() + 1)
                    }
                    ("w", &[_, key, _, _]) => {
                        let key = str_of(v, key);
                        let Some(file) = units_file(&key) else {
                            continue;
                        };
                        let Some((u, at)) = self.g.unit_value(&key) else {
                            continue;
                        };
                        (file.to_owned(), at, at + u.len())
                    }
                    _ => continue,
                };
                span = Some(match span {
                    Some((n, a, z)) if n == name => (n, a.min(s), z.max(e)),
                    Some(sp) => sp,
                    None => (name, s, e),
                });
            }
            let Some((name, s, e)) = span else { continue };
            let text = texts
                .entry(name.clone())
                .or_insert_with(|| self.g.file_text(&name))
                .clone();
            if let Some(t) = text {
                out.push((x.raw(), Some(t), s, e));
            }
        }
        out
    }

    /// The files the job writes and reads back, with their text.
    #[must_use]
    pub fn read_back_texts(&self) -> Vec<(String, String)> {
        let mut t = self.files();
        t.retain(|(n, _)| self.g.written.iter().any(|w| w == n));
        t
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

    /// The engine's own diagnostics (an undeclared definition, a hint's
    /// regime violated), counted apart from TeX's errors: they are not in
    /// the log, and decide neither the output nor the history.
    #[must_use]
    pub fn engine_diagnostics(&self) -> usize {
        self.diagnostics()
            .iter()
            .filter(|d| d.severity == Severity::Diagnostic)
            .count()
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
        if crate::exec::obs_flag!("PHITEX_PKG_TRACE") {
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
        if !crate::exec::prof_flag!("PHITEX_PKG_STATS") {
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
        written_texts(v, acc("streams"))
    }

    /// The key sets of the read-back files that only define (`.aux`),
    /// as the last build published them: [`Options::wkeys`] for a later
    /// session of the same document.
    #[must_use]
    pub fn wkeys(&self) -> Vec<(String, Vec<String>)> {
        let v = self.g.eng.values();
        let acc = |n: &str| self.g.eng.acc_value(self.unit, v.field(n));
        written_defs(v, acc("streams"))
            .into_iter()
            .filter(|(n, _)| defines_only(n))
            .map(|(n, ds)| (n, ds.into_iter().map(|(f, _)| f).collect()))
            .collect()
    }

    /// Whether the output is a PDF file (`\pdfoutput` at the first
    /// `\shipout`); else DVI.
    #[must_use]
    pub fn is_pdf(&self) -> bool {
        let v = self.g.eng.values();
        self.g
            .eng
            .output(self.dvi)
            .is_some_and(|x| crate::assemble::file_bytes_shared(v, x).starts_with(b"%PDF"))
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
                // (the kind compared as a value: no string made per entry)
                let fatal = v.str("fatal");
                v.seq_to_vec(e)
                    .into_iter()
                    .any(|x| list_of(v, x).first() == Some(&fatal))
            })
    }

    /// The output file (DVI or PDF) and the log.
    #[must_use]
    pub fn output(&self) -> (Vec<u8>, String) {
        let (b, log) = self.output_shared();
        (b.as_ref().clone(), log)
    }

    /// [`Build::output`], the file's bytes shared with the engine's
    /// (not copied).
    #[must_use]
    pub fn output_shared(&self) -> (Arc<Vec<u8>>, String) {
        let v = self.g.eng.values();
        let key = (
            self.g.eng.output(self.dvi),
            self.g.eng.acc_value(self.unit, v.field("log")),
            self.g.eng.acc_value(self.unit, v.field("pagehash")),
            self.g.eval_errors.len(),
        );
        if let Some((k, x)) = &self
            .memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .output
            && *k == key
        {
            return x.clone();
        }
        let x = self.output_made();
        self.memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .output = Some((key, x.clone()));
        x
    }

    fn output_made(&self) -> (Arc<Vec<u8>>, String) {
        let v = self.g.eng.values();
        let mut dvi: Arc<Vec<u8>> = self
            .g
            .eng
            .output(self.dvi)
            .map(|x| crate::assemble::file_bytes_shared(v, x))
            .unwrap_or_default();
        let t0 = std::time::Instant::now();
        let n = self.pages().len();
        crate::timeline::note("output: the pages", t0.elapsed());
        let t0 = std::time::Instant::now();
        let mut memo = self.memo.lock().unwrap_or_else(PoisonError::into_inner);
        let (fatal, fatal_pdf) = memo.log.update(
            v,
            &self.opts.name,
            self.g.eng.acc_value(self.unit, v.field("log")),
        );
        let fatal = fatal && (dvi.starts_with(b"%PDF") || fatal_pdf);
        crate::timeline::note(
            "output: the log laid out (fatal errors found)",
            t0.elapsed(),
        );
        // (pdfTeX §794: after a fatal error the PDF file is removed)
        let end = if fatal {
            dvi = Arc::default();
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
        let log = memo.log.finish(&end);
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
        let t_spans = std::time::Instant::now();
        let v = self.g.eng.values();
        // the document's text and its nodes' spans: the text itself where
        // the nodes cover it in order (every document), not a copy
        let contiguous = {
            let mut at = 0;
            self.nodes.iter().all(|(_, _, src)| {
                let Src::Doc(s, e) = *src;
                let ok = s == at;
                at = e;
                ok
            }) && self.nodes.last().is_none_or(|(_, _, src)| {
                let Src::Doc(_, e) = *src;
                e >= self.text.len()
            })
        };
        let mut owned = String::new();
        let mut spans = Vec::with_capacity(self.nodes.len());
        for (_, _, src) in &self.nodes {
            if contiguous {
                let Src::Doc(s, _) = *src;
                spans.push(s.min(self.text.len()));
            } else {
                spans.push(owned.len());
                owned.push_str(src.text(&self.text));
            }
        }
        let doc: &str = if contiguous { &self.text } else { &owned };
        // (the document's line starts, once: a diagnostic's line by a
        // search, not by counting the text before it)
        crate::timeline::note("diagnostics: the nodes' spans", t_spans.elapsed());
        let t_lines = std::time::Instant::now();
        let mut kept = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        let made;
        let doc_lines: &[usize] = if contiguous {
            kept.get_or_insert_with(|| line_starts(doc))
        } else {
            made = line_starts(doc);
            &made
        };
        crate::timeline::note("diagnostics: the document's line starts", t_lines.elapsed());
        // the node of the text a node is under, by its parents: the text's
        // nodes by id made only for a diagnostic placed by its node alone
        let mut top_map: Option<phitex_defuse::FxHashMap<u64, usize>> = None;
        let mut top = |node: u64| -> usize {
            let m = top_map.get_or_insert_with(|| {
                self.nodes
                    .iter()
                    .enumerate()
                    .map(|(i, (_, n, _))| (n.raw(), i))
                    .collect()
            });
            let mut n = phitex_defuse::NodeId::from_raw(node);
            loop {
                if let Some(&i) = m.get(&n.raw()) {
                    return i;
                }
                match self.g.eng.node_info(n).and_then(|i| i.parent) {
                    Some(p) => n = p,
                    None => return 0,
                }
            }
        };
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
        let t_entries = std::time::Instant::now();
        let n_entries = entries.len();
        // (an `\input` file's text, read once a call, not once an entry)
        let mut files: phitex_defuse::FxHashMap<String, Arc<str>> =
            phitex_defuse::FxHashMap::default();
        for e in entries {
            let xs = list_of(v, e);
            let int = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
            let kind = int(0);
            let message = xs.get(1).map(|&x| str_of(v, x)).unwrap_or_default();
            let seg = xs.get(5).map(|&x| str_of(v, x)).unwrap_or_default();
            let loc = usize::try_from(int(6)).unwrap_or(0);
            let before: String = seg.chars().take(loc).collect();
            let in_doc = xs.get(2).is_some_and(|&n| str_of(v, n) == DOC_NAME);
            let file_text: Arc<str>;
            let (file, text, at): (String, &str, usize) = if int(3) >= 0 && in_doc {
                // a line of the document's own text: the position the
                // node's frame carries is the byte in the document
                let at = usize::try_from(int(3)).unwrap_or(0)
                    + seg.chars().take(loc).map(char::len_utf8).sum::<usize>();
                (docname.clone(), doc, at)
            } else if int(3) >= 0
                && let Some(&n) = xs.get(2)
                && n != ValueId::UNIT
            {
                // a line of an `\input` file: its characters are bytes
                let name = str_of(v, n);
                file_text = files
                    .entry(name.clone())
                    .or_insert_with(|| {
                        let bytes = (self.opts.finder)(name.trim_start_matches("./"))
                            .map(|(_, b)| b)
                            .unwrap_or_default();
                        bytes
                            .iter()
                            .map(|&b| char::from(b))
                            .collect::<String>()
                            .into()
                    })
                    .clone();
                let at = usize::try_from(int(3)).unwrap_or(0) + loc;
                (name, &*file_text, at)
            } else {
                let node = u64::from_ne_bytes(int(4).to_ne_bytes());
                let i = top(node);
                let from = spans.get(i).copied().unwrap_or(0);
                let at = doc
                    .get(from..)
                    .and_then(|d| (!seg.is_empty()).then(|| d.find(&seg)).flatten())
                    .map_or(from, |k| from + k + before.len());
                let at = if self.nodes.is_empty() { 0 } else { at };
                (docname.clone(), doc, at)
            };
            // the line around `at`, as characters; `at` in the file's
            // bytes (the document's text is UTF-8, a file's characters
            // are its bytes)
            // (in the text's bytes: a line break is one byte; a file's
            // characters are its bytes, `at` their count)
            let b = if file == docname {
                if text.is_char_boundary(at.min(text.len())) {
                    at.min(text.len())
                } else {
                    0
                }
            } else {
                text.char_indices().nth(at).map_or(text.len(), |(k, _)| k)
            };
            let head = &text[..b];
            // (the document's line by its line starts; a file's counted)
            let line = if file == docname {
                doc_lines.partition_point(|&k| k <= b)
            } else {
                head.bytes().filter(|&c| c == b'\n').count() + 1
            };
            let ls = head.rfind('\n').map_or(0, |k| k + 1);
            let le = text[b..].find('\n').map_or(text.len(), |k| b + k);
            let before: String = text[ls..b].to_owned();
            let after: String = text[b..le].trim_end_matches('\r').to_owned();
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
                severity: match kind {
                    0 => Severity::Warning,
                    1 => Severity::Error,
                    _ => Severity::Diagnostic,
                },
                message,
                context,
                help,
            });
        }
        crate::timeline::note(
            &format!("diagnostics: {n_entries} entries"),
            t_entries.elapsed(),
        );
        out.extend(self.prelude.diags.iter().cloned());
        for m in &self.g.eval_warnings {
            out.push(Diagnostic {
                file: docname.clone(),
                range: (0, 0),
                line: 1,
                severity: Severity::Warning,
                message: m.clone(),
                context: String::new(),
                help: None,
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
                let md = match crate::hints::may_define(&self.g.ops.hints, v, &i.args) {
                    phitex_defuse::MayDefine::Any => "Any".to_owned(),
                    other => format!("{other:?}").chars().take(12).collect(),
                };
                let h = crate::hints::hint_of_args(&self.g.ops.hints, v, &i.args);
                let reads =
                    crate::hints::reads(&self.g.ops.hints, v, &i.args).map_or(0, |r| r.len());
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
            let h = crate::hints::hint_of_args(&self.g.ops.hints, v, &i.args);
            let md = match crate::hints::may_define(&self.g.ops.hints, v, &i.args) {
                phitex_defuse::MayDefine::Any => "Any".to_owned(),
                other => format!("{other:?}").chars().take(160).collect(),
            };
            let eff = if crate::hints::effect_none(&self.g.ops.hints, v, &i.args) {
                "None"
            } else {
                "Pending"
            };
            let reads: Vec<String> = crate::hints::reads(&self.g.ops.hints, v, &i.args)
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
        let step = |n: phitex_defuse::NodeId| eng.node_info(n).is_some_and(|i| i.op == "finish");
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
        let acc = self.g.eng.acc_value(self.unit, f);
        if let Some((k, x)) = &self
            .memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pages
            && *k == acc
        {
            return x.clone();
        }
        let x = self.pages_made();
        self.memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pages = Some((acc, x.clone()));
        x
    }

    fn pages_made(&self) -> Vec<u64> {
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
    // (the format's bindings made values on the pool, in their order)
    let mut ex = crate::exec::Coarse(crate::exec::Threads::new(g.threads));
    let chunk = env.len().div_ceil(g.threads.max(1) * 4).max(1);
    let mut sets: Vec<(Field, ValueId)> =
        crate::exec::map_on(&mut ex, env.len().div_ceil(chunk), &|c| {
            env[c * chunk..((c + 1) * chunk).min(env.len())]
                .iter()
                .map(|(k, t)| (v.field(k), import(v, t)))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect();
    sets.push((v.field("jobname"), v.str(&opts.jobname)));
    sets.push((v.field("dvicomment"), v.str(&opts.comment)));
    sets.push((v.field("shellescape"), v.int(i64::from(opts.shell_escape))));
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
    if crate::exec::obs_flag!("PHITEX_RECORD_CHECK") {
        g.start_env.extend(sets.iter().copied());
    }
    // (spec-final §4.5: the job's bindings go with the environment, one
    // push for all of them, made again only where an entry changed; the
    // graph has one unit, so the unit's start and the environment are
    // one place to read them)
    let _ = unit;
    g.job = sets;
    let env = g.environment(opts.fixpoint);
    g.push_env(env);
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
        "fread",
        "pkgrecs",
        "gdefs",
        crate::pdfout::XPAGES,
        crate::vprof::VPROF,
        crate::pages::PITEMS,
    ] {
        let fld = eng.values().field(f);
        eng.declare_accumulator(fld, Arc::new(SeqMonoid));
    }
    // (the streams by file, `run::stream_bucket`, and the files' names)
    let mut per: Vec<Field> = (0..crate::run::STREAM_BUCKETS)
        .map(|k| eng.values().field(&format!("wstreams:{k}")))
        .collect();
    per.push(eng.values().field(crate::run::WNAMES));
    for fld in per {
        eng.declare_accumulator(fld, Arc::new(SeqMonoid));
    }
    let lines = eng.values().field("lines");
    eng.declare_accumulator(lines, Arc::new(LinesMonoid));
    let allocs = eng.values().field(crate::pdfout::ALLOCS);
    eng.declare_accumulator(allocs, Arc::new(SeqMonoid));
    for f in crate::hints::COUNTERS {
        let fld = eng.values().field(f);
        eng.declare_accumulator(fld, Arc::new(SumMonoid));
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
        // (both sorted by name: merged in one walk, the names compared
        // borrowed, no string made)
        let pair = |e: ValueId| {
            v.with_list(e, |p| (p.first().copied(), p.get(1).copied()))
                .unwrap_or((None, None))
        };
        let cmp = |a: ValueId, b: ValueId| {
            if a == b {
                return std::cmp::Ordering::Equal;
            }
            v.with_str(a, |x| v.with_str(b, |y| x.cmp(y)))
                .flatten()
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        let mut out: Vec<ValueId> = Vec::with_capacity(xs.len() + ys.len());
        let mut last: Option<(ValueId, i64)> = None;
        let (mut i, mut j) = (0, 0);
        let mut push = |n: ValueId, k: i64, out: &mut Vec<ValueId>| match &mut last {
            Some((m, c)) if cmp(*m, n).is_eq() => *c += k,
            _ => {
                if let Some((m, c)) = last.replace((n, k)) {
                    out.push(v.list(vec![m, v.int(c)]));
                }
            }
        };
        while i < xs.len() || j < ys.len() {
            let a = xs.get(i).map(|&e| pair(e));
            let b = ys.get(j).map(|&e| pair(e));
            let take_a = match (a, b) {
                (Some((Some(na), _)), Some((Some(nb), _))) => !cmp(na, nb).is_gt(),
                (Some(_), _) => true,
                _ => false,
            };
            let (n, k) = if take_a {
                i += 1;
                a.unwrap_or((None, None))
            } else {
                j += 1;
                b.unwrap_or((None, None))
            };
            if let (Some(n), Some(k)) = (n, k) {
                push(n, int_of(v, k), &mut out);
            }
        }
        if let Some((m, c)) = last {
            out.push(v.list(vec![m, v.int(c)]));
        }
        v.list(out)
    }
}

/// `PHITEX_DECL_TRACE`: a loop-back declaration, its family and whether
/// its initial value is the one declared before.
fn decl_trace(eng: &Engine, f: Field, init: ValueId) {
    static LAST: std::sync::Mutex<Option<FxHashMap<Field, ValueId>>> = std::sync::Mutex::new(None);

    if !crate::exec::obs_flag!("PHITEX_DECL_TRACE") {
        return;
    }
    let mut l = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prev = l.get_or_insert_with(FxHashMap::default).insert(f, init);
    let name = field_name(eng.values(), f);
    let fam = name.split(':').next().unwrap_or("").to_owned();
    let how = match prev {
        None => "new",
        Some(p) if p == init => "same init",
        Some(_) => "other init",
    };
    eprintln!("decl-trace: declare_loopback {fam}: {how}");
}

/// `PHITEX_DECL_TRACE`: a full environment map, by family: fields new,
/// changed, unchanged and dropped against the map supplied before.
fn env_trace(eng: &Engine, env: &[(Field, ValueId)]) {
    static LAST: std::sync::Mutex<Option<FxHashMap<Field, ValueId>>> = std::sync::Mutex::new(None);

    if !crate::exec::obs_flag!("PHITEX_DECL_TRACE") {
        return;
    }
    let mut l = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let old = l.take().unwrap_or_default();
    let new: FxHashMap<Field, ValueId> = env.iter().copied().collect();
    let v = eng.values();
    let fam = |f: Field| field_name(v, f).split(':').next().unwrap_or("").to_owned();
    let mut c: std::collections::BTreeMap<String, [u32; 4]> = std::collections::BTreeMap::new();
    for (&f, &x) in &new {
        let k = match old.get(&f) {
            None => 0,
            Some(&y) if y != x => 1,
            Some(_) => 2,
        };
        c.entry(fam(f)).or_default()[k] += 1;
    }
    for &f in old.keys().filter(|f| !new.contains_key(f)) {
        c.entry(fam(f)).or_default()[3] += 1;
    }
    let s: Vec<String> = c
        .iter()
        .map(|(k, [a, b, d, e])| format!("{k}: {a} new {b} changed {d} same {e} dropped"))
        .collect();
    eprintln!(
        "decl-trace: set_environment {} fields; {}",
        new.len(),
        s.join("; ")
    );
    *l = Some(new);
}

/// (debug) The first leaf where two values differ: its path, and both
/// leaves shown with their ids and kinds.
fn structural_diff(
    v: &Values,
    a: ValueId,
    b: ValueId,
    path: String,
) -> Option<(String, String, String)> {
    use phitex_defuse::Value;
    if a == b {
        return None;
    }
    let (va, vb) = (v.get(a), v.get(b));
    match (&*va, &*vb) {
        (Value::List(xa), Value::List(xb)) => {
            if xa.len() != xb.len() {
                return Some((
                    path,
                    format!("list len {}", xa.len()),
                    format!("list len {}", xb.len()),
                ));
            }
            for (i, (&x, &y)) in xa.iter().zip(xb.iter()).enumerate() {
                if let Some(d) = structural_diff(v, x, y, format!("{path}/{i}")) {
                    return Some(d);
                }
            }
            Some((
                path,
                format!("list {:?} (same elements)", a.raw()),
                format!("list {:?}", b.raw()),
            ))
        }
        _ => Some((
            path,
            format!("{:?} id {}", *va, a.raw())
                .chars()
                .take(120)
                .collect(),
            format!("{:?} id {}", *vb, b.raw())
                .chars()
                .take(120)
                .collect(),
        )),
    }
}

/// Debug: `PHITEX_DEBUG_NODES=id,id,...` prints the engine's view of
/// those nodes (`debug_node`: label, state, flags; the parent) at the
/// evaluation's start and end.
fn debug_nodes(eng: &Engine, when: &str) {
    let Some(list) = crate::exec::obs_str!("PHITEX_DEBUG_NODES") else {
        return;
    };
    eprintln!("debug nodes {when}:");
    for id in list.split(',').filter_map(|s| s.trim().parse::<u64>().ok()) {
        let n = phitex_defuse::NodeId::from_raw(id);
        let parent = eng
            .node_info(n)
            .map(|i| i.parent.map(phitex_defuse::NodeId::raw));
        let d = eng.debug_node(n);
        let tail = d.rfind(" flags ").map_or(d.as_str(), |i| &d[i..]);
        let head: String = d.chars().take(40).collect();
        eprintln!("  {id}: parent {parent:?}; {head} ...{tail}");
        for l in eng.debug_reads(n).lines() {
            if l.contains("count:0") || l.starts_with("\"ps\"") || l.starts_with("ps ") {
                eprintln!("    read: {}", l.chars().take(160).collect::<String>());
            }
        }
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

#[cfg(test)]
mod canonical_tests {
    use super::*;

    /// The marking line by line, as it was before the copy between
    /// lines with comments (the invariant: the same text and marks).
    fn by_lines(text: &str) -> (String, Vec<(u32, String)>) {
        let (mut out, mut marks) = (String::new(), Vec::new());
        for (n, line) in text.split_inclusive('\n').enumerate() {
            let (body, nl) = match line.strip_suffix('\n') {
                Some(b) => (b, "\n"),
                None => (line, ""),
            };
            canonical_line(body, n, &mut out, &mut marks);
            out.push_str(nl);
        }
        (out, marks)
    }

    #[test]
    fn lines_without_comments_are_copied_as_they_are() {
        for t in [
            "",
            "a\n",
            "a",
            "no comment\nat all\n",
            "x % one\n\n\\% not one\n\\\\% one \t\r\nend",
            "%\n% \n%%\n\\%%\n",
            "a\nb\nc % c\nd\ne\\% f % g\n\n\nh\n% last",
            "é % ü\nü\n",
        ] {
            assert_eq!(canonical_text(t).unwrap(), by_lines(t), "{t:?}");
            let (mut out, mut marks) = (String::new(), Vec::new());
            canonical_lines(t, 7, &mut out, &mut marks);
            let (o, m) = by_lines(t);
            assert_eq!(out, o);
            assert_eq!(
                marks,
                m.into_iter().map(|(n, c)| (n + 7, c)).collect::<Vec<_>>()
            );
        }
    }
}
