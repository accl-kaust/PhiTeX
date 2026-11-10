//! A node and what its last run left.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::api::{Brackets, Cause, Effect, Locality, NodeId, OpId, PathId, Rel, UnitId};
use crate::index::RangeKey;
use crate::lock::{Lock, Mx};
use crate::order::{Pos, SegId};
use crate::segment::RangeReader;
use crate::values::{Field, ValueId};

/// A node's state: one byte, changed by compare-and-swap, so a re-ask never
/// takes a lock on the node.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum State {
    /// Ran; nothing it read has changed since.
    Clean = 0,
    /// To run: on a worker's deque or the injector.
    Dirty = 1,
    Running = 2,
    /// Re-asked while running: its run commits, then it runs again. Its
    /// reads return `Stop` from the moment it is marked.
    RunningDirty = 3,
    /// Taken by a claim.
    Inert = 4,
    /// Removed: unfiled, never run again.
    Gone = 5,
    /// Taken by a claim while a run of it is in progress: its reads return
    /// `Stop`, its commit files nothing, and its worker makes it `Inert`;
    /// given back meanwhile, it is `RunningDirty`, run again by that worker,
    /// never by a second one beside it.
    RunningInert = 6,
}

pub(crate) struct Node {
    pub(crate) id: NodeId,
    pub(crate) unit: UnitId,
    /// Its operation and arguments: its text, swapped by an edit
    /// (`Engine::replace`), cloned by a read.
    pub(crate) op: AtomicU32,
    pub(crate) text: Lock<Arc<[ValueId]>>,
    pub(crate) parent: Option<NodeId>,
    /// Its segment and label, written under the owning segment's write
    /// lock (a relabel, a split), read under its read lock. A lock-free read
    /// (`pos_hint`) is good enough to find the segment to lock.
    pub(crate) seg: AtomicU32,
    pub(crate) pos: AtomicU64,
    pub(crate) state: AtomicU8,
    /// Its commit has filed its reads and answered them under its lock and
    /// it is still `Running`: a walk that finds one of them changed marks it
    /// as `sched::mark_dirty` does.
    pub(crate) committing: AtomicBool,
    /// A `next` reader was filed as stopped here while the node ran
    /// (`commit::file_stop`): its commit may have taken its stops before
    /// that filing, so the end of the run takes them again
    /// (`commit::reask_late_stops`).
    pub(crate) late_stops: AtomicBool,
    /// Runs in this evaluation (`Limits::runs_per_node`), the evaluation
    /// they count in.
    pub(crate) runs: AtomicU32,
    pub(crate) run_epoch: AtomicU64,
    /// Its runs in this evaluation that were not resumptions
    /// (`Report::resumed`): a second one is a refuted node.
    pub(crate) reruns: AtomicU32,
    /// The field its last commit's run parked on (`Ctx::parked_on`).
    pub(crate) parked_on: Mx<Option<Field>>,
    /// Bumped at each commit; a reader record carrying an older serial is
    /// stale and dropped when met, so a commit never visits other segments
    /// to retract its node's old reads. Zero: never committed.
    pub(crate) serial: AtomicU32,
    /// What the last commit left, diffed against by the next; a record
    /// filed before any run (`provisional`) holds predictions only.
    pub(crate) last: Mx<Option<Arc<RunRecord>>>,
    /// Who woke it last and why: the realised chain.
    pub(crate) woke: Mx<Option<Wake>>,
    /// A re-ask that came while it ran (`sched::mark_stale` leaves a
    /// running node to its commit): the wake, for the run's next check
    /// (`Ctx::tick`) to stop it on if one of its reads moved; the flag
    /// says one waits, read without the lock.
    pub(crate) stale_wake: Mx<Option<Wake>>,
    pub(crate) stale: AtomicBool,
    /// The first commit whose filed readers stand
    /// ([`crate::commit::FILED`], [`ReadRec::filed`]): every commit after it
    /// went on from the run before and kept its reads.
    pub(crate) filed_floor: AtomicU32,
    /// A re-ask came while this run ran. Unlike [`Node::stale`] it stays
    /// set to the commit: what the commit does not check of itself, the
    /// filed reads, it checks when this says one may have moved.
    pub(crate) asked: AtomicBool,
    /// Its wake is more than one: a re-ask came while it was queued (or a
    /// re-ask kept for a run's check while one was) and only one cause is
    /// told ([`crate::sched`]'s `merge_woke`, `note_stale`). The next run
    /// takes it (`Ctx::new`): woken by the answer it parked on, it is not
    /// woken by that answer alone, and its commit checks the reads it kept.
    pub(crate) woke_merged: AtomicBool,
    /// Fields whose walks stopped at it while it had not committed
    /// (`commit::first_hole`), each with the scope depth of the definition
    /// walked from: walked on from it when it commits, is taken or is
    /// removed.
    pub(crate) stopped: Mx<Vec<(Field, u16)>>,
    /// Runs that met it as the first node before them that has not
    /// committed (`Ctx::hole_before`), with their serial then: asked
    /// again when it commits, is taken, is removed or moves.
    pub(crate) waiters: Mx<Vec<(NodeId, u32)>>,
    /// Range readers whose range starts or ends at it: asked again when
    /// it is removed (their bound gone, they answer none).
    pub(crate) bounded: Mx<Vec<(Field, RangeKey, RangeReader)>>,
    /// Nanoseconds of its last run, its commit and its walk forward, and
    /// when it committed (from the evaluation's start).
    pub(crate) run_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) done_ns: AtomicU64,
    /// Reruns caused by `At::Final` re-asks in this evaluation
    /// (`Limits::final_reruns`), and the build they count in (`Engine::final_gen`); past
    /// the limit the node is capped.
    pub(crate) final_reruns: AtomicU32,
    pub(crate) final_epoch: AtomicU64,
    /// Runs wait for this node's commit (`Shared::waiting` has an entry
    /// for it): set under that lock by the run that parks, cleared under
    /// it by the commit that releases them; a commit that finds it unset
    /// takes no lock (`sched::release_waiting`).
    pub(crate) awaited: AtomicBool,
    /// The node whose claim took it (raw id + 1; 0: none), set and
    /// cleared under its segment's write lock (`commit::apply_claim`): a
    /// claim passes over a sibling another claimer holds, and gives back
    /// only its own.
    pub(crate) claimer: AtomicU64,
    /// The furthest sibling its claim holds (raw id + 1; 0: none): a
    /// claim made smaller gives back what it holds up to there, past
    /// siblings it never held.
    pub(crate) claim_end: AtomicU64,
    /// Held over one claim's walk of its siblings (`commit::apply_claim`:
    /// `claim_end` read, the siblings taken or given back, `claim_end`
    /// stored): two walks of one claimer (its run's claim, its commit's,
    /// the give-back when it is taken or removed) interleaved, the later
    /// store left `claim_end` short of a sibling the other took, which no
    /// give-back reached again. True once it leaves the line
    /// (`commit::release_leaving`): from then on it claims nothing.
    pub(crate) claim_mx: Mx<bool>,
    /// A run of it stopped at the settling layer's frontier
    /// (`Ctx::hold_at_frontier`, `Ctx::wait_settled`): one begun past the
    /// frontier would most likely stop there again, its work thrown away,
    /// so it waits for the frontier first (`sched::work`). Only when it
    /// runs: what it makes is what it would make anyway.
    pub(crate) held_frontier: AtomicBool,
    /// Its last run's distance past the frontier where it read, in places
    /// plus one (0: not past it): a refutation of that run halves the
    /// speculation window (docs/spec-window.md).
    pub(crate) ahead: std::sync::atomic::AtomicU64,
    /// Its run was held by the window alone, not a read of the layer's
    /// (`Ctx::hold_at_frontier`): it waits for the window, not the
    /// frontier (`sched::frontier_holds`).
    pub(crate) held_window: AtomicBool,
    /// Let go from a hold of the window's (`sched::release_frontier`):
    /// its next run is not held by the window again, whatever the width
    /// is when it starts (docs/spec-window.md, Progress: a release is a
    /// run, never a hold again).
    pub(crate) window_pass: AtomicBool,
    /// Let go past the settling layer's frontier with nothing else to run
    /// (`Pool::release_past_only`): its next run's reads of the layer's
    /// fields are not held at the frontier, which stays where the layer
    /// set it.
    pub(crate) frontier_pass: AtomicBool,
    /// The fields a commit of a run re-asked while it ran changed but did
    /// not walk (`commit::commit`): the node runs again at once, and its
    /// next commit, of whatever kind, or its removal walks them
    /// (`commit::flush_unwalked`), against what each reader read.
    pub(crate) unwalked: Mx<Vec<Field>>,
    /// Its runs ahead of the frontier refuted by the page layer
    /// (`commit::reask_reader`; `PHITEX_WINDOW_WAVE`).
    pub(crate) layer_refuted: AtomicU32,
    /// A wake of it moved the settling layer's frontier back
    /// (`sched::frontier_back`): the frontier it set, the one it replaced
    /// and `Engine::frontier_gen` then. Its commit puts the replaced one
    /// back when nothing has set the frontier since and the layer is not
    /// to run again (`sched::frontier_restore`).
    pub(crate) back_frontier: Mx<Option<(u64, u64, u64)>>,
    /// Its re-asks for fields changed by commits still going on: the
    /// committing node, that commit's run serial, the node's `restated`
    /// then and the field (`sched::asked_before`).
    pub(crate) asks: Mx<Vec<(NodeId, u32, u32, Field)>>,
    /// Counts the times a value it filed was put right in place after it
    /// was filed (a relation counted again from its input,
    /// `commit::settle_rel`, `commit::walk_here`): a re-ask for its change
    /// made before carried the older value (`sched::asked_before`).
    pub(crate) restated: AtomicU32,
    /// Placed as its placer's continuation (`Ctx::follow`,
    /// `Ctx::follow_after`), not as its child: a node placed after the
    /// placer and its children goes before it.
    pub(crate) follows: AtomicBool,
    /// The node it was placed right after (raw id + 1; 0: pushed by the
    /// client, after none): the `at` of a `Ctx::follow_after`, else the
    /// placer or the placement before it. A placement kept where it stood
    /// is checked against it (`commit::stranded`).
    pub(crate) anchor: AtomicU64,
    /// Its parked state (`Ctx::park`) rests on a read that moved: a run
    /// that went on from it read one kept from the run before that no
    /// longer holds. The next run is offered none (`Ctx::resumed`).
    pub(crate) state_stale: AtomicBool,
    /// Its last commit's definitions are none of a settling layer's
    /// decisions (`Ctx::settle_now`): a reader outside the layer is
    /// re-asked at once, not held to the quiet point
    /// (`sched::settles_edge`).
    pub(crate) settle_now: AtomicBool,
    /// Its last run panicked on inputs that hold, held for
    /// `Engine::evaluate`'s decision once the line is quiet: no run of it
    /// comes in this pool, so no run waits for its commit
    /// (`sched::settled_node`).
    pub(crate) held_panic: AtomicBool,
    /// Where it stands among the nodes placed at its anchor: 0 a child
    /// of it, 1 after it and its children (another's `Ctx::follow_after`,
    /// or the placement after it in its placer's run), 2 its own
    /// continuation (`Ctx::follow`). Those of a lower slot stand before
    /// those of a higher one.
    pub(crate) slot: AtomicU8,
    /// Whether its last commit was refuted by the commit itself (a read it
    /// resumed on moved): its record's reads may hold while the state it
    /// parked rests on the old answers (`Ctx::last_run_stands`).
    pub(crate) refuted: AtomicBool,
    /// Its last commit that published anything moved what a later node
    /// read, or it never ran (`sched::note_move`): those wait for it to
    /// settle (`sched::hold_for`). A node of a chain, or one that never ran,
    /// is listed in `Engine::movers`.
    pub(crate) moves: AtomicBool,
    /// It is the front of a node an edit removed (`sched::mark_front`):
    /// the first after it to run again, those after it waiting for it
    /// in `Engine::movers` until it commits (`sched::clear_front`).
    pub(crate) front: AtomicBool,
    /// It was stopped before a run to wait for a node (`sched::work`): the
    /// wake that asked for the run still stands; its waiting ended by
    /// that node's settling clears this (`sched::release_waiting`), a
    /// quiet point's does not.
    pub(crate) held: AtomicBool,
    /// How long its longest run that committed took, ns (`sched::work`).
    pub(crate) full_ns: AtomicU64,
    /// Its order among the nodes placed after the same anchor in slot 1
    /// by different placers (`Ctx::rank_placement`): one of a lower rank
    /// stands before one of a higher; 0 none (placed right after the
    /// anchor's children, before those already there).
    pub(crate) rank: AtomicU64,
    /// Its operation's static effect (`Operation::effect`), packed
    /// (`pack_effect`): asked once when the node is made or its text
    /// replaced.
    pub(crate) effect: AtomicU32,
    /// It has brackets filed in its segment.
    pub(crate) has_brackets: AtomicBool,
    /// The scope epoch its last change of brackets made
    /// (`Engine::scope_gen`): the walk that re-asks the readers after
    /// that change trusts where depths agree again only if no other
    /// change was filed since (`commit::reask_after_effect_at`).
    pub(crate) bracket_gen: AtomicU64,
    /// The nodes whose reads are anchored at it (`At::Before`,
    /// `At::AfterOwn`), and the witnesses filed at it
    /// ([`crate::guard::Foreign`]): a walk past it asks their records, its
    /// removal or move asks them again. A node each, not a field each: a
    /// picture's run reads 144 000 fields at its parent.
    pub(crate) foreign: Mx<crate::guard::Foreign>,
    /// `foreign` was ever filed into: a walk past a node never read at by
    /// another takes no lock of it for that.
    pub(crate) has_foreign: AtomicBool,
    /// Relocated since its last commit (`commit::relocate`): its next
    /// commit checks its placements where it stands now, though its run
    /// published nothing new (`Ctx::unchanged`).
    pub(crate) moved: AtomicBool,
    /// Its claim takes every node after it in its unit, not its siblings
    /// alone (`Ctx::claim_rest`: the job is over at it); cleared by an
    /// ordinary claim.
    pub(crate) claim_all: AtomicBool,
}

