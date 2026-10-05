//! `XeTeX`'s native fonts (`XeTeX` §744 and on): fonts loaded through
//! `partex-otf` rather than from a TFM file.

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;

/// `XeTeX` §744: `font_area` of a native font loaded through the
/// OpenType/Graphite layout (`otgr_font_flag`), not a string number.
pub(crate) const OTGR_FONT_FLAG: i32 = 0xFFFE;
/// `aat_font_flag` (macOS only: never made here).
pub(crate) const AAT_FONT_FLAG: i32 = 0xFFFF;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `is_native_font(f)`: whether font `f` was loaded as a native font.
    pub(crate) fn is_native_font(&self, f: i32) -> bool {
        self.unicode
            && usize::try_from(f)
                .ok()
                .and_then(|f| self.fonts.area.get(f))
                .is_some_and(|&a| a == OTGR_FONT_FLAG || a == AAT_FONT_FLAG)
    }
}
