//! C4: `\patterns`, `\hyphenation`, and hyphenating a word (tex.web 891–965).
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq)]
pub struct Patterns { /* C4 */ }
#[derive(Clone, Debug, PartialEq)]
pub struct Exceptions { /* C4 */ }
/// Hyphen positions of a word (lowercase codes), language data given.
pub fn hyphenate(_word: &[u16], _p: Option<&Patterns>, _e: Option<&Exceptions>, _lhm: usize, _rhm: usize) -> Vec<usize> { todo!("C4") }