/// An effect as 32 bits: closes, opens, `before`, and whether pending.
pub(crate) fn pack_effect(e: Effect) -> u32 {
    let b = e.brackets();
    u32::from(b.closes)
        | (u32::from(b.opens) << 8)
        | (u32::from(b.before) << 16)
        | (u32::from(e == Effect::Pending) << 17)
}

/// The brackets of a packed effect, and whether it is pending.
pub(crate) fn unpack_effect(x: u32) -> (Brackets, bool) {
    let b = Brackets {
        closes: (x & 0xff) as u8,
        opens: ((x >> 8) & 0xff) as u8,
        before: x & (1 << 16) != 0,
    };
    (b, x & (1 << 17) != 0)
}

impl Node {
    pub(crate) fn new(
        id: NodeId,
        unit: UnitId,
        op: OpId,
        args: Vec<ValueId>,
        parent: Option<NodeId>,
        seg: SegId,
        pos: Pos,
    ) -> Node {
        Node {
            final_reruns: AtomicU32::new(0),
            final_epoch: AtomicU64::new(0),
            awaited: AtomicBool::new(false),
            claimer: AtomicU64::new(0),
            claim_end: AtomicU64::new(0),
            claim_mx: Mx::new(false),
            held_frontier: AtomicBool::new(false),
            ahead: std::sync::atomic::AtomicU64::new(0),
            held_window: AtomicBool::new(false),
            window_pass: AtomicBool::new(false),
            frontier_pass: AtomicBool::new(false),
            unwalked: Mx::new(Vec::new()),
            layer_refuted: AtomicU32::new(0),
            back_frontier: Mx::new(None),
            asks: Mx::new(Vec::new()),
            restated: AtomicU32::new(0),
            follows: AtomicBool::new(false),
            anchor: AtomicU64::new(0),
            state_stale: AtomicBool::new(false),
            settle_now: AtomicBool::new(false),
            held_panic: AtomicBool::new(false),
            slot: AtomicU8::new(0),
            rank: AtomicU64::new(0),
            full_ns: AtomicU64::new(0),
            refuted: AtomicBool::new(false),
            moves: AtomicBool::new(false),
            front: AtomicBool::new(false),
            held: AtomicBool::new(false),
            effect: AtomicU32::new(0),
            has_brackets: AtomicBool::new(false),
            bracket_gen: AtomicU64::new(0),
            foreign: Mx::new(crate::guard::Foreign::default()),
            has_foreign: AtomicBool::new(false),
            moved: AtomicBool::new(false),
            claim_all: AtomicBool::new(false),
            id,
            unit,
            op: AtomicU32::new(op.0),
            text: Lock::new(args.into()),
            parent,
            seg: AtomicU32::new(seg.0),
            pos: AtomicU64::new(pos.0),
            state: AtomicU8::new(State::Dirty as u8),
            committing: AtomicBool::new(false),
            late_stops: AtomicBool::new(false),
            runs: AtomicU32::new(0),
            run_epoch: AtomicU64::new(0),
            reruns: AtomicU32::new(0),
            parked_on: Mx::new(None),
            serial: AtomicU32::new(0),
            last: Mx::new(None),
            woke: Mx::new(None),
            stale_wake: Mx::new(None),
            filed_floor: AtomicU32::new(0),
            asked: AtomicBool::new(false),
            woke_merged: AtomicBool::new(false),
            stale: AtomicBool::new(false),
            stopped: Mx::new(Vec::new()),
            waiters: Mx::new(Vec::new()),
            bounded: Mx::new(Vec::new()),
            run_ns: AtomicU64::new(0),
            commit_ns: AtomicU64::new(0),
            reask_ns: AtomicU64::new(0),
            done_ns: AtomicU64::new(0),
        }
    }

