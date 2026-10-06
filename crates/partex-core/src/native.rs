//! `XeTeX`'s native fonts (`XeTeX` §744 and on, `XeTeX_ext.c`): fonts
//! loaded through `partex-otf` (looked up by name in the index of the
//! installed fonts, or by file), and the words set in them.
//!
//! A native font's layout state (the script of the last word shaped in
//! it, which sets the default direction of the next) is engine state: a
//! field of the font table (`FontData::native_dir`), read and written
//! where a word is measured.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::RefCell;

use partex_engine::native::{GlyphNode, NativeGlyph, NativeWord};
use partex_engine::node::FontId;
use partex_engine::node::{Node, Whatsit};
use partex_engine::persist::Persist;
use partex_otf::xetex::fontmgr::FontManager;
use partex_otf::xetex::{Diagnostic, XeTeXFont};
use partex_otf::{FontSource, KpseFormat};

use crate::arith::Scaled;
use crate::fonts::fx;
use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// A loaded native font (`font_layout_engine`), with what loading it
/// computed.
pub(crate) struct NativeFont {
    pub(crate) font: XeTeXFont,
    /// The fontdimens it was loaded with (§744's `font_info`).
    pub(crate) params: Vec<Scaled>,
    /// `height_base`, `depth_base`: the ascent and descent.
    pub(crate) height_base: Scaled,
    pub(crate) depth_base: Scaled,
    /// Whether it is an OpenType math font (`isOpenTypeMathFont`).
    pub(crate) math: bool,
}

impl core::fmt::Debug for NativeFont {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "NativeFont({:?})", self.font.name_of_file)
    }
}

/// A native font is saved by reference to its files: the face's bytes
/// and the mapping's are the `Arc`s the host served (shared values: in a
/// store, the content-addressed blobs of the files served, kept once
/// however many states and fonts hold them, as a TFM font's bytes are),
/// with the face's index and content key and what loading computed
/// (features, sizes, colour, fontdimens) as values. Loading parses the
/// face and compiles the mapping again; the shaper's caches are made
/// again as they are used.
impl Persist for NativeFont {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        let NativeFont {
            font,
            params,
            height_base,
            depth_base,
            math,
        } = self;
        let XeTeXFont {
            face,
            path,
            index,
            point_size,
            scaled_size,
            script,
            language,
            features,
            req_engine,
            rgba,
            extend,
            slant,
            embolden,
            flags,
            letter_space,
            // (compiled again from its file)
            mapping: _,
            mapping_file,
            design_size,
            name_of_file,
            last_script,
        } = font;
        let key = face.key();
        face.data().save(s);
        (face.index(), key.hash[0], key.hash[1], key.len, key.index).save(s);
        String::from(&**path).save(s);
        index.save(s);
        point_size.save(s);
        scaled_size.save(s);
        script.save(s);
        language.save(s);
        features.len().save(s);
        for f in features {
            (f.tag, f.value, f.start, f.end).save(s);
        }
        req_engine.save(s);
        rgba.save(s);
        extend.save(s);
        slant.save(s);
        embolden.save(s);
        flags.save(s);
        letter_space.save(s);
        mapping_file.save(s);
        design_size.save(s);
        name_of_file.save(s);
        last_script.save(s);
        params.save(s);
        height_base.save(s);
        depth_base.save(s);
        math.save(s);
    }

    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let data: Arc<[u8]> = Persist::load(l)?;
        let (face_index, h0, h1, len, key_index) = Persist::load(l)?;
        let key = partex_otf::face::FaceKey {
            hash: [h0, h1],
            len,
            index: key_index,
        };
        let face = partex_otf::face::Face::with_key(data, face_index, key)?;
        let path: String = Persist::load(l)?;
        let index = Persist::load(l)?;
        let point_size = Persist::load(l)?;
        let scaled_size = Persist::load(l)?;
        let script = Persist::load(l)?;
        let language = Persist::load(l)?;
        let n: usize = Persist::load(l)?;
        let mut features = Vec::new();
        for _ in 0..n {
            let (tag, value, start, end) = Persist::load(l)?;
            features.push(partex_otf::shape::Feature {
                tag,
                value,
                start,
                end,
            });
        }
        let req_engine = Persist::load(l)?;
        let rgba = Persist::load(l)?;
        let extend = Persist::load(l)?;
        let slant = Persist::load(l)?;
        let embolden = Persist::load(l)?;
        let flags = Persist::load(l)?;
        let letter_space = Persist::load(l)?;
        let mapping_file: Option<Arc<[u8]>> = Persist::load(l)?;
        let mapping = match &mapping_file {
            Some(b) => Some(Arc::new(partex_otf::teckit::Mapping::new(b, true, true)?)),
            None => None,
        };
        Some(NativeFont {
            font: XeTeXFont {
                face,
                path: Arc::from(path.as_str()),
                index,
                point_size,
                scaled_size,
                script,
                language,
                features,
                req_engine,
                rgba,
                extend,
                slant,
                embolden,
                flags,
                letter_space,
                mapping,
                mapping_file,
                design_size: Persist::load(l)?,
                name_of_file: Persist::load(l)?,
                last_script: Persist::load(l)?,
            },
            params: Persist::load(l)?,
            height_base: Persist::load(l)?,
            depth_base: Persist::load(l)?,
            math: Persist::load(l)?,
        })
    }
}

