//! dvi.c, dvi.h, dvicodes.h: reading the DVI/XDV file and interpreting
//! its pages.
//!
//! The DVI file is [`State::dvi_file`], a [`MemFile`]: C reads xelatex's
//! XDV from stdin (`linear` processing); here the host puts the XDV bytes
//! in `dvi_file` before `dvi_init(None, …)`. dvipdfmx.c's globals that
//! dvi.h declares (`paper_width`, `paper_height`, `landscape_mode`,
//! `dvi_ptex_with_vert`) are in `crate::session::State`.
//! Subfonts (`subfont_locate_font`, SFD) and pkfont are not ported.

use crate::pdfdev::{PdfRect, Spt};
use crate::prelude::*;

// dvicodes.h

pub const SET_CHAR_0: u8 = 0;
pub const SET_CHAR_1: u8 = 1;
pub const SET_CHAR_127: u8 = 127;
/// Typesets its single operand between 128 and 255.
pub const SET1: u8 = 128;
pub const SET2: u8 = 129;
pub const SET3: u8 = 130;
pub const SET4: u8 = 131;
/// A rule of height, width (four signed bytes each).
pub const SET_RULE: u8 = 132;
pub const PUT1: u8 = 133;
pub const PUT2: u8 = 134;
pub const PUT3: u8 = 135;
pub const PUT4: u8 = 136;
pub const PUT_RULE: u8 = 137;
pub const NOP: u8 = 138;
pub const BOP: u8 = 139;
pub const EOP: u8 = 140;
pub const PUSH: u8 = 141;
pub const POP: u8 = 142;
pub const RIGHT1: u8 = 143;
pub const RIGHT2: u8 = 144;
pub const RIGHT3: u8 = 145;
pub const RIGHT4: u8 = 146;
pub const W0: u8 = 147;
pub const W1: u8 = 148;
pub const W2: u8 = 149;
pub const W3: u8 = 150;
pub const W4: u8 = 151;
pub const X0: u8 = 152;
pub const X1: u8 = 153;
pub const X2: u8 = 154;
pub const X3: u8 = 155;
pub const X4: u8 = 156;
pub const DOWN1: u8 = 157;
pub const DOWN2: u8 = 158;
pub const DOWN3: u8 = 159;
pub const DOWN4: u8 = 160;
pub const Y0: u8 = 161;
pub const Y1: u8 = 162;
pub const Y2: u8 = 163;
pub const Y3: u8 = 164;
pub const Y4: u8 = 165;
pub const Z0: u8 = 166;
pub const Z1: u8 = 167;
pub const Z2: u8 = 168;
pub const Z3: u8 = 169;
pub const Z4: u8 = 170;
pub const FNT_NUM_0: u8 = 171;
pub const FNT_NUM_1: u8 = 172;
pub const FNT_NUM_63: u8 = 234;
pub const FNT1: u8 = 235;
pub const FNT2: u8 = 236;
pub const FNT3: u8 = 237;
pub const FNT4: u8 = 238;
/// Special, one byte length.
pub const XXX1: u8 = 239;
pub const XXX2: u8 = 240;
pub const XXX3: u8 = 241;
pub const XXX4: u8 = 242;
pub const FNT_DEF1: u8 = 243;
pub const FNT_DEF2: u8 = 244;
pub const FNT_DEF3: u8 = 245;
pub const FNT_DEF4: u8 = 246;
/// Preamble.
pub const PRE: u8 = 247;
/// ID byte for DVI.
pub const DVI_ID: u8 = 2;
/// With ASCII pTeX VW mode extension.
pub const DVIV_ID: u8 = 3;
/// Older XeTeX `.xdv` without `XDV_TEXT_AND_GLYPHS`.
pub const XDV_ID_OLD: u8 = 6;
/// XeTeX `.xdv`.
pub const XDV_ID: u8 = 7;
/// Postamble.
pub const POST: u8 = 248;
pub const POST_POST: u8 = 249;
pub const PADDING: u8 = 223;
/// TeX-XeT begin_reflect.
pub const BEGIN_REFLECT: u8 = 250;
/// TeX-XeT end_reflect.
pub const END_REFLECT: u8 = 251;
/// Fontdef for a native platform font.
pub const XDV_NATIVE_FONT_DEF: u8 = 252;
/// Glyph IDs with X and Y positions.
pub const XDV_GLYPHS: u8 = 253;
/// Like `XDV_GLYPHS` plus the original Unicode text.
pub const XDV_TEXT_AND_GLYPHS: u8 = 254;
/// ASCII pTeX DIR command.
pub const PTEXDIR: u8 = 255;

