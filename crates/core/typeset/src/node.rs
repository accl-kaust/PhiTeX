//! The items of horizontal and vertical lists (§133–§158), as plain data.
//!
//! One deliberate departure from tex.web's representation: a
//! discretionary owns its replacement text ([`Disc::replace`]) instead of
//! counting the nodes that follow it (`replace_count`). Everything else is
//! tex.web's node set. Marks and whatsits are opaque to these algorithms
//! and carry only the caller's handle.

use crate::arith::Scaled;
use crate::fonts::FontId;
use crate::glue::{Glue, GlueSet, Order};

/// `null_flag` (§138): a running dimension of a rule.
pub const RUNNING: Scaled = -0o10000000000;

/// A list of nodes.
pub type List = Vec<Node>;

/// Horizontal or vertical (`hlist_node`, `vlist_node`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BoxKind {
    /// `hlist_node`.
    #[default]
    H,
    /// `vlist_node`.
    V,
}

/// A box (§135).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxNode {
    /// H or V.
    pub kind: BoxKind,
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
    /// `shift_amount`.
    pub shift: Scaled,
    /// The glue setting.
    pub glue: GlueSet,
    /// Contents.
    pub list: List,
}

/// A rule (§138); [`RUNNING`] marks a running dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rule {
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
}

/// An insertion (§140).
#[derive(Clone, Debug, PartialEq)]
pub struct Insert {
    /// The insertion class (box number).
    pub number: u16,
    /// Natural height plus depth of `list`.
    pub height: Scaled,
    /// `\splitmaxdepth`.
    pub depth: Scaled,
    /// `\splittopskip`.
    pub split_top_skip: Glue,
    /// `\floatingpenalty`.
    pub float_cost: i32,
    /// The vertical list inserted.
    pub list: List,
}

/// A ligature (§143).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ligature {
    /// The font.
    pub font: FontId,
    /// The ligature character.
    pub ch: u16,
    /// The characters it replaces (`lig_ptr`).
    pub original: Vec<u16>,
    /// Formed with the left boundary (subtype bit 1).
    pub left_hit: bool,
    /// Formed with the right boundary (subtype bit 0).
    pub right_hit: bool,
}

/// A discretionary (§145).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Disc {
    /// Before the break.
    pub pre: List,
    /// After the break.
    pub post: List,
    /// Material present when there is no break here (tex.web's
    /// `replace_count` nodes following the discretionary).
    pub replace: List,
}

/// Math on or off (§147).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MathKind {
    /// `before`.
    Before,
    /// `after`.
    After,
}

/// Glue parameter codes (§224) for [`GlueKind::Param`].
pub mod param {
    /// `\lineskip`.
    pub const LINE_SKIP: u8 = 0;
    /// `\baselineskip`.
    pub const BASELINE_SKIP: u8 = 1;
    /// `\leftskip`.
    pub const LEFT_SKIP: u8 = 7;
    /// `\rightskip`.
    pub const RIGHT_SKIP: u8 = 8;
    /// `\topskip`.
    pub const TOP_SKIP: u8 = 9;
    /// `\splittopskip`.
    pub const SPLIT_TOP_SKIP: u8 = 10;
    /// `\parfillskip`.
    pub const PAR_FILL_SKIP: u8 = 14;
}

/// What a glue node is (§149).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlueKind {
    /// Ordinary glue.
    #[default]
    Normal,
    /// From the glue parameter with this code ([`param`]); tex.web's
    /// subtype is the code plus one.
    Param(u8),
    /// `cond_math_glue` (`\nonscript`).
    CondMath,
    /// `mu_glue`.
    Mu,
    /// `a_leaders` (`\leaders`).
    ALeaders,
    /// `c_leaders` (`\cleaders`).
    CLeaders,
    /// `x_leaders` (`\xleaders`).
    XLeaders,
}

/// Glue (§149).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlueNode {
    /// The specification.
    pub spec: Glue,
    /// Its kind.
    pub kind: GlueKind,
    /// Its specification is its own, a new one, not the shared
    /// `zero_glue` even when all of it is zero: glue made by `scan_glue`
    /// from a dimension (`\hskip 0pt`, `\hskip\z@`, a negated register),
    /// where a glue register or parameter set to zero is `zero_glue`
    /// itself (§1229 `trap_zero_glue`). pdfTeX's margin protrusion
    /// passes over glue only when it is `zero_glue` (`cp_skipable`).
    pub own_spec: bool,
    /// The leader box or rule, for leaders.
    pub leader: Option<Box<Node>>,
}

/// What a kern is (§155).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KernKind {
    /// Implicit (font kern).
    #[default]
    Normal,
    /// `\kern` or italic correction.
    Explicit,
    /// For an accent.
    AccKern,
    /// `\mkern`.
    Mu,
    /// pdfTeX's `margin_kern_node`: a character's protrusion at a line's
    /// edge, with the character (`margin_char`).
    Margin {
        /// `left_side` (else `right_side`).
        left: bool,
        /// The character's font.
        font: FontId,
        /// The character.
        ch: u16,
    },
}

/// An unset box of an alignment (§159).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unset {
    /// Natural width (height in a vertical alignment).
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
    /// Number of columns spanned, minus one.
    pub span_count: u16,
    /// Total stretch.
    pub stretch: Scaled,
    /// Its order.
    pub stretch_order: Order,
    /// Total shrink.
    pub shrink: Scaled,
    /// Its order.
    pub shrink_order: Order,
    /// Contents.
    pub list: List,
}

