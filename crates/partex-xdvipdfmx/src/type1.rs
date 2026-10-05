//! type1.c, type1.h: Type 1 fonts, embedded as CFF (FontFile3/Type1C).
//!
//! `pdf_font *` is the font's id in `self.font.fonts`.

use crate::cff::CffFont;
use crate::prelude::*;

/// Fixed-width font.
pub const FONT_FLAG_FIXEDPITCH: i32 = 1 << 0;
/// Serif font.
pub const FONT_FLAG_SERIF: i32 = 1 << 1;
/// Symbolic font.
pub const FONT_FLAG_SYMBOLIC: i32 = 1 << 2;
/// Script font.
pub const FONT_FLAG_SCRIPT: i32 = 1 << 3;
/// Adobe Standard Character Set.
pub const FONT_FLAG_STANDARD: i32 = 1 << 5;
/// Italic.
pub const FONT_FLAG_ITALIC: i32 = 1 << 6;
/// All-cap font.
pub const FONT_FLAG_ALLCAP: i32 = 1 << 16;
/// Small-cap font.
pub const FONT_FLAG_SMALLCAP: i32 = 1 << 17;
/// Force bold at small text sizes.
pub const FONT_FLAG_FORCEBOLD: i32 = 1 << 18;

/// `basefonts` of `is_basefont`: the 14 standard fonts.
pub static BASEFONTS: [&[u8]; 14] = [
    b"Courier",
    b"Courier-Bold",
    b"Courier-Oblique",
    b"Courier-BoldOblique",
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Symbol",
    b"Times-Roman",
    b"Times-Bold",
    b"Times-Italic",
    b"Times-BoldItalic",
    b"ZapfDingbats",
];

/// `is_basefont` (static).
fn is_basefont(name: &[u8]) -> bool {
    todo!()
}

impl Dpx {
    /// `pdf_font_open_type1`: 0 ok, -1 not a Type 1 font.
    pub fn pdf_font_open_type1(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> i32 {
        todo!()
    }
    /// `get_font_attr` (static; prefixed: Dpx methods share a namespace): the descriptor's attributes.
    fn type1_get_font_attr(&mut self, font_id: i32, cffont: &CffFont) {
        todo!()
    }
    /// `add_metrics` (static, prefixed): `Widths`, `FirstChar`, `LastChar`.
    fn type1_add_metrics(
        &mut self,
        font_id: i32,
        cffont: &CffFont,
        enc_vec: &[Option<Vec<u8>>],
        widths: &[f64],
        num_glyphs: i32,
    ) {
        todo!()
    }
    /// `write_fontfile` (static, prefixed, the non-LIBDPX one): the FontFile3
    /// stream; `pdfcharset` the CharSet string.
    fn type1_write_fontfile(
        &mut self,
        font_id: i32,
        cffont: &mut CffFont,
        pdfcharset: Option<Obj>,
    ) -> i32 {
        todo!()
    }
    /// `pdf_font_load_type1`.
    pub fn pdf_font_load_type1(&mut self, font_id: i32) -> i32 {
        todo!()
    }
}