// dvi.c

pub const DVI_STACK_DEPTH_MAX: usize = 256;
pub const TEX_FONTS_ALLOC_SIZE: u32 = 16;
pub const VF_NESTING_MAX: usize = 16;

/// Typesetting from left to right.
pub const LTYPESETTING: i32 = 0;
/// Typesetting from right to left.
pub const RTYPESETTING: i32 = 1;
/// Skimming through a reflected segment measuring its width.
pub const SKIMMING: i32 = 2;

/// `loaded_font.type`.
pub const PHYSICAL: i32 = 1;
pub const VIRTUAL: i32 = 2;
pub const SUBFONT: i32 = 3;
pub const NATIVE: i32 = 4;
/// `loaded_font.source`.
pub const DVI: i32 = 1;
pub const VF: i32 = 2;

pub const ENC_UNICODE: i32 = 1;
pub const ENC_UTF16: i32 = 2;

pub const XDV_FLAG_VERTICAL: u16 = 0x0100;
pub const XDV_FLAG_COLORED: u16 = 0x0200;
pub const XDV_FLAG_FEATURES: u16 = 0x0400;
pub const XDV_FLAG_EXTEND: u16 = 0x1000;
pub const XDV_FLAG_SLANT: u16 = 0x2000;
pub const XDV_FLAG_EMBOLDEN: u16 = 0x4000;

/// 64K should be plenty for most pages.
pub const DVI_PAGE_BUF_CHUNK: u32 = 0x10000;

/// `SIG_DVILUA_FNT_DEF` (`'LuaF'`).
pub const SIG_DVILUA_FNT_DEF: u32 =
    (b'L' as u32) << 24 | (b'u' as u32) << 16 | (b'a' as u32) << 8 | b'F' as u32;

/// `invalid_signature`.
pub const INVALID_SIGNATURE: &[u8] = b"Something is wrong. Are you sure this is a DVI file?";

/// `UTF32toUTF16HS`.
#[must_use]
pub fn utf32_to_utf16_hs(x: u32) -> u32 {
    0xd800 + (((x - 0x10000) >> 10) & 0x3ff)
}
/// `UTF32toUTF16LS`.
#[must_use]
pub fn utf32_to_utf16_ls(x: u32) -> u32 {
    0xdc00 + (x & 0x3ff)
}

/// `struct dvi_header`.
#[derive(Clone, Debug)]
pub struct DviHeader {
    pub unit_num: u32,
    pub unit_den: u32,
    pub mag: u32,
    pub media_width: u32,
    pub media_height: u32,
    pub stackdepth: u32,
    /// `comment[257]` (without the NUL).
    pub comment: Vec<u8>,
}

impl Default for DviHeader {
    fn default() -> Self {
        DviHeader {
            unit_num: 25_400_000,
            unit_den: 473_628_672,
            mag: 1000,
            media_width: 0,
            media_height: 0,
            stackdepth: 0,
            comment: Vec::new(),
        }
    }
}

/// `struct dvi_lr`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DviLr {
    pub state: i32,
    pub font: i32,
    pub buf_index: u32,
}

/// `struct gm`: glyph metrics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gm {
    pub advance: Spt,
    pub ascent: Spt,
    pub descent: Spt,
}

/// `struct loaded_font`.
#[derive(Clone, Debug, Default)]
pub struct LoadedFont {
    /// `PHYSICAL`, `VIRTUAL`, `SUBFONT`, `NATIVE`.
    pub type_: i32,
    /// Returned by dev (physical) or by vf (virtual).
    pub font_id: i32,
    pub subfont_id: i32,
    pub tfm_id: i32,
    pub size: Spt,
    /// `DVI` or `VF`.
    pub source: i32,
    pub rgba_color: u32,
    pub rgba_used: u8,
    /// Transparency ExtGState.
    pub xgs_id: i32,
    /// XeTeX's glyph metrics (`gm`, `num_glyphs` entries).
    pub gm: Vec<Gm>,
    pub shift_gid: i32,
    pub num_glyphs: u16,
    pub layout_dir: i32,
    pub extend: f32,
    pub slant: f32,
    pub embolden: f32,
    pub is_unicode: i32,
    pub minbytes: i32,
    pub padbytes: [u8; 4],
}

