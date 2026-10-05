//! OpenType layout queries as HarfBuzz answers them (`hb-ot-layout.cc`):
//! script, language and feature tags of `GSUB`/`GPOS`, and the `size`
//! feature's parameters.

use alloc::vec::Vec;

use crate::face::{Face, rd_u16, rd_u32};
use crate::{Tag, tag};

/// `HB_OT_TAG_GSUB` or `HB_OT_TAG_GPOS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    Gsub,
    Gpos,
}

/// `HB_OT_LAYOUT_DEFAULT_LANGUAGE_INDEX` / `HB_OT_LAYOUT_NO_SCRIPT_INDEX`.
pub const NOT_FOUND: u32 = 0xFFFF;

/// A `GSUB` or `GPOS` table's script and feature lists.
struct Gsubgpos<'a> {
    d: &'a [u8],
    scripts: usize,
    features: usize,
}

impl<'a> Gsubgpos<'a> {
    fn new(face: &'a Face, t: Table) -> Option<Self> {
        Self::from_bytes(face.table(match t {
            Table::Gsub => tag(b"GSUB"),
            Table::Gpos => tag(b"GPOS"),
        })?)
    }

    fn from_bytes(d: &'a [u8]) -> Option<Self> {
        if rd_u16(d, 0)? != 1 {
            return None;
        }
        Some(Gsubgpos {
            d,
            scripts: rd_u16(d, 4)? as usize,
            features: rd_u16(d, 6)? as usize,
        })
    }

    fn script_count(&self) -> u32 {
        if self.scripts == 0 {
            return 0;
        }
        rd_u16(self.d, self.scripts).map_or(0, u32::from)
    }

    fn script_tag(&self, i: u32) -> Tag {
        rd_u32(self.d, self.scripts + 2 + 6 * i as usize).unwrap_or(0)
    }

    /// The Script table of record `i`, or `None` (HarfBuzz's Null Script).
    fn script(&self, i: u32) -> Option<usize> {
        if i >= self.script_count() {
            return None;
        }
        let off = rd_u16(self.d, self.scripts + 2 + 6 * i as usize + 4)? as usize;
        (off != 0).then_some(self.scripts + off)
    }

    fn feature_tag(&self, i: u32) -> Tag {
        if self.features == 0 || i == NOT_FOUND {
            return 0;
        }
        let n = rd_u16(self.d, self.features).map_or(0, u32::from);
        if i >= n {
            return 0;
        }
        rd_u32(self.d, self.features + 2 + 6 * i as usize).unwrap_or(0)
    }

    fn feature_count(&self) -> u32 {
        if self.features == 0 {
            return 0;
        }
        rd_u16(self.d, self.features).map_or(0, u32::from)
    }
}

/// HarfBuzz's `bfind` over `n` records of `stride` bytes at `base` whose
/// first four bytes are the tag: (found, index or insertion point).
fn bfind(d: &[u8], base: usize, n: u32, stride: usize, key: Tag) -> (bool, u32) {
    let (mut min, mut max) = (0i32, n as i32 - 1);
    while min <= max {
        let mid = u32::midpoint(min as u32, max as u32) as i32;
        let t = rd_u32(d, base + mid as usize * stride).unwrap_or(0);
        match key.cmp(&t) {
            core::cmp::Ordering::Less => max = mid - 1,
            core::cmp::Ordering::Greater => min = mid + 1,
            core::cmp::Ordering::Equal => return (true, mid as u32),
        }
    }
    (false, min as u32)
}

/// `hb_ot_layout_table_get_script_tags`.
#[must_use]
pub fn script_tags(face: &Face, t: Table) -> Vec<Tag> {
    let Some(g) = Gsubgpos::new(face, t) else {
        return Vec::new();
    };
    (0..g.script_count()).map(|i| g.script_tag(i)).collect()
}

/// `hb_ot_layout_script_get_language_tags`: the LangSys records' tags of
/// script `script_index` (none for an index out of range).
#[must_use]
pub fn language_tags(face: &Face, t: Table, script_index: u32) -> Vec<Tag> {
    let Some(g) = Gsubgpos::new(face, t) else {
        return Vec::new();
    };
    let Some(s) = g.script(script_index) else {
        return Vec::new();
    };
    let n = rd_u16(g.d, s + 2).unwrap_or(0) as usize;
    (0..n)
        .map(|i| rd_u32(g.d, s + 4 + 6 * i).unwrap_or(0))
        .collect()
}

/// `hb_ot_layout_table_find_script`: `(true, index)` for the tag itself;
/// otherwise `(false, …)` with `DFLT`'s, `dflt`'s or `latn`'s index, or
/// [`NOT_FOUND`].
#[must_use]
pub fn find_script(face: &Face, t: Table, script: Tag) -> (bool, u32) {
    let Some(g) = Gsubgpos::new(face, t) else {
        return (false, NOT_FOUND);
    };
    let n = g.script_count();
    let find = |k: Tag| bfind(g.d, g.scripts + 2, n, 6, k);
    if let (true, i) = find(script) {
        return (true, i);
    }
    for k in [tag(b"DFLT"), tag(b"dflt"), tag(b"latn")] {
        if let (true, i) = find(k) {
            return (false, i);
        }
    }
    (false, NOT_FOUND)
}

