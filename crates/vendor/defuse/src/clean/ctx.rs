//! A run's view of a world: every read is asked of one immutable world and
//! recorded as a predicate; every write goes into the run's result.

use std::any::Any;
use std::sync::Arc;

use super::core::Core;
use super::world::{Def, FieldCell, FrameId, Id, LastReads, NONE, Place, Pos, Read, St, World};
use crate::api::{Effect, FieldAt, Locality, NodeId, OpId, Stop};
use crate::values::FxHashMap;
use crate::values::{Field, ValueId, Values};

/// A placement a run made, before the merge gives it an id.
/// The placements a run's earlier attempts made: children, continuations,
/// the children that are not scope markers.
pub(crate) type Base = (usize, usize, usize);

/// A stopped run's kept state: its token (`Ctx::park`), the reads its
/// attempts made (it resumes while they hold), its placements so far.
/// (The placements only when it parked continuing: `Ctx::park_continuing`.)
pub(crate) type KeptState = (Box<dyn Any + Send>, Vec<Read>, Option<Base>);

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Spec {
    pub op: OpId,
    pub args: Arc<[ValueId]>,
    pub place: Place,
}

/// Why a run stopped: what it waits on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Wait {
    /// A node in work: its result (a definition, an output, a structure).
    Node(Id, Option<Field>),
    /// A child this run places, not yet made: the run's unfolding is
    /// committed and the run waits for the child.
    Child(usize),
    /// An environment field neither supplied nor declared absent.
    Env(Field),
}

/// A value read through a view ([`Ctx::view`], [`Ctx::acc_view`]): the
/// run depends on what it inspects of it ([`Ctx::at`], [`Ctx::get`],
/// [`Ctx::len`], [`Ctx::has`], [`Ctx::whole`]), path by path, not on the
/// value. A handle: valid in the run attempt that made it (not across a
/// park or a restart).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct View(u32);

/// A part of a value composed by [`Ctx::compose`]: a value of the run's
/// own, or a view's value (another composition's included), passed on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub enum Part {
    Val(ValueId),
    View(View),
}

/// A run resumed at one of its last run's marks (`Ctx::resumed`): that
/// run's kept token and result, and the mark (its index in the result's
/// marks).
pub(crate) struct MarkResume {
    pub after: Arc<dyn Any + Send + Sync>,
    pub old: Arc<super::world::Res>,
    pub j: usize,
    /// The placements that run made before the mark: the resumed run's
    /// first ones.
    pub kids: Vec<Spec>,
    pub conts: Vec<Spec>,
}

/// A parked token with the views its attempt made (they stand when the
/// next attempt resumes it).
pub(crate) struct KeptViews {
    pub token: Box<dyn Any + Send>,
    pub views: Vec<(u32, u32, ValueId)>,
    pub composed: Vec<Vec<super::world::CPart>>,
}

/// What a run took by index and by key through one view (root read,
/// path) since its last mark.
pub(crate) struct Gets {
    root: u32,
    path: u32,
    ix: Vec<u32>,
    keys: Vec<ValueId>,
}

