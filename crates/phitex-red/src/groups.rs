//! B5: `{` `}` `\begingroup` `\endgroup` `\aftergroup`, and dispatch of a
//! group's end by its kind to its owner (boxes, math, align, output, insert, disc…).
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
/// `}` (or `\egroup`) was met.
pub fn right_brace(_c: &mut dyn crate::ctx::Ctx) { todo!("B5") }
