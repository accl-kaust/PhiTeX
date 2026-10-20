//! Claims: a claimer takes following siblings, which then do not run (they
//! are inert, their facts gone, their unfolding freed), and sibling views:
//! the forward view a claim and `Ctx::following` see, the backward one of
//! `Ctx::prev_sibling_where`, and a stream claim's walk.
//!
//! Who holds what is a structural fact of the world, resolved in text order
//! over a unit's claimers (the earlier claimer wins) whenever the unit's
//! structure changed.

use std::sync::Arc;

use super::build::Builder;
use super::core::Core;
use super::ptree::{NoMeasure, PMap};
use super::world::{Id, Node, Place, St, World, text_key};
use crate::api::{Claim, Until};
use crate::values::{FxHashMap, ValueId};

/// A claimer's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimSt {
    /// What its text's claim hook said when it was registered.
    pub decl: Claim,
    /// A pending claim's reach (`claim_limit`).
    pub limit: Option<usize>,
    /// The claim its last committed run made (it overrides `decl`).
    pub run: Option<usize>,
    /// A stopped run's claim: siblings past it need not wait.
    pub early: Option<usize>,
    /// The siblings it holds.
    pub taken: Arc<[Id]>,
    /// The last position its window reached (what its hooks and its claim
    /// looked at): a change there resolves it again.
    pub reach: super::world::Pos,
}

/// The farthest reach in a subtree of the claim index.
pub(crate) struct MaxReach;

impl super::ptree::Measure<super::world::Pos, (Id, super::world::Pos)> for MaxReach {
    type S = super::world::Pos;
    fn zero(&self) -> super::world::Pos {
        0
    }
    fn of(&self, _: &super::world::Pos, v: &(Id, super::world::Pos)) -> super::world::Pos {
        v.1
    }
    fn add(&self, a: &super::world::Pos, b: &super::world::Pos) -> super::world::Pos {
        (*a).max(*b)
    }
}

impl ClaimSt {
    fn new(decl: Claim) -> ClaimSt {
        ClaimSt {
            decl,
            limit: None,
            run: None,
            early: None,
            taken: Arc::from(Vec::new()),
            reach: 0,
        }
    }
}

impl World {
    /// Whether `x` is a continuation of `me`'s (at any depth of following).
    pub(crate) fn own_cont(&self, me: Id, x: Id) -> bool {
        let mut p = x;
        while let Some(n) = self.node(p) {
            if !matches!(n.place, Place::Cont | Place::After(_)) {
                return false;
            }
            if n.parent == me {
                return true;
            }
            p = n.parent;
        }
        false
    }

    /// Whether `x` was placed (`follow_after`, or as a continuation of
    /// such a node) by a node after `me`.
    fn placed_after_view(&self, me: Id, x: &Node) -> bool {
        let Some(m) = self.node(me) else { return false };
        let mut x = x;
        loop {
            match x.place {
                Place::After(_) => return self.node(x.parent).is_some_and(|p| p.pos > m.pos),
                Place::Cont => match self.node(x.parent) {
                    Some(p) => x = p,
                    None => return false,
                },
                _ => return false,
            }
        }
    }

    /// Whether `x` is taken from `me`: inert, held by a claimer other than
    /// `me` and its continuations.
    pub(crate) fn taken_from(&self, me: Id, x: &Node) -> bool {
        match x.st {
            St::Inert(by) => by != me && !self.own_cont(me, by),
            _ => false,
        }
    }

    /// `me`'s forward view: the following siblings in text order, crossing
    /// up to the ancestors' later siblings, to a scope marker at the level
    /// walked; `taken` says which nodes are taken from `me`.
    pub(crate) fn forward<'w, F: Fn(Id, &Node) -> bool + 'w>(&'w self, me: Id, taken: F) -> Forward<'w, F> {
        let d = self.node(me).map_or(0, |n| n.depth);
        Forward {
            w: self,
            me,
            cur: self.subtree_end(me),
            d,
            taken,
            done: false,
        }
    }

    /// `me`'s backward view: earlier siblings at its depth, nearest first,
    /// to its parent or a scope marker; each with whether it is inert.
    pub(crate) fn backward(&self, me: Id) -> Backward<'_> {
        let d = self.node(me).map_or(0, |n| n.depth);
        Backward {
            w: self,
            cur: me,
            d,
            done: false,
        }
    }

