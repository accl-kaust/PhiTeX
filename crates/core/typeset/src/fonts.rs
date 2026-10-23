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
    fn whd_table(&self, font: FontId) -> Option<&[(Scaled, Scaled, Scaled)]> {
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
}
