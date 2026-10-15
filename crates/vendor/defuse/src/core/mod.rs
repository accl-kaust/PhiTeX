//! The linear core (docs/linear-core.md): a run is a transaction over
//! predicates on a sharded, versioned store; no steps are needed for
//! correctness, commits happen on the producing worker, and one validator
//! serves waits, speculation and edits.

pub(crate) mod commit;
pub(crate) mod ctx;
pub(crate) mod dump;
pub(crate) mod edit;
pub(crate) mod eval;
pub(crate) mod order;
pub(crate) mod query;
pub(crate) mod report;
pub(crate) mod run;
#[cfg(feature = "observe")]
pub(crate) mod watch;

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use crate::api::{Brackets, Cause, Claim, Effect, Locality, MayDefine, Monoid, OpId, Operation};
use crate::sync::Arena;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};

/// An index into the entry arena.
pub(crate) type Slot = u32;
pub(crate) const NIL: Slot = u32::MAX;

// Entry statuses.
pub(crate) const S_FREE: u8 = 0;
/// Created or released: must run.
pub(crate) const S_NEW: u8 = 1;
/// Possibly affected: validate, run if a predicate broke.
pub(crate) const S_DIRTY: u8 = 2;
pub(crate) const S_RUN: u8 = 3;
pub(crate) const S_PARK: u8 = 4;
/// Committed in the current step of a stepped (single-worker) evaluation:
/// readers treat it as in work until the step ends.
pub(crate) const S_STEP: u8 = 5;
pub(crate) const S_CLEAN: u8 = 6;
pub(crate) const S_INERT: u8 = 7;
pub(crate) const S_MARKER: u8 = 8;

pub(crate) const fn in_work(s: u8) -> bool {
    matches!(s, S_NEW | S_DIRTY | S_RUN | S_PARK | S_STEP)
}

// Entry flags.
pub(crate) const F_STALE: u32 = 1;
pub(crate) const F_QUEUED: u32 = 1 << 1;
pub(crate) const F_NEEDS_RUN: u32 = 1 << 2;
/// The committed log has structure-sensitive predicates.
pub(crate) const F_SENS: u32 = 1 << 3;
pub(crate) const F_MARKED: u32 = 1 << 4;
/// A pending definition set with an Any/Prefixes bound.
pub(crate) const F_WILD: u32 = 1 << 5;
/// The static effect is Pending.
pub(crate) const F_PEFF: u32 = 1 << 6;
/// The static claim is Pending.
pub(crate) const F_PCLAIM: u32 = 1 << 7;
pub(crate) const F_HAS_OUT: u32 = 1 << 8;
/// Free me once my worker is done.
pub(crate) const F_DOOM_FREE: u32 = 1 << 9;
/// Make me inert once my worker is done.
pub(crate) const F_DOOM_INERT: u32 = 1 << 10;
/// Listed in its unit's in-work wild list.
pub(crate) const F_IN_WILD: u32 = 1 << 11;
/// Listed in its unit's in-work pending-effect list.
pub(crate) const F_IN_PEFF: u32 = 1 << 12;
/// Listed in its unit's in-work pending-claim list.
pub(crate) const F_IN_PCLAIM: u32 = 1 << 13;
/// A pending definition set (static defs `None`).
pub(crate) const F_PSET: u32 = 1 << 14;
/// An app node (memoized application).
pub(crate) const F_APP: u32 = 1 << 15;
/// Has a committed result.
pub(crate) const F_DONE: u32 = 1 << 16;
/// A static Count claimer.
/// The last commit took a guessed definition; a change to it is a misprediction.
pub(crate) const F_GUESS: u32 = 1 << 17;
pub(crate) const F_SCLAIM: u32 = 1 << 18;
/// Has forward reads (outputs of later nodes, streams).
pub(crate) const F_FWD: u32 = 1 << 19;
/// Chosen to run speculatively this step.
pub(crate) const F_SPECJOB: u32 = 1 << 20;
/// Running only to check its committed log.
pub(crate) const F_CHECKING: u32 = 1 << 21;
/// Physically in the unit's wild / pending-effect / pending-claim list (it
/// counts there only with `F_IN_*`; a node that left stays until the list
/// is compacted).
pub(crate) const F_AT_WILD: u32 = 1 << 22;
pub(crate) const F_AT_PEFF: u32 = 1 << 23;
pub(crate) const F_AT_PCLAIM: u32 = 1 << 24;
pub(crate) const F_AT_PFX: u32 = 1 << 25;
/// Its text declares its contributions: range readers fold them unrun.
pub(crate) const F_SCONTRIB: u32 = 1 << 26;
/// Its definition set is narrowed by its last run (`Ctx::narrow`).
pub(crate) const F_NARROWED: u32 = 1 << 27;
/// Its run is a guess on its previous inputs (`Speculation::Predicted`:
/// it has a result and other work is in flight).
pub(crate) const F_PREDRUN: u32 = 1 << 28;
/// Its committed run read a predicted definition
/// (`Operation::predicted_defs`) not yet confirmed or refuted.
pub(crate) const F_PREDREAD: u32 = 1 << 29;
/// A run on a prediction failed its commit: the next run reads what the
/// step committed.
pub(crate) const F_PREDREF: u32 = 1 << 30;
/// Its committed run read through a projection: a change is judged by its
/// check, not when it is notified.
pub(crate) const F_PROJREAD: u32 = 1 << 31;

