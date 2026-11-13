//! The client's parallel passes (a hint pass's chunks, a PDF's page
//! contents, the DVI file's pages) on up to `threads` threads, the
//! operations' counters, and the debug switches. The engine runs nodes on
//! its own workers (`phitex_defuse::Engine::evaluate`); nothing here
//! schedules a node. A pass's results land in their index's slot and are
//! read back in index order, so any thread count gives the same output.
//!
//! Natively, feature `rayon` (default): a persistent rayon pool per
//! thread count; without it workers scoped to each pass. On wasm32 with
//! `atomics` the same pool, its workers spawned by the host through the
//! callback given to [`set_spawn_worker`]; else in order on the calling
//! thread.

use phitex_defuse::FxHashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Whether the environment variable `$name` is set, read once per
/// process (a `getenv` takes the environment's lock and scans it; the
/// debug switches are tested on every run). Unset, empty or "0" is off.
macro_rules! env_flag {
    ($name:literal) => {{
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *F.get_or_init(|| std::env::var_os($name).is_some_and(|v| !v.is_empty() && v != "0"))
    }};
}
pub(crate) use env_flag;

/// A debug variable's text, read once per process (`None`: unset or
/// empty).
macro_rules! env_str {
    ($name:literal) => {{
        static F: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        F.get_or_init(|| std::env::var($name).ok().filter(|v| !v.is_empty()))
            .as_deref()
    }};
}
pub(crate) use env_str;

/// A trace, dump or debug check's switch (`observe` builds): `$name` set
/// ([`env_flag`]); compiled out of other builds, its code with it.
macro_rules! obs_flag {
    ($name:literal) => {
        cfg!(feature = "observe") && $crate::exec::env_flag!($name)
    };
}
pub(crate) use obs_flag;

/// Lines the runs print with `PHITEX_TEX_TIME` (a node's time, a page's
/// phases), kept and printed after the evaluation ([`flush_later`]):
/// stderr written inside the round would be in its walls.
static LATER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Keeps `line` for [`flush_later`].
#[cold]
#[inline(never)]
pub(crate) fn later(line: String) {
    LATER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(line);
}

/// Prints the lines kept by [`later`], in order.
#[cold]
#[inline(never)]
pub(crate) fn flush_later() {
    let lines = std::mem::take(
        &mut *LATER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for l in lines {
        eprintln!("{l}");
    }
}

/// A trace or dump's text switch (`observe` builds, [`env_str`]).
macro_rules! obs_str {
    ($name:literal) => {
        if cfg!(feature = "observe") {
            $crate::exec::env_str!($name)
        } else {
            None
        }
    };
}
pub(crate) use obs_str;

/// A timing or counter switch (`profile` builds): `$name` set
/// ([`env_flag`]); compiled out of other builds, its code with it.
macro_rules! prof_flag {
    ($name:literal) => {
        cfg!(feature = "profile") && $crate::exec::env_flag!($name)
    };
}
pub(crate) use prof_flag;

/// How the host starts a worker thread: it runs the closure on a new
/// worker (on wasm32: a web worker sharing the module's memory).
pub type SpawnWorker = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

static SPAWN: OnceLock<SpawnWorker> = OnceLock::new();

/// Gives the executor the host's way to start workers (required on
/// wasm32 for threads; natively optional, std threads otherwise). Only
/// the first call counts.
pub fn set_spawn_worker(f: SpawnWorker) {
    let _ = SPAWN.set(f);
}

/// Whether this target has threads at all: WebAssembly without atomics
/// (`wasm32-wasip1`, the single-threaded core a browser without shared
/// memory between threads runs, Firefox's extensions) has none, and
/// everything runs on the one thread there is.
pub(crate) const THREADED: bool =
    !cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));

/// A thread of the engine's own beside the evaluation, with the workers'
/// stack ([`phitex_defuse::WORKER_STACK`]), not std's default; on a
/// target without threads ([`THREADED`]), `f` runs here, at once.
pub(crate) fn thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Beside<T> {
    if !THREADED {
        return Beside::Done(f());
    }
    Beside::Thread(
        std::thread::Builder::new()
            .stack_size(phitex_defuse::WORKER_STACK)
            .spawn(f)
            .expect("a thread"),
    )
}

/// [`thread`]'s work: running beside, or done in place.
pub(crate) enum Beside<T> {
    Thread(std::thread::JoinHandle<T>),
    Done(T),
}

impl<T> Beside<T> {
    /// Its result, once the thread has ended ([`std::thread::JoinHandle::join`]).
    pub(crate) fn join(self) -> std::thread::Result<T> {
        match self {
            Beside::Thread(h) => h.join(),
            Beside::Done(t) => Ok(t),
        }
    }
}

/// A clock for the executor's counters: nanoseconds from some fixed
/// point, `None` where the target has none (wasm32: the host's clock is
/// not reachable from here).
pub(crate) trait Clock: Sync {
    fn now(&self) -> Option<u64>;
}

/// `std::time::Instant` from the first reading.
struct StdClock;

impl Clock for StdClock {
    fn now(&self) -> Option<u64> {
        static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let e = *EPOCH.get_or_init(std::time::Instant::now);
        u64::try_from(e.elapsed().as_nanos()).ok()
    }
}

/// The executor's clock.
pub(crate) fn clock() -> &'static dyn Clock {
    &StdClock
}

