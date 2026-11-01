// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/TtfUtil.cpp, src/inc/TtfTypes.h,
// src/inc/TtfUtil.h), LGPL-2.1-or-later.

//! The TrueType table reads graphite2 makes (`TtfUtil`): the table checks,
//! the cmap subtables (formats 4 and 12), `hmtx`, `loca` and `glyf`.
//! Sizes are those of TtfTypes.h's packed structs.

use crate::be::{i16_at, u16_at, u32_at};

pub(crate) type Tag = u32;

pub(crate) const fn tag(s: &[u8; 4]) -> Tag {
    u32::from_be_bytes(*s)
}

pub(crate) const CMAP: Tag = tag(b"cmap");
pub(crate) const HEAD: Tag = tag(b"head");
pub(crate) const HHEA: Tag = tag(b"hhea");
pub(crate) const HMTX: Tag = tag(b"hmtx");
pub(crate) const MAXP: Tag = tag(b"maxp");
pub(crate) const NAME: Tag = tag(b"name");
pub(crate) const POST: Tag = tag(b"post");
pub(crate) const OS_2: Tag = tag(b"OS/2");
pub(crate) const GLYF: Tag = tag(b"glyf");
pub(crate) const LOCA: Tag = tag(b"loca");
pub(crate) const SILF: Tag = tag(b"Silf");
pub(crate) const GLAT: Tag = tag(b"Glat");
pub(crate) const GLOC: Tag = tag(b"Gloc");
pub(crate) const FEAT: Tag = tag(b"Feat");
pub(crate) const SILL: Tag = tag(b"Sill");

const ONE_FIX: u32 = 1 << 16;
const SIZEOF_CMAP: usize = 12; // CharacterCodeMap
const SIZEOF_HEAD: usize = 54; // FontHeader
const SIZEOF_POST: usize = 32; // PostScriptGlyphName
const SIZEOF_HHEA: usize = 36; // HorizontalHeader
const SIZEOF_MAXP: usize = 32; // MaximumProfile
const SIZEOF_NAME: usize = 18; // FontNames
const SIZEOF_GLYPH: usize = 10; // Glyph
const SIZEOF_CMAP4: usize = 16; // CmapSubTableFormat4
const SIZEOF_CMAP12: usize = 28; // CmapSubTableFormat12

/// `CheckTable`: whether the table can be used.
pub(crate) fn check_table(t: Tag, d: &[u8]) -> bool {
    let n = d.len();
    if n < 4 {
        return false;
    }
    match t {
        CMAP => n >= SIZEOF_CMAP && u16_at(d, 0) == 0,
        HEAD => {
            n >= SIZEOF_HEAD
                && u32_at(d, 0) == ONE_FIX
                && u32_at(d, 12) == 0x5F0F_3CF5
                && i16_at(d, 52) == 0
                && matches!(i16_at(d, 50), 0 | 1)
        }
        POST => n >= SIZEOF_POST && matches!(u32_at(d, 0), 0x10000 | 0x20000 | 0x30000 | 0x28000),
        HHEA => n >= SIZEOF_HHEA && u32_at(d, 0) == ONE_FIX && i16_at(d, 32) == 0,
        MAXP => n >= SIZEOF_MAXP && u32_at(d, 0) == ONE_FIX,
        // (a version 0 to 4 table too short for its version passes: the
        // C++ falls out of its size checks to `return true`)
        OS_2 => u16_at(d, 0) <= 4,
        NAME => n >= SIZEOF_NAME && u16_at(d, 0) == 0,
        GLYF => n >= SIZEOF_GLYPH,
        _ => true,
    }
}

/// `GlyphCount`.
pub(crate) fn glyph_count(maxp: &[u8]) -> usize {
    usize::from(u16_at(maxp, 4))
}

/// `DesignUnits`.
pub(crate) fn design_units(head: &[u8]) -> i32 {
    i32::from(u16_at(head, 18))
}

/// `HorMetrics`: (lsb, advance) or `None`.
pub(crate) fn hor_metrics(gid: u16, hmtx: &[u8], hhea: &[u8]) -> Option<(i32, u32)> {
    let n_long = usize::from(u16_at(hhea, 34));
    let g = usize::from(gid);
    if g < n_long {
        if (g + 1) * 4 > hmtx.len() {
            return None;
        }
        Some((
            i32::from(i16_at(hmtx, g * 4 + 2)),
            u32::from(u16_at(hmtx, g * 4)),
        ))
    } else {
        let lsb_off = 4 * n_long + 2 * (g - n_long);
        if lsb_off >= hmtx.len().wrapping_sub(2) || n_long == 0 {
            return None;
        }
        Some((
            i32::from(i16_at(hmtx, lsb_off)),
            u32::from(u16_at(hmtx, (n_long - 1) * 4)),
        ))
    }
}

