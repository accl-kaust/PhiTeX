//! Evaluation: N equal threads, a scheduler function, reads that park.
//!
//! Work is pieces (their runs), paragraphs (line breaking, memoised by
//! content), the page scan, and pages (DVI, memoised by content). Each
//! idle thread asks [`pick`] for work, does it, and asks again; no thread
//! is special. A piece's read of a name resolves to the latest definition
//! an earlier piece exported; if an earlier piece that could define it has
//! not run, the read **parks** (`Reads::Suspend`: the thread waits, or runs
//! that piece itself if nobody has started it) or **predicts** that it
//! does not define it (`Reads::Speculate`). A piece is **checked** (by any
//! thread, in parallel, once every piece before it has a run) when every
//! value it read is the value that reaches it; one that is not runs again.
//! When a run changes, only the readers of the names whose values changed
//! (found through a reverse index, name -> readers) lose their checks, and
//! only where no closer definition stands between. The commit only moves a
//! pointer over consecutive checked pieces.
//!
//! Whether two values are the same is decided by identity (the same `Arc`)
//! and then equality, never by hashes: a read is confirmed when the value
//! reaching it equals the value it read; a re-run piece's paragraph equal
//! to its previous run's keeps its lines; a page equal to the last build's
//! page at its place keeps its DVI.
//!
//! The definitions every run exported (the [`Names`] index) are read by
//! all threads at once; only a finished run writes to it. The scheduler's
//! own state is separate, held only to choose work and install results.
//!
//! An edit is the same evaluation: pieces whose text is unchanged keep
//! their last run, unconfirmed; those whose reads still match are kept,
//! the others run again. A cold build is an edit from nothing.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::dvi;
use crate::hash::{FxMap, hash_of};
use crate::linebreak::{Line, break_lines};
use crate::page::{Page, Scan, VItem, material};
use crate::piece::{Env, Para, PieceRun, cut, run_piece};
use crate::tfm::Font;
use crate::val::{Key, Val, initial};

/// What a read does when an earlier piece that could define its name has
/// not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    /// Park until it has (or run it here, if nobody has started it).
    Suspend,
    /// Predict that it does not define the name; confirmation checks.
    Speculate,
}

/// How the scheduler chooses among ready work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Earliest first; under `Suspend`, a piece starts only when nothing
    /// before it is still being read.
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
}

impl Default for Options {
    fn default() -> Self {
        Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub pieces: usize,
    /// Pieces run this build (first runs and runs again).
    pub ran: usize,
    /// Pieces whose last run was kept (their reads matched).
    pub kept: usize,
    /// Runs thrown away because a version they read was not the one that
    /// reaches them.
    pub reruns: usize,
    /// Reads that parked, and parks resolved by running the piece here.
    pub parks: usize,
    pub leapfrogs: usize,
    pub breaks_run: usize,
    /// Paragraphs equal to their piece's previous run's, keeping its lines.
    pub breaks_reused: usize,
    pub pages: usize,
    pub pages_shipped: usize,
    /// Pages equal to the last build's page at their place.
    pub pages_reused: usize,
    /// Work done by each thread (tokens read, items broken, scanned,
    /// shipped).
    pub work: Vec<u64>,
    pub tasks: Vec<u64>,
    /// Time spent by kind, all threads: runs, line breaking, the scan,
    /// shipping.
    pub time_run: Duration,
    pub time_break: Duration,
    pub time_scan: Duration,
    pub time_ship: Duration,
    pub time_confirm: Duration,
    /// Serial time: cutting the source, setting up, joining the DVI.
    pub time_cut: Duration,
    pub time_setup: Duration,
    pub time_join: Duration,
    pub wall: Duration,
}

pub struct Output {
    pub dvi: Vec<u8>,
    pub stats: Stats,
    pub errors: Vec<String>,
    /// The graph, as text.
    pub dump: String,
}

type Lines = Arc<Vec<Arc<Line>>>;

/// A piece as the last build left it: its text, its run, its paragraphs'
/// lines.
struct Kept {
    text: Arc<[char]>,
    run: Option<Arc<PieceRun>>,
    lines: Vec<Option<Lines>>,
}

/// What survives between builds: the pieces, and the pages with their DVI.
pub struct Build {
    font: Arc<Font>,
    pieces: Vec<Kept>,
    pages: Vec<(Page, Arc<Vec<u8>>)>,
}

const SHARDS: usize = 64;

type Defs = FxMap<Key, BTreeMap<usize, Val>>;

/// Every current run's exports (by name, then piece), and the pieces with
/// no run. Read by every thread at once, in shards; written only when a run
/// is installed or dropped. Readers that race with a writer may see a
/// stale answer: confirmation checks every read against the final one.
struct Names {
    shards: Vec<RwLock<Defs>>,
    unfinished: RwLock<BTreeSet<usize>>,
}

impl Names {
    fn new() -> Names {
        Names { shards: (0..SHARDS).map(|_| RwLock::default()).collect(), unfinished: RwLock::default() }
    }