/// `struct font_def`.
#[derive(Clone, Debug, Default)]
pub struct FontDef {
    pub tex_id: i32,
    pub point_size: Spt,
    pub design_size: Spt,
    pub font_name: Vec<u8>,
    /// Index of the loaded font in `loaded_fonts`.
    pub font_id: i32,
    pub used: i32,
    pub native: i32,
    /// Only for native fonts (XeTeX).
    pub rgba_color: u32,
    pub rgba_used: u8,
    pub face_index: u32,
    /// 1 vertical, 0 horizontal.
    pub layout_dir: i32,
    pub extend: i32,
    pub slant: i32,
    pub embolden: i32,
}

/// `struct dvi_registers`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DviRegisters {
    pub h: i32,
    pub v: i32,
    pub w: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub d: u32,
}

/// `struct spt_coord`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SptCoord {
    pub x: Spt,
    pub y: Spt,
}

/// What `dvi_scan_specials` reads and updates (C's in/out pointers).
#[derive(Clone, Debug, Default)]
pub struct ScanSpecials {
    pub page_width: f64,
    pub page_height: f64,
    pub x_offset: f64,
    pub y_offset: f64,
    pub landscape: i32,
    /// None where C passes NULL (do_dvi_pages): version, encryption and
    /// trailer ID are then not scanned.
    pub ext: Option<ScanSpecialsExt>,
}

/// The rest of `dvi_scan_specials`'s pointers (first page only).
#[derive(Clone, Debug, Default)]
pub struct ScanSpecialsExt {
    pub majorversion: i32,
    pub minorversion: i32,
    pub do_enc: i32,
    pub key_bits: i32,
    pub permission: i32,
    /// `opasswd` (at most `MAX_PWD_LEN` bytes).
    pub owner_pw: Vec<u8>,
    /// `upasswd`.
    pub user_pw: Vec<u8>,
    pub has_id: i32,
    pub id1: [u8; 16],
    pub id2: [u8; 16],
}

/// dvi.c's statics.
#[derive(Clone, Debug)]
pub struct State {
    /// The DVI/XDV file.
    pub dvi_file: Option<MemFile>,
    /// 1 for strict linear processing of the input (stdin).
    pub linear: i32,
    pub page_loc: Vec<i32>,
    pub num_pages: u32,
    pub dvi_file_size: u32,
    pub dvi_info: DviHeader,
    pub dev_origin_x: f64,
    pub dev_origin_y: f64,
    /// State at the start of the current skimming.
    pub lr_state: DviLr,
    /// Current direction or skimming depth.
    pub lr_mode: i32,
    /// Total width of the reflected segment.
    pub lr_width: u32,
    pub lr_width_stack: [u32; DVI_STACK_DEPTH_MAX],
    pub lr_width_stack_depth: u32,
    /// `loaded_fonts` (`num_loaded_fonts` is the length).
    pub loaded_fonts: Vec<LoadedFont>,
    /// `def_fonts` (`num_def_fonts` is the length).
    pub def_fonts: Vec<FontDef>,
    pub compute_boxes: i32,
    pub link_annot: i32,
    /// 1: phantoms with the current font size; 2: with the given height
    /// and depth.
    pub catch_phantom: i32,
    pub phantom_height: f64,
    pub phantom_depth: f64,
    /// `dvi_page_buffer` (its length is `dvi_page_buf_size`).
    pub dvi_page_buffer: Vec<u8>,
    pub dvi_page_buf_size: u32,
    pub dvi_page_buf_index: u32,
    pub pre_id_byte: i32,
    pub post_id_byte: i32,
    pub is_ptex: i32,
    pub has_ptex: i32,
    pub dvi2pts: f64,
    pub total_mag: f64,
    pub dvi_state: DviRegisters,
    pub dvi_stack: [DviRegisters; DVI_STACK_DEPTH_MAX],
    pub dvi_stack_depth: u32,
    pub current_font: i32,
    pub processing_page: i32,
    pub marked_depth: u32,
    pub tagged_depth: i32,
    pub compensation: SptCoord,
    pub saved_dvi_font: [i32; VF_NESTING_MAX],
    pub num_saved_fonts: i32,
    /// `dvi_scan_specials`'s static `buffered_page`.
    pub buffered_page: i32,
}