/// `FindCmapSubtable(cmap, platform, encoding, length)`: the subtable's
/// offset in `cmap`.
pub(crate) fn find_cmap_subtable(cmap: &[u8], platform: u16, encoding: i32) -> Option<usize> {
    let length = cmap.len();
    let n = usize::from(u16_at(cmap, 2));
    if length != 0 && SIZEOF_CMAP.wrapping_add(8usize.wrapping_mul(n.wrapping_sub(1))) > length {
        return None;
    }
    for i in 0..n {
        let e = 4 + 8 * i;
        if u16_at(cmap, e) == platform
            && (encoding == -1 || i32::from(u16_at(cmap, e + 2)) == encoding)
        {
            let offset = u32_at(cmap, e + 4) as usize;
            if length != 0 {
                if offset > length.wrapping_sub(2) {
                    return None;
                }
                let format = u16_at(cmap, offset);
                if format == 4 {
                    if offset > length.wrapping_sub(4) {
                        return None;
                    }
                    let sub_len = usize::from(u16_at(cmap, offset + 2));
                    if i + 1 == n {
                        if sub_len > length - offset {
                            return None;
                        }
                    } else if sub_len > u32_at(cmap, e + 8 + 4) as usize {
                        return None;
                    }
                }
                if format == 12 {
                    if offset > length.wrapping_sub(6) {
                        return None;
                    }
                    let sub_len = u32_at(cmap, offset + 2) as usize;
                    if i + 1 == n {
                        if sub_len > length - offset {
                            return None;
                        }
                    } else if sub_len > u32_at(cmap, e + 8 + 4) as usize {
                        return None;
                    }
                }
            }
            return Some(offset);
        }
    }
    None
}

/// `CheckCmapSubtable4`.
pub(crate) fn check_cmap_subtable4(cmap: &[u8], st: Option<usize>) -> bool {
    let Some(st) = st else { return false };
    let table_len = cmap.len().wrapping_sub(st);
    if table_len < 6 || u16_at(cmap, st) != 4 {
        return false;
    }
    if table_len < SIZEOF_CMAP4 {
        return false;
    }
    let length = usize::from(u16_at(cmap, st + 2));
    if length > table_len || length < SIZEOF_CMAP4 {
        return false;
    }
    let n_ranges = usize::from(u16_at(cmap, st + 6) >> 1);
    if n_ranges == 0 || length < SIZEOF_CMAP4 + 4 * n_ranges * 2 {
        return false;
    }
    u16_at(cmap, st + 14 + 2 * (n_ranges - 1)) == 0xFFFF
}

/// `CmapSubtable4Lookup`.
pub(crate) fn cmap_subtable4_lookup(cmap: &[u8], st: usize, uid: u32, range_key: i32) -> u16 {
    let n_seg = usize::from(u16_at(cmap, st + 6) >> 1);
    let end = st + 14; // end_code[0]
    let mut mid: usize; // index into end_code
    let ch_end: u32;
    if range_key != 0 {
        mid = range_key as usize;
        ch_end = u32::from(u16_at(cmap, end + 2 * mid));
    } else {
        let mut left = 0usize;
        let mut n = n_seg;
        let mut ce = 0u32;
        mid = 0;
        while n > 0 {
            let c_mid = n >> 1;
            mid = left + c_mid;
            ce = u32::from(u16_at(cmap, end + 2 * mid));
            if uid <= ce {
                if c_mid == 0 || uid > u32::from(u16_at(cmap, end + 2 * (mid - 1))) {
                    break;
                }
                n = c_mid;
            } else {
                left = mid + 1;
                n -= c_mid + 1;
            }
        }
        if n == 0 {
            return 0;
        }
        ch_end = ce;
    }
    mid += n_seg + 1;
    let ch_start = u32::from(u16_at(cmap, end + 2 * mid));
    if ch_end >= uid && uid >= ch_start {
        mid += n_seg;
        let id_delta = u16_at(cmap, end + 2 * mid) as i16;
        mid += n_seg;
        let id_range_offset = u16_at(cmap, end + 2 * mid);
        if id_range_offset == 0 {
            return (i32::from(id_delta) + uid as i32) as u16;
        }
        // (in uint16 units from the subtable's start)
        let offset = (uid - ch_start) as usize + usize::from(id_range_offset >> 1) + (7 + mid);
        if offset * 2 + 1 >= usize::from(u16_at(cmap, st + 2)) {
            return 0;
        }
        let g = u16_at(cmap, st + 2 * offset);
        return if g != 0 {
            g.wrapping_add(id_delta as u16)
        } else {
            0
        };
    }
    0
}

