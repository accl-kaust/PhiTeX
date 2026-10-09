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

use std::collections::{BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::dvi;
use crate::graph::{Env, Para, PieceId, Run, Src, cut_from, run_piece};
use crate::hash::{FxMap, FxSet, hash_of};
use crate::linebreak::break_lines;
use crate::logs::{self, Frag};
use crate::page::{Lines, Page, PageOrg, Scan, material};
use crate::tfm::{Font, FontFiles};
use crate::val::{Key, Meaning, Val, initial};

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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: 1,
            reads: Reads::Suspend,
            order: Order::Priority,
            dump: false,
            etex: true,
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
}

const SHARDS: usize = 64;

type Defs = FxMap<Key, Vec<(Pos, Val)>>;

/// A place in document order: (piece position, node). A piece's exports
/// are at `(k, END)`; node `j` of piece `k` reads what is defined before
/// `(k, j)`. (Elements put between a piece's nodes — an output routine's,
/// an `\input` file's — will define at `(k, j)`; none does yet.)
pub type Pos = (u32, u32);
/// The node of a piece's exports: after all its nodes.
pub const END: u32 = u32::MAX;

/// The document's elements in order, each with a stable id (a piece's is
/// its `PieceId`), and where each is: what an edge's ids mean, and which
/// element a place in the names index is.
struct Sequence {
    /// Position -> the piece there.
    pieces: Vec<PieceId>,
    /// Every element, by place (a piece at `(k, END)`).
    elems: Vec<(Pos, PieceId)>,
    /// Id -> place (`(NONE, END)`: not in this build).
    pos: Vec<Pos>,
}

impl Sequence {
    /// The sequence of these pieces, in order, ids below `next`.
    fn new(pieces: Vec<PieceId>, next: PieceId) -> Sequence {
        let mut pos = vec![(NONE, END); next as usize];
        let mut elems = Vec::with_capacity(pieces.len());
        for (k, &id) in pieces.iter().enumerate() {
            pos[id as usize] = (k as u32, END);
            elems.push(((k as u32, END), id));
        }
        Sequence { pieces, elems, pos }
    }

    /// The piece at position `k`.
    fn piece(&self, k: usize) -> PieceId {
        self.pieces[k]
    }

    /// Where element `id` is, if it is in this build.
    fn pos_of(&self, id: PieceId) -> Option<Pos> {
        self.pos.get(id as usize).copied().filter(|p| p.0 != NONE)
    }

    /// The element at place `p`.
    fn id_at(&self, p: Pos) -> PieceId {
        if p.1 == END {
            return self.pieces[p.0 as usize];
        }
        let i = self.elems.partition_point(|e| e.0 < p);
        self.elems[i].1
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
    /// The values before any definition are e-TeX's extended mode's.
    etex: bool,
}

impl Names {
    fn new(etex: bool) -> Names {
        Names {
            shards: (0..SHARDS).map(|_| RwLock::default()).collect(),
            defined: (0..SHARDS).map(|_| AtomicU64::new(0)).collect(),
            etex,
        }
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
        let (s, bit) = Names::route(key);
        if self.defined[s].load(Ordering::Acquire) & bit == 0 {
            return (None, initial(key, font, self.etex), 0, END);
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
            return (None, initial(key, font, self.etex), 0, hi);
        }
        (None, initial(key, font, self.etex), 0, END)
    }

    /// Place `at` now defines `key` as `val` (none: no longer defines it).
    fn set(&self, at: Pos, key: &Key, val: Option<&Val>) {
        let (s, bit) = Names::route(key);
        if val.is_some() {
            self.defined[s].fetch_or(bit, Ordering::AcqRel);
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
    dirty: BTreeSet<Key>,
    runs: u32,
    /// The piece whose definition answers each of its run's reads, as its
    /// edges say (aligned with `Run::reads`).
    srcs: Vec<PieceId>,
    /// Its part of the log, once its run and lines are final.
    log: Option<Arc<Frag>>,
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
    state: Mutex<State>,
    cv: Condvar,
    font: Arc<Font>,
    fonts: Arc<FontFiles>,
    opts: Options,
}

type ParaLines = (Arc<Para>, Lines);

enum Task {
    /// Run piece `.0` again from the nodes reading the names `.1`.
    Run(usize, Vec<Key>),
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
        pieces: Vec<Vec<ParaLines>>,
        finish: bool,
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
        }
    }

