//! Typesetting math formulas (tex.web parts 34–36, §680–§767), copied as a
//! pure function: [`mlist_to_hlist`] turns a math list into a horizontal
//! list, as a function of the list, the style, the parameters the formula
//! read where it ended ([`Params`]), the fonts of the math families
//! ([`Fonts`]) and their `\fontdimen`s and `\skewchar`s, read through
//! [`MathEnv`] (the families' `\fontdimen`s are global values: the σ
//! parameters of family 2, the ξ parameters of family 3, §700–702).
//!
//! XeTeX's (xetex.web §742–§806): families 0–255, characters of 21 bits;
//! a family whose font is an OpenType math font (`native::is_math`) reads
//! its parameters from the font's MATH table (`native::mathsy`,
//! `native::mathex`: pure functions of the font value), its characters
//! are glyphs (`Item::Glyph`), its delimiters, radicals, big operators and
//! accents grow by the table's variants and assemblies, scripts are
//! kerned by its math kerns, accents placed by its attachment points.
//!
//! A math list is a `Vec<Item>`: noads, style changes and choices are
//! [`Item::Noad`] entries ([`MItem`]); the other items (glue, kerns,
//! penalties, rules, discretionaries, marks) are the nodes an mlist may
//! hold (§730). The few [`MItem`]s that are not entries — a script or
//! nucleus filled in, a noad replaced, `\limits`, `\over` — are what the
//! list's gatherer applies ([`gather`], tex.web's `fin_mlist`, §1184).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::arith::xn_over_d;
use crate::item::{BoxNode, Item, RUNNING};
use crate::pack::{self, Spec};
use crate::native::{self, GlyphItem};
use crate::tfm::{Font, Glyph, LigKern, Tag};
use crate::val::Glue;

/// §688: styles (`cramped` is odd).
pub const DISPLAY: u8 = 0;
pub const TEXT: u8 = 2;
pub const SCRIPT: u8 = 4;
pub const SCRIPT_SCRIPT: u8 = 6;
/// §679: `default_code`, the thickness `\over` asks for.
pub const DEFAULT_CODE: i32 = 0o10_000_000_000;
/// §149: glue subtypes in mlists (`\nonscript`, `\mskip`) and `\mkern`'s.
pub const COND_MATH_GLUE: u8 = 98;
pub const MU_GLUE: u8 = 99;
/// §157
const INF_PENALTY: i32 = 10000;
/// §699: the sizes, as offsets of the family fonts (XeTeX's 256
/// families: `script_size` is `number_math_families`).
pub const TEXT_SIZE: usize = 0;
pub const SCRIPT_SIZE: usize = 256;
pub const SCRIPT_SCRIPT_SIZE: usize = 512;
/// The family fonts' slots: 256 families in three sizes.
pub const FAM_SLOTS: usize = 768;
/// xetex.web: accent noad subtypes (`\Umathaccent fixed`, `bottom`).
pub const FIXED_ACC: u8 = 1;
pub const BOTTOM_ACC: u8 = 2;
/// `get_ot_math_accent_pos` without an attachment point.
const NO_ACCENT_POS: i32 = 0x7FFF_FFFF;
/// `min_halfword` (xetex.web's `stack_glue_into_box` sets a vertical
/// box's width to it).
const MIN_HALFWORD: i32 = -0xFFF_FFFF;
/// xetex.web §742: the MATH constants math reads.
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
/// The largest dimension (`max_dimen`).
const MAX_DIMEN: i32 = 0o7_777_777_777;

/// §764: `math_spacing`, indexed by `(r_type-ord)*8+(t-ord)`.
const MATH_SPACING: &[u8; 64] = b"0234000122*4000133**3**344*0400400*000000234000111*1111112341011";

/// §702
pub const fn cramped(c: u8) -> u8 {
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
pub fn half(x: i32) -> i32 {
    if x % 2 != 0 { (x + 1).div_euclid(2) } else { x / 2 }
}

/// §683: a delimiter field (XeTeX: 8-bit families, 21-bit characters).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u32,
    pub large_fam: u8,
    pub large_char: u32,
}

impl Delim {
    /// From a 24-bit delimiter code (§1160), or XeTeX's extended one
    /// (`"40000000 + fam*"200000 + usv`: one size).
    pub fn from_code(v: i32) -> Delim {
        if v >= 0x4000_0000 {
            return Delim {
                small_fam: ((v / 0x20_0000) % 0x100) as u8,
                small_char: (v % 0x20_0000) as u32,
                large_fam: 0,
                large_char: 0,
            };
        }
        Delim {
            small_fam: ((v / 0o4_000_000) % 16) as u8,
            small_char: ((v / 0o10_000) % 256) as u32,
            large_fam: ((v / 256) % 16) as u8,
            large_char: (v % 256) as u32,
        }
    }
}

/// §681: a noad field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

/// §682: `\limits`, `\nolimits`, `\displaylimits` (normal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Limits {
    #[default]
    Normal,
    Limits,
    NoLimits,
}

/// §682–§687: the kinds of noad.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        thickness: i32,
        left: Delim,
        right: Delim,
    },
    Under,
    Over,
    /// `sub`: XeTeX's `fixed_acc`, `bottom_acc` bits.
    Accent {
        fam: u8,
        ch: u32,
        sub: u8,
    },
    Vcenter,
    Left(Delim),
    /// `\right`; e-TeX's `\middle` (`true`: etex.web's `middle_noad`
    /// subtype).
    Right(Delim, bool),
}

impl Kind {
    /// tex.web's type code (§682–§687).
    pub fn code(&self) -> i32 {
        match self {
            Kind::Ord => ORD,
            Kind::Op(_) => OP,
            Kind::Bin => BIN,
            Kind::Rel => REL,
            Kind::Open => OPEN,
            Kind::Close => CLOSE,
            Kind::Punct => PUNCT,
            Kind::Inner => INNER,
            Kind::Radical(_) => 24,
            Kind::Fraction { .. } => FRACTION,
            Kind::Under => 26,
            Kind::Over => 27,
            Kind::Accent { .. } => 28,
            Kind::Vcenter => 29,
            Kind::Left(_) => LEFT,
            Kind::Right(..) => RIGHT,
        }
    }

    /// `\mathord` … `\mathinner`, `\underline`, `\overline` by the
    /// command's code (tex.web 1156: `ord_noad` + 0 … 7, `under_noad`,
    /// `over_noad`).
    pub fn comp(code: u8) -> Kind {
        match code {
            0 => Kind::Ord,
            1 => Kind::Op(Limits::Normal),
            2 => Kind::Bin,
            3 => Kind::Rel,
            4 => Kind::Open,
            5 => Kind::Close,
            6 => Kind::Punct,
            7 => Kind::Inner,
            8 => Kind::Under,
            _ => Kind::Over,
        }
    }

    /// §1151: the class a math character's code gives (`c div "1000`).
    pub fn class(c: i32) -> Kind {
        Kind::comp(((c / 0o10_000) % 8) as u8)
    }

    /// `scripts_allowed` (§1176).
    pub fn scripts_allowed(&self) -> bool {
        (ORD..LEFT).contains(&self.code())
    }
}

const ORD: i32 = 16;
const OP: i32 = 17;
const BIN: i32 = 18;
const REL: i32 = 19;
const OPEN: i32 = 20;
const CLOSE: i32 = 21;
const PUNCT: i32 = 22;
const INNER: i32 = 23;
const FRACTION: i32 = 25;
const LEFT: i32 = 30;
const RIGHT: i32 = 31;
const STYLE_NODE: i32 = 14;
const PENALTY_NODE: i32 = 12;

/// A noad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noad {
    pub kind: Kind,
    pub nucleus: Field,
    pub sup: Field,
    pub sub: Field,
}

impl Noad {
    pub fn new(kind: Kind) -> Noad {
        Noad { kind, nucleus: Field::Empty, sup: Field::Empty, sub: Field::Empty }
    }
}

/// Which field of a noad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Nucleus,
    Sup,
    Sub,
}

/// An entry of a math list, or what its gatherer applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MItem {
    Noad(Noad),
    /// §688: a style change.
    Style(u8),
    /// §689: `\mathchoice`: display, text, script, scriptscript.
    Choice(Box<[Vec<Item>; 4]>),
    // ---- applied by the gatherer to the list's last noad
    /// A field filled in (a subformula's group ended; a character).
    Set(Which, Field),
    /// §1186: a subformula that is one accent noad replaces the ord noad
    /// whose nucleus it was.
    Replace(Noad),
    /// §1159: `\limits` and friends after an operator.
    Limits(Limits),
    /// §1181: `\over` and friends: the list so far is the numerator.
    Above {
        thickness: i32,
        left: Delim,
        right: Delim,
    },
}

/// An mlist entry as an item.
pub fn item(m: MItem) -> Item {
    Item::Noad(Arc::new(m))
}

