//! C7: TFM fonts (parse, `\font` with `at`/`scaled`, `\fontdimen`,
//! `\hyphenchar`, `\skewchar`, `\fontname`, `\nullfont`), fonts by id.
use std::sync::Arc;
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Debug, PartialEq, Eq)]
pub struct Font { /* C7: name, size, design size, chars, lig/kern program, params, file bytes */ }
pub enum LigKern { None, Kern(i32), Lig { op: u8, c: u16 } }
impl Font {
    pub fn name(&self) -> &str { todo!("C7") }
    pub fn size(&self) -> i32 { todo!("C7") }
    pub fn has(&self, _c: u16) -> bool { todo!("C7") }
    pub fn width(&self, _c: u16) -> i32 { todo!("C7") }
    pub fn height(&self, _c: u16) -> i32 { todo!("C7") }
    pub fn depth(&self, _c: u16) -> i32 { todo!("C7") }
    pub fn italic(&self, _c: u16) -> i32 { todo!("C7") }
    pub fn lig_kern(&self, _a: u16, _b: u16) -> LigKern { todo!("C7") }
    /// Boundary char programs: `a` None = left boundary, `b` None = right.
    pub fn lig_kern_boundary(&self, _a: Option<u16>, _b: Option<u16>) -> LigKern { todo!("C7") }
    pub fn param(&self, _n: u16) -> i32 { todo!("C7") }
    /// Extensible recipe / next larger (math delimiters).
    pub fn next_larger(&self, _c: u16) -> Option<u16> { todo!("C7") }
    pub fn extensible(&self, _c: u16) -> Option<[u16; 4]> { todo!("C7") }
    pub fn tfm(&self) -> &Arc<[u8]> { todo!("C7") }
}
/// Fonts by id: the session's table (append-only, shared).
pub trait Fonts { fn font(&self, id: crate::val::FontId) -> Arc<Font>; }
/// The session's font table.
pub fn fonts() -> &'static dyn Fonts { todo!("C7") }
