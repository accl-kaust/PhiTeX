//! tt_table.c, tt_table.h: the head, hhea, vhea, maxp, VORG, OS/2, hmtx
//! and name tables.
//!
//! Readers are `impl Sfnt` methods returning the table by value (C's
//! `NEW`ed pointer; `tt_read_VORG_table`'s NULL is `None`). Packers are
//! methods of the table returning the packed bytes.

use crate::prelude::*;
use crate::sfnt::{BYTE, CHAR, FWord, Fixed, SHORT, Sfnt, UFWord, ULONG, USHORT};

/// `TT_HEAD_TABLE_SIZE`.
pub const TT_HEAD_TABLE_SIZE: usize = 54;
/// `TT_MAXP_TABLE_SIZE`.
pub const TT_MAXP_TABLE_SIZE: usize = 32;
/// `TT_HHEA_TABLE_SIZE`.
pub const TT_HHEA_TABLE_SIZE: usize = 36;
/// `TT_VHEA_TABLE_SIZE`.
pub const TT_VHEA_TABLE_SIZE: usize = 36;

/// `struct tt_head_table`.
#[derive(Clone, Debug, Default)]
pub struct TtHeadTable {
    pub version: Fixed,
    pub font_revision: Fixed,
    pub check_sum_adjustment: ULONG,
    pub magic_number: ULONG,
    pub flags: USHORT,
    pub units_per_em: USHORT,
    pub created: [BYTE; 8],
    pub modified: [BYTE; 8],
    pub x_min: FWord,
    pub y_min: FWord,
    pub x_max: FWord,
    pub y_max: FWord,
    pub mac_style: USHORT,
    pub lowest_rec_ppem: USHORT,
    pub font_direction_hint: SHORT,
    pub index_to_loc_format: SHORT,
    pub glyph_data_format: SHORT,
}

/// `struct tt_hhea_table`.
#[derive(Clone, Debug, Default)]
pub struct TtHheaTable {
    pub version: Fixed,
    pub ascent: FWord,
    pub descent: FWord,
    pub line_gap: FWord,
    pub advance_width_max: UFWord,
    pub min_left_side_bearing: FWord,
    pub min_right_side_bearing: FWord,
    pub x_max_extent: FWord,
    pub caret_slope_rise: SHORT,
    pub caret_slope_run: SHORT,
    pub caret_offset: FWord,
    /// Set to 0.
    pub reserved: [SHORT; 4],
    pub metric_data_format: SHORT,
    pub num_of_long_hor_metrics: USHORT,
    /// Extra information.
    pub num_of_ex_side_bearings: USHORT,
}

/// `struct tt_vhea_table`.
#[derive(Clone, Debug, Default)]
pub struct TtVheaTable {
    pub version: Fixed,
    pub vert_typo_ascender: SHORT,
    pub vert_typo_descender: SHORT,
    pub vert_typo_line_gap: SHORT,
    pub advance_height_max: SHORT,
    pub min_top_side_bearing: SHORT,
    pub min_bottom_side_bearing: SHORT,
    pub y_max_extent: SHORT,
    pub caret_slope_rise: SHORT,
    pub caret_slope_run: SHORT,
    pub caret_offset: SHORT,
    /// Set to 0.
    pub reserved: [SHORT; 4],
    pub metric_data_format: SHORT,
    pub num_of_long_ver_metrics: USHORT,
    /// Extra information.
    pub num_of_ex_side_bearings: USHORT,
}

/// `struct tt_maxp_table`.
#[derive(Clone, Debug, Default)]
pub struct TtMaxpTable {
    pub version: Fixed,
    pub num_glyphs: USHORT,
    pub max_points: USHORT,
    pub max_contours: USHORT,
    pub max_component_points: USHORT,
    pub max_component_contours: USHORT,
    pub max_zones: USHORT,
    pub max_twilight_points: USHORT,
    pub max_storage: USHORT,
    pub max_function_defs: USHORT,
    pub max_instruction_defs: USHORT,
    pub max_stack_elements: USHORT,
    pub max_size_of_instructions: USHORT,
    pub max_component_elements: USHORT,
    pub max_component_depth: USHORT,
}

