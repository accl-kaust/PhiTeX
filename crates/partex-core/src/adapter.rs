//! A prototype of the engine as `partex_incr::Machine`, over one slice of
//! its state: the eqtb cells (`DESIGN.md` §7.0, "TeX's adapter").
//!
//! The runtime's trace applies a region by writing the values of the
//! cells it wrote into another state ([`partex_incr::Trace::apply`]).
//! For eqtb that is [`Tex::import_cells`] (a value is a word plus the
//! token list, glue, box or shape it names, copied by content), and two
//! states agree on a cell when its content does ([`Tex::cell_content`],
//! which is also the cell's version for guards). The sanitizer checks the
//! round trip per region: the entry with the region's writes applied has
//! the exit's eqtb.

use alloc::vec::Vec;

use crate::host::Host;
use crate::objs::Obj;
use crate::tex::Tex;
use crate::track::{Cell, Tracker};
use crate::web::{HASH_BASE, INT_BASE};

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Apply the writes `written` of a region that ended in `exit`: the
    /// eqtb cells among them take `exit`'s values. Returns the written
    /// cells outside the slice, left alone.
    pub fn apply_writes(&mut self, exit: &Self, written: &[Cell]) -> Vec<Cell> {
        let mut eqtb = Vec::new();
        let mut rest = Vec::new();
        for &c in written {
            match c {
                Cell::Eqtb(p) if (1..crate::xregs::EXT_BASE).contains(&p) => eqtb.push(p),
                _ => rest.push(c),
            }
        }
        if !self.import_cells(exit, &eqtb) {
            rest.extend(eqtb.into_iter().map(Cell::Eqtb));
        }
        rest
    }

    /// The eqtb cells (the registers above 255 included) whose content or save
    /// level differs from `other`'s.
    pub fn eqtb_differences(&self, other: &Self) -> Vec<Cell> {
        // An entry is its word and the object beside it (`objs.rs`, DESIGN
        // 7.17.12). The words are compared by chunk, a chunk the two
        // states share holding the same words. The objects are compared
        // apart: the word of an entry that holds one is the same whatever
        // it holds (its `equiv` is null, or a macro's `\protected` flag),
        // and a chunk of words the two states share says nothing of the
        // objects beside it (a `\def` that gives a macro a new body
        // writes its word back as it was). The objects are journaled in
        // chunks of their own, a chunk the two states share holding the
        // same objects; in the others, an object that is the very value
        // in both states is the same without a look, and any other pair
        // is compared by content, as the cell's version is
        // (`Tex::cell_content`).
        let words = self
            .eqtb
            .differences(&other.eqtb, |a, b| a.bits() == b.bits());
        let objs = self
            .eqtb_obj
            .differences(&other.eqtb_obj, <Option<Obj> as crate::journal::Elem>::same);
        let mut out: Vec<Cell> = words
            .into_iter()
            .chain(objs)
            .filter_map(|i| i32::try_from(i).ok())
            .map(Cell::Eqtb)
            .filter(|&c| self.cell_content(c) != other.cell_content(c))
            .collect();
        // (a slot's name is part of the value only with names as cells:
        // below)
        for (i, (a, b)) in self.xeq_level.iter().zip(&other.xeq_level).enumerate() {
            if a != b {
                out.push(Cell::Eqtb(INT_BASE + i32::try_from(i).unwrap_or(0)));
            }
        }
        if self.name_cells && other.name_cells && self.hash.len() == other.hash.len() {
            // (with names as cells, the hash slots whose names differ, by
            // their characters, as their eqtb cells; those whose links
            // differ)
            for i in self
                .hash
                .differences(&other.hash, |a, b| a.bits() == b.bits())
            {
                let p = HASH_BASE + i32::try_from(i).unwrap_or(0);
                if *self.slot_name(p) != *other.slot_name(p) {
                    out.push(Cell::Eqtb(p));
                }
                if self.hash[i].lh() != other.hash[i].lh() {
                    out.push(Cell::HashNext(p));
                }
            }
        }
        // (the registers above 255 set in either, by content)
        let locs: alloc::collections::BTreeSet<i32> =
            self.xregs.locs().chain(other.xregs.locs()).collect();
        for p in locs {
            let c = Cell::Eqtb(p);
            if self.cell_content(c) != other.cell_content(c) {
                out.push(c);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}