/// The size of a whatsit that takes room (pdfTeX's `pdf_width`,
/// `pdf_height`, `pdf_depth` of a `\pdfrefxform` or `\pdfrefximage`
/// node).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WhatsitSize {
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
}

/// One glyph of a [`Native`] node: its id and its position from the
/// node's origin (scaled points; `y` down is negative, as XeTeX keeps
/// it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Glyph {
    /// The glyph id.
    pub gid: u16,
    /// Its x offset.
    pub x: Scaled,
    /// Its y offset.
    pub y: Scaled,
}

/// What a [`Native`] node is (XeTeX's whatsit subtypes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NativeKind {
    /// `native_word_node`: a word shaped in a native font.
    #[default]
    Word,
    /// `native_word_node_AT`: a word whose text the output carries too
    /// (`\XeTeXgenerateactualtext`).
    WordText,
    /// `glyph_node`: one glyph by its id (`\XeTeXglyph`, math).
    Glyph,
}

/// XeTeX's native word or glyph: a box-like item of a font the engine
/// shapes outside TeX, its glyphs laid out (docs/spec-xetex.md §4.1).
/// To the algorithms here it is its width, height and depth (XeTeX's
/// "incorporate a whatsit node" cases); its text is for hyphenation and
/// the output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Native {
    /// Word, word with its text, or glyph.
    pub kind: NativeKind,
    /// The font.
    pub font: FontId,
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
    /// The text, in UTF-16 units as XeTeX keeps it (empty for a glyph).
    pub text: std::sync::Arc<[u16]>,
    /// The glyphs as laid out.
    pub glyphs: std::sync::Arc<[Glyph]>,
}

/// A list item.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A character (§134).
    Char {
        /// Font.
        font: FontId,
        /// Character code.
        ch: u16,
    },
    /// A box (apart: a list is mostly characters, which a narrow node
    /// keeps small to move and store; a box is made once per line).
    Box(Box<BoxNode>),
    /// A rule.
    Rule(Rule),
    /// An insertion.
    Ins(Box<Insert>),
    /// A mark: opaque, the caller's handle.
    Mark(u32),
    /// `\vadjust` material.
    Adjust(List),
    /// A ligature.
    Lig(Ligature),
    /// A discretionary (apart, as a box).
    Disc(Box<Disc>),
    /// A whatsit: opaque, the caller's handle (§1341), and its size if
    /// it has one: pdfTeX's `\pdfrefxform` and `\pdfrefximage` nodes
    /// carry the form's or image's dimensions, which packaging, line
    /// breaking and the page builder count (pdfTeX §1365, "Incorporate a
    /// whatsit node into an hbox/vbox", "Prepare to move whatsit p to the
    /// current page"); other whatsits have none (§1360).
    Whatsit(u32, Option<WhatsitSize>),
    /// A language whatsit (§1341): `\language`, `\lefthyphenmin`,
    /// `\righthyphenmin` for what follows; no dimensions.
    Language {
        /// Language number.
        lang: u16,
        /// Left hyphen minimum.
        lhm: u8,
        /// Right hyphen minimum.
        rhm: u8,
    },
    /// Math on/off with its surround width.
    Math(MathKind, Scaled),
    /// Glue.
    Glue(GlueNode),
    /// A kern.
    Kern(Scaled, KernKind),
    /// A penalty.
    Penalty(i32),
    /// An unset box.
    Unset(Box<Unset>),
    /// XeTeX's native word or glyph (apart: never in a pdfTeX list).
    Native(Box<Native>),
}

// (a list is a run of these, mostly characters: the large variants apart
// keep it narrow, five words)
const _: () = assert!(std::mem::size_of::<Node>() <= 40);

impl Node {
    /// A plain glue node.
    #[must_use]
    pub fn glue(spec: Glue) -> Node {
        Node::Glue(GlueNode {
            spec,
            kind: GlueKind::Normal,
            own_spec: false,
            leader: None,
        })
    }

    /// A glue node from a parameter.
    #[must_use]
    pub fn param_glue(code: u8, spec: Glue) -> Node {
        Node::Glue(GlueNode {
            spec,
            kind: GlueKind::Param(code),
            own_spec: false,
            leader: None,
        })
    }

    /// `precedes_break` (§148): `type < math_node`.
    #[must_use]
    pub fn precedes_break(&self) -> bool {
        !self.is_discardable()
    }

    /// A box, rule or unset node's (width, height, depth, shift); `None`
    /// for other nodes. Rules and unset boxes have no shift.
    #[must_use]
    pub fn box_dims(&self) -> Option<(Scaled, Scaled, Scaled, Scaled)> {
        match self {
            Node::Box(b) => Some((b.width, b.height, b.depth, b.shift)),
            Node::Rule(r) => Some((r.width, r.height, r.depth, 0)),
            Node::Unset(u) => Some((u.width, u.height, u.depth, 0)),
            _ => None,
        }
    }

    /// Whether this is a discardable item: glue, kern, penalty, math
    /// (`non_discardable` is `type < math_node`, §148; unset nodes
    /// are past `math_node` too).
    #[must_use]
    pub fn is_discardable(&self) -> bool {
        matches!(
            self,
            Node::Math(..) | Node::Glue(_) | Node::Kern(..) | Node::Penalty(_) | Node::Unset(_)
        )
    }
}
