//! The engine's side of the tracking sanitizer (`DESIGN.md` §7.12).
//!
//! Guards are sound only if every read of a tracked cell reaches the
//! [`Tracker`]. The sanitizer checks that for a region (the commands
//! between two stops, [`Tex::set_stop_at`]) in two ways, both driven by the
//! command line (`PARTEX_SANITIZE`):
//!
//! - **Shadow diff** ([`Tex::changed_cells`]): the cells whose words differ
//!   between the region's entry and exit must all have been reported as
//!   written.
//! - **Poisoning** ([`Tex::poison_cells`]): a copy of the entry state gets
//!   a different value in every tracked cell the region did not report
//!   reading, and runs the same region. If the region's output, its
//!   writes or the untracked state differ from the real run's, some read
//!   bypassed the tracker; bisecting the poisoned set names the cell.
//!
//! Poison is a valid value of the cell's kind where that is cheap (an
//! integer or dimension off by one, a catcode moved on, a meaning made
//! undefined or `\relax`, a token list, box or shape emptied, glue made
//! zero or 1sp, a font changed, a name renamed); cells without a cheap
//! valid alternative are left alone. Poisoning writes the tables directly,
//! past the tracker and without reference counts: the poisoned engine is
//! thrown away after the comparison.

use alloc::vec::Vec;
use core::hash::Hash;

use partex_engine::stablehash::StableHasher;

use crate::cmds::{BOX_REF, GLUE_REF, SHAPE_REF, UNDEFINED_CS};
use crate::host::Host;
use crate::mem::{MemoryWord, NULL};
use crate::tex::Tex;
use crate::track::{Cell, Tracker};
use crate::web::{
    ACTIVE_BASE, CAT_CODE_BASE, CUR_FONT_LOC, EQTB_SIZE, ETEX_PEN_BASE, ETEX_PENS,
    FROZEN_PROTECTION, GLUE_BASE, HASH_BASE, INT_BASE, LC_CODE_BASE, LOCAL_BASE, MATH_FONT_BASE,
    NULL_CS, PAR_SHAPE_LOC, RELAX, TOK_VAL, UNDEFINED_CONTROL_SEQUENCE, XORD_CODE_BASE,
};
use crate::xregs::{EXT_BASE, ext_reg, is_word_kind};

