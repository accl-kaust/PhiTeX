//! A world: the complete state of one round, immutable and persistent
//! (`docs/clean-core.md`). A new world is made from an old one by cloning it
//! (every map is an `Arc` clone) and mutating the clone: only the paths that
//! change are copied.

use std::sync::Arc;

use super::ptree::{Count, Measure, NoMeasure, PMap};
use crate::api::{Brackets, Effect, Locality, MayDefine, OpId};
use crate::values::{Field, FxHashMap, ValueId};

/// A node id (`NodeId`'s raw value).
pub(crate) type Id = u64;
/// A position in a unit's order (an order-maintenance label).
pub(crate) type Pos = u64;
/// No node.
pub(crate) const NONE: Id = 0;

/// A frame: a scope of a unit. The unit's own frame is `root(u)`; a frame
/// opened by a node is named by the node and the frame's index among those
/// it opens.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct FrameId(pub u64);

impl FrameId {
    pub(crate) fn root(u: u32) -> FrameId {
        FrameId(u64::from(u) << 8 | 0xff)
    }
    pub(crate) fn opened(by: Id, k: u8) -> FrameId {
        FrameId(by << 8 | u64::from(k))
    }
    pub(crate) fn is_root(self) -> bool {
        self.0 & 0xff == 0xff
    }
}

/// What a node may define while its definition set is pending.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) enum May {
    #[default]
    Nothing,
    Any,
    Names(Vec<Field>),
    Prefixes(Vec<String>),
}

impl From<MayDefine> for May {
    fn from(m: MayDefine) -> Self {
        match m {
            MayDefine::Nothing => May::Nothing,
            MayDefine::Any => May::Any,
            MayDefine::Names(v) => May::Names(v),
            MayDefine::Prefixes(v) => May::Prefixes(v),
        }
    }
}

/// What a text's hooks say about it, asked once per text.
#[derive(Clone, Debug, Default)]
pub(crate) struct Static {
    /// The fields it defines (`None`: a pending set, bounded by `may`).
    pub defs: Option<Arc<[(Field, Locality)]>>,
    pub may: May,
    pub effect: Option<Effect>,
    pub contribs: Option<Arc<[(Field, ValueId)]>>,
    /// The listed contributions are all of them (none to other fields).
    pub contribs_complete: bool,
    pub preds: Option<Arc<[(Field, ValueId)]>>,
    pub fin: bool,
    /// The fields the run is declared to read (`reads`, a hint).
    pub reads: Option<Arc<[Field]>>,
}

/// A text: an operation on arguments, with its static facts.
#[derive(Debug)]
pub(crate) struct Text {
    pub op: OpId,
    pub args: Arc<[ValueId]>,
    pub stat: Static,
}

/// How a node came to be.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Place {
    /// Pushed or inserted by the client.
    Top,
    /// A child of its parent (`child`).
    Child,
    /// A continuation of its parent (`follow`), after the parent's subtree.
    Cont,
    /// A continuation placed after `anchor`'s subtree (`follow_after`).
    After(Id),
    /// A scope marker its parent placed among its children (`open_scope`
    /// opens, `close_scope` closes).
    Marker(bool),
}

/// Where a node is in its run's life.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum St {
    /// To run (no result, or one whose reads may no longer hold).
    Dirty,
    /// Its committed result stands.
    Done,
    /// Claimed by `by`: not run.
    Inert(Id),
    /// A final node held until the final phase.
    Held,
}

/// A definition a run made.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Def {
    pub f: Field,
    pub loc: Locality,
    pub val: ValueId,
}

/// One read a run made: the question and the answer it got.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Read {
    /// The definition of `f` reaching position `at` (a node: before it, or
    /// after its subtree when `after`), answered by `site` (`NONE`: the
    /// environment or the unit's start) with `val`, through projection
    /// `proj` (0: in full).
    Use {
        f: Field,
        at: Id,
        after: bool,
        site: Id,
        val: Option<ValueId>,
        proj: u16,
    },
    /// A loop-back field's round value.
    Loop { f: Field, val: ValueId },
    /// An environment field.
    Env { f: Field, val: Option<ValueId> },
    /// The output of a node.
    Out { node: Id, val: Option<ValueId> },
    /// The kind of the scope a node is in.
    Kind { at: Id, val: Option<ValueId> },
    /// Whether a field is a loop-back field.
    IsLoop { f: Field, val: bool },
    /// A position: whether `node` is there (`use_field_at`).
    At { node: Id, there: bool },
    /// The fold of accumulator `f` over the window after `from` (`NONE`:
    /// the unit's start) up to `to` (`NONE`: the reader); `past`: `to` may
    /// lie past the reader (`Ctx::acc_fold`, `Ctx::acc_next`; with `to`
    /// `NONE`, the window runs to the unit's end), else the window stops
    /// at it.
    Range { f: Field, from: Id, to: Id, val: ValueId, past: bool },
    /// The `i`-th following sibling (`Ctx::following`): its id and text.
    Sib { i: u32, ans: Option<(Id, usize)> },
    /// The siblings a declared count claim holds (`Ctx::claimed`).
    Claimed { list: Vec<(Id, usize)> },
    /// A backward sibling walk (`Ctx::prev_sibling_where`): the entries
    /// walked (id, text, inert) up to the match, and the match.
    Prev { walk: Vec<(Id, usize, bool)>, ans: Option<Id> },
    /// Sequence `f` before the reader (`prev`: its last element).
    Seq { f: Field, prev: bool, val: Option<ValueId> },
    /// A stream claim's elements and outputs, and the node it ended before.
    Stream { outs: Vec<(Id, ValueId)>, end: Option<Id>, fields: Vec<(Field, Id)> },
}

/// A text's identity (texts are interned: one per operation and arguments).
pub(crate) fn text_key(t: &Arc<Text>) -> usize {
    Arc::as_ptr(t) as usize
}

/// One reader index entry a read files.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum ReadKey {
    /// A reader of field `f` of a unit at a position.
    Field(u32, Field, Pos),
    /// A reader of a node's output (or of its being there).
    Out(Id),
    /// A range reader of an accumulator over a window.
    Range(u32, Field, Pos, Pos),
}

/// A committed run's result.
#[derive(Clone, Debug)]
pub(crate) struct Res {
    pub out: ValueId,
    pub defs: Vec<Def>,
    pub contribs: Vec<(Field, ValueId)>,
    pub pubs: Vec<(Field, ValueId)>,
    /// The scope effect it decided (a pending effect's), if any.
    pub effect: Option<Effect>,
    pub kinds: Vec<ValueId>,
    pub kids: Vec<Id>,
    pub conts: Vec<Id>,
    pub reads: Vec<Read>,
    pub diag: Vec<ValueId>,
    /// The claim the run made (`Ctx::claim`), overriding the declared one.
    pub claim: Option<usize>,
    /// The reader entries at fixed places a worker filed ahead for this
    /// run (sorted, distinct), with the renumbering epoch it made them at
    /// (`World::relabels`): the merge files the run by them.
    pub filed: Option<(u64, Arc<[ReadKey]>)>,
    /// Its definitions by field (the first of each), made when first
    /// asked of a result with many (`Engine::def_value`).
    pub def_ix: std::sync::OnceLock<Arc<FxHashMap<Field, ValueId>>>,
}

