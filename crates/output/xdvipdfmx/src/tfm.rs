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
    tfm.header = Vec::new();
    tfm.id = 0;
    tfm.nt = 0;
    tfm.chartypes = Vec::new();
    tfm.level = 0;
    tfm.fontdir = 0;
    tfm.iswide = 0;
    tfm.nco = 0;
    tfm.ncw = 0;
    tfm.npc = 0;
    tfm.char_info = Vec::new();
    tfm.width_index = Vec::new();
    tfm.height_index = Vec::new();
    tfm.depth_index = Vec::new();
    tfm.width = Vec::new();
    tfm.height = Vec::new();
    tfm.depth = Vec::new();
}

/// `tfm_font_clear` (static).
fn tfm_font_clear(tfm: &mut TfmFont) {
    tfm.header = Vec::new();
    tfm.char_info = Vec::new();
    tfm.width = Vec::new();
    tfm.height = Vec::new();
    tfm.depth = Vec::new();
    tfm.chartypes = Vec::new();
    tfm.width_index = Vec::new();
    tfm.height_index = Vec::new();
    tfm.depth_index = Vec::new();
}

/// `lookup_char` (static): the index, or -1.
fn lookup_char(map: &CharMap, charcode: i32) -> i32 {
    if charcode >= map.coverage.first_char
        && charcode <= map.coverage.first_char + map.coverage.num_chars
    {
        return map.indices[character_index((charcode - map.coverage.first_char) as u32) as usize]
            as i32;
    }
    if charcode <= map.coverage.last_char {
        return map.indices[0] as i32;
    }
    -1
}

/// `lookup_range` (static): the index, or -1.
fn lookup_range(map: &RangeMap, charcode: i32) -> i32 {
    let mut idx = i32::from(map.num_coverages) - 1;
    while idx >= 0 && charcode >= map.coverages[idx as usize].first_char {
        if charcode
            <= map.coverages[idx as usize].first_char + map.coverages[idx as usize].num_chars
        {
            return i32::from(map.indices[character_index(idx as u32) as usize]);
        }
        idx -= 1;
    }
    if charcode as i64 <= i64::from(JFM_LASTCHAR) {
        return i32::from(map.indices[0]);
    }
    -1
}

/// `fm_init` (static).
fn fm_init(fm: &mut FontMetric) {
    fm.tex_name = Vec::new();
    fm.firstchar = 0;
    fm.lastchar = 0;
    fm.fontdir = FONT_DIR_HORIZ;
    fm.codingscheme = None;
    fm.designsize = 0;
    fm.widths = Vec::new();
    fm.heights = Vec::new();
    fm.depths = Vec::new();
    fm.charmap = FmCharmap::None;
    fm.source = SOURCE_TYPE_TFM;
}

/// `fm_clear` (static).
fn fm_clear(fm: &mut FontMetric) {
    fm.tex_name = Vec::new();
    fm.widths = Vec::new();
    fm.heights = Vec::new();
    fm.depths = Vec::new();
    fm.codingscheme = None;
    fm.charmap = FmCharmap::None;
}

/// `fread_fwords` (static): `words.len()` fix words; bytes read.
fn fread_fwords(words: &mut [Fixword], fp: &mut MemFile) -> Result<i32> {
    for w in words.iter_mut() {
        *w = fp.get_signed_quad()?;
    }
    Ok((words.len() * 4) as i32)
}

/// `fread_uquads` (static).
fn fread_uquads(quads: &mut [u32], fp: &mut MemFile) -> Result<i32> {
    for q in quads.iter_mut() {
        *q = fp.get_unsigned_quad()?;
    }
    Ok((quads.len() * 4) as i32)
}

/// `tfm_check_size` (static).
fn tfm_check_size(tfm: &mut TfmFont, tfm_file_size: i64) -> Result<()> {
    let mut expected_size: u32 = 6;
    if tfm_file_size < i64::from(tfm.wlenfile) * 4 {
        crate::fatal!("Can't proceed...");
    }
    expected_size = expected_size.wrapping_add(tfm.ec.wrapping_sub(tfm.bc).wrapping_add(1));
    expected_size = expected_size.wrapping_add(tfm.wlenheader);
    expected_size = expected_size.wrapping_add(tfm.nwidths);
    expected_size = expected_size.wrapping_add(tfm.nheights);
    expected_size = expected_size.wrapping_add(tfm.ndepths);
    expected_size = expected_size.wrapping_add(tfm.nitcor);
    expected_size = expected_size.wrapping_add(tfm.nlig);
    expected_size = expected_size.wrapping_add(tfm.nkern);
    expected_size = expected_size.wrapping_add(tfm.nextens);
    expected_size = expected_size.wrapping_add(tfm.nfonparm);
    if is_jfm(tfm.id) {
        expected_size = expected_size.wrapping_add((tfm.nt + 1) as u32);
    }
    if expected_size != tfm.wlenfile {
        crate::warn!(
            "TFM file size is expected to be {} bytes but it says it is {}bytes!",
            i64::from(expected_size) * 4,
            i64::from(tfm.wlenfile) * 4
        );
        if tfm_file_size > i64::from(expected_size) * 4 {
            crate::warn!("Proceeding nervously...");
        } else {
            crate::fatal!("Can't proceed...");
        }
    }
    Ok(())
}

