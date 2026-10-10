//! XeTeX's native fonts and what is set in them (xetex.web §744–§756,
//! `XeTeX_ext.c`), as values: a native font is a font value carrying the
//! OpenType face (partex-otf's `XeTeXFont`, shared by `Arc` like a macro's
//! body); a word set in it is an item whose glyphs are a pure function of
//! (the font, the text, the default direction, `\XeTeXuseglyphmetrics`).
//! There is no font cache and no layout engine state: the direction of the
//! last word shaped in a font (XeTeX's `hb_buffer` script, which sets the
//! default direction of a word without strong characters) is
//! `Key::NativeDir(font)`, read and defined by the nodes that set words
//! (`graph/xetex.rs`); a word carries the direction it was shaped with, and
//! its parts are shaped again with it (hyphenation, `hpack`'s merging).

use std::sync::Arc;

use partex_otf::xetex::{Diagnostic, NativeLayout, XeTeXFont};

use crate::tfm::Font;
use crate::val::Glue;

/// A native font's own data (`font_layout_engine`, `height_base`,
/// `depth_base`, `font_letter_space`, `font_glue`).
pub struct NativeFont {
    pub xf: XeTeXFont,
    /// `height_base`, `depth_base`: the font's ascent and descent.
    pub height_base: i32,
    pub depth_base: i32,
    /// An OpenType math font (`isOpenTypeMathFont`).
    pub math: bool,
    /// `font_glue[f]` as `\fontdimen2`–`4` load it: its identity is what
    /// `hlist_out` compares (xetex.web §1414: a run of words joined by the
    /// font's own interword glue is one glyph run).
    pub glue: Arc<Glue>,
    /// The name `\font` asked for (`name_of_file` before the lookup): a
    /// format loads the font again by it.
    pub asked: Arc<str>,
}

impl std::fmt::Debug for NativeFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NativeFont({})", self.xf.name_of_file)
    }
}

/// `first_math_fontdimen` plus the MATH constants.
pub const MATH_FONT_DIMENS: usize = 10 + partex_otf::math::LAST_CONSTANT as usize;

/// What loading a native font gives: the font, and what XeTeX prints
/// meanwhile.
pub type Loaded = (Option<Arc<Font>>, Vec<Diagnostic>);

/// xetex.web §744 `load_native_font` after `find_native_font`: the font
/// value of `xf` at `actual_size` (its fontdimens: slant, space, stretch,
/// shrink, x-height, quad, extra space, cap height, and an OpenType math
/// font's constants), asked for as `asked`.
pub fn make_font(mut xf: XeTeXFont, asked: &str, actual_size: i32) -> Font {
    xf.last_script = None;
    // (the width of a space, measured without the mapping, in a fresh
    // engine: `new_native_character(f, " ")`)
    let space = {
        let mut probe = xf.clone();
        probe.mapping = None;
        crate::tfm::with_shaper(|sh| probe.layout(sh, &[0x20], false, probe.letter_space).0.width)
    };
    let p = xf.native_params(space, actual_size);
    let math = xf.is_math_font();
    let mut params = Vec::with_capacity(p.params.len() + 1);
    params.push(0);
    params.extend_from_slice(&p.params);
    let name = xf.name_of_file.clone();
    let glue = Arc::new(Glue {
        width: params.get(2).copied().unwrap_or(0),
        stretch: params.get(3).copied().unwrap_or(0),
        shrink: params.get(4).copied().unwrap_or(0),
        ..Glue::default()
    });
    let mut f = Font::null();
    f.key = Font::key_of(&name, actual_size);
    f.name = name;
    f.design_size = xf.design_size;
    f.size = actual_size;
    f.bc = 0;
    f.ec = 65535;
    f.params = Arc::from(params);
    f.native = Some(Arc::new(NativeFont {
        height_base: p.height_base,
        depth_base: p.depth_base,
        math,
        glue,
        asked: Arc::from(asked),
        xf,
    }));
    f
}

/// One glyph of a native word: its id and position from the word's origin
/// (y down).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NGlyph {
    pub gid: u16,
    pub x: i32,
    pub y: i32,
}

/// A `native_word_node` (subtype 40; 41 `native_word_node_AT`, `at`: its
/// text goes to the PDF as `/ActualText`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeWord {
    pub font: Arc<Font>,
    pub at: bool,
    /// The characters, UTF-16 (`native_length` of them).
    pub text: Arc<[u16]>,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub glyphs: Arc<[NGlyph]>,
    /// Measured with `\XeTeXuseglyphmetrics` positive.
    pub gm: bool,
    /// The default direction it was shaped with (the font's last script
    /// was a right-to-left one).
    pub rtl: bool,
}

/// A `glyph_node` (`\XeTeXglyph`, math's glyphs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphItem {
    pub font: Arc<Font>,
    pub gid: u16,
    pub w: i32,
    pub h: i32,
    pub d: i32,
}