/// Whether an eqtb location holds a meaning (a control sequence or an
/// active or single character).
fn is_meaning(p: i32) -> bool {
    (ACTIVE_BASE..GLUE_BASE).contains(&p) || (p > EQTB_SIZE && p < EXT_BASE)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The tracked cells whose words differ between `self` and `other`
    /// (eqtb with its levels, the hash, registers above 255). Tables the
    /// two share since a clone are skipped unread.
    #[must_use]
    pub fn changed_cells(&self, other: &Self) -> Vec<Cell> {
        let mut out: Vec<Cell> = self
            .eqtb
            .differences(&other.eqtb, |a, b| a.bits() == b.bits())
            .into_iter()
            .filter_map(|i| i32::try_from(i).ok())
            .map(Cell::Eqtb)
            .collect();
        for (i, (a, b)) in self.xeq_level.iter().zip(&other.xeq_level).enumerate() {
            if a != b {
                out.push(Cell::Eqtb(INT_BASE + i32::try_from(i).unwrap_or(0)));
            }
        }
        for i in self
            .hash
            .differences(&other.hash, |a, b| a.bits() == b.bits())
        {
            let p = HASH_BASE + i32::try_from(i).unwrap_or(0);
            let (a, b) = (self.hash[i], other.hash[i]);
            if a.rh() != b.rh() {
                out.push(Cell::Hash(p));
            }
            if a.lh() != b.lh() {
                out.push(Cell::HashNext(p));
            }
        }
        let mut ext: Vec<i32> = self
            .xregs
            .cells()
            .chain(other.xregs.cells())
            .map(|(l, _, _)| l)
            .collect();
        ext.sort_unstable();
        ext.dedup();
        for l in ext {
            if self.xregs.get(l).bits() != other.xregs.get(l).bits()
                || self.xregs.level(l) != other.xregs.level(l)
            {
                out.push(Cell::Eqtb(l));
            }
        }
        // what the words that did not change name, by value
        self.named_differences(other, &mut out);
        // the other families, compared whole
        let fonts = self.font_ptr.max(other.font_ptr);
        let families = (0..=fonts)
            .map(Cell::Font)
            .chain((0..16).map(Cell::Read))
            .chain((0..16).map(Cell::Out))
            .chain([Cell::FontTable, Cell::Random]);
        for c in families {
            if self.family_digest(c) != other.family_digest(c) {
                out.push(c);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The eqtb locations (and registers above 255) whose words are the
    /// same in both states but name a token list, glue, shape or box that
    /// differs: a value changed in place under its id, as `\wd` changes
    /// a box register's box (§1247), without a write of the word. The
    /// words' differences above cannot see those; a value shared by both
    /// states (the same list, the same `Arc`) is equal without a look.
    fn named_differences(&self, other: &Self, out: &mut Vec<Cell>) {
        extern crate std;
        use crate::cmds::{CALL, END_TEMPLATE, LONG_OUTER_CALL};
        use crate::web::{BOX_VAL, GLUE_VAL, MU_VAL, OUTPUT_ROUTINE_LOC};
        // (the values entries hold: a shared value is equal without a look)
        let differs_at = |p: i32| -> bool {
            use crate::objs::Obj;
            match (self.peek_obj(p), other.peek_obj(p)) {
                (None, None) => false,
                (Some(Obj::Toks(a)), Some(Obj::Toks(b))) => {
                    !(alloc::sync::Arc::ptr_eq(a, b) || a == b)
                }
                (Some(Obj::Box(a)), Some(Obj::Box(b))) => {
                    !(alloc::sync::Arc::ptr_eq(a, b) || **a == **b)
                }
                (Some(Obj::Shape(a)), Some(Obj::Shape(b))) => {
                    !(alloc::sync::Arc::ptr_eq(a, b) || a == b)
                }
                (Some(Obj::Glue(a)), Some(Obj::Glue(b))) => a != b,
                _ => true,
            }
        };
        if std::env::var_os("PARTEX_SANITIZE_NAMED").is_some_and(|v| v == "0") {
            return;
        }
        // (the locations in use, a chunk's words at a time: the unchanged
        // words, as a changed one is found above)
        let top = usize::try_from(self.eqtb_top.max(other.eqtb_top).max(EQTB_SIZE))
            .unwrap_or(0)
            .min(usize::try_from(EXT_BASE - 1).unwrap_or(0));
        let (ints, dims) = (
            usize::try_from(INT_BASE).unwrap_or(0),
            usize::try_from(EQTB_SIZE).unwrap_or(0),
        );
        let mut words: Vec<(i32, MemoryWord)> = Vec::new();
        for c in 0..self.eqtb.chunks().min(other.eqtb.chunks()) {
            let base = c * crate::journal::CHUNK;
            if base > top {
                break;
            }
            let ((a, _), (b, _)) = (self.eqtb.view(c), other.eqtb.view(c));
            for (i, (w, x)) in a.iter().zip(b).enumerate() {
                let p = base + i;
                if p == 0 || p > top || (ints..=dims).contains(&p) || w.bits() != x.bits() {
                    continue;
                }
                words.push((i32::try_from(p).unwrap_or(0), *w));
            }
        }
        for (p, w) in words {
            let differs = if (OUTPUT_ROUTINE_LOC..ETEX_PEN_BASE).contains(&p) {
                differs_at(p)
            } else {
                match w.b0() {
                    CALL..=LONG_OUTER_CALL | END_TEMPLATE | GLUE_REF | SHAPE_REF | BOX_REF => {
                        differs_at(p)
                    }
                    _ => false,
                }
            };
            if differs {
                out.push(Cell::Eqtb(p));
            }
        }
        for (l, w, _) in self.xregs.cells() {
            if other.xregs.get(l).bits() != w.bits() {
                continue;
            }
            let differs = match ext_reg(l).0 {
                TOK_VAL | BOX_VAL | GLUE_VAL | MU_VAL => differs_at(l),
                _ => false,
            };
            if differs {
                out.push(Cell::Eqtb(l));
            }
        }
    }

    /// The value of a cell outside eqtb and the hash, hashed.
    pub(crate) fn family_digest(&self, c: Cell) -> u128 {
        let mut h = StableHasher::new();
        match c {
            Cell::Eqtb(_) | Cell::Hash(_) | Cell::HashNext(_) => {
                unreachable!("not a family: {c:?}")
            }
            Cell::Str(_) => {
                // the strings a search for these characters would find
                let (pool, start) = (
                    self.str_pool
                        .prefix(self.pool_ptr)
                        .iter()
                        .copied()
                        .collect::<Vec<u8>>(),
                    self.str_start
                        .prefix(self.str_ptr + 1)
                        .iter()
                        .copied()
                        .collect::<Vec<usize>>(),
                );
                for s in self.init_str_ptr..self.str_ptr {
                    let b = &pool[start[s]..start[s + 1]];
                    if crate::strings::str_cell(b) == c {
                        (s, b).hash(&mut h);
                    }
                }
            }
            Cell::Font(f) => {
                let i = usize::try_from(f).unwrap_or(usize::MAX);
                let fonts = &self.fonts;
                (
                    fonts.metrics.get(i),
                    fonts.hyphen_char.get(i),
                    fonts.skew_char.get(i),
                    fonts.codes.get(i),
                    fonts.expand.get(i),
                )
                    .hash(&mut h);
            }
            Cell::FontTable => {
                let n = usize::try_from(self.font_ptr + 1).unwrap_or(0);
                let fonts = &self.fonts;
                (self.font_ptr, &fonts.name[..n], &fonts.area[..n]).hash(&mut h);
                for m in &fonts.metrics[..n] {
                    (m.size, m.design_size).hash(&mut h);
                }
            }
            Cell::Read(n) => {
                let i = usize::try_from(n).unwrap_or(usize::MAX);
                let file = self.read_file.get(i).and_then(Option::as_ref);
                (self.read_open.get(i), file.map(|f| (&f.data[..], f.pos))).hash(&mut h);
            }
            Cell::Out(n) => {
                let i = usize::try_from(n).unwrap_or(usize::MAX);
                self.write_open.get(i).hash(&mut h);
            }
            Cell::Random => self.random.hash(&mut h),
        }
        h.finish128()
    }

    /// Every tracked cell in use: eqtb outside the control sequences, the
    /// control sequences that have a name, their hash slots, and the
    /// registers above 255 that differ from their default.
    #[must_use]
    pub fn cells_in_use(&self) -> Vec<Cell> {
        let mut out = Vec::new();
        let named = |p: i32| {
            usize::try_from(p - HASH_BASE)
                .ok()
                .filter(|&i| i < self.hash.len())
                .is_some_and(|i| self.hash[i].rh() != 0)
        };
        for p in 1..i32::try_from(self.eqtb.len()).unwrap_or(0) {
            let cs = (HASH_BASE..UNDEFINED_CONTROL_SEQUENCE).contains(&p) || p > EQTB_SIZE;
            if !cs || named(p) {
                out.push(Cell::Eqtb(p));
            }
            if cs && named(p) {
                out.push(Cell::Hash(p));
            }
        }
        out.extend(self.xregs.cells().map(|(l, _, _)| Cell::Eqtb(l)));
        out.extend((1..=self.font_ptr).map(Cell::Font));
        out.extend((0..16).map(Cell::Read));
        out.extend((0..16).map(Cell::Out));
        out.extend([Cell::FontTable, Cell::Random]);
        out.sort_unstable();
        out
    }

    /// Give each of `cells` a different value where a cheap valid one
    /// exists (see the module documentation), past the tracker. Returns
    /// the cells poisoned.
    pub fn poison_cells(&mut self, cells: &[Cell]) -> Vec<Cell> {
        let mut done = Vec::new();
        for &c in cells {
            if self.poison(c) {
                done.push(c);
            }
        }
        // (meanings changed under the skipped-text cache)
        self.skip.epoch += 1;
        done
    }

    fn poison(&mut self, c: Cell) -> bool {
        match c {
            Cell::Font(f) => {
                let i = usize::try_from(f).unwrap_or(usize::MAX);
                if f < 1 || f > self.font_ptr {
                    return false;
                }
                for p in self.fonts.params_mut(f).iter_mut() {
                    *p ^= 1;
                }
                self.fonts.hyphen_char[i] ^= 1;
                self.fonts.skew_char[i] ^= 1;
                self.fonts.glue[i] = None;
                true
            }
            // (nor for the pool's strings)
            Cell::FontTable | Cell::Str(_) | Cell::HashNext(_) => false,
            Cell::Read(n) => {
                let i = usize::try_from(n).unwrap_or(usize::MAX);
                if self
                    .read_open
                    .get(i)
                    .is_none_or(|&o| o == crate::web::CLOSED)
                {
                    return false;
                }
                self.read_open[i] = crate::web::CLOSED;
                self.read_file[i] = None;
                true
            }
            Cell::Out(n) => {
                let i = usize::try_from(n).unwrap_or(usize::MAX);
                if !self.write_open.get(i).copied().unwrap_or(false) {
                    return false;
                }
                self.write_open[i] = false;
                true
            }
            Cell::Random => {
                self.random.random_seed ^= 1;
                let seed = self.random.random_seed;
                self.random.init_randoms(seed);
                true
            }
            Cell::Hash(p) => {
                let Ok(i) = usize::try_from(p - HASH_BASE) else {
                    return false;
                };
                if i >= self.hash.len() || self.hash[i].rh() == 0 {
                    return false;
                }
                // another name that exists
                let other =
                    self.hash[usize::try_from(FROZEN_PROTECTION - HASH_BASE).unwrap_or(0)].rh();
                let alt = if self.hash[i].rh() == other {
                    self.hash[usize::try_from(NULL_CS + 1 - HASH_BASE).unwrap_or(0)].rh()
                } else {
                    other
                };
                if alt == 0 || alt == self.hash[i].rh() {
                    return false;
                }
                self.hash[i].set_rh(alt);
                true
            }
            Cell::Eqtb(p) if p >= EXT_BASE => {
                let (kind, _) = ext_reg(p);
                let mut w = self.xregs.get(p);
                let mut o = self.xregs.obj(p).cloned();
                if is_word_kind(kind) {
                    w.set_int(w.int() ^ 1);
                } else if kind == TOK_VAL && o.is_some() {
                    w.set_b0(UNDEFINED_CS);
                    o = None;
                } else if w.b0() == BOX_REF && o.is_some() {
                    o = None;
                } else if w.b0() == GLUE_REF {
                    o = Some(crate::objs::Obj::Glue(self.poison_glue(o.as_ref())));
                } else {
                    return false;
                }
                self.xregs.set(p, w);
                self.xregs.set_obj(p, o);
                true
            }
            Cell::Eqtb(p) => {
                let Some(i) = usize::try_from(p).ok().filter(|&i| i < self.eqtb.len()) else {
                    return false;
                };
                let mut w = self.eqtb[i];
                let mut o = self.eqtb_obj.get(i).cloned().flatten();
                if !self.poison_word(p, &mut w, &mut o) {
                    return false;
                }
                self.eqtb[i] = w;
                if let Some(x) = self.eqtb_obj.get_mut(i) {
                    *x = o;
                }
                true
            }
        }
    }

    /// A different value for `eqtb[p]`, if a cheap valid one exists.
    fn poison_word(
        &mut self,
        p: i32,
        w: &mut MemoryWord,
        o: &mut Option<crate::objs::Obj>,
    ) -> bool {
        if is_meaning(p) {
            // undefined, or `\relax` if it was undefined (the level kept)
            if w.b0() == UNDEFINED_CS {
                w.set_b0(RELAX);
                w.set_rh(256);
            } else {
                w.set_b0(UNDEFINED_CS);
                w.set_rh(NULL);
            }
            *o = None;
            return true;
        }
        if p >= INT_BASE {
            // integers and dimensions (the levels are in `xeq_level`)
            w.set_int(w.int() ^ 1);
            return true;
        }
        if (GLUE_BASE..LOCAL_BASE).contains(&p) {
            *o = Some(crate::objs::Obj::Glue(self.poison_glue(o.as_ref())));
            return true;
        }
        if p == PAR_SHAPE_LOC || (ETEX_PEN_BASE..ETEX_PENS).contains(&p) || w.b0() == SHAPE_REF {
            if o.is_none() {
                return false;
            }
            *o = None;
            return true;
        }
        if (LOCAL_BASE..CUR_FONT_LOC).contains(&p) {
            // token lists and boxes: emptied (an empty token list is
            // undefined, as `\toks0={}` leaves it)
            if o.is_none() {
                return false;
            }
            if w.b0() != BOX_REF {
                w.set_b0(UNDEFINED_CS);
            }
            *o = None;
            return true;
        }
        if p == CUR_FONT_LOC || (MATH_FONT_BASE..CAT_CODE_BASE).contains(&p) {
            if self.font_ptr < 1 {
                return false;
            }
            w.set_rh(i32::from(w.rh() == 0));
            return true;
        }
        if (XORD_CODE_BASE..MATH_FONT_BASE).contains(&p) {
            return false; // encTeX's tables
        }
        if (CAT_CODE_BASE..LC_CODE_BASE).contains(&p) {
            w.set_rh((w.rh() + 1) % 16);
            return true;
        }
        // the other codes: lc, uc, sf, math, char sub
        w.set_rh(w.rh() ^ 1);
        true
    }

    /// Glue other than `o`: zero glue, or 1sp if it was zero.
    fn poison_glue(&mut self, o: Option<&crate::objs::Obj>) -> crate::objs::Glue {
        if let Some(crate::objs::Obj::Glue(g)) = o
            && !g.spec.shared_zero
        {
            return crate::objs::Glue::ZERO;
        }
        let mut g = partex_engine::node::GlueSpec::ZERO_GLUE;
        g.shared_zero = false;
        g.width = 1;
        self.new_glue_value(g, None)
    }

    /// The word of a tracked cell and its level, raw (ids included): for
    /// telling which cells a run left alone.
    #[must_use]
    pub fn cell_raw(&self, c: Cell) -> (u64, i32) {
        let slot = |p: i32| {
            usize::try_from(p - HASH_BASE)
                .ok()
                .filter(|&i| i < self.hash.len())
                .map(|i| self.hash[i])
        };
        match c {
            Cell::Hash(p) => (slot(p).map_or(0, |w| u64::from(w.rh().cast_unsigned())), 0),
            Cell::HashNext(p) => (slot(p).map_or(0, |w| u64::from(w.lh().cast_unsigned())), 0),
            Cell::Eqtb(p) => {
                let level = if p >= INT_BASE && (p <= EQTB_SIZE || p >= EXT_BASE) {
                    self.peek_xeq_level(p)
                } else {
                    0
                };
                (self.peek_eqtb(p).bits(), level)
            }
            other => {
                let d = self.family_digest(other);
                (u64::try_from(d & u128::from(u64::MAX)).unwrap_or(0), 0)
            }
        }
    }

    /// The location of the current input, for reports: file name, line.
    #[must_use]
    pub fn input_location(&self) -> (Vec<u8>, i32) {
        let name = self
            .full_source_filename_stack
            .get(self.in_open)
            .copied()
            .filter(|&s| s > 0)
            .map(|s| self.str_bytes(usize::try_from(s).unwrap_or(0)).to_vec())
            .unwrap_or_default();
        (name, self.line)
    }
}

/// TeX's statistics about its internal tables, as lines of the log or the
/// terminal (§1334 at the end of a job, §639 under `\tracingstats`, the
/// `\dump` report, pdfTeX's extra memory): partex does not reproduce
/// them (`AGENTS.md`; the oracle comparisons mask them, `xtask/src/mask.rs`).
/// `line` is one line, with or without its newline, whose digit runs are
/// taken as any number.
#[must_use]
pub fn statistics_line(line: &[u8]) -> bool {
    const WHOLE: &[&[u8]] = &[
        b" # strings out of #",
        b" # string out of #",
        b" # string characters out of #",
        b" # words of memory out of #",
        b" # multiletter control sequences out of #+#",
        b" # words of font info for # fonts, out of # for #",
        b" # words of font info for # font, out of # for #",
        b" # hyphenation exceptions out of #",
        b" # hyphenation exception out of #",
        b" #i,#n,#p,#b,#s stack positions out of #i,#n,#p,#b,#s",
        b" # words of extra memory for PDF output out of # (max. #)",
        b"# strings of total length #",
        b"# multiletter control sequences",
        b"# words of font info for # preloaded fonts",
        b"# words of font info for # preloaded font",
        b"# hyphenation exceptions",
        b"# hyphenation exception",
        b"Hyphenation trie of length # has # ops out of #",
        b"Hyphenation trie of length # has # op out of #",
    ];
    const PART: &[&[u8]] = &[
        b"Memory usage before: #&#; after: #&#; still untouched: #",
        b"# memory locations dumped; current usage is #&#",
    ];
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let mut t: Vec<u8> = Vec::with_capacity(line.len());
    for &b in line {
        if b.is_ascii_digit() {
            if t.last() != Some(&b'#') {
                t.push(b'#');
            }
        } else {
            t.push(b);
        }
    }
    WHOLE.contains(&t.as_slice()) || PART.iter().any(|p| t.windows(p.len()).any(|w| w == *p))
}

/// `out` with the digits of every line [`statistics_line`] names left out
/// (what a comparison of two runs' output looks at).
#[must_use]
pub fn mask_statistics(out: &[u8]) -> Vec<u8> {
    let mut masked = Vec::with_capacity(out.len());
    for line in out.split_inclusive(|&b| b == b'\n') {
        if statistics_line(line) {
            masked.extend(line.iter().filter(|b| !b.is_ascii_digit()));
        } else {
            masked.extend_from_slice(line);
        }
    }
    masked
}

#[cfg(test)]
mod named_tests {
    use alloc::sync::Arc;

    use crate::testing::engine;
    use crate::track::Cell;
    use crate::web::BOX_VAL;
    use crate::xregs::reg_loc;

    /// A box register's box changed in place, under the same id and with
    /// the same eqtb word (`\wd`, §1247, before it reported its write):
    /// the shadow diff names the register; an unchanged box is not named.
    #[test]
    fn a_box_changed_in_place_is_a_changed_cell() {
        for n in [7, 300] {
            // (two engines made alike: the region's entry, and its exit)
            let loc = reg_loc(BOX_VAL, n);
            let made = || {
                let mut t = engine();
                t.set_box_reg(n, Some(Arc::new(partex_engine::node::BoxNode::default())));
                t
            };
            let (entry, mut t) = (made(), made());
            // (the words' part of the shadow diff sees nothing here; the
            // bare test engine has no fonts for the families' part)
            let named = |a: &crate::tex::Tex<_, _>, b: &crate::tex::Tex<_, _>| {
                let mut out = alloc::vec::Vec::new();
                a.named_differences(b, &mut out);
                out
            };
            assert!(!named(&entry, &t).contains(&Cell::Eqtb(loc)));
            // (the entry's value replaced past the accessors, the word
            // kept: what a change in place under the same word was)
            let changed = crate::objs::Obj::Box(Arc::new(partex_engine::node::BoxNode {
                width: 5 << 16,
                ..partex_engine::node::BoxNode::default()
            }));
            if loc >= crate::xregs::EXT_BASE {
                t.xregs.set_obj(loc, Some(changed));
            } else if let Some(x) = t.eqtb_obj.get_mut(usize::try_from(loc).unwrap_or(0)) {
                *x = Some(changed);
            }
            assert!(
                named(&entry, &t).contains(&Cell::Eqtb(loc)),
                "box register {n} changed in place is not a changed cell"
            );
        }
    }
}

#[cfg(test)]
mod statistics_tests {
    use super::{mask_statistics, statistics_line};

    #[test]
    fn statistics_lines_are_named_and_only_they() {
        for l in [
            &b" 29604 strings out of 469503\n"[..],
            b" 755478 string characters out of 5470311",
            b" 20674 words of memory out of 5000000",
            b" 29185 multiletter control sequences out of 15000+600000",
            b" 1560 words of font info for 45 fonts, out of 8000000 for 9000",
            b" 14 hyphenation exceptions out of 8191",
            b" 35i,5n,38p,147b,148s stack positions out of 10000i,1000n,20000p,200000b,200000s",
            b" 1 words of extra memory for PDF output out of 10000 (max. 10000000)",
            b"Memory usage before: 159&484; after: 102&328; still untouched: 1613",
        ] {
            assert!(
                statistics_line(l),
                "{}",
                core::str::from_utf8(l).unwrap_or("?")
            );
        }
        for l in [
            &b" 44 PDF objects out of 1000 (max. 8388607)"[..],
            b"Output written on modern.pdf (1 page, 12345 bytes).",
            b"3 out of 5 cells",
            b" 29604 strings out of 469503 and more",
        ] {
            assert!(
                !statistics_line(l),
                "{}",
                core::str::from_utf8(l).unwrap_or("?")
            );
        }
        let a = mask_statistics(b"x 1\n 453 strings out of 469503\ny 2\n");
        let b = mask_statistics(b"x 1\n 29604 strings out of 469503\ny 2\n");
        assert_eq!(a, b);
        assert_eq!(a, b"x 1\n  strings out of \ny 2\n");
        assert_ne!(mask_statistics(b"x 1\n"), mask_statistics(b"x 2\n"));
    }
}
