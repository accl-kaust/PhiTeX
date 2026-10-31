//! The semantic nest (§211–§219), local to the run: the lists a run
//! builds and the groups it opens are the run's own until it ends.
//!
//! A document node is a paragraph region: its run begins in the
//! vertical list of its enclosing scope, builds its paragraph (and the
//! boxes, groups and math inside it) on this nest, and ends back in that
//! vertical list. Only what reaches level 0 leaves the run: the lines of
//! the paragraph and the vertical material around it, as contributions
//! to the accumulator `items` tagged with the enclosing list's id, and
//! the definitions that are global or made outside every group the run
//! opened. A group opened and closed in the same run touches no engine
//! scope: its local definitions are restored here when it closes, and its
//! list is a `Vec` here. Nothing of the nest is passed to another node.
//!
//! Level 0 is the engine's scope the node runs in: its mode and list come
//! from the scope kind the engine gives it. A format node runs its whole file and closes
//! every group it opens: a group open at a format node's end is an
//! error. A document region is cut at every paragraph end at brace depth
//! 0 of its text: one whose run ends with one group of its own open (an
//! environment's `\begin`) leaves it to the engine as a scope
//! (`Run::leave_scope`), and one that closes the scope it runs in (an
//! environment's `\end`) ends there, its engine effect a close
//! (`Run::close_group`), the rest of its input following it outside.

use phitex_defuse::{ValueId, Values};
use phitex_typeset::node::Node;

use crate::build::{FoldState, IGNORE_DEPTH};
use crate::run::Run;
use crate::val::Tok;

/// One level of the nest: a list under construction, or a group that
/// shares the list below it (a simple group, `\begingroup`).
#[derive(Clone, Debug)]
pub(crate) struct Level {
    /// tex.web's mode of the list this level builds (±1, ±102, ±203).
    pub mode: i32,
    /// The group that opened the level (`val::group`), 0 for a
    /// paragraph (a paragraph is not a group, §1091).
    pub group: i64,
    /// The id of the list the level builds: a local id (negative, this
    /// run's) when it owns one, else the id of the list below it.
    pub lid: i64,
    /// Whether the level owns its list.
    pub owns: bool,
    /// The list, its transitions folded as they are appended
    /// ([`crate::build::fold_item`]).
    pub list: Vec<Item>,
    pub fold: FoldState,
    /// A paragraph's `(language, packed hyphen minima)` (§1091).
    pub par: Option<(i64, i64)>,
    /// `\prevgraf` of the vertical list this level builds.
    pub prev_graf: i64,
    /// `mode_line` (§213): the input line this level began on (a
    /// paragraph's, its box reports' `pack_begin_line`, §816).
    pub mode_line: i64,
    /// Where it began, for its box reports' diagnostics.
    pub mode_place: Option<crate::run::DiagPlace>,
    /// What the opener recorded for the closer (a box's context and
    /// spec, an insert's number, …): as the scope kind's extra.
    pub extra: Vec<ValueId>,
    /// The group's number in the run's state table
    /// ([`crate::state::State::open`]), where its saves and its local
    /// definitions begin there, and the table's clock when it opened.
    pub gid: u32,
    pub save_base: usize,
    pub def_base: usize,
    pub open_stamp: u64,
    /// `\aftergroup` tokens (§282).
    pub ag: Vec<Tok>,
}

/// An item of a list of this run's: a value, or a node kept native
/// (what the main loop makes: characters, ligatures, spaces,
/// discretionaries; never one with a mark or whatsit handle), encoded
/// only when a reader asks for values ([`Item::value`]) and handed to
/// the line breaker or the packager as it is.
#[derive(Clone, Debug)]
pub(crate) enum Item {
    V(ValueId),
    N(Node),
}

impl Item {
    /// The item's value, the node encoded (and kept so) if native.
    pub(crate) fn value(&mut self, v: &Values) -> ValueId {
        match self {
            Item::V(x) => *x,
            Item::N(n) => {
                let x = crate::nodes::encode(v, n, &crate::nodes::Side::default());
                *self = Item::V(x);
                x
            }
        }
    }

    /// The item's value, without keeping it.
    pub(crate) fn to_value(&self, v: &Values) -> ValueId {
        match self {
            Item::V(x) => *x,
            Item::N(n) => crate::nodes::encode(v, n, &crate::nodes::Side::default()),
        }
    }

    /// Whether the item is a page builder's marker
    /// ([`crate::build::is_marker`]); a native node never is.
    pub(crate) fn is_marker(&self, v: &Values) -> bool {
        match self {
            Item::V(x) => crate::build::is_marker(v, *x),
            Item::N(_) => false,
        }
    }
}

/// The values of a list's items, its native nodes encoded and kept so.
pub(crate) fn values(v: &Values, items: &mut [Item]) -> Vec<ValueId> {
    items.iter_mut().map(|x| x.value(v)).collect()
}

/// The values of a list's items, given up.
pub(crate) fn into_values(v: &Values, items: Vec<Item>) -> Vec<ValueId> {
    items.into_iter().map(|x| x.to_value(v)).collect()
}

/// A list of values as items.
pub(crate) fn of_values(xs: Vec<ValueId>) -> Vec<Item> {
    xs.into_iter().map(Item::V).collect()
}

