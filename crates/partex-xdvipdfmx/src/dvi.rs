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
    /// A native font's file and face (for the glyph runs).
    pub native_path: Vec<u8>,
    pub face_index: u32,
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
    /// Whether `dvi_do_page` looks past EOP for the postamble (C, linear;
    /// the page API checks for it itself).
    pub peek_after_eop: bool,
    /// The glyph runs of the page being done (`do_glyphs`).
    pub glyph_runs: Vec<GlyphRun>,
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
            peek_after_eop: true,
            glyph_runs: Vec::new(),
        }
    }
}

/// A glyph XeTeX placed (`XDV_GLYPHS`), as the page shows it: a side
/// output of [`Dpx::dvi_do_page`].
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    /// The font file (absolute path) and face index.
    pub font_file: Vec<u8>,
    pub face_index: u32,
    /// The glyph id (in the font file).
    pub glyph: u16,
    /// The glyph's origin in the page's default user space (bp), and its
    /// size (bp).
    pub x: f64,
    pub y: f64,
    pub size: f64,
    /// RGBA (the font's colour; opaque black when it has none).
    pub rgba: u32,
}

impl Dpx {
    fn dvi_file(&mut self) -> &mut MemFile {
        self.dvi.dvi_file.as_mut().expect("DVI file")
    }

    /// `get_origin`: `dev_origin_x` if `x`, else `dev_origin_y`.
    pub fn get_origin(&mut self, x: i32) -> f64 {
        if x != 0 {
            self.dvi.dev_origin_x
        } else {
            self.dvi.dev_origin_y
        }
    }

    /// `need_more_fonts`.
    fn need_more_fonts(&mut self, _n: u32) {}

    // The page buffer (the current page's bytes, read once).

    fn buf_put(&mut self, ch: u8) {
        let i = self.dvi.dvi_page_buf_index as usize;
        if i >= self.dvi.dvi_page_buffer.len() {
            self.dvi
                .dvi_page_buffer
                .resize(i + DVI_PAGE_BUF_CHUNK as usize, 0);
        }
        self.dvi.dvi_page_buffer[i] = ch;
        self.dvi.dvi_page_buf_index += 1;
    }

    /// `get_and_buffer_unsigned_byte` (from `dvi_file`).
    fn get_and_buffer_unsigned_byte(&mut self) -> Result<i32> {
        let ch = self.dvi_file().getc();
        if ch < 0 {
            crate::fatal!("File ended prematurely");
        }
        self.buf_put(ch as u8);
        Ok(ch)
    }
    /// `get_and_buffer_unsigned_pair`.
    fn get_and_buffer_unsigned_pair(&mut self) -> Result<u32> {
        let pair = self.get_and_buffer_unsigned_byte()? as u32;
        Ok((pair << 8) | self.get_and_buffer_unsigned_byte()? as u32)
    }
    /// `get_and_buffer_bytes`.
    fn get_and_buffer_bytes(&mut self, count: u32) -> Result<()> {
        let f = self.dvi.dvi_file.as_mut().expect("DVI file");
        let data = f.read(count as usize).to_vec();
        if data.len() != count as usize {
            crate::fatal!("File ended prematurely");
        }
        let i = self.dvi.dvi_page_buf_index as usize;
        if i + data.len() > self.dvi.dvi_page_buffer.len() {
            self.dvi
                .dvi_page_buffer
                .resize(i + data.len() + DVI_PAGE_BUF_CHUNK as usize, 0);
        }
        self.dvi.dvi_page_buffer[i..i + data.len()].copy_from_slice(&data);
        self.dvi.dvi_page_buf_index += count;
        Ok(())
    }
    /// `get_buffered_unsigned_byte`.
    fn get_buffered_unsigned_byte(&mut self) -> i32 {
        let c = self.dvi.dvi_page_buffer[self.dvi.dvi_page_buf_index as usize];
        self.dvi.dvi_page_buf_index += 1;
        i32::from(c)
    }
    /// `get_buffered_unsigned_pair`.
    fn get_buffered_unsigned_pair(&mut self) -> u32 {
        let a = self.get_buffered_unsigned_byte() as u32;
        (a << 8) | self.get_buffered_unsigned_byte() as u32
    }
    /// `get_buffered_signed_quad`.
    fn get_buffered_signed_quad(&mut self) -> i32 {
        let mut quad = self.get_buffered_unsigned_byte();
        if quad >= 0x80 {
            quad -= 0x100;
        }
        for _ in 0..3 {
            quad = (quad << 8) | self.get_buffered_unsigned_byte();
        }
        quad
    }
    /// `get_buffered_signed_num`: `num + 1` bytes.
    fn get_buffered_signed_num(&mut self, num: u8) -> i32 {
        let mut quad = self.get_buffered_unsigned_byte();
        if quad > 0x7f {
            quad -= 0x100;
        }
        for _ in 0..num.min(3) {
            quad = (quad << 8) | self.get_buffered_unsigned_byte();
        }
        quad
    }
    /// `get_buffered_unsigned_num`: `num + 1` bytes.
    fn get_buffered_unsigned_num(&mut self, num: u8) -> i32 {
        let mut quad = self.get_buffered_unsigned_byte();
        if num == 3 && quad > 0x7f {
            quad -= 0x100;
        }
        for _ in 0..num.min(3) {
            quad = (quad << 8) | self.get_buffered_unsigned_byte();
        }
        quad
    }

    /// `dvi_npages`.
    pub fn dvi_npages(&mut self) -> u32 {
        self.dvi.num_pages
    }