/// The list a math list's nodes have built so far (after its last
/// `\over`): what `sub_sup` and `\limits` look at the end of.
pub fn current(items: impl IntoIterator<Item = Item>) -> Vec<Item> {
    let mut since = Vec::new();
    for it in items {
        if let Item::Noad(m) = &it
            && let MItem::Above { .. } = &**m
        {
            since.clear();
            continue;
        }
        since.push(it);
    }
    gather(since, None).0
}

/// What an mlist's gatherer reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatherError {
    /// §1183: a second `\over` in one list.
    Ambiguous,
}

/// §1184–§1185 `fin_mlist`: the math list from the items its nodes
/// emitted (fields filled in, `\limits` applied, the `\over` split made),
/// with `right` (a `\right`'s noad) at its end.
pub fn gather(items: impl IntoIterator<Item = Item>, right: Option<Noad>) -> (Vec<Item>, Vec<GatherError>) {
    let mut list: Vec<Item> = Vec::new();
    let mut errors = Vec::new();
    let mut incompleat: Option<(i32, Delim, Delim, Vec<Item>)> = None;
    for it in items {
        let m = match &it {
            Item::Noad(m) => m,
            Item::Remove(k) => {
                crate::page::remove_last(&mut list, *k);
                continue;
            }
            _ => {
                list.push(it);
                continue;
            }
        };
        match &**m {
            MItem::Noad(_) | MItem::Style(_) | MItem::Choice(_) => list.push(it),
            MItem::Set(w, f) => {
                if let Some(Item::Noad(last)) = list.last_mut()
                    && let MItem::Noad(n) = Arc::make_mut(last)
                {
                    let slot = match w {
                        Which::Nucleus => &mut n.nucleus,
                        Which::Sup => &mut n.sup,
                        Which::Sub => &mut n.sub,
                    };
                    *slot = f.clone();
                }
            }
            MItem::Replace(n) => {
                if let Some(last) = list.last_mut() {
                    *last = item(MItem::Noad(n.clone()));
                }
            }
            MItem::Limits(l) => {
                if let Some(Item::Noad(last)) = list.last_mut()
                    && let MItem::Noad(n) = Arc::make_mut(last)
                    && let Kind::Op(x) = &mut n.kind
                {
                    *x = *l;
                }
            }
            MItem::Above { thickness, left, right } => {
                if incompleat.is_some() {
                    errors.push(GatherError::Ambiguous);
                } else {
                    incompleat = Some((*thickness, *left, *right, std::mem::take(&mut list)));
                }
            }
        }
    }
    let Some((thickness, left, right_d, mut num)) = incompleat else {
        list.extend(right.map(|r| item(MItem::Noad(r))));
        return (list, errors);
    };
    // (§1185: the denominator is the list since the `\over`; with a
    // `\right`, the `\left` noad stays first and the right noad last)
    let mut frac = Noad::new(Kind::Fraction { thickness, left, right: right_d });
    frac.sub = Field::Mlist(list);
    match right {
        None => {
            frac.sup = Field::Mlist(num);
            (vec![item(MItem::Noad(frac))], errors)
        }
        Some(r) => {
            // (etex.web §1185: the numerator begins after the last left or
            // middle noad, `delim_ptr`)
            let k = num
                .iter()
                .rposition(|i| matches!(i, Item::Noad(m) if matches!(&**m, MItem::Noad(n) if matches!(n.kind, Kind::Left(_) | Kind::Right(_, true)))))
                .map_or(usize::from(!num.is_empty()), |k| k + 1);
            let rest = num.split_off(k.min(num.len()));
            frac.sup = Field::Mlist(rest);
            let mut out: Vec<Item> = num;
            out.push(item(MItem::Noad(frac)));
            out.push(item(MItem::Noad(r)));
            (out, errors)
        }
    }
}

/// What a formula reads besides its fonts, where it ends (inside its
/// group).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Params {
    pub script_space: i32,
    pub null_delimiter_space: i32,
    pub delimiter_factor: i32,
    pub delimiter_shortfall: i32,
    pub bin_op_penalty: i32,
    pub rel_penalty: i32,
    pub thin_mu_skip: Arc<Glue>,
    pub med_mu_skip: Arc<Glue>,
    pub thick_mu_skip: Arc<Glue>,
}

/// `\textfont`, `\scriptfont`, `\scriptscriptfont` of the families, at
/// `fam + size` (`size` 0, 256, 512; [`FAM_SLOTS`] long); `None`:
/// `\nullfont` (or a family not read: none the formula uses).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fonts(pub Vec<Option<Arc<Font>>>);

impl Fonts {
    fn get(&self, z: usize) -> Option<&Arc<Font>> {
        self.0.get(z).and_then(Option::as_ref)
    }
}

/// The values a formula reads lazily: a font's `\fontdimen`s as possibly
/// changed, its `\skewchar`.
pub trait MathEnv {
    fn param(&mut self, f: &Arc<Font>, k: u16) -> i32;
    fn skew_char(&mut self, f: &Arc<Font>) -> i32;
    /// XeTeX: `get_native_glyph(new_native_character(f, c), 0)`, the
    /// glyph of character `c` set in native font `f` (with its mapping;
    /// a character the font lacks is warned of).
    fn native_char_glyph(&mut self, f: &Arc<Font>, c: u32) -> u16 {
        native::native(f).map_or(0, |nf| nf.xf.map_char_to_glyph(c as i32) as u16)
    }
}

/// What the caller reports, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// §723: "\textfont 3 is undefined (character x)"; `size` 0, 256,
    /// 512.
    UndefinedFamily { size: usize, fam: u8, ch: u32 },
    /// §581: the character is not in the font.
    MissingChar { font: Arc<Font>, ch: u32 },
}

fn kern(w: i32) -> Item {
    Item::Kern { w, sub: 0 }
}

/// §704: `fraction_rule`.
fn rule(t: i32) -> Item {
    Item::Rule { w: RUNNING, h: t, d: 0 }
}

fn boxed(b: BoxNode) -> Item {
    Item::Box(Arc::new(b))
}

fn unbox(b: Arc<BoxNode>) -> BoxNode {
    Arc::try_unwrap(b).unwrap_or_else(|b| (*b).clone())
}

/// §649: `hpack(list, natural)` (math never reports a box).
pub fn hpack_natural(list: Vec<Item>) -> BoxNode {
    pack::hpack(list, Spec::NATURAL, &pack::Params::QUIET, None).b
}

/// §668: `vpack(list, natural)`.
fn vpack_natural(list: Vec<Item>) -> BoxNode {
    pack::vpack(list, Spec::NATURAL, MAX_DIMEN, &pack::Params::QUIET).b
}

/// The height and depth `hpack` would give a list.
fn natural_hd(list: &[Item]) -> (i32, i32) {
    let (mut h, mut d) = (0, 0);
    for it in list {
        let (ht, dp, s) = match it {
            Item::Char(f, c) => match f.glyph(i32::from(*c)) {
                Some(g) => (g.height, g.depth, 0),
                None => continue,
            },
            Item::Lig(l) => match l.font.glyph(i32::from(l.c)) {
                Some(g) => (g.height, g.depth, 0),
                None => continue,
            },
            Item::Box(b) => (b.h, b.d, b.shift),
            Item::Rule { h, d, .. } => (*h, *d, 0),
            Item::Glyph(g) => (g.h, g.d, 0),
            Item::Native(w) => (w.h, w.d, 0),
            _ => continue,
        };
        h = h.max(ht - s);
        d = d.max(dp + s);
    }
    (h, d)
}

/// §716, §717: `m` split into its integer and fraction parts.
fn mu_parts(m: i32) -> (i32, i32) {
    let (mut n, mut f) = (m / 0o200_000, m % 0o200_000);
    if f < 0 {
        n -= 1;
        f += 0o200_000;
    }
    (n, f)
}

/// §716: `mu_mult(x)`.
fn mu_mult(n: i32, f: i32, x: i32) -> i32 {
    let y = xn_over_d(x, f, 0o200_000).0;
    nx_plus_y(n, x, y)
}

/// §105: `n*x+y`, 0 on overflow (where TeX sets `arith_error`).
fn nx_plus_y(mut n: i32, mut x: i32, y: i32) -> i32 {
    if n < 0 {
        x = -x;
        n = -n;
    }
    if n == 0 {
        y
    } else if x <= (MAX_DIMEN - y) / n && -x <= (MAX_DIMEN + y) / n {
        n * x + y
    } else {
        0
    }
}

/// §716: `math_glue(g, m)`.
pub fn math_glue(g: &Glue, m: i32) -> Glue {
    let (n, f) = mu_parts(m);
    Glue {
        width: mu_mult(n, f, g.width),
        stretch: if g.stretch_order == 0 { mu_mult(n, f, g.stretch) } else { g.stretch },
        shrink: if g.shrink_order == 0 { mu_mult(n, f, g.shrink) } else { g.shrink },
        stretch_order: g.stretch_order,
        shrink_order: g.shrink_order,
    }
}

/// §557: `char_kern`.
fn char_kern(font: &Font, ci: LigKern) -> i32 {
    let i = 256 * usize::from(ci.op - LigKern::KERN) + usize::from(ci.remainder);
    font.kerns.get(i).copied().unwrap_or(0)
}

