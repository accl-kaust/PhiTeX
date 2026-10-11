//! C3: line breaking as a pure function (tex.web 813–890, e-TeX
//! `\lastlinefit`, pdfTeX protrusion/expansion); hyphenation via `hyph.rs`.
use std::sync::Arc;
use crate::item::{BoxNode, Item};
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
/// Everything line breaking reads (gathered by `list::end_graf`).
#[derive(Clone, Debug, PartialEq)]
pub struct LineParams { /* C3 defines */ }
/// The lines (boxes and the vertical material between them) and `\prevgraf`.
pub struct Lines { pub items: Vec<Item>, pub prev_graf: i32, pub last: Option<Arc<BoxNode>> }
pub fn line_break(_hlist: Vec<Item>, _p: &LineParams, _fonts: &dyn crate::font::Fonts) -> Lines { todo!("C3") }
