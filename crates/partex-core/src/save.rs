//! Part 19: Saving and restoring equivalents (§268–§288), and `show_eqtb`
//! (§252), which `restore_trace` uses.

use crate::host::Host;
use crate::mem::{NULL, Pointer};
use crate::objs::Obj;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use crate::xregs::{EXT_BASE, Saved, ext_reg, is_word_kind};
use partex_engine::node::Node;

/// The `equiv` of a macro's word: its list's `\protected` flag (e-TeX's
/// `protected_token` at the list's head, §1295), so that `print_cmd_chr`
/// and the expansion tests read it from the word; the list is the
/// entry's value (`objs.rs`). Other words keep `e`.
pub(crate) fn macro_flag(t: i32, e: i32, o: Option<&Obj>) -> i32 {
    if (crate::cmds::CALL..=crate::cmds::LONG_OUTER_CALL).contains(&t) {
        o.and_then(Obj::toks)
            .map_or(e, |l| i32::from(l.protected()))
    } else {
        e
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    fn sx(p: i32) -> usize {
        usize::try_from(p).expect("negative save stack index")
    }

    // The save stack as a value (DESIGN 7.17.12, `save_stack`): each entry
    // is a slot versioned by its content (its word, the eqtb value it
    // holds, whether it holds one), read by the accessors below and
    // written by the setters; `save_ptr`, `cur_level`, `cur_group` and
    // `cur_boundary` are scalar slots; e-TeX's chain of saved registers
    // above 255 is one more.
    #[inline]
    fn save_read(&self, p: i32) {
        if T::VALUES {
            let k = crate::track::save::ENTRY + u32::try_from(p).unwrap_or(0);
            self.tracker
                .value_read(crate::track::Row::Save(k), || self.save_entry_version(p));
        }
    }

    #[inline]
    fn save_wrote(&self, p: i32) {
        if T::VALUES {
            let k = crate::track::save::ENTRY + u32::try_from(p).unwrap_or(0);
            self.tracker.value_wrote(crate::track::Row::Save(k));
        }
    }

    #[inline]
    fn save_top_read(&self, k: u32) {
        if T::VALUES {
            self.tracker.value_read(crate::track::Row::Save(k), || {
                self.save_row_version(i64::from(k))
            });
        }
    }

    #[inline]
    fn save_top_wrote(&self, k: u32) {
        if T::VALUES {
            self.tracker.value_wrote(crate::track::Row::Save(k));
        }
    }

    /// Entry `p`'s version: its word, the value it holds, whether it holds
    /// one (made from the value; an object carries its own).
    fn save_entry_version(&self, p: i32) -> u128 {
        let i = Self::sx(p);
        let word = if i < self.save_stack.len() {
            self.save_stack[i].bits()
        } else {
            0
        };
        let obj = self
            .save_obj
            .get(i)
            .and_then(Option::as_ref)
            .map(Obj::version);
        let eqtb = self.save_eqtb.get(i).copied().unwrap_or(false);
        partex_ssa::Version::of(&(word, obj, eqtb)).0
    }

    /// The version of save stack slot `s` (`track::save`) now.
    pub(crate) fn save_row_version(&self, s: i64) -> u128 {
        use crate::track::save::{
            CUR_BOUNDARY, CUR_GROUP, CUR_LEVEL, ENTRY, SAVE_PTR, XCHAIN, XENTRY,
        };
        let int = crate::track::scalar_version_i32;
        match u32::try_from(s).unwrap_or(u32::MAX) {
            SAVE_PTR => int(self.save_ptr),
            CUR_LEVEL => int(self.cur_level),
            CUR_GROUP => int(self.cur_group),
            CUR_BOUNDARY => int(self.cur_boundary),
            XCHAIN => partex_ssa::Version::of(&(self.xregs.chain_level, self.xregs.chain_lens())).0,
            k if k >= XENTRY => {
                let i = usize::try_from(k - XENTRY).unwrap_or(usize::MAX);
                let one = |c: &Saved| {
                    (
                        c.loc,
                        c.word.bits(),
                        c.obj.as_ref().map(Obj::version),
                        c.level,
                    )
                };
                partex_ssa::Version::of(&self.xregs.chain_entry(i).map(one)).0
            }
            k if k >= ENTRY => self.save_entry_version(i32::try_from(k - ENTRY).unwrap_or(0)),
            _ => 0,
        }
    }

    /// §271: `save_ptr`, read.
    pub(crate) fn save_ptr(&self) -> i32 {
        self.save_top_read(crate::track::save::SAVE_PTR);
        self.save_ptr
    }
    pub(crate) fn set_save_ptr(&mut self, v: i32) {
        self.save_ptr = v;
        self.save_top_wrote(crate::track::save::SAVE_PTR);
    }
    /// §221: `cur_level`, read.
    pub(crate) fn cur_level(&self) -> i32 {
        self.save_top_read(crate::track::save::CUR_LEVEL);
        self.cur_level
    }
    pub(crate) fn set_cur_level(&mut self, v: i32) {
        self.cur_level = v;
        self.save_top_wrote(crate::track::save::CUR_LEVEL);
    }
    /// §271: `cur_group`, read.
    pub(crate) fn cur_group(&self) -> i32 {
        self.save_top_read(crate::track::save::CUR_GROUP);
        self.cur_group
    }
    pub(crate) fn set_cur_group(&mut self, v: i32) {
        self.cur_group = v;
        self.save_top_wrote(crate::track::save::CUR_GROUP);
    }
    /// §271: `cur_boundary`, read.
    pub(crate) fn cur_boundary(&self) -> i32 {
        self.save_top_read(crate::track::save::CUR_BOUNDARY);
        self.cur_boundary
    }
    pub(crate) fn set_cur_boundary(&mut self, v: i32) {
        self.cur_boundary = v;
        self.save_top_wrote(crate::track::save::CUR_BOUNDARY);
    }

    /// The shape of e-TeX's chains of saved registers above 255, read
    /// (and written).
    fn xchain_read(&self) {
        self.save_top_read(crate::track::save::XCHAIN);
    }
    fn xchain_wrote(&self) {
        self.save_top_wrote(crate::track::save::XCHAIN);
    }
    /// The chains' entry `i` (laid end to end), read (and written).
    fn xentry_slot(i: usize) -> u32 {
        crate::track::save::XENTRY.saturating_add(u32::try_from(i).unwrap_or(u32::MAX))
    }
    /// The chains' entry `i` as [`Tracker::save_entry`] names a copy:
    /// below zero, apart from the save stack's entries.
    fn xentry_at(i: usize) -> i32 {
        -1 - i32::try_from(i).unwrap_or(i32::MAX - 1)
    }
    fn xentry_read(&self, i: usize) {
        self.save_top_read(Self::xentry_slot(i));
    }
    fn xentry_wrote(&self, i: usize) {
        self.save_top_wrote(Self::xentry_slot(i));
    }

    /// §268: `save_type(p)`.
    pub(crate) fn save_type(&self, p: i32) -> i32 {
        self.save_read(p);
        self.save_stack[Self::sx(p)].b0()
    }
    fn set_save_type(&mut self, p: i32, v: i32) {
        self.mark_save(p, false);
        self.save_stack[Self::sx(p)].set_b0(v);
    }
    /// §268: `save_level(p)`.
    pub(crate) fn save_level(&self, p: i32) -> i32 {
        self.save_read(p);
        self.save_stack[Self::sx(p)].b1()
    }
    fn set_save_level(&mut self, p: i32, v: i32) {
        self.mark_save(p, false);
        self.save_stack[Self::sx(p)].set_b1(v);
    }
    /// §268: `save_index(p)`.
    pub(crate) fn save_index(&self, p: i32) -> i32 {
        self.save_read(p);
        self.save_stack[Self::sx(p)].rh()
    }
    fn set_save_index(&mut self, p: i32, v: i32) {
        self.mark_save(p, false);
        self.save_stack[Self::sx(p)].set_rh(v);
    }
    /// §274: `saved(k)` = `save_stack[save_ptr+k].int`.
    pub(crate) fn saved(&self, k: i32) -> i32 {
        let p = self.save_ptr() + k;
        self.save_read(p);
        self.save_stack[Self::sx(p)].int()
    }
    pub(crate) fn set_saved(&mut self, k: i32, v: i32) {
        let p = self.save_ptr() + k;
        self.mark_save(p, false);
        self.save_stack[Self::sx(p)].set_int(v);
    }

    /// Record whether save stack slot `p` holds a copy of an eqtb word
    /// (whose ids the state hash must follow) or a plain value; a write
    /// of the entry.
    fn mark_save(&mut self, p: i32, eqtb: bool) {
        self.save_wrote(p);
        let p = Self::sx(p);
        // (an entry made a plain value holds no object: one a dead entry
        // left there is not this entry's, and its version would carry it)
        if !eqtb && let Some(o) = self.save_obj.get_mut(p) {
            *o = None;
        }
        if self.save_eqtb.len() <= p {
            if !eqtb {
                return; // (unmarked slots are plain)
            }
            self.save_eqtb.resize(p + 1, false);
        }
        self.save_eqtb[p] = eqtb;
    }

    /// §273 (e-TeX keeps room for seven more entries)
    fn check_full_save_stack(&mut self) -> Result<(), Jump> {
        let room = match self.params.flavor {
            crate::params::Flavor::Tex => 6,
            crate::params::Flavor::PdfTex | crate::params::Flavor::XeTeX => 7,
        };
        // (the size tested apart from the statistic, which a dropped run
        // leaves at its deepest)
        if self.save_ptr() > self.max_save_stack || self.save_ptr() > self.params.save_size - room {
            self.max_save_stack = self.max_save_stack.max(self.save_ptr());
            if self.save_ptr() > self.params.save_size - room {
                return self.overflow(b"save size", self.params.save_size);
            }
        }
        Ok(())
    }

    /// §274: begin a new level of grouping.
    pub(crate) fn new_save_level(&mut self, c: i32) -> Result<(), Jump> {
        self.check_full_save_stack()?;
        if self.etex_ex() {
            // e-TeX: the line where the group began, below its boundary
            self.set_saved(0, self.line);
            self.set_save_ptr(self.save_ptr() + 1);
        }
        self.set_save_type(self.save_ptr(), LEVEL_BOUNDARY);
        self.set_save_level(self.save_ptr(), self.cur_group());
        self.set_save_index(self.save_ptr(), self.cur_boundary());
        if self.cur_level() == MAX_QUARTERWORD {
            // quit if `cur_level+1` is too big to be stored in eqtb
            return self.overflow(b"grouping levels", MAX_QUARTERWORD - MIN_QUARTERWORD);
        }
        self.set_cur_boundary(self.save_ptr());
        self.set_cur_group(c);
        if self.int_par(TRACING_GROUPS_CODE) > 0 {
            self.group_trace(false);
        }
        self.set_cur_level(self.cur_level() + 1);
        self.set_save_ptr(self.save_ptr() + 1);
        Ok(())
    }

    /// §275: get ready to forget a value: the object an entry held is
    /// dropped with it (the value's own drop is tex.web's reference count).
    #[allow(clippy::unused_self, reason = "tex.web's procedure, kept by name")]
    pub(crate) fn eq_destroy(&mut self, o: Option<Obj>) {
        drop(o);
    }

    /// The object save stack slot `p` holds beside its word.
    fn save_obj_at(&self, p: i32) -> Option<&Obj> {
        self.save_obj.get(Self::sx(p)).and_then(Option::as_ref)
    }

    /// Put object `o` in save stack slot `p`.
    fn set_save_obj(&mut self, p: i32, o: Option<Obj>) {
        self.save_wrote(p);
        let p = Self::sx(p);
        if self.save_obj.len() <= p {
            if o.is_none() {
                return;
            }
            self.save_obj.resize(p + 1, None);
        }
        self.save_obj[p] = o;
    }

    /// §276: save `eqtb[p]`.
    pub(crate) fn eq_save(&mut self, p: Pointer, l: i32) -> Result<(), Jump> {
        if p >= EXT_BASE {
            return self.save_ext(p, l);
        }
        self.check_full_save_stack()?;
        if T::SOFT_READS {
            self.tracker
                .saved(crate::track::Cell::Eqtb(p), self.cur_level());
            self.tracker.save_entry(
                crate::track::Cell::Eqtb(p),
                self.cur_level(),
                Some(self.save_ptr),
            );
        }
        if l == LEVEL_ZERO {
            self.set_save_type(self.save_ptr(), RESTORE_ZERO);
        } else {
            let (at, w) = (Self::sx(self.save_ptr()), self.peek_eqtb(p));
            self.save_stack[at] = w;
            let o = self.peek_obj(p).cloned();
            self.set_save_obj(self.save_ptr(), o);
            self.mark_save(self.save_ptr(), true);
            self.set_save_ptr(self.save_ptr() + 1);
            self.set_save_type(self.save_ptr(), RESTORE_OLD_VALUE);
        }
        self.set_save_level(self.save_ptr(), l);
        self.set_save_index(self.save_ptr(), p);
        self.set_save_ptr(self.save_ptr() + 1);
        Ok(())
    }

    /// e-TeX: save a register above 255 in the chain of the current
    /// level.
    fn save_ext(&mut self, p: Pointer, l: i32) -> Result<(), Jump> {
        self.xchain_read();
        self.xchain_wrote();
        if self.cur_level() != self.xregs.chain_level {
            self.check_full_save_stack()?;
            self.set_save_type(self.save_ptr(), RESTORE_SA);
            self.set_save_level(self.save_ptr(), self.xregs.chain_level);
            self.set_save_index(self.save_ptr(), NULL);
            self.set_save_ptr(self.save_ptr() + 1);
            let outer = core::mem::take(&mut self.xregs.chain);
            self.xregs.outer.push(outer);
            self.xregs.chain_level = self.cur_level();
        }
        let i = self.xregs.chain_base() + self.xregs.chain.len();
        if T::SOFT_READS {
            self.tracker
                .saved(crate::track::Cell::Eqtb(p), self.cur_level());
            self.tracker.save_entry(
                crate::track::Cell::Eqtb(p),
                self.cur_level(),
                Some(Self::xentry_at(i)),
            );
        }
        let word = self.peek_eqtb(p);
        let obj = self.peek_obj(p).cloned();
        self.xentry_wrote(i);
        self.xregs.chain.push(Saved {
            loc: p,
            word,
            obj,
            level: l,
        });
        Ok(())
    }

    /// e-TeX: restore the registers above 255 saved at the level that
    /// ends, newest first.
    fn restore_ext(&mut self) {
        self.xchain_read();
        self.xchain_wrote();
        // (each entry restored is read, and dropped: a write)
        if T::VALUES {
            let base = self.xregs.chain_base();
            for i in base..base + self.xregs.chain.len() {
                self.xentry_read(i);
                self.xentry_wrote(i);
            }
        }
        let base = self.xregs.chain_base();
        let chain = core::mem::take(&mut self.xregs.chain);
        let tracing = self.int_par(TRACING_RESTORES_CODE) > 0;
        for (k, s) in chain.into_iter().enumerate().rev() {
            let word = is_word_kind(ext_reg(s.loc).0);
            // (whether the current value is global decides)
            self.report_eqtb_read(s.loc);
            let level = if word {
                self.peek_xeq_level(s.loc)
            } else {
                self.peek_eqtb(s.loc).b1()
            };
            if level == LEVEL_ONE {
                if !word {
                    self.eq_destroy(s.obj);
                }
                if tracing {
                    self.restore_trace(s.loc, b"retaining");
                }
            } else {
                if word {
                    self.set_eqtb(s.loc, s.word);
                    self.set_xeq_level(s.loc, s.level);
                } else {
                    self.set_eqtb_entry(s.loc, s.word, s.obj);
                }
                self.memo.restored(crate::track::Cell::Eqtb(s.loc));
                if T::SOFT_READS {
                    self.tracker
                        .restored(crate::track::Cell::Eqtb(s.loc), self.cur_level() + 1);
                    self.tracker
                        .restore_entry(crate::track::Cell::Eqtb(s.loc), Self::xentry_at(base + k));
                }
                if tracing {
                    self.restore_trace(s.loc, b"restoring");
                }
            }
        }
        self.xregs.chain = self.xregs.outer.pop().unwrap_or_default();
    }

    /// e-TeX: `\tracingassigns` output for `eqtb[p]`.
    #[inline]
    fn assign_trace(&mut self, p: Pointer, s: &[u8]) {
        if self.int_par(TRACING_ASSIGNS_CODE) > 0 {
            self.restore_trace(p, s);
        }
    }

    /// §277: new data for eqtb.
    /// Are the entry at `p` and the value (`e`, `o`) the same value of
    /// type `t`? (e-TeX's pointer comparison: an object is the same if it
    /// is the very value, glue by its lineage.)
    fn same_equiv(&self, p: Pointer, t: i32, e: i32, o: Option<&Obj>) -> bool {
        if t == crate::cmds::GLUE_REF {
            // (no value is `zero_glue`, lineage 0: the pointers compared)
            let lineage = |o: Option<&Obj>| match o {
                Some(Obj::Glue(g)) => g.lineage,
                _ => 0,
            };
            return lineage(self.peek_obj(p)) == lineage(o);
        }
        match (self.peek_obj(p), o) {
            (None, None) => self.peek_eqtb(p).rh() == e || crate::equiv::holds_object(t),
            (Some(Obj::Toks(a)), Some(Obj::Toks(b))) => alloc::sync::Arc::ptr_eq(a, b),
            (Some(Obj::Glue(a)), Some(Obj::Glue(b))) => a.lineage == b.lineage,
            (Some(Obj::Shape(a)), Some(Obj::Shape(b))) => alloc::sync::Arc::ptr_eq(a, b),
            // (a box assigned is a new node in tex.web, `\copy`'s too:
            // never the one the register has, §1077; nor is any other pair)
            _ => false,
        }
    }

    /// Whether a local assignment depends on the value it replaces, and
    /// so reads it (DESIGN.md §7.1): inside a group (the old value may be
    /// saved, and the level decides whether), or when `\tracingassigns`
    /// shows e-TeX's reassignment check. At level one without tracing the
    /// outcome is the new value whatever the old one was, and the check is
    /// skipped: it could only keep an equal value (for an undefined
    /// control sequence never defined, at level zero instead of one, which
    /// nothing prints).
    fn assignment_reads(&self, p: Pointer) -> bool {
        let tracing = self.int_par(TRACING_ASSIGNS_CODE) > 0;
        let reads = self.cur_level() > LEVEL_ONE || tracing;
        if reads {
            if T::SOFT_READS
                && crate::ssa::SOFT_READS_ON.load(core::sync::atomic::Ordering::Relaxed)
                && !tracing
                && self.int_par(TRACING_RESTORES_CODE) <= 0
            {
                // (the old value is saved and restored, or kept: a read
                // only if the group outlives the region, `Tracker::soft_read`;
                // a trace would print it)
                self.tracker
                    .soft_read(crate::track::Cell::Eqtb(p), self.cur_level());
            } else {
                self.report_eqtb_read(p);
            }
        }
        reads
    }

    /// Whether `eqtb[p]`, assigned locally, holds the open step's entry
    /// value ([`Tracker::entry_value`]): saved whatever its level.
    fn holds_entry_value(&self, p: Pointer) -> bool {
        T::SOFT_READS
            && self.cur_level() > LEVEL_ONE
            && self.int_par(TRACING_ASSIGNS_CODE) <= 0
            && self.int_par(TRACING_RESTORES_CODE) <= 0
            && self
                .tracker
                .entry_value(crate::track::Cell::Eqtb(p), self.cur_level())
    }

    pub(crate) fn eq_define(&mut self, p: Pointer, t: i32, e: i32) -> Result<(), Jump> {
        debug_assert!(
            !crate::equiv::holds_object(t) || e == NULL,
            "an object by id"
        );
        self.eq_define_obj(p, t, e, None)
    }

    /// §277 with the object the entry holds (`objs.rs`).
    pub(crate) fn eq_define_obj(
        &mut self,
        p: Pointer,
        t: i32,
        e: i32,
        o: Option<Obj>,
    ) -> Result<(), Jump> {
        self.memo.wrote_local(self.cur_level());
        let reads = self.assignment_reads(p);
        // (a value the open step began with, assigned in a group it opened:
        // saved and replaced whatever it is, `Tracker::entry_value`)
        let entry = reads && self.holds_entry_value(p);
        // (pdfTeX's `\def` makes a new list, never the one already there)
        if reads
            && !entry
            && self.etex_ex()
            && !self.fresh_def
            && self.peek_eqtb(p).b0() == t
            && self.same_equiv(p, t, e, o.as_ref())
        {
            self.assign_trace(p, b"reassigning");
            // (the new copy goes, the value kept stays)
            self.eq_destroy(o);
            return Ok(());
        }
        self.assign_trace(p, b"changing");
        if self.peek_eqtb(p).b1() == self.cur_level() && !entry {
            // (the old value is dropped with the entry's write below)
        } else if self.cur_level() > LEVEL_ONE {
            self.eq_save(p, self.peek_eqtb(p).b1())?;
        }
        let mut w = self.peek_eqtb(p);
        w.set_b1(self.cur_level());
        w.set_b0(t);
        w.set_rh(macro_flag(t, e, o.as_ref()));
        self.set_eqtb_entry(p, w, o);
        self.assign_trace(p, b"into");
        Ok(())
    }

    /// §278
    pub(crate) fn eq_word_define(&mut self, p: Pointer, w: i32) -> Result<(), Jump> {
        self.memo.wrote_local(self.cur_level());
        if !self.assignment_reads(p) {
            // (at level one every word's level is one)
            self.set_eqtb_int(p, w);
            return Ok(());
        }
        let entry = self.holds_entry_value(p);
        if self.etex_ex() && !entry && self.peek_eqtb(p).int() == w {
            self.assign_trace(p, b"reassigning");
            return Ok(());
        }
        self.assign_trace(p, b"changing");
        if self.peek_xeq_level(p) != self.cur_level() || entry {
            self.eq_save(p, self.peek_xeq_level(p))?;
            self.set_xeq_level(p, self.cur_level());
        }
        self.set_eqtb_int(p, w);
        self.assign_trace(p, b"into");
        Ok(())
    }

    /// §279: global `eq_define`.
    pub(crate) fn geq_define(&mut self, p: Pointer, t: i32, e: i32) {
        debug_assert!(
            !crate::equiv::holds_object(t) || e == NULL,
            "an object by id"
        );
        self.geq_define_obj(p, t, e, None);
    }

    /// §279 with the object the entry holds.
    pub(crate) fn geq_define_obj(&mut self, p: Pointer, t: i32, e: i32, o: Option<Obj>) {
        self.memo.wrote_global(p);
        self.assign_trace(p, b"globally changing");
        let mut w = self.peek_eqtb(p);
        w.set_b1(LEVEL_ONE);
        w.set_b0(t);
        w.set_rh(macro_flag(t, e, o.as_ref()));
        self.set_eqtb_entry(p, w, o);
        self.assign_trace(p, b"into");
    }

    /// §279: global `eq_word_define`.
    pub(crate) fn geq_word_define(&mut self, p: Pointer, w: i32) {
        self.memo.wrote_global(p);
        self.assign_trace(p, b"globally changing");
        self.set_eqtb_int(p, w);
        self.set_xeq_level(p, LEVEL_ONE);
        self.assign_trace(p, b"into");
    }

    /// §280: save token `t` for insertion after the group.
    pub(crate) fn save_for_after(&mut self, t: i32) -> Result<(), Jump> {
        self.memo.after_group(self.cur_level());
        if self.cur_level() > LEVEL_ONE {
            self.check_full_save_stack()?;
            self.set_save_type(self.save_ptr(), INSERT_TOKEN);
            self.set_save_level(self.save_ptr(), LEVEL_ZERO);
            self.set_save_index(self.save_ptr(), t);
            self.set_save_ptr(self.save_ptr() + 1);
        }
        Ok(())
    }

    /// §281: pop the top level off the save stack. Not a call (DESIGN
    /// 4.3 item 2): a group's end is writes of the step that runs it, its
    /// restores ordinary writes through the accessors, and a value put
    /// back equal to what its address held keeps its version (the
    /// accessor versions it by content), so the readers after the group
    /// stay asleep. The `\aftergroup` tokens it puts in the input (§326)
    /// are the step's too: no record stands for a group's end, so no hit
    /// can lose them.
    pub(crate) fn unsave(&mut self) -> Result<(), Jump> {
        if self.cur_level() <= LEVEL_ONE {
            // `unsave` is not used when `cur_group=bottom_level`
            return self.confusion(b"curlevel");
        }
        let group = self.cur_group();
        // (the input level below the `\aftergroup` tokens, for a memo
        // recording; `back_input` may first end finished lists)
        let mut below_after = None;
        self.set_cur_level(self.cur_level() - 1);
        // §282: clear off top level from `save_stack`.
        let mut l = 0;
        // e-TeX: in extended mode, the tokens of `\aftergroup` after the
        // first join its backed-up list.
        let mut joined = false;
        loop {
            self.set_save_ptr(self.save_ptr() - 1);
            if self.save_type(self.save_ptr()) == LEVEL_BOUNDARY {
                break;
            }
            let p = self.save_index(self.save_ptr());
            if self.save_type(self.save_ptr()) == INSERT_TOKEN {
                // §326: insert token `p` into TeX's input.
                if joined {
                    self.insert_front(p);
                    if p < RIGHT_BRACE_LIMIT {
                        if p < LEFT_BRACE_LIMIT {
                            self.set_align_state(self.align_state() - 1);
                        } else {
                            self.set_align_state(self.align_state() + 1);
                        }
                    }
                } else {
                    let t = self.cur_tok;
                    self.cur_tok = p;
                    self.back_input()?;
                    self.cur_tok = t;
                    below_after.get_or_insert(self.input_ptr - 1);
                    joined = self.etex_ex();
                }
            } else if self.save_type(self.save_ptr()) == RESTORE_SA {
                self.restore_ext();
                self.xregs.chain_level = self.save_level(self.save_ptr());
                self.xchain_wrote();
            } else {
                if self.save_type(self.save_ptr()) == RESTORE_OLD_VALUE {
                    l = self.save_level(self.save_ptr());
                    self.set_save_ptr(self.save_ptr() - 1);
                } else {
                    let (at, w) = (
                        Self::sx(self.save_ptr()),
                        self.eqtb(UNDEFINED_CONTROL_SEQUENCE),
                    );
                    self.save_stack[at] = w;
                    self.set_save_obj(self.save_ptr(), None);
                    self.mark_save(self.save_ptr(), true);
                }
                // §283: store `save_stack[save_ptr]` in `eqtb[p]`, unless
                // `eqtb[p]` holds a global value.
                self.save_read(self.save_ptr());
                self.save_wrote(self.save_ptr());
                let saved = self.save_stack[Self::sx(self.save_ptr())];
                let at = Self::sx(self.save_ptr());
                let saved_obj = self.save_obj.get_mut(at).and_then(Option::take);
                // (\tracingrestores is looked at after the restoring)
                let tracing = |t: &Self| t.int_par(TRACING_RESTORES_CODE) > 0;
                // (whether the current value is global decides)
                self.report_eqtb_read(p);
                if p < INT_BASE || p > EQTB_SIZE {
                    if self.peek_eqtb(p).b1() == LEVEL_ONE {
                        self.eq_destroy(saved_obj); // destroy the saved value
                        if tracing(self) {
                            self.restore_trace(p, b"retaining");
                        }
                    } else {
                        // (the current value is dropped with the write)
                        self.set_eqtb_entry(p, saved, saved_obj); // restore the saved value
                        self.memo.restored(crate::track::Cell::Eqtb(p));
                        if T::SOFT_READS {
                            self.tracker
                                .restored(crate::track::Cell::Eqtb(p), self.cur_level() + 1);
                            self.tracker
                                .restore_entry(crate::track::Cell::Eqtb(p), self.save_ptr);
                        }
                        if tracing(self) {
                            self.restore_trace(p, b"restoring");
                        }
                    }
                } else if self.peek_xeq_level(p) != LEVEL_ONE {
                    self.set_eqtb(p, saved);
                    self.memo.restored(crate::track::Cell::Eqtb(p));
                    self.set_xeq_level(p, l);
                    if T::SOFT_READS {
                        self.tracker
                            .restored(crate::track::Cell::Eqtb(p), self.cur_level() + 1);
                        self.tracker
                            .restore_entry(crate::track::Cell::Eqtb(p), self.save_ptr);
                    }
                    if tracing(self) {
                        self.restore_trace(p, b"restoring");
                    }
                } else if tracing(self) {
                    self.restore_trace(p, b"retaining");
                }
            }
        }
        if T::SOFT_READS {
            self.tracker.group_end(self.cur_level() + 1);
        }
        if self.int_par(TRACING_GROUPS_CODE) > 0 {
            self.group_trace(true);
        }
        if self.grp_stack[self.in_open] == self.cur_boundary() {
            // groups possibly not properly nested with files
            self.group_warning();
        }
        self.set_cur_group(self.save_level(self.save_ptr()));
        self.set_cur_boundary(self.save_index(self.save_ptr()));
        if self.etex_ex() {
            self.set_save_ptr(self.save_ptr() - 1);
        }
        if self.memo.recording() {
            self.memo_group_closed(group, below_after.unwrap_or(self.input_ptr));
        }
        Ok(())
    }

    /// §284: `eqtb[p]` has just been restored or retained.
    fn restore_trace(&mut self, p: Pointer, s: &[u8]) {
        self.begin_diagnostic();
        self.print_char(b'{');
        self.print_str(s);
        self.print_char(b' ');
        self.show_eqtb(p);
        self.print_char(b'}');
        self.end_diagnostic(false);
    }

    /// §288: check `\mag`.
    pub(crate) fn prepare_mag(&mut self) -> Result<(), Jump> {
        let mag = self.int_par(MAG_CODE);
        if self.mag_set() > 0 && mag != self.mag_set() {
            self.print_err(b"Incompatible magnification (");
            self.print_int(mag);
            self.print_str(b");");
            self.print_nl(b" the previous value will be retained");
            self.help(&[
                b"I can handle only one magnification ratio per job. So I've",
                b"reverted to the magnification you used earlier on this run.",
            ]);
            self.int_error(self.mag_set())?;
            self.geq_word_define(INT_BASE + MAG_CODE, self.mag_set()); // `mag:=mag_set`
        }
        let mag = self.int_par(MAG_CODE);
        if mag <= 0 || mag > 32768 {
            self.print_err(b"Illegal magnification has been changed to 1000");
            self.help(&[b"The magnification ratio must be between 1 and 32768."]);
            self.int_error(mag)?;
            self.geq_word_define(INT_BASE + MAG_CODE, 1000);
        }
        self.set_mag_set(self.int_par(MAG_CODE));
        Ok(())
    }

    /// §252: display `eqtb[n]` symbolically.
    pub(crate) fn show_eqtb(&mut self, n: Pointer) {
        if n >= EXT_BASE {
            // e-TeX: a register above 255
            let (kind, r) = ext_reg(n);
            self.print_register_name(kind, r);
            self.print_char(b'=');
            match kind {
                INT_VAL => self.print_int(self.eqtb_int(n)),
                DIMEN_VAL => {
                    self.print_scaled(self.eqtb_int(n));
                    self.print_str(b"pt");
                }
                GLUE_VAL => self.print_spec(&self.glue_at(n), b"pt"),
                MU_VAL => self.print_spec(&self.glue_at(n), b"mu"),
                BOX_VAL => match self.peek_box(n) {
                    None => self.print_str(b"void"),
                    Some(b) => {
                        self.depth_threshold = 0;
                        self.breadth_max = 1;
                        self.show_node_list(&[Node::Box(b)]);
                    }
                },
                _ => {
                    if let Some(t) = self.equiv_toks(n).cloned() {
                        self.show_token_list(&t, NULL, 32);
                    }
                }
            }
        } else if n < ACTIVE_BASE {
            self.print_char(b'?'); // this can't happen
        } else if n < GLUE_BASE || (n > EQTB_SIZE && n <= self.eqtb_top) {
            // §223: region 1 or 2.
            self.sprint_cs(n);
            self.print_char(b'=');
            self.print_cmd_chr(self.eq_type(n), self.equiv(n));
            if self.eq_type(n) >= CALL {
                self.print_char(b':');
                if let Some(t) = self.equiv_toks(n).cloned() {
                    self.show_token_list(&t, NULL, 32);
                }
            }
        } else if n < LOCAL_BASE {
            // §229: region 3.
            if n < SKIP_BASE {
                self.print_skip_param(n - GLUE_BASE);
                self.print_char(b'=');
                if n < GLUE_BASE + THIN_MU_SKIP_CODE {
                    self.print_spec(&self.glue_at(n), b"pt");
                } else {
                    self.print_spec(&self.glue_at(n), b"mu");
                }
            } else if n < MU_SKIP_BASE {
                self.print_esc(b"skip");
                self.print_int(n - SKIP_BASE);
                self.print_char(b'=');
                self.print_spec(&self.glue_at(n), b"pt");
            } else {
                self.print_esc(b"muskip");
                self.print_int(n - MU_SKIP_BASE);
                self.print_char(b'=');
                self.print_spec(&self.glue_at(n), b"mu");
            }
        } else if n < INT_BASE {
            self.show_eqtb_region4(n);
        } else if n < DIMEN_BASE {
            // §242: region 5.
            if n < COUNT_BASE {
                self.print_param(n - INT_BASE);
            } else if n < DEL_CODE_BASE {
                self.print_esc(b"count");
                self.print_int(n - COUNT_BASE);
            } else {
                self.print_esc(b"delcode");
                self.print_int(n - DEL_CODE_BASE);
            }
            self.print_char(b'=');
            self.print_int(self.eqtb_int(n));
        } else if n <= EQTB_SIZE {
            // §251: region 6.
            if n < SCALED_BASE {
                self.print_length_param(n - DIMEN_BASE);
            } else {
                self.print_esc(b"dimen");
                self.print_int(n - SCALED_BASE);
            }
            self.print_char(b'=');
            self.print_scaled(self.eqtb_int(n));
            self.print_str(b"pt");
        } else {
            self.print_char(b'?'); // this can't happen either
        }
    }

    /// pdfTeX §251: show equivalent `n`, in region 4.
    fn show_eqtb_region4(&mut self, n: Pointer) {
        if n == PAR_SHAPE_LOC || (ETEX_PEN_BASE..ETEX_PENS).contains(&n) {
            self.print_cmd_chr(SET_SHAPE, n);
            self.print_char(b'=');
            match self
                .eqtb_obj(n)
                .and_then(|o| match o {
                    Obj::Shape(s) => Some(s.clone()),
                    _ => None,
                })
                .as_deref()
            {
                Some(crate::objs::Shaped::Lines(s)) => {
                    let lines = i32::try_from(s.len()).unwrap_or(i32::MAX);
                    self.print_int(lines);
                }
                Some(crate::objs::Shaped::Penalties(p)) => {
                    let (n, first) = (p.len(), p[0]);
                    self.print_int(i32::try_from(n).unwrap_or(i32::MAX));
                    self.print_char(b' ');
                    self.print_int(first);
                    if n > 1 {
                        self.print_esc(b"ETC.");
                    }
                }
                None => self.print_char(b'0'),
            }
        } else if n < TOKS_BASE {
            self.print_cmd_chr(ASSIGN_TOKS, n);
            self.print_char(b'=');
            if let Some(t) = self.equiv_toks(n).cloned() {
                self.show_token_list(&t, NULL, 32);
            }
        } else if n < BOX_BASE {
            self.print_esc(b"toks");
            self.print_int(n - TOKS_BASE);
            self.print_char(b'=');
            if let Some(t) = self.equiv_toks(n).cloned() {
                self.show_token_list(&t, NULL, 32);
            }
        } else if n < CUR_FONT_LOC {
            self.print_esc(b"box");
            self.print_int(n - BOX_BASE);
            self.print_char(b'=');
            match self.peek_box(n) {
                None => self.print_str(b"void"),
                Some(b) => {
                    self.depth_threshold = 0;
                    self.breadth_max = 1;
                    self.show_node_list(&[Node::Box(b)]);
                }
            }
        } else if n < CAT_CODE_BASE {
            // §234: show the font identifier in `eqtb[n]`.
            if n == CUR_FONT_LOC {
                self.print_str(b"current font");
            } else if n < MATH_FONT_BASE + SCRIPT_SIZE {
                self.print_esc(b"textfont");
                self.print_int(n - MATH_FONT_BASE);
            } else if n < MATH_FONT_BASE + SCRIPT_SCRIPT_SIZE {
                self.print_esc(b"scriptfont");
                self.print_int(n - MATH_FONT_BASE - SCRIPT_SIZE);
            } else {
                self.print_esc(b"scriptscriptfont");
                self.print_int(n - MATH_FONT_BASE - SCRIPT_SCRIPT_SIZE);
            }
            self.print_char(b'=');
            self.print_font_id(self.equiv(n));
        } else if n < MATH_CODE_BASE {
            // §235: show the halfword code in `eqtb[n]`.
            if n < LC_CODE_BASE {
                self.print_esc(b"catcode");
                self.print_int(n - CAT_CODE_BASE);
            } else if n < UC_CODE_BASE {
                self.print_esc(b"lccode");
                self.print_int(n - LC_CODE_BASE);
            } else if n < SF_CODE_BASE {
                self.print_esc(b"uccode");
                self.print_int(n - UC_CODE_BASE);
            } else {
                self.print_esc(b"sfcode");
                self.print_int(n - SF_CODE_BASE);
            }
            self.print_char(b'=');
            self.print_int(self.equiv(n));
        } else {
            self.print_esc(b"mathcode");
            self.print_int(n - MATH_CODE_BASE);
            self.print_char(b'=');
            self.print_int(self.equiv(n));
        }
    }
}
