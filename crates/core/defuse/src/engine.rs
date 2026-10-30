//! The engine as the client holds it: units, nodes, the environment,
//! evaluation, edits, reports.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::api::{
    Cause, EditSource, EnvProvider, EvalError, Limits, Monoid, NodeId, NodeInfo, NodeState, OpId, Operation, PathId,
    ProjId, Projection, Report, SeqMonoid, Settled, Step, UnitId,
};
use crate::commit::{self, CommitStats, Predicted, own_write, place_after, settle_predictions, unfile};
use crate::index::{FieldIndex, RangeIndex};
use crate::lock::{Lock, Mx, ns};
use crate::node::{Node, State, Wake};
use crate::order::{GAP, Order, Pos, ROOM_TRIES, SegId, UNIT_SPAN};
use crate::read::{self, answer, apply_path, reaching_at};
use crate::sched::{Pool, Worker, enqueue, mark_dirty};
use crate::segment::Segment;
use crate::table::Table;
use crate::values::{Collected, Field, FxHashMap, FxHashSet, ValueId, Values};

/// A unit's range of the line.
pub(crate) struct Unit {
    pub(crate) start: Pos,
    pub(crate) end: Pos,
    /// The label of the last node pushed, for the next push.
    pub(crate) last: AtomicU64,
}

/// A reader of the environment's value of a field (`Ctx::env`).
#[derive(Clone, Debug)]
pub(crate) struct EnvReader {
    pub(crate) node: NodeId,
    pub(crate) serial: u32,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
}

/// The environment: the values reaching the start of the line, from the
/// client's settings and, on a miss, its provider.
/// Its readers are filed by field with the range readers
/// (`index::RangeIndex`).
pub(crate) struct Env {
    pub(crate) values: Lock<FxHashMap<Field, Option<ValueId>>>,
    pub(crate) provider: Lock<Option<Box<EnvProvider>>>,
}

pub(crate) struct Paths {
    pub(crate) by_steps: Lock<FxHashMap<Box<[Step]>, PathId>>,
    pub(crate) steps: Table<Box<[Step]>>,
    /// A path one step further, by (path, step): `Ctx::at` and `get`
    /// extend a view's path without a list of its steps.
    pub(crate) ext: Lock<FxHashMap<(PathId, Step), PathId>>,
}

/// Path ids below this are projections' (`PathId::of_proj`).
const PROJ_PATHS: usize = 1024;
/// Nodes `push_many` files in each segment it makes: room for as many again.
const PUSH_RUN: usize = crate::order::SEG_CAP / 2;

/// `Ctx::apply`'s memo: (op, args) -> output.
pub(crate) type Memo = FxHashMap<(OpId, Box<[ValueId]>), ValueId>;

pub(crate) struct Monoids {
    pub(crate) by_field: FxHashMap<Field, Arc<dyn Monoid>>,
    pub(crate) by_prefix: Vec<(String, Arc<dyn Monoid>)>,
    pub(crate) default: Arc<dyn Monoid>,
}

/// Counts an evaluation keeps outside the workers.
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) nodes_made: AtomicU64,
    pub(crate) edits: AtomicU64,
}

/// Dirty nodes made outside an evaluation, and why: the next one's seed;
/// and what the walks outside did, for its report.
#[derive(Default)]
pub(crate) struct Pending {
    pub(crate) nodes: Vec<NodeId>,
    pub(crate) causes: Vec<(NodeId, Cause)>,
    pub(crate) refuted_by_field: FxHashMap<Field, u64>,
    pub(crate) stats: CommitStats,
}

/// Every method takes `&self`: a mutation is a placement or a mark under a
/// segment's lock, and an edit during an evaluation is just more dirty
/// nodes. Only `evaluate` is exclusive by convention of the client.
pub struct Engine {
    pub(crate) values: Values,
    pub(crate) ops: Table<Arc<dyn Operation>>,
    pub(crate) units: Table<Unit>,
    pub(crate) nodes: Table<Arc<Node>>,
    pub(crate) segs: Table<Arc<Segment>>,
    pub(crate) order: Order,
    /// Segments defining each field; segments contributing to each field;
    /// segments with readers of each field.
    pub(crate) defs: FieldIndex,
    pub(crate) contribs: FieldIndex,
    pub(crate) readers: FieldIndex,
    /// The range readers of each field, by interval.
    pub(crate) ranges: RangeIndex,
    pub(crate) monoids: Lock<Monoids>,
    pub(crate) env: Env,
    pub(crate) paths: Paths,
    pub(crate) projections: Table<Projection>,
    /// Relations each operation has made: (op, field) -> c (`commit::learn_shape`).
    pub(crate) shapes: Lock<FxHashMap<(OpId, Field), i64>>,
    pub(crate) shapes_n: AtomicUsize,
    /// `Ctx::apply`'s memo.
    pub(crate) memo: Lock<Memo>,
    pub(crate) marked: Lock<FxHashSet<NodeId>>,
    pub(crate) marked_n: AtomicUsize,
    pub(crate) settled: Mx<Vec<Settled>>,
    pub(crate) limits: Lock<Limits>,
    pub(crate) version: AtomicU64,
    pub(crate) stats: Stats,
    pub(crate) pending: Mx<Pending>,
    /// Evaluations so far: the epoch a node's run count belongs to.
    pub(crate) epoch: AtomicU64,
    pub(crate) t0: Instant,
    /// The last node to commit in the last evaluation.
    pub(crate) last_commit: AtomicU64,
    /// The last evaluation's report, for the chain queries.
    pub(crate) last_report: Mx<Option<Report>>,
    /// A change count per field bucket ([`Engine::field_epoch`]): a run's
    /// read whose bucket did not move by its commit needs no second look.
    pub(crate) field_epochs: Box<[AtomicU64]>,
    /// Reader records filed by runs no longer their node's last (a run
    /// committed after them, the node gone), since the last sweep; and
    /// the records the last sweep left ([`Engine::sweep_readers`]).
    pub(crate) stale_reads: AtomicU64,
    /// Predictions a commit replaced with another value or depth: the
    /// node, the field, the predicted value and depth. Judged when the
    /// evaluation ends against what the node finally defines there
    /// (`Engine::judge_predictions`): a run that read stale inputs and is
    /// run again is a rerun, not a wrong prediction.
    pub(crate) pred_pending: Mx<Vec<(NodeId, Field, ValueId, u16)>>,
    pub(crate) swept_left: AtomicU64,
    /// Counts the changes of any node's brackets: a chain of open scopes
    /// cached at an older count is stale (`read::chain_start`), and a run
    /// that began before one checks every read at its commit.
    pub(crate) scope_gen: AtomicU64,
    /// Nodes with brackets filed: none, and no scope is walked.
    pub(crate) bracket_nodes: AtomicUsize,
    /// The field a read of the scope depth is recorded under
    /// (`Ctx::depth`): its readers are re-asked with the rest after a
    /// changed bracket.
    pub(crate) depth_field: Field,
    /// Some `At::Final` reader was ever filed.
    pub(crate) has_finals: std::sync::atomic::AtomicBool,
    /// Wakes held to the next quiet point (`sched::hold_wake`): a reader
    /// re-asked by a change a settling operation's node made.
    pub(crate) held: crate::lock::Mx<Vec<(NodeId, crate::node::Wake)>>,
    /// The fields a settling operation's node defined or was predicted to
    /// define (`Operation::settles_at_quiet`), as a bit set by hash
    /// (`Engine::settle_marked`): a reader of one past the settling
    /// layer's frontier waits for it (`Ctx::set_frontier`).
    pub(crate) settle_marks: Box<[AtomicU64]>,
    /// The same fields exactly, for a field whose bit is set: a bit is a
    /// hash's, which another field's may share (amsldoc -j8: `\@@par`,
    /// `\toks@` and `\check@nocorr@`, which no routine defines, read by
    /// every paragraph, held past the frontier as if one did, 620 of the
    /// 1000 reads held).
    pub(crate) settle_exact: Lock<crate::FxHashSet<Field>>,
    /// The settling layer's frontier (`Ctx::set_frontier`): the raw id
    /// of the node past which it may yet place nodes, plus one; 0, none
    /// (no reader waits).
    pub(crate) frontier: AtomicU64,
    /// The node that set the frontier, its raw id plus one: a stop of its
    /// that moves on moves the frontier with it (`commit::reask_range`).
    pub(crate) frontier_by: AtomicU64,
    /// (`PHITEX_TEX_TIME`) Reads by readers outside the settling layer of
    /// a field it defines held past its frontier (`Ctx::set_frontier`).
    pub(crate) frontier_held: AtomicU64,
    /// Counts the rebalances (`order::Order::rebalance`): a range reader
    /// whose span was found before one is filed again with its span now.
    pub(crate) layout_gen: AtomicU64,
    /// A panic outside a run cut a commit or a walk short
    /// (`sched::work_caught`): the next evaluation checks the line first
    /// (`Engine::repair_line`).
    pub(crate) repair: std::sync::atomic::AtomicBool,
    /// The evaluations running, with [`COLLECTING`] while a collection
    /// runs: neither begins during the other (a task reads a list without
    /// a pin, `values::TaskGuard`).
    pub(crate) busy: AtomicU64,
    /// Counts the builds the loop-back passes are counted in
    /// (`sched::allow_final_rerun`): each evaluation, and each edit taken
    /// during one (`sched::absorb`), a document of its own.
    pub(crate) final_gen: AtomicU64,
}

/// The buckets of [`Engine::field_epochs`].
const EPOCH_BUCKETS: usize = 4096;

/// The words of `Engine::settle_marks` (65 536 bits).
const SETTLE_MARK_WORDS: usize = 1024;

/// A field's bit in `Engine::settle_marks`.
fn settle_bit(f: Field) -> (usize, u64) {
    let h = f.0.raw().wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 48;
    #[allow(clippy::cast_possible_truncation, reason = "16 bits")]
    ((h >> 6) as usize % SETTLE_MARK_WORDS, 1 << (h & 63))
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// [`Engine::busy`]'s bit for a collection running.
pub(crate) const COLLECTING: u64 = 1 << 63;

/// Takes `.1` back from [`Engine::busy`] as it drops (an unwind as well).
struct Unbusy<'a>(&'a AtomicU64, u64);

impl Drop for Unbusy<'_> {
    fn drop(&mut self) {
        if self.1 == COLLECTING {
            self.0.fetch_and(!COLLECTING, Ordering::SeqCst);
        } else {
            self.0.fetch_sub(self.1, Ordering::SeqCst);
        }
    }
}

