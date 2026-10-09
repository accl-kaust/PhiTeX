//! The graph (DESIGN 7.3–7.7, 7.11–7.12): one arena of nodes, built by
//! evaluating it, kept up to date by pushing changes in position order.
//!
//! - Nodes live in columns indexed by [`NodeId`]. A node's position is
//!   its Dewey path: its parent and its order label among its siblings.
//! - Every edge has a reverse entry on its source, so a change wakes
//!   exactly the readers of what changed, field by field.
//! - Steps are the only nodes that make nodes. Their emissions are
//!   matched to their old children by key; their successors are matched
//!   by key, cursor, group and state (convergence).
//! - Names are an index of definitions by position with scopes.

use std::cmp::Ordering;
use std::collections::HashMap as StdMap;
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::marker::PhantomData;

use crate::lang::{Chain, Class, Fam, Lang, Slot, Step};
use crate::memo::Memo;
use crate::profile::{Ev, Prof, Tick};
use crate::seq::{ElemId, Leaf, Seq};
use crate::value::{Proj, Sel, Value, project};
use crate::ver::{Ver, hash64, name_hash};

/// Tags that tell a scan element's memo entries (state, output) apart.
const SCAN_STATE: u64 = 0x5343_414e_5354;
const SCAN_OUT: u64 = 0x5343_414e_4f55;

/// A node's index in the arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u32);

/// An interned name.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NameId(pub u32);

/// A fast hasher for the core's own tables (ids and hashes as keys).
#[derive(Default, Clone, Copy)]
pub(crate) struct Fx(u64);

impl Hasher for Fx {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type Map<K, V> = StdMap<K, V, BuildHasherDefault<Fx>>;
pub(crate) type Set<K> = std::collections::HashSet<K, BuildHasherDefault<Fx>>;

pub(crate) const NONE: u32 = u32::MAX;
const ROOT: u32 = 0;
/// No group: definitions at the outer level live to the end.
const NOGROUP: u64 = 0;
/// A chain read's flag (in its aux): only the payloads before it.
const BEFORE: u64 = 1 << 32;
/// The cursor past the last element.
pub const END: ElemId = ElemId(u64::MAX);

/// What a node is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Root,
    /// A value the client sets between runs.
    Input,
    /// A constant a step emitted.
    Const,
    Leaf,
    Unfold,
    Step,
    Scan,
    /// A cross-run slot's value.
    Cross,
    /// A chain's payloads.
    ChainRead,
    /// A slot family's entries from the last run, in order.
    Family,
}

const DIRTY: u8 = 1;
const QUEUED: u8 = 2;
const DEAD: u8 = 4;
/// A step that is a sealed region (DESIGN 7.11): a run of steps folded.
const SEALED: u8 = 8;

mod check;
mod par;
mod runs;
use runs::Runs;
mod compact;
mod seal;
mod tune;

/// An operand: what it reads (`src`, through `sel`), and the name it
/// was resolved from, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Opd {
    pub src: u32,
    pub sel: Sel,
    pub name: u32,
}

/// A reverse edge: `node` reads the list's owner (valid while `node`'s
/// generation is `era`).
#[derive(Clone, Copy)]
struct Rev {
    node: u32,
    era: u32,
    next: u32,
}

/// A position: a node's (its parent, its label), or a definition's or a
/// scope event's (the step, its emission order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pos {
    pub parent: u32,
    pub ord: u64,
}

/// A definition: the one record of it (DESIGN 7.5). Its step's range in
/// `NameTab::recs` lists it, and its name's list indexes it.
#[derive(Clone, Copy, Debug)]
struct DefRec {
    name: u32,
    /// Where it is: (its step, its emission's ordinal there).
    step: u32,
    sub: u32,
    src: u32,
    sel: Sel,
    group: u64,
    global: bool,
}

impl DefRec {
    fn pos(&self) -> Pos {
        Pos {
            parent: self.step,
            ord: u64::from(self.sub),
        }
    }
}

/// A step's own record.
#[derive(Clone, Debug, Default)]
pub(crate) struct StepInfo {
    pub unfold: u32,
    /// Its first child.
    pub first: u32,
    pub key: u64,
    /// The cursor at the step's start.
    pub at: ElemId,
    /// Input elements consumed.
    pub took: u32,
    pub grp_in: u64,
    /// The version of the state it ran from.
    pub in_ver: Ver,
    /// Its definitions: a range of `Graph::step_defs`.
    d0: u32,
    dn: u32,
    /// The run (`Graph::epoch`) it last ran in.
    pub ran: u32,
    /// Its emissions when it last ran (a sealed region: its steps').
    pub work: u32,
    /// A sealed region: the steps folded (0: a step).
    pub folded: u32,
}

pub(crate) struct UnfoldInfo<V> {
    /// Its first step.
    pub first: u32,
    /// Whether `keys` and `owners` are kept up to date: they are built
    /// the first time a resync or an input edit needs them, so a cold
    /// build pays nothing for them.
    pub indexed: bool,
    pub keys: Map<u64, u32>,
    /// Each input element that starts a step's consumption → the step.
    pub owners: Map<ElemId, u32>,
    pub input: Option<Seq<V>>,
    pub args_ver: Ver,
    /// During a resume after an input edit: where the changed elements
    /// end (new indices); a successor starting before it runs again.
    pub hunk_end: Option<usize>,
    /// The group open where the unfold was made.
    pub grp0: u64,
    /// Speculative entry: where the chain stops (an input index), and
    /// where it stopped (the last step's successor, not made yet).
    pub stop: Option<usize>,
    pub parked: Option<Parked>,
    /// A segment's first step: its key and its input index.
    pub start: Option<(u64, usize)>,
}

/// A step whose successor is not made yet: what the successor check
/// needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Parked {
    pub s: u32,
    pub key: u64,
    pub cursor: ElemId,
    pub idx: usize,
    pub grp: u64,
    pub ver: Ver,
}

pub(crate) struct ScanInfo<V> {
    pub input: Seq<V>,
    /// The state after each element (keyed like the input).
    pub states: Seq<V>,
    pub outs: Seq<V>,
    pub init_ver: Ver,
    pub args_ver: Ver,
    pub done: bool,
}

#[derive(Default)]
struct NameTab {
    by_hash: Map<u64, u32>,
    hashes: Vec<u64>,
    spell: Vec<Box<[u8]>>,
    /// Each name's definitions, in position order: indices into `recs`.
    defs: Vec<Runs<u32>>,
    /// Every definition's record; a step's are a range (`StepInfo::d0`,
    /// `dn`). A replaced range stays until compaction.
    recs: Vec<DefRec>,
    /// Nodes that read the name: (anchor, node, era), sorted by the
    /// anchor's position (a top-level step or root node, whose order
    /// never changes); stale entries pruned at the run's end.
    readers: Vec<Runs<(u32, u32, u32)>>,
    /// Names whose reader lists hold stale entries.
    prune: Vec<u32>,
    pruned: Vec<bool>,
    /// Each name's definitions' generation: bumped when one is added or
    /// removed (a dry outcome that read the name then no longer holds).
    gens: Vec<u32>,
}

#[derive(Clone, Debug)]
struct Group {
    parent: u64,
    /// Where it ends: the first of `closers` in position order.
    close: Option<Pos>,
    /// Every place a step closes it. A client may close a group twice
    /// (an unbalanced `}` in a box closes the group around the box, and
    /// the outer `}` closes it again); its scope ends at the first, in
    /// position order, whatever order the steps ran in.
    closers: Vec<Pos>,
    names: Vec<u32>,
}

/// The counts of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Leaves, constants, crosses and chain reads evaluated.
    pub evals: u64,
    /// Emissions merged into an equal earlier one of their step (CSE).
    pub merged: u64,
    /// Steps run.
    pub steps: u64,
    /// Scan elements stepped.
    pub scanned: u64,
    pub created: u64,
    pub removed: u64,
    /// Steps folded into sealed regions.
    pub sealed: u64,
    /// Nodes woken by a change they read.
    pub woken: u64,
    /// Runs of the cross-run loop beyond the first.
    pub iterations: u32,
    /// Slots still changing when the loop's bound was reached, each with
    /// the versions it took, in order.
    pub oscillating: Vec<(Slot, Vec<Ver>)>,
    /// Of `evals`: cross-run reads (slots and families) evaluated.
    pub cross_evals: u64,
    /// Groups closed with none open (a client error).
    pub unbalanced: u64,
    /// The largest step state seen, in bytes ([`Config::debug`]).
    pub state_max: usize,
    /// The sum of step states' bytes, over `steps`.
    pub state_sum: u64,
    /// Steps whose op ran dry on a worker (parallel rounds), and of
    /// those, the ones whose outcome held when their turn came.
    pub dry_runs: u64,
    pub dry_used: u64,
    /// The run was cancelled (`Graph::cancel_token`): work is left queued.
    pub cancelled: bool,
}

/// Switches.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools, reason = "independent switches")]
pub struct Config {
    /// Bound on cross-run iterations.
    pub max_iters: u32,
    /// Evaluate every leaf again after the run and compare (impurity or a
    /// missed wake panics).
    pub check: bool,
    /// Report step state sizes.
    pub debug: bool,
    /// Worker threads for speculative entry (1: none; wasm: always 1).
    pub workers: usize,
    /// A segment's private graph: no cross-run loop.
    pub segment: bool,
    /// Keep every emission as a node (the text form shows them, check
    /// mode re-evaluates them); off, a step's pure interior is transient.
    pub keep_interior: bool,
    /// Seal settled runs of root unfolds' steps after each run, about
    /// this many steps a run (0: never; DESIGN 7.11).
    pub seal: u32,
    /// A sealed run's bound in emissions: what its first edit re-runs.
    pub seal_max: u32,
    /// Runs a step must stay quiet before it is sealed again, once it
    /// ran after the first build (an edited region stays live).
    pub seal_quiet: u32,
    /// Parallel rounds run only when a step's op takes at least this long
    /// on average (ns, sampled): cheaper ops cost less run in turn than
    /// their outcomes cost to hand over.
    pub round_min_ns: u64,
    /// `tune` (DESIGN 7.21): what a memo probe costs (ns). An op is
    /// memoized when its mean cost times its reuse is above it, and its
    /// reuse is measured only when its cost alone is.
    pub memo_probe_ns: u64,
    /// `tune`: timed samples of an op, and keyed evaluations or memo
    /// probes, before it decides about it.
    pub tune_min_samples: u64,
    pub tune_min_keyed: u64,
    /// `tune`: the memo store's budget it sets when it opts the first op
    /// in and the store has none.
    pub auto_memo_bytes: usize,
    /// `tune`: re-runs that make a region hot (kept live four times
    /// longer); a region that re-ran once is sealed after an eighth of
    /// `seal_quiet`.
    pub hot_runs: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_iters: 5,
            check: false,
            debug: false,
            workers: 1,
            segment: false,
            keep_interior: false,
            seal: 0,
            seal_max: 4096,
            seal_quiet: 16,
            round_min_ns: 10_000,
            memo_probe_ns: 100,
            tune_min_samples: 16,
            tune_min_keyed: 64,
            auto_memo_bytes: 64 << 20,
            hot_runs: 4,
        }
    }
}

/// The node columns.
pub struct Nodes<L: Lang> {
    /// Each node's header: one cache line holds all a node's own fields
    /// but its value.
    pub(crate) h: Vec<Hdr<L::Op>>,
    pub(crate) fams: Map<u32, u32>,
    pub(crate) val: Vec<L::Val>,
    pub(crate) opds: Vec<Opd>,
    revs: Vec<Rev>,
    absent: L::Val,
}

/// A node's fields but its value (DESIGN 7.11).
#[derive(Clone, Copy)]
pub(crate) struct Hdr<O> {
    /// Its label among its siblings.
    pub ord: u64,
    /// Index into the kind's table (steps, unfolds, scans), or the slot
    /// or chain.
    pub aux: u64,
    pub op: O,
    pub parent: u32,
    pub next: u32,
    /// Its key (hashed), for matching only.
    pub key: u32,
    /// Its operands: the first in the arena, and how many.
    pub a0: u32,
    /// Its reader list's head.
    pub rd: u32,
    pub era: u32,
    pub an: u16,
    pub depth: u16,
    pub kind: Kind,
    /// The class's tag (its payload, a chain or slot, is in `aux`; an
    /// entry's family in `fams`).
    pub class: u8,
    pub flags: u8,
}

impl<L: Lang> Nodes<L> {
    /// Node `n`'s class.
    #[inline]
    pub(crate) fn class(&self, n: u32) -> Class {
        let u = n as usize;
        match self.h[u].class {
            0 => Class::Pure,
            #[allow(clippy::cast_possible_truncation)]
            1 => Class::Effect(Chain(self.h[u].aux as u32)),
            2 => Class::Barrier,
            3 => Class::Publish(Slot(self.h[u].aux)),
            _ => Class::Entry(Fam(self.fams[&n]), Slot(self.h[u].aux)),
        }
    }

    #[inline]
    pub(crate) fn pos(&self, n: u32) -> Pos {
        Pos {
            parent: self.h[n as usize].parent,
            ord: self.h[n as usize].ord,
        }
    }

    #[inline]
    fn pdepth(&self, p: Pos) -> u16 {
        self.h[p.parent as usize].depth + 1
    }

    /// Position order: pre-order of the region tree.
    pub(crate) fn cmp_pos(&self, a: Pos, b: Pos) -> Ordering {
        if a.parent == b.parent {
            return a.ord.cmp(&b.ord);
        }
        let (mut a, mut b) = (a, b);
        let (mut da, mut db) = (self.pdepth(a), self.pdepth(b));
        let mut lifted_a = false;
        let mut lifted_b = false;
        while da > db {
            a = self.pos(a.parent);
            da -= 1;
            lifted_a = true;
        }
        while db > da {
            b = self.pos(b.parent);
            db -= 1;
            lifted_b = true;
        }
        if a == b {
            // (one is inside the other's subtree: the ancestor first)
            return match (lifted_a, lifted_b) {
                (true, _) => Ordering::Greater,
                (_, true) => Ordering::Less,
                _ => Ordering::Equal,
            };
        }
        while a.parent != b.parent {
            a = self.pos(a.parent);
            b = self.pos(b.parent);
        }
        a.ord.cmp(&b.ord)
    }

    #[inline]
    pub(crate) fn cmp_node(&self, a: u32, b: u32) -> Ordering {
        self.cmp_pos(self.pos(a), self.pos(b))
    }

    #[inline]
    pub(crate) fn opds_of(&self, n: u32) -> &[Opd] {
        let a = self.h[n as usize].a0 as usize;
        &self.opds[a..a + self.h[n as usize].an as usize]
    }

    #[inline]
    fn read(&self, o: &Opd) -> Proj<'_, L::Val> {
        if o.src == NONE {
            Proj::Ref(&self.absent)
        } else {
            project(&self.val[o.src as usize], o.sel, &self.absent)
        }
    }

    #[inline]
    fn read_ver(&self, o: &Opd) -> Ver {
        if o.src == NONE {
            // (not the default value's: a step tells an undefined name
            // from one defined to the default, so a change between them
            // must wake it)
            Ver::ABSENT
        } else {
            o.sel.ver(&self.val[o.src as usize])
        }
    }

    pub(crate) fn is_dead(&self, n: u32) -> bool {
        self.h[n as usize].flags & DEAD != 0
    }
}

/// The operand values an op sees.
pub struct Args<'a, L: Lang> {
    g: &'a Nodes<L>,
    opds: &'a [Opd],
    /// In a step's sweep: the emission's operands, the step's ids (`NONE`
    /// for the interior, whose values are in the buffer), the buffer, and
    /// the operands resolved outside the step.
    sweep: Option<Sweep<'a, L>>,
}

type Sweep<'a, L> = (&'a [EArg], &'a [u32], &'a [<L as Lang>::Val]);

/// What a leaf's evaluation goes through besides its op (DESIGN 7.8,
/// 7.21): the memo store, the ops the auto opt-in memoizes, the profile's
/// sampling and the ops whose reuse it measures. Read only during a run;
/// `None` (no store, no profile) is the plain path.
pub(crate) struct Hook<L: Lang> {
    pub(crate) memo: std::sync::Arc<std::sync::Mutex<Memo<L::Val>>>,
    /// The store has a budget.
    pub(crate) memo_on: bool,
    /// The graph profiles (`P`).
    pub(crate) prof: bool,
    /// Ops memoized by `Graph::tune`.
    pub(crate) auto: OpSet<L::Op>,
    /// Ops whose evaluations' keys the profile looks at.
    pub(crate) watch: OpSet<L::Op>,
    /// No store and nothing watched: only the profile's samples go
    /// through the hook (`Hook::settle` keeps it).
    pub(crate) plain: bool,
}

impl<L: Lang> Hook<L> {
    /// `plain` brought up to date.
    pub(crate) fn settle(&mut self) {
        self.plain = !self.memo_on && self.watch.len() == 0;
    }
}

impl<L: Lang> Default for Hook<L> {
    fn default() -> Self {
        Hook {
            memo: std::sync::Arc::default(),
            memo_on: false,
            prof: false,
            auto: OpSet::default(),
            watch: OpSet::default(),
            plain: true,
        }
    }
}

impl<L: Lang> Clone for Hook<L> {
    fn clone(&self) -> Self {
        Hook {
            memo: self.memo.clone(),
            memo_on: self.memo_on,
            prof: self.prof,
            auto: self.auto.clone(),
            watch: self.watch.clone(),
            plain: self.plain,
        }
    }
}

/// A set of ops with a 64-bit filter in front: most ops not in it are
/// turned away by a hash and a mask.
#[derive(Clone)]
pub(crate) struct OpSet<O> {
    mask: u64,
    set: Set<O>,
}

impl<O> Default for OpSet<O> {
    fn default() -> Self {
        OpSet {
            mask: 0,
            set: Set::default(),
        }
    }
}

