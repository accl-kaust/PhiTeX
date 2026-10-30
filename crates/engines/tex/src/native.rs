//! XeTeX's native fonts and words (docs/spec-xetex.md §3 "font loading",
//! §4): fonts found through `phitex-otf` (by name in the build's font
//! index, or `[file]` through its font files), loaded once per XeTeX
//! (its font files and index, [`NativeFonts`]) for a lookup, their words
//! shaped where a run makes them.
//!
//! A native font is a font like a TFM one to the rest of the engine: its
//! `fontinfo:` value (whose file is `[NATIVE, spec, size asked]`, so a
//! run in any process finds the same font again), its `\fontdimen`s in
//! its cells, its number from the order of loads. What the TFM path
//! reads of a font's file, a native font answers from its
//! [`NativeFont`].
//!
//! The one state a font carries from word to word is the script of the
//! last word shaped in it (the default direction of a word without a
//! strong character, spec §4.1): the field `fontdir:<f>`, read once a
//! run and written only where a word changes it.

use std::sync::{Arc, Mutex};

use phitex_defuse::{FxHashMap, ValueId, Values};
use phitex_otf::xetex::XeTeXFont;
use phitex_typeset::node::{Glyph, Native, NativeKind, Node};

use crate::exec::LockWait as _;
use crate::run::{R, Run};
use crate::val::{int_of, list_of, str_of};
use crate::xetex::XeTeX;

/// The tag of a native font's file value in its `fontinfo:`.
pub(crate) const NATIVE: &str = "\u{1}native";

/// A loaded native font (XeTeX's `font_layout_engine` with what
/// `load_native_font` computed).
pub(crate) struct NativeFont {
    pub font: XeTeXFont,
    /// The `\fontdimen`s it is loaded with.
    pub params: Vec<i32>,
    /// `height_base`, `depth_base`: the ascent and descent.
    pub height_base: i32,
    pub depth_base: i32,
    /// `font_size`.
    pub size: i32,
    /// Its definition in the XDV file after its number (`make_font_def`).
    pub def: Vec<u8>,
}

/// [`NativeFonts`]' loads: a specification and the size asked, and what
/// they found.
type Loads = FxHashMap<(String, i32), Option<Arc<NativeFont>>>;

/// The native fonts loaded through one [`XeTeX`]'s font files and index
/// ([`find`]): by the specification and the size asked, and the name
/// lookup over its index, made once. A font found, or not, is the
/// answer of that XeTeX's files, so the table is its own and goes with
/// it ([`XeTeX::native`], shared by its clones). A process-wide table
/// keyed by the files' and index's addresses gave a later build whose
/// files were allocated where a dropped build's had been that build's
/// answers: in a long-running process (a browser's core, a pdfLaTeX
/// session then an XeLaTeX one), the misses of a build made before its
/// fonts' files had come, for ever.
#[derive(Default)]
pub struct NativeFonts {
    loads: Mutex<Loads>,
    manager: Mutex<Option<phitex_otf::xetex::fontmgr::FontManager>>,
}

thread_local! {
    /// The worker's shaper (pure, its memo the worker's own).
    static SHAPER: std::cell::RefCell<phitex_otf::shape::Shaper> =
        std::cell::RefCell::new(phitex_otf::shape::Shaper::new());
}

/// A lookup's source: the build's, or none.
struct NoFiles;

impl phitex_otf::FontSource for NoFiles {
    fn read(&self, _: &str) -> Option<Arc<[u8]>> {
        None
    }
    fn find_file(&self, _: &str, _: phitex_otf::KpseFormat) -> Option<String> {
        None
    }
}

/// `find_native_font(spec, s)` and `load_native_font`'s measures: the
/// font `spec` names at `s` (`s < 0`: scaled), loaded once per process.
pub(crate) fn find(xe: &XeTeX, spec: &str, s: i32) -> Option<Arc<NativeFont>> {
    let key = (spec.to_owned(), s);
    if let Some(x) = xe.native.loads.locked().get(&key) {
        return x.clone();
    }
    let x = load(xe, spec, s).map(Arc::new);
    xe.native.loads.locked().insert(key, x.clone());
    x
}

