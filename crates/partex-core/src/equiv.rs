//! Part 17: The table of equivalents — accessors and procedures
//! (constants are in `eqtb.rs`).

use crate::cmds::{
    BOX_REF, CAR_RET, COMMENT, DATA, ESCAPE, GLUE_REF, IGNORE, INVALID_CHAR, LETTER, OTHER_CHAR,
    SHAPE_REF, SPACER, UNDEFINED_CS,
};
use crate::eqtb::*;
use crate::error::{SPOTLESS, WARNING_ISSUED};
use crate::host::Host;
use crate::mem::{MemoryWord, NULL};
use crate::objs::Shape;
use crate::print::TERM_AND_LOG;
use crate::tex::{Jump, Tex};
use crate::track::{Cell, Tracker};
use crate::web::{BOX_VAL, DIMEN_VAL, GLUE_VAL, INT_VAL, MU_VAL, TOK_VAL};
#[allow(unused_imports)]
pub use crate::web::{CARRIAGE_RETURN, NULL_CODE, NULL_FONT, VAR_CODE};
use crate::wide::code_loc;
use crate::xregs::{EXT_BASE, reg_loc};
use alloc::sync::Arc;
use partex_engine::node::{BoxNode, GlueSpec, Tokens};

use crate::objs::{Glue, Obj, Shaped};

/// Whether `eq_type` `t` names an object its entry holds (`objs.rs`).
#[must_use]
pub(crate) fn holds_object(t: i32) -> bool {
    matches!(
        t,
        crate::cmds::CALL
            ..=crate::cmds::LONG_OUTER_CALL
                | crate::cmds::GLUE_REF
                | crate::cmds::SHAPE_REF
                | crate::cmds::BOX_REF
    )
}

