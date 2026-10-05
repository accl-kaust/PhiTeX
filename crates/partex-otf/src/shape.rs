//! Shaping a run: HarfRust (HarfBuzz 14.5's port) with the font functions
//! XeTeX gives HarfBuzz (`XeTeXFontInst.cpp`), which answer from FreeType:
//! the character map FreeType selects ([`Face::char_index`]), a variation
//! sequence falling back to the base character's glyph, `FT_Get_Advance`'s
//! unscaled advances, and glyph extents from the outline's control box.
//! The font is at its units per em with no ppem (device tables unused),
//! so everything comes back in font units.
//!
//! [`shape`] is a pure function of the face's content and the request;
//! [`Shaper`] keeps HarfRust's plans and (switchable, bounded) a memo of
//! results by [`crate::face::FaceKey`].

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use harfrust::{Direction as HrDirection, GlyphId, ShaperFont};

use crate::Tag;
use crate::face::{Face, FaceKey};

/// The direction of a run (`hb_direction_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    TopToBottom,
}

/// A feature setting (`hb_feature_t`): `value` from `start` to `end`
/// (cluster indices; `u32::MAX` for the end of the text).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Feature {
    pub tag: Tag,
    pub value: u32,
    pub start: u32,
    pub end: u32,
}

/// What to shape: `text[start..start + len]` of a UTF-16 paragraph (the
/// rest is context, as `hb_buffer_add_utf16` takes it).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeRequest {
    /// The whole text, UTF-16.
    pub text: Vec<u16>,
    /// The run's first code unit.
    pub start: usize,
    /// The run's length in code units.
    pub len: usize,
    pub direction: Direction,
    /// The script as an ISO 15924 tag (`hb_script_t`, e.g. `Latn`), or
    /// `None` to guess it from the text.
    pub script: Option<Tag>,
    /// The language as a BCP 47 string (`hb_language_t`), or `None`.
    pub language: Option<String>,
    pub features: Vec<Feature>,
}

/// One shaped glyph (`hb_glyph_info_t` + `hb_glyph_position_t`), in font
/// units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ShapedGlyph {
    pub gid: u32,
    /// The UTF-16 index of the first code unit of the glyph's cluster, in
    /// the whole text.
    pub cluster: u32,
    pub x_advance: i32,
    pub y_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

/// The font functions XeTeX installs (`_get_font_funcs`).
struct FreeTypeFuncs<'a> {
    face: &'a Face,
}

impl harfrust::FontFuncs for FreeTypeFuncs<'_> {
    fn nominal_glyph(&self, _font: &ShaperFont, c: u32) -> Option<GlyphId> {
        let g = self.face.char_index(c);
        (g != 0).then(|| GlyphId::new(g))
    }

    fn variation_glyph(&self, _font: &ShaperFont, c: u32, vs: u32) -> Option<GlyphId> {
        // `_get_glyph`: the variant, else the base character's glyph.
        let mut g = if vs != 0 {
            self.face.char_variant_index(c, vs)
        } else {
            0
        };
        if g == 0 {
            g = self.face.char_index(c);
        }
        (g != 0).then(|| GlyphId::new(g))
    }

    fn glyph_h_advance(&self, _font: &ShaperFont, glyph: GlyphId) -> i32 {
        self.face.h_advance(glyph.to_u32()).unwrap_or(0) as i32
    }

    fn glyph_v_advance(&self, _font: &ShaperFont, glyph: GlyphId) -> i32 {
        // FreeType's vertical metrics grow downward.
        -(self.face.v_advance(glyph.to_u32()).unwrap_or(0) as i32)
    }

    fn glyph_v_origin(&self, _font: &ShaperFont, _glyph: GlyphId) -> (i32, i32) {
        // `_get_glyph_v_origin`: (0, 0), for pre-0.9999 compatibility.
        (0, 0)
    }

    fn glyph_extents(&self, _font: &ShaperFont, glyph: GlyphId) -> Option<harfrust::GlyphExtents> {
        let b = self.face.cbox(glyph.to_u32())?;
        Some(harfrust::GlyphExtents {
            x_bearing: b.x_min,
            y_bearing: b.y_max,
            width: b.x_max - b.x_min,
            height: -(b.y_max - b.y_min),
        })
    }
}

