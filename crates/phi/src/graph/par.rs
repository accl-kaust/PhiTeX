//! Parallel rebuild rounds (DESIGN 7.12). When many steps are dirty at
//! once (a preamble definition wakes every paragraph that reads it), the
//! earliest of them run their op on workers, dry, against the graph as it
//! is: each produces its emissions and records what it read from outside,
//! with versions. The graph then runs as in turn. When it reaches a step
//! whose dry outcome still holds (everything it read has the same version,
//! no definition of a name it read and no group changed since), the
//! outcome stands in for running the op; otherwise the op runs again. So
//! the result is the sequential one by construction; the workers only
//! take the ops' cost off the critical path.
//!
//! Cancellation: `Graph::cancel_token` is a flag another thread may set.
//! A run polls it between items (and a long step may, through
//! `StepCx::cancelled`); it then returns early with `Report::cancelled`,
//! the rest of the work still queued for the next run.

#![cfg_attr(
    target_arch = "wasm32",
    allow(
        dead_code,
        unused_imports,
        reason = "no threads on wasm: rounds never run"
    )
)]

use std::marker::PhantomData;
use std::sync::atomic::Ordering as AO;

use super::{Args, DEAD, DIRTY, END, Emit, Graph, Kind, NONE, Opd, SArg, StepCx};
use crate::lang::{Lang, Step};
use crate::memo::{Codec, Memo, MemoError};
use crate::seq::ElemId;
use crate::value::Value;
use crate::ver::Ver;

/// A step's op run dry, and what it read.
pub(super) struct Dry<L: Lang> {
    pub(super) em: Emit<L>,
    pub(super) res: Step<L::Val>,
    pub(super) start: usize,
    pub(super) idx: usize,
    pub(super) grp_out: u64,
    pub(super) end_cursor: ElemId,
    pub(super) in_ver: Ver,
    /// Every operand read from outside the step, with its version then.
    reads: Vec<(Opd, Ver)>,
    /// Every name read, with its definitions' generation then.
    names: Vec<(u32, u32)>,
    nnames: usize,
    groups_gen: u64,
    input_ver: Ver,
    at: ElemId,
}

impl<L: Lang> Graph<L> {
    /// Step `s`'s op run against the graph as it is, read only.
    pub(super) fn dry_step(&self, s: u32, mut em: Emit<L>) -> Dry<L> {
        em.clear();
        let si = self.n.h[s as usize].aux as usize;
        let u = self.n.h[s as usize].parent;
        let ui = self.n.h[u as usize].aux as usize;
        let input = self.unfolds[ui].input.as_ref();
        let at = self.steps[si].at;
        let start = match input {
            None => 0,
            Some(inp) if at == END => inp.len(),
            Some(inp) => inp.index_of(at).unwrap_or(inp.len()),
        };
        let prev_o = self.n.opds_of(s)[0];
        let uo = self.n.opds_of(u);
        let (res, idx, grp_out, end_cursor, in_ver) = {
            let mut cx = StepCx {
                g: &self.n,
                names: &self.names,
                groups: &self.groups,
                unfold_opds: uo,
                input,
                idx: start,
                start,
                leaf: &[] as &[(ElemId, L::Val)],
                lbase: 0,
                step: s,
                pos: self.n.pos(s),
                grp: self.steps[si].grp_in,
                opened: 0,
                em: &mut em,
                ext: self.ext.as_deref(),
                keep: self.cfg.keep_interior,
                cancel: &self.cancel,
                memo: self.memo_on.then_some(&*self.memo),
                _brand: PhantomData,
            };
            let args = Args::of(&self.n, &uo[2..]);
            let st = self.n.read(&prev_o);
            let in_ver = st.ver();
            let res = L::step(self.n.h[s as usize].op, &st, &args, &mut cx);
            (res, cx.idx, cx.grp, cx.cursor(), in_ver)
        };
        // what it read
        let mut reads: Vec<(Opd, Ver)> = vec![(prev_o, in_ver)];
        reads.extend(uo.iter().map(|o| (*o, self.n.read_ver(o))));
        let mut names: Vec<u32> = Vec::new();
        for &(m, o) in &em.reads {
            names.push(m);
            reads.push((o, self.n.read_ver(&o)));
        }
        for e in &em.args {
            match e.a {
                SArg::Name(m, _) => {
                    names.push(m);
                    reads.push((e.o, self.n.read_ver(&e.o)));
                }
                SArg::Node(..) => reads.push((e.o, self.n.read_ver(&e.o))),
                SArg::Local(..) => {}
            }
        }
        names.sort_unstable();
        names.dedup();
        let names = names
            .into_iter()
            .map(|m| (m, self.names.gens.get(m as usize).copied().unwrap_or(0)))
            .collect();
        Dry {
            em,
            res,
            start,
            idx,
            grp_out,
            end_cursor,
            in_ver,
            reads,
            names,
            nnames: self.names.spell.len(),
            groups_gen: self.groups_gen,
            input_ver: input.map_or(Ver::ABSENT, crate::seq::Seq::ver),
            at,
        }
    }

    /// Whether step `s`'s dry outcome still holds.
    fn holds(&self, s: u32, d: &Dry<L>) -> bool {
        let si = self.n.h[s as usize].aux as usize;
        let u = self.n.h[s as usize].parent;
        let ui = self.n.h[u as usize].aux as usize;
        self.names.spell.len() == d.nnames
            && self.groups_gen == d.groups_gen
            && self.steps[si].at == d.at
            && self.unfolds[ui]
                .input
                .as_ref()
                .map_or(Ver::ABSENT, crate::seq::Seq::ver)
                == d.input_ver
            && self.n.opds_of(s)[0] == d.reads[0].0
            && d.names
                .iter()
                .all(|&(m, g)| self.names.gens.get(m as usize).copied().unwrap_or(0) == g)
            && d.reads.iter().all(|(o, v)| self.n.read_ver(o) == *v)
    }

