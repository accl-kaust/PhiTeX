//! Typesetting math formulas (tex.web parts 34–36, §680–§767).
//!
//! An mlist is a `Vec<Item>` of typed noads and nodes; [`mlist_to_hlist`]
//! turns it into an hlist, as a function of the list, the style, the
//! parameters it reads and the fonts of the math families. Its boxes are
//! packed by the caller ([`Env::hpack`], [`Env::vpack`]): `hpack` (§649)
//! and `vpack` (§668) are routines of their own, which set `\badness`
//! and are recorded calls (DESIGN 7.17.2).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::Scaled;
use crate::font::{Glyph, LigKern, Tag};
use crate::native::GlyphNode;
use crate::node::{BoxNode, FontId, GlueSign, GlueSpec, Node, Order, RUNNING, Whatsit, push_char};
use crate::pack::{self, Confusion, Fonts, Spec};
use crate::scaled::xn_over_d;

/// §157
const INF_PENALTY: i32 = 10000;
/// `XeTeX` §132: `min_halfword`, which is `null`.
const MIN_HALFWORD: Scaled = -0xFFF_FFFF;
/// §679: `default_code`, the thickness of `\over`.
pub const DEFAULT_CODE: Scaled = 0o10000000000;
/// §155, §149: glue and kern subtypes in mlists.
pub const COND_MATH_GLUE: u8 = 98;
pub const MU_GLUE: u8 = 99;
const EXPLICIT: u8 = 1;
/// `XeTeX` §729: accent subtypes (`\Umathaccent fixed`, `bottom`).
pub const FIXED_ACC: u8 = 1;
pub const BOTTOM_ACC: u8 = 2;
/// `get_ot_math_accent_pos` without an attachment point.
pub const NO_ACCENT_POS: Scaled = 0x7FFF_FFFF;
/// `XeTeX` §742: the OpenType `MATH` constants math reads.
const DISPLAY_OPERATOR_MIN_HEIGHT: u32 = 3;
const ACCENT_BASE_HEIGHT: u32 = 6;
const SUBSCRIPT_TOP_MAX: u32 = 9;
const SUPERSCRIPT_BOTTOM_MIN: u32 = 13;
const SUB_SUPERSCRIPT_GAP_MIN: u32 = 15;
const SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT: u32 = 16;
const STACK_GAP_MIN: u32 = 26;
const STACK_DISPLAY_STYLE_GAP_MIN: u32 = 27;
const FRACTION_NUMERATOR_GAP_MIN: u32 = 36;
const FRACTION_NUM_DISPLAY_STYLE_GAP_MIN: u32 = 37;
const FRACTION_DENOMINATOR_GAP_MIN: u32 = 39;
const FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN: u32 = 40;
const RADICAL_VERTICAL_GAP: u32 = 49;
const RADICAL_DISPLAY_STYLE_VERTICAL_GAP: u32 = 50;
const RADICAL_RULE_THICKNESS: u32 = 51;
/// §688: styles.
pub const DISPLAY: u8 = 0;
pub const TEXT: u8 = 2;
pub const SCRIPT: u8 = 4;
pub const SCRIPT_SCRIPT: u8 = 6;
/// §699: sizes.
const TEXT_SIZE: usize = 0;
const SCRIPT_SIZE: usize = crate::web::SCRIPT_SIZE as usize;
const SCRIPT_SCRIPT_SIZE: usize = crate::web::SCRIPT_SCRIPT_SIZE as usize;
/// §224: `thin_mu_skip_code`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const THIN_MU_SKIP: u8 = crate::web::THIN_MU_SKIP_CODE as u8;

/// §764: `math_spacing`, indexed by `(r_type-ord)*8+(t-ord)`.
const MATH_SPACING: &[u8; 64] = b"02340001\
      22*40001\
      33**3**3\
      44*04004\
      00*00000\
      02340001\
      11*11111\
      12341011";

/// §702
const fn cramped(c: u8) -> u8 {
    2 * (c / 2) + 1
}
const fn sub_style(c: u8) -> u8 {
    2 * (c / 4) + SCRIPT + 1
}
const fn sup_style(c: u8) -> u8 {
    2 * (c / 4) + SCRIPT + (c % 2)
}
const fn num_style(c: u8) -> u8 {
    c + 2 - 2 * (c / 6)
}
const fn denom_style(c: u8) -> u8 {
    2 * (c / 2) + 1 + 2 - 2 * (c / 6)
}

/// §100
fn half(x: Scaled) -> Scaled {
    if x % 2 != 0 { (x + 1) / 2 } else { x / 2 }
}

/// §683: a delimiter field (`XeTeX`: characters of 21 bits, a family of
/// 8, and `\Udelimiter`'s one size with no large variant).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u32,
    pub large_fam: u8,
    pub large_char: u32,
}

crate::persist_struct!(Delim {
    small_fam,
    small_char,
    large_fam,
    large_char
});

/// §681: a noad field (`XeTeX`: a character of 21 bits, its plane kept
/// with the family, `plane_and_fam_field`).
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub enum Field {
    #[default]
    Empty,
    Char {
        fam: u8,
        ch: u32,
    },
    /// A character in mid-word (§752): no italic correction.
    TextChar {
        fam: u8,
        ch: u32,
    },
    Box(Arc<BoxNode>),
    Mlist(Vec<Item>),
}

crate::persist_enum!(Field { Empty, Char { fam, ch }, TextChar { fam, ch }, Box(a0), Mlist(a0) });

/// §682: `\limits`, `\nolimits`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Limits {
    #[default]
    Normal,
    Limits,
    NoLimits,
}

crate::persist_enum!(Limits {
    Normal,
    Limits,
    NoLimits
});

/// §682–§687: the kinds of noad.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Kind {
    Ord,
    Op(Limits),
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
    Radical(Delim),
    /// The numerator is `sup`, the denominator `sub`.
    Fraction {
        thickness: Scaled,
        left: Delim,
        right: Delim,
    },
    Under,
    Over,
    /// `sub`: `XeTeX`'s subtype, [`FIXED_ACC`] and [`BOTTOM_ACC`]
    /// (`\Umathaccent fixed`, `bottom`).
    Accent {
        fam: u8,
        ch: u32,
        sub: u8,
    },
    Vcenter,
    Left(Delim),
    Right(Delim),
    /// e-TeX's `\middle`: a right noad that another group follows.
    Middle(Delim),
}

crate::persist_enum!(Kind { Ord, Op(a0), Bin, Rel, Open, Close, Punct, Inner, Radical(a0), Fraction { thickness, left, right }, Under, Over, Accent { fam, ch, sub }, Vcenter, Left(a0), Right(a0), Middle(a0) });

impl Kind {
    /// tex.web's type code (§682–§687).
    fn code(&self) -> i32 {
        match self {
            Kind::Ord => 16,
            Kind::Op(_) => 17,
            Kind::Bin => 18,
            Kind::Rel => 19,
            Kind::Open => 20,
            Kind::Close => 21,
            Kind::Punct => 22,
            Kind::Inner => 23,
            Kind::Radical(_) => 24,
            Kind::Fraction { .. } => 25,
            Kind::Under => 26,
            Kind::Over => 27,
            Kind::Accent { .. } => 28,
            Kind::Vcenter => 29,
            Kind::Left(_) => 30,
            Kind::Right(_) | Kind::Middle(_) => 31,
        }
    }
}

const ORD: i32 = 16;
const OP: i32 = 17;
const BIN: i32 = 18;
const REL: i32 = 19;
const OPEN: i32 = 20;
const PUNCT: i32 = 22;
const INNER: i32 = 23;
const LEFT: i32 = 30;
const RIGHT: i32 = 31;
const PENALTY_NODE: i32 = 12;

#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Noad {
    pub kind: Kind,
    pub nucleus: Field,
    pub sup: Field,
    pub sub: Field,
}

crate::persist_struct!(Noad {
    kind,
    nucleus,
    sup,
    sub
});

impl Noad {
    #[must_use]
    pub fn new(kind: Kind) -> Noad {
        Noad {
            kind,
            nucleus: Field::Empty,
            sup: Field::Empty,
            sub: Field::Empty,
        }
    }
}

/// An mlist entry.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Item {
    Noad(Box<Noad>),
    /// §688: a style change.
    Style(u8),
    /// §689: `\mathchoice`: display, text, script, scriptscript.
    Choice(Box<[Vec<Item>; 4]>),
    /// A node that may appear in an mlist: rule, glue, kern, penalty,
    /// discretionary, whatsit, mark, insertion, adjustment.
    Node(Node),
}

crate::persist_enum!(Item { Noad(a0), Style(a0), Choice(a0), Node(a0) });

/// What the formula reads besides its fonts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Params {
    pub script_space: Scaled,
    pub null_delimiter_space: Scaled,
    pub delimiter_factor: i32,
    pub delimiter_shortfall: Scaled,
    pub bin_op_penalty: i32,
    pub rel_penalty: i32,
    pub thin_mu_skip: GlueSpec,
    pub med_mu_skip: GlueSpec,
    pub thick_mu_skip: GlueSpec,
}

/// What kind of font a math character's family holds (`XeTeX`'s
/// `is_native_font`, `is_ot_font`, `is_new_mathfont`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// A TFM font.
    #[default]
    Tfm,
    /// An OpenType font without a `MATH` table.
    Ot,
    /// An OpenType math font.
    OtMath,
}

impl FontKind {
    fn is_native(self) -> bool {
        self != FontKind::Tfm
    }
    fn is_new_mathfont(self) -> bool {
        self == FontKind::OtMath
    }
}

/// One part of an OpenType glyph assembly (`XeTeXOTMath.cpp`'s
/// `ot_part_*`), its lengths scaled to the font.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphPart {
    pub glyph: u16,
    pub start_connector: Scaled,
    pub end_connector: Scaled,
    pub full_advance: Scaled,
    pub extender: bool,
}

