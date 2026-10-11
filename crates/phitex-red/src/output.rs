//! D2: `\output` applied at a fire (box255, marks, inserts), `\shipout`,
//! whatsits expanded at shipout (`\write` text → stream/written file).
use std::sync::Arc;
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
/// A shipped page: its box, `\count0..9`, and the PDF/DVI parameters read at shipout.
#[derive(Clone, Debug, PartialEq)]
pub struct Shipped { pub b: Arc<crate::item::BoxNode>, pub counts: [i32; 10], pub params: Arc<[(crate::val::Key, crate::val::Val)]> }
