//! XeTeX's font side (`XeTeX_ext.c`, `XeTeXLayoutInterface.cpp`,
//! `XeTeXFontInst.cpp`, `XeTeXOTMath.cpp`) on the neutral core: the same
//! `float` and `Fixed` arithmetic, so every number given to TeX is the
//! oracle's.
//!
//! What stays the engine's: TeX's memory and nodes, `font_info`, the
//! printing of diagnostics (returned here as [`Diagnostic`]s), the
//! glyph-bounding-box cache keyed by TeX font number (a cache only).
//! [`XeTeXFont`] is `XeTeXLayoutEngine` + `XeTeXFontInst`: one per
//! `\font` loaded, sharing its [`Face`] with every other size.

pub mod fontmgr;
mod ot_lang;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::face::Face;
use crate::layout::{self, Table};
use crate::math::{GlyphPart, KernSide, Math};
use crate::shape::{Direction, Feature, ShapeRequest, Shaper};
use crate::teckit::Mapping;
use crate::{FontSource, KpseFormat, Tag, tag};

/// TeX's scaled points / Mac `Fixed` (16.16).
pub type Fixed = i32;

/// `D2Fix`: `(int)(d * 65536.0 + 0.5)` (truncated toward zero).
#[must_use]
pub fn d2fix(d: f64) -> Fixed {
    (d * 65536.0 + 0.5) as i32
}

/// `Fix2D`.
#[must_use]
pub fn fix2d(f: Fixed) -> f64 {
    f64::from(f) / 65536.0
}

/// tex.web's `xn_over_d` (§107): `x * n / d` in TeX's arithmetic.
#[must_use]
pub fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let positive = x >= 0;
    let x = i64::from(x).abs();
    let (n, d) = (i64::from(n), i64::from(d));
    let t = (x % 0o100000) * n;
    let mut u = (x / 0o100000) * n + (t / 0o100000);
    let v = (u % d) * 0o100000 + (t % 0o100000);
    if u / d >= 0o100000 {
        // arith_error; TeX keeps u
    } else {
        u = 0o100000 * (u / d) + (v / d);
    }
    if positive { u as i32 } else { -(u as i32) }
}

/// `hb_tag_from_string(s, len)`: up to four bytes (stopping at a NUL),
/// padded with spaces; 0 for an empty string.
#[must_use]
pub fn tag_from_str(s: &[u8]) -> Tag {
    if s.is_empty() || s[0] == 0 {
        return 0;
    }
    let mut t = [b' '; 4];
    for (i, &c) in s.iter().take(4).enumerate() {
        if c == 0 {
            break;
        }
        t[i] = c;
    }
    u32::from_be_bytes(t)
}

/// `hb_ot_tag_to_language`: the BCP 47 language HarfBuzz gives an
/// OpenType language tag (`None` for `dflt`).
#[must_use]
pub fn ot_tag_to_language(t: Tag) -> Option<String> {
    use core::fmt::Write;
    if t == tag(b"dflt") {
        return None;
    }
    if let Ok(i) = ot_lang::OT_TAG_TO_LANGUAGE.binary_search_by_key(&t, |e| e.0) {
        return Some(String::from(ot_lang::OT_TAG_TO_LANGUAGE[i].1));
    }
    let b = t.to_be_bytes();
    let mut s = String::new();
    if b[0].is_ascii_alphabetic()
        && b[1].is_ascii_alphabetic()
        && b[2].is_ascii_alphabetic()
        && b[3] == b' '
    {
        for &c in &b[..3] {
            s.push(char::from(c.to_ascii_lowercase()));
        }
        s.push('-');
    }
    let _ = write!(s, "x-hbot-{t:08x}");
    Some(s)
}

/// `hb_ot_tag_to_script`: an OpenType script tag as `hb_script_t`
/// (an ISO 15924 tag, e.g. `latn` → `Latn`, `dev2` → `Deva`).
#[must_use]
pub fn ot_tag_to_script(t: Tag) -> Tag {
    let digit = t & 0xFF;
    if digit == u32::from(b'2') || digit == u32::from(b'3') {
        let t2 = t & 0xFFFF_FF32;
        let new = [
            (b"bng2", b"Beng"),
            (b"dev2", b"Deva"),
            (b"gjr2", b"Gujr"),
            (b"gur2", b"Guru"),
            (b"knd2", b"Knda"),
            (b"mlm2", b"Mlym"),
            (b"ory2", b"Orya"),
            (b"tml2", b"Taml"),
            (b"tel2", b"Telu"),
            (b"mym2", b"Mymr"),
        ];
        for (k, v) in new {
            if t2 == tag(k) {
                return tag(v);
            }
        }
        return 0x5A7A_7A7A; // HB_SCRIPT_UNKNOWN 'Zzzz'
    }
    if t == tag(b"DFLT") {
        return 0;
    }
    if t == tag(b"math") {
        return tag(b"Zmth");
    }
    let mut t = t;
    if t & 0x0000_FF00 == 0x0000_2000 {
        t |= (t >> 8) & 0x0000_FF00;
    }
    if t & 0x0000_00FF == 0x0000_0020 {
        t |= (t >> 8) & 0x0000_00FF;
    }
    t & !0x2000_0000
}

/// `FONT_FLAGS_COLORED`.
pub const FONT_FLAGS_COLORED: u8 = 0x01;
/// `FONT_FLAGS_VERTICAL`.
pub const FONT_FLAGS_VERTICAL: u8 = 0x02;

/// What XeTeX prints while loading a font (`font_feature_warning`,
/// `font_mapping_warning`, the `-> path` of `\tracingfonts`). Each carries
/// `name_of_file` as it was then (without its leading space).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// "Unknown feature `F' in font `N'."
    UnknownFeature { feature: String, font: String },
    /// "Loaded mapping `M' for font `N'." (`\tracingfonts` > 1)
    MappingLoaded { mapping: String, font: String },
    /// "Font mapping `M' for font `N' not found."
    MappingNotFound { mapping: String, font: String },
    /// "Font mapping `M' for font `N' not usable; bad mapping file or
    /// incorrect mapping type."
    MappingNotUsable { mapping: String, font: String },
    /// " -> PATH" (`\tracingfonts` > 0).
    FontPath(String),
}

/// A glyph's bounds in points (`GlyphBBox`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlyphBBox {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

/// One glyph of a native word: id and position (scaled points, y
/// negative upwards), and its advance (`glyphAdvances`, used for letter
/// spacing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeGlyph {
    pub gid: u16,
    pub x: Fixed,
    pub y: Fixed,
    pub advance: Fixed,
}

/// `measure_native_node`'s layout of a word: its glyphs and width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeLayout {
    pub glyphs: Vec<NativeGlyph>,
    pub width: Fixed,
}

/// The fontdimens `load_native_font` gives a native font (xetex.web):
/// slant, space, stretch, shrink, x-height, quad, extra space, cap height,
/// then for a MATH font the count and the 56 math constants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeParams {
    pub params: Vec<Fixed>,
    pub height_base: Fixed,
    pub depth_base: Fixed,
}