impl Default for State {
    fn default() -> Self {
        State {
            dvi_file: None,
            linear: 0,
            page_loc: Vec::new(),
            num_pages: 0,
            dvi_file_size: 0,
            dvi_info: DviHeader::default(),
            dev_origin_x: 72.0,
            dev_origin_y: 770.0,
            lr_state: DviLr::default(),
            lr_mode: 0,
            lr_width: 0,
            lr_width_stack: [0; DVI_STACK_DEPTH_MAX],
            lr_width_stack_depth: 0,
            loaded_fonts: Vec::new(),
            def_fonts: Vec::new(),
            compute_boxes: 0,
            link_annot: 1,
            catch_phantom: 0,
            phantom_height: 0.0,
            phantom_depth: 0.0,
            dvi_page_buffer: Vec::new(),
            dvi_page_buf_size: 0,
            dvi_page_buf_index: 0,
            pre_id_byte: 0,
            post_id_byte: 0,
            is_ptex: 0,
            has_ptex: 0,
            dvi2pts: 1.52018,
            total_mag: 1.0,
            dvi_state: DviRegisters::default(),
            dvi_stack: [DviRegisters::default(); DVI_STACK_DEPTH_MAX],
            dvi_stack_depth: 0,
            current_font: -1,
            processing_page: 0,
            marked_depth: 0,
            tagged_depth: -1,
            compensation: SptCoord::default(),
            saved_dvi_font: [0; VF_NESTING_MAX],
            num_saved_fonts: 0,
            buffered_page: -1,
        }
    }
}

impl Dpx {
    /// `get_origin`: `dev_origin_x` if `x`, else `dev_origin_y`.
    pub fn get_origin(&mut self, x: i32) -> f64 {
        todo!()
    }

    /// `need_more_fonts`.
    fn need_more_fonts(&mut self, n: u32) {
        todo!()
    }

    // The page buffer (the current page's bytes, read once).

    /// `get_and_buffer_unsigned_byte` (from `dvi_file`).
    fn get_and_buffer_unsigned_byte(&mut self) -> i32 {
        todo!()
    }
    /// `get_and_buffer_unsigned_pair`.
    fn get_and_buffer_unsigned_pair(&mut self) -> u32 {
        todo!()
    }
    /// `get_and_buffer_bytes`.
    fn get_and_buffer_bytes(&mut self, count: u32) {
        todo!()
    }
    /// `get_buffered_unsigned_byte`.
    fn get_buffered_unsigned_byte(&mut self) -> i32 {
        todo!()
    }
    /// `get_buffered_unsigned_pair`.
    fn get_buffered_unsigned_pair(&mut self) -> u32 {
        todo!()
    }
    /// `get_buffered_signed_quad`.
    fn get_buffered_signed_quad(&mut self) -> i32 {
        todo!()
    }
    /// `get_buffered_signed_num`: `num + 1` bytes.
    fn get_buffered_signed_num(&mut self, num: u8) -> i32 {
        todo!()
    }
    /// `get_buffered_unsigned_num`: `num + 1` bytes.
    fn get_buffered_unsigned_num(&mut self, num: u8) -> i32 {
        todo!()
    }

    /// `dvi_npages`.
    pub fn dvi_npages(&mut self) -> u32 {
        todo!()
    }

    /// `check_id_bytes`.
    fn check_id_bytes(&mut self) {
        todo!()
    }
    /// `need_XeTeX`.
    fn need_xetex(&mut self, c: i32) {
        todo!()
    }
    /// `need_pTeX`.
    fn need_ptex(&mut self, c: i32) {
        todo!()
    }
    /// `find_post`: the postamble's location.
    fn find_post(&mut self) -> i32 {
        todo!()
    }
    /// `get_page_info`.
    fn get_page_info(&mut self, post_location: i32) {
        todo!()
    }
    /// `dvi_tell_mag`.
    pub fn dvi_tell_mag(&mut self) -> f64 {
        todo!()
    }
    /// `do_scales`.
    fn do_scales(&mut self, mag: f64) {
        todo!()
    }
    /// `get_dvi_info`.
    fn get_dvi_info(&mut self, post_location: i32) {
        todo!()
    }
    /// `get_preamble_dvi_info`.
    fn get_preamble_dvi_info(&mut self) {
        todo!()
    }
    /// `dvi_comment`: a copy of `dvi_info.comment`.
    pub fn dvi_comment(&mut self) -> Vec<u8> {
        todo!()
    }
    /// `proc_dvilua_font_record`.
    fn proc_dvilua_font_record(
        &mut self,
        tex_id: i32,
        font_name: &[u8],
        point_size: u32,
        design_size: u32,
    ) {
        todo!()
    }
    /// `read_font_record`.
    fn read_font_record(&mut self, tex_id: i32) {
        todo!()
    }
    /// `read_native_font_record`.
    fn read_native_font_record(&mut self, tex_id: i32) {
        todo!()
    }
    /// `get_dvi_fonts`.
    fn get_dvi_fonts(&mut self, post_location: i32) {
        todo!()
    }
    /// `get_comment`.
    fn get_comment(&mut self) {
        todo!()
    }

