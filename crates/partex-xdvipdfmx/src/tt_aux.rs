//! tt_aux.c, tt_aux.h: TrueType Collection offsets and the font
//! descriptor of an sfnt.

use crate::prelude::*;
use crate::sfnt::{Sfnt, ULONG};

/// Font descriptor `/Flags` bits (tt_aux.c).
pub const FIXEDWIDTH: i32 = 1 << 0;
pub const SERIF: i32 = 1 << 1;
pub const SYMBOLIC: i32 = 1 << 2;
pub const SCRIPT: i32 = 1 << 3;
pub const STANDARD: i32 = 1 << 5;
pub const ITALIC: i32 = 1 << 6;
pub const ALLCAP: i32 = 1 << 16;
pub const SMALLCAP: i32 = 1 << 17;
pub const FORCEBOLD: i32 = 1 << 18;

impl Sfnt {
    /// `ttc_read_offset`: the offset of the `ttc_idx`th font of a TTC.
    pub fn ttc_read_offset(&mut self, ttc_idx: ULONG) -> ULONG {
        todo!()
    }
}

impl Dpx {
    /// `tt_get_fontdesc`: the FontDescriptor dict (none without a `post`
    /// table); `embed` is in/out (cleared when the license forbids
    /// embedding, unless `ignore_font_license`). `type_` is C's `type`: 0
    /// for a CID-keyed font (adds /Style /Panose), 1 for a simple font.
    pub fn tt_get_fontdesc(
        &mut self,
        sfont: &mut Sfnt,
        embed: &mut i32,
        stemv: i32,
        type_: i32,
        fontname: &[u8],
    ) -> Option<Obj> {
        todo!()
    }
}
