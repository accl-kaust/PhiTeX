//! tfm.c, tfm.h: TFM, OFM and JFM font metrics.
//!
//! The loaded metrics are `self.tfm.fms` (C's `fms`, count = `len()`),
//! indexed by the tfm id `tfm_open` returns.

use crate::prelude::*;

/// `fixword` (numbers.h): a TFM fix word.
pub type Fixword = i32;

/// `UCS_LASTCHAR`.
pub const UCS_LASTCHAR: u32 = 0x10FFFF;
/// `JFM_LASTCHAR`.
pub const JFM_LASTCHAR: u32 = 0xFFFFFF;

/// `TFM_FORMAT`.
pub const TFM_FORMAT: i32 = 1;
/// `OFM_FORMAT`.
pub const OFM_FORMAT: i32 = 2;
/// `FWBASE`.
pub const FWBASE: f64 = (1 << 20) as f64;
/// `JFM_ID`.
pub const JFM_ID: i32 = 11;
/// `JFMV_ID` (vertical JFM).
pub const JFMV_ID: i32 = 9;

/// `SOURCE_TYPE_TFM`.
pub const SOURCE_TYPE_TFM: i32 = 0;
/// `SOURCE_TYPE_JFM`.
pub const SOURCE_TYPE_JFM: i32 = 1;
/// `SOURCE_TYPE_OFM`.
pub const SOURCE_TYPE_OFM: i32 = 2;

/// `MAPTYPE_NONE`.
pub const MAPTYPE_NONE: i32 = 0;
/// `MAPTYPE_CHAR`.
pub const MAPTYPE_CHAR: i32 = 1;
/// `MAPTYPE_RANGE`.
pub const MAPTYPE_RANGE: i32 = 2;

/// `FONT_DIR_HORIZ`.
pub const FONT_DIR_HORIZ: i32 = 0;
/// `FONT_DIR_VERT`.
pub const FONT_DIR_VERT: i32 = 1;
/// `FONT_DIR_RT`.
pub const FONT_DIR_RT: i32 = 5;

/// `MAX_FONTS` (allocation step of `fms`).
pub const MAX_FONTS: u32 = 16;

/// `IS_JFM`.
#[must_use]
pub fn is_jfm(i: i32) -> bool {
    i == JFM_ID || i == JFMV_ID
}

/// `CHARACTER_INDEX`.
#[must_use]
pub fn character_index(i: u32) -> u32 {
    if i > UCS_LASTCHAR {
        UCS_LASTCHAR + 1
    } else {
        i
    }
}

/// `IS_WIDE_CHAR`.
#[must_use]
pub fn is_wide_char(i: i32) -> bool {
    i >= 0x2E80 && !(0xFB00..=0xFB06).contains(&i)
}

/// `struct tfm_font`: a TFM/OFM file being read.
#[derive(Clone, Debug, Default)]
pub struct TfmFont {
    pub id: i32,
    pub nt: i32,
    pub level: i32,
    pub wlenfile: u32,
    pub wlenheader: u32,
    pub bc: u32,
    pub ec: u32,
    pub nwidths: u32,
    pub nheights: u32,
    pub ndepths: u32,
    pub nitcor: u32,
    pub nlig: u32,
    pub nkern: u32,
    pub nextens: u32,
    pub nfonparm: u32,
    pub fontdir: u32,
    pub iswide: u32,
    pub nco: u32,
    pub ncw: u32,
    pub npc: u32,
    pub header: Vec<Fixword>,
    pub chartypes: Vec<u32>,
    pub char_info: Vec<u32>,
    pub width_index: Vec<u16>,
    pub height_index: Vec<u8>,
    pub depth_index: Vec<u8>,
    pub width: Vec<Fixword>,
    pub height: Vec<Fixword>,
    pub depth: Vec<Fixword>,
}

/// `struct coverage`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Coverage {
    pub first_char: i32,
    pub num_chars: i32,
    pub last_char: i32,
}

/// `struct range_map`: all characters of a range have the same metrics.
#[derive(Clone, Debug, Default)]
pub struct RangeMap {
    pub num_coverages: u16,
    pub coverages: Vec<Coverage>,
    pub indices: Vec<u16>,
}

/// `struct char_map` (one coverage).
#[derive(Clone, Debug, Default)]
pub struct CharMap {
    pub coverage: Coverage,
    pub indices: Vec<u32>,
}

/// `font_metric`'s `charmap` (`type` + `data`).
#[derive(Clone, Debug, Default)]
pub enum FmCharmap {
    /// `MAPTYPE_NONE`.
    #[default]
    None,
    /// `MAPTYPE_CHAR`.
    Char(CharMap),
    /// `MAPTYPE_RANGE`.
    Range(RangeMap),
}

/// `struct font_metric`. `Default` is `fm_init`.
#[derive(Clone, Debug, Default)]
pub struct FontMetric {
    pub tex_name: Vec<u8>,
    pub designsize: Fixword,
    pub codingscheme: Option<Vec<u8>>,
    pub level: i32,
    pub fontdir: i32,
    pub iswide: i32,
    pub firstchar: i32,
    pub lastchar: i32,
    pub widths: Vec<Fixword>,
    pub heights: Vec<Fixword>,
    pub depths: Vec<Fixword>,
    pub charmap: FmCharmap,
    pub source: i32,
}

/// tfm.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `fms` (`numfms` = `len()`).
    pub fms: Vec<FontMetric>,
    /// `max_fms`.
    pub max_fms: u32,
}

