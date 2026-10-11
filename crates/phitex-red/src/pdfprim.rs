//! D3: pdfTeX primitives (`\pdfobj` `\pdfrefobj` `\pdfannot` `\pdfstartlink`
//! `\pdfendlink` `\pdfdest` `\pdfoutline` `\pdfliteral` `\pdfxform` `\pdfximage`
//! `\pdfrefx*` `\pdfcatalog` `\pdfinfo` `\pdfnames` `\pdftrailer` `\pdfsavepos`
//! `\pdflastx/ypos` `\pdfcolorstack` `\pdfsetmatrix` `\pdfsave/restore` …).
use crate::prim::Registry;
pub fn register(_r: &mut Registry) {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfWhat { /* D3 */ }
