//! fontmap.c, fontmap.h: the font map (`.map` files, `\special{pdf:mapline}`,
//! XeTeX's native fonts).
//!
//! The map is `self.fontmap.fontmap`, an [`HtTable`] of [`FontmapRec`]s
//! keyed by TeX font name (`None` before `pdf_init_fontmaps`). Lookups
//! return an owned copy of the record. SFD subfonts (`foo@SFD@`) need
//! subfont.c, which is not ported: those paths are `todo!()`.

use crate::dpxutil::HtTable;
use crate::prelude::*;

/// `FONTMAP_RMODE_REPLACE`.
pub const FONTMAP_RMODE_REPLACE: i32 = 0;
/// `FONTMAP_RMODE_APPEND`.
pub const FONTMAP_RMODE_APPEND: i32 = b'+' as i32;
/// `FONTMAP_RMODE_REMOVE`.
pub const FONTMAP_RMODE_REMOVE: i32 = b'-' as i32;

/// `FONTMAP_OPT_NOEMBED`.
pub const FONTMAP_OPT_NOEMBED: i32 = 1 << 1;
/// `FONTMAP_OPT_VERT`.
pub const FONTMAP_OPT_VERT: i32 = 1 << 2;

/// `FONTMAP_STYLE_NONE`.
pub const FONTMAP_STYLE_NONE: i32 = 0;
/// `FONTMAP_STYLE_BOLD`.
pub const FONTMAP_STYLE_BOLD: i32 = 1;
/// `FONTMAP_STYLE_ITALIC`.
pub const FONTMAP_STYLE_ITALIC: i32 = 2;
/// `FONTMAP_STYLE_BOLDITALIC`.
pub const FONTMAP_STYLE_BOLDITALIC: i32 = 3;

/// `CID_MAPREC_CSI_DELIM`.
pub const CID_MAPREC_CSI_DELIM: u8 = b'/';

/// `fontmap_opt`.
#[derive(Clone, Debug, PartialEq)]
pub struct FontmapOpt {
    pub slant: f64,
    pub extend: f64,
    pub bold: f64,
    pub mapc: i32,
    pub flags: i32,
    pub otl_tags: Option<Vec<u8>>,
    pub tounicode: Option<Vec<u8>>,
    pub design_size: f64,
    /// Adobe-Japan1-4, etc.
    pub charcoll: Option<Vec<u8>>,
    /// TTC index.
    pub index: u32,
    pub style: i32,
    pub stemv: i32,
    pub use_glyph_encoding: i32,
}

/// `pdf_init_fontmap_record`'s values.
impl Default for FontmapOpt {
    fn default() -> Self {
        FontmapOpt {
            slant: 0.0,
            extend: 1.0,
            bold: 0.0,
            mapc: -1,
            flags: 0,
            otl_tags: None,
            tounicode: None,
            design_size: -1.0,
            charcoll: None,
            index: 0,
            style: FONTMAP_STYLE_NONE,
            stemv: -1,
            use_glyph_encoding: 0,
        }
    }
}

/// `fontmap_rec`'s `charmap` (SFD subfont mapping).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontmapCharmap {
    pub sfd_name: Option<Vec<u8>>,
    pub subfont_id: Option<Vec<u8>>,
}

/// `fontmap_rec`. `Default` is `pdf_init_fontmap_record`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontmapRec {
    pub map_name: Option<Vec<u8>>,
    pub font_name: Option<Vec<u8>>,
    pub enc_name: Option<Vec<u8>>,
    pub charmap: FontmapCharmap,
    pub opt: FontmapOpt,
}

/// fontmap.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `fontmap` (NULL before `pdf_init_fontmaps`).
    pub fontmap: Option<HtTable<FontmapRec>>,
}

/// `pdf_init_fontmap_record`.
pub fn pdf_init_fontmap_record(mrec: &mut FontmapRec) {
    todo!()
}

/// `pdf_clear_fontmap_record`.
pub fn pdf_clear_fontmap_record(mrec: &mut FontmapRec) {
    todo!()
}

/// `pdf_copy_fontmap_record` (static).
fn pdf_copy_fontmap_record(dst: &mut FontmapRec, src: &FontmapRec) {
    todo!()
}

/// `fill_in_defaults` (static).
fn fill_in_defaults(mrec: &mut FontmapRec, tex_name: &[u8]) {
    todo!()
}

