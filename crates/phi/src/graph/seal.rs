//! Sealed regions (DESIGN 7.11, the retained tier): a run of a root
//! unfold's settled steps folded into one step. The fold keeps what the
//! run's outside sees: its entry (the first step's key, cursor, group and
//! the state it ran from), its output state, its net definitions with
//! their sources, and the reads it made from outside. Its interior is
//! dropped. Woken, it runs again from its entry state as its first step
//! did, and the steps after it are made again until one meets the step
//! that followed the fold: the result is the run's, exactly.
//!
//! Which runs: steps whose members are pure leaves and constants only (an
//! effect, a publisher, a creator or a cross read ends a run), cut where
//! the group in force is the one the run started in (no group opened in
//! the run is open after it, none open before it is closed in it), and
//! where the step's identity hashes to a cut (content-defined, so a fresh
//! build and an edited one cut alike), or after `4 F` steps.

use super::{
    DEAD, DIRTY, DefEntry, DefRec, Graph, Kind, Map, NONE, Opd, Ordering, Pos, QUEUED, Rev, SEALED,
    Set,
};
use crate::lang::Lang;

/// Net definitions' and kept members' labels in a fold (above any
/// emission's ordinal).
pub(super) const NETBASE: u64 = 1 << 62;
/// A step of the run being folded.
const MARK: u8 = 16;

impl<L: Lang> Graph<L> {
    /// Seal what is due at the end of a run: everything after the first
    /// build; later, the stretch of live steps around each step that has
    /// been quiet long enough since it ran.
    pub(super) fn seal_due_now(&mut self) {
        if self.cfg.seal == 0 {
            return;
        }
        if self.epoch == 1 {
            self.seal_all();
            return;
        }
        let f = u64::from(self.cfg.seal.max(1));
        // (a few runs: each one's lists spliced in place)
        let mut pend = Pending {
            local: true,
            ..Pending::default()
        };
        let mut any = false;
        while let Some(&(due, d)) = self.hot.front() {
            if due > self.epoch {
                break;
            }
            self.hot.pop_front();
            let h = &self.n.h[d as usize];
            if h.kind != Kind::Step
                || h.flags & (DEAD | SEALED) != 0
                || self.epoch - self.steps[h.aux as usize].ran < self.cfg.seal_quiet
            {
                continue;
            }
            let u = h.parent;
            if self.n.h[u as usize].parent != super::ROOT {
                continue;
            }
            // (back to the stretch's start: the step after a sealed one,
            // or the unfold's first)
            let mut start = d;
            loop {
                let p = self.n.opds_of(start)[0].src;
                if p == NONE {
                    break;
                }
                let hp = &self.n.h[p as usize];
                if hp.kind != Kind::Step || hp.parent != u || hp.flags & SEALED != 0 {
                    break;
                }
                start = p;
            }
            self.seal_span(u, start, f, &mut pend);
            any = true;
        }
        let _ = any;
    }

    /// Seal the settled runs of every root unfold now (`Config::seal`
    /// steps a run on average), and compact: what that seals and frees.
    pub fn seal(&mut self) -> u64 {
        let before = self.rep.sealed;
        self.seal_all();
        let tf = std::mem::take(&mut self.to_free);
        for n in tf {
            self.set_first(n, NONE);
            self.n.val[n as usize] = L::Val::default();
            self.free.push(n);
        }
        self.compact_if_sparse();
        self.rep.sealed - before
    }

    /// Seal the settled runs of every root unfold.
    fn seal_all(&mut self) {
        let f = u64::from(self.cfg.seal.max(1));
        let mut us = Vec::new();
        let mut c = self.root_first;
        while c != NONE {
            if self.n.h[c as usize].kind == Kind::Unfold && !self.n.is_dead(c) {
                us.push(c);
            }
            c = self.n.h[c as usize].next;
        }
        let mut pend = Pending::default();
        for &u in &us {
            self.seal_unfold(u, f, &mut pend);
        }
        self.seal_lists(pend);
        // (the key and owner maps made here, with the pass, not by the
        // first edit after it; compaction keeps them)
        for u in us {
            self.index_steps(u);
        }
    }