impl Res {
    /// The value it defines for `f` (defined at more than one locality,
    /// the first made).
    pub(crate) fn def_value(&self, f: Field) -> Option<ValueId> {
        if self.defs.len() <= 16 {
            return self.defs.iter().find(|d| d.f == f).map(|d| d.val);
        }
        let ix = self.def_ix.get_or_init(|| {
            let mut m = FxHashMap::default();
            for d in &self.defs {
                m.entry(d.f).or_insert(d.val);
            }
            Arc::new(m)
        });
        ix.get(&f).copied()
    }
}

impl Default for Res {
    fn default() -> Self {
        Res {
            out: ValueId::UNIT,
            defs: Vec::new(),
            contribs: Vec::new(),
            pubs: Vec::new(),
            effect: None,
            kinds: Vec::new(),
            kids: Vec::new(),
            conts: Vec::new(),
            reads: Vec::new(),
            diag: Vec::new(),
            claim: None,
            filed: None,
            def_ix: std::sync::OnceLock::new(),
        }
    }
}

/// A node in a world.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub text: Arc<Text>,
    pub unit: u32,
    pub parent: Id,
    pub place: Place,
    /// Depth in the unfolding: a top node 0, a child one more than its
    /// parent, a continuation its parent's.
    pub depth: u32,
    pub pos: Pos,
    /// The frame it is in (after the brackets it has before itself).
    pub frame: FrameId,
    pub st: St,
    pub res: Option<Arc<Res>>,
    /// A run defined outside its pending set's bound: it may define
    /// anything until its text changes.
    pub wide: bool,
    /// The bound a stopped run narrowed its pending set to (`Ctx::narrow`),
    /// until it runs again.
    pub narrowed: Option<Arc<May>>,
}

impl Node {
    /// What its pending set may define now.
    pub(crate) fn may(&self) -> &May {
        self.narrowed.as_deref().unwrap_or(&self.text.stat.may)
    }
}

impl Node {
    /// The scope brackets it has now: its run's decided effect, else its
    /// text's (a pending one none).
    pub(crate) fn brackets(&self) -> Brackets {
        if let Place::Marker(open) = self.place {
            return if open {
                Brackets {
                    opens: 1,
                    ..Brackets::default()
                }
            } else {
                Brackets {
                    closes: 1,
                    ..Brackets::default()
                }
            };
        }
        if matches!(self.st, St::Inert(_)) {
            return Brackets::default();
        }
        match (self.res.as_ref().and_then(|r| r.effect), self.text.stat.effect) {
            (Some(e), _) | (None, Some(e)) => e.brackets(),
            (None, None) => Brackets::default(),
        }
    }

    pub(crate) fn out(&self) -> Option<ValueId> {
        match self.st {
            St::Inert(_) => None,
            _ => self.res.as_ref().map(|r| r.out),
        }
    }
}

/// A frame of a unit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub parent: FrameId,
    pub depth: u32,
    /// Its opener's position (0: a root frame).
    pub start: Pos,
    pub kind: Option<ValueId>,
}

/// A unit: its order and frames, and everything indexed by position in
/// it (a cell, written as a whole).
#[derive(Clone, Default)]
pub(crate) struct Unit {
    /// The nodes by position (counted: ranks for sibling walks).
    pub order: PMap<Pos, Id, usize>,
    pub frames: PMap<FrameId, Frame>,
    /// The application node owning the unit, if any.
    pub app: Id,
    /// The unit's own environment (an application's `set_env`).
    pub env: PMap<Field, ValueId>,
    /// Nodes in work (dirty or held), by position.
    pub work: PMap<Pos, Id, usize>,
    /// Nodes in work without a result that may contribute to an
    /// accumulator (see [`World::range_blocker`]).
    pub acc_open: PMap<Pos, Id>,
    /// Its claimers.
    pub claimers: PMap<Id, ()>,
    /// Claimers by position, each with how far its window reaches
    /// (summarised: the farthest reach in a subtree).
    pub claim_at: PMap<Pos, (Id, Pos), Pos>,
    /// Pending claimers in work that hold later siblings back.
    pub holders: PMap<Pos, Id>,
    /// Nodes in work whose definition set is pending, by what they may
    /// define (any field, a name, a prefix: `pset_key`) and by position.
    pub psets: PMap<u64, PMap<Pos, Id>>,
    /// The same nodes by position only (most reads see none).
    pub pset_nodes: PMap<Pos, Id>,
    /// Entries of `psets` by key bucket (`key % 64`): a key whose bucket is
    /// empty is not looked up.
    pub pset_buckets: Buckets,
}

/// A per-unit position index of [`Unit`].
#[derive(Copy, Clone)]
enum PosIx {
    PsetNodes,
    AccOpen,
    Holders,
}

impl PosIx {
    fn of(self, un: &Unit) -> &PMap<Pos, Id> {
        match self {
            PosIx::PsetNodes => &un.pset_nodes,
            PosIx::AccOpen => &un.acc_open,
            PosIx::Holders => &un.holders,
        }
    }
    fn of_mut(self, un: &mut Unit) -> &mut PMap<Pos, Id> {
        match self {
            PosIx::PsetNodes => &mut un.pset_nodes,
            PosIx::AccOpen => &mut un.acc_open,
            PosIx::Holders => &mut un.holders,
        }
    }
}

/// One definition entry in a field's index.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Entry {
    pub node: Id,
    /// `None`: declared, not yet made by a committed run.
    pub val: Option<ValueId>,
    pub loc: Locality,
}

/// A field's definitions in a unit: by frame and by position, and the
/// globals.
#[derive(Clone, Default)]
pub(crate) struct FieldDefs {
    pub local: PMap<FrameId, PMap<Pos, Entry>>,
    pub globals: PMap<Pos, Entry>,
}

/// One field of one unit: everything keyed by it.
#[derive(Clone, Default)]
pub(crate) struct FieldCell {
    pub defs: FieldDefs,
    /// Contributions to it (an accumulator), folded by its monoid in
    /// every subtree.
    pub contribs: PMap<Pos, ValueId, ValueId>,
    /// Contributors in work: without a result that declare a
    /// contribution to it, and with one that contributed to it.
    pub acc_static: PMap<Pos, Id>,
    pub acc_res: PMap<Pos, Id>,
}

/// Whether node `n` is done and publishes loop-back fields.
fn publishes(n: &Node) -> bool {
    n.st == St::Done && n.res.as_ref().is_some_and(|r| !r.pubs.is_empty())
}

