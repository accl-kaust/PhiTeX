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
//! whose values changed are marked **dirty**, node by node (those after it
//! that read the name from before it), and their pieces run again from
//! those nodes. A piece's own reads are validated when it is installed,
//! against the index as it is then; a stale read marks its node and the
//! piece runs again from it. Under `Reads::Suspend` a read of a name whose
//! definition is being remade parks until that definition is installed
//! (or runs the defining piece itself if nobody has started it); under
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

use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::dvi;
use crate::graph::{Env, Para, PieceId, Run, Src, cut, run_piece};
use crate::hash::{FxMap, hash_of};
use crate::linebreak::{Line, break_lines};
use crate::page::{Page, Scan, material};
use crate::tfm::{Font, FontFiles};
use crate::val::{Key, Val, initial};

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
}

impl Default for Options {
    fn default() -> Self {
        Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false }
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
    pub breaks_run: usize,
    /// Paragraphs equal to one already broken, keeping its lines.
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
    /// shipping, installing (validation and marking).
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
    /// What the nodes wrote to the log, in document order (`\message`
    /// texts; `\immediate\write` lines, ending in a newline):
    /// `print::log` lays it out as TeX's log.
    pub log: Vec<String>,
}

type Lines = Arc<Vec<Arc<Line>>>;

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
}

/// The edges: name -> reader piece -> (its first node reading the name
/// from before the piece, the piece whose definition it read).
type Readers = FxMap<Key, FxMap<PieceId, (u32, PieceId)>>;

/// What survives between builds: the pieces with their nodes, the edges,
/// the pages with their DVI.
pub struct Build {
    font: Arc<Font>,
    /// The TFM files `\font` can load.
    fonts: Arc<FontFiles>,
    pieces: Vec<Kept>,
    pages: Vec<(Page, Arc<Vec<u8>>)>,
    next_id: PieceId,
    readers: Readers,
}

const SHARDS: usize = 64;

type Defs = FxMap<Key, Vec<(u32, Val)>>;

/// Every installed run's exports: name -> (piece position, value), by
/// position. Read by every thread at once, in shards; written only when a
/// run is installed, under the scheduler's lock.
struct Names {
    shards: Vec<RwLock<Defs>>,
}

impl Names {
    fn new() -> Names {
        Names { shards: (0..SHARDS).map(|_| RwLock::default()).collect() }
    }

    fn shard(&self, key: &Key) -> &RwLock<Defs> {
        // (the hash only routes to a shard; the map's `==` decides)
        &self.shards[(hash_of(key) % SHARDS as u64) as usize]
    }

    /// The definition of `key` reaching piece `k`: its piece's position
    /// (none: the initial value) and its value.
    fn reaching(&self, key: &Key, k: usize, font: &Arc<Font>) -> (Option<u32>, Val) {
        let g = self.shard(key).read().expect("names");
        if let Some(v) = g.get(key) {
            let i = v.partition_point(|(p, _)| (*p as usize) < k);
            if i > 0 {
                return (Some(v[i - 1].0), v[i - 1].1.clone());
            }
        }
        (None, initial(key, font))
    }

