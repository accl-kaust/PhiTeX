//! tt_cmap.c, tt_cmap.h: the sfnt `cmap` table, and the CMaps made from
//! it (ToUnicode streams, Unicode input CMaps for OpenType fonts).
//!
//! CMaps built here are local (`&mut CMap`, `CMap_new`) until
//! `CMap_cache_add`; cached ones (`cmap_add`, the `-UCS32-Add` CMap) are
//! named by their id (`i32`) in the CMap cache.

use crate::cff::CffFont;
use crate::cmap::CMap;
use crate::prelude::*;
use crate::sfnt::{BYTE, SHORT, Sfnt, ULONG, USHORT};
use crate::tt_gsub::OtlGsub;
use crate::tt_post::TtPostTable;

/// `VERBOSE_LEVEL_MIN` (tt_cmap.c).
pub const VERBOSE_LEVEL_MIN: i32 = 0;

/// `TT_MAC`.
pub const TT_MAC: u16 = 1;
/// `TT_WIN`.
pub const TT_WIN: u16 = 3;

pub const TT_WIN_SYMBOL: u16 = 0;
pub const TT_WIN_UNICODE: u16 = 1;
pub const TT_WIN_SJIS: u16 = 2;
pub const TT_WIN_RPC: u16 = 3;
pub const TT_WIN_BIG5: u16 = 4;
pub const TT_WIN_WANSUNG: u16 = 5;
pub const TT_WIN_JOHAB: u16 = 6;
pub const TT_WIN_UCS4: u16 = 10;

pub const TT_MAC_ROMAN: u16 = 0;
pub const TT_MAC_JAPANESE: u16 = 1;
pub const TT_MAC_TRADITIONAL_CHINESE: u16 = 2;
pub const TT_MAC_KOREAN: u16 = 3;
pub const TT_MAC_SIMPLIFIED_CHINESE: u16 = 25;

/// `MAX_UNICODES` (tt_cmap.c).
pub const MAX_UNICODES: usize = 32;

/// `srange_min`.
pub const SRANGE_MIN: [u8; 2] = [0x00, 0x00];
/// `srange_max`.
pub const SRANGE_MAX: [u8; 2] = [0xff, 0xff];
/// `lrange_min`.
pub const LRANGE_MIN: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
/// `lrange_max`.
pub const LRANGE_MAX: [u8; 4] = [0x7f, 0xff, 0xff, 0xff];

/// `cmap_plat_enc_rec`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CmapPlatEncRec {
    pub platform: i16,
    pub encoding: i16,
}

/// `cmap_plat_encs`: the subtables tried for a ToUnicode CMap, in order.
pub const CMAP_PLAT_ENCS: [CmapPlatEncRec; 6] = [
    CmapPlatEncRec {
        platform: 3,
        encoding: 10,
    },
    CmapPlatEncRec {
        platform: 0,
        encoding: 3,
    },
    CmapPlatEncRec {
        platform: 0,
        encoding: 4,
    },
    CmapPlatEncRec {
        platform: 0,
        encoding: 0,
    },
    CmapPlatEncRec {
        platform: 3,
        encoding: 1,
    },
    CmapPlatEncRec {
        platform: 0,
        encoding: 1,
    },
];

/// `struct cmap0`.
#[derive(Clone, Debug)]
pub struct Cmap0 {
    pub glyph_index_array: [BYTE; 256],
}

/// `struct SubHeader`.
#[derive(Clone, Debug, Default)]
pub struct SubHeader {
    pub first_code: USHORT,
    pub entry_count: USHORT,
    pub id_delta: SHORT,
    pub id_range_offset: USHORT,
}

/// `struct cmap2`.
#[derive(Clone, Debug)]
pub struct Cmap2 {
    pub sub_header_keys: [USHORT; 256],
    pub sub_headers: Vec<SubHeader>,
    pub glyph_index_array: Vec<USHORT>,
}

/// `struct cmap4`.
#[derive(Clone, Debug, Default)]
pub struct Cmap4 {
    pub seg_count_x2: USHORT,
    pub search_range: USHORT,
    pub entry_selector: USHORT,
    pub range_shift: USHORT,
    pub end_count: Vec<USHORT>,
    pub reserved_pad: USHORT,
    pub start_count: Vec<USHORT>,
    pub id_delta: Vec<USHORT>,
    pub id_range_offset: Vec<USHORT>,
    pub glyph_index_array: Vec<USHORT>,
}

/// `struct cmap6`.
#[derive(Clone, Debug, Default)]
pub struct Cmap6 {
    pub first_code: USHORT,
    pub entry_count: USHORT,
    pub glyph_index_array: Vec<USHORT>,
}

