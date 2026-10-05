//! cidtype0.c, cidtype0.h: CIDFontType0 (CFF CIDFonts, OpenType/CFF, and
//! Type1 / Type1C converted to CFF CIDFonts).
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; `cff_font *`
//! is `&mut CffFont` (owned by the caller, closed with `cff_close`);
//! `sfnt *` is `&mut Sfnt`. `CIDToGIDMap`, `used_chars` are byte slices.

#![allow(non_snake_case)]

use crate::cff::CffFont;
use crate::cmap::Cid;
use crate::pdffont::CidOpt;
use crate::prelude::*;
use crate::sfnt::Sfnt;
use crate::tt_table::{TtHeadTable, TtLongMetrics, TtMaxpTable};

/// `TYPE1_NAME_LEN_MAX`.
pub const TYPE1_NAME_LEN_MAX: usize = 127;
/// `WBUF_SIZE` (`create_ToUnicode_stream`).
pub const WBUF_SIZE: usize = 1024;

/// `FONT_FLAG_FIXEDPITCH`: fixed-width font.
pub const FONT_FLAG_FIXEDPITCH: i32 = 1 << 0;
/// `FONT_FLAG_SERIF`.
pub const FONT_FLAG_SERIF: i32 = 1 << 1;
/// `FONT_FLAG_SYMBOLIC`.
pub const FONT_FLAG_SYMBOLIC: i32 = 1 << 2;
/// `FONT_FLAG_SCRIPT`.
pub const FONT_FLAG_SCRIPT: i32 = 1 << 3;
/// `FONT_FLAG_STANDARD`: Adobe standard character set.
pub const FONT_FLAG_STANDARD: i32 = 1 << 5;
/// `FONT_FLAG_ITALIC`.
pub const FONT_FLAG_ITALIC: i32 = 1 << 6;
/// `FONT_FLAG_ALLCAP`.
pub const FONT_FLAG_ALLCAP: i32 = 1 << 16;
/// `FONT_FLAG_SMALLCAP`.
pub const FONT_FLAG_SMALLCAP: i32 = 1 << 17;
/// `FONT_FLAG_FORCEBOLD`.
pub const FONT_FLAG_FORCEBOLD: i32 = 1 << 18;

/// `get_font_attr`'s `L_c`: glyphs for the cap height.
pub const L_C: &[&[u8]] = &[b"H", b"P", b"Pi", b"Rho"];
/// `get_font_attr`'s `L_d`: glyphs for the descent.
pub const L_D: &[&[u8]] = &[b"p", b"q", b"mu", b"eta"];
/// `get_font_attr`'s `L_a`: glyphs for the ascent.
pub const L_A: &[&[u8]] = &[b"b", b"h", b"lambda"];

impl Dpx {
    /// `add_CIDHMetrics` (static): `/W` from `hmtx`.
    fn add_CIDHMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        maxp: &TtMaxpTable,
        head: &TtHeadTable,
        hmtx: &[TtLongMetrics],
    ) {
        todo!()
    }

    /// `add_CIDVMetrics` (static): `/DW2`, `/W2` (VORG, vmtx).
    fn add_CIDVMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        maxp: &TtMaxpTable,
        head: &TtHeadTable,
        hmtx: &[TtLongMetrics],
    ) {
        todo!()
    }

    /// `add_CIDMetrics` (static).
    fn add_CIDMetrics(
        &mut self,
        sfont: &mut Sfnt,
        fontdict: Obj,
        cid_to_gid_map: Option<&[u8]>,
        last_cid: u16,
        need_vmetrics: i32,
    ) {
        todo!()
    }

    /// `write_fontfile` (static): the embedded `/FontFile3` (CIDFontType0C).
    fn write_fontfile(&mut self, font_id: i32, cffont: &mut CffFont) -> i32 {
        todo!()
    }

    /// `CIDFont_type0_add_CIDSet` (static): PDF/A `/CIDSet`.
    fn CIDFont_type0_add_CIDSet(&mut self, font_id: i32, used_chars: &[u8], last_cid: u16) {
        todo!()
    }

    /// `CIDFont_type0_dofont`: 0 or an error.
    pub fn CIDFont_type0_dofont(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `CIDFont_type0_open_from_t1`: 0, or -1 if `name` is not a Type 1 font.
    pub fn CIDFont_type0_open_from_t1(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> i32 {
        todo!()
    }

    /// `CIDFont_type0_open`: 0, or -1 if `name` is not a CFF CIDFont
    /// (OpenType).
    pub fn CIDFont_type0_open(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> i32 {
        todo!()
    }

    /// `CIDFont_type0_open_from_t1c`: 0, or -1 if `name` is not a bare
    /// CFF (Type1C in OpenType).
    pub fn CIDFont_type0_open_from_t1c(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> i32 {
        todo!()
    }

    /// `CIDFont_type0_t1cdofont`.
    pub fn CIDFont_type0_t1cdofont(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `load_base_CMap` (static): the Unicode to CID CMap built from the
    /// glyph names; its cache id or -1.
    fn load_base_CMap(&mut self, font_name: &[u8], wmode: i32, cffont: &mut CffFont) -> i32 {
        todo!()
    }

    /// `t1_load_UnicodeCMap`: a cache id, or -1 (`otl_tags` not supported).
    pub fn t1_load_UnicodeCMap(
        &mut self,
        font_name: &[u8],
        otl_tags: Option<&[u8]>,
        wmode: i32,
    ) -> i32 {
        todo!()
    }

    /// `create_ToUnicode_stream` (static).
    fn create_ToUnicode_stream(
        &mut self,
        cffont: &mut CffFont,
        font_name: &[u8],
        used_glyphs: &[u8],
    ) -> Option<Obj> {
        todo!()
    }

    /// `CIDFont_type0_t1create_ToUnicode_stream`: a reference, or none.
    pub fn CIDFont_type0_t1create_ToUnicode_stream(
        &mut self,
        filename: &[u8],
        fontname: &[u8],
        used_chars: &[u8],
    ) -> Option<Obj> {
        todo!()
    }

    /// `get_font_attr` (static): descriptor entries from the glyphs.
    fn get_font_attr(&mut self, font_id: i32, cffont: &mut CffFont) {
        todo!()
    }

    /// `add_metrics` (static): `/W` (and `/DW`) from `widths`.
    fn add_metrics(
        &mut self,
        font_id: i32,
        cffont: &mut CffFont,
        cid_to_gid_map: &[u8],
        widths: &[f64],
        default_width: f64,
        last_cid: Cid,
    ) {
        todo!()
    }

    /// `CIDFont_type0_t1dofont`.
    pub fn CIDFont_type0_t1dofont(&mut self, font_id: i32) -> i32 {
        todo!()
    }
}
