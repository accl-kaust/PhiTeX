//! cidtype2.c, cidtype2.h: CIDFontType2 (TrueType CIDFonts).
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; a `CMap *`
//! from the cache is a cmap id (-1 for C's NULL), passed on as
//! `Option<&CMap>` (`CMap_cache_get`) to the decoding helpers.

#![allow(non_snake_case)]

use crate::cmap::{CMap, Cid};
use crate::pdffont::CidOpt;
use crate::prelude::*;
use crate::tt_glyf::TtGlyphs;

/// `required_table`: `(tag, must_exist)` for the embedded font.
pub const REQUIRED_TABLE: &[(&[u8; 4], bool)] = &[
    (b"OS/2", false),
    (b"head", true),
    (b"hhea", true),
    (b"loca", true),
    (b"maxp", true),
    (b"name", true),
    (b"glyf", true),
    (b"hmtx", true),
    (b"fpgm", false),
    (b"cvt ", false),
    (b"prep", false),
];

/// `validate_name`'s `badstrlist`.
pub const BADSTRLIST: &[&[u8]] = &[
    b"-WIN-RKSJ-H",
    b"-WINP-RKSJ-H",
    b"-WING-RKSJ-H",
    b"-90pv-RKSJ-H",
];

/// `WIN_UCS_INDEX_MAX`.
pub const WIN_UCS_INDEX_MAX: i32 = 1;
/// `KNOWN_ENCODINGS_MAX`.
pub const KNOWN_ENCODINGS_MAX: i32 = 10;

/// `known_encodings`: `(platform, encoding, pdfnames)` (TT_WIN = 3,
/// TT_MAC = 1; without C's NULL end).
pub const KNOWN_ENCODINGS: &[(u16, u16, &[&[u8]])] = &[
    (3, 10, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]), // TT_WIN, TT_WIN_UCS4
    (3, 1, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]),  // TT_WIN, TT_WIN_UNICODE
    (3, 2, &[b"90ms-RKSJ"]),                                    // TT_WIN, TT_WIN_SJIS
    (3, 3, &[b"GBK-EUC"]),                                      // TT_WIN, TT_WIN_RPC
    (3, 4, &[b"ETen-B5"]),                                      // TT_WIN, TT_WIN_BIG5
    (3, 5, &[b"KSCms-UHC"]),                                    // TT_WIN, TT_WIN_WANSUNG
    (1, 1, &[b"90pv-RKSJ"]),                                    // TT_MAC, TT_MAC_JAPANESE
    (1, 2, &[b"B5pc"]),      // TT_MAC, TT_MAC_TRADITIONAL_CHINESE
    (1, 25, &[b"GBpc-EUC"]), // TT_MAC, TT_MAC_SIMPLIFIED_CHINESE
    (1, 3, &[b"KSCpc-EUC"]), // TT_MAC, TT_MAC_KOREAN
    (0, 4, &[b"UCSms-UCS4", b"UCSms-UCS2", b"UCS4", b"UCS2"]),
];

/// `FIX_CJK_UNIOCDE_SYMBOLS`.
pub const FIX_CJK_UNIOCDE_SYMBOLS: i32 = 1;

/// `fix_CJK_symbols`'s `CJK_Uni_symbols`: `(alt1, alt2)` Microsoft/Apple
/// Unicode mapping differences (the last is C's EOD, kept: C loops over
/// all of them).
pub const CJK_UNI_SYMBOLS: &[(u16, u16)] = &[
    (0x2014, 0x2015),
    (0x2016, 0x2225),
    (0x203E, 0xFFE3),
    (0x2026, 0x22EF),
    (0x2212, 0xFF0D),
    (0x301C, 0xFF5E),
    (0xFFE0, 0x00A2),
    (0xFFE1, 0x00A3),
    (0xFFE2, 0x00AC),
    (0xFFE5, 0x00A5),
    (0xFFFF, 0xFFFF),
];

/// `validate_name` (static): removes NULs and a bad suffix from
/// `fontname[..len]` in place (truncating it); 0, or -1 if empty.
fn validate_name(fontname: &mut Vec<u8>, len: i32) -> i32 {
    todo!()
}

/// `fix_CJK_symbols` (static).
fn fix_CJK_symbols(code: u16) -> u16 {
    todo!()
}

impl Dpx {
    /// `find_tocode_cmap` (static): `<reg>-<ord>-<pdfname>` from
    /// `KNOWN_ENCODINGS[select]`; a cache id, or -1 (C's NULL).
    fn find_tocode_cmap(&mut self, reg: Option<&[u8]>, ord: Option<&[u8]>, select: i32) -> i32 {
        todo!()
    }

    /// `add_TTCIDHMetrics` (static): `/DW`, `/W`.
    fn add_TTCIDHMetrics(
        &mut self,
        fontdict: Obj,
        g: &TtGlyphs,
        used_chars: &[u8],
        cidtogidmap: Option<&[u8]>,
        last_cid: u16,
    ) {
        todo!()
    }

    /// `add_TTCIDVMetrics` (static): `/DW2`, `/W2`.
    fn add_TTCIDVMetrics(&mut self, fontdict: Obj, g: &TtGlyphs, used_chars: &[u8], last_cid: u16) {
        todo!()
    }

    /// `cid_to_code` (static): the code (or -1) and C's `*puvs` (a
    /// variation selector, or -1). `cmap` none returns `cid`.
    fn cid_to_code(&self, cmap: Option<&CMap>, cid: Cid, unicode_cmap: i32) -> (i32, i32) {
        todo!()
    }

    /// `cid_to_gid` (static): `cmap` none returns `cid`.
    fn cid_to_gid(&self, cmap: Option<&CMap>, cid: Cid) -> u16 {
        todo!()
    }

    /// `CIDFont_type2_dofont`.
    pub fn CIDFont_type2_dofont(&mut self, font_id: i32) -> i32 {
        todo!()
    }

    /// `CIDFont_type2_open`: 0, or -1 if `name` is not a TrueType font.
    pub fn CIDFont_type2_open(
        &mut self,
        font_id: i32,
        name: &[u8],
        index: i32,
        opt: &mut CidOpt,
    ) -> i32 {
        todo!()
    }
}
