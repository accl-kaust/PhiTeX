//! The client: a document's text as nodes, the prelude's environment,
//! the files the graph asks for, edits, and the outputs read back (DVI
//! bytes, the log).
//!
//! Nodes come from the syntactic pass ([`phitex_source::pass`]) sliced by
//! `hints::slices_of`: one per paragraph region or loading command of a
//! document, one for a whole format. Files a run reads come in place
//! from the environment provider (`Graph::provide`).

use crate::exec::LockWait as _;
use phitex_defuse::FxHashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

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
#[allow(clippy::struct_excessive_bools)]
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
    /// Whether a package load never seen runs on the optimistic hint
    /// (`crate::pkgrec`): not `Off`. The output is the same either way.
    pub speculation: Speculation,
    /// The round-one key sets of the read-back files that only define
    /// (`.aux`: `wkeys:<name>`), as [`Build::wkeys`] gave them in an
    /// earlier session (the last build's values); a file not listed has
    /// no guess. A right guess saves the loop-back's second round; a
    /// wrong one costs nothing more than no guess.
    pub wkeys: Vec<(String, Vec<String>)>,
    /// The files the job writes as they are when the build begins (name,
    /// text), as an earlier run left them on disk (`fixpoint` only): the
    /// first round reads each back from this text where the job reads it
    /// (`crate::run::Run::seed_written`), as TeX's next run reads the file
    /// the last one wrote, so a round that writes the files as it read
    /// them is the last. A file not listed starts absent. The engine
    /// reads no file of its own.
    pub written_init: Vec<(String, String)>,
    /// The package loads' records ([`Build::export_pkgrec`]): kept
    /// machine-wide beside the format, keyed by the load and the files
    /// it read, never by the document, so a new document's first build
    /// uses what any earlier one learned (`crate::pkgrec`). Empty: none
    /// (every load unbounded, or optimistic under speculation).
    pub pkgrec: Vec<u8>,
    /// The document nodes' records ([`Build::export_noderec`]): those
    /// whose own text cannot bound what they define, keyed by their
    /// text, kept per document (`crate::pkgrec`). Merged with
    /// [`Self::pkgrec`]. Empty: none.
    pub noderec: Vec<u8>,
    /// Whether a document node whose text cannot bound what it defines
    /// is recorded as a package load is (`crate::pkgrec::is_recorded`):
    /// off, its runs record nothing and [`Self::noderec`] is not read.
    pub node_records: bool,
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

/// Whether a package load never seen runs on the optimistic hint
/// ([`Options::speculation`], `crate::pkgrec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speculation {
    /// Never.
    Off,
    /// On nodes a previous run predicts.
    Predicted,
    /// On any candidate.
    All,
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
    #[allow(
        clippy::must_use_candidate,
        reason = "a send; the version is for whoever wants it"
    )]
    pub fn send_edit(&self, text: Arc<str>, hint: Option<Hint>) -> u64 {
        let mut p = self.0.locked();
        p.version += 1;
        // (a hint against a text the engine has not taken is no hint)
        let hint = if p.text.is_some() { None } else { hint };
        p.text = Some((text, hint));
        p.version
    }

    /// Whether a text waits that no build has taken.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.0.locked().text.is_some()
    }

    fn take(&self) -> Option<(Arc<str>, Option<Hint>, u64)> {
        let mut p = self.0.locked();
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
    /// comments, for the environment (`doc:mark:<id>`).
    canon: &'a mut bool,
    marks: Marks,
    /// The package loads' hints (`crate::pkgrec`).
    loads: Option<&'a crate::pkgrec::LoadHints>,
    /// The build's hint registry (`crate::hints::Registry`).
    hints: Arc<crate::hints::Registry>,
    /// The nodes the texts applied replaced, inserted or removed.
    applied: usize,
}

impl phitex_defuse::EditSource for Absorb<'_> {
    #[allow(clippy::too_many_lines)]
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        let (raw, hint, version) = self.edits.take()?;
        crate::timeline::lap("engine: the edit taken");
        // (the units hold the last text as a file, [`as_file`]: a range of
        // the last text is one of theirs only where that text ended its
        // lines already; a text cut off mid-line, as a file read while
        // its editor writes it, has them a byte longer)
        let whole = self.text.is_empty() || self.text.ends_with('\n');
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
                    match canonical_splice(old, self.text, &self.marks.get(), &raw, ends) {
                        None => canonical_text(&raw).map(|(t, ms)| (Some(t), ms)),
                        Some((ms, r)) => {
                            // (`PHITEX_CANON_CHECK`: against the whole text's)
                            if crate::exec::obs_flag!("PHITEX_CANON_CHECK") {
                                assert!(
                                    canonical_text(&raw).as_ref()
                                        == Some(&(self.text.clone(), ms.clone())),
                                    "canonical_splice differs from canonical_text"
                                );
                            }
                            range = Some(r);
                            // (the line starts: those in the range made again,
                            // those after it moved)
                            let mut ls = self.lines.locked();
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
                                    assert!(
                                        *v == line_starts(self.text),
                                        "the line starts moved wrong"
                                    );
                                }
                            }
                            Some((None, ms))
                        }
                    }
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
                *self.lines.locked() = None;
            }
        } else {
            *self.lines.locked() = None;
            *self.canon = false;
            self.marks.set(Vec::new());
            *self.raw = None;
            *self.text = raw.to_string();
        }
        crate::timeline::lap("engine: the canonical text (comments marked)");
        let n = apply_text(
            &self.hints,
            eng,
            self.exec,
            self.end,
            self.nodes,
            self.text,
            range.filter(|_| whole),
            self.incr.as_deref_mut(),
            self.loads,
        );
        self.applied += n;
        if n > 0 {
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

/// The most nodes an edit's text and files may change for its
/// evaluation to run on the calling thread, no pool woken (an edit's
/// evaluation of a few nodes on the pool spent more waking and stealing
/// than running), and the most it runs and queues there: past them it
/// goes on the pool, in steps, as a cold build (`Executor::widen`).
const INLINE_NODES: usize = 16;

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
/// its units ([`doc_nodes`]), its analysis registered under its name
/// (`file`: a document file's, else the document's), a format's the
/// whole text as one.
fn slice_keys(
    reg: &crate::hints::Registry,
    v: &Values,
    text: &str,
    incr: Option<&Incr>,
    file: Option<ValueId>,
) -> Vec<(u64, DocNode)> {
    match incr {
        Some(inc) => {
            match file {
                Some(name) => register_file(reg, name, inc),
                None => register_doc(reg, v, inc),
            }
            doc_nodes(text, inc)
        }
        None if text.is_empty() => Vec::new(),
        None => vec![(key(text, true), DocNode::Doc(0, text.len()))],
    }
}

/// A document file's analysis registered under its name: its chunks'
/// (a node made from part of a slice's line is hinted from the chunk
/// holding it, by the file's offsets).
fn register_file(reg: &crate::hints::Registry, name: ValueId, inc: &Incr) {
    let cs: Vec<(usize, Arc<crate::hints::TextHints>)> =
        inc.chunks.iter().map(|c| (c.start, c.th.clone())).collect();
    crate::hints::register_chunks(reg, name, Arc::from(cs));
}

/// A text cut again (the document's, or a document file's, `file`): its
/// units spliced where it changed (`range`, as [`Incr::set_text_edit`]
/// takes it; none: anywhere), the chunks that reaches analysed again;
/// its slices and their keys ([`slice_keys`]).
fn cut_text(
    reg: &crate::hints::Registry,
    v: &Values,
    text: &str,
    range: Option<(usize, usize, usize)>,
    incr: Option<&mut Incr>,
    file: Option<ValueId>,
) -> Vec<(u64, DocNode)> {
    match incr {
        Some(inc) => {
            match range {
                Some(r) => inc.set_text_edit(text, r),
                None => inc.set_text(text),
            }
            crate::timeline::lap("engine: the units spliced and walked");
            slice_keys(reg, v, text, Some(inc), file)
        }
        None => slice_keys(reg, v, text, None, file),
    }
}

/// Where an edit puts the nodes it inserts ([`apply_slices`]).
#[derive(Clone, Copy)]
enum Placing {
    /// The document's: top-level nodes, before the one given (the
    /// writer, [`Build::end`]) where none precedes.
    Top(NodeId),
    /// A document file's pieces: placed beside the others, as the run
    /// that placed those placed them (`Engine::insert_placed`).
    Placed,
}

/// What [`apply_slices`] did: the nodes kept, replaced, removed and
/// inserted.
#[derive(Clone, Copy, Default)]
struct Applied {
    /// The nodes kept at the start and at the end.
    prefix: usize,
    suffix: usize,
    replaced: usize,
    removed: usize,
    inserted: usize,
}

impl Applied {
    /// The nodes it changed.
    fn changed(self) -> usize {
        self.replaced + self.removed + self.inserted
    }
}

/// Edits the nodes `nodes_now` (each with its key) to the slices `new`:
/// the slices both share at the start and the end stay, those between
/// are replaced, removed or inserted (their arguments `args_of`'s).
/// The document's text and a document file's are edited so alike, the
/// file's pieces under each node that placed them ([`Build::stage_file`]).
/// `None`: a file's pieces with none to place new ones beside.
fn apply_slices(
    eng: &Engine,
    exec: OpId,
    at: Placing,
    nodes_now: &mut Vec<(u64, NodeId, Src)>,
    new: &[(u64, DocNode)],
    args_of: &dyn Fn(&Engine, usize) -> Vec<ValueId>,
) -> Option<Applied> {
    let old = &*nodes_now;
    if old.len() == new.len() && old.iter().zip(new).all(|(o, n)| o.0 == n.0) {
        return Some(Applied {
            prefix: old.len(),
            ..Applied::default()
        });
    }
    if old.is_empty() && matches!(at, Placing::Placed) {
        return None;
    }
    let (a, b) = common_ends(old, new);
    let (old_mid, new_mid) = (old.len() - a - b, new.len() - a - b);
    let common = old_mid.min(new_mid);
    crate::timeline::lap("engine: the slice-key diff");
    for k in common..old_mid {
        match at {
            Placing::Top(_) => eng.remove(old[a + k].1),
            Placing::Placed => eng.remove(old[a + k].1),
        }
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
            let next = old.get(a + old_mid).map(|x| x.1);
            match (at, nodes.last()) {
                (Placing::Top(_), Some(&(_, p, _))) => eng.insert_after(p, exec, args),
                (Placing::Top(end), None) => eng.insert_before(next.unwrap_or(end), exec, args),
                (Placing::Placed, Some(&(_, p, _))) => eng.insert_after(p, exec, args),
                (Placing::Placed, None) => eng.insert_before(next?, exec, args),
            }
        };
        nodes.push((new[i].0, n, Src::of(&new[i].1)));
    }
    *nodes_now = nodes;
    crate::timeline::lap("engine: replace/insert/remove, the node list made");
    Some(Applied {
        prefix: a,
        suffix: b,
        replaced: common,
        removed: old_mid - common,
        inserted: new_mid - common,
    })
}

/// Edits the document's nodes `nodes` to the text `text` ([`cut_text`],
/// [`apply_slices`]). The nodes replaced, inserted or removed (0: a text
/// whose slices are the nodes' already).
// one call site (Absorb::absorb), which passes its fields apart
#[allow(clippy::too_many_arguments)]
fn apply_text(
    reg: &crate::hints::Registry,
    eng: &Engine,
    exec: OpId,
    end: NodeId,
    nodes_now: &mut Vec<(u64, NodeId, Src)>,
    text: &str,
    range: Option<(usize, usize, usize)>,
    mut incr: Option<&mut Incr>,
    loads: Option<&crate::pkgrec::LoadHints>,
) -> usize {
    let t_all = std::time::Instant::now();
    let text = as_file(text);
    crate::timeline::lap("engine: the text as a file");
    // (the range that changed where it is known and the text is the file
    // as it is)
    let range = range.filter(|_| matches!(text, std::borrow::Cow::Borrowed(_)));
    let new = cut_text(reg, eng.values(), &text, range, incr.as_deref_mut(), None);
    crate::timeline::lap("engine: the slices and their keys");
    let t_hints = t_all.elapsed();
    let incr = incr.as_deref();
    let old_len = nodes_now.len();
    let args_of = |eng: &Engine, k: usize| node_args(eng.values(), &text, incr, &new[k].1, loads, None);
    let t_diff = t_all.elapsed();
    let Some(d) = apply_slices(eng, exec, Placing::Top(end), nodes_now, &new, &args_of) else {
        return 0;
    };
    if d.changed() == 0 {
        return 0;
    }
    edit_apply_note(
        t_all,
        [t_hints, t_hints, t_diff],
        [
            old_len,
            new.len(),
            d.prefix + d.suffix,
            d.replaced,
            d.removed,
            d.inserted,
            text.len(),
        ],
    );
    d.changed()
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
            written_init: Vec::new(),
            pkgrec: Vec::new(),
            noderec: Vec::new(),
            node_records: true,
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
        // (a string or a number interned from where it is: no copy of the
        // leaf made first)
        Tree::Leaf(Value::Str(x)) => v.str(x),
        Tree::Leaf(Value::Int(i)) => v.int(*i),
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

type Env = Arc<crate::fmtenv::FormatEnv>;

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
    /// The definitions harvested from its text and files
    /// ([`prelude_defs`]), as the exported prelude kept them: read when
    /// first asked, not harvested again.
    defs_bytes: Option<Vec<u8>>,
    /// Its harvested definitions, once made or read.
    defs: OnceLock<Defs>,
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
    // (the list and its tag looked at in place: no copy of either per
    // entry)
    let tag = v
        .with_list(e, |xs| (xs.len() == 3).then(|| xs[0]))
        .unwrap_or_else(|| {
            let xs = list_of(v, e);
            (xs.len() == 3).then(|| xs[0])
        });
    tag.is_some_and(|t| v.with_str(t, |s| s == CAND).unwrap_or(false))
}

/// A `streams` entry without its page tag (`[page, entry]` or
/// `[page, entry, piece, candidate]` from a page's finisher, `page` its
/// `[piece, nth]`, [`crate::run::Run::contribute_stream`]).
pub(crate) fn stream_entry(v: &Values, e: ValueId) -> ValueId {
    let xs = list_of(v, e);
    match xs.as_slice() {
        [p, x] | [p, x, _, _] if page_of(v, *p).is_some() => *x,
        _ => e,
    }
}

/// A page tag's page: the piece it fired in and the pages fired in that
/// piece before it.
type PageOf = (i64, i64);

/// The [`PageOf`] a page tag names.
fn page_of(v: &Values, p: ValueId) -> Option<PageOf> {
    v.with_list(p, |xs| match xs {
        [a, n] => Some((v.as_int(*a)?, v.as_int(*n)?)),
        _ => None,
    })
    .flatten()
}

/// A page-tagged entry's page and, for a page whose break is inside a
/// piece, that piece and candidate.
fn page_tag(v: &Values, e: ValueId) -> Option<(PageOf, Option<(i64, i64)>)> {
    // (the entry's ids looked at in place: no copy of the list)
    let ids = |xs: &[ValueId]| match xs {
        [j, _] => Some((*j, None)),
        [j, _, p, k] => Some((*j, Some((*p, *k)))),
        _ => None,
    };
    let (j, pk) = v
        .with_list(e, ids)
        .unwrap_or_else(|| ids(&list_of(v, e)))?;
    Some((
        page_of(v, j)?,
        match pk {
            None => None,
            Some((p, k)) => Some((v.as_int(p)?, v.as_int(k)?)),
        },
    ))
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
    // piece in the order they fired there, stable within one page; the
    // pieces in the run's order)
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
        let mut pieces: Vec<i64> = Vec::new();
        for &e in &run {
            if let Some(((p, _), _)) = page_tag(v, e)
                && !pieces.contains(&p)
            {
                pieces.push(p);
            }
        }
        run.sort_by_key(|&e| {
            page_tag(v, e).map_or((0, 0), |((p, n), _)| (pieces.iter().position(|&q| q == p).unwrap_or(0), n))
        });
        es.extend(run);
    }
    // (each page's last entry, by the piece its break is inside of)
    let mut pages_of: FxHashMap<i64, Vec<(usize, i64)>> = FxHashMap::default();
    for (p, &e) in es.iter().enumerate() {
        if let Some((_, Some((pid, k)))) = page_tag(v, e) {
            let last = es
                .get(p + 1)
                .and_then(|&n| page_tag(v, n))
                .is_none_or(|t| Some(t.0) != page_tag(v, e).map(|u| u.0));
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
        merge_defs(v, &mut out[k].1, made);
    }
    out
}

/// One line's definitions (`made`, [`Run::interpret_line`]) added to a
/// file's read-back definitions `ds` ([`written_defs`]): each field
/// once, where it was first defined, with its last value.
pub(crate) fn merge_defs(v: &Values, ds: &mut Vec<(String, ValueId)>, made: ValueId) {
    for d in list_of(v, made) {
        let xs = list_of(v, d);
        // (`[field]`: a definition a read-back nested in the line
        // made, the other file's: a key of this file whose value that
        // file publishes, `UNIT` here)
        let (f, val) = match xs.as_slice() {
            [f, x, g] => (str_of(v, *f), v.list(vec![*x, *g])),
            [f] => (str_of(v, *f), ValueId::UNIT),
            _ => continue,
        };
        match ds.iter_mut().find(|(n, _)| *n == f) {
            Some(slot) if val != ValueId::UNIT => slot.1 = val,
            Some(_) => {}
            None => ds.push((f, val)),
        }
    }
}