/// What a build runs now, readable from another thread while it runs
/// (`Build::progress`): one slot per worker holding the job it runs
/// ([`Progress::pack`]: the operation and the node's byte offset in the
/// document; 0 when idle), the jobs finished in this evaluation and
/// the jobs begun so far (a run is a job: a node run again counts
/// again). Workers only store into atomics (relaxed), once at a job's
/// start and once at its end. A reader may also be told each new page
/// as it is reached ([`Progress::on_page`]).
#[derive(Debug)]
pub struct Progress {
    slots: Box<[AtomicU64]>,
    done: AtomicU64,
    total: AtomicU64,
    /// The highest page a finisher reached in this evaluation: the
    /// pages shipped up to its firing, its own with them.
    page: AtomicU64,
    /// The pages each finisher shipped on its last run, by its node:
    /// its firing's place (the ordinary firings up to it, whether it is
    /// a special one, [`crate::pages::Page`]) and the pages. Kept across
    /// evaluations (an edit's finishers that did not run again shipped
    /// what they shipped), the nodes gone dropped
    /// ([`Progress::keep_ships`]).
    ships: std::sync::Mutex<FxHashMap<u64, (i64, bool, u32)>>,
    /// The evaluations begun (the first 1).
    evaluations: AtomicU64,
    on_page: OnceLock<PageHook>,
    /// The format: 0 not yet known, 1 being made, 2 ready
    /// ([`Progress::format`]).
    format: AtomicU64,
    on_format: OnceLock<FormatHook>,
    /// The pages streamed: each page's PDF made as its writer runs, not
    /// only once the loop-backs settle ([`Progress::set_streaming`]).
    streaming: std::sync::atomic::AtomicBool,
    on_stream: OnceLock<StreamHook>,
    /// The pages streamed' colors ([`Colors`]).
    colors: std::sync::Mutex<Colors>,
    /// Each page streamed's specials on the document (XeTeX's named
    /// objects made or added to: `crate::xdvpdf::XdvPage`), by the page's
    /// identity ([`PageId`]).
    doc_specials: std::sync::Mutex<FxHashMap<PageId, Vec<Vec<u8>>>>,
    /// Told at every job's start ([`Progress::on_tick`]).
    on_tick: OnceLock<TickHook>,
}

/// A page made while the build runs ([`Progress::on_stream`]): a PDF
/// file of that page alone, as the build's own file shows it, the fonts
/// it uses embedded. It is the page as this pass of the build made it:
/// a later pass, or the build's settling, may make it otherwise.
#[derive(Clone, Debug)]
pub struct StreamedPage {
    /// The evaluation it was made in ([`Progress::evaluation`]).
    pub evaluation: u64,
    /// Its place among the pages shipped, from 1.
    pub page: u64,
    /// The page's identity as [`crate::Build::pages`] gives it, were the
    /// build to settle with it.
    pub hash: u64,
    /// The one-page PDF file. Empty: page `page` told before as not
    /// [`StreamedPage::exact`], with the same `hash`, is now known exact
    /// (the pages before it found to leave the colors as they found them).
    pub pdf: Vec<u8>,
    /// Whether the file draws the page as the build's file will: every
    /// object it draws with was complete, in it, and the pages before it
    /// left the color stacks as they found them. Not exact: drawn with
    /// what it has, those it lacks left out (a graphics state, a shading
    /// or pattern, a color space, an external object made later).
    pub exact: bool,
}

/// A colour stack command a page's walk meets, in its order: the stack's
/// number and the command (a form's are the form stacks', not met).
pub(crate) type ColorOp = (usize, phitex_pdf::ColorStackCmd);

/// The colour stacks' page values where a page begins, each stack not as
/// it was made: its number, the entries pushed below its current value,
/// its current value (`None` in either: the value it was made with). A
/// page's own file starts from them (`crate::pdfout::page_alone`).
pub(crate) type ColorStart = Vec<phitex_pdf::PageColor>;

/// [`ColorStart`] after the commands `ops` (pdfTeX's `colorstackpush`,
/// `pop`, `set`; a pop from an empty stack changes nothing).
fn color_after(start: &ColorStart, ops: &[ColorOp]) -> ColorStart {
    type Stack = (Vec<Option<Vec<u8>>>, Option<Vec<u8>>);
    let mut m: std::collections::BTreeMap<usize, Stack> = start
        .iter()
        .map(|(n, s, c)| (*n, (s.clone(), c.clone())))
        .collect();
    for (n, op) in ops {
        let e = m.entry(*n).or_default();
        match op {
            phitex_pdf::ColorStackCmd::Push(t) => {
                let cur = e.1.take();
                e.0.push(cur);
                e.1 = Some(t.clone());
            }
            phitex_pdf::ColorStackCmd::Pop => {
                if let Some(x) = e.0.pop() {
                    e.1 = x;
                }
            }
            phitex_pdf::ColorStackCmd::Set(t) => e.1 = Some(t.clone()),
            phitex_pdf::ColorStackCmd::Current => {}
        }
    }
    m.into_iter()
        .filter(|(_, (s, c))| !s.is_empty() || c.is_some())
        .map(|(n, (s, c))| (n, s, c))
        .collect()
}

/// A page by what made it, whatever its place: the finisher that
/// shipped it (its node) and its place among that finisher's pages. An
/// edit that moves pages leaves a page's identity as it was, its number
/// not ([`Progress::page_of`]).
pub(crate) type PageId = (u64, u32);

/// The colour stacks where each page streamed begins ([`Progress::page_colors`]):
/// a page is drawn exactly alone only where every page before it is
/// known, its file starting from the colours they leave it (a colour
/// pushed on one page and popped on a later one colours the pages
/// between). Kept across a build's passes and its edits' evaluations (a
/// page not made again leaves what it left), each page's commands by its
/// identity, so that a page an edit moves carries its own to its new
/// place; made afresh when a build begins to stream
/// ([`Progress::set_streaming`]).
#[derive(Debug, Default)]
struct Colors {
    /// The commands of each page, by its identity.
    ops: FxHashMap<PageId, Vec<ColorOp>>,
    /// The pages, in order, whose commands made [`Colors::starts`]:
    /// `starts[j + 1]` is where the page after `seq[j]` begins.
    seq: Vec<PageId>,
    /// Where pages 1, 2, ... begin, as far as every page before is known
    /// (the first: none carried in).
    starts: Vec<ColorStart>,
    /// The pages streamed before every page before them was known, drawn
    /// from no colour carried in ([`Waiting`]), by page (from 1): this
    /// evaluation's ([`Progress::begin`]).
    waiting: std::collections::BTreeMap<u64, Waiting>,
    /// What was told of each page (from 1) last, its file streamed
    /// ([`Told`]): a page made again as it was told is not told again.
    told: std::collections::BTreeMap<u64, Told>,
}

