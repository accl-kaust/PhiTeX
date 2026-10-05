//! type0.c, type0.h: Type0 (composite) fonts.
//!
//! `pdf_font *` is a `font_id: i32` into `self.font.fonts`; `cid_id` is
//! the descendant CIDFont's font id.

#![allow(non_snake_case)]

use crate::prelude::*;

/// `CMAP_PART0` (`create_dummy_CMap`'s Adobe-Identity-UCS2 header).
pub const CMAP_PART0: &[u8] = b"%!PS-Adobe-3.0 Resource-CMap\n\
%%DocumentNeededResources: ProcSet (CIDInit)\n\
%%IncludeResource: ProcSet (CIDInit)\n\
%%BeginResource: CMap (Adobe-Identity-UCS2)\n\
%%Title: (Adobe-Identity-UCS2 Adobe UCS2 0)\n\
%%Version: 1.0\n\
%%Copyright:\n\
%% ---\n\
%%EndComments\n\n";

/// `CMAP_PART1`.
pub const CMAP_PART1: &[u8] = b"/CIDInit /ProcSet findresource begin\n\
\n\
12 dict begin\n\nbegincmap\n\n\
/CIDSystemInfo 3 dict dup begin\n  \
/Registry (Adobe) def\n  \
/Ordering (UCS2) def\n  \
/Supplement 0 def\n\
end def\n\n\
/CMapName /Adobe-Identity-UCS2 def\n\
/CMapVersion 1.0 def\n\
/CMapType 2 def\n\n\
2 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n";

/// `CMAP_PART3`.
pub const CMAP_PART3: &[u8] = b"endcmap\n\n\
CMapName currentdict /CMap defineresource pop\n\n\
end\nend\n\n\
%%EndResource\n\
%%EOF\n";

impl Dpx {
    /// `try_load_ToUnicode_file` (static): `<cmap_base>-UTF16`, then
    /// `-UCS2`; a reference.
    fn try_load_ToUnicode_file(&mut self, cmap_base: &[u8]) -> Option<Obj> {
        todo!()
    }

    /// `Type0Font_attach_ToUnicode_stream` (static).
    fn Type0Font_attach_ToUnicode_stream(&mut self, font_id: i32) {
        todo!()
    }

    /// `pdf_font_load_type0`.
    pub fn pdf_font_load_type0(&mut self, font_id: i32) {
        todo!()
    }

    /// `pdf_font_open_type0`: 0, or -1 when `cid_id < 0`.
    pub fn pdf_font_open_type0(&mut self, font_id: i32, cid_id: i32, wmode: i32) -> i32 {
        todo!()
    }

    /// `create_dummy_CMap` (static): the Adobe-Identity-UCS2 stream.
    fn create_dummy_CMap(&mut self) -> Obj {
        todo!()
    }

    /// `pdf_read_ToUnicode_file` (static): a reference to the "CMap"
    /// resource (defined `PDF_RES_FLUSH_IMMEDIATE`), or none.
    fn pdf_read_ToUnicode_file(&mut self, cmap_name: &[u8]) -> Option<Obj> {
        todo!()
    }
}