    /// `clear_state`.
    fn clear_state(&mut self) {
        todo!()
    }
    /// `dvi_mark_depth`.
    fn dvi_mark_depth(&mut self) {
        todo!()
    }
    /// `dvi_tag_depth`.
    pub fn dvi_tag_depth(&mut self) {
        todo!()
    }
    /// `dvi_untag_depth`.
    pub fn dvi_untag_depth(&mut self) {
        todo!()
    }
    /// `dvi_compute_boxes`.
    pub fn dvi_compute_boxes(&mut self, flag: i32) {
        todo!()
    }
    /// `dvi_link_annot`.
    pub fn dvi_link_annot(&mut self, flag: i32) {
        todo!()
    }
    /// `dvi_is_tracking_boxes`.
    pub fn dvi_is_tracking_boxes(&mut self) -> bool {
        todo!()
    }
    /// `dvi_set_linkmode`.
    pub fn dvi_set_linkmode(&mut self, mode: i32) {
        todo!()
    }
    /// `dvi_set_phantom_height`.
    pub fn dvi_set_phantom_height(&mut self, height: f64, depth: f64) {
        todo!()
    }
    /// `dvi_set_compensation`.
    pub fn dvi_set_compensation(&mut self, x: f64, y: f64) {
        todo!()
    }

    /// `set_string`.
    fn set_string(&mut self, xpos: Spt, ypos: Spt, instr: &[u8], width: Spt, font_id: i32) {
        todo!()
    }
    /// `set_rule`.
    fn set_rule(&mut self, xpos: Spt, ypos: Spt, width: Spt, height: Spt) {
        todo!()
    }
    /// `calc_rect`.
    fn calc_rect(&mut self, xpos: Spt, ypos: Spt, width: Spt, height: Spt, depth: Spt) -> PdfRect {
        todo!()
    }

    /// `dvi_do_special`.
    pub fn dvi_do_special(&mut self, buffer: &[u8]) {
        todo!()
    }
    /// `dvi_unit_size`.
    pub fn dvi_unit_size(&mut self) -> f64 {
        todo!()
    }
    /// `dvi_locate_font`: the index into `loaded_fonts` (C's `cur_id`).
    pub fn dvi_locate_font(&mut self, tfm_name: &[u8], ptsize: Spt) -> i32 {
        todo!()
    }
    /// `dvi_locate_native_font`.
    fn dvi_locate_native_font(
        &mut self,
        filename: &[u8],
        index: u32,
        ptsize: Spt,
        layout_dir: i32,
        extend: i32,
        slant: i32,
        embolden: i32,
    ) -> i32 {
        todo!()
    }

