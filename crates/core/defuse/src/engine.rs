//! The engine as the client holds it: units, nodes, the environment,
//! evaluation, edits, reports.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::api::{
    CapPolicy, Cause, EditSource, EnvProvider, EvalError, Limits, LoopStart, Monoid, NodeId, NodeInfo, NodeState,
    OpId, Operation, PathId, ProjId, Projection, Report, SeqMonoid, Settled, Step, UnitId,
};
use crate::commit::{self, CommitStats, Predicted, own_write, place_after, settle_predictions, unfile};
use crate::index::{FieldIndex, RangeIndex};
use crate::lock::{Lock, Mx, ns};
use crate::node::{Node, State, Wake};
use crate::order::{GAP, Order, Pos, SegId, UNIT_SPAN};
use crate::read::{self, answer, apply_path, reaching_at};
use crate::sched::{Pool, Worker, enqueue, mark_dirty};
use crate::segment::Segment;
use crate::table::Table;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};

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
pub(crate) struct Env {
    pub(crate) values: Lock<FxHashMap<Field, Option<ValueId>>>,
    pub(crate) provider: Lock<Option<Box<EnvProvider>>>,
    pub(crate) readers: Lock<FxHashMap<Field, Vec<EnvReader>>>,
}

pub(crate) struct Paths {
    pub(crate) by_steps: Lock<FxHashMap<Box<[Step]>, PathId>>,
    pub(crate) steps: Table<Box<[Step]>>,
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
    /// Fields the client says never change after setup: reads of them are
    /// not recorded.
    pub(crate) stable: Lock<Arc<FxHashSet<Field>>>,
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
}