/// `tfm_get_sizes` (static).
fn tfm_get_sizes(tfm_file: &mut MemFile, tfm_file_size: i64, tfm: &mut TfmFont) -> Result<()> {
    let first_hword = tfm_file.get_unsigned_pair()?;
    if is_jfm(i32::from(first_hword)) {
        tfm.id = i32::from(first_hword);
        tfm.nt = i32::from(tfm_file.get_unsigned_pair()?);
        tfm.wlenfile = u32::from(tfm_file.get_unsigned_pair()?);
    } else {
        tfm.wlenfile = u32::from(first_hword);
    }

    tfm.wlenheader = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.bc = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.ec = u32::from(tfm_file.get_unsigned_pair()?);
    if tfm.ec < tfm.bc {
        crate::fatal!("TFM file error: ec({}) < bc({}) ???", tfm.ec, tfm.bc);
    }
    tfm.nwidths = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nheights = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.ndepths = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nitcor = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nlig = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nkern = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nextens = u32::from(tfm_file.get_unsigned_pair()?);
    tfm.nfonparm = u32::from(tfm_file.get_unsigned_pair()?);

    tfm_check_size(tfm, tfm_file_size)?;
    Ok(())
}

/// `get_unsigned_triple_kanji` (static).
fn get_unsigned_triple_kanji(file: &mut MemFile) -> Result<u32> {
    let mut triple = u32::from(file.get_unsigned_byte()?);
    triple = (triple << 8) | u32::from(file.get_unsigned_byte()?);
    triple |= u32::from(file.get_unsigned_byte()?) << 16;
    Ok(triple)
}

/// `jfm_do_char_type_array` (static).
fn jfm_do_char_type_array(tfm_file: &mut MemFile, tfm: &mut TfmFont) -> Result<()> {
    tfm.chartypes = vec![0u32; (UCS_LASTCHAR + 1) as usize];
    for _ in 0..tfm.nt.max(0) as u32 {
        let charcode = get_unsigned_triple_kanji(tfm_file)?;
        let chartype = u16::from(tfm_file.get_unsigned_byte()?);
        if charcode < UCS_LASTCHAR + 1 {
            tfm.chartypes[charcode as usize] = u32::from(chartype);
        }
    }
    Ok(())
}

/// `jfm_make_charmap` (static).
fn jfm_make_charmap(fm: &mut FontMetric, tfm: &mut TfmFont) {
    if tfm.nt > 1 {
        let mut indices = vec![0u32; (UCS_LASTCHAR + 2) as usize];
        indices[(UCS_LASTCHAR + 1) as usize] = tfm.chartypes[0];
        for code in 0..=UCS_LASTCHAR as usize {
            indices[code] = tfm.chartypes[code];
        }
        fm.charmap = FmCharmap::Char(CharMap {
            coverage: Coverage {
                first_char: 0,
                num_chars: UCS_LASTCHAR as i32,
                last_char: JFM_LASTCHAR as i32,
            },
            indices,
        });
    } else {
        fm.charmap = FmCharmap::Range(RangeMap {
            num_coverages: 1,
            coverages: vec![Coverage {
                first_char: 0,
                num_chars: UCS_LASTCHAR as i32,
                last_char: JFM_LASTCHAR as i32,
            }],
            // Only default type used.
            indices: vec![0],
        });
    }
}