/// `CmapSubtable4NextCodepoint`.
pub(crate) fn cmap_subtable4_next(cmap: &[u8], st: usize, uid: u32, key: &mut i32) -> u32 {
    let n_range = i32::from(u16_at(cmap, st + 6) >> 1);
    let end = st + 14;
    let start = end + 2 * n_range as usize + 2;
    let mut prev = uid;
    if prev == 0 {
        *key = 0;
        return u32::from(u16_at(cmap, start));
    } else if prev >= 0xFFFF {
        *key = n_range - 1;
        return 0xFFFF;
    }
    let mut i = *key;
    while i > 0 && u32::from(u16_at(cmap, start + 2 * i as usize)) > prev {
        i -= 1;
    }
    while i < n_range - 1 && u32::from(u16_at(cmap, end + 2 * i as usize)) < prev {
        i += 1;
    }
    let s = u32::from(u16_at(cmap, start + 2 * i as usize));
    let e = u32::from(u16_at(cmap, end + 2 * i as usize));
    if s > prev {
        prev = s.wrapping_sub(1);
    }
    if e > prev {
        *key = i;
        return prev + 1;
    }
    *key = i + 1;
    if i + 1 >= n_range {
        0xFFFF
    } else {
        u32::from(u16_at(cmap, start + 2 * (i + 1) as usize))
    }
}

/// `CheckCmapSubtable12`.
pub(crate) fn check_cmap_subtable12(cmap: &[u8], st: Option<usize>) -> bool {
    let Some(st) = st else { return false };
    let table_len = cmap.len().wrapping_sub(st);
    if table_len < 6 || u16_at(cmap, st) != 12 {
        return false;
    }
    if table_len < SIZEOF_CMAP12 {
        return false;
    }
    let length = u32_at(cmap, st + 4) as usize;
    if length > table_len || length < SIZEOF_CMAP12 {
        return false;
    }
    let num_groups = u32_at(cmap, st + 12) as usize;
    !(num_groups > 0x1000_0000
        || length != SIZEOF_CMAP12 + num_groups.wrapping_sub(1).wrapping_mul(12))
}

/// `CmapSubtable12Lookup`.
pub(crate) fn cmap_subtable12_lookup(cmap: &[u8], st: usize, uid: u32, range_key: i32) -> u16 {
    let groups = u32_at(cmap, st + 12);
    let mut i = range_key as u32;
    while i < groups {
        let g = st + 16 + 12 * i as usize;
        let s = u32_at(cmap, g);
        let e = u32_at(cmap, g + 4);
        if uid >= s && uid <= e {
            return u32_at(cmap, g + 8).wrapping_add(uid - s) as u16;
        }
        i += 1;
    }
    0
}

/// `CmapSubtable12NextCodepoint`.
pub(crate) fn cmap_subtable12_next(cmap: &[u8], st: usize, uid: u32, key: &mut i32) -> u32 {
    let n_range = u32_at(cmap, st + 12) as i32;
    let grp = |i: i32, f: usize| u32_at(cmap, st + 16 + 12 * i as usize + f);
    let mut prev = uid;
    if prev == 0 {
        *key = 0;
        return grp(0, 0);
    } else if prev >= 0x10FFFF {
        *key = n_range;
        return 0x10FFFF;
    }
    let mut i = *key;
    while i > 0 && grp(i, 0) > prev {
        i -= 1;
    }
    while i < n_range - 1 && grp(i, 4) < prev {
        i += 1;
    }
    let s = grp(i, 0);
    let e = grp(i, 4);
    if s > prev {
        prev = s.wrapping_sub(1);
    }
    if e > prev {
        *key = i;
        return prev.wrapping_add(1);
    }
    *key = i + 1;
    if i + 1 >= n_range {
        0x10FFFF
    } else {
        grp(i + 1, 0)
    }
}

/// `LocaLookup`: the glyph's offset in `glyf`, `usize::MAX` for an empty
/// glyph, `usize::MAX - 1` for a bad glyph id.
pub(crate) fn loca_lookup(gid: u16, loca: &[u8], head: &[u8]) -> usize {
    let n = loca.len();
    let g = usize::from(gid);
    let mut res = usize::MAX - 1;
    match i16_at(head, 50) {
        0 => {
            if n > 1 && g + 1 < n >> 1 {
                res = usize::from(u16_at(loca, 2 * g)) << 1;
                if res == usize::from(u16_at(loca, 2 * g + 2)) << 1 {
                    return usize::MAX;
                }
            }
        }
        1 => {
            if n > 3 && g + 1 < n >> 2 {
                res = u32_at(loca, 4 * g) as usize;
                if res == u32_at(loca, 4 * g + 4) as usize {
                    return usize::MAX;
                }
            }
        }
        _ => {}
    }
    res
}

/// `GlyfLookup` + `GlyfBox`: the glyph's bounding box, if its entry fits.
pub(crate) fn glyf_box(glyf: &[u8], offset: usize) -> Option<(i32, i32, i32, i32)> {
    if offset >= glyf.len().wrapping_sub(SIZEOF_GLYPH) {
        return None;
    }
    Some((
        i32::from(i16_at(glyf, offset + 2)),
        i32::from(i16_at(glyf, offset + 4)),
        i32::from(i16_at(glyf, offset + 6)),
        i32::from(i16_at(glyf, offset + 8)),
    ))
}