impl<O: std::hash::Hash + Eq + Copy> OpSet<O> {
    fn bit(op: &O) -> u64 {
        use std::hash::BuildHasher;
        1 << (BuildHasherDefault::<Fx>::default().hash_one(op) >> 58)
    }
    #[inline]
    pub(crate) fn has(&self, op: &O) -> bool {
        self.mask & Self::bit(op) != 0 && self.set.contains(op)
    }
    pub(crate) fn insert(&mut self, op: O) {
        self.mask |= Self::bit(&op);
        self.set.insert(op);
    }
    pub(crate) fn remove(&mut self, op: &O) {
        if self.set.remove(op) {
            self.mask = self.set.iter().fold(0, |m, o| m | Self::bit(o));
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.set.len()
    }
}

/// The time now, where the platform has a clock (not wasm32).
#[inline]
#[allow(clippy::unnecessary_wraps, reason = "None on wasm32")]
pub(crate) fn now() -> Option<std::time::Instant> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(std::time::Instant::now())
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Nanoseconds since `t`, saturated, less what reading the clock twice
/// costs (measured once: the median of 101 empty timings).
pub(crate) fn ns_since(t: std::time::Instant) -> u32 {
    static COST: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let ns = u32::try_from(t.elapsed().as_nanos()).unwrap_or(u32::MAX);
    let cost = *COST.get_or_init(|| {
        let mut v: Vec<u32> = (0..101)
            .map(|_| {
                let t = std::time::Instant::now();
                u32::try_from(t.elapsed().as_nanos()).unwrap_or(u32::MAX)
            })
            .collect();
        v.sort_unstable();
        v[50]
    });
    ns.saturating_sub(cost)
}

/// Leaf `op` evaluated; with a hook, through it (the memo store, the
/// profile), with what the profile should record.
#[inline]
fn eval_op<L: Lang>(
    hook: Option<&Hook<L>>,
    tick: &mut Tick,
    op: L::Op,
    args: &Args<'_, L>,
) -> (L::Val, Option<Ev<L::Op>>) {
    match hook {
        None => (L::eval(op, args), None),
        // (profiling alone, and not a sample: the countdown only)
        Some(h) if h.plain => {
            if tick.due() {
                eval_hooked(h, tick, op, args)
            } else {
                (L::eval(op, args), None)
            }
        }
        Some(h) => eval_hooked(h, tick, op, args),
    }
}

/// [`eval_op`] through the hook (out of line: the plain path stays small).
#[inline(never)]
fn eval_hooked<L: Lang>(
    h: &Hook<L>,
    tick: &mut Tick,
    op: L::Op,
    args: &Args<'_, L>,
) -> (L::Val, Option<Ev<L::Op>>) {
    let w = if h.prof { tick.next() } else { 0 };
    let memo = h.memo_on && (L::memo(op) || h.auto.has(&op));
    let watch = h.prof && h.watch.has(&op);
    let (tag, key) = if memo || watch {
        let t = L::op_tag(op);
        (t, args.key(t))
    } else {
        (0, Ver::ABSENT)
    };
    let ev = |ns| {
        (w > 0 || watch).then_some(Ev {
            op,
            w,
            ns,
            key: if watch { key } else { Ver::ABSENT },
            kind: crate::profile::LEAF,
        })
    };
    if memo {
        let mut m = h.memo.lock().expect("the memo store");
        let hit = m.get(key).cloned();
        m.note(tag, hit.is_some());
        drop(m);
        if let Some(v) = hit {
            return (v, ev(None));
        }
    }
    let t = if w > 0 { now() } else { None };
    let v = L::eval(op, args);
    let ns = t.map(ns_since);
    if memo {
        h.memo
            .lock()
            .expect("the memo store")
            .put(key, v.clone(), v.bytes());
    }
    (v, ev(ns))
}

impl<'a, L: Lang> Args<'a, L> {
    /// `Ver::node(tag, the operands' versions)`, without allocating for
    /// up to 8 operands.
    fn key(&self, tag: u64) -> Ver {
        let n = self.len();
        if n <= 8 {
            let mut vs = [Ver::ABSENT; 8];
            for (i, v) in vs.iter_mut().enumerate().take(n) {
                *v = self.get(i).ver();
            }
            Ver::node(tag, &vs[..n])
        } else {
            let vs: Vec<Ver> = (0..n).map(|i| self.get(i).ver()).collect();
            Ver::node(tag, &vs)
        }
    }
    fn of(g: &'a Nodes<L>, opds: &'a [Opd]) -> Self {
        Args {
            g,
            opds,
            sweep: None,
        }
    }
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        match self.sweep {
            None => self.opds.len(),
            Some((sa, ..)) => sa.len(),
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Operand `i`'s value.
    ///
    /// # Panics
    ///
    /// If there is no operand `i`.
    #[must_use]
    // (always: the client's ops call it per operand, and without the
    // attribute LLVM stopped inlining it into them once the hook made
    // the crate's own callers more: +9% instructions at K20)
    #[allow(clippy::inline_always, reason = "measured, above")]
    #[inline(always)]
    pub fn get(&self, i: usize) -> Proj<'a, L::Val> {
        let Some((sa, ids, buf)) = self.sweep else {
            return self.g.read(&self.opds[i]);
        };
        match sa[i].a {
            SArg::Local(ix, sel) => {
                let x = ids.get(ix as usize).copied().unwrap_or(NONE);
                if x == NONE {
                    project(&buf[ix as usize], sel, &self.g.absent)
                } else {
                    project(&self.g.val[x as usize], sel, &self.g.absent)
                }
            }
            _ => self.g.read(&sa[i].o),
        }
    }
}

/// A handle on a node a step emitted, valid only in that step (the
/// brand `'s` cannot escape it).
#[derive(Clone, Copy)]
pub struct Local<'s> {
    ix: u32,
    _brand: PhantomData<fn(&'s ()) -> &'s ()>,
}

/// An operand of a node a step emits.
#[derive(Clone, Copy)]
pub enum Arg<'s> {
    Local(Local<'s>),
    Field(Local<'s>, u32),
    /// The unfold's operand `i` (0 its input, 1 its initial state, then
    /// its arguments).
    Up(u16),
    UpField(u16, u32),
    /// A name, resolved at the emitted node's position.
    Name(NameId),
    /// Field `f` of what a name reaches: its reader is woken only when
    /// that field changes.
    NameField(NameId, u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SArg {
    Local(u32, Sel),
    Node(u32, Sel),
    /// A name, and the reader's own field of it (`NOX`: none).
    Name(u32, u32),
}

/// No field of the reader's own on a name (`SArg::Name`).
const NOX: u32 = u32::MAX;

fn extra(x: u32) -> Option<u32> {
    (x != NOX).then_some(x)
}

/// A name's resolution, with the reader's own field on top.
fn with_extra(mut o: Opd, x: u32) -> Opd {
    if o.src != NONE {
        o.sel = o.sel.then(extra(x));
    }
    o
}

/// An emission's operand, and what it resolved to outside the step.
#[derive(Clone, Copy)]
struct EArg {
    a: SArg,
    o: Opd,
}

const NOOPD: Opd = Opd {
    src: NONE,
    sel: Sel::WHOLE,
    name: NONE,
};

struct Spec<O> {
    kind: Kind,
    /// Its value is in the buffer already (evaluated as it was emitted).
    done: bool,
    /// It becomes a node (DESIGN 7.4, transient interiors): anything not
    /// a pure leaf or constant, a definition's source, and what a creator
    /// reads. The rest lives only in the sweep.
    big: bool,
    /// The class's tag (its payload in `aux`, an entry's family in `fam`).
    ctag: u8,
    args: (u32, u16),
    key: u32,
    sub: u32,
    fam: u32,
    op: O,
    /// The class's payload, or the kind's (slot, chain).
    aux: u64,
    grp: u64,
}

impl<O> Spec<O> {
    fn class(&self) -> Class {
        match self.ctag {
            0 => Class::Pure,
            #[allow(clippy::cast_possible_truncation)]
            1 => Class::Effect(Chain(self.aux as u32)),
            2 => Class::Barrier,
            3 => Class::Publish(Slot(self.aux)),
            _ => Class::Entry(Fam(self.fam), Slot(self.aux)),
        }
    }
}

enum Event {
    /// A group opened, and the group open before it.
    Open(u64, u64),
    Close(u64),
}

/// A step's emissions.
struct Emit<L: Lang> {
    specs: Vec<Spec<L::Op>>,
    /// Each emission's value, evaluated as it is emitted where it can be
    /// (`Spec::done`).
    vals: Vec<L::Val>,
    /// The big emissions, in the order they became big.
    bigs: Vec<u32>,
    /// The emissions' operands, each with what it resolved to outside
    /// the step.
    args: Vec<EArg>,
    defs: Vec<(u32, SArg, bool, u64, u64)>,
    events: Vec<(Event, u64)>,
    reads: Vec<(u32, Opd)>,
    new_names: Vec<(u64, Box<[u8]>)>,
    next_key: Option<u64>,
    sub: u64,
    /// CSE (DESIGN 7.10): a pure leaf's (op, operands) hash -> its
    /// emission, and the definitions and group events before it.
    cse: Map<u64, (u32, u32, u32)>,
    /// The leaf being pushed's entry, made by the probe.
    cse_pending: Option<(u64, u32, u32)>,
    /// Emissions merged into an earlier one.
    merged: u32,
    /// For the profile (DESIGN 7.21): sampled or keyed evaluations, CSE
    /// probes (op, merged) and ops emitted with a class other than Pure.
    ev: Vec<Ev<L::Op>>,
    cse_ev: Vec<(L::Op, bool)>,
    impure: Vec<L::Op>,
    /// The emission of the unfold the step called (`StepCx::call`).
    call: Option<u32>,
}

impl<L: Lang> Default for Emit<L> {
    fn default() -> Self {
        Emit {
            specs: Vec::new(),
            vals: Vec::new(),
            bigs: Vec::new(),
            args: Vec::new(),
            defs: Vec::new(),
            events: Vec::new(),
            reads: Vec::new(),
            new_names: Vec::new(),
            next_key: None,
            sub: 0,
            cse: Map::default(),
            cse_pending: None,
            merged: 0,
            ev: Vec::new(),
            cse_ev: Vec::new(),
            impure: Vec::new(),
            call: None,
        }
    }
}

impl<L: Lang> Emit<L> {
    /// The step's own definition of `m` alive at emission `sub`, if any
    /// (`StepCx::own_def` as it was when that emission was made).
    fn own_def_at(&self, m: u32, sub: u64) -> Option<SArg> {
        for &(n, a, global, dsub, grp) in self.defs.iter().rev() {
            if n != m || dsub > sub {
                continue;
            }
            let closed = !global
                && grp != NOGROUP
                && self.events.iter().any(|(e, esub)| {
                    matches!(e, Event::Close(g) if *g == grp) && *esub > dsub && *esub < sub
                });
            if !closed {
                return Some(a);
            }
        }
        None
    }

    /// Buffers sized for a typical step (a dry step's own).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code, reason = "no rounds on wasm"))]
    fn sized() -> Self {
        let mut e = Self::default();
        e.specs.reserve(32);
        e.vals.reserve(32);
        e.args.reserve(64);
        e.bigs.reserve(8);
        e.defs.reserve(8);
        e.reads.reserve(8);
        e
    }

    fn mark_big(&mut self, ix: u32) {
        let sp = &mut self.specs[ix as usize];
        if !sp.big {
            sp.big = true;
            self.bigs.push(ix);
        }
    }

    fn clear(&mut self) {
        self.specs.clear();
        self.vals.clear();
        self.bigs.clear();
        self.args.clear();
        self.defs.clear();
        self.events.clear();
        self.reads.clear();
        self.new_names.clear();
        self.next_key = None;
        self.sub = 0;
        self.cse.clear();
        self.cse_pending = None;
        self.merged = 0;
        self.ev.clear();
        self.cse_ev.clear();
        self.impure.clear();
        self.call = None;
    }
}

/// The name a named source is defined to (DESIGN 7.22): a prefix no
/// client spelling starts with.
fn source_spelling(key: &[u8]) -> Vec<u8> {
    let mut s = b"\0\x01phi-source\0".to_vec();
    s.extend_from_slice(key);
    s
}

/// Names' hashes and spellings (a segment's view of its parent's names).
type Spellings = [(u64, Box<[u8]>)];
type SpellVec = Vec<(u64, Box<[u8]>)>;

/// `StepCx::defined_since`'s answer: names, each with its value here
/// (`None`: no definition reaches).
pub type Since<'s, V> = Vec<(NameId, Option<Proj<'s, V>>)>;

/// The start of a step, for `StepCx::defined_since`: valid while that
/// step lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Here {
    step: u32,
    key: u64,
}

/// What a step sees and does besides its state and operands: its input,
/// names, and the nodes, definitions and scopes it makes.
pub struct StepCx<'s, L: Lang> {
    g: &'s Nodes<L>,
    names: &'s NameTab,
    groups: &'s Map<u64, Group>,
    unfold_opds: &'s [Opd],
    input: Option<&'s Seq<L::Val>>,
    idx: usize,
    start: usize,
    /// The leaf around `idx`, and its first element's index.
    leaf: &'s [(ElemId, L::Val)],
    lbase: usize,
    step: u32,
    pos: Pos,
    grp: u64,
    opened: u32,
    em: &'s mut Emit<L>,
    ext: Option<&'s Map<u64, L::Val>>,
    /// A segment: the spellings of the names in `ext`.
    ext_spell: Option<&'s Spellings>,
    /// The steps' records and the groups each step closed (for
    /// `defined_since`).
    steps: &'s [StepInfo],
    unfolds: &'s [UnfoldInfo<L::Val>],
    closes: &'s Map<u32, Vec<u64>>,
    /// Every emission is big ([`Config::keep_interior`]).
    keep: bool,
    /// The run's cancellation flag.
    cancel: &'s std::sync::atomic::AtomicBool,
    /// The hook leaves are evaluated through, if any.
    hook: Option<&'s Hook<L>>,
    /// The profile's sampling countdown (copied back after the step).
    tick: Tick,
    _brand: PhantomData<fn(&'s ()) -> &'s ()>,
}

impl<'s, L: Lang> StepCx<'s, L> {
    fn name_hash(&self, n: u32) -> u64 {
        let base = self.names.hashes.len();
        if (n as usize) < base {
            self.names.hashes[n as usize]
        } else {
            self.em.new_names[n as usize - base].0
        }
    }

    /// Element `i` of the input, through the cached leaf.
    #[inline]
    fn elem(&mut self, i: usize) -> Option<&'s (ElemId, L::Val)> {
        if i < self.lbase || i >= self.lbase + self.leaf.len() {
            let (leaf, base) = self.input?.leaf_at(i)?;
            self.leaf = leaf;
            self.lbase = base;
        }
        self.leaf.get(i - self.lbase)
    }

    /// Consume the next input element.
    #[allow(
        clippy::should_implement_trait,
        reason = "a cursor, not an iterator: it records what it consumed"
    )]
    pub fn next(&mut self) -> Option<&'s L::Val> {
        let x = &self.elem(self.idx)?.1;
        self.idx += 1;
        Some(x)
    }

    /// The input element `k` ahead, not consumed.
    pub fn peek(&mut self, k: usize) -> Option<&'s L::Val> {
        Some(&self.elem(self.idx + k)?.1)
    }

    /// The identity of the next input element ([`END`] past the last).
    pub fn cursor(&mut self) -> ElemId {
        self.elem(self.idx).map_or(END, |e| e.0)
    }

    /// The elements consumed so far by this step.
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.idx - self.start
    }

    /// The name spelled `s` (interned by content).
    pub fn name(&mut self, s: &[u8]) -> NameId {
        let h = name_hash(s);
        if let Some(&i) = self.names.by_hash.get(&h) {
            assert!(
                *self.names.spell[i as usize] == *s,
                "two names with one hash"
            );
            return NameId(i);
        }
        let base = u32::try_from(self.names.spell.len()).expect("names fit u32");
        if let Some(k) = self.em.new_names.iter().position(|e| e.0 == h) {
            assert!(*self.em.new_names[k].1 == *s, "two names with one hash");
            return NameId(base + u32::try_from(k).expect("names fit u32"));
        }
        self.em.new_names.push((h, s.into()));
        NameId(base + u32::try_from(self.em.new_names.len() - 1).expect("names fit u32"))
    }

    /// Name `n`'s value at this step's start, or a constant this step
    /// defined it to; `None` if undefined. The read is recorded: the step
    /// runs again when it changes.
    pub fn read(&mut self, n: NameId) -> Option<Proj<'s, L::Val>> {
        // (its own definitions first: one whose value is known now)
        if let Some(SArg::Local(ix, sel)) = self.own_def(n.0)
            && self.em.specs[ix as usize].done
        {
            // (the borrow ends with the step's buffer, so the value is
            // cloned)
            let v = &self.em.vals[ix as usize];
            let p = project(v, sel, &self.g.absent);
            return Some(Proj::Owned((*p).clone()));
        }
        let o = self.resolve_read(n.0);
        self.em.reads.push((n.0, o));
        self.value_of(n.0, o)
    }

    /// Name `m` resolved from outside the step, as `read` does.
    fn resolve_read(&self, m: u32) -> Opd {
        if (m as usize) < self.names.defs.len() {
            // (after a group this step closed: the name as it is past the
            // close, as a leaf's operand resolves)
            if self
                .em
                .events
                .iter()
                .any(|(e, _)| matches!(e, Event::Close(_)))
            {
                self.resolve_here(m)
            } else {
                resolve(self.g, self.names, self.groups, m, self.pos)
            }
        } else {
            Opd {
                src: NONE,
                sel: Sel::WHOLE,
                name: m,
            }
        }
    }

    /// The value `o` (name `m`'s resolution) reads.
    fn value_of(&self, m: u32, o: Opd) -> Option<Proj<'s, L::Val>> {
        if o.src == NONE {
            // (a segment: the name as the graph it was entered from has it)
            let ext = self.ext?;
            ext.get(&self.name_hash(m)).map(Proj::Ref)
        } else {
            Some(self.g.read(&o))
        }
    }

    /// Name `m`'s value as `read` gives it, not recorded.
    fn peek_name(&self, m: u32) -> Option<Proj<'s, L::Val>> {
        match self.own_def(m) {
            Some(SArg::Local(ix, sel)) if self.em.specs[ix as usize].done => {
                let v = &self.em.vals[ix as usize];
                return Some(Proj::Owned((*project(v, sel, &self.g.absent)).clone()));
            }
            Some(SArg::Node(src, sel)) => {
                return Some(self.g.read(&Opd {
                    src,
                    sel,
                    name: NONE,
                }));
            }
            _ => {}
        }
        self.value_of(m, self.resolve_read(m))
    }

    /// Every name a definition reaches here, with its value, as `read`
    /// would give it (the step's own definitions, the groups it closed
    /// and `\global` included), but **not recorded**: the step depends on
    /// none of them unless it reads them. In a segment, the names of the
    /// graph it was entered from are included (made names of this step if
    /// it has not named them).
    pub fn defined_reaching(&mut self) -> Vec<(NameId, Proj<'s, L::Val>)> {
        if let Some(sp) = self.ext_spell {
            for (h, spelling) in sp {
                if !self.names.by_hash.contains_key(h) {
                    self.name(spelling);
                }
            }
        }
        let n = self.names.spell.len() + self.em.new_names.len();
        (0..n)
            .filter_map(|m| {
                let m = u32::try_from(m).expect("names fit u32");
                self.peek_name(m).map(|v| (NameId(m), v))
            })
            .collect()
    }

    /// This step's start, for a later [`StepCx::defined_since`].
    #[must_use]
    pub fn here(&self) -> Here {
        Here {
            step: self.step,
            key: self.steps[self.g.h[self.step as usize].aux as usize].key,
        }
    }

    /// The names whose reaching definition may differ between `p0` (the
    /// start of an earlier step of this unfold) and here, each with its
    /// value here as `read` gives it (`None`: no definition reaches),
    /// **not recorded**. They are the names defined from `p0` on (by its
    /// step, the steps after it, their nested unfolds, and this step so
    /// far) and the names defined in groups closed in that stretch, so a
    /// local definition made before `p0` whose group closed since is in
    /// it. Names may be listed whose value did not change. `None` if
    /// `p0`'s step is gone (an edit removed it), is not of this unfold, or
    /// is not before here: then use [`StepCx::defined_reaching`]. Cost:
    /// the steps walked (a sealed run is one) plus their definitions.
    pub fn defined_since(&mut self, p0: Here) -> Option<Since<'s, L::Val>> {
        let u = self.g.h[self.step as usize].parent;
        let h0 = self.g.h.get(p0.step as usize)?;
        if h0.kind != Kind::Step
            || h0.flags & DEAD != 0
            || h0.parent != u
            || self.steps[h0.aux as usize].key != p0.key
        {
            return None;
        }
        let mut names: Set<u32> = Set::default();
        let mut c = p0.step;
        while c != self.step {
            if c == NONE {
                return None;
            }
            self.since_step(c, &mut names);
            c = self.g.h[c as usize].next;
        }
        // (this step so far)
        for d in &self.em.defs {
            names.insert(d.0);
        }
        for (e, _) in &self.em.events {
            if let Event::Close(g) = e
                && let Some(gr) = self.groups.get(g)
            {
                names.extend(gr.names.iter().copied());
            }
        }
        let mut names: Vec<u32> = names.into_iter().collect();
        names.sort_unstable();
        Some(
            names
                .into_iter()
                .map(|m| (NameId(m), self.peek_name(m)))
                .collect(),
        )
    }

    /// Step `s`'s definitions and closed groups' names, and its nested
    /// unfolds' steps', into `names`.
    fn since_step(&self, s: u32, names: &mut Set<u32>) {
        let si = &self.steps[self.g.h[s as usize].aux as usize];
        for d in &self.names.recs[si.d0 as usize..(si.d0 + si.dn) as usize] {
            names.insert(d.name);
        }
        if let Some(cl) = self.closes.get(&s) {
            for g in cl {
                if let Some(gr) = self.groups.get(g) {
                    names.extend(gr.names.iter().copied());
                }
            }
        }
        let mut c = si.first;
        while c != NONE {
            let hc = &self.g.h[c as usize];
            if hc.kind == Kind::Unfold && hc.flags & DEAD == 0 {
                let mut t = self.unfolds[hc.aux as usize].first;
                while t != NONE {
                    if self.g.h[t as usize].kind == Kind::Step {
                        self.since_step(t, names);
                    }
                    t = self.g.h[t as usize].next;
                }
            }
            c = self.g.h[c as usize].next;
        }
    }

    fn arg(&self, a: Arg<'s>) -> SArg {
        match a {
            Arg::Local(l) => SArg::Local(l.ix, Sel::WHOLE),
            Arg::Field(l, f) => SArg::Local(l.ix, Sel::field(f)),
            Arg::Up(i) => {
                let o = self.unfold_opds[i as usize];
                SArg::Node(o.src, o.sel)
            }
            Arg::UpField(i, f) => {
                let o = self.unfold_opds[i as usize];
                debug_assert!(o.sel.is_whole());
                SArg::Node(o.src, Sel::field(f))
            }
            Arg::Name(n) => SArg::Name(n.0, NOX),
            Arg::NameField(n, f) => SArg::Name(n.0, f),
        }
    }

    /// CSE (DESIGN 7.10): an earlier emission of this step that is the
    /// same pure leaf: the same op, aux and operands, and, if it reads a
    /// name, no definition or group event since (so the name resolves to
    /// the same definition). Its value is this one's, now and after any
    /// edit. Otherwise the probe leaves the leaf's entry to `push`.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "emissions and operands of a step fit u32"
    )]
    #[inline(never)]
    fn cse(&mut self, op: L::Op, args: &[Arg<'s>], aux: u64) -> Option<Local<'s>> {
        use std::hash::{BuildHasher, Hash};
        let mut h = BuildHasherDefault::<Fx>::default().build_hasher();
        op.hash(&mut h);
        aux.hash(&mut h);
        let mut named = false;
        for &a in args {
            let a = self.arg(a);
            named |= matches!(a, SArg::Name(..));
            a.hash(&mut h);
        }
        let (nd, ne) = if named {
            (self.em.defs.len() as u32, self.em.events.len() as u32)
        } else {
            (0, 0)
        };
        let k = h.finish();
        if let Some(&(ix, d, e)) = self.em.cse.get(&k) {
            let sp = &self.em.specs[ix as usize];
            let (a0, an) = (sp.args.0 as usize, sp.args.1 as usize);
            if (d, e) == (nd, ne)
                && sp.op == op
                && sp.aux == aux
                && sp.kind == Kind::Leaf
                && sp.ctag == 0
                && an == args.len()
                && args
                    .iter()
                    .zip(&self.em.args[a0..a0 + an])
                    .all(|(&a, ea)| self.arg(a) == ea.a)
            {
                self.em.merged += 1;
                if self.hook.is_some_and(|h| h.prof) {
                    self.em.cse_ev.push((op, true));
                }
                return Some(Local {
                    ix,
                    _brand: PhantomData,
                });
            }
            if (d, e) != (nd, ne) {
                // (a later definition: the later read takes the entry)
                self.em.cse.remove(&k);
            }
        }
        self.em.cse_pending = Some((k, nd, ne));
        if self.hook.is_some_and(|h| h.prof) {
            self.em.cse_ev.push((op, false));
        }
        None
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "emissions and operands of a step fit u32"
    )]
    #[allow(clippy::too_many_lines, reason = "one emission, in order")]
    fn push(
        &mut self,
        kind: Kind,
        op: L::Op,
        class: Class,
        args: &[Arg<'s>],
        lit: Option<L::Val>,
        aux: u64,
    ) -> Local<'s> {
        if kind == Kind::Leaf
            && L::cse(op)
            && matches!(class, Class::Pure)
            && lit.is_none()
            && self.em.next_key.is_none()
            && let Some(l) = self.cse(op, args, aux)
        {
            return l;
        }
        let a0 = self.em.args.len();
        // (a leaf whose operands are all known is evaluated now; one
        // reading a name, after its own definitions are looked at)
        let mut ready = kind == Kind::Leaf;
        let mut named = false;
        for &a in args {
            let a = self.arg(a);
            let o = match a {
                SArg::Local(j, _) => {
                    ready &= self.em.specs[j as usize].done;
                    NOOPD
                }
                SArg::Node(src, sel) => Opd {
                    src,
                    sel,
                    name: NONE,
                },
                SArg::Name(..) => {
                    named = true;
                    NOOPD
                }
            };
            self.em.args.push(EArg { a, o });
        }
        let ix = self.em.specs.len();
        let (ctag, aux, fam) = match class {
            Class::Pure => (0, aux, 0),
            Class::Effect(c) => (1, u64::from(c.0), 0),
            Class::Barrier => (2, aux, 0),
            Class::Publish(s) => (3, s.0, 0),
            Class::Entry(f, s) => (4, s.0, f.0),
        };
        let key = match self.em.next_key.take() {
            // (explicit keys and ordinals apart)
            Some(k) => (hash64(&k) as u32) | 0x8000_0000,
            None => ix as u32 & 0x7fff_ffff,
        };
        let sub = self.em.sub;
        self.em.sub += 1;
        debug_assert!(u16::try_from(args.len()).is_ok(), "at most 65535 operands");
        let (done, v) = match (kind, lit) {
            (Kind::Const, Some(v)) => (true, v),
            _ if ready && !named => {
                let em = &*self.em;
                let args = Args {
                    g: self.g,
                    opds: &[],
                    sweep: Some((&em.args[a0..], &[], &em.vals)),
                };
                let (v, ev) = eval_op(self.hook, &mut self.tick, op, &args);
                if let Some(ev) = ev {
                    self.em.ev.push(ev);
                }
                (true, v)
            }
            _ => (false, L::Val::default()),
        };
        if ctag != 0 && kind == Kind::Leaf && self.hook.is_some_and(|h| h.prof) {
            self.em.impure.push(op);
        }
        let big = self.keep || !matches!(kind, Kind::Leaf | Kind::Const) || ctag != 0;
        if big {
            self.em.bigs.push(ix as u32);
        }
        if matches!(kind, Kind::Unfold | Kind::Scan) {
            // (a creator's operands are nodes: it reads them itself)
            for k in a0..self.em.args.len() {
                if let SArg::Local(j, _) = self.em.args[k].a {
                    self.em.mark_big(j);
                }
            }
        }
        self.em.specs.push(Spec {
            kind,
            done,
            big,
            ctag,
            args: (a0 as u32, args.len() as u16),
            key,
            sub: sub as u32,
            fam,
            op,
            aux,
            grp: self.grp,
        });
        self.em.vals.push(v);
        if let Some(h) = self.em.cse_pending.take() {
            self.em.cse.entry(h.0).or_insert((ix as u32, h.1, h.2));
        }
        if ready && named {
            self.eval_now(ix);
        }
        Local {
            ix: ix as u32,
            _brand: PhantomData,
        }
    }

    /// Evaluate emission `ix` now, if all it reads is known: emissions
    /// already evaluated, nodes, and names as they are here (this step's
    /// own definitions and closed groups taken into account). Otherwise
    /// it waits for the sweep.
    fn eval_now(&mut self, ix: usize) {
        let (a0, an) = {
            let sp = &self.em.specs[ix];
            (sp.args.0 as usize, sp.args.1 as usize)
        };
        for a in a0..a0 + an {
            match self.em.args[a].a {
                SArg::Local(j, _) => {
                    if !self.em.specs[j as usize].done {
                        return;
                    }
                }
                SArg::Node(src, sel) => {
                    self.em.args[a].o = Opd {
                        src,
                        sel,
                        name: NONE,
                    };
                }
                SArg::Name(m, x) => match self.own_def(m) {
                    Some(SArg::Local(j, sel)) => {
                        if !self.em.specs[j as usize].done {
                            return;
                        }
                        // (this step's own definition: read in the step)
                        self.em.args[a].a = SArg::Local(j, sel.then(extra(x)));
                    }
                    Some(SArg::Node(src, sel)) => {
                        let sel = sel.then(extra(x));
                        self.em.args[a] = EArg {
                            a: SArg::Node(src, sel),
                            o: Opd {
                                src,
                                sel,
                                name: NONE,
                            },
                        };
                    }
                    Some(SArg::Name(..)) => return,
                    None => {
                        let o = if (m as usize) < self.names.defs.len() {
                            with_extra(self.resolve_here(m), x)
                        } else {
                            Opd {
                                src: NONE,
                                sel: Sel::WHOLE,
                                name: m,
                            }
                        };
                        if o.src == NONE && self.ext.is_some() {
                            // (an import of a segment: made in the sweep)
                            return;
                        }
                        self.em.args[a].o = o;
                    }
                },
            }
        }
        let v = {
            let em = &*self.em;
            let args = Args {
                g: self.g,
                opds: &[],
                sweep: Some((&em.args[a0..a0 + an], &[], &em.vals)),
            };
            let (v, ev) = eval_op(self.hook, &mut self.tick, em.specs[ix].op, &args);
            if let Some(ev) = ev {
                self.em.ev.push(ev);
            }
            v
        };
        let em = &mut *self.em;
        em.vals[ix] = v;
        em.specs[ix].done = true;
    }

    /// This step's own definition of `m` alive here, if any.
    fn own_def(&self, m: u32) -> Option<SArg> {
        for &(n, a, global, sub, grp) in self.em.defs.iter().rev() {
            if n != m {
                continue;
            }
            let closed = !global
                && grp != NOGROUP
                && self
                    .em
                    .events
                    .iter()
                    .any(|(e, esub)| matches!(e, Event::Close(g) if *g == grp) && *esub > sub);
            if !closed {
                return Some(a);
            }
        }
        None
    }

    /// Name `m` as it is at this step's start, but for the groups this
    /// step has closed so far.
    fn resolve_here(&self, m: u32) -> Opd {
        let closed: Vec<u64> = self
            .em
            .events
            .iter()
            .filter_map(|(e, _)| match e {
                Event::Close(g) => Some(*g),
                Event::Open(..) => None,
            })
            .collect();
        resolve_with(self.g, self.names, self.groups, m, self.pos, &closed)
    }

    /// The next node emitted is keyed by `k` instead of its ordinal.
    pub fn key(&mut self, k: u64) {
        self.em.next_key = Some(k);
    }

    /// A constant.
    pub fn lit(&mut self, v: L::Val) -> Local<'s> {
        self.push(Kind::Const, L::Op::default(), Class::Pure, &[], Some(v), 0)
    }

    /// A leaf.
    pub fn leaf(&mut self, op: L::Op, class: Class, args: &[Arg<'s>]) -> Local<'s> {
        self.push(Kind::Leaf, op, class, args, None, 0)
    }

    /// A nested unfold over `input` from `init`.
    pub fn unfold(
        &mut self,
        op: L::Op,
        input: Arg<'s>,
        init: Arg<'s>,
        args: &[Arg<'s>],
    ) -> Local<'s> {
        let mut all = vec![input, init];
        all.extend_from_slice(args);
        self.push(Kind::Unfold, op, Class::Pure, &all, None, 0)
    }

    /// A continuation call: a nested unfold over `input` from `init`
    /// whose result the step's successor continues from. The step then
    /// returns `Step::Call { key }`. One call a step.
    ///
    /// # Panics
    ///
    /// On a second call in one step.
    pub fn call(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) {
        assert!(self.em.call.is_none(), "one call a step");
        let l = self.unfold(op, input, init, args);
        self.em.call = Some(l.ix);
    }

    /// The named source `key` (`Graph::source`) read as a name: `None`
    /// if it is absent. Either way the read is recorded, so a source that
    /// appears, changes or goes wakes the step. Its value (the file's
    /// elements, for `\\read`) is read with `read` on the same name.
    pub fn source(&mut self, key: &[u8]) -> Option<Arg<'s>> {
        let m = self.name(&source_spelling(key));
        self.read(m)?;
        Some(Arg::Name(m))
    }

    /// A scan over `input` from `init`.
    pub fn scan(
        &mut self,
        op: L::Op,
        input: Arg<'s>,
        init: Arg<'s>,
        args: &[Arg<'s>],
    ) -> Local<'s> {
        let mut all = vec![input, init];
        all.extend_from_slice(args);
        self.push(Kind::Scan, op, Class::Pure, &all, None, 0)
    }

    /// Slot `s`'s value from the last run (predicted).
    pub fn cross(&mut self, s: Slot) -> Local<'s> {
        self.push(Kind::Cross, L::Op::default(), Class::Pure, &[], None, s.0)
    }

    /// Family `f`'s entries from the last run, in order (predicted).
    pub fn family(&mut self, f: Fam) -> Local<'s> {
        self.push(
            Kind::Family,
            L::Op::default(),
            Class::Pure,
            &[],
            None,
            u64::from(f.0),
        )
    }

    /// The payloads of chain `c` before this point: an append list (a
    /// hook, a token list built by appends) read where it is used. An
    /// append is an effect on the chain whose payload does not read the
    /// list, so changing one append re-runs that append and the reads
    /// after it, never the other appends.
    pub fn chain_before(&mut self, c: Chain) -> Local<'s> {
        self.push(
            Kind::ChainRead,
            L::Op::default(),
            Class::Pure,
            &[],
            None,
            u64::from(c.0) | BEFORE,
        )
    }

    /// The payloads of chain `c` (all of it: a chain read is placed after
    /// what it reads by the client).
    pub fn chain(&mut self, c: Chain) -> Local<'s> {
        self.push(
            Kind::ChainRead,
            L::Op::default(),
            Class::Pure,
            &[],
            None,
            u64::from(c.0),
        )
    }

    /// Define name `n` here as `v`'s value, local to the open group
    /// unless `global`.
    pub fn define(&mut self, n: NameId, v: Arg<'s>, global: bool) {
        let s = self.arg(v);
        let sub = self.em.sub;
        self.em.sub += 1;
        let grp = if global { NOGROUP } else { self.grp };
        if let SArg::Local(ix, _) = s {
            self.em.mark_big(ix);
        }
        self.em.defs.push((n.0, s, global, sub, grp));
    }

    /// Open a group: local definitions after it end at its close.
    pub fn open_group(&mut self) {
        self.opened += 1;
        let g = (u64::from(self.step) << 32) | u64::from(self.opened);
        let sub = self.em.sub;
        self.em.sub += 1;
        self.em.events.push((Event::Open(g, self.grp), sub));
        self.grp = g;
    }

    /// Close the innermost open group.
    pub fn close_group(&mut self) {
        let sub = self.em.sub;
        self.em.sub += 1;
        let g = self.grp;
        self.em.events.push((Event::Close(g), sub));
        if g != NOGROUP {
            // (opened in this step, or before: its parent from the table)
            let local = self.em.events.iter().find_map(|(e, _)| match e {
                Event::Open(x, p) if *x == g => Some(*p),
                _ => None,
            });
            self.grp = local.unwrap_or_else(|| self.groups.get(&g).map_or(NOGROUP, |x| x.parent));
        }
    }

    /// Whether the run was cancelled (long steps poll it).
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// [`resolve`], with the groups in `closed` closed too.
fn resolve_with<L: Lang>(
    g: &Nodes<L>,
    names: &NameTab,
    groups: &Map<u64, Group>,
    m: u32,
    at: Pos,
    closed: &[u64],
) -> Opd {
    if closed.is_empty() {
        return resolve(g, names, groups, m, at);
    }
    let none = Opd {
        src: NONE,
        sel: Sel::WHOLE,
        name: m,
    };
    let Some(defs) = names.defs.get(m as usize) else {
        return none;
    };
    let k =
        defs.partition_point(|&i| g.cmp_pos(names.recs[i as usize].pos(), at) == Ordering::Less);
    for &i in defs.iter_back(k) {
        let d = &names.recs[i as usize];
        let alive = d.global
            || d.group == NOGROUP
            || (!closed.contains(&d.group)
                && groups
                    .get(&d.group)
                    .and_then(|x| x.close)
                    .is_none_or(|c| g.cmp_pos(c, at) == Ordering::Greater));
        if alive {
            return Opd {
                src: d.src,
                sel: d.sel,
                name: m,
            };
        }
    }
    none
}

