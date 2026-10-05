//! XeTeX's fonts, without FreeType, HarfBuzz, ICU, fontconfig or TECkit
//! (DESIGN 4.7, part 2).
//!
//! `no_std` + `alloc`: no file is read here. Font bytes come in as
//! `Arc<[u8]>` from the caller (the engine's Host), and a [`FontSource`]
//! answers the file lookups the XeTeX layer needs.
//!
//! Two layers:
//!
//! - **The core**, neutral between engines (XeTeX now, LuaTeX later):
//!   [`face::Face`] (a face parsed once, shared by content, with
//!   FreeType 2.14's unscaled numbers: advances, control boxes,
//!   ascender/descender, the character map FreeType picks, glyph names),
//!   [`shape`] (HarfRust with font functions that answer as XeTeX's
//!   FreeType callbacks do; glyph ids, clusters, advances and offsets in
//!   font units), [`math`] (the MATH table as HarfBuzz reads it),
//!   [`layout`] (OpenType script, language and feature lists, the `size`
//!   feature), [`bidi`] (directional runs), [`teckit`] (TECkit's
//!   runtime), [`names`] and [`index`] (the raw name records of every
//!   face, serialized).
//! - **[`xetex`]**, XeTeX's semantics on top: `XeTeX_ext.c`'s font
//!   functions in `Fixed` and `float` arithmetic, the feature strings,
//!   `XeTeXFontMgr`'s name matching over an [`index::FontIndex`],
//!   `measure_native_node`'s layout as plain per-glyph values, and
//!   `makefontdef`'s bytes.

#![no_std]
// The ports keep the C sources' names and arithmetic: float/int casts are
// what XeTeX does (`(int)(d * 65536.0 + 0.5)`), not accidents.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::missing_panics_doc,
    // Prose names programs and formats (FreeType, HarfBuzz, XeTeX, …).
    clippy::doc_markdown
)]

extern crate alloc;

pub mod bidi;
pub mod face;
pub mod index;
mod inflate;
pub mod layout;
pub mod math;
pub mod names;
pub mod shape;
pub mod teckit;
pub mod woff;
pub mod xetex;

pub use face::Face;

/// A four-byte OpenType tag as a big-endian number (`hb_tag_t`).
pub type Tag = u32;

/// `HB_TAG(a,b,c,d)`.
#[must_use]
pub const fn tag(s: &[u8; 4]) -> Tag {
    u32::from_be_bytes(*s)
}

/// What the Host answers for the fonts: file bytes and kpathsea lookups.
///
/// The name lookup (`"Name/B/I"`) needs no query: it runs in this crate
/// over the [`index::FontIndex`] the Host gives
/// ([`xetex::fontmgr::FontManager`]).
pub trait FontSource {
    /// The bytes of the file at `path` (an absolute path from the index or
    /// from [`FontSource::find_file`]).
    fn read(&self, path: &str) -> Option<alloc::sync::Arc<[u8]>>;

    /// kpathsea's `kpse_find_file(name, format, must_exist=0)`: the path,
    /// or `None`.
    fn find_file(&self, name: &str, format: KpseFormat) -> Option<alloc::string::String>;
}

/// The kpathsea formats XeTeX's font code searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KpseFormat {
    /// `kpse_opentype_format` (`.otf`, `OPENTYPEFONTS`).
    OpenType,
    /// `kpse_truetype_format` (`.ttf`, `.ttc`, `TTFONTS`).
    TrueType,
    /// `kpse_type1_format` (`.pfb`, `.pfa`, `T1FONTS`).
    Type1,
    /// `kpse_miscfonts_format` (where `.tec` mappings live).
    MiscFonts,
}
