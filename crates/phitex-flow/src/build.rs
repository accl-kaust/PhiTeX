//! Evaluation as graph reduction (CORE.md, "Reduction").
//!
//! The source is cut into texts (blank lines at brace depth 0); each text
//! is a **piece**: text not evaluated yet, with the state it is entered
//! in. A piece's run evaluates it (`graph::run_piece`): a node per command,
//! each reading versions and defining versions. What a run finds out about
//! the sequence itself is a **local rewrite**, made where it was found by
//! the worker installing the run: an `\input` puts the file's pieces and
//! the rest of the calling line after the piece, `\endinput` mutes the
//! rest of its file, a run reading on past its text is joined with the
//! texts after it, a cut the cutter missed re-cuts the rest of the text,
//! the job's end mutes what follows. Pieces are named by structure (which
//! list, which of its texts) and ordered by labels with gaps, never by
//! position: nothing installed moves when the sequence is rewritten. There
//! are no rounds, and no run is ever matched to a text by the text.
//!
//! A read of a name from before a piece resolves in the names index (name
//! -> definitions by place) to the latest definition before it; each such
//! read is an **edge** (the readers index). A changed definition marks
//! exactly the readers whose value changed. Line breaking is a task per
//! paragraph; the page builder is a fold over the settled pieces' lines
//! that applies the output routine where it fires; pages become DVI or PDF
//! by pure functions. N equal threads run whatever the scheduler picks;
//! under `Reads::Speculate` a read takes the value known and is validated
//! when the run is installed. An edit is the same evaluation: the edited
//! texts are new pieces, everything whose reads still hold is kept. With
//! [`Build::set_converge`] the files the build writes and reads back
//! (`.aux`, `.toc`) are a loop-back: the build evaluates again, only the
//! readers of what changed, until what it reads is what it wrote (pdfTeX
//! run to convergence).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::dvi;
pub use crate::graph::MAIN_MEMORY;

/// The shipped pages' source places, for SyncTeX and the viewer.
pub mod sync;

/// Evaluation as graph reduction (the agreed design; the default engine).
mod reduce;

/// The cutter's default: a cut proposed every this many lines (at a line
/// beginning with a control sequence, at brace depth 0), besides blank
/// lines ([`Options::cut_lines`]).
pub const CUT_LINES: usize = 200;
use crate::graph::output::{self, DEAD_CYCLES, INSERT_PENALTIES, OutputReads};
use crate::graph::{Check, Env, FileText, Para, PieceId, RState, Run, Src, cut_every, cut_from, run_piece};
use crate::hash::{FxMap, FxSet, hash_of};
use crate::linebreak::break_lines;
use crate::logs::{self, Frag};
use crate::page::{CallLog, Fired, FoldEnv, IGNORE_DEPTH, Lines, Page, PageOrg, Routine, Scan, material, push_vertical};
use crate::show::RAW;
use crate::tfm::{Font, FontFiles, Kpse, KpseFormat};
use crate::val::{Key, Meaning, Shown, Tok, Val};

/// What a read does when the definition it finds is being remade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    /// Park until it is installed (or run its piece here, if nobody has).
    Suspend,
    /// Take the old value; validation at install catches a change.
    Speculate,
}

/// How the scheduler chooses among ready work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Earliest first.
    Priority,
    /// Uniformly at random (seeded): the scheduler affects speed, never
    /// results.
    Random(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub threads: usize,
    pub reads: Reads,
    pub order: Order,
    /// Print the graph into `Output::dump`.
    pub dump: bool,
    /// The format is e-TeX's extended mode (`etex -ini` given `*`): e-TeX's
    /// primitives are named (`val::initial`). Fixed for a `Build`'s life:
    /// pieces kept from an earlier update read the mode they ran with.
    pub etex: bool,
    /// TeX's main memory, in words of material a run may hold
    /// (`graph::Live`): past it, "TeX capacity exceeded" ends the run
    /// (tex.web 94, 120). [`MAIN_MEMORY`] is web2c's.
    pub main_memory: u64,
    /// The cutter proposes a cut every this many lines (at a line
    /// beginning with a control sequence, at brace depth 0) besides blank
    /// lines: a scheduling knob, the results are the same for any value
    /// (`usize::MAX`: blank lines only). [`CUT_LINES`] by default.
    pub cut_lines: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: 1,
            reads: Reads::Suspend,
            order: Order::Priority,
            dump: false,
            etex: true,
            main_memory: MAIN_MEMORY,
            cut_lines: CUT_LINES,
        }
    }
}

/// What a build is doing now, for a caller that shows progress while it
/// runs (the CLI's spinner): counters the build's threads store into and
/// another thread reads, never waited on. Given by [`Build::set_progress`].
#[derive(Debug, Default)]
pub struct Progress {
    /// The round under way (from 1; `Stats::rounds` when it is over).
    pub round: AtomicUsize,
    /// The pieces settled this round, of the pieces in all.
    pub settled: AtomicUsize,
    pub total: AtomicUsize,
    /// The pages shipped this round.
    pub pages: AtomicUsize,
    /// 0 idle, 1 laying the sequence out, 2 running the pieces, 3 writing
    /// the output.
    pub phase: std::sync::atomic::AtomicU8,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub pieces: usize,
    /// Piece runs this build (first runs and runs again).
    pub ran: usize,
    /// Command nodes run this build, and command nodes in all.
    pub commands_run: u64,
    pub commands: u64,
    /// Pieces whose nodes were all kept.
    pub kept: usize,
    /// Pieces run again because a definition a node of theirs read was
    /// remade.
    pub reruns: usize,
    /// Reads that parked, and parks resolved by running the piece here.
    pub parks: usize,
    pub leapfrogs: usize,
    /// Speculating runs abandoned while running (a prediction refuted, or
    /// a long run on predictions an earlier piece has not confirmed).
    pub abandoned: usize,
    pub breaks_run: usize,
    /// Paragraphs equal to one already broken, keeping its lines.
    pub breaks_reused: usize,
    pub pages: usize,
    pub pages_shipped: usize,
    /// Pages equal to the last build's page at their place.
    pub pages_reused: usize,
    /// Pages whose commands depend on where they start (written for a
    /// place), and bodies written again when the DVI was laid out.
    pub bodies_fixed: usize,
    pub bodies_again: usize,
    /// Evaluations of the sequence: one, and one more each time a run
    /// changed it (an `\input` read or changed, an `\endinput`).
    pub rounds: usize,
    /// Of them, the rounds made for pdfTeX's read-backs.
    pub pdf_rounds: usize,
    /// Each round's wall time.
    pub round_walls: Vec<Duration>,
    /// The last round was the one that may not lay the sequence out again
    /// (the rounds stopped placing anything new: `MAX_STALLED_ROUNDS`).
    pub forced_last: bool,
    /// Output routines run, and kept from the last build.
    pub routines: usize,
    pub routines_kept: usize,
    /// Work done by each thread (tokens read, items broken, scanned,
    /// shipped).
    pub work: Vec<u64>,
    pub tasks: Vec<u64>,
    /// Time spent by kind, all threads: runs, line breaking, the scan,
    /// shipping, installing (validation and marking).
    pub time_run: Duration,
    pub time_break: Duration,
    pub time_scan: Duration,
    pub time_ship: Duration,
    pub time_log: Duration,
    pub time_confirm: Duration,
    /// Of installing, the time the scheduler's lock was held.
    pub time_held: Duration,
    /// Serial time: cutting the source, setting up, joining the DVI.
    pub time_cut: Duration,
    pub time_setup: Duration,
    pub time_join: Duration,
    pub wall: Duration,
}

impl Stats {
    /// Adds an earlier round's counts of work done to these (what a round
    /// holds as a state, pieces, pages, kept, stays this round's).
    fn absorb(&mut self, o: Stats) {
        self.ran += o.ran;
        self.commands_run += o.commands_run;
        self.reruns += o.reruns;
        self.parks += o.parks;
        self.leapfrogs += o.leapfrogs;
        self.abandoned += o.abandoned;
        self.breaks_run += o.breaks_run;
        self.routines += o.routines;
        if self.work.len() < o.work.len() {
            self.work.resize(o.work.len(), 0);
            self.tasks.resize(o.tasks.len(), 0);
        }
        for (a, b) in self.work.iter_mut().zip(&o.work) {
            *a += b;
        }
        for (a, b) in self.tasks.iter_mut().zip(&o.tasks) {
            *a += b;
        }
        self.time_run += o.time_run;
        self.time_break += o.time_break;
        self.time_scan += o.time_scan;
        self.time_ship += o.time_ship;
        self.time_log += o.time_log;
        self.time_confirm += o.time_confirm;
        self.time_held += o.time_held;
        self.time_setup += o.time_setup;
        let later = std::mem::take(&mut self.round_walls);
        self.round_walls = o.round_walls;
        self.round_walls.extend(later);
    }
}

pub struct Output {
    pub dvi: Vec<u8>,
    /// The PDF file, in PDF mode (`\pdfoutput` positive at the first
    /// page; the DVI file is then empty).
    pub pdf: Vec<u8>,
    pub stats: Stats,
    pub errors: Vec<String>,
    /// The graph, as text.
    pub dump: String,
    /// What the nodes wrote to the log, in document order (`\message`
    /// texts; `\immediate\write` lines, ending in a newline):
    /// `print::log` lays it out as TeX's log.
    pub log: Vec<String>,
    /// The files `\openout` opened and `\write` wrote (immediate ones in
    /// document order, deferred ones as their pages were shipped, in DVI
    /// order): each file's name and contents, in the order they were
    /// first opened (`logs::replay`).
    pub writes: Vec<(String, String)>,
    /// The pages each job of a session shipped, in order (one job unless
    /// the source is a session's, `graph::jobs`).
    pub job_pages: Vec<usize>,
    /// In PDF mode, each page's glyph runs and rules where they were put
    /// (`pdf::content::Origin`: for SyncTeX and the viewer; not part of
    /// the pages' values).
    pub origins: Vec<Vec<crate::pdf::content::Origin>>,
    /// Every file the build read (`Key::File`, each name once, in document
    /// order), with where it was found (`./name` for one the run wrote) or
    /// `None`: the CLI's `-recorder` (`.fls`) and a watch's dependencies.
    pub files_read: Vec<(String, Option<String>)>,
    /// The files [`Build::set_derive`]'s function made this update (a
    /// `.bbl`, an `.ind`), as the evaluations read them, and its other
    /// files (a `.blg`, an `.ilg`): name and bytes.
    pub derived: Vec<(String, Vec<u8>)>,
    pub derived_side: Vec<(String, Vec<u8>)>,
    /// What the derivations said (a line each).
    pub notes: Vec<String>,
}

/// A file derived from the files a build writes: BibTeX's `.bbl` of an
/// `.aux`, makeindex's `.ind` of an `.idx`.
pub struct Derived {
    /// The file the document reads (empty: none, only a note).
    pub name: String,
    /// Its bytes (none: the derivation made none).
    pub bytes: Option<Vec<u8>>,
    /// Its other file (`.blg`, `.ilg`), not read by the document.
    pub side: Option<(String, Vec<u8>)>,
    pub note: String,
}

/// The files derived from the files a build writes, by a pure function of
/// them and of the files it reads itself (the `.bib` files, the styles):
/// part of the build's loop-back ([`Build::set_converge`]), so a derived
/// file is a value the next evaluation reads like any other, and only its
/// readers run again when it changes (CORE.md, "Reduction": BibTeX and
/// makeindex are nodes of the build, not runs between builds).
pub trait Derive: Send {
    /// What `writes` (each written file's name and text, TeX's
    /// characters) derive. `known(name)`: the build holds a version of
    /// file `name` (a derivation whose inputs are unchanged since its last
    /// call, and whose output the build holds, may make nothing).
    fn derive(&mut self, writes: &[(String, String)], known: &dyn Fn(&str) -> bool) -> Vec<Derived>;
}


/// A read answered before any definition.
const INITIAL: PieceId = u32::MAX;
/// The evaluations an update may make for pdfTeX's read-backs (then an
/// error, as TeX's capacity exceeded: CORE.md, "pdfTeX's read-backs").
const PDF_ROUNDS: usize = 8;
/// The evaluations of the loop-back of the files a build writes and reads
/// again ([`Build::set_converge`]), as latexmk's bound.
const FILE_ROUNDS: usize = 5;
/// How many times one piece's runs may rewrite the sequence in an update
/// before the build fails (the sequence would flip for ever).
const EDIT_LIMIT: u32 = 64;
/// How many times a piece may run in one update before the build fails as
/// a livelock.
const RUN_LIMIT: u32 = 4096;
/// The ids of the places between a piece's nodes (where the page builder
/// fired) begin here; pieces' ids are below.
const VIRT_BASE: PieceId = 0x8000_0000;
/// How long an idle worker waits before it looks whether every run in
/// progress is parked for good.
const STALL_CHECK: Duration = Duration::from_secs(2);
/// How many builds a piece that ran in an edit keeps its nodes for.
pub const HOT_BUILDS: u64 = 8;
/// How often (in nodes run) a run on predictions looks at them again.
const CHECK_EVERY: u64 = 1 << 12;
/// Nodes after which a run on predictions an earlier piece has not
/// confirmed yet is given up (its piece then waits for them).
const GIVE_UP_AFTER: u64 = 1 << 18;

/// A place in document order: (piece, node). A piece's exports are at
/// `(id, END)`; node `j` of a piece reads what is defined before `(id, j)`;
/// the page builder's definitions where it fired are at `(id, node)`.
/// Places are ordered by their pieces' order labels ([`Labels`]), never by
/// the ids.
pub type Pos = (PieceId, u32);
/// The node of a piece's exports: after all its nodes.
pub const END: u32 = u32::MAX;

/// The pieces in order: a label per piece, with gaps, so that pieces put
/// between two others take labels between theirs and nothing else moves
/// (the labels are spread again, in the same order, when a gap runs out).
/// A piece taken out keeps its last label: only what holds it by label
/// compares it again (the scheduler's sets, re-keyed at each spread).
#[derive(Default)]
struct Labels {
    ord: Vec<u64>,
    by: BTreeMap<u64, PieceId>,
    /// How many times the labels were spread.
    spreads: u64,
}

impl Labels {
    fn of(&self, id: PieceId) -> u64 {
        self.ord.get(id as usize).copied().unwrap_or(u64::MAX)
    }

    /// The order key of place `p`.
    fn key(&self, p: Pos) -> (u64, u32) {
        (self.of(p.0), p.1)
    }

    fn contains(&self, id: PieceId) -> bool {
        self.ord.get(id as usize).is_some_and(|o| self.by.get(o) == Some(&id))
    }

    fn first(&self) -> Option<PieceId> {
        self.by.values().next().copied()
    }

    fn next(&self, id: PieceId) -> Option<PieceId> {
        let o = self.of(id);
        self.by.range(o.saturating_add(1)..).next().map(|x| *x.1)
    }

    fn prev(&self, id: PieceId) -> Option<PieceId> {
        let o = self.of(id);
        self.by.range(..o).next_back().map(|x| *x.1)
    }

    /// The labels free between `anchor` (none: the start) and the piece
    /// after it.
    fn gap(&self, anchor: Option<PieceId>) -> (u64, u64) {
        let lo = anchor.map_or(0, |a| self.of(a));
        let hi = self.by.range(lo.saturating_add(1)..).next().map_or(u64::MAX - 1, |x| *x.0);
        (lo, hi)
    }

    /// Puts `ids`, in order, right after `anchor` (none: first).
    fn insert_after(&mut self, anchor: Option<PieceId>, ids: &[PieceId]) {
        if ids.is_empty() {
            return;
        }
        let n = ids.len() as u64;
        let (lo, hi) = self.gap(anchor);
        if hi.saturating_sub(lo) <= n + 1 {
            self.spread(n);
        }
        let (lo, hi) = self.gap(anchor);
        let step = (hi - lo) / (n + 1);
        for (i, &id) in ids.iter().enumerate() {
            let o = lo + step * (i as u64 + 1);
            if self.ord.len() <= id as usize {
                self.ord.resize(id as usize + 1, u64::MAX);
            }
            self.ord[id as usize] = o;
            self.by.insert(o, id);
        }
    }

    /// The labels spread evenly again, in the same order, with room for
    /// `more` between any two.
    fn spread(&mut self, more: u64) {
        let ids: Vec<PieceId> = self.by.values().copied().collect();
        self.by.clear();
        self.spreads += 1;
        let step = ((u64::MAX >> 2) / (ids.len() as u64 + 2)).max(more + 2);
        for (i, id) in ids.into_iter().enumerate() {
            let o = step * (i as u64 + 1);
            self.ord[id as usize] = o;
            self.by.insert(o, id);
        }
    }

    fn remove(&mut self, id: PieceId) {
        let o = self.of(id);
        if self.by.get(&o) == Some(&id) {
            self.by.remove(&o);
        }
    }

    /// The pieces in order.
    fn ids(&self) -> Vec<PieceId> {
        self.by.values().copied().collect()
    }
}

/// Per piece, what a thread looks at without the scheduler's lock: its
/// run is installed and valid ([`DONE`]), its run in progress is to stop
/// ([`CANCEL`]). In chunks made once and never moved.
struct Flags(RwLock<Vec<Box<[AtomicU8]>>>);

const FLAG_CHUNK: usize = 1 << 12;
const DONE: u8 = 1;
const CANCEL: u8 = 2;

impl Flags {
    fn new() -> Flags {
        Flags(RwLock::new(Vec::new()))
    }

    /// Calls `f` with piece `id`'s flags (chunks are made as ids grow).
    fn with<R>(&self, id: PieceId, f: impl FnOnce(&AtomicU8) -> R) -> R {
        let i = id as usize;
        let (c, o) = (i / FLAG_CHUNK, i % FLAG_CHUNK);
        {
            let v = self.0.read().expect("flags");
            if let Some(ch) = v.get(c) {
                return f(&ch[o]);
            }
        }
        let mut v = self.0.write().expect("flags");
        while v.len() <= c {
            v.push((0..FLAG_CHUNK).map(|_| AtomicU8::new(0)).collect());
        }
        f(&v[c][o])
    }

    fn get(&self, id: PieceId, f: u8) -> bool {
        self.with(id, |a| a.load(Ordering::Acquire) & f != 0)
    }

    fn set(&self, id: PieceId, f: u8, on: bool) {
        self.with(id, |a| {
            if on {
                a.fetch_or(f, Ordering::AcqRel);
            } else {
                a.fetch_and(!f, Ordering::AcqRel);
            }
        });
    }
}

/// The ids of the places between pieces' nodes (where the page builder
/// fired): an edge's source when a routine's definition answered a read.
/// Kept from build to build.
#[derive(Default, Clone)]
struct Virt {
    ids: FxMap<(PieceId, u32), PieceId>,
    at: FxMap<PieceId, Pos>,
    next: PieceId,
}

/// The length of the common prefix of `a` and `b`.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const CHUNK: usize = 4096;
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + CHUNK <= n && a[i..i + CHUNK] == b[i..i + CHUNK] {
        i += CHUNK;
    }
    i + a[i..n]
        .iter()
        .zip(&b[i..n])
        .take_while(|(x, y)| x == y)
        .count()
}

/// The length of the common suffix of `a` and `b`.
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    const CHUNK: usize = 4096;
    let n = a.len().min(b.len());
    let (a, b) = (&a[a.len() - n..], &b[b.len() - n..]);
    let mut i = 0;
    while i + CHUNK <= n && a[n - i - CHUNK..n - i] == b[n - i - CHUNK..n - i] {
        i += CHUNK;
    }
    i + a[..n - i]
        .iter()
        .rev()
        .zip(b[..n - i].iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

const SHARDS: usize = 64;

// ------------------------------------------------------------ names

/// Control sequences' names and their numbers (`Tok::Cs`, `Key::Cs`): one
/// table for a build, filled as names are met, kept from update to update.
/// A number is an alias of its name within the build, never part of a
/// version: two numbers are equal exactly when the names are, and a value's
/// content is the same whichever number its names were given.
pub struct CsTable {
    ids: Vec<RwLock<FxMap<Arc<str>, u32>>>,
    names: RwLock<Vec<Arc<str>>>,
}

/// Names given their numbers first, in this order ([`CS_PAR`]…).
const CS_FIRST: [&str; 3] = ["par", "nullfont", "\u{0}output"];

/// `\par`'s number.
pub const CS_PAR: u32 = 0;
/// `\nullfont`'s number.
pub const CS_NULLFONT: u32 = 1;
/// The output routine's opening command's number (`graph::output`).
pub const CS_OUTPUT: u32 = 2;

impl std::fmt::Debug for CsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CsTable")
    }
}

/// A build's table is itself.
impl PartialEq for CsTable {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for CsTable {}

impl Default for CsTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CsTable {
    pub fn new() -> CsTable {
        let t = CsTable { ids: (0..SHARDS).map(|_| RwLock::default()).collect(), names: RwLock::default() };
        for n in CS_FIRST {
            t.id(n);
        }
        t
    }

    /// The number of control sequence `name`.
    pub fn id(&self, name: &str) -> u32 {
        let s = (hash_of(name) % SHARDS as u64) as usize;
        if let Some(&i) = self.ids[s].read().expect("names").get(name) {
            return i;
        }
        let mut ids = self.ids[s].write().expect("names");
        if let Some(&i) = ids.get(name) {
            return i;
        }
        let a: Arc<str> = Arc::from(name);
        let mut names = self.names.write().expect("names");
        let i = u32::try_from(names.len()).expect("fewer than 2^32 names");
        names.push(a.clone());
        ids.insert(a, i);
        i
    }

    /// The name of control sequence number `id`.
    pub fn name(&self, id: u32) -> Arc<str> {
        self.names.read().expect("names").get(id as usize).cloned().unwrap_or_else(|| Arc::from(""))
    }
}

/// The input files read in a build, by name (`None`: not found): where
/// each was found, as TeX Live shows it, and its bytes.
type FileCache = Mutex<FxMap<Arc<str>, Option<(Arc<str>, Arc<[u8]>)>>>;

/// What the build answers itself, never a definition (`Env::file`,
/// `Env::jobname`, `Env::line_base`): the input files (each read once per
/// build, so a pure function of its name during it), the job's name, the
/// lines of the source before each piece.
struct Given {
    dirs: Arc<Vec<PathBuf>>,
    /// The lookup after `dirs`, as a program ([`Build::set_kpse`]).
    finder: Option<(Arc<dyn Kpse>, Arc<str>)>,
    /// The files given in memory (`Build::set_file`), before the
    /// directories'.
    mem: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    files: FileCache,
    /// The files PDF output read through the finder ([`Given::pdf_found`]).
    pdf: FileCache,
    job: Arc<str>,
    /// The lines of its file before each piece, by id.
    bases: RwLock<FxMap<PieceId, u32>>,
    /// The terminal's lines and the interaction mode before any
    /// definition (`Build::set_terminal`, `Build::set_interaction`), and
    /// the sizes TeX was compiled with.
    terminal: Option<Arc<[u8]>>,
    interaction: i32,
    /// The command line's `-interaction`, set at each job's start.
    interaction_option: Option<i32>,
    limits: crate::print::Limits,
    /// The PDF file's creation date (`Build::set_pdf_date`).
    pdf_date: i64,
    /// The PDF fold's answers, the random seed and the clock given.
    pdf_answers: Arc<PdfAnswers>,
    pdf_seed: i32,
    pdf_clock: i32,
    /// XeTeX's native fonts (`Build::set_xetex`).
    xe: Option<Arc<crate::native::Fonts>>,
}

impl Given {
    fn new(
        dirs: Arc<Vec<PathBuf>>,
        job: Arc<str>,
        mem: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    ) -> Given {
        Given {
            dirs,
            finder: None,
            mem,
            files: Mutex::default(),
            pdf: Mutex::default(),
            job,
            bases: RwLock::default(),
            terminal: None,
            interaction: 0,
            interaction_option: None,
            limits: crate::print::Limits::TEX_LIVE,
            pdf_date: 0,
            pdf_answers: Arc::default(),
            pdf_seed: 0,
            pdf_clock: 0,
            xe: None,
        }
    }

    /// XeTeX's `load_native_font` through the given fonts.
    fn native_font(&self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        match &self.xe {
            Some(x) => x.load(name, size, tracing),
            None => (None, Vec::new()),
        }
    }

    /// The file `name`: given in memory, else in the first directory
    /// holding it.
    fn file(&self, name: &str) -> Option<Arc<[u8]>> {
        self.found(name).map(|(_, b)| b)
    }

    /// Where file `name` was found, as TeX Live shows it: `./name` given
    /// in memory, in the first directory (the document's, kpathsea's
    /// current directory) or a relative one; else the path.
    fn path(&self, name: &str) -> Arc<str> {
        self.found(name)
            .map_or_else(|| Arc::from(format!("./{name}")), |(p, _)| p)
    }

    fn found(&self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        if let Some(b) = self.mem.get(name) {
            return Some((Arc::from(format!("./{name}")), b.clone()));
        }
        let mut g = self.files.lock().expect("files");
        if let Some(v) = g.get(name) {
            return v.clone();
        }
        // (the directories, the name as it is — with a finder, web2c's
        // `-output-directory`, tried before kpathsea — then the finder)
        let v = self
            .dirs
            .iter()
            .enumerate()
            .find_map(|(k, d)| {
                let b = std::fs::read(d.join(name)).ok()?;
                let shown = if k == 0 || d.is_relative() {
                    format!("./{name}")
                } else {
                    d.join(name).to_string_lossy().into_owned()
                };
                Some((Arc::from(shown), Arc::from(b)))
            })
            .or_else(|| self.by_finder(name, KpseFormat::Tex));
        g.insert(Arc::from(name), v.clone());
        v
    }

    /// File `name` of `format` as the finder finds it, if there is a
    /// finder: where, as TeX Live shows it, and its bytes.
    fn by_finder(&self, name: &str, format: KpseFormat) -> Option<(Arc<str>, Arc<[u8]>)> {
        let (f, prog) = self.finder.as_ref()?;
        let p = f.find(name, format, prog)?;
        let b = f.read(&p)?;
        let shown = if p.starts_with('/') { p } else { format!("./{}", p.trim_start_matches("./")) };
        Some((Arc::from(shown), b))
    }

    /// A file PDF output reads (the map file, a font file, an encoding):
    /// given in memory, else by the finder in the format pdfTeX asks for
    /// it ([`crate::kpse::pdf_format`]); without a finder, as an input
    /// file, then below TeX Live's font directories ([`crate::tfm::pdf_file`]).
    fn pdf_found(&self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        if self.finder.is_none() {
            return self.found(name).or_else(|| crate::tfm::pdf_file(name));
        }
        if let Some(b) = self.mem.get(name) {
            return Some((Arc::from(format!("./{name}")), b.clone()));
        }
        let mut g = self.pdf.lock().expect("pdf files");
        if let Some(v) = g.get(name) {
            return v.clone();
        }
        let v = self.by_finder(name, crate::kpse::pdf_format(name));
        g.insert(Arc::from(name), v.clone());
        v
    }

    /// The lines of its file before piece `id`.
    fn base(&self, id: PieceId) -> u32 {
        self.bases.read().expect("bases").get(&id).copied().unwrap_or(0)
    }

    /// What a read of `key` from piece `k` finds, if the build answers it.
    fn answer(&self, key: &Key, k: PieceId) -> Option<Val> {
        match key {
            Key::File(n) => Some(self.file(n).map_or(Val::Void, Val::Bytes)),
            Key::Job => Some(Val::Name(self.job.clone())),
            Key::LineBase => Some(Val::Int(self.base(k) as i32)),
            Key::PdfAns(a, b, c, d) => Some(
                self.pdf_answers.get(&(k, [*a, *b, *c, *d])).map_or(Val::Void, |v| Val::Int(*v)),
            ),
            Key::PdfSeed => Some(Val::Int(self.pdf_seed)),
            Key::PdfClock => Some(Val::Int(self.pdf_clock)),
            Key::PdfDate => Some(Val::Name(Arc::from(crate::pdf::pdf_date(self.pdf_date)))),
            _ => None,
        }
    }
}

type Defs = FxMap<Key, Vec<(Pos, Val)>>;

/// Every installed run's exports, and the page builder's definitions where
/// it fired: name -> (place, value), in document order (by the places'
/// labels). Read by every thread at once, in shards; written under the
/// scheduler's lock.
struct Names {
    shards: Vec<RwLock<Defs>>,
    /// Per shard, a bit per group of keys some piece has defined (set
    /// before the definition goes in, never cleared): a key whose bit is
    /// clear has only its initial value, found without the lock.
    defined: Vec<AtomicU64>,
    /// The same, for the names ever defined between a piece's nodes (by
    /// the page builder or an output routine).
    inner: Vec<AtomicU64>,
    cs: Arc<CsTable>,
    any_inner: AtomicBool,
    /// The names the build answers itself (files, the job, line bases).
    given: Given,
    etex: bool,
    /// The format's definitions, at the place before the first piece.
    format: Option<Arc<FormatDefs>>,
    /// The pieces' order.
    labels: Arc<RwLock<Labels>>,
    /// The folds' values at the nodes reading them (the page builder's
    /// quantities, the files the run wrote), by piece and node: the last
    /// build's, then this one's as the folds validate the reads.
    fold_vals: RwLock<FxMap<(PieceId, u32, Key), Val>>,
    /// Each written file's bytes as a read of it last found them, for a
    /// read at a place the folds have no value for yet.
    file_guess: RwLock<FxMap<Arc<str>, Val>>,
}

/// A format's definitions by name, and its fonts' numbering
/// (`crate::format::Format`), as the names index holds them.
#[derive(Default)]
struct FormatDefs {
    defs: FxMap<Key, Val>,
    fonts: Vec<Arc<str>>,
}

impl Names {
    fn new(given: Given, etex: bool, cs: Arc<CsTable>, format: Option<Arc<FormatDefs>>, labels: Arc<RwLock<Labels>>) -> Names {
        Names {
            format,
            cs,
            shards: (0..SHARDS).map(|_| RwLock::default()).collect(),
            defined: (0..SHARDS).map(|_| AtomicU64::new(0)).collect(),
            inner: (0..SHARDS).map(|_| AtomicU64::new(0)).collect(),
            any_inner: AtomicBool::new(false),
            given,
            etex,
            labels,
            fold_vals: RwLock::default(),
            file_guess: RwLock::default(),
        }
    }