/// Where name `m` at `at` resolves: the latest definition before `at`
/// alive there.
fn resolve<L: Lang>(
    g: &Nodes<L>,
    names: &NameTab,
    groups: &Map<u64, Group>,
    m: u32,
    at: Pos,
) -> Opd {
    let none = Opd {
        src: NONE,
        sel: Sel::WHOLE,
        name: m,
    };
    let Some(defs) = names.defs.get(m as usize) else {
        return none;
    };
    let k =
        defs.partition_point(|&i| g.cmp_pos(names.recs[i as usize].pos(), at) == Ordering::Less);
    for &i in defs.iter_back(k) {
        let d = &names.recs[i as usize];
        let alive = d.global
            || d.group == NOGROUP
            || groups
                .get(&d.group)
                .and_then(|x| x.close)
                .is_none_or(|c| g.cmp_pos(c, at) == Ordering::Greater);
        if alive {
            return Opd {
                src: d.src,
                sel: d.sel,
                name: m,
            };
        }
    }
    none
}

/// The graph. `P`: profiling compiled in (DESIGN 7.21); off, the
/// default, it costs nothing.
pub struct Graph<L: Lang, const P: bool = false> {
    pub(crate) n: Nodes<L>,
    names: NameTab,
    groups: Map<u64, Group>,
    pub(crate) steps: Vec<StepInfo>,
    pub(crate) unfolds: Vec<UnfoldInfo<L::Val>>,
    pub(crate) scans: Vec<ScanInfo<L::Val>>,
    free: Vec<u32>,
    to_free: Vec<u32>,
    heap: Vec<u32>,
    /// Each chain's payloads, in position order.
    chains: Map<u32, Runs<u32>>,
    chain_readers: Map<u32, Vec<u32>>,
    pubs: Map<u64, Vec<u32>>,
    crosses: Map<u64, Vec<u32>>,
    pred: std::sync::Arc<Map<u64, L::Val>>,
    /// Families: their entries' publishers, their readers, their values
    /// from the last run.
    fams: Map<u32, Vec<u32>>,
    fam_readers: Map<u32, Vec<u32>>,
    fpred: std::sync::Arc<Map<u32, L::Val>>,
    /// Slots and families whose publishers changed in this run.
    dirty_slots: std::collections::BTreeSet<u64>,
    dirty_fams: std::collections::BTreeSet<u32>,
    /// Elements each scan stepped in this run, by op.
    scan_runs: Vec<(L::Op, u64)>,
    hunks: Vec<u32>,
    root_ord: u64,
    grp_hint: u64,
    em: Emit<L>,
    keymap: Map<u32, u32>,
    /// The root region's first node.
    root_first: u32,
    /// Where the last step ended: its unfold, cursor and input index.
    hint: (u32, ElemId, usize),
    /// Runs so far (a step's `ran` is the run it last ran in).
    epoch: u32,
    /// Steps that ran after the first build, with the run at which they
    /// will have been quiet long enough to be sealed (in that order).
    hot: std::collections::VecDeque<(u32, u32)>,
    /// Emission buffers for dry steps, reused.
    em_pool: Vec<Emit<L>>,
    /// Dry outcomes of a parallel round, waiting for their steps' turns.
    outcomes: Map<u32, par::Dry<L>>,
    /// Bumped when the group table changes.
    groups_gen: u64,
    /// Items to pop before another round is tried.
    round_cool: usize,
    /// Steps run, and a running mean of an op's time in ns (sampled).
    sampled: u64,
    step_ns: u64,
    /// The next round's size, the last round's outcomes, and the outcomes
    /// used when it was made.
    round_size: usize,
    round_made: u64,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code, reason = "no rounds on wasm"))]
    round_used0: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The memo store (DESIGN 7.8): `Lang::memo` ops only, off while its
    /// budget is 0; shared with the segments of a parallel build).
    /// The memo store and the auto opt-in (DESIGN 7.8, 7.21).
    pub(crate) hook: Hook<L>,
    /// The profile's sampling countdown, and the profile (`P` only).
    tick: Tick,
    pub(crate) prof: Prof<L::Op>,
    /// Regions' quiet times set by `tune` (by step key), where they are
    /// not `Config::seal_quiet`.
    pub(crate) quiet: Map<u64, u32>,
    /// The input leaf the last step ran in.
    lcache: Option<Leaf<L::Val>>,
    /// The groups each step closed.
    closes: Map<u32, Vec<u64>>,
    /// Scratch buffers, reused so the hot path allocates nothing.
    sc_opds: Vec<Opd>,
    sc_ids: Vec<u32>,
    sc_gone: Vec<u32>,
    sc_copds: Vec<Opd>,
    sc_defs: Vec<DefRec>,
    /// Debug: interior sizes of each step, by the step's op.
    interior: Map<u64, (L::Op, Vec<u32>)>,
    /// Speculative entries proposed for a cold unfold.
    entries: Map<u32, Vec<crate::lang::Entry<L::Val>>>,
    /// A segment's names from the graph it was entered from: the value
    /// each has where the segment starts.
    ext: Option<std::sync::Arc<Map<u64, L::Val>>>,
    ext_spell: Option<std::sync::Arc<SpellVec>>,
    /// Import nodes made for those, by name hash.
    imports: Map<u64, u32>,
    /// Named sources: name -> its input (a root node: never moved).
    sources: Map<u32, u32>,
    pub cfg: Config,
    rep: Report,
}

impl<L: Lang, const P: bool> Default for Graph<L, P> {
    fn default() -> Self {
        Self::new()
    }
}

/// The spacing of step labels.
const GAP: u64 = 1 << 32;

impl<L: Lang, const P: bool> Graph<L, P> {
    #[must_use]
    pub fn new() -> Self {
        let mut g = Graph {
            n: Nodes {
                h: Vec::new(),
                fams: Map::default(),
                val: Vec::new(),
                opds: Vec::new(),
                revs: Vec::new(),
                absent: L::Val::default(),
            },
            names: NameTab::default(),
            groups: Map::default(),
            steps: Vec::new(),
            unfolds: Vec::new(),
            scans: Vec::new(),
            free: Vec::new(),
            to_free: Vec::new(),
            heap: Vec::new(),
            chains: Map::default(),
            chain_readers: Map::default(),
            pubs: Map::default(),
            crosses: Map::default(),
            pred: std::sync::Arc::new(Map::default()),
            fams: Map::default(),
            fam_readers: Map::default(),
            fpred: std::sync::Arc::new(Map::default()),
            dirty_slots: std::collections::BTreeSet::new(),
            dirty_fams: std::collections::BTreeSet::new(),
            scan_runs: Vec::new(),
            hunks: Vec::new(),
            root_ord: 0,
            grp_hint: NOGROUP,
            em: Emit::default(),
            keymap: Map::default(),
            root_first: NONE,
            hint: (NONE, END, 0),
            epoch: 0,
            hot: std::collections::VecDeque::new(),
            outcomes: Map::default(),
            em_pool: Vec::new(),
            hook: Hook {
                prof: P,
                ..Hook::default()
            },
            tick: Tick::default(),
            prof: Prof::default(),
            quiet: Map::default(),
            groups_gen: 0,
            round_cool: 0,
            sampled: 0,
            step_ns: 0,
            round_size: 0,
            round_made: 0,
            round_used0: 0,
            cancel: std::sync::Arc::default(),
            lcache: None,
            closes: Map::default(),
            sc_opds: Vec::new(),
            sc_ids: Vec::new(),
            sc_gone: Vec::new(),
            sc_copds: Vec::new(),
            sc_defs: Vec::new(),
            interior: Map::default(),
            entries: Map::default(),
            ext: None,
            ext_spell: None,
            imports: Map::default(),
            sources: Map::default(),
            cfg: Config::default(),
            rep: Report::default(),
        };
        // (node 0 is the root region)
        let r = g.alloc(Kind::Root, L::Op::default(), Class::Pure, NONE, 0, 0, 0);
        debug_assert_eq!(r, ROOT);
        g.n.h[0].depth = 0;
        g
    }

    /// Reserve room for `nodes` nodes and `opds` operands.
    pub fn reserve(&mut self, nodes: usize, opds: usize) {
        self.n.h.reserve(nodes);
        self.n.val.reserve(nodes);
        self.n.opds.reserve(opds);
        self.n.revs.reserve(opds);
    }

    #[allow(clippy::too_many_arguments)]
    fn alloc(
        &mut self,
        kind: Kind,
        op: L::Op,
        class: Class,
        parent: u32,
        ord: u64,
        key: u32,
        aux: u64,
    ) -> u32 {
        let (tag, aux) = match class {
            Class::Pure => (0, aux),
            Class::Effect(c) => (1, u64::from(c.0)),
            Class::Barrier => (2, aux),
            Class::Publish(s) => (3, s.0),
            Class::Entry(_, s) => (4, s.0),
        };
        let depth = if parent == NONE {
            0
        } else {
            self.n.h[parent as usize].depth + 1
        };
        self.rep.created += 1;
        let n = &mut self.n;
        let mut hdr = Hdr {
            ord,
            aux,
            op,
            parent,
            next: NONE,
            key,
            a0: 0,
            rd: NONE,
            era: 0,
            an: 0,
            depth,
            kind,
            class: tag,
            flags: 0,
        };
        let i = if let Some(i) = self.free.pop() {
            let u = i as usize;
            hdr.era = n.h[u].era.wrapping_add(1);
            n.h[u] = hdr;
            n.val[u] = L::Val::default();
            i
        } else {
            let i = u32::try_from(n.h.len()).expect("fewer than 2^32 nodes");
            assert!(i != NONE, "fewer than 2^32 - 1 nodes");
            n.h.push(hdr);
            n.val.push(L::Val::default());
            i
        };
        if let Class::Entry(f, _) = class {
            n.fams.insert(i, f.0);
        }
        i
    }

    /// Give `n` operands `os` (a new generation: its old reverse entries
    /// go stale).
    fn set_opds(&mut self, n: u32, os: &[Opd]) {
        let u = n as usize;
        // (the same operands again: its edges stand)
        if self.n.h[u].an > 0 && self.n.opds_of(n) == os {
            return;
        }
        self.drop_readers(n);
        let era = self.n.h[u].era.wrapping_add(1);
        self.n.h[u].era = era;
        self.n.h[u].a0 = u32::try_from(self.n.opds.len()).expect("operands fit u32");
        self.n.h[u].an = u16::try_from(os.len()).expect("at most 65535 operands");
        for o in os {
            self.n.opds.push(*o);
            if o.src != NONE {
                let s = o.src as usize;
                let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
                self.n.revs.push(Rev {
                    node: n,
                    era,
                    next: self.n.h[s].rd,
                });
                self.n.h[s].rd = r;
            }
            if o.name != NONE {
                let a = self.anchor(n);
                self.insert_reader(o.name, (a, n, era));
            }
        }
    }