    /// Step `s`'s dry outcome, if one was made and still holds.
    pub(super) fn take_outcome(&mut self, s: u32) -> Option<Dry<L>> {
        let d = self.outcomes.remove(&s)?;
        if self.holds(s, &d) {
            self.rep.dry_used += 1;
            Some(d)
        } else {
            None
        }
    }

    /// A round: the earliest dirty steps (up to a budget) run dry on the
    /// workers. Their outcomes wait for the steps' turns.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn round(&mut self) {
        let w = self.cfg.workers;
        // (the round's size follows how much of the last one held:
        // doubled while nearly all did, halved when not; a round costs a
        // thread start per worker, so a long run of held outcomes should
        // take few rounds)
        if self.round_size == 0 {
            self.round_size = 64 * w;
        } else if self.round_made > 0 {
            if self.rep.dry_used - self.round_used0 >= self.round_made * 9 / 10 {
                self.round_size = (self.round_size * 2).min(4096 * w);
            } else {
                self.round_size = (self.round_size / 2).max(16 * w);
            }
        }
        self.round_used0 = self.rep.dry_used;
        let budget = self.round_size;
        // (the queue's first item, and the dirty steps after it along its
        // unfold, in order: a walk, not heap operations)
        let mut steps = Vec::with_capacity(budget);
        if let Some(&top) = self.heap.first() {
            let mut c = top;
            let mut walked = 0;
            while c != NONE && steps.len() < budget && walked < 4 * budget {
                let h = &self.n.h[c as usize];
                if h.kind != Kind::Step {
                    break;
                }
                if h.flags & (DEAD | DIRTY) == DIRTY
                    && self.n.opds_of(c).first().is_some_and(|o| o.src != NONE)
                {
                    steps.push(c);
                }
                c = h.next;
                walked += 1;
            }
        }
        if steps.len() < 2 {
            self.round_made = 0;
            self.round_cool = budget;
            return;
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        // (emission buffers from the pool, one per step, moved to workers)
        let mut pool = std::mem::take(&mut self.em_pool);
        let mut bufs: Vec<Vec<Emit<L>>> = (0..w).map(|_| Vec::new()).collect();
        for k in 0..steps.len() {
            bufs[k % w].push(pool.pop().unwrap_or_else(Emit::sized));
        }
        self.em_pool = pool;
        let me = &*self;
        let mut outs: Vec<(u32, Dry<L>)> = std::thread::scope(|sc| {
            let hs: Vec<_> = bufs
                .into_iter()
                .map(|mut buf| {
                    let (next, steps) = (&next, &steps);
                    sc.spawn(move || {
                        let mut out = Vec::new();
                        loop {
                            if me.cancel.load(AO::Relaxed) {
                                break;
                            }
                            let k = next.fetch_add(1, AO::Relaxed);
                            let Some(&s) = steps.get(k) else { break };
                            let em = buf.pop().unwrap_or_else(Emit::sized);
                            out.push((s, me.dry_step(s, em)));
                        }
                        out
                    })
                })
                .collect();
            hs.into_iter()
                .flat_map(|h| h.join().expect("a dry step panicked"))
                .collect()
        });
        self.rep.dry_runs += outs.len() as u64;
        self.round_made = outs.len() as u64;
        for (s, d) in outs.drain(..) {
            self.outcomes.insert(s, d);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(super) fn round(&mut self) {}

    /// The memo store's byte budget (0, the default: no store).
    ///
    /// # Panics
    ///
    /// If a worker panicked holding the store.
    pub fn set_memo_budget(&mut self, bytes: usize) {
        self.memo_on = bytes > 0;
        let mut m = self.memo.lock().expect("the memo store");
        m.budget = bytes;
        if m.bytes() > bytes {
            *m = Memo::default();
            m.budget = bytes;
        }
    }

    /// The memo store's (entries, bytes, probes, hits).
    ///
    /// # Panics
    ///
    /// If a worker panicked holding the store.
    pub fn memo_stats(&self) -> (usize, usize, u64, u64) {
        let m = self.memo.lock().expect("the memo store");
        (m.len(), m.bytes(), m.probes, m.hits)
    }

    /// The memo store as an image (to load into another graph, later).
    ///
    /// # Panics
    ///
    /// If a worker panicked holding the store.
    pub fn save_memo<C: Codec<L::Val>>(&self, c: &C) -> Vec<u8> {
        let mut out = Vec::new();
        self.memo.lock().expect("the memo store").save(c, &mut out);
        out
    }

    /// Entries from an image, up to the budget.
    ///
    /// # Errors
    ///
    /// A bad image: the store is then left empty.
    ///
    /// # Panics
    ///
    /// If a worker panicked holding the store.
    pub fn load_memo<C: Codec<L::Val>>(&mut self, c: &C, img: &[u8]) -> Result<(), MemoError> {
        self.memo.lock().expect("the memo store").load(c, img)
    }

    /// The flag that cancels a run (another thread may set it; the run
    /// clears it when it returns cancelled).
    #[must_use]
    pub fn cancel_token(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.cancel.clone()
    }
}