/// A loaded native font (`XeTeXLayoutEngine` with its `XeTeXFontInst`).
#[derive(Clone)]
pub struct XeTeXFont {
    pub face: Arc<Face>,
    /// The file's path as XeTeX writes it in the XDV.
    pub path: Arc<str>,
    /// The face index.
    pub index: u32,
    /// `m_pointSize`.
    pub point_size: f32,
    /// The size asked for (`scaled_size` after a negative one is
    /// resolved).
    pub scaled_size: Fixed,
    /// `engine->script` (an OpenType script tag, 0 for none).
    pub script: Tag,
    /// `engine->language`, as HarfBuzz holds it.
    pub language: Option<String>,
    pub features: Vec<Feature>,
    /// `script=`'s, `language=`'s and `shaper=`'s raw values aside, the
    /// engine requested (`/OT`, `/GR`, `/AAT`: 'O', 'G', 'A', or 0).
    pub req_engine: u8,
    /// `0xRRGGBBAA`.
    pub rgba: u32,
    pub extend: f32,
    pub slant: f32,
    pub embolden: f32,
    /// `loadedfontflags`.
    pub flags: u8,
    /// `loadedfontletterspace`.
    pub letter_space: Fixed,
    /// `loadedfontmapping`.
    pub mapping: Option<Arc<Mapping>>,
    /// `loadedfontdesignsize`.
    pub design_size: Fixed,
    /// `name_of_file` after the lookup (without the leading space): the
    /// font's full name with the style and feature strings, for a name;
    /// the spec as given for `[file]`.
    pub name_of_file: String,
    /// The script of the last buffer shaped with this font
    /// (`hb_buffer_get_script(engine->hbBuffer)`), which sets the next
    /// word's default direction.
    pub last_script: Option<Tag>,
}

/// `readCommonFeatures`'s `read_double`.
fn read_double(s: &[u8], mut i: usize) -> (f64, usize) {
    while matches!(s.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    let mut neg = false;
    if s.get(i) == Some(&b'-') {
        neg = true;
        i += 1;
    } else if s.get(i) == Some(&b'+') {
        i += 1;
    }
    let mut val = 0.0f64;
    while let Some(&c @ b'0'..=b'9') = s.get(i) {
        val = val * 10.0 + f64::from(c - b'0');
        i += 1;
    }
    if s.get(i) == Some(&b'.') {
        let mut dec = 10.0f64;
        i += 1;
        while let Some(&c @ b'0'..=b'9') = s.get(i) {
            val += f64::from(c - b'0') / dec;
            i += 1;
            dec *= 10.0;
        }
    }
    (if neg { -val } else { val }, i)
}

/// `read_rgb_a`: (value, bytes read).
fn read_rgb_a(s: &[u8]) -> (u32, usize) {
    let hex = |c: u8| match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'A'..=b'F' => Some(u32::from(c - b'A' + 10)),
        b'a'..=b'f' => Some(u32::from(c - b'a' + 10)),
        _ => None,
    };
    let mut v = 0u32;
    for i in 0..6 {
        match s.get(i).and_then(|&c| hex(c)) {
            Some(h) => v = (v << 4) + h,
            None => return (0x0000_00FF, i),
        }
    }
    v <<= 8;
    let mut alpha = 0u32;
    let mut n = 0;
    while n < 2 {
        match s.get(6 + n).and_then(|&c| hex(c)) {
            Some(h) => alpha = (alpha << 4) + h,
            None => break,
        }
        n += 1;
    }
    if n == 2 {
        (v + alpha, 8)
    } else {
        (v + 0xFF, 6 + n)
    }
}

/// `read_tag_with_param`: the tag and the `=n` parameter.
fn read_tag_with_param(s: &[u8]) -> (Tag, i32) {
    let mut i = 0;
    while i < s.len() && !matches!(s[i], b':' | b';' | b',' | b'=') {
        i += 1;
    }
    let t = tag_from_str(&s[..i]);
    let mut param = 0i32;
    if s.get(i) == Some(&b'=') {
        i += 1;
        let mut neg = false;
        if s.get(i) == Some(&b'-') {
            neg = true;
            i += 1;
        }
        while let Some(&c @ b'0'..=b'9') = s.get(i) {
            param = param.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
            i += 1;
        }
        if neg {
            param = -param;
        }
    }
    (t, param)
}

/// `splitFontName`: (name, variant, features, face index).
type SplitName<'a> = (&'a [u8], Option<&'a [u8]>, Option<&'a [u8]>, u32);

fn split_font_name(name: &[u8]) -> SplitName<'_> {
    let mut var: Option<usize> = None;
    let mut feat: Option<usize> = None;
    let mut index = 0u32;
    let end = name.len();
    if name.first() == Some(&b'[') {
        let mut within = true;
        let mut i = 1;
        while i < end {
            let c = name[i];
            if within && c == b']' {
                within = false;
                if var.is_none() {
                    var = Some(i);
                }
            } else if c == b':' {
                if within && var.is_none() {
                    var = Some(i);
                    i += 1;
                    while i < end && name[i].is_ascii_digit() {
                        index = index
                            .wrapping_mul(10)
                            .wrapping_add(u32::from(name[i] - b'0'));
                        i += 1;
                    }
                    i -= 1;
                } else if !within && feat.is_none() {
                    feat = Some(i);
                }
            }
            i += 1;
        }
    } else {
        for (i, &c) in name.iter().enumerate() {
            if c == b'/' && var.is_none() && feat.is_none() {
                var = Some(i);
            } else if c == b':' && feat.is_none() {
                feat = Some(i);
            }
        }
    }
    let feat = feat.unwrap_or(end);
    let var = var.unwrap_or(feat);
    let name_s = &name[..var];
    let var_s = (feat > var).then(|| &name[var + 1..feat]);
    let feat_s = (end > feat).then(|| &name[feat + 1..end]);
    (name_s, var_s, feat_s, index)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The font being loaded: what `loadOTfont` and `findnativefont` set.
struct Loading<'a> {
    src: &'a dyn FontSource,
    tracing_fonts: i32,
    diags: Vec<Diagnostic>,
    mapping: Option<Arc<Mapping>>,
    flags: u8,
    letter_space: Fixed,
    name_of_file: String,
}

impl Loading<'_> {
    /// `load_mapping_file(s, e, 0)`.
    fn load_mapping(&mut self, name: &[u8]) -> Option<Arc<Mapping>> {
        let mut file = lossy(name);
        file.push_str(".tec");
        let Some(path) = self.src.find_file(&file, KpseFormat::MiscFonts) else {
            self.diags.push(Diagnostic::MappingNotFound {
                mapping: file,
                font: self.name_of_file.clone(),
            });
            return None;
        };
        let cnv = self
            .src
            .read(&path)
            .and_then(|b| Mapping::new(&b, true, true))
            .map(Arc::new);
        if cnv.is_none() {
            self.diags.push(Diagnostic::MappingNotUsable {
                mapping: file,
                font: self.name_of_file.clone(),
            });
        } else if self.tracing_fonts > 1 {
            self.diags.push(Diagnostic::MappingLoaded {
                mapping: file,
                font: self.name_of_file.clone(),
            });
        }
        cnv
    }
}

/// `getDesignSize`: the `size` feature's design size in TeX points, else
/// 10.
#[must_use]
pub fn design_size(face: &Face) -> f64 {
    match layout::size_params(face) {
        Some(p) => f64::from(p.design_size) * 72.27 / 72.0 / 10.0,
        None => 10.0,
    }
}