/// `tfm_unpack_arrays` (static). A character past 255, or an index past
/// its table, is an error: C writes or reads past its arrays there.
fn tfm_unpack_arrays(fm: &mut FontMetric, tfm: &mut TfmFont) -> Result<()> {
    fm.widths = vec![0; 256];
    fm.heights = vec![0; 256];
    fm.depths = vec![0; 256];
    for i in tfm.bc..=tfm.ec {
        let charinfo = tfm.char_info[(i - tfm.bc) as usize];
        let width_index = (charinfo >> 24) as u16;
        let height_index = ((charinfo >> 20) & 0xf) as u8;
        let depth_index = ((charinfo >> 16) & 0xf) as u8;
        let (Some(&w), Some(&h), Some(&d)) = (
            tfm.width.get(usize::from(width_index)),
            tfm.height.get(usize::from(height_index)),
            tfm.depth.get(usize::from(depth_index)),
        ) else {
            crate::fatal!(
                "Invalid TFM file: char {i}: an index past the width, height or depth table."
            );
        };
        if i > 255 {
            crate::fatal!("Invalid TFM file: char {i} past 255.");
        }
        fm.widths[i as usize] = w;
        fm.heights[i as usize] = h;
        fm.depths[i as usize] = d;
    }
    Ok(())
}

/// `sput_bigendian` (static): `n` bytes of `v` into `s`.
fn sput_bigendian(s: &mut [u8], v: i32, n: i32) -> i32 {
    let mut v = v;
    let mut i = n - 1;
    while i >= 0 {
        s[i as usize] = (v & 0xff) as u8;
        v >>= 8;
        i -= 1;
    }
    n
}

/// `tfm_unpack_header` (static).
fn tfm_unpack_header(fm: &mut FontMetric, tfm: &mut TfmFont) -> Result<()> {
    if tfm.wlenheader < 12 {
        fm.codingscheme = None;
    } else {
        let len = tfm.header[2] >> 24;
        if !(0..=39).contains(&len) {
            crate::fatal!("Invalid TFM header.");
        }
        if len > 0 {
            let mut buf = [0u8; 40];
            let mut p = 0usize;
            p += sput_bigendian(&mut buf[p..], tfm.header[2], 3) as usize;
            for i in 1..=(len / 4) as usize {
                p += sput_bigendian(&mut buf[p..], tfm.header[2 + i], 4) as usize;
            }
            buf[len as usize] = 0;
            // A C string: it ends at the first NUL.
            let end = buf.iter().position(|&c| c == 0).unwrap_or(40);
            fm.codingscheme = Some(buf[..end].to_vec());
        } else {
            fm.codingscheme = None;
        }
    }
    fm.designsize = tfm.header[1];
    Ok(())
}

/// `ofm_check_size_one` (static).
fn ofm_check_size_one(tfm: &mut TfmFont, ofm_file_size: i64) -> Result<()> {
    let mut ofm_size: u32 = 14;
    ofm_size =
        ofm_size.wrapping_add(2u32.wrapping_mul(tfm.ec.wrapping_sub(tfm.bc).wrapping_add(1)));
    ofm_size = ofm_size.wrapping_add(tfm.wlenheader);
    ofm_size = ofm_size.wrapping_add(tfm.nwidths);
    ofm_size = ofm_size.wrapping_add(tfm.nheights);
    ofm_size = ofm_size.wrapping_add(tfm.ndepths);
    ofm_size = ofm_size.wrapping_add(tfm.nitcor);
    ofm_size = ofm_size.wrapping_add(2u32.wrapping_mul(tfm.nlig));
    ofm_size = ofm_size.wrapping_add(tfm.nkern);
    ofm_size = ofm_size.wrapping_add(2u32.wrapping_mul(tfm.nextens));
    ofm_size = ofm_size.wrapping_add(tfm.nfonparm);
    if i64::from(tfm.wlenfile) != ofm_file_size / 4 || tfm.wlenfile != ofm_size {
        crate::fatal!("OFM file problem.  Table sizes don't agree.");
    }
    Ok(())
}

