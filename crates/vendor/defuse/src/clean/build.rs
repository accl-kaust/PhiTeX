//! Making a new world from an old one: node creation and removal, placement,
//! frames, definition filing, a run's result applied, and the readers whose
//! answers changed marked dirty.

use std::sync::Arc;

use super::core::Core;
use super::ctx::Spec;
use super::ptree::{Count, NoMeasure, PMap};
use super::world::{ReadKey, 
    AccMeasure, Def, Entry, Frame, FrameId, Id, Key, NONE, Node, Place, Pos, Read, Res, St, Text, Unit, World,
};
use crate::api::Locality;
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, hash_words};

/// The label gap between two pushed nodes.
pub(crate) const PUSH_GAP: Pos = 1 << 40;

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
    /// Readers past a shadow bound, asked under CLEAN_SCOPED_CHECK only.
    beyond: Vec<(Id, Field)>,
    /// Readers made dirty because a use now reaches another site.
    pub rebinds: u64,
    /// Claims wait for the evaluation (a client's edits).
    pub defer_claims: bool,
    /// A step's merge: the field writes recorded, not made
    /// ([`super::batch`]).
    pub defer: Option<super::batch::Pending>,
    /// While a run's placements are made: the unit, the lowest position
    /// whose frames are to be walked again, and the nodes whose declared
    /// definitions are to be filed once they are.
    hold: Option<(u32, Pos, Vec<Id>)>,
    pub counts: Counts,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(core: &'a Core, w: World) -> Builder<'a> {
        let mut w = w;
        w.ver += 1;
        Builder {
            core,
            w,
            dirty: Vec::new(),
            created: 0,
            freed: Vec::new(),
            woken: Vec::new(),
            recheck: FxHashMap::default(),
            beyond: Vec::new(),
            rebinds: 0,
            defer_claims: false,
            defer: None,
            hold: None,
            counts: Counts::default(),
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
        self.recheck.insert(r, None);
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

    /// A field reader filed (`on`) or not.
    fn reader(&mut self, u: u32, f: Field, pos: Pos, id: Id, on: bool) {
        if let Some(d) = &mut self.defer {
            d.push(u, f, super::batch::FieldOp::Reader { pos, id, on });
        } else if on {
            self.w.add_reader(u, f, pos, id);
        } else {
            self.w.drop_reader(u, f, pos, id);
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
        let ids: Vec<(Id, Option<Vec<Field>>)> = std::mem::take(&mut self.recheck).into_iter().collect();
        for (id, on) in ids {
            let Some(n) = self.w.node(id) else { continue };
            if n.st != St::Done {
                continue;
            }
            let Some(res) = n.res.clone() else { continue };
            let asked = |r: &Read| {
                let a = on.as_ref().is_none_or(|fs| read_on(r, fs));
                if a {
                    super::core::prof::count(20);
                }
                a
            };
            let mut broken = res.reads.iter().find(|r| asked(r) && !holds(self.core, &self.w, id, r));
            // (CLEAN_SCOPED_CHECK: a read of another field broken too is
            // an invalidation missed elsewhere; named, and taken.)
            if broken.is_none() && on.is_some() && scoped_check() {
                broken = res.reads.iter().find(|r| !holds(self.core, &self.w, id, r));
                if let Some(r) = broken {
                    eprintln!("scoped check: {id} asked on {on:?}, broken {r:?}");
                }
            }
            if let Some(r) = broken {
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
    /// its placements to be made again.
    fn stale_placer(&mut self, id: Id, n: &Node) {
        if n.res.as_ref().is_some_and(|r| !r.kids.is_empty() || !r.conts.is_empty()) {
            self.w.stale.insert(id, (), &NoMeasure);
        }
    }

    // ------------------------------------------------------------ positions

    /// `k` positions strictly between `a` and `b`, evenly spread; `None`
    /// when the gap is too small.
    fn spread(a: Pos, b: Pos, k: usize) -> Option<Vec<Pos>> {
        let k = k as u64;
        if b <= a || b - a <= k {
            return None;
        }
        let step = (b - a) / (k + 1);
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
        let hi = self.next_pos(u, after);
        let hi = if hi == Pos::MAX {
            after.saturating_add(PUSH_GAP * (k as u64 + 1))
        } else {
            hi
        };
        if let Some(v) = Self::spread(after, hi, k) {
            return v;
        }
        let after = self.relabel(u, after);
        let hi = self.next_pos(u, after);
        let hi = if hi == Pos::MAX { after + PUSH_GAP * (k as u64 + 1) } else { hi };
        Self::spread(after, hi, k).expect("a renumbered unit has room")
    }

    /// Renumbers unit `u` evenly; returns the new position of what was at
    /// `keep`.
    fn relabel(&mut self, u: u32, keep: Pos) -> Pos {
        self.w.relabels += 1;
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
        // Nodes.
        for &(q, id) in &order {
            if let Some(mut nd) = self.w.node(id).cloned() {
                nd.pos = q;
                self.w.put(id, nd);
            }
        }
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
            let mut readers: Vec<((Pos, Id), ())> = c.readers.iter().map(|(&(p, id), ())| ((remap(p), id), ())).collect();
            readers.sort_by_key(|x| x.0);
            readers.dedup_by_key(|x| x.0);
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
            let mut ranges: Vec<((Pos, Id), Pos)> = c.range_readers.iter().map(|(&(p, id), &lo)| ((remap(p), id), remap(lo))).collect();
            ranges.sort_by_key(|x| x.0);
            ranges.dedup_by_key(|x| x.0);
            self.w.with_field(u, f, |c| {
                c.defs = nf;
                c.readers = PMap::from_sorted(readers, &NoMeasure);
                c.contribs = contribs;
                c.range_readers = PMap::from_sorted(ranges, &NoMeasure);
            });
        }
        // Scope and sibling readers and the work of the unit: keys and
        // reaches.
        let re = |p: Pos| if p == Pos::MAX || p == 0 { p } else { remap(p) };
        self.w.with_unit(u, |c| {
            let mut v: Vec<((Pos, Id), ())> = c.kind_readers.iter().map(|(&(p, id), ())| ((remap(p), id), ())).collect();
            v.sort_by_key(|x| x.0);
            c.kind_readers = PMap::from_sorted(v, &NoMeasure);
            let mut v: Vec<(Pos, (Id, Pos))> = c.fwd_readers.iter().map(|(&p, &(id, r))| (remap(p), (id, re(r)))).collect();
            v.sort_by_key(|x| x.0);
            c.fwd_readers = PMap::from_sorted(v, &super::claims::MaxReach);
            let mut v: Vec<(Pos, (Id, Pos))> = c.back_readers.iter().map(|(&p, &(id, r))| (remap(p), (id, re(r)))).collect();
            v.sort_by_key(|x| x.0);
            c.back_readers = PMap::from_sorted(v, &super::claims::MinLow);
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
        let st = if text.stat.fin { St::Held } else { St::Dirty };
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
        };
        let statics = n.text.stat.contribs.clone();
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
        let Some(n) = self.w.node(id).cloned() else { return };
        let Some(decl) = n.text.stat.defs.clone() else { return };
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
        self.def_changed_in(n.unit, f, n.pos, Some(fr), loc == Locality::Global);
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
        for r in self.w.readers_between(u, f, at.saturating_sub(1), hi) {
            self.recheck_on(r, f);
        }
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
        self.w.stale.remove(&id, &NoMeasure);
        if let Some(res) = n.res.clone() {
            for &k in res.kids.iter().chain(&res.conts) {
                if self.w.node(k).is_some_and(|c| c.parent == id) {
                    self.free(k);
                }
            }
            self.drop_reads(id, &n, &res);
        }
        self.unfile_all(id, &n);
        self.claimer_gone(id, n.unit);
        self.w.unlink(n.unit, n.pos);
        self.w.drop_node(id);
        // Its readers and those of its position.
        if let Some(m) = self.w.out_readers.get(&id).cloned() {
            for (&r, ()) in m.iter() {
                self.recheck(r, 13);
            }
        }
        self.w.out_readers.remove(&id, &NoMeasure);
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
        self.shape_changed(u);
        let mut hit: Vec<Id> = Vec::new();
        if let Some(un) = self.w.unit_opt(u) {
            hit.extend(super::claims::stab_forward(&un.fwd_readers, p).into_iter().map(|e| e.1));
            hit.extend(super::claims::stab_backward(&un.back_readers, p).into_iter().map(|e| e.1));
        }
        for r in hit {
            self.recheck(r, 14);
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
        self.w.with_unit(u, |un| {
            un.env.insert(f, x, &NoMeasure);
        });
        self.def_changed(u, f, 0);
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
        if let Some(decl) = n.text.stat.defs.clone() {
            for &(f, loc) in decl.iter() {
                if !n.res.as_ref().is_some_and(|r| r.defs.iter().any(|d| d.f == f && d.loc.entry_key() == loc.entry_key())) {
                    self.unfile(id, n, f, loc);
                }
            }
        }
        for (f, _) in contribs_of(n) {
            self.contrib(n.unit, f, n.pos, None);
        }
    }

    /// Node `x`'s output is gone or changed: its readers are re-asked.
    pub(crate) fn out_gone(&mut self, x: Id) {
        self.w.touch_key(Key::Out(x));
        if let Some(m) = self.w.out_readers.get(&x).cloned() {
            for (&r, ()) in m.iter() {
                self.recheck(r, 15);
            }
        }
    }

    /// Replaces a node's text: it runs again; its declared definitions are
    /// filed anew.
    pub(crate) fn retext(&mut self, id: Id, text: Arc<Text>) {
        let Some(n) = self.w.node(id).cloned() else { return };
        let old_b = n.brackets();
        if let Some(decl) = n.text.stat.defs.clone() {
            for &(f, loc) in decl.iter() {
                if !n.res.as_ref().is_some_and(|r| r.defs.iter().any(|d| d.f == f && d.loc.entry_key() == loc.entry_key())) {
                    self.unfile(id, &n, f, loc);
                }
            }
        }
        let mut m = n.clone();
        m.text = text;
        m.wide = false;
        m.narrowed = None;
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
        if let Some((hu, hp, _)) = &mut self.hold
            && *hu == u
        {
            *hp = (*hp).min(from);
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
            self.shape_changed(u);
        }
        // The scope readers in the walked stretch are re-asked (also when
        // nothing moved: a frame that is gone leaves no trace in the walk).
        if let Some(m) = self.w.unit_opt(u).map(|un| &un.kind_readers) {
            let hit: Vec<Id> = m
                .range(std::ops::Bound::Included(&(from, 0)), std::ops::Bound::Included(&(walked, Id::MAX)))
                .map(|(&(_, r), ())| r)
                .collect();
            for r in hit {
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
        if let Some(decl) = &n.text.stat.defs {
            for &(f, loc) in decl.iter() {
                if !keys.iter().any(|e| e.0 == f && e.1.entry_key() == loc.entry_key()) {
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
    fn placed_id(parent: Id, place: Place, op: crate::api::OpId, ord: u32) -> Id {
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
        let u = self.w.node(id).map(|n| n.unit).expect("live");
        self.hold = Some((u, Pos::MAX, Vec::new()));
        let r = self.place_held(id, kids, conts);
        // The frames walked once, from the lowest placement; then the new
        // nodes' declared definitions filed in them.
        if let Some((u, p, ids)) = self.hold.take() {
            if p != Pos::MAX {
                self.frames_now(u, p);
            }
            for x in ids {
                self.file_declared(x);
            }
        }
        r
    }

    fn place_held(&mut self, id: Id, kids: &[Spec], conts: &[Spec]) -> (Vec<Id>, Vec<Id>) {
        let n = self.w.node(id).cloned().expect("live");
        let old: (Vec<Id>, Vec<Id>) = n.res.as_ref().map_or((Vec::new(), Vec::new()), |r| (r.kids.clone(), r.conts.clone()));
        let assign = |specs: &[Spec]| -> Vec<Id> {
            let mut ord: FxHashMap<(u64, crate::api::OpId, u64), u32> = FxHashMap::default();
            specs
                .iter()
                .map(|s| {
                    let key = match s.place {
                        Place::Child => (1, s.op, 0),
                        Place::Cont => (2, s.op, 0),
                        Place::After(a) => (3, s.op, a),
                        Place::Marker(o) => (4 + u64::from(o), s.op, 0),
                        Place::Top => (6, s.op, 0),
                    };
                    let k = ord.entry(key).or_insert(0);
                    *k += 1;
                    Self::placed_id(id, s.place, s.op, *k)
                })
                .collect()
        };
        let kid_ids = assign(kids);
        let cont_ids = assign(conts);
        // Gone: freed.
        let keep: FxHashSet<Id> = kid_ids.iter().chain(&cont_ids).copied().collect();
        for &x in old.0.iter().chain(&old.1) {
            if !keep.contains(&x) {
                self.free(x);
            }
        }
        // Kids: in order after `id` (each after the previous one's subtree).
        let mut after = id;
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
                // (After the follow nodes other placers keep there.)
                None => self.after_conts(anchor, self.w.subtree_end(anchor), id, spec.place),
            };
            let end = self.put_placed(id, &n, c, spec, depth, base);
            last_cont.insert(anchor, end);
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
                e = self.extent_end(y);
            }
            let Some(ep) = self.w.node(e).map(|n| n.pos) else { return e };
            let last = self.w.node(x).and_then(|n| n.res.as_ref()).and_then(|r| {
                r.conts
                    .iter()
                    .rev()
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
        let text = self.core.text(Some(x), spec.op, spec.args.clone());
        match self.w.node(x).cloned() {
            Some(old) => {
                // Kept: in place when still right after `after`.
                let right = self.w.next(after) == Some(x);
                if !right {
                    if super::core::debug() {
                        eprintln!("moved {x}: placed by {parent} after {after}, whose next is {:?}", self.w.next(after));
                    }
                    // Moved: made again at its new place.
                    self.free(x);
                    return self.make_placed(parent, pn, x, text, spec.place, depth, after);
                }
                if !Arc::ptr_eq(&old.text, &text) && (old.text.op != text.op || old.text.args != text.args) {
                    self.retext(x, text);
                }
                self.extent_end(x)
            }
            None => self.make_placed(parent, pn, x, text, spec.place, depth, after),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_placed(&mut self, parent: Id, pn: &Node, x: Id, text: Arc<Text>, place: Place, depth: u32, after: Id) -> Id {
        let u = pn.unit;
        let p = self.w.node(after).map_or(0, |e| e.pos);
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

    /// Registers a committed run's reads in the reader indexes; the
    /// entries filed.
    pub(crate) fn add_reads(&mut self, id: Id, n: &Node, reads: &[Read]) -> Arc<[ReadKey]> {
        let (keys, agg) = self.read_keys(id, n, reads);
        for &k in &keys {
            self.index_key(id, k, true);
        }
        let t = super::core::prof::start();
        self.index_agg(id, n, agg, true);
        super::core::prof::stop(30, t);
        keys.into()
    }

    /// Takes a result's reads out of the reader indexes.
    pub(crate) fn drop_reads(&mut self, id: Id, n: &Node, res: &Res) {
        let (keys, agg) = self.read_keys(id, n, &res.reads);
        let filed = res.filed.as_ref().filter(|(e, _)| *e == self.w.relabels);
        match filed {
            Some((_, ks)) => {
                for &k in ks.iter() {
                    self.index_key(id, k, false);
                }
            }
            None => {
                for &k in &keys {
                    self.index_key(id, k, false);
                }
            }
        }
        self.index_agg(id, n, agg, false);
    }

    /// Moves node `id`'s reader entries from result `old`'s to those of
    /// read log `new`: only the entries that differ are unfiled and filed
    /// (a run that reads what it read before touches no index); the
    /// entries filed.
    fn reindex_reads(&mut self, id: Id, n: &Node, old: &Res, new: &[Read]) -> Arc<[ReadKey]> {
        let (kn, an) = self.read_keys(id, n, new);
        if let Some((_, ko)) = old.filed.as_ref().filter(|(e, _)| *e == self.w.relabels) {
            // (As filed: both sorted.)
            let (mut a, mut b) = (0, 0);
            while a < ko.len() || b < kn.len() {
                match (ko.get(a), kn.get(b)) {
                    (Some(x), Some(y)) if x == y => {
                        a += 1;
                        b += 1;
                    }
                    (Some(&x), y) if y.is_none_or(|y| x < *y) => {
                        self.index_key(id, x, false);
                        a += 1;
                    }
                    (_, Some(&y)) => {
                        self.index_key(id, y, true);
                        b += 1;
                    }
                    _ => unreachable!("one side left"),
                }
            }
        } else {
            super::core::prof::count(30);
            let (ko, _) = self.read_keys(id, n, &old.reads);
            let ko: FxHashSet<ReadKey> = ko.into_iter().collect();
            let kns: FxHashSet<ReadKey> = kn.iter().copied().collect();
            for &k in ko.difference(&kns) {
                self.index_key(id, k, false);
            }
            // (An entry in both is filed if it is not there: one filed at a
            // position that moved since, as dropping and filing it again
            // would.)
            for &k in &kn {
                if !ko.contains(&k) || !self.has_key(id, k) {
                    self.index_key(id, k, true);
                }
            }
        }
        // (Windows hang on subtree extents that move: filed again.)
        let t = super::core::prof::start();
        self.reindex_agg(id, n, an);
        super::core::prof::stop(30, t);
        kn.into()
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

    /// Whether node `id`'s reader entry `k` is filed (deferred writes
    /// aside: a node's own are its commit's).
    fn has_key(&self, id: Id, k: ReadKey) -> bool {
        let _t = super::core::prof::Span::new(26);
        match k {
            ReadKey::Field(u, f, pos) => self.w.field(u, f).is_some_and(|c| c.readers.contains_key(&(pos, id))),
            ReadKey::Out(x) => self.w.out_readers.get(&x).is_some_and(|m| m.contains_key(&id)),
            ReadKey::Range(ru, f, lo, hi) => self.w.field(ru, f).and_then(|c| c.range_readers.get(&(hi, id))) == Some(&lo),
        }
    }

    /// The index entries of read log `reads` of node `id`, and its scope
    /// and sibling windows.
    fn read_keys(&self, id: Id, n: &Node, reads: &[Read]) -> (Vec<ReadKey>, ReadAgg) {
        let _t = super::core::prof::Span::new(25);
        if super::core::prof::on() {
            super::core::prof::add(26, reads.len() as u64);
        }
        let u = n.unit;
        let mut keys = Vec::with_capacity(reads.len());
        let mut agg = ReadAgg::default();
        // Just past node `x`'s subtree (a change there is in the window).
        let past = |w: &World, x: Id| -> Pos {
            let e = w.subtree_end(x);
            w.node(e).and_then(|en| w.unit(en.unit).order.above(&en.pos).map(|(&p, _)| p)).unwrap_or(Pos::MAX)
        };
        for r in reads {
            match r {
                Read::Use { f, at, after, .. } => {
                    keys.push(ReadKey::Field(u, *f, self.read_pos(*at, *after, n)));
                    // (Its answer follows the frame chain it reads through.)
                    agg.kind = true;
                }
                Read::Out { node, .. } | Read::At { node, .. } => keys.push(ReadKey::Out(*node)),
                Read::Kind { .. } => agg.kind = true,
                Read::Range { f, from, to, past, .. } => {
                    if let Some((ru, lo, hi)) = self.w.window(id, *from, *to, *past) {
                        keys.push(ReadKey::Range(ru, *f, lo, hi));
                    }
                    // (Its anchors going moves its window.)
                    for x in [*from, *to] {
                        if x != NONE {
                            keys.push(ReadKey::Out(x));
                        }
                    }
                }
                Read::Seq { f, .. } => {
                    keys.push(ReadKey::Field(u, *f, n.pos));
                    agg.kind = true;
                }
                Read::Sib { ans, .. } => {
                    let r = ans.map_or(Pos::MAX, |(x, _)| past(&self.w, x));
                    agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
                }
                Read::Claimed { list } => {
                    let r = list.last().map_or(Pos::MAX, |&(x, _)| past(&self.w, x));
                    agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
                }
                Read::Prev { ans, .. } => {
                    let l = ans.and_then(|x| self.w.node(x)).map_or(0, |x| x.pos);
                    agg.back = Some(agg.back.map_or(l, |b| b.min(l)));
                }
                Read::Stream { outs, end, fields } => {
                    let r = end.map_or(Pos::MAX, |e| past(&self.w, e));
                    agg.fwd = Some(agg.fwd.map_or(r, |f| f.max(r)));
                    keys.extend(outs.iter().map(|&(x, _)| ReadKey::Out(x)));
                    for &(f, x) in fields {
                        if let Some(xn) = self.w.node(x) {
                            keys.push(ReadKey::Field(xn.unit, f, xn.pos));
                        }
                    }
                }
                Read::Loop { .. } | Read::Env { .. } | Read::IsLoop { .. } => {}
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

    /// Files (`on`) or unfiles one reader entry of node `id`.
    fn index_key(&mut self, id: Id, k: ReadKey, on: bool) {
        super::core::prof::count(29);
        let _t = super::core::prof::Span::new(match k {
            ReadKey::Field(..) => 27,
            ReadKey::Out(_) => 28,
            ReadKey::Range(..) => 29,
        });
        match k {
            ReadKey::Field(u, f, pos) => self.reader(u, f, pos, id, on),
            ReadKey::Out(x) => {
                if on {
                    self.w.add_out_reader(x, id);
                } else {
                    self.w.drop_out_reader(x, id);
                }
            }
            ReadKey::Range(ru, f, lo, hi) => {
                if let Some(d) = &mut self.defer {
                    d.push(ru, f, super::batch::FieldOp::RangeReader { lo, hi, id, on });
                } else if on {
                    self.w.add_range_reader(ru, f, lo, hi, id);
                } else {
                    self.w.drop_range_reader(ru, f, hi, id);
                }
            }
        }
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

    /// The position a use read is filed at: its node's (a read after a
    /// subtree at the node, conservatively: any change after it re-asks).
    fn read_pos(&self, at: Id, _after: bool, n: &Node) -> Pos {
        self.w.node(at).map_or(n.pos, |x| x.pos)
    }

    // --------------------------------------------------------------- commit

    /// Commits node `id`'s run: placements, definitions, output; the
    /// readers whose answers changed are re-asked. Returns whether its
    /// output changed.
    pub(crate) fn commit(&mut self, id: Id, mut res: Res, kids: &[Spec], conts: &[Spec]) -> bool {
        let Some(n) = self.w.node(id).cloned() else { return false };
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
        let filed = super::core::prof::time(11, || match &old {
            Some(o) => self.reindex_reads(id, &n, o, &res.reads),
            None => self.add_reads(id, &n, &res.reads),
        });
        res.filed = Some((self.w.relabels, filed));
        self.counts.merge.reads += super::core::ns_since(t);
        let t_defs = super::core::prof::start();
        let t = super::core::now();
        // Definitions: removed, added, changed.
        let old_defs: Vec<Def> = old.as_ref().map_or_else(Vec::new, |o| o.defs.clone());
        let decl = n.text.stat.defs.clone();
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
        // Declared entries this run did not make go.
        if let Some(dl) = decl.as_ref() {
            for &(f, loc) in dl.iter() {
                let made = res.defs.iter().any(|e| e.f == f && e.loc.entry_key() == loc.entry_key());
                let had = old_defs.iter().any(|e| e.f == f && e.loc.entry_key() == loc.entry_key());
                if !made && !had {
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
        super::core::prof::time(13, || {
            let before: Vec<(Field, ValueId)> = contribs_of(&n);
            for &(f, _) in &before {
                if !res.contribs.iter().any(|c| c.0 == f) {
                    self.contrib(n.unit, f, n.pos, None);
                }
            }
            for &(f, x) in &res.contribs {
                if !before.contains(&(f, x)) {
                    self.contrib(n.unit, f, n.pos, Some(x));
                }
            }
        });
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
        let out_changed = old.as_ref().is_none_or(|o| o.out != res.out);
        let mut n2 = n.clone();
        n2.wide |= widen;
        n2.narrowed = None;
        let new_defs = res.defs.clone();
        let claim = res.claim;
        n2.res = Some(Arc::new(res));
        if n2.st == St::Dirty {
            n2.st = St::Done;
        }
        self.w.put(id, n2.clone());
        // Scope entries filed where the old effect put them move with it.
        if effect_changed {
            let mut keys: Vec<(Field, Locality)> = old_defs.iter().map(|d| (d.f, d.loc)).collect();
            keys.extend(decl.iter().flat_map(|d| d.iter().copied()));
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
                if let Some(m) = self.w.out_readers.get(&id).cloned() {
                    for (&r, ()) in m.iter() {
                        self.recheck(r, 18);
                    }
                }
            }
        });
        super::core::prof::time(14, || self.run_claimed(id, claim));
        self.counts.merge.outs += super::core::ns_since(t);
        self.w.stale.remove(&id, &NoMeasure);
        self.woken.push(id);
        out_changed
    }

    /// Commits a stopped run's unfolding only (its children made, so the
    /// run can wait for them).
    pub(crate) fn commit_unfolding(&mut self, id: Id, kids: &[Spec], conts: &[Spec]) {
        let Some(n) = self.w.node(id).cloned() else { return };
        let (k, c) = self.place(id, kids, conts);
        let mut n2 = self.w.node(id).cloned().unwrap_or(n);
        let mut res = n2.res.as_deref().cloned().unwrap_or_default();
        res.kids = k;
        res.conts = c;
        n2.res = Some(Arc::new(res));
        self.w.put(id, n2);
        self.w.stale.remove(&id, &NoMeasure);
    }
}

/// A node's contributions as filed: its run's, else its text's declared.
fn contribs_of(n: &Node) -> Vec<(Field, ValueId)> {
    match (&n.res, &n.text.stat.contribs) {
        (Some(r), _) if !matches!(n.st, St::Inert(_)) => r.contribs.clone(),
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

/// Whether read `r` of node `me` still holds; `strict`: a definition still
/// to be made between its site and the reader breaks it (else the read
/// stands as a guess until that definer commits).
pub(crate) fn holds_with(core: &Core, w: &World, me: Id, r: &Read, strict: bool) -> bool {
    match r {
        Read::Use {
            f,
            at,
            after,
            site,
            val,
            proj,
        } => {
            if let Some(&lv) = w.loops.get(f) {
                let _ = lv;
                return false;
            }
            let Some((u, pos, fr)) = w.frame_at(*at, *after) else {
                return false;
            };
            let reach = w.reaching(u, *f, pos, fr, if *after { NONE } else { *at });
            if strict && w.pending_set_between(core, u, *f, reach.map_or(0, |r| r.0), pos, me).is_some() {
                return false;
            }
            let (s2, v2) = match reach {
                Some((_, e)) => match e.val {
                    Some(v) => (e.node, Some(v)),
                    // A definer in work: the answer stands as a guess until
                    // it commits (its commit re-asks the readers).
                    None => return !strict,
                },
                None => (NONE, w.env_of(core, u, *f)),
            };
            let _ = me;
            if *proj != 0 {
                return v2.map(|x| core.project(*proj, x)) == val.map(|x| core.project(*proj, x));
            }
            let _ = (s2, site);
            v2 == *val
        }
        Read::Loop { f, val } => w.loops.get(f) == Some(val),
        Read::Env { f, val } => core.env_get(w, *f).flatten() == *val,
        Read::Out { node, val } => w.node(*node).and_then(Node::out) == *val,
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
        Read::Range { f, from, to, val, past } => {
            let Some(mo) = core.acc(*f) else {
                return *val == ValueId::UNIT;
            };
            let m = AccMeasure {
                m: &**mo,
                v: &core.values,
            };
            let Some((u, lo, hi)) = w.window(me, *from, *to, *past) else {
                return false;
            };
            w.fold(&m, u, *f, lo, hi) == *val
        }
    }
}

/// Whether use read `r` now reaches another site.
pub(crate) fn rebound(core: &Core, w: &World, r: &Read) -> bool {
    let _ = core;
    let Read::Use { f, at, after, site, .. } = r else {
        return false;
    };
    if w.loops.get(f).is_some() {
        return false;
    }
    let Some((u, pos, fr)) = w.frame_at(*at, *after) else {
        return true;
    };
    let now = w.reaching(u, *f, pos, fr, if *after { NONE } else { *at }).map_or(NONE, |(_, e)| e.node);
    now != *site
}

/// Why read `r` of node `me` no longer holds.
pub(crate) fn why_broken(core: &Core, w: &World, me: Id, r: &Read) -> crate::api::Rerun {
    use crate::api::Rerun;
    match r {
        Read::Use { f, at, after, val, .. } => {
            let new = w
                .frame_at(*at, *after)
                .and_then(|(u, pos, fr)| match w.reaching(u, *f, pos, fr, if *after { NONE } else { *at }) {
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
        Read::Out { node, .. } => Rerun::Output {
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