    /// `dvi_dev_xpos`.
    pub fn dvi_dev_xpos(&mut self) -> f64 {
        todo!()
    }
    /// `dvi_dev_ypos`.
    pub fn dvi_dev_ypos(&mut self) -> f64 {
        todo!()
    }
    /// `do_moveto`.
    fn do_moveto(&mut self, x: i32, y: i32) {
        todo!()
    }
    /// `dvi_right`.
    pub fn dvi_right(&mut self, x: i32) {
        todo!()
    }
    /// `dvi_down`.
    pub fn dvi_down(&mut self, y: i32) {
        todo!()
    }
    /// `dvi_set`.
    pub fn dvi_set(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_put`.
    pub fn dvi_put(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_rule`.
    pub fn dvi_rule(&mut self, width: i32, height: i32) {
        todo!()
    }
    /// `dvi_dirchg`.
    pub fn dvi_dirchg(&mut self, dir: u8) {
        todo!()
    }
    /// `do_setrule`.
    fn do_setrule(&mut self) {
        todo!()
    }
    /// `do_putrule`.
    fn do_putrule(&mut self) {
        todo!()
    }
    /// `dvi_push`.
    pub fn dvi_push(&mut self) {
        todo!()
    }
    /// `dvi_pop`.
    pub fn dvi_pop(&mut self) {
        todo!()
    }
    /// `dvi_w`.
    pub fn dvi_w(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_w0`.
    pub fn dvi_w0(&mut self) {
        todo!()
    }
    /// `dvi_x`.
    pub fn dvi_x(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_x0`.
    pub fn dvi_x0(&mut self) {
        todo!()
    }
    /// `dvi_y`.
    pub fn dvi_y(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_y0`.
    pub fn dvi_y0(&mut self) {
        todo!()
    }
    /// `dvi_z`.
    pub fn dvi_z(&mut self, ch: i32) {
        todo!()
    }
    /// `dvi_z0`.
    pub fn dvi_z0(&mut self) {
        todo!()
    }
    /// `skip_fntdef`.
    fn skip_fntdef(&mut self) {
        todo!()
    }
    /// `do_fntdef`.
    fn do_fntdef(&mut self, tex_id: i32) {
        todo!()
    }
    /// `dvi_set_font`.
    pub fn dvi_set_font(&mut self, font_id: i32) {
        todo!()
    }
    /// `do_fnt`.
    fn do_fnt(&mut self, tex_id: i32) {
        todo!()
    }
    /// `do_xxx`.
    fn do_xxx(&mut self, size: i32) {
        todo!()
    }
    /// `do_bop`.
    fn do_bop(&mut self) {
        todo!()
    }
    /// `do_eop`.
    fn do_eop(&mut self) {
        todo!()
    }
    /// `do_dir`.
    fn do_dir(&mut self) {
        todo!()
    }
    /// `lr_width_push`.
    fn lr_width_push(&mut self) {
        todo!()
    }
    /// `lr_width_pop`.
    fn lr_width_pop(&mut self) {
        todo!()
    }
    /// `dvi_begin_reflect`.
    fn dvi_begin_reflect(&mut self) {
        todo!()
    }
    /// `dvi_end_reflect`.
    fn dvi_end_reflect(&mut self) {
        todo!()
    }
    /// `skip_native_font_def`.
    fn skip_native_font_def(&mut self) {
        todo!()
    }
    /// `do_native_font_def`.
    fn do_native_font_def(&mut self, tex_id: i32) {
        todo!()
    }
    /// `skip_glyphs`.
    fn skip_glyphs(&mut self) {
        todo!()
    }
    /// `do_glyphs`.
    fn do_glyphs(&mut self, do_actual_text: i32) {
        todo!()
    }
    /// `check_postamble`.
    fn check_postamble(&mut self) {
        todo!()
    }
    /// `dvi_do_page`.
    pub fn dvi_do_page(&mut self, page_paper_height: f64, hmargin: f64, vmargin: f64) {
        todo!()
    }
    /// `dvi_init`: the scale (dvi2pts). `dvi_filename` none: the XDV is
    /// already in `self.dvi.dvi_file` (C's stdin, linear processing).
    pub fn dvi_init(&mut self, dvi_filename: Option<&[u8]>, mag: f64) -> f64 {
        todo!()
    }
    /// `dvi_close`.
    pub fn dvi_close(&mut self) {
        todo!()
    }
    /// `dvi_vf_init`.
    pub fn dvi_vf_init(&mut self, dev_font_id: i32) {
        todo!()
    }
    /// `dvi_vf_finish`.
    pub fn dvi_vf_finish(&mut self) {
        todo!()
    }

    /// `scan_special_encrypt` (pdf:encrypt; the values are only recorded,
    /// encryption itself is not ported): status.
    fn scan_special_encrypt(&mut self, ext: &mut ScanSpecialsExt, s: &[u8], pp: &mut usize) -> i32 {
        todo!()
    }
    /// `scan_special_trailerid`: status.
    fn scan_special_trailerid(
        &mut self,
        ext: &mut ScanSpecialsExt,
        s: &[u8],
        pp: &mut usize,
    ) -> i32 {
        todo!()
    }
    /// `scan_special`: status; `buf` is the special's bytes.
    fn scan_special(&mut self, sp: &mut ScanSpecials, buf: &[u8]) -> i32 {
        todo!()
    }
    /// `dvi_scan_specials`: page sizes, offsets, landscape (and on the
    /// first page the version, encryption, trailer ID) from the page's
    /// specials, updating `sp`.
    pub fn dvi_scan_specials(&mut self, page_no: i32, sp: &mut ScanSpecials) {
        todo!()
    }
}