    /// Cuts `src` into pieces, the pieces outside the edit from the last
    /// source to this one being the last cut's (the same `Arc`s): the cut is
    /// made again from the piece before the one the edit begins in, up to
    /// the first piece start after the edit that is a piece start of the
    /// last cut (from a piece start on, the cut depends only on the text
    /// after it).
    fn cut(&self, src: &str) -> (Vec<Arc<[char]>>, Vec<usize>) {
        if self.pieces.is_empty() {
            let (texts, starts, _) = cut_from(src, 0, |_| false);
            return (texts, starts);
        }
        let (ob, nb) = (self.src.as_bytes(), src.as_bytes());
        let pre = common_prefix(ob, nb);
        let olds = || self.pieces.iter().map(|o| o.text.clone());
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
        let (mid, mid_starts, _) = cut_from(src, from, |p| {
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
        let threads = opts.threads.max(1);
        let (texts, starts) = self.cut(src);
        self.src.clear();
        self.src.push_str(src);
        self.starts = starts;
        let time_cut = t0.elapsed();
        // (old pieces found by their text: the same `Arc` (a piece outside
        // the edit), or equal text at its old place; then equal text
        // anywhere, in order; the map's hash only finds, `==` decides)
        let mut olds: Vec<Kept> = std::mem::take(&mut self.pieces);
        let old_runs: Vec<Option<Arc<Run>>> = olds.iter().map(|o| o.run.clone()).collect();
        let mut used = vec![false; olds.len()];
        let mut found: Vec<Option<usize>> = vec![None; texts.len()];
        let by_ptr: FxMap<usize, usize> = olds
            .iter()
            .enumerate()
            .filter(|(_, o)| o.run.is_some())
            .map(|(i, o)| (o.text.as_ptr() as usize, i))
            .collect();
        for (k, t) in texts.iter().enumerate() {
            let at = by_ptr
                .get(&(t.as_ptr() as usize))
                .copied()
                .filter(|&i| Arc::ptr_eq(&olds[i].text, t))
                .or_else(|| {
                    (olds.get(k).is_some_and(|o| o.run.is_some() && o.text == *t)).then_some(k)
                });
            if let Some(i) = at.filter(|&i| !used[i]) {
                used[i] = true;
                found[k] = Some(i);
            }
        }
        let mut by_text: FxMap<Arc<[char]>, VecDeque<usize>> = FxMap::default();
        if found.iter().any(Option::is_none) {
            for (i, kp) in olds.iter().enumerate() {
                if kp.run.is_some() && !used[i] {
                    by_text.entry(kp.text.clone()).or_default().push_back(i);
                }
            }
        }
        for (k, t) in texts.iter().enumerate() {
            if found[k].is_some() {
                continue;
            }
            let Some(q) = by_text.get_mut(t) else {
                continue;
            };
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
                // (each old piece is taken at most once: its lines and edge
                // sources move)
                let o = &mut olds[i];
                p.push(PState {
                    id: o.id,
                    text,
                    status: Status::Done,
                    run: o.run.clone(),
                    lines: std::mem::take(&mut o.lines),
                    srcs: std::mem::take(&mut o.srcs),
                    log: o.log.take(),
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
                    .max_by_key(|&i| {
                        olds[i]
                            .text
                            .iter()
                            .zip(text.iter())
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
                p.push(PState {
                    id,
                    text,
                    status: Status::Unstarted,
                    run: None,
                    lines: Vec::new(),
                    srcs: near
                        .map(|i| std::mem::take(&mut olds[i].srcs))
                        .unwrap_or_default(),
                    log: None,
                    prev,
                    dirty: BTreeSet::new(),
                    runs: 0,
                });
            }
        }
        let n = p.len();
        let seq = Sequence::new(p.iter().map(|x| x.id).collect(), self.next_id);
        // (the names index: what the kept pieces define, at their places,
        // and what an edited piece's predecessor defined, as its prediction)
        let names = Names::new(opts.etex);
        for (k, ps) in p.iter().enumerate() {
            if let Some(r) = ps.run.as_ref().or(ps.prev.as_ref().map(|(r, _, _)| r)) {
                for (key, val) in &r.exports {
                    names.set((k as u32, END), key, Some(val));
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
            let now = seq.pos_of(o.id).map(|p| p.0);
            if now != Some(old_pos[o.id as usize]) {
                touched.extend(r.exports.iter().map(|(k, _)| k.clone()));
            }
            if now.is_none() {
                for (key, _, _) in &r.reads {
                    if let Some(m) = readers.get_mut(key) {
                        m.remove(&o.id);
                    }
                }
            }
        }
        touched.sort();
        touched.dedup();
        let mut marks: Vec<(usize, Key)> = Vec::new();
        for key in &touched {
            let Some(m) = readers.get(key) else { continue };
            for (&rid, &(node, src)) in m {
                let Some(rpos) = seq.pos_of(rid) else {
                    continue;
                };
                let (def, ..) = names.reaching(key, (rpos.0, node), &self.font);
                let def_id = def.map_or(INITIAL, |d| seq.id_at(d));
                if src != def_id {
                    marks.push((rpos.0 as usize, key.clone()));
                }
            }
        }
        let unstarted: BTreeSet<usize> = p
            .iter()
            .enumerate()
            .filter(|(_, x)| x.status == Status::Unstarted)
            .map(|(k, _)| k)
            .collect();
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
            matching: !self.page_orgs.is_empty() || !self.marks.is_empty(),
            old_runs,
            last_diff: None,
            last_diff_known: false,
            pages: Vec::new(),
            orgs: Vec::new(),
            marks_before: Vec::new(),
            marks_after: Vec::new(),
            bodies: Vec::new(),
            numbers: Arc::default(),
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
            old_orgs: Arc::new(std::mem::take(&mut self.page_orgs)),
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
        state.stats.time_setup = t0.elapsed().saturating_sub(time_cut);
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
        let mut bodies: Vec<Arc<dvi::Body>> = st
            .bodies
            .iter()
            .map(|b| b.clone().expect("every page shipped"))
            .collect();
        let numbers = dvi::numbers(
            st.p.iter()
                .filter_map(|ps| ps.run.as_ref().map(|r| &r.fonts[..])),
        );
        let (out, again) = dvi::assemble(&numbers, &st.pages, &mut bodies);
        st.stats.bodies_again = again;
        let mut errors = Vec::new();
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
        let log = {
            let frags: Vec<&Frag> =
                st.p.iter()
                    .map(|ps| &**ps.log.as_ref().expect("a part"))
                    .collect();
            logs::gather(&frags, &marks, &st.pages)
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
        self.pieces =
            st.p.into_iter()
                .map(|ps| Kept {
                    id: ps.id,
                    text: ps.text,
                    run: ps.run,
                    lines: ps.lines,
                    srcs: ps.srcs,
                    log: ps.log,
                })
                .collect();
        self.readers = st.readers;
        self.pages = st.pages.into_iter().zip(bodies).collect();
        self.page_orgs = st.orgs;
        self.marks = marks;
        Output {
            dvi: out,
            stats: st.stats,
            errors,
            dump,
            log,
        }
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
        for rd in n.reads.iter() {
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
        if let Some(phi) = n.phi() {
            let _ = write!(out, " | φ test={}", phi.test);
            for (key, after, before) in &phi.keys {
                let _ = write!(out, " | {key} = φ({after} | {before})");
            }
        }
        for d in n.defs() {
            let _ = write!(
                out,
                " | defines {}{} = {}",
                if d.global { "global " } else { "" },
                d.key,
                d.val
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
            4 => g.stats.time_log += dt,
            _ => g.stats.time_ship += dt,
        }
    }
}

fn finished(g: &State) -> bool {
    g.settled == g.p.len()
        && g.scan_done
        && g.unshipped == 0
        && g.log_queue.is_empty()
        && g.logs_running == 0
}

/// Piece `k` read a definition of `key` since remade: it runs again from
/// the first of its nodes reading the name (every node reading it is
/// looked at again). Returns whether it was done.
fn mark(g: &mut State, k: usize, key: Key) -> bool {
    let ps = &mut g.p[k];
    ps.dirty.insert(key);
    if ps.status == Status::Done {
        ps.status = Status::Unstarted;
        g.unstarted.insert(k);
        g.stats.reruns += 1;
        return true;
    }
    false
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
fn reached(g: &State, sh: &Shared, m: Pos, keys: &[Key]) -> Vec<(usize, Key)> {
    let mut out = Vec::new();
    for key in keys {
        let Some(rm) = g.readers.get(key) else {
            continue;
        };
        for (&rid, &(node, src)) in rm {
            let Some(rpos) = sh.seq.pos_of(rid) else {
                continue;
            };
            if (rpos.0, node) <= m {
                continue;
            }
            if src_at_or_before(&sh.seq, src, m) {
                out.push((rpos.0 as usize, key.clone()));
            }
        }
    }
    out
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
    g.old_runs.get(k).and_then(Option::as_ref).is_some_and(|o| {
        Arc::ptr_eq(o, run)
            || (o.paras.len() == run.paras.len()
                && o.paras
                    .iter()
                    .zip(&run.paras)
                    .all(|(a, b)| Arc::ptr_eq(a, b) || **a == **b))
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

/// The scheduler: a function of the state, choosing this thread's next
/// piece of work.
fn pick(g: &mut State, sh: &Shared) -> Option<Task> {
    let n = g.p.len();
    while g.settled < n && g.p[g.settled].status == Status::Done && g.p[g.settled].dirty.is_empty()
    {
        g.settled += 1;
    }
    if g.matching {
        advance_match(g);
    }
    let mut scan_to = g.scan_next;
    while scan_to < g.settled && lines_ready(g, scan_to) {
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
    let run_ok = !g.unstarted.is_empty();
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
                    r.paras
                        .iter()
                        .zip(&ps.lines)
                        .map(|(p, l)| (p.clone(), l.clone().expect("lines ready")))
                        .collect()
                })
                .collect();
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
        } => {
            let before = scan.fed;
            let mut converged = None;
            'feed: for (d, paras) in pieces.iter().enumerate() {
                let k = (from + d) as u32;
                let skip = if d == 0 { skip as usize } else { 0 };
                for (i, it) in material(paras).into_iter().enumerate().skip(skip) {
                    scan.feed(it, (k, i as u32));
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
            if finish && converged.is_none() {
                scan.finish((to as u32, 0));
            }
            let work = scan.fed - before;
            let (new, new_orgs) = scan.take_pages();
            let mut g = lock(sh);
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
            let body = Arc::new(dvi::page_body(&page, &numbers, &[], &[], 0));
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
fn run_and_install(sh: &Shared, k: usize, dirty: &[Key]) -> u64 {
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
    let run = Arc::new(run_piece(
        &text,
        &mut env,
        old.as_ref().map(|(r, t)| (&**r, &t[..])),
        dirty,
    ));
    let steps = run.ran;
    let t = Instant::now();
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
            (def, now == *val)
        })
        .collect();
    let changed = changed_names(old_run.as_deref(), &run);
    let equal = equal_paras(old_run.as_deref(), &run);
    // (a run reading the same names as the old one, in the same order,
    // leaves the edges whose node and source are the same as they are)
    let same_keys = old_run.as_ref().is_some_and(|o| {
        o.reads.len() == run.reads.len() && o.reads.iter().zip(&run.reads).all(|(a, b)| a.0 == b.0)
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
                    *c = (def, now == *val);
                }
            }
        }
        // (the edges: the new run's, each with the piece whose definition
        // answers it now, replace the old run's; a read answered otherwise
        // than it was is stale)
        let mut stale: BTreeSet<Key> = BTreeSet::new();
        let old_srcs = std::mem::take(&mut g.p[k].srcs);
        let mut srcs = Vec::with_capacity(run.reads.len());
        for (i, ((key, node, _), &(def, same))) in run.reads.iter().zip(&checks).enumerate() {
            if !same {
                stale.insert(key.clone());
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
                && old_run.as_ref().is_some_and(|o| o.reads[i].1 == *node)
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
            for (key, _, _) in &o.reads {
                if !read_keys.contains(key)
                    && let Some(m) = g.readers.get_mut(key)
                {
                    m.remove(&id);
                }
            }
        }
        if !changed.is_empty() {
            g.names_log.push(changed.clone());
        }
        for key in &changed {
            let val = run
                .exports
                .binary_search_by(|(x, _)| x.cmp(key))
                .ok()
                .map(|i| &run.exports[i].1);
            sh.names.set((k as u32, END), key, val);
        }
        sh.names_gen.store(g.names_log.len(), Ordering::Release);
        g.p[k].run = Some(run.clone());
        g.p[k].dirty.clear();
        g.p[k].runs += 1;
        g.stats.ran += 1;
        g.running.remove(&k);
        let marks = reached(&g, sh, (k as u32, END), &changed);
        for (r, key) in marks {
            if mark(&mut g, r, key) {
                sh.done[r].store(false, Ordering::Release);
            }
        }
        queue_breaks(&mut g, k, &equal);
        if stale.is_empty() {
            g.p[k].status = Status::Done;
            sh.done[k].store(true, Ordering::Release);
        } else {
            g.p[k].dirty = stale;
            g.p[k].status = Status::Unstarted;
            g.unstarted.insert(k);
            g.stats.reruns += 1;
        }
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
}

impl Env for PieceEnv<'_> {
    fn font_file(&mut self, name: &str) -> Option<Arc<[u8]>> {
        self.sh.fonts.get(name)
    }

    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32) {
        let sh = self.sh;
        loop {
            let seen = sh.generation.load(Ordering::Acquire);
            let (def, val, lo, hi) = sh.names.reaching(key, (self.k as u32, node), &sh.font);
            let Some(d) = def else {
                // (an undefined name while an earlier piece may still
                // define it: wait for the earlier pieces, or run the first
                // of them nobody has started)
                if sh.opts.reads == Reads::Speculate
                    || !matches!(val, Val::Meaning(Meaning::Undefined))
                {
                    return (val, Src::Initial, lo, hi);
                }
                let leapfrog = {
                    let mut g = lock(sh);
                    if let Some(&e) = g.unstarted.first().filter(|&&e| e < self.k) {
                        g.unstarted.remove(&e);
                        g.running.insert(e);
                        g.p[e].status = Status::Running;
                        g.stats.parks += 1;
                        g.stats.leapfrogs += 1;
                        Some((
                            e,
                            std::mem::take(&mut g.p[e].dirty)
                                .into_iter()
                                .collect::<Vec<Key>>(),
                        ))
                    } else if g.running.first().is_some_and(|&e| e < self.k) {
                        g.stats.parks += 1;
                        None
                    } else {
                        return (val, Src::Initial, lo, hi);
                    }
                };
                if let Some((e, dirty)) = leapfrog {
                    run_and_install(sh, e, &dirty);
                } else {
                    let mut p = sh.park.lock().expect("park");
                    while sh.generation.load(Ordering::Acquire) == seen {
                        p = sh.parked.wait(p).expect("park");
                    }
                }
                continue;
            };
            let src = Src::Piece(sh.seq.id_at(d));
            let d = d.0 as usize;
            // (an element between this piece's own nodes is this run's)
            if sh.opts.reads == Reads::Speculate
                || d == self.k
                || sh.done[d].load(Ordering::Acquire)
            {
                return (val, src, lo, hi);
            }
            // (the definition is being remade: park until it is installed,
            // or make it here if nobody has started)
            let leapfrog = {
                let mut g = lock(sh);
                match g.p[d].status {
                    Status::Done => return (val, src, lo, hi),
                    Status::Unstarted => {
                        g.unstarted.remove(&d);
                        g.running.insert(d);
                        g.p[d].status = Status::Running;
                        g.stats.parks += 1;
                        g.stats.leapfrogs += 1;
                        Some(
                            std::mem::take(&mut g.p[d].dirty)
                                .into_iter()
                                .collect::<Vec<Key>>(),
                        )
                    }
                    Status::Running => {
                        g.stats.parks += 1;
                        None
                    }
                }
            };
            if let Some(dirty) = leapfrog {
                run_and_install(sh, d, &dirty);
            } else {
                let mut p = sh.park.lock().expect("park");
                while sh.generation.load(Ordering::Acquire) == seen {
                    p = sh.parked.wait(p).expect("park");
                }
            }
        }
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
        let (names, font) = (Names::new(true), font());
        let x = Key::Cs("x".into());
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
        let mut seq = Sequence::new(vec![10, 11, 12, 13], 21);
        seq.elems.insert(2, ((2, 5), 20));
        seq.pos[20] = (2, 5);
        assert_eq!(seq.id_at((2, 5)), 20);
        assert_eq!(seq.id_at((2, END)), 12);
        assert_eq!(seq.pos_of(20), Some((2, 5)));
        assert_eq!(seq.pos_of(14), None);
        assert!(src_at_or_before(&seq, 20, (2, END)) && !src_at_or_before(&seq, 20, (2, 4)));
        assert!(src_at_or_before(&seq, INITIAL, (0, 0)));
    }
}
