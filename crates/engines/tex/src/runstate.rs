//! The run-local state table: every field a run reads or defines, in
//! slots by location ([`Loc`]), each a flat record ([`Entry`]).
//!
//! A run asks the engine for a field once, on its first read (the read
//! set the engine tracks), decodes it once, and from then on reads and
//! writes the slot. A local definition inside a group the run opened is
//! saved on this table's save stack and restored at the group's close;
//! a global one is retained through it (§283). At the end, the
//! definitions still in force at the run's base level are encoded once
//! each and published ([`State::published`]); everything between is
//! private to the run.
//!
//! Locations: the fixed ranges and the meanings of [`crate::fields`]
//! (below [`SPILL`]), and a spill for any other field, by its handle
//! (the keyed families, a few reads a run, never a command's path).

use std::rc::Rc;

use phitex_defuse::{Field, FxHashMap, Locality, ValueId, Values};
use phitex_typeset::glue::Glue;

use crate::val::{Mean, Tok};

/// A location in the table.
pub(crate) type Loc = u32;

/// The bit of a spill location (its index in the spill below it).
pub(crate) const SPILL: Loc = 1 << 31;

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
const K_CHAR: u8 = 10;
const K_UNDEF: u8 = 11;
const K_FONT: u8 = 12;
const K_CHARDEF: u8 = 13;
const K_MATHCHARDEF: u8 = 14;
const K_REG: u8 = 15;
const K_NOTEXP: u8 = 16;

const LOADED: u8 = 1;
const HAS_ID: u8 = 2;
const PRELOADED: u8 = 4;
/// `cmd` and `chr` hold the meaning's command code and modifier
/// (`eq_type`, `equiv`, §221).
const CC: u8 = 8;

/// One slot of the run's table (spec-final §2.1): a meaning's record,
/// `cmd`, `chr` and a `payload` (a macro's handle in the table's macro
/// store, a primitive's name, a glue's or a token list's handle), or an
/// integer in `chr`; the engine value once known (`id`). Copied, never
/// cloned: nothing in it is reference counted.
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
        }
    }
}

/// Whether `m` is a definition of a command's transient state outside
/// the table of equivalents (§1266 `after_token`: set by one command,
/// emptied by the next assignment, never saved or restored by a group)
/// that leaves the value the run read before it: then it changes
/// nothing at any position, in a scope or not, and is not published (an
/// empty token and no definition are the same). A run that reads the
/// state and leaves another value (sets a token and leaves it pending,
/// or empties one a run before it left pending) still publishes it.
fn unchanged_transient(m: &Meta, x: ValueId) -> bool {
    m.loc == crate::fields::cell::AFTERTOKEN
        && match m.eng {
            Some(Some(e)) => e == x,
            Some(None) => x == ValueId::UNIT,
            None => false,
        }
}

/// What a run defined at a location.
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
}

/// A latest-definition mark: its stamp, whether local, its group.
pub(crate) type Ldef = (u64, bool, Option<u32>);