/// A page told ([`Colors::told`]): its identity, whether exact, the
/// colours its file began from (whether known: [`Progress::page_colors`])
/// and its file's key ([`Made::File`]).
#[derive(Debug, PartialEq)]
struct Told {
    hash: u64,
    exact: bool,
    start: ColorStart,
    known: bool,
    file: u64,
}

/// What a page's file made gives [`Progress::stream_page`]: the file as
/// it was told last (its key the same: nothing to tell), or a file, the
/// objects it draws with all in it, its making again from the colours
/// carried in once they are known (where they are not yet), and its key:
/// what the file is made of (the page, the colours it begins from, the
/// commands of the objects it takes), the same key the same file.
pub(crate) enum Made {
    Same,
    File(Vec<u8>, bool, Option<Remake>, u64),
}

impl Colors {
    /// [`Colors::starts`] made good for the pages in `order` (their
    /// identities, first first): kept as far as the pages they were made
    /// from are the same pages in the same places, made on from there
    /// while each page's commands are known.
    fn settle(&mut self, order: &[PageId]) {
        if self.starts.is_empty() {
            self.starts.push(ColorStart::new());
        }
        let same = self
            .seq
            .iter()
            .zip(order)
            .take_while(|(a, b)| a == b)
            .count();
        self.seq.truncate(same);
        self.starts.truncate(same + 1);
        while let Some(id) = order.get(self.seq.len())
            && let Some(o) = self.ops.get(id)
        {
            let next = color_after(self.starts.last().unwrap_or(&ColorStart::new()), o);
            self.starts.push(next);
            self.seq.push(*id);
        }
    }

    /// Where page `k` (from 1) begins, where every page before it is
    /// known.
    fn start(&self, k: u64) -> Option<&ColorStart> {
        usize::try_from(k.checked_sub(1)?)
            .ok()
            .and_then(|i| self.starts.get(i))
    }
}

/// A page's file made again from where its colours begin, on the values
/// it was made from (none: those values are gone, a collection since, or
/// the writer could not): its PDF and whether it draws every object as
/// the build's file will.
pub(crate) type Remake =
    Box<dyn FnOnce(&phitex_defuse::Values, &ColorStart) -> Option<(Vec<u8>, bool)> + Send>;

/// A page's identity made again from the colours carried into it
/// ([`Progress::page_colors`]): another page to a viewer.
pub(crate) fn recolored(hash: u64) -> u64 {
    phitex_defuse::hash_words(hash, &[0x636f_6c6f_7273])
}

/// A page streamed before its colours' start was known
/// ([`Progress::stream_page`]): its identity, whether its file draws
/// every object as the build's will, and how to make it again.
struct Waiting {
    hash: u64,
    objects: bool,
    remake: Remake,
}

impl std::fmt::Debug for Waiting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Waiting({:x}, {})", self.hash, self.objects)
    }
}

/// What is told of the pages streamed ([`Progress::on_stream`]): each
/// page made, every one, but one made again as it was told last.
pub struct StreamHook {
    pub page: Box<dyn Fn(StreamedPage) + Send + Sync>,
}

impl std::fmt::Debug for StreamHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamHook")
    }
}

/// What is told as the build goes on ([`Progress::on_tick`]).
pub struct TickHook(Box<dyn Fn() + Send + Sync>);

impl std::fmt::Debug for TickHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TickHook")
    }
}

/// What is told of the format ([`Progress::on_format`]).
pub struct FormatHook(Box<dyn Fn(bool) + Send + Sync>);

impl std::fmt::Debug for FormatHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormatHook")
    }
}

/// What is told of a new page: the evaluation (as
/// [`Progress::evaluation`] counts) and the page reached.
pub struct PageHook(Box<dyn Fn(u64, u64) + Send + Sync>);

impl std::fmt::Debug for PageHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PageHook")
    }
}

impl Progress {
    /// The offset of a node not in the document's own text (one a run
    /// unfolded).
    pub const INSIDE: u64 = (1 << 48) - 1;

    /// Slots for `workers` workers and the evaluating thread (a reader
    /// can make it before the build: [`crate::Options::progress`]).
    #[must_use]
    pub fn new(workers: usize) -> Progress {
        Progress {
            slots: (0..=workers).map(|_| AtomicU64::new(0)).collect(),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            page: AtomicU64::new(0),
            ships: std::sync::Mutex::new(FxHashMap::default()),
            evaluations: AtomicU64::new(0),
            on_page: OnceLock::new(),
            format: AtomicU64::new(0),
            on_format: OnceLock::new(),
            streaming: std::sync::atomic::AtomicBool::new(false),
            on_stream: OnceLock::new(),
            colors: std::sync::Mutex::default(),
            doc_specials: std::sync::Mutex::new(FxHashMap::default()),
            on_tick: OnceLock::new(),
        }
    }

    /// `hook` told of each page made while the build runs, once
    /// [`Progress::set_streaming`] is on (once per progress: whether it
    /// was set): its `page` given the page on the writer's thread.
    pub fn on_stream(&self, hook: StreamHook) -> bool {
        self.on_stream.set(hook).is_ok()
    }

