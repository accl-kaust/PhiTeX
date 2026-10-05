//! tt_gsub.c, tt_gsub.h: OpenType GSUB lookups (single, alternate,
//! ligature substitutions).
//!
//! The `clt_*`/`otl_gsub_read_*` readers fill their table in place and
//! return C's byte count (or -1); the `*_release` functions are `Drop`.
//! The `dpx_conf.verbose_level` MESGs are dropped, so `OtlGsub` methods
//! need no `Dpx`. In/out gids stay `&mut USHORT`.

use core::cmp::Ordering;

use crate::cmap::CMap;
use crate::prelude::*;
use crate::sfnt::{Fixed, SHORT, Sfnt, USHORT};

/// `VERBOSE_LEVEL_MIN` (tt_gsub.c).
pub const VERBOSE_LEVEL_MIN: i32 = 2;

pub const OTL_GSUB_TYPE_SINGLE: u16 = 1;
pub const OTL_GSUB_TYPE_MULTIPLE: u16 = 2;
pub const OTL_GSUB_TYPE_ALTERNATE: u16 = 3;
pub const OTL_GSUB_TYPE_LIGATURE: u16 = 4;
pub const OTL_GSUB_TYPE_CONTEXT: u16 = 5;
pub const OTL_GSUB_TYPE_CCONTEXT: u16 = 6;
pub const OTL_GSUB_TYPE_ESUBST: u16 = 7;

/// `GSUB_LIST_MAX`.
pub const GSUB_LIST_MAX: usize = 32;

/// `Offset` (tt_gsub.c).
pub type Offset = USHORT;
/// `GlyphID` (tt_gsub.c).
pub type GlyphID = USHORT;

/// `struct clt_record`.
#[derive(Clone, Debug, Default)]
pub struct CltRecord {
    /// 4-byte identifier (C's `char[5]`, NUL-terminated).
    pub tag: [u8; 4],
    pub offset: Offset,
}

/// `struct clt_range`.
#[derive(Clone, Debug, Default)]
pub struct CltRange {
    /// First GlyphID in the range (C's `Start`).
    pub start: GlyphID,
    /// Last GlyphID in the range (C's `End`).
    pub end: GlyphID,
    /// Coverage index of the first GID.
    pub start_coverage_index: USHORT,
}

/// `struct clt_record_list`.
#[derive(Clone, Debug, Default)]
pub struct CltRecordList {
    pub count: USHORT,
    pub record: Vec<CltRecord>,
}

/// `struct clt_number_list`.
#[derive(Clone, Debug, Default)]
pub struct CltNumberList {
    pub count: USHORT,
    pub value: Vec<USHORT>,
}

/// `struct clt_coverage`.
#[derive(Clone, Debug, Default)]
pub struct CltCoverage {
    /// 1 (list) or 2 (range).
    pub format: USHORT,
    /// Glyphs/range count.
    pub count: USHORT,
    /// GlyphIDs, in numerical order (format 1).
    pub list: Vec<GlyphID>,
    /// Glyph ranges, ordered by start (format 2).
    pub range: Vec<CltRange>,
}

/// `struct otl_gsub_header`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubHeader {
    /// 0x00010000.
    pub version: Fixed,
    pub script_list: Offset,
    pub feature_list: Offset,
    pub lookup_list: Offset,
}

/// `struct otl_gsub_single1`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubSingle1 {
    /// Added to the original GlyphID to get the substitute.
    pub delta_glyph_id: SHORT,
    pub coverage: CltCoverage,
}

/// `struct otl_gsub_single2`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubSingle2 {
    pub glyph_count: USHORT,
    /// Substitute GlyphIDs, by coverage index.
    pub substitute: Vec<GlyphID>,
    pub coverage: CltCoverage,
}

/// `struct otl_gsub_altset`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubAltset {
    pub glyph_count: USHORT,
    pub alternate: Vec<GlyphID>,
}

/// `struct otl_gsub_alternate1`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubAlternate1 {
    pub alternate_set_count: USHORT,
    pub alternate_set: Vec<OtlGsubAltset>,
    pub coverage: CltCoverage,
}

/// `struct otl_gsub_ligtab`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubLigtab {
    /// GlyphID of the ligature glyph.
    pub lig_glyph: GlyphID,
    pub comp_count: USHORT,
    /// `comp_count - 1` components (the first one excluded).
    pub component: Vec<GlyphID>,
}

/// `struct otl_gsub_ligset`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubLigset {
    pub ligature_count: USHORT,
    pub ligature: Vec<OtlGsubLigtab>,
}