    /// Whether step `s` may be in a sealed run.
    fn sealable(&self, s: u32) -> bool {
        let h = &self.n.h[s as usize];
        if h.flags & (DIRTY | QUEUED | SEALED | DEAD) != 0 || h.next == NONE {
            return false;
        }
        // (cold: run only in the first build, or quiet long enough since)
        let ran = self.steps[h.aux as usize].ran;
        if ran > 1 && self.epoch - ran < self.cfg.seal_quiet {
            return false;
        }
        let mut c = self.first(s);
        while c != NONE {
            let hc = &self.n.h[c as usize];
            if !matches!(hc.kind, Kind::Leaf | Kind::Const) || hc.class != 0 {
                return false;
            }
            c = hc.next;
        }
        true
    }

    fn cut_here(&self, s: u32, f: u64) -> bool {
        let si = &self.steps[self.n.h[s as usize].aux as usize];
        let mut x = si.key ^ si.at.0.rotate_left(29) ^ 0x9e37_79b9_7f4a_7c15;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x.is_multiple_of(f)
    }

    fn seal_unfold(&mut self, u: u32, f: u64, pend: &mut Pending) {
        let mut s = self.first(u);
        while s != NONE {
            if self.n.h[s as usize].flags & SEALED != 0 {
                s = self.n.h[s as usize].next;
            } else {
                s = self.seal_span(u, s, f, pend);
            }
        }
    }

    /// Seal the runs of the stretch of unsealed steps from `s0` (to the
    /// next sealed step, or the end): the node it stopped at.
    fn seal_span(&mut self, u: u32, s0: u32, f: u64, pend: &mut Pending) -> u32 {
        let mut run: Vec<u32> = Vec::new();
        // (the last balanced cut in `run`: its length there)
        let mut cut = 0usize;
        let mut g0 = 0u64;
        let max = u64::from(self.cfg.seal_max);
        let mut work = 0u64;
        let mut s = s0;
        while s != NONE && self.n.h[s as usize].flags & SEALED == 0 {
            let nx = self.n.h[s as usize].next;
            if !self.sealable(s) {
                self.seal_run(u, &run[..cut], pend);
                run.clear();
                cut = 0;
                s = nx;
                continue;
            }
            let w = u64::from(self.steps[self.n.h[s as usize].aux as usize].work).max(1);
            if !run.is_empty() && work + w > max {
                // (the bound: what the first edit in it re-runs)
                self.seal_run(u, &run[..cut], pend);
                // (the steps after the last cut start the next run: they
                // begin in the same group; with no cut at all, the run
                // stays live)
                let rest: Vec<u32> = if cut == 0 {
                    Vec::new()
                } else {
                    run[cut..].to_vec()
                };
                run = rest;
                cut = 0;
                work = 0;
                for (i, &x) in run.iter().enumerate() {
                    let xi = &self.steps[self.n.h[x as usize].aux as usize];
                    work += u64::from(xi.work).max(1);
                    let nx = self.n.h[x as usize].next;
                    if self.steps[self.n.h[nx as usize].aux as usize].grp_in == g0 {
                        cut = i + 1;
                    }
                }
                if work + w > max {
                    run.clear();
                    cut = 0;
                    work = 0;
                }
            }
            if run.is_empty() {
                g0 = self.steps[self.n.h[s as usize].aux as usize].grp_in;
            }
            run.push(s);
            work += w;
            let out = self.steps[self.n.h[nx as usize].aux as usize].grp_in;
            if out == g0 {
                cut = run.len();
                if run.len() >= 2 && self.cut_here(s, f) {
                    self.seal_run(u, &run, pend);
                    run.clear();
                    cut = 0;
                    work = 0;
                }
            }
            s = nx;
        }
        self.seal_run(u, &run[..cut], pend);
        s
    }

    /// Sealed regions now: (steps, emissions) of each, in order.
    #[must_use]
    pub fn sealed_runs(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for (i, h) in self.n.h.iter().enumerate() {
            if h.kind == Kind::Step && h.flags & (SEALED | DEAD) == SEALED {
                let _ = i;
                let si = &self.steps[h.aux as usize];
                out.push((si.folded, si.work));
            }
        }
        out
    }

