//! Type 1 fonts as the Type1C (CFF) subsets pdfwrite embeds
//! (gdevpsf2.c `psf_write_type2_font`, gdevpsfx.c's Type 1 to Type 2
//! charstring conversion).

use alloc::vec::Vec;

use crate::font::Font;
use crate::interp::Ps;

/// The CFF of `font`'s `glyphs` (names, sorted, `.notdef` among them).
pub fn write(
    _ps: &mut Ps<'_>,
    _font: &Font,
    _glyphs: &[Vec<u8>],
    _enc: &[Option<Vec<u8>>],
    _used: &[bool; 256],
    _bbox: &[i32; 4],
) -> Vec<u8> {
    Vec::new()
}
