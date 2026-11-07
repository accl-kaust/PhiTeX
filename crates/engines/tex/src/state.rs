//! The run's view of TeX's state (docs/spec-state-model.md): every item
//! its own engine field, never a table; a read recorded per item the run
//! uses; a write that is nothing (a restore of what the run read), a
//! relation (an integer register stepped by a constant, never read) or a
//! value, by one rule ([`State::publication`]); page-layer items read as
//! symbols in pieces ([`State::symbol`], `crate::defer`).
//!
//! The table holds, per location ([`Loc`]): the item's current value as
//! a flat record ([`Entry`]: a meaning's command code and modifier and
//! its macro by handle, an integer, a glue or token list by handle, or
//! the engine's value), what reached the run (the value it read, kept
//! with its first write's bookkeeping, [`Meta`]), an integer register's
//! map over the run ([`Map`]) while the run never read it, and the
//! run-local group saves (§277–§283): a local write inside a group the
//! run opens is restored at the group's close and never published; a
//! group the run leaves open is a scope of the engine, its locals
//! published at its depth (`Locality::Scope`, `Locality::ScopeAt`).
//!
//! Locations: the fixed ranges and the meanings of [`crate::fields`]
//! (below [`SPILL`]), and a spill for any other field, by its handle.

use std::rc::Rc;

use phitex_defuse::{Ctx, Field, FxHashMap, Locality, ValueId, Values};
use phitex_typeset::glue::Glue;

use crate::val::{Mean, Tok};

/// A location in the table.
pub(crate) type Loc = u32;

/// The bit of a spill location (its index in the spill below it).
pub(crate) const SPILL: Loc = 1 << 31;

/// An integer register's map over this run: `x ↦ a·x + b`, `keep` true
/// for `add b`, false for `set b`, the sum wrapping at 32 bits as
/// `\advance` does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Map {
    pub keep: bool,
    pub add: i32,
}

impl Map {
    pub(crate) const ID: Map = Map { keep: true, add: 0 };
    #[allow(dead_code)]
    pub(crate) fn set(c: i32) -> Map {
        Map {
            keep: false,
            add: c,
        }
    }
    pub(crate) fn add(c: i32) -> Map {
        Map { keep: true, add: c }
    }
    /// This map, then `next`.
    #[must_use]
    pub(crate) fn then(self, next: Map) -> Map {
        if next.keep {
            Map {
                keep: self.keep,
                add: self.add.wrapping_add(next.add),
            }
        } else {
            next
        }
    }
    pub(crate) fn apply(self, x: i32) -> i32 {
        if self.keep {
            x.wrapping_add(self.add)
        } else {
            self.add
        }
    }
}

/// What the run holds for an item (spec §2.6), as [`State::held`] tells it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Held {
    /// Not touched yet (or an integer register only stepped, [`Map`]).
    Absent,
    /// A value put in place of the engine's (a finisher's state at its
    /// candidate, a job-written line's state at its read-back point): no
    /// read recorded.
    Loaded(Option<ValueId>),
    /// A page-layer item in a piece: its symbol handed out, no read.
    Symbol,
    /// Used: the read is recorded with this value.
    Used(Option<ValueId>),
    /// Written by this run.
    Written,
}

/// The families of fixed locations (the layout of [`crate::fields`]),
/// by the prefix of their fields' names: what a run's reads are counted
/// by ([`crate::run::NodeAudit`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Family {
    Cat,
    Lc,
    Uc,
    Sf,
    Mc,
    Dc,
    Param,
    Count,
    Dimen,
    Skip,
    MuSkip,
    Toks,
    Box,
    Fam,
    Font,
    Active,
    Meaning,
}

impl Family {
    /// The field name prefix of the family (`cat:`, `param:`, …); a
    /// font's cells go by several (`fontinfo:`, `fd:`, …): `font`.
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Family::Cat => "cat:",
            Family::Lc => "lc:",
            Family::Uc => "uc:",
            Family::Sf => "sf:",
            Family::Mc => "mc:",
            Family::Dc => "dc:",
            Family::Param => "param:",
            Family::Count => "count:",
            Family::Dimen => "dimen:",
            Family::Skip => "skip:",
            Family::MuSkip => "muskip:",
            Family::Toks => "toks:",
            Family::Box => "box:",
            Family::Fam => "fam:",
            Family::Font => "font",
            Family::Active => "act:",
            Family::Meaning => "cs:",
        }
    }

    /// The family a location belongs to; `None` for a state cell with a
    /// fixed name and for the spill.
    pub(crate) fn of(loc: Loc) -> Option<Family> {
        use crate::fields::{
            BOX0, CAT0, CODE0, FAM0, FONT0, MEAN_ACTIVE, MEAN_BASE, REG0, STATIC0,
        };
        let l = loc as usize;
        if loc & SPILL != 0 {
            return None;
        }
        Some(if l < CAT0 {
            Family::Param
        } else if l < REG0 {
            Family::Cat
        } else if l < CODE0 {
            [
                Family::Count,
                Family::Dimen,
                Family::Skip,
                Family::MuSkip,
                Family::Toks,
            ][(l - REG0) / 256]
        } else if l < BOX0 {
            [Family::Lc, Family::Uc, Family::Sf, Family::Mc, Family::Dc][(l - CODE0) / 256]
        } else if l < FAM0 {
            Family::Box
        } else if l < STATIC0 {
            Family::Fam
        } else if l < FONT0 {
            return None;
        } else if l < MEAN_ACTIVE {
            Family::Font
        } else if l < MEAN_BASE {
            Family::Active
        } else {
            Family::Meaning
        })
    }
}

/// [`State::read_across`]'s length before its first merge.
const ACROSS_MERGE: usize = 1 << 14;

/// [`State::read_across`]'s entries of a location that overlap taken as one:
/// a binding read in stretch after stretch is an entry each.
fn merge_across(xs: &mut Vec<(Loc, u32, u32)>) {
    xs.sort_unstable();
    xs.dedup_by(|b, a| {
        let same = a.0 == b.0 && b.1 <= a.2;
        if same {
            a.2 = a.2.max(b.2);
        }
        same
    });
}

/// Whether every read of `loc`'s value a run makes is one
/// [`State::note_read`] sees, at the place it is made. Not: what a run reads
/// through a memo kept while the table is unchanged (category codes, font
/// cells, `\\endlinechar`), the codes that go with them, what the page
/// builder takes from a candidate's markers (the page parameters) and box
/// registers, which a `\\box` or `\\wd` reads by other ways.
pub(crate) fn read_by_position(loc: Loc) -> bool {
    use crate::fields::Par;
    !is_page_param(loc)
        && loc != Par::endlinechar as Loc
        && !matches!(
            Family::of(loc),
            Some(
                Family::Cat
                    | Family::Lc
                    | Family::Uc
                    | Family::Sf
                    | Family::Mc
                    | Family::Dc
                    | Family::Box
                    | Family::Fam
                    | Family::Font
            )
        )
}

/// The kinds of a slot's value ([`Entry`]'s `kind`): an engine value
/// not decoded yet, a typed one, or a meaning (`K_MEAN0` and above:
/// `cmd` and `chr` hold its command code and modifier, `payload` its
/// macro or primitive name).
const K_ABSENT: u8 = 0;
const K_RAW: u8 = 1;
const K_INT: u8 = 2;
const K_GLUE: u8 = 3;
const K_TOKS: u8 = 4;
const K_MEAN0: u8 = 8;
const K_PRIM: u8 = 8;
const K_MACRO: u8 = 9;

/// The macro store's length before [`State::compact_macros`] first looks.
const MACROS_AT: usize = 1 << 16;
const K_CHAR: u8 = 10;
const K_UNDEF: u8 = 11;
const K_FONT: u8 = 12;
const K_CHARDEF: u8 = 13;
const K_MATHCHARDEF: u8 = 14;
const K_REG: u8 = 15;
const K_NOTEXP: u8 = 16;

const LOADED: u8 = 1;
const HAS_ID: u8 = 2;
/// A value put in place of the engine's ([`State::preload`]): no read.
const PRELOADED: u8 = 4;
/// `cmd` and `chr` hold the meaning's command code and modifier
/// (`eq_type`, `equiv`, §221).
const CC: u8 = 8;
/// The entry's engine value (`id`) is an alias definition's
/// (`phitex_defuse::alias_value`): another register's value as of a
/// piece, a symbol written to this item ([`Entry::alias`]); a use
/// resolves it first (`Run::load`, [`State::resolve_alias`]).
const ALIAS: u8 = 16;
/// An entry kept at a candidate ([`State::note_cand`]) of an integer
/// register the run had stepped there and not read: its value there is
/// what reached the run plus `chr` ([`AtVal::Reaching`]).
const STEPPED: u8 = 32;

/// One slot's value (spec-final §2.1): a meaning's record, `cmd`, `chr`
/// and a `payload` (a macro's handle in the table's macro store, a
/// primitive's name, a glue's or a token list's handle), or an integer
/// in `chr`; the engine value once known (`id`). Copied, never cloned:
/// nothing in it is reference counted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    kind: u8,
    flags: u8,
    cmd: u8,
    chr: i32,
    payload: u32,
    id: Option<ValueId>,
}

const UNLOADED: Entry = Entry {
    kind: K_ABSENT,
    flags: 0,
    cmd: 0,
    chr: 0,
    payload: 0,
    id: None,
};

impl Entry {
    /// The engine's value as it is (`None`: absent), not decoded.
    pub(crate) const fn raw(x: Option<ValueId>) -> Entry {
        Entry {
            kind: if x.is_some() { K_RAW } else { K_ABSENT },
            flags: HAS_ID,
            cmd: 0,
            chr: 0,
            payload: 0,
            id: x,
        }
    }

    /// Field `g`'s value as of piece `at`, unseen: a symbol written to
    /// an integer register (spec §2.2, `crate::defer::try_symbolic`),
    /// published as the alias definition it is (`Ctx::define_alias`).
    pub(crate) fn alias(v: &Values, g: Field, at: phitex_defuse::NodeId) -> Entry {
        Entry {
            kind: K_RAW,
            flags: HAS_ID | ALIAS,
            cmd: 0,
            chr: 0,
            payload: 0,
            id: Some(phitex_defuse::alias_value(v, g, at)),
        }
    }

    /// An alias definition's value as it is (an offset with it kept),
    /// known as one ([`ALIAS`]).
    pub(crate) fn alias_raw(x: ValueId) -> Entry {
        Entry {
            kind: K_RAW,
            flags: HAS_ID | ALIAS,
            cmd: 0,
            chr: 0,
            payload: 0,
            id: Some(x),
        }
    }

    /// An entry holding engine value `x` as it is, an alias definition's
    /// value known as one ([`ALIAS`]).
    pub(crate) fn raw_seen(v: &Values, x: Option<ValueId>) -> Entry {
        let mut e = Entry::raw(x);
        if x.is_some_and(|y| phitex_defuse::alias_of(v, y).is_some()) {
            e.flags |= ALIAS;
        }
        e
    }

    /// An integer (a count, a dimension, a code).
    pub(crate) const fn int(i: i32) -> Entry {
        Entry {
            kind: K_INT,
            flags: 0,
            cmd: 0,
            chr: i,
            payload: 0,
            id: None,
        }
    }
}

/// How an integer register's steps are kept while the run never read
/// it ([`Slot::mapped`]).
const MAP_NONE: u8 = 0;
const MAP_LOCAL: u8 = 1;
const MAP_GLOBAL: u8 = 2;

#[derive(Clone)]
struct Slot {
    e: Entry,
    /// The group ([`State::open`]'s number) whose save holds this slot's
    /// previous value, and the stamp of that save.
    save_gid: u32,
    save_stamp: u64,
    /// The stamp of the slot's latest global definition: saves stamped
    /// before it are inert (§283).
    ginert: u64,
    /// Its definition record, plus one (0: never defined).
    meta: u32,
    touched: bool,
    field: Option<Field>,
    /// An integer register stepped by constants and never read: the
    /// steps composed ([`State::add`]), local or global ([`MAP_LOCAL`],
    /// [`MAP_GLOBAL`]); none once the register is read or set.
    map: Map,
    mapped: u8,
    /// The value the run read of a register it steps ([`State::add`]):
    /// what reached it, the steps applied to it in [`Slot::e`].
    base: Option<ValueId>,
    /// Steps kept under a local write in a group the run made before it
    /// read the register ([`State::set`]): [`Slot::base`] unknown until
    /// the run reads it ([`State::set_step_base`]).
    base_unread: bool,
    /// A value given for the slot where the run first uses it, read then
    /// (a finisher's state at its candidate, `Run::fin_lazy_value`).
    lazy: Option<ValueId>,
    /// The run handed out its symbol ([`State::symbol`]).
    sym: bool,
    /// The group ([`State::open`]'s number) whose part of
    /// [`State::defined`] holds this location already: a local
    /// definition there again is not listed again.
    def_gid: u32,
    /// The candidate ([`State::cands`]'s index, plus one) whose entries
    /// hold this location already: only the first entry at a candidate
    /// is its value there ([`State::at_candidates`]).
    cand_noted: u32,
    /// The candidates gone past when the binding in force was last read
    /// ([`State::note_read`]): reads between two of them count once.
    read_at: std::cell::Cell<u32>,
}

impl Default for Slot {
    fn default() -> Slot {
        Slot {
            e: UNLOADED,
            save_gid: 0,
            save_stamp: 0,
            ginert: 0,
            meta: 0,
            touched: false,
            field: None,
            map: Map::ID,
            mapped: MAP_NONE,
            base: None,
            base_unread: false,
            lazy: None,
            sym: false,
            def_gid: 0,
            cand_noted: 0,
            read_at: std::cell::Cell::new(0),
        }
    }
}

/// What a run defined at a location: the write log's summary for it.
#[derive(Clone)]
struct Meta {
    loc: Loc,
    /// The engine's value when first defined, if the run had read it.
    // (None: not read; Some(None): read, undefined in the engine)
    #[allow(clippy::option_option)]
    eng: Option<Option<ValueId>>,
    /// The definition in force at the run's base level, if any.
    publ: Option<(Entry, Locality)>,
    /// The stamp of the latest definition, and whether it was local.
    ldef_stamp: u64,
    ldef_local: bool,
    /// The group of the run the latest local definition was made in
    /// ([`State::open`]'s number; `None`: the base level, or unknown).
    ldef_gid: Option<u32>,
}

/// A value saved by a group, restored at its close.
#[derive(Clone)]
struct Saved {
    loc: Loc,
    prev: Entry,
    gid: u32,
    stamp: u64,
    /// The stamp of this save.
    at: u64,
    /// The slot's latest-definition mark when saved (stamp, local, its
    /// group), restored with the value: after a group's close the
    /// binding in force is the saved one, an outer group's local or the
    /// run's base, and the mark says whose.
    ldef: Ldef,
    /// The value the slot was given to be read where the run first uses
    /// it ([`State::set_lazy`]), not read yet when the write hid it: the
    /// group's close gives it back (a finisher's local at its candidate,
    /// redefined in a group of the routine and read after it).
    lazy: Option<ValueId>,
}

/// A latest-definition mark: its stamp, whether local, its group.
pub(crate) type Ldef = (u64, bool, Option<u32>);

/// What a run publishes of an item (spec §2.3, [`State::publication`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pub {
    /// A value (rule 4).
    Value(ValueId),
    /// `+c` on the value reaching the run (rule 3).
    Rel(i64),
    /// Items appended to the macro reaching the run, a list value of
    /// token lists (`Run::appending`, `phitex_defuse::Rel::Append`).
    Append(ValueId),
}

/// One publication: the location, what, where, and whether it is a
/// continuation's carried binding, published only where it differs from
/// the value reaching the run (rule 2, checked by the run with a read).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Published {
    pub loc: Loc,
    pub what: Pub,
    pub at: Locality,
    pub carried: bool,
}

/// The table.
pub(crate) struct State {
    eq: Vec<Slot>,
    spill: Vec<Slot>,
    spill_ix: FxHashMap<Field, u32>,
    /// The spill indices of the numbered fields ([`State::numbered`]), by
    /// prefix (its address) and number.
    numbered: FxHashMap<(usize, i64), u32>,
    touched: Vec<Loc>,
    metas: Vec<Meta>,
    /// The scope's definitions of locations the run's base level also
    /// publishes, at another value: the field's second entry, the scope's
    /// ([`State::abandon_to_scope`]); with several frames left open, each
    /// frame's locals at its own frame ([`State::abandon_to_frames`],
    /// `Locality::ScopeAt`). Made as the run ends.
    scope_extra: Vec<(Loc, Entry, Locality)>,
    /// The values the slots hold by handle (an entry's `payload`): the
    /// glues, token lists and macros decoded or defined in this run.
    glues: Vec<Glue>,
    toks: Vec<Rc<[Tok]>>,
    macros: Vec<Rc<crate::val::Macro>>,
    /// The store's length at which [`State::compact_macros`] next looks:
    /// a macro redefined leaves its old body behind (TeX frees it, §275),
    /// and a run defining in a loop grew the store without end
    /// (lisp-on-tex's div2: 60 GB at 470 MB/s; pegmatch, codehigh: 8 GB).
    macros_at: usize,
    save: Vec<Saved>,
    /// Locations defined locally inside a group of the run (for a group
    /// left open: its definitions become the run's).
    defined: Vec<Loc>,
    stamp: u64,
    /// A page parameter (§980: `\\vsize`, `\\maxdepth`, `\\topskip`,
    /// `\\holdinginserts`) defined at any level, or restored at a
    /// group's end, since the last [`State::take_page_params`]: the
    /// page builder reads eqtb as it stands (§987), groups or not.
    pp_dirty: bool,
    next_gid: u32,
    /// The locations a continuation's table brought ([`State::from_cont`]):
    /// a group-local binding it publishes again is no definition where it
    /// equals the value reaching the run ([`Published::carried`]).
    cont_locs: Vec<Loc>,
    /// Bumped whenever a category code or another code below 256 may
    /// have changed ([`State::cat_gen`]); unique across tables of a
    /// thread, so a table put in place of another is never mistaken for
    /// it.
    cat_gen: u64,
    /// Bumped whenever any slot's value may have changed: the run's
    /// memos of decoded state (`Run::font_entry`) stand while it does.
    any_gen: u64,
    /// Bumped whenever a font's cell may have changed.
    font_gen: u64,
    /// The font cells without a fixed location the run asked for
    /// ([`State::note_font_loc`]).
    font_slow: phitex_defuse::FxHashSet<Loc>,
    /// Bumped by every change of the table a command could not undo by
    /// rewinding its input (the table's part of `Run::mutations`).
    muts: u64,
    /// The page-builder candidates the run passed
    /// ([`State::mark_candidate`]): each one's stamp and, for each
    /// location defined after it and before the next one for the first
    /// time since it, the entry the location held at the candidate.
    cands: Vec<(u64, Vec<(Loc, Entry)>)>,
    /// A run of a code table's entries set but not yet written ([`BulkCodes`]).
    bulk: Option<Box<BulkCodes>>,
    /// The candidates the run went on past where a page's routine can
    /// reach what it reads after ([`State::mark_past`]): each one's clock
    /// and number.
    past: Vec<(u64, u32)>,
    /// `past.len()`, one load for a read's check ([`State::note_read`]).
    past_n: u32,
    /// What the run read holding a binding from before a candidate it went
    /// on past: `(loc, from, to)`, a value bound before [`State::past`]'s
    /// `from` was read after its `to - 1`. A routine's global definition at
    /// one of those candidates is what TeX's read there saw
    /// ([`State::note_read`]). A [`State::rollback`] leaves a read it took
    /// back as a read made, but for the spill locations it drops.
    read_across: std::cell::RefCell<Vec<(Loc, u32, u32)>>,
    /// The length of [`State::read_across`] at which it is merged next.
    across_merge_at: std::cell::Cell<usize>,
    /// The locations a routine's value was put in force at
    /// ([`State::apply_global`]), with the candidate: no older binding shows
    /// through it (§283). Few, and looked at once a stretch.
    floors: Vec<(Loc, u32)>,
    /// The table as it stood at [`State::checkpoint`], kept as it
    /// changes: [`State::rollback`] puts it back.
    undo: Option<Box<Undo>>,
    /// The meanings defined by a body whose expansion read the lines
    /// before the run (`\inputlineno`, `Run::lines_taint`): a look at
    /// one of them sets `taint_read`.
    tainted: Vec<Loc>,
    taint_read: std::cell::Cell<bool>,
}