/// Whether a node in state `st` is in work.
pub(crate) fn in_work(st: St) -> bool {
    matches!(st, St::Dirty | St::Held)
}

/// An accumulator's monoid as a tree measure.
pub(crate) struct AccMeasure<'a> {
    pub m: &'a dyn crate::api::Monoid,
    pub v: &'a crate::values::Values,
}

impl Measure<Pos, ValueId> for AccMeasure<'_> {
    type S = ValueId;
    fn zero(&self) -> ValueId {
        self.m.empty(self.v)
    }
    fn of(&self, _k: &Pos, x: &ValueId) -> ValueId {
        *x
    }
    fn add(&self, a: &ValueId, b: &ValueId) -> ValueId {
        self.m.combine(self.v, *a, *b)
    }
}

/// A key whose change a read depends on (validation by version).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Key {
    /// A field's definitions in a unit.
    Defs(u32, Field),
    /// A node's output.
    Out(Id),
    /// A unit's structure (order, frames, kinds).
    Shape(u32),
    Loop(Field),
    Env(Field),
    /// A field's contributions in a unit.
    Acc(u32, Field),
}

/// Counts of entries by key bucket.
#[derive(Clone)]
pub(crate) struct Buckets(Arc<[u32; 64]>);

impl Default for Buckets {
    fn default() -> Self {
        Buckets(Arc::new([0; 64]))
    }
}

/// The state of one round.
#[derive(Clone, Default)]
pub(crate) struct World {
    /// Bumped by every merge.
    pub ver: u64,
    /// The last renumbering's epoch (`live::fresh_epoch`; 0: none yet):
    /// positions taken at another may have moved since.
    pub relabels: u64,
    pub nodes: PMap<Id, Node>,
    pub units: PMap<u32, Arc<Unit>>,
    /// Each field of each unit: its definitions, readers, contributions
    /// and contributors in work (a cell, written as a whole).
    pub fields: PMap<(u32, Field), Arc<FieldCell>>,
    /// The reader indexes (of fields, ranges, outputs, structure), live:
    /// shared by this world's copies (see `live`; a kept world takes a
    /// [`World::snapshot`]).
    pub rd: super::live::Readers,
    /// Loop-back fields' values for this round.
    pub loops: PMap<Field, ValueId>,
    /// The version each key last changed at.
    pub changed: PMap<Key, u64>,
    /// Claimers' state, by claimer.
    pub claims: PMap<Id, super::claims::ClaimSt>,
    /// Nodes whose claim hook is still to be asked (texts made since).
    pub unregistered: PMap<Id, ()>,
    /// Units whose claims are to be resolved again, whole.
    pub claims_dirty: PMap<u32, ()>,
    /// Positions whose change claims that reach them are to see.
    pub claim_points: PMap<(u32, Pos), ()>,
    /// Claimers to resolve again (their own state changed).
    pub claims_todo: PMap<Id, ()>,
    /// Nodes marked at creation by their operation (settled reports).
    pub op_marked: PMap<Id, ()>,
    /// The final nodes ([`crate::api::Operation::final_only`]).
    pub fins: PMap<Id, ()>,
    /// The done nodes that publish loop-back fields.
    pub publishers: PMap<Id, ()>,
    /// Placed nodes whose placer runs again for a change (a read that
    /// broke, a new text) and has not placed them again yet: they wait for
    /// it (their texts may be stale).
    pub awaiting: PMap<Id, ()>,
}

impl World {
    /// Whether `other` is this world unchanged, as a read sees it (every
    /// map a read's answer stands on shares its root): a run on one
    /// reads the same in the other. While `other` is kept, any change
    /// to this one copies the changed map's root.
    pub(crate) fn same_as(&self, other: &World) -> bool {
        self.nodes.ptr_eq(&other.nodes)
            && self.units.ptr_eq(&other.units)
            && self.fields.ptr_eq(&other.fields)
            && self.loops.ptr_eq(&other.loops)
            && self.claims.ptr_eq(&other.claims)
            && self.fins.ptr_eq(&other.fins)
            && self.publishers.ptr_eq(&other.publishers)
    }

    /// Field `f` of unit `u`, if anything is filed under it.
    #[inline]
    pub(crate) fn field(&self, u: u32, f: Field) -> Option<&FieldCell> {
        self.fields.get(&(u, f)).map(|c| &**c)
    }

    /// Changes field `f` of unit `u` (made if new).
    #[inline]
    pub(crate) fn with_field<R>(&mut self, u: u32, f: Field, g: impl FnOnce(&mut FieldCell) -> R) -> R {
        let mut g = Some(g);
        let mut r = None;
        self.fields.upsert((u, f), &NoMeasure, |c| r = Some((g.take().expect("once"))(Arc::make_mut(c))));
        r.expect("ran")
    }

    /// The fields of unit `u` with a cell.
    pub(crate) fn fields_of(&self, u: u32) -> Vec<Field> {
        self.fields
            .range(std::ops::Bound::Included(&(u, Field(ValueId::from_raw(0)))), std::ops::Bound::Unbounded)
            .take_while(|(k, _)| k.0 == u)
            .map(|(k, _)| k.1)
            .collect()
    }

    pub(crate) fn node(&self, id: Id) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub(crate) fn unit(&self, u: u32) -> &Unit {
        self.units.get(&u).expect("a live unit")
    }

    /// Unit `u`, if live.
    #[inline]
    pub(crate) fn unit_opt(&self, u: u32) -> Option<&Unit> {
        self.units.get(&u).map(|c| &**c)
    }

    /// Changes unit `u` (made if new: an add to its indexes).
    #[inline]
    pub(crate) fn with_unit<R>(&mut self, u: u32, g: impl FnOnce(&mut Unit) -> R) -> R {
        let mut g = Some(g);
        let mut r = None;
        self.units.upsert(u, &NoMeasure, |c| r = Some((g.take().expect("once"))(Arc::make_mut(c))));
        r.expect("ran")
    }

    /// Changes unit `u` if it is live (a removal from its indexes: a unit
    /// gone stays gone).
    #[inline]
    pub(crate) fn with_unit_if(&mut self, u: u32, g: impl FnOnce(&mut Unit)) {
        self.units.update(&u, &NoMeasure, |c| g(Arc::make_mut(c)));
    }

    /// This world as it is now, its reader indexes copied: a world kept
    /// (a round's) that later changes to this one do not reach.
    pub(crate) fn snapshot(&self) -> World {
        let mut w = self.clone();
        w.rd = self.rd.snapshot();
        w
    }

    /// Sets (`on`) or clears node `id` at `p` in unit `u`'s index `ix`
    /// (clearing only its own entry); whether it changed.
    fn unit_set(&mut self, u: u32, ix: PosIx, p: Pos, id: Id, on: bool) -> bool {
        let has = self.unit_opt(u).and_then(|un| ix.of(un).get(&p)).copied();
        if on == (has == Some(id)) || (!on && has != Some(id)) {
            return false;
        }
        self.with_unit(u, |un| {
            let m = ix.of_mut(un);
            if on {
                m.insert(p, id, &NoMeasure);
            } else {
                m.remove(&p, &NoMeasure);
            }
        });
        true
    }

