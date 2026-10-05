//! cid.c, cid.h, cid_basefont.h: CIDFonts (dispatch to CIDFontType0/2
//! and the never-embedded base fonts), character collections.
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`.

#![allow(non_snake_case)]

use core::cell::RefCell;

use crate::fontmap::{FONTMAP_OPT_NOEMBED, FontmapOpt};
use crate::pdffont::{
    CIDFONT_FLAG_TRUETYPE, CIDFONT_FLAG_TYPE1, CIDFONT_FLAG_TYPE1C, FONT_STYLE_BOLD,
    FONT_STYLE_BOLDITALIC, FONT_STYLE_ITALIC, PDF_FONT_FLAG_BASEFONT, PDF_FONT_FONTTYPE_CIDTYPE0,
    PDF_FONT_FONTTYPE_CIDTYPE2, PDF_FONT_FONTTYPE_TRUETYPE, PDF_FONT_FONTTYPE_TYPE1,
    PDF_FONT_FONTTYPE_TYPE1C, UsedChars,
};
use crate::pdffont::{CidOpt, CidSysInfo};
use crate::prelude::*;

/// `PDF_CID_SUPPORT_MIN`.
pub const PDF_CID_SUPPORT_MIN: i32 = 2;
/// `PDF_CID_SUPPORT_MAX`.
pub const PDF_CID_SUPPORT_MAX: i32 = 6;
/// `SUP_IDX_MAX`.
pub const SUP_IDX_MAX: i32 = 20;
/// `UCS_CC`.
pub const UCS_CC: usize = 0;
/// `ACC_START`.
pub const ACC_START: usize = 1;
/// `ACC_END`.
pub const ACC_END: usize = 4;
/// `CIDFONT_FORCE_FIXEDPITCH`.
pub const CIDFONT_FORCE_FIXEDPITCH: i32 = 1 << 1;

/// `CIDFont_stdcc_def`: `(registry, ordering, supplement[pdf version - 10])`,
/// the Unicode and PDF standard character collections (without C's NULL end).
pub const CIDFONT_STDCC_DEF: &[(&[u8], &[u8], [i32; 21])] = &[
    (
        b"Adobe",
        b"UCS",
        [
            -1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
    (
        b"Adobe",
        b"GB1",
        [
            -1, -1, 0, 2, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        ],
    ),
    (
        b"Adobe",
        b"CNS1",
        [
            -1, -1, 0, 0, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        ],
    ),
    (
        b"Adobe",
        b"Japan1",
        [
            -1, -1, 2, 2, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
        ],
    ),
    (
        b"Adobe",
        b"Korea1",
        [
            -1, -1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
        ],
    ),
    (
        b"Adobe",
        b"Identity",
        [
            -1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
];

/// `CIDFont_stdcc_alias`: `(name, index into CIDFONT_STDCC_DEF)`, in C's
/// order (prefix matched).
pub const CIDFONT_STDCC_ALIAS: &[(&[u8], usize)] = &[
    (b"AU", 0),
    (b"AG1", 1),
    (b"AC1", 2),
    (b"AJ1", 3),
    (b"AK1", 4),
    (b"AI", 5),
    (b"UCS", 0),
    (b"GB1", 1),
    (b"CNS1", 2),
    (b"Japan1", 3),
    (b"Korea1", 4),
    (b"Identity", 5),
    (b"U", 0),
    (b"G", 1),
    (b"C", 2),
    (b"J", 3),
    (b"K", 4),
    (b"I", 5),
];

/// `CSI_IDENTITY`'s registry and ordering (Adobe-Identity-0).
pub const CSI_IDENTITY_RO: (&[u8], &[u8], i32) = (b"Adobe", b"Identity", 0);
/// `CSI_UNICODE`'s registry and ordering (Adobe-UCS-0).
pub const CSI_UNICODE_RO: (&[u8], &[u8], i32) = (b"Adobe", b"UCS", 0);

/// `cid_basefont` (cid_basefont.h): `(fontname, fontdict, descriptor)`,
/// the never-embedded fixed-pitch CJK fonts (without the C `NULL` end).
pub const CID_BASEFONT: &[(&[u8], &[u8], &[u8])] = &[
    (
        b"Ryumin-Light",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /DW 1000 /W [  231   632 500  8718 [500 500] ]>>",
        b"<< /CapHeight 709 /Ascent 723 /Descent -241 /StemV 69 /FontBBox [-170 -331 1024 903] /ItalicAngle 0 /Flags 6 /Style << /Panose <010502020300000000000000> >> >>",
    ),
    (
        b"GothicBBB-Medium",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo <<  /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /DW 1000 /W [  231   632 500  8718 [500 500] ]>>",
        b"<< /CapHeight 737 /Ascent 752 /Descent -271 /StemV 99 /FontBBox [-174 -268 1001 944] /ItalicAngle 0 /Flags 4 /Style << /Panose <0801020b0500000000000000> >> >>",
    ),
    (
        b"MHei-Medium-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 0 >> /DW 1000 /W [13648 13742 500 17603 [500] ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-45 -250 1015 887] /ItalicAngle 0 /Flags 4 /XHeight 553 /Style << /Panose <000001000600000000000000> >> >>",
    ),
    (
        b"MSung-Light-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 0 >> /DW 1000 /W [13648 13742 500 17603 [500] ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-160 -249 1015 888] /ItalicAngle 0 /Flags 6 /XHeight 553 /Style << /Panose <000000000400000000000000> >> >>",
    ),
    (
        b"STSong-Light-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /DW 1000 /W [  814 939 500  7716 [500] 22355 [500 500] 22357 [500] ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Flags 6 /XHeight 599 /Style << /Panose <000000000400000000000000> >> >>",
    ),
    (
        b"STHeiti-Regular-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 1 >> /DW 1000 /W [  814 939 500  7716 [500] 22355 [500 500] 22357 [500] ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-34 -250 1000 882] /ItalicAngle 0 /Flags 4 /XHeight 599 /Style << /Panose <000001000600000000000000> >> >>",
    ),
    (
        b"HeiseiKakuGo-W5-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement  2 >> /DW 1000 /W [  231   632 500  8718 [500 500] ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -221 /StemV 114 /FontBBox [-92 -250 1010 922] /ItalicAngle 0 /Flags 4 /XHeight 553 /Style << /Panose <0801020b0600000000000000> >> >>",
    ),
    (
        b"HeiseiMin-W3-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /DW 1000 /W [  231   632 500  8718 [500 500] ]>>",
        b"<< /Ascent 723 /CapHeight 709 /Descent -241 /StemV 69 /FontBBox [-123 -257 1001 910] /ItalicAngle 0 /Flags 6 /XHeight 450 /Style << /Panose <010502020400000000000000> >> >>",
    ),
    (
        b"HYGoThic-Medium-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 1 >> /DW 1000 /W [   97 [500]  8094  8190 500 ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-6 -145 1003 880] /ItalicAngle 0 /Flags 4 /XHeight 553 /Style << /Panose <000001000600000000000000> >> >>",
    ),
    (
        b"HYSMyeongJo-Medium-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 1 >> /DW 1000 /W [   97 [500]  8094  8190 500 ]>>",
        b"<< /Ascent 752 /CapHeight 737 /Descent -271 /StemV 58 /FontBBox [-0 -148 1001 880] /ItalicAngle 0 /Flags 6 /XHeight 553 /Style << /Panose <000000000600000000000000> >> >>",
    ),
    (
        b"MSungStd-Light-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 4 >> /DW 1000 /W [13648 13742 500 17603 [500] ]>>",
        b"<< /Ascent 880 /CapHeight 662 /Descent -120 /StemV 54 /FontBBox [-160 -249 1015 1071] /ItalicAngle 0 /Flags 6 /Style << /Panose <000000000400000000000000> >> >>",
    ),
    (
        b"STSongStd-Light-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /DW 1000 /W [  814 939 500  7716 [500] 22355 [500 500] 22357 [500] ]>>",
        b"<< /Ascent 880 /CapHeight 626 /Descent -120 /StemV 44 /FontBBox [-134 -254 1001 905] /ItalicAngle 0 /Flags 6 /Style << /Panose <000000000400000000000000> >> >>",
    ),
    (
        b"HYSMyeongJoStd-Medium-Acro",
        b"<< /Subtype /CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 2 >> /DW 1000 /W [   97 [500]  8094  8190 500 ]>>",
        b"<< /Ascent 880 /CapHeight 720 /Descent -120 /StemV 60 /FontBBox [-28 -148 1001 880] /ItalicAngle 0 /Flags 6 /Style << /Panose <000000000600000000000000> >> >>",
    ),
    (
        b"AdobeMingStd-Light-Acro",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (CNS1) /Supplement 4 >> /DW 1000 /W [13648 13742 500 17603 [500] ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 48 /CapHeight 731 /FontBBox [-38 -121 1002 918] /ItalicAngle 0 /Flags 6 /XHeight 466 /Style << /Panose <000002020300000000000000> >> >>",
    ),
    (
        b"AdobeSongStd-Light-Acro",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /DW 1000 /W [  814 939 500  7716 [500] 22355 [500 500] 22357 [500] ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 66 /CapHeight 626 /FontBBox [-134 -254 1001 905] /ItalicAngle 0 /Flags 6 /XHeight 416 /Style << /Panose <000002020300000000000000> >> >>",
    ),
    (
        b"KozMinPro-Regular-Acro",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 4 >> /DW 1000 /W [  231   632 500  8718 [500 500]  9738  9757 250  9758  9778 333 12063 12087 500 ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 86 /CapHeight 740 /FontBBox [-195 -272 1110 1075] /ItalicAngle 0 /Flags 6 /XHeight 502 /Style << /Panose <000002020400000000000000> >> >>",
    ),
    (
        b"KozGoPro-Medium-Acro",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering(Japan1) /Supplement 4 >> /DW 1000 /W [  231   632 500  8718 [500 500]  9738  9757 250  9758  9778 333 12063 12087 500 ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 99 /CapHeight 763 /FontBBox [-149 -374 1254 1008] /ItalicAngle 0 /Flags 4 /XHeight 549 /Style << /Panose <0000020b0700000000000000> >> >>",
    ),
    (
        b"AdobeMyungjoStd-Medium-Acro",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 2 >> /DW 1000 /W [   97 [500]  8094  8190 500 ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 99 /CapHeight 719 /FontBBox [-28 -148 1001 880] /ItalicAngle 0 /Flags 6 /XHeight 478 /Style << /Panose <000002020600000000000000> >> >>",
    ),
    (
        b"KozMinProVI-Regular",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo <<   /Registry (Adobe)   /Ordering (Japan1)   /Supplement 6 >> /DW 1000 /W [  231   632 500   8718 [500 500]   9738  9757 250   9758  9778 333   12063 12087 500 ]        >>",
        b"<< /Ascent 880 /Descent -120 /StemV 86 /CapHeight 742 /FontBBox [-437 -340 1144 1317] /ItalicAngle 0 /Flags 6 /XHeight 503 /Style <<   /Panose <000002020400000000000000> >>      >>",
    ),
    (
        b"AdobeHeitiStd-Regular",
        b"<< /Subtype/CIDFontType0 /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /DW 1000 /W [  814 939 500  7716 [500] 22355 [500 500] 22357 [500] ]>>",
        b"<< /Ascent 880 /Descent -120 /StemV 66 /CapHeight 626 /FontBBox [-134 -254 1001 905] /ItalicAngle 0 /Flags 6 /XHeight 416 /Style << /Panose <000002020300000000000000> >> >>",
    ),
];

/// cid.c's globals.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `opt_flags_cidfont` (`CIDFONT_FORCE_FIXEDPITCH`…).
    pub opt_flags_cidfont: i32,
}

/// `CSI_IDENTITY`: a new Adobe-Identity-0.
#[must_use]
pub fn CSI_IDENTITY() -> CidSysInfo {
    CidSysInfo {
        registry: Some(b"Adobe".to_vec()),
        ordering: Some(b"Identity".to_vec()),
        supplement: 0,
    }
}

/// `CSI_UNICODE`: a new Adobe-UCS-0.
#[must_use]
pub fn CSI_UNICODE() -> CidSysInfo {
    CidSysInfo {
        registry: Some(b"Adobe".to_vec()),
        ordering: Some(b"UCS".to_vec()),
        supplement: 0,
    }
}

/// `strcmp(a, b) == 0` on C strings that may be NULL (C would crash on a
/// NULL; an absent string compares as empty here).
fn streq(a: &Option<Vec<u8>>, b: &[u8]) -> bool {
    a.as_deref().unwrap_or(b"") == b
}

impl Dpx {
    /// `CIDFont_set_flags`: ORs into `opt_flags_cidfont`.
    pub fn CIDFont_set_flags(&mut self, flags: i32) {
        self.cid.opt_flags_cidfont |= flags;
    }

    /// `CIDFont_get_usedchars`: the font's `usedchars` (8192 bytes,
    /// allocated zeroed on first use). Shared, as pdffont's `UsedChars`.
    pub fn CIDFont_get_usedchars(&mut self, font_id: i32) -> UsedChars {
        let font = &mut self.font.fonts[font_id as usize];
        font.usedchars
            .get_or_insert_with(|| Rc::new(RefCell::new(vec![0u8; 8192])))
            .clone()
    }

    /// `CIDFont_get_usedchars_v`: `cid.usedchars_v`, likewise.
    pub fn CIDFont_get_usedchars_v(&mut self, font_id: i32) -> UsedChars {
        let font = &mut self.font.fonts[font_id as usize];
        font.cid
            .usedchars_v
            .get_or_insert_with(|| Rc::new(RefCell::new(vec![0u8; 8192])))
            .clone()
    }

    /// `CIDFont_is_ACCFont`: an Adobe CJK collection (GB1, CNS1, Japan1,
    /// Korea1).
    pub fn CIDFont_is_ACCFont(&self, font_id: i32) -> bool {
        let csi = &self.font.fonts[font_id as usize].cid.csi;
        for i in ACC_START..=ACC_END {
            if streq(&csi.registry, CIDFONT_STDCC_DEF[i].0)
                && streq(&csi.ordering, CIDFONT_STDCC_DEF[i].1)
            {
                return true;
            }
        }
        false
    }

    /// `CIDFont_is_UCSFont`: ordering UCS or UCS2.
    pub fn CIDFont_is_UCSFont(&self, font_id: i32) -> bool {
        let csi = &self.font.fonts[font_id as usize].cid.csi;
        streq(&csi.ordering, b"UCS") || streq(&csi.ordering, b"UCS2")
    }

    /// `source_font_type` (static): from `CIDFONT_FLAG_*`.
    fn source_font_type(&self, font_id: i32) -> i32 {
        let font = &self.font.fonts[font_id as usize];
        let mut type_ = PDF_FONT_FONTTYPE_CIDTYPE0;
        if font.flags & CIDFONT_FLAG_TYPE1 != 0 {
            type_ = PDF_FONT_FONTTYPE_TYPE1;
        } else if font.flags & CIDFONT_FLAG_TYPE1C != 0 {
            type_ = PDF_FONT_FONTTYPE_TYPE1C;
        } else if font.flags & CIDFONT_FLAG_TRUETYPE != 0 {
            type_ = PDF_FONT_FONTTYPE_TRUETYPE;
        }
        type_
    }

    /// `pdf_font_load_cidfont`.
    pub fn pdf_font_load_cidfont(&mut self, font_id: i32) {
        if font_id < 0 || self.font.fonts[font_id as usize].reference.is_none() {
            return;
        }

        let mut error = 0;
        match self.font.fonts[font_id as usize].subtype {
            PDF_FONT_FONTTYPE_CIDTYPE0 => match self.source_font_type(font_id) {
                PDF_FONT_FONTTYPE_TYPE1 => error = self.CIDFont_type0_t1dofont(font_id),
                PDF_FONT_FONTTYPE_TYPE1C => error = self.CIDFont_type0_t1cdofont(font_id),
                _ => error = self.CIDFont_type0_dofont(font_id),
            },
            PDF_FONT_FONTTYPE_CIDTYPE2 => error = self.CIDFont_type2_dofont(font_id),
            _ => {}
        }

        if error != 0 {
            error!(
                "Error occurred while loading font: {}",
                String::from_utf8_lossy(
                    self.font.fonts[font_id as usize]
                        .filename
                        .as_deref()
                        .unwrap_or_default()
                )
            );
        }
    }

    /// `CIDFont_base_open` (static): one of `CID_BASEFONT`, or -1.
    fn CIDFont_base_open(&mut self, font_id: i32, name: &[u8], opt: &mut CidOpt) -> i32 {
        let Some(idx) = CID_BASEFONT.iter().position(|&(fontname, _, _)| {
            name == fontname
                || (name.len() + 5 == fontname.len() && name == &fontname[..fontname.len() - 5])
        }) else {
            return -1;
        };

        let mut fontname = name.to_vec();
        match opt.style {
            FONT_STYLE_BOLD => fontname.extend_from_slice(b",Bold"),
            FONT_STYLE_ITALIC => fontname.extend_from_slice(b",Italic"),
            FONT_STYLE_BOLDITALIC => fontname.extend_from_slice(b",BoldItalic"),
            _ => {}
        }
        let (_, dict_src, desc_src) = CID_BASEFONT[idx];
        let fontdict = self.o.parse_pdf_dict(dict_src, &mut 0, None);
        let descriptor = self.o.parse_pdf_dict(desc_src, &mut 0, None);
        let (Some(fontdict), Some(descriptor)) = (fontdict, descriptor) else {
            panic!("CIDFont_base_open: parse failed");
        };

        {
            let font = &mut self.font.fonts[font_id as usize];
            font.fontname = Some(fontname.clone());
            font.flags |= PDF_FONT_FLAG_BASEFONT;
        }
        {
            let tmp = self.o.lookup_dict(fontdict, b"CIDSystemInfo");
            assert!(self.o.is_dict(tmp));
            let tmp = tmp.unwrap();
            let registry = self
                .o
                .string_value(self.o.lookup_dict(tmp, b"Registry").unwrap())
                .to_vec();
            let ordering = self
                .o
                .string_value(self.o.lookup_dict(tmp, b"Ordering").unwrap())
                .to_vec();
            let supplement = self
                .o
                .number_value(self.o.lookup_dict(tmp, b"Supplement").unwrap())
                as i32;
            let font = &mut self.font.fonts[font_id as usize];
            font.cid.csi.registry = Some(registry);
            font.cid.csi.ordering = Some(ordering);
            font.cid.csi.supplement = supplement;
        }
        {
            let tmp = self.o.lookup_dict(fontdict, b"Subtype");
            assert!(self.o.is_name(tmp));
            let type_ = self.o.name_value(tmp.unwrap());
            let subtype = if type_ == b"CIDFontType0" {
                PDF_FONT_FONTTYPE_CIDTYPE0
            } else if type_ == b"CIDFontType2" {
                PDF_FONT_FONTTYPE_CIDTYPE2
            } else {
                error!("Unknown CIDFontType \"{}\"", String::from_utf8_lossy(type_));
            };
            self.font.fonts[font_id as usize].subtype = subtype;
        }

        if self.cid.opt_flags_cidfont & CIDFONT_FORCE_FIXEDPITCH != 0 {
            if self.o.lookup_dict(fontdict, b"W").is_some() {
                self.o.remove_dict(fontdict, b"W");
            }
            if self.o.lookup_dict(fontdict, b"W2").is_some() {
                self.o.remove_dict(fontdict, b"W2");
            }
        }

        self.o.put_name(fontdict, b"Type", b"Font");
        self.o.put_name(fontdict, b"BaseFont", &fontname);
        self.o.put_name(descriptor, b"Type", b"FontDescriptor");
        self.o.put_name(descriptor, b"FontName", &fontname);

        let font = &mut self.font.fonts[font_id as usize];
        font.resource = Some(fontdict);
        font.descriptor = Some(descriptor);

        opt.embed = 0;

        0
    }

    /// The `cid_opt` C builds from the map record (and the CMap's CSI).
    fn cid_make_opt(
        &self,
        map_name: &[u8],
        cmap_csi: Option<&CidSysInfo>,
        fmap_opt: &FontmapOpt,
    ) -> (CidOpt, i32) {
        let mut opt = CidOpt {
            style: fmap_opt.style,
            embed: if fmap_opt.flags & FONTMAP_OPT_NOEMBED != 0 {
                0
            } else {
                1
            },
            csi: CidSysInfo::default(),
            stemv: 0,
        };
        let mut has_csi = self.get_cidsysinfo(&mut opt.csi, map_name, Some(fmap_opt));
        opt.stemv = fmap_opt.stemv;

        if has_csi == 0
            && let Some(cmap_csi) = cmap_csi
        {
            // No CIDSystemInfo supplied explicitly. Copy from CMap's one
            // if available.
            opt.csi.registry = cmap_csi.registry.clone();
            opt.csi.ordering = cmap_csi.ordering.clone();
            opt.csi.supplement = cmap_csi.supplement;
            has_csi = 1;
        }
        (opt, has_csi)
    }

    /// `pdf_font_cidfont_lookup_cache`: among `self.font.fonts[..count]`,
    /// the font id, or -1 (may raise a cached CIDType2 font's supplement,
    /// as C does).
    pub fn pdf_font_cidfont_lookup_cache(
        &mut self,
        count: i32,
        map_name: &[u8],
        cmap_csi: Option<&CidSysInfo>,
        fmap_opt: &FontmapOpt,
    ) -> i32 {
        let (opt, has_csi) = self.cid_make_opt(map_name, cmap_csi, fmap_opt);

        // Here, we do not compare font->ident and map_name because of
        // implicit CIDSystemInfo supplied by CMap for TrueType.
        let mut font_id = 0;
        while font_id < count {
            let font = &mut self.font.fonts[font_id as usize];
            if font.subtype != PDF_FONT_FONTTYPE_CIDTYPE0
                && font.subtype != PDF_FONT_FONTTYPE_CIDTYPE2
            {
                font_id += 1;
                continue;
            }
            if streq(&font.filename, map_name)
                && font.cid.options.style == opt.style
                && font.index == fmap_opt.index
            {
                if font.cid.options.embed == opt.embed {
                    // Case 1: CSI not available (Identity CMap)
                    //         Font is TrueType --> continue
                    //         Font is CIDFont  --> break
                    // Case 2: CSI matched      --> break
                    if has_csi == 0 {
                        if font.subtype == PDF_FONT_FONTTYPE_CIDTYPE2 {
                            font_id += 1;
                            continue;
                        }
                        break;
                    } else if streq(
                        &font.cid.csi.registry,
                        opt.csi.registry.as_deref().unwrap_or(b""),
                    ) && streq(
                        &font.cid.csi.ordering,
                        opt.csi.ordering.as_deref().unwrap_or(b""),
                    ) {
                        if font.subtype == PDF_FONT_FONTTYPE_CIDTYPE2 {
                            // FIXME: font modified
                            font.cid.csi.supplement =
                                opt.csi.supplement.max(font.cid.csi.supplement);
                        }
                        break;
                    }
                } else if font.flags & PDF_FONT_FLAG_BASEFONT != 0 {
                    break;
                }
            }
            font_id += 1;
        }

        if font_id < count { font_id } else { -1 }
    }

    /// `pdf_font_open_cidfont`: 0, or -1 if no loader opened it.
    pub fn pdf_font_open_cidfont(
        &mut self,
        font_id: i32,
        map_name: &[u8],
        cmap_csi: Option<&CidSysInfo>,
        fmap_opt: &FontmapOpt,
    ) -> i32 {
        let (mut opt, _has_csi) = self.cid_make_opt(map_name, cmap_csi, fmap_opt);
        let index = fmap_opt.index as i32;

        if self.CIDFont_type0_open(font_id, map_name, index, &mut opt) < 0
            && self.CIDFont_type2_open(font_id, map_name, index, &mut opt) < 0
            && self.CIDFont_type0_open_from_t1(font_id, map_name, index, &mut opt) < 0
            && self.CIDFont_type0_open_from_t1c(font_id, map_name, index, &mut opt) < 0
            && self.CIDFont_base_open(font_id, map_name, &mut opt) < 0
        {
            return -1;
        }

        let font = &mut self.font.fonts[font_id as usize];
        font.filename = Some(map_name.to_vec());
        font.ident = Some(map_name.to_vec());
        font.index = fmap_opt.index;
        font.cid.options = opt;

        if font.cid.csi.registry.is_some() && font.cid.csi.ordering.is_some() {
            if let Some(cmap_csi) = cmap_csi {
                if font.cid.csi.registry != cmap_csi.registry
                    || font.cid.csi.ordering != cmap_csi.ordering
                {
                    warn!("Inconsistent ROS found:\n");
                    error!("Incompatible CMap specified for this font.");
                }
                if font.cid.csi.supplement < cmap_csi.supplement {
                    font.cid.csi.supplement = cmap_csi.supplement;
                }
            }
        } else {
            assert!(font.subtype == PDF_FONT_FONTTYPE_CIDTYPE2);
            if let Some(cmap_csi) = cmap_csi {
                font.cid.csi.registry = cmap_csi.registry.clone();
                font.cid.csi.ordering = cmap_csi.ordering.clone();
                font.cid.csi.supplement = cmap_csi.supplement;
            } else {
                // This means font's internal glyph ordering.
                font.cid.csi.registry = Some(b"Adobe".to_vec());
                font.cid.csi.ordering = Some(b"Identity".to_vec());
                font.cid.csi.supplement = 0;
            }
        }

        0
    }

    /// `get_cidsysinfo` (static): `has_csi` (0/1), filling `csi` from
    /// `fmap_opt.charcoll` (uses the output PDF version).
    fn get_cidsysinfo(
        &self,
        csi: &mut CidSysInfo,
        map_name: &[u8],
        fmap_opt: Option<&FontmapOpt>,
    ) -> i32 {
        let mut has_csi = 0;
        let mut csi_idx: i32 = -1;

        let mut sup_idx = self.o.get_version() - 10;
        sup_idx = if sup_idx > SUP_IDX_MAX {
            SUP_IDX_MAX
        } else {
            sup_idx
        };

        let Some(fmap_opt) = fmap_opt else {
            return 0;
        };
        let Some(charcoll) = fmap_opt.charcoll.as_deref() else {
            return 0;
        };

        // First try alias for standard one.
        for &(name, index) in CIDFONT_STDCC_ALIAS {
            let n = name.len();
            if charcoll.starts_with(name) {
                csi_idx = index as i32;
                let def = &CIDFONT_STDCC_DEF[index];
                csi.registry = Some(def.0.to_vec());
                csi.ordering = Some(def.1.to_vec());
                if charcoll.len() > n {
                    csi.supplement = crate::fmt::strtol(&charcoll[n..], 10).0 as u64 as i32;
                } else {
                    // Use heighest supported value for current output PDF
                    // version.
                    csi.supplement = def.2[sup_idx as usize];
                }
                has_csi = 1;
                break;
            }
        }
        if has_csi == 0 {
            let bad = || -> ! {
                error!(
                    "String can't be converted to REGISTRY-ORDERING-SUPPLEMENT: {}",
                    String::from_utf8_lossy(charcoll)
                )
            };
            // Full REGISTRY-ORDERING-SUPPLEMENT
            let Some(p0) = charcoll.iter().position(|&c| c == b'-') else {
                bad()
            };
            if p0 + 1 >= charcoll.len() {
                bad();
            }
            let p = p0 + 1;
            let Some(q0) = charcoll[p..].iter().position(|&c| c == b'-').map(|x| x + p) else {
                bad()
            };
            if q0 + 1 >= charcoll.len() {
                bad();
            }
            let q = q0 + 1;
            if !charcoll[q].is_ascii_digit() {
                bad();
            }

            csi.registry = Some(charcoll[..p0].to_vec());
            csi.ordering = Some(charcoll[p..q0].to_vec());
            csi.supplement = crate::fmt::strtol(&charcoll[q..], 10).0 as u64 as i32;

            has_csi = 1;

            // Check for standart character collections.
            for (i, def) in CIDFONT_STDCC_DEF.iter().enumerate() {
                if streq(&csi.registry, def.0) && streq(&csi.ordering, def.1) {
                    csi_idx = i as i32;
                    break;
                }
            }
        }

        if csi_idx >= 0
            && csi.supplement > CIDFONT_STDCC_DEF[csi_idx as usize].2[sup_idx as usize]
            && (fmap_opt.flags & FONTMAP_OPT_NOEMBED) != 0
        {
            warn!(
                "Heighest supplement number supported in PDF-{}.{} for {:?}-{:?}.",
                self.o.version_major, self.o.version_minor, csi.registry, csi.ordering
            );
            warn!(
                "Some character may not shown without embedded font (--> {:?}).",
                map_name
            );
        }

        has_csi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_font_lookup_names() {
        // the "-Acro" suffix may be left out
        let names: Vec<&[u8]> = CID_BASEFONT.iter().map(|b| b.0).collect();
        assert!(names.contains(&b"HeiseiMin-W3-Acro".as_slice()));
        assert_eq!(CID_BASEFONT.len(), 20);
    }
}
