// Copyright 2012, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/GlyphCache.cpp, src/GlyphFace.cpp,
// src/inc/GlyphCache.h, src/inc/GlyphFace.h), LGPL-2.1-or-later.

//! The glyphs: each one's box, advance and attributes (`Glat`/`Gloc`),
//! and the collision octaboxes (`Glat` version 3), loaded as
//! `gr_face_preloadGlyphs` loads them (every glyph at once, the boxes
//! only if some glyph has sub-boxes).

use alloc::vec::Vec;

use crate::be::{u16_at, u32_at, u8_at};
use crate::position::{Position, Rect};
use crate::sparse::Sparse;
use crate::ttf;

/// `GlyphFace`.
#[derive(Clone, Debug, Default)]
pub(crate) struct GlyphFace {
    pub bbox: Rect,
    pub advance: Position,
    pub attrs: Sparse,
}

/// `enum metrics`.
pub(crate) mod metric {
    pub const LSB: u8 = 0;
    pub const RSB: u8 = 1;
    pub const BB_TOP: u8 = 2;
    pub const BB_BOTTOM: u8 = 3;
    pub const BB_LEFT: u8 = 4;
    pub const BB_RIGHT: u8 = 5;
    pub const BB_HEIGHT: u8 = 6;
    pub const BB_WIDTH: u8 = 7;
    pub const ADV_WIDTH: u8 = 8;
    pub const ADV_HEIGHT: u8 = 9;
    pub const ASCENT: u8 = 10;
    pub const DESCENT: u8 = 11;
}

impl GlyphFace {
    /// `getMetric`.
    pub(crate) fn metric(&self, m: u8) -> i32 {
        let b = &self.bbox;
        match m {
            metric::LSB | metric::BB_LEFT => b.bl.x as i32,
            metric::RSB => (self.advance.x - b.tr.x) as i32,
            metric::BB_TOP => b.tr.y as i32,
            metric::BB_BOTTOM => b.bl.y as i32,
            metric::BB_RIGHT => b.tr.x as i32,
            metric::BB_HEIGHT => (b.tr.y - b.bl.y) as i32,
            metric::BB_WIDTH => (b.tr.x - b.bl.x) as i32,
            metric::ADV_WIDTH => self.advance.x as i32,
            metric::ADV_HEIGHT => self.advance.y as i32,
            _ => 0,
        }
    }
}

/// `GlyphBox`: the slanted box and the sub-boxes (each a box and its
/// slanted box).
#[derive(Clone, Debug, Default)]
pub(crate) struct GlyphBox {
    pub slant: Rect,
    pub subs: Vec<Rect>,
}

/// `GlyphCache`.
pub(crate) struct GlyphCache {
    glyphs: Vec<GlyphFace>,
    /// `_boxes`: present (`hasBoxes`) when `Glat` is version 3; each
    /// glyph's box where one was read.
    boxes: Option<Vec<Option<GlyphBox>>>,
    num_glyphs: u16,
    num_attrs: u16,
    upem: u16,
}

const EMPTY_RECT: Rect = Rect { bl: Position { x: 0.0, y: 0.0 }, tr: Position { x: 0.0, y: 0.0 } };

/// The tables `GlyphCache::Loader` reads.
pub(crate) struct GlyphTables<'a> {
    pub head: Option<&'a [u8]>,
    pub hhea: Option<&'a [u8]>,
    pub hmtx: Option<&'a [u8]>,
    pub glyf: Option<&'a [u8]>,
    pub loca: Option<&'a [u8]>,
    pub maxp: Option<&'a [u8]>,
    pub glat: Option<&'a [u8]>,
    pub gloc: Option<&'a [u8]>,
}

struct Loader<'a> {
    head: &'a [u8],
    hhea: &'a [u8],
    hmtx: &'a [u8],
    glyf: Option<&'a [u8]>,
    loca: &'a [u8],
    glat: &'a [u8],
    gloc: &'a [u8],
    long_fmt: bool,
    has_boxes: bool,
    num_glyphs_graphics: u16,
    num_glyphs_attributes: u16,
    num_attrs: u16,
}