    /// What a read of `key` at `at` is answered with when a fold of the
    /// build gives it (`graph::Env::fold_read`): the value the fold had
    /// there (this build's, else the last build's), else the names index's
    /// or the file as found. `None`: not a fold's.
    fn fold_value(&self, key: &Key, at: Pos, font: &Arc<Font>) -> Option<Val> {
        if !matches!(key, Key::Page(_) | Key::File(_)) {
            return None;
        }
        let known = self
            .fold_vals
            .read()
            .expect("fold values")
            .get(&(at.0, at.1, key.clone()))
            .cloned();
        Some(known.unwrap_or_else(|| match key {
            Key::File(n) => self
                .file_guess
                .read()
                .expect("file guesses")
                .get(n)
                .cloned()
                .unwrap_or_else(|| self.given.file(n).map_or(Val::Void, Val::Bytes)),
            _ => self.reaching(key, at, font).1,
        }))
    }

    /// What a read of `key` at `at` is validated against, and the
    /// definition the names index has there: a fold's read only ever
    /// against the fold's value at that node, every other read against
    /// the names index.
    fn read_now(&self, key: &Key, at: Pos, font: &Arc<Font>) -> (Option<Pos>, Val) {
        let (def, now, ..) = self.reaching(key, at, font);
        (def, self.fold_value(key, at, font).unwrap_or(now))
    }

    /// Whether a read of `key` at `at` that saw `saw` sees it still.
    fn read_holds(&self, key: &Key, at: Pos, font: &Arc<Font>, saw: &Val) -> bool {
        saw.seen_as(&self.read_now(key, at, font).1)
    }

    fn set_fold_value(&self, key: &Key, at: Pos, v: Val) {
        self.fold_vals.write().expect("fold values").insert((at.0, at.1, key.clone()), v);
    }

    fn any_inner(&self) -> bool {
        self.any_inner.load(Ordering::Acquire)
    }

    /// The shard of `key` and its bit there. (The hash only routes; the
    /// map's `==` decides.)
    fn route(key: &Key) -> (usize, u64) {
        let h = hash_of(key);
        ((h % SHARDS as u64) as usize, 1 << ((h >> 32) % 64))
    }

    /// The definition of `key` reaching place `at` (what is defined
    /// before it): its place (none: the initial value), its value, and the
    /// nodes of `at`'s piece it reaches, `lo..hi` (all of them, `0..END`,
    /// unless an element between the piece's nodes defines `key`).
    fn reaching(&self, key: &Key, at: Pos, font: &Arc<Font>) -> (Option<Pos>, Val, u32, u32) {
        if let Some(v) = self.given.answer(key, at.0) {
            return (None, v, 0, END);
        }
        let (s, bit) = Names::route(key);
        if self.defined[s].load(Ordering::Acquire) & bit == 0 {
            return (None, self.initial(key, font), 0, END);
        }
        let found = {
            let l = self.labels.read().expect("labels");
            let target = l.key(at);
            let g = self.shards[s].read().expect("names");
            g.get(key).map(|v| {
                let i = v.partition_point(|(p, _)| l.key(*p) < target);
                let hi = match v.get(i) {
                    Some(((k, j), _)) if *k == at.0 && *j != END => j + 1,
                    _ => END,
                };
                if i > 0 {
                    let (p, val) = &v[i - 1];
                    let lo = if p.0 == at.0 { p.1 + 1 } else { 0 };
                    (Some((*p, val.clone(), lo)), hi)
                } else {
                    (None, hi)
                }
            })
        };
        match found {
            Some((Some((p, val, lo)), hi)) => (Some(p), val, lo, hi),
            Some((None, hi)) => (None, self.initial(key, font), 0, hi),
            None => (None, self.initial(key, font), 0, END),
        }
    }

    /// The value of `key` before any definition: the format's if it
    /// defines it, else IniTeX's, but what the session gives (the
    /// terminal, the interaction mode).
    fn initial(&self, key: &Key, font: &Arc<Font>) -> Val {
        if let Some(v) = self.format.as_ref().and_then(|f| f.defs.get(key)) {
            return v.clone();
        }
        match key {
            Key::Interaction => Val::Int(self.given.interaction),
            Key::ReadStream(16) => match &self.given.terminal {
                Some(d) => Val::Stream(Arc::new(crate::val::Stream { data: d.clone(), next: 0, normal: true })),
                None => Val::Void,
            },
            _ => crate::val::initial_in(key, font, self.etex, self.given.xe.is_some(), Some(&self.cs)),
        }
    }

    /// Whether `key` may be defined between a piece's nodes.
    fn has_inner(&self, key: &Key) -> bool {
        let (s, bit) = Names::route(key);
        self.inner[s].load(Ordering::Acquire) & bit != 0
    }

    /// The value defined exactly at `at`, if any.
    fn at(&self, key: &Key, at: Pos) -> Option<Val> {
        let (s, _) = Names::route(key);
        let l = self.labels.read().expect("labels");
        let target = l.key(at);
        let g = self.shards[s].read().expect("names");
        let v = g.get(key)?;
        let i = v.partition_point(|(p, _)| l.key(*p) < target);
        v.get(i).filter(|(p, _)| *p == at).map(|(_, x)| x.clone())
    }

    /// Place `at` now defines `key` as `val` (none: no longer defines it).
    fn set(&self, at: Pos, key: &Key, val: Option<&Val>) {
        let (s, bit) = Names::route(key);
        if val.is_some() {
            self.defined[s].fetch_or(bit, Ordering::AcqRel);
            if at.1 != END {
                self.inner[s].fetch_or(bit, Ordering::AcqRel);
                self.any_inner.store(true, Ordering::Release);
            }
        }
        let l = self.labels.read().expect("labels");
        let target = l.key(at);
        let mut g = self.shards[s].write().expect("names");
        if val.is_none() && !g.contains_key(key) {
            return;
        }
        let v = g.entry(key.clone()).or_default();
        let i = v.partition_point(|(p, _)| l.key(*p) < target);
        let here = v.get(i).is_some_and(|(p, _)| *p == at);
        match (val, here) {
            (Some(val), true) => v[i].1 = val.clone(),
            (Some(val), false) => v.insert(i, (at, val.clone())),
            (None, true) => {
                v.remove(i);
            }
            (None, false) => {}
        }
    }

    /// Every definition piece `id` makes (its exports and those between
    /// its nodes) taken out: the names.
    fn drop_piece(&self, id: PieceId, exports: &[(Key, Val)]) -> Vec<Key> {
        let mut out = Vec::new();
        for (key, _) in exports {
            if self.at(key, (id, END)).is_some() {
                self.set((id, END), key, None);
                out.push(key.clone());
            }
        }
        out
    }
}

/// A name's readers: reader piece -> (its first node reading the name
/// from before the piece, the piece whose definition it read).
type ReadersOf = Edges;

/// The edges, name -> its readers, in shards by name (`Names::route`, as
/// the names index): a run's edges go in before the scheduler's lock is
/// taken (`run_and_install`), each shard locked once per run; under the
/// lock, `reached` and the edge moves lock the shard of each name they
/// look at. Between rounds the build alone holds it (`get_mut`, no
/// locking).
struct Readers(Vec<Mutex<FxMap<Key, ReadersOf>>>);

impl Default for Readers {
    fn default() -> Self {
        Readers((0..SHARDS).map(|_| Mutex::default()).collect())
    }
}

impl Readers {
    /// The shard holding `key`'s readers, locked.
    fn shard(&self, key: &Key) -> MutexGuard<'_, FxMap<Key, ReadersOf>> {
        self.0[Names::route(key).0].lock().expect("readers")
    }

    /// `key`'s readers, the build alone holding the index.
    fn get_mut(&mut self, key: &Key) -> Option<&mut ReadersOf> {
        self.0[Names::route(key).0].get_mut().expect("readers").get_mut(key)
    }

    /// Every name and its readers, the build alone holding the index.
    fn iter_mut(&mut self) -> impl Iterator<Item = (&Key, &mut ReadersOf)> {
        self.0.iter_mut().flat_map(|m| m.get_mut().expect("readers").iter_mut())
    }

    /// Piece `id`'s edge for `key` is now `e`.
    fn insert(map: &mut FxMap<Key, ReadersOf>, key: &Key, id: PieceId, e: (u32, PieceId)) {
        if let Some(m) = map.get_mut(key) {
            m.insert(id, e);
        } else {
            let mut m = Edges::default();
            m.insert(id, e);
            map.insert(key.clone(), m);
        }
    }
}


/// A name's edges: per reading piece (by id, in order), its first reader
/// node and the piece whose definition it read. Pieces are installed
/// mostly in order, so an edge is mostly added at the end.
#[derive(Clone, Debug, Default)]
struct Edges(Vec<Edge>);

/// A reading piece's edge: (its id, (its first reader node, the defining
/// piece)).
type Edge = (PieceId, (u32, PieceId));

impl Edges {
    fn find(&self, id: PieceId) -> Result<usize, usize> {
        match self.0.last() {
            None => Err(0),
            Some(l) if l.0 < id => Err(self.0.len()),
            _ => self.0.binary_search_by_key(&id, |e| e.0),
        }
    }
    fn get_mut(&mut self, id: PieceId) -> Option<&mut (u32, PieceId)> {
        let i = self.find(id).ok()?;
        Some(&mut self.0[i].1)
    }
    fn insert(&mut self, id: PieceId, e: (u32, PieceId)) {
        match self.find(id) {
            Ok(i) => self.0[i].1 = e,
            Err(i) => self.0.insert(i, (id, e)),
        }
    }
    fn remove(&mut self, id: PieceId) {
        if let Ok(i) = self.find(id) {
            self.0.remove(i);
        }
    }
    fn keys(&self) -> impl Iterator<Item = &PieceId> {
        self.0.iter().map(|e| &e.0)
    }
    fn shrink_to_fit(&mut self) {
        self.0.shrink_to_fit();
    }
}

impl<'a> IntoIterator for &'a Edges {
    type Item = (&'a PieceId, &'a (u32, PieceId));
    type IntoIter = std::iter::Map<std::slice::Iter<'a, Edge>, fn(&'a Edge) -> Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|e| (&e.0, &e.1))
    }
}

/// The PDF fold's answers to the PDF events (`Key::PdfAns`): by the
/// piece holding the command (an output routine's: the firing call's)
/// and the event's `what::Tag` with the field's slot.
pub type PdfAnswers = FxMap<(PieceId, [u32; 4]), i32>;

/// What the PDF side keeps between builds: the last fold's answers, the
/// PDF events after the last page, the givens (`Key::PdfSeed`,
/// `Key::PdfClock`), and the evaluations of this update made for the
/// read-backs (CORE.md, "pdfTeX's read-backs").
#[derive(Default, Clone)]
struct PdfKept {
    answers: Arc<PdfAnswers>,
    tail: crate::page::PdfEvents,
    seed: i32,
    clock: i32,
    rounds: usize,
}

/// The page builder's definitions where it fired: the piece, the node, the
/// names and values.
type FoldDefs = Vec<(PieceId, u32, Vec<(Key, Val)>)>;

/// The page builder's results kept between evaluations.
#[derive(Default, Clone)]
struct FoldKept {
    fancy: bool,
    defs: FoldDefs,
    memo: Vec<Arc<Memo>>,
}

/// An output routine's run: what it read (its tokens, the page, what the
/// fold handed it, the fold's definitions, the document's), and what it
/// gave.
struct Memo {
    /// The fire's ordinal its PDF events and their answers were filed
    /// under (`what::Tag`): a name, not an input; a memo reused at another
    /// fire has them filed again under that one.
    ordinal: u32,
    toks: Arc<Vec<Tok>>,
    page: Arc<crate::item::BoxNode>,
    fire: OutputReads,
    overlay: Vec<(Key, Val)>,
    reads: Vec<(Key, Val)>,
    exports: Vec<(Key, Val)>,
    items: Vec<crate::item::Item>,
    log: Vec<String>,
    errors: Vec<String>,
}

/// What a piece's run says of the sequence: the file its last node reads
/// (and where that node ends), whether it ends its file (`\endinput`) or
/// the job, whether it stopped at a cut the cutter missed, whether it reads
/// on past its text. The sequence is rewritten where a run says otherwise
/// than the sequence has it ([`apply_structure`]).
#[derive(Clone, Default, Debug)]
struct Expect {
    input: Option<Arc<FileText>>,
    stop: usize,
    endinput: bool,
    /// The job ends in it (`\end`, `\dump`): no piece follows.
    ends: bool,
    /// It ends at a cut the cutter missed (at `stop`): the rest of its
    /// text is cut again from there.
    cut: bool,
    /// It does not stop where the next text begins: its last command reads
    /// on past its text's end, or it leaves a state no piece is entered in
    /// (a paragraph, a group open). It and the next text are one piece.
    join: bool,
}

impl Expect {
    fn of(r: &Run) -> Expect {
        if r.nodes_len() == 0 && r.lean.is_none() {
            return Expect::default();
        }
        let out = r.out();
        let input = r.input().cloned();
        let cut = r.ends_at_cut();
        let join = input.is_none() && !cut && (r.crossed || !out.enterable());
        Expect {
            join,
            stop: if input.is_some() || cut { out.pos } else { 0 },
            cut,
            endinput: out.ends_file(),
            // (a session's next job begins by defining it 0)
            ends: r
                .exports
                .binary_search_by(|(k, _)| k.cmp(&Key::Ended))
                .is_ok_and(|i| r.exports[i].1 != Val::Int(0)),
            input,
        }
    }
}

type ListId = u32;

/// What a list's texts are.
#[derive(Clone)]
enum ListKind {
    /// The source: every job's main text.
    Root,
    /// A file a piece's `\input` read.
    File(Arc<FileText>),
    /// The rest of a piece's text after a cut the cutter missed, cut again.
    Recut,
}

/// A text the sequence reads, cut into texts at blank lines.
struct List {
    kind: ListKind,
    /// The piece whose run made it (none: the root).
    caller: Option<PieceId>,
    texts: Vec<Arc<[char]>>,
    /// The lines before each text, from the list's start (the root's: from
    /// its job's start).
    lines: Vec<u32>,
    /// Where the caller's run stopped (its `\input`'s end, or the cut).
    stop: usize,
    /// A file's: the rest of the caller's line, read after the file.
    rest: Option<PieceId>,
    alive: bool,
}

impl List {
    fn new(kind: ListKind, caller: Option<PieceId>, texts: Vec<Arc<[char]>>, stop: usize) -> List {
        let lines = line_starts(&texts, matches!(kind, ListKind::Root));
        List { kind, caller, texts, lines, stop, rest: None, alive: true }
    }

    fn file_name(&self) -> Option<&Arc<str>> {
        match &self.kind {
            ListKind::File(f) => Some(&f.name),
            _ => None,
        }
    }
}

/// The lines before each text (a root's counted again at each job's start).
fn line_starts(texts: &[Arc<[char]>], jobs: bool) -> Vec<u32> {
    let mut v = Vec::with_capacity(texts.len());
    let mut n = 0u32;
    for t in texts {
        if jobs && t.first() == Some(&crate::graph::JOB) {
            n = 0;
        }
        v.push(n);
        n += count_lines(t) as u32;
    }
    v
}

fn count_lines(t: &[char]) -> usize {
    t.iter().filter(|&&c| c == '\r').count()
}

/// What a piece's text is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// Texts of its list, whole (`Piece::span`: one, or several joined).
    Texts,
    /// The rest of piece `.0`'s line after its `\input`, the file read
    /// (then the texts `span` of its list, joined with it).
    Rest(PieceId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Unstarted,
    Running,
    Done,
}

/// How a piece is entered: the canonical start (a job's first text), the
/// state an `\input` node of piece `.0` left (its file's first piece), the
/// rest of piece `.0`'s line after the file, or the state the piece before
/// it leaves (`.0`, the live piece before it; assumed to be the canonical
/// start, or its file's start, until that piece has run).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entry {
    Start,
    File(PieceId),
    Rest(PieceId),
    After(PieceId),
}

/// A piece's dirty names, each from the first node of its run that may
/// read it otherwise.
type Dirty = BTreeMap<Key, u32>;

/// An edited piece's predecessor: its run, lines and text, which its run
/// starts from (every node whose text and reads are the same kept).
type Prev = (Arc<Run>, Vec<Option<Lines>>, Arc<[char]>);

/// A piece: a text of a list (or the rest of a line after an `\input`,
/// joined with texts after it), its place, and its run.
struct Piece {
    id: PieceId,
    /// In the sequence (else taken out, for good).
    alive: bool,
    /// After the job's end or its file's `\endinput`: not read (TeX stops
    /// before it), not run, kept for when that changes.
    muted: bool,
    list: ListId,
    shape: Shape,
    /// The texts of its list it holds whole, `from..to` (a rest: those
    /// joined after it).
    span: (u32, u32),
    /// The texts of its list its text comes from.
    covers: (u32, u32),
    /// The lines of its list from text `covers.0`'s start to its own.
    base_off: u32,
    /// The lines of its file before it (`Key::LineBase`).
    base: u32,
    text: Arc<[char]>,
    /// Where the texts it was joined from begin: its run stops at the
    /// first it can (CORE.md, cuts).
    stops: Arc<[usize]>,
    /// Its `\input`s are read in it (a file whose end leaves what the rest
    /// of the line goes on with, or whose last command reads on past it).
    inline: bool,
    /// Its run does not end at a cut the cutter missed (the re-cut's last
    /// text read on past its end).
    nocut: bool,
    /// What the sequence's mutes were last computed with of its run:
    /// (`ends`, `endinput`) (a run not installed valid says nothing).
    muting: (bool, bool),
    /// The file its run's `\input` read, as a list (its pieces follow it).
    file: Option<ListId>,
    /// The rest of its text after the cut its run stopped at, as a list.
    recut: Option<ListId>,
    /// Its runs' rewrites of the sequence this update.
    edits: u32,
    status: Status,
    run: Option<Arc<Run>>,
    lines: Vec<Option<Lines>>,
    prev: Option<Prev>,
    dirty: Dirty,
    runs: u32,
    /// The piece whose definition answers each of its run's reads (aligned
    /// with `Run::reads`).
    srcs: Vec<PieceId>,
    log: Option<Arc<Frag>>,
    /// What its run says of the sequence.
    got: Expect,
    /// Its reads suspend whatever the policy.
    suspend: bool,
    hot_until: u64,
    /// The state its run in progress was entered in.
    entered: Option<RState>,
    recheck: bool,
    /// Its run with and without its nodes: lean once the scan has passed.
    lean_later: Option<(Arc<Run>, Arc<Run>)>,
    /// Its run reads the page builder's quantities the scan has not
    /// validated yet.
    fold_unchecked: bool,
}

impl Piece {
    fn new(id: PieceId, list: ListId, shape: Shape, span: (u32, u32), covers: (u32, u32), base_off: u32, text: Arc<[char]>, stops: Arc<[usize]>) -> Piece {
        Piece {
            id,
            alive: true,
            muted: false,
            list,
            shape,
            span,
            covers,
            base_off,
            base: 0,
            text,
            stops,
            inline: false,
            nocut: false,
            muting: (false, false),
            file: None,
            recut: None,
            edits: 0,
            status: Status::Unstarted,
            run: None,
            lines: Vec::new(),
            prev: None,
            dirty: Dirty::new(),
            runs: 0,
            srcs: Vec::new(),
            log: None,
            got: Expect::default(),
            suspend: false,
            hot_until: 0,
            entered: None,
            recheck: false,
            lean_later: None,
            fold_unchecked: false,
        }
    }

    fn live(&self) -> bool {
        self.alive && !self.muted
    }
}

/// The sequence: the pieces (by id, taken-out ones kept as tombstones),
/// the lists they are texts of, their order.
struct Seq {
    p: Vec<Piece>,
    lists: Vec<List>,
    labels: Arc<RwLock<Labels>>,
    root: Option<ListId>,
    cut_lines: usize,
    /// The pieces of each file text read (the same `Arc`s while its text is).
    file_cuts: FxMap<usize, (Arc<str>, Vec<Arc<[char]>>)>,
}

impl Seq {
    fn new(cut_lines: usize) -> Seq {
        Seq { p: Vec::new(), lists: Vec::new(), labels: Arc::default(), root: None, cut_lines, file_cuts: FxMap::default() }
    }