fn load(xe: &XeTeX, spec: &str, s: i32) -> Option<NativeFont> {
    let no = NoFiles;
    let src: &dyn phitex_otf::FontSource = match &xe.fonts {
        Some(f) => f.as_ref(),
        None => &no,
    };
    let (found, _diags) = {
        let mut m = xe.native.manager.locked();
        let m = m.get_or_insert_with(|| {
            phitex_otf::xetex::fontmgr::FontManager::new(xe.font_index.clone().unwrap_or_default())
        });
        phitex_otf::xetex::find_native_font(m, src, spec, s, 0)
    };
    let mut font = found?;
    let size = if s >= 0 {
        s
    } else if s == -1000 {
        font.design_size
    } else {
        phitex_otf::xetex::xn_over_d(font.design_size, -s, 1000)
    };
    // (`load_native_font`: the space measured without the mapping, the
    // font's script state as the measure leaves it)
    let space = SHAPER.with(|sh| {
        let mut sh = sh.borrow_mut();
        let mapping = font.mapping.take();
        let w = font.layout_word(&mut sh, &[0x20], font.letter_space).width;
        font.mapping = mapping;
        w
    });
    let p = font.native_params(space, size);
    let def = font.font_def();
    Some(NativeFont {
        params: p.params,
        height_base: p.height_base,
        depth_base: p.depth_base,
        size,
        def,
        font,
    })
}

/// The native font of a `fontinfo:` value, if it is one: found again by
/// its specification ([`find`]).
pub(crate) fn of_info(xe: &XeTeX, v: &Values, info: Option<ValueId>) -> Option<Arc<NativeFont>> {
    let i = crate::fonts::info(v, info)?;
    let (spec, s) = native_spec(v, i.file?)?;
    find(xe, &spec, s)
}

/// A native font's file value: its specification and the size asked.
pub(crate) fn native_spec(v: &Values, file: ValueId) -> Option<(String, i32)> {
    let xs = list_of(v, file);
    if xs.first().map(|&t| str_of(v, t)).as_deref() != Some(NATIVE) {
        return None;
    }
    Some((
        str_of(v, *xs.get(1)?),
        i32::try_from(int_of(v, *xs.get(2)?)).ok()?,
    ))
}

/// A word node of `layout`, `text` in native font `nf` (number `f`):
/// its height and depth the font's, or (`glyph_metrics`) its glyphs'.
fn word_of(
    nf: &NativeFont,
    f: u32,
    text: &[u16],
    kind: NativeKind,
    layout: &phitex_otf::xetex::NativeLayout,
    glyph_metrics: bool,
) -> Native {
    let (height, depth) = if glyph_metrics {
        nf.font
            .layout_height_depth(&layout.glyphs)
            .unwrap_or((nf.height_base, nf.depth_base))
    } else {
        (nf.height_base, nf.depth_base)
    };
    Native {
        kind,
        font: f,
        width: layout.width,
        height,
        depth,
        text: Arc::from(text),
        glyphs: layout
            .glyphs
            .iter()
            .map(|g| Glyph {
                gid: g.gid,
                x: g.x,
                y: g.y,
            })
            .collect(),
    }
}

/// `set_native_metrics` on a word a box merges (xetex.web's
/// "Incorporate a whatsit node into an hbox"): `text` in `like`'s font,
/// laid out left to right by default (the font's last script is the
/// run's; a merged run's text has its own strong characters).
pub(crate) fn reshape(nf: &NativeFont, like: &Native, text: &[u16], glyph_metrics: bool) -> Native {
    let (layout, _) = SHAPER.with(|sh| {
        let mut sh = sh.borrow_mut();
        nf.font.layout(&mut sh, text, false, nf.font.letter_space)
    });
    word_of(nf, like.font, text, like.kind, &layout, glyph_metrics)
}

/// The UTF-16 units of `c`.
pub(crate) fn utf16(c: u32, out: &mut Vec<u16>) {
    if c > 0xFFFF {
        let x = c - 0x1_0000;
        out.push(u16::try_from(0xD800 + x / 0x400).unwrap_or(0));
        out.push(u16::try_from(0xDC00 + x % 0x400).unwrap_or(0));
    } else {
        out.push(u16::try_from(c).unwrap_or(0));
    }
}

impl Run<'_, '_> {
    /// Font `f`'s native font, if it is one (once a run).
    pub(crate) fn native_font(&mut self, f: i64) -> R<Option<Arc<NativeFont>>> {
        if !self.xe || f == 0 {
            return Ok(None);
        }
        if let Some((_, x)) = self.xfonts.iter().find(|e| e.0 == f) {
            return Ok(x.clone());
        }
        let info = self.font_get(f, crate::fields::FontCell::Info)?;
        let x = self.ops.xe.get().and_then(|xe| of_info(xe, self.v, info));
        self.xfonts.push((f, x.clone()));
        Ok(x)
    }

