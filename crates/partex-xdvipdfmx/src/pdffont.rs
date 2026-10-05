//! pdffont.c, pdffont.h: the font cache, every font kind's common record.
//!
//! The cache is `self.font.fonts` (C's `font_cache`; count = `len()`),
//! indexed by `font_id`; the font loaders (type1.rs, truetype.rs, type0.rs,
//! cid.rs, …) take a `font_id` where C takes a `pdf_font *`.
//!
//! `usedchars` is shared in C (a Type 0 font points to its CIDFont's
//! array, flag `PDF_FONT_FLAG_USEDCHAR_SHARED`; pdfdev keeps the pointer):
//! here it is a [`UsedChars`], an `Rc<RefCell<Vec<u8>>>`, cloned to share.
//! 256 bytes (one per code) for simple fonts, 8192 (a bitmap, see
//! [`add_to_used_chars2`]) for CIDFonts.

use core::cell::RefCell;

use crate::fontmap::{
    FONTMAP_STYLE_BOLD, FONTMAP_STYLE_BOLDITALIC, FONTMAP_STYLE_ITALIC, FONTMAP_STYLE_NONE,
    FontmapRec,
};
use crate::prelude::*;

/// `PDF_FONT_FONTTYPE_TYPE1`.
pub const PDF_FONT_FONTTYPE_TYPE1: i32 = 0;
/// `PDF_FONT_FONTTYPE_TYPE1C`.
pub const PDF_FONT_FONTTYPE_TYPE1C: i32 = 1;
/// `PDF_FONT_FONTTYPE_TYPE3`.
pub const PDF_FONT_FONTTYPE_TYPE3: i32 = 2;
/// `PDF_FONT_FONTTYPE_TRUETYPE`.
pub const PDF_FONT_FONTTYPE_TRUETYPE: i32 = 3;
/// `PDF_FONT_FONTTYPE_TYPE0`.
pub const PDF_FONT_FONTTYPE_TYPE0: i32 = 4;
/// `PDF_FONT_FONTTYPE_CIDTYPE0`.
pub const PDF_FONT_FONTTYPE_CIDTYPE0: i32 = 5;
/// `PDF_FONT_FONTTYPE_CIDTYPE2`.
pub const PDF_FONT_FONTTYPE_CIDTYPE2: i32 = 6;

/// `PDF_FONT_FLAG_NOEMBED`.
pub const PDF_FONT_FLAG_NOEMBED: i32 = 1 << 0;
/// `PDF_FONT_FLAG_COMPOSITE`.
pub const PDF_FONT_FLAG_COMPOSITE: i32 = 1 << 1;
/// `PDF_FONT_FLAG_BASEFONT`.
pub const PDF_FONT_FLAG_BASEFONT: i32 = 1 << 2;
/// `PDF_FONT_FLAG_USEDCHAR_SHARED`.
pub const PDF_FONT_FLAG_USEDCHAR_SHARED: i32 = 1 << 3;
/// `PDF_FONT_FLAG_IS_ALIAS`.
pub const PDF_FONT_FLAG_IS_ALIAS: i32 = 1 << 4;
/// `PDF_FONT_FLAG_IS_REENCODE`.
pub const PDF_FONT_FLAG_IS_REENCODE: i32 = 1 << 5;
/// `PDF_FONT_FLAG_ACCFONT`.
pub const PDF_FONT_FLAG_ACCFONT: i32 = 1 << 6;
/// `PDF_FONT_FLAG_UCSFONT`.
pub const PDF_FONT_FLAG_UCSFONT: i32 = 1 << 7;

/// `CIDFONT_FLAG_TYPE1`.
pub const CIDFONT_FLAG_TYPE1: i32 = 1 << 8;
/// `CIDFONT_FLAG_TYPE1C`.
pub const CIDFONT_FLAG_TYPE1C: i32 = 1 << 9;
/// `CIDFONT_FLAG_TRUETYPE`.
pub const CIDFONT_FLAG_TRUETYPE: i32 = 1 << 10;

/// `PDF_FONT_PARAM_DESIGN_SIZE`.
pub const PDF_FONT_PARAM_DESIGN_SIZE: i32 = 1;
/// `PDF_FONT_PARAM_POINT_SIZE`.
pub const PDF_FONT_PARAM_POINT_SIZE: i32 = 2;