/// The buckets of [`Engine::field_epochs`].
const EPOCH_BUCKETS: usize = 4096;

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let paths = Paths {
            by_steps: Lock::new(FxHashMap::default()),
            steps: Table::new(),
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
        Engine {
            field_epochs: (0..EPOCH_BUCKETS).map(|_| AtomicU64::new(0)).collect(),
            values: Values::new(),
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
                readers: Lock::new(FxHashMap::default()),
            },
            paths,
            projections,
            shapes: Lock::new(FxHashMap::default()),
            shapes_n: AtomicUsize::new(0),
            memo: Lock::new(FxHashMap::default()),
            stable: Lock::new(Arc::new(FxHashSet::default())),
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
        self.nodes.get(usize::try_from(id.raw()).unwrap_or(usize::MAX)).expect("a node of this engine")
    }

    /// A node, None while it is not on the line yet: a commit's record
    /// names the placements it puts after other nodes (`place_after`)
    /// before they are placed.
    pub(crate) fn placed_node(&self, id: NodeId) -> Option<&Arc<Node>> {
        self.nodes.get(usize::try_from(id.raw()).unwrap_or(usize::MAX))
    }

    pub(crate) fn seg(&self, id: SegId) -> &Arc<Segment> {
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
                .with_str(f.0, |s| m.by_prefix.iter().find(|(p, _)| s.starts_with(p.as_str())).map(|x| x.1.clone()))
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
        let mut steps: Vec<Step> = self.path_steps(p).map(|x| x.to_vec()).unwrap_or_default();
        steps.push(s);
        self.path(&steps)
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

    pub(crate) fn note_settled(&self, node: &Node, output: ValueId) {
        if self.marked_n.load(Ordering::Relaxed) == 0 || !self.marked.read().contains(&node.id) {
            return;
        }
        self.settled.lock().push(Settled {
            node: node.id,
            version: self.version.load(Ordering::Acquire),
            output,
        });
    }

    /// The last node of a unit, if any.
    pub(crate) fn unit_last_node(&self, u: UnitId) -> Option<NodeId> {
        for sid in self.order.all().into_iter().rev() {
            let s = self.seg(sid);
            if s.unit != u {
                continue;
            }
            if let Some((_, &n)) = s.inner.read().nodes.iter().next_back() {
                return Some(n);
            }
        }
        None
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
        self.segs.set(sid.0 as usize, Arc::new(Segment::new(sid, id, start, end)));
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
        std::thread::scope(|s| {
            let mut shares = shares.into_iter();
            let mine = shares.next().expect("a share");
            for share in shares {
                s.spawn(|| share.into_iter().for_each(|(r, xs)| made(r, xs)));
            }
            mine.into_iter().for_each(|(r, xs)| made(r, xs));
        });
        self.order.insert_many((0..runs).map(|r| (label(r * PUSH_RUN), sids[r])).collect());
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

    /// The nearest pushed sibling before `n` that is not inert, re-asked
    /// when its claim may have changed shape (`Cause::Structure`).
    fn wake_claimer(&self, n: &Node) {
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
                    if s.parent.is_some() || s.state() == State::Gone || s.taken() {
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

    /// A node before `node`, dirty.
    pub fn insert_before(&self, node: NodeId, op: OpId, args: Vec<ValueId>) -> NodeId {
        let near = self.node(node);
        let id = NodeId::from_raw(self.nodes.reserve() as u64);
        let (n, pred) = commit::built(self, near.unit, None, op, args, id);
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
        *n.text.write() = args.into();
        self.stats.edits.fetch_add(1, Ordering::Relaxed);
        if n.taken() {
            self.wake_claimer(n);
            return;
        }
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
            return inner.defs.get(&f).and_then(|m| m.get(&p)).filter(|d| d.by == n).map(|d| d.value);
        }
    }

    pub fn defs_of(&self, n: NodeId) -> Vec<(Field, ValueId)> {
        let node = self.node(n);
        let Some(r) = node.run_record() else {
            return Vec::new();
        };
        r.defs
            .iter()
            .map(|d| (d.f, self.def_value(n, d.f).unwrap_or(d.value)))
            .collect()
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

    /// The fold of contributions to `f` (default: `SeqMonoid`).
    pub fn declare_accumulator(&self, f: Field, m: Arc<dyn Monoid>) {
        self.monoids.write().by_field.insert(f, m);
    }

    pub fn declare_accumulator_prefix(&self, prefix: &str, m: Arc<dyn Monoid>) {
        self.monoids.write().by_prefix.push((prefix.to_owned(), m));
    }

    /// A field that never changes after setup: reads of it go unrecorded.
    pub fn set_stable(&self, f: Field) {
        let mut s = self.stable.write();
        let mut set = (**s).clone();
        set.insert(f);
        *s = Arc::new(set);
    }

    pub fn set_limits(&self, l: Limits) {
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
            let readers = self.env.readers.write().remove(f).unwrap_or_default();
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
                self.env.readers.write().entry(*f).or_default().append(&mut stands);
            }
            if let Some(first) = self.order.first() {
                commit::reask_from(self, None, *f, Pos(0), first, None, None, &mut st);
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
    pub fn evaluate_absorbing(&self, threads: usize, src: &mut dyn EditSource) -> Result<Report, EvalError> {
        self.evaluate_with(threads, Some(src))
    }

    fn evaluate_with(&self, threads: usize, src: Option<&mut dyn EditSource>) -> Result<Report, EvalError> {
        let t0 = Instant::now();
        let n = threads.max(1);
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let (pool, workers) = Pool::new(n, epoch, self.limits(), self.stable.read().clone());
        let mut seed = self.take_pending(&workers[0]);
        // (in position order, as the blocks run: a node inserted before
        // others runs with them, not after every node pushed)
        seed.sort_by_key(|&n| self.node(n).pos_hint());
        if !seed.is_empty() {
            let per = seed.len().div_ceil(n);
            for (i, chunk) in seed.chunks(per).enumerate() {
                workers[i].seed(chunk);
            }
        }
        let workers = pool.run(self, workers, src)?;
        let mut rep = Report::default();
        let mut by_op: FxHashMap<OpId, u64> = FxHashMap::default();
        let mut rbf: FxHashMap<Field, u64> = FxHashMap::default();
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        for w in &workers {
            rep.nodes_run += load(&w.nodes_run);
            rep.runs += load(&w.runs);
            rep.refuted_nodes += load(&w.refuted_nodes);
            rep.wasted_ns += load(&w.wasted_ns);
            rep.predictions_confirmed += load(&w.predictions_confirmed);
            rep.predictions_refuted += load(&w.predictions_refuted);
            rep.relative_resolved += load(&w.relative_resolved);
            rep.shaped += load(&w.shaped);
            rep.commits += load(&w.commits);
            rep.steals += load(&w.steals);
            rep.times.op_ns += load(&w.op_ns);
            rep.times.wasted_ns += load(&w.wasted_ns);
            rep.times.commit_ns += load(&w.commit_ns);
            rep.times.commit_max_ns = rep.times.commit_max_ns.max(load(&w.commit_max_ns));
            rep.times.reask_ns += load(&w.reask_ns);
            rep.times.idle_ns += load(&w.idle_ns);
            rep.times.lock_wait_ns += load(&w.lock_wait_ns);
            rep.times.lock_wait_max_ns = rep.times.lock_wait_max_ns.max(load(&w.lock_wait_max_ns));
            let mut log = w.log.lock();
            for (op, k) in log.runs_by_op.drain() {
                *by_op.entry(op).or_default() += k;
            }
            rep.rerun_causes.append(&mut log.causes);
            for (f, k) in log.refuted_by_field.drain() {
                *rbf.entry(f).or_default() += k;
            }
            rep.run_times.append(&mut log.run_times);
        }
        rep.first_runs = rep.nodes_run.saturating_sub(rep.refuted_nodes);
        rep.runs_by_op = by_op.into_iter().map(|(op, k)| (self.op(op).name(), k)).collect();
        rep.runs_by_op.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        rep.refuted_by_field = rbf.into_iter().collect();
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

    pub fn is_live(&self, node: NodeId) -> bool {
        self.nodes
            .get(usize::try_from(node.raw()).unwrap_or(usize::MAX))
            .is_some_and(|n| n.state() != State::Gone)
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
            out.push_str(&format!(
                "{:?} path {} seen {} now {}{}\n",
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

    /// Checks every recorded read against its current answer and every
    /// relation against its input: a line per violation. Empty after any
    /// evaluation, or the engine is wrong.
    pub fn check_invariants(&self) -> Vec<String> {
        let mut out = Vec::new();
        let v = self.values();
        for i in 0..self.nodes.len() {
            let Some(node) = self.nodes.get(i) else {
                continue;
            };
            let n = node.id;
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
                        crate::node::ReadKind::Field { f, at: crate::node::Anchor::Me } => read::reaching_at(self, *f, n, false)
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
                } else if self.def_value(n, d.f) != Some(d.value) {
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
                (n, node.run_ns.load(Ordering::Relaxed), self.op(node.op()).name().to_owned())
            })
            .collect()
    }

    /// Accepted, unused: no run audit in this core.
    pub fn set_run_audit(&self, _on: bool) {}

    pub fn set_loop_start(&self, _s: LoopStart) {}

    pub fn set_max_rounds(&self, _n: u64) {}

    pub fn set_round_cap_policy(&self, _p: CapPolicy) {}

    pub fn declare_loopback(&self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn declare_loopback_early(&self, _f: Field, _init: ValueId) {
        unimplemented!("loop-back: next run")
    }

    pub fn declare_loopbacks(&self, _fields: &[(Field, ValueId)]) {
        unimplemented!("loop-back: next run")
    }

    pub fn loop_value(&self, _f: Field) -> Option<ValueId> {
        unimplemented!("loop-back: next run")
    }
}
