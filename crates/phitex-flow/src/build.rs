//! Evaluation: N equal threads, a scheduler function, edges from every
//! definition to the nodes that read it.
//!
//! The graph's nodes are made by running pieces (see `graph`). What a
//! piece defines at its end goes into the **names index** (name -> the
//! pieces defining it, by position), which every thread reads at once; a
//! node's read of a name from before its piece resolves there to the
//! latest definition before it. Each such read is an **edge**, kept in the
//! readers index: name -> (reader piece, reader node, defining piece).
//!
//! A read is answered with the definitions known so far; it may be wrong
//! if an earlier piece that has not run yet defines the name. So when a
//! piece's definitions are installed, exactly the readers of the names
//! whose values changed are marked **dirty** (the pieces after it that read
//! the name from before it, by name: every node of theirs reading it is
//! dirty), and those pieces run again from their first dirty node. A
//! piece's own reads are validated when it is installed, against the index
//! as it is then; a stale read marks its name and the piece runs again
//! from its readers. Under `Reads::Suspend` a read of a name whose
//! definition is being remade parks until that definition is installed
//! (or runs the defining piece itself if nobody has started it), and so
//! does a read of an undefined name while an earlier piece has not
//! finished (or it runs the first earlier piece nobody has started); under
//! `Reads::Speculate` it takes the old value and is validated. Either way
//! the result is the same: the scheduler affects speed, never results.
//!
//! Line breaking is a task per paragraph (a paragraph equal to one already
//! broken keeps its lines). The page builder is a fold over the lines in
//! order; it reads a piece once nothing before it can change any more. A
//! page equal to the last build's page at its place keeps its DVI.
//!
//! An edit is the same evaluation: pieces whose text is unchanged keep
//! their nodes; readers of a definition whose place changed are marked;
//! the edited pieces run from the first node the edit touches. A cold
//! build is an edit from nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::dvi;
pub use crate::graph::MAIN_MEMORY;

/// The cutter's default: a cut proposed every this many lines (at a line
/// beginning with a control sequence, at brace depth 0), besides blank
/// lines ([`Options::cut_lines`]).
pub const CUT_LINES: usize = 200;
use crate::graph::output::{self, DEAD_CYCLES, OutputReads};
use crate::graph::{
    Check, Env, FileText, Para, PieceId, RState, Run, Src, cut_every, cut_from, run_piece,
};
use crate::hash::{FxMap, FxSet, hash_of};
use crate::linebreak::break_lines;
use crate::logs::{self, Frag};
use crate::page::{
    CallLog, Fired, FoldEnv, IGNORE_DEPTH, Lines, Page, PageOrg, Routine, Scan, material,
    push_vertical,
};
use crate::show::RAW;
use crate::tfm::{Font, FontFiles};
use crate::val::{Key, Meaning, Shown, Tok, TokPar, Val};

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
    /// (`graph::words_of`): past it, "TeX capacity exceeded" ends the run
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
    /// Each round's wall time.
    pub round_walls: Vec<Duration>,
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
}

/// A piece whose definition was read before any definition.
const INITIAL: PieceId = u32::MAX;
/// A piece not in this build.
const NONE: u32 = u32::MAX;

/// A piece as the last build left it.
struct Kept {
    id: PieceId,
    text: Arc<[char]>,
    run: Option<Arc<Run>>,
    lines: Vec<Option<Lines>>,
    srcs: Vec<PieceId>,
    log: Option<Arc<Frag>>,
    /// Left by a round that stopped before it ran again: the names it is
    /// still to read again (it runs from them).
    pending: Option<Dirty>,
    /// What its run says of the sequence ([`Expect::of`], made when it ran).
    got: Expect,
    /// It ran in an edit: it keeps its nodes until this build.
    hot_until: u64,
}

/// What a piece is entered with (CORE.md, `\input` and cuts): the
/// canonical start (a job's first piece), the state an `\input` node left
/// (the file's first piece, `.0` the calling piece), the rest of the
/// calling line (`.0` the calling piece, `.1` the file's last), or the
/// state the piece before it (`.0`) leaves at the cut — assumed to be the
/// canonical start, or the start of the file `.1` calls (the state its
/// `\input` left), until that piece has run: the entry is a read of its
/// end, validated like any read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entry {
    Start,
    File(usize),
    Rest(usize, Option<usize>),
    After(usize, Option<usize>),
}

/// What the sequence assumed of a piece's run: the file its last node
/// reads (and where that node ends), and whether the file ends after it
/// (`\endinput`). A run that differs changes the sequence.
#[derive(Clone, Default)]
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
        if r.nodes_len() == 0 {
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

    fn same(&self, o: &Expect) -> bool {
        self.stop == o.stop
            && self.endinput == o.endinput
            && self.ends == o.ends
            && self.cut == o.cut
            && self.join == o.join
            && match (&self.input, &o.input) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
                (None, None) => true,
                _ => false,
            }
    }
}

/// A piece of the sequence a round evaluates.
struct Spec {
    text: Arc<[char]>,
    entry: Entry,
    /// Its lines before it in its own file, and what the log has after it.
    base: usize,
    post: Option<String>,
    /// The last build's piece it is (its run kept).
    old: Option<usize>,
    expect: Expect,
    /// Its `\input`s are read in the piece ([`Build::inline`]).
    inline: bool,
    /// Where texts it was joined from begin: its run stops at the first
    /// where it can.
    stops: Arc<[usize]>,
    /// `old` is another place's piece of equal text: its run is where this
    /// one runs from, not kept.
    moved: bool,
}

/// Finds the last build's pieces by their text: the same `Arc`, or equal
/// text (in order), each taken once. (Equal texts are looked for among the
/// pieces of the same length: no text is hashed.)
struct Matcher {
    /// Every last build's piece by its text's `Arc` — those whose run a
    /// restructure stopped before it ran included: a piece keeps its own
    /// place, never another's run of equal text.
    by_ptr: FxMap<usize, Vec<usize>>,
    by_len: Option<FxMap<usize, VecDeque<usize>>>,
    used: Vec<bool>,
    /// The last build's piece taken last: one of equal text right after
    /// it is at its own place (a text cut again near an edit is a new
    /// `Arc`).
    last: Option<usize>,
}

impl Matcher {
    fn new(olds: &[Kept]) -> Matcher {
        let mut by_ptr: FxMap<usize, Vec<usize>> = FxMap::default();
        for (i, o) in olds.iter().enumerate() {
            by_ptr.entry(o.text.as_ptr() as usize).or_default().push(i);
        }
        Matcher {
            by_ptr,
            by_len: None,
            used: vec![false; olds.len()],
            last: None,
        }
    }

    /// The last build's piece of text `t` (the same `Arc`) still to be
    /// taken.
    fn by_arc(&self, olds: &[Kept], t: &Arc<[char]>) -> Option<usize> {
        self.by_ptr
            .get(&(t.as_ptr() as usize))?
            .iter()
            .copied()
            .find(|&i| Arc::ptr_eq(&olds[i].text, t) && !self.used[i])
    }

    /// Whether the last build's piece of text `t` (the same `Arc`), with a
    /// run, is still to be taken.
    fn has(&self, olds: &[Kept], t: &Arc<[char]>) -> bool {
        self.by_arc(olds, t).is_some_and(|i| olds[i].run.is_some())
    }

    /// The last build's piece of `t`: the same `Arc` (`.1` true: its own
    /// place), else one of equal text that ran (`.1` false: another
    /// place's run, which is never kept as it is — its reads are of that
    /// place: the piece runs from it, every read looked at again).
    fn find(&mut self, olds: &[Kept], t: &Arc<[char]>) -> Option<(usize, bool)> {
        if let Some(i) = self.by_arc(olds, t) {
            self.used[i] = true;
            self.last = Some(i);
            return Some((i, true));
        }
        let m = self.by_len.get_or_insert_with(|| {
            let mut m: FxMap<usize, VecDeque<usize>> = FxMap::default();
            for (i, o) in olds.iter().enumerate() {
                if o.run.is_some() {
                    m.entry(o.text.len()).or_default().push_back(i);
                }
            }
            m
        });
        let q = m.get_mut(&t.len())?;
        let next = self.last.map_or(0, |l| l + 1);
        let at = q
            .iter()
            .position(|&i| i == next && !self.used[i] && olds[i].text == *t)
            .or_else(|| q.iter().position(|&i| !self.used[i] && olds[i].text == *t))?;
        let i = q.remove(at)?;
        self.used[i] = true;
        let own = i == next;
        if own {
            self.last = Some(i);
        }
        Some((i, own))
    }
}

fn count_lines(t: &[char]) -> usize {
    t.iter().filter(|&&c| c == '\r').count()
}

/// The edges: name -> reader piece -> (its first node reading the name
/// from before the piece, the piece whose definition it read).
type Readers = FxMap<Key, FxMap<PieceId, (u32, PieceId)>>;

/// A piece's text and the pieces it was re-cut into.
type Recut = (Arc<[char]>, Vec<Arc<[char]>>);

/// How many texts after one were joined with it, the text all are, and
/// where each begins in it.
type Joined = (usize, Arc<[char]>, Arc<[usize]>);

/// A text, the texts after it it was joined with, the text that is all of
/// them, and where each after the first begins in it.
type Join = (Arc<[char]>, Vec<Arc<[char]>>, Arc<[char]>, Arc<[usize]>);

/// What survives between builds: the pieces with their nodes, the edges,
/// the pages with their DVI.
pub struct Build {
    font: Arc<Font>,
    /// The TFM files `\font` can load.
    fonts: Arc<FontFiles>,
    pieces: Vec<Kept>,
    pages: Vec<(Page, Arc<dvi::Body>)>,
    /// Where each page came from in the material (`page_orgs[i]` for
    /// `pages[i]`), and the page builder's marks.
    page_orgs: Vec<PageOrg>,
    marks: Vec<(u32, u32)>,
    next_id: PieceId,
    readers: Readers,
    /// The source the pieces were cut from, and the byte offset of each
    /// piece's first line in it.
    src: String,
    starts: Vec<usize>,
    /// The pieces of the main text (the cut's).
    texts: Vec<Arc<[char]>>,
    /// Files given in memory (`set_file`), found before the input
    /// directories' (a changed one is seen by its readers' reads).
    files: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    /// The pieces of each file read (the same `Arc`s while its text is),
    /// and the rest of each line that called `\input`.
    file_cuts: FxMap<usize, FileCut>,
    rests: FxMap<(usize, usize), Rest>,
    /// The pieces of the rest of a text after a cut the cutter missed (the
    /// same `Arc`s while the text is).
    recuts: FxMap<(usize, usize), Recut>,
    /// Two texts read as one piece (a run read on past the first), by
    /// their `Arc`s.
    joins: FxMap<usize, Vec<Join>>,
    /// The pieces whose `\input`s are read in the piece: the file's end
    /// left what the rest of the calling line goes on with (a paragraph, a
    /// group open) or its last command read on past it.
    inline: Vec<Arc<[char]>>,
    /// The cutter's [`Options::cut_lines`] the texts were cut with.
    cut_lines: usize,
    /// The last build's runs, in its order: what its pages were made of.
    page_runs: Option<Vec<Option<Arc<Run>>>>,
    /// The counts of this build's rounds that changed the sequence.
    carried: Stats,
    /// What the page builder left: whether it fired a routine or met an
    /// insertion or a mark (then it is made again from its start), its
    /// definitions at each place it fired (by the piece and node), and its
    /// routines' runs.
    fold: FoldKept,
    /// The control sequences' names met in the build, by number.
    cs: Arc<CsTable>,
    /// The ids of the elements between a piece's nodes (fires).
    virt: FxMap<(PieceId, u32), PieceId>,
    /// Where `\input` and `\openin` find files, and the job's name.
    inputs: Arc<Vec<PathBuf>>,
    job: Arc<str>,
    /// What the session gives before any definition: the terminal's lines
    /// (`Key::ReadStream(16)`), the interaction mode (`Key::Interaction`).
    terminal: Option<Arc<[u8]>>,
    interaction: i32,
    /// The command line's `-interaction` (`set_interaction_option`).
    interaction_option: Option<i32>,
    /// The sizes TeX was compiled with.
    limits: crate::print::Limits,
    /// A settled piece drops its nodes (`graph::Lean`; the default): a
    /// run of it again runs every node; else its nodes are kept and a run
    /// again runs only those whose reads or text changed. A piece that ran
    /// in one of the last [`HOT_BUILDS`] edits keeps them meanwhile (edits
    /// come near each other).
    lean: bool,
    /// Updates made (the cold build is the first).
    builds: u64,
    /// The format the build starts from (`set_format`).
    format: Option<Arc<FormatDefs>>,
    /// The PDF file's creation date, in seconds since 1970 (UTC):
    /// `SOURCE_DATE_EPOCH` for pdfTeX (`Build::set_pdf_date`).
    pdf_date: i64,
    /// The engine is XeTeX, with these installed fonts and this finder
    /// (`Build::set_xetex`).
    xe: Option<Arc<crate::native::Fonts>>,
}

/// A file's text and its pieces.
type FileCut = (Arc<str>, Vec<Arc<[char]>>);
/// A calling piece's text and the rest of its line.
type Rest = (Arc<[char]>, Arc<[char]>);
/// The page builder's definitions where it fired: the piece, the node, the
/// names and values.
type FoldDefs = Vec<(PieceId, u32, Vec<(Key, Val)>)>;

/// The page builder's results kept between builds.
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
    /// The files given in memory (`Build::set_file`), before the
    /// directories'.
    mem: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
    files: FileCache,
    job: Arc<str>,
    /// The lines of its file before each piece of the sequence.
    bases: Vec<u32>,
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
    /// XeTeX's native fonts (`Build::set_xetex`).
    xe: Option<Arc<crate::native::Fonts>>,
}

impl Given {
    fn new(
        dirs: Arc<Vec<PathBuf>>,
        job: Arc<str>,
        mem: Arc<FxMap<Arc<str>, Arc<[u8]>>>,
        bases: Vec<u32>,
    ) -> Given {
        Given {
            dirs,
            mem,
            files: Mutex::default(),
            job,
            bases,
            terminal: None,
            interaction: 0,
            interaction_option: None,
            limits: crate::print::Limits::TEX_LIVE,
            pdf_date: 0,
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
        let v = self.dirs.iter().enumerate().find_map(|(k, d)| {
            let b = std::fs::read(d.join(name)).ok()?;
            let shown = if k == 0 || d.is_relative() {
                format!("./{name}")
            } else {
                d.join(name).to_string_lossy().into_owned()
            };
            Some((Arc::from(shown), Arc::from(b)))
        });
        g.insert(Arc::from(name), v.clone());
        v
    }

    /// The lines of its file before piece `k`.
    fn base(&self, k: usize) -> u32 {
        self.bases.get(k).copied().unwrap_or(0)
    }

    /// What a read of `key` from piece `k` finds, if the build answers it.
    fn answer(&self, key: &Key, k: usize) -> Option<Val> {
        match key {
            Key::File(n) => Some(self.file(n).map_or(Val::Void, Val::Bytes)),
            Key::Job => Some(Val::Name(self.job.clone())),
            Key::LineBase => Some(Val::Int(self.base(k) as i32)),
            _ => None,
        }
    }
}

type Defs = FxMap<Key, Vec<(Pos, Val)>>;

/// A place in document order: (piece position, node). A piece's exports
/// are at `(k, END)`; node `j` of piece `k` reads what is defined before
/// `(k, j)`. (Elements put between a piece's nodes — an output routine's,
/// an `\input` file's — will define at `(k, j)`; none does yet.)
pub type Pos = (u32, u32);
/// The node of a piece's exports: after all its nodes.
pub const END: u32 = u32::MAX;

/// The document's elements in order, each with a stable id (a piece's is
/// its `PieceId`; an element between a piece's nodes — where the page
/// builder fired — has one by its piece and node), and where each is: what
/// an edge's ids mean, and which element a place in the names index is.
struct Sequence {
    /// Position -> the piece there.
    pieces: Vec<PieceId>,
    /// Piece id -> place (`(NONE, END)`: not in this build).
    pos: Vec<Pos>,
    /// The elements between nodes.
    virt: RwLock<Virt>,
}

/// The elements between a piece's nodes: their ids by (piece, node),
/// kept between builds, and their places in this one.
#[derive(Default)]
struct Virt {
    ids: FxMap<(PieceId, u32), PieceId>,
    at: FxMap<PieceId, Pos>,
    next: PieceId,
}

impl Sequence {
    /// The sequence of these pieces, in order, ids below `next`, with the
    /// last build's ids of elements between nodes.
    fn new(pieces: Vec<PieceId>, next: PieceId, ids: FxMap<(PieceId, u32), PieceId>) -> Sequence {
        let mut pos = vec![(NONE, END); next as usize];
        for (k, &id) in pieces.iter().enumerate() {
            pos[id as usize] = (k as u32, END);
        }
        let mut at = FxMap::default();
        for (&(p, node), &id) in &ids {
            if let Some(&(k, _)) = pos.get(p as usize).filter(|x| x.0 != NONE) {
                at.insert(id, (k, node));
            }
        }
        Sequence {
            pieces,
            pos,
            virt: RwLock::new(Virt { ids, at, next }),
        }
    }