/// `tfm_font_init` (static).
fn tfm_font_init(tfm: &mut TfmFont) {
    todo!()
}

/// `tfm_font_clear` (static).
fn tfm_font_clear(tfm: &mut TfmFont) {
    todo!()
}

/// `lookup_char` (static): the index, or -1.
fn lookup_char(map: &CharMap, charcode: i32) -> i32 {
    todo!()
}

/// `lookup_range` (static): the index, or -1.
fn lookup_range(map: &RangeMap, charcode: i32) -> i32 {
    todo!()
}

/// `fm_init` (static).
fn fm_init(fm: &mut FontMetric) {
    todo!()
}

/// `fm_clear` (static).
fn fm_clear(fm: &mut FontMetric) {
    todo!()
}

/// `fread_fwords` (static): `words.len()` fix words; bytes read.
fn fread_fwords(words: &mut [Fixword], fp: &mut MemFile) -> i32 {
    todo!()
}

/// `fread_uquads` (static).
fn fread_uquads(quads: &mut [u32], fp: &mut MemFile) -> i32 {
    todo!()
}

/// `tfm_check_size` (static).
fn tfm_check_size(tfm: &mut TfmFont, tfm_file_size: i64) {
    todo!()
}

/// `tfm_get_sizes` (static).
fn tfm_get_sizes(tfm_file: &mut MemFile, tfm_file_size: i64, tfm: &mut TfmFont) {
    todo!()
}

/// `get_unsigned_triple_kanji` (static).
fn get_unsigned_triple_kanji(file: &mut MemFile) -> u32 {
    todo!()
}

/// `jfm_do_char_type_array` (static).
fn jfm_do_char_type_array(tfm_file: &mut MemFile, tfm: &mut TfmFont) {
    todo!()
}

/// `jfm_make_charmap` (static).
fn jfm_make_charmap(fm: &mut FontMetric, tfm: &mut TfmFont) {
    todo!()
}

/// `tfm_unpack_arrays` (static).
fn tfm_unpack_arrays(fm: &mut FontMetric, tfm: &mut TfmFont) {
    todo!()
}

/// `sput_bigendian` (static): `n` bytes of `v` into `s`.
fn sput_bigendian(s: &mut [u8], v: i32, n: i32) -> i32 {
    todo!()
}

/// `tfm_unpack_header` (static).
fn tfm_unpack_header(fm: &mut FontMetric, tfm: &mut TfmFont) {
    todo!()
}

/// `ofm_check_size_one` (static).
fn ofm_check_size_one(tfm: &mut TfmFont, ofm_file_size: i64) {
    todo!()
}

/// `ofm_get_sizes` (static).
fn ofm_get_sizes(ofm_file: &mut MemFile, ofm_file_size: i64, tfm: &mut TfmFont) {
    todo!()
}

/// `ofm_do_char_info_zero` (static).
fn ofm_do_char_info_zero(tfm_file: &mut MemFile, tfm: &mut TfmFont) {
    todo!()
}

/// `ofm_do_char_info_one` (static).
fn ofm_do_char_info_one(tfm_file: &mut MemFile, tfm: &mut TfmFont) {
    todo!()
}

/// `ofm_unpack_arrays` (static).
fn ofm_unpack_arrays(fm: &mut FontMetric, tfm: &mut TfmFont, num_chars: u32) {
    todo!()
}

/// `read_ofm` (static).
fn read_ofm(fm: &mut FontMetric, ofm_file: &mut MemFile, ofm_file_size: i64) {
    todo!()
}

/// `read_tfm` (static).
fn read_tfm(fm: &mut FontMetric, tfm_file: &mut MemFile, tfm_file_size: i64) {
    todo!()
}

impl Dpx {
    /// `fms_need` (static).
    fn fms_need(&mut self, n: u32) {
        todo!()
    }

    /// `tfm_open`: the tfm id, or -1 (an error when `must_exist`).
    pub fn tfm_open(&mut self, tfm_name: &[u8], must_exist: bool) -> i32 {
        todo!()
    }

    /// `tfm_close_all`.
    pub fn tfm_close_all(&mut self) {
        todo!()
    }

    /// `tfm_get_width`: a fraction of the design size.
    pub fn tfm_get_width(&mut self, font_id: i32, ch: i32) -> f64 {
        todo!()
    }

    /// `tfm_get_fw_width`.
    pub fn tfm_get_fw_width(&mut self, font_id: i32, ch: i32) -> Fixword {
        todo!()
    }

    /// `tfm_get_fw_height`.
    pub fn tfm_get_fw_height(&mut self, font_id: i32, ch: i32) -> Fixword {
        todo!()
    }

    /// `tfm_get_fw_depth`.
    pub fn tfm_get_fw_depth(&mut self, font_id: i32, ch: i32) -> Fixword {
        todo!()
    }

    /// `tfm_string_width`.
    pub fn tfm_string_width(&mut self, font_id: i32, s: &[u8]) -> Fixword {
        todo!()
    }

    /// `tfm_get_design_size`: in big points.
    pub fn tfm_get_design_size(&mut self, font_id: i32) -> f64 {
        todo!()
    }

    /// `tfm_is_jfm`: 1 JFM, 2 wide OFM level 1, else 0.
    pub fn tfm_is_jfm(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `tfm_exists`: an OFM or a TFM file is found.
    pub fn tfm_exists(&mut self, tfm_name: &[u8]) -> bool {
        todo!()
    }
}