impl<H: Host, T: Tracker> Tex<H, T> {
    #[inline]
    #[allow(
        clippy::cast_sign_loss,
        reason = "measured: the checked conversion showed in profiles"
    )]
    fn eqtb_idx(p: i32) -> usize {
        // (`p` is never negative: a sign-extended index would fail the
        // bounds check anyway)
        p as usize
    }

    /// The object `eqtb[p]` holds beside its word, if any (`objs.rs`); its
    /// read is the entry's.
    #[inline]
    pub(crate) fn eqtb_obj(&self, p: i32) -> Option<&Obj> {
        let _ = self.eqtb(p);
        self.peek_obj(p)
    }

    /// The object `eqtb[p]` holds, without telling the tracker.
    #[inline]
    pub(crate) fn peek_obj(&self, p: i32) -> Option<&Obj> {
        if p >= EXT_BASE {
            return self.xregs.obj(p);
        }
        self.eqtb_obj
            .get(Self::eqtb_idx(p))
            .and_then(Option::as_ref)
    }

    /// The box `eqtb[p]` holds, without telling the tracker (a display of
    /// the entry).
    pub(crate) fn peek_box(&self, p: i32) -> Option<Arc<BoxNode>> {
        match self.peek_obj(p) {
            Some(Obj::Box(b)) => Some(b.clone()),
            _ => None,
        }
    }

    /// The token list `eqtb[p]` holds (a macro's body, a token register or
    /// parameter), if any.
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "every macro call")]
    pub(crate) fn equiv_toks(&self, p: i32) -> Option<&Tokens> {
        self.eqtb_obj(p).and_then(Obj::toks)
    }

    /// Put word `w` and object `o` in `eqtb[p]`: one write of the entry.
    pub(crate) fn set_eqtb_entry(&mut self, p: i32, w: MemoryWord, o: Option<Obj>) {
        if p >= EXT_BASE {
            self.xregs.set_obj(p, o);
        } else {
            let i = Self::eqtb_idx(p);
            if i < self.eqtb_obj.len() {
                self.eqtb_obj[i] = o;
            }
        }
        self.set_eqtb(p, w);
    }

    /// The whole word `eqtb[p]`.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "measured: 4% of the PGF subset left inline"
    )]
    pub(crate) fn eqtb(&self, p: i32) -> MemoryWord {
        self.tracker.read(Cell::Eqtb(p));
        let w = if p >= EXT_BASE {
            self.xregs.get(p)
        } else {
            self.eqtb[Self::eqtb_idx(p)]
        };
        if T::VALUES {
            self.tracker.read_value(Cell::Eqtb(p), w.bits());
            self.tracker
                .read_content(Cell::Eqtb(p), || self.eqtb_content_by_tokens(p));
        }
        if self.memo.recording() {
            self.memo_read_eqtb(p, w);
        }
        w
    }

    /// Report a read of `eqtb[p]` that goes past the accessors (the
    /// save stack's bookkeeping, `save.rs`), with its content for a
    /// tracker that keeps versions.
    pub(crate) fn report_eqtb_read(&self, p: i32) {
        self.tracker.read(Cell::Eqtb(p));
        if T::VALUES {
            self.tracker
                .read_content(Cell::Eqtb(p), || self.eqtb_content_by_tokens(p));
        }
    }

    /// A read of `eqtb[p]` (value `w`) while a call is being recorded.
    #[cold]
    #[inline(never)]
    fn memo_read_eqtb(&self, p: i32, w: MemoryWord) {
        if self.memo.first_read(Cell::Eqtb(p)) {
            self.memo
                .push_read(Cell::Eqtb(p), w.bits(), self.list_generation(p, w));
        }
    }

    /// For a word that names a token list (a macro's meaning, a token
    /// parameter or register): the list's generation plus one, so that a
    /// recorded read can tell a reused id from the list it read (0 for
    /// other words).
    pub(crate) fn list_generation(&self, p: i32, _w: MemoryWord) -> u32 {
        // (the list by its content: the memo tells a changed list from the
        // one it read; a plain run keeps no versions)
        match self.peek_obj(p) {
            Some(Obj::Toks(t)) => {
                #[allow(clippy::cast_possible_truncation, reason = "a 32-bit tag")]
                let h = partex_engine::stablehash::StableHasher::of(&(t.protected(), t.tokens()))
                    as u32;
                h | 1
            }
            _ => 0,
        }
    }
    /// `eqtb[p]` without telling the tracker: for the save-stack
    /// bookkeeping of assignments, which report the read themselves when
    /// the value matters (`save.rs`: inside a group, under
    /// `\tracingassigns`, at a group's end), and for freeing the value an
    /// assignment replaces (DESIGN.md §7.1).
    #[inline]
    pub(crate) fn peek_eqtb(&self, p: i32) -> MemoryWord {
        if p >= EXT_BASE {
            return self.xregs.get(p);
        }
        self.eqtb[Self::eqtb_idx(p)]
    }
    /// `xeq_level[p]` without telling the tracker (see [`Self::peek_eqtb`]).
    pub(crate) fn peek_xeq_level(&self, p: i32) -> i32 {
        if p >= EXT_BASE {
            return self.xregs.level(p);
        }
        self.xeq_level[usize::try_from(p - INT_BASE).expect("xeq_level index")]
    }
    #[inline]
    pub(crate) fn set_eqtb(&mut self, p: i32, w: MemoryWord) {
        self.tracker.write(Cell::Eqtb(p));
        self.memo.wrote(Cell::Eqtb(p));
        if T::VALUES {
            let old = if p >= EXT_BASE {
                self.xregs.get(p)
            } else {
                self.eqtb[Self::eqtb_idx(p)]
            };
            self.tracker
                .write_value(Cell::Eqtb(p), old.bits(), w.bits());
        }
        let old = self.peek_eqtb(p);
        self.note_meaning(p, old, w);
        if T::CLASSES {
            self.note_class(p, old, w);
        }
        if p >= EXT_BASE {
            self.xregs.set(p, w);
        } else {
            self.eqtb[Self::eqtb_idx(p)] = w;
        }
        if T::VALUES {
            self.wrote_eqtb(p);
        }
    }

    /// Every table slot's version (and every token list's), for a tracker
    /// that keeps the tables'
    /// version arrays: made by a writer that stores the tables wholesale,
    /// past the accessors (the engine's initial tables, a format's load),
    /// as the accessors make one at each write (DESIGN 7.17.12). The
    /// registers above 255 are versioned at their default too.
    pub(crate) fn version_tables(&mut self) {
        if !T::VALUES {
            return;
        }
        // (the objects entries hold carry the versions they were made
        // with: a format's are made at its load)
        // (a control sequence's word that names no object has the content
        // of its bits: most are undefined, so each distinct word is hashed
        // once)
        let mut by_bits: alloc::collections::BTreeMap<u64, u128> =
            alloc::collections::BTreeMap::new();
        let mut last: Option<(u64, u128)> = None;
        for p in (1..).take(self.eqtb.len().saturating_sub(1)) {
            let w = self.peek_eqtb(p);
            let cs = p < GLUE_BASE || (p > EQTB_SIZE && p < EXT_BASE);
            let names = matches!(
                w.b0(),
                crate::cmds::CALL
                    ..=crate::cmds::LONG_OUTER_CALL
                        | crate::cmds::END_TEMPLATE
                        | crate::cmds::GLUE_REF
                        | crate::cmds::SHAPE_REF
                        | crate::cmds::BOX_REF
            );
            if cs && !names {
                // (runs of equal words, most of them undefined, look up once)
                let v = match last {
                    Some((bits, v)) if bits == w.bits() => v,
                    _ => {
                        let v = *by_bits
                            .entry(w.bits())
                            .or_insert_with(|| self.cell_content(Cell::Eqtb(p)));
                        last = Some((w.bits(), v));
                        v
                    }
                };
                self.tracker.wrote(Cell::Eqtb(p), v);
            } else {
                self.wrote_eqtb(p);
            }
        }
        if self.max_reg_num > 255 {
            for kind in [INT_VAL, DIMEN_VAL, GLUE_VAL, MU_VAL, BOX_VAL, TOK_VAL] {
                for n in 256..=self.max_reg_num {
                    self.wrote_eqtb(crate::xregs::reg_loc(kind, n));
                }
            }
        }
        // (the pool's strings by number, and the allocators)
        for n in 0..self.str_ptr {
            self.tracker
                .row_made(crate::track::Row::Str(n), self.string_version(n));
        }

        // (the scalar rows)
        self.version_scalars();
        // (the fonts' fields and their table, and hyphenation)
        self.version_fonts();
        self.version_hyph();
        // (the DVI and PDF writers' tables)
        self.version_writers();
        if T::NAMES {
            // (an empty slot's name and link are the same everywhere)
            let (empty, end) = (
                self.name_content(Cell::Hash(crate::web::HASH_BASE - 1)),
                self.name_content(Cell::HashNext(crate::web::HASH_BASE - 1)),
            );
            for (p, w) in (crate::web::HASH_BASE..).zip(self.hash.slices().flatten()) {
                let text = if w.rh() == 0 {
                    empty
                } else {
                    self.name_content(Cell::Hash(p))
                };
                let next = if w.lh() == 0 {
                    end
                } else {
                    self.name_content(Cell::HashNext(p))
                };
                self.tracker.wrote(Cell::Hash(p), text);
                self.tracker.wrote(Cell::HashNext(p), next);
            }
        }
    }

    /// The version of `eqtb[p]` made at its write, from the content
    /// written (the word, its level, the objects it names), for a tracker
    /// that keeps the table's version array (DESIGN 7.17.12).
    #[cold]
    #[inline(never)]
    pub(crate) fn wrote_eqtb(&self, p: i32) {
        self.tracker
            .wrote(Cell::Eqtb(p), self.cell_content(Cell::Eqtb(p)));
    }

    /// A control sequence changed what it is to a skipped conditional
    /// text: the skips remembered are stale (`skipcache.rs`).
    #[inline]
    pub(crate) fn note_meaning(&mut self, p: i32, old: MemoryWord, new: MemoryWord) {
        let cs = p < GLUE_BASE || (p > EQTB_SIZE && p < EXT_BASE);
        if !cs {
            return;
        }
        let (a, b) = (
            crate::skipcache::class(old.b0(), old.rh()),
            crate::skipcache::class(new.b0(), new.rh()),
        );
        if a != b {
            self.skip.epoch += 1;
            if self.skip_tracked {
                self.class_hash ^= class_mix(p, a) ^ class_mix(p, b);
                self.classes_written = true;
            }
        }
    }

    /// A write of control sequence `p`'s meaning that changes its class
    /// ([`Tracker::class_wrote`]).
    #[inline]
    fn note_class(&self, p: i32, old: MemoryWord, new: MemoryWord) {
        let cs = p < GLUE_BASE || (p > EQTB_SIZE && p < EXT_BASE);
        if cs
            && crate::skipcache::token_class(old.b0(), old.rh())
                != crate::skipcache::token_class(new.b0(), new.rh())
        {
            self.tracker.class_wrote(p);
        }
    }

    /// The class of control sequence `p`'s meaning
    /// (`skipcache::token_class`), not told to the tracker.
    pub(crate) fn token_class_of(&self, p: i32) -> u8 {
        let w = self.peek_eqtb(p);
        crate::skipcache::token_class(w.b0(), w.rh())
    }

    /// Control sequence `p`'s meaning, looked up where only its token is
    /// wanted ([`Tex::token_only`]): a meaning of class 0 is read as its
    /// class ([`Tracker::read_class`]), any other as itself. (A call's
    /// memo, `memo.rs`, reads the meaning still.)
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "a test of a const, on the token path")]
    pub(crate) fn token_meaning(&self, p: i32) -> MemoryWord {
        if T::CLASSES && crate::ssa::CLASS_READS.load(core::sync::atomic::Ordering::Relaxed) {
            let w = self.peek_eqtb(p);
            if crate::skipcache::token_class(w.b0(), w.rh()) == 0 {
                self.tracker.read_class(p);
                // (a call's memo keeps the whole meaning)
                if self.memo.recording() {
                    self.memo_read_eqtb(p, w);
                }
                return w;
            }
        }
        self.eqtb(p)
    }

    /// Control sequence `p`'s meaning as `get_next` looks it up: as
    /// [`Tex::token_meaning`] while [`Tex::token_only`].
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "a test of a const, on the token path")]
    pub(crate) fn lookup_meaning(&self, p: i32) -> MemoryWord {
        if T::CLASSES && self.token_only {
            self.token_meaning(p)
        } else {
            self.eqtb(p)
        }
    }

    /// Run `f` with [`Tex::token_only`] set: its lookups want only the
    /// tokens.
    pub(crate) fn tokens_only<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<R, Jump>,
    ) -> Result<R, Jump> {
        let was = core::mem::replace(&mut self.token_only, true);
        let r = f(self);
        self.token_only = was;
        r
    }

    /// Remember skips with a machine's tracker (`class_hash` from
    /// scratch: every control sequence's class to a skip).
    pub fn set_skip_tracked(&mut self, on: bool) {
        self.skip_tracked = on;
        let mut h = 0;
        if on {
            for (p, w) in (0..).zip(self.eqtb.slices().flatten()) {
                if p < GLUE_BASE || (p > EQTB_SIZE && p < EXT_BASE) {
                    h ^= class_mix(p, crate::skipcache::class(w.b0(), w.rh()));
                }
            }
        }
        self.class_hash = h;
    }
    /// Change one field of `eqtb[p]`.
    #[inline]
    fn modify_eqtb(&mut self, p: i32, f: impl FnOnce(&mut MemoryWord)) {
        self.tracker.write(Cell::Eqtb(p));
        self.memo.wrote(Cell::Eqtb(p));
        let (old, new) = if p >= EXT_BASE {
            let mut w = self.xregs.get(p);
            let old = w;
            f(&mut w);
            self.xregs.set(p, w);
            (old, w)
        } else {
            let x = &mut self.eqtb[Self::eqtb_idx(p)];
            let old = *x;
            f(x);
            (old, *x)
        };
        self.note_meaning(p, old, new);
        if T::CLASSES {
            self.note_class(p, old, new);
        }
        if T::VALUES {
            self.tracker
                .write_value(Cell::Eqtb(p), old.bits(), new.bits());
            self.wrote_eqtb(p);
        }
    }

    /// Eqtb location `p` changed without its word changing: what the word
    /// names was changed in place (§1247 sets a box register's box's
    /// dimensions). A write of the location, as the tracker and the
    /// memos see it (its content is what changed).
    pub(crate) fn note_changed_in_place(&mut self, p: i32) {
        self.modify_eqtb(p, |_| {});
    }

    /// §221: `eq_level`, `eq_type`, `equiv`.
    #[inline]
    pub(crate) fn eq_level(&self, p: i32) -> i32 {
        self.eqtb(p).b1()
    }
    #[inline]
    pub(crate) fn set_eq_level(&mut self, p: i32, v: i32) {
        self.modify_eqtb(p, |w| w.set_b1(v));
    }
    #[inline]
    pub(crate) fn eq_type(&self, p: i32) -> i32 {
        self.eqtb(p).b0()
    }
    #[inline]
    pub(crate) fn set_eq_type(&mut self, p: i32, v: i32) {
        self.modify_eqtb(p, |w| w.set_b0(v));
    }
    #[inline]
    pub(crate) fn equiv(&self, p: i32) -> i32 {
        self.eqtb(p).rh()
    }
    #[inline]
    pub(crate) fn set_equiv(&mut self, p: i32, v: i32) {
        self.modify_eqtb(p, |w| w.set_rh(v));
    }
    /// `eqtb[p].int` / `.sc` (regions 5 and 6).
    #[inline]
    pub(crate) fn eqtb_int(&self, p: i32) -> i32 {
        self.eqtb(p).int()
    }
    #[inline]
    pub(crate) fn set_eqtb_int(&mut self, p: i32, v: i32) {
        self.modify_eqtb(p, |w| w.set_int(v));
    }

    /// §253: `xeq_level[p]` for `int_base <= p <= eqtb_size`.
    pub(crate) fn xeq_level(&self, p: i32) -> i32 {
        self.tracker.read(Cell::Eqtb(p));
        if T::VALUES {
            self.tracker
                .read_content(Cell::Eqtb(p), || self.eqtb_content_by_tokens(p));
        }
        if p >= EXT_BASE {
            return self.xregs.level(p);
        }
        self.xeq_level[usize::try_from(p - INT_BASE).expect("xeq_level index")]
    }
    pub(crate) fn set_xeq_level(&mut self, p: i32, v: i32) {
        self.tracker.write(Cell::Eqtb(p));
        if p >= EXT_BASE {
            self.xregs.set_level(p, v);
        } else {
            self.xeq_level[usize::try_from(p - INT_BASE).expect("xeq_level index")] = v;
        }
        if T::VALUES {
            // (`xeq_level` is part of the entry's value, §253)
            self.wrote_eqtb(p);
        }
    }

    // §224, §230, §236, §247: named locations.
    /// The glue specification at eqtb location `p` (a `glue_ref`).
    pub(crate) fn glue_at(&self, p: i32) -> GlueSpec {
        self.glue_value(p).spec
    }

    /// The glue value (the specification and its lineage) at `p`.
    pub(crate) fn glue_value(&self, p: i32) -> Glue {
        match self.eqtb_obj(p) {
            Some(Obj::Glue(g)) => *g,
            _ => Glue::ZERO,
        }
    }

    /// New glue of `spec`: a new lineage, or `zero_glue`'s, or `lineage`
    /// (an unchanged copy of glue of that lineage).
    pub(crate) fn new_glue_value(&mut self, spec: GlueSpec, lineage: Option<u64>) -> Glue {
        if spec.shared_zero {
            return Glue::ZERO;
        }
        let lineage = lineage.unwrap_or_else(|| {
            self.glue_lineage += 1;
            self.scalar_wrote(
                crate::track::Row::Scalar(crate::track::scalar::GLUE_LINEAGE),
                i32::try_from(self.glue_lineage).unwrap_or(i32::MAX),
            );
            self.glue_lineage
        });
        Glue { spec, lineage }
    }
    pub(crate) fn glue_par(&self, n: i32) -> GlueSpec {
        self.glue_at(GLUE_BASE + n)
    }
    pub(crate) fn skip(&self, n: i32) -> GlueSpec {
        self.glue_at(reg_loc(GLUE_VAL, n))
    }
    pub(crate) fn mu_skip(&self, n: i32) -> GlueSpec {
        self.glue_at(reg_loc(MU_VAL, n))
    }
    pub(crate) fn toks(&self, n: i32) -> Option<&Tokens> {
        self.equiv_toks(reg_loc(TOK_VAL, n))
    }
    /// `\box n`, if not void.
    pub(crate) fn box_reg(&self, n: i32) -> Option<&Arc<BoxNode>> {
        match self.eqtb_obj(reg_loc(BOX_VAL, n)) {
            Some(Obj::Box(b)) => Some(b),
            _ => None,
        }
    }
    pub(crate) fn cur_font(&self) -> i32 {
        self.equiv(CUR_FONT_LOC)
    }
    pub(crate) fn fam_fnt(&self, n: i32) -> i32 {
        self.equiv(MATH_FONT_BASE + n)
    }
    pub(crate) fn cat_code(&self, c: i32) -> i32 {
        self.equiv(code_loc(CAT_CODE_BASE, c))
    }
    /// Code `c` of the table at `base` (`cat_code_base`, ...), without
    /// telling the tracker; a character past the narrow tables has
    /// `IniTeX`'s value (12 for its category).
    pub(crate) fn peek_code(&self, base: i32, c: i32) -> i32 {
        self.peek_eqtb(code_loc(base, c)).rh()
    }
    pub(crate) fn lc_code(&self, c: i32) -> i32 {
        self.equiv(code_loc(LC_CODE_BASE, c))
    }
    pub(crate) fn uc_code(&self, c: i32) -> i32 {
        self.equiv(code_loc(UC_CODE_BASE, c))
    }
    pub(crate) fn sf_code(&self, c: i32) -> i32 {
        self.equiv(code_loc(SF_CODE_BASE, c))
    }
    pub(crate) fn math_code(&self, c: i32) -> i32 {
        self.equiv(code_loc(MATH_CODE_BASE, c))
    }
    /// `box(n):=null` (at the same level), returning the box.
    pub(crate) fn take_box(&mut self, n: i32) -> Option<Arc<BoxNode>> {
        let loc = reg_loc(BOX_VAL, n);
        let b = match self.eqtb_obj(loc) {
            Some(Obj::Box(b)) => Some(b.clone()),
            _ => None,
        };
        let w = self.peek_eqtb(loc);
        self.set_eqtb_entry(loc, w, None);
        b
    }

    /// `box(n):=b`: tex.web's direct assignment (no save stack entry,
    /// the level stays).
    pub(crate) fn set_box_reg(&mut self, n: i32, mut b: Option<Arc<BoxNode>>) {
        if let Some(b) = &mut b {
            self.sync_arc(b);
        }
        let loc = reg_loc(BOX_VAL, n);
        let w = self.peek_eqtb(loc);
        self.set_eqtb_entry(loc, w, b.map(Obj::Box));
    }

    /// `\parshape`, if set.
    pub(crate) fn par_shape(&self) -> Option<&Shape> {
        match self.eqtb_obj(PAR_SHAPE_LOC) {
            Some(Obj::Shape(s)) => match &**s {
                Shaped::Lines(s) => Some(s),
                Shaped::Penalties(_) => None,
            },
            _ => None,
        }
    }

    /// e-TeX's penalty array at eqtb location `loc`, if set.
    pub(crate) fn penalties(&self, loc: i32) -> Option<&[i32]> {
        match self.eqtb_obj(loc) {
            Some(Obj::Shape(s)) => match &**s {
                Shaped::Penalties(p) => Some(p),
                Shaped::Lines(_) => None,
            },
            _ => None,
        }
    }
    pub(crate) fn int_par(&self, code: i32) -> i32 {
        self.eqtb_int(INT_BASE + code)
    }
    pub(crate) fn set_int_par(&mut self, code: i32, v: i32) {
        self.set_eqtb_int(INT_BASE + code, v);
    }
    pub(crate) fn count(&self, n: i32) -> i32 {
        self.eqtb_int(reg_loc(INT_VAL, n))
    }
    pub(crate) fn del_code(&self, c: i32) -> i32 {
        self.eqtb_int(code_loc(DEL_CODE_BASE, c))
    }
    pub(crate) fn dimen_par(&self, code: i32) -> i32 {
        self.eqtb_int(DIMEN_BASE + code)
    }
    pub(crate) fn set_dimen_par(&mut self, code: i32, v: i32) {
        self.set_eqtb_int(DIMEN_BASE + code, v);
    }
    pub(crate) fn dimen(&self, n: i32) -> i32 {
        self.eqtb_int(reg_loc(DIMEN_VAL, n))
    }
    pub(crate) fn escape_char(&self) -> i32 {
        self.int_par(ESCAPE_CHAR_CODE)
    }
    pub(crate) fn new_line_char(&self) -> i32 {
        self.int_par(NEW_LINE_CHAR_CODE)
    }
    pub(crate) fn tracing_online(&self) -> i32 {
        self.int_par(TRACING_ONLINE_CODE)
    }

    /// Initialize table entries (done by INITEX only): the parts from
    /// §222, §228, §232, §240 and §250.
    pub(crate) fn init_eqtb(&mut self) {
        // §222
        self.set_eq_type(UNDEFINED_CONTROL_SEQUENCE, UNDEFINED_CS);
        self.set_equiv(UNDEFINED_CONTROL_SEQUENCE, NULL);
        self.set_eq_level(UNDEFINED_CONTROL_SEQUENCE, LEVEL_ZERO);
        let undefined = self.eqtb(UNDEFINED_CONTROL_SEQUENCE);
        for k in ACTIVE_BASE..=self.eqtb_top {
            self.set_eqtb(k, undefined);
        }
        // §228
        self.set_equiv(GLUE_BASE, NULL);
        self.set_eq_level(GLUE_BASE, LEVEL_ONE);
        self.set_eq_type(GLUE_BASE, GLUE_REF);
        let w = self.eqtb(GLUE_BASE);
        for k in GLUE_BASE..LOCAL_BASE {
            self.set_eqtb_entry(k, w, Some(Obj::Glue(Glue::ZERO)));
        }
        // §232
        self.set_equiv(PAR_SHAPE_LOC, NULL);
        self.set_eq_type(PAR_SHAPE_LOC, SHAPE_REF);
        self.set_eq_level(PAR_SHAPE_LOC, LEVEL_ONE);
        let w = self.eqtb(PAR_SHAPE_LOC);
        for k in ETEX_PEN_BASE..ETEX_PENS {
            self.set_eqtb(k, w); // pdfTeX §250
        }
        let undefined = self.eqtb(UNDEFINED_CONTROL_SEQUENCE);
        for k in OUTPUT_ROUTINE_LOC..=TOKS_BASE + 255 {
            self.set_eqtb(k, undefined);
        }
        self.set_equiv(BOX_BASE, NULL);
        self.set_eq_type(BOX_BASE, BOX_REF);
        self.set_eq_level(BOX_BASE, LEVEL_ONE);
        let w = self.eqtb(BOX_BASE);
        for k in BOX_BASE + 1..=BOX_BASE + 255 {
            self.set_eqtb(k, w);
        }
        self.set_equiv(CUR_FONT_LOC, NULL_FONT);
        self.set_eq_type(CUR_FONT_LOC, DATA);
        self.set_eq_level(CUR_FONT_LOC, LEVEL_ONE);
        let w = self.eqtb(CUR_FONT_LOC);
        for k in MATH_FONT_BASE..MATH_FONT_BASE + NUMBER_MATH_FONTS {
            self.set_eqtb(k, w);
        }
        self.set_equiv(CAT_CODE_BASE, 0);
        self.set_eq_type(CAT_CODE_BASE, DATA);
        self.set_eq_level(CAT_CODE_BASE, LEVEL_ONE);
        let w = self.eqtb(CAT_CODE_BASE);
        for k in CAT_CODE_BASE + 1..INT_BASE {
            self.set_eqtb(k, w);
        }
        for k in 0..=255 {
            self.set_equiv(CAT_CODE_BASE + k, OTHER_CHAR);
            self.set_equiv(MATH_CODE_BASE + k, k);
            self.set_equiv(SF_CODE_BASE + k, 1000);
        }
        self.set_equiv(CAT_CODE_BASE + CARRIAGE_RETURN, CAR_RET);
        self.set_equiv(CAT_CODE_BASE + i32::from(b' '), SPACER);
        self.set_equiv(CAT_CODE_BASE + i32::from(b'\\'), ESCAPE);
        self.set_equiv(CAT_CODE_BASE + i32::from(b'%'), COMMENT);
        self.set_equiv(CAT_CODE_BASE + crate::charset::INVALID_CODE, INVALID_CHAR);
        self.set_equiv(CAT_CODE_BASE + NULL_CODE, IGNORE);
        // (`XeTeX` §258: class 7 is ``use the current family'')
        let (var_code, fam1) = if self.unicode {
            (
                crate::mathcodes::set_class_field(crate::mathcodes::VAR_FAM_CLASS),
                crate::mathcodes::set_family_field(1),
            )
        } else {
            (VAR_CODE, 0x100)
        };
        for k in i32::from(b'0')..=i32::from(b'9') {
            self.set_equiv(MATH_CODE_BASE + k, k + var_code);
        }
        let case = i32::from(b'a') - i32::from(b'A');
        for k in i32::from(b'A')..=i32::from(b'Z') {
            self.set_equiv(CAT_CODE_BASE + k, LETTER);
            self.set_equiv(CAT_CODE_BASE + k + case, LETTER);
            self.set_equiv(MATH_CODE_BASE + k, k + var_code + fam1);
            self.set_equiv(MATH_CODE_BASE + k + case, k + case + var_code + fam1);
            self.set_equiv(LC_CODE_BASE + k, k + case);
            self.set_equiv(LC_CODE_BASE + k + case, k + case);
            self.set_equiv(UC_CODE_BASE + k, k);
            self.set_equiv(UC_CODE_BASE + k + case, k);
            self.set_equiv(SF_CODE_BASE + k, 999);
        }
        // §240
        for k in INT_BASE..DEL_CODE_BASE {
            self.set_eqtb_int(k, 0);
        }
        self.set_int_par(CHAR_SUB_DEF_MIN_CODE, 256);
        self.set_int_par(CHAR_SUB_DEF_MAX_CODE, -1);
        self.set_int_par(MAG_CODE, 1000);
        self.set_int_par(TOLERANCE_CODE, 10000);
        self.set_int_par(HANG_AFTER_CODE, 1);
        self.set_int_par(MAX_DEAD_CYCLES_CODE, 25);
        self.set_int_par(ESCAPE_CHAR_CODE, i32::from(b'\\'));
        self.set_int_par(END_LINE_CHAR_CODE, CARRIAGE_RETURN);
        for k in 0..=255 {
            self.set_eqtb_int(DEL_CODE_BASE + k, -1);
        }
        self.set_eqtb_int(DEL_CODE_BASE + i32::from(b'.'), 0); // null delimiter
        self.set_int_par(SHOW_STREAM_CODE, -1); // pdfTeX §258
        if self.unicode {
            // `XeTeX`: "for backward compatibility with standard TeX by
            // default"
            self.set_eqtb_int(ETEX_STATE_BASE + XETEX_HYPHENATABLE_LENGTH_CODE, 63);
        }
        // §250
        for k in DIMEN_BASE..=EQTB_SIZE {
            self.set_eqtb_int(k, 0);
        }
        self.init_pdftex_params();
        // pdfTeX §1653: start in compatibility mode.
        self.etex_mode = false;
        self.init_etex_compat();
    }

    /// §254: `xeq_level` starts at level one (set initial values).
    pub(crate) fn init_xeq_level(&mut self) {
        for k in INT_BASE..=EQTB_SIZE {
            self.set_xeq_level(k, LEVEL_ONE);
        }
    }

    /// §225: the symbolic name of a glue parameter.
    pub(crate) fn print_skip_param(&mut self, n: i32) {
        let name: &[u8] = match n {
            LINE_SKIP_CODE => b"lineskip",
            BASELINE_SKIP_CODE => b"baselineskip",
            PAR_SKIP_CODE => b"parskip",
            ABOVE_DISPLAY_SKIP_CODE => b"abovedisplayskip",
            BELOW_DISPLAY_SKIP_CODE => b"belowdisplayskip",
            ABOVE_DISPLAY_SHORT_SKIP_CODE => b"abovedisplayshortskip",
            BELOW_DISPLAY_SHORT_SKIP_CODE => b"belowdisplayshortskip",
            LEFT_SKIP_CODE => b"leftskip",
            RIGHT_SKIP_CODE => b"rightskip",
            TOP_SKIP_CODE => b"topskip",
            SPLIT_TOP_SKIP_CODE => b"splittopskip",
            TAB_SKIP_CODE => b"tabskip",
            SPACE_SKIP_CODE => b"spaceskip",
            XSPACE_SKIP_CODE => b"xspaceskip",
            PAR_FILL_SKIP_CODE => b"parfillskip",
            THIN_MU_SKIP_CODE => b"thinmuskip",
            MED_MU_SKIP_CODE => b"medmuskip",
            THICK_MU_SKIP_CODE => b"thickmuskip",
            XETEX_LINEBREAK_SKIP_CODE if self.params.flavor == crate::params::Flavor::XeTeX => {
                b"XeTeXlinebreakskip"
            }
            _ => {
                self.print_str(b"[unknown glue parameter!]");
                return;
            }
        };
        self.print_esc(name);
    }

    /// §237: the symbolic name of an integer parameter.
    pub(crate) fn print_param(&mut self, n: i32) {
        match int_param_name(n) {
            Some(name) => self.print_esc(name),
            None => self.print_str(b"[unknown integer parameter!]"),
        }
    }

    /// §247: the symbolic name of a dimension parameter.
    pub(crate) fn print_length_param(&mut self, n: i32) {
        match dimen_param_name(n) {
            Some(name) => self.print_esc(name),
            None => self.print_str(b"[unknown dimen parameter!]"),
        }
    }

    /// §241: set `\time`, `\day`, `\month`, `\year` from the host.
    pub(crate) fn fix_date_and_time(&mut self) {
        let now = self.host.now();
        self.set_sys_time(now.minutes);
        self.set_sys_day(now.day);
        self.set_sys_month(now.month);
        self.set_sys_year(now.year);
        self.set_int_par(TIME_CODE, self.sys_time());
        self.set_int_par(DAY_CODE, self.sys_day());
        self.set_int_par(MONTH_CODE, self.sys_month());
        self.set_int_par(YEAR_CODE, self.sys_year());
    }

    /// §245: prepare to do some tracing.
    pub(crate) fn begin_diagnostic(&mut self) {
        self.old_setting = self.selector();
        if self.tracing_online() <= 0 && self.selector() == TERM_AND_LOG {
            self.set_selector(self.selector() - 1);
            if self.history() == SPOTLESS {
                self.set_history(WARNING_ISSUED);
            }
        }
    }

    /// §245: restore proper conditions after tracing.
    pub(crate) fn end_diagnostic(&mut self, blank_line: bool) {
        self.print_nl(b"");
        if blank_line {
            self.print_ln();
        }
        self.set_selector(self.old_setting);
    }
}