/// One entry of a unit's order: a node or a scope marker. Hot fields are
/// atomics read without locks; the rest is behind `d`.
pub(crate) struct Rec {
    pub id: AtomicU64,
    pub gn: AtomicU32,
    pub unit: AtomicU32,
    pub label: AtomicU64,
    pub prev: AtomicU32,
    pub next: AtomicU32,
    pub depth: AtomicU32,
    pub parent: AtomicU32,
    /// The frame the entry is in (after its before-brackets).
    pub frame: AtomicU32,
    /// The frame at the entry's start.
    pub top_in: AtomicU32,
    /// The first of the frames this entry opens.
    pub frames: AtomicU32,
    pub nframes: AtomicU32,
    pub status: AtomicU8,
    pub flags: AtomicU32,
    pub out: AtomicU64,
    /// The generation of the committed log (subscriptions carry it).
    pub lgen: AtomicU32,
    /// Realised depth of its last run: the evaluation's epoch (high 32
    /// bits) and the depth (`Report::realised_critical_path`).
    pub rdep: AtomicU64,
    /// Realised time of its last run: its run's duration plus the longest
    /// realised time it read (ns; valid in `rdep`'s evaluation).
    pub rtime: AtomicU64,
    pub park_seq: AtomicU32,
    pub claimed_by: AtomicU32,
    pub d: Mutex<Box<Data>>,
}

impl Rec {
    pub(crate) fn st(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }
    pub(crate) fn fl(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }
    pub(crate) fn has(&self, f: u32) -> bool {
        self.fl() & f != 0
    }
    pub(crate) fn set(&self, f: u32) {
        self.flags.fetch_or(f, Ordering::AcqRel);
    }
    pub(crate) fn clear(&self, f: u32) {
        self.flags.fetch_and(!f, Ordering::AcqRel);
    }
    pub(crate) fn lab(&self) -> u64 {
        self.label.load(Ordering::Acquire)
    }
    #[track_caller]
    pub(crate) fn data(&self) -> MutexGuard<'_, Box<Data>> {
        lock(&self.d)
    }
}

/// What an entry is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Node,
    /// A scope opened or closed by the parent's `open_scope`/`close_scope`.
    Marker {
        open: bool,
    },
}

/// How the entry came to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    Top,
    Child,
    Cont,
    App,
}