    /// Piece `k` now defines `key` as `val` (none: no longer defines it).
    fn set(&self, k: usize, key: &Key, val: Option<&Val>) {
        let mut g = self.shard(key).write().expect("names");
        let v = g.entry(key.clone()).or_default();
        let i = v.partition_point(|(p, _)| (*p as usize) < k);
        let here = v.get(i).is_some_and(|(p, _)| *p as usize == k);
        match (val, here) {
            (Some(val), true) => v[i].1 = val.clone(),
            (Some(val), false) => v.insert(i, (k as u32, val.clone())),
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
    prev: Option<(Arc<Run>, Vec<Option<Lines>>, Arc<[char]>)>,
    /// Nodes of its run that read a definition since remade.
    dirty: BTreeSet<u32>,
    runs: u32,
}

/// The scheduler's state.
struct State {
    p: Vec<PState>,
    unstarted: BTreeSet<usize>,
    running: BTreeSet<usize>,
    /// Pieces before this one cannot change any more in this build: the
    /// page scan reads them.
    settled: usize,
    readers: Readers,
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
    /// Position -> piece, and piece -> position (`NONE`: not in this
    /// build), fixed for the build.
    ids: Vec<PieceId>,
    pos_of: Vec<u32>,
    /// Bumped (and `parked` notified) whenever a run is installed.
    generation: Mutex<u64>,
    parked: Condvar,
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    fonts: Arc<FontFiles>,
    opts: Options,
}

type ParaLines = (Arc<Para>, Lines);

enum Task {
    /// Run piece `.0` from its dirty nodes `.1`.
    Run(usize, Vec<u32>),
    Break(usize, usize, Arc<Para>),
    Scan { scan: Scan, to: usize, pieces: Vec<Vec<ParaLines>>, finish: bool },
    Ship(usize, Page),
}

impl Build {
    /// A build whose documents can load `font` by its name, and the TFM
    /// files in TeX Live's directories.
    pub fn new(font: Arc<Font>) -> Build {
        let fonts = Arc::new(FontFiles::new(Self::given(&font), crate::tfm::default_dirs()));
        Build { font, fonts, pieces: Vec::new(), pages: Vec::new(), next_id: 0, readers: FxMap::default() }
    }

    fn given(font: &Font) -> Vec<(String, Arc<[u8]>)> {
        if font.data.is_empty() { Vec::new() } else { vec![(font.name.clone(), font.data.clone())] }
    }

    /// Where `\font` finds TFM files (searched below these directories).
    pub fn set_font_dirs(&mut self, dirs: Vec<std::path::PathBuf>) {
        self.fonts = Arc::new(FontFiles::new(Self::given(&self.font), dirs));
    }

    /// Builds `src`: what changed, and what the changes reach.
    pub fn update(&mut self, src: &str, opts: Options) -> Output {
        let t0 = Instant::now();
        let threads = opts.threads.max(1);
        let texts = cut(src);
        let time_cut = t0.elapsed();
        // (old pieces found by their text: the map's hash only finds, `==`
        // decides)
        let olds: Vec<Kept> = self.pieces.drain(..).collect();
        // (an unchanged piece at its old place first; then equal text
        // anywhere, in order)
        let mut used = vec![false; olds.len()];
        let mut found: Vec<Option<usize>> = vec![None; texts.len()];
        for (k, t) in texts.iter().enumerate() {
            if olds.get(k).is_some_and(|o| o.run.is_some() && o.text == *t) {
                used[k] = true;
                found[k] = Some(k);
            }
        }
        let mut by_text: FxMap<Arc<[char]>, VecDeque<usize>> = FxMap::default();
        for (i, kp) in olds.iter().enumerate() {
            if kp.run.is_some() && !used[i] {
                by_text.entry(kp.text.clone()).or_default().push_back(i);
            }
        }
        for (k, t) in texts.iter().enumerate() {
            if found[k].is_some() {
                continue;
            }
            let Some(q) = by_text.get_mut(t) else { continue };
            while let Some(i) = q.pop_front() {
                if !used[i] {
                    used[i] = true;
                    found[k] = Some(i);
                    break;
                }
            }
        }
        let mut old_pos = vec![NONE; self.next_id as usize];
        for (i, o) in olds.iter().enumerate() {
            old_pos[o.id as usize] = i as u32;
        }
        let mut p = Vec::with_capacity(texts.len());
        for (k, text) in texts.into_iter().enumerate() {
            if let Some(i) = found[k] {
                let o = &olds[i];
                p.push(PState {
                    id: o.id,
                    text,
                    status: Status::Done,
                    run: o.run.clone(),
                    lines: o.lines.clone(),
                    prev: None,
                    dirty: BTreeSet::new(),
                    runs: 0,
                });
            } else {
                // (an edited piece: the unmatched old piece nearest its place,
                // whose chain it runs from)
                let near = [k, k.wrapping_sub(1), k + 1]
                    .into_iter()
                    .filter(|&i| i < olds.len() && !used[i] && olds[i].run.is_some())
                    .max_by_key(|&i| olds[i].text.iter().zip(text.iter()).take_while(|(a, b)| a == b).count());
                let prev = near.map(|i| {
                    used[i] = true;
                    (olds[i].run.clone().expect("a run"), olds[i].lines.clone(), olds[i].text.clone())
                });
                // (it keeps that piece's identity: its old definitions stay
                // in the index as the prediction until it runs, so a reader
                // of them parks on it rather than reading past it)
                let id = match near {
                    Some(i) => olds[i].id,
                    None => {
                        self.next_id += 1;
                        self.next_id - 1
                    }
                };
                p.push(PState { id, text, status: Status::Unstarted, run: None, lines: Vec::new(), prev, dirty: BTreeSet::new(), runs: 0 });
            }
        }
        let n = p.len();
        let ids: Vec<PieceId> = p.iter().map(|x| x.id).collect();
        let mut pos_of = vec![NONE; self.next_id as usize];
        for (k, &id) in ids.iter().enumerate() {
            pos_of[id as usize] = k as u32;
        }
        // (the names index: what the kept pieces define, at their places,
        // and what an edited piece's predecessor defined, as its prediction)
        let names = Names::new();
        for (k, ps) in p.iter().enumerate() {
            if let Some(r) = ps.run.as_ref().or(ps.prev.as_ref().map(|(r, _, _)| r)) {
                for (key, val) in &r.exports {
                    names.set(k, key, Some(val));
                }
            }
        }
        // (the edges of removed pieces go; the names whose defining places
        // changed — defined by a removed or moved piece — have their
        // readers looked at: one reading another definition now is marked)
        let mut readers = std::mem::take(&mut self.readers);
        let mut touched: Vec<Key> = Vec::new();
        for o in &olds {
            let Some(r) = &o.run else { continue };
            let now = pos_of[o.id as usize];
            if now == NONE || now != old_pos[o.id as usize] {
                touched.extend(r.exports.iter().map(|(k, _)| k.clone()));
            }
            if now == NONE {
                for (key, _, _) in &r.reads {
                    if let Some(m) = readers.get_mut(key) {
                        m.remove(&o.id);
                    }
                }
            }
        }
        touched.sort();
        touched.dedup();
        let mut marks: Vec<(usize, u32)> = Vec::new();
        for key in &touched {
            let Some(m) = readers.get(key) else { continue };
            for (&rid, &(node, src)) in m {
                let rpos = pos_of[rid as usize];
                if rpos == NONE {
                    continue;
                }
                let (def, _) = names.reaching(key, rpos as usize, &self.font);
                let def_id = def.map_or(INITIAL, |d| ids[d as usize]);
                if src != def_id {
                    marks.push((rpos as usize, node));
                }
            }
        }
        let unstarted: BTreeSet<usize> = p.iter().enumerate().filter(|(_, x)| x.status == Status::Unstarted).map(|(k, _)| k).collect();
        let mut state = State {
            p,
            unstarted,
            running: BTreeSet::new(),
            settled: 0,
            readers,
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
        for (k, node) in marks {
            mark(&mut state, k, node);
        }
        // (kept runs' paragraphs never broken go to the queue)
        for k in 0..n {
            if state.p[k].status == Status::Done && !lines_ready(&state, k) {
                let r = state.p[k].run.clone();
                queue_breaks(&mut state, k, r);
            }
        }
        state.stats.time_cut = time_cut;
        state.stats.time_setup = t0.elapsed() - time_cut;
        let sh = Shared {
            names,
            ids,
            pos_of,
            generation: Mutex::new(0),
            parked: Condvar::new(),
            state: Mutex::new(state),
            cv: Condvar::new(),
            font: self.font.clone(),
            fonts: self.fonts.clone(),
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
        let marks = st.scan.as_ref().map(|s| s.marks.clone()).unwrap_or_default();
        let log = {
            let pieces: Vec<_> =
                st.p.iter().map(|ps| (&ps.text[..], &**ps.run.as_ref().expect("every piece ran"), &ps.lines[..])).collect();
            crate::logs::gather(&pieces, &marks, &st.pages)
        };
        for (k, ps) in st.p.iter().enumerate() {
            let r = ps.run.as_ref().expect("every piece ran");
            for e in &r.errors {
                errors.push(format!("piece {k}: {e}"));
            }
            if ps.runs == 0 {
                st.stats.kept += 1;
            }
            st.stats.commands += r.chain.nodes.len() as u64;
            if opts.dump {
                dump_piece(&mut dump, k, ps, r);
            }
        }
        if opts.dump {
            for (i, pg) in st.pages.iter().enumerate() {
                let _ = writeln!(dump, "page {}: {} items", i + 1, pg.b.list.len());
            }
        }
        st.stats.time_join = t_join.elapsed();
        st.stats.pages = st.pages.len();
        st.stats.wall = t0.elapsed();
        self.pieces = st.p.into_iter().map(|ps| Kept { id: ps.id, text: ps.text, run: ps.run, lines: ps.lines }).collect();
        self.readers = st.readers;
        self.pages = st.pages.into_iter().zip(bodies).collect();
        Output { dvi: out, stats: st.stats, errors, dump, log }
    }
}

fn dump_piece(out: &mut String, k: usize, ps: &PState, r: &Run) {
    let how = if ps.runs == 0 { "kept" } else { "ran" };
    let _ = writeln!(
        out,
        "piece {k} (id {}) [{how}; {} nodes, {} run]: {} reads from before it, {} definitions after it, {} paragraphs",
        ps.id,
        r.chain.nodes.len(),
        r.ran,
        r.reads.len(),
        r.exports.len(),
        r.paras.len()
    );
    for (i, n) in r.chain.nodes.iter().enumerate() {
        let _ = write!(out, "  n{i} {:?} {}", n.kind, n.cmd);
        for rd in &n.reads {
            match rd.src {
                Src::Node(d) => {
                    let _ = write!(out, " | reads {} = {} from n{d}", rd.key, rd.val);
                }
                Src::Piece(p) => {
                    let _ = write!(out, " | reads {} = {} from piece id {p}", rd.key, rd.val);
                }
                Src::Initial => {
                    let _ = write!(out, " | reads {} = {} initial", rd.key, rd.val);
                }
            }
        }
        if let Some(phi) = &n.phi {
            let _ = write!(out, " | φ test={}", phi.test);
            for (key, after, before) in &phi.keys {
                let _ = write!(out, " | {key} = φ({after} | {before})");
            }
        }
        for d in &n.defs {
            let _ = write!(out, " | defines {}{} = {}", if d.global { "global " } else { "" }, d.key, d.val);
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
            _ => g.stats.time_ship += dt,
        }
    }
}

fn finished(g: &State) -> bool {
    g.settled == g.p.len() && g.scan_done && g.unshipped == 0
}

/// Node `node` of piece `k` read a definition since remade: the piece
/// runs again from it.
fn mark(g: &mut State, k: usize, node: u32) {
    let ps = &mut g.p[k];
    ps.dirty.insert(node);
    if ps.status == Status::Done {
        ps.status = Status::Unstarted;
        g.unstarted.insert(k);
        g.stats.reruns += 1;
    }
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
/// read those names from before `m` (or from `m`), by the edges.
fn reached(g: &State, sh: &Shared, m: usize, keys: &[Key]) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    for key in keys {
        let Some(rm) = g.readers.get(key) else { continue };
        for (&rid, &(node, src)) in rm {
            let rpos = sh.pos_of[rid as usize];
            if rpos == NONE || rpos as usize <= m {
                continue;
            }
            let before = src == INITIAL || {
                let spos = sh.pos_of[src as usize];
                spos == NONE || spos as usize <= m
            };
            if before {
                out.push((rpos as usize, node));
            }
        }
    }
    out
}

/// Gives each of piece `k`'s paragraphs its lines: a paragraph equal to
/// one of `old`'s (or of the edited piece's predecessor) keeps its lines;
/// the others are queued.
fn queue_breaks(g: &mut State, k: usize, old: Option<Arc<Run>>) {
    let run = g.p[k].run.clone().expect("a run");
    let old_lines = std::mem::take(&mut g.p[k].lines);
    let prev: Option<(Arc<Run>, Vec<Option<Lines>>)> = match g.p[k].prev.take() {
        Some((r, l, _)) => Some((r, l)),
        None => old.map(|r| (r, old_lines)),
    };
    let mut lines = vec![None; run.paras.len()];
    for (i, para) in run.paras.iter().enumerate() {
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
    while g.settled < n && g.p[g.settled].status == Status::Done && g.p[g.settled].dirty.is_empty() {
        g.settled += 1;
    }
    let mut scan_to = g.scan_next;
    while scan_to < g.settled && lines_ready(g, scan_to) {
        scan_to += 1;
    }
    let scan_finish = g.scan_next == n && g.settled == n && !g.scan_done;
    let scan_ok = g.scan.is_some() && (scan_to > g.scan_next || scan_finish);
    let run_ok = !g.unstarted.is_empty();
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
                Order::Priority => *g.unstarted.first().expect("an unstarted piece"),
            };
            g.unstarted.remove(&k);
            g.running.insert(k);
            g.p[k].status = Status::Running;
            let dirty = std::mem::take(&mut g.p[k].dirty).into_iter().collect();
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
                    r.paras.iter().zip(&ps.lines).map(|(p, l)| (p.clone(), l.clone().expect("lines ready"))).collect()
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
        Task::Run(k, dirty) => (run_and_install(sh, k, &dirty), 0),
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
            (page.b.list.len() as u64, 3)
        }
    }
}

/// Runs piece `k` (marked running by whoever chose it) from its dirty
/// nodes and installs the run: its exports into the names index, its
/// reads as edges, the readers its changed definitions reach marked; its
/// own reads validated against the index as it is now.
fn run_and_install(sh: &Shared, k: usize, dirty: &[u32]) -> u64 {
    let (text, old) = {
        let g = lock(sh);
        let ps = &g.p[k];
        let old = match (&ps.run, &ps.prev) {
            (Some(r), _) => Some((r.clone(), ps.text.clone())),
            (None, Some((r, _, t))) => Some((r.clone(), t.clone())),
            (None, None) => None,
        };
        (ps.text.clone(), old)
    };
    let mut env = PieceEnv { sh, k };
    let run = Arc::new(run_piece(&text, &mut env, old.as_ref().map(|(r, t)| (&r.chain, &t[..])), dirty));
    let steps = run.ran;
    let t = Instant::now();
    {
        let mut g = lock(sh);
        g.stats.commands_run += run.ran;
        let id = sh.ids[k];
        // (what stood for this piece until now: its last run, or for an
        // edited piece the run of the piece it replaces)
        let old_run = g.p[k].run.clone().or_else(|| g.p[k].prev.as_ref().map(|(r, _, _)| r.clone()));
        // (the edges: the old run's go, the new run's come, each with the
        // piece whose definition answers it now; a read answered otherwise
        // than it was is stale)
        if let Some(o) = &old_run {
            for (key, _, _) in &o.reads {
                if let Some(m) = g.readers.get_mut(key) {
                    m.remove(&id);
                }
            }
        }
        let mut stale: BTreeSet<u32> = BTreeSet::new();
        for (key, node, val) in &run.reads {
            let (def, now) = sh.names.reaching(key, k, &sh.font);
            if now != *val {
                stale.insert(*node);
            }
            let src = def.map_or(INITIAL, |d| sh.ids[d as usize]);
            g.readers.entry(key.clone()).or_default().insert(id, (*node, src));
        }
        let changed = changed_names(old_run.as_deref(), &run);
        for key in &changed {
            let val = run.exports.binary_search_by(|(x, _)| x.cmp(key)).ok().map(|i| &run.exports[i].1);
            sh.names.set(k, key, val);
        }
        g.p[k].run = Some(run.clone());
        g.p[k].dirty.clear();
        g.p[k].runs += 1;
        g.stats.ran += 1;
        g.running.remove(&k);
        let marks = reached(&g, sh, k, &changed);
        for (r, node) in marks {
            mark(&mut g, r, node);
        }
        queue_breaks(&mut g, k, old_run);
        if stale.is_empty() {
            g.p[k].status = Status::Done;
        } else {
            g.p[k].dirty = stale;
            g.p[k].status = Status::Unstarted;
            g.unstarted.insert(k);
            g.stats.reruns += 1;
        }
        g.stats.time_confirm += t.elapsed();
    }
    *sh.generation.lock().expect("generation") += 1;
    sh.parked.notify_all();
    sh.cv.notify_all();
    steps
}

/// A piece's reads of names from before it.
struct PieceEnv<'a> {
    sh: &'a Shared,
    k: usize,
}

impl Env for PieceEnv<'_> {
    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.sh.fonts.get(name)
    }

    fn reach(&mut self, key: &Key) -> (Val, Src) {
        let sh = self.sh;
        loop {
            let seen = *sh.generation.lock().expect("generation");
            let (def, val) = sh.names.reaching(key, self.k, &sh.font);
            let Some(d) = def else { return (val, Src::Initial) };
            let d = d as usize;
            let src = Src::Piece(sh.ids[d]);
            if sh.opts.reads == Reads::Speculate {
                return (val, src);
            }
            // (the definition is being remade: park until it is installed,
            // or make it here if nobody has started)
            let leapfrog = {
                let mut g = lock(sh);
                match g.p[d].status {
                    Status::Done => return (val, src),
                    Status::Unstarted => {
                        g.unstarted.remove(&d);
                        g.running.insert(d);
                        g.p[d].status = Status::Running;
                        g.stats.parks += 1;
                        g.stats.leapfrogs += 1;
                        Some(std::mem::take(&mut g.p[d].dirty).into_iter().collect::<Vec<u32>>())
                    }
                    Status::Running => {
                        g.stats.parks += 1;
                        None
                    }
                }
            };
            match leapfrog {
                Some(dirty) => {
                    run_and_install(sh, d, &dirty);
                }
                None => {
                    let mut g = sh.generation.lock().expect("generation");
                    while *g == seen {
                        g = sh.parked.wait(g).expect("generation");
                    }
                }
            }
        }
    }
}
