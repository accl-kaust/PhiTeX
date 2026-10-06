//! `XeTeX`'s pages: the glyphs xdvipdfmx's glyph runs give
//! (`partex_xdvipdfmx::glyphrun`), drawn from their own fonts' outlines
//! (OpenType and TrueType through skrifa, a TFM font's Type 1 from its
//! `.pfb`), the PDF giving the paths and rules ([`crate::Pdf::draw_with`]).
//! The Overleaf extension core's `xetex::extra`.

// (geometry in f64 rounded to integers, as pdfdraw's)
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names
)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use partex_xdvipdfmx::api::{GlyphRun, GlyphSource};

use crate::esc;
use crate::pdfdraw::Extra;

/// TeX Live's tree, as the XDV names its fonts.
pub const TL: &[u8] = b"/usr/share/texmf-dist/";

/// `name` inside TeX Live, if it is in it.
#[must_use]
pub fn rel(name: &[u8]) -> &[u8] {
    name.strip_prefix(TL).unwrap_or(name)
}

/// A font program read with skrifa, for its outlines and advances.
pub struct Face {
    data: Arc<[u8]>,
    index: u32,
    upem: f32,
}

impl Face {
    fn new(data: Arc<[u8]>, index: u32) -> Option<Face> {
        use skrifa::raw::TableProvider;
        let f = skrifa::FontRef::from_index(&data, index).ok()?;
        let upem = f.head().map_or(1000.0, |h| f32::from(h.units_per_em()));
        Some(Face { data, index, upem })
    }

    /// Glyph `gid`'s advance, in ems.
    fn advance(&self, gid: u16) -> f64 {
        use skrifa::MetadataProvider;
        use skrifa::instance::{LocationRef, Size};
        let Ok(f) = skrifa::FontRef::from_index(&self.data, self.index) else {
            return 0.5;
        };
        let a = f
            .glyph_metrics(Size::unscaled(), LocationRef::default())
            .advance_width(skrifa::GlyphId::new(u32::from(gid)))
            .unwrap_or(500.0);
        f64::from(a / self.upem)
    }

    /// Glyph `gid`'s outline as SVG path data in 1/1000 em, y up.
    fn path(&self, gid: u16) -> Option<String> {
        use skrifa::MetadataProvider;
        use skrifa::instance::{LocationRef, Size};
        use skrifa::outline::DrawSettings;
        let f = skrifa::FontRef::from_index(&self.data, self.index).ok()?;
        let g = f
            .outline_glyphs()
            .get(skrifa::GlyphId::new(u32::from(gid)))?;
        let mut pen = crate::glyphs::Pen {
            d: String::new(),
            k: 1000.0 / self.upem,
        };
        g.draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .ok()?;
        Some(pen.d)
    }
}

/// A font program read for its outlines: an OpenType/TrueType face
/// (skrifa), or a Type 1 font (a TFM font's, from its map entry).
pub enum Prog {
    Otf(Face),
    T1(crate::type1::Type1),
}

/// A font file's bytes, by its name (none: not found).
pub type Bytes<'a> = dyn FnMut(&[u8]) -> Option<Arc<[u8]>> + 'a;

/// Font programs read, by (file, face index), kept across builds (`None`: unreadable).
pub type Faces = HashMap<(Vec<u8>, u32), Option<Prog>>;

/// A run's font program: its file and face (`u32::MAX`: a Type 1 font).
type Program<'a> = (&'a [u8], u32);

/// A run's font program and its glyph: a gid, or a Type 1 code and name.
/// `None`: drawn some other way (a `CMap`'s composite font).
fn glyph_of(s: &GlyphSource) -> Option<(Program<'_>, u16, Option<&[u8]>)> {
    match s {
        GlyphSource::Native {
            font_file,
            face_index,
            gid,
        }
        | GlyphSource::TrueType {
            font_file,
            face_index,
            gid,
        }
        | GlyphSource::OpenType {
            font_file,
            face_index,
            gid,
        } => Some(((font_file, *face_index), *gid, None)),
        GlyphSource::Type1 {
            font_file,
            glyph_name,
            code,
        } => Some(((font_file, u32::MAX), u16::from(*code), Some(glyph_name))),
        GlyphSource::Other { .. } => None,
    }
}