/// What [`State::rollback`] puts back: each slot and metas entry as it
/// was before the first change since the checkpoint, the lengths of the
/// vectors that only grow meanwhile, the others whole, and the
/// counters.
pub(crate) struct Undo {
    slots: Vec<(Loc, Slot)>,
    logged: phitex_defuse::FxHashSet<Loc>,
    metas: Vec<(usize, Meta)>,
    metas_logged: phitex_defuse::FxHashSet<usize>,
    lens: [usize; 7],
    spill_added: Vec<Field>,
    save: Vec<Saved>,
    defined: Vec<Loc>,
    scope_extra: Vec<(Loc, Entry, Locality)>,
    cont_locs: Vec<Loc>,
    cands: Vec<(u64, Vec<(Loc, Entry)>)>,
    past: usize,
    stamp: u64,
    pp_dirty: bool,
    next_gid: u32,
    cat_gen: u64,
    any_gen: u64,
    font_gen: u64,
    muts: u64,
}

thread_local! {
    static CAT_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A new catcode generation, never given out before on this thread.
fn new_cat_gen() -> u64 {
    CAT_GEN.with(|c| {
        c.set(c.get() + 1);
        c.get()
    })
}

/// Whether `loc` is a category code or another code table's entry
/// below 256 (the run's flat copies, `Run::cat`, `Run::code`).
#[inline]
fn is_cat(loc: Loc) -> bool {
    (loc as usize).wrapping_sub(crate::fields::CAT0) < 256
        || (loc as usize).wrapping_sub(crate::fields::CODE0) < 5 * 256
}

thread_local! {
    /// Tables of finished runs, emptied, for the next run on the thread
    /// (a table spans every control sequence seen).
    static POOL: std::cell::RefCell<Vec<Vec<Slot>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The touched lists of finished runs, emptied.
    static TOUCHED_POOL: std::cell::RefCell<Vec<Vec<Loc>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The metas' vectors of the thread's tables dropped, emptied.
    static METAS_POOL: std::cell::RefCell<Vec<Vec<Meta>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Clone for State {
    fn clone(&self) -> State {
        // (a pooled table is all default slots, and a slot of this one
        // that is not is in its touched list, [`State::slot`]: those
        // copied, not the whole table)
        let mut eq = POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
        eq.truncate(self.eq.len());
        eq.resize_with(self.eq.len(), Slot::default);
        for &l in &self.touched {
            if l < SPILL
                && let Some(s) = self.eq.get(l as usize)
            {
                eq[l as usize] = s.clone();
            }
        }
        State {
            eq,
            spill: self.spill.clone(),
            spill_ix: self.spill_ix.clone(),
            numbered: self.numbered.clone(),
            touched: self.touched.clone(),
            metas: {
                let mut m = METAS_POOL
                    .with(|p| p.borrow_mut().pop())
                    .unwrap_or_default();
                m.extend_from_slice(&self.metas);
                m
            },
            scope_extra: self.scope_extra.clone(),
            save: self.save.clone(),
            defined: self.defined.clone(),
            stamp: self.stamp,
            pp_dirty: self.pp_dirty,
            next_gid: self.next_gid,
            cont_locs: self.cont_locs.clone(),
            glues: self.glues.clone(),
            toks: self.toks.clone(),
            macros: self.macros.clone(),
            macros_at: self.macros_at,
            cat_gen: new_cat_gen(),
            any_gen: new_cat_gen(),
            font_gen: new_cat_gen(),
            font_slow: self.font_slow.clone(),
            muts: self.muts,
            cands: self.cands.clone(),
            bulk: self.bulk.clone(),
            past: self.past.clone(),
            past_n: self.past_n,
            read_across: self.read_across.clone(),
            across_merge_at: self.across_merge_at.clone(),
            floors: self.floors.clone(),
            undo: None,
            tainted: self.tainted.clone(),
            taint_read: self.taint_read.clone(),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let mut eq = std::mem::take(&mut self.eq);
        for &l in &self.touched {
            if l < SPILL
                && let Some(s) = eq.get_mut(l as usize)
            {
                *s = Slot::default();
            }
        }
        let mut metas = std::mem::take(&mut self.metas);
        metas.clear();
        if metas.capacity() > 0 {
            METAS_POOL.with(|p| {
                let mut p = p.borrow_mut();
                if p.len() < 4 {
                    p.push(metas);
                }
            });
        }
        let mut touched = std::mem::take(&mut self.touched);
        touched.clear();
        TOUCHED_POOL.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < 4 {
                p.push(touched);
            }
        });
        POOL.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < 4 {
                p.push(eq);
            }
        });
    }
}

/// A location's value at a candidate ([`State::at_candidates`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AtVal {
    /// The value there (`None`: absent).
    Known(Option<ValueId>),
    /// Not read by the run: what reached the run, plus the steps the run
    /// made of the integer register before the candidate (0 for any other
    /// location, [`State::add`]).
    Reaching(i32),
}

/// A location's value at a candidate ([`State::at_candidates`]): the
/// candidate's index, and the value.
pub(crate) type AtCandidate = (usize, AtVal);

/// The values at the candidates of the locations defined after them
/// ([`State::at_candidates`]).
pub(crate) struct AtCandidates {
    by_loc: FxHashMap<Loc, Vec<AtCandidate>>,
}

impl AtCandidates {
    /// `loc`'s value at candidate `i`: `None` where nothing after the
    /// candidate defined it.
    pub(crate) fn get(&self, i: usize, loc: Loc) -> Option<AtVal> {
        let h = self.by_loc.get(&loc)?;
        h.get(h.partition_point(|x| x.0 < i)).map(|x| x.1)
    }

    /// The candidate (by index) after which the binding of `loc` in force
    /// at candidate `i` was made, where the run made it after one.
    pub(crate) fn made_after(&self, i: usize, loc: Loc) -> Option<usize> {
        let h = self.by_loc.get(&loc)?;
        let p = h.partition_point(|x| x.0 < i);
        p.checked_sub(1).map(|p| h[p].0)
    }

    /// `loc`'s entries at the candidates, by candidate.
    pub(crate) fn history(&self, loc: Loc) -> &[AtCandidate] {
        self.by_loc.get(&loc).map_or(&[], Vec::as_slice)
    }

    /// Every location defined after some candidate.
    pub(crate) fn locs(&self) -> impl Iterator<Item = Loc> + '_ {
        self.by_loc.keys().copied()
    }
}

/// Whether `m` is a definition of a command's transient state outside
/// the table of equivalents (§1266 `after_token`: set by one command,
/// emptied by the next assignment, never saved or restored by a group)
/// that leaves the value the run read before it: then it changes
/// nothing at any position, in a scope or not, and is not published (an
/// empty token and no definition are the same).
fn unchanged_transient(m: &Meta, x: ValueId) -> bool {
    m.loc == crate::fields::cell::AFTERTOKEN
        && match m.eng {
            Some(Some(e)) => e == x,
            Some(None) => x == ValueId::UNIT,
            None => false,
        }
}

impl State {
    pub(crate) fn new() -> State {
        State {
            eq: POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default(),
            spill: Vec::new(),
            spill_ix: FxHashMap::default(),
            numbered: FxHashMap::default(),
            touched: TOUCHED_POOL
                .with(|p| p.borrow_mut().pop())
                .unwrap_or_else(|| Vec::with_capacity(256)),
            metas: METAS_POOL
                .with(|p| p.borrow_mut().pop())
                .unwrap_or_default(),
            scope_extra: Vec::new(),
            save: Vec::new(),
            defined: Vec::new(),
            stamp: 0,
            pp_dirty: false,
            next_gid: 0,
            cont_locs: Vec::new(),
            glues: Vec::new(),
            toks: Vec::new(),
            macros: Vec::new(),
            macros_at: MACROS_AT,
            cat_gen: new_cat_gen(),
            any_gen: new_cat_gen(),
            font_gen: new_cat_gen(),
            font_slow: phitex_defuse::FxHashSet::default(),
            muts: 0,
            cands: Vec::new(),
            bulk: None,
            past: Vec::new(),
            past_n: 0,
            read_across: std::cell::RefCell::new(Vec::new()),
            across_merge_at: std::cell::Cell::new(ACROSS_MERGE),
            floors: Vec::new(),
            undo: None,
            tainted: Vec::new(),
            taint_read: std::cell::Cell::new(false),
        }
    }

    // ------------------------------------------------------------ undo

    /// Every entry the table holds that may name a macro by handle: the
    /// slots, the definition records' published entries, the groups'
    /// saves, the scope's extra definitions and the candidates' entries.
    fn each_entry(&mut self, mut f: impl FnMut(&mut Entry)) {
        for s in self.eq.iter_mut().chain(self.spill.iter_mut()) {
            f(&mut s.e);
        }
        for m in &mut self.metas {
            if let Some((e, _)) = &mut m.publ {
                f(e);
            }
        }
        for s in &mut self.save {
            f(&mut s.prev);
        }
        for (_, e, _) in &mut self.scope_extra {
            f(e);
        }
        for (_, es) in &mut self.cands {
            for (_, e) in es {
                f(e);
            }
        }
    }

    /// The macro store without the bodies no entry names any more, the
    /// handles renumbered, once it has doubled since it was last looked
    /// at; never while a checkpoint keeps the store's length for a
    /// [`State::rollback`]. Called between commands
    /// (`Run::command`), where no entry is held outside the table.
    pub(crate) fn compact_macros(&mut self) {
        if self.macros.len() < self.macros_at || self.undo.is_some() {
            return;
        }
        let mut new = vec![u32::MAX; self.macros.len()];
        self.each_entry(|e| {
            if e.kind == K_MACRO
                && let Some(n) = new.get_mut(e.payload as usize)
            {
                *n = 0;
            }
        });
        let mut kept = Vec::new();
        for (i, n) in new.iter_mut().enumerate() {
            if *n == 0 {
                *n = u32::try_from(kept.len()).unwrap_or(u32::MAX);
                kept.push(self.macros[i].clone());
            }
        }
        self.each_entry(|e| {
            if e.kind == K_MACRO
                && let Some(&n) = new.get(e.payload as usize)
            {
                e.payload = n;
            }
        });
        self.macros = kept;
        self.macros_at = (2 * self.macros.len()).max(MACROS_AT);
        // (a memo of decoded state stands only while nothing changed)
        self.any_gen = new_cat_gen();
    }

    /// Starts keeping the table's changes for a [`State::rollback`]
    /// (instead of a copy of the whole table): false if it keeps them
    /// already.
    pub(crate) fn checkpoint(&mut self) -> bool {
        if self.undo.is_some() {
            return false;
        }
        self.undo = Some(Box::new(Undo {
            slots: Vec::new(),
            logged: phitex_defuse::FxHashSet::default(),
            metas: Vec::new(),
            metas_logged: phitex_defuse::FxHashSet::default(),
            lens: [
                self.metas.len(),
                self.touched.len(),
                self.glues.len(),
                self.toks.len(),
                self.macros.len(),
                self.eq.len(),
                self.spill.len(),
            ],
            spill_added: Vec::new(),
            save: self.save.clone(),
            defined: self.defined.clone(),
            scope_extra: self.scope_extra.clone(),
            cont_locs: self.cont_locs.clone(),
            cands: self.cands.clone(),
            past: self.past.len(),
            stamp: self.stamp,
            pp_dirty: self.pp_dirty,
            next_gid: self.next_gid,
            cat_gen: self.cat_gen,
            any_gen: self.any_gen,
            font_gen: self.font_gen,
            muts: self.muts,
        }));
        true
    }

    /// The table as it stood at the [`State::checkpoint`].
    pub(crate) fn rollback(&mut self) {
        let Some(u) = self.undo.take() else {
            return;
        };
        let [metas, touched, glues, toks, macros, eq, spill] = u.lens;
        self.metas.truncate(metas);
        for (i, m) in u.metas {
            self.metas[i] = m;
        }
        self.touched.truncate(touched);
        self.glues.truncate(glues);
        self.toks.truncate(toks);
        self.macros.truncate(macros);
        for (loc, sl) in u.slots {
            if loc & SPILL == 0 {
                if let Some(x) = self.eq.get_mut(loc as usize) {
                    *x = sl;
                }
            } else if let Some(x) = self.spill.get_mut((loc & !SPILL) as usize) {
                *x = sl;
            }
        }
        // (slots made since: dropped, as the table did not have them)
        self.eq.truncate(eq);
        self.spill.truncate(spill);
        for f in u.spill_added {
            self.spill_ix.remove(&f);
        }
        let n = u32::try_from(spill).unwrap_or(u32::MAX);
        self.numbered.retain(|_, i| *i < n);
        // (a spill location dropped is another's when it is numbered again)
        self.read_across
            .get_mut()
            .retain(|e| e.0 & SPILL == 0 || e.0 & !SPILL < n);
        // (a font cell whose slot went with them: a checkpoint names each
        // of these by its field, and a location with no slot has none)
        self.font_slow.retain(|&l| l & SPILL == 0 || l & !SPILL < n);
        self.save = u.save;
        self.defined = u.defined;
        self.scope_extra = u.scope_extra;
        self.cont_locs = u.cont_locs;
        self.cands = u.cands;
        self.past.truncate(u.past);
        self.past_n = u32::try_from(self.past.len()).unwrap_or(u32::MAX);
        self.stamp = u.stamp;
        self.pp_dirty = u.pp_dirty;
        self.next_gid = u.next_gid;
        self.cat_gen = u.cat_gen;
        self.any_gen = u.any_gen;
        self.font_gen = u.font_gen;
        self.muts = u.muts;
    }

    /// The changes kept since a [`State::checkpoint`], taken.
    pub(crate) fn take_undo(&mut self) -> Option<Box<Undo>> {
        self.undo.take()
    }

    /// Changes kept since a checkpoint, put back ([`State::take_undo`]).
    pub(crate) fn set_undo(&mut self, u: Option<Box<Undo>>) {
        self.undo = u;
    }

    /// Notes slot `loc` before a change, while a checkpoint stands.
    #[cold]
    #[inline(never)]
    fn log_slot(&mut self, loc: Loc) {
        let Some(u) = self.undo.as_mut() else { return };
        if !u.logged.insert(loc) {
            return;
        }
        let old = if loc & SPILL == 0 {
            self.eq.get(loc as usize).cloned()
        } else {
            self.spill.get((loc & !SPILL) as usize).cloned()
        };
        u.slots.push((loc, old.unwrap_or_default()));
    }

    /// Metas entry `mi`, to change, noted first while a checkpoint
    /// stands.
    #[inline]
    fn meta_mut(&mut self, mi: usize) -> &mut Meta {
        if self.undo.is_some() {
            self.log_meta(mi);
        }
        &mut self.metas[mi]
    }

    /// Notes metas entry `mi` before a change, while a checkpoint stands.
    #[cold]
    #[inline(never)]
    fn log_meta(&mut self, mi: usize) {
        if let Some(u) = self.undo.as_mut()
            && mi < u.lens[0]
            && u.metas_logged.insert(mi)
        {
            u.metas.push((mi, self.metas[mi].clone()));
        }
    }

    // ------------------------------------------------------ candidates

    /// A page-builder candidate passed: the entries the locations
    /// defined after it hold here are kept ([`State::at_candidates`]).
    pub(crate) fn mark_candidate(&mut self) {
        self.cands.push((self.stamp, Vec::new()));
    }

    /// The run goes on past its candidate `k`, where a page's routine can
    /// fire and the text after it read what the routine set: reads from
    /// here on are looked at ([`State::note_read`]).
    pub(crate) fn mark_past(&mut self, k: u32) {
        self.past.push((self.stamp, k));
        self.past_n = u32::try_from(self.past.len()).unwrap_or(u32::MAX);
    }

    /// A read of the value `loc` holds. Between two candidates, one load
    /// and one comparison; once in each stretch the binding's age is looked
    /// at ([`State::note_read_across`]).
    #[inline]
    pub(crate) fn note_read(&self, loc: Loc) {
        if let Some(s) = self.slot_ref(loc)
            && s.read_at.get() != self.past_n
        {
            self.note_read_across(s, loc);
        }
    }