/// A picture (`pic_node`, `pdf_node`): the file as the XDV names it, the
/// page and box, the transformation, the box it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pic {
    pub pdf: bool,
    pub path: Arc<[u8]>,
    pub page: i32,
    pub pdf_box: u8,
    /// `pic_transform1`…`6`: a, b, c, d, x, y (scaled).
    pub t: [i32; 6],
    pub w: i32,
    pub h: i32,
    pub d: i32,
}

/// The native font of `f`, if it is one.
pub fn native(f: &Font) -> Option<&Arc<NativeFont>> {
    f.native.as_ref()
}

/// `is_rtl` of a script tag (0: none).
pub fn script_rtl(script: u32) -> bool {
    script != 0 && partex_otf::xetex::script_is_rtl(script)
}

/// The UTF-16 units of scalar value `c`.
pub fn utf16_of(c: u32) -> Vec<u16> {
    if c > 0xFFFF {
        let v = c - 0x1_0000;
        vec![(0xD800 + v / 0x400) as u16, (0xDC00 + v % 0x400) as u16]
    } else {
        vec![c as u16]
    }
}

/// `get_native_usv(p, i)`: the character at unit `i` (a surrogate pair's
/// first unit gives the pair's character).
pub fn usv_at(text: &[u16], i: usize) -> u32 {
    let u = u32::from(text[i]);
    if (0xD800..0xDC00).contains(&u)
        && let Some(&lo) = text.get(i + 1)
        && (0xDC00..0xE000).contains(&u32::from(lo))
    {
        return 0x1_0000 + (u - 0xD800) * 0x400 + (u32::from(lo) - 0xDC00);
    }
    u
}

/// The characters of `text`.
pub fn chars_of(text: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let c = usv_at(text, i);
        i += if c > 0xFFFF { 2 } else { 1 };
        out.push(c);
    }
    out
}

/// `measure_native_node` of `text` in native font `font` (`gm`:
/// `\XeTeXuseglyphmetrics`), its default direction `rtl`: the word, and
/// the script its last run was shaped in (the font's next `NativeDir`,
/// 0: none).
pub fn word(font: &Arc<Font>, text: Arc<[u16]>, at: bool, gm: bool, rtl: bool) -> (NativeWord, u32) {
    let Some(nf) = native(font) else {
        let w = NativeWord {
            font: font.clone(),
            at,
            text,
            w: 0,
            h: 0,
            d: 0,
            glyphs: Arc::from([]),
            gm,
            rtl,
        };
        return (w, 0);
    };
    let (layout, script) = layout(nf, &text, rtl);
    let (h, d) = if gm {
        nf.xf.layout_height_depth(&layout.glyphs).unwrap_or((nf.height_base, nf.depth_base))
    } else {
        (nf.height_base, nf.depth_base)
    };
    let glyphs: Arc<[NGlyph]> = layout.glyphs.iter().map(|g| NGlyph { gid: g.gid, x: g.x, y: g.y }).collect();
    (NativeWord { font: font.clone(), at, text, w: layout.width, h, d, glyphs, gm, rtl }, script)
}

/// The layout of `text` in `nf` with default direction `rtl`.
fn layout(nf: &NativeFont, text: &[u16], rtl: bool) -> (NativeLayout, u32) {
    crate::tfm::with_shaper(|sh| {
        let (l, s) = nf.xf.layout(sh, text, rtl, nf.xf.letter_space);
        (l, s.unwrap_or(0))
    })
}

impl NativeWord {
    /// A word of `text` in this word's font, measured as this one was
    /// (its parts: hyphenation, `hpack`'s merging).
    pub fn part(&self, text: &[u16]) -> NativeWord {
        word(&self.font, Arc::from(text), self.at, self.gm, self.rtl).0
    }

    /// `store_justified_native_glyphs`: measured again (without glyph
    /// metrics), its glyphs stretched to width `saved` (on its spaces, or
    /// all its glyphs).
    pub fn justified(&self, saved: i32) -> NativeWord {
        let Some(nf) = native(&self.font) else { return self.clone() };
        let (mut l, _) = layout(nf, &self.text, self.rtl);
        nf.xf.justify(&mut l, saved);
        let glyphs: Arc<[NGlyph]> = l.glyphs.iter().map(|g| NGlyph { gid: g.gid, x: g.x, y: g.y }).collect();
        NativeWord { w: saved, glyphs, ..self.clone() }
    }

    /// `make_xdv_glyph_array_data`: the width, the count, each glyph's x
    /// and y, then each glyph's id.
    pub fn glyph_array(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(6 + self.glyphs.len() * 10);
        v.extend_from_slice(&self.w.to_be_bytes());
        v.extend_from_slice(&(self.glyphs.len() as u16).to_be_bytes());
        for g in self.glyphs.iter() {
            v.extend_from_slice(&g.x.to_be_bytes());
            v.extend_from_slice(&g.y.to_be_bytes());
        }
        for g in self.glyphs.iter() {
            v.extend_from_slice(&g.gid.to_be_bytes());
        }
        v
    }