/// What the operation's hooks say about the node's text.
#[derive(Clone, Debug)]
pub(crate) struct Static {
    pub defs: Option<Vec<(Field, Locality)>>,
    pub may: MayDefineS,
    pub effect: Effect,
    pub claim: ClaimS,
    /// What few texts declare (boxed: most declare none of it).
    pub rare: Option<Box<StaticRare>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StaticRare {
    pub limit: Option<usize>,
    /// The operation's read hint for the text.
    pub reads: Option<Vec<Field>>,
    /// The contributions the text declares (`Operation::static_contribs`).
    pub contribs: Option<Vec<(Field, ValueId)>>,
    /// The definitions the text is predicted to make
    /// (`Operation::predicted_defs`).
    pub preds: Option<Box<[(Field, ValueId)]>>,
}

impl Static {
    pub(crate) fn limit(&self) -> Option<usize> {
        self.rare.as_ref().and_then(|r| r.limit)
    }
    pub(crate) fn reads(&self) -> Option<&Vec<Field>> {
        self.rare.as_ref().and_then(|r| r.reads.as_ref())
    }
    pub(crate) fn contribs(&self) -> Option<&Vec<(Field, ValueId)>> {
        self.rare.as_ref().and_then(|r| r.contribs.as_ref())
    }
    pub(crate) fn preds(&self) -> Option<&[(Field, ValueId)]> {
        self.rare.as_ref().and_then(|r| r.preds.as_deref())
    }
}

impl Default for Static {
    fn default() -> Self {
        Static {
            defs: None,
            may: MayDefineS::default(),
            effect: Effect::None,
            claim: ClaimS::default(),
            rare: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum MayDefineS {
    #[default]
    Nothing,
    Any,
    Names(Vec<Field>),
    Prefixes(Vec<String>),
}

impl From<MayDefine> for MayDefineS {
    fn from(m: MayDefine) -> Self {
        match m {
            MayDefine::Nothing => MayDefineS::Nothing,
            MayDefine::Any => MayDefineS::Any,
            MayDefine::Names(n) => MayDefineS::Names(n),
            MayDefine::Prefixes(p) => MayDefineS::Prefixes(p),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ClaimS {
    #[default]
    None,
    Count(usize),
    Pending,
}

impl From<Claim> for ClaimS {
    fn from(c: Claim) -> Self {
        match c {
            Claim::None => ClaimS::None,
            Claim::Count(n) => ClaimS::Count(n),
            Claim::Pending => ClaimS::Pending,
        }
    }
}

/// A definition's value in the index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DefVal {
    /// Declared, not yet made by a committed run.
    Pending,
    Val(ValueId),
    /// Sequence markers.
    Push(ValueId),
    Take,
    Repl(ValueId),
}

/// A committed definition of a node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Def {
    pub f: Field,
    pub loc: Locality,
    pub val: DefVal,
}

/// The answer of a definer read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum DAns {
    Site {
        node: Slot,
        gn: u32,
        val: DefVal,
    },
    Env(Option<ValueId>),
    Loop(ValueId),
    /// A sequence read: the element / the list.
    Seq(Option<ValueId>),
}

/// What a sequence read asks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DMode {
    Use,
    Prev,
    Before,
}

/// A recorded read: the question and the answer it got.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Pred {
    Def {
        f: Field,
        at: Slot,
        mode: DMode,
        ans: DAns,
        /// Visibility depended on frames.
        sens: bool,
        /// Definition entries consulted (for value-change subscriptions).
        /// (A sequence read's; a use's is its answer's site.)
        sites: Box<[(Slot, u32)]>,
        /// Answered from a subject in work (speculation).
        spec: Option<Slot>,
        /// Read through this projection (0: in full).
        proj: u16,
        /// The run saw which node defined it (`use_field_at`): an equal
        /// value from another node is a change to it.
        who: bool,
    },
    /// A position read at node `node` ([`crate::Ctx::use_field_at`]):
    /// holds while it is there.
    At {
        node: Slot,
        gn: u32,
    },
    Out {
        node: Slot,
        gn: u32,
        val: Option<ValueId>,
        spec: bool,
    },
    Range {
        f: Field,
        at: Slot,
        from: Option<u64>,
        to: Option<u64>,
        val: ValueId,
        last: Slot,
    },
    Env {
        f: Field,
        val: Option<ValueId>,
    },
    Kind {
        at: Slot,
        val: Option<ValueId>,
        opener: Slot,
    },
    /// The i-th following sibling.
    Sib {
        i: u32,
        ans: Option<(Slot, u32, u64)>,
    },
    /// The siblings scanned backward and the one found.
    PrevSib {
        seen: Vec<(Slot, u32, u64)>,
        ans: Option<Slot>,
    },
    /// The claimed siblings' texts.
    Claimed {
        list: Vec<(Slot, u32, u64)>,
    },
    /// A stream claim: entries visited and the elements' outputs.
    Stream {
        seen: Vec<(Slot, u32, u64)>,
        outs: Vec<ValueId>,
        /// The fields `until` read, at the candidates.
        reads: Vec<(Slot, Field)>,
    },
    App {
        node: Slot,
        gn: u32,
        val: ValueId,
    },
}

/// The read a stopped run waits to ask again.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ask {
    Def(Field),
    Range(Field, Option<u64>, Option<u64>),
}

/// A wait recorded for the critical chain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct WaitRec {
    pub kind: &'static str,
    pub by: Slot,
    /// `by`'s id then (it may since have gone).
    pub id: u64,
    pub field: Option<Field>,
    pub prefix: Option<String>,
}

/// The committed result of a node's last run.
pub(crate) struct Committed {
    pub log: Vec<Pred>,
    pub defs: Vec<Def>,
    pub contribs: Vec<(Field, ValueId)>,
    pub pubs: Vec<(Field, ValueId)>,
    pub effect: Option<Effect>,
    pub output: ValueId,
    pub sig: u64,
    pub binds: u64,
    pub waits: Vec<WaitRec>,
}

/// A stopped run's state, taken up when its reads still hold.
pub(crate) struct Kept {
    pub log: Vec<Pred>,
    pub token: Option<Box<dyn Any + Send>>,
}

/// What an evaluation's canonical counters compare against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Snap {
    pub done: bool,
    pub sig: u64,
    pub out: Option<ValueId>,
    pub binds: u64,
}

