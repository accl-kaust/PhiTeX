//! Making a new world from an old one: node creation and removal, placement,
//! frames, definition filing, a run's result applied, and the readers whose
//! answers changed marked dirty.

use std::sync::Arc;

use super::core::Core;
use super::ctx::Spec;
use super::live::Slot;
use super::ptree::{Count, NoMeasure, PMap};
use super::world::{ReadKey, 
    AccMeasure, Def, Entry, Frame, FrameId, Id, Key, NONE, Node, Place, Pos, Read, Res, St, Text, Unit, World,
};
use crate::api::Locality;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, hash_words};

/// The label gap between two pushed nodes.
pub(crate) const PUSH_GAP: Pos = 1 << 40;
/// The most a position placed between two others is past the first.
const STEP_GAP: Pos = 1 << 30;

/// What a builder made.
pub(crate) struct Made {
    pub w: World,
    pub dirty: Vec<Id>,
    pub created: u64,
    pub freed: Vec<Id>,
    pub woken: Vec<Id>,
    pub rebinds: u64,
    /// What runs refuted of their texts' declarations.
    pub counts: Counts,
    /// The field writes still recorded (claims settled with them).
    pub defer: Option<super::batch::Pending>,
}

/// Declarations runs refuted ([`crate::Report`]'s fields of these names).
#[derive(Default, Clone, Copy)]
pub(crate) struct Counts {
    pub claims_refuted: u64,
    pub claims_past_reach: u64,
    pub claimed_after_run: u64,
    pub effects_refuted: u64,
    pub claim_siblings: u64,
    pub defs_refuted: u64,
    pub declarations_refuted: u64,
    pub static_contribs_refuted: u64,
    /// Its commits' time by phase (the merge's, `MergeTimes`).
    pub merge: crate::api::MergeTimes,
}

impl Counts {
    pub(crate) fn add_to(&self, r: &mut crate::api::Report) {
        r.claims_refuted += self.claims_refuted;
        r.claims_past_reach += self.claims_past_reach;
        r.claimed_after_run += self.claimed_after_run;
        r.effects_refuted += self.effects_refuted;
        r.claim_siblings += self.claim_siblings;
        r.defs_refuted += self.defs_refuted;
        r.declarations_refuted += self.declarations_refuted;
        r.static_contribs_refuted += self.static_contribs_refuted;
    }
}

/// A world being made, and what the making found.
pub(crate) struct Builder<'a> {
    pub core: &'a Core,
    pub w: World,
    /// Nodes made dirty (to queue).
    pub dirty: Vec<Id>,
    /// Nodes created and freed.
    pub created: u64,
    pub freed: Vec<Id>,
    /// Nodes whose runs it should wake (their subject committed or went).
    pub woken: Vec<Id>,
    /// Readers to re-ask once the change is complete: all their reads
    /// (`None`), or only those of the fields whose definitions or
    /// contributions changed.
    recheck: FxHashMap<Id, Option<Vec<Field>>>,
    /// Of those, the readers re-asked on their reads of structure too (the
    /// structure in a window they read changed).
    shaped: FxHashSet<Id>,
    /// Of those, the readers re-asked on their reads of these nodes'
    /// outputs.
    out_on: FxHashMap<Id, Vec<Id>>,
    /// Auditing (`Core::rechecks` on): why each reader re-asked on all its
    /// reads was.
    all_why: Option<FxHashMap<Id, u8>>,
    /// Readers past a shadow bound, asked under CLEAN_SCOPED_CHECK only.
    beyond: Vec<(Id, Field)>,
    /// Readers made dirty because a use now reaches another site.
    pub rebinds: u64,
    /// Claims wait for the evaluation (a client's edits).
    pub defer_claims: bool,
    /// A step's merge: the field writes recorded, not made
    /// ([`super::batch`]).
    pub defer: Option<super::batch::Pending>,
    /// While a run's placements are made: the unit, the positions whose
    /// frames are to be walked again (each: a walk stops where the frames
    /// agree again, short of a placement further on), and the nodes whose
    /// declared definitions are to be filed once they are.
    hold: Option<(u32, Vec<Pos>, Vec<Id>)>,
    pub counts: Counts,
    /// The run committed continued from its kept state (`Ctx::base`): its
    /// placements go after the children and continuations its earlier
    /// attempts made (the first this many of those committed), which
    /// stand.
    pub cont: Option<(usize, usize)>,
    /// A continuing placer's placements as its last stop left them
    /// (`ContMemo`): taken by its next stop's commit, made again by it.
    pub cont_memo: Option<ContMemo>,
}

/// The ordinal of a placement among its placer's of its kind: by place,
/// op and (after a node) anchor.
type PlaceKey = (u64, crate::api::OpId, u64);

/// What a continuing placer's stop committed, for its next stop: its result
/// then (by address: unchanged since, its placements are as left), how
/// many placements its attempts had made, all live, and the ordinals they
/// took. The next stop places only its new ones, not walking the earlier.
pub(crate) struct ContMemo {
    res: usize,
    bk: usize,
    bc: usize,
    ord_k: FxHashMap<PlaceKey, u32>,
    ord_c: FxHashMap<PlaceKey, u32>,
}

/// Whether CLEAN_KEPT_CHECK asks a continuing placer's stop to place as
/// without its memo too, and compare.
fn memo_check() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("CLEAN_KEPT_CHECK").is_ok_and(|v| v == "1"))
}