    /// The evaluations begun from now on stream their pages (`on`) or do
    /// not: a page's PDF side runs as soon as its page is shipped, not
    /// once the loop-backs settle, and each page made is told
    /// ([`Progress::on_stream`]), a clean build's and every edit's. The
    /// output is the same either way. What the pages streamed left (their
    /// colours, what was told) is kept while it stays on, so an edit's
    /// pages begin from the pages it left alone; made afresh when it is
    /// turned on.
    pub fn set_streaming(&self, on: bool) {
        if on && !self.streaming.swap(true, Ordering::Relaxed) {
            *self
                .colors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Colors::default();
            self.doc_specials
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        self.streaming.store(on, Ordering::Relaxed);
    }

    /// The pages' identities in their order ([`PageId`]), as
    /// [`Progress::page_of`] places them.
    fn order(&self) -> Vec<PageId> {
        let s = self
            .ships
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut f: Vec<(i64, bool, u64, u32)> =
            s.iter().map(|(&n, &(p, sp, k))| (p, sp, n, k)).collect();
        drop(s);
        f.sort_unstable();
        f.into_iter()
            .flat_map(|(_, _, n, k)| (0..k).map(move |i| (n, i)))
            .collect()
    }

    /// Whether the evaluations stream their pages.
    #[must_use]
    pub fn streaming(&self) -> bool {
        self.streaming.load(Ordering::Relaxed)
    }

    /// Page `k` (from 1) made in this evaluation, `hash` its identity,
    /// told: `make` gives, from where its colours begin (none: not known
    /// yet, drawn from none carried in), its PDF and whether that draws
    /// every object as the build's file will (none: not streamed). It is
    /// told exact where that is so and its colours' start is known
    /// ([`Progress::page_colors`]); where it is not known yet, told again
    /// exact once it is known to carry none. A page made as it was told
    /// last (the same identity, from the same colours, exact) is not made
    /// or told again: an edit tells the pages it changed.
    pub(crate) fn stream_page(
        &self,
        k: u64,
        hash: u64,
        make: impl FnOnce(Option<&ColorStart>, Option<u64>) -> Option<Made>,
    ) {
        let Some(h) = self.on_stream.get() else {
            return;
        };
        let order = self.order();
        let (start, last) = {
            let mut c = self
                .colors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            c.settle(&order);
            let start = c.start(k).cloned();
            if let Some(st) = &start
                && c.told
                    .get(&k)
                    .is_some_and(|t| t.hash == hash && t.exact && t.start == *st)
            {
                return;
            }
            // (the key of the file told last of the page, made from the
            // colours it begins from now, known: a file of the same key
            // is that file, not told again)
            let last = start.as_ref().and_then(|st| {
                c.told
                    .get(&k)
                    .filter(|t| t.known && t.hash == hash && t.start == *st)
                    .map(|t| t.file)
            });
            (start, last)
        };
        let got = make(start.as_ref(), last);
        let Some(Made::File(pdf, objects, remake, file)) = got else {
            if got.is_none() {
                self.colors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .told
                    .remove(&k);
            }
            return;
        };
        let e = self.evaluation();
        let exact = objects && start.is_some();
        {
            let mut c = self
                .colors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match remake.filter(|_| start.is_none()) {
                Some(remake) => {
                    c.waiting.insert(
                        k,
                        Waiting {
                            hash,
                            objects,
                            remake,
                        },
                    );
                }
                None => {
                    c.waiting.remove(&k);
                }
            }
            c.told.insert(
                k,
                Told {
                    hash,
                    exact,
                    known: start.is_some(),
                    start: start.unwrap_or_default(),
                    file,
                },
            );
        }
        (h.page)(StreamedPage {
            evaluation: e,
            page: k,
            hash,
            pdf,
            exact,
        });
    }

    /// Page `k` (from 1) of this build, `id` its identity ([`PageId`]),
    /// meets the colour stack commands `ops`: where the pages after it
    /// begin known as far as every page before them is. Each page
    /// streamed before that, from no colour carried in
    /// ([`Progress::stream_page`]), is told again: where none is carried
    /// in, exact as it is; else made again on `v` from the colours carried
    /// in, under another identity (the viewer draws it again), and told.
    pub(crate) fn page_colors(&self, id: PageId, ops: Vec<ColorOp>, v: &phitex_defuse::Values) {
        let e = self.evaluation();
        let order = self.order();
        let known: Vec<(u64, ColorStart, Waiting)> = {
            let mut c = self
                .colors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let c = &mut *c;
            if c.ops.get(&id) != Some(&ops) {
                // (the pages after it begin anew from it)
                if let Some(j) = c.seq.iter().position(|x| *x == id) {
                    c.seq.truncate(j);
                    c.starts.truncate(j + 1);
                }
                c.ops.insert(id, ops);
            }
            // (the pages gone: their commands dropped)
            if c.ops.len() > order.len() {
                let live: phitex_defuse::FxHashSet<PageId> = order.iter().copied().collect();
                c.ops.retain(|x, _| live.contains(x));
            }
            c.settle(&order);
            let n = c.starts.len() as u64;
            let rest = c.waiting.split_off(&(n + 1));
            let done = std::mem::replace(&mut c.waiting, rest);
            done.into_iter()
                .filter_map(|(p, w)| {
                    let st = c.start(p)?.clone();
                    Some((p, st, w))
                })
                .collect()
        };
        let Some(h) = self.on_stream.get() else {
            return;
        };
        for (page, start, w) in known {
            // (no key: made again, a file other than any made from it)
            let told = |exact: bool| Told {
                hash: w.hash,
                exact,
                start: start.clone(),
                known: false,
                file: 0,
            };
            if start.is_empty() {
                if w.objects {
                    self.colors
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .told
                        .insert(page, told(true));
                    (h.page)(StreamedPage {
                        evaluation: e,
                        page,
                        hash: w.hash,
                        pdf: Vec::new(),
                        exact: true,
                    });
                }
            } else if let Some((pdf, exact)) = (w.remake)(v, &start) {
                self.colors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .told
                    .insert(page, told(exact));
                (h.page)(StreamedPage {
                    evaluation: e,
                    page,
                    hash: crate::exec::recolored(w.hash),
                    pdf,
                    exact,
                });
            }
        }
    }

    /// Page `id` ([`PageId`]) of this build makes or adds to the
    /// document's named objects with `specials` (XeTeX's `pdf:obj @x`,
    /// `pdf:put @x`, `pdf:stream @x`).
    pub(crate) fn page_doc_specials(&self, id: PageId, specials: Vec<Vec<u8>>) {
        self.doc_specials
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, specials);
    }