/// `ofm_get_sizes` (static). `ptex_with_vert` is dvi.c's
/// `dvi_ptex_with_vert` (it only chooses a warning).
fn ofm_get_sizes(
    ofm_file: &mut MemFile,
    ofm_file_size: i64,
    tfm: &mut TfmFont,
    ptex_with_vert: i32,
) -> Result<()> {
    tfm.level = ofm_file.get_signed_quad()?;

    tfm.wlenfile = ofm_file.get_positive_quad("OFM", "wlenfile")?;
    tfm.wlenheader = ofm_file.get_positive_quad("OFM", "wlenheader")?;
    tfm.bc = ofm_file.get_positive_quad("OFM", "bc")?;
    tfm.ec = ofm_file.get_positive_quad("OFM", "ec")?;
    if tfm.ec < tfm.bc {
        crate::fatal!("OFM file error: ec({}) < bc({}) ???", tfm.ec, tfm.bc);
    }
    tfm.nwidths = ofm_file.get_positive_quad("OFM", "nwidths")?;
    tfm.nheights = ofm_file.get_positive_quad("OFM", "nheights")?;
    tfm.ndepths = ofm_file.get_positive_quad("OFM", "ndepths")?;
    tfm.nitcor = ofm_file.get_positive_quad("OFM", "nitcor")?;
    tfm.nlig = ofm_file.get_positive_quad("OFM", "nlig")?;
    tfm.nkern = ofm_file.get_positive_quad("OFM", "nkern")?;
    tfm.nextens = ofm_file.get_positive_quad("OFM", "nextens")?;
    tfm.nfonparm = ofm_file.get_positive_quad("OFM", "nfonparm")?;
    tfm.fontdir = ofm_file.get_positive_quad("OFM", "fontdir")?;
    if tfm.fontdir != 0 {
        if ptex_with_vert != 0
            && tfm.fontdir == FONT_DIR_RT as u32
            && tfm.level == 1
            && tfm.ec >= 0x2E00
        {
            crate::warn!("I will interpret a font direction as pTeX vertical writing.");
        } else {
            crate::warn!("I may be interpreting a font direction incorrectly.");
        }
    }
    if tfm.level == 0 {
        ofm_check_size_one(tfm, ofm_file_size)?;
    } else if tfm.level == 1 {
        tfm.nco = ofm_file.get_positive_quad("OFM", "nco")?;
        tfm.ncw = ofm_file.get_positive_quad("OFM", "nco")?;
        tfm.npc = ofm_file.get_positive_quad("OFM", "npc")?;
        let pos = 4 * i64::from(tfm.nco.wrapping_sub(tfm.wlenheader));
        ofm_file.seek_absolute(pos as usize);
    } else {
        crate::fatal!("Can't handle OFM files with level > 1");
    }
    Ok(())
}

/// `ofm_do_char_info_zero` (static).
fn ofm_do_char_info_zero(tfm_file: &mut MemFile, tfm: &mut TfmFont) -> Result<()> {
    let num_chars = tfm.ec.wrapping_sub(tfm.bc).wrapping_add(1);
    if num_chars != 0 {
        tfm.width_index = vec![0; num_chars as usize];
        tfm.height_index = vec![0; num_chars as usize];
        tfm.depth_index = vec![0; num_chars as usize];
        for i in 0..num_chars as usize {
            tfm.width_index[i] = tfm_file.get_unsigned_pair()?;
            tfm.height_index[i] = tfm_file.get_unsigned_byte()?;
            tfm.depth_index[i] = tfm_file.get_unsigned_byte()?;
            // Ignore remaining quad
            tfm_file.skip_bytes(4)?;
        }
    }
    Ok(())
}

/// `ofm_do_char_info_one` (static). The index arrays get one spare entry:
/// C writes one past them when the last repeats reach `num_chars`.
fn ofm_do_char_info_one(tfm_file: &mut MemFile, tfm: &mut TfmFont) -> Result<()> {
    let num_char_infos = tfm.ncw / (3 + (tfm.npc / 2));
    let num_chars = tfm.ec.wrapping_sub(tfm.bc).wrapping_add(1);

    if num_chars != 0 {
        tfm.width_index = vec![0; num_chars as usize + 1];
        tfm.height_index = vec![0; num_chars as usize + 1];
        tfm.depth_index = vec![0; num_chars as usize + 1];
        let mut char_infos_read: u32 = 0;
        let mut i: u32 = 0;
        while i < num_chars && char_infos_read < num_char_infos {
            let iu = i as usize;
            tfm.width_index[iu] = tfm_file.get_unsigned_pair()?;
            tfm.height_index[iu] = tfm_file.get_unsigned_byte()?;
            tfm.depth_index[iu] = tfm_file.get_unsigned_byte()?;
            // Ignore next quad
            tfm_file.skip_bytes(4)?;
            let repeats = i32::from(tfm_file.get_unsigned_pair()?);
            // Skip params
            for _ in 0..tfm.npc {
                tfm_file.get_unsigned_pair()?;
            }
            // Remove word padding if necessary
            if tfm.npc % 2 == 0 {
                tfm_file.get_unsigned_pair()?;
            }
            char_infos_read += 1;
            if i64::from(i) + i64::from(repeats) > i64::from(num_chars) {
                crate::fatal!("Repeats causes number of characters to be exceeded.");
            }
            for j in 0..repeats as usize {
                tfm.width_index[iu + j + 1] = tfm.width_index[iu];
                tfm.height_index[iu + j + 1] = tfm.height_index[iu];
                tfm.depth_index[iu + j + 1] = tfm.depth_index[iu];
            }
            if is_wide_char(i as i32) || is_wide_char(i as i32 + repeats) {
                tfm.iswide = 1;
            }
            // Skip ahead because we have already handled repeats
            i += repeats as u32;
            i += 1;
        }
    }
    Ok(())
}