/// `struct charGroup`.
#[derive(Clone, Debug, Default)]
pub struct CharGroup {
    pub start_char_code: ULONG,
    pub end_char_code: ULONG,
    pub start_glyph_id: ULONG,
}

/// `struct cmap12`.
#[derive(Clone, Debug, Default)]
pub struct Cmap12 {
    pub n_groups: ULONG,
    pub groups: Vec<CharGroup>,
}

/// `struct variationSelector`.
#[derive(Clone, Debug, Default)]
pub struct VariationSelector {
    pub var_selector: ULONG,
    /// Default UVS table.
    pub num_unicode_value_ranges: ULONG,
    pub ranges_start_unicode_value: Vec<ULONG>,
    pub ranges_additional_count: Vec<BYTE>,
    /// Non-default UVS table.
    pub num_uvs_mappings: ULONG,
    pub uvs_mappings_unicode_value: Vec<ULONG>,
    pub uvs_mappings_glyph_id: Vec<USHORT>,
}

/// `struct cmap14`.
#[derive(Clone, Debug, Default)]
pub struct Cmap14 {
    pub num_var_selector_records: ULONG,
    pub var_selector: Vec<VariationSelector>,
}

/// `tt_cmap`'s `void *map`, by `format`.
#[derive(Clone, Debug, Default)]
pub enum TtCmapMap {
    #[default]
    None,
    Cmap0(Box<Cmap0>),
    Cmap2(Box<Cmap2>),
    Cmap4(Box<Cmap4>),
    Cmap6(Box<Cmap6>),
    Cmap12(Box<Cmap12>),
    Cmap14(Box<Cmap14>),
}

/// `tt_cmap`.
#[derive(Clone, Debug, Default)]
pub struct TtCmap {
    pub format: USHORT,
    pub platform: USHORT,
    pub encoding: USHORT,
    /// Or version, only for Mac.
    pub language: ULONG,
    pub map: TtCmapMap,
}