    fn labels(&self) -> std::sync::RwLockReadGuard<'_, Labels> {
        self.labels.read().expect("labels")
    }

    /// The first live piece.
    fn first_live(&self) -> Option<PieceId> {
        let l = self.labels();
        let mut x = l.first();
        while let Some(k) = x {
            if self.p[k as usize].live() {
                return Some(k);
            }
            x = l.next(k);
        }
        None
    }

    /// The live piece after `k`.
    fn next_live(&self, k: PieceId) -> Option<PieceId> {
        let l = self.labels();
        let mut x = l.next(k);
        while let Some(j) = x {
            if self.p[j as usize].live() {
                return Some(j);
            }
            x = l.next(j);
        }
        None
    }

    /// The live piece before `k`.
    fn prev_live(&self, k: PieceId) -> Option<PieceId> {
        let l = self.labels();
        let mut x = l.prev(k);
        while let Some(j) = x {
            if self.p[j as usize].live() {
                return Some(j);
            }
            x = l.prev(j);
        }
        None
    }

    /// The live pieces in order.
    fn live_ids(&self) -> Vec<PieceId> {
        self.labels().ids().into_iter().filter(|&k| self.p[k as usize].live()).collect()
    }

    /// Whether `a` comes before `b`.
    fn before(&self, a: PieceId, b: PieceId) -> bool {
        let l = self.labels();
        l.of(a) < l.of(b)
    }

    fn ord(&self, k: PieceId) -> u64 {
        self.labels().of(k)
    }

    /// The last piece in order of the pieces under `k`: its file's rest
    /// (and what is under it), else its re-cut's last piece.
    fn extent_end(&self, k: PieceId) -> PieceId {
        let ps = &self.p[k as usize];
        if let Some(l) = ps.file
            && let Some(r) = self.lists[l as usize].rest
            && self.p[r as usize].alive
        {
            return self.extent_end(r);
        }
        if let Some(l) = ps.recut {
            let last = self.labels().ids().into_iter().rev().find(|&x| self.p[x as usize].alive && self.p[x as usize].list == l);
            if let Some(x) = last {
                return self.extent_end(x);
            }
        }
        k
    }

    /// Takes piece `k` and everything under it out of the sequence: the
    /// pieces taken out, in order.
    fn remove_extent(&mut self, k: PieceId) -> Vec<PieceId> {
        let end = self.extent_end(k);
        let mut out = Vec::new();
        {
            let l = self.labels();
            let mut x = Some(k);
            while let Some(j) = x {
                out.push(j);
                if j == end {
                    break;
                }
                x = l.next(j);
            }
        }
        let mut l = self.labels.write().expect("labels");
        for &j in &out {
            l.remove(j);
            let ps = &mut self.p[j as usize];
            ps.alive = false;
            for kid in [ps.file.take(), ps.recut.take()].into_iter().flatten() {
                self.lists[kid as usize].alive = false;
            }
        }
        out
    }

    /// A new piece (not in the order yet).
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, list: ListId, shape: Shape, span: (u32, u32), covers: (u32, u32), base_off: u32, text: Arc<[char]>, stops: Arc<[usize]>) -> PieceId {
        let id = PieceId::try_from(self.p.len()).expect("fewer pieces than ids");
        assert!(id < VIRT_BASE, "too many pieces");
        self.p.push(Piece::new(id, list, shape, span, covers, base_off, text, stops));
        id
    }

    fn add_list(&mut self, l: List) -> ListId {
        let id = ListId::try_from(self.lists.len()).expect("fewer lists than ids");
        self.lists.push(l);
        id
    }

    /// Puts `ids` after `anchor` in the order.
    fn place_after(&mut self, anchor: Option<PieceId>, ids: &[PieceId]) {
        self.labels.write().expect("labels").insert_after(anchor, ids);
    }

    /// The pieces of a file's text (the last cut's `Arc`s while the text is
    /// the same).
    fn file_pieces(&mut self, text: &Arc<str>) -> Vec<Arc<[char]>> {
        let key = text.as_ptr() as usize;
        if let Some((t, v)) = self.file_cuts.get(&key)
            && Arc::ptr_eq(t, text)
        {
            return v.clone();
        }
        let v = self
            .file_cuts
            .values()
            .find(|(t, _)| **t == **text)
            .map_or_else(|| cut_every(text, self.cut_lines), |x| x.1.clone());
        self.file_cuts.insert(key, (text.clone(), v.clone()));
        v
    }

    /// How deep piece `k`'s list is in files.
    fn depth(&self, k: PieceId) -> u32 {
        let mut d = 0;
        let mut l = self.p[k as usize].list;
        while let Some(c) = self.lists[l as usize].caller {
            d += 1;
            l = self.p[c as usize].list;
            if d > 1000 {
                break;
            }
        }
        d
    }

    /// Puts file `f`'s pieces after piece `k` (whose run stopped at `stop`,
    /// at its `\input`), then the rest of `k`'s line: the new pieces.
    fn make_file(&mut self, k: PieceId, f: Arc<FileText>, stop: usize) -> Vec<PieceId> {
        let texts = self.file_pieces(&f.text);
        let l = self.add_list(List::new(ListKind::File(f), Some(k), texts.clone(), stop));
        let mut ids = Vec::with_capacity(texts.len() + 1);
        for (i, t) in texts.iter().enumerate() {
            let i = i as u32;
            ids.push(self.add(l, Shape::Texts, (i, i + 1), (i, i + 1), 0, t.clone(), Arc::from([])));
        }
        let (kl, kc, ko, kt) = {
            let ps = &self.p[k as usize];
            (ps.list, ps.covers, ps.base_off, ps.text.clone())
        };
        let stop = stop.min(kt.len());
        let rest: Arc<[char]> = Arc::from(&kt[stop..]);
        let r = self.add(kl, Shape::Rest(k), (kc.1, kc.1), kc, ko + count_lines(&kt[..stop]) as u32, rest, Arc::from([]));
        ids.push(r);
        self.lists[l as usize].rest = Some(r);
        self.p[k as usize].file = Some(l);
        self.place_after(Some(k), &ids);
        ids
    }

    /// Puts the rest of piece `k`'s text after its run's stop, cut again,
    /// after it: the new pieces.
    fn make_recut(&mut self, k: PieceId, stop: usize) -> Vec<PieceId> {
        let text = self.p[k as usize].text.clone();
        let stop = stop.min(text.len());
        let src: String = text[stop..].iter().map(|&c| if c == '\r' { '\n' } else { c }).collect();
        let texts = cut_every(&src, self.cut_lines);
        let l = self.add_list(List::new(ListKind::Recut, Some(k), texts.clone(), stop));
        let mut ids = Vec::with_capacity(texts.len());
        for (i, t) in texts.iter().enumerate() {
            let i = i as u32;
            ids.push(self.add(l, Shape::Texts, (i, i + 1), (i, i + 1), 0, t.clone(), Arc::from([])));
        }
        self.p[k as usize].recut = Some(l);
        self.place_after(Some(k), &ids);
        ids
    }

    /// The live pieces after `k` in its list that its join takes (as many
    /// as make the text twice as long, one at least): none if `k` is its
    /// list's last.
    fn join_siblings(&self, k: PieceId) -> Vec<PieceId> {
        let ps = &self.p[k as usize];
        let end = self.extent_end(k);
        let mut out = Vec::new();
        let mut len = 0usize;
        let l = self.labels();
        let mut x = l.next(end);
        while let Some(j) = x {
            let q = &self.p[j as usize];
            if q.list == ps.list && q.live() {
                out.push(j);
                len += q.text.len();
                if len >= ps.text.len() {
                    break;
                }
                x = l.next(self.extent_end(j));
                continue;
            }
            // (out of its list: past its end)
            if !self.inside(j, ps.list) {
                break;
            }
            x = l.next(j);
        }
        out
    }

    /// Whether piece `j` is in list `l`, or under one of its pieces.
    fn inside(&self, j: PieceId, l: ListId) -> bool {
        let mut m = self.p[j as usize].list;
        loop {
            if m == l {
                return true;
            }
            match self.lists[m as usize].caller {
                Some(c) => m = self.p[c as usize].list,
                None => return false,
            }
        }
    }

    /// Joins piece `k` with `sibs` (the pieces after it in its list) into
    /// one piece at its place: the new piece, and those taken out.
    fn join(&mut self, k: PieceId, sibs: &[PieceId]) -> (PieceId, Vec<PieceId>) {
        let anchor = self.labels().prev(k);
        let (list, shape, span0, cov0, base_off, mut text, mut stops) = {
            let ps = &self.p[k as usize];
            (ps.list, ps.shape, ps.span.0, ps.covers.0, ps.base_off, ps.text.to_vec(), ps.stops.to_vec())
        };
        let mut span1 = self.p[k as usize].span.1;
        let mut cov1 = self.p[k as usize].covers.1;
        for &s in sibs {
            let q = &self.p[s as usize];
            stops.push(text.len());
            text.extend_from_slice(&q.text);
            span1 = span1.max(q.span.1);
            cov1 = cov1.max(q.covers.1);
        }
        let mut removed = self.remove_extent(k);
        for &s in sibs {
            if self.p[s as usize].alive {
                removed.extend(self.remove_extent(s));
            }
        }
        let j = self.add(list, shape, (span0, span1), (cov0, cov1), base_off, Arc::from(text), Arc::from(stops));
        self.place_after(anchor, &[j]);
        (j, removed)
    }

    /// How piece `k` is entered.
    fn entry_of(&self, k: PieceId) -> Entry {
        let ps = &self.p[k as usize];
        let list = &self.lists[ps.list as usize];
        if let Shape::Rest(c) = ps.shape {
            return Entry::Rest(c);
        }
        match list.kind {
            ListKind::Root if ps.span.0 == 0 || ps.text.first() == Some(&crate::graph::JOB) => Entry::Start,
            ListKind::File(_) if ps.span.0 == 0 => Entry::File(list.caller.expect("a file's caller")),
            _ => match self.prev_live(k) {
                Some(q) => Entry::After(q),
                None => Entry::Start,
            },
        }
    }

    /// The piece whose `\input` a piece entered after a cut is in the file
    /// of (its assumed entry's), if any.
    fn ctx_of(&self, k: PieceId) -> Option<PieceId> {
        let mut l = self.p[k as usize].list;
        loop {
            let list = &self.lists[l as usize];
            match (&list.kind, list.caller) {
                (ListKind::File(_), Some(c)) => return Some(c),
                (ListKind::Recut, Some(c)) => l = self.p[c as usize].list,
                _ => return None,
            }
        }
    }

    /// The last live piece of the file piece `c`'s `\input` read (none: an
    /// empty file).
    fn last_of(&self, c: PieceId) -> Option<PieceId> {
        let l = self.p[c as usize].file?;
        let r = self.lists[l as usize].rest?;
        self.prev_live(r).filter(|&x| x != c)
    }

    /// The pieces entered in a state piece `k` leaves.
    fn dependents(&self, k: PieceId) -> Vec<PieceId> {
        let mut v = Vec::new();
        if let Some(n) = self.next_live(k)
            && self.entry_of(n) == Entry::After(k)
        {
            v.push(n);
        }
        if let Some(l) = self.p[k as usize].file {
            if let Some(n) = self.next_live(k)
                && self.p[n as usize].list == l
            {
                v.push(n);
            }
            if let Some(r) = self.lists[l as usize].rest
                && self.p[r as usize].live()
            {
                v.push(r);
            }
        }
        let list = &self.lists[self.p[k as usize].list as usize];
        if let (ListKind::File(_), Some(c)) = (&list.kind, list.caller)
            && let Some(r) = list.rest
            && self.p[r as usize].live()
            && self.last_of(c) == Some(k)
            && !v.contains(&r)
        {
            v.push(r);
        }
        v
    }

    /// The pieces muted again: after a job's end and after each file's
    /// `\endinput` up to the file's end. Returns the pieces muted and those
    /// unmuted now.
    fn recompute_mutes(&mut self) -> (Vec<PieceId>, Vec<PieceId>) {
        let order = self.labels().ids();
        let mut ended = false;
        let mut cut: FxSet<ListId> = FxSet::default();
        let (mut muted, mut unmuted) = (Vec::new(), Vec::new());
        for k in order {
            let (list, starts_job, root) = {
                let ps = &self.p[k as usize];
                let root = matches!(self.lists[ps.list as usize].kind, ListKind::Root);
                (ps.list, root && ps.text.first() == Some(&crate::graph::JOB), root)
            };
            if starts_job {
                ended = false;
                cut.clear();
            }
            let _ = root;
            let caller_muted = self.lists[list as usize].caller.is_some_and(|c| self.p[c as usize].muted);
            let m = ended || cut.contains(&list) || caller_muted;
            let ps = &mut self.p[k as usize];
            if m != ps.muted {
                ps.muted = m;
                if m { muted.push(k) } else { unmuted.push(k) }
            }
            if !m && ps.run.is_some() {
                if ps.muting.0 {
                    ended = true;
                }
                if ps.muting.1 {
                    // (its file's pieces after it, and if it is in a re-cut,
                    // the pieces of the texts it was cut from after it)
                    let mut l = list;
                    loop {
                        cut.insert(l);
                        let li = &self.lists[l as usize];
                        match (&li.kind, li.caller) {
                            (ListKind::Recut, Some(c)) => l = self.p[c as usize].list,
                            _ => break,
                        }
                    }
                }
            }
        }
        (muted, unmuted)
    }

    /// Each live piece's lines before it in its file.
    fn set_bases(&mut self) {
        let order = self.labels().ids();
        let mut list_base: FxMap<ListId, u32> = FxMap::default();
        for k in order {
            let (l, c0, off) = {
                let ps = &self.p[k as usize];
                (ps.list, ps.covers.0, ps.base_off)
            };
            let lb = match list_base.get(&l) {
                Some(&b) => b,
                None => {
                    let li = &self.lists[l as usize];
                    let b = match (&li.kind, li.caller) {
                        (ListKind::Recut, Some(c)) => {
                            let cp = &self.p[c as usize];
                            cp.base + count_lines(&cp.text[..li.stop.min(cp.text.len())]) as u32
                        }
                        _ => 0,
                    };
                    list_base.insert(l, b);
                    b
                }
            };
            let li = &self.lists[l as usize];
            self.p[k as usize].base = lb + li.lines.get(c0 as usize).copied().unwrap_or(0) + off;
        }
    }

    /// What the log adds after each piece: a file's closing parenthesis
    /// after its last piece (" )" where the job ended in it).
    fn posts(&self) -> FxMap<PieceId, String> {
        let mut out: FxMap<PieceId, String> = FxMap::default();
        for k in self.live_ids() {
            let ps = &self.p[k as usize];
            let Some(l) = ps.file else { continue };
            if ps.inline {
                continue;
            }
            // (the job ended in the file: its parenthesis closes at the end)
            let rest_live = self.lists[l as usize].rest.is_some_and(|r| self.p[r as usize].live());
            let last = self.last_of(k).unwrap_or(k);
            let paren = if rest_live { ")" } else { " )" };
            out.entry(last).or_insert_with(|| RAW.to_string()).push_str(paren);
        }
        out
    }

    /// The root's texts are `texts` now: the pieces of texts no longer
    /// there (and of joins and lines holding them) taken out, pieces made
    /// for the new ones, each starting from the run of the piece it
    /// replaces. Returns the pieces taken out.
    fn edit_root(&mut self, texts: Vec<Arc<[char]>>) -> Vec<PieceId> {
        let Some(root) = self.root else {
            let l = self.add_list(List::new(ListKind::Root, None, texts.clone(), 0));
            self.root = Some(l);
            let ids: Vec<PieceId> = texts
                .iter()
                .enumerate()
                .map(|(i, t)| self.add(l, Shape::Texts, (i as u32, i as u32 + 1), (i as u32, i as u32 + 1), 0, t.clone(), Arc::from([])))
                .collect();
            self.place_after(None, &ids);
            return Vec::new();
        };
        let old = std::mem::take(&mut self.lists[root as usize].texts);
        let same = |a: &Arc<[char]>, b: &Arc<[char]>| Arc::ptr_eq(a, b) || **a == **b;
        let (ol, nl) = (old.len(), texts.len());
        let p = old.iter().zip(&texts).take_while(|(a, b)| same(a, b)).count();
        let s = old[p..].iter().rev().zip(texts[p..].iter().rev()).take_while(|(a, b)| same(a, b)).count();
        let (a, b, nb) = ((p as u32), (ol - s) as u32, (nl - s) as u32);
        if p == ol && p == nl {
            self.lists[root as usize].texts = old;
            return Vec::new();
        }
        let roots: Vec<PieceId> = self.labels().ids().into_iter().filter(|&x| self.p[x as usize].list == root).collect();
        let mut removed = Vec::new();
        // (old text index -> the run of the piece that held it)
        let mut prevs: BTreeMap<u32, Prev> = BTreeMap::new();
        for x in roots {
            if !self.p[x as usize].alive {
                continue;
            }
            let (f, t) = self.p[x as usize].covers;
            let hit = (f < b && t > a) || (a == b && f < a && a < t);
            if !hit {
                continue;
            }
            if self.p[x as usize].shape == Shape::Texts
                && let Some(r) = self.p[x as usize].run.clone().or_else(|| self.p[x as usize].prev.as_ref().map(|p| p.0.clone()))
            {
                let ps = &self.p[x as usize];
                prevs.insert(f, (r, ps.lines.clone(), ps.text.clone()));
            }
            removed.extend(self.remove_extent(x));
        }
        let delta = i64::from(nb) - i64::from(b);
        let shift = |v: u32| -> u32 { (i64::from(v) + delta) as u32 };
        for ps in &mut self.p {
            if ps.alive && ps.list == root && ps.covers.0 >= b {
                ps.span = (shift(ps.span.0), shift(ps.span.1));
                ps.covers = (shift(ps.covers.0), shift(ps.covers.1));
            }
        }
        self.lists[root as usize].lines = line_starts(&texts, true);
        self.lists[root as usize].texts = texts.clone();
        // (the texts no piece holds now, each a piece: after the piece
        // before it in the list, or what is under that)
        let mut held = vec![false; nl];
        let mut by_start: BTreeMap<u32, PieceId> = BTreeMap::new();
        for x in self.labels().ids() {
            let ps = &self.p[x as usize];
            if ps.alive && ps.list == root {
                for i in ps.span.0..ps.span.1 {
                    if let Some(h) = held.get_mut(i as usize) {
                        *h = true;
                    }
                }
                if ps.shape == Shape::Texts {
                    by_start.insert(ps.span.0, x);
                }
            }
        }
        for i in 0..nl {
            if held[i] {
                continue;
            }
            let i32_ = i as u32;
            let anchor = by_start.range(..i32_).next_back().map(|(_, &x)| self.extent_end(x));
            let id = self.add(root, Shape::Texts, (i32_, i32_ + 1), (i32_, i32_ + 1), 0, texts[i].clone(), Arc::from([]));
            // (the piece it replaces: at its place among the texts edited,
            // or the join that held it)
            let old_i = if i32_ < a { i32_ } else if i32_ < nb { (a + (i32_ - a)).min(b.saturating_sub(1).max(a)) } else { (i64::from(i32_) - delta) as u32 };
            let pv = prevs.get(&old_i).cloned().or_else(|| prevs.range(..=old_i).next_back().map(|(_, v)| v.clone()));
            self.p[id as usize].prev = pv;
            self.place_after(anchor, &[id]);
            by_start.insert(i32_, id);
        }
        removed
    }
}

/// The logs of the calls that fired a routine, by call ([`page::Scan::call_logs`]).
type CallLogs = Vec<(u32, Vec<CallLog>)>;

/// A run and the last node reading each name it reads.
type LastReaders = (Arc<Run>, Arc<FxMap<Key, u32>>);

type ParaLines = (Arc<Para>, Lines);

/// A piece of the last evaluation, in order, as its pages were made of it:
/// what SyncTeX and the viewer look pieces up by (by index).
struct Kept {
    id: PieceId,
    text: Arc<[char]>,
    run: Option<Arc<Run>>,
    lines: Vec<Option<Lines>>,
}

/// What survives between builds: the sequence with its runs, the edges,
/// the pages with their DVI or PDF.
pub struct Build {
    font: Arc<Font>,
    /// Where the build reports what it does now (none: nobody looks).
    board: Option<Arc<Progress>>,
    /// The folds' values the last evaluation's reads were validated
    /// against, by piece and node: the next one's predictions.
    fold_vals: FxMap<(PieceId, u32, Key), Val>,
    file_guess: FxMap<Arc<str>, Val>,
    /// Each piece's file (`None`: the main file) and the lines of that
    /// file before it ([`Build::source_place`]).
    places: FxMap<PieceId, (Option<Arc<str>>, u32)>,
    fonts: Arc<FontFiles>,
    /// The sequence, with every piece's run.
    seq: Seq,
    /// The last evaluation's live pieces, in order.
    pieces: Vec<Kept>,
    /// Piece id -> its index in `pieces`.
    dense: FxMap<PieceId, usize>,
    pages: Vec<(Page, Arc<dvi::Body>)>,
    page_orgs: Vec<PageOrg>,
    marks: Vec<(u32, u32)>,
    call_logs: CallLogs,
    readers: Readers,
    src: String,
    starts: Vec<usize>,
    texts: Vec<Arc<[char]>>,
    /// Files given in memory (`set_file`), found before the input
    /// directories' (a changed one is seen by its readers' reads).
    files: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    cut_lines: usize,
    /// What the page builder left: whether it fired a routine or met an
    /// insertion or a mark, its definitions at each place it fired, and
    /// its routines' runs.
    fold: FoldKept,
    cs: Arc<CsTable>,
    virt: Virt,
    inputs: Arc<Vec<PathBuf>>,
    finder: Option<(Arc<dyn Kpse>, Arc<str>)>,
    job: Arc<str>,
    terminal: Option<Arc<[u8]>>,
    interaction: i32,
    interaction_option: Option<i32>,
    limits: crate::print::Limits,
    lean: bool,
    max_rounds: usize,
    builds: u64,
    format: Option<Arc<FormatDefs>>,
    pdf_date: i64,
    etex_last: bool,
    xe: Option<Arc<crate::native::Fonts>>,
    pdf: PdfKept,
    /// The files the build writes and reads back are a loop-back
    /// ([`Build::set_converge`]).
    converge: bool,
    /// The loop-back's evaluations this update.
    file_rounds: usize,
    /// Names an edit took out or moved since the last evaluation: their
    /// readers are looked at again.
    touched: Vec<Key>,
    /// Reads refuted after an evaluation (a file read back otherwise than
    /// it was predicted): their pieces run again from the node.
    refuted: Vec<(PieceId, Key, u32)>,
    /// The files derived from the written ones ([`Build::set_derive`]).
    derive: Option<Mutex<Box<dyn Derive>>>,
    /// What the derivations made and said this update.
    derived: Vec<(String, Vec<u8>)>,
    derived_side: Vec<(String, Vec<u8>)>,
    notes: Vec<String>,
    /// Updates are evaluated by graph reduction ([`reduce`]); else by the
    /// pieces' scheduler (`PHITEX_ENGINE=pieces`).
    reduce: bool,
    /// The reductions of the last evaluation, by place: the memo an edit
    /// un-reduces against.
    rmemo: reduce::RMemo,
}

/// The scheduler's state.
struct State {
    /// The names each change to the names index changed, in order.
    names_log: Vec<Vec<Key>>,
    seq: Seq,
    unstarted: BTreeSet<(u64, PieceId)>,
    running: BTreeSet<(u64, PieceId)>,
    /// The labels' spreads `unstarted` and `running` are keyed after.
    spreads: u64,
    /// The first live piece not settled (none: all are): the pieces before
    /// it cannot change any more in this evaluation.
    settled: Option<PieceId>,
    /// The piece the settled frontier stopped at, known settled now.
    break_queue: VecDeque<(PieceId, usize, Arc<Para>)>,
    scan: Option<Box<Scan>>,
    /// The last piece the scan fed whole (none: none yet): settled, so
    /// never rewritten — the next is the live piece after it.
    scan_last: Option<PieceId>,
    /// A piece the scan fed part of, and how many of its items (it stopped
    /// inside it): the next to feed.
    scan_part: Option<(PieceId, u32)>,
    scan_done: bool,
    /// The thread running the scan in flight.
    scan_thread: Option<std::thread::ThreadId>,
    /// The last piece the scan in flight was given.
    scan_inflight: Option<PieceId>,
    /// Bumped when the scan is made again from the start: a task of an
    /// older one is not a result.
    scan_gen: u64,
    /// The last piece whose fonts are numbered (`numbers`).
    num_last: Option<PieceId>,
    /// The fonts' numbering before any piece (the format's).
    numbers0: Arc<dvi::Numbers>,
    /// The pieces are compared with what the last evaluation's scan was
    /// fed (`old_seq`, at `match_j`), not fed: at the first that differs
    /// the scan resumes.
    matching: bool,
    old_seq: Arc<Vec<PieceId>>,
    old_pos: FxMap<PieceId, usize>,
    match_j: usize,
    old_runs: FxMap<PieceId, Arc<Run>>,
    /// Once known: the last piece whose material differs from the last
    /// evaluation's (the scan can converge after it).
    last_diff: Option<PieceId>,
    last_diff_known: bool,
    pages: Vec<Page>,
    orgs: Vec<PageOrg>,
    bodies: Vec<Option<Arc<dvi::Body>>>,
    numbers: Arc<dvi::Numbers>,
    /// The last settled piece looked at for its part of the log.
    log_last: Option<PieceId>,
    log_queue: VecDeque<PieceId>,
    logs_running: usize,
    unshipped: usize,
    ship_queue: VecDeque<usize>,
    old_pages: Vec<Option<(Page, Arc<dvi::Body>)>>,
    old_orgs: Arc<Vec<PageOrg>>,
    old_orgs_whole: Arc<Vec<PageOrg>>,
    old_fancy: bool,
    old_marks: Vec<(u32, u32)>,
    old_pdf_tail: crate::page::PdfEvents,
    pdf_tail: crate::page::PdfEvents,
    marks_before: Vec<(u32, u32)>,
    marks_after: Vec<(u32, u32)>,
    old_call_logs: CallLogs,
    call_logs_before: CallLogs,
    call_logs_after: CallLogs,
    rng: u64,
    stats: Stats,
    done: bool,
    inflight: usize,
    /// The page builder's definitions made this evaluation, and the last
    /// one's not made again yet (by place, in order).
    fold_defs: FoldDefs,
    predicted: Vec<(Pos, Vec<Key>)>,
    memo_new: Vec<Arc<Memo>>,
    last_readers: FxMap<PieceId, LastReaders>,
    /// The scheduler stalled, or the sequence would not settle: the build
    /// fails with this, it never hangs.
    stalled: Option<String>,
}

struct Shared {
    names: Names,
    labels: Arc<RwLock<Labels>>,
    flags: Flags,
    board: Option<Arc<Progress>>,
    virt: RwLock<Virt>,
    /// Bumped (under `park`, and `parked` notified) whenever a run is
    /// installed.
    generation: AtomicU64,
    park: Mutex<()>,
    parked: Condvar,
    /// The runs parked now.
    parked_n: AtomicUsize,
    /// How many changes the names index has had.
    names_gen: AtomicUsize,
    /// Runs replaced, dropped on a thread of their own.
    drops: std::sync::mpsc::Sender<Arc<Run>>,
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    fonts: Arc<FontFiles>,
    opts: Options,
    /// The last evaluation's routine runs, by their page's first box.
    memo_old: FxMap<usize, Vec<Arc<Memo>>>,
    readers: Readers,
    lean: bool,
    build_no: u64,
    /// The settled frontier's label (`State::settled`), read without the
    /// lock.
    settled_at: AtomicU64,
}

impl Shared {
    /// The id of place `p` as an edge's source: a piece's own for its
    /// exports, one by place for a place between its nodes.
    fn id_at(&self, p: Pos) -> PieceId {
        if p.1 == END {
            return p.0;
        }
        if let Some(&id) = self.virt.read().expect("virt").ids.get(&p) {
            return id;
        }
        let mut v = self.virt.write().expect("virt");
        if let Some(&id) = v.ids.get(&p) {
            return id;
        }
        let id = v.next.max(VIRT_BASE);
        v.next = id + 1;
        v.ids.insert(p, id);
        v.at.insert(id, p);
        id
    }

    /// Where the definer `id` is, if it is in the sequence.
    fn pos_of(&self, id: PieceId) -> Option<Pos> {
        if id >= VIRT_BASE {
            let p = self.virt.read().expect("virt").at.get(&id).copied()?;
            return self.labels.read().expect("labels").contains(p.0).then_some(p);
        }
        self.labels.read().expect("labels").contains(id).then_some((id, END))
    }

    /// Whether place `a` is before place `b`.
    fn before(&self, a: Pos, b: Pos) -> bool {
        let l = self.labels.read().expect("labels");
        l.key(a) < l.key(b)
    }

    fn ord(&self, k: PieceId) -> u64 {
        self.labels.read().expect("labels").of(k)
    }
}

/// Where to look a routine run up: its page's first box (the same `Arc`
/// while the page begins with the same line), or 0.
fn memo_key(page: &crate::item::BoxNode) -> usize {
    page.list
        .iter()
        .find_map(|it| match it {
            crate::item::Item::Box(x) => Some(Arc::as_ptr(x) as usize),
            _ => None,
        })
        .unwrap_or(0)
}

enum Task {
    /// Run piece `.0` (again, from the nodes reading the names `.1`).
    Run(PieceId, Vec<(Key, u32)>),
    Break(PieceId, usize, Arc<Para>),
    Scan {
        scan: Box<Scan>,
        sgen: u64,
        /// The first piece, and how many items of its material to leave out.
        from: PieceId,
        skip: u32,
        /// Nothing differs from the last evaluation after this piece: a
        /// restart point after it equal to one of the last pages' begins
        /// the last pages from there on.
        converge: Option<(PieceId, Arc<Vec<PageOrg>>)>,
        /// The piece after the last fed (none: the end).
        to: Option<PieceId>,
        pieces: Vec<(PieceId, Arc<Run>, Vec<ParaLines>)>,
        finish: bool,
        predicted: Vec<Pos>,
        end_run: Option<(PieceId, Arc<Run>)>,
    },
    Ship(usize, Page, Arc<dvi::Numbers>, u64),
    Log(PieceId, Arc<[char]>, Arc<Run>, Vec<Option<Lines>>),
}

/// What an evaluation ended with: the output, or another evaluation to
/// make (a loop-back: what it read back is not what it wrote).
enum Eval {
    Done(Box<Output>),
    Again(Stats),
}

impl Build {
    /// A build whose documents can load `font` by its name, and the TFM
    /// files in TeX Live's directories.
    pub fn new(font: Arc<Font>) -> Build {
        let fonts = Arc::new(FontFiles::new(Self::given(&font), crate::tfm::default_dirs()));
        Build {
            font,
            board: None,
            fonts,
            seq: Seq::new(CUT_LINES),
            pieces: Vec::new(),
            dense: FxMap::default(),
            pages: Vec::new(),
            page_orgs: Vec::new(),
            marks: Vec::new(),
            call_logs: Vec::new(),
            readers: Readers::default(),
            fold_vals: FxMap::default(),
            file_guess: FxMap::default(),
            places: FxMap::default(),
            src: String::new(),
            starts: Vec::new(),
            inputs: Arc::new(Vec::new()),
            finder: None,
            job: Arc::from(crate::graph::JOBNAME),
            lean: true,
            max_rounds: PDF_ROUNDS + FILE_ROUNDS,
            builds: 0,
            texts: Vec::new(),
            files: Arc::default(),
            cut_lines: CUT_LINES,
            fold: FoldKept::default(),
            virt: Virt { next: VIRT_BASE, ..Virt::default() },
            cs: Arc::new(CsTable::new()),
            terminal: None,
            interaction: 0,
            interaction_option: None,
            limits: crate::print::Limits::TEX_LIVE,
            format: None,
            pdf_date: 0,
            pdf: PdfKept::default(),
            etex_last: false,
            xe: None,
            converge: false,
            file_rounds: 0,
            touched: Vec::new(),
            refuted: Vec::new(),
            derive: None,
            derived: Vec::new(),
            derived_side: Vec::new(),
            notes: Vec::new(),
            reduce: std::env::var("PHITEX_ENGINE").map_or(true, |v| v != "pieces"),
            rmemo: reduce::RMemo::default(),
        }
    }

    /// Evaluates by graph reduction (`true`, the default) or by the
    /// pieces' scheduler.
    pub fn set_reduce(&mut self, on: bool) {
        self.reduce = on;
    }

    /// The files derived from the files the build writes (BibTeX,
    /// makeindex: [`Derive`]), made inside the loop-back of
    /// [`Build::set_converge`]: an evaluation that wrote what a derivation
    /// reads is followed by the derivation, and its output, when it
    /// differs from the version the build holds, is read by the next
    /// evaluation (only its readers run again).
    pub fn set_derive(&mut self, d: Option<Box<dyn Derive>>) {
        self.derive = d.map(Mutex::new);
    }

    /// The files the build writes and reads back (`.aux`, `.toc`: a
    /// `\label`'s page read by a `\pageref`) are a loop-back in memory: an
    /// update evaluates again, only the readers of what changed, until what
    /// they read is what was written (pdfTeX run to convergence, as
    /// latexmk runs it; at most [`FILE_ROUNDS`] times). Off until set: an
    /// update is one TeX run.
    pub fn set_converge(&mut self, on: bool) {
        self.converge = on;
    }

    /// Takes pieces out of the build for good: their edges go, the names
    /// they defined are looked at again by their readers at the next
    /// evaluation.
    fn take_out(&mut self, removed: &[PieceId]) {
        if removed.is_empty() {
            return;
        }
        let gone: FxSet<PieceId> = removed.iter().copied().collect();
        for &k in removed {
            let ps = &mut self.seq.p[k as usize];
            if let Some(r) = ps.run.take() {
                for i in 0..r.read_count() {
                    if let Some(m) = self.readers.get_mut(r.read_at(i).0) {
                        m.remove(k);
                    }
                }
                self.touched.extend(r.exports.iter().map(|(key, _)| key.clone()));
            }
            ps.lines.clear();
            ps.log = None;
            ps.lean_later = None;
        }
        let defs = std::mem::take(&mut self.fold.defs);
        for (pid, node, d) in defs {
            if gone.contains(&pid) {
                self.touched.extend(d.into_iter().map(|x| x.0));
            } else {
                self.fold.defs.push((pid, node, d));
            }
        }
    }

    /// Builds `src`: what changed, and what the changes reach.
    pub fn update(&mut self, src: &str, opts: Options) -> Output {
        if self.reduce {
            return self.reduce_update(src, opts);
        }
        let t0 = Instant::now();
        self.builds += 1;
        self.etex_last = opts.etex;
        // (cut otherwise: every text is cut again, the sequence made anew)
        if opts.cut_lines != self.cut_lines {
            self.cut_lines = opts.cut_lines;
            self.texts.clear();
            self.src.clear();
            self.starts.clear();
            let all: Vec<PieceId> = self.seq.labels().ids();
            self.take_out(&all);
            self.seq = Seq::new(self.cut_lines);
        }
        let (texts, starts) = self.cut(src);
        self.src.clear();
        self.src.push_str(src);
        self.starts = starts;
        self.texts.clone_from(&texts);
        let removed = self.seq.edit_root(texts);
        self.take_out(&removed);
        let time_cut = t0.elapsed();
        self.pdf.rounds = 0;
        self.file_rounds = 0;
        self.derived.clear();
        self.derived_side.clear();
        self.notes.clear();
        let mut carried = Stats::default();
        let mut rounds = 0;
        loop {
            rounds += 1;
            if let Some(b) = &self.board {
                b.round.store(rounds, Ordering::Relaxed);
                b.pages.store(0, Ordering::Relaxed);
                b.phase.store(1, Ordering::Relaxed);
            }
            let t_round = Instant::now();
            let last = rounds >= self.max_rounds.max(1);
            match self.evaluate(opts, time_cut, last) {
                Eval::Done(out) => {
                    let mut out = *out;
                    carried.round_walls.push(t_round.elapsed());
                    out.stats.absorb(carried);
                    out.stats.rounds = rounds;
                    out.stats.forced_last = last;
                    out.stats.wall = t0.elapsed();
                    return out;
                }
                Eval::Again(mut st) => {
                    st.round_walls.push(t_round.elapsed());
                    carried.absorb(st);
                }
            }
        }
    }

    /// Piece `k`'s material as the page builder is fed it
    /// (`page::material`), as the last build left it.
    pub fn material_of(&self, k: usize) -> Option<Vec<crate::item::Item>> {
        let kept = self.pieces.get(k)?;
        let run = kept.run.as_ref()?;
        let paras: Option<Vec<(Arc<crate::item::Para>, Lines)>> = run
            .paras
            .iter()
            .zip(&kept.lines)
            .map(|(p, l)| Some((p.clone(), l.clone().unwrap_or_default())))
            .collect();
        Some(material(&paras?))
    }

    /// The reads piece `k`'s nodes hold, and how many distinct `Read`s
    /// they are.
    pub fn reads_held(&self, k: usize) -> Option<(usize, usize)> {
        Some(self.pieces.get(k)?.run.as_ref()?.reads_held())
    }

    /// The index in the last build's pieces of piece `id`.
    /// The live pieces, a line each (debugging).
    #[doc(hidden)]
    pub fn dump_pieces(&self) -> String {
        let mut out = String::new();
        for k in self.seq.live_ids() {
            let ps = &self.seq.p[k as usize];
            let head: String = ps.text.iter().take(40).collect::<String>().replace('\n', "|");
            let _ = writeln!(out, "{k} list {} {:?} join {} cut {} input {} ends {} len {} [{head}]", ps.list, ps.status, ps.got.join, ps.got.cut, ps.got.input.is_some(), ps.got.ends, ps.text.len());
        }
        out
    }

    pub fn piece_index(&self, id: PieceId) -> Option<usize> {
        self.dense.get(&id).copied()
    }

