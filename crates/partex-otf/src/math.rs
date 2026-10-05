//! The `MATH` table as HarfBuzz reads it (`hb-ot-math.cc`) with the font at
//! its units per em and no ppem: every value in font units, device tables
//! unused.

use alloc::vec::Vec;

use crate::face::{Face, rd_i16, rd_u16};
use crate::tag;

/// `hb_ot_math_constant_t`'s last value
/// (`RADICAL_DEGREE_BOTTOM_RAISE_PERCENT`).
pub const LAST_CONSTANT: u32 = 55;

/// `hb_ot_math_kern_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernSide {
    TopRight = 0,
    TopLeft = 1,
    BottomRight = 2,
    BottomLeft = 3,
}

/// `hb_ot_math_glyph_part_t`, in font units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphPart {
    pub glyph: u32,
    pub start_connector_length: i32,
    pub end_connector_length: i32,
    pub full_advance: i32,
    /// `HB_OT_MATH_GLYPH_PART_FLAG_EXTENDER`.
    pub extender: bool,
}

/// A face's `MATH` table.
pub struct Math<'a> {
    d: &'a [u8],
    face: &'a Face,
}

/// Coverage lookup (formats 1 and 2), HarfBuzz's binary searches.
fn coverage(d: &[u8], o: usize, g: u32) -> Option<u32> {
    match rd_u16(d, o)? {
        1 => {
            let n = rd_u16(d, o + 2)? as i32;
            let (mut min, mut max) = (0i32, n - 1);
            while min <= max {
                let mid = ((min as u32 + max as u32) / 2) as i32;
                let v = u32::from(rd_u16(d, o + 4 + 2 * mid as usize)?);
                if g < v {
                    max = mid - 1;
                } else if g > v {
                    min = mid + 1;
                } else {
                    return Some(mid as u32);
                }
            }
            None
        }
        2 => {
            let n = rd_u16(d, o + 2)? as i32;
            let (mut min, mut max) = (0i32, n - 1);
            while min <= max {
                let mid = ((min as u32 + max as u32) / 2) as i32;
                let r = o + 4 + 6 * mid as usize;
                let (start, end) = (u32::from(rd_u16(d, r)?), u32::from(rd_u16(d, r + 2)?));
                if g < start {
                    max = mid - 1;
                } else if g > end {
                    min = mid + 1;
                } else {
                    return Some(u32::from(rd_u16(d, r + 4)?) + g - start);
                }
            }
            None
        }
        _ => None,
    }
}

impl<'a> Math<'a> {
    /// The face's `MATH` table, if it has one (`hb_ot_math_has_data`).
    #[must_use]
    pub fn new(face: &'a Face) -> Option<Self> {
        let d = face.table(tag(b"MATH"))?;
        if rd_u16(d, 0)? != 1 {
            return None;
        }
        Some(Math { d, face })
    }

    fn sub(&self, at: usize) -> Option<usize> {
        let o = rd_u16(self.d, at)? as usize;
        (o != 0).then_some(o)
    }

    /// `hb_ot_math_get_constant`.
    #[must_use]
    pub fn constant(&self, n: u32) -> i32 {
        let Some(c) = self.sub(4) else { return 0 };
        let d = self.d;
        match n {
            0 | 1 => rd_i16(d, c + 2 * n as usize).map_or(0, i32::from),
            2 | 3 => rd_u16(d, c + 2 * n as usize).map_or(0, i32::from),
            4..=54 => rd_i16(d, c + 8 + 4 * (n as usize - 4)).map_or(0, i32::from),
            55 => rd_i16(d, c + 8 + 4 * 51).map_or(0, i32::from),
            _ => 0,
        }
    }

    fn glyph_info(&self, which: usize) -> Option<usize> {
        let gi = self.sub(6)?;
        let o = rd_u16(self.d, gi + 2 * which)? as usize;
        (o != 0).then_some(gi + o)
    }

    /// `hb_ot_math_get_glyph_italics_correction`: 0 if not covered.
    #[must_use]
    pub fn italics_correction(&self, g: u32) -> i32 {
        self.value_table(0, g).unwrap_or(0)
    }

    /// The top accent attachment of `g`, or `None` if not covered (HarfBuzz
    /// then answers half the advance: [`Math::top_accent_attachment`]).
    #[must_use]
    pub fn top_accent(&self, g: u32) -> Option<i32> {
        self.value_table(1, g)
    }

    /// `hb_ot_math_get_glyph_top_accent_attachment`: the attachment, or the
    /// glyph's advance (XeTeX's FreeType advance) divided by 2.
    #[must_use]
    pub fn top_accent_attachment(&self, g: u32) -> i32 {
        self.top_accent(g)
            .unwrap_or_else(|| self.face.h_advance(g).unwrap_or(0) as i32 / 2)
    }

