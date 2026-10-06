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
    csi.registry = None;
    csi.ordering = None;
    csi.supplement = 0;
}

/// `pdf_init_font_struct` (static).
pub fn pdf_init_font_struct(font: &mut PdfFont) {
    *font = PdfFont::default();
    font.cid.options.style = FONT_STYLE_NONE;
}

impl Dpx {
    /// `pdf_font_set_dpi`: only PK fonts use it (pkfont.c is not ported).
    pub fn pdf_font_set_dpi(&mut self, _font_dpi: i32) {}

    /// `pdf_font_make_uniqueTag`: six letters from an MD5 of the DVI and
    /// PDF file names and a counter.
    ///
    /// 20260113's counter is a union of a `char[sizeof(int)]` and an
    /// `int *`: `unique_tag_count.i++` steps the pointer by
    /// `sizeof(int)`, and `sizeof(unique_tag_count)` (the pointer's 8
    /// bytes, little-endian on the x86-64 and arm64 TeX Live builds) are
    /// hashed. So the n-th tag hashes the 8 bytes of `4 * n`.
    #[allow(non_snake_case)]
    pub fn pdf_font_make_uniqueTag(&mut self) -> [u8; 6] {
        self.font.unique_tag_count += 1;
        let counter = 4u64 * self.font.unique_tag_count as u64;
        let mut data = Vec::new();
        if let Some(d) = &self.dvi_filename {
            data.extend_from_slice(d);
        }
        if let Some(p) = &self.pdf_filename {
            data.extend_from_slice(p);
        }
        data.extend_from_slice(&counter.to_le_bytes());
        let digest = partex_engine::md5::md5(&data);
        let mut tag = [0u8; 6];
        for i in 0..6 {
            tag[i] = b'A' + digest[i] % 26;
        }
        tag
    }

    /// `pdf_flush_font` (static).
    fn pdf_flush_font(&mut self, font_id: i32) -> Result<()> {
        let fi = font_id as usize;
        let font = &self.font.fonts[fi];
        if font.flags & (PDF_FONT_FLAG_IS_ALIAS | PDF_FONT_FLAG_IS_REENCODE) != 0 {
            return Ok(());
        }
        if let (Some(resource), Some(_)) = (font.resource, font.reference) {
            match font.subtype {
                PDF_FONT_FONTTYPE_TYPE3
                | PDF_FONT_FONTTYPE_TYPE0
                | PDF_FONT_FONTTYPE_CIDTYPE0
                | PDF_FONT_FONTTYPE_CIDTYPE2 => {}
                _ => {
                    let descriptor = font.descriptor;
                    let fontname = font.fontname.clone().expect("fontname");
                    let name = if font.flags & PDF_FONT_FLAG_NOEMBED != 0 {
                        fontname
                    } else {
                        let tag = self.pdf_font_get_uniqueTag(font_id);
                        // ("%6s+%s")
                        let mut n = Vec::new();
                        for _ in tag.len()..6 {
                            n.push(b' ');
                        }
                        n.extend_from_slice(&tag);
                        n.push(b'+');
                        n.extend_from_slice(&fontname);
                        n
                    };
                    self.o.put_name(resource, b"BaseFont", &name)?;
                    if let Some(d) = descriptor {
                        self.o.put_name(d, b"FontName", &name)?;
                        let r = self.o.ref_obj(d)?;
                        self.o.put(resource, b"FontDescriptor", r)?;
                    }
                }
            }
        }
        let font = &mut self.font.fonts[fi];
        let (r, d, f) = (
            font.resource.take(),
            font.descriptor.take(),
            font.reference.take(),
        );
        self.o.release_opt(r)?;
        self.o.release_opt(d)?;
        self.o.release_opt(f)?;
        Ok(())
    }

    /// `pdf_clean_font_struct` (static).
    fn pdf_clean_font_struct(&mut self, font_id: i32) {
        let font = &mut self.font.fonts[font_id as usize];
        font.ident = None;
        font.filename = None;
        font.fontname = None;
        font.usedchars = None;
        font.cid.csi.registry = None;
        font.cid.csi.ordering = None;
        font.cid.options.csi.registry = None;
        font.cid.options.csi.ordering = None;
        font.cid.usedchars_v = None;
    }