/// pdfTeX §255: TeX's, web2c's (`MLTeX`, `encTeX`, ...), pdfTeX's, e-TeX's and
/// `SyncTeX`'s integer parameters. One table serves every flavor: a flavor
/// has primitives for its own parameters only.
#[must_use]
pub fn int_param_name(n: i32) -> Option<&'static [u8]> {
    /// pdfTeX §1700: `eTeX_state_code+TeXXeT_code`.
    const TEXXET_STATE_CODE: i32 = ETEX_STATE_CODE + TEXXET_CODE;
    Some(match n {
        PRETOLERANCE_CODE => b"pretolerance",
        TOLERANCE_CODE => b"tolerance",
        LINE_PENALTY_CODE => b"linepenalty",
        HYPHEN_PENALTY_CODE => b"hyphenpenalty",
        EX_HYPHEN_PENALTY_CODE => b"exhyphenpenalty",
        CLUB_PENALTY_CODE => b"clubpenalty",
        WIDOW_PENALTY_CODE => b"widowpenalty",
        DISPLAY_WIDOW_PENALTY_CODE => b"displaywidowpenalty",
        BROKEN_PENALTY_CODE => b"brokenpenalty",
        BIN_OP_PENALTY_CODE => b"binoppenalty",
        REL_PENALTY_CODE => b"relpenalty",
        PRE_DISPLAY_PENALTY_CODE => b"predisplaypenalty",
        POST_DISPLAY_PENALTY_CODE => b"postdisplaypenalty",
        INTER_LINE_PENALTY_CODE => b"interlinepenalty",
        DOUBLE_HYPHEN_DEMERITS_CODE => b"doublehyphendemerits",
        FINAL_HYPHEN_DEMERITS_CODE => b"finalhyphendemerits",
        ADJ_DEMERITS_CODE => b"adjdemerits",
        MAG_CODE => b"mag",
        DELIMITER_FACTOR_CODE => b"delimiterfactor",
        LOOSENESS_CODE => b"looseness",
        TIME_CODE => b"time",
        DAY_CODE => b"day",
        MONTH_CODE => b"month",
        YEAR_CODE => b"year",
        SHOW_BOX_BREADTH_CODE => b"showboxbreadth",
        SHOW_BOX_DEPTH_CODE => b"showboxdepth",
        HBADNESS_CODE => b"hbadness",
        VBADNESS_CODE => b"vbadness",
        PAUSING_CODE => b"pausing",
        TRACING_ONLINE_CODE => b"tracingonline",
        TRACING_MACROS_CODE => b"tracingmacros",
        TRACING_STATS_CODE => b"tracingstats",
        TRACING_PARAGRAPHS_CODE => b"tracingparagraphs",
        TRACING_PAGES_CODE => b"tracingpages",
        TRACING_OUTPUT_CODE => b"tracingoutput",
        TRACING_LOST_CHARS_CODE => b"tracinglostchars",
        TRACING_COMMANDS_CODE => b"tracingcommands",
        TRACING_RESTORES_CODE => b"tracingrestores",
        UC_HYPH_CODE => b"uchyph",
        OUTPUT_PENALTY_CODE => b"outputpenalty",
        MAX_DEAD_CYCLES_CODE => b"maxdeadcycles",
        HANG_AFTER_CODE => b"hangafter",
        FLOATING_PENALTY_CODE => b"floatingpenalty",
        GLOBAL_DEFS_CODE => b"globaldefs",
        CUR_FAM_CODE => b"fam",
        ESCAPE_CHAR_CODE => b"escapechar",
        DEFAULT_HYPHEN_CHAR_CODE => b"defaulthyphenchar",
        DEFAULT_SKEW_CHAR_CODE => b"defaultskewchar",
        END_LINE_CHAR_CODE => b"endlinechar",
        NEW_LINE_CHAR_CODE => b"newlinechar",
        LANGUAGE_CODE => b"language",
        LEFT_HYPHEN_MIN_CODE => b"lefthyphenmin",
        RIGHT_HYPHEN_MIN_CODE => b"righthyphenmin",
        HOLDING_INSERTS_CODE => b"holdinginserts",
        ERROR_CONTEXT_LINES_CODE => b"errorcontextlines",
        CHAR_SUB_DEF_MIN_CODE => b"charsubdefmin",
        CHAR_SUB_DEF_MAX_CODE => b"charsubdefmax",
        TRACING_CHAR_SUB_DEF_CODE => b"tracingcharsubdef",
        MUBYTE_IN_CODE => b"mubytein",
        MUBYTE_OUT_CODE => b"mubyteout",
        MUBYTE_LOG_CODE => b"mubytelog",
        SPEC_OUT_CODE => b"specialout",
        TRACING_STACK_LEVELS_CODE => b"tracingstacklevels",
        PARTOKEN_CONTEXT_CODE => b"partokencontext",
        SHOW_STREAM_CODE => b"showstream",
        PDF_OUTPUT_CODE => b"pdfoutput",
        PDF_COMPRESS_LEVEL_CODE => b"pdfcompresslevel",
        PDF_OBJCOMPRESSLEVEL_CODE => b"pdfobjcompresslevel",
        PDF_DECIMAL_DIGITS_CODE => b"pdfdecimaldigits",
        PDF_MOVE_CHARS_CODE => b"pdfmovechars",
        PDF_IMAGE_RESOLUTION_CODE => b"pdfimageresolution",
        PDF_PK_RESOLUTION_CODE => b"pdfpkresolution",
        PDF_UNIQUE_RESNAME_CODE => b"pdfuniqueresname",
        PDF_OPTION_ALWAYS_USE_PDFPAGEBOX_CODE => b"pdfoptionalwaysusepdfpagebox",
        PDF_OPTION_PDF_INCLUSION_ERRORLEVEL_CODE => b"pdfoptionpdfinclusionerrorlevel",
        PDF_MAJOR_VERSION_CODE => b"pdfmajorversion",
        PDF_MINOR_VERSION_CODE => b"pdfminorversion",
        PDF_FORCE_PAGEBOX_CODE => b"pdfforcepagebox",
        PDF_PAGEBOX_CODE => b"pdfpagebox",
        PDF_INCLUSION_ERRORLEVEL_CODE => b"pdfinclusionerrorlevel",
        PDF_GAMMA_CODE => b"pdfgamma",
        PDF_IMAGE_GAMMA_CODE => b"pdfimagegamma",
        PDF_IMAGE_HICOLOR_CODE => b"pdfimagehicolor",
        PDF_IMAGE_APPLY_GAMMA_CODE => b"pdfimageapplygamma",
        PDF_ADJUST_SPACING_CODE => b"pdfadjustspacing",
        PDF_PROTRUDE_CHARS_CODE => b"pdfprotrudechars",
        PDF_TRACING_FONTS_CODE => b"pdftracingfonts",
        PDF_ADJUST_INTERWORD_GLUE_CODE => b"pdfadjustinterwordglue",
        PDF_PREPEND_KERN_CODE => b"pdfprependkern",
        PDF_APPEND_KERN_CODE => b"pdfappendkern",
        PDF_GEN_TOUNICODE_CODE => b"pdfgentounicode",
        PDF_DRAFTMODE_CODE => b"pdfdraftmode",
        PDF_INCLUSION_COPY_FONT_CODE => b"pdfinclusioncopyfonts",
        PDF_SUPPRESS_WARNING_DUP_DEST_CODE => b"pdfsuppresswarningdupdest",
        PDF_SUPPRESS_WARNING_DUP_MAP_CODE => b"pdfsuppresswarningdupmap",
        PDF_SUPPRESS_WARNING_PAGE_GROUP_CODE => b"pdfsuppresswarningpagegroup",
        PDF_INFO_OMIT_DATE_CODE => b"pdfinfoomitdate",
        PDF_SUPPRESS_PTEX_INFO_CODE => b"pdfsuppressptexinfo",
        PDF_OMIT_CHARSET_CODE => b"pdfomitcharset",
        PDF_OMIT_INFO_DICT_CODE => b"pdfomitinfodict",
        PDF_OMIT_PROCSET_CODE => b"pdfomitprocset",
        PDF_PTEX_USE_UNDERSCORE_CODE => b"pdfptexuseunderscore",
        TRACING_ASSIGNS_CODE => b"tracingassigns",
        TRACING_GROUPS_CODE => b"tracinggroups",
        TRACING_IFS_CODE => b"tracingifs",
        TRACING_SCAN_TOKENS_CODE => b"tracingscantokens",
        TRACING_NESTING_CODE => b"tracingnesting",
        PRE_DISPLAY_DIRECTION_CODE => b"predisplaydirection",
        LAST_LINE_FIT_CODE => b"lastlinefit",
        SAVING_VDISCARDS_CODE => b"savingvdiscards",
        SAVING_HYPH_CODES_CODE => b"savinghyphcodes",
        IGNORE_PRIMITIVE_ERROR_CODE => b"ignoreprimitiveerror",
        TEXXET_STATE_CODE => b"TeXXeTstate",
        // `XeTeX`'s (codes no other engine uses)
        SUPPRESS_FONTNOTFOUND_ERROR_CODE => b"suppressfontnotfounderror",
        XETEX_LINEBREAK_PENALTY_CODE => b"XeTeXlinebreakpenalty",
        XETEX_PROTRUDE_CHARS_CODE => b"XeTeXprotrudechars",
        c if c == ETEX_STATE_CODE + XETEX_UPWARDS_CODE => b"XeTeXupwardsmode",
        c if c == ETEX_STATE_CODE + XETEX_USE_GLYPH_METRICS_CODE => b"XeTeXuseglyphmetrics",
        c if c == ETEX_STATE_CODE + XETEX_INTER_CHAR_TOKENS_CODE => b"XeTeXinterchartokenstate",
        c if c == ETEX_STATE_CODE + XETEX_DASH_BREAK_CODE => b"XeTeXdashbreakstate",
        c if c == ETEX_STATE_CODE + XETEX_INPUT_NORMALIZATION_CODE => b"XeTeXinputnormalization",
        c if c == ETEX_STATE_CODE + XETEX_TRACING_FONTS_CODE => b"XeTeXtracingfonts",
        c if c == ETEX_STATE_CODE + XETEX_INTERWORD_SPACE_SHAPING_CODE => {
            b"XeTeXinterwordspaceshaping"
        }
        c if c == ETEX_STATE_CODE + XETEX_GENERATE_ACTUAL_TEXT_CODE => b"XeTeXgenerateactualtext",
        c if c == ETEX_STATE_CODE + XETEX_HYPHENATABLE_LENGTH_CODE => b"XeTeXhyphenatablelength",
        SYNCTEX_CODE => b"synctex",
        _ => return None,
    })
}

