//! A step's field writes applied in parallel.
//!
//! A commit that changes no structure (no placements, the same effect)
//! writes only to the indexes kept per field: the definitions, the readers,
//! the contributions and the range readers of `(unit, field)`. A step's
//! merge records those writes ([`FieldOp`]) instead of making them; a flush
//! groups them by key, takes each key's maps out of the world, applies the
//! groups in parallel (each in the order its writes were recorded, which
//! is text order) and puts the maps back. Different keys share nothing, so
//! the order between groups does not matter.

use super::core::Core;
use super::ptree::{NoMeasure, PMap};
use super::world::{AccMeasure, Entry, FieldDefs, FrameId, Id, Key, Pos, World};
use crate::api::Locality;
use crate::values::{Field, FxHashMap, ValueId};

/// One write to a field's indexes.
pub(crate) enum FieldOp {
    /// A reader of the field at `pos` (`on`), or not.
    Reader { pos: Pos, id: Id, on: bool },
    /// A range reader of the accumulator over `(lo, hi)`, or not.
    RangeReader { lo: Pos, hi: Pos, id: Id, on: bool },
    /// A definition entry filed (`if_changed`: unless the same is filed);
    /// the readers after `at` are re-asked.
    File { pos: Pos, fr: FrameId, e: Entry, at: Pos, if_changed: bool },
    /// A definition entry removed; the readers after `at` are re-asked.
    Unfile { pos: Pos, fr: FrameId, global: bool, at: Pos },
    /// A contribution set (`None`: removed); the range readers over it are
    /// re-asked.
    Contrib { pos: Pos, x: Option<ValueId> },
}

/// One key's maps, taken out of the world while its writes are applied.
struct Group {
    key: (u32, Field),
    ops: Vec<FieldOp>,
    defs: FieldDefs,
    readers: PMap<(Pos, Id), ()>,
    contribs: PMap<Pos, ValueId, ValueId>,
    range_readers: PMap<(Pos, Id), Pos>,
    defs_touched: bool,
    acc_touched: bool,
    /// Readers to re-ask on the field (`true`: past the shadow bound,
    /// under CLEAN_SCOPED_CHECK only).
    recheck: Vec<(Id, Field, bool)>,
    done: bool,
}

impl Group {
    fn apply(&mut self, core: &Core) {
        self.done = true;
        let f = self.key.1;
        for op in std::mem::take(&mut self.ops) {
            match op {
                FieldOp::Reader { pos, id, on } => {
                    if on {
                        self.readers.insert((pos, id), (), &NoMeasure);
                    } else {
                        self.readers.remove(&(pos, id), &NoMeasure);
                    }
                }
                FieldOp::RangeReader { lo, hi, id, on } => {
                    if on {
                        self.range_readers.insert((hi, id), lo, &NoMeasure);
                    } else {
                        self.range_readers.remove(&(hi, id), &NoMeasure);
                    }
                }
                FieldOp::File { pos, fr, e, at, if_changed } => {
                    if if_changed {
                        let have = if e.loc == Locality::Global {
                            self.defs.globals.get(&pos)
                        } else {
                            self.defs.local.get(&fr).and_then(|m| m.get(&pos))
                        };
                        if have.is_some_and(|h| h.node == e.node && h.val == e.val && h.loc == e.loc) {
                            continue;
                        }
                    }
                    if e.loc == Locality::Global {
                        self.defs.globals.insert(pos, e, &NoMeasure);
                    } else {
                        self.defs.local.upsert(fr, &NoMeasure, |m| {
                            m.insert(pos, e, &NoMeasure);
                        });
                    }
                    self.def_changed(core, at, fr, e.loc == Locality::Global);
                }
                FieldOp::Unfile { pos, fr, global, at } => {
                    if global {
                        self.defs.globals.remove(&pos, &NoMeasure);
                    } else {
                        let mut empty = false;
                        self.defs.local.update(&fr, &NoMeasure, |m| {
                            m.remove(&pos, &NoMeasure);
                            empty = m.is_empty();
                        });
                        if empty {
                            self.defs.local.remove(&fr, &NoMeasure);
                        }
                    }
                    self.def_changed(core, at, fr, global);
                }
                FieldOp::Contrib { pos, x } => {
                    let Some(mo) = core.accs.get(&f) else { continue };
                    let m = AccMeasure {
                        m: &**mo,
                        v: &core.values,
                    };
                    match x {
                        Some(v) => {
                            self.contribs.insert(pos, v, &m);
                        }
                        None => {
                            self.contribs.remove(&pos, &m);
                        }
                    }
                    self.acc_touched = true;
                    self.recheck.extend(
                        self.range_readers
                            .range(std::ops::Bound::Excluded(&(pos, Id::MAX)), std::ops::Bound::Unbounded)
                            .filter(|(_, lo)| **lo < pos)
                            .map(|(k, _)| (k.1, f, false)),
                    );
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
        let hi = super::build::shadow_end(core, Some(&self.defs), f, at, fr, global);
        self.recheck.extend(
            self.readers
                .range(std::ops::Bound::Excluded(&(lo, Id::MAX)), std::ops::Bound::Excluded(&(hi, 0)))
                .map(|(k, ())| (k.1, f, false)),
        );
        if hi != Pos::MAX && super::build::scoped_check() {
            self.recheck.extend(
                self.readers
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
        if !matches!(op, FieldOp::Reader { .. } | FieldOp::RangeReader { .. }) {
            self.fields.insert(f);
        }
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
                    defs: w.defs.get(&key).cloned().unwrap_or_default(),
                    readers: w.readers.get(&key).cloned().unwrap_or_default(),
                    contribs: w.contribs.get(&key).cloned().unwrap_or_default(),
                    range_readers: w.range_readers.get(&key).cloned().unwrap_or_default(),
                    defs_touched: false,
                    acc_touched: false,
                    recheck: Vec::new(),
                    done: false,
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
            w.defs.insert(key, g.defs, &NoMeasure);
            w.readers.insert(key, g.readers, &NoMeasure);
            if !g.contribs.is_empty() || w.contribs.get(&key).is_some() {
                w.contribs.insert(key, g.contribs, &NoMeasure);
            }
            if !g.range_readers.is_empty() || w.range_readers.get(&key).is_some() {
                w.range_readers.insert(key, g.range_readers, &NoMeasure);
            }
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
