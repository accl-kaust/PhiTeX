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
//! from the scope kind (a format definition inside `\begingroup`, a box
//! opened on an earlier line of the format). Groups a one-command node of
//! the format leaves open at its end are still engine scopes
//! (`Run::open_group` on an empty nest); a document region is balanced
//! by the pass, so a region never leaves one.

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
    /// and whether a global definition since made the entry inert
    /// (§283: a global definition is retained).
    pub saved: Vec<(Field, Option<Option<ValueId>>, bool)>,
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
                bp_at: 0,
            },
            par: None,
            prev_graf: 0,
            extra,
            saved: Vec::new(),
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
            for d in tail {
                if d.2 == Locality::Global {
                    self.defs.push(d);
                }
            }
        }
        for &(f, prev, inert) in l.saved.iter().rev() {
            if inert {
                continue;
            }
            match prev {
                Some(x) => {
                    self.own.insert(f, x);
                }
                None => {
                    self.own.remove(&f);
                }
            }
        }
        if !l.saved.is_empty() {
            self.forget_cats();
        }
        let toks: Vec<Tok> = l.ag.clone();
        self.push_toks(toks);
        Some(l)
    }

    /// Before a local definition of `f`: the innermost open group saves
    /// what the run's view held, to restore at its close. Nothing is
    /// saved outside every group of this run (the definition is the
    /// run's, for the engine's scope).
    pub(crate) fn save_local(&mut self, f: Field) {
        let prev = self.own.get(&f).copied();
        let Some(l) = self.nest.iter_mut().rev().find(|l| l.group != 0) else {
            return;
        };
        if l.saved.iter().any(|s| s.0 == f) {
            return;
        }
        l.saved.push((f, prev, false));
    }

    /// A global definition of `f`: every saved entry for it is inert.
    pub(crate) fn make_inert(&mut self, f: Field) {
        for l in &mut self.nest {
            for s in &mut l.saved {
                if s.0 == f {
                    s.2 = true;
                }
            }
        }
    }

    /// Whether a local definition made now would belong to a group of
    /// this run (and so never reach the engine).
    pub(crate) fn in_local_group(&self) -> bool {
        self.nest.iter().any(|l| l.group != 0)
    }
}
