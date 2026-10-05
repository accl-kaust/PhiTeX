//! tt_gsub.c, tt_gsub.h: OpenType GSUB lookups (single, alternate,
//! ligature substitutions).
//!
//! The `clt_*`/`otl_gsub_read_*` readers fill their table in place and
//! return C's byte count (or -1); the `*_release` functions are `Drop`.
//! The `dpx_conf.verbose_level` MESGs are dropped, so `OtlGsub` methods
//! need no `Dpx`. In/out gids stay `&mut USHORT`.

#![allow(non_snake_case)]

use core::cmp::Ordering;

use crate::cmap::CMap;
use crate::otl_opt::{OtlOpt, otl_match_optrule};
use crate::pdffont::is_used_char2;
use crate::prelude::*;
use crate::sfnt::{Fixed, SHORT, Sfnt, USHORT};
use crate::unicode::{UC_UTF16BE_encode_char, UC_is_valid};

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

/// A C string: the bytes up to the first NUL.
fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `clt_read_record` (static).
fn clt_read_record(rec: &mut CltRecord, sfont: &mut Sfnt) -> i32 {
    for i in 0..4 {
        rec.tag[i] = sfont.sfnt_get_char() as u8;
    }
    rec.offset = sfont.sfnt_get_ushort();
    6
}
/// `clt_read_range` (static).
fn clt_read_range(rec: &mut CltRange, sfont: &mut Sfnt) -> i32 {
    rec.start = sfont.sfnt_get_ushort();
    rec.end = sfont.sfnt_get_ushort();
    rec.start_coverage_index = sfont.sfnt_get_ushort();
    6
}
/// `clt_read_record_list` (static).
fn clt_read_record_list(list: &mut CltRecordList, sfont: &mut Sfnt) -> i32 {
    list.count = sfont.sfnt_get_ushort();
    let mut len = 2;
    list.record = Vec::with_capacity(list.count as usize);
    for _ in 0..list.count {
        let mut rec = CltRecord::default();
        len += clt_read_record(&mut rec, sfont);
        list.record.push(rec);
    }
    len
}
/// `clt_read_number_list` (static).
fn clt_read_number_list(list: &mut CltNumberList, sfont: &mut Sfnt) -> i32 {
    list.count = sfont.sfnt_get_ushort();
    list.value = Vec::with_capacity(list.count as usize);
    for _ in 0..list.count {
        list.value.push(sfont.sfnt_get_ushort());
    }
    2 + 2 * i32::from(list.count)
}
/// `clt_read_script_table` (static).
fn clt_read_script_table(tab: &mut CltScriptTable, sfont: &mut Sfnt) -> i32 {
    tab.default_lang_sys = sfont.sfnt_get_ushort();
    let mut len = 2;
    len += clt_read_record_list(&mut tab.lang_sys_record, sfont);
    len
}
/// `clt_read_langsys_table` (static).
fn clt_read_langsys_table(tab: &mut CltLangsysTable, sfont: &mut Sfnt) -> i32 {
    tab.lookup_order = sfont.sfnt_get_ushort();
    tab.req_feature_index = sfont.sfnt_get_ushort();
    let mut len = 4;
    len += clt_read_number_list(&mut tab.feature_index, sfont);
    len
}
/// `clt_read_feature_table` (static).
fn clt_read_feature_table(tab: &mut CltFeatureTable, sfont: &mut Sfnt) -> i32 {
    tab.feature_params = sfont.sfnt_get_ushort();
    let mut len = 2;
    len += clt_read_number_list(&mut tab.lookup_list_index, sfont);
    len
}
/// `clt_read_lookup_table` (static).
fn clt_read_lookup_table(tab: &mut CltLookupTable, sfont: &mut Sfnt) -> i32 {
    tab.lookup_type = sfont.sfnt_get_ushort();
    tab.lookup_flag = sfont.sfnt_get_ushort();
    let mut len = 4;
    len += clt_read_number_list(&mut tab.sub_table_list, sfont);
    len
}
/// `clt_read_coverage` (static).
fn clt_read_coverage(cov: &mut CltCoverage, sfont: &mut Sfnt) -> i32 {
    cov.format = sfont.sfnt_get_ushort();
    cov.count = sfont.sfnt_get_ushort();
    let mut len = 4;

    match cov.format {
        1 => {
            // list
            cov.list = Vec::with_capacity(cov.count as usize);
            for _ in 0..cov.count {
                cov.list.push(sfont.sfnt_get_ushort());
            }
            cov.range = Vec::new();
            len += 2 * i32::from(cov.count);
        }
        2 => {
            // range
            cov.range = Vec::with_capacity(cov.count as usize);
            for _ in 0..cov.count {
                let mut r = CltRange::default();
                len += clt_read_range(&mut r, sfont);
                cov.range.push(r);
            }
            cov.list = Vec::new();
        }
        _ => error!("Unknown coverage format"),
    }
    len
}
/// `clt_lookup_coverage` (static): the coverage index of `gid`, or -1.
fn clt_lookup_coverage(cov: &CltCoverage, gid: USHORT) -> i32 {
    match cov.format {
        1 => {
            // list
            for i in 0..cov.count as usize {
                if cov.list[i] > gid {
                    break;
                } else if cov.list[i] == gid {
                    return i as i32; /* found */
                }
            }
        }
        2 => {
            // range
            for i in 0..cov.count as usize {
                if gid < cov.range[i].start {
                    break;
                } else if gid <= cov.range[i].end {
                    // found
                    return i32::from(cov.range[i].start_coverage_index) + i32::from(gid)
                        - i32::from(cov.range[i].start);
                }
            }
        }
        _ => error!("Unknown coverage format"),
    }
    -1 /* not found */
}
/// `otl_gsub_read_single` (static).
fn otl_gsub_read_single(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    let offset = sfont.stream.tell() as u32;

    subtab.lookup_type = OTL_GSUB_TYPE_SINGLE;
    subtab.subst_format = sfont.sfnt_get_ushort();
    let mut len = 2;

    if subtab.subst_format == 1 {
        let mut data = Box::new(OtlGsubSingle1::default());
        let cov_offset = sfont.sfnt_get_ushort();
        data.delta_glyph_id = sfont.sfnt_get_short();
        len += 4;

        sfont.sfnt_seek_set(offset.wrapping_add(u32::from(cov_offset)));
        len += clt_read_coverage(&mut data.coverage, sfont);
        subtab.table = OtlGsubTable::Single1(data);
    } else if subtab.subst_format == 2 {
        let mut data = Box::new(OtlGsubSingle2::default());
        let cov_offset = sfont.sfnt_get_ushort();
        data.glyph_count = sfont.sfnt_get_ushort();
        len += 4;

        if data.glyph_count != 0 {
            data.substitute = Vec::with_capacity(data.glyph_count as usize);
            for _ in 0..data.glyph_count {
                data.substitute.push(sfont.sfnt_get_ushort());
            }
            len += 2 * i32::from(data.glyph_count);
        }

        sfont.sfnt_seek_set(offset.wrapping_add(u32::from(cov_offset)));
        len += clt_read_coverage(&mut data.coverage, sfont);
        subtab.table = OtlGsubTable::Single2(data);
    } else {
        error!("unexpected SubstFormat");
    }

    len
}
/// `otl_gsub_read_alternate` (static).
fn otl_gsub_read_alternate(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    let offset = sfont.stream.tell() as u32;

    subtab.lookup_type = OTL_GSUB_TYPE_ALTERNATE;
    subtab.subst_format = sfont.sfnt_get_ushort(); /* Must be 1 */
    if subtab.subst_format != 1 {
        warn!(
            "Unknown GSUB SubstFormat for Alternate: {}",
            subtab.subst_format
        );
        return -1;
    }

    let mut len = 2;
    let mut data = Box::new(OtlGsubAlternate1::default());

    let cov_offset = sfont.sfnt_get_ushort();
    len += 2;
    let mut altset_offsets = CltNumberList::default();
    len += clt_read_number_list(&mut altset_offsets, sfont);
    data.alternate_set_count = altset_offsets.count;
    data.alternate_set = Vec::with_capacity(data.alternate_set_count as usize);
    for i in 0..data.alternate_set_count as usize {
        let mut altset = OtlGsubAltset::default();
        let altset_offset = offset.wrapping_add(u32::from(altset_offsets.value[i]));
        sfont.sfnt_seek_set(altset_offset);
        altset.glyph_count = sfont.sfnt_get_ushort();
        len += 2;
        if altset.glyph_count != 0 {
            altset.alternate = Vec::with_capacity(altset.glyph_count as usize);
            for _ in 0..altset.glyph_count {
                altset.alternate.push(sfont.sfnt_get_ushort());
                len += 2;
            }
        }
        data.alternate_set.push(altset);
    }
    sfont.sfnt_seek_set(offset.wrapping_add(u32::from(cov_offset)));
    len += clt_read_coverage(&mut data.coverage, sfont);
    subtab.table = OtlGsubTable::Alternate1(data);

    len
}
/// `otl_gsub_read_ligature` (static).
fn otl_gsub_read_ligature(subtab: &mut OtlGsubSubtab, sfont: &mut Sfnt) -> i32 {
    let offset = sfont.stream.tell() as u32;

    subtab.lookup_type = OTL_GSUB_TYPE_LIGATURE;
    subtab.subst_format = sfont.sfnt_get_ushort(); /* Must be 1 */
    if subtab.subst_format != 1 {
        warn!(
            "Unknown GSUB SubstFormat for Ligature: {}",
            subtab.subst_format
        );
        return -1;
    }

    let mut len = 2;
    let mut data = Box::new(OtlGsubLigature1::default());

    let cov_offset = sfont.sfnt_get_ushort();
    len += 2;
    let mut ligset_offsets = CltNumberList::default();
    len += clt_read_number_list(&mut ligset_offsets, sfont);
    data.lig_set_count = ligset_offsets.count;
    data.ligature_set = Vec::with_capacity(data.lig_set_count as usize);
    for i in 0..data.lig_set_count as usize {
        let mut ligset = OtlGsubLigset::default();
        let ligset_offset = offset.wrapping_add(u32::from(ligset_offsets.value[i]));
        sfont.sfnt_seek_set(ligset_offset);
        let mut ligset_tab = CltNumberList::default();
        len += clt_read_number_list(&mut ligset_tab, sfont);

        ligset.ligature_count = ligset_tab.count;
        ligset.ligature = Vec::with_capacity(ligset_tab.count as usize);
        for j in 0..ligset_tab.count as usize {
            sfont.sfnt_seek_set(ligset_offset.wrapping_add(u32::from(ligset_tab.value[j])));
            let mut lig = OtlGsubLigtab {
                lig_glyph: sfont.sfnt_get_ushort(),
                comp_count: sfont.sfnt_get_ushort(),
                component: Vec::new(),
            };
            if lig.comp_count != 0 {
                let n = i32::from(lig.comp_count) - 1;
                lig.component = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    lig.component.push(sfont.sfnt_get_ushort());
                }
                len += 4 + n * 2;
            }
            ligset.ligature.push(lig);
        }
        data.ligature_set.push(ligset);
    }

    sfont.sfnt_seek_set(offset.wrapping_add(u32::from(cov_offset)));
    len += clt_read_coverage(&mut data.coverage, sfont);
    subtab.table = OtlGsubTable::Ligature1(data);

    len
}
/// `otl_gsub_read_header` (static).
fn otl_gsub_read_header(head: &mut OtlGsubHeader, sfont: &mut Sfnt) -> i32 {
    head.version = sfont.sfnt_get_ulong();
    head.script_list = sfont.sfnt_get_ushort();
    head.feature_list = sfont.sfnt_get_ushort();
    head.lookup_list = sfont.sfnt_get_ushort();
    10
}