    /// The texts of `me`'s forward view, as the claim hooks take them.
    pub(crate) fn texts<'w, F: Fn(Id, &Node) -> bool + 'w>(
        &'w self,
        me: Id,
        taken: F,
    ) -> impl Iterator<Item = (crate::api::OpId, Vec<ValueId>)> + 'w {
        self.forward(me, taken).take(4096).filter_map(move |x| {
            let n = self.node(x)?;
            Some((n.text.op, n.text.args.to_vec()))
        })
    }

    /// A pending claimer `c` in work that holds `s` back: an earlier claimer
    /// of `s`'s unit whose claim is still to be decided, `s` within its
    /// reach and not ending it.
    pub(crate) fn held_by(&self, core: &Core, s: Id) -> Option<Id> {
        let sn = self.node(s)?;
        if self.claimers.get(&sn.unit).is_none_or(|m| m.is_empty()) {
            return None;
        }
        let hs = self.holders.get(&sn.unit)?;
        // The nearest earlier holder that holds `s` back (it re-checks when
        // that one finishes).
        let mut k = sn.pos;
        while let Some((&p, &c)) = hs.below(&k) {
            k = p;
            if self.holds_back(core, c, s, sn) {
                return Some(c);
            }
        }
        None
    }

    /// Whether claimer `c` (in work) holds `s` back.
    fn holds_back(&self, core: &Core, c: Id, s: Id, sn: &Node) -> bool {
        let Some(st) = self.claims.get(&c) else { return false };
        if st.decl != Claim::Pending || st.run.is_some() {
            return false;
        }
        let Some(cn) = self.node(c) else { return false };
        // `s`'s parent is an ancestor-or-self of the claimer's parent.
        let mut p = cn.parent;
        let mut under = p == sn.parent;
        while !under && p != super::world::NONE {
            p = self.node(p).map_or(super::world::NONE, |n| n.parent);
            under = p == sn.parent;
        }
        if !under {
            return false;
        }
        let o = &core.ops[cn.text.op.0 as usize];
        let until = core.hook(Some(c), cn.text.op, "until", || Until::No, || {
            o.until(&cn.text.args, &core.values, sn.text.op, &sn.text.args)
        });
        if until == Until::Yes {
            return false;
        }
        match st.early.or(st.limit) {
            Some(0) => false,
            Some(k) => self.forward(c, |_, x| self.taken_from(c, x)).take(k).any(|x| x == s),
            None => true,
        }
    }

    /// A stream claim of `c`: the outputs of the following leaves up to the
    /// one its `until` hooks end it before. `Err(x)`: leaf `x` is in work.
    #[allow(clippy::type_complexity)]
    pub(crate) fn stream(&self, core: &Core, c: Id) -> Result<(Vec<(Id, ValueId)>, Option<Id>, Vec<(crate::values::Field, Id)>), Id> {
        let Some(cn) = self.node(c) else { return Ok((Vec::new(), None, Vec::new())) };
        let mut fields = Vec::new();
        let d = cn.depth;
        let o = &core.ops[cn.text.op.0 as usize];
        let v = &core.values;
        let mut outs = Vec::new();
        let mut cur = self.subtree_end(c);
        while let Some(x) = self.next(cur) {
            let Some(n) = self.node(x) else { break };
            if n.depth < d || (n.depth == d && matches!(n.place, Place::Marker(_))) {
                break;
            }
            if matches!(n.place, Place::Marker(_)) || self.taken_from(c, n) || self.placed_after_view(c, n) {
                cur = self.subtree_end(x);
                continue;
            }
            cur = x;
            let until = core.hook(Some(c), cn.text.op, "until", || Until::No, || {
                o.until(&cn.text.args, v, n.text.op, &n.text.args)
            });
            if super::world::in_work(n.st) {
                if until == Until::Yes {
                    return Ok((outs, Some(x), fields));
                }
                return Err(x);
            }
            if n.res.as_ref().is_some_and(|r| !r.kids.is_empty()) {
                // Passed over: its descendants are candidates.
                continue;
            }
            let ends = match until {
                Until::No => false,
                Until::Yes => true,
                Until::Read(f) => {
                    fields.push((f, x));
                    let val = self.value_before(core, x, f).map_err(|_| x)?;
                    core.hook(Some(c), cn.text.op, "until_value", || false, || {
                        o.until_value(&cn.text.args, v, n.text.op, &n.text.args, val)
                    })
                }
                Until::ReadEach(fs) => {
                    let mut vals = Vec::with_capacity(fs.len());
                    for f in fs {
                        fields.push((f, x));
                        vals.push(self.value_before(core, x, f).map_err(|_| x)?);
                    }
                    core.hook(Some(c), cn.text.op, "until_values", || false, || {
                        o.until_values(&cn.text.args, v, n.text.op, &n.text.args, &vals)
                    })
                }
            };
            if ends {
                return Ok((outs, Some(x), fields));
            }
            outs.push((x, n.out().unwrap_or(ValueId::UNIT)));
        }
        Ok((outs, None, fields))
    }

    /// The value of `f` reaching just before node `x` (`Err`: its definer
    /// is in work).
    pub(crate) fn value_before(&self, core: &Core, x: Id, f: crate::values::Field) -> Result<Option<ValueId>, Id> {
        let Some((u, pos, fr)) = self.frame_at(x, false) else { return Ok(None) };
        match self.reaching(u, f, pos, fr, x) {
            Some((_, e)) => match e.val {
                Some(v) => Ok(Some(v)),
                None => Err(e.node),
            },
            None => Ok(self.env_of(core, u, f)),
        }
    }
}