/// `FONT_STYLE_NONE`.
pub const FONT_STYLE_NONE: i32 = FONTMAP_STYLE_NONE;
/// `FONT_STYLE_BOLD`.
pub const FONT_STYLE_BOLD: i32 = FONTMAP_STYLE_BOLD;
/// `FONT_STYLE_ITALIC`.
pub const FONT_STYLE_ITALIC: i32 = FONTMAP_STYLE_ITALIC;
/// `FONT_STYLE_BOLDITALIC`.
pub const FONT_STYLE_BOLDITALIC: i32 = FONTMAP_STYLE_BOLDITALIC;

/// `CACHE_ALLOC_SIZE`.
pub const CACHE_ALLOC_SIZE: u32 = 16;

/// A font's used-character array, shared as C shares the pointer.
pub type UsedChars = Rc<RefCell<Vec<u8>>>;

/// `CIDSysInfo`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CidSysInfo {
    pub registry: Option<Vec<u8>>,
    pub ordering: Option<Vec<u8>>,
    pub supplement: i32,
}

/// `cid_opt`.
#[derive(Clone, Debug, Default)]
pub struct CidOpt {
    pub csi: CidSysInfo,
    pub style: i32,
    pub embed: i32,
    pub stemv: i32,
}

/// `pdf_font`'s `type0`.
#[derive(Clone, Debug)]
pub struct PdfFontType0 {
    /// Only a single descendant is allowed (a font_id).
    pub descendant: i32,
    pub wmode: i32,
}

impl Default for PdfFontType0 {
    fn default() -> Self {
        PdfFontType0 {
            descendant: -1,
            wmode: 0,
        }
    }
}

/// `pdf_font`'s `cid`.
#[derive(Clone, Debug, Default)]
pub struct PdfFontCid {
    /// Character collection.
    pub csi: CidSysInfo,
    /// Options from the map record.
    pub options: CidOpt,
    pub need_vmetrics: i32,
    pub usedchars_v: Option<UsedChars>,
}

/// `pdf_font`. `Default` is `pdf_init_font_struct`.
#[derive(Clone, Debug)]
pub struct PdfFont {
    /// Map name.
    pub ident: Option<Vec<u8>>,
    /// Its id; for an alias or a re-encoded font, the font it stands for.
    pub font_id: i32,
    pub subtype: i32,
    pub filename: Option<Vec<u8>>,
    /// Encoding or CMap id.
    pub encoding_id: i32,
    pub index: u32,
    pub fontname: Option<Vec<u8>>,
    /// `uniqueID`: six letters and a NUL, empty (all 0) until made.
    pub unique_id: [u8; 7],
    pub reference: Option<Obj>,
    pub resource: Option<Obj>,
    pub descriptor: Option<Obj>,
    pub usedchars: Option<UsedChars>,
    pub flags: i32,
    /// PK fonts.
    pub point_size: f64,
    pub design_size: f64,
    pub type0: PdfFontType0,
    pub cid: PdfFontCid,
}

impl Default for PdfFont {
    fn default() -> Self {
        PdfFont {
            ident: None,
            font_id: -1,
            subtype: -1,
            filename: None,
            encoding_id: -1,
            index: 0,
            fontname: None,
            unique_id: [0; 7],
            reference: None,
            resource: None,
            descriptor: None,
            usedchars: None,
            flags: 0,
            point_size: 0.0,
            design_size: 0.0,
            type0: PdfFontType0::default(),
            cid: PdfFontCid::default(),
        }
    }
}

/// pdffont.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `font_cache.fonts` (count = `len()`).
    pub fonts: Vec<PdfFont>,
    /// `font_cache.capacity`.
    pub capacity: i32,
    /// `unique_tag_count`.
    pub unique_tag_count: i32,
}

/// `add_to_used_chars2`: set bit `c` (MSB first).
pub fn add_to_used_chars2(b: &mut [u8], c: u32) {
    b[(c / 8) as usize] |= 1 << (7 - (c % 8));
}

/// `is_used_char2`.
#[must_use]
pub fn is_used_char2(b: &[u8], c: u32) -> bool {
    b[(c / 8) as usize] & (1 << (7 - (c % 8))) != 0
}

/// `init_CIDSysInfo` (static).
#[allow(non_snake_case)]
pub fn init_CIDSysInfo(csi: &mut CidSysInfo) {
    todo!()
}

/// `pdf_init_font_struct` (static).
pub fn pdf_init_font_struct(font: &mut PdfFont) {
    todo!()
}

