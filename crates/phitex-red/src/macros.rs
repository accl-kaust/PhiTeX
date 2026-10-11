//! B2: macros as function values: `\def` family (with `\long` `\outer`
//! `\protected`), parameter matching, `Apply`.
use crate::ctx::Ctx;
use crate::prim::Registry;
use crate::tok::Tok;
use crate::val::Macro;
pub fn register(_r: &mut Registry) {}
/// Matches `m`'s parameters against the input and pushes the substituted body.
pub fn expand_macro(_ctx: &mut dyn Ctx, _name: Tok, _m: &Macro) { todo!("B2") }