    pub(crate) fn op(&self) -> OpId {
        OpId(self.op.load(Ordering::Acquire))
    }

    pub(crate) fn args(&self) -> Arc<[ValueId]> {
        self.text.read().clone()
    }

    pub(crate) fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            0 => State::Clean,
            1 => State::Dirty,
            2 => State::Running,
            3 => State::RunningDirty,
            4 => State::Inert,
            6 => State::RunningInert,
            _ => State::Gone,
        }
    }

    pub(crate) fn cas_state(&self, from: State, to: State) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    /// The node whose claim holds it.
    pub(crate) fn claimer(&self) -> Option<NodeId> {
        self.claimer
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(NodeId::from_raw)
    }

    /// Whether a claim of a node before `me` (at `at`) holds it: one
    /// `me` cannot take. A claimer after `me` holds it inside its own
    /// span, which `me` takes with the claimer.
    pub(crate) fn held_before(&self, eng: &crate::engine::Engine, me: NodeId, at: crate::order::Pos) -> bool {
        self.claimer().is_some_and(|c| {
            let cn = eng.node(c);
            c != me && cn.state() != State::Gone && cn.pos_hint() < at
        })
    }

    /// Taken by a claim (`Inert`, or `RunningInert` while a run is in
    /// progress).
    pub(crate) fn taken(&self) -> bool {
        matches!(self.state(), State::Inert | State::RunningInert)
    }

    /// Takes it for a claim: `Inert`, or `RunningInert` while a run of it is
    /// in progress (that run's worker holds its count of the outstanding
    /// nodes). False if it was taken or gone already.
    pub(crate) fn take(&self) -> bool {
        loop {
            let (from, to) = match self.state() {
                s @ (State::Clean | State::Dirty) => (s, State::Inert),
                s @ (State::Running | State::RunningDirty) => (s, State::RunningInert),
                State::Inert | State::RunningInert | State::Gone => return false,
            };
            if self.cas_state(from, to) {
                return true;
            }
        }
    }

    /// Gives it back from a claim: true when it is `Dirty` now and the
    /// caller queues it; a run still in progress is made `RunningDirty`
    /// instead, for its worker to queue once it ends.
    pub(crate) fn release(&self) -> bool {
        loop {
            match self.state() {
                State::Inert => {
                    if self.cas_state(State::Inert, State::Dirty) {
                        return true;
                    }
                }
                State::RunningInert => {
                    if self.cas_state(State::RunningInert, State::RunningDirty) {
                        return false;
                    }
                }
                State::Clean | State::Dirty | State::Running | State::RunningDirty | State::Gone => return false,
            }
        }
    }

    pub(crate) fn seg_hint(&self) -> SegId {
        SegId(self.seg.load(Ordering::Acquire))
    }

    /// The node it was placed right after (none: pushed by the client).
    pub(crate) fn anchor(&self) -> Option<NodeId> {
        match self.anchor.load(Ordering::Acquire) {
            0 => None,
            a => Some(NodeId::from_raw(a - 1)),
        }
    }

    pub(crate) fn slot(&self) -> u8 {
        self.slot.load(Ordering::Acquire)
    }

    pub(crate) fn rank(&self) -> u64 {
        self.rank.load(Ordering::Acquire)
    }

    pub(crate) fn pos_hint(&self) -> Pos {
        Pos(self.pos.load(Ordering::Acquire))
    }

    /// Whether a run of it has committed.
    pub(crate) fn committed(&self) -> bool {
        self.serial.load(Ordering::Acquire) > 0
    }

    /// Never committed and not taken or removed: its contributions are
    /// still to come.
    /// To run, again or for the first time (a hole, or a node refuted
    /// since its last commit), or running short of its commit's filing
    /// (`committing`, set under the segment's lock the commit takes the
    /// `next` readers stopped at it under): what it contributes is not
    /// known.
    pub(crate) fn is_open(&self) -> bool {
        match self.state() {
            State::Dirty => true,
            State::Running | State::RunningDirty => self.is_hole() || !self.committing.load(Ordering::Acquire),
            _ => false,
        }
    }

    pub(crate) fn is_hole(&self) -> bool {
        !self.committed() && matches!(self.state(), State::Dirty | State::Running | State::RunningDirty)
    }

    pub(crate) fn record(&self) -> Option<Arc<RunRecord>> {
        self.last.lock().clone()
    }

    /// Its last run's record, not a prediction.
    pub(crate) fn run_record(&self) -> Option<Arc<RunRecord>> {
        self.record().filter(|r| !r.provisional)
    }
}

