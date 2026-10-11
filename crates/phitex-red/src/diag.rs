//! E2: errors (with TeX's recovery: inserted/deleted tokens), warnings,
//! `\show` `\showbox` `\showthe` `\showlists` `\showgroups` `\showtokens`.
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq)]
pub struct Diag { pub msg: String, pub help: Vec<String>, pub line: i32 }
/// Reports an error (emitted as `Emit::Diag`).
pub fn error(_c: &mut dyn crate::ctx::Ctx, _msg: &str, _help: &[&str]) { todo!("E2") }