    /// The node (its index in piece `k`) that made an item of the piece's
    /// material, or, `path` naming items inside it, the item there. For
    /// SyncTeX and the viewer, with [`Build::source_place`].
    pub fn material_node(&self, k: usize, item: u32, path: &[usize]) -> Option<u32> {
        let kept = self.pieces.get(k)?;
        let run = kept.run.as_ref()?;
        let nodes = run.para_nodes();
        let mut i = item as usize;
        for (j, para) in run.paras.iter().enumerate() {
            let pn = *nodes.get(j)?;
            if i < para.before.len() {
                return inner_node(pn, &para.before[i], path);
            }
            i -= para.before.len();
            if para.lb.is_some() {
                let lines = kept.lines.get(j)?.as_ref()?;
                for l in lines.iter() {
                    let n = 1 + l.adjust.len() + usize::from(l.pen.is_some());
                    if i < n {
                        if i > 0 {
                            return Some(pn);
                        }
                        let Some((&first, rest)) = path.split_first() else { return Some(pn) };
                        let node = l.node_of(para, pn, first)?;
                        return inner_node(node, l.b.list.get(first)?, rest);
                    }
                    i -= n;
                }
            }
            if i < para.after.len() {
                return inner_node(pn, &para.after[i], path);
            }
            i -= para.after.len();
        }
        None
    }

    /// Where node `node` of piece `k` (as the last build laid it out) is in
    /// the source: its file (`None`: the main file) and line.
    pub fn source_place(&self, k: usize, node: u32) -> Option<(Option<Arc<str>>, u32)> {
        let kept = self.pieces.get(k)?;
        let (file, base) = self.places.get(&kept.id)?;
        let pos = kept.run.as_ref()?.node_pos(node as usize)?;
        let p = pos.min(kept.text.len());
        let line = 1 + kept.text[..p].iter().filter(|&&c| c == '\r').count() as u32;
        Some((file.clone(), base + line))
    }
    /// The format this build starts from (`format::decode` of `bytes`):
    /// its definitions are at the place before the first piece, where a
    /// name no piece defined before is read. Given before the first
    /// update.
    pub fn set_format(&mut self, bytes: &[u8]) -> Result<(), String> {
        let f = crate::format::decode(bytes, &self.cs)?;
        self.format = Some(Arc::new(FormatDefs { defs: f.defs.into_iter().collect(), fonts: f.fonts }));
        Ok(())
    }

    /// The format the build makes (`format::encode`): each name's
    /// definition reaching the end of the source, as the format's `\dump`
    /// leaves it (the format it started from, then each piece's exports in
    /// order), of the names a format keeps (`format::saved`); and the
    /// fonts' numbering (the format's, then the fonts each piece loads).
    pub fn format(&self) -> Result<Vec<u8>, String> {
        if !self.fold.defs.is_empty() {
            return Err("a format whose job fired an output routine is not saved yet".into());
        }
        let mut defs: FxMap<Key, Val> = self.format.as_ref().map(|f| f.defs.clone()).unwrap_or_default();
        let mut numbers = dvi::numbers([self.format_fonts()]);
        for p in &self.pieces {
            if let Some(r) = &p.run {
                for (k, v) in &r.exports {
                    defs.insert(k.clone(), v.clone());
                }
                dvi::extend(&mut numbers, &r.fonts);
            }
        }
        let defs: Vec<(Key, Val)> = defs.into_iter().filter(|(k, _)| crate::format::saved(k)).collect();
        let mut fonts: Vec<(u32, Arc<str>)> = numbers.into_iter().map(|(f, n)| (n, f)).collect();
        fonts.sort();
        let fonts: Vec<Arc<str>> = fonts.into_iter().map(|(_, f)| f).collect();
        crate::format::encode(&defs, &fonts, &self.cs)
    }

    /// The fonts the format the build starts from loaded, in order (the
    /// build's are numbered after them).
    fn format_fonts(&self) -> &[Arc<str>] {
        self.format.as_ref().map_or(&[], |f| &f.fonts[..])
    }

    /// The text of file `name` (what `\input name` and `\openin` find),
    /// from now on, before the input directories'.
    pub fn set_file(&mut self, name: &str, text: &str) {
        self.set_file_bytes(name, text.as_bytes());
    }

    /// The bytes of file `name` (as [`Build::set_file`]).
    pub fn set_file_bytes(&mut self, name: &str, bytes: &[u8]) {
        if self.files.get(name).is_some_and(|t| **t == *bytes) {
            return;
        }
        Arc::make_mut(&mut self.files).insert(Arc::from(name), Arc::from(bytes));
    }

    /// File `name` is no more in memory.
    pub fn remove_file(&mut self, name: &str) {
        Arc::make_mut(&mut self.files).remove(name);
    }

    /// Where `\input` and `\openin` find files: these directories, in
    /// order (a file's name is relative to them).
    pub fn set_input_dirs(&mut self, dirs: Vec<PathBuf>) {
        self.inputs = Arc::new(dirs);
    }

    /// How a file's name becomes a path (the CLI's: TeX Live's kpathsea,
    /// [`crate::kpse::Kpathsea`], as program `prog`: `tex`, `latex`,
    /// `pdftex`…): `\input` and `\openin` files (format `tex`) after the
    /// input directories (the name as it is: web2c's `-output-directory`),
    /// TFM files (`tfm`; instead of the font directories, until
    /// `set_font_dirs`) and the files PDF output reads (`map`, `type1
    /// fonts`, `enc files`…). A given of the build, never a node: a read of
    /// `Key::File` is answered as before (the file's bytes), only the
    /// lookup differs. Files given in memory (`set_file`) come first.
    pub fn set_kpse(&mut self, kpse: Arc<dyn Kpse>, prog: &str) {
        let prog: Arc<str> = Arc::from(prog);
        self.fonts = Arc::new(FontFiles::with_kpse(Self::given(&self.font), kpse.clone(), prog.clone()));
        self.finder = Some((kpse, prog));
    }

    /// The job's name (`\jobname`; `texput` until set).
    pub fn set_jobname(&mut self, name: &str) {
        self.job = Arc::from(name);
    }

    /// Whether settled pieces drop their nodes (`true` until set): with
    /// `false` an edit runs again only the nodes whose reads or text
    /// changed, holding every node in memory.
    pub fn set_lean(&mut self, lean: bool) {
        self.lean = lean;
    }

    /// (tests) How many rounds may lay the sequence out before the last,
    /// which may not ([`MAX_ROUNDS`] until set).
    #[doc(hidden)]
    /// Where to report what the build does now ([`Progress`]: the round,
    /// the pieces settled, the pages shipped), for a spinner; none by
    /// default.
    pub fn set_progress(&mut self, board: Option<Arc<Progress>>) {
        self.board = board;
    }

    pub fn set_max_rounds(&mut self, n: usize) {
        self.max_rounds = n;
    }

    /// The pieces of a file's text (the last cut's `Arc`s while the text
    /// is the same; the pieces of a changed text are found by their text).

    /// The terminal: the lines the user types (`Key::ReadStream(16)`
    /// before any definition), read by a job's start, an error's prompt
    /// in `\errorstopmode`, `\read16`. Unset, the terminal is closed.
    pub fn set_terminal(&mut self, lines: &[&str]) {
        let mut b = Vec::new();
        for l in lines {
            b.extend(l.bytes());
            b.push(b'\n');
        }
        self.terminal = Some(Arc::from(b));
    }

    /// The interaction mode before any definition (the command line's
    /// `-interaction`: 0 `batchmode` … 3 `errorstopmode`, IniTeX's own).
    /// Batch mode until set, as the oracles run TeX.
    pub fn set_interaction(&mut self, mode: i32) {
        self.interaction = mode.clamp(0, 3);
    }

    /// The command line's `-interaction` (none until set): web2c sets the
    /// mode from it at each job's start, after the format (tex.ch's
    /// `interaction_option`), over the mode the format dumped.
    pub fn set_interaction_option(&mut self, mode: Option<i32>) {
        self.interaction_option = mode.map(|m| m.clamp(0, 3));
    }

    /// The sizes TeX was compiled with (`print::Limits`; TeX Live's
    /// until set). Fixed for the build's life.
    pub fn set_limits(&mut self, limits: crate::print::Limits) {
        self.limits = limits;
    }

    /// The PDF file's creation date and `/ID`'s time, in seconds since
    /// 1970 UTC (pdfTeX's `SOURCE_DATE_EPOCH`; 0 until set). No clock is
    /// read: the date is a given of the build.
    pub fn set_pdf_seed_clock(&mut self, seed: i32, clock: i32) {
        self.pdf.seed = seed;
        self.pdf.clock = clock;
    }

    /// The PDF file's creation date (see [`Build::set_pdf_seed_clock`] for
    /// the random seed and the clock).
    pub fn set_pdf_date(&mut self, epoch: i64) {
        self.pdf_date = epoch;
    }

    /// The engine is XeTeX (`xetex -ini`, with e-TeX's extended mode as
    /// [`Options::etex`] says): its primitives and IniTeX values, Unicode
    /// input, native fonts from `fonts` (the installed fonts' index and
    /// the file finder, both given), XDV output and the PDF xdvipdfmx
    /// makes of it. Fixed for a `Build`'s life, given before the first
    /// update.
    pub fn set_xetex(&mut self, fonts: impl Into<Arc<crate::native::Fonts>>) {
        self.xe = Some(fonts.into());
    }

    /// XeTeX's PDF of an XDV file this build wrote (`Output::dvi`): what
    /// `xdvipdfmx -q -E -o JOB.pdf` makes of it, in process
    /// (`native::xdv_to_pdf`), dated by [`Build::set_pdf_date`]. `None`
    /// when the engine is not XeTeX.
    pub fn xdv_pdf(&self, xdv: &[u8]) -> Option<Result<Vec<u8>, String>> {
        let xe = self.xe.as_ref()?;
        let name = format!("{}.pdf", self.job);
        // (the build's finder if it has one, [`Build::set_kpse`], else the
        // input directories, then the native fonts' finder)
        let files: Arc<dyn crate::tfm::Kpse> = Arc::new(DocFiles {
            mem: self.files.clone(),
            dirs: if self.finder.is_some() { Arc::new(Vec::new()) } else { self.inputs.clone() },
            inner: self.finder.as_ref().map_or_else(|| xe.kpse().clone(), |(k, _)| k.clone()),
        });
        Some(crate::native::xdv_to_pdf(xdv, &files, xe.distiller(), &name, self.pdf_date))
    }

    /// The engine is XeTeX ([`Build::set_xetex`]).
    pub fn xetex(&self) -> bool {
        self.xe.is_some()
    }

    /// The sizes set by [`Build::set_limits`].
    pub fn limits(&self) -> crate::print::Limits {
        self.limits
    }

    /// Cuts `src` into pieces, the pieces outside the edit from the last
    /// source to this one being the last cut's (the same `Arc`s): the cut is
    /// made again from the piece before the one the edit begins in, up to
    /// the first piece start after the edit that is a piece start of the
    /// last cut (from a piece start on, the cut depends only on the text
    /// after it).
    fn cut(&self, src: &str) -> (Vec<Arc<[char]>>, Vec<usize>) {
        if self.texts.is_empty() {
            let (texts, starts, _) = cut_from(src, 0, self.cut_lines, |_| false);
            return (texts, starts);
        }
        let (ob, nb) = (self.src.as_bytes(), src.as_bytes());
        let pre = common_prefix(ob, nb);
        let olds = || self.texts.iter().cloned();
        if pre == ob.len() && pre == nb.len() {
            return (olds().collect(), self.starts.clone());
        }
        let suf = common_suffix(&ob[pre..], &nb[pre..]);
        let new_end = nb.len() - suf;
        // (the piece the edit begins in, and the one before it)
        let i = self.starts.partition_point(|&s| s <= pre).saturating_sub(1);
        let i0 = i.saturating_sub(1);
        let from = self.starts[i0];
        let mut again_at = None;
        let (mid, mid_starts, _) = cut_from(src, from, self.cut_lines, |p| {
            if p < new_end {
                return false;
            }
            // (p is in the unchanged tail: its place in the old source)
            let q = p + ob.len() - nb.len();
            match self.starts.binary_search(&q) {
                Ok(j) => {
                    again_at = Some(j);
                    true
                }
                Err(_) => false,
            }
        });
        let mut texts: Vec<Arc<[char]>> = olds().take(i0).collect();
        let mut starts: Vec<usize> = self.starts[..i0].to_vec();
        texts.extend(mid);
        starts.extend(mid_starts);
        if let Some(j) = again_at {
            texts.extend(olds().skip(j));
            starts.extend(self.starts[j..].iter().map(|&s| s + nb.len() - ob.len()));
        }
        (texts, starts)
    }

    fn given(font: &Font) -> Vec<(String, Arc<[u8]>)> {
        if font.data.is_empty() {
            Vec::new()
        } else {
            vec![(font.name.clone(), font.data.clone())]
        }
    }

    /// Where `\font` finds TFM files (searched below these directories).
    pub fn set_font_dirs(&mut self, dirs: Vec<std::path::PathBuf>) {
        self.fonts = Arc::new(FontFiles::new(Self::given(&self.font), dirs));
    }

    /// Builds `src`: what changed, and what the changes reach.

