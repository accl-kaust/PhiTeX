//! Directional runs of a word, as XeTeX asks ICU's `ubidi` for them
//! (`ubidi_setPara` with a default paragraph level, `ubidi_getDirection`,
//! `ubidi_getVisualRun`), computed with `unicode-bidi`.
//!
//! The direction follows ICU's `directionFromFlags`: left to right unless
//! the text has a right-to-left class (or an Arabic number with a
//! neutral), right to left if it has no left-to-right class, mixed
//! otherwise; the paragraph level counts as one of the classes.

use alloc::vec::Vec;

use unicode_bidi::{BidiClass, Level, bidi_class};

/// `UBiDiDirection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BidiDirection {
    Ltr,
    Rtl,
    Mixed,
}

/// A visual run: `text[start..start + len]`, right to left or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub len: usize,
    pub rtl: bool,
}

/// The analysis of a word: its direction and, when mixed, its runs in
/// visual order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bidi {
    pub direction: BidiDirection,
    pub runs: Vec<Run>,
}

fn chars(text: &[u16]) -> Vec<(usize, char)> {
    let mut v = Vec::with_capacity(text.len());
    let mut i = 0;
    for r in char::decode_utf16(text.iter().copied()) {
        let (c, n) = match r {
            Ok(c) => (c, c.len_utf16()),
            Err(_) => ('\u{FFFD}', 1),
        };
        v.push((i, c));
        i += n;
    }
    v
}

/// The paragraph level ICU picks with `UBIDI_DEFAULT_LTR` or
/// `UBIDI_DEFAULT_RTL` (rules P2 and P3: the first strong character
/// outside isolates).
fn para_level(text: &[(usize, char)], default_rtl: bool) -> bool {
    let mut depth = 0usize;
    for &(_, c) in text {
        match bidi_class(c) {
            BidiClass::LRI | BidiClass::RLI | BidiClass::FSI => depth += 1,
            BidiClass::PDI => depth = depth.saturating_sub(1),
            BidiClass::L if depth == 0 => return false,
            BidiClass::R | BidiClass::AL if depth == 0 => return true,
            BidiClass::B => break,
            _ => {}
        }
    }
    default_rtl
}

/// `ubidi_setPara(text, default level)` then `ubidi_getDirection` and,
/// for a mixed text, `ubidi_getVisualRun` for each run.
#[must_use]
pub fn analyze(text: &[u16], default_rtl: bool) -> Bidi {
    let cs = chars(text);
    let rtl_para = para_level(&cs, default_rtl);
    let (mut has_rtl, mut has_ltr, mut has_an, mut has_n) = (rtl_para, !rtl_para, false, false);
    for &(_, c) in &cs {
        match bidi_class(c) {
            BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI => {
                has_rtl = true;
            }
            BidiClass::L | BidiClass::EN | BidiClass::LRE | BidiClass::LRO | BidiClass::LRI => {
                has_ltr = true;
            }
            BidiClass::AN => {
                has_ltr = true;
                has_an = true;
            }
            BidiClass::ON
            | BidiClass::CS
            | BidiClass::ES
            | BidiClass::ET
            | BidiClass::B
            | BidiClass::S
            | BidiClass::WS
            | BidiClass::BN
            | BidiClass::PDF
            | BidiClass::FSI
            | BidiClass::PDI => has_n = true,
            BidiClass::NSM => {}
        }
        // FSI is LRI or RLI for ICU; it is both an isolate (a neutral) and
        // one of the two.
        if bidi_class(c) == BidiClass::FSI {
            has_ltr = true;
        }
    }
    let direction = if !(has_rtl || (has_an && has_n)) {
        BidiDirection::Ltr
    } else if !has_ltr {
        BidiDirection::Rtl
    } else {
        BidiDirection::Mixed
    };
    if direction != BidiDirection::Mixed {
        return Bidi {
            direction,
            runs: Vec::new(),
        };
    }
    let level = if rtl_para { Level::rtl() } else { Level::ltr() };
    let info = unicode_bidi::utf16::BidiInfo::new(text, Some(level));
    let mut runs = Vec::new();
    for para in &info.paragraphs {
        let (levels, vruns) = info.visual_runs(para, para.range.clone());
        for r in vruns {
            if r.is_empty() {
                continue;
            }
            runs.push(Run {
                start: r.start,
                len: r.end - r.start,
                rtl: levels[r.start].is_rtl(),
            });
        }
    }
    Bidi { direction, runs }
}