    /// The document's specials of pages 1 to `k - 1` in order, where each
    /// of those pages is known ([`Progress::page_doc_specials`]).
    pub(crate) fn doc_specials_before(&self, k: u64) -> Option<Vec<Vec<u8>>> {
        let order = self.order();
        let m = self
            .doc_specials
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = usize::try_from(k.saturating_sub(1)).unwrap_or(usize::MAX);
        if order.len() < before {
            return None;
        }
        let mut out = Vec::new();
        for id in &order[..before] {
            out.extend(m.get(id)?.iter().cloned());
        }
        Some(out)
    }

    /// Whether a page streamed would be told ([`Progress::on_stream`]).
    pub(crate) fn streams(&self) -> bool {
        self.on_stream.get().is_some()
    }

    /// `hook` called on the thread that begins a job, at every job's
    /// start (once per progress: whether it was set; it keeps its own
    /// pace, a clock's): a
    /// host with one thread lets its other work in there.
    pub fn on_tick(&self, hook: Box<dyn Fn() + Send + Sync>) -> bool {
        self.on_tick.set(TickHook(hook)).is_ok()
    }

    /// The place (from 1) of page `i` (from 0) of those finisher `node`
    /// shipped, as [`Progress::note_ships`] counts the pages: the pages
    /// shipped by the firings before its own, then its own up to `i`.
    pub(crate) fn page_of(&self, node: u64, i: u32) -> Option<u64> {
        let s = self
            .ships
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let &(p, special, n) = s.get(&node)?;
        if i >= n {
            return None;
        }
        let before: u64 = s
            .values()
            .filter(|e| (e.0, e.1) < (p, special))
            .map(|e| u64::from(e.2))
            .sum();
        Some(before + u64::from(i) + 1)
    }

    /// `hook(ready)` called on the building thread when the format is
    /// found not cached and is being made (false; never for a build
    /// without a prelude: INITEX's bare state is no format), and when it
    /// is ready, made or read (true), before the document's first
    /// evaluation (once per progress: whether it was set).
    pub fn on_format(&self, hook: Box<dyn Fn(bool) + Send + Sync>) -> bool {
        self.on_format.set(FormatHook(hook)).is_ok()
    }

    /// The format: `None` not yet known, `Some(false)` being made,
    /// `Some(true)` ready.
    #[must_use]
    pub fn format(&self) -> Option<bool> {
        match self.format.load(Ordering::Relaxed) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    /// The format is being made (`ready` false) or is ready.
    pub(crate) fn note_format(&self, ready: bool) {
        self.format
            .store(if ready { 2 } else { 1 }, Ordering::Relaxed);
        if let Some(h) = self.on_format.get() {
            (h.0)(ready);
        }
    }

    /// `hook(evaluation, page)` called, on the finisher's thread, each
    /// time an evaluation reaches a page beyond the highest it had
    /// reached (once per progress: a second hook is not set; whether it
    /// was).
    pub fn on_page(&self, hook: Box<dyn Fn(u64, u64) + Send + Sync>) -> bool {
        self.on_page.set(PageHook(hook)).is_ok()
    }

    /// The evaluations begun so far (0: none; each settle of a build
    /// is one).
    #[must_use]
    pub fn evaluation(&self) -> u64 {
        self.evaluations.load(Ordering::Relaxed)
    }

    /// A slot's value: `op` (1 a text node, 2 a box put, 3 an output
    /// page, 0 another) in the top 16 bits, the byte offset below.
    #[must_use]
    pub fn pack(op: u16, offset: u64) -> u64 {
        (u64::from(op) + 1) << 48 | offset.min(Self::INSIDE)
    }

    /// A slot's value as `(op, offset)`; `None` when idle.
    #[must_use]
    pub fn unpack(x: u64) -> Option<(u16, u64)> {
        let op = u16::try_from(x >> 48).ok()?.checked_sub(1)?;
        Some((op, x & Self::INSIDE))
    }

    /// What each worker runs now.
    #[must_use]
    pub fn running(&self) -> Vec<Option<(u16, u64)>> {
        self.slots
            .iter()
            .map(|s| Self::unpack(s.load(Ordering::Relaxed)))
            .collect()
    }

    /// An evaluation begins: no jobs known or done, no page streamed in
    /// it waiting for its colours (the last's: its build is in).
    pub(crate) fn begin(&self) {
        self.colors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .waiting
            .clear();
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.page.store(0, Ordering::Relaxed);
        self.evaluations.fetch_add(1, Ordering::Relaxed);
    }

    /// The finisher `node`, of the firing at `place` (the ordinary
    /// firings up to it, whether it is a special one), shipped `shipped`
    /// pages: the page the build reached is the pages shipped by the
    /// firings up to it, this one's with them. Not the firings counted:
    /// a routine may ship no page (a two-column page's first column) or
    /// several (Grail, two columns: "page 35" of its 18 pages).
    pub(crate) fn note_ships(&self, node: u64, place: (i64, bool), shipped: u32) {
        let n = {
            let mut s = self
                .ships
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // (an ordinary firing's place is one firing's: a finisher
            // made again in its place replaces the one before)
            if !place.1 {
                s.retain(|&k, e| k == node || (e.0, e.1) != place);
            }
            s.insert(node, (place.0, place.1, shipped));
            s.values()
                .filter(|e| (e.0, e.1) <= place)
                .map(|e| u64::from(e.2))
                .sum::<u64>()
        };
        if n > 0
            && self.page.fetch_max(n, Ordering::Relaxed) < n
            && let Some(h) = self.on_page.get()
        {
            (h.0)(self.evaluations.load(Ordering::Relaxed), n);
        }
    }

    /// Only the finishers `live` keeps counted ([`Progress::note_ships`]):
    /// the nodes gone shipped nothing that stands.
    pub(crate) fn keep_ships(&self, live: impl Fn(u64) -> bool) {
        self.ships
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|&k, _| live(k));
    }