    /// Node `n`'s entries in the reader lists of the names it reads, taken
    /// out (in O(log R) each); a name where one is not found is pruned
    /// whole at the run's end.
    fn drop_readers(&mut self, n: u32) {
        let u = n as usize;
        let (a0, an, era) = (
            self.n.h[u].a0 as usize,
            self.n.h[u].an as usize,
            self.n.h[u].era,
        );
        if an == 0 {
            return;
        }
        let a = self.anchor(n);
        for k in 0..an {
            let m = self.n.opds[a0 + k].name;
            if m != NONE && !self.remove_reader(m, a, n, era) {
                self.mark_prune(m);
            }
        }
    }

    fn mark_prune(&mut self, m: u32) {
        let i = m as usize;
        if self.names.pruned.len() <= i {
            self.names.pruned.resize(i + 1, false);
        }
        if !self.names.pruned[i] {
            self.names.pruned[i] = true;
            self.names.prune.push(m);
        }
    }

    /// The node's anchor: its ancestor that is a root node or a step of a
    /// root unfold (itself if it is one).
    fn anchor(&self, mut n: u32) -> u32 {
        while self.n.h[n as usize].depth > 2 {
            n = self.n.h[n as usize].parent;
        }
        n
    }

    /// A position lifted to its anchor's.
    fn anchor_pos(&self, mut p: Pos) -> Pos {
        while self.n.h[p.parent as usize].depth + 1 > 2 {
            p = self.n.pos(p.parent);
        }
        p
    }

    fn add_rev(&mut self, src: u32, reader: u32) {
        let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
        self.n.revs.push(Rev {
            node: reader,
            era: self.n.h[reader as usize].era,
            next: self.n.h[src as usize].rd,
        });
        self.n.h[src as usize].rd = r;
    }

    // ---- the worklist ----

    fn heap_less(&self, a: u32, b: u32) -> bool {
        self.n.cmp_node(a, b) == Ordering::Less
    }