    /// The script of the last word shaped in font `f` (`fontdir:<f>`).
    fn last_script(&mut self, f: i64) -> R<Option<u32>> {
        if let Some((_, s)) = self.xdirs.iter().find(|e| e.0 == f) {
            return Ok(*s);
        }
        let x = self.get(&format!("fontdir:{f}"))?;
        let s = x
            .map(|x| int_of(self.v, x))
            .filter(|&n| n != 0)
            .and_then(|n| u32::try_from(n).ok());
        self.xdirs.push((f, s));
        Ok(s)
    }

    /// `set_native_metrics`: the word `text` in native font `f` laid out
    /// (its default direction from the font's last script, which the
    /// layout moves on), with its height and depth (the font's, or the
    /// glyphs' with `\\XeTeXuseglyphmetrics`).
    pub(crate) fn native_word(
        &mut self,
        f: i64,
        nf: &NativeFont,
        text: &[u16],
        kind: NativeKind,
    ) -> R<Native> {
        let last = self.last_script(f)?;
        let rtl = last.is_some_and(phitex_otf::xetex::script_is_rtl);
        let (layout, after) = SHAPER.with(|sh| {
            let mut sh = sh.borrow_mut();
            nf.font.layout(&mut sh, text, rtl, nf.font.letter_space)
        });
        if after != last {
            let v = self.v;
            self.set(
                &format!("fontdir:{f}"),
                v.int(i64::from(after.unwrap_or(0))),
                true,
            );
            if let Some(e) = self.xdirs.iter_mut().find(|e| e.0 == f) {
                e.1 = after;
            }
        }
        let glyph_metrics = self.param("XeTeXuseglyphmetrics")? > 0;
        Ok(word_of(
            nf,
            u32::try_from(f).unwrap_or(0),
            text,
            kind,
            &layout,
            glyph_metrics,
        ))
    }

    /// The kind of a word made now (`\\XeTeXgenerateactualtext`).
    pub(crate) fn word_kind(&mut self) -> R<NativeKind> {
        Ok(if self.param("XeTeXgenerateactualtext")? > 0 {
            NativeKind::WordText
        } else {
            NativeKind::Word
        })
    }

    /// `new_native_character(f, c)`: character `c` in native font `f`,
    /// through the font's mapping.
    pub(crate) fn native_char(&mut self, f: i64, nf: &NativeFont, c: u32) -> R<Native> {
        let mut units = Vec::with_capacity(2);
        utf16(c, &mut units);
        let fu = u32::try_from(f).unwrap_or(0);
        if nf.font.mapping.is_some() {
            let mapped = nf.font.apply_mapping(&units);
            // (each character mapped to no glyph: "Missing character")
            let mut i = 0;
            while i < mapped.len() {
                let u = u32::from(mapped[i]);
                let (ch, n) = if (0xD800..0xDC00).contains(&u) && i + 1 < mapped.len() {
                    (
                        (u - 0xD800) * 1024 + u32::from(mapped[i + 1]) - 0xDC00 + 0x1_0000,
                        2,
                    )
                } else {
                    (u, 1)
                };
                if nf.font.map_char_to_glyph(ch.cast_signed()) == 0 {
                    self.char_warning(fu, ch)?;
                }
                i += n;
            }
            return self.native_word(f, nf, &mapped, NativeKind::Word);
        }
        if nf.font.map_char_to_glyph(c.cast_signed()) == 0 {
            self.char_warning(fu, c)?;
        }
        self.native_word(f, nf, &units, NativeKind::Word)
    }

    /// `\\XeTeXglyph`'s node: glyph `g` of native font `f`
    /// (`set_native_glyph_metrics`).
    pub(crate) fn native_glyph(&mut self, nf: &NativeFont, f: i64, g: u16) -> R<Native> {
        let glyph_metrics = self.param("XeTeXuseglyphmetrics")? > 0;
        let (w, hd) = nf.font.measure_glyph(u32::from(g), glyph_metrics);
        let (h, d) = hd.unwrap_or((nf.height_base, nf.depth_base));
        Ok(Native {
            kind: NativeKind::Glyph,
            font: u32::try_from(f).unwrap_or(0),
            width: w,
            height: h,
            depth: d,
            text: Arc::from([]),
            glyphs: Arc::from([Glyph { gid: g, x: 0, y: 0 }]),
        })
    }
}

impl Run<'_, '_> {
    /// XeTeX's `read_font_info`: a quoted name a native font first, then
    /// a TFM file, then (unquoted) a native font; the null font, with
    /// the error unless `\\suppressfontnotfounderror`, where none is.
    pub(crate) fn xe_read_font_info(&mut self, name: &str, quoted: bool, s: i32) -> R<i64> {
        if quoted && let Some(f) = self.load_native_font(name, s)? {
            return Ok(f);
        }
        if let Some(f) = self.tfm_font_info(name, s)? {
            return Ok(f);
        }
        if !quoted && let Some(f) = self.load_native_font(name, s)? {
            return Ok(f);
        }
        if self.param("suppressfontnotfounderror")? == 0 {
            self.log("error", "Font not loadable")?;
        }
        Ok(0)
    }

