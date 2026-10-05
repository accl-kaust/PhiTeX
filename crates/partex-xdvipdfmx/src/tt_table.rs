//! tt_table.c, tt_table.h: the head, hhea, vhea, maxp, VORG, OS/2, hmtx
//! and name tables.
//!
//! Readers are `impl Sfnt` methods returning the table by value (C's
//! `NEW`ed pointer; `tt_read_VORG_table`'s NULL is `None`). Packers are
//! methods of the table returning the packed bytes.

#![allow(non_snake_case)]

use crate::prelude::*;
use crate::sfnt::{
    BYTE, CHAR, FWord, Fixed, SHORT, Sfnt, UFWord, ULONG, USHORT, sfnt_put_short, sfnt_put_ulong,
    sfnt_put_ushort,
};

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
        let mut data = vec![0u8; TT_HEAD_TABLE_SIZE];
        let p = &mut data[..];
        let mut i = 0usize;
        i += sfnt_put_ulong(&mut p[i..], self.version) as usize;
        i += sfnt_put_ulong(&mut p[i..], self.font_revision) as usize;
        i += sfnt_put_ulong(&mut p[i..], self.check_sum_adjustment) as usize;
        i += sfnt_put_ulong(&mut p[i..], self.magic_number) as usize;
        i += sfnt_put_ushort(&mut p[i..], self.flags) as usize;
        i += sfnt_put_ushort(&mut p[i..], self.units_per_em) as usize;
        for k in 0..8 {
            p[i] = self.created[k];
            i += 1;
        }
        for k in 0..8 {
            p[i] = self.modified[k];
            i += 1;
        }
        i += sfnt_put_short(&mut p[i..], self.x_min) as usize;
        i += sfnt_put_short(&mut p[i..], self.y_min) as usize;
        i += sfnt_put_short(&mut p[i..], self.x_max) as usize;
        i += sfnt_put_short(&mut p[i..], self.y_max) as usize;
        i += sfnt_put_ushort(&mut p[i..], self.mac_style) as usize;
        i += sfnt_put_ushort(&mut p[i..], self.lowest_rec_ppem) as usize;
        i += sfnt_put_short(&mut p[i..], self.font_direction_hint) as usize;
        i += sfnt_put_short(&mut p[i..], self.index_to_loc_format) as usize;
        sfnt_put_short(&mut p[i..], self.glyph_data_format);
        data
    }
}

impl TtHheaTable {
    /// `tt_pack_hhea_table`: `TT_HHEA_TABLE_SIZE` bytes.
    #[must_use]
    pub fn tt_pack_hhea_table(&self) -> Vec<u8> {
        let mut data = vec![0u8; TT_HHEA_TABLE_SIZE];
        let p = &mut data[..];
        let mut i = 0usize;
        i += sfnt_put_ulong(&mut p[i..], self.version) as usize;
        i += sfnt_put_short(&mut p[i..], self.ascent) as usize;
        i += sfnt_put_short(&mut p[i..], self.descent) as usize;
        i += sfnt_put_short(&mut p[i..], self.line_gap) as usize;
        i += sfnt_put_ushort(&mut p[i..], self.advance_width_max) as usize;
        i += sfnt_put_short(&mut p[i..], self.min_left_side_bearing) as usize;
        i += sfnt_put_short(&mut p[i..], self.min_right_side_bearing) as usize;
        i += sfnt_put_short(&mut p[i..], self.x_max_extent) as usize;
        i += sfnt_put_short(&mut p[i..], self.caret_slope_rise) as usize;
        i += sfnt_put_short(&mut p[i..], self.caret_slope_run) as usize;
        i += sfnt_put_short(&mut p[i..], self.caret_offset) as usize;
        for k in 0..4 {
            i += sfnt_put_short(&mut p[i..], self.reserved[k]) as usize;
        }
        i += sfnt_put_short(&mut p[i..], self.metric_data_format) as usize;
        sfnt_put_ushort(&mut p[i..], self.num_of_long_hor_metrics);
        data
    }
}

