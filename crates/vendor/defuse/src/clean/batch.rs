//! A step's field writes applied in parallel.
//!
//! A commit that changes no structure (no placements, the same effect)
//! writes only to the indexes kept per field: the definitions, the readers,
//! the contributions and the range readers of `(unit, field)`. A step's
//! merge records those writes ([`FieldOp`]) instead of making them; a flush
//! groups them by key, takes each key's cell out of the world, applies the
//! groups in parallel (each in the order its writes were recorded, which
//! is text order) and puts the cells back. Different keys share nothing, so
//! the order between groups does not matter.

use super::core::Core;
use super::ptree::NoMeasure;
use super::world::{AccMeasure, Entry, FieldCell, FrameId, Id, Key, Pos, World};
use crate::api::Locality;
use crate::values::{Field, FxHashMap, ValueId};

/// One write to a field's indexes (its readers are filed at once, in
/// the live reader indexes).
pub(crate) enum FieldOp {
    /// A definition entry filed (`if_changed`: unless the same is filed);
    /// the readers after `at` are re-asked.
    File { pos: Pos, fr: FrameId, e: Entry, at: Pos, if_changed: bool },
    /// A definition entry removed; the readers after `at` are re-asked.
    Unfile { pos: Pos, fr: FrameId, global: bool, at: Pos },
    /// A contribution set (`None`: removed); the range readers over it are
    /// re-asked.
    Contrib { pos: Pos, x: Option<ValueId> },
}

/// One key's cell, taken out of the world while its writes are applied.
struct Group {
    key: (u32, Field),
    ops: Vec<FieldOp>,
    cell: FieldCell,
    /// The world's reader indexes (live).
    rd: super::live::Readers,
    defs_touched: bool,
    acc_touched: bool,
    /// Readers to re-ask on the field (`true`: past the shadow bound,
    /// under CLEAN_SCOPED_CHECK only).
    recheck: Vec<(Id, Field, bool)>,
    done: bool,
    /// The positions it changed (definitions and contributions).
    changed: Vec<Pos>,
}

impl Group {
    fn apply(&mut self, core: &Core) {
        self.done = true;
        let f = self.key.1;
        for op in std::mem::take(&mut self.ops) {
            self.changed.push(match op {
                FieldOp::File { pos, .. } | FieldOp::Unfile { pos, .. } | FieldOp::Contrib { pos, .. } => pos,
            });
            match op {
                FieldOp::File { pos, fr, e, at, if_changed } => {
                    if if_changed {
                        let have = if e.loc == Locality::Global {
                            self.cell.defs.globals.get(&pos)
                        } else {
                            self.cell.defs.local.get(&fr).and_then(|m| m.get(&pos))
                        };
                        if have.is_some_and(|h| h.node == e.node && h.val == e.val && h.loc == e.loc) {
                            continue;
                        }
                    }
                    if e.loc == Locality::Global {
                        self.cell.defs.globals.insert(pos, e, &NoMeasure);
                    } else {
                        self.cell.defs.local.upsert(fr, &NoMeasure, |m| {
                            m.insert(pos, e, &NoMeasure);
                        });
                    }
                    // (A pending entry breaks no read: `pending_breaks_none`.)
                    if e.val.is_none() && super::build::pending_breaks_none(core, f) {
                        self.defs_touched = true;
                    } else {
                        self.def_changed(core, at, fr, e.loc == Locality::Global);
                    }
                }
                FieldOp::Unfile { pos, fr, global, at } => {
                    if global {
                        self.cell.defs.globals.remove(&pos, &NoMeasure);
                    } else {
                        let mut empty = false;
                        self.cell.defs.local.update(&fr, &NoMeasure, |m| {
                            m.remove(&pos, &NoMeasure);
                            empty = m.is_empty();
                        });
                        if empty {
                            self.cell.defs.local.remove(&fr, &NoMeasure);
                        }
                    }
                    self.def_changed(core, at, fr, global);
                }
                FieldOp::Contrib { pos, x } => {
                    let Some(mo) = core.acc(f) else { continue };
                    let m = AccMeasure {
                        m: &**mo,
                        v: &core.values,
                    };
                    match x {
                        Some(v) => {
                            self.cell.contribs.insert(pos, v, &m);
                        }
                        None => {
                            self.cell.contribs.remove(&pos, &m);
                        }
                    }
                    self.acc_touched = true;
                    if let Some(c) = self.rd.field(self.key.0, f) {
                        self.recheck.extend(c.over(pos).into_iter().map(|r| {
                            super::core::prof::count(21);
                            (r, f, false)
                        }));
                    }
                }
            }
        }
    }