/// The cold part of an entry.
pub(crate) struct Data {
    pub kind: Kind,
    pub role: Role,
    pub op: OpId,
    pub args: Arc<[ValueId]>,
    /// The hash of `(op, args)`, kept with them.
    pub thash: u64,
    /// Parent (children) or creator (continuations).
    pub owner: Slot,
    pub stat: Static,
    pub kids: Vec<Slot>,
    pub conts: Vec<Slot>,
    pub res: Option<Box<Committed>>,
    pub kept: Option<Box<Kept>>,
    /// Nodes parked on this one (`seq` `u32::MAX`: a speculation to
    /// validate).
    pub waiters: Vec<(Slot, u32)>,
    pub out_users: Vec<(Slot, u32)>,
    pub sib_users: Vec<(Slot, u32)>,
    /// The key of the last committed run's diagnostics in `Shared::diags`
    /// (0: none).
    pub diag: u64,
    /// Completed runs in this evaluation (`Limits::runs_per_node`).
    pub runs: u32,
    pub run_epoch: u64,
    pub causes: Vec<Cause>,
    pub snap_epoch: u32,
    pub born: u64,
    /// The committed kind of a kind-pending opener.
    pub kind_val: Option<ValueId>,
    /// What few nodes use (boxed: most never need it).
    pub x: Option<Box<Extra>>,
}

/// A node's rarely used state.
#[derive(Default)]
pub(crate) struct Extra {
    /// Nodes whose committed runs guessed this (in-work) node's value.
    pub guessed_by: Vec<Slot>,
    /// Of this node's guess targets, how many are still in work.
    pub guess_pending: u32,
    /// Waits since the last commit.
    pub waited: Vec<WaitRec>,
    pub claimed: Vec<Slot>,
    /// What a static claimer consumed (window) when its claim was computed.
    pub window: Vec<Slot>,
    pub reported: Option<(u64, ValueId)>,
    /// The unit an app node owns (`u32::MAX`: none).
    pub app_unit: u32,
    pub app_key: u64,
    /// What the node is parked on.
    pub wait_on: Option<query::Wait>,
    /// The read it stopped at, asked again before it runs.
    pub ask: Option<Ask>,
    /// A pending claimer's claim set by a run that then stopped: that
    /// attempt's answer, so siblings past it are not held (cleared when it
    /// runs again from the start).
    pub early_claim: Option<usize>,
    /// Where a check of the committed log that waited stopped: the next
    /// check starts there (reset by every touch).
    pub check_from: u32,
    /// That check waited where the one before it started.
    pub check_stuck: bool,
}

impl Data {
    /// The rarely used state, made on first use.
    pub(crate) fn ex(&mut self) -> &mut Extra {
        self.x.get_or_insert_with(|| {
            Box::new(Extra {
                app_unit: u32::MAX,
                ..Extra::default()
            })
        })
    }
    pub(crate) fn app_unit(&self) -> u32 {
        self.x.as_ref().map_or(u32::MAX, |e| e.app_unit)
    }
    pub(crate) fn app_key(&self) -> u64 {
        self.x.as_ref().map_or(0, |e| e.app_key)
    }
    pub(crate) fn claimed(&self) -> Vec<Slot> {
        self.x.as_ref().map_or_else(Vec::new, |e| e.claimed.clone())
    }
    pub(crate) fn take_claimed(&mut self) -> Vec<Slot> {
        self.x
            .as_mut()
            .map_or_else(Vec::new, |e| std::mem::take(&mut e.claimed))
    }
    /// Drops the rarely used state once nothing in it is set.
    pub(crate) fn trim_x(&mut self) {
        if self.x.as_ref().is_some_and(|e| {
            e.guessed_by.is_empty()
                && e.guess_pending == 0
                && e.waited.is_empty()
                && e.claimed.is_empty()
                && e.window.is_empty()
                && e.reported.is_none()
                && e.app_unit == u32::MAX
                && e.wait_on.is_none()
                && e.ask.is_none()
        }) {
            self.x = None;
        }
    }

    pub(crate) fn wait_on(&self) -> Option<&query::Wait> {
        self.x.as_ref().and_then(|e| e.wait_on.as_ref())
    }
}

/// The hash of a node's text.
pub(crate) fn text_hash_of(op: OpId, args: &[ValueId]) -> u64 {
    let mut w: Vec<u64> = args.iter().map(|a| a.raw()).collect();
    w.push(u64::from(op.0));
    crate::values::hash_words(77, &w)
}

impl Data {
    pub(crate) fn new(kind: Kind, role: Role, op: OpId, args: Arc<[ValueId]>, owner: Slot) -> Data {
        Data {
            kind,
            role,
            op,
            thash: text_hash_of(op, &args),
            args,
            owner,
            stat: Static::default(),
            kids: Vec::new(),
            conts: Vec::new(),
            res: None,
            kept: None,
            waiters: Vec::new(),
            out_users: Vec::new(),
            sib_users: Vec::new(),
            diag: 0,
            runs: 0,
            run_epoch: 0,
            causes: Vec::new(),
            snap_epoch: u32::MAX,
            born: 0,
            kind_val: None,
            x: None,
        }
    }
}

/// A scope instance. Frames are owned by the entry that opens them and
/// reused when the structure is recomputed.
pub(crate) struct Frame {
    pub unit: AtomicU32,
    /// The opening entry (`NIL` for a unit's root frame).
    pub opener: AtomicU32,
    pub parent: AtomicU32,
    pub depth: AtomicU32,
    /// 0: no kind, 1: `kind` holds it, 2: set by the opener's run.
    pub kmode: AtomicU8,
    pub kind: AtomicU64,
}