/// The draw list's additions for a page's glyph runs, page height `h`
/// (bp; the runs' y is up from the bottom), font refs from `f0`. A run
/// whose transform (`ctm`'s linear part times `tm`) is not the plain
/// `size` scale is drawn alone with its matrix (rotated, slanted,
/// extended text); the others in runs along a baseline.
pub fn extra(
    runs: &[GlyphRun],
    h: f64,
    f0: usize,
    faces: &mut Faces,
    bytes: &mut Bytes<'_>,
) -> Extra {
    let mut e = Extra::default();
    let mut ids: HashMap<(Vec<u8>, u32), usize> = HashMap::new();
    let mut drawn: std::collections::HashSet<(usize, u16)> = std::collections::HashSet::new();
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let r5 = |v: f64| (v * 1e5).round() / 1e5;
    // (a run's linear map from ems to the page, y up: L·size·Tm)
    let lin = |r: &GlyphRun| {
        let [a, b, c, d, _, _] = r.ctm;
        let [t0, t1, t2, t3] = r.tm;
        let k = r.size;
        [
            k * (a * t0 + c * t1),
            k * (b * t0 + d * t1),
            k * (a * t2 + c * t3),
            k * (b * t2 + d * t3),
        ]
    };
    let plain = |r: &GlyphRun| {
        let m = lin(r);
        (m[0] - r.size).abs() < 1e-6
            && m[1].abs() < 1e-6
            && m[2].abs() < 1e-6
            && (m[3] - r.size).abs() < 1e-6
    };
    let colour = |r: &GlyphRun| {
        if r.rgba >> 8 == 0 {
            String::new()
        } else {
            format!(",{}", esc(&format!("#{:06x}", r.rgba >> 8)))
        }
    };
    // (a font program's ref, read once)
    let mut fref = |e: &mut Extra, file: &[u8], face: u32| -> usize {
        *ids.entry((file.to_vec(), face)).or_insert_with(|| {
            let base = String::from_utf8_lossy(rel(file))
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string();
            let id: String = format!(
                "x{}_{}",
                base,
                if face == u32::MAX {
                    "t1".into()
                } else {
                    face.to_string()
                }
            )
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
            e.fonts.push(id);
            f0 + e.fonts.len() - 1
        })
    };
    let mut i = 0;
    while i < runs.len() {
        let r0 = &runs[i];
        let Some(((file0, face0), _, _)) = glyph_of(&r0.source) else {
            i += 1;
            continue;
        };
        let fr = fref(&mut e, file0, face0);
        let prog = faces.entry((file0.to_vec(), face0)).or_insert_with(|| {
            let b = bytes(file0)?;
            if face0 == u32::MAX {
                crate::type1::Type1::from_file(&b).map(Prog::T1)
            } else {
                Face::new(b, face0).map(Prog::Otf)
            }
        });
        // (outline and advance (ems) of a run's glyph)
        let mut outline = |e: &mut Extra, g: u16, name: Option<&[u8]>| -> f64 {
            let Some(p) = prog.as_ref() else { return 0.5 };
            match p {
                Prog::Otf(f) => {
                    if drawn.insert((fr, g))
                        && let Some(d) = f.path(g)
                    {
                        let _ = write!(
                            e.g,
                            "{}\"{fr}:{g}\":{}",
                            if e.g.is_empty() { "" } else { "," },
                            esc(&d)
                        );
                    }
                    f.advance(g)
                }
                Prog::T1(t) => {
                    let n = String::from_utf8_lossy(name.unwrap_or(b".notdef"));
                    if drawn.insert((fr, g))
                        && let Some(d) = t.path(&n)
                    {
                        let _ = write!(
                            e.g,
                            "{}\"{fr}:{g}\":{}",
                            if e.g.is_empty() { "" } else { "," },
                            esc(&d)
                        );
                    }
                    t.width(&n).unwrap_or(500.0) / 1000.0
                }
            }
        };
        // (a transformed glyph: alone, with its matrix in SVG's terms,
        // y down, outlines in 1/1000 em)
        if !plain(r0) {
            let (_, g, name) = glyph_of(&r0.source).unwrap_or(((&[], 0), 0, None));
            outline(&mut e, g, name);
            let m = lin(r0);
            let _ = write!(
                e.t,
                "{}[-1,{},{},{},\"\",{fr},[{g}],{},[{},{},{},{}]]",
                if e.t.is_empty() { "" } else { "," },
                r2(r0.size),
                r2(h - r0.y),
                esc(&r2(r0.x).to_string()),
                if r0.rgba >> 8 == 0 {
                    "null".to_string()
                } else {
                    esc(&format!("#{:06x}", r0.rgba >> 8))
                },
                r5(m[0] / 1000.0),
                r5(-m[1] / 1000.0),
                r5(m[2] / 1000.0),
                r5(-m[3] / 1000.0)
            );
            i += 1;
            continue;
        }
        let mut j = i;
        let (mut xs, mut codes, mut txt, mut txs) =
            (String::new(), String::new(), String::new(), String::new());
        // (where the glyph before ended: a gap wider than a fifth of the
        // size is a word space, which TeX sets as glue, not a glyph)
        let mut end: Option<f64> = None;
        // (a cluster whose ActualText was taken: its other glyphs add no text)
        let mut cluster_done: Option<u32> = None;
        while j < runs.len() {
            let r = &runs[j];
            let Some(((file, face), g, name)) = glyph_of(&r.source) else {
                break;
            };
            if file != file0
                || face != face0
                || (r.size - r0.size).abs() > 1e-6
                || (r.y - r0.y).abs() > 1e-3
                || r.rgba != r0.rgba
                || !plain(r)
            {
                break;
            }
            let _ = write!(xs, "{}{}", if xs.is_empty() { "" } else { " " }, r2(r.x));
            let _ = write!(codes, "{}{g}", if codes.is_empty() { "" } else { "," });
            let adv = outline(&mut e, g, name);
            if let Some(e) = end
                && r.x - e > 0.2 * r.size
            {
                txt.push(' ');
                let _ = write!(txs, " {}", r2(e));
            }
            if cluster_done != Some(r.cluster)
                && let Some(t) = &r.text
            {
                let n = t.chars().count().max(1);
                for (k, c) in t.chars().enumerate() {
                    txt.push(c);
                    let _ = write!(
                        txs,
                        "{}{}",
                        if txs.is_empty() { "" } else { " " },
                        r2(r.x + adv * r.size * k as f64 / n as f64)
                    );
                }
                if r.actual_text {
                    cluster_done = Some(r.cluster);
                }
            }
            end = Some(r.x + adv * r.size);
            j += 1;
        }
        let _ = write!(
            e.t,
            "{}[-1,{},{},{},\"\",{fr},[{codes}]{}]",
            if e.t.is_empty() { "" } else { "," },
            r2(r0.size),
            r2(h - r0.y),
            esc(&xs),
            colour(r0)
        );
        // (the text over the glyphs, invisible: for selecting and finding)
        if !txt.is_empty() {
            let _ = write!(
                e.t,
                ",[0,{},{},{},{},1]",
                r2(r0.size),
                r2(h - r0.y),
                esc(&txs),
                esc(&txt)
            );
        }
        i = j.max(i + 1);
    }
    e
}