/// A first-pass result.
enum Done {
    Noad { code: i32, hlist: Vec<Item>, delim: Delim },
    Style(u8),
    Node(Item),
}

struct Ctx<'a, E: MathEnv> {
    env: &'a mut E,
    fonts: &'a Fonts,
    params: &'a Params,
    events: Vec<Event>,
    style: u8,
    size: usize,
    mu: i32,
    /// §724: `cur_f`, the font of the last character fetched (XeTeX reads
    /// it after the fact). A formula begins with none (CORE.md: XeTeX's
    /// carries over from the formula before).
    cur_f: Option<Arc<Font>>,
}

/// §722: what `fetch` found.
enum Fetched {
    /// An undefined family or a missing character (the field emptied).
    None,
    Tfm(Arc<Font>, Glyph),
    /// A native font's character (`cur_i` null).
    Native(Arc<Font>),
}

/// A glyph of an OpenType math font with its glyph metrics
/// (`set_native_glyph_metrics(p, 1)`).
fn glyph_item(f: &Arc<Font>, g: u16) -> GlyphItem {
    native::glyph(f, g, true)
}

/// XeTeX's `is_glyph_node` of a list's first item.
fn glyph_of(n: Option<&Item>) -> Option<Arc<GlyphItem>> {
    match n {
        Some(Item::Glyph(g)) => Some(g.clone()),
        _ => None,
    }
}

/// xetex.web §749 `stack_glyph_into_box`: glyph `g` after the others in
/// an hlist (the first one's height and depth not counted), under them in
/// a vlist.
fn stack_glyph_into_box(b: &mut BoxNode, g: GlyphItem) {
    if b.vertical {
        b.h = g.h;
        b.w = b.w.max(g.w);
        b.list.insert(0, Item::Glyph(Arc::new(g)));
    } else {
        if !b.list.is_empty() {
            b.h = b.h.max(g.h);
            b.d = b.d.max(g.d);
        }
        b.list.push(Item::Glyph(Arc::new(g)));
    }
}

/// xetex.web §749 `stack_glue_into_box`: glue of width `min` stretching
/// to `max`. In a vlist it sets the box's width to the glue node's
/// `width` word, which holds `leader_ptr`: `null`.
fn stack_glue_into_box(b: &mut BoxNode, min: i32, max: i32) {
    let g = Item::Glue { g: Arc::new(Glue { width: min, stretch: max - min, ..Glue::default() }), sub: 0, zero: false };
    if b.vertical {
        b.list.insert(0, g);
        b.w = MIN_HALFWORD;
    } else {
        b.list.push(g);
    }
}

/// §726: `mlist` in `style` as an hlist; `penalties`: penalties after
/// binary operators and relations (an inline formula in a paragraph).
pub fn mlist_to_hlist(
    mlist: Vec<Item>,
    style: u8,
    penalties: bool,
    fonts: &Fonts,
    params: &Params,
    env: &mut impl MathEnv,
) -> (Vec<Item>, Vec<Event>) {
    let mut cx = Ctx { env, fonts, params, events: Vec::new(), style, size: 0, mu: 0, cur_f: None };
    let list = cx.mlist_to_hlist(mlist, style, penalties);
    (list, cx.events)
}