    /// Unit `u`'s nodes in work by position.
    #[inline]
    pub(crate) fn work_of(&self, u: u32) -> Option<&PMap<Pos, Id, usize>> {
        self.unit_opt(u).map(|un| &un.work)
    }

    /// The node after `id` in its unit's order.
    pub(crate) fn next(&self, id: Id) -> Option<Id> {
        let n = self.node(id)?;
        self.unit(n.unit).order.above(&n.pos).map(|(_, &x)| x)
    }

    /// The node before `id` in its unit's order.
    pub(crate) fn prev(&self, id: Id) -> Option<Id> {
        let n = self.node(id)?;
        self.unit(n.unit).order.below(&n.pos).map(|(_, &x)| x)
    }

    /// The last node of `id`'s subtree (`id` itself if it has none).
    pub(crate) fn subtree_end(&self, id: Id) -> Id {
        let Some(n) = self.node(id) else { return id };
        let d = n.depth;
        let u = self.unit(n.unit);
        let mut last = id;
        for (_, &x) in u.order.range(std::ops::Bound::Excluded(&n.pos), std::ops::Bound::Unbounded) {
            if self.node(x).is_none_or(|m| m.depth <= d) {
                break;
            }
            last = x;
        }
        last
    }

    /// Whether `a` is before `b` in their unit.
    pub(crate) fn before(&self, a: Id, b: Id) -> bool {
        match (self.node(a), self.node(b)) {
            (Some(x), Some(y)) => x.unit == y.unit && x.pos < y.pos,
            _ => false,
        }
    }

    /// The frame chain from `f` to the root, innermost first.
    pub(crate) fn chain(&self, u: u32, f: FrameId) -> Vec<FrameId> {
        let un = self.unit(u);
        let mut v = vec![f];
        let mut g = f;
        while !g.is_root() {
            let Some(fr) = un.frames.get(&g) else { break };
            g = fr.parent;
            v.push(g);
            if v.len() > 1 << 16 {
                break;
            }
        }
        v
    }

    /// The frame a read at position `pos` (before node `at`, or after its
    /// subtree when `after`) is in.
    pub(crate) fn frame_at(&self, at: Id, after: bool) -> Option<(u32, Pos, FrameId)> {
        let n = self.node(at)?;
        if !after {
            return Some((n.unit, n.pos, n.frame));
        }
        let end = self.subtree_end(at);
        match self.next(end) {
            Some(x) => {
                let m = self.node(x)?;
                Some((m.unit, m.pos, m.frame))
            }
            // After the last node: the frame stack after it.
            None => {
                let e = self.node(end)?;
                Some((e.unit, e.pos + 1, self.frame_after(end)))
            }
        }
    }

    /// The innermost frame open after node `id` (its brackets after it
    /// applied).
    pub(crate) fn frame_after(&self, id: Id) -> FrameId {
        let Some(n) = self.node(id) else {
            return FrameId::root(0);
        };
        let b = n.brackets();
        let un = self.unit(n.unit);
        let mut f = n.frame;
        let (ca, oa) = if b.before { (0, 0) } else { (b.closes, b.opens) };
        for _ in 0..ca {
            if f.is_root() {
                break;
            }
            f = un.frames.get(&f).map_or(f, |fr| fr.parent);
        }
        if oa > 0 {
            let ob = if b.before { b.opens } else { 0 };
            f = FrameId::opened(id, ob + oa - 1);
        }
        f
    }

    /// The definition of `f` reaching position `pos` in frame `fr` of unit
    /// `u`: the nearest earlier entry in a frame of its chain, or the
    /// nearest earlier global. `skip` passes a node's own entries.
    pub(crate) fn reaching(&self, u: u32, f: Field, pos: Pos, fr: FrameId, skip: Id) -> Option<(Pos, Entry)> {
        let fd = &self.field(u, f)?.defs;
        let mut best: Option<(Pos, Entry)> = None;
        let take = |best: &mut Option<(Pos, Entry)>, m: &PMap<Pos, Entry>| {
            let mut k = pos;
            while let Some((&p, e)) = m.below(&k) {
                if best.is_some_and(|b| b.0 >= p) {
                    return;
                }
                if e.node != skip {
                    *best = Some((p, *e));
                    return;
                }
                k = p;
            }
        };
        // The chain, innermost first, walked in place: an enclosing frame's
        // entries before the read lie at or before the inner frame's
        // opener, so past an entry after it the walk stops.
        let un = self.unit(u);
        let mut g = fr;
        for _ in 0..=1 << 16 {
            if let Some(m) = fd.local.get(&g) {
                take(&mut best, m);
            }
            if g.is_root() {
                break;
            }
            let Some(frm) = un.frames.get(&g) else { break };
            if best.is_some_and(|b| b.0 > frm.start) {
                break;
            }
            g = frm.parent;
        }
        take(&mut best, &fd.globals);
        best
    }

    /// The window of a range read by `me` from `from` up to `to`: the
    /// positions strictly between which contributions fold.
    pub(crate) fn window(&self, me: Id, from: Id, to: Id, past: bool) -> Option<(u32, Pos, Pos)> {
        let n = self.node(me)?;
        let u = n.unit;
        let lo = self
            .node(from)
            .filter(|a| a.unit == u && from != NONE)
            .map_or(0, |a| a.pos);
        // (Past the reader with no end: to the unit's end.)
        let mut hi = if past && to == NONE { Pos::MAX } else { n.pos };
        if let Some(t) = self.node(to).filter(|t| t.unit == u && to != NONE)
            && (past || t.pos < hi)
        {
            hi = t.pos + 1;
        }
        Some((u, lo, hi))
    }

    /// The scope kind at node `id`: its innermost frame with a kind.
    /// The environment's value of `f` for a read in unit `u` that no
    /// definition reaches: the unit's own environment, then (outside an
    /// application) the engine's; never the provider.
    pub(crate) fn env_of(&self, core: &super::core::Core, u: u32, f: Field) -> Option<ValueId> {
        if let Some(un) = self.units.get(&u) {
            if let Some(&x) = un.env.get(&f) {
                return Some(x);
            }
            if un.app != NONE {
                return None;
            }
        }
        core.env_known(f).flatten()
    }

    pub(crate) fn kind_at(&self, id: Id) -> Option<ValueId> {
        let n = self.node(id)?;
        let un = self.unit(n.unit);
        // A node deciding its own effect reads the scope it enters, before
        // its own brackets.
        let g = if n.text.stat.effect.is_none() && !matches!(n.place, Place::Marker(_)) {
            self.prev(id).map_or(FrameId::root(n.unit), |p| self.frame_after(p))
        } else {
            n.frame
        };
        // (The innermost frame's own kind: kinds are not inherited.)
        un.frames.get(&g)?.kind
    }

