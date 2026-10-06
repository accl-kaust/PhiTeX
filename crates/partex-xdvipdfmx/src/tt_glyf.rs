//! tt_glyf.c, tt_glyf.h: subsetting the glyf/loca/hmtx tables.

use core::cmp::Ordering;

use crate::prelude::*;
use crate::sfnt::{
    BYTE, SFNT_TYPE_DFONT, SFNT_TYPE_TRUETYPE, SFNT_TYPE_TTC, SHORT, Sfnt, ULONG, USHORT,
    sfnt_put_short, sfnt_put_ulong, sfnt_put_ushort,
};
use crate::tt_table::{TtHeadTable, TtHheaTable, TtLongMetrics, TtMaxpTable};

/// `NUM_GLYPH_LIMIT`.
pub const NUM_GLYPH_LIMIT: u32 = 65534;
/// `TABLE_DATA_ALLOC_SIZE`.
pub const TABLE_DATA_ALLOC_SIZE: u32 = 40960;
/// `GLYPH_ARRAY_ALLOC_SIZE`.
pub const GLYPH_ARRAY_ALLOC_SIZE: u16 = 256;

/// Composite glyph flags (tt_glyf.c).
pub const ARG_1_AND_2_ARE_WORDS: u16 = 1 << 0;
pub const ARGS_ARE_XY_VALUES: u16 = 1 << 1;
pub const ROUND_XY_TO_GRID: u16 = 1 << 2;
pub const WE_HAVE_A_SCALE: u16 = 1 << 3;
pub const RESERVED: u16 = 1 << 4;
pub const MORE_COMPONENT: u16 = 1 << 5;
pub const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 1 << 6;
pub const WE_HAVE_A_TWO_BY_TWO: u16 = 1 << 7;
pub const WE_HAVE_INSTRUCTIONS: u16 = 1 << 8;
pub const USE_MY_METRICS: u16 = 1 << 9;

/// `struct tt_glyph_desc`.
#[derive(Clone, Debug, Default)]
pub struct TtGlyphDesc {
    pub gid: USHORT,
    /// GID in the original font.
    pub ogid: USHORT,
    pub advw: USHORT,
    pub advh: USHORT,
    pub lsb: SHORT,
    pub tsb: SHORT,
    pub llx: SHORT,
    pub lly: SHORT,
    pub urx: SHORT,
    pub ury: SHORT,
    pub length: ULONG,
    /// The glyph's glyf data (C's NULL is empty).
    pub data: Vec<BYTE>,
}

/// `struct tt_glyphs`: `gd` holds the `num_glyphs` glyphs (`max_glyphs`
/// is kept as C counts it).
#[derive(Clone, Debug, Default)]
pub struct TtGlyphs {
    pub num_glyphs: USHORT,
    pub max_glyphs: USHORT,
    pub last_gid: USHORT,
    pub emsize: USHORT,
    /// Optimal value for DW.
    pub dw: USHORT,
    /// Default value.
    pub default_advh: USHORT,
    /// Default value.
    pub default_tsb: SHORT,
    pub gd: Vec<TtGlyphDesc>,
    /// 8192 bytes: a bit per gid, MSB first.
    pub used_slot: Vec<u8>,
}

