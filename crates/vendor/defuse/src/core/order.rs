//! A unit's order: entries in a doubly linked list with order labels, the
//! frame (scope) structure derived from their brackets, sibling views, and
//! the structural writes (children, continuations, claims, inertness).
//! Structural writers hold the unit's `s` mutex; readers use the atomics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::{
    Br, ClaimS, Data, DefE, DefVal, F_APP, F_DONE, F_HAS_OUT, F_IN_PCLAIM, F_IN_PEFF, F_IN_WILD, F_NEEDS_RUN, F_PCLAIM,
    F_PEFF, F_PSET, F_QUEUED, F_SCLAIM, F_WILD, Frame, K_NONE, K_PENDING, K_VALUE, Kind, MayDefineS, NIL, Rec, Role,
    S_CLEAN, S_DIRTY, S_FREE, S_INERT, S_MARKER, S_NEW, S_PARK, S_RUN, S_STEP, Shared, Slot, Static, StaticRare, Unit,
    lock,
};
use crate::api::{Cause, Effect, Locality, OpId};
use crate::values::{Field, ValueId};

const GAP: u64 = 1 << 32;
const MIN_GAP: u64 = 1 << 8;
pub(crate) const NO_FRAME: u32 = u32::MAX;

/// How a frame's kind is set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum KindSpec {
    None,
    Value(ValueId),
    Pending,
}

/// A child or continuation a run made, or a scope marker.
#[derive(Clone, Debug)]
pub(crate) enum Spec {
    Node { id: u64, op: OpId, args: Arc<[ValueId]> },
    Marker { id: u64, open: bool },
}

impl Spec {
    pub(crate) fn id(&self) -> u64 {
        match self {
            Spec::Node { id, .. } | Spec::Marker { id, .. } => *id,
        }
    }
}

/// Notifications a structural write produces, dispatched after it.
#[derive(Default)]
pub(crate) struct Notes {
    pub touch: Vec<(Slot, u32, Cause)>,
    /// Static claimers whose window changed.
    pub reclaim: Vec<Slot>,
    /// Entries from which frames must be recomputed.
    pub frames_from: Vec<Slot>,
    /// Nodes to enqueue (new or released).
    pub enqueue: Vec<Slot>,
    /// Static facts of texts about to be created, by node id, found
    /// before the structural lock (`register` adds the claim).
    pub pre: crate::values::FxHashMap<u64, Static>,
}

impl Shared {
    // ------------------------------------------------------------ records

    /// A new unit.
    pub(crate) fn new_unit_rec(&self, app: Slot) -> u32 {
        let fi = self.frames.push_with(1, |_| Frame {
            unit: AtomicU32::new(0),
            opener: AtomicU32::new(NIL),
            parent: AtomicU32::new(NO_FRAME),
            depth: AtomicU32::new(0),
            kmode: AtomicU8::new(K_NONE),
            kind: AtomicU64::new(0),
        });
        let u = self.units.push_with(1, |_| Unit {
            first: AtomicU32::new(NIL),
            last: AtomicU32::new(NIL),
            #[allow(clippy::cast_possible_truncation)]
            root_frame: fi as u32,
            app: AtomicU32::new(app),
            live: AtomicBool::new(true),
            s: std::sync::RwLock::new(()),
            sv: AtomicU64::new(0),
            wflag: AtomicBool::new(false),
            stripes: std::sync::OnceLock::new(),
            label_epoch: AtomicU64::new(0),
            frame_epoch: AtomicU64::new(0),
            env: crate::sync::BrLock::new(crate::values::FxHashMap::default()),
            wild: crate::sync::BrLock::new(Vec::new()),
            pfx: crate::sync::BrLock::new(crate::values::FxHashMap::default()),
            pfx_lens: crate::sync::BrLock::new(Vec::new()),
            peff: crate::sync::BrLock::new(Vec::new()),
            pclaim: crate::sync::BrLock::new(Vec::new()),
            pclaim_lo: std::sync::atomic::AtomicUsize::new(0),
            lists: std::sync::Mutex::new(0),
        });
        #[allow(clippy::cast_possible_truncation)]
        let u = u as u32;
        self.frame(self.unit(u).root_frame).unit.store(u, Ordering::Release);
        u
    }

    /// Allocates an entry (not yet linked).
    pub(crate) fn alloc(&self, unit: u32, id: u64, data: Data, depth: u32, parent: Slot) -> Slot {
        let marker = matches!(data.kind, Kind::Marker { .. });
        let mut data = Some(data);
        let i = self.recs.push_with(1, |_| Rec {
            id: AtomicU64::new(id),
            gn: AtomicU32::new(1),
            unit: AtomicU32::new(unit),
            label: AtomicU64::new(0),
            prev: AtomicU32::new(NIL),
            next: AtomicU32::new(NIL),
            depth: AtomicU32::new(depth),
            parent: AtomicU32::new(parent),
            frame: AtomicU32::new(NO_FRAME),
            top_in: AtomicU32::new(NO_FRAME),
            frames: AtomicU32::new(NO_FRAME),
            nframes: AtomicU32::new(0),
            status: AtomicU8::new(if marker { S_MARKER } else { S_NEW }),
            flags: AtomicU32::new(0),
            out: AtomicU64::new(0),
            lgen: AtomicU32::new(0),
            rdep: AtomicU64::new(0),
            rtime: AtomicU64::new(0),
            park_seq: AtomicU32::new(0),
            claimed_by: AtomicU32::new(NIL),
            d: std::sync::Mutex::new(Box::new(data.take().expect("one record"))),
        });
        #[allow(clippy::cast_possible_truncation)]
        let s = i as Slot;
        lock(self.id_shard(id)).insert(id, s);
        if !marker {
            self.live.fetch_add(1, Ordering::Relaxed);
            self.c.created.fetch_add(1, Ordering::Relaxed);
            crate::obs::count!(Create);
        }
        let op = {
            let mut d = self.rec(s).data();
            d.born = self.epoch.load(Ordering::Relaxed);
            d.op
        };
        lock(&self.hdirty).push(s);
        if !marker && super::rlock(&self.marked_ops).contains(&op) {
            self.rec(s).set(super::F_MARKED);
            lock(&self.marked).push(s);
            lock(&self.to_report).push(s);
        }
        s
    }

    pub(crate) fn is_marker(&self, s: Slot) -> bool {
        self.rec(s).st() == S_MARKER
    }

    pub(crate) fn next(&self, s: Slot) -> Slot {
        self.rec(s).next.load(Ordering::Acquire)
    }

    pub(crate) fn prev(&self, s: Slot) -> Slot {
        self.rec(s).prev.load(Ordering::Acquire)
    }

    pub(crate) fn depth(&self, s: Slot) -> u32 {
        self.rec(s).depth.load(Ordering::Acquire)
    }

    pub(crate) fn unit_of(&self, s: Slot) -> u32 {
        self.rec(s).unit.load(Ordering::Acquire)
    }

    /// Whether `a` comes before `b` (same unit).
    pub(crate) fn before(&self, a: Slot, b: Slot) -> bool {
        self.rec(a).lab() < self.rec(b).lab()
    }

    // ------------------------------------------------------------- labels

    /// Links `news` (in order) after `after` (`NIL`: at the unit's front).
    pub(crate) fn link_after(&self, u: u32, after: Slot, news: &[Slot]) {
        if news.is_empty() {
            return;
        }
        let un = self.unit(u);
        let k = news.len() as u64;
        let (mut lo, mut hi) = self.gap(u, after);
        if hi - lo <= k + 1 || (hi - lo) / (k + 1) < 2 {
            self.relabel(u, after, k);
            (lo, hi) = self.gap(u, after);
        }
        let step = ((hi - lo) / (k + 1)).max(1);
        let nxt = if after == NIL {
            un.first.load(Ordering::Acquire)
        } else {
            self.next(after)
        };
        let mut prev = after;
        for (i, &n) in news.iter().enumerate() {
            let r = self.rec(n);
            r.label.store(lo + step * (i as u64 + 1), Ordering::Release);
            r.prev.store(prev, Ordering::Release);
            prev = n;
        }
        for w in news.windows(2) {
            self.rec(w[0]).next.store(w[1], Ordering::Release);
        }
        let lastn = news[news.len() - 1];
        self.rec(lastn).next.store(nxt, Ordering::Release);
        if nxt == NIL {
            un.last.store(lastn, Ordering::Release);
        } else {
            self.rec(nxt).prev.store(lastn, Ordering::Release);
        }
        if after == NIL {
            un.first.store(news[0], Ordering::Release);
        } else {
            self.rec(after).next.store(news[0], Ordering::Release);
        }
    }

    fn gap(&self, u: u32, after: Slot) -> (u64, u64) {
        let un = self.unit(u);
        let lo = if after == NIL { 0 } else { self.rec(after).lab() };
        let nxt = if after == NIL {
            un.first.load(Ordering::Acquire)
        } else {
            self.next(after)
        };
        let hi = if nxt == NIL {
            lo.saturating_add(GAP.saturating_mul(64)).min(u64::MAX - 1).max(lo + 1)
        } else {
            self.rec(nxt).lab()
        };
        (lo, hi)
    }