    /// [`State::note_read`], the first in its stretch: where the binding
    /// in force was made before a candidate passed, its value was read
    /// across those candidates.
    #[cold]
    #[inline(never)]
    fn note_read_across(&self, s: &Slot, loc: Loc) {
        // (a location the run has not touched goes to the engine, whose
        // reads are the run's own record, `Run::note_engine_reads`)
        if !s.touched {
            return;
        }
        let now = self.past_n;
        s.read_at.set(now);
        let ldef = usize::try_from(s.meta)
            .ok()
            .and_then(|m| m.checked_sub(1))
            .and_then(|m| self.metas.get(m))
            .map_or(0, |m| m.ldef_stamp);
        let mut from = u32::try_from(self.past.partition_point(|c| c.0 < ldef)).unwrap_or(u32::MAX);
        for f in &self.floors {
            if f.0 == loc {
                from = from.max(f.1);
            }
        }
        if from < now {
            let mut xs = self.read_across.borrow_mut();
            xs.push((loc, from, now));
            if xs.len() >= self.across_merge_at.get() {
                merge_across(&mut xs);
                self.across_merge_at.set(2 * xs.len() + ACROSS_MERGE);
            }
        }
    }

    /// A read of `loc` the run made from elsewhere than its table: as read
    /// across all the candidates it has gone past.
    pub(crate) fn note_read_elsewhere(&self, loc: Loc) {
        if self.past_n > 0 {
            self.read_across.borrow_mut().push((loc, 0, self.past_n));
        }
    }

    /// The location of spill field `f`, if the run has made one.
    pub(crate) fn spill_of(&self, f: Field) -> Option<Loc> {
        self.spill_ix.get(&f).map(|&i| SPILL | i)
    }

    /// [`State::read_across`]: `(loc, from, to)` by index in [`State::past`].
    pub(crate) fn reads_across(&self) -> Vec<(Loc, u32, u32)> {
        self.read_across.borrow().clone()
    }

    /// A routine's value put in force at the candidate just gone past
    /// ([`State::apply_global`]) counts as read across it.
    pub(crate) fn note_applied(&self, loc: Loc) {
        let now = self.past_n;
        if now > 0 {
            self.read_across.borrow_mut().push((loc, now - 1, now));
        }
    }

    /// How many candidates the run has gone past.
    pub(crate) fn past_n(&self) -> u32 {
        self.past_n
    }

    /// The numbers of the candidates the run has gone past, in order.
    pub(crate) fn past_ks(&self) -> Vec<u32> {
        self.past.iter().map(|p| p.1).collect()
    }

    /// Notes `loc`'s entry before a definition, where it is the first
    /// since the last candidate (its latest definition `ldef` before it).
    #[inline]
    fn note_cand(&mut self, loc: Loc, ldef: u64, prev: Entry) {
        let prev = self.stepped_entry(loc, prev);
        // (once per candidate: a location a loop's group restores and
        // the loop defines again was noted at every pass, a million
        // entries in xkcdcolors' `\foreach`, each written by every
        // checkpoint after)
        let k = u32::try_from(self.cands.len()).unwrap_or(u32::MAX);
        if self.cands.last().is_none_or(|c| ldef > c.0) || self.slot(loc).cand_noted == k {
            return;
        }
        self.slot(loc).cand_noted = k;
        if let Some((_, xs)) = self.cands.last_mut() {
            xs.push((loc, prev));
        }
    }

    /// `prev`, `loc`'s entry, as kept at a candidate: where the run holds
    /// steps of the register it has not read, marked with them
    /// ([`STEPPED`]), its value there what reached the run plus them.
    fn stepped_entry(&self, loc: Loc, prev: Entry) -> Entry {
        if prev.flags & (LOADED | STEPPED) != 0 {
            return prev;
        }
        match self.slot_ref(loc) {
            Some(s) if s.mapped != MAP_NONE && s.map.keep && s.map.add != 0 => {
                let mut e = UNLOADED;
                e.flags = STEPPED;
                e.chr = s.map.add;
                e
            }
            _ => prev,
        }
    }

    /// The values at the candidates of the locations defined after them:
    /// for each candidate passed, in order, each such location with its
    /// engine value there (`None`: not known here, the run had neither
    /// read nor defined it before). Kept by location: a location's value
    /// at candidate `i` is its first entry at a candidate from `i` on
    /// ([`AtCandidates::get`]), each entry encoded once.
    pub(crate) fn at_candidates(&self, v: &Values) -> AtCandidates {
        let mut by_loc: FxHashMap<Loc, Vec<AtCandidate>> = FxHashMap::default();
        for (i, (_, xs)) in self.cands.iter().enumerate() {
            for &(l, e) in xs {
                let h = by_loc.entry(l).or_default();
                // (the first entry at a candidate is the value there)
                if h.last().is_some_and(|x| x.0 == i) {
                    continue;
                }
                let x = if e.flags & LOADED != 0 {
                    AtVal::Known(self.encode(&e, v))
                } else if e.flags & STEPPED != 0 {
                    AtVal::Reaching(e.chr)
                } else {
                    AtVal::Reaching(0)
                };
                h.push((i, x));
            }
        }
        AtCandidates { by_loc }
    }

    // ----------------------------------------------------- generations

    /// The catcode generation: equal while no category code below 256
    /// changed.
    #[inline]
    pub(crate) fn cat_gen(&self) -> u64 {
        self.cat_gen
    }

    /// The table's mutation counter ([`State`]'s `muts`).
    #[inline]
    pub(crate) fn muts(&self) -> u64 {
        self.muts
    }

    /// The table's change generation ([`State`]'s `any_gen`).
    pub(crate) fn any_gen(&self) -> u64 {
        self.any_gen
    }

    /// The fonts' change generation ([`State`]'s `font_gen`).
    pub(crate) fn font_gen(&self) -> u64 {
        self.font_gen
    }

    /// Notes `loc` as a font's cell without a fixed location.
    pub(crate) fn note_font_loc(&mut self, loc: Loc) {
        self.font_slow.insert(loc);
    }

    /// A look at meaning `loc`'s value (a macro's body; its command code
    /// and modifier, which a tainted definition does not change, are no
    /// look): noted where it is tainted ([`State::taint`]).
    #[inline]
    fn look(&self, loc: Loc) {
        if !self.tainted.is_empty() && self.tainted.contains(&loc) {
            self.taint_read.set(true);
        }
    }

    /// Meaning `loc` was just defined by a body whose expansion read the
    /// lines before the run: a look at it from now on is noted
    /// ([`State::taint_read`]).
    pub(crate) fn taint(&mut self, loc: Loc) {
        if !self.tainted.contains(&loc) {
            self.tainted.push(loc);
        }
    }

    /// The tainted meanings ([`State::taint`]).
    pub(crate) fn tainted(&self) -> &[Loc] {
        &self.tainted
    }

    /// Whether a tainted meaning was looked at since it was tainted.
    pub(crate) fn taint_read(&self) -> bool {
        self.taint_read.get()
    }

    /// A change of `loc` may come: the generations it bumps.
    #[inline]
    fn changing(&mut self, loc: Loc) {
        self.any_gen = new_cat_gen();
        if (loc as usize).wrapping_sub(crate::fields::FONT0)
            < crate::fields::FONTS * crate::fields::FONT_W
            || (!self.font_slow.is_empty() && self.font_slow.contains(&loc))
        {
            self.font_gen = self.any_gen;
        }
        if is_cat(loc) {
            self.cat_gen = new_cat_gen();
        }
    }

    // ----------------------------------------------------------- slots

    #[inline]
    fn slot_ref(&self, loc: Loc) -> Option<&Slot> {
        if loc & SPILL == 0 {
            self.eq.get(loc as usize)
        } else {
            self.spill.get((loc & !SPILL) as usize)
        }
    }

    #[inline]
    fn slot(&mut self, loc: Loc) -> &mut Slot {
        if self.undo.is_some() {
            self.log_slot(loc);
        }
        let s = if loc & SPILL == 0 {
            let i = loc as usize;
            if i >= self.eq.len() {
                self.eq.resize_with(i + 1 + 1024, Slot::default);
            }
            &mut self.eq[i]
        } else {
            let i = (loc & !SPILL) as usize;
            if i >= self.spill.len() {
                self.spill.resize_with(i + 1, Slot::default);
            }
            &mut self.spill[i]
        };
        if !s.touched {
            s.touched = true;
            self.touched.push(loc);
        }
        s
    }

    /// The spill locations of `prev` (the state a continuation's
    /// locations were computed in, [`State::from_cont`]): a field past
    /// the static cells keeps its location, and its slot its field.
    pub(crate) fn adopt_spill(&mut self, prev: &State) {
        self.spill_ix.clone_from(&prev.spill_ix);
        self.numbered.clone_from(&prev.numbered);
        let fields: Vec<(Field, u32)> = self.spill_ix.iter().map(|(&f, &i)| (f, i)).collect();
        for (f, i) in fields {
            self.slot(SPILL | i).field = Some(f);
        }
    }

    /// The spill location of the numbered field `<prefix>:<n>` (a
    /// register or a code past the fixed ones,
    /// [`crate::fields::Fields::numbered`]), if it has one already: one
    /// lookup, not the field's name made, interned and looked up.
    #[inline]
    pub(crate) fn numbered(&self, prefix: &'static str, n: i64) -> Option<Loc> {
        self.numbered
            .get(&(prefix.as_ptr() as usize, n))
            .map(|&i| SPILL | i)
    }

    /// [`State::spill_loc`] of `f`, the numbered field `<prefix>:<n>`
    /// ([`State::numbered`]).
    pub(crate) fn add_numbered(&mut self, prefix: &'static str, n: i64, f: Field) -> Loc {
        let l = self.spill_loc(f);
        self.numbered
            .insert((prefix.as_ptr() as usize, n), l & !SPILL);
        l
    }

    /// The spill location of field `f`.
    pub(crate) fn spill_loc(&mut self, f: Field) -> Loc {
        if let Some(&i) = self.spill_ix.get(&f) {
            return SPILL | i;
        }
        let i = u32::try_from(self.spill.len()).unwrap_or(0);
        self.spill_ix.insert(f, i);
        if let Some(u) = self.undo.as_mut() {
            u.spill_added.push(f);
        }
        let l = SPILL | i;
        self.slot(l).field = Some(f);
        l
    }

    /// The field of `loc`, if known here.
    #[inline]
    pub(crate) fn field(&self, loc: Loc) -> Option<Field> {
        self.slot_ref(loc).and_then(|s| s.field)
    }

    pub(crate) fn set_field(&mut self, loc: Loc, f: Field) {
        self.slot(loc).field = Some(f);
    }

    /// The command code and modifier of `loc` when it is loaded and
    /// decoded (one slot access, one flag test); `None` otherwise.
    #[inline]
    pub(crate) fn cmd_chr_fast(&self, loc: Loc) -> Option<(u8, i32)> {
        // (a meaning's location is never a spill one: one bounds test)
        let s = self.eq.get(loc as usize)?;
        if s.e.flags & (LOADED | CC) != (LOADED | CC) {
            return None;
        }
        if s.read_at.get() != self.past_n {
            self.note_read_across(s, loc);
        }
        Some((s.e.cmd, s.e.chr))
    }

    #[inline]
    pub(crate) fn is_loaded(&self, loc: Loc) -> bool {
        self.slot_ref(loc).is_some_and(|s| s.e.flags & LOADED != 0)
    }

    /// Whether `loc` is loaded (1), loaded holding an alias definition's
    /// value its use resolves first (2, [`State::resolve_alias`]), or
    /// not loaded (0).
    #[inline]
    pub(crate) fn load_state(&self, loc: Loc) -> u8 {
        match self
            .slot_ref(loc)
            .map_or(0, |s| s.e.flags & (LOADED | ALIAS))
        {
            0 | ALIAS => 0,
            LOADED => 1,
            _ => 2,
        }
    }

    /// The field and piece of the alias definition `loc` holds, if it
    /// holds one unresolved ([`Entry::alias`]).
    /// With the integer the alias adds to that value (a relation the
    /// engine stacked on an alias, `phitex_defuse::alias_parts`).
    pub(crate) fn alias_at(
        &self,
        loc: Loc,
        v: &Values,
    ) -> Option<(Field, phitex_defuse::NodeId, i64)> {
        let s = self.slot_ref(loc)?;
        if s.e.flags & ALIAS == 0 {
            return None;
        }
        phitex_defuse::alias_parts(v, s.e.id?)
    }

    /// `loc`'s alias definition resolved to `x` where the run uses it
    /// (`Run::load`): the slot holds the integer, the binding it
    /// publishes stays the alias.
    pub(crate) fn resolve_alias(&mut self, loc: Loc, x: i32) {
        self.muts += 1;
        let e = &mut self.slot(loc).e;
        e.kind = K_INT;
        e.chr = x;
        e.id = None;
        e.flags &= !(HAS_ID | ALIAS);
    }

    /// What the run holds for `loc` (spec §2.6).
    pub(crate) fn held(&self, loc: Loc) -> Held {
        let Some(s) = self.slot_ref(loc) else {
            return Held::Absent;
        };
        if s.meta != 0 {
            return Held::Written;
        }
        if s.e.flags & LOADED == 0 {
            return if s.sym { Held::Symbol } else { Held::Absent };
        }
        if s.e.flags & PRELOADED != 0 {
            return Held::Loaded(s.e.id);
        }
        Held::Used(self.engine_value(loc).flatten())
    }

    // ------------------------------------------------------------ reads

    /// The item's value for use (spec §2.2): read from the engine at the
    /// run's position, the read recorded with the value the run uses
    /// (`Ctx::read`). An integer register the run stepped before it read
    /// it ([`State::add`]) holds the value read with the steps applied:
    /// a write of the run, its read the value read. Caller: `Run::load`,
    /// after its own sources (a finisher's preloaded state, a
    /// job-written line's state at its read-back point).
    pub(crate) fn get(
        &mut self,
        cx: &mut Ctx<'_>,
        v: &Values,
        loc: Loc,
        f: Field,
    ) -> Result<(), phitex_defuse::Stop> {
        let x = cx.read(f)?;
        self.load_read(v, loc, x);
        Ok(())
    }

    pub(crate) fn load_read(&mut self, v: &Values, loc: Loc, x: Option<ValueId>) {
        let s = self.slot(loc);
        let mut e = Entry::raw(x);
        e.flags = LOADED | HAS_ID;
        s.e = e;
        if s.mapped == MAP_NONE {
            return;
        }
        // (the steps over the value read: the run holds their result and
        // publishes them as before, a relation)
        s.base = x;
        let y = s.map.apply(crate::val::i32_of(v, x));
        let mut e = Entry::int(y);
        e.flags = LOADED;
        s.e = e;
    }

    /// The run's definitions of `loc` taken back: made at the base level
    /// (no group of the run saved or bound it; the caller knows no
    /// candidate passed since the first, `Run::reg_first_def`). The
    /// location is then unread (a read of it reads the engine's value
    /// again) and the run publishes nothing of it: a register put back
    /// to the value reaching the run (`crate::defer::restore_reaching`).
    /// False, nothing changed, otherwise.
    pub(crate) fn undefine_fresh(&mut self, loc: Loc) -> bool {
        let Some(mi) = self
            .slot_ref(loc)
            .filter(|s| s.save_gid == 0)
            .and_then(|s| usize::try_from(s.meta).ok())
            .filter(|&m| m > 0)
            .map(|m| m - 1)
        else {
            return false;
        };
        let m = &self.metas[mi];
        if m.publ.is_none() || m.ldef_gid.is_some() {
            return false;
        }
        self.changing(loc);
        self.muts += 1;
        let e = &mut self.slot(loc).e;
        e.flags = 0;
        e.id = None;
        let m = self.meta_mut(mi);
        m.publ = None;
        m.ldef_stamp = 0;
        m.ldef_local = false;
        m.ldef_gid = None;
        true
    }

    /// Whether the binding of `loc` in force is one this run made.
    pub(crate) fn defined_here(&self, loc: Loc) -> bool {
        self.slot_ref(loc)
            .and_then(|s| usize::try_from(s.meta).ok())
            .filter(|&m| m > 0)
            .and_then(|m| self.metas.get(m - 1))
            .is_some_and(|m| m.ldef_stamp > 0)
    }

    /// A value read in place of the engine's (a finisher's state at its
    /// candidate, a job-written line's at its read-back point): loaded,
    /// no read recorded, never compared with the engine's at the end.
    pub(crate) fn preload(&mut self, v: &Values, loc: Loc, x: Option<ValueId>) {
        self.changing(loc);
        self.muts += 1;
        let mut e = Entry::raw_seen(v, x);
        e.flags |= LOADED | PRELOADED;
        let s = self.slot(loc);
        s.e = e;
        s.lazy = None;
    }

    /// A value given for `loc` where the run first uses it, read then
    /// (`Run::fin_lazy_value`); `None` takes it back.
    pub(crate) fn set_lazy(&mut self, loc: Loc, x: Option<ValueId>) {
        if x.is_none() && self.slot_ref(loc).is_none_or(|s| s.lazy.is_none()) {
            return;
        }
        self.muts += 1;
        self.slot(loc).lazy = x;
    }

    /// The value given for `loc` where the run first uses it, if any.
    #[inline]
    pub(crate) fn lazy(&self, loc: Loc) -> Option<ValueId> {
        self.slot_ref(loc).and_then(|s| s.lazy)
    }

    /// A symbol of page-layer item `loc` made in this run (spec §2.2):
    /// where the item is neither loaded, written nor stepped by the run,
    /// the symbol stands for the value reaching the run (`crate::defer`),
    /// and the slot says so. Callers: `\the` and `\number` on the register
    /// alone in a piece, after `defer::deferrable` said it is a page-layer
    /// one.
    pub(crate) fn note_symbol(&mut self, loc: Loc) {
        let s = self.slot_ref(loc);
        if s.is_some_and(|s| {
            s.e.flags & LOADED != 0 || s.meta != 0 || s.mapped != MAP_NONE || s.lazy.is_some()
        }) {
            return;
        }
        self.slot(loc).sym = true;
    }

    /// The locations the run defined, from its `from`th on, and how many
    /// it defined (a checkpoint's undo may have taken some back since a
    /// caller looked: then fewer), with the bindings a continuation
    /// brought where `from` is 0 ([`crate::defer::binds_like`]).
    pub(crate) fn defined_locs_from(&self, from: usize) -> (Vec<Loc>, usize) {
        let n = self.metas.len();
        let mut ls: Vec<Loc> = self.metas[from.min(n)..].iter().map(|m| m.loc).collect();
        if from == 0 {
            ls.extend_from_slice(&self.cont_locs);
        }
        (ls, n)
    }