impl Dpx {
    /// `pdf_font_set_dpi`: only PK fonts use it (pkfont.c is not ported).
    pub fn pdf_font_set_dpi(&mut self, font_dpi: i32) {
        todo!()
    }

    /// `pdf_font_make_uniqueTag`: six letters from an MD5 of the DVI and
    /// PDF file names and a counter.
    #[allow(non_snake_case)]
    pub fn pdf_font_make_uniqueTag(&mut self) -> [u8; 6] {
        todo!()
    }

    /// `pdf_flush_font` (static).
    fn pdf_flush_font(&mut self, font_id: i32) {
        todo!()
    }

    /// `pdf_clean_font_struct` (static).
    fn pdf_clean_font_struct(&mut self, font_id: i32) {
        todo!()
    }

    /// `pdf_init_fonts`.
    pub fn pdf_init_fonts(&mut self) {
        todo!()
    }

    /// `pdf_close_fonts`.
    pub fn pdf_close_fonts(&mut self) {
        todo!()
    }

    /// `GET_FONT` (static): `font_id`, or the font an alias stands for.
    #[allow(non_snake_case)]
    fn GET_FONT(&self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_get_font_data`.
    pub fn pdf_get_font_data(&mut self, font_id: i32) -> &mut PdfFont {
        todo!()
    }

    /// `pdf_get_font_ident`: a copy.
    pub fn pdf_get_font_ident(&mut self, font_id: i32) -> Option<Vec<u8>> {
        todo!()
    }

    /// `pdf_get_font_subtype`.
    pub fn pdf_get_font_subtype(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_get_font_reference` (not linked: the caller links).
    pub fn pdf_get_font_reference(&mut self, font_id: i32) -> Obj {
        todo!()
    }

    /// `pdf_get_font_resource` (not linked).
    pub fn pdf_get_font_resource(&mut self, font_id: i32) -> Obj {
        todo!()
    }

    /// `pdf_get_font_usedchars`: the shared array (made, 256 bytes, for a
    /// simple font that has none); none for a Type 0 font without one.
    pub fn pdf_get_font_usedchars(&mut self, font_id: i32) -> Option<UsedChars> {
        todo!()
    }

    /// `pdf_get_font_encoding`.
    pub fn pdf_get_font_encoding(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_get_font_wmode`.
    pub fn pdf_get_font_wmode(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_font_resource_name`: `F<id>` (C writes it to `buf` and returns
    /// its length).
    pub fn pdf_font_resource_name(&mut self, font_id: i32) -> Vec<u8> {
        todo!()
    }

    /// `try_load_ToUnicode_CMap` (static).
    #[allow(non_snake_case)]
    fn try_load_ToUnicode_CMap(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_font_findresource`: the font_id, or -1.
    pub fn pdf_font_findresource(&mut self, ident: &[u8], scale: f64) -> i32 {
        todo!()
    }

    /// `create_font_alias` (static).
    fn create_font_alias(&mut self, ident: &[u8], font_id: i32) -> i32 {
        todo!()
    }

    /// `create_font_reencoded` (static).
    fn create_font_reencoded(&mut self, ident: &[u8], font_id: i32, cmap_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_font_load_font`: the font_id, or -1.
    pub fn pdf_font_load_font(
        &mut self,
        ident: &[u8],
        font_scale: f64,
        mrec: Option<&FontmapRec>,
    ) -> i32 {
        todo!()
    }

    /// `pdf_font_get_resource`: made (a dict with `/Type /Font` and the
    /// subtype) on first use; not linked.
    pub fn pdf_font_get_resource(&mut self, font_id: i32) -> Obj {
        todo!()
    }

    /// `pdf_font_get_descriptor`: made on first use; not linked.
    pub fn pdf_font_get_descriptor(&mut self, font_id: i32) -> Obj {
        todo!()
    }

    /// `pdf_font_get_uniqueTag`: the six letters (made on first use).
    #[allow(non_snake_case)]
    pub fn pdf_font_get_uniqueTag(&mut self, font_id: i32) -> Vec<u8> {
        todo!()
    }

    /// `pdf_check_tfm_widths`: 0 or -1 (`widths` and `usedchars` indexed
    /// by code).
    pub fn pdf_check_tfm_widths(
        &mut self,
        ident: &[u8],
        widths: &mut [f64],
        firstchar: i32,
        lastchar: i32,
        usedchars: &[u8],
    ) -> i32 {
        todo!()
    }
}