pub(crate) const K_NONE: u8 = 0;
pub(crate) const K_VALUE: u8 = 1;
pub(crate) const K_PENDING: u8 = 2;

/// A unit.
pub(crate) struct Unit {
    pub first: AtomicU32,
    pub last: AtomicU32,
    pub root_frame: u32,
    /// The app node owning the unit (`NIL`: a client unit).
    pub app: AtomicU32,
    pub live: AtomicBool,
    /// Serialises writers (commits and edits of the unit).
    pub s: RwLock<()>,
    /// Odd while a writer holds `s`: lock-free readers retry when it moved.
    pub sv: AtomicU64,
    /// A writer holds or waits for `s` (committing readers stay out).
    pub wflag: AtomicBool,
    /// Committing readers in, by thread stripe (made at the first).
    pub stripes: std::sync::OnceLock<Box<[Pad]>>,
    /// Odd while labels are being rewritten.
    pub label_epoch: AtomicU64,
    /// Bumped when existing entries' frames change.
    pub frame_epoch: AtomicU64,
    pub env: crate::sync::BrLock<FxHashMap<Field, ValueId>>,
    /// In-work nodes whose pending definition sets are not named fields.
    pub wild: crate::sync::BrLock<Vec<Slot>>,
    /// In-work nodes whose pending definition sets are prefixes, by
    /// prefix (label order; entries count only with `F_IN_WILD`).
    pub pfx: crate::sync::BrLock<FxHashMap<String, Vec<Slot>>>,
    /// The distinct lengths of the prefixes in `pfx`, ascending.
    pub pfx_lens: crate::sync::BrLock<Vec<usize>>,
    /// In-work nodes whose effect is pending.
    pub peff: crate::sync::BrLock<Vec<Slot>>,
    /// In-work pending claimers.
    pub pclaim: crate::sync::BrLock<Vec<Slot>>,
    /// Every entry of `pclaim` below this index has left (they leave
    /// mostly in label order, so a scan starts here, not at 0).
    pub pclaim_lo: std::sync::atomic::AtomicUsize,
    /// Guards the lists; holds how many entries in them have left.
    pub lists: Mutex<usize>,
}

/// A definition entry in a field index.
#[derive(Clone, Debug)]
pub(crate) struct DefE {
    pub node: Slot,
    pub loc: Locality,
    pub val: DefVal,
    pub users: Vec<(Slot, u32)>,
    /// The frame the entry is filed under (its node's frame).
    pub fk: u32,
}

/// A subscription of a reader at a position.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sub {
    pub node: Slot,
    pub gn: u32,
    pub at: Slot,
    /// For range readers: the anchor (`NIL`: the unit's start).
    pub from: Slot,
}

/// Everything known about one field in one unit.
#[derive(Default)]
pub(crate) struct FieldIdx {
    /// Every entry, by label.
    pub defs: Vec<DefE>,
    /// The entries by the frame they are in, by label.
    pub local: FxHashMap<u32, Vec<Slot>>,
    /// The global entries, by label.
    pub globals: Vec<Slot>,
    /// The local entries outside the root frame, by label.
    pub nonroot: Vec<Slot>,
    /// Every reader, by position.
    pub readers: Vec<Sub>,
    /// The readers by each frame of their scope chain, by position.
    pub freaders: FxHashMap<u32, Vec<Sub>>,
    /// Sequence readers by their frame, by position (they read every
    /// entry of their frame before them).
    pub sreaders: FxHashMap<u32, Vec<Sub>>,
    pub named: Vec<Slot>,
    pub contribs: Vec<(Slot, ValueId)>,
    pub range_readers: Vec<Sub>,
}

pub(crate) const SHARDS: usize = 256;

pub(crate) type FieldMap = FxHashMap<(u32, Field), Box<FieldIdx>>;

/// The environment: supplied values and fields declared absent.
#[derive(Default)]
pub(crate) struct EnvState {
    pub known: FxHashMap<Field, ValueId>,
    pub absent: FxHashSet<Field>,
    /// Fields the provider did not know, this evaluation.
    pub unknown: FxHashSet<Field>,
    /// The provider's answers (`None`: absent), kept apart from what the
    /// client sets so that setting the environment again keeps them.
    pub provided: FxHashMap<Field, Option<ValueId>>,
}

impl EnvState {
    /// What a read of `f` finds: `Some(answer)`, or `None` unknown.
    #[allow(clippy::option_option)]
    pub(crate) fn get(&self, f: Field) -> Option<Option<ValueId>> {
        if let Some(&x) = self.known.get(&f) {
            return Some(Some(x));
        }
        if self.absent.contains(&f) {
            return Some(None);
        }
        self.provided.get(&f).copied()
    }
}

