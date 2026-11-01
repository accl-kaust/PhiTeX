//! A small font for tests: every character is described by hand.

use std::collections::HashMap;

use crate::arith::Scaled;
use crate::fonts::{CharInfo, FontId, Fonts, Left, LigKern};

#[derive(Default)]
pub struct TestFonts {
    pub chars: HashMap<(FontId, u16), CharInfo>,
    pub ligkern: HashMap<(FontId, Left, u16), LigKern>,
    pub bchar: HashMap<FontId, u16>,
    pub params: HashMap<(FontId, u16), Scaled>,
}

impl TestFonts {
    pub fn with_char(mut self, f: FontId, c: u16, w: Scaled, h: Scaled, d: Scaled) -> Self {
        self.chars.insert(
            (f, c),
            CharInfo {
                width: w,
                height: h,
                depth: d,
                ..CharInfo::default()
            },
        );
        self
    }
}

impl Fonts for TestFonts {
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        self.chars.get(&(font, ch)).copied()
    }
    fn lig_kern(&self, font: FontId, left: Left, right: u16) -> Option<LigKern> {
        self.ligkern.get(&(font, left, right)).copied()
    }
    fn has_left_boundary(&self, font: FontId) -> bool {
        self.ligkern
            .keys()
            .any(|k| k.0 == font && k.1 == Left::Boundary)
    }
    fn bchar(&self, font: FontId) -> Option<u16> {
        self.bchar.get(&font).copied()
    }
    fn param(&self, font: FontId, n: u16) -> Scaled {
        self.params.get(&(font, n)).copied().unwrap_or(0)
    }
    fn param_count(&self, font: FontId) -> u16 {
        self.params
            .keys()
            .filter(|k| k.0 == font)
            .map(|k| k.1)
            .max()
            .unwrap_or(0)
    }
}