    /// Marks `k` changed at this world's version.
    pub(crate) fn touch_key(&mut self, k: Key) {
        let v = self.ver;
        self.changed.insert(k, v, &NoMeasure);
    }

    /// The version `k` last changed at (0: never).
    pub(crate) fn key_ver(&self, k: &Key) -> u64 {
        self.changed.get(k).copied().unwrap_or(0)
    }

    /// Adds a node to its unit's order at `pos`.
    pub(crate) fn link(&mut self, u: u32, pos: Pos, id: Id) {
        self.with_unit(u, |un| {
            un.order.insert(pos, id, &Count);
        });
    }

    /// Removes a node from its unit's order.
    pub(crate) fn unlink(&mut self, u: u32, pos: Pos) {
        self.with_unit_if(u, |un| {
            un.order.remove(&pos, &Count);
        });
    }

    /// Files a definition entry of node `id` (at `pos`, in frame `fr` for
    /// a local).
    pub(crate) fn file_def(&mut self, u: u32, f: Field, pos: Pos, fr: FrameId, e: Entry) {
        self.with_field(u, f, |c| {
            let fd = &mut c.defs;
            if e.loc == Locality::Global {
                fd.globals.insert(pos, e, &NoMeasure);
            } else {
                fd.local.upsert(fr, &NoMeasure, |m| {
                    m.insert(pos, e, &NoMeasure);
                });
            }
        });
    }

    /// Removes a definition entry.
    pub(crate) fn unfile_def(&mut self, u: u32, f: Field, pos: Pos, fr: FrameId, global: bool) {
        if self.field(u, f).is_none() {
            return;
        }
        self.with_field(u, f, |c| {
            let fd = &mut c.defs;
            if global {
                fd.globals.remove(&pos, &NoMeasure);
            } else {
                let mut empty = false;
                fd.local.update(&fr, &NoMeasure, |m| {
                    m.remove(&pos, &NoMeasure);
                    empty = m.is_empty();
                });
                if empty {
                    fd.local.remove(&fr, &NoMeasure);
                }
            }
        });
    }

    /// Adds a reader of `f` at `pos`.
    pub(crate) fn add_reader(&self, u: u32, f: Field, pos: Pos, id: Id) {
        self.rd.with_field(u, f, |c| {
            c.readers.insert((pos, id), (), &NoMeasure);
        });
    }

    pub(crate) fn drop_reader(&self, u: u32, f: Field, pos: Pos, id: Id) {
        self.rd.with_field(u, f, |c| {
            c.readers.remove(&(pos, id), &NoMeasure);
        });
    }

    /// The readers of `f` in unit `u` reading at a position in `(lo, hi)`.
    pub(crate) fn readers_between(&self, u: u32, f: Field, lo: Pos, hi: Pos) -> Vec<Id> {
        let Some(c) = self.rd.field(u, f) else {
            return Vec::new();
        };
        c.readers.range(
            std::ops::Bound::Excluded(&(lo, Id::MAX)),
            std::ops::Bound::Excluded(&(hi, 0)),
        )
        .map(|(k, ())| k.1)
        .collect()
    }

    pub(crate) fn add_out_reader(&self, of: Id, id: Id) {
        self.rd.add_out(of, id);
    }

    pub(crate) fn drop_out_reader(&self, of: Id, id: Id) {
        self.rd.drop_out(of, id);
    }

    /// The readers of node `of`'s output.
    pub(crate) fn out_readers_of(&self, of: Id) -> Vec<Id> {
        self.rd.outs(of).map_or_else(Vec::new, |m| m.iter().map(|(&r, ())| r).collect())
    }

    pub(crate) fn add_kind_reader(&self, u: u32, p: Pos, id: Id) {
        self.rd.with_unit(u, |un| {
            un.kind_readers.insert((p, id), (), &NoMeasure);
        });
    }

    pub(crate) fn drop_kind_reader(&self, u: u32, p: Pos, id: Id) {
        self.rd.with_unit(u, |un| {
            un.kind_readers.remove(&(p, id), &NoMeasure);
        });
    }

    /// The scope readers of unit `u` reading at a position in `[lo, hi]`.
    pub(crate) fn kind_readers_in(&self, u: u32, lo: Pos, hi: Pos) -> Vec<Id> {
        self.rd.unit(u).map_or_else(Vec::new, |un| {
            un.kind_readers
                .range(std::ops::Bound::Included(&(lo, 0)), std::ops::Bound::Included(&(hi, Id::MAX)))
                .map(|(&(_, r), ())| r)
                .collect()
        })
    }

    /// The sibling readers of unit `u` whose windows hold position `p`.
    pub(crate) fn window_readers_at(&self, u: u32, p: Pos) -> Vec<Id> {
        let Some(un) = self.rd.unit(u) else { return Vec::new() };
        let mut hit: Vec<Id> = super::claims::stab_forward(&un.fwd_readers, p).into_iter().map(|e| e.1).collect();
        hit.extend(super::claims::stab_backward(&un.back_readers, p).into_iter().map(|e| e.1));
        hit
    }

    /// Files reader `id` at `p` in a window index (`back`: backward) with
    /// its reach; `None` takes it out.
    pub(crate) fn set_window(&self, back: bool, u: u32, p: Pos, id: Id, reach: Option<Pos>) {
        if reach.is_none() && self.window_of(back, u, p, id).is_none() {
            return;
        }
        // (In place: a copy of the unit's windows would copy every path.)
        self.rd.with_unit(u, |un| match reach {
            Some(r) => {
                let m = if back { &mut un.back_readers } else { &mut un.fwd_readers };
                if back {
                    m.insert(p, (id, r), &super::claims::MinLow);
                } else {
                    m.insert(p, (id, r), &super::claims::MaxReach);
                }
            }
            None => {
                let m = if back { &mut un.back_readers } else { &mut un.fwd_readers };
                if m.get(&p).is_some_and(|e| e.0 == id) {
                    if back {
                        m.remove(&p, &super::claims::MinLow);
                    } else {
                        m.remove(&p, &super::claims::MaxReach);
                    }
                }
            }
        });
    }

    /// Node `id`'s entry in a window index at `p` (`back`: backward), if
    /// it holds it.
    pub(crate) fn window_of(&self, back: bool, u: u32, p: Pos, id: Id) -> Option<Pos> {
        let un = self.rd.unit(u)?;
        if back { &un.back_readers } else { &un.fwd_readers }.get(&p).filter(|e| e.0 == id).map(|e| e.1)
    }

    /// Whether node `id` is a kind (scope) reader at `p`.
    pub(crate) fn is_kind_reader(&self, u: u32, p: Pos, id: Id) -> bool {
        self.rd.unit(u).is_some_and(|un| un.kind_readers.contains_key(&(p, id)))
    }