/// `read_cmap0` (static).
fn read_cmap0(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap0>> {
    todo!()
}
/// `lookup_cmap0` (static).
fn lookup_cmap0(map: &Cmap0, cc: USHORT) -> USHORT {
    todo!()
}
/// `read_cmap2` (static).
fn read_cmap2(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap2>> {
    todo!()
}
/// `lookup_cmap2` (static).
fn lookup_cmap2(map: &Cmap2, cc: USHORT) -> USHORT {
    todo!()
}
/// `read_cmap4` (static).
fn read_cmap4(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap4>> {
    todo!()
}
/// `lookup_cmap4` (static).
fn lookup_cmap4(map: &Cmap4, cc: USHORT) -> USHORT {
    todo!()
}
/// `read_cmap6` (static).
fn read_cmap6(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap6>> {
    todo!()
}
/// `lookup_cmap6` (static).
fn lookup_cmap6(map: &Cmap6, cc: USHORT) -> USHORT {
    todo!()
}
/// `read_cmap12` (static).
fn read_cmap12(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap12>> {
    todo!()
}
/// `lookup_cmap12` (static).
fn lookup_cmap12(map: &Cmap12, cccc: ULONG) -> USHORT {
    todo!()
}
/// `read_cmap14` (static).
fn read_cmap14(sfont: &mut Sfnt, offset: ULONG, len: ULONG) -> Option<Box<Cmap14>> {
    todo!()
}
/// `lookup_cmap14` (static).
fn lookup_cmap14(
    map: &Cmap14,
    cmap_default: Option<&TtCmap>,
    unicode: ULONG,
    uvs: ULONG,
) -> USHORT {
    todo!()
}

impl Sfnt {
    /// `tt_cmap_read`: the subtable for `platform`/`encoding`, if any.
    pub fn tt_cmap_read(&mut self, platform: USHORT, encoding: USHORT) -> Option<TtCmap> {
        todo!()
    }
}

impl TtCmap {
    /// `tt_cmap_release`.
    pub fn tt_cmap_release(self) {}
    /// `tt_cmap_lookup`: the gid of `cc`, 0 if none.
    #[must_use]
    pub fn tt_cmap_lookup(&self, cc: ULONG) -> USHORT {
        todo!()
    }
}

/// `tt_cmap_uvs_lookup`: the gid of `unicode` + selector `uvs` (format
/// 14 `cmap_uvs`, falling back to `cmap_default` for default UVS).
#[must_use]
pub fn tt_cmap_uvs_lookup(
    cmap_uvs: &TtCmap,
    cmap_default: Option<&TtCmap>,
    unicode: ULONG,
    uvs: ULONG,
) -> USHORT {
    todo!()
}

/// `create_GIDToCIDMap` (static): fills `gid_to_cid_map[..num_glyphs]`
/// from the CFF charset.
fn create_GIDToCIDMap(gid_to_cid_map: &mut [u16], num_glyphs: u16, cffont: &mut CffFont) {
    todo!()
}

/// `is_PUA_or_presentation` (static).
fn is_PUA_or_presentation(uni: u32) -> bool {
    todo!()
}

/// `lookup_glyph_name` (static): from `post`, else from the CFF charset.
fn lookup_glyph_name(
    post: Option<&TtPostTable>,
    cffont: Option<&mut CffFont>,
    gid: USHORT,
) -> Option<Vec<u8>> {
    todo!()
}

/// `create_inverse_cmap4` (static): fills `map_base`/`map_sub` (indexed
/// by gid, `num_glyphs` long).
fn create_inverse_cmap4(
    map_base: &mut [i32],
    map_sub: &mut [i32],
    num_glyphs: USHORT,
    map: &Cmap4,
) {
    todo!()
}

/// `create_inverse_cmap12` (static).
fn create_inverse_cmap12(
    map_base: &mut [i32],
    map_sub: &mut [i32],
    num_glyphs: USHORT,
    map: &Cmap12,
) {
    todo!()
}

/// `load_cmap4` (static): the Unicode input CMap `cmap` (local) from a
/// format 4 subtable; `gsub_vert`/`gsub_list` applied to each gid.
fn load_cmap4(
    map: &Cmap4,
    gid_to_cid_map: &[u16],
    num_glyphs: USHORT,
    gsub_vert: Option<&OtlGsub>,
    gsub_list: Option<&OtlGsub>,
    cmap: &mut CMap,
    map_base: &mut [i32],
    map_sub: &mut [i32],
) {
    todo!()
}

/// `load_cmap12` (static).
fn load_cmap12(
    map: &Cmap12,
    gid_to_cid_map: &[u16],
    num_glyphs: USHORT,
    gsub_vert: Option<&OtlGsub>,
    gsub_list: Option<&OtlGsub>,
    cmap: &mut CMap,
    map_base: &mut [i32],
    map_sub: &mut [i32],
) {
    todo!()
}

impl Dpx {
    /// `handle_subst_glyphs` (static): adds to `cmap` the used CIDs that
    /// `cmap_add` (a cached CMap id) maps; the count added. Clears the
    /// bits of the CIDs handled in `used_chars`.
    fn handle_subst_glyphs(
        &mut self,
        cmap: &mut CMap,
        cmap_add: i32,
        used_chars: &mut [u8],
    ) -> i32 {
        todo!()
    }

    /// `add_ToUnicode_via_glyph_name` (static): AGL lookups of the glyph
    /// names of used gids; the count added.
    fn add_ToUnicode_via_glyph_name(
        &mut self,
        cmap: &mut CMap,
        used_chars: &mut [u8],
        num_glyphs: USHORT,
        gid_to_cid_map: Option<&[u16]>,
        sfont: &mut Sfnt,
        cffont: Option<&mut CffFont>,
    ) -> i32 {
        todo!()
    }

    /// `create_ToUnicode_cmap` (static): the ToUnicode CMap stream;
    /// `cmap_add` is a cached CMap id, if any.
    fn create_ToUnicode_cmap(
        &mut self,
        ttcmap: &TtCmap,
        cmap_name: &[u8],
        cmap_add: Option<i32>,
        used_chars: &[u8],
        sfont: &mut Sfnt,
    ) -> Option<Obj> {
        todo!()
    }

    /// `otf_create_ToUnicode_stream`: a reference to the
    /// `<basefont>-UTF16` CMap resource (found or made), none on failure.
    pub fn otf_create_ToUnicode_stream(
        &mut self,
        map_name: &[u8],
        ttc_index: u32,
        basefont: &[u8],
        used_chars: &[u8],
    ) -> Option<Obj> {
        todo!()
    }

    /// `otf_load_Unicode_CMap`: the cmap id of the Unicode input CMap of
    /// the font (`otl_tags`: GSUB features to apply), -1 on failure.
    pub fn otf_load_Unicode_CMap(
        &mut self,
        map_name: &[u8],
        ttc_index: u32,
        otl_tags: Option<&[u8]>,
        wmode: i32,
    ) -> i32 {
        todo!()
    }

    /// `otf_try_load_GID_to_CID_map`: the cmap id of the GID-to-CID CMap
    /// of a CID-keyed CFF OpenType font, -1 if not one.
    pub fn otf_try_load_GID_to_CID_map(
        &mut self,
        map_name: &[u8],
        ttc_index: u32,
        wmode: i32,
    ) -> i32 {
        todo!()
    }
}
