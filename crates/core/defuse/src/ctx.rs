//! What a run sees and does.
//!
//! Every read is answered from what is committed now and recorded with its
//! answer; a definition, contribution or placement is kept in the product
//! until the commit. A `Ctx` holds no lock between calls.

use std::sync::Arc;

use crate::api::{At, Cause, Effect, FieldAt, Locality, MayDefine, NodeId, OpId, PathId, Rel, Step, Stop};
use crate::engine::Engine;
use crate::node::{Anchor, DefRec, Node, Placement, Product, ReadKind, ReadRec, RunRecord, State};
use crate::order::Pos;
use crate::read::{self, SITE, next_value, pos_of, reaching_at, through};
use crate::sched::Worker;
use crate::values::{Field, FxHashMap, ValueId, Values};

/// A handle on a value read through a path, extended step by step
/// (`Ctx::at`) and read at a step (`Ctx::get`): the run depends only on
/// what it took.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct View(u32);

pub(crate) struct ViewRec {
    pub(crate) kind: ReadKind,
    pub(crate) path: PathId,
    /// The value under the path when the view was made.
    pub(crate) value: Option<ValueId>,
    /// Its field's change count before the view was made.
    pub(crate) epoch: u64,
}

pub struct Ctx<'a> {
    pub(crate) eng: &'a Engine,
    pub(crate) worker: Option<&'a Worker>,
    pub(crate) me: &'a Arc<Node>,
    pub(crate) args: Arc<[ValueId]>,
    pub(crate) product: Product,
    pub(crate) views: Vec<ViewRec>,
    /// Its last record, for `placed_output`.
    pub(crate) last: Option<Arc<RunRecord>>,
    /// The change count the read in hand loaded ([`ReadRec::epoch`]).
    pub(crate) ep: u64,
    /// The last sibling `following` found, to go on from.
    pub(crate) follow_at: Option<(usize, NodeId)>,
    /// What `read` answered in this run, by field: a field read again is
    /// answered from here, with no second lookup and no second record.
    pub(crate) seen: FxHashMap<Field, Option<ValueId>>,
    /// Where the last `next_contribution` that found none stopped.
    pub(crate) stop_at: Option<NodeId>,
    /// Why this run was asked for, as it began (`woken_by`): a cause a
    /// re-ask stores while it runs is the next run's.
    pub(crate) woke: Option<Cause>,
    /// My wake was more than one cause (`Node::woke_merged`, taken).
    woke_merged: bool,
    /// My last run's placements by operation and arguments, made on the
    /// first `placed_output_or`.
    placed_keys: Option<PlacedKeys>,
    /// Why the run said it is stuck (`Ctx::stuck`).
    pub(crate) stuck: Option<String>,
    /// The node whose commit the run stopped to wait for (`Ctx::read`
    /// on its prediction at depth 0).
    pub(crate) wait_on: Option<NodeId>,
    /// The run stopped at a read past the settling layer's frontier
    /// (`Ctx::set_frontier`): it runs again once the frontier passes it.
    pub(crate) wait_frontier: bool,
    /// `Ctx::goes_on`: the run goes on from where its last run stopped.
    pub(crate) goes_on: bool,
    /// Its checks so far ([`Ctx::tick`]).
    ticks: u64,
    /// The reads `unread` took out, each by its index in the reads as
    /// they were then, in order: a point's count of reads made before it
    /// ([`Ctx::reads_made`]) taken before them is moved to the record's
    /// by [`Ctx::reads_kept_of`].
    unread_log: Vec<usize>,
    /// `PHITEX_RESUME_CHECK`: the reads of the run checked
    /// ([`Ctx::stash_reads`]).
    stashed: Vec<ReadRec>,
    /// [`Ctx::read_before_placer`]: my reads of a field's reaching
    /// definition are made before the node that placed me.
    before_placer: Option<NodeId>,
}

/// The check ([`Ctx::tick`]) from which a run's reads are looked at
/// again: a TeX run's 64 checks are a quarter of a million commands,
/// past the longest run of a document's piece or page, so ordinary runs
/// are never looked at, and a run that goes on for ever is looked at a
/// logarithmic number of times and after each re-ask.
const LONG_RUN_TICKS: u64 = 64;

/// A run's placements by operation and arguments.
type PlacedKeys = FxHashMap<(OpId, Arc<[ValueId]>), NodeId>;