    /// One evaluation of the sequence: every piece whose reads do not hold
    /// runs, the page builder folds over the lines, the pages ship. Its
    /// output, or another evaluation to make (a loop-back).
    #[allow(clippy::too_many_lines)]
    fn evaluate(&mut self, opts: Options, time_cut: Duration, last: bool) -> Eval {
        let threads = opts.threads.max(1);
        let t_setup = Instant::now();
        let mut seq = std::mem::replace(&mut self.seq, Seq::new(self.cut_lines));
        seq.cut_lines = self.cut_lines;
        // (what the runs say of the job's end and each file's end)
        let (_, unmuted) = seq.recompute_mutes();
        for k in unmuted {
            let ps = &mut seq.p[k as usize];
            if ps.status == Status::Done {
                ps.status = Status::Unstarted;
            }
        }
        seq.set_bases();
        let mut given = Given::new(self.inputs.clone(), self.job.clone(), self.files.clone());
        given.finder.clone_from(&self.finder);
        given.terminal.clone_from(&self.terminal);
        given.interaction = self.interaction;
        given.interaction_option = self.interaction_option;
        given.limits = self.limits;
        given.pdf_date = self.pdf_date;
        given.pdf_answers = self.pdf.answers.clone();
        given.pdf_seed = self.pdf.seed;
        given.pdf_clock = self.pdf.clock;
        given.xe.clone_from(&self.xe);
        {
            let mut b = given.bases.write().expect("bases");
            for ps in &seq.p {
                if ps.alive {
                    b.insert(ps.id, ps.base);
                }
            }
        }
        let names = Names::new(given, opts.etex, self.cs.clone(), self.format.clone(), seq.labels.clone());
        names.file_guess.write().expect("file guesses").clone_from(&self.file_guess);
        names.fold_vals.write().expect("fold values").clone_from(&self.fold_vals);
        let order = seq.live_ids();
        // (the names index: what the pieces define, at their places, and
        // what an edited piece's predecessor defined, as its prediction)
        for &k in &order {
            let ps = &seq.p[k as usize];
            if let Some(r) = ps.run.as_ref().or(ps.prev.as_ref().map(|(r, _, _)| r)) {
                for (key, val) in &r.exports {
                    names.set((k, END), key, Some(val));
                }
            }
        }
        let virt = RwLock::new(std::mem::take(&mut self.virt));
        let id_at = |p: Pos| -> PieceId {
            if p.1 == END {
                return p.0;
            }
            if let Some(&id) = virt.read().expect("virt").ids.get(&p) {
                return id;
            }
            let mut v = virt.write().expect("virt");
            let id = v.next.max(VIRT_BASE);
            v.next = id + 1;
            v.ids.insert(p, id);
            v.at.insert(id, p);
            id
        };
        // (the last evaluation's runs, what its pages were made of)
        let old_runs: FxMap<PieceId, Arc<Run>> =
            self.pieces.iter().filter_map(|kp| kp.run.clone().map(|r| (kp.id, r))).collect();
        // (the page builder's definitions where it fired, in a piece kept as
        // it was: predictions until the page builder passes there again)
        let mut touched = std::mem::take(&mut self.touched);
        let mut predicted: Vec<(Pos, Vec<Key>)> = Vec::new();
        let fold = self.fold.clone();
        for (pid, node, defs) in &fold.defs {
            let kept = seq.p.get(*pid as usize).is_some_and(|ps| {
                ps.live()
                    && ps
                        .run
                        .as_ref()
                        .zip(old_runs.get(pid))
                        .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            });
            if kept {
                for (key, val) in defs {
                    names.set((*pid, *node), key, Some(val));
                }
                predicted.push(((*pid, *node), defs.iter().map(|d| d.0.clone()).collect()));
            } else {
                touched.extend(defs.iter().map(|d| d.0.clone()));
            }
        }
        {
            let l = seq.labels();
            predicted.sort_by_key(|x| l.key(x.0));
        }
        for (pos, keys) in &predicted {
            let ps = &seq.p[pos.0 as usize];
            if let Some(r) = ps.run.as_ref() {
                for key in keys {
                    refresh_export(&names, &self.font, pos.0, r, key);
                }
            }
        }
        // (the names whose definitions an edit took out or moved: their
        // readers are looked at, one reading another value now is marked)
        let mut readers = std::mem::take(&mut self.readers);
        touched.sort();
        touched.dedup();
        let mut marks: Vec<(PieceId, Key)> = Vec::new();
        let mut moves: Vec<(PieceId, Key, PieceId)> = Vec::new();
        for key in &touched {
            let Some(m) = readers.get_mut(key) else { continue };
            let mut dead = Vec::new();
            for (&rid, &(node, src)) in &*m {
                let Some(ps) = seq.p.get(rid as usize) else { continue };
                if !ps.alive {
                    dead.push(rid);
                    continue;
                }
                if !ps.live() {
                    continue;
                }
                let (def, ..) = names.reaching(key, (rid, node), &self.font);
                let def_id = def.map_or(INITIAL, id_at);
                if src == def_id {
                    continue;
                }
                if ps.run.as_ref().is_some_and(|run| reads_still(&names, &self.font, run, rid, key)) {
                    moves.push((rid, key.clone(), def_id));
                } else {
                    marks.push((rid, key.clone()));
                }
            }
            for d in dead {
                m.remove(d);
            }
        }
        for (r, key, src) in moves {
            move_edge(&readers, &mut seq.p[r as usize], &key, src);
        }
        // (the names the build answers itself — a file's bytes, the job's
        // name, the lines before a piece, the PDF answers — read with a
        // value they no longer have: their readers are marked)
        for (key, m) in readers.iter_mut() {
            if !matches!(
                key,
                Key::File(_) | Key::Job | Key::LineBase | Key::PdfAns(..) | Key::PdfSeed | Key::PdfClock | Key::PdfDate
            ) {
                continue;
            }
            for &rid in m.keys() {
                let Some(ps) = seq.p.get(rid as usize).filter(|p| p.live()) else { continue };
                let Some(r) = &ps.run else { continue };
                let changed = r.values_read(key).iter().any(|x| {
                    let now = match key {
                        Key::File(_) => names.fold_value(key, (rid, x.1), &self.font),
                        _ => names.given.answer(key, rid),
                    };
                    now.as_ref() != Some(&x.2)
                });
                if changed {
                    marks.push((rid, key.clone()));
                }
            }
        }
        // (reads the last evaluation found refuted: run again from there)
        for (k, key, n) in std::mem::take(&mut self.refuted) {
            if seq.p.get(k as usize).is_some_and(Piece::live) {
                dirty_from(&mut seq.p[k as usize].dirty, key, n);
                seq.p[k as usize].status = Status::Unstarted;
            }
        }
        // (a run entered otherwise than its piece is now runs again)
        for &k in &order {
            let ps = &seq.p[k as usize];
            if ps.status == Status::Done
                && let Some(r) = &ps.run
                && entry_state(&seq, k).is_some_and(|e| r.entry() != e)
            {
                seq.p[k as usize].status = Status::Unstarted;
            }
        }
        for &k in &order {
            let ps = &mut seq.p[k as usize];
            ps.edits = 0;
            ps.runs = 0;
            if ps.run.is_none() || !ps.dirty.is_empty() {
                ps.status = Status::Unstarted;
            }
        }
        let unstarted: BTreeSet<(u64, PieceId)> = {
            let l = seq.labels();
            order
                .iter()
                .filter(|&&k| seq.p[k as usize].status != Status::Done)
                .map(|&k| (l.of(k), k))
                .collect()
        };
        for &k in &order {
            if seq.p[k as usize].status != Status::Done {
                seq.p[k as usize].status = Status::Unstarted;
            }
        }
        let fancy = fold.fancy;
        let mut memo_old: FxMap<usize, Vec<Arc<Memo>>> = FxMap::default();
        for m in fold.memo {
            memo_old.entry(memo_key(&m.page)).or_default().push(m);
        }
        let matching = !self.page_orgs.is_empty() || !self.marks.is_empty();
        let old_orgs_whole = Arc::new(std::mem::take(&mut self.page_orgs));
        let old_orgs = if fancy { Arc::default() } else { old_orgs_whole.clone() };
        let old_seq: Arc<Vec<PieceId>> = Arc::new(self.pieces.iter().map(|kp| kp.id).collect());
        let old_pos: FxMap<PieceId, usize> = old_seq.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        let first = seq.first_live();
        let n = order.len();
        let mut state = State {
            names_log: Vec::new(),
            seq,
            unstarted,
            running: BTreeSet::new(),
            spreads: 0,
            settled: first,
            break_queue: VecDeque::new(),
            scan: Some(Box::default()),
            scan_last: None,
            scan_part: None,
            scan_done: false,
            scan_thread: None,
            scan_inflight: None,
            scan_gen: 0,
            num_last: None,
            numbers0: Arc::new(dvi::numbers([self.format_fonts()])),
            matching,
            old_seq,
            old_pos,
            match_j: 0,
            old_runs,
            last_diff: None,
            last_diff_known: false,
            pages: Vec::new(),
            orgs: Vec::new(),
            bodies: Vec::new(),
            numbers: Arc::new(dvi::numbers([self.format_fonts()])),
            log_last: None,
            log_queue: VecDeque::new(),
            logs_running: 0,
            unshipped: 0,
            ship_queue: VecDeque::new(),
            old_pages: std::mem::take(&mut self.pages).into_iter().map(Some).collect(),
            old_orgs_whole,
            old_orgs,
            old_fancy: fancy,
            old_marks: std::mem::take(&mut self.marks),
            old_pdf_tail: self.pdf.tail.clone(),
            pdf_tail: Arc::default(),
            marks_before: Vec::new(),
            marks_after: Vec::new(),
            old_call_logs: std::mem::take(&mut self.call_logs),
            call_logs_before: Vec::new(),
            call_logs_after: Vec::new(),
            rng: match opts.order {
                Order::Random(s) => s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
                Order::Priority => 1,
            },
            stats: Stats { pieces: n, work: vec![0; threads], tasks: vec![0; threads], ..Stats::default() },
            done: false,
            inflight: 0,
            fold_defs: Vec::new(),
            predicted,
            memo_new: Vec::new(),
            last_readers: FxMap::default(),
            stalled: None,
        };
        let flags = Flags::new();
        for &k in &order {
            flags.set(k, DONE, state.seq.p[k as usize].status == Status::Done);
        }
        for (k, key) in marks {
            mark(&mut state, &flags, k, key);
        }
        // (kept runs' paragraphs never broken go to the queue)
        for &k in &order {
            if state.seq.p[k as usize].status == Status::Done && !lines_ready(&state, k) {
                let r = state.seq.p[k as usize].run.clone().expect("a run");
                let equal = equal_paras(Some(&r), &r);
                queue_breaks(&mut state, k, Some(&r), &equal);
            }
        }
        state.stats.time_cut = time_cut;
        state.stats.time_setup = t_setup.elapsed();
        let labels = state.seq.labels.clone();
        let sh = Shared {
            names,
            labels,
            flags,
            board: self.board.clone(),
            virt,
            generation: AtomicU64::new(0),
            park: Mutex::new(()),
            parked: Condvar::new(),
            parked_n: AtomicUsize::new(0),
            names_gen: AtomicUsize::new(0),
            drops: {
                let (tx, rx) = std::sync::mpsc::channel::<Arc<Run>>();
                let _ = std::thread::Builder::new().name("flow-drop".into()).spawn(move || {
                    for r in rx {
                        drop(r);
                    }
                });
                tx
            },
            state: Mutex::new(state),
            cv: Condvar::new(),
            font: self.font.clone(),
            fonts: self.fonts.clone(),
            opts,
            memo_old,
            readers,
            lean: self.lean,
            build_no: self.builds,
            settled_at: AtomicU64::new(0),
        };
        {
            let mut g = lock(&sh);
            settle(&mut g, &sh);
            if let Some(b) = &sh.board {
                b.total.store(g.seq.live_ids().len(), Ordering::Relaxed);
                b.phase.store(2, Ordering::Relaxed);
            }
        }
        std::thread::scope(|s| {
            for id in 0..threads {
                let sh = &sh;
                std::thread::Builder::new()
                    .name(format!("flow-{id}"))
                    .stack_size(256 << 20)
                    .spawn_scoped(s, move || worker(sh, id))
                    .expect("spawn a worker");
            }
        });
        if let Some(b) = &sh.board {
            b.phase.store(3, Ordering::Relaxed);
        }
        // (the last evaluation's page builder definitions the scan did not
        // reach again — a fold that converged — stay as they were)
        let unreached: FoldDefs = {
            let mut g = lock(&sh);
            let mut out: FoldDefs = Vec::new();
            for (pos, keys) in std::mem::take(&mut g.predicted) {
                if g.fold_defs.iter().any(|x| x.0 == pos.0 && x.1 == pos.1) {
                    continue;
                }
                let defs: Vec<(Key, Val)> = keys.into_iter().filter_map(|k| sh.names.at(&k, pos).map(|v| (k, v))).collect();
                out.push((pos.0, pos.1, defs));
            }
            out
        };
        self.virt = std::mem::take(&mut *sh.virt.write().expect("virt"));
        let mut st = sh.state.into_inner().expect("state");
        self.readers = sh.readers;
        if let Some(d) = st.stalled.take() {
            panic!("{d}");
        }
        self.fold_vals.clone_from(&sh.names.fold_vals.read().expect("fold values"));
        let order = st.seq.live_ids();
        let dense: FxMap<PieceId, usize> = order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let t_join = Instant::now();
        let mut bodies: Vec<Arc<dvi::Body>> = st.bodies.iter().map(|b| b.clone().expect("every page shipped")).collect();
        let numbers = dvi::numbers(
            std::iter::once(self.format_fonts())
                .chain(order.iter().filter_map(|&k| st.seq.p[k as usize].run.as_ref().map(|r| &r.fonts[..]))),
        );
        // (how many fonts the job had loaded up to each piece)
        let loaded_by_piece: Vec<u32> = {
            let mut m = dvi::numbers([self.format_fonts()]);
            order
                .iter()
                .map(|&k| {
                    if let Some(r) = &st.seq.p[k as usize].run {
                        dvi::extend(&mut m, &r.fonts);
                    }
                    m.len() as u32
                })
                .collect()
        };
        let mut errors = Vec::new();
        let mut pdf_answers = None;
        let (out, pdf, again) = if st.pages.first().is_some_and(|p| p.ship.pdf.is_some()) {
            let format_fonts = self.format_fonts().len() as u32;
            let ship_fonts: Vec<u32> = st
                .orgs
                .iter()
                .map(|o| match dense.get(&o.call.0).and_then(|d| d.checked_sub(1)) {
                    Some(d) => loaded_by_piece.get(d).or(loaded_by_piece.last()).copied().unwrap_or(format_fonts),
                    None => format_fonts,
                })
                .collect();
            let w = pdf_file(&sh.names, &sh.font, &sh.fonts, order.last().copied(), &numbers, ship_fonts, &st.pages, &st.orgs, &st.pdf_tail, &mut bodies);
            errors.extend(w.errors.into_iter().map(|e| format!("pdf: {e}")));
            pdf_answers = Some(w.answers);
            (Vec::new(), w.bytes, w.again)
        } else {
            let xdv = sh.names.given.xe.is_some();
            let (out, again) = dvi::assemble_as(&numbers, &st.pages, &mut bodies, sh.names.given.limits.dvi_buf_size, xdv);
            (out, Vec::new(), again)
        };
        st.stats.bodies_again = again;
        let mut dump = String::new();
        let marks: Vec<(u32, u32)> = st
            .marks_before
            .iter()
            .chain(st.scan.as_ref().map_or(&[][..], |s| &s.marks[..]))
            .chain(&st.marks_after)
            .copied()
            .collect();
        for &k in &order {
            let ps = &mut st.seq.p[k as usize];
            let run = ps.run.clone().expect("every piece ran");
            if !ps.log.as_ref().is_some_and(|f| f.holds(&run, &ps.lines)) {
                ps.log = Some(Arc::new(logs::fragment(&ps.text, &run, &ps.lines)));
            }
        }
        let call_logs: CallLogs = st
            .call_logs_before
            .iter()
            .chain(st.scan.as_ref().map_or(&[][..], |s| &s.call_logs[..]))
            .chain(&st.call_logs_after)
            .cloned()
            .collect();
        let posts = st.seq.posts();
        let (log, writes, probes) = {
            let frags: Vec<&Frag> = order.iter().map(|&k| &**st.seq.p[k as usize].log.as_ref().expect("a part")).collect();
            let end = order.len().checked_sub(1).map_or((0, 0), |j| {
                (j, st.seq.p[order[j] as usize].run.as_ref().map_or(0, |r| r.nodes_len()))
            });
            let places = logs::page_places(&frags, &marks, st.pages.len(), end);
            let mut page_io = Vec::with_capacity(st.pages.len());
            for (j, (pg, at)) in st.pages.iter().zip(&places).enumerate() {
                let io = match at.map(|(x, i)| (order.get(x).copied(), i)) {
                    Some((Some(k), i)) => match st.seq.p[k as usize].run.as_ref() {
                        Some(r) if bodies[j].io && pg.builder => {
                            let mut env = JoinEnv { names: &sh.names, font: &sh.font, fonts: &sh.fonts, k, cut: false, split: false, stop: usize::MAX, virt: &sh.virt };
                            let r = regen(&mut env, &st.seq.p[k as usize].text, r);
                            let pos = crate::pdf::content::write_positions(pg);
                            crate::graph::page_io(&r.chain, i, &mut env, &pg.b, &pos, &mut errors)
                        }
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                page_io.push(io);
            }
            let parts: Vec<logs::Part<'_>> = order
                .iter()
                .map(|&k| {
                    let ps = &st.seq.p[k as usize];
                    logs::Part { frag: ps.log.as_ref().expect("a part"), base: ps.base as usize, post: posts.get(&k).map(String::as_str) }
                })
                .collect();
            let (log, writes, probes) = logs::replay_probed(logs::gather(&parts, &marks, &call_logs, &st.pages, &page_io), opts.etex);
            let probes: Vec<(PieceId, u32, Vec<(String, String)>)> = probes.into_iter().map(|(x, nd, f)| (order[x], nd, f)).collect();
            (log, writes, probes)
        };
        // (each read of a file the run wrote, against what it had written of
        // it by the reading node; a read that saw something else runs again
        // from its node, in another evaluation)
        let mut refuted: Vec<(PieceId, Key, u32)> = Vec::new();
        for (k, node, files) in &probes {
            let Some(r) = st.seq.p[*k as usize].run.as_ref() else { continue };
            for (key, nd, read) in r.all_reads() {
                let Key::File(name) = key else { continue };
                if nd != node {
                    continue;
                }
                let now = match files.iter().find(|f| *f.0 == **name) {
                    Some((_, text)) => Val::Bytes(text.chars().map(|c| u32::from(c).min(255) as u8).collect()),
                    None => sh.names.given.file(name).map_or(Val::Void, Val::Bytes),
                };
                if now != *read {
                    refuted.push((*k, key.clone(), *nd));
                }
                self.file_guess.insert(name.clone(), now.clone());
                sh.names.set_fold_value(key, (*k, *nd), now);
            }
        }
        self.fold_vals.clone_from(&sh.names.fold_vals.read().expect("fold values"));
        // (each piece's file and lines before it, for `source_place`)
        self.places = order
            .iter()
            .map(|&k| {
                let ps = &st.seq.p[k as usize];
                let mut l = ps.list;
                let file = loop {
                    let li = &st.seq.lists[l as usize];
                    match (&li.kind, li.caller) {
                        (ListKind::Recut, Some(c)) => l = st.seq.p[c as usize].list,
                        _ => break li.file_name().cloned(),
                    }
                };
                (k, (file, ps.base))
            })
            .collect();
        // (the files read, each once, in document order, where found)
        let mut files_read: Vec<(String, Option<String>)> = Vec::new();
        let mut read_names: FxSet<Arc<str>> = FxSet::default();
        for &k in &order {
            let Some(r) = st.seq.p[k as usize].run.as_ref() else { continue };
            let mut fr: Vec<(u32, &Arc<str>, bool)> = r
                .all_reads()
                .iter()
                .filter_map(|(key, n, v)| match key {
                    Key::File(name) => Some((*n, name, matches!(v, Val::Bytes(_)))),
                    _ => None,
                })
                .collect();
            fr.sort_by_key(|x| x.0);
            for (_, name, found) in fr {
                if read_names.insert(name.clone()) {
                    files_read.push((name.to_string(), found.then(|| sh.names.given.path(name).to_string())));
                }
            }
        }
        for &k in &order {
            let ps = &st.seq.p[k as usize];
            let r = ps.run.as_ref().expect("every piece ran");
            for e in &r.errors {
                errors.push(format!("piece {}: {e}", dense[&k]));
            }
            if ps.runs == 0 {
                st.stats.kept += 1;
            }
            st.stats.commands += r.nodes_len() as u64;
            if opts.dump {
                let mut env = JoinEnv { names: &sh.names, font: &sh.font, fonts: &sh.fonts, k, cut: false, split: false, stop: usize::MAX, virt: &sh.virt };
                let r = regen(&mut env, &ps.text, r);
                dump_piece(&mut dump, dense[&k], ps, &r, &sh.names.cs, &st.seq);
            }
        }
        if let Some(sc) = &st.scan {
            for e in &sc.errors {
                errors.push(format!("page builder: {e}"));
            }
        }
        if opts.dump {
            for (i, pg) in st.pages.iter().enumerate() {
                let _ = writeln!(dump, "page {}: {} items", i + 1, pg.b.list.len());
            }
        }
        let jobs = job_of(order.iter().map(|&k| st.seq.p[k as usize].text.first() == Some(&crate::graph::JOB)));
        let mut job_pages = vec![0usize; jobs.last().map_or(1, |j| j + 1)];
        for o in &st.orgs {
            let j = dense.get(&o.call.0).and_then(|&d| jobs.get(d)).copied().unwrap_or(0);
            job_pages[j] += 1;
        }
        st.stats.time_join = t_join.elapsed();
        st.stats.pages = st.pages.len();
        let mut fold_defs = std::mem::take(&mut st.fold_defs);
        fold_defs.extend(unreached);
        self.fold = FoldKept {
            fancy: st.scan.as_ref().is_some_and(|s| s.fancy),
            defs: fold_defs,
            memo: std::mem::take(&mut st.memo_new),
        };
        // (the loop-back: what was written and is read again)
        let mut again_why: Option<&str> = None;
        if !refuted.is_empty() {
            if last {
                errors.push("the files read back did not settle".into());
            } else {
                self.refuted = refuted;
                again_why = Some("read-backs");
            }
        }
        if again_why.is_none()
            && let Some(a) = pdf_answers.as_ref()
            && *a != *self.pdf.answers
        {
            if self.pdf.rounds < PDF_ROUNDS && !last {
                self.pdf.rounds += 1;
                again_why = Some("pdf");
            } else {
                errors.push(format!("! TeX capacity exceeded, sorry [pdf read-back rounds={PDF_ROUNDS}]."));
            }
        }
        if let Some(a) = pdf_answers {
            self.pdf.answers = Arc::new(a);
        }
        if again_why.is_none() && self.converge && !last && self.file_rounds < FILE_ROUNDS {
            let xetex = self.xe.is_some();
            let mut changed = false;
            for (name, text) in &writes {
                if !read_names.contains(name.as_str()) {
                    continue;
                }
                let bytes: Vec<u8> = if xetex {
                    text.as_bytes().to_vec()
                } else {
                    text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect()
                };
                let now = sh.names.given.file(name);
                if now.as_deref() != Some(&bytes[..]) {
                    Arc::make_mut(&mut self.files).insert(Arc::from(name.as_str()), Arc::from(bytes));
                    changed = true;
                }
            }
            // (the files derived from the written ones — BibTeX's `.bbl`,
            // makeindex's `.ind` —: values the next evaluation reads)
            if let Some(d) = self.derive.as_ref() {
                let made = {
                    let files = &self.files;
                    let known = |n: &str| files.contains_key(n);
                    d.lock().expect("derive").derive(&writes, &known)
                };
                for f in made {
                    self.notes.push(f.note);
                    if let Some(side) = f.side {
                        self.derived_side.retain(|x| x.0 != side.0);
                        self.derived_side.push(side);
                    }
                    let Some(bytes) = f.bytes else { continue };
                    if f.name.is_empty() {
                        continue;
                    }
                    self.derived.retain(|x| x.0 != f.name);
                    self.derived.push((f.name.clone(), bytes.clone()));
                    if self.files.get(f.name.as_str()).is_none_or(|t| **t != *bytes) {
                        Arc::make_mut(&mut self.files).insert(Arc::from(f.name.as_str()), Arc::from(bytes));
                        if read_names.contains(f.name.as_str()) {
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                self.file_rounds += 1;
                again_why = Some("files");
            }
        }
        let quiet_lean = again_why.is_none() && self.lean;
        let build_no = self.builds;
        if quiet_lean {
            for &k in &order {
                let ps = &mut st.seq.p[k as usize];
                if ps.hot_until < build_no
                    && let Some(r) = ps.run.clone()
                    && r.lean.is_none()
                {
                    ps.run = Some(Arc::new(r.leaned(&ps.text)));
                    ps.log = None;
                }
            }
        }
        for (_, m) in self.readers.iter_mut() {
            m.shrink_to_fit();
        }
        self.pieces = order
            .iter()
            .map(|&k| {
                let ps = &st.seq.p[k as usize];
                Kept { id: k, text: ps.text.clone(), run: ps.run.clone(), lines: ps.lines.clone() }
            })
            .collect();
        self.dense = dense;
        // (the pieces the evaluation ended with)
        st.stats.pieces = self.pieces.len();
        if std::env::var_os("PHITEX_STRUCT").is_some() {
            let all: Vec<String> = st.seq.labels().ids().iter().map(|&k| { let ps = &st.seq.p[k as usize]; format!("{k}:{:?}{}{}{}", ps.status, if ps.muted { "M" } else { "" }, if ps.alive { "" } else { "D" }, if ps.got.ends { "E" } else { "" }) }).collect();
            eprintln!("end of evaluation: {} live; {}", self.pieces.len(), all.join(" "));
        }
        self.pages = st.pages.into_iter().zip(bodies).collect();
        self.page_orgs = st.orgs;
        self.marks = marks;
        self.call_logs = call_logs;
        self.pdf.tail = st.pdf_tail;
        for ps in &mut st.seq.p {
            ps.prev = None;
        }
        self.seq = st.seq;
        if again_why.is_some() {
            return Eval::Again(st.stats);
        }
        st.stats.pdf_rounds = self.pdf.rounds;
        let origins = self.pages.iter().map(|(_, b)| b.pdf.as_ref().map_or_else(Vec::new, |p| p.origins.0.clone())).collect();
        Eval::Done(Box::new(Output {
            origins,
            dvi: out,
            pdf,
            stats: st.stats,
            errors,
            dump,
            log,
            writes,
            job_pages,
            files_read,
            derived: self.derived.clone(),
            derived_side: self.derived_side.clone(),
            notes: self.notes.clone(),
        }))
    }
}

fn lock(sh: &Shared) -> MutexGuard<'_, State> {
    sh.state.lock().expect("state")
}

fn worker(sh: &Shared, id: usize) {
    let mut g = lock(sh);
    let mut stall_seen: Option<u64> = None;
    loop {
        if g.done {
            return;
        }
        let Some(task) = pick(&mut g, sh) else {
            if finished(&g) {
                g.done = true;
                sh.cv.notify_all();
                return;
            }
            // (nothing to do, nothing running, the evaluation not finished:
            // nothing will ever change that — or every run in progress is
            // parked and no definition came since the last look)
            let parked = sh.parked_n.load(Ordering::Acquire);
            let gen_now = sh.generation.load(Ordering::Acquire);
            if g.inflight == 0 || (parked == g.inflight && stall_seen == Some(gen_now)) {
                stall(&mut g, sh, "the scheduler stalled");
                return;
            }
            let (g2, timeout) = sh.cv.wait_timeout(g, STALL_CHECK).expect("state");
            g = g2;
            stall_seen = (timeout.timed_out() && g.inflight > 0).then_some(gen_now);
            continue;
        };
        stall_seen = None;
        g.stats.tasks[id] += 1;
        g.inflight += 1;
        drop(g);
        let t = Instant::now();
        let (work, kind) = execute(sh, task);
        let dt = t.elapsed();
        g = lock(sh);
        g.inflight -= 1;
        g.stats.work[id] += work;
        match kind {
            0 => g.stats.time_run += dt,
            1 => g.stats.time_break += dt,
            2 => g.stats.time_scan += dt,
            4 => g.stats.time_log += dt,
            _ => g.stats.time_ship += dt,
        }
    }
}

/// The scheduler stalled, or the sequence would not settle: the
/// evaluation ends, the runs parked stop, and the build fails with why.
fn stall(g: &mut State, sh: &Shared, why: &str) {
    let mut d = format!("{why}: ");
    let live = g.seq.live_ids();
    let unsettled: Vec<String> = live
        .iter()
        .filter(|&&k| g.seq.p[k as usize].status != Status::Done)
        .take(8)
        .map(|&k| {
            let ps = &g.seq.p[k as usize];
            format!(
                "piece {k} {:?} entry {:?} dirty {:?} [{}]",
                ps.status,
                g.seq.entry_of(k),
                ps.dirty.keys().take(4).collect::<Vec<_>>(),
                ps.text.iter().take(40).collect::<String>().replace('\r', "|")
            )
        })
        .collect();
    let _ = write!(d, "{} pieces, {} running, {} parked; {}", live.len(), g.running.len(), sh.parked_n.load(Ordering::Acquire), unsettled.join("; "));
    if g.stalled.is_none() {
        g.stalled = Some(d);
    }
    g.done = true;
    for &(_, k) in &g.running {
        sh.flags.set(k, CANCEL, true);
    }
    let _p = sh.park.lock().expect("park");
    sh.generation.fetch_add(1, Ordering::AcqRel);
    sh.parked.notify_all();
    sh.cv.notify_all();
}

fn finished(g: &State) -> bool {
    g.settled.is_none()
        && g.scan_done
        && g.unshipped == 0
        && g.log_queue.is_empty()
        && g.logs_running == 0
        && g.inflight == 0
        && g.unstarted.is_empty()
        && g.running.is_empty()
}

/// Whether piece `r` read `key` only for whether it is `\outer`, and it
/// still is or still is not.
fn outer_still(g: &State, sh: &Shared, r: PieceId, key: &Key) -> bool {
    let Some(vals) = g.seq.p[r as usize].run.as_ref().and_then(|run| run.outer_reads_only(key)) else {
        return false;
    };
    vals.iter().all(|(_, node, v)| sh.names.read_holds(key, (r, *node), &sh.font, v))
}

/// `key` is dirty in `d` from node `from` on (the earliest wins).
fn dirty_from(d: &mut Dirty, key: Key, from: u32) {
    d.entry(key).and_modify(|f| *f = (*f).min(from)).or_insert(from);
}

/// Piece `k` read a definition of `key` since remade: it runs again from
/// the first of its nodes reading the name. Whether it was done.
fn mark(g: &mut State, flags: &Flags, k: PieceId, key: Key) -> bool {
    mark_from(g, flags, k, key, 0)
}

/// [`mark`], its nodes from `from` on only.
fn mark_from(g: &mut State, flags: &Flags, k: PieceId, key: Key, from: u32) -> bool {
    if !g.seq.p[k as usize].live() {
        return false;
    }
    // (the settled frontier comes back to it; a piece the scan has passed
    // run again: the scan is made again from the start)
    if g.settled.is_none_or(|s| g.seq.before(k, s)) {
        g.settled = Some(k);
    }
    if behind_scan(g, k) {
        reset_scan(g);
    }
    let ord = g.seq.ord(k);
    let ps = &mut g.seq.p[k as usize];
    dirty_from(&mut ps.dirty, key, from);
    if ps.status == Status::Done {
        ps.status = Status::Unstarted;
        g.unstarted.insert((ord, k));
        g.stats.reruns += 1;
        flags.set(k, DONE, false);
        return true;
    }
    false
}

/// Whether the scan has fed piece `k` (whole or in part): run again, the
/// scan is made again. A piece fed in part is not, when the scan itself
/// marks it (a page quantity it read is another, a routine's definition
/// reaches it: from a node after what was fed).
fn behind_scan(g: &State, k: PieceId) -> bool {
    // (the scan's own marks: from the page builder's reads and
    // definitions, made by the thread running it)
    let own = g.scan_thread == Some(std::thread::current().id());
    if g.scan_part.is_some_and(|p| p.0 == k) {
        return !own;
    }
    // (a piece a scan in flight is feeding)
    if let Some(l) = g.scan_inflight
        && !g.seq.before(l, k)
    {
        return !own;
    }
    match scan_at(g) {
        Some((s, _)) => g.seq.before(k, s),
        None => true,
    }
}

/// The piece the scan feeds next, and how many of its items it has fed
/// (none: the end).
fn scan_at(g: &State) -> Option<(PieceId, u32)> {
    if let Some(p) = g.scan_part {
        return Some(p);
    }
    match g.scan_last {
        Some(l) => g.seq.next_live(l).map(|k| (k, 0)),
        None => g.seq.first_live().map(|k| (k, 0)),
    }
}

/// The live piece after `last` (none: the first), for a cursor naming the
/// last piece it handled.
fn after_cursor(g: &State, last: Option<PieceId>) -> Option<PieceId> {
    match last {
        Some(l) => g.seq.next_live(l),
        None => g.seq.first_live(),
    }
}

/// The scheduler's sets keyed again by the labels, after a spread.
fn rekey(g: &mut State, sh: &Shared) {
    let now = g.seq.labels().spreads;
    if now == g.spreads {
        return;
    }
    g.spreads = now;
    let l = g.seq.labels.clone();
    let l = l.read().expect("labels");
    g.unstarted = g.unstarted.iter().map(|&(_, k)| (l.of(k), k)).collect();
    g.running = g.running.iter().map(|&(_, k)| (l.of(k), k)).collect();
    let at = g.settled.map_or(u64::MAX, |k| l.of(k));
    sh.settled_at.store(at, Ordering::Release);
}

/// The scan is made again from the first piece (a piece it had passed runs
/// again: rare, a loop-back's).
fn reset_scan(g: &mut State) {
    g.scan_gen += 1;
    g.scan = Some(Box::default());
    g.scan_last = None;
    g.scan_part = None;
    g.scan_inflight = None;
    g.scan_thread = None;
    g.scan_done = false;
    g.num_last = None;
    g.numbers = g.numbers0.clone();
    g.log_last = None;
    g.matching = false;
    g.pages.clear();
    g.orgs.clear();
    g.bodies.clear();
    g.ship_queue.clear();
    g.unshipped = 0;
    g.marks_before.clear();
    g.marks_after.clear();
    g.call_logs_before.clear();
    g.call_logs_after.clear();
    g.pdf_tail = Arc::default();
    g.old_pages.iter_mut().for_each(|x| *x = None);
    g.old_orgs = Arc::default();
}

/// Piece `k`'s export of `key` as the names index holds it at `(k, END)`:
/// none when a definition made between its nodes after its own last
/// definition of `key` is the one reaching its end.
fn export_of(names: &Names, font: &Arc<Font>, k: PieceId, run: &Run, key: &Key) -> Option<Val> {
    let i = run.exports.binary_search_by(|(x, _)| x.cmp(key)).ok()?;
    if names.has_inner(key) {
        let last = run.resolve(key, run.nodes_len()).map(|(d, _)| d);
        if let (Some(p), ..) = names.reaching(key, (k, END), font)
            && p.0 == k
            && p.1 != END
            && last.is_none_or(|d| p.1 >= d)
        {
            return None;
        }
    }
    Some(run.exports[i].1.clone())
}

/// Sets piece `k`'s export of `key` as [`export_of`] gives it; whether it
/// changed.
fn refresh_export(names: &Names, font: &Arc<Font>, k: PieceId, run: &Run, key: &Key) -> bool {
    let v = export_of(names, font, k, run, key);
    if names.at(key, (k, END)) == v {
        return false;
    }
    names.set((k, END), key, v.as_ref());
    true
}

/// The names whose exported values differ between two runs of a piece.
fn changed_names(old: Option<&Run>, new: &Run) -> Vec<Key> {
    let empty = Vec::new();
    let a = old.map_or(&empty, |r| &r.exports);
    let b = &new.exports;
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x.0 == y.0 => {
                if x.1 != y.1 {
                    out.push(x.0.clone());
                }
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) if x.0 < y.0 => {
                out.push(x.0.clone());
                i += 1;
            }
            (Some(x), None) => {
                out.push(x.0.clone());
                i += 1;
            }
            (_, Some(y)) => {
                out.push(y.0.clone());
                j += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// Is an edge's source (`src`, where it was defined) at or before `m`?
fn src_at_or_before(sh: &Shared, src: PieceId, m: Pos) -> bool {
    src == INITIAL || sh.pos_of(src).is_none_or(|p| !sh.before(m, p))
}

/// Place `m`'s definitions of `keys` changed: the readers after it whose
/// read of the name now gives another value than they read (exact: a
/// reader holding no read of the name, or reading the same value, is not
/// one — its edge moves to the definer reaching it now).
fn reached(g: &mut State, sh: &Shared, m: Pos, keys: &[Key]) -> Vec<(PieceId, Key)> {
    let mut out = Vec::new();
    for key in keys {
        let mut shard = sh.readers.shard(key);
        let Some(rm) = shard.get(key) else {
            continue;
        };
        let mut moves: Vec<(PieceId, PieceId)> = Vec::new();
        let mut dead: Vec<PieceId> = Vec::new();
        for (&rid, &(node, src)) in rm {
            let Some(ps) = g.seq.p.get(rid as usize) else { continue };
            if !ps.alive {
                dead.push(rid);
                continue;
            }
            if !ps.live() {
                continue;
            }
            if !sh.before(m, (rid, node)) || (m.1 != END && rid == m.0) {
                continue;
            }
            if !src_at_or_before(sh, src, m) {
                continue;
            }
            // (only a reader holding a read of the name, whose value now
            // differs from what it read)
            match &ps.run {
                Some(run) if ps.status == Status::Done || ps.status == Status::Unstarted => {
                    if run.values_read(key).is_empty() {
                        dead.push(rid);
                        continue;
                    }
                    if reads_still(&sh.names, &sh.font, run, rid, key) {
                        let (def, ..) = sh.names.reaching(key, (rid, node), &sh.font);
                        let def_id = def.map_or(INITIAL, |d| sh.id_at(d));
                        if def_id != src {
                            moves.push((rid, def_id));
                        }
                        continue;
                    }
                }
                _ => {}
            }
            out.push((rid, key.clone()));
        }
        if let Some(rm) = shard.get_mut(key) {
            for d in dead {
                rm.remove(d);
            }
        }
        for (r, src) in moves {
            move_edge_in(&mut shard, &mut g.seq.p[r as usize], key, src);
        }
    }
    out
}

/// Whether piece `r`'s `run` read `key` with the values reaching it now.
fn reads_still(names: &Names, font: &Arc<Font>, run: &Run, r: PieceId, key: &Key) -> bool {
    let vals = run.values_read(key);
    !vals.is_empty() && vals.iter().all(|(_, node, v)| names.read_holds(key, (r, *node), font, v))
}

/// Piece `k`'s dirty names as it starts to run: those its last run read
/// with values that no longer reach it (the others' edges move).
fn still_dirty(g: &mut State, sh: &Shared, k: PieceId) -> Vec<(Key, u32)> {
    let dirty = std::mem::take(&mut g.seq.p[k as usize].dirty);
    let Some(run) = g.seq.p[k as usize].run.clone() else {
        return dirty.into_iter().collect();
    };
    let any = !dirty.is_empty();
    let mut out = Vec::new();
    for (key, from) in dirty {
        if from > 0 || sh.names.has_inner(&key) || !reads_still(&sh.names, &sh.font, &run, k, &key) {
            out.push((key, from));
            continue;
        }
        let node = run.values_read(&key)[0].1;
        let (def, ..) = sh.names.reaching(&key, (k, node), &sh.font);
        let src = def.map_or(INITIAL, |d| sh.id_at(d));
        move_edge(&sh.readers, &mut g.seq.p[k as usize], &key, src);
    }
    g.seq.p[k as usize].recheck = any && out.is_empty();
    out
}

/// Piece `k`, chosen to run, whose every marked name reads its value
/// again: its run is kept if it is entered as it was. Whether it was.
fn keep_again(g: &mut State, sh: &Shared, k: PieceId) -> bool {
    let Some(run) = g.seq.p[k as usize].run.clone() else { return false };
    if entry_assumed(g, k) || !entry_holds(g, k, &run) {
        return false;
    }
    let ord = g.seq.ord(k);
    g.running.remove(&(ord, k));
    g.seq.p[k as usize].status = Status::Done;
    g.seq.p[k as usize].entered = None;
    if !lines_ready(g, k) {
        let ps = &mut g.seq.p[k as usize];
        ps.lines.resize(run.paras.len(), None);
        for (j, para) in run.paras.iter().enumerate() {
            if ps.lines[j].is_none() && !g.break_queue.iter().any(|q| q.0 == k && q.1 == j) {
                g.break_queue.push_back((k, j, para.clone()));
            }
        }
    }
    g.stats.kept += 1;
    sh.flags.set(k, DONE, true);
    settle(g, sh);
    true
}

/// Piece `ps`'s edge for `key` comes from `src` now (its reads unchanged).
fn move_edge(readers: &Readers, ps: &mut Piece, key: &Key, src: PieceId) {
    move_edge_in(&mut readers.shard(key), ps, key, src);
}

fn move_edge_in(shard: &mut FxMap<Key, ReadersOf>, ps: &mut Piece, key: &Key, src: PieceId) {
    if let Some(e) = shard.get_mut(key).and_then(|m| m.get_mut(ps.id)) {
        e.1 = src;
    }
    if let Some(i) = ps.run.as_ref().and_then(|r| r.first_read(key))
        && let Some(s) = ps.srcs.get_mut(i)
    {
        *s = src;
    }
}

/// Gives each of piece `k`'s paragraphs its lines: a paragraph equal to
/// one of `old`'s (or of the edited piece's predecessor) keeps its lines;
/// the others are queued.
fn queue_breaks(g: &mut State, k: PieceId, old: Option<&Run>, equal: &ParaCands) {
    let run = g.seq.p[k as usize].run.clone().expect("a run");
    let old_lines = std::mem::take(&mut g.seq.p[k as usize].lines);
    let prev_lines = match g.seq.p[k as usize].prev.take() {
        Some((_, l, _)) => l,
        None => old_lines,
    };
    let mut lines = vec![None; run.paras.len()];
    let mut off: isize = 0;
    for (i, para) in run.paras.iter().enumerate() {
        let hint = (i as isize + off).max(0) as usize;
        let reuse = equal.of(i, hint).find_map(|j| {
            let l = prev_lines.get(j as usize).cloned().flatten()?;
            let q = old?.paras.get(j as usize)?;
            (Arc::ptr_eq(q, para) || **q == **para).then_some((j, l))
        });
        match reuse {
            Some((j, l)) => {
                off = j as isize - i as isize;
                lines[i] = Some(l);
                g.stats.breaks_reused += 1;
            }
            None => g.break_queue.push_back((k, i, para.clone())),
        }
    }
    g.seq.p[k as usize].lines = lines;
}

fn equal_paras(old: Option<&Run>, run: &Run) -> ParaCands {
    let Some(o) = old else { return ParaCands::default() };
    let shape = |p: &crate::item::Para| (p.before.len(), p.hlist.len(), p.after.len());
    let mut index: FxMap<(usize, usize, usize), usize> = FxMap::default();
    let mut buckets: Vec<Vec<u32>> = Vec::new();
    for (j, q) in o.paras.iter().enumerate() {
        let b = *index.entry(shape(q)).or_insert_with(|| {
            buckets.push(Vec::new());
            buckets.len() - 1
        });
        buckets[b].push(j as u32);
    }
    let of = run.paras.iter().map(|p| index.get(&shape(p)).copied()).collect();
    ParaCands { olds: o.paras.len(), of, buckets }
}

/// The old paragraphs of a paragraph's lengths compared with it, nearest
/// where it is looked for first ([`ParaCands::of`]).
const NEAR_PARAS: usize = 16;

/// [`equal_paras`]: for paragraph `i`, its own place (if `old` has one),
/// then the old paragraphs in `buckets[of[i]]` (of its lengths).
#[derive(Default)]
struct ParaCands {
    olds: usize,
    of: Vec<Option<usize>>,
    buckets: Vec<Vec<u32>>,
}

impl ParaCands {
    /// Its own place, the place `hint`, then the old paragraphs of its
    /// lengths nearest `hint`, at most [`NEAR_PARAS`] of them: a document
    /// of thousands of paragraphs of one shape (a `.dtx`'s code lines) is
    /// not compared pair by pair (one not found is broken again: the same
    /// lines).
    fn of(&self, i: usize, hint: usize) -> impl Iterator<Item = u32> + '_ {
        let own = (i < self.olds).then_some(i as u32);
        let hinted = (hint != i && hint < self.olds).then_some(hint as u32);
        let b = self.of.get(i).copied().flatten().map_or(&[][..], |b| &self.buckets[b][..]);
        let p = b.partition_point(|&j| (j as usize) < hint);
        let near = (0..NEAR_PARAS)
            .flat_map(move |d| [p.checked_add(d), p.checked_sub(d + 1)])
            .filter_map(move |x| x.and_then(|x| b.get(x)).copied());
        own.into_iter()
            .chain(hinted)
            .chain(near.filter(move |&j| j as usize != i && j as usize != hint))
    }
}


/// The state piece `k`'s run leaves, if it has run.
fn out_of(seq: &Seq, k: PieceId) -> Option<RState> {
    seq.p[k as usize].run.as_ref().map(|r| r.out())
}

/// The state a piece after a cut is assumed to be entered in: the
/// canonical start, or the start of the file it is in (none: the file's
/// caller has not run).
fn assumed_entry(seq: &Seq, k: PieceId) -> Option<RState> {
    match seq.ctx_of(k) {
        None => Some(RState::start()),
        Some(c) => out_of(seq, c).map(|o| RState::entered_after(&RState::file_start(&o))),
    }
}

/// The state piece `k` is entered in now (none: a piece it is entered
/// from has not run).
fn entry_state(seq: &Seq, k: PieceId) -> Option<RState> {
    Some(match seq.entry_of(k) {
        Entry::Start => RState::start(),
        Entry::File(c) => RState::file_start(&out_of(seq, c)?),
        Entry::Rest(c) => {
            let caller = out_of(seq, c)?;
            let file = match seq.last_of(c) {
                Some(l) => Some(out_of(seq, l)?),
                None => None,
            };
            RState::rest(file.as_ref(), &caller)
        }
        // (a state no piece is entered in: the two are one piece once the
        // run before says so; the assumption stands until then)
        Entry::After(q) => match out_of(seq, q) {
            Some(o) if o.enterable() && !o.halted() => RState::entered_after(&o),
            _ => assumed_entry(seq, k)?,
        },
    })
}

/// Whether a run was entered in the state piece `k` is entered in now.
fn entry_holds(g: &State, k: PieceId, r: &Run) -> bool {
    entry_state(&g.seq, k).is_some_and(|e| r.entry() == e)
}

/// Whether piece `k` is entered on the state assumed at its cut (the
/// piece before it has not settled its run).
fn entry_assumed(g: &State, k: PieceId) -> bool {
    matches!(g.seq.entry_of(k), Entry::After(q) if !(g.seq.p[q as usize].status == Status::Done && g.seq.p[q as usize].dirty.is_empty()))
}

/// Whether the pieces piece `k` is entered from have run (installed and
/// valid; speculating: run at all).
fn entry_ready(g: &State, k: PieceId, speculate: bool) -> bool {
    let need = |c: PieceId| {
        let ps = &g.seq.p[c as usize];
        if speculate {
            ps.run.is_some()
        } else {
            ps.status == Status::Done && ps.dirty.is_empty()
        }
    };
    match g.seq.entry_of(k) {
        Entry::Start => true,
        Entry::File(c) => need(c),
        Entry::Rest(c) => need(c) && g.seq.last_of(c).is_none_or(need),
        Entry::After(q) => {
            let Some(assumed) = assumed_entry(&g.seq, k) else {
                return false;
            };
            if g.seq.p[k as usize].suspend {
                return g.seq.p[q as usize].status == Status::Done && g.seq.p[q as usize].dirty.is_empty();
            }
            match &g.seq.p[q as usize].run {
                // (the piece before reads on into this one: its text is not a
                // place a piece begins; the two are joined when it settles)
                Some(_) if g.seq.p[q as usize].got.join => false,
                Some(r) => {
                    let out = r.out();
                    !out.enterable() || out.halted() || RState::entered_after(&out) == assumed || need(q)
                }
                None => true,
            }
        }
    }
}

fn lines_ready(g: &State, k: PieceId) -> bool {
    let ps = &g.seq.p[k as usize];
    ps.run.as_ref().is_some_and(|r| ps.lines.len() == r.paras.len() && ps.lines.iter().all(Option::is_some))
}

fn rand(g: &mut State, n: usize) -> usize {
    let mut x = g.rng;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    g.rng = x;
    (x % n as u64) as usize
}

/// Whether piece `k` is before the settled frontier.
fn is_settled(g: &State, k: PieceId) -> bool {
    g.settled.is_none_or(|s| g.seq.before(k, s))
}

/// Moves the settled frontier over the pieces whose runs are final.
fn settle(g: &mut State, sh: &Shared) {
    while let Some(k) = g.settled {
        let ps = &g.seq.p[k as usize];
        if !ps.live() {
            g.settled = g.seq.next_live(k);
        } else if ps.status == Status::Done && ps.dirty.is_empty() {
            // (what its run says of the sequence holds before it is final:
            // a join with texts that came after it was run, a kept run's)
            let got = ps.got.clone();
            apply_structure(g, sh, k, &got, true);
            rekey(g, sh);
            if g.done {
                return;
            }
            if g.settled != Some(k) {
                continue;
            }
            let ps = &g.seq.p[k as usize];
            if ps.live() && ps.status == Status::Done && ps.dirty.is_empty() {
                g.settled = g.seq.next_live(k);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let at = g.settled.map_or(u64::MAX, |k| g.seq.ord(k));
    sh.settled_at.store(at, Ordering::Release);
}

/// Whether piece `k`'s material is what the last evaluation's scan was fed
/// for it.
fn same_material(g: &State, k: PieceId) -> bool {
    let Some(run) = g.seq.p[k as usize].run.as_ref() else { return false };
    g.old_runs.get(&k).is_some_and(|o| {
        Arc::ptr_eq(o, run)
            || (o.paras.len() == run.paras.len()
                && o.paras.iter().zip(&run.paras).all(|(a, b)| Arc::ptr_eq(a, b) || **a == **b)
                && o.exports == run.exports)
    })
}

/// Compares the settled pieces with what the last evaluation's scan was
/// fed, piece by piece in order, without feeding them; at the first that
/// differs the scan resumes.
fn advance_match(g: &mut State) {
    while g.matching {
        let Some((k, _)) = scan_at(g) else { break };
        if !is_settled(g, k) {
            break;
        }
        if g.old_seq.get(g.match_j) == Some(&k) && same_material(g, k) {
            g.match_j += 1;
            g.scan_last = Some(k);
            g.scan_part = None;
        } else {
            resume(g, Some(k));
        }
    }
    if g.matching && scan_at(g).is_none() && g.settled.is_none() {
        if g.match_j == g.old_seq.len() {
            // (the same material throughout: the same pages)
            for i in 0..g.old_pages.len() {
                let (pg, b) = g.old_pages[i].take().expect("an old page");
                g.pages.push(pg);
                g.bodies.push(Some(b));
                g.stats.pages_reused += 1;
            }
            g.orgs = g.old_orgs_whole.to_vec();
            if let Some(s) = g.scan.as_mut() {
                s.fancy |= g.old_fancy;
            }
            g.marks_before = std::mem::take(&mut g.old_marks);
            g.pdf_tail = g.old_pdf_tail.clone();
            g.call_logs_before = std::mem::take(&mut g.old_call_logs);
            g.scan_done = true;
            g.matching = false;
        } else {
            resume(g, None);
        }
    }
}

/// Once every piece is settled: the last piece whose material differs
/// from the last evaluation's, the two sequences compared from their ends
/// (none: the scan cannot converge on the last pages).
fn last_diff(g: &mut State) -> Option<PieceId> {
    if g.settled.is_some() || g.old_orgs.is_empty() {
        return None;
    }
    if !g.last_diff_known {
        let new = g.seq.live_ids();
        let (mut i, mut j) = (new.len(), g.old_seq.len());
        while i > 0 && j > 0 && new[i - 1] == g.old_seq[j - 1] && same_material(g, new[i - 1]) {
            i -= 1;
            j -= 1;
        }
        g.last_diff = if i == 0 { None } else { Some(new[i - 1]) };
        g.last_diff_known = true;
    }
    g.last_diff
}

/// The scan resumes before piece `c` (none: the end), the first whose
/// material differs from the last evaluation's: at the restart point of
/// the last old page whose making call and restart point are among the
/// pieces matched; the pages before it and the marks of the calls before
/// it are kept.
fn resume(g: &mut State, _c: Option<PieceId>) {
    g.matching = false;
    let mj = g.match_j;
    let old_at = |o: (u32, u32)| g.old_pos.get(&o.0).map(|&j| (j, o.1));
    let before = |o: (u32, u32)| old_at(o).is_some_and(|(j, _)| j < mj);
    let at = (1..=g.old_orgs.len()).rev().find(|&p| {
        let o = &g.old_orgs[p - 1];
        before(o.call) && o.resume.is_some_and(|(s, _)| before(s))
    });
    let Some(p) = at else {
        g.scan_last = None;
        g.scan_part = None;
        return;
    };
    let o = g.old_orgs[p - 1].clone();
    let (s, d) = o.resume.expect("a restart point");
    let ci = o.call_idx as usize;
    for i in 0..p {
        let (pg, b) = g.old_pages[i].take().expect("an old page");
        g.pages.push(pg);
        g.bodies.push(Some(b));
        g.orgs.push(g.old_orgs[i].clone());
        g.stats.pages_reused += 1;
    }
    let (kept, call_from) = if old_at(s) > old_at(o.call) { (ci + 1, None) } else { (ci, Some(g.old_marks[ci].0)) };
    g.marks_before = g.old_marks[..kept].to_vec();
    g.call_logs_before = g.old_call_logs.iter().filter(|c| (c.0 as usize) < kept).cloned().collect();
    g.scan = Some(Box::new(Scan::resumed(p as u32, d, call_from, kept as u32, o.last_call.clone(), o.call, o.fold.as_deref())));
    g.scan_last = g.seq.prev_live(s.0);
    g.scan_part = (s.1 > 0).then_some((s.0, s.1));
}

/// The fonts of the pieces the scan has fed whole numbered.
fn number_through(g: &mut State) {
    let Some(last) = g.scan_last else { return };
    while g.num_last != Some(last) {
        let Some(k) = after_cursor(g, g.num_last) else { break };
        if let Some(r) = g.seq.p[k as usize].run.clone() {
            dvi::extend(Arc::make_mut(&mut g.numbers), &r.fonts);
        }
        g.num_last = Some(k);
    }
}

/// The scheduler: a function of the state, choosing this thread's next
/// piece of work.
fn pick(g: &mut State, sh: &Shared) -> Option<Task> {
    if g.done {
        return None;
    }
    rekey(g, sh);
    settle(g, sh);
    if g.matching {
        advance_match(g);
    }
    let scan_from = scan_at(g);
    let mut scan_to = scan_from.map(|x| x.0);
    let mut fed: Vec<PieceId> = Vec::new();
    if g.scan.is_some() && !g.scan_done {
        while let Some(k) = scan_to {
            if is_settled(g, k) && lines_ready(g, k) {
                fed.push(k);
                scan_to = g.seq.next_live(k);
            } else {
                break;
            }
        }
    }
    // (a settled piece whose lines are all broken: its part of the log)
    while let Some(k) = after_cursor(g, g.log_last) {
        if !is_settled(g, k) || !lines_ready(g, k) {
            break;
        }
        let ps = &g.seq.p[k as usize];
        let run = ps.run.as_ref().expect("a settled piece has a run");
        if !ps.log.as_ref().is_some_and(|f| f.holds(run, &ps.lines)) {
            g.log_queue.push_back(k);
        }
        g.log_last = Some(k);
    }
    let scan_finish = g.scan.is_some() && !g.scan_done && scan_to.is_none() && g.settled.is_none();
    let scan_ok = g.scan.is_some() && !g.scan_done && (!fed.is_empty() || scan_finish);
    let speculate = sh.opts.reads == Reads::Speculate;
    let ready = g
        .unstarted
        .iter()
        .map(|x| x.1)
        .find(|&k| entry_ready(g, k, speculate && !g.seq.p[k as usize].suspend));
    let mut kinds = [0u8; 5];
    let mut nk = 0;
    for (ok, kind) in [
        (!g.ship_queue.is_empty(), 3u8),
        (scan_ok, 2),
        (ready.is_some(), 0),
        (!g.break_queue.is_empty(), 1),
        (!g.log_queue.is_empty(), 4),
    ] {
        if ok {
            kinds[nk] = kind;
            nk += 1;
        }
    }
    if nk == 0 {
        return None;
    }
    let kind = match sh.opts.order {
        Order::Priority => kinds[0],
        Order::Random(_) => kinds[rand(g, nk)],
    };
    match kind {
        0 => {
            let first = ready.expect("a piece ready to run");
            let k = match sh.opts.order {
                Order::Random(_) => {
                    let i = rand(g, g.unstarted.len());
                    let k = g.unstarted.iter().nth(i).expect("an unstarted piece").1;
                    if entry_ready(g, k, speculate && !g.seq.p[k as usize].suspend) { k } else { first }
                }
                Order::Priority => first,
            };
            let ord = g.seq.ord(k);
            g.unstarted.remove(&(ord, k));
            g.running.insert((ord, k));
            g.seq.p[k as usize].status = Status::Running;
            let dirty = still_dirty(g, sh, k);
            Some(Task::Run(k, dirty))
        }
        1 => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.break_queue.len()),
                Order::Priority => 0,
            };
            let (k, j, para) = g.break_queue.remove(i).expect("a queued paragraph");
            Some(Task::Break(k, j, para))
        }
        2 => {
            let pieces = fed
                .iter()
                .map(|&k| {
                    let ps = &g.seq.p[k as usize];
                    let r = ps.run.as_ref().expect("a settled piece has a run");
                    let paras = r.paras.iter().zip(&ps.lines).map(|(p, l)| (p.clone(), l.clone().expect("lines ready"))).collect();
                    (k, r.clone(), paras)
                })
                .collect();
            let predicted = g.predicted.iter().map(|x| x.0).collect();
            let end_run = g.seq.live_ids().last().and_then(|&k| g.seq.p[k as usize].run.clone().map(|r| (k, r)));
            let scan = g.scan.take().expect("the scan is free");
            let from = scan_from.map(|x| x.0).or(end_run.as_ref().map(|x| x.0)).unwrap_or(0);
            let skip = scan_from.map_or(0, |x| x.1);
            // (no convergence before a run whose page quantities the scan
            // has not validated)
            let unchecked = fed.iter().any(|&k| g.seq.p[k as usize].fold_unchecked);
            let converge = if unchecked { None } else { last_diff(g).map(|after| (after, g.old_orgs.clone())) };
            g.scan_inflight = fed.last().copied();
            g.scan_thread = Some(std::thread::current().id());
            Some(Task::Scan { scan, sgen: g.scan_gen, from, skip, converge, to: scan_to, pieces, finish: scan_finish, predicted, end_run })
        }
        4 => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.log_queue.len()),
                Order::Priority => 0,
            };
            let k = g.log_queue.remove(i).expect("a queued piece");
            g.logs_running += 1;
            let ps = &g.seq.p[k as usize];
            Some(Task::Log(k, ps.text.clone(), ps.run.clone().expect("a run"), ps.lines.clone()))
        }
        _ => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.ship_queue.len()),
                Order::Priority => 0,
            };
            let pg = g.ship_queue.remove(i).expect("a page to ship");
            Some(Task::Ship(pg, g.pages[pg].clone(), g.numbers.clone(), g.scan_gen))
        }
    }
}

