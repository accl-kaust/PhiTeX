//! type1c.c, type1c.h: OpenType (CFF) simple fonts.
//!
//! `pdf_font *` is the font's id in `self.font.fonts`.

use crate::cff::{Card16, CffFont};
use crate::prelude::*;

impl Dpx {
    /// `pdf_font_open_type1c`: 0 ok, -1 not an OpenType/CFF font.
    pub fn pdf_font_open_type1c(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> i32 {
        todo!()
    }
    /// `add_SimpleMetrics` (static, prefixed: Dpx methods share a namespace): `Widths`, `FirstChar`, `LastChar`.
    #[allow(non_snake_case)]
    fn type1c_add_SimpleMetrics(
        &mut self,
        font_id: i32,
        cffont: &CffFont,
        widths: &[f64],
        num_glyphs: Card16,
    ) {
        todo!()
    }
    /// `pdf_font_load_type1c`.
    pub fn pdf_font_load_type1c(&mut self, font_id: i32) -> i32 {
        todo!()
    }
}
