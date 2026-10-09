//! Evaluation: N equal threads, a scheduler function, reads that park.
//!
//! Work is pieces (their runs), paragraphs (line breaking, memoised by
//! content), the page scan, and pages (DVI, memoised by content). Each
//! idle thread asks [`pick`] for work, does it, and asks again; no thread
//! is special. A piece's read of a name resolves to the latest definition
//! an earlier piece exported; if an earlier piece that could define it has
//! not run, the read **parks** (`Reads::Suspend`: the thread waits, or runs
//! that piece itself if nobody has started it) or **predicts** that it
//! does not define it (`Reads::Speculate`). A piece is confirmed, in
//! order, when every version it read is the version that reaches it; one
//! that is not runs again. Confirmation only compares versions.
//!
//! An edit is the same evaluation: pieces whose text is unchanged keep
//! their last run, unconfirmed; those whose reads still match are kept,
//! the others run again. A cold build is an edit from nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::dvi;
use crate::hash::hash_of;
use crate::linebreak::{Line, break_lines};
use crate::page::{Page, Scan, VItem, material};
use crate::piece::{Env, Para, PieceRun, cut, run_piece};
use crate::tfm::Font;
use crate::val::{Key, Val, Ver, initial};

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
    /// before it is still unread.
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
}

impl Default for Options {
    fn default() -> Self {
        Options { threads: 1, reads: Reads::Suspend, order: Order::Priority }
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
    pub breaks_memo: usize,
    pub pages: usize,
    pub pages_shipped: usize,
    pub pages_memo: usize,
    /// Work done by each thread (tokens read, items broken, scanned).
    pub work: Vec<u64>,
    pub tasks: Vec<u64>,
    pub wall: Duration,
}

pub struct Output {
    pub dvi: Vec<u8>,
    pub stats: Stats,
    pub errors: Vec<String>,
    /// The graph, as text.
    pub dump: String,
}

/// What survives between builds: each piece's text and last run, and the
/// memos (line breaks and pages by content).
pub struct Build {
    font: Arc<Font>,
    pieces: Vec<(Arc<[char]>, u64, Option<Arc<PieceRun>>)>,
    lines: HashMap<u64, Arc<Vec<Arc<Line>>>>,
    pages: HashMap<u64, Arc<Vec<u8>>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Unstarted,
    Running,
    Done,
}

struct PState {
    text: Arc<[char]>,
    hash: u64,
    status: Status,
    run: Option<Arc<PieceRun>>,
    runs: u32,
}

struct State {
    p: Vec<PState>,
    /// Every current run's exports: name -> piece -> (version, value).
    defs: HashMap<Key, BTreeMap<usize, (Ver, Val)>>,
    /// Pieces with no run.
    unfinished: BTreeSet<usize>,
    /// Pieces before this one are confirmed.
    confirmed: usize,
    lines: HashMap<u64, Arc<Vec<Arc<Line>>>>,
    breaking: HashSet<u64>,
    break_queue: VecDeque<(u64, Arc<Para>)>,
    scan: Option<Scan>,
    scan_next: usize,
    scan_done: bool,
    pages: Vec<Page>,
    bodies: Vec<Option<Arc<Vec<u8>>>>,
    page_memo: HashMap<u64, Arc<Vec<u8>>>,
    shipping: HashSet<usize>,
    rng: u64,
    stats: Stats,
    done: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    opts: Options,
}

enum Task {
    Run(usize),
    Break(u64, Arc<Para>),
    Scan { scan: Scan, to: usize, items: Vec<VItem>, finish: bool },
    Ship(usize, u64, Page),
}

impl Build {
    pub fn new(font: Arc<Font>) -> Build {
        Build { font, pieces: Vec::new(), lines: HashMap::new(), pages: HashMap::new() }
    }