/// Does a task (outside the scheduler's lock) and installs its result.
/// Returns the work done and its kind.
fn execute(sh: &Shared, task: Task) -> (u64, u8) {
    match task {
        Task::Run(k, dirty) => (run_and_install(sh, k, &dirty), 0),
        Task::Break(k, j, para) => {
            let lines = Arc::new(break_lines(&para));
            let mut g = lock(sh);
            // (the piece may have run again meanwhile: only its own paragraph)
            let ps = &g.seq.p[k as usize];
            let current = ps.alive && ps.run.as_ref().and_then(|r| r.paras.get(j)).is_some_and(|q| Arc::ptr_eq(q, &para));
            if current && j < ps.lines.len() {
                g.seq.p[k as usize].lines[j] = Some(lines);
            }
            g.stats.breaks_run += 1;
            drop(g);
            sh.cv.notify_all();
            (para.hlist.len() as u64, 1)
        }
        Task::Scan { mut scan, sgen, from, skip, converge, to, pieces, finish, predicted, end_run } => {
            let before = scan.fed;
            let mut converged = None;
            // (where the scan stopped, a fire's definitions having reached a
            // node it read: piece and item to go on from)
            let mut stop: Option<(PieceId, u32)> = None;
            let mut env = BuildFold { sh, k: from, node: 0, run: None, predicted, fired: Vec::new(), routines: 0, kept: 0 };
            let mut checked: Vec<(PieceId, Arc<Run>)> = Vec::new();
            'feed: for (d, (k, run, paras)) in pieces.iter().enumerate() {
                let k = *k;
                env.k = k;
                env.run = Some(run.clone());
                let skip = if d == 0 { skip as usize } else { 0 };
                let nodes = run.para_nodes();
                let freads = page_reads(run);
                let mut fr = 0usize;
                let mut i = 0usize;
                for (pj, pl) in paras.iter().enumerate() {
                    let node = nodes.get(pj).copied().unwrap_or(0);
                    for it in material(std::slice::from_ref(pl)) {
                        if i < skip {
                            while freads.get(fr).is_some_and(|r| r.1 <= node) {
                                fr += 1;
                            }
                            i += 1;
                            continue;
                        }
                        if !check_page_reads(sh, &scan, k, &freads, &mut fr, Some(node)) {
                            stop = Some((k, i as u32));
                            break 'feed;
                        }
                        env.node = node;
                        if env.pass((k, node)) {
                            stop = Some((k, i as u32));
                            break 'feed;
                        }
                        scan.feed(it, (k, i as u32), &mut env);
                        i += 1;
                        if scan.halt {
                            scan.halt = false;
                            stop = Some((k, i as u32));
                            break 'feed;
                        }
                        // (the same restart point with the same fold state:
                        // the pages after it are the last evaluation's)
                        if let Some((page, r, f)) = scan.just.take()
                            && let Some((after, olds)) = &converge
                            && sh.before((*after, END), (r.0.0, 0))
                            && let Some(j) = olds.iter().position(|o| o.resume == Some(r) && o.fold == f)
                        {
                            converged = Some((page as usize, j));
                            break 'feed;
                        }
                    }
                }
                if !check_page_reads(sh, &scan, k, &freads, &mut fr, None) {
                    stop = Some((k, i as u32));
                    break 'feed;
                }
                checked.push((k, run.clone()));
            }
            if finish && converged.is_none() && stop.is_none() {
                let end_org = match &end_run {
                    Some((ek, r)) => {
                        env.k = *ek;
                        env.node = r.nodes_len().saturating_sub(1) as u32;
                        env.run = Some(r.clone());
                        (*ek, u32::MAX)
                    }
                    None => (0, u32::MAX),
                };
                if env.pass_all() {
                    stop = end_run.as_ref().map(|(ek, _)| (*ek, u32::MAX));
                } else {
                    scan.finish(end_org, &mut env);
                }
            }
            let (routines, routines_kept) = (env.routines, env.kept);
            let work = scan.fed - before;
            let (new, new_orgs) = scan.take_pages();
            let tail = (finish && stop.is_none() && converged.is_none()).then(|| Arc::new(std::mem::take(&mut scan.pdf)));
            let mut g = lock(sh);
            if sgen != g.scan_gen {
                // (the scan was made again from the start meanwhile)
                drop(g);
                sh.cv.notify_all();
                return (work, 2);
            }
            g.scan_inflight = None;
            g.scan_thread = None;
            if let Some(t) = tail {
                g.pdf_tail = t;
            }
            for (k, r) in checked {
                if g.seq.p[k as usize].run.as_ref().is_some_and(|x| Arc::ptr_eq(x, &r)) {
                    g.seq.p[k as usize].fold_unchecked = false;
                }
            }
            g.stats.routines += routines;
            g.stats.routines_kept += routines_kept;
            let finish = finish && stop.is_none();
            let _ = to;
            if std::env::var_os("PHITEX_STRUCT").is_some() {
                let ids: Vec<String> = pieces.iter().map(|(k, _, _)| format!("{k}[{}]", g.seq.p[*k as usize].text.iter().take(12).collect::<String>().replace('\n', "|"))).collect();
                eprintln!("scan: from {from} skip {skip} fed {} stop {stop:?} finish {finish}", ids.join(" "));
            }
            let part = match stop {
                Some((k, i)) if i != u32::MAX => Some((k, i)),
                _ => None,
            };
            // (the pieces the scan passed drop their nodes, if lean)
            for &(k, _, _) in &pieces {
                if part.is_some_and(|p| p.0 == k) {
                    break;
                }
                g.scan_last = Some(k);
                g.scan_part = None;
                let ps = &mut g.seq.p[k as usize];
                if let Some((full, l)) = ps.lean_later.take()
                    && ps.status == Status::Done
                    && ps.run.as_ref().is_some_and(|r| Arc::ptr_eq(r, &full))
                {
                    ps.run = Some(l);
                }
            }
            if matches!(stop, Some((_, u32::MAX))) {
                g.scan_last = g.seq.live_ids().last().copied();
            }
            if let Some(p) = part {
                g.scan_last = g.seq.prev_live(p.0);
                g.scan_part = (p.1 > 0).then_some(p);
            }
            number_through(&mut g);
            for (pg, org) in new.into_iter().zip(new_orgs) {
                let i = g.pages.len();
                let same = g.old_pages.get(i).and_then(Option::as_ref).filter(|(op, _)| *op == pg).map(|(_, b)| b.clone());
                g.pages.push(pg);
                g.orgs.push(org);
                if let Some(b) = same {
                    g.bodies.push(Some(b));
                    g.stats.pages_reused += 1;
                } else {
                    g.bodies.push(None);
                    g.unshipped += 1;
                    g.ship_queue.push_back(i);
                }
            }
            for (k, r) in std::mem::take(&mut scan.late) {
                if let Some(o) = g.orgs.get_mut(k as usize) {
                    o.resume = Some(r);
                }
            }
            if let Some((k, j)) = converged {
                // (new page k and old page j have the same restart point,
                // after the last difference: the fold from there is the last
                // evaluation's, its pages and marks shifted)
                let olds = g.old_orgs.clone();
                let made = g.pages.len();
                let shift = k as i64 - j as i64;
                let dmarks = i64::from(g.orgs[k].call_idx) - i64::from(olds[j].call_idx);
                for i in (made as i64 - shift) as usize..g.old_pages.len() {
                    let Some((pg, b)) = g.old_pages[i].take() else { continue };
                    g.pages.push(pg);
                    g.bodies.push(Some(b));
                    let mut o = olds[i].clone();
                    if o.call_idx != u32::MAX {
                        o.call_idx = (i64::from(o.call_idx) + dmarks) as u32;
                    }
                    g.orgs.push(o);
                    g.stats.pages_reused += 1;
                }
                let done = (g.marks_before.len() + scan.marks.len()) as i64;
                let from = (done - dmarks) as usize;
                let sh_ = |x: u32| (i64::from(x) + shift) as u32;
                g.marks_after = g.old_marks[from.min(g.old_marks.len())..].iter().map(|&(a, b)| (sh_(a), sh_(b))).collect();
                g.call_logs_after = g
                    .old_call_logs
                    .iter()
                    .filter(|c| c.0 as usize >= from)
                    .map(|(c, l)| {
                        let l = l
                            .iter()
                            .map(|e| match e {
                                CallLog::Page(p) => CallLog::Page(sh_(*p)),
                                CallLog::Text(t) => CallLog::Text(t.clone()),
                            })
                            .collect();
                        ((i64::from(*c) + dmarks) as u32, l)
                    })
                    .collect();
                g.scan_last = g.seq.live_ids().last().copied();
                g.scan_part = None;
                g.pdf_tail = g.old_pdf_tail.clone();
                g.scan_done = true;
            }
            if finish {
                g.scan_done = true;
            }
            g.scan = Some(scan);
            drop(g);
            sh.cv.notify_all();
            for (_, r, _) in pieces {
                let _ = sh.drops.send(r);
            }
            (work, 2)
        }
        Task::Ship(i, page, numbers, sgen) => {
            let body = Arc::new(dvi::page_body_in(&page, &numbers, &[], &[], 0, sh.names.given.limits.dvi_buf_size));
            let mut g = lock(sh);
            if sgen == g.scan_gen && i < g.bodies.len() {
                if body.fixed.is_some() {
                    g.stats.bodies_fixed += 1;
                }
                g.bodies[i] = Some(body);
                g.unshipped -= 1;
                g.stats.pages_shipped += 1;
                if let Some(b) = &sh.board {
                    b.pages.fetch_add(1, Ordering::Relaxed);
                }
            }
            drop(g);
            sh.cv.notify_all();
            (page.b.list.len() as u64, 3)
        }
        Task::Log(k, text, run, lines) => {
            let f = Arc::new(logs::fragment(&text, &run, &lines));
            let work = f.entries.len() as u64;
            let mut g = lock(sh);
            if g.seq.p[k as usize].alive {
                g.seq.p[k as usize].log = Some(f);
            }
            g.logs_running -= 1;
            drop(g);
            sh.cv.notify_all();
            (work, 4)
        }
    }
}