    /// `pdf_init_fonts`.
    pub fn pdf_init_fonts(&mut self) -> Result<()> {
        self.agl_init_map()?;
        self.CMap_cache_init()?;
        self.pdf_init_encodings();
        self.font.fonts.clear();
        self.font.capacity = CACHE_ALLOC_SIZE as i32;
        Ok(())
        // (C seeds rand() with the time: nothing here uses rand)
    }

    /// `pdf_close_fonts`.
    pub fn pdf_close_fonts(&mut self) -> Result<()> {
        let count = self.font.fonts.len();
        for font_id in 0..count {
            let font = &self.font.fonts[font_id];
            if font.flags & (PDF_FONT_FLAG_IS_ALIAS | PDF_FONT_FLAG_IS_REENCODE) != 0
                || font.reference.is_none()
            {
                continue;
            }
            if font.subtype == PDF_FONT_FONTTYPE_CIDTYPE0
                || font.subtype == PDF_FONT_FONTTYPE_CIDTYPE2
            {
                continue;
            }
            let fid = font_id as i32;
            self.try_load_ToUnicode_CMap(fid)?;
            let font = &self.font.fonts[font_id];
            match font.subtype {
                PDF_FONT_FONTTYPE_TYPE1 => {
                    if font.flags & PDF_FONT_FLAG_BASEFONT == 0 {
                        self.pdf_font_load_type1(fid)?;
                    }
                }
                PDF_FONT_FONTTYPE_TYPE1C => {
                    self.pdf_font_load_type1c(fid)?;
                }
                PDF_FONT_FONTTYPE_TRUETYPE => {
                    self.pdf_font_load_truetype(fid)?;
                }
                PDF_FONT_FONTTYPE_TYPE3 => {
                    crate::fatal!("PK fonts are not supported");
                }
                PDF_FONT_FONTTYPE_TYPE0 => {
                    self.pdf_font_load_type0(fid)?;
                }
                _ => {}
            }
            let font = &self.font.fonts[font_id];
            if font.encoding_id >= 0 && font.subtype != PDF_FONT_FONTTYPE_TYPE0 {
                let enc = font.encoding_id;
                // (C passes the font's usedchars, NULL when the font was
                // never shown; pdf_encoding_add_usedchars then does
                // nothing)
                if let Some(uc) = font.usedchars.clone() {
                    let uc = uc.borrow();
                    self.pdf_encoding_add_usedchars(enc, &uc)?;
                }
            }
        }
        self.pdf_encoding_complete()?;
        for font_id in 0..count {
            let font = &self.font.fonts[font_id];
            if font.flags & (PDF_FONT_FLAG_IS_ALIAS | PDF_FONT_FLAG_IS_REENCODE) != 0
                || font.reference.is_none()
            {
                continue;
            }
            if font.subtype == PDF_FONT_FONTTYPE_CIDTYPE0
                || font.subtype == PDF_FONT_FONTTYPE_CIDTYPE2
            {
                self.pdf_font_load_cidfont(font_id as i32)?;
            }
        }
        for font_id in 0..count {
            let fid = font_id as i32;
            let font = &self.font.fonts[font_id];
            if font.flags & (PDF_FONT_FLAG_IS_ALIAS | PDF_FONT_FLAG_IS_REENCODE) != 0
                || font.reference.is_none()
            {
                self.pdf_flush_font(fid)?;
                self.pdf_clean_font_struct(fid);
                continue;
            }
            let (enc, subtype) = (font.encoding_id, font.subtype);
            if enc >= 0
                && subtype != PDF_FONT_FONTTYPE_TYPE0
                && subtype != PDF_FONT_FONTTYPE_CIDTYPE0
                && subtype != PDF_FONT_FONTTYPE_CIDTYPE2
            {
                let resource = self.font.fonts[font_id].resource.expect("resource");
                if let Some(enc_obj) = self.pdf_get_encoding_obj(enc)? {
                    if subtype == PDF_FONT_FONTTYPE_TRUETYPE {
                        if self.pdf_encoding_is_predefined(enc)? && self.o.is_name(Some(enc_obj)) {
                            let l = self.o.link(enc_obj)?;
                            self.o.put(resource, b"Encoding", l)?;
                        }
                    } else {
                        let v = if self.o.is_name(Some(enc_obj)) {
                            self.o.link(enc_obj)?
                        } else {
                            self.o.ref_obj(enc_obj)?
                        };
                        self.o.put(resource, b"Encoding", v)?;
                    }
                }
                if self.o.lookup_dict(resource, b"ToUnicode")?.is_none()
                    && let Some(tounicode) = self.pdf_encoding_get_tounicode(enc)?
                {
                    let r = self.o.ref_obj(tounicode)?;
                    self.o.put(resource, b"ToUnicode", r)?;
                }
            } else if subtype == PDF_FONT_FONTTYPE_TRUETYPE {
                let resource = self.font.fonts[font_id].resource.expect("resource");
                self.o
                    .put_name(resource, b"Encoding", b"MacRomanEncoding")?;
            }
            self.pdf_flush_font(fid)?;
            self.pdf_clean_font_struct(fid);
        }
        self.font.fonts.clear();
        self.font.capacity = 0;
        self.CMap_cache_close();
        self.pdf_close_encodings()?;
        self.agl_close_map();
        Ok(())
    }