/// `struct tt_os2__table`.
#[derive(Clone, Debug, Default)]
pub struct TtOs2Table {
    /// 0x0001 or 0x0002.
    pub version: USHORT,
    pub x_avg_char_width: SHORT,
    pub us_weight_class: USHORT,
    pub us_width_class: USHORT,
    /// `if (fsType & 0x08)` editable embedding.
    pub fs_type: SHORT,
    pub y_subscript_x_size: SHORT,
    pub y_subscript_y_size: SHORT,
    pub y_subscript_x_offset: SHORT,
    pub y_subscript_y_offset: SHORT,
    pub y_superscript_x_size: SHORT,
    pub y_superscript_y_size: SHORT,
    pub y_superscript_x_offset: SHORT,
    pub y_superscript_y_offset: SHORT,
    pub y_strikeout_size: SHORT,
    pub y_strikeout_position: SHORT,
    pub s_family_class: SHORT,
    pub panose: [BYTE; 10],
    pub ul_unicode_range1: ULONG,
    pub ul_unicode_range2: ULONG,
    pub ul_unicode_range3: ULONG,
    pub ul_unicode_range4: ULONG,
    pub ach_vend_id: [CHAR; 4],
    pub fs_selection: USHORT,
    pub us_first_char_index: USHORT,
    pub us_last_char_index: USHORT,
    pub s_typo_ascender: SHORT,
    pub s_typo_descender: SHORT,
    pub s_typo_line_gap: SHORT,
    pub us_win_ascent: USHORT,
    pub us_win_descent: USHORT,
    pub ul_code_page_range1: ULONG,
    pub ul_code_page_range2: ULONG,
    /// Version 0x0002.
    pub sx_height: SHORT,
    pub s_cap_height: SHORT,
    pub us_default_char: USHORT,
    pub us_break_char: USHORT,
    pub us_max_context: USHORT,
}

/// `struct tt_vertOriginYMetrics`.
#[derive(Clone, Debug, Default)]
pub struct TtVertOriginYMetrics {
    pub glyph_index: USHORT,
    pub vert_origin_y: SHORT,
}

/// `struct tt_VORG_table`.
#[derive(Clone, Debug, Default)]
pub struct TtVorgTable {
    pub default_vert_origin_y: SHORT,
    pub num_vert_origin_y_metrics: USHORT,
    pub vert_origin_y_metrics: Vec<TtVertOriginYMetrics>,
}

/// `struct tt_longMetrics`.
#[derive(Clone, Debug, Default)]
pub struct TtLongMetrics {
    pub advance: USHORT,
    pub side_bearing: SHORT,
}

impl TtHeadTable {
    /// `tt_pack_head_table`: `TT_HEAD_TABLE_SIZE` bytes.
    #[must_use]
    pub fn tt_pack_head_table(&self) -> Vec<u8> {
        todo!()
    }
}

impl TtHheaTable {
    /// `tt_pack_hhea_table`: `TT_HHEA_TABLE_SIZE` bytes.
    #[must_use]
    pub fn tt_pack_hhea_table(&self) -> Vec<u8> {
        todo!()
    }
}

impl TtMaxpTable {
    /// `tt_pack_maxp_table`: `TT_MAXP_TABLE_SIZE` bytes.
    #[must_use]
    pub fn tt_pack_maxp_table(&self) -> Vec<u8> {
        todo!()
    }
}

impl Sfnt {
    /// `tt_read_head_table`.
    pub fn tt_read_head_table(&mut self) -> TtHeadTable {
        todo!()
    }
    /// `tt_read_hhea_table`.
    pub fn tt_read_hhea_table(&mut self) -> TtHheaTable {
        todo!()
    }
    /// `tt_read_maxp_table`.
    pub fn tt_read_maxp_table(&mut self) -> TtMaxpTable {
        todo!()
    }
    /// `tt_read_vhea_table`.
    pub fn tt_read_vhea_table(&mut self) -> TtVheaTable {
        todo!()
    }
    /// `tt_read_VORG_table`: none without a VORG table.
    pub fn tt_read_VORG_table(&mut self) -> Option<TtVorgTable> {
        todo!()
    }
    /// `tt_read_longMetrics`: `numGlyphs` entries (hmtx/vmtx).
    pub fn tt_read_longMetrics(
        &mut self,
        num_glyphs: USHORT,
        num_long_metrics: USHORT,
        num_ex_side_bearings: USHORT,
    ) -> Vec<TtLongMetrics> {
        todo!()
    }
    /// `tt_read_os2__table`: defaults when the font has no OS/2 table
    /// (C never returns NULL).
    pub fn tt_read_os2__table(&mut self) -> TtOs2Table {
        todo!()
    }
    /// `tt_get_name` (static): name `name_id` for the platform, encoding
    /// and language, at most `destlen - 1` bytes (C's return is the
    /// length; empty = not found).
    fn tt_get_name(
        &mut self,
        destlen: USHORT,
        plat_id: USHORT,
        enco_id: USHORT,
        lang_id: USHORT,
        name_id: USHORT,
    ) -> Vec<u8> {
        todo!()
    }
    /// `tt_get_ps_fontname`: the PostScript name, at most `destlen - 1`
    /// bytes (C's return is its length; empty = none).
    pub fn tt_get_ps_fontname(&mut self, destlen: USHORT) -> Vec<u8> {
        todo!()
    }
}