    /// The steps the run holds of register `loc`, unread: what a symbol
    /// made now adds to the value reaching the run.
    pub(crate) fn steps_of(&self, loc: Loc) -> i32 {
        self.slot_ref(loc)
            .filter(|s| s.mapped != MAP_NONE)
            .map_or(0, |s| s.map.add)
    }

    // ---------------------------------------------------------- values

    /// The engine value of entry `e` (`None`: absent), encoded from its
    /// record and the table's stores.
    pub(crate) fn encode(&self, e: &Entry, v: &Values) -> Option<ValueId> {
        if e.flags & HAS_ID != 0 || e.kind == K_RAW {
            return e.id;
        }
        match e.kind {
            K_ABSENT => None,
            K_INT => Some(crate::val::int_id(v, i64::from(e.chr))),
            K_GLUE => Some(crate::val::glue_value(v, &self.glues[e.payload as usize])),
            K_TOKS => Some(crate::val::toks_value(v, &self.toks[e.payload as usize])),
            _ => Some(self.mean_of_entry(e).encode(v)),
        }
    }

    /// [`State::encode`], a macro's or a token list's value taken from
    /// `memo` where the same body was encoded before.
    fn encode_memo(&self, e: &Entry, v: &Values, memo: &mut CkMemo) -> Option<ValueId> {
        if e.flags & HAS_ID != 0 || e.kind == K_RAW {
            return e.id;
        }
        match e.kind {
            K_MACRO => {
                let m = &self.macros[e.payload as usize];
                let k = Rc::as_ptr(m) as usize;
                if let Some(&(_, x)) = memo.macros.get(&k) {
                    return Some(x);
                }
                let x = Mean::Macro(m.clone()).encode(v);
                memo.macros.insert(k, (m.clone(), x));
                Some(x)
            }
            K_TOKS => {
                let t = &self.toks[e.payload as usize];
                let k = Rc::as_ptr(t).cast::<Tok>() as usize;
                if let Some(&(_, x)) = memo.toks.get(&k) {
                    return Some(x);
                }
                let x = crate::val::toks_value(v, t);
                memo.toks.insert(k, (t.clone(), x));
                Some(x)
            }
            _ => self.encode(e, v),
        }
    }

    /// The meaning a meaning entry holds (a macro's body shared).
    fn mean_of_entry(&self, e: &Entry) -> Mean {
        match e.kind {
            K_PRIM => Mean::Prim(crate::val::prim_by_handle(e.payload), e.cmd, e.chr),
            K_MACRO => Mean::Macro(self.macros[e.payload as usize].clone()),
            K_CHAR => Mean::Char(e.cmd, e.chr.cast_unsigned()),
            K_FONT => Mean::Font(i64::from(e.chr)),
            K_CHARDEF => Mean::CharDef(i64::from(e.chr)),
            // (payload 1: XeTeX's `\\Umathchardef`, its code's 32 bits
            // flagged past them, `crate::xetex::U_MATH`)
            K_MATHCHARDEF if e.payload == 1 => {
                Mean::MathCharDef(crate::xetex::U_MATH | i64::from(e.chr.cast_unsigned()))
            }
            K_MATHCHARDEF => Mean::MathCharDef(i64::from(e.chr)),
            K_REG => Mean::Reg(e.cmd, e.chr),
            K_NOTEXP => Mean::NotExpanded,
            _ => Mean::Undefined,
        }
    }

    /// The entry of glue `g` (kept in the table's glue store).
    pub(crate) fn glue_entry(&mut self, g: Glue) -> Entry {
        self.glues.push(g);
        Entry {
            kind: K_GLUE,
            flags: 0,
            cmd: 0,
            chr: 0,
            payload: u32::try_from(self.glues.len() - 1).unwrap_or(0),
            id: None,
        }
    }

    /// The entry of meaning `m`: its command code and modifier, its
    /// macro kept in the table's macro store by handle.
    pub(crate) fn mean_entry(&mut self, m: Mean) -> Entry {
        let (cmd, chr) = m.cmd_chr();
        let (kind, payload) = match m {
            Mean::Prim(n, _, _) => (K_PRIM, crate::val::prim_handle(n)),
            Mean::Macro(x) => {
                self.macros.push(x);
                (K_MACRO, u32::try_from(self.macros.len() - 1).unwrap_or(0))
            }
            Mean::Char(..) => (K_CHAR, 0),
            Mean::Undefined => (K_UNDEF, 0),
            Mean::Font(_) => (K_FONT, 0),
            Mean::CharDef(_) => (K_CHARDEF, 0),
            Mean::MathCharDef(n) => (K_MATHCHARDEF, u32::from(n >= crate::xetex::U_MATH)),
            Mean::Reg(..) => (K_REG, 0),
            Mean::NotExpanded => (K_NOTEXP, 0),
        };
        Entry {
            kind,
            flags: CC,
            cmd,
            chr,
            payload,
            id: None,
        }
    }

    /// The loaded entry of `loc` (call [`State::is_loaded`] first).
    #[inline]
    fn entry_mut(&mut self, loc: Loc) -> &mut Entry {
        if self.undo.is_some() {
            self.log_slot(loc);
        }
        let s = if loc & SPILL == 0 {
            &mut self.eq[loc as usize]
        } else {
            &mut self.spill[(loc & !SPILL) as usize]
        };
        &mut s.e
    }

    /// The engine's value of `loc` as the run read it, before any
    /// definition of its own (`None`: not known here).
    #[allow(clippy::option_option)]
    pub(crate) fn engine_value(&self, loc: Loc) -> Option<Option<ValueId>> {
        let s = self.slot_ref(loc)?;
        if s.meta != 0 {
            return self.metas.get(s.meta as usize - 1).and_then(|m| m.eng);
        }
        if s.mapped != MAP_NONE {
            return (s.e.flags & LOADED != 0).then_some(s.base);
        }
        (s.e.flags & (LOADED | HAS_ID | PRELOADED) == LOADED | HAS_ID).then_some(s.e.id)
    }

    /// The engine value of loaded `loc` (encoded once).
    pub(crate) fn id_of(&mut self, loc: Loc, v: &Values) -> Option<ValueId> {
        // (known already: read, nothing changed)
        if let Some(s) = self.slot_ref(loc)
            && s.e.flags & HAS_ID != 0
        {
            return s.e.id;
        }
        let e = *self.entry_mut(loc);
        if e.flags & HAS_ID != 0 {
            return e.id;
        }
        let x = self.encode(&e, v);
        let e = self.entry_mut(loc);
        e.id = x;
        e.flags |= HAS_ID;
        x
    }

    /// Loaded `loc` as an integer (decoded once).
    #[inline]
    pub(crate) fn int_of(&mut self, loc: Loc, v: &Values) -> i32 {
        if let Some(s) = self.slot_ref(loc)
            && s.e.kind == K_INT
        {
            return s.e.chr;
        }
        let e = self.entry_mut(loc);
        match e.kind {
            K_INT => e.chr,
            K_ABSENT => 0,
            K_RAW => {
                let i = crate::val::i32_of(v, e.id);
                e.kind = K_INT;
                e.chr = i;
                i
            }
            _ => {
                let x = self.id_of(loc, v);
                crate::val::i32_of(v, x)
            }
        }
    }

    /// Loaded `loc` as glue (decoded once).
    pub(crate) fn glue_of(&mut self, loc: Loc, v: &Values) -> Glue {
        let e = *self.entry_mut(loc);
        match e.kind {
            K_GLUE => self.glues[e.payload as usize],
            K_ABSENT => Glue::ZERO,
            K_RAW => {
                let g = crate::val::glue_of(v, e.id);
                self.glues.push(g);
                let h = u32::try_from(self.glues.len() - 1).unwrap_or(0);
                let e = self.entry_mut(loc);
                e.kind = K_GLUE;
                e.payload = h;
                g
            }
            _ => {
                let x = self.id_of(loc, v);
                crate::val::glue_of(v, x)
            }
        }
    }

    /// The macro in loaded `loc`, when it holds one already decoded.
    #[inline]
    pub(crate) fn macro_at(&self, loc: Loc) -> Option<Rc<crate::val::Macro>> {
        self.look(loc);
        self.note_read(loc);
        let s = self.slot_ref(loc)?;
        if s.e.flags & LOADED == 0 || s.e.kind != K_MACRO {
            return None;
        }
        self.macros.get(s.e.payload as usize).cloned()
    }

    /// Decodes loaded meaning `loc` in place when it is the engine's
    /// value as read: its command code, modifier and handle set (an
    /// undefined name keeps its absence: it encodes as none).
    fn decode_mean_at(&mut self, loc: Loc, v: &Values) {
        let e = *self.entry_mut(loc);
        match e.kind {
            K_ABSENT => {
                let (c, h) = Mean::Undefined.cmd_chr();
                let e = self.entry_mut(loc);
                e.cmd = c;
                e.chr = h;
                e.flags |= CC;
            }
            K_RAW => {
                let mut n = self.mean_entry(Mean::decode(v, e.id));
                n.flags = e.flags | CC;
                n.id = e.id;
                *self.entry_mut(loc) = n;
            }
            _ => {}
        }
    }

    /// Loaded `loc` as a meaning (decoded once).
    #[inline]
    pub(crate) fn mean_of(&mut self, loc: Loc, v: &Values) -> Mean {
        self.look(loc);
        self.note_read(loc);
        let e = *self.entry_mut(loc);
        match e.kind {
            K_ABSENT => Mean::Undefined,
            K_RAW => {
                self.decode_mean_at(loc, v);
                let e = *self.entry_mut(loc);
                self.mean_of_entry(&e)
            }
            k if k >= K_MEAN0 => self.mean_of_entry(&e),
            _ => {
                let x = self.id_of(loc, v);
                Mean::decode(v, x)
            }
        }
    }

    /// Whether the entry is a macro's.
    pub(crate) fn entry_is_macro(e: &Entry) -> bool {
        e.kind == K_MACRO
    }

    /// Loaded meaning `loc`'s entry where it is decoded: what `\let`
    /// copies (§1221), the macro's handle shared.
    #[inline]
    pub(crate) fn meaning_entry(&self, loc: Loc) -> Option<Entry> {
        let s = self.eq.get(loc as usize)?;
        if s.e.flags & LOADED == 0 || s.e.kind < K_MEAN0 {
            return None;
        }
        self.look(loc);
        self.note_read(loc);
        Some(s.e)
    }

    /// Whether loaded meanings `a` and `b`, both macros, are the same
    /// macro for `\ifx` (§507: the same parameters and body), read in
    /// place. `None` where either is no decoded macro.
    pub(crate) fn same_macro(&self, a: Loc, b: Loc) -> Option<bool> {
        let m = |l: Loc| {
            let s = self.eq.get(l as usize)?;
            self.note_read(l);
            (s.e.flags & LOADED != 0 && s.e.kind == K_MACRO).then_some(s.e.payload as usize)
        };
        let (pa, pb) = (m(a)?, m(b)?);
        if pa == pb {
            return Some(true);
        }
        let (x, y) = (self.macros.get(pa)?, self.macros.get(pb)?);
        Some(Rc::ptr_eq(x, y) || (x.params == y.params && x.body == y.body))
    }

    /// The command code and modifier of loaded meaning `loc` (decoded
    /// once; nothing cloned).
    #[inline]
    pub(crate) fn cmd_chr_of(&mut self, loc: Loc, v: &Values) -> (u8, i32) {
        self.note_read(loc);
        let e = *self.entry_mut(loc);
        if e.flags & CC != 0 {
            return (e.cmd, e.chr);
        }
        if e.kind == K_RAW || e.kind == K_ABSENT {
            self.decode_mean_at(loc, v);
            let e = *self.entry_mut(loc);
            if e.flags & CC != 0 {
                return (e.cmd, e.chr);
            }
        }
        self.mean_of(loc, v).cmd_chr()
    }

    /// Loaded `loc` as a token list (decoded once).
    #[allow(dead_code)]
    pub(crate) fn toks_of(&mut self, loc: Loc, v: &Values) -> Rc<[Tok]> {
        let e = *self.entry_mut(loc);
        match e.kind {
            K_TOKS => self.toks[e.payload as usize].clone(),
            K_ABSENT => Rc::from([]),
            K_RAW => {
                let t = crate::val::toks_rc(v, e.id);
                self.toks.push(t.clone());
                let h = u32::try_from(self.toks.len() - 1).unwrap_or(0);
                let e = self.entry_mut(loc);
                e.kind = K_TOKS;
                e.payload = h;
                t
            }
            _ => {
                let x = self.id_of(loc, v);
                crate::val::toks_rc(v, x)
            }
        }
    }

    // ---------------------------------------------------------- writes

    /// A new group of the run: its number, and where its saves and
    /// definitions begin, and its stamp.
    pub(crate) fn open(&mut self) -> (u32, usize, usize, u64) {
        self.next_gid += 1;
        (
            self.next_gid,
            self.save.len(),
            self.defined.len(),
            self.stamp,
        )
    }

    /// The meta of `loc`, made on its first definition: what the run
    /// read of it before (its engine value, where it read one).
    fn meta(&mut self, loc: Loc) -> usize {
        let m = self.slot(loc).meta;
        if m != 0 {
            return m as usize - 1;
        }
        let eng = self.engine_value(loc);
        let i = self.metas.len();
        self.slot(loc).meta = u32::try_from(i + 1).unwrap_or(0);
        self.metas.push(Meta {
            loc,
            eng,
            publ: None,
            ldef_stamp: 0,
            ldef_local: false,
            ldef_gid: None,
        });
        i
    }

    /// Whether a run of code entries can wait ([`BulkCodes`]): none waits
    /// already, and no undo log or candidate's entries keep each write.
    pub(crate) fn bulk_ok(&self) -> bool {
        self.bulk.is_none() && self.undo.is_none() && self.cands.is_empty()
    }

    /// Whether entries wait ([`BulkCodes`]).
    #[inline]
    pub(crate) fn bulk_waits(&self) -> bool {
        self.bulk.is_some()
    }

    /// Entries `lo..hi` of code table `table` each set locally in group
    /// `gid` to the character itself, to be written when asked for.
    pub(crate) fn bulk_codes(&mut self, table: usize, lo: i32, hi: i32, gid: u32) {
        self.any_gen = new_cat_gen();
        self.muts += 1;
        self.stamp += 1;
        self.bulk = Some(Box::new(BulkCodes {
            table,
            lo,
            hi,
            gid,
            save_at: self.save.len(),
            done: phitex_defuse::FxHashSet::default(),
        }));
    }

    /// Entry `c` of code table `table` is asked for, at `loc`: written now if
    /// it waits.
    #[inline]
    pub(crate) fn bulk_take(&mut self, table: usize, c: i32, loc: Loc) {
        let Some(b) = self.bulk.as_mut() else {
            return;
        };
        if b.table == table && (b.lo..b.hi).contains(&c) && b.done.insert(c) {
            let gid = b.gid;
            self.set(loc, Entry::int(c), false, Some(gid));
        }
    }

    /// All that waits taken off, to be written: table, the entries not
    /// written yet, their group.
    pub(crate) fn bulk_rest(&mut self) -> Option<(usize, Vec<i32>, u32)> {
        let b = self.bulk.take()?;
        let rest = (b.lo..b.hi).filter(|c| !b.done.contains(c)).collect();
        Some((b.table, rest, b.gid))
    }

    /// A write (spec §2.3; §277–§279: `eq_define` / `geq_define`): `grp`
    /// is the innermost group of the run open now, if any. A local write
    /// inside it saves the value it hides, once per group while that
    /// value is the group's own (§277, §283: after a global write the
    /// next local one saves again); a global one makes the saves before
    /// it inert. A write outside every group of the run, or a global
    /// one, is the run's binding at its base level, which
    /// [`State::publication`] publishes by the one rule.
    pub(crate) fn set(&mut self, loc: Loc, val: Entry, global: bool, grp: Option<u32>) {
        self.changing(loc);
        self.muts += 1;
        let mi = self.meta(loc);
        // (its entry at the candidate before, taken before a set below
        // replaces the steps, [`State::stepped_entry`])
        let cand_prev = (!self.cands.is_empty()).then(|| {
            let e = self.slot(loc).e;
            self.stepped_entry(loc, e)
        });
        // (a register only stepped so far: its steps are no binding once
        // it is set at the run's base level or globally, the value
        // replaces them; a local write in a group keeps them, the run's
        // base binding, which the group's close restores to and the run
        // publishes (§283: `\global\advance\c@page\@ne` then
        // `{\advance\c@page\m@ne}` in an output routine, mparhack's))
        if let Some(s) = self.slot_ref(loc)
            && s.mapped != MAP_NONE
        {
            let unread = s.e.flags & LOADED == 0;
            let s = self.slot(loc);
            if global || grp.is_none() {
                s.map = Map::ID;
                s.mapped = MAP_NONE;
                s.base = None;
                s.base_unread = false;
            } else if unread {
                s.base_unread = true;
            }
        }
        self.stamp += 1;
        let st = self.stamp;
        let ldef = {
            let m = &self.metas[mi];
            (m.ldef_stamp, m.ldef_local, m.ldef_gid)
        };
        if let Some(prev) = cand_prev {
            self.note_cand(loc, ldef.0, prev);
        }
        let s = self.slot(loc);
        let lazy = s.lazy.take();
        let mut saved = None;
        if global {
            s.ginert = st;
        } else if let Some(g) = grp
            && !(s.save_gid == g && s.ginert <= s.save_stamp)
        {
            saved = Some(Saved {
                loc,
                prev: s.e,
                gid: s.save_gid,
                stamp: s.save_stamp,
                at: st,
                ldef,
                lazy,
            });
            s.save_gid = g;
            s.save_stamp = st;
        }
        s.e = stored(val);
        if let Some(x) = saved {
            self.save.push(x);
        }
        // (once per group: a loop redefining a macro in a group open
        // throughout listed it at every pass, 1.6 M entries in
        // xkcdcolors' `\foreach`, and every checkpoint wrote the list)
        if !global && let Some(g) = grp {
            let s = self.slot(loc);
            if s.def_gid != g {
                s.def_gid = g;
                self.defined.push(loc);
            }
        }
        let m = self.meta_mut(mi);
        m.ldef_stamp = st;
        m.ldef_local = !global;
        m.ldef_gid = if global { None } else { grp };
        if global {
            m.publ = Some((val, Locality::Global));
        } else if grp.is_none() {
            m.publ = Some((val, Locality::Local));
        }
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
    }

