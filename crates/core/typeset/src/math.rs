//! Math layout: `mlist_to_hlist` and its construction routines
//! (§680–§767), with e-TeX's `\middle`.
//!
//! An mlist is a vector of [`MItem`]s whose noads own their fields; font
//! families, parameters and character metrics are supplied by the caller
//! ([`MathParams`], [`Fonts`]).

use std::collections::VecDeque;

use crate::arith::{Scaled, half, nx_plus_y, x_over_n, xn_over_d};
use crate::fonts::{CharInfo, CharTag, FontId, Fonts, Left, LigKern, LigOp};
use crate::glue::{Glue, Order};
use crate::linebreak::INF_PENALTY;
use crate::fonts::{GlyphPart, NativeMath};
use crate::glue::GlueSign;
use crate::node::{
    BoxKind, BoxNode, GlueKind, GlueNode, Glyph, KernKind, List, Native, NativeKind, Node, RUNNING, Rule,
};
use crate::pack::{NATURAL, PackParams, Spec, hpack, vpack};

/// A math style (§688): 0 display, 2 text, 4 script, 6 scriptscript;
/// plus 1 when cramped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Style(pub u8);

impl Style {
    /// `display_style`.
    pub const DISPLAY: Style = Style(0);
    /// `text_style`.
    pub const TEXT: Style = Style(2);
    /// `script_style`.
    pub const SCRIPT: Style = Style(4);
    /// `script_script_style`.
    pub const SCRIPT_SCRIPT: Style = Style(6);

    /// `cramped_style` (§702).
    #[must_use]
    pub fn cramped(self) -> Style {
        Style(2 * (self.0 / 2) + 1)
    }
    /// `sub_style` (§702).
    #[must_use]
    pub fn sub(self) -> Style {
        Style(2 * (self.0 / 4) + 4 + 1)
    }
    /// `sup_style` (§702).
    #[must_use]
    pub fn sup(self) -> Style {
        Style(2 * (self.0 / 4) + 4 + (self.0 % 2))
    }
    /// `num_style` (§702).
    #[must_use]
    pub fn num(self) -> Style {
        Style(self.0 + 2 - 2 * (self.0 / 6))
    }
    /// `denom_style` (§702).
    #[must_use]
    pub fn denom(self) -> Style {
        Style(2 * (self.0 / 2) + 1 + 2 - 2 * (self.0 / 6))
    }
    /// The size code (§703): 0 text, 1 script, 2 scriptscript.
    #[must_use]
    pub fn size(self) -> usize {
        if self.0 < 4 {
            0
        } else {
            usize::from((self.0 - 2) / 2)
        }
    }
}

/// A noad field (§681).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Field {
    /// `empty`.
    #[default]
    Empty,
    /// `math_char`.
    MathChar {
        /// Family.
        fam: u8,
        /// Character (XeTeX's of 21 bits, its plane kept with the
        /// family, `plane_and_fam_field`).
        ch: u32,
    },
    /// `math_text_char`: a character in the middle of a word (§752).
    MathTextChar {
        /// Family.
        fam: u8,
        /// Character.
        ch: u32,
    },
    /// `sub_box`: a box, shifted down by its shift amount.
    SubBox(BoxNode),
    /// `sub_mlist`: a formula (possibly empty, which differs from
    /// `Empty`).
    SubMlist(MList),
}

/// A delimiter field (§683): small and large variants; family 0 and
/// character 0 together mean none (XeTeX: characters of 21 bits, a
/// family of 8, and `\Udelimiter`'s one size with no large variant).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delimiter {
    /// `small_fam`.
    pub small_fam: u8,
    /// `small_char`.
    pub small_char: u32,
    /// `large_fam`.
    pub large_fam: u8,
    /// `large_char`.
    pub large_char: u32,
}

/// The limits convention of an Op noad (§682).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Limits {
    /// `normal`: limits in display style only.
    #[default]
    Normal,
    /// `\limits`.
    Limits,
    /// `\nolimits`.
    NoLimits,
}

/// The kind of a noad with nucleus and scripts (§682, §683, §687).
#[derive(Clone, Debug, PartialEq)]
pub enum NoadKind {
    /// Ord.
    Ord,
    /// Op.
    Op(Limits),
    /// Bin.
    Bin,
    /// Rel.
    Rel,
    /// Open.
    Open,
    /// Close.
    Close,
    /// Punct.
    Punct,
    /// Inner.
    Inner,
    /// `\radical`.
    Radical(Delimiter),
    /// `\overline`.
    Over,
    /// `\underline`.
    Under,
    /// `\mathaccent`: the accent's family and character, and XeTeX's
    /// subtype ([`FIXED_ACC`], [`BOTTOM_ACC`]: `\Umathaccent fixed`,
    /// `bottom`).
    Accent(u8, u32, u8),
    /// `\vcenter`: the nucleus is a vbox.
    Vcenter,
}

/// A noad (§681).
#[derive(Clone, Debug, PartialEq)]
pub struct Noad {
    /// Its kind.
    pub kind: NoadKind,
    /// `nucleus`.
    pub nucleus: Field,
    /// `supscr`.
    pub supscr: Field,
    /// `subscr`.
    pub subscr: Field,
}

impl Noad {
    /// A noad with only a nucleus.
    #[must_use]
    pub fn new(kind: NoadKind, nucleus: Field) -> Noad {
        Noad {
            kind,
            nucleus,
            supscr: Field::Empty,
            subscr: Field::Empty,
        }
    }
}

/// A generalized fraction (§683).
#[derive(Clone, Debug, PartialEq)]
pub struct Fraction {
    /// The rule thickness; `None` is `default_code`.
    pub thickness: Option<Scaled>,
    /// The numerator mlist.
    pub numerator: MList,
    /// The denominator mlist.
    pub denominator: MList,
    /// `left_delimiter`.
    pub left: Delimiter,
    /// `right_delimiter`.
    pub right: Delimiter,
}

/// An mlist item.
#[derive(Clone, Debug, PartialEq)]
pub enum MItem {
    /// A noad.
    Noad(Noad),
    /// A fraction noad.
    Fraction(Fraction),
    /// `\left`.
    Left(Delimiter),
    /// `\right`, or e-TeX's `\middle` when the flag is set.
    Right(Delimiter, bool),
    /// A style node.
    Style(Style),
    /// `\mathchoice`: display, text, script, scriptscript.
    Choice(Box<[MList; 4]>),
    /// A node: insertion, mark, adjust, whatsit, penalty, discretionary,
    /// rule, glue (math glue is [`GlueKind::Mu`], `\nonscript` is
    /// [`GlueKind::CondMath`]) or kern ([`KernKind::Mu`] for `\mkern`).
    Node(Node),
}

/// An mlist.
pub type MList = Vec<MItem>;

/// What `mlist_to_hlist` reads from eqtb.
pub struct MathParams<'a> {
    /// `fam_fnt`: the font of each family at text, script and
    /// scriptscript size; `None` is `null_font`.
    pub fam_fnt: [[Option<FontId>; 16]; 3],
    /// `\thinmuskip`.
    pub thin_mu_skip: Glue,
    /// `\medmuskip`.
    pub med_mu_skip: Glue,
    /// `\thickmuskip`.
    pub thick_mu_skip: Glue,
    /// `\scriptspace`.
    pub script_space: Scaled,
    /// `\nulldelimiterspace`.
    pub null_delimiter_space: Scaled,
    /// `\delimiterfactor`.
    pub delimiter_factor: i32,
    /// `\delimitershortfall`.
    pub delimiter_shortfall: Scaled,
    /// `\binoppenalty`.
    pub bin_op_penalty: i32,
    /// `\relpenalty`.
    pub rel_penalty: i32,
    /// `\skewchar` of a font.
    pub skew_char: &'a dyn Fn(FontId) -> i32,
}

/// The errors and warnings tex.web would issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathDiag {
    /// "\textfont n is undefined (character c)" (§723); size 0..=2.
    UndefinedFamily {
        /// Size code.
        size: usize,
        /// Family.
        fam: u8,
        /// Character.
        ch: u32,
    },
    /// `char_warning`: the character is not in the font (§722).
    MissingChar {
        /// Font.
        font: FontId,
        /// Character.
        ch: u32,
    },
}

/// `thin_mu_skip_code` (§224).
const THIN_MU_SKIP: u8 = 15;
/// `med_mu_skip_code`.
const MED_MU_SKIP: u8 = 16;
/// `thick_mu_skip_code`.
const THICK_MU_SKIP: u8 = 17;