/// The table.
pub(crate) struct State {
    eq: Vec<Slot>,
    spill: Vec<Slot>,
    spill_ix: FxHashMap<Field, u32>,
    touched: Vec<Loc>,
    metas: Vec<Meta>,
    /// The scope's definitions of locations the run's base level also
    /// publishes, at another value: the field's second entry, the scope's
    /// ([`State::abandon_to_scope`]); with several frames left open, each
    /// frame's locals at its own frame ([`State::abandon_to_frames`],
    /// `Locality::ScopeAt`). Made as the run ends, never parked.
    scope_extra: Vec<(Loc, Entry, Locality)>,
    /// The values the slots hold by handle (an entry's `payload`): the
    /// glues, token lists and macros decoded or defined in this run,
    /// and their engine values once encoded.
    glues: Vec<Glue>,
    toks: Vec<Rc<[Tok]>>,
    macros: Vec<Rc<crate::val::Macro>>,
    save: Vec<Saved>,
    /// Locations defined locally inside a group of the run (for a group
    /// left open: its definitions become the run's).
    defined: Vec<Loc>,
    stamp: u64,
    /// The locations defined at the base level (or globally) since the
    /// last [`State::take_base_defs`].
    base_defs: Vec<Loc>,
    /// A page parameter (§980: `\\vsize`, `\\maxdepth`, `\\topskip`,
    /// `\\holdinginserts`) defined at any level, or restored at a
    /// group's end, since the last [`State::take_page_params`]: the
    /// page builder reads eqtb as it stands (§987), groups or not.
    pp_dirty: bool,
    /// In a page step: each location it defined, and whether only by
    /// `\advance` (its value then the one it read plus a change), for
    /// the writer's fold of the chain ([`State::advanced_only`]).
    kinds: Option<FxHashMap<Loc, bool>>,
    next_gid: u32,
    /// The locations a continuation's table brought ([`State::from_cont`]):
    /// a group-local binding it republishes is no definition where it
    /// equals the value reaching the run ([`State::drop_publ`]).
    cont_locs: Vec<Loc>,
    /// Bumped whenever a category code or another code (`\\lccode`,
    /// `\\uccode`, `\\sfcode`, `\\mathcode`, `\\delcode`) below 256 may
    /// have changed
    /// ([`State::cat_gen`]); unique across tables of a thread, so a
    /// table put in place of another is never mistaken for it.
    cat_gen: u64,
    /// Bumped whenever any slot's value may have changed (a definition,
    /// a restore, a preload): the run's memos of decoded state
    /// (`Run::font_entry`) are valid while it stands.
    any_gen: u64,
    /// Bumped by every change of the table a command could not undo by
    /// rewinding its input (a definition, a save, a restore, a close):
    /// the table's part of the run's mutation counter
    /// (`Run::mutations`, spec-final §2.4).
    muts: u64,
    /// The page-builder candidates the run passed
    /// ([`State::mark_candidate`]): each one's stamp and, for each
    /// location defined after it and before the next one for the first
    /// time since it, the entry the location held at the candidate.
    cands: Vec<(u64, Vec<(Loc, Entry)>)>,
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
    /// The touched lists of finished runs, emptied (a run touches a few
    /// hundred locations: the list is not grown again run after run).
    static TOUCHED_POOL: std::cell::RefCell<Vec<Vec<Loc>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Clone for State {
    fn clone(&self) -> State {
        // (a pooled table is all default slots, and a slot of this one
        // that is not is in its touched list, [`State::slot`]: those
        // copied, not the whole table, which spans every control
        // sequence the thread's runs have seen)
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
            touched: self.touched.clone(),
            metas: self.metas.clone(),
            scope_extra: self.scope_extra.clone(),
            save: self.save.clone(),
            defined: self.defined.clone(),
            stamp: self.stamp,
            base_defs: self.base_defs.clone(),
            pp_dirty: self.pp_dirty,
            kinds: self.kinds.clone(),
            next_gid: self.next_gid,
            cont_locs: self.cont_locs.clone(),
            glues: self.glues.clone(),
            toks: self.toks.clone(),
            macros: self.macros.clone(),
            cat_gen: new_cat_gen(),
            any_gen: new_cat_gen(),
            muts: self.muts,
            cands: self.cands.clone(),
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

/// A location's value at a candidate ([`State::at_candidates`]): the
/// candidate's index, and the value (`None`: not known there).
#[allow(clippy::option_option)]
type AtCandidate = (usize, Option<Option<ValueId>>);

/// The values at the candidates of the locations defined after them
/// ([`State::at_candidates`]).
pub(crate) struct AtCandidates {
    by_loc: FxHashMap<Loc, Vec<AtCandidate>>,
}

impl AtCandidates {
    /// `loc`'s value at candidate `i`: `None` where nothing after the
    /// candidate defined it.
    #[allow(clippy::option_option)]
    pub(crate) fn get(&self, i: usize, loc: Loc) -> Option<Option<Option<ValueId>>> {
        let h = self.by_loc.get(&loc)?;
        h.get(h.partition_point(|x| x.0 < i)).map(|x| x.1)
    }

    /// Every location defined after some candidate.
    pub(crate) fn locs(&self) -> impl Iterator<Item = Loc> + '_ {
        self.by_loc.keys().copied()
    }
}

impl State {
    pub(crate) fn new() -> State {
        State {
            eq: POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default(),
            spill: Vec::new(),
            spill_ix: FxHashMap::default(),
            touched: TOUCHED_POOL
                .with(|p| p.borrow_mut().pop())
                .unwrap_or_else(|| Vec::with_capacity(256)),
            metas: Vec::new(),
            scope_extra: Vec::new(),
            save: Vec::new(),
            defined: Vec::new(),
            stamp: 0,
            base_defs: Vec::new(),
            pp_dirty: false,
            kinds: None,
            next_gid: 0,
            cont_locs: Vec::new(),
            glues: Vec::new(),
            toks: Vec::new(),
            macros: Vec::new(),
            cat_gen: new_cat_gen(),
            any_gen: new_cat_gen(),
            muts: 0,
            cands: Vec::new(),
        }
    }

    /// A page-builder candidate passed: the entries the locations
    /// defined after it hold here are kept ([`State::at_candidate`]).
    pub(crate) fn mark_candidate(&mut self) {
        self.cands.push((self.stamp, Vec::new()));
    }

    /// Notes `loc`'s entry before a definition, where it is the first
    /// since the last candidate (its latest definition `ldef` before it).
    #[inline]
    fn note_cand(&mut self, loc: Loc, ldef: u64, prev: Entry) {
        if let Some((st, xs)) = self.cands.last_mut()
            && ldef <= *st
        {
            xs.push((loc, prev));
        }
    }

    /// The values at the candidates of the locations defined after them:
    /// for each candidate passed, in order, each such location with its
    /// engine value there (`None`: not known here, the run had neither
    /// read nor defined it before).
    /// Kept by location: a location's value at candidate `i` is its
    /// first entry at a candidate from `i` on ([`AtCandidates::get`]),
    /// each entry encoded once, no map per candidate.
    pub(crate) fn at_candidates(&self, v: &Values) -> AtCandidates {
        let mut by_loc: FxHashMap<Loc, Vec<AtCandidate>> = FxHashMap::default();
        for (i, (_, xs)) in self.cands.iter().enumerate() {
            for &(l, e) in xs {
                let h = by_loc.entry(l).or_default();
                // (the first entry at a candidate is the value there)
                if h.last().is_some_and(|x| x.0 == i) {
                    continue;
                }
                let x = (e.flags & LOADED != 0).then(|| self.encode(&e, v));
                h.push((i, x));
            }
        }
        AtCandidates { by_loc }
    }

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

    #[inline]
    fn slot_ref(&self, loc: Loc) -> Option<&Slot> {
        if loc & SPILL == 0 {
            self.eq.get(loc as usize)
        } else {
            self.spill.get((loc & !SPILL) as usize)
        }
    }

    fn slot(&mut self, loc: Loc) -> &mut Slot {
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
        let fields: Vec<(Field, u32)> = self.spill_ix.iter().map(|(&f, &i)| (f, i)).collect();
        for (f, i) in fields {
            self.slot(SPILL | i).field = Some(f);
        }
    }

    /// The spill location of field `f`.
    pub(crate) fn spill_loc(&mut self, f: Field) -> Loc {
        if let Some(&i) = self.spill_ix.get(&f) {
            return SPILL | i;
        }
        let i = u32::try_from(self.spill.len()).unwrap_or(0);
        self.spill_ix.insert(f, i);
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
        (s.e.flags & (LOADED | CC) == (LOADED | CC)).then_some((s.e.cmd, s.e.chr))
    }

    #[inline]
    pub(crate) fn is_loaded(&self, loc: Loc) -> bool {
        self.slot_ref(loc).is_some_and(|s| s.e.flags & LOADED != 0)
    }

    /// The engine's value of `loc`, read: `Unloaded -> Loaded`.
    pub(crate) fn fill(&mut self, loc: Loc, x: Option<ValueId>) {
        let mut e = Entry::raw(x);
        e.flags = LOADED | HAS_ID;
        self.slot(loc).e = e;
    }

    /// A value read in place of the engine's (a page step's `\count0..9`
    /// as the step before left them): loaded, and never compared with
    /// the engine's at the end.
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
            "touched {} in-touched-list {} save_gid {} ginert {} ldef {:?} defined-list {}",
            s.touched,
            self.touched.contains(&loc),
            s.save_gid,
            s.ginert,
            m.map(|m| (m.ldef_local, m.ldef_gid, m.ldef_stamp)),
            self.defined.contains(&loc)
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

    pub(crate) fn preload(&mut self, loc: Loc, x: Option<ValueId>) {
        self.any_gen = new_cat_gen();
        self.muts += 1;
        if is_cat(loc) {
            self.cat_gen = new_cat_gen();
        }
        let mut e = Entry::raw(x);
        e.flags = LOADED | HAS_ID | PRELOADED;
        self.slot(loc).e = e;
    }

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

    /// The meaning a meaning entry holds (a macro's body shared).
    fn mean_of_entry(&self, e: &Entry) -> Mean {
        match e.kind {
            K_PRIM => Mean::Prim(crate::val::prim_by_handle(e.payload), e.cmd, e.chr),
            K_MACRO => Mean::Macro(self.macros[e.payload as usize].clone()),
            K_CHAR => Mean::Char(e.cmd, e.chr.cast_unsigned()),
            K_FONT => Mean::Font(i64::from(e.chr)),
            K_CHARDEF => Mean::CharDef(i64::from(e.chr)),
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

    /// The entry of token list `t`.
    #[allow(dead_code)]
    pub(crate) fn toks_entry(&mut self, t: Rc<[Tok]>) -> Entry {
        self.toks.push(t);
        Entry {
            kind: K_TOKS,
            flags: 0,
            cmd: 0,
            chr: 0,
            payload: u32::try_from(self.toks.len() - 1).unwrap_or(0),
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
            Mean::MathCharDef(_) => (K_MATHCHARDEF, 0),
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
        if loc & SPILL == 0 {
            &mut self.eq[loc as usize].e
        } else {
            &mut self.spill[(loc & !SPILL) as usize].e
        }
    }

    /// The local definitions at the base level of locations a
    /// continuation's table brought (its open groups' bindings, which
    /// the run republishes), with their values.
    pub(crate) fn cont_republished(&self, v: &Values) -> Vec<(Loc, ValueId)> {
        let mut out = Vec::new();
        for &l in &self.cont_locs {
            let Some(s) = self.slot_ref(l) else { continue };
            let Some(m) = usize::try_from(s.meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| self.metas.get(m - 1))
            else {
                continue;
            };
            if let Some((val, Locality::Local)) = &m.publ
                && let Some(x) = self.encode(val, v)
            {
                out.push((l, x));
            }
        }
        out
    }

    /// `loc`'s local definition at the base level is no definition (its
    /// value is the one reaching the run): not published.
    pub(crate) fn drop_publ(&mut self, loc: Loc) {
        self.muts += 1;
        let Some(m) = self
            .slot_ref(loc)
            .and_then(|s| usize::try_from(s.meta).ok())
            .filter(|&m| m > 0)
        else {
            return;
        };
        if let Some(m) = self.metas.get_mut(m - 1)
            && matches!(m.publ, Some((_, Locality::Local)))
        {
            m.publ = None;
        }
    }

    /// The records ([`crate::fields::record_of`]) a cell of which the run
    /// defined, at any level.
    pub(crate) fn defined_records(&self) -> [bool; crate::fields::NREC] {
        let mut out = [false; crate::fields::NREC];
        for m in &self.metas {
            if let Some((b, _)) = crate::fields::record_of(m.loc as usize) {
                out[b] = true;
            }
        }
        out
    }

    /// The engine's value of `loc` as the run read it, before any
    /// definition of its own (`None`: not known here).
    #[allow(clippy::option_option)]
    pub(crate) fn engine_value(&self, loc: Loc) -> Option<Option<ValueId>> {
        let s = self.slot_ref(loc)?;
        if s.meta != 0 {
            return self.metas.get(s.meta as usize - 1).and_then(|m| m.eng);
        }
        (s.e.flags & (LOADED | HAS_ID | PRELOADED) == LOADED | HAS_ID).then_some(s.e.id)
    }

    /// The engine value of loaded `loc` (encoded once).
    pub(crate) fn id_of(&mut self, loc: Loc, v: &Values) -> Option<ValueId> {
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

    /// The command code and modifier of loaded meaning `loc` (decoded
    /// once; nothing cloned).
    #[inline]
    pub(crate) fn cmd_chr_of(&mut self, loc: Loc, v: &Values) -> (u8, i32) {
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

    /// The meta of `loc`, made on its first definition.
    fn meta(&mut self, loc: Loc) -> usize {
        let m = self.slot(loc).meta;
        if m != 0 {
            return m as usize - 1;
        }
        let s = self.slot(loc);
        let e = &s.e;
        let eng = (e.flags & LOADED != 0 && e.flags & PRELOADED == 0).then_some(e.id);
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

    /// Defines `loc` (§277–§279: `eq_define` / `geq_define`): `grp` is the
    /// innermost group of the run open now, if any. A local definition
    /// inside it saves the value it hides, once per group while that
    /// value is the group's own (§277, §283: after a global definition
    /// the next local one saves again); a global one makes the saves
    /// before it inert. A definition outside every group of the run, or
    /// a global one, is the run's for its base level.
    pub(crate) fn set(&mut self, loc: Loc, val: Entry, global: bool, grp: Option<u32>) {
        self.any_gen = new_cat_gen();
        self.muts += 1;
        if is_cat(loc) {
            self.cat_gen = new_cat_gen();
        }
        let mi = self.meta(loc);
        self.stamp += 1;
        let st = self.stamp;
        let ldef = {
            let m = &self.metas[mi];
            (m.ldef_stamp, m.ldef_local, m.ldef_gid)
        };
        if !self.cands.is_empty() {
            let prev = self.slot(loc).e;
            self.note_cand(loc, ldef.0, prev);
        }
        let s = self.slot(loc);
        if global {
            s.ginert = st;
        } else if let Some(g) = grp
            && !(s.save_gid == g && s.ginert <= s.save_stamp)
        {
            let saved = Saved {
                loc,
                prev: s.e,
                gid: s.save_gid,
                stamp: s.save_stamp,
                at: st,
                ldef,
            };
            s.save_gid = g;
            s.save_stamp = st;
            self.save.push(saved);
        }
        let s = self.slot(loc);
        s.e = stored(val);
        if !global && grp.is_some() {
            self.defined.push(loc);
        }
        let m = &mut self.metas[mi];
        m.ldef_stamp = st;
        m.ldef_local = !global;
        m.ldef_gid = if global { None } else { grp };
        if global {
            m.publ = Some((val, Locality::Global));
        } else if grp.is_none() {
            m.publ = Some((val, Locality::Local));
        }
        if global || grp.is_none() {
            self.base_defs.push(loc);
        }
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
    }

    /// Notes a definition of `loc`, an `\advance` or not, when watching.
    pub(crate) fn note_kind(&mut self, loc: Loc, advance: bool) {
        if let Some(k) = &mut self.kinds {
            k.entry(loc)
                .and_modify(|a| *a &= advance)
                .or_insert(advance);
        }
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
        self.any_gen = new_cat_gen();
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
        // (`\multiply\vsize` changes a page parameter as `\vsize=`
        // does: the next candidate notes it, [`State::take_page_params`])
        if is_page_param(loc) {
            self.pp_dirty = true;
        }
        // (a group's binding changed in place stays that group's: its
        // local definition, by the group that saved it when the last
        // definition does not say; `\multiply\doublecol@number` in
        // multicol's environment group)
        let save_gid = self.slot_ref(loc).map_or(0, |s| s.save_gid);
        let m = &mut self.metas[mi];
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

    /// Closes a group whose saves and definitions began at `save_base`
    /// and `def_base`: its saves are restored (the inert ones, hidden by
    /// a global definition since, are not, §283).
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

    pub(crate) fn close(&mut self, save_base: usize, def_base: usize) {
        self.muts += 1;
        while self.save.len() > save_base {
            let Some(s) = self.save.pop() else { break };
            let slot = self.slot(s.loc);
            if slot.ginert > s.at {
                continue;
            }
            self.any_gen = new_cat_gen();
            if is_cat(s.loc) {
                self.cat_gen = new_cat_gen();
            }
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
            slot.save_gid = s.gid;
            slot.save_stamp = s.stamp;
            let meta = slot.meta;
            if let Some(m) = usize::try_from(meta)
                .ok()
                .filter(|&m| m > 0)
                .and_then(|m| self.metas.get_mut(m - 1))
            {
                (m.ldef_stamp, m.ldef_local, m.ldef_gid) = s.ldef;
            }
        }
        self.defined.truncate(def_base.min(self.defined.len()));
    }

    /// Leaves the groups from the one whose saves and definitions began
    /// at `save_base` and `def_base` (opened at `open_stamp`) open: they
    /// are not closed here (the engine's scope, or the end of the run),
    /// so the local definitions made inside them are the run's own.
    pub(crate) fn abandon(&mut self, save_base: usize, def_base: usize, open_stamp: u64) {
        self.muts += 1;
        let locs: Vec<Loc> = self.defined[def_base.min(self.defined.len())..].to_vec();
        for loc in locs {
            let mi = self.meta(loc);
            if self.metas[mi].ldef_local && self.metas[mi].ldef_stamp > open_stamp {
                let val = self.slot(loc).e;
                self.metas[mi].publ = Some((val, Locality::Local));
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

    /// Whether the latest definition of `loc` stands at the run's end
    /// as its own: global, or local outside every group of the run (a
    /// local one in a group is restored at the group's close).
    pub(crate) fn def_stands(&self, loc: Loc) -> bool {
        self.slot_ref(loc)
            .and_then(|s| usize::try_from(s.meta).ok().filter(|&m| m > 0))
            .and_then(|m| self.metas.get(m - 1))
            .is_some_and(|m| !m.ldef_local || m.ldef_gid.is_none())
    }

    /// The groups opened so far: a group [`State::open`] opens after
    /// this has a greater number.
    pub(crate) fn gid_mark(&self) -> u32 {
        self.next_gid
    }

    /// The locations whose value the run read from the engine: read
    /// before the run defined them, or never defined (a value preloaded
    /// in place of the engine's is not one). In the run's own order (its
    /// definitions' bookkeeping first, then the rest as first touched),
    /// never by location: a meaning's location is its name's number in
    /// the process's interner, which numbers names in the order the
    /// threads first meet them, so a list in location order differs
    /// between two builds of one text (each location appears once: one
    /// meta a location, one place in the touched list).
    pub(crate) fn read_locs(&self) -> Vec<Loc> {
        let mut out: Vec<Loc> = self
            .metas
            .iter()
            .filter(|m| m.eng.is_some())
            .map(|m| m.loc)
            .collect();
        let has_meta: phitex_defuse::FxHashSet<Loc> = self.metas.iter().map(|m| m.loc).collect();
        for &l in &self.touched {
            if has_meta.contains(&l) {
                continue;
            }
            if self
                .slot_ref(l)
                .is_some_and(|s| s.e.flags & LOADED != 0 && s.e.flags & PRELOADED == 0)
            {
                out.push(l);
            }
        }
        out
    }

    /// [`State::read_locs`] with the values read (`None`: undefined in
    /// the engine), in the same order.
    pub(crate) fn read_values(&self) -> Vec<(Loc, Option<ValueId>)> {
        let mut out: Vec<(Loc, Option<ValueId>)> = self
            .metas
            .iter()
            .filter_map(|m| m.eng.map(|e| (m.loc, e)))
            .collect();
        let has_meta: phitex_defuse::FxHashSet<Loc> = self.metas.iter().map(|m| m.loc).collect();
        for &l in &self.touched {
            if has_meta.contains(&l) {
                continue;
            }
            if let Some(s) = self.slot_ref(l)
                && s.e.flags & LOADED != 0
                && s.e.flags & PRELOADED == 0
            {
                out.push((l, s.e.id));
            }
        }
        out
    }

    /// The locations the run defined at its base level back to the
    /// engine's value it read first (restored: no net effect).
    pub(crate) fn restored_locs(&self, v: &Values) -> phitex_defuse::FxHashSet<Loc> {
        self.metas
            .iter()
            .filter(|m| {
                m.publ.as_ref().is_some_and(|(val, _)| {
                    m.eng
                        .is_some_and(|e| e.is_some() && self.encode(val, v) == e)
                })
            })
            .map(|m| m.loc)
            .collect()
    }

    /// The definitions made after stamp `since`, at any level (a line
    /// read back is interpreted wherever the run is): the location, the
    /// value in force and the locality of the latest one. A local one
    /// made in a group opened after `gids` (see [`State::gid_mark`]) is
    /// left out: that group has ended (§282), its value restored.
    pub(crate) fn defined_since(
        &mut self,
        v: &Values,
        since: u64,
        gids: u32,
    ) -> Vec<(Loc, Option<ValueId>, Locality)> {
        self.defined_since_by(v, since, gids, false)
    }

    /// Every location the run defined: its latest definition's stamp,
    /// whether local, and the group it was made in. [`State::defined_since`]
    /// of any stamp and group mark is these filtered as it filters them
    /// (`stamp > since`, no local one of a group opened after `gids`), in
    /// this order.
    pub(crate) fn defined_stamped(&self) -> Vec<(Loc, u64, bool, Option<u32>)> {
        self.metas
            .iter()
            .filter(|m| m.ldef_stamp > 0)
            .map(|m| (m.loc, m.ldef_stamp, m.ldef_local, m.ldef_gid))
            .collect()
    }

    /// [`State::defined_since`] in the order of the latest definitions
    /// (their stamps), not of the locations' first definitions in the
    /// run: what a stretch of the run defined, listed as that stretch
    /// alone decides, whatever the run touched before it.
    pub(crate) fn defined_since_in_order(
        &mut self,
        v: &Values,
        since: u64,
        gids: u32,
    ) -> Vec<(Loc, Option<ValueId>, Locality)> {
        self.defined_since_by(v, since, gids, true)
    }

    fn defined_since_by(
        &mut self,
        v: &Values,
        since: u64,
        gids: u32,
        by_stamp: bool,
    ) -> Vec<(Loc, Option<ValueId>, Locality)> {
        let mut hits: Vec<(Loc, bool, u64)> = self
            .metas
            .iter()
            .filter(|m| m.ldef_stamp > since)
            .filter(|m| !(m.ldef_local && m.ldef_gid.is_some_and(|g| g > gids)))
            .map(|m| (m.loc, m.ldef_local, m.ldef_stamp))
            .collect();
        if by_stamp {
            hits.sort_by_key(|h| h.2);
        }
        hits.into_iter()
            .map(|(loc, local, _)| {
                let x = self.id_of(loc, v);
                let l = if local {
                    Locality::Local
                } else {
                    Locality::Global
                };
                (loc, x, l)
            })
            .collect()
    }

    /// The definitions to publish: each location's definition at the base
    /// level, encoded once; a local one equal to the engine's value the
    /// run read is left out (the same binding). `top`: a global one too
    /// (a run at the unit's top level, no scope open around it, where a
    /// global definition equal to the value in force changes nothing:
    /// only the run's net effect is published; inside a scope, `\global`
    /// of the value a local binding holds outlives the scope's close,
    /// §283).
    pub(crate) fn published(&self, v: &Values, top: bool) -> Vec<(Loc, ValueId, Locality)> {
        let mut out = Vec::with_capacity(self.metas.len());
        for m in &self.metas {
            let Some((val, l)) = &m.publ else { continue };
            let Some(x) = self.encode(val, v) else {
                continue;
            };
            if (top || *l != Locality::Global) && m.eng == Some(Some(x)) {
                continue;
            }
            if unchanged_transient(m, x) {
                continue;
            }
            out.push((m.loc, x, *l));
        }
        for (loc, val, l) in &self.scope_extra {
            if let Some(x) = self.encode(val, v) {
                out.push((*loc, x, *l));
            }
        }
        out
    }

    /// Whether the run has a local definition at its base level that
    /// changes the engine's value (one [`State::published`] keeps), made
    /// outside every group of the run: while a group is open, a
    /// definition inside it leaves `publ` as the base level's.
    pub(crate) fn has_base_local_def(&self, v: &Values) -> bool {
        self.metas.iter().any(|m| match &m.publ {
            Some((val, Locality::Local)) => {
                self.encode(val, v).is_some_and(|x| m.eng != Some(Some(x)))
            }
            _ => false,
        })
    }

    /// The value `m` publishes at the run's base level, if any (a local
    /// one equal to the engine's value it read publishes nothing, nor a
    /// global one at the top level, [`State::published`]).
    fn base_publish(&self, m: &Meta, v: &Values, top: bool) -> Option<ValueId> {
        let (val, l) = m.publ.as_ref()?;
        if *l == Locality::Scope {
            return None;
        }
        let x = self.encode(val, v)?;
        if ((top || *l == Locality::Local) && m.eng == Some(Some(x))) || unchanged_transient(m, x) {
            None
        } else {
            Some(x)
        }
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

    /// Leaves the group whose saves and definitions began at `save_base`
    /// and `def_base` (opened at `open_stamp`) to the engine as the scope
    /// the run opens after itself: its local definitions are published
    /// as the scope's (`Locality::Scope`, `Ctx::define_in_scope`), the
    /// base level's stay the run's; a group's local of the value the base
    /// level publishes is that definition seen inside; of another value,
    /// a second entry of the field, the scope's ([`State::scope_extra`]).
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
                    self.metas[mi].publ = Some((val, Locality::Scope));
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
    /// group's local definitions, at the value in force at its level,
    /// are frame `k`'s (`Locality::ScopeAt(k)`); the base level's stay
    /// the run's. A location an inner group defined again holds the
    /// group's value in the inner group's first save of it (what that
    /// group's close restores, §282; a save hidden by a global
    /// definition since restores nothing, §283).
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
        if let Some((_, l)) = &mut self.metas[mi].publ {
            *l = Locality::Scope;
        }
    }

    /// The table without `Rc`, for a parked run ([`PState`]): the table
    /// the run had, so the run resumed from it holds what a run that never
    /// parked would (whether a run parks is the schedule's choice): every
    /// location it touched in its order, its engine reads with their
    /// values (they still hold when the engine gives the park back), its
    /// definitions' bookkeeping in its order.
    pub(crate) fn park(&mut self, v: &Values) -> PState {
        let ploc = |s: &State, l: Loc| -> PLoc {
            if l & SPILL == 0 {
                PLoc::Fixed(l)
            } else {
                PLoc::Spill(s.field(l).unwrap_or(Field(ValueId::UNIT)))
            }
        };
        let pent = |e: &Entry| -> PEntry {
            let id = if e.flags & HAS_ID != 0 {
                e.id
            } else {
                self.encode(e, v)
            };
            PEntry {
                id,
                flags: (e.flags | HAS_ID) & !CC,
            }
        };
        let mut slots = Vec::new();
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            if s.meta == 0 && s.save_gid == 0 && s.ginert == 0 && s.e.flags & LOADED == 0 {
                continue;
            }
            let meta = (s.meta != 0).then(|| {
                let m = &self.metas[s.meta as usize - 1];
                PMeta {
                    eng: m.eng,
                    publ: m.publ.as_ref().map(|(x, l)| (self.encode(x, v), *l)),
                    ldef_stamp: m.ldef_stamp,
                    ldef_local: m.ldef_local,
                    ldef_gid: m.ldef_gid,
                }
            });
            slots.push(PSlot {
                loc: ploc(self, l),
                e: pent(&s.e),
                save_gid: s.save_gid,
                save_stamp: s.save_stamp,
                ginert: s.ginert,
                meta,
            });
        }
        let save = self
            .save
            .iter()
            .map(|s| PSaved {
                loc: ploc(self, s.loc),
                prev: pent(&s.prev),
                gid: s.gid,
                stamp: s.stamp,
                at: s.at,
                ldef: s.ldef,
            })
            .collect();
        let defined = self.defined.iter().map(|&l| ploc(self, l)).collect();
        let base_defs = self.base_defs.iter().map(|&l| ploc(self, l)).collect();
        let kinds = self
            .kinds
            .as_ref()
            .map(|k| k.iter().map(|(&l, &a)| (ploc(self, l), a)).collect());
        let cont_locs = self.cont_locs.iter().map(|&l| ploc(self, l)).collect();
        let cands = self
            .cands
            .iter()
            .map(|(st, xs)| {
                (
                    *st,
                    xs.iter().map(|(l, e)| (ploc(self, *l), pent(e))).collect(),
                )
            })
            .collect();
        let touched = self.touched.iter().map(|&l| ploc(self, l)).collect();
        let meta_order = self.metas.iter().map(|m| ploc(self, m.loc)).collect();
        PState {
            cands,
            cont_locs,
            touched,
            meta_order,
            slots,
            save,
            defined,
            base_defs,
            pp_dirty: self.pp_dirty,
            kinds,
            stamp: self.stamp,
            next_gid: self.next_gid,
        }
    }

    /// The table of a parked run ([`State::park`]).
    pub(crate) fn unpark(p: PState) -> State {
        let mut st = State::new();
        let loc = |st: &mut State, l: PLoc| -> Loc {
            match l {
                PLoc::Fixed(l) => l,
                PLoc::Spill(f) => st.spill_loc(f),
            }
        };
        let ent = |e: PEntry| {
            let mut x = Entry::raw(e.id);
            x.flags = e.flags;
            x
        };
        // (the touched list in the run's order first: the reads it
        // reports follow it, [`State::read_locs`])
        for l in p.touched {
            let l = loc(&mut st, l);
            st.slot(l);
        }
        // (the metas in the run's order: its definitions' bookkeeping
        // comes first in the reads it reports)
        let order: Vec<Loc> = p.meta_order.into_iter().map(|l| loc(&mut st, l)).collect();
        let at: FxHashMap<Loc, usize> = order.iter().enumerate().map(|(i, &l)| (l, i)).collect();
        let mut metas: Vec<Option<Meta>> = (0..order.len()).map(|_| None).collect();
        for s in p.slots {
            let l = loc(&mut st, s.loc);
            if let Some(m) = s.meta
                && let Some(&i) = at.get(&l)
            {
                metas[i] = Some(Meta {
                    loc: l,
                    eng: m.eng,
                    publ: m.publ.map(|(x, lc)| (Entry::raw(x), lc)),
                    ldef_stamp: m.ldef_stamp,
                    ldef_local: m.ldef_local,
                    ldef_gid: m.ldef_gid,
                });
            }
            let slot = st.slot(l);
            slot.e = ent(s.e);
            slot.save_gid = s.save_gid;
            slot.save_stamp = s.save_stamp;
            slot.ginert = s.ginert;
        }
        for m in metas.into_iter().flatten() {
            st.metas.push(m);
            let i = u32::try_from(st.metas.len()).unwrap_or(0);
            let l = st.metas[st.metas.len() - 1].loc;
            st.slot(l).meta = i;
        }
        for s in p.save {
            let l = loc(&mut st, s.loc);
            st.save.push(Saved {
                loc: l,
                prev: ent(s.prev),
                gid: s.gid,
                stamp: s.stamp,
                at: s.at,
                ldef: s.ldef,
            });
        }
        for d in p.defined {
            let l = loc(&mut st, d);
            st.defined.push(l);
        }
        for d in p.base_defs {
            let l = loc(&mut st, d);
            st.base_defs.push(l);
        }
        for d in p.cont_locs {
            let l = loc(&mut st, d);
            st.cont_locs.push(l);
        }
        st.pp_dirty = p.pp_dirty;
        if let Some(ks) = p.kinds {
            let mut m = FxHashMap::default();
            for (d, a) in ks {
                let l = loc(&mut st, d);
                m.insert(l, a);
            }
            st.kinds = Some(m);
        }
        for (stamp, xs) in p.cands {
            let mut ys = Vec::with_capacity(xs.len());
            for (l, e) in xs {
                let l = loc(&mut st, l);
                ys.push((l, ent(e)));
            }
            st.cands.push((stamp, ys));
        }
        st.stamp = p.stamp;
        st.next_gid = p.next_gid;
        st.cat_gen = new_cat_gen();
        st.any_gen = new_cat_gen();
        st
    }
}

/// A location of a parked table: a fixed one or a meaning's (stable for
/// the process), or a spill one by its field.
enum PLoc {
    Fixed(Loc),
    Spill(Field),
}

struct PEntry {
    id: Option<ValueId>,
    flags: u8,
}

struct PMeta {
    // None: not read; Some(None): read, undefined in the engine
    #[allow(clippy::option_option)]
    eng: Option<Option<ValueId>>,
    publ: Option<(Option<ValueId>, Locality)>,
    ldef_stamp: u64,
    ldef_local: bool,
    ldef_gid: Option<u32>,
}

struct PSlot {
    loc: PLoc,
    e: PEntry,
    save_gid: u32,
    save_stamp: u64,
    ginert: u64,
    meta: Option<PMeta>,
}

struct PSaved {
    loc: PLoc,
    prev: PEntry,
    gid: u32,
    stamp: u64,
    at: u64,
    ldef: Ldef,
}

/// A run's table, parked ([`State::park`]).
pub(crate) struct PState {
    cands: Vec<(u64, Vec<(PLoc, PEntry)>)>,
    cont_locs: Vec<PLoc>,
    /// Every location the run touched, in its order.
    touched: Vec<PLoc>,
    /// The locations of the run's metas, in its order.
    meta_order: Vec<PLoc>,
    slots: Vec<PSlot>,
    save: Vec<PSaved>,
    defined: Vec<PLoc>,
    base_defs: Vec<PLoc>,
    pp_dirty: bool,
    kinds: Option<Vec<(PLoc, bool)>>,
    stamp: u64,
    next_gid: u32,
}

/// An entry as a definition stores it: loaded, its engine value and
/// meaning record kept where known.
fn stored(val: Entry) -> Entry {
    let mut e = val;
    e.flags = LOADED | (val.flags & (HAS_ID | CC));
    e
}

/// Whether `loc` is one of the page builder's parameters (§980).
fn is_page_param(loc: Loc) -> bool {
    use crate::fields::Par;
    [
        Par::vsize as Loc,
        Par::maxdepth as Loc,
        Par::topskip as Loc,
        Par::holdinginserts as Loc,
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
            if s.save_gid == 0 && s.ginert == 0 && !group_local {
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
    pub(crate) fn from_cont(c: ContState) -> State {
        let mut st = State::new();
        let ent = |x: Option<ValueId>, loaded: bool| {
            let mut e = Entry::raw(x);
            e.flags = if loaded { LOADED | HAS_ID } else { 0 };
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
            });
        }
        st.defined = c.defined;
        st.stamp = c.stamp;
        st.next_gid = c.next_gid;
        st
    }

    /// A global definition made between a cut and its resume (the
    /// finisher's) stands (§283): the binding is the engine's value now,
    /// and the open groups' saves of it are inert.
    pub(crate) fn override_global(&mut self, loc: Loc, x: Option<ValueId>) {
        self.any_gen = new_cat_gen();
        if is_cat(loc) {
            self.cat_gen = new_cat_gen();
        }
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
        let m = &mut self.metas[mi];
        m.ldef_stamp = st;
        m.ldef_local = false;
        m.ldef_gid = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reads a run reports come in the run's own order, whatever the
    /// locations' numbers: a meaning's number is its name's place in the
    /// process's interner, which the threads' schedule decides.
    #[test]
    fn reads_in_the_runs_order_not_by_location() {
        let mut st = State::new();
        st.fill(900, None);
        st.fill(7, None);
        st.fill(300, None);
        // (a location read, then defined: still a read, through its meta)
        st.set(300, Entry::raw(None), false, None);
        st.fill(40, None);
        assert_eq!(st.read_locs(), vec![300, 900, 7, 40]);
        let order: Vec<Loc> = st.read_values().into_iter().map(|(l, _)| l).collect();
        assert_eq!(order, st.read_locs());
    }

    /// A run resumed from a park reports the reads a run that never
    /// parked would, in the same order, and a location it read before
    /// the park and defines after it is still a read: whether a run
    /// parks is the schedule's choice. (The park dropped the slots that
    /// held only an engine read, so the reads before it went missing
    /// from the reads a piece contributes for a finisher's check.)
    #[test]
    fn a_parked_runs_reads_are_a_straight_runs() {
        let v = Values::new();
        let mut st = State::new();
        st.fill(900, Some(v.int(1)));
        st.fill(7, None);
        st.fill(300, Some(v.int(3)));
        st.set(300, Entry::raw(Some(v.int(4))), false, None);
        st.fill(40, Some(v.int(5)));
        st.set(41, Entry::raw(Some(v.int(6))), false, None);
        let want = st.read_values();
        let mut back = State::unpark(st.park(&v));
        assert_eq!(back.read_values(), want);
        // (defined after the resume: read before it, so still a read)
        st.set(900, Entry::raw(Some(v.int(7))), false, None);
        back.set(900, Entry::raw(Some(v.int(7))), false, None);
        st.fill(8, None);
        back.fill(8, None);
        assert_eq!(back.read_values(), st.read_values());
        assert!(back.read_locs().contains(&900));
    }
}
