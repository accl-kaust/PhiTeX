//! What the algorithms ask of fonts. Fonts are not loaded here: the caller
//! answers through [`Fonts`] (tex.web's `char_info`, `lig_kern` program,
//! `param` and `font_bchar`, §539–§576).

use crate::arith::Scaled;

/// A font, as the caller numbers them (tex.web's `internal_font_number`).
pub type FontId = u32;

/// What follows a character in its font's size chain (§544).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CharTag {
    /// No successor.
    #[default]
    None,
    /// `list_tag`: the next larger character.
    List(u16),
    /// `ext_tag`: an extensible recipe; `None` pieces are absent.
    Extensible {
        /// Top piece.
        top: Option<u16>,
        /// Middle piece.
        mid: Option<u16>,
        /// Bottom piece.
        bot: Option<u16>,
        /// Repeated piece.
        rep: u16,
    },
}

/// A character's metrics (§543, §554).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CharInfo {
    /// `char_width`.
    pub width: Scaled,
    /// `char_height`.
    pub height: Scaled,
    /// `char_depth`.
    pub depth: Scaled,
    /// `char_italic`.
    pub italic: Scaled,
    /// The size chain or extensible recipe; whether the character has a
    /// lig/kern program is [`Fonts::lig_kern`]'s to answer.
    pub tag: CharTag,
}

/// The kind of a ligature instruction (§545): which of the two characters
/// are retained around the ligature, and how many of the result's
/// characters the scan passes over (`>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LigOp {
    /// `=:` op 0.
    Plain,
    /// `=:|` op 1: keep the right character.
    KeepRight,
    /// `=:|>` op 5.
    KeepRightSkip,
    /// `|=:` op 2: keep the left character.
    KeepLeft,
    /// `|=:>` op 6.
    KeepLeftSkip,
    /// `|=:|` op 3: keep both.
    KeepBoth,
    /// `|=:|>` op 7.
    KeepBothSkip1,
    /// `|=:|>>` op 11.
    KeepBothSkip2,
}

/// One lig/kern instruction for a pair of characters (§545).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LigKern {
    /// Insert a kern between the pair.
    Kern(Scaled),
    /// Form a ligature with character `result`.
    Lig(LigOp, u16),
}

/// The left side of a lig/kern lookup: a character, or the font's left
/// boundary (`bchar_label`, §545).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Left {
    /// A character.
    Char(u16),
    /// The left boundary.
    Boundary,
}

/// A font's expansion (pdfTeX's `\pdffontexpand`, `set_expand_params`):
/// the fonts it is expanded to at its limits, and the step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Expansion {
    /// `pdf_font_stretch`: the font expanded by the stretch limit, and
    /// the limit (`pdf_font_expand_ratio` of it).
    pub stretch: Option<(FontId, i32)>,
    /// `pdf_font_shrink`: the font expanded by the shrink limit, and its
    /// (negative) ratio.
    pub shrink: Option<(FontId, i32)>,
    /// `pdf_font_step`.
    pub step: i32,
}

/// A character's width, height and depth ([`Fonts::whd_table`]).
pub type Whd = (Scaled, Scaled, Scaled);

/// One part of an OpenType glyph assembly (`XeTeXOTMath.cpp`'s
/// `ot_part_*`), its lengths in scaled points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphPart {
    /// The glyph.
    pub glyph: u16,
    /// The connector lengths at its start and end.
    pub start_connector: Scaled,
    pub end_connector: Scaled,
    /// Its full advance.
    pub full_advance: Scaled,
    /// Whether it repeats.
    pub extender: bool,
}

