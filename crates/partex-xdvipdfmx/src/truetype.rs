//! truetype.c, truetype.h: simple (non-CID) TrueType fonts.

use crate::prelude::*;
use crate::sfnt::{Sfnt, USHORT};
use crate::tt_cmap::TtCmap;
use crate::tt_gsub::OtlGsub;
use crate::tt_post::TtPostTable;

/// `required_table`: the tables kept in an embedded TrueType font, and
/// whether each must exist.
pub const REQUIRED_TABLE: [(&[u8], i32); 12] = [
    (b"OS/2", 0),
    (b"head", 1),
    (b"hhea", 1),
    (b"loca", 1),
    (b"maxp", 1),
    (b"name", 1),
    (b"glyf", 1),
    (b"hmtx", 1),
    (b"fpgm", 0),
    (b"cvt ", 0),
    (b"prep", 0),
    (b"cmap", 1),
];

/// `struct glyph_mapper`: glyph name to gid lookups, borrowing the font.
#[derive(Debug)]
pub struct GlyphMapper<'a> {
    pub codetogid: Option<TtCmap>,
    pub gsub: Option<OtlGsub>,
    pub sfont: &'a mut Sfnt,
    pub nametogid: Option<TtPostTable>,
}

/// `agl_decompose_glyphname` (static): the count, the `_`-separated
/// components (at most `size`; C's `nptrs`, ERROR beyond) and the suffix
/// after the first `.` (C's `*suffix`).
fn agl_decompose_glyphname(glyphname: &[u8], size: i32) -> (i32, Vec<Vec<u8>>, Option<Vec<u8>>) {
    todo!()
}

/// `select_gsub` (static): loads/selects GSUB feature `feat`; 0, or -1.
fn select_gsub(feat: &[u8], gm: &mut GlyphMapper<'_>) -> i32 {
    todo!()
}

/// `composeglyph` (static): ligature substitution of `glyphs` (feature
/// `feat`, or the default ones); status and gid.
fn composeglyph(glyphs: &[USHORT], feat: Option<&[u8]>, gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
    todo!()
}

/// `composeuchar` (static): `composeglyph` of the gids of `unicodes`.
fn composeuchar(unicodes: &[i32], feat: Option<&[u8]>, gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
    todo!()
}

/// `findposttable` (static): status and gid from the `post` table.
fn findposttable(glyph_name: &[u8], gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
    todo!()
}

/// `setup_glyph_mapper` (static): status and the mapper (C fills `gm`).
fn setup_glyph_mapper(sfont: &mut Sfnt) -> (i32, GlyphMapper<'_>) {
    todo!()
}

/// `clean_glyph_mapper` (static).
fn clean_glyph_mapper(gm: GlyphMapper<'_>) {}

impl Dpx {
    /// `pdf_font_open_truetype`: 0, or -1 if not a usable TrueType font.
    pub fn pdf_font_open_truetype(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
    ) -> i32 {
        todo!()
    }

    /// `pdf_font_load_truetype`: 0, or -1 on error.
    pub fn pdf_font_load_truetype(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `do_widths` (static): /Widths, /FirstChar, /LastChar of the font.
    fn do_widths(&mut self, font_id: i32, widths: &[f64; 256]) {
        todo!()
    }

    /// `do_builtin_encoding` (static): subset by the (3,0)/(1,0) cmap.
    fn do_builtin_encoding(&mut self, font_id: i32, usedchars: &[u8], sfont: &mut Sfnt) -> i32 {
        todo!()
    }

    /// `do_custom_encoding` (static): subset by glyph names; `encoding` is
    /// the 256 glyph names of the encoding (`pdf_encoding_get_encoding`).
    fn do_custom_encoding(
        &mut self,
        font_id: i32,
        encoding: &[Option<Vec<u8>>],
        usedchars: &[u8],
        sfont: &mut Sfnt,
    ) -> i32 {
        todo!()
    }

    /// `selectglyph` (static): the variant of gid `in_` for `suffix`
    /// (`.sc`, `.onum`, …, through GSUB); status and gid.
    fn selectglyph(
        &mut self,
        in_: USHORT,
        suffix: &[u8],
        gm: &mut GlyphMapper<'_>,
    ) -> (i32, USHORT) {
        todo!()
    }

    /// `findcomposite` (static): `a_b_c` names through ligatures.
    fn findcomposite(&mut self, glyphname: &[u8], gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
        todo!()
    }

    /// `findparanoiac` (static): AGL lookups, alternates, compositions.
    fn findparanoiac(&mut self, glyphname: &[u8], gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
        todo!()
    }

    /// `resolve_glyph` (static): a glyph name to a gid; status and gid.
    fn resolve_glyph(&mut self, glyphname: &[u8], gm: &mut GlyphMapper<'_>) -> (i32, USHORT) {
        todo!()
    }
}