/// The fonts of the math families.
///
/// The methods with defaults are `XeTeX`'s native fonts (none in TeX and
/// pdfTeX: every font a TFM font).
pub trait Env: Fonts {
    /// `\textfont`, `\scriptfont`, `\scriptscriptfont` by `fam+size`
    /// (`None`: `\nullfont`).
    fn fam_fnt(&self, n: usize) -> Option<FontId>;
    /// `\fontdimen k` of `f`, as possibly changed.
    fn param(&self, f: FontId, k: usize) -> Scaled;
    fn skew_char(&self, f: FontId) -> i32;
    /// §649: `hpack(list, spec)`. Math packs only to its natural width
    /// or, with infinite glue, to an exact one (§715), so TeX's packing
    /// reports nothing here.
    fn hpack(&mut self, list: Vec<Node>, spec: Spec) -> BoxNode;
    /// §668: `vpack(list, natural)`.
    ///
    /// # Errors
    /// A confusion (a character node in a vlist), after which the math
    /// is abandoned.
    fn vpack(&mut self, list: Vec<Node>) -> Result<BoxNode, Confusion>;

    /// Report `events` now (before what comes next, a native font's
    /// warning).
    fn report(&mut self, events: Vec<Event>);
    /// Whether this is `XeTeX` (whose math looks ahead at a script's
    /// first character, `XeTeX` §805).
    fn xetex(&self) -> bool {
        false
    }
    /// `XeTeX`'s `cur_f` before the formula (a global the last formula
    /// left), and after it.
    fn cur_f(&self) -> Option<FontId> {
        None
    }
    fn set_cur_f(&mut self, _f: Option<FontId>) {}
    fn font_kind(&self, _f: FontId) -> FontKind {
        FontKind::Tfm
    }
    /// `get_native_mathsy_param(f, k)`, `get_native_mathex_param(f, k)`.
    fn native_mathsy(&self, _f: FontId, _k: usize) -> Scaled {
        0
    }
    fn native_mathex(&self, _f: FontId, _k: usize) -> Scaled {
        0
    }
    /// `get_ot_math_constant(f, k)`.
    fn ot_math_constant(&self, _f: FontId, _k: u32) -> Scaled {
        0
    }
    /// `map_char_to_glyph(f, c)`.
    fn map_char_to_glyph(&self, _f: FontId, _c: u32) -> u16 {
        0
    }
    /// `get_native_glyph(new_native_character(f, c), 0)`: the glyph of
    /// character `c` laid out in native font `f` (warning of a character
    /// the font lacks, as `new_native_character` does).
    fn native_char_glyph(&mut self, _f: FontId, _c: u32) -> u16 {
        0
    }
    /// A `glyph_node` of glyph `g` with `set_native_glyph_metrics(p, 1)`.
    fn glyph_node(&self, f: FontId, g: u16) -> GlyphNode {
        GlyphNode {
            font: f,
            gid: g,
            width: 0,
            height: 0,
            depth: 0,
        }
    }
    /// `get_ot_math_variant(f, g, n, &adv, horiz)`: the variant and its
    /// advance, -1 past the last.
    fn ot_math_variant(&self, _f: FontId, g: u16, _n: u32, _horiz: bool) -> (u16, Scaled) {
        (g, -1)
    }
    /// `get_ot_assembly_ptr(f, g, horiz)`.
    fn ot_assembly(&self, _f: FontId, _g: u16, _horiz: bool) -> Option<Vec<GlyphPart>> {
        None
    }
    fn ot_min_connector_overlap(&self, _f: FontId) -> Scaled {
        0
    }
    fn ot_math_ital_corr(&self, _f: FontId, _g: u16) -> Scaled {
        0
    }
    /// `get_ot_math_accent_pos(f, g)` (`"7FFFFFFF`: none).
    fn ot_math_accent_pos(&self, _f: FontId, _g: u16) -> Scaled {
        NO_ACCENT_POS
    }
    /// `get_ot_math_kern(f, g, sf, sg, cmd, shift)`; `sup`: `sup_cmd`.
    fn ot_math_kern(
        &self,
        _f: FontId,
        _g: u16,
        _sf: Option<FontId>,
        _sg: u16,
        _sup: bool,
        _shift: Scaled,
    ) -> Scaled {
        0
    }
}

/// What the caller reports, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// §723: "\textfont 3 is undefined (character x)"; `size` is 0, 256
    /// or 512 (`script_size`, 4.7).
    UndefinedFamily { size: u16, fam: u8, ch: u32 },
    /// §581: the character is not in the font.
    MissingChar { font: FontId, ch: u32 },
}

struct Ctx<'a, E: Env> {
    env: &'a mut E,
    params: &'a Params,
    events: Vec<Event>,
    style: u8,
    size: usize,
    mu: Scaled,
    /// §767: `cur_f`, the font of the last character fetched (`None`:
    /// `null_font`), which `XeTeX` reads after the fact.
    cur_f: Option<FontId>,
}

/// §765: what `fetch` found (`cur_f` and `cur_i`).
#[derive(Clone, Copy)]
enum Fetched {
    /// An undefined family or a missing character: `cur_i` null, the
    /// field emptied.
    None,
    Tfm(FontId, Glyph),
    /// A native font's (`cur_i` null too).
    Native(FontId),
}

fn kern(width: Scaled) -> Node {
    Node::Kern {
        width,
        subtype: 0,
        sync: crate::origin::Side(0),
    }
}

/// §704: `fraction_rule`.
fn rule(t: Scaled) -> Node {
    Node::Rule {
        width: RUNNING,
        height: t,
        depth: 0,
        sync: crate::origin::Side(0),
    }
}

fn boxed(b: BoxNode) -> Node {
    Node::Box(b.share())
}

fn unbox(b: Arc<BoxNode>) -> BoxNode {
    Arc::try_unwrap(b).unwrap_or_else(|b| (*b).clone())
}

/// `XeTeX`'s `glyph_node`.
fn glyph_node(g: GlyphNode) -> Node {
    Node::Whatsit(Box::new(Whatsit::Glyph(g)))
}

/// `XeTeX`'s `is_glyph_node`: the glyph of a node, if it is one.
fn glyph_of(n: Option<&Node>) -> Option<GlyphNode> {
    match n {
        Some(Node::Whatsit(w)) => match &**w {
            Whatsit::Glyph(g) => Some(*g),
            _ => None,
        },
        _ => None,
    }
}

/// A glyph identifier (`XeTeX` keeps them in 16 bits).
fn glyph_id(x: u32) -> u16 {
    u16::try_from(x & 0xFFFF).unwrap_or(0)
}

/// `XeTeX` §749 `stack_glyph_into_box`: glyph `g` after the others in an
/// hlist (the first one's height and depth not counted), under them in
/// a vlist.
fn stack_glyph_into_box(b: &mut BoxNode, g: GlyphNode) {
    if b.vertical {
        b.list.insert(0, glyph_node(g));
        b.height = g.height;
        b.width = b.width.max(g.width);
    } else {
        if !b.list.is_empty() {
            b.height = b.height.max(g.height);
            b.depth = b.depth.max(g.depth);
        }
        b.list.push(glyph_node(g));
    }
}

/// `XeTeX` §749 `stack_glue_into_box`: glue of width `min` stretching to
/// `max`. In a vlist it sets the box's width to the glue node's `width`,
/// a word of the node that holds its `leader_ptr`: `null`.
fn stack_glue_into_box(b: &mut BoxNode, min: Scaled, max: Scaled) {
    let g = Node::Glue {
        spec: GlueSpec {
            width: min,
            stretch: max - min,
            ..GlueSpec::default()
        },
        subtype: 0,
        sync: crate::origin::Side(0),
    };
    if b.vertical {
        b.list.insert(0, g);
        b.width = MIN_HALFWORD;
    } else {
        b.list.push(g);
    }
}

/// Append `list` to `out`, keeping glyph runs canonical.
fn append(out: &mut Vec<Node>, list: Vec<Node>) {
    out.reserve(list.len());
    for n in list {
        match n {
            // a run that cannot merge with the last one stays canonical
            Node::Glyphs(g) if !matches!(out.last(), Some(Node::Glyphs(h)) if h.font == g.font && !h.is_full()) =>
            {
                out.push(Node::Glyphs(g));
            }
            Node::Glyphs(g) => {
                for &c in g.chars() {
                    push_char(out, g.font, c);
                }
            }
            n => out.push(n),
        }
    }
}

/// §726: convert `mlist` in `style` to an hlist; `penalties`: insert
/// penalties after binary operators and relations.
pub fn mlist_to_hlist(
    mlist: Vec<Item>,
    style: u8,
    penalties: bool,
    params: &Params,
    env: &mut impl Env,
) -> Result<(Vec<Node>, Vec<Event>), Confusion> {
    let cur_f = env.cur_f();
    let mut cx = Ctx {
        env,
        params,
        events: Vec::new(),
        style,
        size: 0,
        mu: 0,
        cur_f,
    };
    let list = cx.mlist_to_hlist(mlist, style, penalties)?;
    let cur_f = cx.cur_f;
    cx.env.set_cur_f(cur_f);
    Ok((list, cx.events))
}

/// A first-pass result.
enum Done {
    Noad {
        code: i32,
        hlist: Vec<Node>,
        delim: Delim,
    },
    Style(u8),
    Node(Node),
}