/// §764: the inter-atom spacing table, Ord..Inner by Ord..Inner.
const MATH_SPACING: &[u8; 64] = b"02340001\
22*40001\
33**3**3\
44*04004\
00*00000\
02340001\
11*11111\
12341011";

/// The noad types that matter between passes (§682), in tex.web's
/// order from `ord_noad`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum T {
    Ord = 0,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Inner,
    Radical,
    Fraction,
    Under,
    Over,
    Accent,
    Vcenter,
    Left,
}

/// An item after the first pass.
enum P {
    Noad { t: T, hlist: List },
    LeftRight { delim: Delimiter, right: bool },
    Style(Style),
    Node(Node),
}

struct Ctx<'a> {
    p: &'a MathParams<'a>,
    fonts: &'a dyn Fonts,
    cur_style: Style,
    cur_size: usize,
    cur_mu: Scaled,
    diags: Vec<MathDiag>,
    /// XeTeX's `cur_f`: the font of the last character fetched (`None`:
    /// the null font), which XeTeX reads after the fact.
    cur_f: Option<FontId>,
}

/// The result of `fetch` (§722): `i` a TFM character's metrics, `native`
/// a native font's character (XeTeX §765: `cur_i` null, no warning).
struct Fetched {
    f: FontId,
    c: u32,
    i: Option<CharInfo>,
    native: bool,
}

/// XeTeX's accent subtypes (`\Umathaccent fixed`, `bottom`).
pub const FIXED_ACC: u8 = 1;
pub const BOTTOM_ACC: u8 = 2;
/// `get_ot_math_accent_pos` without an attachment point.
const NO_ACCENT_POS: Scaled = 0x7FFF_FFFF;
/// `min_halfword` (XeTeX: a glue node's word read as a width).
const MIN_HALFWORD: Scaled = -0xFFF_FFFF;
/// The OpenType `MATH` constants math reads (XeTeX's
/// `mathConstantIndex`).
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

/// A glyph node (XeTeX's `glyph_node` with `set_native_glyph_metrics(p,
/// 1)`).
fn glyph_node(nm: &dyn NativeMath, f: FontId, g: u16) -> Native {
    let (width, height, depth) = nm.glyph_whd(g);
    Native {
        kind: NativeKind::Glyph,
        font: f,
        width,
        height,
        depth,
        text: std::sync::Arc::from([]),
        glyphs: std::sync::Arc::from([Glyph { gid: g, x: 0, y: 0 }]),
    }
}

/// XeTeX's `is_glyph_node`: the glyph of a node, if it is one.
fn glyph_of(n: Option<&Node>) -> Option<&Native> {
    match n {
        Some(Node::Native(g)) if g.kind == NativeKind::Glyph => Some(g),
        _ => None,
    }
}

/// XeTeX's `stack_glyph_into_box`: glyph `g` after the others in an
/// hlist (the first one's height and depth not counted), under them in
/// a vlist.
fn stack_glyph_into_box(b: &mut BoxNode, g: Native) {
    if b.kind == BoxKind::V {
        b.height = g.height;
        b.width = b.width.max(g.width);
        b.list.insert(0, Node::Native(Box::new(g)));
    } else {
        if !b.list.is_empty() {
            b.height = b.height.max(g.height);
            b.depth = b.depth.max(g.depth);
        }
        b.list.push(Node::Native(Box::new(g)));
    }
}

/// XeTeX's `stack_glue_into_box`: glue of width `min` stretching to
/// `max`. In a vlist it sets the box's width to the glue node's `width`
/// word, which holds its `leader_ptr`: `null`.
fn stack_glue_into_box(b: &mut BoxNode, min: Scaled, max: Scaled) {
    let g = Node::glue(Glue {
        width: min,
        stretch: max - min,
        ..Glue::ZERO
    });
    if b.kind == BoxKind::V {
        b.list.insert(0, g);
        b.width = MIN_HALFWORD;
    } else {
        b.list.push(g);
    }
}

fn new_kern(w: Scaled) -> Node {
    Node::Kern(w, KernKind::Normal)
}

fn fraction_rule(t: Scaled) -> Node {
    // §704
    Node::Rule(Rule {
        width: RUNNING,
        height: t,
        depth: 0,
    })
}

fn ss_glue() -> Node {
    Node::glue(Glue {
        width: 0,
        stretch: 0x1_0000,
        stretch_order: Order::Fil,
        shrink: 0x1_0000,
        shrink_order: Order::Fil,
    })
}

fn tag(i: Option<CharInfo>) -> CharTag {
    i.map_or(CharTag::None, |i| i.tag)
}