/// Decodes UTF-16 code unit `i` of `text` as `hb_utf16_t::next` does:
/// (code point, units used); a lone surrogate is U+FFFD.
fn utf16_next(text: &[u16], i: usize, end: usize) -> (u32, usize) {
    let c = u32::from(text[i]);
    if (0xD800..0xDC00).contains(&c) {
        if i + 1 < end {
            let l = u32::from(text[i + 1]);
            if (0xDC00..0xE000).contains(&l) {
                return (0x10000 + ((c - 0xD800) << 10) + (l - 0xDC00), 2);
            }
        }
        return (0xFFFD, 1);
    }
    if (0xDC00..0xE000).contains(&c) {
        return (0xFFFD, 1);
    }
    (c, 1)
}

/// `hb_utf16_t::prev`: the code point ending before `i` (> `start`).
fn utf16_prev(text: &[u16], i: usize, start: usize) -> (u32, usize) {
    let c = u32::from(text[i - 1]);
    if (0xDC00..0xE000).contains(&c) {
        if i - 1 > start {
            let h = u32::from(text[i - 2]);
            if (0xD800..0xDC00).contains(&h) {
                return (0x10000 + ((h - 0xD800) << 10) + (c - 0xDC00), 2);
            }
        }
        return (0xFFFD, 1);
    }
    if (0xD800..0xDC00).contains(&c) {
        return (0xFFFD, 1);
    }
    (c, 1)
}

const CONTEXT_LENGTH: usize = 5;

/// Fills `buffer` as `hb_buffer_add_utf16(buffer, text, len, start, len)`
/// does: up to five characters of context on each side, each character's
/// cluster its code unit index.
fn add_utf16(buffer: &mut harfrust::Buffer, text: &[u16], start: usize, len: usize) {
    let mut pre = Vec::new();
    let mut i = start;
    while i > 0 && pre.len() < CONTEXT_LENGTH {
        let (c, n) = utf16_prev(text, i, 0);
        pre.push(c);
        i -= n;
    }
    buffer.set_pre_context_codepoints(&pre);
    let end = start + len;
    let mut i = start;
    while i < end {
        let (c, n) = utf16_next(text, i, end);
        buffer.push(c, i as u32);
        i += n;
    }
    let mut post = Vec::new();
    let mut i = end;
    while i < text.len() && post.len() < CONTEXT_LENGTH {
        let (c, n) = utf16_next(text, i, text.len());
        post.push(c);
        i += n;
    }
    buffer.set_post_context_codepoints(&post);
}

fn hr_direction(d: Direction) -> HrDirection {
    match d {
        Direction::LeftToRight => HrDirection::LeftToRight,
        Direction::RightToLeft => HrDirection::RightToLeft,
        Direction::TopToBottom => HrDirection::TopToBottom,
    }
}

fn hr_features(fs: &[Feature]) -> Vec<harfrust::Feature> {
    fs.iter()
        .map(|f| {
            let end = if f.end == u32::MAX {
                usize::MAX
            } else {
                f.end as usize
            };
            harfrust::Feature::new(
                harfrust::Tag::from_u32(f.tag),
                f.value,
                f.start as usize..end,
            )
        })
        .collect()
}

/// Prepares the buffer and the plan's properties: (buffer, script,
/// language).
fn prepare(req: &ShapeRequest) -> harfrust::Buffer {
    let mut buffer = harfrust::Buffer::new();
    add_utf16(&mut buffer, &req.text, req.start, req.len);
    buffer.set_direction(hr_direction(req.direction));
    buffer.set_script(
        req.script
            .and_then(|s| harfrust::Script::from_iso15924_tag(harfrust::Tag::from_u32(s))),
    );
    buffer.set_language(req.language.as_deref().and_then(harfrust::Language::new));
    buffer.guess_segment_properties();
    buffer
}

fn collect(buffer: &harfrust::Buffer) -> Vec<ShapedGlyph> {
    buffer
        .glyph_infos()
        .iter()
        .zip(buffer.glyph_positions())
        .map(|(i, p)| ShapedGlyph {
            gid: i.glyph_id,
            cluster: i.cluster,
            x_advance: p.x_advance,
            y_advance: p.y_advance,
            x_offset: p.x_offset,
            y_offset: p.y_offset,
        })
        .collect()
}