    /// `GET_FONT` (static): `font_id`, or the font an alias stands for
    /// (-1 when out of range).
    #[allow(non_snake_case)]
    fn GET_FONT(&self, font_id: i32) -> i32 {
        if font_id >= 0 && (font_id as usize) < self.font.fonts.len() {
            let font = &self.font.fonts[font_id as usize];
            if font.flags & PDF_FONT_FLAG_IS_ALIAS != 0 {
                return font.font_id;
            }
            return font_id;
        }
        -1
    }

    fn check_id(&self, font_id: i32) -> Result<()> {
        if font_id < 0 || font_id as usize >= self.font.fonts.len() {
            crate::fatal!("Invalid font ID: {}", font_id);
        }
        Ok(())
    }

    /// `GET_FONT`, then the font a re-encoded font stands for.
    pub(crate) fn get_font_reencoded(&self, font_id: i32) -> usize {
        let mut f = self.GET_FONT(font_id);
        if self.font.fonts[f as usize].flags & PDF_FONT_FLAG_IS_REENCODE != 0 {
            f = self.GET_FONT(self.font.fonts[f as usize].font_id);
        }
        f as usize
    }

    /// `pdf_get_font_data`.
    pub fn pdf_get_font_data(&mut self, font_id: i32) -> Result<&mut PdfFont> {
        self.check_id(font_id)?;
        Ok(&mut self.font.fonts[font_id as usize])
    }

    /// `pdf_get_font_ident`: a copy.
    pub fn pdf_get_font_ident(&mut self, font_id: i32) -> Result<Option<Vec<u8>>> {
        self.check_id(font_id)?;
        Ok(self.font.fonts[font_id as usize].ident.clone())
    }

    /// `pdf_get_font_subtype`.
    pub fn pdf_get_font_subtype(&mut self, font_id: i32) -> Result<i32> {
        self.check_id(font_id)?;
        let f = self.get_font_reencoded(font_id);
        Ok(self.font.fonts[f].subtype)
    }

    /// `pdf_get_font_reference`: a new link (as in C).
    pub fn pdf_get_font_reference(&mut self, font_id: i32) -> Result<Obj> {
        self.check_id(font_id)?;
        let f = self.get_font_reencoded(font_id);
        if self.font.fonts[f].reference.is_none() {
            let res = self.pdf_font_get_resource(f as i32)?;
            let r = self.o.ref_obj(res)?;
            self.font.fonts[f].reference = Some(r);
        }
        if self.font.fonts[f].subtype == PDF_FONT_FONTTYPE_TYPE0 {
            let resource = self.font.fonts[f].resource.expect("resource");
            if self.o.lookup_dict(resource, b"DescendantFonts")?.is_none() {
                let array = self.o.new_array();
                let d = self.font.fonts[f].type0.descendant;
                let r = self.pdf_get_font_reference(d)?;
                self.o.add_array(array, r)?;
                self.o.put(resource, b"DescendantFonts", array)?;
            }
        }
        let r = self.font.fonts[f].reference.expect("reference");
        self.o.link(r)
    }

