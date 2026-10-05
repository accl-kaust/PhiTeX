//! `XeTeX`'s whatsits (`XeTeX` §169–§170): words in native fonts, single
//! glyphs, and pictures. They are boxes to packaging and line breaking
//! (width, height, depth); what they draw is theirs.
//!
//! A native word keeps its text (UTF-16, as `XeTeX` does) and, once
//! measured, its glyphs as plain data (identifier and position each), not
//! packed bytes: so a list's glyphs can be read by anyone (DESIGN 4.7).

use alloc::sync::Arc;

use crate::Scaled;
use crate::node::FontId;

/// One glyph of a native word: its identifier and position from the
/// word's origin (`XeTeX`'s glyph info: 16-bit identifier, 32-bit `x` and
/// `y`; `y` grows downwards).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NativeGlyph {
    pub gid: u16,
    pub x: Scaled,
    pub y: Scaled,
}

/// A `native_word_node` (subtype 40, or 41 `native_word_node_AT` when
/// `actual_text`: the PDF gets the text as `/ActualText`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NativeWord {
    pub font: FontId,
    pub actual_text: bool,
    /// The characters, UTF-16 (`native_length` of them).
    pub text: Arc<[u16]>,
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
    /// The glyphs `set_native_metrics` laid out (none until measured).
    pub glyphs: Arc<[NativeGlyph]>,
}

impl NativeWord {
    /// A word of `text` in `font`, not yet measured (`new_native_word_node`).
    #[must_use]
    pub fn new(font: FontId, actual_text: bool, text: Arc<[u16]>) -> Self {
        Self {
            font,
            actual_text,
            text,
            width: 0,
            height: 0,
            depth: 0,
            glyphs: Arc::from([]),
        }
    }
}

/// A `glyph_node` (subtype 42): one glyph of a native font by identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphNode {
    pub font: FontId,
    pub gid: u16,
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
}

/// A `pic_node` (43) or `pdf_node` (44): a picture file placed with a
/// transform.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PicNode {
    /// Whether the file is a PDF (`pdf_node`).
    pub pdf: bool,
    /// The file's path, as the XDV names it.
    pub path: Arc<[u8]>,
    pub page: i32,
    /// `pic_pdf_box`: which PDF box (`\XeTeXpdffile ... crop` …).
    pub pdf_box: u8,
    /// The transform: `a b c d tx ty` (`pic_transform1..6`, Fixed).
    pub transform: [i32; 6],
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
}
