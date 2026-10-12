//! `writettf.c`: TrueType font files (`/FontFile2`), subset to the
//! glyphs used.

use std::collections::BTreeSet;

use crate::PdfError;
use crate::map::MapEntry;
use crate::t1::FontDesc;

/// `writettf`: the font file to embed.
///
/// # Errors
/// Not written yet.
pub(crate) fn write_ttf(
    _data: &[u8],
    _fm: &MapEntry,
    _fd: &mut FontDesc,
    _glyph_names: bool,
    _tags: &mut BTreeSet<[u8; 6]>,
) -> Result<Vec<u8>, PdfError> {
    Err(PdfError::Unsupported("TrueType font files".into()))
}