    fn push_dirty(&mut self, n: u32) {
        let f = &mut self.n.h[n as usize].flags;
        *f |= DIRTY;
        if *f & (QUEUED | DEAD) != 0 {
            return;
        }
        *f |= QUEUED;
        self.heap.push(n);
        let mut i = self.heap.len() - 1;
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap_less(self.heap[i], self.heap[p]) {
                self.heap.swap(i, p);
                i = p;
            } else {
                break;
            }
        }
    }

    fn pop(&mut self) -> Option<u32> {
        let top = *self.heap.first()?;
        let last = self.heap.pop().expect("non-empty");
        if !self.heap.is_empty() {
            self.heap[0] = last;
            let mut i = 0;
            loop {
                let (l, r) = (2 * i + 1, 2 * i + 2);
                let mut m = i;
                if l < self.heap.len() && self.heap_less(self.heap[l], self.heap[m]) {
                    m = l;
                }
                if r < self.heap.len() && self.heap_less(self.heap[r], self.heap[m]) {
                    m = r;
                }
                if m == i {
                    break;
                }
                self.heap.swap(i, m);
                i = m;
            }
        }
        self.n.h[top as usize].flags &= !QUEUED;
        Some(top)
    }

    // ---- values and waking ----

    /// Set `n`'s value; wake its readers whose read changed. Returns
    /// whether the version changed.
    fn set_val(&mut self, n: u32, v: L::Val) -> bool {
        let u = n as usize;
        if self.n.val[u].ver() == v.ver() {
            return false;
        }
        let old = std::mem::replace(&mut self.n.val[u], v);
        self.wake(n, &old);
        self.class_changed(n);
        true
    }

    /// A node's class-side effect changed (its payload, or it came or
    /// went): its chain's readers woken, its slot and family marked.
    fn class_changed(&mut self, n: u32) {
        match self.n.class(n) {
            Class::Effect(c) => self.wake_chain_at(c.0, n),
            Class::Publish(s) => {
                self.dirty_slots.insert(s.0);
            }
            Class::Entry(f, s) => {
                self.dirty_slots.insert(s.0);
                self.dirty_fams.insert(f.0);
            }
            _ => {}
        }
    }

    fn wake_chain(&mut self, c: u32) {
        self.wake_chain_at(c, NONE);
    }

    /// Wake chain `c`'s readers that see payload `p` (`NONE`: all): a
    /// bounded read only if it comes after `p`.
    fn wake_chain_at(&mut self, c: u32, p: u32) {
        if let Some(rs) = self.chain_readers.get(&c) {
            let rs = rs.clone();
            for r in rs {
                if self.n.is_dead(r) {
                    continue;
                }
                if p != NONE
                    && self.n.h[r as usize].aux & BEFORE != 0
                    && self.n.cmp_node(r, p) != Ordering::Greater
                {
                    continue;
                }
                self.rep.woken += 1;
                self.push_dirty(r);
            }
        }
    }

    /// Wake the readers of `n` whose read of it changed from `old`.
    fn wake(&mut self, n: u32, old: &L::Val) {
        let mut prev = NONE;
        let mut e = self.n.h[n as usize].rd;
        while e != NONE {
            let Rev { node: r, era, next } = self.n.revs[e as usize];
            let ru = r as usize;
            if self.n.h[ru].era != era || self.n.h[ru].flags & DEAD != 0 {
                // (stale: unlinked)
                if prev == NONE {
                    self.n.h[n as usize].rd = next;
                } else {
                    self.n.revs[prev as usize].next = next;
                }
                e = next;
                continue;
            }
            let new = &self.n.val[n as usize];
            // (a step reads the state before it against the one it ran
            // from: a step inserted before it that ends in that same state
            // changes nothing for it)
            let step = self.n.h[ru].kind == Kind::Step;
            let changed = self.n.opds_of(r).iter().enumerate().any(|(k, o)| {
                o.src == n
                    && if step && k == 0 {
                        self.steps[self.n.h[ru].aux as usize].in_ver != new.ver()
                    } else {
                        o.sel.ver(old) != o.sel.ver(new)
                    }
            });
            if changed {
                self.rep.woken += 1;
                self.push_dirty(r);
            }
            prev = e;
            e = next;
        }
    }

    // ---- the client's root region ----

    fn root_node(&mut self, kind: Kind, op: L::Op, class: Class, aux: u64) -> u32 {
        self.root_ord += 1;
        let ord = self.root_ord;
        let key = u32::try_from(ord & 0x7fff_ffff).unwrap_or(0);
        let n = self.alloc(kind, op, class, ROOT, ord, key, aux);
        // (root nodes in order: linked at the end)
        self.link_last(ROOT, n);
        n
    }

    fn link_last(&mut self, parent: u32, n: u32) {
        // root regions are appended to; keep a tail in `aux` of the root
        let tail = self.n.h[parent as usize].aux;
        if self.first(parent) == NONE {
            self.set_first(parent, n);
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let t = tail as u32;
            self.n.h[t as usize].next = n;
        }
        self.n.h[parent as usize].aux = u64::from(n);
    }

    /// An input: a value the client sets between runs.
    pub fn input(&mut self, v: L::Val) -> NodeId {
        let n = self.root_node(Kind::Input, L::Op::default(), Class::Pure, 0);
        self.n.val[n as usize] = v;
        NodeId(n)
    }

    /// The named source `key` (DESIGN 7.22: a file `\\input`,
    /// `\\include` or `\\openin` reads): an input steps read as a name
    /// (`StepCx::source`). Made, or set if it exists; a source made after
    /// steps found it absent wakes them. Edit it with `set` on the node.
    pub fn source(&mut self, key: &[u8], v: L::Val) -> NodeId {
        let m = self.intern(&source_spelling(key));
        if let Some(&n) = self.sources.get(&m) {
            self.set(NodeId(n), v);
            return NodeId(n);
        }
        let i = self.root_node(Kind::Input, L::Op::default(), Class::Pure, 0);
        self.n.val[i as usize] = v;
        let ri = u32::try_from(self.names.recs.len()).expect("definitions fit u32");
        self.names.recs.push(DefRec {
            name: m,
            step: ROOT,
            sub: 0,
            src: i,
            sel: Sel::WHOLE,
            group: NOGROUP,
            global: true,
        });
        self.names.defs[m as usize].insert(0, ri);
        self.names.gens[m as usize] += 1;
        self.sources.insert(m, i);
        self.reresolve(
            m,
            Pos {
                parent: ROOT,
                ord: 0,
            },
        );
        NodeId(i)
    }

    /// The named source `key` removed (the file is gone): its readers
    /// read it as absent. False if there was none.
    pub fn remove_source(&mut self, key: &[u8]) -> bool {
        let Some(m) = self.name_id(&source_spelling(key)).map(|n| n.0) else {
            return false;
        };
        let Some(i) = self.sources.remove(&m) else {
            return false;
        };
        let at = Pos {
            parent: ROOT,
            ord: 0,
        };
        self.remove_def(m, at);
        self.reresolve(m, at);
        // (the input itself stays, read by nothing now: a root node is
        // never unlinked)
        self.n.val[i as usize] = L::Val::default();
        true
    }

    /// Set an input's value.
    pub fn set(&mut self, n: NodeId, v: L::Val) {
        debug_assert_eq!(self.n.h[n.0 as usize].kind, Kind::Input);
        self.set_val(n.0, v);
    }

    fn opd((n, sel): (NodeId, Sel)) -> Opd {
        Opd {
            src: n.0,
            sel,
            name: NONE,
        }
    }

    /// A leaf of the root region.
    pub fn leaf(&mut self, op: L::Op, class: Class, args: &[(NodeId, Sel)]) -> NodeId {
        let n = self.root_node(Kind::Leaf, op, class, 0);
        let mut os = std::mem::take(&mut self.sc_opds);
        os.clear();
        os.extend(args.iter().map(|&(a, sel)| Opd {
            src: a.0,
            sel,
            name: NONE,
        }));
        self.set_opds(n, &os);
        self.sc_opds = os;
        self.register(n);
        self.settle_new(n);
        NodeId(n)
    }

    /// An unfold of the root region.
    pub fn unfold(&mut self, op: L::Op, input: NodeId, init: NodeId, args: &[NodeId]) -> NodeId {
        let n = self.root_node(Kind::Unfold, op, Class::Pure, 0);
        let mut os = vec![
            Self::opd((input, Sel::WHOLE)),
            Self::opd((init, Sel::WHOLE)),
        ];
        os.extend(args.iter().map(|&a| Self::opd((a, Sel::WHOLE))));
        self.set_opds(n, &os);
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// A scan of the root region.
    pub fn scan(
        &mut self,
        op: L::Op,
        input: (NodeId, Sel),
        init: NodeId,
        args: &[NodeId],
    ) -> NodeId {
        let n = self.root_node(Kind::Scan, op, Class::Pure, 0);
        let mut os = vec![Self::opd(input), Self::opd((init, Sel::WHOLE))];
        os.extend(args.iter().map(|&a| Self::opd((a, Sel::WHOLE))));
        self.set_opds(n, &os);
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// A read of chain `c`, in the root region (place it after what it
    /// reads).
    pub fn chain_read(&mut self, c: Chain) -> NodeId {
        let n = self.root_node(
            Kind::ChainRead,
            L::Op::default(),
            Class::Pure,
            u64::from(c.0),
        );
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// Slot `s`'s predicted value, in the root region.
    pub fn cross(&mut self, s: Slot) -> NodeId {
        let n = self.root_node(Kind::Cross, L::Op::default(), Class::Pure, s.0);
        self.register(n);
        self.settle_new(n);
        NodeId(n)
    }

    /// Family `f`'s entries from the last run, in the root region.
    pub fn family(&mut self, f: Fam) -> NodeId {
        let n = self.root_node(Kind::Family, L::Op::default(), Class::Pure, u64::from(f.0));
        self.register(n);
        self.settle_new(n);
        NodeId(n)
    }

    /// Set up a new node's side tables (unfold, scan, chains, slots).
    fn register(&mut self, n: u32) {
        let u = n as usize;
        match self.n.h[u].kind {
            Kind::Unfold => {
                let grp0 = self.cur_grp_for(n);
                self.n.h[u].aux = self.unfolds.len() as u64;
                self.unfolds.push(UnfoldInfo {
                    first: NONE,
                    indexed: false,
                    keys: Map::default(),
                    owners: Map::default(),
                    input: None,
                    args_ver: Ver::ABSENT,
                    hunk_end: None,
                    grp0,
                    stop: None,
                    parked: None,
                    start: None,
                });
            }
            Kind::Scan => {
                self.n.h[u].aux = self.scans.len() as u64;
                self.scans.push(ScanInfo {
                    input: Seq::new(),
                    states: Seq::new(),
                    outs: Seq::new(),
                    init_ver: Ver::ABSENT,
                    args_ver: Ver::ABSENT,
                    done: false,
                });
            }
            Kind::Cross => {
                self.crosses.entry(self.n.h[u].aux).or_default().push(n);
            }
            Kind::ChainRead => {
                #[allow(clippy::cast_possible_truncation)]
                let c = self.n.h[u].aux as u32;
                self.chain_readers.entry(c).or_default().push(n);
            }
            Kind::Family => {
                self.fam_readers
                    .entry(self.n.h[u].aux as u32)
                    .or_default()
                    .push(n);
            }
            _ => {}
        }
        self.register_class(n);
    }

    fn register_class(&mut self, n: u32) {
        match self.n.class(n) {
            Class::Effect(c) => {
                let at = self.n.pos(n);
                let g = &self.n;
                let list = self.chains.entry(c.0).or_default();
                let k = match list.last() {
                    Some(&l) if g.cmp_pos(g.pos(l), at) == Ordering::Greater => {
                        list.partition_point(|&x| g.cmp_pos(g.pos(x), at) == Ordering::Less)
                    }
                    _ => list.len(),
                };
                list.insert(k, n);
            }
            Class::Publish(s) => {
                self.pubs.entry(s.0).or_default().push(n);
                self.dirty_slots.insert(s.0);
            }
            Class::Entry(f, s) => {
                self.pubs.entry(s.0).or_default().push(n);
                self.fams.entry(f.0).or_default().push(n);
                self.dirty_slots.insert(s.0);
                self.dirty_fams.insert(f.0);
            }
            _ => {}
        }
    }

    fn cur_grp_for(&self, n: u32) -> u64 {
        // (an unfold a step made: the group open at its emission, kept by
        // the step processing in `grp_hint`)
        let _ = n;
        self.grp_hint
    }
}

impl<L: Lang, const P: bool> Graph<L, P> {
    /// The first child (or step) of a step, an unfold or the root.
    pub(crate) fn first(&self, n: u32) -> u32 {
        let h = &self.n.h[n as usize];
        match h.kind {
            Kind::Step => self.steps[h.aux as usize].first,
            Kind::Unfold => self.unfolds[h.aux as usize].first,
            Kind::Root => self.root_first,
            _ => NONE,
        }
    }

    pub(crate) fn set_first(&mut self, n: u32, v: u32) {
        let h = self.n.h[n as usize];
        match h.kind {
            Kind::Step => self.steps[h.aux as usize].first = v,
            Kind::Unfold => self.unfolds[h.aux as usize].first = v,
            Kind::Root => self.root_first = v,
            _ => debug_assert!(v == NONE, "only steps, unfolds and the root have children"),
        }
    }

    /// The children (or steps) of `n`, in order.
    pub(crate) fn children(&self, n: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = self.first(n);
        while c != NONE {
            out.push(c);
            c = self.n.h[c as usize].next;
        }
        out
    }

    /// A new node: evaluated now if it is a leaf whose operands are all
    /// settled (in position order nothing before it changes again), else
    /// queued.
    fn settle_new(&mut self, n: u32) {
        let u = n as usize;
        let leafy = matches!(
            self.n.h[u].kind,
            Kind::Leaf | Kind::Const | Kind::Cross | Kind::Family
        );
        let clean = self
            .n
            .opds_of(n)
            .iter()
            .all(|o| o.src == NONE || self.n.h[o.src as usize].flags & DIRTY == 0);
        if leafy && clean {
            self.n.h[u].flags &= !DIRTY;
            self.process_kind(n);
        } else {
            self.push_dirty(n);
        }
    }

    fn process(&mut self, n: u32) {
        let f = self.n.h[n as usize].flags;
        if f & DEAD != 0 || f & DIRTY == 0 {
            return;
        }
        self.n.h[n as usize].flags &= !DIRTY;
        self.process_kind(n);
    }

    fn process_kind(&mut self, n: u32) {
        match self.n.h[n as usize].kind {
            Kind::Leaf => self.eval_leaf(n),
            Kind::Root | Kind::Input | Kind::Const => {}
            Kind::Cross => {
                let v = self
                    .pred
                    .get(&self.n.h[n as usize].aux)
                    .cloned()
                    .unwrap_or_default();
                self.rep.evals += 1;
                self.rep.cross_evals += 1;
                self.set_val(n, v);
            }
            Kind::Family => {
                let f = self.n.h[n as usize].aux as u32;
                let v = self
                    .fpred
                    .get(&f)
                    .cloned()
                    .unwrap_or_else(|| L::chain_val(Seq::new()));
                self.rep.evals += 1;
                self.rep.cross_evals += 1;
                self.set_val(n, v);
            }
            Kind::ChainRead => self.eval_chain(n),
            Kind::Step => self.run_chain(n),
            Kind::Unfold => {
                if let Some(s) = self.run_unfold(n) {
                    self.run_chain(s);
                }
            }
            Kind::Scan => self.run_scan(n),
        }
    }

    /// A scan element through the memo store: keyed by the op, the
    /// state, the element and the arguments (valid in any scan of any
    /// document).
    fn scan_memo(
        &self,
        op: L::Op,
        st: &L::Val,
        x: &L::Val,
        args: &Args<'_, L>,
        av: Ver,
    ) -> (L::Val, L::Val) {
        let parts = [st.ver(), x.ver(), av];
        let ks = Ver::node(L::op_tag(op) ^ SCAN_STATE, &parts);
        let ko = Ver::node(L::op_tag(op) ^ SCAN_OUT, &parts);
        let mut m = self.hook.memo.lock().expect("the memo store");
        let hit = match (m.get(ks).cloned(), m.get(ko).cloned()) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        m.note(L::op_tag(op), hit.is_some());
        drop(m);
        if let Some(h) = hit {
            return h;
        }
        let (a, b) = L::scan(op, st, x, args);
        let mut m = self.hook.memo.lock().expect("the memo store");
        m.put(ks, a.clone(), a.bytes());
        m.put(ko, b.clone(), b.bytes());
        (a, b)
    }

    fn eval_leaf(&mut self, n: u32) {
        let v = {
            let args = Args::of(&self.n, self.n.opds_of(n));
            let hook = (P || self.hook.memo_on).then_some(&self.hook);
            let (v, ev) = eval_op(hook, &mut self.tick, self.n.h[n as usize].op, &args);
            if let Some(ev) = ev {
                self.prof.fold(ev);
            }
            v
        };
        self.rep.evals += 1;
        self.set_val(n, v);
    }

    /// The live nodes of chain `c` in position order.
    fn chain_nodes(&self, c: u32) -> Vec<u32> {
        let mut ns: Vec<u32> = self
            .chains
            .get(&c)
            .map(|v| v.iter().copied().filter(|&x| !self.n.is_dead(x)).collect())
            .unwrap_or_default();
        ns.dedup();
        ns
    }

    fn eval_chain(&mut self, n: u32) {
        let aux = self.n.h[n as usize].aux;
        #[allow(clippy::cast_possible_truncation)]
        let c = aux as u32;
        // (in position order already; a bounded read takes those before it)
        let items: Vec<(ElemId, L::Val)> = match self.chains.get(&c) {
            None => Vec::new(),
            Some(list) => {
                let k = if aux & BEFORE == 0 {
                    list.len()
                } else {
                    let at = self.n.pos(n);
                    list.partition_point(|&x| self.n.cmp_pos(self.n.pos(x), at) == Ordering::Less)
                };
                let mut last = NONE;
                list.iter()
                    .take(k)
                    .copied()
                    .filter(|&x| {
                        let keep = x != last && !self.n.is_dead(x);
                        last = x;
                        keep
                    })
                    .map(|x| (ElemId(u64::from(x)), self.n.val[x as usize].clone()))
                    .collect()
            }
        };
        let v = L::chain_val(Seq::from_vec(items));
        self.rep.evals += 1;
        self.set_val(n, v);
    }

    // ---- unfolds ----

    fn args_ver(&self, n: u32, from: usize) -> Ver {
        let vs: Vec<Ver> = self.n.opds_of(n)[from..]
            .iter()
            .map(|o| self.n.read_ver(o))
            .collect();
        Ver::node(0x6172_6773, &vs)
    }

    /// An unfold's input, initial state or arguments changed (or it is
    /// new): the step to run first, if any (others are queued).
    fn run_unfold(&mut self, u: u32) -> Option<u32> {
        let ui = self.n.h[u as usize].aux as usize;
        let uo: Vec<Opd> = self.n.opds_of(u).to_vec();
        let input = L::as_seq(&self.n.read(&uo[0])).cloned().unwrap_or_default();
        let args_ver = self.args_ver(u, 2);
        let first = self.first(u);
        let init = Opd {
            src: uo[1].src,
            sel: uo[1].sel,
            name: NONE,
        };
        if first == NONE {
            let (key0, i0) = self.unfolds[ui].start.unwrap_or((0, 0));
            let at = input.get(i0).map_or(END, |e| e.0);
            let grp = self.unfolds[ui].grp0;
            if self.cfg.workers > 1 && !self.cfg.segment && self.n.h[u as usize].parent == ROOT {
                let ents = {
                    let args = Args::of(&self.n, &self.n.opds_of(u)[2..]);
                    L::entries(self.n.h[u as usize].op, &self.n.read(&uo[0]), &args)
                };
                if let Some(e) = ents.iter().map(|e| e.at).filter(|&a| a > 0).min() {
                    self.unfolds[ui].stop = Some(e);
                    self.entries.insert(u, ents);
                }
            }
            let s = self.new_step(u, NONE, key0, at, grp, init);
            self.unfolds[ui].input = Some(input);
            self.unfolds[ui].args_ver = args_ver;
            return Some(s);
        }
        let mut start = None;
        if self.n.opds_of(first)[0] != init {
            let mut os: Vec<Opd> = self.n.opds_of(first).to_vec();
            os[0] = init;
            self.set_opds(first, &os);
            start = Some(first);
        }
        if self.unfolds[ui].args_ver != args_ver {
            self.unfolds[ui].args_ver = args_ver;
            for s in self.children(u) {
                self.push_dirty(s);
            }
            start = Some(first);
        }
        let old = self.unfolds[ui].input.take().unwrap_or_default();
        if old.ver() != input.ver() || old.len() != input.len() {
            let h = input.diff(&old);
            self.unfolds[ui].hunk_end = Some(input.len() - h.suffix);
            self.hunks.push(u);
            self.index_steps(u);
            let mut s = first;
            let mut i = h.prefix;
            while i > 0 {
                i -= 1;
                let id = old.get(i).expect("in the old input").0;
                if let Some(&o) = self.unfolds[ui].owners.get(&id)
                    && !self.n.is_dead(o)
                {
                    s = o;
                    break;
                }
            }
            if s == first {
                // (the first step starts at the input's start, wherever that is now)
                let fi = self.n.h[first as usize].aux as usize;
                self.steps[fi].at = input.get(0).map_or(END, |e| e.0);
            }
            if start.is_some() && start != Some(s) {
                self.push_dirty(s);
            } else {
                start = Some(s);
            }
        }
        self.unfolds[ui].input = Some(input);
        // (input indices may have moved)
        self.hint = (NONE, END, 0);
        start
    }

    /// Build unfold `u`'s key and owner maps if they are not kept yet.
    fn index_steps(&mut self, u: u32) {
        let ui = self.n.h[u as usize].aux as usize;
        if self.unfolds[ui].indexed {
            return;
        }
        let mut keys = std::mem::take(&mut self.unfolds[ui].keys);
        let mut owners = std::mem::take(&mut self.unfolds[ui].owners);
        keys.clear();
        owners.clear();
        let mut c = self.unfolds[ui].first;
        while c != NONE {
            let si = &self.steps[self.n.h[c as usize].aux as usize];
            keys.insert(si.key, c);
            if si.took > 0 {
                owners.insert(si.at, c);
            }
            c = self.n.h[c as usize].next;
        }
        let info = &mut self.unfolds[ui];
        info.keys = keys;
        info.owners = owners;
        info.indexed = true;
    }

    /// A new step of unfold `u` after `prev` (`NONE`: the first).
    fn new_step(&mut self, u: u32, prev: u32, key: u64, at: ElemId, grp: u64, opd0: Opd) -> u32 {
        let ui = self.n.h[u as usize].aux as usize;
        let after = if prev == NONE {
            NONE
        } else {
            self.n.h[prev as usize].next
        };
        let ord = self.step_ord(u, prev, after);
        #[allow(clippy::cast_possible_truncation)]
        let k32 = key as u32;
        let si = self.steps.len() as u64;
        let s = self.alloc(
            Kind::Step,
            self.n.h[u as usize].op,
            Class::Pure,
            u,
            ord,
            k32,
            si,
        );
        self.steps.push(StepInfo {
            unfold: u,
            first: NONE,
            key,
            at,
            took: 0,
            grp_in: grp,
            in_ver: Ver::ABSENT,
            d0: 0,
            dn: 0,
            ran: self.epoch,
            work: 0,
            folded: 0,
        });
        if prev == NONE {
            self.n.h[s as usize].next = self.first(u);
            self.set_first(u, s);
        } else {
            self.n.h[s as usize].next = after;
            self.n.h[prev as usize].next = s;
        }
        if self.unfolds[ui].indexed {
            self.unfolds[ui].keys.insert(key, s);
        }
        self.set_opds(s, &[opd0]);
        if after != NONE {
            // (the old successor now follows the new step)
            let mut os: Vec<Opd> = self.n.opds_of(after).to_vec();
            os[0] = Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            };
            self.set_opds(after, &os);
        }
        s
    }

    /// A label between `prev` and `after` among `u`'s steps, relabeling
    /// them all if there is no room.
    fn step_ord(&mut self, u: u32, prev: u32, after: u32) -> u64 {
        let lo = if prev == NONE {
            0
        } else {
            self.n.h[prev as usize].ord
        };
        if after == NONE {
            return lo + GAP;
        }
        let hi = self.n.h[after as usize].ord;
        if hi - lo > 1 {
            // (a run of insertions after one step takes small gaps, so
            // many fit before a relabel)
            let gap = hi - lo;
            return lo + if gap > 1 << 17 { 1 << 16 } else { gap / 2 };
        }
        // (no room: spread every step of `u` again, keeping the order)

        let mut c = self.first(u);
        let mut k = 0u64;
        let mut found = 0;
        while c != NONE {
            k += 1;
            self.n.h[c as usize].ord = k * GAP;
            if c == prev {
                k += 1;
                found = k * GAP;
            }
            c = self.n.h[c as usize].next;
        }
        if prev == NONE {
            // (before the first: shift all by one gap)
            let mut c = self.first(u);
            while c != NONE {
                self.n.h[c as usize].ord += GAP;
                c = self.n.h[c as usize].next;
            }
            return GAP;
        }
        found
    }

    /// Run step `s`: its op, its emissions matched to its old children,
    /// its definitions and scopes, then its successor.
    #[allow(clippy::too_many_lines)]
    fn run_step(&mut self, s: u32) -> Option<u32> {
        self.rep.steps += 1;
        // (a sealed region runs again as its first step: the steps after
        // it are made again until one meets the step that followed it)
        self.n.h[s as usize].flags &= !SEALED;
        let si = self.n.h[s as usize].aux as usize;
        let u = self.steps[si].unfold;
        let ui = self.n.h[u as usize].aux as usize;
        let input = self.unfolds[ui].input.clone();
        let at = self.steps[si].at;
        let start = match &input {
            None => 0,
            Some(inp) if at == END => inp.len(),
            // (the step before this one just ended here)
            Some(_) if self.hint.0 == u && self.hint.1 == at => self.hint.2,
            Some(inp) => inp.index_of(at).unwrap_or(inp.len()),
        };
        let prev_o = self.n.opds_of(s)[0];
        let dry = if self.outcomes.is_empty() {
            None
        } else {
            self.take_outcome(s)
        };
        let (start, res, idx, grp_out, end_cursor, in_ver) = if let Some(d) = dry {
            let old = std::mem::replace(&mut self.em, d.em);
            self.em_pool.push(old);
            (d.start, d.res, d.idx, d.grp_out, d.end_cursor, d.in_ver)
        } else {
            self.em.clear();
            if let Some(inp) = &input
                && !self.lcache.as_ref().is_some_and(|l| l.holds(inp, start))
            {
                self.lcache = inp.leaf(start);
            }
            let (res, idx, grp_out, end_cursor, in_ver) = {
                let Graph {
                    n,
                    names,
                    groups,
                    em,
                    steps,
                    ext,
                    ext_spell,
                    closes,
                    unfolds,
                    cfg,
                    lcache,
                    cancel,
                    hook,
                    tick,
                    sampled,
                    step_ns,
                    ..
                } = self;
                // (the cached leaf, if it is this input's and holds the start)
                let (leaf, lbase): (&[(ElemId, L::Val)], usize) = match (lcache.as_ref(), &input) {
                    (Some(l), Some(inp)) if l.holds(inp, start) => (l.elems(), l.base()),
                    _ => (&[], 0),
                };
                let uo = n.opds_of(u);
                let mut cx = StepCx {
                    g: n,
                    names,
                    groups,
                    unfold_opds: uo,
                    input: input.as_ref(),
                    idx: start,
                    start,
                    leaf,
                    lbase,
                    step: s,
                    pos: n.pos(s),
                    grp: steps[si].grp_in,
                    opened: 0,
                    em,
                    ext: ext.as_deref(),
                    ext_spell: ext_spell.as_deref().map(Vec::as_slice),
                    steps,
                    unfolds,
                    closes,
                    keep: cfg.keep_interior,
                    cancel,
                    hook: (P || hook.memo_on).then_some(&*hook),
                    tick: *tick,
                    _brand: PhantomData,
                };
                let args = Args::of(n, &uo[2..]);
                let st = n.read(&prev_o);
                let in_ver = st.ver();
                // (one step in 32 timed: what an op costs decides whether a
                // parallel round can pay for itself)
                *sampled = sampled.wrapping_add(1);
                let t = if *sampled % 32 == 0 { now() } else { None };
                let res = L::step(n.h[s as usize].op, &st, &args, &mut cx);
                if let Some(t) = t {
                    #[allow(clippy::cast_possible_truncation, reason = "a step under 584 years")]
                    let ns = t.elapsed().as_nanos() as u64;
                    *step_ns = (*step_ns * 7 + ns) / 8;
                }
                *tick = cx.tick;
                (res, cx.idx, cx.grp, cx.cursor(), in_ver)
            };
            (start, res, idx, grp_out, end_cursor, in_ver)
        };
        if self.cfg.debug {
            let b = match &res {
                Step::Next { st, .. } => st.bytes(),
                Step::Done(v) => v.bytes(),
                Step::Call { .. } => 0,
            };
            self.rep.state_max = self.rep.state_max.max(b);
            self.rep.state_sum += b as u64;
        }
        // (read now: a nested unfold running in the sweep reuses `em`)
        let call_ix = self.em.call;
        self.rep.merged += u64::from(self.em.merged);
        if P {
            let em = &mut self.em;
            for ev in em.ev.drain(..) {
                self.prof.fold(ev);
            }
            for (op, hit) in em.cse_ev.drain(..) {
                self.prof.cse(op, hit);
            }
            for op in em.impure.drain(..) {
                self.prof.impure(op);
            }
            let rerun = self.epoch > 1;
            self.prof.step(self.n.h[s as usize].op, rerun);
            if rerun {
                self.prof.region(self.steps[si].key, self.epoch);
            }
        }
        // names the step made
        let new_names = std::mem::take(&mut self.em.new_names);
        for (h, sp) in new_names {
            let id = u32::try_from(self.names.spell.len()).expect("names fit u32");
            self.names.by_hash.insert(h, id);
            self.names.hashes.push(h);
            self.names.spell.push(sp);
            self.names.defs.push(Runs::default());
            self.names.readers.push(Runs::default());
            self.names.gens.push(0);
        }
        // which emissions become nodes ("big"); the rest is the sweep's
        let ids = self.match_children(s);
        self.grp_hint = NOGROUP;
        self.apply_groups(s, si);
        let touched = self.apply_defs(s, si, &ids);
        // the step's own operands: its state, then the names it read, then
        // what its interior read from outside (added by the sweep)
        let mut os = std::mem::take(&mut self.sc_opds);
        os.clear();
        os.push(prev_o);
        let reads = std::mem::take(&mut self.em.reads);
        let at_s = self.n.pos(s);
        for &(m, o) in &reads {
            os.push(if o.src == NONE {
                self.resolve_or_import(m, at_s)
            } else {
                o
            });
        }
        self.em.reads = reads;
        self.sweep(s, &ids, &mut os);
        // (a call ran in the sweep: the step depends on its result, so an
        // edit inside the called unfold that changes it resumes here)
        let call = call_ix.map(|ix| ids[ix as usize]);
        if let Some(c) = call {
            os.push(Opd {
                src: c,
                sel: Sel::WHOLE,
                name: NONE,
            });
        }
        // (readers of what the step defines, now that its sources have
        // their values)
        if !touched.is_empty() {
            let from = self.n.pos(s);
            for m in touched {
                self.reresolve(m, from);
            }
        }
        self.sc_ids = ids;
        self.set_opds(s, &os);
        self.sc_opds = os;
        // its input range
        let took = u32::try_from(idx - start).expect("took fits u32");
        self.hint = (u, end_cursor, idx);
        let old_at = self.steps[si].at;
        if self.unfolds[ui].indexed {
            if self.steps[si].took > 0 && self.unfolds[ui].owners.get(&old_at) == Some(&s) {
                self.unfolds[ui].owners.remove(&old_at);
            }
            if took > 0 {
                self.unfolds[ui].owners.insert(at, s);
            }
        }
        let work = u32::try_from(self.em.specs.len()).expect("emissions fit u32");
        let epoch = self.epoch;
        let st_info = &mut self.steps[si];
        st_info.took = took;
        st_info.in_ver = in_ver;
        st_info.ran = epoch;
        st_info.work = work;
        st_info.folded = 0;
        if epoch > 1 && self.cfg.seal > 0 {
            let due = epoch + self.quiet_of(si);
            // (in order of due run: with quiet times per region, not
            // always the last)
            if self.hot.back().is_none_or(|b| b.0 <= due) {
                self.hot.push_back((due, s));
            } else {
                let k = self.hot.partition_point(|b| b.0 <= due);
                self.hot.insert(k, (due, s));
            }
        }
        match res {
            Step::Done(v) => {
                self.set_val(s, v.clone());
                let mut c = self.n.h[s as usize].next;
                while c != NONE {
                    let nx = self.n.h[c as usize].next;
                    self.remove(c);
                    c = nx;
                }
                self.n.h[s as usize].next = NONE;
                self.set_val(u, v);
                None
            }
            Step::Next { st, key } => {
                let ver = st.ver();
                self.set_val(s, st);
                self.successor(s, u, ui, key, end_cursor, idx, grp_out, ver)
            }
            Step::Call { key } => {
                let c = call.expect("Step::Call after StepCx::call");
                let v = self.n.val[c as usize].clone();
                let ver = v.ver();
                self.set_val(s, v);
                // (the call's result was set while it ran in the sweep,
                // before this step read it: not a reason to run again)
                self.n.h[s as usize].flags &= !DIRTY;
                self.successor(s, u, ui, key, end_cursor, idx, grp_out, ver)
            }
        }
    }

    /// Run steps from `s` on while each one's successor must run.
    fn run_chain(&mut self, s: u32) {
        let mut s = s;
        loop {
            self.n.h[s as usize].flags &= !DIRTY;
            match self.run_step(s) {
                Some(nx) => s = nx,
                None => break,
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn successor(
        &mut self,
        s: u32,
        u: u32,
        ui: usize,
        key: u64,
        cursor: ElemId,
        idx: usize,
        grp: u64,
        ver: Ver,
    ) -> Option<u32> {
        if let Some(stop) = self.unfolds[ui].stop
            && idx >= stop
            && self.n.h[s as usize].next == NONE
        {
            self.unfolds[ui].stop = None;
            self.unfolds[ui].parked = Some(Parked {
                s,
                key,
                cursor,
                idx,
                grp,
                ver,
            });
            if !self.cfg.segment {
                self.speculate(u);
            }
            return None;
        }
        let input_ok = self.unfolds[ui].hunk_end.is_none_or(|h| idx >= h);
        let q = self.n.h[s as usize].next;
        let fits = |g: &Self, j: u32| {
            let sj = &g.steps[g.n.h[j as usize].aux as usize];
            sj.key == key && sj.at == cursor && sj.grp_in == grp
        };
        if q != NONE && fits(self, q) {
            let qi = self.n.h[q as usize].aux as usize;
            return (self.steps[qi].in_ver != ver || !input_ok).then_some(q);
        }
        if q != NONE {
            self.index_steps(u);
        }
        if q != NONE
            && let Some(&j) = self.unfolds[ui].keys.get(&key)
            && j != s
            && !self.n.is_dead(j)
            && self.n.h[j as usize].parent == u
            && self.n.h[j as usize].ord > self.n.h[s as usize].ord
            && fits(self, j)
        {
            let mut c = q;
            while c != j {
                let nx = self.n.h[c as usize].next;
                self.remove(c);
                c = nx;
            }
            self.n.h[s as usize].next = j;
            let mut os: Vec<Opd> = self.n.opds_of(j).to_vec();
            os[0] = Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            };
            self.set_opds(j, &os);
            let ji = self.n.h[j as usize].aux as usize;
            return (self.steps[ji].in_ver != ver || !input_ok).then_some(j);
        }
        let n = self.new_step(
            u,
            s,
            key,
            cursor,
            grp,
            Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            },
        );
        Some(n)
    }

    /// Match the step's big emissions to its old children by key: the
    /// ids, in emission order (`NONE` for the interior). Old children not
    /// matched are removed.
    fn match_children(&mut self, s: u32) -> Vec<u32> {
        self.keymap.clear();
        let mut c = self.first(s);
        while c != NONE {
            self.keymap.insert(self.n.h[c as usize].key, c);
            c = self.n.h[c as usize].next;
        }
        let specs = std::mem::take(&mut self.em.specs);
        let mut bigs = std::mem::take(&mut self.em.bigs);
        bigs.sort_unstable();
        let mut ids = std::mem::take(&mut self.sc_ids);
        ids.clear();
        ids.resize(specs.len(), NONE);
        let mut gone = std::mem::take(&mut self.sc_gone);
        gone.clear();
        for (nbig, &i) in bigs.iter().enumerate() {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "emissions of a step fit u32"
            )]
            let nbig = nbig as u32;
            let sp = &specs[i as usize];
            // (an explicit key, else the ordinal among the big ones)
            let key = if sp.key & 0x8000_0000 != 0 {
                sp.key
            } else {
                nbig
            };
            let cand = self.keymap.remove(&key);
            let reuse = cand.filter(|&o| {
                let u = o as usize;
                self.n.h[u].kind == sp.kind
                    && self.n.h[u].op == sp.op
                    && self.n.class(o) == sp.class()
                    && {
                        // (an unfold's or scan's table index is its aux)
                        matches!(
                            sp.kind,
                            Kind::Unfold | Kind::Scan | Kind::Leaf | Kind::Const
                        ) || self.n.h[u].aux == sp.aux
                    }
            });
            if reuse.is_none()
                && let Some(o) = cand
            {
                gone.push(o);
            }
            let id = if let Some(o) = reuse {
                self.n.h[o as usize].ord = u64::from(sp.sub);
                if sp.kind == Kind::Unfold {
                    let ui = self.n.h[o as usize].aux as usize;
                    if self.unfolds[ui].grp0 != sp.grp {
                        self.unfolds[ui].grp0 = sp.grp;
                        let f = self.first(o);
                        if f != NONE {
                            let fi = self.n.h[f as usize].aux as usize;
                            self.steps[fi].grp_in = sp.grp;
                            self.push_dirty(f);
                        }
                    }
                }
                o
            } else {
                self.grp_hint = sp.grp;
                let o = self.alloc(
                    sp.kind,
                    sp.op,
                    sp.class(),
                    s,
                    u64::from(sp.sub),
                    key,
                    sp.aux,
                );
                self.n.h[o as usize].flags |= DIRTY;
                self.register(o);
                o
            };
            ids[i as usize] = id;
        }
        gone.extend(self.keymap.values().copied());
        for &o in &gone {
            self.remove(o);
        }
        self.sc_gone = gone;
        // link the big ones in order
        let mut prev = NONE;
        for &i in &bigs {
            let id = ids[i as usize];
            if prev == NONE {
                self.set_first(s, id);
            } else {
                self.n.h[prev as usize].next = id;
            }
            prev = id;
        }
        if prev == NONE {
            self.set_first(s, NONE);
        } else {
            self.n.h[prev as usize].next = NONE;
        }
        self.em.specs = specs;
        self.em.bigs = bigs;
        ids
    }

    /// The step's sweep: every emission evaluated in order, the
    /// interior's values in a buffer dropped after, the big ones' set on
    /// their nodes (waking their readers, field by field). What the
    /// interior read from outside the step is added to `os`, the step's
    /// operands.
    #[allow(clippy::too_many_lines, reason = "one pass over the emissions")]
    fn sweep(&mut self, s: u32, ids: &[u32], os: &mut Vec<Opd>) {
        let specs = std::mem::take(&mut self.em.specs);
        let mut eargs = std::mem::take(&mut self.em.args);
        // (the emissions evaluated as they were emitted have their values
        // here already; the rest are evaluated here, in order)
        let mut buf = std::mem::take(&mut self.em.vals);
        let mut copds = std::mem::take(&mut self.sc_copds);
        let mut interior = 0u32;
        for (i, sp) in specs.iter().enumerate() {
            let id = ids[i];
            let (a0, an) = (sp.args.0 as usize, sp.args.1 as usize);
            let leaf = sp.kind == Kind::Leaf;
            if leaf && sp.done {
                // (evaluated as it was emitted: what it read from outside
                // the step is resolved already)
                os.extend(
                    eargs[a0..a0 + an]
                        .iter()
                        .filter(|e| !matches!(e.a, SArg::Local(..)))
                        .map(|e| e.o),
                );
            }
            #[allow(clippy::needless_range_loop, reason = "the operand is written back")]
            for a in a0..a0 + an {
                if leaf && sp.done {
                    break;
                }
                // (a name the step itself defined before this emission
                // reads that definition inside the step, as `eval_now`
                // would have had the definition been evaluated then)
                if let SArg::Name(m, x) = eargs[a].a {
                    match self.em.own_def_at(m, u64::from(sp.sub)) {
                        Some(SArg::Local(j, sel)) => {
                            eargs[a].a = SArg::Local(j, sel.then(extra(x)));
                        }
                        Some(SArg::Node(src, sel)) => {
                            eargs[a].a = SArg::Node(src, sel.then(extra(x)));
                        }
                        _ => {}
                    }
                }
                match eargs[a].a {
                    SArg::Local(ix, sel) => {
                        // (a member whose value comes from outside the step:
                        // a leaf of the step reads it, so the step does)
                        let x = ids[ix as usize];
                        if leaf
                            && x != NONE
                            && matches!(
                                self.n.h[x as usize].kind,
                                Kind::Unfold
                                    | Kind::Scan
                                    | Kind::Cross
                                    | Kind::ChainRead
                                    | Kind::Family
                            )
                        {
                            os.push(Opd {
                                src: x,
                                sel,
                                name: NONE,
                            });
                        }
                    }
                    SArg::Node(src, sel) => {
                        let o = Opd {
                            src,
                            sel,
                            name: NONE,
                        };
                        eargs[a].o = o;
                        if leaf {
                            os.push(o);
                        }
                    }
                    SArg::Name(m, x) => {
                        let o = with_extra(
                            self.resolve_or_import(
                                m,
                                Pos {
                                    parent: s,
                                    ord: u64::from(sp.sub),
                                },
                            ),
                            x,
                        );
                        eargs[a].o = o;
                        if leaf {
                            os.push(o);
                        }
                    }
                }
            }
            match sp.kind {
                Kind::Const => {
                    if id != NONE {
                        self.set_val(id, buf[i].clone());
                        self.n.h[id as usize].flags &= !DIRTY;
                    }
                }
                Kind::Leaf => {
                    if !sp.done {
                        let v = {
                            let args = Args {
                                g: &self.n,
                                opds: &[],
                                sweep: Some((&eargs[a0..a0 + an], ids, &buf)),
                            };
                            let hook = (P || self.hook.memo_on).then_some(&self.hook);
                            let (v, ev) = eval_op(hook, &mut self.tick, sp.op, &args);
                            if let Some(ev) = ev {
                                self.prof.fold(ev);
                            }
                            v
                        };
                        buf[i] = v;
                    }
                    self.rep.evals += 1;
                    if id == NONE {
                        interior += 1;
                    } else {
                        self.n.h[id as usize].flags &= !DIRTY;
                        self.set_val(id, buf[i].clone());
                    }
                }
                Kind::Unfold | Kind::Scan => {
                    // (its operands are nodes: what it reads, it reads itself)
                    copds.clear();
                    copds.extend((a0..a0 + an).map(|a| match eargs[a].a {
                        SArg::Local(ix, sel) => Opd {
                            src: ids[ix as usize],
                            sel,
                            name: NONE,
                        },
                        _ => eargs[a].o,
                    }));
                    if self.n.opds_of(id) != copds.as_slice() {
                        self.set_opds(id, &copds);
                        self.n.h[id as usize].flags |= DIRTY;
                    }
                    if self.n.h[id as usize].flags & DIRTY != 0 {
                        self.n.h[id as usize].flags &= !DIRTY;
                        if sp.kind == Kind::Scan {
                            self.run_scan(id);
                        } else if let Some(f) = self.run_unfold(id) {
                            self.run_chain(f);
                        }
                    }
                }
                Kind::Cross | Kind::ChainRead | Kind::Family => {
                    if self.n.h[id as usize].flags & DIRTY != 0 {
                        self.n.h[id as usize].flags &= !DIRTY;
                        self.process_kind(id);
                    }
                }
                _ => unreachable!("{:?} emitted", sp.kind),
            }
        }
        if self.cfg.debug {
            let op = self.n.h[s as usize].op;
            // (constants count too: every emission that is not a node)
            let consts = specs
                .iter()
                .zip(ids)
                .filter(|(sp, id)| sp.kind == Kind::Const && **id == NONE)
                .count();
            #[allow(clippy::cast_possible_truncation, reason = "emissions fit u32")]
            let n = interior + consts as u32;
            self.interior
                .entry(L::op_tag(op))
                .or_insert_with(|| (op, Vec::new()))
                .1
                .push(n);
        }
        self.em.specs = specs;
        self.em.args = eargs;
        self.em.vals = buf;
        self.sc_copds = copds;
    }

    fn sarg(&mut self, a: SArg, ids: &[u32], at: Pos) -> Opd {
        match a {
            SArg::Local(ix, sel) => Opd {
                src: ids[ix as usize],
                sel,
                name: NONE,
            },
            SArg::Node(src, sel) => Opd {
                src,
                sel,
                name: NONE,
            },
            SArg::Name(m, x) => with_extra(self.resolve_or_import(m, at), x),
        }
    }

    /// Name `m` at `at`; in a segment, a name defined only where the
    /// segment was entered from becomes an import: a definition before
    /// everything, of an input holding that value.
    fn resolve_or_import(&mut self, m: u32, at: Pos) -> Opd {
        let o = resolve(&self.n, &self.names, &self.groups, m, at);
        if o.src != NONE {
            return o;
        }
        let h = self.names.hashes[m as usize];
        let Some(v) = self.ext.as_ref().and_then(|e| e.get(&h)).cloned() else {
            return o;
        };
        if self.imports.contains_key(&h) {
            return o;
        }
        let i = self.root_node(Kind::Input, L::Op::default(), Class::Pure, 0);
        self.n.val[i as usize] = v;
        self.imports.insert(h, i);
        let ri = u32::try_from(self.names.recs.len()).expect("definitions fit u32");
        self.names.recs.push(DefRec {
            name: m,
            step: ROOT,
            sub: 0,
            src: i,
            sel: Sel::WHOLE,
            group: NOGROUP,
            global: true,
        });
        self.names.defs[m as usize].insert(0, ri);
        self.names.gens[m as usize] += 1;
        resolve(&self.n, &self.names, &self.groups, m, at)
    }
}

