//! A step's run carried from one runtime to another (`DESIGN.md` 3.10,
//! "Per worker" and "Commit"): a worker runs a step on a runtime of its
//! own and hands over what the run recorded, its records, its reads from
//! outside the step with the versions they found, and the slots it left
//! as it found them; the runtime the build lives in takes them as the
//! step's run, as if it had run there ([`Runtime::import_step`]).
//!
//! A record names its children by their place in the arena, which is the
//! worker's: the export lists the records of the step's subtrees children
//! first, and a child is named by its place in that list.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fold::StepId;
use crate::hash::Version;
use crate::machine::{Loc, Machine};
use crate::runtime::{Item, RecId, Record, Runtime};
use crate::table::Table;

/// A record of a step's run, as exported: [`Record`]'s fields, its
/// writes with their values, its children named by their place in
/// [`StepExport::recs`], and whether it is a lean frame's.
pub struct ExportRec<M: Machine> {
    pub func: M::Func,
    pub name: Version,
    pub args: Vec<Version>,
    pub result: M::Val,
    pub reads: Vec<(Loc<M::Addr>, Version)>,
    pub writes: Vec<(M::Addr, Option<M::Val>)>,
    pub items: Vec<Item<M>>,
    pub cost: u64,
    pub own: u64,
    pub content: Version,
    pub lean: bool,
}

/// A step's run as another runtime takes it ([`Runtime::export_step`],
/// [`Runtime::import_step`]).
pub struct StepExport<M: Machine> {
    /// The records of the run, children before their parents.
    pub recs: Vec<ExportRec<M>>,
    /// The step's own records (its calls of the top level), by their
    /// place in `recs`, in order.
    pub top: Vec<u32>,
    /// Its reads from outside it (each with its address's hash), in the
    /// order read.
    pub reads: Vec<(u64, M::Addr)>,
    /// With versions kept ([`Runtime::keep_step_versions`]), the version
    /// each read found; else empty.
    pub vers: Vec<Version>,
    /// The slots it left as it found them, or wrote dead: not its
    /// definitions ([`Runtime::end_step_soft`]).
    pub skip: Vec<M::Addr>,
}

impl<M: Machine> StepExport<M> {
    /// The step's reads from outside it with the versions they found.
    pub fn versioned(&self) -> impl Iterator<Item = (&M::Addr, Version)> {
        self.reads
            .iter()
            .zip(self.vers.iter().copied().chain(core::iter::repeat(Version::ABSENT)))
            .map(|((_, a), v)| (a, v))
    }

    /// The slots its records wrote, each with the value its last write
    /// left, in the order of their last writes within each record, the
    /// step's own records in order (a slot written by two of them: the
    /// later one's).
    #[must_use]
    pub fn writes(&self) -> Vec<(M::Addr, Option<M::Val>)>
    where
        M::Addr: Ord,
    {
        let mut out: Vec<(M::Addr, Option<M::Val>)> = Vec::new();
        let mut at: alloc::collections::BTreeMap<M::Addr, usize> =
            alloc::collections::BTreeMap::new();
        for &t in &self.top {
            for (a, v) in &self.recs[t as usize].writes {
                if let Some(&i) = at.get(a) {
                    out[i].1.clone_from(v);
                } else {
                    at.insert(a.clone(), out.len());
                    out.push((a.clone(), v.clone()));
                }
            }
        }
        out
    }
}