/// A check of a record's reads ([`RunRecord::read_holds`]): the folds
/// it made, and the reads it found holding.
#[derive(Default)]
pub(crate) struct HoldMemo {
    pub(crate) fold: crate::read::FoldMemo,
    held: crate::FxHashMap<((ReadKind, PathId), Option<ValueId>), ()>,
}

/// What a run produced, as the commit took it.
pub(crate) struct RunRecord {
    pub(crate) serial: u32,
    pub(crate) output: ValueId,
    pub(crate) defs: Vec<DefRec>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) reads: crate::reads::Reads,
    /// The nodes whose definitions its reads got, each once (those it
    /// kept from its last run's with them): what its next run waits for
    /// (`read::open_definer_before`). A hint.
    pub(crate) sources: Box<[NodeId]>,
    /// Its placements in order (`Ctx::child`, `Ctx::follow`), matched by
    /// (op, args) at the next commit so unchanged ones are kept.
    pub(crate) placed: Vec<NodeId>,
    pub(crate) claim: usize,
    /// Its claim was of the rest (`Ctx::claim_rest`, `Node::claim_all`):
    /// kept with the count, which means every text node after it then,
    /// not that many siblings.
    pub(crate) claim_all: bool,
    pub(crate) effect: Effect,
    /// Filed before any run: predictions, replaced by the first commit.
    pub(crate) provisional: bool,
    /// What the run stored for its next run (`Ctx::park`): a long loop's
    /// state at the point it stopped, read back by `Ctx::resumed`. Not a
    /// read: no record, no re-ask. A root at a collection.
    pub(crate) state: Option<ValueId>,
    /// The placements (indices into `placed`) whose next commit re-asks
    /// the node, whatever their output (`Ctx::placed_output_wait`). Not
    /// reads: nothing checks them, no run keeps them.
    pub(crate) waits: Box<[usize]>,
    /// The engine's scope generation before its first read (a run that
    /// kept reads of the one before, that one's when older): while it
    /// stands, a read of a field whose change count is the one it read
    /// at gives what it gave ([`RunRecord::read_holds`]).
    pub(crate) scope_gen: u64,
    /// The same for the relocations counted (`Engine::moves_gen`).
    pub(crate) moves_gen: u64,
}