/// The lowest position a backward window reaches, in a subtree.
pub(crate) struct MinLow;

impl super::ptree::Measure<super::world::Pos, (Id, super::world::Pos)> for MinLow {
    type S = super::world::Pos;
    fn zero(&self) -> super::world::Pos {
        super::world::Pos::MAX
    }
    fn of(&self, _: &super::world::Pos, v: &(Id, super::world::Pos)) -> super::world::Pos {
        v.1
    }
    fn add(&self, a: &super::world::Pos, b: &super::world::Pos) -> super::world::Pos {
        (*a).min(*b)
    }
}

/// A window index: entries by position, each with how far it reaches.
pub(crate) type Windows = PMap<super::world::Pos, (Id, super::world::Pos), super::world::Pos>;

/// The entries of a forward window index before `p` whose reach is at
/// least `p`, by position.
pub(crate) fn stab_forward(m: &Windows, p: super::world::Pos) -> Vec<(super::world::Pos, Id)> {
    use std::ops::Bound::{Excluded, Included, Unbounded};
    let mut out = Vec::new();
    let mut hi = p;
    while m.fold(Unbounded, Excluded(&hi), &MaxReach) >= p {
        let (mut lo, mut top) = (0usize, m.rank(&hi).saturating_sub(1));
        while lo < top {
            let mid = (lo + top).div_ceil(2);
            let k = *m.nth(mid).expect("ranked").0;
            if m.fold(Included(&k), Excluded(&hi), &MaxReach) >= p {
                lo = mid;
            } else {
                top = mid - 1;
            }
        }
        let (&k, &(c, reach)) = m.nth(lo).expect("ranked");
        if reach >= p {
            out.push((k, c));
        }
        hi = k;
    }
    out.reverse();
    out
}