    fn value_table(&self, which: usize, g: u32) -> Option<i32> {
        let t = self.glyph_info(which)?;
        let cov = rd_u16(self.d, t)? as usize;
        if cov == 0 {
            return None;
        }
        let i = coverage(self.d, t + cov, g)?;
        let n = u32::from(rd_u16(self.d, t + 2)?);
        if i >= n {
            return None;
        }
        rd_i16(self.d, t + 4 + 4 * i as usize).map(i32::from)
    }

    /// `hb_ot_math_get_glyph_kerning`: the kern on `side` of `g` at
    /// `height` (font units).
    #[must_use]
    pub fn kerning(&self, g: u32, side: KernSide, height: i32) -> i32 {
        self.kerning_opt(g, side, height).unwrap_or(0)
    }

    fn kerning_opt(&self, g: u32, side: KernSide, height: i32) -> Option<i32> {
        let d = self.d;
        let t = self.glyph_info(3)?;
        let cov = rd_u16(d, t)? as usize;
        if cov == 0 {
            return None;
        }
        let i = coverage(d, t + cov, g)?;
        let n = u32::from(rd_u16(d, t + 2)?);
        if i >= n {
            return None;
        }
        let k = rd_u16(d, t + 4 + 8 * i as usize + 2 * side as usize)? as usize;
        if k == 0 {
            return None;
        }
        let k = t + k;
        let count = rd_u16(d, k)? as i32;
        // upper bound: the number of correction heights <= height.
        let (mut min, mut max) = (0i32, count - 1);
        let mut pos = None;
        while min <= max {
            let mid = ((min as u32 + max as u32) / 2) as i32;
            let v = i32::from(rd_i16(d, k + 2 + 4 * mid as usize)?);
            if height < v {
                max = mid - 1;
            } else if height > v {
                min = mid + 1;
            } else {
                pos = Some(mid);
                break;
            }
        }
        let idx = match pos {
            Some(p) => p + 1,
            None => min,
        } as usize;
        rd_i16(d, k + 2 + 4 * count as usize + 4 * idx).map(i32::from)
    }

    fn construction(&self, g: u32, horizontal: bool) -> Option<usize> {
        let d = self.d;
        let v = self.sub(8)?;
        let vcount = u32::from(rd_u16(d, v + 6)?);
        let hcount = u32::from(rd_u16(d, v + 8)?);
        let (cov, count) = if horizontal {
            (rd_u16(d, v + 4)?, hcount)
        } else {
            (rd_u16(d, v + 2)?, vcount)
        };
        if cov == 0 {
            return None;
        }
        let mut i = coverage(d, v + cov as usize, g)?;
        if i >= count {
            return None;
        }
        if horizontal {
            i += vcount;
        }
        let o = rd_u16(d, v + 10 + 2 * i as usize)? as usize;
        (o != 0).then_some(v + o)
    }

    /// `hb_ot_math_get_glyph_variants`: the variants of `g` (glyph, advance).
    #[must_use]
    pub fn variants(&self, g: u32, horizontal: bool) -> Vec<(u32, i32)> {
        let Some(c) = self.construction(g, horizontal) else {
            return Vec::new();
        };
        let n = rd_u16(self.d, c + 2).unwrap_or(0) as usize;
        (0..n)
            .filter_map(|i| {
                let r = c + 4 + 4 * i;
                Some((
                    u32::from(rd_u16(self.d, r)?),
                    i32::from(rd_u16(self.d, r + 2)?),
                ))
            })
            .collect()
    }

    /// `hb_ot_math_get_glyph_assembly`: the parts of `g`'s assembly (none
    /// without one) and its italics correction.
    #[must_use]
    pub fn assembly(&self, g: u32, horizontal: bool) -> (Vec<GlyphPart>, i32) {
        let Some(c) = self.construction(g, horizontal) else {
            return (Vec::new(), 0);
        };
        let d = self.d;
        let a = rd_u16(d, c).unwrap_or(0) as usize;
        if a == 0 {
            return (Vec::new(), 0);
        }
        let a = c + a;
        let ic = rd_i16(d, a).map_or(0, i32::from);
        let n = rd_u16(d, a + 4).unwrap_or(0) as usize;
        let parts = (0..n)
            .filter_map(|i| {
                let r = a + 6 + 10 * i;
                Some(GlyphPart {
                    glyph: u32::from(rd_u16(d, r)?),
                    start_connector_length: i32::from(rd_u16(d, r + 2)?),
                    end_connector_length: i32::from(rd_u16(d, r + 4)?),
                    full_advance: i32::from(rd_u16(d, r + 6)?),
                    extender: rd_u16(d, r + 8)? & 1 != 0,
                })
            })
            .collect();
        (parts, ic)
    }

    /// `hb_ot_math_get_min_connector_overlap`.
    #[must_use]
    pub fn min_connector_overlap(&self) -> i32 {
        self.sub(8)
            .and_then(|v| rd_u16(self.d, v))
            .map_or(0, i32::from)
    }
}