    /// Fold `run` (consecutive, balanced) into its first step.
    #[allow(clippy::too_many_lines, reason = "one fold, table by table")]
    fn seal_run(&mut self, u: u32, run: &[u32], pend: &mut Pending) {
        if run.len() < 2 {
            return;
        }
        let s1 = run[0];
        let sk = run[run.len() - 1];
        let q = self.n.h[sk as usize].next;
        for &s in run {
            self.n.h[s as usize].flags |= MARK;
        }
        let inside = |g: &Self, x: u32| {
            x != NONE
                && (g.n.h[x as usize].flags & MARK != 0
                    || g.n.h[g.n.h[x as usize].parent as usize].flags & MARK != 0)
        };
        let p1 = self.n.pos(s1);
        let pq = self.n.pos(q);
        if pend.local {
            for &s in run {
                for o in self.n.opds_of(s) {
                    if o.name != NONE {
                        pend.read.push(o.name);
                    }
                }
            }
        }
        // the reads from outside, in order, once each
        let mut ext: Vec<Opd> = vec![self.n.opds_of(s1)[0]];
        let mut seen: Set<(u32, u32, u32)> = Set::default();
        for &s in run {
            for o in &self.n.opds_of(s)[1..] {
                if inside(self, o.src) {
                    continue;
                }
                if seen.insert((o.src, o.sel.0, o.name)) {
                    ext.push(*o);
                }
            }
        }
        // net definitions: per name the last one alive after the run (one
        // in a group the run opened is not: the run closed it)
        let opened_here = |g: &Self, grp: u64| {
            grp != super::NOGROUP && {
                let st = (grp >> 32) as u32;
                (st as usize) < g.n.h.len() && g.n.h[st as usize].flags & MARK != 0
            }
        };
        let mut net: Vec<DefRec> = Vec::new();
        let mut at_name: Map<u32, usize> = Map::default();
        let mut took = 0u32;
        let mut work = 0u32;
        for &s in run {
            let si = &self.steps[self.n.h[s as usize].aux as usize];
            took += si.took;
            work += si.work;
            for d in &self.step_defs[si.d0 as usize..(si.d0 + si.dn) as usize] {
                pend.touched.push(d.name);
                if !d.global && opened_here(self, d.group) {
                    continue;
                }
                if let Some(&k) = at_name.get(&d.name) {
                    net[k] = *d;
                } else {
                    at_name.insert(d.name, net.len());
                    net.push(*d);
                }
            }
        }
        // members kept: the net definitions' sources inside the run
        let mut keep: Vec<u32> = Vec::new();
        for d in &net {
            if inside(self, d.src)
                && self.n.h[d.src as usize].flags & MARK == 0
                && !keep.contains(&d.src)
            {
                keep.push(d.src);
            }
        }
        for &k in &keep {
            self.n.h[k as usize].flags |= MARK;
        }
        // the interior dropped: every member not kept, every step but the
        // first
        for &s in run {
            let mut c = self.first(s);
            while c != NONE {
                let nx = self.n.h[c as usize].next;
                if self.n.h[c as usize].flags & MARK == 0 {
                    self.drop_quiet(c, !pend.local);
                }
                c = nx;
            }
            // (groups the run opened and closed are gone with it)
            for g in self.closes.remove(&s).unwrap_or_default() {
                self.groups.remove(&g);
            }
        }
        for &s in &run[1..] {
            self.set_first(s, NONE);
            self.drop_quiet(s, !pend.local);
        }
        // the fold: the first step, its members the kept ones
        let mut prev = NONE;
        for (i, &k) in keep.iter().enumerate() {
            let h = &mut self.n.h[k as usize];
            h.flags &= !MARK;
            h.parent = s1;
            h.ord = NETBASE + i as u64;
            // (keys of their own: members of different steps may share
            // an ordinal, and matching needs each one once)
            h.key = 0x4000_0000 | u32::try_from(i).expect("members fit u32");
            h.next = NONE;
            if prev == NONE {
                self.set_first(s1, k);
            } else {
                self.n.h[prev as usize].next = k;
            }
            prev = k;
        }
        if keep.is_empty() {
            self.set_first(s1, NONE);
        }
        let si1 = self.n.h[s1 as usize].aux as usize;
        let d0 = u32::try_from(self.step_defs.len()).expect("definitions fit u32");
        for (i, d) in net.iter().enumerate() {
            let rec = DefRec {
                sub: NETBASE + i as u64,
                ..*d
            };
            pend.defs.push((
                rec.name,
                DefEntry {
                    pos: Pos {
                        parent: s1,
                        ord: rec.sub,
                    },
                    src: rec.src,
                    sel: rec.sel,
                    group: rec.group,
                    global: rec.global,
                },
            ));
            self.step_defs.push(rec);
        }
        self.steps[si1].d0 = d0;
        self.steps[si1].dn = u32::try_from(net.len()).expect("definitions fit u32");
        self.steps[si1].took = took;
        self.steps[si1].work = work;
        self.steps[si1].folded = u32::try_from(run.len()).expect("steps fit u32");
        let v = std::mem::take(&mut self.n.val[sk as usize]);
        self.n.val[s1 as usize] = v;
        for &s in run {
            self.n.h[s as usize].flags &= !MARK;
        }
        self.n.h[s1 as usize].flags |= SEALED;
        self.n.h[s1 as usize].next = q;
        // its operands (readers' lists merged in bulk after)
        self.seal_opds(s1, &ext, pend);
        // the step after reads the fold
        let qa0 = self.n.h[q as usize].a0 as usize;
        self.n.opds[qa0].src = s1;
        let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
        self.n.revs.push(Rev {
            node: q,
            era: self.n.h[q as usize].era,
            next: self.n.h[s1 as usize].rd,
        });
        self.n.h[s1 as usize].rd = r;
        let ui = self.n.h[u as usize].aux as usize;
        if self.unfolds[ui].indexed {
            for &s in &run[1..] {
                let si = &self.steps[self.n.h[s as usize].aux as usize];
                let (key, at) = (si.key, si.at);
                let info = &mut self.unfolds[ui];
                if info.keys.get(&key) == Some(&s) {
                    info.keys.remove(&key);
                }
                if info.owners.get(&at) == Some(&s) {
                    info.owners.remove(&at);
                }
            }
            let at1 = self.steps[si1].at;
            if took > 0 {
                self.unfolds[ui].owners.insert(at1, s1);
            }
        }
        self.rep.sealed += run.len() as u64;
        if pend.local {
            self.splice_lists(p1, pq, pend);
        }
    }