/// A run's view of the store.
pub struct Ctx<'a> {
    pub(crate) core: &'a Core,
    pub(crate) w: &'a World,
    pub(crate) me: Id,
    pub(crate) unit: u32,
    pub(crate) op: OpId,
    pub(crate) args: Arc<[ValueId]>,
    pub(crate) reads: Vec<Read>,
    pub(crate) defs: Vec<Def>,
    /// Its definitions forwarding a loop-back field (`define_forward`).
    pub(crate) forwards: Vec<(Field, Locality, Field)>,
    /// The views this attempt made ([`View`]): each one's root read (its
    /// index in `reads`; `u32::MAX`: none, a constant), path and value.
    pub(crate) views: Vec<(u32, u32, ValueId)>,
    /// The compositions its views name (`Ctx::compose`).
    pub(crate) composed: Vec<Vec<super::world::CPart>>,
    /// What it took by index and by key through each view since the last
    /// mark ([`Ctx::get`]), read as one at the next (`flush_gets`).
    pub(crate) gets: Vec<Gets>,
    /// The lists it took elements of by index lately (a few, most recent
    /// last): a get looks its list up once.
    lists: Vec<(ValueId, Arc<crate::values::Value>)>,
    /// The marks this attempt made, the token it keeps for a later run
    /// (`checkpoint`, `keep_after`), the run it resumes from a mark of
    /// (`resumed`), and the old mark it was cut off at (its index).
    pub(crate) marks: Vec<super::world::Mark>,
    /// The values it passed on through views (`define_from` and the
    /// others).
    pub(crate) passes: Vec<super::world::Pass>,
    /// The output `output_from` set from a view not passed (read in full).
    pub(crate) out_from: Option<ValueId>,
    pub(crate) after: Option<super::world::After>,
    pub(crate) from_mark: Option<MarkResume>,
    pub(crate) cutoff: Option<(usize, Option<Vec<(Field, Locality, Option<ValueId>)>>)>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) pubs: Vec<(Field, ValueId)>,
    pub(crate) kids: Vec<Spec>,
    pub(crate) conts: Vec<Spec>,
    pub(crate) effect: Option<Effect>,
    pub(crate) kinds: Vec<ValueId>,
    pub(crate) diag: Vec<ValueId>,
    pub(crate) wait: Option<Wait>,
    pub(crate) kept: Option<Box<dyn Any + Send>>,
    /// The kept token was parked continuing (`park_continuing`).
    pub(crate) park_cont: bool,
    /// Resumed from a kept state.
    pub(crate) resumed: bool,
    /// A resumed run continues: the placements its earlier attempts made
    /// stand (the first of its committed ones), and it places after them
    /// (their counts: children, continuations, the children that are not
    /// scope markers).
    pub(crate) base: Option<Base>,
    /// A resumed run named a node that is gone (its kept state is of a
    /// world before): it runs again from the start.
    pub(crate) named_gone: bool,
    pub(crate) restart: bool,
    pub(crate) error: Option<crate::api::EvalError>,
    /// The run's claim (`claim`).
    pub(crate) claim: Option<usize>,
    /// Applications this run asked for that are not made yet.
    pub(crate) apps: Vec<(Id, OpId, Arc<[ValueId]>)>,
    /// Environment fields set by an application's run (`set_env`).
    pub(crate) env: Vec<(Field, ValueId)>,
    /// Where each definition, publication, contribution and environment
    /// field is in its list; the children unfolded (markers aside).
    ix: FxHashMap<(u8, Field, u8), usize>,
    /// Each definition read made (field, at, after, projection): its
    /// entry in `reads`. The world a run sees does not change, so the same
    /// question asked again has the same answer and is not logged again.
    read_ix: FxHashMap<(Field, Id, bool, bool, u32), usize>,
    /// The first path read of each field at each point: another path's
    /// read there takes its value without asking again (a whole read's is
    /// in `read_ix`).
    raw_ix: FxHashMap<(Field, Id, bool, bool), usize>,
    pub(crate) nkids: usize,
    /// The run stopped on a pending definition set.
    pub(crate) pset_wait: bool,
    /// Reads of a definer still to run may take its predicted value.
    pub(crate) predict: bool,
    /// Committed values only: a read of a definer in work waits for it.
    pub(crate) careful: bool,
    /// It read a definer in work (a guess the merge checks).
    pub(crate) guessed: bool,
    /// It folded contributors in work as they last contributed
    /// (`Monoid::guess_ran`).
    pub(crate) fold_ran: bool,
    /// Reads answered by a prediction.
    pub(crate) predicted: u32,
    /// Its last result from a previous evaluation's definition reads of
    /// the fields guessed from the last (`Engine::guess_from_last`): taken
    /// on first need.
    last: Option<Option<Arc<LastReads>>>,
    /// Reads it guessed from the last.
    pub(crate) from_last: u32,
    /// A speculative run's read budget; spent, the run stops (`fuel_out`).
    pub(crate) fuel: Option<u64>,
    pub(crate) fuel_out: bool,
    /// The range a stopped run waits on (field, from, to).
    pub(crate) range_wait: Option<(Field, Id, Id, bool)>,
    /// The bound the run narrowed its pending set to.
    pub(crate) narrowed: Option<super::world::May>,
    /// The calls of a recorded run (`observe`; `RecOp::run` collects them).
    #[cfg(feature = "observe")]
    pub(crate) calls: std::cell::RefCell<Option<crate::record::Calls>>,
    /// What the run's reads look up again and again in its one world: the
    /// last read point asked (with the fuel a read there spends), the
    /// frame chain of the last frame read in, and field cells.
    point: Option<((Id, bool, bool), Option<(u32, Pos, FrameId, Id)>, u64)>,
    chain: Option<((u32, FrameId), Vec<(FrameId, Pos)>)>,
    cells: FxHashMap<(u32, Field), Option<&'a FieldCell>>,
    /// The last pending-set entry before the last read position asked
    /// (`World::last_pset_below`): reads there whose definition is past it
    /// wait on none.
    pset_near: Option<((u32, Pos), Option<Pos>)>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(core: &'a Core, w: &'a World, me: Id, kept: Option<Box<dyn Any + Send>>) -> Ctx<'a> {
        let n = w.node(me).expect("a live node");
        // (A token kept with the stopped attempt's views: they stand.)
        let (kept, views, composed) = match kept.map(|k| k.downcast::<KeptViews>()) {
            Some(Ok(kv)) => (Some(kv.token), kv.views, kv.composed),
            Some(Err(k)) => (Some(k), Vec::new(), Vec::new()),
            None => (None, Vec::new(), Vec::new()),
        };
        // (Resumed at a mark: no client token, the mark's.)
        let (kept, from_mark) = match kept.map(|k| k.downcast::<MarkResume>()) {
            Some(Ok(m)) => (None, Some(*m)),
            Some(Err(k)) => (Some(k), None),
            None => (None, None),
        };
        // (Resumed at a mark: the placements before it stand.)
        let (kids0, conts0) = from_mark.as_ref().map_or_else(Default::default, |m| (m.kids.clone(), m.conts.clone()));
        let nkids = kids0.len();
        // (About as many reads as its last run made, or its operation's.)
        let hint = n.res.as_ref().map_or(0, |r| r.reads.len()).max(core.op_reads.get(n.text.op.0 as usize).map_or(0, |a| a.load(std::sync::atomic::Ordering::Relaxed) as usize));
        Ctx {
            core,
            w,
            me,
            unit: n.unit,
            op: n.text.op,
            args: n.text.args.clone(),
            reads: Vec::with_capacity(hint),
            defs: Vec::new(),
            forwards: Vec::new(),
            views,
            composed,
            gets: Vec::new(),
            lists: Vec::new(),
            // (Resumed at a mark: the marks before it stand.)
            passes: Vec::new(),
            out_from: None,
            marks: from_mark.as_ref().map_or_else(Vec::new, |m| m.old.marks[..m.j].to_vec()),
            after: None,
            from_mark,
            cutoff: None,
            contribs: Vec::new(),
            pubs: Vec::new(),
            kids: kids0,
            conts: conts0,
            effect: None,
            kinds: Vec::new(),
            diag: Vec::new(),
            wait: None,
            resumed: kept.is_some(),
            base: None,
            named_gone: false,
            kept,
            park_cont: false,
            restart: false,
            error: None,
            claim: None,
            apps: Vec::new(),
            env: Vec::new(),
            ix: FxHashMap::default(),
            read_ix: FxHashMap::with_capacity_and_hasher(hint, Default::default()),
            raw_ix: FxHashMap::default(),
            nkids,
            pset_wait: false,
            predict: false,
            careful: false,
            guessed: false,
            fold_ran: false,
            predicted: 0,
            last: None,
            from_last: 0,
            fuel: None,
            fuel_out: false,
            range_wait: None,
            narrowed: None,
            #[cfg(feature = "observe")]
            calls: std::cell::RefCell::new(None),
            point: None,
            chain: None,
            cells: FxHashMap::with_capacity_and_hasher(hint / 2, Default::default()),
            pset_near: None,
        }
    }

    /// Spends `n` of a speculative run's fuel; spent out, the run stops.
    fn spend(&mut self, n: u64) -> Result<(), Stop> {
        if let Some(fuel) = &mut self.fuel {
            if *fuel < n {
                *fuel = 0;
                self.fuel_out = true;
                return Err(self.stop(Wait::Node(self.me, None)));
            }
            *fuel -= n;
        }
        Ok(())
    }

    #[track_caller]
    fn stop(&mut self, w: Wait) -> Stop {
        if super::core::debug() {
            // (Where it stopped, and the subject as this run's world has it.)
            let x = match w {
                Wait::Node(x, _) => Some(x),
                _ => None,
            };
            eprintln!(
                "stopped {} at ctx.rs:{} on {w:?}: {:?} (op {:?}, reads hinted {:?}, subject result {:?})",
                self.me,
                std::panic::Location::caller().line(),
                x.and_then(|x| self.w.node(x)).map(|n| (n.st, n.pos)),
                self.w.node(self.me).map(|n| self.core.op_name(n.text.op)),
                self.w.node(self.me).map(|n| n.text.stat.reads.is_some()),
                x.and_then(|x| self.w.node(x)).map(|n| n.res.is_some()),
            );
        }
        self.wait = Some(w);
        Stop { _private: () }
    }

    #[must_use]
    pub fn values(&self) -> &'a Values {
        &self.core.values
    }

    #[must_use]
    pub fn args(&self) -> Vec<ValueId> {
        self.args.to_vec()
    }

    #[must_use]
    pub fn op(&self) -> OpId {
        self.op
    }

    #[must_use]
    pub fn node(&self) -> NodeId {
        NodeId(self.me)
    }

    /// The value of `f` reaching this node: the nearest earlier visible
    /// definition, a loop-back field's round value, or the environment.
    ///
    /// # Errors
    ///
    /// `Stop` when the answer's subject is still in work.
    pub fn use_field(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.use_field_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        Ok(self.read_at(f, self.me, false, false, 0)?.map(|x| x.0))
    }

    /// Reads `f` at node `at` (before it, or after its subtree when
    /// `after`): the value and the site.
    fn read_at(&mut self, f: Field, at: Id, after: bool, own: bool, path: u32) -> Result<Option<(ValueId, Id)>, Stop> {
        let key = (f, at, after, own, path);
        if let Some(&i) = self.read_ix.get(&key) {
            match self.reads[i] {
                Read::Use { site, val, .. } => return Ok(val.map(|v| (v, site))),
                Read::Loop { val, .. } => return Ok(Some((val, NONE))),
                _ => {}
            }
        }
        // (Plain and projection reads: as ever. A path's read: out of line.)
        if path >= super::core::PATH0 {
            return self.read_at_path(f, at, after, own, path);
        }
        let t = super::core::prof::start();
        let before = self.reads.len();
        let r = self.read_at_inner(f, at, after, own, path);
        if r.is_ok() && self.reads.len() == before + 1 {
            self.read_ix.insert(key, before);
        }
        super::core::prof::stop(18, t);
        r
    }

    /// [`Self::read_at`] through a path (`PATH0` on), not read at this key
    /// yet: at a point read already, whole or through another path, that
    /// read's value, read again through this one.
    #[inline(never)]
    fn read_at_path(&mut self, f: Field, at: Id, after: bool, own: bool, path: u32) -> Result<Option<(ValueId, Id)>, Stop> {
        let key = (f, at, after, own, path);
        if let Some(&i) = self.read_ix.get(&(f, at, after, own, 0)).or_else(|| self.raw_ix.get(&(f, at, after, own)))
            && let Read::Use { site, val, .. } = self.reads[i]
        {
            self.spend(1)?;
            self.read_ix.insert(key, self.reads.len());
            self.reads.push(Read::Use { f, at, after, own, site, val, path });
            return Ok(val.map(|v| (v, site)));
        }
        let before = self.reads.len();
        let r = self.read_at_inner(f, at, after, own, path);
        if r.is_ok() && self.reads.len() == before + 1 {
            self.read_ix.insert(key, before);
            if matches!(self.reads[before], Read::Use { .. }) {
                self.raw_ix.entry((f, at, after, own)).or_insert(before);
            }
        }
        r
    }

    fn read_at_inner(&mut self, f: Field, at: Id, after: bool, own: bool, path: u32) -> Result<Option<(ValueId, Id)>, Stop> {
        // (Fuel: one, and one per frame the read walks.)
        let w = self.w;
        let key = (at, after, own);
        let (point, depth) = match &self.point {
            Some((k, p, d)) if *k == key => (*p, *d),
            _ => {
                let d = w.node(at).map_or(1, |n| w.unit(n.unit).frames.get(&n.frame).map_or(1, |fr| u64::from(fr.depth) + 1));
                let p = w.read_point(at, after, own);
                self.point = Some((key, p, d));
                (p, d)
            }
        };
        self.spend(depth)?;
        if let Some(&lv) = w.loops.get(&f) {
            self.reads.push(Read::Loop { f, val: lv });
            return Ok(Some((lv, NONE)));
        }
        let Some((u, pos, fr, skip)) = point else {
            return Ok(None);
        };
        let reach = super::core::prof::time(21, || {
            let cell = *self.cells.entry((u, f)).or_insert_with(|| w.field(u, f));
            let cell = cell?;
            if self.chain.as_ref().is_none_or(|c| c.0 != (u, fr)) {
                self.chain = Some(((u, fr), w.chain_starts(u, fr)));
            }
            World::reaching_in(&cell.defs, &self.chain.as_ref().expect("set").1, pos, skip)
        });
        // A pending definition set between the site and the reader: wait
        // (speculating, the read guesses past it; a definition it makes
        // re-asks the reader).
        let from = reach.map_or(0, |r| r.0);
        if (!self.predict || self.core.wait_pending.contains(&f))
            && self.pset_after(u, pos, from)
            && let Some(x) = super::core::prof::time(19, || self.w.pending_set_wait(self.core, u, f, from, pos, self.me))
        {
            if super::core::debug() && self.w.node(x).is_some_and(|n| !matches!(n.st, St::Dirty | St::Held)) {
                eprintln!("read {} of {f:?}: pending set of {x} in state {:?}", self.me, self.w.node(x).map(|n| n.st));
            }
            self.pset_wait = true;
            return Err(self.stop(Wait::Node(x, Some(f))));
        }
        match reach {
            Some((_, e)) => {
                let site = e.node;
                let val = match e.val {
                    Some(v) => Some(v),
                    None if self.predict
                        && let Some(v) = self
                            .w
                            .node(site)
                            .and_then(|n| n.text.stat.preds.as_ref())
                            .and_then(|ps| ps.iter().find(|p| p.0 == f).map(|p| p.1)) =>
                    {
                        // The definer's prediction, as a guess the merge
                        // checks.
                        self.predicted += 1;
                        Some(v)
                    }
                    None if self.predict && !self.careful && let Some(v) = self.last_read(f, key) => {
                        // What its last evaluation read here, likewise.
                        self.predicted += 1;
                        self.from_last += 1;
                        Some(v)
                    }
                    None => {
                        if super::core::debug() && self.w.node(site).is_some_and(|n| matches!(n.st, St::Inert(_) | St::Done)) {
                            eprintln!("site {site} {:?} frame {:?} pos {:?}; reader frame {fr:?}", self.w.node(site).map(|n| n.st), self.w.node(site).map(|n| n.frame), self.w.node(site).map(|n| n.pos));
                        }
                        if super::core::debug() && self.w.node(site).is_some_and(|n| matches!(n.st, St::Inert(_))) {
                            eprintln!("read {} of {f:?} at {at}: pending entry of inert {site} in frame {fr:?} entry {e:?}", self.me);
                        }
                        // Declared, not yet made: wait for the definer.
                        return Err(self.stop(Wait::Node(site, Some(f))));
                    }
                };
                if self.in_work(site) && site != self.me {
                    // (A definer in work holds its last result: taken as a
                    // guess the merge validates, unless running carefully.)
                    if self.careful {
                        return Err(self.stop(Wait::Node(site, Some(f))));
                    }
                    self.guessed = true;
                }
                self.reads.push(Read::Use {
                    f,
                    at,
                    after,
                    own,
                    site,
                    val,
                    path,
                });
                Ok(val.map(|v| (v, site)))
            }
            None => {
                let val = self.env_value(f)?;
                self.reads.push(Read::Use {
                    f,
                    at,
                    after,
                    own,
                    site: NONE,
                    val,
                    path,
                });
                Ok(val.map(|v| (v, NONE)))
            }
        }
    }

    /// Whether a pending-set entry lies between `from` and read position
    /// `pos` of unit `u` (asked of the world once per position).
    fn pset_after(&mut self, u: u32, pos: Pos, from: Pos) -> bool {
        let near = match self.pset_near {
            Some((k, p)) if k == (u, pos) => p,
            _ => {
                let p = self.w.last_pset_below(u, pos, self.me);
                self.pset_near = Some(((u, pos), p));
                p
            }
        };
        near.is_some_and(|p| p > from)
    }

    /// What this node's last result from a previous evaluation read of `f`
    /// at `key`, when `f` is guessed from the last.
    fn last_read(&mut self, f: Field, key: (Id, bool, bool)) -> Option<ValueId> {
        if !self.core.guess_last.contains(&f) {
            return None;
        }
        self.last_reads()?.vals.get(&(f, key.0, key.1, key.2)).copied()
    }

    /// Its last result from a previous evaluation's reads of the fields
    /// guessed from the last, which its result carries on.
    pub(crate) fn last_reads(&mut self) -> Option<Arc<LastReads>> {
        if self.core.guess_last.is_empty() {
            return None;
        }
        let (core, w, me) = (self.core, self.w, self.me);
        self.last
            .get_or_insert_with(|| w.node(me).and_then(|n| n.res.as_ref()).and_then(|r| r.last_reads(core, core.eval_no.load(std::sync::atomic::Ordering::Relaxed))))
            .clone()
    }

    /// Whether a range read of accumulator `f` folds its contributors in
    /// work as they last contributed (`Monoid::speculate`, predicting, not
    /// running carefully).
    fn guess_fold(&self, f: Field) -> bool {
        self.predict && !self.careful && self.core.acc(f).is_some_and(|m| m.speculate())
    }

    /// The node in work a range read of accumulator `f` over `(lo, hi)`
    /// waits for: the first that may contribute; none when the fold is
    /// guessed; guessing those that ran (`Monoid::guess_ran`), the first
    /// without a result. (`guessed` set when it folds one in work.)
    fn fold_blocker(&mut self, f: Field, u: u32, lo: Pos, hi: Pos) -> Option<Id> {
        let x = self.w.range_blocker(self.me, f, u, lo, hi)?;
        if self.guess_fold(f) {
            self.guessed = true;
            return None;
        }
        if self.predict && !self.careful && self.core.acc(f).is_some_and(|m| m.guess_ran()) {
            let open = self.w.range_blocker_open(self.core, self.me, f, u, lo, hi);
            if open.is_none() {
                self.guessed = true;
                self.fold_ran = true;
            }
            return open;
        }
        Some(x)
    }

    /// Whether node `id` is in work in this world.
    fn in_work(&self, id: Id) -> bool {
        self.w.node(id).is_some_and(|n| matches!(n.st, St::Dirty | St::Held))
    }

    /// The environment's value of `f` when no definition reaches: supplied,
    /// declared absent (`None`), or unknown (a field never supplied reads
    /// as absent).
    fn env_value(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        Ok(self.w.env_of(self.core, self.unit, f))
    }

    /// The definition of `f` reaching the position just after node `at`
    /// (what it unfolded or claims included): the value and its defining
    /// node (`None`: the unit's start value). `at` is any other node of
    /// this unit, before or past this one (a page builder reading the
    /// state at the pieces after it): a definition coming in or changing
    /// up to `at` re-asks, as for any read. One past this node may reach
    /// this node's own definition, read as it last committed.
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_at(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &at, )) } else { serde_json::Value::Null };
        let r = self.use_field_at_inner(f, at);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_at", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_at_inner(&mut self, f: Field, at: Option<NodeId>) -> Result<FieldAt, Stop> {
        self.use_field_pos(f, at, true, false)
    }

    /// The definition of `f` reaching the position just before node `at`
    /// (before or past this one, as [`Ctx::use_field_at`]).
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition (or structure before it) is in work.
    pub fn use_field_before(&mut self, f: Field, at: NodeId) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &at, )) } else { serde_json::Value::Null };
        let r = self.use_field_before_inner(f, at);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_before", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_before_inner(&mut self, f: Field, at: NodeId) -> Result<FieldAt, Stop> {
        self.use_field_pos(f, Some(at), false, false)
    }

    /// The definition of `f` reaching the position right after node `at`'s
    /// own definitions (what its run made, the text it claims included):
    /// not after what it unfolded, nor after nodes placed after it or
    /// after its children (a page's finisher placed in its extent reads
    /// the state the piece's own run left, not one that includes itself).
    /// `at` is any other node of this unit, before or past this one, as
    /// for [`Ctx::use_field_at`].
    ///
    /// # Errors
    ///
    /// [`Stop`] while that definition (or structure before it) is in work.
    pub fn use_field_after_own(&mut self, f: Field, at: NodeId) -> Result<FieldAt, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &at, )) } else { serde_json::Value::Null };
        let r = self.use_field_pos(f, Some(at), false, true);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_after_own", a, crate::record::stop_or(&r));
        r
    }

    fn use_field_pos(&mut self, f: Field, at: Option<NodeId>, after: bool, own: bool) -> Result<FieldAt, Stop> {
        let Some(at) = at else {
            // The unit's start: its start value.
            let v = self.env_value(f)?;
            self.reads.push(Read::Env { f, val: v });
            return Ok(v.map_or(FieldAt::Unset, |x| FieldAt::Def(x, None)));
        };
        let a = at.0;
        if self.w.node(a).is_none_or(|n| matches!(n.place, Place::Marker(_))) {
            self.named_gone |= self.resumed && self.w.node(a).is_none();
            self.reads.push(Read::At { node: a, there: false });
            return Ok(FieldAt::Gone);
        }
        // (Before or past the reader in its unit, not the reader itself.)
        if a == self.me || self.w.node(a).is_none_or(|n| n.unit != self.unit) {
            self.error = Some(crate::api::EvalError::BadPosition {
                node: self.node(),
                at,
            });
            return Ok(FieldAt::Unset);
        }
        self.reads.push(Read::At { node: a, there: true });
        let r = self.read_at(f, a, after, own, 0)?;
        Ok(match r {
            None => FieldAt::Unset,
            Some((v, site)) => FieldAt::Def(v, (site != NONE).then_some(NodeId(site))),
        })
    }

    /// Uses field `f` through projection `p`: returns the projected value
    /// (`None` if nothing defines it), and the run depends only on it.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::use_field`].
    pub fn use_field_proj(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &p, )) } else { serde_json::Value::Null };
        let r = self.use_field_proj_inner(f, p);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_proj", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_proj_inner(&mut self, f: Field, p: crate::api::ProjId) -> Result<Option<ValueId>, Stop> {
        let r = self.read_at(f, self.me, false, false, u32::from(p.0))?;
        Ok(r.map(|(x, _)| self.core.project(p.0, x)))
    }

    /// The path of `steps` ([`crate::Engine::path`]), for this and later
    /// runs.
    pub fn path(&self, steps: &[crate::api::Step]) -> crate::api::PathId {
        crate::api::PathId(self.core.intern_path(steps))
    }

    /// Uses field `f` through path `p`: returns the path's answer (`None`
    /// if nothing defines `f`, or a step does not apply), and the run
    /// depends only on it.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::use_field`].
    pub fn use_field_path(&mut self, f: Field, p: crate::api::PathId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&f), self.core.rec_steps(p.0)]) } else { serde_json::Value::Null };
        let r = self.use_field_path_inner(f, p);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_field_path", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_field_path_inner(&mut self, f: Field, p: crate::api::PathId) -> Result<Option<ValueId>, Stop> {
        let r = self.read_at(f, self.me, false, false, p.0)?;
        Ok(r.and_then(|(x, _)| self.core.eval_path(p.0, x)))
    }

    /// [`Ctx::acc_fold`] through path `p`: the path's answer on the fold,
    /// the run depending on it only.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::acc_fold`].
    pub fn acc_fold_path(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, p: crate::api::PathId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&f), crate::record::j(&from), crate::record::j(&to), self.core.rec_steps(p.0)]) } else { serde_json::Value::Null };
        let r = self.acc_fold_path_inner(f, from, to, true, p.0);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_fold_path", a, crate::record::stop_or(&r));
        r
    }

    fn acc_fold_path_inner(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, past: bool, p: u32) -> Result<Option<ValueId>, Stop> {
        let before = self.reads.len();
        let x = self.acc_window(f, from, to, past)?;
        if self.reads.len() > before
            && let Some(Read::Range { path, .. }) = self.reads.last_mut()
        {
            *path = p;
        }
        Ok(self.core.eval_path(p, x))
    }

    /// Views field `f` ([`View`]): the run depends on whether `f` has a
    /// value (`None` if not), and on what it inspects through the view.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::use_field`].
    pub fn view(&mut self, f: Field) -> Result<Option<View>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.view_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "view", a, crate::record::stop_or(&r));
        r
    }

    fn view_inner(&mut self, f: Field) -> Result<Option<View>, Stop> {
        let present = super::core::PRESENT;
        let Some((x, _)) = self.read_at(f, self.me, false, false, present)? else {
            return Ok(None);
        };
        let root = self.read_ix.get(&(f, self.me, false, false, present)).map_or(u32::MAX, |&i| u32::try_from(i).unwrap_or(u32::MAX));
        Ok(Some(self.new_view(root, 0, x)))
    }

    /// Views the fold of accumulator `f` over a window, as [`Ctx::acc_fold`]
    /// folds it ([`View`]): the run depends on what it inspects.
    ///
    /// # Errors
    ///
    /// [`Stop`] as for [`Ctx::acc_fold`].
    pub fn acc_view(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &to, )) } else { serde_json::Value::Null };
        let r = self.acc_view_inner(f, from, to);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_view", a, crate::record::stop_or(&r));
        r
    }

    fn acc_view_inner(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<View, Stop> {
        let present = super::core::PRESENT;
        let before = self.reads.len();
        self.acc_fold_path_inner(f, from, to, true, present)?;
        // (The fold's read, unless the window was empty: a constant.)
        let (root, x) = match self.reads.last() {
            Some(Read::Range { val, .. }) if self.reads.len() > before => (u32::try_from(self.reads.len() - 1).unwrap_or(u32::MAX), *val),
            _ => (u32::MAX, ValueId::UNIT),
        };
        Ok(self.new_view(root, 0, x))
    }

    fn new_view(&mut self, root: u32, path: u32, x: ValueId) -> View {
        self.views.push((root, path, x));
        View(u32::try_from(self.views.len() - 1).expect("too many views"))
    }

    /// Records a read of a view's root (read `root`) through path `p`: the
    /// root's own read, its path replaced (none for a constant, or a
    /// loop-back's read, which is of the whole value).
    fn view_read(&mut self, root: u32, p: u32) {
        // (A composition inspected: its holes read in full.)
        if root & super::world::COMPOSED != 0 {
            let k = (root & !super::world::COMPOSED) as usize;
            for i in 0..self.composed.get(k).map_or(0, Vec::len) {
                match self.composed[k][i] {
                    super::world::CPart::Hole { read, path, .. } => self.view_read(read, path),
                    super::world::CPart::Nested(j) => self.view_read(super::world::COMPOSED | j, 0),
                    super::world::CPart::Val(_) => {}
                }
            }
            return;
        }
        let Some(r) = self.reads.get(root as usize) else { return };
        match *r {
            Read::Use { f, at, after, own, site, val, .. } => {
                let key = (f, at, after, own, p);
                if self.read_ix.contains_key(&key) {
                    return;
                }
                self.read_ix.insert(key, self.reads.len());
                self.reads.push(Read::Use { f, at, after, own, site, val, path: p });
            }
            Read::Range { f, from, to, val, past, .. } => self.reads.push(Read::Range { f, from, to, val, past, path: p }),
            _ => {}
        }
    }

    /// Step `s` into view `v`: the view of what it gives (`None` if it
    /// does not apply). The run depends on whether it applies.
    pub fn at(&mut self, v: View, s: crate::api::Step) -> Option<View> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&v), self.core.rec_step(s)]) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        let p = self.core.extend_path(path, s);
        self.view_read(root, self.core.extend_path(p, crate::api::Step::Present));
        let r = self.core.step(s, x).map(|y| self.new_view(root, p, y));
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "at", a, crate::record::j(&r));
        r
    }

    /// What step `s` gives of view `v` (`None` if it does not apply): the
    /// run depends on it. Elements taken by index or by key through one
    /// view are read as one read of it, at the next mark and at the run's
    /// end (cheap per get, one entry per view to commit).
    pub fn get(&mut self, v: View, s: crate::api::Step) -> Option<ValueId> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&v), self.core.rec_step(s)]) } else { serde_json::Value::Null };
        let r = self.get_inner(v, s);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "get", a, crate::record::j(&r));
        r
    }

    fn get_inner(&mut self, v: View, s: crate::api::Step) -> Option<ValueId> {
        use crate::api::Step;
        let (root, path, x) = self.views[v.0 as usize];
        match s {
            // (Gathered: one read of the view, at the next mark.)
            Step::Index(_) | Step::Key(_) if root & super::world::COMPOSED == 0 && matches!(self.reads.get(root as usize), Some(Read::Use { .. } | Read::Range { .. })) => {
                let g = match self.gets.iter().rposition(|g| g.root == root && g.path == path) {
                    Some(i) => &mut self.gets[i],
                    None => {
                        self.gets.push(Gets { root, path, ix: Vec::new(), keys: Vec::new() });
                        self.gets.last_mut().expect("pushed")
                    }
                };
                match s {
                    Step::Index(i) => {
                        g.ix.push(i);
                        return self.index_of(x, i);
                    }
                    Step::Key(k) => g.keys.push(k),
                    _ => {}
                }
            }
            _ => self.view_read(root, self.core.extend_path(path, s)),
        }
        self.core.step(s, x)
    }

    /// Element `i` of `x` (as `Step::Index`), a list's looked up once
    /// for the gets that follow.
    fn index_of(&mut self, x: ValueId, i: u32) -> Option<ValueId> {
        use crate::values::Value;
        let at = match self.lists.iter().rposition(|l| l.0 == x) {
            Some(k) => k,
            None => {
                let val = self.core.values.get(x);
                if !matches!(*val, Value::List(_)) {
                    return self.core.step(crate::api::Step::Index(i), x);
                }
                if self.lists.len() == 4 {
                    self.lists.remove(0);
                }
                self.lists.push((x, val));
                self.lists.len() - 1
            }
        };
        match &*self.lists[at].1 {
            Value::List(l) => l.get(i as usize).copied(),
            _ => None,
        }
    }

    /// Reads what the run took by index and by key through each view since
    /// the last mark: one read of the view's root each, through a path
    /// ending in the indices (`Step::Indices`) or the keys (`Step::Keys`)
    /// taken; one taken alone, through its own step. The run depends on
    /// exactly those elements.
    pub(crate) fn flush_gets(&mut self) {
        use crate::api::Step;
        if self.gets.is_empty() {
            return;
        }
        let v = &self.core.values;
        for mut g in std::mem::take(&mut self.gets) {
            g.ix.sort_unstable();
            g.ix.dedup();
            let s = match g.ix[..] {
                [] => None,
                [i] => Some(Step::Index(i)),
                _ => Some(Step::Indices(v.list(g.ix.iter().map(|&i| v.int(i64::from(i))).collect()))),
            };
            if let Some(s) = s {
                self.view_read(g.root, self.core.extend_path(g.path, s));
            }
            g.keys.sort_unstable_by_key(|k| k.raw());
            g.keys.dedup();
            let s = match g.keys[..] {
                [] => None,
                [k] => Some(Step::Key(k)),
                _ => Some(Step::Keys(v.list(g.keys))),
            };
            if let Some(s) = s {
                self.view_read(g.root, self.core.extend_path(g.path, s));
            }
        }
    }

    /// View `v`'s length (as [`crate::Step::Len`]): the run depends on it.
    pub fn len(&mut self, v: View) -> Option<u64> {
        let n = self.get(v, crate::api::Step::Len)?;
        self.core.values.as_int(n).and_then(|n| u64::try_from(n).ok())
    }

    /// Whether view `v` has key `k` (as [`crate::Step::Has`]): the run
    /// depends on it.
    pub fn has(&mut self, v: View, k: ValueId) -> bool {
        self.get(v, crate::api::Step::Has(k)).and_then(|b| self.core.values.as_bool(b)) == Some(true)
    }

    /// Whether a view (root `root`, path `path`) can be passed on: a use's,
    /// a fold's (`acc_view`: followed while its window stands), or a whole
    /// composition.
    fn passable(&self, root: u32, path: u32) -> bool {
        if root & super::world::COMPOSED != 0 {
            return path == 0;
        }
        matches!(self.reads.get(root as usize), Some(Read::Use { .. } | Read::Range { .. }))
    }

    /// Composes a list of `parts` (a [`View`] of it): the run's own values
    /// and views' values, passed on. Passed on in turn ([`Ctx::define_from`]
    /// and the others), it follows its views' values: when one changes and
    /// stays there, the list is made again and refiled, the node not run
    /// again. Inspected, it depends on all of its views' values.
    pub fn compose(&mut self, parts: &[Part]) -> View {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(parts, )) } else { serde_json::Value::Null };
        use super::world::{COMPOSED, CPart};
        let mut cp = Vec::with_capacity(parts.len());
        let mut xs = Vec::with_capacity(parts.len());
        for p in parts {
            match *p {
                Part::Val(x) => {
                    cp.push(CPart::Val(x));
                    xs.push(x);
                }
                Part::View(v) => {
                    let (root, path, x) = self.views[v.0 as usize];
                    if root & COMPOSED != 0 && path == 0 {
                        cp.push(CPart::Nested(root & !COMPOSED));
                    } else if self.passable(root, path) {
                        cp.push(CPart::Hole { read: root, path, val: x });
                    } else {
                        self.view_read(root, path);
                        cp.push(CPart::Val(x));
                    }
                    xs.push(x);
                }
            }
        }
        let x = self.core.values.list(xs);
        self.composed.push(cp);
        let k = u32::try_from(self.composed.len() - 1).expect("too many compositions");
        let r = self.new_view(COMPOSED | k, 0, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "compose", a, crate::record::j(&r));
        r
    }

    /// Defines `f` (at `loc`) as view `v`'s value, passed on without
    /// inspecting it: the run depends on whether `v` has a value, not on
    /// the value; when the value read changes and stays there, the
    /// definition follows it without the node running again (only `f`'s
    /// readers are asked again). A view of a loop-back field forwards it
    /// (`define_forward`). Soft: `f` defined locally as `f` reaching this
    /// node (`view(f)`), not defined here yet, is not defined (its readers
    /// read what this node read).
    pub fn define_from(&mut self, f: Field, v: View, loc: Locality) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &v, &loc, )) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        let me = self.me;
        let soft = path == 0
            && matches!(loc, Locality::Local)
            && !self.ix.contains_key(&(0, f, loc.entry_key()))
            && matches!(self.reads.get(root as usize), Some(&Read::Use { f: g, at, after: false, own: false, .. }) if g == f && at == me);
        if !soft {
            self.define_with_inner(f, x, loc);
        }
        match self.reads.get(root as usize) {
            _ if soft => {}
            _ if self.passable(root, path) => self.passes.push(super::world::Pass { to: super::world::PassTo::Def(f, loc), read: root, path, val: x, set: None }),
            Some(&Read::Loop { f: from, .. }) if path == 0 => self.forwards.push((f, loc, from)),
            _ => self.view_read(root, path),
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_from", a, serde_json::Value::Null);
    }

    /// Defines `f` as view `v`'s value with its part at path `p` replaced
    /// by `x` (a cell of a record: an index of a list, a key of an
    /// association list, nested). The run depends only on that part being
    /// there: when the rest of `v`'s value changes, `f` is made again and
    /// refiled, the node not run again. Returns false, defining nothing,
    /// if the part is not there.
    pub fn define_set(&mut self, f: Field, v: View, p: crate::api::PathId, x: ValueId, loc: Locality) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&f), crate::record::j(&v), self.core.rec_steps(p.0), crate::record::j(&x), crate::record::j(&loc)]) } else { serde_json::Value::Null };
        let r = self.define_set_inner(f, v, p.0, x, loc);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_set", a, crate::record::j(&r));
        r
    }

    fn define_set_inner(&mut self, f: Field, v: View, p: u32, x: ValueId, loc: Locality) -> bool {
        let (root, path, base) = self.views[v.0 as usize];
        let mut at = path;
        for &s in self.core.path_steps(p) {
            at = self.core.extend_path(at, s);
        }
        let set = self.core.set_path(p, base, x);
        if set.is_none() || !self.passable(root, path) {
            // (Not one a part is set in: read in full.)
            self.view_read(root, path);
        } else {
            self.view_read(root, self.core.extend_path(at, crate::api::Step::Present));
        }
        let Some(y) = set else { return false };
        self.define_with_inner(f, y, loc);
        if self.passable(root, path) {
            self.passes.push(super::world::Pass { to: super::world::PassTo::Def(f, loc), read: root, path, val: base, set: Some((p, x)) });
        }
        true
    }

    /// Contributes view `v`'s value to accumulator `f`, passed on as
    /// [`Ctx::define_from`] passes it (the run's only contribution to `f`;
    /// another combined with it reads it in full).
    pub fn contribute_from(&mut self, f: Field, v: View) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &v, )) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        let first = !self.ix.contains_key(&(2, f, 0));
        self.contribute_inner(f, x);
        if first && self.passable(root, path) {
            self.passes.push(super::world::Pass { to: super::world::PassTo::Contrib(f), read: root, path, val: x, set: None });
        } else {
            self.view_read(root, path);
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contribute_from", a, serde_json::Value::Null);
    }

    /// Publishes view `v`'s value for loop-back field `f`. (For now the
    /// run depends on all of the value, as [`Ctx::whole`] makes it: a
    /// publication follows its value at the merge only, where the early
    /// moves are made.)
    pub fn publish_from(&mut self, f: Field, v: View) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &v, )) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        self.publish_inner(f, x);
        self.view_read(root, path);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "publish_from", a, serde_json::Value::Null);
    }

    /// Makes view `v`'s value the run's output, passed on as
    /// [`Ctx::define_from`] passes it: whatever the run returns, its output
    /// is this value (the last such call's).
    pub fn output_from(&mut self, v: View) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&v, )) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        self.passes.retain(|p| p.to != super::world::PassTo::Out);
        if self.passable(root, path) {
            self.passes.push(super::world::Pass { to: super::world::PassTo::Out, read: root, path, val: x, set: None });
        } else {
            self.view_read(root, path);
            self.out_from = Some(x);
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "output_from", a, serde_json::Value::Null);
    }

    /// View `v`'s value: the run depends on all of it.
    pub fn whole(&mut self, v: View) -> ValueId {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&v, )) } else { serde_json::Value::Null };
        let (root, path, x) = self.views[v.0 as usize];
        self.view_read(root, path);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "whole", a, crate::record::j(&x));
        x
    }

    /// Reads each of `fs` as [`Ctx::use_field`] reads one, in order, into
    /// `out` (cleared first): the same reads, recorded alike, with the
    /// fields' lookups made together (many fields read at once, a run's
    /// first touch of each, cost less so than one at a time).
    ///
    /// # Errors
    ///
    /// [`Stop`] at the first that stops (those before it read, in `out`).
    pub fn use_fields(&mut self, fs: &[Field], out: &mut Vec<Option<ValueId>>) -> Result<(), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&fs, )) } else { serde_json::Value::Null };
        out.clear();
        self.prefetch_cells(fs.iter().copied());
        let mut r = Ok(());
        for &f in fs {
            match self.use_field_inner(f) {
                Ok(v) => out.push(v),
                Err(s) => {
                    r = Err(s);
                    break;
                }
            }
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_fields", a, if r.is_ok() { serde_json::json!({"ok": crate::record::j(&*out)}) } else { serde_json::json!("stop") });
        r
    }

    /// [`Ctx::use_fields`] through projections: each as
    /// [`Ctx::use_field_proj`] reads one.
    ///
    /// # Errors
    ///
    /// [`Stop`] at the first that stops (those before it read, in `out`).
    pub fn use_fields_proj(&mut self, fs: &[(Field, crate::api::ProjId)], out: &mut Vec<Option<ValueId>>) -> Result<(), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&fs, )) } else { serde_json::Value::Null };
        out.clear();
        self.prefetch_cells(fs.iter().map(|p| p.0));
        let mut r = Ok(());
        for &(f, p) in fs {
            match self.use_field_proj_inner(f, p) {
                Ok(v) => out.push(v),
                Err(s) => {
                    r = Err(s);
                    break;
                }
            }
        }
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_fields_proj", a, if r.is_ok() { serde_json::json!({"ok": crate::record::j(&*out)}) } else { serde_json::json!("stop") });
        r
    }

    /// Looks up the cells of fields `fs` read at this node not looked up
    /// yet, together.
    fn prefetch_cells(&mut self, fs: impl Iterator<Item = Field>) {
        let u = self.unit;
        let todo: Vec<Field> = fs.filter(|f| !self.cells.contains_key(&(u, *f))).collect();
        // (A few are as cheap one at a time.)
        if todo.len() < 8 {
            return;
        }
        for (f, c) in self.w.fields_of_many(u, todo.into_iter()) {
            self.cells.insert((u, f), c);
        }
    }

    /// The environment's value of `f`; waits while it is neither supplied
    /// nor declared absent.
    ///
    /// # Errors
    ///
    /// `Stop` while the field is unknown.
    pub fn use_env(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.use_env_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "use_env", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn use_env_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        match self.core.env_get(self.w, f) {
            Some(val) => {
                self.reads.push(Read::Env { f, val });
                Ok(val)
            }
            None => Err(self.stop(Wait::Env(f))),
        }
    }

    pub fn define(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        self.define_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define", a, serde_json::Value::Null);
    }

    #[inline]
    fn define_inner(&mut self, f: Field, x: ValueId) {
        let loc = self
            .w
            .node(self.me)
            .and_then(|n| n.text.stat.defs.as_ref().and_then(|d| d.iter().find(|e| e.0 == f).map(|e| e.1)))
            .unwrap_or(Locality::Local);
        self.define_with_inner(f, x, loc);
    }

    pub fn define_with(&mut self, f: Field, x: ValueId, loc: Locality) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, &loc, )) } else { serde_json::Value::Null };
        self.define_with_inner(f, x, loc);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_with", a, serde_json::Value::Null);
    }

    #[inline]
    fn define_with_inner(&mut self, f: Field, x: ValueId, loc: Locality) {
        let k = loc.entry_key();
        if !self.forwards.is_empty() {
            self.forwards.retain(|x| !(x.0 == f && x.1.entry_key() == k));
        }
        if !self.passes.is_empty() {
            self.passes.retain(|p| !matches!(p.to, super::world::PassTo::Def(g, l) if g == f && l.entry_key() == k));
        }
        if let Some(&i) = self.ix.get(&(0, f, k)) {
            let d = &mut self.defs[i];
            d.loc = loc;
            d.val = x;
        } else {
            self.ix.insert((0, f, k), self.defs.len());
            self.defs.push(Def { f, loc, val: x });
        }
    }

    /// Defines `f` (at `loc`) as loop-back field `from`'s round value,
    /// forwarded: the run does not read `from`, and when its round value
    /// changes the definition follows without the node running again (only
    /// `f`'s readers are asked again). While `from` is not a loop-back
    /// field, `f` is defined as `from` read here (`use_field`), and
    /// nothing while it reads none.
    ///
    /// # Errors
    ///
    /// [`Stop`] as `use_field` of `from`, when it is not a loop-back field.
    pub fn define_forward(&mut self, f: Field, from: Field, loc: Locality) -> Result<(), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &loc, )) } else { serde_json::Value::Null };
        let r = self.define_forward_inner(f, from, loc);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "define_forward", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn define_forward_inner(&mut self, f: Field, from: Field, loc: Locality) -> Result<(), Stop> {
        if let Some(&lv) = self.w.loops.get(&from) {
            // (Its value is not a read: it follows.)
            self.reads.push(Read::IsLoop { f: from, val: true });
            self.define_with_inner(f, lv, loc);
            self.forwards.push((f, loc, from));
            return Ok(());
        }
        if let Some((v, _)) = self.read_at(from, self.me, false, false, 0)? {
            self.define_with_inner(f, v, loc);
        }
        Ok(())
    }

    /// Defines `f` as a local of the scope this node opens after itself.
    pub fn define_in_scope(&mut self, f: Field, x: ValueId) {
        self.define_with(f, x, Locality::Scope);
    }

    /// Defines `f` as a local of the `k`-th frame this node opens.
    pub fn define_in_scope_at(&mut self, f: Field, k: u8, x: ValueId) {
        self.define_with(f, x, Locality::ScopeAt(k));
    }

    /// Declares `f` a loop-back field from a run.
    pub fn declare_loopback(&mut self, f: Field, init: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &init, )) } else { serde_json::Value::Null };
        self.declare_loopback_inner(f, init);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "declare_loopback", a, serde_json::Value::Null);
    }

    #[inline]
    fn declare_loopback_inner(&mut self, f: Field, init: ValueId) {
        self.core.declare_loop_later(f, init, self.me, crate::api::LoopOptions::default());
    }

    /// Declares loop-back field `f` (as `declare_loopback`), its round
    /// values early: see [`Engine::declare_loopback_early`] (not
    /// deterministic).
    ///
    /// [`Engine::declare_loopback_early`]: crate::Engine::declare_loopback_early
    pub fn declare_loopback_early(&mut self, f: Field, init: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &init, )) } else { serde_json::Value::Null };
        self.core.declare_loop_later(f, init, self.me, crate::api::LoopOptions { early: true, last_wins: false });
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "declare_loopback_early", a, serde_json::Value::Null);
    }

    /// Declares loop-back field `f` (as `declare_loopback`) with options
    /// `o` ([`crate::LoopOptions`]: early values, the last publication
    /// winning).
    pub fn declare_loopback_with(&mut self, f: Field, init: ValueId, o: crate::api::LoopOptions) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &init, &o, )) } else { serde_json::Value::Null };
        self.core.declare_loop_later(f, init, self.me, o);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "declare_loopback_with", a, serde_json::Value::Null);
    }

    /// Publishes a loop-back field's next round value.
    pub fn publish(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        self.publish_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "publish", a, serde_json::Value::Null);
    }

    #[inline]
    fn publish_inner(&mut self, f: Field, x: ValueId) {
        if let Some(&i) = self.ix.get(&(1, f, 0)) {
            self.pubs[i].1 = x;
        } else {
            self.ix.insert((1, f, 0), self.pubs.len());
            self.pubs.push((f, x));
        }
    }

    /// Unfolds a child: a node placed after this one's earlier children.
    pub fn child(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.child_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "child", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn child_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let i = self.nkids;
        self.nkids += 1;
        self.kids.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::Child,
        });
        i
    }

    /// A continuation: a node placed after this one's subtree, at its level.
    pub fn follow(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn follow_inner(&mut self, op: OpId, args: Vec<ValueId>) -> usize {
        let i = self.base.map_or(0, |b| b.1) + self.conts.len();
        self.conts.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::Cont,
        });
        i
    }

    /// A continuation placed right after an earlier node `at` of this
    /// unit, at `at`'s level. An `at` that is gone, in another unit or not
    /// before this node places it after this one (as `follow`).
    pub fn follow_after(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&at, &op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_after_inner(at, op, args, false);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow_after", a, crate::record::j(&r));
        r
    }

    /// As [`Ctx::follow_after`], with `at` before or past this node: a
    /// continuation placed right after a later node of this unit (as a
    /// page builder places a page's finisher after the piece it broke
    /// at). Readers after `at` are re-asked as for any node coming in. An
    /// `at` that is gone, in another unit or this node places it after
    /// this one (as `follow`).
    pub fn follow_after_any(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>) -> usize {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&at, &op, &args, )) } else { serde_json::Value::Null };
        let r = self.follow_after_inner(at, op, args, true);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "follow_after_any", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn follow_after_inner(&mut self, at: NodeId, op: OpId, args: Vec<ValueId>, any: bool) -> usize {
        let ok = self.w.node(at.0).is_some_and(|n| n.unit == self.unit && !matches!(n.place, Place::Marker(_)))
            && if any { at.0 != self.me } else { self.w.before(at.0, self.me) };
        if !ok {
            return self.follow_inner(op, args);
        }
        self.reads.push(Read::At {
            node: at.0,
            there: self.w.node(at.0).is_some(),
        });
        let i = self.base.map_or(0, |b| b.1) + self.conts.len();
        self.conts.push(Spec {
            op,
            args: Arc::from(args),
            place: Place::After(at.0),
        });
        i
    }

    /// Opens a scope among the children (closed by `close_scope`).
    pub fn open_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.open_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "open_scope", a, serde_json::Value::Null);
    }

    #[inline]
    fn open_scope_inner(&mut self) {
        self.kids.push(Spec {
            op: OpId(0),
            args: Arc::from([]),
            place: Place::Marker(true),
        });
    }

    pub fn close_scope(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.close_scope_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "close_scope", a, serde_json::Value::Null);
    }

    #[inline]
    fn close_scope_inner(&mut self) {
        self.kids.push(Spec {
            op: OpId(1),
            args: Arc::from([]),
            place: Place::Marker(false),
        });
    }

    /// The output of child `i` of this run.
    ///
    /// # Errors
    ///
    /// [`Stop`] if the child has not produced it yet.
    pub fn output_of(&mut self, i: usize) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, )) } else { serde_json::Value::Null };
        let r = self.output_of_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "output_of", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn output_of_inner(&mut self, i: usize) -> Result<ValueId, Stop> {
        let res = self.w.node(self.me).and_then(|n| n.res.as_ref());
        // (Continuing: one of the earlier attempts' children, as committed.)
        let (bk, _, bn) = self.base.unwrap_or((0, 0, 0));
        if i < bn {
            let c = res.and_then(|r| r.kids.iter().copied().filter(|&k| self.w.node(k).is_none_or(|c| !matches!(c.place, Place::Marker(_)))).nth(i));
            return match c {
                Some(c) => self.read_out(c),
                None => Ok(ValueId::UNIT),
            };
        }
        // The child this run placed `i`-th, if the committed unfolding has
        // it (the same placement order: the same node).
        let Some(k) = self
            .kids
            .iter()
            .enumerate()
            .filter(|(_, s)| !matches!(s.place, Place::Marker(_)))
            .nth(i - bn)
            .map(|(k, _)| k)
        else {
            return Ok(ValueId::UNIT);
        };
        let committed = res
            .and_then(|r| r.kids.get(bk + k).copied())
            .filter(|&c| self.w.node(c).is_some_and(|n| n.text.op == self.kids[k].op && n.text.args == self.kids[k].args));
        let Some(c) = committed else {
            return Err(self.stop(Wait::Child(bk + k)));
        };
        self.read_out(c)
    }

    /// The output of continuation `i` of this run (the index `follow`,
    /// `follow_after` or `follow_after_any` gave), as it last committed
    /// under the text this run placed it with: `None` while it has no
    /// result of that text (not made yet, not run, or given new arguments
    /// and not run since). Never a stop: the read is recorded, and the
    /// continuation's commit re-asks this run.
    pub fn placed_output(&mut self, i: usize) -> Option<ValueId> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, )) } else { serde_json::Value::Null };
        let r = self.placed_output_inner(i, None);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "placed_output", a, crate::record::j(&r));
        r
    }

    /// As [`Ctx::placed_output`], `used` standing in for none (the guess
    /// the reader goes on with): the read stands while the continuation
    /// has no result of its text or outputs `used`, and is re-asked only
    /// when another output commits.
    pub fn placed_output_or(&mut self, i: usize, used: ValueId) -> ValueId {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, &used, )) } else { serde_json::Value::Null };
        let r = self.placed_output_inner(i, Some(used)).unwrap_or(used);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "placed_output_or", a, crate::record::j(&r));
        r
    }

    fn placed_output_inner(&mut self, i: usize, or: Option<ValueId>) -> Option<ValueId> {
        let bc = self.base.map_or(0, |b| b.1);
        let committed: &[Id] = self.w.node(self.me).and_then(|n| n.res.as_ref()).map_or(&[], |r| &r.conts[..]);
        let (node, op, args) = if i < bc {
            // (An earlier attempt's, as it stands.)
            let c = *committed.get(i)?;
            let n = self.w.node(c)?;
            (c, n.text.op, n.text.args.clone())
        } else {
            // This attempt's: the node its commit makes or keeps (the
            // ordinals go on from the earlier attempts' placements).
            let k = i - bc;
            let s = self.conts.get(k)?;
            let key = |place: Place, op: OpId| match place {
                Place::After(a) => (3, op, a),
                p => (u64::from(!matches!(p, Place::Cont)), op, 0),
            };
            let want = key(s.place, s.op);
            let earlier = committed[..bc.min(committed.len())]
                .iter()
                .filter(|&&x| self.w.node(x).is_some_and(|n| key(n.place, n.text.op) == want))
                .count();
            let mine = self.conts[..k].iter().filter(|c| key(c.place, c.op) == want).count();
            let ord = u32::try_from(earlier + mine + 1).unwrap_or(u32::MAX);
            (super::build::Builder::placed_id(self.me, s.place, s.op, ord), s.op, s.args.clone())
        };
        let val = self.w.node(node).and_then(|n| n.placed_out(op, &args)).or(or);
        self.reads.push(Read::Placed { node, op, args, val, or });
        val
    }

    fn read_out(&mut self, c: Id) -> Result<ValueId, Stop> {
        let Some(n) = self.w.node(c) else {
            return Ok(ValueId::UNIT);
        };
        if matches!(n.st, St::Dirty | St::Held) && n.res.is_none() {
            return Err(self.stop(Wait::Node(c, None)));
        }
        let val = n.out();
        self.reads.push(Read::Out { node: c, val });
        Ok(val.unwrap_or(ValueId::UNIT))
    }

    /// Applies `op` to `args`: a memoized application in a unit of its own.
    ///
    /// # Errors
    ///
    /// `Stop` until the application has its result.
    pub fn apply(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&op, &args, )) } else { serde_json::Value::Null };
        let r = self.apply_inner(op, args);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "apply", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn apply_inner(&mut self, op: OpId, args: Vec<ValueId>) -> Result<ValueId, Stop> {
        let app = app_id(op, &args);
        match self.w.node(app) {
            None => {
                self.apps.push((app, op, Arc::from(args)));
                Err(self.stop(Wait::Node(app, None)))
            }
            Some(n) if matches!(n.st, St::Dirty | St::Held) => Err(self.stop(Wait::Node(app, None))),
            Some(n) => {
                let val = n.out();
                self.reads.push(Read::Out { node: app, val });
                Ok(val.unwrap_or(ValueId::UNIT))
            }
        }
    }

    /// Adds `x` to accumulator `f` at this node.
    pub fn contribute(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        self.contribute_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contribute", a, serde_json::Value::Null);
    }

    #[inline]
    fn contribute_inner(&mut self, f: Field, x: ValueId) {
        // (A value passed to it before is combined with this one: read in
        // full now.)
        if let Some(i) = self.passes.iter().position(|p| p.to == super::world::PassTo::Contrib(f)) {
            let p = self.passes.remove(i);
            self.view_read(p.read, p.path);
        }
        if let Some(&i) = self.ix.get(&(2, f, 0)) {
            let c = &mut self.contribs[i];
            c.1 = match self.core.acc(f) {
                Some(m) => m.combine(&self.core.values, c.1, x),
                None => x,
            };
        } else {
            self.ix.insert((2, f, 0), self.contribs.len());
            self.contribs.push((f, x));
        }
    }

    /// The fold of `f`'s contributions after `from` and before this node.
    ///
    /// # Errors
    ///
    /// `Stop` while a node in the range is in work.
    pub fn acc_range(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, )) } else { serde_json::Value::Null };
        let r = self.acc_range_inner(f, from);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_inner(&mut self, f: Field, from: Option<NodeId>) -> Result<ValueId, Stop> {
        self.acc_range_to_inner(f, from, None)
    }

    /// [`Ctx::acc_range`] through path `p`: the path's answer on the fold
    /// of `f`'s contributions after `from` and before this node, the run
    /// depending on it only.
    ///
    /// # Errors
    ///
    /// `Stop` as for [`Ctx::acc_range`].
    pub fn acc_range_path(&mut self, f: Field, from: Option<NodeId>, p: crate::api::PathId) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { serde_json::json!([crate::record::j(&f), crate::record::j(&from), self.core.rec_steps(p.0)]) } else { serde_json::Value::Null };
        let r = self.acc_fold_path_inner(f, from, None, false, p.0);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range_path", a, crate::record::stop_or(&r));
        r
    }

    /// A register kept as definitions and contributions: the definition
    /// of `f` reaching this node (as [`Self::use_field`]), and the fold of
    /// accumulator `acc`'s contributions after that definition's node
    /// (its own excluded) and before this one, from the unit's start when
    /// none reaches or it is the environment's. One read of each: a
    /// different definition reaching, or a contribution changing in the
    /// window, re-asks; the fold guesses as [`Self::acc_range`] does.
    ///
    /// # Errors
    ///
    /// [`Stop`] while the definition, or a node in the window, is in work.
    pub fn acc_range_after_def(&mut self, acc: Field, f: Field) -> Result<(Option<ValueId>, ValueId), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&acc, &f, )) } else { serde_json::Value::Null };
        let r = self.acc_range_after_def_inner(acc, f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range_after_def", a, crate::record::stop_or(&r));
        r
    }

    fn acc_range_after_def_inner(&mut self, acc: Field, f: Field) -> Result<(Option<ValueId>, ValueId), Stop> {
        let def = self.read_at(f, self.me, false, false, 0)?;
        let from = def.map(|d| d.1).filter(|&s| s != NONE).map(NodeId);
        let fold = self.acc_range_to_inner(acc, from, None)?;
        Ok((def.map(|d| d.0), fold))
    }

    /// The fold of accumulator `f` over the window `(from, to]`.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the window has finished.
    pub fn acc_range_to(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &to, )) } else { serde_json::Value::Null };
        let r = self.acc_range_to_inner(f, from, to);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_range_to", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn acc_range_to_inner(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        self.acc_window(f, from, to, false)
    }

    /// The fold of accumulator `f` over the window `(from, to]`, which may
    /// lie past this node (`to` after it): a reader of the nodes after it,
    /// as a page builder reads the pieces it breaks. This node's own
    /// contribution to `f`, if any, is folded as it last committed.
    ///
    /// # Errors
    ///
    /// [`Stop`] until every node in the window has finished.
    pub fn acc_fold(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &from, &to, )) } else { serde_json::Value::Null };
        let r = self.acc_window(f, from, to, true);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_fold", a, crate::record::stop_or(&r));
        r
    }

    /// The first node after `after` (`None`: the unit's start), before or
    /// past this one, that contributes to accumulator `f`, with its
    /// contribution; `None` when none is left in the unit. A reader that
    /// folds forward a node at a time, as a page builder takes the pieces
    /// after it until it decides, calls it again from the node it got.
    /// What it read is the window up to that node: a contributor that
    /// comes in between, or a change of the node's contribution, re-asks.
    ///
    /// # Errors
    ///
    /// [`Stop`] while a node in the window, up to the next contributor,
    /// is in work and may contribute.
    pub fn acc_next(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &after, )) } else { serde_json::Value::Null };
        let r = self.acc_next_inner(f, after);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "acc_next", a, crate::record::stop_or(&r));
        r
    }

    /// Node `x`, named by this run as a window's end, as the window takes
    /// it: one gone is read as gone and taken as none (a resumed run that
    /// names one starts over: its kept state names what is no longer
    /// there).
    fn anchor(&mut self, x: Option<NodeId>) -> Id {
        let Some(x) = x else { return NONE };
        if self.w.node(x.0).is_some() {
            return x.0;
        }
        self.reads.push(Read::At { node: x.0, there: false });
        self.named_gone |= self.resumed;
        NONE
    }

    fn acc_next_inner(&mut self, f: Field, after: Option<NodeId>) -> Result<Option<(NodeId, ValueId)>, Stop> {
        let from = self.anchor(after);
        let Some((u, lo, _)) = self.w.window(self.me, from, NONE, true) else {
            return Ok(None);
        };
        // The next committed (or listed) contribution, not this node's.
        let order = &self.w.unit(u).order;
        let next = self.w.field(u, f).map(|c| &c.contribs).and_then(|m| {
            m.range(std::ops::Bound::Excluded(&lo), std::ops::Bound::Unbounded)
                .filter_map(|(p, _)| order.get(p).map(|&x| (*p, x)))
                .find(|&(_, x)| x != self.me)
        });
        let hi = next.map_or(Pos::MAX, |(p, _)| p + 1);
        // (A node before it in work that may contribute comes first;
        // guessing, it is passed as it last contributed. Waited for alone,
        // not with the rest of the window: it may be the next contributor,
        // and a page builder folds piece by piece as they finish.)
        if let Some(x) = self.fold_blocker(f, u, lo, hi) {
            return Err(self.stop(Wait::Node(x, Some(f))));
        }
        let val = match self.core.acc(f) {
            Some(mo) => {
                let m = super::world::AccMeasure {
                    m: &**mo,
                    v: &self.core.values,
                };
                self.w.fold(&m, u, f, lo, hi)
            }
            None => ValueId::UNIT,
        };
        let to = next.map_or(NONE, |(_, x)| x);
        self.reads.push(Read::Range { f, from, to, val, past: true, path: 0 });
        Ok(next.map(|(_, x)| (NodeId(x), val)))
    }

    fn acc_window(&mut self, f: Field, from: Option<NodeId>, to: Option<NodeId>, past: bool) -> Result<ValueId, Stop> {
        let from = self.anchor(from);
        let to = self.anchor(to);
        let Some((u, lo, hi)) = self.w.window(self.me, from, to, past) else {
            return Ok(ValueId::UNIT);
        };
        // A node in the window still in work that may contribute: waited
        // for (one that declares its contributions, or whose last run made
        // none to `f`, is folded as it stands; its change re-asks), unless
        // the fold is guessed (`Monoid::speculate`).
        if let Some(x) = self.fold_blocker(f, u, lo, hi) {
            {
                if super::core::debug() && self.w.node(x).is_some_and(|n| !matches!(n.st, St::Dirty | St::Held)) {
                    eprintln!("range {} of {f:?}: blocker {x} in state {:?}", self.me, self.w.node(x).map(|n| n.st));
                }
                // (It waits for the whole range: re-parked on the next blocker
                // without running while one is left.)
                self.range_wait = Some((f, from, to, past));
                return Err(self.stop(Wait::Node(x, Some(f))));
            }
        }
        let val = match self.core.acc(f) {
            Some(mo) => {
                let m = super::world::AccMeasure {
                    m: &**mo,
                    v: &self.core.values,
                };
                self.w.fold(&m, u, f, lo, hi)
            }
            None => ValueId::UNIT,
        };
        self.reads.push(Read::Range { f, from, to, val, past, path: 0 });
        Ok(val)
    }

    /// The kind of the scope this node is in.
    ///
    /// # Errors
    ///
    /// `Stop` while the scope structure before it is undecided.
    pub fn scope_kind(&mut self) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.scope_kind_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "scope_kind", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn scope_kind_inner(&mut self) -> Result<Option<ValueId>, Stop> {
        let val = self.w.kind_at(self.me);
        self.reads.push(Read::Kind { at: self.me, val });
        Ok(val)
    }

    /// Sets the kind of the scopes this node opens (a kind-pending effect).
    pub fn set_kind(&mut self, k: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&k, )) } else { serde_json::Value::Null };
        self.set_kind_inner(k);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kind", a, serde_json::Value::Null);
    }

    #[inline]
    fn set_kind_inner(&mut self, k: ValueId) {
        self.kinds = vec![k];
    }

    /// Sets the kinds of the frames this node opens, outermost first.
    pub fn set_kinds(&mut self, ks: &[ValueId]) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&ks, )) } else { serde_json::Value::Null };
        self.set_kinds_inner(ks);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_kinds", a, serde_json::Value::Null);
    }

    #[inline]
    fn set_kinds_inner(&mut self, ks: &[ValueId]) {
        self.kinds = ks.to_vec();
    }

    /// Decides a pending effect.
    pub fn set_effect(&mut self, e: Effect) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&e, )) } else { serde_json::Value::Null };
        self.set_effect_inner(e);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_effect", a, serde_json::Value::Null);
    }

    #[inline]
    fn set_effect_inner(&mut self, e: Effect) {
        self.effect = Some(e);
    }

    /// Whether this node's effect is decided by its run.
    #[must_use]
    pub fn effect_pending(&self) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.effect_pending_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "effect_pending", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn effect_pending_inner(&self) -> bool {
        self.w.node(self.me).is_some_and(|n| n.text.stat.effect.is_none())
    }

    /// Narrows this node's definition set to `m`.
    pub fn narrow(&mut self, m: crate::api::MayDefine) {
        #[cfg(feature = "observe")]
        {
            let a = if self.calls.borrow().is_some() { crate::record::j(&(&m,)) } else { serde_json::Value::Null };
            crate::record::call(&self.calls, "narrow", a, serde_json::Value::Null);
        }
        self.narrowed = Some(m.into());
    }

    /// Claims the first `n` following siblings (applied when the run
    /// commits; it overrides the text's declared claim).
    pub fn claim(&mut self, n: usize) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&n, )) } else { serde_json::Value::Null };
        self.claim_inner(n);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim", a, serde_json::Value::Null);
    }

    #[inline]
    fn claim_inner(&mut self, n: usize) {
        self.claim = Some(n);
    }

    /// The claimed siblings' texts: a declared count claim's current
    /// siblings, else the first `n` following ones of this run's `claim(n)`.
    #[must_use]
    pub fn claimed(&mut self) -> Vec<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.claimed_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claimed", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn claimed_inner(&mut self) -> Vec<(OpId, Vec<ValueId>)> {
        let st = self.w.claims.get(&self.me);
        if let Some(st) = st.filter(|st| matches!(st.decl, crate::api::Claim::Count(_)) && self.claim.is_none()) {
            let list: Vec<(Id, usize)> = st.taken.iter().map(|&x| (x, super::claims::key_of(self.w, x))).collect();
            self.reads.push(Read::Claimed { list: list.clone() });
            return list
                .iter()
                .filter_map(|&(x, _)| self.w.node(x).map(|n| (n.text.op, n.text.args.to_vec())))
                .collect();
        }
        let n = self.claim.unwrap_or(0);
        (0..n).filter_map(|i| self.following_inner(i)).collect()
    }

    /// The `i`-th following sibling's text (never waits).
    #[must_use]
    pub fn following(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&i, )) } else { serde_json::Value::Null };
        let r = self.following_inner(i);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "following", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn following_inner(&mut self, i: usize) -> Option<(OpId, Vec<ValueId>)> {
        let (w, me) = (self.w, self.me);
        let got = w.forward(me, |_, x| w.taken_from(me, x)).nth(i);
        let ans = got.map(|x| (x, super::claims::key_of(w, x)));
        let rd = Read::Sib {
            i: u32::try_from(i).unwrap_or(u32::MAX),
            ans,
        };
        if !self.reads.contains(&rd) {
            self.reads.push(rd);
        }
        got.and_then(|x| w.node(x)).map(|n| (n.text.op, n.text.args.to_vec()))
    }

    /// The outputs of the stream this node claims: the following leaves up
    /// to the one its `until` hooks end it before.
    ///
    /// # Errors
    ///
    /// `Stop` while an element is in work.
    pub fn claim_stream(&mut self) -> Result<Vec<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.claim_stream_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "claim_stream", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn claim_stream_inner(&mut self) -> Result<Vec<ValueId>, Stop> {
        let got = self.w.stream(self.core, self.me);
        // An element that read a definition of this node's feeds the
        // stream itself: no order makes progress.
        let feeds = |x: Id| {
            self.w.node(x).and_then(|n| n.res.as_ref()).is_some_and(|r| {
                r.reads.iter().any(|rd| matches!(rd, Read::Use { site, .. } if *site == self.me))
            })
        };
        let fed = match &got {
            Ok((outs, _, _)) => outs.iter().map(|o| o.0).find(|&x| feeds(x)),
            Err(x) => Some(*x).filter(|&x| feeds(x)),
        };
        if let Some(x) = fed {
            self.error = Some(crate::api::EvalError::NoProgress {
                rounds: 0,
                waiters: vec![
                    crate::api::Waiter {
                        node: NodeId(self.me),
                        on: crate::api::WaitOn::Queued,
                    },
                    crate::api::Waiter {
                        node: NodeId(x),
                        on: crate::api::WaitOn::Node(NodeId(self.me), None),
                    },
                ],
                report: Box::default(),
            });
            return Err(self.stop(Wait::Node(x, None)));
        }
        match got {
            Ok((outs, end, fields)) => {
                let vals = outs.iter().map(|o| o.1).collect();
                self.reads.push(Read::Stream { outs, end, fields });
                Ok(vals)
            }
            Err(x) => Err(self.stop(Wait::Node(x, None))),
        }
    }

    /// The nearest previous sibling whose text passes `pred` (never waits).
    #[must_use]
    pub fn prev_sibling_where(&mut self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        #[cfg(feature = "observe")]
        if self.calls.borrow().is_some() {
            let asked = std::cell::RefCell::new(Vec::new());
            let r = self.prev_sibling_where_inner(&|op, args| {
                let b = pred(op, args);
                asked.borrow_mut().push((op, args.to_vec(), b));
                b
            });
            crate::record::call(&self.calls, "prev_sibling_where", crate::record::j(&asked.into_inner()), crate::record::j(&r));
            return r;
        }
        self.prev_sibling_where_inner(pred)
    }

    #[inline]
    fn prev_sibling_where_inner(&mut self, pred: &dyn Fn(OpId, &[ValueId]) -> bool) -> Option<NodeId> {
        let w = self.w;
        let mut walk = Vec::new();
        let mut ans = None;
        for (x, inert) in w.backward(self.me) {
            let Some(n) = w.node(x) else { break };
            walk.push((x, super::world::text_key(&n.text), inert));
            if !inert && pred(n.text.op, &n.text.args) {
                ans = Some(x);
                break;
            }
        }
        self.reads.push(Read::Prev { walk, ans });
        ans.map(NodeId)
    }

    /// Pushes `x` on sequence `f` in this node's scope.
    pub fn push_contribution(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        self.push_contribution_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "push_contribution", a, serde_json::Value::Null);
    }

    #[inline]
    fn push_contribution_inner(&mut self, f: Field, x: ValueId) {
        self.define_with_inner(f, x, Locality::Local);
    }

    fn seq(&mut self, f: Field, prev: bool) -> Result<Option<ValueId>, Stop> {
        if super::core::debug() {
            let ops = self.w.seq_ops(self.core, self.me, f);
            eprintln!("seq {} prev {prev}: {:?}", self.me, ops.map(|v| v.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>()));
        }
        match self.w.seq_read(self.core, self.me, f, prev) {
            Ok(val) => {
                self.reads.push(Read::Seq { f, prev, val });
                Ok(val)
            }
            Err(x) => Err(self.stop(Wait::Node(x, Some(f)))),
        }
    }

    /// The last element of sequence `f` in this node's scope.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn prev_contribution(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.prev_contribution_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn prev_contribution_inner(&mut self, f: Field) -> Result<Option<ValueId>, Stop> {
        self.seq(f, true)
    }

    /// Sequence `f` in this node's scope, before it (a sequence value).
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn contributions_before_me(&mut self, f: Field) -> Result<ValueId, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.contributions_before_me_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "contributions_before_me", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn contributions_before_me_inner(&mut self, f: Field) -> Result<ValueId, Stop> {
        Ok(self.seq(f, false)?.unwrap_or(ValueId::NIL))
    }

    /// Takes the last element of sequence `f` (replacing it by `r`): later
    /// readers see it gone; this one gets it.
    ///
    /// # Errors
    ///
    /// `Stop` while an earlier entry of the scope is to be made.
    pub fn take_prev_contribution(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &r, )) } else { serde_json::Value::Null };
        let r = self.take_prev_contribution_inner(f, r);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "take_prev_contribution", a, crate::record::stop_or(&r));
        r
    }

    #[inline]
    fn take_prev_contribution_inner(&mut self, f: Field, r: Option<ValueId>) -> Result<Option<ValueId>, Stop> {
        let got = self.seq(f, true)?;
        let v = &self.core.values;
        let x = match r {
            Some(r) => super::seq::repl(v, r),
            None => super::seq::take(v),
        };
        self.define_with_inner(f, x, Locality::Local);
        Ok(got)
    }

    /// Sets an environment field of the unit this application owns (an
    /// application's run only).
    pub fn set_env(&mut self, f: Field, x: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, &x, )) } else { serde_json::Value::Null };
        self.set_env_inner(f, x);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "set_env", a, serde_json::Value::Null);
    }

    #[inline]
    fn set_env_inner(&mut self, f: Field, x: ValueId) {
        if let Some(&i) = self.ix.get(&(3, f, 0)) {
            self.env[i].1 = x;
        } else {
            self.ix.insert((3, f, 0), self.env.len());
            self.env.push((f, x));
        }
    }

    /// Keeps `token` if this run stops: the next run gets it from
    /// `parked` when its reads so far still hold. The token saves work, not
    /// output: the next run makes its whole result again, its definitions,
    /// contributions and placements (the placements a stopped run made are
    /// made at its stop, and the next run placing them again in the same
    /// order names the same nodes; one it no longer places goes).
    pub fn park(&mut self, token: Box<dyn Any + Send>) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.park_inner(token);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "park", a, serde_json::Value::Null);
    }

    #[inline]
    fn park_inner(&mut self, token: Box<dyn Any + Send>) {
        self.kept = Some(token);
        self.park_cont = false;
    }

    /// As [`Ctx::park`], for a run that continues: resumed from `token`
    /// (its reads holding), the placements this run's attempts made so
    /// far stand, and it makes only new ones, after them; child and
    /// continuation indices (and [`Ctx::output_of`]) go on from theirs.
    /// A run that starts over (no token, a token whose reads broke,
    /// [`Ctx::restart`]) makes all of them, as with `park`, where every
    /// attempt makes all its placements again.
    pub fn park_continuing(&mut self, token: Box<dyn Any + Send>) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.park_inner(token);
        self.park_cont = true;
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "park_continuing", a, serde_json::Value::Null);
    }

    /// Marks a point of the run the next run may resume from: `mark`, the
    /// client's name for it (an undo-log position, say), and `state`, a
    /// version of the client's whole state there (equal states: the run
    /// goes on alike from there). A run resumed at an earlier mark
    /// ([`Ctx::resumed`]) that reaches a mark of its last run with an
    /// equal state, the same outputs so far and that run's reads after it
    /// still holding is cut off there: the last run's result stands (the
    /// call returns `Err`; the run returns it).
    ///
    /// A run resumed at a mark makes its whole result again, as one
    /// resumed from a park does: what it made before the mark too.
    ///
    /// # Errors
    ///
    /// [`Stop`] when the run is cut off.
    pub fn checkpoint(&mut self, mark: u32, state: ValueId) -> Result<(), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&mark, &state, )) } else { serde_json::Value::Null };
        let r = self.checkpoint_inner(mark, state);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "checkpoint", a, crate::record::stop_or(&r));
        r
    }

    fn checkpoint_inner(&mut self, mark: u32, state: ValueId) -> Result<(), Stop> {
        self.mark_at(mark, state, state, None)
    }

    /// Marks a point as [`Ctx::checkpoint`] does, and offers a cut off
    /// with a patch: `as_old` is the client's state with the slots it
    /// patches at the last run's values there, and `patch` the definitions
    /// this run's result makes otherwise than that run's, given that from
    /// here it runs as that run did (`None`: none of that field). The
    /// client answers that the last run never read those slots after this
    /// point, and that its definitions so far equal that run's but for
    /// them; the engine checks its other outputs. Cut off, the last
    /// run's result stands with `patch` applied; only the patched
    /// definitions' readers are asked again.
    ///
    /// # Errors
    ///
    /// [`Stop`] when the run is cut off.
    pub fn checkpoint_patched(&mut self, mark: u32, state: ValueId, as_old: ValueId, patch: &[(Field, Locality, Option<ValueId>)]) -> Result<(), Stop> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&mark, &state, &as_old, &patch, )) } else { serde_json::Value::Null };
        let r = self.mark_at(mark, state, as_old, Some(patch));
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "checkpoint_patched", a, crate::record::stop_or(&r));
        r
    }

    fn mark_at(&mut self, mark: u32, state: ValueId, as_old: ValueId, patch: Option<&[(Field, Locality, Option<ValueId>)]>) -> Result<(), Stop> {
        self.flush_gets();
        let (digest, odigest) = self.out_digests();
        let read_ix = u32::try_from(self.reads.len()).unwrap_or(u32::MAX);
        let (kids, conts) = (u32::try_from(self.kids.len()).unwrap_or(u32::MAX), u32::try_from(self.conts.len()).unwrap_or(u32::MAX));
        self.marks.push(super::world::Mark { mark, read_ix, state, digest, odigest, kids, conts });
        let Some(fm) = &self.from_mark else { return Ok(()) };
        let old = fm.old.clone();
        let patched = patch.is_some_and(|p| !p.is_empty());
        let Some(j) = (fm.j + 1..old.marks.len()).find(|&j| {
            let m = &old.marks[j];
            if patched { m.state == as_old && m.odigest == odigest } else { m.state == state && m.digest == digest }
        }) else {
            return Ok(());
        };
        // (Its last run's reads after the mark must hold; a run of an
        // application, which sets its unit's environment, goes on.)
        if !self.env.is_empty() || !self.apps.is_empty() {
            return Ok(());
        }
        let from = old.marks[j].read_ix as usize;
        if !old.reads[from..].iter().all(|r| super::build::holds(self.core, self.w, self.me, r)) {
            return Ok(());
        }
        self.cutoff = Some((j, patch.filter(|p| !p.is_empty()).map(<[_]>::to_vec)));
        Err(Stop { _private: () })
    }

    /// A digest of the run's outputs so far: its definitions,
    /// contributions, publications and environment by key, its placements,
    /// kinds and diagnostics in order, its effect and claim.
    fn out_digests(&self) -> (u64, u64) {
        use crate::values::hash_words;
        let mut d: u64 = 0;
        for x in &self.defs {
            d = d.wrapping_add(hash_words(1, &[x.f.0.raw(), u64::from(x.loc.entry_key()), x.val.raw()]));
        }
        let h = self.out_digest_rest();
        (h.wrapping_add(d), h)
    }

    /// [`Ctx::out_digests`] of the outputs but definitions.
    fn out_digest_rest(&self) -> u64 {
        use crate::values::hash_words;
        let mut h: u64 = 0;
        for (k, l) in [(2, &self.contribs), (3, &self.pubs), (4, &self.env)] {
            for &(f, x) in l {
                h = h.wrapping_add(hash_words(k, &[f.0.raw(), x.raw()]));
            }
        }
        for (k, l) in [(5, &self.kids), (6, &self.conts)] {
            for (i, s) in l.iter().enumerate() {
                let mut w: Vec<u64> = vec![i as u64, u64::from(s.op.0)];
                w.extend(s.args.iter().map(|a| a.raw()));
                h = h.wrapping_add(hash_words(k, &w));
            }
        }
        for (k, l) in [(7, &self.kinds), (8, &self.diag)] {
            for (i, x) in l.iter().enumerate() {
                h = h.wrapping_add(hash_words(k, &[i as u64, x.raw()]));
            }
        }
        h = h.wrapping_add(hash_words(9, &[self.claim.map_or(u64::MAX, |c| c as u64)]));
        h = h.wrapping_add(hash_words(10, &[u64::from(self.effect.is_some())]));
        h
    }

    /// Keeps `token` with the run's result: a later run of the node that
    /// resumes at one of this run's marks gets it ([`Ctx::resumed`]), and
    /// takes the client's state back to the mark from it (its undo log).
    pub fn keep_after(&mut self, token: Arc<dyn Any + Send + Sync>) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.after = Some(super::world::After(token));
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "keep_after", a, serde_json::Value::Null);
    }

    /// Resumed at a mark of the node's last run ([`Ctx::checkpoint`]): the
    /// token that run kept and the mark. The run goes on from the mark's
    /// state; its reads before the mark are its own (they still hold).
    pub fn resumed(&mut self) -> Option<(Arc<dyn Any + Send + Sync>, u32)> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.from_mark.as_ref().map(|m| (m.after.clone(), m.old.marks[m.j].mark));
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "resumed", a, crate::record::j(&r.as_ref().map(|r| r.1)));
        r
    }

    /// The token a stopped run of this node kept.
    pub fn parked(&mut self) -> Option<Box<dyn Any + Send>> {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        let r = self.parked_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "parked", a, crate::record::j(&r.is_some()));
        r
    }

    #[inline]
    fn parked_inner(&mut self) -> Option<Box<dyn Any + Send>> {
        self.kept.take()
    }

    /// Drops the kept state: the run starts over, and what its first pass
    /// read and made is not this run's.
    pub fn restart(&mut self) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&()) } else { serde_json::Value::Null };
        self.restart_inner();
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "restart", a, serde_json::Value::Null);
    }

    #[inline]
    fn restart_inner(&mut self) {
        self.kept = None;
        self.base = None;
        self.restart = true;
        self.reads.clear();
        self.read_ix.clear();
        self.raw_ix.clear();
        self.defs.clear();
        self.forwards.clear();
        self.views.clear();
        self.composed.clear();
        self.gets.clear();
        self.marks.clear();
        self.passes.clear();
        self.out_from = None;
        self.after = None;
        self.from_mark = None;
        self.cutoff = None;
        self.contribs.clear();
        self.pubs.clear();
        self.kids.clear();
        self.conts.clear();
        self.effect = None;
        self.kinds.clear();
        self.diag.clear();
        self.claim = None;
        self.env.clear();
        self.ix.clear();
        self.nkids = 0;
    }

    pub fn diagnostic(&mut self, msg: ValueId) {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&msg, )) } else { serde_json::Value::Null };
        self.diagnostic_inner(msg);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "diagnostic", a, serde_json::Value::Null);
    }

    #[inline]
    fn diagnostic_inner(&mut self, msg: ValueId) {
        self.diag.push(msg);
    }

    /// A pure text computation, memoized by the value store.
    pub fn memo_text(&self, a: ValueId, b: ValueId, produce: impl FnOnce(&Values) -> ValueId) -> ValueId {
        let key = (a, b);
        if let Some(&v) = self
            .core
            .text_memo
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return v;
        }
        let v = produce(&self.core.values);
        self.core
            .text_memo
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, v);
        v
    }

    /// Whether this run is speculative.
    #[must_use]
    pub fn is_speculative(&self) -> bool {
        false
    }

    /// The values' epoch: every [`ValueId`] this run makes or reads stays
    /// valid while it is unchanged.
    #[must_use]
    pub fn values_epoch(&self) -> u64 {
        0
    }

    /// Whether `f` is a loop-back field.
    pub fn is_loopback(&mut self, f: Field) -> bool {
        #[cfg(feature = "observe")]
        let a = if self.calls.borrow().is_some() { crate::record::j(&(&f, )) } else { serde_json::Value::Null };
        let r = self.is_loopback_inner(f);
        #[cfg(feature = "observe")]
        crate::record::call(&self.calls, "is_loopback", a, crate::record::j(&r));
        r
    }

    #[inline]
    fn is_loopback_inner(&mut self, f: Field) -> bool {
        let val = self.core.is_loop(f);
        self.reads.push(Read::IsLoop { f, val });
        val
    }

    /// Accounts `n` units of work.
    ///
    /// # Errors
    ///
    /// Never in this core.
    pub fn tick(&mut self, _n: u64) -> Result<(), Stop> {
        Ok(())
    }
}

/// The node of application `op` on `args`.
pub(crate) fn app_id(op: OpId, args: &[ValueId]) -> Id {
    let mut w: Vec<u64> = args.iter().map(|a| a.raw()).collect();
    w.push(u64::from(op.0));
    // (Never a placed id: those have bit 63; never a pushed one: small.)
    crate::values::hash_words(0xa99, &w) & !(1 << 63) | 1 << 62
}