    pub(crate) fn add(&mut self, v: &Values, loc: Loc, c: i32, global: bool) -> bool {
        let g = if global { MAP_GLOBAL } else { MAP_LOCAL };
        if self.slot_ref(loc).is_some_and(|s| {
            s.e.flags & PRELOADED != 0
                || s.meta != 0
                || s.lazy.is_some()
                || (s.mapped != MAP_NONE && s.mapped != g)
        }) {
            return false;
        }
        self.changing(loc);
        self.muts += 1;
        // (after a candidate: what it holds there is kept, unread the value
        // that reached the run, for the routine of a page firing there,
        // `State::at_candidates`; the stamp says it came after the candidate,
        // `State::defined_stamped`)
        if !self.cands.is_empty() {
            self.stamp += 1;
            let prev = self.slot(loc).e;
            self.note_cand(loc, 0, prev);
        }
        let loaded = self.is_loaded(loc);
        let cur = if loaded {
            Some(self.int_of(loc, v))
        } else {
            None
        };
        let s = self.slot(loc);
        if loaded && s.mapped == MAP_NONE {
            s.base = s.e.id;
        }
        s.map = s.map.then(Map::add(c));
        s.mapped = g;
        if let Some(x) = cur {
            let mut e = Entry::int(x.wrapping_add(c));
            e.flags = LOADED;
            s.e = e;
        }
        // (a step uses the value it steps)
        self.note_read(loc);
        true
    }

    /// A restore of `loc` to its own symbol (spec §2.3 rule 1,
    /// `crate::defer::try_symbolic`): the run's steps of the register made
    /// the identity, nothing read and nothing published. A register the
    /// run read and holds unwritten holds the value it read already.
    pub(crate) fn restore(&mut self, loc: Loc) {
        if self.slot_ref(loc).is_some_and(|s| s.mapped != MAP_NONE) {
            self.changing(loc);
            let s = self.slot(loc);
            s.map = Map::ID;
            s.mapped = MAP_NONE;
            if s.e.flags & LOADED != 0 {
                let mut e = Entry::raw(s.base);
                e.flags = LOADED | HAS_ID;
                s.e = e;
            }
            s.base = None;
            s.base_unread = false;
        }
        self.muts += 1;
    }

    /// A set of `loc` to its own symbol plus `j` (`crate::defer::try_symbolic`,
    /// docs/spec-relative-counters.md §2.5): the run's map of the register
    /// made `+j` whatever it held, nothing read and nothing published; a
    /// register the run loaded holds the value it loaded plus `j`. False,
    /// nothing changed, where [`State::add`] would refuse a step.
    pub(crate) fn restore_plus(&mut self, v: &Values, loc: Loc, j: i32, global: bool) -> bool {
        let g = if global { MAP_GLOBAL } else { MAP_LOCAL };
        if self.slot_ref(loc).is_some_and(|s| {
            s.e.flags & PRELOADED != 0
                || s.meta != 0
                || s.lazy.is_some()
                || (s.mapped != MAP_NONE && s.mapped != g)
        }) {
            return false;
        }
        self.restore(loc);
        self.add(v, loc, j, global)
    }

    /// The integer registers whose steps the run publishes beneath a
    /// scope's value of its own ([`State::steps_under_scope`]) and has not
    /// read: the run reads them ([`State::set_step_base`]) before it
    /// publishes the steps as the value they make.
    pub(crate) fn unread_steps_under_scope(&self) -> Vec<Loc> {
        self.touched
            .iter()
            .copied()
            .filter(|&l| {
                self.slot_ref(l)
                    .is_some_and(|s| s.base_unread && s.mapped != MAP_NONE && s.map.keep)
                    && self.steps_under_scope(l)
            })
            .collect()
    }

    /// The value the run reads of register `loc` it stepped before a
    /// local write in a group ([`State::unread_steps_under_scope`]).
    pub(crate) fn set_step_base(&mut self, loc: Loc, x: Option<ValueId>) {
        self.changing(loc);
        self.muts += 1;
        let s = self.slot(loc);
        s.base = x;
        s.base_unread = false;
    }

    /// Whether register `loc`'s steps lie beneath a value of its own in
    /// a scope the run opens after itself (a group's local write left
    /// open, [`State::abandon_to_scope`], [`State::abandon_to_frames`]):
    /// one field holds a relation or values, not both, so the steps are
    /// published as the value they make ([`State::publication`]).
    fn steps_under_scope(&self, loc: Loc) -> bool {
        let scoped = |l: &Locality| matches!(l, Locality::Scope | Locality::ScopeAt(_));
        self.slot_ref(loc)
            .and_then(|s| (s.meta as usize).checked_sub(1))
            .and_then(|mi| self.metas.get(mi))
            .is_some_and(|m| m.publ.as_ref().is_some_and(|(_, l)| scoped(l)))
            || self.scope_extra.iter().any(|e| e.0 == loc && scoped(&e.2))
    }

    /// Whether the run holds steps of register `loc` that change it
    /// ([`State::add`]).
    pub(crate) fn has_map(&self, loc: Loc) -> bool {
        self.slot_ref(loc)
            .is_some_and(|s| s.mapped != MAP_NONE && s.map != Map::ID)
    }

    /// Whether the table a continuation brought ([`State::from_cont`])
    /// holds a page parameter's binding local to a group open at the cut
    /// (multicol's `\\vsize` in its environment): a binding no field has.
    pub(crate) fn carries_page_param(&self) -> bool {
        self.cont_locs.iter().any(|&l| {
            is_page_param(l)
                && self
                    .slot_ref(l)
                    .and_then(|s| usize::try_from(s.meta).ok().filter(|&m| m > 0))
                    .and_then(|m| self.metas.get(m - 1))
                    .is_some_and(|m| m.ldef_local && m.ldef_gid.is_some())
        })
    }

    /// Whether a page parameter changed since the last call
    /// ([`State::set`], [`State::close`]).
    pub(crate) fn take_page_params(&mut self) -> bool {
        std::mem::take(&mut self.pp_dirty)
    }

    /// A binding changed in place (§1079, §1247), nothing saved:
    /// `base` when the binding is the run's base level's (then it is
    /// the run's definition there, `global` at level one), else a
    /// group's of the run.
    pub(crate) fn modify(&mut self, loc: Loc, val: Entry, base: bool, global: bool) {
        self.changing(loc);
        self.muts += 1;
        let mi = self.meta(loc);
        if !self.cands.is_empty() {
            let prev = self.slot(loc).e;
            let ldef = self.metas[mi].ldef_stamp;
            self.note_cand(loc, ldef, prev);
        }
        self.stamp += 1;
        let st = self.stamp;
        self.slot(loc).e = stored(val);
        if !base {
            self.defined.push(loc);
        }
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
        // (a group's binding changed in place stays that group's: its
        // local definition, by the group that saved it when the last
        // definition does not say)
        let save_gid = self.slot_ref(loc).map_or(0, |s| s.save_gid);
        let m = self.meta_mut(mi);
        m.ldef_stamp = st;
        m.ldef_local = true;
        if base {
            m.ldef_gid = None;
        } else if m.ldef_gid.is_none() && save_gid != 0 {
            m.ldef_gid = Some(save_gid);
        }
        if base {
            let l = if global {
                Locality::Global
            } else {
                Locality::Local
            };
            m.publ = Some((val, l));
        }
    }

    /// The locations a close of the groups down to `save_base` restores
    /// ([`State::close`]).
    pub(crate) fn saved_locs(&self, save_base: usize) -> Vec<Loc> {
        self.save
            .get(save_base..)
            .unwrap_or_default()
            .iter()
            .map(|s| s.loc)
            .collect()
    }

    /// Closes a group whose saves and definitions began at `save_base`
    /// and `def_base`: its saves are restored (the inert ones, hidden by
    /// a global definition since, are not, §283). A local write in the
    /// group dies with it: nothing of it is published.
    pub(crate) fn close(&mut self, save_base: usize, def_base: usize) {
        self.muts += 1;
        if self.bulk.as_ref().is_some_and(|b| b.save_at >= save_base) {
            self.bulk = None;
            self.any_gen = new_cat_gen();
        }
        while self.save.len() > save_base {
            let Some(s) = self.save.pop() else { break };
            let slot = self.slot(s.loc);
            if slot.ginert > s.at {
                continue;
            }
            self.changing(s.loc);
            if is_page_param(s.loc) {
                self.pp_dirty = true;
            }
            // (a restore changes the location as a definition does: its
            // entry at the candidate before kept, [`State::at_candidates`])
            if !self.cands.is_empty() {
                let cur = self.slot(s.loc).e;
                let meta = self.slot(s.loc).meta;
                let ldef = usize::try_from(meta)
                    .ok()
                    .filter(|&m| m > 0)
                    .and_then(|m| self.metas.get(m - 1))
                    .map_or(0, |m| m.ldef_stamp);
                self.note_cand(s.loc, ldef, cur);
            }
            let slot = self.slot(s.loc);
            slot.e = s.prev;
            // (an older binding again: its next read is looked at)
            slot.read_at.set(u32::MAX);
            slot.lazy = s.lazy;
            slot.save_gid = s.gid;
            slot.save_stamp = s.stamp;
            let meta = slot.meta;
            if let Some(m) = usize::try_from(meta)
                .ok()
                .filter(|&m| m > 0 && m <= self.metas.len())
            {
                let m = self.meta_mut(m - 1);
                (m.ldef_stamp, m.ldef_local, m.ldef_gid) = s.ldef;
            }
        }
        self.defined.truncate(def_base.min(self.defined.len()));
    }

    /// Leaves the groups from the one whose saves and definitions began
    /// at `save_base` and `def_base` (opened at `open_stamp`) open: they
    /// are not closed here (the end of the run), so the local writes
    /// made inside them are the run's own.
    pub(crate) fn abandon(&mut self, save_base: usize, def_base: usize, open_stamp: u64) {
        self.muts += 1;
        let locs: Vec<Loc> = self.defined[def_base.min(self.defined.len())..].to_vec();
        for loc in locs {
            let mi = self.meta(loc);
            if self.metas[mi].ldef_local && self.metas[mi].ldef_stamp > open_stamp {
                let val = self.slot(loc).e;
                self.meta_mut(mi).publ = Some((val, Locality::Local));
            }
        }
        self.save.truncate(save_base.min(self.save.len()));
        self.defined.truncate(def_base.min(self.defined.len()));
    }

    /// The number of the definitions at the base level, saves and
    /// definitions in groups, and the clock (what a command changed).
    pub(crate) fn sig(&self) -> (usize, usize, usize, u64) {
        (
            self.metas.len(),
            self.save.len(),
            self.defined.len(),
            self.stamp,
        )
    }

    /// The clock.
    pub(crate) fn stamp(&self) -> u64 {
        self.stamp
    }

    /// Whether `loc` was defined after stamp `since`.
    pub(crate) fn defined_after(&self, loc: Loc, since: u64) -> bool {
        self.slot_ref(loc)
            .and_then(|s| usize::try_from(s.meta).ok().filter(|&m| m > 0))
            .and_then(|m| self.metas.get(m - 1))
            .is_some_and(|m| m.ldef_stamp > since)
    }

    /// The sum of the steps of integer register `loc` the run made, where
    /// it only stepped it and never read it (nothing loaded, preloaded or
    /// lazy, no other definition): the run's value of it is what reached
    /// the run plus this.
    pub(crate) fn unread_steps(&self, loc: Loc) -> Option<i32> {
        self.slot_ref(loc)
            .filter(|s| {
                s.meta == 0
                    && s.mapped != MAP_NONE
                    && s.map.keep
                    && s.e.flags & (LOADED | PRELOADED) == 0
                    && s.lazy.is_none()
            })
            .map(|s| s.map.add)
    }

    /// Whether the latest definition of `loc` stands at the run's end
    /// as its own: global, or local outside every group of the run.
    pub(crate) fn def_stands(&self, loc: Loc) -> bool {
        self.slot_ref(loc)
            .and_then(|s| usize::try_from(s.meta).ok().filter(|&m| m > 0))
            .and_then(|m| self.metas.get(m - 1))
            .map_or_else(
                // (only stepped, even where the steps cancel, `State::add`:
                // nothing to publish, and what they leave is still later in
                // TeX's order than a routine run at a candidate before them.
                // `\\g__kernel_prg_map_int` stepped -1 then +1 around a
                // candidate in latex_starray: the routine's value of it stood
                // over the piece's, the next step made -1, and a name built
                // from it went wrong)
                || self.slot_ref(loc).is_some_and(|s| s.mapped != MAP_NONE),
                |m| !m.ldef_local || m.ldef_gid.is_none(),
            )
    }

    /// The groups opened so far: a group [`State::open`] opens after
    /// this has a greater number.
    pub(crate) fn gid_mark(&self) -> u32 {
        self.next_gid
    }

    /// The locations whose value the run read from the engine (the reads
    /// it recorded): read before the run defined them, or never defined
    /// (a value put in place of the engine's is not one). In the run's
    /// own order (its definitions' bookkeeping first, then the rest as
    /// first touched), never by location: a meaning's location is its
    /// name's number in the process's interner, which numbers names in
    /// the order the threads first meet them.
    pub(crate) fn used_locs(&self) -> Vec<Loc> {
        let mut out: Vec<Loc> = self
            .metas
            .iter()
            .filter(|m| m.eng.is_some())
            .map(|m| m.loc)
            .collect();
        for &l in &self.touched {
            if self.slot_ref(l).is_some_and(|s| {
                s.meta == 0 && s.e.flags & LOADED != 0 && s.e.flags & PRELOADED == 0
            }) {
                out.push(l);
            }
        }
        out
    }