/// `struct otl_gsub_ligature1`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubLigature1 {
    pub lig_set_count: USHORT,
    pub ligature_set: Vec<OtlGsubLigset>,
    pub coverage: CltCoverage,
}

/// `otl_gsub_subtab`'s `table` union.
#[derive(Clone, Debug, Default)]
pub enum OtlGsubTable {
    #[default]
    None,
    Single1(Box<OtlGsubSingle1>),
    Single2(Box<OtlGsubSingle2>),
    Alternate1(Box<OtlGsubAlternate1>),
    Ligature1(Box<OtlGsubLigature1>),
}

/// `struct otl_gsub_subtab`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubSubtab {
    pub lookup_type: USHORT,
    pub subst_format: USHORT,
    pub table: OtlGsubTable,
}

/// `struct clt_script_table`.
#[derive(Clone, Debug, Default)]
pub struct CltScriptTable {
    pub default_lang_sys: Offset,
    pub lang_sys_record: CltRecordList,
}

/// `struct clt_langsys_table`.
#[derive(Clone, Debug, Default)]
pub struct CltLangsysTable {
    /// Reserved.
    pub lookup_order: Offset,
    pub req_feature_index: USHORT,
    /// Indices into the FeatureList.
    pub feature_index: CltNumberList,
}

/// `struct clt_feature_table`.
#[derive(Clone, Debug, Default)]
pub struct CltFeatureTable {
    pub feature_params: Offset,
    pub lookup_list_index: CltNumberList,
}

/// `struct clt_lookup_table`.
#[derive(Clone, Debug, Default)]
pub struct CltLookupTable {
    pub lookup_type: USHORT,
    pub lookup_flag: USHORT,
    /// Offsets from the beginning of the Lookup table.
    pub sub_table_list: CltNumberList,
}

/// `struct otl_gsub_tab`.
#[derive(Clone, Debug, Default)]
pub struct OtlGsubTab {
    pub script: Vec<u8>,
    pub language: Vec<u8>,
    pub feature: Vec<u8>,
    pub num_subtables: i32,
    pub subtables: Vec<OtlGsubSubtab>,
}

/// `struct otl_gsub` (`otl_gsub`).
#[derive(Clone, Debug, Default)]
pub struct OtlGsub {
    pub num_gsubs: i32,
    /// -1: none.
    pub select: i32,
    /// The `first`/`gsub_entry` chain: indices into `gsubs`, in order.
    pub first: Vec<i32>,
    /// At most `GSUB_LIST_MAX`.
    pub gsubs: Vec<OtlGsubTab>,
}