/// pdfTeX §265: `print_length_param`.
#[must_use]
pub fn dimen_param_name(n: i32) -> Option<&'static [u8]> {
    Some(match n {
        PAR_INDENT_CODE => b"parindent",
        MATH_SURROUND_CODE => b"mathsurround",
        LINE_SKIP_LIMIT_CODE => b"lineskiplimit",
        HSIZE_CODE => b"hsize",
        VSIZE_CODE => b"vsize",
        MAX_DEPTH_CODE => b"maxdepth",
        SPLIT_MAX_DEPTH_CODE => b"splitmaxdepth",
        BOX_MAX_DEPTH_CODE => b"boxmaxdepth",
        HFUZZ_CODE => b"hfuzz",
        VFUZZ_CODE => b"vfuzz",
        DELIMITER_SHORTFALL_CODE => b"delimitershortfall",
        NULL_DELIMITER_SPACE_CODE => b"nulldelimiterspace",
        SCRIPT_SPACE_CODE => b"scriptspace",
        PRE_DISPLAY_SIZE_CODE => b"predisplaysize",
        DISPLAY_WIDTH_CODE => b"displaywidth",
        DISPLAY_INDENT_CODE => b"displayindent",
        OVERFULL_RULE_CODE => b"overfullrule",
        HANG_INDENT_CODE => b"hangindent",
        H_OFFSET_CODE => b"hoffset",
        V_OFFSET_CODE => b"voffset",
        EMERGENCY_STRETCH_CODE => b"emergencystretch",
        PDF_H_ORIGIN_CODE => b"pdfhorigin",
        PDF_V_ORIGIN_CODE => b"pdfvorigin",
        PDF_PAGE_WIDTH_CODE => b"pdfpagewidth",
        PDF_PAGE_HEIGHT_CODE => b"pdfpageheight",
        PDF_LINK_MARGIN_CODE => b"pdflinkmargin",
        PDF_DEST_MARGIN_CODE => b"pdfdestmargin",
        PDF_THREAD_MARGIN_CODE => b"pdfthreadmargin",
        PDF_FIRST_LINE_HEIGHT_CODE => b"pdffirstlineheight",
        PDF_LAST_LINE_DEPTH_CODE => b"pdflastlinedepth",
        PDF_EACH_LINE_HEIGHT_CODE => b"pdfeachlineheight",
        PDF_EACH_LINE_DEPTH_CODE => b"pdfeachlinedepth",
        PDF_IGNORED_DIMEN_CODE => b"pdfignoreddimen",
        PDF_PX_DIMEN_CODE => b"pdfpxdimen",
        _ => return None,
    })
}

/// Control sequence `p`'s class to a skip, hashed (0 for class 0), for
/// `class_hash`.
fn class_mix(p: i32, class: u8) -> u128 {
    if class == 0 {
        0
    } else {
        partex_engine::stablehash::StableHasher::of(&(p, class))
    }
}