    /// `get_native_italic_correction`.
    pub fn italic_correction(&self) -> i32 {
        match native(&self.font) {
            Some(nf) => match self.glyphs.last() {
                Some(g) => nf.xf.glyph_italic_correction(u32::from(g.gid)) + nf.xf.letter_space,
                None => 0,
            },
            None => 0,
        }
    }
}

/// `set_native_glyph_metrics`: glyph `gid` of native font `font`.
pub fn glyph(font: &Arc<Font>, gid: u16, gm: bool) -> GlyphItem {
    let (w, h, d) = match native(font) {
        Some(nf) => {
            let (w, hd) = nf.xf.measure_glyph(u32::from(gid), gm);
            let (h, d) = hd.unwrap_or((nf.height_base, nf.depth_base));
            (w, h, d)
        }
        None => (0, 0, 0),
    };
    GlyphItem { font: font.clone(), gid, w, h, d }
}

/// `get_native_glyph_italic_correction` of a glyph item.
pub fn glyph_italic(g: &GlyphItem) -> i32 {
    native(&g.font).map_or(0, |nf| nf.xf.glyph_italic_correction(u32::from(g.gid)))
}

/// xetex.web §745 `new_native_character(f, c)`'s text: the character, or
/// what the font's mapping makes of it.
pub fn mapped(font: &Font, c: u32) -> Vec<u16> {
    let units = utf16_of(c);
    match native(font) {
        Some(nf) if nf.xf.mapping.is_some() => nf.xf.apply_mapping(&units),
        _ => units,
    }
}

/// Whether native font `f` has a glyph for character `c`
/// (`map_char_to_glyph(f, c) <> 0`).
pub fn has_char(f: &Font, c: u32) -> bool {
    native(f).is_some_and(|nf| nf.xf.map_char_to_glyph(c as i32) != 0)
}

/// `print_native_word`'s text: the characters (a lone surrogate as
/// `.`).
pub fn text_of(text: &[u16]) -> String {
    chars_of(text).into_iter().map(|c| char::from_u32(c).unwrap_or('.')).collect()
}

/// The fontdimen `n` of an OpenType math font's MATH table as XeTeX's
/// `get_native_mathsy_param` (family 2) reads it.
pub fn mathsy(f: &Font, n: i32) -> i32 {
    native(f).map_or(0, |nf| nf.xf.mathsy_param(n, f.size))
}

/// `get_native_mathex_param` (family 3).
pub fn mathex(f: &Font, n: i32) -> i32 {
    native(f).map_or(0, |nf| nf.xf.mathex_param(n, f.size))
}

/// Whether `f` is an OpenType math font (`is_new_mathfont`).
pub fn is_math(f: &Font) -> bool {
    native(f).is_some_and(|nf| nf.math)
}

pub use crate::tfm::Fonts;

/// XeTeX's output driver in process: the PDF `xdvipdfmx -q -E -o
/// PDF_NAME` makes of `xdv` (partex-xdvipdfmx, linked), its files found by
/// `kpse`, its streams compressed by zlib's deflate (`pdf::zlib`), its date
/// `SOURCE_DATE_EPOCH` = `epoch`.
pub fn xdv_to_pdf(xdv: &[u8], kpse: &Arc<dyn crate::tfm::Kpse>, pdf_name: &str, epoch: i64) -> Result<Vec<u8>, String> {
    let options = partex_xdvipdfmx::api::Options {
        pdf_filename: Some(pdf_name.as_bytes().to_vec()),
        source_date_epoch: Some(epoch),
        ..partex_xdvipdfmx::api::Options::default()
    };
    let files = Box::new(Files(kpse.clone()));
    let deflate: partex_xdvipdfmx::obj::Deflate = Box::new(|level, data| crate::pdf::zlib::compress(data, level));
    partex_xdvipdfmx::api::Session::convert(options, files, deflate, xdv).map_err(|e| e.to_string())
}

/// xdvipdfmx's file finder over the build's.
struct Files(Arc<dyn crate::tfm::Kpse>);

impl partex_xdvipdfmx::io::Files for Files {
    fn find(&mut self, name: &[u8], format: partex_xdvipdfmx::io::Format, progname: &[u8]) -> Option<Vec<u8>> {
        use crate::tfm::KpseFormat as K;
        use partex_xdvipdfmx::io::Format as F;
        let f = match format {
            F::Fontmap => K::FontMap,
            F::Type1 => K::Type1,
            F::TrueType => K::TrueType,
            F::OpenType => K::OpenType,
            F::Cmap => K::Cmap,
            F::Sfd => K::Sfd,
            F::Enc => K::Enc,
            F::Tfm => K::Tfm,
            F::Ofm => K::Ofm,
            F::Vf => K::Vf,
            F::Ovf => K::Ovf,
            F::Pict => K::Pict,
            F::Tex => K::Tex,
            F::ProgramText => K::ProgramText,
            F::ProgramBinary => K::ProgramBinary,
        };
        let name = String::from_utf8_lossy(name);
        let prog = String::from_utf8_lossy(progname);
        self.0.find(&name, f, &prog).map(String::into_bytes)
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.0.read(&String::from_utf8_lossy(path))
    }
}