    /// One run's lists spliced in place: its steps' definitions and
    /// readers (between its first step and the node after it) replaced by
    /// the fold's.
    fn splice_lists(&mut self, p1: Pos, pq: Pos, pend: &mut Pending) {
        let g = &self.n;
        pend.touched.sort_unstable();
        pend.touched.dedup();
        for &m in &pend.touched {
            let list = &self.names.defs[m as usize];
            let lo = list.partition_point(|d| g.cmp_pos(d.pos, p1) != Ordering::Greater);
            let hi = list.partition_point(|d| g.cmp_pos(d.pos, pq) == Ordering::Less);
            let add: Vec<DefEntry> = pend.defs.iter().filter(|x| x.0 == m).map(|x| x.1).collect();
            self.names.defs[m as usize].splice(lo, hi, add);
        }
        pend.read.extend(pend.readers.iter().map(|x| x.0));
        pend.read.sort_unstable();
        pend.read.dedup();
        for &m in &pend.read {
            let g = &self.n;
            let list = &self.names.readers[m as usize];
            let lo = list.partition_point(|e| g.cmp_pos(g.pos(e.0), p1) == Ordering::Less);
            let hi = list.partition_point(|e| g.cmp_pos(g.pos(e.0), pq) == Ordering::Less);
            let add: Vec<(u32, u32, u32)> = pend
                .readers
                .iter()
                .filter(|x| x.0 == m)
                .map(|x| x.1)
                .collect();
            self.names.readers[m as usize].splice(lo, hi, add);
        }
        pend.touched.clear();
        pend.defs.clear();
        pend.read.clear();
        pend.readers.clear();
    }

