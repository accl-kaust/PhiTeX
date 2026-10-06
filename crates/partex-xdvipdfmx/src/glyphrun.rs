//! Glyph runs: a side output of [`Dpx::dvi_do_page`], every glyph a page
//! draws, in the order the content stream shows them, with what a viewer
//! needs to draw it again and to copy its text. Nothing here writes to
//! the PDF: the runs are made from the state the driver keeps anyway, and
//! a font's tables are read again on the side (never an error: a table
//! that cannot be read leaves its fields empty).
//!
//! **One run per glyph.** Each `set_string` of a glyph is a run: each
//! glyph of a native word (`set_glyphs`, `set_text_and_glyphs`), each
//! character of a TFM font (`set_char`, `set`, `put`), a virtual font's
//! characters expanded to the glyphs of its base fonts as its packets set
//! them (so one DVI character may be several runs, or none). Rules and
//! specials make none. (Glyphs drawn into a form, `pdf:bxobj` to
//! `pdf:exobj`, are runs of the page being read, placed in the form's
//! space.)
//!
//! **Where.** The run's `x`, `y` are its glyph's origin on the page, in
//! bp, in the page's default user space (y up, from the media box's lower
//! left corner): the device position (the DVI position, `\mag` aside)
//! with the current transformation matrix `ctm` applied, so
//! `(x, y) = ctm·(xd, yd)`, `ctm = [a b c d e f]` mapping `(p, q)` to
//! `(a·p + c·q + e, b·p + d·q + f)`, as PDF's `cm` does. `ctm` is the
//! driver's (`pdf_dev_currentmatrix`): the page's own (`\mag`, the
//! origin offset), then what `pdf:btrans`/`bt`, `x:rotate`, `x:scale`
//! and graphicx's `\rotatebox`/`\scalebox` concatenate; a `cm` inside a
//! literal (`pdf:literal`, `pdf:code`) is not known to the driver, and is
//! not in it. A point `(u, v)` of the glyph's outline, in em (font units
//! over units per em), lands at
//! `(x, y) + L·(size·Tm·(u, v))`, `L = [a c; b d]` (the linear part of
//! `ctm`), `Tm = [tm0 tm2; tm1 tm3]` (the text matrix's linear part:
//! `[extend 0 slant 1]` for horizontal text, the map entry's or the
//! native font's `ExtendFont`/`SlantFont`), `size` the font's size in the
//! space `ctm` maps (the `Tf` operand). With no transformation (`ctm`
//! `[m 0 0 m e f]`, `\mag` `m`) the glyph is upright, `m·size` bp.
//! `(xd, yd)` is TeX's position for the glyph; the content stream starts
//! each string there (`Tm`/`Td` under the same `cm`s, to the driver's
//! precision, 0.001 bp), and a PDF viewer advances through a string by
//! the font's `/Widths`, which xdvipdfmx takes from the font file (a
//! Type 1 font's charstrings, whole units) while it places by the TFM's:
//! a string's later glyphs show in the PDF off their runs by the sum of
//! those differences, under a thousandth of an em a glyph (0.06 bp after
//! eight `cmr10` periods; none for native fonts, whose widths are the
//! shaper's).
//!
//! **Colour.** `color` is the fill colour of the graphics state at the
//! glyph: the colour the content stream sets for it, as `color push`
//! (xcolor, `\textcolor`), `pdf:bcolor`/`pdf:bc` and the colour stack's
//! pops leave it (the colour stack's top, `pdf_color_get_current`, unless
//! a `Q` restored an older one); a native font's own colour (fontspec's
//! `Color=`) wins, pushed around its glyphs as the driver does. `rgba` is
//! it as a viewer paints it, with the native font's alpha.
//!
//! **Text.** Runs are grouped into clusters (`cluster`, numbered on the
//! page from 0 in the order of their first run): a native word shown
//! with its text (`\XeTeXgenerateactualtext`, `set_text_and_glyphs`) is
//! one cluster, its text (in logical order: the `/ActualText` the PDF
//! gets) on its first run, `actual_text` set; any other glyph is a
//! cluster by itself, its text what the font's `ToUnicode` CMap maps it
//! to: a native font's (`otf_create_ToUnicode_stream`: its `cmap`
//! inverted, then GSUB's substitutions and ligatures, glyph names and
//! presentation forms), a TFM font's glyph name through the AGL (its
//! encoding's `ToUnicode`, or the built-in encoding's). Glyphs of
//! right-to-left text come in visual order, each with its own
//! characters, as a PDF viewer reads them from the content stream: a
//! viewer rebuilds the logical order with the bidi algorithm, as PDF
//! viewers do. (A PDF drops a TFM font's whole `ToUnicode` when one of
//! its glyphs has no Unicode name; its viewers then read the names
//! themselves.)

