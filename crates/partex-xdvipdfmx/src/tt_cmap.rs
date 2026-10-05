//! tt_cmap.c, tt_cmap.h: the sfnt `cmap` table, and the CMaps made from
//! it (ToUnicode streams, Unicode input CMaps for OpenType fonts).
//!
//! CMaps built here are local (`&mut CMap`, `CMap_new`) until
//! `CMap_cache_add`; cached ones (`cmap_add`, the `-UCS32-Add` CMap) are
//! named by their id (`i32`) in the CMap cache.

#![allow(non_snake_case)]

use crate::cff::{CffFont, FONTTYPE_CIDFONT};
use crate::cid::CSI_UNICODE;
use crate::cmap::CMap;
use crate::cmap::{CMAP_TYPE_CODE_TO_CID, CMAP_TYPE_TO_UNICODE};
use crate::pdffont::{CidSysInfo, is_used_char2};
use crate::pdfresource::PDF_RES_FLUSH_IMMEDIATE;
use crate::prelude::*;
use crate::sfnt::{
    BYTE, SFNT_TYPE_DFONT, SFNT_TYPE_POSTSCRIPT, SFNT_TYPE_TRUETYPE, SFNT_TYPE_TTC, SHORT, Sfnt,
    ULONG, USHORT,
};
use crate::tt_gsub::OtlGsub;
use crate::tt_gsub::otl_gsub_add_ToUnicode;
use crate::tt_post::TtPostTable;
use crate::unicode::{UC_UTF16BE_encode_char, UC_is_valid};

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
    if len < 256 {
        warn!("invalid format 0 TT cmap subtable");
        return None;
    }
    let mut map = Box::new(Cmap0 {
        glyph_index_array: [0; 256],
    });
    for i in 0..256 {
        map.glyph_index_array[i] = sfont.sfnt_get_byte();
    }
    Some(map)
}
/// `lookup_cmap0` (static).
fn lookup_cmap0(map: &Cmap0, cc: USHORT) -> USHORT {
    if cc > 255 {
        0
    } else {
        USHORT::from(map.glyph_index_array[cc as usize])
    }
}
/// `read_cmap2` (static).
fn read_cmap2(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap2>> {
    if len < 512 {
        warn!("invalid fromt2 TT cmap subtable");
        return None;
    }

    let mut map = Box::new(Cmap2 {
        sub_header_keys: [0; 256],
        sub_headers: Vec::new(),
        glyph_index_array: Vec::new(),
    });
    for i in 0..256 {
        map.sub_header_keys[i] = sfont.sfnt_get_ushort();
    }
    let mut n: USHORT = 0;
    for i in 0..256 {
        map.sub_header_keys[i] /= 8;
        if n < map.sub_header_keys[i] {
            n = map.sub_header_keys[i];
        }
    }
    n = n.wrapping_add(1); /* the number of subHeaders is one plus the max of subHeaderKeys */

    if len < 512u32.wrapping_add(u32::from(n) * 8) {
        warn!("invalid/truncated format2 TT cmap subtable");
        return None;
    }

    map.sub_headers = Vec::with_capacity(n as usize);
    for i in 0..n {
        let mut sh = SubHeader {
            first_code: sfont.sfnt_get_ushort(),
            entry_count: sfont.sfnt_get_ushort(),
            id_delta: sfont.sfnt_get_short(),
            id_range_offset: sfont.sfnt_get_ushort(),
        };
        // It makes things easier to let the offset start from the
        // beginning of glyphIndexArray.
        if sh.id_range_offset != 0 {
            sh.id_range_offset = sh
                .id_range_offset
                .wrapping_sub((2 + (i32::from(n) - i32::from(i) - 1) * 8) as USHORT);
        }
        map.sub_headers.push(sh);
    }

    // The length of glyphIndexArray.
    let n = (len.wrapping_sub(518).wrapping_sub(u32::from(n) * 8) as USHORT) / 2;

    map.glyph_index_array = Vec::with_capacity(n as usize);
    for _ in 0..n {
        map.glyph_index_array.push(sfont.sfnt_get_ushort());
    }

    Some(map)
}
/// `lookup_cmap2` (static).
fn lookup_cmap2(map: &Cmap2, cc: USHORT) -> USHORT {
    let mut idx: USHORT = 0;

    let hi = (cc >> 8) & 0xff;
    let lo = i32::from(cc & 0xff);

    // Select which subHeader to use.
    let i = map.sub_header_keys[hi as usize] as usize;

    let first_code = map.sub_headers[i].first_code;
    let entry_count = map.sub_headers[i].entry_count;
    let id_delta = map.sub_headers[i].id_delta;
    let mut id_range_offset = map.sub_headers[i].id_range_offset / 2;

    if lo >= i32::from(first_code) && lo < i32::from(first_code) + i32::from(entry_count) {
        id_range_offset = id_range_offset.wrapping_add((lo - i32::from(first_code)) as USHORT);
        // C reads past the array for a broken font.
        idx = map
            .glyph_index_array
            .get(id_range_offset as usize)
            .copied()
            .unwrap_or(0);
        if idx != 0 {
            idx = ((i32::from(idx) + i32::from(id_delta)) & 0xffff) as USHORT;
        }
    }

    idx
}
/// `read_cmap4` (static).
fn read_cmap4(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap4>> {
    if len < 8 {
        warn!("invalid format 4 TT cmap subtable");
        return None;
    }

    let mut map = Box::new(Cmap4::default());

    let mut seg_count = sfont.sfnt_get_ushort();
    map.seg_count_x2 = seg_count;
    map.search_range = sfont.sfnt_get_ushort();
    map.entry_selector = sfont.sfnt_get_ushort();
    map.range_shift = sfont.sfnt_get_ushort();

    seg_count /= 2;

    let mut read = |sfont: &mut Sfnt, n: USHORT| -> Vec<USHORT> {
        let mut v = Vec::with_capacity(n as usize);
        for _ in 0..n {
            v.push(sfont.sfnt_get_ushort());
        }
        v
    };
    map.end_count = read(sfont, seg_count);
    map.reserved_pad = sfont.sfnt_get_ushort();
    map.start_count = read(sfont, seg_count);
    map.id_delta = read(sfont, seg_count);
    map.id_range_offset = read(sfont, seg_count);

    let n = (len.wrapping_sub(16).wrapping_sub(8 * u32::from(seg_count)) / 2) as USHORT;
    map.glyph_index_array = read(sfont, n);

    Some(map)
}
/// `lookup_cmap4` (static).
fn lookup_cmap4(map: &Cmap4, cc: USHORT) -> USHORT {
    let mut gid: USHORT = 0;

    // Segments are sorted in order of increasing endCode values. Last
    // segment maps 0xffff to gid 0 (?).
    let seg_count = map.seg_count_x2 / 2;
    let mut i = seg_count;
    while i > 0 && {
        i -= 1;
        cc <= map.end_count[i as usize]
    } {
        let iu = i as usize;
        if cc >= map.start_count[iu] {
            if map.id_range_offset[iu] == 0 {
                gid = ((u32::from(cc) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT;
            } else if cc == 0xffff && map.id_range_offset[iu] == 0xffff {
                // This is for protection against some old broken fonts...
                gid = 0;
            } else {
                let mut j = (i32::from(map.id_range_offset[iu])
                    - (i32::from(seg_count) - i32::from(i)) * 2)
                    as USHORT;
                j = (i32::from(cc) - i32::from(map.start_count[iu]) + i32::from(j / 2)) as USHORT;
                // C reads past the array for a broken font.
                gid = map.glyph_index_array.get(j as usize).copied().unwrap_or(0);
                if gid != 0 {
                    gid = ((u32::from(gid) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT;
                }
            }
            break;
        }
    }

    gid
}
/// `read_cmap6` (static).
fn read_cmap6(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap6>> {
    if len < 4 {
        warn!("invalid format 6 TT cmap subtable");
        return None;
    }
    let mut map = Box::new(Cmap6 {
        first_code: sfont.sfnt_get_ushort(),
        entry_count: sfont.sfnt_get_ushort(),
        glyph_index_array: Vec::new(),
    });
    map.glyph_index_array = Vec::with_capacity(map.entry_count as usize);
    for _ in 0..map.entry_count {
        map.glyph_index_array.push(sfont.sfnt_get_ushort());
    }
    Some(map)
}
/// `lookup_cmap6` (static).
fn lookup_cmap6(map: &Cmap6, cc: USHORT) -> USHORT {
    let idx = cc.wrapping_sub(map.first_code);
    if idx < map.entry_count {
        return map.glyph_index_array[idx as usize];
    }
    0
}
/// `read_cmap12` (static).
fn read_cmap12(sfont: &mut Sfnt, len: ULONG) -> Option<Box<Cmap12>> {
    if len < 4 {
        warn!("invalid format 12 TT cmap subtable");
        return None;
    }
    let mut map = Box::new(Cmap12 {
        n_groups: sfont.sfnt_get_ulong(),
        groups: Vec::new(),
    });
    for _ in 0..map.n_groups {
        let g = CharGroup {
            start_char_code: sfont.sfnt_get_ulong(),
            end_char_code: sfont.sfnt_get_ulong(),
            start_glyph_id: sfont.sfnt_get_ulong(),
        };
        map.groups.push(g);
    }
    Some(map)
}
/// `lookup_cmap12` (static).
fn lookup_cmap12(map: &Cmap12, cccc: ULONG) -> USHORT {
    let mut gid: USHORT = 0;
    let mut i = map.n_groups as i32;
    while i > 0 && {
        i -= 1;
        cccc <= map.groups[i as usize].end_char_code
    } {
        let g = &map.groups[i as usize];
        if cccc >= g.start_char_code {
            gid = (cccc
                .wrapping_sub(g.start_char_code)
                .wrapping_add(g.start_glyph_id)
                & 0xffff) as USHORT;
            break;
        }
    }
    gid
}
/// `read_cmap14` (static).
fn read_cmap14(sfont: &mut Sfnt, offset: ULONG, len: ULONG) -> Option<Box<Cmap14>> {
    if len < 4 {
        warn!("invalid format 14 TT cmap subtable");
        return None;
    }

    let mut map = Box::new(Cmap14 {
        num_var_selector_records: sfont.sfnt_get_ulong(),
        var_selector: Vec::new(),
    });
    let n = map.num_var_selector_records as usize;
    let mut default_uvs_offset: Vec<ULONG> = Vec::with_capacity(n);
    let mut non_default_uvs_offset: Vec<ULONG> = Vec::with_capacity(n);
    // Read VariationSelector Record.
    for _ in 0..n {
        let vs = VariationSelector {
            var_selector: sfont.sfnt_get_uint24(),
            ..VariationSelector::default()
        };
        map.var_selector.push(vs);
        default_uvs_offset.push(sfont.sfnt_get_ulong());
        non_default_uvs_offset.push(sfont.sfnt_get_ulong());
    }
    for i in 0..n {
        let vs = &mut map.var_selector[i];
        // Read DefaultUVS Table.
        if default_uvs_offset[i] > 0 {
            sfont.sfnt_seek_set(offset.wrapping_add(default_uvs_offset[i]));
            vs.num_unicode_value_ranges = sfont.sfnt_get_ulong();
            for _ in 0..vs.num_unicode_value_ranges {
                vs.ranges_start_unicode_value.push(sfont.sfnt_get_uint24());
                vs.ranges_additional_count.push(sfont.sfnt_get_byte());
            }
        } else {
            vs.num_unicode_value_ranges = 0;
        }
        // Read NonDefaultUVS Table.
        if non_default_uvs_offset[i] > 0 {
            sfont.sfnt_seek_set(offset.wrapping_add(non_default_uvs_offset[i]));
            vs.num_uvs_mappings = sfont.sfnt_get_ulong();
            for _ in 0..vs.num_uvs_mappings {
                vs.uvs_mappings_unicode_value.push(sfont.sfnt_get_uint24());
                vs.uvs_mappings_glyph_id.push(sfont.sfnt_get_ushort());
            }
        } else {
            vs.num_uvs_mappings = 0;
        }
    }

    Some(map)
}
/// `lookup_cmap14` (static).
fn lookup_cmap14(
    map: &Cmap14,
    cmap_default: Option<&TtCmap>,
    unicode: ULONG,
    uvs: ULONG,
) -> USHORT {
    for i in 0..map.num_var_selector_records as usize {
        let vs = &map.var_selector[i];
        if vs.var_selector == uvs {
            for j in 0..vs.num_unicode_value_ranges as usize {
                if vs.ranges_start_unicode_value[j] <= unicode
                    && unicode
                        <= vs.ranges_start_unicode_value[j]
                            .wrapping_add(u32::from(vs.ranges_additional_count[j]))
                {
                    return cmap_default
                        .expect("tt_cmap_uvs_lookup: no default cmap")
                        .tt_cmap_lookup(unicode);
                }
            }
            for j in 0..vs.num_uvs_mappings as usize {
                if vs.uvs_mappings_unicode_value[j] == unicode {
                    return vs.uvs_mappings_glyph_id[j];
                }
            }
            return 0;
        }
    }
    0
}

impl Sfnt {
    /// `tt_cmap_read`: the subtable for `platform`/`encoding`, if any.
    pub fn tt_cmap_read(&mut self, platform: USHORT, encoding: USHORT) -> Option<TtCmap> {
        let mut length: ULONG = 0;

        let mut offset = self.sfnt_locate_table(b"cmap");
        let _ = self.sfnt_get_ushort();
        let n_subtabs = self.sfnt_get_ushort();

        let mut i: USHORT = 0;
        while i < n_subtabs {
            let p_id = self.sfnt_get_ushort();
            let e_id = self.sfnt_get_ushort();
            if p_id != platform || e_id != encoding {
                self.sfnt_get_ulong();
            } else {
                offset = offset.wrapping_add(self.sfnt_get_ulong());
                break;
            }
            i += 1;
        }

        if i == n_subtabs {
            return None;
        }

        let mut cmap = TtCmap {
            format: 0,
            platform,
            encoding,
            language: 0,
            map: TtCmapMap::None,
        };

        self.sfnt_seek_set(offset);
        cmap.format = self.sfnt_get_ushort();
        // Length and version (language) are ULONG for format 8, 10, 12!
        if cmap.format <= 6 {
            length = ULONG::from(self.sfnt_get_ushort());
            cmap.language = ULONG::from(self.sfnt_get_ushort()); /* language (Mac) */
        } else if cmap.format != 14 {
            if self.sfnt_get_ushort() != 0 {
                // reserved - 0
                warn!("Unrecognized cmap subtable format.");
                return None;
            }
            length = self.sfnt_get_ulong();
            cmap.language = self.sfnt_get_ulong();
        } else {
            length = self.sfnt_get_ulong();
            cmap.language = 0;
        }

        cmap.map = match cmap.format {
            0 => read_cmap0(self, length).map_or(TtCmapMap::None, TtCmapMap::Cmap0),
            2 => read_cmap2(self, length).map_or(TtCmapMap::None, TtCmapMap::Cmap2),
            4 => read_cmap4(self, length).map_or(TtCmapMap::None, TtCmapMap::Cmap4),
            6 => read_cmap6(self, length).map_or(TtCmapMap::None, TtCmapMap::Cmap6),
            12 => read_cmap12(self, length).map_or(TtCmapMap::None, TtCmapMap::Cmap12),
            14 => read_cmap14(self, offset, length).map_or(TtCmapMap::None, TtCmapMap::Cmap14),
            _ => {
                warn!("Unrecognized OpenType/TrueType cmap format.");
                return None;
            }
        };

        if matches!(cmap.map, TtCmapMap::None) {
            return None;
        }

        Some(cmap)
    }
}

impl TtCmap {
    /// `tt_cmap_release`.
    pub fn tt_cmap_release(self) {}
    /// `tt_cmap_lookup`: the gid of `cc`, 0 if none.
    #[must_use]
    pub fn tt_cmap_lookup(&self, cc: ULONG) -> USHORT {
        if cc > 0xffff && self.format < 12 {
            warn!("Four bytes charcode not supported in OpenType/TrueType cmap format 0...6.");
            return 0;
        }

        match (&self.map, self.format) {
            (TtCmapMap::Cmap0(m), 0) => lookup_cmap0(m, cc as USHORT),
            (TtCmapMap::Cmap2(m), 2) => lookup_cmap2(m, cc as USHORT),
            (TtCmapMap::Cmap4(m), 4) => lookup_cmap4(m, cc as USHORT),
            (TtCmapMap::Cmap6(m), 6) => lookup_cmap6(m, cc as USHORT),
            (TtCmapMap::Cmap12(m), 12) => lookup_cmap12(m, cc),
            _ => {
                warn!(
                    "Unrecognized OpenType/TrueType cmap subtable format: {}",
                    self.format
                );
                0
            }
        }
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
    let TtCmapMap::Cmap14(map) = &cmap_uvs.map else {
        warn!("Unicode Variation Sequences in OpenType/TrueType cmap must be format 14.");
        return 0;
    };
    if cmap_uvs.format != 14 {
        warn!("Unicode Variation Sequences in OpenType/TrueType cmap must be format 14.");
        return 0;
    }
    lookup_cmap14(map, cmap_default, unicode, uvs)
}

/// `create_GIDToCIDMap` (static): fills `gid_to_cid_map[..num_glyphs]`
/// from the CFF charset. C writes past the map for charsets longer than
/// the font (format 0) and at `[num_glyphs]` (formats 1, 2): those
/// writes are dropped.
fn create_GIDToCIDMap(gid_to_cid_map: &mut [u16], num_glyphs: u16, cffont: &CffFont) {
    if cffont.flag & FONTTYPE_CIDFONT == 0 {
        for gid in 0..num_glyphs {
            gid_to_cid_map[gid as usize] = gid;
        }
        return;
    }

    for x in gid_to_cid_map[..num_glyphs as usize].iter_mut() {
        *x = 0;
    }

    let Some(charset) = cffont.charsets.as_ref() else {
        return;
    };
    let mut set = |gid: u16, cid: u16| {
        if let Some(x) = gid_to_cid_map.get_mut(gid as usize) {
            *x = cid;
        }
    };
    match charset.format {
        0 => {
            let cids = &charset.glyphs;
            let mut gid: u16 = 1;
            for i in 0..charset.num_entries as usize {
                set(gid, cids[i]);
                gid = gid.wrapping_add(1);
            }
        }
        1 => {
            let ranges = &charset.range1;
            let mut gid: u16 = 1;
            for i in 0..charset.num_entries as usize {
                let mut cid = ranges[i].first;
                let mut count: u16 = u16::from(ranges[i].n_left) + 1; /* card8 */
                while count > 0 && gid <= num_glyphs {
                    count -= 1;
                    set(gid, cid);
                    gid = gid.wrapping_add(1);
                    cid = cid.wrapping_add(1);
                }
                // C's `count-- > 0` also decrements when the test fails.
            }
        }
        2 => {
            let ranges = &charset.range2;
            if charset.num_entries == 1 && ranges[0].first == 1 {
                // "Complete" CIDFont.
                for gid in 0..num_glyphs {
                    set(gid, gid);
                }
            } else {
                // Not trivial mapping.
                let mut gid: u16 = 1;
                for i in 0..charset.num_entries as usize {
                    let mut cid = ranges[i].first;
                    let mut count: u16 = ranges[i].n_left.wrapping_add(1);
                    while count > 0 && gid <= num_glyphs {
                        count -= 1;
                        set(gid, cid);
                        gid = gid.wrapping_add(1);
                        cid = cid.wrapping_add(1);
                    }
                }
            }
        }
        f => {
            warn!("Unknown CFF charset format...: {}", f);
        }
    }
}

/// `is_PUA_or_presentation` (static).
fn is_PUA_or_presentation(uni: u32) -> bool {
    // Some of CJK Radicals Supplement and Kangxi Radicals are commonly
    // double encoded, lower the priority. CJK Compatibility Ideographs &
    // Supplement added. Soft-hyphen (U+00AD) to lower its priority.
    (0x2E80..=0x2EF3).contains(&uni)
        || (0x2F00..=0x2FD5).contains(&uni)
        || (0xE000..=0xF8FF).contains(&uni)
        || (0xFB00..=0xFB4F).contains(&uni)
        || (0xF900..=0xFAFF).contains(&uni)
        || (0x2F800..=0x2FA1F).contains(&uni)
        || (0xF0000..=0xFFFFD).contains(&uni)
        || (0x100000..=0x10FFFD).contains(&uni)
        || uni == 0x00AD
}

/// `lookup_glyph_name` (static): from `post`, else from the CFF charset.
fn lookup_glyph_name(
    post: Option<&TtPostTable>,
    cffont: Option<&CffFont>,
    gid: USHORT,
) -> Option<Vec<u8>> {
    let mut name = None;
    if let Some(post) = post {
        name = post.tt_get_glyphname(gid);
    }
    if name.is_none() {
        if let Some(cffont) = cffont {
            name = Some(cffont.cff_get_glyphname(gid));
        }
    }
    name
}

/// `create_inverse_cmap4` (static): fills `map_base`/`map_sub` (indexed
/// by gid, `num_glyphs` long; C writes past them for gids beyond the
/// font, dropped here).
fn create_inverse_cmap4(
    map_base: &mut [i32],
    map_sub: &mut [i32],
    num_glyphs: USHORT,
    map: &Cmap4,
) {
    let _ = num_glyphs;
    let seg_count = map.seg_count_x2 / 2;
    for i in 0..seg_count {
        let iu = i as usize;
        let c0 = map.start_count[iu];
        let c1 = map.end_count[iu];
        let d = (i32::from(map.id_range_offset[iu] / 2) - (i32::from(seg_count) - i32::from(i)))
            as USHORT;
        let n = i32::from(c1) - i32::from(c0);
        let mut j: i32 = 0;
        while j <= n {
            let ch = c0.wrapping_add(j as USHORT);
            let gid: USHORT = if map.id_range_offset[iu] == 0 {
                ((u32::from(ch) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT
            } else if c0 == 0xffff && c1 == 0xffff && map.id_range_offset[iu] == 0xffff {
                // This is for protection against some old broken fonts...
                0
            } else {
                let g = map
                    .glyph_index_array
                    .get((j + i32::from(d)) as usize)
                    .copied()
                    .unwrap_or(0);
                ((u32::from(g) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT
            };
            if is_PUA_or_presentation(u32::from(ch)) {
                if let Some(x) = map_sub.get_mut(gid as usize) {
                    *x = i32::from(ch);
                }
            } else if let Some(x) = map_base.get_mut(gid as usize) {
                *x = i32::from(ch);
            }
            j += 1;
        }
    }
}

/// `create_inverse_cmap12` (static).
fn create_inverse_cmap12(
    map_base: &mut [i32],
    map_sub: &mut [i32],
    num_glyphs: USHORT,
    map: &Cmap12,
) {
    let _ = num_glyphs;
    for i in 0..map.n_groups as usize {
        let g = &map.groups[i];
        let mut ch = g.start_char_code;
        while ch <= g.end_char_code {
            let d = ch.wrapping_sub(g.start_char_code) as i32;
            let gid = (g.start_glyph_id.wrapping_add(d as u32) & 0xffff) as USHORT;
            if is_PUA_or_presentation(ch) {
                if let Some(x) = map_sub.get_mut(gid as usize) {
                    *x = ch as i32;
                }
            } else if let Some(x) = map_base.get_mut(gid as usize) {
                *x = ch as i32;
            }
            if ch == u32::MAX {
                break;
            }
            ch += 1;
        }
    }
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
    let seg_count = map.seg_count_x2 / 2;
    let mut i: i32 = i32::from(seg_count) - 1;
    while i >= 0 {
        let iu = i as usize;
        let c0 = map.start_count[iu];
        let c1 = map.end_count[iu];
        let d = (i32::from(map.id_range_offset[iu] / 2) - (i32::from(seg_count) - i)) as USHORT;
        let n = i32::from(c1) - i32::from(c0);
        let mut j: i32 = 0;
        while j <= n {
            let ch = c0.wrapping_add(j as USHORT);
            let mut gid: USHORT = if map.id_range_offset[iu] == 0 {
                ((u32::from(ch) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT
            } else if c0 == 0xffff && c1 == 0xffff && map.id_range_offset[iu] == 0xffff {
                // This is for protection against some old broken fonts...
                0
            } else {
                let g = map
                    .glyph_index_array
                    .get((j + i32::from(d)) as usize)
                    .copied()
                    .unwrap_or(0);
                ((u32::from(g) + u32::from(map.id_delta[iu])) & 0xffff) as USHORT
            };
            if gid != 0 && gid != 0xffff {
                // Apply GSUB features.
                if let Some(gsub_list) = gsub_list {
                    gsub_list.otl_gsub_apply_chain(&mut gid);
                }
                if let Some(gsub_vert) = gsub_vert {
                    gsub_vert.otl_gsub_apply(&mut gid);
                }
                let cid = if gid < num_glyphs {
                    gid_to_cid_map[gid as usize]
                } else {
                    0
                };
                let buf = [0, 0, (ch >> 8) as u8, (ch & 0xff) as u8];
                cmap.CMap_add_cidchar(&buf, cid);
                // For ToUnicode creation.
                if is_PUA_or_presentation(u32::from(ch)) {
                    if let Some(x) = map_sub.get_mut(gid as usize) {
                        *x = i32::from(ch);
                    }
                } else if let Some(x) = map_base.get_mut(gid as usize) {
                    *x = i32::from(ch);
                }
            }
            j += 1;
        }
        i -= 1;
    }
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
    for i in 0..map.n_groups as usize {
        let g = &map.groups[i];
        let mut ch = g.start_char_code;
        while ch <= g.end_char_code {
            let d = ch.wrapping_sub(g.start_char_code) as i32;
            let mut gid = (g.start_glyph_id.wrapping_add(d as u32) & 0xffff) as USHORT;
            if let Some(gsub_list) = gsub_list {
                gsub_list.otl_gsub_apply_chain(&mut gid);
            }
            if let Some(gsub_vert) = gsub_vert {
                gsub_vert.otl_gsub_apply(&mut gid);
            }
            let cid = if gid < num_glyphs {
                gid_to_cid_map[gid as usize]
            } else {
                0
            };
            let buf = [
                (ch >> 24) as u8,
                (ch >> 16) as u8,
                (ch >> 8) as u8,
                (ch & 0xff) as u8,
            ];
            cmap.CMap_add_cidchar(&buf, cid);
            if is_PUA_or_presentation(ch) {
                if let Some(x) = map_sub.get_mut(gid as usize) {
                    *x = ch as i32;
                }
            } else if let Some(x) = map_base.get_mut(gid as usize) {
                *x = ch as i32;
            }
            if ch == u32::MAX {
                break;
            }
            ch += 1;
        }
    }
}

/// The CMap bit `c` of `used_chars` cleared.
fn clear_used_char2(used_chars: &mut [u8], c: u16) {
    used_chars[(c / 8) as usize] &= !(1 << (7 - (c % 8)));
}

/// A font file opened as C's otf_* functions open it: TrueType, then
/// OpenType, then dfont (`ttc_index`).
fn open_sfnt(dpx: &mut Dpx, name: &[u8], ttc_index: u32) -> Option<Option<Sfnt>> {
    use crate::dpxfile::ResType;
    if let Some(fp) = dpx.dpx_open_file(name, ResType::TtFont) {
        return Some(Sfnt::sfnt_open(fp));
    }
    if let Some(fp) = dpx.dpx_open_file(name, ResType::OtFont) {
        return Some(Sfnt::sfnt_open(fp));
    }
    if let Some(fp) = dpx.dpx_open_file(name, ResType::DFont) {
        return Some(Sfnt::dfont_open(fp, ttc_index as i32));
    }
    None
}

/// `csi.registry = strdup("Adobe"); csi.ordering = strdup("Identity");
/// csi.supplement = 0;`
fn csi_adobe_identity() -> CidSysInfo {
    CidSysInfo {
        registry: Some(b"Adobe".to_vec()),
        ordering: Some(b"Identity".to_vec()),
        supplement: 0,
    }
}

/// The CIDSystemInfo of a CID-keyed CFF font (its ROS, or
/// Adobe-Identity-0).
fn cff_csi(cffont: &CffFont) -> CidSysInfo {
    let topdict = cffont.topdict.as_ref().expect("CFF: no Top DICT");
    if topdict.cff_dict_known(b"ROS") == 0 {
        csi_adobe_identity()
    } else {
        let reg = topdict.cff_dict_get(b"ROS", 0) as u16;
        let ord = topdict.cff_dict_get(b"ROS", 1) as u16;
        CidSysInfo {
            registry: Some(cffont.cff_get_string(reg)),
            ordering: Some(cffont.cff_get_string(ord)),
            supplement: topdict.cff_dict_get(b"ROS", 2) as i32,
        }
    }
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
        let mut count = 0;
        for cid in 0..65536u32 {
            if !is_used_char2(used_chars, cid) {
                continue;
            }
            let inbuf = [((cid >> 8) & 0xff) as u8, (cid & 0xff) as u8];
            let mut outbuf = [0u8; 254];
            let mut inpos = 0usize;
            let mut inbytesleft = 2;
            let mut outpos = 0usize;
            let mut outbytesleft = 254;
            let add = self.CMap_cache_get(cmap_add);
            self.CMap_decode(
                add,
                &inbuf,
                &mut inpos,
                &mut inbytesleft,
                &mut outbuf,
                &mut outpos,
                &mut outbytesleft,
            );
            if inbytesleft == 0 {
                let len = (254 - outbytesleft) as usize;
                cmap.CMap_add_bfchar(&inbuf, &outbuf[..len]);
                clear_used_char2(used_chars, cid as u16);
                count += 1;
            }
        }
        count
    }

    /// `add_ToUnicode_via_glyph_name` (static): AGL lookups of the glyph
    /// names of used gids; the count added.
    fn add_ToUnicode_via_glyph_name(
        &mut self,
        cmap: &mut CMap,
        used_chars: &mut [u8],
        num_glyphs: USHORT,
        gid_to_cid_map: &[u16],
        sfont: &mut Sfnt,
        cffont: Option<&CffFont>,
    ) -> i32 {
        let mut count = 0;

        let post = sfont.tt_read_post_table();
        if post.is_none() && cffont.is_none() {
            return count;
        }

        for gid in 0..num_glyphs {
            let cid = gid_to_cid_map[gid as usize];
            if is_used_char2(used_chars, u32::from(cid)) {
                let mut unicodes = [0i32; MAX_UNICODES];
                if let Some(name) = lookup_glyph_name(post.as_ref(), cffont, gid) {
                    let unicode_count = self.agl_get_unicodes(&name, &mut unicodes);
                    if unicode_count > 0 {
                        let n = unicode_count as usize;
                        let mut buf = vec![0u8; n * 4 + 2];
                        let mut p = 2usize;
                        let mut len = 0usize;
                        for k in 0..n {
                            len += UC_UTF16BE_encode_char(unicodes[k], &mut buf, &mut p);
                        }
                        buf[0] = (cid >> 8) as u8;
                        buf[1] = (cid & 0xff) as u8;
                        cmap.CMap_add_bfchar(&buf[..2], &buf[2..2 + len]);
                        clear_used_char2(used_chars, cid);
                        count += 1;
                    }
                }
            }
        }

        count
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
        // Get num_glyphs from maxp table.
        let num_glyphs = sfont.tt_read_maxp_table().num_glyphs;

        // Initialize GID to Unicode mapping table.
        let mut map_base = vec![-1i32; num_glyphs as usize];
        let mut map_sub = vec![-1i32; num_glyphs as usize];

        // Create "base" mapping from inverse mapping of OpenType cmap.
        match &ttcmap.map {
            TtCmapMap::Cmap4(m) if ttcmap.format == 4 => {
                create_inverse_cmap4(&mut map_base, &mut map_sub, num_glyphs, m);
            }
            TtCmapMap::Cmap12(m) if ttcmap.format == 12 => {
                create_inverse_cmap12(&mut map_base, &mut map_sub, num_glyphs, m);
            }
            _ => {}
        }

        // Now create ToUnicode CMap stream.
        let mut cffont: Option<CffFont> = None;
        if sfont.type_ == SFNT_TYPE_POSTSCRIPT {
            let offset = sfont.sfnt_find_table_pos(b"CFF ");
            // "CFF " table must exist here. Just abort...
            if offset == 0 {
                error!("\"CFF \" table not found. Must be found before... Can't continue.");
            }
            let mut c =
                CffFont::cff_open(sfont.stream.clone(), offset as i32, 0).expect("cff_open failed");
            c.cff_read_charsets();
            cffont = Some(c);
        }
        let is_cidfont = cffont
            .as_ref()
            .is_some_and(|c| c.flag & FONTTYPE_CIDFONT != 0);

        // GID to CID mapping info.
        let mut gid_to_cid_map = vec![0u16; num_glyphs as usize];
        if is_cidfont {
            create_GIDToCIDMap(&mut gid_to_cid_map, num_glyphs, cffont.as_ref().unwrap());
        } else {
            for gid in 0..num_glyphs {
                gid_to_cid_map[gid as usize] = gid;
            }
        }
        let mut cmap = CMap::CMap_new();
        cmap.CMap_set_name(cmap_name);
        cmap.CMap_set_wmode(0);
        cmap.CMap_set_type(CMAP_TYPE_TO_UNICODE);
        cmap.CMap_set_CIDSysInfo(Some(&CSI_UNICODE()));
        cmap.CMap_add_codespacerange(&SRANGE_MIN, &SRANGE_MAX);

        let mut count: i32 = 0;
        let mut used_chars_copy = used_chars[..8192].to_vec();
        for gid in 0..num_glyphs {
            let cid = gid_to_cid_map[gid as usize];
            if is_used_char2(&used_chars_copy, u32::from(cid)) {
                let ch = map_base[gid as usize];
                if UC_is_valid(ch) {
                    let src = [(cid >> 8) as u8, (cid & 0xff) as u8];
                    let mut dst = [0u8; 4];
                    let mut p = 0usize;
                    let len = UC_UTF16BE_encode_char(ch, &mut dst, &mut p);
                    cmap.CMap_add_bfchar(&src, &dst[..len]);
                    clear_used_char2(&mut used_chars_copy, cid);
                    count += 1;
                }
            }
        }

        // cmap_add here stores information about all unencoded glyphs
        // which can be accessed only through OT Layout GSUB table. This is
        // only available when encoding is "unicode".
        if let Some(cmap_add) = cmap_add {
            count += self.handle_subst_glyphs(&mut cmap, cmap_add, &mut used_chars_copy);
        } else {
            // Else, try gathering information from GSUB tables.
            count += otl_gsub_add_ToUnicode(
                &mut cmap,
                &mut used_chars_copy,
                &map_base,
                &map_sub,
                num_glyphs,
                &gid_to_cid_map,
                sfont,
            );
        }
        // Find Unicode mapping via PostScript glyph names...
        count += self.add_ToUnicode_via_glyph_name(
            &mut cmap,
            &mut used_chars_copy,
            num_glyphs,
            &gid_to_cid_map,
            sfont,
            if is_cidfont { None } else { cffont.as_ref() },
        );
        if let Some(c) = cffont {
            c.cff_close();
        }

        // Finally, PUA and presentation forms...
        for gid in 0..num_glyphs {
            let cid = gid_to_cid_map[gid as usize];
            if is_used_char2(&used_chars_copy, u32::from(cid)) {
                let ch = map_sub[gid as usize];
                if UC_is_valid(ch) {
                    let src = [(cid >> 8) as u8, (cid & 0xff) as u8];
                    let mut dst = [0u8; 4];
                    let mut p = 0usize;
                    let len = UC_UTF16BE_encode_char(ch, &mut dst, &mut p);
                    cmap.CMap_add_bfchar(&src, &dst[..len]);
                    clear_used_char2(&mut used_chars_copy, cid);
                    count += 1;
                }
            }
        }

        if count < 1 {
            None
        } else {
            self.CMap_create_stream(&cmap)
        }
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
        let font_name = map_name;
        let mut cmap_ref: Option<Obj> = None;

        let mut cmap_name = basefont.to_vec();
        cmap_name.extend_from_slice(b"-UTF16");

        let cmap_id = self.pdf_findresource(b"CMap", &cmap_name);
        if cmap_id >= 0 {
            return self.pdf_get_resource_reference(cmap_id);
        }

        let mut sfont = open_sfnt(self, font_name, ttc_index)??;

        let offset: ULONG = match sfont.type_ {
            SFNT_TYPE_DFONT => sfont.offset,
            SFNT_TYPE_TTC => {
                let offset = sfont.ttc_read_offset(ttc_index);
                if offset == 0 {
                    warn!("Invalid TTC index for font");
                    return None;
                }
                offset
            }
            _ => 0,
        };

        if sfont.sfnt_read_table_directory(offset) < 0 {
            warn!("Could not read OpenType/TrueType table directory");
            return None;
        }

        // cmap_add stores the ToUnicode mapping of unencoded glyphs reached
        // only through GSUB substitution ("unicode" encoding in the
        // fontmap).
        let cmap_add = {
            let mut cmap_add_name = font_name.to_vec();
            cmap_add_name.extend_from_slice(format!(":{}-UCS32-Add", ttc_index as i32).as_bytes());
            let cmap_add_id = self.CMap_cache_find(&cmap_add_name);
            if cmap_add_id < 0 {
                None
            } else {
                Some(cmap_add_id)
            }
        };

        let mut ttcmap = None;
        for pe in &CMAP_PLAT_ENCS {
            let Some(t) = sfont.tt_cmap_read(pe.platform as USHORT, pe.encoding as USHORT) else {
                continue;
            };
            if t.format == 4 || t.format == 12 {
                ttcmap = Some(t);
                break;
            }
        }
        if let Some(ttcmap) = ttcmap {
            self.CMap_set_silent(1); /* many warnings without this... */
            let cmap_obj =
                self.create_ToUnicode_cmap(&ttcmap, &cmap_name, cmap_add, used_chars, &mut sfont);
            self.CMap_set_silent(0);
            if let Some(cmap_obj) = cmap_obj {
                let cmap_id = self.pdf_defineresource(
                    b"CMap",
                    Some(&cmap_name),
                    cmap_obj,
                    PDF_RES_FLUSH_IMMEDIATE,
                );
                cmap_ref = self.pdf_get_resource_reference(cmap_id);
            }
        }

        cmap_ref
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
        // First look for cache if it was already loaded.
        let mut cmap_name = map_name.to_vec();
        let h_or_v = if wmode != 0 { "V" } else { "H" };
        if let Some(otl_tags) = otl_tags {
            cmap_name.extend_from_slice(format!(":{}:", ttc_index as i32).as_bytes());
            cmap_name.extend_from_slice(otl_tags);
            cmap_name.extend_from_slice(format!("-UCS4-{h_or_v}").as_bytes());
        } else {
            cmap_name.extend_from_slice(format!(":{}-UCS4-{h_or_v}", ttc_index as i32).as_bytes());
        }
        let mut cmap_id = self.CMap_cache_find(&cmap_name);
        if cmap_id >= 0 {
            return cmap_id;
        }

        let Some(sfont) = open_sfnt(self, map_name, ttc_index) else {
            return -1;
        };
        let Some(mut sfont) = sfont else {
            warn!("Could not open OpenType/TrueType/dfont font file");
            return -1;
        };
        let offset: ULONG = match sfont.type_ {
            SFNT_TYPE_TTC => {
                let offset = sfont.ttc_read_offset(ttc_index);
                if offset == 0 {
                    warn!("Offset=0 returned for font, TTC_index={}", ttc_index);
                    return -1;
                }
                offset
            }
            SFNT_TYPE_TRUETYPE | SFNT_TYPE_POSTSCRIPT => 0,
            SFNT_TYPE_DFONT => sfont.offset,
            _ => {
                warn!("Not a OpenType/TrueType/TTC font?");
                return -1;
            }
        };

        if sfont.sfnt_read_table_directory(offset) < 0 {
            warn!("Could not read OpenType/TrueType table directory");
            return -1;
        }

        let num_glyphs: u16 = sfont.tt_read_maxp_table().num_glyphs;

        let mut gid_to_cid_map = vec![0u16; num_glyphs as usize];
        let csi;
        if sfont.type_ == SFNT_TYPE_POSTSCRIPT {
            let offset = sfont.sfnt_find_table_pos(b"CFF ");
            // Possibly "CFF2" table for variable font: not supported.
            if offset == 0 {
                warn!("PS OpenType but no \"CFF \" table.. Maybe variable font? (not supported)");
                return -1;
            }
            let Some(mut cffont) = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0) else {
                return -1;
            };
            if cffont.flag & FONTTYPE_CIDFONT == 0 {
                csi = csi_adobe_identity();
                for gid in 0..num_glyphs {
                    gid_to_cid_map[gid as usize] = gid;
                }
            } else {
                csi = cff_csi(&cffont);
                cffont.cff_read_charsets();
                create_GIDToCIDMap(&mut gid_to_cid_map, num_glyphs, &cffont);
            }
            cffont.cff_close();
        } else {
            csi = csi_adobe_identity();
            for gid in 0..num_glyphs {
                gid_to_cid_map[gid as usize] = gid;
            }
        }

        let ttcmap = sfont
            .tt_cmap_read(3, 10) /* Microsoft UCS4 */
            .or_else(|| sfont.tt_cmap_read(3, 1)) /* Microsoft UCS2 */
            .or_else(|| sfont.tt_cmap_read(0, 3)) /* Unicode 2.0 or later */
            .or_else(|| sfont.tt_cmap_read(0, 4));

        if let Some(ttcmap) = ttcmap {
            let gsub_vert = if wmode == 1 {
                let mut gsub_vert = OtlGsub::otl_gsub_new();
                if gsub_vert.otl_gsub_add_feat(b"*", b"*", b"vrt2", &mut sfont) < 0 {
                    if gsub_vert.otl_gsub_add_feat(b"*", b"*", b"vert", &mut sfont) < 0 {
                        warn!("GSUB feature vrt2/vert not found.");
                        None
                    } else {
                        gsub_vert.otl_gsub_select(b"*", b"*", b"vert");
                        Some(gsub_vert)
                    }
                } else {
                    gsub_vert.otl_gsub_select(b"*", b"*", b"vrt2");
                    Some(gsub_vert)
                }
            } else {
                None
            };
            let gsub_list = if let Some(otl_tags) = otl_tags {
                let mut gsub_list = OtlGsub::otl_gsub_new();
                if gsub_list.otl_gsub_add_feat_list(otl_tags, &mut sfont) < 0 {
                    warn!("Reading GSUB feature table(s) failed");
                } else {
                    gsub_list.otl_gsub_set_chain(otl_tags);
                }
                Some(gsub_list)
            } else {
                None
            };
            let mut cmap = CMap::CMap_new();
            cmap.CMap_set_name(&cmap_name);
            cmap.CMap_set_type(CMAP_TYPE_CODE_TO_CID);
            cmap.CMap_set_wmode(wmode);
            cmap.CMap_add_codespacerange(&LRANGE_MIN, &LRANGE_MAX);
            cmap.CMap_set_CIDSysInfo(Some(&csi));
            let mut map_base = vec![-1i32; num_glyphs as usize];
            let mut map_sub = vec![-1i32; num_glyphs as usize];
            match &ttcmap.map {
                TtCmapMap::Cmap12(m) if ttcmap.format == 12 => {
                    load_cmap12(
                        m,
                        &gid_to_cid_map,
                        num_glyphs,
                        gsub_vert.as_ref(),
                        gsub_list.as_ref(),
                        &mut cmap,
                        &mut map_base,
                        &mut map_sub,
                    );
                }
                TtCmapMap::Cmap4(m) if ttcmap.format == 4 => {
                    load_cmap4(
                        m,
                        &gid_to_cid_map,
                        num_glyphs,
                        gsub_vert.as_ref(),
                        gsub_list.as_ref(),
                        &mut cmap,
                        &mut map_base,
                        &mut map_sub,
                    );
                }
                _ => {}
            }
            drop(gsub_vert);
            drop(gsub_list);
            drop(ttcmap);

            if otl_tags.is_some() {
                let mut tounicode_name = map_name.to_vec();
                tounicode_name
                    .extend_from_slice(format!(":{}-UCS32-Add", ttc_index as i32).as_bytes());
                let mut tounicode_id = self.CMap_cache_find(&tounicode_name);
                if tounicode_id < 0 {
                    let mut tounicode = CMap::CMap_new();
                    tounicode.CMap_set_name(&tounicode_name);
                    tounicode.CMap_set_type(CMAP_TYPE_TO_UNICODE);
                    tounicode.CMap_set_wmode(0);
                    tounicode.CMap_add_codespacerange(&SRANGE_MIN, &SRANGE_MAX);
                    tounicode.CMap_set_CIDSysInfo(Some(&CSI_UNICODE()));
                    tounicode.CMap_add_bfchar(&SRANGE_MIN, &SRANGE_MAX);
                    tounicode_id = self.CMap_cache_add(tounicode);
                }

                let tounicode = self.CMap_cache_get_mut(tounicode_id);
                for gid in 0..num_glyphs as usize {
                    let cid = gid_to_cid_map[gid];
                    if cid > 0 {
                        let ch = if UC_is_valid(map_base[gid]) {
                            map_base[gid]
                        } else {
                            map_sub[gid]
                        };
                        if UC_is_valid(ch) {
                            let src = [(cid >> 8) as u8, (cid & 0xff) as u8];
                            let mut dst = [0u8; 4];
                            let mut p = 0usize;
                            let len = UC_UTF16BE_encode_char(ch, &mut dst, &mut p);
                            if len > 0 {
                                tounicode.CMap_add_bfchar(&src, &dst[..len]);
                            }
                        }
                    }
                }
            }
            cmap_id = self.CMap_cache_add(cmap);
        }

        cmap_id
    }

    /// `otf_try_load_GID_to_CID_map`: the cmap id of the GID-to-CID CMap
    /// of a CID-keyed CFF OpenType font, -1 if not one.
    pub fn otf_try_load_GID_to_CID_map(
        &mut self,
        map_name: &[u8],
        ttc_index: u32,
        wmode: i32,
    ) -> i32 {
        let mut cmap_id;

        // Check if already loaded.
        let mut cmap_name = map_name.to_vec();
        cmap_name.extend_from_slice(format!(":{}-{}-GID", ttc_index as i32, wmode).as_bytes());
        cmap_id = self.CMap_cache_find(&cmap_name);
        if cmap_id >= 0 {
            return cmap_id;
        }

        let Some(sfont) = open_sfnt(self, map_name, ttc_index) else {
            return -1;
        };
        let Some(mut sfont) = sfont else {
            warn!("Could not open OpenType/TrueType/dfont font file");
            return -1;
        };
        let offset: ULONG = match sfont.type_ {
            SFNT_TYPE_TTC => {
                let offset = sfont.ttc_read_offset(ttc_index);
                if offset == 0 {
                    warn!("Invalid TTC index for font: {}", ttc_index);
                    return -1;
                }
                offset
            }
            SFNT_TYPE_TRUETYPE | SFNT_TYPE_POSTSCRIPT => 0,
            SFNT_TYPE_DFONT => sfont.offset,
            _ => {
                warn!("Not a OpenType/TrueType/TTC font?");
                return -1;
            }
        };

        if sfont.sfnt_read_table_directory(offset) < 0 {
            warn!("Could not read OpenType/TrueType table directory");
            return -1;
        }
        if sfont.type_ != SFNT_TYPE_POSTSCRIPT {
            return -1;
        }

        // Read GID-to-CID mapping if CFF OpenType is found.
        let csrange: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
        let num_glyphs: u16 = sfont.tt_read_maxp_table().num_glyphs;

        let offset = sfont.sfnt_find_table_pos(b"CFF ");
        if offset == 0 {
            warn!("PS OpenType but no \"CFF \" table.. Maybe variable font? (not supported)");
            return -1;
        }
        let cffont = CffFont::cff_open(sfont.stream.clone(), offset as i32, 0);
        if let Some(mut cffont) = cffont {
            if cffont.flag & FONTTYPE_CIDFONT != 0 {
                let csi = cff_csi(&cffont);
                cffont.cff_read_charsets();
                let mut gid_to_cid_map = vec![0u16; num_glyphs as usize];
                create_GIDToCIDMap(&mut gid_to_cid_map, num_glyphs, &cffont);
                let mut cmap = CMap::CMap_new();
                cmap.CMap_set_name(&cmap_name);
                cmap.CMap_set_type(CMAP_TYPE_CODE_TO_CID);
                cmap.CMap_set_wmode(wmode);
                cmap.CMap_add_codespacerange(&csrange[0..2], &csrange[2..4]);
                cmap.CMap_set_CIDSysInfo(Some(&csi));
                for gid in 0..num_glyphs {
                    let src = [(gid >> 8) as u8, (gid & 0xff) as u8];
                    let c = gid_to_cid_map[gid as usize];
                    let dst = [(c >> 8) as u8, (c & 0xff) as u8];
                    cmap.CMap_add_bfchar(&src, &dst);
                }
                cmap_id = self.CMap_cache_add(cmap);
            }
            cffont.cff_close();
        }

        cmap_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmap4_lookup() {
        // Two segments: 0x20..0x7e by delta -29, and the 0xffff end.
        let map = Cmap4 {
            seg_count_x2: 4,
            end_count: vec![0x7e, 0xffff],
            start_count: vec![0x20, 0xffff],
            id_delta: vec![(-29i16) as u16, 1],
            id_range_offset: vec![0, 0],
            ..Cmap4::default()
        };
        assert_eq!(lookup_cmap4(&map, 0x41), 0x41 - 29);
        assert_eq!(lookup_cmap4(&map, 0x10), 0);
        assert_eq!(lookup_cmap4(&map, 0x80), 0);
        assert_eq!(lookup_cmap4(&map, 0xffff), 0);
        let mut base = vec![-1; 100];
        let mut sub = vec![-1; 100];
        create_inverse_cmap4(&mut base, &mut sub, 100, &map);
        assert_eq!(base[0x41 - 29], 0x41);
        assert_eq!(base[0], 0xffff);
    }

    #[test]
    fn cmap12_lookup() {
        let map = Cmap12 {
            n_groups: 2,
            groups: vec![
                CharGroup {
                    start_char_code: 0x41,
                    end_char_code: 0x5a,
                    start_glyph_id: 10,
                },
                CharGroup {
                    start_char_code: 0x1d400,
                    end_char_code: 0x1d419,
                    start_glyph_id: 100,
                },
            ],
        };
        assert_eq!(lookup_cmap12(&map, 0x42), 11);
        assert_eq!(lookup_cmap12(&map, 0x1d401), 101);
        assert_eq!(lookup_cmap12(&map, 0x60), 0);
        assert!(is_PUA_or_presentation(0xE000));
        assert!(is_PUA_or_presentation(0xAD));
        assert!(!is_PUA_or_presentation(0x41));
    }
}
