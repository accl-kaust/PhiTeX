//! The environment: a persistent map name → value, with group and list scopes.
//! Bodies owned by A1; signatures are the contract.
use std::sync::Arc;

use crate::tok::Toks;
use crate::val::{Key, Val};

/// TeX's group codes (etex `\currentgrouptype`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupKind {
    Bottom,
    Simple,
    Hbox,
    AdjustedHbox,
    Vbox,
    Vtop,
    Align,
    NoAlign,
    Output,
    Math,
    Disc,
    Insert,
    Vcenter,
    MathChoice,
    SemiSimple,
    MathShift,
    MathLeft,
}

/// A list's mode; `inner` is TeX's negative mode.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    Vertical { inner: bool },
    Horizontal { inner: bool },
    Math { display: bool },
}

/// A set of definitions a node made: local ones (to the innermost group) and
/// global ones, plus scope changes, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delta(pub Vec<Change>);

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Def { key: Key, val: Val, global: bool },
    /// A change where the value lives (TeX's direct `eqtb` store, e.g.
    /// `box(n):=null` by `\box`): no save entry, the entry keeps its level.
    Replace { key: Key, val: Val },
    OpenGroup(GroupKind),
    /// Closing the innermost group: its local definitions go, `\aftergroup`
    /// tokens are pushed onto the input by the closer.
    CloseGroup,
    AfterGroup(crate::tok::Tok),
    OpenList(Mode),
    CloseList,
}

/// The persistent environment. Cloning is O(1).
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub(crate) inner: Arc<crate::graph::EnvRep>,
}

impl Env {
    /// The value of `key` (no read is recorded: use `Ctx::get` in nodes).
    pub fn lookup(&self, key: &Key) -> Val {
        crate::graph::env_lookup(self, key)
    }
    pub fn apply(&self, d: &Delta) -> Env {
        crate::graph::env_apply(self, d)
    }
    pub fn group_kind(&self) -> GroupKind {
        crate::graph::env_group_kind(self)
    }
    pub fn group_depth(&self) -> u32 {
        crate::graph::env_group_depth(self)
    }
    pub fn mode(&self) -> Mode {
        crate::graph::env_mode(self)
    }
    pub fn list_depth(&self) -> u32 {
        crate::graph::env_list_depth(self)
    }
    pub fn after_group(&self) -> Toks {
        crate::graph::env_after_group(self)
    }
}

impl Env {
    /// A hash of everything this env answers (definitions, scope chains),
    /// maintained incrementally: equal hashes mean equal envs (128 bits; a
    /// collision is treated as impossible). O(1).
    pub fn hash(&self) -> u128 {
        crate::graph::env_hash(self)
    }
}
