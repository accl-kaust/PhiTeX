//! The run-local state table: every field a run reads or defines, in
//! slots by location ([`Loc`]), in its native form ([`Native`]).
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

/// A slot's value in the run's own form. An engine value is kept as it
/// is (`Raw`) until a typed read decodes it once.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) enum Native {
    /// The engine has no definition (an undefined control sequence, a
    /// void register).
    Absent,
    Int(i32),
    Glue(Glue),
    /// A decoded meaning (a macro's body is shared, so a clone is cheap).
    Mean(Mean),
    Toks(Rc<[Tok]>),
    /// Anything else, or not decoded yet: the engine value.
    Raw(ValueId),
}

impl Native {
    /// The engine value of this one (`None`: absent).
    pub(crate) fn encode(&self, v: &Values) -> Option<ValueId> {
        match self {
            Native::Absent => None,
            Native::Int(i) => Some(crate::val::int_id(v, i64::from(*i))),
            Native::Glue(g) => Some(crate::val::glue_value(v, g)),
            Native::Mean(m) => Some(m.encode(v)),
            Native::Toks(t) => Some(crate::val::toks_value(v, t)),
            Native::Raw(x) => Some(*x),
        }
    }
}

const LOADED: u8 = 1;
const HAS_ID: u8 = 2;
const PRELOADED: u8 = 4;

/// A slot's value and what is known of it.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    val: Native,
    /// The engine value of `val`, once known (`HAS_ID`).
    id: Option<ValueId>,
    flags: u8,
}

const UNLOADED: Entry = Entry {
    val: Native::Absent,
    id: None,
    flags: 0,
};

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

/// What a run defined at a location.
#[derive(Clone)]
struct Meta {
    loc: Loc,
    /// The engine's value when first defined, if the run had read it.
    eng: Option<Option<ValueId>>,
    /// The definition in force at the run's base level, if any.
    publ: Option<(Native, Locality)>,
    /// The stamp of the latest definition, and whether it was local.
    ldef_stamp: u64,
    ldef_local: bool,
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
}

/// The table.
pub(crate) struct State {
    eq: Vec<Slot>,
    spill: Vec<Slot>,
    spill_ix: FxHashMap<Field, u32>,
    touched: Vec<Loc>,
    metas: Vec<Meta>,
    save: Vec<Saved>,
    /// Locations defined locally inside a group of the run (for a group
    /// left open: its definitions become the run's).
    defined: Vec<Loc>,
    stamp: u64,
    next_gid: u32,
}

thread_local! {
    /// Tables of finished runs, emptied, for the next run on the thread
    /// (a table spans every control sequence seen).
    static POOL: std::cell::RefCell<Vec<Vec<Slot>>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Clone for State {
    fn clone(&self) -> State {
        let mut eq = POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
        eq.clear();
        eq.extend(self.eq.iter().cloned());
        State {
            eq,
            spill: self.spill.clone(),
            spill_ix: self.spill_ix.clone(),
            touched: self.touched.clone(),
            metas: self.metas.clone(),
            save: self.save.clone(),
            defined: self.defined.clone(),
            stamp: self.stamp,
            next_gid: self.next_gid,
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
        POOL.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < 4 {
                p.push(eq);
            }
        });
    }
}

