//! Material: the items commands emit (tex.web part 10's node kinds, as
//! immutable values), boxes, and the few pseudo-items a list's gatherer
//! applies (removals, the vertical list's interline-glue requests, the
//! page builder's call points). Nothing here is a list being built: a
//! command node emits items; the node that ends a paragraph or a box
//! gathers them.

use std::sync::Arc;

use crate::tfm::Font;
use crate::val::{Glue, Tok};

/// §135: how a box's glue is set.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GlueSet {
    /// 0 normal, 1 stretching, -1 shrinking.
    pub sign: i8,
    pub order: u8,
    /// TeX's `glue_set`, a double.
    pub ratio: f64,
}

impl Eq for GlueSet {}

/// §138: a rule dimension taken from the enclosing box.
pub const RUNNING: i32 = -0o10_000_000_000;

/// A box: tex.web's hlist or vlist node.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BoxNode {
    pub vertical: bool,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub shift: i32,
    pub list: Vec<Item>,
    pub glue: GlueSet,
}

/// A ligature: its character and the characters it was made of (tex.web
/// 143), with whether it began or ended at a boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lig {
    pub font: Arc<Font>,
    pub c: u8,
    pub orig: Vec<u8>,
    pub left: bool,
    pub right: bool,
}

/// A discretionary: what goes before and after a break there; the
/// `replace` items after it in the list are dropped at a break.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disc {
    pub pre: Vec<Item>,
    pub post: Vec<Item>,
    pub replace: u8,
}

/// What appending a box to a vertical list reads (tex.web 679).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interline {
    pub baselineskip: Arc<Glue>,
    pub lineskip: Arc<Glue>,
    pub lineskiplimit: i32,
}

/// What `ship_out` reads (tex.web 638–641, 617): `\count0`–`\count9`,
/// `\hoffset`, `\voffset`, `\mag` and the date (for the file's comment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShipParams {
    pub counts: [i32; 10],
    pub hoffset: i32,
    pub voffset: i32,
    pub mag: i32,
    /// `\year`, `\month`, `\day`, `\time`.
    pub date: [i32; 4],
}

/// A point where TeX calls the page builder (tex.web 994), with what it
/// reads there: the page's specifications (tex.web 987: frozen when the
/// page gets its first box), `\topskip`, and what shipping a page reads;
/// `\hsize` for `\end`'s box (tex.web 1054).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageCall {
    pub vsize: i32,
    pub maxdepth: i32,
    pub topskip: Arc<Glue>,
    pub hsize: i32,
    pub ship: Arc<ShipParams>,
}

/// An explicit `\shipout`: the box and what shipping reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shipped {
    pub b: Arc<BoxNode>,
    pub ship: Arc<ShipParams>,
}

/// An unset box (tex.web 159): an alignment's entry, or a row, before the
/// column widths are known. An entry's glue totals are its list's highest
/// order (§796); a row's are zero (§799).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Unset {
    /// A vertical list (`\valign`'s entries and rows).
    pub vertical: bool,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    /// The columns an entry spans, less one (`span_count`).
    pub span: u16,
    /// The column an entry begins in.
    pub col: u16,
    pub stretch: i32,
    pub stretch_order: u8,
    pub shrink: i32,
    pub shrink_order: u8,
    pub list: Vec<Item>,
    /// A `\halign` row: what appending it to the vertical list read
    /// (tex.web 799: `append_to_vlist`).
    pub inter: Option<Arc<Interline>>,
}

/// A command's contribution to the main vertical list, in order: vertical
/// items, then — for `\par` — a paragraph's lines (line breaking is a pure
/// function of `hlist` and `lb`, memoised by equality), then more vertical
/// items. A node that emits vertical material at the outer level carries
/// it here.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Para {
    pub before: Vec<Item>,
    pub hlist: Vec<Item>,
    /// What line breaking reads; `None`: no paragraph.
    pub lb: Option<Arc<crate::linebreak::LineParams>>,
    pub after: Vec<Item>,
}