    /// Spreads labels around `after` so that `k` entries fit after it.
    fn relabel(&self, u: u32, after: Slot, k: u64) {
        self.escalate(u);
        let un = self.unit(u);
        un.label_epoch.fetch_add(1, Ordering::AcqRel);
        // The window: entries [l, r] around `after`.
        let mut l = after;
        let mut r = if after == NIL {
            un.first.load(Ordering::Acquire)
        } else {
            self.next(after)
        };
        let mut count: u64 = u64::from(l != NIL) + u64::from(r != NIL);
        let mut width = 1u64;
        loop {
            let lo = if l == NIL { 0 } else { self.rec(l).lab() };
            let hi = if r == NIL { u64::MAX - 1 } else { self.rec(r).lab() };
            let need = (count + k + 2).saturating_mul(MIN_GAP);
            if hi.saturating_sub(lo) >= need {
                break;
            }
            let lp = if l == NIL { NIL } else { self.prev(l) };
            let rn = if r == NIL { NIL } else { self.next(r) };
            if l == NIL && r == NIL {
                break;
            }
            for _ in 0..width {
                if l != NIL {
                    let p = self.prev(l);
                    l = p;
                    if l == NIL {
                        break;
                    }
                    count += 1;
                }
            }
            for _ in 0..width {
                if r != NIL {
                    let n = self.next(r);
                    r = n;
                    if r == NIL {
                        break;
                    }
                    count += 1;
                }
            }
            let _ = (lp, rn);
            width *= 2;
        }
        // Collect the window's entries and the slot index of `after`.
        let lo = if l == NIL { 0 } else { self.rec(l).lab() };
        let hi = if r == NIL { u64::MAX - 1 } else { self.rec(r).lab() };
        let mut xs = Vec::new();
        let mut x = if l == NIL {
            un.first.load(Ordering::Acquire)
        } else {
            self.next(l)
        };
        while x != NIL && x != r {
            xs.push(x);
            x = self.next(x);
        }
        let total = xs.len() as u64 + k + 1;
        let step = (hi - lo) / (total + 1);
        let mut i = 1u64;
        let mut placed_gap = after == NIL || after == l;
        if placed_gap {
            i += k;
        }
        for &x in &xs {
            self.rec(x).label.store(lo + step * i, Ordering::Release);
            i += 1;
            if !placed_gap && x == after {
                i += k;
                placed_gap = true;
            }
        }
        un.label_epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Unlinks `s` from its unit's list.
    pub(crate) fn unlink(&self, s: Slot) {
        self.escalate(self.unit_of(s));
        let u = self.unit_of(s);
        let un = self.unit(u);
        let r = self.rec(s);
        let p = r.prev.load(Ordering::Acquire);
        let n = r.next.load(Ordering::Acquire);
        if p == NIL {
            if un.first.load(Ordering::Acquire) == s {
                un.first.store(n, Ordering::Release);
            }
        } else {
            self.rec(p).next.store(n, Ordering::Release);
        }
        if n == NIL {
            if un.last.load(Ordering::Acquire) == s {
                un.last.store(p, Ordering::Release);
            }
        } else {
            self.rec(n).prev.store(p, Ordering::Release);
        }
        r.prev.store(NIL, Ordering::Release);
        r.next.store(NIL, Ordering::Release);
    }

    /// The last entry of `s`'s subtree (`s` if it has none).
    pub(crate) fn subtree_end(&self, s: Slot) -> Slot {
        let d = self.depth(s);
        let mut x = s;
        loop {
            let n = self.next(x);
            if n == NIL || self.depth(n) <= d {
                return x;
            }
            x = n;
        }
    }

    // ------------------------------------------------------------- frames

    pub(crate) fn frame_parent(&self, f: u32) -> u32 {
        self.frame(f).parent.load(Ordering::Acquire)
    }

    pub(crate) fn frame_of(&self, s: Slot) -> u32 {
        self.rec(s).frame.load(Ordering::Acquire)
    }

    pub(crate) fn top_in(&self, s: Slot) -> u32 {
        self.rec(s).top_in.load(Ordering::Acquire)
    }

    /// The label where frame `f` starts (0 for a root frame).
    pub(crate) fn frame_start(&self, f: u32) -> u64 {
        let o = self.frame(f).opener.load(Ordering::Acquire);
        if o == NIL { 0 } else { self.rec(o).lab() }
    }

    /// Whether the parent chain of `top` is `stack` (root first), without
    /// building it.
    fn chain_is(&self, top: u32, stack: &[u32]) -> bool {
        let mut f = top;
        for &e in stack.iter().rev() {
            if f != e {
                return false;
            }
            f = self.frame_parent(f);
        }
        f == NO_FRAME
    }

    fn chain(&self, top: u32) -> Vec<u32> {
        let mut v = Vec::new();
        let mut f = top;
        while f != NO_FRAME {
            v.push(f);
            f = self.frame_parent(f);
            if v.len() > 1 << 16 {
                break;
            }
        }
        v.reverse();
        v
    }

    /// The brackets of an entry as the structure counts them now, and the
    /// kind of the scopes it opens.
    pub(crate) fn brackets_of(&self, s: Slot) -> (Br, KindSpec) {
        let r = self.rec(s);
        let st = r.st();
        if st == S_FREE || st == S_INERT {
            return (Br::default(), KindSpec::None);
        }
        let d = r.data();
        if let Kind::Marker { open } = d.kind {
            return (
                if open {
                    Br { oa: 1, ..Br::default() }
                } else {
                    Br { ca: 1, ..Br::default() }
                },
                KindSpec::None,
            );
        }
        // A declared effect is a guess: the committed run's, when it set
        // one, holds.
        let eff = match (d.res.as_ref().and_then(|c| c.effect), &d.stat.effect) {
            (Some(e), _) => e,
            (None, Effect::Pending) => Effect::None,
            (None, e) => *e,
        };
        let b = eff.brackets();
        let kind = if b.kind_pending || (b.opens > 0 && (d.kind_val.is_some() || !d.kind_vals.is_empty())) {
            KindSpec::Pending
        } else {
            match b.kind {
                Some(k) => KindSpec::Value(k),
                None => KindSpec::None,
            }
        };
        (Br::of(&b), kind)
    }

    fn ensure_frames(&self, s: Slot, n: u32) -> u32 {
        let r = self.rec(s);
        if r.nframes.load(Ordering::Acquire) >= n {
            return r.frames.load(Ordering::Acquire);
        }
        let u = self.unit_of(s);
        let base = self.frames.push_with(n as usize, |_| Frame {
            unit: AtomicU32::new(u),
            opener: AtomicU32::new(s),
            parent: AtomicU32::new(NO_FRAME),
            depth: AtomicU32::new(0),
            kmode: AtomicU8::new(K_NONE),
            kind: AtomicU64::new(0),
        });
        #[allow(clippy::cast_possible_truncation)]
        let base = base as u32;
        r.frames.store(base, Ordering::Release);
        r.nframes.store(n, Ordering::Release);
        base
    }

    fn set_frame(&self, f: u32, parent: u32, depth: u32, kind: KindSpec, opener: Slot) -> bool {
        let fr = self.frame(f);
        let mut changed = fr.parent.swap(parent, Ordering::AcqRel) != parent;
        changed |= fr.depth.swap(depth, Ordering::AcqRel) != depth;
        let (m, k) = match kind {
            KindSpec::None => (K_NONE, 0),
            KindSpec::Value(v) => (K_VALUE, v.raw()),
            KindSpec::Pending => {
                let kv = self.opener_kind(opener, f);
                (K_PENDING, kv.map_or(0, ValueId::raw))
            }
        };
        changed |= fr.kmode.swap(m, Ordering::AcqRel) != m;
        changed |= fr.kind.swap(k, Ordering::AcqRel) != k;
        changed
    }

    /// The frame stack after entry `e`.
    fn stack_after(&self, e: Slot, root: u32) -> Vec<u32> {
        let t = self.top_in(e);
        let mut stack = if t == NO_FRAME { vec![root] } else { self.chain(t) };
        let (br, _) = self.brackets_of(e);
        let base = self.rec(e).frames.load(Ordering::Acquire);
        for _ in 0..br.cb {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        for k in 0..u32::from(br.ob) {
            stack.push(base + k);
        }
        for _ in 0..br.ca {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        for k in 0..u32::from(br.oa) {
            stack.push(base + u32::from(br.ob) + k);
        }
        stack
    }

    /// A new entry's frames as `recompute_frames` gives them, set before it
    /// is linked after `after` (its own scope frames are new: no reader
    /// sees them before the entry).
    fn preset_frames(&self, u: u32, after: Slot, x: Slot) {
        let root = self.unit(u).root_frame;
        let mut stack = self.stack_after(after, root);
        let r = self.rec(x);
        r.top_in.store(*stack.last().expect("root"), Ordering::Release);
        let (br, kind) = self.brackets_of(x);
        let n = u32::from(br.ob) + u32::from(br.oa);
        let base = if n > 0 { self.ensure_frames(x, n) } else { 0 };
        for _ in 0..br.cb {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        for k in 0..u32::from(br.ob) {
            #[allow(clippy::cast_possible_truncation)]
            let dep = stack.len() as u32;
            self.set_frame(base + k, *stack.last().expect("root"), dep, kind, x);
            stack.push(base + k);
        }
        r.frame.store(*stack.last().expect("root"), Ordering::Release);
        for _ in 0..br.ca {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        for k in 0..u32::from(br.oa) {
            let f = base + u32::from(br.ob) + k;
            #[allow(clippy::cast_possible_truncation)]
            let dep = stack.len() as u32;
            self.set_frame(f, *stack.last().expect("root"), dep, kind, x);
            stack.push(f);
        }
    }

    /// Whether frame `f` differs from what `set_frame` would make it.
    fn frame_differs(&self, f: u32, parent: u32, depth: u32, kind: KindSpec, opener: Slot) -> bool {
        let fr = self.frame(f);
        let (m, k) = match kind {
            KindSpec::None => (K_NONE, 0),
            KindSpec::Value(v) => (K_VALUE, v.raw()),
            KindSpec::Pending => (K_PENDING, self.opener_kind(opener, f).map_or(0, ValueId::raw)),
        };
        fr.parent.load(Ordering::Acquire) != parent
            || fr.depth.load(Ordering::Acquire) != depth
            || fr.kmode.load(Ordering::Acquire) != m
            || fr.kind.load(Ordering::Acquire) != k
    }

    /// Recomputes frames from `start` on until the structure agrees with
    /// what it was; returns whether any existing entry's scopes changed.
    pub(crate) fn recompute_frames(&self, u: u32, start: Slot, moved: &mut Vec<Slot>) -> Option<Slot> {
        let un = self.unit(u);
        let root = un.root_frame;
        let p = self.prev(start);
        let mut stack = if p == NIL {
            vec![root]
        } else {
            self.stack_after(p, root)
        };
        let mut x = start;
        let mut first = true;
        let mut changed = false;
        // A frame got a new parent or depth: entries under it must be
        // revisited even when their top frame is the same.
        let mut re: crate::values::FxHashSet<u32> = crate::values::FxHashSet::default();
        // How many frames on the stack are in `re`.
        let mut re_on = 0usize;
        let mut last = start;
        while x != NIL {
            let r = self.rec(x);
            let top = *stack.last().expect("root");
            let old_top = r.top_in.load(Ordering::Acquire);
            if !first && old_top == top && re_on == 0 && self.chain_is(top, &stack) {
                break;
            }
            last = x;
            first = false;
            let old_frame = r.frame.load(Ordering::Acquire);
            let (br, kind) = self.brackets_of(x);
            if old_top != top {
                self.escalate(u);
            }
            r.top_in.store(top, Ordering::Release);
            let n = u32::from(br.ob) + u32::from(br.oa);
            let base = if n > 0 { self.ensure_frames(x, n) } else { 0 };
            for _ in 0..br.cb {
                if stack.len() > 1
                    && let Some(f) = stack.pop()
                    && re.contains(&f)
                {
                    re_on -= 1;
                }
            }
            for k in 0..u32::from(br.ob) {
                let f = base + k;
                #[allow(clippy::cast_possible_truncation)]
                let dep = stack.len() as u32;
                if self.is_light(u) && self.frame_differs(f, *stack.last().expect("root"), dep, kind, x) {
                    self.escalate(u);
                }
                let ch = self.set_frame(f, *stack.last().expect("root"), dep, kind, x);
                if ch {
                    re.insert(f);
                }
                if re.contains(&f) {
                    re_on += 1;
                }
                changed |= ch && old_top != NO_FRAME;
                stack.push(f);
            }
            let fr = *stack.last().expect("root");
            if old_frame != fr {
                self.escalate(u);
            }
            r.frame.store(fr, Ordering::Release);
            for _ in 0..br.ca {
                if stack.len() > 1
                    && let Some(f) = stack.pop()
                    && re.contains(&f)
                {
                    re_on -= 1;
                }
            }
            for k in 0..u32::from(br.oa) {
                let f = base + u32::from(br.ob) + k;
                #[allow(clippy::cast_possible_truncation)]
                let dep = stack.len() as u32;
                if self.is_light(u) && self.frame_differs(f, *stack.last().expect("root"), dep, kind, x) {
                    self.escalate(u);
                }
                let ch = self.set_frame(f, *stack.last().expect("root"), dep, kind, x);
                if ch {
                    re.insert(f);
                }
                if re.contains(&f) {
                    re_on += 1;
                }
                changed |= ch && old_top != NO_FRAME;
                stack.push(f);
            }
            if old_top != NO_FRAME && (old_top != top || old_frame != fr) {
                changed = true;
            }
            if old_frame != fr {
                // (A new entry too: its static definitions were filed
                // before it had a frame.)
                moved.push(x);
            }
            x = r.next.load(Ordering::Acquire);
        }
        if changed {
            un.frame_epoch.fetch_add(1, Ordering::AcqRel);
            Some(last)
        } else {
            None
        }
    }

    // ----------------------------------------------------------- siblings

    /// The following siblings of `s` as `claimer` sees them: its parent's
    /// later children, then (crossing up) its ancestors' later siblings,
    /// skipping descendants and nodes claimed by others; ends at a scope
    /// marker. Calls `f(entry, skipped)` until it returns false.
    pub(crate) fn following(&self, s: Slot, claimer: Slot, f: impl FnMut(Slot, bool) -> bool) {
        self.following_from(s, claimer, f);
    }

    /// `following`, starting after `s` (the claimer itself or one of the
    /// siblings it sees).
    pub(crate) fn following_from(&self, s: Slot, claimer: Slot, mut f: impl FnMut(Slot, bool) -> bool) {
        let mut level = self.depth(s);
        let mut x = self.next(s);
        while x != NIL {
            let d = self.depth(x);
            if d > level {
                x = self.next(x);
                continue;
            }
            if self.is_marker(x) {
                return;
            }
            level = d;
            if self.own_cont(x, claimer) || self.placed_after_view(x, claimer) {
                x = self.next(x);
                continue;
            }
            // Taken by another claimer: skipped. What the claimer's own
            // follows took is its own output: still a sibling to it.
            let skipped = self.taken(x, claimer);
            if !f(x, skipped) {
                return;
            }
            x = self.next(x);
        }
    }

    /// Whether `x` was placed here ([`crate::Ctx::follow_after`]) by a
    /// node after `viewer` (or follows one so placed): it is that node's
    /// output, not a sibling `viewer` sees (else placing after `viewer`
    /// would change what `viewer` read).
    pub(crate) fn placed_after_view(&self, x: Slot, viewer: Slot) -> bool {
        let mut y = x;
        for _ in 0..64 {
            let (role, owner) = {
                let d = self.rec(y).data();
                (d.role, d.owner)
            };
            if role != Role::Cont || owner == NIL {
                return false;
            }
            // A continuation before its owner was placed by it.
            if self.before(y, owner) {
                return viewer != owner && self.before(viewer, owner);
            }
            y = owner;
        }
        false
    }

    /// The last entry of `x`'s extent: its subtree, then its own follows
    /// (and theirs), which come after the subtree at its level.
    pub(crate) fn extent_end(&self, x: Slot) -> Slot {
        let mut e = self.subtree_end(x);
        loop {
            let n = self.next(e);
            if n == NIL || !self.own_cont(n, x) {
                return e;
            }
            e = self.subtree_end(n);
        }
    }

    /// Whether `x` is a continuation `c` made (directly or through its
    /// continuations): not a sibling `c` sees.
    pub(crate) fn own_cont(&self, x: Slot, c: Slot) -> bool {
        let mut y = x;
        for _ in 0..1 << 16 {
            let (role, owner) = {
                let d = self.rec(y).data();
                (d.role, d.owner)
            };
            if role != Role::Cont || owner == NIL {
                return false;
            }
            if owner == c {
                return true;
            }
            y = owner;
        }
        false
    }

    /// Whether `x` is taken from `claimer`'s view: inert, or claimed by
    /// another claimer and still running under a doom mark (it turns inert
    /// at its own commit). What the claimer's own follows took is its own
    /// output: still a sibling to it.
    pub(crate) fn taken(&self, x: Slot, claimer: Slot) -> bool {
        let r = self.rec(x);
        let by = r.claimed_by.load(Ordering::Acquire);
        (r.st() == S_INERT || (by != NIL && r.has(super::F_DOOM_INERT)))
            && by != claimer
            && !(by != NIL && claimer != NIL && self.own_cont(by, claimer))
    }

    /// The previous siblings of `s` in its own list, nearest first (stops
    /// at the list's start or an opening marker).
    pub(crate) fn preceding(&self, s: Slot, mut f: impl FnMut(Slot, bool) -> bool) {
        let level = self.depth(s);
        let mut x = self.prev(s);
        while x != NIL {
            let d = self.depth(x);
            if d < level {
                return;
            }
            if d == level {
                if self.is_marker(x) {
                    return;
                }
                let skipped = self.taken(x, NIL);
                if !f(x, skipped) {
                    return;
                }
            }
            x = self.prev(x);
        }
    }

    /// An entry's text: operation and arguments.
    pub(crate) fn text(&self, s: Slot) -> (OpId, Arc<[ValueId]>) {
        let d = self.rec(s).data();
        (d.op, d.args.clone())
    }

    pub(crate) fn text_hash(&self, s: Slot) -> u64 {
        self.rec(s).data().thash
    }

    /// Subscribes `reader` (log generation `gn`) to structural changes at
    /// `s`.
    pub(crate) fn sib_subscribe(&self, s: Slot, reader: Slot, gn: u32) {
        let mut d = self.rec(s).data();
        if !d.sib_users.iter().any(|&(n, g)| n == reader && g == gn) {
            d.sib_users.push((reader, gn));
        }
    }

    /// Notifies the readers that saw `s` as a sibling.
    pub(crate) fn notify_sib(&self, s: Slot, notes: &mut Notes) {
        let users = std::mem::take(&mut self.rec(s).data().sib_users);
        for (n, g) in users {
            self.note_sib_user(n, g, notes);
        }
    }

    fn note_sib_user(&self, n: Slot, g: u32, notes: &mut Notes) {
        let r = self.rec(n);
        if g == 0 {
            if r.has(F_SCLAIM) && !notes.reclaim.contains(&n) {
                notes.reclaim.push(n);
            }
        } else if g == r.lgen.load(Ordering::Acquire) || g == u32::MAX {
            notes.touch.push((n, u32::MAX, Cause::Structure));
        }
    }

    /// Notifies the readers whose following view passes the gap before
    /// `s` (an entry inserted or removed there).
    pub(crate) fn notify_gap(&self, s: Slot, notes: &mut Notes) {
        let d = self.depth(s);
        let mut x = self.prev(s);
        // The flat predecessor and its ancestors down to `s`'s level.
        while x != NIL {
            self.notify_sib(x, notes);
            let dx = self.depth(x);
            if dx <= d {
                break;
            }
            // up to the parent
            let p = self.rec(x).parent.load(Ordering::Acquire);
            if p == NIL {
                break;
            }
            x = p;
        }
        let n = self.next(self.subtree_end(s));
        if n != NIL {
            self.notify_sib(n, notes);
        }
    }

    // -------------------------------------------------- static analysis

    /// Runs the operation's static hooks on `s`'s text.
    pub(crate) fn analyze(&self, s: Slot) -> Static {
        let (op, args) = self.text(s);
        let mut st = self.analyze_text(op, &args);
        self.analyze_claim(s, op, &args, &mut st);
        st
    }

    /// The claim hooks on `s`'s text (they look at its following siblings).
    fn analyze_claim(&self, s: Slot, op: OpId, args: &[ValueId], st: &mut Static) {
        let o = &self.ops[op.0 as usize];
        let v = &self.values;
        let mut it = self.following_texts(s);
        st.claim = o.claim(args, v, &mut it).into();
        let limit = if st.claim == ClaimS::Pending {
            let mut it = self.following_texts(s);
            o.claim_limit(args, v, &mut it)
        } else {
            None
        };
        if limit.is_some() || st.limit().is_some() {
            st.rare.get_or_insert_with(Box::default).limit = limit;
        }
    }

    /// The static hooks that read the text alone (no claim): what a
    /// commit can find before it takes its unit's structural lock.
    pub(crate) fn analyze_text(&self, op: OpId, args: &[ValueId]) -> Static {
        let o = self.ops[op.0 as usize].clone();
        let v = &self.values;
        let defs = o.defs(args, v);
        let may = if defs.is_some() {
            MayDefineS::Nothing
        } else {
            o.may_define(args, v).into()
        };
        let effect = o.effect(args, v);
        let reads = o.reads(args, v);
        let contribs = o.static_contribs(args, v).map(|mut cs| {
            // One per field (later ones combine as a run's would).
            let mut out: Vec<(Field, ValueId)> = Vec::new();
            for (f, x) in cs.drain(..) {
                if let Some(c) = out.iter_mut().find(|c| c.0 == f) {
                    c.1 = self.accs.get(&f).map_or(x, |m| m.combine(v, c.1, x));
                } else {
                    out.push((f, x));
                }
            }
            out
        });
        let preds = o.predicted_defs(args, v).map(Vec::into_boxed_slice);
        if preds.is_some() {
            self.any_preds.store(true, Ordering::Relaxed);
        }
        let rare = StaticRare {
            limit: None,
            reads,
            contribs,
            preds,
        };
        Static {
            defs,
            may,
            effect,
            claim: ClaimS::None,
            rare: (rare != StaticRare::default()).then(|| Box::new(rare)),
        }
    }

    /// Following siblings' texts (for the claim hooks).
    pub(crate) fn following_texts(&self, s: Slot) -> impl Iterator<Item = (OpId, Vec<ValueId>)> + '_ {
        let mut level = self.depth(s);
        let mut x = self.next(s);
        std::iter::from_fn(move || {
            while x != NIL {
                let cur = x;
                let d = self.depth(cur);
                x = self.next(cur);
                if d > level {
                    continue;
                }
                if self.is_marker(cur) {
                    x = NIL;
                    return None;
                }
                level = d;
                if self.own_cont(cur, s) || self.taken(cur, s) || self.placed_after_view(cur, s) {
                    continue;
                }
                let (op, a) = self.text(cur);
                return Some((op, a.to_vec()));
            }
            None
        })
    }

    // ------------------------------------------------------ registration

    /// Puts an active node's static facts into the store: declared
    /// definitions (pending), named pending sets, the in-work lists.
    pub(crate) fn register(&self, s: Slot, notes: &mut Notes) {
        let r = self.rec(s);
        let u = self.unit_of(s);
        if r.data().kind != Kind::Node {
            return;
        }
        let id = r.id.load(Ordering::Acquire);
        let stat = match notes.pre.remove(&id) {
            Some(mut st) => {
                let (op, args) = self.text(s);
                self.analyze_claim(s, op, &args, &mut st);
                st
            }
            None => self.analyze(s),
        };
        let mut fl = 0;
        if stat.defs.is_none() {
            fl |= F_PSET;
            if matches!(stat.may, MayDefineS::Any | MayDefineS::Prefixes(_)) {
                fl |= F_WILD;
            }
        }
        if stat.effect == Effect::Pending {
            fl |= F_PEFF;
        }
        match stat.claim {
            ClaimS::Pending => fl |= F_PCLAIM,
            ClaimS::Count(_) => fl |= F_SCLAIM,
            ClaimS::None => {}
        }
        if stat.contribs().is_some() {
            fl |= super::F_SCONTRIB;
        }
        r.clear(F_PSET | F_WILD | F_PEFF | F_PCLAIM | F_SCLAIM | super::F_SCONTRIB);
        r.set(fl);
        // Declared contributions go in before the run (unless a committed
        // run's already stand).
        if let Some(cs) = stat.contribs()
            && r.data().res.is_none()
        {
            for &(f, x) in cs {
                self.set_contrib(u, s, f, Some(x), notes);
            }
        }
        let defs = stat.defs.clone();
        let names = match &stat.may {
            MayDefineS::Names(ns) => Some(ns.clone()),
            _ => None,
        };
        {
            let mut d = r.data();
            // A changed declaration: the effect its last run set (under the
            // old one) is not this declaration's refutation.
            if d.stat.effect != stat.effect
                && let Some(c) = d.res.as_mut()
            {
                c.effect = None;
            }
            d.stat = stat;
        }
        if let Some(defs) = &defs {
            for &(f, loc) in defs {
                let exists = super::rlock(self.shard(u, f))
                    .get(&(u, f))
                    .is_some_and(|idx| self.def_pos_k(idx, s, loc.entry_key()).is_some());
                if !exists {
                    self.insert_def(u, s, f, loc, DefVal::Pending, notes);
                }
            }
        }
        if let Some(ns) = &names {
            for &f in ns {
                let mut sh = self.wshard(u, f);
                let idx = sh.entry((u, f)).or_default();
                self.drain(idx);
                self.drain(idx);
                if !idx.named.contains(&s) {
                    let pos = self.sorted_pos(&idx.named, s);
                    idx.named.insert(pos, s);
                }
            }
        }
        self.enter_work_lists(s);
        if fl & F_SCLAIM != 0 {
            notes.reclaim.push(s);
        }
    }

    /// The removal counterpart of `register` for the named lists.
    pub(crate) fn unregister_named(&self, s: Slot) {
        let u = self.unit_of(s);
        let may = self.rec(s).data().stat.may.clone();
        if let MayDefineS::Names(ns) = may {
            for f in ns {
                let mut sh = self.wshard(u, f);
                if let Some(idx) = sh.get_mut(&(u, f)) {
                    idx.named.retain(|&x| x != s);
                }
            }
        }
    }

    /// Replaces in-work `s`'s definition set (narrowed by its run, or the
    /// text's again), with its named and wild list entries.
    pub(crate) fn set_may(&self, s: Slot, may: MayDefineS) {
        let u = self.unit_of(s);
        let r = self.rec(s);
        self.unregister_named(s);
        if let MayDefineS::Names(ns) = &may {
            for &f in ns {
                let mut sh = self.wshard(u, f);
                let idx = sh.entry((u, f)).or_default();
                self.drain(idx);
                self.drain(idx);
                if !idx.named.contains(&s) {
                    let pos = self.sorted_pos(&idx.named, s);
                    idx.named.insert(pos, s);
                }
            }
        }
        let wild = matches!(may, MayDefineS::Any | MayDefineS::Prefixes(_)) && r.data().stat.defs.is_none();
        r.data().stat.may = may;
        if wild {
            r.set(F_WILD);
            self.enter_work_lists(s);
        } else {
            r.clear(F_WILD);
            let un = self.unit(u);
            let mut dead = lock(&un.lists);
            if r.fl() & super::F_IN_WILD != 0 {
                r.clear(super::F_IN_WILD);
                *dead += 1;
            }
        }
    }

    /// Index of `s` in a label-sorted slot list.
    pub(crate) fn sorted_pos(&self, xs: &[Slot], s: Slot) -> usize {
        let l = self.rec(s).lab();
        xs.partition_point(|&x| self.rec(x).lab() < l)
    }

    /// Adds `s` to its unit's in-work lists its flags call for.
    pub(crate) fn enter_work_lists(&self, s: Slot) {
        let r = self.rec(s);
        let u = self.unit(self.unit_of(s));
        let mut dead = lock(&u.lists);
        // A prefix pending set: indexed by its prefixes.
        let fl = r.fl();
        if fl & F_WILD != 0 && fl & F_IN_WILD == 0 {
            let ps = match &r.data().stat.may {
                MayDefineS::Prefixes(ps) => Some(ps.clone()),
                _ => None,
            };
            if let Some(ps) = ps {
                if fl & super::F_AT_PFX != 0 {
                    *dead = dead.saturating_sub(1);
                } else {
                    let mut m = u.pfx.write();
                    let mut lens = u.pfx_lens.write();
                    for p in ps {
                        if let Err(i) = lens.binary_search(&p.len()) {
                            lens.insert(i, p.len());
                        }
                        let v = m.entry(p).or_default();
                        let pos = self.sorted_pos(v, s);
                        v.insert(pos, s);
                    }
                    r.set(super::F_AT_PFX);
                }
                r.set(F_IN_WILD);
            }
        }
        for (want, have, at, list) in [
            (F_WILD, F_IN_WILD, super::F_AT_WILD, &u.wild),
            (F_PEFF, F_IN_PEFF, super::F_AT_PEFF, &u.peff),
            (F_PCLAIM, F_IN_PCLAIM, super::F_AT_PCLAIM, &u.pclaim),
        ] {
            let fl = r.fl();
            if fl & want != 0 && fl & have == 0 {
                if fl & at != 0 {
                    // Still there from before: counts again.
                    *dead = dead.saturating_sub(1);
                    if want == F_PCLAIM {
                        let v = list.read();
                        let pos = self.sorted_pos(&v, s);
                        u.pclaim_lo.fetch_min(pos, Ordering::AcqRel);
                    }
                } else {
                    let mut v = list.write();
                    let pos = self.sorted_pos(&v, s);
                    v.insert(pos, s);
                    if want == F_PCLAIM {
                        u.pclaim_lo.fetch_min(pos, Ordering::AcqRel);
                    }
                    r.set(at);
                }
                r.set(have);
            }
        }
    }

    /// Takes `s` out of its unit's pending claimers only (it claims nothing
    /// for now).
    pub(crate) fn leave_pclaim(&self, s: Slot) {
        let r = self.rec(s);
        if !r.has(F_IN_PCLAIM) {
            return;
        }
        let u = self.unit(self.unit_of(s));
        let mut dead = lock(&u.lists);
        r.clear(F_IN_PCLAIM);
        *dead += 1;
        let v = u.pclaim.read();
        let mut lo = u.pclaim_lo.load(Ordering::Acquire);
        while lo < v.len() && !self.rec(v[lo]).has(F_IN_PCLAIM) {
            lo += 1;
        }
        u.pclaim_lo.store(lo, Ordering::Release);
    }

    /// Removes `s` from its unit's in-work lists (`keep_claim`: stay a
    /// pending claimer).
    pub(crate) fn leave_work_lists(&self, s: Slot) {
        let r = self.rec(s);
        let fl = r.fl();
        if fl & (F_IN_WILD | F_IN_PEFF | F_IN_PCLAIM) == 0 {
            return;
        }
        let u = self.unit(self.unit_of(s));
        let mut dead = lock(&u.lists);
        for have in [F_IN_WILD, F_IN_PEFF, F_IN_PCLAIM] {
            if r.fl() & have != 0 {
                r.clear(have);
                *dead += 1;
            }
        }
        if fl & F_IN_PCLAIM != 0 {
            let v = u.pclaim.read();
            let mut lo = u.pclaim_lo.load(Ordering::Acquire);
            while lo < v.len() && !self.rec(v[lo]).has(F_IN_PCLAIM) {
                lo += 1;
            }
            u.pclaim_lo.store(lo, Ordering::Release);
        }
        // Compacted once the entries that left outnumber the rest.
        let total = u.wild.read().len() + u.peff.read().len() + u.pclaim.read().len();
        if *dead * 2 > total + 64 {
            for (have, at, list) in [
                (F_IN_WILD, super::F_AT_WILD, &u.wild),
                (F_IN_PEFF, super::F_AT_PEFF, &u.peff),
                (F_IN_PCLAIM, super::F_AT_PCLAIM, &u.pclaim),
            ] {
                let mut v = list.write();
                v.retain(|&x| {
                    let keep = self.rec(x).has(have);
                    if !keep {
                        self.rec(x).clear(at);
                    }
                    keep
                });
            }
            let mut m = u.pfx.write();
            let mut gone = Vec::new();
            for v in m.values_mut() {
                v.retain(|&x| {
                    let keep = self.rec(x).has(F_IN_WILD);
                    if !keep {
                        gone.push(x);
                    }
                    keep
                });
            }
            m.retain(|_, v| !v.is_empty());
            u.pclaim_lo.store(0, Ordering::Release);
            for x in gone {
                self.rec(x).clear(super::F_AT_PFX);
            }
            *dead = 0;
        }
    }

    /// Takes freed `s` out of the lists it is still physically in (its
    /// slot may be reused at another label).
    fn drop_from_lists(&self, s: Slot) {
        let r = self.rec(s);
        let ats = super::F_AT_WILD | super::F_AT_PEFF | super::F_AT_PCLAIM | super::F_AT_PFX;
        if r.fl() & ats == 0 {
            return;
        }
        let u = self.unit(self.unit_of(s));
        let mut dead = lock(&u.lists);
        if r.fl() & super::F_AT_PFX != 0 {
            for v in u.pfx.write().values_mut() {
                v.retain(|&x| x != s);
            }
            r.clear(super::F_AT_PFX);
        }
        for (at, list) in [
            (super::F_AT_WILD, &u.wild),
            (super::F_AT_PEFF, &u.peff),
            (super::F_AT_PCLAIM, &u.pclaim),
        ] {
            if r.fl() & at != 0 {
                list.write().retain(|&x| x != s);
                if at == super::F_AT_PCLAIM {
                    u.pclaim_lo.store(0, Ordering::Release);
                }
                r.clear(at);
                *dead = dead.saturating_sub(1);
            }
        }
    }

    /// Where `s`'s entry is in `idx.defs`.
    /// `s`'s entry (its outer one, not a scope entry).
    pub(crate) fn def_pos(&self, idx: &super::FieldIdx, s: Slot) -> Option<usize> {
        self.def_pos_k(idx, s, 0)
    }

    /// `s`'s entry with key `scope` (`Locality::entry_key`).
    pub(crate) fn def_pos_k(&self, idx: &super::FieldIdx, s: Slot, scope: u8) -> Option<usize> {
        let l = self.rec(s).lab();
        let mut i = idx.defs.partition_point(|e| self.rec(e.node).lab() < l);
        while i < idx.defs.len() && self.rec(idx.defs[i].node).lab() == l {
            if idx.defs[i].node == s && idx.defs[i].scope == scope {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// `s`'s entry filed under frame `g` (`NO_FRAME`: its global one).
    pub(crate) fn def_pos_at(&self, idx: &super::FieldIdx, s: Slot, g: u32) -> Option<usize> {
        if g != NO_FRAME
            && let Some(i) = self
                .def_positions(idx, s)
                .find(|&i| idx.defs[i].scope != 0 && idx.defs[i].fk == g)
        {
            return Some(i);
        }
        self.def_pos(idx, s)
    }

    /// All of `s`'s entries.
    pub(crate) fn def_positions(&self, idx: &super::FieldIdx, s: Slot) -> impl Iterator<Item = usize> {
        let l = self.rec(s).lab();
        let lo = idx.defs.partition_point(|e| self.rec(e.node).lab() < l);
        let mut hi = lo;
        while hi < idx.defs.len() && self.rec(idx.defs[hi].node).lab() == l {
            hi += 1;
        }
        (lo..hi)
            .filter(move |&i| idx.defs[i].node == s)
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn ins_sorted(&self, v: &mut Vec<Slot>, s: Slot) {
        let l = self.rec(s).lab();
        let i = v.partition_point(|&x| self.rec(x).lab() < l);
        let mut j = i;
        while j < v.len() && self.rec(v[j]).lab() == l {
            if v[j] == s {
                return;
            }
            j += 1;
        }
        v.insert(i, s);
    }

    fn rm_sorted(&self, v: &mut Vec<Slot>, s: Slot) {
        let l = self.rec(s).lab();
        let mut i = v.partition_point(|&x| self.rec(x).lab() < l);
        while i < v.len() && self.rec(v[i]).lab() == l {
            if v[i] == s {
                v.remove(i);
                return;
            }
            i += 1;
        }
    }

    /// Files `s`'s entry under its frame (and as global, or as outside
    /// the root frame).
    fn add_def_keys(&self, idx: &mut super::FieldIdx, u: u32, s: Slot, loc: Locality, fk: u32) {
        self.ins_sorted(idx.local.entry(fk).or_default(), s);
        if loc == Locality::Global {
            self.ins_sorted(&mut idx.globals, s);
        } else if fk != self.unit(u).root_frame {
            self.ins_sorted(&mut idx.nonroot, s);
        }
    }

    /// Unfiles `s`'s entry with key `scope` (its other entries keep the
    /// lists they share).
    fn rm_def_keys(&self, idx: &mut super::FieldIdx, s: Slot, loc: Locality, fk: u32, scope: u8) {
        let root = self.unit(self.unit_of(s)).root_frame;
        let others: Vec<(u32, Locality)> = self
            .def_positions(idx, s)
            .filter(|&i| idx.defs[i].scope != scope)
            .map(|i| (idx.defs[i].fk, idx.defs[i].loc))
            .collect();
        if !others.is_empty() {
            if !others.iter().any(|o| o.0 == fk)
                && let Some(v) = idx.local.get_mut(&fk)
            {
                self.rm_sorted(v, s);
                if v.is_empty() {
                    idx.local.remove(&fk);
                }
            }
            if loc == Locality::Global {
                if !others.iter().any(|o| o.1 == Locality::Global) {
                    self.rm_sorted(&mut idx.globals, s);
                }
            } else if !others.iter().any(|o| o.1 != Locality::Global && o.0 != root) {
                self.rm_sorted(&mut idx.nonroot, s);
            }
            return;
        }
        if let Some(v) = idx.local.get_mut(&fk) {
            self.rm_sorted(v, s);
            if v.is_empty() {
                idx.local.remove(&fk);
            }
        }
        if loc == Locality::Global {
            self.rm_sorted(&mut idx.globals, s);
        } else {
            self.rm_sorted(&mut idx.nonroot, s);
        }
    }

    /// Inserts a subscription by its position.
    /// Files `idx`'s inbox into its lists (under the shard's write lock).
    pub(crate) fn drain(&self, idx: &mut super::FieldIdx) {
        let items = std::mem::take(idx.inbox.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner));
        for it in items {
            match it {
                super::Inbox::Use { sub, chain } => {
                    self.ins_sub(&mut idx.readers, sub);
                    for &g in chain.iter() {
                        self.ins_sub(idx.freaders.entry(g).or_default(), sub);
                    }
                }
                super::Inbox::Seq { sub, frame } => {
                    self.ins_sub(&mut idx.readers, sub);
                    self.ins_sub(idx.sreaders.entry(frame).or_default(), sub);
                }
                super::Inbox::Range(sub) => self.ins_sub(&mut idx.range_readers, sub),
                super::Inbox::User { node, reader, gn } => {
                    let is: Vec<usize> = self.def_positions(idx, node).collect();
                    for i in is {
                        idx.defs[i].users.push((reader, gn));
                    }
                }
            }
        }
    }

    /// Delivers subscriptions to field `f` of unit `u`: into its inbox
    /// under the read lock, or (no index yet) filed under the write lock.
    pub(crate) fn deliver(&self, u: u32, f: Field, items: Vec<super::Inbox>) {
        {
            let sh = super::rlock(self.shard(u, f));
            if let Some(idx) = sh.get(&(u, f)) {
                lock(&idx.inbox).extend(items);
                return;
            }
        }
        let mut sh = self.wshard(u, f);
        let idx = sh.entry((u, f)).or_default();
        self.drain(idx);
        lock(&idx.inbox).extend(items);
        self.drain(idx);
    }

    pub(crate) fn ins_sub(&self, v: &mut super::SubList, sub: super::Sub) {
        if !v.unsorted
            && let Some(last) = v.v.last()
            && self.rec(last.at).lab() > self.rec(sub.at).lab()
        {
            v.unsorted = true;
        }
        v.v.push(sub);
    }

    /// Notifies the live subscriptions of `v` at positions in `(lo, hi]`
    /// (`hi` none: to the end), dropping the dead ones met.
    fn scan_subs(
        &self,
        v: &mut super::SubList,
        lo: u64,
        hi: Option<u64>,
        skip: Slot,
        cause: &Cause,
        notes: &mut Notes,
    ) {
        if v.unsorted {
            // (Stable: entries at one label keep their commit order.)
            v.v.sort_by_key(|x| self.rec(x.at).lab());
            v.unsorted = false;
        }
        let v = &mut v.v;
        let start = v.partition_point(|x| self.rec(x.at).lab() <= lo);
        let mut j = start;
        let mut i = start;
        while i < v.len() {
            let sub = v[i];
            if hi.is_some_and(|h| self.rec(sub.at).lab() > h) {
                break;
            }
            let r = self.rec(sub.node);
            let live = r.lgen.load(Ordering::Acquire) == sub.gn && r.st() != S_FREE;
            if live {
                if sub.at != skip {
                    notes.touch.push((sub.node, sub.gn, cause.clone()));
                }
                v[j] = sub;
                j += 1;
            }
            i += 1;
        }
        if j < i {
            v.drain(j..i);
        }
    }

    /// The label of the first entry of `v` after label `l`.
    fn next_after(&self, v: &[Slot], l: u64) -> Option<u64> {
        let i = v.partition_point(|&x| self.rec(x).lab() <= l);
        v.get(i).map(|&x| self.rec(x).lab())
    }

    /// Notifies the readers an entry of `s` filed under `fk` (or global)
    /// may come between and their answers: those after it up to the next
    /// entry that every one of them sees.
    fn notify_entry(&self, idx: &mut super::FieldIdx, s: Slot, f: Field, loc: Locality, fk: u32, notes: &mut Notes) {
        let l = self.rec(s).lab();
        let cause = Cause::Changed { site: None, field: f };
        if loc == Locality::Global {
            let hi = self.next_after(&idx.globals, l);
            self.scan_subs(&mut idx.readers, l, hi, s, &cause, notes);
        } else {
            let hi = idx.local.get(&fk).and_then(|v| self.next_after(v, l));
            if let Some(v) = idx.freaders.get_mut(&fk) {
                self.scan_subs(v, l, hi, s, &cause, notes);
            }
        }
        if let Some(v) = idx.sreaders.get_mut(&fk) {
            self.scan_subs(v, l, None, s, &cause, notes);
        }
    }

    /// Inserts a definition entry for `s`, notifying readers after it.
    pub(crate) fn insert_def(&self, u: u32, s: Slot, f: Field, loc: Locality, val: DefVal, notes: &mut Notes) {
        // (A scope frame reads the node's data: before the shard lock. A
        // node's own frame is read under it, so a frame walk's refile,
        // which takes the lock, sees this entry or files it right.)
        let fk_scope = (loc.entry_key() != 0).then(|| self.def_frame(s, loc));
        let mut sh = self.wshard(u, f);
        let fk_new = fk_scope.unwrap_or_else(|| self.frame_of(s));
        let idx = sh.entry((u, f)).or_default();
        self.drain(idx);
        let scope = loc.entry_key();
        if let Some(i) = self.def_pos_k(idx, s, scope) {
            let e = &mut idx.defs[i];
            if e.val != val || e.loc != loc {
                let old_loc = e.loc;
                e.val = val;
                e.loc = loc;
                let fk = e.fk;
                // A reader that read this value from here (committed, in
                // work or as a prediction the value bears out) stands.
                let kept = match val {
                    DefVal::Val(x) if old_loc == loc => Some(x),
                    _ => None,
                };
                let projs = !self.projs.is_empty() || self.any_proj.load(Ordering::Relaxed);
                for (n, g) in std::mem::take(&mut e.users) {
                    // Only a settled reader's committed log is the one this
                    // subscription stands for (one committing has
                    // subscribed its new run before storing its log).
                    let settled = {
                        let nr = self.rec(n);
                        !super::in_work(nr.st()) && nr.lgen.load(Ordering::Acquire) == g
                    };
                    if settled
                        && let Some(x) = kept
                        && self.read_from(n, s, f) == Some(x)
                    {
                        e.users.push((n, g));
                        continue;
                    }
                    if settled && projs && old_loc == loc && self.proj_stands(n, s, f, val) {
                        e.users.push((n, g));
                        continue;
                    }
                    notes.touch.push((n, g, Cause::Changed { site: None, field: f }));
                }
                if old_loc != loc {
                    self.rm_def_keys(idx, s, old_loc, fk, scope);
                    self.add_def_keys(idx, u, s, loc, fk);
                    self.notify_entry(idx, s, f, old_loc, fk, notes);
                    self.notify_entry(idx, s, f, loc, fk, notes);
                }
            }
            return;
        }
        let l = self.rec(s).lab();
        let pos = idx.defs.partition_point(|e| self.rec(e.node).lab() < l);
        let fk = fk_new;
        idx.defs.insert(
            pos,
            DefE {
                node: s,
                loc,
                val,
                users: Vec::new(),
                fk,
                scope,
            },
        );
        self.add_def_keys(idx, u, s, loc, fk);
        self.notify_entry(idx, s, f, loc, fk, notes);
    }

    /// The frame an entry of `s` with locality `loc` is filed under: its
    /// node's frame, or for `Locality::Scope` the innermost scope the node
    /// opens after itself (its own frame while it opens none).
    pub(crate) fn def_frame(&self, s: Slot, loc: Locality) -> u32 {
        if let Locality::ScopeAt(k) = loc {
            let (br, _) = self.brackets_of(s);
            let r = self.rec(s);
            let n = u32::from(br.ob) + u32::from(br.oa);
            if u32::from(k) < n && r.nframes.load(Ordering::Acquire) >= n {
                return r.frames.load(Ordering::Acquire) + u32::from(k);
            }
            return self.frame_of(s);
        }
        if loc == Locality::Scope {
            let (br, _) = self.brackets_of(s);
            if br.oa > 0 {
                let r = self.rec(s);
                let k = u32::from(br.ob) + u32::from(br.oa);
                if r.nframes.load(Ordering::Acquire) >= k {
                    return r.frames.load(Ordering::Acquire) + k - 1;
                }
            }
        }
        self.frame_of(s)
    }

    /// Whether one of `s`'s scope-local entries is filed under another
    /// frame than the one it opens now.
    pub(crate) fn scope_misfiled(&self, u: u32, s: Slot) -> bool {
        // (The run's definitions and the declared ones, filed before it ran.)
        let defs: Vec<(Field, Locality)> = {
            let d = self.rec(s).data();
            let run = d
                .res
                .as_ref()
                .into_iter()
                .flat_map(|c| c.defs.iter().map(|d| (d.f, d.loc)));
            let stat = d.stat.defs.iter().flatten().copied();
            run.chain(stat).filter(|e| e.1.entry_key() != 0).collect()
        };
        defs.into_iter().any(|(f, loc)| {
            let want = self.def_frame(s, loc);
            let sh = super::rlock(self.shard(u, f));
            sh.get(&(u, f))
                .and_then(|idx| self.def_pos_k(idx, s, loc.entry_key()).map(|i| idx.defs[i].fk != want))
                .unwrap_or(false)
        })
    }

    /// The run-set kind of frame `f` that `opener` opens: its entry in
    /// `set_kinds` by position, else `set_kind`'s.
    pub(crate) fn opener_kind(&self, opener: Slot, f: u32) -> Option<ValueId> {
        let r = self.rec(opener);
        let base = r.frames.load(Ordering::Acquire);
        let d = r.data();
        f.checked_sub(base)
            .and_then(|i| d.kind_vals.get(i as usize).copied())
            .or(d.kind_val)
    }

    /// Files `s`'s entries again under its frame now (its frame changed),
    /// notifying the readers on both sides.
    pub(crate) fn refile_defs(&self, u: u32, s: Slot, notes: &mut Notes) {
        let fields: Vec<Field> = {
            let d = self.rec(s).data();
            let mut v: Vec<Field> = d
                .res
                .as_ref()
                .map_or_else(Vec::new, |c| c.defs.iter().map(|d| d.f).collect());
            if let Some(sd) = &d.stat.defs {
                v.extend(sd.iter().map(|e| e.0));
            }
            v.sort_unstable_by_key(|f| f.0.raw());
            v.dedup();
            v
        };
        let fk_node = self.frame_of(s);
        for f in fields {
            let mut sh = self.wshard(u, f);
            let Some(idx) = sh.get_mut(&(u, f)) else { continue };
            self.drain(idx);
            let is: Vec<usize> = self.def_positions(idx, s).collect();
            for i in is {
                let (old, loc, scope) = (idx.defs[i].fk, idx.defs[i].loc, idx.defs[i].scope);
                let fk = if scope == 0 { fk_node } else { self.def_frame(s, loc) };
                for (n, g) in std::mem::take(&mut idx.defs[i].users) {
                    notes.touch.push((n, g, Cause::Changed { site: None, field: f }));
                }
                if old != fk {
                    self.rm_def_keys(idx, s, loc, old, scope);
                    self.notify_entry(idx, s, f, loc, old, notes);
                    idx.defs[i].fk = fk;
                    self.add_def_keys(idx, u, s, loc, fk);
                }
                self.notify_entry(idx, s, f, loc, fk, notes);
            }
        }
    }

    /// Removes `s`'s definitions of `f` (every entry), notifying their
    /// users and readers.
    pub(crate) fn remove_def(&self, u: u32, s: Slot, f: Field, notes: &mut Notes) {
        let keys: Vec<u8> = {
            let sh = super::rlock(self.shard(u, f));
            sh.get(&(u, f)).map_or_else(Vec::new, |idx| {
                self.def_positions(idx, s).map(|i| idx.defs[i].scope).collect()
            })
        };
        for k in keys {
            self.remove_def_k(u, s, f, k, notes);
        }
    }

    /// Removes `s`'s definition of `f` with entry key `scope`.
    pub(crate) fn remove_def_k(&self, u: u32, s: Slot, f: Field, scope: u8, notes: &mut Notes) {
        let mut sh = self.wshard(u, f);
        let Some(idx) = sh.get_mut(&(u, f)) else { return };
        self.drain(idx);
        if let Some(i) = self.def_pos_k(idx, s, scope) {
            let e = idx.defs.remove(i);
            for (n, g) in e.users {
                notes.touch.push((n, g, Cause::Changed { site: None, field: f }));
            }
            self.rm_def_keys(idx, s, e.loc, e.fk, scope);
            self.notify_entry(idx, s, f, e.loc, e.fk, notes);
        }
    }

    /// Sets the contribution of `s` to accumulator `f`.
    pub(crate) fn set_contrib(&self, u: u32, s: Slot, f: Field, val: Option<ValueId>, notes: &mut Notes) {
        let mut sh = self.wshard(u, f);
        let idx = sh.entry((u, f)).or_default();
        self.drain(idx);
        let l = self.rec(s).lab();
        // Labels are not unique (a node freed keeps the label its
        // replacement takes): look through every entry at `l`.
        let pos = {
            let mut i = idx.contribs.partition_point(|c| self.rec(c.0).lab() < l);
            while i < idx.contribs.len() && idx.contribs[i].0 != s && self.rec(idx.contribs[i].0).lab() == l {
                i += 1;
            }
            (i < idx.contribs.len() && idx.contribs[i].0 == s).then_some(i)
        };
        let changed = match (pos, val) {
            (Some(i), Some(v)) => {
                if idx.contribs[i].1 == v {
                    false
                } else {
                    idx.contribs[i].1 = v;
                    true
                }
            }
            (Some(i), None) => {
                idx.contribs.remove(i);
                true
            }
            (None, Some(v)) => {
                let p = idx.contribs.partition_point(|c| self.rec(c.0).lab() < l);
                idx.contribs.insert(p, (s, v));
                true
            }
            (None, None) => false,
        };
        if changed {
            // The range readers after it (by position) whose range starts
            // before it.
            let v = &mut idx.range_readers;
            if v.unsorted {
                v.v.sort_by_key(|x| self.rec(x.at).lab());
                v.unsorted = false;
            }
            let v = &mut v.v;
            let start = v.partition_point(|x| self.rec(x.at).lab() <= l);
            let mut j = start;
            for i in start..v.len() {
                let sub = v[i];
                let r = self.rec(sub.node);
                let live = r.lgen.load(Ordering::Acquire) == sub.gn && r.st() != S_FREE;
                if live {
                    let lo = if sub.from == NIL { 0 } else { self.rec(sub.from).lab() };
                    if lo < l {
                        notes
                            .touch
                            .push((sub.node, sub.gn, Cause::Range { from: None, field: f }));
                    }
                    v[j] = sub;
                    j += 1;
                }
            }
            v.truncate(j);
        }
    }

    // -------------------------------------------- deactivation and frees

    /// Removes a node's facts from the store (it goes inert or is freed):
    /// definitions, contributions, publications, output, its unfolding and
    /// its claims. Its static facts stay in `data.stat`.
    pub(crate) fn clear_facts(&self, s: Slot, notes: &mut Notes) {
        let u = self.unit_of(s);
        let r = self.rec(s);
        let (defs, contribs, pubs, kids, conts, claimed, waiters, outs, stat_defs) = {
            let mut d = r.data();
            let res = d.res.take();
            d.kept = None;
            let (defs, contribs, pubs) = res.map_or_else(
                || (Vec::new(), Vec::new(), Vec::new()),
                |c| {
                    let c = *c;
                    (c.defs, c.contribs, c.pubs)
                },
            );
            (
                defs,
                contribs,
                pubs,
                std::mem::take(&mut d.kids),
                std::mem::take(&mut d.conts),
                d.take_claimed(),
                std::mem::take(&mut d.waiters),
                std::mem::take(&mut d.out_users),
                (d.stat.defs.clone(), d.stat.contribs().cloned()),
            )
        };
        let (stat_defs, stat_contribs) = stat_defs;
        r.clear(F_DONE);
        for d in &defs {
            self.remove_def(u, s, d.f, notes);
        }
        if let Some(sd) = stat_defs {
            for (f, _) in sd {
                self.remove_def(u, s, f, notes);
            }
        }
        for (f, _) in contribs.into_iter().chain(stat_contribs.unwrap_or_default()) {
            self.set_contrib(u, s, f, None, notes);
        }
        if !pubs.is_empty() {
            let mut p = lock(&self.pubs);
            let mut dirty = lock(&self.pubs_dirty);
            for (f, _) in pubs {
                if let Some(m) = p.get_mut(&f) {
                    m.remove(&s);
                }
                dirty.insert(f);
            }
        }
        if r.has(F_HAS_OUT) {
            r.clear(F_HAS_OUT);
        }
        for (n, g) in outs {
            notes.touch.push((n, g, Cause::Output { of: self.node_id(s) }));
        }
        for (n, q) in waiters {
            notes.touch.push((n, q, Cause::Structure));
        }
        self.unregister_named(s);
        self.leave_work_lists(s);
        for k in kids.into_iter().chain(conts) {
            self.free_entry(k, notes);
        }
        for c in claimed {
            self.release(c, s, notes);
        }
        {
            let mut un = self
                .undeclared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            un.remove(&s);
        }
        notes.frames_from.push(s);
    }

    /// Frees `s` and its subtree.
    pub(crate) fn free_entry(&self, s: Slot, notes: &mut Notes) {
        self.escalate(self.unit_of(s));
        let r = self.rec(s);
        loop {
            let st = r.st();
            if st == S_FREE {
                return;
            }
            if st == S_RUN {
                r.set(super::F_DOOM_FREE);
                if r.st() == S_RUN {
                    // Its worker frees it; the order loses it now.
                    self.drop_from_order(s, notes);
                    return;
                }
                continue;
            }
            if r.status
                .compare_exchange(st, S_FREE, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        self.finish_free(s, notes);
    }

    /// The order and id bookkeeping of a free (the facts go with
    /// `clear_facts`).
    fn drop_from_order(&self, s: Slot, notes: &mut Notes) {
        self.note(super::merge::kn(s));
        let r = self.rec(s);
        if r.prev.load(Ordering::Acquire) == NIL && self.unit(self.unit_of(s)).first.load(Ordering::Acquire) != s {
            return; // already unlinked
        }
        self.notify_gap(s, notes);
        self.notify_sib(s, notes);
        let nx = self.next(s);
        self.unlink(s);
        if nx != NIL {
            notes.frames_from.push(nx);
        }
    }

    /// Completes a free once the node is not running.
    pub(crate) fn finish_free(&self, s: Slot, notes: &mut Notes) {
        self.note(super::merge::kn(s));
        let r = self.rec(s);
        let marker = matches!(r.data().kind, Kind::Marker { .. });
        if !marker {
            self.clear_facts(s, notes);
        }
        self.drop_from_order(s, notes);
        // A claim this node was under no longer counts.
        r.claimed_by.store(NIL, Ordering::Release);
        let id = r.id.load(Ordering::Acquire);
        {
            let mut m = lock(self.id_shard(id));
            if m.get(&id) == Some(&s) {
                m.remove(&id);
            }
        }
        self.leave_work_lists(s);
        self.drop_from_lists(s);
        r.status.store(S_FREE, Ordering::Release);
        self.wake_waiters(s);
        r.gn.fetch_add(1, Ordering::AcqRel);
        if !marker {
            self.live.fetch_sub(1, Ordering::Relaxed);
            let born = r.data().born;
            crate::obs::count!(Free);
            if born < self.epoch.load(Ordering::Relaxed) {
                self.c.freed.fetch_add(1, Ordering::Relaxed);
            } else {
                self.c.created.fetch_sub(1, Ordering::Relaxed);
            }
            self.snapshot(s);
            if r.has(F_APP) {
                let key = r.data().app_key();
                let mut apps = lock(&self.apps);
                if apps.get(&key) == Some(&s) {
                    apps.remove(&key);
                }
                let au = r.data().app_unit();
                if au != u32::MAX {
                    self.unit(au).live.store(false, Ordering::Release);
                }
            }
        }
    }

    /// Makes `s` inert: claimed by `by`.
    pub(crate) fn make_inert(&self, s: Slot, by: Slot, notes: &mut Notes) {
        self.escalate(self.unit_of(s));
        if self.held_by_other(s, by) {
            return;
        }
        lock(&self.hdirty).push(s);
        let r = self.rec(s);
        r.claimed_by.store(by, Ordering::Release);
        loop {
            let st = r.st();
            if matches!(st, S_FREE | S_INERT | S_MARKER) {
                return;
            }
            if st == S_RUN {
                r.set(super::F_DOOM_INERT);
                if r.st() == S_RUN {
                    notes.frames_from.push(s);
                    self.notify_sib(s, notes);
                    return;
                }
                continue;
            }
            if r.status
                .compare_exchange(st, S_INERT, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        self.finish_inert(s, notes);
    }

    pub(crate) fn finish_inert(&self, s: Slot, notes: &mut Notes) {
        self.note(super::merge::kn(s));
        self.snapshot(s);
        self.clear_facts(s, notes);
        self.rec(s).status.store(S_INERT, Ordering::Release);
        self.wake_waiters(s);
        self.notify_sib(s, notes);
        notes.frames_from.push(s);
    }

    /// Releases `s` from the claim of `by` (if it is still under it).
    pub(crate) fn release(&self, s: Slot, by: Slot, notes: &mut Notes) {
        self.escalate(self.unit_of(s));
        let r = self.rec(s);
        if r.claimed_by.load(Ordering::Acquire) != by {
            return;
        }
        r.claimed_by.store(NIL, Ordering::Release);
        if r.st() != S_INERT {
            // Claimed while it ran: the mark that would make it inert at
            // its commit goes with the claim.
            r.clear(super::F_DOOM_INERT);
            return;
        }
        r.status.store(S_NEW, Ordering::Release);
        r.set(F_NEEDS_RUN);
        self.register(s, notes);
        self.notify_sib(s, notes);
        notes.frames_from.push(s);
        notes.enqueue.push(s);
    }

    /// Saves `s`'s committed state for the canonical counters, once per
    /// evaluation.
    pub(crate) fn snapshot(&self, s: Slot) {
        let ep = self.epoch.load(Ordering::Relaxed);
        let r = self.rec(s);
        let mut d = r.data();
        #[allow(clippy::cast_possible_truncation)]
        let ep32 = ep as u32;
        if d.snap_epoch == ep32 {
            return;
        }
        d.snap_epoch = ep32;
        // (Only a node with a committed run keeps one: a new node compares
        // as not done.)
        let snap = d.res.is_some().then(|| super::Snap {
            done: r.has(F_DONE),
            sig: d.res.as_ref().map_or(0, |c| c.sig),
            out: d.res.as_ref().map(|c| c.output),
            binds: d.res.as_ref().map_or(0, |c| c.binds),
        });
        drop(d);
        if let Some(sn) = snap {
            lock(&self.snaps).insert(s, sn);
        }
        lock(&self.touched).push(s);
        lock(&self.hdirty).push(s);
    }

    // ------------------------------------------------- children and claims

    /// Makes `parent`'s children (with markers) and continuations those
    /// specified, reusing the ones with equal ids. Returns the new kids'
    /// slots in order.
    pub(crate) fn set_unfolding(
        &self,
        parent: Slot,
        kids: &[Spec],
        conts: &[Spec],
        anchors: &[Option<Slot>],
        notes: &mut Notes,
    ) {
        let u = self.unit_of(parent);
        let (old_kids, old_conts) = {
            let d = self.rec(parent).data();
            (d.kids.clone(), d.conts.clone())
        };
        let same = |old: &[Slot], new: &[Spec]| {
            old.len() == new.len()
                && old
                    .iter()
                    .zip(new)
                    .all(|(&o, n)| self.rec(o).id.load(Ordering::Acquire) == n.id() && self.rec(o).st() != S_FREE)
        };
        let kids_same = same(&old_kids, kids);
        let conts_same = same(&old_conts, conts);
        if kids_same && conts_same {
            return;
        }
        let pdepth = self.depth(parent);
        let grand = self.rec(parent).parent.load(Ordering::Acquire);
        if !kids_same {
            let new = self.respec(
                u,
                parent,
                &old_kids,
                kids,
                &[],
                pdepth + 1,
                parent,
                parent,
                Role::Child,
                notes,
            );
            self.rec(parent).data().kids = new;
            self.notify_sib(parent, notes);
        }
        if !conts_same {
            let new = self.respec(
                u,
                parent,
                &old_conts,
                conts,
                anchors,
                pdepth,
                grand,
                parent,
                Role::Cont,
                notes,
            );
            self.rec(parent).data().conts = new;
        }
    }

    /// Replaces `old` entries by `specs` (reusing equal ids), placing new
    /// ones after `parent`'s subtree part that precedes them.
    #[allow(clippy::too_many_arguments)]
    fn respec(
        &self,
        u: u32,
        parent: Slot,
        old: &[Slot],
        specs: &[Spec],
        anchors: &[Option<Slot>],
        depth: u32,
        tree_parent: Slot,
        owner: Slot,
        role: Role,
        notes: &mut Notes,
    ) -> Vec<Slot> {
        let keep: std::collections::HashSet<u64> = specs.iter().map(Spec::id).collect();
        for &o in old {
            let id = self.rec(o).id.load(Ordering::Acquire);
            if !keep.contains(&id) || self.rec(o).st() == S_FREE {
                self.free_entry(o, notes);
            }
        }
        let mut out = Vec::with_capacity(specs.len());
        // Where the next entry goes: after this.
        let mut after = if role == Role::Cont {
            self.subtree_end(parent)
        } else {
            parent
        };
        let mut fresh: Vec<Slot> = Vec::new();
        // Kept entries keep their places only in their old order: one the
        // specs now put before an earlier-kept one is made again where it
        // belongs (its old place is after it in the list).
        let mut last_old: Option<usize> = None;
        // Placed after an earlier node: the last one placed there so far.
        let mut at_end: crate::FxHashMap<Slot, Slot> = crate::FxHashMap::default();
        for (k, sp) in specs.iter().enumerate() {
            if let Some(a) = anchors.get(k).copied().flatten() {
                let kept = self
                    .slot_of(sp.id())
                    .filter(|&x| self.rec(x).st() != S_FREE && old.contains(&x));
                let x = if let Some(x) = kept {
                    x
                } else {
                    let Spec::Node { id, op, args } = sp else { continue };
                    let data = Data::new(Kind::Node, role, *op, args.clone(), owner);
                    let s = self.alloc(u, *id, data, self.depth(a), self.rec(a).parent.load(Ordering::Acquire));
                    let pos = at_end
                        .get(&a)
                        .map_or_else(|| self.extent_end(a), |&p| self.extent_end(p));
                    self.link_after(u, pos, &[s]);
                    notes.frames_from.push(s);
                    s
                };
                at_end.insert(a, x);
                out.push(x);
                continue;
            }
            let mut existing = self
                .slot_of(sp.id())
                .filter(|&x| self.rec(x).st() != S_FREE && old.contains(&x));
            if let Some(x) = existing {
                let i = old.iter().position(|&o| o == x);
                if last_old.is_some_and(|l| i.is_some_and(|i| i < l)) {
                    self.free_entry(x, notes);
                    existing = None;
                } else {
                    last_old = i.or(last_old);
                }
            }
            if let Some(x) = existing {
                if !fresh.is_empty() {
                    self.link_after(u, after, &fresh);
                    for &n in &fresh {
                        notes.frames_from.push(n);
                    }
                    fresh.clear();
                }
                out.push(x);
                after = self.extent_end(x);
                continue;
            }
            let s = match sp {
                Spec::Node { id, op, args } => {
                    let data = Data::new(Kind::Node, role, *op, args.clone(), owner);
                    self.alloc(u, *id, data, depth, tree_parent)
                }
                Spec::Marker { id, open } => {
                    let data = Data::new(
                        Kind::Marker { open: *open },
                        role,
                        OpId(0),
                        Arc::from(Vec::new()),
                        owner,
                    );
                    self.alloc(u, *id, data, depth, tree_parent)
                }
            };
            out.push(s);
            // A light writer's new entry is seen whole once linked: its
            // frame first (an entry without brackets is in the frame after
            // its predecessor; one with brackets needs the full writer).
            if self.is_light(u) {
                if after == NIL {
                    self.escalate(u);
                } else {
                    // Its static facts first: its brackets come from them
                    // (`register` files the rest after it is linked).
                    if let Some(st) = notes.pre.get(&sp.id()) {
                        self.rec(s).data().stat = st.clone();
                    }
                    self.preset_frames(u, after, s);
                }
            }
            // Link now so later specs find their place.
            self.link_after(u, after, &[s]);
            after = s;
            fresh.clear();
            notes.frames_from.push(s);
        }
        for &s in &out {
            if old.contains(&s) {
                continue;
            }
            if self.rec(s).st() == S_NEW {
                self.rec(s).set(F_NEEDS_RUN);
                self.register(s, notes);
                notes.enqueue.push(s);
            }
            self.notify_gap(s, notes);
        }
        out
    }

    /// Recomputes a static count claim.
    /// The first `n` following siblings of `s` (a run's claim).
    pub(crate) fn run_claim_list(&self, s: Slot, n: usize) -> Vec<Slot> {
        let mut list = Vec::new();
        if n > 0 {
            self.following(s, s, |x, skipped| {
                if !skipped {
                    list.push(x);
                }
                list.len() < n
            });
        }
        list
    }

    pub(crate) fn reclaim(&self, c: Slot, notes: &mut Notes) {
        let r = self.rec(c);
        let st = r.st();
        if matches!(st, S_FREE | S_MARKER) {
            return;
        }
        if st == S_INERT {
            let claimed = r.data().take_claimed();
            for x in claimed {
                self.release(x, c, notes);
            }
            return;
        }
        // Its run's claim over the declared one: the first `n` again.
        let run = r.data().x.as_ref().and_then(|e| e.claim_run);
        if let Some(n) = run {
            let list = self.run_claim_list(c, n);
            if self.apply_claim(c, &list, notes) {
                notes.touch.push((c, u32::MAX, Cause::Structure));
            }
            return;
        }
        let (op, args) = self.text(c);
        let o = self.ops[op.0 as usize].clone();
        // The siblings the hook may read, gathered a few at a time: most
        // hooks look at the first few only.
        let mut cap = 16usize;
        let (view, seen, consumed, n) = loop {
            let mut view: Vec<Slot> = Vec::new();
            let mut seen: Vec<Slot> = Vec::new();
            self.following(c, c, |x, skipped| {
                seen.push(x);
                if !skipped {
                    view.push(x);
                }
                view.len() < cap
            });
            let consumed = std::cell::Cell::new(0usize);
            let texts: Vec<(OpId, Vec<ValueId>)> = view
                .iter()
                .map(|&x| {
                    let (op, a) = self.text(x);
                    (op, a.to_vec())
                })
                .collect();
            let mut it = texts.iter().cloned().inspect(|_| consumed.set(consumed.get() + 1));
            let claim = o.claim(&args, &self.values, &mut it);
            let want = match claim {
                crate::api::Claim::Count(n) => n,
                _ => 0,
            };
            let more = view.len() == cap && (consumed.get() >= view.len() || want >= view.len());
            if !more || cap >= 4096 {
                let n = want.min(view.len());
                break (view, seen, consumed.get(), n);
            }
            cap = (cap * 4).min(4096);
        };
        let window_len = consumed.max(n).min(view.len());
        let new: Vec<Slot> = view[..n].to_vec();
        // Subscribe to what the hook saw (and the gaps after it).
        let last_seen = if window_len == 0 { c } else { view[window_len - 1] };
        for &x in seen
            .iter()
            .take_while(|&&x| x != last_seen)
            .chain(std::iter::once(&last_seen))
        {
            self.sib_subscribe(x, c, 0);
        }
        self.sib_subscribe(c, c, 0);
        let old = std::mem::replace(&mut r.data().ex().claimed, new.clone());
        r.data().ex().window = view[..window_len].to_vec();
        for &x in &old {
            if !new.contains(&x) {
                self.release(x, c, notes);
            }
        }
        for &x in &new {
            if !old.contains(&x) {
                self.make_inert(x, c, notes);
            }
        }
    }

    /// Applies a pending claimer's decided claim: `list` (the siblings its
    /// run saw).
    pub(crate) fn apply_claim(&self, c: Slot, list: &[Slot], notes: &mut Notes) -> bool {
        let r = self.rec(c);
        let old = r.data().x.as_ref().map(|e| e.claimed.clone()).unwrap_or_default();
        for &x in &old {
            if !list.contains(&x) {
                self.release(x, c, notes);
            }
        }
        // A node an earlier claimer holds is not taken (it stays that
        // claimer's); one a later claimer holds is taken over. The list
        // records only what it took (a release later frees only those).
        let mut taken = Vec::with_capacity(list.len());
        for &x in list {
            if !old.contains(&x) && self.held_by_other(x, c) {
                continue;
            }
            if !old.contains(&x) {
                self.make_inert(x, c, notes);
            }
            taken.push(x);
        }
        let changed = old != taken;
        r.data().ex().claimed = taken;
        changed
    }

    /// Whether `x` is claimed by a live claimer before `c` (an earlier
    /// claimer wins, as in the text's order: a later one's claim yields).
    fn held_by_other(&self, x: Slot, c: Slot) -> bool {
        let by = self.rec(x).claimed_by.load(Ordering::Acquire);
        by != NIL && by != c && !matches!(self.rec(by).st(), S_FREE) && self.before(by, c)
    }

    /// Processes the structural follow-ups: static claims, frames, scans.
    /// After an edit: claims as `settle_structure`, but the frame walks
    /// wait for the evaluation (`settle_deferred`), so that the edits
    /// made before it are walked over once.
    pub(crate) fn settle_structure_edit(&self, u: u32, notes: &mut Notes) {
        let fs = std::mem::take(&mut notes.frames_from);
        self.settle_structure(u, notes);
        let fs2 = std::mem::take(&mut notes.frames_from);
        let mut d = lock(&self.deferred_frames);
        d.extend(fs);
        d.extend(fs2);
    }

    /// The frame walks the edits deferred.
    pub(crate) fn settle_deferred(&self) {
        let mut fs = std::mem::take(&mut *lock(&self.deferred_frames));
        if fs.is_empty() {
            return;
        }
        fs.retain(|&x| self.rec(x).st() != S_FREE);
        fs.sort_by_key(|&x| (self.unit_of(x), self.rec(x).lab()));
        fs.dedup();
        let mut i = 0;
        while i < fs.len() {
            let u = self.unit_of(fs[i]);
            let mut notes = Notes::default();
            while i < fs.len() && self.unit_of(fs[i]) == u {
                notes.frames_from.push(fs[i]);
                i += 1;
            }
            {
                let _g = self.wunit(u);
                self.settle_structure(u, &mut notes);
            }
            self.dispatch(&mut notes, NIL);
        }
    }

    pub(crate) fn settle_structure(&self, u: u32, notes: &mut Notes) {
        let mut guard = 0;
        if !notes.reclaim.is_empty() {
            self.escalate(u);
        }
        while !notes.reclaim.is_empty() {
            let mut rs = std::mem::take(&mut notes.reclaim);
            rs.sort_by_key(|&x| self.rec(x).lab());
            rs.dedup();
            for c in rs {
                if self.unit_of(c) == u {
                    self.reclaim(c, notes);
                }
            }
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        let mut fs = std::mem::take(&mut notes.frames_from);
        fs.retain(|&x| self.rec(x).st() != S_FREE && self.unit_of(x) == u);
        fs.sort_by_key(|&x| self.rec(x).lab());
        fs.dedup();
        for x in fs {
            if self.rec(x).st() == S_FREE {
                continue;
            }
            let mut moved = Vec::new();
            let walked = self.recompute_frames(u, x, &mut moved);
            // Scope-local definitions of the walked entries go under the
            // scopes they open now (their frames may be new).
            let moved_set: crate::FxHashSet<Slot> = if moved.len() > 16 {
                moved.iter().copied().collect()
            } else {
                crate::FxHashSet::default()
            };
            let is_moved = |y: Slot| {
                if moved.len() > 16 {
                    moved_set.contains(&y)
                } else {
                    moved.contains(&y)
                }
            };
            let mut y = x;
            loop {
                if !is_moved(y) && self.scope_misfiled(u, y) {
                    self.refile_defs(u, y, notes);
                }
                if Some(y) == walked || walked.is_none() {
                    break;
                }
                y = self.next(y);
                if y == NIL {
                    break;
                }
            }
            // Definitions whose scope changed: their readers after them.
            for m in moved {
                // Its own reads now see from another scope (unless it has
                // read nothing it keeps).
                if self.has_reads(m) {
                    notes.touch.push((m, u32::MAX, Cause::Structure));
                }
                self.refile_defs(u, m, notes);
            }
            if let Some(last) = walked {
                // The readers in the walked range whose answers may depend
                // on the structure.
                let mut y = x;
                loop {
                    let r = self.rec(y);
                    // Every reader in the range: frames above it may have
                    // moved even where its own frame did not.
                    // (A node that has read nothing yet has nothing to check.)
                    if matches!(r.st(), S_CLEAN | S_STEP | S_PARK | S_DIRTY | S_RUN) && self.has_reads(y) {
                        notes.touch.push((y, u32::MAX, Cause::Structure));
                        self.c.scope_region.fetch_add(1, Ordering::Relaxed);
                    }
                    if y == last {
                        break;
                    }
                    y = self.next(y);
                    if y == NIL {
                        break;
                    }
                }
            }
        }
    }

    /// Whether `s` has a committed or a kept read log.
    fn has_reads(&self, s: Slot) -> bool {
        let r = self.rec(s);
        let st = r.st();
        if st == S_RUN {
            return true;
        }
        let d = r.data();
        if st == S_PARK || (st == S_DIRTY && r.has(F_NEEDS_RUN)) {
            // It runs again anyway; only the reads it kept are checked.
            return d.kept.as_ref().is_some_and(|k| !k.log.is_empty());
        }
        d.res.is_some() || d.kept.as_ref().is_some_and(|k| !k.log.is_empty())
    }

    /// `s` stands: the readers of its predictions that its definitions
    /// did not bear out (kept while it was open) run again.
    pub(crate) fn settle_predictions(&self, s: Slot) {
        let preds = {
            let d = self.rec(s).data();
            match d.stat.preds() {
                Some(p) => p.to_vec(),
                None => return,
            }
        };
        let u = self.unit_of(s);
        let mut touch = Vec::new();
        for (f, p) in preds {
            let mut sh = self.wshard(u, f);
            let Some(idx) = sh.get_mut(&(u, f)) else { continue };
            self.drain(idx);
            let Some(i) = self.def_pos(idx, s) else { continue };
            // Not yet made (its commit's insertion compares then).
            let DefVal::Val(v) = idx.defs[i].val else { continue };
            if v == p {
                continue;
            }
            // Only a reader known to have read the value itself stands (a
            // log not yet stored is checked).
            let users = std::mem::take(&mut idx.defs[i].users);
            for (n, g) in users {
                if self.read_from(n, s, f) == Some(v) {
                    idx.defs[i].users.push((n, g));
                } else {
                    touch.push((n, g, f));
                }
            }
        }
        for (n, g, f) in touch {
            self.touch(n, g, &Cause::Changed { site: None, field: f }, false);
        }
    }

    /// A run on a prediction whose validation broke: the guess failed.
    pub(crate) fn prediction_broke(&self, s: Slot, why: Option<Field>) {
        let r = self.rec(s);
        let pred = r.flags.fetch_and(!super::F_PREDREAD, Ordering::AcqRel) & super::F_PREDREAD != 0;
        if !pred && !r.has(super::F_PROJREAD) {
            return;
        }
        if pred {
            self.c.predictions_refuted.fetch_add(1, Ordering::Relaxed);
        }
        if r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel) & super::F_GUESS != 0 {
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
            self.c.mispredictions.fetch_add(1, Ordering::Relaxed);
            let mut t = self.tal();
            if let Some(f) = why {
                *t.refuted_by_field.entry(f).or_default() += 1;
                self.note_refuted(f);
            }
            *t.refuted_nodes.entry(s).or_default() += 1;
        }
        self.note_rerun(s, crate::api::Rerun::Refuted { field: why });
    }

    /// Records why `s` was made dirty.
    pub(crate) fn note_rerun(&self, s: Slot, why: crate::api::Rerun) {
        super::lock(&self.reruns).push((s, Some(why)));
    }

    /// The public reason for a dirtying cause (`old`: the value the
    /// committed run read, when it read one).
    fn rerun_of(d: &super::Data, cause: &Cause, refuted: bool) -> crate::api::Rerun {
        use crate::api::Rerun;
        match cause {
            Cause::Changed { field, .. } if refuted => Rerun::Refuted { field: Some(*field) },
            Cause::Changed { field, .. } => Rerun::Changed {
                field: *field,
                old: Self::read_value(d, *field).unwrap_or(ValueId::UNIT),
                new: ValueId::UNIT,
            },
            Cause::Rebind { field, .. } if refuted => Rerun::Refuted { field: Some(*field) },
            Cause::Output { .. } if refuted => Rerun::Refuted { field: None },
            Cause::Rebind { field, .. } => Rerun::Rebind { field: *field },
            Cause::Output { of } => Rerun::Output { of: *of },
            Cause::Woken { field, .. } => Rerun::Woken { field: *field },
            Cause::Loop { field } => Rerun::Loop { field: *field },
            Cause::Environment { field } => Rerun::Environment { field: *field },
            Cause::Range { field, .. } => Rerun::Range { field: Some(*field) },
            _ => Rerun::Other,
        }
    }

    /// The value `n`'s committed run read for `f` from `s`.
    fn read_from(&self, n: Slot, s: Slot, f: Field) -> Option<ValueId> {
        let mut d = self.rec(n).data();
        let res = d.res.as_mut()?;
        let read = |p: &super::Pred| match p {
            super::Pred::Def {
                f: g,
                ans:
                    super::DAns::Site {
                        node,
                        val: DefVal::Val(v),
                        ..
                    },
                ..
            } => Some(((*g, *node), *v)),
            _ => None,
        };
        if res.log.len() <= 16 {
            return res.log.iter().filter_map(read).find(|e| e.0 == (f, s)).map(|e| e.1);
        }
        let ix = res.read_ix.get_or_insert_with(|| {
            let mut m = crate::FxHashMap::default();
            for (k, v) in res.log.iter().filter_map(read) {
                // The first read of a field from a site, as in the scan.
                m.entry(k).or_insert(v);
            }
            Box::new(m)
        });
        ix.get(&(f, s)).copied()
    }

    /// Whether every read `n`'s committed run made of `f` from `s` was
    /// projected, and projects `val` equal.
    fn proj_stands(&self, n: Slot, s: Slot, f: Field, val: DefVal) -> bool {
        let DefVal::Val(y) = val else { return false };
        let d = self.rec(n).data();
        let Some(res) = d.res.as_ref() else { return false };
        let mut any = false;
        for p in &res.log {
            if let super::Pred::Def {
                f: g,
                proj,
                ans: super::DAns::Site { node, val: old, .. },
                ..
            } = p
                && *g == f
                && *node == s
            {
                let DefVal::Val(x) = *old else { return false };
                if *proj == 0 || self.project(*proj, x) != self.project(*proj, y) {
                    return false;
                }
                any = true;
            }
        }
        any
    }

    /// The value a node's committed run read for `f`.
    pub(crate) fn read_value(d: &super::Data, f: Field) -> Option<ValueId> {
        let res = d.res.as_ref()?;
        res.log.iter().find_map(|p| match p {
            super::Pred::Def { f: g, ans, .. } if *g == f => match ans {
                super::DAns::Site {
                    val: super::DefVal::Val(v),
                    ..
                }
                | super::DAns::Loop(v) => Some(*v),
                super::DAns::Env(v) | super::DAns::Seq(v) => *v,
                super::DAns::Site { .. } => None,
            },
            _ => None,
        })
    }

    /// Marks a node for validation (or a re-run with `run`).
    pub(crate) fn touch(&self, s: Slot, gn: u32, cause: &Cause, run: bool) {
        let r = self.rec(s);
        // Something it read may have changed: its next check starts over.
        if let Some(e) = r.data().x.as_mut() {
            e.check_from = 0;
        }
        if gn != u32::MAX && gn != super::SPEC_SEQ {
            if r.st() == S_PARK && r.has(F_NEEDS_RUN) {
                // It runs again when woken: only its kept log (under its
                // park sequence) is still read; its committed one is not.
                if r.park_seq.load(Ordering::Acquire) != gn {
                    return;
                }
            } else if r.lgen.load(Ordering::Acquire) != gn
                && !(r.st() == S_PARK && r.park_seq.load(Ordering::Acquire) == gn)
            {
                return;
            }
        }
        let mut refuted = false;
        if r.has(super::F_GUESS)
            && !r.has(super::F_PREDREAD | super::F_PROJREAD)
            && matches!(
                cause,
                Cause::Changed { .. } | Cause::Rebind { .. } | Cause::Output { .. }
            )
            && r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel) & super::F_GUESS != 0
        {
            refuted = true;
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
            self.c.mispredictions.fetch_add(1, Ordering::Relaxed);
            let mut t = self.tal();
            if let Cause::Changed { field: f, .. } = cause {
                *t.refuted_by_field.entry(*f).or_default() += 1;
                self.note_refuted(*f);
            }
            *t.refuted_nodes.entry(s).or_default() += 1;
        }
        if run {
            r.set(F_NEEDS_RUN);
        }
        loop {
            let st = r.st();
            match st {
                S_CLEAN | S_STEP | S_PARK => {
                    // In the lists before it is seen in work: a reader that
                    // finds it busy must also find its pending sets.
                    if st != S_PARK {
                        self.enter_work_lists(s);
                    }
                    if r.status
                        .compare_exchange(st, S_DIRTY, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        let why = {
                            let mut d = r.data();
                            let why = Self::rerun_of(&d, cause, refuted);
                            if d.causes.len() < 8 {
                                d.causes.push(cause.clone());
                            }
                            why
                        };
                        self.note_rerun(s, why);
                        self.enqueue(s);
                        return;
                    }
                }
                S_RUN => {
                    r.set(super::F_STALE);
                    // Against the commit's state store then stale check.
                    std::sync::atomic::fence(Ordering::SeqCst);
                    if r.st() == S_RUN {
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    /// Queues `s` (once).
    pub(crate) fn enqueue(&self, s: Slot) {
        let r = self.rec(s);
        if r.flags.fetch_or(F_QUEUED, Ordering::AcqRel) & F_QUEUED != 0 {
            return;
        }
        self.work.fetch_add(1, Ordering::SeqCst);
        if self.stepped() {
            let mut q = lock(&self.q);
            q.next.push(s);
            self.qlen.fetch_add(1, Ordering::SeqCst);
        } else if !self.push_ready(s) {
            // Kept for this worker: nobody to wake.
            return;
        }
        self.wake_idle(false);
    }

    /// Wakes a sleeping idle worker (all with `all`).
    pub(crate) fn wake_idle(&self, all: bool) {
        if self.sleepers.load(Ordering::SeqCst) == 0 {
            return;
        }
        let _g = lock(&self.idle_mx);
        if all {
            self.idle_cv.notify_all();
        } else {
            self.idle_cv.notify_one();
        }
    }

    /// Dispatches notes outside the structural lock.
    pub(crate) fn dispatch(&self, notes: &mut Notes, me: Slot) {
        for (n, g, c) in std::mem::take(&mut notes.touch) {
            if n == me {
                continue;
            }
            self.touch(n, g, &c, false);
        }
        for n in std::mem::take(&mut notes.enqueue) {
            if matches!(self.rec(n).st(), S_NEW | S_DIRTY) {
                self.enter_work_lists(n);
                self.enqueue(n);
            }
        }
    }

    /// Wakes a parked node if it still waits with sequence `seq`.
    /// A guess of `f` was refuted: past `NO_GUESS_AFTER` of them in this
    /// evaluation, reads of `f` wait for their definer instead (on a
    /// chain whose every link changes, each guess is wrong).
    pub(crate) fn note_refuted(&self, f: Field) {
        let mut m = self.no_guess.write();
        let n = m.entry(f).or_default();
        *n = (*n + 1).min(super::NO_GUESS_MAX);
        if *n >= super::NO_GUESS_AFTER {
            self.any_no_guess.store(true, Ordering::Release);
        }
    }

    /// Whether reads of `f` may still guess in this evaluation.
    pub(crate) fn guess_ok(&self, f: Field) -> bool {
        !self.any_no_guess.load(Ordering::Acquire)
            || self.no_guess.read().get(&f).is_none_or(|&n| n < super::NO_GUESS_AFTER)
    }

    /// `g`'s guesses all held: no longer outstanding.
    pub(crate) fn confirm_guess(&self, g: Slot) {
        let r = self.rec(g);
        if r.flags.fetch_and(!super::F_GUESS, Ordering::AcqRel) & super::F_GUESS != 0 {
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
            self.c.confirmations.fetch_add(1, Ordering::Relaxed);
            if r.flags.fetch_and(!super::F_PREDREAD, Ordering::AcqRel) & super::F_PREDREAD != 0 {
                self.c.predictions_confirmed.fetch_add(1, Ordering::Relaxed);
                // (A commit confirming itself settles after its own
                // definitions.)
                if r.st() != S_RUN {
                    self.settle_predictions(g);
                }
            }
        }
    }

    /// `q` is done (committed, validated, freed or inert): the guesses on
    /// it that its commit did not refute hold.
    pub(crate) fn guesses_settle(&self, q: Slot) {
        let gs = self
            .rec(q)
            .data()
            .x
            .as_mut()
            .map_or_else(Vec::new, |e| std::mem::take(&mut e.guessed_by));
        for g in gs {
            // Its guess on `q` is compared now: a commit of `q` that raced
            // the guesser's registration did not touch it.
            let log = self.rec(g).data().res.as_ref().map(|c| c.log.clone());
            super::query::sib_cursor_reset();
            let moved = log.is_some_and(|l| {
                l.iter().any(|p| {
                    matches!(p, super::Pred::Def { spec: Some(x), .. } if *x == q)
                        && !matches!(self.recheck(g, p, false), Ok(Ok(())))
                })
            });
            if moved {
                self.touch(g, u32::MAX, &Cause::Structure, false);
            }
            let done = {
                let mut d = self.rec(g).data();
                let e = d.ex();
                e.guess_pending = e.guess_pending.saturating_sub(1);
                e.guess_pending == 0
            };
            if done {
                self.confirm_guess(g);
            }
        }
    }

    /// Wakes the nodes parked on `s`, which has just left work for good
    /// (freed or inert): its state is stored first, so a waiter that
    /// registers after this finds it out of work and wakes itself.
    pub(crate) fn wake_waiters(&self, s: Slot) {
        let w = std::mem::take(&mut self.rec(s).data().waiters);
        for (n, q) in w {
            self.wake(n, q);
        }
        self.guesses_settle(s);
    }

    /// Wakes the waiters of a node that is done, except that one waiting
    /// on its claim and now held by another pending claimer moves to that
    /// claimer still parked (a chain of claimers wakes each follower once,
    /// not once per claimer).
    pub(crate) fn wake_waiters_of(&self, waiters: Vec<(Slot, u32)>) {
        for (n, q) in waiters {
            let r = self.rec(n);
            let claim_wait = r.st() == super::S_PARK
                && r.park_seq.load(Ordering::Acquire) == q
                && r.data()
                    .x
                    .as_ref()
                    .and_then(|e| e.wait_on.as_ref())
                    .is_some_and(|w| w.kind == "claim");
            if claim_wait && let Some(c) = self.held_by(n) {
                let cr = self.rec(c);
                if super::in_work(cr.st()) && cr.st() != super::S_STEP {
                    cr.data().waiters.push((n, q));
                    if let Some(w) = r.data().x.as_mut().and_then(|e| e.wait_on.as_mut()) {
                        w.on = super::query::Subj::Node(c);
                    }
                    // Done meanwhile: its waiters were taken before this one.
                    if !super::in_work(cr.st()) || cr.st() == super::S_STEP {
                        self.wake(n, q);
                    }
                    continue;
                }
            }
            self.wake(n, q);
        }
    }

    pub(crate) fn wake(&self, s: Slot, seq: u32) {
        #[cfg(feature = "observe")]
        crate::trace::node(self, s, "wake", "");
        let r = self.rec(s);
        if seq == super::SPEC_SEQ {
            self.touch(s, u32::MAX, &Cause::Structure, false);
            return;
        }
        if r.park_seq.load(Ordering::Acquire) != seq {
            return;
        }
        if r.status
            .compare_exchange(S_PARK, S_DIRTY, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            crate::obs::count!(Wake);
            self.c.raises.fetch_add(1, Ordering::Relaxed);
            self.enqueue(s);
        }
    }

    pub(crate) fn status_name(st: u8) -> &'static str {
        match st {
            S_FREE => "free",
            S_NEW => "new",
            S_DIRTY => "dirty",
            S_RUN => "running",
            S_PARK => "parked",
            S_STEP => "step",
            S_CLEAN => "clean",
            S_INERT => "inert",
            _ => "marker",
        }
    }

    /// The live entries of a unit in order.
    pub(crate) fn entries(&self, u: u32) -> Vec<Slot> {
        let mut v = Vec::new();
        let mut x = self.unit(u).first.load(Ordering::Acquire);
        while x != NIL {
            v.push(x);
            x = self.next(x);
        }
        v
    }

    pub(crate) fn kind_of_frame(&self, f: u32) -> (u8, ValueId, Slot) {
        let fr = self.frame(f);
        (
            fr.kmode.load(Ordering::Acquire),
            ValueId::from_raw(fr.kind.load(Ordering::Acquire)),
            fr.opener.load(Ordering::Acquire),
        )
    }
}
