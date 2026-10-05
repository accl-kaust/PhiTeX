//! cid.c, cid.h, cid_basefont.h: CIDFonts (dispatch to CIDFontType0/2
//! and the never-embedded base fonts), character collections.
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`.

#![allow(non_snake_case)]

use crate::fontmap::FontmapOpt;
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

impl Dpx {
    /// `CIDFont_set_flags`: ORs into `opt_flags_cidfont`.
    pub fn CIDFont_set_flags(&mut self, flags: i32) {
        todo!()
    }

    /// `CIDFont_get_usedchars`: the font's `usedchars` (8192 bytes,
    /// allocated zeroed on first use). Shared, as pdffont's `UsedChars`.
    pub fn CIDFont_get_usedchars(&mut self, font_id: i32) -> crate::pdffont::UsedChars {
        todo!()
    }

    /// `CIDFont_get_usedchars_v`: `cid.usedchars_v`, likewise.
    pub fn CIDFont_get_usedchars_v(&mut self, font_id: i32) -> crate::pdffont::UsedChars {
        todo!()
    }

    /// `CIDFont_is_ACCFont`: an Adobe CJK collection (GB1, CNS1, Japan1,
    /// Korea1).
    pub fn CIDFont_is_ACCFont(&self, font_id: i32) -> bool {
        todo!()
    }

    /// `CIDFont_is_UCSFont`: ordering UCS or UCS2.
    pub fn CIDFont_is_UCSFont(&self, font_id: i32) -> bool {
        todo!()
    }

    /// `source_font_type` (static): from `CIDFONT_FLAG_*`.
    fn source_font_type(&self, font_id: i32) -> i32 {
        todo!()
    }

    /// `pdf_font_load_cidfont`.
    pub fn pdf_font_load_cidfont(&mut self, font_id: i32) {
        todo!()
    }

    /// `CIDFont_base_open` (static): one of `CID_BASEFONT`, or -1.
    fn CIDFont_base_open(&mut self, font_id: i32, name: &[u8], opt: &mut CidOpt) -> i32 {
        todo!()
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
        todo!()
    }

    /// `pdf_font_open_cidfont`: 0, or -1 if no loader opened it.
    pub fn pdf_font_open_cidfont(
        &mut self,
        font_id: i32,
        map_name: &[u8],
        cmap_csi: Option<&CidSysInfo>,
        fmap_opt: &FontmapOpt,
    ) -> i32 {
        todo!()
    }

    /// `get_cidsysinfo` (static): `has_csi` (0/1), filling `csi` from
    /// `fmap_opt.charcoll` (uses the output PDF version).
    fn get_cidsysinfo(
        &self,
        csi: &mut CidSysInfo,
        map_name: &[u8],
        fmap_opt: Option<&FontmapOpt>,
    ) -> i32 {
        todo!()
    }
}