/// What finding native fonts keeps across loads: the font manager over
/// the index (made at the first native font) and the shaper's caches;
/// and the line break rules of `\XeTeXlinebreaklocale`. None is engine
/// state: all are made again from the index or the data, by a clone of
/// the engine as by a state loaded (neither saved nor cloned).
#[derive(Default)]
pub(crate) struct NativeEnv {
    mgr: Option<FontManager>,
    shaper: Option<partex_otf::shape::Shaper>,
    /// ICU's line break rules last used (`\XeTeXlinebreaklocale`), read
    /// from the data once a rule set.
    pub(crate) linebreak: Option<(
        crate::icu_linebreak::RuleSet,
        Arc<crate::icu_linebreak::Rules>,
    )>,
}

impl Clone for NativeEnv {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// The fonts and files of `FontSource`, read through the host as loads.
struct Source<'a, H: Host, T: Tracker> {
    host: RefCell<&'a mut H>,
    tracker: &'a T,
    /// Files found by a lookup, by the path it gave.
    found: RefCell<BTreeMap<String, Arc<[u8]>>>,
}

impl<H: Host, T: Tracker> Source<'_, H, T> {
    fn read_kind(&self, name: &[u8], kind: FileKind) -> Option<crate::host::OpenedFile> {
        let f = self.host.borrow_mut().read_file(name, kind);
        if T::VALUES {
            self.tracker
                .load(name, kind, f.as_ref().map(|f| &f.contents));
        }
        f
    }
}

impl<H: Host, T: Tracker> FontSource for Source<'_, H, T> {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        if let Some(d) = self.found.borrow().get(path) {
            return Some(d.clone());
        }
        self.read_kind(path.as_bytes(), FileKind::Other)
            .map(|f| f.contents)
    }

    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        let kind = match format {
            KpseFormat::OpenType => FileKind::OpenType,
            KpseFormat::TrueType => FileKind::TrueType,
            KpseFormat::Type1 => FileKind::Type1,
            KpseFormat::MiscFonts => FileKind::MiscFonts,
        };
        let f = self.read_kind(name.as_bytes(), kind)?;
        let path = String::from_utf8_lossy(&f.name).into_owned();
        self.found.borrow_mut().insert(path.clone(), f.contents);
        Some(path)
    }
}

