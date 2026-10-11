//! C5: math mode: `$` `$$`, noads, `\over` & co, `\left` `\middle` `\right`,
//! `\mathchoice`, `\eqno`, display placement, and `mlist_to_hlist` (pure).
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noad { /* C5 */ }
/// `$` in horizontal mode or a math group's end.
pub fn init_math(_c: &mut dyn crate::ctx::Ctx) { todo!("C5") }
pub fn math_group_end(_c: &mut dyn crate::ctx::Ctx, _kind: crate::env::GroupKind) { todo!("C5") }