/// `ofm_unpack_arrays` (static).
fn ofm_unpack_arrays(fm: &mut FontMetric, tfm: &mut TfmFont, num_chars: u32) {
    let n = (tfm.bc + num_chars) as usize;
    fm.widths = vec![0; n];
    fm.heights = vec![0; n];
    fm.depths = vec![0; n];
    for i in 0..num_chars as usize {
        let k = tfm.bc as usize + i;
        fm.widths[k] = tfm.width[tfm.width_index[i] as usize];
        fm.heights[k] = tfm.height[tfm.height_index[i] as usize];
        fm.depths[k] = tfm.depth[tfm.depth_index[i] as usize];
    }
}

/// `read_ofm` (static).
fn read_ofm(
    fm: &mut FontMetric,
    ofm_file: &mut MemFile,
    ofm_file_size: i64,
    ptex_with_vert: i32,
) -> Result<()> {
    let mut tfm = TfmFont::default();
    tfm_font_init(&mut tfm);

    ofm_get_sizes(ofm_file, ofm_file_size, &mut tfm, ptex_with_vert)?;

    if tfm.level < 0 || tfm.level > 1 {
        crate::fatal!("OFM level {} not supported.", tfm.level);
    }

    if tfm.wlenheader > 0 {
        tfm.header = vec![0; tfm.wlenheader as usize];
        fread_fwords(&mut tfm.header, ofm_file)?;
    }
    if tfm.level == 0 {
        ofm_do_char_info_zero(ofm_file, &mut tfm)?;
    } else if tfm.level == 1 {
        ofm_do_char_info_one(ofm_file, &mut tfm)?;
    }
    if tfm.nwidths > 0 {
        tfm.width = vec![0; tfm.nwidths as usize];
        fread_fwords(&mut tfm.width, ofm_file)?;
    }
    if tfm.nheights > 0 {
        tfm.height = vec![0; tfm.nheights as usize];
        fread_fwords(&mut tfm.height, ofm_file)?;
    }
    if tfm.ndepths > 0 {
        tfm.depth = vec![0; tfm.ndepths as usize];
        fread_fwords(&mut tfm.depth, ofm_file)?;
    }

    let num_chars = tfm.ec - tfm.bc + 1;
    ofm_unpack_arrays(fm, &mut tfm, num_chars);
    tfm_unpack_header(fm, &mut tfm)?;
    fm.firstchar = tfm.bc as i32;
    fm.lastchar = tfm.ec as i32;
    fm.level = tfm.level;
    fm.source = SOURCE_TYPE_OFM;
    fm.iswide = tfm.iswide as i32;

    tfm_font_clear(&mut tfm);
    Ok(())
}

/// `read_tfm` (static).
fn read_tfm(fm: &mut FontMetric, tfm_file: &mut MemFile, tfm_file_size: i64) -> Result<()> {
    let mut tfm = TfmFont::default();
    tfm_font_init(&mut tfm);

    tfm_get_sizes(tfm_file, tfm_file_size, &mut tfm)?;
    fm.firstchar = tfm.bc as i32;
    fm.lastchar = tfm.ec as i32;
    if tfm.wlenheader > 0 {
        tfm.header = vec![0; tfm.wlenheader as usize];
        fread_fwords(&mut tfm.header, tfm_file)?;
    }
    if is_jfm(tfm.id) {
        jfm_do_char_type_array(tfm_file, &mut tfm)?;
        jfm_make_charmap(fm, &mut tfm);
        fm.firstchar = 0;
        fm.lastchar = JFM_LASTCHAR as i32;
        fm.fontdir = if tfm.id == JFMV_ID {
            FONT_DIR_VERT
        } else {
            FONT_DIR_HORIZ
        };
        fm.source = SOURCE_TYPE_JFM;
    }
    let n = tfm.ec.wrapping_sub(tfm.bc).wrapping_add(1);
    if n > 0 {
        tfm.char_info = vec![0; n as usize];
        fread_uquads(&mut tfm.char_info, tfm_file)?;
    }
    if tfm.nwidths > 0 {
        tfm.width = vec![0; tfm.nwidths as usize];
        fread_fwords(&mut tfm.width, tfm_file)?;
    }
    if tfm.nheights > 0 {
        tfm.height = vec![0; tfm.nheights as usize];
        fread_fwords(&mut tfm.height, tfm_file)?;
    }
    if tfm.ndepths > 0 {
        tfm.depth = vec![0; tfm.ndepths as usize];
        fread_fwords(&mut tfm.depth, tfm_file)?;
    }
    tfm_unpack_arrays(fm, &mut tfm)?;
    tfm_unpack_header(fm, &mut tfm)?;

    tfm_font_clear(&mut tfm);
    Ok(())
}

