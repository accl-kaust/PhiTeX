//! Lists left open across nodes (docs/spec-slices.md §6, §6.1;
//! `PHITEX_BOXSCOPE=1`, off by default).
//!
//! A run at rest at its text's end with lists of its own open (a box
//! being built, the paragraph the box is in) ends there instead of
//! reading on: what is inside the box is cut into slices like any text,
//! and each is a node of its own.
//!
//! Lists and groups are two stacks in TeX (the nest and the save stack,
//! §211, §268): a paragraph begun inside a group goes on after the group
//! closes. So they are left apart:
//!
//! - every group open (a box's own group among them) is an engine scope,
//!   as [`Run::leave_scope`] leaves one; its kind carries the mode and
//!   the id of the list it is in;
//! - every list open is a list id unique among nodes ([`global_lid`]),
//!   its items so far contributions under that id ([`Run::append`]), as
//!   the main list's are; the node that ends it folds them
//!   ([`Run::list_fold`]);
//! - a paragraph, which no group brackets, is a field `par:<list it is
//!   in>` holding its id and its own parameters, read only where a
//!   group's close leaves the paragraph on top again
//!   ([`Run::reown_paragraph`]).

use crate::build::sf_value;
use crate::nest::Level;
use crate::run::{R, Run};
use crate::val::{group, int_of, list_of};
use phitex_defuse::ValueId;

/// Whether open lists may be left to the following nodes.
pub(crate) fn on() -> bool {
    crate::exec::env_flag!("PHITEX_BOXSCOPE")
}

/// A list id for the `k`-th list a node leaves open: one no other node's
/// list has (a run's own ids are small negative counters).
fn global_lid(node: u64, k: usize) -> i64 {
    let n = i64::try_from(node & ((1 << 50) - 1)).unwrap_or(0);
    -(1i64 << 55) - n * 64 - i64::try_from(k).unwrap_or(63).min(63)
}

/// The field holding the paragraph open in list `vlid`.
fn par_field(vlid: i64) -> String {
    format!("par:{vlid}")
}

fn is_box_group(g: i64) -> bool {
    matches!(
        g,
        group::HBOX | group::ADJUSTED_HBOX | group::VBOX | group::VTOP
    )
}

impl Run<'_, '_> {
    /// Whether the run's open levels can all be left (`leave_lists`): a
    /// text piece or a resume with no conditional, alignment, scope effect,
    /// picture child or character run pending; the innermost level a
    /// group; each level a group sharing the list below, a box's group, or
    /// a paragraph with a group above it.
    pub(crate) fn can_leave_lists(&self) -> bool {
        let op = self.cx.op();
        if !((op == self.ops.exec || op == self.ops.resume)
            && !self.nest.is_empty()
            && self.nest.len() < usize::from(u8::MAX - 2)
            && self.conds.is_empty()
            && self.effect == phitex_defuse::Effect::None
            && self.aopen == 0
            && self.pic_child.is_none()
            && self.chars_live.is_none()
            && self.appending.is_empty()
            && self.appending_vals.is_empty()
            && self.nest.last().is_some_and(|l| l.group != 0)
            && self.nest.iter().any(|l| l.owns))
        {
            return false;
        }
        self.nest.iter().enumerate().all(|(k, l)| {
            if !l.owns {
                l.group != 0
            } else if l.group == 0 {
                l.mode == 102
                    && l.par.is_some()
                    && self.nest.get(k + 1).is_some_and(|u| u.group != 0)
            } else {
                is_box_group(l.group) && l.par.is_none() && l.prev_graf == 0
            }
        })
    }

    /// Leaves the run's open levels to the following nodes: each list's
    /// items as contributions under its global id, each paragraph's
    /// parameters as `par:<list>`, the groups as scopes
    /// ([`Run::leave_scope`]).
    pub(crate) fn leave_lists(&mut self) -> R<()> {
        let v = self.v;
        let node = self.cx.node().raw();
        let base = self.base_mode()?;
        self.muts += 1;
        let mut levels = std::mem::take(&mut self.nest);
        // the run's own ids made global
        let mut k = 0;
        for i in 0..levels.len() {
            if !levels[i].owns {
                continue;
            }
            let old = levels[i].lid;
            let new = global_lid(node, k);
            k += 1;
            for l in &mut levels[i..] {
                if l.lid == old {
                    l.lid = new;
                }
            }
        }
        // each list's items and its transitions' state
        for i in 0..levels.len() {
            if !levels[i].owns {
                continue;
            }
            let lid = levels[i].lid;
            let items = crate::nest::into_values(v, std::mem::take(&mut levels[i].list));
            for x in items {
                self.append(lid, x);
            }
            let l = &levels[i];
            if l.mode.abs() == 1 {
                let pd = crate::build::pd_value(v, l.fold.pd);
                self.append(lid, pd);
            } else {
                self.append(lid, sf_value(v, l.fold.sf));
            }
            if l.group == 0 {
                // a paragraph: what its end reads besides its items
                let vlid = match i.checked_sub(1) {
                    Some(b) => levels[b].lid,
                    None => base.lid,
                };
                let (lang, packed) = l.par.unwrap_or((0, 0));
                let rec = v.list(vec![v.int(lid), v.int(lang), v.int(packed), v.int(l.clang)]);
                self.set(&par_field(vlid), rec, true);
            }
        }
        // the groups as scopes
        self.nest = levels.into_iter().filter(|l| l.group != 0).collect();
        self.leave_scope()
    }

    /// After the close of an engine scope whose kind `md` is a group in
    /// a paragraph left open (`leave_lists`) and whose outer kind is the
    /// vertical list the paragraph is in: the paragraph is this run's
    /// again, its items folded, its parameters read from `par:<list>`.
    pub(crate) fn reown_paragraph(
        &mut self,
        md: &crate::build::Mode,
        outer: &crate::build::Mode,
    ) -> R<bool> {
        if md.km != 102 || outer.km.abs() != 1 || md.lid > -(1i64 << 55) {
            return Ok(false);
        }
        let v = self.v;
        let Some(rec) = self.get(&par_field(outer.lid))? else {
            return Ok(false);
        };
        let xs = list_of(v, rec);
        let n = |i: usize| xs.get(i).map_or(0, |&x| int_of(v, x));
        if xs.len() < 4 || n(0) != md.lid {
            return Ok(false);
        }
        let f = self.list_fold(md.lid)?;
        let mut l = Level::new(102, 0, md.lid, true, Vec::new());
        self.open_level(&mut l);
        l.list = crate::nest::of_values(f.items);
        l.fold = crate::build::FoldState {
            sf: f.sf,
            pd: f.pd,
            since_bp: f.since_bp,
        };
        l.par = Some((n(1), n(2)));
        l.clang = n(3);
        self.set(&par_field(outer.lid), ValueId::UNIT, true);
        self.muts += 1;
        self.nest.push(l);
        Ok(true)
    }
}