    /// Every location whose value the run holds: read (loaded), defined,
    /// or carried by its continuation; a location may be listed twice.
    pub(crate) fn held_locs(&self) -> impl Iterator<Item = Loc> + '_ {
        self.metas
            .iter()
            .map(|m| m.loc)
            .chain(
                self.touched
                    .iter()
                    .copied()
                    .filter(|&l| self.slot_ref(l).is_some_and(|s| s.e.flags & LOADED != 0)),
            )
            .chain(self.cont_locs.iter().copied())
    }

    /// Every location the run defined: its latest definition's stamp,
    /// whether local, and the group it was made in.
    pub(crate) fn defined_stamped(&self) -> Vec<(Loc, u64, bool, Option<u32>)> {
        let mut out: Vec<_> = self
            .metas
            .iter()
            .filter(|m| m.ldef_stamp > 0)
            .map(|m| (m.loc, m.ldef_stamp, m.ldef_local, m.ldef_gid))
            .collect();
        // (a register stepped after a candidate, no definition: as one made
        // right after the last candidate holding it, [`State::add`])
        for (st, xs) in &self.cands {
            for &(l, _) in xs {
                if self
                    .slot_ref(l)
                    .is_some_and(|s| s.meta == 0 && s.mapped != MAP_NONE)
                {
                    out.retain(|x: &(Loc, u64, bool, Option<u32>)| x.0 != l);
                    out.push((l, st + 1, false, None));
                }
            }
        }
        out
    }

    /// The locations whose latest definition after stamp `since` is
    /// local, as [`State::defined_since_in_order`] filters them, in the
    /// order of their first definitions in the run, without their values.
    pub(crate) fn defined_local_since(&self, since: u64, gids: u32) -> Vec<Loc> {
        self.metas
            .iter()
            .filter(|m| m.ldef_stamp > since)
            .filter(|m| !(m.ldef_local && m.ldef_gid.is_some_and(|g| g > gids)))
            .filter(|m| m.ldef_local)
            .map(|m| m.loc)
            .collect()
    }

    /// The definitions made after stamp `since`, at any level (a line
    /// read back is interpreted wherever the run is): the location, the
    /// value in force and the locality of the latest one. A local one
    /// made in a group opened after `gids` is left out: that group has
    /// ended (§282). In the order of the latest definitions.
    pub(crate) fn defined_since_in_order(
        &mut self,
        v: &Values,
        since: u64,
        gids: u32,
    ) -> Vec<(Loc, Option<ValueId>, Locality)> {
        let mut hits: Vec<(Loc, bool, u64)> = self
            .metas
            .iter()
            .filter(|m| m.ldef_stamp > since)
            .filter(|m| !(m.ldef_local && m.ldef_gid.is_some_and(|g| g > gids)))
            .map(|m| (m.loc, m.ldef_local, m.ldef_stamp))
            .collect();
        hits.sort_by_key(|h| h.2);
        hits.into_iter()
            .map(|(loc, local, _)| {
                let x = self.id_of(loc, v);
                (
                    loc,
                    x,
                    if local {
                        Locality::Local
                    } else {
                        Locality::Global
                    },
                )
            })
            .collect()
    }

    // ----------------------------------------------------- publication

    /// What `m`'s binding at the run's base level publishes (spec §2.3):
    /// nothing where it restores what the run read (rule 1: a local one
    /// anywhere, a global one where the run stands outside every scope
    /// of the document, `top`, as inside one a global write of a scope's
    /// local value outlives the scope, §283), nor where it is the
    /// command's transient state left as it was read; else its value
    /// (rule 4). A scope's local value over steps the run publishes
    /// beneath it ([`State::add`]: `\global\advance\@listdepth\@ne`
    /// then `\advance\@listdepth\m@ne` in a group left open, KOMA's
    /// addmargin) restores nothing: the steps changed what it hides.
    fn base_value(&self, m: &Meta, v: &Values, top: bool) -> Option<(ValueId, Locality)> {
        let (val, l) = m.publ.as_ref()?;
        let x = self.encode(val, v)?;
        if ((top || *l != Locality::Global) && m.eng == Some(Some(x)) && !self.has_map(m.loc))
            || unchanged_transient(m, x)
        {
            return None;
        }
        Some((x, *l))
    }

    /// Whether the run has a global binding at its base level equal to
    /// what it read, which publishes nothing only outside every scope of
    /// the document ([`State::base_value`]'s `top`).
    pub(crate) fn has_global_restore(&self, v: &Values) -> bool {
        self.metas.iter().any(|m| {
            matches!(&m.publ, Some((val, Locality::Global))
                if m.eng.is_some_and(|e| e.is_some() && self.encode(val, v) == e))
        })
    }

    /// What the run publishes at its end (spec §2.3), by the one rule:
    /// per item it wrote at its base level (its global writes at any
    /// depth, its local writes outside every group it opened or in a
    /// group it leaves open, at that group's depth), nothing where the
    /// write restores what it read, a relation where it only stepped an
    /// integer register it never read, a value otherwise; and the items
    /// restored (no net effect: neither published nor a read of a
    /// package's record). `top`: the run stands outside every scope of
    /// the document.
    pub(crate) fn publication(&self, v: &Values, top: bool) -> (Vec<Published>, Vec<Loc>) {
        self.publication_kept(v, top, &[])
    }

    /// [`State::publication`], the items of `kept` published as they
    /// stand wherever the run wrote them, restores and transients too:
    /// those a read-back the run made itself defined after its read-back
    /// point (`Run::rb_check`), whose binding is the run's own, not what
    /// the run read (a read-back child defines what its run did not).
    pub(crate) fn publication_kept(
        &self,
        v: &Values,
        top: bool,
        kept: &[Loc],
    ) -> (Vec<Published>, Vec<Loc>) {
        let mut out = Vec::with_capacity(self.metas.len());
        let mut restored = Vec::new();
        for m in &self.metas {
            let keep = kept.contains(&m.loc);
            if !keep
                && let Some((val, _)) = &m.publ
                && m.eng
                    .is_some_and(|e| e.is_some() && self.encode(val, v) == e)
                && !self.has_map(m.loc)
            {
                restored.push(m.loc);
            }
            let made = if keep {
                m.publ
                    .as_ref()
                    .and_then(|(val, l)| Some((self.encode(val, v)?, *l)))
            } else {
                self.base_value(m, v, top)
            };
            if let Some((x, l)) = made {
                let carried = l == Locality::Local && self.cont_locs.contains(&m.loc);
                out.push(Published {
                    loc: m.loc,
                    what: Pub::Value(x),
                    at: l,
                    carried,
                });
            }
        }
        for (loc, val, l) in &self.scope_extra {
            if let Some(x) = self.encode(val, v) {
                out.push(Published {
                    loc: *loc,
                    what: Pub::Value(x),
                    at: *l,
                    carried: false,
                });
            }
        }
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            // (steps a set has not replaced: a local write in a group
            // since, [`State::set`], left them the base binding)
            if s.mapped == MAP_NONE {
                continue;
            }
            if s.map == Map::ID {
                restored.push(l);
                continue;
            }
            let at = if s.mapped == MAP_GLOBAL {
                Locality::Global
            } else {
                Locality::Local
            };
            // (beneath a scope's value of the run's own, the value the
            // steps make from the one read: `\global\advance\@listdepth
            // \@ne`, then `\advance\@listdepth\m@ne` in a group left
            // open, KOMA's addmargin, inside it 0 and outside 1)
            let made = s.map.keep && !s.base_unread && self.steps_under_scope(l);
            let what = if made {
                Pub::Value(crate::val::int_id(
                    v,
                    i64::from(s.map.apply(crate::val::i32_of(v, s.base))),
                ))
            } else if s.map.keep {
                Pub::Rel(i64::from(s.map.add))
            } else {
                Pub::Value(crate::val::int_id(v, i64::from(s.map.add)))
            };
            out.push(Published {
                loc: l,
                what,
                at,
                carried: false,
            });
        }
        (out, restored)
    }

    /// The value the run's base level publishes of `m`, if any (not a
    /// scope's).
    fn base_publish(&self, m: &Meta, v: &Values, top: bool) -> Option<ValueId> {
        if matches!(m.publ, Some((_, Locality::Scope))) {
            return None;
        }
        self.base_value(m, v, top).map(|x| x.0)
    }

    /// Whether the run's base level publishes field `f` (at `loc`, its
    /// static location, else its spill location if it has one).
    pub(crate) fn base_publishes(&self, v: &Values, f: Field, loc: Option<Loc>, top: bool) -> bool {
        let Some(loc) = loc.or_else(|| self.spill_ix.get(&f).map(|&i| SPILL | i)) else {
            return false;
        };
        self.slot_ref(loc)
            .and_then(|s| (s.meta as usize).checked_sub(1))
            .and_then(|mi| self.metas.get(mi))
            .is_some_and(|m| self.base_publish(m, v, top).is_some())
    }

    /// The group's local definitions as they stand: of the group whose
    /// definitions began at `def_base` (opened at `open_stamp`), each
    /// location whose latest definition is a local one made since, with
    /// its value.
    fn group_locals(
        &self,
        v: &Values,
        def_base: usize,
        open_stamp: u64,
    ) -> Vec<(usize, Option<ValueId>)> {
        let mut out = Vec::new();
        for &loc in &self.defined[def_base.min(self.defined.len())..] {
            let Some(s) = self.slot_ref(loc) else {
                continue;
            };
            let Some(mi) = (s.meta as usize).checked_sub(1) else {
                continue;
            };
            let Some(m) = self.metas.get(mi) else {
                continue;
            };
            if m.ldef_local && m.ldef_stamp > open_stamp {
                out.push((mi, self.encode(&s.e, v)));
            }
        }
        out
    }

    /// Leaves the group whose saves and definitions began at `save_base`
    /// and `def_base` (opened at `open_stamp`) to the engine as the scope
    /// the run opens after itself: its local writes are published at its
    /// depth (`Locality::Scope`), the base level's stay the run's; a
    /// group's local of the value the base level publishes is that
    /// definition seen inside; of another value, a second entry of the
    /// field, the scope's ([`State::scope_extra`]).
    pub(crate) fn abandon_to_scope(
        &mut self,
        v: &Values,
        save_base: usize,
        def_base: usize,
        open_stamp: u64,
        top: bool,
    ) {
        self.muts += 1;
        for (mi, x) in self.group_locals(v, def_base, open_stamp) {
            let Some(x) = x else { continue };
            let loc = self.metas[mi].loc;
            match self.base_publish(&self.metas[mi], v, top) {
                Some(y) if y == x => {}
                Some(_) => {
                    if !self.scope_extra.iter().any(|e| e.0 == loc) {
                        let val = self.slot(loc).e;
                        self.scope_extra.push((loc, val, Locality::Scope));
                    }
                }
                None => {
                    let val = self.slot(loc).e;
                    self.meta_mut(mi).publ = Some((val, Locality::Scope));
                }
            }
        }
        self.save.truncate(save_base.min(self.save.len()));
        self.defined.truncate(def_base.min(self.defined.len()));
    }

    /// A definition of `loc` in the scope the run opens after itself,
    /// beside the run's own one at its base level ([`State::scope_extra`]).
    pub(crate) fn define_scope_extra(&mut self, loc: Loc, val: Entry) {
        self.muts += 1;
        self.scope_extra.retain(|e| e.0 != loc);
        self.scope_extra.push((loc, val, Locality::Scope));
    }

    /// A definition of `loc` in frame `k` of those the run opens after
    /// itself (0: the outermost; [`State::abandon_to_frames`]).
    pub(crate) fn define_frame_extra(&mut self, loc: Loc, val: Entry, k: u8) {
        self.muts += 1;
        let l = Locality::ScopeAt(k);
        self.scope_extra.retain(|e| !(e.0 == loc && e.2 == l));
        self.scope_extra.push((loc, val, l));
    }

    /// Leaves the run's open groups, `levels` (each `(save_base,
    /// def_base, open_stamp)`, outermost first), to the engine as the
    /// frames the run opens after itself, frame `k` for group `k`: each
    /// group's local writes, at the value in force at its level, are
    /// frame `k`'s (`Locality::ScopeAt(k)`); the base level's stay the
    /// run's. A location an inner group defined again holds the group's
    /// value in the inner group's first save of it (what that group's
    /// close restores, §282; a save hidden by a global definition since
    /// restores nothing, §283).
    pub(crate) fn abandon_to_frames(&mut self, levels: &[(usize, usize, u64)]) {
        self.muts += 1;
        let Some(&(sb0, db0, _)) = levels.first() else {
            return;
        };
        let mut out: Vec<(Loc, Entry, Locality)> = Vec::new();
        for (k, &(_, db, open_stamp)) in levels.iter().enumerate() {
            let next = levels.get(k + 1);
            let de = next
                .map_or(self.defined.len(), |l| l.1)
                .min(self.defined.len());
            let se = next.map_or(self.save.len(), |l| l.0).min(self.save.len());
            let mut seen = phitex_defuse::FxHashSet::default();
            for i in db.min(de)..de {
                let loc = self.defined[i];
                if !seen.insert(loc) {
                    continue;
                }
                let Some(s) = self.slot_ref(loc) else {
                    continue;
                };
                let inner = self.save[se..]
                    .iter()
                    .find(|x| x.loc == loc && s.ginert <= x.at);
                let (val, ldef) = if let Some(x) = inner {
                    (x.prev, x.ldef)
                } else {
                    let Some(m) = (s.meta as usize)
                        .checked_sub(1)
                        .and_then(|mi| self.metas.get(mi))
                    else {
                        continue;
                    };
                    (s.e, (m.ldef_stamp, m.ldef_local, m.ldef_gid))
                };
                if ldef.1 && ldef.0 > open_stamp {
                    out.push((
                        loc,
                        val,
                        Locality::ScopeAt(u8::try_from(k).unwrap_or(u8::MAX - 2)),
                    ));
                }
            }
        }
        for e in out {
            self.scope_extra.retain(|x| !(x.0 == e.0 && x.2 == e.2));
            self.scope_extra.push(e);
        }
        self.save.truncate(sb0.min(self.save.len()));
        self.defined.truncate(db0.min(self.defined.len()));
    }

    /// The run's definition of `loc` at its base level made the scope's
    /// it opens after itself ([`State::abandon_to_scope`]).
    pub(crate) fn publish_in_scope(&mut self, loc: Loc) {
        self.muts += 1;
        let mi = self.meta(loc);
        if let Some((_, l)) = &mut self.meta_mut(mi).publ {
            *l = Locality::Scope;
        }
    }

    // ----------------------------------------------------------- debug

    /// Debug: a slot's marks (touched, local definition and its group,
    /// saving group, inert stamp).
    pub(crate) fn debug_marks(&self, loc: Loc) -> String {
        let Some(s) = self.slot_ref(loc) else {
            return "no slot".to_owned();
        };
        let m = usize::try_from(s.meta)
            .ok()
            .filter(|&m| m > 0)
            .and_then(|m| self.metas.get(m - 1));
        format!(
            "{:?} touched {} in-touched-list {} save_gid {} ginert {} ldef {:?} defined-list {} map {:?}/{}",
            self.held(loc),
            s.touched,
            self.touched.contains(&loc),
            s.save_gid,
            s.ginert,
            m.map(|m| (m.ldef_local, m.ldef_gid, m.ldef_stamp)),
            self.defined.contains(&loc),
            s.map,
            s.mapped
        )
    }

    /// Debug: a slot's entry as the run holds it (value kind, id,
    /// flags), without loading it.
    pub(crate) fn debug_slot(&self, loc: Loc) -> String {
        self.slot_ref(loc).map_or_else(
            || "none".to_owned(),
            |s| {
                format!(
                    "absent {} id {:?} flags {:#x}",
                    s.e.kind == K_ABSENT,
                    s.e.id,
                    s.e.flags
                )
            },
        )
    }

    /// Debug: an entry about to be defined, rendered.
    pub(crate) fn debug_entry(&self, e: &Entry, v: &Values) -> String {
        match e.kind {
            K_INT => e.chr.to_string(),
            _ => self
                .encode(e, v)
                .map_or_else(|| "absent".to_owned(), |x| v.render(x)),
        }
    }
}
/// What a run read of a meaning by a projection alone, by location
/// ([`crate::run::Run::step_lookup`] and its kin): one dense entry per
/// location below [`SPILL`], a map above it; the entries set are listed
/// so the table goes back to its thread's pool cleared.
pub(crate) struct Memos {
    dense: Vec<Memo>,
    set: Vec<Loc>,
    spill: FxHashMap<Loc, Memo>,
}

/// One location's projected reads ([`Memos`]): the step class, the
/// command class, and whether it is outer, defined, a number, each with
/// its bit in `has`.
#[derive(Clone, Copy, Default)]
pub(crate) struct Memo {
    pub step: i64,
    pub class: u8,
    pub has: u8,
    pub val: u8,
}

/// [`Memo::has`]'s bits (and [`Memo::val`]'s for the booleans).
pub(crate) const M_STEP: u8 = 1;
pub(crate) const M_CLASS: u8 = 2;
pub(crate) const M_OUTER: u8 = 4;
pub(crate) const M_DEFINED: u8 = 8;
pub(crate) const M_NUM: u8 = 16;

thread_local! {
    static MEMO_POOL: std::cell::RefCell<Vec<Vec<Memo>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Default for Memos {
    fn default() -> Memos {
        Memos {
            dense: MEMO_POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default(),
            set: Vec::new(),
            spill: FxHashMap::default(),
        }
    }
}

impl Drop for Memos {
    fn drop(&mut self) {
        let mut d = std::mem::take(&mut self.dense);
        for &l in &self.set {
            if let Some(m) = d.get_mut(l as usize) {
                *m = Memo::default();
            }
        }
        MEMO_POOL.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < 4 {
                p.push(d);
            }
        });
    }
}

impl Memos {
    /// `loc`'s entry (empty where nothing was read).
    #[inline]
    pub(crate) fn get(&self, loc: Loc) -> Memo {
        if loc & SPILL == 0 {
            self.dense.get(loc as usize).copied().unwrap_or_default()
        } else {
            self.spill.get(&loc).copied().unwrap_or_default()
        }
    }

    /// `loc`'s entry, to set.
    pub(crate) fn entry(&mut self, loc: Loc) -> &mut Memo {
        if loc & SPILL != 0 {
            return self.spill.entry(loc).or_default();
        }
        let i = loc as usize;
        if i >= self.dense.len() {
            self.dense.resize(i + 1 + 1024, Memo::default());
        }
        let m = &mut self.dense[i];
        if m.has == 0 {
            self.set.push(loc);
        }
        m
    }

    /// Sets bit `b` of `loc` to `x` (a boolean projection).
    pub(crate) fn set_bool(&mut self, loc: Loc, b: u8, x: bool) {
        let m = self.entry(loc);
        m.has |= b;
        if x {
            m.val |= b;
        } else {
            m.val &= !b;
        }
    }

    /// `loc`'s boolean projection `b`, where read.
    #[inline]
    pub(crate) fn bool_of(&self, loc: Loc, b: u8) -> Option<bool> {
        let m = self.get(loc);
        (m.has & b != 0).then_some(m.val & b != 0)
    }
}

/// An entry as a definition stores it: loaded, its engine value and
/// meaning record kept where known.
fn stored(val: Entry) -> Entry {
    let mut e = val;
    e.flags = LOADED | (val.flags & (HAS_ID | CC | ALIAS));
    e
}

/// Local definitions in the group open now of a code table's entries
/// `lo..hi` past its fixed slots, each character's entry the character,
/// not yet written ([`State::bulk_codes`]): a table of 65536 entries set
/// whole in a group that then reads three of them and ends. No slot, no
/// save entry and no definition mark exist until an entry is asked for
/// ([`State::bulk_take`]), or the run does anything other than the few
/// commands that cannot see the table ([`crate::run::Run::flush_bulk_codes`]);
/// the group's close forgets what was never asked for, which leaves each
/// entry as restoring it would.
#[derive(Clone)]
pub(crate) struct BulkCodes {
    table: usize,
    lo: i32,
    hi: i32,
    gid: u32,
    /// The save stack's height when made: the group that holds it closes
    /// to no more than this.
    save_at: usize,
    /// Entries written since.
    done: phitex_defuse::FxHashSet<i32>,
}

/// Whether `loc` is one of the page builder's parameters (§980).
pub(crate) fn is_page_param(loc: Loc) -> bool {
    use crate::fields::Par;
    [
        Par::vsize as Loc,
        Par::maxdepth as Loc,
        Par::topskip as Loc,
        Par::holdinginserts as Loc,
        Par::savingvdiscards as Loc,
    ]
    .contains(&loc)
}

// ---------------------------------------------------- continuations

/// A slot of the table for a continuation (`crate::pages`, a run cut at
/// a candidate): a location bound locally in a group still open, or
/// saved by one, or defined globally in the run.
pub(crate) struct ContSlot {
    pub loc: Loc,
    /// The current binding (`None`: absent); the resume keeps it only when
    /// `loaded`, else it reads the engine.
    pub cur: Option<ValueId>,
    /// Whether the binding is an open group's of the run.
    pub loaded: bool,
    pub save_gid: u32,
    pub save_stamp: u64,
    pub ginert: u64,
    /// The latest definition: its stamp, whether local, its group.
    pub ldef: Option<(u64, bool, Option<u32>)>,
    /// For a group-local binding: the value at the run's base level (its
    /// own definition there, or the engine's value it read; `Some(None)`
    /// absent); `None` when the run never read it.
    #[allow(clippy::option_option)]
    pub base: Option<Option<ValueId>>,
    pub group_local: bool,
}

/// A save of the table for a continuation.
pub(crate) struct ContSaved {
    pub loc: Loc,
    pub prev: Option<ValueId>,
    pub prev_loaded: bool,
    pub gid: u32,
    pub stamp: u64,
    pub at: u64,
    pub ldef: Ldef,
}

/// The table's groups for a continuation: what the run's open groups
/// bound and saved, nothing of its base level (published by the run) or
/// of its engine reads (the resume reads again, after the finisher).
pub(crate) struct ContState {
    pub slots: Vec<ContSlot>,
    pub save: Vec<ContSaved>,
    pub defined: Vec<Loc>,
    pub stamp: u64,
    pub next_gid: u32,
}