    /// The piece at position `k`.
    fn piece(&self, k: usize) -> PieceId {
        self.pieces[k]
    }

    /// Where element `id` is, if it is in this build.
    fn pos_of(&self, id: PieceId) -> Option<Pos> {
        if let Some(p) = self.pos.get(id as usize)
            && p.0 != NONE
        {
            return Some(*p);
        }
        self.virt.read().expect("virt").at.get(&id).copied()
    }

    /// The element at place `p`.
    fn id_at(&self, p: Pos) -> PieceId {
        if p.1 == END {
            return self.pieces[p.0 as usize];
        }
        let key = (self.pieces[p.0 as usize], p.1);
        if let Some(&id) = self.virt.read().expect("virt").ids.get(&key) {
            return id;
        }
        let mut v = self.virt.write().expect("virt");
        if let Some(&id) = v.ids.get(&key) {
            return id;
        }
        let id = v.next;
        v.next += 1;
        v.ids.insert(key, id);
        v.at.insert(id, p);
        id
    }
}

/// Is an edge's source (`src`, where it was defined) at or before `m`?
fn src_at_or_before(seq: &Sequence, src: PieceId, m: Pos) -> bool {
    src == INITIAL || seq.pos_of(src).is_none_or(|p| p <= m)
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

/// Every installed run's exports: name -> (place, value), by place. Read by every thread at once, in shards; written only when a
/// run is installed, under the scheduler's lock.
struct Names {
    shards: Vec<RwLock<Defs>>,
    /// Per shard, a bit per group of keys some piece has defined in this
    /// build (set before the definition goes in, never cleared): a key
    /// whose bit is clear has only its initial value, found without the
    /// lock.
    defined: Vec<AtomicU64>,
    /// The same, for the names ever defined between a piece's nodes (by
    /// the page builder or an output routine: never cleared).
    inner: Vec<AtomicU64>,
    /// The build's control sequence names.
    cs: Arc<CsTable>,
    /// Whether any is (any bit of `inner`).
    any_inner: std::sync::atomic::AtomicBool,
    /// The names the build answers itself (files, the job, line bases).
    given: Given,
    /// The values before any definition are e-TeX's extended mode's.
    etex: bool,
    /// The format's definitions, at the place before the first piece
    /// (`Build::set_format`): what a name no piece defined before means.
    format: Option<Arc<FormatDefs>>,
}

/// A format's definitions by name, and its fonts' numbering
/// (`crate::format::Format`), as the names index holds them.
#[derive(Default)]
struct FormatDefs {
    defs: FxMap<Key, Val>,
    fonts: Vec<Arc<str>>,
}

impl Names {
    fn new(given: Given, etex: bool, cs: Arc<CsTable>, format: Option<Arc<FormatDefs>>) -> Names {
        Names {
            format,
            cs,
            shards: (0..SHARDS).map(|_| RwLock::default()).collect(),
            defined: (0..SHARDS).map(|_| AtomicU64::new(0)).collect(),
            inner: (0..SHARDS).map(|_| AtomicU64::new(0)).collect(),
            any_inner: std::sync::atomic::AtomicBool::new(false),
            given,
            etex,
        }
    }

    /// The shard of `key` and its bit there. (The hash only routes; the
    /// map's `==` decides.)
    /// Whether anything between a piece's nodes defines any name now.
    fn any_inner(&self) -> bool {
        self.any_inner.load(Ordering::Acquire)
    }

    fn route(key: &Key) -> (usize, u64) {
        let h = hash_of(key);
        ((h % SHARDS as u64) as usize, 1 << ((h >> 32) % 64))
    }

    /// The definition of `key` reaching place `at` (what is defined
    /// before it): its place (none: the initial value), its value, and the
    /// nodes of `at`'s piece it reaches, `lo..hi` (all of them, `0..END`,
    /// unless an element between the piece's nodes defines `key`).
    fn reaching(&self, key: &Key, at: Pos, font: &Arc<Font>) -> (Option<Pos>, Val, u32, u32) {
        if let Some(v) = self.given.answer(key, at.0 as usize) {
            return (None, v, 0, END);
        }
        let (s, bit) = Names::route(key);
        if self.defined[s].load(Ordering::Acquire) & bit == 0 {
            return (None, self.initial(key, font), 0, END);
        }
        let g = self.shards[s].read().expect("names");
        if let Some(v) = g.get(key) {
            let i = v.partition_point(|(p, _)| *p < at);
            let hi = match v.get(i) {
                Some(((k, j), _)) if *k == at.0 && *j != END => j + 1,
                _ => END,
            };
            if i > 0 {
                let (p, val) = &v[i - 1];
                let lo = if p.0 == at.0 { p.1 + 1 } else { 0 };
                return (Some(*p), val.clone(), lo, hi);
            }
            return (None, self.initial(key, font), 0, hi);
        }
        (None, self.initial(key, font), 0, END)
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
        let g = self.shards[s].read().expect("names");
        let v = g.get(key)?;
        let i = v.binary_search_by(|(p, _)| p.cmp(&at)).ok()?;
        Some(v[i].1.clone())
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
        let mut g = self.shards[s].write().expect("names");
        let v = g.entry(key.clone()).or_default();
        let i = v.partition_point(|(p, _)| *p < at);
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Unstarted,
    Running,
    Done,
}

struct PState {
    id: PieceId,
    text: Arc<[char]>,
    status: Status,
    /// Its installed run: its exports are in the names index, its reads
    /// are edges.
    run: Option<Arc<Run>>,
    /// The lines of each of the run's paragraphs, once broken.
    lines: Vec<Option<Lines>>,
    /// An edited piece's: the removed piece nearest its place, whose
    /// chain it runs from (kept wherever it still holds), with its lines
    /// and text.
    prev: Option<Prev>,
    /// Nodes of its run that read a definition since remade.
    dirty: Dirty,
    runs: u32,
    /// The piece whose definition answers each of its run's reads, as its
    /// edges say (aligned with `Run::reads`).
    srcs: Vec<PieceId>,
    /// Its part of the log, once its run and lines are final.
    log: Option<Arc<Frag>>,
    entry: Entry,
    expect: Expect,
    /// What its run says of the sequence.
    got: Expect,
    base: usize,
    post: Option<String>,
    /// Its reads suspend whatever the policy (a speculating run of it went
    /// on long on predictions nothing confirmed).
    suspend: bool,
    /// It ran in an edit: it keeps its nodes until this build (then it
    /// goes lean, `graph::Lean`).
    hot_until: u64,
    /// Its `\input`s are read in it ([`Build::inline`]).
    inline: bool,
    /// Where its run may stop ([`Spec::stops`]).
    stops: Arc<[usize]>,
    /// The state its run in progress was entered in.
    entered: Option<RState>,
    /// Every name it was marked for has the value its run read again
    /// ([`still_dirty`]): the run stands if its entry does.
    recheck: bool,
    /// Its run (`.0`) without its nodes (`.1`), to stand for it once the
    /// page builder has passed it: until then a definition the page
    /// builder makes between its nodes runs only the nodes after it again.
    lean_later: Option<(Arc<Run>, Arc<Run>)>,
}

/// A piece's dirty names, each from the first node of its run that may
/// read it otherwise (0: every reader; a definition made between its own
/// nodes, at node `i`, reaches its nodes from `i + 1`).
type Dirty = BTreeMap<Key, u32>;

/// `key` is dirty in `d` from node `from` on (the earliest wins).
fn dirty_from(d: &mut Dirty, key: Key, from: u32) {
    d.entry(key).and_modify(|f| *f = (*f).min(from)).or_insert(from);
}

/// An edited piece's predecessor: its run, lines and text.
type Prev = (Arc<Run>, Vec<Option<Lines>>, Arc<[char]>);

/// The scheduler's state.
struct State {
    /// The names each change to the names index changed, in order.
    names_log: Vec<Vec<Key>>,
    p: Vec<PState>,
    unstarted: BTreeSet<usize>,
    running: BTreeSet<usize>,
    /// Pieces before this one cannot change any more in this build: the
    /// page scan reads them.
    settled: usize,
    readers: Readers,
    break_queue: VecDeque<(usize, usize, Arc<Para>)>,
    scan: Option<Box<Scan>>,
    scan_next: usize,
    /// Items of piece `scan_next`'s material the scan does not feed (it
    /// resumed at a page that started there).
    scan_skip: u32,
    scan_done: bool,
    /// The pieces are being compared with what the last build's scan was
    /// fed at their place (its runs' paragraphs, `old_runs`), not fed: at
    /// the first that differs the scan resumes from the last page that
    /// started and was decided before it.
    matching: bool,
    old_runs: Vec<Option<Arc<Run>>>,
    /// Once known (`last_diff_known`): the last piece whose material
    /// differs from the last build's.
    last_diff: Option<usize>,
    last_diff_known: bool,
    pages: Vec<Page>,
    orgs: Vec<PageOrg>,
    bodies: Vec<Option<Arc<dvi::Body>>>,
    /// The fonts' numbers as loaded by the pieces before `numbered` (what
    /// a page shipped now numbers its fonts by; checked at the join).
    numbers: Arc<dvi::Numbers>,
    numbered: usize,
    /// The pieces before `log_next` have their part of the log, or are
    /// queued for it, or are making it (`logs_running`).
    log_next: usize,
    log_queue: VecDeque<usize>,
    logs_running: usize,
    unshipped: usize,
    ship_queue: VecDeque<usize>,
    old_pages: Vec<Option<(Page, Arc<dvi::Body>)>>,
    old_orgs: Arc<Vec<PageOrg>>,
    old_marks: Vec<(u32, u32)>,
    /// The page builder's marks before the scan's (kept), and after them
    /// (the last build's, where the scan met its pages again).
    marks_before: Vec<(u32, u32)>,
    marks_after: Vec<(u32, u32)>,
    rng: u64,
    stats: Stats,
    done: bool,
    /// For each piece, the pieces entered with its state.
    deps: Vec<Vec<usize>>,
    /// A run changed the sequence: no new task; the round ends once the
    /// tasks in flight are done.
    restructure: bool,
    /// A settled run says otherwise of the sequence than the layout
    /// assumed: once every piece has settled, the sequence is laid out
    /// again with all they say (the runs in progress are not stopped for
    /// it, and the pieces after it keep runs that still hold).
    restructure_due: bool,
    inflight: usize,
    /// The page builder's definitions installed this build (by piece id
    /// and node), the last build's not yet made again (by place), and its
    /// routines' runs this build.
    fold_defs: FoldDefs,
    predicted: Vec<(Pos, Vec<Key>)>,
    memo_new: Vec<Arc<Memo>>,
}

struct Shared {
    names: Names,
    /// The elements in order, fixed for the build.
    seq: Sequence,
    /// Bumped (under `park`, and `parked` notified) whenever a run is
    /// installed.
    generation: AtomicU64,
    park: Mutex<()>,
    parked: Condvar,
    /// How many changes the names index has had (`State::names_log`'s
    /// length, written under the scheduler's lock).
    names_gen: AtomicUsize,
    /// Each piece's status is `Done` (a copy of the scheduler's, written
    /// under its lock, read without it).
    done: Vec<AtomicBool>,
    /// A piece's run in progress is to stop: a name it read was defined
    /// again while it ran, or the sequence changes (read by the run between
    /// its nodes, [`PieceEnv`]'s `check`).
    cancel: Vec<AtomicBool>,
    /// Each piece's `\input`s are read in it ([`Build::inline`]).
    inline: Vec<bool>,
    /// Each piece's run is parked, waiting for an earlier piece.
    waiting: Vec<AtomicBool>,
    /// The round is ending ([`restructure_now`]): no run waits.
    restructuring: AtomicBool,
    /// Where each piece's run may stop ([`Spec::stops`]).
    stops: Vec<Arc<[usize]>>,
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    fonts: Arc<FontFiles>,
    opts: Options,
    /// The last build's routine runs, by their page's first box.
    memo_old: FxMap<usize, Vec<Arc<Memo>>>,
    /// A run may change the sequence (not in a round that must be the
    /// last).
    restructure_ok: bool,
    /// Settled pieces drop their nodes; which update this is.
    lean: bool,
    build_no: u64,
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

type ParaLines = (Arc<Para>, Lines);

enum Task {
    /// Run piece `.0` again from the nodes reading the names `.1` (each
    /// from its node on).
    Run(usize, Vec<(Key, u32)>),
    Break(usize, usize, Arc<Para>),
    Scan {
        scan: Box<Scan>,
        /// The position of `pieces[0]`, and how many items of its material
        /// to leave out.
        from: usize,
        skip: u32,
        /// Nothing differs from the last build after this piece: a restart
        /// point after it equal to one of the last build's pages' begins
        /// the last build's pages from there on.
        converge: Option<(usize, Arc<Vec<PageOrg>>)>,
        to: usize,
        pieces: Vec<(Arc<Run>, Vec<ParaLines>)>,
        finish: bool,
        /// Where the last build's page builder defined names it has not
        /// defined again yet, and the last piece's run (for the end).
        predicted: Vec<Pos>,
        end_run: Option<Arc<Run>>,
    },
    Ship(usize, Page, Arc<dvi::Numbers>),
    /// Piece `.0`'s part of the log, from its text, run and lines.
    Log(usize, Arc<[char]>, Arc<Run>, Vec<Option<Lines>>),
}

impl Build {
    /// A build whose documents can load `font` by its name, and the TFM
    /// files in TeX Live's directories.
    pub fn new(font: Arc<Font>) -> Build {
        let fonts = Arc::new(FontFiles::new(
            Self::given(&font),
            crate::tfm::default_dirs(),
        ));
        Build {
            font,
            fonts,
            pieces: Vec::new(),
            pages: Vec::new(),
            page_orgs: Vec::new(),
            marks: Vec::new(),
            next_id: 0,
            readers: FxMap::default(),
            src: String::new(),
            starts: Vec::new(),
            inputs: Arc::new(Vec::new()),
            job: Arc::from(crate::graph::JOBNAME),
            lean: true,
            builds: 0,
            texts: Vec::new(),
            files: Arc::default(),
            file_cuts: FxMap::default(),
            rests: FxMap::default(),
            recuts: FxMap::default(),
            joins: FxMap::default(),
            inline: Vec::new(),
            cut_lines: CUT_LINES,
            page_runs: None,
            carried: Stats::default(),
            fold: FoldKept::default(),
            virt: FxMap::default(),
            cs: Arc::new(CsTable::new()),
            terminal: None,
            interaction: 0,
            interaction_option: None,
            limits: crate::print::Limits::TEX_LIVE,
            format: None,
            pdf_date: 0,
            xe: None,
        }
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
        if self
            .files
            .get(name)
            .is_some_and(|t| **t == *text.as_bytes())
        {
            return;
        }
        Arc::make_mut(&mut self.files).insert(Arc::from(name), Arc::from(text.as_bytes()));
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

    /// The pieces of a file's text (the last cut's `Arc`s while the text
    /// is the same; the pieces of a changed text are found by their text).
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

    /// The pieces of `text` after a cut the cutter missed at `stop` (a
    /// line's start: the lines from there, cut as the main text is).
    fn recut(&mut self, text: &Arc<[char]>, stop: usize) -> Vec<Arc<[char]>> {
        let key = (text.as_ptr() as usize, stop);
        if let Some((t, v)) = self.recuts.get(&key)
            && Arc::ptr_eq(t, text)
        {
            return v.clone();
        }
        let src: String = text[stop.min(text.len())..]
            .iter()
            .map(|&c| if c == '\r' { '\n' } else { c })
            .collect();
        let v = cut_every(&src, self.cut_lines);
        self.recuts.insert(key, (text.clone(), v.clone()));
        v
    }

    /// The most texts of `rest` that `a` was laid out as one with (in
    /// one join or several, each joining the last's text), and the text
    /// all are, if the last build's piece of that text is still to be
    /// taken.
    fn known_join(&self, a: &Arc<[char]>, rest: &[Arc<[char]>], m: &Matcher, olds: &[Kept]) -> Option<Joined> {
        let recs = self.joins.get(&(a.as_ptr() as usize))?;
        let mut best: Option<Joined> = None;
        for (x, r, j, st) in recs {
            if !Arc::ptr_eq(x, a) || r.len() > rest.len() || !r.iter().zip(rest).all(|(p, q)| Arc::ptr_eq(p, q)) {
                continue;
            }
            let n = r.len();
            let cand = match self.known_join(j, &rest[n..], m, olds) {
                Some((n2, j2, st2)) => Some((n + n2, j2, st2)),
                None => m.has(olds, j).then(|| (n, j.clone(), st.clone())),
            };
            if let Some(c) = cand
                && best.as_ref().is_none_or(|b| c.0 > b.0)
            {
                best = Some(c);
            }
        }
        best
    }

    /// `a` and the texts `rest` after it as one text (the same `Arc` while
    /// they are), with where `a`'s own joined texts (`stops`) and each of
    /// `rest` begin in it: the places its run may stop at (CORE.md, cuts).
    fn joined(&mut self, a: &Arc<[char]>, stops: &[usize], rest: &[Arc<[char]>]) -> (Arc<[char]>, Arc<[usize]>) {
        let recs = self.joins.entry(a.as_ptr() as usize).or_default();
        recs.retain(|(x, _, _, _)| Arc::ptr_eq(x, a));
        if let Some((_, _, j, st)) = recs
            .iter()
            .find(|(_, r, _, _)| r.len() == rest.len() && r.iter().zip(rest).all(|(p, q)| Arc::ptr_eq(p, q)))
        {
            return (j.clone(), st.clone());
        }
        let mut v: Vec<char> = Vec::with_capacity(a.len() + rest.iter().map(|t| t.len()).sum::<usize>());
        let mut st = stops.to_vec();
        v.extend_from_slice(a);
        for t in rest {
            st.push(v.len());
            v.extend_from_slice(t);
        }
        let j: Arc<[char]> = Arc::from(v);
        let st: Arc<[usize]> = Arc::from(st);
        recs.push((a.clone(), rest.to_vec(), j.clone(), st.clone()));
        (j, st)
    }

    /// The rest of a line that called `\input` (from `stop` in `caller`).
    fn rest_text(&mut self, caller: &Arc<[char]>, stop: usize) -> Arc<[char]> {
        let key = (caller.as_ptr() as usize, stop);
        if let Some((c, r)) = self.rests.get(&key)
            && Arc::ptr_eq(c, caller)
        {
            return r.clone();
        }
        let r: Arc<[char]> = Arc::from(&caller[stop.min(caller.len())..]);
        self.rests.insert(key, (caller.clone(), r.clone()));
        r
    }

    /// The sequence: the main text's pieces, and after each piece whose
    /// last run read a file the file's pieces (up to one whose run ended
    /// it with `\endinput`) and the rest of the calling line — what the
    /// kept runs say; a run that says otherwise changes it.
    fn layout(&mut self, texts: &[Arc<[char]>], olds: &[Kept]) -> Vec<Spec> {
        let mut m = Matcher::new(olds);
        let mut out = Vec::with_capacity(texts.len());
        let mut base = 0;
        // (each job of a session from its own first piece, its lines
        // counted from its start; a job that ended is read no further)
        let mut i = 0;
        while i < texts.len() {
            let mut j = i + 1;
            while j < texts.len() && texts[j].first() != Some(&crate::graph::JOB) {
                j += 1;
            }
            if texts[i].first() == Some(&crate::graph::JOB) {
                base = 0;
            }
            let b = base;
            base += texts[i..j].iter().map(|t| count_lines(t)).sum::<usize>();
            self.place_list(&mut out, &mut m, olds, &texts[i..j], Entry::Start, None, b, 0);
            i = j;
        }
        out
    }

    /// Places the pieces `texts` of one text (the main text's, a file's,
    /// the rest of a re-cut piece's), each with what its run reads; the
    /// first entered with `first`, each other in the state the piece
    /// before it leaves (`ctx`: the file's calling piece). A run that does
    /// not stop where the next text begins reads on into it: the two are
    /// one piece (CORE.md, cuts). Returns the last piece placed.
    #[allow(clippy::too_many_arguments)]
    fn place_list(
        &mut self,
        out: &mut Vec<Spec>,
        m: &mut Matcher,
        olds: &[Kept],
        texts: &[Arc<[char]>],
        first: Entry,
        ctx: Option<usize>,
        base: usize,
        depth: u32,
    ) -> Option<usize> {
        let mut last = None;
        let mut b = base;
        let mut i = 0;
        while i < texts.len() {
            let entry = if i == 0 { first } else { Entry::After(out.len() - 1, ctx) };
            let k0 = out.len();
            let mut l = self.place(out, m, olds, &texts[i], entry, b, ctx, depth);
            b += count_lines(&texts[i]);
            i += 1;
            // (a run reading on past its text reads the texts after it with
            // it, as one, up to the first place one begins where it can
            // stop: the rest is cut again from there; or did when they were
            // laid out last — that piece is kept, its run never stops at
            // the first place to say so again. As many texts as make it
            // twice as long: a run reading on and on takes rounds as the
            // log of its length.)
            while i < texts.len() && !out[l].expect.endinput && !out[k0..].iter().any(|s| s.expect.ends) {
                let t = out[l].text.clone();
                let (w, joined, stops) = if let Some(k) = self.known_join(&t, &texts[i..], m, olds) {
                    k
                } else if out[l].expect.join {
                    let mut w = 1;
                    let mut len = texts[i].len();
                    while i + w < texts.len() && len < t.len() {
                        len += texts[i + w].len();
                        w += 1;
                    }
                    let own = out[l].stops.clone();
                    let (j, st) = self.joined(&t, &own, &texts[i..i + w]);
                    (w, j, st)
                } else {
                    break;
                };
                let (e, lb) = (out[l].entry, out[l].base);
                b += texts[i..i + w].iter().map(|x| count_lines(x)).sum::<usize>();
                i += w;
                out.truncate(l);
                l = self.place_with(out, m, olds, &joined, e, lb, ctx, depth, stops);
            }
            last = Some(l);
            // (the job ended: what follows is never read; the file ended)
            if out[k0..].iter().any(|s| s.expect.ends) || (ctx.is_some() && out[l].expect.endinput) {
                break;
            }
        }
        last
    }

    /// Places piece `text`, then what its run reads and the rest of its
    /// line; returns the index of the last piece placed.
    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        out: &mut Vec<Spec>,
        m: &mut Matcher,
        olds: &[Kept],
        text: &Arc<[char]>,
        entry: Entry,
        base: usize,
        ctx: Option<usize>,
        depth: u32,
    ) -> usize {
        self.place_with(out, m, olds, text, entry, base, ctx, depth, Arc::from([]))
    }

    /// [`Build::place`], the piece's run stopping at the first of `stops`
    /// where it can (a text joined from several).
    #[allow(clippy::too_many_arguments)]
    fn place_with(
        &mut self,
        out: &mut Vec<Spec>,
        m: &mut Matcher,
        olds: &[Kept],
        text: &Arc<[char]>,
        entry: Entry,
        base: usize,
        ctx: Option<usize>,
        depth: u32,
        stops: Arc<[usize]>,
    ) -> usize {
        let inline = self.inline.iter().any(|t| Arc::ptr_eq(t, text) || **t == **text);
        // (a run that read a file as pieces is not one of a piece whose
        // files are read in it)
        let found = m
            .find(olds, text)
            .filter(|&(i, _)| !inline || olds[i].got.input.is_none())
            .filter(|&(i, _)| old_entry_holds(out, olds, entry, i));
        let old = found.map(|x| x.0);
        let moved = found.is_some_and(|x| !x.1);
        let k = out.len();
        let expect = old.map(|i| olds[i].got.clone()).unwrap_or_default();
        out.push(Spec {
            text: text.clone(),
            entry,
            base,
            post: None,
            old,
            expect: expect.clone(),
            inline,
            stops,
            moved,
        });
        if expect.cut {
            // (the cutter missed a cut: the rest of the text is cut again
            // from there)
            let stop = expect.stop.min(text.len());
            let b = base + count_lines(&text[..stop]);
            let pieces = self.recut(text, stop);
            return self
                .place_list(out, m, olds, &pieces, Entry::After(k, ctx), ctx, b, depth)
                .unwrap_or(k);
        }
        let Some(f) = expect.input.clone() else {
            return k;
        };
        if depth > 64 {
            return k;
        }
        let pieces = self.file_pieces(&f.text);
        let last = self.place_list(out, m, olds, &pieces, Entry::File(k), Some(k), 0, depth + 1);
        let ended = out[k..].iter().any(|s| s.expect.ends);
        // (the file's end leaves what the rest of the calling line goes on
        // with — a paragraph, a group open — or its last command reads on
        // past it: the file is read in the calling piece)
        if !ended && last.is_some_and(|l| out[l].expect.join) {
            self.inline.push(text.clone());
            out.truncate(k);
            return self.place(out, m, olds, text, entry, base, ctx, depth);
        }
        let close = last.unwrap_or(k);
        // (the job ended in the file: its parenthesis closes at the end,
        // tex.web 1333, and the rest of the calling line is never read)
        let paren = if ended { " )" } else { ")" };
        match &mut out[close].post {
            Some(p) => p.push_str(paren),
            p @ None => *p = Some(format!("{RAW}{paren}")),
        }
        if ended {
            return close;
        }
        let stop = expect.stop.min(text.len());
        let rest = self.rest_text(text, stop);
        let rbase = base + count_lines(&text[..stop]);
        self.place(out, m, olds, &rest, Entry::Rest(k, last), rbase, ctx, depth)
    }

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
    pub fn set_pdf_date(&mut self, epoch: i64) {
        self.pdf_date = epoch;
    }

    /// The engine is XeTeX (`xetex -ini`, with e-TeX's extended mode as
    /// [`Options::etex`] says): its primitives and IniTeX values, Unicode
    /// input, native fonts from `fonts` (the installed fonts' index and
    /// the file finder, both given), XDV output and the PDF xdvipdfmx
    /// makes of it. Fixed for a `Build`'s life, given before the first
    /// update.
    pub fn set_xetex(&mut self, fonts: crate::native::Fonts) {
        self.xe = Some(Arc::new(fonts));
    }

    /// XeTeX's PDF of an XDV file this build wrote (`Output::dvi`): what
    /// `xdvipdfmx -q -E -o JOB.pdf` makes of it, in process
    /// (`native::xdv_to_pdf`), dated by [`Build::set_pdf_date`]. `None`
    /// when the engine is not XeTeX.
    pub fn xdv_pdf(&self, xdv: &[u8]) -> Option<Result<Vec<u8>, String>> {
        let xe = self.xe.as_ref()?;
        let name = format!("{}.pdf", self.job);
        Some(crate::native::xdv_to_pdf(xdv, xe.kpse(), &name, self.pdf_date))
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
    pub fn update(&mut self, src: &str, opts: Options) -> Output {
        let t0 = Instant::now();
        self.builds += 1;
        // (cut otherwise: every text is cut again)
        if opts.cut_lines != self.cut_lines {
            self.cut_lines = opts.cut_lines;
            self.texts.clear();
            self.src.clear();
            self.starts.clear();
            self.file_cuts.clear();
            self.recuts.clear();
        }
        let (texts, starts) = self.cut(src);
        self.src.clear();
        self.src.push_str(src);
        self.starts = starts;
        self.texts.clone_from(&texts);
        let time_cut = t0.elapsed();
        let saved = (
            self.pages.clone(),
            self.page_orgs.clone(),
            self.marks.clone(),
        );
        // (the pages to compare with are the last build's: so are the runs
        // whose material made them, whatever a round lays out)
        self.page_runs = Some(self.pieces.iter().map(|o| o.run.clone()).collect());
        let mut rounds = 0;
        loop {
            rounds += 1;
            let t_round = Instant::now();
            if let Some(mut out) = self.round(&texts, opts, t0, time_cut, rounds > 64) {
                let mut carried = std::mem::take(&mut self.carried);
                carried.round_walls.push(t_round.elapsed());
                out.stats.absorb(carried);
                out.stats.rounds = rounds;
                out.stats.wall = t0.elapsed();
                return out;
            }
            self.carried.round_walls.push(t_round.elapsed());
            // (a run changed the sequence: evaluated again, its runs kept;
            // the pages to compare with are the last build's)
            self.pages.clone_from(&saved.0);
            self.page_orgs.clone_from(&saved.1);
            self.marks.clone_from(&saved.2);
        }
    }

    /// One evaluation of the sequence the kept runs give: `None` if a run
    /// changed the sequence (the runs are kept for the next).
    #[allow(clippy::too_many_lines)]
    fn round(
        &mut self,
        texts: &[Arc<[char]>],
        opts: Options,
        t0: Instant,
        time_cut: Duration,
        last: bool,
    ) -> Option<Output> {
        let threads = opts.threads.max(1);
        let t_setup = Instant::now();
        let mut olds: Vec<Kept> = std::mem::take(&mut self.pieces);
        let old_runs: Vec<Option<Arc<Run>>> = olds.iter().map(|o| o.run.clone()).collect();
        let page_runs = self.page_runs.clone().unwrap_or_else(|| old_runs.clone());
        let specs = self.layout(texts, &olds);
        let mut used = vec![false; olds.len()];
        for sp in &specs {
            if let Some(i) = sp.old {
                used[i] = true;
            }
        }
        let mut old_pos = vec![NONE; self.next_id as usize];
        for (i, o) in olds.iter().enumerate() {
            old_pos[o.id as usize] = i as u32;
        }
        // (for each piece, the last build's piece of the nearest kept piece
        // before it and after it)
        let mut prev_old = vec![None; specs.len()];
        let mut next_old = vec![None; specs.len()];
        let mut seen = None;
        for (k, sp) in specs.iter().enumerate() {
            prev_old[k] = seen;
            seen = sp.old.or(seen);
        }
        seen = None;
        for (k, sp) in specs.iter().enumerate().rev() {
            next_old[k] = seen;
            seen = sp.old.or(seen);
        }
        let mut p: Vec<PState> = Vec::with_capacity(specs.len());
        for (k, sp) in specs.iter().enumerate() {
            let base = |text: Arc<[char]>, id: PieceId, status: Status| PState {
                id,
                text,
                status,
                run: None,
                lines: Vec::new(),
                prev: None,
                dirty: Dirty::new(),
                runs: 0,
                srcs: Vec::new(),
                log: None,
                entry: sp.entry,
                expect: sp.expect.clone(),
                got: sp.expect.clone(),
                base: sp.base,
                post: sp.post.clone(),
                suspend: false,
                hot_until: 0,
                inline: sp.inline,
                stops: sp.stops.clone(),
                entered: None,
                recheck: false,
                lean_later: None,
            };
            if let Some(i) = sp.old.filter(|&i| sp.moved || olds[i].run.is_none()) {
                // (its own place, not run yet — a restructure stopped the
                // round before it ran — or another place's run of equal
                // text, which it runs from: every node it keeps has its
                // reads looked at again here)
                let o = &mut olds[i];
                let mut ps = base(sp.text.clone(), o.id, Status::Unstarted);
                ps.srcs = std::mem::take(&mut o.srcs);
                if sp.moved
                    && let Some(r) = &o.run
                {
                    ps.prev = Some((r.clone(), std::mem::take(&mut o.lines), o.text.clone()));
                }
                p.push(ps);
            } else if let Some(i) = sp.old {
                // (each old piece is taken at most once: its lines and edge
                // sources move)
                let o = &mut olds[i];
                let mut ps = base(sp.text.clone(), o.id, Status::Done);
                ps.run.clone_from(&o.run);
                ps.lines = std::mem::take(&mut o.lines);
                ps.srcs = std::mem::take(&mut o.srcs);
                ps.log = o.log.take();
                ps.hot_until = o.hot_until;
                if let Some(d) = o.pending.take() {
                    ps.status = Status::Unstarted;
                    ps.dirty = d;
                }
                p.push(ps);
            } else {
                // (an edited piece: the unmatched old piece nearest its place,
                // whose chain it runs from)
                let (before, after) = (prev_old[k], next_old[k]);
                let near = [
                    before.map(|i| i + 1),
                    after.and_then(|i| i.checked_sub(1)),
                    (olds.len() == specs.len()).then_some(k),
                ]
                .into_iter()
                .flatten()
                .filter(|&i| i < olds.len() && !used[i] && olds[i].run.is_some())
                .max_by_key(|&i| {
                    olds[i]
                        .text
                        .iter()
                        .zip(sp.text.iter())
                        .take_while(|(a, b)| a == b)
                        .count()
                });
                let prev = near.map(|i| {
                    used[i] = true;
                    (
                        olds[i].run.clone().expect("a run"),
                        std::mem::take(&mut olds[i].lines),
                        olds[i].text.clone(),
                    )
                });
                // (it keeps that piece's identity: its old definitions stay
                // in the index as the prediction until it runs, so a reader
                // of them parks on it rather than reading past it)
                let id = if let Some(i) = near {
                    olds[i].id
                } else {
                    self.next_id += 1;
                    self.next_id - 1
                };
                let mut ps = base(sp.text.clone(), id, Status::Unstarted);
                ps.srcs = near
                    .map(|i| std::mem::take(&mut olds[i].srcs))
                    .unwrap_or_default();
                ps.prev = prev;
                p.push(ps);
            }
        }
        let n = p.len();
        let seq = Sequence::new(
            p.iter().map(|x| x.id).collect(),
            self.next_id,
            std::mem::take(&mut self.virt),
        );
        // (the names index: what the kept pieces define, at their places,
        // and what an edited piece's predecessor defined, as its prediction)
        let bases: Vec<u32> = p.iter().map(|x| x.base as u32).collect();
        let mut given = Given::new(self.inputs.clone(), self.job.clone(), self.files.clone(), bases);
        given.terminal.clone_from(&self.terminal);
        given.interaction = self.interaction;
        given.interaction_option = self.interaction_option;
        given.limits = self.limits;
        given.pdf_date = self.pdf_date;
        given.xe.clone_from(&self.xe);
        let names = Names::new(given, opts.etex, self.cs.clone(), self.format.clone());
        for (k, ps) in p.iter().enumerate() {
            if let Some(r) = ps.run.as_ref().or(ps.prev.as_ref().map(|(r, _, _)| r)) {
                for (key, val) in &r.exports {
                    names.set((k as u32, END), key, Some(val));
                }
            }
        }
        // (the page builder's definitions where it fired, in a piece kept as
        // it was: predictions until the page builder passes there again)
        let mut touched: Vec<Key> = Vec::new();
        let mut predicted: Vec<(Pos, Vec<Key>)> = Vec::new();
        let fold = self.fold.clone();
        for (pid, node, defs) in &fold.defs {
            let kept = seq.pos_of(*pid).filter(|&(k, _)| {
                let i = old_pos.get(*pid as usize).copied().unwrap_or(NONE);
                i != NONE
                    && p[k as usize]
                        .run
                        .as_ref()
                        .zip(old_runs.get(i as usize).and_then(Option::as_ref))
                        .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
            });
            match kept {
                Some((k, _)) => {
                    for (key, val) in defs {
                        names.set((k, *node), key, Some(val));
                    }
                    predicted.push(((k, *node), defs.iter().map(|d| d.0.clone()).collect()));
                }
                None => touched.extend(defs.iter().map(|d| d.0.clone())),
            }
        }
        predicted.sort_by_key(|x| x.0);
        for (pos, keys) in &predicted {
            let k = pos.0 as usize;
            if let Some(r) = p[k].run.as_ref().or(p[k].prev.as_ref().map(|(r, _, _)| r)) {
                for key in keys {
                    refresh_export(&names, &self.font, k, r, key);
                }
            }
        }
        // (the edges of removed pieces go; the names whose defining places
        // changed — defined by a removed or moved piece — have their
        // readers looked at: one reading another definition now is marked)
        let mut readers = std::mem::take(&mut self.readers);
        for o in &olds {
            let Some(r) = &o.run else { continue };
            let now = seq.pos_of(o.id).map(|p| p.0);
            if now != Some(old_pos[o.id as usize]) {
                touched.extend(r.exports.iter().map(|(k, _)| k.clone()));
            }
            if now.is_none() {
                for i in 0..r.read_count() {
                    if let Some(m) = readers.get_mut(r.read_at(i).0) {
                        m.remove(&o.id);
                    }
                }
            }
        }
        touched.sort();
        touched.dedup();
        let mut marks: Vec<(usize, Key)> = Vec::new();
        let mut moves: Vec<(usize, Key, PieceId)> = Vec::new();
        for key in &touched {
            let Some(m) = readers.get(key) else { continue };
            for (&rid, &(node, src)) in m {
                let Some(rpos) = seq.pos_of(rid) else {
                    continue;
                };
                let (def, ..) = names.reaching(key, (rpos.0, node), &self.font);
                let def_id = def.map_or(INITIAL, |d| seq.id_at(d));
                if src == def_id {
                    continue;
                }
                // (another definer giving every read the value it read: the
                // edge moves to it, nothing runs again)
                let r = rpos.0 as usize;
                if p[r].run.as_ref().is_some_and(|run| reads_still(&names, &self.font, run, r, key)) {
                    moves.push((r, key.clone(), def_id));
                } else {
                    marks.push((r, key.clone()));
                }
            }
        }
        for (r, key, src) in moves {
            move_edge(&mut readers, &mut p[r], &key, src);
        }
        // (the names the build answers itself — a file's bytes, the job's
        // name, the lines before a piece — read with a value they no longer
        // have: their readers are marked)
        for (key, m) in &readers {
            if !matches!(key, Key::File(_) | Key::Job | Key::LineBase) {
                continue;
            }
            for &rid in m.keys() {
                let Some(rpos) = seq.pos_of(rid).map(|x| x.0 as usize) else {
                    continue;
                };
                let Some(r) = &p[rpos].run else { continue };
                let now = names.given.answer(key, rpos);
                if r.values_read(key)
                    .iter()
                    .any(|x| now.as_ref() != Some(&x.2))
                {
                    marks.push((rpos, key.clone()));
                }
            }
        }
        // (a kept run entered otherwise than its piece is now — the piece
        // before it left another state at the cut — runs again)
        for k in 0..p.len() {
            if p[k].status == Status::Done
                && let Some(r) = &p[k].run
                && entry_state_in(&p, k).is_some_and(|e| r.entry() != e)
            {
                p[k].status = Status::Unstarted;
            }
        }
        let unstarted: BTreeSet<usize> = p
            .iter()
            .enumerate()
            .filter(|(_, x)| x.status == Status::Unstarted)
            .map(|(k, _)| k)
            .collect();
        let mut deps = vec![Vec::new(); n];
        for (k, ps) in p.iter().enumerate() {
            match ps.entry {
                Entry::Start => {}
                Entry::File(c) => deps[c].push(k),
                Entry::Rest(c, l) | Entry::After(c, l) => {
                    deps[c].push(k);
                    if let Some(l) = l {
                        deps[l].push(k);
                    }
                }
            }
        }
        let fancy = fold.fancy;
        let mut memo_old: FxMap<usize, Vec<Arc<Memo>>> = FxMap::default();
        for m in fold.memo {
            memo_old.entry(memo_key(&m.page)).or_default().push(m);
        }
        let mut state = State {
            names_log: Vec::new(),
            p,
            unstarted,
            running: BTreeSet::new(),
            settled: 0,
            readers,
            break_queue: VecDeque::new(),
            scan: Some(Box::default()),
            scan_next: 0,
            scan_skip: 0,
            scan_done: false,
            matching: !fancy && (!self.page_orgs.is_empty() || !self.marks.is_empty()),
            old_runs: page_runs,
            last_diff: None,
            last_diff_known: false,
            pages: Vec::new(),
            orgs: Vec::new(),
            marks_before: Vec::new(),
            marks_after: Vec::new(),
            bodies: Vec::new(),
            numbers: Arc::new(dvi::numbers([self.format_fonts()])),
            numbered: 0,
            log_next: 0,
            log_queue: VecDeque::new(),
            logs_running: 0,
            unshipped: 0,
            ship_queue: VecDeque::new(),
            old_pages: std::mem::take(&mut self.pages)
                .into_iter()
                .map(Some)
                .collect(),
            old_orgs: Arc::new(if fancy {
                Vec::new()
            } else {
                std::mem::take(&mut self.page_orgs)
            }),
            old_marks: std::mem::take(&mut self.marks),
            rng: match opts.order {
                Order::Random(s) => s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
                Order::Priority => 1,
            },
            stats: Stats {
                pieces: n,
                work: vec![0; threads],
                tasks: vec![0; threads],
                ..Stats::default()
            },
            done: false,
            deps,
            restructure: false,
            restructure_due: false,
            inflight: 0,
            fold_defs: Vec::new(),
            predicted,
            memo_new: Vec::new(),
        };
        for (k, key) in marks {
            mark(&mut state, k, key);
        }
        // (kept runs' paragraphs never broken go to the queue)
        for k in 0..n {
            if state.p[k].status == Status::Done && !lines_ready(&state, k) {
                let r = state.p[k].run.clone().expect("a run");
                let equal = equal_paras(Some(&r), &r);
                queue_breaks(&mut state, k, &equal);
            }
        }
        state.stats.time_cut = time_cut;
        state.stats.time_setup = t_setup.elapsed();
        let sh = Shared {
            names,
            seq,
            generation: AtomicU64::new(0),
            names_gen: AtomicUsize::new(0),
            park: Mutex::new(()),
            parked: Condvar::new(),
            done: state
                .p
                .iter()
                .map(|ps| AtomicBool::new(ps.status == Status::Done))
                .collect(),
            cancel: state.p.iter().map(|_| AtomicBool::new(false)).collect(),
            inline: state.p.iter().map(|ps| ps.inline).collect(),
            waiting: state.p.iter().map(|_| AtomicBool::new(false)).collect(),
            restructuring: AtomicBool::new(false),
            stops: state.p.iter().map(|ps| ps.stops.clone()).collect(),
            state: Mutex::new(state),
            cv: Condvar::new(),
            font: self.font.clone(),
            fonts: self.fonts.clone(),
            opts,
            restructure_ok: !last,
            lean: self.lean,
            build_no: self.builds,
            memo_old,
        };
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
        // (the last build's page builder definitions the scan did not reach
        // again — a fold that converged — stay as they were)
        let unreached: FoldDefs = {
            let mut g = lock(&sh);
            let mut out: FoldDefs = Vec::new();
            for (pos, keys) in std::mem::take(&mut g.predicted) {
                let pid = sh.seq.piece(pos.0 as usize);
                if g.fold_defs.iter().any(|x| x.0 == pid && x.1 == pos.1) {
                    continue;
                }
                let defs: Vec<(Key, Val)> = keys
                    .into_iter()
                    .filter_map(|k| sh.names.at(&k, pos).map(|v| (k, v)))
                    .collect();
                out.push((pid, pos.1, defs));
            }
            out
        };
        let virt = std::mem::take(&mut *sh.seq.virt.write().expect("virt"));
        self.virt = virt.ids;
        self.next_id = self.next_id.max(virt.next);
        let mut st = sh.state.into_inner().expect("state");
        if st.restructure {
            // (the sequence changed: the runs are kept, with what each still
            // has to read again)
            self.pieces =
                st.p.into_iter()
                    .map(|ps| {
                        let pending =
                            (ps.status != Status::Done || !ps.dirty.is_empty()).then_some(ps.dirty);
                        match (ps.run, ps.prev) {
                            (Some(run), _) => Kept {
                                id: ps.id,
                                text: ps.text,
                                run: Some(run),
                                lines: ps.lines,
                                srcs: ps.srcs,
                                log: ps.log,
                                pending,
                                got: ps.got,
                                hot_until: ps.hot_until,
                            },
                            (None, Some((run, lines, text))) => Kept {
                                id: ps.id,
                                text,
                                got: Expect::of(&run),
                                run: Some(run),
                                lines,
                                srcs: ps.srcs,
                                log: None,
                                pending: None,
                                hot_until: ps.hot_until,
                            },
                            (None, None) => Kept {
                                id: ps.id,
                                text: ps.text,
                                run: None,
                                lines: Vec::new(),
                                srcs: ps.srcs,
                                log: None,
                                pending: None,
                                got: Expect::default(),
                                hot_until: 0,
                            },
                        }
                    })
                    .collect();
            self.readers = st.readers;
            self.carried.absorb(st.stats);
            return None;
        }
        let t_join = Instant::now();
        let mut bodies: Vec<Arc<dvi::Body>> = st
            .bodies
            .iter()
            .map(|b| b.clone().expect("every page shipped"))
            .collect();
        let numbers = dvi::numbers(
            std::iter::once(self.format_fonts())
                .chain(st.p.iter().filter_map(|ps| ps.run.as_ref().map(|r| &r.fonts[..]))),
        );
        let mut errors = Vec::new();
        // (PDF mode, from the first page's `\pdfoutput`: the PDF file is
        // the fold over the pages, as the DVI file is)
        let (out, pdf, again) = if st.pages.first().is_some_and(|p| p.ship.pdf.is_some()) {
            let live = live_pieces(&st.p);
            let last = live.iter().rposition(|&l| l).unwrap_or(0);
            let w = pdf_file(&sh.names, &sh.font, last, &numbers, &st.pages, &mut bodies);
            errors.extend(w.errors.into_iter().map(|e| format!("pdf: {e}")));
            (Vec::new(), w.bytes, w.again)
        } else {
            // (XeTeX: the XDV file, its id byte and comment XeTeX's)
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
        // (a piece whose part was not made in the build: none, unless its
        // lines were not all broken)
        for ps in &mut st.p {
            let run = ps.run.as_ref().expect("every piece ran");
            if !ps.log.as_ref().is_some_and(|f| f.holds(run, &ps.lines)) {
                ps.log = Some(Arc::new(logs::fragment(&ps.text, run, &ps.lines)));
            }
        }
        let call_logs: Vec<(u32, Vec<CallLog>)> = st
            .scan
            .as_ref()
            .map(|s| s.call_logs.clone())
            .unwrap_or_default();
        // (the piece holding `\end` ends the document: the pieces after it
        // are not in the log; CORE.md: the core is to not run them)
        // (and in a session, the pieces after it up to the next job's start)
        let live = live_pieces(&st.p);
        let (log, writes) = {
            let idx: Vec<usize> = (0..st.p.len()).filter(|&k| live[k]).collect();
            let frags: Vec<&Frag> = idx.iter().map(|&k| &**st.p[k].log.as_ref().expect("a part")).collect();
            // (each page the page builder shipped does its whatsits where
            // its call stands: its `\write`s expanded with the definitions
            // reaching there)
            let end = idx.len().checked_sub(1).map_or((0, 0), |j| {
                (j, st.p[idx[j]].run.as_ref().map_or(0, |r| r.nodes_len()))
            });
            let places = logs::page_places(&frags, &marks, st.pages.len(), end);
            let mut page_io = Vec::with_capacity(st.pages.len());
            for (j, (pg, at)) in st.pages.iter().zip(&places).enumerate() {
                let io = match at.map(|(x, i)| (idx.get(x).copied(), i)) {
                    // (only a page whose body met a whatsit has events)
                    Some((Some(k), i)) => match st.p[k].run.as_ref() {
                        Some(r) if bodies[j].io && pg.builder => {
                            let mut env = JoinEnv {
                                names: &sh.names,
                                seq: &sh.seq,
                                font: &sh.font,
                                fonts: &sh.fonts,
                                k,
                                cut: false,
                                split: false,
                                stop: usize::MAX,
                            };
                            let r = regen(&mut env, &st.p[k].text, r);
                            crate::graph::page_io(&r.chain, i, &mut env, &pg.b, &mut errors)
                        }
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                page_io.push(io);
            }
            let parts: Vec<logs::Part<'_>> = idx
                .iter()
                .map(|&k| &st.p[k])
                .map(|ps| logs::Part {
                    frag: ps.log.as_ref().expect("a part"),
                    base: ps.base,
                    post: ps.post.as_deref(),
                })
                .collect();
            logs::replay(
                logs::gather(&parts, &marks, &call_logs, &st.pages, &page_io),
                opts.etex,
            )
        };
        for (k, ps) in st.p.iter().enumerate() {
            let r = ps.run.as_ref().expect("every piece ran");
            if live[k] {
                for e in &r.errors {
                    errors.push(format!("piece {k}: {e}"));
                }
            }
            if ps.runs == 0 {
                st.stats.kept += 1;
            }
            st.stats.commands += r.nodes_len() as u64;
            if opts.dump {
                let mut env = JoinEnv {
                    names: &sh.names,
                    seq: &sh.seq,
                    font: &sh.font,
                    fonts: &sh.fonts,
                    k,
                    cut: false,
                    split: false,
                    stop: usize::MAX,
                };
                let r = regen(&mut env, &ps.text, r);
                dump_piece(&mut dump, k, ps, &r, &sh.names.cs);
            }
        }
        if let Some(sc) = &st.scan {
            for e in &sc.errors {
                errors.push(format!("page builder: {e}"));
            }
        }
        if last && st.restructure {
            errors.push("the sequence of pieces did not settle (\\input)".into());
        }
        if opts.dump {
            for (i, pg) in st.pages.iter().enumerate() {
                let _ = writeln!(dump, "page {}: {} items", i + 1, pg.b.list.len());
            }
        }
        let jobs = job_of(st.p.iter().map(|ps| ps.text.first() == Some(&crate::graph::JOB)));
        let mut job_pages = vec![0usize; jobs.last().map_or(1, |j| j + 1)];
        for o in &st.orgs {
            let j = jobs.get(o.call.0 as usize).copied().unwrap_or(0);
            job_pages[j] += 1;
        }
        st.stats.time_join = t_join.elapsed();
        st.stats.pages = st.pages.len();
        st.stats.wall = t0.elapsed();
        let mut fold_defs = std::mem::take(&mut st.fold_defs);
        fold_defs.extend(unreached);
        self.fold = FoldKept {
            fancy: st.scan.as_ref().is_some_and(|s| s.fancy),
            defs: fold_defs,
            memo: std::mem::take(&mut st.memo_new),
        };
        // (a piece quiet since it last ran in an edit drops its nodes now)
        let build_no = self.builds;
        let lean = self.lean;
        self.pieces =
            st.p.into_iter()
                .map(|ps| {
                    let quiet = lean && ps.hot_until < build_no;
                    let (run, log) = match ps.run {
                        Some(r) if quiet && r.lean.is_none() => {
                            (Some(Arc::new(r.leaned(&ps.text))), None)
                        }
                        r => (r, ps.log),
                    };
                    Kept {
                        id: ps.id,
                        text: ps.text,
                        run,
                        lines: ps.lines,
                        srcs: ps.srcs,
                        log,
                        pending: None,
                        got: ps.got,
                        hot_until: ps.hot_until,
                    }
                })
                .collect();
        self.readers = st.readers;
        // (the edges are kept until the next build: no spare capacity)
        for m in self.readers.values_mut() {
            m.shrink_to_fit();
        }
        self.pages = st.pages.into_iter().zip(bodies).collect();
        self.page_orgs = st.orgs;
        self.marks = marks;
        Some(Output {
            dvi: out,
            pdf,
            stats: st.stats,
            errors,
            dump,
            log,
            writes,
            job_pages,
        })
    }
}

/// The PDF file of the pages (`pdf::fold`): the pages' streams (in
/// `bodies`, made again where the fold says), what the job's end reads
/// (the definitions reaching the end of piece `last`, the place before
/// the next), the files the fold
/// reads (given, in the input directories, then TeX Live's).
/// The files the PDF fold reads: given, in the input directories, then
/// TeX Live's.
struct PdfFiles<'a>(&'a Given);

impl crate::pdf::fold::Files for PdfFiles<'_> {
    fn get(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        self.0.found(name).or_else(|| crate::tfm::pdf_file(name))
    }
}

fn pdf_file(names: &Names, font: &Arc<Font>, last: usize, numbers: &dvi::Numbers, pages: &[Page], bodies: &mut [Arc<dvi::Body>]) -> crate::pdf::fold::Written {
    use crate::val::{IntPar, TokPar};
    let at = (last as u32 + 1, 0);
    let read = |key: &Key| names.reaching(key, at, font).1;
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
    };
    let mut pdf_bodies: Vec<Arc<crate::pdf::content::PageBody>> =
        bodies.iter().map(|b| b.pdf.clone().unwrap_or_else(|| Arc::new(crate::pdf::content::empty()))).collect();
    let w = crate::pdf::fold::assemble(pages, &mut pdf_bodies, numbers, &end, &mut PdfFiles(&names.given));
    for (b, p) in bodies.iter_mut().zip(pdf_bodies) {
        if !b.pdf.as_ref().is_some_and(|x| Arc::ptr_eq(x, &p)) {
            let mut nb = dvi::Body { bytes: Vec::new(), fonts: Vec::new(), fixed: None, max_push: 0, io: p.io, pdf: None };
            nb.pdf = Some(p);
            *b = Arc::new(nb);
        }
    }
    w
}

/// The join's reads (a page's `\write`s expanded where it was shipped):
/// every piece has run, nothing parks.
struct JoinEnv<'a> {
    names: &'a Names,
    seq: &'a Sequence,
    font: &'a Arc<Font>,
    fonts: &'a Arc<FontFiles>,
    k: usize,
    /// The run ran up to a cut the cutter missed: so does its run again.
    cut: bool,
    /// The run ended at an `\input` whose file is pieces: so does its run
    /// again.
    split: bool,
    /// The run stopped here, where a text it was joined from begins: so
    /// does its run again.
    stop: usize,
}

/// A lean piece's run with its nodes again (`graph::Lean`): run again,
/// reading only, over the names index as it is — for the join's page
/// events and the dump.
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

    // (run again as it ran: up to its cut)
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
        let (def, val, lo, hi) = self.names.reaching(key, (self.k as u32, node), self.font);
        (
            val,
            def.map_or(Src::Initial, |d| Src::Piece(self.seq.id_at(d))),
            lo,
            hi,
        )
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

/// Which pieces are the document's: in each job, those up to the first
/// holding `\end` or `\dump` (TeX stops reading there; CORE.md: the core
/// is to not run the others).
fn live_pieces(p: &[PState]) -> Vec<bool> {
    let mut live = vec![true; p.len()];
    let mut ended = false;
    for (k, ps) in p.iter().enumerate() {
        if ps.text.first() == Some(&crate::graph::JOB) {
            ended = false;
        }
        live[k] = !ended;
        if ps.run.as_ref().is_some_and(|r| crate::graph::ends_document(r)) {
            ended = true;
        }
    }
    live
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

fn dump_piece(out: &mut String, k: usize, ps: &PState, r: &Run, cs: &CsTable) {
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
        ps.entry,
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
        if let Some(phi) = n.phi() {
            let _ = write!(out, " | φ test={}", phi.test);
            for (key, after, before) in &phi.keys {
                let _ = write!(out, " | {} = φ({} | {})", Shown(key, cs), Shown(after, cs), Shown(before, cs));
            }
        }
        for d in n.defs() {
            let _ = write!(
                out,
                " | defines {}{} = {}",
                if d.global { "global " } else { "" },
                Shown(&d.key, cs),
                Shown(&d.val, cs)
            );
        }
        out.push('\n');
    }
}

fn lock(sh: &Shared) -> MutexGuard<'_, State> {
    sh.state.lock().expect("state")
}

fn worker(sh: &Shared, id: usize) {
    let mut g = lock(sh);
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
            g = sh.cv.wait(g).expect("state");
            continue;
        };
        g.stats.tasks[id] += 1;
        g.inflight += 1;
        drop(g);
        let t = Instant::now();
        let (work, kind) = execute(sh, task);
        let dt = t.elapsed();
        g = lock(sh);
        g.inflight -= 1;
        if g.restructure && g.inflight == 0 {
            sh.cv.notify_all();
        }
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

fn finished(g: &State) -> bool {
    if g.restructure {
        return g.inflight == 0;
    }
    g.settled == g.p.len()
        && g.scan_done
        && g.unshipped == 0
        && g.log_queue.is_empty()
        && g.logs_running == 0
}

/// Whether piece `r` read `key` only for whether it is `\outer`
/// (`graph::Field::Outer`), and it still is or still is not: the piece
/// need not run again for it.
fn outer_still(g: &State, sh: &Shared, r: usize, key: &Key) -> bool {
    let Some(vals) = g.p[r].run.as_ref().and_then(|run| run.outer_reads_only(key)) else {
        return false;
    };
    vals.iter().all(|(_, node, v)| v.seen_as(&sh.names.reaching(key, (r as u32, *node), &sh.font).1))
}

/// Piece `k` read a definition of `key` since remade: it runs again from
/// the first of its nodes reading the name (every node reading it is
/// looked at again). Returns whether it was done.
fn mark(g: &mut State, k: usize, key: Key) -> bool {
    mark_from(g, k, key, 0)
}

/// [`mark`], its nodes from `from` on only (a definition between its own
/// nodes reaches those after it).
fn mark_from(g: &mut State, k: usize, key: Key, from: u32) -> bool {
    // (a page builder's definition can reach a piece the scan has not read
    // yet but that had settled)
    if k < g.settled {
        g.settled = k;
    }
    let ps = &mut g.p[k];
    dirty_from(&mut ps.dirty, key, from);
    if ps.status == Status::Done {
        ps.status = Status::Unstarted;
        g.unstarted.insert(k);
        g.stats.reruns += 1;
        return true;
    }
    false
}

/// Piece `k`'s export of `key` as the names index holds it at `(k, END)`:
/// none when a definition made between its nodes after its own last
/// definition of `key` (an output routine's, where the page builder fired)
/// is the one reaching its end.
fn export_of(names: &Names, font: &Arc<Font>, k: usize, run: &Run, key: &Key) -> Option<Val> {
    let i = run.exports.binary_search_by(|(x, _)| x.cmp(key)).ok()?;
    if names.has_inner(key) {
        let last = run.resolve(key, run.nodes_len()).map(|(d, _)| d);
        if let (Some(p), ..) = names.reaching(key, (k as u32, END), font)
            && p.0 == k as u32
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
fn refresh_export(names: &Names, font: &Arc<Font>, k: usize, run: &Run, key: &Key) -> bool {
    let v = export_of(names, font, k, run, key);
    if names.at(key, (k as u32, END)) == v {
        return false;
    }
    names.set((k as u32, END), key, v.as_ref());
    true
}

/// The names whose exported values differ between two runs of a piece.
fn changed_names(old: Option<&Run>, new: &Run) -> Vec<Key> {
    let empty = Vec::new();
    let a = old.map_or(&empty, |r| &r.exports);
    let b = &new.exports;
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    // (exports are sorted by name)
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

/// Piece `m`'s definitions of `keys` changed: the nodes after it that
/// read those names from before `m` (or from `m`), by the edges. A
/// settled reader whose every read of the name has the value it read (a
/// new definer of the same value) is not one: its edge moves to the
/// definer reaching it now.
fn reached(g: &mut State, sh: &Shared, m: Pos, keys: &[Key]) -> Vec<(usize, Key)> {
    let mut out = Vec::new();
    let mut moves: Vec<(usize, Key, PieceId)> = Vec::new();
    for key in keys {
        let Some(rm) = g.readers.get(key) else {
            continue;
        };
        for (&rid, &(node, src)) in rm {
            let Some(rpos) = sh.seq.pos_of(rid) else {
                continue;
            };
            // (a place between a piece's nodes reaches the piece's nodes
            // after it: the piece's first reader may be before it, a later
            // one after)
            // (piece m.0's own nodes after m: `changed_at`, by its nodes'
            // reads; its edge is its first reader's, which may be before m)
            if (rpos.0, node) <= m || (m.1 != END && rpos.0 == m.0) {
                continue;
            }
            if !src_at_or_before(&sh.seq, src, m) {
                continue;
            }
            let r = rpos.0 as usize;
            let ps = &g.p[r];
            if ps.status == Status::Done
                && let Some(run) = &ps.run
                && reads_still(&sh.names, &sh.font, run, r, key)
            {
                let (def, ..) = sh.names.reaching(key, (rpos.0, node), &sh.font);
                let def_id = def.map_or(INITIAL, |d| sh.seq.id_at(d));
                if def_id != src {
                    moves.push((r, key.clone(), def_id));
                }
                continue;
            }
            out.push((r, key.clone()));
        }
    }
    for (r, key, src) in moves {
        move_edge(&mut g.readers, &mut g.p[r], &key, src);
    }
    out
}

/// Whether piece `r`'s `run` read `key` with the values reaching it now:
/// each of its reads of the name, at its node.
fn reads_still(names: &Names, font: &Arc<Font>, run: &Run, r: usize, key: &Key) -> bool {
    let vals = run.values_read(key);
    !vals.is_empty()
        && vals
            .iter()
            .all(|(_, node, v)| v.seen_as(&names.reaching(key, (r as u32, *node), font).1))
}

/// Piece `k`'s dirty names as it starts to run: those its last run read
/// with values that no longer reach it. A name marked when its definer
/// changed whose reads have their values again (it moved to a new piece
/// that has run since, with the same value) is not: its edge moves to the
/// definer reaching it now. (Taken from the piece.)
fn still_dirty(g: &mut State, sh: &Shared, k: usize) -> Vec<(Key, u32)> {
    let dirty = std::mem::take(&mut g.p[k].dirty);
    let Some(run) = g.p[k].run.clone() else {
        return dirty.into_iter().collect();
    };
    let any = !dirty.is_empty();
    let mut out = Vec::new();
    for (key, from) in dirty {
        // (a name defined between some piece's nodes — the page builder's
        // — may have marked this piece from one of its own fires: its
        // nodes after the fire read another value though the first reader
        // (the value recorded) does not, so it stays dirty)
        if from > 0 || sh.names.has_inner(&key) || !reads_still(&sh.names, &sh.font, &run, k, &key) {
            out.push((key, from));
            continue;
        }
        let node = run.values_read(&key)[0].1;
        let (def, ..) = sh.names.reaching(&key, (k as u32, node), &sh.font);
        let src = def.map_or(INITIAL, |d| sh.seq.id_at(d));
        move_edge(&mut g.readers, &mut g.p[k], &key, src);
    }
    g.p[k].recheck = any && out.is_empty();
    out
}

/// Piece `k`, chosen to run, whose every marked name reads its value
/// again ([`PState::recheck`]): its run is kept, as at the round's start,
/// if it is entered as it was. Whether it was.
fn keep_again(g: &mut State, sh: &Shared, k: usize) -> bool {
    let ps = &g.p[k];
    let Some(run) = ps.run.clone() else { return false };
    // (entered on a state assumed at its cut: not known yet)
    let assumed = matches!(ps.entry, Entry::After(q, _) if !(g.p[q].status == Status::Done && g.p[q].dirty.is_empty()));
    if assumed || !entry_holds(g, k, &run) {
        return false;
    }
    g.running.remove(&k);
    g.p[k].status = Status::Done;
    g.p[k].entered = None;
    g.stats.kept += 1;
    sh.done[k].store(true, Ordering::Release);
    settle(g, sh);
    true
}

/// Piece `ps`'s edge for `key` comes from `src` now (its reads unchanged).
fn move_edge(readers: &mut Readers, ps: &mut PState, key: &Key, src: PieceId) {
    if let Some(e) = readers.get_mut(key).and_then(|m| m.get_mut(&ps.id)) {
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
///
/// `equal[i]` lists the old paragraphs equal to paragraph `i`
/// ([`equal_paras`], found before taking the lock), its own place first.
fn queue_breaks(g: &mut State, k: usize, equal: &[Vec<u32>]) {
    let run = g.p[k].run.clone().expect("a run");
    let old_lines = std::mem::take(&mut g.p[k].lines);
    let prev_lines = match g.p[k].prev.take() {
        Some((_, l, _)) => l,
        None => old_lines,
    };
    let mut lines = vec![None; run.paras.len()];
    for (i, para) in run.paras.iter().enumerate() {
        let reuse = equal[i]
            .iter()
            .find_map(|&j| prev_lines.get(j as usize).cloned().flatten());
        match reuse {
            Some(l) => {
                lines[i] = Some(l);
                g.stats.breaks_reused += 1;
            }
            None => g.break_queue.push_back((k, i, para.clone())),
        }
    }
    g.p[k].lines = lines;
}

/// For each paragraph of `run`, the paragraphs of `old` equal to it (by
/// identity, then equality), its own place first.
fn equal_paras(old: Option<&Run>, run: &Run) -> Vec<Vec<u32>> {
    run.paras
        .iter()
        .enumerate()
        .map(|(i, para)| {
            let Some(o) = old else { return Vec::new() };
            let eq = |j: usize| {
                o.paras
                    .get(j)
                    .is_some_and(|q| Arc::ptr_eq(q, para) || **q == **para)
            };
            let mut v: Vec<u32> = Vec::new();
            if eq(i) {
                v.push(i as u32);
            }
            v.extend(
                (0..o.paras.len())
                    .filter(|&j| j != i && eq(j))
                    .map(|j| j as u32),
            );
            v
        })
        .collect()
}

/// Whether the pieces piece `k` is entered from have run (installed and
/// valid; speculating: run at all). A piece after a cut runs on the state
/// assumed there until the piece before it has run and left another.
fn entry_ready(g: &State, k: usize, speculate: bool) -> bool {
    let need = |c: usize| {
        let ps = &g.p[c];
        if speculate {
            ps.run.is_some()
        } else {
            ps.status == Status::Done && ps.dirty.is_empty()
        }
    };
    match g.p[k].entry {
        Entry::Start => true,
        Entry::File(c) => need(c),
        Entry::Rest(c, l) => need(c) && l.is_none_or(need),
        Entry::After(q, c) => {
            let Some(assumed) = assumed_entry(&g.p, c) else {
                return false;
            };
            // (a piece whose run on the assumption was given up waits for
            // the piece before it)
            if g.p[k].suspend {
                return g.p[q].status == Status::Done && g.p[q].dirty.is_empty();
            }
            match &g.p[q].run {
                // (the piece before reads on into this one: its text is not
                // a place a piece begins; the sequence is laid out again —
                // unless the layout knew and could not join them)
                Some(_) if g.p[q].got.join && !g.p[q].expect.join => false,
                Some(r) => {
                    let out = r.out();
                    !out.enterable() || out.halted() || RState::entered_after(&out) == assumed || need(q)
                }
                None => true,
            }
        }
    }
}

/// Whether the last build's run `i` was entered in the state the layout
/// so far says its piece is (from the last build's runs of the pieces it
/// is entered from; unknown where one has none): a run entered otherwise
/// says nothing of the sequence (it began in the middle of a command, or
/// in another file's place).
fn old_entry_holds(out: &[Spec], olds: &[Kept], entry: Entry, i: usize) -> bool {
    let Some(run) = &olds[i].run else { return true };
    let prev = |c: usize| out[c].old.and_then(|j| olds[j].run.as_ref());
    let state = match entry {
        Entry::Start => Some(RState::start()),
        Entry::File(c) => prev(c).map(|r| RState::file_start(&r.out())),
        Entry::Rest(c, l) => match (prev(c), l.map(prev)) {
            (Some(c), None) => Some(RState::rest(None, &c.out())),
            (Some(c), Some(Some(l))) => Some(RState::rest(Some(&l.out()), &c.out())),
            _ => None,
        },
        Entry::After(q, c) => {
            let assumed = match c {
                None => Some(RState::start()),
                Some(c) => prev(c).map(|r| RState::entered_after(&RState::file_start(&r.out()))),
            };
            match prev(q) {
                // (the piece before reads on into it)
                Some(_) if out[q].expect.join => return false,
                Some(r) if r.out().enterable() && !r.out().halted() => Some(RState::entered_after(&r.out())),
                _ => assumed,
            }
        }
    };
    state.is_none_or(|e| run.entry() == e)
}

/// The state a piece after a cut is assumed to be entered in: the
/// canonical start, or the start of the file piece `c` calls (none: `c`
/// has not run).
fn assumed_entry(p: &[PState], c: Option<usize>) -> Option<RState> {
    match c {
        None => Some(RState::start()),
        Some(c) => p[c].run.as_ref().map(|r| RState::entered_after(&RState::file_start(&r.out()))),
    }
}

/// The state piece `k` is entered in now (none: a piece it is entered
/// from has not run).
fn entry_state(g: &State, k: usize) -> Option<RState> {
    entry_state_in(&g.p, k)
}

fn entry_state_in(p: &[PState], k: usize) -> Option<RState> {
    let out = |c: usize| p[c].run.as_ref().map(|r| r.out());
    Some(match p[k].entry {
        Entry::Start => RState::start(),
        Entry::File(c) => RState::file_start(&out(c)?),
        Entry::Rest(c, l) => {
            let caller = out(c)?;
            let file = match l {
                Some(l) => Some(out(l)?),
                None => None,
            };
            RState::rest(file.as_ref(), &caller)
        }
        // (a state no piece is entered in: the two are laid out as one
        // piece, the assumption stands until then)
        // (after a piece that stopped at a fatal error nothing is read: the
        // pieces after it read nothing of its state — the log ends there)
        Entry::After(q, c) => match out(q) {
            Some(o) if o.enterable() && !o.halted() => RState::entered_after(&o),
            _ => assumed_entry(p, c)?,
        },
    })
}

/// Whether a run was entered in the state piece `k` is entered in now.
fn entry_holds(g: &State, k: usize, r: &Run) -> bool {
    entry_state(g, k).is_some_and(|e| r.entry() == e)
}

fn lines_ready(g: &State, k: usize) -> bool {
    let ps = &g.p[k];
    ps.run
        .as_ref()
        .is_some_and(|r| ps.lines.len() == r.paras.len() && ps.lines.iter().all(Option::is_some))
}

fn rand(g: &mut State, n: usize) -> usize {
    let mut x = g.rng;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    g.rng = x;
    (x % n as u64) as usize
}

/// Compares the settled pieces with what the last build's scan was fed at
/// their place, without feeding them; at the first that differs (or at the
/// end, if the number of pieces differs) the scan resumes.
fn advance_match(g: &mut State) {
    let n = g.p.len();
    while g.matching && g.scan_next < g.settled {
        let k = g.scan_next;
        if same_material(g, k) {
            g.scan_next += 1;
        } else {
            resume(g, k);
        }
    }
    if g.matching && g.scan_next == n && g.settled == n {
        if g.old_runs.len() == n {
            // (the same material throughout: the same pages)
            for i in 0..g.old_pages.len() {
                let (pg, b) = g.old_pages[i].take().expect("an old page");
                g.pages.push(pg);
                g.bodies.push(Some(b));
                g.stats.pages_reused += 1;
            }
            g.orgs = g.old_orgs.to_vec();
            g.marks_before = std::mem::take(&mut g.old_marks);
            g.scan_done = true;
            g.matching = false;
        } else {
            resume(g, n);
        }
    }
}

/// Whether piece `k`'s material is what the last build's scan was fed at
/// its place.
fn same_material(g: &State, k: usize) -> bool {
    let run = g.p[k].run.as_ref().expect("a settled piece has a run");
    // (and what the page builder reads after the piece: `\output`, what
    // the default output ships with)
    let keys: Vec<Key> = std::iter::once(Key::TokPar(TokPar::Output))
        .chain(crate::page::ship_keys())
        .collect();
    let output = |r: &Run| -> Vec<Option<Val>> {
        keys.iter()
            .map(|key| {
                r.exports
                    .binary_search_by(|(x, _)| x.cmp(key))
                    .ok()
                    .map(|i| r.exports[i].1.clone())
            })
            .collect()
    };
    g.old_runs.get(k).and_then(Option::as_ref).is_some_and(|o| {
        Arc::ptr_eq(o, run)
            || (o.paras.len() == run.paras.len()
                && o.paras
                    .iter()
                    .zip(&run.paras)
                    .all(|(a, b)| Arc::ptr_eq(a, b) || **a == **b)
                && output(o) == output(run))
    })
}

/// Once every piece is settled, as many as the last build's: the last
/// piece whose material differs from it (none: the scan cannot converge on
/// the last build's pages).
fn last_diff(g: &mut State) -> Option<usize> {
    let n = g.p.len();
    if g.settled < n || g.old_runs.len() != n || g.old_orgs.is_empty() {
        return None;
    }
    if !g.last_diff_known {
        g.last_diff = (0..n).rev().find(|&k| !same_material(g, k));
        g.last_diff_known = true;
    }
    g.last_diff
}

/// The scan resumes before piece `c`, the first whose material differs
/// from the last build's (equal paragraphs are equal material: lines are a
/// function of the paragraph): at the restart point of the last old page
/// whose making call and restart point are before `c`; the pages before it
/// and the marks of the calls before it are kept.
fn resume(g: &mut State, c: usize) {
    g.matching = false;
    let before = |o: (u32, u32)| (o.0 as usize) < c;
    let at = (1..=g.old_orgs.len()).rev().find(|&p| {
        let o = &g.old_orgs[p - 1];
        before(o.call) && o.resume.is_some_and(|(s, _)| before(s))
    });
    let Some(p) = at else {
        g.scan_next = 0;
        g.scan_skip = 0;
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
    // (a point after its call: the call's mark is kept whole; before it:
    // the call is made again, its pages before the point kept)
    let (kept, call_from) = if s > o.call {
        (ci + 1, None)
    } else {
        (ci, Some(g.old_marks[ci].0))
    };
    g.marks_before = g.old_marks[..kept].to_vec();
    g.scan = Some(Box::new(Scan::resumed(
        p as u32,
        d,
        call_from,
        kept as u32,
        o.last_call.clone(),
    )));
    g.scan_next = s.0 as usize;
    g.scan_skip = s.1;
}

/// The round ends: no new task; the runs in progress waiting for a
/// piece that will not run in it stop (they are not results), the others
/// finish and are kept where they hold.
fn restructure_now(g: &mut State, sh: &Shared) {
    g.restructure = true;
    sh.restructuring.store(true, Ordering::Release);
    for &q in &g.running {
        if sh.waiting[q].load(Ordering::Acquire) {
            sh.cancel[q].store(true, Ordering::Relaxed);
        }
    }
    let _p = sh.park.lock().expect("park");
    sh.generation.fetch_add(1, Ordering::AcqRel);
    sh.parked.notify_all();
}

/// Moves the settled frontier over the pieces whose runs are final; a
/// settled run that says otherwise of the sequence than the layout assumed
/// ends the round (now, if it reads on into the next text; once all have
/// settled, else). Whether the round ends.
fn settle(g: &mut State, sh: &Shared) -> bool {
    let n = g.p.len();
    while g.settled < n && g.p[g.settled].status == Status::Done && g.p[g.settled].dirty.is_empty()
    {
        // (a settled run that says otherwise of the sequence than the
        // layout assumed changes it, once all have settled)
        let k = g.settled;
        if sh.restructure_ok && !g.p[k].got.same(&g.p[k].expect) {
            // (a settled run reading on into the next text: the pieces
            // after it begin in the middle of a command, they wait for the
            // sequence laid out again — the runs in progress finish and are
            // kept where they hold)
            if g.p[k].got.join {
                if k + 1 < n && g.p[k + 1].status == Status::Running {
                    sh.cancel[k + 1].store(true, Ordering::Relaxed);
                }
                restructure_now(g, sh);
                return true;
            }
            g.restructure_due = true;
        }
        g.settled += 1;
    }
    if g.restructure_due && g.settled == n {
        // (a run that said so may have run again since)
        if (0..n).any(|k| !g.p[k].got.same(&g.p[k].expect)) {
            restructure_now(g, sh);
            return true;
        }
        g.restructure_due = false;
    }
    false
}

/// The scheduler: a function of the state, choosing this thread's next
/// piece of work.
fn pick(g: &mut State, sh: &Shared) -> Option<Task> {
    if g.restructure {
        return None;
    }
    let n = g.p.len();
    if settle(g, sh) {
        return None;
    }
    if g.matching {
        advance_match(g);
    }
    let mut scan_to = g.scan_next;
    // (while the scan is out, nothing to look for: its next pieces are
    // counted when it is back)
    while g.scan.is_some() && scan_to < g.settled && lines_ready(g, scan_to) {
        scan_to += 1;
    }
    // (a settled piece whose lines are all broken: its part of the log)
    while g.log_next < g.settled && lines_ready(g, g.log_next) {
        let k = g.log_next;
        let ps = &g.p[k];
        let run = ps.run.as_ref().expect("a settled piece has a run");
        if !ps.log.as_ref().is_some_and(|f| f.holds(run, &ps.lines)) {
            g.log_queue.push_back(k);
        }
        g.log_next += 1;
    }
    let scan_finish = g.scan_next == n && g.settled == n && !g.scan_done;
    let scan_ok = g.scan.is_some() && (scan_to > g.scan_next || scan_finish);
    let speculate = sh.opts.reads == Reads::Speculate;
    let ready = g
        .unstarted
        .iter()
        .copied()
        .find(|&k| entry_ready(g, k, speculate && !g.p[k].suspend));
    let run_ok = ready.is_some();
    // (priority: pages out first, then the scan, the reading, line breaking)
    let mut kinds = [0u8; 5];
    let mut nk = 0;
    for (ok, kind) in [
        (!g.ship_queue.is_empty(), 3u8),
        (scan_ok, 2),
        (run_ok, 0),
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
                    let k = *g.unstarted.iter().nth(i).expect("an unstarted piece");
                    if entry_ready(g, k, speculate) {
                        k
                    } else {
                        first
                    }
                }
                Order::Priority => first,
            };
            g.unstarted.remove(&k);
            g.running.insert(k);
            g.p[k].status = Status::Running;
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
            let pieces = (g.scan_next..scan_to)
                .map(|k| {
                    let ps = &g.p[k];
                    let r = ps.run.as_ref().expect("a settled piece has a run");
                    let paras = r
                        .paras
                        .iter()
                        .zip(&ps.lines)
                        .map(|(p, l)| (p.clone(), l.clone().expect("lines ready")))
                        .collect();
                    (r.clone(), paras)
                })
                .collect();
            let predicted = g.predicted.iter().map(|x| x.0).collect();
            let end_run = g.p.last().and_then(|ps| ps.run.clone());
            let scan = g.scan.take().expect("the scan is free");
            let (from, skip) = (g.scan_next, std::mem::take(&mut g.scan_skip));
            let converge = last_diff(g).map(|after| (after, g.old_orgs.clone()));
            Some(Task::Scan {
                scan,
                from,
                skip,
                converge,
                to: scan_to,
                pieces,
                finish: scan_finish,
                predicted,
                end_run,
            })
        }
        4 => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.log_queue.len()),
                Order::Priority => 0,
            };
            let k = g.log_queue.remove(i).expect("a queued piece");
            g.logs_running += 1;
            let ps = &g.p[k];
            Some(Task::Log(
                k,
                ps.text.clone(),
                ps.run.clone().expect("a run"),
                ps.lines.clone(),
            ))
        }
        _ => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.ship_queue.len()),
                Order::Priority => 0,
            };
            let pg = g.ship_queue.remove(i).expect("a page to ship");
            Some(Task::Ship(pg, g.pages[pg].clone(), g.numbers.clone()))
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
            let current = g.p[k]
                .run
                .as_ref()
                .and_then(|r| r.paras.get(j))
                .is_some_and(|q| Arc::ptr_eq(q, &para));
            if current {
                g.p[k].lines[j] = Some(lines);
            }
            g.stats.breaks_run += 1;
            drop(g);
            sh.cv.notify_all();
            (para.hlist.len() as u64, 1)
        }
        Task::Scan {
            mut scan,
            from,
            skip,
            converge,
            to,
            pieces,
            finish,
            predicted,
            end_run,
        } => {
            let before = scan.fed;
            let mut converged = None;
            // (where the scan stopped, a fire's definitions having reached a
            // node it read: piece and item to go on from)
            let mut stop: Option<(usize, u32)> = None;
            let mut env = BuildFold {
                sh,
                k: from,
                node: 0,
                run: None,
                predicted,
                fired: Vec::new(),
                routines: 0,
                kept: 0,
            };
            'feed: for (d, (run, paras)) in pieces.iter().enumerate() {
                let k = from + d;
                env.k = k;
                env.run = Some(run.clone());
                let skip = if d == 0 { skip as usize } else { 0 };
                let nodes = run.para_nodes();
                let mut i = 0usize;
                for (pj, pl) in paras.iter().enumerate() {
                    let node = nodes.get(pj).copied().unwrap_or(0);
                    for it in material(std::slice::from_ref(pl)) {
                        if i < skip {
                            i += 1;
                            continue;
                        }
                        env.node = node;
                        if env.pass((k as u32, node)) {
                            stop = Some((k, i as u32));
                            break 'feed;
                        }
                        scan.feed(it, (k as u32, i as u32), &mut env);
                        i += 1;
                        if scan.halt {
                            scan.halt = false;
                            stop = Some((k, i as u32));
                            break 'feed;
                        }
                        if let Some((page, r)) = scan.just.take()
                            && let Some((after, olds)) = &converge
                            && r.0.0 as usize > *after
                            && let Some(j) = olds.iter().position(|o| o.resume == Some(r))
                        {
                            converged = Some((page as usize, j));
                            break 'feed;
                        }
                    }
                }
            }
            if finish && converged.is_none() && stop.is_none() {
                if let Some(r) = end_run {
                    env.k = to.saturating_sub(1);
                    env.node = r.nodes_len().saturating_sub(1) as u32;
                    env.run = Some(r);
                }
                if env.pass((u32::MAX, 0)) {
                    stop = Some((to, 0));
                } else {
                    scan.finish((to as u32, 0), &mut env);
                }
            }
            let (routines, routines_kept) = (env.routines, env.kept);
            let work = scan.fed - before;
            let (new, new_orgs) = scan.take_pages();
            let mut g = lock(sh);
            g.stats.routines += routines;
            g.stats.routines_kept += routines_kept;
            let finish = finish && stop.is_none();
            let to = match stop {
                Some((k, i)) => {
                    g.scan_skip = i;
                    k
                }
                None => to,
            };
            // (the pieces the scan passed drop their nodes, if lean)
            for k in g.scan_next.min(to)..to {
                let ps = &mut g.p[k];
                if let Some((full, l)) = ps.lean_later.take()
                    && ps.status == Status::Done
                    && ps.run.as_ref().is_some_and(|r| Arc::ptr_eq(r, &full))
                {
                    ps.run = Some(l);
                }
            }
            g.scan_next = to;
            // (the fonts the pieces fed load, numbered)
            while g.numbered < to {
                let k = g.numbered;
                if let Some(r) = g.p[k].run.clone() {
                    dvi::extend(Arc::make_mut(&mut g.numbers), &r.fonts);
                }
                g.numbered += 1;
            }
            for (pg, org) in new.into_iter().zip(new_orgs) {
                let i = g.pages.len();
                // (unchanged lines are the same `Arc`s: a cheap comparison)
                let same = g
                    .old_pages
                    .get(i)
                    .and_then(Option::as_ref)
                    .filter(|(op, _)| *op == pg)
                    .map(|(_, b)| b.clone());
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
                g.orgs[k as usize].resume = Some(r);
            }
            if let Some((k, j)) = converged {
                // (new page k and old page j have the same restart point,
                // after the last difference: the fold from there is the
                // last build's, its pages and marks shifted)
                let olds = g.old_orgs.clone();
                let made = g.pages.len();
                let shift = k as i64 - j as i64;
                let dmarks = i64::from(g.orgs[k].call_idx) - i64::from(olds[j].call_idx);
                for i in (made as i64 - shift) as usize..g.old_pages.len() {
                    let (pg, b) = g.old_pages[i].take().expect("an old page");
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
                g.marks_after = g.old_marks[from.min(g.old_marks.len())..]
                    .iter()
                    .map(|&(a, b)| (sh_(a), sh_(b)))
                    .collect();
                g.scan_next = g.p.len();
                g.scan_done = true;
            }
            if finish {
                g.scan_done = true;
            }
            g.scan = Some(scan);
            drop(g);
            sh.cv.notify_all();
            (work, 2)
        }
        Task::Ship(i, page, numbers) => {
            // (at the file's start, with nothing defined: the join puts in
            // or leaves out the `fnt_def`s, and writes again a page whose
            // commands depend on where it lands)
            let body = Arc::new(dvi::page_body_in(&page, &numbers, &[], &[], 0, sh.names.given.limits.dvi_buf_size));
            let mut g = lock(sh);
            if body.fixed.is_some() {
                g.stats.bodies_fixed += 1;
            }
            g.bodies[i] = Some(body);
            g.unshipped -= 1;
            g.stats.pages_shipped += 1;
            drop(g);
            sh.cv.notify_all();
            (page.b.list.len() as u64, 3)
        }
        Task::Log(k, text, run, lines) => {
            let f = Arc::new(logs::fragment(&text, &run, &lines));
            let work = f.entries.len() as u64;
            let mut g = lock(sh);
            g.p[k].log = Some(f);
            g.logs_running -= 1;
            drop(g);
            sh.cv.notify_all();
            (work, 4)
        }
    }
}