fn stop() -> Stop {
    Stop { _private: () }
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(eng: &'a Engine, worker: Option<&'a Worker>, me: &'a Arc<Node>) -> Ctx<'a> {
        Ctx {
            eng,
            worker,
            me,
            args: me.args(),
            product: Product {
                output: ValueId::UNIT,
                defs: Vec::new(),
                contribs: Vec::new(),
                reads: Vec::new(),
                sources: Vec::new(),
                placements: Vec::new(),
                ranks: Vec::new(),
                claim: 0,
                claim_all: false,
                effect: None,
                place_effects: Vec::new(),
                scope_gen: eng.scope_gen.load(std::sync::atomic::Ordering::Acquire),
                unchanged: false,
                own_claim: false,
                at_hole: false,
                state: None,
                resumed: std::cell::Cell::new(false),
                kept_reads: std::cell::Cell::new(None),
                woken_at: None,
                on_park: false,
                settle_now: false,
                waits: Vec::new(),
                frontier: None,
                parked_on: None,
            },
            views: Vec::new(),
            ep: u64::MAX,
            follow_at: None,
            seen: FxHashMap::default(),
            unread_log: Vec::new(),
            stashed: Vec::new(),
            stop_at: None,
            woke: me.woke.lock().as_ref().map(|w| w.cause.clone()),
            woke_merged: me.woke_merged.swap(false, std::sync::atomic::Ordering::SeqCst),
            placed_keys: None,
            last: me.run_record(),
            stuck: None,
            wait_on: None,
            wait_frontier: false,
            goes_on: false,
            ticks: 0,
            before_placer: None,
        }
    }

    /// From now on, every read of a field's definition reaching me
    /// (`read`, `read_path`, `read_or_path`, `read_derived`, `def_site`,
    /// `snapshot`, `read_item`) is made, answered and filed as a
    /// read before the node that placed me (`At::Before(placer)`), not
    /// at my position: none of my placer's definitions, nor those of the
    /// children it placed before me, answer it, and a change of them
    /// asks me nothing. For a child whose text stands in the middle of
    /// its placer's (a picture of a figure's run): what the placer had
    /// bound before the child's place it hands over itself, and what it
    /// defines after it the child, as in the text, never sees. My views
    /// (`view_field`, `view`) stay mine: a field the placer defines for
    /// me is read there, and so is my scope depth (`depth`). A node with
    /// no placer reads at its position.
    pub fn read_before_placer(&mut self) {
        self.before_placer = self.me.parent;
    }

    /// Where a read of a definition reaching me is made
    /// ([`Ctx::read_before_placer`]): the node it is made before, and
    /// the anchor it is filed with.
    fn here(&self) -> (NodeId, Anchor) {
        match self.before_placer {
            Some(p) => (p, Anchor::Before(p)),
            None => (self.me.id, Anchor::Me),
        }
    }

    /// The product, with the output.
    pub(crate) fn into_product(mut self, output: ValueId) -> Product {
        self.product.output = output;
        self.product.woken_at = match self.woke {
            Some(Cause::Range { by, .. } | Cause::Changed { site: Some(by), .. }) => Some(by),
            _ => None,
        };
        self.product.on_park = !self.woke_merged
            && match &self.woke {
                Some(Cause::Range { field, .. } | Cause::Changed { field, .. } | Cause::Rebind { field, .. }) => {
                    *self.me.parked_on.lock() == Some(*field)
                }
                _ => false,
            };
        self.product
    }

    pub fn values(&self) -> &'a Values {
        &self.eng.values
    }

    /// My unit: a read at its end (`At::End`).
    pub fn unit(&self) -> crate::api::UnitId {
        self.me.unit
    }

    pub fn node(&self) -> NodeId {
        self.me.id
    }

    pub fn op(&self) -> OpId {
        self.me.op()
    }

    pub fn args(&self) -> &[ValueId] {
        &self.args
    }

    /// `Err(Stop)` once a commit has refuted a read of this run.
    fn check(&self) -> Result<(), Stop> {
        match self.me.state() {
            State::RunningDirty | State::Inert | State::RunningInert | State::Gone => Err(stop()),
            State::Clean | State::Dirty | State::Running => Ok(()),
        }
    }

    fn record(&mut self, kind: ReadKind, path: PathId, seen: Option<ValueId>) {
        let epoch = if kind.field().is_some() { self.ep } else { u64::MAX };
        self.product.reads.push(ReadRec {
            kind,
            path,
            seen,
            epoch,
            filed: false,
        });
    }

    /// The node whose definition a read got (`RunRecord::sources`).
    fn note_source(&mut self, by: Option<NodeId>) {
        if let Some(by) = by
            && self.product.sources.last() != Some(&by)
        {
            self.product.sources.push(by);
        }
    }

    /// How many reads my run has made so far (a mark for
    /// [`Ctx::fields_read_since`]).
    pub fn reads_len(&self) -> usize {
        self.product.reads.len()
    }

    /// The fields my run read from its read `from` on (a mark of
    /// [`Ctx::reads_len`]), with each read's index, in order (a field read
    /// twice is listed twice): a client checking what it read after a
    /// point against what another node may define.
    pub fn fields_read_since(&self, from: usize) -> impl Iterator<Item = (usize, Field)> + '_ {
        self.product
            .reads
            .iter()
            .enumerate()
            .skip(from)
            .filter_map(|(i, r)| r.kind.field().map(|f| (i, f)))
    }

    /// Loads `f`'s change count before a read of it ([`ReadRec::epoch`]).
    fn before_read(&mut self, f: Field) {
        self.ep = self.eng.field_epoch(f);
    }

    /// A family snapshot (docs/spec-state-model.md §2.1, §9): the value
    /// folding the reaching definitions of every field whose name starts
    /// with `family`'s name (`cat:`, `param:`, `count:` …) at my position,
    /// as a list of `[field, value]` pairs in field order; unrecorded (no
    /// read is filed), cached by content in the store. A client loads a
    /// whole table from it and records only the items it uses
    /// (`read_item`). The body is the rewrite's; the test is
    /// tests/snapshot.rs.
    pub fn snapshot(&mut self, family: Field) -> Result<ValueId, Stop> {
        self.check()?;
        let v = self.values();
        let Some(prefix) = v.as_str(family.0) else {
            return Ok(v.list(Vec::new()));
        };
        let has = |f: Field| v.with_str(f.0, |s| s.starts_with(prefix.as_str())).unwrap_or(false);
        // (the family's fields: those with a definition filed in some
        // segment, and those the environment holds now; one reaching me
        // is defined in a segment before me or is the environment's)
        let mut fields: Vec<Field> = Vec::new();
        for sid in self.eng.order.all() {
            let seg = self.eng.seg(sid);
            let inner = seg.inner.read();
            fields.extend(inner.defs.keys().copied().filter(|&f| has(f)));
        }
        fields.extend(self.eng.env.values.read().keys().copied().filter(|&f| has(f)));
        fields.sort_unstable_by_key(|f| f.0.raw());
        fields.dedup();
        let mut pairs = Vec::with_capacity(fields.len());
        let (at, _) = self.here();
        for f in fields {
            let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
            self.note_source(r.by);
            if let Some(x) = r.value {
                pairs.push(v.list(vec![f.0, x]));
            }
        }
        // (interned by content: the same definitions reaching give the
        // same value wherever they reach)
        Ok(v.list(pairs))
    }

    /// Records a read of `f` answered `seen`, as `read` would have
    /// recorded it, without answering it again: a client that loaded
    /// `f`'s value from a snapshot and used it files the read here, so a
    /// definition of `f` reaching me that differs from `seen` re-asks me
    /// (docs/spec-state-model.md §2.1).
    pub fn read_item(&mut self, f: Field, seen: Option<ValueId>) -> Result<(), Stop> {
        self.check()?;
        if self.seen.contains_key(&f) {
            return Ok(());
        }
        // (no change count: the value was loaded before now, so the
        // commit checks the read whatever its field's count says)
        self.ep = u64::MAX;
        let (_, anchor) = self.here();
        self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, seen);
        self.seen.insert(f, seen);
        Ok(())
    }

    /// The definition of `f` reaching me, or the environment's value; an
    /// alias definition followed ([`Ctx::define_alias`]).
    pub fn read(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        if let Some(&x) = self.seen.get(&f) {
            return Ok(x);
        }
        self.before_read(f);
        let (at, anchor) = self.here();
        let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
        self.note_source(r.by);
        self.wait_for_settling(&r)?;
        self.hold_at_frontier(f)?;
        self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, r.value);
        let x = self.follow_aliases(r.value)?;
        self.seen.insert(f, x);
        Ok(x)
    }

    /// A definition reaching me that is a settling node's prediction
    /// (`Operation::settles_at_quiet`, filed before its first run), where
    /// I am no settling node: the run stops, and runs again once that
    /// node commits (or at the next quiet point), instead of going on on
    /// a guess the last ones missed. That node's reader as the page
    /// loop is its finisher's: TeX's own chain from a routine to the text
    /// after it.
    fn wait_for_settling(&mut self, r: &crate::read::Reaching) -> Result<(), Stop> {
        if !r.provisional {
            return Ok(());
        }
        let Some(by) = r.by else { return Ok(()) };
        let settles = |n: NodeId| {
            self.eng
                .placed_node(n)
                .is_some_and(|x| self.eng.op(x.op()).settles_at_quiet())
        };
        if by == self.me.id || !settles(by) || self.eng.op(self.me.op()).settles_at_quiet() {
            return Ok(());
        }
        // (a prediction left by a node clean on a run of its own: no
        // commit of it is coming to wait for)
        if crate::sched::settled_node(self.eng, by) {
            return Ok(());
        }
        self.wait_on = Some(by);
        Err(stop())
    }

    /// A read of a field the settling layer defines
    /// (`Operation::settles_at_quiet`; `Engine::settle_marks`) by a node
    /// outside it that stands past the layer's frontier
    /// (`Ctx::set_frontier`): the run
    /// stops, and runs again once the frontier passes it. Past the
    /// frontier the layer may yet place a node before the reader, whose
    /// definition the read would meet (TeX's text after a page's break
    /// reads what that page's routine defined): read now, the answer is
    /// one the reader's run is thrown away for once that node comes.
    fn hold_at_frontier(&mut self, f: Field) -> Result<(), Stop> {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        let eng = self.eng;
        let fr = eng.frontier.load(Acquire);
        if fr == 0 || !eng.settle_marked(f) || eng.op(self.me.op()).settles_at_quiet() {
            return Ok(());
        }
        if !crate::sched::past_frontier(eng, self.me, fr) {
            return Ok(());
        }
        eng.frontier_held.fetch_add(1, Relaxed);
        if crate::sched::run_log() {
            eprintln!(
                "frontier: read held node {} field {} at {}",
                self.me.id.raw(),
                self.values().render(f.0),
                fr.saturating_sub(1)
            );
        }
        self.wait_frontier = true;
        Err(stop())
    }

    /// The settling layer's frontier, from this run's commit: the node
    /// past which it may yet place nodes (`None`: it places none, every
    /// reader is free). A reader of a field the layer defines past it
    /// waits for it to pass ([`Ctx::read`]).
    pub fn set_frontier(&mut self, at: Option<NodeId>) {
        self.product.frontier = Some(at);
    }

    /// `x` with each alias definition it is followed: a read of the
    /// aliased field before the alias's node, recorded as mine
    /// ([`Ctx::define_alias`]).
    fn follow_aliases(&mut self, mut x: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        for _ in 0..32 {
            let Some((g, at)) = x.and_then(|y| read::alias_of(self.values(), y)) else {
                return Ok(x);
            };
            x = match self.read_at_raw(g, At::Before(at))? {
                FieldAt::Def(y, _) => Some(y),
                FieldAt::Unset | FieldAt::Gone => None,
            };
        }
        Ok(x)
    }

    /// `f` := the value of `g` reaching node `at` (`At::Before(at)`), an
    /// alias: a reader of `f` that this definition reaches is answered
    /// with that value and reads `g` there itself, so a change of `g`'s
    /// definition reaching `at` asks that reader again and never me; an
    /// alias of an alias is followed. For a value a run passes on without
    /// looking at it (a page-layer register's symbol copied into another
    /// register, docs/spec-state-model.md §2.2).
    pub fn define_alias(&mut self, f: Field, g: Field, at: NodeId, loc: Locality) {
        let x = read::alias_value(self.values(), g, at);
        self.product.defs.push(DefRec {
            f,
            loc,
            value: x,
            rel: None,
        });
    }

    /// The definition of `f` reaching another position; an alias
    /// definition followed ([`Ctx::define_alias`]).
    pub fn read_at(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        let r = self.read_at_raw(f, at)?;
        if let FieldAt::Def(x, by) = r
            && read::alias_of(self.values(), x).is_some()
        {
            return Ok(match self.follow_aliases(Some(x))? {
                Some(y) => FieldAt::Def(y, by),
                None => FieldAt::Unset,
            });
        }
        Ok(r)
    }

    /// [`Ctx::read_at`] with an alias definition as it is.
    fn read_at_raw(&mut self, f: Field, at: At) -> Result<FieldAt, Stop> {
        self.check()?;
        self.before_read(f);
        let (anchor, r) = match at {
            At::Before(n) => (Anchor::Before(n), reaching_at(self.eng, f, n, false)),
            At::AfterOwn(n) => (Anchor::AfterOwn(n), reaching_at(self.eng, f, n, true)),
            At::Final => (Anchor::Final, Some(crate::read::final_read(self.eng, f))),
            At::End(u) => (
                Anchor::End(u),
                Some(read::on_one_layout(self.eng, || {
                    read::reaching_visible(self.eng, f, Pos(self.eng.unit(u).end.0 - 1), true)
                })),
            ),
        };
        if let (Some(x), Anchor::Before(_) | Anchor::AfterOwn(_)) = (&r, &anchor) {
            self.note_source(x.by);
            self.wait_for_settling(x)?;
            self.hold_at_frontier(f)?;
        }
        self.record(
            ReadKind::Field { f, at: anchor },
            PathId::WHOLE,
            r.and_then(|x| x.value),
        );
        Ok(match r {
            None => FieldAt::Gone,
            Some(x) => match x.value {
                Some(v) => FieldAt::Def(v, x.by),
                None => FieldAt::Unset,
            },
        })
    }

    /// [`Ctx::read_at`] at the line's end (`At::Final`, a loop-back
    /// read) through a path: the run depends only on the path's answer
    /// (an alias definition: on all of it, followed). None: no value
    /// there, or the path does not apply.
    pub fn read_final_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let r = crate::read::final_read(self.eng, f);
        if let Some(x) = r.value
            && read::alias_of(self.values(), x).is_some()
        {
            self.record(ReadKind::Field { f, at: Anchor::Final }, PathId::WHOLE, r.value);
            let y = self.follow_aliases(r.value)?;
            return Ok(read::apply_path(self.eng, y, p));
        }
        let x = through(self.eng, r, p);
        self.record(ReadKind::Field { f, at: Anchor::Final }, p, x);
        Ok(x)
    }

    /// The answer `read_path` would give, with no read recorded: the run
    /// depends on nothing of it. For a reader that looks first and then
    /// records a narrower read covering what it used of the answer (each
    /// element it checked: `Step::Within` of them), and goes by that
    /// read's answer, never by this one alone.
    pub fn peek_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        let (at, _) = self.here();
        let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
        if r.value.is_some_and(|x| read::alias_of(self.values(), x).is_some()) {
            return Ok(None);
        }
        Ok(read::through_at(self.eng, r, p, at, false))
    }

    /// Records a read of `f` through path `p` answered `seen`, as
    /// `read_path` records one, without answering it: a client that
    /// looked first ([`Ctx::peek_path`]) and used `seen` of the answer
    /// files what it used here. A definition of `f` whose answer through
    /// `p` differs from `seen` re-asks me, one already made before this
    /// record as well (the commit checks the read whatever the field's
    /// change count says).
    pub fn note_path(&mut self, f: Field, p: PathId, seen: Option<ValueId>) -> Result<(), Stop> {
        self.check()?;
        self.ep = u64::MAX;
        let (_, anchor) = self.here();
        self.record(ReadKind::Field { f, at: anchor }, p, seen);
        Ok(())
    }

    /// `read` through a path: the run depends only on the path's answer
    /// (an alias definition: on all of it, followed).
    pub fn read_path(&mut self, f: Field, p: PathId) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let (at, anchor) = self.here();
        let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
        self.note_source(r.by);
        if let Some(x) = r.value
            && read::alias_of(self.values(), x).is_some()
        {
            self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, r.value);
            let y = self.follow_aliases(r.value)?;
            return Ok(read::apply_path(self.eng, y, p));
        }
        let x = through(self.eng, r, p);
        self.record(ReadKind::Field { f, at: anchor }, p, x);
        Ok(x)
    }

    /// A derived read (`Step::Derived`): the field named `prefix`, the
    /// digits of `v + j`, `suffix`, where `v` is the integer value of
    /// `base` (0 where none) reaching me, read where it reaches me and
    /// answered through `then`. The run depends on that answer alone:
    /// recorded as a read of `base` and one of the named field, each
    /// through the same path, whose answer either re-asks by computing the
    /// name again (so a move of `v` that names a field holding what the
    /// old one held leaves me standing). The answer, and the field named
    /// now. (An alias definition on either field: a whole read of each,
    /// followed, as `read_path` reads one.)
    pub fn read_derived(
        &mut self,
        base: Field,
        prefix: &str,
        j: i64,
        suffix: &str,
        then: PathId,
    ) -> Result<(Option<ValueId>, Field), Stop> {
        self.check()?;
        let v = self.values();
        let spec = v.list(vec![base.0, v.str(prefix), v.int(j), v.str(suffix)]);
        let mut steps = vec![Step::Derived(spec)];
        if then != PathId::WHOLE {
            steps.extend_from_slice(self.eng.path_steps(then).unwrap_or_default());
        }
        let p = self.eng.path(&steps);
        self.before_read(base);
        let ep_base = self.ep;
        let (at, anchor) = self.here();
        let rb = reaching_at(self.eng, base, at, false).ok_or_else(stop)?;
        self.note_source(rb.by);
        self.wait_for_settling(&rb)?;
        self.hold_at_frontier(base)?;
        let g = read::derived_name(v, spec, rb.value).ok_or_else(stop)?;
        self.before_read(g);
        let ep_g = self.ep;
        let rg = reaching_at(self.eng, g, at, false).ok_or_else(stop)?;
        self.note_source(rg.by);
        self.wait_for_settling(&rg)?;
        self.hold_at_frontier(g)?;
        let aliased = |x: Option<ValueId>| x.is_some_and(|x| read::alias_of(v, x).is_some());
        if aliased(rb.value) || aliased(rg.value) {
            let b = self.read(base)?;
            let g2 = read::derived_name(v, spec, b).ok_or_else(stop)?;
            let x = self.read(g2)?;
            return Ok((read::apply_path(self.eng, x, then), g2));
        }
        let x = read::through_at(self.eng, rg, p, at, false);
        self.ep = ep_base;
        self.record(ReadKind::Field { f: base, at: anchor }, p, x);
        self.ep = ep_g;
        self.record(ReadKind::Field { f: g, at: anchor }, p, x);
        Ok((x, g))
    }

    /// `read` with one look-up, answered whole or through `p` as
    /// `through_path` decides from the value itself: where it is true the
    /// run depends only on `p`'s answer (recorded as `read_path` records
    /// it, and not kept as the field's whole value, so a later `read` of
    /// `f` records that too); where false, a whole read as `read` makes.
    /// The value reaching me, and whether `p` was taken. (An alias
    /// definition is read whole, followed.)
    pub fn read_or_path(
        &mut self,
        f: Field,
        p: PathId,
        through_path: impl FnOnce(&Values, ValueId) -> bool,
    ) -> Result<(Option<ValueId>, bool), Stop> {
        self.check()?;
        if let Some(&x) = self.seen.get(&f) {
            return Ok((x, false));
        }
        self.before_read(f);
        let (at, anchor) = self.here();
        let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
        self.note_source(r.by);
        self.wait_for_settling(&r)?;
        self.hold_at_frontier(f)?;
        if let Some(x) = r.value
            && read::alias_of(self.values(), x).is_none()
            && through_path(self.values(), x)
        {
            let y = read::apply_path(self.eng, Some(x), p);
            self.record(ReadKind::Field { f, at: anchor }, p, y);
            return Ok((Some(x), true));
        }
        self.record(ReadKind::Field { f, at: anchor }, PathId::WHOLE, r.value);
        let x = self.follow_aliases(r.value)?;
        self.seen.insert(f, x);
        Ok((x, false))
    }

    /// The node whose definition of `f` reaches me (None: the environment).
    pub fn def_site(&mut self, f: Field) -> Result<Option<NodeId>, Stop> {
        self.check()?;
        self.before_read(f);
        let (at, anchor) = self.here();
        let r = reaching_at(self.eng, f, at, false).ok_or_else(stop)?;
        self.note_source(r.by);
        let seen = through(self.eng, r, SITE);
        self.record(ReadKind::Field { f, at: anchor }, SITE, seen);
        Ok(r.by)
    }

    /// The environment's value of `f`, whatever is defined before me.
    pub fn env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        let x = read::env_value(self.eng, f);
        self.record(ReadKind::Env { f }, PathId::WHOLE, x);
        Ok(x)
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Local);
    }

    /// A definition at the locality's depth (`Locality`): the commit
    /// files it with its depth, from my brackets (`set_effect`, or my
    /// operation's `effect`) and the depth reaching me.
    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        self.product.defs.push(DefRec {
            f,
            loc,
            value: x,
            rel: None,
        });
    }

    /// `f` := the integer definition of `f` reaching me, plus `c`. Kept as
    /// a relation: when its input changes the engine recomputes it without
    /// running me, and once this operation has made one, its pending nodes
    /// are given it ahead of their runs. This is how a counter chain is not
    /// a chain of runs. The run need not read `f` first.
    pub fn define_rel(&mut self, f: Field, c: i64) {
        self.define_rel_with(f, c, Locality::Local);
    }

    /// [`Ctx::define_rel`] at a locality: `Global`, the value reaching me
    /// (the innermost binding seen here) plus `c`, standing past the
    /// scopes I am in (a global `\advance` inside an environment).
    pub fn define_rel_with(&mut self, f: Field, c: i64, loc: Locality) {
        self.product.defs.push(DefRec {
            f,
            loc,
            value: ValueId::UNIT,
            rel: Some(Rel::Add(c)),
        });
    }

    /// `f` := the definition of `f` reaching me with the items of list
    /// `items` appended by the client's append ([`Engine::set_append`]),
    /// at a locality: a relation as [`Ctx::define_rel`]'s, recomputed in
    /// place when the value reaching me moves (a list each node adds to,
    /// microtype's `\\MT@font@list`, is not a chain of runs). The run need
    /// not read `f`.
    pub fn define_append_with(&mut self, f: Field, items: ValueId, loc: Locality) {
        self.product.defs.push(DefRec {
            f,
            loc,
            value: ValueId::UNIT,
            rel: Some(Rel::Append(items)),
        });
    }

    /// Adds `x` to `f`'s contributions at my position.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        self.product.contribs.push((f, x));
    }

    /// A bound's label: None when the node named is gone (the read
    /// answers none, as a read at a gone node does, `FieldAt::Gone`).
    fn bound(&self, n: Option<NodeId>) -> Option<Pos> {
        match n {
            Some(n) => pos_of(self.eng, n),
            None => Some(read::unit_start(self.eng, self.me)),
        }
    }

    /// The fold's value; None where a bound is gone.
    fn fold_value(
        &mut self,
        f: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
        incl: bool,
    ) -> Result<Option<ValueId>, Stop> {
        self.check()?;
        self.before_read(f);
        // (the bounds' labels and the fold between them on one layout)
        read::on_one_layout(self.eng, || {
            let Some(fp) = self.bound(from) else {
                return Ok(None);
            };
            let tp = match to {
                Some(n) => pos_of(self.eng, n),
                None => Some(pos_of(self.eng, self.me.id).ok_or_else(stop)?),
            };
            if let Some(tp) = tp {
                read::trace_inverted(self.eng, "run", self.me, f, (from, fp), (to, read::end_of(tp, incl)));
            }
            Ok(tp.map(|tp| read::fold(self.eng, f, Some(fp), Some(read::end_of(tp, incl)))))
        })
    }

    /// The monoid's empty value: a fold whose bound is gone.
    fn empty(&self, f: Field) -> ValueId {
        self.eng.monoid(f).empty(self.values())
    }

    /// The declared monoid's fold over the contributions to `f` after
    /// `from` (None: the unit's start) and before `to` (None: me); empty
    /// where `from` or `to` is gone.
    pub fn fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.record(
            ReadKind::Fold {
                f,
                from,
                to,
                incl: false,
            },
            PathId::WHOLE,
            x,
        );
        Ok(x.unwrap_or_else(|| self.empty(f)))
    }

    /// The fold over the contributions to `f` after `from` (None: the
    /// unit's start) up to and with `to`'s own.
    pub fn fold_through(&mut self, f: Field, from: Option<NodeId>, to: NodeId) -> Result<ValueId, Stop> {
        let x = self.fold_value(f, from, Some(to), true)?;
        self.record(
            ReadKind::Fold {
                f,
                from,
                to: Some(to),
                incl: true,
            },
            PathId::WHOLE,
            x,
        );
        Ok(x.unwrap_or_else(|| self.empty(f)))
    }

    /// `fold` through a path.
    pub fn fold_path(
        &mut self,
        f: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
        p: PathId,
    ) -> Result<Option<ValueId>, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        let y = x.and_then(|x| read::apply_path(self.eng, Some(x), p));
        self.record(
            ReadKind::Fold {
                f,
                from,
                to,
                incl: false,
            },
            p,
            y,
        );
        Ok(y)
    }

    /// The first contribution to `f` after `after` (None: the unit's start),
    /// or None while the next node after it has not committed, or where
    /// `after` is gone.
    pub fn next_contribution(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        self.check()?;
        self.before_read(f);
        // (the bound's label and the search from it on one layout)
        let Some(n) = read::on_one_layout(self.eng, || {
            self.bound(after)
                .map(|ap| read::next_contribution(self.eng, f, Some(ap), self.me.id))
        }) else {
            self.record(ReadKind::Next { f, after }, PathId::WHOLE, None);
            return Ok(None);
        };
        self.record(ReadKind::Next { f, after }, PathId::WHOLE, next_value(self.eng, &n));
        self.stop_at = if n.item.is_none() { n.stop } else { None };
        Ok(n.item)
    }

    /// The node the last `next_contribution` that answered none stopped
    /// at, one that has not committed yet; None where it met the line's
    /// end (or a gone bound).
    pub fn stopped_at(&self) -> Option<NodeId> {
        self.stop_at
    }

    /// A view on `fold(f, from, to)`, read step by step.
    pub fn view(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        let x = self.fold_value(f, from, to, false)?;
        self.views.push(ViewRec {
            kind: ReadKind::Fold {
                f,
                from,
                to,
                incl: false,
            },
            path: PathId::WHOLE,
            value: x,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// A view on `fold_through(f, from, to)`.
    pub fn view_through(&mut self, f: Field, from: Option<NodeId>, to: NodeId) -> Result<View, Stop> {
        let x = self.fold_value(f, from, Some(to), true)?;
        self.views.push(ViewRec {
            kind: ReadKind::Fold {
                f,
                from,
                to: Some(to),
                incl: true,
            },
            path: PathId::WHOLE,
            value: x,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// A view on `read(f)`.
    pub fn view_field(&mut self, f: Field) -> Result<View, Stop> {
        self.check()?;
        self.before_read(f);
        let r = reaching_at(self.eng, f, self.me.id, false).ok_or_else(stop)?;
        self.note_source(r.by);
        self.views.push(ViewRec {
            kind: ReadKind::Field { f, at: Anchor::Me },
            path: PathId::WHOLE,
            value: r.value,
            epoch: self.ep,
        });
        Ok(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    fn view_rec(&self, v: View) -> Option<&ViewRec> {
        self.views.get(v.0 as usize)
    }

    /// The view one step further; None where the step does not apply.
    pub fn at(&mut self, v: View, s: Step) -> Option<View> {
        let rec = self.view_rec(v)?;
        let (kind, value, epoch) = (rec.kind.clone(), rec.value, rec.epoch);
        let path = self.eng.path_extend(rec.path, s);
        let x = read::step(self.eng, value, s);
        let present = self.eng.path_extend(path, Step::Present);
        let seen = Some(self.eng.values().bool(x.is_some()));
        self.ep = epoch;
        self.record(kind.clone(), present, seen);
        x?;
        self.views.push(ViewRec {
            kind,
            path,
            value: x,
            epoch,
        });
        Some(View(u32::try_from(self.views.len() - 1).unwrap_or(u32::MAX)))
    }

    /// The value at one step further, recorded as a read of that path.
    pub fn get(&mut self, v: View, s: Step) -> Option<ValueId> {
        let rec = self.view_rec(v)?;
        let (kind, value, epoch) = (rec.kind.clone(), rec.value, rec.epoch);
        let path = self.eng.path_extend(rec.path, s);
        let x = read::step(self.eng, value, s);
        self.ep = epoch;
        self.record(kind, path, x);
        x
    }

    pub fn len(&mut self, v: View) -> Option<u64> {
        let x = self.get(v, Step::Len)?;
        self.eng.values().as_int(x).and_then(|i| u64::try_from(i).ok())
    }

    pub fn has(&mut self, v: View, k: ValueId) -> bool {
        self.get(v, Step::Has(k))
            .and_then(|x| self.eng.values().as_bool(x))
            .unwrap_or(false)
    }

    /// The whole value under the view, recorded as a read of all of it.
    pub fn whole(&mut self, v: View) -> ValueId {
        let Some(rec) = self.view_rec(v) else {
            return ValueId::UNIT;
        };
        let (kind, path, value) = (rec.kind.clone(), rec.path, rec.value);
        self.ep = rec.epoch;
        self.record(kind, path, value);
        value.unwrap_or(ValueId::UNIT)
    }

    /// Places a node after me, before my continuation and next sibling;
    /// its index among my placements.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::Child(op, args));
        self.product.placements.len() - 1
    }

    /// Takes back my placements of this run from the `n`-th on, and my
    /// reads of their outputs: a pass that a run throws away and makes
    /// again leaves none of what it placed (its other reads stay, as a
    /// second pass's do).
    pub fn unplace_from(&mut self, n: usize) {
        let p = &mut self.product;
        p.placements.truncate(n);
        p.ranks.truncate(n);
        p.waits.retain(|&j| j < n);
        p.reads
            .retain(|rd| !matches!(rd.kind, ReadKind::Placed { i, .. } if i >= n));
        self.placed_keys = None;
    }

    /// [`Ctx::child`], unless this run placed that very node already (a
    /// second pass of a run over the same text): that placement's index.
    pub fn child_once(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let same = self
            .product
            .placements
            .iter()
            .position(|p| matches!(p, Placement::Child(o, a) if *o == op && a[..] == args[..]));
        same.unwrap_or_else(|| self.child(op, args))
    }

    /// Whether this node ran before (its last record stands): a run of
    /// an edit's evaluation on a node the build has, not a node's first.
    pub fn ran_before(&self) -> bool {
        self.last.is_some()
    }

    /// Places a node after me and my children.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::Follow(op, args));
        self.product.placements.len() - 1
    }

    /// The brackets my `i`-th placement is filed with until it runs,
    /// where its operation leaves them to its run (`Effect::Pending`):
    /// its text is not its arguments, but I know it (a cut piece's rest,
    /// whose continuation I made). Filed in my commit with my own: a
    /// close my text predicted that moves into the rest leaves the depth
    /// after the rest as it was, and the readers there stand. A guess:
    /// the node's run files what it does.
    pub fn predict_placed_effect(&mut self, i: usize, e: Effect) {
        if e != Effect::Pending && e != Effect::None {
            self.product.place_effects.push((i, e));
        }
    }

    /// Places a node after `at` and its placements.
    pub fn follow_after(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        self.product.placements.push(Placement::FollowAfter(at, op, args));
        self.product.placements.len() - 1
    }

    /// Places a node after the node my `j`-th placement of this run
    /// stands for, and its placements: a run placing several nodes after
    /// one node elsewhere puts each after the one before, in its order
    /// (`follow_after` puts each right after that node's own children).
    ///
    /// # Panics
    ///
    /// Where `j` is not a placement this run made before.
    pub fn follow_after_placed(&mut self, j: usize, op: OpId, args: Vec<ValueId>) -> usize {
        assert!(
            j < self.product.placements.len(),
            "follow_after_placed({j}): no such placement before"
        );
        self.product.placements.push(Placement::FollowAfterPlaced(j, op, args));
        self.product.placements.len() - 1
    }

    /// Gives my `i`-th placement of this run, one placed after another
    /// node (`follow_after`, `follow_after_placed`), its order among the
    /// nodes others place after that node: it stands after those of a
    /// lower rank and before those of a higher one, wherever each placer
    /// stands and whenever each commits. Unranked (0), it goes right
    /// after that node's children, before every node placed after it
    /// already. A page's finisher ranks by the page's place among the
    /// document's fires, so a finisher placed by a loop node from a start
    /// another one has since moved never stands before the finisher of an
    /// earlier page after the same piece.
    ///
    /// # Panics
    ///
    /// Where `i` is not a placement this run made.
    pub fn rank_placement(&mut self, i: usize, rank: u64) {
        assert!(
            i < self.product.placements.len(),
            "rank_placement({i}): no such placement"
        );
        let r = &mut self.product.ranks;
        if r.len() <= i {
            r.resize(i + 1, 0);
        }
        r[i] = rank;
    }

    /// The output my `i`-th placement had at my last commit, if it has run.
    pub fn placed_output(&mut self, i: usize) -> Option<ValueId> {
        let c = self.last.as_ref().and_then(|r| r.placed.get(i).copied());
        let out = c.and_then(|c| {
            let n = self.eng.node(c);
            (n.state() != State::Gone)
                .then(|| n.run_record().map(|r| r.output))
                .flatten()
        });
        self.record(ReadKind::Placed { i, guess: false }, PathId::WHOLE, out);
        out
    }

    /// The output of the node my `i`-th placement of this run stands for
    /// (my last run's of the same operation and arguments) if it has run,
    /// else `guess`: recorded as seen, so a placement whose first output
    /// is the guess re-asks me for nothing, and one that differs re-asks
    /// me (a loop folding a child's result goes on on its guess).
    pub fn placed_output_or(&mut self, i: usize, guess: ValueId) -> ValueId {
        let out = self.placed_child_output(i);
        let x = out.unwrap_or(guess);
        self.record(ReadKind::Placed { i, guess: true }, PathId::WHOLE, Some(x));
        x
    }

    /// [`Ctx::placed_output_or`] through path `p`: the answer of `p` on
    /// the output of the node my `i`-th placement of this run stands for,
    /// if it has run, else on the output `guess` gives (asked only then,
    /// None: no output); recorded as that
    /// answer alone, so a child's output that changes elsewhere than `p`
    /// leaves me standing, and one whose first output answers otherwise
    /// than the guess re-asks me.
    pub fn placed_output_path_or(
        &mut self,
        i: usize,
        p: PathId,
        guess: impl FnOnce(&Self) -> Option<ValueId>,
    ) -> Option<ValueId> {
        let x = match self.placed_child_output(i) {
            Some(o) => read::apply_path(self.eng, Some(o), p),
            None => {
                let g = guess(self);
                read::apply_path(self.eng, g, p)
            }
        };
        self.record(ReadKind::Placed { i, guess: true }, p, x);
        x
    }

    /// The value of `f` at the line's end (`At::Final`, a loop-back), not
    /// recorded: for a guess that a recorded read checks
    /// ([`Ctx::placed_output_path_or`]), never for what a run makes.
    #[must_use]
    pub fn peek_final(&self, f: Field) -> Option<ValueId> {
        read::final_read(self.eng, f).value
    }

    /// The output of the node my `i`-th placement of this run stands for
    /// (`Ctx::placed_output_or`'s), if it has run: not recorded, a peek.
    /// A run that stops for a placement's output reads it with
    /// [`Ctx::placed_output_wait`], or nothing wakes it when it comes.
    pub fn placed_output_peek(&mut self, i: usize) -> Option<ValueId> {
        self.placed_child_output(i)
    }

    /// The node my `i`-th placement of this run stands for re-asks me at
    /// its next commit, whatever its output (a loop that stops for a
    /// child's result, not going on on a guess of it, nor on the output
    /// of a run before that child was re-asked). Not a read: nothing
    /// refutes the run by it, and no later run keeps it.
    pub fn placed_output_wait(&mut self, i: usize) {
        if !self.product.waits.contains(&i) {
            self.product.waits.push(i);
        }
    }

    /// The output of the node my `i`-th placement of this run stands for
    /// (my last run's of the same operation and arguments), if it has run.
    fn placed_child_output(&mut self, i: usize) -> Option<ValueId> {
        if self.placed_keys.is_none() {
            let mut m: FxHashMap<(OpId, Arc<[ValueId]>), NodeId> = FxHashMap::default();
            for &c in self.last.as_ref().map_or(&[][..], |r| &r.placed[..]) {
                if let Some(n) = self.eng.placed_node(c).filter(|n| n.state() != State::Gone) {
                    m.entry((n.op(), n.args())).or_insert(c);
                }
            }
            self.placed_keys = Some(m);
        }
        self.product.placements.get(i).and_then(|p| {
            let (op, args) = p.key();
            let c = *self.placed_keys.as_ref()?.get(&(op, Arc::from(args)))?;
            let n = self.eng.node(c);
            (n.state() != State::Gone)
                .then(|| n.run_record().map(|r| r.output))
                .flatten()
        })
    }

    /// `op` on `args` inline: the output. It reads at my position and its
    /// reads are mine. A run that read nothing is memoised by (op, args);
    /// one that read depends on where it ran, and runs at each caller.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        self.check()?;
        let key = (op, args.clone().into_boxed_slice());
        if let Some(&x) = self.eng.memo.read().get(&key) {
            return Ok(x);
        }
        let o = self.eng.op(op).clone();
        let mut sub = Ctx::new(self.eng, self.worker, self.me);
        sub.args = args.into();
        let out = o.run(&mut sub)?;
        if sub.product.reads.is_empty() {
            self.eng.memo.write().insert(key, out);
        } else {
            self.product.reads.append(&mut sub.product.reads);
        }
        Ok(out)
    }

    /// Takes my next `n` siblings: they become inert and `following` gives
    /// their (op, args). Pushed siblings only, not placements.
    pub fn claim(&mut self, n: usize) {
        self.me.claim_all.store(false, std::sync::atomic::Ordering::Release);
        self.product.claim = n;
        self.product.claim_all = false;
        crate::commit::apply_claim(self.eng, self.worker, self.me, n);
    }

    /// Claims every text node (`Operation::text`) after this one in its
    /// unit, whatever placed it (not its pushed siblings alone, as `claim`
    /// does): the job is over here (TeX's fatal error, §93: nothing after
    /// it is read). The nodes are inert, their products withdrawn, until a
    /// run of this node claims otherwise; what is not text (the writers
    /// after the text) stays and runs on what was made before. A fatal error in a paper's preamble claimed its one
    /// pushed sibling and left the 11000 pieces of the included files to
    /// run on, 65 times each at ten threads.
    pub fn claim_rest(&mut self) {
        self.me.claim_all.store(true, std::sync::atomic::Ordering::Release);
        self.product.claim = usize::MAX;
        self.product.claim_all = true;
        crate::commit::apply_claim(self.eng, self.worker, self.me, usize::MAX);
    }

    /// The `i`-th claimed sibling's (op, args).
    pub fn following(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        let n = read::following_on(self.eng, self.me, i, &mut self.follow_at);
        let seen = n.as_ref().map(|n| read::text_value(self.eng, n));
        self.record(ReadKind::Following { i }, PathId::WHOLE, seen);
        n.map(|n| (n.op(), n.args().to_vec()))
    }

    /// Whether no `i`-th claimed sibling of operation `op` follows me (none,
    /// or one of another operation): the end of the text `op`'s nodes are
    /// pieces of. Found so, it is a read of that sibling's operation
    /// ([`Ctx::following`] through its op: asked again when a sibling of
    /// `op` comes there, not when another's text is edited); a sibling of
    /// `op` found is no read, as a run that ends before its next sibling
    /// reads nothing of it.
    pub fn ends_before(&mut self, i: usize, op: OpId) -> bool {
        let n = read::following_on(self.eng, self.me, i, &mut self.follow_at);
        if n.as_ref().is_some_and(|n| n.op() == op) {
            return false;
        }
        let path = self.path(&[Step::Index(0)]);
        let seen = read::apply_path(self.eng, n.as_ref().map(|n| read::text_value(self.eng, n)), path);
        self.record(ReadKind::Following { i }, path, seen);
        true
    }

    /// The node of the `i`-th claimed sibling, as the `following(i)`
    /// just made found it (its read recorded there); None otherwise.
    #[must_use]
    pub fn followed(&self, i: usize) -> Option<NodeId> {
        self.follow_at.filter(|x| x.0 == i).map(|x| x.1)
    }

    /// The first node before me on the line that has not committed, or
    /// None when every node before me has run and committed once.
    /// A node found is recorded: the commit files me with it, and its
    /// commit (or its being taken, removed or moved) runs me again. None
    /// is not: a node placed before me later asks nothing of a run that
    /// found the prefix whole. A run that reads
    /// on into what follows it reads a prefix TeX has all of; before
    /// that prefix has run, what the run read of it is a mix of the
    /// definitions some of its nodes have made and others have not.
    pub fn hole_before(&mut self) -> Result<Option<NodeId>, Stop> {
        self.check()?;
        let h = read::on_one_layout(self.eng, || read::hole_before(self.eng, self.me));
        if h.is_some() {
            self.record(ReadKind::Hole, PathId::WHOLE, read::hole_value(self.eng, h));
        }
        Ok(h)
    }

    /// The run stopped at a hole before it ([`Ctx::hole_before`]) with
    /// nothing to say yet: what my last run made stands as this run's
    /// ([`Ctx::unchanged`]), filed with this run's reads (the hole among
    /// them), where I have run before; true then. Taken back instead, its
    /// brackets and definitions left the line until the hole filled and
    /// came back with the next run: each such stop a change of every
    /// reader after me (biblatex at -j4: example pieces flipping between
    /// four scopes open and none, each flip a walk of 3.4 M readers, mc
    /// 80562). Not my last run's state: its checkpoints count that run's
    /// reads, which this record does not hold (a run going on from one,
    /// or cut off at one, rested on reads it never checked: csquotes at
    /// -j8, an example environment's body typeset as text, accl 25423).
    /// False on a first run: nothing to keep.
    pub fn keep_last_at_hole(&mut self) -> bool {
        if self.last.is_none() {
            return false;
        }
        self.product.unchanged = true;
        self.product.at_hole = true;
        true
    }

    /// Stops the run until the nearest piece of the text before me asked
    /// again since it committed ([`read::text_to_run_before`]) commits,
    /// when there is one: nothing is filed, my last run's products stand,
    /// and I run again at that commit or at the quiet point. For a run
    /// that would end the job's text at me (a fatal error, which claims
    /// every node after me): made while the text before me is still to
    /// run, the end rests on a guess of that text, and its claim of the
    /// rest took back every node after me and asked their readers again,
    /// to be undone once the text ran (biblatex at -j4, an example piece
    /// cut from its \end reading on to the input's end: one run of 71 s,
    /// 49 s of it the claim's re-asks, mc 80568, 80574). The page layer is
    /// not waited for, nor a piece's first run: a fatal error on a prefix
    /// every piece of which stands ends the job at once, no piece after
    /// it begun.
    pub fn wait_text_before(&mut self) -> Result<(), Stop> {
        self.check()?;
        if let Some(x) = read::on_one_layout(self.eng, || read::text_to_run_before(self.eng, self.me)) {
            self.wait_on = Some(x);
            return Err(stop());
        }
        Ok(())
    }

    /// [`Ctx::hole_before`] where the text before me can change what I
    /// read: the first node before me that has not committed and whose
    /// operation may define one of `fields` (`Operation::may_define`),
    /// None where no such node is. A node found is recorded as
    /// `hole_before` records the first hole (its commit, or its being
    /// taken, removed or moved, runs me again, and I look again); None is
    /// not recorded.
    pub fn hole_before_defining(&mut self, fields: &[Field]) -> Result<Option<NodeId>, Stop> {
        self.check()?;
        let (h, first) = read::on_one_layout(self.eng, || read::hole_before_defining(self.eng, self.me, fields));
        if h.is_some() {
            self.record(ReadKind::Hole, PathId::WHOLE, read::hole_value(self.eng, first));
        }
        Ok(h)
    }

    /// Whether node `n` stands past the settling layer's frontier
    /// ([`Ctx::set_frontier`]). Not a read.
    pub fn past_frontier(&self, n: NodeId) -> bool {
        self.eng.placed_node(n).is_some_and(|x| {
            crate::sched::past_frontier(
                self.eng,
                x,
                self.eng.frontier.load(std::sync::atomic::Ordering::Acquire),
            )
        })
    }

    /// Whether this evaluation has more than one thread: work done ahead
    /// on a guess can stand beside what it guesses at, not only before it.
    /// Never for what a run makes, only for how far it goes on a guess.
    pub fn has_other_threads(&self) -> bool {
        self.worker.is_some_and(|w| w.shared.n > 1)
    }

    /// Stops the run until I stand at the committed frontier: every node
    /// before me in my unit has committed in this pass with no re-ask
    /// pending (none to run, again or for the first time, none running or
    /// committing), and the settling layer's frontier is not before me
    /// (`Ctx::set_frontier`: the pages before me fired, their finishers
    /// placed before me). For an effect outside the engine that a run on
    /// a guessed state must never make, though the run is thrown away (a
    /// shell command): made past this point, it is made on the state TeX's
    /// own order has here. Stopped, the run is queued again when the node
    /// waited for commits, or when the frontier passes me; the check is
    /// made again then. A node before me that changes after this (a
    /// loop-back's next pass, an edit) re-asks me through what I read.
    pub fn wait_settled(&mut self) -> Result<(), Stop> {
        use std::sync::atomic::Ordering::Acquire;
        self.check()?;
        let eng = self.eng;
        if let Some(x) = read::on_one_layout(eng, || read::unsettled_before(eng, self.me)) {
            self.wait_on = Some(x);
            return Err(stop());
        }
        if crate::sched::past_frontier(eng, self.me, eng.frontier.load(Acquire)) {
            self.wait_frontier = true;
            return Err(stop());
        }
        // (and what I read before that still stands: a node before me that
        // committed after my read and before this check has settled, and
        // its re-ask finds no read of mine filed yet: the effect was made
        // on the old answer, a run's commit too late to take it back.
        // Stopped, the run begins again, every node before it settled)
        if !self.own_reads_hold() {
            return Err(stop());
        }
        Ok(())
    }

    /// Narrows what I may define, for prediction. Accepted, not used by
    /// this core.
    pub fn narrow(&mut self, _m: MayDefine) {}

    /// My scope brackets, when they are known only by running
    /// (`Operation::effect` gave `Pending`). The commit files them; the
    /// readers after me whose answer moved are re-asked.
    pub fn set_effect(&mut self, e: Effect) {
        self.product.effect = Some(e);
    }

    /// The scope depth I start at: scopes open just before me, my own
    /// closes not applied. Recorded: a bracket before me that changes it
    /// runs me again.
    pub fn depth(&mut self) -> Result<u32, Stop> {
        self.check()?;
        let f = self.eng.depth_field;
        self.before_read(f);
        let d = read::on_one_layout(self.eng, || read::depth_of(self.eng, self.me.id)).ok_or_else(stop)?;
        let x = self.eng.values().int(i64::from(d));
        self.record(ReadKind::Field { f, at: Anchor::Me }, PathId::WHOLE, Some(x));
        Ok(d)
    }

    /// Whether a claim took my node while this run was on its way (a
    /// sibling's run claimed it after a worker took it to run). Not a
    /// read.
    pub fn claimed_away(&self) -> bool {
        matches!(self.me.state(), State::Inert | State::RunningInert)
    }

    /// Always true: a run never knows it is final. Side effects belong
    /// after the evaluation, from outputs.
    pub fn is_speculative(&self) -> bool {
        true
    }

    /// The value store's epoch: its instance and the collections it has
    /// made ([`crate::Values::collect`]). An id a client cached at one
    /// epoch is valid while the epoch lasts: one a collection freed is
    /// never handed out again.
    pub fn values_epoch(&self) -> u64 {
        self.eng.values.epoch()
    }

    /// This run publishes nothing new: what my last run defined,
    /// contributed, placed and output stands as it is, and only this
    /// run's reads are filed, so a change to one (a node it stopped at
    /// committing) runs me again. For a run that met what is not there
    /// yet (a stop at a hole) before it could say anything: what it
    /// said before is the best guess until then, and taking it back
    /// would re-ask every reader of it, and remove every placement and
    /// theirs, to put them back once the hole fills. On a first run there
    /// is nothing to keep (a prediction stands as made).
    pub fn unchanged(&mut self) {
        self.product.unchanged = true;
    }

    /// [`Ctx::unchanged`], but this run's own claim stands (none, unless
    /// it claims), not the last run's: what the last run claimed it
    /// decided on what this run could not get. A finisher whose page is
    /// stale keeps what its routine defined on the page before, but not
    /// the claim of the rest a fatal error of that routine made there:
    /// kept, it held the text after the finisher inert, so the page
    /// builder never folded that text again and the page stayed stale
    /// for good, the job ended at that finisher with its hundred errors
    /// in the log (thumbs at -j8: an overview page's routine run on
    /// values not settled yet).
    pub fn unchanged_unclaimed(&mut self) {
        self.product.unchanged = true;
        self.product.own_claim = true;
    }

    /// What this run defines is no decision of its settling layer
    /// (`Operation::settles_at_quiet`) but a step of a serial chain the
    /// text waits on: a changed definition re-asks a reader outside the
    /// layer at once, as it re-asks one inside it, instead of holding the
    /// wake to the next quiet point (docs/spec-settle.md §2). Kept with
    /// the run's record: a run that publishes nothing new
    /// (`Ctx::unchanged`) keeps its last run's.
    pub fn settle_now(&mut self) {
        self.product.settle_now = true;
    }

    /// Stores `state` with this run's record for my next run (`resumed`):
    /// a long loop (the page builder over the whole text) that meets a
    /// hole commits what it has so far, definitions and placements, and
    /// goes on from here when the hole fills, instead of from its start.
    /// The state is a value of mine, no read: nothing re-asks me for it.
    pub fn park(&mut self, state: ValueId) {
        self.product.state = Some(state);
    }

    /// The run stops for the answer to `f` it has not got (a search that
    /// met a contributor not in yet, a definition not made yet): the run
    /// woken by that answer and going on from here (`Ctx::goes_on`) is a
    /// resumption, not a refuted run ([`crate::Report::resumed`]).
    pub fn parked_on(&mut self, f: Field) {
        self.product.parked_on = Some(f);
    }

    /// The run goes on from where my last run stopped and does none of
    /// that run's work again: woken by the answer it parked on
    /// (`Ctx::parked_on`), it is a resumption.
    pub fn goes_on(&mut self) {
        self.goes_on = true;
    }

    /// What my last run stored with `park`, if it did; `None` on a first
    /// run, after a run that stored nothing, and after an edit of my text.
    /// A run that takes it goes on from what the last run read: those
    /// reads are kept with this run's (the ones it reads again replaced),
    /// so a change behind where it resumes still asks it again.
    pub fn resumed(&self) -> Option<ValueId> {
        if matches!(self.woken_by(), Some(Cause::Edited))
            || self.me.state_stale.load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let st = self.last.as_ref().and_then(|r| r.state);
        if st.is_some() {
            self.product.resumed.set(true);
        }
        st
    }

    /// What my last run stored with `park`, for a run of my edited text
    /// (woken by [`Cause::Edited`]) that goes on from a point of the text
    /// the edit left as it was: the client looks for that point, and takes
    /// the state with [`Ctx::goes_on_from_edit`]. `None` for another
    /// cause, a stale state ([`Ctx::resumed`]'s rule) or none stored.
    pub fn parked_before_edit(&self) -> Option<ValueId> {
        if !matches!(self.woken_by(), Some(Cause::Edited))
            || self.me.state_stale.load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        self.last.as_ref().and_then(|r| r.state)
    }

    /// The state my last run stored with `park`, stale or not, as a value
    /// only: taking it neither goes on from it nor keeps any of that
    /// run's reads (for that, [`Ctx::resumed`]).
    pub fn last_state(&self) -> Option<ValueId> {
        self.last.as_ref().and_then(|r| r.state)
    }

    /// This run does not go on from the state [`Ctx::resumed`] offered
    /// after all: its last run's reads are not kept with its own. A run
    /// that took the state, then met what it must wait for and keeps its
    /// last products standing ([`Ctx::unchanged`]), files only what it
    /// read: keeping the last run's reads, one that moved since woke it
    /// again at each commit of the same wait, with no end.
    pub fn not_resumed(&self) {
        self.product.resumed.set(false);
    }

    /// Whether my last run stored a state with `park` (whatever woke me).
    pub fn last_parked(&self) -> bool {
        self.last.as_ref().is_some_and(|r| r.state.is_some())
    }

    /// The run goes on from the state [`Ctx::parked_before_edit`] gave:
    /// its last run's reads are kept with this run's (those it reads
    /// again replaced), as for [`Ctx::resumed`], so a change behind the
    /// point it went on from still asks it again.
    pub fn goes_on_from_edit(&self) {
        self.product.resumed.set(true);
    }

    /// What my last run stored with `park`, whatever woke me (not an
    /// edit: [`Ctx::parked_before_edit`]), for a run that goes on from a
    /// point of it reached before the read that moved
    /// ([`Ctx::last_reads_hold`], [`Ctx::goes_on_after_reads`]). `None`
    /// for an edit, a stale state or none stored.
    pub fn parked_state(&self) -> Option<ValueId> {
        if matches!(self.woken_by(), Some(Cause::Edited))
            || self.me.state_stale.load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        self.last.as_ref().filter(|r| !r.provisional).and_then(|r| r.state)
    }

    /// How many reads this run has made so far: stored with a point of
    /// the run it may go on from next time ([`Ctx::last_reads_hold`]).
    pub fn reads_made(&self) -> usize {
        self.product.kept_reads.get().unwrap_or(0) + self.product.reads.len()
    }

    /// Whether the first `k` reads of my last run all still give what
    /// they gave (`read::holds`): a point of that run reached after them
    /// stands, and a run may go on from there. Not a read.
    pub fn last_reads_hold(&self, k: usize) -> bool {
        let Some(r) = self.last.as_ref().filter(|r| !r.provisional) else {
            return false;
        };
        if k > r.reads.len() {
            return false;
        }
        let mut memo = crate::read::FoldMemo::new();
        r.reads[..k].iter().all(|rd| {
            !matches!(rd.kind, ReadKind::Placed { .. } | ReadKind::Hole)
                && crate::read::holds(self.eng, self.me, rd, &mut memo)
        })
    }

    /// A run woken for a read that moved finds every read of my last run
    /// giving what it gave: a run is a function of its reads, so it would
    /// make what that run made, and that run's definitions, contributions,
    /// placements and output stand ([`Ctx::unchanged`]) with its reads
    /// filed again. What moved moved back, a value on the way among the
    /// nodes before this one that re-ran out of order, and nothing is run:
    /// Grail's last pages' routines ran 3.8 s of hyperxmp on such wakes at
    /// -j8 to make what they had made.
    ///
    /// Not for a node whose last run parked on an answer it had not got
    /// (`Ctx::parked_on`: a page loop at a hole, woken to fold on from
    /// where it stopped); one that ran to its end and parked a state only
    /// for a run after an edit to go on from (a text piece's checkpoint)
    /// made that state of its reads as it made the rest (a run that went
    /// on from a state keeps the reads it rests on), and stands with it
    /// (beamer: every frame after a \label typed between two frames ran
    /// again, and its page's finisher, on the empty
    /// `\beamer@framehypertargets` the first frame had set back; 29
    /// frames, 240 ms at -j1). Not for one relocated (its definitions are filed where it stood:
    /// `commit::relocate`), or refuted by its own commit (the state it
    /// parked rests on what moved, though the reads hold now:
    /// `resumed_reread.rs`), or asked for anything but a read (an edit, a
    /// creation, the structure around it). A hole it met is a read like
    /// the others: filled since, it does not hold. The output of that run,
    /// if it stands.
    pub(crate) fn last_run_stands(&mut self) -> Option<ValueId> {
        if !matches!(
            self.woke,
            Some(
                Cause::Changed { .. }
                    | Cause::Range { .. }
                    | Cause::Rebind { .. }
                    | Cause::Output { .. }
                    | Cause::Environment { .. }
            )
        ) || self.me.moved.load(std::sync::atomic::Ordering::Acquire)
            || self.me.refuted.load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let r = self
            .last
            .as_ref()
            .filter(|r| !r.provisional && (r.state.is_none() || self.me.parked_on.lock().is_none()))?;
        let mut memo = crate::read::FoldMemo::new();
        if !r
            .reads
            .iter()
            .all(|rd| crate::read::holds(self.eng, self.me, rd, &mut memo))
        {
            return None;
        }
        let output = r.output;
        self.product.reads.extend(r.reads.iter().cloned());
        self.product.unchanged = true;
        Some(output)
    }

    /// Whether my last run's reads from its `m`-th on all still give
    /// what they gave, none of them a placement's output or a hole: what
    /// that run made after a point of it reached after `m` reads stands
    /// for a run that reaches that point in the same state
    /// ([`Ctx::goes_on_as_last`]). Not a read.
    pub fn last_reads_hold_from(&self, m: usize) -> bool {
        let Some(r) = self.last.as_ref().filter(|r| !r.provisional) else {
            return false;
        };
        let Some(tail) = r.reads.get(m..) else {
            return false;
        };
        let mut memo = crate::read::FoldMemo::new();
        tail.iter().all(|rd| {
            !matches!(rd.kind, ReadKind::Placed { .. } | ReadKind::Hole)
                && crate::read::holds(self.eng, self.me, rd, &mut memo)
        })
    }

    /// The run reached a point of my last run, after that run's first
    /// `m` reads, in the state that run had there, its reads after it
    /// holding ([`Ctx::last_reads_hold_from`]): it ends there, and what
    /// my last run made stands ([`Ctx::unchanged`]), resting on this
    /// run's reads and that run's after the point. The cutoff.
    pub fn goes_on_as_last(&mut self, m: usize) {
        let Some(r) = self.last.as_ref() else { return };
        let tail = r.reads.get(m..).unwrap_or_default().to_vec();
        self.product.reads.extend(tail);
        self.product.unchanged = true;
    }

    /// `PHITEX_RESUME_CHECK`: whether what this run made so far, its
    /// output `output`, is what my last run made: its definitions (the
    /// last of each field), contributions and output.
    pub fn same_as_last(&self, output: ValueId) -> Result<(), String> {
        let Some(r) = self.last.as_ref().filter(|r| !r.provisional) else {
            return Err("no last run".to_owned());
        };
        let v = &self.eng.values;
        if r.output != output {
            return Err(format!(
                "output {} against {}",
                v.render(output).chars().take(100).collect::<String>(),
                v.render(r.output).chars().take(100).collect::<String>()
            ));
        }
        let cs = crate::commit::combine_dups(self.eng, self.product.contribs.clone());
        if cs != r.contribs {
            let k = cs.iter().zip(&r.contribs).take_while(|(a, b)| a == b).count();
            let name = |x: Option<&(Field, ValueId)>| x.map(|c| v.render(c.0.0)).unwrap_or_default();
            let val = |x: Option<&(Field, ValueId)>| x.map(|c| v.render(c.1)).unwrap_or_default();
            let (a, b) = (val(cs.get(k)), val(r.contribs.get(k)));
            let at = a.bytes().zip(b.bytes()).take_while(|(p, q)| p == q).count();
            let cut = |s: &str| {
                let lo = s.floor_char_boundary(at.saturating_sub(60));
                let hi = s.floor_char_boundary((at + 240).min(s.len()));
                s[lo..hi].to_owned()
            };
            return Err(format!(
                "contribs: {} against {}, apart at {k}: {} / {}: this run's …{}… last run's …{}…",
                cs.len(),
                r.contribs.len(),
                name(cs.get(k)),
                name(r.contribs.get(k)),
                cut(&a),
                cut(&b)
            ));
        }
        let defs = crate::commit::dedup_defs(self.product.defs.clone());
        let mut a: Vec<String> = defs
            .iter()
            .map(|d| {
                format!(
                    "{}{:?}{:?}{}",
                    v.render(d.f.0),
                    d.loc,
                    d.rel,
                    if d.rel.is_some() { 0 } else { d.value.raw() }
                )
            })
            .collect();
        let mut b: Vec<String> = r
            .defs
            .iter()
            .map(|d| {
                format!(
                    "{}{:?}{:?}{}",
                    v.render(d.f.0),
                    d.loc,
                    d.rel,
                    if d.rel.is_some() { 0 } else { d.value.raw() }
                )
            })
            .collect();
        a.sort();
        b.sort();
        if a != b {
            let only_a: Vec<&String> = a.iter().filter(|x| !b.contains(x)).take(3).collect();
            let only_b: Vec<&String> = b.iter().filter(|x| !a.contains(x)).take(3).collect();
            return Err(format!("defs: this run's {only_a:?} last run's {only_b:?}"));
        }
        Ok(())
    }

    /// `PHITEX_RESUME_CHECK`: the fields my last run defined.
    pub fn last_def_fields(&self) -> Vec<Field> {
        self.last
            .as_ref()
            .map(|r| r.defs.iter().map(|d| d.f).collect())
            .unwrap_or_default()
    }

    /// `PHITEX_RESUME_CHECK`: my last run's and this run's reads of `f`
    /// and definitions of it, as one line.
    pub fn reads_of(&self, f: Field) -> String {
        let v = &self.eng.values;
        let show = |rs: &[ReadRec]| -> Vec<String> {
            rs.iter()
                .enumerate()
                .filter(|(_, r)| r.kind.field() == Some(f))
                .map(|(i, r)| format!("#{i} {:?} {:?}", r.kind, r.seen.map(|x| v.render(x))))
                .collect()
        };
        let last = self.last.as_ref();
        format!(
            "last run reads {:?} defs {:?}; this run reads {:?} defs {:?}",
            last.map(|r| show(&r.reads)).unwrap_or_default(),
            last.map(|r| r
                .defs
                .iter()
                .filter(|d| d.f == f)
                .map(|d| format!("{:?} {:?} {}", d.loc, d.rel, v.render(d.value)))
                .collect::<Vec<_>>())
                .unwrap_or_default(),
            show(&self.product.reads),
            self.product
                .defs
                .iter()
                .filter(|d| d.f == f)
                .map(|d| format!("{:?} {:?} {}", d.loc, d.rel, v.render(d.value)))
                .collect::<Vec<_>>()
        )
    }

    /// The first of my last run's first `k` reads that no longer gives
    /// what it gave, as one line (`PHITEX_CK_TRACE`: why a run asked
    /// again goes on from no checkpoint).
    pub fn first_moved_read(&self, k: usize) -> Option<String> {
        let r = self.last.as_ref()?;
        let v = &self.eng.values;
        let mut memo = crate::read::FoldMemo::new();
        r.reads.iter().take(k).enumerate().find_map(|(i, rd)| {
            let bad = matches!(rd.kind, ReadKind::Placed { .. } | ReadKind::Hole)
                || !crate::read::holds(self.eng, self.me, rd, &mut memo);
            bad.then(|| {
                let f = rd.kind.field().map(|f| v.render(f.0)).unwrap_or_default();
                format!("read {i} of {k}: {f} {:?}", rd.kind)
            })
        })
    }

    /// The run goes on from a point of my last run reached after its
    /// first `k` reads (which hold, [`Ctx::last_reads_hold`]): those
    /// reads stand with this run's (those it reads again replaced), the
    /// rest of the last run's are dropped.
    pub fn goes_on_after_reads(&self, k: usize) {
        self.product.resumed.set(true);
        self.product.kept_reads.set(Some(k));
    }

    /// Whether node `a` stands before node `b` on the line (both alive). A
    /// loop resumed after a contribution moved asks whether the mover lies
    /// behind its cursor (then it goes back to the checkpoint before it)
    /// or ahead (then it goes on). Not a read.
    pub fn before(&self, a: NodeId, b: NodeId) -> bool {
        self.eng.node(a).pos_hint() < self.eng.node(b).pos_hint()
    }

    /// Whether node `n`, where it stands, closes a scope after its reads
    /// and definitions: what it ran in is out of sight from a node after
    /// it. Not a read: a change of a node's brackets re-asks the readers it
    /// reaches past.
    pub fn closes_scope_after(&self, n: NodeId) -> bool {
        self.eng.placed_node(n).is_some_and(|x| {
            let b = x.record().map_or_else(
                || crate::node::unpack_effect(x.effect.load(std::sync::atomic::Ordering::Acquire)).0,
                |r| r.effect.brackets(),
            );
            b.closes > 0 && !b.before
        })
    }

    /// Node `n`'s operation, where it stands (placed, not removed). Not a
    /// read.
    pub fn op_of(&self, n: NodeId) -> Option<OpId> {
        self.eng
            .placed_node(n)
            .filter(|x| x.state() != State::Gone)
            .map(|x| x.op())
    }

    /// Whether the node that placed me has nothing more to commit: clean
    /// on a run of its own (`sched::settled_node`). A node it placed that
    /// reads what that one read after it (a page's finisher, the page's
    /// pieces as they are now) may meet them ahead of it: what its placer
    /// made it from has moved, and its placer, re-asked, runs again and
    /// places it anew or not at all.
    pub fn placer_settled(&self) -> bool {
        self.me.parent.is_none_or(|p| crate::sched::settled_node(self.eng, p))
    }

    /// Stops the run until the node that placed me commits again
    /// ([`Ctx::placer_settled`] false): queued then, or at the quiet point.
    pub fn wait_for_placer(&mut self) -> Stop {
        self.wait_on = self.me.parent;
        stop()
    }

    /// Whether node `n` still stands (placed, not removed): a run about
    /// to place a node after `n` (`follow_after`) asks first. Not a read.
    pub fn alive(&self, n: NodeId) -> bool {
        self.eng.placed_node(n).is_some_and(|x| x.state() != State::Gone)
    }

    /// Why this run was asked for: the cause that marked me dirty (a
    /// definition or contribution I read that moved, with its site; my
    /// creation; an edit). A loop resumes from the checkpoint before the
    /// site. `None` where the engine recorded none.
    pub fn woken_by(&self) -> Option<Cause> {
        self.woke.clone()
    }

    /// Forgets my reads of `f` in this run: the commit records none.
    pub fn unread(&mut self, f: Field) {
        self.seen.remove(&f);
        let mut i = 0;
        let log = &mut self.unread_log;
        self.product.reads.retain(|r| {
            let keep = match &r.kind {
                ReadKind::Field { f: g, .. }
                | ReadKind::Env { f: g }
                | ReadKind::Fold { f: g, .. }
                | ReadKind::Next { f: g, .. } => *g != f,
                ReadKind::Placed { .. } | ReadKind::Following { .. } | ReadKind::Hole => true,
            };
            if keep {
                i += 1;
            } else {
                log.push(i);
            }
            keep
        });
    }

    /// Starts the run's product again, as at its start (its wake and
    /// last run kept): a check that runs the node again whole after a
    /// run that went on from a checkpoint (`PHITEX_RESUME_CHECK`), so the
    /// whole run's product is what is committed.
    pub fn begin_again(&mut self) {
        let scope_gen = self.product.scope_gen;
        let fresh = Ctx::new(self.eng, self.worker, self.me);
        self.product = fresh.product;
        self.product.scope_gen = scope_gen;
        self.views.clear();
        self.follow_at = None;
        self.seen.clear();
        self.unread_log.clear();
        self.stop_at = None;
        self.placed_keys = None;
        self.stuck = None;
        self.wait_on = None;
        self.wait_frontier = false;
        self.goes_on = false;
    }

    /// `PHITEX_RESUME_CHECK`: the run's reads so far kept aside, with
    /// the last run's it rests on ([`Ctx::stashed_hold`]).
    pub fn stash_reads(&mut self) {
        let mut xs: Vec<ReadRec> = match (self.product.kept_reads.get(), self.last.as_ref()) {
            (Some(k), Some(r)) => r.reads[..k.min(r.reads.len())].to_vec(),
            _ => Vec::new(),
        };
        xs.extend(self.product.reads.iter().cloned());
        self.stashed = xs;
    }

    /// Whether the reads [`Ctx::stash_reads`] kept aside, and this run's,
    /// all still give what they gave: runs on them, made now, would make
    /// what they made.
    pub fn stashed_hold(&self) -> bool {
        let mut memo = crate::read::FoldMemo::new();
        self.stashed.iter().chain(&self.product.reads).all(|rd| {
            matches!(rd.kind, ReadKind::Placed { .. } | ReadKind::Hole)
                || crate::read::holds(self.eng, self.me, rd, &mut memo)
        })
    }

    /// Whether this run's reads so far all still give what they gave,
    /// and no field it read has changed since, even back
    /// (`PHITEX_RESUME_CHECK`: at -j8 a field can move and move back
    /// while a run reads it in two places).
    pub fn own_reads_hold(&self) -> bool {
        let mut memo = crate::read::FoldMemo::new();
        self.product.reads.iter().all(|rd| {
            matches!(rd.kind, ReadKind::Placed { .. } | ReadKind::Hole)
                || (crate::read::holds(self.eng, self.me, rd, &mut memo)
                    && rd
                        .kind
                        .field()
                        .is_none_or(|f| rd.epoch == u64::MAX || self.eng.field_epoch(f) == rd.epoch))
        })
    }

    /// The first of my last run's first `k` reads this run's reads
    /// differ from, in order (kind, path, answer), as one line; `None`
    /// where they all agree (`PHITEX_RESUME_CHECK`: what a whole run read
    /// before a checkpoint a run went on from, against what that
    /// checkpoint's run read).
    pub fn first_read_apart(&self, k: usize) -> Option<String> {
        let last = self.last.as_ref()?;
        let v = &self.eng.values;
        let show = |r: &ReadRec| {
            let f = r.kind.field().map(|f| v.render(f.0)).unwrap_or_default();
            let seen = r.seen.map(|x| {
                let s = v.render(x);
                s.chars().take(120).collect::<String>()
            });
            format!("{f} {:?} seen {seen:?}", r.kind)
        };
        let n = k.min(last.reads.len());
        for i in 0..n {
            let Some(b) = self.product.reads.get(i) else {
                return Some(format!("read {i} of {n}: the whole run made only {i}"));
            };
            let a = &last.reads[i];
            if a.kind != b.kind || a.path != b.path || a.seen != b.seen {
                return Some(format!("read {i} of {n}: before {} | whole {}", show(a), show(b)));
            }
        }
        None
    }

    /// The run took a checkpoint: a read of a field after it is filed
    /// again, not answered from [`Ctx::read`]'s memo of a read before it.
    /// The cutoff ([`Ctx::last_reads_hold_from`]) checks only the reads
    /// after the checkpoint: what a run's rest learns from a read before
    /// it has to be among them, or in the state the checkpoint compares.
    pub fn reads_begin_again(&mut self) {
        self.seen.clear();
    }

    /// How many reads `unread` has taken out so far (with a count of
    /// [`Ctx::reads_made`], for [`Ctx::reads_kept_of`]).
    pub fn unread_count(&self) -> usize {
        self.unread_log.len()
    }

    /// A count of reads made before a point of this run
    /// ([`Ctx::reads_made`], taken then) as the record will have them:
    /// less those `unread` took out since from before it.
    /// (`at`: `unread`'s count of take-outs then, [`Ctx::unread_count`]).
    pub fn reads_kept_of(&self, made: usize, at: usize) -> usize {
        let kept = self.product.kept_reads.get().unwrap_or(0);
        let mut c = made;
        for &i in self.unread_log.get(at..).unwrap_or_default() {
            if kept + i < c {
                c -= 1;
            }
        }
        c
    }

    /// A path interned for `read_path` and friends.
    pub fn path(&self, steps: &[Step]) -> PathId {
        self.eng.path(steps)
    }

    /// The run's periodic check (a client calls it every so much work,
    /// a TeX run every 4096 commands): `Stop` when the run is refuted
    /// (`RunningDirty`); and from its 64th check on, a long run, when one
    /// of its reads so far no longer holds, looked at at every power of
    /// two and at the next check after a re-ask came while it ran
    /// (`sched::mark_stale` leaves a running node to its commit). A run
    /// on what moved may never end (a TeX loop on a stale page or float
    /// state) and so never reach the commit that would refute it, and a
    /// run's first reads are filed nowhere a walk could re-ask it by. A
    /// run whose reads all hold goes on; a short run is left to its
    /// commit, as before.
    pub fn tick(&mut self, _n: u64) -> Result<(), Stop> {
        self.check()?;
        // (what waits behind a run gone long)
        if let Some(w) = self.worker {
            w.offer();
        }
        self.ticks += 1;
        if self.ticks >= LONG_RUN_TICKS
            && (self.ticks.is_power_of_two() || self.me.stale.load(std::sync::atomic::Ordering::Acquire))
        {
            self.recheck()?;
        }
        Ok(())
    }

    /// [`Ctx::tick`]'s look at the reads: the run marked dirty (for the
    /// re-ask that came, if one) and stopped if one moved; as the
    /// commit's check, a field no change has counted since it was read
    /// stands, and a sibling's read is left to the commit.
    #[cold]
    fn recheck(&mut self) -> Result<(), Stop> {
        use std::sync::atomic::Ordering;
        self.me.stale.store(false, Ordering::Release);
        let wake = self.me.stale_wake.lock().take();
        let scope_moved =
            read::scoped(self.eng) && self.eng.scope_gen.load(Ordering::Acquire) != self.product.scope_gen;
        let mut memo = crate::read::FoldMemo::new();
        // (a read of a placement's output is of this run's `i`-th placement,
        // which the last run's `i`-th need not be: a pass that placed a
        // child its last run did not (the read-back child after a pass
        // read back inline) reads another node's output there. So the
        // child this placement stands for, by its key, as the read found
        // it: its output now, or none where it has not run (a guess, which
        // its commit checks, `reask_parent`). Read through `holds`, a
        // read moved on every check, the run stopped each time and never
        // committed, and its child never ran: no progress)
        let placed: Vec<usize> = self
            .product
            .reads
            .iter()
            .filter_map(|rd| match rd.kind {
                ReadKind::Placed { i, .. } => Some(i),
                _ => None,
            })
            .collect();
        let mut outs: FxHashMap<usize, Option<ValueId>> = FxHashMap::default();
        for i in placed {
            if let std::collections::hash_map::Entry::Vacant(e) = outs.entry(i) {
                e.insert(self.placed_child_output(i));
            }
        }
        let moved = self.product.reads.iter().any(|rd| {
            if matches!(rd.kind, ReadKind::Following { .. }) {
                return false;
            }
            if let ReadKind::Placed { i, guess } = rd.kind {
                return match outs.get(&i).copied().flatten() {
                    Some(o) => read::apply_path(self.eng, Some(o), rd.path) != rd.seen,
                    // (no child of that key has run: a guess holds until
                    // the child commits; a real read of none saw none)
                    None => !guess && rd.seen.is_some(),
                };
            }
            if !scope_moved
                && !matches!(rd.kind, ReadKind::Next { .. })
                && rd.kind.field().is_some_and(|f| self.eng.field_epoch(f) == rd.epoch)
            {
                return false;
            }
            !crate::read::holds(self.eng, self.me, rd, &mut memo)
        });
        if !moved {
            return Ok(());
        }
        match wake {
            Some(wake) => {
                crate::sched::mark_dirty_now(self.eng, self.worker, self.me, wake);
            }
            None => {
                let _ = self.me.cas_state(State::Running, State::RunningDirty);
            }
        }
        if let Some(w) = self.worker {
            Worker::add(&w.stopped_stale, 1);
        }
        Err(stop())
    }

    /// The run makes no progress (`why`, the client's measure against
    /// `Limits::stall` and `Limits::memory_bytes`): the `Stop` to return,
    /// and the evaluation ends with `EvalError::Stuck` naming this node.
    pub fn stuck(&mut self, why: String) -> Stop {
        self.stuck = Some(why);
        stop()
    }

    /// The engine's limits (`Engine::set_limits`).
    pub fn limits(&self) -> crate::Limits {
        self.eng.limits()
    }

    /// How long the evaluation running this has been going (`None` for a
    /// run outside one): a run checks `Limits::time` itself, which the
    /// scheduler checks only between runs.
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.worker.map(|w| w.shared.t0.elapsed())
    }
}