impl TtMaxpTable {
    /// `tt_pack_maxp_table`: `TT_MAXP_TABLE_SIZE` bytes.
    #[must_use]
    pub fn tt_pack_maxp_table(&self) -> Vec<u8> {
        let mut data = vec![0u8; TT_MAXP_TABLE_SIZE];
        let p = &mut data[..];
        let mut i = 0usize;
        i += sfnt_put_ulong(&mut p[i..], self.version) as usize;
        for v in [
            self.num_glyphs,
            self.max_points,
            self.max_contours,
            self.max_component_points,
            self.max_component_contours,
            self.max_zones,
            self.max_twilight_points,
            self.max_storage,
            self.max_function_defs,
            self.max_instruction_defs,
            self.max_stack_elements,
            self.max_size_of_instructions,
            self.max_component_elements,
            self.max_component_depth,
        ] {
            i += sfnt_put_ushort(&mut p[i..], v) as usize;
        }
        data
    }
}

impl Sfnt {
    /// `tt_read_head_table`.
    pub fn tt_read_head_table(&mut self) -> TtHeadTable {
        let mut table = TtHeadTable::default();
        self.sfnt_locate_table(b"head");
        table.version = self.sfnt_get_ulong();
        table.font_revision = self.sfnt_get_ulong();
        table.check_sum_adjustment = self.sfnt_get_ulong();
        table.magic_number = self.sfnt_get_ulong();
        table.flags = self.sfnt_get_ushort();
        table.units_per_em = self.sfnt_get_ushort();
        for i in 0..8 {
            table.created[i] = self.sfnt_get_byte();
        }
        for i in 0..8 {
            table.modified[i] = self.sfnt_get_byte();
        }
        table.x_min = self.sfnt_get_short();
        table.y_min = self.sfnt_get_short();
        table.x_max = self.sfnt_get_short();
        table.y_max = self.sfnt_get_short();
        table.mac_style = self.sfnt_get_short() as USHORT;
        table.lowest_rec_ppem = self.sfnt_get_short() as USHORT;
        table.font_direction_hint = self.sfnt_get_short();
        table.index_to_loc_format = self.sfnt_get_short();
        table.glyph_data_format = self.sfnt_get_short();
        table
    }
    /// `tt_read_hhea_table`.
    pub fn tt_read_hhea_table(&mut self) -> TtHheaTable {
        let mut table = TtHheaTable::default();
        self.sfnt_locate_table(b"hhea");
        table.version = self.sfnt_get_ulong();
        table.ascent = self.sfnt_get_short();
        table.descent = self.sfnt_get_short();
        table.line_gap = self.sfnt_get_short();
        table.advance_width_max = self.sfnt_get_ushort();
        table.min_left_side_bearing = self.sfnt_get_short();
        table.min_right_side_bearing = self.sfnt_get_short();
        table.x_max_extent = self.sfnt_get_short();
        table.caret_slope_rise = self.sfnt_get_short();
        table.caret_slope_run = self.sfnt_get_short();
        table.caret_offset = self.sfnt_get_short();
        for i in 0..4 {
            table.reserved[i] = self.sfnt_get_short();
        }
        table.metric_data_format = self.sfnt_get_short();
        if table.metric_data_format != 0 {
            error!("unknown metricDataFormat");
        }
        table.num_of_long_hor_metrics = self.sfnt_get_ushort();

        let len = self.sfnt_find_table_len(b"hmtx");
        table.num_of_ex_side_bearings =
            (len.wrapping_sub(u32::from(table.num_of_long_hor_metrics) * 4) / 2) as USHORT;
        table
    }
    /// `tt_read_maxp_table`.
    pub fn tt_read_maxp_table(&mut self) -> TtMaxpTable {
        let mut table = TtMaxpTable::default();
        self.sfnt_locate_table(b"maxp");
        table.version = self.sfnt_get_ulong();
        table.num_glyphs = self.sfnt_get_ushort();
        table.max_points = self.sfnt_get_ushort();
        table.max_contours = self.sfnt_get_ushort();
        table.max_component_points = self.sfnt_get_ushort();
        table.max_component_contours = self.sfnt_get_ushort();
        table.max_zones = self.sfnt_get_ushort();
        table.max_twilight_points = self.sfnt_get_ushort();
        table.max_storage = self.sfnt_get_ushort();
        table.max_function_defs = self.sfnt_get_ushort();
        table.max_instruction_defs = self.sfnt_get_ushort();
        table.max_stack_elements = self.sfnt_get_ushort();
        table.max_size_of_instructions = self.sfnt_get_ushort();
        table.max_component_elements = self.sfnt_get_ushort();
        table.max_component_depth = self.sfnt_get_ushort();
        table
    }
    /// `tt_read_vhea_table`.
    pub fn tt_read_vhea_table(&mut self) -> TtVheaTable {
        let mut table = TtVheaTable::default();
        self.sfnt_locate_table(b"vhea");
        table.version = self.sfnt_get_ulong();
        table.vert_typo_ascender = self.sfnt_get_short();
        table.vert_typo_descender = self.sfnt_get_short();
        table.vert_typo_line_gap = self.sfnt_get_short();
        table.advance_height_max = self.sfnt_get_short(); /* ushort ? */
        table.min_top_side_bearing = self.sfnt_get_short();
        table.min_bottom_side_bearing = self.sfnt_get_short();
        table.y_max_extent = self.sfnt_get_short();
        table.caret_slope_rise = self.sfnt_get_short();
        table.caret_slope_run = self.sfnt_get_short();
        table.caret_offset = self.sfnt_get_short();
        for i in 0..4 {
            table.reserved[i] = self.sfnt_get_short();
        }
        table.metric_data_format = self.sfnt_get_short();
        table.num_of_long_ver_metrics = self.sfnt_get_ushort();

        let len = self.sfnt_find_table_len(b"vmtx");
        table.num_of_ex_side_bearings =
            (len.wrapping_sub(u32::from(table.num_of_long_ver_metrics) * 4) / 2) as USHORT;
        table
    }
    /// `tt_read_VORG_table`: none without a VORG table.
    pub fn tt_read_VORG_table(&mut self) -> Option<TtVorgTable> {
        let offset = self.sfnt_find_table_pos(b"VORG");
        if offset > 0 {
            let mut vorg = TtVorgTable::default();
            self.sfnt_locate_table(b"VORG");
            if self.sfnt_get_ushort() != 1 || self.sfnt_get_ushort() != 0 {
                error!("Unsupported VORG version.");
            }
            vorg.default_vert_origin_y = self.sfnt_get_short();
            vorg.num_vert_origin_y_metrics = self.sfnt_get_ushort();
            // The vertOriginYMetrics array must be sorted in increasing
            // glyphIndex order.
            for _ in 0..vorg.num_vert_origin_y_metrics {
                let glyph_index = self.sfnt_get_ushort();
                let vert_origin_y = self.sfnt_get_short();
                vorg.vert_origin_y_metrics.push(TtVertOriginYMetrics {
                    glyph_index,
                    vert_origin_y,
                });
            }
            Some(vorg)
        } else {
            None
        }
    }
    /// `tt_read_longMetrics`: `numGlyphs` entries (hmtx/vmtx), read from
    /// the current position.
    pub fn tt_read_longMetrics(
        &mut self,
        num_glyphs: USHORT,
        num_long_metrics: USHORT,
        num_ex_side_bearings: USHORT,
    ) -> Vec<TtLongMetrics> {
        let mut m = Vec::with_capacity(num_glyphs as usize);
        let mut last_adv: USHORT = 0;
        let mut last_esb: SHORT = 0;
        for gid in 0..num_glyphs {
            if gid < num_long_metrics {
                last_adv = self.sfnt_get_ushort();
            }
            if i32::from(gid) < i32::from(num_long_metrics) + i32::from(num_ex_side_bearings) {
                last_esb = self.sfnt_get_short();
            }
            m.push(TtLongMetrics {
                advance: last_adv,
                side_bearing: last_esb,
            });
        }
        m
    }
    /// `tt_read_os2__table`: defaults when the font has no OS/2 table
    /// (C never returns NULL).
    pub fn tt_read_os2__table(&mut self) -> TtOs2Table {
        let mut table = TtOs2Table::default();
        if self.sfnt_find_table_pos(b"OS/2") > 0 {
            self.sfnt_locate_table(b"OS/2");
            table.version = self.sfnt_get_ushort();
            table.x_avg_char_width = self.sfnt_get_short();
            table.us_weight_class = self.sfnt_get_ushort();
            table.us_width_class = self.sfnt_get_ushort();
            table.fs_type = self.sfnt_get_short();
            table.y_subscript_x_size = self.sfnt_get_short();
            table.y_subscript_y_size = self.sfnt_get_short();
            table.y_subscript_x_offset = self.sfnt_get_short();
            table.y_subscript_y_offset = self.sfnt_get_short();
            table.y_superscript_x_size = self.sfnt_get_short();
            table.y_superscript_y_size = self.sfnt_get_short();
            table.y_superscript_x_offset = self.sfnt_get_short();
            table.y_superscript_y_offset = self.sfnt_get_short();
            table.y_strikeout_size = self.sfnt_get_short();
            table.y_strikeout_position = self.sfnt_get_short();
            table.s_family_class = self.sfnt_get_short();
            for i in 0..10 {
                table.panose[i] = self.sfnt_get_byte();
            }
            table.ul_unicode_range1 = self.sfnt_get_ulong();
            table.ul_unicode_range2 = self.sfnt_get_ulong();
            table.ul_unicode_range3 = self.sfnt_get_ulong();
            table.ul_unicode_range4 = self.sfnt_get_ulong();
            for i in 0..4 {
                table.ach_vend_id[i] = self.sfnt_get_char();
            }
            table.fs_selection = self.sfnt_get_ushort();
            table.us_first_char_index = self.sfnt_get_ushort();
            table.us_last_char_index = self.sfnt_get_ushort();
            if self.sfnt_find_table_len(b"OS/2") >= 78 {
                // Not in Apple's original 68-byte table; Microsoft's
                // "format 0" has them.
                table.s_typo_ascender = self.sfnt_get_short();
                table.s_typo_descender = self.sfnt_get_short();
                table.s_typo_line_gap = self.sfnt_get_short();
                table.us_win_ascent = self.sfnt_get_ushort();
                table.us_win_descent = self.sfnt_get_ushort();
                if table.version > 0 {
                    // Format 1 adds these two.
                    table.ul_code_page_range1 = self.sfnt_get_ulong();
                    table.ul_code_page_range2 = self.sfnt_get_ulong();
                    if table.version > 1 {
                        // Formats 2 and 3 add five more.
                        table.sx_height = self.sfnt_get_short();
                        table.s_cap_height = self.sfnt_get_short();
                        table.us_default_char = self.sfnt_get_ushort();
                        table.us_break_char = self.sfnt_get_ushort();
                        table.us_max_context = self.sfnt_get_ushort();
                    }
                }
            }
        } else {
            // Used in add_CIDVMetrics() of cidtype0.c.
            table.s_typo_ascender = 880;
            table.s_typo_descender = -120;
            // Used in tt_get_fontdesc() of tt_aux.c.
            table.us_weight_class = 400; /* Normal(Regular) */
            table.x_avg_char_width = 0; /* ignore */
            table.version = 0; /* TrueType rev 1.5 */
            table.fs_type = 0; /* Installable Embedding */
            table.fs_selection = 0; /* All undefined */
            table.s_family_class = 0; /* No Classification */
            for i in 0..10 {
                table.panose[i] = 0; /* All Any */
            }
        }
        table
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
        let mut length: USHORT = 0;
        let mut dest = Vec::new();

        let name_offset = self.sfnt_locate_table(b"name");
        let is_utf16_be = plat_id == 3 && enco_id == 1 && lang_id == 0x0409 && name_id == 6;

        if self.sfnt_get_ushort() != 0 {
            error!("Expecting zero");
        }

        let num_names = self.sfnt_get_ushort();
        let string_offset = self.sfnt_get_ushort();
        let mut i: i32 = 0;
        while i < i32::from(num_names) {
            let p_id = self.sfnt_get_ushort();
            let e_id = self.sfnt_get_ushort();
            let l_id = self.sfnt_get_ushort();
            let n_id = self.sfnt_get_ushort();
            length = self.sfnt_get_ushort();
            let offset = self.sfnt_get_ushort();
            // Language ID value 0xffff for `accept any language ID'.
            if p_id == plat_id
                && e_id == enco_id
                && (lang_id == 0xffff || l_id == lang_id)
                && n_id == name_id
            {
                if is_utf16_be {
                    length /= 2;
                }
                if i32::from(length) > i32::from(destlen) - 1 {
                    warn!(
                        "Name string too long ({}), truncating to {}",
                        length, destlen
                    );
                    length = destlen.wrapping_sub(1);
                }
                self.sfnt_seek_set(
                    name_offset
                        .wrapping_add(u32::from(string_offset))
                        .wrapping_add(u32::from(offset)),
                );
                if is_utf16_be {
                    for _ in 0..length {
                        dest.push((self.sfnt_get_ushort() & 0x00ff) as u8);
                    }
                } else {
                    dest = vec![0u8; length as usize];
                    let n = self.sfnt_read(&mut dest);
                    // C leaves the rest of its buffer as it was.
                    let _ = n;
                }
                break;
            }
            i += 1;
        }
        if i == i32::from(num_names) {
            length = 0;
        }
        dest.truncate(length as usize);
        dest
    }
    /// `tt_get_ps_fontname`: the PostScript name, at most `destlen - 1`
    /// bytes (C's return is its length; empty = none).
    pub fn tt_get_ps_fontname(&mut self, destlen: USHORT) -> Vec<u8> {
        // First try Mac-Roman PS name and then Win-Unicode PS name.
        let name = self.tt_get_name(destlen, 1, 0, 0, 6);
        if !name.is_empty() {
            return name;
        }
        let name = self.tt_get_name(destlen, 3, 1, 0x409, 6);
        if !name.is_empty() {
            return name;
        }
        let name = self.tt_get_name(destlen, 3, 5, 0x412, 6);
        if !name.is_empty() {
            return name;
        }

        warn!("No valid PostScript name available");
        // Workaround for some bad TTfonts: language ID 0xffff for `accept
        // any language ID'.
        let name = self.tt_get_name(destlen, 1, 0, 0xffff, 6);
        if name.is_empty() {
            // Finally falling back to Mac Roman name field.
            return self.tt_get_name(destlen, 1, 0, 0, 1);
        }
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_sizes() {
        let h = TtHeadTable {
            version: 0x0001_0000,
            units_per_em: 2048,
            x_min: -5,
            index_to_loc_format: 1,
            ..TtHeadTable::default()
        };
        let d = h.tt_pack_head_table();
        assert_eq!(d.len(), TT_HEAD_TABLE_SIZE);
        assert_eq!(&d[..4], &[0, 1, 0, 0]);
        assert_eq!(&d[18..20], &[0x08, 0x00]);
        assert_eq!(&d[36..38], &[0xff, 0xfb]);
        assert_eq!(&d[50..52], &[0, 1]);
        assert_eq!(TtHheaTable::default().tt_pack_hhea_table().len(), 36);
        assert_eq!(TtMaxpTable::default().tt_pack_maxp_table().len(), 32);
    }
}