    fn shard(&self, key: &Key) -> &RwLock<Defs> {
        // (the hash only routes to a shard; the map's `==` decides)
        &self.shards[(hash_of(key) % SHARDS as u64) as usize]
    }

    /// The definition of `key` reaching piece `k`, and (if asked) the
    /// nearest earlier piece with no run that could still define it.
    fn reaching(&self, key: &Key, k: usize, font: &Arc<Font>, blocking: bool) -> (Val, Option<usize>) {
        // (pieces with no run first: a run is added before it leaves them)
        let unf = if blocking { self.unfinished.read().expect("names").range(..k).next_back().copied() } else { None };
        let def = self.shard(key).read().expect("names").get(key).and_then(|m| m.range(..k).next_back()).map(|(j, v)| (*j, v.clone()));
        let blocked = match (unf, &def) {
            (Some(u), Some((j, _))) => (u > *j).then_some(u),
            (Some(u), None) => Some(u),
            (None, _) => None,
        };
        match def {
            Some((_, val)) => (val, blocked),
            None => (initial(key, font), blocked),
        }
    }

    /// Whether a piece strictly between `j` and `k` defines `key`.
    fn defines_between(&self, key: &Key, j: usize, k: usize) -> bool {
        k > j + 1 && self.shard(key).read().expect("names").get(key).is_some_and(|m| m.range(j + 1..k).next().is_some())
    }

    fn remove(&self, k: usize, run: &PieceRun) {
        self.unfinished.write().expect("names").insert(k);
        for (key, _) in &run.exports {
            if let Some(m) = self.shard(key).write().expect("names").get_mut(key) {
                m.remove(&k);
            }
        }
    }

    fn add(&self, k: usize, run: &PieceRun) {
        for (key, val) in &run.exports {
            self.shard(key).write().expect("names").entry(key.clone()).or_default().insert(k, val.clone());
        }
        self.unfinished.write().expect("names").remove(&k);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Unstarted,
    Running,
    Done,
}

struct PState {
    text: Arc<[char]>,
    status: Status,
    run: Option<Arc<PieceRun>>,
    /// The lines of each of the run's paragraphs, once broken.
    lines: Vec<Option<Lines>>,
    /// The run (and lines) a re-run replaces, to keep equal paragraphs'
    /// lines.
    prev: Option<(Arc<PieceRun>, Vec<Option<Lines>>)>,
    runs: u32,
    /// Its reads were found to be the values reaching it, and no earlier
    /// run has changed since.
    checked: bool,
    checking: bool,
}

/// The scheduler's state.
struct State {
    p: Vec<PState>,
    unstarted: BTreeSet<usize>,
    running: BTreeSet<usize>,
    /// Pieces before this one are confirmed.
    confirmed: usize,
    /// Every change of a run: (piece, epoch, the names whose values
    /// changed). A check started at an epoch is void if an earlier piece
    /// changed a name it read since.
    changes: Vec<(usize, u64, Arc<Vec<Key>>)>,
    epoch: u64,
    /// Who read each name: name -> pieces.
    readers: FxMap<Key, BTreeSet<usize>>,
    break_queue: VecDeque<(usize, usize, Arc<Para>)>,
    scan: Option<Scan>,
    scan_next: usize,
    scan_done: bool,
    pages: Vec<Page>,
    bodies: Vec<Option<Arc<Vec<u8>>>>,
    unshipped: usize,
    ship_queue: VecDeque<usize>,
    old_pages: Vec<(Page, Arc<Vec<u8>>)>,
    rng: u64,
    stats: Stats,
    done: bool,
}

struct Shared {
    names: Names,
    /// Bumped (and `parked` notified) whenever a run is installed.
    generation: Mutex<u64>,
    parked: Condvar,
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    opts: Options,
}

type ParaLines = (Arc<Para>, Lines);

enum Task {
    /// Check piece `.0`'s run (started at epoch `.2`).
    Check(usize, Arc<PieceRun>, u64),
    Run(usize),
    Break(usize, usize, Arc<Para>),
    Scan { scan: Scan, to: usize, pieces: Vec<Vec<ParaLines>>, finish: bool },
    Ship(usize, Page),
}

impl Build {
    pub fn new(font: Arc<Font>) -> Build {
        Build { font, pieces: Vec::new(), pages: Vec::new() }
    }