/// Runs piece `k` (marked running by whoever chose it) from its dirty
/// nodes and installs the run: its exports into the names index, its
/// reads as edges, the readers its changed definitions reach marked; its
/// own reads validated against the index as it is now.
fn run_and_install(sh: &Shared, k: usize, dirty: &[(Key, u32)]) -> u64 {
    let (text, old, entry, strict, entry_assumed) = {
        let mut g = lock(sh);
        if std::mem::take(&mut g.p[k].recheck) && dirty.is_empty() && keep_again(&mut g, sh, k) {
            drop(g);
            notify(sh, true);
            return 0;
        }
        g.p[k].entered = entry_state(&g, k);
        let ps = &g.p[k];
        // (entered on the state assumed at the cut: the piece before it
        // has not settled its run)
        let entry_assumed =
            matches!(ps.entry, Entry::After(q, _) if !(g.p[q].status == Status::Done && g.p[q].dirty.is_empty()));
        let old = match (&ps.run, &ps.prev) {
            (Some(r), _) => Some((r.clone(), ps.text.clone())),
            (None, Some((r, _, t))) => Some((r.clone(), t.clone())),
            (None, None) => None,
        };
        (ps.text.clone(), old, ps.entered.clone(), ps.suspend, entry_assumed)
    };
    sh.cancel[k].store(false, Ordering::Relaxed);
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
    };
    let run = Arc::new(run_piece(
        &text,
        &mut env,
        old.as_ref().map(|(r, t)| (&**r, &t[..])),
        dirty,
        entry.as_ref(),
    ));
    if run.abandoned {
        // (not a result: the piece runs again, from its last run, its
        // dirty names still dirty)
        let mut g = lock(sh);
        g.stats.abandoned += 1;
        g.stats.commands_run += run.ran;
        g.p[k].suspend |= env.gave_up;
        for (key, from) in dirty {
            dirty_from(&mut g.p[k].dirty, key.clone(), *from);
        }
        g.p[k].status = Status::Unstarted;
        g.running.remove(&k);
        g.unstarted.insert(k);
        drop(g);
        notify(sh, true);
        return run.ran;
    }
    let steps = run.ran;
    let t = Instant::now();
    let got = Expect::of(&run);
    // (what the piece keeps once it has settled: the run without its nodes)
    // (a run in an edit keeps its nodes a while: [`HOT_BUILDS`])
    let hot = sh.build_no > 1;
    let lean = (sh.lean && !hot).then(|| Arc::new(run.leaned(&text)));
    // (what stood for this piece until now: its last run, or for an edited
    // piece the run of the piece it replaces; nobody else installs this
    // piece while it runs here)
    let old_run = old.map(|(r, _)| r);
    // (before the lock: each read validated against the names index as it
    // is now, noting how many changes to the index that has seen; the
    // names whose values this run changes; the paragraphs it kept)
    let seen = sh.names_gen.load(Ordering::Acquire);
    let mut checks: Vec<(Option<Pos>, bool)> = run
        .reads
        .iter()
        .map(|(key, node, val)| {
            let (def, now, ..) = sh.names.reaching(key, (k as u32, *node), &sh.font);
            (def, val.seen_as(&now))
        })
        .collect();
    let changed = changed_names(old_run.as_deref(), &run);
    let equal = equal_paras(old_run.as_deref(), &run);
    // (a run reading the same names as the old one, in the same order,
    // leaves the edges whose node and source are the same as they are)
    let same_keys = old_run.as_ref().is_some_and(|o| {
        o.read_count() == run.reads.len()
            && run
                .reads
                .iter()
                .enumerate()
                .all(|(i, b)| *o.read_at(i).0 == b.0)
    });
    let read_keys: FxSet<&Key> = if old_run.is_some() && !same_keys {
        run.reads.iter().map(|(key, _, _)| key).collect()
    } else {
        FxSet::default()
    };
    {
        let mut g = lock(sh);
        let held = Instant::now();
        g.stats.commands_run += run.ran;
        let id = sh.seq.piece(k);
        // (names changed since the reads were validated: those reads again;
        // the index changes only under this lock)
        let now_gen = sh.names_gen.load(Ordering::Acquire);
        if now_gen != seen {
            let since: FxSet<&Key> = g.names_log[seen..now_gen].iter().flatten().collect();
            for ((key, node, val), c) in run.reads.iter().zip(&mut checks) {
                if since.contains(key) {
                    let (def, now, ..) = sh.names.reaching(key, (k as u32, *node), &sh.font);
                    *c = (def, val.seen_as(&now));
                }
            }
        }
        // (the edges: the new run's, each with the piece whose definition
        // answers it now, replace the old run's; a read answered otherwise
        // than it was is stale)
        let mut stale = Dirty::new();
        let old_srcs = std::mem::take(&mut g.p[k].srcs);
        let mut srcs = Vec::with_capacity(run.reads.len());
        for (i, ((key, node, _), &(def, same))) in run.reads.iter().zip(&checks).enumerate() {
            if !same {
                dirty_from(&mut stale, key.clone(), 0);
            }
            let src = def.map_or(INITIAL, |d| sh.seq.id_at(d));
            srcs.push(src);
            // (one edge per name: its first reader; a second entry for the
            // name, read with another value, follows it)
            if i > 0 && run.reads[i - 1].0 == *key {
                continue;
            }
            if same_keys
                && old_srcs.get(i) == Some(&src)
                && old_run.as_ref().is_some_and(|o| o.read_at(i).1 == *node)
            {
                continue;
            }
            if let Some(m) = g.readers.get_mut(key) {
                m.insert(id, (*node, src));
            } else {
                let mut m = FxMap::default();
                m.insert(id, (*node, src));
                g.readers.insert(key.clone(), m);
            }
        }
        g.p[k].srcs = srcs;
        if let Some(o) = old_run.as_ref().filter(|_| !same_keys) {
            for i in 0..o.read_count() {
                let key = o.read_at(i).0;
                if !read_keys.contains(key)
                    && let Some(m) = g.readers.get_mut(key)
                {
                    m.remove(&id);
                }
            }
        }
        let mut changed = changed;
        for key in &changed {
            refresh_export(&sh.names, &sh.font, k, &run, key);
        }
        // (an export a definition between the piece's nodes may shadow:
        // where its own last definition is may have moved)
        for (key, _) in &run.exports {
            if sh.names.has_inner(key)
                && !changed.contains(key)
                && refresh_export(&sh.names, &sh.font, k, &run, key)
            {
                changed.push(key.clone());
            }
        }
        if !changed.is_empty() {
            g.names_log.push(changed.clone());
        }
        sh.names_gen.store(g.names_log.len(), Ordering::Release);
        g.p[k].run = Some(run.clone());
        g.p[k].got = got.clone();
        // (names marked while it ran — a definition made between its nodes
        // reaching a node whose read the run's first reader of the same
        // value stands for — are read again)
        let late = std::mem::take(&mut g.p[k].dirty);
        for (key, from) in late {
            dirty_from(&mut stale, key, from);
        }
        g.p[k].runs += 1;
        g.stats.ran += 1;
        g.running.remove(&k);
        let marks = reached(&mut g, sh, (k as u32, END), &changed);
        for (r, key) in marks {
            if outer_still(&g, sh, r, &key) {
                continue;
            }
            if mark(&mut g, r, key) {
                sh.done[r].store(false, Ordering::Release);
            } else if g.p[r].status == Status::Running {
                // (its run read what is defined again: it stops)
                sh.cancel[r].store(true, Ordering::Relaxed);
            }
        }
        queue_breaks(&mut g, k, &equal);
        // (a run reading on past its text: the piece after it begins in
        // the middle of a command, its run in progress is not a result)
        if got.join && k + 1 < g.p.len() && g.p[k + 1].status == Status::Running {
            sh.cancel[k + 1].store(true, Ordering::Relaxed);
        }
        // (entered in the state its predecessors leave now, or run again)
        let entered = entry_holds(&g, k, &run);
        if stale.is_empty() && entered {
            g.p[k].lean_later = lean.map(|l| (run.clone(), l));
            if hot {
                g.p[k].hot_until = sh.build_no + HOT_BUILDS;
            }
            g.p[k].status = Status::Done;
            sh.done[k].store(true, Ordering::Release);
            // (a run reading another file, or ending its file, changes the
            // sequence)
            // (a run that says otherwise of the sequence than the layout
            // assumed changes it, once it has settled: `pick`)
        } else {
            g.p[k].dirty = stale;
            g.p[k].status = Status::Unstarted;
            g.unstarted.insert(k);
            g.stats.reruns += 1;
        }
        // (the pieces entered in the state this one leaves)
        for q in g.deps[k].clone() {
            // (a run in progress entered otherwise than its piece is now
            // stops: it is not a result)
            if g.p[q].status == Status::Running
                && g.p[q].entered.as_ref().is_some_and(|e| entry_state(&g, q).is_some_and(|now| now != *e))
            {
                sh.cancel[q].store(true, Ordering::Relaxed);
            }
            let stale_entry = g.p[q].status == Status::Done
                && g.p[q].run.as_ref().is_some_and(|r| !entry_holds(&g, q, r));
            if stale_entry {
                g.p[q].status = Status::Unstarted;
                g.unstarted.insert(q);
                g.stats.reruns += 1;
                sh.done[q].store(false, Ordering::Release);
                if q < g.settled {
                    g.settled = q;
                }
            }
        }
        // (the frontier moves now: every thread may be waiting in a run)
        settle(&mut g, sh);
        g.stats.time_confirm += t.elapsed();
        g.stats.time_held += held.elapsed();
    }
    {
        let _p = sh.park.lock().expect("park");
        sh.generation.fetch_add(1, Ordering::AcqRel);
    }
    sh.parked.notify_all();
    sh.cv.notify_all();
    steps
}