/// `clt_read_record` (static).
fn clt_read_record(rec: &mut CltRecord, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_range` (static).
fn clt_read_range(rec: &mut CltRange, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_record_list` (static).
fn clt_read_record_list(list: &mut CltRecordList, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_number_list` (static).
fn clt_read_number_list(list: &mut CltNumberList, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_script_table` (static).
fn clt_read_script_table(tab: &mut CltScriptTable, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_langsys_table` (static).
fn clt_read_langsys_table(tab: &mut CltLangsysTable, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_feature_table` (static).
fn clt_read_feature_table(tab: &mut CltFeatureTable, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_lookup_table` (static).
fn clt_read_lookup_table(tab: &mut CltLookupTable, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_read_coverage` (static).
fn clt_read_coverage(cov: &mut CltCoverage, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `clt_lookup_coverage` (static): the coverage index of `gid`, or -1.
fn clt_lookup_coverage(cov: &CltCoverage, gid: USHORT) -> i32 {
    todo!()
}
/// `otl_gsub_read_single` (static).
fn otl_gsub_read_single(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `otl_gsub_read_alternate` (static).
fn otl_gsub_read_alternate(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `otl_gsub_read_ligature` (static).
fn otl_gsub_read_ligature(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `otl_gsub_read_header` (static).
fn otl_gsub_read_header(head: &mut OtlGsubHeader, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `otl_gsub_read_feat` (static): the subtables of `gsub`'s
/// script/language/feature; 0, or -1 if none.
fn otl_gsub_read_feat(gsub: &mut OtlGsubTab, sfont: &mut Sfnt) -> i32 {
    todo!()
}
/// `otl_gsub_apply_single` (static): 0 if `gid` was substituted, else -1.
fn otl_gsub_apply_single(subtab: &OtlGsubSubtab, gid: &mut USHORT) -> i32 {
    todo!()
}
/// `otl_gsub_apply_alternate` (static).
fn otl_gsub_apply_alternate(subtab: &OtlGsubSubtab, alt_idx: USHORT, gid: &mut USHORT) -> i32 {
    todo!()
}
/// `glyph_seq_cmp` (static): 0 if the sequences are equal.
fn glyph_seq_cmp(glyph_seq0: &[GlyphID], glyph_seq1: &[GlyphID]) -> i32 {
    todo!()
}
/// `otl_gsub_apply_ligature` (static): `gid_out` untouched on failure.
fn otl_gsub_apply_ligature(subtab: &OtlGsubSubtab, gid_in: &[USHORT], gid_out: &mut USHORT) -> i32 {
    todo!()
}
/// `scan_otl_tag` (static): status (0 or -1), script, language, feature
/// (each 4 bytes, space padded) of `scrp.lang.feat` (or `feat`) at the
/// start of `otl_tags` (which ends at C's `endptr`).
fn scan_otl_tag(otl_tags: Option<&[u8]>) -> (i32, [u8; 4], [u8; 4], [u8; 4]) {
    todo!()
}

impl OtlGsub {
    /// `otl_gsub_new`.
    #[must_use]
    pub fn otl_gsub_new() -> OtlGsub {
        todo!()
    }
    /// `otl_gsub_release`.
    pub fn otl_gsub_release(self) {}
    /// `clear_chain` (static).
    fn clear_chain(&mut self) {
        todo!()
    }
    /// `otl_gsub_add_feat`: 0, or -1 if absent/already there.
    pub fn otl_gsub_add_feat(
        &mut self,
        script: &[u8],
        language: &[u8],
        feature: &[u8],
        sfont: &mut Sfnt,
    ) -> i32 {
        todo!()
    }
    /// `gsub_find` (static): the index in `gsubs`, or -1.
    fn gsub_find(&self, script: &[u8], language: &[u8], feature: &[u8]) -> i32 {
        todo!()
    }
    /// `otl_gsub_select`: the index selected, or -1.
    pub fn otl_gsub_select(&mut self, script: &[u8], language: &[u8], feature: &[u8]) -> i32 {
        todo!()
    }
    /// `otl_gsub_apply`: in/out `gid`; 0 if substituted, else -1.
    pub fn otl_gsub_apply(&self, gid: &mut USHORT) -> i32 {
        todo!()
    }
    /// `otl_gsub_apply_alt`: in/out `gid`.
    pub fn otl_gsub_apply_alt(&self, alt_idx: USHORT, gid: &mut USHORT) -> i32 {
        todo!()
    }
    /// `otl_gsub_apply_lig`: `num_gids` is `gid_in.len()`; `gid_out`
    /// written on success only.
    pub fn otl_gsub_apply_lig(&self, gid_in: &[USHORT], gid_out: &mut USHORT) -> i32 {
        todo!()
    }
    /// `otl_gsub_add_feat_list`: the features of `otl_tags` (`:`-separated).
    pub fn otl_gsub_add_feat_list(&mut self, otl_tags: &[u8], sfont: &mut Sfnt) -> i32 {
        todo!()
    }
    /// `otl_gsub_set_chain`.
    pub fn otl_gsub_set_chain(&mut self, otl_tags: &[u8]) -> i32 {
        todo!()
    }
    /// `otl_gsub_apply_chain`: in/out `gid`.
    pub fn otl_gsub_apply_chain(&self, gid: &mut USHORT) -> i32 {
        todo!()
    }
}

/// `add_glyph_if_valid` (static).
fn add_glyph_if_valid(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
    gid: USHORT,
    gid_sub: USHORT,
) -> i32 {
    todo!()
}

/// `add_ToUnicode_single` (static).
fn add_ToUnicode_single(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    subtab: &OtlGsubSubtab,
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
) -> i32 {
    todo!()
}

/// `add_alternate1_inverse_map` (static).
fn add_alternate1_inverse_map(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
    gid: USHORT,
    idx: i32,
    data: &OtlGsubAlternate1,
) -> i32 {
    todo!()
}

/// `add_ToUnicode_alternate` (static).
fn add_ToUnicode_alternate(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    subtab: &OtlGsubSubtab,
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
) -> i32 {
    todo!()
}

/// `add_ligature1_inverse_map` (static).
fn add_ligature1_inverse_map(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
    gid_1: USHORT,
    idx: i32,
    data: &OtlGsubLigature1,
) -> i32 {
    todo!()
}

/// `add_ToUnicode_ligature` (static).
fn add_ToUnicode_ligature(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    subtab: &OtlGsubSubtab,
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
) -> i32 {
    todo!()
}

/// `otl_gsub_add_ToUnicode`: ToUnicode entries (into the local `cmap`)
/// for used CIDs reached by GSUB; clears their bits in `used_chars`;
/// the count added.
pub fn otl_gsub_add_ToUnicode(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
    sfont: &mut Sfnt,
) -> i32 {
    todo!()
}