    /// `load_native_font`: the font `spec` names at `s` loaded (or the
    /// font of that full name and size loaded before, xetex.web's second
    /// look); `None` where `phitex-otf` finds none.
    fn load_native_font(&mut self, spec: &str, s: i32) -> R<Option<i64>> {
        let Some(xe) = self.ops.xe.get().cloned() else {
            return Ok(None);
        };
        let Some(nf) = find(&xe, spec, s) else {
            return Ok(None);
        };
        let v = self.v;
        let full = nf.font.name_of_file.clone();
        let size = nf.size;
        let mut held = false;
        let k = crate::fonts::pick_key(&full, size, &mut |k| {
            let info = self.font_get(k, crate::fields::FontCell::Info)?;
            let other = crate::fonts::info(v, info).map(|i| i.name != full || i.size != size);
            held = other == Some(false);
            Ok::<_, crate::run::Halt>(other)
        })?;
        let f = i64::from(k);
        if held {
            return Ok(Some(f));
        }
        self.set("fontlast", v.int(f), true);
        self.font_loaded(f);
        crate::pdfout::font_load(self, &full, size, f);
        let file = v.list(vec![v.str(NATIVE), v.str(spec), v.int(i64::from(s))]);
        let info = v.list(vec![
            v.str(&full),
            v.int(i64::from(size)),
            v.int(i64::from(nf.font.design_size)),
            file,
            v.int(i64::from(size)),
        ]);
        self.font_set(f, crate::fields::FontCell::Info, info, true);
        for (k, p) in nf.params.iter().enumerate() {
            let k = i32::try_from(k + 1).unwrap_or(0);
            self.font_set(
                f,
                crate::fields::FontCell::Fd(k),
                v.int(i64::from(*p)),
                true,
            );
        }
        self.font_set(
            f,
            crate::fields::FontCell::Fdn,
            v.int(i64::try_from(nf.params.len()).unwrap_or(0)),
            true,
        );
        let hc = i64::from(self.ipar(crate::fields::Par::defaulthyphenchar)?);
        let sc = i64::from(self.ipar(crate::fields::Par::defaultskewchar)?);
        self.font_set(f, crate::fields::FontCell::Hc, v.int(hc), true);
        self.font_set(f, crate::fields::FontCell::Sc, v.int(sc), true);
        self.xfonts.retain(|e| e.0 != f);
        self.xfonts.push((f, Some(nf)));
        Ok(Some(f))
    }
}

/// An item of a list being built, as the native main loop sees it.
enum Seen {
    /// Not a node (a space factor transition): passed over.
    Skip,
    /// A native word of the font asked about.
    Word,
    /// A character or a ligature (`is_char_node`) or a discretionary.
    CharOrDisc,
    /// Anything else.
    Other,
}

fn seen(v: &Values, it: &crate::nest::Item, f: u32) -> Seen {
    match it {
        crate::nest::Item::N(n) => match n {
            n if is_word_of(n, f) => Seen::Word,
            Node::Char { .. } | Node::Lig(_) | Node::Disc(_) => Seen::CharOrDisc,
            _ => Seen::Other,
        },
        crate::nest::Item::V(x) => match crate::nodes::tag(v, *x) {
            crate::nodes::T_SF | crate::nodes::T_PD => Seen::Skip,
            crate::nodes::T_CHAR | crate::nodes::T_LIG | crate::nodes::T_DISC => Seen::CharOrDisc,
            crate::nodes::T_NATIVE => {
                let mut side = crate::nodes::Side::default();
                match crate::nodes::decode(v, *x, &mut side) {
                    Some(n) if is_word_of(&n, f) => Seen::Word,
                    _ => Seen::Other,
                }
            }
            _ => Seen::Other,
        },
    }
}

/// Whether `n` is invisible to interword space shaping
/// (`node_is_invisible_to_interword_space`: penalties, insertions, marks,
/// adjustments, and whatsits other than native ones).
fn invisible(v: &Values, it: &crate::nest::Item) -> bool {
    let tag = match it {
        crate::nest::Item::N(n) => match n {
            Node::Penalty(_)
            | Node::Ins(_)
            | Node::Mark(_)
            | Node::Adjust(_)
            | Node::Whatsit(..)
            | Node::Language { .. } => return true,
            _ => return false,
        },
        crate::nest::Item::V(x) => crate::nodes::tag(v, *x),
    };
    matches!(
        tag,
        crate::nodes::T_PENALTY
            | crate::nodes::T_INS
            | crate::nodes::T_MARK
            | crate::nodes::T_ADJUST
            | crate::nodes::T_WHATSIT
            | crate::nodes::T_LANGUAGE
            | crate::nodes::T_SF
    )
}