impl Ctx<'_> {
    fn pack() -> PackParams {
        // packaging inside math never reports (tex.web's hpack would, but
        // with the box-specific limits the caller sees through `badness`)
        PackParams {
            badness: 10000,
            fuzz: crate::arith::MAX_DIMEN,
            overfull_rule: 0,
        }
    }

    fn hpack_nat(&self, list: List) -> BoxNode {
        hpack(list, NATURAL, self.fonts, &Self::pack(), false).node
    }

    fn vpack_nat(list: List) -> BoxNode {
        vpack(list, NATURAL, &Self::pack()).node
    }

    /// §703
    fn set_style(&mut self, s: Style) {
        self.cur_style = s;
        self.cur_size = s.size();
        self.cur_mu = x_over_n(self.mathsy(6, self.cur_size), 18).value;
    }

    fn fam_fnt(&self, fam: u8, size: usize) -> Option<FontId> {
        match self.p.fam_fnt[size].get(usize::from(fam)) {
            Some(&f) => f,
            // (XeTeX's families past sixteen)
            None => self.fonts.math_family(fam, size).flatten(),
        }
    }

    /// The native font `f`, if it is one.
    fn nm(&self, f: Option<FontId>) -> Option<&dyn NativeMath> {
        self.fonts.native_math(f?)
    }

    /// XeTeX's `is_new_mathfont(f)`.
    fn is_new_mathfont(&self, f: Option<FontId>) -> bool {
        self.nm(f).is_some_and(NativeMath::is_math)
    }

    /// XeTeX's `is_new_mathfont(cur_f)`.
    fn cur_mathfont(&self) -> bool {
        self.is_new_mathfont(self.cur_f)
    }

    /// `get_ot_math_constant(cur_f, k)`.
    fn cur_constant(&self, k: u32) -> Scaled {
        self.nm(self.cur_f).map_or(0, |nm| nm.constant(k))
    }

    /// `mathsy(n)` at `size` (§700; XeTeX: an OpenType math font's from
    /// its constants).
    fn mathsy(&self, n: u16, size: usize) -> Scaled {
        let f = self.fam_fnt(2, size);
        match self.nm(f) {
            Some(nm) if nm.is_math() => nm.mathsy(n),
            _ => f.map_or(0, |f| self.fonts.param(f, n)),
        }
    }

    /// `mathex(n)` at `cur_size` (§701).
    fn mathex(&self, n: u16) -> Scaled {
        let f = self.fam_fnt(3, self.cur_size);
        match self.nm(f) {
            Some(nm) if nm.is_math() => nm.mathex(n),
            _ => f.map_or(0, |f| self.fonts.param(f, n)),
        }
    }

    fn axis_height(&self, size: usize) -> Scaled {
        self.mathsy(22, size)
    }

    fn math_x_height(&self, size: usize) -> Scaled {
        self.mathsy(5, size)
    }

    fn default_rule_thickness(&self) -> Scaled {
        self.mathex(8)
    }

    /// `fetch(a)` (§722, XeTeX §765): empties the field on failure;
    /// sets `cur_f`.
    fn fetch(&mut self, a: &mut Field) -> Fetched {
        let (fam, c) = match a {
            Field::MathChar { fam, ch } | Field::MathTextChar { fam, ch } => (*fam, *ch),
            _ => unreachable!("fetch of a field that is no character"),
        };
        let f = self.fam_fnt(fam, self.cur_size);
        self.cur_f = f;
        let Some(f) = f else {
            self.diags.push(MathDiag::UndefinedFamily {
                size: self.cur_size,
                fam,
                ch: c,
            });
            *a = Field::Empty;
            return Fetched {
                f: 0,
                c,
                i: None,
                native: false,
            };
        };
        if self.fonts.native_math(f).is_some() {
            return Fetched {
                f,
                c,
                i: None,
                native: true,
            };
        }
        let i = u16::try_from(c).ok().and_then(|c| self.fonts.char_info(f, c));
        if i.is_none() {
            self.diags.push(MathDiag::MissingChar { font: f, ch: c });
            *a = Field::Empty;
        }
        Fetched {
            f,
            c,
            i,
            native: false,
        }
    }

    /// `char_box(f, c)` (§709).
    fn char_box(&self, f: FontId, c: u32) -> BoxNode {
        let c = u16::try_from(c).unwrap_or(0);
        let i = self.fonts.char_info(f, c).unwrap_or_default();
        BoxNode {
            kind: BoxKind::H,
            width: i.width + i.italic,
            height: i.height,
            depth: i.depth,
            list: vec![Node::Char { font: f, ch: c }],
            ..BoxNode::default()
        }
    }

    fn height_plus_depth(&self, f: FontId, c: u16) -> Scaled {
        let i = self.fonts.char_info(f, c).unwrap_or_default();
        i.height + i.depth
    }

    /// XeTeX's `build_opentype_assembly`: a box at least `s` high (wide
    /// if `horiz`) of the parts `a` of font `f`, as few copies of the
    /// extenders as do, their overlaps glue that stretches.
    fn opentype_assembly(nm: &dyn NativeMath, f: FontId, a: &[GlyphPart], s: Scaled, horiz: bool) -> BoxNode {
        let mut b = BoxNode {
            kind: if horiz { BoxKind::H } else { BoxKind::V },
            ..BoxNode::default()
        };
        // how many repeats of each extender to use
        let min_o = nm.min_connector_overlap();
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
            // (XeTeX loops for ever on extenders that add nothing)
            if s_max >= s || no_extenders || n >= 1 << 16 {
                break;
            }
            n += 1;
        }
        // the box, with glue wherever an overlap occurs
        let mut prev_o = 0;
        for p in a {
            for _ in 0..if p.extender { n } else { 1 } {
                let oo = p.start_connector.min(prev_o);
                let o = oo.min(min_o);
                if oo > 0 {
                    stack_glue_into_box(&mut b, -oo, -o);
                }
                stack_glyph_into_box(&mut b, glyph_node(nm, f, p.glyph));
                prev_o = p.end_connector;
            }
        }
        // its natural size and stretch, and the glue set to reach `s`
        let (mut nat, mut stretch) = (0, 0);
        for p in &b.list {
            match p {
                Node::Native(g) => nat += if horiz { g.width } else { g.height + g.depth },
                Node::Glue(g) => {
                    nat += g.spec.width;
                    stretch += g.spec.stretch;
                }
                _ => {}
            }
        }
        let size = if s > nat && stretch > 0 {
            let o = (s - nat).min(stretch);
            b.glue.order = Order::Normal;
            b.glue.sign = GlueSign::Stretching;
            b.glue.ratio = f64::from(o) / f64::from(stretch);
            nat + crate::arith::zround(f64::from(stretch) * b.glue.ratio)
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

    /// `var_delimiter(d, s, v)` (§706; XeTeX §749: in an OpenType font
    /// the character's vertical variants are looked at, then its
    /// assembly).
    #[allow(clippy::too_many_lines)]
    fn var_delimiter(&self, d: Delimiter, s: usize, v: Scaled) -> BoxNode {
        // (the best so far: font and character, a glyph in a native font)
        let mut best: Option<(FontId, u32)> = None;
        let mut ext = None;
        let mut assembly: Option<Vec<GlyphPart>> = None;
        let mut w = 0;
        'found: for (z, x0) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // §707 (XeTeX: `x` a quarterword, made a glyph by the first
            // native font met, which the smaller sizes then look at)
            let mut x = x0 & 0xFFFF;
            if z == 0 && x == 0 {
                continue;
            }
            for size in (0..=s).rev() {
                let Some(g) = self.fam_fnt(z, size) else {
                    continue;
                };
                if let Some(nm) = self.fonts.native_math(g) {
                    x = u32::from(nm.map_char(x));
                    best = Some((g, x));
                    w = 0;
                    let mut n = 0;
                    loop {
                        let (y, u) = nm.variant(u16::try_from(x & 0xFFFF).unwrap_or(0), n, false);
                        if u > w {
                            best = Some((g, u32::from(y)));
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
                    // (none big enough: is the character extensible?)
                    assembly = nm.assembly(u16::try_from(x & 0xFFFF).unwrap_or(0), false);
                    if assembly.is_some() {
                        break 'found;
                    }
                    continue;
                }
                // §708
                let mut y = u16::try_from(x).ok();
                while let Some(q) = y.and_then(|y| self.fonts.char_info(g, y)) {
                    let yc = u32::from(y.unwrap_or(0));
                    if let CharTag::Extensible { top, mid, bot, rep } = q.tag {
                        best = Some((g, yc));
                        ext = Some((top, mid, bot, rep));
                        break 'found;
                    }
                    let u = q.height + q.depth;
                    if u > w {
                        best = Some((g, yc));
                        w = u;
                        if u >= v {
                            break 'found;
                        }
                    }
                    match q.tag {
                        CharTag::List(n) => y = Some(n),
                        _ => break,
                    }
                }
            }
        }
        let native = best.and_then(|(f, _)| self.fonts.native_math(f).map(|nm| (f, nm)));
        let mut b = match (best, ext, native) {
            (Some(_), _, Some((f, nm))) => {
                let c = best.map_or(0, |b| u16::try_from(b.1 & 0xFFFF).unwrap_or(0));
                if let Some(a) = &assembly {
                    Self::opentype_assembly(nm, f, a, v, false)
                } else {
                    let g = glyph_node(nm, f, c);
                    BoxNode {
                        kind: BoxKind::V,
                        width: g.width,
                        height: g.height,
                        depth: g.depth,
                        list: vec![Node::Native(Box::new(g))],
                        ..BoxNode::default()
                    }
                }
            }
            (Some((f, _)), Some((top, mid, bot, rep)), None) => {
                // §713, §714
                let mut b = BoxNode {
                    kind: BoxKind::V,
                    ..BoxNode::default()
                };
                let u = self.height_plus_depth(f, rep);
                let ri = self.fonts.char_info(f, rep).unwrap_or_default();
                b.width = ri.width + ri.italic;
                let mut w = 0;
                for c in [bot, mid, top].into_iter().flatten() {
                    w += self.height_plus_depth(f, c);
                }
                let mut n = 0;
                if u > 0 {
                    while w < v {
                        w += u;
                        n += 1;
                        if mid.is_some() {
                            w += u;
                        }
                    }
                }
                let stack = |c: u16, b: &mut BoxNode| {
                    let p = self.char_box(f, u32::from(c));
                    b.height = p.height;
                    b.list.insert(0, Node::Box(Box::new(p)));
                };
                if let Some(c) = bot {
                    stack(c, &mut b);
                }
                for _ in 0..n {
                    stack(rep, &mut b);
                }
                if let Some(c) = mid {
                    stack(c, &mut b);
                    for _ in 0..n {
                        stack(rep, &mut b);
                    }
                }
                if let Some(c) = top {
                    stack(c, &mut b);
                }
                b.depth = w - b.height;
                b
            }
            (Some((f, c)), None, None) => self.char_box(f, c),
            (None, _, _) => BoxNode {
                width: self.p.null_delimiter_space,
                ..BoxNode::default()
            },
        };
        b.shift = half(b.height - b.depth) - self.axis_height(s);
        b
    }

    /// `rebox(b, w)` (§715).
    fn rebox(&self, b: BoxNode, w: Scaled) -> BoxNode {
        if b.width != w && !b.list.is_empty() {
            let b = if b.kind == BoxKind::V {
                self.hpack_nat(vec![Node::Box(Box::new(b))])
            } else {
                b
            };
            let mut list = b.list;
            if let [Node::Char { font, ch }] = list.as_slice() {
                let v = self.fonts.char_info(*font, *ch).map_or(0, |i| i.width);
                if v != b.width {
                    list.push(new_kern(b.width - v));
                }
            }
            list.insert(0, ss_glue());
            list.push(ss_glue());
            hpack(list, Spec::Exactly(w), self.fonts, &Self::pack(), false).node
        } else {
            BoxNode { width: w, ..b }
        }
    }

    /// `math_glue(g, m)` (§716).
    fn math_glue(g: &Glue, m: Scaled) -> Glue {
        let d = x_over_n(m, 0o200000);
        let (mut n, mut f) = (d.value, d.remainder);
        if f < 0 {
            n -= 1;
            f += 0o200000;
        }
        let mu_mult = |x: Scaled| nx_plus_y(n, x, xn_over_d(x, f, 0o200000).value).unwrap_or(0);
        Glue {
            width: mu_mult(g.width),
            stretch: if g.stretch_order == Order::Normal {
                mu_mult(g.stretch)
            } else {
                g.stretch
            },
            stretch_order: g.stretch_order,
            shrink: if g.shrink_order == Order::Normal {
                mu_mult(g.shrink)
            } else {
                g.shrink
            },
            shrink_order: g.shrink_order,
        }
    }

    /// `clean_box(p, s)` (§720).
    fn clean_box(&mut self, p: &Field, s: Style) -> BoxNode {
        let q: List = match p {
            Field::MathChar { .. } | Field::MathTextChar { .. } | Field::SubMlist(_) => {
                let mlist = match p {
                    Field::SubMlist(m) => m.clone(),
                    _ => vec![MItem::Noad(Noad::new(NoadKind::Ord, p.clone()))],
                };
                let save_style = self.cur_style;
                let h = self.mlist_to_hlist(mlist, s, false);
                self.set_style(save_style);
                h
            }
            Field::SubBox(b) => vec![Node::Box(Box::new(b.clone()))],
            Field::Empty => vec![Node::Box(Box::default())],
        };
        // found:
        let mut x = match q.as_slice() {
            [Node::Box(b)] if b.shift == 0 => (**b).clone(),
            _ => self.hpack_nat(q),
        };
        // §721
        if let [Node::Char { .. }, Node::Kern(..)] = x.list.as_slice() {
            x.list.pop();
        }
        x
    }

    /// `overbar(b, k, t)` (§705).
    fn overbar(b: BoxNode, k: Scaled, t: Scaled) -> BoxNode {
        Self::vpack_nat(vec![
            new_kern(t),
            fraction_rule(t),
            new_kern(k),
            Node::Box(Box::new(b)),
        ])
    }

    /// `make_over` (§734).
    fn make_over(&mut self, q: &mut Noad) {
        let t = self.default_rule_thickness();
        let x = self.clean_box(&q.nucleus, self.cur_style.cramped());
        q.nucleus = Field::SubBox(Self::overbar(x, 3 * t, t));
    }

    /// `make_under` (§735).
    fn make_under(&mut self, q: &mut Noad) {
        let t = self.default_rule_thickness();
        let x = self.clean_box(&q.nucleus, self.cur_style);
        let xh = x.height;
        let mut y = Self::vpack_nat(vec![Node::Box(Box::new(x)), new_kern(3 * t), fraction_rule(t)]);
        let delta = y.height + y.depth + t;
        y.height = xh;
        y.depth = delta - y.height;
        q.nucleus = Field::SubBox(y);
    }

    /// `make_vcenter` (§736).
    fn make_vcenter(&mut self, q: &mut Noad) {
        let axis = self.axis_height(self.cur_size);
        let Field::SubBox(v) = &mut q.nucleus else {
            panic!("vcenter: nucleus is no box")
        };
        assert!(v.kind == BoxKind::V, "vcenter");
        let delta = v.height + v.depth;
        v.height = axis + half(delta);
        v.depth = delta - v.height;
    }

    /// `make_radical` (§737; XeTeX §780: an OpenType math font's rule
    /// and gaps).
    fn make_radical(&mut self, q: &mut Noad, d: Delimiter) {
        let f = self.fam_fnt(d.small_fam, self.cur_size);
        let ot = self.is_new_mathfont(f);
        let constant = |cx: &Self, k| cx.nm(f).map_or(0, |nm| nm.constant(k));
        let t = if ot {
            constant(self, RADICAL_RULE_THICKNESS)
        } else {
            self.default_rule_thickness()
        };
        let x = self.clean_box(&q.nucleus, self.cur_style.cramped());
        let mut clr = if ot {
            if self.cur_style < Style::TEXT {
                constant(self, RADICAL_DISPLAY_STYLE_VERTICAL_GAP)
            } else {
                constant(self, RADICAL_VERTICAL_GAP)
            }
        } else if self.cur_style < Style::TEXT {
            t + self.math_x_height(self.cur_size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let mut y = self.var_delimiter(d, self.cur_size, x.height + x.depth + clr + t);
        if ot {
            y.depth = y.height + y.depth - t;
            y.height = t;
        }
        let delta = y.depth - (x.height + x.depth + clr);
        if delta > 0 {
            clr += half(delta);
        }
        y.shift = -(x.height + clr);
        let yh = y.height;
        let over = Self::overbar(x, clr, yh);
        q.nucleus = Field::SubBox(self.hpack_nat(vec![Node::Box(Box::new(y)), Node::Box(Box::new(over))]));
    }

    /// XeTeX's `compute_ot_math_accent_pos`: the top accent attachment of
    /// the nucleus's character (or of the nucleus of an accent that is its
    /// subformula's first noad).
    fn compute_ot_math_accent_pos(&mut self, nucleus: &mut Field) -> Scaled {
        match nucleus {
            Field::MathChar { ch, .. } => {
                let ch = *ch;
                self.fetch(nucleus);
                match self.nm(self.cur_f) {
                    Some(nm) => nm.accent_pos(nm.char_glyph(ch)),
                    // (XeTeX makes a native character of a TFM font's)
                    None => NO_ACCENT_POS,
                }
            }
            Field::SubMlist(l) => match l.first_mut() {
                Some(MItem::Noad(r)) if matches!(r.kind, NoadKind::Accent(..)) => {
                    self.compute_ot_math_accent_pos(&mut r.nucleus)
                }
                _ => NO_ACCENT_POS,
            },
            _ => NO_ACCENT_POS,
        }
    }

    /// `make_math_accent` (§738; XeTeX §781: accents of native fonts,
    /// placed by their attachment points, growing by their horizontal
    /// variants and assemblies; bottom and fixed accents).
    #[allow(clippy::too_many_lines)]
    fn make_math_accent(&mut self, q: &mut Noad, fam: u8, ch: u32, sub: u8) {
        let bottom = sub & BOTTOM_ACC != 0;
        let mut acc = Field::MathChar { fam, ch };
        let a = self.fetch(&mut acc);
        let (f, mut c) = (a.f, a.c);
        let (mut s, mut x, w, mut h);
        let mut tfm_i = None;
        if a.native {
            s = if bottom {
                0
            } else {
                self.compute_ot_math_accent_pos(&mut q.nucleus)
            };
            x = self.clean_box(&q.nucleus, self.cur_style.cramped());
            w = x.width;
            h = x.height;
        } else if let Some(mut i) = a.i {
            // §741
            s = 0;
            if matches!(q.nucleus, Field::MathChar { .. }) {
                let n = self.fetch(&mut q.nucleus);
                if n.i.is_some()
                    && let Ok(skew) = u16::try_from((self.p.skew_char)(n.f))
                    && let Ok(nc) = u16::try_from(n.c)
                    && let Some(LigKern::Kern(k)) = self.fonts.lig_kern(n.f, Left::Char(nc), skew)
                {
                    s = k;
                }
            }
            x = self.clean_box(&q.nucleus, self.cur_style.cramped());
            w = x.width;
            h = x.height;
            // §740
            while let CharTag::List(y) = i.tag {
                let Some(i2) = self.fonts.char_info(f, y) else {
                    break;
                };
                if i2.width > w {
                    break;
                }
                c = u32::from(y);
                i = i2;
            }
            tfm_i = Some(i);
        } else {
            return;
        }
        let mut delta = if self.is_new_mathfont(Some(f)) {
            if bottom {
                0
            } else {
                h.min(self.nm(Some(f)).map_or(0, |nm| nm.constant(ACCENT_BASE_HEIGHT)))
            }
        } else {
            h.min(self.fonts.param(f, 5))
        };
        if (q.supscr != Field::Empty || q.subscr != Field::Empty)
            && matches!(q.nucleus, Field::MathChar { .. })
        {
            // §742
            let inner = Noad {
                kind: NoadKind::Ord,
                nucleus: std::mem::take(&mut q.nucleus),
                supscr: std::mem::take(&mut q.supscr),
                subscr: std::mem::take(&mut q.subscr),
            };
            q.nucleus = Field::SubMlist(vec![MItem::Noad(inner)]);
            x = self.clean_box(&q.nucleus, self.cur_style);
            delta = delta + x.height - h;
            h = x.height;
        }
        let native = if tfm_i.is_none() { self.nm(Some(f)) } else { None };
        let mut y = if let Some(nm) = native {
            // the accent as a glyph, larger as the accentee is wider
            let g0 = nm.char_glyph(c);
            let mut p = glyph_node(nm, f, g0);
            let mut assembly = None;
            if sub & FIXED_ACC == 0 {
                let mut k = 0;
                loop {
                    let (g, w2) = nm.variant(g0, k, true);
                    if w2 > 0 && w2 <= w {
                        p = glyph_node(nm, f, g);
                        k += 1;
                    }
                    // (XeTeX loops for ever on a variant of no width)
                    if w2 < 0 || w2 >= w || w2 == 0 {
                        if w2 < 0 {
                            assembly = nm.assembly(g0, true).map(|a| Self::opentype_assembly(nm, f, &a, w, true));
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
                    list: vec![Node::Box(Box::new(b))],
                    ..BoxNode::default()
                },
                None => BoxNode {
                    width: p.width,
                    height: p.height,
                    depth: p.depth,
                    list: vec![Node::Native(Box::new(p))],
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
                Some(g) => match nm.accent_pos(g.glyphs.first().map_or(0, |g| g.gid)) {
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
        } else {
            let mut y = self.char_box(f, c);
            y.shift = s + half(w - y.width);
            y
        };
        y.width = 0;
        let xw = x.width;
        let mut yv = if bottom {
            let xh = h;
            let mut y = Self::vpack_nat(vec![Node::Box(Box::new(x)), Node::Box(Box::new(y))]);
            y.shift = -(xh - y.height);
            y
        } else {
            let mut yv = Self::vpack_nat(vec![Node::Box(Box::new(y)), new_kern(-delta), Node::Box(Box::new(x))]);
            if yv.height < h {
                // §739
                yv.list.insert(0, new_kern(h - yv.height));
                yv.height = h;
            }
            yv
        };
        yv.width = xw;
        q.nucleus = Field::SubBox(yv);
    }

    /// `make_fraction` (§743); returns `new_hlist`.
    #[allow(clippy::too_many_lines)]
    fn make_fraction(&mut self, q: &mut Fraction) -> List {
        let thickness = match q.thickness {
            Some(t) => t,
            None => self.default_rule_thickness(),
        };
        q.thickness = Some(thickness);
        // §744
        let mut x = self.clean_box(
            &Field::SubMlist(std::mem::take(&mut q.numerator)),
            self.cur_style.num(),
        );
        let mut z = self.clean_box(
            &Field::SubMlist(std::mem::take(&mut q.denominator)),
            self.cur_style.denom(),
        );
        if x.width < z.width {
            x = self.rebox(x, z.width);
        } else {
            z = self.rebox(z, x.width);
        }
        let s = self.cur_size;
        let (mut shift_up, mut shift_down) = if self.cur_style < Style::TEXT {
            (self.mathsy(8, s), self.mathsy(11, s))
        } else {
            (
                if thickness == 0 {
                    self.mathsy(10, s)
                } else {
                    self.mathsy(9, s)
                },
                self.mathsy(12, s),
            )
        };
        let axis = self.axis_height(s);
        let delta;
        // (XeTeX §789, §790: the gaps of `cur_f`, the font of the last
        // character fetched, if an OpenType math font)
        let ot = self.cur_mathfont();
        let display = self.cur_style < Style::TEXT;
        if thickness == 0 {
            // §745
            let t = self.default_rule_thickness();
            let clr = if ot {
                self.cur_constant(if display {
                    STACK_DISPLAY_STYLE_GAP_MIN
                } else {
                    STACK_GAP_MIN
                })
            } else if display {
                7 * t
            } else {
                3 * t
            };
            let d = half(clr - ((shift_up - x.depth) - (z.height - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
            delta = 0;
        } else {
            // §746
            let (clr1, clr2) = if ot {
                if display {
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
            } else if display {
                (3 * thickness, 3 * thickness)
            } else {
                (thickness, thickness)
            };
            delta = half(thickness);
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
            kind: BoxKind::V,
            height: shift_up + x.height,
            depth: z.depth + shift_down,
            width: x.width,
            ..BoxNode::default()
        };
        let (xd, zh) = (x.depth, z.height);
        v.list.push(Node::Box(Box::new(x)));
        if thickness == 0 {
            v.list.push(new_kern((shift_up - xd) - (zh - shift_down)));
        } else {
            v.list.push(new_kern((shift_up - xd) - (axis + delta)));
            v.list.push(fraction_rule(thickness));
            v.list.push(new_kern((axis - delta) - (zh - shift_down)));
        }
        v.list.push(Node::Box(Box::new(z)));
        // §748
        let delta = if self.cur_style < Style::TEXT {
            self.mathsy(20, s)
        } else {
            self.mathsy(21, s)
        };
        let l = self.var_delimiter(q.left, s, delta);
        let r = self.var_delimiter(q.right, s, delta);
        vec![Node::Box(Box::new(self.hpack_nat(vec![
            Node::Box(Box::new(l)),
            Node::Box(Box::new(v)),
            Node::Box(Box::new(r)),
        ])))]
    }

    /// `make_op` (§749); returns delta and, with limits, `new_hlist`.
    #[allow(clippy::too_many_lines)]
    fn make_op(&mut self, q: &mut Noad) -> (Scaled, Option<List>) {
        let NoadKind::Op(lim) = &mut q.kind else {
            unreachable!()
        };
        if *lim == Limits::Normal && self.cur_style < Style::TEXT {
            *lim = Limits::Limits;
        }
        let limits = *lim == Limits::Limits;
        let mut delta = 0;
        if matches!(q.nucleus, Field::MathChar { .. }) {
            let mut fc = self.fetch(&mut q.nucleus);
            if !fc.native {
                if self.cur_style < Style::TEXT
                    && let CharTag::List(c) = tag(fc.i)
                    && let Some(i) = self.fonts.char_info(fc.f, c)
                {
                    fc.c = u32::from(c);
                    fc.i = Some(i);
                    if let Field::MathChar { ch, .. } = &mut q.nucleus {
                        // (XeTeX: the character's low 16 bits, its plane kept)
                        *ch = (*ch & !0xFFFF) | u32::from(c);
                    }
                }
                delta = fc.i.map_or(0, |i| i.italic);
            }
            let mut x = self.clean_box(&q.nucleus, self.cur_style);
            if self.cur_mathfont()
                && let Some(f) = self.cur_f
                && let Some(nm) = self.fonts.native_math(f)
                && let Some(p0) = glyph_of(x.list.first())
            {
                // XeTeX §793: in display style, a variant at least
                // `displayOperatorMinHeight` high and larger than the
                // text-style glyph, else the glyph's assembly
                let mut p = p0.clone();
                let mut assembly = None;
                if self.cur_style < Style::TEXT {
                    let mut h1 = nm.constant(DISPLAY_OPERATOR_MIN_HEIGHT);
                    let h = (p.height + p.depth).wrapping_mul(5) / 4;
                    if h1 < h {
                        h1 = h;
                    }
                    let c = p.glyphs.first().map_or(0, |g| g.gid);
                    let mut n = 0;
                    loop {
                        let (g, h2) = nm.variant(c, n, false);
                        if h2 > 0 {
                            p = glyph_node(nm, f, g);
                        }
                        n += 1;
                        if h2 < 0 || h2 >= h1 {
                            if h2 < 0 {
                                assembly = nm.assembly(c, false).map(|a| Self::opentype_assembly(nm, f, &a, h1, false));
                            }
                            break;
                        }
                    }
                }
                let (w, h, d) = if let Some(b) = assembly {
                    delta = 0;
                    let dims = (b.width, b.height, b.depth);
                    x.list = vec![Node::Box(Box::new(b))];
                    dims
                } else {
                    delta = nm.ital_corr(p.glyphs.first().map_or(0, |g| g.gid));
                    let dims = (p.width, p.height, p.depth);
                    x.list[0] = Node::Native(Box::new(p));
                    dims
                };
                x.width = w;
                x.height = h;
                x.depth = d;
            }
            if q.subscr != Field::Empty && !limits {
                x.width -= delta;
            }
            x.shift = half(x.height - x.depth) - self.axis_height(self.cur_size);
            q.nucleus = Field::SubBox(x);
        }
        if !limits {
            return (delta, None);
        }
        // §750 (XeTeX: `cur_f` as the operator left it)
        let save_f = self.cur_f;
        let x = self.clean_box(&q.supscr, self.cur_style.sup());
        let y = self.clean_box(&q.nucleus, self.cur_style);
        let z = self.clean_box(&q.subscr, self.cur_style.sub());
        self.cur_f = save_f;
        let mut v = BoxNode {
            kind: BoxKind::V,
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
        if q.supscr == Field::Empty {
            v.list.push(Node::Box(Box::new(y)));
        } else {
            let shift_up = (self.mathex(11) - x.depth).max(self.mathex(9));
            let bos5 = self.mathex(13);
            v.height += bos5 + x.height + x.depth + shift_up;
            v.list.push(new_kern(bos5));
            v.list.push(Node::Box(Box::new(x)));
            v.list.push(new_kern(shift_up));
            v.list.push(Node::Box(Box::new(y)));
        }
        if q.subscr != Field::Empty {
            let shift_down = (self.mathex(12) - z.height).max(self.mathex(10));
            let bos5 = self.mathex(13);
            v.depth += bos5 + z.height + z.depth + shift_down;
            v.list.push(new_kern(shift_down));
            v.list.push(Node::Box(Box::new(z)));
            v.list.push(new_kern(bos5));
        }
        (delta, Some(vec![Node::Box(Box::new(v))]))
    }

    /// `make_ord` (§752, §753); `rest` is the mlist after `q`.
    fn make_ord(&mut self, q: &mut Noad, rest: &mut VecDeque<MItem>) {
        loop {
            // restart:
            if q.subscr != Field::Empty || q.supscr != Field::Empty {
                return;
            }
            let Field::MathChar { fam, .. } = q.nucleus else {
                return;
            };
            let Some(MItem::Noad(p)) = rest.front() else {
                return;
            };
            if !matches!(
                p.kind,
                NoadKind::Ord
                    | NoadKind::Op(_)
                    | NoadKind::Bin
                    | NoadKind::Rel
                    | NoadKind::Open
                    | NoadKind::Close
                    | NoadKind::Punct
            ) {
                return;
            }
            let Field::MathChar { fam: pf, ch: pc } = p.nucleus else {
                return;
            };
            if pf != fam {
                return;
            }
            if let Field::MathChar { fam, ch } = q.nucleus {
                q.nucleus = Field::MathTextChar { fam, ch };
            }
            let cur = self.fetch(&mut q.nucleus);
            if cur.i.is_none() {
                return;
            }
            // (XeTeX compares the next character's low 16 bits)
            let (Ok(cc), Ok(pc16)) = (u16::try_from(cur.c), u16::try_from(pc & 0xFFFF)) else {
                return;
            };
            let Some(instr) = self.fonts.lig_kern(cur.f, Left::Char(cc), pc16) else {
                return;
            };
            match instr {
                LigKern::Kern(k) => {
                    rest.push_front(MItem::Node(new_kern(k)));
                    return;
                }
                LigKern::Lig(op, rem) => {
                    let rem = u32::from(rem);
                    let set_q = |q: &mut Noad, c: u32| {
                        if let Field::MathChar { ch, .. } | Field::MathTextChar { ch, .. } =
                            &mut q.nucleus
                        {
                            *ch = c;
                        }
                    };
                    match op {
                        LigOp::KeepRight | LigOp::KeepRightSkip => set_q(q, rem),
                        LigOp::KeepLeft | LigOp::KeepLeftSkip => {
                            if let Some(MItem::Noad(p)) = rest.front_mut() {
                                p.nucleus = Field::MathChar {
                                    fam: pf,
                                    ch: (pc & !0xFFFF) | rem,
                                };
                            }
                        }
                        LigOp::KeepBoth | LigOp::KeepBothSkip1 | LigOp::KeepBothSkip2 => {
                            let nucleus = if op == LigOp::KeepBothSkip2 {
                                Field::MathTextChar { fam, ch: rem }
                            } else {
                                Field::MathChar { fam, ch: rem }
                            };
                            rest.push_front(MItem::Noad(Noad::new(NoadKind::Ord, nucleus)));
                        }
                        LigOp::Plain => {
                            set_q(q, rem);
                            if let Some(MItem::Noad(p)) = rest.pop_front() {
                                q.subscr = p.subscr;
                                q.supscr = p.supscr;
                            }
                        }
                    }
                    if !matches!(
                        op,
                        LigOp::Plain | LigOp::KeepRight | LigOp::KeepLeft | LigOp::KeepBoth
                    ) {
                        return;
                    }
                    if let Field::MathTextChar { fam, ch } = q.nucleus {
                        q.nucleus = Field::MathChar { fam, ch };
                    }
                }
            }
        }
    }

    /// XeTeX §805 "Fetch first character of a sub/superscript": the
    /// font and glyph of script `field`'s first character if it is in an
    /// OpenType math font. Kerns and glue before it are passed, style
    /// changes obeyed, a `\mathchoice` followed; it is fetched in the
    /// style found (a missing one complained about and emptied, as
    /// `fetch` does).
    fn script_glyph(&mut self, field: &mut Field) -> Option<(FontId, u16)> {
        let mut this_style = self.cur_style.sub();
        let head = match field {
            Field::SubMlist(l) => first_nucleus(l, &mut this_style)?,
            f => f,
        };
        let Field::MathChar { ch, .. } = *head else {
            return None;
        };
        let save_f = self.cur_f;
        let saved_style = self.cur_style;
        self.set_style(this_style);
        self.fetch(head);
        let r = match self.nm(self.cur_f) {
            Some(nm) if nm.is_math() => self.cur_f.map(|f| (f, nm.char_glyph(ch))),
            _ => None,
        };
        self.cur_f = save_f;
        self.set_style(saved_style);
        r
    }

    /// XeTeX §806: the OpenType math kern between the nucleus `p` (if it
    /// starts with a glyph) and a script's glyph, shifted by `shift`;
    /// appended to `p` unless zero.
    fn attach_ot_kern(&self, p: &mut List, script: Option<(FontId, u16)>, sup: bool, shift: Scaled) -> Scaled {
        let Some(g) = glyph_of(p.first()) else {
            return 0;
        };
        let (Some(nm), Some((sf, sg))) = (self.fonts.native_math(g.font), script) else {
            return 0;
        };
        let Some(snm) = self.fonts.native_math(sf) else {
            return 0;
        };
        let k = nm.math_kern(g.glyphs.first().map_or(0, |g| g.gid), (snm, sg), sup, shift);
        if k != 0 {
            p.push(new_kern(k));
        }
        k
    }

    /// `make_scripts(q, delta)` (§756; XeTeX §800: an OpenType math
    /// font's script parameters and kerns, of `cur_f`); appends to
    /// `hlist`.
    #[allow(clippy::too_many_lines)]
    fn make_scripts(&mut self, q: &mut Noad, hlist: &mut List, delta: Scaled) {
        let (mut shift_up, mut shift_down) =
            if matches!(hlist.first(), Some(Node::Char { .. })) || glyph_of(hlist.first()).is_some() {
                (0, 0)
            } else {
                let z = self.hpack_nat(hlist.clone());
                let t = if self.cur_style < Style::SCRIPT { 1 } else { 2 };
                (z.height - self.mathsy(18, t), z.depth + self.mathsy(19, t))
            };
        let s = self.cur_size;
        let x = if q.supscr == Field::Empty {
            // §757
            let sub = self.script_glyph(&mut q.subscr);
            let save_f = self.cur_f;
            let mut x = self.clean_box(&q.subscr, self.cur_style.sub());
            self.cur_f = save_f;
            x.width += self.p.script_space;
            shift_down = shift_down.max(self.mathsy(16, s));
            let clr = if self.cur_mathfont() {
                x.height - self.cur_constant(SUBSCRIPT_TOP_MAX)
            } else {
                x.height - (self.math_x_height(s) * 4).abs() / 5
            };
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            if self.cur_mathfont() {
                self.attach_ot_kern(hlist, sub, false, shift_down);
            }
            x
        } else {
            // §758
            let sup = self.script_glyph(&mut q.supscr);
            let save_f = self.cur_f;
            let mut x = self.clean_box(&q.supscr, self.cur_style.sup());
            self.cur_f = save_f;
            x.width += self.p.script_space;
            let clr = if self.cur_style.0 % 2 == 1 {
                self.mathsy(15, s)
            } else if self.cur_style < Style::TEXT {
                self.mathsy(13, s)
            } else {
                self.mathsy(14, s)
            };
            shift_up = shift_up.max(clr);
            let clr = if self.cur_mathfont() {
                x.depth + self.cur_constant(SUPERSCRIPT_BOTTOM_MIN)
            } else {
                x.depth + self.math_x_height(s).abs() / 4
            };
            shift_up = shift_up.max(clr);
            if self.cur_mathfont() && q.subscr == Field::Empty {
                // (with a subscript the kern goes into the shift)
                self.attach_ot_kern(hlist, sup, true, shift_up);
            }
            if q.subscr == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // §759
                let save_f = self.cur_f;
                let sub = self.script_glyph(&mut q.subscr);
                let sub_empty = q.subscr == Field::Empty;
                let mut y = self.clean_box(&q.subscr, self.cur_style.sub());
                self.cur_f = save_f;
                y.width += self.p.script_space;
                shift_down = shift_down.max(self.mathsy(17, s));
                let ot = self.cur_mathfont();
                let mut clr = if ot {
                    self.cur_constant(SUB_SUPERSCRIPT_GAP_MIN)
                } else {
                    4 * self.default_rule_thickness()
                } - ((shift_up - x.depth) - (y.height - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    clr = if ot {
                        self.cur_constant(SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT)
                    } else {
                        (self.math_x_height(s) * 4).abs() / 5
                    } - (shift_up - x.depth);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                let (mut sub_kern, mut sup_kern) = (0, 0);
                if ot {
                    sub_kern = self.attach_ot_kern(hlist, sub, false, shift_down);
                    if sub_empty {
                        sup_kern = self.attach_ot_kern(hlist, sup, true, shift_up);
                    }
                }
                // (the superscript `delta` to the right of the subscript)
                x.shift = sup_kern + delta - sub_kern;
                let k = (shift_up - x.depth) - (y.height - shift_down);
                let mut v = Self::vpack_nat(vec![Node::Box(Box::new(x)), new_kern(k), Node::Box(Box::new(y))]);
                v.shift = shift_down;
                v
            }
        };
        hlist.push(Node::Box(Box::new(x)));
    }

    /// `make_left_right` (§762, e-TeX's form).
    fn make_left_right(
        &mut self,
        d: Delimiter,
        style: Style,
        max_d: Scaled,
        max_h: Scaled,
    ) -> List {
        self.set_style(style);
        let mut delta2 = max_d + self.axis_height(self.cur_size);
        let mut delta1 = max_h + max_d - delta2;
        if delta2 > delta1 {
            delta1 = delta2;
        }
        let mut delta = (delta1 / 500) * self.p.delimiter_factor;
        delta2 = delta1 + delta1 - self.p.delimiter_shortfall;
        if delta < delta2 {
            delta = delta2;
        }
        vec![Node::Box(Box::new(self.var_delimiter(d, self.cur_size, delta)))]
    }

    /// `mlist_to_hlist` (§726).
    #[allow(clippy::too_many_lines)]
    fn mlist_to_hlist(&mut self, mlist: MList, style: Style, penalties: bool) -> List {
        let mut items: VecDeque<MItem> = mlist.into();
        let mut out: Vec<P> = Vec::new();
        let mut r: Option<usize> = None;
        let mut r_type = T::Op;
        let mut max_h = 0;
        let mut max_d = 0;
        self.set_style(style);
        while let Some(item) = items.pop_front() {
            // §728
            let mut noad = match item {
                MItem::Style(s) => {
                    self.set_style(s);
                    out.push(P::Style(s));
                    continue;
                }
                MItem::Choice(c) => {
                    // §731
                    let [d, t, s, ss] = *c;
                    let chosen = match self.cur_style.0 / 2 {
                        0 => d,
                        1 => t,
                        2 => s,
                        _ => ss,
                    };
                    for it in chosen.into_iter().rev() {
                        items.push_front(it);
                    }
                    out.push(P::Style(self.cur_style));
                    continue;
                }
                MItem::Node(n) => {
                    // §730
                    let n = match n {
                        Node::Rule(rl) => {
                            max_h = max_h.max(rl.height);
                            max_d = max_d.max(rl.depth);
                            Node::Rule(rl)
                        }
                        Node::Glue(mut g) => {
                            // §732
                            if g.kind == GlueKind::Mu {
                                g.spec = Self::math_glue(&g.spec, self.cur_mu);
                                g.kind = GlueKind::Normal;
                            } else if self.cur_size != 0
                                && g.kind == GlueKind::CondMath
                                && matches!(
                                    items.front(),
                                    Some(MItem::Node(Node::Glue(_) | Node::Kern(..)))
                                )
                            {
                                items.pop_front();
                            }
                            Node::Glue(g)
                        }
                        Node::Kern(w, KernKind::Mu) => {
                            // §717
                            let g = Self::math_glue(&Glue::fixed(w), self.cur_mu);
                            Node::Kern(g.width, KernKind::Explicit)
                        }
                        n @ (Node::Kern(..)
                        | Node::Ins(_)
                        | Node::Mark(_)
                        | Node::Adjust(_)
                        | Node::Whatsit(..)
                        | Node::Language { .. }
                        | Node::Penalty(_)
                        | Node::Disc(_)) => n,
                        n => panic!("mlist1: {n:?}"),
                    };
                    out.push(P::Node(n));
                    continue;
                }
                MItem::Left(d) => {
                    out.push(P::LeftRight {
                        delim: d,
                        right: false,
                    });
                    r = Some(out.len() - 1);
                    r_type = T::Left;
                    continue;
                }
                MItem::Right(d, middle) => {
                    // §729
                    if r_type == T::Bin {
                        set_ord(&mut out, r);
                    }
                    // `\middle` is a right noad too (etex.ch)
                    let _ = middle;
                    out.push(P::LeftRight {
                        delim: d,
                        right: true,
                    });
                    r = Some(out.len() - 1);
                    // e-TeX: done_with_noad of a right noad
                    r_type = T::Left;
                    self.set_style(style);
                    continue;
                }
                MItem::Fraction(mut f) => {
                    let h = self.make_fraction(&mut f);
                    self.check_dimensions(&h, &mut max_h, &mut max_d);
                    out.push(P::Noad {
                        t: T::Fraction,
                        hlist: h,
                    });
                    r = Some(out.len() - 1);
                    r_type = T::Fraction;
                    continue;
                }
                MItem::Noad(n) => n,
            };
            let mut delta = 0;
            if noad.kind == NoadKind::Bin
                && matches!(
                    r_type,
                    T::Bin | T::Op | T::Rel | T::Open | T::Punct | T::Left
                )
            {
                noad.kind = NoadKind::Ord;
            }
            let t = match &noad.kind {
                NoadKind::Ord => T::Ord,
                NoadKind::Op(_) => T::Op,
                NoadKind::Bin => T::Bin,
                NoadKind::Rel => T::Rel,
                NoadKind::Open => T::Open,
                NoadKind::Close => T::Close,
                NoadKind::Punct => T::Punct,
                NoadKind::Inner => T::Inner,
                NoadKind::Radical(_) => T::Radical,
                NoadKind::Over => T::Over,
                NoadKind::Under => T::Under,
                NoadKind::Accent(..) => T::Accent,
                NoadKind::Vcenter => T::Vcenter,
            };
            if matches!(t, T::Rel | T::Close | T::Punct) && r_type == T::Bin {
                set_ord(&mut out, r);
            }
            let mut done: Option<List> = None;
            match noad.kind.clone() {
                NoadKind::Op(_) => {
                    let (d, h) = self.make_op(&mut noad);
                    delta = d;
                    done = h;
                }
                NoadKind::Ord => self.make_ord(&mut noad, &mut items),
                NoadKind::Radical(d) => self.make_radical(&mut noad, d),
                NoadKind::Over => self.make_over(&mut noad),
                NoadKind::Under => self.make_under(&mut noad),
                NoadKind::Accent(fam, ch, sub) => self.make_math_accent(&mut noad, fam, ch, sub),
                NoadKind::Vcenter => self.make_vcenter(&mut noad),
                _ => {}
            }
            let hlist = if let Some(h) = done {
                h
            } else {
                // §754
                let mut p: List = match &mut noad.nucleus {
                    f @ (Field::MathChar { .. } | Field::MathTextChar { .. }) => {
                        // §755
                        let text = matches!(f, Field::MathTextChar { .. });
                        let fc = self.fetch(f);
                        if fc.native
                            && let Some(nm) = self.fonts.native_math(fc.f)
                        {
                            // XeTeX §799: a glyph node
                            let g = glyph_node(nm, fc.f, nm.char_glyph(fc.c));
                            delta = nm.ital_corr(g.glyphs.first().map_or(0, |g| g.gid));
                            if text && !nm.is_math() {
                                delta = 0;
                            }
                            let mut p = vec![Node::Native(Box::new(g))];
                            if noad.subscr == Field::Empty && delta != 0 {
                                p.push(new_kern(delta));
                                delta = 0;
                            }
                            p
                        } else if let Some(i) = fc.i {
                            delta = i.italic;
                            let mut p = vec![Node::Char {
                                font: fc.f,
                                ch: u16::try_from(fc.c).unwrap_or(0),
                            }];
                            if text && self.fonts.param(fc.f, 2) != 0 {
                                delta = 0;
                            }
                            if noad.subscr == Field::Empty && delta != 0 {
                                p.push(new_kern(delta));
                                delta = 0;
                            }
                            p
                        } else {
                            Vec::new()
                        }
                    }
                    Field::Empty => Vec::new(),
                    Field::SubBox(b) => vec![Node::Box(Box::new(std::mem::take(b)))],
                    Field::SubMlist(m) => {
                        let save_style = self.cur_style;
                        let h = self.mlist_to_hlist(std::mem::take(m), save_style, false);
                        self.set_style(save_style);
                        vec![Node::Box(Box::new(self.hpack_nat(h)))]
                    }
                };
                if noad.subscr != Field::Empty || noad.supscr != Field::Empty {
                    self.make_scripts(&mut noad, &mut p, delta);
                }
                p
            };
            self.check_dimensions(&hlist, &mut max_h, &mut max_d);
            out.push(P::Noad { t, hlist });
            r = Some(out.len() - 1);
            r_type = t;
        }
        // §729
        if r_type == T::Bin {
            set_ord(&mut out, r);
        }
        // §760: the second pass
        let mut hl = List::new();
        let mut r_type: Option<T> = None;
        self.set_style(style);
        let n = out.len();
        // what follows each item, for §767
        let next_is_rel: Vec<bool> = out
            .iter()
            .map(|p| matches!(p, P::Noad { t: T::Rel, .. }))
            .collect();
        let next_is_penalty: Vec<bool> = out
            .iter()
            .map(|p| matches!(p, P::Node(Node::Penalty(_))))
            .collect();
        for (k, p) in out.into_iter().enumerate() {
            // §761
            let mut pen = INF_PENALTY;
            let (t, list, is_right) = match p {
                P::Style(s) => {
                    self.set_style(s);
                    continue;
                }
                P::Node(n) => {
                    hl.push(n);
                    continue;
                }
                P::Noad { t, hlist } => {
                    let t = match t {
                        T::Op | T::Open | T::Close | T::Punct | T::Inner => t,
                        T::Bin => {
                            pen = self.p.bin_op_penalty;
                            T::Bin
                        }
                        T::Rel => {
                            pen = self.p.rel_penalty;
                            T::Rel
                        }
                        _ => T::Ord,
                    };
                    (t, hlist, false)
                }
                P::LeftRight { delim, right } => {
                    let h = self.make_left_right(delim, style, max_d, max_h);
                    (if right { T::Close } else { T::Open }, h, right)
                }
            };
            // §766
            if let Some(rt) = r_type {
                let x = match MATH_SPACING[(rt as usize) * 8 + t as usize] {
                    b'0' => None,
                    b'1' => (self.cur_style < Style::SCRIPT).then_some(THIN_MU_SKIP),
                    b'2' => Some(THIN_MU_SKIP),
                    b'3' => (self.cur_style < Style::SCRIPT).then_some(MED_MU_SKIP),
                    b'4' => (self.cur_style < Style::SCRIPT).then_some(THICK_MU_SKIP),
                    _ => panic!("mlist4"),
                };
                if let Some(x) = x {
                    let g = match x {
                        THIN_MU_SKIP => self.p.thin_mu_skip,
                        MED_MU_SKIP => self.p.med_mu_skip,
                        _ => self.p.thick_mu_skip,
                    };
                    hl.push(Node::Glue(GlueNode {
                        spec: Self::math_glue(&g, self.cur_mu),
                        kind: GlueKind::Param(x),
                        own_spec: false,
                        leader: None,
                    }));
                }
            }
            // §767
            hl.extend(list);
            if penalties
                && k + 1 < n
                && pen < INF_PENALTY
                && !next_is_penalty[k + 1]
                && !next_is_rel[k + 1]
            {
                hl.push(Node::Penalty(pen));
            }
            // e-TeX: after a right (or middle) noad, open
            r_type = Some(if is_right { T::Open } else { t });
        }
        hl
    }

    /// `check_dimensions` (§727).
    fn check_dimensions(&self, h: &List, max_h: &mut Scaled, max_d: &mut Scaled) {
        let z = self.hpack_nat(h.clone());
        *max_h = (*max_h).max(z.height);
        *max_d = (*max_d).max(z.depth);
    }
}

/// XeTeX §805: the nucleus of the first noad of a script's mlist that is
/// ordinary to `\mathpunct`, past kerns, glue and style changes (which
/// set `style`), into the branch of a `\mathchoice` that `style` picks.
fn first_nucleus<'a>(l: &'a mut [MItem], style: &mut Style) -> Option<&'a mut Field> {
    for item in l {
        match item {
            MItem::Node(Node::Kern(..) | Node::Glue(_)) => {}
            MItem::Style(s) => *style = *s,
            MItem::Choice(c) => {
                let k = usize::from(style.0 / 2).min(3);
                return first_nucleus(&mut c[k], style);
            }
            MItem::Noad(n)
                if matches!(
                    n.kind,
                    NoadKind::Ord
                        | NoadKind::Op(_)
                        | NoadKind::Bin
                        | NoadKind::Rel
                        | NoadKind::Open
                        | NoadKind::Close
                        | NoadKind::Punct
                ) =>
            {
                return Some(&mut n.nucleus);
            }
            _ => return None,
        }
    }
    None
}

/// §729: the noad at `r` becomes an Ord.
fn set_ord(out: &mut [P], r: Option<usize>) {
    if let Some(P::Noad { t, .. }) = r.and_then(|r| out.get_mut(r)) {
        *t = T::Ord;
    }
}

/// `mlist_to_hlist` (§719–§767): the hlist for `mlist` in `style`, with
/// penalties for line breaks when `penalties` (`mlist_penalties`); also
/// the diagnostics tex.web would print.
#[must_use]
pub fn mlist_to_hlist(
    mlist: MList,
    style: Style,
    penalties: bool,
    params: &MathParams,
    fonts: &dyn Fonts,
) -> (List, Vec<MathDiag>) {
    let mut ctx = Ctx {
        p: params,
        fonts,
        cur_style: style,
        cur_size: 0,
        cur_mu: 0,
        diags: Vec::new(),
        cur_f: fonts.math_cur_f(),
    };
    let h = ctx.mlist_to_hlist(mlist, style, penalties);
    if ctx.cur_f != fonts.math_cur_f() {
        fonts.set_math_cur_f(ctx.cur_f);
    }
    (h, ctx.diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_transitions_follow_section_702() {
        let d = Style::DISPLAY;
        assert_eq!(d.cramped(), Style(1));
        assert_eq!(d.sub(), Style(5));
        assert_eq!(d.sup(), Style(4));
        assert_eq!(Style(1).sup(), Style(5));
        assert_eq!(d.num(), Style(2));
        assert_eq!(d.denom(), Style(3));
        assert_eq!(Style::SCRIPT_SCRIPT.num(), Style(6));
        assert_eq!(Style::SCRIPT_SCRIPT.denom(), Style(7));
        assert_eq!(Style::TEXT.sub(), Style(5));
        assert_eq!(Style::SCRIPT.sup(), Style(6));
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6, 7].map(|s| Style(s).size()),
            [0, 0, 0, 0, 1, 1, 2, 2]
        );
    }

    #[test]
    fn math_glue_scales_by_mu() {
        // 18mu at a quad of 10pt: cur_mu = 10pt/18
        let mu = x_over_n(10 * 0x1_0000, 18).value;
        let g = Ctx::math_glue(&Glue::new(18 * 0x1_0000, 9 * 0x1_0000, 0), mu);
        assert_eq!(g.width, 655_344);
        assert_eq!(g.stretch, 327_672);
        let fil = Glue {
            stretch: 0x1_0000,
            stretch_order: Order::Fil,
            ..Glue::ZERO
        };
        assert_eq!(Ctx::math_glue(&fil, mu).stretch, 0x1_0000);
    }
}