/// Pieces leave the evaluation (taken out for good, or muted): their runs
/// in progress stop, their definitions leave the names index (their
/// readers marked), their edges go when taken out; a cursor on one moves
/// to `replacement`.
fn leave(g: &mut State, sh: &Shared, gone: &[PieceId], replacement: Option<PieceId>, out_for_good: bool) {
    let mut touched: Vec<(PieceId, Vec<Key>)> = Vec::new();
    for &x in gone {
        let ord = g.seq.ord(x);
        if g.running.remove(&(ord, x)) {
            sh.flags.set(x, CANCEL, true);
        }
        g.unstarted.remove(&(ord, x));
        sh.flags.set(x, DONE, false);
        let mut keys: Vec<Key> = Vec::new();
        if let Some(r) = g.seq.p[x as usize].run.clone() {
            keys.extend(sh.names.drop_piece(x, &r.exports));
            if out_for_good {
                for i in 0..r.read_count() {
                    let key = r.read_at(i).0;
                    if let Some(m) = sh.readers.shard(key).get_mut(key) {
                        m.remove(x);
                    }
                }
            }
        } else if let Some((r, _, _)) = g.seq.p[x as usize].prev.clone() {
            keys.extend(sh.names.drop_piece(x, &r.exports));
        }
        // (the page builder's definitions between its nodes)
        let mut i = 0;
        while i < g.fold_defs.len() {
            if g.fold_defs[i].0 == x {
                let (_, node, defs) = g.fold_defs.remove(i);
                for (key, _) in defs {
                    sh.names.set((x, node), &key, None);
                    keys.push(key);
                }
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < g.predicted.len() {
            if g.predicted[i].0.0 == x {
                let (pos, ks) = g.predicted.remove(i);
                for key in ks {
                    sh.names.set(pos, &key, None);
                    keys.push(key);
                }
            } else {
                i += 1;
            }
        }
        if out_for_good {
            let ps = &mut g.seq.p[x as usize];
            ps.run = None;
            ps.lines.clear();
            ps.log = None;
            ps.lean_later = None;
            ps.prev = None;
            ps.dirty.clear();
        }
        if g.settled == Some(x) {
            // (back to the first piece: `settle` walks on to the frontier)
            g.settled = replacement.or_else(|| g.seq.first_live());
        }
        let gone_cursor = |c: Option<PieceId>| c == Some(x);
        if gone_cursor(g.scan_last) || gone_cursor(g.num_last) || gone_cursor(g.scan_part.map(|p| p.0)) {
            reset_scan(g);
        }
        if gone_cursor(g.log_last) {
            g.log_last = None;
        }
        g.log_queue.retain(|&q| q != x);
        keys.sort();
        keys.dedup();
        if !keys.is_empty() {
            touched.push((x, keys));
        }
    }
    for (x, keys) in touched {
        changed_at(g, sh, (x, END), keys);
    }
}

/// Pieces enter the evaluation (made, or unmuted): to run, their last run
/// (if any) standing as the prediction of what they define.
fn enter(g: &mut State, sh: &Shared, new: &[PieceId]) {
    g.seq.set_bases();
    let mut changed: Vec<(PieceId, Vec<Key>)> = Vec::new();
    {
        let mut b = sh.names.given.bases.write().expect("bases");
        for &x in new {
            b.insert(x, g.seq.p[x as usize].base);
        }
    }
    for &x in new {
        let ps = &mut g.seq.p[x as usize];
        if !ps.live() {
            continue;
        }
        ps.status = Status::Unstarted;
        sh.flags.set(x, DONE, false);
        sh.flags.set(x, CANCEL, false);
        let pred = ps.run.clone().or(ps.prev.as_ref().map(|p| p.0.clone()));
        let ord = g.seq.ord(x);
        g.unstarted.insert((ord, x));
        if let Some(r) = pred {
            let mut keys = Vec::new();
            for (key, val) in &r.exports {
                sh.names.set((x, END), key, Some(val));
                keys.push(key.clone());
            }
            changed.push((x, keys));
        }
        if g.settled.is_none_or(|s| g.seq.before(x, s)) {
            g.settled = Some(x);
        }
    }
    for (x, keys) in changed {
        changed_at(g, sh, (x, END), keys);
    }
}

/// The live piece after `k` and what is under it: entered in the state
/// that leaves; run again if its run was entered otherwise.
fn recheck_after(g: &mut State, sh: &Shared, k: PieceId) {
    let end = g.seq.extent_end(k);
    let Some(q) = g.seq.next_live(end) else { return };
    let ps = &g.seq.p[q as usize];
    if ps.status == Status::Done
        && let Some(r) = ps.run.clone()
        && !entry_holds(g, q, &r)
    {
        mark_entry(g, sh, q);
    }
    if g.seq.p[q as usize].status == Status::Running
        && g.seq.p[q as usize].entered.as_ref().is_some_and(|e| entry_state(&g.seq, q).is_some_and(|now| now != *e))
    {
        sh.flags.set(q, CANCEL, true);
    }
}

/// Piece `q`'s run was entered otherwise than it is now: it runs again.
fn mark_entry(g: &mut State, sh: &Shared, q: PieceId) {
    let ord = g.seq.ord(q);
    g.seq.p[q as usize].status = Status::Unstarted;
    g.unstarted.insert((ord, q));
    g.stats.reruns += 1;
    sh.flags.set(q, DONE, false);
    if g.settled.is_none_or(|s| g.seq.before(q, s)) {
        g.settled = Some(q);
    }
    if behind_scan(g, q) {
        reset_scan(g);
    }
}

/// Piece `k`'s run, installed and valid, says what it says of the sequence:
/// where that differs from the sequence, the sequence is rewritten there
/// (CORE.md, "Reduction": local rewrites). `was` is what its last run said.
///
/// A join (and a caller made to read on, `inline`/`nocut`) cannot be
/// undone: it waits for the run to be final (`final_`: at the settled
/// frontier). The rest (a file's pieces, a re-cut, what is muted) is
/// undone by the run that says otherwise, and is applied at once.
fn apply_structure(g: &mut State, sh: &Shared, k: PieceId, was: &Expect, final_: bool) {
    let got = g.seq.p[k as usize].got.clone();
    let count = |g: &mut State, sh: &Shared, what: &str| -> bool {
        let ps = &mut g.seq.p[k as usize];
        ps.edits += 1;
        if std::env::var_os("PHITEX_STRUCT").is_some() {
            eprintln!("structure: piece {k} {what} (rewrite {}) got {:?} [{}]", ps.edits, (got.join, got.cut, got.stop, got.input.is_some(), got.ends, got.endinput), ps.text.iter().take(30).collect::<String>().replace('\n', "|"));
        }
        if ps.edits > EDIT_LIMIT {
            let why = format!(
                "the sequence of pieces did not settle: piece {k}'s run rewrote it {EDIT_LIMIT} times [{}]",
                ps.text.iter().take(50).collect::<String>().replace('\r', "|")
            );
            stall(g, sh, &why);
            return false;
        }
        true
    };
    // (a run reading on past its text: it and the texts after it in its
    // list are one piece)
    if got.join && final_ {
        let sibs = g.seq.join_siblings(k);
        if !sibs.is_empty() {
            if !count(g, sh, "join") {
                return;
            }
            let prev: Prev = {
                let ps = &g.seq.p[k as usize];
                (ps.run.clone().expect("a run"), ps.lines.clone(), ps.text.clone())
            };
            let (j, removed) = g.seq.join(k, &sibs);
            rekey(g, sh);
            g.seq.p[j as usize].prev = Some(prev);
            leave(g, sh, &removed, Some(j), true);
            enter(g, sh, &[j]);
            recheck_after(g, sh, j);
            return;
        }
        // (its list's last text: a file's, read in its caller; a re-cut's,
        // whose caller does not stop at the cut)
        let l = g.seq.p[k as usize].list;
        let (kind, caller) = {
            let li = &g.seq.lists[l as usize];
            (li.kind.clone(), li.caller)
        };
        if let Some(c) = caller
            && g.seq.p[c as usize].live()
        {
            if !count(g, sh, "inline/nocut") {
                return;
            }
            match kind {
                ListKind::File(_) => g.seq.p[c as usize].inline = true,
                _ => g.seq.p[c as usize].nocut = true,
            }
            let kids = remove_kids(g, c);
            leave(g, sh, &kids, None, true);
            mark_entry(g, sh, c);
            recheck_after(g, sh, c);
            return;
        }
    }
    let mut changed = false;
    // (a cut the cutter missed: the rest of the text cut again after it)
    let recut_at = g.seq.p[k as usize].recut.map(|l| g.seq.lists[l as usize].stop);
    let want_cut = got.cut.then_some(got.stop);
    if recut_at != want_cut {
        if !count(g, sh, "recut") {
            return;
        }
        let kids = remove_kids(g, k);
        leave(g, sh, &kids, None, true);
        if let Some(stop) = want_cut {
            let new = g.seq.make_recut(k, stop);
            rekey(g, sh);
            enter(g, sh, &new);
        }
        changed = true;
    }
    // (an `\input`: the file's pieces and the rest of the line after it)
    let want = if g.seq.p[k as usize].inline { None } else { got.input.clone() };
    let have = g.seq.p[k as usize].file.map(|l| {
        let li = &g.seq.lists[l as usize];
        (li.kind.clone(), li.stop)
    });
    let same = match (&want, &have) {
        (None, None) => true,
        (Some(f), Some((ListKind::File(h), stop))) => (Arc::ptr_eq(f, h) || **f == **h) && *stop == got.stop,
        _ => false,
    };
    if !same && !(want.is_none() && have.is_none()) {
        if !count(g, sh, "input") {
            return;
        }
        if have.is_some() {
            let kids = remove_kids(g, k);
            leave(g, sh, &kids, None, true);
        }
        if let Some(f) = want {
            if g.seq.depth(k) > 64 {
                g.seq.p[k as usize].inline = true;
                mark_entry(g, sh, k);
            } else {
                let new = g.seq.make_file(k, f, got.stop);
                rekey(g, sh);
                enter(g, sh, &new);
            }
        }
        changed = true;
    }
    // (the job's end, a file's end: what follows is muted, or back)
    let _ = was;
    if (got.ends, got.endinput) != g.seq.p[k as usize].muting || changed {
        g.seq.p[k as usize].muting = (got.ends, got.endinput);
        let (muted, unmuted) = g.seq.recompute_mutes();
        if std::env::var_os("PHITEX_STRUCT").is_some() {
            eprintln!("mutes: after piece {k} (ends {} was {}, endinput {} was {}): muted {muted:?} unmuted {unmuted:?}", got.ends, was.ends, got.endinput, was.endinput);
        }
        if !muted.is_empty() {
            leave(g, sh, &muted, None, false);
        }
        if !unmuted.is_empty() {
            for &x in &unmuted {
                let ps = &mut g.seq.p[x as usize];
                if ps.run.is_some() && ps.prev.is_none() {
                    ps.prev = ps.run.as_ref().map(|r| (r.clone(), ps.lines.clone(), ps.text.clone()));
                }
            }
            enter(g, sh, &unmuted);
        }
        if !muted.is_empty() || !unmuted.is_empty() {
            // (the entries after what changed)
            for q in g.seq.live_ids() {
                let ps = &g.seq.p[q as usize];
                if ps.status == Status::Done
                    && let Some(r) = ps.run.clone()
                    && !entry_holds(g, q, &r)
                {
                    mark_entry(g, sh, q);
                }
            }
        }
    }
    if changed {
        recheck_after(g, sh, k);
    }
}

/// Takes out what is under piece `k` (its file's pieces and rest, its
/// re-cut's pieces): those taken out.
fn remove_kids(g: &mut State, k: PieceId) -> Vec<PieceId> {
    let end = g.seq.extent_end(k);
    if end == k {
        let ps = &mut g.seq.p[k as usize];
        for l in [ps.file.take(), ps.recut.take()].into_iter().flatten() {
            g.seq.lists[l as usize].alive = false;
        }
        return Vec::new();
    }
    let first = g.seq.labels().next(k).expect("a piece under it");
    // (taken out one top-level piece at a time, each with what is under it)
    let mut out = Vec::new();
    let mut x = Some(first);
    let end_ord = g.seq.ord(end);
    while let Some(j) = x {
        if g.seq.ord(j) > end_ord {
            break;
        }
        let next_after = {
            let e = g.seq.extent_end(j);
            g.seq.labels().next(e)
        };
        out.extend(g.seq.remove_extent(j));
        x = next_after;
    }
    let ps = &mut g.seq.p[k as usize];
    for l in [ps.file.take(), ps.recut.take()].into_iter().flatten() {
        g.seq.lists[l as usize].alive = false;
    }
    out
}

/// Runs piece `k` (marked running by whoever chose it) from its dirty
/// nodes and installs the run: its exports into the names index, its reads
/// as edges, the readers its changed definitions reach marked; its own
/// reads validated against the index as it is now; what it says of the
/// sequence made so.
fn run_and_install(sh: &Shared, k: PieceId, dirty: &[(Key, u32)]) -> u64 {
    let (text, old, entry, strict, entry_assumed, old_srcs, follows, inline, nocut, stops) = {
        let mut g = lock(sh);
        let ord = g.seq.ord(k);
        if !g.seq.p[k as usize].live() {
            g.running.remove(&(ord, k));
            drop(g);
            notify(sh, true);
            return 0;
        }
        if std::mem::take(&mut g.seq.p[k as usize].recheck)
            && dirty.is_empty()
            && g.seq.p[k as usize].dirty.is_empty()
            && keep_again(&mut g, sh, k)
        {
            drop(g);
            notify(sh, true);
            return 0;
        }
        g.seq.p[k as usize].entered = entry_state(&g.seq, k);
        let assumed = entry_assumed(&g, k);
        let follows = g.seq.next_live(k).is_some();
        let ps = &g.seq.p[k as usize];
        let old = match (&ps.run, &ps.prev) {
            (Some(r), _) => Some((r.clone(), ps.text.clone())),
            (None, Some((r, _, t))) => Some((r.clone(), t.clone())),
            (None, None) => None,
        };
        (ps.text.clone(), old, ps.entered.clone(), ps.suspend, assumed, ps.srcs.clone(), follows, ps.inline, ps.nocut, ps.stops.clone())
    };
    sh.flags.set(k, CANCEL, false);
    let mut env = PieceEnv {
        sh,
        k,
        spec: sh.opts.reads == Reads::Speculate && !strict,
        strict,
        predicted: Vec::new(),
        checked: sh.names_gen.load(Ordering::Acquire),
        gave_up: false,
        entry_assumed,
        seen: FxMap::default(),
        fold_read: false,
        follows,
        inline,
        nocut,
        stops,
    };
    let run = Arc::new(run_piece(&text, &mut env, old.as_ref().map(|(r, t)| (&**r, &t[..])), dirty, entry.as_ref()));
    if run.abandoned {
        let mut g = lock(sh);
        g.stats.abandoned += 1;
        g.stats.commands_run += run.ran;
        let ord = g.seq.ord(k);
        g.running.remove(&(ord, k));
        if g.seq.p[k as usize].live() {
            g.seq.p[k as usize].suspend |= env.gave_up;
            for (key, from) in dirty {
                dirty_from(&mut g.seq.p[k as usize].dirty, key.clone(), *from);
            }
            g.seq.p[k as usize].status = Status::Unstarted;
            g.unstarted.insert((ord, k));
        }
        drop(g);
        notify(sh, true);
        return run.ran;
    }
    let steps = run.ran;
    let t = Instant::now();
    let got = Expect::of(&run);
    let hot = sh.build_no > 1;
    let lean = (sh.lean && !hot).then(|| Arc::new(run.leaned(&text)));
    let old_run = old.map(|(r, _)| r);
    // (before the lock: each read validated against the names index as it
    // is now, noting how many changes to the index that has seen; the names
    // whose values this run changes; the paragraphs it kept)
    let seen = sh.names_gen.load(Ordering::Acquire);
    let mut checks: Vec<(Option<Pos>, bool)> = run
        .reads
        .iter()
        .map(|(key, node, val)| {
            let (def, now) = sh.names.read_now(key, (k, *node), &sh.font);
            (def, val.seen_as(&now))
        })
        .collect();
    let changed = changed_names(old_run.as_deref(), &run);
    let equal = equal_paras(old_run.as_deref(), &run);
    let same_keys = old_run.as_ref().is_some_and(|o| {
        o.read_count() == run.reads.len() && run.reads.iter().enumerate().all(|(i, b)| *o.read_at(i).0 == b.0)
    });
    let read_keys: FxSet<&Key> = if old_run.is_some() && !same_keys {
        run.reads.iter().map(|(key, _, _)| key).collect()
    } else {
        FxSet::default()
    };
    let mut srcs: Vec<PieceId> = checks.iter().map(|&(def, _)| def.map_or(INITIAL, |d| sh.id_at(d))).collect();
    // (whether read `i` holds its name's edge: its first reader; the page
    // builder's quantities have none, the scan validates each read)
    let holds_edge = |i: usize| {
        let key = &run.reads[i].0;
        !((i > 0 && run.reads[i - 1].0 == *key) || matches!(key, Key::Page(_)))
    };
    {
        let mut ins: Vec<(usize, usize)> = (0..run.reads.len())
            .filter(|&i| holds_edge(i))
            .filter(|&i| {
                !(same_keys
                    && old_srcs.get(i) == Some(&srcs[i])
                    && old_run.as_ref().is_some_and(|o| o.read_at(i).1 == run.reads[i].1))
            })
            .map(|i| (Names::route(&run.reads[i].0).0, i))
            .collect();
        ins.sort_unstable();
        let mut gone: Vec<(usize, &Key)> = Vec::new();
        if let Some(o) = old_run.as_ref().filter(|_| !same_keys) {
            for i in 0..o.read_count() {
                let key = o.read_at(i).0;
                if !read_keys.contains(key) {
                    gone.push((Names::route(key).0, key));
                }
            }
        }
        gone.sort_unstable_by_key(|x| x.0);
        let (mut a, mut b) = (0, 0);
        while a < ins.len() || b < gone.len() {
            let sh_i = ins.get(a).map_or(usize::MAX, |x| x.0).min(gone.get(b).map_or(usize::MAX, |x| x.0));
            let mut m = sh.readers.0[sh_i].lock().expect("readers");
            while let Some(&(_, i)) = ins.get(a).filter(|x| x.0 == sh_i) {
                let (key, node, _) = &run.reads[i];
                Readers::insert(&mut m, key, k, (*node, srcs[i]));
                a += 1;
            }
            while let Some(&(_, key)) = gone.get(b).filter(|x| x.0 == sh_i) {
                if let Some(r) = m.get_mut(key) {
                    r.remove(k);
                }
                b += 1;
            }
        }
    }
    {
        let mut g = lock(sh);
        let held = Instant::now();
        g.stats.commands_run += run.ran;
        let ord = g.seq.ord(k);
        g.running.remove(&(ord, k));
        if !g.seq.p[k as usize].live() {
            // (taken out or muted while it ran: no result)
            drop(g);
            notify(sh, true);
            return steps;
        }
        // (names changed since the reads were validated: those reads again)
        let now_gen = sh.names_gen.load(Ordering::Acquire);
        if now_gen != seen {
            let since: FxSet<&Key> = g.names_log[seen.min(g.names_log.len())..now_gen.min(g.names_log.len())].iter().flatten().collect();
            for (i, ((key, node, val), c)) in run.reads.iter().zip(&mut checks).enumerate() {
                if since.contains(key) {
                    let (def, now) = sh.names.read_now(key, (k, *node), &sh.font);
                    *c = (def, val.seen_as(&now));
                    let src = def.map_or(INITIAL, |d| sh.id_at(d));
                    if src != srcs[i] {
                        srcs[i] = src;
                        if holds_edge(i) {
                            Readers::insert(&mut sh.readers.shard(key), key, k, (*node, src));
                        }
                    }
                }
            }
        }
        let mut stale = Dirty::new();
        for ((key, _, _), &(_, same)) in run.reads.iter().zip(&checks) {
            if !same {
                dirty_from(&mut stale, key.clone(), 0);
            }
        }
        g.seq.p[k as usize].srcs = srcs;
        let mut changed = changed;
        for key in &changed {
            refresh_export(&sh.names, &sh.font, k, &run, key);
        }
        for (key, _) in &run.exports {
            if sh.names.has_inner(key) && !changed.contains(key) && refresh_export(&sh.names, &sh.font, k, &run, key) {
                changed.push(key.clone());
            }
        }
        if !changed.is_empty() {
            g.names_log.push(changed.clone());
        }
        sh.names_gen.store(g.names_log.len(), Ordering::Release);
        let was = std::mem::take(&mut g.seq.p[k as usize].got);
        g.seq.p[k as usize].run = Some(run.clone());
        g.seq.p[k as usize].got = got;
        g.seq.p[k as usize].fold_unchecked = env.fold_read || run.reads.iter().any(|r| matches!(r.0, Key::Page(_)));
        let late = std::mem::take(&mut g.seq.p[k as usize].dirty);
        for (key, from) in late {
            dirty_from(&mut stale, key, from);
        }
        g.seq.p[k as usize].runs += 1;
        g.stats.ran += 1;
        if g.seq.p[k as usize].runs == RUN_LIMIT {
            let why = format!(
                "piece {k} ran {RUN_LIMIT} times (a livelock): read again {:?}, its definitions that changed {:?}",
                stale.keys().take(8).collect::<Vec<_>>(),
                changed.iter().take(8).collect::<Vec<_>>()
            );
            stall(&mut g, sh, &why);
        }
        let marks = reached(&mut g, sh, (k, END), &changed);
        for (r, key) in marks {
            if outer_still(&g, sh, r, &key) {
                continue;
            }
            if !mark(&mut g, &sh.flags, r, key) && g.seq.p[r as usize].status == Status::Running {
                sh.flags.set(r, CANCEL, true);
            }
        }
        queue_breaks(&mut g, k, old_run.as_deref(), &equal);
        let entered = entry_holds(&g, k, &run);
        if stale.is_empty() && entered {
            g.seq.p[k as usize].lean_later = lean.map(|l| (run.clone(), l));
            if hot {
                g.seq.p[k as usize].hot_until = sh.build_no + HOT_BUILDS;
            }
            g.seq.p[k as usize].status = Status::Done;
            sh.flags.set(k, DONE, true);
            if let Some(b) = &sh.board {
                b.settled.fetch_add(1, Ordering::Relaxed);
            }
            // (what the run says of the sequence: the sequence rewritten
            // where it says otherwise)
            apply_structure(&mut g, sh, k, &was, false);
        } else {
            g.seq.p[k as usize].dirty = stale;
            g.seq.p[k as usize].status = Status::Unstarted;
            g.unstarted.insert((ord, k));
            g.stats.reruns += 1;
        }
        // (the pieces entered in the state this one leaves)
        if g.seq.p[k as usize].alive {
            for q in g.seq.dependents(k) {
                if g.seq.p[q as usize].status == Status::Running
                    && g.seq.p[q as usize].entered.as_ref().is_some_and(|e| entry_state(&g.seq, q).is_some_and(|now| now != *e))
                {
                    sh.flags.set(q, CANCEL, true);
                }
                let stale_entry = g.seq.p[q as usize].status == Status::Done
                    && g.seq.p[q as usize].run.as_ref().is_some_and(|r| !entry_holds(&g, q, r));
                if stale_entry {
                    mark_entry(&mut g, sh, q);
                }
            }
        }
        settle(&mut g, sh);
        g.stats.time_confirm += t.elapsed();
        g.stats.time_held += held.elapsed();
    }
    notify(sh, true);
    if let Some(o) = old_run {
        let _ = sh.drops.send(o);
    }
    steps
}

/// A piece's reads of names from before it.
struct PieceEnv<'a> {
    sh: &'a Shared,
    k: PieceId,
    /// Reads run on predictions (`Reads::Speculate`, and not a piece whose
    /// speculation was given up).
    spec: bool,
    /// Every read waits for the pieces before it that may define its name.
    strict: bool,
    /// The reads made on predictions (name, node, value).
    predicted: Vec<(Key, u32, Val)>,
    /// The names index's change count when the predictions were last
    /// checked.
    checked: usize,
    /// The run was abandoned on predictions nothing confirmed.
    gave_up: bool,
    /// It was entered on the state assumed at its cut.
    entry_assumed: bool,
    /// Every read from before the piece so far.
    seen: FxMap<Key, Vec<(u32, Val)>>,
    /// A read found a definition the page builder made between a piece's
    /// nodes, not confirmed by this evaluation's scan yet.
    fold_read: bool,
    /// A piece follows it (a run reading past its text mid-command ends).
    follows: bool,
    /// Its `\input`s are read in it.
    inline: bool,
    /// It does not end at a cut the cutter missed.
    nocut: bool,
    /// Where the texts it was joined from begin.
    stops: Arc<[usize]>,
}

impl PieceEnv<'_> {
    /// Whether a read this run made is refuted: the names index now gives
    /// another value there.
    fn refuted(&self, from: usize, to: usize) -> bool {
        let sh = self.sh;
        let keys: Vec<Key> = {
            let g = lock(sh);
            let to = to.min(g.names_log.len());
            let mut v: Vec<Key> = g.names_log[from.min(to)..to].iter().flatten().filter(|k| self.seen.contains_key(*k)).cloned().collect();
            v.sort();
            v.dedup();
            v
        };
        keys.iter().any(|key| self.seen[key].iter().any(|(node, val)| sh.names.reaching(key, (self.k, *node), &sh.font).1 != *val))
    }

    fn reach_now(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let sh = self.sh;
        loop {
            let seen = sh.generation.load(Ordering::Acquire);
            let (def, val, lo, hi) = sh.names.reaching(key, (self.k, node), &sh.font);
            if sh.flags.get(self.k, CANCEL) {
                return (val, Src::Initial, lo, hi);
            }
            let Some(d) = def else {
                // (an undefined name while an earlier piece may still define
                // it: wait for the earlier pieces, or run the first of them
                // nobody has started)
                if self.spec || !(self.strict || matches!(val, Val::Meaning(Meaning::Undefined))) {
                    self.predicted.push((key.clone(), node, val.clone()));
                    return (val, Src::Initial, lo, hi);
                }
                let me = sh.ord(self.k);
                let leapfrog = {
                    let mut g = lock(sh);
                    let ready = g.unstarted.range(..(me, 0)).map(|x| x.1).find(|&e| entry_ready(&g, e, false));
                    if let Some(e) = ready {
                        let eo = g.seq.ord(e);
                        g.unstarted.remove(&(eo, e));
                        g.running.insert((eo, e));
                        g.seq.p[e as usize].status = Status::Running;
                        g.stats.parks += 1;
                        g.stats.leapfrogs += 1;
                        Some((e, still_dirty(&mut g, sh, e)))
                    } else if g.running.first().is_some_and(|x| x.0 < me) || g.unstarted.first().is_some_and(|x| x.0 < me) {
                        g.stats.parks += 1;
                        None
                    } else {
                        return (val, Src::Initial, lo, hi);
                    }
                };
                if let Some((e, dirty)) = leapfrog {
                    run_and_install(sh, e, &dirty);
                } else {
                    park(sh, seen);
                }
                continue;
            };
            if d.1 != END {
                self.fold_read = true;
            }
            let src = Src::Piece(sh.id_at(d));
            let dk = d.0;
            if dk == self.k || sh.flags.get(dk, DONE) {
                return (val, src, lo, hi);
            }
            if self.spec {
                self.predicted.push((key.clone(), node, val.clone()));
                return (val, src, lo, hi);
            }
            // (the definition is being remade: park until it is installed,
            // or make it here if nobody has started)
            let leapfrog = {
                let mut g = lock(sh);
                match g.seq.p[dk as usize].status {
                    Status::Done => return (val, src, lo, hi),
                    _ if !g.seq.p[dk as usize].live() => return (val, src, lo, hi),
                    Status::Unstarted => {
                        let e = if entry_ready(&g, dk, false) {
                            Some(dk)
                        } else {
                            let dord = g.seq.ord(dk);
                            g.unstarted.range(..(dord, 0)).map(|x| x.1).find(|&e| entry_ready(&g, e, false))
                        };
                        g.stats.parks += 1;
                        e.map(|e| {
                            let eo = g.seq.ord(e);
                            g.unstarted.remove(&(eo, e));
                            g.running.insert((eo, e));
                            g.seq.p[e as usize].status = Status::Running;
                            g.stats.leapfrogs += 1;
                            (e, still_dirty(&mut g, sh, e))
                        })
                    }
                    Status::Running => {
                        g.stats.parks += 1;
                        None
                    }
                }
            };
            if let Some((e, dirty)) = leapfrog {
                run_and_install(sh, e, &dirty);
            } else {
                park(sh, seen);
            }
        }
    }

    /// Whether a piece before this one has not settled its run.
    fn unconfirmed(&self) -> bool {
        self.sh.settled_at.load(Ordering::Acquire) < self.sh.ord(self.k)
    }
}

impl Env for PieceEnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.sh.names.cs
    }

    fn folds(&mut self, key: &Key) -> bool {
        matches!(key, Key::Page(_))
    }

    fn fold_read(&mut self, key: &Key, node: u32) -> Option<Val> {
        self.sh.names.fold_value(key, (self.k, node), &self.sh.font)
    }

    fn limits(&self) -> crate::print::Limits {
        self.sh.names.given.limits
    }

    fn interaction_option(&self) -> Option<i32> {
        self.sh.names.given.interaction_option
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.sh.fonts.get(name)
    }

    fn xetex(&self) -> bool {
        self.sh.names.given.xe.is_some()
    }

    fn native_font(&mut self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        self.sh.names.given.native_font(name, size, tracing)
    }

    fn main_memory(&self) -> u64 {
        self.sh.opts.main_memory
    }

    fn check(&mut self, ran: u64, words: u64) -> Check {
        if self.sh.flags.get(self.k, CANCEL) {
            return Check::Abandon;
        }
        let cap = self.sh.opts.main_memory;
        let periodic = ran.is_multiple_of(CHECK_EVERY);
        if !(periodic || words > cap) {
            return Check::Go;
        }
        let now = self.sh.names_gen.load(Ordering::Acquire);
        if now != self.checked {
            let from = self.checked;
            self.checked = now;
            if self.refuted(from, now) {
                return Check::Abandon;
            }
        }
        if (!self.predicted.is_empty() || self.entry_assumed) && (ran >= GIVE_UP_AFTER || words > cap) && self.unconfirmed() {
            self.gave_up = true;
            return Check::Abandon;
        }
        if words > cap { Check::Exceeded(cap) } else { Check::Go }
    }

    fn has_between(&mut self, key: &Key) -> bool {
        self.sh.names.has_inner(key)
    }

    fn crossed_ends(&mut self) -> bool {
        self.follows
    }

    fn cut_ok(&mut self) -> bool {
        !self.nocut
    }

    fn split_inputs(&mut self) -> bool {
        !self.inline
    }

    fn give_up_deep(&mut self) -> bool {
        if (!self.predicted.is_empty() || self.entry_assumed) && self.unconfirmed() {
            self.gave_up = true;
            return true;
        }
        false
    }

    fn stop_at(&mut self, pos: usize) -> bool {
        // (a piece whose re-cut's last text read on into the text after
        // it: it reads its whole text, no stop)
        !self.nocut && self.stops.binary_search(&pos).is_ok()
    }

    fn any_between(&mut self) -> bool {
        self.sh.names.any_inner()
    }

    fn between(&mut self, key: &Key, after: u32, node: u32) -> Option<(Val, Src)> {
        let sh = self.sh;
        if !sh.names.has_inner(key) {
            return None;
        }
        match sh.names.reaching(key, (self.k, node), &sh.font) {
            (Some(p), val, ..) if p.0 == self.k && p.1 != END && p.1 >= after => Some((val, Src::Piece(sh.id_at(p)))),
            _ => None,
        }
    }

    fn file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.sh.names.given.file(name)
    }

    fn file_path(&mut self, name: &str) -> Arc<str> {
        self.sh.names.given.path(name)
    }

    fn jobname(&mut self) -> Arc<str> {
        self.sh.names.given.job.clone()
    }

    fn line_base(&mut self) -> i32 {
        self.sh.names.given.base(self.k) as i32
    }

    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let r = self.reach_now(key, node);
        self.seen.entry(key.clone()).or_default().push((node, r.0.clone()));
        r
    }
}

/// What the page builder reads where it is called — node `node` of piece
/// `k` (`run`): the piece's own definitions up to that node, else the
/// names index before the place (a fire at the same place included).
fn read_at(sh: &Shared, run: Option<&Run>, k: PieceId, node: u32, key: &Key) -> Val {
    read_at_with(sh, run, k, node, key, true)
}

/// [`read_at`], the definitions made between the piece's nodes at the
/// node itself (where the page builder fired) seen or not.
fn read_at_with(sh: &Shared, run: Option<&Run>, k: PieceId, node: u32, key: &Key, at_node: bool) -> Val {
    let local = run.and_then(|r| r.resolve(key, node as usize + 1));
    let to = if at_node { node.saturating_add(1) } else { node };
    let (ipos, ival, ..) = sh.names.reaching(key, (k, to), &sh.font);
    match (local, ipos) {
        (Some((d, _)), Some(p)) if p.0 == k && p.1 != END && p.1 >= d => ival,
        (Some((_, v)), _) => v,
        (None, _) => ival,
    }
}

/// The page builder's environment in a scan task: it reads where the page
/// builder is called, runs output routines there and installs its
/// definitions.
struct BuildFold<'a> {
    sh: &'a Shared,
    k: PieceId,
    node: u32,
    run: Option<Arc<Run>>,
    /// The last evaluation's definitions' places not made again yet, in
    /// order.
    predicted: Vec<Pos>,
    /// Where this evaluation's page builder fired, in order.
    fired: Vec<Pos>,
    routines: usize,
    kept: usize,
}