impl<M: Machine> Runtime<M> {
    /// End the open step without closing it into this runtime's fold: its
    /// soft reads settled as [`Runtime::end_step_soft`] settles them
    /// (`read` the slots whose entry values it left saved, `skip` those it
    /// left as it found them), and its run handed over, the records taken
    /// out of the arena. `None`: no step is open.
    pub fn export_step(&mut self, read: &[M::Addr], skip: Vec<M::Addr>) -> Option<StepExport<M>> {
        self.open.step?;
        if !read.is_empty() {
            let have: alloc::collections::BTreeSet<&M::Addr> =
                self.open.step_reads.iter().map(|(_, a)| a).collect();
            let new: Vec<M::Addr> = read
                .iter()
                .filter(|a| !have.contains(a))
                .cloned()
                .collect();
            for a in &new {
                self.open.force_step_read(a);
            }
        }
        self.open.step = None;
        let top_ids = core::mem::take(&mut self.open.step_recs);
        let reads = core::mem::take(&mut self.open.step_reads);
        let vers = self
            .open
            .step_vers
            .as_mut()
            .map(core::mem::take)
            .unwrap_or_default();
        self.open.step_read_at.clear();
        self.open.step_wrote_at = Table::new();
        // (the subtrees' records, children first: a depth-first walk
        // that places each record after its children)
        let mut place: BTreeMap<RecId, u32> = BTreeMap::new();
        let mut order: Vec<RecId> = Vec::new();
        for &t in &top_ids {
            let mut stack: Vec<(RecId, bool)> = alloc::vec![(t, false)];
            while let Some((id, done)) = stack.pop() {
                if place.contains_key(&id) {
                    continue;
                }
                if done {
                    place.insert(id, u32::try_from(order.len()).expect("fewer than 2^32"));
                    order.push(id);
                    continue;
                }
                stack.push((id, true));
                for c in self.record(id).children() {
                    if !place.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
            }
        }
        let recs = order
            .iter()
            .map(|&id| {
                let r = self.record(id);
                ExportRec {
                    func: r.func,
                    name: r.name,
                    args: r.args.clone(),
                    result: r.result.clone(),
                    reads: r.reads.clone(),
                    writes: self.wa.get(r.w).to_vec(),
                    items: r
                        .items
                        .iter()
                        .map(|it| match it {
                            Item::Call(c) => Item::Call(*place.get(c).expect("a child placed")),
                            o => o.clone(),
                        })
                        .collect(),
                    cost: r.cost,
                    own: r.own,
                    content: r.content,
                    lean: self.lean.get(id as usize).copied().unwrap_or(false),
                }
            })
            .collect();
        let top = top_ids
            .iter()
            .map(|t| *place.get(t).expect("a record placed"))
            .collect();
        Some(StepExport {
            recs,
            top,
            reads,
            vers,
            skip,
        })
    }

    /// Take `x`, a run of step `id` made by another runtime, as the step's
    /// run here, the step open ([`Runtime::rerun_step`] or
    /// [`Runtime::begin_step`]): its records put in the arena (each found
    /// equal to one there, or added), and the step closed with its reads,
    /// as [`Runtime::end_step_soft`] closes a run made here.
    pub fn import_step(&mut self, id: StepId, x: StepExport<M>) -> Option<StepId> {
        debug_assert_eq!(self.open.step.map(|s| s.0), Some(id), "the step open");
        let mut ids: Vec<RecId> = Vec::with_capacity(x.recs.len());
        for r in x.recs {
            let items = r
                .items
                .into_iter()
                .map(|it| match it {
                    Item::Call(k) => Item::Call(ids[k as usize]),
                    o => o,
                })
                .collect();
            let rec = Record {
                func: r.func,
                name: r.name,
                args: r.args,
                result: r.result,
                reads: r.reads,
                w: crate::runtime::WSpan::default(),
                items,
                cost: r.cost,
                own: r.own,
                content: r.content,
            };
            let live = self.live_records();
            let rid = self.place(rec, r.writes);
            self.note_lean(rid, self.live_records() > live, r.lean);
            ids.push(rid);
        }
        for &t in &x.top {
            let rid = ids[t as usize];
            self.open.step_recs.push(rid);
            if let Some(root) = self.open.frames.first_mut() {
                root.items.push(Item::Call(rid));
            }
        }
        self.open.step_reads = x.reads;
        self.end_step_soft(&[], &x.skip)
    }

    /// Let every record go (a worker's runtime between two runs): the
    /// arena, the memo and the fold start empty; the recorder's tables
    /// stay, as a trip's do.
    pub fn reset_records(&mut self) {
        self.recs.clear();
        self.wa = crate::runtime::Writes::default();
        self.free.clear();
        self.memo.clear();
        self.dedup.clear();
        self.roots.clear();
        self.live = 0;
        self.live_after_gc = 0;
        self.inert.clear();
        self.lean.clear();
        self.owner.clear();
        self.fold = crate::fold::Fold::default();
        self.log.clear();
    }
}
