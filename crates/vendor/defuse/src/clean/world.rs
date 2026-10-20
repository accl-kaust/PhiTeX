//! A world: the complete state of one round, immutable and persistent
//! (`docs/clean-core.md`). A new world is made from an old one by cloning it
//! (every map is an `Arc` clone) and mutating the clone: only the paths that
//! change are copied.

use std::sync::Arc;

use super::ptree::{Count, Measure, NoMeasure, PMap};
use crate::api::{Brackets, Effect, Locality, MayDefine, OpId};
use crate::values::{Field, ValueId};

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
    /// the unit's start) up to `to` (`NONE`: the reader).
    Range { f: Field, from: Id, to: Id, val: ValueId },
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

/// A unit's order and frames.
#[derive(Clone, Default)]
pub(crate) struct Unit {
    /// The nodes by position (counted: ranks for sibling walks).
    pub order: PMap<Pos, Id, usize>,
    pub frames: PMap<FrameId, Frame>,
    /// The application node owning the unit, if any.
    pub app: Id,
    /// The unit's own environment (an application's `set_env`).
    pub env: PMap<Field, ValueId>,
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
    pub nodes: PMap<Id, Node>,
    pub units: PMap<u32, Unit>,
    pub defs: PMap<(u32, Field), FieldDefs>,
    /// Readers of a field, by the position they read at.
    pub readers: PMap<(u32, Field), PMap<(Pos, Id), ()>>,
    /// Readers of a node's output.
    pub out_readers: PMap<Id, PMap<Id, ()>>,
    /// Readers of the scope structure at their position (kinds, sequence
    /// frames), by unit.
    pub kind_readers: PMap<u32, PMap<(Pos, Id), ()>>,
    /// Readers of following siblings (`following`, `claimed`, streams), by
    /// unit and position, each with how far its window reaches.
    pub fwd_readers: PMap<u32, super::claims::Windows>,
    /// Readers of earlier siblings (`prev_sibling_where`), by unit and
    /// position, each with the lowest position its walk reached.
    pub back_readers: PMap<u32, super::claims::Windows>,
    /// Contributions to accumulators, by unit and field, folded by the
    /// field's monoid in every subtree.
    pub contribs: PMap<(u32, Field), PMap<Pos, ValueId, ValueId>>,
    /// Range readers of an accumulator: by the end of their window and the
    /// reader, the window's start.
    pub range_readers: PMap<(u32, Field), PMap<(Pos, Id), Pos>>,
    /// Nodes in work (dirty or held), by unit and position.
    pub work: PMap<u32, PMap<Pos, Id, usize>>,
    /// Loop-back fields' values for this round.
    pub loops: PMap<Field, ValueId>,
    /// The version each key last changed at.
    pub changed: PMap<Key, u64>,
    /// Claimers' state, by claimer.
    pub claims: PMap<Id, super::claims::ClaimSt>,
    /// The claimers of each unit.
    pub claimers: PMap<u32, PMap<Id, ()>>,
    /// Nodes whose claim hook is still to be asked (texts made since).
    pub unregistered: PMap<Id, ()>,
    /// Units whose claims are to be resolved again, whole.
    pub claims_dirty: PMap<u32, ()>,
    /// Positions whose change claims that reach them are to see.
    pub claim_points: PMap<(u32, Pos), ()>,
    /// Claimers to resolve again (their own state changed).
    pub claims_todo: PMap<Id, ()>,
    /// Claimers by unit and position, each with how far its window
    /// reaches (summarised: the farthest reach in a subtree).
    pub claim_at: PMap<u32, PMap<Pos, (Id, Pos), Pos>>,
    /// Pending claimers in work that hold later siblings back, by unit and
    /// position.
    pub holders: PMap<u32, PMap<Pos, Id>>,
    /// Nodes in work whose definition set is pending, by unit, by what
    /// they may define (any field, a name, a prefix: `pset_key`) and by
    /// position.
    pub psets: PMap<(u32, u64), PMap<Pos, Id>>,
    /// The same nodes by unit and position only (most reads see none).
    pub pset_nodes: PMap<u32, PMap<Pos, Id>>,
    /// Entries of `psets` by unit and key bucket (`key % 64`): a key whose
    /// bucket is empty is not looked up.
    pub pset_buckets: PMap<u32, Buckets>,
    /// Nodes marked at creation by their operation (settled reports).
    pub op_marked: PMap<Id, ()>,
}

