//! B5: assignments as deltas: prefixes, registers, parameters, `\let`
//! `\futurelet` `\chardef`… `\advance` `\multiply` `\divide`, code tables,
//! `\afterassignment`, `\globaldefs`, `\setbox` (box from `boxes.rs`), `\parshape`.
use crate::ctx::Ctx;
use crate::prim::{PrimId, Registry};
use crate::tok::Tok;
pub fn register(_r: &mut Registry) {}
/// An assignment beginning with `first` (after prefixes were gathered: 1 global 2 long 4 outer 8 protected).
pub fn prefixed_command(_c: &mut dyn Ctx, _first: PrimId, _prefixes: u8) { todo!("B5") }
/// `get_r_token`: the name to be defined.
pub fn get_r_token(_c: &mut dyn Ctx) -> Tok { todo!("B5") }
/// Defines with `\globaldefs` and `\afterassignment` applied.
pub fn assign(_c: &mut dyn Ctx, _key: crate::val::Key, _val: crate::val::Val, _global: bool) { todo!("B5") }