/// `readline` (static): a line of at most `buf_len - 1` bytes, `%`
/// comments and trailing blanks removed, or none at the end.
fn readline(buf_len: usize, fp: &mut MemFile) -> Option<Vec<u8>> {
    todo!()
}

/// `skip_blank` (static).
fn skip_blank(s: &[u8], pp: &mut usize) {
    todo!()
}

/// `parse_string_value` (static).
fn parse_string_value(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `parse_integer_value` (static).
fn parse_integer_value(s: &[u8], pp: &mut usize, base: i32) -> Option<Vec<u8>> {
    todo!()
}

/// `fontmap_parse_mapdef_dpm` (static): dvipdfm format; 0 or -1.
fn fontmap_parse_mapdef_dpm(mrec: &mut FontmapRec, mapdef: &[u8]) -> i32 {
    todo!()
}

/// `fontmap_parse_mapdef_dps` (static): dvips/pdfTeX format; 0 or -1.
fn fontmap_parse_mapdef_dps(mrec: &mut FontmapRec, mapdef: &[u8]) -> i32 {
    todo!()
}

/// `chop_sfd_name` (static): the font name without `@SFD@` and the SFD
/// name, or none.
fn chop_sfd_name(tex_name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    todo!()
}

/// `make_subfont_name` (static).
fn make_subfont_name(map_name: &[u8], sfd_name: &[u8], sub_id: &[u8]) -> Option<Vec<u8>> {
    todo!()
}

/// `is_pdfm_mapline`: -1 dvips/pdfTeX format, 1 dvipdfm format, 0 either
/// (two entries); see C.
#[must_use]
pub fn is_pdfm_mapline(mline: &[u8]) -> i32 {
    todo!()
}

/// `pdf_read_fontmap_line`: `mline` is the whole line (C's
/// `mline_strlen`); `format` > 0 dvipdfm, < 0 dvips, 0 guess. 0 or -1.
pub fn pdf_read_fontmap_line(mrec: &mut FontmapRec, mline: &[u8], format: i32) -> i32 {
    todo!()
}

/// `substr` (static): the bytes before `stop`, advancing `*pp` past it.
fn substr(s: &[u8], pp: &mut usize, stop: u8) -> Option<Vec<u8>> {
    todo!()
}

/// `strip_options` (static): the font name without `:n:`, `!`, `/csi`,
/// `,Bold` options, which go to `opt`.
fn strip_options(map_name: &[u8], opt: &mut FontmapOpt) -> Option<Vec<u8>> {
    todo!()
}

impl Dpx {
    /// `pdf_init_fontmaps`.
    pub fn pdf_init_fontmaps(&mut self) {
        todo!()
    }

    /// `pdf_close_fontmaps`.
    pub fn pdf_close_fontmaps(&mut self) {
        todo!()
    }

    /// `pdf_load_fontmap_file`: `mode` is a `FONTMAP_RMODE_*`; 0 or -1.
    pub fn pdf_load_fontmap_file(&mut self, filename: &[u8], mode: i32) -> i32 {
        todo!()
    }

    /// `pdf_append_fontmap_record`: 0 or -1.
    pub fn pdf_append_fontmap_record(&mut self, kp: &[u8], mrec: &FontmapRec) -> i32 {
        todo!()
    }

    /// `pdf_remove_fontmap_record`: 0 or -1.
    pub fn pdf_remove_fontmap_record(&mut self, kp: &[u8]) -> i32 {
        todo!()
    }

    /// `pdf_insert_fontmap_record`: a copy of the record inserted, or none.
    pub fn pdf_insert_fontmap_record(
        &mut self,
        kp: &[u8],
        mrec: &FontmapRec,
    ) -> Option<FontmapRec> {
        todo!()
    }

    /// `pdf_lookup_fontmap_record`: a copy of the record.
    pub fn pdf_lookup_fontmap_record(&mut self, kp: &[u8]) -> Option<FontmapRec> {
        todo!()
    }

    /// `pdf_insert_native_fontmap_record`: a copy of the record inserted.
    pub fn pdf_insert_native_fontmap_record(
        &mut self,
        filename: &[u8],
        index: u32,
        layout_dir: i32,
        extend: i32,
        slant: i32,
        embolden: i32,
    ) -> Option<FontmapRec> {
        todo!()
    }
}