impl<L: Lang, const P: bool> Graph<L, P> {
    /// The step's scope events, applied to the group table.
    fn apply_groups(&mut self, s: u32, si: usize) {
        let events = std::mem::take(&mut self.em.events);
        let mut closed = Vec::new();
        for (e, sub) in &events {
            match *e {
                Event::Open(g, p) => {
                    self.groups
                        .entry(g)
                        .or_insert_with(|| Group {
                            parent: p,
                            close: None,
                            closers: Vec::new(),
                            names: Vec::new(),
                        })
                        .parent = p;
                }
                Event::Close(g) => {
                    if g == NOGROUP {
                        self.rep.unbalanced += 1;
                        continue;
                    }
                    let pos = Pos {
                        parent: s,
                        ord: *sub,
                    };
                    self.groups.entry(g).or_insert_with(|| Group {
                        parent: NOGROUP,
                        close: None,
                        closers: Vec::new(),
                        names: Vec::new(),
                    });
                    closed.push((g, pos));
                }
            }
        }
        let _ = si;
        // (each group this step closes now or closed before: its places
        // in this step replaced by the new ones)
        let old_closed = self.closes.remove(&s).unwrap_or_default();
        let mut gs: Vec<u64> = closed.iter().map(|c| c.0).chain(old_closed).collect();
        gs.sort_unstable();
        gs.dedup();
        for g in &gs {
            let now: Vec<Pos> = closed.iter().filter(|c| c.0 == *g).map(|c| c.1).collect();
            self.set_closers(*g, s, &now);
        }
        let closed: Vec<u64> = gs
            .into_iter()
            .filter(|g| closed.iter().any(|c| c.0 == *g))
            .collect();
        if !closed.is_empty() {
            self.closes.insert(s, closed);
        }
        self.em.events = events;
    }

    /// Group `g`'s closes in step `s` set to `now`; where it ends moved,
    /// its names' readers resolved again from the earlier of the two.
    fn set_closers(&mut self, g: u64, s: u32, now: &[Pos]) {
        let Some(gr) = self.groups.get_mut(&g) else {
            return;
        };
        let before = gr.closers.len();
        gr.closers.retain(|c| c.parent != s);
        if now.is_empty() && gr.closers.len() == before {
            return;
        }
        gr.closers.extend_from_slice(now);
        let old = gr.close;
        let n = &self.n;
        let new = gr.closers.iter().copied().min_by(|a, b| n.cmp_pos(*a, *b));
        if new == old {
            return;
        }
        gr.close = new;
        self.groups_gen += 1;
        let from = match (old, new) {
            (Some(a), Some(b)) => {
                if self.n.cmp_pos(a, b) == Ordering::Less {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return,
        };
        self.reresolve_group(g, from);
    }

    fn reresolve_group(&mut self, g: u64, from: Pos) {
        let names = self
            .groups
            .get(&g)
            .map(|x| x.names.clone())
            .unwrap_or_default();
        for m in names {
            self.reresolve(m, from);
        }
    }

    /// The step's definitions, applied to the name index.
    /// The names whose readers must be resolved again (after the
    /// sweep: a new source has its value then).
    fn apply_defs(&mut self, s: u32, si: usize, ids: &[u32]) -> Vec<u32> {
        let defs = std::mem::take(&mut self.em.defs);
        if defs.is_empty() && self.steps[si].dn == 0 {
            self.em.defs = defs;
            return Vec::new();
        }
        let mut new = std::mem::take(&mut self.sc_defs);
        new.clear();
        new.extend(defs.iter().map(|&(m, a, global, sub, grp)| {
            let o = self.sarg(
                a,
                ids,
                Pos {
                    parent: s,
                    ord: sub,
                },
            );
            DefRec {
                name: m,
                step: s,
                sub: u32::try_from(sub).expect("emissions fit u32"),
                src: o.src,
                sel: o.sel,
                group: grp,
                global,
            }
        }));
        self.em.defs = defs;
        let (d0, dn) = (self.steps[si].d0 as usize, self.steps[si].dn as usize);
        let old = &self.names.recs[d0..d0 + dn];
        let same = old.len() == new.len()
            && old.iter().zip(&new).all(|(a, b)| {
                a.name == b.name
                    && a.sub == b.sub
                    && a.src == b.src
                    && a.sel == b.sel
                    && a.group == b.group
                    && a.global == b.global
            });
        if same {
            self.sc_defs = new;
            return Vec::new();
        }
        let old = old.to_vec();
        let mut touched = Vec::new();
        for d in &old {
            self.remove_def(d.name, d.pos());
            touched.push(d.name);
        }
        let r0 = u32::try_from(self.names.recs.len()).expect("definitions fit u32");
        self.names.recs.extend_from_slice(&new);
        for (k, d) in new.iter().enumerate() {
            self.insert_def(d.name, r0 + u32::try_from(k).expect("definitions fit u32"));
            if !d.global
                && d.group != NOGROUP
                && let Some(gr) = self.groups.get_mut(&d.group)
                && !gr.names.contains(&d.name)
            {
                gr.names.push(d.name);
            }
            touched.push(d.name);
        }
        self.steps[si].d0 = r0;
        self.steps[si].dn = u32::try_from(new.len()).expect("definitions fit u32");
        self.sc_defs = new;
        touched.sort_unstable();
        touched.dedup();
        touched
    }

    /// Readers of name `m` after `from` resolved again; those whose value
    /// changed are woken. Only readers before the next definition after
    /// `from` that is alive to the end (global, or in no group) can
    /// change: past it, every reader reaches it or a later one. So the
    /// cost is the run of readers between the two (DESIGN 7.5).
    fn reresolve(&mut self, m: u32, from: Pos) {
        let af = self.anchor_pos(from);
        // (no reader after it: nothing to do, and no search for the bound)
        let rs = &self.names.readers[m as usize];
        if rs
            .last()
            .is_none_or(|l| self.n.cmp_pos(self.n.pos(l.0), af) == Ordering::Less)
        {
            return;
        }
        let rs = std::mem::take(&mut self.names.readers[m as usize]);
        let k0 = rs.partition_point(|e| self.n.cmp_pos(self.n.pos(e.0), af) == Ordering::Less);
        // (the bound: the anchor of the next definition alive to the end)
        let bound = {
            let defs = &self.names.defs[m as usize];
            // (in a later anchor: one in `from`'s own anchor, the changed
            // step's own, does not bound it)
            let k = defs.partition_point(|&d| {
                self.n
                    .cmp_pos(self.anchor_pos(self.names.recs[d as usize].pos()), af)
                    != Ordering::Greater
            });
            defs.iter_from(k)
                .map(|&i| &self.names.recs[i as usize])
                .find(|d| d.global || d.group == NOGROUP)
                .map(|d| self.anchor_pos(d.pos()))
        };
        for &(a, r, era) in rs.iter_from(k0) {
            if let Some(b) = bound
                && self.n.cmp_pos(self.n.pos(a), b) == Ordering::Greater
            {
                break;
            }
            let ru = r as usize;
            if self.n.h[ru].era != era || self.n.h[ru].flags & DEAD != 0 {
                continue;
            }
            let at = self.n.pos(r);
            if self.n.cmp_pos(at, from) != Ordering::Greater {
                continue;
            }
            let a0 = self.n.h[ru].a0 as usize;
            for k in 0..self.n.h[ru].an as usize {
                let o = self.n.opds[a0 + k];
                if o.name != m {
                    continue;
                }
                let nw = with_extra(
                    resolve(&self.n, &self.names, &self.groups, m, at),
                    o.sel.extra().unwrap_or(NOX),
                );
                if nw.src != o.src || nw.sel != o.sel {
                    let ov = self.n.read_ver(&o);
                    self.n.opds[a0 + k] = nw;
                    if nw.src != NONE {
                        self.add_rev(nw.src, r);
                    }
                    if ov != self.n.read_ver(&nw) {
                        self.rep.woken += 1;
                        self.push_dirty(r);
                    }
                }
            }
        }
        // (readers added meanwhile go in their places again)
        let added = std::mem::replace(&mut self.names.readers[m as usize], rs);
        for &e in added.iter() {
            self.insert_reader(m, e);
        }
    }

    /// A reader of name `m`, in its anchor's place.
    fn insert_reader(&mut self, m: u32, e: (u32, u32, u32)) {
        let list = &self.names.readers[m as usize];
        let ap = self.n.pos(e.0);
        let k = match list.last() {
            None => 0,
            Some(l) if l.0 == e.0 || self.n.cmp_pos(self.n.pos(l.0), ap) != Ordering::Greater => {
                list.len()
            }
            _ => list.partition_point(|x| self.n.cmp_pos(self.n.pos(x.0), ap) != Ordering::Greater),
        };
        self.names.readers[m as usize].insert(k, e);
    }

    /// Name `m`'s reader entry for `node` at `era` (anchored at `a`)
    /// removed; false if it is not found where it should be.
    fn remove_reader(&mut self, m: u32, a: u32, node: u32, era: u32) -> bool {
        let list = &self.names.readers[m as usize];
        let ap = self.n.pos(a);
        let k = list.partition_point(|x| self.n.cmp_pos(self.n.pos(x.0), ap) == Ordering::Less);
        let mut found = None;
        for (i, x) in (k..).zip(list.iter_from(k)) {
            if x.0 != a && self.n.cmp_pos(self.n.pos(x.0), ap) == Ordering::Greater {
                break;
            }
            if x.1 == node && x.2 == era {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(i) => {
                self.names.readers[m as usize].remove(i);
                true
            }
            None => false,
        }
    }

    /// A definition of name `m`, in its place (at the end, mostly).
    fn insert_def(&mut self, m: u32, ri: u32) {
        let recs = &self.names.recs;
        let at = recs[ri as usize].pos();
        let list = &self.names.defs[m as usize];
        let k = match list.last() {
            None => 0,
            Some(&l) if self.n.cmp_pos(recs[l as usize].pos(), at) == Ordering::Less => list.len(),
            _ => list
                .partition_point(|&x| self.n.cmp_pos(recs[x as usize].pos(), at) == Ordering::Less),
        };
        self.names.defs[m as usize].insert(k, ri);
        self.names.gens[m as usize] += 1;
    }

    /// Remove name `m`'s definition at `pos`.
    fn remove_def(&mut self, m: u32, pos: Pos) {
        let recs = &self.names.recs;
        let list = &self.names.defs[m as usize];
        let k = list
            .partition_point(|&x| self.n.cmp_pos(recs[x as usize].pos(), pos) == Ordering::Less);
        if list.get(k).is_some_and(|&x| recs[x as usize].pos() == pos) {
            self.names.defs[m as usize].remove(k);
            self.names.gens[m as usize] += 1;
        }
    }

    /// Remove `n` and everything under it.
    fn remove(&mut self, n: u32) {
        let u = n as usize;
        if self.n.h[u].flags & DEAD != 0 {
            return;
        }
        let mut c = self.first(n);
        while c != NONE {
            let nx = self.n.h[c as usize].next;
            self.remove(c);
            c = nx;
        }
        self.drop_readers(n);
        self.n.h[u].flags |= DEAD;
        self.n.h[u].era = self.n.h[u].era.wrapping_add(1);
        self.rep.removed += 1;
        self.to_free.push(n);
        if self.n.h[u].kind == Kind::Step {
            let si = self.n.h[u].aux as usize;
            let su = self.steps[si].unfold;
            let ui = self.n.h[su as usize].aux as usize;
            let (key, at) = (self.steps[si].key, self.steps[si].at);
            if self.unfolds[ui].indexed {
                if self.unfolds[ui].keys.get(&key) == Some(&n) {
                    self.unfolds[ui].keys.remove(&key);
                }
                if self.unfolds[ui].owners.get(&at) == Some(&n) {
                    self.unfolds[ui].owners.remove(&at);
                }
            }
            let (d0, dn) = (self.steps[si].d0 as usize, self.steps[si].dn as usize);
            self.steps[si].dn = 0;
            let defs = self.names.recs[d0..d0 + dn].to_vec();
            let from = self.n.pos(n);
            let mut touched = Vec::new();
            for d in &defs {
                self.remove_def(d.name, d.pos());
                touched.push(d.name);
            }
            touched.sort_unstable();
            touched.dedup();
            for m in touched {
                self.reresolve(m, from);
            }
            for g in self.closes.remove(&n).unwrap_or_default() {
                self.set_closers(g, n, &[]);
            }
        }
        self.class_changed(n);
    }

    // ---- scans ----

    #[allow(clippy::too_many_lines, reason = "one scan, resumed")]
    fn run_scan(&mut self, sc: u32) {
        let ix = self.n.h[sc as usize].aux as usize;
        let uo: Vec<Opd> = self.n.opds_of(sc).to_vec();
        let input = L::as_seq(&self.n.read(&uo[0])).cloned().unwrap_or_default();
        let init = self.n.read(&uo[1]).clone();
        let args_ver = self.args_ver(sc, 2);
        let op = self.n.h[sc as usize].op;
        let info = &mut self.scans[ix];
        let full = !info.done || info.init_ver != init.ver() || info.args_ver != args_ver;
        let (old_input, old_states, old_outs) = (
            std::mem::take(&mut info.input),
            std::mem::take(&mut info.states),
            std::mem::take(&mut info.outs),
        );
        let (p, sfx) = if full {
            (0, 0)
        } else {
            let h = input.diff(&old_input);
            (h.prefix, h.suffix)
        };
        let (n_new, n_old) = (input.len(), old_input.len());
        let mut st = if p == 0 {
            init.clone()
        } else {
            old_states.get(p - 1).expect("a state").1.clone()
        };
        let mut fresh_states: Vec<(ElemId, L::Val)> = Vec::new();
        let mut fresh: Vec<(ElemId, L::Val)> = Vec::new();
        // (`met`: the old index from which the old run's states and outputs
        // hold again)
        let mut met = None;
        let mut stepped = 0;
        let old_at = |k: usize| -> Ver {
            // the old state before old element k
            if k == 0 {
                init.ver()
            } else {
                old_states.get(k - 1).expect("a state").1.ver()
            }
        };
        if !full && p >= n_new - sfx && st.ver() == old_at(p + n_old - n_new) {
            // (only deletions, and the state before them holds after them)
            met = Some(p + n_old - n_new);
        }
        if met.is_none() {
            let args = Args::of(&self.n, &uo[2..]);
            let memo = self.hook.memo_on && (L::memo(op) || self.hook.auto.has(&op));
            let watch = P && self.hook.watch.has(&op);
            let av = if memo || watch {
                args.key(0)
            } else {
                Ver::ABSENT
            };
            for (i, (id, x)) in input.iter_from(p).enumerate().map(|(k, e)| (k + p, e)) {
                let w = if P { self.tick.next() } else { 0 };
                let t = if w > 0 { now() } else { None };
                let key = if watch {
                    Ver::node(L::op_tag(op) ^ SCAN_STATE, &[st.ver(), x.ver(), av])
                } else {
                    Ver::ABSENT
                };
                let (s2, out) = if memo {
                    self.scan_memo(op, &st, x, &args, av)
                } else {
                    L::scan(op, &st, x, &args)
                };
                if P && (w > 0 || watch) {
                    self.prof.fold(Ev {
                        op,
                        w,
                        // (a memo hit's time is not the op's)
                        ns: if memo { None } else { t.map(ns_since) },
                        key,
                        kind: crate::profile::SCAN,
                    });
                }
                st = s2;
                self.rep.scanned += 1;
                stepped += 1;
                fresh.push((id, out));
                fresh_states.push((id, st.clone()));
                if !full && i + 1 >= n_new - sfx && i + 1 + n_old >= n_new {
                    let ok = i + 1 + n_old - n_new;
                    if ok >= 1 && ok <= n_old && old_at(ok) == st.ver() {
                        met = Some(ok);
                        break;
                    }
                }
            }
        }
        let del = match met {
            Some(ok) => ok - p.min(ok),
            None => n_old - p.min(n_old),
        };
        let outs = old_outs.splice(p, del, fresh);
        let states = old_states.splice(p, del, fresh_states);
        let last = states
            .get(states.len().wrapping_sub(1))
            .map_or_else(|| init.clone(), |e| e.1.clone());
        let v = {
            let args = Args::of(&self.n, &uo[2..]);
            L::scan_result(op, &last, &outs, &args)
        };
        let info = &mut self.scans[ix];
        info.input = input;
        info.states = states;
        info.outs = outs;
        info.init_ver = init.ver();
        info.args_ver = args_ver;
        info.done = true;
        self.scan_runs.push((op, stepped));
        self.rep.evals += 1;
        self.set_val(sc, v);
    }

    // ---- the run ----

    /// Family `f`'s entries as published in this run, in position order
    /// (each keyed by its slot).
    fn family_value(&self, f: u32) -> L::Val {
        let mut ns: Vec<u32> = self
            .fams
            .get(&f)
            .map(|v| v.iter().copied().filter(|&x| !self.n.is_dead(x)).collect())
            .unwrap_or_default();
        ns.sort_by(|&a, &b| self.n.cmp_node(a, b));
        ns.dedup();
        let items = ns
            .into_iter()
            .map(|x| {
                let s = self.n.class(x).slot().map_or(0, |s| s.0);
                (ElemId(s), self.n.val[x as usize].clone())
            })
            .collect();
        L::chain_val(Seq::from_vec(items))
    }

    /// The value slot `s` takes in the next run.
    fn published(&self, s: u64) -> L::Val {
        let mut best: Option<u32> = None;
        for &p in self.pubs.get(&s).into_iter().flatten() {
            if self.n.is_dead(p) {
                continue;
            }
            if best.is_none_or(|b| self.n.cmp_node(p, b) == Ordering::Greater) {
                best = Some(p);
            }
        }
        best.map(|b| self.n.val[b as usize].clone())
            .unwrap_or_default()
    }

    /// Evaluate what is new or changed, to quiescence, then iterate the
    /// cross-run slots to their fixed point.
    #[allow(
        clippy::too_many_lines,
        reason = "the run loop, then the cross-run loop"
    )]
    pub fn run(&mut self) -> Report {
        self.rep = Report::default();
        self.epoch += 1;
        if P {
            self.prof.runs += 1;
        }
        self.round_size = 0;
        self.round_made = 0;
        self.scan_runs.clear();
        let mut hist: Map<u64, Vec<Ver>> = Map::default();
        let par = self.cfg.workers > 1 && !self.cfg.segment;
        loop {
            loop {
                if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    // (the rest stays queued for the next run)
                    self.cancel
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    self.outcomes.clear();
                    self.rep.cancelled = true;
                    self.tidy();
                    return self.rep.clone();
                }
                if par && self.outcomes.is_empty() && self.step_ns >= self.cfg.round_min_ns {
                    if self.round_cool == 0 && self.heap.len() >= 2 * self.cfg.workers {
                        self.round();
                    } else {
                        self.round_cool = self.round_cool.saturating_sub(1);
                    }
                }
                let Some(n) = self.pop() else { break };
                self.process(n);
            }
            if self.cfg.segment {
                // (a segment's slots are its graph's: it reads, never iterates)
                self.dirty_slots.clear();
                self.dirty_fams.clear();
                break;
            }
            let mut changed = Vec::new();
            for sl in std::mem::take(&mut self.dirty_slots) {
                let v = self.published(sl);
                let cur = self
                    .pred
                    .get(&sl)
                    .map_or_else(|| L::Val::default().ver(), Value::ver);
                if cur != v.ver() {
                    changed.push((sl, v));
                }
            }
            let mut fchanged = Vec::new();
            for f in std::mem::take(&mut self.dirty_fams) {
                let v = self.family_value(f);
                let cur = self
                    .fpred
                    .get(&f)
                    .map_or_else(|| L::chain_val(Seq::new()).ver(), Value::ver);
                if cur != v.ver() {
                    fchanged.push((f, v));
                }
            }
            if changed.is_empty() && fchanged.is_empty() {
                break;
            }
            if self.rep.iterations >= self.cfg.max_iters {
                for (sl, v) in changed {
                    let mut h = hist.remove(&sl).unwrap_or_default();
                    h.push(v.ver());
                    self.rep.oscillating.push((Slot(sl), h));
                }
                for (f, v) in fchanged {
                    let mut h = hist.remove(&(u64::from(f) | 1 << 63)).unwrap_or_default();
                    h.push(v.ver());
                    self.rep.oscillating.push((Slot(u64::from(f) | 1 << 63), h));
                }
                break;
            }
            self.rep.iterations += 1;
            for (sl, v) in changed {
                hist.entry(sl).or_default().push(v.ver());
                std::sync::Arc::make_mut(&mut self.pred).insert(sl, v);
                for c in self.crosses.get(&sl).cloned().unwrap_or_default() {
                    if !self.n.is_dead(c) {
                        self.push_dirty(c);
                    }
                }
            }
            for (f, v) in fchanged {
                hist.entry(u64::from(f) | 1 << 63)
                    .or_default()
                    .push(v.ver());
                std::sync::Arc::make_mut(&mut self.fpred).insert(f, v);
                for c in self.fam_readers.get(&f).cloned().unwrap_or_default() {
                    if !self.n.is_dead(c) {
                        self.push_dirty(c);
                    }
                }
            }
        }
        for u in std::mem::take(&mut self.hunks) {
            let ui = self.n.h[u as usize].aux as usize;
            if !self.n.is_dead(u) {
                self.unfolds[ui].hunk_end = None;
            }
        }
        self.outcomes.clear();
        if self.cfg.check {
            self.check();
        } else if !self.cfg.segment {
            self.seal_due_now();
        }
        self.tidy();
        if P {
            self.prof.flush();
        }
        self.rep.clone()
    }