    /// The field's definitions changed at `at`: its readers after it are
    /// re-asked, up to the next entry that shadows the change.
    fn def_changed(&mut self, core: &Core, at: Pos, fr: FrameId, global: bool) {
        self.defs_touched = true;
        let lo = at.saturating_sub(1);
        let f = self.key.1;
        let hi = super::build::shadow_end(core, Some(&self.cell.defs), f, at, fr, global);
        let Some(c) = self.rd.field(self.key.0, f) else { return };
        self.recheck.extend(
            c.readers
                .range(std::ops::Bound::Excluded(&(lo, Id::MAX)), std::ops::Bound::Excluded(&(hi, 0)))
                .map(|(k, ())| (k.1, f, false)),
        );
        if hi != Pos::MAX && super::build::scoped_check() {
            self.recheck.extend(
                c.readers
                    .range(std::ops::Bound::Excluded(&(hi - 1, Id::MAX)), std::ops::Bound::Unbounded)
                    .map(|(k, ())| (k.1, f, true)),
            );
        }
    }
}

/// The writes a step's merge recorded, by key in order.
#[derive(Default)]
pub(crate) struct Pending {
    ops: Vec<((u32, Field), FieldOp)>,
    /// The fields whose definitions or contributions are written (a
    /// result reading one waits for the flush; reader writes change no
    /// answer).
    pub fields: crate::values::FxHashSet<Field>,
}

impl Pending {
    pub(crate) fn push(&mut self, u: u32, f: Field, op: FieldOp) {
        self.fields.insert(f);
        self.ops.push(((u, f), op));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Applies the writes to `w` (in parallel on `exec` when there are
    /// many); the readers to ask again, each on its field.
    pub(crate) fn flush(&mut self, core: &Core, w: &mut World, exec: &mut dyn crate::api::Executor) -> Vec<(Id, Field, bool)> {
        self.fields.clear();
        if self.ops.is_empty() {
            return Vec::new();
        }
        let mut at: FxHashMap<(u32, Field), usize> = FxHashMap::default();
        let mut groups: Vec<Group> = Vec::new();
        for (key, op) in self.ops.drain(..) {
            let i = *at.entry(key).or_insert_with(|| {
                groups.push(Group {
                    key,
                    ops: Vec::new(),
                    cell: w.field(key.0, key.1).cloned().unwrap_or_default(),
                    rd: w.rd.clone(),
                    defs_touched: false,
                    acc_touched: false,
                    recheck: Vec::new(),
                    done: false,
                    changed: Vec::new(),
                });
                groups.len() - 1
            });
            groups[i].ops.push(op);
        }
        let par = exec.parallelism().max(1);
        let total: usize = groups.iter().map(|g| g.ops.len()).sum();
        super::core::prof::count(12);
        // (Workers pay off on many writes only.)
        if par == 1 || groups.len() == 1 || total < 4096 {
            for g in &mut groups {
                g.apply(core);
            }
        } else {
            // Chunks of about equal work, a few per worker.
            let n = (par * 4).min(groups.len());
            let per = total.div_ceil(n);
            let mut chunks: Vec<std::sync::Mutex<Vec<Group>>> = Vec::new();
            let mut cur: Vec<Group> = Vec::new();
            let mut size = 0;
            for g in groups.drain(..) {
                size += g.ops.len();
                cur.push(g);
                if size >= per {
                    chunks.push(std::sync::Mutex::new(std::mem::take(&mut cur)));
                    size = 0;
                }
            }
            if !cur.is_empty() {
                chunks.push(std::sync::Mutex::new(cur));
            }
            let run = |i: usize| {
                for g in super::core::lock(&chunks[i]).iter_mut() {
                    g.apply(core);
                }
                crate::api::RunResult(())
            };
            let mut out = Vec::with_capacity(chunks.len());
            exec.execute(chunks.len(), &run, &mut out);
            groups = chunks.into_iter().flat_map(|c| c.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)).collect();
            // (An executor that dropped a job: its groups here.)
            for g in groups.iter_mut().filter(|g| !g.done) {
                g.apply(core);
            }
        }
        let mut recheck = Vec::new();
        for g in groups {
            let key = g.key;
            for &p in &g.changed {
                w.note(super::world::Change::Field(key.0, key.1, p));
            }
            w.fields.insert(super::world::FKey::new(key.0, key.1), std::sync::Arc::new(g.cell), &NoMeasure);
            if g.defs_touched {
                w.touch_key(Key::Defs(key.0, key.1));
            }
            if g.acc_touched {
                w.touch_key(Key::Acc(key.0, key.1));
            }
            recheck.extend(g.recheck);
        }
        recheck
    }
}