/// An item of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Char(Arc<Font>, u8),
    Lig(Arc<Lig>),
    Box(Arc<BoxNode>),
    Rule {
        w: i32,
        h: i32,
        d: i32,
    },
    /// Glue: its spec, its subtype (0 normal; a parameter's number + 1;
    /// 98 `\nonscript`/cond math, 99 mu), and whether the spec is TeX's
    /// shared zero glue (a zero parameter or register).
    Glue {
        g: Arc<Glue>,
        sub: u8,
        zero: bool,
    },
    /// A kern: 0 a font's, 1 explicit, 2 for an accent.
    Kern {
        w: i32,
        sub: u8,
    },
    Penalty(i32),
    Disc(Arc<Disc>),
    Mark(Arc<Vec<Tok>>),
    /// An unset box: only inside an alignment, until it is set.
    Unset(Arc<Unset>),
    /// e-TeX's `\marks n` for a class `n` other than 0 (class 0 is
    /// `Mark`).
    Marks(u16, Arc<Vec<Tok>>),
    /// A math node (tex.web 147): where a formula begins (`after` false)
    /// or ends, with `\mathsurround` as its width; e-TeX's TeXXeT nodes
    /// too (etex.web §1433), `lr` being the subtype of the beginning one
    /// (0 a formula's edges, 2 `M`, 6 `\beginL`, 10 `\beginR`): the
    /// subtype is `lr + after`.
    Math {
        w: i32,
        after: bool,
        lr: u8,
    },
    /// A language whatsit (tex.web 1341): `\setlanguage`, or a change of
    /// `\language` in a paragraph (1376); the line breaker's hyphenation
    /// takes language and minima from it.
    Lang {
        lang: u8,
        lhm: u8,
        rhm: u8,
    },
    /// An entry of a math list (a noad, a style change, a choice) or what
    /// the list's gatherer applies to its last noad (`mlist::MItem`): only
    /// in the items of nodes inside a formula.
    Noad(Arc<crate::mlist::MItem>),
    // ---- pseudo-items, applied by the gatherer of the list
    /// `\unpenalty` (0), `\unkern` (1), `\unskip` (2): remove the last item
    /// if it is of that kind.
    Remove(u8),
    /// A box appended to a vertical list: the gatherer puts the interline
    /// glue before it (tex.web 679).
    Append(Arc<BoxNode>, Arc<Interline>),
    /// `\prevdepth` set in a vertical list.
    PrevDepth(i32),
    /// The page builder is called here (outer vertical list only).
    BuildPage(Arc<PageCall>),
    /// `\shipout`, in the outer vertical list's order.
    Shipout(Arc<Shipped>),
    /// `\end` (tex.web 1054).
    End(Arc<PageCall>),
    /// The hundredth error (tex.web 82: `jump_out`): the build ends, the
    /// page so far is not shipped.
    Halt,
}

impl Item {
    /// The width TeX's `hpack` gives the item (characters, boxes, rules,
    /// glue, kerns).
    pub fn width(&self) -> i32 {
        match self {
            Item::Char(f, c) => f.glyph(i32::from(*c)).map_or(0, |g| g.width),
            Item::Lig(l) => l.font.glyph(i32::from(l.c)).map_or(0, |g| g.width),
            Item::Box(b) => b.w,
            Item::Unset(u) => u.w,
            Item::Rule { w, .. } | Item::Kern { w, .. } | Item::Math { w, .. } => *w,
            Item::Glue { g, .. } => g.width,
            _ => 0,
        }
    }

    /// tex.web's `is_char_node` (characters; ligatures are nodes).
    pub fn is_char(&self) -> bool {
        matches!(self, Item::Char(..))
    }

    /// §148: `precedes_break` — a box, rule, insertion, mark, adjustment,
    /// ligature, discretionary or whatsit (types below `math_node`).
    pub fn precedes_break(&self) -> bool {
        matches!(
            self,
            Item::Box(_)
                | Item::Rule { .. }
                | Item::Mark(_)
                | Item::Marks(..)
                | Item::Lig(_)
                | Item::Disc(_)
                | Item::Lang { .. }
        )
    }

    /// §148: `non_discardable`.
    pub fn non_discardable(&self) -> bool {
        self.is_char() || self.precedes_break()
    }
}

/// A glue item from a parameter (`sub` = its number + 1).
pub fn param_glue(g: &Arc<Glue>, param: u8) -> Item {
    Item::Glue { g: g.clone(), sub: param + 1, zero: is_zero(g) }
}

/// Whether a parameter's or register's glue is TeX's shared zero glue
/// (tex.web 1229's `trap_zero_glue`).
pub fn is_zero(g: &Glue) -> bool {
    g.width == 0 && g.stretch == 0 && g.shrink == 0
}