use alloc::collections::BTreeMap;

use crate::pdfcolor::{
    PDF_COLORSPACE_TYPE_CMYK, PDF_COLORSPACE_TYPE_GRAY, PDF_COLORSPACE_TYPE_RGB,
    PDF_COLORSPACE_TYPE_SPOT, PdfColor,
};
use crate::pdfdev::Spt;
use crate::pdffont::{
    CIDFONT_FLAG_TYPE1, PDF_FONT_FLAG_BASEFONT, PDF_FONT_FLAG_IS_ALIAS, PDF_FONT_FONTTYPE_CIDTYPE2,
    PDF_FONT_FONTTYPE_TRUETYPE, PDF_FONT_FONTTYPE_TYPE0, PDF_FONT_FONTTYPE_TYPE1,
    PDF_FONT_FONTTYPE_TYPE1C,
};
use crate::prelude::*;

/// Where a glyph's shape comes from: a font file and the glyph in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlyphSource {
    /// A native font's glyph (`XDV_GLYPHS`): glyph `gid` of face
    /// `face_index` of `font_file` (the path the driver opened).
    Native {
        font_file: Vec<u8>,
        face_index: u32,
        gid: u16,
    },
    /// Character `code` of a TFM font drawn with Type 1 font `font_file`
    /// (where the map entry's file was found; a base 14 font's name when
    /// the entry names no file): glyph `glyph_name`, the name the map
    /// entry's encoding (`.enc`) gives `code`, else the font's built-in
    /// encoding (`.notdef` where it gives none).
    Type1 {
        font_file: Vec<u8>,
        glyph_name: Vec<u8>,
        code: u8,
    },
    /// Character `code`'s glyph of a TrueType font (a `.ttf` map entry):
    /// the glyph the driver draws, its name in the entry's encoding through
    /// `post`, the Unicode `cmap` and GSUB, with no encoding the code in
    /// the Mac Roman `cmap`; 0 (`.notdef`) where none is found.
    TrueType {
        font_file: Vec<u8>,
        face_index: u32,
        gid: u16,
    },
    /// Character `code`'s glyph of a CFF OpenType font (a `.otf` map
    /// entry): the glyph its name (the entry's encoding's, else the CFF's
    /// built-in encoding's) has in the CFF's charset; 0 where none.
    OpenType {
        font_file: Vec<u8>,
        face_index: u32,
        gid: u16,
    },
    /// A TFM font drawn some other way (a composite font through a CMap):
    /// the font file the map entry names and the code shown.
    Other { font_file: Vec<u8>, code: u32 },
}

/// A fill colour as the content stream sets it, in its colour space.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorSpec {
    /// `g`: gray, 0 black to 1 white.
    Gray(f64),
    /// `rg`.
    Rgb([f64; 3]),
    /// `k`.
    Cmyk([f64; 4]),
    /// A separation (`pdf:bc` with a spot colour): its name and tint.
    Spot { name: Option<String>, tint: f64 },
    /// Any other colour space (one from a resource): its components.
    Other(Vec<f64>),
}

impl ColorSpec {
    /// The colour as RGBA (`0xRRGGBBAA`), alpha `alpha`, as PDF viewers
    /// paint it without colour management: gray as its level on each
    /// channel; CMYK as `r = 1 - min(1, c + k)`, `g = 1 - min(1, m + k)`,
    /// `b = 1 - min(1, y + k)`; a spot colour as gray `1 - tint` (its
    /// alternate space is not followed); another space by its count of
    /// components (1 gray, 3 RGB, 4 CMYK), else black.
    #[must_use]
    pub fn rgba(&self, alpha: u8) -> u32 {
        let cmyk = |c: f64, m: f64, y: f64, k: f64| {
            [
                1.0 - (c + k).min(1.0),
                1.0 - (m + k).min(1.0),
                1.0 - (y + k).min(1.0),
            ]
        };
        let [r, g, b] = match self {
            ColorSpec::Gray(v) => [*v; 3],
            ColorSpec::Rgb(v) => *v,
            ColorSpec::Cmyk([c, m, y, k]) => cmyk(*c, *m, *y, *k),
            ColorSpec::Spot { tint, .. } => [1.0 - tint; 3],
            ColorSpec::Other(v) => match v[..] {
                [g] => [g; 3],
                [r, g, b] => [r, g, b],
                [c, m, y, k] => cmyk(c, m, y, k),
                _ => [0.0; 3],
            },
        };
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        (byte(r) << 24) | (byte(g) << 16) | (byte(b) << 8) | u32::from(alpha)
    }

