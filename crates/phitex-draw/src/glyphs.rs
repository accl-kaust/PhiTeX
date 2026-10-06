//! Glyph outlines of the embedded fonts that are not Type 1 programs:
//! `TrueType` (`/FontFile2`, and `/FontFile3 /OpenType`) and bare CFF
//! (`/FontFile3 /Type1C`), read with skrifa; each a code's glyph as SVG
//! path data in 1/1000 em, y up, as [`crate::type1::Type1::path`]'s.
//! (Type 3 glyphs, content streams, are read in `pdfdraw`.)

use std::fmt::Write as _;

use skrifa::raw::ps::cff::CffFontRef;
use skrifa::raw::tables::cmap::PlatformId;

/// A pen writing SVG path data, its coordinates times `k`, rounded.
pub(crate) struct Pen {
    pub d: String,
    pub k: f32,
}

impl skrifa::outline::OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let k = self.k;
        let _ = write!(self.d, "M{:.0} {:.0}", x * k, y * k);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let k = self.k;
        let _ = write!(self.d, "L{:.0} {:.0}", x * k, y * k);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let k = self.k;
        let _ = write!(
            self.d,
            "Q{:.0} {:.0} {:.0} {:.0}",
            x1 * k,
            y1 * k,
            x * k,
            y * k
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let k = self.k;
        let _ = write!(
            self.d,
            "C{:.0} {:.0} {:.0} {:.0} {:.0} {:.0}",
            x1 * k,
            y1 * k,
            x2 * k,
            y2 * k,
            x * k,
            y * k
        );
    }
    fn close(&mut self) {
        self.d.push('Z');
    }
}

/// Windows-1252's 0x80–0x9f, as `WinAnsiEncoding` reads them (0: none).
const CP1252: [u16; 32] = [
    0x20ac, 0, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017d, 0, 0, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x02dc,
    0x2122, 0x0161, 0x203a, 0x0153, 0, 0x017e, 0x0178,
];

/// What a simple font's code means, for finding its glyph: the name the
/// font's `/Differences` give it, its `/ToUnicode` text.
pub(crate) struct Code<'a> {
    pub name: Option<&'a str>,
    pub text: Option<&'a str>,
}

/// The glyphs of a simple `TrueType` (or `OpenType`) font `data` for codes
/// 0–255, much as PDF 32000 §9.6.6.4 finds them: a nonsymbolic font's
/// code by its `/Differences` name in `post`, else by its character (its
/// text's, else `WinAnsiEncoding`'s) in the (3,1) `cmap`; a symbolic
/// one's in (3,0) at `0xF000` + code, else the code in (1,0).
pub(crate) fn sfnt_gids<'a>(
    data: &[u8],
    symbolic: bool,
    code: &dyn Fn(u8) -> Code<'a>,
) -> Vec<Option<u32>> {
    use skrifa::raw::TableProvider;
    let Ok(f) = skrifa::FontRef::new(data) else {
        return vec![None; 256];
    };
    let cmap = f.cmap().ok();
    let sub = |p: PlatformId, e: u16| {
        let c = cmap.as_ref()?;
        c.encoding_records()
            .iter()
            .find(|r| r.platform_id() == p && r.encoding_id() == e)
            .and_then(|r| r.subtable(c.offset_data()).ok())
    };
    let (uni, sym, mac) = (
        sub(PlatformId::Windows, 1),
        sub(PlatformId::Windows, 0),
        sub(PlatformId::Macintosh, 0),
    );
    let post = f.post().ok();
    let found = |g: skrifa::GlyphId| Some(g.to_u32()).filter(|&g| g != 0);
    (0..=255u8)
        .map(|c| {
            let code = code(c);
            let ch = code
                .text
                .and_then(|t| t.chars().next())
                .or_else(|| match c {
                    0x80..=0x9f => char::from_u32(u32::from(CP1252[usize::from(c - 0x80)])),
                    c => Some(char::from(c)),
                });
            let by_uni = || {
                ch.and_then(|ch| uni.as_ref()?.map_codepoint(ch))
                    .and_then(found)
            };
            let by_sym = || {
                [0xf000, 0, 0xf100, 0xf200].iter().find_map(|b| {
                    sym.as_ref()?
                        .map_codepoint(b | u32::from(c))
                        .and_then(found)
                })
            };
            let by_mac = || mac.as_ref()?.map_codepoint(c).and_then(found);
            let by_name = || {
                let name = code.name?;
                let post = post.as_ref()?;
                (0..f.maxp().ok()?.num_glyphs()).find_map(|g| {
                    (post.glyph_name(skrifa::raw::types::GlyphId16::new(g)) == Some(name))
                        .then_some(u32::from(g))
                })
            };
            if symbolic {
                by_sym().or_else(by_mac).or_else(by_uni).or_else(by_name)
            } else {
                by_name().or_else(by_uni).or_else(by_sym).or_else(by_mac)
            }
        })
        .collect()
}

/// Glyph `gid` of `TrueType` or `OpenType` font `data`.
pub(crate) fn sfnt_path(data: &[u8], gid: u32) -> Option<String> {
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::DrawSettings;
    use skrifa::raw::TableProvider;
    let f = skrifa::FontRef::new(data).ok()?;
    let upem = f.head().map_or(1000.0, |h| f32::from(h.units_per_em()));
    let g = f.outline_glyphs().get(skrifa::GlyphId::new(gid))?;
    let mut pen = Pen {
        d: String::new(),
        k: 1000.0 / upem,
    };
    g.draw(
        DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
        &mut pen,
    )
    .ok()?;
    Some(pen.d)
}

/// The glyphs of bare CFF font `data` for codes 0–255: by the name
/// `/Differences` gives a code (`name`), in the font's charset, else by
/// the font's own encoding.
pub(crate) fn cff_gids(data: &[u8], name: &dyn Fn(u8) -> Option<String>) -> Vec<Option<u32>> {
    let Ok(f) = CffFontRef::new(data, 0, None) else {
        return vec![None; 256];
    };
    let names: std::collections::HashMap<&[u8], u32> = f
        .charset()
        .map(|cs| {
            cs.iter()
                .filter_map(|(g, sid)| Some((f.string(sid)?, g.to_u32())))
                .collect()
        })
        .unwrap_or_default();
    let enc = f.encoding();
    (0..=255u8)
        .map(|c| match name(c) {
            Some(n) => names.get(n.as_bytes()).copied(),
            None => enc.as_ref()?.map(c).map(skrifa::GlyphId::to_u32),
        })
        .map(|g| g.filter(|&g| g != 0))
        .collect()
}

/// Glyph `gid` of bare CFF font `data`.
pub(crate) fn cff_path(data: &[u8], gid: u32) -> Option<String> {
    let f = CffFontRef::new(data, 0, None).ok()?;
    let gid = skrifa::GlyphId::new(gid);
    let sub = f.subfont(f.subfont_index(gid).unwrap_or(0), &[]).ok()?;
    let mut pen = Pen {
        d: String::new(),
        k: 1000.0 / f32::from(i16::try_from(f.upem()).unwrap_or(1000)),
    };
    f.draw(&sub, gid, &[], None, &mut pen).ok()?;
    Some(pen.d)
}