/// The native node an item is, if it is one.
fn native_of(v: &Values, it: &crate::nest::Item) -> Option<Native> {
    match it {
        crate::nest::Item::N(Node::Native(n)) => Some((**n).clone()),
        crate::nest::Item::N(_) => None,
        crate::nest::Item::V(x) => {
            if crate::nodes::tag(v, *x) != crate::nodes::T_NATIVE {
                return None;
            }
            let mut side = crate::nodes::Side::default();
            match crate::nodes::decode(v, *x, &mut side) {
                Some(Node::Native(n)) => Some(*n),
                _ => None,
            }
        }
    }
}

impl Run<'_, '_> {
    /// [`crate::run::Run::char_run`] in native font `f`: words and the
    /// spaces between them (xetex.web §1034's native part, §1041–§1044).
    pub(crate) fn native_char_run(
        &mut self,
        md: &crate::build::Mode,
        f: u32,
        nf: &Arc<NativeFont>,
    ) -> R<()> {
        use crate::build::SF_UNREAD;
        use crate::val::cmd;
        let v = self.v;
        self.chars_eff = SF_UNREAD;
        let mut sf = SF_UNREAD;
        let mut out: Vec<crate::nest::Item> = Vec::new();
        let mut raws: Vec<(usize, ValueId)> = Vec::new();
        let mut space = crate::build::SpaceMemo::default();
        loop {
            match self.cur.cmd {
                cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN | cmd::CHAR_NUM => {
                    let ch = if self.cur.cmd == cmd::CHAR_NUM {
                        self.scan_usv_num()?
                    } else {
                        self.cur.chr
                    };
                    if sf != SF_UNREAD {
                        raws.push((out.len(), crate::build::sf_value(v, sf)));
                        self.chars_eff = crate::build::sf_compose(self.chars_eff, sf);
                        sf = SF_UNREAD;
                    }
                    self.flush_chars(md.lid, &mut out, std::mem::take(&mut raws));
                    let next =
                        self.native_collect(md, i64::from(f), nf, ch.cast_unsigned(), &mut sf)?;
                    if next {
                        continue;
                    }
                }
                cmd::NO_BOUNDARY => {
                    self.get_x_token();
                    self.ck()?;
                    continue;
                }
                cmd::SPACER | cmd::EX_SPACE => {
                    // (`othercases`: `check_for_post_char_toks(big_switch)`)
                    if !self.ic_post()? {
                        self.char_space(
                            f,
                            self.cur.cmd == cmd::EX_SPACE,
                            &mut sf,
                            &mut out,
                            &mut raws,
                            &mut space,
                        )?;
                    }
                }
                _ => {
                    self.back_cur();
                    break;
                }
            }
            // big_switch
            if !self.fast_char() {
                if !self.has_input() {
                    break;
                }
                if sf != SF_UNREAD {
                    raws.push((out.len(), crate::build::sf_value(v, sf)));
                    sf = SF_UNREAD;
                }
                self.flush_chars(md.lid, &mut out, std::mem::take(&mut raws));
                self.chars_eff = SF_UNREAD;
                self.get_x_token();
                self.ck()?;
            }
            if !matches!(
                self.cur.cmd,
                cmd::LETTER
                    | cmd::OTHER_CHAR
                    | cmd::CHAR_GIVEN
                    | cmd::CHAR_NUM
                    | cmd::NO_BOUNDARY
                    | cmd::SPACER
                    | cmd::EX_SPACE
            ) {
                self.back_cur();
                break;
            }
        }
        if sf != SF_UNREAD {
            raws.push((out.len(), crate::build::sf_value(v, sf)));
        }
        self.flush_chars(md.lid, &mut out, raws);
        self.chars_live = None;
        Ok(())
    }

    /// The space factor the characters so far set, made the list's
    /// (as XeTeX's `space_factor` is at once) before anything reads it.
    fn flush_sf(&mut self, md: &crate::build::Mode, sf: &mut i32) {
        use crate::build::SF_UNREAD;
        if *sf != SF_UNREAD {
            let v = self.v;
            let raws = vec![(0, crate::build::sf_value(v, *sf))];
            self.chars_eff = crate::build::sf_compose(self.chars_eff, *sf);
            *sf = SF_UNREAD;
            self.flush_chars(md.lid, &mut Vec::new(), raws);
        }
    }