    /// Replaces a node's record (the in-work index follows its state).
    pub(crate) fn put(&mut self, id: Id, n: Node) {
        let old = self.nodes.get(&id).map(|o| (o.unit, o.pos));
        let moved = old != Some((n.unit, n.pos));
        let was = self.nodes.get(&id).map(|o| (o.unit, o.pos, pset_keys(o)));
        let now = (n.unit, n.pos, pset_keys(&n));
        self.put_node(id, n);
        if was.as_ref() != Some(&now) {
            if let Some((u, p, ks)) = was {
                if !ks.is_empty() {
                    self.unit_set(u, PosIx::PsetNodes, p, id, false);
                }
                for k in ks {
                    self.pset_set(u, k, p, id, false);
                }
            }
            if !now.2.is_empty() {
                self.unit_set(now.0, PosIx::PsetNodes, now.1, id, true);
            }
            for &k in &now.2 {
                self.pset_set(now.0, k, now.1, id, true);
            }
        }
        if let Some((u, p)) = old {
            self.unhold(u, p, id);
            if moved {
                self.unclaim_at(u, p, id);
            }
        }
        self.refresh_holder(id);
        if moved {
            self.refresh_claim_at(id);
        }
    }

    fn unclaim_at(&mut self, u: u32, p: Pos, id: Id) {
        if self.unit_opt(u).is_some_and(|un| un.claim_at.get(&p).is_some_and(|e| e.0 == id)) {
            self.with_unit_if(u, |un| {
                un.claim_at.remove(&p, &super::claims::MaxReach);
            });
        }
    }

    /// Files claimer `id` in the claim index at its position with its reach
    /// (or takes it out when it claims nothing).
    pub(crate) fn refresh_claim_at(&mut self, id: Id) {
        let Some(n) = self.nodes.get(&id) else { return };
        let (u, p) = (n.unit, n.pos);
        match self.claims.get(&id) {
            Some(st) => {
                let reach = st.reach;
                if self.unit_opt(u).is_none_or(|un| un.claim_at.get(&p) != Some(&(id, reach))) {
                    self.with_unit(u, |un| {
                        un.claim_at.insert(p, (id, reach), &super::claims::MaxReach);
                    });
                }
            }
            None => self.unclaim_at(u, p, id),
        }
    }

    /// Whether node `id` is a pending claimer in work holding siblings.
    fn is_holder(&self, id: Id, n: &Node) -> bool {
        in_work(n.st)
            && self.claims.get(&id).is_some_and(|st| {
                st.decl == crate::api::Claim::Pending && st.run.is_none() && st.early != Some(0)
            })
    }

    fn unhold(&mut self, u: u32, p: Pos, id: Id) {
        self.unit_set(u, PosIx::Holders, p, id, false);
    }

    /// Files node `id` in the holders index if it holds (after its node or
    /// claim state changed).
    pub(crate) fn refresh_holder(&mut self, id: Id) {
        let Some(n) = self.nodes.get(&id) else { return };
        let (u, p) = (n.unit, n.pos);
        let on = self.is_holder(id, n);
        self.unit_set(u, PosIx::Holders, p, id, on);
    }

    fn put_node(&mut self, id: Id, n: Node) {
        let (fin, publ) = (n.text.stat.fin, publishes(&n));
        if self.nodes.get(&id).is_none_or(|o| o.text.stat.fin != fin) {
            if fin {
                self.fins.insert(id, (), &NoMeasure);
            } else {
                self.fins.remove(&id, &NoMeasure);
            }
        }
        if self.nodes.get(&id).is_none_or(|o| publishes(o) != publ) {
            if publ {
                self.publishers.insert(id, (), &NoMeasure);
            } else {
                self.publishers.remove(&id, &NoMeasure);
            }
        }
        let was = self.nodes.get(&id).map(|o| (o.unit, o.pos, in_work(o.st)));
        let now = (n.unit, n.pos, in_work(n.st));
        if was != Some(now) {
            if let Some((u, p, true)) = was {
                self.set_work(u, p, None);
            }
            if now.2 {
                self.set_work(now.0, now.1, Some(id));
            }
        }
        let old = self.nodes.get(&id).filter(|o| in_work(o.st)).cloned();
        if (old.is_some() || in_work(n.st))
            && !old.as_ref().is_some_and(|o| {
                in_work(n.st)
                    && (o.unit, o.pos) == (n.unit, n.pos)
                    && Arc::ptr_eq(&o.text, &n.text)
                    && match (&o.res, &n.res) {
                        (None, None) => true,
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a.contribs.iter().map(|c| c.0).eq(b.contribs.iter().map(|c| c.0)),
                        _ => false,
                    }
            })
        {
            if let Some(o) = &old {
                self.acc_index(id, o, false);
            }
            self.acc_index(id, &n, true);
        }
        self.nodes.insert(id, n, &NoMeasure);
    }

    /// Removes a node's record.
    pub(crate) fn drop_node(&mut self, id: Id) {
        if let Some(o) = self.nodes.get(&id) {
            let (u, p, w, ks) = (o.unit, o.pos, in_work(o.st), pset_keys(o));
            let o = w.then(|| o.clone());
            if let Some(o) = o {
                self.set_work(u, p, None);
                self.acc_index(id, &o, false);
            }
            self.unhold(u, p, id);
            self.unclaim_at(u, p, id);
            if !ks.is_empty() {
                self.unit_set(u, PosIx::PsetNodes, p, id, false);
            }
            for k in ks {
                self.pset_set(u, k, p, id, false);
            }
        }
        self.nodes.remove(&id, &NoMeasure);
        self.op_marked.remove(&id, &NoMeasure);
        self.fins.remove(&id, &NoMeasure);
        self.publishers.remove(&id, &NoMeasure);
    }

    /// The final nodes in state `st` (from the index, not the whole world).
    pub(crate) fn fins_in(&self, st: St) -> Vec<Id> {
        self.fins.iter().filter(|(id, _)| self.node(**id).is_some_and(|n| n.st == st)).map(|(&id, _)| id).collect()
    }

    /// The dirty nodes (from the in-work index).
    pub(crate) fn dirty_nodes(&self) -> Vec<Id> {
        self.units.iter().flat_map(|(_, un)| un.work.iter().map(|(_, &id)| id)).filter(|&id| self.node(id).is_some_and(|n| n.st == St::Dirty)).collect()
    }

    fn set_work(&mut self, u: u32, p: Pos, id: Option<Id>) {
        match id {
            Some(x) => self.with_unit(u, |un| {
                un.work.insert(p, x, &Count);
            }),
            None => self.with_unit_if(u, |un| {
                un.work.remove(&p, &Count);
            }),
        }
    }

