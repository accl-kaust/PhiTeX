//! Typesetting math formulas (tex.web parts 34–36, §680–§767), copied as a
//! pure function: [`mlist_to_hlist`] turns a math list into a horizontal
//! list, as a function of the list, the style, the parameters the formula
//! read where it ended ([`Params`]), the fonts of the math families
//! ([`Fonts`]) and their `\fontdimen`s and `\skewchar`s, read through
//! [`MathEnv`] (the families' `\fontdimen`s are global values: the σ
//! parameters of family 2, the ξ parameters of family 3, §700–702).
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
/// §699: the sizes, as offsets of the family fonts.
const TEXT_SIZE: usize = 0;
const SCRIPT_SIZE: usize = 16;
const SCRIPT_SCRIPT_SIZE: usize = 32;
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

/// §683: a delimiter field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u8,
    pub large_fam: u8,
    pub large_char: u8,
}

impl Delim {
    /// From a 24-bit delimiter code (§1160).
    pub fn from_code(v: i32) -> Delim {
        Delim {
            small_fam: ((v / 0o4_000_000) % 16) as u8,
            small_char: ((v / 0o10_000) % 256) as u8,
            large_fam: ((v / 256) % 16) as u8,
            large_char: (v % 256) as u8,
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
        ch: u8,
    },
    /// A character in mid-word (§752): no italic correction.
    TextChar {
        fam: u8,
        ch: u8,
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
    Accent {
        fam: u8,
        ch: u8,
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

/// `\textfont`, `\scriptfont`, `\scriptscriptfont` of the 16 families, at
/// `fam + size` (`size` 0, 16, 32); `None`: `\nullfont`.
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
}

/// What the caller reports, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// §723: "\textfont 3 is undefined (character x)"; `size` 0, 16, 32.
    UndefinedFamily { size: usize, fam: u8, ch: u8 },
    /// §581: the character is not in the font.
    MissingChar { font: Arc<Font>, ch: u8 },
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
    let mut cx = Ctx { env, fonts, params, events: Vec::new(), style, size: 0, mu: 0 };
    let list = cx.mlist_to_hlist(mlist, style, penalties);
    (list, cx.events)
}

impl<E: MathEnv> Ctx<'_, E> {
    fn fam(&self, z: usize) -> Option<Arc<Font>> {
        self.fonts.get(z).cloned()
    }

    /// `mathsy(k)` of family 2 in `size`.
    fn mathsy(&mut self, k: u16, size: usize) -> i32 {
        match self.fam(2 + size) {
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
            Some(f) => self.env.param(&f, k),
            None => 0,
        }
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

    /// §706: a delimiter of size at least `v` for `d` in size `s`.
    fn var_delimiter(&mut self, d: Delim, s: usize, v: i32) -> BoxNode {
        let mut f: Option<Arc<Font>> = None;
        let mut c: u8 = 0;
        let mut ext: Option<u8> = None;
        let mut w = 0;
        'found: for (z0, x) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // (§707)
            if z0 != 0 || x != 0 {
                let mut z = usize::from(z0) + s + SCRIPT_SIZE;
                loop {
                    z -= SCRIPT_SIZE;
                    if let Some(g) = self.fam(z) {
                        // (§708)
                        let mut y = x;
                        while let Some(q) = g.glyph(i32::from(y)) {
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
                            y = next;
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
            Some(f) => match ext {
                Some(r) => Self::extensible(&f, r, v),
                None => Self::char_box(&f, c),
            },
        };
        b.shift = half(b.h - b.d) - self.axis_height(s);
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

    /// §722: `fetch`: the font and metrics of a math character, or an
    /// event (and `None`) if there are none.
    fn fetch(&mut self, fam: u8, ch: u8) -> Option<(Arc<Font>, Glyph)> {
        let Some(f) = self.fam(usize::from(fam) + self.size) else {
            self.events.push(Event::UndefinedFamily { size: self.size, fam, ch });
            return None;
        };
        if let Some(g) = f.glyph(i32::from(ch)) {
            Some((f, g))
        } else {
            self.events.push(Event::MissingChar { font: f, ch });
            None
        }
    }

    /// `fetch` of a character field, emptied if the character is not
    /// there.
    fn fetch_field(&mut self, field: &mut Field) -> Option<(Arc<Font>, u8, Glyph)> {
        let (Field::Char { fam, ch } | Field::TextChar { fam, ch }) = *field else { return None };
        let r = self.fetch(fam, ch);
        if r.is_none() {
            *field = Field::Empty;
        }
        r.map(|(f, g)| (f, ch, g))
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
                Kind::Accent { fam, ch } => {
                    let (fam, ch) = (*fam, *ch);
                    self.make_math_accent(&mut q, fam, ch);
                }
                Kind::Vcenter => self.make_vcenter(&mut q),
            }
            // (§754: the nucleus as an hlist, with the scripts attached)
            let text = matches!(q.nucleus, Field::TextChar { .. });
            let mut p = match std::mem::take(&mut q.nucleus) {
                mut n @ (Field::Char { .. } | Field::TextChar { .. }) => {
                    // (§755)
                    match self.fetch_field(&mut n) {
                        Some((f, ch, g)) => {
                            delta = g.italic;
                            let mut p = vec![Item::Char(f.clone(), ch)];
                            if text && self.env.param(&f, 2) != 0 {
                                delta = 0;
                            }
                            if q.sub == Field::Empty && delta != 0 {
                                p.push(kern(delta));
                                delta = 0;
                            }
                            p
                        }
                        None => Vec::new(),
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

    /// §737
    fn make_radical(&mut self, q: &mut Noad, d: Delim) {
        let x = self.clean_box(std::mem::take(&mut q.nucleus), cramped(self.style));
        let t = self.default_rule_thickness();
        let mut clr = if self.style < TEXT {
            let size = self.size;
            t + self.math_x_height(size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let size = self.size;
        let mut y = self.var_delimiter(d, size, x.h + x.d + clr + t);
        let delta = y.d - (x.h + x.d + clr);
        if delta > 0 {
            clr += half(delta);
        }
        y.shift = -(x.h + clr);
        let yh = y.h;
        let o = Self::overbar(x, clr, yh);
        q.nucleus = Field::Box(Arc::new(hpack_natural(vec![boxed(y), boxed(o)])));
    }

    /// §741: the skew of a character nucleus: its kern with the font's
    /// `\skewchar`.
    fn skew(&mut self, nucleus: &mut Field) -> i32 {
        let mut s = 0;
        if let Field::Char { .. } = nucleus
            && let Some((cf, _, g)) = self.fetch_field(nucleus)
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

    /// §738
    fn make_math_accent(&mut self, q: &mut Noad, fam: u8, ch: u8) {
        let Some((f, mut i)) = self.fetch(fam, ch) else { return };
        let mut c = ch;
        let s = self.skew(&mut q.nucleus);
        let accentee =
            if matches!(q.nucleus, Field::Char { .. }) { q.nucleus.clone() } else { std::mem::take(&mut q.nucleus) };
        let mut x = self.clean_box(accentee, cramped(self.style));
        let (w, mut h) = (x.w, x.h);
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
        let xh = self.env.param(&f, 5);
        let mut delta = if h < xh { h } else { xh };
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
        let mut y = Self::char_box(&f, c);
        y.shift = s + half(w - y.w);
        y.w = 0;
        let xw = x.w;
        let mut y = vpack_natural(vec![boxed(y), kern(-delta), boxed(x)]);
        y.w = xw;
        if y.h < h {
            // (§739)
            y.list.insert(0, kern(h - y.h));
            y.h = h;
        }
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
        if thickness == 0 {
            // (§745)
            let t = self.default_rule_thickness();
            let clr = if style < TEXT { 7 * t } else { 3 * t };
            let d = half(clr - ((shift_up - x.d) - (z.h - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
        } else {
            // (§746)
            let clr = if style < TEXT { 3 * thickness } else { thickness };
            delta = half(thickness);
            let axis = self.axis_height(size);
            let delta1 = clr - ((shift_up - x.d) - (axis + delta));
            let delta2 = clr - ((axis - delta) - (z.h - shift_down));
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
            if let Some((f, _, mut g)) = self.fetch_field(&mut q.nucleus) {
                if self.style < TEXT
                    && let Tag::List(c) = g.tag
                    && let Some(i) = f.glyph(i32::from(c))
                {
                    g = i;
                    if let Field::Char { ch, .. } = &mut q.nucleus {
                        *ch = c;
                    }
                }
                delta = g.italic;
            }
            let s = self.style;
            let mut x = self.clean_box(std::mem::take(&mut q.nucleus), s);
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
        let style = self.style;
        let sup_empty = q.sup == Field::Empty;
        let sub_empty = q.sub == Field::Empty;
        let x = self.clean_box(std::mem::take(&mut q.sup), sup_style(style));
        let y = self.clean_box(std::mem::take(&mut q.nucleus), style);
        let z = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
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
            let Some((cf, _, g)) = self.fetch_field(&mut q.nucleus) else { return };
            let Tag::Lig(start) = g.tag else { return };
            let mut k = usize::from(start);
            let first = cf.lig_kerns[k];
            if first.skip > LigKern::STOP {
                k = 256 * usize::from(first.op) + usize::from(first.remainder);
            }
            loop {
                let ci = cf.lig_kerns[k];
                if ci.next == pch && ci.skip <= LigKern::STOP {
                    if ci.op >= LigKern::KERN {
                        todo.push_front(kern(char_kern(&cf, ci)));
                        return;
                    }
                    let rem = ci.remainder;
                    match ci.op {
                        1 | 5 => q.nucleus = Field::TextChar { fam, ch: rem },
                        2 | 6 => {
                            if let Some(Item::Noad(pm)) = todo.front_mut()
                                && let MItem::Noad(p) = Arc::make_mut(pm)
                            {
                                p.nucleus = Field::Char { fam, ch: rem };
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

    /// §756–§759: the scripts of `q` attached to its hlist `p`.
    fn make_scripts(&mut self, q: &mut Noad, p: &mut Vec<Item>, delta: i32) {
        let style = self.style;
        let size = self.size;
        let (mut shift_up, mut shift_down);
        if matches!(p.first(), Some(Item::Char(..))) {
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
            let mut x = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
            x.w += script_space;
            shift_down = shift_down.max(self.mathsy(16, size));
            let clr = x.h - (self.math_x_height(size) * 4).abs() / 5;
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            x
        } else {
            // (§758)
            let mut x = self.clean_box(std::mem::take(&mut q.sup), sup_style(style));
            x.w += script_space;
            let clr = if style % 2 == 1 {
                self.mathsy(15, size)
            } else if style < TEXT {
                self.mathsy(13, size)
            } else {
                self.mathsy(14, size)
            };
            shift_up = shift_up.max(clr);
            let clr = x.d + self.math_x_height(size).abs() / 4;
            shift_up = shift_up.max(clr);
            if q.sub == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // (§759)
                let mut y = self.clean_box(std::mem::take(&mut q.sub), sub_style(style));
                y.w += script_space;
                shift_down = shift_down.max(self.mathsy(17, size));
                let clr = 4 * self.default_rule_thickness() - ((shift_up - x.d) - (y.h - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    let clr = (self.math_x_height(size) * 4).abs() / 5 - (shift_up - x.d);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                x.shift = delta;
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