impl BuildFold<'_> {
    /// The scan is at `at`: the last evaluation's page builder definitions
    /// before it that this one did not make again go (their readers
    /// marked). Whether a reader was marked (the scan stops).
    fn pass(&mut self, at: Pos) -> bool {
        let n = self.predicted.iter().take_while(|&&p| self.sh.before(p, at)).count();
        if n == 0 {
            return false;
        }
        let gone: Vec<Pos> = self.predicted.drain(..n).collect();
        self.retract(gone)
    }

    /// [`BuildFold::pass`] to the end of the document.
    fn pass_all(&mut self) -> bool {
        if self.predicted.is_empty() {
            return false;
        }
        let gone: Vec<Pos> = std::mem::take(&mut self.predicted);
        self.retract(gone)
    }

    /// The last evaluation's page builder definitions at `gone` go (their
    /// readers marked); whether one was.
    fn retract(&mut self, gone: Vec<Pos>) -> bool {
        let sh = self.sh;
        let mut g = lock(sh);
        let mut any = false;
        for pos in gone {
            let Some(i) = g.predicted.iter().position(|x| x.0 == pos) else {
                continue;
            };
            let (_, keys) = g.predicted.remove(i);
            for key in &keys {
                sh.names.set(pos, key, None);
            }
            any |= refresh_exports(&mut g, sh, pos, &keys);
            any |= changed_at(&mut g, sh, pos, keys);
        }
        drop(g);
        notify(sh, any);
        any
    }

    /// The value of `key` an output routine reads first: the fold's
    /// definition, else where the page builder is called.
    fn routine_read(&self, overlay: &[(Key, Val)], key: &Key) -> Val {
        if let Some((_, v)) = overlay.iter().rev().find(|(x, _)| x == key) {
            return v.clone();
        }
        read_at(self.sh, self.run.as_deref(), self.k, self.node, key)
    }

    /// The last build's run of this routine on this page, if everything
    /// it read is the same.
    fn find_memo(&self, rt: &Routine, overlay: &[(Key, Val)]) -> Option<Arc<Memo>> {
        let cands = self.sh.memo_old.get(&memo_key(&rt.page.b))?;
        cands
            .iter()
            .find(|m| {
                // (the fire's ordinal is not an input: only the name its
                // PDF events and their answers are filed under, `pdf_scope`;
                // the memo's answers are read under this fire's)
                (Arc::ptr_eq(&m.toks, &rt.toks) || m.toks == rt.toks)
                    && (Arc::ptr_eq(&m.page, &rt.page.b) || m.page == rt.page.b)
                    && m.fire == rt.reads
                    && m.overlay == overlay
                    && m.reads.iter().all(|(key, val)| {
                        let key = match key {
                            Key::PdfAns(1, o, n, f) if *o == m.ordinal => &Key::PdfAns(1, rt.fire, *n, *f),
                            k => k,
                        };
                        val.seen_as(&self.routine_read(overlay, key))
                    })
            })
            .cloned()
    }

    /// Runs the routine as nodes (tex.web 1025–1026) from the entry state
    /// `output::output_entry` makes of the fire: its paragraphs broken,
    /// its pages and its list at its end (`OutputEnd`) gathered.
    fn run_routine(&self, rt: &Routine, overlay: &[(Key, Val)]) -> Memo {
        let mut env = RoutineEnv {
            fold: self,
            overlay,
            fire: rt.fire,
        };
        let entry = output::output_entry(&rt.page, &rt.reads, &rt.toks);
        let run = Arc::new(output::run_routine(&entry, &mut env));
        let paras: Vec<ParaLines> = run
            .paras
            .iter()
            .map(|p| {
                (
                    p.clone(),
                    if p.lb.is_some() {
                        Arc::new(break_lines(p))
                    } else {
                        Arc::new(Vec::new())
                    },
                )
            })
            .collect();
        let mut items = Vec::new();
        let mut pd = IGNORE_DEPTH;
        for it in material(&paras) {
            push_vertical(&mut items, &mut pd, it);
        }
        let lines: Vec<Option<Lines>> = paras.iter().map(|(_, l)| Some(l.clone())).collect();
        let log = logs::texts(&logs::fragment(&[], &run, &lines));
        Memo {
            ordinal: rt.fire,
            toks: rt.toks.clone(),
            page: rt.page.b.clone(),
            fire: rt.reads.clone(),
            overlay: overlay.to_vec(),
            reads: run
                .reads
                .iter()
                .map(|(k, _, v)| (k.clone(), v.clone()))
                .collect(),
            exports: run.exports.clone(),
            items,
            log,
            errors: run.errors.clone(),
        }
    }
}

impl FoldEnv for BuildFold<'_> {
    fn xetex(&self) -> bool {
        self.sh.names.given.xe.is_some()
    }

    fn read(&mut self, key: &Key) -> Val {
        // (the last build's definitions where the page builder fires now,
        // not made again yet, are not read: they are what its fires there
        // made, and this build's first fire there decides afresh — whether
        // it holds the insertions, which routine runs; a later fire at the
        // same place reads this build's)
        let pos = (self.k, self.node);
        let fresh = !self.predicted.contains(&pos) || self.fired.last() == Some(&pos);
        read_at_with(self.sh, self.run.as_deref(), self.k, self.node, key, fresh)
    }

    fn toks_text(&mut self, toks: &[Tok]) -> Vec<u8> {
        let esc = match self.read(&Key::IntPar(crate::val::IntPar::Escapechar)) {
            Val::Int(e) => e,
            _ => -1,
        };
        let cs = self.sh.names.cs.clone();
        crate::pdf::toks_text(toks, &|n| cs.name(n), esc, &mut |c| self.read(&Key::Cat(c)) == Val::Cat(11))
    }

    fn fire(&mut self, defs: Vec<(Key, Val)>, routine: Option<Routine>) -> Fired {
        let mut fired = Fired::default();
        let pos = (self.k, self.node);
        if self.fired.last() != Some(&pos) {
            self.fired.push(pos);
        }
        if self.predicted.contains(&pos) {
            self.predicted.retain(|&p| p != pos);
            // (the last build's definitions here go before a routine reads:
            // it reads only this build's; without one they are compared
            // as they are made again)
            if routine.is_some() {
                fired.halt |= self.retract(vec![pos]);
            }
        }
        let mut all = defs.clone();
        let mut memo = None;
        if let Some(rt) = routine {
            let m = if let Some(m) = self.find_memo(&rt, &defs) {
                self.kept += 1;
                fired.memo = true;
                m
            } else {
                self.routines += 1;
                Arc::new(self.run_routine(&rt, &defs))
            };
            fired.items.clone_from(&m.items);
            // (its PDF events filed under this fire's ordinal)
            if m.ordinal != rt.fire {
                for it in &mut fired.items {
                    if let crate::item::Item::Pdf(p) = it
                        && p.tag[0] == 1
                        && p.tag[1] == m.ordinal
                    {
                        let mut q = (**p).clone();
                        q.tag[1] = rt.fire;
                        *it = crate::item::Item::Pdf(Arc::new(q));
                    }
                }
            }
            fired.log.clone_from(&m.log);
            fired.errors.clone_from(&m.errors);
            // (its exports — `\box255` void, `\outputpenalty`,
            // `\deadcycles`, the marks, what it defined globally — at the
            // firing place, after the fold's own)
            for (key, val) in &m.exports {
                if *key == Key::Page(DEAD_CYCLES)
                    && let Val::Int(d) = val
                {
                    fired.dead_cycles = Some(*d);
                }
                match all.iter_mut().find(|x| x.0 == *key) {
                    Some(x) => x.1 = val.clone(),
                    None => all.push((key.clone(), val.clone())),
                }
            }
            memo = Some(m);
        }
        fired.halt |= install_fold_defs(self.sh, pos, all, memo);
        fired
    }
}

/// An output routine's reads: the fold's definitions, then the document's
/// where the page builder is called (all final: the pieces before that
/// place have settled).
struct RoutineEnv<'a> {
    fold: &'a BuildFold<'a>,
    overlay: &'a [(Key, Val)],
    fire: u32,
}

impl Env for RoutineEnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.fold.sh.names.cs
    }

    fn pdf_scope(&self) -> (u32, u32) {
        (1, self.fire)
    }

    fn interaction_option(&self) -> Option<i32> {
        self.fold.sh.names.given.interaction_option
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.fold.sh.fonts.get(name)
    }

    fn xetex(&self) -> bool {
        self.fold.sh.names.given.xe.is_some()
    }

    fn native_font(&mut self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        self.fold.sh.names.given.native_font(name, size, tracing)
    }

    fn reach(&mut self, key: &Key, _node: u32) -> (Val, Src, u32, u32) {
        (
            self.fold.routine_read(self.overlay, key),
            Src::Initial,
            0,
            END,
        )
    }
}

/// The page builder's definitions at `pos` (a fire, its routine's
/// included) enter the names index there; the readers they reach are
/// marked. Whether one was (the scan stops).
fn install_fold_defs(sh: &Shared, pos: Pos, defs: Vec<(Key, Val)>, memo: Option<Arc<Memo>>) -> bool {
    let mut g = lock(sh);
    if let Some(m) = memo {
        g.memo_new.push(m);
    }
    let mut changed: Vec<Key> = Vec::new();
    if let Some(i) = g.predicted.iter().position(|x| x.0 == pos) {
        let (_, keys) = g.predicted.remove(i);
        for key in keys {
            if !defs.iter().any(|d| d.0 == key) {
                sh.names.set(pos, &key, None);
                changed.push(key);
            }
        }
    }
    for (key, val) in &defs {
        if sh.names.at(key, pos).as_ref() != Some(val) {
            sh.names.set(pos, key, Some(val));
            changed.push(key.clone());
        }
    }
    let moved = refresh_exports(&mut g, sh, pos, &changed);
    match g.fold_defs.iter_mut().find(|x| x.0 == pos.0 && x.1 == pos.1) {
        Some(x) => {
            for (key, val) in defs {
                match x.2.iter_mut().find(|d| d.0 == key) {
                    Some(d) => d.1 = val,
                    None => x.2.push((key, val)),
                }
            }
        }
        None => g.fold_defs.push((pos.0, pos.1, defs)),
    }
    let any = changed_at(&mut g, sh, pos, changed) || moved;
    drop(g);
    notify(sh, any);
    any
}

/// Definitions of `keys` between piece `pos.0`'s nodes changed: its exports
/// of them, as [`export_of`] gives them.
fn refresh_exports(g: &mut State, sh: &Shared, pos: Pos, keys: &[Key]) -> bool {
    if pos.1 == END {
        return false;
    }
    let k = pos.0;
    let Some(run) = g.seq.p.get(k as usize).and_then(|ps| ps.run.clone()) else {
        return false;
    };
    let moved: Vec<Key> = keys.iter().filter(|key| refresh_export(&sh.names, &sh.font, k, &run, key)).cloned().collect();
    changed_at(g, sh, (pos.0, END), moved)
}

/// The names `keys` changed at `pos`: logged, and their readers after it
/// whose values changed marked (a place between a piece's nodes reaches
/// that piece's own nodes after it that read the name: by their reads,
/// never by what the piece defines). Whether one was.
fn changed_at(g: &mut State, sh: &Shared, pos: Pos, keys: Vec<Key>) -> bool {
    if keys.is_empty() {
        return false;
    }
    let mut marks: Vec<(PieceId, Key, u32)> = reached(g, sh, pos, &keys).into_iter().map(|(r, k)| (r, k, 0)).collect();
    if pos.1 != END
        && let Some(run) = g.seq.p.get(pos.0 as usize).and_then(|ps| ps.run.clone())
    {
        let k = pos.0;
        let last = if run.lean.is_none() {
            let fresh = g.last_readers.get(&k).is_some_and(|(r, _)| Arc::ptr_eq(r, &run));
            if !fresh {
                let m = Arc::new(run.last_readers());
                g.last_readers.insert(k, (run.clone(), m));
            }
            g.last_readers.get(&k).map(|x| x.1.clone())
        } else {
            None
        };
        for key in &keys {
            // (the page builder's quantities are the scan's to validate; a
            // lean run's nodes after the place read the name if its reads
            // from before the piece say so, or a definition of its own
            // reaching across the place may be what they read)
            let after = match &last {
                Some(m) => m.get(key).is_some_and(|&j| j > pos.1),
                None => run.values_read(key).iter().any(|x| x.1 > pos.1) || run.defines_across(key, pos.1),
            };
            if !matches!(key, Key::Page(_)) && after {
                marks.push((k, key.clone(), pos.1 + 1));
            }
        }
    }
    g.names_log.push(keys);
    sh.names_gen.store(g.names_log.len(), Ordering::Release);
    let any = !marks.is_empty();
    for (r, key, from) in marks {
        if !mark_from(g, &sh.flags, r, key, from) && g.seq.p[r as usize].status == Status::Running {
            sh.flags.set(r, CANCEL, true);
        }
    }
    any
}

/// A run waits until a definition is installed (the generation moves from
/// `seen`) or the evaluation ends. Every thread parked so, none idle to
/// look, and nothing installed for a while: the scheduler stalled.
fn park(sh: &Shared, seen: u64) {
    let moved = || sh.generation.load(Ordering::Acquire) != seen;
    let mut p = sh.park.lock().expect("park");
    sh.parked_n.fetch_add(1, Ordering::AcqRel);
    while !moved() {
        let (p2, t) = sh.parked.wait_timeout(p, STALL_CHECK).expect("park");
        p = p2;
        if t.timed_out() && !moved() {
            drop(p);
            {
                let mut g = lock(sh);
                let parked = sh.parked_n.load(Ordering::Acquire);
                if g.done {
                    sh.parked_n.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
                if !moved() && parked == g.inflight && g.inflight == g.stats.work.len() {
                    stall(&mut g, sh, "the scheduler stalled (every run parked)");
                }
            }
            p = sh.park.lock().expect("park");
        }
    }
    sh.parked_n.fetch_sub(1, Ordering::AcqRel);
}

/// The node that made the item at `path` inside `it`, which node `maker`
/// made.
fn inner_node(maker: u32, it: &crate::item::Item, path: &[usize]) -> Option<u32> {
    let Some((&first, rest)) = path.split_first() else { return Some(maker) };
    let (crate::item::Item::Box(b) | crate::item::Item::Append(b, _)) = it else { return Some(maker) };
    let node = b.origins.node_of(maker, first).unwrap_or(maker);
    inner_node(node, b.list.get(first)?, rest)
}

/// A run's reads of the page builder's quantities, by node.
fn page_reads(run: &Run) -> Vec<(u8, u32, Val)> {
    let mut v: Vec<(u8, u32, Val)> = (0..=INSERT_PENALTIES)
        .flat_map(|q| run.values_read(&Key::Page(q)).iter().map(move |(_, n, val)| (q, *n, val.clone())))
        .collect();
    let all = run.all_reads();
    let a = all.partition_point(|x| x.0 < Key::Page(INSERT_PENALTIES + 1));
    for (key, n, val) in &all[a..] {
        let Key::Page(q) = key else { break };
        v.push((*q, *n, val.clone()));
    }
    v.sort_by_key(|x| x.1);
    v
}

/// The scan is before the first item of node `upto` of piece `k` (`None`:
/// after the piece's material): its reads of the page builder's quantities
/// up to there are checked against the page as the scan has it. At the
/// first read that saw another value its piece is marked from its node
/// and `false` is returned: the scan stops there.
fn check_page_reads(sh: &Shared, scan: &Scan, k: PieceId, reads: &[(u8, u32, Val)], fr: &mut usize, upto: Option<u32>) -> bool {
    while let Some((q, node, read)) = reads.get(*fr).filter(|r| upto.is_none_or(|u| r.1 <= u)) {
        *fr += 1;
        let key = Key::Page(*q);
        let now = Val::Int(scan.page_quantity(*q));
        sh.names.set_fold_value(&key, (k, *node), now.clone());
        if now != *read {
            let mut g = lock(sh);
            let done = mark_from(&mut g, &sh.flags, k, key, *node);
            if !done && g.seq.p[k as usize].status == Status::Running {
                sh.flags.set(k, CANCEL, true);
            }
            return false;
        }
    }
    true
}

/// Wakes the parked readers, and the idle workers if there is new work.
fn notify(sh: &Shared, work: bool) {
    {
        let _p = sh.park.lock().expect("park");
        sh.generation.fetch_add(1, Ordering::AcqRel);
    }
    sh.parked.notify_all();
    if work {
        sh.cv.notify_all();
    }
}

/// The PDF file of the pages (`pdf::fold`): the pages' streams (in
/// `bodies`, made again where the fold says), what the job's end reads
/// (the definitions reaching the end of piece `last`, the place before
/// the next), the files the fold
/// reads (given, in the input directories, then TeX Live's).
/// The files the PDF fold reads: given, in the input directories, then
/// TeX Live's.
struct PdfFiles<'a> {
    names: &'a Names,
    font: &'a Arc<Font>,
    /// The TFM files `\font` can load (a virtual font's local fonts).
    fonts: &'a Arc<FontFiles>,
    /// The job's last piece: what the job's end reads is what reaches
    /// after it.
    last: Option<PieceId>,
}

impl crate::pdf::fold::Files for PdfFiles<'_> {
    fn get(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        self.names.given.pdf_found(name)
    }
    fn font_attr(&mut self, key: &str) -> Option<crate::pdf::what::Text> {
        match self.names.after(&Key::FontAttr(Arc::from(key)), self.last, self.font) {
            Val::Name(s) if !s.is_empty() => Some(s.chars().map(|c| u32::from(c).min(255) as u8).collect()),
            _ => None,
        }
    }
    fn vf(&mut self, name: &str) -> Option<Arc<[u8]>> {
        match &self.names.given.finder {
            Some((f, prog)) => f.find(name, KpseFormat::Vf, prog).and_then(|p| f.read(&p)),
            None => crate::tfm::vf_file(name),
        }
    }
    fn tfm(&mut self, name: &str) -> Option<Arc<[u8]>> {
        match &self.names.given.finder {
            Some((f, prog)) => f.find(name, KpseFormat::Tfm, prog).and_then(|p| f.read(&p)),
            None => self.fonts.get(name),
        }
    }
    fn glyph_unicode(&mut self, name: &str) -> Option<crate::pdf::font::unicode::Uni> {
        use crate::pdf::font::unicode::Uni;
        match self.names.after(&Key::GlyphUnicode(Arc::from(name)), self.last, self.font) {
            Val::Int(c) => Some(Uni::Code(i64::from(c))),
            Val::Name(s) => Some(Uni::Seq(s)),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pdf_file(
    names: &Names,
    font: &Arc<Font>,
    fonts: &Arc<FontFiles>,
    last: Option<PieceId>,
    numbers: &dvi::Numbers,
    ship_fonts: Vec<u32>,
    pages: &[Page],
    orgs: &[PageOrg],
    tail: &crate::page::PdfEvents,
    bodies: &mut [Arc<dvi::Body>],
) -> crate::pdf::fold::Written {
    use crate::val::{IntPar, TokPar};
    let read = |key: &Key| names.after(key, last, font);
    let int = |p: IntPar| match read(&Key::IntPar(p)) {
        Val::Int(i) => i,
        _ => 0,
    };
    let esc = int(IntPar::Escapechar);
    let text = |p: TokPar| match read(&Key::TokPar(p)) {
        Val::Toks(t) if !t.is_empty() => {
            let cs = &names.cs;
            Some(Arc::from(crate::pdf::toks_text(&t, &|n| cs.name(n), esc, &mut |c| read(&Key::Cat(c)) == Val::Cat(11))))
        }
        _ => None,
    };
    let end = crate::pdf::fold::End {
        compress_level: int(IntPar::Pdfcompresslevel),
        gen_tounicode: int(IntPar::Pdfgentounicode),
        omit_charset: int(IntPar::Pdfomitcharset),
        info_omit_date: int(IntPar::Pdfinfoomitdate),
        suppress_ptex_info: int(IntPar::Pdfsuppressptexinfo),
        ptex_use_underscore: int(IntPar::Pdfptexuseunderscore),
        omit_info_dict: int(IntPar::Pdfomitinfodict),
        pages_attr: text(TokPar::Pdfpagesattr),
        date: crate::pdf::pdf_date(names.given.pdf_date),
        file_name: format!("{}.pdf", names.given.job),
        banner: crate::graph::PDFTEX_BANNER.to_string(),
        params: Arc::new(crate::pdf::read_params(&mut |k| read(k))),
        page_width: match read(&Key::DimenPar(crate::val::DimenPar::Pdfpagewidth)) {
            Val::Int(i) => i,
            _ => 0,
        },
        page_height: match read(&Key::DimenPar(crate::val::DimenPar::Pdfpageheight)) {
            Val::Int(i) => i,
            _ => 0,
        },
        ship_fonts,
    };
    let mut pdf_bodies: Vec<Arc<crate::pdf::content::PageBody>> =
        bodies.iter().map(|b| b.pdf.clone().unwrap_or_else(|| Arc::new(crate::pdf::content::empty()))).collect();
    let events: Vec<&crate::page::PdfEvents> = orgs.iter().map(|o| &o.pdf).chain(std::iter::once(tail)).collect();
    let w = crate::pdf::fold::assemble(pages, &mut pdf_bodies, &events, numbers, &end, &mut PdfFiles { names, font, fonts, last });
    for (b, p) in bodies.iter_mut().zip(pdf_bodies) {
        if !b.pdf.as_ref().is_some_and(|x| Arc::ptr_eq(x, &p)) {
            let mut nb = dvi::Body { bytes: Vec::new(), fonts: Vec::new(), fixed: None, max_push: 0, io: p.io, pdf: None };
            nb.pdf = Some(p);
            *b = Arc::new(nb);
        }
    }
    w
}

impl Names {
    /// What `key` means after piece `last` (none: before any piece): the
    /// job's end.
    fn after(&self, key: &Key, last: Option<PieceId>, font: &Arc<Font>) -> Val {
        match last {
            Some(k) => self.at(key, (k, END)).unwrap_or_else(|| self.reaching(key, (k, END), font).1),
            None => self.initial(key, font),
        }
    }
}

/// The join's reads (a page's `\write`s expanded where it was shipped):
/// every piece has run, nothing parks.
struct JoinEnv<'a> {
    names: &'a Names,
    font: &'a Arc<Font>,
    fonts: &'a Arc<FontFiles>,
    virt: &'a RwLock<Virt>,
    k: PieceId,
    /// The run ran up to a cut the cutter missed: so does its run again.
    cut: bool,
    /// The run ended at an `\input` whose file is pieces: so does its run
    /// again.
    split: bool,
    /// The run stopped here: so does its run again.
    stop: usize,
}

/// A lean piece's run with its nodes again: run again, reading only, over
/// the names index as it is — for the join's page events and the dump.
fn regen(env: &mut JoinEnv<'_>, text: &[char], r: &Arc<Run>) -> Arc<Run> {
    if r.lean.is_none() {
        return r.clone();
    }
    let entry = r.entry();
    env.cut = r.ends_at_cut();
    env.split = r.input().is_some();
    env.stop = if env.cut { r.out().pos } else { usize::MAX };
    Arc::new(run_piece(text, env, None, &[], Some(&entry)))
}

impl Env for JoinEnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.names.cs
    }

    fn limits(&self) -> crate::print::Limits {
        self.names.given.limits
    }

    fn interaction_option(&self) -> Option<i32> {
        self.names.given.interaction_option
    }

    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.fonts.get(name)
    }

    fn xetex(&self) -> bool {
        self.names.given.xe.is_some()
    }

    fn native_font(&mut self, name: &str, size: i32, tracing: i32) -> crate::native::Loaded {
        self.names.given.native_font(name, size, tracing)
    }

    fn cut_ok(&mut self) -> bool {
        self.cut
    }

    fn split_inputs(&mut self) -> bool {
        self.split
    }

    fn stop_at(&mut self, pos: usize) -> bool {
        pos == self.stop
    }

    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let (def, val, lo, hi) = self.names.reaching(key, (self.k, node), self.font);
        let src = match def {
            None => Src::Initial,
            Some(d) if d.1 == END => Src::Piece(d.0),
            Some(d) => Src::Piece(self.virt.read().expect("virt").ids.get(&d).copied().unwrap_or(d.0)),
        };
        (val, src, lo, hi)
    }

    fn folds(&mut self, key: &Key) -> bool {
        matches!(key, Key::Page(_))
    }

    fn fold_read(&mut self, key: &Key, node: u32) -> Option<Val> {
        self.names.fold_value(key, (self.k, node), self.font)
    }

    fn file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.names.given.file(name)
    }

    fn file_path(&mut self, name: &str) -> Arc<str> {
        self.names.given.path(name)
    }

    fn jobname(&mut self) -> Arc<str> {
        self.names.given.job.clone()
    }

    fn line_base(&mut self) -> i32 {
        self.names.given.base(self.k) as i32
    }
}

/// xdvipdfmx's finder in [`Build::xdv_pdf`], one more [`Kpse`]: the
/// document's files first (those given in memory, then the input
/// directories, as the picture specials name them, `./NAME` in the
/// first), then the build's finder.
struct DocFiles {
    mem: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    dirs: Arc<Vec<PathBuf>>,
    inner: Arc<dyn crate::tfm::Kpse>,
}

impl crate::tfm::Kpse for DocFiles {
    fn find(&self, name: &str, format: crate::tfm::KpseFormat, prog: &str) -> Option<String> {
        let bare = name.strip_prefix("./").unwrap_or(name);
        if self.mem.contains_key(bare) {
            return Some(format!("./{bare}"));
        }
        if let Some(p) = self.dirs.iter().map(|d| d.join(bare)).find(|p| p.is_file()) {
            return Some(p.to_string_lossy().into_owned());
        }
        self.inner.find(name, format, prog)
    }

    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        let bare = path.strip_prefix("./").unwrap_or(path);
        if let Some(b) = self.mem.get(bare) {
            return Some(b.clone());
        }
        if std::path::Path::new(bare).is_relative()
            && let Some(b) = self.dirs.iter().find_map(|d| std::fs::read(d.join(bare)).ok())
        {
            return Some(Arc::from(b));
        }
        self.inner.read(path)
    }
}

/// The job of each piece of a session (0 for all of a document that is
/// not one).
fn job_of(texts: impl Iterator<Item = bool>) -> Vec<usize> {
    let mut j = 0usize;
    let mut seen = false;
    texts
        .map(|starts| {
            if starts && seen {
                j += 1;
            }
            seen = true;
            j
        })
        .collect()
}

fn dump_piece(out: &mut String, k: usize, ps: &Piece, r: &Run, cs: &CsTable, seq: &Seq) {
    let how = if ps.runs == 0 { "kept" } else { "ran" };
    let _ = writeln!(
        out,
        "piece {k} (id {}) [{how}; {} nodes, {} run]: {} reads from before it, {} definitions after it, {} paragraphs; entered {:?}; {:?}",
        ps.id,
        r.nodes_len(),
        r.ran,
        r.read_count(),
        r.exports.len(),
        r.paras.len(),
        seq.entry_of(ps.id),
        ps.text.iter().take(40).collect::<String>()
    );
    for (i, n) in r.chain.nodes.iter().enumerate() {
        let _ = write!(out, "  n{i} {:?} {}", n.kind, Shown(&n.cmd, cs));
        for rd in n.reads.iter() {
            match rd.src {
                Src::Node(d) => {
                    let _ = write!(out, " | reads {} = {} from n{d}", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
                Src::Piece(p) => {
                    let _ = write!(out, " | reads {} = {} from piece id {p}", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
                Src::Initial => {
                    let _ = write!(out, " | reads {} = {} initial", Shown(&rd.key, cs), Shown(&rd.val, cs));
                }
            }
        }
        if let Some(c) = n.phi() {
            let _ = write!(out, " | test={}", c.test);
        }
        for d in n.defs() {
            let _ = write!(out, " | defines {}{} = {}", if d.global { "global " } else { "" }, Shown(&d.key, cs), Shown(&d.val, cs));
        }
        out.push('\n');
    }
}

impl Output {
    /// The transcripts (and terminals) of a session's jobs, laid out with
    /// `limits`, each ending as `close_files_and_terminate` ends it
    /// (tex.web 642: the pages written on `dvi_name`, or none).
    pub fn job_logs(&self, limits: &crate::print::Limits, dvi_name: &str) -> Vec<crate::print::JobLog> {
        use crate::show::{LN, NL};
        let foots: Vec<String> = self
            .job_pages
            .iter()
            .map(|&n| {
                let mut f = String::new();
                f.push(NL);
                if n == 0 {
                    f.push_str("No pages of output.");
                } else {
                    let _ = write!(
                        f,
                        "Output written on {dvi_name} ({n} page{}, {} bytes).",
                        if n == 1 { "" } else { "s" },
                        self.dvi.len()
                    );
                }
                f.push(LN);
                f
            })
            .collect();
        crate::print::session(&self.log, limits, &foots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Arc<Font> {
        let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
        Arc::new(Font::parse("cmr10", &b).unwrap())
    }

    /// Places are pieces (by their order labels, never their ids) and
    /// nodes: an element between piece 12's nodes 5 and 6 defines `\x`:
    /// node 6 on read it, the nodes before read piece 10's, the pieces
    /// after read piece 12's exports.
    #[test]
    fn places_are_pieces_and_nodes() {
        let labels: Arc<RwLock<Labels>> = Arc::default();
        // (ids out of order: the labels decide)
        labels.write().unwrap().insert_after(None, &[10, 11, 2, 13]);
        let given = Given::new(Arc::new(Vec::new()), Arc::from("texput"), Arc::default());
        let (names, font) = (Names::new(given, true, Arc::new(CsTable::new()), None, labels.clone()), font());
        let x = Key::Cs(names.cs.id("x"));
        names.set((10, END), &x, Some(&Val::Int(1)));
        names.set((2, 5), &x, Some(&Val::Int(2)));
        names.set((2, END), &x, Some(&Val::Int(3)));
        assert!(matches!(names.reaching(&x, (10, 7), &font), (None, _, 0, END)));
        assert!(matches!(names.reaching(&x, (11, 7), &font), (Some((10, END)), Val::Int(1), 0, END)));
        assert!(matches!(names.reaching(&x, (2, 3), &font), (Some((10, END)), Val::Int(1), 0, 6)));
        assert!(matches!(names.reaching(&x, (2, 6), &font), (Some((2, 5)), Val::Int(2), 6, END)));
        assert!(matches!(names.reaching(&x, (13, 0), &font), (Some((2, END)), Val::Int(3), 0, END)));
        // (a piece put between two others: nothing moves)
        labels.write().unwrap().insert_after(Some(11), &[40, 41]);
        assert!(matches!(names.reaching(&x, (40, 0), &font), (Some((10, END)), Val::Int(1), 0, END)));
        assert!(matches!(names.reaching(&x, (13, 0), &font), (Some((2, END)), Val::Int(3), 0, END)));
        let l = labels.read().unwrap();
        assert_eq!(l.ids(), vec![10, 11, 40, 41, 2, 13]);
    }

    /// Labels spread again when a gap runs out, keeping the order.
    #[test]
    fn labels_spread_in_order() {
        let mut l = Labels::default();
        l.insert_after(None, &[0, 1]);
        for i in 2..200u32 {
            l.insert_after(Some(0), &[i]);
        }
        let ids = l.ids();
        assert_eq!(ids.len(), 200);
        assert_eq!(ids[0], 0);
        assert_eq!(*ids.last().unwrap(), 1);
        // (each put right after 0: the latest first)
        assert_eq!(ids[1], 199);
    }
}