impl World {
    pub(crate) fn node(&self, id: Id) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub(crate) fn unit(&self, u: u32) -> &Unit {
        self.units.get(&u).expect("a live unit")
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
        let fd = self.defs.get(&(u, f))?;
        let mut best: Option<(Pos, Entry)> = None;
        let mut take = |m: &PMap<Pos, Entry>| {
            let mut k = pos;
            while let Some((&p, e)) = m.below(&k) {
                if best.is_some_and(|b| b.0 >= p) {
                    return;
                }
                if e.node != skip {
                    best = Some((p, *e));
                    return;
                }
                k = p;
            }
        };
        for g in self.chain(u, fr) {
            if let Some(m) = fd.local.get(&g) {
                take(m);
            }
        }
        take(&fd.globals);
        best
    }

    /// The window of a range read by `me` from `from` up to `to`: the
    /// positions strictly between which contributions fold.
    pub(crate) fn window(&self, me: Id, from: Id, to: Id) -> Option<(u32, Pos, Pos)> {
        let n = self.node(me)?;
        let u = n.unit;
        let lo = self
            .node(from)
            .filter(|a| a.unit == u && from != NONE)
            .map_or(0, |a| a.pos);
        let mut hi = n.pos;
        if let Some(t) = self.node(to).filter(|t| t.unit == u && to != NONE)
            && t.pos < hi
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
        let mut un = self.units.get(&u).cloned().unwrap_or_default();
        un.order.insert(pos, id, &Count);
        self.units.insert(u, un, &NoMeasure);
    }

    /// Removes a node from its unit's order.
    pub(crate) fn unlink(&mut self, u: u32, pos: Pos) {
        self.units.update(&u, &NoMeasure, |un| {
            un.order.remove(&pos, &Count);
        });
    }