/// `strrchr(name, '.')` compared with `strcasecmp`.
fn has_suffix_ci(name: &[u8], suffix: &[u8]) -> Option<bool> {
    let dot = name.iter().rposition(|&c| c == b'.')?;
    Some(name[dot..].eq_ignore_ascii_case(suffix))
}

/// kpathsea's program name: dvipdfmx.c pretends to be `dvipdfmx`.
const KPSE_PROGNAME: &[u8] = b"dvipdfmx";

impl Dpx {
    /// `fms_need` (static).
    fn fms_need(&mut self, n: u32) {
        if n > self.tfm.max_fms {
            self.tfm.max_fms = (self.tfm.max_fms + MAX_FONTS).max(n);
            let extra = (self.tfm.max_fms as usize).saturating_sub(self.tfm.fms.len());
            self.tfm.fms.reserve(extra);
        }
    }

    /// `tfm_open`: the tfm id, or -1 (an error when `must_exist`).
    /// kpathsea's `must_exist` (mktextfm) is the host's business: the
    /// retry looks the TFM up again.
    pub fn tfm_open(&mut self, tfm_name: &[u8], must_exist: bool) -> Result<i32> {
        let mut format = TFM_FORMAT;

        for (i, fm) in self.tfm.fms.iter().enumerate() {
            if fm.tex_name == tfm_name {
                return Ok(i as i32);
            }
        }

        let mut file_name: Option<Vec<u8>>;
        {
            let is_tfm_or_ofm = matches!(has_suffix_ci(tfm_name, b".tfm"), Some(true))
                || matches!(has_suffix_ci(tfm_name, b".ofm"), Some(true));
            let ofm_name = if is_tfm_or_ofm {
                None
            } else {
                let mut n = tfm_name.to_vec();
                n.extend_from_slice(b".ofm");
                Some(n)
            };
            file_name = None;
            if let Some(ofm) = &ofm_name {
                file_name = self.files.find(ofm, crate::io::Format::Ofm, KPSE_PROGNAME);
            }
            if file_name.is_some() {
                format = OFM_FORMAT;
            } else {
                file_name = self
                    .files
                    .find(tfm_name, crate::io::Format::Tfm, KPSE_PROGNAME);
                if file_name.is_some() {
                    format = TFM_FORMAT;
                } else {
                    file_name = self
                        .files
                        .find(tfm_name, crate::io::Format::Ofm, KPSE_PROGNAME);
                    if let Some(f) = &file_name {
                        format = if matches!(has_suffix_ci(f, b".ofm"), Some(true)) {
                            OFM_FORMAT
                        } else {
                            TFM_FORMAT
                        };
                    }
                }
            }
        }

        if file_name.is_none() {
            if must_exist {
                file_name = self
                    .files
                    .find(tfm_name, crate::io::Format::Tfm, KPSE_PROGNAME);
                if file_name.is_some() {
                    format = TFM_FORMAT;
                } else {
                    crate::fatal!(
                        "Unable to find TFM file \"{}\".",
                        String::from_utf8_lossy(tfm_name)
                    );
                }
            } else {
                return Ok(-1);
            }
        }
        let file_name = file_name.unwrap();

        let Some(data) = self.files.read(&file_name) else {
            crate::fatal!(
                "Could not open specified TFM/OFM file \"{}\".",
                String::from_utf8_lossy(tfm_name)
            );
        };
        let mut tfm_file = MemFile::new(data, &file_name);

        let tfm_file_size = tfm_file.len() as i64;
        if tfm_file_size > 0x1_ffff_ffff {
            crate::fatal!("TFM/OFM file size exceeds 33-bit");
        }
        if tfm_file_size < 24 {
            crate::fatal!("TFM/OFM file too small to be a valid file.");
        }

        let numfms = self.tfm.fms.len() as u32;
        self.fms_need(numfms + 1);
        let mut fm = FontMetric::default();
        fm_init(&mut fm);

        if format == OFM_FORMAT {
            let ptex_with_vert = self.session.dvi_ptex_with_vert;
            read_ofm(&mut fm, &mut tfm_file, tfm_file_size, ptex_with_vert)?;
        } else {
            read_tfm(&mut fm, &mut tfm_file, tfm_file_size)?;
        }

        fm.tex_name = tfm_name.to_vec();
        self.tfm.fms.push(fm);
        Ok(numfms as i32)
    }