    /// `collect_native` .. `collected`: the characters from `ch` on in
    /// font `f`, made words on the list. True when `cur` holds the token
    /// that ended them (`reswitch`).
    #[allow(clippy::too_many_lines)]
    fn native_collect(
        &mut self,
        md: &crate::build::Mode,
        f: i64,
        nf: &Arc<NativeFont>,
        ch: u32,
        sf: &mut i32,
    ) -> R<bool> {
        use crate::val::cmd;
        let hyph = self.font_int(f, crate::fields::FontCell::Hc)?;
        let dash = self.param("XeTeXdashbreakstate")? > 0;
        let is_break = |u: i64| u == i64::from(hyph) || (dash && (u == 0x2014 || u == 0x2013));
        let mut text: Vec<u16> = Vec::new();
        let mut main_h = 0usize;
        let mut is_hyph = false;
        let mut ch = ch;
        // (`\XeTeXinterchartoks`: the class before the first character
        // is the boundary; tokens found between two classes end the
        // characters here, `goto collected` then `big_switch`)
        self.ic_prev = crate::xetex::CLASS_BOUNDARY;
        let ic = self.param("XeTeXinterchartokenstate")? > 0;
        let reswitch = loop {
            self.adjust_sf(md, ch, sf)?;
            if ic && self.ic_check(ch, self.cur.cmd == cmd::LETTER)? {
                break false;
            }
            utf16(ch, &mut text);
            is_hyph = is_break(i64::from(ch));
            if main_h == 0 && is_hyph {
                main_h = text.len();
            }
            // (try to collect as many characters as possible in the font)
            if self.fast_char() {
                if self.cur.cmd == cmd::SPACER {
                    // (`check_for_post_char_toks(collected)`)
                    break !(ic && self.ic_post()?);
                }
                ch = self.cur.chr.cast_unsigned();
                continue;
            }
            if !self.has_input() {
                break false;
            }
            if self.next_ck()? {
                if !matches!(
                    self.cur.cmd,
                    cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN
                ) {
                    // (the space factor as XeTeX's `adjust_space_factor`
                    // left it, for what the expansion reads: acmart's
                    // `\@addpunct` after a run-in head ending in `.`)
                    self.flush_sf(md, sf);
                    self.x_token();
                    self.ck()?;
                }
                match self.cur.cmd {
                    cmd::LETTER | cmd::OTHER_CHAR | cmd::CHAR_GIVEN => {
                        ch = self.cur.chr.cast_unsigned();
                    }
                    cmd::CHAR_NUM => ch = self.scan_usv_num()?.cast_unsigned(),
                    _ => break !(ic && self.ic_post()?),
                }
            } else {
                self.get_next_or_end()?;
                break !(ic && self.ic_post()?);
            }
        };
        // (tokens before the first character: nothing collected)
        if text.is_empty() {
            return Ok(reswitch);
        }
        // collected:
        if nf.font.mapping.is_some() {
            text = nf.font.apply_mapping(&text);
            main_h = text
                .iter()
                .position(|&u| is_break(i64::from(u)))
                .map_or(0, |k| k + 1);
        }
        if self.ipar(crate::fields::Par::tracinglostchars)? > 0 {
            for c in char::decode_utf16(text.iter().copied()) {
                let c = c.map_or(0xFFFD, u32::from);
                if nf.font.map_char_to_glyph(c.cast_signed()) == 0 {
                    self.char_warning(u32::try_from(f).unwrap_or(0), c)?;
                }
            }
        }
        let lid = md.lid;
        let kind = self.word_kind()?;
        if md.m > 0 {
            let mut temp = 0usize;
            let mut main_k = text.len();
            loop {
                if main_h == 0 {
                    main_h = main_k;
                }
                if let Some(tail) = self.take_mergeable_tail(lid, f) {
                    let mut t: Vec<u16> = tail.text.to_vec();
                    t.extend_from_slice(&text[temp..temp + main_h]);
                    self.locale_words(lid, f, nf, &t, kind)?;
                    main_k = text.len() - main_h - temp;
                    temp = main_h;
                } else {
                    let t = text[temp..temp + main_h].to_vec();
                    self.locale_words(lid, f, nf, &t, kind)?;
                    temp += main_h;
                    main_k -= main_h;
                }
                main_h = 0;
                while main_h < main_k && !is_break(i64::from(text[temp + main_h])) {
                    main_h += 1;
                }
                if main_h < main_k {
                    main_h += 1;
                }
                if main_k > 0 || is_hyph {
                    self.append_node(lid, Node::Disc(Box::default()));
                }
                if main_k == 0 {
                    break;
                }
            }
        } else {
            let t = match self.take_mergeable_tail(lid, f) {
                Some(tail) => {
                    let mut t = tail.text.to_vec();
                    t.extend_from_slice(&text);
                    t
                }
                None => text,
            };
            let w = self.native_word(f, nf, &t, kind)?;
            self.append_node(lid, Node::Native(Box::new(w)));
        }
        if self.param("XeTeXinterwordspaceshaping")? > 0 {
            self.shape_interword_space(lid, f, nf)?;
        }
        Ok(reswitch)
    }