impl<E: MathEnv> Ctx<'_, E> {
    fn fam(&self, z: usize) -> Option<Arc<Font>> {
        self.fonts.get(z).cloned()
    }

    /// `mathsy(k)` of family 2 in `size` (xetex.web §742: an OpenType
    /// math font's from its MATH table).
    fn mathsy(&mut self, k: u16, size: usize) -> i32 {
        match self.fam(2 + size) {
            Some(f) if native::is_math(&f) => native::mathsy(&f, i32::from(k)),
            Some(f) => self.env.param(&f, k),
            None => 0,
        }
    }
    fn math_x_height(&mut self, s: usize) -> i32 {
        self.mathsy(5, s)
    }
    fn axis_height(&mut self, s: usize) -> i32 {
        self.mathsy(22, s)
    }
    /// `mathex(k)` of family 3 in the current size.
    fn mathex(&mut self, k: u16) -> i32 {
        match self.fam(3 + self.size) {
            Some(f) if native::is_math(&f) => native::mathex(&f, i32::from(k)),
            Some(f) => self.env.param(&f, k),
            None => 0,
        }
    }

    /// `is_new_mathfont(cur_f)`.
    fn cur_mathfont(&self) -> bool {
        self.cur_f.as_ref().is_some_and(|f| native::is_math(f))
    }

    /// `get_ot_math_constant(cur_f, k)`.
    fn cur_constant(&self, k: u32) -> i32 {
        self.cur_f.as_ref().and_then(|f| native::native(f)).map_or(0, |nf| nf.xf.ot_math_constant(k))
    }
    fn default_rule_thickness(&mut self) -> i32 {
        self.mathex(8)
    }

    /// §703
    fn set_size_and_mu(&mut self) {
        self.size = if self.style < SCRIPT { TEXT_SIZE } else { SCRIPT_SIZE * usize::from((self.style - TEXT) / 2) };
        self.mu = self.mathsy(6, self.size) / 18;
    }

    /// §705
    fn overbar(b: BoxNode, k: i32, t: i32) -> BoxNode {
        vpack_natural(vec![kern(t), rule(t), kern(k), boxed(b)])
    }

    /// §709: a box holding character `c` of `f`, its width with the
    /// italic correction.
    fn char_box(f: &Arc<Font>, c: u8) -> BoxNode {
        let g = f.glyph(i32::from(c)).unwrap_or_default();
        BoxNode {
            w: g.width + g.italic,
            h: g.height,
            d: g.depth,
            list: vec![Item::Char(f.clone(), c)],
            ..BoxNode::default()
        }
    }

    fn height_plus_depth(f: &Font, c: u8) -> i32 {
        let g = f.glyph(i32::from(c)).unwrap_or_default();
        g.height + g.depth
    }

    /// §706 (xetex.web §749): a delimiter of size at least `v` for `d` in
    /// size `s`. In a native font the character's vertical variants are
    /// looked at, then its assembly.
    fn var_delimiter(&mut self, d: Delim, s: usize, v: i32) -> BoxNode {
        let mut f: Option<Arc<Font>> = None;
        let mut c: u32 = 0;
        let mut ext: Option<u8> = None;
        let mut assembly: Option<Vec<partex_otf::math::GlyphPart>> = None;
        let mut w = 0;
        'found: for (z0, x0) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // (§707; XeTeX's character is a quarterword)
            let mut x = x0 & 0xFFFF;
            if z0 != 0 || x != 0 {
                let mut z = usize::from(z0) + s + SCRIPT_SIZE;
                loop {
                    z -= SCRIPT_SIZE;
                    if let Some(g) = self.fam(z) {
                        if let Some(nf) = native::native(&g) {
                            // (xetex.web §751)
                            x = nf.xf.map_char_to_glyph(x as i32) as u32 & 0xFFFF;
                            f = Some(g.clone());
                            c = x;
                            w = 0;
                            let mut n = 0;
                            loop {
                                let (y, u) = nf.xf.ot_math_variant(x, n, false);
                                if u > w {
                                    c = y;
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
                            // (no glyph big enough: is it extensible?)
                            assembly = nf.xf.ot_assembly(x, false);
                            if assembly.is_some() {
                                break 'found;
                            }
                        } else {
                            // (§708)
                            let mut y = x;
                            while let Some(q) = g.glyph(y as i32) {
                                if let Tag::Ext(r) = q.tag {
                                    f = Some(g.clone());
                                    c = y;
                                    ext = Some(r);
                                    break 'found;
                                }
                                let u = q.height + q.depth;
                                if u > w {
                                    f = Some(g.clone());
                                    c = y;
                                    w = u;
                                    if u >= v {
                                        break 'found;
                                    }
                                }
                                let Tag::List(next) = q.tag else { break };
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
            None => BoxNode { w: self.params.null_delimiter_space, ..BoxNode::default() },
            Some(f) if f.native.is_none() => match ext {
                Some(r) => Self::extensible(&f, r, v),
                None => Self::char_box(&f, c as u8),
            },
            Some(f) => match assembly {
                Some(a) => Self::opentype_assembly(&f, &a, v, false),
                None => {
                    let g = glyph_item(&f, c as u16);
                    BoxNode { vertical: true, w: g.w, h: g.h, d: g.d, list: vec![Item::Glyph(Arc::new(g))], ..BoxNode::default() }
                }
            },
        };
        b.shift = half(b.h - b.d) - self.axis_height(s);
        b
    }

    /// xetex.web §749 `build_opentype_assembly`: a box at least `s` high
    /// (wide if `horiz`) of the parts `a` of font `f`, as few copies of the
    /// extenders as do, their overlaps glue that stretches.
    fn opentype_assembly(f: &Arc<Font>, a: &[partex_otf::math::GlyphPart], s: i32, horiz: bool) -> BoxNode {
        let mut b = BoxNode { vertical: !horiz, ..BoxNode::default() };
        let min_o = native::native(f).map_or(0, |nf| nf.xf.ot_min_connector_overlap());
        let mut n: u32 = 0;
        loop {
            let mut no_extenders = true;
            let mut s_max: i32 = 0;
            let mut prev_o = 0;
            for p in a {
                let reps = if p.extender {
                    no_extenders = false;
                    n
                } else {
                    1
                };
                for _ in 0..reps {
                    let o = p.start_connector_length.min(min_o).min(prev_o);
                    s_max = s_max.wrapping_sub(o).wrapping_add(p.full_advance);
                    prev_o = p.end_connector_length;
                }
            }
            // (XeTeX loops for ever on extenders that add nothing)
            if s_max >= s || no_extenders || n >= 1 << 16 {
                break;
            }
            n += 1;
        }
        let mut prev_o = 0;
        for p in a {
            for _ in 0..if p.extender { n } else { 1 } {
                let mut o = p.start_connector_length.min(prev_o);
                let oo = o;
                o = o.min(min_o);
                if oo > 0 {
                    stack_glue_into_box(&mut b, -oo, -o);
                }
                stack_glyph_into_box(&mut b, glyph_item(f, p.glyph as u16));
                prev_o = p.end_connector_length;
            }
        }
        let (mut nat, mut stretch) = (0, 0);
        for p in &b.list {
            match p {
                Item::Glyph(g) => nat += if horiz { g.w } else { g.h + g.d },
                Item::Glue { g, .. } => {
                    nat += g.width;
                    stretch += g.stretch;
                }
                _ => {}
            }
        }
        let size = if s > nat && stretch > 0 {
            let o = (s - nat).min(stretch);
            b.glue.order = 0;
            b.glue.sign = 1;
            b.glue.ratio = f64::from(o) / f64::from(stretch);
            nat + crate::show::zround(f64::from(stretch) * b.glue.ratio)
        } else {
            nat
        };
        if horiz {
            b.w = size;
        } else {
            b.h = size;
        }
        b
    }

    /// §713–§715: an extensible character of `f` with recipe `r`.
    fn extensible(f: &Arc<Font>, r: u8, v: i32) -> BoxNode {
        let e = f.extens.get(usize::from(r)).copied().unwrap_or_default();
        let mut b = BoxNode { vertical: true, ..BoxNode::default() };
        let u = Self::height_plus_depth(f, e.rep);
        let g = f.glyph(i32::from(e.rep)).unwrap_or_default();
        b.w = g.width + g.italic;
        let mut w = 0;
        for c in [e.bot, e.mid, e.top] {
            if c != 0 {
                w += Self::height_plus_depth(f, c);
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
        // (§715: stacked from the bottom up)
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
            b.list.push(boxed(Self::char_box(f, c)));
        }
        if let Some(&c) = stack.last() {
            b.h = Self::char_box(f, c).h;
        }
        b.d = w - b.h;
        b
    }

    /// §715: `rebox(b, w)`: `b` `w` wide, its contents centered.
    fn rebox(mut b: BoxNode, w: i32) -> BoxNode {
        if b.w != w && !b.list.is_empty() {
            if b.vertical {
                b = hpack_natural(vec![boxed(b)]);
            }
            let width = b.w;
            let mut list = b.list;
            if let [Item::Char(f, c)] = list.as_slice() {
                let v = f.glyph(i32::from(*c)).unwrap_or_default().width;
                if v != width {
                    list.push(kern(width - v));
                }
            }
            let ss = Item::Glue {
                g: Arc::new(Glue { width: 0, stretch: 65536, shrink: 65536, stretch_order: 1, shrink_order: 1 }),
                sub: 0,
                zero: false,
            };
            list.insert(0, ss.clone());
            list.push(ss);
            pack::hpack(list, Spec::Exactly(w), &pack::Params::QUIET, None).b
        } else {
            b.w = w;
            b
        }
    }

    /// §720: `clean_box`: `field` in style `s` as a box.
    fn clean_box(&mut self, field: Field, s: u8) -> BoxNode {
        let q = match field {
            Field::Char { .. } | Field::TextChar { .. } => {
                let mut n = Noad::new(Kind::Ord);
                n.nucleus = field;
                Some(self.sub_mlist(vec![item(MItem::Noad(n))], s))
            }
            Field::Box(b) => {
                if b.shift == 0 {
                    return Self::simplify(unbox(b));
                }
                Some(vec![Item::Box(b)])
            }
            Field::Mlist(l) => Some(self.sub_mlist(l, s)),
            Field::Empty => None,
        };
        let x = match q {
            None => BoxNode::default(),
            Some(mut q) => {
                if let [Item::Box(b)] = q.as_slice()
                    && b.shift == 0
                {
                    let Some(Item::Box(b)) = q.pop() else { unreachable!() };
                    unbox(b)
                } else {
                    hpack_natural(q)
                }
            }
        };
        Self::simplify(x)
    }

    /// §721: an unneeded italic correction goes.
    fn simplify(mut x: BoxNode) -> BoxNode {
        if let [Item::Char(..), Item::Kern { .. }] = x.list.as_slice() {
            x.list.pop();
        }
        x
    }

    /// A recursive call in style `s`, without penalties; the style is
    /// restored.
    fn sub_mlist(&mut self, l: Vec<Item>, s: u8) -> Vec<Item> {
        let save = self.style;
        let r = self.mlist_to_hlist(l, s, false);
        self.style = save;
        self.set_size_and_mu();
        r
    }

    /// §722 (xetex.web §765): `fetch` of math character `(fam, ch)`,
    /// setting `cur_f`; an event if it is not there.
    fn fetch(&mut self, fam: u8, ch: u32) -> Fetched {
        let f = self.fam(usize::from(fam) + self.size);
        self.cur_f.clone_from(&f);
        let Some(f) = f else {
            self.events.push(Event::UndefinedFamily { size: self.size, fam, ch });
            return Fetched::None;
        };
        if f.native.is_some() {
            return Fetched::Native(f);
        }
        match f.glyph(ch as i32).filter(|_| ch <= 255) {
            Some(g) => Fetched::Tfm(f, g),
            None => {
                self.events.push(Event::MissingChar { font: f, ch });
                Fetched::None
            }
        }
    }

    /// `fetch` of a character field, emptied if the character is not
    /// there.
    fn fetch_field(&mut self, field: &mut Field) -> Fetched {
        let (Field::Char { fam, ch } | Field::TextChar { fam, ch }) = *field else { return Fetched::None };
        let r = self.fetch(fam, ch);
        if let Fetched::None = r {
            *field = Field::Empty;
        }
        r
    }

    /// `fetch` of a TFM character field: its font, character and metrics.
    fn fetch_tfm(&mut self, field: &mut Field) -> Option<(Arc<Font>, u8, Glyph)> {
        let (Field::Char { ch, .. } | Field::TextChar { ch, .. }) = *field else { return None };
        match self.fetch_field(field) {
            Fetched::Tfm(f, g) => Some((f, ch as u8, g)),
            _ => None,
        }
    }

    fn mlist_to_hlist(&mut self, mlist: Vec<Item>, style: u8, penalties: bool) -> Vec<Item> {
        self.style = style;
        let mut todo: VecDeque<Item> = mlist.into();
        let mut done: Vec<Done> = Vec::with_capacity(todo.len());
        let mut r: Option<usize> = None;
        let mut r_type = OP;
        let (mut max_h, mut max_d) = (0, 0);
        self.set_size_and_mu();
        while let Some(it) = todo.pop_front() {
            // (§727)
            let m = match it {
                Item::Noad(m) => m,
                other => {
                    let n = self.first_pass_node(other, &mut todo, &mut max_h, &mut max_d);
                    done.push(Done::Node(n));
                    continue;
                }
            };
            let mut q = match Arc::try_unwrap(m).unwrap_or_else(|m| (*m).clone()) {
                MItem::Noad(q) => q,
                MItem::Style(s) => {
                    self.style = s;
                    self.set_size_and_mu();
                    done.push(Done::Style(s));
                    continue;
                }
                MItem::Choice(c) => {
                    // (§731)
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
                _ => continue,
            };
            // (§728)
            let mut delta = 0;
            if q.kind == Kind::Bin && matches!(r_type, BIN | OP | REL | OPEN | PUNCT | LEFT) {
                q.kind = Kind::Ord;
            }
            match &q.kind {
                Kind::Bin | Kind::Open | Kind::Inner => {}
                Kind::Rel | Kind::Close | Kind::Punct | Kind::Right(..) => {
                    // (§729)
                    if r_type == BIN
                        && let Some(Done::Noad { code, .. }) = r.map(|i| &mut done[i])
                    {
                        *code = ORD;
                    }
                    if let Kind::Right(d, _) = q.kind {
                        done.push(Done::Noad { code: RIGHT, hlist: Vec::new(), delim: d });
                        r = Some(done.len() - 1);
                        // (etex.web §727: after a right noad — a `\middle` —
                        // the list goes on as after a left one, in the
                        // list's style)
                        r_type = LEFT;
                        self.style = style;
                        self.set_size_and_mu();
                        continue;
                    }
                }
                Kind::Left(d) => {
                    done.push(Done::Noad { code: LEFT, hlist: Vec::new(), delim: *d });
                    r = Some(done.len() - 1);
                    r_type = LEFT;
                    continue;
                }
                Kind::Fraction { .. } => {
                    let h = self.make_fraction(&mut q);
                    push_noad(&mut done, &mut r, &mut r_type, FRACTION, h, &mut max_h, &mut max_d);
                    continue;
                }
                Kind::Op(_) => {
                    let (d, limits) = self.make_op(&mut q);
                    delta = d;
                    if let Some(v) = limits {
                        push_noad(&mut done, &mut r, &mut r_type, OP, vec![boxed(v)], &mut max_h, &mut max_d);
                        continue;
                    }
                }
                Kind::Ord => self.make_ord(&mut q, &mut todo),
                Kind::Radical(d) => {
                    let d = *d;
                    self.make_radical(&mut q, d);
                }
                Kind::Over => self.make_over(&mut q),
                Kind::Under => self.make_under(&mut q),
                Kind::Accent { fam, ch, sub } => {
                    let (fam, ch, sub) = (*fam, *ch, *sub);
                    self.make_math_accent(&mut q, fam, ch, sub);
                }
                Kind::Vcenter => self.make_vcenter(&mut q),
            }
            // (§754: the nucleus as an hlist, with the scripts attached)
            let text = matches!(q.nucleus, Field::TextChar { .. });
            let mut p = match std::mem::take(&mut q.nucleus) {
                mut n @ (Field::Char { .. } | Field::TextChar { .. }) => {
                    // (§755, xetex.web §799)
                    let (Field::Char { ch, .. } | Field::TextChar { ch, .. }) = n else { unreachable!() };
                    match self.fetch_field(&mut n) {
                        Fetched::Native(f) => {
                            let g = self.env.native_char_glyph(&f, ch);
                            let g = glyph_item(&f, g);
                            delta = native::native(&f).map_or(0, |nf| nf.xf.ot_math_ital_corr(u32::from(g.gid)));
                            if text && !native::is_math(&f) {
                                delta = 0;
                            }
                            let mut p = vec![Item::Glyph(Arc::new(g))];
                            if q.sub == Field::Empty && delta != 0 {
                                p.push(kern(delta));
                                delta = 0;
                            }
                            p
                        }
                        Fetched::Tfm(f, g) => {
                            delta = g.italic;
                            let mut p = vec![Item::Char(f.clone(), ch as u8)];
                            if text && self.env.param(&f, 2) != 0 {
                                delta = 0;
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
                Field::Box(b) => vec![Item::Box(b)],
                Field::Mlist(l) => {
                    let s = self.style;
                    let h = self.sub_mlist(l, s);
                    vec![boxed(hpack_natural(h))]
                }
            };
            if q.sub != Field::Empty || q.sup != Field::Empty {
                self.make_scripts(&mut q, &mut p, delta);
            }
            let code = q.kind.code();
            push_noad(&mut done, &mut r, &mut r_type, code, p, &mut max_h, &mut max_d);
        }
        // (§729: a final bin noad is an ord)
        if r_type == BIN
            && let Some(Done::Noad { code, .. }) = r.map(|i| &mut done[i])
        {
            *code = ORD;
        }
        self.second_pass(done, style, penalties, max_h, max_d)
    }

    /// §730: the nodes of an mlist in the first pass.
    fn first_pass_node(&mut self, n: Item, todo: &mut VecDeque<Item>, max_h: &mut i32, max_d: &mut i32) -> Item {
        match n {
            Item::Rule { h, d, .. } => {
                *max_h = (*max_h).max(h);
                *max_d = (*max_d).max(d);
                n
            }
            Item::Glue { g, sub, zero } => {
                // (§732)
                if sub == MU_GLUE {
                    Item::Glue { g: Arc::new(math_glue(&g, self.mu)), sub: 0, zero: false }
                } else {
                    if self.size != TEXT_SIZE
                        && sub == COND_MATH_GLUE
                        && matches!(todo.front(), Some(Item::Glue { .. } | Item::Kern { .. }))
                    {
                        todo.pop_front();
                    }
                    Item::Glue { g, sub, zero }
                }
            }
            Item::Kern { w, sub } if sub == MU_GLUE => {
                // (§717)
                let (n, f) = mu_parts(self.mu);
                Item::Kern { w: mu_mult(n, f, w), sub: 1 }
            }
            other => other,
        }
    }

    /// §734
    fn make_over(&mut self, q: &mut Noad) {
        let b = self.clean_box(std::mem::take(&mut q.nucleus), cramped(self.style));
        let t = self.default_rule_thickness();
        q.nucleus = Field::Box(Arc::new(Self::overbar(b, 3 * t, t)));
    }

    /// §735
    fn make_under(&mut self, q: &mut Noad) {
        let x = self.clean_box(std::mem::take(&mut q.nucleus), self.style);
        let t = self.default_rule_thickness();
        let xh = x.h;
        let mut y = vpack_natural(vec![boxed(x), kern(3 * t), rule(t)]);
        let delta = y.h + y.d + t;
        y.h = xh;
        y.d = delta - y.h;
        q.nucleus = Field::Box(Arc::new(y));
    }

    /// §736
    fn make_vcenter(&mut self, q: &mut Noad) {
        let Field::Box(v) = &mut q.nucleus else { return };
        let size = self.size;
        let a = self.axis_height(size);
        let v = Arc::make_mut(v);
        let delta = v.h + v.d;
        v.h = a + half(delta);
        v.d = delta - v.h;
    }

    /// §737 (xetex.web §780: an OpenType math font's rule and gaps).
    fn make_radical(&mut self, q: &mut Noad, d: Delim) {
        let f = self.fam(usize::from(d.small_fam) + self.size);
        let ot = f.as_ref().is_some_and(|f| native::is_math(f));
        let constant = |k: u32| f.as_ref().and_then(|f| native::native(f)).map_or(0, |nf| nf.xf.ot_math_constant(k));
        let t = if ot { constant(RADICAL_RULE_THICKNESS) } else { self.default_rule_thickness() };
        let x = self.clean_box(std::mem::take(&mut q.nucleus), cramped(self.style));
        let mut clr = if ot {
            if self.style < TEXT { constant(RADICAL_DISPLAY_STYLE_VERTICAL_GAP) } else { constant(RADICAL_VERTICAL_GAP) }
        } else if self.style < TEXT {
            let size = self.size;
            t + self.math_x_height(size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let size = self.size;
        let mut y = self.var_delimiter(d, size, x.h + x.d + clr + t);
        if ot {
            y.d = y.h + y.d - t;
            y.h = t;
        }
        let delta = y.d - (x.h + x.d + clr);
        if delta > 0 {
            clr += half(delta);
        }
        y.shift = -(x.h + clr);
        let yh = y.h;
        let o = Self::overbar(x, clr, yh);
        q.nucleus = Field::Box(Arc::new(hpack_natural(vec![boxed(y), boxed(o)])));
    }

    /// xetex.web §781 `compute_ot_math_accent_pos`: the top accent
    /// attachment of the nucleus's character (or of the nucleus of an
    /// accent that is its subformula's first noad).
    fn compute_ot_math_accent_pos(&mut self, nucleus: &mut Field) -> i32 {
        match nucleus {
            Field::Char { ch, .. } => {
                let ch = *ch;
                self.fetch_field(nucleus);
                match self.cur_f.clone() {
                    Some(f) if f.native.is_some() => {
                        let g = self.env.native_char_glyph(&f, ch);
                        native::native(&f).map_or(NO_ACCENT_POS, |nf| nf.xf.ot_math_accent_pos(u32::from(g)))
                    }
                    _ => NO_ACCENT_POS,
                }
            }
            Field::Mlist(l) => match l.first_mut() {
                Some(Item::Noad(m)) => {
                    let m = Arc::make_mut(m);
                    match m {
                        MItem::Noad(r) if matches!(r.kind, Kind::Accent { .. }) => {
                            self.compute_ot_math_accent_pos(&mut r.nucleus)
                        }
                        _ => NO_ACCENT_POS,
                    }
                }
                _ => NO_ACCENT_POS,
            },
            _ => NO_ACCENT_POS,
        }
    }

    /// §741: the skew of a character nucleus: its kern with the font's
    /// `\skewchar`.
    fn skew(&mut self, nucleus: &mut Field) -> i32 {
        let mut s = 0;
        if let Field::Char { .. } = nucleus
            && let Some((cf, _, g)) = self.fetch_tfm(nucleus)
            && let Tag::Lig(start) = g.tag
        {
            let skew = self.env.skew_char(&cf);
            let mut k = usize::from(start);
            let first = cf.lig_kerns[k];
            if first.skip > LigKern::STOP {
                k = 256 * usize::from(first.op) + usize::from(first.remainder);
            }
            loop {
                let ci = cf.lig_kerns[k];
                if i32::from(ci.next) == skew {
                    if ci.op >= LigKern::KERN && ci.skip <= LigKern::STOP {
                        s = char_kern(&cf, ci);
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

    /// §738 (xetex.web §781: accents of native fonts, placed by their
    /// attachment points, growing by their horizontal variants and
    /// assemblies; bottom and fixed accents).
    fn make_math_accent(&mut self, q: &mut Noad, fam: u8, ch: u32, sub: u8) {
        let bottom = sub & BOTTOM_ACC != 0;
        let accentee = |q: &mut Noad| {
            if matches!(q.nucleus, Field::Char { .. }) { q.nucleus.clone() } else { std::mem::take(&mut q.nucleus) }
        };
        let (f, mut s, mut x, w, mut h, tfm) = match self.fetch(fam, ch) {
            Fetched::None => return,
            Fetched::Native(f) => {
                let s = if bottom { 0 } else { self.compute_ot_math_accent_pos(&mut q.nucleus) };
                let a = accentee(q);
                let x = self.clean_box(a, cramped(self.style));
                let (w, h) = (x.w, x.h);
                (f, s, x, w, h, None)
            }
            Fetched::Tfm(f, mut i) => {
                let mut c = ch as u8;
                let s = self.skew(&mut q.nucleus);
                let a = accentee(q);
                let x = self.clean_box(a, cramped(self.style));
                let (w, h) = (x.w, x.h);
                // (§740: a larger accent if there is one and it fits)
                while let Tag::List(y) = i.tag {
                    match f.glyph(i32::from(y)) {
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
        let mut delta = if native::is_math(&f) {
            if bottom {
                0
            } else {
                h.min(native::native(&f).map_or(0, |nf| nf.xf.ot_math_constant(ACCENT_BASE_HEIGHT)))
            }
        } else {
            h.min(self.env.param(&f, 5))
        };
        if (q.sup != Field::Empty || q.sub != Field::Empty) && matches!(q.nucleus, Field::Char { .. }) {
            // (§742: the scripts go into box `x`)
            let mut n = Noad::new(Kind::Ord);
            n.nucleus = std::mem::take(&mut q.nucleus);
            n.sup = std::mem::take(&mut q.sup);
            n.sub = std::mem::take(&mut q.sub);
            let s2 = self.style;
            x = self.clean_box(Field::Mlist(vec![item(MItem::Noad(n))]), s2);
            delta = delta + x.h - h;
            h = x.h;
        }
        let mut y = if let Some(c) = tfm {
            let mut y = Self::char_box(&f, c);
            y.shift = s + half(w - y.w);
            y
        } else {
            // (the accent as a glyph, larger as the accentee is wider)
            let c = self.env.native_char_glyph(&f, ch);
            let nf = native::native(&f).cloned();
            let mut p = glyph_item(&f, c);
            let mut assembly = None;
            if sub & FIXED_ACC == 0
                && let Some(nf) = &nf
            {
                let mut a = 0;
                loop {
                    let (g, w2) = nf.xf.ot_math_variant(u32::from(c), a, true);
                    if w2 > 0 && w2 <= w {
                        p = glyph_item(&f, g as u16);
                        a += 1;
                    }
                    // (XeTeX loops for ever on a variant of no width)
                    if w2 < 0 || w2 >= w || w2 == 0 {
                        if w2 < 0 {
                            assembly = nf.xf.ot_assembly(u32::from(c), true).map(|a| Self::opentype_assembly(&f, &a, w, true));
                        }
                        break;
                    }
                }
            }
            let mut y = match assembly {
                Some(b) => BoxNode { w: b.w, h: b.h, d: b.d, list: vec![boxed(b)], ..BoxNode::default() },
                None => BoxNode { w: p.w, h: p.h, d: p.d, list: vec![Item::Glyph(Arc::new(p))], ..BoxNode::default() },
            };
            if bottom {
                y.h = y.h.max(0);
            } else {
                y.d = y.d.max(0);
            }
            let sa = match glyph_of(y.list.first()) {
                Some(g) => match nf.as_ref().map_or(NO_ACCENT_POS, |nf| nf.xf.ot_math_accent_pos(u32::from(g.gid))) {
                    NO_ACCENT_POS => half(y.w),
                    sa => sa,
                },
                None => half(y.w),
            };
            if bottom || s == NO_ACCENT_POS {
                s = half(w);
            }
            y.shift = s - sa;
            y
        };
        y.w = 0;
        let xw = x.w;
        let mut y = if bottom {
            let xh = h;
            let mut y = vpack_natural(vec![boxed(x), boxed(y)]);
            y.shift = -(xh - y.h);
            y
        } else {
            let mut y = vpack_natural(vec![boxed(y), kern(-delta), boxed(x)]);
            if y.h < h {
                // (§739)
                y.list.insert(0, kern(h - y.h));
                y.h = h;
            }
            y
        };
        y.w = xw;
        q.nucleus = Field::Box(Arc::new(y));
    }

    /// §743–§748: a fraction's hlist.
    fn make_fraction(&mut self, q: &mut Noad) -> Vec<Item> {
        let Kind::Fraction { thickness, left, right } = q.kind else { unreachable!() };
        let thickness = if thickness == DEFAULT_CODE { self.default_rule_thickness() } else { thickness };
        let size = self.size;
        let style = self.style;
        // (§744)
        let x = self.clean_box(std::mem::take(&mut q.sup), num_style(style));
        let z = self.clean_box(std::mem::take(&mut q.sub), denom_style(style));
        let (x, z) = if x.w < z.w {
            let zw = z.w;
            (Self::rebox(x, zw), z)
        } else {
            let xw = x.w;
            (x, Self::rebox(z, xw))
        };
        let (mut shift_up, mut shift_down);
        if style < TEXT {
            shift_up = self.mathsy(8, size);
            shift_down = self.mathsy(11, size);
        } else {
            shift_down = self.mathsy(12, size);
            shift_up = if thickness == 0 { self.mathsy(10, size) } else { self.mathsy(9, size) };
        }
        let mut delta = 0;
        // (xetex.web §789, §790: the gaps of `cur_f`, the font of the last
        // character fetched, if an OpenType math font)
        let ot = self.cur_mathfont();
        if thickness == 0 {
            // (§745)
            let clr = if ot {
                self.cur_constant(if style < TEXT { STACK_DISPLAY_STYLE_GAP_MIN } else { STACK_GAP_MIN })
            } else {
                let t = self.default_rule_thickness();
                if style < TEXT { 7 * t } else { 3 * t }
            };
            let d = half(clr - ((shift_up - x.d) - (z.h - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
        } else {
            // (§746)
            let (clr1, clr2) = if ot {
                if style < TEXT {
                    (self.cur_constant(FRACTION_NUM_DISPLAY_STYLE_GAP_MIN), self.cur_constant(FRACTION_DENOM_DISPLAY_STYLE_GAP_MIN))
                } else {
                    (self.cur_constant(FRACTION_NUMERATOR_GAP_MIN), self.cur_constant(FRACTION_DENOMINATOR_GAP_MIN))
                }
            } else if style < TEXT {
                (3 * thickness, 3 * thickness)
            } else {
                (thickness, thickness)
            };
            delta = half(thickness);
            let axis = self.axis_height(size);
            let delta1 = clr1 - ((shift_up - x.d) - (axis + delta));
            let delta2 = clr2 - ((axis - delta) - (z.h - shift_down));
            if delta1 > 0 {
                shift_up += delta1;
            }
            if delta2 > 0 {
                shift_down += delta2;
            }
        }
        // (§747)
        let mut v = BoxNode { vertical: true, h: shift_up + x.h, d: z.d + shift_down, w: x.w, ..BoxNode::default() };
        let (xd, zh) = (x.d, z.h);
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
        // (§748)
        let delta = if style < TEXT { self.mathsy(20, size) } else { self.mathsy(21, size) };
        let l = self.var_delimiter(left, size, delta);
        let r = self.var_delimiter(right, size, delta);
        vec![boxed(hpack_natural(vec![boxed(l), boxed(v), boxed(r)]))]
    }

    /// §749–§751: the italic correction, and the box with limits if it has
    /// them.
    fn make_op(&mut self, q: &mut Noad) -> (i32, Option<BoxNode>) {
        let Kind::Op(limits) = &mut q.kind else { unreachable!() };
        if *limits == Limits::Normal && self.style < TEXT {
            *limits = Limits::Limits;
        }
        let limits = *limits == Limits::Limits;
        let mut delta = 0;
        if let Field::Char { .. } = q.nucleus {
            if let Fetched::Tfm(f, mut g) = self.fetch_field(&mut q.nucleus) {
                if self.style < TEXT
                    && let Tag::List(c) = g.tag
                    && let Some(i) = f.glyph(i32::from(c))
                {
                    g = i;
                    if let Field::Char { ch, .. } = &mut q.nucleus {
                        *ch = u32::from(c);
                    }
                }
                delta = g.italic;
            }
            let s = self.style;
            let mut x = self.clean_box(std::mem::take(&mut q.nucleus), s);
            if self.cur_mathfont()
                && let Some(f) = self.cur_f.clone()
                && let Some(nf) = native::native(&f).cloned()
                && let Some(mut p) = glyph_of(x.list.first()).map(|g| (*g).clone())
            {
                // (xetex.web §793: in display style a variant at least
                // `displayOperatorMinHeight` high and larger than the text
                // style's glyph, else the glyph's assembly)
                let mut assembly = None;
                if self.style < TEXT {
                    let mut h1 = nf.xf.ot_math_constant(DISPLAY_OPERATOR_MIN_HEIGHT);
                    let h = (p.h + p.d).wrapping_mul(5) / 4;
                    if h1 < h {
                        h1 = h;
                    }
                    let c = p.gid;
                    let mut n = 0;
                    loop {
                        let (g, h2) = nf.xf.ot_math_variant(u32::from(c), n, false);
                        if h2 > 0 {
                            p = glyph_item(&f, g as u16);
                        }
                        n += 1;
                        if h2 < 0 || h2 >= h1 {
                            if h2 < 0 {
                                assembly = nf.xf.ot_assembly(u32::from(c), false).map(|a| Self::opentype_assembly(&f, &a, h1, false));
                            }
                            break;
                        }
                    }
                }
                let (w, h, d) = if let Some(b) = assembly {
                    delta = 0;
                    let dims = (b.w, b.h, b.d);
                    x.list = vec![boxed(b)];
                    dims
                } else {
                    delta = nf.xf.ot_math_ital_corr(u32::from(p.gid));
                    let dims = (p.w, p.h, p.d);
                    x.list[0] = Item::Glyph(Arc::new(p));
                    dims
                };
                x.w = w;
                x.h = h;
                x.d = d;
            }
            if q.sub != Field::Empty && !limits {
                x.w -= delta;
            }
            let size = self.size;
            x.shift = half(x.h - x.d) - self.axis_height(size);
            q.nucleus = Field::Box(Arc::new(x));
        }
        if !limits {
            return (delta, None);
        }
        // (§750)
        let save_f = self.cur_f.clone();
        let style = self.style;
        let sup_empty = q.sup == Field::Empty;
        let sub_empty = q.sub == Field::Empty;
        let x = self.clean_box(std::mem::take(&mut q.sup), sup_style(style));
        let y = self.clean_box(std::mem::take(&mut q.nucleus), style);
        let z = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
        self.cur_f = save_f;
        let mut v = BoxNode { vertical: true, w: y.w.max(x.w).max(z.w), ..BoxNode::default() };
        let mut x = Self::rebox(x, v.w);
        let y = Self::rebox(y, v.w);
        let mut z = Self::rebox(z, v.w);
        x.shift = half(delta);
        z.shift = -x.shift;
        v.h = y.h;
        v.d = y.d;
        // (§751)
        if sup_empty {
            v.list.push(boxed(y));
        } else {
            let shift_up = (self.mathex(11) - x.d).max(self.mathex(9));
            let s5 = self.mathex(13);
            v.h += s5 + x.h + x.d + shift_up;
            v.list.push(kern(s5));
            v.list.push(boxed(x));
            v.list.push(kern(shift_up));
            v.list.push(boxed(y));
        }
        if !sub_empty {
            let shift_down = (self.mathex(12) - z.h).max(self.mathex(10));
            let s5 = self.mathex(13);
            v.d += s5 + z.h + z.d + shift_down;
            v.list.push(kern(shift_down));
            v.list.push(boxed(z));
            v.list.push(kern(s5));
        }
        (delta, Some(v))
    }

    /// §752–§753: ligatures and kerns between ordinary characters.
    fn make_ord(&mut self, q: &mut Noad, todo: &mut VecDeque<Item>) {
        'restart: loop {
            if q.sub != Field::Empty || q.sup != Field::Empty {
                return;
            }
            let Field::Char { fam, ch } = q.nucleus else { return };
            let Some(Item::Noad(pm)) = todo.front() else { return };
            let MItem::Noad(p) = &**pm else { return };
            let Field::Char { fam: pfam, ch: pch } = p.nucleus else { return };
            if !(ORD..=PUNCT).contains(&p.kind.code()) || pfam != fam {
                return;
            }
            q.nucleus = Field::TextChar { fam, ch };
            let Some((cf, _, g)) = self.fetch_tfm(&mut q.nucleus) else { return };
            let Tag::Lig(start) = g.tag else { return };
            let mut k = usize::from(start);
            let first = cf.lig_kerns[k];
            if first.skip > LigKern::STOP {
                k = 256 * usize::from(first.op) + usize::from(first.remainder);
            }
            loop {
                let ci = cf.lig_kerns[k];
                // (XeTeX compares the character's low 16 bits)
                if u32::from(ci.next) == pch & 0xFFFF && ci.skip <= LigKern::STOP {
                    if ci.op >= LigKern::KERN {
                        todo.push_front(kern(char_kern(&cf, ci)));
                        return;
                    }
                    let rem = u32::from(ci.remainder);
                    match ci.op {
                        1 | 5 => q.nucleus = Field::TextChar { fam, ch: rem },
                        2 | 6 => {
                            if let Some(Item::Noad(pm)) = todo.front_mut()
                                && let MItem::Noad(p) = Arc::make_mut(pm)
                            {
                                p.nucleus = Field::Char { fam, ch: (pch & !0xFFFF) | rem };
                            }
                        }
                        3 | 7 | 11 => {
                            let mut r = Noad::new(Kind::Ord);
                            r.nucleus = if ci.op < 11 {
                                Field::Char { fam, ch: rem }
                            } else {
                                Field::TextChar { fam, ch: rem }
                            };
                            todo.push_front(item(MItem::Noad(r)));
                        }
                        _ => {
                            let Some(Item::Noad(pm)) = todo.pop_front() else { unreachable!() };
                            let MItem::Noad(p) = Arc::try_unwrap(pm).unwrap_or_else(|m| (*m).clone()) else {
                                unreachable!()
                            };
                            q.nucleus = Field::TextChar { fam, ch: rem };
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

    /// xetex.web §805 "Fetch first character of a sub/superscript": the
    /// font and glyph of script `field`'s first character if it is in an
    /// OpenType math font. Kerns and glue before it are passed, style
    /// changes obeyed, a `\mathchoice` followed; it is fetched in
    /// subscript style, a missing one complained about (and its field
    /// emptied) as `fetch` does.
    fn script_glyph(&mut self, field: &mut Field) -> (Option<Arc<Font>>, u16) {
        let mut this_style = sub_style(self.style);
        let head = match field {
            Field::Mlist(l) => first_nucleus(l, &mut this_style),
            f => Some(f),
        };
        let Some(head) = head else { return (None, 0) };
        let Field::Char { ch, .. } = *head else { return (None, 0) };
        let save_f = self.cur_f.clone();
        let saved_style = self.style;
        self.style = this_style;
        self.set_size_and_mu();
        self.fetch_field(head);
        let r = match self.cur_f.clone() {
            Some(f) if native::is_math(&f) => {
                let g = self.env.native_char_glyph(&f, ch);
                (Some(f), g)
            }
            _ => (None, 0),
        };
        self.cur_f = save_f;
        self.style = saved_style;
        self.set_size_and_mu();
        r
    }

    /// xetex.web §806: the OpenType math kern between nucleus `p` (if it
    /// begins with a glyph) and a script of glyph `sg` in `sf`, shifted by
    /// `shift`; appended to `p` unless zero.
    fn attach_ot_kern(p: &mut Vec<Item>, script: &(Option<Arc<Font>>, u16), sup: bool, shift: i32) -> i32 {
        let Some(g) = glyph_of(p.first()) else { return 0 };
        let Some(nf) = native::native(&g.font) else { return 0 };
        let k = match &script.0 {
            Some(sf) => match native::native(sf) {
                Some(snf) => {
                    let cmd = if sup { partex_otf::xetex::ScriptKind::Sup } else { partex_otf::xetex::ScriptKind::Sub };
                    nf.xf.ot_math_kern(u32::from(g.gid), &snf.xf, u32::from(script.1), cmd, shift)
                }
                None => 0,
            },
            None => 0,
        };
        if k != 0 {
            p.push(kern(k));
        }
        k
    }

    /// §756–§759 (xetex.web §800: an OpenType math font's script
    /// parameters and kerns, of `cur_f`): the scripts of `q` attached to
    /// its hlist `p`.
    fn make_scripts(&mut self, q: &mut Noad, p: &mut Vec<Item>, delta: i32) {
        let style = self.style;
        let size = self.size;
        let (mut shift_up, mut shift_down);
        if matches!(p.first(), Some(Item::Char(..) | Item::Glyph(_))) {
            shift_up = 0;
            shift_down = 0;
        } else {
            let (zh, zd) = natural_hd(p);
            let t = if style < SCRIPT { SCRIPT_SIZE } else { SCRIPT_SCRIPT_SIZE };
            shift_up = zh - self.mathsy(18, t);
            shift_down = zd + self.mathsy(19, t);
        }
        let script_space = self.params.script_space;
        let x = if q.sup == Field::Empty {
            // (§757)
            let sub = self.script_glyph(&mut q.sub);
            let save_f = self.cur_f.clone();
            let mut x = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
            self.cur_f = save_f;
            x.w += script_space;
            shift_down = shift_down.max(self.mathsy(16, size));
            let clr = if self.cur_mathfont() {
                x.h - self.cur_constant(SUBSCRIPT_TOP_MAX)
            } else {
                x.h - (self.math_x_height(size) * 4).abs() / 5
            };
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            if self.cur_mathfont() {
                Self::attach_ot_kern(p, &sub, false, shift_down);
            }
            x
        } else {
            // (§758)
            let sup = self.script_glyph(&mut q.sup);
            let save_f = self.cur_f.clone();
            let mut x = self.clean_box(std::mem::take(&mut q.sup), sup_style(style));
            self.cur_f = save_f;
            x.w += script_space;
            let clr = if style % 2 == 1 {
                self.mathsy(15, size)
            } else if style < TEXT {
                self.mathsy(13, size)
            } else {
                self.mathsy(14, size)
            };
            shift_up = shift_up.max(clr);
            let clr = if self.cur_mathfont() {
                x.d + self.cur_constant(SUPERSCRIPT_BOTTOM_MIN)
            } else {
                x.d + self.math_x_height(size).abs() / 4
            };
            shift_up = shift_up.max(clr);
            if self.cur_mathfont() && q.sub == Field::Empty {
                // (with a subscript the kern goes into the shift)
                Self::attach_ot_kern(p, &sup, true, shift_up);
            }
            if q.sub == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // (§759)
                let save_f = self.cur_f.clone();
                let sub = self.script_glyph(&mut q.sub);
                let sub_empty = q.sub == Field::Empty;
                let mut y = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
                self.cur_f = save_f;
                y.w += script_space;
                shift_down = shift_down.max(self.mathsy(17, size));
                let ot = self.cur_mathfont();
                let clr = if ot { self.cur_constant(SUB_SUPERSCRIPT_GAP_MIN) } else { 4 * self.default_rule_thickness() }
                    - ((shift_up - x.d) - (y.h - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    let clr = if ot {
                        self.cur_constant(SUPERSCRIPT_BOTTOM_MAX_WITH_SUBSCRIPT)
                    } else {
                        (self.math_x_height(size) * 4).abs() / 5
                    } - (shift_up - x.d);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                let (mut sub_kern, mut sup_kern) = (0, 0);
                if ot {
                    sub_kern = Self::attach_ot_kern(p, &sub, false, shift_down);
                    if sub_empty {
                        sup_kern = Self::attach_ot_kern(p, &sup, true, shift_up);
                    }
                }
                // (the superscript `delta` right of the subscript)
                x.shift = sup_kern + delta - sub_kern;
                let k = kern((shift_up - x.d) - (y.h - shift_down));
                let mut x = vpack_natural(vec![boxed(x), k, boxed(y)]);
                x.shift = shift_down;
                x
            }
        };
        p.push(boxed(x));
    }

    /// §760–§767: the second pass: spacing and penalties.
    fn second_pass(&mut self, done: Vec<Done>, style: u8, penalties: bool, max_h: i32, max_d: i32) -> Vec<Item> {
        let mut out = Vec::new();
        let mut r_type = 0;
        self.style = style;
        self.set_size_and_mu();
        let codes: Vec<i32> = done
            .iter()
            .map(|d| match d {
                Done::Noad { code, .. } => *code,
                Done::Style(_) => STYLE_NODE,
                Done::Node(Item::Penalty(_)) => PENALTY_NODE,
                Done::Node(_) => 0,
            })
            .collect();
        for (i, d) in done.into_iter().enumerate() {
            // (§761)
            let (code, mut hlist, delim) = match d {
                Done::Style(s) => {
                    // (§763)
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
                OP | OPEN | CLOSE | PUNCT | INNER => code,
                BIN => {
                    pen = self.params.bin_op_penalty;
                    BIN
                }
                REL => {
                    pen = self.params.rel_penalty;
                    REL
                }
                FRACTION => INNER,
                LEFT | RIGHT => {
                    hlist = vec![boxed(self.make_left_right(delim, style, max_d, max_h))];
                    code - (LEFT - OPEN)
                }
                _ => ORD,
            };
            // (§766)
            if r_type > 0 {
                let k = ((r_type - ORD) * 8 + (t - ORD)) as usize;
                let script = self.style < SCRIPT;
                let x = match MATH_SPACING[k] {
                    b'1' => script.then_some(0),
                    b'2' => Some(0),
                    b'3' => script.then_some(1),
                    b'4' => script.then_some(2),
                    _ => None,
                };
                if let Some(x) = x {
                    let g = [&self.params.thin_mu_skip, &self.params.med_mu_skip, &self.params.thick_mu_skip][x];
                    let g = math_glue(g, self.mu);
                    out.push(Item::Glue { g: Arc::new(g), sub: 16 + x as u8, zero: false });
                }
            }
            // (§767)
            out.extend(hlist);
            if penalties
                && let Some(&next) = codes.get(i + 1)
                && pen < INF_PENALTY
                && next != PENALTY_NODE
                && next != REL
            {
                out.push(Item::Penalty(pen));
            }
            // (etex.web §760: after a right noad, as after an open one)
            r_type = if code == RIGHT { OPEN } else { t };
        }
        out
    }

    /// §762
    fn make_left_right(&mut self, d: Delim, style: u8, max_d: i32, max_h: i32) -> BoxNode {
        self.style = style;
        self.set_size_and_mu();
        let size = self.size;
        let mut delta2 = max_d + self.axis_height(size);
        let mut delta1 = max_h + max_d - delta2;
        if delta2 > delta1 {
            delta1 = delta2;
        }
        let mut delta = (delta1 / 500) * self.params.delimiter_factor;
        delta2 = delta1 + delta1 - self.params.delimiter_shortfall;
        if delta < delta2 {
            delta = delta2;
        }
        self.var_delimiter(d, size, delta)
    }
}

/// xetex.web §805: the nucleus of the first noad of a script's mlist that
/// is ordinary to `\mathpunct`, past kerns, glue and style changes (which
/// set `style`), into the branch of a `\mathchoice` that `style` picks.
fn first_nucleus<'a>(l: &'a mut [Item], style: &mut u8) -> Option<&'a mut Field> {
    for it in l {
        match it {
            Item::Kern { .. } | Item::Glue { .. } | Item::Leaders(_) => {}
            Item::Noad(m) => {
                let m = Arc::make_mut(m);
                match m {
                    MItem::Style(s) => *style = *s,
                    MItem::Choice(c) => {
                        let k = usize::from(*style / 2).min(3);
                        return first_nucleus(&mut c[k], style);
                    }
                    MItem::Noad(n)
                        if matches!(
                            n.kind,
                            Kind::Ord | Kind::Op(_) | Kind::Bin | Kind::Rel | Kind::Open | Kind::Close | Kind::Punct
                        ) =>
                    {
                        return Some(&mut n.nucleus);
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    None
}

/// The math families a list uses (its characters', delimiters' and
/// accents', in its subformulas too): XeTeX's 256 families are read only
/// where used.
pub fn families(list: &[Item], out: &mut [bool; 256]) {
    let field = |f: &Field, out: &mut [bool; 256]| match f {
        Field::Char { fam, .. } | Field::TextChar { fam, .. } => out[usize::from(*fam)] = true,
        Field::Mlist(l) => families(l, out),
        _ => {}
    };
    let delim = |d: &Delim, out: &mut [bool; 256]| {
        out[usize::from(d.small_fam)] = true;
        out[usize::from(d.large_fam)] = true;
    };
    for it in list {
        let Item::Noad(m) = it else { continue };
        match &**m {
            MItem::Noad(n) | MItem::Replace(n) => {
                field(&n.nucleus, out);
                field(&n.sup, out);
                field(&n.sub, out);
                match &n.kind {
                    Kind::Radical(d) | Kind::Left(d) | Kind::Right(d, _) => delim(d, out),
                    Kind::Fraction { left, right, .. } => {
                        delim(left, out);
                        delim(right, out);
                    }
                    Kind::Accent { fam, .. } => out[usize::from(*fam)] = true,
                    _ => {}
                }
            }
            MItem::Choice(c) => {
                for l in c.iter() {
                    families(l, out);
                }
            }
            MItem::Set(_, f) => field(f, out),
            MItem::Above { left, right, .. } => {
                delim(left, out);
                delim(right, out);
            }
            _ => {}
        }
    }
}

/// `done_with_noad` after `check_dimensions` (§727).
fn push_noad(
    done: &mut Vec<Done>,
    r: &mut Option<usize>,
    r_type: &mut i32,
    code: i32,
    hlist: Vec<Item>,
    max_h: &mut i32,
    max_d: &mut i32,
) {
    let (h, d) = natural_hd(&hlist);
    *max_h = (*max_h).max(h);
    *max_d = (*max_d).max(d);
    done.push(Done::Noad { code, hlist, delim: Delim::default() });
    *r = Some(done.len() - 1);
    *r_type = code;
}