impl State {
    /// Every location the run defined so far (locally or globally, in a
    /// group or not) with its binding now (`None`: absent): the run's
    /// state at this point where it is not the engine's (a picture
    /// child's view of its parent, `crate::pic`).
    /// The registers the run has stepped without reading or defining
    /// them (a map that adds, `State::add`): `(loc, steps, global)` each,
    /// in the order touched. A child reading before the run
    /// (`crate::pic`) starts from them, as TeX's picture starts from the
    /// value the run left.
    pub(crate) fn mapped_now(&self) -> Vec<(Loc, i32, bool)> {
        let mut out = Vec::new();
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            if s.mapped == MAP_NONE
                || s.meta != 0
                || s.e.flags & LOADED != 0
                || !s.map.keep
                || s.map.add == 0
            {
                continue;
            }
            out.push((l, s.map.add, s.mapped == MAP_GLOBAL));
        }
        out
    }

    pub(crate) fn defined_now(&self, v: &Values) -> Vec<(Loc, Option<ValueId>)> {
        let mut out = Vec::new();
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            let defined = usize::try_from(s.meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| self.metas.get(m - 1))
                .is_some_and(|m| m.ldef_stamp > 0);
            // (a binding a group's end took back to one the run never
            // loaded: the engine's, which the child reads itself)
            if !defined || s.e.flags & LOADED == 0 {
                continue;
            }
            let cur = if s.e.flags & HAS_ID != 0 {
                s.e.id
            } else {
                self.encode(&s.e, v)
            };
            out.push((l, cur));
        }
        out
    }

    /// `open`: the groups of the run open now (their ids, the nest's
    /// group levels): a slot is a group's local only while its local
    /// definition's group is one of them.
    pub(crate) fn cont(&self, v: &Values, open: &[u32]) -> ContState {
        let ent = |e: &Entry| -> (Option<ValueId>, bool) {
            let id = if e.flags & HAS_ID != 0 {
                e.id
            } else {
                self.encode(e, v)
            };
            (id, e.flags & LOADED != 0)
        };
        let mut slots = Vec::new();
        // (the local definitions in the groups still open: a group's
        // close restores the slot without clearing the meta's mark of a
        // local definition in a group; `\begingroup\let\thepage\relax
        // ...\endgroup` closed before the cut is no local of the
        // continuation. By the group's id, which a resume keeps: the
        // `defined` list does not survive a resume's own closes)
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            let m = usize::try_from(s.meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| self.metas.get(m - 1));
            let group_local =
                m.is_some_and(|m| m.ldef_local && m.ldef_gid.is_some_and(|g| open.contains(&g)));
            // (a slot no open group saved and no local binding: what the
            // run defined there globally is published, and the resume
            // reads it from the engine as any piece does, depending on
            // it only if it reads it; with no save open its last global
            // definition's stamp makes no save inert)
            if s.save_gid == 0 && !group_local {
                continue;
            }
            let (cur, loaded) = ent(&s.e);
            let base = m.and_then(|m| match &m.publ {
                Some((val, _)) => Some(self.encode(val, v)),
                None => m.eng,
            });
            slots.push(ContSlot {
                loc: l,
                cur,
                loaded: group_local && loaded,
                save_gid: s.save_gid,
                save_stamp: s.save_stamp,
                ginert: s.ginert,
                ldef: m.map(|m| (m.ldef_stamp, m.ldef_local, m.ldef_gid)),
                base,
                group_local,
            });
        }
        let save = self
            .save
            .iter()
            .map(|s| {
                let (prev, prev_loaded) = ent(&s.prev);
                ContSaved {
                    loc: s.loc,
                    prev,
                    prev_loaded,
                    gid: s.gid,
                    stamp: s.stamp,
                    at: s.at,
                    ldef: s.ldef,
                }
            })
            .collect();
        ContState {
            slots,
            save,
            defined: self.defined.clone(),
            stamp: self.stamp,
            next_gid: self.next_gid,
        }
    }

    /// The table of a resume piece ([`State::cont`]).
    pub(crate) fn from_cont(v: &Values, c: ContState) -> State {
        let mut st = State::new();
        // (a binding that is an alias definition's value stays one,
        // resolved where the resume uses it)
        // (one not loaded is read from the engine: its value is not
        // carried, `Run::cont_encode`)
        let ent = |x: Option<ValueId>, loaded: bool| {
            if !loaded {
                return UNLOADED;
            }
            let mut e = Entry::raw_seen(v, x);
            e.flags = LOADED | HAS_ID | (e.flags & ALIAS);
            e
        };
        for s in c.slots {
            st.cont_locs.push(s.loc);
            let meta = s.ldef.map(|(stamp, local, gid)| {
                st.metas.push(Meta {
                    loc: s.loc,
                    eng: None,
                    publ: None,
                    ldef_stamp: stamp,
                    ldef_local: local,
                    ldef_gid: gid,
                });
                u32::try_from(st.metas.len()).unwrap_or(0)
            });
            let slot = st.slot(s.loc);
            slot.e = ent(s.cur, s.loaded);
            slot.save_gid = s.save_gid;
            slot.save_stamp = s.save_stamp;
            slot.ginert = s.ginert;
            slot.meta = meta.unwrap_or(0);
        }
        for s in c.save {
            st.save.push(Saved {
                loc: s.loc,
                prev: ent(s.prev, s.prev_loaded),
                gid: s.gid,
                stamp: s.stamp,
                at: s.at,
                ldef: s.ldef,
                lazy: None,
            });
        }
        st.defined = c.defined;
        st.stamp = c.stamp;
        st.next_gid = c.next_gid;
        st
    }

    /// A routine's global definition in force where it ran, at a candidate
    /// the run passes (`Run::order_after_fires`): the value as if read,
    /// never the run's own definition (it publishes nothing of it; the
    /// routine's finisher, after it, defines it), and the open groups'
    /// saves of it inert (§279, §283).
    pub(crate) fn apply_global(&mut self, v: &Values, loc: Loc, x: Option<ValueId>) {
        self.changing(loc);
        self.muts += 1;
        // (the value at this candidate, which the routine begins with, is
        // the one before: [`State::at_candidates`])
        if !self.cands.is_empty() {
            let (prev, meta) = {
                let s = self.slot(loc);
                (s.e, s.meta)
            };
            let ldef = usize::try_from(meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| self.metas.get(m - 1))
                .map_or(0, |m| m.ldef_stamp);
            self.note_cand(loc, ldef, prev);
        }
        self.stamp += 1;
        let st = self.stamp;
        let mut e = Entry::raw_seen(v, x);
        e.flags |= LOADED | PRELOADED;
        let now = self.past_n;
        let s = self.slot(loc);
        s.e = e;
        s.lazy = None;
        s.ginert = st;
        s.read_at.set(u32::MAX);
        self.floors.push((loc, now.saturating_sub(1)));
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
    }

    /// A global definition made between a cut and its resume (the
    /// finisher's) stands (§283): the binding is the engine's value now,
    /// and the open groups' saves of it are inert.
    pub(crate) fn override_global(&mut self, loc: Loc, x: Option<ValueId>) {
        self.changing(loc);
        self.muts += 1;
        let mi = self.meta(loc);
        self.stamp += 1;
        let st = self.stamp;
        let s = self.slot(loc);
        let mut e = Entry::raw(x);
        e.flags = LOADED | HAS_ID;
        s.e = e;
        s.ginert = st;
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
        let m = self.meta_mut(mi);
        m.ldef_stamp = st;
        m.ldef_local = false;
        m.ldef_gid = None;
    }
}

/// A run's table at its base level, no group of its own open
/// ([`State::base_of`]): what an edited node's run that goes on from a
/// checkpoint takes up ([`crate::resume`]): what the run defined,
/// stepped and read (a definition after a candidate notes the value the
/// run read there).
pub(crate) struct Base {
    slots: Vec<BaseSlot>,
    /// The candidates passed ([`State::mark_candidate`]): each one's clock
    /// and the entries noted at it (`None`: not loaded).
    cands: Vec<(u64, Vec<BaseCand>)>,
    /// [`State::past`], [`State::read_across`].
    past: Vec<(u64, u32)>,
    read_across: Vec<(Loc, u32, u32)>,
    extra: Vec<(Loc, Option<ValueId>, Locality)>,
    defined: Vec<Loc>,
    font_slow: Vec<Loc>,
    stamp: u64,
    next_gid: u32,
    pp_dirty: bool,
    /// The save stack of the run's open groups, bottom first
    /// ([`Saved`]: the entry saved by its value and flags).
    save: Vec<BaseSave>,
    /// The locations a resume piece's table carried
    /// ([`State::from_cont`]).
    carried: Vec<Loc>,
    /// Every location touched, in order (those only read among them):
    /// what iterates them goes in the run's order after a resumption too
    /// (a cut's group-local bindings, [`State::cont`]).
    touched: Vec<Loc>,
}

/// What a run's checkpoints made before, kept for its next ones
/// ([`State::base_of`], [`Base::to_value`]): a macro's and a token list's
/// value by the body it encodes (held, so its address names no other),
/// and each location's slot value by the slot it encodes. A run's tables
/// change by a few locations between two checkpoints: each made anew
/// cost Grail and amsldoc 5-9% of a cold build (callgrind, `ck_take`).
#[derive(Default)]
pub(crate) struct CkMemo {
    macros: FxHashMap<usize, (Rc<crate::val::Macro>, ValueId)>,
    toks: FxHashMap<usize, (Rc<[Tok]>, ValueId)>,
    slots: FxHashMap<Loc, (BaseSlot, ValueId)>,
    /// The touched locations' list by chunks of [`TOUCHED_CHUNK`]: each
    /// chunk's locations and its value.
    touched: Vec<(Vec<Loc>, ValueId)>,
    /// [`State::read_across`]' list by chunks of [`TOUCHED_CHUNK`]: a
    /// chunk's entries change where the run reads them again.
    across: Vec<(Vec<ReadAcross>, ValueId)>,
}

/// An entry of [`State::read_across`]: location, from, to.
type ReadAcross = (Loc, u32, u32);

/// The touched locations a chunk of [`Base::to_value`]'s list holds: the
/// list grows at its end, its chunks before the last stay as they were.
const TOUCHED_CHUNK: usize = 256;

/// A save of [`Base`]'s.
struct BaseSave {
    loc: Loc,
    prev: Option<ValueId>,
    flags: u8,
    gid: u32,
    stamp: u64,
    at: u64,
    ldef: Ldef,
    lazy: Option<ValueId>,
}

/// A candidate's kept entry in a [`Base`]: its location, its value where
/// loaded (`None` not loaded), and the steps of an unread register
/// ([`STEPPED`], else 0).
#[allow(clippy::option_option)]
type BaseCand = (Loc, Option<Option<ValueId>>, i32);

#[derive(Clone, PartialEq)]
struct BaseSlot {
    loc: Loc,
    cur: Option<ValueId>,
    flags: u8,
    /// The marks of the save a closed group restored (the save stack is
    /// empty: they name no open group).
    save: (u32, u64, u64),
    map: Map,
    mapped: u8,
    base: Option<ValueId>,
    lazy: Option<ValueId>,
    sym: bool,
    meta: Option<BaseMeta>,
}

#[allow(clippy::option_option)]
#[derive(Clone, PartialEq)]
struct BaseMeta {
    /// Its index among the table's metas: their order is the order of
    /// the run's first definitions, which a cut's check data keeps
    /// ([`State::defined_local_since`]).
    ix: u32,
    /// The slot's [`Slot::def_gid`] and [`Slot::cand_noted`].
    def_gid: u32,
    cand_noted: u32,
    eng: Option<Option<ValueId>>,
    publ: Option<(Option<ValueId>, Locality)>,
    ldef_stamp: u64,
    ldef_local: bool,
    ldef_gid: Option<u32>,
}