/// `findnativefont(name, scaled_size)`: the font `name` names
/// (`"Name/B/I:features"` through `mgr`, or `"[file]:features"` through
/// kpathsea), at `scaled_size` (negative: a `scaled` factor of the design
/// size), with what XeTeX prints meanwhile. `None` (and the diagnostics)
/// where XeTeX fails.
pub fn find_native_font(
    mgr: &mut fontmgr::FontManager,
    src: &dyn FontSource,
    name: &str,
    scaled_size: Fixed,
    tracing_fonts: i32,
) -> (Option<XeTeXFont>, Vec<Diagnostic>) {
    let mut ld = Loading {
        src,
        tracing_fonts,
        diags: Vec::new(),
        mapping: None,
        flags: 0,
        letter_space: 0,
        name_of_file: String::from(name),
    };
    let (name_s, var_s, feat_s, index) = split_font_name(name.as_bytes());
    let mut scaled_size = scaled_size;
    let mut result = None;
    if name_s.first() == Some(&b'[') {
        let file = lossy(&name_s[1..]);
        let path = src
            .find_file(&file, KpseFormat::OpenType)
            .or_else(|| src.find_file(&file, KpseFormat::TrueType))
            .or_else(|| src.find_file(&file, KpseFormat::Type1));
        if let Some(path) = path
            && let Some(data) = src.read(&path)
            && let Some(face) = Face::new(data, index)
        {
            if scaled_size < 0 {
                let dsize = d2fix(design_size(&face));
                scaled_size = if scaled_size == -1000 {
                    dsize
                } else {
                    xn_over_d(dsize, -scaled_size, 1000)
                };
            }
            let design = d2fix(design_size(&face));
            let mut req = 0u8;
            if let Some(v) = var_s {
                if v.starts_with(b"/AAT") {
                    req = b'A';
                } else if v.starts_with(b"/OT") || v.starts_with(b"/ICU") {
                    req = b'O';
                } else if v.starts_with(b"/GR") {
                    req = b'G';
                }
            }
            let f = load_ot_font(
                &mut ld,
                face,
                Arc::from(path.as_str()),
                index,
                scaled_size,
                req,
                feat_s,
                design,
            );
            if tracing_fonts > 0 {
                ld.diags.push(Diagnostic::FontPath(path.clone()));
            }
            result = Some(f);
        }
    } else {
        let mut variant: Option<String> = var_s.map(lossy);
        let found = mgr.find_font(&lossy(name_s), variant.as_mut(), fix2d(scaled_size));
        if let Some(found) = found {
            for d in found.diagnostics {
                ld.diags.push(d);
            }
            let full = mgr.full_name(found.font);
            let entry = mgr.entry(found.font).clone();
            ld.name_of_file = full.clone();
            if let Some(data) = src.read(&entry.path)
                && let Some(face) = Face::new(data, entry.index)
            {
                if scaled_size < 0 {
                    let dsize = d2fix(design_size(&face));
                    scaled_size = if scaled_size == -1000 {
                        dsize
                    } else {
                        xn_over_d(dsize, -scaled_size, 1000)
                    };
                }
                let req = mgr.req_engine();
                let f = load_ot_font(
                    &mut ld,
                    face,
                    entry.path.clone(),
                    entry.index,
                    scaled_size,
                    req,
                    feat_s,
                    found.design_size,
                );
                result = Some(f);
            }
            let mut nof = full;
            if let Some(v) = &variant
                && !v.is_empty()
            {
                nof.push('/');
                nof.push_str(v);
            }
            if let Some(f) = feat_s
                && !f.is_empty()
            {
                nof.push(':');
                nof.push_str(&lossy(f));
            }
            ld.name_of_file = nof;
        }
    }
    let result = result.map(|mut f| {
        f.name_of_file.clone_from(&ld.name_of_file);
        f
    });
    (result, ld.diags)
}

/// `loadOTfont`: the feature string read, the engine made.
#[allow(clippy::too_many_arguments)]
fn load_ot_font(
    ld: &mut Loading,
    face: Arc<Face>,
    path: Arc<str>,
    index: u32,
    scaled_size: Fixed,
    req_engine: u8,
    feat: Option<&[u8]>,
    design_size: Fixed,
) -> XeTeXFont {
    let mut script: Tag = 0;
    let mut language: Option<Vec<u8>> = None;
    let mut features: Vec<Feature> = Vec::new();
    let mut rgb: u32 = 0x0000_00FF;
    let mut extend = 1.0f32;
    let mut slant = 0.0f32;
    let mut embolden = 0.0f32;
    let mut letterspace = 0.0f32;
    if let Some(s) = feat {
        let mut cp1 = 0usize;
        let at = |i: usize| s.get(i).copied().unwrap_or(0);
        while at(cp1) != 0 {
            if matches!(at(cp1), b':' | b';' | b',') {
                cp1 += 1;
            }
            while matches!(at(cp1), b' ' | b'\t') {
                cp1 += 1;
            }
            if at(cp1) == 0 {
                break;
            }
            let mut cp2 = cp1;
            while !matches!(at(cp2), 0 | b':' | b';' | b',') {
                cp2 += 1;
            }
            let opt = &s[cp1..cp2];
            let mut ok = None;
            if opt.starts_with(b"script") {
                ok = Some(if at(cp1 + 6) == b'=' {
                    script = tag_from_str(&s[cp1 + 7..cp2]);
                    true
                } else {
                    false
                });
            } else if opt.starts_with(b"language") {
                ok = Some(if at(cp1 + 8) == b'=' {
                    language = Some(s[cp1 + 9..cp2].to_vec());
                    true
                } else {
                    false
                });
            } else if opt.starts_with(b"shaper") {
                ok = Some(at(cp1 + 6) == b'=');
            }
            if ok.is_none() {
                // readCommonFeatures
                let common = |key: &[u8]| opt.starts_with(key);
                if common(b"mapping") {
                    ok = Some(if at(cp1 + 7) == b'=' {
                        ld.mapping = ld.load_mapping(&s[cp1 + 8..cp2]);
                        true
                    } else {
                        false
                    });
                } else if common(b"extend") {
                    ok = Some(at(cp1 + 6) == b'=');
                    if ok == Some(true) {
                        extend = read_double(s, cp1 + 7).0 as f32;
                    }
                } else if common(b"slant") {
                    ok = Some(at(cp1 + 5) == b'=');
                    if ok == Some(true) {
                        slant = read_double(s, cp1 + 6).0 as f32;
                    }
                } else if common(b"embolden") {
                    ok = Some(at(cp1 + 8) == b'=');
                    if ok == Some(true) {
                        embolden = read_double(s, cp1 + 9).0 as f32;
                    }
                } else if common(b"letterspace") {
                    ok = Some(at(cp1 + 11) == b'=');
                    if ok == Some(true) {
                        letterspace = read_double(s, cp1 + 12).0 as f32;
                    }
                } else if common(b"color") {
                    ok = Some(if at(cp1 + 5) == b'=' {
                        let (v, n) = read_rgb_a(&s[cp1 + 6..]);
                        rgb = v;
                        if n == 6 || n == 8 {
                            ld.flags |= FONT_FLAGS_COLORED;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    });
                }
            }
            if ok.is_none() {
                if at(cp1) == b'+' {
                    let (t, mut param) = read_tag_with_param(&s[cp1 + 1..]);
                    if param >= 0 {
                        param += 1;
                    }
                    features.push(Feature {
                        tag: t,
                        value: param as u32,
                        start: 0,
                        end: u32::MAX,
                    });
                    ok = Some(true);
                } else if at(cp1) == b'-' {
                    features.push(Feature {
                        tag: tag_from_str(&s[cp1 + 1..cp2]),
                        value: 0,
                        start: 0,
                        end: u32::MAX,
                    });
                    ok = Some(true);
                } else if opt.starts_with(b"vertical") {
                    let mut cp3 = cp2 as isize;
                    if matches!(at(cp2), b';' | b':' | b',') {
                        cp3 -= 1;
                    }
                    while cp3 >= 0 && matches!(at(cp3 as usize), 0 | b' ' | b'\t') {
                        cp3 -= 1;
                    }
                    if at(cp3 as usize) != 0 {
                        cp3 += 1;
                    }
                    if cp3 as usize == cp1 + 8 {
                        ld.flags |= FONT_FLAGS_VERTICAL;
                        ok = Some(true);
                    }
                }
            }
            if ok != Some(true) {
                ld.diags.push(Diagnostic::UnknownFeature {
                    feature: lossy(opt),
                    font: ld.name_of_file.clone(),
                });
            }
            cp1 = cp2;
        }
    }
    if embolden != 0.0 {
        embolden = (f64::from(embolden) * fix2d(scaled_size) / 100.0) as f32;
    }
    if letterspace != 0.0 {
        ld.letter_space = ((f64::from(letterspace) / 100.0) * f64::from(scaled_size)) as i32;
    }
    if ld.flags & FONT_FLAGS_COLORED == 0 {
        rgb = 0x0000_00FF;
    }
    let lang_tag = tag_from_str(language.as_deref().unwrap_or(b""));
    XeTeXFont {
        face,
        path,
        index,
        point_size: fix2d(scaled_size) as f32,
        scaled_size,
        script,
        language: ot_tag_to_language(lang_tag),
        features,
        req_engine,
        rgba: rgb,
        extend,
        slant,
        embolden,
        flags: ld.flags,
        letter_space: ld.letter_space,
        mapping: ld.mapping.clone(),
        design_size,
        name_of_file: ld.name_of_file.clone(),
        last_script: None,
    }
}

/// `\XeTeXOTcountscripts` and friends (`otfontget*`'s `what` codes).
pub mod what {
    pub const COUNT_GLYPHS: i32 = 1;
    pub const COUNT_FEATURES: i32 = 8;
    pub const FEATURE_CODE: i32 = 9;
    pub const FIND_FEATURE_BY_NAME: i32 = 10;
    pub const IS_EXCLUSIVE_FEATURE: i32 = 11;
    pub const COUNT_SELECTORS: i32 = 12;
    pub const SELECTOR_CODE: i32 = 13;
    pub const FIND_SELECTOR_BY_NAME: i32 = 14;
    pub const IS_DEFAULT_SELECTOR: i32 = 15;
    pub const OT_COUNT_SCRIPTS: i32 = 16;
    pub const OT_COUNT_LANGUAGES: i32 = 17;
    pub const OT_COUNT_FEATURES: i32 = 18;
    pub const OT_SCRIPT_CODE: i32 = 19;
    pub const OT_LANGUAGE_CODE: i32 = 20;
    pub const OT_FEATURE_CODE: i32 = 21;
}

/// `sup_cmd` / `sub_cmd` of `get_ot_math_kern`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    Sup,
    Sub,
}