/// `glyf_cmp` (static): the `qsort` order of glyph descriptions.
fn glyf_cmp(v1: &TtGlyphDesc, v2: &TtGlyphDesc) -> Ordering {
    if v1.gid == v2.gid {
        Ordering::Equal
    } else if v1.gid < v2.gid {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

impl TtGlyphs {
    /// `tt_build_init`: glyph 0 already added.
    #[must_use]
    pub fn tt_build_init() -> Result<TtGlyphs> {
        let mut g = TtGlyphs {
            num_glyphs: 0,
            max_glyphs: 0,
            last_gid: 0,
            emsize: 1,
            dw: 0,
            default_advh: 0,
            default_tsb: 0,
            gd: Vec::new(),
            used_slot: vec![0u8; 8192],
        };
        g.tt_add_glyph(0, 0)?;
        Ok(g)
    }
    /// `tt_build_finish`.
    pub fn tt_build_finish(self) {}
    /// `find_empty_slot` (static).
    fn find_empty_slot(&self) -> Result<USHORT> {
        let mut gid: u32 = 0;
        while gid < NUM_GLYPH_LIMIT {
            if self.used_slot[(gid / 8) as usize] & (1 << (7 - (gid % 8))) == 0 {
                break;
            }
            gid += 1;
        }
        if gid == NUM_GLYPH_LIMIT {
            fatal!("No empty glyph slot available.");
        }
        Ok(gid as USHORT)
    }
    /// `tt_add_glyph`: returns `new_gid`.
    pub fn tt_add_glyph(&mut self, gid: USHORT, new_gid: USHORT) -> Result<USHORT> {
        if self.used_slot[(new_gid / 8) as usize] & (1 << (7 - (new_gid % 8))) != 0 {
            warn!("Slot {} already used.", new_gid);
        } else {
            if u32::from(self.num_glyphs) + 1 >= NUM_GLYPH_LIMIT {
                fatal!("Too many glyphs.");
            }
            if self.num_glyphs >= self.max_glyphs {
                self.max_glyphs = self.max_glyphs.wrapping_add(GLYPH_ARRAY_ALLOC_SIZE);
            }
            self.gd.push(TtGlyphDesc {
                gid: new_gid,
                ogid: gid,
                length: 0,
                data: Vec::new(),
                ..TtGlyphDesc::default()
            });
            self.used_slot[(new_gid / 8) as usize] |= 1 << (7 - (new_gid % 8));
            self.num_glyphs += 1;
        }
        if new_gid > self.last_gid {
            self.last_gid = new_gid;
        }
        Ok(new_gid)
    }
    /// `tt_get_index`: the index in `gd` of new gid `gid` (0 if absent).
    #[must_use]
    pub fn tt_get_index(&self, gid: USHORT) -> USHORT {
        let mut idx: USHORT = 0;
        while idx < self.num_glyphs {
            if gid == self.gd[idx as usize].gid {
                break;
            }
            idx += 1;
        }
        if idx == self.num_glyphs {
            idx = 0;
        }
        idx
    }
    /// `tt_find_glyph`: the new gid of original gid `gid` (0 if absent).
    #[must_use]
    pub fn tt_find_glyph(&self, gid: USHORT) -> USHORT {
        for d in &self.gd[..self.num_glyphs as usize] {
            if gid == d.ogid {
                return d.gid;
            }
        }
        0
    }
}

/// `hmtx`, `vmtx` (or none), `loca` of the font, read as tt_build_tables
/// and tt_get_metrics both do; also sets `emsize`, `default_advh`,
/// `default_tsb`.
#[allow(clippy::type_complexity)]
fn read_metrics(
    sfont: &mut Sfnt,
    g: &mut TtGlyphs,
) -> Result<(
    TtHeadTable,
    TtHheaTable,
    TtMaxpTable,
    Vec<TtLongMetrics>,
    Option<Vec<TtLongMetrics>>,
    Vec<ULONG>,
)> {
    // unitsPerEm --> head, numHMetrics --> hhea, indexToLocFormat -->
    // head, numGlyphs --> maxp.
    let head = sfont.tt_read_head_table()?;
    let hhea = sfont.tt_read_hhea_table()?;
    let maxp = sfont.tt_read_maxp_table()?;

    if hhea.metric_data_format != 0 {
        fatal!("Unknown metricDataFormat.");
    }

    g.emsize = head.units_per_em;

    sfont.sfnt_locate_table(b"hmtx")?;
    let hmtx = sfont.tt_read_longMetrics(
        maxp.num_glyphs,
        hhea.num_of_long_hor_metrics,
        hhea.num_of_ex_side_bearings,
    )?;

    let os2 = sfont.tt_read_os2__table()?;
    g.default_advh = (i32::from(os2.s_typo_ascender) - i32::from(os2.s_typo_descender)) as USHORT;
    g.default_tsb = (i32::from(g.default_advh) - i32::from(os2.s_typo_ascender)) as SHORT;

    let vmtx = if sfont.sfnt_find_table_pos(b"vmtx") > 0 {
        let vhea = sfont.tt_read_vhea_table()?;
        sfont.sfnt_locate_table(b"vmtx")?;
        Some(sfont.tt_read_longMetrics(
            maxp.num_glyphs,
            vhea.num_of_long_ver_metrics,
            vhea.num_of_ex_side_bearings,
        )?)
    } else {
        None
    };

    sfont.sfnt_locate_table(b"loca")?;
    let n = maxp.num_glyphs as usize + 1;
    let mut location: Vec<ULONG> = Vec::with_capacity(n);
    if head.index_to_loc_format == 0 {
        for _ in 0..n {
            location.push(2 * ULONG::from(sfont.sfnt_get_ushort()?));
        }
    } else if head.index_to_loc_format == 1 {
        for _ in 0..n {
            location.push(sfont.sfnt_get_ulong()?);
        }
    } else {
        fatal!("Unknown IndexToLocFormat.");
    }

    Ok((head, hhea, maxp, hmtx, vmtx, location))
}

/// The metrics of glyph `i` of `g` (original gid `gid`), as both
/// readers set them; the glyph's data length.
fn set_glyph_metrics(
    g: &mut TtGlyphs,
    i: usize,
    gid: USHORT,
    hmtx: &[TtLongMetrics],
    vmtx: Option<&[TtLongMetrics]>,
    location: &[ULONG],
    w_stat: &mut [u16],
) -> ULONG {
    let loc = location[gid as usize];
    let len = location[gid as usize + 1].wrapping_sub(loc);
    let (default_advh, default_tsb, emsize) = (g.default_advh, g.default_tsb, g.emsize);
    let d = &mut g.gd[i];
    d.advw = hmtx[gid as usize].advance;
    d.lsb = hmtx[gid as usize].side_bearing;
    if let Some(vmtx) = vmtx {
        d.advh = vmtx[gid as usize].advance;
        d.tsb = vmtx[gid as usize].side_bearing;
    } else {
        d.advh = default_advh;
        d.tsb = default_tsb;
    }
    d.length = len;
    d.data = Vec::new();
    if d.advw <= emsize {
        w_stat[d.advw as usize] = w_stat[d.advw as usize].wrapping_add(1);
    } else {
        // Larger than em.
        w_stat[emsize as usize + 1] = w_stat[emsize as usize + 1].wrapping_add(1);
    }
    len
}

/// The most frequent advance width, as both readers find it.
fn set_dw(g: &mut TtGlyphs, w_stat: &[u16]) {
    let mut max_count: i32 = -1;
    g.dw = g.gd[0].advw;
    for i in 0..(i32::from(g.emsize) + 1) as usize {
        if i32::from(w_stat[i]) > max_count {
            max_count = i32::from(w_stat[i]);
            g.dw = i as USHORT;
        }
    }
}

impl Sfnt {
    /// `tt_build_tables`: replaces glyf, loca, hmtx, hhea, maxp, head
    /// with the subset ones (`sfnt_set_table`); 0 (errors are ERROR).
    pub fn tt_build_tables(&mut self, g: &mut TtGlyphs) -> Result<i32> {
        if self.type_ != SFNT_TYPE_TRUETYPE
            && self.type_ != SFNT_TYPE_TTC
            && self.type_ != SFNT_TYPE_DFONT
        {
            fatal!("Invalid font type");
        }

        if u32::from(g.num_glyphs) > NUM_GLYPH_LIMIT {
            fatal!("Too many glyphs.");
        }

        let (mut head, mut hhea, mut maxp, hmtx, vmtx, location) = read_metrics(self, g)?;

        let mut w_stat = vec![0u16; g.emsize as usize + 2];
        // Read glyf table.
        let offset = self.sfnt_locate_table(b"glyf")?;
        // The num_glyphs may grow when composite glyph is found. A
        // component of a composite glyph is appended to the used glyphs if
        // not already there, and the composite's program is changed to
        // refer to the new gid of its components.
        let mut i: u32 = 0;
        while i < NUM_GLYPH_LIMIT {
            if i >= u32::from(g.num_glyphs) {
                // Finished.
                break;
            }
            let iu = i as usize;
            i += 1;

            let gid = g.gd[iu].ogid; // old gid
            if gid >= maxp.num_glyphs {
                fatal!("Invalid glyph index (gid {})", gid);
            }

            let len = set_glyph_metrics(g, iu, gid, &hmtx, vmtx.as_deref(), &location, &mut w_stat);
            let loc = location[gid as usize];

            if len == 0 {
                // Does not contain any data.
                continue;
            } else if len < 10 {
                fatal!("Invalid TrueType glyph data (gid {}).", gid);
            }

            let mut data = vec![0u8; len as usize];
            let endptr = len as usize;
            let mut p = 0usize;

            self.sfnt_seek_set(offset.wrapping_add(loc));
            let number_of_contours = self.sfnt_get_short()?;
            p += sfnt_put_short(&mut data[p..], number_of_contours) as usize;

            // BoundingBox: FWord x 4.
            g.gd[iu].llx = self.sfnt_get_short()?;
            g.gd[iu].lly = self.sfnt_get_short()?;
            g.gd[iu].urx = self.sfnt_get_short()?;
            g.gd[iu].ury = self.sfnt_get_short()?;
            if vmtx.is_none() {
                // vertOriginY == sTypeAscender
                g.gd[iu].tsb = (i32::from(g.default_advh)
                    - i32::from(g.default_tsb)
                    - i32::from(g.gd[iu].ury)) as SHORT;
            }
            p += sfnt_put_short(&mut data[p..], g.gd[iu].llx) as usize;
            p += sfnt_put_short(&mut data[p..], g.gd[iu].lly) as usize;
            p += sfnt_put_short(&mut data[p..], g.gd[iu].urx) as usize;
            p += sfnt_put_short(&mut data[p..], g.gd[iu].ury) as usize;

            // Read everything else.
            self.sfnt_read(&mut data[p..]);
            // Fix GIDs of composite glyphs.
            if number_of_contours < 0 {
                loop {
                    if p >= endptr {
                        fatal!("Invalid TrueType glyph data (gid {}): {} bytes", gid, len);
                    }
                    // Flags and gid of the component glyph are both USHORT.
                    let flags = (u16::from(data[p]) << 8) | u16::from(data[p + 1]);
                    p += 2;
                    let cgid = (u16::from(data[p]) << 8) | u16::from(data[p + 1]);
                    if cgid >= maxp.num_glyphs {
                        fatal!(
                            "Invalid gid ({} > {}) in composite glyph {}.",
                            cgid,
                            maxp.num_glyphs,
                            gid
                        );
                    }
                    let mut new_gid = g.tt_find_glyph(cgid);
                    if new_gid == 0 {
                        let slot = g.find_empty_slot()?;
                        new_gid = g.tt_add_glyph(cgid, slot)?;
                    }
                    p += sfnt_put_ushort(&mut data[p..], new_gid) as usize;
                    // Just skip the rest.
                    p += if flags & ARG_1_AND_2_ARE_WORDS != 0 {
                        4
                    } else {
                        2
                    };
                    if flags & WE_HAVE_A_SCALE != 0 {
                        // F2Dot14
                        p += 2;
                    } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                        // F2Dot14 x 2
                        p += 4;
                    } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                        // F2Dot14 x 4
                        p += 8;
                    }
                    if flags & MORE_COMPONENT == 0 {
                        break;
                    }
                }
                // TrueType instructions come here.
            }
            g.gd[iu].data = data;
        }
        drop(location);
        drop(hmtx);
        drop(vmtx);

        set_dw(g, &w_stat);
        drop(w_stat);

        let n = g.num_glyphs as usize;
        g.gd[..n].sort_by(glyf_cmp);
        let (hmtx_table_data, loca_table_data, glyf_table_data) = {
            let mut glyf_table_size: ULONG = 0;
            let mut num_hm_known = false;
            let last_advw = g.gd[n - 1].advw;
            for i in (0..n).rev() {
                let d = &g.gd[i];
                let padlen = if d.length % 4 != 0 {
                    4 - (d.length % 4)
                } else {
                    0
                };
                glyf_table_size = glyf_table_size.wrapping_add(d.length.wrapping_add(padlen));
                if !num_hm_known && last_advw != d.advw {
                    hhea.num_of_long_hor_metrics = d.gid.wrapping_add(2);
                    num_hm_known = true;
                }
            }
            // All advance widths are the same.
            if !num_hm_known {
                hhea.num_of_long_hor_metrics = 1;
            }
            let hmtx_table_size = (i32::from(hhea.num_of_long_hor_metrics) * 2
                + (i32::from(g.last_gid) + 1) * 2) as ULONG;

            // Choosing short format does not always give good result when
            // compressed. Sometimes increases size.
            let loca_table_size: ULONG = if glyf_table_size < 0x20000 {
                head.index_to_loc_format = 0;
                ((i32::from(g.last_gid) + 2) * 2) as ULONG
            } else {
                head.index_to_loc_format = 1;
                ((i32::from(g.last_gid) + 2) * 4) as ULONG
            };

            let mut hmtx_data = vec![0u8; hmtx_table_size as usize];
            let mut loca_data = vec![0u8; loca_table_size as usize];
            let mut glyf_data = vec![0u8; glyf_table_size as usize];
            let mut p = 0usize;
            let mut q = 0usize;

            let mut offset: ULONG = 0;
            let mut prev: USHORT = 0;
            let num_long = i32::from(hhea.num_of_long_hor_metrics);
            for i in 0..n {
                let gap = i32::from(g.gd[i].gid) - i32::from(prev) - 1;
                for j in 1..=gap {
                    if i32::from(prev) + j == num_long - 1 {
                        p += sfnt_put_ushort(&mut hmtx_data[p..], last_advw) as usize;
                    } else if i32::from(prev) + j < num_long {
                        p += sfnt_put_ushort(&mut hmtx_data[p..], 0) as usize;
                    }
                    p += sfnt_put_short(&mut hmtx_data[p..], 0) as usize;
                    if head.index_to_loc_format == 0 {
                        q += sfnt_put_ushort(&mut loca_data[q..], (offset / 2) as USHORT) as usize;
                    } else {
                        q += sfnt_put_ulong(&mut loca_data[q..], offset) as usize;
                    }
                }
                let d = &mut g.gd[i];
                let padlen = if d.length % 4 != 0 {
                    4 - (d.length % 4)
                } else {
                    0
                };
                if i32::from(d.gid) < num_long {
                    p += sfnt_put_ushort(&mut hmtx_data[p..], d.advw) as usize;
                }
                p += sfnt_put_short(&mut hmtx_data[p..], d.lsb) as usize;
                if head.index_to_loc_format == 0 {
                    q += sfnt_put_ushort(&mut loca_data[q..], (offset / 2) as USHORT) as usize;
                } else {
                    q += sfnt_put_ulong(&mut loca_data[q..], offset) as usize;
                }
                let o = offset as usize;
                let l = d.length as usize;
                glyf_data[o..o + l + padlen as usize].fill(0);
                glyf_data[o..o + l].copy_from_slice(&d.data[..l]);
                offset = offset.wrapping_add(d.length.wrapping_add(padlen));
                prev = d.gid;
                // Free data here since it consumes much memory.
                d.data = Vec::new();
                d.length = 0;
            }
            if head.index_to_loc_format == 0 {
                sfnt_put_ushort(&mut loca_data[q..], (offset / 2) as USHORT);
            } else {
                sfnt_put_ulong(&mut loca_data[q..], offset);
            }
            (hmtx_data, loca_data, glyf_data)
        };
        self.sfnt_set_table(b"hmtx", hmtx_table_data);
        self.sfnt_set_table(b"loca", loca_table_data);
        self.sfnt_set_table(b"glyf", glyf_table_data);

        head.check_sum_adjustment = 0;
        maxp.num_glyphs = g.last_gid.wrapping_add(1);

        self.sfnt_set_table(b"maxp", maxp.tt_pack_maxp_table());
        self.sfnt_set_table(b"hhea", hhea.tt_pack_hhea_table());
        self.sfnt_set_table(b"head", head.tt_pack_head_table());

        Ok(0)
    }
    /// `tt_get_metrics`: fills the metrics of `g` without building
    /// tables (CFF-based OpenType); 0.
    pub fn tt_get_metrics(&mut self, g: &mut TtGlyphs) -> Result<i32> {
        if self.type_ != SFNT_TYPE_TRUETYPE
            && self.type_ != SFNT_TYPE_TTC
            && self.type_ != SFNT_TYPE_DFONT
        {
            fatal!("Invalid font type");
        }

        let (_head, _hhea, maxp, hmtx, vmtx, location) = read_metrics(self, g)?;

        let mut w_stat = vec![0u16; g.emsize as usize + 2];
        // Read glyf table.
        let offset = self.sfnt_locate_table(b"glyf")?;
        for i in 0..g.num_glyphs as usize {
            let gid = g.gd[i].ogid; // old gid
            if gid >= maxp.num_glyphs {
                fatal!("Invalid glyph index (gid {})", gid);
            }

            let len = set_glyph_metrics(g, i, gid, &hmtx, vmtx.as_deref(), &location, &mut w_stat);
            let loc = location[gid as usize];

            if len == 0 {
                // Does not contain any data.
                continue;
            } else if len < 10 {
                fatal!("Invalid TrueType glyph data (gid {}).", gid);
            }

            self.sfnt_seek_set(offset.wrapping_add(loc));
            let _ = self.sfnt_get_short()?;

            // BoundingBox: FWord x 4.
            g.gd[i].llx = self.sfnt_get_short()?;
            g.gd[i].lly = self.sfnt_get_short()?;
            g.gd[i].urx = self.sfnt_get_short()?;
            g.gd[i].ury = self.sfnt_get_short()?;
            if vmtx.is_none() {
                // vertOriginY == sTypeAscender
                g.gd[i].tsb = (i32::from(g.default_advh)
                    - i32::from(g.default_tsb)
                    - i32::from(g.gd[i].ury)) as SHORT;
            }
        }

        set_dw(g, &w_stat);

        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_find() {
        let mut g = TtGlyphs::tt_build_init().unwrap();
        assert_eq!(g.num_glyphs, 1);
        assert_eq!(g.tt_add_glyph(37, 5).unwrap(), 5);
        assert_eq!(g.tt_add_glyph(38, 5).unwrap(), 5); // slot used: not added
        assert_eq!(g.num_glyphs, 2);
        assert_eq!(g.last_gid, 5);
        assert_eq!(g.tt_find_glyph(37), 5);
        assert_eq!(g.tt_find_glyph(38), 0);
        assert_eq!(g.tt_get_index(5), 1);
        assert_eq!(g.tt_get_index(6), 0);
        assert_eq!(g.find_empty_slot().unwrap(), 1);
        assert_eq!(g.max_glyphs, 256);
    }
}

/// Subsets of a TeX Live font against xdvipdfmx's own code (a C harness
/// over sfnt.c, tt_table.c, tt_glyf.c): `PARTEX_XPDF_DUMP=dir` writes the
/// FontFile2 bytes and the glyph metrics for comparison.
#[cfg(test)]
mod font_tests {
    extern crate std;

    use super::*;
    use crate::ctx::{Dpx, DpxConf};
    use crate::io::{Files, Format};
    use crate::obj::PdfOut;
    use alloc::sync::Arc;
    use core::fmt::Write as _;

    struct NoFiles;
    impl Files for NoFiles {
        fn find(&mut self, _: &[u8], _: Format, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
        fn read(&mut self, _: &[u8]) -> Option<Arc<[u8]>> {
            None
        }
    }

    pub(crate) fn test_dpx() -> Dpx {
        Dpx::new(Box::new(NoFiles), Box::new(|_, d: &[u8]| d.to_vec()))
    }

    /// The FontFile2 of `gids` (`identity`: new gid = gid, as cidtype2;
    /// else 1, 2, …, as truetype), its Length1 and a metrics dump.
    fn subset(path: &str, identity: bool, gids: &[u16]) -> Option<(Vec<u8>, String)> {
        let data = std::fs::read(path).ok()?;
        let mut sfont = Sfnt::sfnt_open(MemFile::new(Arc::from(data), path.as_bytes())).unwrap()?;
        sfont.sfnt_read_table_directory(0).unwrap();
        let mut g = TtGlyphs::tt_build_init().unwrap();
        for (i, &gid) in gids.iter().enumerate() {
            g.tt_add_glyph(gid, if identity { gid } else { i as u16 + 1 })
                .unwrap();
        }
        sfont.tt_build_tables(&mut g).unwrap();
        let mut dump = String::new();
        let _ = writeln!(
            dump,
            "num_glyphs {} last_gid {} dw {} emsize {} advh {} tsb {}",
            g.num_glyphs, g.last_gid, g.dw, g.emsize, g.default_advh, g.default_tsb
        );
        for d in &g.gd {
            let _ = writeln!(
                dump,
                "gd {} {} {} {} {} {}",
                d.gid, d.ogid, d.advw, d.lsb, d.tsb, d.ury
            );
        }
        for (tag, must) in crate::cidtype2::REQUIRED_TABLE {
            sfont.sfnt_require_table(*tag, i32::from(*must));
        }
        let mut dpx = test_dpx();
        let s = dpx.sfnt_create_FontFile_stream(&mut sfont).unwrap()?;
        let dict = dpx.o.stream_dict(s).unwrap();
        let l1 = dpx.o.lookup_dict(dict, b"Length1").unwrap().unwrap();
        let _ = writeln!(dump, "/Length1 {}", dpx.o.number_value(l1).unwrap());
        Some((dpx.o.stream_data(s).unwrap().to_vec(), dump))
    }

    const DEJAVU: &str = "/usr/share/texmf-dist/fonts/truetype/public/dejavu/DejaVuSans.ttf";

    /// The md5 of the DejaVuSans.ttf the hashes below were made from.
    const FONT_MD5: [(&str, &str); 3] = [
        (DEJAVU, "b0e31de57cd5307954a3c54136ce68ae"),
        (SPECTRAL, "7b78ff83168097bf78ed628b3ed15d9c"),
        (AMIRI, "bca12f1468d2ff8ed1512e7f6e73fb16"),
    ];
    const SPECTRAL: &str =
        "/usr/share/texmf-dist/fonts/truetype/production/spectral/Spectral-Regular.ttf";
    const AMIRI: &str = "/usr/share/texmf-dist/fonts/truetype/public/amiri/Amiri-BoldItalic.ttf";

    /// The md5 of what the C code writes for each case.
    const C_MD5: [(&str, &str); 5] = [
        ("dejavu-id", "939eac881233a8a258ce5abb7ce2de8b"),
        ("dejavu-seq", "c4fe04cef1832ca3a0e422e506236940"),
        ("dejavu-big", "49f5c9fe1006d21e1bd5bb45d7384766"),
        ("spectral-id", "2e8868aade2ef30cee1d94281f4359d5"),
        ("amiri-seq", "7e3d71b4a584a121c06103591feab6bb"),
    ];

    fn hex(d: &[u8]) -> String {
        let mut s = String::new();
        for b in d {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    fn cases() -> Vec<(&'static str, &'static str, bool, Vec<u16>)> {
        let some: Vec<u16> = vec![
            3, 36, 37, 38, 68, 69, 70, 100, 101, 150, 151, 152, 200, 201, 202, 500, 600, 1000,
        ];
        vec![
            ("dejavu-id", DEJAVU, true, some.clone()),
            ("dejavu-seq", DEJAVU, false, some),
            ("dejavu-big", DEJAVU, true, (1..3000).collect()),
            ("spectral-id", SPECTRAL, true, (1..300).collect()),
            ("amiri-seq", AMIRI, false, (100..1300).step_by(3).collect()),
        ]
    }

    #[test]
    fn subsets() {
        let dir = std::env::var("PARTEX_XPDF_DUMP").ok();
        for (name, path, identity, gids) in cases() {
            let Some((bytes, dump)) = subset(path, identity, &gids) else {
                continue;
            };
            assert_eq!(&bytes[..4], &[0, 1, 0, 0]);
            if let Some(dir) = dir.as_deref() {
                std::fs::write(std::format!("{dir}/{name}.bin"), &bytes).unwrap();
                std::fs::write(std::format!("{dir}/{name}.txt"), dump).unwrap();
                let args: Vec<String> = gids.iter().map(|g| std::format!("{g}")).collect();
                std::fs::write(
                    std::format!("{dir}/{name}.args"),
                    std::format!(
                        "{path} {} {}",
                        if identity { "i" } else { "s" },
                        args.join(" ")
                    ),
                )
                .unwrap();
            }
            let font = std::fs::read(path).unwrap();
            let font_md5 = FONT_MD5.iter().find(|f| f.0 == path).unwrap().1;
            if hex(&partex_engine::md5::md5(&font)) == font_md5 {
                if let Some(&(_, want)) = C_MD5.iter().find(|c| c.0 == name) {
                    assert_eq!(hex(&partex_engine::md5::md5(&bytes)), want, "{name}");
                }
            }
        }
    }
}