    /// `tfm_close_all`.
    pub fn tfm_close_all(&mut self) {
        for fm in &mut self.tfm.fms {
            fm_clear(fm);
        }
        self.tfm.fms = Vec::new();
    }

    /// `CHECK_ID`.
    fn tfm_check_id(&self, n: i32) -> Result<()> {
        if n < 0 || n as usize >= self.tfm.fms.len() {
            crate::fatal!("TFM: Invalid TFM ID: {}", n);
        }
        Ok(())
    }

    /// The index `tfm_get_fw_width` and its kin look up.
    fn tfm_char_idx(&self, font_id: i32, ch: i32) -> Result<usize> {
        self.tfm_check_id(font_id)?;
        let fm = &self.tfm.fms[font_id as usize];
        let mut idx: i32 = 0;
        if ch >= fm.firstchar && ch <= fm.lastchar {
            match &fm.charmap {
                FmCharmap::Char(map) => {
                    idx = lookup_char(map, ch);
                    if idx < 0 {
                        crate::fatal!("Invalid char: {}\n", ch);
                    }
                }
                FmCharmap::Range(map) => {
                    idx = lookup_range(map, ch);
                    if idx < 0 {
                        crate::fatal!("Invalid char: {}\n", ch);
                    }
                }
                FmCharmap::None => idx = ch,
            }
        } else if ch > fm.lastchar && i64::from(ch) <= i64::from(JFM_LASTCHAR) {
            idx = 0;
        } else {
            crate::fatal!("Invalid char: {}\n", ch);
        }
        Ok(idx as usize)
    }

    /// `tfm_get_width`: a fraction of the design size.
    pub fn tfm_get_width(&mut self, font_id: i32, ch: i32) -> Result<f64> {
        Ok(f64::from(self.tfm_get_fw_width(font_id, ch)?) / FWBASE)
    }

    /// `tfm_get_fw_width`.
    pub fn tfm_get_fw_width(&mut self, font_id: i32, ch: i32) -> Result<Fixword> {
        let idx = self.tfm_char_idx(font_id, ch)?;
        Ok(self.tfm.fms[font_id as usize].widths[idx])
    }

    /// `tfm_get_fw_height`.
    pub fn tfm_get_fw_height(&mut self, font_id: i32, ch: i32) -> Result<Fixword> {
        let idx = self.tfm_char_idx(font_id, ch)?;
        Ok(self.tfm.fms[font_id as usize].heights[idx])
    }

    /// `tfm_get_fw_depth`.
    pub fn tfm_get_fw_depth(&mut self, font_id: i32, ch: i32) -> Result<Fixword> {
        let idx = self.tfm_char_idx(font_id, ch)?;
        Ok(self.tfm.fms[font_id as usize].depths[idx])
    }

    /// `tfm_string_width`: `s` is C's `(s, len)`.
    pub fn tfm_string_width(&mut self, font_id: i32, s: &[u8]) -> Result<Fixword> {
        let mut result: Fixword = 0;
        self.tfm_check_id(font_id)?;
        let len = s.len();
        if self.tfm.fms[font_id as usize].source == SOURCE_TYPE_JFM {
            for i in 0..len / 2 {
                let ch = (i32::from(s[2 * i]) << 8) | i32::from(s[2 * i + 1]);
                result = result.wrapping_add(self.tfm_get_fw_width(font_id, ch)?);
            }
        } else {
            for &c in s {
                result = result.wrapping_add(self.tfm_get_fw_width(font_id, i32::from(c))?);
            }
        }
        Ok(result)
    }

    /// `tfm_get_design_size`: in big points.
    pub fn tfm_get_design_size(&mut self, font_id: i32) -> Result<f64> {
        self.tfm_check_id(font_id)?;
        Ok(f64::from(self.tfm.fms[font_id as usize].designsize) / FWBASE * (72.0 / 72.27))
    }