    /// `pdf_get_font_resource` (not linked).
    pub fn pdf_get_font_resource(&mut self, font_id: i32) -> Result<Obj> {
        self.check_id(font_id)?;
        let f = self.get_font_reencoded(font_id);
        match self.font.fonts[f].resource {
            Some(r) => Ok(r),
            None => {
                let d = self.o.new_dict();
                self.font.fonts[f].resource = Some(d);
                Ok(d)
            }
        }
    }

    /// `pdf_get_font_usedchars`: the shared array (made, 256 bytes, for a
    /// simple font that has none); none for a Type 0 font without one.
    pub fn pdf_get_font_usedchars(&mut self, font_id: i32) -> Result<Option<UsedChars>> {
        self.check_id(font_id)?;
        let f = self.get_font_reencoded(font_id);
        let font = &mut self.font.fonts[f];
        if font.subtype != PDF_FONT_FONTTYPE_TYPE0 && font.usedchars.is_none() {
            font.usedchars = Some(Rc::new(RefCell::new(vec![0u8; 256])));
        }
        Ok(font.usedchars.clone())
    }

    /// `pdf_get_font_encoding`.
    pub fn pdf_get_font_encoding(&mut self, font_id: i32) -> Result<i32> {
        self.check_id(font_id)?;
        let f = self.GET_FONT(font_id);
        Ok(self.font.fonts[f as usize].encoding_id)
    }

    /// `pdf_get_font_wmode`.
    pub fn pdf_get_font_wmode(&mut self, font_id: i32) -> Result<i32> {
        self.check_id(font_id)?;
        let f = self.get_font_reencoded(font_id);
        let font = &self.font.fonts[f];
        if font.subtype == PDF_FONT_FONTTYPE_TYPE0 {
            Ok(font.type0.wmode)
        } else {
            Ok(0)
        }
    }

    /// `pdf_font_resource_name`: `F<id>` (C writes it to `buf` and returns
    /// its length).
    pub fn pdf_font_resource_name(&mut self, font_id: i32) -> Result<Vec<u8>> {
        self.check_id(font_id)?;
        let mut font_id = font_id;
        if self.font.fonts[font_id as usize].flags & PDF_FONT_FLAG_IS_ALIAS != 0 {
            font_id = self.font.fonts[font_id as usize].font_id;
        }
        let f = self.GET_FONT(font_id);
        if self.font.fonts[f as usize].flags & PDF_FONT_FLAG_IS_REENCODE != 0 {
            font_id = self.font.fonts[f as usize].font_id;
        }
        let mut b = crate::fmt::Buf::new();
        b.push(b'F');
        b.int(font_id);
        Ok(b.0)
    }

    /// `try_load_ToUnicode_CMap` (static).
    #[allow(non_snake_case)]
    fn try_load_ToUnicode_CMap(&mut self, font_id: i32) -> Result<i32> {
        let font = &self.font.fonts[font_id as usize];
        if font.subtype == PDF_FONT_FONTTYPE_TYPE0 {
            return Ok(0);
        }
        let ident = font.ident.clone().expect("ident");
        let mrec = self.pdf_lookup_fontmap_record(&ident);
        let has_tounicode = mrec.as_ref().and_then(|m| m.opt.tounicode.clone());
        let cmap_name = has_tounicode.clone().unwrap_or_else(|| ident.clone());
        if let Some(tounicode) = self.pdf_load_ToUnicode_stream(&cmap_name)? {
            if self.o.type_of(Some(tounicode)) != crate::obj::PDF_STREAM {
                crate::fatal!("Object returned by pdf_load_ToUnicode_stream() not stream object!");
            } else if self.o.stream_length(tounicode)? > 0 {
                let fontdict = self.pdf_font_get_resource(font_id)?;
                let r = self.o.ref_obj(tounicode)?;
                self.o.put(fontdict, b"ToUnicode", r)?;
            }
            self.o.release(tounicode)?;
        }
        Ok(0)
    }