/// `first_math_fontdimen` plus the MATH constants: an OpenType math font's
/// fontdimens.
#[allow(clippy::cast_possible_wrap, reason = "55")]
const MATH_FONT_DIMENS: i32 = 10 + partex_otf::math::LAST_CONSTANT as i32;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX`'s e-TeX state `code` (`\XeTeXtracingfonts` …).
    pub(crate) fn xetex_state(&self, code: i32) -> i32 {
        self.eqtb_int(ETEX_STATE_BASE + code)
    }

    /// Font `f`'s native font, if it is one.
    pub(crate) fn native_font(&self, f: i32) -> Option<&Arc<NativeFont>> {
        usize::try_from(f)
            .ok()
            .and_then(|i| self.fonts.native.get(i))
            .and_then(Option::as_ref)
    }

    /// `is_native_font(f)`: whether font `f` was loaded as a native font.
    pub(crate) fn is_native_font(&self, f: i32) -> bool {
        self.native_font(f).is_some()
    }

    /// The font manager over the index of the installed fonts (the index
    /// read at the first native font, a load).
    fn font_manager(&mut self) -> FontManager {
        if let Some(m) = self.xfont.mgr.take() {
            return m;
        }
        let src = Source {
            host: RefCell::new(&mut self.host),
            tracker: &self.tracker,
            found: RefCell::new(BTreeMap::new()),
        };
        let index = src
            .read_kind(b"", FileKind::FontIndex)
            .and_then(|f| partex_otf::index::FontIndex::from_bytes(&f.contents))
            .unwrap_or_default();
        FontManager::new(Arc::new(index))
    }

    /// `XeTeX` §744 `load_native_font`: the font `name_of_file` names (a
    /// font name with its options, or a `[file]`), at size `s`, or the
    /// null font.
    pub(crate) fn load_native_font(
        &mut self,
        u: i32,
        nom: i32,
        aire: i32,
        s: Scaled,
    ) -> Result<i32, Jump> {
        let name = String::from_utf8_lossy(&self.name_of_file).into_owned();
        let tracing = self.xetex_state(XETEX_TRACING_FONTS_CODE);
        let mut mgr = self.font_manager();
        let (found, diags) = {
            let src = Source {
                host: RefCell::new(&mut self.host),
                tracker: &self.tracker,
                found: RefCell::new(BTreeMap::new()),
            };
            partex_otf::xetex::find_native_font(&mut mgr, &src, &name, s, tracing)
        };
        self.xfont.mgr = Some(mgr);
        for d in &diags {
            self.native_font_diagnostic(d);
        }
        let Some(font) = found else {
            return Ok(NULL_FONT);
        };
        let actual_size = if s >= 0 {
            s
        } else if s == -1000 {
            font.design_size
        } else {
            self.xn_over_d(font.design_size, -s, 1000)
        };
        // Look again to see if the font is already loaded, now that its
        // canonical name is known.
        let full_name = font.name_of_file.clone().into_bytes();
        self.font_table_read();
        for f in self.fonts.loaded_fonts() {
            if self.is_native_font(f)
                && self.fonts.get(f).size == actual_size
                && self.str_bytes(crate::input::ux(self.fonts.name[fx(f)]))
                    == Self::pool_bytes_of(&full_name).as_slice()
            {
                self.found_font(f);
                return Ok(f);
            }
        }
        let math = font.is_math_font();
        let num_font_dimens = if math { MATH_FONT_DIMENS } else { 8 };
        if self.font_ptr == self.params.font_max
            || self.fmem_ptr + num_font_dimens > self.params.font_mem_size
        {
            // §567: apologize for not loading the font.
            self.start_font_error_message(u, nom, aire, s);
            self.print_str(b" not loaded: Not enough room left");
            self.help(&[
                b"I'm afraid I won't be able to make use of this font,",
                b"because my memory for character-size data is too small.",
                b"If you're really stuck, ask a wizard to enlarge me.",
                b"Or maybe try `I\\font<same font id>=<name of loaded font>'.",
            ]);
            self.error()?;
            return Ok(NULL_FONT);
        }
        // Measure the width of the space character (with no mapping).
        let space = {
            let mut probe = font.clone();
            probe.mapping = None;
            let shaper = self
                .xfont
                .shaper
                .get_or_insert_with(partex_otf::shape::Shaper::new);
            let (l, _) = probe.layout(shaper, &[0x20], probe.default_rtl(), probe.letter_space);
            l.width
        };
        let p = font.native_params(space, actual_size);
        let mut metrics = partex_engine::font::Font::null();
        metrics.design_size = font.design_size;
        metrics.size = actual_size;
        metrics.params.clone_from(&p.params);
        let full = self.make_pool_string(&full_name)?;
        let key = font.face.key();
        let (f, ident, idv) = self.new_font_slot(
            &crate::fonts::FontIdent::Native {
                face: (key.hash, key.len, key.index),
                name: &full_name,
                size: actual_size,
            },
            &full_name,
        );
        self.tracker.read(crate::track::Cell::FontTable);
        self.tracker.write(crate::track::Cell::FontTable);
        self.tracker.write(crate::track::Cell::Font(f));
        let empty = self.pool_str(b"");
        self.fonts
            .place(f, metrics, Arc::from([]), full, empty, ident, idv);
        self.fonts.native[fx(f)] = Some(Arc::new(NativeFont {
            font,
            params: p.params,
            height_base: p.height_base,
            depth_base: p.depth_base,
            math,
        }));
        self.font_named(full);
        self.font_loaded(f);
        self.fonts.hyphen_char[fx(f)] = self.int_par(DEFAULT_HYPHEN_CHAR_CODE);
        self.fonts.skew_char[fx(f)] = self.int_par(DEFAULT_SKEW_CHAR_CODE);
        self.fmem_ptr += num_font_dimens;
        self.font_ptr += 1;
        self.font_made(f);
        Ok(f)
    }

    /// The pool's bytes for UTF-8 text `s` (as [`Tex::make_pool_string`]
    /// stores it).
    fn pool_bytes_of(s: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in String::from_utf8_lossy(s).chars() {
            crate::strings::cesu8(u32::from(c), |b| out.push(b));
        }
        out
    }

    /// A new pool string of UTF-8 text `s`.
    pub(crate) fn make_pool_string(&mut self, s: &[u8]) -> Result<i32, Jump> {
        let text = String::from_utf8_lossy(s).into_owned();
        self.str_room(text.len() * 2)?;
        for c in text.chars() {
            self.append_char(u32::from(c));
        }
        Ok(i32::try_from(self.make_string()?).unwrap_or(0))
    }

    /// What `XeTeX`'s font loading prints (`font_feature_warning`,
    /// `font_mapping_warning`, the path under `\XeTeXtracingfonts`).
    fn native_font_diagnostic(&mut self, d: &Diagnostic) {
        let raw = |t: &mut Self, s: &str| {
            for &b in s.as_bytes() {
                t.print_raw_char(u32::from(b), true);
            }
        };
        self.begin_diagnostic();
        match d {
            Diagnostic::UnknownFeature { feature, font } => {
                self.print_nl(b"Unknown ");
                self.print_str(b"feature `");
                raw(self, feature);
                self.print_str(b"' in font `");
                raw(self, font);
                self.print_str(b"'.");
            }
            Diagnostic::MappingLoaded { mapping, font }
            | Diagnostic::MappingNotFound { mapping, font }
            | Diagnostic::MappingNotUsable { mapping, font } => {
                if matches!(d, Diagnostic::MappingLoaded { .. }) {
                    self.print_nl(b"Loaded mapping `");
                } else {
                    self.print_nl(b"Font mapping `");
                }
                raw(self, mapping);
                self.print_str(b"' for font `");
                raw(self, font);
                match d {
                    Diagnostic::MappingNotFound { .. } => self.print_str(b"' not found."),
                    Diagnostic::MappingNotUsable { .. } => {
                        self.print_str(b"' not usable;");
                        self.print_nl(b"bad mapping file or incorrect mapping type.");
                    }
                    _ => self.print_str(b"'."),
                }
            }
            Diagnostic::FontPath(path) => {
                self.print_nl(b" ");
                self.print_str(b"-> ");
                for &b in path.as_bytes() {
                    self.print_char_x(u32::from(b));
                }
            }
        }
        self.end_diagnostic(false);
    }

    /// The fontdimens native font `f` was loaded with (a font made again in
    /// its slot, [`Tex::remake_font`]).
    pub(crate) fn native_params(&self, f: i32) -> Option<Vec<Scaled>> {
        self.native_font(f).map(|n| n.params.clone())
    }

    /// `set_native_metrics` (`measure_native_node`): lay out word `w`'s
    /// text in its font: its glyphs, width, and height and depth (the
    /// font's, or with `\XeTeXuseglyphmetrics` the glyphs' bounds).
    pub(crate) fn measure_native(&mut self, w: &mut NativeWord) {
        let f = i32::from(w.font.0);
        let Some(nf) = self.native_font(f).cloned() else {
            return;
        };
        self.font_read(f, crate::track::font::NATIVE_DIR);
        let use_glyph_metrics = self.xetex_state(XETEX_USE_GLYPH_METRICS_CODE) > 0;
        let dir = self.fonts.native_dir[fx(f)];
        let default_rtl = dir != 0 && partex_otf::xetex::script_is_rtl(dir);
        let shaper = self
            .xfont
            .shaper
            .get_or_insert_with(partex_otf::shape::Shaper::new);
        let (layout, last) = nf
            .font
            .layout(shaper, &w.text, default_rtl, nf.font.letter_space);
        let last = last.unwrap_or(0);
        if last != dir {
            self.fonts.native_dir[fx(f)] = last;
            self.font_wrote(f, crate::track::font::NATIVE_DIR);
        }
        let (height, depth) = if use_glyph_metrics {
            nf.font
                .layout_height_depth(&layout.glyphs)
                .unwrap_or((nf.height_base, nf.depth_base))
        } else {
            (nf.height_base, nf.depth_base)
        };
        w.width = layout.width;
        w.height = height;
        w.depth = depth;
        w.glyphs = layout
            .glyphs
            .iter()
            .map(|g| NativeGlyph {
                gid: g.gid,
                x: g.x,
                y: g.y,
                cluster: g.cluster,
            })
            .collect();
    }

    /// A `glyph_node` of glyph `g` of native font `f` measured as
    /// `set_native_glyph_metrics(p, use_glyph_metrics)` does.
    pub(crate) fn native_glyph_node(&self, f: i32, g: u16, use_glyph_metrics: bool) -> GlyphNode {
        let mut n = GlyphNode {
            font: font_id(f),
            gid: g,
            width: 0,
            height: 0,
            depth: 0,
        };
        if let Some(nf) = self.native_font(f) {
            let (w, hd) = nf.font.measure_glyph(u32::from(g), use_glyph_metrics);
            let (h, d) = hd.unwrap_or((nf.height_base, nf.depth_base));
            n.width = w;
            n.height = h;
            n.depth = d;
        }
        n
    }

    /// `XeTeX` §1444: implement `\XeTeXglyph`, a glyph of the current
    /// font by its number (a paragraph started in vertical mode).
    pub(crate) fn implement_glyph(&mut self) -> Result<(), Jump> {
        match self.mode().abs() {
            VMODE => {
                self.back_input()?;
                self.new_graf(true)
            }
            MMODE => self.report_illegal_case(),
            _ => {
                let f = self.cur_font();
                if !self.is_native_font(f) {
                    return self.font_kind_error(
                        EXTENSION,
                        GLYPH_CODE,
                        f,
                        b"; not a native platform font",
                    );
                }
                // The node is the tail while its number is scanned.
                let mut g = self.native_glyph_node(f, 0, false);
                self.tail_append(Node::Whatsit(Box::new(Whatsit::Glyph(g))));
                self.scan_int()?;
                if self.cur_val < 0 || self.cur_val > 65535 {
                    self.print_err(b"Bad glyph number");
                    self.help(&[
                        b"A glyph number must be between 0 and 65535.",
                        b"I changed this one to zero.",
                    ]);
                    self.int_error(self.cur_val)?;
                    self.cur_val = 0;
                }
                self.pop_tail();
                self.font_read(f, crate::track::font::METRICS);
                let use_glyph_metrics = self.xetex_state(XETEX_USE_GLYPH_METRICS_CODE) > 0;
                g = self.native_glyph_node(
                    f,
                    u16::try_from(self.cur_val).unwrap_or(0),
                    use_glyph_metrics,
                );
                self.tail_append(Node::Whatsit(Box::new(Whatsit::Glyph(g))));
                Ok(())
            }
        }
    }

    /// `new_native_word_node` with its metrics set: a word of `text` in
    /// native font `f`.
    pub(crate) fn new_native_word(&mut self, f: i32, text: &[u16]) -> NativeWord {
        let at = self.xetex_state(XETEX_GENERATE_ACTUAL_TEXT_CODE) > 0;
        let mut w = NativeWord::new(font_id(f), at, Arc::from(text));
        self.measure_native(&mut w);
        w
    }

    /// `XeTeX` §745 `new_native_character`: character `c` in native font
    /// `f` (through the font's mapping, if any), warning of characters
    /// the font lacks.
    pub(crate) fn new_native_character(&mut self, f: i32, c: i32) -> Result<NativeWord, Jump> {
        let Some(nf) = self.native_font(f).cloned() else {
            return Ok(NativeWord::new(font_id(f), false, Arc::from([])));
        };
        let units = utf16_of(c);
        if nf.font.mapping.is_some() {
            let mapped = nf.font.apply_mapping(&units);
            let mut i = 0;
            while i < mapped.len() {
                let u = i32::from(mapped[i]);
                let (ch, n) = if (0xD800..0xDC00).contains(&u) && i + 1 < mapped.len() {
                    (
                        (u - 0xD800) * 1024 + i32::from(mapped[i + 1]) - 0xDC00 + 0x1_0000,
                        2,
                    )
                } else {
                    (u, 1)
                };
                if nf.font.map_char_to_glyph(ch) == 0 {
                    self.char_warning(f, ch)?;
                }
                i += n;
            }
            return Ok(self.new_native_word(f, &mapped));
        }
        if self.int_par(TRACING_LOST_CHARS_CODE) > 0 && nf.font.map_char_to_glyph(c) == 0 {
            self.char_warning(f, c)?;
        }
        // (`subtype(p):=native_word_node`: never `_AT`)
        let mut w = NativeWord::new(font_id(f), false, Arc::from(units));
        self.measure_native(&mut w);
        Ok(w)
    }
}

