#![allow(dead_code)] // (the skeleton: wired by the rewrite, docs/spec-state-model.md)
//! The run's view of TeX's state (docs/spec-state-model.md): every item
//! its own engine field, loaded through unrecorded family snapshots,
//! read per item used, written as nothing, a relation or a value by one
//! rule, published from the write log. Replaces `runstate`'s records,
//! save stack and publication paths, `fields`' record cells and
//! `regfold`.
//!
//! Every `todo!()` is a body the rewrite fills; each doc names its
//! callers and what it must do.

use phitex_defuse::{Ctx, Field, Locality, ValueId, Values};

use crate::defer::Sym;
use crate::run::R;

/// A location in the table: the fixed ranges of `fields` (parameters,
/// codes, registers, boxes, fams, meanings) and the spill, as today.
pub(crate) type Loc = u32;

/// An integer register's map over this run: `x ↦ a·x + b`, `keep`
/// true for `add b`, false for `set b` (the old `regfold::Affine`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Map {
    pub keep: bool,
    pub add: i32,
}

impl Map {
    pub(crate) const ID: Map = Map { keep: true, add: 0 };
    pub(crate) fn set(c: i32) -> Map {
        Map { keep: false, add: c }
    }
    pub(crate) fn add(c: i32) -> Map {
        Map { keep: true, add: c }
    }
    /// This map, then `next`.
    #[must_use]
    pub(crate) fn then(self, next: Map) -> Map {
        if next.keep { Map { keep: self.keep, add: self.add.wrapping_add(next.add) } } else { next }
    }
    pub(crate) fn apply(self, x: i32) -> i32 {
        if self.keep { x.wrapping_add(self.add) } else { self.add }
    }
}

/// What the run holds for an item (spec §2.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Held {
    /// Not touched yet.
    Absent,
    /// Loaded through the family's snapshot, not used: no read recorded.
    Loaded(Option<ValueId>),
    /// A page-layer item in a piece: its symbol, no read recorded.
    Symbol(Sym),
    /// Used: the read is recorded with this value.
    Used(Option<ValueId>),
    /// Written by this run (the value it holds now).
    Written(Option<ValueId>),
}

/// One item's slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    /// What reached the run (the loaded value or symbol), kept for the
    /// restore rule (§2.3 rule 1) and the equal rule (rule 2).
    pub reached: Held,
    /// What the run holds now.
    pub now: Held,
    /// The integer register's map over this run (rule 3), `ID` for the
    /// rest.
    pub map: Map,
    /// The depth of the group the last local write was made in (0: the
    /// run's base level), and whether the last write was global.
    pub depth: u16,
    pub global: bool,
    /// Whether the run read the item (a recorded read).
    pub used: bool,
}

/// A family of items loaded by one snapshot (§2.1): its field prefix
/// and the locations it covers.
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
    Meaning,
}

impl Family {
    /// The engine field prefix of the family (`cat:`, `param:`, …).
    pub(crate) fn prefix(self) -> &'static str {
        // fill: match self: "cat:" | "lc:" | … | "count:" | … | "cs:" (meanings: cs: and act:)
        todo!()
    }

    /// The family a location belongs to (the layout of `fields`).
    pub(crate) fn of(loc: Loc) -> Option<Family> {
        let _ = loc;
        // fill: by the fixed ranges of crate::fields (PAR0, CAT0, REG0, CODE0, BOX0, FAM0, MEAN_ACTIVE, MEAN_BASE)
        todo!()
    }
}

/// The run's table.
pub(crate) struct State {
    slots: Vec<Slot>,
    /// The families loaded so far, with their snapshot values.
    snapshots: Vec<(Family, ValueId)>,
    /// The current group depth of the run (0 at its base level), and the
    /// locations written locally at each depth, for `close`.
    depth: u16,
    local_writes: Vec<Vec<Loc>>,
    /// The write log: every write in order (loc, map or value, global,
    /// depth), for `publish`.
    log: Vec<Write>,
}

/// One write in the log.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Write {
    pub loc: Loc,
    pub kind: WriteKind,
    pub global: bool,
    pub depth: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum WriteKind {
    Value(Option<ValueId>),
    Add(i32),
    /// The item restored to what the run read (rule 1).
    Restore,
}

impl State {
    pub(crate) fn new(nslots: usize) -> State {
        let _ = nslots;
        // fill: slots of Held::Absent / Map::ID / depth 0, empty snapshots, depth 0, one local_writes level, empty log
        todo!()
    }