/// `SET_BIT`.
fn set_bit(b: &mut [u8], p: usize) {
    b[p / 8] |= 1 << (7 - (p % 8));
}
/// `BIT_SET`.
fn bit_set(b: &[u8], p: usize) -> bool {
    b[p / 8] & (1 << (7 - (p % 8))) != 0
}

/// One subtable read by `otl_gsub_read_feat`: pushed when the reader
/// returned a positive length (C writes into the next slot and counts
/// it).
fn read_subtab(
    subtabs: &mut Vec<OtlGsubSubtab>,
    sfont: &mut Sfnt,
    read: fn(&mut OtlGsubSubtab, &mut Sfnt) -> i32,
) {
    let mut st = OtlGsubSubtab::default();
    let r = read(&mut st, sfont);
    if r <= 0 {
        warn!("Reading GSUB subtable failed...");
    } else {
        subtabs.push(st);
    }
}

/// `otl_gsub_read_feat` (static): the subtables of `gsub`'s
/// script/language/feature; 0, or -1 if none.
fn otl_gsub_read_feat(gsub: &mut OtlGsubTab, sfont: &mut Sfnt) -> i32 {
    let gsub_offset = sfont.sfnt_find_table_pos(b"GSUB");
    if gsub_offset == 0 {
        return -1; /* not found */
    }

    let mut script = OtlOpt::otl_new_opt();
    script.otl_parse_optstring(Some(&gsub.script));
    let mut language = OtlOpt::otl_new_opt();
    language.otl_parse_optstring(Some(&gsub.language));
    let mut feature = OtlOpt::otl_new_opt();
    feature.otl_parse_optstring(Some(&gsub.feature));

    let mut feat_bits = vec![0u8; 8192];

    // GSUB header.
    let mut head = OtlGsubHeader::default();
    sfont.sfnt_seek_set(gsub_offset);
    otl_gsub_read_header(&mut head, sfont);

    // Script.
    let mut offset = gsub_offset.wrapping_add(u32::from(head.script_list));
    sfont.sfnt_seek_set(offset);
    let mut script_list = CltRecordList::default();
    clt_read_record_list(&mut script_list, sfont);

    for script_idx in 0..script_list.count as usize {
        if otl_match_optrule(Some(&script), &script_list.record[script_idx].tag) != 0 {
            let mut script_tab = CltScriptTable::default();

            offset = gsub_offset
                .wrapping_add(u32::from(head.script_list))
                .wrapping_add(u32::from(script_list.record[script_idx].offset));
            sfont.sfnt_seek_set(offset);
            clt_read_script_table(&mut script_tab, sfont);

            if otl_match_optrule(Some(&language), b"dflt") != 0 && script_tab.default_lang_sys != 0
            {
                let mut langsys_tab = CltLangsysTable::default();
                sfont.sfnt_seek_set(offset.wrapping_add(u32::from(script_tab.default_lang_sys)));
                clt_read_langsys_table(&mut langsys_tab, sfont);
                if otl_match_optrule(Some(&feature), b"____") != 0 /* _FIXME_ */
                    && langsys_tab.req_feature_index != 0xFFFF
                {
                    set_bit(&mut feat_bits, langsys_tab.req_feature_index as usize);
                }
                for feat_idx in 0..langsys_tab.feature_index.count as usize {
                    set_bit(
                        &mut feat_bits,
                        langsys_tab.feature_index.value[feat_idx] as usize,
                    );
                }
            }
            for langsys_idx in 0..script_tab.lang_sys_record.count as usize {
                let langsys_rec = &script_tab.lang_sys_record.record[langsys_idx];
                if otl_match_optrule(Some(&language), &langsys_rec.tag) != 0 {
                    let mut langsys_tab = CltLangsysTable::default();
                    sfont.sfnt_seek_set(offset.wrapping_add(u32::from(langsys_rec.offset)));
                    clt_read_langsys_table(&mut langsys_tab, sfont);
                    if otl_match_optrule(Some(&feature), b"____") != 0 /* _FIXME_ */
                        || langsys_tab.req_feature_index != 0xFFFF
                    {
                        set_bit(&mut feat_bits, langsys_tab.req_feature_index as usize);
                    }
                    for feat_idx in 0..langsys_tab.feature_index.count as usize {
                        set_bit(
                            &mut feat_bits,
                            langsys_tab.feature_index.value[feat_idx] as usize,
                        );
                    }
                }
            }
        }
    }

    // Feature List.
    offset = gsub_offset.wrapping_add(u32::from(head.feature_list));
    sfont.sfnt_seek_set(offset);
    let mut feature_list = CltRecordList::default();
    clt_read_record_list(&mut feature_list, sfont);

    // Lookup List.
    offset = gsub_offset.wrapping_add(u32::from(head.lookup_list));
    sfont.sfnt_seek_set(offset);
    let mut lookup_list = CltNumberList::default();
    clt_read_number_list(&mut lookup_list, sfont);

    let mut subtab: Vec<OtlGsubSubtab> = Vec::new();
    // Whether C's `subtab` is non-NULL: RENEW to 0 elements frees it.
    let mut subtab_allocated = false;

    for feat_idx in 0..feature_list.count as usize {
        if bit_set(&feat_bits, feat_idx)
            && otl_match_optrule(Some(&feature), &feature_list.record[feat_idx].tag) != 0
        {
            let mut feature_table = CltFeatureTable::default();

            // Feature Table.
            offset = gsub_offset
                .wrapping_add(u32::from(head.feature_list))
                .wrapping_add(u32::from(feature_list.record[feat_idx].offset));
            sfont.sfnt_seek_set(offset);
            clt_read_feature_table(&mut feature_table, sfont);

            // Lookup table.
            for i in 0..feature_table.lookup_list_index.count as usize {
                let mut lookup_table = CltLookupTable::default();

                let ll_idx = feature_table.lookup_list_index.value[i];
                if ll_idx >= lookup_list.count {
                    error!("invalid Lookup index.");
                }

                offset = gsub_offset
                    .wrapping_add(u32::from(head.lookup_list))
                    .wrapping_add(u32::from(lookup_list.value[ll_idx as usize]));
                sfont.sfnt_seek_set(offset);
                clt_read_lookup_table(&mut lookup_table, sfont);

                if lookup_table.lookup_type != OTL_GSUB_TYPE_SINGLE
                    && lookup_table.lookup_type != OTL_GSUB_TYPE_ALTERNATE
                    && lookup_table.lookup_type != OTL_GSUB_TYPE_LIGATURE
                    && lookup_table.lookup_type != OTL_GSUB_TYPE_ESUBST
                {
                    continue;
                }

                subtab_allocated = subtab.len() + lookup_table.sub_table_list.count as usize > 0;
                for st_idx in 0..lookup_table.sub_table_list.count as usize {
                    offset = gsub_offset
                        .wrapping_add(u32::from(head.lookup_list))
                        .wrapping_add(u32::from(lookup_list.value[ll_idx as usize]))
                        .wrapping_add(u32::from(lookup_table.sub_table_list.value[st_idx]));

                    sfont.sfnt_seek_set(offset);

                    match lookup_table.lookup_type {
                        OTL_GSUB_TYPE_SINGLE => {
                            read_subtab(&mut subtab, sfont, otl_gsub_read_single);
                        }
                        OTL_GSUB_TYPE_ALTERNATE => {
                            read_subtab(&mut subtab, sfont, otl_gsub_read_alternate);
                        }
                        OTL_GSUB_TYPE_LIGATURE => {
                            read_subtab(&mut subtab, sfont, otl_gsub_read_ligature);
                        }
                        OTL_GSUB_TYPE_ESUBST => {
                            let subst_format = sfont.sfnt_get_ushort();
                            if subst_format != 1 {
                                continue;
                            }
                            let extension_lookup_type = sfont.sfnt_get_ushort();
                            let extension_offset = sfont.sfnt_get_ulong();

                            sfont.sfnt_seek_set(offset.wrapping_add(extension_offset));
                            match extension_lookup_type {
                                OTL_GSUB_TYPE_SINGLE => {
                                    read_subtab(&mut subtab, sfont, otl_gsub_read_single);
                                }
                                OTL_GSUB_TYPE_ALTERNATE => {
                                    read_subtab(&mut subtab, sfont, otl_gsub_read_alternate);
                                }
                                OTL_GSUB_TYPE_LIGATURE => {
                                    read_subtab(&mut subtab, sfont, otl_gsub_read_ligature);
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if subtab_allocated {
        gsub.num_subtables = subtab.len() as i32;
        gsub.subtables = subtab;
    } else {
        return -1;
    }

    0
}
/// `otl_gsub_apply_single` (static): 0 if `gid` was substituted, else -1.
fn otl_gsub_apply_single(subtab: &OtlGsubSubtab, gid: &mut USHORT) -> i32 {
    match (&subtab.table, subtab.subst_format) {
        (OtlGsubTable::Single1(data), 1) => {
            let idx = clt_lookup_coverage(&data.coverage, *gid);
            if idx >= 0 {
                *gid = gid.wrapping_add(data.delta_glyph_id as USHORT);
                return 0; /* found */
            }
        }
        (OtlGsubTable::Single2(data), 2) => {
            let idx = clt_lookup_coverage(&data.coverage, *gid);
            if idx >= 0 && idx < i32::from(data.glyph_count) {
                *gid = data.substitute[idx as usize];
                return 0; /* found */
            }
        }
        _ => {}
    }
    -1
}
/// `otl_gsub_apply_alternate` (static).
fn otl_gsub_apply_alternate(subtab: &OtlGsubSubtab, alt_idx: USHORT, gid: &mut USHORT) -> i32 {
    if subtab.subst_format == 1 {
        if let OtlGsubTable::Alternate1(data) = &subtab.table {
            let idx = clt_lookup_coverage(&data.coverage, *gid);
            if idx < 0 || idx >= i32::from(data.alternate_set_count) {
                return -1;
            }
            let altset = &data.alternate_set[idx as usize];
            if alt_idx >= altset.glyph_count {
                return -1;
            }
            *gid = altset.alternate[alt_idx as usize];
            return 0;
        }
    }
    -1
}
/// `glyph_seq_cmp` (static): 0 if the sequences are equal.
fn glyph_seq_cmp(glyph_seq0: &[GlyphID], glyph_seq1: &[GlyphID]) -> i32 {
    let (n0, n1) = (glyph_seq0.len(), glyph_seq1.len());
    if n0 != n1 {
        return n0 as i32 - n1 as i32;
    }
    for i in 0..n0 {
        if glyph_seq0[i] != glyph_seq1[i] {
            return i32::from(glyph_seq0[i]) - i32::from(glyph_seq1[i]);
        }
    }
    0
}
/// `otl_gsub_apply_ligature` (static): `gid_out` untouched on failure.
fn otl_gsub_apply_ligature(subtab: &OtlGsubSubtab, gid_in: &[USHORT], gid_out: &mut USHORT) -> i32 {
    if gid_in.is_empty() {
        return -1;
    }
    if subtab.subst_format == 1 {
        if let OtlGsubTable::Ligature1(data) = &subtab.table {
            let idx = clt_lookup_coverage(&data.coverage, gid_in[0]);
            if idx >= 0 && idx < i32::from(data.lig_set_count) {
                let ligset = &data.ligature_set[idx as usize];
                for j in 0..ligset.ligature_count as usize {
                    let lig = &ligset.ligature[j];
                    // C: CompCount - 1 as a USHORT (65535 for 0).
                    let n1 = lig.comp_count.wrapping_sub(1) as usize;
                    let equal = if n1 != gid_in.len() - 1 {
                        false
                    } else {
                        glyph_seq_cmp(&gid_in[1..], &lig.component[..n1]) == 0
                    };
                    if equal {
                        *gid_out = lig.lig_glyph;
                        return 0; /* found */
                    }
                }
            }
        }
    }
    -1
}
/// `scan_otl_tag` (static): status (0 or -1), script, language, feature
/// (C strings: 4 bytes, space padded, but `*` alone for a bare feature;
/// the feature is all of what is left) of `scrp.lang.feat` (or `feat`)
/// at the start of `otl_tags` (which ends at C's `endptr`).
fn scan_otl_tag(otl_tags: Option<&[u8]>) -> (i32, Vec<u8>, Vec<u8>, Vec<u8>) {
    let fail = (-1, Vec::new(), Vec::new(), Vec::new());
    let Some(s) = otl_tags else {
        return fail;
    };
    if s.is_empty() {
        return fail;
    }
    let endptr = s.len();

    let mut script = vec![b' '; 4];
    let mut language = vec![b' '; 4];
    let feature;

    // First parse otl_tags variable.
    let mut p = 0usize;
    let period = s[p..].iter().position(|&c| c == b'.').map(|n| p + n);
    if let Some(period) = period {
        // Format scrp.lang.feat
        if period < p + 5 {
            script[..period - p].copy_from_slice(&s[p..period]);
        } else {
            warn!("Invalid OTL script tag found");
            return fail;
        }
        p = period + 1;
        let period = s[p..].iter().position(|&c| c == b'.').map(|n| p + n);
        if let Some(period) = period {
            // Now lang part.
            if period < p + 5 {
                language[..period - p].copy_from_slice(&s[p..period]);
            } else {
                warn!("Invalid OTL lanuage tag found");
                return fail;
            }
            p = period + 1;
        }
    } else {
        script = b"*".to_vec();
        language = b"*".to_vec();
    }

    // Finally feature.
    if p + 4 <= endptr {
        feature = c_str(&s[p..endptr]).to_vec();
    } else {
        warn!("No valid OTL feature tag specified.");
        return fail;
    }

    (0, script, language, feature)
}

impl OtlGsub {
    /// `otl_gsub_new`.
    #[must_use]
    pub fn otl_gsub_new() -> OtlGsub {
        OtlGsub {
            num_gsubs: 0,
            select: -1,
            first: Vec::new(),
            gsubs: Vec::new(),
        }
    }
    /// `otl_gsub_release`.
    pub fn otl_gsub_release(self) {}
    /// `clear_chain` (static).
    fn clear_chain(&mut self) {
        self.first.clear();
    }
    /// `otl_gsub_add_feat`: 0, or -1 if absent/already there.
    pub fn otl_gsub_add_feat(
        &mut self,
        script: &[u8],
        language: &[u8],
        feature: &[u8],
        sfont: &mut Sfnt,
    ) -> i32 {
        let (script, language, feature) = (c_str(script), c_str(language), c_str(feature));
        if self.num_gsubs > GSUB_LIST_MAX as i32 {
            error!("Too many GSUB features...");
        }
        let mut i = 0;
        while i < self.num_gsubs {
            let gsub = &self.gsubs[i as usize];
            if script == &gsub.script[..]
                && language == &gsub.language[..]
                && feature == &gsub.feature[..]
            {
                self.select = i;
                return 0;
            }
            i += 1;
        }

        let mut gsub = OtlGsubTab {
            script: script.to_vec(),
            language: language.to_vec(),
            feature: feature.to_vec(),
            num_subtables: 0,
            subtables: Vec::new(),
        };

        let retval = otl_gsub_read_feat(&mut gsub, sfont);
        if retval >= 0 {
            self.select = i;
            self.gsubs.push(gsub);
            self.num_gsubs += 1;
        }

        retval
    }
    /// `gsub_find` (static): the index in `gsubs`, or -1.
    fn gsub_find(&self, script: &[u8], language: &[u8], feature: &[u8]) -> i32 {
        let (script, language, feature) = (c_str(script), c_str(language), c_str(feature));
        for i in 0..self.num_gsubs as usize {
            let gsub = &self.gsubs[i];
            if &gsub.script[..] == script
                && &gsub.language[..] == language
                && &gsub.feature[..] == feature
            {
                return i as i32;
            }
        }
        -1
    }
    /// `otl_gsub_select`: the index selected, or -1.
    pub fn otl_gsub_select(&mut self, script: &[u8], language: &[u8], feature: &[u8]) -> i32 {
        self.select = self.gsub_find(script, language, feature);
        self.select
    }
    /// The selected GSUB (ERROR if none).
    fn selected(&self) -> &OtlGsubTab {
        let i = self.select;
        if i < 0 || i >= self.num_gsubs {
            error!("GSUB not selected...");
        }
        &self.gsubs[i as usize]
    }
    /// `otl_gsub_apply`: in/out `gid`; 0 if substituted, else -1.
    pub fn otl_gsub_apply(&self, gid: &mut USHORT) -> i32 {
        let mut retval = -1;
        let gsub = self.selected();
        let mut j = 0;
        while retval < 0 && j < gsub.num_subtables as usize {
            let subtab = &gsub.subtables[j];
            if subtab.lookup_type == OTL_GSUB_TYPE_SINGLE {
                retval = otl_gsub_apply_single(subtab, gid);
            }
            j += 1;
        }
        retval
    }
    /// `otl_gsub_apply_alt`: in/out `gid`.
    pub fn otl_gsub_apply_alt(&self, alt_idx: USHORT, gid: &mut USHORT) -> i32 {
        let mut retval = -1;
        let gsub = self.selected();
        let mut j = 0;
        while retval < 0 && j < gsub.num_subtables as usize {
            let subtab = &gsub.subtables[j];
            if subtab.lookup_type == OTL_GSUB_TYPE_ALTERNATE {
                retval = otl_gsub_apply_alternate(subtab, alt_idx, gid);
            }
            j += 1;
        }
        retval
    }
    /// `otl_gsub_apply_lig`: `num_gids` is `gid_in.len()`; `gid_out`
    /// written on success only.
    pub fn otl_gsub_apply_lig(&self, gid_in: &[USHORT], gid_out: &mut USHORT) -> i32 {
        let mut retval = -1;
        let gsub = self.selected();
        let mut j = 0;
        while retval < 0 && j < gsub.num_subtables as usize {
            let subtab = &gsub.subtables[j];
            if subtab.lookup_type == OTL_GSUB_TYPE_LIGATURE {
                retval = otl_gsub_apply_ligature(subtab, gid_in, gid_out);
            }
            j += 1;
        }
        retval
    }
    /// `otl_gsub_add_feat_list`: the features of `otl_tags` (`:`-separated).
    pub fn otl_gsub_add_feat_list(&mut self, otl_tags: &[u8], sfont: &mut Sfnt) -> i32 {
        let otl_tags = c_str(otl_tags);
        self.clear_chain();
        let endptr = otl_tags.len();
        let mut p = 0usize;
        while p < endptr {
            let nextptr = otl_tags[p..]
                .iter()
                .position(|&c| c == b':')
                .map_or(endptr, |n| p + n);
            let (r, script, language, feature) = scan_otl_tag(Some(&otl_tags[p..nextptr]));
            if r >= 0 {
                let idx = self.gsub_find(&script, &language, &feature);
                if idx < 0 {
                    self.otl_gsub_add_feat(&script, &language, &feature, sfont);
                }
            }
            p = nextptr + 1;
        }
        0
    }
    /// `otl_gsub_set_chain`.
    pub fn otl_gsub_set_chain(&mut self, otl_tags: &[u8]) -> i32 {
        let otl_tags = c_str(otl_tags);
        self.clear_chain();
        let endptr = otl_tags.len();
        let mut p = 0usize;
        while p < endptr {
            let nextptr = otl_tags[p..]
                .iter()
                .position(|&c| c == b':')
                .map_or(endptr, |n| p + n);
            let (r, script, language, feature) = scan_otl_tag(Some(&otl_tags[p..nextptr]));
            if r >= 0 {
                let idx = self.gsub_find(&script, &language, &feature);
                if idx >= 0 && idx <= self.num_gsubs {
                    self.first.push(idx);
                }
            }
            p = nextptr + 1;
        }
        0
    }
    /// `otl_gsub_apply_chain`: in/out `gid`.
    pub fn otl_gsub_apply_chain(&self, gid: &mut USHORT) -> i32 {
        let mut retval = -1;
        for &idx in &self.first {
            if idx < 0 || idx >= self.num_gsubs {
                continue;
            }
            let gsub = &self.gsubs[idx as usize];
            retval = -1;
            let mut i = 0;
            while retval < 0 && i < gsub.num_subtables as usize {
                let subtab = &gsub.subtables[i];
                if subtab.lookup_type == OTL_GSUB_TYPE_SINGLE {
                    retval = otl_gsub_apply_single(subtab, gid);
                }
                i += 1;
            }
        }
        retval
    }
}

/// `UC_UTF16BE_encode_char` of `ch` into `dst`: the bytes written.
fn encode_utf16be(ch: i32, dst: &mut [u8], p: &mut usize) -> usize {
    UC_UTF16BE_encode_char(ch, dst, p)
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
    let mut count = 0;
    let mut dst = [0u8; 4];
    let mut p = 0usize;

    if gid_sub >= num_glyphs || gid >= num_glyphs {
        return 0;
    }

    let cid_sub = gid_to_cid_map[gid_sub as usize];
    if is_used_char2(used_chars, u32::from(cid_sub)) {
        let mut ch = map_base[gid as usize];
        if UC_is_valid(ch) {
            let src = [(cid_sub >> 8) as u8, (cid_sub & 0xff) as u8];
            let len = encode_utf16be(ch, &mut dst, &mut p);
            cmap.CMap_add_bfchar(&src, &dst[..len]);
            used_chars[(cid_sub / 8) as usize] &= !(1 << (7 - (cid_sub % 8)));
            count = 1;
        } else {
            ch = map_sub[gid as usize];
            if UC_is_valid(ch) {
                let src = [(cid_sub >> 8) as u8, (cid_sub & 0xff) as u8];
                let len = encode_utf16be(ch, &mut dst, &mut p);
                cmap.CMap_add_bfchar(&src, &dst[..len]);
                used_chars[(cid_sub / 8) as usize] &= !(1 << (7 - (cid_sub % 8)));
                count = 1;
            }
        }
    }
    count
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
    let mut count = 0;

    match (&subtab.table, subtab.subst_format) {
        (OtlGsubTable::Single1(data), 1) => {
            let cov = &data.coverage;
            match cov.format {
                1 => {
                    // list
                    for idx in 0..cov.count as usize {
                        let gid = cov.list[idx];
                        let gid_sub = gid.wrapping_add(data.delta_glyph_id as USHORT);
                        count += add_glyph_if_valid(
                            cmap,
                            used_chars,
                            map_base,
                            map_sub,
                            num_glyphs,
                            gid_to_cid_map,
                            gid,
                            gid_sub,
                        );
                    }
                }
                2 => {
                    // range
                    for i in 0..cov.count as usize {
                        let mut gid = cov.range[i].start;
                        while gid <= cov.range[i].end && gid < num_glyphs {
                            let gid_sub = gid.wrapping_add(data.delta_glyph_id as USHORT);
                            count += add_glyph_if_valid(
                                cmap,
                                used_chars,
                                map_base,
                                map_sub,
                                num_glyphs,
                                gid_to_cid_map,
                                gid,
                                gid_sub,
                            );
                            gid = gid.wrapping_add(1);
                        }
                    }
                }
                _ => {}
            }
        }
        (OtlGsubTable::Single2(data), 2) => {
            let cov = &data.coverage;
            match cov.format {
                1 => {
                    // list
                    for idx in 0..cov.count {
                        let gid = cov.list[idx as usize];
                        if idx < data.glyph_count {
                            let gid_sub = data.substitute[idx as usize];
                            count += add_glyph_if_valid(
                                cmap,
                                used_chars,
                                map_base,
                                map_sub,
                                num_glyphs,
                                gid_to_cid_map,
                                gid,
                                gid_sub,
                            );
                        }
                    }
                }
                2 => {
                    // range
                    for i in 0..cov.count as usize {
                        let mut gid = cov.range[i].start;
                        while gid <= cov.range[i].end && gid < num_glyphs {
                            let idx = cov.range[i]
                                .start_coverage_index
                                .wrapping_add(gid)
                                .wrapping_sub(cov.range[i].start);
                            if idx < data.glyph_count {
                                let gid_sub = data.substitute[idx as usize];
                                count += add_glyph_if_valid(
                                    cmap,
                                    used_chars,
                                    map_base,
                                    map_sub,
                                    num_glyphs,
                                    gid_to_cid_map,
                                    gid,
                                    gid_sub,
                                );
                            }
                            gid = gid.wrapping_add(1);
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

    count
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
    let mut count = 0;
    if idx >= 0 && idx < i32::from(data.alternate_set_count) {
        let altset = &data.alternate_set[idx as usize];
        if altset.glyph_count == 0 {
            return count;
        }
        for i in 0..altset.glyph_count as usize {
            let gid_alt = altset.alternate[i];
            count += add_glyph_if_valid(
                cmap,
                used_chars,
                map_base,
                map_sub,
                num_glyphs,
                gid_to_cid_map,
                gid,
                gid_alt,
            );
        }
    }
    count
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
    let mut count = 0;

    if subtab.subst_format == 1 {
        if let OtlGsubTable::Alternate1(data) = &subtab.table {
            let cov = &data.coverage;
            match cov.format {
                1 => {
                    // list
                    for idx in 0..cov.count {
                        let gid = cov.list[idx as usize];
                        if gid < num_glyphs {
                            count += add_alternate1_inverse_map(
                                cmap,
                                used_chars,
                                map_base,
                                map_sub,
                                num_glyphs,
                                gid_to_cid_map,
                                gid,
                                i32::from(idx),
                                data,
                            );
                        }
                    }
                }
                2 => {
                    // range
                    for i in 0..cov.count as usize {
                        let mut gid = cov.range[i].start;
                        while gid <= cov.range[i].end && gid < num_glyphs {
                            let idx = cov.range[i]
                                .start_coverage_index
                                .wrapping_add(gid)
                                .wrapping_sub(cov.range[i].start);
                            count += add_alternate1_inverse_map(
                                cmap,
                                used_chars,
                                map_base,
                                map_sub,
                                num_glyphs,
                                gid_to_cid_map,
                                gid,
                                i32::from(idx),
                                data,
                            );
                            gid = gid.wrapping_add(1);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    count
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
    let mut count = 0;

    if idx >= 0 && idx < i32::from(data.lig_set_count) {
        let ligset = &data.ligature_set[idx as usize];
        for j in 0..ligset.ligature_count as usize {
            let lig = &ligset.ligature[j];
            let gid_sub = lig.lig_glyph;
            if gid_sub < num_glyphs {
                let cid = gid_to_cid_map[gid_sub as usize];
                if is_used_char2(used_chars, u32::from(cid)) {
                    let comp_count = lig.comp_count;
                    let mut fail_count = 0;

                    // C allocates comp_count entries and writes the first
                    // one even for 0.
                    let mut ucv = vec![0i32; (comp_count as usize).max(1)];
                    let mut ch = if UC_is_valid(map_base[gid_1 as usize]) {
                        map_base[gid_1 as usize]
                    } else {
                        map_sub[gid_1 as usize]
                    };
                    ucv[0] = ch;
                    fail_count += if UC_is_valid(ch) { 0 } else { 1 };
                    let n = i32::from(lig.comp_count) - 1;
                    for i in 0..n.max(0) as usize {
                        let gid = lig.component[i];
                        if gid < num_glyphs {
                            ch = if UC_is_valid(map_base[gid as usize]) {
                                map_base[gid as usize]
                            } else {
                                map_sub[gid as usize]
                            };
                            ucv[i + 1] = ch;
                            fail_count += if UC_is_valid(ch) { 0 } else { 1 };
                        } else {
                            fail_count += 1;
                        }
                    }
                    if fail_count == 0 {
                        let src = [(cid >> 8) as u8, (cid & 0xff) as u8];
                        let mut dst = vec![0u8; comp_count as usize * 4];
                        let mut p = 0usize;
                        let mut len = 0usize;
                        for i in 0..comp_count as usize {
                            len += encode_utf16be(ucv[i], &mut dst, &mut p);
                        }
                        cmap.CMap_add_bfchar(&src, &dst[..len]);
                        used_chars[(cid / 8) as usize] &= !(1 << (7 - (cid % 8)));
                        count += 1;
                    }
                }
            }
        }
    }

    count
}

/// `add_ToUnicode_ligature` (static): C returns 0 whatever it added.
fn add_ToUnicode_ligature(
    cmap: &mut CMap,
    used_chars: &mut [u8],
    subtab: &OtlGsubSubtab,
    map_base: &[i32],
    map_sub: &[i32],
    num_glyphs: USHORT,
    gid_to_cid_map: &[u16],
) -> i32 {
    let mut count = 0;

    if subtab.subst_format == 1 {
        if let OtlGsubTable::Ligature1(data) = &subtab.table {
            let cov = &data.coverage;
            match cov.format {
                1 => {
                    // list
                    for idx in 0..cov.count {
                        let gid = cov.list[idx as usize];
                        if gid < num_glyphs {
                            count += add_ligature1_inverse_map(
                                cmap,
                                used_chars,
                                map_base,
                                map_sub,
                                num_glyphs,
                                gid_to_cid_map,
                                gid,
                                i32::from(idx),
                                data,
                            );
                        }
                    }
                }
                2 => {
                    // range
                    for i in 0..cov.count as usize {
                        let mut gid = cov.range[i].start;
                        while gid <= cov.range[i].end && gid < num_glyphs {
                            let idx = cov.range[i]
                                .start_coverage_index
                                .wrapping_add(gid)
                                .wrapping_sub(cov.range[i].start);
                            if gid < num_glyphs {
                                count += add_ligature1_inverse_map(
                                    cmap,
                                    used_chars,
                                    map_base,
                                    map_sub,
                                    num_glyphs,
                                    gid_to_cid_map,
                                    gid,
                                    i32::from(idx),
                                    data,
                                );
                            }
                            gid = gid.wrapping_add(1);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let _ = count;

    0
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
    let mut count = 0;

    let mut gsub_list = OtlGsub::otl_gsub_new();
    gsub_list.otl_gsub_add_feat(b"*", b"*", b"*", sfont);

    for i in 0..gsub_list.num_gsubs as usize {
        let gsub = &gsub_list.gsubs[i];
        for j in 0..gsub.num_subtables as usize {
            let subtab = &gsub.subtables[j];
            match subtab.lookup_type {
                OTL_GSUB_TYPE_SINGLE => {
                    count += add_ToUnicode_single(
                        cmap,
                        used_chars,
                        subtab,
                        map_base,
                        map_sub,
                        num_glyphs,
                        gid_to_cid_map,
                    );
                }
                OTL_GSUB_TYPE_ALTERNATE => {
                    count += add_ToUnicode_alternate(
                        cmap,
                        used_chars,
                        subtab,
                        map_base,
                        map_sub,
                        num_glyphs,
                        gid_to_cid_map,
                    );
                }
                OTL_GSUB_TYPE_LIGATURE => {
                    count += add_ToUnicode_ligature(
                        cmap,
                        used_chars,
                        subtab,
                        map_base,
                        map_sub,
                        num_glyphs,
                        gid_to_cid_map,
                    );
                }
                _ => {}
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_tags() {
        let (r, s, l, f) = scan_otl_tag(Some(b"liga"));
        assert_eq!(
            (r, &s[..], &l[..], &f[..]),
            (0, &b"*"[..], &b"*"[..], &b"liga"[..])
        );
        let (r, s, l, f) = scan_otl_tag(Some(b"latn.liga"));
        assert_eq!(
            (r, &s[..], &l[..], &f[..]),
            (0, &b"latn"[..], &b"    "[..], &b"liga"[..])
        );
        let (r, s, l, f) = scan_otl_tag(Some(b"kana.JAN.vert"));
        assert_eq!(
            (r, &s[..], &l[..], &f[..]),
            (0, &b"kana"[..], &b"JAN "[..], &b"vert"[..])
        );
        assert_eq!(scan_otl_tag(Some(b"lig")).0, -1);
        assert_eq!(scan_otl_tag(Some(b"latnx.liga")).0, -1);
        assert_eq!(scan_otl_tag(Some(b"")).0, -1);
    }

    #[test]
    fn coverage() {
        let cov = CltCoverage {
            format: 2,
            count: 2,
            list: Vec::new(),
            range: vec![
                CltRange {
                    start: 10,
                    end: 12,
                    start_coverage_index: 0,
                },
                CltRange {
                    start: 20,
                    end: 20,
                    start_coverage_index: 3,
                },
            ],
        };
        assert_eq!(clt_lookup_coverage(&cov, 11), 1);
        assert_eq!(clt_lookup_coverage(&cov, 20), 3);
        assert_eq!(clt_lookup_coverage(&cov, 15), -1);
        assert_eq!(clt_lookup_coverage(&cov, 5), -1);
    }
}

/// cmap, post and GSUB lookups of TeX Live fonts against xdvipdfmx's own
/// code (a C harness over tt_cmap.c, tt_gsub.c, otl_opt.c, tt_post.c):
/// the same dump; `PARTEX_XPDF_DUMP=dir` writes it.
#[cfg(test)]
mod font_tests {
    extern crate std;

    use super::*;
    use crate::stream::MemFile;
    use alloc::string::String;
    use alloc::sync::Arc;
    use core::fmt::Write as _;

    fn dump_gsub(
        out: &mut String,
        sfont: &mut Sfnt,
        n: u16,
        s: &[u8],
        l: &[u8],
        f: &[u8],
        set: &[u16],
    ) {
        let mut g = OtlGsub::otl_gsub_new();
        let r = g.otl_gsub_add_feat(s, l, f, sfont);
        let st = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let _ = writeln!(out, "feat {}.{}.{} {}", st(s), st(l), st(f), r);
        if r >= 0 {
            for gid in 0..n {
                let mut g2 = gid;
                if g.otl_gsub_apply(&mut g2) == 0 {
                    let _ = writeln!(out, "s {gid} {g2}");
                }
                for k in 0..3u16 {
                    let mut g2 = gid;
                    if g.otl_gsub_apply_alt(k, &mut g2) == 0 {
                        let _ = writeln!(out, "a {gid} {k} {g2}");
                    }
                }
            }
            for &a in set {
                for &b in set {
                    let mut g2 = 0;
                    if g.otl_gsub_apply_lig(&[a, b], &mut g2) == 0 {
                        let _ = writeln!(out, "l {a} {b} {g2}");
                    }
                    for &c in set {
                        let mut g2 = 0;
                        if g.otl_gsub_apply_lig(&[a, b, c], &mut g2) == 0 {
                            let _ = writeln!(out, "l {a} {b} {c} {g2}");
                        }
                    }
                }
            }
            let mut g2 = 0;
            if g.otl_gsub_apply_lig(&set[..1], &mut g2) == 0 {
                let _ = writeln!(out, "l1 {} {g2}", set[0]);
            }
        }
    }

    /// What the harness prints for `path` and the `(script, language,
    /// feature)` triples.
    fn dump(path: &str, feats: &[(&[u8], &[u8], &[u8])]) -> Option<String> {
        let data = std::fs::read(path).ok()?;
        let mut sfont = Sfnt::sfnt_open(MemFile::new(Arc::from(data), path.as_bytes()))?;
        let mut out = String::new();
        let _ = writeln!(out, "type {}", sfont.type_);
        sfont.sfnt_read_table_directory(0);
        let n = sfont.tt_read_maxp_table().num_glyphs;
        let _ = writeln!(out, "numGlyphs {n}");
        {
            let h = sfont.tt_read_head_table();
            let hh = sfont.tt_read_hhea_table();
            let o = sfont.tt_read_os2__table();
            let nm = sfont.tt_get_ps_fontname(255);
            let _ = writeln!(out, "psname {} {}", nm.len(), String::from_utf8_lossy(&nm));
            let nm = sfont.tt_get_ps_fontname(8);
            let _ = writeln!(out, "psname8 {} {}", nm.len(), String::from_utf8_lossy(&nm));
            let _ = writeln!(
                out,
                "head {} {} {} {} {} {} {} {}",
                h.units_per_em,
                h.mac_style,
                h.x_min,
                h.y_min,
                h.x_max,
                h.y_max,
                h.index_to_loc_format,
                h.check_sum_adjustment
            );
            let _ = writeln!(
                out,
                "hhea {} {} {} {} {} {}",
                hh.ascent,
                hh.descent,
                hh.line_gap,
                hh.advance_width_max,
                hh.num_of_long_hor_metrics,
                hh.num_of_ex_side_bearings
            );
            let _ = writeln!(
                out,
                "os2 {} {} {} {} {} {} {} {} {} {} {}",
                o.version,
                o.x_avg_char_width,
                o.us_weight_class,
                o.fs_type,
                o.s_family_class,
                o.s_typo_ascender,
                o.s_typo_descender,
                o.s_typo_line_gap,
                o.fs_selection,
                o.sx_height,
                o.s_cap_height
            );
            sfont.sfnt_locate_table(b"hmtx");
            let m = sfont.tt_read_longMetrics(
                n,
                hh.num_of_long_hor_metrics,
                hh.num_of_ex_side_bearings,
            );
            let mut sum: u64 = 0;
            for x in &m {
                sum = sum
                    .wrapping_mul(31)
                    .wrapping_add(u64::from(x.advance) * 7 + u64::from(x.side_bearing as u16));
            }
            let _ = writeln!(out, "hmtx {}", sum & 0xffff_ffff);
            if sfont.sfnt_find_table_pos(b"vmtx") > 0 {
                let v = sfont.tt_read_vhea_table();
                let _ = writeln!(
                    out,
                    "vhea {} {}",
                    v.num_of_long_ver_metrics, v.num_of_ex_side_bearings
                );
            }
            if let Some(vo) = sfont.tt_read_VORG_table() {
                let _ = writeln!(
                    out,
                    "vorg {} {}",
                    vo.default_vert_origin_y, vo.num_vert_origin_y_metrics
                );
            }
        }
        let mut uni: Option<crate::tt_cmap::TtCmap> = None;
        for (p, e) in [(3, 10), (3, 1), (0, 3), (0, 4), (1, 0), (3, 0), (0, 5)] {
            let Some(c) = sfont.tt_cmap_read(p, e) else {
                let _ = writeln!(out, "cmap {p} {e} none");
                continue;
            };
            let _ = writeln!(
                out,
                "cmap {p} {e} format {} language {}",
                c.format, c.language
            );
            let max: u32 = if c.format >= 12 { 0x10FFFF } else { 0xFFFF };
            if c.format != 14 {
                for cc in 0..=max {
                    let g = c.tt_cmap_lookup(cc);
                    if g != 0 {
                        let _ = writeln!(out, "{cc:x} {g}");
                    }
                }
            }
            if uni.is_none() && (c.format == 4 || c.format == 12) {
                uni = Some(c);
            }
        }
        let mut set: Vec<u16> = Vec::new();
        for &ch in b"filtaeocsTh0123" {
            let g = uni.as_ref().map_or(0, |u| u.tt_cmap_lookup(u32::from(ch)));
            if g != 0 {
                set.push(g);
            }
        }
        for i in 1..20u16 {
            if i < n {
                set.push(i);
            }
        }
        if let Some(post) = sfont.tt_read_post_table() {
            let _ = writeln!(out, "post {:08x} {}", post.version, post.number_of_glyphs);
            for gid in 0..n.min(600) {
                if let Some(nm) = post.tt_get_glyphname(gid) {
                    let _ = writeln!(
                        out,
                        "{gid} {} {}",
                        String::from_utf8_lossy(&nm),
                        post.tt_lookup_post_table(&nm)
                    );
                }
            }
        }
        for (s, l, f) in feats {
            dump_gsub(&mut out, &mut sfont, n, s, l, f, &set);
        }
        let tags = b"liga:latn.dflt.smcp:onum:kern:DFLT.dflt.c2sc:x";
        let mut g = OtlGsub::otl_gsub_new();
        let r = g.otl_gsub_add_feat_list(tags, &mut sfont);
        g.otl_gsub_set_chain(tags);
        let _ = writeln!(out, "chain {r}");
        for gid in 0..n {
            let mut g2 = gid;
            let r = g.otl_gsub_apply_chain(&mut g2);
            if r == 0 || g2 != gid {
                let _ = writeln!(out, "c {gid} {g2} {r}");
            }
        }
        Some(out)
    }

    const FEATS: &[(&[u8], &[u8], &[u8])] = &[
        (b"*", b"*", b"liga"),
        (b"*", b"*", b"smcp"),
        (b"*", b"*", b"onum"),
        (b"*", b"*", b"salt"),
        (b"*", b"*", b"vert"),
        (b"*", b"*", b"(?lig|lig?|?cmp|cmp?|frac|afrc)"),
        (b"latn", b"dflt", b"liga"),
        (b"latn", b"*", b"c2sc"),
        (b"arab", b"*", b"init"),
        (b"*", b"*", b"*"),
    ];

    /// (name, font, md5 of the C harness's output, md5 of the font).
    const CASES: &[(&str, &str, &str, &str)] = &[
        (
            "dejavu",
            "/usr/share/texmf-dist/fonts/truetype/public/dejavu/DejaVuSans.ttf",
            "d21470f4cc127851afe5d8b31276623b",
            "b0e31de57cd5307954a3c54136ce68ae",
        ),
        (
            "ebgaramond",
            "/usr/share/texmf-dist/fonts/opentype/public/ebgaramond/EBGaramond-Regular.otf",
            "53c8ed11e55827945693e7cf5e8928a0",
            "c3133d2af9ea5c7f03dfc0b08cdfee46",
        ),
        (
            "amiri",
            "/usr/share/texmf-dist/fonts/truetype/public/amiri/Amiri-BoldItalic.ttf",
            "d816cc1e4dec2f453e7ad83d9586cd8d",
            "bca12f1468d2ff8ed1512e7f6e73fb16",
        ),
        (
            "spectral",
            "/usr/share/texmf-dist/fonts/truetype/production/spectral/Spectral-Regular.ttf",
            "7188377447f373854a09b700e538fbcf",
            "7b78ff83168097bf78ed628b3ed15d9c",
        ),
    ];

    fn hex(d: &[u8]) -> String {
        let mut s = String::new();
        for b in d {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    #[test]
    fn lookups() {
        let dir = std::env::var("PARTEX_XPDF_DUMP").ok();
        for &(name, path, want, font_md5) in CASES {
            let Some(out) = dump(path, FEATS) else {
                continue;
            };
            if let Some(dir) = dir.as_deref() {
                std::fs::write(std::format!("{dir}/{name}.gsub.txt"), &out).unwrap();
                let mut args = String::from(path);
                for (s, l, f) in FEATS {
                    let st = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
                    let _ = write!(args, " '{}' '{}' '{}'", st(s), st(l), st(f));
                }
                std::fs::write(std::format!("{dir}/{name}.gsub.args"), args).unwrap();
            }
            let font = std::fs::read(path).unwrap();
            if !want.is_empty() && hex(&partex_engine::md5::md5(&font)) == font_md5 {
                assert_eq!(
                    hex(&partex_engine::md5::md5(out.as_bytes())),
                    want,
                    "{name}"
                );
            }
        }
    }
}