    /// `tfm_is_jfm`: 1 JFM, 2 wide OFM level 1, else 0.
    pub fn tfm_is_jfm(&mut self, font_id: i32) -> Result<i32> {
        let mut is_jfm = 0;
        self.tfm_check_id(font_id)?;
        let fm = &self.tfm.fms[font_id as usize];
        if fm.source == SOURCE_TYPE_JFM {
            is_jfm = 1;
        }
        if fm.source == SOURCE_TYPE_OFM && fm.level == 1 && fm.iswide == 1 {
            is_jfm = 2;
        }
        Ok(is_jfm)
    }

    /// `tfm_exists`: an OFM or a TFM file is found.
    pub fn tfm_exists(&mut self, tfm_name: &[u8]) -> bool {
        if self
            .files
            .find(tfm_name, crate::io::Format::Ofm, KPSE_PROGNAME)
            .is_some()
        {
            return true;
        }
        self.files
            .find(tfm_name, crate::io::Format::Tfm, KPSE_PROGNAME)
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    fn tfm_bytes() -> Vec<u8> {
        let mut b = Vec::new();
        let pair = |b: &mut Vec<u8>, v: u16| b.extend_from_slice(&v.to_be_bytes());
        let quad = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_be_bytes());
        // lf lh bc ec nw nh nd ni nl nk ne np
        for v in [25u16, 12, 65, 66, 2, 2, 1, 0, 0, 0, 0, 0] {
            pair(&mut b, v);
        }
        quad(&mut b, 0x1234_5678); // checksum
        quad(&mut b, 10 << 20); // design size
        quad(&mut b, 0x0854_6558); // 8, "TeX"
        quad(&mut b, 0x2074_6578); // " tex"
        quad(&mut b, 0x7400_0000); // "t"
        for _ in 5..12 {
            quad(&mut b, 0);
        }
        quad(&mut b, 0x0110_0000); // A: width 1, height 1, depth 0
        quad(&mut b, 0x0100_0000); // B: width 1
        quad(&mut b, 0);
        quad(&mut b, 0x8_0000);
        quad(&mut b, 0);
        quad(&mut b, 0x7_0000);
        quad(&mut b, 0);
        b
    }

    #[test]
    fn read_a_tfm() {
        let data = tfm_bytes();
        assert_eq!(data.len(), 100);
        let n = data.len() as i64;
        let mut f = MemFile::new(Arc::from(data), b"x.tfm");
        let mut fm = FontMetric::default();
        fm_init(&mut fm);
        read_tfm(&mut fm, &mut f, n).unwrap();
        assert_eq!(fm.firstchar, 65);
        assert_eq!(fm.lastchar, 66);
        assert_eq!(fm.designsize, 10 << 20);
        assert_eq!(fm.codingscheme.as_deref(), Some(&b"TeX text"[..]));
        assert_eq!(fm.widths[65], 0x8_0000);
        assert_eq!(fm.heights[65], 0x7_0000);
        assert_eq!(fm.widths[66], 0x8_0000);
        assert_eq!(fm.heights[66], 0);
        assert_eq!(fm.widths[64], 0);
        assert_eq!(f.tell(), 100);
    }

    #[test]
    fn jfm_maps() {
        let r = RangeMap {
            num_coverages: 1,
            coverages: vec![Coverage {
                first_char: 0,
                num_chars: UCS_LASTCHAR as i32,
                last_char: JFM_LASTCHAR as i32,
            }],
            indices: vec![0],
        };
        assert_eq!(lookup_range(&r, 0x3042), 0);
        assert_eq!(lookup_range(&r, 0x0100_0000), -1);
        let mut indices = vec![0u32; (UCS_LASTCHAR + 2) as usize];
        indices[0x3042] = 3;
        indices[(UCS_LASTCHAR + 1) as usize] = 7;
        let c = CharMap {
            coverage: Coverage {
                first_char: 0,
                num_chars: UCS_LASTCHAR as i32,
                last_char: JFM_LASTCHAR as i32,
            },
            indices,
        };
        assert_eq!(lookup_char(&c, 0x3042), 3);
        assert_eq!(lookup_char(&c, 0x20_0000), 0);
        assert_eq!(lookup_char(&c, 0x0100_0000), -1);
        let mut s = [0u8; 4];
        assert_eq!(sput_bigendian(&mut s, 0x0102_0304, 3), 3);
        assert_eq!(&s[..3], &[2, 3, 4]);
    }
}