/// The node font of font `f`.
pub(crate) fn font_id(f: i32) -> FontId {
    FontId(u16::try_from(f).unwrap_or(0))
}

/// The UTF-16 units of scalar value `c`.
pub(crate) fn utf16_of(c: i32) -> Vec<u16> {
    let c = u32::try_from(c).unwrap_or(0);
    if c > 0xFFFF {
        let v = c - 0x1_0000;
        alloc::vec![
            u16::try_from(0xD800 + v / 0x400).unwrap_or(0),
            u16::try_from(0xDC00 + v % 0x400).unwrap_or(0)
        ]
    } else {
        alloc::vec![u16::try_from(c).unwrap_or(0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestHost;
    use crate::track::Untracked;
    use partex_engine::persist::{Loader, Saver};

    const TTF: &[u8] = include_bytes!("../../phitex-draw/tests/data/dejavu-A.ttf");

    /// `[dejavu-A.ttf]` at 10 pt with features, loaded as
    /// `load_native_font` loads it.
    fn native(host: &mut TestHost) -> NativeFont {
        let src = Source {
            host: RefCell::new(host),
            tracker: &Untracked,
            found: RefCell::new(BTreeMap::new()),
        };
        let mut mgr = FontManager::new(Arc::default());
        let (f, _) = partex_otf::xetex::find_native_font(
            &mut mgr,
            &src,
            "[dejavu-A.ttf]:+kern;color=FF000080;letterspace=2",
            10 << 16,
            0,
        );
        NativeFont {
            font: f.expect("the font loads"),
            params: alloc::vec![1, 2, 3],
            height_base: 7,
            depth_base: -2,
            math: false,
        }
    }

    /// A native font is saved by reference to the file the host served
    /// (one shared value with it, not a copy per font) and loads as it
    /// was: the same face, fields and shaping, shared as it was shared.
    #[test]
    fn native_fonts_round_trip() {
        let mut h = TestHost::default();
        h.files.insert(b"dejavu-A.ttf".to_vec(), TTF.to_vec());
        let nf = Arc::new(native(&mut h));
        let file = nf.font.face.data().clone();
        let mut s = Saver::new();
        (file.clone(), nf.clone(), nf.clone()).save(&mut s);
        let bytes = s.into_bytes();
        assert!(
            bytes.len() < TTF.len() + 1000,
            "the font's bytes were saved again"
        );
        let mut l = Loader::new(&bytes);
        let (f, a, b): (Arc<[u8]>, Arc<NativeFont>, Arc<NativeFont>) =
            Persist::load(&mut l).expect("the font loads");
        assert!(l.at_end());
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&f, a.font.face.data()));
        let (x, y) = (&nf.font, &a.font);
        assert_eq!(x.face.key(), y.face.key());
        assert_eq!(
            (
                &*x.path,
                x.index,
                x.scaled_size,
                x.rgba,
                x.flags,
                x.letter_space
            ),
            (
                &*y.path,
                y.index,
                y.scaled_size,
                y.rgba,
                y.flags,
                y.letter_space
            )
        );
        assert_eq!(x.features, y.features);
        assert_eq!(x.name_of_file, y.name_of_file);
        assert_eq!(
            (&nf.params, nf.height_base, nf.depth_base),
            (&a.params, a.height_base, a.depth_base)
        );
        let mut shaper = partex_otf::shape::Shaper::new();
        let text = [0x41, 0x41];
        assert_eq!(
            x.layout(&mut shaper, &text, false, x.letter_space),
            y.layout(&mut shaper, &text, false, y.letter_space)
        );
    }
}