    /// `check_id_bytes`.
    fn check_id_bytes(&mut self) -> Result<()> {
        let (pre, post) = (self.dvi.pre_id_byte, self.dvi.post_id_byte);
        if pre != post && (pre != i32::from(DVI_ID) || post != i32::from(DVIV_ID)) {
            crate::fatal!(
                "Inconsistent DVI id_bytes {} (pre) and {} (post)",
                pre,
                post
            );
        }
        Ok(())
    }
    /// `need_XeTeX`.
    fn need_xetex(&mut self, c: i32) -> Result<()> {
        if self.conf.compat_mode != crate::ctx::CompatMode::Xdv {
            crate::fatal!("DVI opcode {} only valid for XeTeX", c);
        }
        Ok(())
    }
    /// `need_pTeX`.
    fn need_ptex(&mut self, c: i32) -> Result<()> {
        if self.dvi.is_ptex == 0 {
            crate::fatal!("DVI opcode {} only valid for Ascii pTeX", c);
        }
        self.dvi.has_ptex = 1;
        Ok(())
    }
    /// `find_post`: the postamble's location.
    fn find_post(&mut self) -> Result<i32> {
        let size = self.dvi_file().len();
        if size > 0x7fff_ffff {
            crate::fatal!("DVI file size exceeds 31-bit");
        }
        self.dvi.dvi_file_size = size as u32;
        let mut current = size as i64;
        let mut ch;
        loop {
            current -= 1;
            self.dvi_file().seek_absolute(current as usize);
            ch = self.dvi_file().getc();
            if !(ch == i32::from(PADDING) && current > 0) {
                break;
            }
        }
        if (size as i64 - current) < 4
            || current == 0
            || !(ch == i32::from(DVI_ID)
                || ch == i32::from(DVIV_ID)
                || ch == i32::from(XDV_ID)
                || ch == i32::from(XDV_ID_OLD))
        {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        self.dvi.post_id_byte = ch;
        if ch == i32::from(XDV_ID) || ch == i32::from(XDV_ID_OLD) {
            self.conf.compat_mode = crate::ctx::CompatMode::Xdv;
        }
        self.dvi.is_ptex = i32::from(ch == i32::from(DVIV_ID));
        self.session.dvi_ptex_with_vert = self.dvi.is_ptex;
        current -= 5;
        self.dvi_file().seek_absolute(current as usize);
        if self.dvi_file().getc() != i32::from(POST_POST) {
            crate::fatal!("Found where post_post opcode should be");
        }
        let current = self.dvi_file().get_signed_quad()?;
        self.dvi_file().seek_absolute(current as usize);
        if self.dvi_file().getc() != i32::from(POST) {
            crate::fatal!("Found where post opcode should be");
        }
        self.dvi_file().seek_absolute(0);
        if self.dvi_file().get_unsigned_byte()? != PRE {
            crate::fatal!("Found where PRE was expected");
        }
        let ch = self.dvi_file().get_unsigned_byte()?;
        if !(ch == DVI_ID || ch == XDV_ID || ch == XDV_ID_OLD) {
            crate::fatal!("DVI ID = {}", ch);
        }
        self.dvi.pre_id_byte = i32::from(ch);
        self.check_id_bytes()?;
        Ok(current)
    }
    /// `get_page_info`.
    fn get_page_info(&mut self, post_location: i32) -> Result<()> {
        self.dvi_file().seek_absolute(post_location as usize + 27);
        let n = u32::from(self.dvi_file().get_unsigned_pair()?);
        if n == 0 {
            crate::fatal!("Page count is 0!");
        }
        self.dvi.num_pages = n;
        let mut page_loc = vec![0i32; n as usize];
        self.dvi_file().seek_absolute(post_location as usize + 1);
        page_loc[n as usize - 1] = self.dvi_file().get_unsigned_quad()? as i32;
        for i in (0..n as usize - 1).rev() {
            self.dvi_file().seek_absolute(page_loc[i + 1] as usize + 41);
            page_loc[i] = self.dvi_file().get_unsigned_quad()? as i32;
        }
        self.dvi.page_loc = page_loc;
        Ok(())
    }
    /// `dvi_tell_mag`.
    pub fn dvi_tell_mag(&mut self) -> f64 {
        self.dvi.total_mag
    }
    /// `do_scales`.
    fn do_scales(&mut self, mag: f64) {
        self.dvi.total_mag = f64::from(self.dvi.dvi_info.mag) / 1000.0 * mag;
        self.dvi.dvi2pts =
            f64::from(self.dvi.dvi_info.unit_num) / f64::from(self.dvi.dvi_info.unit_den);
        self.dvi.dvi2pts *= 72.0 / 254000.0;
    }
    /// `get_dvi_info`.
    fn get_dvi_info(&mut self, post_location: i32) -> Result<()> {
        self.dvi_file().seek_absolute(post_location as usize + 5);
        self.dvi.dvi_info.unit_num = self.dvi_file().get_unsigned_quad()?;
        self.dvi.dvi_info.unit_den = self.dvi_file().get_unsigned_quad()?;
        self.dvi.dvi_info.mag = self.dvi_file().get_unsigned_quad()?;
        self.dvi.dvi_info.media_height = self.dvi_file().get_unsigned_quad()?;
        self.dvi.dvi_info.media_width = self.dvi_file().get_unsigned_quad()?;
        self.dvi.dvi_info.stackdepth = u32::from(self.dvi_file().get_unsigned_pair()?);
        if self.dvi.dvi_info.stackdepth as usize > DVI_STACK_DEPTH_MAX {
            crate::fatal!("Capacity exceeded.");
        }
        Ok(())
    }
    /// `get_preamble_dvi_info`.
    fn get_preamble_dvi_info(&mut self) -> Result<()> {
        if self.dvi_file().get_unsigned_byte()? != PRE {
            crate::fatal!("Found where PRE was expected");
        }
        let ch = self.dvi_file().get_unsigned_byte()?;
        if !(ch == DVI_ID || ch == XDV_ID || ch == XDV_ID_OLD) {
            crate::fatal!("DVI ID = {}", ch);
        }
        self.dvi.pre_id_byte = i32::from(ch);
        if ch == XDV_ID || ch == XDV_ID_OLD {
            self.conf.compat_mode = crate::ctx::CompatMode::Xdv;
        }
        self.dvi.is_ptex = i32::from(ch == DVI_ID);
        self.dvi.dvi_info.unit_num = self.dvi_file().get_positive_quad("DVI", "unit_num")?;
        self.dvi.dvi_info.unit_den = self.dvi_file().get_positive_quad("DVI", "unit_den")?;
        self.dvi.dvi_info.mag = self.dvi_file().get_positive_quad("DVI", "mag")?;
        let n = self.dvi_file().get_unsigned_byte()? as usize;
        let c = self.dvi_file().read(n).to_vec();
        if c.len() != n {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        self.dvi.dvi_info.comment = c;
        self.dvi.num_pages = 0x7FF_FFFF;
        Ok(())
    }
    /// `dvi_comment`: a copy of `dvi_info.comment`.
    pub fn dvi_comment(&mut self) -> Vec<u8> {
        // (C's comment is a C string: it ends at a NUL)
        let c = &self.dvi.dvi_info.comment;
        c.iter()
            .position(|&b| b == 0)
            .map_or_else(|| c.clone(), |n| c[..n].to_vec())
    }
    /// `proc_dvilua_font_record`.
    fn proc_dvilua_font_record(
        &mut self,
        tex_id: i32,
        font_name: &[u8],
        point_size: u32,
        design_size: u32,
    ) -> Result<()> {
        let mut index: u32 = 0;
        let mut embolden: i32 = 0;
        let mut slant: i32 = 0;
        let mut extend: i32 = 0x0001_0000;
        let file_name = &font_name[1..];
        let Some(q) = file_name.iter().position(|&c| c == b']') else {
            crate::fatal!("Syntax error in dvilua fnt_def: no ']' found in font name.");
        };
        let rest = &file_name[q + 1..];
        let fname = file_name[..q].to_vec();
        if rest.first() == Some(&b':') {
            let mut p = 1;
            while p < rest.len() && rest[p] != 0 {
                let delim = rest[p..]
                    .iter()
                    .position(|&c| c == b';')
                    .map_or(rest.len(), |d| p + d);
                let Some(kv) = rest[p..delim]
                    .iter()
                    .position(|&c| c == b'=')
                    .map(|k| p + k)
                else {
                    crate::fatal!("Syntax error in dvilua fnt_def: not in key=value format");
                };
                let key = &rest[p..kv];
                let val = &rest[kv + 1..delim];
                let (v, used) = crate::fmt::strtol(val, 10);
                if used == val.len() {
                    match key {
                        b"index" => index = v as u32,
                        b"embolden" => embolden = v as i32,
                        b"slant" => slant = v as i32,
                        b"extend" => extend = v as i32,
                        _ => {}
                    }
                }
                p = delim + 1;
            }
        }
        self.dvi.def_fonts.push(FontDef {
            tex_id,
            font_name: fname,
            face_index: index,
            point_size: point_size as Spt,
            design_size: design_size as Spt,
            used: 0,
            native: 1,
            layout_dir: 0,
            rgba_color: 0xffff_ffff,
            rgba_used: 0,
            extend,
            slant,
            embolden,
            font_id: 0,
        });
        Ok(())
    }
    /// `read_font_record`.
    fn read_font_record(&mut self, tex_id: i32) -> Result<()> {
        let f = self.dvi_file();
        let checksum = f.get_unsigned_quad()?;
        let point_size = f.get_positive_quad("DVI", "point_size")?;
        let design_size = f.get_positive_quad("DVI", "design_size")?;
        let dir_length = f.get_unsigned_byte()? as usize;
        let name_length = f.get_unsigned_byte()? as usize;
        if f.read(dir_length).len() != dir_length {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        let font_name = f.read(name_length).to_vec();
        if font_name.len() != name_length {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        if checksum == SIG_DVILUA_FNT_DEF && name_length > 0 && font_name[0] == b'[' {
            self.proc_dvilua_font_record(tex_id, &font_name, point_size, design_size)?;
            return Ok(());
        }
        self.dvi.def_fonts.push(FontDef {
            tex_id,
            font_name,
            point_size: point_size as Spt,
            design_size: design_size as Spt,
            used: 0,
            native: 0,
            rgba_color: 0xffff_ffff,
            rgba_used: 0,
            face_index: 0,
            layout_dir: 0,
            extend: 0x0001_0000,
            slant: 0,
            embolden: 0,
            font_id: 0,
        });
        Ok(())
    }
    /// `read_native_font_record`.
    fn read_native_font_record(&mut self, tex_id: i32) -> Result<()> {
        let f = self.dvi_file();
        let point_size = f.get_positive_quad("DVI", "point_size")?;
        let flags = f.get_unsigned_pair()?;
        let len = f.get_unsigned_byte()? as usize;
        let font_name = f.read(len).to_vec();
        if font_name.len() != len {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        let index = f.get_positive_quad("DVI", "index")?;
        let mut d = FontDef {
            tex_id,
            font_name,
            face_index: index,
            point_size: point_size as Spt,
            design_size: 655_360,
            used: 0,
            native: 1,
            layout_dir: 0,
            rgba_color: 0xffff_ffff,
            rgba_used: 0,
            extend: 0x0001_0000,
            slant: 0,
            embolden: 0,
            font_id: 0,
        };
        if flags & XDV_FLAG_VERTICAL != 0 {
            d.layout_dir = 1;
        }
        if flags & XDV_FLAG_COLORED != 0 {
            d.rgba_color = f.get_unsigned_quad()?;
            d.rgba_used = 1;
        }
        if flags & XDV_FLAG_EXTEND != 0 {
            d.extend = f.get_signed_quad()?;
        }
        if flags & XDV_FLAG_SLANT != 0 {
            d.slant = f.get_signed_quad()?;
        }
        if flags & XDV_FLAG_EMBOLDEN != 0 {
            d.embolden = f.get_signed_quad()?;
        }
        self.dvi.def_fonts.push(d);
        Ok(())
    }
    /// `get_dvi_fonts`.
    fn get_dvi_fonts(&mut self, post_location: i32) -> Result<()> {
        self.dvi_file().seek_absolute(post_location as usize + 29);
        loop {
            let code = self.dvi_file().get_unsigned_byte()?;
            if code == POST_POST {
                break;
            }
            match code {
                FNT_DEF1..=FNT_DEF4 => {
                    let id = self.dvi_file().get_unsigned_num(code - FNT_DEF1)?;
                    self.read_font_record(id)?;
                }
                XDV_NATIVE_FONT_DEF => {
                    self.need_xetex(i32::from(code))?;
                    let id = self.dvi_file().get_signed_quad()?;
                    self.read_native_font_record(id)?;
                }
                _ => crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or("")),
            }
        }
        Ok(())
    }
    /// `get_comment`.
    fn get_comment(&mut self) -> Result<()> {
        self.dvi_file().seek_absolute(14);
        let n = self.dvi_file().get_unsigned_byte()? as usize;
        let c = self.dvi_file().read(n).to_vec();
        if c.len() != n {
            crate::fatal!("{}", core::str::from_utf8(INVALID_SIGNATURE).unwrap_or(""));
        }
        self.dvi.dvi_info.comment = c;
        Ok(())
    }

    /// `clear_state`.
    fn clear_state(&mut self) {
        self.dvi.dvi_state = DviRegisters::default();
        self.pdf_dev_set_dirmode(0);
        self.dvi.dvi_stack_depth = 0;
        self.dvi.current_font = -1;
    }
    /// `dvi_mark_depth`.
    fn dvi_mark_depth(&mut self) -> Result<()> {
        if self.dvi.link_annot != 0
            && self.dvi.marked_depth as i32 == self.dvi.tagged_depth
            && self.dvi.dvi_stack_depth as i32 == self.dvi.tagged_depth - 1
        {
            self.pdf_doc_break_annot()?;
        }
        self.dvi.marked_depth = self.dvi.dvi_stack_depth;
        Ok(())
    }
    /// `dvi_tag_depth`.
    pub fn dvi_tag_depth(&mut self) {
        self.dvi.tagged_depth = self.dvi.marked_depth as i32;
        self.dvi_compute_boxes(1);
    }
    /// `dvi_untag_depth`.
    pub fn dvi_untag_depth(&mut self) {
        self.dvi.tagged_depth = -1;
        self.dvi_compute_boxes(0);
    }
    /// `dvi_compute_boxes`.
    pub fn dvi_compute_boxes(&mut self, flag: i32) {
        self.dvi.compute_boxes = flag;
    }
    /// `dvi_link_annot`.
    pub fn dvi_link_annot(&mut self, flag: i32) {
        self.dvi.link_annot = flag;
    }
    /// `dvi_is_tracking_boxes`.
    pub fn dvi_is_tracking_boxes(&mut self) -> bool {
        // (C compares the unsigned marked_depth with the int tagged_depth:
        // -1 converts to UINT_MAX)
        self.dvi.compute_boxes != 0
            && self.dvi.link_annot != 0
            && self.dvi.marked_depth >= self.dvi.tagged_depth as u32
    }
    /// `dvi_set_linkmode`.
    pub fn dvi_set_linkmode(&mut self, mode: i32) {
        self.dvi.catch_phantom = i32::from(mode != 0);
    }
    /// `dvi_set_phantom_height`.
    pub fn dvi_set_phantom_height(&mut self, height: f64, depth: f64) {
        self.dvi.phantom_height = height;
        self.dvi.phantom_depth = depth;
        self.dvi.catch_phantom = 2;
    }
    /// `dvi_set_compensation`.
    pub fn dvi_set_compensation(&mut self, x: f64, y: f64) {
        self.dvi.compensation.x = libm::round(x / self.dvi.dvi2pts) as Spt;
        self.dvi.compensation.y = libm::round(y / self.dvi.dvi2pts) as Spt;
    }

    /// `set_string`.
    fn set_string(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        instr: &[u8],
        width: Spt,
        font_id: i32,
    ) -> Result<()> {
        let xpos = xpos.wrapping_sub(self.dvi.compensation.x);
        let ypos = ypos.wrapping_sub(self.dvi.compensation.y);
        self.pdf_dev_set_string(xpos, ypos, instr, width, font_id)?;
        Ok(())
    }
    /// `set_rule`.
    fn set_rule(&mut self, xpos: Spt, ypos: Spt, width: Spt, height: Spt) -> Result<()> {
        let xpos = xpos.wrapping_sub(self.dvi.compensation.x);
        let ypos = ypos.wrapping_sub(self.dvi.compensation.y);
        self.pdf_dev_set_rule(xpos, ypos, width, height)?;
        Ok(())
    }
    /// `calc_rect`.
    fn calc_rect(&mut self, xpos: Spt, ypos: Spt, width: Spt, height: Spt, depth: Spt) -> PdfRect {
        let xpos = xpos.wrapping_sub(self.dvi.compensation.x);
        let ypos = ypos.wrapping_sub(self.dvi.compensation.y);
        self.pdf_dev_set_rect(xpos, ypos, width, height, depth)
    }

    /// `dvi_do_special`.
    pub fn dvi_do_special(&mut self, buffer: &[u8]) -> Result<()> {
        self.graphics_mode()?;
        let x_user = f64::from(self.dvi.dvi_state.h) * self.dvi.dvi2pts;
        let y_user = -f64::from(self.dvi.dvi_state.v) * self.dvi.dvi2pts;
        let mag = self.dvi_tell_mag();
        let (r, is_drawable, rect) = self.spc_exec_special(buffer, x_user, y_user, mag)?;
        if r < 0 {
            return Ok(());
        }
        if self.dvi_is_tracking_boxes() && is_drawable != 0 {
            self.pdf_doc_expand_box(&rect);
        }
        Ok(())
    }
    /// `dvi_unit_size`.
    pub fn dvi_unit_size(&mut self) -> f64 {
        self.dvi.dvi2pts
    }
    /// `dvi_locate_font`: the index into `loaded_fonts` (C's `cur_id`).
    pub fn dvi_locate_font(&mut self, tfm_name: &[u8], ptsize: Spt) -> Result<i32> {
        let cur_id = self.dvi.loaded_fonts.len();
        self.dvi.loaded_fonts.push(LoadedFont::default());
        let mrec = self.pdf_lookup_fontmap_record(tfm_name);
        let subfont_id = -1;
        if let Some(m) = &mrec
            && m.charmap.sfd_name.is_some()
            && m.charmap.subfont_id.is_some()
        {
            crate::fatal!("Subfonts (SFD) are not supported");
        }
        let tfm_id = self.tfm_open(tfm_name, true)?;
        {
            let lf = &mut self.dvi.loaded_fonts[cur_id];
            *lf = LoadedFont::default();
            lf.tfm_id = tfm_id;
            lf.subfont_id = subfont_id;
            lf.size = ptsize;
            lf.source = VF;
        }
        if mrec.is_none() {
            let font_id = self.vf_locate_font(tfm_name, ptsize)?;
            if font_id >= 0 {
                let lf = &mut self.dvi.loaded_fonts[cur_id];
                lf.type_ = VIRTUAL;
                lf.font_id = font_id;
                return Ok(cur_id as i32);
            }
        }
        let name: Vec<u8> = match mrec.as_ref().and_then(|m| m.map_name.clone()) {
            Some(n) => n,
            None => tfm_name.to_vec(),
        };
        let font_id = self.pdf_dev_locate_font(&name, ptsize)?;
        if font_id < 0 {
            crate::fatal!(
                "Cannot proceed without .vf or \"physical\" font for PDF output... ({})",
                alloc::string::String::from_utf8_lossy(tfm_name)
            );
        }
        let mut padbytes = [0u8; 4];
        let mut is_unicode = 0;
        if let Some(m) = &mrec {
            if m.opt.mapc >= 0 {
                padbytes[2] = ((m.opt.mapc >> 8) & 0xff) as u8;
            }
            if m.enc_name.as_deref() == Some(b"unicode") {
                is_unicode = ENC_UNICODE;
                if m.opt.mapc >= 0 {
                    padbytes[0] = ((m.opt.mapc >> 24) & 0xff) as u8;
                    padbytes[1] = ((m.opt.mapc >> 16) & 0xff) as u8;
                }
            } else if m
                .enc_name
                .as_deref()
                .is_some_and(|e| e.windows(5).any(|w| w == b"UTF16"))
            {
                is_unicode = ENC_UTF16;
            }
        }
        let minbytes = self.pdf_dev_font_minbytes(font_id)?;
        let lf = &mut self.dvi.loaded_fonts[cur_id];
        lf.padbytes = padbytes;
        lf.is_unicode = is_unicode;
        lf.minbytes = minbytes.min(4);
        lf.type_ = PHYSICAL;
        lf.font_id = font_id;
        Ok(cur_id as i32)
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
    ) -> Result<i32> {
        let (path, is_dfont, is_type1) = if let Some(p) = self.dpx_find_dfont_file(filename)? {
            (p, true, false)
        } else if let Some(p) = self.dpx_find_type1_file(filename)? {
            (p, false, true)
        } else if let Some(p) = match self.dpx_find_opentype_file(filename)? {
            Some(p) => Some(p),
            None => self.dpx_find_truetype_file(filename)?,
        } {
            (p, false, false)
        } else {
            crate::fatal!(
                "Cannot proceed without the font: {}",
                alloc::string::String::from_utf8_lossy(filename)
            );
        };
        let cur_id = self.dvi.loaded_fonts.len();
        self.dvi.loaded_fonts.push(LoadedFont::default());
        let mut key = Vec::new();
        key.extend_from_slice(&path);
        let mut b = crate::fmt::Buf::new();
        b.push(b'/');
        b.uint(index);
        b.push(b'/');
        b.push(if layout_dir == 0 { b'H' } else { b'V' });
        b.push(b'/');
        b.int(extend);
        b.push(b'/');
        b.int(slant);
        b.push(b'/');
        b.int(embolden);
        key.extend_from_slice(&b.0);
        let mrec = match self.pdf_lookup_fontmap_record(&key) {
            Some(m) => m,
            None => match self
                .pdf_insert_native_fontmap_record(&path, index, layout_dir, extend, slant, embolden)
            {
                Some(m) => m,
                None => crate::fatal!("Failed to insert font record for font"),
            },
        };
        let font_id = self.pdf_dev_locate_font(&key, ptsize)?;
        let minbytes = self.pdf_dev_font_minbytes(font_id)?;
        {
            let lf = &mut self.dvi.loaded_fonts[cur_id];
            lf.font_id = font_id;
            lf.size = ptsize;
            lf.type_ = NATIVE;
            lf.minbytes = minbytes.min(4);
            lf.native_path = path.clone();
            lf.face_index = index;
        }
        let ext = mrec.opt.extend;
        if is_type1 {
            let Some(mut fp) = self.dpx_open_file(filename, crate::dpxfile::ResType::T1Font)?
            else {
                return Ok(-1);
            };
            if crate::t1_load::is_pfb(&mut fp) == 0 {
                crate::fatal!("Failed to read Type 1 font");
            }
            let mut enc_vec: Vec<Option<Vec<u8>>> = vec![None; 256];
            let Some(cffont) = self.t1_load_font(Some(&mut enc_vec), 0, &mut fp)? else {
                crate::fatal!("Failed to read Type 1 font");
            };
            let num_glyphs = cffont.num_glyphs;
            let shift = cffont.is_notdef_notzero;
            let mut gm = vec![Gm::default(); num_glyphs as usize + 1];
            let cstrings = cffont.cstrings.as_ref().expect("CharStrings");
            for i in 0..num_glyphs {
                let gid = if shift != 0 { i + 1 } else { i };
                if gid == num_glyphs {
                    break;
                }
                let g = gid as usize;
                let start = cstrings.offset[g] as usize - 1;
                let end = cstrings.offset[g + 1] as usize - 1;
                let mut ginfo = crate::t1_char::T1Ginfo::default();
                let subrs = cffont.subrs.first().and_then(Option::as_ref);
                self.t1char_get_metrics(&cstrings.data[start..end], subrs, Some(&mut ginfo))?;
                let advance = if layout_dir == 0 { ginfo.wx } else { ginfo.wy };
                let ascent = ginfo.bbox.ury;
                let descent = ginfo.bbox.lly;
                let size = f64::from(ptsize);
                gm[g].advance = (size * ((advance / 1000.0) * ext)) as Spt;
                gm[g].ascent = (size * (ascent / 1000.0)) as Spt;
                gm[g].descent = (size * (descent / 1000.0)) as Spt;
            }
            let lf = &mut self.dvi.loaded_fonts[cur_id];
            lf.shift_gid = shift;
            lf.num_glyphs = num_glyphs;
            lf.gm = gm;
        } else {
            let Some(data) = self.files.read(&path) else {
                crate::fatal!("Cannot proceed without the font");
            };
            let fp = MemFile::new(data, &path);
            let sfont = if is_dfont {
                crate::sfnt::Sfnt::dfont_open(fp, index as i32)?
            } else {
                crate::sfnt::Sfnt::sfnt_open(fp)?
            };
            let Some(mut sfont) = sfont else {
                crate::fatal!("Failed to open the font");
            };
            let offset = if sfont.type_ == crate::sfnt::SFNT_TYPE_TTC {
                sfont.ttc_read_offset(index)?
            } else if sfont.type_ == crate::sfnt::SFNT_TYPE_DFONT {
                sfont.offset
            } else {
                0
            };
            sfont.sfnt_read_table_directory(offset)?;
            let head = sfont.tt_read_head_table()?;
            let maxp = sfont.tt_read_maxp_table()?;
            let hhea = sfont.tt_read_hhea_table()?;
            let size = f64::from(ptsize);
            let upem = f64::from(head.units_per_em);
            let ascent = (size * (f64::from(hhea.ascent) / upem)) as Spt;
            let descent = (size * (f64::from(hhea.descent) / upem)) as Spt;
            let num_glyphs = maxp.num_glyphs;
            let metrics = if layout_dir == 1 && sfont.sfnt_find_table_pos(b"vmtx") > 0 {
                let vhea = sfont.tt_read_vhea_table()?;
                sfont.sfnt_locate_table(b"vmtx")?;
                sfont.tt_read_longMetrics(
                    num_glyphs,
                    vhea.num_of_long_ver_metrics,
                    vhea.num_of_ex_side_bearings,
                )?
            } else {
                sfont.sfnt_locate_table(b"hmtx")?;
                sfont.tt_read_longMetrics(
                    num_glyphs,
                    hhea.num_of_long_hor_metrics,
                    hhea.num_of_ex_side_bearings,
                )?
            };
            let mut gm = vec![Gm::default(); num_glyphs as usize];
            for i in 0..num_glyphs as usize {
                gm[i].advance = (size * (f64::from(metrics[i].advance) / upem) * ext) as Spt;
                gm[i].ascent = ascent;
                gm[i].descent = descent;
            }
            let lf = &mut self.dvi.loaded_fonts[cur_id];
            lf.num_glyphs = num_glyphs;
            lf.gm = gm;
        }
        let lf = &mut self.dvi.loaded_fonts[cur_id];
        lf.layout_dir = layout_dir;
        lf.extend = mrec.opt.extend as f32;
        lf.slant = mrec.opt.slant as f32;
        lf.embolden = mrec.opt.bold as f32;
        Ok(cur_id as i32)
    }

    /// `dvi_dev_xpos`.
    pub fn dvi_dev_xpos(&mut self) -> f64 {
        f64::from(self.dvi.dvi_state.h) * self.dvi.dvi2pts
    }
    /// `dvi_dev_ypos`.
    pub fn dvi_dev_ypos(&mut self) -> f64 {
        -(f64::from(self.dvi.dvi_state.v) * self.dvi.dvi2pts)
    }
    /// `do_moveto`.
    fn do_moveto(&mut self, x: i32, y: i32) {
        self.dvi.dvi_state.h = x;
        self.dvi.dvi_state.v = y;
    }
    /// `dvi_right`.
    pub fn dvi_right(&mut self, x: i32) {
        if self.dvi.lr_mode >= SKIMMING {
            self.dvi.lr_width = self.dvi.lr_width.wrapping_add(x as u32);
            return;
        }
        let x = if self.dvi.lr_mode == RTYPESETTING {
            x.wrapping_neg()
        } else {
            x
        };
        let save_h = self.dvi.dvi_state.h;
        let save_v = self.dvi.dvi_state.v;
        let st = &mut self.dvi.dvi_state;
        match st.d {
            0 => st.h = st.h.wrapping_add(x),
            1 => st.v = st.v.wrapping_add(x),
            3 => st.v = st.v.wrapping_sub(x),
            _ => {}
        }
        if self.dvi_is_tracking_boxes() && self.dvi.catch_phantom > 0 {
            let (height, depth): (Spt, Spt) = if self.dvi.catch_phantom == 1 {
                let cf = self.dvi.current_font;
                let h = if cf >= 0 && (cf as usize) < self.dvi.loaded_fonts.len() {
                    self.dvi.loaded_fonts[cf as usize].size
                } else {
                    0
                };
                (h, 0)
            } else {
                (
                    (self.dvi.phantom_height / self.dvi.dvi2pts) as Spt,
                    (self.dvi.phantom_depth / self.dvi.dvi2pts) as Spt,
                )
            };
            let st = self.dvi.dvi_state;
            let width = match st.d {
                1 | 2 => st.v.wrapping_sub(save_v),
                _ => st.h.wrapping_sub(save_h),
            };
            let rect = self.calc_rect(save_h, save_v.wrapping_neg(), width, height, depth);
            self.pdf_doc_expand_box(&rect);
        }
    }
    /// `dvi_down`.
    pub fn dvi_down(&mut self, y: i32) {
        if self.dvi.lr_mode < SKIMMING {
            let st = &mut self.dvi.dvi_state;
            match st.d {
                0 => st.v = st.v.wrapping_add(y),
                1 => st.h = st.h.wrapping_sub(y),
                3 => st.h = st.h.wrapping_add(y),
                _ => {}
            }
        }
    }

    /// The code bytes `dvi_set`/`dvi_put` show for `ch` (`wbuf`, `n`).
    fn char_bytes(&mut self, font: usize, ch: i32) -> Result<([u8; 4], usize)> {
        let lf = &self.dvi.loaded_fonts[font];
        let mut wbuf = lf.padbytes;
        let n;
        if ch > 65535 {
            let (is_unicode, tfm_id) = (lf.is_unicode, lf.tfm_id);
            if is_unicode == ENC_UTF16 && self.tfm_is_jfm(tfm_id)? != 0 {
                let hs = utf32_to_utf16_hs(ch as u32);
                let ls = utf32_to_utf16_ls(ch as u32);
                wbuf = [(hs >> 8) as u8, hs as u8, (ls >> 8) as u8, ls as u8];
            } else {
                wbuf = [
                    (ch >> 24) as u8,
                    (ch >> 16) as u8,
                    (ch >> 8) as u8,
                    ch as u8,
                ];
            }
            n = 4;
        } else if ch > 255 {
            wbuf[2] = (ch >> 8) as u8;
            wbuf[3] = ch as u8;
            n = 2;
        } else if lf.subfont_id >= 0 {
            crate::fatal!("Subfonts (SFD) are not supported");
        } else {
            wbuf[3] = ch as u8;
            n = 1;
        }
        Ok((wbuf, n))
    }

    /// `dvi_set`.
    pub fn dvi_set(&mut self, ch: i32) -> Result<()> {
        if self.dvi.current_font < 0 {
            crate::fatal!("No font selected!");
        }
        let cf = self.dvi.current_font as usize;
        let (ftype, num_glyphs, tfm_id, size) = {
            let f = &self.dvi.loaded_fonts[cf];
            (f.type_, f.num_glyphs, f.tfm_id, f.size)
        };
        let width = if ftype == NATIVE {
            if ch >= 0 && ch < i32::from(num_glyphs) {
                self.dvi.loaded_fonts[cf].gm[ch as usize].advance
            } else {
                return Ok(());
            }
        } else {
            let w = self.tfm_get_fw_width(tfm_id, ch)?;
            crate::stream::sqxfw(size, w)
        };
        if self.dvi.lr_mode >= SKIMMING {
            self.dvi.lr_width = self.dvi.lr_width.wrapping_add(width as u32);
            return Ok(());
        }
        if self.dvi.lr_mode == RTYPESETTING {
            self.dvi_right(width);
        }
        match ftype {
            PHYSICAL => {
                let (wbuf, n) = self.char_bytes(cf, ch)?;
                let (minbytes, font_id) = (
                    self.dvi.loaded_fonts[cf].minbytes,
                    self.dvi.loaded_fonts[cf].font_id,
                );
                let cbytes = (minbytes as usize).max(n);
                let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
                self.set_string(h, v.wrapping_neg(), &wbuf[4 - cbytes..], width, font_id)?;
                if self.dvi_is_tracking_boxes() {
                    let height = self.tfm_get_fw_height(tfm_id, ch)?;
                    let depth = self.tfm_get_fw_depth(tfm_id, ch)?;
                    let height = crate::stream::sqxfw(size, height);
                    let depth = crate::stream::sqxfw(size, depth);
                    let rect = self.calc_rect(h, v.wrapping_neg(), width, height, depth);
                    self.pdf_doc_expand_box(&rect);
                }
            }
            NATIVE => {
                let wbuf = [(ch >> 8) as u8, ch as u8];
                let font_id = self.dvi.loaded_fonts[cf].font_id;
                let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
                self.set_string(h, v.wrapping_neg(), &wbuf, width, font_id)?;
                if self.dvi_is_tracking_boxes() {
                    let g = self.dvi.loaded_fonts[cf].gm[ch as usize];
                    let rect = self.calc_rect(h, v.wrapping_neg(), width, g.ascent, g.descent);
                    self.pdf_doc_expand_box(&rect);
                }
            }
            VIRTUAL => {
                let font_id = self.dvi.loaded_fonts[cf].font_id;
                self.vf_set_char(ch, font_id)?;
            }
            _ => {}
        }
        if self.dvi.lr_mode == LTYPESETTING {
            self.dvi_right(width);
        }
        Ok(())
    }
    /// `dvi_put`.
    pub fn dvi_put(&mut self, ch: i32) -> Result<()> {
        if self.dvi.current_font < 0 {
            crate::fatal!("No font selected!");
        }
        let cf = self.dvi.current_font as usize;
        let (ftype, tfm_id, size, font_id) = {
            let f = &self.dvi.loaded_fonts[cf];
            (f.type_, f.tfm_id, f.size, f.font_id)
        };
        match ftype {
            PHYSICAL => {
                let w = self.tfm_get_fw_width(tfm_id, ch)?;
                let width = crate::stream::sqxfw(size, w);
                let (wbuf, n) = self.char_bytes(cf, ch)?;
                let minbytes = self.dvi.loaded_fonts[cf].minbytes;
                let cbytes = (minbytes as usize).max(n);
                let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
                self.set_string(h, v.wrapping_neg(), &wbuf[4 - cbytes..], width, font_id)?;
                if self.dvi_is_tracking_boxes() {
                    let height = self.tfm_get_fw_height(tfm_id, ch)?;
                    let depth = self.tfm_get_fw_depth(tfm_id, ch)?;
                    let height = crate::stream::sqxfw(size, height);
                    let depth = crate::stream::sqxfw(size, depth);
                    let rect = self.calc_rect(h, v.wrapping_neg(), width, height, depth);
                    self.pdf_doc_expand_box(&rect);
                }
            }
            VIRTUAL => self.vf_set_char(ch, font_id)?,
            _ => {}
        }
        Ok(())
    }
    /// `dvi_rule`.
    pub fn dvi_rule(&mut self, width: i32, height: i32) -> Result<()> {
        if width > 0 && height > 0 {
            let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
            self.do_moveto(h, v);
            match self.dvi.dvi_state.d {
                0 => self.set_rule(h, v.wrapping_neg(), width, height)?,
                1 => self.set_rule(h, v.wrapping_neg().wrapping_sub(width), height, width)?,
                3 => self.set_rule(h.wrapping_sub(height), v.wrapping_neg(), height, width)?,
                _ => {}
            }
        }
        Ok(())
    }
    /// `dvi_dirchg`.
    pub fn dvi_dirchg(&mut self, dir: u8) {
        self.dvi.dvi_state.d = u32::from(dir);
        self.pdf_dev_set_dirmode(i32::from(dir));
    }
    /// `do_setrule`.
    fn do_setrule(&mut self) -> Result<()> {
        let height = self.get_buffered_signed_quad();
        let width = self.get_buffered_signed_quad();
        match self.dvi.lr_mode {
            LTYPESETTING => {
                self.dvi_rule(width, height)?;
                self.dvi_right(width);
            }
            RTYPESETTING => {
                self.dvi_right(width);
                self.dvi_rule(width, height)?;
            }
            _ => self.dvi.lr_width = self.dvi.lr_width.wrapping_add(width as u32),
        }
        Ok(())
    }
    /// `do_putrule`.
    fn do_putrule(&mut self) -> Result<()> {
        let height = self.get_buffered_signed_quad();
        let width = self.get_buffered_signed_quad();
        match self.dvi.lr_mode {
            LTYPESETTING => self.dvi_rule(width, height)?,
            RTYPESETTING => {
                self.dvi_right(width);
                self.dvi_rule(width, height)?;
                self.dvi_right(width.wrapping_neg());
            }
            _ => {}
        }
        Ok(())
    }
    /// `dvi_push`.
    pub fn dvi_push(&mut self) -> Result<()> {
        if self.dvi.dvi_stack_depth as usize >= DVI_STACK_DEPTH_MAX {
            crate::fatal!("DVI stack exceeded limit.");
        }
        let d = self.dvi.dvi_stack_depth as usize;
        self.dvi.dvi_stack[d] = self.dvi.dvi_state;
        self.dvi.dvi_stack_depth += 1;
        Ok(())
    }
    /// `dvi_pop`.
    pub fn dvi_pop(&mut self) -> Result<()> {
        if self.dvi.dvi_stack_depth == 0 {
            crate::fatal!("Tried to pop an empty stack.");
        }
        self.dvi.dvi_stack_depth -= 1;
        self.dvi.dvi_state = self.dvi.dvi_stack[self.dvi.dvi_stack_depth as usize];
        let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
        self.do_moveto(h, v);
        self.pdf_dev_set_dirmode(self.dvi.dvi_state.d as i32);
        Ok(())
    }
    /// `dvi_w`.
    pub fn dvi_w(&mut self, ch: i32) {
        self.dvi.dvi_state.w = ch;
        self.dvi_right(ch);
    }
    /// `dvi_w0`.
    pub fn dvi_w0(&mut self) {
        self.dvi_right(self.dvi.dvi_state.w);
    }
    /// `dvi_x`.
    pub fn dvi_x(&mut self, ch: i32) {
        self.dvi.dvi_state.x = ch;
        self.dvi_right(ch);
    }
    /// `dvi_x0`.
    pub fn dvi_x0(&mut self) {
        self.dvi_right(self.dvi.dvi_state.x);
    }
    /// `dvi_y`.
    pub fn dvi_y(&mut self, ch: i32) {
        self.dvi.dvi_state.y = ch;
        self.dvi_down(ch);
    }
    /// `dvi_y0`.
    pub fn dvi_y0(&mut self) {
        self.dvi_down(self.dvi.dvi_state.y);
    }
    /// `dvi_z`.
    pub fn dvi_z(&mut self, ch: i32) {
        self.dvi.dvi_state.z = ch;
        self.dvi_down(ch);
    }
    /// `dvi_z0`.
    pub fn dvi_z0(&mut self) {
        self.dvi_down(self.dvi.dvi_state.z);
    }
    /// `skip_fntdef`.
    fn skip_fntdef(&mut self) -> Result<()> {
        let f = self.dvi_file();
        f.skip_bytes(12)?;
        let area_len = f.get_unsigned_byte()? as usize;
        let name_len = f.get_unsigned_byte()? as usize;
        f.skip_bytes(area_len + name_len)?;
        Ok(())
    }
    /// `do_fntdef`.
    fn do_fntdef(&mut self, tex_id: i32) -> Result<()> {
        if self.dvi.linear != 0 {
            self.read_font_record(tex_id)?;
        } else {
            self.skip_fntdef()?;
        }
        self.dvi.dvi_page_buf_index -= 1;
        Ok(())
    }
    /// `dvi_set_font`.
    pub fn dvi_set_font(&mut self, font_id: i32) {
        self.dvi.current_font = font_id;
    }
    /// `do_fnt`.
    fn do_fnt(&mut self, tex_id: i32) -> Result<()> {
        let Some(i) = self.dvi.def_fonts.iter().position(|d| d.tex_id == tex_id) else {
            crate::fatal!(
                "Tried to select a font that hasn't been defined: id={}",
                tex_id
            );
        };
        if self.dvi.def_fonts[i].used == 0 {
            let d = self.dvi.def_fonts[i].clone();
            let font_id = if d.native != 0 {
                self.dvi_locate_native_font(
                    &d.font_name,
                    d.face_index,
                    d.point_size,
                    d.layout_dir,
                    d.extend,
                    d.slant,
                    d.embolden,
                )?
            } else {
                self.dvi_locate_font(&d.font_name, d.point_size)?
            };
            let fi = font_id as usize;
            self.dvi.loaded_fonts[fi].rgba_color = d.rgba_color;
            self.dvi.loaded_fonts[fi].rgba_used = d.rgba_used;
            if d.rgba_used == 0 {
                self.dvi.loaded_fonts[fi].xgs_id = -1;
            } else {
                let a = d.rgba_color & 0xff;
                let xgs_dict = self.o.new_dict();
                self.o.put_name(xgs_dict, b"Type", b"ExtGState")?;
                self.o.put_number(xgs_dict, b"ca", f64::from(a) / 255.0)?;
                self.o.put_number(xgs_dict, b"CA", f64::from(a) / 255.0)?;
                self.dvi.loaded_fonts[fi].xgs_id =
                    self.pdf_defineresource(b"ExtGState", None, xgs_dict, 0)?;
            }
            self.dvi.loaded_fonts[fi].source = DVI;
            self.dvi.def_fonts[i].used = 1;
            self.dvi.def_fonts[i].font_id = font_id;
        }
        self.dvi.current_font = self.dvi.def_fonts[i].font_id;
        Ok(())
    }
    /// `do_xxx`.
    fn do_xxx(&mut self, size: i32) -> Result<()> {
        let start = self.dvi.dvi_page_buf_index as usize;
        if self.dvi.lr_mode < SKIMMING {
            let buf = self.dvi.dvi_page_buffer[start..start + size as usize].to_vec();
            self.dvi_do_special(&buf)?;
        }
        self.dvi.dvi_page_buf_index += size as u32;
        Ok(())
    }
    /// `do_bop`.
    fn do_bop(&mut self) -> Result<()> {
        if self.dvi.processing_page != 0 {
            crate::fatal!("Got a bop in the middle of a page!");
        }
        self.dvi.dvi_page_buf_index += 44;
        self.clear_state();
        self.dvi.processing_page = 1;
        let mag = self.dvi_tell_mag();
        let (ox, oy) = (self.dvi.dev_origin_x, self.dvi.dev_origin_y);
        self.pdf_doc_begin_page(mag, ox, oy)?;
        self.spc_exec_at_begin_page()?;
        Ok(())
    }
    /// `do_eop`.
    fn do_eop(&mut self) -> Result<()> {
        self.dvi.processing_page = 0;
        if self.dvi.dvi_stack_depth != 0 {
            crate::fatal!("DVI stack depth is not zero at end of page");
        }
        self.spc_exec_at_end_page()?;
        self.pdf_doc_end_page()?;
        Ok(())
    }
    /// `do_dir`.
    fn do_dir(&mut self) {
        self.dvi.dvi_state.d = self.get_buffered_unsigned_byte() as u32;
        self.pdf_dev_set_dirmode(self.dvi.dvi_state.d as i32);
    }
    /// `lr_width_push`.
    fn lr_width_push(&mut self) -> Result<()> {
        if self.dvi.lr_width_stack_depth as usize >= DVI_STACK_DEPTH_MAX {
            crate::fatal!("Segment width stack exceeded limit.");
        }
        let d = self.dvi.lr_width_stack_depth as usize;
        self.dvi.lr_width_stack[d] = self.dvi.lr_width;
        self.dvi.lr_width_stack_depth += 1;
        Ok(())
    }
    /// `lr_width_pop`.
    fn lr_width_pop(&mut self) -> Result<()> {
        if self.dvi.lr_width_stack_depth == 0 {
            crate::fatal!("Tried to pop an empty segment width stack.");
        }
        self.dvi.lr_width_stack_depth -= 1;
        self.dvi.lr_width = self.dvi.lr_width_stack[self.dvi.lr_width_stack_depth as usize];
        Ok(())
    }
    /// `dvi_begin_reflect`.
    fn dvi_begin_reflect(&mut self) {
        if self.dvi.lr_mode >= SKIMMING {
            self.dvi.lr_mode += 1;
        } else {
            self.dvi.lr_state.buf_index = self.dvi.dvi_page_buf_index;
            self.dvi.lr_state.font = self.dvi.current_font;
            self.dvi.lr_state.state = self.dvi.lr_mode;
            self.dvi.lr_mode = SKIMMING;
            self.dvi.lr_width = 0;
        }
    }
    /// `dvi_end_reflect`.
    fn dvi_end_reflect(&mut self) -> Result<()> {
        match self.dvi.lr_mode {
            SKIMMING => {
                self.dvi.current_font = self.dvi.lr_state.font;
                self.dvi.dvi_page_buf_index = self.dvi.lr_state.buf_index;
                self.dvi.lr_mode = LTYPESETTING + RTYPESETTING - self.dvi.lr_state.state;
                self.dvi_right((self.dvi.lr_width as i32).wrapping_neg());
                self.lr_width_push()?;
            }
            LTYPESETTING | RTYPESETTING => {
                self.lr_width_pop()?;
                self.dvi_right((self.dvi.lr_width as i32).wrapping_neg());
                self.dvi.lr_mode = LTYPESETTING + RTYPESETTING - self.dvi.lr_mode;
            }
            _ => self.dvi.lr_mode -= 1,
        }
        Ok(())
    }
    /// `skip_native_font_def`.
    fn skip_native_font_def(&mut self) -> Result<()> {
        let f = self.dvi_file();
        f.skip_bytes(4)?;
        let flags = f.get_unsigned_pair()?;
        let name_length = f.get_unsigned_byte()? as usize;
        f.skip_bytes(name_length + 4)?;
        for flag in [
            XDV_FLAG_COLORED,
            XDV_FLAG_EXTEND,
            XDV_FLAG_SLANT,
            XDV_FLAG_EMBOLDEN,
        ] {
            if flags & flag != 0 {
                f.skip_bytes(4)?;
            }
        }
        Ok(())
    }
    /// `do_native_font_def`.
    fn do_native_font_def(&mut self, tex_id: i32) -> Result<()> {
        if self.dvi.linear != 0 {
            self.read_native_font_record(tex_id)?;
        } else {
            self.skip_native_font_def()?;
        }
        self.dvi.dvi_page_buf_index -= 1;
        Ok(())
    }
    /// `skip_glyphs`.
    fn skip_glyphs(&mut self) {
        let slen = self.get_buffered_unsigned_pair();
        self.dvi.dvi_page_buf_index += slen * 10;
    }
    /// `do_glyphs`.
    fn do_glyphs(&mut self, do_actual_text: i32) -> Result<()> {
        if self.dvi.current_font < 0 {
            crate::fatal!("No font selected!");
        }
        let cf = self.dvi.current_font as usize;
        if do_actual_text != 0 {
            let slen = self.get_buffered_unsigned_pair();
            if self.dvi.lr_mode >= SKIMMING {
                self.dvi.dvi_page_buf_index += slen * 2;
            } else {
                let mut unicodes = Vec::with_capacity(slen as usize);
                for _ in 0..slen {
                    unicodes.push(self.get_buffered_unsigned_pair() as u16);
                }
                self.pdf_dev_begin_actualtext(&unicodes)?;
            }
        }
        let width = self.get_buffered_signed_quad();
        if self.dvi.lr_mode >= SKIMMING {
            self.dvi.lr_width = self.dvi.lr_width.wrapping_add(width as u32);
            self.skip_glyphs();
            return Ok(());
        }
        if self.dvi.lr_mode == RTYPESETTING {
            self.dvi_right(width);
        }
        let slen = self.get_buffered_unsigned_pair() as usize;
        let mut xloc = Vec::with_capacity(slen);
        let mut yloc = Vec::with_capacity(slen);
        for _ in 0..slen {
            xloc.push(self.get_buffered_signed_quad());
            yloc.push(self.get_buffered_signed_quad());
        }
        let (rgba_used, rgba_color, xgs_id) = {
            let f = &self.dvi.loaded_fonts[cf];
            (f.rgba_used, f.rgba_color, f.xgs_id)
        };
        if rgba_used == 1 {
            let mut color = crate::pdfcolor::PdfColor::default();
            color.pdf_color_rgbcolor(
                f64::from((rgba_color >> 24) as u8) / 255.0,
                f64::from((rgba_color >> 16) as u8) / 255.0,
                f64::from((rgba_color >> 8) as u8) / 255.0,
            );
            self.pdf_color_push(&color, &color)?;
            if xgs_id >= 0 {
                let mut resname = crate::fmt::Buf::new();
                resname.extend(b"Xtx_Gs_");
                resname.hex_padded(cf as u32, 8);
                let r = self.pdf_get_resource_reference(xgs_id)?.expect("reference");
                self.pdf_doc_add_page_resource(b"ExtGState", &resname.0, r)?;
                self.graphics_mode()?;
                self.pdf_dev_gsave()?;
                let mut content = crate::fmt::Buf::new();
                content.extend(b" /");
                content.extend(&resname.0);
                content.extend(b" gs ");
                self.pdf_doc_add_page_content(&content.0)?;
            }
        }
        let (num_glyphs, shift_gid, font_id, size) = {
            let f = &self.dvi.loaded_fonts[cf];
            (f.num_glyphs, f.shift_gid, f.font_id, f.size)
        };
        for i in 0..slen {
            let mut glyph_id = self.get_buffered_unsigned_pair() as u16;
            let mut advance: Spt = 0;
            let (h, v) = (self.dvi.dvi_state.h, self.dvi.dvi_state.v);
            if glyph_id < num_glyphs {
                if shift_gid != 0 {
                    glyph_id += 1;
                }
                let g = self.dvi.loaded_fonts[cf].gm[glyph_id as usize];
                advance = g.advance;
                if self.dvi_is_tracking_boxes() {
                    let rect = self.calc_rect(
                        h.wrapping_add(xloc[i]),
                        v.wrapping_neg().wrapping_sub(yloc[i]),
                        advance,
                        g.ascent,
                        g.descent.wrapping_neg(),
                    );
                    self.pdf_doc_expand_box(&rect);
                }
            }
            let wbuf = [(glyph_id >> 8) as u8, glyph_id as u8];
            let (x, y) = (
                h.wrapping_add(xloc[i]),
                v.wrapping_neg().wrapping_sub(yloc[i]),
            );
            self.record_glyph_run(cf, glyph_id, x, y, size, rgba_used, rgba_color);
            self.set_string(x, y, &wbuf, advance, font_id)?;
        }
        if rgba_used == 1 {
            if xgs_id >= 0 {
                self.graphics_mode()?;
                self.pdf_dev_grestore()?;
            }
            self.pdf_color_pop()?;
        }
        if do_actual_text != 0 {
            self.pdf_dev_end_actualtext()?;
        }
        if self.dvi.lr_mode == LTYPESETTING {
            self.dvi_right(width);
        }
        Ok(())
    }

    /// The side output: a glyph of a native font at DVI position
    /// (`x`, `y`) (y up), in the page's default user space.
    fn record_glyph_run(
        &mut self,
        cf: usize,
        glyph: u16,
        x: Spt,
        y: Spt,
        size: Spt,
        rgba_used: u8,
        rgba: u32,
    ) {
        let lf = &self.dvi.loaded_fonts[cf];
        if lf.type_ != NATIVE {
            return;
        }
        let (d, mag) = (self.dvi.dvi2pts, self.dvi.total_mag);
        let run = GlyphRun {
            font_file: lf.native_path.clone(),
            face_index: lf.face_index,
            glyph,
            x: self.dvi.dev_origin_x
                + mag * (f64::from(x.wrapping_sub(self.dvi.compensation.x)) * d),
            y: self.dvi.dev_origin_y
                + mag * (f64::from(y.wrapping_sub(self.dvi.compensation.y)) * d),
            size: mag * f64::from(size) * d,
            rgba: if rgba_used == 1 { rgba } else { 0x0000_00ff },
        };
        self.dvi.glyph_runs.push(run);
    }

    /// `check_postamble`.
    fn check_postamble(&mut self) -> Result<()> {
        self.dvi_file().skip_bytes(28)?;
        loop {
            let code = self.dvi_file().get_unsigned_byte()?;
            if code == POST_POST {
                break;
            }
            match code {
                FNT_DEF1..=FNT_DEF4 => {
                    self.dvi_file().skip_bytes((code + 1 - FNT_DEF1) as usize)?;
                    self.skip_fntdef()?;
                }
                XDV_NATIVE_FONT_DEF => {
                    self.dvi_file().skip_bytes(4)?;
                    self.skip_native_font_def()?;
                }
                _ => crate::fatal!("Unexpected op code ({}) in postamble", code),
            }
        }
        self.dvi_file().skip_bytes(4)?;
        let post_id_byte = self.dvi_file().get_unsigned_byte()?;
        self.dvi.post_id_byte = i32::from(post_id_byte);
        if !(post_id_byte == DVI_ID
            || post_id_byte == DVIV_ID
            || post_id_byte == XDV_ID
            || post_id_byte == XDV_ID_OLD)
        {
            crate::fatal!("DVI ID = {}", post_id_byte);
        }
        self.check_id_bytes()?;
        if self.dvi.has_ptex != 0 && post_id_byte != DVIV_ID {
            crate::fatal!("Saw opcode {} in DVI file not for Ascii pTeX", PTEXDIR);
        }
        self.dvi.num_pages = 0;
        Ok(())
    }
    /// `dvi_do_page`.
    pub fn dvi_do_page(
        &mut self,
        page_paper_height: f64,
        hmargin: f64,
        vmargin: f64,
    ) -> Result<()> {
        self.dvi.dvi_page_buf_index = 0;
        self.dvi.dev_origin_x = hmargin;
        self.dvi.dev_origin_y = page_paper_height - vmargin;
        self.dvi.dvi_stack_depth = 0;
        loop {
            let opcode = self.get_buffered_unsigned_byte() as u8;
            if opcode <= SET_CHAR_127 {
                self.dvi_set(i32::from(opcode))?;
                continue;
            }
            if (FNT_NUM_0..=FNT_NUM_63).contains(&opcode) {
                self.do_fnt(i32::from(opcode - FNT_NUM_0))?;
                continue;
            }
            match opcode {
                SET1 | SET2 | SET3 => {
                    let c = self.get_buffered_unsigned_num(opcode - SET1);
                    self.dvi_set(c)?;
                }
                SET4 => crate::fatal!("Multibyte (>24 bits) character not supported!"),
                SET_RULE => self.do_setrule()?,
                PUT1 | PUT2 | PUT3 => {
                    let c = self.get_buffered_unsigned_num(opcode - PUT1);
                    self.dvi_put(c)?;
                }
                PUT4 => crate::fatal!("Multibyte (>24 bits) character not supported!"),
                PUT_RULE => self.do_putrule()?,
                NOP => {}
                BOP => self.do_bop()?,
                EOP => {
                    self.do_eop()?;
                    if self.dvi.linear != 0 && self.dvi.peek_after_eop {
                        let c = self.dvi_file().get_unsigned_byte()?;
                        if c == POST {
                            self.check_postamble()?;
                        } else {
                            self.dvi_file().ungetc();
                        }
                    }
                    return Ok(());
                }
                PUSH => {
                    self.dvi_push()?;
                    if self.dvi.lr_mode >= SKIMMING {
                        self.lr_width_push()?;
                    }
                    self.dvi_mark_depth()?;
                }
                POP => {
                    self.dvi_pop()?;
                    if self.dvi.lr_mode >= SKIMMING {
                        self.lr_width_pop()?;
                    }
                    self.dvi_mark_depth()?;
                }
                RIGHT1..=RIGHT4 => {
                    let v = self.get_buffered_signed_num(opcode - RIGHT1);
                    self.dvi_right(v);
                }
                W0 => self.dvi_w0(),
                W1..=W4 => {
                    let v = self.get_buffered_signed_num(opcode - W1);
                    self.dvi_w(v);
                }
                X0 => self.dvi_x0(),
                X1..=X4 => {
                    let v = self.get_buffered_signed_num(opcode - X1);
                    self.dvi_x(v);
                }
                DOWN1..=DOWN4 => {
                    let v = self.get_buffered_signed_num(opcode - DOWN1);
                    self.dvi_down(v);
                }
                Y0 => self.dvi_y0(),
                Y1..=Y4 => {
                    let v = self.get_buffered_signed_num(opcode - Y1);
                    self.dvi_y(v);
                }
                Z0 => self.dvi_z0(),
                Z1..=Z4 => {
                    let v = self.get_buffered_signed_num(opcode - Z1);
                    self.dvi_z(v);
                }
                FNT1..=FNT4 => {
                    let v = self.get_buffered_unsigned_num(opcode - FNT1);
                    self.do_fnt(v)?;
                }
                XXX1..=XXX4 => {
                    let size = self.get_buffered_unsigned_num(opcode - XXX1);
                    if size >= 0 {
                        self.do_xxx(size)?;
                    }
                }
                FNT_DEF1..=FNT_DEF4 => {}
                PTEXDIR => {
                    self.need_ptex(i32::from(opcode))?;
                    self.do_dir();
                }
                XDV_GLYPHS => {
                    self.need_xetex(i32::from(opcode))?;
                    self.do_glyphs(0)?;
                }
                XDV_TEXT_AND_GLYPHS => {
                    self.need_xetex(i32::from(opcode))?;
                    self.do_glyphs(1)?;
                }
                XDV_NATIVE_FONT_DEF => self.need_xetex(i32::from(opcode))?,
                BEGIN_REFLECT => {
                    self.need_xetex(i32::from(opcode))?;
                    self.dvi_begin_reflect();
                }
                END_REFLECT => {
                    self.need_xetex(i32::from(opcode))?;
                    self.dvi_end_reflect()?;
                }
                POST if self.dvi.linear != 0 && self.dvi.processing_page == 0 => {
                    self.dvi.num_pages = 0;
                    return Ok(());
                }
                PRE | POST | POST_POST => {
                    crate::fatal!("Unexpected preamble or postamble in dvi file")
                }
                _ => crate::fatal!("Unexpected opcode or DVI file ended prematurely"),
            }
        }
        Ok(())
    }
    /// `dvi_init`: the scale (dvi2pts). `dvi_filename` none: the XDV is
    /// already in `self.dvi.dvi_file` (C's stdin, linear processing).
    pub fn dvi_init(&mut self, dvi_filename: Option<&[u8]>, mag: f64) -> Result<f64> {
        if dvi_filename.is_none() {
            self.dvi.linear = 1;
            self.get_preamble_dvi_info()?;
            self.do_scales(mag);
            if !self.dvi_file().eof() {
                let c = self.dvi_file().get_unsigned_byte()?;
                if c == POST {
                    self.check_postamble()?;
                } else {
                    self.dvi_file().ungetc();
                }
            }
        } else {
            let post_location = self.find_post()?;
            self.get_dvi_info(post_location)?;
            self.do_scales(mag);
            self.get_page_info(post_location)?;
            self.get_comment()?;
            self.get_dvi_fonts(post_location)?;
        }
        self.clear_state();
        self.dvi.dvi_page_buffer = vec![0; DVI_PAGE_BUF_CHUNK as usize];
        self.dvi.dvi_page_buf_size = DVI_PAGE_BUF_CHUNK;
        Ok(self.dvi.dvi2pts)
    }
    /// `dvi_close`.
    pub fn dvi_close(&mut self) {
        self.dvi.dvi_file = None;
        self.dvi.def_fonts.clear();
        self.dvi.page_loc.clear();
        self.dvi.num_pages = 0;
        self.dvi.loaded_fonts.clear();
        self.vf_close_all_fonts();
        self.tfm_close_all();
        self.dvi.dvi_page_buffer.clear();
        self.dvi.dvi_page_buf_size = 0;
    }
    /// `dvi_vf_init`.
    pub fn dvi_vf_init(&mut self, dev_font_id: i32) -> Result<()> {
        self.dvi_push()?;
        let st = &mut self.dvi.dvi_state;
        st.w = 0;
        st.x = 0;
        st.y = 0;
        st.z = 0;
        if (self.dvi.num_saved_fonts as usize) < VF_NESTING_MAX {
            let n = self.dvi.num_saved_fonts as usize;
            self.dvi.saved_dvi_font[n] = self.dvi.current_font;
            self.dvi.num_saved_fonts += 1;
        } else {
            crate::fatal!("Virtual fonts nested too deeply!");
        }
        self.dvi.current_font = dev_font_id;
        Ok(())
    }
    /// `dvi_vf_finish`.
    pub fn dvi_vf_finish(&mut self) -> Result<()> {
        self.dvi_pop()?;
        if self.dvi.num_saved_fonts > 0 {
            self.dvi.num_saved_fonts -= 1;
            self.dvi.current_font = self.dvi.saved_dvi_font[self.dvi.num_saved_fonts as usize];
        } else {
            crate::fatal!("Tried to pop an empty font stack");
        }
        Ok(())
    }

    /// `scan_special_encrypt` (pdf:encrypt; the values are only recorded,
    /// encryption itself is not ported): status.
    fn scan_special_encrypt(
        &mut self,
        ext: &mut ScanSpecialsExt,
        s: &[u8],
        pp: &mut usize,
    ) -> Result<i32> {
        let mut error = 0;
        crate::parse::skip_white(s, pp);
        ext.owner_pw.clear();
        ext.user_pw.clear();
        while error == 0 && *pp < s.len() {
            let Some(kp) = crate::dpxutil::parse_c_ident(s, pp) else {
                break;
            };
            crate::parse::skip_white(s, pp);
            match &kp[..] {
                b"ownerpw" | b"userpw" => match self.o.parse_pdf_string(s, pp) {
                    Some(obj) => {
                        let v = self.o.string_value(obj)?;
                        let n = v.len().min(crate::session::MAX_PWD_LEN - 1);
                        let v = v[..n].to_vec();
                        if &kp[..] == b"ownerpw" {
                            ext.owner_pw = v;
                        } else {
                            ext.user_pw = v;
                        }
                        self.o.release(obj)?;
                    }
                    None => error = -1,
                },
                b"length" | b"perm" => {
                    let obj = self.o.parse_pdf_number(s, pp);
                    match obj {
                        Some(o) if self.o.is_number(Some(o)) => {
                            let v = self.o.number_value(o)? as u32;
                            if &kp[..] == b"length" {
                                ext.key_bits = v as i32;
                            } else {
                                ext.permission = v as i32;
                            }
                        }
                        _ => error = -1,
                    }
                    self.o.release_opt(obj)?;
                }
                _ => error = -1,
            }
            crate::parse::skip_white(s, pp);
        }
        Ok(error)
    }
    /// `scan_special_trailerid`: status.
    fn scan_special_trailerid(
        &mut self,
        ext: &mut ScanSpecialsExt,
        s: &[u8],
        pp: &mut usize,
    ) -> Result<i32> {
        let mut error = 0;
        crate::parse::skip_white(s, pp);
        match self.o.parse_pdf_array(s, pp, None)? {
            Some(id_array) => {
                if self.o.array_length(id_array)? == 2 {
                    let t1 = self.o.get_array(id_array, 0)?;
                    let t2 = self.o.get_array(id_array, 1)?;
                    if self.o.is_string(t1)
                        && self.o.string_value(t1.expect("string"))?.len() == 16
                        && self.o.is_string(t2)
                        && self.o.string_value(t2.expect("string"))?.len() == 16
                    {
                        ext.id1
                            .copy_from_slice(self.o.string_value(t1.expect("string"))?);
                        ext.id2
                            .copy_from_slice(self.o.string_value(t2.expect("string"))?);
                    } else {
                        error = -1;
                    }
                } else {
                    error = -1;
                }
                self.o.release(id_array)?;
            }
            None => error = -1,
        }
        crate::parse::skip_white(s, pp);
        Ok(error)
    }
    /// `scan_special`: status; `buf` is the special's bytes.
    fn scan_special(&mut self, sp: &mut ScanSpecials, buf: &[u8]) -> Result<i32> {
        use crate::dpxutil::{dpx_util_read_length, parse_c_ident, parse_float_decimal};
        let s = buf;
        let mut p = 0usize;
        let (mut ns_pdf, mut ns_dvipdfmx) = (false, false);
        let mut error = 0;
        let sw = |p: &mut usize| crate::parse::skip_white(s, p);
        sw(&mut p);
        let mut q = parse_c_ident(s, &mut p);
        match q.as_deref() {
            Some(b"pdf") | Some(b"x") | Some(b"dvipdfmx") => {
                let which = q.clone();
                sw(&mut p);
                if p < s.len() && s[p] == b':' {
                    p += 1;
                    sw(&mut p);
                    q = parse_c_ident(s, &mut p);
                    match which.as_deref() {
                        Some(b"pdf") => ns_pdf = true,
                        Some(b"dvipdfmx") => ns_dvipdfmx = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        sw(&mut p);
        let at = |p: usize| s.get(p).copied().unwrap_or(0);
        let Some(q) = q else { return Ok(error) };
        let mag = self.dvi_tell_mag();
        match &q[..] {
            b"landscape" => sp.landscape = 1,
            b"pagesize" if ns_pdf => {
                while error == 0 && p < s.len() {
                    let Some(kp) = parse_c_ident(s, &mut p) else {
                        break;
                    };
                    sw(&mut p);
                    match &kp[..] {
                        b"width" | b"height" | b"xoffset" | b"yoffset" => {
                            let (e, tmp) = dpx_util_read_length(mag, s, &mut p);
                            error = e;
                            if error == 0 {
                                let v = tmp * mag;
                                match &kp[..] {
                                    b"width" => sp.page_width = v,
                                    b"height" => sp.page_height = v,
                                    b"xoffset" => sp.x_offset = v,
                                    _ => sp.y_offset = v,
                                }
                            }
                        }
                        b"default" => {
                            sp.page_width = self.session.paper_width;
                            sp.page_height = self.session.paper_height;
                            sp.landscape = self.session.landscape_mode;
                            sp.x_offset = 72.0;
                            sp.y_offset = 72.0;
                        }
                        _ => {}
                    }
                    sw(&mut p);
                }
            }
            b"papersize" => {
                let mut qchr = 0u8;
                if at(p) == b'=' {
                    p += 1;
                }
                sw(&mut p);
                if p < s.len() && (s[p] == b'\'' || s[p] == b'"') {
                    qchr = s[p];
                    p += 1;
                    sw(&mut p);
                }
                let (e, tmp) = dpx_util_read_length(1.0, s, &mut p);
                error = e;
                if error == 0 {
                    sw(&mut p);
                    if p < s.len() && s[p] == b',' {
                        p += 1;
                        sw(&mut p);
                    }
                    let (e, tmp1) = dpx_util_read_length(1.0, s, &mut p);
                    error = e;
                    if error == 0 {
                        sp.page_width = tmp;
                        sp.page_height = tmp1;
                        sw(&mut p);
                    }
                }
                if error == 0 && qchr != 0 && (p >= s.len() || s[p] != qchr) {
                    error = -1;
                }
                if error == 0 {
                    self.session.paper_width = sp.page_width;
                    self.session.paper_height = sp.page_height;
                }
            }
            b"minorversion" | b"majorversion" if ns_pdf && sp.ext.is_some() => {
                if at(p) == b'=' {
                    p += 1;
                }
                sw(&mut p);
                if let Some(kv) = parse_float_decimal(s, &mut p) {
                    let v = crate::fmt::strtol(&kv, 10).0 as i32;
                    let ext = sp.ext.as_mut().expect("ext");
                    if &q[..] == b"minorversion" {
                        ext.minorversion = v;
                    } else {
                        ext.majorversion = v;
                    }
                }
            }
            b"encrypt" if ns_pdf && sp.ext.is_some() => {
                let mut ext = sp.ext.take().expect("ext");
                ext.do_enc = 1;
                error = self.scan_special_encrypt(&mut ext, s, &mut p)?;
                sp.ext = Some(ext);
            }
            b"config" if ns_dvipdfmx => {
                self.read_config_special(s, &mut p)?;
            }
            b"trailerid" if ns_pdf && sp.ext.is_some() => {
                let mut ext = sp.ext.take().expect("ext");
                error = self.scan_special_trailerid(&mut ext, s, &mut p)?;
                ext.has_id = i32::from(error == 0);
                sp.ext = Some(ext);
            }
            _ => {}
        }
        Ok(error)
    }
    /// `dvi_scan_specials`: page sizes, offsets, landscape (and on the
    /// first page the version, encryption, trailer ID) from the page's
    /// specials, updating `sp`.
    pub fn dvi_scan_specials(&mut self, page_no: i32, sp: &mut ScanSpecials) -> Result<()> {
        if page_no == self.dvi.buffered_page || self.dvi.num_pages == 0 {
            return Ok(());
        }
        self.dvi.buffered_page = page_no;
        self.dvi.dvi_page_buf_index = 0;
        if self.dvi.linear == 0 {
            if page_no as u32 >= self.dvi.num_pages {
                crate::fatal!("Invalid page number: {}", page_no);
            }
            let offset = self.dvi.page_loc[page_no as usize];
            self.dvi_file().seek_absolute(offset as usize);
        }
        loop {
            let opcode = self.get_and_buffer_unsigned_byte()? as u8;
            if opcode == EOP {
                break;
            }
            if opcode <= SET_CHAR_127 || (FNT_NUM_0..=FNT_NUM_63).contains(&opcode) {
                continue;
            }
            match opcode {
                XXX1..=XXX4 => {
                    let mut size = self.get_and_buffer_unsigned_byte()? as u32;
                    for _ in XXX1..opcode {
                        size = size
                            .wrapping_mul(0x100)
                            .wrapping_add(self.get_and_buffer_unsigned_byte()? as u32);
                    }
                    let f = self.dvi.dvi_file.as_mut().expect("DVI file");
                    let data = f.read(size as usize).to_vec();
                    if data.len() != size as usize {
                        crate::fatal!("Reading DVI file failed!");
                    }
                    self.scan_special(sp, &data)?;
                    let i = self.dvi.dvi_page_buf_index as usize;
                    if i + data.len() > self.dvi.dvi_page_buffer.len() {
                        self.dvi
                            .dvi_page_buffer
                            .resize(i + data.len() + DVI_PAGE_BUF_CHUNK as usize, 0);
                    }
                    self.dvi.dvi_page_buffer[i..i + data.len()].copy_from_slice(&data);
                    self.dvi.dvi_page_buf_index += size;
                }
                BOP => self.get_and_buffer_bytes(44)?,
                NOP | PUSH | POP | W0 | X0 | Y0 | Z0 => {}
                SET1 | PUT1 | RIGHT1 | DOWN1 | W1 | X1 | Y1 | Z1 | FNT1 => {
                    self.get_and_buffer_bytes(1)?
                }
                SET2 | PUT2 | RIGHT2 | DOWN2 | W2 | X2 | Y2 | Z2 | FNT2 => {
                    self.get_and_buffer_bytes(2)?
                }
                SET3 | PUT3 | RIGHT3 | DOWN3 | W3 | X3 | Y3 | Z3 | FNT3 => {
                    self.get_and_buffer_bytes(3)?
                }
                SET4 | PUT4 | RIGHT4 | DOWN4 | W4 | X4 | Y4 | Z4 | FNT4 => {
                    self.get_and_buffer_bytes(4)?
                }
                SET_RULE | PUT_RULE => self.get_and_buffer_bytes(8)?,
                FNT_DEF1..=FNT_DEF4 => {
                    let id = self.dvi_file().get_unsigned_num(opcode - FNT_DEF1)?;
                    self.do_fntdef(id)?;
                }
                XDV_GLYPHS => {
                    self.need_xetex(i32::from(opcode))?;
                    self.get_and_buffer_bytes(4)?;
                    let len = self.get_and_buffer_unsigned_pair()?;
                    self.get_and_buffer_bytes(len * 10)?;
                }
                XDV_TEXT_AND_GLYPHS => {
                    self.need_xetex(i32::from(opcode))?;
                    let len = self.get_and_buffer_unsigned_pair()?;
                    self.get_and_buffer_bytes(len * 2)?;
                    self.get_and_buffer_bytes(4)?;
                    let len = self.get_and_buffer_unsigned_pair()?;
                    self.get_and_buffer_bytes(len * 10)?;
                }
                XDV_NATIVE_FONT_DEF => {
                    self.need_xetex(i32::from(opcode))?;
                    let id = self.dvi_file().get_signed_quad()?;
                    self.do_native_font_def(id)?;
                }
                BEGIN_REFLECT | END_REFLECT => self.need_xetex(i32::from(opcode))?,
                PTEXDIR => {
                    self.need_ptex(i32::from(opcode))?;
                    self.get_and_buffer_bytes(1)?;
                }
                POST if self.dvi.linear != 0 && self.dvi.dvi_page_buf_index == 1 => return Ok(()),
                _ => crate::fatal!("Unexpected opcode {}", opcode),
            }
        }
        Ok(())
    }
}