/// A piece's reads of names from before it.
struct PieceEnv<'a> {
    sh: &'a Shared,
    k: usize,
    /// Reads run on predictions (`Reads::Speculate`, and not a piece whose
    /// speculation was given up).
    spec: bool,
    /// Every read waits for the pieces before it that may define its name
    /// (a piece whose speculating run was given up).
    strict: bool,
    /// The reads made on predictions: a definition not installed yet, or
    /// none while earlier pieces may still make one (name, node, value).
    predicted: Vec<(Key, u32, Val)>,
    /// The names index's change count when the predictions were last
    /// checked.
    checked: usize,
    /// The run was abandoned on predictions nothing confirmed (its piece
    /// then reads strictly).
    gave_up: bool,
    /// It was entered on the state assumed at its cut (a prediction too:
    /// the piece before it had not settled).
    entry_assumed: bool,
    /// Every read from before the piece so far: the name, its reader, the
    /// value read.
    seen: FxMap<Key, Vec<(u32, Val)>>,
}

/// How many builds a piece that ran in an edit keeps its nodes for.
pub const HOT_BUILDS: u64 = 8;

/// How often (in nodes run) a run on predictions looks at them again.
const CHECK_EVERY: u64 = 1 << 12;
/// Nodes after which a run on predictions an earlier piece has not
/// confirmed yet is given up (its piece then waits for them).
const GIVE_UP_AFTER: u64 = 1 << 18;