    /// `color`'s space and components.
    fn of(color: &PdfColor) -> ColorSpec {
        let v = &color.values;
        match color.r#type {
            PDF_COLORSPACE_TYPE_GRAY => ColorSpec::Gray(v[0]),
            PDF_COLORSPACE_TYPE_RGB => ColorSpec::Rgb([v[0], v[1], v[2]]),
            PDF_COLORSPACE_TYPE_CMYK => ColorSpec::Cmyk([v[0], v[1], v[2], v[3]]),
            PDF_COLORSPACE_TYPE_SPOT => ColorSpec::Spot {
                name: color
                    .spot_color_name
                    .as_deref()
                    .map(|n| String::from_utf8_lossy(n).into_owned()),
                tint: v[0],
            },
            _ => {
                let n = usize::try_from(color.num_components)
                    .unwrap_or(0)
                    .min(v.len());
                ColorSpec::Other(v[..n].to_vec())
            }
        }
    }
}

/// A glyph a page draws (see the [module](self)'s documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    /// The font file and the glyph in it.
    pub source: GlyphSource,
    /// The glyph's origin on the page (bp, default user space, y up),
    /// `ctm` applied.
    pub x: f64,
    pub y: f64,
    /// The font's size (bp) in the space `ctm` maps to the page.
    pub size: f64,
    /// The current transformation matrix `[a b c d e f]`.
    pub ctm: [f64; 6],
    /// The text matrix's linear part `[a b c d]`, per unit of `size`.
    pub tm: [f64; 4],
    /// The fill colour, as RGBA (`0xRRGGBBAA`).
    pub rgba: u32,
    /// The fill colour, in its space.
    pub color: ColorSpec,
    /// The text of the run's cluster, on its first run: the cluster's
    /// `/ActualText`, or the glyph's `ToUnicode` text; none on the other
    /// runs of a cluster, or for a glyph that maps to none.
    pub text: Option<String>,
    /// The run's cluster, numbered on the page from 0.
    pub cluster: u32,
    /// The cluster's text is its `/ActualText` (logical order, the whole
    /// cluster's), not one glyph's `ToUnicode` text.
    pub actual_text: bool,
}

/// What a TFM font's characters are drawn with, worked out at its first
/// glyph (by code: 256 names, glyph ids and texts; empty where none).
#[derive(Clone, Debug)]
struct RunFont {
    /// 1 Type 1, 2 TrueType, 3 OpenType, 0 other.
    kind: u8,
    file: Vec<u8>,
    face: u32,
    names: Vec<Option<Vec<u8>>>,
    gids: Vec<u16>,
    texts: Vec<Option<String>>,
}

/// An `/ActualText` span being drawn: its cluster, and its text until its
/// first run takes it.
#[derive(Clone, Debug)]
struct Span {
    cluster: u32,
    text: Option<String>,
}

/// The glyph runs' state.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The page's runs so far.
    pub runs: Vec<GlyphRun>,
    /// The page's next cluster.
    next_cluster: u32,
    span: Option<Span>,
    /// By PDF font id.
    fonts: BTreeMap<i32, Rc<RunFont>>,
    /// A native font's `ToUnicode` texts by glyph id, by PDF font id.
    native: BTreeMap<i32, Rc<Vec<Option<String>>>>,
}

impl State {
    /// A page begins: no runs yet.
    pub fn page_begin(&mut self) {
        self.runs.clear();
        self.next_cluster = 0;
        self.span = None;
    }
}

impl Dpx {
    /// `set_text_and_glyphs`' glyphs begin: one cluster, text `unicodes`
    /// (UTF-16).
    pub(crate) fn glyph_runs_span(&mut self, unicodes: &[u16]) {
        let r = &mut self.runs;
        r.span = Some(Span {
            cluster: r.next_cluster,
            text: Some(String::from_utf16_lossy(unicodes)),
        });
        r.next_cluster += 1;
    }

    /// The span's glyphs end.
    pub(crate) fn glyph_runs_span_end(&mut self) {
        self.runs.span = None;
    }