impl XeTeXFont {
    /// `unitsToPoints`.
    #[must_use]
    pub fn units_to_points(&self, units: f32) -> f32 {
        (units * self.point_size) / f32::from(self.face.units_per_em())
    }

    /// `pointsToUnits`.
    #[must_use]
    pub fn points_to_units(&self, points: f32) -> f32 {
        (points * f32::from(self.face.units_per_em())) / self.point_size
    }

    /// `XeTeXFontInst::getGlyphWidth`.
    #[must_use]
    pub fn glyph_width(&self, gid: u32) -> f32 {
        self.units_to_points(self.face.h_advance(gid).unwrap_or(0) as f32)
    }

    /// `XeTeXFontInst::getGlyphBounds` (zeros where the glyph fails to
    /// load).
    #[must_use]
    pub fn font_glyph_bounds(&self, gid: u32) -> GlyphBBox {
        match self.face.cbox(gid) {
            Some(b) => GlyphBBox {
                x_min: self.units_to_points(b.x_min as f32),
                y_min: self.units_to_points(b.y_min as f32),
                x_max: self.units_to_points(b.x_max as f32),
                y_max: self.units_to_points(b.y_max as f32),
            },
            None => GlyphBBox::default(),
        }
    }

    /// `getGlyphBounds(engine, …)`: the bounds, x scaled by `extend`.
    #[must_use]
    pub fn glyph_bounds(&self, gid: u32) -> GlyphBBox {
        let mut b = self.font_glyph_bounds(gid);
        if self.extend != 0.0 {
            b.x_min *= self.extend;
            b.x_max *= self.extend;
        }
        b
    }

    /// `getGlyphHeightDepth`.
    #[must_use]
    pub fn glyph_height_depth(&self, gid: u32) -> (f32, f32) {
        let b = self.font_glyph_bounds(gid);
        (b.y_max, -b.y_min)
    }

    /// `getGlyphSidebearings(engine, …)`.
    #[must_use]
    pub fn glyph_sidebearings(&self, gid: u32) -> (f32, f32) {
        let width = self.glyph_width(gid);
        let b = self.font_glyph_bounds(gid);
        let (mut l, mut r) = (b.x_min, width - b.x_max);
        if self.extend != 0.0 {
            l *= self.extend;
            r *= self.extend;
        }
        (l, r)
    }

    /// `getGlyphItalCorr(engine, …)`.
    #[must_use]
    pub fn glyph_ital_corr(&self, gid: u32) -> f32 {
        let width = self.glyph_width(gid);
        let b = self.font_glyph_bounds(gid);
        let r = if b.x_max > width {
            b.x_max - width
        } else {
            0.0
        };
        self.extend * r
    }

    /// `mapchartoglyph`.
    #[must_use]
    pub fn map_char_to_glyph(&self, ch: i32) -> i32 {
        if ch > 0x10FFFF || (0xD800..=0xDFFF).contains(&ch) || ch < 0 {
            return 0;
        }
        self.face.char_index(ch as u32) as i32
    }

    /// `mapglyphtoindex`: the glyph named `name`, or 0.
    #[must_use]
    pub fn map_glyph_to_index(&self, name: &[u8]) -> i32 {
        self.face.glyph_index_by_name(name) as i32
    }

    /// `getfontcharrange`.
    #[must_use]
    pub fn font_char_range(&self, first: bool) -> i32 {
        let (f, l) = self.face.char_range();
        (if first { f } else { l }) as i32
    }

    /// `printglyphname`'s bytes. XeTeX prints each with `print_char` as a
    /// C `char`, so a byte from 0x80 up arrives sign-extended
    /// (`b as i8 as i32`).
    #[must_use]
    pub fn glyph_name(&self, gid: u32) -> Vec<u8> {
        self.face.glyph_name(gid).unwrap_or_default()
    }