    /// Registries pruned of the dead; removed slots freed for reuse.
    fn tidy(&mut self) {
        let dead = |n: &Nodes<L>, x: u32| n.h[x as usize].flags & DEAD != 0;
        for v in self.chains.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.chain_readers.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.pubs.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.crosses.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.fams.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.fam_readers.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for m in std::mem::take(&mut self.names.prune) {
            self.names.pruned[m as usize] = false;
            let g = &self.n;
            self.names.readers[m as usize]
                .retain(|&(_, r, e)| g.h[r as usize].era == e && g.h[r as usize].flags & DEAD == 0);
        }
        // (a freed node keeps its DEAD flag until reused)
        let tf = std::mem::take(&mut self.to_free);
        for n in tf {
            self.set_first(n, NONE);
            self.n.val[n as usize] = L::Val::default();
            self.free.push(n);
        }
        if self.cfg.seal > 0 && !self.cfg.segment {
            self.compact_if_sparse();
        }
    }

    /// Every live leaf and scan evaluated again: a value that differs is
    /// an impure op or a missed wake.
    fn check(&mut self) {
        let mut seen = vec![false; self.n.h.len()];
        let mut stack = vec![ROOT];
        while let Some(x) = stack.pop() {
            seen[x as usize] = true;
            let mut c = self.first(x);
            while c != NONE {
                assert!(
                    !self.n.is_dead(c),
                    "check: dead node %{c} linked under %{x}"
                );
                assert_eq!(
                    self.n.h[c as usize].parent, x,
                    "check: %{c} linked under %{x}"
                );
                stack.push(c);
                c = self.n.h[c as usize].next;
            }
        }
        for (i, s) in seen.iter().enumerate() {
            assert!(
                *s || self.n.h[i].flags & DEAD != 0,
                "check: live node %{i} ({:?} {:?}, parent %{}) is not in the tree",
                self.n.h[i].kind,
                self.n.h[i].op,
                self.n.h[i].parent
            );
        }
        for n in 1..self.n.h.len() {
            let n32 = u32::try_from(n).expect("fits");
            // (a step's members are checked with their step, below)
            if self.n.is_dead(n32)
                || self.n.h[n].kind != Kind::Leaf
                || self.n.h[self.n.h[n].parent as usize].kind == Kind::Step
            {
                continue;
            }
            let v = {
                let args = Args::of(&self.n, self.n.opds_of(n32));
                L::eval(self.n.h[n].op, &args)
            };
            assert!(
                v.ver() == self.n.val[n].ver(),
                "check: node %{n} ({:?}) is {:?} but evaluates to {:?}",
                self.n.h[n].op,
                self.n.val[n],
                v
            );
            for o in self.n.opds_of(n32) {
                if o.name != NONE {
                    let at = self.n.pos(n32);
                    let want = with_extra(
                        resolve(&self.n, &self.names, &self.groups, o.name, at),
                        o.sel.extra().unwrap_or(NOX),
                    );
                    assert!(
                        want.src == o.src && want.sel == o.sel,
                        "check: node %{n} reads name {} at %{} but it resolves to %{}",
                        o.name,
                        o.src,
                        want.src
                    );
                }
            }
        }
        self.check_index();
        self.check_steps();
    }

    /// The group table and the name index as the steps make them: a
    /// close is a live step's that lists it; each name's definitions are
    /// in position order, each one its step's, and every live step's are
    /// there.
    fn check_index(&self) {
        // the group table: a close is a live step's, which lists it
        for (g, gr) in &self.groups {
            for c in &gr.closers {
                assert!(
                    !self.n.is_dead(c.parent)
                        && self.closes.get(&c.parent).is_some_and(|l| l.contains(g)),
                    "check: group {g:x} closed at {c:?} by a step that does not close it"
                );
            }
            let first = gr
                .closers
                .iter()
                .copied()
                .min_by(|a, b| self.n.cmp_pos(*a, *b));
            assert_eq!(
                gr.close, first,
                "check: group {g:x} ends elsewhere than its first close"
            );
        }
        // the name index: each name's definitions in position order, and
        // every live step's definitions in it
        for (m, list) in self.names.defs.iter().enumerate() {
            for &k in list {
                let d = &self.names.recs[k as usize];
                let ok = d.step == ROOT
                    || (!self.n.is_dead(d.step)
                        && self.n.h[d.step as usize].kind == Kind::Step
                        && {
                            let si = &self.steps[self.n.h[d.step as usize].aux as usize];
                            (si.d0..si.d0 + si.dn).contains(&k)
                        });
                assert!(
                    ok,
                    "check: name {m}'s definition (record {k}) at step %{} is not that step's",
                    d.step
                );
            }
            let v: Vec<u32> = list.iter().copied().collect();
            for w in v.windows(2) {
                let (a, b) = (
                    self.names.recs[w[0] as usize].pos(),
                    self.names.recs[w[1] as usize].pos(),
                );
                assert!(
                    self.n.cmp_pos(a, b) == Ordering::Less,
                    "check: name {m}'s definitions out of order: {a:?} then {b:?}"
                );
            }
        }
        for n in 1..self.n.h.len() {
            let n32 = u32::try_from(n).expect("fits");
            if self.n.is_dead(n32) || self.n.h[n].kind != Kind::Step {
                continue;
            }
            let si = &self.steps[self.n.h[n].aux as usize];
            for k in si.d0..si.d0 + si.dn {
                let d = &self.names.recs[k as usize];
                assert!(
                    self.names.defs[d.name as usize].iter().any(|&x| x == k),
                    "check: step %{n}'s definition of name {} (record {k}) is not in the index",
                    d.name
                );
            }
        }
    }

    // ---- reading the results ----

    /// A node's value.
    #[must_use]
    pub fn value(&self, n: NodeId) -> &L::Val {
        &self.n.val[n.0 as usize]
    }

    /// Chain `c`'s payloads in order.
    #[must_use]
    pub fn chain(&self, c: Chain) -> Vec<L::Val> {
        self.chain_nodes(c.0)
            .into_iter()
            .map(|x| self.n.val[x as usize].clone())
            .collect()
    }

    /// The value slot `s` was predicted to have in this run.
    #[must_use]
    pub fn slot(&self, s: Slot) -> L::Val {
        self.pred.get(&s.0).cloned().unwrap_or_default()
    }

    /// Slot `s`'s publishers and prediction, for debugging.
    #[doc(hidden)]
    #[must_use]
    pub fn debug_slot(&self, s: Slot) -> String {
        let mut out = format!(
            "pred {:?}; published {:?}\n",
            self.pred.get(&s.0),
            self.published(s.0)
        );
        for &p in self.pubs.get(&s.0).into_iter().flatten() {
            let _ = writeln!(
                out,
                "  pub %{p} dead {} val {:?} pos {:?} parent %{}",
                self.n.is_dead(p),
                self.n.val[p as usize],
                self.n.pos(p),
                self.n.h[p as usize].parent
            );
        }
        out
    }

    /// Predict slot `s` (a cold start from a persisted run).
    pub fn predict(&mut self, s: Slot, v: L::Val) {
        std::sync::Arc::make_mut(&mut self.pred).insert(s.0, v);
    }

    /// Step interiors by op (`Config::debug`): for each op, the steps
    /// run, and their transient emissions' count at most and at the 99th
    /// percentile. An op whose interior grows with its input breaks the
    /// rule on `Lang::step`: what can grow is a nested unfold, scan or
    /// sequence.
    #[must_use]
    pub fn interior_sizes(&self) -> Vec<(L::Op, usize, u32, u32)> {
        let mut out: Vec<(L::Op, usize, u32, u32)> = self
            .interior
            .values()
            .map(|(op, v)| {
                let mut v = v.clone();
                v.sort_unstable();
                let n = v.len();
                (*op, n, v[n - 1], v[(n * 99 / 100).min(n - 1)])
            })
            .collect();
        out.sort_by_key(|x| L::op_tag(x.0));
        out
    }

    /// The number of live nodes.
    #[must_use]
    pub fn live(&self) -> usize {
        (1..self.n.h.len())
            .filter(|&i| self.n.h[i].flags & DEAD == 0)
            .count()
    }

    /// The name spelled `s`, if any node has used it.
    #[must_use]
    pub fn name_id(&self, s: &[u8]) -> Option<NameId> {
        self.names
            .by_hash
            .get(&name_hash(s))
            .filter(|&&i| *self.names.spell[i as usize] == *s)
            .map(|&i| NameId(i))
    }

    /// The spelling of name `n`.
    #[must_use]
    pub fn spelling(&self, n: NameId) -> &[u8] {
        &self.names.spell[n.0 as usize]
    }

    /// Name `n`'s value at the end of the program.
    #[must_use]
    pub fn name_value(&self, n: NameId) -> Option<L::Val> {
        let end = Pos {
            parent: ROOT,
            ord: u64::MAX,
        };
        let o = resolve(&self.n, &self.names, &self.groups, n.0, end);
        (o.src != NONE).then(|| self.n.read(&o).clone())
    }

    pub(crate) fn names_spell(&self, m: u32) -> Vec<u8> {
        self.names.spell[m as usize].to_vec()
    }

    /// Whether step `s` is a sealed region.
    pub(crate) fn sealed(&self, s: u32) -> bool {
        self.n.h[s as usize].flags & SEALED != 0
    }

    /// A step's definitions: (source, selector, name, global).
    pub(crate) fn step_defs(&self, s: u32) -> Vec<(u32, Sel, u32, bool)> {
        let si = &self.steps[self.n.h[s as usize].aux as usize];
        self.names.recs[si.d0 as usize..(si.d0 + si.dn) as usize]
            .iter()
            .map(|d| (d.src, d.sel, d.name, d.global))
            .collect()
    }

    /// The elements the scans of `op` stepped in the last run.
    #[must_use]
    pub fn scanned(&self, op: L::Op) -> u64 {
        self.scan_runs
            .iter()
            .filter(|r| r.0 == op)
            .map(|r| r.1)
            .sum()
    }

    /// A scan's states after each element (versions), for tests.
    #[must_use]
    pub fn state_vers(&self, n: NodeId) -> Vec<Ver> {
        self.scans[self.n.h[n.0 as usize].aux as usize]
            .states
            .iter()
            .map(|e| e.1.ver())
            .collect()
    }

    /// Name `n`'s definitions, for debugging: (defining node, position,
    /// group, its close, global).
    #[must_use]
    pub fn debug_defs(&self, n: NameId) -> String {
        struct DebugDef {
            src: u32,
            pos: Pos,
            group: u64,
            global: bool,
        }
        let mut out = String::new();
        for &i in self.names.defs[n.0 as usize].iter() {
            let d = &self.names.recs[i as usize];
            let d = (d.src, d.pos(), d.group, d.global);
            let d = DebugDef {
                src: d.0,
                pos: d.1,
                group: d.2,
                global: d.3,
            };
            let close = self.groups.get(&d.group).and_then(|g| g.close);
            let _ = write!(
                out,
                "[close before def: {:?}; group parent {:x?}] ",
                close.map(|c| self.n.cmp_pos(c, d.pos)),
                self.groups.get(&d.group).map(|g| g.parent)
            );
            let _ = writeln!(
                out,
                "src %{} at ({}, {}) group {:x} close {:?} global {} dead {}",
                d.src,
                d.pos.parent,
                d.pos.ord,
                d.group,
                close.map(|c| (c.parent, c.ord)),
                d.global,
                d.src != NONE && self.n.is_dead(d.src)
            );
        }
        out
    }

    /// Bytes the graph holds in its own tables (values' heap contents,
    /// the client's, not counted): (live nodes, bytes).
    #[must_use]
    pub fn mem(&self) -> (usize, usize) {
        use std::mem::size_of;
        let n = &self.n;
        let cols = n.h.len() * (size_of::<Hdr<L::Op>>() + size_of::<L::Val>());
        let arenas = n.opds.len() * size_of::<Opd>() + n.revs.len() * size_of::<Rev>();
        let steps =
            self.steps.len() * size_of::<StepInfo>() + self.names.recs.len() * size_of::<DefRec>();
        let names: usize = self
            .names
            .defs
            .iter()
            .map(|d| d.len() * size_of::<u32>())
            .sum::<usize>()
            + self
                .names
                .readers
                .iter()
                .map(|r| r.len() * 12)
                .sum::<usize>();
        (self.live(), cols + arenas + steps + names)
    }

    /// The root region's nodes, in order.
    #[must_use]
    pub fn roots(&self) -> Vec<NodeId> {
        self.children(ROOT).into_iter().map(NodeId).collect()
    }
}

/// Grafting segments in order: the last step so far, each segment's
/// boundary (the step before it, if it stopped), and how the last ended.
struct Grafting<L: Lang> {
    tail: u32,
    bounds: Vec<(Option<Parked>, u32)>,
    prev: Option<Parked>,
    done: Option<L::Val>,
    ended: bool,
}

impl<L: Lang, const P: bool> Graph<L, P> {
    /// Speculative entry (DESIGN 7.4): the chain of `u` stopped at its
    /// first entry; run a segment from each entry on the workers, graft
    /// them in order, and check each arrival like any successor.
    #[allow(
        clippy::too_many_lines,
        reason = "the segments, their run on the workers, and the grafting, in order"
    )]
    fn speculate(&mut self, u: u32) {
        let ui = self.n.h[u as usize].aux as usize;
        let parked = self.unfolds[ui].parked.take().expect("parked");
        let mut ents = self.entries.remove(&u).unwrap_or_default();
        ents.sort_by_key(|e| e.at);
        ents.retain(|e| e.at >= parked.idx && e.guess.is_some());
        ents.dedup_by_key(|e| e.at);
        if ents.is_empty() {
            if let Some(x) = self.successor(
                parked.s,
                u,
                ui,
                parked.key,
                parked.cursor,
                parked.idx,
                parked.grp,
                parked.ver,
            ) {
                self.push_dirty(x);
            }
            return;
        }
        // every name as it is where the segments start
        let at = Pos {
            parent: u,
            ord: self.n.h[parked.s as usize].ord + 1,
        };
        let mut ext = Map::default();
        let mut spell = Vec::new();
        for m in 0..self.names.spell.len() {
            let o = resolve(&self.n, &self.names, &self.groups, m as u32, at);
            if o.src != NONE {
                ext.insert(self.names.hashes[m], self.n.read(&o).clone());
                spell.push((self.names.hashes[m], self.names.spell[m].clone()));
            }
        }
        let ext = std::sync::Arc::new(ext);
        let spell = std::sync::Arc::new(spell);
        let uo: Vec<Opd> = self.n.opds_of(u).to_vec();
        let input = self.n.read(&uo[0]).clone();
        let args: Vec<L::Val> = uo[2..].iter().map(|o| self.n.read(o).clone()).collect();
        let op = self.n.h[u as usize].op;
        let segs: Vec<(usize, Option<usize>, u64, L::Val)> = ents
            .iter()
            .enumerate()
            .map(|(k, e)| {
                (
                    e.at,
                    ents.get(k + 1).map(|n| n.at),
                    e.key,
                    e.guess.clone().expect("guessed"),
                )
            })
            .collect();
        let pred = self.pred.clone();
        let fpred = self.fpred.clone();
        let (keep, debug) = (self.cfg.keep_interior, self.cfg.debug);
        let hook = self.hook.clone();
        let job = |seg: &(usize, Option<usize>, u64, L::Val)| {
            let mut p: Graph<L, P> = Graph::new();
            p.cfg.segment = true;
            p.cfg.keep_interior = keep;
            p.cfg.debug = debug;
            p.hook = hook.clone();
            p.ext = Some(ext.clone());
            p.ext_spell = Some(spell.clone());
            p.pred = pred.clone();
            p.fpred = fpred.clone();
            let i = p.input(input.clone());
            let init = p.input(seg.3.clone());
            let a: Vec<NodeId> = args.iter().map(|v| p.input(v.clone())).collect();
            let pu = p.unfold(op, i, init, &a).0;
            let pui = p.n.h[pu as usize].aux as usize;
            p.unfolds[pui].start = Some((seg.2, seg.0));
            p.unfolds[pui].stop = seg.1;
            p.run();
            p.pack(pu)
        };
        let mut st = Grafting {
            tail: parked.s,
            bounds: Vec::new(),
            prev: Some(parked),
            done: None,
            ended: false,
        };
        // (segments are grafted in order as they come, while the workers go
        // on with the next ones)
        #[cfg(not(target_arch = "wasm32"))]
        if self.cfg.workers > 1 && segs.len() > 1 {
            use std::sync::atomic::{AtomicUsize, Ordering as AO};
            let next = AtomicUsize::new(0);
            let workers = self.cfg.workers.min(segs.len());
            std::thread::scope(|sc| {
                let (tx, rx) = std::sync::mpsc::channel();
                for _ in 0..workers {
                    let tx = tx.clone();
                    let (next, segs, job) = (&next, &segs, &job);
                    sc.spawn(move || {
                        loop {
                            let k = next.fetch_add(1, AO::Relaxed);
                            if k >= segs.len() {
                                break;
                            }
                            if tx.send((k, job(&segs[k]))).is_err() {
                                break;
                            }
                        }
                    });
                }
                drop(tx);
                let mut waiting: std::collections::BTreeMap<usize, Pack<L>> =
                    std::collections::BTreeMap::new();
                let mut want = 0;
                for (k, out) in rx {
                    waiting.insert(k, out);
                    while let Some(o) = waiting.remove(&want) {
                        if want == 0 {
                            // (room for the segments to come, as big as the
                            // first: the tables then grow without copying)
                            let n = segs.len();
                            self.n.h.reserve(n * o.hdrs.len());
                            self.n.val.reserve(n * o.vals.len());
                            self.n.opds.reserve(n * o.opds.len());
                            self.n.revs.reserve(n * o.opds.len());
                            self.steps.reserve(n * o.steps.len());
                            self.names.recs.reserve(n * o.defs.len());
                        }
                        self.graft_next(u, &mut st, o);
                        want += 1;
                    }
                }
            });
            self.arrive(u, ui, st);
            return;
        }
        for seg in &segs {
            let o = job(seg);
            self.graft_next(u, &mut st, o);
        }
        self.arrive(u, ui, st);
    }

    fn graft_next(&mut self, u: u32, st: &mut Grafting<L>, out: Pack<L>) {
        if st.ended {
            return;
        }
        let mut out = out;
        if P {
            self.prof.merge(std::mem::take(&mut out.prof));
        }
        let Some((first, last, end)) = self.graft(u, st.tail, out) else {
            return;
        };
        st.bounds.push((st.prev, first));
        st.tail = last;
        st.prev = end;
        if end.is_none() {
            st.done = Some(self.n.val[last as usize].clone());
            st.ended = true;
        }
    }

    /// Each grafted segment's arrival checked in order, like any
    /// successor.
    fn arrive(&mut self, u: u32, ui: usize, st: Grafting<L>) {
        for (p, first) in st.bounds {
            match p {
                // (a step an earlier arrival's resync removed has no successor)
                Some(p) if self.n.is_dead(p.s) => {}
                Some(p) => {
                    if let Some(x) =
                        self.successor(p.s, u, ui, p.key, p.cursor, p.idx, p.grp, p.ver)
                    {
                        self.push_dirty(x);
                    }
                }
                None => {
                    // (the part before ended: nothing after it is the chain's)
                    let mut c = first;
                    while c != NONE {
                        let nx = self.n.h[c as usize].next;
                        self.remove(c);
                        c = nx;
                    }
                    return;
                }
            }
        }
        if let Some(p) = st.prev.filter(|p| !self.n.is_dead(p.s)) {
            // (the last segment stopped: go on from there)
            if let Some(x) = self.successor(p.s, u, ui, p.key, p.cursor, p.idx, p.grp, p.ver) {
                self.push_dirty(x);
            }
        } else if let Some(v) = st.done {
            self.set_val(u, v);
        }
    }

    /// The name with this spelling, interned.
    fn intern(&mut self, sp: &[u8]) -> u32 {
        let h = name_hash(sp);
        if let Some(&i) = self.names.by_hash.get(&h) {
            assert!(
                *self.names.spell[i as usize] == *sp,
                "two names with one hash"
            );
            return i;
        }
        let id = u32::try_from(self.names.spell.len()).expect("names fit u32");
        self.names.by_hash.insert(h, id);
        self.names.hashes.push(h);
        self.names.spell.push(sp.into());
        self.names.defs.push(Runs::default());
        self.names.readers.push(Runs::default());
        self.names.gens.push(0);
        id
    }
}