impl RunRecord {
    /// Whether its read `rd` still gives what it gave: unchecked where
    /// nothing of its field changed since it read (a read at a place
    /// that stands, its field's change count as it was, no scope moved),
    /// as a commit's check takes its reads (`commit::commit`'s
    /// refutation); else asked (`read::holds`). A picture's child holds up to 144 000 reads:
    /// asked one by one, a check that found them all standing cost
    /// 70-140 ms (Grail, a deleted paragraph: 0.46 s of it).
    pub(crate) fn read_holds(
        &self,
        eng: &crate::Engine,
        me: &Node,
        rd: &ReadRec,
        scope_moved: bool,
        memo: &mut HoldMemo,
    ) -> bool {
        // (a read made again, as one before it found holding: a run asks
        // one thing many times, a `TikZ` picture microtype's font list
        // 20 592 times by 76 paths, each asked again at every check)
        let key = (rd.key(), rd.seen());
        if memo.held.contains_key(&key) {
            return true;
        }
        let ok = self.read_holds_once(eng, me, rd, scope_moved, &mut memo.fold);
        if ok {
            memo.held.insert(key, ());
        }
        ok
    }

    /// [`RunRecord::read_holds`] for one read.
    fn read_holds_once(
        &self,
        eng: &crate::Engine,
        me: &Node,
        rd: &ReadRec,
        scope_moved: bool,
        memo: &mut crate::read::FoldMemo,
    ) -> bool {
        // (a read at a place: at the reader, or before or after a node
        // still there and not moved, whose answer only a change of its
        // field's definitions moves; not a final read, which reads what
        // was published at a quiet point, nor a fold or a search, whose
        // bounds may go with no change of the field: a read before a
        // node removed since answers none)
        let at_place = |n: NodeId| {
            let a = eng.node(n);
            a.state() != State::Gone && !a.moved.load(Ordering::Acquire)
        };
        let placed = match rd.kind() {
            ReadKind::Field { at: Anchor::Me, .. } => at_place(me.id),
            ReadKind::Field {
                at: Anchor::Before(n) | Anchor::AfterOwn(n),
                ..
            } => at_place(me.id) && at_place(*n),
            ReadKind::Env { .. } => true,
            _ => false,
        };
        if placed
            && !scope_moved
            && rd.epoch != u64::MAX
            && rd.kind().field().is_some_and(|f| eng.field_epoch(f) == rd.epoch)
        {
            return true;
        }
        crate::read::holds(eng, me, rd, memo)
    }