impl<'a> Loader<'a> {
    /// `Loader(face)`; `None` where it is not usable (`operator bool`).
    fn new(t: &GlyphTables<'a>) -> Option<Loader<'a>> {
        let (head, hhea, hmtx) = (t.head?, t.hhea?, t.hmtx?);
        if t.glyf.is_some() != t.loca.is_some() {
            return None;
        }
        let maxp = t.maxp?;
        let num_glyphs_graphics = ttf::glyph_count(maxp) as u16;
        let loca = t.loca.unwrap_or(&[]);
        if t.glyf.is_some()
            && ttf::loca_lookup(num_glyphs_graphics.wrapping_sub(1), loca, head) == usize::MAX - 1
        {
            return None;
        }
        let glat = t.glat?;
        let gloc = t.gloc?;
        if gloc.len() < 8 {
            return None;
        }
        let version = u32_at(gloc, 0);
        let flags = u16_at(gloc, 4);
        let num_attrs = u16_at(gloc, 6);
        let long_fmt = flags & 1 != 0;
        let ids = if flags & 2 != 0 { 2 * usize::from(num_attrs) } else { 0 };
        let tmp = (gloc.len().wrapping_sub(8).wrapping_sub(ids) / if long_fmt { 4 } else { 2 }) as isize - 1;
        if version >= 0x0002_0000
            || tmp < 0
            || tmp > 65535
            || num_attrs == 0
            || num_attrs > 0x3000
            || num_glyphs_graphics as isize > tmp
            || glat.len() < 4
        {
            return None;
        }
        let gversion = u32_at(glat, 0);
        if gversion >= 0x0004_0000 || (gversion >= 0x0003_0000 && glat.len() < 8) {
            return None;
        }
        // (graphite2 forces the boxes on for any version 3 table)
        let has_boxes = gversion >= 0x0003_0000;
        Some(Loader {
            head,
            hhea,
            hmtx,
            glyf: t.glyf,
            loca,
            glat,
            gloc,
            long_fmt,
            has_boxes,
            num_glyphs_graphics,
            num_glyphs_attributes: tmp as u16,
            num_attrs,
        })
    }

    fn num_glyphs(&self) -> u16 {
        self.num_glyphs_graphics.max(self.num_glyphs_attributes)
    }

    fn gloc_range(&self, gid: u16) -> (usize, usize) {
        let g = usize::from(gid);
        if self.long_fmt {
            (u32_at(self.gloc, 8 + 4 * g) as usize, u32_at(self.gloc, 8 + 4 * g + 4) as usize)
        } else {
            (usize::from(u16_at(self.gloc, 8 + 2 * g)), usize::from(u16_at(self.gloc, 8 + 2 * g + 2)))
        }
    }

    /// `read_glyph`.
    fn read_glyph(&self, gid: u16, numsubs: &mut i32) -> Option<GlyphFace> {
        let mut glyph = GlyphFace::default();
        if gid < self.num_glyphs_graphics {
            if let Some(glyf) = self.glyf {
                let locidx = ttf::loca_lookup(gid, self.loca, self.head);
                if let Some((x0, y0, x1, y1)) = ttf::glyf_box(glyf, locidx) {
                    if x0 > x1 || y0 > y1 {
                        return None;
                    }
                    glyph.bbox = Rect::new(
                        Position::new(x0 as f32, y0 as f32),
                        Position::new(x1 as f32, y1 as f32),
                    );
                }
            }
            if let Some((_, adv)) = ttf::hor_metrics(gid, self.hmtx, self.hhea) {
                glyph.advance = Position::new(adv as f32, 0.0);
            }
        }
        if gid < self.num_glyphs_attributes {
            let g = usize::from(gid);
            if 8 + g * if self.long_fmt { 4 } else { 2 } > self.gloc.len() {
                return None;
            }
            let (mut glocs, gloce) = self.gloc_range(gid);
            let glat = self.glat;
            if glocs >= glat.len().wrapping_sub(1) || gloce > glat.len() {
                return None;
            }
            let glat_version = u32_at(glat, 0);
            if glat_version >= 0x0003_0000 {
                if glocs >= gloce {
                    return None;
                }
                let bmap = u16_at(glat, glocs);
                let num = bmap.count_ones() as usize;
                *numsubs += num as i32;
                glocs += 6 + 8 * num;
                if glocs > gloce {
                    return None;
                }
            }
            let na = usize::from(self.num_attrs);
            let pairs = if glat_version < 0x0002_0000 {
                if gloce.wrapping_sub(glocs) < 4 || gloce.wrapping_sub(glocs) > na * 4 {
                    return None;
                }
                glat_pairs(glat, glocs, gloce, 1)
            } else {
                if gloce.wrapping_sub(glocs) < 6
                    || gloce.wrapping_sub(glocs) > na * 6
                    || glocs > glat.len().wrapping_sub(4)
                {
                    return None;
                }
                glat_pairs(glat, glocs, gloce, 2)
            };
            glyph.attrs = Sparse::new(pairs.iter().copied());
            if !glyph.attrs.is_valid() || glyph.attrs.capacity() > na {
                return None;
            }
        }
        Some(glyph)
    }

    /// `read_box`: the box, and whether it was read whole (a box whose
    /// sub-boxes do not fit keeps its slanted box: the C++ made it before
    /// failing).
    fn read_box(&self, gid: u16, glyph: &GlyphFace) -> (GlyphBox, bool) {
        let mut out = GlyphBox::default();
        if gid >= self.num_glyphs_attributes {
            return (out, false);
        }
        let (glocs, gloce) = self.gloc_range(gid);
        let glat = self.glat;
        if gloce > glat.len() || glocs + 6 >= gloce {
            return (out, false);
        }
        let mut p = glocs;
        let bmap = u16_at(glat, p);
        p += 2;
        let num = bmap.count_ones() as usize;
        let bbox = glyph.bbox;
        let diamax = Rect::new(
            Position::new(bbox.bl.x + bbox.bl.y, bbox.bl.x - bbox.tr.y),
            Position::new(bbox.tr.x + bbox.tr.y, bbox.tr.x - bbox.bl.y),
        );
        let rd = |b: &Rect, p: usize| {
            readbox(b, u8_at(glat, p), u8_at(glat, p + 2), u8_at(glat, p + 1), u8_at(glat, p + 3))
        };
        out.slant = rd(&diamax, p);
        p += 4;
        out.subs = alloc::vec![EMPTY_RECT; 2 * num];
        if glocs + 6 + num * 8 >= gloce {
            return (out, false);
        }
        for i in 0..num * 2 {
            out.subs[i] = rd(if i & 1 != 0 { &diamax } else { &bbox }, p);
            p += 4;
        }
        (out, true)
    }
}

/// The (key, value) pairs of a glyph's `Glat` entries (`_glat_iterator`,
/// `W` bytes a key and a count): from `start` while the next value's
/// offset is before `end - 1`.
fn glat_pairs(glat: &[u8], start: usize, end: usize, w: usize) -> Vec<(u16, u16)> {
    let peek_w = |o: usize| if w == 1 { u32::from(u8_at(glat, o)) } else { u32::from(u16_at(glat, o)) };
    let mut out = Vec::new();
    let mut e = start;
    let mut v = start + 2 * w;
    let mut n: u32 = 0;
    while v < end.wrapping_sub(1) {
        out.push(((peek_w(e) + n) as u16, u16_at(glat, v)));
        n += 1;
        v += 2;
        if n == peek_w(e + w) {
            n = 0;
            e = v;
            v += 2 * w;
        }
    }
    out
}

fn scale_to(t: u8, zmin: f32, zmax: f32) -> f32 {
    zmin + f32::from(t) * (zmax - zmin) / 255.0
}

fn readbox(b: &Rect, zxmin: u8, zymin: u8, zxmax: u8, zymax: u8) -> Rect {
    Rect::new(
        Position::new(scale_to(zxmin, b.bl.x, b.tr.x), scale_to(zymin, b.bl.y, b.tr.y)),
        Position::new(scale_to(zxmax, b.bl.x, b.tr.x), scale_to(zymax, b.bl.y, b.tr.y)),
    )
}

impl GlyphCache {
    /// `GlyphCache(face, gr_face_preloadGlyphs)`.
    pub(crate) fn new(t: &GlyphTables) -> GlyphCache {
        let empty = GlyphCache { glyphs: Vec::new(), boxes: None, num_glyphs: 0, num_attrs: 0, upem: 0 };
        let Some(ld) = Loader::new(t) else { return empty };
        let n = ld.num_glyphs();
        if n == 0 {
            return empty;
        }
        let mut numsubs = 0i32;
        let mut glyphs = Vec::with_capacity(usize::from(n));
        for gid in 0..n {
            match ld.read_glyph(gid, &mut numsubs) {
                Some(g) => glyphs.push(g),
                // (glyph 0 dropped: the cache is emptied)
                None => return empty,
            }
        }
        let mut boxes = None;
        if ld.has_boxes {
            let mut bx: Vec<Option<GlyphBox>> = alloc::vec![None; usize::from(n)];
            if numsubs > 0 {
                for gid in 0..n {
                    let (b, ok) = ld.read_box(gid, &glyphs[usize::from(gid)]);
                    bx[usize::from(gid)] = Some(b);
                    if !ok {
                        // (the block is freed and glyph 0's box dropped;
                        // the others keep what was read)
                        bx[0] = None;
                        break;
                    }
                }
            }
            boxes = Some(bx);
        }
        GlyphCache {
            glyphs,
            boxes,
            num_glyphs: n,
            num_attrs: ld.num_attrs,
            upem: ttf::design_units(ld.head) as u16,
        }
    }