/// An environment provider.
pub(crate) type Provider = Box<crate::api::EnvProvider>;

/// Ready nodes, lowest label first.
pub(crate) type ReadyHeap = std::collections::BinaryHeap<std::cmp::Reverse<(u64, Slot)>>;

/// Ready-queue shards when running free. One: a worker takes the lowest
/// label overall. With 8 (a worker's own first) the queue's lock was
/// cheaper, but nodes ran far out of text order, and under
/// `Speculation::All` readers guessed past definers that had not run
/// (loader client at 12 workers: 5.5k runs became 21k, 0.35 s 1.4 s).
pub(crate) const QSHARDS: usize = 1;

/// The work queue: the current step's nodes and the next step's.
#[derive(Default)]
pub(crate) struct QueueInner {
    /// Ready nodes, lowest order label first (text order within a unit).
    pub cur: std::collections::BinaryHeap<std::cmp::Reverse<(u64, Slot)>>,
    pub next: Vec<Slot>,
}

/// Per-evaluation counters, shared by the workers.
#[derive(Default)]
pub(crate) struct Counters {
    pub runs: AtomicU64,
    pub stops: AtomicU64,
    pub raises: AtomicU64,
    pub pending_waits: AtomicU64,
    pub pending_defining: AtomicU64,
    pub speculations: AtomicU64,
    pub confirmations: AtomicU64,
    pub mispredictions: AtomicU64,
    pub fuel_outs: AtomicU64,
    pub spec_stops: AtomicU64,
    pub resumed: AtomicU64,
    pub restarted: AtomicU64,
    pub declarations_refuted: AtomicU64,
    pub static_contribs_refuted: AtomicU64,
    pub predictions_confirmed: AtomicU64,
    pub predictions_refuted: AtomicU64,
    pub realised: AtomicU64,
    pub realised_ns: AtomicU64,
    pub env_provided: AtomicU64,
    pub commits: AtomicU64,
    pub scope_region: AtomicU64,
    pub created: AtomicU64,
    pub freed: AtomicU64,
    pub gated: AtomicU64,
}

/// Counters keyed by name or node, merged at the end.
#[derive(Default)]
pub(crate) struct Tallies {
    pub runs_by_op: FxHashMap<&'static str, u64>,
    /// Each node's run time this evaluation, all attempts (ns).
    pub run_ns: FxHashMap<Slot, u64>,
    pub waits_by_op: FxHashMap<&'static str, u64>,
    pub waits_by_kind: FxHashMap<&'static str, u64>,
    pub waited: FxHashMap<Slot, u64>,
    pub restarted_by: FxHashMap<&'static str, u64>,
    pub refuted_by_field: FxHashMap<Field, u64>,
    pub refuted_nodes: FxHashMap<Slot, u32>,
}