    /// The list's tail, taken off it, when it is a native word of font `f`
    /// that a word in `f` joins (xetex.web: the node before it neither a
    /// character nor a discretionary, and not the list's head).
    fn take_mergeable_tail(&mut self, lid: i64, f: i64) -> Option<Native> {
        let v = self.v;
        let fu = u32::try_from(f).unwrap_or(0);
        let l = self.level_of(lid)?;
        let mut idx = l.list.len();
        let mut tail = None;
        let mut before = None;
        while idx > 0 {
            idx -= 1;
            match seen(v, &l.list[idx], fu) {
                Seen::Skip => {}
                s if tail.is_none() => {
                    if !matches!(s, Seen::Word) {
                        return None;
                    }
                    tail = Some(idx);
                }
                s => {
                    before = Some(s);
                    break;
                }
            }
        }
        let (Some(t), Some(b)) = (tail, before) else {
            return None;
        };
        if matches!(b, Seen::CharOrDisc) {
            return None;
        }
        let it = l.list.remove(t);
        l.fold.since_bp = l.fold.since_bp.saturating_sub(1);
        native_of(v, &it)
    }

    /// `do_locale_linebreaks`: `text` as words on list `lid` (one word
    /// without a `\XeTeXlinebreaklocale`).
    fn locale_words(
        &mut self,
        lid: i64,
        f: i64,
        nf: &Arc<NativeFont>,
        text: &[u16],
        kind: NativeKind,
    ) -> R<()> {
        let v = self.v;
        let loc = self
            .get(crate::xetex::LINEBREAK_LOCALE_FIELD)?
            .map(|x| crate::val::str_of(v, x))
            .unwrap_or_default();
        if loc.is_empty() || text.len() == 1 {
            let w = self.native_word(f, nf, text, kind)?;
            self.append_node(lid, Node::Native(Box::new(w)));
            return Ok(());
        }
        // (ICU's line breaks for the locale: a penalty and the glue
        // between the parts, the glue unless zero_glue, the penalty
        // unless zero with glue)
        let skip = self.glue_param("XeTeXlinebreakskip")?;
        let use_skip = skip.width != 0 || skip.stretch != 0 || skip.shrink != 0;
        let pen = self.param("XeTeXlinebreakpenalty")?;
        let use_penalty = pen != 0 || !use_skip;
        let mut prev = 0;
        for offs in crate::xetex::line_breaks(&loc, text) {
            if offs == 0 {
                continue;
            }
            if prev != 0 {
                if use_penalty {
                    self.append_node(lid, Node::Penalty(pen));
                }
                if use_skip {
                    self.append_node(
                        lid,
                        Node::param_glue(crate::xetex::LINEBREAK_SKIP_CODE, skip),
                    );
                }
            }
            let w = self.native_word(f, nf, &text[prev..offs], kind)?;
            self.append_node(lid, Node::Native(Box::new(w)));
            prev = offs;
        }
        Ok(())
    }

    /// `\XeTeXinterwordspaceshaping`: the space between the word just
    /// appended and the word before it in the same font, measured in
    /// context; a kern after the glue where it differs from the font's
    /// space.
    fn shape_interword_space(&mut self, lid: i64, f: i64, nf: &Arc<NativeFont>) -> R<()> {
        let v = self.v;
        let space = self.font_int(f, crate::fields::FontCell::Fd(2))?;
        let Some(l) = self.level_of(lid) else {
            return Ok(());
        };
        let n = l.list.len();
        if n == 0 {
            return Ok(());
        }
        let tail = n - 1;
        let Some(pp) = (0..tail)
            .rev()
            .find(|&k| native_of(v, &l.list[k]).is_some_and(|w| w.kind != NativeKind::Glyph))
        else {
            return Ok(());
        };
        let Some(first) = native_of(v, &l.list[pp]) else {
            return Ok(());
        };
        if i64::from(first.font) != f {
            return Ok(());
        }
        let mut p = pp + 1;
        while p < tail && invisible(v, &l.list[p]) {
            p += 1;
        }
        let is_glue = match &l.list[p] {
            crate::nest::Item::N(n) => matches!(n, Node::Glue(_)),
            crate::nest::Item::V(x) => {
                matches!(
                    crate::nodes::tag(v, *x),
                    crate::nodes::T_GLUE | crate::nodes::T_SPACE
                )
            }
        };
        if !is_glue {
            return Ok(());
        }
        let mut ppp = p + 1;
        while ppp < tail && invisible(v, &l.list[ppp]) {
            ppp += 1;
        }
        if ppp != tail {
            return Ok(());
        }
        let Some(second) = native_of(v, &l.list[tail]) else {
            return Ok(());
        };
        let mut t: Vec<u16> = first.text.to_vec();
        t.push(0x20);
        t.extend_from_slice(&second.text);
        let w = self.native_word(f, nf, &t, NativeKind::Word)?;
        let d = w.width - first.width - second.width;
        if d != space {
            let k = Node::Kern(d - space, phitex_typeset::node::KernKind::Normal);
            if let Some(l) = self.level_of(lid) {
                l.list.insert(p + 1, crate::nest::Item::N(k));
                l.fold.since_bp += 1;
            }
        }
        Ok(())
    }
}

