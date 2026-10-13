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
//! error. A document region is balanced by the pass, so a region never
//! leaves one either.

use phitex_defuse::{Field, Locality, ValueId};

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
    pub list: Vec<ValueId>,
    pub fold: FoldState,
    /// A paragraph's `(language, packed hyphen minima)` (§1091).
    pub par: Option<(i64, i64)>,
    /// `\prevgraf` of the vertical list this level builds.
    pub prev_graf: i64,
    /// What the opener recorded for the closer (a box's context and
    /// spec, an insert's number, …): as the scope kind's extra.
    pub extra: Vec<ValueId>,
    /// Bindings to restore when the group closes: the field, what the
    /// run's own view held before (`None`: nothing, the engine's value),
    /// and the stamp of the save (`Run::stamp`): a global definition of
    /// the field stamped later makes the entry inert (§283: a global
    /// definition is retained).
    #[allow(clippy::option_option)]
    pub saved: Vec<(Field, Option<Option<ValueId>>, u64)>,
    /// The latest entry of `saved` per field (O(1) `eq_save` check).
    pub saved_ix: crate::FxHashMap<Field, usize>,
    /// The run's `defs` length when the group opened: definitions after
    /// it are the group's, undone at its close unless global.
    pub defs_len: usize,
    /// `\aftergroup` tokens (§282).
    pub ag: Vec<Tok>,
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
            extra,
            saved: Vec::new(),
            saved_ix: crate::FxHashMap::default(),
            defs_len: 0,
            ag: Vec::new(),
        }
    }
}

impl Run<'_, '_> {
    /// A list id of this run's own: never a node's, never another run's.
    pub(crate) fn new_lid(&mut self) -> i64 {
        self.next_lid -= 1;
        self.next_lid
    }

    /// The innermost level, if the run opened any.
    pub(crate) fn top(&self) -> Option<&Level> {
        self.nest.last()
    }

    pub(crate) fn top_mut(&mut self) -> Option<&mut Level> {
        self.nest.last_mut()
    }

    /// The level that owns list `lid`, if it is one of this run's.
    pub(crate) fn level_of(&mut self, lid: i64) -> Option<&mut Level> {
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
        l.defs_len = self.defs.len();
        self.nest.push(l);
        lid
    }

    /// Opens a group that shares the list below it: `{`, `\begingroup`
    /// (§1063), at the mode and list of the innermost level.
    pub(crate) fn push_group(&mut self, mode: i32, lid: i64, group: i64, extra: Vec<ValueId>) {
        let mut l = Level::new(mode, group, lid, false, extra);
        l.defs_len = self.defs.len();
        self.nest.push(l);
    }

    /// Closes the innermost level: its local definitions undone (the
    /// globals among them kept, §283), its `\aftergroup` tokens put
    /// back (§282); the level is returned with its list.
    pub(crate) fn pop_level(&mut self) -> Option<Level> {
        let l = self.nest.pop()?;
        // the group's definitions: the globals stay, the locals go (a
        // paragraph is no group: its definitions are the level's below)
        if l.group != 0 {
            let tail: Vec<(Field, ValueId, Locality)> =
                self.defs.split_off(l.defs_len.min(self.defs.len()));
            for d in &tail {
                if let Some(ix) = self.def_ix.get_mut(&d.0) {
                    ix.pop();
                }
            }
            for d in tail {
                if d.2 == Locality::Global {
                    self.def_ix.entry(d.0).or_default().push(self.defs.len());
                    self.defs.push(d);
                }
            }
        }
        for &(f, prev, at) in l.saved.iter().rev() {
            if self.inert(f, at) {
                continue;
            }
            if let Some(x) = prev {
                self.own.insert(f, x);
            } else {
                self.own.remove(&f);
            }
            self.slot_note(f, prev);
        }
        let toks: Vec<Tok> = l.ag.clone();
        self.push_toks(toks);
        Some(l)
    }

    /// Before a local definition of `f`: the innermost open group saves
    /// what the run's view held, to restore at its close. Nothing is
    /// saved outside every group of this run (the definition is the
    /// run's, for the engine's scope). A group saves once per field
    /// while its value is its own; after a global definition (its saved
    /// entries inert) the next local one saves again, as `eq_save` does
    /// when `eq_level` is `level_one` (§277, §283): that save restores
    /// the global value at the close.
    pub(crate) fn save_local(&mut self, f: Field) {
        let prev = self.own.get(&f).copied();
        let Some(k) = self.nest.iter().rposition(|l| l.group != 0) else {
            return;
        };
        if let Some(&i) = self.nest[k].saved_ix.get(&f)
            && !self.inert(f, self.nest[k].saved[i].2)
        {
            return;
        }
        self.stamp += 1;
        let at = self.stamp;
        let l = &mut self.nest[k];
        l.saved_ix.insert(f, l.saved.len());
        l.saved.push((f, prev, at));
    }

    /// Whether a saved entry of `f` stamped `at` was made inert by a
    /// global definition since.
    fn inert(&self, f: Field, at: u64) -> bool {
        self.ginert.get(&f).is_some_and(|&g| g > at)
    }

    /// Rebuilds `def_ix` after `defs` moved (an in-place change below
    /// the innermost group, `modify_at`).
    pub(crate) fn reindex_defs(&mut self) {
        self.def_ix.clear();
        for (i, d) in self.defs.iter().enumerate() {
            self.def_ix.entry(d.0).or_default().push(i);
        }
    }

    /// A binding of `f` made at group depth `lvl` changed in place
    /// (§1079, §1247: a box register "at the same level"): the value in
    /// force is replaced without saving it, since the group that made the
    /// binding saved what it hid, and the definition is filed with that
    /// group's definitions, below every group opened since, so their
    /// closes keep it. `base` is the depth of level 0.
    pub(crate) fn modify_at(&mut self, f: Field, val: ValueId, lvl: i64, base: i64) {
        self.own.insert(f, Some(val));
        self.irreversible += 1;
        self.slot_note(f, Some(Some(val)));
        // the first open group deeper than the binding's level
        let mut depth = base;
        let mut at = None;
        for (i, l) in self.nest.iter().enumerate() {
            if l.group != 0 {
                depth += 1;
                if depth > lvl {
                    at = Some(i);
                    break;
                }
            }
        }
        let d = (f, val, Locality::Local);
        match at {
            // the binding is the innermost group's own (or the run's,
            // outside every group): a definition at this level
            None => {
                if self.in_local_group() {
                    self.def_ix.entry(f).or_default().push(self.defs.len());
                    self.defs.push(d);
                } else {
                    self.defs.retain(|x| x.0 != f);
                    self.defs.push(d);
                    self.reindex_defs();
                }
            }
            Some(i) => {
                let pos = self.nest[i].defs_len.min(self.defs.len());
                self.defs.insert(pos, d);
                for l in &mut self.nest[i..] {
                    l.defs_len += 1;
                }
                self.reindex_defs();
            }
        }
    }

    /// A global definition of `f`: every saved entry for it is inert
    /// (the entries stamped before it, `inert`).
    pub(crate) fn make_inert(&mut self, f: Field) {
        if self.nest.is_empty() {
            return;
        }
        self.stamp += 1;
        self.ginert.insert(f, self.stamp);
    }

    /// Whether a local definition made now would belong to a group of
    /// this run (and so never reach the engine).
    pub(crate) fn in_local_group(&self) -> bool {
        self.nest.iter().any(|l| l.group != 0)
    }
}