/// A segment made ready to graft, on the worker that ran it (DESIGN
/// 7.15): its nodes under its unfold numbered from 0 in their order,
/// every reference inside it relative, values moved out; what crosses
/// its edge listed apart. Grafting it is then an append with the ids
/// shifted.
struct Pack<L: Lang> {
    /// The segment's profile (DESIGN 7.21; empty unless `P`).
    prof: Prof<L::Op>,
    hdrs: Vec<Hdr<L::Op>>,
    vals: Vec<L::Val>,
    /// Operands: `src` relative with [`REL`] set, or [`NONE`]; `name`
    /// the segment's.
    opds: Vec<Opd>,
    /// Operands that are the unfold's operand `k` here.
    outer: Vec<(u32, u16)>,
    /// Operands read from before the segment by name: (operand, its
    /// node, the version it read there).
    named: Vec<(u32, u32, Ver)>,
    steps: Vec<StepInfo>,
    unfolds: Vec<UnfoldInfo<L::Val>>,
    scans: Vec<ScanInfo<L::Val>>,
    defs: Vec<DefRec>,
    closes: Vec<(u32, Vec<u64>)>,
    groups: Vec<(u64, Group)>,
    /// Nodes the registries list (cross reads, chain and family reads,
    /// classes but pure).
    regs: Vec<u32>,
    fams: Vec<(u32, u32)>,
    /// Name readers (the segment's name, the node), by name then node,
    /// and definitions (the name, the index in `defs`, the step), by name
    /// then index: each name's are appended in one go when they come
    /// after its list's last.
    rnames: Vec<(u32, u32)>,
    dorder: Vec<(u32, u32, u32)>,
    /// The top steps, in chain order.
    tops: Vec<u32>,
    spell: Vec<Box<[u8]>>,
    end: Option<Parked>,
    depth: u16,
}

/// A relative id in a [`Pack`] (where an absolute one may stand too).
const REL: u32 = 0x8000_0000;
/// The segment's unfold, in a [`Pack`].
const UP: u32 = NONE - 1;

impl<L: Lang, const P: bool> Graph<L, P> {
    /// This segment graph, made ready to graft (on its worker).
    #[allow(clippy::too_many_lines, reason = "one pass per table, in order")]
    #[allow(clippy::cast_possible_truncation, reason = "ids and arenas fit u32")]
    fn pack(mut self, pu: u32) -> Pack<L> {
        let p = &mut self;
        let np = p.n.h.len();
        let dpu = p.n.h[pu as usize].depth;
        let mut map = vec![NONE; np];
        let mut k = 0u32;
        for (x, h) in p.n.h.iter().enumerate().skip(1) {
            if h.flags & DEAD == 0 && h.depth > dpu {
                map[x] = k;
                k += 1;
            }
        }
        debug_assert!(k < REL, "a segment of fewer than 2^31 nodes");
        let rel = |x: u32| -> u32 {
            if x == NONE {
                NONE
            } else if x == pu {
                UP
            } else {
                map[x as usize]
            }
        };
        let inside = |x: u32| x != NONE && x != pu && map[x as usize] != NONE;
        let encg = |g: u64| -> u64 {
            let hi = (g >> 32) as u32;
            if g == NOGROUP || !inside(hi) {
                g
            } else {
                (u64::from(map[hi as usize] | REL) << 32) | (g & 0xffff_ffff)
            }
        };
        let puo = p.n.opds_of(pu).to_vec();
        let mut pval = std::mem::take(&mut p.n.val);
        let count = k as usize;
        let mut dpos: Vec<Pos> = Vec::new();
        let mut pk = Pack {
            hdrs: Vec::with_capacity(count),
            vals: Vec::with_capacity(count),
            opds: Vec::new(),
            outer: Vec::new(),
            named: Vec::new(),
            steps: Vec::new(),
            unfolds: Vec::new(),
            scans: Vec::new(),
            defs: Vec::new(),
            closes: Vec::new(),
            groups: Vec::new(),
            regs: Vec::new(),
            fams: Vec::new(),
            rnames: Vec::new(),
            dorder: Vec::new(),
            tops: Vec::new(),
            spell: Vec::new(),
            end: None,
            depth: dpu,
            prof: std::mem::take(&mut p.prof),
        };
        for x in 1..np {
            let r = map[x];
            if r == NONE {
                continue;
            }
            let mut h = p.n.h[x];
            h.parent = rel(h.parent);
            h.next = rel(h.next);
            h.flags &= !(DIRTY | QUEUED);
            h.rd = NONE;
            let a0 = pk.opds.len() as u32;
            let (pa0, pan) = (p.n.h[x].a0 as usize, p.n.h[x].an as usize);
            for o in &p.n.opds[pa0..pa0 + pan] {
                let i = pk.opds.len() as u32;
                if o.name != NONE {
                    pk.rnames.push((o.name, r));
                }
                if inside(o.src) {
                    pk.opds.push(Opd {
                        src: map[o.src as usize] | REL,
                        sel: o.sel,
                        name: o.name,
                    });
                } else if let Some(kk) = puo.iter().position(|u| u.src == o.src && o.src != NONE)
                    && o.name == NONE
                {
                    pk.opds.push(Opd {
                        src: NONE,
                        sel: o.sel,
                        name: NONE,
                    });
                    pk.outer.push((i, kk as u16));
                } else if o.name == NONE {
                    debug_assert!(
                        o.src == NONE,
                        "a segment reads only itself, its inputs and names"
                    );
                    pk.opds.push(*o);
                } else {
                    // (from before the segment: what it read there, a root
                    // of the segment's graph, whose value stays in `pval`)
                    let was = if o.src == NONE {
                        Ver::ABSENT
                    } else {
                        o.sel.ver(&pval[o.src as usize])
                    };
                    pk.opds.push(Opd {
                        src: NONE,
                        sel: o.sel,
                        name: o.name,
                    });
                    pk.named.push((i, r, was));
                }
            }
            h.a0 = a0;
            match h.kind {
                Kind::Step => {
                    let si = &p.steps[h.aux as usize];
                    let d0 = pk.defs.len() as u32;
                    for d in &p.names.recs[si.d0 as usize..(si.d0 + si.dn) as usize] {
                        dpos.push(d.pos());
                        pk.dorder.push((d.name, pk.defs.len() as u32, r));
                        pk.defs.push(DefRec {
                            name: d.name,
                            step: r,
                            sub: d.sub,
                            src: if inside(d.src) {
                                map[d.src as usize] | REL
                            } else {
                                NONE
                            },
                            sel: d.sel,
                            group: encg(d.group),
                            global: d.global,
                        });
                    }
                    pk.steps.push(StepInfo {
                        unfold: rel(si.unfold),
                        first: rel(si.first),
                        key: si.key,
                        at: si.at,
                        took: si.took,
                        grp_in: encg(si.grp_in),
                        in_ver: si.in_ver,
                        d0,
                        dn: si.dn,
                        ran: 0,
                        work: si.work,
                        folded: 0,
                    });
                    if let Some(cl) = p.closes.get(&(x as u32)) {
                        pk.closes.push((r, cl.iter().map(|&g| encg(g)).collect()));
                    }
                    h.aux = (pk.steps.len() - 1) as u64;
                }
                Kind::Unfold => {
                    let pi = &mut p.unfolds[h.aux as usize];
                    pk.unfolds.push(UnfoldInfo {
                        first: rel(pi.first),
                        indexed: false,
                        keys: Map::default(),
                        owners: Map::default(),
                        input: pi.input.take(),
                        args_ver: pi.args_ver,
                        hunk_end: None,
                        grp0: encg(pi.grp0),
                        stop: None,
                        parked: None,
                        start: None,
                    });
                    h.aux = (pk.unfolds.len() - 1) as u64;
                }
                Kind::Scan => {
                    let pi = &mut p.scans[h.aux as usize];
                    pk.scans.push(ScanInfo {
                        input: std::mem::take(&mut pi.input),
                        states: std::mem::take(&mut pi.states),
                        outs: std::mem::take(&mut pi.outs),
                        init_ver: pi.init_ver,
                        args_ver: pi.args_ver,
                        done: pi.done,
                    });
                    h.aux = (pk.scans.len() - 1) as u64;
                }
                _ => {}
            }
            if h.class == 4
                && let Some(&f) = p.n.fams.get(&(x as u32))
            {
                pk.fams.push((r, f));
            }
            if h.class != 0 || matches!(h.kind, Kind::Cross | Kind::ChainRead | Kind::Family) {
                pk.regs.push(r);
            }
            pk.hdrs.push(h);
            pk.vals.push(std::mem::take(&mut pval[x]));
        }
        let mut c = p.first(pu);
        while c != NONE {
            pk.tops.push(map[c as usize]);
            c = p.n.h[c as usize].next;
        }
        for (&g, gr) in &p.groups {
            if !inside((g >> 32) as u32) {
                continue;
            }
            pk.groups.push((
                encg(g),
                Group {
                    parent: encg(gr.parent),
                    close: gr.close.map(|c| Pos {
                        parent: rel(c.parent),
                        ord: c.ord,
                    }),
                    closers: gr
                        .closers
                        .iter()
                        .map(|c| Pos {
                            parent: rel(c.parent),
                            ord: c.ord,
                        })
                        .collect(),
                    names: gr.names.clone(),
                },
            ));
        }
        pk.rnames.sort_unstable();
        // (by name, then by position: a step's own definitions come before
        // its nested unfolds' steps' in the numbering, but those of a call
        // made before them precede them in position)
        pk.dorder.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| p.n.cmp_pos(dpos[a.1 as usize], dpos[b.1 as usize]))
        });
        let pui = p.n.h[pu as usize].aux as usize;
        pk.end = p.unfolds[pui].parked.map(|e| Parked {
            s: map[e.s as usize],
            grp: encg(e.grp),
            ..e
        });
        pk.spell = std::mem::take(&mut p.names.spell);
        pk
    }

    /// Graft a packed segment's steps (and everything under them) after
    /// `tail`, the last step of `u`: the first and last steps grafted, and
    /// where the segment stopped (`None`: it ended).
    #[allow(clippy::too_many_lines, reason = "one pass per table, in order")]
    #[allow(clippy::cast_possible_truncation, reason = "ids and arenas fit u32")]
    fn graft(&mut self, u: u32, tail: u32, pk: Pack<L>) -> Option<(u32, u32, Option<Parked>)> {
        if pk.tops.is_empty() {
            return None;
        }
        let base = self.n.h.len() as u32;
        let ob = self.n.opds.len() as u32;
        let (sb, ub, cb) = (self.steps.len(), self.unfolds.len(), self.scans.len());
        let db = self.names.recs.len() as u32;
        let du = self.n.h[u as usize].depth;
        let count = pk.hdrs.len();
        self.rep.created += count as u64;
        let names: Vec<u32> = pk.spell.iter().map(|sp| self.intern(sp)).collect();
        let dec = |x: u32| -> u32 {
            if x == NONE {
                NONE
            } else if x == UP {
                u
            } else {
                x + base
            }
        };
        let decs = |x: u32| -> u32 {
            if x != NONE && x & REL != 0 {
                (x & !REL) + base
            } else {
                x
            }
        };
        let decg = |g: u64| -> u64 {
            let hi = (g >> 32) as u32;
            if g == NOGROUP || hi & REL == 0 {
                g
            } else {
                (u64::from((hi & !REL) + base) << 32) | (g & 0xffff_ffff)
            }
        };
        // headers and values, appended
        let pd = pk.depth;
        self.n.h.extend(pk.hdrs.iter().map(|&h| {
            let mut h = h;
            h.parent = dec(h.parent);
            h.next = dec(h.next);
            h.a0 += ob;
            h.depth = h.depth - pd + du;
            h.aux += match h.kind {
                Kind::Step => sb as u64,
                Kind::Unfold => ub as u64,
                Kind::Scan => cb as u64,
                _ => 0,
            };
            h
        }));
        self.n.val.extend(pk.vals);
        for (r, f) in pk.fams {
            self.n.fams.insert(r + base, f);
        }
        // the kinds' tables
        let epoch = self.epoch;
        self.steps.extend(pk.steps.into_iter().map(|si| StepInfo {
            ran: epoch,
            unfold: dec(si.unfold),
            first: dec(si.first),
            grp_in: decg(si.grp_in),
            d0: si.d0 + db,
            ..si
        }));
        self.unfolds
            .extend(pk.unfolds.into_iter().map(|ui| UnfoldInfo {
                first: dec(ui.first),
                grp0: decg(ui.grp0),
                ..ui
            }));
        self.scans.extend(pk.scans);
        // the top steps' labels after `tail`, in chain order
        let first = pk.tops[0] + base;
        let mut last = tail;
        self.n.h[tail as usize].next = first;
        for &r in &pk.tops {
            let s = r + base;
            let ord = self.step_ord(u, last, NONE);
            self.n.h[s as usize].ord = ord;
            last = s;
        }
        // groups (definitions' lives depend on them)
        for (g, gr) in pk.groups {
            let ng = Group {
                parent: decg(gr.parent),
                close: gr.close.map(|c| Pos {
                    parent: dec(c.parent),
                    ord: c.ord,
                }),
                closers: gr
                    .closers
                    .iter()
                    .map(|c| Pos {
                        parent: dec(c.parent),
                        ord: c.ord,
                    })
                    .collect(),
                names: gr.names.iter().map(|&m| names[m as usize]).collect(),
            };
            self.groups.insert(decg(g), ng);
            self.groups_gen += 1;
        }
        for (r, cl) in pk.closes {
            self.closes
                .insert(r + base, cl.into_iter().map(decg).collect());
        }
        // operands: inside shifted, the unfold's operands, names from
        // before the segment resolved here and checked against what the
        // segment read
        let uo: Vec<Opd> = self.n.opds_of(u).to_vec();
        self.n.opds.extend(pk.opds.iter().map(|o| Opd {
            src: decs(o.src),
            sel: o.sel,
            name: if o.name == NONE {
                NONE
            } else {
                names[o.name as usize]
            },
        }));
        for (i, k) in pk.outer {
            let o = &mut self.n.opds[(ob + i) as usize];
            let x = uo[k as usize];
            *o = Opd {
                src: x.src,
                sel: if o.sel.is_whole() { x.sel } else { o.sel },
                name: NONE,
            };
        }
        // (the first step reads the step before it here)
        let fa0 = self.n.h[first as usize].a0 as usize;
        self.n.opds[fa0] = Opd {
            src: tail,
            sel: Sel::WHOLE,
            name: NONE,
        };
        let mut dirty: Vec<u32> = Vec::new();
        // (every such read resolves as at the segment's start: the
        // segment's own definitions are not in the index yet, and nothing
        // of the main graph lies between; so once per name)
        let mut seen: Map<(u32, u32), Opd> = Map::default();
        for (i, r, was) in pk.named {
            let m = r + base;
            let name = self.n.opds[(ob + i) as usize].name;
            let x = self.n.opds[(ob + i) as usize].sel.extra().unwrap_or(NOX);
            let o = if let Some(&o) = seen.get(&(name, x)) {
                o
            } else {
                let at = self.n.pos(m);
                let o = with_extra(resolve(&self.n, &self.names, &self.groups, name, at), x);
                seen.insert((name, x), o);
                o
            };
            if self.n.read_ver(&o) != was && dirty.last() != Some(&m) {
                dirty.push(m);
            }
            self.n.opds[(ob + i) as usize] = o;
        }
        // reverse edges, readers and definitions, node by node
        for m in base..base + count as u32 {
            let mu = m as usize;
            let (a0, an, era) = (self.n.h[mu].a0, self.n.h[mu].an, self.n.h[mu].era);
            for k in a0..a0 + u32::from(an) {
                let o = self.n.opds[k as usize];
                if o.src != NONE {
                    let r = self.n.revs.len() as u32;
                    let s = o.src as usize;
                    self.n.revs.push(Rev {
                        node: m,
                        era,
                        next: self.n.h[s].rd,
                    });
                    self.n.h[s].rd = r;
                }
            }
        }
        // readers, a name at a time: appended when they come after its
        // list's last (a graft at the end, the usual case)
        let mut i = 0;
        while i < pk.rnames.len() {
            let pm = pk.rnames[i].0;
            let mut j = i;
            while j < pk.rnames.len() && pk.rnames[j].0 == pm {
                j += 1;
            }
            let m = names[pm as usize];
            let es: Vec<(u32, u32, u32)> = pk.rnames[i..j]
                .iter()
                .map(|&(_, r)| {
                    let x = r + base;
                    (self.anchor(x), x, self.n.h[x as usize].era)
                })
                .collect();
            let list = &self.names.readers[m as usize];
            let fast = list.last().is_none_or(|l| {
                self.n.cmp_pos(self.n.pos(l.0), self.n.pos(es[0].0)) != Ordering::Greater
            });
            if fast {
                self.names.readers[m as usize].extend(es);
            } else {
                for e in es {
                    self.insert_reader(m, e);
                }
            }
            i = j;
        }
        // definitions: the steps' records, then the name index a name at
        // a time
        self.names.recs.extend(pk.defs.iter().map(|d| DefRec {
            name: names[d.name as usize],
            step: d.step + base,
            src: decs(d.src),
            group: decg(d.group),
            ..*d
        }));
        let mut i = 0;
        while i < pk.dorder.len() {
            let pm = pk.dorder[i].0;
            let mut j = i;
            while j < pk.dorder.len() && pk.dorder[j].0 == pm {
                j += 1;
            }
            let m = names[pm as usize];
            let first = db + pk.dorder[i].1;
            let recs = &self.names.recs;
            let list = &self.names.defs[m as usize];
            let fast = list.last().is_none_or(|&l| {
                self.n
                    .cmp_pos(recs[l as usize].pos(), recs[first as usize].pos())
                    == Ordering::Less
            });
            if fast {
                self.names.defs[m as usize].extend(pk.dorder[i..j].iter().map(|x| db + x.1));
            } else {
                for k in i..j {
                    self.insert_def(m, db + pk.dorder[k].1);
                }
            }
            i = j;
        }
        // registries
        let mut chains: Vec<u32> = Vec::new();
        for r in pk.regs {
            let m = r + base;
            let mu = m as usize;
            match self.n.h[mu].kind {
                Kind::Cross => self.crosses.entry(self.n.h[mu].aux).or_default().push(m),
                Kind::ChainRead => {
                    // (a read in a segment saw only the segment's payloads:
                    // read again here)
                    self.chain_readers
                        .entry(self.n.h[mu].aux as u32)
                        .or_default()
                        .push(m);
                    dirty.push(m);
                }
                Kind::Family => self
                    .fam_readers
                    .entry(self.n.h[mu].aux as u32)
                    .or_default()
                    .push(m),
                _ => {}
            }
            if self.n.h[mu].class != 0 {
                if let Class::Effect(c) = self.n.class(m) {
                    chains.push(c.0);
                }
                self.register_class(m);
            }
        }
        chains.sort_unstable();
        chains.dedup();
        for c in chains {
            self.wake_chain(c);
        }
        for d in dirty {
            self.rep.woken += 1;
            self.push_dirty(d);
        }
        let end = pk.end.map(|e| Parked {
            s: e.s + base,
            grp: decg(e.grp),
            ..e
        });
        Some((first, last, end))
    }
}
