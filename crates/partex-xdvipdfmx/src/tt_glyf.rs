//! tt_glyf.c, tt_glyf.h: subsetting the glyf/loca/hmtx tables.

use core::cmp::Ordering;

use crate::prelude::*;
use crate::sfnt::{BYTE, SHORT, Sfnt, ULONG, USHORT};

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
    todo!()
}

impl TtGlyphs {
    /// `tt_build_init`: glyph 0 already added.
    #[must_use]
    pub fn tt_build_init() -> TtGlyphs {
        todo!()
    }
    /// `tt_build_finish`.
    pub fn tt_build_finish(self) {}
    /// `find_empty_slot` (static).
    fn find_empty_slot(&self) -> USHORT {
        todo!()
    }
    /// `tt_add_glyph`: returns `new_gid`.
    pub fn tt_add_glyph(&mut self, gid: USHORT, new_gid: USHORT) -> USHORT {
        todo!()
    }
    /// `tt_get_index`: the index in `gd` of new gid `gid` (0 if absent).
    #[must_use]
    pub fn tt_get_index(&self, gid: USHORT) -> USHORT {
        todo!()
    }
    /// `tt_find_glyph`: the new gid of original gid `gid` (0 if absent).
    #[must_use]
    pub fn tt_find_glyph(&self, gid: USHORT) -> USHORT {
        todo!()
    }
}

impl Sfnt {
    /// `tt_build_tables`: replaces glyf, loca, hmtx, hhea, maxp, head
    /// with the subset ones (`sfnt_set_table`); 0 (errors are ERROR).
    pub fn tt_build_tables(&mut self, g: &mut TtGlyphs) -> i32 {
        todo!()
    }
    /// `tt_get_metrics`: fills the metrics of `g` without building
    /// tables (CFF-based OpenType); 0.
    pub fn tt_get_metrics(&mut self, g: &mut TtGlyphs) -> i32 {
        todo!()
    }
}