impl Level {
    pub(crate) fn new(mode: i32, group: i64, lid: i64, owns: bool, extra: Vec<ValueId>) -> Level {
        Level {
            mode,
            group,
            lid,
            owns,
            list: Vec::new(),
            fold: FoldState {
                sf: 1000,
                pd: IGNORE_DEPTH,
                since_bp: 0,
            },
            par: None,
            prev_graf: 0,
            mode_line: 0,
            mode_place: None,
            extra,
            gid: 0,
            save_base: 0,
            def_base: 0,
            open_stamp: 0,
            ag: Vec::new(),
        }
    }
}

impl Run<'_, '_> {
    /// A list id of this run's own: never a node's, never another run's.
    pub(crate) fn new_lid(&mut self) -> i64 {
        self.muts += 1;
        self.next_lid -= 1;
        self.next_lid
    }

    /// The innermost level, if the run opened any.
    pub(crate) fn top(&self) -> Option<&Level> {
        self.nest.last()
    }

    pub(crate) fn top_mut(&mut self) -> Option<&mut Level> {
        self.muts += 1;
        self.nest.last_mut()
    }

    /// The level that owns list `lid`, if it is one of this run's.
    pub(crate) fn level_of(&mut self, lid: i64) -> Option<&mut Level> {
        self.muts += 1;
        self.nest.iter_mut().rev().find(|l| l.owns && l.lid == lid)
    }

    /// How many groups the run has open (a paragraph is not one).
    pub(crate) fn local_groups(&self) -> i64 {
        i64::try_from(self.nest.iter().filter(|l| l.group != 0).count()).unwrap_or(0)
    }

    /// Opens a level that builds a list of its own: a paragraph (`group`
    /// 0), a box, math, an insert, an alignment. Returns its list id.
    pub(crate) fn push_list(&mut self, mode: i32, group: i64, extra: Vec<ValueId>) -> i64 {
        let lid = self.new_lid();
        let mut l = Level::new(mode, group, lid, true, extra);
        self.open_level(&mut l);
        self.muts += 1;
        self.nest.push(l);
        lid
    }

    /// Opens a group that shares the list below it: `{`, `\begingroup`
    /// (§1063), at the mode and list of the innermost level.
    pub(crate) fn push_group(&mut self, mode: i32, lid: i64, group: i64, extra: Vec<ValueId>) {
        let mut l = Level::new(mode, group, lid, false, extra);
        self.open_level(&mut l);
        self.muts += 1;
        self.nest.push(l);
    }

    fn open_level(&mut self, l: &mut Level) {
        let (gid, sb, db, st) = self.st.open();
        l.gid = gid;
        l.save_base = sb;
        l.def_base = db;
        l.open_stamp = st;
    }

    /// The innermost group of this run open now (a paragraph is not one).
    #[inline]
    pub(crate) fn innermost_group(&self) -> Option<u32> {
        self.nest.iter().rev().find(|l| l.group != 0).map(|l| l.gid)
    }

    /// Closes the innermost level: its local definitions undone (the
    /// globals among them kept, §283), its `\aftergroup` tokens put
    /// back (§282); the level is returned with its list.
    pub(crate) fn pop_level(&mut self) -> Option<Level> {
        self.muts += 1;
        let l = self.nest.pop()?;
        // (a paragraph is no group: its definitions are the level's below)
        if l.group != 0 {
            // (a restore after a read-back point changes what the point
            // had, as a definition does: kept for the lines written
            // later, [`Run::note_pre`])
            if self.read_back_done && self.interp.is_none() {
                for loc in self.st.saved_locs(l.save_base) {
                    self.note_pre(loc);
                }
            }
            self.st.close(l.save_base, l.def_base);
        }
        // (§282: each one put back by `back_input`, so a brace among
        // them is counted in `align_state` once more when read, §325: an
        // `\aftergroup}` closing a box in an alignment's entry, LaTeX's
        // `lrbox`)
        let toks: Vec<Tok> = l.ag.clone();
        for &t in &toks {
            self.uncount_brace(t);
        }
        self.push_toks(toks);
        Some(l)
    }

    /// Levels taken off the nest without closing them (a group left to
    /// the engine as a scope, the groups open at the end of the run):
    /// the local definitions made in their groups are the run's own.
    pub(crate) fn abandon_levels(&mut self, ls: &[Level]) {
        self.muts += 1;
        if let Some(l) = ls.iter().find(|l| l.group != 0) {
            self.st.abandon(l.save_base, l.def_base, l.open_stamp);
        }
    }

    /// A binding of location `loc` made at group depth `lvl` changed in
    /// place (§1079, §1247: a box register "at the same level"): the
    /// value in force is replaced without saving it, since the group
    /// that made the binding saved what it hid, and the definition is
    /// that group's, so the closes of the groups opened since keep it.
    /// `base` is the depth of level 0.
    pub(crate) fn modify_at(&mut self, loc: crate::state::Loc, val: ValueId, lvl: i64, base: i64) {
        self.muts += 1;
        // a binding made at or below the run's base level (outside every
        // group of the run) changes there: the run's definition at its
        // base level, which the closes of the run's groups keep (§1079);
        // at level one it is retained as a global one is (§283); else
        // the binding is a group's of this run
        let at_base = lvl <= base;
        self.st
            .modify(loc, crate::state::Entry::raw(Some(val)), at_base, lvl == 0);
    }
}