/// The entries of a backward window index after `p` whose low end is at
/// most `p`, by position.
pub(crate) fn stab_backward(m: &Windows, p: super::world::Pos) -> Vec<(super::world::Pos, Id)> {
    use std::ops::Bound::{Excluded, Included, Unbounded};
    let mut out = Vec::new();
    let mut lo = p;
    while m.fold(Excluded(&lo), Unbounded, &MinLow) <= p {
        let n = m.len();
        let (mut a, mut b) = (m.rank(&lo) + usize::from(m.contains_key(&lo)), n - 1);
        while a < b {
            let mid = (a + b) / 2;
            let k = *m.nth(mid).expect("ranked").0;
            if m.fold(Excluded(&lo), Included(&k), &MinLow) <= p {
                b = mid;
            } else {
                a = mid + 1;
            }
        }
        let (&k, &(c, low)) = m.nth(a).expect("ranked");
        if low <= p {
            out.push((k, c));
        }
        lo = k;
    }
    out
}

impl World {
    /// The claimers of unit `u` before position `p` whose window reaches
    /// `p`, by position.
    pub(crate) fn stab(&self, u: u32, p: super::world::Pos) -> Vec<(super::world::Pos, Id)> {
        let Some(m) = self.claim_at.get(&u) else { return Vec::new() };
        stab_forward(m, p)
    }
}

/// A forward sibling walk.
pub(crate) struct Forward<'w, F> {
    w: &'w World,
    me: Id,
    cur: Id,
    d: u32,
    taken: F,
    done: bool,
}

impl<F: Fn(Id, &Node) -> bool> Iterator for Forward<'_, F> {
    type Item = Id;

    fn next(&mut self) -> Option<Id> {
        while !self.done {
            let Some(x) = self.w.next(self.cur) else {
                self.done = true;
                break;
            };
            let n = self.w.node(x)?;
            if n.depth > self.d {
                // Inside a subtree already passed (cannot be: skipped whole).
                self.cur = x;
                continue;
            }
            self.d = n.depth;
            if matches!(n.place, Place::Marker(_)) {
                self.done = true;
                break;
            }
            self.cur = self.w.subtree_end(x);
            if self.w.own_cont(self.me, x) || self.w.placed_after_view(self.me, n) || (self.taken)(x, n) {
                continue;
            }
            return Some(x);
        }
        None
    }
}

/// A backward sibling walk: (node, inert).
pub(crate) struct Backward<'w> {
    w: &'w World,
    cur: Id,
    d: u32,
    done: bool,
}

impl Iterator for Backward<'_> {
    type Item = (Id, bool);

    fn next(&mut self) -> Option<(Id, bool)> {
        while !self.done {
            let Some(x) = self.w.prev(self.cur) else {
                self.done = true;
                break;
            };
            self.cur = x;
            let n = self.w.node(x)?;
            if n.depth > self.d {
                continue;
            }
            if n.depth < self.d || matches!(n.place, Place::Marker(_)) {
                self.done = true;
                break;
            }
            return Some((x, matches!(n.st, St::Inert(_))));
        }
        None
    }
}