impl PieceEnv<'_> {
    /// Whether a read this run made — a prediction, or a definition since
    /// made again (its piece ran again) — is refuted: the names index now
    /// gives another value there. (`from..to`: the changes of the index
    /// since it last looked.)
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
        keys.iter().any(|key| {
            self.seen[key]
                .iter()
                .any(|(node, val)| sh.names.reaching(key, (self.k as u32, *node), &sh.font).1 != *val)
        })
    }

    fn reach_now(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let sh = self.sh;
        loop {
            let seen = sh.generation.load(Ordering::Acquire);
            let (def, val, lo, hi) = sh.names.reaching(key, (self.k as u32, node), &sh.font);
            // (the run is stopped — the sequence is laid out again, or a
            // name it read was defined again —: no waiting, any value; it
            // is not a result)
            if sh.cancel[self.k].load(Ordering::Relaxed) {
                return (val, Src::Initial, lo, hi);
            }
            let wait = |me: &Self| {
                // (the round is ending: a run that would wait stops)
                if sh.restructuring.load(Ordering::Acquire) {
                    sh.cancel[me.k].store(true, Ordering::Relaxed);
                    return false;
                }
                true
            };
            let Some(d) = def else {
                // (an undefined name while an earlier piece may still
                // define it: wait for the earlier pieces, or run the first
                // of them nobody has started)
                if self.spec || !(self.strict || matches!(val, Val::Meaning(Meaning::Undefined))) {
                    if self.k > 0 {
                        self.predicted.push((key.clone(), node, val.clone()));
                    }
                    return (val, Src::Initial, lo, hi);
                }
                if !wait(self) {
                    return (val, Src::Initial, lo, hi);
                }
                let leapfrog = {
                    let mut g = lock(sh);
                    let ready = g
                        .unstarted
                        .iter()
                        .copied()
                        .take_while(|&e| e < self.k)
                        .find(|&e| entry_ready(&g, e, false));
                    if let Some(e) = ready {
                        g.unstarted.remove(&e);
                        g.running.insert(e);
                        g.p[e].status = Status::Running;
                        g.stats.parks += 1;
                        g.stats.leapfrogs += 1;
                        Some((e, still_dirty(&mut g, self.sh, e)))
                    } else if g.running.first().is_some_and(|&e| e < self.k)
                        || g.unstarted.first().is_some_and(|&e| e < self.k)
                    {
                        g.stats.parks += 1;
                        None
                    } else {
                        return (val, Src::Initial, lo, hi);
                    }
                };
                if let Some((e, dirty)) = leapfrog {
                    run_and_install(sh, e, &dirty);
                } else {
                    if !wait(self) {
                        return (val, Src::Initial, lo, hi);
                    }
                    let mut p = sh.park.lock().expect("park");
                    sh.waiting[self.k].store(true, Ordering::Release);
                    while sh.generation.load(Ordering::Acquire) == seen && !sh.restructuring.load(Ordering::Acquire) {
                        p = sh.parked.wait(p).expect("park");
                    }
                    sh.waiting[self.k].store(false, Ordering::Release);
                }
                continue;
            };
            let src = Src::Piece(sh.seq.id_at(d));
            let d = d.0 as usize;
            // (an element between this piece's own nodes is this run's)
            if d == self.k || sh.done[d].load(Ordering::Acquire) {
                return (val, src, lo, hi);
            }
            if self.spec {
                self.predicted.push((key.clone(), node, val.clone()));
                return (val, src, lo, hi);
            }
            // (the definition is being remade: park until it is installed,
            // or make it here if nobody has started)
            if !wait(self) {
                return (val, src, lo, hi);
            }
            let leapfrog = {
                let mut g = lock(sh);
                match g.p[d].status {
                    Status::Done => return (val, src, lo, hi),
                    Status::Unstarted => {
                        // (not entered yet — it waits for the pieces it is
                        // entered from —: the first of the pieces before it
                        // that may run now runs here; a thread parked here
                        // may be the only one, its own run nested in
                        // others' it leapfrogged)
                        let e = if entry_ready(&g, d, false) {
                            Some(d)
                        } else {
                            g.unstarted.iter().copied().take_while(|&e| e < d).find(|&e| entry_ready(&g, e, false))
                        };
                        g.stats.parks += 1;
                        e.map(|e| {
                            g.unstarted.remove(&e);
                            g.running.insert(e);
                            g.p[e].status = Status::Running;
                            g.stats.leapfrogs += 1;
                            (e, still_dirty(&mut g, self.sh, e))
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
                if !wait(self) {
                    return (val, Src::Initial, lo, hi);
                }
                let mut p = sh.park.lock().expect("park");
                sh.waiting[self.k].store(true, Ordering::Release);
                while sh.generation.load(Ordering::Acquire) == seen && !sh.restructuring.load(Ordering::Acquire) {
                    p = sh.parked.wait(p).expect("park");
                }
                sh.waiting[self.k].store(false, Ordering::Release);
            }
        }
    }

    /// Whether a piece before this one has not settled its run.
    fn unconfirmed(&self) -> bool {
        (0..self.k).any(|e| !self.sh.done[e].load(Ordering::Acquire))
    }
}

impl Env for PieceEnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.sh.names.cs
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

    fn check(&mut self, ran: u64, words: u64) -> Check {
        // (marked stale while it runs, or the sequence changes: it stops)
        if self.sh.cancel[self.k].load(Ordering::Relaxed) {
            return Check::Abandon;
        }
        let cap = self.sh.opts.main_memory;
        let periodic = ran.is_multiple_of(CHECK_EVERY);
        if !(periodic || words > cap) {
            return Check::Go;
        }
        // (validate while running: a read refuted — a prediction, or a
        // definition made again since — abandons the run, which runs again
        // on what is known then)
        let now = self.sh.names_gen.load(Ordering::Acquire);
        if now != self.checked {
            let from = self.checked;
            self.checked = now;
            if self.refuted(from, now) {
                return Check::Abandon;
            }
        }
        // (a long run, or one past the memory, on predictions not confirmed
        // yet: given up; its piece waits for the pieces before it)
        if (!self.predicted.is_empty() || self.entry_assumed)
            && (ran >= GIVE_UP_AFTER || words > cap)
            && self.unconfirmed()
        {
            self.gave_up = true;
            return Check::Abandon;
        }
        if words > cap {
            Check::Exceeded(cap)
        } else {
            Check::Go
        }
    }

    fn has_between(&mut self, key: &Key) -> bool {
        self.sh.names.has_inner(key)
    }

    fn cut_ok(&mut self) -> bool {
        self.sh.restructure_ok
    }

    fn split_inputs(&mut self) -> bool {
        self.sh.restructure_ok && !self.sh.inline[self.k]
    }

    fn give_up_deep(&mut self) -> bool {
        if (!self.predicted.is_empty() || self.entry_assumed) && self.unconfirmed() {
            self.gave_up = true;
            return true;
        }
        false
    }

    fn stop_at(&mut self, pos: usize) -> bool {
        self.sh.restructure_ok && self.sh.stops[self.k].binary_search(&pos).is_ok()
    }

    fn any_between(&mut self) -> bool {
        self.sh.names.any_inner()
    }

    fn between(&mut self, key: &Key, after: u32, node: u32) -> Option<(Val, Src)> {
        let sh = self.sh;
        if !sh.names.has_inner(key) {
            return None;
        }
        let k = self.k as u32;
        match sh.names.reaching(key, (k, node), &sh.font) {
            (Some(p), val, ..) if p.0 == k && p.1 != END && p.1 >= after => {
                Some((val, Src::Piece(sh.seq.id_at(p))))
            }
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
        // (every read from before the piece, to see it while running if
        // its definition is made again)
        self.seen.entry(key.clone()).or_default().push((node, r.0.clone()));
        r
    }
}

/// What the page builder reads where it is called — node `node` of piece
/// `k` (`run`): the piece's own definitions up to that node, else the
/// names index before the place (a fire at the same place included).
fn read_at(sh: &Shared, run: Option<&Run>, k: usize, node: u32, key: &Key) -> Val {
    read_at_with(sh, run, k, node, key, true)
}

/// [`read_at`], the definitions made between the pieces' nodes at the
/// node itself (where the page builder fired) seen or not.
fn read_at_with(sh: &Shared, run: Option<&Run>, k: usize, node: u32, key: &Key, at_node: bool) -> Val {
    let local = run.and_then(|r| r.resolve(key, node as usize + 1));
    let to = if at_node { node.saturating_add(1) } else { node };
    let (ipos, ival, ..) = sh.names.reaching(key, (k as u32, to), &sh.font);
    match (local, ipos) {
        (Some((d, _)), Some(p)) if p.0 == k as u32 && p.1 != END && p.1 >= d => ival,
        (Some((_, v)), _) => v,
        (None, _) => ival,
    }
}

/// The page builder's environment in a scan task: it reads where the
/// page builder is called, runs output routines there and installs its
/// definitions.
struct BuildFold<'a> {
    sh: &'a Shared,
    k: usize,
    node: u32,
    run: Option<Arc<Run>>,
    /// The last build's definitions' places not made again yet.
    predicted: Vec<Pos>,
    /// Where this build's page builder fired, in order.
    fired: Vec<Pos>,
    routines: usize,
    kept: usize,
}

impl BuildFold<'_> {
    /// The scan is at `at`: the last build's page builder definitions
    /// before it that this build's did not make again go (their readers
    /// marked). Whether a reader was marked (the scan stops).
    fn pass(&mut self, at: Pos) -> bool {
        let n = self.predicted.partition_point(|&p| p < at);
        if n == 0 {
            return false;
        }
        let gone: Vec<Pos> = self.predicted.drain(..n).collect();
        self.retract(gone)
    }

    /// The last build's page builder definitions at `gone` go (their
    /// readers marked); whether one was.
    fn retract(&mut self, gone: Vec<Pos>) -> bool {
        let sh = self.sh;
        let mut g = lock(sh);
        let mut any = false;
        for pos in gone {
            let Ok(i) = g.predicted.binary_search_by_key(&pos, |x| x.0) else {
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
                (Arc::ptr_eq(&m.toks, &rt.toks) || m.toks == rt.toks)
                    && (Arc::ptr_eq(&m.page, &rt.page.b) || m.page == rt.page.b)
                    && m.fire == rt.reads
                    && m.overlay == overlay
                    && m.reads
                        .iter()
                        .all(|(key, val)| val.seen_as(&self.routine_read(overlay, key)))
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
        let pos = (self.k as u32, self.node);
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
        let pos = (self.k as u32, self.node);
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
                m
            } else {
                self.routines += 1;
                Arc::new(self.run_routine(&rt, &defs))
            };
            fired.items.clone_from(&m.items);
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
}

impl Env for RoutineEnv<'_> {
    fn cs(&self) -> &Arc<CsTable> {
        &self.fold.sh.names.cs
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
fn install_fold_defs(
    sh: &Shared,
    pos: Pos,
    defs: Vec<(Key, Val)>,
    memo: Option<Arc<Memo>>,
) -> bool {
    let mut g = lock(sh);
    if let Some(m) = memo {
        g.memo_new.push(m);
    }
    let mut changed: Vec<Key> = Vec::new();
    // (what the last build defined here, made again: the names it no
    // longer defines go)
    if let Ok(i) = g.predicted.binary_search_by_key(&pos, |x| x.0) {
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
    let pid = sh.seq.piece(pos.0 as usize);
    match g.fold_defs.iter_mut().find(|x| x.0 == pid && x.1 == pos.1) {
        Some(x) => {
            for (key, val) in defs {
                match x.2.iter_mut().find(|d| d.0 == key) {
                    Some(d) => d.1 = val,
                    None => x.2.push((key, val)),
                }
            }
        }
        None => g.fold_defs.push((pid, pos.1, defs)),
    }
    let any = changed_at(&mut g, sh, pos, changed) || moved;
    drop(g);
    notify(sh, any);
    any
}

/// Definitions of `keys` between piece `pos.0`'s nodes changed: its
/// exports of them, as [`export_of`] gives them.
fn refresh_exports(g: &mut State, sh: &Shared, pos: Pos, keys: &[Key]) -> bool {
    if pos.1 == END {
        return false;
    }
    let k = pos.0 as usize;
    let Some(run) = g.p.get(k).and_then(|ps| ps.run.clone()) else {
        return false;
    };
    let moved: Vec<Key> = keys
        .iter()
        .filter(|key| refresh_export(&sh.names, &sh.font, k, &run, key))
        .cloned()
        .collect();
    // (the readers of the export see the definition between the nodes now,
    // or the export again)
    changed_at(g, sh, (pos.0, END), moved)
}

/// The names `keys` changed at `pos`: logged, and their readers after it
/// marked. Whether one was.
fn changed_at(g: &mut State, sh: &Shared, pos: Pos, keys: Vec<Key>) -> bool {
    if keys.is_empty() {
        return false;
    }
    let mut marks: Vec<(usize, Key, u32)> = reached(g, sh, pos, &keys).into_iter().map(|(r, k)| (r, k, 0)).collect();
    // (a place between piece k's nodes reaches its own nodes after it,
    // those reading the name from one of its earlier nodes included — not
    // those before it: they run again from the first after it)
    if pos.1 != END
        && let Some(run) = g.p.get(pos.0 as usize).and_then(|ps| ps.run.as_ref())
    {
        for key in &keys {
            if run.reads_after(key, pos.1) {
                marks.push((pos.0 as usize, key.clone(), pos.1 + 1));
            }
        }
    }
    g.names_log.push(keys);
    sh.names_gen.store(g.names_log.len(), Ordering::Release);
    let any = !marks.is_empty();
    for (r, key, from) in marks {
        if mark_from(g, r, key, from) {
            sh.done[r].store(false, Ordering::Release);
        } else if g.p[r].status == Status::Running {
            sh.cancel[r].store(true, Ordering::Relaxed);
        }
    }
    any
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
        let b = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../partex-core/testdata/cmr10.tfm"
        ))
        .unwrap();
        Arc::new(Font::parse("cmr10", &b).unwrap())
    }

    /// An element between piece 2's nodes 5 and 6 defines `\x`: node 6 on
    /// read it, the nodes before read piece 0's, the pieces after read
    /// piece 2's exports; a read is kept for the nodes it reaches.
    #[test]
    fn places_are_pieces_and_nodes() {
        let given = Given::new(
            Arc::new(Vec::new()),
            Arc::from("texput"),
            Arc::default(),
            Vec::new(),
        );
        let (names, font) = (Names::new(given, true, Arc::new(CsTable::new()), None), font());
        let x = Key::Cs(names.cs.id("x"));
        names.set((0, END), &x, Some(&Val::Int(1)));
        names.set((2, 5), &x, Some(&Val::Int(2)));
        names.set((2, END), &x, Some(&Val::Int(3)));
        assert!(matches!(
            names.reaching(&x, (0, 7), &font),
            (None, _, 0, END)
        ));
        assert!(matches!(
            names.reaching(&x, (1, 7), &font),
            (Some((0, END)), Val::Int(1), 0, END)
        ));
        assert!(matches!(
            names.reaching(&x, (2, 3), &font),
            (Some((0, END)), Val::Int(1), 0, 6)
        ));
        assert!(matches!(
            names.reaching(&x, (2, 5), &font),
            (Some((0, END)), Val::Int(1), 0, 6)
        ));
        assert!(matches!(
            names.reaching(&x, (2, 6), &font),
            (Some((2, 5)), Val::Int(2), 6, END)
        ));
        assert!(matches!(
            names.reaching(&x, (3, 0), &font),
            (Some((2, END)), Val::Int(3), 0, END)
        ));
        let seq = Sequence::new(vec![10, 11, 12, 13], 21, FxMap::default());
        let v = seq.id_at((2, 5));
        assert_eq!(v, 21, "an element between nodes gets a new id");
        assert_eq!(seq.id_at((2, 5)), v, "and keeps it");
        assert_eq!(seq.id_at((2, END)), 12);
        assert_eq!(seq.pos_of(v), Some((2, 5)));
        assert_eq!(seq.pos_of(14), None);
        assert!(src_at_or_before(&seq, v, (2, END)) && !src_at_or_before(&seq, v, (2, 4)));
        assert!(src_at_or_before(&seq, INITIAL, (0, 0)));
        // (the next build keeps the id where the piece moved)
        let ids = seq.virt.into_inner().expect("virt").ids;
        let seq = Sequence::new(vec![9, 12, 13], 22, ids);
        assert_eq!(seq.pos_of(v), Some((1, 5)));
        assert_eq!(seq.id_at((1, 5)), v);
    }
}