    /// Files node `n` (`on`) in the accumulator indexes, or takes it out.
    fn acc_index(&mut self, id: Id, n: &Node, on: bool) {
        // (Declaring no contributions at all, or all of them, is
        // contributing nowhere else.)
        let stat = n.text.stat.contribs.as_deref();
        if !in_work(n.st) || stat.is_some_and(|cs| cs.is_empty() || n.text.stat.contribs_complete) {
            return;
        }
        let (u, p) = (n.unit, n.pos);
        match &n.res {
            None => {
                self.unit_set(u, PosIx::AccOpen, p, id, on);
                for &(f, _) in stat.unwrap_or_default() {
                    self.field_index_set(u, f, false, p, id, on);
                }
            }
            Some(r) => {
                for &(f, _) in &r.contribs {
                    if !stat.is_some_and(|cs| cs.iter().any(|c| c.0 == f)) {
                        self.field_index_set(u, f, true, p, id, on);
                    }
                }
            }
        }
    }

    /// A node in the window `(lo, hi)` of unit `u` still in work that may
    /// contribute to `f`: the first, other than `me`. One that declares
    /// its contributions to `f` (or none at all), or whose last run made
    /// none to `f`, is folded as it stands; its change re-asks the reader.
    pub(crate) fn range_blocker(&self, me: Id, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<Id> {
        let mut lo = lo;
        loop {
            let open = self.first_open(f, u, lo, hi);
            let res = self
                .field(u, f)
                .map(|c| &c.acc_res)
                .and_then(|m| m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi)).next())
                .map(|(&p, &x)| (p, x));
            let x = match (open, res) {
                (Some(a), Some(b)) => a.min(b),
                (a, b) => a.or(b)?,
            };
            if x.1 != me {
                return Some(x.1);
            }
            lo = x.0;
        }
    }

    /// The last such blocker in `(lo, hi)` before `hi` (other than `me`):
    /// the one a reader parked on is woken by last, when commits come in
    /// text order.
    pub(crate) fn range_blocker_last(&self, me: Id, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<Id> {
        let mut hi = hi;
        loop {
            let open = self.last_open(f, u, lo, hi);
            let res = self
                .field(u, f)
                .map(|c| &c.acc_res)
                .and_then(|m| m.below(&hi))
                .filter(|(p, _)| **p > lo)
                .map(|(&p, &x)| (p, x));
            let x = match (open, res) {
                (Some(a), Some(b)) => a.max(b),
                (a, b) => a.or(b)?,
            };
            if x.1 != me {
                return Some(x.1);
            }
            hi = x.0;
        }
    }

    /// The last node without a result in work in `(lo, hi)` of unit `u`
    /// that does not declare its contribution to `f` (as [`Self::first_open`],
    /// from the end).
    fn last_open(&self, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<(Pos, Id)> {
        let open = &self.unit_opt(u)?.acc_open;
        let a = open.rank(&lo) + usize::from(open.contains_key(&lo));
        let b = open.rank(&hi);
        if a >= b {
            return None;
        }
        let Some(st) = self.field(u, f).map(|c| &c.acc_static).filter(|m| !m.is_empty()) else {
            return open.nth(b - 1).map(|(&p, &x)| (p, x));
        };
        let s1 = st.rank(&hi);
        // (The last k+1 open nodes all declare: as many declaring ones
        // from the k-th last on.)
        let all_declare = |k: usize| -> bool {
            let (&p, _) = open.nth(b - 1 - k).expect("in range");
            s1 - st.rank(&p) == k + 1
        };
        let (mut l, mut r) = (0, b - a);
        while l < r {
            let mid = l + (r - l) / 2;
            if all_declare(mid) {
                l = mid + 1;
            } else {
                r = mid;
            }
        }
        (l < b - a).then(|| open.nth(b - 1 - l).map(|(&p, &x)| (p, x))).flatten()
    }

    /// The first node without a result in work in `(lo, hi)` of unit `u`
    /// that does not declare its contribution to `f`: those that do are a
    /// prefix-closed subset, so the first not among them is found by
    /// bisection on counts (O(log² n)).
    fn first_open(&self, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<(Pos, Id)> {
        let open = &self.unit_opt(u)?.acc_open;
        let a = open.rank(&lo) + usize::from(open.contains_key(&lo));
        let b = open.rank(&hi);
        if a >= b {
            return None;
        }
        let Some(st) = self.field(u, f).map(|c| &c.acc_static).filter(|m| !m.is_empty()) else {
            return open.nth(a).map(|(&p, &x)| (p, x));
        };
        let s0 = st.rank(&lo) + usize::from(st.contains_key(&lo));
        // (The first k+1 open nodes all declare: as many declaring ones up
        // to the k-th.)
        let all_declare = |k: usize| -> bool {
            let (&p, _) = open.nth(a + k).expect("in range");
            st.rank(&p) + usize::from(st.contains_key(&p)) - s0 == k + 1
        };
        let (mut l, mut r) = (0, b - a);
        while l < r {
            let mid = l + (r - l) / 2;
            if all_declare(mid) {
                l = mid + 1;
            } else {
                r = mid;
            }
        }
        (l < b - a).then(|| open.nth(a + l).map(|(&p, &x)| (p, x))).flatten()
    }

    /// Whether node `id` is ready by its read hints: no hinted field is
    /// defined before it by a node still in work, nor past a pending set.
    /// Not ready: the node it waits on, when one in work is known.
    pub(crate) fn hint_blocker(&self, core: &super::core::Core, id: Id, predict: bool) -> Result<(), Option<Id>> {
        let Some(n) = self.node(id) else { return Ok(()) };
        let Some(fs) = n.text.stat.reads.as_ref() else { return Ok(()) };
        let Some((u, pos, fr)) = self.frame_at(id, false) else { return Ok(()) };
        let working = |x: Id| self.node(x).is_some_and(|d| in_work(d.st)).then_some(x);
        for &f in fs.iter() {
            if self.loops.get(&f).is_some() {
                continue;
            }
            // An accumulator: held while a node before it in work may
            // still contribute (it would stop on the fold otherwise).
            if let Some(m) = core.acc(f) {
                // (A guessed fold does not wait: `Monoid::speculate`.)
                if !(predict && m.speculate())
                    && let Some(x) = self.range_blocker_last(id, f, u, 0, pos)
                {
                    return Err(Some(x));
                }
                continue;
            }
            let reach = self.reaching(u, f, pos, fr, id);
            if let Some((_, e)) = reach {
                let busy = e.val.is_none() || self.node(e.node).is_some_and(|d| in_work(d.st));
                // (A definer in work that predicts the field: its
                // prediction stands in, when predictions are used.)
                let predicted = predict
                    && self
                        .node(e.node)
                        .and_then(|d| d.text.stat.preds.as_ref())
                        .is_some_and(|ps| ps.iter().any(|p| p.0 == f));
                if busy && !predicted {
                    return Err(working(e.node));
                }
            }
            if !predict && let Some(x) = self.pending_set_between(core, u, f, reach.map_or(0, |r| r.0), pos, id) {
                return Err(working(x));
            }
        }
        Ok(())
    }

    /// The first node in work strictly between `lo` and `hi` in unit `u`
    /// whose definition set is still pending and may hold `f` (a reader
    /// past it waits: its answer is not known yet).
    /// Sets (`on`) or clears node `id`'s pending-set entry under key `k`.
    fn pset_set(&mut self, u: u32, k: u64, p: Pos, id: Id, on: bool) {
        // (An entry filed over another node's at `p` takes its place: the
        // count stays.)
        let has = self.unit_opt(u).and_then(|un| un.psets.get(&k)).and_then(|m| m.get(&p)).copied();
        let other = on && has.is_some_and(|x| x != id);
        if on == (has == Some(id)) || (!on && has != Some(id)) {
            return;
        }
        self.with_unit(u, |un| {
            un.psets.upsert(k, &NoMeasure, |m| {
                if on {
                    m.insert(p, id, &NoMeasure);
                } else {
                    m.remove(&p, &NoMeasure);
                }
            });
            if !other {
                let b = Arc::make_mut(&mut un.pset_buckets.0);
                let i = (k % 64) as usize;
                b[i] = if on { b[i] + 1 } else { b[i] - 1 };
            }
        });
    }

    pub(crate) fn pending_set_between(&self, core: &super::core::Core, u: u32, f: Field, lo: Pos, hi: Pos, me: Id) -> Option<Id> {
        // (Usually none in the window at all; when few, each is asked
        // directly, the first that may hold `f` in text order.)
        let pending = |x: Id| {
            self.node(x).is_some_and(|n| n.text.stat.defs.is_none() && (n.wide || core.may_cover(n.may(), n.text.op, &n.text.args, f)))
        };
        let un = self.unit_opt(u)?;
        let m = &un.pset_nodes;
        let mut few: [Id; 8] = [NONE; 8];
        let mut k = 0;
        for (_, &x) in m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi)) {
            if x == me {
                continue;
            }
            if k == few.len() {
                k += 1;
                break;
            }
            few[k] = x;
            k += 1;
        }
        if k == 0 {
            return None;
        }
        if k <= few.len() {
            return few[..k].iter().copied().find(|&x| pending(x));
        }
        // The sets that may hold `f`: any field, its name, its prefixes
        // (rolled over the name in one pass).
        let mut best: Option<(Pos, Id)> = None;
        let buckets = &un.pset_buckets;
        let mut look = |k: u64| {
            if buckets.0[(k % 64) as usize] == 0 {
                return;
            }
            let Some(m) = un.psets.get(&k) else { return };
            let upto = best.map_or(hi, |b| b.0);
            let hit = m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&upto)).find(|&(_, &x)| {
                x != me
                    && self.node(x).is_some_and(|n| {
                        n.text.stat.defs.is_none() && (n.wide || core.may_cover(n.may(), n.text.op, &n.text.args, f))
                    })
            });
            if let Some((&p, &x)) = hit {
                best = Some((p, x));
            }
        };
        look(0);
        look(name_key(f));
        core.values.with_str(f.0, |name| {
            let mut h = PREFIX_SEED;
            for (i, b) in name.bytes().enumerate() {
                h = prefix_step(h, b);
                if name.is_char_boundary(i + 1) {
                    look(h | 1);
                }
            }
        });
        best.map(|b| b.1)
    }

    /// Sets (or with `None` removes) node `id`'s contribution to `f` at
    /// `pos`.
    pub(crate) fn set_contrib(&mut self, m: &AccMeasure<'_>, u: u32, f: Field, pos: Pos, x: Option<ValueId>) {
        self.with_field(u, f, |c| match x {
            Some(v) => {
                c.contribs.insert(pos, v, m);
            }
            None => {
                c.contribs.remove(&pos, m);
            }
        });
    }

    /// The fold of `f`'s contributions in unit `u` strictly between `lo`
    /// and `hi`.
    pub(crate) fn fold(&self, m: &AccMeasure<'_>, u: u32, f: Field, lo: Pos, hi: Pos) -> ValueId {
        match self.field(u, f).map(|c| &c.contribs) {
            Some(t) => t.fold(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi), m),
            None => m.zero(),
        }
    }

    pub(crate) fn add_range_reader(&self, u: u32, f: Field, lo: Pos, hi: Pos, id: Id) {
        self.rd.with_field(u, f, |c| {
            c.range_readers.insert((hi, id), lo, &NoMeasure);
        });
    }

    pub(crate) fn drop_range_reader(&self, u: u32, f: Field, hi: Pos, id: Id) {
        self.rd.with_field(u, f, |c| {
            c.range_readers.remove(&(hi, id), &NoMeasure);
        });
    }

    /// The range readers of `f` whose window holds position `q`.
    pub(crate) fn range_readers_over(&self, u: u32, f: Field, q: Pos) -> Vec<Id> {
        let Some(c) = self.rd.field(u, f) else {
            return Vec::new();
        };
        c.range_readers.range(std::ops::Bound::Excluded(&(q, Id::MAX)), std::ops::Bound::Unbounded)
            .filter(|(_, lo)| **lo < q)
            .map(|(k, _)| k.1)
            .collect()
    }
}