/// `hb_ot_layout_script_select_language` with one tag
/// (`hb_ot_layout_script_find_language`): `(true, index)`, else
/// `(false, dflt's index or NOT_FOUND)`.
#[must_use]
pub fn find_language(face: &Face, t: Table, script_index: u32, language: Tag) -> (bool, u32) {
    let Some(g) = Gsubgpos::new(face, t) else {
        return (false, NOT_FOUND);
    };
    let Some(s) = g.script(script_index) else {
        return (false, NOT_FOUND);
    };
    let n = u32::from(rd_u16(g.d, s + 2).unwrap_or(0));
    if let (true, i) = bfind(g.d, s + 4, n, 6, language) {
        return (true, i);
    }
    if let (true, i) = bfind(g.d, s + 4, n, 6, tag(b"dflt")) {
        return (false, i);
    }
    (false, NOT_FOUND)
}

/// `hb_ot_layout_language_get_feature_tags`: the tags of the features of
/// LangSys `lang_index` ([`NOT_FOUND`] for the default one) of script
/// `script_index`, required feature excluded.
#[must_use]
pub fn feature_tags(face: &Face, t: Table, script_index: u32, lang_index: u32) -> Vec<Tag> {
    let Some(g) = Gsubgpos::new(face, t) else {
        return Vec::new();
    };
    let Some(s) = g.script(script_index) else {
        return Vec::new();
    };
    let ls = if lang_index == NOT_FOUND {
        rd_u16(g.d, s).unwrap_or(0) as usize
    } else {
        let n = u32::from(rd_u16(g.d, s + 2).unwrap_or(0));
        if lang_index >= n {
            0
        } else {
            rd_u16(g.d, s + 4 + 6 * lang_index as usize + 4).unwrap_or(0) as usize
        }
    };
    if ls == 0 {
        return Vec::new();
    }
    let ls = s + ls;
    let n = rd_u16(g.d, ls + 4).unwrap_or(0) as usize;
    (0..n)
        .map(|i| g.feature_tag(u32::from(rd_u16(g.d, ls + 6 + 2 * i).unwrap_or(0xFFFF))))
        .collect()
}

/// The `size` feature's parameters (`hb_ot_layout_get_size_params`),
/// in decipoints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeParams {
    pub design_size: u16,
    pub subfamily_id: u16,
    pub subfamily_name_id: u16,
    pub range_start: u16,
    pub range_end: u16,
}

/// `FeatureParamsSize::sanitize`.
fn size_params_at(d: &[u8], o: usize) -> Option<SizeParams> {
    let p = SizeParams {
        design_size: rd_u16(d, o)?,
        subfamily_id: rd_u16(d, o + 2)?,
        subfamily_name_id: rd_u16(d, o + 4)?,
        range_start: rd_u16(d, o + 6)?,
        range_end: rd_u16(d, o + 8)?,
    };
    if p.design_size == 0 {
        None
    } else if p.subfamily_id == 0
        && p.subfamily_name_id == 0
        && p.range_start == 0
        && p.range_end == 0
    {
        Some(p)
    } else if p.design_size < p.range_start
        || p.design_size > p.range_end
        || p.subfamily_name_id < 256
        || p.subfamily_name_id > 32767
    {
        None
    } else {
        Some(p)
    }
}

/// `hb_ot_layout_get_size_params`: the first `size` feature of `GPOS` with
/// a design size, its parameters found relative to the Feature table or,
/// failing that, to the FeatureList (old Adobe tools' offsets).
#[must_use]
pub fn size_params(face: &Face) -> Option<SizeParams> {
    size_params_in(face.table(tag(b"GPOS"))?)
}

/// [`size_params`] of the `GPOS` table `gpos`.
#[must_use]
pub fn size_params_in(gpos: &[u8]) -> Option<SizeParams> {
    let g = Gsubgpos::from_bytes(gpos)?;
    if g.features == 0 {
        return None;
    }
    let size = tag(b"size");
    for i in 0..g.feature_count() {
        if g.feature_tag(i) != size {
            continue;
        }
        let Some(foff) = rd_u16(g.d, g.features + 2 + 6 * i as usize + 4) else {
            continue;
        };
        let feature = g.features + foff as usize;
        let Some(params) = rd_u16(g.d, feature) else {
            continue;
        };
        if params == 0 {
            continue;
        }
        let p = size_params_at(g.d, feature + params as usize).or_else(|| {
            let rel = (params as usize).checked_sub(feature - g.features)?;
            (rel <= 0xFFFF && rel != 0)
                .then(|| size_params_at(g.d, feature + rel))
                .flatten()
        });
        if let Some(p) = p {
            return Some(p);
        }
    }
    None
}