impl State {
    /// The table at its base level ([`Base`]): each location the run
    /// defined, stepped, holds a symbol or a value given in place of the
    /// engine's at, or a save marks; with the save stack of the groups
    /// open. None while a command's changes may be undone.
    pub(crate) fn base_of(&self, v: &Values, memo: &mut CkMemo) -> Option<Base> {
        if self.undo.is_some() {
            return None;
        }
        let mut slots = Vec::new();
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            // (one the run only read stays as read: what it holds at a
            // candidate is the value there, `State::at_candidates`)
            if s.meta == 0
                && s.mapped == MAP_NONE
                && !s.sym
                && s.lazy.is_none()
                && s.e.flags & (PRELOADED | LOADED) == 0
                && (s.save_gid, s.save_stamp, s.ginert) == (0, 0, 0)
            {
                continue;
            }
            let meta = usize::try_from(s.meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| Some((m - 1, self.metas.get(m - 1)?)))
                .map(|(ix, m)| BaseMeta {
                    ix: u32::try_from(ix).unwrap_or(u32::MAX),
                    def_gid: s.def_gid,
                    cand_noted: s.cand_noted,
                    eng: m.eng,
                    publ: m
                        .publ
                        .as_ref()
                        .map(|(e, l)| (self.encode_memo(e, v, memo), *l)),
                    ldef_stamp: m.ldef_stamp,
                    ldef_local: m.ldef_local,
                    ldef_gid: m.ldef_gid,
                });
            slots.push(BaseSlot {
                loc: l,
                cur: self.encode_memo(&s.e, v, memo),
                flags: s.e.flags,
                save: (s.save_gid, s.save_stamp, s.ginert),
                map: s.map,
                mapped: s.mapped,
                base: s.base,
                lazy: s.lazy,
                sym: s.sym,
                meta,
            });
        }
        let cands = self
            .cands
            .iter()
            .map(|(st, xs)| {
                (
                    *st,
                    xs.iter()
                        .map(|(l, e)| {
                            (
                                *l,
                                (e.flags & LOADED != 0).then(|| self.encode_memo(e, v, memo)),
                                if e.flags & STEPPED != 0 { e.chr } else { 0 },
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        Some(Base {
            slots,
            cands,
            past: self.past.clone(),
            read_across: self.read_across.borrow().clone(),
            extra: self
                .scope_extra
                .iter()
                .map(|(l, e, at)| (*l, self.encode_memo(e, v, memo), *at))
                .collect(),
            defined: self.defined.clone(),
            font_slow: self.font_slow.iter().copied().collect(),
            stamp: self.stamp,
            next_gid: self.next_gid,
            pp_dirty: self.pp_dirty,
            save: self
                .save
                .iter()
                .map(|s| BaseSave {
                    loc: s.loc,
                    prev: self.encode_memo(&s.prev, v, memo),
                    flags: s.prev.flags,
                    gid: s.gid,
                    stamp: s.stamp,
                    at: s.at,
                    ldef: s.ldef,
                    lazy: s.lazy,
                })
                .collect(),
            carried: self.cont_locs.clone(),
            touched: self.touched.clone(),
        })
    }

    /// How many locations the table holds (what a base table costs to
    /// make, [`State::base_of`]).
    pub(crate) fn touched_len(&self) -> usize {
        self.touched.len()
    }

    /// How long a base table of it is ([`State::base_of`]): its lists'
    /// entries, the locations twice (their slots and their order).
    pub(crate) fn base_len(&self) -> usize {
        2 * self.touched.len()
            + self.defined.len()
            + self.save.len()
            + self.cands.iter().map(|c| c.1.len()).sum::<usize>()
    }

    /// Why [`State::base_of`] gives none (a trace's).
    pub(crate) fn base_why(&self) -> String {
        let saved = self
            .touched
            .iter()
            .filter(|&&l| self.slot_ref(l).is_some_and(|s| s.save_gid != 0))
            .count();
        format!(
            "saves {}, carried {}, undo {}, slots a group saved {saved}",
            self.save.len(),
            self.cont_locs.len(),
            self.undo.is_some()
        )
    }

    /// Takes up `b` ([`State::base_of`]) in a table with no definition
    /// of its own yet.
    pub(crate) fn with_base(&mut self, v: &Values, b: Base) {
        let ent = |x: Option<ValueId>, flags: u8| {
            let mut e = Entry::raw_seen(v, x);
            e.flags = (flags & (LOADED | PRELOADED))
                | (e.flags & ALIAS)
                | if x.is_some() { HAS_ID } else { 0 };
            e
        };
        // (the locations touched in the run's order first)
        for &l in &b.touched {
            self.slot(l);
        }
        // (the metas in their order in the run, [`BaseMeta::ix`])
        let mut order: Vec<(u32, usize)> = b
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((s.meta.as_ref()?.ix, i)))
            .collect();
        order.sort_unstable();
        let mut meta_of = vec![0u32; b.slots.len()];
        for (_, i) in order {
            let s = &b.slots[i];
            let Some(m) = s.meta.as_ref() else { continue };
            self.metas.push(Meta {
                loc: s.loc,
                eng: m.eng,
                publ: m.publ.map(|(x, l)| (ent(x, LOADED), l)),
                ldef_stamp: m.ldef_stamp,
                ldef_local: m.ldef_local,
                ldef_gid: m.ldef_gid,
            });
            meta_of[i] = u32::try_from(self.metas.len()).unwrap_or(0);
        }
        for (i, s) in b.slots.into_iter().enumerate() {
            self.changing(s.loc);
            let meta = s.meta.as_ref().map(|_| meta_of[i]);
            let (def_gid, cand_noted) = s
                .meta
                .as_ref()
                .map_or((0, 0), |m| (m.def_gid, m.cand_noted));
            let e = if s.cur.is_none() && s.flags & LOADED == 0 {
                UNLOADED
            } else {
                ent(s.cur, s.flags)
            };
            let slot = self.slot(s.loc);
            slot.e = e;
            (slot.save_gid, slot.save_stamp, slot.ginert) = s.save;
            slot.map = s.map;
            slot.mapped = s.mapped;
            slot.base = s.base;
            slot.lazy = s.lazy;
            slot.sym = s.sym;
            slot.def_gid = def_gid;
            slot.cand_noted = cand_noted;
            slot.meta = meta.unwrap_or(0);
        }
        for (l, x, at) in b.extra {
            let e = ent(x, LOADED);
            self.scope_extra.push((l, e, at));
        }
        for (st, xs) in b.cands {
            let xs = xs
                .into_iter()
                .map(|(l, x, c)| (l, x.map_or_else(|| unloaded_stepped(c), |x| ent(x, LOADED))))
                .collect();
            self.cands.push((st, xs));
        }
        self.past = b.past;
        self.past_n = u32::try_from(self.past.len()).unwrap_or(u32::MAX);
        self.read_across = std::cell::RefCell::new(b.read_across);
        self.defined = b.defined;
        self.font_slow.extend(b.font_slow);
        self.stamp = b.stamp;
        self.next_gid = b.next_gid;
        self.pp_dirty = b.pp_dirty;
        self.save = b
            .save
            .into_iter()
            .map(|s| Saved {
                loc: s.loc,
                prev: if s.prev.is_none() && s.flags & LOADED == 0 {
                    UNLOADED
                } else {
                    ent(s.prev, s.flags)
                },
                gid: s.gid,
                stamp: s.stamp,
                at: s.at,
                ldef: s.ldef,
                lazy: s.lazy,
            })
            .collect();
        self.cont_locs = b.carried;
        self.muts += 1;
    }
}

/// The entry of a candidate's register not loaded ([`State::with_base`]):
/// unloaded, and where `c` is not 0 stepped by `c` ([`STEPPED`]).
fn unloaded_stepped(c: i32) -> Entry {
    let mut e = UNLOADED;
    if c != 0 {
        e.flags = STEPPED;
        e.chr = c;
    }
    e
}

impl BaseSlot {
    /// One slot of [`Base::of_value`].
    fn of_value(v: &Values, s: ValueId, loc: &mut dyn FnMut(Field) -> Loc) -> Option<BaseSlot> {
        use crate::val::{int_of, list_of};
        let at = |x: ValueId| match int_of(v, x) {
            0 => Locality::Local,
            1 => Locality::Global,
            2 => Locality::Scope,
            k => Locality::ScopeAt(u8::try_from(k - 3).unwrap_or(0)),
        };
        let u = |x: ValueId| u64::from_ne_bytes(int_of(v, x).to_ne_bytes());
        let ys = list_of(v, s);
        let [
            f,
            has,
            cur,
            flags,
            save_gid,
            save_at,
            ginert,
            keep,
            add,
            mapped,
            base,
            lazy,
            sym,
            eng,
            publ,
            publ_at,
            stamp,
            local,
            gid,
            ix,
            dg,
            cn,
        ] = ys[..]
        else {
            return None;
        };
        let has = int_of(v, has);
        let bit = |k: u32, x: ValueId| (has >> k & 1 != 0).then_some(x);
        let meta = (has >> 3 & 1 != 0).then(|| BaseMeta {
            ix: u32::try_from(int_of(v, ix)).unwrap_or(u32::MAX),
            def_gid: u32::try_from(int_of(v, dg)).unwrap_or(0),
            cand_noted: u32::try_from(int_of(v, cn)).unwrap_or(0),
            eng: (has >> 4 & 1 != 0).then(|| bit(5, eng)),
            publ: (has >> 6 & 1 != 0).then(|| (bit(7, publ), at(publ_at))),
            ldef_stamp: u(stamp),
            ldef_local: int_of(v, local) != 0,
            ldef_gid: u32::try_from(int_of(v, gid)).ok(),
        });
        Some(BaseSlot {
            loc: loc(Field(f)),
            cur: bit(0, cur),
            flags: u8::try_from(int_of(v, flags)).unwrap_or(0),
            save: (
                u32::try_from(int_of(v, save_gid)).unwrap_or(0),
                u(save_at),
                u(ginert),
            ),
            map: Map {
                keep: int_of(v, keep) != 0,
                add: i32::try_from(int_of(v, add)).unwrap_or(0),
            },
            mapped: u8::try_from(int_of(v, mapped)).unwrap_or(0),
            base: bit(1, base),
            lazy: bit(2, lazy),
            sym: int_of(v, sym) != 0,
            meta,
        })
    }
}

impl Base {
    /// As a value, each location as its field (`field`).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn to_value(
        &self,
        v: &Values,
        field: &mut dyn FnMut(Loc) -> Field,
        memo: &mut CkMemo,
    ) -> ValueId {
        let opt = |x: Option<ValueId>| v.list(x.into_iter().collect());
        let at = |l: Locality| {
            v.int(match l {
                Locality::Local => 0,
                Locality::Global => 1,
                Locality::Scope => 2,
                Locality::ScopeAt(k) => 3 + i64::from(k),
            })
        };
        let u = |x: u64| v.int(i64::from_ne_bytes(x.to_ne_bytes()));
        let slots = self
            .slots
            .iter()
            .map(|s| {
                if let Some((was, x)) = memo.slots.get(&s.loc)
                    && was == s
                {
                    return *x;
                }
                // (one flat list: a value absent is UNIT with its bit off
                // in `has`, `BaseSlot::of_value`; a list for each part
                // made most of a checkpoint's cost)
                let val = |x: Option<ValueId>| x.unwrap_or(ValueId::UNIT);
                let m = s.meta.as_ref();
                let eng = m.and_then(|m| m.eng);
                let publ = m.and_then(|m| m.publ);
                let has = i64::from(s.cur.is_some())
                    | i64::from(s.base.is_some()) << 1
                    | i64::from(s.lazy.is_some()) << 2
                    | i64::from(m.is_some()) << 3
                    | i64::from(eng.is_some()) << 4
                    | i64::from(eng.flatten().is_some()) << 5
                    | i64::from(publ.is_some()) << 6
                    | i64::from(publ.and_then(|p| p.0).is_some()) << 7;
                let x = v.list(vec![
                    field(s.loc).0,
                    v.int(has),
                    val(s.cur),
                    v.int(i64::from(s.flags)),
                    v.int(i64::from(s.save.0)),
                    u(s.save.1),
                    u(s.save.2),
                    v.int(i64::from(s.map.keep)),
                    v.int(i64::from(s.map.add)),
                    v.int(i64::from(s.mapped)),
                    val(s.base),
                    val(s.lazy),
                    v.int(i64::from(s.sym)),
                    val(eng.flatten()),
                    val(publ.and_then(|p| p.0)),
                    at(publ.map_or(Locality::Local, |p| p.1)),
                    u(m.map_or(0, |m| m.ldef_stamp)),
                    v.int(m.map_or(0, |m| i64::from(m.ldef_local))),
                    v.int(m.and_then(|m| m.ldef_gid).map_or(-1, i64::from)),
                    v.int(m.map_or(0, |m| i64::from(m.ix))),
                    v.int(m.map_or(0, |m| i64::from(m.def_gid))),
                    v.int(m.map_or(0, |m| i64::from(m.cand_noted))),
                ]);
                memo.slots.insert(s.loc, (s.clone(), x));
                x
            })
            .collect();
        let extra = self
            .extra
            .iter()
            .map(|&(l, x, a)| v.list(vec![field(l).0, opt(x), at(a)]))
            .collect();
        let cands = self
            .cands
            .iter()
            .map(|(st, xs)| {
                // (flat: each entry its field, 0 not loaded, 1 absent or
                // 2 a value, and the value or UNIT)
                let mut ys = Vec::with_capacity(1 + 3 * xs.len());
                ys.push(u(*st));
                // (3: not loaded, stepped, the steps in place of the value)
                for &(l, x, c) in xs {
                    ys.push(field(l).0);
                    ys.push(v.int(match x {
                        None if c != 0 => 3,
                        None => 0,
                        Some(None) => 1,
                        Some(Some(_)) => 2,
                    }));
                    ys.push(if c != 0 && x.is_none() {
                        v.int(i64::from(c))
                    } else {
                        x.flatten().unwrap_or(ValueId::UNIT)
                    });
                }
                v.list(ys)
            })
            .collect();
        let defined = self.defined.iter().map(|&l| field(l).0).collect();
        let font_slow = self.font_slow.iter().map(|&l| field(l).0).collect();
        let save = self
            .save
            .iter()
            .map(|s| {
                v.list(vec![
                    field(s.loc).0,
                    opt(s.prev),
                    v.int(i64::from(s.flags)),
                    v.int(i64::from(s.gid)),
                    u(s.stamp),
                    u(s.at),
                    u(s.ldef.0),
                    v.int(i64::from(s.ldef.1)),
                    v.int(s.ldef.2.map_or(-1, i64::from)),
                    opt(s.lazy),
                ])
            })
            .collect();
        let carried = self.carried.iter().map(|&l| field(l).0).collect();
        let touched: Vec<ValueId> = self
            .touched
            .chunks(TOUCHED_CHUNK)
            .enumerate()
            .map(|(k, ls)| {
                if let Some((was, x)) = memo.touched.get(k)
                    && was[..] == *ls
                {
                    return *x;
                }
                let x = v.list(ls.iter().map(|&l| field(l).0).collect());
                memo.touched.truncate(k);
                memo.touched.push((ls.to_vec(), x));
                x
            })
            .collect();
        // (flat in its chunks: each entry its field and the two candidates)
        let across: Vec<ValueId> = self
            .read_across
            .chunks(TOUCHED_CHUNK)
            .enumerate()
            .map(|(k, es)| {
                if let Some((was, x)) = memo.across.get(k)
                    && was[..] == *es
                {
                    return *x;
                }
                let mut ys = Vec::with_capacity(3 * es.len());
                for &(l, a, b) in es {
                    ys.push(field(l).0);
                    ys.push(v.int(i64::from(a)));
                    ys.push(v.int(i64::from(b)));
                }
                let x = v.list(ys);
                if memo.across.len() > k {
                    memo.across[k] = (es.to_vec(), x);
                } else {
                    memo.across.push((es.to_vec(), x));
                }
                x
            })
            .collect();
        let past: Vec<ValueId> = self
            .past
            .iter()
            .flat_map(|&(st, k)| [u(st), v.int(i64::from(k))])
            .collect();
        v.list(vec![
            v.list(slots),
            v.list(cands),
            v.list(past),
            v.list(across),
            v.list(extra),
            v.list(defined),
            v.list(font_slow),
            u(self.stamp),
            v.int(i64::from(self.next_gid)),
            v.int(i64::from(self.pp_dirty)),
            v.list(save),
            v.list(carried),
            v.list(touched),
        ])
    }

    /// From [`Base::to_value`]'s value, each field's location (`loc`).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn of_value(
        v: &Values,
        x: ValueId,
        loc: &mut dyn FnMut(Field) -> Loc,
    ) -> Option<Base> {
        use crate::val::{int_of, list_of};
        let xs = list_of(v, x);
        let [
            slots,
            cands,
            past,
            across,
            extra,
            defined,
            font_slow,
            stamp,
            next_gid,
            pp_dirty,
            save,
            carried,
            touched,
        ] = xs[..]
        else {
            return None;
        };
        let opt = |x: ValueId| list_of(v, x).first().copied();
        let at = |x: ValueId| match int_of(v, x) {
            0 => Locality::Local,
            1 => Locality::Global,
            2 => Locality::Scope,
            k => Locality::ScopeAt(u8::try_from(k - 3).unwrap_or(0)),
        };
        let u = |x: ValueId| u64::from_ne_bytes(int_of(v, x).to_ne_bytes());
        let mut out = Base {
            slots: Vec::new(),
            cands: Vec::new(),
            past: list_of(v, past)
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[st, k]| (u(st), u32::try_from(int_of(v, k)).unwrap_or(0)))
                .collect(),
            read_across: Vec::new(),
            extra: Vec::new(),
            defined: Vec::new(),
            font_slow: Vec::new(),
            stamp: u(stamp),
            next_gid: u32::try_from(int_of(v, next_gid)).unwrap_or(0),
            pp_dirty: int_of(v, pp_dirty) != 0,
            save: Vec::new(),
            carried: Vec::new(),
            touched: list_of(v, touched)
                .into_iter()
                .flat_map(|c| list_of(v, c))
                .map(|f| loc(Field(f)))
                .collect(),
        };
        for e in list_of(v, save) {
            let [f, prev, flags, gid, stamp, at_, ls, ll, lg, lazy] = list_of(v, e)[..] else {
                return None;
            };
            out.save.push(BaseSave {
                loc: loc(Field(f)),
                prev: opt(prev),
                flags: u8::try_from(int_of(v, flags)).unwrap_or(0),
                gid: u32::try_from(int_of(v, gid)).unwrap_or(0),
                stamp: u(stamp),
                at: u(at_),
                ldef: (u(ls), int_of(v, ll) != 0, u32::try_from(int_of(v, lg)).ok()),
                lazy: opt(lazy),
            });
        }
        out.carried = list_of(v, carried)
            .into_iter()
            .map(|f| loc(Field(f)))
            .collect();
        for s in list_of(v, slots) {
            out.slots.push(BaseSlot::of_value(v, s, loc)?);
        }
        for c in list_of(v, cands) {
            let xs = list_of(v, c);
            let (&st, es) = xs.split_first()?;
            if es.len() % 3 != 0 {
                return None;
            }
            let mut ys = Vec::with_capacity(es.len() / 3);
            for &[f, tag, x] in es.as_chunks::<3>().0 {
                let l = loc(Field(f));
                ys.push(match int_of(v, tag) {
                    0 => (l, None, 0),
                    1 => (l, Some(None), 0),
                    3 => (l, None, i32::try_from(int_of(v, x)).unwrap_or(0)),
                    _ => (l, Some(Some(x)), 0),
                });
            }
            out.cands.push((u(st), ys));
        }
        for e in list_of(v, extra) {
            if let [f, x, a] = list_of(v, e)[..] {
                out.extra.push((loc(Field(f)), opt(x), at(a)));
            }
        }
        out.defined = list_of(v, defined)
            .into_iter()
            .map(|f| loc(Field(f)))
            .collect();
        out.font_slow = list_of(v, font_slow)
            .into_iter()
            .map(|f| loc(Field(f)))
            .collect();
        // (last: a spill location is numbered at its first asking, the
        // table's own locations in their order first, as the run had them)
        out.read_across = list_of(v, across)
            .into_iter()
            .flat_map(|c| list_of(v, c))
            .collect::<Vec<_>>()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[f, a, b]| {
                let n = |x| u32::try_from(int_of(v, x)).unwrap_or(0);
                (loc(Field(f)), n(a), n(b))
            })
            .collect();
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reads a run reports come in the run's own order, whatever the
    /// locations' numbers.
    #[test]
    fn reads_in_the_runs_order_not_by_location() {
        let v = Values::new();
        let mut st = State::new();
        st.load_read(&v, 900, None);
        st.load_read(&v, 7, None);
        st.load_read(&v, 300, None);
        // (a location read, then defined: still a read, through its meta)
        st.set(300, Entry::raw(None), false, None);
        st.load_read(&v, 40, None);
        assert_eq!(st.used_locs(), vec![300, 900, 7, 40]);
    }

    /// A cut inside a group: a global definition made there with no save
    /// of its location open is not carried; a location the open group
    /// saved keeps its slot and its mark.
    /// A font's cell noted past a checkpoint and rolled back: its slot is
    /// gone, so it is no font cell here. Kept, a checkpoint asked this
    /// location's field, had none to find, and the answer poisoned a
    /// thread's names (`cs:` at another run's location: arabtex-doc's
    /// expanded fonts lost their `\fontdimen`s in the run gone on from it).
    #[test]
    fn a_rollback_drops_the_font_cells_of_the_slots_it_drops() {
        let v = Values::new();
        let mut st = State::new();
        let kept = st.spill_loc(v.field("fd:1:1"));
        st.note_font_loc(kept);
        assert!(st.checkpoint());
        let gone = st.spill_loc(v.field("fd:1:2"));
        st.note_font_loc(gone);
        st.rollback();
        assert!(st.field(gone).is_none());
        assert!(st.font_slow.contains(&kept));
        assert!(!st.font_slow.contains(&gone));
    }

    #[test]
    fn a_continuation_carries_only_what_the_engine_does_not_hold() {
        let v = Values::new();
        let mut st = State::new();
        let (g, sb, db, _) = st.open();
        st.set(300, Entry::raw(Some(v.int(1))), true, Some(g));
        st.set(301, Entry::raw(Some(v.int(2))), false, Some(g));
        let c = st.cont(&v, &[g]);
        let locs: Vec<Loc> = c.slots.iter().map(|s| s.loc).collect();
        assert!(!locs.contains(&300));
        assert!(locs.contains(&301));
        let mut r = State::from_cont(&v, c);
        assert!(!r.defined_after(300, 0));
        assert!(!r.is_loaded(300));
        assert!(r.defined_after(301, 0));
        r.set(300, Entry::raw(Some(v.int(5))), false, Some(g));
        assert!(r.is_loaded(300));
        r.close(sb, db);
        assert!(!r.is_loaded(300));
        assert!(!r.is_loaded(301));
    }

    /// A global restore of the value read publishes nothing outside every
    /// scope, and a value inside one; a local write in a closed group
    /// publishes nothing; steps of a register never read are one
    /// relation, and a value once it is read.
    #[test]
    fn the_one_rule() {
        let v = Values::new();
        let mut st = State::new();
        st.load_read(&v, 10, Some(v.int(3)));
        st.set(10, Entry::int(4), true, None);
        st.set(10, Entry::int(3), true, None);
        assert!(st.publication(&v, true).0.is_empty());
        assert_eq!(st.publication(&v, false).0.len(), 1);
        let (g, sb, db, _) = st.open();
        st.set(11, Entry::int(7), false, Some(g));
        st.close(sb, db);
        assert!(st.publication(&v, true).0.iter().all(|p| p.loc != 11));
        assert!(st.add(&v, 12, 1, true));
        assert!(st.add(&v, 12, 2, true));
        assert!(!st.add(&v, 12, 1, false), "a local step after a global one");
        let p = st.publication(&v, true).0;
        assert!(
            p.iter()
                .any(|p| p.loc == 12 && p.what == Pub::Rel(3) && p.at == Locality::Global)
        );
        st.load_read(&v, 12, Some(v.int(5)));
        assert_eq!(st.int_of(12, &v), 8);
        assert_eq!(st.engine_value(12), Some(Some(v.int(5))));
        let p = st.publication(&v, true).0;
        assert!(p.iter().any(|p| p.loc == 12 && p.what == Pub::Rel(3)));
        st.set(12, Entry::int(5), true, None);
        assert!(
            st.publication(&v, true).0.iter().all(|p| p.loc != 12),
            "set back to what it read"
        );
        assert!(st.add(&v, 13, 2, false));
        st.restore(13);
        let (p, _) = st.publication(&v, true);
        assert!(p.iter().all(|p| p.loc != 13));
    }

    /// A global step, then a local write in a group (§283: mparhack's
    /// `\global\advance\c@page\@ne` then `{\advance\c@page\m@ne}` in the
    /// output routine): the close restores the stepped value, and the
    /// step is still the run's global binding, read or not.
    #[test]
    fn a_local_write_in_a_group_keeps_a_global_step() {
        let v = Values::new();
        for read in [true, false] {
            let mut st = State::new();
            assert!(st.add(&v, 12, 1, true));
            if read {
                st.load_read(&v, 12, Some(v.int(1)));
                assert_eq!(st.int_of(12, &v), 2);
            }
            let (g, sb, db, _) = st.open();
            st.set(12, Entry::int(1), false, Some(g));
            assert_eq!(st.int_of(12, &v), 1);
            st.close(sb, db);
            let p = st.publication(&v, true).0;
            assert!(
                p.iter()
                    .any(|p| p.loc == 12 && p.what == Pub::Rel(1) && p.at == Locality::Global),
                "read {read}: the step is not published"
            );
            if !read {
                st.load_read(&v, 12, Some(v.int(1)));
            }
            assert_eq!(st.int_of(12, &v), 2, "read {read}");
        }
    }
}