impl<E: Env> Ctx<'_, E> {
    fn glyph(&self, f: FontId, c: u32) -> Option<Glyph> {
        i32::try_from(c)
            .ok()
            .and_then(|c| self.env.font(f).glyph(c))
    }

    fn kind(&self, f: Option<FontId>) -> FontKind {
        f.map_or(FontKind::Tfm, |f| self.env.font_kind(f))
    }

    /// `XeTeX`: `is_new_mathfont(cur_f)`.
    fn cur_mathfont(&self) -> bool {
        self.kind(self.cur_f).is_new_mathfont()
    }

    /// `XeTeX` §742: `mathsy(k)` of family 2 in size `size`, an OpenType
    /// math font's from its constants.
    fn mathsy(&self, k: usize, size: usize) -> Scaled {
        match self.env.fam_fnt(2 + size) {
            Some(f) if self.env.font_kind(f).is_new_mathfont() => self.env.native_mathsy(f, k),
            Some(f) => self.env.param(f, k),
            None => 0,
        }
    }
    fn math_x_height(&self, s: usize) -> Scaled {
        self.mathsy(5, s)
    }
    fn axis_height(&self, s: usize) -> Scaled {
        self.mathsy(22, s)
    }
    /// `XeTeX` §743: `mathex(k)` of family 3 in the current size.
    fn mathex(&self, k: usize) -> Scaled {
        match self.env.fam_fnt(3 + self.size) {
            Some(f) if self.env.font_kind(f).is_new_mathfont() => self.env.native_mathex(f, k),
            Some(f) => self.env.param(f, k),
            None => 0,
        }
    }
    fn default_rule_thickness(&self) -> Scaled {
        self.mathex(8)
    }

    /// `new_native_character`'s glyph, after the events so far (its
    /// warning comes after theirs).
    fn native_char_glyph(&mut self, f: FontId, c: u32) -> u16 {
        if !self.events.is_empty() {
            let e = core::mem::take(&mut self.events);
            self.env.report(e);
        }
        self.env.native_char_glyph(f, c)
    }

    /// `get_ot_math_constant(f, k)` of `cur_f`.
    fn cur_constant(&self, k: u32) -> Scaled {
        self.cur_f.map_or(0, |f| self.env.ot_math_constant(f, k))
    }

    /// §703
    fn set_size_and_mu(&mut self) {
        self.size = if self.style < SCRIPT {
            TEXT_SIZE
        } else {
            SCRIPT_SIZE * usize::from((self.style - TEXT) / 2)
        };
        self.mu = self.mathsy(6, self.size) / 18;
    }

    /// §649: `hpack(list, natural)`.
    fn hpack(&mut self, list: Vec<Node>) -> BoxNode {
        self.env.hpack(list, Spec::NATURAL)
    }

    /// §668: `vpack(list, natural)`.
    fn vpack(&mut self, list: Vec<Node>) -> Result<BoxNode, Confusion> {
        self.env.vpack(list)
    }

    /// §705
    fn overbar(&mut self, b: BoxNode, k: Scaled, t: Scaled) -> Result<BoxNode, Confusion> {
        self.vpack(vec![kern(t), rule(t), kern(k), boxed(b)])
    }

    /// §709: a box holding character `c` of TFM font `f`, its width
    /// with the italic correction.
    fn char_box(&self, f: FontId, c: u8) -> BoxNode {
        let g = self.glyph(f, u32::from(c)).unwrap_or_default();
        BoxNode {
            width: g.width + g.italic,
            height: g.height,
            depth: g.depth,
            list: vec![Node::Glyphs(crate::node::Glyphs::one(f, c))],
            ..BoxNode::default()
        }
    }

    fn height_plus_depth(&self, f: FontId, c: u8) -> Scaled {
        let g = self.glyph(f, u32::from(c)).unwrap_or_default();
        g.height + g.depth
    }

    /// §706 (`XeTeX` §749): a delimiter of size at least `v` for `d` in
    /// size `s`. In an OpenType font the character's vertical variants
    /// are looked at, then its assembly.
    fn var_delimiter(&mut self, d: Delim, s: usize, v: Scaled) -> Result<BoxNode, Confusion> {
        // the best so far: font, character (a glyph in an OpenType font)
        let mut f: Option<FontId> = None;
        let mut c: u32 = 0;
        let mut ext: Option<u8> = None;
        let mut assembly: Option<Vec<GlyphPart>> = None;
        let mut w = 0;
        'found: for (z0, x0) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // §707: look at the variants of `(z,x)` (`XeTeX`: `x` is a
            // quarterword).
            let mut x = x0 & 0xFFFF;
            if z0 != 0 || x != 0 {
                let mut z = usize::from(z0) + s + SCRIPT_SIZE;
                loop {
                    z -= SCRIPT_SIZE;
                    if let Some(g) = self.env.fam_fnt(z) {
                        if self.env.font_kind(g).is_native() {
                            // `XeTeX` §751
                            x = u32::from(self.env.map_char_to_glyph(g, x));
                            f = Some(g);
                            c = x;
                            w = 0;
                            let mut n = 0;
                            loop {
                                let (y, u) = self.env.ot_math_variant(g, glyph_id(x), n, false);
                                if u > w {
                                    c = u32::from(y);
                                    w = u;
                                    if u >= v {
                                        break 'found;
                                    }
                                }
                                n += 1;
                                if u < 0 {
                                    break;
                                }
                            }
                            // no glyph big enough: is the character extensible?
                            assembly = self.env.ot_assembly(g, glyph_id(x), false);
                            if assembly.is_some() {
                                break 'found;
                            }
                        } else {
                            // §708
                            let mut y = x;
                            while let Some(q) = self.glyph(g, y) {
                                if let Tag::Ext(r) = q.tag {
                                    f = Some(g);
                                    c = y;
                                    ext = Some(r);
                                    break 'found;
                                }
                                let u = q.height + q.depth;
                                if u > w {
                                    f = Some(g);
                                    c = y;
                                    w = u;
                                    if u >= v {
                                        break 'found;
                                    }
                                }
                                let Tag::List(next) = q.tag else {
                                    break;
                                };
                                y = u32::from(next);
                            }
                        }
                    }
                    if z < SCRIPT_SIZE {
                        break;
                    }
                }
            }
        }
        let mut b = match f {
            None => BoxNode {
                width: self.params.null_delimiter_space,
                ..BoxNode::default()
            },
            Some(f) if !self.env.font_kind(f).is_native() => match ext {
                Some(r) => self.extensible(f, r, v)?,
                None => self.char_box(f, u8::try_from(c).unwrap_or(0)),
            },
            Some(f) => {
                if let Some(a) = assembly {
                    self.opentype_assembly(f, &a, v, false)
                } else {
                    let g = self.env.glyph_node(f, glyph_id(c));
                    BoxNode {
                        vertical: true,
                        width: g.width,
                        height: g.height,
                        depth: g.depth,
                        list: vec![glyph_node(g)],
                        ..BoxNode::default()
                    }
                }
            }
        };
        b.shift = half(b.height - b.depth) - self.axis_height(s);
        Ok(b)
    }

    /// `XeTeX` §749 `build_opentype_assembly`: a box at least `s` high
    /// (wide if `horiz`) of the parts `a` of font `f`, as few copies of
    /// the extenders as do, their overlaps glue that stretches.
    fn opentype_assembly(&mut self, f: FontId, a: &[GlyphPart], s: Scaled, horiz: bool) -> BoxNode {
        let mut b = BoxNode {
            vertical: !horiz,
            ..BoxNode::default()
        };
        // how many repeats of each extender to use
        let min_o = self.env.ot_min_connector_overlap(f);
        let mut n: u32 = 0;
        loop {
            let mut no_extenders = true;
            let mut s_max: Scaled = 0;
            let mut prev_o = 0;
            for p in a {
                let reps = if p.extender {
                    no_extenders = false;
                    n
                } else {
                    1
                };
                for _ in 0..reps {
                    let o = p.start_connector.min(min_o).min(prev_o);
                    s_max = s_max.wrapping_sub(o).wrapping_add(p.full_advance);
                    prev_o = p.end_connector;
                }
            }
            // (`XeTeX` loops for ever on extenders that add nothing)
            if s_max >= s || no_extenders || n >= 1 << 16 {
                break;
            }
            n += 1;
        }
        // the box, with glue wherever an overlap occurs
        let mut prev_o = 0;
        for p in a {
            for _ in 0..if p.extender { n } else { 1 } {
                let mut o = p.start_connector.min(prev_o);
                let oo = o; // the maximum overlap
                o = o.min(min_o);
                if oo > 0 {
                    stack_glue_into_box(&mut b, -oo, -o);
                }
                let g = self.env.glyph_node(f, p.glyph);
                stack_glyph_into_box(&mut b, g);
                prev_o = p.end_connector;
            }
        }
        // its natural size and stretch, and the glue set to reach `s`
        let (mut nat, mut stretch) = (0, 0);
        for p in &b.list {
            match p {
                Node::Whatsit(w) => {
                    if let Whatsit::Glyph(g) = &**w {
                        nat += if horiz { g.width } else { g.height + g.depth };
                    }
                }
                Node::Glue { spec, .. } => {
                    nat += spec.width;
                    stretch += spec.stretch;
                }
                _ => {}
            }
        }
        let size = if s > nat && stretch > 0 {
            let o = (s - nat).min(stretch); // don't stretch more than `stretch`
            b.glue_order = Order::Normal;
            b.glue_sign = GlueSign::Stretching;
            b.glue_set = f64::from(o) / f64::from(stretch);
            nat + crate::scaled::zround(f64::from(stretch) * b.glue_set)
        } else {
            nat
        };
        if horiz {
            b.width = size;
        } else {
            b.height = size;
        }
        b
    }

    /// §713: an extensible character of font `f` with recipe `r`.
    fn extensible(&self, f: FontId, r: u8, v: Scaled) -> Result<BoxNode, Confusion> {
        let e = *self
            .env
            .font(f)
            .extensibles
            .get(usize::from(r))
            .ok_or(Confusion("extensible"))?;
        let mut b = BoxNode {
            vertical: true,
            ..BoxNode::default()
        };
        // §714
        let u = self.height_plus_depth(f, e.rep);
        let g = self.glyph(f, u32::from(e.rep)).unwrap_or_default();
        b.width = g.width + g.italic;
        let mut w = 0;
        for c in [e.bot, e.mid, e.top] {
            if c != 0 {
                w += self.height_plus_depth(f, c);
            }
        }
        let mut n = 0;
        if u > 0 {
            while w < v {
                w += u;
                n += 1;
                if e.mid != 0 {
                    w += u;
                }
            }
        }
        // §715: stacked from the bottom up.
        let mut stack = Vec::new();
        if e.bot != 0 {
            stack.push(e.bot);
        }
        for _ in 0..n {
            stack.push(e.rep);
        }
        if e.mid != 0 {
            stack.push(e.mid);
            for _ in 0..n {
                stack.push(e.rep);
            }
        }
        if e.top != 0 {
            stack.push(e.top);
        }
        for &c in stack.iter().rev() {
            b.list.push(boxed(self.char_box(f, c)));
        }
        if let Some(&c) = stack.last() {
            b.height = self.char_box(f, c).height;
        }
        b.depth = w - b.height;
        Ok(b)
    }

    /// §715: change the width of `b` to `w`, centering its contents.
    fn rebox(&mut self, mut b: BoxNode, w: Scaled) -> BoxNode {
        if b.width != w && !b.list.is_empty() {
            if b.vertical {
                b = self.hpack(vec![boxed(b)]);
            }
            let width = b.width;
            let mut list = b.list;
            if let [Node::Glyphs(g)] = list.as_slice()
                && g.chars().len() == 1
            {
                let v = self
                    .glyph(g.font, u32::from(g.chars()[0]))
                    .unwrap_or_default()
                    .width;
                if v != width {
                    list.push(kern(width - v));
                }
            }
            let ss = Node::Glue {
                spec: GlueSpec {
                    width: 0,
                    stretch: 0o200000,
                    shrink: 0o200000,
                    stretch_order: Order::Fil,
                    shrink_order: Order::Fil,
                    shared_zero: false,
                },
                subtype: 0,
                sync: crate::origin::Side(0),
            };
            list.insert(0, ss.clone());
            list.push(ss);
            self.env.hpack(list, Spec::Exactly(w))
        } else {
            b.width = w;
            b
        }
    }

    /// §716: convert mu glue `g` to pt glue with `m` pt per mu.
    fn math_glue(g: &GlueSpec, m: Scaled) -> GlueSpec {
        let (n, f) = mu_parts(m);
        GlueSpec {
            width: mu_mult(n, f, g.width),
            stretch: if g.stretch_order == Order::Normal {
                mu_mult(n, f, g.stretch)
            } else {
                g.stretch
            },
            shrink: if g.shrink_order == Order::Normal {
                mu_mult(n, f, g.shrink)
            } else {
                g.shrink
            },
            stretch_order: g.stretch_order,
            shrink_order: g.shrink_order,
            shared_zero: false,
        }
    }

    /// §720: the translation of `field` in style `s`, as a box.
    fn clean_box(&mut self, field: Field, s: u8) -> Result<BoxNode, Confusion> {
        let q = match field {
            Field::Char { .. } | Field::TextChar { .. } => {
                let mut n = Noad::new(Kind::Ord);
                n.nucleus = field;
                Some(self.sub_mlist(vec![Item::Noad(Box::new(n))], s)?)
            }
            Field::Box(b) => {
                if b.shift == 0 {
                    return Ok(self.simplify(unbox(b)));
                }
                Some(vec![Node::Box(b)])
            }
            Field::Mlist(l) => Some(self.sub_mlist(l, s)?),
            Field::Empty => None,
        };
        let x = match q {
            None => BoxNode::default(),
            Some(mut q) => {
                if let [Node::Box(b)] = q.as_slice()
                    && b.shift == 0
                {
                    let Some(Node::Box(b)) = q.pop() else {
                        unreachable!()
                    };
                    unbox(b)
                } else {
                    self.hpack(q)
                }
            }
        };
        Ok(self.simplify(x))
    }

    /// §721: simplify a trivial box.
    fn simplify(&self, mut x: BoxNode) -> BoxNode {
        let _ = self;
        if let [Node::Glyphs(g), Node::Kern { .. }] = x.list.as_slice()
            && g.chars().len() == 1
        {
            x.list.pop(); // unneeded italic correction
        }
        x
    }

    /// A recursive call in style `s`, without penalties; the current
    /// style is restored.
    fn sub_mlist(&mut self, l: Vec<Item>, s: u8) -> Result<Vec<Node>, Confusion> {
        let save = self.style;
        let r = self.mlist_to_hlist(l, s, false)?;
        self.style = save;
        self.set_size_and_mu();
        Ok(r)
    }

    /// §722 (`XeTeX` §765): fetch math character `(fam, ch)`, setting
    /// `cur_f`; an event if it is not there.
    fn fetch(&mut self, fam: u8, ch: u32) -> Fetched {
        let f = self.env.fam_fnt(usize::from(fam) + self.size);
        self.cur_f = f;
        let Some(f) = f else {
            self.events.push(Event::UndefinedFamily {
                size: u16::try_from(self.size).unwrap_or(0),
                fam,
                ch,
            });
            return Fetched::None;
        };
        if self.env.font_kind(f).is_native() {
            Fetched::Native(f)
        } else if let Some(g) = self.glyph(f, ch) {
            Fetched::Tfm(f, g)
        } else {
            self.events.push(Event::MissingChar { font: f, ch });
            Fetched::None
        }
    }

    /// `fetch` of a character field, emptying it if the character is not
    /// there.
    fn fetch_field(&mut self, field: &mut Field) -> Fetched {
        let (Field::Char { fam, ch } | Field::TextChar { fam, ch }) = *field else {
            return Fetched::None;
        };
        let r = self.fetch(fam, ch);
        if let Fetched::None = r {
            *field = Field::Empty;
        }
        r
    }

    /// `fetch` of a TFM character field: its font, character and metrics.
    fn fetch_tfm(&mut self, field: &mut Field) -> Option<(FontId, u32, Glyph)> {
        let (Field::Char { ch, .. } | Field::TextChar { ch, .. }) = *field else {
            return None;
        };
        match self.fetch_field(field) {
            Fetched::Tfm(f, g) => Some((f, ch, g)),
            _ => None,
        }
    }

    fn mlist_to_hlist(
        &mut self,
        mlist: Vec<Item>,
        style: u8,
        penalties: bool,
    ) -> Result<Vec<Node>, Confusion> {
        self.style = style;
        let mut todo: VecDeque<Item> = mlist.into();
        let mut done: Vec<Done> = Vec::with_capacity(todo.len());
        let mut r: Option<usize> = None; // the last noad
        let mut r_type = OP;
        let (mut max_h, mut max_d) = (0, 0);
        self.set_size_and_mu();
        while let Some(item) = todo.pop_front() {
            // §727
            let mut q = match item {
                Item::Noad(q) => q,
                Item::Style(s) => {
                    self.style = s;
                    self.set_size_and_mu();
                    done.push(Done::Style(s));
                    continue;
                }
                Item::Choice(c) => {
                    // §731
                    let [d, t, s, ss] = *c;
                    let chosen = match self.style / 2 {
                        0 => d,
                        1 => t,
                        2 => s,
                        _ => ss,
                    };
                    done.push(Done::Style(self.style));
                    for i in chosen.into_iter().rev() {
                        todo.push_front(i);
                    }
                    continue;
                }
                Item::Node(n) => {
                    done.push(Done::Node(
                        self.first_pass_node(n, &mut todo, &mut max_h, &mut max_d)?,
                    ));
                    continue;
                }
            };
            // §728
            let mut delta = 0;
            let mut convert = true;
            if q.kind == Kind::Bin && matches!(r_type, BIN | OP | REL | OPEN | PUNCT | LEFT) {
                q.kind = Kind::Ord;
            }
            match &q.kind {
                Kind::Bin | Kind::Open | Kind::Inner => {}
                Kind::Rel | Kind::Close | Kind::Punct | Kind::Right(_) | Kind::Middle(_) => {
                    // §729
                    if r_type == BIN
                        && let Some(Done::Noad { code, .. }) = r.map(|i| &mut done[i])
                    {
                        *code = ORD;
                    }
                    if let Kind::Right(_) | Kind::Middle(_) = q.kind {
                        convert = false;
                    }
                }
                Kind::Left(_) => {
                    convert = false;
                }
                Kind::Fraction { .. } => {
                    let h = self.make_fraction(&mut q)?;
                    push_noad(
                        &mut done,
                        &mut r,
                        &mut r_type,
                        &q,
                        h,
                        &mut max_h,
                        &mut max_d,
                        self,
                    );
                    continue;
                }
                Kind::Op(_) => {
                    let (d, limits) = self.make_op(&mut q)?;
                    delta = d;
                    if let Some(v) = limits {
                        push_noad(
                            &mut done,
                            &mut r,
                            &mut r_type,
                            &q,
                            vec![boxed(v)],
                            &mut max_h,
                            &mut max_d,
                            self,
                        );
                        continue;
                    }
                }
                Kind::Ord => self.make_ord(&mut q, &mut todo),
                Kind::Radical(d) => {
                    let d = *d;
                    self.make_radical(&mut q, d)?;
                }
                Kind::Over => self.make_over(&mut q)?,
                Kind::Under => self.make_under(&mut q)?,
                Kind::Accent { fam, ch, sub } => {
                    let (fam, ch, sub) = (*fam, *ch, *sub);
                    self.make_math_accent(&mut q, fam, ch, sub)?;
                }
                Kind::Vcenter => self.make_vcenter(&mut q)?,
            }
            if !convert {
                let delim = match q.kind {
                    Kind::Left(d) | Kind::Right(d) | Kind::Middle(d) => d,
                    _ => Delim::default(),
                };
                done.push(Done::Noad {
                    code: q.kind.code(),
                    hlist: Vec::new(),
                    delim,
                });
                r = Some(done.len() - 1);
                r_type = q.kind.code();
                if r_type == RIGHT {
                    // e-TeX: what follows a `\middle` starts afresh.
                    r_type = LEFT;
                    self.style = style;
                    self.set_size_and_mu();
                }
                continue;
            }
            // §754: convert `nucleus(q)` to an hlist and attach the
            // sub/superscripts.
            let text = matches!(q.nucleus, Field::TextChar { .. });
            let mut p = match core::mem::take(&mut q.nucleus) {
                mut n @ (Field::Char { .. } | Field::TextChar { .. }) => {
                    // §755 (`XeTeX` §799)
                    let (Field::Char { ch, .. } | Field::TextChar { ch, .. }) = n else {
                        unreachable!()
                    };
                    match self.fetch_field(&mut n) {
                        Fetched::Native(f) => {
                            let g = self.native_char_glyph(f, ch);
                            let g = self.env.glyph_node(f, g);
                            delta = self.env.ot_math_ital_corr(f, g.gid);
                            if text && !self.env.font_kind(f).is_new_mathfont() {
                                delta = 0; // no italic correction in mid-word of text font
                            }
                            let mut p = vec![glyph_node(g)];
                            if q.sub == Field::Empty && delta != 0 {
                                p.push(kern(delta));
                                delta = 0;
                            }
                            p
                        }
                        Fetched::Tfm(f, g) => {
                            delta = g.italic;
                            let c = u8::try_from(ch).unwrap_or(0);
                            let mut p = vec![Node::Glyphs(crate::node::Glyphs::one(f, c))];
                            if text && self.env.param(f, 2) != 0 {
                                delta = 0; // no italic correction in mid-word of text font
                            }
                            if q.sub == Field::Empty && delta != 0 {
                                p.push(kern(delta));
                                delta = 0;
                            }
                            p
                        }
                        Fetched::None => Vec::new(),
                    }
                }
                Field::Empty => Vec::new(),
                Field::Box(b) => vec![Node::Box(b)],
                Field::Mlist(l) => {
                    let h = self.sub_mlist(l, self.style)?;
                    vec![boxed(self.hpack(h))]
                }
            };
            if q.sub != Field::Empty || q.sup != Field::Empty {
                self.make_scripts(&mut q, &mut p, delta)?;
            }
            push_noad(
                &mut done,
                &mut r,
                &mut r_type,
                &q,
                p,
                &mut max_h,
                &mut max_d,
                self,
            );
        }
        // §729: convert a final `bin_noad` to an `ord_noad`.
        if r_type == BIN
            && let Some(Done::Noad { code, .. }) = r.map(|i| &mut done[i])
        {
            *code = ORD;
        }
        self.second_pass(done, style, penalties, max_h, max_d)
    }

    /// §730: nodes in the first pass.
    fn first_pass_node(
        &mut self,
        mut n: Node,
        todo: &mut VecDeque<Item>,
        max_h: &mut Scaled,
        max_d: &mut Scaled,
    ) -> Result<Node, Confusion> {
        match &mut n {
            Node::Ins(_)
            | Node::Mark(_)
            | Node::Adjust(_)
            | Node::Whatsit(_)
            | Node::Penalty(_)
            | Node::Disc(_)
            | Node::Leaders(_) => {}
            Node::Rule { height, depth, .. } => {
                *max_h = (*max_h).max(*height);
                *max_d = (*max_d).max(*depth);
            }
            Node::Glue { spec, subtype, .. } => {
                // §732: convert math glue to ordinary glue.
                if *subtype == MU_GLUE {
                    *spec = Self::math_glue(spec, self.mu);
                    *subtype = 0;
                } else if self.size != TEXT_SIZE
                    && *subtype == COND_MATH_GLUE
                    && matches!(
                        todo.front(),
                        Some(Item::Node(
                            Node::Glue { .. } | Node::Leaders(_) | Node::Kern { .. }
                        ))
                    )
                {
                    todo.pop_front();
                }
            }
            Node::Kern { width, subtype, .. } => {
                // §717
                if *subtype == MU_GLUE {
                    let (n, f) = mu_parts(self.mu);
                    *width = mu_mult(n, f, *width);
                    *subtype = EXPLICIT;
                }
            }
            _ => return Err(Confusion("mlist1")),
        }
        Ok(n)
    }

    /// §734
    fn make_over(&mut self, q: &mut Noad) -> Result<(), Confusion> {
        let b = self.clean_box(core::mem::take(&mut q.nucleus), cramped(self.style))?;
        let t = self.default_rule_thickness();
        q.nucleus = Field::Box((self.overbar(b, 3 * t, t)?).share());
        Ok(())
    }

    /// §735
    fn make_under(&mut self, q: &mut Noad) -> Result<(), Confusion> {
        let x = self.clean_box(core::mem::take(&mut q.nucleus), self.style)?;
        let t = self.default_rule_thickness();
        let xh = x.height;
        let mut y = self.vpack(vec![boxed(x), kern(3 * t), rule(t)])?;
        let delta = y.height + y.depth + t;
        y.height = xh;
        y.depth = delta - y.height;
        q.nucleus = Field::Box(y.share());
        Ok(())
    }

    /// §736
    fn make_vcenter(&mut self, q: &mut Noad) -> Result<(), Confusion> {
        let Field::Box(v) = &mut q.nucleus else {
            return Err(Confusion("vcenter"));
        };
        if !v.vertical {
            return Err(Confusion("vcenter"));
        }
        let v = Arc::make_mut(v);
        let delta = v.height + v.depth;
        v.height = self.axis_height(self.size) + half(delta);
        v.depth = delta - v.height;
        v.reversion();
        Ok(())
    }

    /// §737 (`XeTeX` §780: an OpenType math font's rule and gaps).
    fn make_radical(&mut self, q: &mut Noad, d: Delim) -> Result<(), Confusion> {
        let f = self.env.fam_fnt(usize::from(d.small_fam) + self.size);
        let ot = self.kind(f).is_new_mathfont();
        let constant = |cx: &Self, k| f.map_or(0, |f| cx.env.ot_math_constant(f, k));
        let t = if ot {
            constant(self, RADICAL_RULE_THICKNESS)
        } else {
            self.default_rule_thickness()
        };
        let x = self.clean_box(core::mem::take(&mut q.nucleus), cramped(self.style))?;
        let mut clr = if ot {
            if self.style < TEXT {
                constant(self, RADICAL_DISPLAY_STYLE_VERTICAL_GAP)
            } else {
                constant(self, RADICAL_VERTICAL_GAP)
            }
        } else if self.style < TEXT {
            t + self.math_x_height(self.size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let mut y = self.var_delimiter(d, self.size, x.height + x.depth + clr + t)?;
        if ot {
            y.depth = y.height + y.depth - t;
            y.height = t;
        }
        let delta = y.depth - (x.height + x.depth + clr);
        if delta > 0 {
            clr += half(delta); // increase the actual clearance
        }
        y.shift = -(x.height + clr);
        let yh = y.height;
        let o = self.overbar(x, clr, yh)?;
        q.nucleus = Field::Box((self.hpack(vec![boxed(y), boxed(o)])).share());
        Ok(())
    }

    /// `XeTeX` §781 `compute_ot_math_accent_pos`: the top accent
    /// attachment of the nucleus's character (or of the nucleus of an
    /// accent that is its subformula's first noad).
    fn compute_ot_math_accent_pos(&mut self, nucleus: &mut Field) -> Scaled {
        match nucleus {
            Field::Char { ch, .. } => {
                let ch = *ch;
                self.fetch_field(nucleus);
                match self.cur_f {
                    Some(f) if self.env.font_kind(f).is_native() => {
                        let g = self.native_char_glyph(f, ch);
                        self.env.ot_math_accent_pos(f, g)
                    }
                    _ => NO_ACCENT_POS,
                }
            }
            Field::Mlist(l) => match l.first_mut() {
                Some(Item::Noad(r)) if matches!(r.kind, Kind::Accent { .. }) => {
                    self.compute_ot_math_accent_pos(&mut r.nucleus)
                }
                _ => NO_ACCENT_POS,
            },
            _ => NO_ACCENT_POS,
        }
    }

    /// §741: the skew of the accentee `nucleus` (a character of a TFM
    /// font), its kern with the font's `\skewchar`.
    fn skew(&mut self, nucleus: &mut Field) -> Scaled {
        let mut s = 0;
        if let Field::Char { .. } = nucleus
            && let Some((cf, _, g)) = self.fetch_tfm(nucleus)
            && let Tag::Lig(start) = g.tag
        {
            let font = self.env.font(cf);
            let skew = self.env.skew_char(cf);
            let mut k = usize::from(start);
            let first = font.lig_kerns[k];
            if first.skip > LigKern::STOP {
                k = 256 * usize::from(first.op) + usize::from(first.remainder);
            }
            loop {
                let ci = font.lig_kerns[k];
                if i32::from(ci.next) == skew {
                    if ci.op >= LigKern::KERN && ci.skip <= LigKern::STOP {
                        s = char_kern(font, ci);
                    }
                    break;
                }
                if ci.skip >= LigKern::STOP {
                    break;
                }
                k += usize::from(ci.skip) + 1;
            }
        }
        s
    }

    /// The nucleus to box, a copy if it is a character (which §742 may
    /// swap into the box later).
    fn accentee(q: &mut Noad) -> Field {
        if matches!(q.nucleus, Field::Char { .. }) {
            q.nucleus.clone()
        } else {
            core::mem::take(&mut q.nucleus)
        }
    }

    /// §738 (`XeTeX` §781: accents of native fonts, positioned by their
    /// attachment points, growing by their horizontal variants and
    /// assemblies; bottom and fixed accents).
    fn make_math_accent(
        &mut self,
        q: &mut Noad,
        fam: u8,
        ch: u32,
        sub: u8,
    ) -> Result<(), Confusion> {
        let bottom = sub & BOTTOM_ACC != 0;
        let (f, mut s, mut x, w, mut h, tfm) = match self.fetch(fam, ch) {
            Fetched::None => return Ok(()),
            Fetched::Native(f) => {
                let s = if bottom {
                    0
                } else {
                    self.compute_ot_math_accent_pos(&mut q.nucleus)
                };
                let x = self.clean_box(Self::accentee(q), cramped(self.style))?;
                let (w, h) = (x.width, x.height);
                (f, s, x, w, h, None)
            }
            Fetched::Tfm(f, mut i) => {
                let mut c = u8::try_from(ch).unwrap_or(0);
                let s = self.skew(&mut q.nucleus);
                let x = self.clean_box(Self::accentee(q), cramped(self.style))?;
                let (w, h) = (x.width, x.height);
                // §740: switch to a larger accent if available and appropriate.
                while let Tag::List(y) = i.tag {
                    match self.glyph(f, u32::from(y)) {
                        Some(g) if g.width <= w => {
                            i = g;
                            c = y;
                        }
                        _ => break,
                    }
                }
                (f, s, x, w, h, Some(c))
            }
        };
        let mut delta = if self.env.font_kind(f).is_new_mathfont() {
            if bottom {
                0
            } else {
                h.min(self.env.ot_math_constant(f, ACCENT_BASE_HEIGHT))
            }
        } else {
            h.min(self.env.param(f, 5))
        };
        if (q.sup != Field::Empty || q.sub != Field::Empty)
            && matches!(q.nucleus, Field::Char { .. })
        {
            // §742: swap the subscript and superscript into box `x`.
            let mut n = Noad::new(Kind::Ord);
            n.nucleus = core::mem::take(&mut q.nucleus);
            n.sup = core::mem::take(&mut q.sup);
            n.sub = core::mem::take(&mut q.sub);
            x = self.clean_box(Field::Mlist(vec![Item::Noad(Box::new(n))]), self.style)?;
            delta = delta + x.height - h;
            h = x.height;
        }
        let mut y = if let Some(c) = tfm {
            let mut y = self.char_box(f, c);
            y.shift = s + half(w - y.width);
            y
        } else {
            // the accent as a glyph, larger as the accentee is wider
            let c = self.native_char_glyph(f, ch);
            let mut p = self.env.glyph_node(f, c);
            let mut assembly = None;
            if sub & FIXED_ACC == 0 {
                let mut a = 0;
                loop {
                    let (g, w2) = self.env.ot_math_variant(f, c, a, true);
                    if w2 > 0 && w2 <= w {
                        p = self.env.glyph_node(f, g);
                        a += 1;
                    }
                    // (`XeTeX` loops for ever on a variant of no width)
                    if w2 < 0 || w2 >= w || w2 == 0 {
                        if w2 < 0 {
                            assembly = self
                                .env
                                .ot_assembly(f, c, true)
                                .map(|a| self.opentype_assembly(f, &a, w, true));
                        }
                        break;
                    }
                }
            }
            let mut y = match assembly {
                Some(b) => BoxNode {
                    width: b.width,
                    height: b.height,
                    depth: b.depth,
                    list: vec![boxed(b)],
                    ..BoxNode::default()
                },
                None => BoxNode {
                    width: p.width,
                    height: p.height,
                    depth: p.depth,
                    list: vec![glyph_node(p)],
                    ..BoxNode::default()
                },
            };
            if bottom {
                y.height = y.height.max(0);
            } else {
                y.depth = y.depth.max(0);
            }
            // its horizontal position
            let sa = match glyph_of(y.list.first()) {
                Some(g) => match self.env.ot_math_accent_pos(f, g.gid) {
                    NO_ACCENT_POS => half(y.width),
                    sa => sa,
                },
                None => half(y.width),
            };
            if bottom || s == NO_ACCENT_POS {
                s = half(w);
            }
            y.shift = s - sa;
            y
        };
        y.width = 0;
        let xw = x.width;
        let mut y = if bottom {
            let xh = h;
            let mut y = self.vpack(vec![boxed(x), boxed(y)])?;
            y.shift = -(xh - y.height);
            y
        } else {
            let mut y = self.vpack(vec![boxed(y), kern(-delta), boxed(x)])?;
            if y.height < h {
                // §739
                y.list.insert(0, kern(h - y.height));
                y.height = h;
            }
            y
        };
        y.width = xw;
        q.nucleus = Field::Box(y.share());
        Ok(())
    }

    /// §743: the fraction's hlist.
    fn make_fraction(&mut self, q: &mut Noad) -> Result<Vec<Node>, Confusion> {
        let Kind::Fraction {
            thickness,
            left,
            right,
        } = &mut q.kind
        else {
            unreachable!()
        };
        if *thickness == DEFAULT_CODE {
            *thickness = self.default_rule_thickness();
        }
        let (thickness, left, right) = (*thickness, *left, *right);
        let size = self.size;
        let style = self.style;
        // §744
        let mut x = self.clean_box(core::mem::take(&mut q.sup), num_style(style))?;
        let mut z = self.clean_box(core::mem::take(&mut q.sub), denom_style(style))?;
        if x.width < z.width {
            x = self.rebox(x, z.width);
        } else {
            z = self.rebox(z, x.width);
        }
        let (mut shift_up, mut shift_down);
        if style < TEXT {
            shift_up = self.mathsy(8, size);
            shift_down = self.mathsy(11, size);
        } else {
            shift_down = self.mathsy(12, size);
            shift_up = if thickness == 0 {
                self.mathsy(10, size)
            } else {
                self.mathsy(9, size)
            };
        }
        let mut delta = 0;
        // (`XeTeX` §789, §790: with the gaps of `cur_f`, the font of the
        // last character fetched, if an OpenType math font)
        let ot = self.cur_mathfont();
        if thickness == 0 {
            // §745
            let clr = if ot {
                self.cur_constant(if style < TEXT {
                    STACK_DISPLAY_STYLE_GAP_MIN
                } else {
                    STACK_GAP_MIN
                })
            } else {
                let t = self.default_rule_thickness();
                if style < TEXT { 7 * t } else { 3 * t }
            };
            let d = half(clr - ((shift_up - x.depth) - (z.height - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
        } else {
            // §746
            delta = half(thickness);
            let axis = self.axis_height(size);
            let (clr1, clr2) = if ot {
                if style < TEXT {
                    (
                        self.cur_constant(FRACTION_NUM_DISPLAY_STYLE_GAP_MIN),
                        self.cur_constant(FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN),
                    )
                } else {
                    (
                        self.cur_constant(FRACTION_NUMERATOR_GAP_MIN),
                        self.cur_constant(FRACTION_DENOMINATOR_GAP_MIN),
                    )
                }
            } else if style < TEXT {
                (3 * thickness, 3 * thickness)
            } else {
                (thickness, thickness)
            };
            let delta1 = clr1 - ((shift_up - x.depth) - (axis + delta));
            let delta2 = clr2 - ((axis - delta) - (z.height - shift_down));
            if delta1 > 0 {
                shift_up += delta1;
            }
            if delta2 > 0 {
                shift_down += delta2;
            }
        }
        // §747
        let mut v = BoxNode {
            vertical: true,
            height: shift_up + x.height,
            depth: z.depth + shift_down,
            width: x.width,
            ..BoxNode::default()
        };
        let (xd, zh) = (x.depth, z.height);
        v.list.push(boxed(x));
        if thickness == 0 {
            v.list.push(kern((shift_up - xd) - (zh - shift_down)));
        } else {
            let axis = self.axis_height(size);
            v.list.push(kern((shift_up - xd) - (axis + delta)));
            v.list.push(rule(thickness));
            v.list.push(kern((axis - delta) - (zh - shift_down)));
        }
        v.list.push(boxed(z));
        // §748
        let delta = if style < TEXT {
            self.mathsy(20, size)
        } else {
            self.mathsy(21, size)
        };
        let l = self.var_delimiter(left, size, delta)?;
        let r = self.var_delimiter(right, size, delta)?;
        Ok(vec![boxed(self.hpack(vec![boxed(l), boxed(v), boxed(r)]))])
    }

    /// §749: the italic correction, and the box with limits if there
    /// are limits.
    fn make_op(&mut self, q: &mut Noad) -> Result<(Scaled, Option<BoxNode>), Confusion> {
        let Kind::Op(limits) = &mut q.kind else {
            unreachable!()
        };
        if *limits == Limits::Normal && self.style < TEXT {
            *limits = Limits::Limits;
        }
        let limits = *limits == Limits::Limits;
        let mut delta = 0;
        if let Field::Char { .. } = q.nucleus {
            if let Fetched::Tfm(f, mut g) = self.fetch_field(&mut q.nucleus) {
                if self.style < TEXT
                    && let Tag::List(c) = g.tag
                    && let Some(i) = self.glyph(f, u32::from(c))
                {
                    g = i;
                    if let Field::Char { ch, .. } = &mut q.nucleus {
                        *ch = u32::from(c);
                    }
                }
                delta = g.italic;
            }
            let mut x = self.clean_box(core::mem::take(&mut q.nucleus), self.style)?;
            if self.cur_mathfont()
                && let Some(f) = self.cur_f
                && let Some(mut p) = glyph_of(x.list.first())
            {
                // `XeTeX` §793: in display style, a variant at least
                // `displayOperatorMinHeight` high and larger than the
                // text-style glyph, else the glyph's assembly
                let mut assembly = None;
                if self.style < TEXT {
                    let mut h1 = self.env.ot_math_constant(f, DISPLAY_OPERATOR_MIN_HEIGHT);
                    let h = (p.height + p.depth).wrapping_mul(5) / 4;
                    if h1 < h {
                        h1 = h;
                    }
                    let c = p.gid;
                    let mut n = 0;
                    loop {
                        let (g, h2) = self.env.ot_math_variant(f, c, n, false);
                        if h2 > 0 {
                            p = self.env.glyph_node(f, g);
                        }
                        n += 1;
                        if h2 < 0 || h2 >= h1 {
                            if h2 < 0 {
                                assembly = self
                                    .env
                                    .ot_assembly(f, c, false)
                                    .map(|a| self.opentype_assembly(f, &a, h1, false));
                            }
                            break;
                        }
                    }
                }
                let (w, h, d) = if let Some(b) = assembly {
                    delta = 0;
                    let dims = (b.width, b.height, b.depth);
                    x.list = vec![boxed(b)];
                    dims
                } else {
                    x.list[0] = glyph_node(p);
                    delta = self.env.ot_math_ital_corr(f, p.gid);
                    (p.width, p.height, p.depth)
                };
                x.width = w;
                x.height = h;
                x.depth = d;
            }
            if q.sub != Field::Empty && !limits {
                x.width -= delta; // remove italic correction
            }
            x.shift = half(x.height - x.depth) - self.axis_height(self.size);
            q.nucleus = Field::Box(x.share());
        }
        if !limits {
            return Ok((delta, None));
        }
        // §750
        let save_f = self.cur_f;
        let style = self.style;
        let sup_empty = q.sup == Field::Empty;
        let sub_empty = q.sub == Field::Empty;
        let x = self.clean_box(core::mem::take(&mut q.sup), sup_style(style))?;
        let y = self.clean_box(core::mem::take(&mut q.nucleus), style)?;
        let z = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
        self.cur_f = save_f;
        let mut v = BoxNode {
            vertical: true,
            width: y.width.max(x.width).max(z.width),
            ..BoxNode::default()
        };
        let mut x = self.rebox(x, v.width);
        let y = self.rebox(y, v.width);
        let mut z = self.rebox(z, v.width);
        x.shift = half(delta);
        z.shift = -x.shift;
        v.height = y.height;
        v.depth = y.depth;
        // §751
        if sup_empty {
            v.list.push(boxed(y));
        } else {
            let shift_up = (self.mathex(11) - x.depth).max(self.mathex(9));
            v.height += self.mathex(13) + x.height + x.depth + shift_up;
            v.list.push(kern(self.mathex(13)));
            v.list.push(boxed(x));
            v.list.push(kern(shift_up));
            v.list.push(boxed(y));
        }
        if !sub_empty {
            let shift_down = (self.mathex(12) - z.height).max(self.mathex(10));
            v.depth += self.mathex(13) + z.height + z.depth + shift_down;
            v.list.push(kern(shift_down));
            v.list.push(boxed(z));
            v.list.push(kern(self.mathex(13)));
        }
        Ok((delta, Some(v)))
    }

    /// §752: ligatures and kerns between ordinary characters.
    fn make_ord(&mut self, q: &mut Noad, todo: &mut VecDeque<Item>) {
        'restart: loop {
            if q.sub != Field::Empty || q.sup != Field::Empty {
                return;
            }
            let Field::Char { fam, ch: _ } = q.nucleus else {
                return;
            };
            let Some(Item::Noad(p)) = todo.front() else {
                return;
            };
            let Field::Char { fam: pfam, ch: pch } = p.nucleus else {
                return;
            };
            if !(ORD..=PUNCT).contains(&p.kind.code()) || pfam != fam {
                return;
            }
            let Field::Char { ch, .. } = q.nucleus else {
                unreachable!()
            };
            q.nucleus = Field::TextChar { fam, ch };
            let Some((cf, _, g)) = self.fetch_tfm(&mut q.nucleus) else {
                return;
            };
            let Tag::Lig(start) = g.tag else {
                return;
            };
            let font = self.env.font(cf);
            let mut k = usize::from(start);
            let first = font.lig_kerns[k];
            if first.skip > LigKern::STOP {
                k = 256 * usize::from(first.op) + usize::from(first.remainder);
            }
            loop {
                // §753
                let ci = font.lig_kerns[k];
                // (`XeTeX` compares the character's low 16 bits)
                if u32::from(ci.next) == pch & 0xFFFF && ci.skip <= LigKern::STOP {
                    if ci.op >= LigKern::KERN {
                        todo.push_front(Item::Node(kern(char_kern(font, ci))));
                        return;
                    }
                    let Some(Item::Noad(p)) = todo.front_mut() else {
                        unreachable!()
                    };
                    match ci.op {
                        1 | 5 => {
                            q.nucleus = Field::TextChar {
                                fam,
                                ch: u32::from(ci.remainder),
                            }
                        } // =:|, =:|>
                        2 | 6 => {
                            p.nucleus = Field::Char {
                                fam,
                                ch: (pch & !0xFFFF) | u32::from(ci.remainder),
                            }
                        } // |=:, |=:>
                        3 | 7 | 11 => {
                            // |=:|, |=:|>, |=:|>>
                            let mut r = Noad::new(Kind::Ord);
                            r.nucleus = if ci.op < 11 {
                                Field::Char {
                                    fam,
                                    ch: u32::from(ci.remainder),
                                }
                            } else {
                                Field::TextChar {
                                    fam,
                                    ch: u32::from(ci.remainder),
                                } // prevent combination
                            };
                            todo.push_front(Item::Noad(Box::new(r)));
                        }
                        _ => {
                            let Some(Item::Noad(p)) = todo.pop_front() else {
                                unreachable!()
                            };
                            q.nucleus = Field::TextChar {
                                fam,
                                ch: u32::from(ci.remainder),
                            }; // =:
                            q.sub = p.sub;
                            q.sup = p.sup;
                        }
                    }
                    if ci.op > 3 {
                        return;
                    }
                    if let Field::TextChar { fam, ch } = q.nucleus {
                        q.nucleus = Field::Char { fam, ch };
                    }
                    continue 'restart;
                }
                if ci.skip >= LigKern::STOP {
                    return;
                }
                k += usize::from(ci.skip) + 1;
            }
        }
    }

    /// `XeTeX` §805 "Fetch first character of a sub/superscript": the
    /// font and glyph of script `field`'s first character if it is in an
    /// OpenType math font (`None` otherwise). Kerns and glue before it
    /// are skipped, style changes obeyed, a `\mathchoice` followed; it is
    /// fetched in subscript style, a missing one complained about (and
    /// its field emptied) as `fetch` does.
    fn script_glyph(&mut self, field: &mut Field) -> (Option<FontId>, u16) {
        let mut this_style = sub_style(self.style);
        let head = match field {
            Field::Mlist(l) => first_nucleus(l, &mut this_style),
            f => Some(f),
        };
        let Some(head) = head else {
            return (None, 0);
        };
        let Field::Char { ch, .. } = *head else {
            return (None, 0);
        };
        let save_f = self.cur_f;
        let saved_style = self.style;
        self.style = this_style;
        self.set_size_and_mu();
        self.fetch_field(head);
        let r = match self.cur_f {
            Some(f) if self.cur_mathfont() => (Some(f), self.native_char_glyph(f, ch)),
            _ => (None, 0),
        };
        self.cur_f = save_f;
        self.style = saved_style;
        self.set_size_and_mu();
        r
    }

    /// `XeTeX` §806: the OpenType math kern between nucleus `p` (if it
    /// starts with a glyph) and a script of glyph `sg` in `sf`, shifted
    /// by `shift`; appended to `p` unless zero.
    fn attach_ot_kern(
        &mut self,
        p: &mut Vec<Node>,
        script: (Option<FontId>, u16),
        sup: bool,
        shift: Scaled,
    ) -> Scaled {
        let Some(g) = glyph_of(p.first()) else {
            return 0;
        };
        let k = self
            .env
            .ot_math_kern(g.font, g.gid, script.0, script.1, sup, shift);
        if k != 0 {
            p.push(kern(k)); // `attach_hkern_to_new_hlist`
        }
        k
    }

    /// §756 (`XeTeX` §800: an OpenType math font's script parameters
    /// and kerns, of `cur_f`): attach the scripts of `q` to its hlist
    /// `p`.
    fn make_scripts(
        &mut self,
        q: &mut Noad,
        p: &mut Vec<Node>,
        delta: Scaled,
    ) -> Result<(), Confusion> {
        let style = self.style;
        let size = self.size;
        let (mut shift_up, mut shift_down);
        if matches!(p.first(), Some(Node::Glyphs(_))) || glyph_of(p.first()).is_some() {
            shift_up = 0;
            shift_down = 0;
        } else {
            let z = self.hpack(p.clone());
            let t = if style < SCRIPT {
                SCRIPT_SIZE
            } else {
                SCRIPT_SCRIPT_SIZE
            };
            shift_up = z.height - self.mathsy(18, t);
            shift_down = z.depth + self.mathsy(19, t);
        }
        let xetex = self.env.xetex();
        let script = |cx: &mut Self, field: &mut Field| {
            if xetex {
                cx.script_glyph(field)
            } else {
                (None, 0)
            }
        };
        let script_space = self.params.script_space;
        let x = if q.sup == Field::Empty {
            // §757
            let sub = script(self, &mut q.sub);
            let save_f = self.cur_f;
            let mut x = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
            self.cur_f = save_f;
            x.width += script_space;
            shift_down = shift_down.max(self.mathsy(16, size));
            let clr = if self.cur_mathfont() {
                x.height - self.cur_constant(SUBSCRIPT_TOP_MAX)
            } else {
                x.height - (self.math_x_height(size) * 4).abs() / 5
            };
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            if self.cur_mathfont() {
                self.attach_ot_kern(p, sub, false, shift_down);
            }
            x
        } else {
            // §758
            let sup = script(self, &mut q.sup);
            let save_f = self.cur_f;
            let mut x = self.clean_box(core::mem::take(&mut q.sup), sup_style(style))?;
            self.cur_f = save_f;
            x.width += script_space;
            let clr = if style % 2 == 1 {
                self.mathsy(15, size)
            } else if style < TEXT {
                self.mathsy(13, size)
            } else {
                self.mathsy(14, size)
            };
            shift_up = shift_up.max(clr);
            let clr = if self.cur_mathfont() {
                x.depth + self.cur_constant(SUPERSCRIPT_BOTTOM_MIN)
            } else {
                x.depth + self.math_x_height(size).abs() / 4
            };
            shift_up = shift_up.max(clr);
            if self.cur_mathfont() && q.sub == Field::Empty {
                // (with a subscript the kern goes into the shift)
                self.attach_ot_kern(p, sup, true, shift_up);
            }
            if q.sub == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // §759
                let save_f = self.cur_f;
                let sub = script(self, &mut q.sub);
                let sub_empty = q.sub == Field::Empty;
                let mut y = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
                self.cur_f = save_f;
                y.width += script_space;
                shift_down = shift_down.max(self.mathsy(17, size));
                let ot = self.cur_mathfont();
                let clr = if ot {
                    self.cur_constant(SUB_SUPERSCRIPT_GAP_MIN)
                } else {
                    4 * self.default_rule_thickness()
                } - ((shift_up - x.depth) - (y.height - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    let clr = if ot {
                        self.cur_constant(SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT)
                    } else {
                        (self.math_x_height(size) * 4).abs() / 5
                    } - (shift_up - x.depth);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                let (mut sub_kern, mut sup_kern) = (0, 0);
                if ot {
                    sub_kern = self.attach_ot_kern(p, sub, false, shift_down);
                    if sub_empty {
                        sup_kern = self.attach_ot_kern(p, sup, true, shift_up);
                    }
                }
                // superscript is `delta` to the right of the subscript
                x.shift = sup_kern + delta - sub_kern;
                let k = kern((shift_up - x.depth) - (y.height - shift_down));
                let mut x = self.vpack(vec![boxed(x), k, boxed(y)])?;
                x.shift = shift_down;
                x
            }
        };
        p.push(boxed(x));
        Ok(())
    }

    /// §760: the second pass: spacing and penalties.
    fn second_pass(
        &mut self,
        done: Vec<Done>,
        style: u8,
        penalties: bool,
        max_h: Scaled,
        max_d: Scaled,
    ) -> Result<Vec<Node>, Confusion> {
        let mut out = Vec::new();
        let mut r_type = 0;
        self.style = style;
        self.set_size_and_mu();
        let codes: Vec<i32> = done
            .iter()
            .map(|d| match d {
                Done::Noad { code, .. } => *code,
                Done::Style(_) => 14,
                Done::Node(Node::Penalty(_)) => PENALTY_NODE,
                Done::Node(_) => 0,
            })
            .collect();
        for (i, d) in done.into_iter().enumerate() {
            // §761
            let (code, mut hlist, delim) = match d {
                Done::Style(s) => {
                    // §763
                    self.style = s;
                    self.set_size_and_mu();
                    continue;
                }
                Done::Node(n) => {
                    out.push(n);
                    continue;
                }
                Done::Noad { code, hlist, delim } => (code, hlist, delim),
            };
            let mut pen = INF_PENALTY;
            let t = match code {
                OP | OPEN | 21 | PUNCT | INNER => code,
                BIN => {
                    pen = self.params.bin_op_penalty;
                    BIN
                }
                REL => {
                    pen = self.params.rel_penalty;
                    REL
                }
                30 | 31 => {
                    hlist = vec![boxed(self.make_left_right(delim, style, max_d, max_h)?)];
                    code - (LEFT - OPEN)
                }
                _ => ORD,
            };
            // §766
            if r_type > 0 {
                let k = usize::try_from((r_type - ORD) * 8 + (t - ORD)).unwrap_or(0);
                let script = self.style < SCRIPT;
                let x = match MATH_SPACING[k] {
                    b'0' => None,
                    b'1' => script.then_some(0),
                    b'2' => Some(0),
                    b'3' => script.then_some(1),
                    b'4' => script.then_some(2),
                    _ => return Err(Confusion("mlist4")),
                };
                if let Some(x) = x {
                    let g = [
                        &self.params.thin_mu_skip,
                        &self.params.med_mu_skip,
                        &self.params.thick_mu_skip,
                    ][x];
                    out.push(Node::Glue {
                        spec: Self::math_glue(g, self.mu),
                        subtype: THIN_MU_SKIP + u8::try_from(x).unwrap_or(0) + 1,
                        sync: crate::origin::Side(0),
                    });
                }
            }
            // §767
            append(&mut out, hlist);
            let t = if code == RIGHT { OPEN } else { t }; // (e-TeX: after `\middle`)
            if penalties
                && let Some(&next) = codes.get(i + 1)
                && pen < INF_PENALTY
                && next != PENALTY_NODE
                && next != REL
            {
                out.push(Node::Penalty(pen));
            }
            r_type = t;
        }
        Ok(out)
    }

    /// §762
    fn make_left_right(
        &mut self,
        d: Delim,
        style: u8,
        max_d: Scaled,
        max_h: Scaled,
    ) -> Result<BoxNode, Confusion> {
        self.style = style;
        self.set_size_and_mu();
        let size = self.size;
        let mut delta2 = max_d + self.axis_height(size);
        let mut delta1 = max_h + max_d - delta2;
        if delta2 > delta1 {
            delta1 = delta2; // `delta1` is max distance from axis
        }
        let mut delta = (delta1 / 500) * self.params.delimiter_factor;
        delta2 = delta1 + delta1 - self.params.delimiter_shortfall;
        if delta < delta2 {
            delta = delta2;
        }
        self.var_delimiter(d, size, delta)
    }
}

/// Record noad `q` with hlist `hlist` (`done_with_noad` after
/// `check_dimensions`).
#[allow(clippy::too_many_arguments)]
fn push_noad<E: Env>(
    done: &mut Vec<Done>,
    r: &mut Option<usize>,
    r_type: &mut i32,
    q: &Noad,
    hlist: Vec<Node>,
    max_h: &mut Scaled,
    max_d: &mut Scaled,
    cx: &Ctx<'_, E>,
) {
    let (h, d) = pack::natural_height_depth(&hlist, cx.env);
    *max_h = (*max_h).max(h);
    *max_d = (*max_d).max(d);
    let code = q.kind.code();
    done.push(Done::Noad {
        code,
        hlist,
        delim: Delim::default(),
    });
    *r = Some(done.len() - 1);
    *r_type = code;
}

/// `XeTeX` §805: the nucleus of the first noad of a script's mlist that
/// is ordinary to `\mathpunct`, past kerns, glue and style changes (which
/// set `style`), into the branch of a `\mathchoice` that `style` picks.
fn first_nucleus<'a>(l: &'a mut [Item], style: &mut u8) -> Option<&'a mut Field> {
    for item in l {
        match item {
            Item::Node(Node::Kern { .. } | Node::Glue { .. } | Node::Leaders(_)) => {}
            Item::Style(s) => *style = *s,
            Item::Choice(c) => {
                let k = usize::from(*style / 2).min(3);
                return first_nucleus(&mut c[k], style);
            }
            Item::Noad(n)
                if matches!(
                    n.kind,
                    Kind::Ord
                        | Kind::Op(_)
                        | Kind::Bin
                        | Kind::Rel
                        | Kind::Open
                        | Kind::Close
                        | Kind::Punct
                ) =>
            {
                return Some(&mut n.nucleus);
            }
            _ => return None,
        }
    }
    None
}

/// §557: `char_kern`.
fn char_kern(font: &crate::font::Font, ci: LigKern) -> Scaled {
    let i = 256 * usize::from(ci.op - LigKern::KERN) + usize::from(ci.remainder);
    font.kerns.get(i).copied().unwrap_or(0)
}

/// §716, §717: split `m` into its integer and fraction parts.
fn mu_parts(m: Scaled) -> (i32, Scaled) {
    let mut n = m / 0o200000;
    let mut f = m % 0o200000;
    if f < 0 {
        n -= 1;
        f += 0o200000;
    }
    (n, f)
}

/// §716: `mu_mult(x)`, i.e. `nx_plus_y(n, x, xn_over_d(x, f, 2^16))`.
fn mu_mult(n: i32, f: Scaled, x: Scaled) -> Scaled {
    let y = xn_over_d(x, f, 0o200000).unwrap_or(0);
    nx_plus_y(n, x, y)
}

/// §105: `n*x+y`, 0 on overflow (where TeX sets `arith_error`).
fn nx_plus_y(mut n: i32, mut x: Scaled, y: Scaled) -> Scaled {
    const MAX: i32 = 0o7777777777;
    if n < 0 {
        x = -x;
        n = -n;
    }
    if n == 0 {
        y
    } else if x <= (MAX - y) / n && -x <= (MAX + y) / n {
        n * x + y
    } else {
        0
    }
}
