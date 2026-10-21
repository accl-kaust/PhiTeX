// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Font.cpp, src/inc/Font.h,
// src/gr_font.cpp), LGPL-2.1-or-later.

//! A sized font (`gr_font`): the scale from design units, and the
//! application's (hinted) advances where it gives them.

use alloc::vec::Vec;

use crate::face::Face;

pub struct Font {
    pub(crate) scale: f32,
    /// Hinted advances per glyph (`glyph_advance_x`), if given.
    hinted: Option<Vec<f32>>,
    /// The design advances scaled (`Face::default_glyph_advance`).
    unhinted: Vec<f32>,
}

impl Font {
    /// `gr_make_font(ppm, face)`; `None` for a size that is not positive.
    #[must_use]
    pub fn new(ppm: f32, face: &Face) -> Option<Font> {
        Self::with_advances(ppm, face, None)
    }

    /// `gr_make_font_with_advance_fn`: `advance(gid)` the hinted advance
    /// of each glyph.
    #[must_use]
    pub fn with_advances(ppm: f32, face: &Face, advance: Option<&dyn Fn(u16) -> f32>) -> Option<Font> {
        if ppm <= 0.0 {
            return None;
        }
        let scale = ppm / f32::from(face.glyphs.units_per_em());
        let n = face.glyphs.num_glyphs();
        let hinted = advance.map(|f| (0..n).map(f).collect());
        let unhinted = (0..n).map(|g| face.glyphs.glyph(g).advance.x * scale).collect();
        Some(Font { scale, hinted, unhinted })
    }

    pub(crate) fn is_hinted(&self) -> bool {
        self.hinted.is_some()
    }

    /// `advance(glyphid)`.
    pub(crate) fn advance(&self, gid: u16) -> f32 {
        let g = usize::from(gid);
        match &self.hinted {
            Some(h) => h.get(g).copied().unwrap_or(0.0),
            None => self.unhinted.get(g).copied().unwrap_or(0.0),
        }
    }

    /// `scale()`.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }
}