impl World {
    /// [`index_set`] on field `f`'s contributors in work (`res`: those with
    /// a result, else those declaring).
    fn field_index_set(&mut self, u: u32, f: Field, res: bool, p: Pos, id: Id, on: bool) -> bool {
        let has = self.field(u, f).and_then(|c| if res { &c.acc_res } else { &c.acc_static }.get(&p)).copied();
        if on == (has == Some(id)) || (!on && has != Some(id)) {
            return false;
        }
        self.with_field(u, f, |c| {
            let m = if res { &mut c.acc_res } else { &mut c.acc_static };
            if on {
                m.insert(p, id, &NoMeasure);
            } else {
                m.remove(&p, &NoMeasure);
            }
        });
        true
    }
}


/// The pending-set index keys of node `n`: none unless it is in work with
/// its definition set pending; 0 for any field; else a key per name or
/// prefix it may define.
fn pset_keys(n: &Node) -> Vec<u64> {
    if !in_work(n.st) || n.text.stat.defs.is_some() {
        return Vec::new();
    }
    if n.wide {
        return vec![0];
    }
    match n.may() {
        May::Nothing => Vec::new(),
        May::Any => vec![0],
        May::Names(fs) => fs.iter().map(|&f| name_key(f)).collect(),
        May::Prefixes(ps) => ps.iter().map(|p| prefix_key(p)).collect(),
    }
}

fn name_key(f: Field) -> u64 {
    crate::values::hash_words(0x6e61, &[f.0.raw()]) | 1
}

const PREFIX_SEED: u64 = 0xcbf2_9ce4_8422_2325 ^ 0x7072;

fn prefix_step(h: u64, b: u8) -> u64 {
    (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
}

/// A prefix's key: rolled byte by byte, so a name's prefixes take one pass.
fn prefix_key(p: &str) -> u64 {
    p.bytes().fold(PREFIX_SEED, prefix_step) | 1
}
