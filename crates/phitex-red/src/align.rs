//! C6: `\halign` `\valign`: preamble, cells as thunks with templates pushed
//! around them, `\omit` `\span` `\noalign` `\cr` `\crcr` `\tabskip`, setting.
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unset { /* C6 */ }
pub fn align_group_end(_c: &mut dyn crate::ctx::Ctx, _kind: crate::env::GroupKind) { todo!("C6") }