/// Shapes `req` with `face` (`hb_shape_plan_execute` with the `ot` shaper,
/// XeTeX's font functions, scale = units per em, ppem 0).
#[must_use]
pub fn shape(face: &Face, req: &ShapeRequest) -> Vec<ShapedGlyph> {
    let mut buffer = prepare(req);
    let funcs = FreeTypeFuncs { face };
    let font = ShaperFont::new(face.font()).with_font_funcs(Some(&funcs));
    let features = hr_features(&req.features);
    let plan = harfrust::ShapePlan::new(
        face.font(),
        buffer.direction(),
        buffer.script(),
        buffer.language(),
        &features,
    );
    let options = harfrust::ShapeOptions::new()
        .plan(Some(&plan))
        .features(&features);
    if harfrust::shape(&font, &mut buffer, options).is_err() {
        return Vec::new();
    }
    collect(&buffer)
}

/// The plan's identity: what `hb_shape_plan_create_cached` keys on.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlanKey {
    face: FaceKey,
    direction: Direction,
    script: Option<Tag>,
    language: Option<String>,
    features: Vec<Feature>,
}

/// Shapes with HarfRust's plans kept per (face, properties, features),
/// and a memo of results that can be switched off ([`Shaper::set_memo`])
/// with the same output. Both are bounded: when full they are emptied.
pub struct Shaper {
    plans: BTreeMap<PlanKey, Arc<harfrust::ShapePlan>>,
    memo: BTreeMap<(FaceKey, ShapeRequest), Arc<[ShapedGlyph]>>,
    memo_on: bool,
    /// The memo's bound, in entries.
    pub memo_capacity: usize,
    /// The plans' bound, in entries.
    pub plan_capacity: usize,
}

impl Default for Shaper {
    fn default() -> Self {
        Shaper {
            plans: BTreeMap::new(),
            memo: BTreeMap::new(),
            memo_on: true,
            memo_capacity: 1 << 16,
            plan_capacity: 1 << 10,
        }
    }
}

impl Shaper {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Switches the result memo on or off (plans are always kept: they do
    /// not change results).
    pub fn set_memo(&mut self, on: bool) {
        self.memo_on = on;
        if !on {
            self.memo.clear();
        }
    }

    /// Forgets everything kept for faces other than those in `keep`
    /// (bounding memory when faces are released).
    pub fn retain_faces(&mut self, keep: &[FaceKey]) {
        self.plans.retain(|k, _| keep.contains(&k.face));
        self.memo.retain(|k, _| keep.contains(&k.0));
    }

    /// [`shape`], with the plan reused and the result memoized.
    pub fn shape(&mut self, face: &Face, req: &ShapeRequest) -> Arc<[ShapedGlyph]> {
        let key = (face.key(), req.clone());
        if self.memo_on
            && let Some(r) = self.memo.get(&key)
        {
            return r.clone();
        }
        let mut buffer = prepare(req);
        let features = hr_features(&req.features);
        let pk = PlanKey {
            face: face.key(),
            direction: req.direction,
            script: buffer
                .script()
                .map(|s| u32::from_be_bytes(s.tag().to_be_bytes())),
            language: buffer.language().map(|l| String::from(l.as_str())),
            features: req.features.clone(),
        };
        let plan = if let Some(p) = self.plans.get(&pk) {
            p.clone()
        } else {
            let p = Arc::new(harfrust::ShapePlan::new(
                face.font(),
                buffer.direction(),
                buffer.script(),
                buffer.language(),
                &features,
            ));
            if self.plans.len() >= self.plan_capacity {
                self.plans.clear();
            }
            self.plans.insert(pk, p.clone());
            p
        };
        let funcs = FreeTypeFuncs { face };
        let font = ShaperFont::new(face.font()).with_font_funcs(Some(&funcs));
        let options = harfrust::ShapeOptions::new()
            .plan(Some(&plan))
            .features(&features);
        let out: Arc<[ShapedGlyph]> = if harfrust::shape(&font, &mut buffer, options).is_ok() {
            collect(&buffer).into()
        } else {
            Arc::from(Vec::new())
        };
        if self.memo_on {
            if self.memo.len() >= self.memo_capacity {
                self.memo.clear();
            }
            self.memo.insert(key, out.clone());
        }
        out
    }
}