impl<'a> Builder<'a> {
    pub(crate) fn new(core: &'a Core, w: World) -> Builder<'a> {
        let mut w = w;
        // (A version no world had: one kept apart never shares it.)
        w.ver = super::live::fresh_epoch();
        Builder {
            core,
            w,
            dirty: Vec::new(),
            created: 0,
            freed: Vec::new(),
            woken: Vec::new(),
            recheck: FxHashMap::default(),
            shaped: FxHashSet::default(),
            out_on: FxHashMap::default(),
            all_why: core.auditing.load(std::sync::atomic::Ordering::Relaxed).then(FxHashMap::default),
            beyond: Vec::new(),
            rebinds: 0,
            defer_claims: false,
            defer: None,
            hold: None,
            counts: Counts::default(),
            cont: None,
            cont_memo: None,
        }
    }

    /// Readers to re-ask (a flush's), each on a field.
    pub(crate) fn recheck_all(&mut self, ids: impl IntoIterator<Item = (Id, Field, bool)>) {
        for (r, f, beyond) in ids {
            if beyond {
                self.beyond.push((r, f));
            } else {
                self.recheck_on(r, f);
            }
        }
    }

    /// Reader `r` re-asked on all its reads.
    fn recheck(&mut self, r: Id, src: usize) {
        super::core::prof::count(src);
        if let Some(m) = &mut self.all_why {
            m.insert(r, u8::try_from(src).unwrap_or(u8::MAX));
        }
        self.recheck.insert(r, None);
    }

    /// Reader `r` re-asked on its reads of structure and outputs
    /// (`structural`): the structure in a window it reads changed (`why`
    /// 14), or an output it reads (18). (Its uses of fields are re-asked
    /// through the definitions that come or go.)
    fn recheck_shape(&mut self, r: Id, why: u8) {
        super::core::prof::count(usize::from(why));
        if let Some(m) = &mut self.all_why {
            m.insert(r, why);
        }
        self.recheck.entry(r).or_insert_with(|| Some(Vec::new()));
        self.shaped.insert(r);
    }

    /// Reader `r` re-asked on its reads of node `x`'s output (it changed,
    /// or its text: why 18).
    fn recheck_out(&mut self, r: Id, x: Id) {
        super::core::prof::count(18);
        if let Some(m) = &mut self.all_why {
            m.insert(r, 18);
        }
        self.recheck.entry(r).or_insert_with(|| Some(Vec::new()));
        self.out_on.entry(r).or_default().push(x);
    }

    /// Reader `r` re-asked on its reads of `f` (a definition or a
    /// contribution of `f` changed: no other read can break).
    fn recheck_on(&mut self, r: Id, f: Field) {
        super::core::prof::count(19);
        match self.recheck.entry(r) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if let Some(fs) = e.get_mut()
                    && !fs.contains(&f)
                {
                    fs.push(f);
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Some(vec![f]));
            }
        }
    }

    /// The world made, with the readers whose reads broke marked dirty.
    pub(crate) fn finish(mut self) -> Made {
        let t = super::core::now();
        if !self.defer_claims {
            super::core::prof::time(2, || self.settle_claims());
        }
        super::core::prof::time(3, || self.settle());
        self.counts.merge.dependents += super::core::ns_since(t);
        Made {
            w: self.w,
            dirty: self.dirty,
            created: self.created,
            freed: self.freed,
            woken: self.woken,
            rebinds: self.rebinds,
            counts: self.counts,
            defer: self.defer,
        }
    }

    /// Re-asks the reads of the readers queued for it; a broken one makes
    /// its reader dirty.
    fn settle(&mut self) {
        for (id, f) in std::mem::take(&mut self.beyond) {
            // (Re-asked on `f` anyway, by another change: not a miss.)
            if self.recheck.get(&id).is_some_and(|on| on.as_ref().is_none_or(|fs| fs.contains(&f))) {
                continue;
            }
            let Some(n) = self.w.node(id) else { continue };
            if n.st != St::Done {
                continue;
            }
            let Some(res) = n.res.clone() else { continue };
            if let Some(r) = res.reads.iter().find(|r| read_on(r, &[f]) && !holds(self.core, &self.w, id, r)) {
                eprintln!("shadow check: {id} past the bound on {f:?}, broken {r:?}");
                self.recheck_on(id, f);
            }
        }
        // (Rounds: a value passed on refiled re-asks its readers.)
        while !self.recheck.is_empty() {
            self.settle_round();
        }
    }

    fn settle_round(&mut self) {
        let ids: Vec<(Id, Option<Vec<Field>>)> = std::mem::take(&mut self.recheck).into_iter().collect();
        let shaped = std::mem::take(&mut self.shaped);
        let mut out_on = std::mem::take(&mut self.out_on);
        for (id, on) in ids {
            let shape = on.is_some() && shaped.contains(&id);
            let outs: Vec<Id> = if on.is_some() { out_on.remove(&id).unwrap_or_default() } else { Vec::new() };
            let Some(n) = self.w.node(id) else { continue };
            if n.st != St::Done {
                continue;
            }
            let Some(res) = n.res.clone() else { continue };
            if super::core::settle_stats() {
                super::core::settle_note(self.core.op_name(n.text.op), res.reads.len(), on.as_ref().map_or(0, Vec::len));
            }
            // (Many fields asked on, a finisher's: looked up in a set, not
            // each read against the whole list.)
            let on_set: Option<FxHashSet<Field>> = on.as_ref().filter(|fs| fs.len() > 8).map(|fs| fs.iter().copied().collect());
            let asked = |r: &Read| {
                let a = on.as_ref().is_none_or(|fs| match &on_set {
                    Some(set) => read_on_set(r, set),
                    None => read_on(r, fs),
                }) || (shape && structural(r)) || (!outs.is_empty() && read_of(r, &outs));
                if a {
                    super::core::prof::count(20);
                }
                a
            };
            let mut a = Asker::default();
            let check = memo_check();
            let mut audit = super::core::lock(&self.core.rechecks);
            let mut n_asked = 0u64;
            let mut broken = res.reads.iter().find(|r| {
                asked(r) && {
                    n_asked += 1;
                    let ok = holds_asked(self.core, &self.w, id, r, &mut a);
                    assert!(!check || ok == holds(self.core, &self.w, id, r), "reader {id}: its read {r:?} asked with memo: {ok}");
                    if let Some(m) = audit.as_mut() {
                        let e = m.entry(read_field(r)).or_default();
                        e.0 += 1;
                        e.1 += u64::from(!ok);
                    }
                    !ok
                }
            });
            if audit.is_some()
                && (on.is_none() || shape || !outs.is_empty())
            {
                let why = self.all_why.as_ref().and_then(|m| m.get(&id)).copied().unwrap_or(0);
                let mut m = super::core::lock(&self.core.rechecks_all);
                let e = m.entry(why).or_default();
                e.0 += 1;
                e.1 += n_asked;
            }
            drop(audit);
            // (CLEAN_SCOPED_CHECK: a read of another field broken too is
            // an invalidation missed elsewhere; named, and taken.)
            if broken.is_none() && on.is_some() && scoped_check() {
                broken = res.reads.iter().find(|r| !holds(self.core, &self.w, id, r));
                if let Some(r) = broken {
                    eprintln!("scoped check: {id} asked on {on:?}, broken {r:?}");
                }
            }
            // (A run that marked points resumes at the last before its
            // first broken read.)
            if let Some(r) = broken
                && !res.marks.is_empty()
                && res.after.is_some()
                && let Some(b) = res.reads.iter().position(|x| std::ptr::eq(x, r))
            {
                let b = u32::try_from(b).unwrap_or(u32::MAX);
                let at = self.w.log_len;
                let mut m = super::core::lock(&self.core.resume_at);
                let e = m.entry(id).or_insert((b, at));
                *e = (e.0.min(b), e.1.min(at));
            }
            if let Some(r) = broken {
                if super::core::debug()
                    && let Read::Use { f, at, after, own, site, .. } = r
                {
                    let now = self.w.read_point(*at, *after, *own).and_then(|(u, pos, fr, skip)| self.w.reaching(u, *f, pos, fr, skip)).map(|r| r.1.node);
                    let op = |x: Option<Id>| x.and_then(|x| self.w.node(x)).map(|n| self.core.op_name(n.text.op));
                    eprintln!("recheck-broken {id} {:?} on {}: site {site} {:?} now {now:?} {:?}", op(Some(id)), self.core.values.render(f.0), op(Some(*site)), op(now));
                }
                let why = if rebound(self.core, &self.w, r) {
                    self.rebinds += 1;
                    match r {
                        Read::Use { f, .. } => crate::api::Rerun::Rebind { field: *f },
                        _ => crate::api::Rerun::Other,
                    }
                } else {
                    why_broken(self.core, &self.w, id, r)
                };
                self.make_dirty_for(id, why);
            } else if !res.passes.is_empty() {
                self.refile_passes(id, &res, on.as_deref());
            }
        }
    }

    /// The values done node `id` (result `res`) passed on through views
    /// (`Ctx::define_from` and the others) follow what they read, its
    /// reads holding (re-asked on `on`, or all): each target takes the new
    /// value and is filed again (its readers re-asked), the node not run
    /// again.
    fn refile_passes(&mut self, id: Id, res: &Arc<Res>, on: Option<&[Field]>) {
        let Some((r, defs, contribs, out)) = passes_now(self.core, &self.w, id, res, on) else { return };
        let Some(mut n) = self.w.node(id).cloned() else { return };
        self.core.passes_refiled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let after = r.contribs.clone();
        n.res = Some(Arc::new(r));
        self.w.put(id, n.clone());
        for (f, loc, x) in defs {
            self.file_if(id, &n, f, loc, Some(x), true);
        }
        if contribs {
            self.swap_contribs(n.unit, n.pos, &res.contribs, &after);
        }
        if out {
            self.w.touch_key(Key::Out(id));
            for r in self.w.out_readers_of(id) {
                self.recheck_out(r, id);
            }
        }
    }

    /// Re-asks strictly the reads of the done readers in unit `u` past
    /// position `from`: one that stood on a definition still to be made
    /// runs again (and waits for it).
    pub(crate) fn recheck_strict(&mut self, u: u32, from: Pos) {
        let keys: Vec<(u32, Field)> = self.w.fields_of(u).into_iter().map(|f| (u, f)).collect();
        let mut seen: FxHashSet<Id> = FxHashSet::default();
        for k in keys {
            for r in self.w.readers_between(u, k.1, from, Pos::MAX) {
                if !seen.insert(r) {
                    continue;
                }
                let Some(n) = self.w.node(r) else { continue };
                if n.st != St::Done {
                    continue;
                }
                let Some(res) = n.res.clone() else { continue };
                if res.reads.iter().any(|rd| !holds_with(self.core, &self.w, r, rd, true)) {
                    self.make_dirty_for(r, crate::api::Rerun::Other);
                }
            }
        }
    }

    /// Marks a node dirty for reason `why`.
    pub(crate) fn make_dirty_for(&mut self, id: Id, why: crate::api::Rerun) {
        let Some(n) = self.w.node(id) else { return };
        if n.st != St::Done {
            return;
        }
        self.core.cause(id, why);
        let mut n = n.clone();
        // (A final node, past the evaluation's first round, waits for the
        // final phase, its last result kept.)
        let hold = n.text.stat.fin && self.core.hold_fin.load(std::sync::atomic::Ordering::Relaxed);
        n.st = if hold { St::Held } else { St::Dirty };
        // (Running again: its text's bound until it narrows again.)
        n.narrowed = None;
        self.stale_placer(id, &n);
        self.w.put(id, n);
        self.dirty.push(id);
    }

    /// Node `id` (`n`) runs again for a change: what it placed waits for
    /// its next run to place it again (or to finish without it).
    fn stale_placer(&mut self, id: Id, n: &Node) {
        if let Some(r) = n.res.as_ref() {
            if super::core::debug() && !(r.kids.is_empty() && r.conts.is_empty()) {
                eprintln!("stale placer {id}: {} placements wait for it", r.kids.len() + r.conts.len());
            }
            for &x in r.kids.iter().chain(&r.conts) {
                self.w.awaiting.insert(x, (), &NoMeasure);
            }
        }
    }

    // ------------------------------------------------------------ positions

    /// `k` positions strictly between `a` and `b`, evenly spread, at most
    /// `most` apart; `None` when the gap is too small.
    fn spread(a: Pos, b: Pos, k: usize, most: Pos) -> Option<Vec<Pos>> {
        let k = k as u64;
        if b <= a || b - a <= k {
            return None;
        }
        // (Close after `a`, not halfway: what is placed after one node is
        // placed after the last placed there next, and halving the gap
        // each time would renumber the unit every few dozen.)
        let step = ((b - a) / (k + 1)).min(most);
        Some((1..=k).map(|i| a + step * i).collect())
    }

    /// The position after `pos` in unit `u` (`Pos::MAX` past the last).
    fn next_pos(&self, u: u32, pos: Pos) -> Pos {
        self.w
            .unit(u)
            .order
            .above(&pos)
            .map_or(Pos::MAX, |(&p, _)| p)
    }

    /// `k` new positions right after position `after` (0: the unit's start);
    /// the unit is renumbered when the gap is too small.
    fn positions_after(&mut self, u: u32, after: Pos, k: usize) -> Vec<Pos> {
        // (Past the last: a push's gap each.)
        let bounds = |b: &Self, after: Pos| match b.next_pos(u, after) {
            Pos::MAX => (after.saturating_add(PUSH_GAP * (k as u64 + 1)), PUSH_GAP),
            hi => (hi, STEP_GAP),
        };
        let (hi, most) = bounds(self, after);
        if let Some(v) = Self::spread(after, hi, k, most) {
            return v;
        }
        let after = self.relabel(u, after);
        let (hi, most) = bounds(self, after);
        Self::spread(after, hi, k, most).expect("a renumbered unit has room")
    }

    /// Renumbers unit `u` evenly; returns the new position of what was at
    /// `keep`.
    fn relabel(&mut self, u: u32, keep: Pos) -> Pos {
        super::core::prof::count(43);
        let _t = super::core::prof::Span::new(47);
        self.w.note(super::world::Change::All);
        let epoch = super::live::fresh_epoch();
        self.w.relabels = epoch;
        // (Claim windows are positions: the unit's claims resolve again.)
        self.w.claims_dirty.insert(u, (), &NoMeasure);
        self.w.claim_points = Default::default();
        let un = self.w.unit(u).clone();
        let old_work = un.work.clone();
        let n = un.order.len() as u64;
        let gap = (Pos::MAX / 2 / (n + 2)).min(PUSH_GAP);
        let mut map: FxHashMap<Pos, Pos> = FxHashMap::default();
        let mut order: Vec<(Pos, Id)> = Vec::with_capacity(un.order.len());
        for (i, (&p, &id)) in un.order.iter().enumerate() {
            let q = gap * (i as u64 + 1);
            map.insert(p, q);
            order.push((q, id));
        }
        let new_keep = if keep == 0 { 0 } else { map.get(&keep).copied().unwrap_or(keep) };
        let remap = |p: Pos| -> Pos { map.get(&p).copied().unwrap_or(p) };
        // (A position between nodes, a window's end past one: it moves
        // with the node before it, staying before the next.)
        let olds: Vec<Pos> = un.order.iter().map(|(&p, _)| p).collect();
        let news: Vec<Pos> = order.iter().map(|x| x.0).collect();
        let bound = |p: Pos| -> Pos {
            if p == 0 || p == Pos::MAX {
                return p;
            }
            match olds.binary_search(&p) {
                Ok(i) => news[i],
                Err(0) => news.first().map_or(p, |&f| p.min(f.saturating_sub(1))),
                Err(i) => {
                    let n = news[i - 1].saturating_add(p - olds[i - 1]);
                    news.get(i).map_or(n, |&nx| n.min(nx.saturating_sub(1)))
                }
            }
        };
        // (Frames held back to walk from a position: that position moves
        // too; one that names no node walks the whole unit.)
        if let Some((hu, hps, _)) = &mut self.hold
            && *hu == u
        {
            for hp in hps.iter_mut() {
                *hp = map.get(hp).copied().unwrap_or(0);
            }
        }
        // Nodes.
        self.w.quiet = true;
        for &(q, id) in &order {
            if let Some(mut nd) = self.w.node(id).cloned() {
                nd.pos = q;
                self.w.put(id, nd);
            }
        }
        self.w.quiet = false;
        let mut frames = PMap::new();
        for (&f, fr) in un.frames.iter() {
            let mut fr = *fr;
            fr.start = remap(fr.start);
            frames.insert(f, fr, &NoMeasure);
        }
        // (Only what is renumbered here: the nodes' indexes moved with
        // their nodes above.)
        self.w.with_unit(u, |c| {
            c.order = PMap::from_sorted(order, &Count);
            c.frames = frames;
        });
        // The unit's field cells: definitions, readers, contributions and
        // range readers. (Contributors in work moved with their nodes.)
        for f in self.w.fields_of(u) {
            let c = self.w.field(u, f).cloned().expect("listed");
            let re = |m: &PMap<Pos, Entry>| -> PMap<Pos, Entry> {
                let mut v: Vec<(Pos, Entry)> = m.iter().map(|(&p, e)| (remap(p), *e)).collect();
                v.sort_by_key(|x| x.0);
                PMap::from_sorted(v, &NoMeasure)
            };
            let mut nf = super::world::FieldDefs {
                local: PMap::new(),
                globals: re(&c.defs.globals),
            };
            for (fr, m) in c.defs.local.iter() {
                nf.local.insert(*fr, re(m), &NoMeasure);
            }
            let contribs = match self.core.acc(f) {
                Some(mo) if !c.contribs.is_empty() => {
                    let m = AccMeasure {
                        m: &**mo,
                        v: &self.core.values,
                    };
                    let mut v: Vec<(Pos, ValueId)> = c.contribs.iter().map(|(&p, &x)| (remap(p), x)).collect();
                    v.sort_by_key(|x| x.0);
                    PMap::from_sorted(v, &m)
                }
                _ => c.contribs.clone(),
            };
            self.w.with_field(u, f, |c| {
                c.defs = nf;
                c.contribs = contribs;
            });
        }
        let re = bound;
        // The unit's readers (live), with workers' filing held off: what a
        // worker filed before is at the old positions and moves here; what
        // it would file after at old positions it files no more (the count
        // moved).
        {
            let rd = self.w.rd.clone();
            let mut count = rd.relabels_mut();
            *count = epoch;
            rd.remap_slots(|k| match k {
                ReadKey::Field(ku, f, p) if ku == u => ReadKey::Field(ku, f, remap(p)),
                ReadKey::Range(ku, f, lo, hi) if ku == u => ReadKey::Range(ku, f, bound(lo), bound(hi)),
                k => k,
            });
            for f in rd.fields_of(u) {
                let c = rd.field(u, f).expect("listed");
                let mut readers: Vec<((Pos, Id), ())> = c.readers.iter().map(|(&(p, id), ())| ((remap(p), id), ())).collect();
                readers.sort_by_key(|x| x.0);
                readers.dedup_by_key(|x| x.0);
                let mut ranges: Vec<((Pos, Id, Pos), Pos)> = c.range_readers.iter().map(|(&(p, id, lo), _)| ((bound(p), id, bound(lo)), bound(lo))).collect();
                ranges.sort_by_key(|x| x.0);
                ranges.dedup_by_key(|x| x.0);
                rd.set_field(
                    u,
                    f,
                    super::live::FieldReaders {
                        readers: PMap::from_sorted(readers, &NoMeasure),
                        range_readers: PMap::from_sorted(ranges, &super::live::LeastStart),
                    },
                );
            }
            if let Some(c) = rd.unit(u) {
                let mut v: Vec<((Pos, Id), ())> = c.kind_readers.iter().map(|(&(p, id), ())| ((remap(p), id), ())).collect();
                v.sort_by_key(|x| x.0);
                let kind_readers = PMap::from_sorted(v, &NoMeasure);
                let mut v: Vec<(Pos, (Id, Pos))> = c.fwd_readers.iter().map(|(&p, &(id, r))| (remap(p), (id, re(r)))).collect();
                v.sort_by_key(|x| x.0);
                let fwd_readers = PMap::from_sorted(v, &super::claims::MaxReach);
                let mut v: Vec<(Pos, (Id, Pos))> = c.back_readers.iter().map(|(&p, &(id, r))| (remap(p), (id, re(r)))).collect();
                v.sort_by_key(|x| x.0);
                let back_readers = PMap::from_sorted(v, &super::claims::MinLow);
                rd.set_unit(u, super::live::UnitReaders { kind_readers, fwd_readers, back_readers });
            }
        }
        // The work of the unit.
        self.w.with_unit(u, |c| {
            let mut v: Vec<(Pos, Id)> = old_work.iter().map(|(&p, &x)| (remap(p), x)).collect();
            v.sort_by_key(|x| x.0);
            c.work = PMap::from_sorted(v, &Count);
        });
        self.w.touch_key(Key::Shape(u));
        new_keep
    }

    // ---------------------------------------------------------------- nodes

    /// Makes a node: linked at `pos`, its declared definitions filed as
    /// pending, dirty.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(&mut self, id: Id, text: Arc<Text>, u: u32, parent: Id, place: Place, depth: u32, pos: Pos) {
        let _t = super::core::prof::Span::new(31);
        // (A final node, as one made dirty: held past the evaluation's
        // first round only, else it runs as it comes.)
        let st = if text.stat.fin && self.core.hold_fin.load(std::sync::atomic::Ordering::Relaxed) { St::Held } else { St::Dirty };
        if self.core.marked_ops.read().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&text.op) {
            self.w.op_marked.insert(id, (), &NoMeasure);
        }
        let n = Node {
            text,
            unit: u,
            parent,
            place,
            depth,
            pos,
            frame: FrameId::root(u),
            st,
            res: None,
            wide: false,
            narrowed: None,
            was: None,
        };
        let statics = n.text.stat.contribs.clone();
        // (A placed id comes back: filed for anew. Positions workers took
        // of it before it went are stale: a new epoch.)
        let back = self.w.rd.with_slot(id, |s| {
            let gone = matches!(s, Slot::Gone);
            if gone {
                *s = Slot::None;
            }
            gone
        });
        if back {
            let epoch = super::live::fresh_epoch();
            self.w.relabels = epoch;
            *self.w.rd.relabels_mut() = epoch;
        }
        self.w.put(id, n);
        self.w.link(u, pos, id);
        self.w.unregistered.insert(id, (), &NoMeasure);
        self.shape_at(u, pos);
        // (Readers that found it gone are re-asked: a placed id comes back.)
        self.out_gone(id);
        self.created += 1;
        if let Some(cs) = statics {
            for &(f, x) in cs.iter() {
                self.contrib(u, f, pos, Some(x));
            }
        }
        if st == St::Dirty {
            self.dirty.push(id);
        }
    }

    /// A position for a top node right after position `after`.
    pub(crate) fn top_position(&mut self, u: u32, after: Pos) -> Pos {
        self.positions_after(u, after, 1)[0]
    }

    pub(crate) fn file_declared_pub(&mut self, id: Id) {
        self.file_declared(id);
    }

    /// Files a new node's declared definitions as pending (after its frame
    /// is known).
    fn file_declared(&mut self, id: Id) {
        let _t = super::core::prof::Span::new(32);
        let Some(n) = self.w.node(id).cloned() else { return };
        let Some(decl) = n.text.stat.filed.clone() else { return };
        for &(f, loc) in decl.iter() {
            self.file(id, &n, f, loc, None);
        }
    }

    /// Files one definition entry of node `n`.
    fn file(&mut self, id: Id, n: &Node, f: Field, loc: Locality, val: Option<ValueId>) {
        self.file_if(id, n, f, loc, val, false);
    }

    /// Files an entry (`if_changed`: unless the same is filed).
    fn file_if(&mut self, id: Id, n: &Node, f: Field, loc: Locality, val: Option<ValueId>, if_changed: bool) {
        let _t = super::core::prof::Span::new(34);
        let fr = self.def_frame(id, n, loc);
        let pos = def_pos(n, loc);
        let e = Entry { node: id, val, loc };
        if let Some(d) = &mut self.defer {
            d.push(n.unit, f, super::batch::FieldOp::File { pos, fr, e, at: n.pos, if_changed });
            return;
        }
        if if_changed && self.filed(id, n, f, loc) == val.map(|v| (v, loc)) && val.is_some() {
            return;
        }
        self.w.file_def(n.unit, f, pos, fr, e);
        // (A definition to come breaks no read, but for its prediction:
        // the readers after it are asked against that.)
        if val.is_none() && pending_breaks_none(self.core, f) && predicted(&self.w, id, f).is_none() {
            self.w.touch_key(Key::Defs(n.unit, f));
        } else {
            self.def_changed_in(n.unit, f, n.pos, Some(fr), loc == Locality::Global);
        }
    }

    /// The value and locality of node `n`'s filed entry of `f` (`None`:
    /// none filed, or filed pending).
    fn filed(&self, id: Id, n: &Node, f: Field, loc: Locality) -> Option<(ValueId, Locality)> {
        let fd = &self.w.field(n.unit, f)?.defs;
        let pos = def_pos(n, loc);
        let e = if loc == Locality::Global {
            fd.globals.get(&pos)
        } else {
            fd.local.get(&self.def_frame(id, n, loc))?.get(&pos)
        }?;
        (e.node == id).then_some(())?;
        Some((e.val?, e.loc))
    }

    /// The definitions forwarding loop-back fields `moved` (`Ctx::
    /// define_forward`) follow their new round values: each done
    /// forwarder's result takes the value and the entry is filed again (its
    /// readers re-asked), the node not run again.
    pub(crate) fn refile_forwards(&mut self, moved: &[Field]) {
        for &m in moved {
            let Some(&v) = self.w.loops.get(&m) else { continue };
            let raw = m.0.raw();
            let ids: Vec<Id> = self
                .w
                .fwds
                .range(std::ops::Bound::Included(&(raw, 0)), std::ops::Bound::Included(&(raw, Id::MAX)))
                .map(|(&(_, id), _)| id)
                .collect();
            for id in ids {
                let Some(n) = self.w.node(id).filter(|n| n.st == St::Done).cloned() else { continue };
                let Some(r) = &n.res else { continue };
                let mut res = (**r).clone();
                let mut ch = Vec::new();
                let mut lost = false;
                for fw in r.forwards_from(m) {
                    let (f, loc) = (fw.f, fw.loc);
                    // (A part the new value lacks, or the part kept
                    // changed: the node runs again.)
                    let Some(x) = self.core.eval_path(fw.path, v).filter(|_| fw.holds(self.core, &self.w)) else {
                        lost = true;
                        break;
                    };
                    if let Some(d) = res.defs.iter_mut().find(|d| d.f == f && d.loc.entry_key() == loc.entry_key())
                        && d.val != x
                    {
                        d.val = x;
                        ch.push((f, d.loc, x));
                    }
                }
                if lost {
                    self.make_dirty_for(id, crate::api::Rerun::Loop { field: m });
                    continue;
                }
                if ch.is_empty() {
                    continue;
                }
                let mut n2 = n;
                n2.res = Some(Arc::new(res));
                self.w.put(id, n2.clone());
                for (f, loc, x) in ch {
                    self.file_if(id, &n2, f, loc, Some(x), true);
                }
            }
        }
    }

    /// Removes one definition entry of node `n`.
    fn unfile(&mut self, id: Id, n: &Node, f: Field, loc: Locality) {
        let fr = self.def_frame(id, n, loc);
        let pos = def_pos(n, loc);
        if let Some(d) = &mut self.defer {
            d.push(n.unit, f, super::batch::FieldOp::Unfile { pos, fr, global: loc == Locality::Global, at: n.pos });
            return;
        }
        self.w.unfile_def(n.unit, f, pos, fr, loc == Locality::Global);
        self.def_changed_in(n.unit, f, n.pos, Some(fr), loc == Locality::Global);
    }

    /// The frame a definition of locality `loc` by node `n` is filed in.
    fn def_frame(&self, id: Id, n: &Node, loc: Locality) -> FrameId {
        match loc {
            Locality::Local | Locality::Global => n.frame,
            Locality::Scope | Locality::ScopeAt(_) => {
                let b = n.brackets();
                let opens = b.opens;
                if opens == 0 {
                    return n.frame;
                }
                let k = match loc {
                    Locality::ScopeAt(k) => k.min(opens - 1),
                    _ => 0,
                };
                FrameId::opened(id, k)
            }
        }
    }

    /// The definitions of `f` in unit `u` changed at position `at`: its
    /// readers after it are re-asked, up to the next entry that shadows
    /// the change (`fr`: the frame of a local entry; `None` unbounded).
    fn def_changed(&mut self, u: u32, f: Field, at: Pos) {
        self.def_changed_in(u, f, at, None, false);
    }

    fn def_changed_in(&mut self, u: u32, f: Field, at: Pos, fr: Option<FrameId>, global: bool) {
        self.w.touch_key(Key::Defs(u, f));
        let hi = match fr {
            Some(fr) => shadow_end(self.core, self.w.field(u, f).map(|c| &c.defs), f, at, fr, global),
            None => Pos::MAX,
        };
        let t = super::core::prof::start();
        for r in self.w.readers_between(u, f, at.saturating_sub(1), hi) {
            self.recheck_on(r, f);
        }
        super::core::prof::stop(35, t);
        // (CLEAN_SCOPED_CHECK: the readers past the bound are asked too,
        // a broken one named.)
        if hi != Pos::MAX && scoped_check() {
            for r in self.w.readers_between(u, f, hi - 1, Pos::MAX) {
                self.beyond.push((r, f));
            }
        }
    }

    /// Frees node `id` and everything it unfolded.
    pub(crate) fn free(&mut self, id: Id) {
        let Some(n) = self.w.node(id).cloned() else { return };
        if super::core::debug() && n.parent != NONE {
            eprintln!("free {id} (placed by {})", n.parent);
        }
        self.w.awaiting.remove(&id, &NoMeasure);
        if let Some(res) = n.res.clone() {
            for &k in res.kids.iter().chain(&res.conts) {
                if self.w.node(k).is_some_and(|c| c.parent == id) {
                    self.free(k);
                }
            }
            self.drop_reads(id, &n, &res);
        }
        // (What workers filed for a run not committed goes too; a late one
        // files nothing.)
        let w = &self.w;
        w.rd.with_slot(id, |s| {
            for &k in s.keys() {
                index_key(w, id, k, false);
            }
            *s = Slot::Gone;
        });
        self.unfile_all(id, &n);
        self.claimer_gone(id, n.unit);
        self.w.unlink(n.unit, n.pos);
        self.w.drop_node(id);
        // Its readers and those of its position. (They stay filed as its
        // readers: one that runs again reading it, the node made again, has
        // the same entries, and a diff of them files nothing; one that no
        // longer reads it unfiles them.)
        for r in self.w.out_readers_of(id) {
            self.recheck(r, 13);
        }
        for r in self.w.anchor_readers_of(id) {
            self.recheck(r, 13);
        }
        self.shape_at(n.unit, n.pos);
        self.w.touch_key(Key::Out(id));
        // (Scope markers are not nodes to the client.)
        if !matches!(n.place, Place::Marker(_)) {
            self.freed.push(id);
        }
        self.woken.push(id);
        if !n.brackets().eq(&crate::api::Brackets::default()) {
            self.frames_from(n.unit, n.pos);
        }
    }

    /// Files the contributions of every node to fields declared
    /// accumulators since they were made (a contribution to a field no
    /// accumulator was not kept).
    pub(crate) fn refile_contribs(&mut self) {
        let mut todo: Vec<(u32, Field, Pos, ValueId)> = Vec::new();
        for (_, n) in self.w.nodes.iter() {
            for (f, x) in contribs_of(n) {
                if self.core.acc(f).is_some() && self.w.field(n.unit, f).and_then(|c| c.contribs.get(&n.pos)) != Some(&x) {
                    todo.push((n.unit, f, n.pos, x));
                }
            }
        }
        for (u, f, pos, x) in todo {
            self.contrib(u, f, pos, Some(x));
        }
    }

    /// A node's contributions at `pos` of unit `u` made `after` where they
    /// were `before`.
    fn swap_contribs(&mut self, u: u32, pos: Pos, before: &[(Field, ValueId)], after: &[(Field, ValueId)]) {
        for &(f, _) in before {
            if !after.iter().any(|c| c.0 == f) {
                self.contrib(u, f, pos, None);
            }
        }
        for &(f, x) in after {
            if !before.contains(&(f, x)) {
                self.contrib(u, f, pos, Some(x));
            }
        }
    }

    /// Sets a contribution to accumulator `f` at `pos` (a field declared
    /// no accumulator keeps none).
    pub(crate) fn contrib(&mut self, u: u32, f: Field, pos: Pos, x: Option<ValueId>) {
        let Some(mo) = self.core.acc(f) else { return };
        if let Some(d) = &mut self.defer {
            d.push(u, f, super::batch::FieldOp::Contrib { pos, x });
            return;
        }
        let m = AccMeasure {
            m: &**mo,
            v: &self.core.values,
        };
        super::core::prof::time(16, || self.w.set_contrib(&m, u, f, pos, x));
        self.w.touch_key(Key::Acc(u, f));
        for r in super::core::prof::time(17, || self.w.range_readers_over(u, f, pos)) {
            super::core::prof::count(21);
            self.recheck_on(r, f);
        }
    }

    /// The structure of unit `u` changed: its shape readers are re-asked.
    pub(crate) fn shape_changed(&mut self, u: u32) {
        self.w.touch_key(Key::Shape(u));
    }

    /// The structure of unit `u` changed at position `p`: the sibling
    /// readers whose windows reach it are re-asked, the claims reaching it
    /// resolved again.
    pub(crate) fn shape_at(&mut self, u: u32, p: Pos) {
        let _t = super::core::prof::Span::new(33);
        self.shape_changed(u);
        for r in self.w.window_readers_at(u, p) {
            self.recheck_shape(r, 14);
        }
        self.claim_point(u, p);
    }

    /// Makes application `app` (`op` on `args`): the first node of a unit
    /// of its own.
    pub(crate) fn create_app(&mut self, app: Id, op: crate::api::OpId, args: Arc<[ValueId]>) {
        if self.w.node(app).is_some() {
            return;
        }
        let u = self.core.app_unit(app);
        new_unit(&mut self.w, u, app);
        let text = self.core.text(Some(app), op, args);
        let pos = self.positions_after(u, 0, 1)[0];
        self.create(app, text, u, NONE, Place::Top, 0, pos);
        self.frames_from(u, pos);
        self.file_declared(app);
    }

    /// An application node's run set `env`: its unit's environment.
    pub(crate) fn app_env(&mut self, id: Id, env: &[(Field, ValueId)]) {
        if env.is_empty() {
            return;
        }
        let Some(u) = self.w.node(id).map(|n| n.unit) else { return };
        if self.w.units.get(&u).is_none_or(|un| un.app != id) {
            return;
        }
        for &(f, x) in env {
            self.set_unit_env(u, f, x);
        }
    }

    /// Sets field `f` of unit `u`'s environment: its readers are re-asked.
    pub(crate) fn set_unit_env(&mut self, u: u32, f: Field, x: ValueId) {
        let Some(un) = self.w.unit_opt(u) else { return };
        if un.env.get(&f) == Some(&x) {
            return;
        }
        self.w.note(super::world::Change::Field(u, f, 0));
        self.w.with_unit(u, |un| {
            un.env.insert(f, x, &NoMeasure);
        });
        self.def_changed(u, f, 0);
    }

    /// Re-asks the readers of node `id`'s output whose read of it no longer
    /// holds (an output committed equal to the last result's, read as
    /// another while that result was not visible).
    fn recheck_stale_out_readers(&mut self, id: Id) {
        let stale: Vec<Id> = self
            .w
            .out_readers_of(id)
            .into_iter()
            .filter(|&r| {
                self.w.node(r).and_then(|n| n.res.clone()).is_some_and(|res| {
                    res.reads.iter().any(|rd| matches!(rd, Read::Placed { node, .. } | Read::Out { node, .. } if *node == id) && !holds(self.core, &self.w, r, rd))
                })
            })
            .collect();
        for r in stale {
            self.recheck_out(r, id);
        }
    }

    /// Holds every final node that ran: its facts go (its readers wait for
    /// the final phase).
    pub(crate) fn hold_finals(&mut self) {
        let mut fins = self.w.fins_in(St::Done);
        fins.extend(self.w.fins_in(St::Dirty));
        fins.sort_unstable();
        for id in fins {
            let Some(n) = self.w.node(id).cloned() else { continue };
            if let Some(res) = n.res.clone() {
                for &k in res.kids.iter().chain(&res.conts) {
                    if self.w.node(k).is_some_and(|c| c.parent == id) {
                        self.free(k);
                    }
                }
                self.drop_reads(id, &n, &res);
            }
            self.unfile_all(id, &n);
            let mut m = self.w.node(id).cloned().expect("live");
            m.st = St::Held;
            m.res = None;
            self.w.put(id, m);
            self.file_declared(id);
            if let Some(cs) = n.text.stat.contribs.clone() {
                for &(f, x) in cs.iter() {
                    self.contrib(n.unit, f, n.pos, Some(x));
                }
            }
            self.out_gone(id);
            if n.brackets() != crate::api::Brackets::default() {
                self.frames_from(n.unit, n.pos);
            }
        }
    }

    /// Removes every definition entry and contribution of node `n`.
    pub(crate) fn unfile_all(&mut self, id: Id, n: &Node) {
        if let Some(res) = n.res.clone() {
            for d in &res.defs {
                self.unfile(id, n, d.f, d.loc);
            }
        }
        if let Some(decl) = n.text.stat.filed.clone() {
            for &(f, loc) in decl.iter() {
                if !n.res.as_ref().is_some_and(|r| r.defs.iter().any(|d| d.f == f && d.loc == loc)) {
                    self.unfile(id, n, f, loc);
                }
            }
        }
        for (f, _) in contribs_of(n) {
            self.contrib(n.unit, f, n.pos, None);
        }
    }

    /// Node `x`'s output is gone or changed, or it came: its readers and
    /// those of where it is are re-asked.
    pub(crate) fn out_gone(&mut self, x: Id) {
        self.w.touch_key(Key::Out(x));
        for r in self.w.out_readers_of(x) {
            self.recheck(r, 15);
        }
        for r in self.w.anchor_readers_of(x) {
            self.recheck(r, 15);
        }
    }

    /// Replaces a node's text: it runs again; its declared definitions are
    /// filed anew.
    pub(crate) fn retext(&mut self, id: Id, text: Arc<Text>) {
        let Some(n) = self.w.node(id).cloned() else { return };
        let old_b = n.brackets();
        if let Some(decl) = n.text.stat.filed.clone() {
            for &(f, loc) in decl.iter() {
                if !n.res.as_ref().is_some_and(|r| r.defs.iter().any(|d| d.f == f && d.loc == loc)) {
                    self.unfile(id, &n, f, loc);
                }
            }
        }
        let mut m = n.clone();
        m.text = text;
        m.wide = false;
        m.narrowed = None;
        // (The text its last result is of, while that is not this one.)
        let of = |t: &Arc<Text>| n.res.as_ref().is_some_and(|r| r.text == super::world::text_key(t));
        m.was = if of(&m.text) {
            None
        } else if of(&n.text) {
            Some(n.text.clone())
        } else {
            n.was.clone().filter(|w| of(w))
        };
        // (Without a result, or claimed, it contributes what its text
        // declares: the new text's now.)
        let (before, after) = (contribs_of(&n), contribs_of(&m));
        if before != after {
            self.swap_contribs(n.unit, n.pos, &before, &after);
        }
        if matches!(m.st, St::Inert(_)) {
            // A claimed node's new text is analysed only; it does not run.
            self.w.put(id, m);
            self.w.unregistered.insert(id, (), &NoMeasure);
            self.shape_at(n.unit, n.pos);
            return;
        }
        // (A final node: as made dirty, held past the evaluation's first
        // round.)
        let hold = m.text.stat.fin && self.core.hold_fin.load(std::sync::atomic::Ordering::Relaxed);
        m.st = if hold { St::Held } else { St::Dirty };
        self.stale_placer(id, &m);
        // Scope entries filed in the frames the old text opened move to the
        // ones the new text opens (or to its own frame); they keep their
        // values until the run.
        let moved: Vec<(Field, Locality, Option<ValueId>)> = if m.brackets() == old_b {
            Vec::new()
        } else {
            self.entries_of(id, &n)
                .into_iter()
                .filter(|&(_, loc, _)| matches!(loc, Locality::Scope | Locality::ScopeAt(_)) && self.def_frame(id, &n, loc) != self.def_frame(id, &m, loc))
                .collect()
        };
        for &(f, loc, _) in &moved {
            self.unfile(id, &n, f, loc);
        }
        self.w.put(id, m.clone());
        for (f, loc, val) in moved {
            self.file(id, &m, f, loc, val);
        }
        self.w.unregistered.insert(id, (), &NoMeasure);
        self.core.cause(id, crate::api::Rerun::Text);
        if m.st == St::Dirty {
            self.dirty.push(id);
        }
        // (Its last result of its old text, or of its new one, as a text
        // it had before: its placed output appears or goes for a reader.)
        if let Some(r) = &n.res
            && !Arc::ptr_eq(&n.text, &m.text)
            && (r.text == super::world::text_key(&n.text) || r.text == super::world::text_key(&m.text))
        {
            self.w.touch_key(Key::Out(id));
            for x in self.w.out_readers_of(id) {
                self.recheck_out(x, id);
            }
        }
        self.file_declared(id);
        if m.brackets() != old_b {
            self.frames_from(n.unit, n.pos);
        }
        self.shape_at(n.unit, n.pos);
    }

    // --------------------------------------------------------------- frames

    /// Recomputes the frames of unit `u` from position `from` on, until the
    /// frame stack agrees with what it was; nodes whose frame changed have
    /// their definitions filed again and their readers re-asked.
    pub(crate) fn frames_from(&mut self, u: u32, from: Pos) {
        if let Some((hu, hps, _)) = &mut self.hold
            && *hu == u
        {
            hps.push(from);
            return;
        }
        self.frames_now(u, from);
    }

    fn frames_now(&mut self, u: u32, from: Pos) {
        super::core::prof::time(4, || self.frames_walk(u, from));
    }

    fn frames_walk(&mut self, u: u32, from: Pos) {
        let un = self.w.unit(u).clone();
        // The stack before `from`: after the node before it.
        let mut stack: Vec<FrameId> = match un.order.below(&from) {
            Some((_, &p)) => self.stack_after(p),
            None => vec![FrameId::root(u)],
        };
        let mut frames = un.frames.clone();
        if frames.get(&FrameId::root(u)).is_none() {
            frames.insert(
                FrameId::root(u),
                Frame {
                    parent: FrameId::root(u),
                    depth: 0,
                    start: 0,
                    kind: None,
                },
                &NoMeasure,
            );
        }
        let mut moved: Vec<(Id, Node, FrameId)> = Vec::new();
        let mut first = true;
        let mut changed_frames: FxHashSet<FrameId> = FxHashSet::default();
        // (Walked lazily: it usually agrees again after a node or two.)
        let mut next = un.order.at_or_above(&from).map(|(&p, &id)| (p, id));
        let mut walked = from;
        while let Some((p, id)) = next {
            next = un.order.above(&p).map(|(&p, &id)| (p, id));
            // (The node the walk stops at counts: the frame it enters may
            // have changed.)
            walked = p;
            let Some(n) = self.w.node(id).cloned() else { continue };
            let b = n.brackets();
            let (cb, ob, ca, oa) = if b.before {
                (b.closes, b.opens, 0, 0)
            } else {
                (0, 0, b.closes, b.opens)
            };
            // Agrees from here on: the node's recorded frame chain is the
            // stack, and no frame on it changed (later nodes' brackets are
            // their own callers' to recompute).
            // (Compared with the stack it is in: after the brackets it
            // applies before itself.)
            if !first && ob == 0 && stack.iter().all(|f| !changed_frames.contains(f)) {
                let mut at = stack.clone();
                for _ in 0..cb {
                    if at.len() > 1 {
                        at.pop();
                    }
                }
                let mut have = self.w.chain(u, n.frame);
                have.reverse();
                if have == at && n.frame == *at.last().expect("root") {
                    break;
                }
            }
            first = false;
            let mut k: u8 = 0;
            for _ in 0..cb {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            for _ in 0..ob {
                let f = FrameId::opened(id, k);
                k += 1;
                let fr = Frame {
                    parent: *stack.last().expect("root"),
                    depth: stack.len() as u32,
                    start: n.pos,
                    kind: frame_kind(&n, k - 1),
                };
                if frames.get(&f).is_none_or(|o| o.parent != fr.parent || o.depth != fr.depth || o.kind != fr.kind) {
                    changed_frames.insert(f);
                }
                frames.insert(f, fr, &NoMeasure);
                stack.push(f);
            }
            let fr_now = *stack.last().expect("root");
            if n.frame != fr_now {
                moved.push((id, n.clone(), fr_now));
            }
            for _ in 0..ca {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            for _ in 0..oa {
                let f = FrameId::opened(id, k);
                k += 1;
                let fr = Frame {
                    parent: *stack.last().expect("root"),
                    depth: stack.len() as u32,
                    start: n.pos,
                    kind: frame_kind(&n, k - 1),
                };
                if frames.get(&f).is_none_or(|o| o.parent != fr.parent || o.depth != fr.depth || o.kind != fr.kind) {
                    changed_frames.insert(f);
                }
                frames.insert(f, fr, &NoMeasure);
                stack.push(f);
            }
        }
        self.w.with_unit(u, |c| c.frames = frames);
        if !changed_frames.is_empty() || !moved.is_empty() {
            self.w.note(super::world::Change::Frames(u));
            self.shape_changed(u);
        }
        // The scope readers in the walked stretch are re-asked (also when
        // nothing moved: a frame that is gone leaves no trace in the walk).
        {
            for r in self.w.kind_readers_in(u, from, walked) {
                self.recheck(r, 16);
            }
        }
        for (id, old, fr) in moved {
            // Its entries move frames: unfiled under the old, filed under
            // the new.
            let entries: Vec<(Field, Locality, Option<ValueId>)> = self.entries_of(id, &old);
            for &(f, loc, _) in &entries {
                self.unfile(id, &old, f, loc);
            }
            let mut n = self.w.node(id).cloned().expect("live");
            n.frame = fr;
            self.w.put(id, n.clone());
            for (f, loc, val) in entries {
                self.file(id, &n, f, loc, val);
            }
            // Its own reads ask from another frame now.
            self.recheck(id, 17);
        }
    }

    /// A node's definition entries: committed values, else declared pending.
    /// The definition entries node `id` (as `n`) has filed: its run's and
    /// its declared ones, as the index holds them.
    fn entries_of(&self, id: Id, n: &Node) -> Vec<(Field, Locality, Option<ValueId>)> {
        let mut keys: Vec<(Field, Locality)> = Vec::new();
        if let Some(r) = &n.res {
            keys.extend(r.defs.iter().map(|d| (d.f, d.loc)));
        }
        if let Some(decl) = &n.text.stat.filed {
            for &(f, loc) in decl.iter() {
                if !keys.iter().any(|e| e.0 == f && e.1 == loc) {
                    keys.push((f, loc));
                }
            }
        }
        keys.into_iter()
            .filter_map(|(f, loc)| {
                let fd = &self.w.field(n.unit, f)?.defs;
                let pos = def_pos(n, loc);
                let e = if loc == Locality::Global {
                    fd.globals.get(&pos)
                } else {
                    fd.local.get(&self.def_frame(id, n, loc))?.get(&pos)
                }?;
                (e.node == id).then_some((f, e.loc, e.val))
            })
            .collect()
    }

    /// The frame stack after node `id` (its brackets after it applied).
    fn stack_after(&self, id: Id) -> Vec<FrameId> {
        let Some(n) = self.w.node(id) else {
            return vec![FrameId::root(0)];
        };
        let mut stack = self.w.chain(n.unit, n.frame);
        stack.reverse();
        let b = n.brackets();
        if !b.before {
            for _ in 0..b.closes {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            for k in 0..b.opens {
                stack.push(FrameId::opened(id, k));
            }
        }
        stack
    }

    // ------------------------------------------------------------ placement

    /// The id of a placed node: its placer, the kind of placement, the
    /// operation, and its ordinal among the placer's placements of that
    /// kind and operation (and anchor).
    pub(crate) fn placed_id(parent: Id, place: Place, op: crate::api::OpId, ord: u32) -> Id {
        let (tag, anchor) = match place {
            Place::Child => (1, 0),
            Place::Cont => (2, 0),
            Place::After(a) => (3, a),
            Place::Marker(open) => (4 + u64::from(open), 0),
            Place::Top => (6, 0),
        };
        let h = hash_words(0x91ac, &[parent, tag, anchor, u64::from(op.0), u64::from(ord)]);
        // (Never NONE, never a pushed id: those are small.)
        h | 1 << 63
    }

    /// Makes `id`'s placements what its run placed: kept nodes keep their
    /// place and subtree, gone ones are freed, new ones made; returns the
    /// kids' and continuations' ids.
    pub(crate) fn place(&mut self, id: Id, kids: &[Spec], conts: &[Spec]) -> (Vec<Id>, Vec<Id>) {
        self.place_some(id, kids, conts, false)
    }

    /// As [`Self::place`]; `prefix`: the placements of a run that stopped,
    /// so far: the earlier ones it did not make (yet) stay, after them.
    fn place_some(&mut self, id: Id, kids: &[Spec], conts: &[Spec], prefix: bool) -> (Vec<Id>, Vec<Id>) {
        let u = self.w.node(id).map(|n| n.unit).expect("live");
        self.hold = Some((u, Vec::new(), Vec::new()));
        let r = self.place_held(id, kids, conts, prefix);
        // The frames walked from each placement, in order; then the new
        // nodes' declared definitions filed in them.
        if let Some((u, mut ps, ids)) = self.hold.take() {
            ps.sort_unstable();
            ps.dedup();
            for p in ps {
                self.frames_now(u, p);
            }
            for x in ids {
                self.file_declared(x);
            }
        }
        r
    }

    fn place_held(&mut self, id: Id, kids: &[Spec], conts: &[Spec], prefix: bool) -> (Vec<Id>, Vec<Id>) {
        let n = self.w.node(id).cloned().expect("live");
        let old: (Vec<Id>, Vec<Id>) = n.res.as_ref().map_or((Vec::new(), Vec::new()), |r| (r.kids.clone(), r.conts.clone()));
        let cont = self.cont.is_some();
        let (bk, bc) = self.cont.unwrap_or((0, 0));
        let (bk, bc) = (bk.min(old.0.len()), bc.min(old.1.len()));
        let by_order = n.text.stat.after_by_order;
        let key = |place: Place, op: crate::api::OpId| -> PlaceKey {
            match place {
                Place::Child => (1, op, 0),
                Place::Cont => (2, op, 0),
                Place::After(a) => (3, op, a),
                Place::Marker(o) => (4 + u64::from(o), op, 0),
                Place::Top => (6, op, 0),
            }
        };
        // (A stop continuing its last: what it committed, as its memo
        // has it; else walked.)
        let res_ptr = n.res.as_ref().map_or(0, |r| Arc::as_ptr(r) as usize);
        // (Taken whatever this commit is: one not continuing its last stop,
        // a run resumed at a mark, say, places otherwise than the memo
        // counted, and it goes.)
        let memo = self.cont_memo.take().filter(|m| cont && prefix && m.res == res_ptr && m.bk == bk && m.bc == bc);
        // (Continuing: the ordinals go on from the placements committed.)
        let seed = |ids: &[Id]| -> FxHashMap<PlaceKey, u32> {
            let mut ord = FxHashMap::default();
            if cont {
                for x in ids {
                    if let Some(xn) = self.w.node(*x) {
                        *ord.entry(key(xn.place, xn.text.op)).or_insert(0) += 1;
                    }
                }
            }
            ord
        };
        let (seed_k, seed_c) = match &memo {
            Some(m) => {
                if memo_check() {
                    assert!(m.ord_k == seed(&old.0[..bk]) && m.ord_c == seed(&old.1[..bc]), "placer {id}: its memo's ordinals are not its placements'");
                    assert!(old.0[..bk].iter().chain(&old.1[..bc]).all(|&x| self.w.node(x).is_some() && !self.w.awaiting.contains_key(&x)), "placer {id}: its memo's placements are not all live and made");
                }
                (m.ord_k.clone(), m.ord_c.clone())
            }
            None => (seed(&old.0[..bk]), seed(&old.1[..bc])),
        };
        // (Kept by their text: a placement whose text is an earlier
        // result's placement's in the ends the two share keeps that node,
        // so one inserted or removed among them (by an edit,
        // `Engine::insert_placed`) moves none of the others onto its
        // neighbour's text. A stopped run's so far: the start only.)
        let kept_ids = |specs: &[Spec], olds: &[Id]| -> Vec<Option<Id>> {
            let same = |s: &Spec, x: Id| {
                self.w.node(x).is_some_and(|xn| xn.place == s.place && xn.text.op == s.op && *xn.text.args == *s.args)
            };
            let mut out = vec![None; specs.len()];
            if memo.is_some() {
                return out;
            }
            let mut a = 0;
            while a < specs.len() && a < olds.len() && same(&specs[a], olds[a]) {
                out[a] = Some(olds[a]);
                a += 1;
            }
            if !prefix {
                let mut b = 0;
                while b < specs.len() - a
                    && b < olds.len() - a
                    && same(&specs[specs.len() - 1 - b], olds[olds.len() - 1 - b])
                {
                    out[specs.len() - 1 - b] = Some(olds[olds.len() - 1 - b]);
                    b += 1;
                }
            }
            out
        };
        let kept_k = kept_ids(kids, &old.0[bk..]);
        let kept_c = kept_ids(conts, &old.1[bc..]);
        let mut taken: FxHashSet<Id> = kept_k.iter().chain(&kept_c).flatten().copied().collect();
        taken.extend(old.0[..bk].iter().chain(&old.1[..bc]).copied());
        let assign = |specs: &[Spec], kept: &[Option<Id>], mut ord: FxHashMap<PlaceKey, u32>, taken: &mut FxHashSet<Id>| {
            let v = specs
                .iter()
                .zip(kept)
                .map(|(s, &k)| {
                    let o = ord.entry(key(s.place, s.op)).or_insert(0);
                    *o += 1;
                    if let Some(x) = k {
                        return x;
                    }
                    // (An ordinal's id a kept node holds: another, the same
                    // in every run that keeps those nodes.)
                    let mut x = Self::placed_id(id, s.place, s.op, *o);
                    let mut salt = 0u64;
                    while taken.contains(&x) {
                        salt += 1;
                        x = hash_words(0x91ad, &[x, salt]) | 1 << 63;
                    }
                    taken.insert(x);
                    x
                })
                .collect::<Vec<Id>>();
            (v, ord)
        };
        let (kid_ids, ord_k) = assign(kids, &kept_k, seed_k, &mut taken);
        let (cont_ids, ord_c) = assign(conts, &kept_c, seed_c, &mut taken);
        // (Its earlier attempts': asked only where a continuation of its
        // goes after them.)
        let mut own: Option<FxHashSet<Id>> = None;
        let mut keep: FxHashSet<Id> = FxHashSet::default();
        if memo.is_some() {
            // (Its earlier attempts' were made, and stand.)
            for x in kid_ids.iter().chain(&cont_ids) {
                self.w.awaiting.remove(x, &NoMeasure);
            }
        } else {
            // Gone: freed (a stopped run's: not yet made, kept; a continued
            // run's earlier attempts' stand).
            keep.extend(kid_ids.iter().chain(&cont_ids).copied());
            keep.extend(old.0[..bk].iter().chain(&old.1[..bc]).copied());
            if !prefix {
                for &x in old.0.iter().chain(&old.1) {
                    if !keep.contains(&x) {
                        self.free(x);
                    }
                }
            }
            // (Made again: no longer waiting for this placer.)
            for x in &keep {
                self.w.awaiting.remove(x, &NoMeasure);
            }
        }
        // Kids: in order after `id` (each after the previous one's subtree;
        // continuing, after those committed).
        let mut after = id;
        if let Some(&k) = old.0[..bk].iter().rev().find(|&&k| self.w.node(k).is_some()) {
            after = self.extent_end(k);
        }
        for (spec, &kid) in kids.iter().zip(&kid_ids) {
            after = self.put_placed(id, &n, kid, spec, n.depth + 1, after);
        }
        // Continuations: after `id`'s subtree (or the anchor's), at its level.
        let mut last_cont: FxHashMap<Id, Id> = FxHashMap::default();
        for (spec, &c) in conts.iter().zip(&cont_ids) {
            let (anchor, depth) = match spec.place {
                Place::After(a) => (a, self.w.node(a).map_or(n.depth, |x| x.depth)),
                _ => (id, n.depth),
            };
            let base = match last_cont.get(&anchor) {
                Some(&b) => b,
                // (After the follow nodes other placers keep there;
                // continuing, after its own committed there too.)
                None => {
                    let mut b = self.after_conts(anchor, self.w.subtree_end(anchor), id, spec.place);
                    // (Its earlier attempts' there, not the earlier
                    // result's: one of those is what this one makes again.)
                    while cont
                        && let Some(y) = self.w.next(b).filter(|y| {
                            self.w.node(*y).is_some_and(|yn| yn.place == spec.place && yn.parent == id)
                                && own.get_or_insert_with(|| old.0[..bk].iter().chain(&old.1[..bc]).copied().collect()).contains(y)
                        })
                    {
                        b = self.extent_end(y);
                    }
                    b
                }
            };
            let end = self.put_placed(id, &n, c, spec, depth, base);
            last_cont.insert(anchor, end);
        }
        // (By order, `Operation::after_by_order`: a stopped run's `n`-th
        // continuation after a node of an operation stands for the earlier
        // result's `n`-th, which goes, not left beside it.)
        let rest_c: Vec<Id> = {
            let rest = if cont { &old.1[bc..] } else { &old.1[..] };
            if by_order && prefix {
                let made: FxHashSet<Id> = cont_ids.iter().chain(&kid_ids).copied().collect();
                let mut left: FxHashMap<crate::api::OpId, usize> = FxHashMap::default();
                for s in conts.iter().filter(|s| matches!(s.place, Place::After(_))) {
                    *left.entry(s.op).or_insert(0) += 1;
                }
                let mut out = Vec::with_capacity(rest.len());
                for &x in rest {
                    let gone = self.w.node(x).is_some_and(|xn| {
                        matches!(xn.place, Place::After(_))
                            && xn.parent == id
                            && left.get_mut(&xn.text.op).is_some_and(|l| *l > 0 && { *l -= 1; true })
                    });
                    if !gone {
                        out.push(x);
                    } else if !made.contains(&x) {
                        self.free(x);
                    }
                }
                out
            } else {
                rest.to_vec()
            }
        };
        if cont {
            // Its earlier attempts', its own, then (stopped) the earlier
            // result's it has not made again.
            let made: FxHashSet<Id> = kid_ids.iter().chain(&cont_ids).copied().collect();
            let live = memo.is_some();
            let join = |own: &[Id], new: Vec<Id>, rest: &[Id]| -> Vec<Id> {
                let mut v: Vec<Id> = if live { own.to_vec() } else { own.iter().copied().filter(|&x| self.w.node(x).is_some()).collect() };
                v.extend(new);
                if prefix {
                    v.extend(rest.iter().copied().filter(|x| !made.contains(x) && self.w.node(*x).is_some()));
                }
                v
            };
            let (nk, nc) = (kid_ids.len(), cont_ids.len());
            let out = (join(&old.0[..bk], kid_ids, &old.0[bk..]), join(&old.1[..bc], cont_ids, &rest_c));
            // (Its memo for the next stop, all its attempts' live: its
            // result's address set when committed.)
            if prefix && (live || (out.0.len() >= bk + nk && out.1.len() >= bc + nc && out.0[..bk].len() == bk && old.0[..bk] == out.0[..bk] && old.1[..bc] == out.1[..bc])) {
                self.cont_memo = Some(ContMemo { res: 0, bk: bk + nk, bc: bc + nc, ord_k, ord_c });
            }
            return out;
        }
        if prefix {
            let mut kid_ids = kid_ids;
            let mut cont_ids = cont_ids;
            kid_ids.extend(old.0.iter().copied().filter(|x| !keep.contains(x) && self.w.node(*x).is_some()));
            cont_ids.extend(rest_c.iter().copied().filter(|x| !keep.contains(x) && self.w.node(*x).is_some()));
            return (kid_ids, cont_ids);
        }
        (kid_ids, cont_ids)
    }

    /// Where placer `placer`'s first continuation after `anchor` goes: past
    /// `base` (the anchor's subtree end), the follow nodes other placers
    /// keep after the anchor come first, an earlier placer's before a
    /// later one's, then the anchor's own continuations (as a cold build
    /// places them: a page fired at a cut before the text resumes). Two
    /// placers wanting the same slot otherwise move each other's nodes on
    /// every rerun.
    fn after_conts(&self, anchor: Id, base: Id, placer: Id, place: Place) -> Id {
        let ppos = self.w.node(placer).map_or(0, |p| p.pos);
        let mut base = base;
        while let Some(y) = self.w.next(base) {
            let Some(yn) = self.w.node(y) else { break };
            let skip = yn.place == Place::After(anchor)
                && yn.parent != placer
                && match place {
                    // (The anchor's own: after every follow node there.)
                    Place::Cont => true,
                    _ => self.w.node(yn.parent).is_some_and(|q| q.pos < ppos),
                };
            if !skip {
                break;
            }
            base = self.extent_end(y);
        }
        base
    }

    /// The last node of `x`'s extent: its subtree, the follow nodes other
    /// placers keep after it, then its continuations' extents (they follow
    /// it at its level).
    pub(crate) fn extent_end(&self, x: Id) -> Id {
        let mut x = x;
        loop {
            let mut e = self.w.subtree_end(x);
            while let Some(y) = self.w.next(e).filter(|&y| self.w.node(y).is_some_and(|yn| yn.place == Place::After(x))) {
                super::core::prof::count(41);
                e = self.extent_end(y);
            }
            let Some(ep) = self.w.node(e).map(|n| n.pos) else { return e };
            let last = self.w.node(x).and_then(|n| n.res.as_ref()).and_then(|r| {
                r.conts
                    .iter()
                    .rev()
                    .inspect(|_| super::core::prof::count(42))
                    .find(|&&c| self.w.node(c).is_some_and(|cn| cn.place == Place::Cont && cn.parent == x && cn.pos > ep))
                    .copied()
            });
            match last {
                Some(c) => x = c,
                None => return e,
            }
        }
    }

    /// Puts placed node `x` (made by `parent`) right after node `after`
    /// (its exact predecessor), making it if new and retexting it if its
    /// text changed; returns the last node of its extent.
    fn put_placed(&mut self, parent: Id, pn: &Node, x: Id, spec: &Spec, depth: u32, after: Id) -> Id {
        super::core::prof::count(38);
        let text = self.core.text(Some(x), spec.op, spec.args.clone());
        match self.w.node(x).cloned() {
            Some(old) => {
                // Kept: in place when still right after `after` (or it is
                // `after` itself: where it is).
                let right = after == x || self.w.next(after) == Some(x);
                if !right {
                    if super::core::debug() {
                        eprintln!("moved {x}: placed by {parent} after {after}, whose next is {:?}", self.w.next(after));
                    }
                    // Moved: made again at its new place (taken before it
                    // goes: `after` may go with it).
                    super::core::prof::count(40);
                    let p = self.w.node(after).map_or(0, |e| e.pos);
                    self.free(x);
                    return self.make_placed_at(parent, pn, x, text, spec.place, depth, p);
                }
                if !Arc::ptr_eq(&old.text, &text) && (old.text.op != text.op || old.text.args != text.args) {
                    self.retext(x, text);
                }
                self.extent_end(x)
            }
            None => {
                super::core::prof::count(39);
                self.make_placed(parent, pn, x, text, spec.place, depth, after)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_placed(&mut self, parent: Id, pn: &Node, x: Id, text: Arc<Text>, place: Place, depth: u32, after: Id) -> Id {
        let p = self.w.node(after).map_or(0, |e| e.pos);
        self.make_placed_at(parent, pn, x, text, place, depth, p)
    }

    /// As [`Self::make_placed`], right after position `p`.
    #[allow(clippy::too_many_arguments)]
    fn make_placed_at(&mut self, parent: Id, pn: &Node, x: Id, text: Arc<Text>, place: Place, depth: u32, p: Pos) -> Id {
        let u = pn.unit;
        let pos = self.positions_after(u, p, 1)[0];
        self.create(x, text, u, parent, place, depth, pos);
        self.frames_from(u, pos);
        match &mut self.hold {
            Some((hu, _, ids)) if *hu == u => ids.push(x),
            _ => self.file_declared(x),
        }
        x
    }

    // ---------------------------------------------------------------- reads

    /// Files node `id`'s committed reads (`res`): the entries its slot
    /// holds become exactly these (its fixed ones as a worker made them
    /// ahead, when made at the renumbering now), its scope and sibling
    /// windows as they ask.
    fn file_reads(&mut self, id: Id, n: &Node, res: &Res) {
        let fixed: Arc<[ReadKey]> = match res.filed.as_ref().filter(|(e, _)| *e == self.w.relabels) {
            Some((_, k)) => k.clone(),
            None => read_keys(&self.w, id, n, &res.reads, Which::Fixed).0.into(),
        };
        let (moving, agg) = read_keys(&self.w, id, n, &res.reads, Which::Moving);
        // (Ranges sort last: still sorted.)
        if moving.is_empty() {
            self.refile(id, &fixed);
        } else {
            let keys: Vec<ReadKey> = fixed.iter().copied().chain(moving).collect();
            self.refile(id, &keys);
        }
        let t = super::core::prof::start();
        self.reindex_agg(id, n, agg);
        super::core::prof::stop(30, t);
    }

    /// Node `id`'s filed entries made exactly `keys` (sorted, distinct):
    /// only those that differ are unfiled and filed.
    fn refile(&mut self, id: Id, keys: &[ReadKey]) {
        let w = &self.w;
        w.rd.with_slot(id, |s| {
            diff(s.keys(), keys, |k, on| index_key(w, id, k, on));
            *s = if keys.is_empty() { Slot::None } else { Slot::Keys(keys.into()) };
        });
    }

    /// Takes a result's reads out of the reader indexes.
    pub(crate) fn drop_reads(&mut self, id: Id, n: &Node, res: &Res) {
        self.refile(id, &[]);
        let (_, agg) = read_keys(&self.w, id, n, &res.reads, Which::Moving);
        self.index_agg(id, n, agg, false);
    }

    /// Files (`on`) or unfiles node `id`'s scope reader entry and sibling
    /// windows.
    fn index_agg(&mut self, id: Id, n: &Node, agg: ReadAgg, on: bool) {
        let u = n.unit;
        if agg.kind {
            if on {
                self.w.add_kind_reader(u, n.pos, id);
            } else {
                self.w.drop_kind_reader(u, n.pos, id);
            }
        }
        if let Some(r) = agg.fwd {
            self.w.set_window(false, u, n.pos, id, on.then_some(r));
        }
        if let Some(l) = agg.back {
            self.w.set_window(true, u, n.pos, id, on.then_some(l));
        }
    }

    /// Files node `id`'s scope reader entry and sibling windows (at its
    /// position) as `an` asks, writing only what is not already so (an
    /// entry at its position under its id is its own).
    fn reindex_agg(&mut self, id: Id, n: &Node, an: ReadAgg) {
        let u = n.unit;
        let has = self.w.is_kind_reader(u, n.pos, id);
        if an.kind != has {
            if an.kind {
                self.w.add_kind_reader(u, n.pos, id);
            } else {
                self.w.drop_kind_reader(u, n.pos, id);
            }
        }
        for (back, w) in [(false, an.fwd), (true, an.back)] {
            let now = self.w.window_of(back, u, n.pos, id);
            if now != w {
                self.w.set_window(back, u, n.pos, id, w);
            }
        }
    }

    // --------------------------------------------------------------- commit

    /// Commits node `id`'s run: placements, definitions, output; the
    /// readers whose answers changed are re-asked. Returns whether its
    /// output changed.
    pub(crate) fn commit(&mut self, id: Id, mut res: Res, kids: &[Spec], conts: &[Spec]) -> bool {
        let Some(n) = self.w.node(id).cloned() else { return false };
        // (The values it passed on, taken now: it read them in the world it
        // ran in, and its presence reads hold here.)
        if !res.passes.is_empty()
            && let Some((r, ..)) = passes_now(self.core, &self.w, id, &res, None)
        {
            res = r;
        }
        let old = n.res.clone();
        // Placements.
        let t = super::core::now();
        let (k, c) = super::core::prof::time(10, || self.place(id, kids, conts));
        self.counts.merge.place += super::core::ns_since(t);
        res.kids = k;
        res.conts = c;
        let n = self.w.node(id).cloned().expect("live");
        // Reads: the index follows the new log.
        let t = super::core::now();
        super::core::prof::time(11, || self.file_reads(id, &n, &res));
        res.filed = None;
        self.counts.merge.reads += super::core::ns_since(t);
        let t_defs = super::core::prof::start();
        let t = super::core::now();
        // Definitions: removed, added, changed.
        let old_defs: Vec<Def> = old.as_ref().map_or_else(Vec::new, |o| o.defs.clone());
        let decl = n.text.stat.defs.clone();
        let filed = n.text.stat.filed.clone();
        let declared = |f: Field, k: u8| decl.as_ref().is_some_and(|d| d.iter().any(|e| e.0 == f && e.1.entry_key() == k));
        for d in &old_defs {
            if !res.defs.iter().any(|e| e.f == d.f && e.loc.entry_key() == d.loc.entry_key()) {
                self.unfile(id, &n, d.f, d.loc);
                if declared(d.f, d.loc.entry_key()) {
                    // Declared but not made: absent (as made with no value
                    // would be); the entry goes.
                }
            }
        }
        // Declared entries this run did not make go, and predicted ones:
        // their readers are asked again. (One the last run made went
        // above, as made. A global entry and a local one share a key, not
        // an entry: matched exactly.)
        if let Some(dl) = filed.as_ref() {
            // (Each list's entries as a set, the lists long: a package
            // load's run makes hundreds.)
            let made: FxHashSet<(Field, Locality)> = res.defs.iter().chain(old_defs.iter()).map(|e| (e.f, e.loc)).collect();
            for &(f, loc) in dl.iter() {
                if !made.contains(&(f, loc)) {
                    self.unfile(id, &n, f, loc);
                }
            }
        }
        // Declarations the run refuted.
        let mut widen = false;
        match decl.as_ref() {
            Some(dl) => {
                let outside = res.defs.iter().filter(|d| !dl.iter().any(|e| e.0 == d.f && e.1.entry_key() == d.loc.entry_key())).count();
                self.counts.defs_refuted += outside as u64;
            }
            None => {
                let t = &n.text;
                if !n.wide && res.defs.iter().any(|d| !self.core.may_cover(&t.stat.may, t.op, &t.args, d.f)) {
                    self.counts.declarations_refuted += 1;
                    widen = true;
                }
            }
        }
        if let Some(sc) = n.text.stat.contribs.as_ref() {
            let mut a: Vec<(Field, ValueId)> = sc.to_vec();
            let mut b: Vec<(Field, ValueId)> = res.contribs.clone();
            a.sort_by_key(|x| (x.0.0.raw(), x.1.raw()));
            b.sort_by_key(|x| (x.0.0.raw(), x.1.raw()));
            if a != b {
                self.counts.static_contribs_refuted += 1;
            }
        }
        super::core::prof::stop(12, t_defs);
        self.counts.merge.defs += super::core::ns_since(t);
        // Contributions: the run's replace the old (or the declared ones).
        let t = super::core::now();
        super::core::prof::time(13, || self.swap_contribs(n.unit, n.pos, &contribs_of(&n), &res.contribs));
        self.counts.merge.contribs += super::core::ns_since(t);
        let t_defs = super::core::prof::start();
        let t = super::core::now();
        if let (Some(e), Some(d)) = (res.effect, n.text.stat.effect)
            && e != d
            && old.as_ref().and_then(|o| o.effect) != res.effect
        {
            self.counts.effects_refuted += 1;
        }
        let effect_changed = old.as_ref().and_then(|o| o.effect) != res.effect
            || old.as_ref().map_or(&[][..], |o| &o.kinds[..]) != &res.kinds[..];
        // (A result of another text than the last is a new output to a
        // reader of its placed output.)
        res.text = super::world::text_key(&n.text);
        let out_changed = old.as_ref().is_none_or(|o| o.out != res.out || o.text != res.text);
        let mut n2 = n.clone();
        n2.wide |= widen;
        n2.narrowed = None;
        n2.was = None;
        let new_defs = res.defs.clone();
        let claim = res.claim;
        let t_put = super::core::prof::start();
        n2.res = Some(Arc::new(res));
        if n2.st == St::Dirty {
            n2.st = St::Done;
        }
        self.w.put(id, n2.clone());
        super::core::prof::stop(36, t_put);
        // Scope entries filed where the old effect put them move with it.
        if effect_changed {
            let mut keys: Vec<(Field, Locality)> = old_defs.iter().map(|d| (d.f, d.loc)).collect();
            keys.extend(filed.iter().flat_map(|d| d.iter().copied()));
            keys.sort_by_key(|&(f, l)| (f.0.raw(), l.entry_key()));
            keys.dedup();
            for (f, loc) in keys {
                if matches!(loc, Locality::Scope | Locality::ScopeAt(_)) && self.def_frame(id, &n, loc) != self.def_frame(id, &n2, loc) {
                    self.unfile(id, &n, f, loc);
                }
            }
            // The frames first: the node's own frame may change with its
            // effect (a local entry filed in a stale frame could take the
            // slot of its own scope's).
            self.frames_from(n.unit, n.pos);
            n2 = self.w.node(id).cloned().expect("live");
        }
        for d in &new_defs {
            let prev = old_defs.iter().find(|e| e.f == d.f && e.loc.entry_key() == d.loc.entry_key());
            if let Some(p) = prev.filter(|p| p.loc != d.loc) {
                self.unfile(id, &n, p.f, p.loc);
            }
            // Against what is filed (an edit files declared entries as
            // pending), not against the old result.
            self.file_if(id, &n2, d.f, d.loc, Some(d.val), true);
        }
        super::core::prof::stop(12, t_defs);
        self.counts.merge.defs += super::core::ns_since(t);
        let t = super::core::now();
        super::core::prof::time(15, || {
            if out_changed {
                self.w.touch_key(Key::Out(id));
                for r in self.w.out_readers_of(id) {
                    self.recheck_out(r, id);
                }
            } else {
                // (The output its last result had: a reader that read
                // another while that result was not of its text, or not
                // there, is re-asked.)
                self.recheck_stale_out_readers(id);
            }
        });
        super::core::prof::time(14, || self.run_claimed(id, claim));
        self.counts.merge.outs += super::core::ns_since(t);
        self.woken.push(id);
        out_changed
    }

    /// Commits a stopped run's unfolding only (its children made, so the
    /// run can wait for them).
    /// Frees the placements `gone` of placer `id` (see [`passed`]; or a
    /// cycle through them broken).
    pub(crate) fn free_placed(&mut self, id: Id, gone: &[Id]) {
        for &k in gone {
            self.free(k);
        }
        let Some(mut n) = self.w.node(id).cloned() else { return };
        let mut res = n.res.as_deref().cloned().unwrap_or_default();
        res.kids.retain(|k| !gone.contains(k));
        res.conts.retain(|k| !gone.contains(k));
        n.res = Some(Arc::new(res));
        self.w.put(id, n);
    }

    /// Places node `id` (text `text`) beside placed node `at` as its
    /// placer placed `at` (`Engine::insert_placed`).
    pub(crate) fn insert_placed(&mut self, at: Id, before: bool, id: Id, text: Arc<Text>) {
        let Some(an) = self.w.node(at).cloned() else { return };
        let u = an.unit;
        let p = if before {
            self.w.unit(u).order.below(&an.pos).map_or(0, |(&p, _)| p)
        } else {
            let end = self.extent_end(at);
            self.w.node(end).map_or(an.pos, |e| e.pos)
        };
        let pos = self.positions_after(u, p, 1)[0];
        self.create(id, text, u, an.parent, an.place, an.depth, pos);
        self.frames_from(u, pos);
        self.file_declared(id);
        if an.parent == NONE {
            return;
        }
        let Some(mut pn) = self.w.node(an.parent).cloned() else { return };
        let mut res = pn.res.as_deref().cloned().unwrap_or_default();
        for l in [&mut res.kids, &mut res.conts] {
            if let Some(i) = l.iter().position(|&k| k == at) {
                l.insert(if before { i } else { i + 1 }, id);
            }
        }
        pn.res = Some(Arc::new(res));
        self.w.put(an.parent, pn);
    }

    /// Frees placed node `id` and drops it from its placer's placements
    /// (`Engine::remove_placed`).
    pub(crate) fn free_placement(&mut self, id: Id) {
        match self.w.node(id).map(|n| n.parent) {
            Some(p) if p != NONE && self.w.node(p).is_some() => self.free_placed(p, &[id]),
            _ => self.free(id),
        }
    }

    pub(crate) fn commit_unfolding(&mut self, id: Id, kids: &[Spec], conts: &[Spec]) {
        let Some(n) = self.w.node(id).cloned() else { return };
        let (k, c) = self.place_some(id, kids, conts, true);
        let mut n2 = self.w.node(id).cloned().unwrap_or(n);
        let mut res = n2.res.as_deref().cloned().unwrap_or_default();
        res.kids = k;
        res.conts = c;
        let res = Arc::new(res);
        if let Some(m) = &mut self.cont_memo {
            m.res = Arc::as_ptr(&res) as usize;
        }
        n2.res = Some(res);
        self.w.put(id, n2);
    }
}

/// What placer `id`, stopped on node `x`, still holds before `x` (in its
/// unit) and did not place again: its attempt is past it.
pub(crate) fn passed(w: &World, id: Id, x: Id) -> Vec<Id> {
    let (Some(n), Some(xn)) = (w.node(id), w.node(x)) else { return Vec::new() };
    let Some(res) = n.res.as_ref() else { return Vec::new() };
    res.kids
        .iter()
        .chain(&res.conts)
        .copied()
        .filter(|&k| w.awaiting.contains_key(&k) && w.node(k).is_some_and(|kn| kn.parent == id && kn.unit == xn.unit && kn.pos < xn.pos))
        .collect()
}

/// A node's contributions as filed: its run's, else its text's declared.
fn contribs_of(n: &Node) -> Vec<(Field, ValueId)> {
    match (&n.res, &n.text.stat.contribs) {
        // (Claimed: its facts went; released, it files its text's.)
        _ if matches!(n.st, St::Inert(_)) => Vec::new(),
        (Some(r), _) => r.contribs.clone(),
        (_, Some(c)) => c.to_vec(),
        _ => Vec::new(),
    }
}

/// The position a definition of locality `loc` by `n` is filed at.
fn def_pos(n: &Node, _loc: Locality) -> Pos {
    n.pos
}

/// The kind of the `k`-th frame node `n` opens: its run's kinds, else its
/// text's brackets' kind.
fn frame_kind(n: &Node, k: u8) -> Option<ValueId> {
    if let Some(r) = &n.res {
        if let Some(&x) = r.kinds.get(usize::from(k)) {
            return Some(x);
        }
        if let Some(&x) = r.kinds.last() {
            return Some(x);
        }
    }
    n.brackets().kind
}

/// Whether read `r` of node `me` still gets the answer it got.
pub(crate) fn holds(core: &Core, w: &World, me: Id, r: &Read) -> bool {
    holds_with(core, w, me, r, false)
}

/// What asking one reader's reads of one world looks up again and again
/// (as a run's `Ctx` keeps it): the last read point, the last frame chain,
/// the field cells.
#[derive(Default)]
pub(crate) struct Asker<'w> {
    point: Option<((Id, bool, bool), Option<(u32, Pos, FrameId, Id)>)>,
    chain: Option<((u32, FrameId), Vec<(FrameId, Pos)>)>,
    cells: FxHashMap<(u32, Field), Option<&'w super::world::FieldCell>>,
    /// The last field asked at the last point, and its value there (`Err`:
    /// a definer in work): reads of it through other paths ask no further.
    last: Option<(Field, Result<Option<ValueId>, ()>)>,
    /// The last window a path's fold read asked, and its fold now.
    range: LastRange,
}

/// The last window a fold read through a path asked (field, ends, past),
/// and its fold now (`None`: the window gone): reads of it through other
/// paths (a view's) fold no further.
pub(crate) type LastRange = Option<((Field, Id, Id, bool), Option<ValueId>)>;

/// As [`holds`] for fold read `r` through a path, the window the last one
/// asked folded once (`last`).
fn holds_range(core: &Core, w: &World, me: Id, r: &Read, last: &mut LastRange) -> bool {
    let Read::Range { f, from, to, val, past, path } = r else { return holds(core, w, me, r) };
    let k = (*f, *from, *to, *past);
    let now = match *last {
        Some((lk, now)) if lk == k => now,
        _ => {
            let now = range_now(core, w, me, *f, *from, *to, *past);
            *last = Some((k, now));
            now
        }
    };
    now.is_some_and(|now| now == *val || core.eval_path(*path, now) == core.eval_path(*path, *val))
}

/// The fold a read of `f` over the window (`from`, `to`, `past`) of node
/// `me` gets now (`None`: an end or the window gone).
fn range_now(core: &Core, w: &World, me: Id, f: Field, from: Id, to: Id, past: bool) -> Option<ValueId> {
    if [from, to].iter().any(|&x| x != NONE && w.node(x).is_none()) {
        return None;
    }
    let Some(mo) = core.acc(f) else {
        return Some(ValueId::UNIT);
    };
    let m = AccMeasure {
        m: &**mo,
        v: &core.values,
    };
    let (u, lo, hi) = w.window(me, from, to, past)?;
    Some(w.fold(&m, u, f, lo, hi))
}

/// The last path read asked by [`holds_paths`]: its field and point, and
/// the field's value there.
pub(crate) type LastUse = Option<((Field, Id, bool, bool), Option<ValueId>)>;

/// As [`holds`], a path read of the field at the point the last one asked
/// taking that one's value (a plain read asks as `holds`, nothing kept);
/// checked against [`holds`] under `CLEAN_KEPT_CHECK`.
pub(crate) fn holds_paths(core: &Core, w: &World, me: Id, r: &Read, memo: &mut (LastUse, LastRange)) -> bool {
    let last = &mut memo.0;
    let ok = match r {
        Read::Range { path, .. } if *path >= super::core::PATH0 => holds_range(core, w, me, r, &mut memo.1),
        Read::Use { f, at, after, own, val, path, .. } if *path >= super::core::PATH0 => {
            let k = (*f, *at, *after, *own);
            let v2 = match *last {
                Some((lk, v2)) if lk == k => Some(v2),
                _ => use_value(core, w, r),
            };
            match v2 {
                Some(v2) => {
                    *last = Some((k, v2));
                    v2 == *val || v2.map(|x| core.eval_path(*path, x)) == val.map(|x| core.eval_path(*path, x))
                }
                None => holds(core, w, me, r),
            }
        }
        _ => holds(core, w, me, r),
    };
    assert!(!memo_check() || ok == holds(core, w, me, r), "reader {me}: its read {r:?} asked with memo: {ok}");
    ok
}

/// The value use `r` gets now, for [`holds_paths`] to keep (`None`: not
/// one to keep: a loop-back field's, the point gone, a definer in work).
fn use_value(core: &Core, w: &World, r: &Read) -> Option<Option<ValueId>> {
    let Read::Use { f, at, after, own, .. } = r else { return None };
    if w.loops.get(f).is_some() {
        return None;
    }
    let (u, pos, fr, skip) = w.read_point(*at, *after, *own)?;
    match w.reaching(u, *f, pos, fr, skip) {
        Some((_, e)) => e.val.map(Some),
        None => Some(w.env_of(core, u, *f)),
    }
}

/// As [`holds`], with `a`'s lookups (of `w`, unchanged since).
pub(crate) fn holds_asked<'w>(core: &Core, w: &'w World, me: Id, r: &Read, a: &mut Asker<'w>) -> bool {
    let Read::Use { f, at, after, own, val, path, .. } = r else {
        if let Read::Range { path, .. } = r
            && *path >= super::core::PATH0
        {
            return holds_range(core, w, me, r, &mut a.range);
        }
        return holds_with(core, w, me, r, false);
    };
    if w.loops.get(f).is_some() {
        return false;
    }
    let key = (*at, *after, *own);
    let point = match &a.point {
        Some((k, p)) if *k == key => *p,
        _ => {
            let p = w.read_point(*at, *after, *own);
            a.point = Some((key, p));
            a.last = None;
            p
        }
    };
    let Some((u, pos, fr, skip)) = point else {
        return false;
    };
    if *path >= super::core::PATH0
        && let Some((g, got)) = a.last
        && g == *f
    {
        return match got {
            Err(()) => true,
            Ok(v2) => v2 == *val || (*path != 0 && v2.map(|x| core.eval_path(*path, x)) == val.map(|x| core.eval_path(*path, x))),
        };
    }
    let reach = match *a.cells.entry((u, *f)).or_insert_with(|| w.field(u, *f)) {
        None => None,
        Some(cell) => {
            if a.chain.as_ref().is_none_or(|c| c.0 != (u, fr)) {
                a.chain = Some(((u, fr), w.chain_starts(u, fr)));
            }
            World::reaching_in(&cell.defs, &a.chain.as_ref().expect("set").1, pos, skip)
        }
    };
    let v2 = match reach {
        Some((_, e)) => {
            // (A definer in work: the answer stands as a guess; one that
            // predicts it, its prediction is the answer.)
            let Some(v) = e.val.or_else(|| predicted(w, e.node, *f)) else {
                if *path >= super::core::PATH0 {
                    a.last = Some((*f, Err(())));
                }
                return true;
            };
            Some(v)
        }
        None => w.env_of(core, u, *f),
    };
    if *path >= super::core::PATH0 {
        a.last = Some((*f, Ok(v2)));
    }
    // (The value unchanged: any path's answer too.)
    if v2 == *val {
        return true;
    }
    *path != 0 && v2.map(|x| core.eval_path(*path, x)) == val.map(|x| core.eval_path(*path, x))
}

/// Result `res` with the values it passed on (`Res::passes`) taken
/// again, through the reads of fields `on` (or all): `None` if none
/// changed; else the result, and the targets changed (definitions; the
/// contributions; the output).
#[allow(clippy::type_complexity)]
fn passes_now(core: &Core, w: &World, me: Id, res: &Res, on: Option<&[Field]>) -> Option<(Res, Vec<(Field, Locality, ValueId)>, bool, bool)> {
    let mut new: Option<Res> = None;
    let (mut defs, mut contribs, mut out) = (Vec::new(), false, false);
    // (A composition's holes taken again: (composition, part, value,
    // root read, its value).)
    let mut ups = Vec::new();
    for (i, p) in res.passes.iter().enumerate() {
        ups.clear();
        let (x, raw) = if p.read & super::world::COMPOSED != 0 {
            let k = (p.read & !super::world::COMPOSED) as usize;
            let Some(x) = compose_now(core, w, me, res, k, on, &mut ups) else { continue };
            (x, None)
        } else {
            let Some(rd) = res.reads.get(p.read as usize) else { continue };
            let Some(f) = root_field(rd) else { continue };
            if on.is_some_and(|fs| !fs.contains(&f)) {
                continue;
            }
            let Some(raw) = root_now(core, w, me, rd) else { continue };
            let Some(x) = core.eval_path(p.path, raw) else { continue };
            (x, Some(raw))
        };
        if x == p.val {
            continue;
        }
        // (Passed with a part replaced: its part not there now, the node's
        // read of it re-runs it.)
        let y = match p.set {
            Some((sp, nv)) => match core.set_path(sp, x, nv) {
                Some(y) => y,
                None => continue,
            },
            None => x,
        };
        let r = new.get_or_insert_with(|| res.clone());
        r.passes[i].val = x;
        for (rx, raw) in raw.map(|raw| (p.read, raw)).into_iter().chain(ups.iter().map(|u| (u.3, u.4))) {
            match r.reads.get_mut(rx as usize) {
                Some(Read::Use { val, .. }) => *val = Some(raw),
                Some(Read::Range { val, .. }) => *val = raw,
                _ => {}
            }
        }
        for &(k, j, v, read, _) in &ups {
            if let Some(c) = r.composed.get_mut(k).and_then(|c| c.get_mut(j)) {
                *c = super::world::CPart::Hole { read, path: match *c {
                    super::world::CPart::Hole { path, .. } => path,
                    _ => 0,
                }, val: v };
            }
        }
        match p.to {
            super::world::PassTo::Def(f, loc) => {
                if let Some(d) = r.defs.iter_mut().find(|d| d.f == f && d.loc.entry_key() == loc.entry_key()) {
                    d.val = y;
                    defs.push((f, d.loc, y));
                }
            }
            super::world::PassTo::Contrib(f) => {
                if let Some(c) = r.contribs.iter_mut().find(|c| c.0 == f) {
                    c.1 = x;
                    contribs = true;
                }
            }
            super::world::PassTo::Out => {
                r.out = x;
                out = true;
            }
        }
    }
    new.map(|r| (r, defs, contribs, out))
}

/// Composition `k` of result `res` now (`Ctx::compose`): its holes on
/// fields `on` (or all) taken again, those that changed added to `ups`
/// as (composition, part, value, root read, its value); `None` if one's
/// answer is not there.
fn compose_now(core: &Core, w: &World, me: Id, res: &Res, k: usize, on: Option<&[Field]>, ups: &mut Vec<(usize, usize, ValueId, u32, ValueId)>) -> Option<ValueId> {
    let parts = res.composed.get(k)?;
    let mut xs = Vec::with_capacity(parts.len());
    for (j, part) in parts.iter().enumerate() {
        match *part {
            super::world::CPart::Val(x) => xs.push(x),
            super::world::CPart::Hole { read, path, val } => {
                let rd = res.reads.get(read as usize)?;
                let f = root_field(rd)?;
                if on.is_some_and(|fs| !fs.contains(&f)) {
                    xs.push(val);
                    continue;
                }
                // (Its definer in work: the old value stands, the definer's
                // commit refiling it. The others are taken now: a later
                // refile asks only of its own fields.)
                let Some(raw) = root_now(core, w, me, rd) else {
                    xs.push(val);
                    continue;
                };
                let x = core.eval_path(path, raw)?;
                if x != val {
                    ups.push((k, j, x, read, raw));
                }
                xs.push(x);
            }
            super::world::CPart::Nested(i) => xs.push(compose_now(core, w, me, res, i as usize, on, ups)?),
        }
    }
    Some(core.values.list(xs))
}

/// The field a pass's root read reads (a use or a fold), if one passes.
fn root_field(r: &Read) -> Option<Field> {
    match r {
        Read::Use { f, .. } | Read::Range { f, .. } => Some(*f),
        _ => None,
    }
}

/// The value a pass's root read gets now, raw: a use's
/// ([`use_now`]), or a fold's over its window ([`range_now`]; `None`: the
/// window gone, its read broken).
fn root_now(core: &Core, w: &World, me: Id, r: &Read) -> Option<ValueId> {
    match r {
        Read::Range { f, from, to, past, .. } => range_now(core, w, me, *f, *from, *to, *past),
        _ => use_now(core, w, r),
    }
}

/// The raw value use `r` gets now (`None`: none, a loop-back field's, or
/// a definer's in work).
fn use_now(core: &Core, w: &World, r: &Read) -> Option<ValueId> {
    let Read::Use { f, at, after, own, .. } = r else { return None };
    if w.loops.get(f).is_some() {
        return None;
    }
    let (u, pos, fr, skip) = w.read_point(*at, *after, *own)?;
    match w.reaching(u, *f, pos, fr, skip) {
        Some((_, e)) => e.val,
        None => w.env_of(core, u, *f),
    }
}

/// The value node `x` predicts for `f` (`Operation::predicted_defs`).
fn predicted(w: &World, x: Id, f: Field) -> Option<ValueId> {
    w.node(x)?.text.stat.preds.as_ref()?.iter().find(|p| p.0 == f).map(|p| p.1)
}

/// Whether read `r` of node `me` still holds; `strict`: a definition still
/// to be made between its site and the reader breaks it (else the read
/// stands as a guess until that definer commits).
pub(crate) fn holds_with(core: &Core, w: &World, me: Id, r: &Read, strict: bool) -> bool {
    match r {
        Read::Use {
            f,
            at,
            after,
            own,
            site,
            val,
            path,
        } => {
            if let Some(&lv) = w.loops.get(f) {
                let _ = lv;
                return false;
            }
            let Some((u, pos, fr, skip)) = w.read_point(*at, *after, *own) else {
                return false;
            };
            let reach = w.reaching(u, *f, pos, fr, skip);
            if strict && w.pending_set_between(core, u, *f, reach.map_or(0, |r| r.0), pos, me).is_some() {
                return false;
            }
            let (s2, v2) = match reach {
                Some((_, e)) => match e.val {
                    Some(v) => (e.node, Some(v)),
                    // A definer in work: the answer stands as a guess until
                    // it commits (its commit re-asks the readers), but for
                    // its prediction, the answer a read gets now.
                    None => match predicted(w, e.node, *f) {
                        Some(p) if !strict => (e.node, Some(p)),
                        _ => return !strict,
                    },
                },
                None => (NONE, w.env_of(core, u, *f)),
            };
            let _ = (me, s2, site);
            // (The value unchanged: any path's answer too.)
            if v2 == *val {
                return true;
            }
            *path != 0 && v2.map(|x| core.eval_path(*path, x)) == val.map(|x| core.eval_path(*path, x))
        }
        Read::Loop { f, val } => w.loops.get(f) == Some(val),
        Read::Env { f, val } => core.env_get(w, *f).flatten() == *val,
        Read::Out { node, val } => w.node(*node).and_then(Node::out) == *val,
        Read::Placed { node, op, args, val, or } => w.node(*node).and_then(|n| n.placed_out(*op, args)).or(*or) == *val,
        Read::Kind { at, val } => w.kind_at(*at) == *val,
        Read::IsLoop { f, val } => core.is_loop(*f) == *val,
        Read::At { node, there } => w.node(*node).is_some() == *there,
        Read::Sib { i, ans } => {
            let got = w.forward(me, |_, x| w.taken_from(me, x)).nth(*i as usize);
            got.map(|x| (x, super::claims::key_of(w, x))) == *ans
        }
        Read::Claimed { list } => {
            let now: Vec<(Id, usize)> = w
                .claims
                .get(&me)
                .map(|st| st.taken.iter().map(|&x| (x, super::claims::key_of(w, x))).collect())
                .unwrap_or_default();
            now == *list
        }
        Read::Prev { walk, ans } => {
            let mut it = w.backward(me);
            for &(x, k, inert) in walk {
                match it.next() {
                    Some((y, i2)) if y == x && i2 == inert && super::claims::key_of(w, y) == k => {}
                    _ => return false,
                }
            }
            // No match: the walk still ends where it did.
            ans.is_some() || it.next().is_none()
        }
        Read::Seq { f, prev, val } => w.seq_read(core, me, *f, *prev).ok() == Some(*val),
        Read::Stream { outs, end, .. } => match w.stream(core, me) {
            Ok((o, e, _)) => o == *outs && e == *end,
            Err(_) => false,
        },
        Read::Range { f, from, to, val, past, path } => {
            // (An end gone: the window it named is not there.)
            if [*from, *to].iter().any(|&x| x != NONE && w.node(x).is_none()) {
                return false;
            }
            // (Through a path: its answers equal.)
            let same = |now: ValueId| now == *val || (*path != 0 && core.eval_path(*path, now) == core.eval_path(*path, *val));
            let Some(mo) = core.acc(*f) else {
                return same(ValueId::UNIT);
            };
            let m = AccMeasure {
                m: &**mo,
                v: &core.values,
            };
            let Some((u, lo, hi)) = w.window(me, *from, *to, *past) else {
                return false;
            };
            same(w.fold(&m, u, *f, lo, hi))
        }
    }
}

/// The field read `r` reads, if it reads one.
fn read_field(r: &Read) -> Option<Field> {
    match r {
        Read::Use { f, .. } | Read::Loop { f, .. } | Read::Env { f, .. } | Read::Range { f, .. } | Read::Seq { f, .. } | Read::IsLoop { f, .. } => Some(*f),
        _ => None,
    }
}

/// Whether use read `r` now reaches another site.
pub(crate) fn rebound(core: &Core, w: &World, r: &Read) -> bool {
    let _ = core;
    let Read::Use { f, at, after, own, site, .. } = r else {
        return false;
    };
    if w.loops.get(f).is_some() {
        return false;
    }
    let Some((u, pos, fr, skip)) = w.read_point(*at, *after, *own) else {
        return true;
    };
    let now = w.reaching(u, *f, pos, fr, skip).map_or(NONE, |(_, e)| e.node);
    now != *site
}

/// Why read `r` of node `me` no longer holds.
pub(crate) fn why_broken(core: &Core, w: &World, me: Id, r: &Read) -> crate::api::Rerun {
    use crate::api::Rerun;
    match r {
        Read::Use { f, at, after, own, val, .. } => {
            let new = w
                .read_point(*at, *after, *own)
                .and_then(|(u, pos, fr, skip)| match w.reaching(u, *f, pos, fr, skip) {
                    Some((_, e)) => e.val,
                    None => w.env_of(core, u, *f),
                })
                .unwrap_or(ValueId::UNIT);
            Rerun::Changed {
                field: *f,
                old: val.unwrap_or(ValueId::UNIT),
                new,
            }
        }
        Read::Loop { f, .. } => Rerun::Loop { field: *f },
        Read::Env { f, .. } => Rerun::Environment { field: *f },
        Read::Out { node, .. } | Read::Placed { node, .. } => Rerun::Output {
            of: crate::api::NodeId(*node),
        },
        Read::Kind { .. } => Rerun::Scope,
        Read::Sib { .. } | Read::Claimed { .. } | Read::Stream { .. } => Rerun::Claim,
        Read::Prev { .. } => Rerun::SiblingScan,
        Read::Range { f, .. } => Rerun::Range { field: Some(*f) },
        Read::Seq { f, .. } => Rerun::Changed {
            field: *f,
            old: ValueId::UNIT,
            new: w.seq_read(core, me, *f, false).ok().flatten().unwrap_or(ValueId::UNIT),
        },
        Read::At { node, .. } => Rerun::Anchor {
            of: crate::api::NodeId(*node),
        },
        Read::IsLoop { .. } => Rerun::Other,
    }
}

/// A new unit's record.
pub(crate) fn new_unit(w: &mut World, u: u32, app: Id) {
    let mut un = Unit {
        app,
        ..Unit::default()
    };
    un.frames.insert(
        FrameId::root(u),
        Frame {
            parent: FrameId::root(u),
            depth: 0,
            start: 0,
            kind: None,
        },
        &NoMeasure,
    );
    w.units.insert(u, Arc::new(un), &NoMeasure);
}

/// Whether read `r` is one of fields `fs` (a change to their definitions
/// or contributions can break no other read).
/// Whether a pending entry of `f` filed breaks no read: a definition read
/// reaching it stands as a guess until its definer commits (which re-asks
/// it), one reaching past it is as it was, and no other read of `f` asks
/// its definitions (an accumulator's or a sequence's would).
pub(crate) fn pending_breaks_none(core: &Core, f: Field) -> bool {
    core.acc(f).is_none() && !core.seqs.contains(&f)
}

/// Whether read `r` is of the output of one of nodes `xs`.
fn read_of(r: &Read, xs: &[Id]) -> bool {
    match r {
        Read::Out { node, .. } | Read::Placed { node, .. } => xs.contains(node),
        Read::Stream { outs, .. } => outs.iter().any(|(x, _)| xs.contains(x)),
        _ => false,
    }
}

/// [`read_on`] on a set of fields.
fn read_on_set(r: &Read, fs: &FxHashSet<Field>) -> bool {
    match r {
        Read::Use { f, .. }
        | Read::Loop { f, .. }
        | Read::Env { f, .. }
        | Read::IsLoop { f, .. }
        | Read::Range { f, .. }
        | Read::Seq { f, .. } => fs.contains(f),
        Read::Stream { fields, .. } => fields.iter().any(|(f, _)| fs.contains(f)),
        _ => false,
    }
}

fn read_on(r: &Read, fs: &[Field]) -> bool {
    match r {
        Read::Use { f, .. }
        | Read::Loop { f, .. }
        | Read::Env { f, .. }
        | Read::IsLoop { f, .. }
        | Read::Range { f, .. }
        | Read::Seq { f, .. } => fs.contains(f),
        Read::Stream { fields, .. } => fields.iter().any(|(f, _)| fs.contains(f)),
        _ => false,
    }
}

/// Whether read `r` walks the structure (a window, siblings, claims, the
/// stream, a sequence, an output, a kind): a change of structure in a
/// window its reader reads may change it. A use of a field changes only
/// through a definition, re-asked as one.
fn structural(r: &Read) -> bool {
    !matches!(r, Read::Use { .. } | Read::Loop { .. } | Read::Env { .. } | Read::IsLoop { .. })
}

/// Whether `CLEAN_SCOPED_CHECK` asks settle to check every read of a
/// reader re-asked on a field (read once; `profile` builds only).
#[inline]
pub(crate) fn scoped_check() -> bool {
    #[cfg(feature = "profile")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("CLEAN_SCOPED_CHECK").is_some())
    }
    #[cfg(not(feature = "profile"))]
    false
}


/// Which of a read log's reader entries to make.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Which {
    /// Those at fixed places (fields at positions, outputs): what a
    /// worker files ahead of the merge (positions move only with a
    /// renumbering).
    Fixed,
    /// The rest: range windows, the scope entry and sibling windows
    /// (extents that move with structure, its own placements' too).
    Moving,
}

/// The reader entries (`which` of them) of read log `reads` of node `id`
/// in world `w`, sorted and distinct, and its scope and sibling windows.
fn read_keys(w: &World, id: Id, n: &Node, reads: &super::world::Reads, which: Which) -> (Vec<ReadKey>, ReadAgg) {
    // (Most reads are at the reader itself: its position asked once.)
    let mut last_at: Option<(Id, Pos)> = None;
    let mut read_pos = |at: Id, n: &Node| match last_at {
        Some((a, p)) if a == at => p,
        _ => {
            let p = read_pos(w, at, n);
            last_at = Some((at, p));
            p
        }
    };
    let _t = super::core::prof::Span::new(25);
    if super::core::prof::on() {
        super::core::prof::add(26, reads.len() as u64);
    }
    let (fixed, moving) = (which == Which::Fixed, which == Which::Moving);
    let u = n.unit;
    let mut keys = Vec::with_capacity(if fixed { reads.len() } else { 0 });
    let mut agg = ReadAgg::default();
    // Just past node `x`'s subtree (a change there is in the window).
    let past = |x: Id| -> Pos {
        let e = w.subtree_end(x);
        w.node(e).and_then(|en| w.unit(en.unit).order.above(&en.pos).map(|(&p, _)| p)).unwrap_or(Pos::MAX)
    };
    for r in reads {
        match r {
            Read::Use { f, at, .. } => {
                if fixed {
                    keys.push(ReadKey::Field(u, *f, read_pos(*at, n)));
                }
                // (Its answer follows the frame chain it reads through.)
                agg.kind |= moving;
            }
            Read::Out { node, .. } | Read::Placed { node, .. } => {
                if fixed {
                    keys.push(ReadKey::Out(*node));
                }
            }
            Read::At { node, .. } => {
                if fixed {
                    keys.push(ReadKey::Anchor(*node));
                }
            }
            Read::Kind { .. } => agg.kind |= moving,
            Read::Range { f, from, to, past, .. } => {
                if moving && let Some((ru, lo, hi)) = w.window(id, *from, *to, *past) {
                    keys.push(ReadKey::Range(ru, *f, lo, hi));
                }
                // (Its anchors going moves its window.)
                if fixed {
                    for x in [*from, *to] {
                        if x != NONE {
                            keys.push(ReadKey::Anchor(x));
                        }
                    }
                }
            }
            Read::Seq { f, .. } => {
                if fixed {
                    keys.push(ReadKey::Field(u, *f, n.pos));
                }
                agg.kind |= moving;
            }
            Read::Sib { ans, .. } if moving => {
                let r = ans.map_or(Pos::MAX, |(x, _)| past(x));
                agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
            }
            Read::Claimed { list } if moving => {
                let r = list.last().map_or(Pos::MAX, |&(x, _)| past(x));
                agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
            }
            Read::Prev { ans, .. } if moving => {
                let l = ans.and_then(|x| w.node(x)).map_or(0, |x| x.pos);
                agg.back = Some(agg.back.map_or(l, |b| b.min(l)));
            }
            Read::Stream { outs, end, fields } => {
                if moving {
                    let r = end.map_or(Pos::MAX, past);
                    agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
                }
                if fixed {
                    keys.extend(outs.iter().map(|&(x, _)| ReadKey::Out(x)));
                    for &(f, x) in fields {
                        if let Some(xn) = w.node(x) {
                            keys.push(ReadKey::Field(xn.unit, f, xn.pos));
                        }
                    }
                }
            }
            Read::Sib { .. } | Read::Claimed { .. } | Read::Prev { .. } | Read::Loop { .. } | Read::Env { .. } | Read::IsLoop { .. } => {}
        }
    }
    // (Sorted and distinct: filed once each, diffed by merging.)
    keys.sort_unstable();
    let n0 = keys.len();
    keys.dedup();
    if super::core::prof::on() {
        super::core::prof::add(27, n0 as u64);
        super::core::prof::add(28, (n0 - keys.len()) as u64);
    }
    (keys, agg)
}

/// The position a use read is filed at: its node's (a read after a
/// subtree at the node, conservatively: any change after it re-asks).
fn read_pos(w: &World, at: Id, n: &Node) -> Pos {
    w.node(at).map_or(n.pos, |x| x.pos)
}

/// Files (`on`) or unfiles one reader entry of node `id`.
fn index_key(w: &World, id: Id, k: ReadKey, on: bool) {
    super::core::prof::count(29);
    let _t = super::core::prof::Span::new(match k {
        ReadKey::Field(..) => 27,
        ReadKey::Out(_) | ReadKey::Anchor(_) => 28,
        ReadKey::Range(..) => 29,
    });
    match (k, on) {
        (ReadKey::Field(u, f, pos), true) => w.add_reader(u, f, pos, id),
        (ReadKey::Field(u, f, pos), false) => w.drop_reader(u, f, pos, id),
        (ReadKey::Out(x), true) => w.add_out_reader(x, id),
        (ReadKey::Out(x), false) => w.drop_out_reader(x, id),
        (ReadKey::Anchor(x), true) => w.add_anchor_reader(x, id),
        (ReadKey::Anchor(x), false) => w.drop_anchor_reader(x, id),
        (ReadKey::Range(ru, f, lo, hi), true) => w.add_range_reader(ru, f, lo, hi, id),
        (ReadKey::Range(ru, f, lo, hi), false) => w.drop_range_reader(ru, f, lo, hi, id),
    }
}

/// Walks sorted, distinct `old` and `new` together: `step(k, false)` for
/// each entry only in `old`, `step(k, true)` for each only in `new`.
fn diff(old: &[ReadKey], new: &[ReadKey], mut step: impl FnMut(ReadKey, bool)) {
    let (mut a, mut b) = (0, 0);
    while a < old.len() || b < new.len() {
        match (old.get(a), new.get(b)) {
            (Some(x), Some(y)) if x == y => {
                a += 1;
                b += 1;
            }
            (Some(&x), y) if y.is_none_or(|y| x < *y) => {
                step(x, false);
                a += 1;
            }
            (_, Some(&y)) => {
                step(y, true);
                b += 1;
            }
            _ => unreachable!("one side left"),
        }
    }
}

/// A worker's filing of its run's reads (`reads`, of node `id`, run on
/// world `w`) ahead of the merge: the entries at fixed places it lacks
/// are added to the node's (it may hold more for a while: a run not
/// committed; never fewer), unless a renumbering came since `w` was
/// taken. The entries made, with the renumbering they were made at, for
/// the merge to file the committed run by.
pub(crate) fn file_ahead(w: &World, id: Id, reads: &super::world::Reads) -> Option<(u64, Arc<[ReadKey]>)> {
    let n = w.node(id)?;
    let epoch = w.rd.relabels();
    if *epoch != w.relabels {
        return None;
    }
    let (keys, _) = read_keys(w, id, n, reads, Which::Fixed);
    w.rd.with_slot(id, |s| {
        if matches!(s, Slot::Gone) {
            return;
        }
        let mut all = Vec::with_capacity(s.keys().len().max(keys.len()));
        diff(s.keys(), &keys, |k, on| {
            if on {
                index_key(w, id, k, true);
            }
        });
        // (Both sorted: their union.)
        let (old, mut b) = (s.keys(), 0);
        for &k in old {
            while b < keys.len() && keys[b] < k {
                all.push(keys[b]);
                b += 1;
            }
            if b < keys.len() && keys[b] == k {
                b += 1;
            }
            all.push(k);
        }
        all.extend_from_slice(&keys[b..]);
        if !all.is_empty() {
            *s = Slot::Keys(all.into());
        }
    });
    Some((*epoch, keys.into()))
}

/// A read log's scope reader entry and sibling windows.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
struct ReadAgg {
    kind: bool,
    fwd: Option<Pos>,
    back: Option<Pos>,
}

/// The end (exclusive) of the readers a change to `f`'s entry at `at`
/// can reach: a read takes the nearest earlier entry of its frame chain
/// or the globals, so past the next global (and, for a local entry, the
/// next one in its frame) none sees it; that entry's own node still may
/// (a node passes its own entries). A sequence's readers fold every
/// entry: unbounded.
pub(crate) fn shadow_end(core: &Core, fd: Option<&super::world::FieldDefs>, f: Field, at: Pos, fr: FrameId, global: bool) -> Pos {
    let Some(fd) = fd else { return Pos::MAX };
    if core.seqs.contains(&f) {
        return Pos::MAX;
    }
    let mut end = fd.globals.above(&at).map_or(Pos::MAX, |(&p, _)| p);
    if !global && let Some(m) = fd.local.get(&fr) {
        end = end.min(m.above(&at).map_or(Pos::MAX, |(&p, _)| p));
    }
    end.saturating_add(1)
}