    /// `pdf_font_findresource`: the font_id, or -1.
    pub fn pdf_font_findresource(&mut self, ident: &[u8], scale: f64) -> i32 {
        for (font_id, font) in self.font.fonts.iter().enumerate() {
            let found = match font.subtype {
                PDF_FONT_FONTTYPE_TYPE1
                | PDF_FONT_FONTTYPE_TYPE1C
                | PDF_FONT_FONTTYPE_TRUETYPE
                | PDF_FONT_FONTTYPE_TYPE0 => font.ident.as_deref() == Some(ident),
                PDF_FONT_FONTTYPE_TYPE3 => {
                    font.ident.as_deref() == Some(ident) && scale == font.point_size
                }
                _ => false,
            };
            if found {
                return font_id as i32;
            }
        }
        -1
    }

    /// `create_font_alias` (static).
    fn create_font_alias(&mut self, ident: &[u8], font_id: i32) -> i32 {
        if font_id < 0 || font_id as usize >= self.font.fonts.len() {
            return -1;
        }
        let (subtype, encoding_id) = {
            let src = &self.font.fonts[font_id as usize];
            (src.subtype, src.encoding_id)
        };
        let mut font = PdfFont::default();
        pdf_init_font_struct(&mut font);
        font.ident = Some(ident.to_vec());
        font.font_id = font_id;
        font.subtype = subtype;
        font.encoding_id = encoding_id;
        font.flags |= PDF_FONT_FLAG_IS_ALIAS;
        self.font.fonts.push(font);
        (self.font.fonts.len() - 1) as i32
    }

    /// `create_font_reencoded` (static).
    fn create_font_reencoded(&mut self, ident: &[u8], font_id: i32, cmap_id: i32) -> i32 {
        let mut font = PdfFont::default();
        pdf_init_font_struct(&mut font);
        font.ident = Some(ident.to_vec());
        font.font_id = font_id;
        font.subtype = PDF_FONT_FONTTYPE_TYPE0;
        font.encoding_id = cmap_id;
        font.flags |= PDF_FONT_FLAG_IS_REENCODE | PDF_FONT_FLAG_USEDCHAR_SHARED;
        self.font.fonts.push(font);
        (self.font.fonts.len() - 1) as i32
    }