    /// The highest page a finisher reached in this evaluation (0: none
    /// yet): where the build is, as a reader counts pages.
    #[must_use]
    pub fn page(&self) -> u64 {
        self.page.load(Ordering::Relaxed)
    }

    /// The jobs finished in this evaluation.
    #[must_use]
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// The jobs known so far in this evaluation.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// The progress of a build and its text nodes' offsets, by node.
#[derive(Clone)]
pub(crate) struct Watch {
    pub progress: Arc<Progress>,
    pub offsets: Arc<FxHashMap<u64, u64>>,
}

impl Watch {
    pub(crate) fn new(workers: usize, given: Option<Arc<Progress>>) -> Watch {
        Watch {
            progress: given.unwrap_or_else(|| Arc::new(Progress::new(workers))),
            offsets: Arc::new(FxHashMap::default()),
        }
    }
}

thread_local! {
    /// This thread's worker number, for its slot.
    static WORKER: usize = {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
}

/// A run in a progress slot (`Build::progress`): one more job known
/// when it begins, its operation and its node's offset in the slot while
/// it runs, idle and one more job done when it ends.
pub(crate) struct Busy(Option<(Arc<Progress>, usize)>);

impl Busy {
    /// `op` (as [`Progress::pack`]) begins on `node` on this thread, in
    /// the progress the build watches now (`Ops::watch`, set before each
    /// evaluation with the text nodes' offsets).
    pub(crate) fn new(watch: &std::sync::RwLock<Option<Watch>>, op: u16, node: u64) -> Busy {
        let g = watch
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(w) = g.as_ref() else {
            return Busy(None);
        };
        let i = WORKER.with(|x| *x) % w.progress.slots.len();
        let off = w.offsets.get(&node).copied().unwrap_or(Progress::INSIDE);
        w.progress.total.fetch_add(1, Ordering::Relaxed);
        w.progress.slots[i].store(Progress::pack(op, off), Ordering::Relaxed);
        let p = w.progress.clone();
        drop(g);
        // (the tick told with no lock of the build's held here: a host
        // may answer its own requests inside it)
        if let Some(h) = p.on_tick.get() {
            (h.0)();
        }
        Busy(Some((p, i)))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some((p, i)) = self.0.take() {
            p.slots[i].store(0, Ordering::Relaxed);
            p.done.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// What the operations of an evaluation did, counted where they run
/// ([`NodeTimer`]), not where the executor's tasks run: a task is a
/// worker's loop, which also waits while other workers hold work.
#[derive(Default)]
pub(crate) struct OpCounters {
    /// the operations' run time, summed
    busy: AtomicU64,
    /// the run time and the runs by node kind ([`KINDS`], spec-final §1)
    kind_ns: [AtomicU64; KINDS.len()],
    kind_runs: [AtomicU64; KINDS.len()],
    /// operations running now
    running: AtomicUsize,
    /// the most operations running at once
    peak: AtomicUsize,
    /// (`PHITEX_CLOCK_PROBE`) by kind: the clock probe's nanoseconds at
    /// the runs' starts, at their ends, and the runs probed
    probe: [[AtomicU64; 3]; KINDS.len()],
}

/// The iterations of [`clock_probe`]'s chain.
const PROBE_ITERS: u64 = 4096;

/// (`profile`, `PHITEX_CLOCK_PROBE`) The nanoseconds a fixed chain of
/// dependent multiply-adds takes on this core now: its clock at the
/// moment (the chain is 4 cycles an iteration on the nodes measured).
#[inline(never)]
fn clock_probe() -> u64 {
    let t = std::time::Instant::now();
    let a = std::hint::black_box(0x9E37_79B9_u64);
    let mut x = std::hint::black_box(1u64);
    for _ in 0..PROBE_ITERS {
        x = x.wrapping_mul(a).wrapping_add(1);
    }
    std::hint::black_box(x);
    u64::try_from(t.elapsed().as_nanos()).unwrap_or(0)
}

/// The node kinds the cold accounting splits op time by (spec-final §1):
/// a package load is an `exec` run of a load's text.
pub(crate) const KINDS: [&str; 10] = [
    "exec", "load", "pages", "break", "finish", "resume", "files", "link", "dvi", "put",
];

/// The operations' counters ([`NodeTimer`], `profile`), summed over the
/// process, read after each evaluation ([`take_counters`]).
fn counters() -> &'static OpCounters {
    static C: OnceLock<OpCounters> = OnceLock::new();
    C.get_or_init(OpCounters::default)
}

/// The counters since the last take, reset: the operations' run time and
/// runs by node kind ([`KINDS`]), their summed run time, and the most
/// that ran at once.
pub(crate) fn take_counters() -> (
    Vec<(&'static str, std::time::Duration, u64)>,
    std::time::Duration,
    usize,
) {
    let c = counters();
    let by = KINDS
        .iter()
        .enumerate()
        .map(|(i, &k)| {
            (
                k,
                std::time::Duration::from_nanos(c.kind_ns[i].swap(0, Ordering::Relaxed)),
                c.kind_runs[i].swap(0, Ordering::Relaxed),
            )
        })
        .collect();
    let busy = std::time::Duration::from_nanos(c.busy.swap(0, Ordering::Relaxed));
    let peak = c.peak.swap(0, Ordering::Relaxed);
    (by, busy, peak)
}

/// After an evaluation, with `PHITEX_OBS` set and the `profile` feature
/// on: the client's and the engine's probes printed, then reset.
pub(crate) fn after_evaluation() {
    crate::prof::print();
    crate::prof::caches::print();
    crate::prof::pages::print();
    if phitex_defuse::obs::enabled() && prof_flag!("PHITEX_OBS") {
        eprint!("probes (evaluation):\n{:?}", phitex_defuse::obs::snapshot());
        phitex_defuse::obs::reset();
    }
}

/// The thread count: `given`, else `PHITEX_THREADS`, else the available
/// parallelism; 1 where this target cannot run threads.
pub(crate) fn threads(given: Option<usize>) -> usize {
    // (`PHITEX_THREADS`, read once)
    static ENV: OnceLock<Option<usize>> = OnceLock::new();
    if !threads_possible() {
        return 1;
    }
    given.filter(|&n| n > 0).unwrap_or_else(|| {
        ENV.get_or_init(|| {
            std::env::var("PHITEX_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n > 0)
        })
        .unwrap_or_else(cpus)
    })
}

/// Every logical CPU this process may run on (`available_parallelism`);
/// where the platform cannot say (WebAssembly's threads: the host knows,
/// `navigator.hardwareConcurrency`, and must pass it), one, said once on
/// stderr: a host that forgets runs the engine on one thread, and that is
/// never silent.
#[must_use]
pub fn cpus() -> usize {
    static SAID: std::sync::Once = std::sync::Once::new();
    std::thread::available_parallelism().map_or_else(
        |e| {
            SAID.call_once(|| {
                eprintln!("phitex: the platform does not say how many CPUs there are ({e}): one thread; give the count (-j N, PHITEX_THREADS or the host's threads)");
            });
            1
        },
        usize::from,
    )
}

/// Whether this target can run workers at all.
fn threads_possible() -> bool {
    if !THREADED {
        false
    } else if cfg!(target_arch = "wasm32") {
        cfg!(target_feature = "atomics") && cfg!(feature = "rayon") && SPAWN.get().is_some()
    } else {
        true
    }
}

/// A client pass's fan-out ([`map_on`]).
pub(crate) trait Fan {
    /// `f(i)` for every `i < n`.
    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync));
}

/// A client pass whose items each cost a run's worth (the hint pass's
/// chunks, spec-final §5.1): any batch of two or more split over the
/// pool of `self.0` workers, one item a task.
pub(crate) struct Coarse(pub usize);

impl Fan for Coarse {
    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if self.0 <= 1 || n < 2 || !threads_possible() {
            (0..n).for_each(f);
            return;
        }
        trace(n, self.0);
        parallel(self.0, n, 1, f);
    }
}

/// A fan-out inside an operation's run (the exact PDF's page contents,
/// the DVI file's pages): a batch split over `self.0` threads of its own,
/// never a pool's.
pub(crate) struct Inner(pub usize);

impl Fan for Inner {
    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        // (wasm32: `std::thread::spawn` is not there; the fan-out runs in
        // order)
        if self.0 <= 1 || n < 2 || !threads_possible() || cfg!(target_arch = "wasm32") {
            (0..n).for_each(f);
            return;
        }
        trace(n, self.0);
        // (threads of its own, not a pool's: a pool's `install` called
        // from an engine worker's thread runs that pool's other jobs
        // while it waits)
        scoped(self.0, n, 1, f);
    }
}

/// `f(i)` for every `i < n` on `ex` (its `for_each`), the results in
/// order.
pub(crate) fn map_on<T: Send + Sync>(
    ex: &mut dyn Fan,
    n: usize,
    f: &(dyn Fn(usize) -> T + Sync),
) -> Vec<T> {
    let slots: Vec<OnceLock<T>> = (0..n).map(|_| OnceLock::new()).collect();
    ex.for_each(n, &|i| {
        let _ = slots[i].set(f(i));
    });
    slots
        .into_iter()
        .map(|s| s.into_inner().expect("every job ran"))
        .collect()
}

fn trace(n: usize, threads: usize) {
    if obs_flag!("PHITEX_EXEC_TRACE") {
        eprintln!("exec: {n} jobs on {threads} threads");
    }
}

fn parallel(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    on_pool(threads, n, grain, f);
    flush_workers(threads);
}

/// `f(i)` for every `i < n` on the pool of `threads` workers.
#[cfg(feature = "rayon")]
fn on_pool(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    use rayon::prelude::*;
    pool(threads).install(|| {
        (0..n).into_par_iter().with_min_len(grain).for_each(f);
    });
}

/// Every worker of the pool of `threads` adds its probe counts and spans
/// to the totals (a no-op without the `profile` feature, so the broadcast
/// is skipped).
#[cfg(feature = "rayon")]
fn flush_workers(threads: usize) {
    if cfg!(feature = "profile") {
        pool(threads).broadcast(|_| {
            phitex_defuse::obs::flush();
            crate::prof::caches::flush();
        });
        phitex_defuse::obs::flush();
        crate::prof::caches::flush();
    }
}

/// The persistent pool of `threads` workers (an operation's fan-out runs
/// on threads of its own, [`Inner`]).
#[cfg(feature = "rayon")]
fn pool(threads: usize) -> std::sync::Arc<rayon::ThreadPool> {
    use std::sync::{Arc, Mutex, PoisonError};
    type Pools = Mutex<phitex_defuse::FxHashMap<usize, Arc<rayon::ThreadPool>>>;
    static POOLS: OnceLock<Pools> = OnceLock::new();
    let mut m = POOLS
        .get_or_init(|| Mutex::new(phitex_defuse::FxHashMap::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    m.entry(threads)
        .or_insert_with(|| {
            let b = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("phitex-{i}"));
            let pool = if let Some(spawn) = SPAWN.get() {
                b.spawn_handler(move |t| {
                    spawn(Box::new(move || t.run()));
                    Ok(())
                })
                .build()
            } else {
                b.build()
            };
            Arc::new(pool.expect("the worker pool"))
        })
        .clone()
}

#[cfg(not(feature = "rayon"))]
fn on_pool(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    scoped(threads, n, grain, f);
}

/// (The scoped threads flush their counts as they end.)
#[cfg(not(feature = "rayon"))]
fn flush_workers(_: usize) {}

/// `f(i)` for every `i < n` on `threads` threads of its own, made for
/// the call and joined at its end.
fn scoped(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let s = next.fetch_add(grain, Ordering::Relaxed);
            if s >= n {
                break;
            }
            for i in s..(s + grain).min(n) {
                f(i);
            }
        }
        phitex_defuse::obs::flush();
    };
    std::thread::scope(|sc| {
        for _ in 1..threads.min(n.div_ceil(grain)) {
            std::thread::Builder::new()
                .name("phitex-worker".to_owned())
                .stack_size(phitex_defuse::WORKER_STACK)
                .spawn_scoped(sc, work)
                .expect("a worker thread");
        }
        work();
    });
}

/// One operation's run, counted on the process's
/// [`OpCounters`] (busy time, runs at once); with `PHITEX_TEX_TIME`
/// also its operation, node and wall time, printed when the run ends.
pub(crate) struct NodeTimer {
    trace: Option<(std::time::Instant, &'static str, u64)>,
    ops: Option<(Option<u64>, usize)>,
}

impl NodeTimer {
    pub(crate) fn new(op: &'static str, node: u64) -> NodeTimer {
        // (the busy time and the runs at once are counters: `profile`
        // builds only, no write to the shared counters per run else)
        if !cfg!(feature = "profile") {
            return NodeTimer {
                trace: None,
                ops: None,
            };
        }
        let kind = KINDS.iter().position(|&k| k == op).unwrap_or(0);
        let o = counters();
        let c = o.running.fetch_add(1, Ordering::Relaxed) + 1;
        o.peak.fetch_max(c, Ordering::Relaxed);
        if prof_flag!("PHITEX_CLOCK_PROBE") {
            o.probe[kind][0].fetch_add(clock_probe(), Ordering::Relaxed);
        }
        let ops = Some((clock().now(), kind));
        let trace = (prof_flag!("PHITEX_TEX_TIME") || prof_flag!("PHITEX_RUN_TIMES"))
            .then(|| (std::time::Instant::now(), op, node));
        NodeTimer { trace, ops }
    }
}

impl Drop for NodeTimer {
    fn drop(&mut self) {
        if let Some((t0, kind)) = self.ops.take() {
            let o = counters();
            if let (Some(a), Some(b)) = (t0, clock().now()) {
                o.busy.fetch_add(b.saturating_sub(a), Ordering::Relaxed);
                o.kind_ns[kind].fetch_add(b.saturating_sub(a), Ordering::Relaxed);
            }
            o.kind_runs[kind].fetch_add(1, Ordering::Relaxed);
            o.running.fetch_sub(1, Ordering::Relaxed);
            if prof_flag!("PHITEX_CLOCK_PROBE") {
                o.probe[kind][1].fetch_add(clock_probe(), Ordering::Relaxed);
                o.probe[kind][2].fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some((t, op, node)) = self.trace {
            // (`PHITEX_RUN_TIMES=1`, profile: each run's start and end in
            // ms since the process's first run, for a timeline of an edit)
            if prof_flag!("PHITEX_RUN_TIMES") {
                static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                let z = *T0.get_or_init(|| t);
                eprintln!(
                    "runtime: {op} {node} {:.3} {:.3}",
                    t.saturating_duration_since(z).as_secs_f64() * 1e3,
                    z.elapsed().as_secs_f64() * 1e3
                );
            } else {
                later(format!(
                    "nodetime: {op} {node} {:.3}",
                    t.elapsed().as_secs_f64() * 1e3
                ));
            }
        }
    }
}

/// A lock whose waits are counted (`profile`): where a run finds it held,
/// the time it waits, by the call site ([`lock_waits`], the `acct: lock
/// waits` line). Without `profile`, a plain lock.
pub(crate) trait LockWait<T: ?Sized> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T: ?Sized> LockWait<T> for std::sync::Mutex<T> {
    #[track_caller]
    fn locked(&self) -> std::sync::MutexGuard<'_, T> {
        use std::sync::{PoisonError, TryLockError};
        if !cfg!(feature = "profile") {
            return self.lock().unwrap_or_else(PoisonError::into_inner);
        }
        match self.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let at = std::panic::Location::caller();
                let t = clock().now();
                let g = self.lock().unwrap_or_else(PoisonError::into_inner);
                if let (Some(a), Some(b)) = (t, clock().now()) {
                    note_wait(at, b.saturating_sub(a));
                }
                g
            }
        }
    }
}

/// The waits by call site: `(site, waits, nanoseconds)`.
static WAITS: std::sync::Mutex<Vec<(&'static std::panic::Location<'static>, u64, u64)>> =
    std::sync::Mutex::new(Vec::new());

fn note_wait(at: &'static std::panic::Location<'static>, ns: u64) {
    let mut w = WAITS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match w.iter_mut().find(|e| std::ptr::eq(e.0, at)) {
        Some(e) => {
            e.1 += 1;
            e.2 += ns;
        }
        None => w.push((at, 1, ns)),
    }
}

/// (`profile`) The lock waits so far by call site, longest first, and
/// cleared: `(file:line, waits, wait)`.
pub(crate) fn lock_waits() -> Vec<(String, u64, std::time::Duration)> {
    let mut w = std::mem::take(
        &mut *WAITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    w.sort_by_key(|e| std::cmp::Reverse(e.2));
    w.into_iter()
        .map(|(at, n, ns)| {
            (
                format!("{}:{}", at.file(), at.line()),
                n,
                std::time::Duration::from_nanos(ns),
            )
        })
        .collect()
}