    /// `set_string` has drawn glyph `code` of loaded font `lf` (a native
    /// font's glyph id, else a TFM character code) at DVI position
    /// (`x`, `y`), y up (the compensation not taken off yet); `alpha` the
    /// native font's.
    pub(crate) fn record_glyph_run(&mut self, lf: usize, code: u32, x: Spt, y: Spt, alpha: u8) {
        let (ftype, dev_id) = {
            let f = &self.dvi.loaded_fonts[lf];
            (f.type_, f.font_id)
        };
        let Some(dev) = usize::try_from(dev_id)
            .ok()
            .and_then(|i| self.dev.pdev.fonts.get(i))
        else {
            return;
        };
        let (pdf_id, sptsize) = (dev.font_id, dev.sptsize);
        let d = self.dvi.dvi2pts;
        let xd = f64::from(x.wrapping_sub(self.dvi.compensation.x)) * d;
        let yd = f64::from(y.wrapping_sub(self.dvi.compensation.y)) * d;
        let (_, m) = self.pdf_dev_currentmatrix();
        let ctm = [m.a, m.b, m.c, m.d, m.e, m.f];
        let tmx = self.dev.pdev.text_state.matrix;
        let tm = crate::pdfdev::text_matrix(tmx.slant, tmx.extend, tmx.rotate);
        let color = ColorSpec::of(&self.pdfdraw_gs().fillcolor);
        let (source, glyph_text) = if ftype == crate::dvi::NATIVE {
            let f = &self.dvi.loaded_fonts[lf];
            let source = GlyphSource::Native {
                font_file: f.native_path.clone(),
                face_index: f.face_index,
                gid: code as u16,
            };
            let text = if self.runs.span.is_some() {
                None
            } else {
                self.native_text(pdf_id, code as u16)
            };
            (source, text)
        } else {
            let f = self.run_font(pdf_id);
            let c = code as usize;
            let name = || {
                f.names
                    .get(c)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| b".notdef".to_vec())
            };
            let gid = f.gids.get(c).copied().unwrap_or(0);
            let source = match f.kind {
                1 if code < 256 => GlyphSource::Type1 {
                    font_file: f.file.clone(),
                    glyph_name: name(),
                    code: code as u8,
                },
                2 if code < 256 => GlyphSource::TrueType {
                    font_file: f.file.clone(),
                    face_index: f.face,
                    gid,
                },
                3 if code < 256 => GlyphSource::OpenType {
                    font_file: f.file.clone(),
                    face_index: f.face,
                    gid,
                },
                _ => GlyphSource::Other {
                    font_file: f.file.clone(),
                    code,
                },
            };
            (source, f.texts.get(c).cloned().flatten())
        };
        let r = &mut self.runs;
        let (cluster, text, actual_text) = match &mut r.span {
            Some(s) => (s.cluster, s.text.take(), true),
            None => {
                r.next_cluster += 1;
                (r.next_cluster - 1, glyph_text, false)
            }
        };
        r.runs.push(GlyphRun {
            source,
            x: m.a * xd + m.c * yd + m.e,
            y: m.b * xd + m.d * yd + m.f,
            size: f64::from(sptsize) * d,
            ctm,
            tm,
            rgba: color.rgba(alpha),
            color,
            text,
            cluster,
            actual_text,
        });
    }

    /// The text native font `pdf_id`'s `ToUnicode` CMap gives glyph `gid`.
    fn native_text(&mut self, pdf_id: i32, gid: u16) -> Option<String> {
        let table = match self.runs.native.get(&pdf_id) {
            Some(t) => t.clone(),
            None => {
                let t = Rc::new(
                    self.native_tounicode(pdf_id)
                        .ok()
                        .flatten()
                        .unwrap_or_default(),
                );
                self.runs.native.insert(pdf_id, t.clone());
                t
            }
        };
        table.get(usize::from(gid)).cloned().flatten()
    }

    /// `Type0Font_attach_ToUnicode_stream`'s choice for native font
    /// `pdf_id`, as texts by glyph id: the OpenType font's own CMap (an
    /// Adobe-Identity TrueType font, a CFF font); none for the others (a
    /// standard collection's, a CMap file's, a Type 1 font's glyph names).
    fn native_tounicode(&mut self, pdf_id: i32) -> Result<Option<Vec<Option<String>>>> {
        if pdf_id < 0 || pdf_id as usize >= self.font.fonts.len() {
            return Ok(None);
        }
        let t0 = self.get_font_reencoded(pdf_id);
        let font = &self.font.fonts[t0];
        let cid = font.type0.descendant;
        if font.subtype != PDF_FONT_FONTTYPE_TYPE0 || cid < 0 {
            return Ok(None);
        }
        if self.CIDFont_is_ACCFont(cid) || self.CIDFont_is_UCSFont(cid) {
            return Ok(None);
        }
        let c = &self.font.fonts[cid as usize];
        let otf = if c.subtype == PDF_FONT_FONTTYPE_CIDTYPE2 {
            c.cid.csi.registry.as_deref() == Some(b"Adobe")
                && c.cid.csi.ordering.as_deref() == Some(b"Identity")
        } else {
            c.flags & CIDFONT_FLAG_TYPE1 == 0
        };
        if !otf {
            return Ok(None);
        }
        let (ident, index) = (c.ident.clone().unwrap_or_default(), c.index);
        self.otf_tounicode_texts(&ident, index)
    }

    /// What TFM font `pdf_id`'s characters are drawn with.
    fn run_font(&mut self, pdf_id: i32) -> Rc<RunFont> {
        if let Some(f) = self.runs.fonts.get(&pdf_id) {
            return f.clone();
        }
        let f = Rc::new(
            self.make_run_font(pdf_id)
                .ok()
                .flatten()
                .unwrap_or(RunFont {
                    kind: 0,
                    file: Vec::new(),
                    face: 0,
                    names: Vec::new(),
                    gids: Vec::new(),
                    texts: Vec::new(),
                }),
        );
        self.runs.fonts.insert(pdf_id, f.clone());
        f
    }

    fn make_run_font(&mut self, pdf_id: i32) -> Result<Option<RunFont>> {
        let Some(f) = usize::try_from(pdf_id)
            .ok()
            .and_then(|i| self.font.fonts.get(i))
        else {
            return Ok(None);
        };
        let base = if f.flags & PDF_FONT_FLAG_IS_ALIAS != 0 {
            f.font_id
        } else {
            pdf_id
        };
        let font = &self.font.fonts[base as usize];
        let (subtype, flags, index, encoding_id) =
            (font.subtype, font.flags, font.index, font.encoding_id);
        let name = font
            .filename
            .clone()
            .or_else(|| font.ident.clone())
            .unwrap_or_default();
        let enc = if encoding_id >= 0 && subtype != PDF_FONT_FONTTYPE_TYPE0 {
            Some(self.pdf_encoding_get_encoding(encoding_id)?)
        } else {
            None
        };
        let (kind, file, names, gids) = match subtype {
            PDF_FONT_FONTTYPE_TYPE1 if flags & PDF_FONT_FLAG_BASEFONT != 0 => {
                (1, name, enc.unwrap_or_default(), Vec::new())
            }
            PDF_FONT_FONTTYPE_TYPE1 => {
                let file = self.dpx_find_type1_file(&name)?.unwrap_or(name.clone());
                let names = match enc {
                    Some(e) => e,
                    None => match self.dpx_open_file(&name, crate::dpxfile::ResType::T1Font)? {
                        Some(mut fp) => {
                            crate::t1_load::t1_builtin_encoding(&mut fp)?.unwrap_or_default()
                        }
                        None => Vec::new(),
                    },
                };
                (1, file, names, Vec::new())
            }
            PDF_FONT_FONTTYPE_TYPE1C => {
                let file = self.dpx_find_opentype_file(&name)?.unwrap_or(name.clone());
                let (names, gids) = self
                    .t1c_code_glyphs(&name, enc.as_deref())?
                    .unwrap_or_default();
                (3, file, names, gids)
            }
            PDF_FONT_FONTTYPE_TRUETYPE => {
                let file = match self.dpx_find_truetype_file(&name)? {
                    Some(p) => p,
                    None => self.dpx_find_dfont_file(&name)?.unwrap_or(name.clone()),
                };
                let gids = self
                    .tt_code_gids(&name, index as i32, enc.as_deref())?
                    .unwrap_or_default();
                (2, file, enc.unwrap_or_default(), gids)
            }
            _ => (0, name, Vec::new(), Vec::new()),
        };
        let texts = names
            .iter()
            .map(|n| n.as_deref().and_then(|n| self.agl_text(n)))
            .collect();
        Ok(Some(RunFont {
            kind,
            file,
            face: index,
            names,
            gids,
            texts,
        }))
    }

    /// The text `pdf_create_ToUnicode_CMap` maps glyph name `name` to (the
    /// AGL's, `uniXXXX`, `uXXXX[XX]`, `_` joining components, a `.suffix`
    /// dropped), if any.
    fn agl_text(&mut self, name: &[u8]) -> Option<String> {
        let mut buf = [0u8; 1024];
        let mut p = 0;
        let (len, _) = self.agl_sput_UTF16BE(name, &mut buf, &mut p);
        if len < 1 {
            return None;
        }
        let units: Vec<u16> = buf[..p]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}