    /// Whether the scope generation moved since its reads were made, or a
    /// node was relocated ([`RunRecord::read_holds`]'s `scope_moved`).
    pub(crate) fn scope_moved(&self, eng: &crate::Engine) -> bool {
        (crate::read::scoped(eng) && eng.scope_gen.load(Ordering::Acquire) != self.scope_gen)
            || eng.moves_gen.load(Ordering::Acquire) != self.moves_gen
    }

    pub(crate) fn provisional(defs: Vec<DefRec>) -> RunRecord {
        RunRecord {
            serial: 0,
            output: ValueId::UNIT,
            defs,
            contribs: Vec::new(),
            reads: crate::reads::Reads::default(),
            sources: Box::default(),
            placed: Vec::new(),
            claim: 0,
            claim_all: false,
            state: None,
            effect: Effect::None,
            provisional: true,
            waits: Box::default(),
            scope_gen: 0,
            moves_gen: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DefRec {
    pub(crate) f: Field,
    pub(crate) loc: Locality,
    /// For a relation (`Ctx::define_rel`): the value computed from the
    /// reaching input at commit time; `rel` is the relation.
    pub(crate) value: ValueId,
    pub(crate) rel: Option<Rel>,
}

/// Where a read looks, relative to the reader.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Anchor {
    Me,
    Before(NodeId),
    AfterOwn(NodeId),
    End(UnitId),
    /// The last definition on the line (`At::Final`).
    Final,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ReadKind {
    Field {
        f: Field,
        at: Anchor,
    },
    /// The environment's value, whatever is defined before.
    Env {
        f: Field,
    },
    /// The fold over contributions to `f` after `from` (None: the unit's
    /// start) and before `to` (None: the reader), or up to and with `to`'s
    /// own (`incl`).
    Fold {
        f: Field,
        from: Option<NodeId>,
        to: Option<NodeId>,
        incl: bool,
    },
    /// The first contribution to `f` after `after` (None: the unit's start).
    Next {
        f: Field,
        after: Option<NodeId>,
    },
    /// The output of the reader's `i`-th placement; with `guess`, one
    /// that has not run yet answers what the reader saw (its guess,
    /// `Ctx::placed_output_or`).
    Placed {
        i: usize,
        guess: bool,
    },
    /// The `i`-th claimed sibling's (op, args).
    Following {
        i: usize,
    },
    /// The first node before the reader on the line that has not
    /// committed (`Ctx::hole_before`).
    Hole,
}

/// One read as its run makes it: validated by recomputing its answer and
/// comparing to `seen`. Its kind inline: no read is interned (a table of
/// kinds cost para4000 2% of its instructions at -j1). A run's reads are
/// a vector of these while it runs; its commit codes them into its
/// record's [`crate::reads::Reads`], about 11 bytes a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReadRec {
    kind: ReadKind,
    path: PathId,
    /// Its reader stays filed across commits that went on from its run
    /// ([`crate::commit::FILED`]).
    filed: bool,
    /// Its field's change count before it read ([`crate::Engine`]'s
    /// `field_epoch`; `u64::MAX` for a read of none).
    pub(crate) epoch: u64,
    /// What it saw, [`NO_VALUE`] for none.
    seen: u64,
}

/// A read's `seen` when it saw no value: no id has this tag (7).
pub(crate) const NO_VALUE: u64 = u64::MAX;

impl ReadRec {
    pub(crate) fn new(kind: ReadKind, path: PathId, seen: Option<ValueId>, epoch: u64) -> ReadRec {
        ReadRec {
            kind,
            path,
            filed: false,
            epoch,
            seen: seen.map_or(NO_VALUE, ValueId::raw),
        }
    }

    /// From its parts as a record codes them (`seen` raw).
    pub(crate) fn from_parts(kind: ReadKind, path: PathId, filed: bool, epoch: u64, seen: u64) -> ReadRec {
        ReadRec {
            kind,
            path,
            filed,
            epoch,
            seen,
        }
    }

    /// Its kind and path: equal keys, equal reads but for what they saw.
    pub(crate) fn key(&self) -> (ReadKind, PathId) {
        (self.kind, self.path)
    }

    pub(crate) fn kind(&self) -> &ReadKind {
        &self.kind
    }

    pub(crate) fn kind_path(&self) -> (&ReadKind, PathId) {
        (&self.kind, self.path)
    }

    pub(crate) fn path(&self) -> PathId {
        self.path
    }

    pub(crate) fn seen(&self) -> Option<ValueId> {
        (self.seen != NO_VALUE).then(|| ValueId::from_raw(self.seen))
    }

    /// `seen` as its raw word ([`NO_VALUE`] for none).
    pub(crate) fn seen_raw(&self) -> u64 {
        self.seen
    }

    pub(crate) fn filed(&self) -> bool {
        self.filed
    }

    pub(crate) fn set_filed(&mut self, on: bool) {
        self.filed = on;
    }
}

impl ReadKind {
    /// The field it reads, if one.
    pub(crate) fn field(&self) -> Option<Field> {
        match self {
            ReadKind::Field { f, .. } | ReadKind::Env { f } | ReadKind::Fold { f, .. } | ReadKind::Next { f, .. } => {
                Some(*f)
            }
            ReadKind::Placed { .. } | ReadKind::Following { .. } | ReadKind::Hole => None,
        }
    }
}

pub(crate) struct Wake {
    pub(crate) by: Option<NodeId>,
    pub(crate) cause: Cause,
    pub(crate) at_ns: u64,
}

/// A placement a run asks for.
pub(crate) enum Placement {
    /// After the placer, before its continuation and next sibling.
    Child(OpId, Vec<ValueId>),
    /// After the placer and its children.
    Follow(OpId, Vec<ValueId>),
    /// After node `at` (and its placements).
    FollowAfter(NodeId, OpId, Vec<ValueId>),
    /// After the node the placer's `j`-th placement of this run stands
    /// for (and its placements), `j` before this one.
    FollowAfterPlaced(usize, OpId, Vec<ValueId>),
}

impl Placement {
    pub(crate) fn key(&self) -> (OpId, &[ValueId]) {
        match self {
            Placement::Child(op, a)
            | Placement::Follow(op, a)
            | Placement::FollowAfter(_, op, a)
            | Placement::FollowAfterPlaced(_, op, a) => (*op, a),
        }
    }
}

/// A run's product, handed to the commit.
pub(crate) struct Product {
    pub(crate) output: ValueId,
    pub(crate) defs: Vec<DefRec>,
    pub(crate) contribs: Vec<(Field, ValueId)>,
    pub(crate) reads: Vec<ReadRec>,
    /// The nodes whose definitions the reads got (`RunRecord::sources`).
    pub(crate) sources: Vec<NodeId>,
    pub(crate) placements: Vec<Placement>,
    /// The placements' ranks (`Ctx::rank_placement`), by index; one past
    /// the end is 0.
    pub(crate) ranks: Vec<u64>,
    pub(crate) claim: usize,
    /// The claim is of the rest (`Ctx::claim_rest`).
    pub(crate) claim_all: bool,
    /// What the run set (`Ctx::set_effect`); None: its operation's.
    pub(crate) effect: Option<Effect>,
    /// The brackets placements of an operation whose effect waits for
    /// its run (`Effect::Pending`) are filed with until they run
    /// (`Ctx::predict_placed_effect`), by placement index.
    pub(crate) place_effects: Vec<(usize, Effect)>,
    /// The scope epoch when the run began (`Engine::scope_gen`): a
    /// bracket changed since may have moved any of its reads.
    pub(crate) scope_gen: u64,
    /// The relocations counted when it began (`Engine::moves_gen`).
    pub(crate) moves_gen: u64,
    /// The run published nothing new (`Ctx::unchanged`): the last
    /// record's definitions, contributions, placements and output stand,
    /// with this run's reads.
    pub(crate) unchanged: bool,
    /// With `unchanged`, the run's own claim stands, not the last
    /// record's (`Ctx::unchanged_unclaimed`).
    pub(crate) own_claim: bool,
    /// The run stopped at a hole with nothing to say (`Ctx::keep_last_at_hole`):
    /// its last run's products stand, but not that run's state, whose
    /// checkpoints count reads of that run, not of this one's record.
    pub(crate) at_hole: bool,
    /// `Ctx::park`: the state the next run resumes from.
    pub(crate) state: Option<ValueId>,
    /// `Ctx::parked_on`: the field whose answer the run stopped for.
    pub(crate) parked_on: Option<Field>,
    /// The run took its last run's state (`Ctx::resumed`): that run's
    /// reads stand with this one's.
    pub(crate) resumed: std::cell::Cell<bool>,
    /// The site the run was woken by (`Ctx::woken_by`: a range's start,
    /// a changed definition's node): a kept read from it that still moved
    /// at the commit is one the run did not read again where it was told
    /// to go back before it.
    pub(crate) woken_at: Option<NodeId>,
    /// The run was woken by the answer its last run parked on
    /// (`Ctx::parked_on`), and by nothing else (`Node::woke_merged`), so
    /// the kept reads it does not check stood until it ran.
    pub(crate) on_park: bool,
    /// `Ctx::settle_now`: its definitions reach readers outside the
    /// settling layer at once.
    pub(crate) settle_now: bool,
    /// `Ctx::placed_output_wait`: the placements whose next commit
    /// re-asks the run, whatever their output.
    pub(crate) waits: Vec<usize>,
    /// `Ctx::set_frontier`: the settling layer's frontier this run moves
    /// (`Some(None)`: none, every reader free).
    pub(crate) frontier: Option<Option<NodeId>>,
    /// `Ctx::last_run_stands`: the last run's record, every read of which
    /// still holds; nothing ran ([`crate::commit::commit_stood`]).
    pub(crate) stood: Option<Arc<RunRecord>>,
}