/// XeTeX's native font as math reads it (`XeTeXOTMath.cpp`): its glyphs,
/// and, an OpenType math font, its `MATH` table.
pub trait NativeMath {
    /// `is_new_mathfont`: an OpenType font with a `MATH` table.
    fn is_math(&self) -> bool;
    /// `get_native_mathsy_param(f, n)`, `get_native_mathex_param(f, n)`.
    fn mathsy(&self, n: u16) -> Scaled;
    fn mathex(&self, n: u16) -> Scaled;
    /// `get_ot_math_constant(f, n)`.
    fn constant(&self, n: u32) -> Scaled;
    /// `map_char_to_glyph(f, c)`.
    fn map_char(&self, c: u32) -> u16;
    /// `get_native_glyph(new_native_character(f, c), 0)`.
    fn char_glyph(&self, c: u32) -> u16;
    /// A glyph node's width, height and depth
    /// (`set_native_glyph_metrics(p, 1)`).
    fn glyph_whd(&self, g: u16) -> (Scaled, Scaled, Scaled);
    /// `get_ot_math_variant(f, g, n, &adv, horiz)`: the variant and its
    /// advance (the glyph itself and -1 past the last).
    fn variant(&self, g: u16, n: u32, horiz: bool) -> (u16, Scaled);
    /// `get_ot_assembly_ptr(f, g, horiz)`'s parts.
    fn assembly(&self, g: u16, horiz: bool) -> Option<Vec<GlyphPart>>;
    /// `ot_min_connector_overlap(f)`.
    fn min_connector_overlap(&self) -> Scaled;
    /// `get_ot_math_ital_corr(f, g)`.
    fn ital_corr(&self, g: u16) -> Scaled;
    /// `get_ot_math_accent_pos(f, g)` (`"7FFFFFFF` for none).
    fn accent_pos(&self, g: u16) -> Scaled;
    /// `get_ot_math_kern(f, g, sf, sg, cmd, shift)` with `script` the
    /// script's font and glyph (`sup`: `sup_cmd`).
    fn math_kern(&self, g: u16, script: (&dyn NativeMath, u16), sup: bool, shift: Scaled) -> Scaled;
    /// The implementation, for [`NativeMath::math_kern`]'s other font.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// The caller's fonts.
pub trait Fonts {
    /// The metrics of `ch` in `font`, or `None` if it doesn't exist.
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo>;
    /// `char_width` alone (0 when the character doesn't exist): the
    /// line breaker's per-character question.
    fn char_width(&self, font: FontId, ch: u16) -> Scaled {
        self.char_info(font, ch).map_or(0, |i| i.width)
    }
    /// Each character's width, height and depth by code (0s where it
    /// doesn't exist, or past the table's end), where the fonts keep such
    /// a table: a run of characters in one font is measured by indexing
    /// it, one question per run instead of per character. `None` where
    /// they keep none ([`Fonts::char_whd`] is asked).
    fn whd_table(&self, font: FontId) -> Option<&[Whd]> {
        let _ = font;
        None
    }
    /// Width, height and depth (0s when the character doesn't exist):
    /// the packager's per-character question.
    fn char_whd(&self, font: FontId, ch: u16) -> (Scaled, Scaled, Scaled) {
        self.char_info(font, ch)
            .map_or((0, 0, 0), |i| (i.width, i.height, i.depth))
    }
    /// The lig/kern instruction for `left` followed by `right`, if any.
    fn lig_kern(&self, font: FontId, left: Left, right: u16) -> Option<LigKern>;
    /// Whether the font has a left boundary lig/kern program
    /// (`bchar_label <> non_address`).
    fn has_left_boundary(&self, font: FontId) -> bool;
    /// `font_bchar`: the right boundary character, if the font has one.
    fn bchar(&self, font: FontId) -> Option<u16>;
    /// `param(n)` (§558), 1-based; 0 when absent.
    fn param(&self, font: FontId, n: u16) -> Scaled;
    /// The number of parameters (`font_params`), for math's checks (§1195).
    fn param_count(&self, font: FontId) -> u16;
    /// XeTeX's `set_native_metrics` on a word of `like`'s font and kind
    /// whose text is `text` (the words of a run merged in a box), where
    /// the fonts can shape; `None` where they cannot.
    fn shape_native(&self, like: &crate::node::Native, text: &[u16]) -> Option<crate::node::Native> {
        let _ = (like, text);
        None
    }
    /// XeTeX's `new_native_character(f, c)` in `like`'s font (through
    /// the font's mapping), where the fonts can shape.
    fn native_char(&self, like: &crate::node::Native, c: u32) -> Option<crate::node::Native> {
        let _ = (like, c);
        None
    }
    /// XeTeX's native font `font` as math reads it, if it is one.
    fn native_math(&self, font: FontId) -> Option<&dyn NativeMath> {
        let _ = font;
        None
    }
    /// XeTeX's math families past TeX's sixteen: family `fam`'s font at
    /// `size` (0, 1, 2), where the fonts know them (`None`: none known,
    /// `Some(None)`: the null font).
    fn math_family(&self, fam: u8, size: usize) -> Option<Option<FontId>> {
        let _ = (fam, size);
        None
    }
    /// XeTeX's `cur_f` as the last formula left it (math reads it before
    /// a formula sets it), and where this formula leaves it.
    fn math_cur_f(&self) -> Option<FontId> {
        None
    }
    fn set_math_cur_f(&self, f: Option<FontId>) {
        let _ = f;
    }
    /// The font's expansion (pdfTeX), if it is expandable.
    fn expansion(&self, font: FontId) -> Option<Expansion> {
        let _ = font;
        None
    }
    /// `get_ef_code` (pdfTeX's `\efcode`).
    fn ef_code(&self, font: FontId, ch: u16) -> i32 {
        let _ = (font, ch);
        1000
    }
    /// `get_lp_code` (pdfTeX's `\lpcode`).
    fn lp_code(&self, font: FontId, ch: u16) -> i32 {
        let _ = (font, ch);
        0
    }
    /// `get_rp_code` (pdfTeX's `\rpcode`).
    fn rp_code(&self, font: FontId, ch: u16) -> i32 {
        let _ = (font, ch);
        0
    }
    /// `quad(f)`, `\fontdimen6`.
    fn quad(&self, font: FontId) -> Scaled {
        self.param(font, 6)
    }
    /// `get_expand_font(f, e)` (pdfTeX): font `font` expanded by `e`
    /// thousandths (a nonzero multiple of its step), made where it is not
    /// yet; `font` where the fonts make none.
    fn expand_font(&self, font: FontId, e: i32) -> FontId {
        let _ = e;
        font
    }
}