/// The key set of file `name` read back per field (`wkeys:<name>`) from
/// its definitions `ds` ([`written_defs`]): [`WRITTEN_KEY`], then each
/// field, a key a nested read-back's file publishes by its id.
pub(crate) fn key_set(v: &Values, name: &str, ds: &[(String, ValueId)]) -> ValueId {
    let mut keys = vec![v.str(WRITTEN_KEY)];
    for (f, val) in ds {
        keys.push(if *val == ValueId::UNIT {
            v.str(&wdef_id(name, f))
        } else {
            v.str(f)
        });
    }
    v.list(keys)
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
// (4: fonts by key, `crate::fonts::key_of`; the format's font order
// `fontorder`)
// (5: the environment a table of distinct value nodes and the bindings
// by field, `crate::fmtenv`)
pub const PRELUDE_LAYOUT: u32 = 5;

/// The exported prelude's magic, before the layout and the engine's
/// identity.
const PRELUDE_MAGIC: &[u8; 8] = b"PHITEXPR";

/// The engine's identity ([`engine_identity`]): a format exported by an
/// engine of another identity is not read, one of the same is, whatever
/// else the binary holding it changed (a cache names formats by it).
#[must_use]
pub fn engine_id() -> u64 {
    engine_identity()
}

/// The engine's identity: a hash of this crate's sources and of those it
/// depends on by path, the engine's among them (`build.rs`), so changing
/// the engine or how this crate encodes its state invalidates every
/// exported prelude.
fn engine_identity() -> u64 {
    u64::from_str_radix(env!("PHITEX_TEX_SRC_HASH"), 16).unwrap_or(0)
}

/// The format's fonts by key in the order it loaded them (`fontorder`,
/// an environment field): TeX numbers a document's fonts after them,
/// in the order of the document's first loads ([`font_order`]).
pub(crate) const FONT_ORDER: &str = "fontorder";

/// The fonts' keys in the order of their first `fontload` records in
/// `line` (the line's accumulator, a sequence), each once.
pub(crate) fn font_order(v: &Values, line: Option<ValueId>) -> Vec<i64> {
    let mut seen = phitex_defuse::FxHashSet::default();
    let mut out = Vec::new();
    for x in line
        .map(|l| crate::val::cat_to_vec(v, l))
        .unwrap_or_default()
    {
        if let Some(k) = crate::pdfout::fontload_key(v, x)
            && seen.insert(k)
        {
            out.push(k);
        }
    }
    out
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
    c.env.write(&mut out);
    // (its harvested definitions: as read, else as made in this process)
    let made = c.defs.get().map(|d| d.to_bytes());
    match c.defs_bytes.as_ref().or(made.as_ref()) {
        Some(d) => {
            out.push(1);
            put_bytes(&mut out, d);
        }
        None => out.push(0),
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
    let (env, rest) = crate::fmtenv::FormatEnv::read(r.0)?;
    let mut r = Rd(rest);
    let defs_bytes = match r.take(1)?[0] {
        0 => None,
        1 => {
            let n = usize::try_from(r.u64()?).ok()?;
            Some(r.take(n)?.to_vec())
        }
        _ => return None,
    };
    if !r.0.is_empty() {
        return None;
    }
    Some(Cached {
        defs_bytes,
        defs: OnceLock::new(),
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
    /// The passes the files read back took to their fixpoint
    /// (`Report::passes`).
    pub loop_rounds: u64,
    /// The runs of each pass of the last evaluation (`Report::pass_runs`).
    pub pass_runs: Vec<u64>,
    /// Nodes that waited on a value not known yet, and stops.
    pub pending_waits: u64,
    /// Waits on a node whose declared set was pending.
    pub pending_sets: u64,
    pub stops: u64,
    /// Attempts that resumed a stopped run at its blocked read, and that
    /// started over though one was kept (a read changed).
    pub resumed: u64,
    /// Runs resumed at a mark of their last run, and runs cut off at one
    /// (`phitex_defuse::Report::resumed_at_mark`, `cut_in_run`).
    pub resumed_at_mark: u64,
    pub cut_in_run: u64,
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
    /// The longest chain of runs that waited on each other over this
    /// build's evaluations (`realised_critical_path`), in nodes.
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
    /// The version of the text the result is for ([`Build::version`]).
    pub version: u64,
    /// The texts taken while the engine evaluated (the engine's
    /// `edits_applied`): each one's version and the step it came before.
    pub absorbed: Vec<(u64, u64)>,
    /// Scheduler steps.
    pub steps: u64,
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
    /// assumptions, confirmed, and whose assumptions failed.
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    /// Predicted definitions ([`Options::pkg_predict`]) the runs
    /// confirmed, and refuted (their readers re-ran).
    pub predictions_confirmed: u64,
    pub predictions_refuted: u64,
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
    /// Document nodes given their record's hint (`crate::pkgrec`, since
    /// the build began).
    pub node_recorded: u64,
    /// Each node's run time in the evaluations (all its attempts), in
    /// microseconds, by node (`Report::run_times`, all of them).
    pub piece_times: Vec<(u64, u64)>,
    /// spec-final §1: where each evaluation's time went (the engine's
    /// `Report::times`).
    pub round_times: Vec<phitex_defuse::Times>,
    /// The operations' run time and runs by node kind
    /// (`crate::exec::KINDS`), over the evaluations.
    pub op_by_kind: Vec<(&'static str, std::time::Duration, u64)>,
    /// The environment pushes made to the engine (spec-final §4.5).
    pub env_calls: u64,
}

/// A rerun cause's kind and field (`phitex_defuse::Cause`).
fn cause_parts(c: &phitex_defuse::Cause) -> (&'static str, Option<Field>) {
    use phitex_defuse::Cause as C;
    match c {
        C::Rebind { field, .. } => ("Rebind", Some(*field)),
        C::Changed { field, site } => (if site.is_some() { "Changed" } else { "ChangedStart" }, Some(*field)),
        C::Range { field, .. } => ("Range", Some(*field)),
        C::Output { .. } => ("Output", None),
        C::Environment { field } => ("Environment", Some(*field)),
        C::Structure => ("Structure", None),
        C::Created { .. } => ("Created", None),
        C::Edited => ("Edited", None),
    }
}

/// (`PHITEX_RUN_AUDIT`) Every node run again in the evaluation by what
/// made it: its operation, the cause's kind and field.
fn print_rerun_classes(eng: &Engine, r: &Report) {
    let v = eng.values();
    let mut by: std::collections::BTreeMap<(String, &'static str, String), u64> = std::collections::BTreeMap::new();
    for (n, c) in &r.rerun_causes {
        let op = eng.node_info(*n).map_or("gone", |i| i.op).to_owned();
        let (kind, field) = cause_parts(c);
        let f = field.map_or_else(|| "-".to_owned(), |f| crate::val::field_name(v, f));
        *by.entry((op, kind, f)).or_default() += 1;
    }
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    for ((op, kind, f), n) in rows {
        eprintln!("acct-edit: rerun class {op} {kind} {f}: {n} runs");
    }
}

/// The cold accounting of an evaluation (spec-final §1), with
/// `PHITEX_TEX_TIME`: the engine's split of its time, the runs per node,
/// the op time by node kind, the realised critical path in nodes and
/// time and the kinds on the engine's critical chain, and the
/// environment pushes.
#[allow(clippy::too_many_lines)]
fn print_accounting(eng: &Engine, r: &Report, threads: usize, by_kind: &[(&'static str, std::time::Duration, u64)], env_calls: u64) {
    #[allow(clippy::cast_precision_loss)]
    let ms = |ns: u64| ns as f64 / 1e6;
    let t = &r.times;
    eprintln!(
        "acct: wall {:.3} ms at {threads} threads; summed over workers: op {:.3} ms (wasted {:.3}), commit {:.3} ms ({} commits, max {:.3} ms), re-ask {:.3} ms, idle {:.3} ms, lock wait {:.3} ms (max {:.3} ms)",
        ms(t.wall_ns),
        ms(t.op_ns),
        ms(t.wasted_ns),
        ms(t.commit_ns),
        r.commits,
        ms(t.commit_max_ns),
        ms(t.reask_ns),
        ms(t.idle_ns),
        ms(t.lock_wait_ns),
        ms(t.lock_wait_max_ns)
    );
    if phitex_defuse::obs::enabled() {
        let sites: Vec<String> = phitex_defuse::obs::snapshot()
            .lock_sites
            .iter()
            .filter(|s| s.1 > 0)
            .map(|(f, w)| format!("{f} {:.3}", ms(*w)))
            .collect();
        eprintln!("acct: lock wait by the file taking it (ms, the process so far): {}", sites.join(", "));
        let lines: Vec<String> = phitex_defuse::obs::lock_lines().iter().take(12).map(|(l, w)| format!("{l} {:.3}", ms(*w))).collect();
        eprintln!("acct: lock wait by line (ms, the process so far): {}", lines.join(", "));
    }
    if let Some(n) = r.commit_max_node {
        let i = eng.node_info(n);
        let head: String = i
            .as_ref()
            .and_then(|i| i.args.first().map(|&a| eng.values().render(a)))
            .unwrap_or_default()
            .chars()
            .take(60)
            .collect();
        eprintln!(
            "acct: longest commit {:.3} ms: node {} {} [defs, contribs, reads, placed] {:?} {head}",
            ms(t.commit_max_ns),
            n.raw(),
            i.map_or("gone", |i| i.op),
            eng.record_sizes(n)
        );
    }
    let live = eng.live_nodes().max(1);
    #[allow(clippy::cast_precision_loss)]
    let rpn = r.runs as f64 / live as f64;
    eprintln!(
        "acct: {} runs, {} nodes run, {} live nodes, {rpn:.2} runs/node, {} refuted, {} relations resolved, {} shaped, {} steals, {} segments",
        r.runs, r.nodes_run, live, r.refuted_nodes, r.relative_resolved, r.shaped, r.steals, r.segments
    );
    let kinds: Vec<String> = by_kind
        .iter()
        .filter(|k| k.2 > 0)
        .map(|(k, d, n)| format!("{k} {:.3} ms/{n}", d.as_secs_f64() * 1e3))
        .collect();
    eprintln!("acct: op time by kind: {}", kinds.join(", "));
    let mut by_field: Vec<(String, u64)> = r
        .refuted_by_field
        .iter()
        .map(|&(f, n)| (crate::val::field_name(eng.values(), f), n))
        .collect();
    by_field.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    by_field.truncate(20);
    eprintln!("acct: {} nodes refuted; refuted by field {by_field:?}", r.refuted_nodes);
    if crate::exec::prof_flag!("PHITEX_RUN_AUDIT") {
        print_rerun_classes(eng, r);
    }
    // (the kinds on the engine's critical chain, and why each link waited)
    let mut on_chain: std::collections::BTreeMap<&'static str, usize> = std::collections::BTreeMap::new();
    let chain = eng.critical_chain();
    let mut links: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut first = Vec::new();
    for (n, c) in &chain {
        let op = eng.node_info(*n).map_or("?", |i| i.op);
        *on_chain.entry(op).or_default() += 1;
        let c = cause_parts(c).0;
        *links.entry(format!("{op} {c}")).or_default() += 1;
        if first.len() < 40 {
            first.push(format!("{op}:{c}"));
        }
    }
    // (`PHITEX_CHAIN_ALL=1`, profile: every link of the chain, its field
    // by name, and its run, commit, re-ask and gap before it)
    if crate::exec::prof_flag!("PHITEX_CHAIN_ALL") {
        let times: FxHashMap<phitex_defuse::NodeId, [u64; 4]> = eng.critical_chain_times().into_iter().collect();
        let v = eng.values();
        for (n, c) in &chain {
            let op = eng.node_info(*n).map_or("?", |i| i.op);
            let (k, f) = cause_parts(c);
            let f = f.map(|f| crate::val::field_name(v, f)).unwrap_or_default();
            let t = times.get(n).copied().unwrap_or_default();
            eprintln!(
                "acct-chain: {op} {} run {:.3} commit {:.3} reask {:.3} gap {:.3} ms {k} {f}",
                n.raw(),
                ms(t[0]),
                ms(t[1]),
                ms(t[2]),
                ms(t[3]),
            );
        }
    }
    eprintln!("acct: critical chain {} links by kind and cause {links:?}; first: {}", chain.len(), first.join(" "));
    eprintln!(
        "acct: critical path {} nodes, {:.3} ms; chain kinds {on_chain:?}; env calls {env_calls}",
        r.realised_critical_path,
        r.realised_critical_time.as_secs_f64() * 1e3
    );
    {
        // (the client's locks runs found held, by call site)
        let waits = crate::exec::lock_waits();
        let all: std::time::Duration = waits.iter().map(|w| w.2).sum();
        let top: Vec<String> = waits
            .iter()
            .take(12)
            .map(|(at, n, d)| format!("{at} {:.3} ms/{n}", d.as_secs_f64() * 1e3))
            .collect();
        eprintln!("acct: client lock waits {:.3} ms in all; by site: {}", all.as_secs_f64() * 1e3, top.join(", "));
    }
    let ld = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "acct: commands {} (exec {}, finish {}, resume {}, other {}); exec claims declared: none {}, a count {}, pending {}",
        ld(&crate::ops::COMMANDS),
        ld(&crate::ops::COMMANDS_BY[0]),
        ld(&crate::ops::COMMANDS_BY[1]),
        ld(&crate::ops::COMMANDS_BY[2]),
        ld(&crate::ops::COMMANDS_BY[3]),
        ld(&crate::ops::EXEC_CLAIMS[0]),
        ld(&crate::ops::EXEC_CLAIMS[1]),
        ld(&crate::ops::EXEC_CLAIMS[2])
    );
    let mut longest: Vec<(phitex_defuse::NodeId, std::time::Duration)> = r.run_times.clone();
    longest.sort_by_key(|x| std::cmp::Reverse(x.1));
    let longest: Vec<String> = longest
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
    eprintln!("acct: stops by reader and field (top 15): {:?}", crate::run::take_stops(eng.values(), 15));
}

/// An evaluation's counters (`PHITEX_TEX_TIME`).
fn print_round(t: std::time::Duration, r: &Report) {
    // (why nodes ran again, by kind: `Report::rerun_causes`)
    let mut causes = std::collections::BTreeMap::<&'static str, usize>::new();
    if crate::exec::obs_flag!("PHITEX_RERUN_SHOW") {
        for (n, c) in &r.rerun_causes {
            eprintln!("rerun: node {} {c:?}", n.raw());
        }
    }
    for (_, c) in &r.rerun_causes {
        *causes.entry(cause_parts(c).0).or_default() += 1;
    }
    eprintln!(
        "finishers: {} created with a prediction, {} without",
        crate::pages::FIN_PREDICTED.load(std::sync::atomic::Ordering::Relaxed),
        crate::pages::FIN_UNPREDICTED.load(std::sync::atomic::Ordering::Relaxed),
    );
    eprintln!("{}", crate::pages::Feedback::report());
    eprintln!(
        "lines: {} runs' reads of the lines before them taken back",
        crate::control::LINES_TAKEN.load(std::sync::atomic::Ordering::Relaxed)
    );
    let mut longest: Vec<(phitex_defuse::NodeId, std::time::Duration)> = r.run_times.clone();
    longest.sort_by_key(|x| std::cmp::Reverse(x.1));
    eprintln!(
        "round rerun causes: {} nodes, {causes:?}, realised cp {} nodes {:?}, longest runs {:?}",
        r.rerun_causes.len(),
        r.realised_critical_path,
        r.realised_critical_time,
        longest.iter().take(5).map(|&(n, t)| (n.raw(), t)).collect::<Vec<_>>()
    );
    eprintln!(
        "round: {t:?}, {} nodes run, {} runs, {} first runs, {} commits, {} edits, by op {:?}",
        r.nodes_run, r.runs, r.first_runs, r.commits, r.edits, r.runs_by_op
    );
    eprintln!(
        "round predictions: {} confirmed, {} refuted; {} relations resolved, {} shaped; {} refuted nodes",
        r.predictions_confirmed, r.predictions_refuted, r.relative_resolved, r.shaped, r.refuted_nodes
    );
}

impl Stats {
    /// One evaluation's wall time on `threads` threads, and the
    /// operations' counters since the last ([`crate::exec::take_counters`]).
    fn add_exec(&mut self, threads: usize, wall: std::time::Duration, by_kind: &[(&'static str, std::time::Duration, u64)], busy: std::time::Duration, peak: usize) {
        self.wall += wall;
        self.threads = threads;
        self.busy += busy;
        for &(k, d, n) in by_kind {
            match self.op_by_kind.iter_mut().find(|x| x.0 == k) {
                Some(x) => {
                    x.1 += d;
                    x.2 += n;
                }
                None => self.op_by_kind.push((k, d, n)),
            }
        }
        self.peak_running = self.peak_running.max(peak);
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
        let v = eng.values();
        for (n, c) in &r.rerun_causes {
            let op = eng.node_info(*n).map_or("?", |i| i.op);
            let (kind, f) = cause_parts(c);
            let mut why = f.map_or_else(|| kind.to_owned(), |f| format!("{kind} {}", field_name(v, f)));
            if crate::exec::prof_flag!("PHITEX_RUN_LOG") {
                why = format!("{why} #r{}", self.rounds);
            }
            self.reruns.push((n.raw(), op, why));
        }
        if crate::exec::prof_flag!("PHITEX_RUN_LOG") {
            let log = std::mem::take(&mut *crate::ops::RUN_LOG.locked());
            let round = self.rounds;
            for (n, resumed, spec, how) in log {
                let op = eng.node_info(phitex_defuse::NodeId::from_raw(n)).map_or("?", |i| i.op);
                self.run_log.push((n, op, resumed, spec, how, round));
            }
        }
    }

    fn add(&mut self, r: &Report) {
        self.piece_times.extend(
            r.run_times
                .iter()
                .map(|(n, t)| (n.raw(), u64::try_from(t.as_micros()).unwrap_or(u64::MAX))),
        );
        self.nodes_run += r.nodes_run;
        self.runs += r.runs;
        self.rounds += 1;
        // (the passes the files read back took: one, and each rerun a
        // node took on them)
        self.loop_rounds = self.loop_rounds.max(u64::from(r.passes));
        self.pass_runs.clone_from(&r.pass_runs);
        self.version = r.version;
        self.predictions_confirmed += r.predictions_confirmed;
        self.predictions_refuted += r.predictions_refuted;
        self.round_times.push(r.times);
        self.realised_critical_path = self.realised_critical_path.max(r.realised_critical_path);
        // (the chain of runs that waited on each other, in nodes)
        self.critical_path = Some(self.realised_critical_path as usize);
        if r.realised_critical_time >= self.realised_critical_time {
            self.realised_critical_time = r.realised_critical_time;
            let mut ts = r.run_times.clone();
            ts.sort_by_key(|x| std::cmp::Reverse(x.1));
            self.run_times = ts.iter().take(10).map(|&(n, t)| (n.raw(), t)).collect();
        }
        self.page_steps += Report::by_op(&r.runs_by_op, "finish");
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
    /// The files the job writes whose text the client gave as the build
    /// began ([`Options::written_init`]): their loop-backs are declared by
    /// the run that reads them back first (`crate::run::Run::seed_written`).
    seeds: Vec<(String, String)>,
    /// The files the last evaluation's runs opened for writing: their
    /// nodes are placed from the streams (`ops::Files`), so `loopfiles`
    /// lists only the files written before and not opened now.
    opened_last: Vec<String>,
    /// The executor's threads ([`Options::threads`]).
    threads: usize,
    /// Whether the next evaluation runs on the calling thread alone: an
    /// edit's few nodes, no pool woken ([`INLINE_NODES`]).
    inline: bool,
    /// The comments of the document's canonical text
    /// ([`canonical_text`]): `doc:mark:<id>` in the environment, read by a
    /// run that reads a comment's mark as text (`Run::splice_mark`).
    marks: Marks,
    /// The names of the files opened for writing ([`Graph::opened`]), by
    /// the `streams` value they were read from.
    opened_memo: std::sync::Mutex<Option<(Option<ValueId>, Vec<String>)>>,
    /// The guessed key sets of the read-back files that only define, the
    /// environment's `wkeys:<name>` until their files' nodes define them.
    wkeys_guess: Vec<(String, Vec<String>)>,
    /// What the executor runs now, for readers on other threads.
    watch: crate::exec::Watch,
    /// The files the environment provider found or missed during runs
    /// (name, field, value, hash), merged into `files`/`absent`/`reads`
    /// before the environment is set again (`Graph::provide`).
    provided: Arc<std::sync::Mutex<Vec<Provided>>>,
    /// The package loads' records and hints (`crate::pkgrec`), once the
    /// build has set them.
    loads: Option<Arc<crate::pkgrec::LoadHints>>,
    /// The format's bindings as the environment's base ([`job_env`]):
    /// asked before the files ([`Graph::provide`]).
    env_base: Option<EnvBase>,
}

/// The format's bindings by field, each made when first asked.
type EnvBase = Arc<dyn Fn(&phitex_defuse::Values, Field) -> Option<ValueId> + Send + Sync>;

/// A file the environment provider answered: name, field, the value
/// (`None`: absent), and the finder's hash.
type Provided = (String, Field, Option<ValueId>, u64);

/// A document file a document's `\input` reads, cut as the document's
/// text is: its slices, by the same analysis and keys ([`cut_text`]),
/// are the pieces each run reaching the file places, their texts their
/// arguments ([`doc_file_pieces`], [`node_args`]); an edit of the file
/// edits those pieces as an edit of the document edits its nodes
/// ([`apply_slices`], [`Build::stage_file`]). The runs read
/// `dfile:<name>` = `[path, 1]` ([`cut_doc_file`]), never its text, so
/// an edit inside it runs its pieces it changed, not their reader.
pub(crate) struct DocFile {
    path: String,
    text: String,
    incr: Incr,
    /// Its slices and their keys.
    slices: Vec<(u64, DocNode)>,
    /// The nodes whose runs placed its pieces ([`doc_file_pieces`]).
    readers: Vec<NodeId>,
    /// The pieces an edit kept where its text moved, by node: their
    /// arguments' frames and how far the text moved since they were
    /// made (a kept piece keeps its text and the places in it, as a
    /// document's node does, [`apply_slices`]).
    shifts: FxHashMap<u64, (ValueId, i64)>,
    /// Each reader's pieces as the last edit left them, in order.
    placed: FxHashMap<u64, Vec<NodeId>>,
    /// Each slice's arguments as its pieces hold them: a slice an edit
    /// kept where its text moved keeps the arguments it was made with
    /// ([`DocFile::shifts`]), so a reader running again places the piece
    /// it placed, not one moved; none: made from the text.
    args: Vec<Option<Vec<ValueId>>>,
}

/// The build's cut document files, by name ([`DocFile`]).
pub(crate) type DocFiles = Arc<std::sync::Mutex<FxHashMap<String, Arc<std::sync::Mutex<DocFile>>>>>;

/// The provider's answers for cut files in an evaluation, by name: each
/// file cut once.
type DocCuts = std::sync::Mutex<FxHashMap<String, Arc<OnceLock<Option<ValueId>>>>>;

/// Document file `name` (at `path`, its `bytes`) cut ([`DocFile`]):
/// `dfile:<name>`'s value, `[path, 1]`; a file that is not a document's
/// text (`inputs::document_file`) is not cut, `[path, 0]`, its readers
/// read its text.
fn cut_doc_file(
    v: &Values,
    reg: &crate::hints::Registry,
    defs: &Defs,
    docs: &DocFiles,
    name: &str,
    path: &str,
    bytes: &[u8],
) -> ValueId {
    let text = latin1(bytes);
    let pv = v.str(path);
    if !crate::inputs::document_file(path, &text) {
        docs.locked().remove(name);
        return v.list(vec![pv, v.int(0)]);
    }
    let mut incr = Incr::new(defs.clone());
    let slices = cut_text(reg, v, &text, None, Some(&mut incr), Some(pv));
    let df = DocFile {
        path: path.to_owned(),
        text,
        incr,
        slices,
        readers: Vec::new(),
        shifts: FxHashMap::default(),
        placed: FxHashMap::default(),
        args: Vec::new(),
    };
    docs.locked()
        .insert(name.to_owned(), Arc::new(std::sync::Mutex::new(df)));
    v.list(vec![pv, v.int(1)])
}

/// The pieces of cut document file `key` a run reaching it places
/// (`Run::doc_file_regions`): its slices with the arguments the
/// document's nodes get ([`node_args`]); the run's node `node` is kept
/// among the file's readers, whose pieces an edit of the file edits
/// ([`Build::stage_file`]).
pub(crate) fn doc_file_pieces(
    v: &Values,
    docs: &DocFiles,
    key: &str,
    node: NodeId,
    loads: Option<&crate::pkgrec::LoadHints>,
) -> Vec<Vec<ValueId>> {
    let Some(df) = docs.locked().get(key).cloned() else {
        return Vec::new();
    };
    let mut df = df.locked();
    if !df.readers.contains(&node) {
        df.readers.push(node);
    }
    let name = v.str(&df.path);
    let df = &mut *df;
    df.args.resize(df.slices.len(), None);
    df.slices
        .iter()
        .zip(df.args.iter_mut())
        .map(|((_, n), a)| {
            a.get_or_insert_with(|| node_args(v, &df.text, Some(&df.incr), n, loads, Some(name)))
                .clone()
        })
        .collect()
}

/// The arguments of slices `new` an edit made of `old` (`args` theirs,
/// [`DocFile::args`]): those of the ends the two share kept, the others
/// none (made from the text).
fn kept_args(
    old: &[(u64, DocNode)],
    new: &[(u64, DocNode)],
    args: &[Option<Vec<ValueId>>],
) -> Vec<Option<Vec<ValueId>>> {
    let mut a = 0;
    while a < old.len() && a < new.len() && old[a].0 == new[a].0 {
        a += 1;
    }
    let mut b = 0;
    while b < old.len() - a && b < new.len() - a && old[old.len() - 1 - b].0 == new[new.len() - 1 - b].0 {
        b += 1;
    }
    (0..new.len())
        .map(|i| {
            let oi = if i < a {
                Some(i)
            } else if i >= new.len() - b {
                Some(i + old.len() - new.len())
            } else {
                None
            };
            oi.and_then(|oi| args.get(oi).cloned().flatten())
        })
        .collect()
}

/// The pieces of the cut file named `name` (its path, a string value)
/// that node `r` placed, in order ([`doc_file_pieces`]).
fn pieces_of(eng: &Engine, r: NodeId, name: ValueId) -> Vec<NodeId> {
    let v = eng.values();
    eng.children(r)
        .into_iter()
        .filter(|&k| {
            eng.node_info(k).is_some_and(|i| {
                i.args.len() == 3
                    && i.args.first().is_some_and(|&fs| {
                        list_of(v, fs).first().is_some_and(|&f| {
                            let xs = list_of(v, f);
                            xs.first().is_some_and(|&t| str_of(v, t) == "s") && xs.get(5) == Some(&name)
                        })
                    })
            })
        })
        .collect()
}

/// Whether `kids` are still node `r`'s placements, in order and one
/// after another ([`DocFile::placed`]).
fn placed_still(eng: &Engine, r: NodeId, kids: &[NodeId]) -> bool {
    let ch = eng.children(r);
    ch.iter()
        .position(|&k| k == kids[0])
        .is_some_and(|p| ch.get(p..p + kids.len()) == Some(kids))
}

/// How far an edit of its file moved the text of node `n` or of the
/// piece it is under ([`DocFile::shifts`]): 0 where none did.
fn piece_shift(eng: &Engine, shifts: &FxHashMap<u64, (ValueId, i64)>, n: NodeId) -> i64 {
    if shifts.is_empty() {
        return 0;
    }
    let mut n = n;
    loop {
        let Some(i) = eng.node_info(n) else { return 0 };
        if let Some(&(fs, d)) = shifts.get(&n.raw())
            && i.args.first() == Some(&fs)
        {
            return d;
        }
        match i.parent {
            Some(p) => n = p,
            None => return 0,
        }
    }
}



impl Graph {
    #[allow(clippy::too_many_lines)]
    fn new(time: Option<std::time::Duration>, threads: Option<usize>) -> Graph {
        let eng = Engine::new();
        eng.set_limits(phitex_defuse::Limits {
            time,
            ..phitex_defuse::Limits::default()
        });
        let shared: Shared = Arc::new(OnceLock::new());
        let exec = eng.register(Arc::new(Exec(shared.clone())));
        let put = eng.register(Arc::new(Put(shared.clone())));
        let pages = eng.register(Arc::new(crate::pages::Pages(shared.clone())));
        let finish = eng.register(Arc::new(crate::pages::Finish(shared.clone())));
        let resume = eng.register(Arc::new(crate::pages::Resume(shared.clone())));
        let brk = eng.register(Arc::new(crate::parbreak::Break(shared.clone())));
        let dvi = eng.register(Arc::new(Dvi(shared.clone())));
        let link = eng.register(Arc::new(crate::ops::Link(shared.clone())));
        let mark = eng.register(Arc::new(Mark));
        let files_op = eng.register(Arc::new(Files(shared.clone())));
        let tool = eng.register(Arc::new(crate::tools::ToolRun(shared.clone())));
        let toolnode = eng.register(Arc::new(crate::tools::ToolNode(shared.clone())));
        let pdfpage = eng.register(Arc::new(crate::pdfout::PdfPage));
        let fields = Arc::new(crate::fields::Fields::new(eng.values()));
        let class_proj = eng.register_projection(Arc::new(crate::val::class_projection));
        let outer_proj = eng.register_projection(Arc::new(crate::val::outer_projection));
        let step_proj = eng.register_projection(Arc::new(crate::val::step_projection));
        let defined_proj = eng.register_projection(Arc::new(crate::val::defined_projection));
        let tail_proj = eng.register_projection(Arc::new(crate::val::tail_projection));
        let num_proj = eng.register_projection(Arc::new(crate::val::num_projection));
        let clist_proj = eng.register_projection(Arc::new(crate::val::clist_projection));
        let ops = Ops {
            exec,
            put,
            pages,
            finish,
            resume,
            brk,
            tool,
            toolnode,
            pdfpage,
            tool_opts: Arc::default(),
            fields,
            defs: Arc::new(Mutex::new(crate::hints::no_defs().clone())),
            hints: Arc::default(),
            pkg_log: Arc::new(Mutex::new(Vec::new())),
            rec_ran: Arc::default(),
            pkg_loads: Arc::new(std::sync::RwLock::new(None)),
            docs: Arc::default(),
            out_ok: Arc::new(Mutex::new(None)),
            fin_defs: Arc::default(),
            fin_defs_start: Arc::default(),
            fin_predict: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            counters: Arc::default(),
            scope_learned: Arc::default(),
            node_rec: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            pdf_cmds: Arc::default(),
            class_proj,
            outer_proj,
            step_proj,
            defined_proj,
            tail_proj,
            num_proj,
            clist_proj,
        };
        let _ = shared.set(ops.clone());
        declare_fields(&eng);
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
            seeds: Vec::new(),
            threads: crate::exec::threads(threads),
            inline: false,
            marks: Marks::default(),
            opened_memo: std::sync::Mutex::new(None),
            wkeys_guess: Vec::new(),
            opened_last: Vec::new(),
            watch: crate::exec::Watch::new(crate::exec::threads(threads), None),
            eval_errors: Vec::new(),
            eval_warnings: Vec::new(),
            tool_finder: None,
            provided: Arc::default(),
            loads: None,
            env_base: None,
        }
    }

    /// An evaluation error as a reader can act on it: the error, and
    /// each node it names with what it is.
    fn diagnose(&self, e: &phitex_defuse::EvalError, unit: UnitId) -> String {
        use phitex_defuse::EvalError as E;
        let _ = unit;
        let mut ids: Vec<NodeId> = Vec::new();
        match e {
            E::RunawayNode { node, .. } | E::OpPanicked { node, .. } => ids.push(*node),
            E::TotalRuns { .. } | E::Deadline => {}
        }
        // (the field and the nodes' texts first: a harness shows the
        // head of the message; the error's full form last)
        let mut out = format!("evaluation: {e}");
        let v = self.eng.values();
        // (a runaway node's last causes, their fields named)
        if let E::RunawayNode { node, .. } = e {
            let r = self.eng.debug_reads(*node);
            let _ = write!(out, "\n  its reads now: {}", r.lines().take(20).collect::<Vec<_>>().join("; "));
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

    /// Installs the environment provider: a run reading a file the
    /// environment does not hold gets it from the finder in place, as
    /// [`Graph::supply`] would have supplied it, instead of stopping;
    /// a file the job writes is not answered (it is read back).
    #[allow(clippy::too_many_lines)]
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
        // (a document file cut, [`DocFile`]: cut once, `dfile:` answered
        // from the cut)
        let cut: DocCuts = std::sync::Mutex::new(FxHashMap::default());
        let cut_log = Arc::clone(&self.provided);
        let (reg, defs) = (Arc::clone(&self.ops.hints), Arc::clone(&self.ops.defs));
        let docs = Arc::clone(&self.ops.docs);
        let finder_u = Arc::clone(&finder);
        let finder_t = Arc::clone(&finder);
        let tools = self.tool_finder.clone();
        let base = self.env_base.clone();
        self.eng.set_env_provider(Box::new(move |v, f| {
            if let Some(x) = base.as_ref().and_then(|b| b(v, f)) {
                return Some(Some(x));
            }
            let field = v.as_str(f.0)?;
            // (a cut file's size, `\pdffilesize`'s answer without its
            // text)
            if let Some(name) = field.strip_prefix("fsize:") {
                if written.iter().any(|w| w == name) {
                    return None;
                }
                let found = finder_u(name);
                let hash = file_hash(found.as_ref());
                let val = found.map(|(_, b)| v.int(i64::try_from(b.len()).unwrap_or(i64::MAX)));
                if let Ok(mut l) = cut_log.lock() {
                    l.push((name.to_owned(), f, val, hash));
                }
                return Some(val);
            }
            if let Some(name) = field.strip_prefix("dfile:") {
                if written.iter().any(|w| w == name) {
                    return None;
                }
                let cell = cut
                    .locked()
                    .entry(name.to_owned())
                    .or_default()
                    .clone();
                return Some(*cell.get_or_init(|| {
                    let found = finder_u(name);
                    let hash = file_hash(found.as_ref());
                    let defs = defs.locked().clone();
                    let val = found.map(|(path, bytes)| {
                        cut_doc_file(v, &reg, &defs, &docs, name, &path, &bytes)
                    });
                    if let Ok(mut l) = cut_log.lock() {
                        l.push((name.to_owned(), f, val, hash));
                    }
                    val
                }));
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
                .locked()
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
            // names the finder is asked for again; a file read both cut
            // and whole, [`DocFile`], listed once)
            if !name.starts_with("tfile:") && !self.reads.iter().any(|(n, _)| *n == name) {
                self.queries.push((name.clone(), val.is_some()));
                self.reads.push((name, hash));
            }
            match val {
                Some(x) => self.files.push((f, x)),
                None => self.absent.push(f),
            }
        }
    }

    /// A file's whole text as the build holds it: its `file:` value (by
    /// its name or its path), else a cut file's ([`DocFile`]).
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
        let docs = self.ops.docs.locked();
        docs.iter()
            .find(|(n, d)| *n == name || d.locked().path == name)
            .map(|(_, d)| d.locked().text.as_str().into())
    }

    /// A cut file ([`DocFile`]) dropped: the file the job writes, read
    /// back.
    fn drop_cut(&mut self, name: &str) {
        let d = self.eng.values().field(&format!("dfile:{name}"));
        self.files.retain(|&(g, _)| g != d);
        self.absent.retain(|&g| g != d);
        self.ops.docs.locked().remove(name);
    }

    /// The names of the files the job opened for writing.
    fn opened(&self, unit: UnitId) -> Vec<String> {
        let v = self.eng.values();
        let f = v.field("streams");
        let streams = self.eng.acc_value(unit, f);
        let mut m = self.opened_memo.locked();
        if let Some((from, names)) = &*m
            && *from == streams
        {
            return names.clone();
        }
        let entries = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
        let names: Vec<String> = stream_texts(v, &entries)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        *m = Some((streams, names.clone()));
        names
    }

    /// Round one's guesses for the loop-backs: the last build's values
    /// (`opts`); none without one.
    fn guesses(&mut self, opts: &Options) {
        if opts.fixpoint {
            self.seeds.clone_from(&opts.written_init);
        }
        // (a file read back from its text needs no guess of its keys)
        self.wkeys_guess = opts
            .wkeys
            .iter()
            .filter(|(n, _)| !self.seeds.iter().any(|(m, _)| m == n))
            .cloned()
            .collect();
    }

    /// A file the job writes is read back as the job writes it
    /// (`fixpoint`: its fields, `crate::ops::Files`, read at their last
    /// definition), or absent; `file:<name>` is absent, so a run that
    /// read it from the finder before this runs again and reads them.
    fn written_file(&mut self, name: &str, fixpoint: bool, _finder: &Finder) {
        if self.written.iter().any(|w| w == name) {
            return;
        }
        let f = self.eng.values().field(&format!("file:{name}"));
        self.files.retain(|(g, _)| *g != f);
        self.absent.retain(|g| *g != f);
        self.reads.retain(|(n, _)| n != name);
        self.drop_cut(name);
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
        self.written.push(name.to_owned());
    }

    /// Pushes the environment (spec-final §4.5): once per evaluation,
    /// and only when it changed since the last push (the entries and the
    /// absent fields compared as sorted lists; an unchanged one is no
    /// call to the engine).
    fn push_env(&mut self, env: Vec<(Field, ValueId)>) {
        // (a field given twice: the later entry, as the engine's map has
        // it; the job's bindings first, by field once for the job
        // ([`job_env`]), merged with the few entries here: the format's
        // bindings neither hashed nor sorted again per push)
        let env = merge_by_field(&self.job, &by_field(env));
        let mut absent = self.absent.clone();
        absent.sort_unstable_by_key(|f| f.0.raw());
        absent.dedup();
        crate::timeline::lap("env: merged with the job's");
        if env != self.env_pushed {
            // (a few fields changed, an edit's file: those alone, not
            // the whole environment again)
            match env_changes(&self.env_pushed, &env) {
                Some(cs) if !self.env_pushed.is_empty() => self.eng.update_environment(&cs),
                _ => self.eng.set_environment(env.iter().copied()),
            }
            crate::timeline::lap("env: set in the engine");
            self.env_calls += 1;
            self.env_pushed = env;
        }
        if absent != self.absent_pushed {
            let gone: Vec<(Field, Option<ValueId>)> = absent.iter().map(|&f| (f, None)).collect();
            self.eng.update_environment(&gone);
            self.env_calls += 1;
            self.absent_pushed = absent;
        }
    }

    /// The environment: the files supplied, and the loop-back files.
    fn environment(&self, fixpoint: bool) -> Vec<(Field, ValueId)> {
        let v = self.eng.values();
        let mut env = self.files.clone();
        env.extend(self.marks.entries(v));
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
            // (the texts the client gave for the files the job writes,
            // read back in round one where the job first reads each)
            env.push((
                v.field(crate::run::SEEDS),
                v.list(self.seeds.iter().map(|(n, _)| v.str(n)).collect()),
            ));
            for (n, t) in &self.seeds {
                let t = v.str(t);
                env.push((v.field(&format!("{}:{n}", crate::run::SEEDS)), t));
                // (its text and size as the client gave it, for its tools
                // and lookups, and written as given until its node finds
                // otherwise: its readers read the text,
                // `crate::ops::WSAME`)
                env.push((crate::tools::text_field(v, n), t));
                env.push((crate::tools::size_field(v, n), crate::tools::size_value(v, t)));
                env.push((v.field(&format!("{}:{n}", crate::ops::WSAME)), v.bool(true)));
            }
            // (a key set guessed for a file written before, until its node
            // defines the file's)
            for (n, ks) in &self.wkeys_guess {
                if self.written.contains(n) {
                    env.push((v.field(&format!("wkeys:{n}")), v.list(ks.iter().map(|k| v.str(k)).collect())));
                }
            }
        } else {
            env.push((v.field("loopfiles"), ValueId::UNIT));
            env.push((v.field(crate::run::SEEDS), v.list(Vec::new())));
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
                let fd = o.fin_defs.locked().clone();
                *o.fin_defs_start.locked() = Arc::new(fd);
            }
            let t0 = std::time::Instant::now();
            // (an edit's few nodes: on this thread, no worker started,
            // [`INLINE_NODES`])
            let n = if self.inline { 1 } else { self.threads };
            // (`PHITEX_MEM_LOG=secs`, profile: every `secs` seconds of the
            // evaluation, the resident memory, the value store and the live
            // nodes)
            let mem_log = crate::exec::env_str!("PHITEX_MEM_LOG").and_then(|x| x.parse::<f64>().ok()).filter(|_| cfg!(feature = "profile"));
            let done = std::sync::atomic::AtomicBool::new(false);
            let res = std::thread::scope(|sc| {
                if let Some(secs) = mem_log {
                    let (eng, done) = (&self.eng, &done);
                    sc.spawn(move || {
                        let t = std::time::Instant::now();
                        let mut next = secs;
                        while !done.load(std::sync::atomic::Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            if t.elapsed().as_secs_f64() < next {
                                continue;
                            }
                            next += secs;
                            let (n, b, sq) = eng.values().footprint();
                            eprintln!(
                                "mem-log: {:.0} s: resident {} MB; values {n} ({} MB payload), seq nodes {sq}; {}",
                                t.elapsed().as_secs_f64(),
                                mem_mb().0,
                                b >> 20,
                                eng.footprint()
                            );
                        }
                    });
                }
                let r = self.eng.evaluate_absorbing(n, edits);
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                r
            });
            {
                // (the PDF commands no record names any more: dropped now,
                // with no run in work)
                let v = self.eng.values();
                if let Some(line) = self.eng.acc_value(unit, v.field(crate::pdfout::XPAGES)) {
                    let line: Vec<ValueId> = crate::val::CatMemo::default().walk(v, line, Some);
                    crate::pdfout::prune_cmds(v, &line, &self.ops.pdf_cmds);
                }
            }
            if let (Ok(r), Some(path)) = (&res, crate::exec::env_str!("PHITEX_CHAIN_TSV")) {
                dump_chain(&self.eng, r, path);
            }
            let (by_kind, busy, peak) = crate::exec::take_counters();
            crate::exec::after_evaluation();
            let r = match res {
                Ok(r) => r,
                Err(e) => {
                    debug_nodes(&self.eng, "at the evaluation's error");
                    // the build ends here; the error is a diagnostic
                    let m = self.diagnose(&e, unit);
                    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                        eprintln!("{m}");
                    }
                    stats.rounds += 1;
                    stats.add_exec(n, t0.elapsed(), &by_kind, busy, peak);
                    stats.live = self.eng.live_nodes();
                    self.eval_errors.push(m);
                    stats.add_hints(&self.eng, unit, &self.eval_errors);
                    return;
                }
            };
            t_eval += t0.elapsed();
            self.eval_warnings.clear();
            // (a reader of a file read back still moving at the cap: the
            // output is its last run's, as latexmk's is after its last
            // run, and LaTeX's warning says so)
            if !r.capped.is_empty() {
                let v = self.eng.values();
                let mut moving: Vec<String> = r
                    .rerun_causes
                    .iter()
                    .filter(|(n, _)| r.capped.binary_search(n).is_ok())
                    .filter_map(|(_, c)| cause_parts(c).1)
                    .map(|f| crate::val::field_name(v, f))
                    .collect();
                moving.sort_unstable();
                moving.dedup();
                self.eval_warnings.push(format!(
                    "Label(s) may have changed: the files read back still changed after {} rounds; still changing: {}",
                    self.eng.limits().final_reruns + 1,
                    moving.join(", ")
                ));
            }
            for (k, d, n) in &by_kind {
                if *n > 0 {
                    crate::timeline::note(&format!("runs: {k} ×{n} (summed over threads)"), *d);
                }
            }
            {
                let ns = std::time::Duration::from_nanos;
                crate::timeline::note("evaluation: wall", ns(r.times.wall_ns));
                crate::timeline::note("evaluation: commit (summed)", ns(r.times.commit_ns));
                crate::timeline::note("evaluation: re-ask (summed)", ns(r.times.reask_ns));
            }
            crate::timeline::lap("engine: evaluate (the runs, their commits)");
            t_mark = std::time::Instant::now();
            crate::timeline::lap("engine: the evaluation's report");
            // (debug, `PHITEX_RERUN_CHECK=1`: every committed node run
            // again on the line as it stands; a product that differs from
            // its record is a dependency its reads did not record)
            if crate::exec::obs_flag!("PHITEX_RERUN_CHECK") {
                let lines = self.eng.rerun_check();
                for l in &lines {
                    eprintln!("rerun_check: {l}");
                }
                eprintln!("rerun_check: {} differ", lines.len());
            }
            if crate::exec::obs_flag!("PHITEX_CHECK_READS") || crate::exec::obs_flag!("PHITEX_CHECK_INVARIANTS") {
                // (debug: every committed node's reads and relations checked
                // again after this evaluation; `PHITEX_CHECK_READS=fail`: a
                // stale one fails, the tests' setting)
                let tc = std::time::Instant::now();
                let lines = self.eng.check_invariants();
                for l in &lines {
                    eprintln!("check_reads: {l}");
                }
                eprintln!("check_reads: {} stale (checked in {:?})", lines.len(), tc.elapsed());
                assert!(
                    lines.is_empty() || crate::exec::obs_str!("PHITEX_CHECK_READS") != Some("fail"),
                    "{} committed reads no longer hold",
                    lines.len()
                );
                crate::timeline::lap("engine: the reads checked again (PHITEX_CHECK_READS only)");
            }
            if crate::exec::prof_flag!("PHITEX_ROUND_ACCT") {
                // (the edit's cost: its runs, the nodes run, the runs by
                // operation, where the time went)
                let mut ops: Vec<(&str, u64)> = r.runs_by_op.iter().copied().filter(|x| x.1 > 0).collect();
                ops.sort_unstable();
                let ms = |x: u64| std::time::Duration::from_nanos(x).as_secs_f64() * 1e3;
                eprintln!(
                    "acct-edit: wall {:.1} ms, {} runs, {} nodes run, {} commits, runs by op {ops:?}; op {:.1} ms commit {:.1} ms re-ask {:.1} ms idle {:.1} ms lock wait {:.1} ms",
                    t0.elapsed().as_secs_f64() * 1e3,
                    r.runs,
                    r.nodes_run,
                    r.commits,
                    ms(r.times.op_ns),
                    ms(r.times.commit_ns),
                    ms(r.times.reask_ns),
                    ms(r.times.idle_ns),
                    ms(r.times.lock_wait_ns),
                );
                if crate::exec::prof_flag!("PHITEX_RUN_AUDIT") {
                    print_rerun_classes(&self.eng, &r);
                }
                let v = self.eng.values();
                let mut by: Vec<(String, u64)> = r
                    .refuted_by_field
                    .iter()
                    .map(|&(f, n)| (crate::val::field_name(v, f), n))
                    .collect();
                by.sort_by_key(|e| std::cmp::Reverse(e.1));
                by.truncate(8);
                eprintln!(
                    "acct-edit: guesses: refuted nodes {} wasted {:.1} ms; refuted by field {by:?}; relations resolved {} shaped {}",
                    r.refuted_nodes,
                    ms(r.wasted_ns),
                    r.relative_resolved,
                    r.shaped
                );
            }
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                print_round(t0.elapsed(), &r);
                crate::exec::flush_later();
                print_accounting(&self.eng, &r, n, &by_kind, self.env_calls);
                {
                    // (the reruns by cause and field, most first)
                    let v = self.eng.values();
                    let mut by = FxHashMap::<String, usize>::default();
                    for (node, c) in &r.rerun_causes {
                        let (k, f) = cause_parts(c);
                        let name = f.map_or_else(|| "-".to_owned(), |f| crate::val::field_name(v, f));
                        if crate::exec::obs_flag!("PHITEX_RERUN_OPS") {
                            let d: String = describe(&self.eng, *node).chars().take(60).collect();
                            eprintln!("rerunop: {} [{d}] {k} {name}", node.raw());
                        }
                        *by.entry(format!("{k} {name}")).or_default() += 1;
                    }
                    let mut by: Vec<_> = by.into_iter().collect();
                    by.sort_by_key(|a| std::cmp::Reverse(a.1));
                    by.truncate(12);
                    eprintln!("  reruns by field {by:?}");
                }
            }
            crate::timeline::lap("engine: the accounting printed (PHITEX_TEX_TIME only)");
            stats.add(&r);
            stats.env_calls = self.env_calls;
            stats.add_reruns(&self.eng, &r);
            stats.add_refuted(&self.eng, &r);
            stats.add_exec(n, t0.elapsed(), &by_kind, busy, peak);
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
            // (the provider answers every file read in place: none asked
            // after the evaluation)
            let asked_names: Vec<String> = Vec::new();
            // (a document file asked for cut, [`DocFile`], with no
            // provider: cut here)
            let asked_cut: Vec<String> = Vec::new();
            for name in asked_cut {
                let d = self.eng.values().field(&format!("dfile:{name}"));
                if self.files.iter().any(|&(g, _)| g == d)
                    || self.absent.contains(&d)
                    || self.written.contains(&name)
                {
                    continue;
                }
                let defs = self.ops.defs.locked().clone();
                let found = finder(&name);
                if !self.reads.iter().any(|(n, _)| *n == name) {
                    self.queries.push((name.clone(), found.is_some()));
                    self.reads.push((name.clone(), file_hash(found.as_ref())));
                }
                match found {
                    Some((path, bytes)) => {
                        let x = cut_doc_file(
                            self.eng.values(),
                            &self.ops.hints,
                            &defs,
                            &self.ops.docs,
                            &name,
                            &path,
                            &bytes,
                        );
                        self.files.push((d, x));
                    }
                    None => self.absent.push(d),
                }
                asked = true;
                ask_why(&format!("cut file supplied: {name}"));
            }
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                eprintln!("  asked: {asked_names:?}");
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
        // a format's text is one node: a sequential run over the whole
        // file, its `\input`s inline; what it may define is unknowable
        // from its parts, so anything (it runs once, serially, cached)
        let nodes = cut_text(&self.ops.hints, self.eng.values(), text, None, incr.as_deref_mut(), None);
        let incr = incr.as_deref();
        let t1 = t0.elapsed();
        crate::timeline::lap("push: the nodes' slices and keys");
        // (the nodes' arguments made on the pool: values are named by
        // their content, whichever worker makes them)
        let args = {
            let v = self.eng.values();
            let loads = self.loads.as_deref();
            let reg = &*self.ops.hints;
            let mut ex = crate::exec::Coarse(self.threads);
            crate::exec::map_on(&mut ex, nodes.len(), &|k| {
                let a = node_args(v, text, incr, &nodes[k].1, loads, None);
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
        let ids = self.eng.push_many(unit, self.ops.exec, args, self.threads);
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
pub(crate) fn latin1(bytes: &[u8]) -> String {
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

/// The first key of a written file's key set (`wkeys:<name>`) when the
/// job wrote the file, whatever its lines define: the file exists (TeX
/// finds a file the job wrote though no line of it defines anything);
/// no field's.
pub(crate) const WRITTEN_KEY: &str = "\u{2}written";

/// The value a written file's `wkeys:<name>` takes when the file is read
/// back as its text (`wfile:<name>`): a line written to it does more than
/// define ([`written_as_text`]).
pub(crate) fn text_mark(v: &Values) -> ValueId {
    v.str("\u{1}read back as text")
}

/// The job-written files read back as their text: those a line of which
/// did more than define as it was interpreted (`UNIT` in place of its
/// definitions, [`crate::run::Run::interpret_line`]); every other file
/// is read back per field.
pub(crate) fn written_as_text(v: &Values, streams: Option<ValueId>) -> Vec<String> {
    let streams = streams.map(|s| v.seq_to_vec(s)).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for &e in &streams {
        if is_cand_marker(v, e) {
            continue;
        }
        let xs = list_of(v, stream_entry(v, e));
        if xs.get(2) == Some(&ValueId::UNIT) {
            let name = xs.first().map(|&n| str_of(v, n)).unwrap_or_default();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
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


/// A document node's text: a slice of the document (its bytes).
enum DocNode {
    Doc(usize, usize),
}

/// The document's slices and their keys as [`doc_nodes`] made them
/// last (`Incr::slices`): each `(key, start, end)`, where each chunk's
/// begin, and the texts set when made (`Incr::edits`).
pub(crate) struct SliceMemo {
    edits: u64,
    out: Vec<(u64, usize, usize)>,
    first: Vec<usize>,
}

/// Chunk `c`'s slices keyed: the keys made when it was analysed, a slice
/// at its start not beginning a line keyed again.
fn chunk_slices(text: &str, c: &crate::incr::Chunk, out: &mut Vec<(u64, usize, usize)>) {
    out.extend(c.th.slices.iter().zip(c.keys.iter()).map(|(&(s, e), &k)| {
        let (s, e) = (c.start + s, c.start + e);
        let k = if s == c.start && s > 0 && text.as_bytes()[s - 1] != b'\n' {
            key(&text[s..e], false)
        } else {
            k
        };
        (k, s, e)
    }));
}

/// The document's nodes, keyed: per chunk of its units, its slices. (An
/// `\\input` file's regions are made where a run reaches the file,
/// whatever route reaches it: `Run::leftover`.) Made from the last ones
/// where the text set since changed one chunk only (`Incr::one`): its
/// slices and the next chunk's (whose first slice's key reads the
/// byte before it) made again, those after moved.
fn doc_nodes(text: &str, inc: &Incr) -> Vec<(u64, DocNode)> {
    let mut g = inc.slices.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = inc.chunks.len();
    let patched = match (g.as_mut(), inc.one) {
        (Some(m), Some(_)) if m.edits == inc.edits => true,
        (Some(m), Some((usize::MAX, 0))) if m.edits + 1 == inc.edits && m.first.len() == n => true,
        (Some(m), Some((k, d))) if m.edits + 1 == inc.edits && m.first.len() == n && k < n => {
            let to = (k + 2).min(n);
            let (a, b) = (m.first[k], m.first.get(to).copied().unwrap_or(m.out.len()));
            let mut mid = Vec::new();
            for c in &inc.chunks[k..to] {
                chunk_slices(text, c, &mut mid);
            }
            let grown = mid.len().cast_signed() - (b - a).cast_signed();
            for e in &mut m.out[b..] {
                e.1 = e.1.saturating_add_signed(d);
                e.2 = e.2.saturating_add_signed(d);
            }
            m.out.splice(a..b, mid);
            let mut at = a;
            for (j, c) in inc.chunks[k..to].iter().enumerate() {
                m.first[k + j] = at;
                at += c.th.slices.len();
            }
            for f in &mut m.first[to..] {
                *f = f.saturating_add_signed(grown);
            }
            true
        }
        _ => false,
    };
    if !patched {
        let mut out = Vec::new();
        let mut first = Vec::with_capacity(n);
        for c in &inc.chunks {
            first.push(out.len());
            chunk_slices(text, c, &mut out);
        }
        *g = Some(SliceMemo {
            edits: inc.edits,
            out,
            first,
        });
    }
    let m = g.as_mut().expect("made above");
    m.edits = inc.edits;
    if crate::exec::obs_flag!("PHITEX_SLICE_CHECK") {
        let mut all = Vec::new();
        for c in &inc.chunks {
            chunk_slices(text, c, &mut all);
        }
        assert!(all == m.out, "slice check: the slices made from the last ones are not those made anew");
    }
    m.out.iter().map(|&(k, s, e)| (k, DocNode::Doc(s, e))).collect()
}

/// A document node's arguments: its text with its origin, and its hint;
/// a document file's piece (`file`, its name) alike, a region a run
/// placed; a format's (`incr` `None`): its text, and the format's hint.
fn node_args(
    v: &Values,
    text: &str,
    incr: Option<&Incr>,
    n: &DocNode,
    loads: Option<&crate::pkgrec::LoadHints>,
    file: Option<ValueId>,
) -> Vec<ValueId> {
    match (n, incr) {
        (&DocNode::Doc(s, e), Some(inc)) => {
            let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
            let name = file.unwrap_or_else(|| v.str(DOC_NAME));
            let mut a = slice_args_in_file(v, &text[s..e], starts, name, s, ValueId::UNIT);
            // (a document node is a top-level node)
            if file.is_none() {
                a[1] = v.int(3);
            }
            // (a package load: its record's hint, or the optimistic one;
            // a node whose text cannot bound what it defines: its
            // record's, `crate::pkgrec`)
            let (c, at) = inc.chunk_at(s);
            let own = crate::hints::slice_hint_of(&c.th, at);
            let slice = &text[s..e];
            let rec = loads
                .filter(|_| crate::pkgrec::unbounded(&own) || crate::pkgrec::load_key(slice).is_some())
                .and_then(|l| l.hint_for(v, slice));
            a.push(v.str(rec.as_deref().unwrap_or(&*own)));
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

/// The prefix of the environment fields of the canonical text's
/// comments ([`canonical_text`]): `doc:mark:<id>`, a comment's text by
/// the id its mark carries ([`CommentMark`]).
pub(crate) const DOC_MARKS: &str = "doc:mark:";

/// A comment of the canonical text: its line (1-based), the hash of its
/// line's text before the `%` ([`prefix_hash`]), its text. The mark in
/// the text is a NUL and the comment's id ([`mark_id`], 16 hex digits),
/// so a run reads its comment by the id in its own line, not by where
/// the line is in the file.
pub(crate) type CommentMark = (u32, u64, String);


/// The hash of a comment line's text before its `%`.
fn prefix_hash(prefix: &str) -> u64 {
    phitex_defuse::hash_bytes(0x5c0d_e5ed, prefix.as_bytes())
}

/// The id of comment `c` on a line whose text before the `%` hashes to
/// `h`: `h` itself, unless another comment's line has the same text
/// before its `%` and another comment (`collide`): then the comment's
/// text too. So a comment-only edit keeps the line's text (its id), and
/// two marks of one id have the same comment.
fn mark_id(h: u64, c: &str, collide: bool) -> u64 {
    if collide {
        phitex_defuse::hash_bytes(h, c.as_bytes())
    } else {
        h
    }
}

/// The prefix hashes of `marks` with more than one comment.
fn collisions<'a>(
    marks: impl IntoIterator<Item = &'a CommentMark>,
) -> phitex_defuse::FxHashSet<u64> {
    let mut first: FxHashMap<u64, &str> = FxHashMap::default();
    let mut out = phitex_defuse::FxHashSet::default();
    for (_, h, c) in marks {
        match first.get(h) {
            Some(&c0) if c0 != c.as_str() => {
                out.insert(*h);
            }
            Some(_) => {}
            None => {
                first.insert(*h, c);
            }
        }
    }
    out
}

/// Writes the ids of `marks` over their places in `out` (`at`: where each
/// one's 16 digits begin, [`canonical_line`]).
fn fill_ids(
    out: &mut String,
    at: &[usize],
    marks: &[CommentMark],
    collide: &phitex_defuse::FxHashSet<u64>,
) {
    for (&p, (_, h, c)) in at.iter().zip(marks) {
        let id = format!("{:016x}", mark_id(*h, c, collide.contains(h)));
        out.replace_range(p..p + 16, &id);
    }
}

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
    /// The files' texts ([`Build::files`]) by the `streams` value.
    #[allow(clippy::type_complexity)]
    files: Option<(Option<ValueId>, Arc<Vec<(String, String)>>)>,
}

/// The comments by line, and whether the environment lacks them.
type MarksState = (Vec<CommentMark>, bool);

impl Marks {
    fn get(&self) -> Vec<CommentMark> {
        self.0.locked().0.clone()
    }

    fn set(&self, ms: Vec<CommentMark>) {
        let mut m = self.0.locked();
        if m.0 != ms {
            *m = (ms, true);
        }
    }

    /// The `doc:mark:<id>` entries; the environment holds them from here.
    fn entries(&self, v: &Values) -> Vec<(Field, ValueId)> {
        let mut m = self.0.locked();
        m.1 = false;
        let collide = collisions(&m.0);
        m.0.iter()
            .map(|(_, h, c)| {
                let id = mark_id(*h, c, collide.contains(h));
                (v.field(&format!("{DOC_MARKS}{id:016x}")), v.str(c))
            })
            .collect()
    }

    /// Whether marks changed since the environment was set.
    fn dirty(&self) -> bool {
        self.0.locked().1
    }
}

/// The document's text as its tokens need it (spec-redesign §8: a unit's
/// value is its tokens, not its text): each comment's text after its
/// `%` replaced by a NUL and the comment's id ([`CommentMark`]), and the
/// comments (each its line, 1-based). Under the catcodes a document is read with (`\\` an
/// escape, `%` a comment) the tokens are the same, and a comment-only
/// edit changes no node. A run that reads a mark as text (`%` no
/// comment there, a verbatim's) puts the comment back in its line from
/// `doc:mark:<id>`, by the id its mark carries after the NUL
/// (`Run::splice_mark`): the tokens are TeX's either way.
/// `None` for a text this cannot mark (one holding NUL, or `^^`, which
/// can spell an escape or a `%`).
pub(crate) fn canonical_text(text: &str) -> Option<(String, Vec<CommentMark>)> {
    if has_raw_marks(text) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let (mut marks, mut at) = (Vec::new(), Vec::new());
    canonical_lines(text, 0, &mut out, &mut marks, &mut at);
    fill_ids(&mut out, &at, &marks, &collisions(&marks));
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
/// marks, and the range of `canon` replaced, `(start, old end, new end)`;
/// `None` (`canon` untouched) where a comment's line made again or gone
/// has the text before its `%` of another one's, whose id may change
/// with it ([`mark_id`]): the whole text is made again.
fn canonical_splice(
    old: &str,
    canon: &mut String,
    marks: &[CommentMark],
    new: &str,
    ends: Option<(usize, usize)>,
) -> Option<(Vec<CommentMark>, (usize, usize, usize))> {
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
    marks: &[CommentMark],
    new: &str,
    p: usize,
    s: usize,
) -> Option<(Vec<CommentMark>, (usize, usize, usize))> {
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
    let (mut mid, mut at) = (Vec::new(), Vec::new());
    canonical_lines(&new[ls..le], 0, &mut out, &mut mid, &mut at);
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
    // (the marks of the lines kept, and of those made again or gone: a
    // line text before a `%` they share takes the whole text again)
    let lo = u32::try_from(first).unwrap_or(u32::MAX);
    let hi = u32::try_from(first + old_lines).unwrap_or(u32::MAX);
    let kept: phitex_defuse::FxHashSet<u64> = marks
        .iter()
        .filter(|m| m.0 <= lo || m.0 > hi)
        .map(|m| m.1)
        .collect();
    if marks
        .iter()
        .filter(|m| m.0 > lo && m.0 <= hi)
        .chain(&mid)
        .any(|m| kept.contains(&m.1))
    {
        return None;
    }
    fill_ids(&mut out, &at, &mid, &collisions(&mid));
    canon.replace_range(cs..ce, &out);
    crate::timeline::note("canonical: spliced in place", t0.elapsed());
    // (the marks: before the lines, theirs made again, after them moved)
    let new_lines = newlines(&n[ls..le]);
    let shift = |l: u32| -> u32 {
        let l = i64::from(l) + i64::try_from(new_lines).unwrap_or(0)
            - i64::try_from(old_lines).unwrap_or(0);
        u32::try_from(l).unwrap_or(u32::MAX)
    };
    let mut ms: Vec<CommentMark> = marks.iter().filter(|m| m.0 <= lo).cloned().collect();
    ms.extend(mid);
    ms.extend(
        marks
            .iter()
            .filter(|m| m.0 > hi && le_old < o.len())
            .map(|(l, h, c)| (shift(*l), *h, c.clone())),
    );
    Some((ms, (cs, ce, cs + out.len())))
}

/// [`canonical_text`]'s lines of `text`, the first numbered `first`
/// (from 0), onto `out` and `marks`.
fn canonical_lines(
    text: &str,
    first: usize,
    out: &mut String,
    marks: &mut Vec<CommentMark>,
    at: &mut Vec<usize>,
) {
    // (only a line with a `%` can change: the lines between two such
    // lines are copied at once, their ends counted, eight bytes at a time)
    let b = text.as_bytes();
    let (mut done, mut n, mut from) = (0, first, 0);
    while let Some(p) = text[from..].find('%').map(|k| from + k) {
        let ls = text[done..p].rfind('\n').map_or(done, |k| done + k + 1);
        n += newlines(&b[done..ls]);
        out.push_str(&text[done..ls]);
        let le = text[p..].find('\n').map_or(text.len(), |k| p + k);
        canonical_line(&text[ls..le], n, out, marks, at);
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
fn canonical_line(
    body: &str,
    n: usize,
    out: &mut String,
    marks: &mut Vec<CommentMark>,
    at: &mut Vec<usize>,
) {
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
            // (its id's place, [`fill_ids`])
            at.push(out.len());
            out.push_str("0000000000000000");
            // (the comment as web2c's `input_line` keeps it: the
            // line's end, then its trailing blanks removed, §31)
            let c = body[k + 1..]
                .trim_end_matches(['\r', '\n'])
                .trim_end_matches([' ', '\t']);
            marks.push((
                u32::try_from(n + 1).unwrap_or(u32::MAX),
                prefix_hash(&body[..k]),
                c.to_owned(),
            ));
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
    if let Some(d) = c.defs.get() {
        return d.clone();
    }
    let cache = C.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Some(d) = cache.locked().get(&c.key) {
        let _ = c.defs.set(d.clone());
        return d.clone();
    }
    // (as the exported prelude kept them: its key covers the text, every
    // file read and this build of the harvest, `engine_identity`)
    if let Some(d) = c
        .defs_bytes
        .as_deref()
        .and_then(pass::Definitions::from_bytes)
    {
        let d: Defs = Arc::new(d);
        let _ = c.defs.set(d.clone());
        cache.locked().insert(c.key, d.clone());
        return d;
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
    let _ = c.defs.set(d.clone());
    cache.locked().insert(c.key, d.clone());
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

/// The check of a format read from the cache ([`cached_prelude`]),
/// running.
static FORMAT_CHECK: Mutex<Option<std::thread::JoinHandle<bool>>> = Mutex::new(None);

/// Whether the format read from the cache, if one was, still has every
/// file it read as the finder gives it now.
fn format_holds() -> bool {
    FORMAT_CHECK.locked().take().is_none_or(|h| h.join().unwrap_or(false))
}

fn cached_prelude(key: u64, opts: &Options) -> Result<Arc<Cached>, Vec<String>> {
    let finder = opts.prelude_finder();
    let candidates: Vec<Arc<Cached>> = env_cache().locked().get(&key).cloned().unwrap_or_default();
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
    // (a format read from the cache is taken at once, its files found and
    // hashed on a thread of their own beside the job's start; the build
    // takes the answer before its first evaluation, `format_holds`)
    if let Some(c) = given {
        let (finder, cc) = (finder.clone(), c.clone());
        let h = std::thread::spawn(move || {
            let t1 = std::time::Instant::now();
            let fresh = cc.reads.iter().all(|(n, h)| file_hash(finder(n).as_ref()) == *h);
            if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                eprintln!(
                    "format check: {} files found and hashed in {:?} ({}, beside the start)",
                    cc.reads.len(),
                    t1.elapsed(),
                    if fresh { "fresh" } else { "stale" }
                );
            }
            fresh
        });
        *FORMAT_CHECK.locked() = Some(h);
        return Ok(c);
    }
    let mut known: Vec<String> = Vec::new();
    for c in candidates {
        let t1 = std::time::Instant::now();
        // (the files found, read and hashed on the pool)
        let mut ex = crate::exec::Coarse(crate::exec::threads(
            opts.threads,
        ));
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
        g.eng.set_environment([(f, *x)]);
    }
    if let Some(p) = prelude {
        {
            let v = g.eng.values();
            let (f, x) = (v.field("jobname"), v.str(&opts.prelude_jobname));
            g.eng.set_environment([(f, x)]);
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
        // (the order the prelude's fonts were loaded in, by key: the
        // first numbers of a document's fonts, [`FONT_ORDER`])
        let v = g.eng.values();
        let order = font_order(v, g.eng.acc_value(unit, v.field(crate::pdfout::XPAGES)));
        env.retain(|(k, _)| k != FONT_ORDER);
        env.push((
            FONT_ORDER.to_owned(),
            v.list(order.into_iter().map(|k| v.int(k)).collect()),
        ));
    }
    let t_bindings = t_start.elapsed();
    let m_bindings = mem_mb();
    let v = g.eng.values();
    let e = Arc::new(crate::fmtenv::FormatEnv::of(v, &env));
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
        defs_bytes: None,
        defs: OnceLock::new(),
        key: prelude_key_of(key, &g.reads),
        text_key: key,
        reads: g.reads.clone(),
        env: e,
        errors: g.eval_errors.clone(),
        diags,
    });
    if c.errors.is_empty() {
        env_cache().locked().entry(key).or_default().push(c.clone());
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
#[allow(clippy::struct_excessive_bools)]
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
    /// Whether a package load ran in the last evaluation: only then do
    /// the loads' records change (`crate::pkgrec`; a record is only a
    /// load's run's), so only then are they merged and stored.
    loads_ran: bool,
    /// Files changed ([`Build::stage_file`]) not evaluated yet: the next
    /// [`Build::absorb_pending`] evaluates them with the text sent, one
    /// evaluation (its stats the nodes both ran).
    staged: bool,
    /// The nodes the files staged since the last evaluation changed.
    staged_nodes: usize,
    /// Whether the build has evaluated (its cold build done).
    evaluated: bool,
}

impl Drop for Build {
    /// The files its link made go with it ([`crate::assemble::forget_linked`]).
    fn drop(&mut self) {
        crate::assemble::forget_linked(self.g.eng.values().instance());
    }
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
        g.ops
            .node_rec
            .store(opts.node_records, std::sync::atomic::Ordering::Relaxed);
        (*g.ops.out_ok.locked()).clone_from(&opts.out_name_ok);
        g.ops.tool_opts.locked().bibtex.clone_from(&opts.bibtex);
        g.tool_finder.clone_from(&opts.tool_finder);
            if let Some(p) = &opts.progress {
            g.watch = crate::exec::Watch::new(g.threads, Some(p.clone()));
        }
        let unit = g.eng.new_unit();
        crate::timeline::lap("engine: the graph made");
        // (the files the format read are not supplied up front: a run
        // reading one gets it from the provider in place, `Graph::provide`,
        // and one no run reads is never read, hashed or made a value)
        g.guesses(&opts);
        crate::timeline::lap("engine: the guesses (read-back keys)");
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
        // (the values the job made of the format's nodes: each once)
        let made = Arc::new(env.made());
        job_env(&mut g, unit, &env, &made, &opts);
        // (the writers' start, with the format's glyph table, made now
        // beside the job: `crate::pdfout::warm_g2u`)
        if let Some(t) = env.get(g.eng.values(), "pdf:glyphtounicode", &made) {
            crate::pdfout::warm_g2u(g.eng.values(), t);
        }
        // §1337: `\everyjob`, before the job's first line
        let every = env.get(g.eng.values(), "param:everyjob", &made);
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
        *g.ops.defs.locked() = defs.clone();
        let doc = is_doc(prelude, document);
        // the package loads' records (`crate::pkgrec`); a load never seen
        // runs optimistically under speculation
        let optimistic = opts.speculation != Speculation::Off;
        let predict = opts.pkg_predict;
        // (the document nodes' records, the document's own, beside them)
        let mut recs = crate::pkgrec::parse(&opts.pkgrec);
        recs.extend(crate::pkgrec::parse(&opts.noderec));
        g.loads = Some(Arc::new(crate::pkgrec::LoadHints::new(
            recs,
            opts.finder.clone(),
            optimistic,
            predict,
        )));
        // (the loads' predictions, asked by the engine per load node)
        g.ops.pkg_loads.write().unwrap_or_else(std::sync::PoisonError::into_inner).clone_from(&g.loads);
        crate::timeline::lap("engine: the job's environment, the loads' records");
        let (nodes, incr) = g.push_document(unit, &text, defs, doc);
        // (the pages: the first page step, where the text ends: it folds
        // the pieces' profiles to its page's fire and places the page's
        // finisher and the next step, `crate::pages::Pages`; new text
        // pieces go before the mark it follows. With more than one thread
        // it is placed by a node where the text begins, right after the
        // text's first piece, which the caller's thread runs before the
        // others start (the state it sets, read by every piece after it,
        // in before any of them runs): the steps run beside the text,
        // not after the last block of it)
        let steps = g.eng.push(unit, g.mark, Vec::new());
        match nodes.first() {
            Some(&(_, first, _)) if g.threads > 1 => {
                let v = g.eng.values();
                let args = crate::pages::kick_args(v, steps);
                g.eng.insert_after(first, g.ops.pages, args);
            }
            _ => {
                g.eng.push(unit, g.ops.pages, Vec::new());
            }
        }
        // (the files' node, then the link and the end node: new text
        // pieces go before all three; the files' node, whose run gives the
        // loop-backs, before the link and the end node, which run once
        // the loop-backs settle (`final_only`): no range it reads holds
        // them)
        let files_op = g.files_op;
        let files = g.eng.push(unit, files_op, Vec::new());
        g.eng.push(unit, g.link, Vec::new());
        let dvi = g.eng.push(unit, g.dvi, Vec::new());
        let _ = files;
        let end = steps;
        crate::timeline::lap("engine: the link, end and files nodes");
        // (a format whose files changed since it was cached: built again)
        if !format_holds() {
            crate::timeline::lap("engine: the cached format stale");
            return Build::with(Options { prelude_bytes: None, ..opts }, prelude, document);
        }
        crate::timeline::lap("engine: the format's check taken");
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
            loads_ran: false,
            staged: false,
            staged_nodes: 0,
            evaluated: false,
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
    /// it (the environment, or the nodes defining it).
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

    /// The text nodes' offsets in the document, for the progress (an
    /// edit of a long document keeps the last ones: a place shown while
    /// it runs is off by the edit's length at most, and the map is not
    /// made again for every edit).
    fn progress_offsets(&mut self) {
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
    }

    /// The loads' records as the evaluation left them: a load inserted
    /// by a later edit uses them. A record is what a load's run left
    /// (`crate::pkgrec::entry`), so an evaluation in which no load ran
    /// leaves the merged records as they were.
    fn merge_load_records(&mut self) {
        // (the records of the nodes whose run was recorded in it: the
        // others' are merged already)
        let mut ran: phitex_defuse::FxHashSet<u64> = std::mem::take(&mut *self.g.ops.rec_ran.locked())
            .into_iter()
            .collect();
        ran.extend(self.g.ops.pkg_log.locked().iter().map(|r| r.0));
        self.loads_ran = !ran.is_empty();
        if self.loads_ran
            && let Some(l) = &self.g.loads
        {
            l.merge(self.fresh_records(|n, _| ran.contains(&n)));
        }
    }

    /// `PHITEX_TEX_TIME`: the evaluation's phases (ends `t_before`,
    /// `t_apply`, `t_settle` from its start `t_all`) and the sizes.
    fn note_edit_phases(
        &self,
        t_all: std::time::Instant,
        t_before: std::time::Duration,
        t_apply: std::time::Duration,
        t_settle: std::time::Duration,
    ) {
        eprintln!(
            "edit phases: before the apply (offsets, inputs) {t_before:?}, apply (intake, hint pass, slice-key diff, engine replace/insert/remove: `edit apply`) {:?}, settle (`settle phases`) {:?}, after (stats, guesses kept) {:?}",
            t_apply.saturating_sub(t_before),
            t_settle.saturating_sub(t_apply),
            t_all.elapsed().saturating_sub(t_settle)
        );
        let (nodes, units, values) = self.sizes();
        let (node, use_) = (std::mem::size_of::<phitex_defuse::NodeInfo>(), 0);
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

    /// The pages whose hash changed since last told, to the
    /// `on_page_settled` callback.
    fn tell_settled_pages(&mut self) {
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

    fn settle(&mut self) {
        self.staged = false;
        let t_all = std::time::Instant::now();
        self.progress_offsets();
        crate::timeline::lap("engine: the nodes' offsets (progress)");
        let mut stats = Stats::default();
        self.g.queries.clear();
        self.g.ops.pkg_log.locked().clear();
        self.g.ops.rec_ran.locked().clear();
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
            applied: 0,
        };
        let t_before = t_all.elapsed();
        crate::timeline::lap("engine: the edit's inputs");
        // a text sent before the evaluation starts is the one it is for
        if let Some(v) = phitex_defuse::EditSource::absorb(&mut absorb, &self.g.eng) {
            self.g.eng.set_version(v);
        }
        let t_apply = t_all.elapsed();
        // (an edit that starts with few nodes runs inline, [`INLINE_NODES`];
        // the cold build and large edits on the pool)
        self.g.inline = self.evaluated
            && absorb.applied <= INLINE_NODES
            && std::mem::take(&mut self.staged_nodes) <= INLINE_NODES
            && !crate::exec::obs_flag!("PHITEX_NO_INLINE");
        self.evaluated = true;
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
        self.merge_load_records();
        crate::timeline::lap("engine: package load records merged");
        self.stats = stats;
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            self.note_edit_phases(t_all, t_before, t_apply, t_settle);
        }
        self.dump_claims();
        self.dump_readers();
        self.tell_settled_pages();
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
        if !self.edits.pending() && !self.staged {
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

    /// The engine's live nodes, the build's units (one) and the values
    /// in its table.
    #[must_use]
    pub fn sizes(&self) -> (usize, usize, usize) {
        (self.g.eng.live_nodes(), 1, self.g.eng.values().len())
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
    pub fn edit_file(&mut self, name: &str, text: Option<&[u8]>) {
        self.stage_file(name, text);
        if self.staged {
            self.settle();
        }
    }

    /// [`Build::edit_file`] without the evaluation: the next
    /// [`Build::absorb_pending`] (or edit) evaluates it, with the text
    /// sent, as one.
    #[allow(clippy::too_many_lines)]
    pub fn stage_file(&mut self, name: &str, text: Option<&[u8]>) {
        let v = self.g.eng.values();
        let f = v.field(&format!("file:{name}"));
        let d = v.field(&format!("dfile:{name}"));
        let cut_path = self
            .g
            .files
            .iter()
            .find(|&&(g, _)| g == d)
            .and_then(|&(_, x)| list_of(v, x).first().map(|&p| str_of(v, p)));
        let cut = cut_path.is_some() || self.g.absent.contains(&d);
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
                    .or(cut_path)
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
        // (a record listing the file is matched against its new text)
        if let Some(l) = &self.g.loads {
            l.forget(name);
        }
        // (its size, where a run asked for it, `fsize:`)
        let z = v.field(&format!("fsize:{name}"));
        if let Some(k) = self.g.files.iter().position(|(g, _)| *g == z) {
            if let Some((_, b)) = &found {
                self.g.files[k].1 = v.int(i64::try_from(b.len()).unwrap_or(i64::MAX));
            } else {
                self.g.files.remove(k);
                self.g.absent.push(z);
            }
        }
        crate::timeline::lap("stage: the file's field");
        // (a document file cut, [`DocFile`]: cut again where it changed,
        // its pieces edited as the document's nodes are; one that comes,
        // goes, or stops or starts being a document's text: cut afresh,
        // `dfile:` changed, its readers run again)
        if cut {
            let doc = self.g.ops.docs.locked().get(name).cloned();
            let text = found.as_ref().map(|(p, b)| (p, latin1(b)));
            match (doc, text) {
                (Some(df), Some((path, t))) if crate::inputs::document_file(path, &t) => {
                    self.staged_nodes += self.stage_doc_file(&df, t);
                }
                (_, found_text) => {
                    self.g.files.retain(|&(g, _)| g != d);
                    self.g.absent.retain(|&g| g != d);
                    self.g.ops.docs.locked().remove(name);
                    if let (Some((path, bytes)), Some(_)) = (&found, found_text) {
                        let defs = self.g.ops.defs.locked().clone();
                        let x = cut_doc_file(
                            self.g.eng.values(),
                            &self.g.ops.hints,
                            &defs,
                            &self.g.ops.docs,
                            name,
                            path,
                            bytes,
                        );
                        self.g.files.push((d, x));
                    } else {
                        self.g.absent.push(d);
                    }
                    self.staged_nodes += INLINE_NODES + 1;
                }
            }
            crate::timeline::lap("stage: the file cut again, its pieces edited");
        } else {
            self.staged_nodes += 1;
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
                            None,
                        ),
                    )),
                    Src::Doc(..) => None,
                })
                .collect();
            for (n, args) in redo {
                self.g.eng.replace(n, self.g.ops.exec, args);
            }
        }
        crate::timeline::lap("stage: the loads' records");
        self.g.absorb_provided();
        let env = self.g.environment(self.opts.fixpoint);
        let env: Vec<(Field, ValueId)> = (env).into_iter().collect();
        env_trace(&self.g.eng, &env);
        crate::timeline::lap("stage: the environment made");
        self.g.push_env(env);
        crate::timeline::lap("stage: the environment set");
        self.staged = true;
    }

    /// Cut document file `df` ([`DocFile`]) whose text is now `text`:
    /// cut again where it changed (the range from the ends the two texts
    /// share), and the pieces each node that placed them holds edited to
    /// the new slices as the document's nodes are ([`apply_slices`]); a
    /// node whose pieces are not the last cut's runs again. The pieces
    /// replaced, inserted or removed.
    fn stage_doc_file(&mut self, df: &Arc<std::sync::Mutex<DocFile>>, text: String) -> usize {
        let mut df = df.locked();
        if df.text == text {
            return 0;
        }
        let (p, s) = byte_ends(df.text.as_bytes(), text.as_bytes());
        let range = (p, df.text.len() - s, text.len() - s);
        let old = std::mem::take(&mut df.slices);
        df.text = text;
        let pv = self.g.eng.values().str(&df.path);
        let new = {
            let DocFile { text, incr, .. } = &mut *df;
            cut_text(&self.g.ops.hints, self.g.eng.values(), text, Some(range), Some(incr), Some(pv))
        };
        crate::timeline::lap("stage: the file cut again");
        let loads = self.g.loads.clone();
        let exec = self.g.ops.exec;
        let mut n = 0;
        let mut redo = Vec::new();
        let readers = std::mem::take(&mut df.readers);
        let mut kept = Vec::with_capacity(readers.len());
        let mut shifts = std::mem::take(&mut df.shifts);
        let mut placed = std::mem::take(&mut df.placed);
        let delta = i64::try_from(df.text.len()).unwrap_or(0) - i64::try_from(range.1 + s).unwrap_or(0);
        for r in readers {
            if !self.g.eng.is_live(r) {
                continue;
            }
            // (its pieces as the last edit left them, where they still are
            // its placements in order; else found again)
            let kids = match placed.remove(&r.raw()) {
                Some(k) if !k.is_empty() && placed_still(&self.g.eng, r, &k) => k,
                _ => pieces_of(&self.g.eng, r, pv),
            };
            // (a node that no longer places the file's pieces: not its
            // reader now)
            if kids.is_empty() && !old.is_empty() {
                continue;
            }
            kept.push(r);
            if kids.len() != old.len() {
                redo.push(r);
                continue;
            }
            let mut nodes: Vec<(u64, NodeId, Src)> = old
                .iter()
                .zip(&kids)
                .map(|((k, d), &id)| (*k, id, Src::of(d)))
                .collect();
            let args_of = |eng: &Engine, k: usize| {
                node_args(eng.values(), &df.text, Some(&df.incr), &new[k].1, loads.as_deref(), Some(pv))
            };
            match apply_slices(&self.g.eng, exec, Placing::Placed, &mut nodes, &new, &args_of) {
                Some(a) => {
                    n += a.changed();
                    // (each piece's move since it was made: a kept one's
                    // text, and the places in it, as when it was made; those
                    // made now are where they are; those after the change
                    // moved by as much as the text grew)
                    let mid = a.prefix..nodes.len() - a.suffix;
                    for (i, (_, id, _)) in nodes.iter().enumerate() {
                        if mid.contains(&i) {
                            shifts.remove(&id.raw());
                        } else if i >= mid.end && delta != 0 {
                            let e = match shifts.get(&id.raw()) {
                                Some(&(fs, d)) => Some((fs, d + delta)),
                                None => self.g.eng.node_info(*id).and_then(|x| x.args.first().copied()).map(|fs| (fs, delta)),
                            };
                            if let Some(e) = e {
                                shifts.insert(id.raw(), e);
                            }
                        }
                    }
                    placed.insert(r.raw(), nodes.iter().map(|x| x.1).collect());
                }
                None => redo.push(r),
            }
        }
        // (a node whose pieces are not the cut's, or with none to edit:
        // runs again, placing the new cut's)
        for r in redo {
            if let Some(i) = self.g.eng.node_info(r) {
                let args = i.args;
                self.g.eng.refresh_where(&|_, a, _| *a == args[..]);
                n += INLINE_NODES + 1;
            }
        }
        shifts.retain(|&k, _| self.g.eng.is_live(NodeId::from_raw(k)));
        // (each slice's arguments: a kept one's as its pieces hold them,
        // a new one's made from the text, [`DocFile::args`])
        df.args = kept_args(&old, &new, &df.args);
        df.readers = kept;
        df.shifts = shifts;
        df.placed = placed;
        df.slices = new;
        crate::timeline::lap("stage: the file's pieces edited");
        n
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
        // (the nodes a run made of a file's slices, under the document's
        // nodes: each in its file's text, at the slice's place, moved as
        // far as an edit of its file moved the piece it is under,
        // [`Run::doc_file_regions`], [`DocFile::shifts`])
        let v = self.g.eng.values();
        let shifts: FxHashMap<u64, (ValueId, i64)> = self
            .g
            .ops
            .docs
            .locked()
            .values()
            .flat_map(|d| d.locked().shifts.iter().map(|(&k, &x)| (k, x)).collect::<Vec<_>>())
            .collect();
        let mut texts: FxHashMap<String, Option<Arc<str>>> = FxHashMap::default();
        let mut seen: phitex_defuse::FxHashSet<u64> = phitex_defuse::FxHashSet::default();
        let mut stack: Vec<(NodeId, i64)> = self.nodes.iter().map(|(_, n, _)| (*n, 0)).collect();
        while let Some((x, sh)) = stack.pop() {
            if !seen.insert(x.raw()) {
                continue;
            }
            let Some(i) = self.g.eng.node_info(x) else {
                stack.extend(self.g.eng.children(x).into_iter().map(|c| (c, sh)));
                continue;
            };
            let sh = match (shifts.get(&x.raw()), i.args.first()) {
                (Some(&(fs, d)), Some(&a)) if fs == a => d,
                _ => sh,
            };
            stack.extend(self.g.eng.children(x).into_iter().map(|c| (c, sh)));
            let Some(&fs) = i.args.first() else { continue };
            let mut span: Option<(String, usize, usize)> = None;
            for f in list_of(v, fs) {
                let xs = list_of(v, f);
                let Some(&t) = xs.first() else { continue };
                let (name, s, e) = match (str_of(v, t).as_str(), &xs[..]) {
                    ("s", &[_, seg, _, _, _, name, pos]) => {
                        let s = usize::try_from(int_of(v, pos) + sh).unwrap_or(0);
                        (str_of(v, name), s, s + str_of(v, seg).len() + 1)
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
    /// (`crate::pkgrec`; merged at the end of each evaluation in which a
    /// load ran). Kept machine-wide, beside the format.
    #[must_use]
    pub fn export_pkgrec(&self) -> Vec<u8> {
        match &self.g.loads {
            Some(l) => l.bytes_where(|k| !crate::pkgrec::is_node_key(k)),
            None => self.opts.pkgrec.clone(),
        }
    }

    /// The document nodes' records ([`Options::noderec`]): those of the
    /// nodes live in this build (a record whose node is gone is not
    /// kept), the runs of this build's replacing their own. Kept per
    /// document.
    #[must_use]
    pub fn export_noderec(&self) -> Vec<u8> {
        let Some(l) = &self.g.loads else {
            return self.opts.noderec.clone();
        };
        let v = self.g.eng.values();
        let live: phitex_defuse::FxHashSet<String> = self
            .g
            .eng
            .acc_value(self.unit, v.field("pkgrecs"))
            .map(|acc| v.seq_to_vec(acc))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| crate::pkgrec::entry_head(v, e).map(|(_, k)| k))
            .filter(|k| crate::pkgrec::is_node_key(k))
            .collect();
        l.bytes_where(|k| live.contains(k))
    }

    /// Whether a package load or a recorded document node ran in the
    /// last evaluation, so that the records [`Self::export_pkgrec`] and
    /// [`Self::export_noderec`] give may differ from those they gave
    /// before; `false`: they are the same.
    #[must_use]
    pub fn pkgrec_changed(&self) -> bool {
        self.loads_ran
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

    /// The records the recorded nodes' last runs left (`pkgrecs`), by
    /// key, of the nodes and keys `keep` keeps.
    fn fresh_records(&self, keep: impl Fn(u64, &str) -> bool) -> Vec<(String, crate::pkgrec::Rec)> {
        let v = self.g.eng.values();
        let Some(acc) = self.g.eng.acc_value(self.unit, v.field("pkgrecs")) else {
            return Vec::new();
        };
        v.seq_to_vec(acc)
            .into_iter()
            .filter(|&e| crate::pkgrec::entry_head(v, e).is_some_and(|(n, k)| keep(n, &k)))
            .filter_map(|e| {
                let (_, k, r) = crate::pkgrec::decode_entry(v, e)?;
                Some((k, r))
            })
            .collect()
    }

    /// The package loads' counters ([`Stats::pkg_loads`] and the rest).
    #[allow(clippy::too_many_lines)]
    fn pkg_stats(&self, stats: &mut Stats) {
        // (the loads found once: a pass over every node's text)
        let loads: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, _, src)| crate::pkgrec::load_key(src.text(&self.text)).is_some())
            .map(|(_, n, _)| *n)
            .collect();
        stats.pkg_loads = loads.len() as u64;
        stats.pkg_claimed = loads
            .iter()
            .filter(|&&n| {
                self.g
                    .eng
                    .node_info(n)
                    .is_some_and(|i| i.state == phitex_defuse::NodeState::Inert)
            })
            .count() as u64;
        if crate::exec::obs_flag!("PHITEX_PKG_TRACE") {
            let fresh = self.fresh_records(|_, k| !crate::pkgrec::is_node_key(k));
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
            let log = self.g.ops.pkg_log.locked().clone();
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
            stats.node_recorded = l.given[2].load(std::sync::atomic::Ordering::Relaxed);
            // each prediction against what the load's last run published
            // (where a load ran in this evaluation: an edit's that ran
            // none compares nothing, every record not decoded again)
            let ran = !self.g.ops.pkg_log.locked().is_empty();
            let predicted = if ran { l.predicted.locked().clone() } else { FxHashMap::default() };
            if !predicted.is_empty() {
                let fresh: FxHashMap<String, crate::pkgrec::Rec> = self
                    .fresh_records(|_, k| predicted.contains_key(k))
                    .into_iter()
                    .collect();
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
        let log = self.g.ops.pkg_log.locked().clone();
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
        stats.pkg_on_critical = loads.iter().filter(|n| on.contains(&n.raw())).count() as u64;
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

    /// What the last evaluation's link did with the exact PDF's pages:
    /// `(placed, shipped again)`, the pages placed as their own steps
    /// wrote them and those it wrote again itself (none linked: zeros).
    #[must_use]
    pub fn link_pages(&self) -> (u64, u64) {
        let v = self.g.eng.values();
        let acc = self
            .g
            .eng
            .acc_value(self.unit, v.field(crate::pdfout::LINKED));
        acc.map(|a| v.seq_to_vec(a))
            .and_then(|es| es.last().copied())
            .map_or((0, 0), |e| {
                let xs = list_of(v, e);
                let n = |k: usize| {
                    xs.get(k)
                        .map_or(0, |&x| u64::try_from(int_of(v, x)).unwrap_or(0))
                };
                (n(0), n(1))
            })
    }

    /// The files the job wrote (`\openout`, `\write`), with their text,
    /// in the order first opened: what TeX leaves in them.
    #[must_use]
    pub fn files(&self) -> Vec<(String, String)> {
        let v = self.g.eng.values();
        let streams = self.g.eng.acc_value(self.unit, v.field("streams"));
        let mut m = self.memo.locked();
        if let Some((from, fs)) = &m.files
            && *from == streams
        {
            return fs.to_vec();
        }
        let fs = Arc::new(written_texts(v, streams));
        m.files = Some((streams, fs.clone()));
        fs.to_vec()
    }

    /// What [`Build::wkeys`] is made from (the written files'
    /// accumulator, as a value): equal, the same key sets.
    #[must_use]
    pub fn wkeys_source(&self) -> Option<u64> {
        let v = self.g.eng.values();
        self.g
            .eng
            .acc_value(self.unit, v.field("streams"))
            .map(ValueId::raw)
    }

    /// The key sets of the read-back files that only define,
    /// as the last build published them: [`Options::wkeys`] for a later
    /// session of the same document.
    #[must_use]
    pub fn wkeys(&self) -> Vec<(String, Vec<String>)> {
        let v = self.g.eng.values();
        let acc = |n: &str| self.g.eng.acc_value(self.unit, v.field(n));
        let text = written_as_text(v, acc("streams"));
        written_defs(v, acc("streams"))
            .into_iter()
            .filter(|(n, _)| !text.contains(n))
            .map(|(n, ds)| {
                let mut ks = vec![WRITTEN_KEY.to_owned()];
                ks.extend(ds.into_iter().map(|(f, _)| f));
                (n, ks)
            })
            .collect()
    }

    /// Whether the output is a PDF file (`\pdfoutput` at the first
    /// `\shipout`); else DVI.
    #[must_use]
    pub fn is_pdf(&self) -> bool {
        let v = self.g.eng.values();
        self.pdf_layout().is_some()
            || self
                .end_file()
                .is_some_and(|x| crate::assemble::file_bytes_shared(v, x).starts_with(b"%PDF"))
    }

    /// The PDF file's layout, the link's (none: a DVI file, or no file):
    /// its exact bytes made when asked ([`phitex_pdf::Layout::exact`]),
    /// or an update section of a file holding an earlier one
    /// ([`phitex_pdf::Updater`]).
    #[must_use]
    pub fn pdf_layout(&self) -> Option<phitex_pdf::Layout> {
        let v = self.g.eng.values();
        self.end_file()
            .and_then(|x| crate::assemble::file_layout(v, x))
    }

    /// The output file's bytes (the exact PDF file, made from its layout
    /// if not yet; a DVI file), without the log.
    #[must_use]
    pub fn output_bytes(&self) -> Arc<Vec<u8>> {
        let v = self.g.eng.values();
        self.end_file()
            .map(|x| crate::assemble::file_bytes_shared(v, x))
            .unwrap_or_default()
    }

    /// The log's error lines (`! ...`), read without laying the log out:
    /// none unless the errors' count says there are some.
    #[must_use]
    pub fn error_lines(&self) -> Vec<String> {
        let v = self.g.eng.values();
        let n = self
            .g
            .eng
            .acc_value(self.unit, v.field(crate::run::LOG_ERRORS))
            .map_or(0, |x| int_of(v, x));
        if n == 0 {
            return Vec::new();
        }
        let (error, errmsg) = (v.str("error"), v.str("errmsg"));
        self.g
            .eng
            .acc_value(self.unit, v.field("log"))
            .map(|e| v.seq_to_vec(e))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|x| {
                let xs = list_of(v, x);
                let k = *xs.first()?;
                (k == error || k == errmsg).then(|| {
                    format!(
                        "! {}.",
                        xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default()
                    )
                })
            })
            .collect()
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

    /// The output file's value as the end node defined it
    /// ([`crate::assemble::END_FILE`]).
    fn end_file(&self) -> Option<ValueId> {
        let v = self.g.eng.values();
        self.g.eng.def_value(self.dvi, v.field(crate::assemble::END_FILE))
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
        let t0 = std::time::Instant::now();
        let key = (
            self.end_file(),
            self.g.eng.acc_value(self.unit, v.field("log")),
            self.g.eng.acc_value(self.unit, v.field("pagehash")),
            self.g.eval_errors.len(),
        );
        crate::timeline::note("output: the key (log, pagehash folds)", t0.elapsed());
        if let Some((k, x)) = &self.memo.locked().output
            && *k == key
        {
            return x.clone();
        }
        let x = self.output_made();
        self.memo.locked().output = Some((key, x.clone()));
        x
    }

    fn output_made(&self) -> (Arc<Vec<u8>>, String) {
        let v = self.g.eng.values();
        let mut dvi: Arc<Vec<u8>> = self
            .end_file()
            .map(|x| crate::assemble::file_bytes_shared(v, x))
            .unwrap_or_default();
        let t0 = std::time::Instant::now();
        let n = self.pages().len();
        crate::timeline::note("output: the pages", t0.elapsed());
        let t0 = std::time::Instant::now();
        let mut memo = self.memo.locked();
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

    /// A node by its raw id ([`Stats::reruns`]): its operation and
    /// the start of its first argument.
    #[must_use]
    pub fn describe_node(&self, raw: u64) -> String {
        describe(&self.g.eng, phitex_defuse::NodeId::from_raw(raw))
    }

    /// Up to three defined control sequences within two edits of `name`,
    /// for an undefined one's help: those the document's text `doc` has
    /// first, then nearest first; a name with a character not a letter
    /// only when `name` has it too (a format's internal names left out).
    fn near_names(&self, name: &str, doc: &str, known: &mut Option<Vec<String>>) -> Vec<String> {
        if name.is_empty() {
            return Vec::new();
        }
        let known = known.get_or_insert_with(|| {
            let v = self.g.eng.values();
            let mut names: Vec<String> = self
                .prelude
                .env
                .names()
                .filter_map(|k| k.strip_prefix("cs:").map(str::to_owned))
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
        let mut near: Vec<(bool, usize, &String)> = known
            .iter()
            .filter(|k| k.as_str() != name)
            .filter(|k| k.chars().all(|c| c.is_alphabetic() || name.contains(c)))
            .filter_map(|k| {
                let d = edit_distance(name, k);
                (d <= 2).then(|| (!uses_name(doc, k), d, k))
            })
            .collect();
        near.sort();
        near.into_iter()
            .take(3)
            .map(|(_, _, k)| k.clone())
            .collect()
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
        let mut kept = self.lines.locked();
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
        // (each text node's move since it was sliced, by its index)
        let mut shifts: phitex_defuse::FxHashMap<usize, i64> = phitex_defuse::FxHashMap::default();
        // (and each cut file's pieces', [`DocFile::shifts`])
        let doc_shifts: FxHashMap<u64, (ValueId, i64)> = self
            .g
            .ops
            .docs
            .locked()
            .values()
            .flat_map(|d| d.locked().shifts.iter().map(|(&k, &x)| (k, x)).collect::<Vec<_>>())
            .collect();
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
                // node's frame carries is the byte in the document as the
                // text node was sliced, moved by as much as an edit since
                // moved that node (a node an edit left keeps its text, and
                // the positions in it, [`apply_text`])
                let node = u64::from_ne_bytes(int(4).to_ne_bytes());
                let i = top(node);
                let shift = *shifts.entry(i).or_insert_with(|| {
                    let n = self.nodes.get(i).map(|x| x.1);
                    let sliced = n
                        .and_then(|n| self.g.eng.node_info(n))
                        .and_then(|info| info.args.first().copied())
                        .and_then(|fs| list_of(v, fs).first().copied())
                        .map(|f| list_of(v, f))
                        .filter(|xs| xs.first().is_some_and(|&t| str_of(v, t) == "s"))
                        .and_then(|xs| xs.get(6).map(|&p| int_of(v, p)));
                    let now = spans.get(i).copied().and_then(|s| i64::try_from(s).ok());
                    match (sliced, now) {
                        (Some(a), Some(b)) if contiguous => b - a,
                        _ => 0,
                    }
                });
                let at = usize::try_from(int(3) + shift).unwrap_or(0)
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
                // (a cut file's line: the place its piece was made at,
                // moved as far as an edit of the file moved the piece,
                // [`DocFile::shifts`])
                let node = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int(4).to_ne_bytes()));
                let shift = piece_shift(&self.g.eng, &doc_shifts, node);
                let at = usize::try_from(int(3) + shift).unwrap_or(0) + loc;
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
                let near = self.near_names(name, doc, &mut known);
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

    /// The diagnostics at no place alone (the engine's own warnings and
    /// errors, the format's): what [`Build::diagnostics`] gives but the
    /// document's, without placing those.
    #[must_use]
    pub fn unplaced_diagnostics(&self) -> Vec<Diagnostic> {
        let docname = self.opts.name.clone();
        let at = |severity: Severity, m: &String| Diagnostic {
            file: docname.clone(),
            range: (0, 0),
            line: 1,
            severity,
            message: m.clone(),
            context: String::new(),
            help: None,
        };
        let mut out: Vec<Diagnostic> = self.prelude.diags.clone();
        out.extend(self.g.eval_warnings.iter().map(|m| at(Severity::Warning, m)));
        out.extend(self.prelude.errors.iter().chain(&self.g.eval_errors).map(|m| at(Severity::Error, m)));
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
                    Cause::Environment { field } => format!("Environment {}", field_name(v, field)),
                    Cause::Structure => "Structure".to_owned(),
                    Cause::Range { field, by } => format!("Range {} by [{}]", field_name(v, field), d(Some(by))),
                    Cause::Created { by } => format!("Created by [{}]", d(by)),
                    Cause::Edited => "Edited".to_owned(),
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

    /// Every document node's bound (a measure): its text's start, the
    /// fields it may define (`None`: anything; a prefix ends in `*`),
    /// and the siblings it declares it takes (`None`: only its run
    /// knows).
    #[doc(hidden)]
    #[must_use]
    pub fn node_bounds(&self) -> Vec<(String, Option<Vec<String>>, Option<usize>)> {
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
                    phitex_defuse::MayDefine::Any => None,
                    phitex_defuse::MayDefine::Nothing => Some(Vec::new()),
                    phitex_defuse::MayDefine::Names(fs) => {
                        Some(fs.iter().map(|&f| crate::val::field_name(v, f)).collect())
                    }
                    phitex_defuse::MayDefine::Prefixes(ps) => {
                        Some(ps.iter().map(|p| format!("{p}*")).collect())
                    }
                };
                let h = crate::hints::hint_of_args(&self.g.ops.hints, v, &i.args);
                let claim = h.bound.as_ref().and_then(|b| b.claim);
                Some((text, md, claim))
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
            let (kind, field) = cause_parts(&c);
            let by = match c {
                Cause::Changed { site, .. } => site,
                Cause::Rebind { to, .. } => to,
                Cause::Range { by, .. } => Some(by),
                Cause::Output { of } => Some(of),
                Cause::Created { by } => by,
                _ => None,
            };
            out.push(format!(
                "{kind} {}\n    reader: {}\n    writer: {}",
                field.map_or_else(|| "-".to_owned(), |f| field_name(v, f)),
                describe(n),
                by.map_or_else(|| "-".to_owned(), describe)
            ));
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
                Cause::Range { by, field } => (Some(by), Some(field)),
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
                let (kind, field) = cause_parts(&c);
                let field = field.map_or_else(String::new, |f| field_name(v, f));
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
        let t0 = std::time::Instant::now();
        // (a PDF file's pages are the link's, each its identity there
        // (`crate::pdfout::SHOWN`, one contribution): not the fold of
        // every page's hash, which an edit would join again)
        let shown = self.pdf_layout().and_then(|_| {
            self.g
                .eng
                .acc_value(self.unit, v.field(crate::pdfout::SHOWN))
        });
        let acc = if let Some(s) = shown {
            Some(v.list(vec![ValueId::UNIT, s]))
        } else {
            let f = v.field("pagehash");
            self.g.eng.acc_value(self.unit, f).map(|x| {
                let r = self.g.eng.acc_value(self.unit, v.field(crate::pdfout::SHOWN));
                v.list(vec![x, r.unwrap_or(ValueId::UNIT)])
            })
        };
        crate::timeline::note("pages: the key", t0.elapsed());
        if let Some((k, x)) = &self.memo.locked().pages
            && *k == acc
        {
            return x.clone();
        }
        let x = match shown {
            Some(s) => v
                .seq_to_vec(s)
                .first()
                .map(|&l| list_of(v, l))
                .unwrap_or_default()
                .iter()
                .map(|&x| phitex_defuse::hash_words(0x7061_6765, &[v.digest(x)]))
                .collect(),
            None => self.pages_made(),
        };
        self.memo.locked().pages = Some((acc, x.clone()));
        x
    }

    fn pages_made(&self) -> Vec<u64> {
        let v = self.g.eng.values();
        let f = v.field("pagehash");
        let mut out: Vec<u64> = self
            .g
            .eng
            .acc_value(self.unit, f)
            .map(|s| {
                v.seq_to_vec(s)
                    .into_iter()
                    .map(|x| u64::from_ne_bytes(int_of(v, x).to_ne_bytes()))
                    .collect()
            })
            .unwrap_or_default();
        // (a PDF page's hash is its shape's: the link's identity of it
        // too, its sealed lines in it, `crate::pdfout::SHOWN`)
        let shown = self
            .g
            .eng
            .acc_value(self.unit, v.field(crate::pdfout::SHOWN))
            .and_then(|x| v.seq_to_vec(x).first().map(|&l| list_of(v, l)))
            .unwrap_or_default();
        if shown.len() == out.len() {
            for (h, s) in out.iter_mut().zip(shown) {
                *h = phitex_defuse::hash_words(*h, &[v.digest(s)]);
            }
        }
        out
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

/// Whether `text` has the control sequence `\name`: the name after a
/// backslash, and no letter after a name that ends in one.
fn uses_name(text: &str, name: &str) -> bool {
    let ends_in_letter = name.chars().last().is_some_and(char::is_alphabetic);
    text.match_indices('\\').any(|(k, _)| {
        text[k + 1..].strip_prefix(name).is_some_and(|rest| {
            !ends_in_letter || !rest.chars().next().is_some_and(char::is_alphabetic)
        })
    })
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

/// (`PHITEX_CHAIN_TSV=<file>`) The evaluation's realised critical chain appended to
/// `path`, one tab-separated line per link, highest run first: index,
/// height, node, operation, the cause's kind, its field, the prefix or
/// node that placed it or whose output it read, the run's run, commit,
/// re-ask and gap before it (ns), its run time over all attempts (ns), its unit and
/// parent; for a definition read, the definers before the writer that
/// defined the same value (relays), the first of them, the definer
/// before that, and the value; and its arguments as text (tabs and
/// newlines escaped). The
/// next line is the node it waited on. An evaluation starts with a line
/// `# eval <links> <realised path> <realised ns>`.
#[allow(clippy::too_many_lines)]
fn dump_chain(eng: &Engine, r: &Report, path: &str) {
    use phitex_defuse::Cause;
    use std::io::Write as _;
    let v = eng.values();
    let times: FxHashMap<u64, u128> = r
        .run_times
        .iter()
        .map(|(n, d)| (n.raw(), d.as_nanos()))
        .collect();
    // (the chain's run, commit, re-ask and gap before each, ns; its heights)
    let at: Vec<[u64; 4]> = eng.critical_chain_times().into_iter().map(|(_, t)| t).collect();
    let chain = eng.critical_chain_heights();
    let mut s = format!(
        "# eval {} {} {}\n",
        chain.len(),
        r.realised_critical_path,
        r.realised_critical_time.as_nanos()
    );
    let fname = |f: Field| field_name(v, f);
    for (i, (n, h, c)) in chain.iter().enumerate() {
        let (kind, field) = cause_parts(c);
        let field = field.map(fname).unwrap_or_default();
        let extra = match c {
            Cause::Created { by: Some(b) } => b.raw().to_string(),
            Cause::Output { of } => of.raw().to_string(),
            _ => String::new(),
        };
        let info = eng.node_info(*n);
        let op = info.as_ref().map_or("?", |i| i.op);
        let (unit, parent) = info.as_ref().map_or((String::new(), 0), |i| {
            (
                format!("{:?}", i.unit),
                i.parent.map_or(0, phitex_defuse::NodeId::raw),
            )
        });
        let text: String = info
            .as_ref()
            .map(|i| {
                i.args
                    .iter()
                    .take(3)
                    .map(|&x| v.render(x))
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect::<String>()
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\n', "\\n");
        let t = at.get(i).copied().unwrap_or_default();
        // (a definition read: the value the writer defined, and how many
        // definers before it defined that same value (relays: the value
        // came from further up), with the first of them)
        let relay = match c {
            Cause::Changed {
                field,
                site: Some(w),
            }
            | Cause::Rebind {
                field, to: Some(w), ..
            } => {
                let x = eng.def_value(*w, *field);
                let (mut cur, mut k) = (*w, 0u32);
                while k < 100_000 {
                    match eng.reaching_def(cur, *field) {
                        Some(p) if p != cur && eng.def_value(p, *field) == x => {
                            cur = p;
                            k += 1;
                        }
                        _ => break,
                    }
                }
                let first = eng.reaching_def(cur, *field);
                let val: String = x
                    .map(|x| v.render(x))
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect();
                format!(
                    "{k}\t{}\t{}\t{}",
                    cur.raw(),
                    first.map_or(0, phitex_defuse::NodeId::raw),
                    val.replace(['\t', '\n'], " ")
                )
            }
            _ => "\t\t\t".to_owned(),
        };
        let _ = writeln!(
            s,
            "{i}\t{h}\t{}\t{op}\t{kind}\t{field}\t{extra}\t{}\t{}\t{}\t{}\t{}\t{unit}\t{parent}\t{relay}\t{text}",
            n.raw(),
            t[0],
            t[1],
            t[2],
            t[3],
            times.get(&n.raw()).copied().unwrap_or(0)
        );
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(s.as_bytes());
    }
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
fn job_env(g: &mut Graph, unit: UnitId, env: &Env, made: &Arc<crate::fmtenv::Made>, opts: &Options) {
    // (the format's bindings the environment's base: each made when a
    // run first reads it, a value two bindings share made once; one no
    // run reads is never made, nor its name)
    {
        let (fmt, made) = (Arc::clone(env), Arc::clone(made));
        let base: EnvBase = Arc::new(move |v, f| fmt.get_field(v, f, &made));
        g.env_base = Some(Arc::clone(&base));
        g.eng.set_env_provider(Box::new(move |v, f| base(v, f).map(Some)));
    }
    let v = g.eng.values();
    let mut sets = vec![
        (v.field("jobname"), v.str(&opts.jobname)),
        (v.field("dvicomment"), v.str(&opts.comment)),
        (v.field("shellescape"), v.int(i64::from(opts.shell_escape))),
        (
            v.field("pdfdate"),
            v.str(&phitex_pdf::pdf_date(opts.epoch.unwrap_or(0))),
        ),
    ];
    if let Some(t) = opts.epoch {
        // §241 fix_date_and_time, at the job's start
        for (k, x) in date_params(t) {
            sets.push((v.field(k), v.int(x)));
        }
    }
    if crate::exec::obs_flag!("PHITEX_RECORD_CHECK") {
        // (the check reads the format's bindings too: all of them made)
        g.start_env
            .extend(env.bindings(v, made, 0..env.len()).iter().chain(&sets).copied());
    }
    // (spec-final §4.5: the job's bindings go with the environment, one
    // push for all of them, made again only where an entry changed; the
    // graph has one unit, so the unit's start and the environment are
    // one place to read them)
    let _ = unit;
    g.job = by_field(sets);
    let env = g.environment(opts.fixpoint);
    g.push_env(env);
}

/// `base` with `over` over it, both by field with each field once: by
/// field, `over`'s entry for a field both give.
fn merge_by_field(base: &[(Field, ValueId)], over: &[(Field, ValueId)]) -> Vec<(Field, ValueId)> {
    let mut merged = Vec::with_capacity(base.len() + over.len());
    let (mut i, mut j) = (0, 0);
    loop {
        match (base.get(i), over.get(j)) {
            (Some(a), Some(b)) if a.0 == b.0 => {
                merged.push(*b);
                (i, j) = (i + 1, j + 1);
            }
            (Some(a), Some(b)) if a.0.0.raw() > b.0.0.raw() => {
                merged.push(*b);
                j += 1;
            }
            (Some(a), _) => {
                merged.push(*a);
                i += 1;
            }
            (None, Some(b)) => {
                merged.push(*b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    merged
}

/// Bindings by field, each field once: the last given for it.
fn by_field(mut xs: Vec<(Field, ValueId)>) -> Vec<(Field, ValueId)> {
    // (reversed, then a stable sort: the last given first among a
    // field's, which the dedup keeps)
    xs.reverse();
    xs.sort_by_key(|e| e.0.0.raw());
    xs.dedup_by_key(|e| e.0);
    xs
}

/// The fields that differ between environments `old` and `new` (both
/// by field, [`merge_by_field`]), each with its new value (`None`: no
/// longer given); `None` where more than [`ENV_CHANGES`] do.
fn env_changes(
    old: &[(Field, ValueId)],
    new: &[(Field, ValueId)],
) -> Option<Vec<(Field, Option<ValueId>)>> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    loop {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if a.0 == b.0 => {
                if a.1 != b.1 {
                    out.push((b.0, Some(b.1)));
                }
                (i, j) = (i + 1, j + 1);
            }
            (Some(a), Some(b)) if a.0.0.raw() > b.0.0.raw() => {
                out.push((b.0, Some(b.1)));
                j += 1;
            }
            (Some(a), _) => {
                out.push((a.0, None));
                i += 1;
            }
            (None, Some(b)) => {
                out.push((b.0, Some(b.1)));
                j += 1;
            }
            (None, None) => break,
        }
        if out.len() > ENV_CHANGES {
            return None;
        }
    }
    Some(out)
}

/// The most fields [`env_changes`] lists: past them the environment is
/// set whole.
const ENV_CHANGES: usize = 64;

/// The graph's accumulators and loop-backs (their round-one values).
fn declare_fields(eng: &Engine) {
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
    // (a page written apart reads the fonts' loads before it for its
    // provisional numbers, which the link corrects: read past the pieces
    // in work, as few of them load a font, not waiting for all)
    let loads = eng.values().field(crate::pdfout::FONT_LOADS);
    eng.declare_accumulator(loads, Arc::new(crate::pages::MissMonoid));
    let allocs = eng.values().field(crate::pdfout::ALLOCS);
    eng.declare_accumulator(allocs, Arc::new(SeqMonoid));
    let shown = eng.values().field(crate::pdfout::SHOWN);
    eng.declare_accumulator(shown, Arc::new(SeqMonoid));
    let rels = eng.values().field(crate::pdfprim::RT_RELS);
    eng.declare_accumulator(rels, Arc::new(SeqMonoid));
    let linked = eng.values().field(crate::pdfout::LINKED);
    eng.declare_accumulator(linked, Arc::new(SeqMonoid));
    // (the link's line and the pieces' items as the link reads them,
    // whole: pairs, not joined sequences, `crate::val::CatMonoid`)
    for f in [crate::pdfout::XPAGES, crate::pages::LINK_ITEMS] {
        let fld = eng.values().field(f);
        eng.declare_accumulator(fld, Arc::new(crate::val::CatMonoid));
    }
    for f in crate::hints::COUNTERS
        .iter()
        .chain(&[crate::run::LOG_ERRORS, crate::pages::FIN_DONE])
    {
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

/// `PHITEX_DECL_TRACE`: a full environment map, by family: fields new,
/// changed, unchanged and dropped against the map supplied before.
fn env_trace(eng: &Engine, env: &[(Field, ValueId)]) {
    // (the last environment pushed, with the value store its fields are
    // of: a build after another in one process has its own, and an
    // earlier build's field is no name in it)
    #[allow(clippy::type_complexity)]
    static LAST: std::sync::Mutex<Option<(usize, FxHashMap<Field, ValueId>)>> =
        std::sync::Mutex::new(None);

    if !crate::exec::obs_flag!("PHITEX_DECL_TRACE") {
        return;
    }
    let v = eng.values();
    let store = std::ptr::from_ref(v) as usize;
    let mut l = LAST.locked();
    let old = l
        .take()
        .filter(|(s, _)| *s == store)
        .map(|(_, m)| m)
        .unwrap_or_default();
    let new: FxHashMap<Field, ValueId> = env.iter().copied().collect();
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
    *l = Some((store, new));
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
    // (`PHITEX_NODE_DUMP=<file>`, after the evaluation: the main unit's
    // nodes in text order, each its operation, a digest of its arguments
    // and of its output, to compare two runs' committed nodes)
    if when == "after the evaluation"
        && let Some(path) = crate::exec::obs_str!("PHITEX_NODE_DUMP")
        && let Some(root) = eng.node_info(phitex_defuse::NodeId::from_raw(1))
    {
        use std::fmt::Write as _;
        use std::hash::{Hash as _, Hasher as _};
        let v = eng.values();
        let dig = |x: ValueId| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            v.render(x).hash(&mut h);
            h.finish()
        };
        let mut out = String::new();
        for n in eng.order(root.unit) {
            let Some(i) = eng.node_info(n) else { continue };
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for &a in &i.args {
                dig(a).hash(&mut h);
            }
            let o = eng.output(n).map_or(0, dig);
            let head: String = i.args.first().map(|&a| v.render(a)).unwrap_or_default().chars().take(60).collect();
            let _ = write!(
                out,
                "{} {} {:?} parent {:?} claimer {:?} args {:016x} out {o:016x} {head}",
                n.raw(),
                i.op,
                i.state,
                i.parent.map(phitex_defuse::NodeId::raw),
                i.claimer.map(phitex_defuse::NodeId::raw),
                h.finish()
            );
            // (a page's finisher: what it gave back; `PHITEX_NODE_DUMP_READS`
            // names nodes whose reads follow)
            if i.op == "finish" {
                let r: String = eng.output(n).map(|x| v.render(x)).unwrap_or_default().chars().take(300).collect();
                let _ = write!(out, " | {r}");
            }
            out.push('\n');
            // (`PHITEX_NODE_DUMP_DEFS=1`: each node's definitions, a digest
            // of each value)
            if crate::exec::obs_flag!("PHITEX_NODE_DUMP_DEFS") {
                let mut ds: Vec<(String, u64)> = eng.defs_of(n).into_iter().map(|(f, x)| (v.render(f.0), dig(x))).collect();
                ds.sort();
                for (f, d) in ds {
                    let _ = writeln!(out, "  def {f} {d:016x}");
                }
                let mut cs: Vec<(String, u64, String)> = eng
                    .contribs_of(n)
                    .into_iter()
                    .map(|(f, x)| (v.render(f.0), dig(x), v.render(x).chars().take(200).collect()))
                    .collect();
                cs.sort();
                for (f, d, t) in cs {
                    let _ = writeln!(out, "  contrib {f} {d:016x} {t}");
                }
            }
            let argd = format!("{:016x}", {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                for &a in &i.args {
                    dig(a).hash(&mut h);
                }
                h.finish()
            });
            if crate::exec::obs_str!("PHITEX_NODE_DUMP_READS")
                .is_some_and(|l| l == "all" || l.split(',').any(|x| x.trim().parse::<u64>().ok() == Some(n.raw()) || x.trim() == argd))
            {
                let _ = writeln!(out, "    node {}", eng.debug_node(n));
                for l in eng.debug_reads(n).lines() {
                    let _ = writeln!(out, "    read {}", l.replace("}, ", "},\n    read "));
                }
            }
        }
        let _ = std::fs::write(path, out);
    }
    // (`PHITEX_ORDER_AROUND=a,b`: the unit's nodes in text order from
    // node `a` to node `b`, each as the engine has it)
    if let Some(ab) = crate::exec::obs_str!("PHITEX_ORDER_AROUND") {
        let ids: Vec<u64> = ab.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let [a, b] = ids[..]
            && let Some(i) = eng.node_info(phitex_defuse::NodeId::from_raw(a))
        {
            let order = eng.order(i.unit);
            let at = |x: u64| order.iter().position(|n| n.raw() == x);
            if let (Some(p), Some(q)) = (at(a), at(b)) {
                eprintln!("order {when} from {a} to {b}:");
                for n in &order[p.min(q)..=p.max(q)] {
                    let d = eng.debug_node(*n);
                    eprintln!("  {} {}", n.raw(), d.chars().take(4000).collect::<String>());
                }
            }
        }
    }
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
            defs_bytes: None,
            defs: OnceLock::new(),
            key: [3; 32],
            text_key: 5,
            reads: vec![("a.tex".to_owned(), 9)],
            env: {
                let v = Values::new();
                Arc::new(crate::fmtenv::FormatEnv::of(&v, &[("x".to_owned(), v.int(1))]))
            },
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
    fn by_lines(text: &str) -> (String, Vec<CommentMark>) {
        let (mut out, mut marks, mut at) = (String::new(), Vec::new(), Vec::new());
        for (n, line) in text.split_inclusive('\n').enumerate() {
            let (body, nl) = match line.strip_suffix('\n') {
                Some(b) => (b, "\n"),
                None => (line, ""),
            };
            canonical_line(body, n, &mut out, &mut marks, &mut at);
            out.push_str(nl);
        }
        fill_ids(&mut out, &at, &marks, &collisions(&marks));
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
            let (mut out, mut marks, mut at) = (String::new(), Vec::new(), Vec::new());
            canonical_lines(t, 7, &mut out, &mut marks, &mut at);
            fill_ids(&mut out, &at, &marks, &collisions(&marks));
            let (o, m) = by_lines(t);
            assert_eq!(out, o);
            assert_eq!(
                marks,
                m.into_iter()
                    .map(|(n, h, c)| (n + 7, h, c))
                    .collect::<Vec<_>>()
            );
        }
    }
}