    pub(crate) fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    pub(crate) fn num_attrs(&self) -> u16 {
        self.num_attrs
    }

    pub(crate) fn units_per_em(&self) -> u16 {
        self.upem
    }

    /// `glyph(gid)` (glyph 0 for an id past the end).
    pub(crate) fn glyph(&self, gid: u16) -> &GlyphFace {
        self.glyphs.get(usize::from(gid)).unwrap_or(&self.glyphs[0])
    }

    /// `glyphSafe`.
    pub(crate) fn glyph_safe(&self, gid: u16) -> Option<&GlyphFace> {
        self.glyphs.get(usize::from(gid))
    }

    pub(crate) fn has_boxes(&self) -> bool {
        self.boxes.is_some()
    }

    /// `check`.
    pub(crate) fn check(&self, gid: u16) -> bool {
        self.boxes.is_some() && gid < self.num_glyphs
    }

    fn gbox(&self, gid: u16) -> Option<&GlyphBox> {
        self.boxes.as_ref()?.get(usize::from(gid))?.as_ref()
    }

    /// `slant` / `getBoundingSlantBox` (zero without a box).
    pub(crate) fn slant(&self, gid: u16) -> Rect {
        self.gbox(gid).map_or(EMPTY_RECT, |b| b.slant)
    }

    /// `getBoundingBBox`.
    pub(crate) fn bbox(&self, gid: u16) -> Rect {
        self.glyph(gid).bbox
    }

    /// `numSubBounds`.
    pub(crate) fn num_sub_bounds(&self, gid: u16) -> u8 {
        self.gbox(gid).map_or(0, |b| (b.subs.len() / 2) as u8)
    }

    /// `getSubBoundingBBox`.
    pub(crate) fn sub_bbox(&self, gid: u16, i: u8) -> Rect {
        self.gbox(gid).map_or(EMPTY_RECT, |b| b.subs[2 * usize::from(i)])
    }

    /// `getSubBoundingSlantBox`.
    pub(crate) fn sub_slant(&self, gid: u16, i: u8) -> Rect {
        self.gbox(gid).map_or(EMPTY_RECT, |b| b.subs[2 * usize::from(i) + 1])
    }
}