    /// `otgetfontmetrics`: (ascent, descent, x-height, cap height, slant).
    #[must_use]
    pub fn font_metrics(&self) -> (Fixed, Fixed, Fixed, Fixed, Fixed) {
        let ascent = d2fix(f64::from(
            self.units_to_points(f32::from(self.face.ascender())),
        ));
        let descent = d2fix(f64::from(
            self.units_to_points(f32::from(self.face.descender())),
        ));
        let ital = fix2d(self.face.italic_angle().unwrap_or(0)) as f32;
        let font_slant = d2fix(libm::tan(f64::from(-ital) * core::f64::consts::PI / 180.0));
        let slant = d2fix(fix2d(font_slant) * f64::from(self.extend) + f64::from(self.slant));
        let (cap, xh) = match self.face.os2() {
            Some(o) => (
                self.units_to_points(f32::from(o.cap_height)),
                self.units_to_points(f32::from(o.x_height)),
            ),
            None => (0.0, 0.0),
        };
        let mut capheight = d2fix(f64::from(cap));
        let mut xheight = d2fix(f64::from(xh));
        if xheight == 0 {
            let g = self.face.char_index(u32::from(b'x'));
            xheight = if g != 0 {
                d2fix(f64::from(self.glyph_height_depth(g).0))
            } else {
                ascent / 2
            };
        }
        if capheight == 0 {
            let g = self.face.char_index(u32::from(b'X'));
            capheight = if g != 0 {
                d2fix(f64::from(self.glyph_height_depth(g).0))
            } else {
                ascent
            };
        }
        (ascent, descent, xheight, capheight, slant)
    }

    /// `getnativecharheightdepth`, snapped to the baseline, x-height and
    /// cap height (`\fontdimen5`, `6`, `8` of the TeX font: `quad`,
    /// `x_height`, `cap_height`) within 4% of the quad.
    #[must_use]
    pub fn char_height_depth(
        &self,
        ch: i32,
        quad: Fixed,
        x_height: Fixed,
        cap_height: Fixed,
    ) -> (Fixed, Fixed) {
        let gid = self.map_char_to_glyph(ch) as u32;
        let (ht, dp) = self.glyph_height_depth(gid);
        let mut height = d2fix(f64::from(ht));
        let mut depth = d2fix(f64::from(dp));
        let fuzz = quad / 25;
        let snap = |v: &mut Fixed, s: Fixed| {
            let d = *v - s;
            if d <= fuzz && d >= -fuzz {
                *v = s;
            }
        };
        snap(&mut depth, 0);
        snap(&mut height, 0);
        snap(&mut height, x_height);
        snap(&mut height, cap_height);
        (height, depth)
    }

    /// `getnativecharsidebearings`.
    #[must_use]
    pub fn char_sidebearings(&self, ch: i32) -> (Fixed, Fixed) {
        let gid = self.map_char_to_glyph(ch) as u32;
        let (l, r) = self.glyph_sidebearings(gid);
        (d2fix(f64::from(l)), d2fix(f64::from(r)))
    }

    /// `getnativecharwd`.
    #[must_use]
    pub fn char_wd(&self, ch: i32) -> Fixed {
        let gid = self.map_char_to_glyph(ch) as u32;
        d2fix(f64::from(self.extend * self.glyph_width(gid)))
    }

    /// `getnativecharic` (`letter_space`: the TeX font's
    /// `font_letter_space`).
    #[must_use]
    pub fn char_ic(&self, ch: i32, letter_space: Fixed) -> Fixed {
        let (_, rsb) = self.char_sidebearings(ch);
        if rsb < 0 {
            letter_space - rsb
        } else {
            letter_space
        }
    }

    /// `getglyphbounds`: edge 1–4 = left, top, right, bottom.
    #[must_use]
    pub fn glyph_bounds_edge(&self, edge: i32, gid: u32) -> Fixed {
        let (a, b) = if edge & 1 != 0 {
            self.glyph_sidebearings(gid)
        } else {
            self.glyph_height_depth(gid)
        };
        d2fix(f64::from(if edge <= 2 { a } else { b }))
    }

    /// `get_native_glyph_italic_correction` of a glyph node.
    #[must_use]
    pub fn glyph_italic_correction(&self, gid: u32) -> Fixed {
        d2fix(f64::from(self.glyph_ital_corr(gid)))
    }

    /// `get_native_italic_correction` of a word (its last glyph's, plus the
    /// TeX font's letter space).
    #[must_use]
    pub fn word_italic_correction(&self, glyphs: &[NativeGlyph], letter_space: Fixed) -> Fixed {
        match glyphs.last() {
            Some(g) => self.glyph_italic_correction(u32::from(g.gid)) + letter_space,
            None => 0,
        }
    }

    /// `measure_native_glyph`: (width, height, depth); height and depth only
    /// with `use_glyph_metrics` (else `None`: the font's height and depth
    /// bases).
    #[must_use]
    pub fn measure_glyph(
        &self,
        gid: u32,
        use_glyph_metrics: bool,
    ) -> (Fixed, Option<(Fixed, Fixed)>) {
        let wd = d2fix(f64::from(self.glyph_width(gid)));
        if use_glyph_metrics {
            let (ht, dp) = self.glyph_height_depth(gid);
            (wd, Some((d2fix(f64::from(ht)), d2fix(f64::from(dp)))))
        } else {
            (wd, None)
        }
    }

    /// `applymapping`: `text` through the font's mapping (unchanged
    /// without one; empty where TECkit fails).
    #[must_use]
    pub fn apply_mapping(&self, text: &[u16]) -> Vec<u16> {
        match &self.mapping {
            Some(m) => m.apply_utf16(text),
            None => text.to_vec(),
        }
    }

    /// `getDefaultDirection`: whether a word with no strong character is
    /// right to left (the last shaped buffer's script is).
    #[must_use]
    pub fn default_rtl(&self) -> bool {
        self.last_script.is_some_and(script_is_rtl)
    }

