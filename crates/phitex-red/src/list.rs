//! C1: building lists: characters (ligatures/kerns from the font), spaces and
//! space factor, `\char`, `\accent`, glue/kern/penalty/rule commands, `\-`
//! `\discretionary`, `\indent` `\noindent` `\par`, `\vadjust` `\insert` `\mark`,
//! `\unskip` & co, `\end`. Paragraph end calls `linebreak::line_break`.
use crate::ctx::Ctx;
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
/// A paragraph begins (letter or `\indent` in vertical mode).
pub fn new_graf(_c: &mut dyn Ctx, _indent: bool) { todo!("C1") }
/// The paragraph ends (`\par` in horizontal mode, or a vertical command).
pub fn end_graf(_c: &mut dyn Ctx) { todo!("C1") }
/// Appends to a vertical list with interline glue (`append_to_vlist`).
pub fn append_to_vlist(_c: &mut dyn Ctx, _b: std::sync::Arc<crate::item::BoxNode>) { todo!("C1") }
