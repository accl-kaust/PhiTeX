//! Compaction (DESIGN 7.11): after sealing, the dead nodes' slots and the
//! arenas' stale entries are memory still held. Live nodes are moved into
//! the holes from the top (a root-level node never moves: the client
//! holds its id), the tables are truncated, and the operand, reverse-edge
//! and definition arenas are rebuilt from the live nodes alone.

use super::{DEAD, DefRec, Graph, Kind, NOGROUP, NONE, Opd, Pos, Rev, Set, StepInfo};
use crate::lang::Lang;

impl<L: Lang> Graph<L> {
    /// Compact if at least half the node table is dead.
    pub(super) fn compact_if_sparse(&mut self) {
        let dead = self.free.len();
        if dead * 2 >= self.n.h.len() && dead > 1024 {
            self.compact();
        }
    }

    /// Move live nodes into dead slots, truncate, rebuild the arenas.
    #[allow(clippy::too_many_lines, reason = "one pass per table")]
    #[allow(clippy::cast_possible_truncation, reason = "ids fit u32")]
    pub fn compact(&mut self) {
        debug_assert!(self.heap.is_empty(), "compaction between runs");
        let len = self.n.h.len();
        if cfg!(debug_assertions) {
            let dead =
                |x: u32| x != NONE && (x as usize >= len || self.n.h[x as usize].flags & DEAD != 0);
            for (m, list) in self.names.defs.iter().enumerate() {
                for &i in list {
                    let d = &self.names.recs[i as usize];
                    assert!(!dead(d.step), "def of {m} at dead %{}", d.step);
                    assert!(
                        !dead(d.src),
                        "def of {m} from dead %{} at %{}",
                        d.src,
                        d.step
                    );
                }
            }
            for i in 1..len {
                let h = &self.n.h[i];
                if h.flags & DEAD != 0 {
                    continue;
                }
                assert!(!dead(h.parent), "%{i} under dead %{}", h.parent);
                assert!(
                    !dead(h.next),
                    "%{i} ({:?}) then dead %{}, parent %{} {:?} flags {} ord {}, epoch {}",
                    h.kind,
                    h.next,
                    h.parent,
                    self.n.h[h.parent as usize].kind,
                    self.n.h[h.parent as usize].flags,
                    h.ord,
                    self.epoch
                );
                for o in self.n.opds_of(i as u32) {
                    assert!(!dead(o.src), "%{i} ({:?}) reads dead %{}", h.kind, o.src);
                }
                if h.kind == Kind::Step {
                    let si = &self.steps[h.aux as usize];
                    assert!(!dead(si.first), "step %{i} first dead %{}", si.first);
                    assert!(!dead(si.unfold), "step %{i} unfold dead");
                }
            }
        }
        let live = |g: &Self, i: usize| g.n.h[i].flags & DEAD == 0;
        // groups still referenced keep their ids: a slot whose id opened
        // one stays reserved
        let mut used: Set<u64> = Set::default();
        let mut mark = |g: u64| {
            if g != NOGROUP {
                used.insert(g);
            }
        };
        for list in &self.names.defs {
            for &i in list {
                mark(self.names.recs[i as usize].group);
            }
        }
        for i in 1..len {
            if !live(self, i) {
                continue;
            }
            match self.n.h[i].kind {
                Kind::Step => mark(self.steps[self.n.h[i].aux as usize].grp_in),
                Kind::Unfold => {
                    let ui = &self.unfolds[self.n.h[i].aux as usize];
                    mark(ui.grp0);
                    if let Some(p) = ui.parked {
                        mark(p.grp);
                    }
                }
                _ => {}
            }
        }
        for v in self.closes.values() {
            for &g in v {
                mark(g);
            }
        }
        // (and their ancestors)
        let mut stack: Vec<u64> = used.iter().copied().collect();
        while let Some(g) = stack.pop() {
            if let Some(gr) = self.groups.get(&g)
                && gr.parent != NOGROUP
                && used.insert(gr.parent)
            {
                stack.push(gr.parent);
            }
        }
        let mut reserved = vec![false; len];
        for &g in &used {
            let st = (g >> 32) as usize;
            if st < len && !live(self, st) {
                reserved[st] = true;
            }
        }
        // two fingers: the highest movable live node into the lowest hole
        let mut map: Vec<u32> = (0..len as u32).collect();
        let movable = |g: &Self, i: usize| live(g, i) && g.n.h[i].depth > 1;
        let (mut lo, mut hi) = (1usize, len - 1);
        loop {
            while lo < hi && (live(self, lo) || reserved[lo]) {
                lo += 1;
            }
            while hi > lo && !movable(self, hi) {
                hi -= 1;
            }
            if lo >= hi {
                break;
            }
            map[hi] = lo as u32;
            self.n.h[lo] = self.n.h[hi];
            let v = std::mem::take(&mut self.n.val[hi]);
            self.n.val[lo] = v;
            self.n.h[hi].flags |= DEAD;
            lo += 1;
            hi -= 1;
        }
        let new_len = (1..len)
            .rev()
            .find(|&i| live(self, i) || reserved[i])
            .map_or(1, |i| i + 1);
        let mp = |x: u32| if x == NONE { NONE } else { map[x as usize] };
        let mg = |g: u64| -> u64 {
            if g == NOGROUP {
                return g;
            }
            let st = (g >> 32) as usize;
            if st < len {
                (u64::from(map[st]) << 32) | (g & 0xffff_ffff)
            } else {
                g
            }
        };
        self.n.h.truncate(new_len);
        self.n.val.truncate(new_len);
        shrink_roomy(&mut self.n.h);
        shrink_roomy(&mut self.n.val);
        // headers, operands and side tables, node by node
        // (which slots hold live nodes now)
        let hs: Vec<u8> = self.n.h.iter().map(|h| h.flags & DEAD).collect();
        let old_opds = std::mem::take(&mut self.n.opds);
        let old_steps = std::mem::take(&mut self.steps);
        let old_defs = std::mem::take(&mut self.names.recs);
        // (each record's new index: steps' ranges in node order, then the
        // imports, which no step holds)
        let mut rmap = vec![NONE; old_defs.len()];
        let old_unfolds = std::mem::take(&mut self.unfolds);
        let mut old_unfolds: Vec<Option<_>> = old_unfolds.into_iter().map(Some).collect();
        let old_scans = std::mem::take(&mut self.scans);
        let mut old_scans: Vec<Option<_>> = old_scans.into_iter().map(Some).collect();
        let mut opds: Vec<Opd> = Vec::new();
        for i in 0..new_len {
            let h = &mut self.n.h[i];
            if h.flags & DEAD != 0 {
                h.a0 = 0;
                h.an = 0;
                h.rd = NONE;
                continue;
            }
            h.parent = mp(h.parent);
            h.next = mp(h.next);
            h.rd = NONE;
            let (a0, an) = (h.a0 as usize, h.an as usize);
            h.a0 = opds.len() as u32;
            opds.extend(old_opds[a0..a0 + an].iter().map(|o| Opd {
                src: mp(o.src),
                ..*o
            }));
            match h.kind {
                Kind::Step => {
                    let si = old_steps[h.aux as usize].clone();
                    let d0 = self.names.recs.len() as u32;
                    for k in si.d0..si.d0 + si.dn {
                        rmap[k as usize] = self.names.recs.len() as u32;
                        let d = &old_defs[k as usize];
                        self.names.recs.push(DefRec {
                            step: i as u32,
                            src: mp(d.src),
                            group: mg(d.group),
                            ..*d
                        });
                    }
                    h.aux = self.steps.len() as u64;
                    self.steps.push(StepInfo {
                        unfold: mp(si.unfold),
                        first: mp(si.first),
                        grp_in: mg(si.grp_in),
                        d0,
                        ..si
                    });
                }
                Kind::Unfold => {
                    let mut ui = old_unfolds[h.aux as usize]
                        .take()
                        .expect("one unfold a slot");
                    ui.first = mp(ui.first);
                    ui.grp0 = mg(ui.grp0);
                    // (the maps kept, their steps renumbered: dead ones out)
                    let live = |x: u32| {
                        let m = map[x as usize] as usize;
                        (m < new_len && hs[m] == 0).then_some(m as u32)
                    };
                    ui.keys = ui
                        .keys
                        .iter()
                        .filter_map(|(&k, &x)| Some((k, live(x)?)))
                        .collect();
                    ui.owners = ui
                        .owners
                        .iter()
                        .filter_map(|(&k, &x)| Some((k, live(x)?)))
                        .collect();
                    // (room, as for the tables: the first edit after does not
                    // rehash a map of every step)
                    ui.keys.reserve(ui.keys.len() / 8 + 16);
                    ui.owners.reserve(ui.owners.len() / 8 + 16);
                    if let Some(p) = ui.parked.as_mut() {
                        p.s = mp(p.s);
                        p.grp = mg(p.grp);
                    }
                    h.aux = self.unfolds.len() as u64;
                    self.unfolds.push(ui);
                }
                Kind::Scan => {
                    let sc = old_scans[h.aux as usize].take().expect("one scan a slot");
                    h.aux = self.scans.len() as u64;
                    self.scans.push(sc);
                }
                _ => {}
            }
        }
        drop(old_opds);
        // reverse edges, from the operands
        let mut revs: Vec<Rev> = Vec::with_capacity(opds.len());
        for i in 0..new_len {
            let h = self.n.h[i];
            if h.flags & DEAD != 0 {
                continue;
            }
            for o in &opds[h.a0 as usize..h.a0 as usize + h.an as usize] {
                if o.src != NONE {
                    let r = revs.len() as u32;
                    revs.push(Rev {
                        node: i as u32,
                        era: h.era,
                        next: self.n.h[o.src as usize].rd,
                    });
                    self.n.h[o.src as usize].rd = r;
                }
            }
        }
        shrink_roomy(&mut opds);
        shrink_roomy(&mut revs);
        self.n.opds = opds;
        self.n.revs = revs;
        // (imports: defined at the root, held by no step)
        for list in &self.names.defs {
            for &k in list {
                if rmap[k as usize] == NONE {
                    rmap[k as usize] = self.names.recs.len() as u32;
                    let d = &old_defs[k as usize];
                    self.names.recs.push(DefRec {
                        step: mp(d.step),
                        src: mp(d.src),
                        group: mg(d.group),
                        ..*d
                    });
                }
            }
        }
        shrink_roomy(&mut self.names.recs);
        shrink_roomy(&mut self.steps);
        self.root_first = mp(self.root_first);
        // the name index
        for list in &mut self.names.defs {
            for d in list.iter_mut() {
                *d = rmap[*d as usize];
            }
        }
        let g = &self.n;
        for list in &mut self.names.readers {
            list.retain(|&(a, r, e)| {
                let r = map.get(r as usize).copied().unwrap_or(NONE);
                let a = map.get(a as usize).copied().unwrap_or(NONE);
                r != NONE
                    && (r as usize) < new_len
                    && (a as usize) < new_len
                    && g.h[r as usize].flags & DEAD == 0
                    && g.h[r as usize].era == e
            });
            for e in list.iter_mut() {
                *e = (map[e.0 as usize], map[e.1 as usize], e.2);
            }
            list.shrink_to_fit();
        }
        self.names.pruned.fill(false);
        self.names.prune.clear();
        // groups, closes, registries
        let groups = std::mem::take(&mut self.groups);
        for (k, mut gr) in groups {
            if !used.contains(&k) {
                continue;
            }
            gr.parent = mg(gr.parent);
            gr.close = gr.close.map(|c| Pos {
                parent: mp(c.parent),
                ord: c.ord,
            });
            self.groups.insert(mg(k), gr);
        }
        let closes = std::mem::take(&mut self.closes);
        for (s, v) in closes {
            let m = map[s as usize] as usize;
            if m < new_len && self.n.h[m].flags & DEAD == 0 {
                self.closes
                    .insert(m as u32, v.into_iter().map(mg).collect());
            }
        }
        let fix = |v: &mut Vec<u32>, g: &super::Nodes<L>| {
            v.retain(|&x| {
                let m = map[x as usize] as usize;
                m < new_len && g.h[m].flags & DEAD == 0
            });
            for x in v.iter_mut() {
                *x = map[*x as usize];
            }
        };
        for v in self.chains.values_mut() {
            v.retain(|&x| {
                let m = map[x as usize] as usize;
                m < new_len && self.n.h[m].flags & DEAD == 0
            });
            for x in v.iter_mut() {
                *x = map[*x as usize];
            }
        }
        for v in self.chain_readers.values_mut() {
            fix(v, &self.n);
        }
        for v in self.pubs.values_mut() {
            fix(v, &self.n);
        }
        for v in self.crosses.values_mut() {
            fix(v, &self.n);
        }
        for v in self.fams.values_mut() {
            fix(v, &self.n);
        }
        for v in self.fam_readers.values_mut() {
            fix(v, &self.n);
        }
        let nf = std::mem::take(&mut self.n.fams);
        self.n.fams = nf
            .into_iter()
            .filter(|&(k, _)| (k as usize) < len)
            .map(|(k, f)| (map[k as usize], f))
            .collect();
        for v in self.imports.values_mut() {
            *v = map[*v as usize];
        }
        let ents = std::mem::take(&mut self.entries);
        self.entries = ents
            .into_iter()
            .map(|(k, v)| (map[k as usize], v))
            .collect();
        // the free list: the dead slots left
        self.free.clear();
        self.to_free.clear();
        for i in (1..new_len).rev() {
            if self.n.h[i].flags & DEAD != 0 && !reserved[i] {
                self.free.push(i as u32);
            }
        }
        let hot = std::mem::take(&mut self.hot);
        self.hot = hot
            .into_iter()
            .filter_map(|(due, x)| {
                let m = map[x as usize] as usize;
                (m < new_len && self.n.h[m].flags & DEAD == 0).then_some((due, m as u32))
            })
            .collect();
        self.hint = (NONE, super::END, 0);
    }
}

/// Capacity cut to the length plus an eighth: the first nodes made after
/// a compaction do not move the whole table (at 10^8 nodes, a 12 ms
/// first edit). The room is address space, not memory, until it is used.
///
/// The first 2 MB of the room are written once here, so the page faults
/// (a huge page cleared, about 150 us a table) are the seal's, not the
/// first edit's.
fn shrink_roomy<T>(v: &mut Vec<T>) {
    let len = v.len();
    let want = len + len / 8 + 64;
    if v.capacity() > want {
        v.shrink_to(want);
    } else {
        v.reserve_exact(want - len);
    }
    prefault(v);
}

/// The first 2 MB of `v`'s spare capacity written once, so the page
/// faults there (a huge page found and cleared: up to 0.6 ms) are paid
/// now, not by the push of the next edit.
pub(super) fn prefault<T>(v: &mut Vec<T>) {
    let spare = v.spare_capacity_mut();
    let k = spare.len().min((2 << 20) / size_of::<T>().max(1));
    for x in &mut spare[..k] {
        *x = std::mem::MaybeUninit::zeroed();
    }
}