impl State {
    pub(crate) fn new() -> State {
        State {
            eq: POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default(),
            spill: Vec::new(),
            spill_ix: FxHashMap::default(),
            touched: Vec::new(),
            metas: Vec::new(),
            save: Vec::new(),
            defined: Vec::new(),
            stamp: 0,
            next_gid: 0,
        }
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

    #[inline]
    pub(crate) fn is_loaded(&self, loc: Loc) -> bool {
        self.slot_ref(loc).is_some_and(|s| s.e.flags & LOADED != 0)
    }

    /// The engine's value of `loc`, read: `Unloaded -> Loaded`.
    pub(crate) fn fill(&mut self, loc: Loc, x: Option<ValueId>) {
        self.slot(loc).e = Entry {
            val: x.map_or(Native::Absent, Native::Raw),
            id: x,
            flags: LOADED | HAS_ID,
        };
    }

    /// A value read in place of the engine's (a page step's `\count0..9`
    /// as the step before left them): loaded, and never compared with
    /// the engine's at the end.
    pub(crate) fn preload(&mut self, loc: Loc, x: Option<ValueId>) {
        self.slot(loc).e = Entry {
            val: x.map_or(Native::Absent, Native::Raw),
            id: x,
            flags: LOADED | HAS_ID | PRELOADED,
        };
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

    /// The engine value of loaded `loc` (encoded once).
    pub(crate) fn id_of(&mut self, loc: Loc, v: &Values) -> Option<ValueId> {
        let e = self.entry_mut(loc);
        if e.flags & HAS_ID != 0 {
            return e.id;
        }
        let x = e.val.encode(v);
        e.id = x;
        e.flags |= HAS_ID;
        x
    }

    /// Loaded `loc` as an integer (decoded once).
    #[inline]
    pub(crate) fn int_of(&mut self, loc: Loc, v: &Values) -> i32 {
        let e = self.entry_mut(loc);
        match e.val {
            Native::Int(i) => i,
            Native::Absent => 0,
            Native::Raw(x) => {
                let i = crate::val::i32_of(v, Some(x));
                e.val = Native::Int(i);
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
        let e = self.entry_mut(loc);
        match e.val {
            Native::Glue(g) => g,
            Native::Absent => Glue::ZERO,
            Native::Raw(x) => {
                let g = crate::val::glue_of(v, Some(x));
                e.val = Native::Glue(g);
                g
            }
            _ => {
                let x = self.id_of(loc, v);
                crate::val::glue_of(v, x)
            }
        }
    }

    /// Loaded `loc` as a meaning (decoded once).
    #[inline]
    pub(crate) fn mean_of(&mut self, loc: Loc, v: &Values) -> Mean {
        let e = self.entry_mut(loc);
        match &e.val {
            Native::Mean(m) => m.clone(),
            Native::Absent => Mean::Undefined,
            Native::Raw(x) => {
                let m = Mean::decode(v, Some(*x));
                e.val = Native::Mean(m.clone());
                m
            }
            _ => {
                let x = self.id_of(loc, v);
                Mean::decode(v, x)
            }
        }
    }

    /// Loaded `loc` as a token list (decoded once).
    #[allow(dead_code)]
    pub(crate) fn toks_of(&mut self, loc: Loc, v: &Values) -> Rc<[Tok]> {
        let e = self.entry_mut(loc);
        match &e.val {
            Native::Toks(t) => t.clone(),
            Native::Absent => Rc::from([]),
            Native::Raw(x) => {
                let t = crate::val::toks_rc(v, Some(*x));
                e.val = Native::Toks(t.clone());
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
    pub(crate) fn set(&mut self, loc: Loc, val: Native, global: bool, grp: Option<u32>) {
        let mi = self.meta(loc);
        self.stamp += 1;
        let st = self.stamp;
        let s = self.slot(loc);
        if global {
            s.ginert = st;
        } else if let Some(g) = grp
            && !(s.save_gid == g && s.ginert <= s.save_stamp)
        {
            let saved = Saved {
                loc,
                prev: s.e.clone(),
                gid: s.save_gid,
                stamp: s.save_stamp,
                at: st,
            };
            s.save_gid = g;
            s.save_stamp = st;
            self.save.push(saved);
        }
        let id = match val {
            Native::Raw(x) => Some(x),
            _ => None,
        };
        let s = self.slot(loc);
        s.e = Entry {
            val: val.clone(),
            id,
            flags: LOADED | if id.is_some() { HAS_ID } else { 0 },
        };
        if !global && grp.is_some() {
            self.defined.push(loc);
        }
        let m = &mut self.metas[mi];
        m.ldef_stamp = st;
        m.ldef_local = !global;
        if global {
            m.publ = Some((val, Locality::Global));
        } else if grp.is_none() {
            m.publ = Some((val, Locality::Local));
        }
    }

    /// A binding changed in place (§1079, §1247), nothing saved:
    /// `base` when the binding is the run's base level's (then it is
    /// the run's definition there), else a group's of the run.
    pub(crate) fn modify(&mut self, loc: Loc, val: Native, base: bool) {
        let mi = self.meta(loc);
        self.stamp += 1;
        let st = self.stamp;
        let id = match val {
            Native::Raw(x) => Some(x),
            _ => None,
        };
        self.slot(loc).e = Entry {
            val: val.clone(),
            id,
            flags: LOADED | if id.is_some() { HAS_ID } else { 0 },
        };
        if !base {
            self.defined.push(loc);
        }
        let m = &mut self.metas[mi];
        m.ldef_stamp = st;
        m.ldef_local = true;
        if base {
            m.publ = Some((val, Locality::Local));
        }
    }

    /// Closes a group whose saves and definitions began at `save_base`
    /// and `def_base`: its saves are restored (the inert ones, hidden by
    /// a global definition since, are not, §283).
    pub(crate) fn close(&mut self, save_base: usize, def_base: usize) {
        while self.save.len() > save_base {
            let Some(s) = self.save.pop() else { break };
            let slot = self.slot(s.loc);
            if slot.ginert > s.at {
                continue;
            }
            slot.e = s.prev;
            slot.save_gid = s.gid;
            slot.save_stamp = s.stamp;
        }
        self.defined.truncate(def_base.min(self.defined.len()));
    }

    /// Leaves the groups from the one whose saves and definitions began
    /// at `save_base` and `def_base` (opened at `open_stamp`) open: they
    /// are not closed here (the engine's scope, or the end of the run),
    /// so the local definitions made inside them are the run's own.
    pub(crate) fn abandon(&mut self, save_base: usize, def_base: usize, open_stamp: u64) {
        let locs: Vec<Loc> = self.defined[def_base.min(self.defined.len())..].to_vec();
        for loc in locs {
            let mi = self.meta(loc);
            if self.metas[mi].ldef_local && self.metas[mi].ldef_stamp > open_stamp {
                let val = self.slot(loc).e.val.clone();
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

    /// The locations with a definition at the base level.
    pub(crate) fn defined_locs(&self) -> impl Iterator<Item = Loc> + '_ {
        self.metas
            .iter()
            .filter(|m| m.publ.is_some())
            .map(|m| m.loc)
    }

    /// The definitions made after stamp `since`, at any level (a line
    /// read back is interpreted wherever the run is): the location, the
    /// value in force and the locality of the latest one.
    pub(crate) fn defined_since(
        &mut self,
        v: &Values,
        since: u64,
    ) -> Vec<(Loc, Option<ValueId>, Locality)> {
        let hits: Vec<(Loc, bool)> = self
            .metas
            .iter()
            .filter(|m| m.ldef_stamp > since)
            .map(|m| (m.loc, m.ldef_local))
            .collect();
        hits.into_iter()
            .map(|(loc, local)| {
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
    /// run read is left out (the same binding).
    pub(crate) fn published(&self, v: &Values) -> Vec<(Loc, ValueId, Locality)> {
        let mut out = Vec::with_capacity(self.metas.len());
        for m in &self.metas {
            let Some((val, l)) = &m.publ else { continue };
            let Some(x) = val.encode(v) else { continue };
            if *l == Locality::Local && m.eng == Some(Some(x)) {
                continue;
            }
            out.push((m.loc, x, *l));
        }
        out
    }

    /// Whether any of `locs` has a definition at the base level.
    pub(crate) fn defines_any(&self, locs: &[Loc]) -> bool {
        locs.iter().any(|&l| {
            self.slot_ref(l)
                .is_some_and(|s| s.meta != 0 && self.metas[s.meta as usize - 1].publ.is_some())
        })
    }

    /// The table without `Rc`, for a parked run ([`PState`]): the slots
    /// that hold more than an engine read.
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
                e.val.encode(v)
            };
            PEntry {
                id,
                flags: e.flags | HAS_ID,
            }
        };
        let mut slots = Vec::new();
        for &l in &self.touched {
            let Some(s) = self.slot_ref(l) else { continue };
            if s.meta == 0 && s.save_gid == 0 && s.ginert == 0 && s.e.flags & PRELOADED == 0 {
                continue;
            }
            let meta = (s.meta != 0).then(|| {
                let m = &self.metas[s.meta as usize - 1];
                PMeta {
                    eng: m.eng,
                    publ: m.publ.as_ref().map(|(x, l)| (x.encode(v), *l)),
                    ldef_stamp: m.ldef_stamp,
                    ldef_local: m.ldef_local,
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
            })
            .collect();
        let defined = self.defined.iter().map(|&l| ploc(self, l)).collect();
        PState {
            slots,
            save,
            defined,
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
        let ent = |e: PEntry| Entry {
            val: e.id.map_or(Native::Absent, Native::Raw),
            id: e.id,
            flags: e.flags,
        };
        for s in p.slots {
            let l = loc(&mut st, s.loc);
            let meta = s.meta.map(|m| {
                st.metas.push(Meta {
                    loc: l,
                    eng: m.eng,
                    publ: m
                        .publ
                        .map(|(x, lc)| (x.map_or(Native::Absent, Native::Raw), lc)),
                    ldef_stamp: m.ldef_stamp,
                    ldef_local: m.ldef_local,
                });
                u32::try_from(st.metas.len()).unwrap_or(0)
            });
            let slot = st.slot(l);
            slot.e = ent(s.e);
            slot.save_gid = s.save_gid;
            slot.save_stamp = s.save_stamp;
            slot.ginert = s.ginert;
            slot.meta = meta.unwrap_or(0);
        }
        for s in p.save {
            let l = loc(&mut st, s.loc);
            st.save.push(Saved {
                loc: l,
                prev: ent(s.prev),
                gid: s.gid,
                stamp: s.stamp,
                at: s.at,
            });
        }
        for d in p.defined {
            let l = loc(&mut st, d);
            st.defined.push(l);
        }
        st.stamp = p.stamp;
        st.next_gid = p.next_gid;
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
    eng: Option<Option<ValueId>>,
    publ: Option<(Option<ValueId>, Locality)>,
    ldef_stamp: u64,
    ldef_local: bool,
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
}

/// A run's table, parked ([`State::park`]).
pub(crate) struct PState {
    slots: Vec<PSlot>,
    save: Vec<PSaved>,
    defined: Vec<PLoc>,
    stamp: u64,
    next_gid: u32,
}