    /// Builds `src`: the pieces that changed, and what their changes reach.
    pub fn update(&mut self, src: &str, opts: Options) -> Output {
        let t0 = Instant::now();
        let threads = opts.threads.max(1);
        let texts = cut(src);
        let time_cut = t0.elapsed();
        // (old pieces found by their text: the map's hash only finds, `==`
        // decides)
        let mut old: FxMap<Arc<[char]>, VecDeque<(Arc<PieceRun>, Vec<Option<Lines>>)>> = FxMap::default();
        for kp in self.pieces.drain(..) {
            if let Some(r) = kp.run {
                old.entry(kp.text).or_default().push_back((r, kp.lines));
            }
        }
        let mut p = Vec::with_capacity(texts.len());
        let names = Names::new();
        let mut unstarted = BTreeSet::new();
        for (k, text) in texts.into_iter().enumerate() {
            let found = old.get_mut(&text).and_then(VecDeque::pop_front);
            let (run, lines) = match found {
                Some((r, l)) => {
                    names.add(k, &r);
                    (Some(r), l)
                }
                None => {
                    names.unfinished.write().expect("names").insert(k);
                    unstarted.insert(k);
                    (None, Vec::new())
                }
            };
            let status = if run.is_some() { Status::Done } else { Status::Unstarted };
            p.push(PState { text, status, run, lines, prev: None, runs: 0, checked: false, checking: false });
        }
        let n = p.len();
        let mut state = State {
            p,
            unstarted,
            running: BTreeSet::new(),
            confirmed: 0,
            changes: Vec::new(),
            epoch: 0,
            readers: FxMap::default(),
            break_queue: VecDeque::new(),
            scan: Some(Scan::default()),
            scan_next: 0,
            scan_done: false,
            pages: Vec::new(),
            bodies: Vec::new(),
            unshipped: 0,
            ship_queue: VecDeque::new(),
            old_pages: std::mem::take(&mut self.pages),
            rng: match opts.order {
                Order::Random(s) => s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
                Order::Priority => 1,
            },
            stats: Stats { pieces: n, work: vec![0; threads], tasks: vec![0; threads], ..Stats::default() },
            done: false,
        };
        // (kept runs' paragraphs that were never broken go to the queue)
        for k in 0..n {
            if let Some(r) = state.p[k].run.clone() {
                add_readers(&mut state, k, &r);
                if !lines_ready(&state, k) {
                    queue_breaks(&mut state, k);
                }
            }
        }
        state.stats.time_cut = time_cut;
        state.stats.time_setup = t0.elapsed() - time_cut;
        let sh = Shared {
            names,
            generation: Mutex::new(0),
            parked: Condvar::new(),
            state: Mutex::new(state),
            cv: Condvar::new(),
            font: self.font.clone(),
            opts,
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
        let mut st = sh.state.into_inner().expect("state");
        let t_join = Instant::now();
        let bodies: Vec<Arc<Vec<u8>>> = st.bodies.iter().map(|b| b.clone().expect("every page shipped")).collect();
        let out = dvi::assemble(&self.font, &st.pages, &bodies);
        let mut errors = Vec::new();
        let mut dump = String::new();
        for (k, ps) in st.p.iter().enumerate() {
            let r = ps.run.as_ref().expect("every piece ran");
            for e in &r.errors {
                errors.push(format!("piece {k}: {e}"));
            }
            if ps.runs == 0 {
                st.stats.kept += 1;
            }
            if opts.dump {
                dump_piece(&mut dump, k, ps, r);
            }
        }
        if opts.dump {
            for (i, pg) in st.pages.iter().enumerate() {
                let lines = pg.items.iter().filter(|x| matches!(x, VItem::Line(..))).count();
                let _ = writeln!(dump, "page {}: {} lines", i + 1, lines);
            }
        }
        st.stats.time_join = t_join.elapsed();
        st.stats.pages = st.pages.len();
        st.stats.wall = t0.elapsed();
        self.pieces = st.p.into_iter().map(|ps| Kept { text: ps.text, run: ps.run, lines: ps.lines }).collect();
        self.pages = st.pages.into_iter().zip(bodies).collect();
        Output { dvi: out, stats: st.stats, errors, dump }
    }
}

fn dump_piece(out: &mut String, k: usize, ps: &PState, r: &PieceRun) {
    let how = if ps.runs == 0 { "kept" } else { "ran" };
    let _ = writeln!(
        out,
        "piece {k} [{how}, {} steps]: {} reads, {} defines, {} paragraphs",
        r.steps,
        r.reads.len(),
        r.exports.len(),
        r.paras.len()
    );
    for (key, val) in &r.reads {
        let _ = writeln!(out, "  read   {key} = {val}");
    }
    for (key, val) in &r.exports {
        let _ = writeln!(out, "  define {key} = {val}");
    }
    for phi in &r.phis {
        let _ = write!(out, "  φ test={} (untaken arm: {} tokens)", phi.test, phi.skipped);
        for (key, after, before) in &phi.keys {
            let _ = write!(out, " {key} = φ({after} | {before})");
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
        drop(g);
        let t = Instant::now();
        let (work, kind) = execute(sh, task);
        let dt = t.elapsed();
        g = lock(sh);
        g.stats.work[id] += work;
        match kind {
            0 => g.stats.time_run += dt,
            1 => g.stats.time_break += dt,
            2 => g.stats.time_scan += dt,
            3 => g.stats.time_ship += dt,
            _ => g.stats.time_confirm += dt,
        }
    }
}

fn finished(g: &State) -> bool {
    g.confirmed == g.p.len() && g.scan_done && g.unshipped == 0
}

fn add_readers(g: &mut State, k: usize, run: &PieceRun) {
    for (key, _) in &run.reads {
        g.readers.entry(key.clone()).or_default().insert(k);
    }
}

fn remove_readers(g: &mut State, k: usize, run: &PieceRun) {
    for (key, _) in &run.reads {
        if let Some(r) = g.readers.get_mut(key) {
            r.remove(&k);
        }
    }
}

/// The names whose exported values differ between two runs of a piece.
fn changed_names(old: Option<&PieceRun>, new: Option<&PieceRun>) -> Vec<Key> {
    let empty = Vec::new();
    let a = old.map_or(&empty, |r| &r.exports);
    let b = new.map_or(&empty, |r| &r.exports);
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

/// Piece `j`'s exports changed in `keys`: the later readers those
/// definitions reach lose their checks; nothing else does.
fn changed(g: &mut State, sh: &Shared, j: usize, keys: Vec<Key>) {
    if keys.is_empty() {
        return;
    }
    g.epoch += 1;
    let e = g.epoch;
    for key in &keys {
        let Some(readers) = g.readers.get(key) else { continue };
        let reached: Vec<usize> = readers
            .range(j + 1..)
            .copied()
            .take_while(|&k| !sh.names.defines_between(key, j, k))
            .collect();
        for k in reached {
            g.p[k].checked = false;
        }
    }
    g.changes.push((j, e, Arc::new(keys)));
}

/// Checks a piece's run (outside the scheduler's lock, in parallel with
/// other checks): is every value it read the value reaching it? One that
/// is not loses its run.
fn check(sh: &Shared, k: usize, run: &Arc<PieceRun>, epoch: u64) -> u64 {
    // (the same definition is the same `Arc`: equality is a pointer test)
    let ok = run.reads.iter().all(|(key, val)| sh.names.reaching(key, k, &sh.font, false).0 == *val);
    let mut g = lock(sh);
    g.p[k].checking = false;
    let same = g.p[k].run.as_ref().is_some_and(|r| Arc::ptr_eq(r, run));
    // (void if an earlier piece changed a name it read since it started,
    // or if an earlier piece has no run now)
    let void = g.changes.iter().rev().take_while(|(_, e, _)| *e > epoch).any(|(j, _, keys)| {
        *j < k && keys.iter().any(|key| run.reads.iter().any(|(r, _)| r == key))
    }) || g.unstarted.range(..k).next().is_some()
        || g.running.range(..k).next().is_some();
    if same && !void {
        if ok {
            g.p[k].checked = true;
        } else {
            sh.names.remove(k, run);
            remove_readers(&mut g, k, run);
            let lines = std::mem::take(&mut g.p[k].lines);
            g.p[k].prev = Some((run.clone(), lines));
            g.p[k].run = None;
            g.p[k].status = Status::Unstarted;
            g.unstarted.insert(k);
            g.stats.reruns += 1;
            // (no reader past it is invalidated yet: none can be committed
            // while it has no run, and when it runs again only the names
            // whose values differ from this run's reach their readers)
        }
    }
    drop(g);
    sh.cv.notify_all();
    run.reads.len() as u64
}

/// Gives each of piece `k`'s paragraphs its lines: those equal to a
/// paragraph of the previous run keep its lines; the others are queued.
fn queue_breaks(g: &mut State, k: usize) {
    let run = g.p[k].run.clone().expect("a run");
    let prev = g.p[k].prev.take();
    let mut lines = std::mem::take(&mut g.p[k].lines);
    lines.resize(run.paras.len(), None);
    for (i, para) in run.paras.iter().enumerate() {
        if lines[i].is_some() {
            continue;
        }
        let reuse = prev.as_ref().and_then(|(pr, pl)| {
            let same = |j: usize| {
                pr.paras
                    .get(j)
                    .filter(|q| Arc::ptr_eq(q, para) || ***q == **para)
                    .and_then(|_| pl.get(j).cloned().flatten())
            };
            same(i).or_else(|| (0..pr.paras.len()).find_map(same))
        });
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

fn lines_ready(g: &State, k: usize) -> bool {
    let ps = &g.p[k];
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

/// The scheduler: a function of the state, choosing this thread's next
/// piece of work.
fn pick(g: &mut State, sh: &Shared) -> Option<Task> {
    let n = g.p.len();
    // (the commit: a pointer over consecutive checked pieces)
    while g.confirmed < n && g.p[g.confirmed].checked {
        g.confirmed += 1;
    }
    // (checks next: short, and the commit waits on them; any piece all of
    // whose predecessors have runs)
    let mut checks: Vec<usize> = Vec::new();
    let mut k = g.confirmed;
    while k < n && g.p[k].status == Status::Done {
        if !g.p[k].checked && !g.p[k].checking {
            checks.push(k);
            if sh.opts.order == Order::Priority {
                break;
            }
        }
        k += 1;
    }
    if !checks.is_empty() {
        let k = match sh.opts.order {
            Order::Priority => checks[0],
            Order::Random(_) => checks[rand(g, checks.len())],
        };
        g.p[k].checking = true;
        let run = g.p[k].run.clone().expect("a done piece has a run");
        return Some(Task::Check(k, run, g.epoch));
    }
    let first_unstarted = g.unstarted.first().copied();
    let run_ok = match (sh.opts.reads, sh.opts.order, first_unstarted) {
        (_, _, None) => false,
        (Reads::Suspend, Order::Priority, Some(k)) => g.running.range(..k).next().is_none(),
        _ => true,
    };
    let mut scan_to = g.scan_next;
    while scan_to < g.confirmed && lines_ready(g, scan_to) {
        scan_to += 1;
    }
    let scan_finish = g.scan_next == n && g.confirmed == n && !g.scan_done;
    let scan_ok = g.scan.is_some() && (scan_to > g.scan_next || scan_finish);
    // (priority: pages out first, then the scan, the reading, line breaking)
    let mut kinds = [0u8; 4];
    let mut nk = 0;
    for (ok, kind) in [(!g.ship_queue.is_empty(), 3u8), (scan_ok, 2), (run_ok, 0), (!g.break_queue.is_empty(), 1)] {
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
            let k = match sh.opts.order {
                Order::Random(_) => {
                    let i = rand(g, g.unstarted.len());
                    *g.unstarted.iter().nth(i).expect("an unstarted piece")
                }
                Order::Priority => first_unstarted.expect("an unstarted piece"),
            };
            g.unstarted.remove(&k);
            g.running.insert(k);
            g.p[k].status = Status::Running;
            Some(Task::Run(k))
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
                    let r = ps.run.as_ref().expect("a confirmed piece has a run");
                    r.paras
                        .iter()
                        .zip(&ps.lines)
                        .map(|(p, l)| (p.clone(), l.clone().expect("lines ready")))
                        .collect()
                })
                .collect();
            let scan = g.scan.take().expect("the scan is free");
            Some(Task::Scan { scan, to: scan_to, pieces, finish: scan_finish })
        }
        _ => {
            let i = match sh.opts.order {
                Order::Random(_) => rand(g, g.ship_queue.len()),
                Order::Priority => 0,
            };
            let pg = g.ship_queue.remove(i).expect("a page to ship");
            Some(Task::Ship(pg, g.pages[pg].clone()))
        }
    }
}

/// Does a task (outside the scheduler's lock) and installs its result.
/// Returns the work done and its kind.
fn execute(sh: &Shared, task: Task) -> (u64, u8) {
    match task {
        Task::Check(k, run, epoch) => (check(sh, k, &run, epoch), 4),
        Task::Run(k) => (run_and_install(sh, k), 0),
        Task::Break(k, j, para) => {
            let lines = Arc::new(break_lines(&para));
            let mut g = lock(sh);
            // (the piece may have run again meanwhile: only its own paragraph)
            let current = g.p[k].run.as_ref().and_then(|r| r.paras.get(j)).is_some_and(|q| Arc::ptr_eq(q, &para));
            if current {
                g.p[k].lines[j] = Some(lines);
            }
            g.stats.breaks_run += 1;
            drop(g);
            sh.cv.notify_all();
            (para.hlist.len() as u64, 1)
        }
        Task::Scan { mut scan, to, pieces, finish } => {
            let before = scan.fed;
            for paras in &pieces {
                for it in material(paras) {
                    scan.feed(it);
                }
            }
            if finish {
                scan.finish();
            }
            let work = scan.fed - before;
            let new: Vec<Page> = std::mem::take(&mut scan.pages);
            let mut g = lock(sh);
            g.scan_next = to;
            for pg in new {
                let i = g.pages.len();
                // (unchanged lines are the same `Arc`s: a cheap comparison)
                let same = g.old_pages.get(i).filter(|(op, _)| *op == pg).map(|(_, b)| b.clone());
                g.pages.push(pg);
                if let Some(b) = same {
                    g.bodies.push(Some(b));
                    g.stats.pages_reused += 1;
                } else {
                    g.bodies.push(None);
                    g.unshipped += 1;
                    g.ship_queue.push_back(i);
                }
            }
            if finish {
                g.scan_done = true;
            }
            g.scan = Some(scan);
            drop(g);
            sh.cv.notify_all();
            (work, 2)
        }
        Task::Ship(i, page) => {
            let body = Arc::new(dvi::page_body(&page));
            let mut g = lock(sh);
            g.bodies[i] = Some(body);
            g.unshipped -= 1;
            g.stats.pages_shipped += 1;
            drop(g);
            sh.cv.notify_all();
            (page.items.len() as u64, 3)
        }
    }
}

/// Runs piece `k` (marked running by whoever chose it) and installs its
/// run.
fn run_and_install(sh: &Shared, k: usize) -> u64 {
    let text = lock(sh).p[k].text.clone();
    let mut env = PieceEnv { sh, k };
    let run = Arc::new(run_piece(&text, &mut env));
    let steps = run.steps;
    {
        let mut g = lock(sh);
        sh.names.add(k, &run);
        add_readers(&mut g, k, &run);
        let old = g.p[k].prev.as_ref().map(|(r, _)| r.clone());
        let keys = changed_names(old.as_deref(), Some(&run));
        g.p[k].run = Some(run);
        g.p[k].checked = false;
        changed(&mut g, sh, k, keys);
        queue_breaks(&mut g, k);
        g.p[k].status = Status::Done;
        g.p[k].runs += 1;
        g.running.remove(&k);
        g.stats.ran += 1;
    }
    *sh.generation.lock().expect("generation") += 1;
    sh.parked.notify_all();
    sh.cv.notify_all();
    steps
}

/// A piece's reads of names from outside it.
struct PieceEnv<'a> {
    sh: &'a Shared,
    k: usize,
}

impl Env for PieceEnv<'_> {
    fn reach(&mut self, key: &Key) -> Val {
        let sh = self.sh;
        let suspend = sh.opts.reads == Reads::Suspend;
        // (under `Speculate`, a read never blocks: it predicts that no piece
        // without a run defines the name, and confirmation checks)
        let (val, blocked) = sh.names.reaching(key, self.k, &sh.font, suspend);
        if blocked.is_none() {
            return val;
        }
        loop {
            let seen = *sh.generation.lock().expect("generation");
            let (val, blocked) = sh.names.reaching(key, self.k, &sh.font, true);
            let Some(u) = blocked else { return val };
            let leapfrog = {
                let mut g = lock(sh);
                g.stats.parks += 1;
                if g.p[u].status == Status::Unstarted {
                    g.p[u].status = Status::Running;
                    g.unstarted.remove(&u);
                    g.running.insert(u);
                    g.stats.leapfrogs += 1;
                    true
                } else {
                    false
                }
            };
            if leapfrog {
                run_and_install(sh, u);
            } else {
                let mut g = sh.generation.lock().expect("generation");
                while *g == seen {
                    g = sh.parked.wait(g).expect("generation");
                }
            }
        }
    }
}