/// The store and the evaluation state every worker shares.
pub(crate) struct Shared {
    pub values: Values,
    pub ops: Vec<Arc<dyn Operation>>,
    /// Client projections (`ProjId` 2 on).
    pub projs: Vec<crate::api::Projection>,
    pub op_names: Vec<&'static str>,
    pub recs: Arena<Rec>,
    pub frames: Arena<Frame>,
    pub units: Arena<Unit>,
    pub fields: Box<[RwLock<FieldMap>]>,
    pub by_id: Box<[Mutex<FxHashMap<u64, Slot>>]>,
    pub accs: FxHashMap<Field, Arc<dyn Monoid>>,
    pub seqs: FxHashSet<Field>,
    pub loops: FxHashMap<Field, ValueId>,
    pub loop_vals: crate::sync::BrLock<FxHashMap<Field, ValueId>>,
    pub env: crate::sync::BrLock<EnvState>,
    pub provider: Option<Provider>,
    pub apps: Mutex<FxHashMap<u64, Slot>>,
    pub env_readers: Mutex<FxHashMap<Field, Vec<(Slot, u32)>>>,
    pub loop_readers: Mutex<FxHashMap<Field, Vec<(Slot, u32)>>>,
    pub env_waits: Mutex<FxHashMap<Field, Vec<(Slot, u32)>>>,
    pub pubs: Mutex<FxHashMap<Field, FxHashMap<Slot, ValueId>>>,
    /// The loop-back fields whose publications changed since the last
    /// round's end (that round looks at these only).
    pub pubs_dirty: Mutex<crate::values::FxHashSet<Field>>,
    pub undeclared: Mutex<FxHashMap<Slot, Field>>,
    pub q: Mutex<QueueInner>,
    /// Free running: the ready nodes, in `QSHARDS` heaps (a worker takes
    /// from its own first), each lowest label first.
    pub shards: Box<[Mutex<ReadyHeap>]>,
    /// Workers handed shard indexes so far.
    pub next_worker: AtomicUsize,
    /// Nodes queued or being processed (zero: free running is quiet).
    pub work: AtomicUsize,
    pub qlen: AtomicUsize,
    /// Idle workers asleep on `idle_cv`.
    pub sleepers: AtomicUsize,
    pub idle_mx: Mutex<()>,
    pub idle_cv: std::sync::Condvar,
    pub busy: AtomicUsize,
    /// A single worker in steps: commits become visible at step ends.
    pub stepped: AtomicBool,
    pub step_done: Mutex<Vec<Slot>>,
    pub parked: Mutex<Vec<Slot>>,
    pub touched: Mutex<Vec<Slot>>,
    /// The finished nodes' state as the evaluation found them.
    pub snaps: Mutex<FxHashMap<Slot, Snap>>,
    /// Why nodes were made dirty, in order (for `Report::rerun_causes`).
    /// Some text has predicted definitions.
    pub any_preds: AtomicBool,
    /// `ProjId::IS_EMPTY` was read through.
    pub any_proj: AtomicBool,
    /// (`None`: it ran.)
    pub reruns: Mutex<Vec<(Slot, Option<crate::api::Rerun>)>>,
    /// Nodes whose height may have changed since the last report.
    pub hdirty: Mutex<Vec<Slot>>,
    /// Committed runs' diagnostics (kept for the engine's lifetime).
    pub diags: crate::sync::Table<Box<[ValueId]>>,
    pub diag_next: AtomicU64,
    /// `Ctx::memo_text`'s table.
    pub text_memo: RwLock<FxHashMap<(ValueId, ValueId), ValueId>>,
    /// The marked nodes (some may since be unmarked or freed).
    pub marked: Mutex<Vec<Slot>>,
    /// Operations whose new nodes are marked at creation.
    pub marked_ops: RwLock<Vec<OpId>>,
    /// Frame walks the edits left for the evaluation.
    pub deferred_frames: Mutex<Vec<Slot>>,
    pub abort: AtomicBool,
    pub error: Mutex<Option<crate::api::EvalError>>,
    pub c: Counters,
    pub t: Mutex<Tallies>,
    /// How often each node was taken to run this evaluation (`observe`).
    #[cfg(feature = "observe")]
    pub selects: Mutex<FxHashMap<Slot, u32>>,
    /// The canonical window (evaluation number).
    pub epoch: AtomicU64,
    pub spec: AtomicU8,
    /// Guesses one speculative run may take.
    pub fuel: AtomicU64,
    /// Committed runs resting on guesses not yet confirmed, and the cap.
    pub outstanding: std::sync::atomic::AtomicUsize,
    pub max_outstanding: std::sync::atomic::AtomicUsize,
    pub limits: RwLock<crate::api::Limits>,
    pub deadline: Mutex<Option<std::time::Instant>>,
    pub live: AtomicU64,
    pub next_top: AtomicU64,
}

pub(crate) const SPEC_SEQ: u32 = u32::MAX;

impl Shared {
    pub(crate) fn rec(&self, s: Slot) -> &Rec {
        self.recs.get(s as usize)
    }

    pub(crate) fn unit(&self, u: u32) -> &Unit {
        self.units.get(u as usize)
    }

    pub(crate) fn frame(&self, f: u32) -> &Frame {
        self.frames.get(f as usize)
    }

    pub(crate) fn shard(&self, u: u32, f: Field) -> &RwLock<FieldMap> {
        #[allow(clippy::cast_possible_truncation)]
        let h = crate::values::mix(f.0.raw() ^ (u64::from(u) << 40)) as usize;
        &self.fields[h % SHARDS]
    }

    pub(crate) fn id_shard(&self, id: u64) -> &Mutex<FxHashMap<u64, Slot>> {
        #[allow(clippy::cast_possible_truncation)]
        let h = crate::values::mix(id) as usize;
        &self.by_id[h % self.by_id.len()]
    }

    /// The slot of a live node.
    pub(crate) fn slot_of(&self, id: u64) -> Option<Slot> {
        lock(self.id_shard(id)).get(&id).copied()
    }

    pub(crate) fn node_id(&self, s: Slot) -> crate::NodeId {
        crate::NodeId(self.rec(s).id.load(Ordering::Acquire))
    }

    pub(crate) fn stepped(&self) -> bool {
        self.stepped.load(Ordering::Relaxed)
    }

    /// Whether `s` is in work for a strict read.
    pub(crate) fn busy_node(&self, s: Slot) -> bool {
        in_work(self.rec(s).st())
    }

    pub(crate) fn op_name(&self, op: OpId) -> &'static str {
        self.op_names.get(op.0 as usize).copied().unwrap_or("op")
    }
}

/// A unit's writer: excludes other writers; readers that overlap it retry.
pub(crate) struct UnitW<'a> {
    _g: std::sync::RwLockWriteGuard<'a, ()>,
    un: &'a Unit,
}

impl Drop for UnitW<'_> {
    fn drop(&mut self) {
        self.un.sv.fetch_add(1, Ordering::Release);
        self.un.wflag.store(false, Ordering::SeqCst);
    }
}