    fn shape_run(
        &self,
        shaper: &mut Shaper,
        text: &[u16],
        start: usize,
        len: usize,
        rtl: bool,
    ) -> (Arc<[crate::shape::ShapedGlyph]>, Option<Tag>) {
        let direction = if self.flags & FONT_FLAGS_VERTICAL != 0 {
            Direction::TopToBottom
        } else if rtl {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        let script = ot_tag_to_script(self.script);
        let req = ShapeRequest {
            text: text.to_vec(),
            start,
            len,
            direction,
            script: (script != 0).then_some(script),
            language: self.language.clone(),
            features: self.features.clone(),
        };
        let glyphs = shaper.shape(&self.face, &req);
        // The buffer's script after `hb_buffer_guess_segment_properties`.
        let script = req
            .script
            .filter(|&s| harfrust::Script::from_iso15924_tag(harfrust::Tag::from_u32(s)).is_some());
        let script = script.or_else(|| guess_script(&text[start..start + len]));
        (glyphs, script)
    }

    /// `getGlyphPositions` (with `getGlyphs`/`getGlyphAdvances`): the
    /// glyphs, each glyph's position and advance in points, and the pen
    /// position after the last.
    fn positions(&self, glyphs: &[crate::shape::ShapedGlyph]) -> (Vec<(f32, f32)>, Vec<f32>) {
        let vertical = self.flags & FONT_FLAGS_VERTICAL != 0;
        let mut pos = Vec::with_capacity(glyphs.len() + 1);
        let mut adv = Vec::with_capacity(glyphs.len());
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for g in glyphs {
            if vertical {
                pos.push((
                    -self.units_to_points(x + g.y_offset as f32),
                    self.units_to_points(y - g.x_offset as f32),
                ));
                x += g.y_advance as f32;
                y += g.x_advance as f32;
                adv.push(self.units_to_points(g.y_advance as f32));
            } else {
                pos.push((
                    self.units_to_points(x + g.x_offset as f32),
                    -self.units_to_points(y + g.y_offset as f32),
                ));
                x += g.x_advance as f32;
                y += g.y_advance as f32;
                adv.push(self.units_to_points(g.x_advance as f32));
            }
        }
        if vertical {
            pos.push((-self.units_to_points(x), self.units_to_points(y)));
        } else {
            pos.push((self.units_to_points(x), -self.units_to_points(y)));
        }
        if self.extend != 1.0 || self.slant != 0.0 {
            for p in &mut pos {
                p.0 = p.0 * self.extend - p.1 * self.slant;
            }
        }
        (pos, adv)
    }

    /// `measure_native_node`'s layout of `text` (already mapped): bidi runs,
    /// each shaped, positions in `Fixed`, letter spacing applied
    /// (`letter_space`: the TeX font's `font_letter_space`).
    /// `default_rtl` is [`XeTeXFont::default_rtl`]; the second result is
    /// the script the last run's buffer ended with (the next
    /// `last_script`).
    pub fn layout(
        &self,
        shaper: &mut Shaper,
        text: &[u16],
        default_rtl: bool,
        letter_space: Fixed,
    ) -> (NativeLayout, Option<Tag>) {
        let bidi = crate::bidi::analyze(text, default_rtl);
        let mut out = NativeLayout::default();
        let mut last_script = self.last_script;
        match bidi.direction {
            crate::bidi::BidiDirection::Mixed => {
                let (mut x, mut y) = (0.0f64, 0.0f64);
                for run in &bidi.runs {
                    let (glyphs, s) = self.shape_run(shaper, text, run.start, run.len, run.rtl);
                    last_script = s;
                    let (pos, adv) = self.positions(&glyphs);
                    for (i, g) in glyphs.iter().enumerate() {
                        out.glyphs.push(NativeGlyph {
                            gid: g.gid as u16,
                            x: d2fix(f64::from(pos[i].0) + x),
                            y: d2fix(f64::from(pos[i].1) + y),
                            advance: d2fix(f64::from(adv[i])),
                        });
                    }
                    x += f64::from(pos[glyphs.len()].0);
                    y += f64::from(pos[glyphs.len()].1);
                }
                out.width = if out.glyphs.is_empty() { 0 } else { d2fix(x) };
            }
            dir => {
                let rtl = dir == crate::bidi::BidiDirection::Rtl;
                let (glyphs, s) = self.shape_run(shaper, text, 0, text.len(), rtl);
                last_script = s;
                let (pos, adv) = self.positions(&glyphs);
                for (i, g) in glyphs.iter().enumerate() {
                    out.glyphs.push(NativeGlyph {
                        gid: g.gid as u16,
                        x: d2fix(f64::from(pos[i].0)),
                        y: d2fix(f64::from(pos[i].1)),
                        advance: d2fix(f64::from(adv[i])),
                    });
                }
                out.width = if glyphs.is_empty() {
                    0
                } else {
                    d2fix(f64::from(pos[glyphs.len()].0))
                };
            }
        }
        if letter_space != 0 {
            let mut delta: Fixed = 0;
            for g in &mut out.glyphs {
                if g.advance == 0 && delta != 0 {
                    delta -= letter_space;
                }
                g.x += delta;
                delta += letter_space;
            }
            if delta != 0 {
                delta -= letter_space;
                out.width += delta;
            }
        }
        (out, last_script)
    }

    /// [`XeTeXFont::layout`] keeping the default direction's state in the
    /// font, as XeTeX does.
    pub fn layout_word(
        &mut self,
        shaper: &mut Shaper,
        text: &[u16],
        letter_space: Fixed,
    ) -> NativeLayout {
        let (l, s) = self.layout(shaper, text, self.default_rtl(), letter_space);
        self.last_script = s;
        l
    }

    /// `measure_native_node`'s height and depth with
    /// `\XeTeXuseglyphmetrics`: from the glyphs' bounds (`None` for no
    /// glyphs: the font's height and depth bases).
    #[must_use]
    pub fn layout_height_depth(&self, glyphs: &[NativeGlyph]) -> Option<(Fixed, Fixed)> {
        if glyphs.is_empty() {
            return None;
        }
        let (mut y_min, mut y_max) = (65536.0f32, -65536.0f32);
        for g in glyphs {
            let y = fix2d(-g.y) as f32;
            let b = self.glyph_bounds(u32::from(g.gid));
            let (ht, dp) = (b.y_max, -b.y_min);
            if y + ht > y_max {
                y_max = y + ht;
            }
            if y - dp < y_min {
                y_min = y - dp;
            }
        }
        Some((d2fix(f64::from(y_max)), -d2fix(f64::from(y_min))))
    }

    /// `store_justified_native_glyphs`: the glyphs of a word measured at
    /// `width` stretched to `saved_width` (on spaces, or else on every
    /// glyph).
    pub fn justify(&self, layout: &mut NativeLayout, saved_width: Fixed) {
        if layout.width == saved_width {
            return;
        }
        let just = fix2d(saved_width - layout.width);
        let space = self.map_char_to_glyph(i32::from(b' '));
        let n = layout.glyphs.len();
        let spaces = layout
            .glyphs
            .iter()
            .filter(|g| i32::from(g.gid) == space)
            .count();
        if spaces > 0 {
            let mut adjustment = 0.0f64;
            let mut k = 0;
            for g in &mut layout.glyphs {
                g.x = d2fix(fix2d(g.x) + adjustment);
                if i32::from(g.gid) == space {
                    k += 1;
                    adjustment = just * k as f64 / spaces as f64;
                }
            }
        } else {
            for (i, g) in layout.glyphs.iter_mut().enumerate().skip(1) {
                g.x = d2fix(fix2d(g.x) + just * i as f64 / (n - 1) as f64);
            }
        }
        layout.width = saved_width;
    }

    /// `makefontdef`'s bytes (after the font number): size, flags, path,
    /// face index, then color, extend, slant, embolden as the flags say.
    #[must_use]
    pub fn font_def(&self) -> Vec<u8> {
        let mut flags: u16 = 0;
        if self.flags & FONT_FLAGS_VERTICAL != 0 {
            flags |= 0x0100;
        }
        if self.flags & FONT_FLAGS_COLORED != 0 {
            flags |= 0x0200;
        }
        if self.extend != 1.0 {
            flags |= 0x1000;
        }
        if self.slant != 0.0 {
            flags |= 0x2000;
        }
        if self.embolden != 0.0 {
            flags |= 0x4000;
        }
        let path = self.path.as_bytes();
        let mut v = Vec::with_capacity(11 + path.len() + 16);
        v.extend_from_slice(&d2fix(f64::from(self.point_size)).to_be_bytes());
        v.extend_from_slice(&flags.to_be_bytes());
        v.push(path.len() as u8);
        v.extend_from_slice(path);
        v.extend_from_slice(&self.index.to_be_bytes());
        if flags & 0x0200 != 0 {
            v.extend_from_slice(&self.rgba.to_be_bytes());
        }
        if flags & 0x1000 != 0 {
            v.extend_from_slice(&d2fix(f64::from(self.extend)).to_be_bytes());
        }
        if flags & 0x2000 != 0 {
            v.extend_from_slice(&d2fix(f64::from(self.slant)).to_be_bytes());
        }
        if flags & 0x4000 != 0 {
            v.extend_from_slice(&d2fix(f64::from(self.embolden)).to_be_bytes());
        }
        v
    }

    /// `isOpenTypeMathFont`.
    #[must_use]
    pub fn is_math_font(&self) -> bool {
        self.face
            .table(tag(b"MATH"))
            .is_some_and(|d| crate::face::rd_u32(d, 0).is_some_and(|v| v != 0))
    }

    /// `get_ot_math_constant(f, n)`.
    #[must_use]
    pub fn ot_math_constant(&self, n: u32) -> Fixed {
        let Some(m) = Math::new(&self.face) else {
            return 0;
        };
        let v = m.constant(n);
        match n {
            0 | 1 | 55 => v,
            _ => d2fix(f64::from(self.units_to_points(v as f32))),
        }
    }

    /// `load_native_font`'s fontdimens (and height and depth bases):
    /// `space_width` is the width of a native word of one space measured
    /// in this font (`new_native_character(f, " ")`), `font_size` its
    /// `font_size`.
    #[must_use]
    pub fn native_params(&self, space_width: Fixed, font_size: Fixed) -> NativeParams {
        let (ascent, descent, x_ht, cap_ht, font_slant) = self.font_metrics();
        let s = space_width + self.letter_space;
        let mut params = alloc::vec![font_slant, s, s / 2, s / 3, x_ht, font_size, s / 3, cap_ht];
        if self.is_math_font() {
            params.push(10 + crate::math::LAST_CONSTANT as i32);
            for k in 0..=crate::math::LAST_CONSTANT {
                params.push(self.ot_math_constant(k));
            }
        }
        NativeParams {
            params,
            height_base: ascent,
            depth_base: -descent,
        }
    }

    /// `get_native_mathsy_param(f, n)` (`font_size`: the TeX font's).
    #[must_use]
    pub fn mathsy_param(&self, n: i32, font_size: Fixed) -> Fixed {
        match n {
            6 => font_size,
            21 => {
                let d1 = self.mathsy_param(20, font_size);
                (1.5 * f64::from(font_size)).min(f64::from(d1)) as i32
            }
            _ => match sy_constant(n) {
                Some(c) => self.ot_math_constant(c),
                None => 0,
            },
        }
    }

    /// `get_native_mathex_param(f, n)`.
    #[must_use]
    pub fn mathex_param(&self, n: i32, font_size: Fixed) -> Fixed {
        match n {
            6 => font_size,
            _ => match ex_constant(n) {
                Some(c) => self.ot_math_constant(c),
                None => 0,
            },
        }
    }

    /// `get_ot_math_variant(f, g, v, &adv, horiz)`: (glyph, advance), the
    /// glyph itself and -1 without variant `v`.
    #[must_use]
    pub fn ot_math_variant(&self, g: u32, v: u32, horizontal: bool) -> (u32, Fixed) {
        let Some(m) = Math::new(&self.face) else {
            return (g, -1);
        };
        match m.variants(g, horizontal).get(v as usize) {
            Some(&(gl, adv)) => (gl, d2fix(f64::from(self.units_to_points(adv as f32)))),
            None => (g, -1),
        }
    }

    /// `get_ot_assembly_ptr` and the `ot_part_*` accessors: the parts with
    /// connector lengths and advances in `Fixed`; `None` without one.
    #[must_use]
    pub fn ot_assembly(&self, g: u32, horizontal: bool) -> Option<Vec<GlyphPart>> {
        let m = Math::new(&self.face)?;
        let (parts, _) = m.assembly(g, horizontal);
        if parts.is_empty() {
            return None;
        }
        let f = |v: i32| d2fix(f64::from(self.units_to_points(v as f32)));
        Some(
            parts
                .into_iter()
                .map(|p| GlyphPart {
                    glyph: p.glyph,
                    start_connector_length: f(p.start_connector_length),
                    end_connector_length: f(p.end_connector_length),
                    full_advance: f(p.full_advance),
                    extender: p.extender,
                })
                .collect(),
        )
    }

    /// `get_ot_math_ital_corr`.
    #[must_use]
    pub fn ot_math_ital_corr(&self, g: u32) -> Fixed {
        let v = Math::new(&self.face).map_or(0, |m| m.italics_correction(g));
        d2fix(f64::from(self.units_to_points(v as f32)))
    }

    /// `get_ot_math_accent_pos`.
    #[must_use]
    pub fn ot_math_accent_pos(&self, g: u32) -> Fixed {
        let v = match Math::new(&self.face) {
            Some(m) => m.top_accent_attachment(g),
            None => self.face.h_advance(g).unwrap_or(0) as i32 / 2,
        };
        d2fix(f64::from(self.units_to_points(v as f32)))
    }

    /// `ot_min_connector_overlap`.
    #[must_use]
    pub fn ot_min_connector_overlap(&self) -> Fixed {
        let v = Math::new(&self.face).map_or(0, |m| m.min_connector_overlap());
        d2fix(f64::from(self.units_to_points(v as f32)))
    }

    fn math_kern_at(&self, g: u32, height: i32, side: KernSide) -> i32 {
        Math::new(&self.face).map_or(0, |m| m.kerning(g, side, height))
    }

    /// `get_ot_math_kern(f, g, sf, sg, cmd, shift)`.
    #[must_use]
    pub fn ot_math_kern(
        &self,
        g: u32,
        sfont: &XeTeXFont,
        sg: u32,
        cmd: ScriptKind,
        shift: Fixed,
    ) -> Fixed {
        let g_height = self.points_to_units(self.glyph_height_depth(g).0) as i32;
        let g_depth = self.points_to_units(self.glyph_height_depth(g).1) as i32;
        let sg_height = sfont.points_to_units(sfont.glyph_height_depth(sg).0) as i32;
        let sg_depth = sfont.points_to_units(sfont.glyph_height_depth(sg).1) as i32;
        let shift = self.points_to_units(fix2d(shift) as f32) as i32;
        let scale = sfont.point_size / self.point_size;
        let rval = match cmd {
            ScriptKind::Sup => {
                let kern = self.math_kern_at(
                    g,
                    (shift as f32 - scale * sg_depth as f32) as i32,
                    KernSide::TopRight,
                );
                let skern = sfont.math_kern_at(sg, -sg_depth, KernSide::BottomLeft);
                let top = (kern as f32 + scale * skern as f32) as i32;
                let kern = self.math_kern_at(g, g_height, KernSide::TopRight);
                let skern = sfont.math_kern_at(
                    sg,
                    ((g_height - shift) as f32 / scale) as i32,
                    KernSide::BottomLeft,
                );
                let bot = (kern as f32 + scale * skern as f32) as i32;
                top.max(bot)
            }
            ScriptKind::Sub => {
                let kern = self.math_kern_at(
                    g,
                    (scale * sg_height as f32 - shift as f32) as i32,
                    KernSide::BottomRight,
                );
                let skern = sfont.math_kern_at(sg, sg_height, KernSide::TopLeft);
                let top = (kern as f32 + scale * skern as f32) as i32;
                let kern = self.math_kern_at(g, -g_depth, KernSide::BottomRight);
                let skern = sfont.math_kern_at(
                    sg,
                    ((shift - g_depth) as f32 / scale) as i32,
                    KernSide::TopLeft,
                );
                let bot = (kern as f32 + scale * skern as f32) as i32;
                top.max(bot)
            }
        };
        d2fix(f64::from(self.units_to_points(rval as f32)))
    }

    /// `getLargerScriptListTable`, with its quirk: GPOS's list is read
    /// from GSUB, its count capped at GSUB's (the call's count is in and
    /// out), so the answer is always GSUB's script list.
    fn larger_script_list(&self) -> Vec<Tag> {
        layout::script_tags(&self.face, Table::Gsub)
    }

    /// `otfontget(what)`.
    #[must_use]
    pub fn ot_font_get(&self, what: i32) -> i32 {
        match what {
            what::COUNT_GLYPHS => self.face.num_glyphs() as u16 as i32,
            what::OT_COUNT_SCRIPTS => self.larger_script_list().len() as i32,
            _ => 0,
        }
    }

    /// `otfontget1(what, param)`.
    #[must_use]
    pub fn ot_font_get1(&self, what: i32, param: i32) -> i32 {
        match what {
            what::OT_COUNT_LANGUAGES => {
                let list = self.larger_script_list();
                match list.iter().position(|&s| s == param as u32) {
                    Some(i) => {
                        (layout::language_tags(&self.face, Table::Gsub, i as u32).len()
                            + layout::language_tags(&self.face, Table::Gpos, i as u32).len())
                            as i32
                    }
                    None => 0,
                }
            }
            what::OT_SCRIPT_CODE => {
                let list = self.larger_script_list();
                list.get(param as u32 as usize).map_or(0, |&t| t as i32)
            }
            what::IS_EXCLUSIVE_FEATURE => 1,
            _ => 0,
        }
    }

    /// `otfontget2(what, p1, p2)`.
    #[must_use]
    pub fn ot_font_get2(&self, what: i32, p1: i32, p2: i32) -> i32 {
        match what {
            what::OT_LANGUAGE_CODE => {
                let list = self.larger_script_list();
                let index = p2 as u32 as usize;
                for (i, &s) in list.iter().enumerate() {
                    if s == p1 as u32 {
                        let sub = layout::language_tags(&self.face, Table::Gsub, i as u32);
                        if index < sub.len() {
                            return sub[index] as i32;
                        }
                        let pos = layout::language_tags(&self.face, Table::Gpos, i as u32);
                        if index < pos.len() {
                            return pos[index] as i32;
                        }
                    }
                }
                0
            }
            what::OT_COUNT_FEATURES => {
                let mut n = 0;
                for t in [Table::Gsub, Table::Gpos] {
                    if let Some(f) = self.lang_features(t, p1 as u32, p2 as u32) {
                        n += f.len();
                    }
                }
                n as i32
            }
            what::IS_DEFAULT_SELECTOR => i32::from(p2 == 0),
            _ => 0,
        }
    }

    /// `otfontget3(what, p1, p2, p3)`.
    #[must_use]
    pub fn ot_font_get3(&self, what: i32, p1: i32, p2: i32, p3: i32) -> i32 {
        if what != what::OT_FEATURE_CODE {
            return 0;
        }
        let mut index = p3 as u32 as usize;
        for t in [Table::Gsub, Table::Gpos] {
            if let Some(f) = self.lang_features(t, p1 as u32, p2 as u32) {
                if index < f.len() {
                    return f[index] as i32;
                }
                index -= f.len();
            }
        }
        0
    }

    /// The features `countFeatures`/`getIndFeature` read from one table.
    fn lang_features(&self, t: Table, script: Tag, language: Tag) -> Option<Vec<Tag>> {
        let (found, si) = layout::find_script(&self.face, t, script);
        if !found {
            return None;
        }
        let (lfound, li) = layout::find_language(&self.face, t, si, language);
        if !lfound && language != 0 {
            return None;
        }
        Some(layout::feature_tags(&self.face, t, si, li))
    }
}

/// `TeX_sym_to_OT_map`: fontdimen `n` of a math symbols font →
/// `hb_ot_math_constant_t`.
fn sy_constant(n: i32) -> Option<u32> {
    Some(match n {
        5 => 6,        // ACCENT_BASE_HEIGHT
        8 => 33,       // FRACTION_NUMERATOR_DISPLAY_STYLE_SHIFT_UP
        9 => 32,       // FRACTION_NUMERATOR_SHIFT_UP
        10 => 22,      // STACK_TOP_SHIFT_UP
        11 => 35,      // FRACTION_DENOMINATOR_DISPLAY_STYLE_SHIFT_DOWN
        12 => 34,      // FRACTION_DENOMINATOR_SHIFT_DOWN
        13 | 14 => 11, // SUPERSCRIPT_SHIFT_UP
        15 => 12,      // SUPERSCRIPT_SHIFT_UP_CRAMPED
        16 | 17 => 8,  // SUBSCRIPT_SHIFT_DOWN
        18 => 14,      // SUPERSCRIPT_BASELINE_DROP_MAX
        19 => 10,      // SUBSCRIPT_BASELINE_DROP_MIN
        20 => 2,       // DELIMITED_SUB_FORMULA_MIN_HEIGHT
        22 => 5,       // AXIS_HEIGHT
        _ => return None,
    })
}

/// `TeX_ext_to_OT_map`.
fn ex_constant(n: i32) -> Option<u32> {
    Some(match n {
        5 => 6,   // ACCENT_BASE_HEIGHT
        8 => 38,  // FRACTION_RULE_THICKNESS
        9 => 18,  // UPPER_LIMIT_GAP_MIN
        10 => 20, // LOWER_LIMIT_GAP_MIN
        11 => 19, // UPPER_LIMIT_BASELINE_RISE_MIN
        12 => 21, // LOWER_LIMIT_BASELINE_DROP_MIN
        13 => 26, // STACK_GAP_MIN
        _ => return None,
    })
}

/// `hb_buffer_guess_segment_properties`' script: the first character's
/// that is not Common, Inherited or Unknown.
fn guess_script(text: &[u16]) -> Option<Tag> {
    let mut b = harfrust::Buffer::new();
    for r in char::decode_utf16(text.iter().copied()) {
        b.push(u32::from(r.unwrap_or('\u{FFFD}')), 0);
    }
    b.set_direction(harfrust::Direction::LeftToRight);
    b.guess_segment_properties();
    b.script()
        .map(|s| u32::from_be_bytes(s.tag().to_be_bytes()))
}

/// `hb_script_get_horizontal_direction(s) == HB_DIRECTION_RTL`.
#[must_use]
pub fn script_is_rtl(s: Tag) -> bool {
    const RTL: [&[u8; 4]; 36] = [
        b"Arab", b"Hebr", b"Syrc", b"Thaa", b"Cprt", b"Khar", b"Phnx", b"Nkoo", b"Lydi", b"Avst",
        b"Armi", b"Phli", b"Prti", b"Sarb", b"Orkh", b"Samr", b"Mand", b"Merc", b"Mero", b"Mani",
        b"Mend", b"Nbat", b"Narb", b"Palm", b"Phlp", b"Hatr", b"Adlm", b"Rohg", b"Sogo", b"Sogd",
        b"Elym", b"Chrs", b"Yezi", b"Ougr", b"Gara", b"Sidt",
    ];
    RTL.iter().any(|t| tag(t) == s)
}