    /// Builds `src`: the pieces that changed, and what their changes reach.
    pub fn update(&mut self, src: &str, opts: Options) -> Output {
        let t0 = Instant::now();
        let threads = opts.threads.max(1);
        let texts = cut(src);
        let mut old: HashMap<u64, VecDeque<Arc<PieceRun>>> = HashMap::new();
        for (_, h, run) in self.pieces.drain(..) {
            if let Some(r) = run {
                old.entry(h).or_default().push_back(r);
            }
        }
        let mut p = Vec::with_capacity(texts.len());
        let mut defs: HashMap<Key, BTreeMap<usize, (Ver, Val)>> = HashMap::new();
        let mut unfinished = BTreeSet::new();
        for (k, text) in texts.into_iter().enumerate() {
            let hash = hash_of(&*text);
            let run = old.get_mut(&hash).and_then(VecDeque::pop_front);
            match &run {
                Some(r) => {
                    for (key, ver, val) in &r.exports {
                        defs.entry(key.clone()).or_default().insert(k, (*ver, val.clone()));
                    }
                }
                None => {
                    unfinished.insert(k);
                }
            }
            let status = if run.is_some() { Status::Done } else { Status::Unstarted };
            p.push(PState { text, hash, status, run, runs: 0 });
        }
        let n = p.len();
        let state = State {
            p,
            defs,
            unfinished,
            confirmed: 0,
            lines: std::mem::take(&mut self.lines),
            breaking: HashSet::new(),
            break_queue: VecDeque::new(),
            scan: Some(Scan::default()),
            scan_next: 0,
            scan_done: false,
            pages: Vec::new(),
            bodies: Vec::new(),
            page_memo: std::mem::take(&mut self.pages),
            shipping: HashSet::new(),
            rng: match opts.order {
                Order::Random(s) => s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
                Order::Priority => 1,
            },
            stats: Stats { pieces: n, work: vec![0; threads], tasks: vec![0; threads], ..Stats::default() },
            done: false,
        };
        let sh = Shared { state: Mutex::new(state), cv: Condvar::new(), font: self.font.clone(), opts };
        {
            // (kept runs' paragraphs that were never broken go to the queue)
            let mut g = sh.state.lock().expect("state");
            for k in 0..n {
                if let Some(r) = g.p[k].run.clone() {
                    queue_breaks(&mut g, &r);
                }
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
        let mut st = sh.state.into_inner().expect("state");
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
            dump_piece(&mut dump, k, ps, r);
        }
        for (i, pg) in st.pages.iter().enumerate() {
            let lines = pg.items.iter().filter(|x| matches!(x, VItem::Line(..))).count();
            let _ = writeln!(dump, "page {}: {} lines, content {:016x}", i + 1, lines, hash_of(pg));
        }
        st.stats.pages = st.pages.len();
        st.stats.wall = t0.elapsed();
        self.pieces = st.p.into_iter().map(|ps| (ps.text, ps.hash, ps.run)).collect();
        self.lines = st.lines;
        self.pages = st.page_memo;
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
    for (key, ver) in &r.reads {
        let _ = writeln!(out, "  read   {key} = {ver}");
    }
    for (key, ver, _) in &r.exports {
        let _ = writeln!(out, "  define {key} = {ver}");
    }
    for phi in &r.phis {
        let _ = write!(out, "  φ test={} (untaken arm: {} tokens)", phi.test, phi.skipped);
        for (key, after, before) in &phi.keys {
            let _ = write!(out, " {key} = φ({after}, {before})");
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
        if confirm(&mut g, &sh.font) {
            sh.cv.notify_all();
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
        let work = execute(sh, task);
        g = lock(sh);
        g.stats.work[id] += work;
    }
}

fn finished(g: &State) -> bool {
    g.confirmed == g.p.len() && g.scan_done && g.bodies.iter().all(Option::is_some)
}

/// The thin, ordered commit: confirms pieces whose read versions are the
/// versions that reach them. A piece that read another version loses its
/// run. Returns whether anything changed.
fn confirm(g: &mut State, font: &Arc<Font>) -> bool {
    let mut changed = false;
    while g.confirmed < g.p.len() && g.p[g.confirmed].status == Status::Done {
        let k = g.confirmed;
        let run = g.p[k].run.clone().expect("a done piece has a run");
        let ok = run.reads.iter().all(|(key, ver)| reaching(g, key, k, font).0 == *ver);
        changed = true;
        if ok {
            g.confirmed += 1;
        } else {
            drop_run(g, k);
            g.stats.reruns += 1;
            break;
        }
    }
    changed
}

/// The definition of `key` reaching piece `k` among the current runs, and
/// the nearest earlier piece with no run that could still define it.
fn reaching(g: &State, key: &Key, k: usize, font: &Arc<Font>) -> (Ver, Val, Option<usize>) {
    let def = g.defs.get(key).and_then(|m| m.range(..k).next_back());
    let unf = g.unfinished.range(..k).next_back().copied();
    let blocked = match (unf, def) {
        (Some(u), Some((j, _))) => (u > *j).then_some(u),
        (Some(u), None) => Some(u),
        (None, _) => None,
    };
    match def {
        Some((_, (ver, val))) => (*ver, val.clone(), blocked),
        None => {
            let v = initial(key, font);
            (v.ver(), v, blocked)
        }
    }
}

fn drop_run(g: &mut State, k: usize) {
    if let Some(r) = g.p[k].run.take() {
        for (key, _, _) in &r.exports {
            if let Some(m) = g.defs.get_mut(key) {
                m.remove(&k);
            }
        }
    }
    g.p[k].status = Status::Unstarted;
    g.unfinished.insert(k);
}

fn install(g: &mut State, k: usize, run: Arc<PieceRun>) {
    if g.p[k].run.is_some() {
        drop_run(g, k);
    }
    for (key, ver, val) in &run.exports {
        g.defs.entry(key.clone()).or_default().insert(k, (*ver, val.clone()));
    }
    queue_breaks(g, &run);
    g.p[k].run = Some(run);
    g.p[k].status = Status::Done;
    g.p[k].runs += 1;
    g.stats.ran += 1;
    g.unfinished.remove(&k);
}

fn queue_breaks(g: &mut State, run: &Arc<PieceRun>) {
    for (para, &h) in run.paras.iter().zip(&run.para_hashes) {
        if g.lines.contains_key(&h) {
            g.stats.breaks_memo += 1;
        } else if g.breaking.insert(h) {
            g.break_queue.push_back((h, para.clone()));
        }
    }
}

fn lines_ready(g: &State, k: usize) -> bool {
    g.p[k].run.as_ref().is_some_and(|r| r.para_hashes.iter().all(|h| g.lines.contains_key(h)))
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
    // (pages already shipped once are taken from the memo here)
    for i in 0..g.pages.len() {
        if g.bodies[i].is_none() && !g.shipping.contains(&i) {
            let h = hash_of(&g.pages[i]);
            if let Some(b) = g.page_memo.get(&h).cloned() {
                g.bodies[i] = Some(b);
                g.stats.pages_memo += 1;
            }
        }
    }
    let n = g.p.len();
    let mut kinds: Vec<u8> = Vec::with_capacity(4);
    // runs
    let first_unstarted = g.p.iter().position(|x| x.status == Status::Unstarted);
    let run_ok = match (sh.opts.reads, sh.opts.order, first_unstarted) {
        (_, _, None) => false,
        (Reads::Suspend, Order::Priority, Some(k)) => !g.p[..k].iter().any(|x| x.status == Status::Running),
        _ => true,
    };
    if run_ok {
        kinds.push(0);
    }
    if !g.break_queue.is_empty() {
        kinds.push(1);
    }
    let mut scan_to = g.scan_next;
    while scan_to < g.confirmed && lines_ready(g, scan_to) {
        scan_to += 1;
    }
    let scan_finish = g.scan_next == n && g.confirmed == n && !g.scan_done;
    if g.scan.is_some() && (scan_to > g.scan_next || scan_finish) {
        kinds.push(2);
    }
    let ship = (0..g.pages.len()).find(|&i| g.bodies[i].is_none() && !g.shipping.contains(&i));
    if ship.is_some() {
        kinds.push(3);
    }
    if kinds.is_empty() {
        return None;
    }
    let kind = match sh.opts.order {
        Order::Priority => kinds[0],
        Order::Random(_) => {
            let i = rand(g, kinds.len());
            kinds[i]
        }
    };
    match kind {
        0 => {
            let k = match sh.opts.order {
                Order::Random(_) => {
                    let cands: Vec<usize> = (0..n).filter(|&i| g.p[i].status == Status::Unstarted).collect();
                    let i = rand(g, cands.len());
                    cands[i]
                }
                Order::Priority => first_unstarted.expect("an unstarted piece"),
            };
            g.p[k].status = Status::Running;
            Some(Task::Run(k))
        }
        1 => {
            let i = match sh.opts.order {
                Order::Random(_) => {
                    let len = g.break_queue.len();
                    rand(g, len)
                }
                Order::Priority => 0,
            };
            let (h, para) = g.break_queue.remove(i).expect("a queued paragraph");
            Some(Task::Break(h, para))
        }
        2 => {
            let mut items = Vec::new();
            for k in g.scan_next..scan_to {
                let r = g.p[k].run.clone().expect("a confirmed piece has a run");
                let paras: Vec<(Arc<Para>, Arc<Vec<Arc<Line>>>)> = r
                    .paras
                    .iter()
                    .zip(&r.para_hashes)
                    .map(|(p, h)| (p.clone(), g.lines[h].clone()))
                    .collect();
                items.extend(material(&paras));
            }
            let scan = g.scan.take().expect("the scan is free");
            Some(Task::Scan { scan, to: scan_to, items, finish: scan_finish })
        }
        _ => {
            let i = ship.expect("a page to ship");
            g.shipping.insert(i);
            let h = hash_of(&g.pages[i]);
            Some(Task::Ship(i, h, g.pages[i].clone()))
        }
    }
}

/// Does a task (outside the lock) and installs its result.
fn execute(sh: &Shared, task: Task) -> u64 {
    match task {
        Task::Run(k) => run_and_install(sh, k),
        Task::Break(h, para) => {
            let lines = Arc::new(break_lines(&para));
            let mut g = lock(sh);
            g.lines.insert(h, lines);
            g.breaking.remove(&h);
            g.stats.breaks_run += 1;
            drop(g);
            sh.cv.notify_all();
            para.hlist.len() as u64
        }
        Task::Scan { mut scan, to, items, finish } => {
            let before = scan.fed;
            for it in items {
                scan.feed(it);
            }
            if finish {
                scan.finish();
            }
            let work = scan.fed - before;
            let new: Vec<Page> = std::mem::take(&mut scan.pages);
            let mut g = lock(sh);
            g.scan_next = to;
            for pg in new {
                g.pages.push(pg);
                g.bodies.push(None);
            }
            if finish {
                g.scan_done = true;
            }
            g.scan = Some(scan);
            drop(g);
            sh.cv.notify_all();
            work
        }
        Task::Ship(i, h, page) => {
            let body = Arc::new(dvi::page_body(&page));
            let mut g = lock(sh);
            g.page_memo.insert(h, body.clone());
            g.bodies[i] = Some(body);
            g.shipping.remove(&i);
            g.stats.pages_shipped += 1;
            drop(g);
            sh.cv.notify_all();
            page.items.len() as u64
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
    let mut g = lock(sh);
    install(&mut g, k, run);
    drop(g);
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
        let mut g = lock(sh);
        loop {
            let (_, val, blocked) = reaching(&g, key, self.k, &sh.font);
            let Some(u) = blocked else { return val };
            if sh.opts.reads == Reads::Speculate {
                // (predict: no piece without a run defines it; confirmed later)
                return val;
            }
            g.stats.parks += 1;
            if g.p[u].status == Status::Unstarted {
                g.p[u].status = Status::Running;
                g.stats.leapfrogs += 1;
                drop(g);
                run_and_install(sh, u);
                g = lock(sh);
            } else {
                g = sh.cv.wait(g).expect("state");
            }
        }
    }
}