thread_local! {
    /// This thread runs evaluations with the engine's stack (its
    /// evaluation thread): an evaluation begun here needs no thread of
    /// its own.
    static ENGINE_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Engine {
    /// Marks `f` as defined by a settling operation's node
    /// (`Engine::settle_marks`).
    pub(crate) fn mark_settled(&self, f: Field) {
        let (w, b) = settle_bit(f);
        if self.settle_marks[w].load(Ordering::Relaxed) & b != 0 && self.settle_exact.read().contains(&f) {
            return;
        }
        // (the field first, then its bit: a reader that sees the bit finds
        // the field)
        self.settle_exact.write().insert(f);
        self.settle_marks[w].fetch_or(b, Ordering::Release);
    }

    /// (`PHITEX_TEX_TIME`) `Engine::frontier_held`.
    pub fn frontier_held(&self) -> u64 {
        self.frontier_held.load(Ordering::Relaxed)
    }

    /// Whether `f` may be defined by a settling operation's node: its bit,
    /// then, set, the field itself (another field's bit may be the same).
    pub(crate) fn settle_marked(&self, f: Field) -> bool {
        let (w, b) = settle_bit(f);
        self.settle_marks[w].load(Ordering::Acquire) & b != 0 && self.settle_exact.read().contains(&f)
    }

    /// A bracket changed: a new scope count.
    /// A new scope epoch: the epoch it begins.
    pub(crate) fn scope_changed(&self) -> u64 {
        self.scope_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn new() -> Engine {
        let paths = Paths {
            by_steps: Lock::new(FxHashMap::default()),
            steps: Table::new(),
            ext: Lock::new(FxHashMap::default()),
        };
        {
            let mut by = paths.by_steps.write();
            let empty: Box<[Step]> = Box::new([]);
            paths.steps.push(empty.clone());
            by.insert(empty, PathId::WHOLE);
            for i in 1..PROJ_PATHS {
                let steps: Box<[Step]> = Box::new([Step::Proj(ProjId(u16::try_from(i).unwrap_or(u16::MAX)))]);
                paths.steps.push(steps.clone());
                by.insert(steps, PathId(u32::try_from(i).unwrap_or(u32::MAX)));
            }
        }
        let projections = Table::new();
        projections.push(Arc::new(|x: ValueId, _: &Values| x) as Projection);
        projections.push(read::builtin_projection(ProjId::IS_EMPTY).expect("the built-in projection"));
        let values = Values::new();
        let depth_field = values.field("\u{0}depth");
        Engine {
            field_epochs: (0..EPOCH_BUCKETS).map(|_| AtomicU64::new(0)).collect(),
            stale_reads: AtomicU64::new(0),
            pred_pending: Mx::new(Vec::new()),
            swept_left: AtomicU64::new(0),
            scope_gen: AtomicU64::new(1),
            bracket_nodes: AtomicUsize::new(0),
            depth_field,
            has_finals: std::sync::atomic::AtomicBool::new(false),
            held: crate::lock::Mx::new(Vec::new()),
            settle_marks: (0..SETTLE_MARK_WORDS).map(|_| AtomicU64::new(0)).collect(),
            settle_exact: Lock::new(crate::FxHashSet::default()),
            frontier: AtomicU64::new(0),
            frontier_by: AtomicU64::new(0),
            frontier_held: AtomicU64::new(0),
            layout_gen: AtomicU64::new(0),
            repair: std::sync::atomic::AtomicBool::new(false),
            busy: AtomicU64::new(0),
            final_gen: AtomicU64::new(0),
            values,
            ops: Table::new(),
            units: Table::new(),
            nodes: Table::new(),
            segs: Table::new(),
            order: Order::new(),
            defs: FieldIndex::new(),
            contribs: FieldIndex::new(),
            readers: FieldIndex::new(),
            ranges: RangeIndex::new(),
            monoids: Lock::new(Monoids {
                by_field: FxHashMap::default(),
                by_prefix: Vec::new(),
                default: Arc::new(SeqMonoid),
            }),
            env: Env {
                values: Lock::new(FxHashMap::default()),
                provider: Lock::new(None),
            },
            paths,
            projections,
            shapes: Lock::new(FxHashMap::default()),
            shapes_n: AtomicUsize::new(0),
            memo: Lock::new(FxHashMap::default()),
            marked: Lock::new(FxHashSet::default()),
            marked_n: AtomicUsize::new(0),
            settled: Mx::new(Vec::new()),
            limits: Lock::new(Limits::default()),
            version: AtomicU64::new(0),
            stats: Stats::default(),
            pending: Mx::new(Pending::default()),
            epoch: AtomicU64::new(0),
            t0: Instant::now(),
            last_commit: AtomicU64::new(u64::MAX),
            last_report: Mx::new(None),
        }
    }

    pub fn values(&self) -> &Values {
        &self.values
    }

    pub(crate) fn node(&self, id: NodeId) -> &Arc<Node> {
        self.nodes
            .get(usize::try_from(id.raw()).unwrap_or(usize::MAX))
            .expect("a node of this engine")
    }

    /// A node, None while it is not on the line yet: a commit's record
    /// names the placements it puts after other nodes (`place_after`)
    /// before they are placed.
    pub(crate) fn placed_node(&self, id: NodeId) -> Option<&Arc<Node>> {
        self.nodes.get(usize::try_from(id.raw()).unwrap_or(usize::MAX))
    }

    /// Segment `id`; met by a read on one layout (`read::on_one_layout`).
    pub(crate) fn seg(&self, id: SegId) -> &Arc<Segment> {
        let s = self.seg_unmet(id);
        read::met(s);
        s
    }

    /// Segment `id`, not met by the read on one layout asking.
    pub(crate) fn seg_unmet(&self, id: SegId) -> &Arc<Segment> {
        loop {
            if let Some(s) = self.segs.get(id.0 as usize) {
                return s;
            }
            std::hint::spin_loop();
        }
    }

    /// The segment whose range holds `p` now, or the last before `p`
    /// where `p` is in no segment's (a gap, past the last unit's end);
    /// None before the first.
    pub(crate) fn seg_for_pos(&self, p: Pos) -> Option<&Arc<Segment>> {
        loop {
            let sid = self.order.at_or_after(p)?;
            let s = self.seg(sid);
            {
                // (a split moves the order under the segment's lock: the
                // same segment again is no split under way)
                let inner = s.inner.read();
                if p < inner.hi || self.order.at_or_after(p) == Some(sid) {
                    return Some(s);
                }
            }
            std::hint::spin_loop();
        }
    }

    pub(crate) fn op(&self, id: OpId) -> &Arc<dyn Operation> {
        self.ops.get(id.0 as usize).expect("an operation of this engine")
    }

    pub(crate) fn unit(&self, id: UnitId) -> &Unit {
        self.units.get(id.0 as usize).expect("a unit of this engine")
    }

    pub(crate) fn now_ns(&self) -> u64 {
        ns(self.t0)
    }

    pub(crate) fn monoid(&self, f: Field) -> Arc<dyn Monoid> {
        let m = self.monoids.read();
        if let Some(x) = m.by_field.get(&f) {
            return x.clone();
        }
        if !m.by_prefix.is_empty()
            && let Some(x) = self
                .values
                .with_str(f.0, |s| {
                    m.by_prefix
                        .iter()
                        .find(|(p, _)| s.starts_with(p.as_str()))
                        .map(|x| x.1.clone())
                })
                .flatten()
        {
            return x;
        }
        m.default.clone()
    }

    pub(crate) fn path_steps(&self, p: PathId) -> Option<&[Step]> {
        self.paths.steps.get(p.0 as usize).map(|b| &**b)
    }

    pub(crate) fn path_extend(&self, p: PathId, s: Step) -> PathId {
        if let Some(&q) = self.paths.ext.read().get(&(p, s)) {
            return q;
        }
        let mut steps: Vec<Step> = self.path_steps(p).map(|x| x.to_vec()).unwrap_or_default();
        steps.push(s);
        let q = self.path(&steps);
        self.paths.ext.write().insert((p, s), q);
        q
    }

    pub(crate) fn projection(&self, p: ProjId) -> Option<&Projection> {
        self.projections.get(usize::from(p.0))
    }

    pub(crate) fn pend(&self, n: NodeId) {
        self.pending.lock().nodes.push(n);
    }

    pub(crate) fn log_outside(&self, n: NodeId, cause: Cause, field: Option<Field>) {
        let mut p = self.pending.lock();
        p.causes.push((n, cause));
        if let Some(f) = field {
            *p.refuted_by_field.entry(f).or_default() += 1;
        }
    }

    /// What a walk did: into the worker's counts, or the pending ones.
    pub(crate) fn note_stats(&self, w: Option<&Worker>, st: &CommitStats) {
        match w {
            Some(w) => w.note(st),
            None => {
                let mut p = self.pending.lock();
                p.stats.add(st);
            }
        }
    }

    /// The pending nodes, taken; their causes and counts go to the worker.
    pub(crate) fn take_pending(&self, w: &Worker) -> Vec<NodeId> {
        let mut p = self.pending.lock();
        let nodes = std::mem::take(&mut p.nodes);
        let causes = std::mem::take(&mut p.causes);
        let rbf = std::mem::take(&mut p.refuted_by_field);
        let st = std::mem::take(&mut p.stats);
        drop(p);
        w.note(&st);
        let mut log = w.log.lock();
        log.causes.extend(causes);
        for (f, k) in rbf {
            *log.refuted_by_field.entry(f).or_default() += k;
        }
        nodes
    }

    /// A marked node's run committed clean: its output, at the version
    /// the run began at (an edit taken while it ran may move what it read).
    pub(crate) fn note_settled(&self, node: &Node, output: ValueId, version: u64) {
        if self.marked_n.load(Ordering::Relaxed) == 0 || !self.marked.read().contains(&node.id) {
            return;
        }
        self.settled.lock().push(Settled {
            node: node.id,
            version,
            output,
        });
    }

    /// The marked nodes settled since the last take, at a quiet point:
    /// each once, with its output now and the version now, which nothing
    /// queued or running can move (a report made while it ran may name an
    /// output an edit taken since made stale).
    pub(crate) fn take_settled_quiet(&self) -> Vec<Settled> {
        let items = self.take_settled();
        let version = self.version.load(Ordering::Acquire);
        let mut seen = FxHashSet::default();
        let mut out: Vec<Settled> = items
            .iter()
            .rev()
            .filter(|s| seen.insert(s.node))
            .filter_map(|s| {
                self.output(s.node).map(|output| Settled {
                    node: s.node,
                    version,
                    output,
                })
            })
            .collect();
        out.reverse();
        out
    }

    pub fn register(&self, op: Arc<dyn Operation>) -> OpId {
        OpId(u32::try_from(self.ops.push(op)).expect("operation ids"))
    }

    pub fn register_projection(&self, p: Projection) -> ProjId {
        let i = self.projections.push(p);
        assert!(i < PROJ_PATHS, "too many projections");
        ProjId(u16::try_from(i).expect("projection ids"))
    }

    /// A path interned; the same steps give the same id.
    pub fn path(&self, steps: &[Step]) -> PathId {
        if let Some(&p) = self.paths.by_steps.read().get(steps) {
            return p;
        }
        let mut by = self.paths.by_steps.write();
        if let Some(&p) = by.get(steps) {
            return p;
        }
        let boxed: Box<[Step]> = steps.into();
        let i = self.paths.steps.push(boxed.clone());
        let p = PathId(u32::try_from(i).expect("path ids"));
        by.insert(boxed, p);
        p
    }

    fn make_unit(&self, start: Pos, end: Pos) -> UnitId {
        let i = self.units.reserve();
        let id = UnitId(u32::try_from(i).expect("unit ids"));
        self.units.set(
            i,
            Unit {
                start,
                end,
                last: AtomicU64::new(start.0),
            },
        );
        let sid = SegId(u32::try_from(self.segs.reserve()).expect("segment ids"));
        self.segs
            .set(sid.0 as usize, Arc::new(Segment::new(sid, id, start, end)));
        self.order.insert(start, sid);
        id
    }

    /// A unit after the last one made.
    pub fn new_unit(&self) -> UnitId {
        let k = self.units.len() as u64;
        let start = Pos((k + 1) * UNIT_SPAN);
        self.make_unit(start, Pos(start.0 + UNIT_SPAN))
    }

    /// A unit in the gap after `node` (and its placements).
    pub fn new_unit_after(&self, node: NodeId) -> UnitId {
        let end = commit::subtree_end(self, node);
        let endn = self.node(end);
        loop {
            let Some((seg, mut inner)) = own_write(self, endn) else {
                return self.new_unit();
            };
            let p = endn.pos_hint();
            let next = inner
                .nodes
                .range((std::ops::Bound::Excluded(p), std::ops::Bound::Unbounded))
                .next()
                .map_or(inner.hi, |(q, _)| *q);
            if next.0 - p.0 < 8 {
                if !seg.has_room(&inner, 4) {
                    drop(inner);
                    self.order.rebalance(self, seg.id, 4);
                    continue;
                }
                seg.relabel(&mut inner, self);
                continue;
            }
            let quarter = (next.0 - p.0) / 4;
            let (a, b) = (Pos(p.0 + quarter), Pos(p.0 + 3 * quarter));
            let upper = SegId(u32::try_from(self.segs.reserve()).expect("segment ids"));
            seg.split_at(&mut inner, self, b, upper);
            inner.hi = a;
            drop(inner);
            let i = self.units.reserve();
            let id = UnitId(u32::try_from(i).expect("unit ids"));
            self.units.set(
                i,
                Unit {
                    start: a,
                    end: b,
                    last: AtomicU64::new(a.0),
                },
            );
            let sid = SegId(u32::try_from(self.segs.reserve()).expect("segment ids"));
            self.segs.set(sid.0 as usize, Arc::new(Segment::new(sid, id, a, b)));
            self.order.insert(a, sid);
            return id;
        }
    }

    /// A node at the end of `unit`, dirty.
    pub fn push(&self, unit: UnitId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let id = NodeId::from_raw(self.nodes.reserve() as u64);
        let (node, pred) = commit::built(self, unit, None, op, args, id);
        let node = self.push_node(node, pred);
        self.stats.nodes_made.fetch_add(1, Ordering::Relaxed);
        enqueue(
            self,
            None,
            &node,
            Wake {
                by: None,
                cause: Cause::Created { by: None },
                at_ns: self.now_ns(),
            },
        );
        let mut st = CommitStats::default();
        settle_predictions(self, None, &node, &mut st);
        self.note_stats(None, &st);
        id
    }

    /// A built node at the end of its unit, in its last segment.
    pub(crate) fn push_node(&self, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
        let unit = node.unit;
        let u = self.unit(unit);
        loop {
            let seg = self.seg(self.order.seg_at(Pos(u.end.0 - 1)));
            let mut inner = seg.inner.write();
            if seg.unit != unit || Pos(u.end.0 - 1) >= inner.hi {
                continue;
            }
            if Segment::needs_split(&inner) {
                let sid = SegId(u32::try_from(self.segs.reserve()).expect("segment ids"));
                seg.split(&mut inner, self, sid);
                continue;
            }
            let p = match inner.nodes.keys().next_back().copied() {
                None => u.start,
                Some(l) => {
                    if l.0 + GAP < inner.hi.0 {
                        Pos(l.0 + GAP)
                    } else {
                        assert!(inner.hi.0 - l.0 >= 2, "unit {} is out of labels", unit.0);
                        Pos(l.0 + (inner.hi.0 - l.0) / 2)
                    }
                }
            };
            node.seg.store(seg.id.0, Ordering::Release);
            node.pos.store(p.0, Ordering::Release);
            self.nodes
                .set(usize::try_from(node.id.raw()).unwrap_or(usize::MAX), node.clone());
            inner.nodes.insert(p, node.id);
            inner.holes += 1;
            inner.by_op.entry(node.op()).or_default().push(node.id);
            commit::file_predicted(self, seg, &mut inner, &node, pred);
            u.last.store(p.0, Ordering::Release);
            return node;
        }
    }

    /// Nodes at the end of `unit`, in order, dirty, as `push` makes them:
    /// built (their predictions asked) and filed in segments of their own,
    /// half a segment's room each, on `threads` threads; the order's list
    /// takes those segments in one insert.
    pub fn push_many(&self, unit: UnitId, op: OpId, args: Vec<Vec<ValueId>>, threads: usize) -> Vec<NodeId> {
        let n = args.len();
        let u = self.unit(unit);
        // the unit's last segment gives up its labels after its last node
        let (old, p0) = loop {
            let seg = self.seg(self.order.seg_at(Pos(u.end.0 - 1)));
            let inner = seg.inner.read();
            if seg.unit != unit || Pos(u.end.0 - 1) >= inner.hi {
                continue;
            }
            let last = inner.nodes.keys().next_back().copied().unwrap_or(seg.lo.max(u.start));
            break (seg.clone(), Pos(last.0 + GAP));
        };
        if n < PUSH_RUN || p0.0.saturating_add((n as u64 + 1) * GAP) >= u.end.0 {
            return args.into_iter().map(|a| self.push(unit, op, a)).collect();
        }
        let label = |k: usize| Pos(p0.0 + k as u64 * GAP);
        let runs = n.div_ceil(PUSH_RUN);
        let first_id = self.nodes.reserve_n(n);
        let sids: Vec<SegId> = (0..runs)
            .map(|_| SegId(u32::try_from(self.segs.reserve()).expect("segment ids")))
            .collect();
        // (each thread its runs, every `t`-th, their arguments its own)
        let t = threads.clamp(1, runs);
        let mut shares: Vec<Vec<(usize, Vec<Vec<ValueId>>)>> = (0..t).map(|_| Vec::new()).collect();
        let mut args = args.into_iter();
        for r in 0..runs {
            shares[r % t].push((r, args.by_ref().take(PUSH_RUN).collect()));
        }
        let made = |r: usize, xs: Vec<Vec<ValueId>>| {
            let (a, b) = (r * PUSH_RUN, ((r + 1) * PUSH_RUN).min(n));
            let hi = if r + 1 == runs { u.end } else { label(b) };
            let seg = Arc::new(Segment::new(sids[r], unit, label(a), hi));
            {
                let mut inner = seg.inner.write();
                for (k, x) in (a..b).zip(xs) {
                    let id = NodeId::from_raw((first_id + k) as u64);
                    let (node, pred) = commit::built(self, unit, None, op, x, id);
                    let p = label(k);
                    node.seg.store(seg.id.0, Ordering::Release);
                    node.pos.store(p.0, Ordering::Release);
                    *node.woke.lock() = Some(Wake {
                        by: None,
                        cause: Cause::Created { by: None },
                        at_ns: self.now_ns(),
                    });
                    self.nodes.set(first_id + k, node.clone());
                    inner.nodes.insert(p, id);
                    inner.holes += 1;
                    inner.by_op.entry(op).or_default().push(id);
                    commit::file_predicted(self, &seg, &mut inner, &node, pred);
                }
            }
            self.segs.set(sids[r].0 as usize, seg);
        };
        crate::crew::scope(|s| {
            let mut shares = shares.into_iter();
            let mine = shares.next().expect("a share");
            for share in shares {
                s.spawn(|| share.into_iter().for_each(|(r, xs)| made(r, xs)));
            }
            mine.into_iter().for_each(|(r, xs)| made(r, xs));
        });
        self.order
            .insert_many((0..runs).map(|r| (label(r * PUSH_RUN), sids[r])).collect());
        old.inner.write().hi = p0;
        u.last.store(label(n - 1).0, Ordering::Release);
        self.stats.nodes_made.fetch_add(n as u64, Ordering::Relaxed);
        let ids: Vec<NodeId> = (0..n).map(|k| NodeId::from_raw((first_id + k) as u64)).collect();
        self.pending.lock().nodes.extend_from_slice(&ids);
        let mut st = CommitStats::default();
        for id in &ids {
            settle_predictions(self, None, self.node(*id), &mut st);
        }
        self.note_stats(None, &st);
        ids
    }

    /// The node that took `n`, if it is taken, else the nearest pushed
    /// sibling before `n` that is not inert, re-asked
    /// when its claim may have changed shape (`Cause::Structure`).
    fn wake_claimer(&self, n: &Node) {
        // (a node taken: the node that took it, a continuation placed
        // after a sibling as much as a sibling)
        if n.taken()
            && let Some(c) = n.claimer()
        {
            let c = self.node(c);
            if c.state() != State::Gone {
                mark_dirty(
                    self,
                    None,
                    c,
                    Wake {
                        by: Some(n.id),
                        cause: Cause::Structure,
                        at_ns: self.now_ns(),
                    },
                );
                return;
            }
        }
        let mut cur = Some(n.seg_hint());
        let mut upper = Some(n.pos_hint());
        while let Some(sid) = cur {
            let seg = self.seg(sid);
            if seg.unit != n.unit {
                return;
            }
            let found = {
                let inner = seg.inner.read();
                let hi = upper.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                let mut found = None;
                for (_, id) in inner.nodes.range((std::ops::Bound::Unbounded, hi)).rev() {
                    let s = self.node(*id);
                    if s.parent.is_some() || s.state() == State::Gone || s.taken() || !self.op(s.op()).sibling() {
                        continue;
                    }
                    found = Some(s.clone());
                    break;
                }
                found
            };
            if let Some(c) = found {
                if c.run_record().is_some_and(|r| r.claim > 0) {
                    mark_dirty(
                        self,
                        None,
                        &c,
                        Wake {
                            by: Some(n.id),
                            cause: Cause::Structure,
                            at_ns: self.now_ns(),
                        },
                    );
                }
                return;
            }
            upper = None;
            cur = self.order.prev(sid, seg.lo);
        }
    }

    fn inserted(&self, node: &Arc<Node>, near: &Node) -> NodeId {
        self.stats.nodes_made.fetch_add(1, Ordering::Relaxed);
        self.stats.edits.fetch_add(1, Ordering::Relaxed);
        enqueue(
            self,
            None,
            node,
            Wake {
                by: None,
                cause: Cause::Created { by: None },
                at_ns: self.now_ns(),
            },
        );
        let mut st = CommitStats::default();
        settle_predictions(self, None, node, &mut st);
        self.note_stats(None, &st);
        self.wake_claimer(near);
        if near.taken() {
            self.wake_claimer(node);
        }
        node.id
    }

    /// A node after `node` (and its placements), dirty.
    pub fn insert_after(&self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let near = self.node(node);
        let id = NodeId::from_raw(self.nodes.reserve() as u64);
        let (n, pred) = commit::built(self, near.unit, None, op, args, id);
        let n = place_after(self, node, n, pred);
        self.inserted(&n, near)
    }

    /// A node placed beside node `beside` as its placer placed it: after
    /// it (and its placements, and what others placed after it) or
    /// before it, with its placer, anchor and slot, listed among the
    /// placer's placements next to it, dirty. The placer's next run
    /// placing the same (op, args) there keeps it (`match_placements`):
    /// a client that edits a run's placements in step with texts of its
    /// own makes no second node. Beside a node no run placed:
    /// `insert_after`, `insert_before`.
    pub fn insert_placed(&self, beside: NodeId, after: bool, op: OpId, args: Vec<ValueId>) -> NodeId {
        let near = self.node(beside);
        let Some(placer) = near
            .parent
            .filter(|&p| self.node(p).run_record().is_some_and(|r| r.placed.contains(&beside)))
        else {
            return if after {
                self.insert_after(beside, op, args)
            } else {
                self.insert_before(beside, op, args)
            };
        };
        let id = NodeId::from_raw(self.nodes.reserve() as u64);
        let (n, pred) = commit::built(self, near.unit, Some(placer), op, args.clone(), id);
        n.follows.store(near.follows.load(Ordering::Acquire), Ordering::Release);
        n.anchor.store(near.anchor.load(Ordering::Acquire), Ordering::Release);
        n.slot.store(near.slot(), Ordering::Release);
        let Some(n) = commit::place_beside(self, beside, after, n, pred) else {
            return self.insert_after(placer, op, args);
        };
        commit::edit_placed(self, placer, |v| {
            let i = v
                .iter()
                .position(|&x| x == beside)
                .map_or(v.len(), |i| i + usize::from(after));
            v.insert(i, id);
        });
        self.inserted(&n, near)
    }

    /// Removes a placed node and its placements (`remove`), and takes it
    /// out of its placer's placements: the inverse of `insert_placed`.
    pub fn remove_placed(&self, node: NodeId) {
        if let Some(p) = self.node(node).parent {
            commit::edit_placed(self, p, |v| v.retain(|&x| x != node));
        }
        self.remove(node);
    }

    /// A node before `node`, dirty.
    pub fn insert_before(&self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let near = self.node(node);
        let id = NodeId::from_raw(self.nodes.reserve() as u64);
        let (n, pred) = commit::built(self, near.unit, None, op, args, id);
        let mut tries = 0;
        loop {
            let Some((seg, mut inner)) = own_write(self, near) else {
                let n = self.push_node(n, pred);
                return self.inserted(&n, near);
            };
            if Segment::needs_split(&inner) {
                let sid = SegId(u32::try_from(self.segs.reserve()).expect("segment ids"));
                seg.split(&mut inner, self, sid);
                continue;
            }
            if !seg.has_room(&inner, 1) && (tries < ROOM_TRIES || !seg.can_place(&inner)) {
                drop(inner);
                tries += 1;
                assert!(tries <= 2 * ROOM_TRIES, "no room after rebalances");
                self.order.rebalance(self, seg.id, 1);
                continue;
            }
            let p = match seg.gap_before(&inner, near.pos_hint()) {
                Some(p) => p,
                None => {
                    seg.relabel(&mut inner, self);
                    seg.gap_before(&inner, near.pos_hint()).expect("a gap after a relabel")
                }
            };
            n.seg.store(seg.id.0, Ordering::Release);
            n.pos.store(p.0, Ordering::Release);
            self.nodes
                .set(usize::try_from(id.raw()).unwrap_or(usize::MAX), n.clone());
            inner.nodes.insert(p, id);
            inner.holes += 1;
            inner.by_op.entry(op).or_default().push(id);
            commit::file_predicted(self, seg, &mut inner, &n, pred);
            drop(inner);
            return self.inserted(&n, near);
        }
    }

    /// The node's text changed: same id, same position, new (op, args),
    /// dirty; what its old run defined is re-asked at its next commit.
    pub fn replace(&self, node: NodeId, op: OpId, args: Vec<ValueId>) {
        let n = self.node(node);
        let old_op = n.op();
        if old_op != op
            && let Some((_, mut inner)) = own_write(self, n)
        {
            if let Some(v) = inner.by_op.get_mut(&old_op) {
                v.retain(|&x| x != node);
            }
            inner.by_op.entry(op).or_default().push(node);
            n.op.store(op.0, Ordering::Release);
        }
        let effect = self.op(op).effect(&args, &self.values);
        *n.text.write() = args.into();
        self.stats.edits.fetch_add(1, Ordering::Relaxed);
        n.effect.store(crate::node::pack_effect(effect), Ordering::Release);
        if n.taken() {
            self.wake_claimer(n);
            return;
        }
        // (its operation's brackets now: readers after it see them before
        // it runs)
        let mut st = CommitStats::default();
        commit::file_effect(self, None, n, None, &mut st);
        self.note_stats(None, &st);
        mark_dirty(
            self,
            None,
            n,
            Wake {
                by: None,
                cause: Cause::Edited,
                at_ns: self.now_ns(),
            },
        );
    }

    /// Removes a node and its placements (`commit::unfile`).
    pub fn remove(&self, node: NodeId) {
        let n = self.node(node);
        let was_inert = n.taken();
        self.stats.edits.fetch_add(1, Ordering::Relaxed);
        unfile(self, None, n);
        if was_inert {
            self.wake_claimer(n);
        }
    }

    /// Marks every node whose (op, args) passes `pred` dirty.
    pub fn refresh_where(&self, pred: &dyn Fn(OpId, &[ValueId], &Values) -> bool) {
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else {
                continue;
            };
            if matches!(n.state(), State::Gone | State::Inert | State::RunningInert) {
                continue;
            }
            if pred(n.op(), &n.args(), &self.values) {
                mark_dirty(
                    self,
                    None,
                    n,
                    Wake {
                        by: None,
                        cause: Cause::Edited,
                        at_ns: self.now_ns(),
                    },
                );
            }
        }
    }

    /// The nodes `n`'s last run placed, in order (tests).
    pub fn placed_by(&self, n: NodeId) -> Vec<NodeId> {
        self.node(n).record().map(|r| r.placed.clone()).unwrap_or_default()
    }

    /// Whether `a` stands before `b` on the line.
    pub fn before(&self, a: NodeId, b: NodeId) -> bool {
        self.node(a).pos_hint() < self.node(b).pos_hint()
    }

    pub fn output(&self, n: NodeId) -> Option<ValueId> {
        let node = self.nodes.get(usize::try_from(n.raw()).ok()?)?;
        if matches!(node.state(), State::Gone | State::Inert | State::RunningInert) {
            return None;
        }
        node.run_record().map(|r| r.output)
    }

    /// Its placements in order.
    pub fn children(&self, n: NodeId) -> Vec<NodeId> {
        self.node(n)
            .run_record()
            .map(|r| {
                r.placed
                    .iter()
                    .copied()
                    .filter(|&c| self.placed_node(c).is_some_and(|n| n.state() != State::Gone))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn claimed(&self, n: NodeId) -> Vec<NodeId> {
        let node = self.node(n);
        let k = node.run_record().map_or(0, |r| r.claim);
        let mut at = None;
        (0..k)
            .filter_map(|i| read::following_on(self, node, i, &mut at).map(|s| s.id))
            .collect()
    }

    /// The node whose definition of `f` reaches `n` (None: the environment).
    pub fn reaching_def(&self, n: NodeId, f: Field) -> Option<NodeId> {
        reaching_at(self, f, n, false)?.by
    }

    /// `n`'s own definition of `f`.
    pub fn def_value(&self, n: NodeId, f: Field) -> Option<ValueId> {
        let node = self.node(n);
        loop {
            if node.state() == State::Gone {
                return None;
            }
            let seg = self.seg(node.seg_hint());
            let inner = seg.inner.read();
            let p = node.pos_hint();
            if inner.nodes.get(&p) != Some(&n) {
                continue;
            }
            return inner
                .defs
                .get(&f)
                .and_then(|m| m.get(&p))
                .filter(|d| d.by == n)
                .map(|d| d.plain(&self.values).value);
        }
    }

    /// `n`'s own definitions of `f`, one per depth.
    fn def_parts(&self, n: NodeId, f: Field) -> Vec<ValueId> {
        let node = self.node(n);
        loop {
            if node.state() == State::Gone {
                return Vec::new();
            }
            let seg = self.seg(node.seg_hint());
            let inner = seg.inner.read();
            let p = node.pos_hint();
            if inner.nodes.get(&p) != Some(&n) {
                continue;
            }
            return inner
                .defs
                .get(&f)
                .and_then(|m| m.get(&p))
                .filter(|d| d.by == n)
                .map(|d| d.parts(&self.values).iter().map(|x| x.value).collect())
                .unwrap_or_default();
        }
    }

    pub fn defs_of(&self, n: NodeId) -> Vec<(Field, ValueId)> {
        let node = self.node(n);
        let Some(r) = node.run_record() else {
            return Vec::new();
        };
        r.defs
            .iter()
            .map(|d| {
                (
                    d.f,
                    if d.rel.is_some() {
                        self.def_value(n, d.f).unwrap_or(d.value)
                    } else {
                        d.value
                    },
                )
            })
            .collect()
    }

    /// What `n`'s last run contributed, by field.
    pub fn contribs_of(&self, n: NodeId) -> Vec<(Field, ValueId)> {
        self.node(n)
            .run_record()
            .map(|r| r.contribs.clone())
            .unwrap_or_default()
    }

    /// The fold over all of `unit`'s contributions to `f`.
    pub fn acc_value(&self, unit: UnitId, f: Field) -> Option<ValueId> {
        if !self.contribs.has(f) {
            return None;
        }
        let u = self.unit(unit);
        Some(read::fold(self, f, Some(Pos(u.start.0 - 1)), Some(u.end)))
    }

    fn epoch_bucket(f: Field) -> usize {
        usize::try_from(crate::values::hash_words(0, &[f.0.raw()]) % EPOCH_BUCKETS as u64).unwrap_or(0)
    }

    /// `f`'s bucket's change count: loaded before a read, compared at the
    /// reader's commit.
    pub(crate) fn field_epoch(&self, f: Field) -> u64 {
        self.field_epochs[Self::epoch_bucket(f)].load(Ordering::SeqCst)
    }

    /// Counts a change of `f`'s definitions, contributions or environment
    /// value, made before this call.
    pub(crate) fn bump(&self, f: Field) {
        self.field_epochs[Self::epoch_bucket(f)].fetch_add(1, Ordering::SeqCst);
    }

    /// The predictions commits replaced ([`Engine::pred_pending`]),
    /// judged now the evaluation is quiet: confirmed where the node's
    /// last commit defines the field at its place with the predicted
    /// value and depth (a global definition of a node outside every
    /// scope standing for depth 0, as a commit confirms one), else
    /// refuted, by field. A node gone is neither.
    fn judge_predictions(&self, rep: &mut Report, by_field: &mut FxHashMap<Field, u64>) {
        let pending = std::mem::take(&mut *self.pred_pending.lock());
        for (n, f, value, depth) in pending {
            let Some(node) = self.placed_node(n).filter(|x| x.state() != State::Gone) else {
                continue;
            };
            let seg = self.seg(node.seg_hint());
            let inner = seg.inner.read();
            let now = inner
                .defs
                .get(&f)
                .and_then(|m| m.get(&node.pos_hint()))
                .map(|d| (d.value, d.depth));
            drop(inner);
            let at_top = || crate::read::depth_of(self, n) == Some(0);
            let right = now.is_some_and(|(x, d)| {
                x == value && (d == depth || (depth == 0 && d == crate::segment::GLOBAL && at_top()))
            });
            if right {
                rep.predictions_confirmed += 1;
            } else {
                rep.predictions_refuted += 1;
                *by_field.entry(f).or_default() += 1;
                crate::obs::pred_miss(f, now.is_some_and(|(x, _)| x == value));
            }
        }
    }

    /// The fold of contributions to `f` (default: `SeqMonoid`).
    pub fn declare_accumulator(&self, f: Field, m: Arc<dyn Monoid>) {
        self.monoids.write().by_field.insert(f, m);
    }

    pub fn declare_accumulator_prefix(&self, prefix: &str, m: Arc<dyn Monoid>) {
        self.monoids.write().by_prefix.push((prefix.to_owned(), m));
    }

    pub fn set_limits(&self, l: Limits) {
        self.values.count_payload(l.collect_at_bytes.is_some());
        *self.limits.write() = l;
    }

    pub fn limits(&self) -> Limits {
        *self.limits.read()
    }

    /// The version edits are reported at (`Settled::version`).
    pub fn set_version(&self, v: u64) {
        self.version.store(v, Ordering::Release);
    }

    pub fn set_environment(&self, env: impl IntoIterator<Item = (Field, ValueId)>) {
        let mut vals = self.env.values.write();
        for (f, x) in env {
            vals.insert(f, Some(x));
            self.bump(f);
        }
    }

    /// Changes environment values; readers of each changed field whose
    /// read reaches the environment are re-asked.
    pub fn update_environment(&self, changes: &[(Field, Option<ValueId>)]) {
        {
            let mut vals = self.env.values.write();
            for (f, x) in changes {
                vals.insert(*f, *x);
                self.bump(*f);
            }
        }
        let mut st = CommitStats::default();
        for (f, _) in changes {
            // (a loop-back value published from the environment)
            self.ranges.mark_dirty(*f);
            let readers = self.ranges.take_envs(*f);
            // (one whose answer stands is kept for the next change)
            let mut stands = Vec::new();
            for r in readers {
                if !crate::segment::live(self, r.node, r.serial) {
                    continue;
                }
                if apply_path(self, read::env_value(self, *f), r.path) == r.seen {
                    stands.push(r);
                } else {
                    mark_dirty(
                        self,
                        None,
                        self.node(r.node),
                        Wake {
                            by: None,
                            cause: Cause::Environment { field: *f },
                            at_ns: self.now_ns(),
                        },
                    );
                }
            }
            if !stands.is_empty() {
                self.ranges.add_envs(*f, std::mem::take(&mut stands), |r| {
                    crate::segment::live(self, r.node, r.serial)
                });
            }
            if let Some(first) = self.order.first() {
                commit::reask_from(self, None, *f, Pos(0), first, None, None, 0, &mut st);
            }
        }
        self.note_stats(None, &st);
    }

    pub fn environment(&self) -> Vec<(Field, ValueId)> {
        self.env
            .values
            .read()
            .iter()
            .filter_map(|(f, x)| x.map(|x| (*f, x)))
            .collect()
    }

    /// Answers environment misses.
    pub fn set_env_provider(&self, provider: Box<EnvProvider>) {
        *self.env.provider.write() = Some(provider);
    }

    pub fn clear_env_provider(&self) {
        *self.env.provider.write() = None;
    }

    /// Runs on `threads` workers until nothing is dirty or running.
    pub fn evaluate(&self, threads: usize) -> Result<Report, EvalError> {
        self.evaluate_with(threads, None)
    }

    pub fn evaluate_sequential(&self) -> Result<Report, EvalError> {
        self.evaluate(1)
    }

    /// `evaluate`, then `src.absorb` while it gives edits; one report for
    /// all of it.
    pub fn evaluate_absorbing(&self, threads: usize, src: &mut (dyn EditSource + Send)) -> Result<Report, EvalError> {
        self.evaluate_with(threads, Some(src))
    }

    /// The evaluation on a thread of the engine's own, with the stack every
    /// run is given ([`crate::WORKER_STACK`]): the caller's thread, whose
    /// stack the engine does not choose (std's 2 MiB for a spawned one),
    /// only waits. Runs nest as deep as their client lets them (TeX: 10000
    /// expansions), at 1 thread as at 16.
    fn evaluate_with(&self, threads: usize, src: Option<&mut (dyn EditSource + Send)>) -> Result<Report, EvalError> {
        if ENGINE_STACK.with(std::cell::Cell::get) {
            return self.evaluate_here(threads, src);
        }
        // (a thread of the crew's, `crate::crew`: the same one or another
        // whose caches an earlier evaluation left warm, never a new one
        // per evaluation; marked for the evaluation alone, its thread
        // going on to other jobs after it)
        let mut out = None;
        crate::crew::scope(|s| {
            s.spawn(|| {
                struct Unmark;
                impl Drop for Unmark {
                    fn drop(&mut self) {
                        ENGINE_STACK.with(|f| f.set(false));
                    }
                }
                ENGINE_STACK.with(|f| f.set(true));
                let _unmark = Unmark;
                out = Some(self.evaluate_here(threads, src));
            });
        });
        out.expect("the evaluation ended")
    }

    fn evaluate_here(
        &self,
        threads: usize,
        mut src: Option<&mut (dyn EditSource + Send)>,
    ) -> Result<Report, EvalError> {
        let t0 = Instant::now();
        if self.busy.fetch_add(1, Ordering::SeqCst) & COLLECTING != 0 {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            panic!("an evaluation begun while Engine::collect runs");
        }
        let _busy = Unbusy(&self.busy, 1);
        let n = threads.max(1);
        let mut all: Vec<crate::sched::Worker> = Vec::new();
        if self.repair.swap(false, Ordering::AcqRel) {
            self.repair_line();
        }
        loop {
            let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            self.final_gen.fetch_add(1, Ordering::AcqRel);
            let (pool, workers) = Pool::new(n, epoch, self.limits());
            let mut seed = self.take_pending(&workers[0]);
            // (a node that runs only when the line is quiet waits for it)
            seed.retain(|&n| !pool.defer(self, n));
            // (in position order, as the blocks run: a node inserted before
            // others runs with them, not after every node pushed)
            commit::sort_by_label(self, &mut seed);
            if !seed.is_empty() {
                let per = seed.len().div_ceil(n);
                for (i, chunk) in seed.chunks(per).enumerate() {
                    workers[i].seed(chunk);
                }
            }
            // (the trait object's lifetime shortened for the reborrow, which
            // `as_deref_mut` cannot)
            #[allow(clippy::option_as_ref_deref)]
            let workers = pool.run(self, workers, src.as_mut().map(|s| -> &mut dyn EditSource { &mut **s }))?;
            // a run that panicked: a failure if what it read still holds
            // now the line is quiet, else run again with the rest pending
            let panicked: Vec<_> = workers
                .iter()
                .flat_map(|w| std::mem::take(&mut w.log.lock().panicked))
                .collect();
            all.extend(workers);
            if panicked.is_empty() {
                break;
            }
            if let Some((node, _, op, message)) = panicked.iter().find(|(x, reads, _, _)| {
                // (a read that panics when answered again, a client's
                // fold broken since: not one that holds, so the node runs
                // again and its run says)
                reads.iter().all(|rd| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        read::holds(self, self.node(*x), rd, &mut read::FoldMemo::new())
                    }))
                    .unwrap_or(false)
                })
            }) {
                return Err(EvalError::OpPanicked {
                    node: *node,
                    op,
                    message: message.clone(),
                });
            }
        }
        let workers = all;
        if stale_audit_on() {
            for line in self.stale_audit() {
                eprintln!("defuse stale-audit: {line}");
            }
        }
        let swept = self.sweep_readers() as u64
            + workers
                .iter()
                .map(|w| std::mem::take(&mut w.log.lock().swept))
                .sum::<u64>();
        let mut rep = Report {
            readers_swept: swept,
            collections: workers
                .iter()
                .flat_map(|w| std::mem::take(&mut w.log.lock().collections))
                .collect(),
            ..Report::default()
        };
        let mut by_op: FxHashMap<OpId, u64> = FxHashMap::default();
        let mut placed: FxHashMap<OpId, u64> = FxHashMap::default();
        let mut moved: FxHashMap<OpId, u64> = FxHashMap::default();
        let mut wasted: FxHashMap<OpId, u64> = FxHashMap::default();
        let mut commit_by: FxHashMap<OpId, (u64, u64)> = FxHashMap::default();
        let mut rbf: FxHashMap<Field, u64> = FxHashMap::default();
        let mut pbf: FxHashMap<Field, u64> = FxHashMap::default();
        self.judge_predictions(&mut rep, &mut pbf);
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        for w in &workers {
            rep.nodes_run += load(&w.nodes_run);
            rep.runs += load(&w.runs);
            rep.refuted_nodes += load(&w.refuted_nodes);
            rep.resumed += load(&w.resumed);
            rep.parks += load(&w.parks);
            rep.stale_panics += load(&w.stale_panics);
            rep.stopped_stale += load(&w.stopped_stale);
            rep.wasted_ns += load(&w.wasted_ns);
            rep.predictions_confirmed += load(&w.predictions_confirmed);
            rep.relative_resolved += load(&w.relative_resolved);
            rep.shaped += load(&w.shaped);
            rep.commits += load(&w.commits);
            rep.final_reruns = rep
                .final_reruns
                .max(u32::try_from(load(&w.final_max)).unwrap_or(u32::MAX));
            rep.steals += load(&w.steals);
            rep.held_deferred += load(&w.held_deferred);
            rep.held_fired += load(&w.held_fired);
            rep.held_passes = rep
                .held_passes
                .max(u32::try_from(load(&w.held_passes)).unwrap_or(u32::MAX));
            rep.held_unsettled |= load(&w.held_unsettled) > 0;
            rep.times.op_ns += load(&w.op_ns);
            rep.times.wasted_ns += load(&w.wasted_ns);
            rep.times.commit_ns += load(&w.commit_ns);
            rep.times.commit_max_cpu_ns = rep.times.commit_max_cpu_ns.max(load(&w.commit_max_cpu_ns));
            rep.times.commit_allocs += load(&w.commit_allocs);
            rep.times.commit_frees += load(&w.commit_frees);
            if load(&w.commit_max_ns) > rep.times.commit_max_ns {
                rep.times.commit_max_ns = load(&w.commit_max_ns);
                rep.commit_max_node = Some(NodeId(load(&w.commit_max_node)));
            }
            rep.times.reask_ns += load(&w.reask_ns);
            rep.times.idle_ns += load(&w.idle_ns);
            rep.times.lock_wait_ns += load(&w.lock_wait_ns);
            rep.times.lock_wait_max_ns = rep.times.lock_wait_max_ns.max(load(&w.lock_wait_max_ns));
            let mut log = w.log.lock();
            for (op, k) in log.runs_by_op.drain() {
                *by_op.entry(op).or_default() += k;
            }
            for (op, k) in log.placed_by_op.drain() {
                *placed.entry(op).or_default() += k;
            }
            rep.placements_moved += std::mem::take(&mut log.placements_moved);
            for (op, k) in log.moved_by_op.drain() {
                *moved.entry(op).or_default() += k;
            }
            for (op, k) in log.wasted_by_op.drain() {
                *wasted.entry(op).or_default() += k;
            }
            for (op, (t, k)) in log.commit_by_op.drain() {
                let e = commit_by.entry(op).or_default();
                e.0 += t;
                e.1 += k;
            }
            rep.rerun_causes.append(&mut log.causes);
            rep.capped.append(&mut log.capped);
            for (f, k) in log.refuted_by_field.drain() {
                *rbf.entry(f).or_default() += k;
            }
            rep.run_times.append(&mut log.run_times);
        }
        rep.passes = rep.final_reruns + 1;
        rep.absorbed = workers
            .iter()
            .flat_map(|w| std::mem::take(&mut w.log.lock().absorbed))
            .collect();
        let starts: Vec<u64> = workers
            .iter()
            .flat_map(|w| std::mem::take(&mut w.log.lock().pass_starts))
            .collect();
        let mut at = 0;
        for s in starts.into_iter().chain([rep.runs]) {
            rep.pass_runs.push(s.saturating_sub(at));
            at = s;
        }
        rep.capped.sort_unstable();
        rep.capped.dedup();
        rep.first_runs = rep.nodes_run.saturating_sub(rep.refuted_nodes);
        rep.runs_by_op = by_op.into_iter().map(|(op, k)| (self.op(op).name(), k)).collect();
        rep.runs_by_op.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.placed_by_op = placed.into_iter().map(|(op, k)| (self.op(op).name(), k)).collect();
        rep.placed_by_op.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.placements_moved_by_op = moved.into_iter().map(|(op, k)| (self.op(op).name(), k)).collect();
        rep.placements_moved_by_op
            .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.wasted_by_op = wasted.into_iter().map(|(op, k)| (self.op(op).name(), k)).collect();
        rep.wasted_by_op.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.runs_wasted = rep.wasted_by_op.iter().map(|x| x.1).sum();
        rep.commit_by_op = commit_by
            .into_iter()
            .map(|(op, (t, k))| (self.op(op).name(), t, k))
            .collect();
        rep.commit_by_op.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.refuted_by_field = rbf.into_iter().collect();
        rep.predictions_refuted_by_field = pbf.into_iter().collect();
        rep.predictions_refuted_by_field.sort_by_key(|x| std::cmp::Reverse(x.1));
        rep.refuted_by_field.sort_by_key(|x| std::cmp::Reverse(x.1));
        rep.segments = self.order.count();
        rep.version = self.version.load(Ordering::Acquire);
        rep.edits = self.stats.edits.swap(0, Ordering::AcqRel);
        let last = workers
            .iter()
            .filter(|w| load(&w.last_commit) != u64::MAX)
            .max_by_key(|w| load(&w.last_done))
            .map_or(u64::MAX, |w| load(&w.last_commit));
        self.last_commit.store(last, Ordering::Release);
        let chain = self.critical_chain();
        rep.realised_critical_path = u32::try_from(chain.len()).unwrap_or(u32::MAX);
        rep.realised_critical_time = Duration::from_nanos(
            chain
                .iter()
                .map(|(n, _)| self.node(*n).run_ns.load(Ordering::Relaxed))
                .sum(),
        );
        rep.times.wall_ns = ns(t0);
        *self.last_report.lock() = Some(rep.clone());
        Ok(rep)
    }

    /// After a panic outside a run (a client's monoid, projection or
    /// hook at a commit or a walk): what the commit cut short left is put
    /// right before the next evaluation runs anything. Its products stand
    /// filed, but the walks that re-ask their readers, the queueing of
    /// its placements and the removal of those it no longer places may
    /// not have happened. Every node not run since it was made dirty is
    /// pending again, every reader whose answer moved is marked, every
    /// relation is computed from its input again, and every node whose
    /// placer no longer holds it is removed. A look at every node and
    /// read, once per panic; a check that panics again marks its reader.
    fn repair_line(&self) {
        let wake = || Wake {
            by: None,
            cause: Cause::Structure,
            at_ns: self.now_ns(),
        };
        let mut stray = Vec::new();
        for i in 0..self.nodes.len() {
            let Some(node) = self.nodes.get(i) else {
                continue;
            };
            match node.state() {
                State::Gone | State::Inert | State::RunningInert => continue,
                State::Dirty | State::Running | State::RunningDirty => {
                    node.set_state(State::Dirty);
                    self.pend(node.id);
                }
                State::Clean => {
                    let Some(r) = node.run_record() else {
                        continue;
                    };
                    let moved = r.reads.iter().any(|rd| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            answer(self, node, &rd.kind, rd.path) != rd.seen
                        }))
                        .unwrap_or(true)
                    });
                    if moved {
                        mark_dirty(self, None, node, wake());
                    }
                    for d in &r.defs {
                        if let Some(c) = d.rel
                            && commit::settle_rel(self, d.f, node.id, c)
                        {
                            let mut st = CommitStats::default();
                            commit::reask(self, None, d.f, node.pos_hint(), node.seg_hint(), node.id, &mut st);
                            self.note_stats(None, &st);
                        }
                    }
                }
            }
            if let Some(p) = node.parent
                && !self.node(p).record().is_some_and(|r| r.placed.contains(&node.id))
            {
                stray.push(node.clone());
            }
        }
        for n in stray {
            unfile(self, None, &n);
        }
    }

    pub fn is_live(&self, node: NodeId) -> bool {
        self.nodes
            .get(usize::try_from(node.raw()).unwrap_or(usize::MAX))
            .is_some_and(|n| n.state() != State::Gone)
    }

    /// (measure) What the line holds besides values: reader records filed
    /// in segments (all, and those still standing for their node), and the
    /// run records' reads, definitions and contributions of the nodes alive
    /// and of those gone.
    pub fn footprint(&self) -> String {
        let (filed, standing, fields) = self.reader_records();
        let (mut live, mut gone) = ([0usize; 4], [0usize; 4]);
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else { continue };
            let t = if n.state() == State::Gone { &mut gone } else { &mut live };
            t[0] += 1;
            if let Some(r) = n.record() {
                t[1] += r.reads.len();
                t[2] += r.defs.len();
                t[3] += r.contribs.len();
            }
        }
        format!(
            "readers filed {filed} ({standing} standing) over {fields} segment fields; nodes alive {} (reads {}, defs {}, contribs {}), gone {} (reads {}, defs {}, contribs {})",
            live[0], live[1], live[2], live[3], gone[0], gone[1], gone[2], gone[3]
        )
    }

    /// (measure) The reader records filed in segments, those still
    /// standing for their node, and the segments' fields read.
    /// Every value the engine holds: the nodes' arguments and their last
    /// records (output, definitions, contributions, what each read saw),
    /// the segments' definitions, readers, contributions and sums, the
    /// range, final and environment readers, the environment, the paths'
    /// steps, the memo of applications, and the fields named anywhere.
    pub(crate) fn roots(&self, keep: &mut dyn FnMut(ValueId)) {
        for &(_, f, x, _) in self.pred_pending.lock().iter() {
            keep(f.0);
            keep(x);
        }
        let step = |s: &Step, keep: &mut dyn FnMut(ValueId)| match *s {
            Step::Key(x)
            | Step::Has(x)
            | Step::Eq(x)
            | Step::Cmp(x)
            | Step::Indices(x)
            | Step::Keys(x)
            | Step::Rank(x)
            | Step::Ranks(x) => keep(x),
            _ => {}
        };
        for i in 0..self.nodes.len() {
            let Some(n) = self.nodes.get(i) else { continue };
            n.text.read().iter().for_each(|&x| keep(x));
            if let Some(r) = n.last.lock().as_ref() {
                keep(r.output);
                // (the state a resumed run goes on from, `Ctx::park`)
                r.state.into_iter().for_each(&mut *keep);
                for d in &r.defs {
                    keep(d.f.0);
                    keep(d.value);
                }
                for &(f, x) in &r.contribs {
                    keep(f.0);
                    keep(x);
                }
                for rd in &r.reads {
                    if let Some(f) = rd.kind.field() {
                        keep(f.0);
                    }
                    rd.seen.into_iter().for_each(&mut *keep);
                }
            }
            for (f, _, r) in n.bounded.lock().iter() {
                keep(f.0);
                if crate::segment::live(self, r.node, r.serial) {
                    r.seen.iter().for_each(|&(_, x)| x.into_iter().for_each(&mut *keep));
                }
            }
            n.stopped.lock().iter().for_each(|&(f, _)| keep(f.0));
            n.anchored.lock().iter().for_each(|f| keep(f.0));
        }
        for sid in self.order.all() {
            let inner = self.seg(sid).inner.read();
            for (f, m) in &inner.defs {
                keep(f.0);
                m.values().for_each(|d| keep(d.value));
            }
            for (f, rs) in &inner.readers {
                keep(f.0);
                rs.iter().for_each(|r| r.seen.into_iter().for_each(&mut *keep));
            }
            for (f, m) in &inner.contribs {
                keep(f.0);
                m.values().for_each(|&x| keep(x));
            }
            for (f, &x) in &inner.sums {
                keep(f.0);
                keep(x);
            }
            for (f, _, r) in &inner.stops {
                keep(f.0);
                r.seen.iter().for_each(|&(_, x)| x.into_iter().for_each(&mut *keep));
            }
        }
        self.ranges.roots(self, keep);
        for (f, x) in self.env.values.read().iter() {
            keep(f.0);
            if let Some(x) = *x {
                keep(x);
            }
        }
        for i in 0..self.paths.steps.len() {
            if let Some(ss) = self.paths.steps.get(i) {
                ss.iter().for_each(|s| step(s, keep));
            }
        }
        for ((_, a), &x) in self.memo.read().iter() {
            a.iter().for_each(|&y| keep(y));
            keep(x);
        }
        self.monoids.read().by_field.keys().for_each(|f| keep(f.0));
        self.shapes.read().keys().for_each(|(_, f)| keep(f.0));
        keep(self.depth_field.0);
        // (what marked nodes settled with, not taken yet: an output since
        // superseded on its node is in the client's next take)
        self.settled.lock().iter().for_each(|s| keep(s.output));
    }

    /// Frees the values nothing holds ([`Values::collect`]): the engine's
    /// own ([`Engine::roots`]) and what each operation says its client
    /// holds ([`Operation::roots`]). Only where nothing runs: between
    /// evaluations, or with every worker at the collection's barrier.
    /// Panics during an evaluation, which collects at its barrier
    /// (`Limits::collect_at_bytes`): its tasks read lists unpinned.
    pub fn collect(&self) -> Collected {
        assert!(
            self.busy
                .compare_exchange(0, COLLECTING, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok(),
            "Engine::collect during an evaluation: a collection runs between evaluations or at an evaluation's barrier (Limits::collect_at_bytes)"
        );
        let _busy = Unbusy(&self.busy, COLLECTING);
        self.collect_with(Vec::new())
    }

    /// The barrier's collection ([`Engine::collect_with`]), when this
    /// evaluation is the only one running (None: another began meanwhile,
    /// whose tasks the barrier does not stop).
    pub(crate) fn collect_at_barrier(&self, extra: Vec<ValueId>) -> Option<Collected> {
        self.busy
            .compare_exchange(1, 1 | COLLECTING, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        let _busy = Unbusy(&self.busy, COLLECTING);
        Some(self.collect_with(extra))
    }

    /// [`Engine::collect`], with `extra` roots the caller holds (an edit
    /// source's, `EditSource::roots`).
    pub(crate) fn collect_with(&self, extra: Vec<ValueId>) -> Collected {
        // (a stale reader's `seen` holds what an earlier run read: swept
        // first, it holds nothing)
        self.sweep_readers_now();
        let mut roots: Vec<ValueId> = extra;
        self.roots(&mut |x| roots.push(x));
        for i in 0..self.ops.len() {
            if let Some(op) = self.ops.get(i) {
                op.roots(self, &mut |x| roots.push(x));
            }
        }
        self.values.collect(roots)
    }

    /// The reader records of runs no longer their node's last dropped from
    /// every segment, and the lists they leave shrunk: at a quiet point
    /// (the end of an evaluation, a pass's quiescence), where nothing
    /// files a reader, once they outnumber the standing ones. The records
    /// of a node's earlier runs stay filed
    /// until met otherwise ([`crate::segment::push_pruned`]): in a build
    /// that runs most nodes again they were two in three records filed.
    /// The records dropped.
    pub(crate) fn sweep_readers(&self) -> usize {
        // (a sweep costs every record filed: only once the stale ones
        // outnumber those the last sweep left, so it costs each stale
        // record a few visits, and an edit of a few nodes nothing)
        let stale = self.stale_reads.load(Ordering::Relaxed);
        if stale < (1 << 16).max(self.swept_left.load(Ordering::Relaxed)) {
            return 0;
        }
        self.sweep_readers_now()
    }

    /// [`Engine::sweep_readers`] whatever the count.
    fn sweep_readers_now(&self) -> usize {
        self.stale_reads.store(0, Ordering::Relaxed);
        let mut dropped = 0;
        let mut left = 0;
        for sid in self.order.all() {
            let s = self.seg(sid);
            let mut inner = s.inner.write();
            inner.readers.retain(|_, rs| {
                let n = rs.len();
                rs.retain(|r| crate::segment::live(self, r.node, r.serial));
                dropped += n - rs.len();
                if rs.capacity() > 2 * rs.len() + 4 {
                    rs.shrink_to(rs.len());
                }
                left += rs.len();
                !rs.is_empty()
            });
            let n = inner.stops.len();
            inner
                .stops
                .retain(|(_, _, r)| crate::segment::live(self, r.node, r.serial));
            dropped += n - inner.stops.len();
        }
        self.swept_left.store(left as u64, Ordering::Relaxed);
        dropped
    }

    pub fn reader_records(&self) -> (usize, usize, usize) {
        let (mut filed, mut standing, mut fields) = (0usize, 0usize, 0usize);
        for sid in self.order.all() {
            let s = self.seg(sid);
            let inner = s.inner.read();
            fields += inner.readers.len();
            for rs in inner.readers.values() {
                filed += rs.len();
                standing += rs
                    .iter()
                    .filter(|r| crate::segment::live(self, r.node, r.serial))
                    .count();
            }
        }
        (filed, standing, fields)
    }

    /// What each live node's last run defined `f` to, in no order.
    pub fn defined_by_live(&self, f: Field) -> Vec<(NodeId, ValueId)> {
        (0..self.nodes.len())
            .filter_map(|i| self.nodes.get(i))
            .filter(|n| n.state() != State::Gone)
            .filter_map(|n| {
                let r = n.run_record()?;
                let x = r.defs.iter().rev().find(|d| d.f == f)?.value;
                Some((n.id, x))
            })
            .collect()
    }

    pub fn live_nodes(&self) -> usize {
        (0..self.nodes.len())
            .filter(|&i| self.nodes.get(i).is_some_and(|n| n.state() != State::Gone))
            .count()
    }

    /// The unit's nodes in position order.
    pub fn order(&self, u: UnitId) -> Vec<NodeId> {
        let mut out = Vec::new();
        for sid in self.order.all() {
            let s = self.seg(sid);
            if s.unit != u {
                continue;
            }
            out.extend(s.inner.read().nodes.values().copied());
        }
        out
    }

    pub fn node_info(&self, n: NodeId) -> Option<NodeInfo> {
        let node = self.nodes.get(usize::try_from(n.raw()).ok()?)?;
        let state = match node.state() {
            State::Gone => return None,
            State::Inert | State::RunningInert => NodeState::Inert,
            State::Clean => NodeState::Done,
            State::Dirty | State::Running | State::RunningDirty => NodeState::Dirty,
        };
        Some(NodeInfo {
            op: self.op(node.op()).name(),
            args: node.args().to_vec(),
            unit: node.unit,
            parent: node.parent,
            state,
            claimer: node.claimer(),
        })
    }

    /// Marks a node for `take_settled`.
    pub fn mark(&self, n: NodeId) {
        if self.marked.write().insert(n) {
            self.marked_n.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn unmark(&self, n: NodeId) {
        if self.marked.write().remove(&n) {
            self.marked_n.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Marked nodes whose output settled since the last take.
    pub fn take_settled(&self) -> Vec<Settled> {
        std::mem::take(&mut *self.settled.lock())
    }

    /// The nodes the line holds as done whose run read what no longer
    /// reaches them, with their reads: a re-ask lost. Asked with the line
    /// quiet (`PHITEX_STALE_AUDIT`, `profile`), where every done node's
    /// reads hold.
    pub fn stale_audit(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.nodes.len() {
            let Some(node) = self.nodes.get(i) else { continue };
            if node.state() != State::Clean {
                continue;
            }
            let Some(r) = node.run_record() else { continue };
            let mut memo = read::FoldMemo::new();
            if r.reads.iter().all(|rd| read::holds(self, node, rd, &mut memo)) {
                continue;
            }
            out.push(format!(
                "STALE {}{}",
                self.debug_node(node.id),
                self.debug_reads(node.id)
            ));
        }
        out
    }

    pub fn debug_reads(&self, n: NodeId) -> String {
        let node = self.node(n);
        let Some(r) = node.run_record() else {
            return format!("node {}: no run", n.raw());
        };
        let v = self.values();
        let mut out = String::new();
        for rd in &r.reads {
            let now = answer(self, node, &rd.kind, rd.path);
            let show = |x: Option<ValueId>| x.map_or("none".to_owned(), |x| v.render(x));
            // (a field read: its name and the node whose definition reaches)
            let site = match &rd.kind {
                crate::node::ReadKind::Field {
                    f,
                    at: crate::node::Anchor::Me,
                } => format!(
                    " [{} by {:?}]",
                    v.render(f.0),
                    read::reaching_at(self, *f, n, false)
                        .and_then(|r| r.by)
                        .map(NodeId::raw)
                ),
                _ => String::new(),
            };
            out.push_str(&format!(
                "{:?} path {} seen {} now {}{}{site}\n",
                rd.kind,
                rd.path.0,
                show(rd.seen),
                show(now),
                if now == rd.seen { "" } else { " STALE" }
            ));
        }
        out
    }

    pub fn debug_node(&self, n: NodeId) -> String {
        let node = self.node(n);
        let v = self.values();
        let args: Vec<String> = node.args().iter().map(|&a| v.render(a)).collect();
        let mut out = format!(
            "node {} {} [{}] {:?} seg {} pos {} serial {} parent {:?}\n",
            n.raw(),
            self.op(node.op()).name(),
            args.join(", "),
            node.state(),
            node.seg_hint().0,
            node.pos_hint().0,
            node.serial.load(Ordering::Relaxed),
            node.parent.map(NodeId::raw)
        );
        if let Some(r) = node.record() {
            out.push_str(&format!(
                "  record {} provisional {} output {} effect {:?}\n",
                r.serial,
                r.provisional,
                v.render(r.output),
                r.effect
            ));
            for d in &r.defs {
                out.push_str(&format!(
                    "  def {} = {}{} {:?}\n",
                    v.render(d.f.0),
                    v.render(self.def_value(n, d.f).unwrap_or(d.value)),
                    d.rel.map(|c| format!(" (rel {c:+})")).unwrap_or_default(),
                    d.loc
                ));
            }
            for (f, x) in &r.contribs {
                out.push_str(&format!("  contrib {} += {}\n", v.render(f.0), v.render(*x)));
            }
            if !r.placed.is_empty() {
                let ps: Vec<String> = r.placed.iter().map(|p| p.raw().to_string()).collect();
                out.push_str(&format!("  placed [{}]\n", ps.join(", ")));
            }
            if r.claim > 0 {
                out.push_str(&format!("  claim {}\n", r.claim));
            }
        }
        out
    }

    /// Debug: every committed node run again, in line order, on the line
    /// as it stands, its product compared with its record (output,
    /// definitions, contributions, placements, claim); nothing is
    /// committed. A line per node whose run now differs: what a run made
    /// that its recorded reads do not account for.
    pub fn rerun_check(&self) -> Vec<String> {
        let v = self.values();
        let mut out = Vec::new();
        for i in 0..self.nodes.len() {
            let Some(node) = self.nodes.get(i) else {
                continue;
            };
            if node.state() != State::Clean {
                continue;
            }
            let Some(rec) = node.run_record() else {
                continue;
            };
            let op = self.op(node.op()).clone();
            let mut cx = crate::ctx::Ctx::new(self, None, node);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.run(&mut cx)));
            let name = op.name();
            let Ok(Ok(x)) = r else {
                out.push(format!(
                    "node {} ({name}) stops or panics when run again",
                    node.id.raw()
                ));
                continue;
            };
            let p = cx.into_product(x);
            let mut why = Vec::new();
            if p.output != rec.output {
                why.push(format!(
                    "output {} was {}",
                    v.render(p.output).chars().take(200).collect::<String>(),
                    v.render(rec.output).chars().take(200).collect::<String>()
                ));
            }
            let defs = commit::dedup_defs(p.defs);
            for d in &defs {
                if d.rel.is_none()
                    && !rec
                        .defs
                        .iter()
                        .any(|e| e.f == d.f && e.loc == d.loc && e.value == d.value)
                {
                    why.push(format!(
                        "defines {} = {}",
                        v.render(d.f.0),
                        v.render(d.value).chars().take(200).collect::<String>()
                    ));
                }
            }
            for e in &rec.defs {
                if !defs.iter().any(|d| d.f == e.f && d.loc == e.loc) {
                    why.push(format!("no longer defines {}", v.render(e.f.0)));
                }
            }
            let cs = commit::combine_dups(self, p.contribs);
            for (f, x) in &cs {
                if !rec.contribs.iter().any(|(g, y)| g == f && y == x) {
                    let was = rec
                        .contribs
                        .iter()
                        .find(|(g, _)| g == f)
                        .map_or("none".to_owned(), |(_, y)| v.render(*y).chars().take(300).collect());
                    why.push(format!(
                        "contributes {} {} was {was}",
                        v.render(f.0),
                        v.render(*x).chars().take(300).collect::<String>()
                    ));
                }
            }
            let placed: Vec<(OpId, Vec<ValueId>)> = rec
                .placed
                .iter()
                .filter_map(|&c| self.placed_node(c))
                .map(|c| (c.op(), c.args().to_vec()))
                .collect();
            let now: Vec<(OpId, Vec<ValueId>)> = p
                .placements
                .iter()
                .map(|x| {
                    let (o, a) = x.key();
                    (o, a.to_vec())
                })
                .collect();
            if placed != now {
                why.push(format!("places {} nodes, recorded {}", now.len(), placed.len()));
            }
            if p.claim != rec.claim {
                why.push(format!("claims {} was {}", p.claim, rec.claim));
            }
            if !why.is_empty() {
                out.push(format!("node {} ({name}): {}", node.id.raw(), why.join("; ")));
            }
        }
        out
    }

    /// Checks every recorded read against its current answer and every
    /// relation against its input: a line per violation. Empty after any
    /// evaluation, or the engine is wrong.
    pub fn check_invariants(&self) -> Vec<String> {
        let mut out = Vec::new();
        let v = self.values();
        // (each segment filed under every field it holds a definition or a
        // contribution of: a read passes over a segment the index lacks)
        for sid in self.order.all() {
            let s = self.seg(sid);
            let inner = s.inner.read();
            for (f, m) in &inner.defs {
                if !m.is_empty() && !self.defs.holds(*f, s.lo, s.id) {
                    out.push(format!(
                        "segment {} holds definitions of {} the index lacks",
                        sid.0,
                        v.render(f.0)
                    ));
                }
            }
            for (f, m) in &inner.contribs {
                if !m.is_empty() && !self.contribs.holds(*f, s.lo, s.id) {
                    out.push(format!(
                        "segment {} holds contributions to {} the index lacks",
                        sid.0,
                        v.render(f.0)
                    ));
                }
            }
        }
        // (each placer's placements as a set, made once)
        let mut held: FxHashMap<NodeId, FxHashSet<NodeId>> = FxHashMap::default();
        for i in 0..self.nodes.len() {
            let Some(node) = self.nodes.get(i) else {
                continue;
            };
            let n = node.id;
            // (a placed node stands only while its placer holds it)
            if node.state() != State::Gone
                && let Some(p) = node.parent
                && !held
                    .entry(p)
                    .or_insert_with(|| {
                        self.node(p)
                            .record()
                            .map(|r| r.placed.iter().copied().collect())
                            .unwrap_or_default()
                    })
                    .contains(&n)
            {
                out.push(format!(
                    "node {} is on the line, its placer {} holding it no more",
                    n.raw(),
                    p.raw()
                ));
            }
            // (a node taken is held by a claimer on the line that is not
            // taken itself: a node removed or taken claims nothing)
            if node.state() == State::Inert {
                match node.claimer().map(|c| self.node(c).state()) {
                    Some(State::Gone | State::Inert) | None => out.push(format!(
                        "node {} is taken, its claimer {} gone, taken or none",
                        n.raw(),
                        node.claimer().map_or(-1, |c| i64::try_from(c.raw()).unwrap_or(-1))
                    )),
                    _ => {}
                }
            }
            match node.state() {
                State::Gone | State::Inert => continue,
                State::Dirty | State::Running | State::RunningDirty | State::RunningInert => {
                    out.push(format!("node {} is {:?} after the evaluation", n.raw(), node.state()));
                    continue;
                }
                State::Clean => {}
            }
            let Some(r) = node.run_record() else {
                out.push(format!("node {} is clean without a run", n.raw()));
                continue;
            };
            for rd in &r.reads {
                let now = answer(self, node, &rd.kind, rd.path);
                if now != rd.seen {
                    let site = match &rd.kind {
                        crate::node::ReadKind::Field {
                            f,
                            at: crate::node::Anchor::Me,
                        } => read::reaching_at(self, *f, n, false)
                            .and_then(|r| r.by)
                            .map_or(String::new(), |b| {
                                let bn = self.node(b);
                                format!(
                                    " (by {} at {} seg {} serial {}; reader at {} seg {} serial {})",
                                    b.raw(),
                                    bn.pos_hint().0,
                                    bn.seg_hint().0,
                                    bn.serial.load(std::sync::atomic::Ordering::Relaxed),
                                    node.pos_hint().0,
                                    node.seg_hint().0,
                                    node.serial.load(std::sync::atomic::Ordering::Relaxed)
                                )
                            }),
                        _ => String::new(),
                    };
                    out.push(format!(
                        "node {} ({}) read {:?} path {} saw {} but now {}{site}",
                        n.raw(),
                        self.op(node.op()).name(),
                        rd.kind,
                        rd.path.0,
                        rd.seen.map_or("none".to_owned(), |x| v.render(x)),
                        now.map_or("none".to_owned(), |x| v.render(x))
                    ));
                }
            }
            for d in &r.defs {
                if let Some(c) = d.rel {
                    let want = commit::rel_value_at(self, d.f, n, c);
                    let have = self.def_value(n, d.f);
                    if have != Some(want) {
                        out.push(format!(
                            "node {} relation {} {c:+}: holds {} but the input gives {}",
                            n.raw(),
                            v.render(d.f.0),
                            have.map_or("none".to_owned(), |x| v.render(x)),
                            v.render(want)
                        ));
                    }
                } else if !self.def_parts(n, d.f).contains(&d.value) {
                    out.push(format!(
                        "node {} definition {} is not filed as {}",
                        n.raw(),
                        v.render(d.f.0),
                        v.render(d.value)
                    ));
                }
            }
        }
        out
    }

    /// The realised chain of the last evaluation: from the last node to
    /// commit, each woken by the one before, with its cause.
    pub fn critical_chain(&self) -> Vec<(NodeId, Cause)> {
        let last = self.last_commit.load(Ordering::Acquire);
        if last == u64::MAX {
            return Vec::new();
        }
        let mut chain = Vec::new();
        let mut seen = FxHashSet::default();
        let mut cur = Some(NodeId::from_raw(last));
        while let Some(n) = cur {
            if !seen.insert(n) {
                break;
            }
            let Some(node) = self.nodes.get(usize::try_from(n.raw()).unwrap_or(usize::MAX)) else {
                break;
            };
            let (by, cause) = match &*node.woke.lock() {
                Some(w) => (w.by, w.cause.clone()),
                None => (None, Cause::Created { by: None }),
            };
            chain.push((n, cause));
            cur = by;
        }
        chain.reverse();
        chain
    }

    /// What a node's last record holds: its definitions, contributions,
    /// reads and placements, counted.
    pub fn record_sizes(&self, n: NodeId) -> Option<[usize; 4]> {
        let r = self.node(n).record()?;
        Some([r.defs.len(), r.contribs.len(), r.reads.len(), r.placed.len()])
    }

    /// The chain with each node's [run, commit, re-ask, gap-before] in ns.
    pub fn critical_chain_times(&self) -> Vec<(NodeId, [u64; 4])> {
        let chain = self.critical_chain();
        chain
            .iter()
            .map(|(n, _)| {
                let node = self.node(*n);
                let (run, commit, reask, done) = (
                    node.run_ns.load(Ordering::Relaxed),
                    node.commit_ns.load(Ordering::Relaxed),
                    node.reask_ns.load(Ordering::Relaxed),
                    node.done_ns.load(Ordering::Relaxed),
                );
                // from its wake to its run's start
                let woke_at = node.woke.lock().as_ref().map_or(0, |w| w.at_ns);
                let gap = done.saturating_sub(run + commit + reask).saturating_sub(woke_at);
                (*n, [run, commit, reask, gap])
            })
            .collect()
    }

    /// The chain with each node's height: how many woke it in turn.
    pub fn critical_chain_heights(&self) -> Vec<(NodeId, u32, Cause)> {
        self.critical_chain()
            .into_iter()
            .enumerate()
            .map(|(i, (n, c))| (n, u32::try_from(i).unwrap_or(u32::MAX), c))
            .collect()
    }

    /// The chain as (node, run ns, op name).
    pub fn realised_chain(&self) -> Vec<(NodeId, u64, String)> {
        self.critical_chain()
            .into_iter()
            .map(|(n, _)| {
                let node = self.node(n);
                (
                    n,
                    node.run_ns.load(Ordering::Relaxed),
                    self.op(node.op()).name().to_owned(),
                )
            })
            .collect()
    }

    /// Accepted, unused: no run audit in this core.
    pub fn set_run_audit(&self, _on: bool) {}

    /// The scope depth at a node: scopes open just before it (its closes
    /// not yet applied). For the client's diagnostics.
    pub fn depth_at(&self, n: NodeId) -> Option<u32> {
        read::depth_of(self, n)
    }
}

/// `PHITEX_STALE_AUDIT` (`profile`): [`Engine::stale_audit`] after every
/// evaluation, printed.
fn stale_audit_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::obs::enabled()
        && *ON.get_or_init(|| std::env::var_os("PHITEX_STALE_AUDIT").is_some_and(|v| !v.is_empty() && v != "0"))
}
