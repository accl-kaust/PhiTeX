//! The environment: a persistent map name → value, with group and list scopes.
//! Bodies owned by A1; signatures are the contract.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

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
    /// The env after `d`, with fresh definition cells.
    pub fn apply(&self, d: &Delta) -> Env {
        crate::graph::env_apply(self, d)
    }
    /// The env after `d`, its definitions bound to `cells` (in delta order,
    /// as `Node::cells`): the node's own definitions, kept across re-reductions.
    pub fn apply_cells(&self, d: &Delta, cells: &[Arc<DefCell>]) -> Env {
        crate::graph::env_apply_cells(self, d, cells)
    }
    /// The definition reaching `k` here (0: its initial value).
    pub fn def_of(&self, k: &Key) -> DefId {
        crate::graph::env_def(self, k)
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


/// A definition's identity: unique in the session (`DefCell::new` numbers
/// them). 0: a key's initial value (never redefined, never a use edge).
pub type DefId = u64;

static NEXT_DEF: AtomicU64 = AtomicU64::new(1);

/// The next definition id (graphs remember where theirs begin).
pub fn next_def_id() -> DefId {
    NEXT_DEF.load(Ordering::Relaxed)
}

/// One definition: the SSA value a name is bound to. Every env that the
/// definition reaches holds this cell, so re-reducing its node with the same
/// shape updates the value in place for all of them (`DefCell::set`). A cell
/// may be forwarded to an equal older one (`forward`) when an edit realigns
/// on it: readers follow the forward.
#[derive(Debug)]
pub struct DefCell {
    pub id: DefId,
    val: RwLock<Val>,
    fwd: RwLock<Option<Arc<DefCell>>>,
}

impl DefCell {
    pub fn new(val: Val) -> Arc<DefCell> {
        Arc::new(DefCell { id: NEXT_DEF.fetch_add(1, Ordering::Relaxed), val: RwLock::new(val), fwd: RwLock::new(None) })
    }
    /// The cell this one stands for (itself unless forwarded).
    pub fn resolved(self: &Arc<Self>) -> Arc<DefCell> {
        let mut c = self.clone();
        loop {
            let f = c.fwd.read().unwrap().clone();
            match f {
                Some(n) => c = n,
                None => return c,
            }
        }
    }
    pub fn val(self: &Arc<Self>) -> Val {
        self.resolved().val.read().unwrap().clone()
    }
    /// The id readers' use edges name.
    pub fn id_now(self: &Arc<Self>) -> DefId {
        self.resolved().id
    }
    /// Updates the value in place (the node that made it was re-reduced with the same shape).
    pub fn set(self: &Arc<Self>, v: Val) {
        *self.resolved().val.write().unwrap() = v;
    }
    /// From now on this cell stands for `to` (equal value; an edit realigned).
    pub fn forward(self: &Arc<Self>, to: &Arc<DefCell>) {
        let to = to.resolved();
        if !Arc::ptr_eq(&self.resolved(), &to) {
            *self.fwd.write().unwrap() = Some(to);
        }
    }
}