    /// `pdf_font_load_font`: the font_id, or -1.
    pub fn pdf_font_load_font(
        &mut self,
        ident: &[u8],
        font_scale: f64,
        mrec: Option<&FontmapRec>,
    ) -> Result<i32> {
        let mut encoding_id = -1;
        let mut cmap_id = -1;
        let fontname: Vec<u8> = match mrec {
            Some(m) => m.font_name.clone().unwrap_or_default(),
            None => ident.to_vec(),
        };
        if let Some(m) = mrec
            && m.opt.use_glyph_encoding != 0
            && let Some(enc) = &m.enc_name
        {
            let wmode = if enc.as_slice() == b"Identity-V" {
                1
            } else {
                0
            };
            cmap_id = self.otf_try_load_GID_to_CID_map(&fontname, m.opt.index, wmode)?;
        }
        if cmap_id < 0
            && let Some(m) = mrec
            && let Some(enc) = &m.enc_name
        {
            let vert = i32::from(m.opt.flags & crate::fontmap::FONTMAP_OPT_VERT != 0);
            if enc.as_slice() == b"unicode" {
                cmap_id = self.otf_load_Unicode_CMap(
                    &fontname,
                    m.opt.index,
                    m.opt.otl_tags.as_deref(),
                    vert,
                )?;
                if cmap_id < 0 {
                    cmap_id =
                        self.t1_load_UnicodeCMap(&fontname, m.opt.otl_tags.as_deref(), vert)?;
                }
            } else if !contains(enc, b".enc") || contains(enc, b".cmap") {
                cmap_id = self.CMap_cache_find(enc)?;
            }
            if cmap_id < 0 {
                encoding_id = self.pdf_encoding_findresource(enc)?;
            }
        }
        if let Some(m) = mrec
            && m.enc_name.is_some()
            && cmap_id < 0
            && encoding_id < 0
        {
            return Ok(-1);
        }
        if let Some(m) = mrec
            && cmap_id >= 0
        {
            // Composite font.
            let (csi, wmode) = {
                let cmap = self.CMap_cache_get(cmap_id)?;
                let csi = if cmap.CMap_is_Identity() {
                    None
                } else {
                    cmap.CMap_get_CIDSysInfo().cloned()
                };
                (csi, cmap.CMap_get_wmode())
            };
            let count = self.font.fonts.len() as i32;
            let mut cid_id =
                self.pdf_font_cidfont_lookup_cache(count, &fontname, csi.as_ref(), &m.opt)?;
            if cid_id >= 0 {
                let found = self.font.fonts.iter().position(|f| {
                    f.subtype == PDF_FONT_FONTTYPE_TYPE0
                        && f.type0.wmode == wmode
                        && f.type0.descendant == cid_id
                });
                if let Some(font_id) = found {
                    let font = &self.font.fonts[font_id];
                    if font.encoding_id == cmap_id {
                        if font.ident.as_deref() == Some(ident) {
                            return Ok(font_id as i32);
                        }
                        return Ok(self.create_font_alias(ident, font_id as i32));
                    }
                    return Ok(self.create_font_reencoded(ident, font_id as i32, cmap_id));
                }
            }
            if cid_id < 0 {
                cid_id = self.font.fonts.len() as i32;
                let mut cidfont = PdfFont::default();
                pdf_init_font_struct(&mut cidfont);
                self.font.fonts.push(cidfont);
                if self.pdf_font_open_cidfont(cid_id, &fontname, csi.as_ref(), &m.opt)? < 0 {
                    self.pdf_clean_font_struct(cid_id);
                    self.font.fonts.pop();
                    return Ok(-1);
                }
            }
            let font_id = self.font.fonts.len() as i32;
            let mut font = PdfFont::default();
            pdf_init_font_struct(&mut font);
            self.font.fonts.push(font);
            if self.pdf_font_open_type0(font_id, cid_id, wmode)? < 0 {
                self.pdf_clean_font_struct(font_id);
                self.font.fonts.pop();
                return Ok(-1);
            }
            let font = &mut self.font.fonts[font_id as usize];
            font.ident = Some(ident.to_vec());
            font.subtype = PDF_FONT_FONTTYPE_TYPE0;
            font.encoding_id = cmap_id;
            Ok(font_id)
        } else {
            // Simple font: always embed.
            for font_id in 0..self.font.fonts.len() {
                let font = &self.font.fonts[font_id];
                if font.flags & PDF_FONT_FLAG_IS_ALIAS != 0 {
                    continue;
                }
                let hit = match font.subtype {
                    PDF_FONT_FONTTYPE_TYPE1
                    | PDF_FONT_FONTTYPE_TYPE1C
                    | PDF_FONT_FONTTYPE_TRUETYPE => {
                        font.filename.as_deref() == Some(&fontname[..])
                            && encoding_id == font.encoding_id
                            && mrec.is_none_or(|m| m.opt.index == font.index)
                    }
                    PDF_FONT_FONTTYPE_TYPE3 => {
                        font.filename.as_deref() == Some(&fontname[..])
                            && font_scale == font.point_size
                    }
                    _ => false,
                };
                if hit {
                    if font.ident.as_deref() == Some(ident) {
                        return Ok(font_id as i32);
                    }
                    return Ok(self.create_font_alias(ident, font_id as i32));
                }
            }
            let font_id = self.font.fonts.len() as i32;
            let mut font = PdfFont::default();
            pdf_init_font_struct(&mut font);
            font.ident = Some(ident.to_vec());
            font.encoding_id = encoding_id;
            font.filename = Some(fontname.clone());
            font.point_size = font_scale;
            font.index = mrec.map_or(0, |m| m.opt.index);
            if mrec.is_some_and(|m| m.opt.flags & crate::fontmap::FONTMAP_OPT_NOEMBED != 0) {
                font.flags |= PDF_FONT_FLAG_NOEMBED;
            }
            let (index, embedding) = (
                font.index as i32,
                i32::from(font.flags & PDF_FONT_FLAG_NOEMBED == 0),
            );
            self.font.fonts.push(font);
            let subtype = if self.pdf_font_open_type1(
                font_id,
                &fontname,
                index,
                encoding_id,
                embedding,
            )? >= 0
            {
                PDF_FONT_FONTTYPE_TYPE1
            } else if self.pdf_font_open_type1c(
                font_id,
                &fontname,
                index,
                encoding_id,
                embedding,
            )? >= 0
            {
                PDF_FONT_FONTTYPE_TYPE1C
            } else if self.pdf_font_open_truetype(
                font_id,
                &fontname,
                index,
                encoding_id,
                embedding,
            )? >= 0
            {
                PDF_FONT_FONTTYPE_TRUETYPE
            } else {
                // (PK fonts, pdf_font_open_pkfont: not ported)
                self.pdf_clean_font_struct(font_id);
                self.font.fonts.pop();
                return Ok(-1);
            };
            self.font.fonts[font_id as usize].subtype = subtype;
            Ok(font_id)
        }
    }

