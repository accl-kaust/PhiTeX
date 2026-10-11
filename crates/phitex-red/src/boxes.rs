//! C2: `\hbox` `\vbox` `\vtop` `\box` `\copy` `\lastbox` `\vsplit`
//! `\setbox`, `\raise` …, `\unhbox` …, `\leaders`, `\wd` `\ht` `\dp`, `\shipout`'s box.
use crate::ctx::Ctx;
use crate::prim::Registry;
use std::sync::Arc;
pub fn register(_r: &mut Registry) {}
/// Box context (tex.web 1071): 0 plain, shift, `\setbox` n, leaders, shipout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxContext { Plain, Shift(i32), SetBox(u16, bool), Leaders(u8), Ship }
pub fn begin_box(_c: &mut dyn Ctx, _ctx: BoxContext) { todo!("C2") }
pub fn box_end(_c: &mut dyn Ctx, _ctx: BoxContext, _b: Option<Arc<crate::item::BoxNode>>) { todo!("C2") }
/// A box group ends (called by `groups::right_brace`).
pub fn package(_c: &mut dyn Ctx, _kind: crate::env::GroupKind) { todo!("C2") }