    /// The item's value for use (§2.2): loaded through its family's
    /// snapshot on the first touch (`Ctx::snapshot`, unrecorded), then
    /// recorded as a read of the item with the value the run uses
    /// (`Ctx::read_item`). A page-layer item in a piece is not loaded:
    /// `get` of it is a forced read (`defer::force`). Callers: every
    /// `int_loc`, `get_loc`, `reg_int`, `cur_mean`, `ipar`, `catcode`
    /// read in the interpreter.
    pub(crate) fn get(&mut self, cx: &mut Ctx<'_>, v: &Values, loc: Loc) -> R<Option<ValueId>> {
        let _ = (cx, v, loc);
        // fill: load_family(loc) if Absent; if Symbol -> force (the read at the run's position, Used); if Loaded(x) -> cx.read_item(field(loc), x), now = Used(x); return the current value (now)
        todo!()
    }

    /// The item's value for a purpose that does not use it (a save stack
    /// copy at a group's start, a `\relax`-ing probe): loaded, not
    /// recorded.
    pub(crate) fn peek(&mut self, cx: &mut Ctx<'_>, v: &Values, loc: Loc) -> R<Held> {
        let _ = (cx, v, loc);
        // fill: load_family(loc) if Absent; return now without recording
        todo!()
    }

    /// The symbol of a page-layer item at this run (§2.2), where the
    /// item is not yet loaded or written and the run is a piece; None
    /// otherwise (the caller then uses `get`). Callers: `\the` and
    /// `\number` on a register alone (`expand::the_toks_r`, `conv_toks`).
    pub(crate) fn symbol(&mut self, cx: &mut Ctx<'_>, loc: Loc) -> Option<Sym> {
        let _ = (cx, loc);
        // fill: if now is Absent and defer::deferrable(loc) and op is exec/resume: now = Symbol(Sym{loc, node}); Some(sym)
        todo!()
    }

    /// A write (§2.3): the value the run holds from now on, and a log
    /// entry; local at the current depth unless `global`. Callers: every
    /// assignment in the interpreter (`set_loc`, `set_f`, `\let`, `\def`,
    /// `\catcode`, `\count=`, …).
    pub(crate) fn set(&mut self, loc: Loc, x: Option<ValueId>, global: bool) {
        let _ = (loc, x, global);
        // fill: now = Written(x); map = Map::set(as int) for count registers; log.push(Write{Value(x)}); local_writes[depth].push(loc) unless global
        todo!()
    }

    /// `+c` on an integer register (§2.3 rule 3 where it applies: outside
    /// every group of the document, the register never read in this run).
    pub(crate) fn add(&mut self, loc: Loc, c: i32, global: bool) {
        let _ = (loc, c, global);
        // fill: map = map.then(Map::add(c)); now = Written(apply) if loaded/used; log.push(Write{Add(c)})
        todo!()
    }

    /// A restore of the item to its own symbol or to the value it loaded
    /// (§2.3 rule 1): the map back to ID, now back to reached, a Restore
    /// log entry. Caller: `defer::try_restore` and the group close.
    pub(crate) fn restore(&mut self, loc: Loc, global: bool) {
        let _ = (loc, global);
        // fill: now = reached; map = Map::ID; log.push(Write{Restore})
        todo!()
    }

    /// A group opened or closed by the run (§2.5): `close` restores every
    /// location written locally at the closing depth to what it held
    /// before the group (the save stack, run-local only).
    pub(crate) fn open(&mut self) {
        // fill: depth += 1; local_writes.push(vec![])
        todo!()
    }
    pub(crate) fn close(&mut self) {
        // fill: for loc in local_writes.pop(): now = the value before the group (kept on the log) ; depth -= 1
        todo!()
    }

    /// The items to publish at the run's end (§2.3), from the write log:
    /// for each written location at the base level (global writes at any
    /// depth; local writes at depth 0 or in a group the run leaves open,
    /// with that depth): nothing where the last write restores what the
    /// run read or equals the value reaching the run (`Ctx::read` of it
    /// at the run's position, unrecorded through `peek`), a relation
    /// (`Ctx::define_rel_with`) where the net map is `add c` and the
    /// item was never used, a value (`Ctx::define_with`) otherwise.
    pub(crate) fn publish(&self, cx: &mut Ctx<'_>, v: &Values) -> R<()> {
        let _ = (cx, v, Locality::Global);
        // fill: walk the log's last write per loc; apply rules 1-4; define_with / define_rel_with
        todo!()
    }

    /// Loads the family of `loc` through one unrecorded snapshot
    /// (`Ctx::snapshot(prefix field)`), filling every slot of the family
    /// with `Loaded(entry)`.
    fn load_family(&mut self, cx: &mut Ctx<'_>, v: &Values, loc: Loc) -> R<()> {
        let _ = (cx, v, loc);
        // fill: let fam = Family::of(loc); if not in snapshots: s = cx.snapshot(v.field(fam.prefix())); decode entries into Loaded(...)
        todo!()
    }

    /// The engine field of a location (`fields`' layout: `count:<n>`,
    /// `cat:<c>`, `param:<name>`, `cs:<name>`, …).
    pub(crate) fn field(&self, v: &Values, loc: Loc) -> Field {
        let _ = (v, loc);
        // fill: crate::fields::field_of(loc)
        todo!()
    }
}