/// Whether node `n` is a native word (not a glyph) of font `f`.
pub(crate) fn is_word_of(n: &Node, f: u32) -> bool {
    matches!(n, Node::Native(w) if w.font == f && w.kind != NativeKind::Glyph)
}

/// A native font as math reads it (XeTeX's `XeTeXOTMath.cpp` over the
/// font's layout engine; `size`: the TeX font's `font_size`).
impl phitex_typeset::fonts::NativeMath for NativeFont {
    fn is_math(&self) -> bool {
        self.font.is_math_font()
    }

    fn mathsy(&self, n: u16) -> i32 {
        self.font.mathsy_param(i32::from(n), self.size)
    }

    fn mathex(&self, n: u16) -> i32 {
        self.font.mathex_param(i32::from(n), self.size)
    }

    fn constant(&self, n: u32) -> i32 {
        self.font.ot_math_constant(n)
    }

    fn map_char(&self, c: u32) -> u16 {
        u16::try_from(self.font.map_char_to_glyph(c.cast_signed()) & 0xFFFF).unwrap_or(0)
    }

    fn char_glyph(&self, c: u32) -> u16 {
        let mut units = Vec::with_capacity(2);
        utf16(c, &mut units);
        if self.font.mapping.is_some() {
            units = self.font.apply_mapping(&units);
        }
        let (layout, _) = SHAPER.with(|sh| {
            let mut sh = sh.borrow_mut();
            self.font
                .layout(&mut sh, &units, false, self.font.letter_space)
        });
        layout.glyphs.first().map_or(0, |g| g.gid)
    }

    fn glyph_whd(&self, g: u16) -> (i32, i32, i32) {
        let (w, hd) = self.font.measure_glyph(u32::from(g), true);
        let (h, d) = hd.unwrap_or((self.height_base, self.depth_base));
        (w, h, d)
    }

    fn variant(&self, g: u16, n: u32, horiz: bool) -> (u16, i32) {
        let (v, adv) = self.font.ot_math_variant(u32::from(g), n, horiz);
        (u16::try_from(v & 0xFFFF).unwrap_or(0), adv)
    }

    fn assembly(&self, g: u16, horiz: bool) -> Option<Vec<phitex_typeset::fonts::GlyphPart>> {
        self.font.ot_assembly(u32::from(g), horiz).map(|parts| {
            parts
                .into_iter()
                .map(|p| phitex_typeset::fonts::GlyphPart {
                    glyph: u16::try_from(p.glyph & 0xFFFF).unwrap_or(0),
                    start_connector: p.start_connector_length,
                    end_connector: p.end_connector_length,
                    full_advance: p.full_advance,
                    extender: p.extender,
                })
                .collect()
        })
    }

    fn min_connector_overlap(&self) -> i32 {
        self.font.ot_min_connector_overlap()
    }

    fn ital_corr(&self, g: u16) -> i32 {
        self.font.ot_math_ital_corr(u32::from(g))
    }

    fn accent_pos(&self, g: u16) -> i32 {
        self.font.ot_math_accent_pos(u32::from(g))
    }

    fn math_kern(
        &self,
        g: u16,
        script: (&dyn phitex_typeset::fonts::NativeMath, u16),
        sup: bool,
        shift: i32,
    ) -> i32 {
        let Some(sf) = script.0.as_any().downcast_ref::<NativeFont>() else {
            return 0;
        };
        let kind = if sup {
            phitex_otf::xetex::ScriptKind::Sup
        } else {
            phitex_otf::xetex::ScriptKind::Sub
        };
        self.font
            .ot_math_kern(u32::from(g), &sf.font, u32::from(script.1), kind, shift)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