impl Builder<'_> {
    /// Asks the claim hooks of the nodes registered since (their texts are
    /// new) and resolves the claims of the units whose structure changed.
    pub(crate) fn settle_claims(&mut self) {
        for _ in 0..64 {
            let todo: Vec<Id> = self.w.unregistered.iter().map(|(&id, ())| id).collect();
            self.w.unregistered = Default::default();
            for id in todo {
                self.register_claim(id);
            }
            if self.w.claims_dirty.is_empty() && self.w.claim_points.is_empty() && self.w.claims_todo.is_empty() {
                return;
            }
            let full: Vec<u32> = self.w.claims_dirty.iter().map(|(&u, ())| u).collect();
            let points: Vec<(u32, super::world::Pos)> = self.w.claim_points.iter().map(|(&k, ())| k).collect();
            let todo: Vec<Id> = self.w.claims_todo.iter().map(|(&id, ())| id).collect();
            self.w.claims_dirty = Default::default();
            self.w.claim_points = Default::default();
            self.w.claims_todo = Default::default();
            let mut queues: FxHashMap<u32, std::collections::BTreeSet<(super::world::Pos, Id)>> = FxHashMap::default();
            for u in full {
                let q = queues.entry(u).or_default();
                if let Some(m) = self.w.claim_at.get(&u) {
                    q.extend(m.iter().map(|(&p, &(c, _))| (p, c)));
                }
            }
            for (u, p) in points {
                let hits = self.w.stab(u, p);
                queues.entry(u).or_default().extend(hits);
            }
            for c in todo {
                if let Some(n) = self.w.node(c) {
                    queues.entry(n.unit).or_default().insert((n.pos, c));
                }
            }
            let mut units: Vec<u32> = queues.keys().copied().collect();
            units.sort_unstable();
            for u in units {
                let q = queues.remove(&u).unwrap_or_default();
                self.resolve_claims(u, q);
            }
        }
    }

    /// A change at position `p` of unit `u` that claims reaching it see.
    pub(crate) fn claim_point(&mut self, u: u32, p: super::world::Pos) {
        self.w.claim_points.insert((u, p), (), &NoMeasure);
    }

    /// Asks node `id`'s claim hook (and a pending claim's limit).
    fn register_claim(&mut self, id: Id) {
        let Some(n) = self.w.node(id).cloned() else { return };
        if matches!(n.st, St::Inert(_)) {
            return;
        }
        let core = self.core;
        let o = &core.ops[n.text.op.0 as usize];
        let w = &self.w;
        let decl = core.hook(Some(id), n.text.op, "claim", || Claim::None, || {
            o.claim(&n.text.args, &core.values, &mut w.texts(id, |_, x| w.taken_from(id, x)))
        });
        let limit = if decl == Claim::Pending {
            core.hook(Some(id), n.text.op, "claim_limit", || None, || {
                o.claim_limit(&n.text.args, &core.values, &mut w.texts(id, |_, x| w.taken_from(id, x)))
            })
        } else {
            None
        };
        let old = self.w.claims.get(&id).cloned();
        let run = n.res.as_ref().and_then(|r| r.claim);
        if decl == Claim::None && run.is_none() && old.as_ref().is_none_or(|o| o.taken.is_empty()) {
            if old.is_some() {
                self.w.claims.remove(&id, &NoMeasure);
        self.w.refresh_holder(id);
        self.w.refresh_claim_at(id);
                self.unindex_claimer(n.unit, id);
            }
            return;
        }
        let mut st = old.unwrap_or_else(|| ClaimSt::new(decl));
        st.decl = decl;
        st.limit = limit;
        st.run = run;
        self.w.claims.insert(id, st, &NoMeasure);
        self.w.refresh_holder(id);
        self.w.refresh_claim_at(id);
        let mut m = self.w.claimers.get(&n.unit).cloned().unwrap_or_default();
        m.insert(id, (), &NoMeasure);
        self.w.claimers.insert(n.unit, m, &NoMeasure);
        self.w.claims_todo.insert(id, (), &NoMeasure);
    }

    fn unindex_claimer(&mut self, u: u32, id: Id) {
        if let Some(mut m) = self.w.claimers.get(&u).cloned() {
            m.remove(&id, &NoMeasure);
            self.w.claimers.insert(u, m, &NoMeasure);
        }
    }

    /// A claimer's run claimed `n` (`None`: made no claim).
    pub(crate) fn run_claimed(&mut self, id: Id, n: Option<usize>) {
        let Some(node) = self.w.node(id) else { return };
        let u = node.unit;
        let mut st = match self.w.claims.get(&id) {
            Some(s) => s.clone(),
            None if n.is_none() => return,
            None => ClaimSt::new(Claim::None),
        };
        st.early = None;
        if st.run == n && self.w.claims.contains_key(&id) {
            self.w.claims.insert(id, st, &NoMeasure);
        self.w.refresh_holder(id);
        self.w.refresh_claim_at(id);
            return;
        }
        if st.decl != Claim::Pending
            && let Some(k) = n
            && st.decl != Claim::Count(k)
            && !(k == 0 && st.decl == Claim::None)
        {
            self.counts.claims_refuted += 1;
            self.counts.claim_siblings += k as u64;
        }
        st.run = n;
        self.w.claims.insert(id, st, &NoMeasure);
        self.w.refresh_holder(id);
        self.w.refresh_claim_at(id);
        let mut m = self.w.claimers.get(&u).cloned().unwrap_or_default();
        m.insert(id, (), &NoMeasure);
        self.w.claimers.insert(u, m, &NoMeasure);
        self.w.claims_todo.insert(id, (), &NoMeasure);
    }

    /// Resolves the claims of unit `u` queued in `q`, in text order: each
    /// claimer not itself taken takes its count of visible following
    /// siblings (a node a later claimer holds is visible to it: the earlier
    /// wins); nodes taken become inert, nodes no longer taken are released.
    /// What that changes is a change the later claimers reaching it see.
    fn resolve_claims(&mut self, u: u32, mut q: std::collections::BTreeSet<(super::world::Pos, Id)>) {
        while let Some((_, c)) = q.pop_first() {
            let Some(cn) = self.w.node(c).cloned() else { continue };
            let Some(st) = self.w.claims.get(&c).cloned() else { continue };
            let core = self.core;
            let w = &self.w;
            // Taken from `c`: held by an earlier claimer (not its own
            // continuations).
            let taken = |x: &Node| match x.st {
                St::Inert(by) => by != c && !w.own_cont(c, by) && w.node(by).is_some_and(|b| b.pos < cn.pos),
                _ => false,
            };
            let last = std::cell::Cell::new(c);
            let (want, reach): (Vec<Id>, super::world::Pos) = if matches!(cn.st, St::Inert(_)) {
                (Vec::new(), cn.pos)
            } else {
                let k = match (st.run, st.decl) {
                    (Some(n), _) => n,
                    (None, Claim::Count(_)) => {
                        let o = &core.ops[cn.text.op.0 as usize];
                        let mut it = w.forward(c, |_, x| taken(x)).take(4096).filter_map(|x| {
                            last.set(x);
                            let n = w.node(x)?;
                            Some((n.text.op, n.text.args.to_vec()))
                        });
                        match core.hook(Some(c), cn.text.op, "claim", || Claim::None, || o.claim(&cn.text.args, &core.values, &mut it)) {
                            Claim::Count(n) => n,
                            _ => 0,
                        }
                    }
                    _ => 0,
                };
                let want: Vec<Id> = if k == 0 { Vec::new() } else { w.forward(c, |_, x| taken(x)).take(k).collect() };
                // The window: the farthest node looked at, with its subtree.
                let far = want.last().copied().into_iter().chain([last.get()]).filter_map(|x| w.node(w.subtree_end(x))).map(|n| n.pos).max();
                // (One past it: an insert right after the window reaches it.)
                let reach = far.unwrap_or(cn.pos).max(cn.pos);
                let reach = w.unit(u).order.above(&reach).map_or(super::world::Pos::MAX, |(&p, _)| p);
                (want, reach)
            };
            let old: Vec<Id> = st.taken.to_vec();
            if old != want || st.reach != reach {
                let mut st2 = st.clone();
                st2.taken = Arc::from(want.clone());
                st2.reach = reach;
                self.w.claims.insert(c, st2, &NoMeasure);
                self.w.refresh_holder(c);
                self.w.refresh_claim_at(c);
            }
            let mut toggled: Vec<super::world::Pos> = Vec::new();
            for &x in &old {
                if !want.contains(&x) && self.w.node(x).is_some_and(|n| n.st == St::Inert(c)) {
                    toggled.push(self.w.node(x).map_or(0, |n| n.pos));
                    self.release(x);
                }
            }
            for &x in &want {
                let Some(xn) = self.w.node(x) else { continue };
                if xn.st == St::Inert(c) {
                    continue;
                }
                toggled.push(xn.pos);
                if let St::Inert(d) = xn.st {
                    // Taken over from a later claimer: it resolves again.
                    if let Some(dn) = self.w.node(d) {
                        q.insert((dn.pos, d));
                    }
                }
                self.make_inert(x, c);
            }
            // Later claimers whose windows the changes reach.
            for p in toggled {
                for (cp, d) in self.w.stab(u, p) {
                    if cp > cn.pos {
                        q.insert((cp, d));
                    }
                }
                // (The changed node itself may be a claimer.)
            }
        }
    }

    /// Makes `x` inert, held by `c`: it does not run; its facts go, its
    /// unfolding is freed.
    pub(crate) fn make_inert(&mut self, x: Id, c: Id) {
        let Some(n) = self.w.node(x).cloned() else { return };
        let old_b = n.brackets();
        if let Some(res) = n.res.clone() {
            for &k in res.kids.iter().chain(&res.conts) {
                if self.w.node(k).is_some_and(|m| m.parent == x) {
                    self.free(k);
                }
            }
            self.drop_reads(x, &n, &res.reads);
        }
        // (Freeing its unfolding may have moved its frame: as it is now.)
        let n = self.w.node(x).cloned().unwrap_or(n);
        self.unfile_all(x, &n);
        // Its own claims go with it.
        if let Some(st) = self.w.claims.get(&x).cloned() {
            for &t in st.taken.iter() {
                if self.w.node(t).is_some_and(|m| m.st == St::Inert(x)) {
                    self.release(t);
                }
            }
            let mut st = st;
            st.taken = Arc::from(Vec::new());
            self.w.claims.insert(x, st, &NoMeasure);
        self.w.refresh_holder(x);
        self.w.refresh_claim_at(x);
        }
        let mut m = self.w.node(x).cloned().expect("live");
        m.st = St::Inert(c);
        m.res = None;
        self.w.put(x, m);
        self.out_gone(x);
        if old_b != crate::api::Brackets::default() {
            self.frames_from(n.unit, n.pos);
        }
        self.shape_at(n.unit, n.pos);
        self.woken.push(x);
    }

    /// Releases inert node `x`: it is new again and runs.
    pub(crate) fn release(&mut self, x: Id) {
        let Some(n) = self.w.node(x).cloned() else { return };
        let mut m = n.clone();
        m.st = if m.text.stat.fin { St::Held } else { St::Dirty };
        m.res = None;
        self.w.put(x, m.clone());
        if m.st == St::Dirty {
            self.dirty.push(x);
        }
        if let Some(cs) = m.text.stat.contribs.clone() {
            for &(f, v) in cs.iter() {
                self.contrib(m.unit, f, m.pos, Some(v));
            }
        }
        self.file_declared_pub(x);
        if m.brackets() != crate::api::Brackets::default() {
            self.frames_from(m.unit, m.pos);
        }
        self.w.unregistered.insert(x, (), &NoMeasure);
        self.shape_at(m.unit, m.pos);
    }

    /// A claimer `c` freed: the siblings it held are released.
    pub(crate) fn claimer_gone(&mut self, c: Id, u: u32) {
        if let Some(st) = self.w.claims.get(&c).cloned() {
            self.w.claims.remove(&c, &NoMeasure);
        self.w.refresh_holder(c);
        self.w.refresh_claim_at(c);
            self.unindex_claimer(u, c);
            for &t in st.taken.iter() {
                if self.w.node(t).is_some_and(|m| m.st == St::Inert(c)) {
                    self.release(t);
                }
            }
        }
        self.w.unregistered.remove(&c, &NoMeasure);
    }
}

/// The text key of node `x` in `w` (0: gone).
pub(crate) fn key_of(w: &World, x: Id) -> usize {
    w.node(x).map_or(0, |n| text_key(&n.text))
}