/// A counter on a cache line of its own.
#[repr(align(64))]
#[derive(Default)]
pub(crate) struct Pad(pub AtomicU32);

/// Stripes of a unit's committing readers.
pub(crate) const STRIPES: usize = 32;

thread_local! {
    /// This thread's stripe of the committing-reader counters.
    static STRIPE: usize = {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed) % STRIPES
    };
}

/// A commit that changes no structure of its unit: excludes its writers
/// (`UnitW`) without the readers sharing a lock word.
pub(crate) struct UnitR<'a> {
    c: &'a AtomicU32,
}

impl Drop for UnitR<'_> {
    fn drop(&mut self) {
        self.c.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Shared {
    pub(crate) fn wunit(&self, u: u32) -> UnitW<'_> {
        let un = self.unit(u);
        let g = wlock(&un.s);
        // Committing readers come in only while no writer is flagged; wait
        // out those already in.
        un.wflag.store(true, Ordering::SeqCst);
        if let Some(st) = un.stripes.get() {
            for p in st {
                let mut spins = 0u32;
                while p.0.load(Ordering::SeqCst) != 0 {
                    spins += 1;
                    if spins < 128 {
                        std::hint::spin_loop();
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
        }
        un.sv.fetch_add(1, Ordering::AcqRel);
        UnitW { _g: g, un }
    }

    /// Enters a commit that changes no structure of unit `u` (see `UnitR`).
    #[track_caller]
    pub(crate) fn runit(&self, u: u32) -> UnitR<'_> {
        let un = self.unit(u);
        let st = un
            .stripes
            .get_or_init(|| (0..STRIPES).map(|_| Pad::default()).collect());
        let c = &st[STRIPE.with(|s| *s)].0;
        #[cfg(feature = "profile")]
        let t0 = std::time::Instant::now();
        let mut spins = 0u32;
        loop {
            c.fetch_add(1, Ordering::SeqCst);
            if !un.wflag.load(Ordering::SeqCst) {
                #[cfg(feature = "profile")]
                if spins > 0 {
                    #[allow(clippy::cast_possible_truncation)]
                    crate::obs::lock_wait(std::panic::Location::caller(), t0.elapsed().as_nanos() as u64);
                }
                return UnitR { c };
            }
            c.fetch_sub(1, Ordering::SeqCst);
            while un.wflag.load(Ordering::SeqCst) {
                spins += 1;
                if spins < 128 {
                    std::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                }
            }
        }
    }

    /// Runs a read of unit `u`'s structure without blocking: again while a
    /// writer overlapped it.
    #[track_caller]
    pub(crate) fn sread<T>(&self, u: u32, mut f: impl FnMut() -> T) -> T {
        let sv = &self.unit(u).sv;
        let mut spins = 0u32;
        #[cfg(feature = "profile")]
        let t0 = std::time::Instant::now();
        loop {
            let v = sv.load(Ordering::Acquire);
            if v & 1 == 0 {
                let r = f();
                std::sync::atomic::fence(Ordering::Acquire);
                if sv.load(Ordering::Acquire) == v {
                    #[cfg(feature = "profile")]
                    if spins > 0 {
                        #[allow(clippy::cast_possible_truncation)]
                        crate::obs::lock_wait(std::panic::Location::caller(), t0.elapsed().as_nanos() as u64);
                    }
                    return r;
                }
            }
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
}

/// Takes a lock; profiling builds time a blocked acquisition by call site.
macro_rules! take_lock {
    ($try:expr, $block:expr) => {{
        #[cfg(feature = "profile")]
        {
            match $try {
                Ok(g) => return g,
                Err(std::sync::TryLockError::Poisoned(p)) => return p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            let t = std::time::Instant::now();
            let g = $block.unwrap_or_else(std::sync::PoisonError::into_inner);
            #[allow(clippy::cast_possible_truncation)]
            crate::obs::lock_wait(std::panic::Location::caller(), t.elapsed().as_nanos() as u64);
            g
        }
        #[cfg(not(feature = "profile"))]
        {
            $block.unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }};
}

#[track_caller]
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    take_lock!(m.try_lock(), m.lock())
}

#[track_caller]
pub(crate) fn rlock<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    take_lock!(m.try_read(), m.read())
}

#[track_caller]
pub(crate) fn wlock<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    take_lock!(m.try_write(), m.write())
}

/// Brackets split into before and after the entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Br {
    pub cb: u8,
    pub ob: u8,
    pub ca: u8,
    pub oa: u8,
}

impl Br {
    pub(crate) fn of(b: &Brackets) -> Br {
        if b.before {
            Br {
                cb: b.closes,
                ob: b.opens,
                ca: 0,
                oa: 0,
            }
        } else {
            Br {
                cb: 0,
                ob: 0,
                ca: b.closes,
                oa: b.opens,
            }
        }
    }
}