    /// Files a definition entry of node `id` (at `pos`, in frame `fr` for
    /// a local).
    pub(crate) fn file_def(&mut self, u: u32, f: Field, pos: Pos, fr: FrameId, e: Entry) {
        self.defs.upsert((u, f), &NoMeasure, |fd| {
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
        self.defs.update(&(u, f), &NoMeasure, |fd| {
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
    pub(crate) fn add_reader(&mut self, u: u32, f: Field, pos: Pos, id: Id) {
        self.readers.upsert((u, f), &NoMeasure, |m| {
            m.insert((pos, id), (), &NoMeasure);
        });
    }

    pub(crate) fn drop_reader(&mut self, u: u32, f: Field, pos: Pos, id: Id) {
        self.readers.update(&(u, f), &NoMeasure, |m| {
            m.remove(&(pos, id), &NoMeasure);
        });
    }

    /// The readers of `f` in unit `u` reading at a position in `(lo, hi)`.
    pub(crate) fn readers_between(&self, u: u32, f: Field, lo: Pos, hi: Pos) -> Vec<Id> {
        let Some(m) = self.readers.get(&(u, f)) else {
            return Vec::new();
        };
        m.range(
            std::ops::Bound::Excluded(&(lo, Id::MAX)),
            std::ops::Bound::Excluded(&(hi, 0)),
        )
        .map(|(k, ())| k.1)
        .collect()
    }

    pub(crate) fn add_out_reader(&mut self, of: Id, id: Id) {
        self.out_readers.upsert(of, &NoMeasure, |m| {
            m.insert(id, (), &NoMeasure);
        });
    }

    pub(crate) fn drop_out_reader(&mut self, of: Id, id: Id) {
        let mut empty = false;
        self.out_readers.update(&of, &NoMeasure, |m| {
            m.remove(&id, &NoMeasure);
            empty = m.is_empty();
        });
        if empty {
            self.out_readers.remove(&of, &NoMeasure);
        }
    }

    pub(crate) fn add_kind_reader(&mut self, u: u32, p: Pos, id: Id) {
        self.kind_readers.upsert(u, &NoMeasure, |m| {
            m.insert((p, id), (), &NoMeasure);
        });
    }

    pub(crate) fn drop_kind_reader(&mut self, u: u32, p: Pos, id: Id) {
        self.kind_readers.update(&u, &NoMeasure, |m| {
            m.remove(&(p, id), &NoMeasure);
        });
    }

    /// Files reader `id` at `p` in a window index (`back`: backward) with
    /// its reach; `None` takes it out.
    pub(crate) fn set_window(&mut self, back: bool, u: u32, p: Pos, id: Id, reach: Option<Pos>) {
        let map = if back { &mut self.back_readers } else { &mut self.fwd_readers };
        let mut m = map.get(&u).cloned().unwrap_or_default();
        match reach {
            Some(r) => {
                if back {
                    m.insert(p, (id, r), &super::claims::MinLow);
                } else {
                    m.insert(p, (id, r), &super::claims::MaxReach);
                }
            }
            None => {
                if m.get(&p).is_some_and(|e| e.0 == id) {
                    if back {
                        m.remove(&p, &super::claims::MinLow);
                    } else {
                        m.remove(&p, &super::claims::MaxReach);
                    }
                }
            }
        }
        map.insert(u, m, &NoMeasure);
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
                    index_set(&mut self.pset_nodes, u, p, id, false);
                }
                for k in ks {
                    self.pset_set(u, k, p, id, false);
                }
            }
            if !now.2.is_empty() {
                index_set(&mut self.pset_nodes, now.0, now.1, id, true);
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
        if let Some(m) = self.claim_at.get(&u)
            && m.get(&p).is_some_and(|e| e.0 == id)
        {
            let mut m = m.clone();
            m.remove(&p, &super::claims::MaxReach);
            self.claim_at.insert(u, m, &NoMeasure);
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
                let mut m = self.claim_at.get(&u).cloned().unwrap_or_default();
                if m.get(&p) != Some(&(id, reach)) {
                    m.insert(p, (id, reach), &super::claims::MaxReach);
                    self.claim_at.insert(u, m, &NoMeasure);
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
        if let Some(m) = self.holders.get(&u)
            && m.get(&p) == Some(&id)
        {
            let mut m = m.clone();
            m.remove(&p, &NoMeasure);
            self.holders.insert(u, m, &NoMeasure);
        }
    }

    /// Files node `id` in the holders index if it holds (after its node or
    /// claim state changed).
    pub(crate) fn refresh_holder(&mut self, id: Id) {
        let Some(n) = self.nodes.get(&id) else { return };
        let (u, p) = (n.unit, n.pos);
        let on = self.is_holder(id, n);
        let has = self.holders.get(&u).is_some_and(|m| m.get(&p) == Some(&id));
        if on && !has {
            let mut m = self.holders.get(&u).cloned().unwrap_or_default();
            m.insert(p, id, &NoMeasure);
            self.holders.insert(u, m, &NoMeasure);
        } else if !on && has {
            self.unhold(u, p, id);
        }
    }

    fn put_node(&mut self, id: Id, n: Node) {
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
        self.nodes.insert(id, n, &NoMeasure);
    }

    /// Removes a node's record.
    pub(crate) fn drop_node(&mut self, id: Id) {
        if let Some(o) = self.nodes.get(&id) {
            let (u, p, w, ks) = (o.unit, o.pos, in_work(o.st), pset_keys(o));
            if w {
                self.set_work(u, p, None);
            }
            self.unhold(u, p, id);
            self.unclaim_at(u, p, id);
            if !ks.is_empty() {
                index_set(&mut self.pset_nodes, u, p, id, false);
            }
            for k in ks {
                self.pset_set(u, k, p, id, false);
            }
        }
        self.nodes.remove(&id, &NoMeasure);
        self.op_marked.remove(&id, &NoMeasure);
    }

    fn set_work(&mut self, u: u32, p: Pos, id: Option<Id>) {
        self.work.upsert(u, &NoMeasure, |m| match id {
            Some(x) => {
                m.insert(p, x, &Count);
            }
            None => {
                m.remove(&p, &Count);
            }
        });
    }

    /// The nodes in work in unit `u` strictly between positions `lo` and
    /// `hi`, in order.
    pub(crate) fn work_between(&self, u: u32, lo: Pos, hi: Pos) -> impl Iterator<Item = Id> + '_ {
        self.work
            .get(&u)
            .into_iter()
            .flat_map(move |m| m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi)))
            .map(|(_, &x)| x)
    }

    /// A node in the window `(lo, hi)` of unit `u` still in work that may
    /// contribute to `f` (one that declares its contributions, or whose
    /// last run made none to `f`, is folded as it stands; its change
    /// re-asks the reader).
    pub(crate) fn range_blocker(&self, me: Id, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<Id> {
        self.work_between(u, lo, hi).find(|&x| {
            x != me
                && self.node(x).is_some_and(|n| {
                    // (Declared contributions are per field.)
                    !n.text.stat.contribs.as_ref().is_some_and(|cs| cs.iter().any(|c| c.0 == f))
                        && !n.res.as_ref().is_some_and(|r| r.contribs.iter().all(|c| c.0 != f))
                })
        })
    }

    /// Whether node `id` is ready by its read hints: no hinted field is
    /// defined before it by a node still in work, nor past a pending set.
    pub(crate) fn ready_by_hints(&self, core: &super::core::Core, id: Id, predict: bool) -> bool {
        let Some(n) = self.node(id) else { return true };
        let Some(fs) = n.text.stat.reads.as_ref() else { return true };
        let Some((u, pos, fr)) = self.frame_at(id, false) else { return true };
        fs.iter().all(|&f| {
            if self.loops.get(&f).is_some() {
                return true;
            }
            // An accumulator: held while a node before it in work may
            // still contribute (it would stop on the fold otherwise).
            if core.accs.contains_key(&f) {
                return self.range_blocker(id, f, u, 0, pos).is_none();
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
                    return false;
                }
            }
            predict || self.pending_set_between(core, u, f, reach.map_or(0, |r| r.0), pos, id).is_none()
        })
    }

    /// The first node in work strictly between `lo` and `hi` in unit `u`
    /// whose definition set is still pending and may hold `f` (a reader
    /// past it waits: its answer is not known yet).
    /// Sets (`on`) or clears node `id`'s pending-set entry under key `k`.
    fn pset_set(&mut self, u: u32, k: u64, p: Pos, id: Id, on: bool) {
        let had = self.psets.get(&(u, k)).and_then(|m| m.get(&p)) == Some(&id);
        index_set(&mut self.psets, (u, k), p, id, on);
        let has = self.psets.get(&(u, k)).and_then(|m| m.get(&p)) == Some(&id);
        if had != has {
            self.pset_buckets.upsert(u, &NoMeasure, |b| {
                let b = Arc::make_mut(&mut b.0);
                let i = (k % 64) as usize;
                b[i] = if has { b[i] + 1 } else { b[i] - 1 };
            });
        }
    }

    pub(crate) fn pending_set_between(&self, core: &super::core::Core, u: u32, f: Field, lo: Pos, hi: Pos, me: Id) -> Option<Id> {
        // (Usually none in the window at all; when few, each is asked
        // directly, the first that may hold `f` in text order.)
        let pending = |x: Id| {
            self.node(x).is_some_and(|n| n.text.stat.defs.is_none() && (n.wide || core.may_cover(n.may(), n.text.op, &n.text.args, f)))
        };
        let Some(m) = self.pset_nodes.get(&u) else { return None };
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
        let buckets = self.pset_buckets.get(&u).cloned();
        let mut look = |k: u64| {
            if buckets.as_ref().is_some_and(|b| b.0[(k % 64) as usize] == 0) {
                return;
            }
            let Some(m) = self.psets.get(&(u, k)) else { return };
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
        self.contribs.upsert((u, f), &NoMeasure, |t| match x {
            Some(v) => {
                t.insert(pos, v, m);
            }
            None => {
                t.remove(&pos, m);
            }
        });
    }

    /// The fold of `f`'s contributions in unit `u` strictly between `lo`
    /// and `hi`.
    pub(crate) fn fold(&self, m: &AccMeasure<'_>, u: u32, f: Field, lo: Pos, hi: Pos) -> ValueId {
        match self.contribs.get(&(u, f)) {
            Some(t) => t.fold(std::ops::Bound::Excluded(&lo), std::ops::Bound::Excluded(&hi), m),
            None => m.zero(),
        }
    }

    pub(crate) fn add_range_reader(&mut self, u: u32, f: Field, lo: Pos, hi: Pos, id: Id) {
        self.range_readers.upsert((u, f), &NoMeasure, |m| {
            m.insert((hi, id), lo, &NoMeasure);
        });
    }

    pub(crate) fn drop_range_reader(&mut self, u: u32, f: Field, hi: Pos, id: Id) {
        self.range_readers.update(&(u, f), &NoMeasure, |m| {
            m.remove(&(hi, id), &NoMeasure);
        });
    }

    /// The range readers of `f` whose window holds position `q`.
    pub(crate) fn range_readers_over(&self, u: u32, f: Field, q: Pos) -> Vec<Id> {
        let Some(m) = self.range_readers.get(&(u, f)) else {
            return Vec::new();
        };
        m.range(std::ops::Bound::Excluded(&(q, Id::MAX)), std::ops::Bound::Unbounded)
            .filter(|(_, lo)| **lo < q)
            .map(|(k, _)| k.1)
            .collect()
    }
}

/// Sets (`on`) or clears node `id` at position `p` of unit `u` in a
/// per-unit position index (clearing only its own entry).
fn index_set<U: Ord + Clone>(map: &mut PMap<U, PMap<Pos, Id>>, u: U, p: Pos, id: Id, on: bool) {
    let has = map.get(&u).and_then(|m| m.get(&p)).copied();
    if on == (has == Some(id)) {
        return;
    }
    if !on && has != Some(id) {
        return;
    }
    map.upsert(u, &NoMeasure, |m| {
        if on {
            m.insert(p, id, &NoMeasure);
        } else {
            m.remove(&p, &NoMeasure);
        }
    });
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