    /// `pdf_font_get_resource`: made (a dict with `/Type /Font` and the
    /// subtype) on first use; not linked.
    pub fn pdf_font_get_resource(&mut self, font_id: i32) -> Result<Obj> {
        let fi = font_id as usize;
        if let Some(r) = self.font.fonts[fi].resource {
            return Ok(r);
        }
        let d = self.o.new_dict();
        self.o.put_name(d, b"Type", b"Font")?;
        let _ = match self.font.fonts[fi].subtype {
            PDF_FONT_FONTTYPE_TYPE1 | PDF_FONT_FONTTYPE_TYPE1C => {
                self.o.put_name(d, b"Subtype", b"Type1")?
            }
            PDF_FONT_FONTTYPE_TYPE3 => self.o.put_name(d, b"Subtype", b"Type3")?,
            PDF_FONT_FONTTYPE_TRUETYPE => self.o.put_name(d, b"Subtype", b"TrueType")?,
            _ => false,
        };
        self.font.fonts[fi].resource = Some(d);
        Ok(d)
    }

    /// `pdf_font_get_descriptor`: made on first use; not linked. (C
    /// returns NULL for a Type 0 font; no caller asks for one.)
    pub fn pdf_font_get_descriptor(&mut self, font_id: i32) -> Result<Obj> {
        let fi = font_id as usize;
        assert!(
            self.font.fonts[fi].subtype != PDF_FONT_FONTTYPE_TYPE0,
            "descriptor of a Type 0 font"
        );
        if let Some(d) = self.font.fonts[fi].descriptor {
            return Ok(d);
        }
        let d = self.o.new_dict();
        self.o.put_name(d, b"Type", b"FontDescriptor")?;
        self.font.fonts[fi].descriptor = Some(d);
        Ok(d)
    }

    /// `pdf_font_get_uniqueTag`: the six letters (made on first use).
    #[allow(non_snake_case)]
    pub fn pdf_font_get_uniqueTag(&mut self, font_id: i32) -> Vec<u8> {
        let fi = font_id as usize;
        if self.font.fonts[fi].unique_id[0] == 0 {
            let tag = self.pdf_font_make_uniqueTag();
            self.font.fonts[fi].unique_id[..6].copy_from_slice(&tag);
        }
        self.font.fonts[fi].unique_id[..6].to_vec()
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
    ) -> Result<i32> {
        let tolerance = 1.0;
        let tfm_id = self.tfm_open(ident, false)?;
        if tfm_id < 0 {
            return Ok(0);
        }
        let mut sum = 0.0;
        let mut count = 0;
        for code in firstchar..=lastchar {
            if usedchars[code as usize] != 0 {
                let width = 1000. * self.tfm_get_width(tfm_id, code)?;
                let mut diff = widths[code as usize] - width;
                diff = if diff < 0.0 { -diff } else { diff };
                if diff > tolerance {
                    sum += diff;
                }
                count += 1;
            }
        }
        if sum > 0.5 * f64::from(count) * tolerance {
            Ok(-1)
        } else {
            Ok(0)
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