    /// A node dropped by sealing: its definitions and readers are redone
    /// in bulk (`seal_lists`), nothing is woken.
    fn drop_quiet(&mut self, n: u32, prune: bool) {
        let u = n as usize;
        for k in 0..self.n.h[u].an as usize {
            let m = self.n.opds[self.n.h[u].a0 as usize + k].name;
            if m != NONE && prune {
                self.mark_prune(m);
            }
        }
        self.n.h[u].flags |= DEAD;
        self.n.h[u].era = self.n.h[u].era.wrapping_add(1);
        self.rep.removed += 1;
        self.to_free.push(n);
    }

    fn seal_opds(&mut self, s: u32, os: &[Opd], pend: &mut Pending) {
        let u = s as usize;
        for k in 0..self.n.h[u].an as usize {
            let m = self.n.opds[self.n.h[u].a0 as usize + k].name;
            if m != NONE && !pend.local {
                self.mark_prune(m);
            }
        }
        let era = self.n.h[u].era.wrapping_add(1);
        self.n.h[u].era = era;
        self.n.h[u].a0 = u32::try_from(self.n.opds.len()).expect("operands fit u32");
        self.n.h[u].an = u16::try_from(os.len()).expect("at most 65535 operands");
        for o in os {
            self.n.opds.push(*o);
            if o.src != NONE {
                let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
                self.n.revs.push(Rev {
                    node: s,
                    era,
                    next: self.n.h[o.src as usize].rd,
                });
                self.n.h[o.src as usize].rd = r;
            }
            if o.name != NONE {
                pend.readers.push((o.name, (s, s, era)));
            }
        }
    }

    /// The name index after sealing: dropped steps' definitions out, the
    /// folds' in; the folds' readers in. One pass per name touched.
    fn seal_lists(&mut self, mut pend: Pending) {
        let g = &self.n;
        let gone = |p: Pos| {
            let h = &g.h[p.parent as usize];
            h.flags & DEAD != 0 || (h.flags & SEALED != 0 && p.ord < NETBASE)
        };
        pend.defs
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| g.cmp_pos(a.1.pos, b.1.pos)));
        pend.touched.sort_unstable();
        pend.touched.dedup();
        let mut j = 0;
        for &m in &pend.touched {
            let old = std::mem::take(&mut self.names.defs[m as usize]);
            let mut new = Vec::with_capacity(old.len());
            let mut add = Vec::new();
            while j < pend.defs.len() && pend.defs[j].0 < m {
                j += 1;
            }
            while j < pend.defs.len() && pend.defs[j].0 == m {
                add.push(pend.defs[j].1);
                j += 1;
            }
            let mut a = add.into_iter().peekable();
            for &e in &old {
                if gone(e.pos) {
                    continue;
                }
                while let Some(x) = a.peek()
                    && g.cmp_pos(x.pos, e.pos) == Ordering::Less
                {
                    new.push(a.next().expect("peeked"));
                }
                new.push(e);
            }
            new.extend(a);
            self.names.defs[m as usize] = new.into_iter().collect();
        }
        pend.readers.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| g.cmp_pos(g.pos(a.1.0), g.pos(b.1.0)))
        });
        let mut i = 0;
        while i < pend.readers.len() {
            let m = pend.readers[i].0;
            let mut k = i;
            while k < pend.readers.len() && pend.readers[k].0 == m {
                k += 1;
            }
            let g = &self.n;
            let old = std::mem::take(&mut self.names.readers[m as usize]);
            let mut new = Vec::with_capacity(old.len() + k - i);
            let mut a = pend.readers[i..k].iter().map(|x| x.1).peekable();
            for &e in &old {
                while let Some(x) = a.peek()
                    && g.cmp_pos(g.pos(x.0), g.pos(e.0)) == Ordering::Less
                {
                    new.push(a.next().expect("peeked"));
                }
                new.push(e);
            }
            new.extend(a);
            self.names.readers[m as usize] = new.into_iter().collect();
            i = k;
        }
    }
}

/// What sealing leaves for the name index, done in bulk at the end.
#[derive(Default)]
struct Pending {
    /// Splice each run's lists as it is sealed (a few runs), instead of
    /// one merge per name at the end (a whole pass).
    local: bool,
    /// Names the run's steps read (local only).
    read: Vec<u32>,
    touched: Vec<u32>,
    defs: Vec<(u32, DefEntry)>,
    readers: Vec<(u32, (u32, u32, u32))>,
}
