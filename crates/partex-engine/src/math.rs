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
use crate::node::{BoxNode, FontId, GlueSpec, Node, Order, RUNNING, push_char};
use crate::pack::{self, Confusion, Fonts, Spec};
use crate::scaled::xn_over_d;

/// §157
const INF_PENALTY: i32 = 10000;
/// §679: `default_code`, the thickness of `\over`.
pub const DEFAULT_CODE: Scaled = 0o10000000000;
/// §155, §149: glue and kern subtypes in mlists.
pub const COND_MATH_GLUE: u8 = 98;
pub const MU_GLUE: u8 = 99;
const EXPLICIT: u8 = 1;
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

/// §683: a delimiter field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u8,
    pub large_fam: u8,
    pub large_char: u8,
}

crate::persist_struct!(Delim {
    small_fam,
    small_char,
    large_fam,
    large_char
});

/// §681: a noad field.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
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
    Accent {
        fam: u8,
        ch: u8,
    },
    Vcenter,
    Left(Delim),
    Right(Delim),
    /// e-TeX's `\middle`: a right noad that another group follows.
    Middle(Delim),
}

crate::persist_enum!(Kind { Ord, Op(a0), Bin, Rel, Open, Close, Punct, Inner, Radical(a0), Fraction { thickness, left, right }, Under, Over, Accent { fam, ch }, Vcenter, Left(a0), Right(a0), Middle(a0) });

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

/// The fonts of the math families.
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
}

/// What the caller reports, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// §723: "\textfont 3 is undefined (character x)"; `size` is 0, 256
    /// or 512 (`script_size`, 4.7).
    UndefinedFamily { size: u16, fam: u8, ch: u8 },
    /// §581: the character is not in the font.
    MissingChar { font: FontId, ch: u8 },
}

struct Ctx<'a, E: Env> {
    env: &'a mut E,
    params: &'a Params,
    events: Vec<Event>,
    style: u8,
    size: usize,
    mu: Scaled,
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
    let mut cx = Ctx {
        env,
        params,
        events: Vec::new(),
        style,
        size: 0,
        mu: 0,
    };
    let list = cx.mlist_to_hlist(mlist, style, penalties)?;
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
    fn glyph(&self, f: FontId, c: u8) -> Option<Glyph> {
        self.env.font(f).glyph(i32::from(c))
    }

    fn fam_param(&self, n: usize, k: usize) -> Scaled {
        self.env.fam_fnt(n).map_or(0, |f| self.env.param(f, k))
    }
    /// §700
    fn mathsy(&self, k: usize, size: usize) -> Scaled {
        self.fam_param(2 + size, k)
    }
    fn math_x_height(&self, s: usize) -> Scaled {
        self.mathsy(5, s)
    }
    fn axis_height(&self, s: usize) -> Scaled {
        self.mathsy(22, s)
    }
    /// §701
    fn mathex(&self, k: usize) -> Scaled {
        self.fam_param(3 + self.size, k)
    }
    fn default_rule_thickness(&self) -> Scaled {
        self.mathex(8)
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

    /// §709
    fn char_box(&self, f: FontId, c: u8) -> BoxNode {
        let g = self.glyph(f, c).unwrap_or_default();
        BoxNode {
            width: g.width + g.italic,
            height: g.height,
            depth: g.depth,
            list: vec![Node::Glyphs(crate::node::Glyphs::one(f, c))],
            ..BoxNode::default()
        }
    }

    fn height_plus_depth(&self, f: FontId, c: u8) -> Scaled {
        let g = self.glyph(f, c).unwrap_or_default();
        g.height + g.depth
    }

    /// §706: a delimiter of size at least `v` for `d` in size `s`.
    fn var_delimiter(&mut self, d: Delim, s: usize, v: Scaled) -> Result<BoxNode, Confusion> {
        let mut found: Option<(FontId, u8, Glyph)> = None;
        let mut w = 0;
        'found: for (z0, x) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // §707: look at the variants of `(z,x)`.
            if z0 != 0 || x != 0 {
                let mut z = usize::from(z0) + s + 16;
                loop {
                    z -= 16;
                    if let Some(g) = self.env.fam_fnt(z) {
                        // §708
                        let mut y = x;
                        while let Some(q) = self.glyph(g, y) {
                            if let Tag::Ext(_) = q.tag {
                                found = Some((g, y, q));
                                break 'found;
                            }
                            let u = q.height + q.depth;
                            if u > w {
                                found = Some((g, y, q));
                                w = u;
                                if u >= v {
                                    break 'found;
                                }
                            }
                            let Tag::List(next) = q.tag else {
                                break;
                            };
                            y = next;
                        }
                    }
                    if z < 16 {
                        break;
                    }
                }
            }
        }
        let mut b = match found {
            None => BoxNode {
                width: self.params.null_delimiter_space,
                ..BoxNode::default()
            },
            Some((
                f,
                _,
                Glyph {
                    tag: Tag::Ext(r), ..
                },
            )) => self.extensible(f, r, v)?,
            Some((f, c, _)) => self.char_box(f, c),
        };
        b.shift = half(b.height - b.depth) - self.axis_height(s);
        Ok(b)
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
        let g = self.glyph(f, e.rep).unwrap_or_default();
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
                let v = self.glyph(g.font, g.chars()[0]).unwrap_or_default().width;
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

    /// §722: fetch a math character; `None` (with an event) if it is
    /// not there.
    fn fetch(&mut self, fam: u8, ch: u8) -> Option<(FontId, Glyph)> {
        let Some(f) = self.env.fam_fnt(usize::from(fam) + self.size) else {
            self.events.push(Event::UndefinedFamily {
                size: u16::try_from(self.size).unwrap_or(0),
                fam,
                ch,
            });
            return None;
        };
        if let Some(g) = self.glyph(f, ch) {
            Some((f, g))
        } else {
            self.events.push(Event::MissingChar { font: f, ch });
            None
        }
    }

    /// `fetch` of a character field, emptying it if the character is not
    /// there.
    fn fetch_field(&mut self, field: &mut Field) -> Option<(FontId, u8, Glyph)> {
        let (Field::Char { fam, ch } | Field::TextChar { fam, ch }) = *field else {
            return None;
        };
        if let Some((f, g)) = self.fetch(fam, ch) {
            Some((f, ch, g))
        } else {
            *field = Field::Empty;
            None
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
                Kind::Accent { fam, ch } => {
                    let (fam, ch) = (*fam, *ch);
                    self.make_math_accent(&mut q, fam, ch)?;
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
                    // §755
                    if let Some((f, c, g)) = self.fetch_field(&mut n) {
                        delta = g.italic;
                        let mut p = vec![Node::Glyphs(crate::node::Glyphs::one(f, c))];
                        if text && self.env.param(f, 2) != 0 {
                            delta = 0; // no italic correction in mid-word of text font
                        }
                        if q.sub == Field::Empty && delta != 0 {
                            p.push(kern(delta));
                            delta = 0;
                        }
                        p
                    } else {
                        Vec::new()
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

    /// §737
    fn make_radical(&mut self, q: &mut Noad, d: Delim) -> Result<(), Confusion> {
        let x = self.clean_box(core::mem::take(&mut q.nucleus), cramped(self.style))?;
        let t = self.default_rule_thickness();
        let mut clr = if self.style < TEXT {
            t + self.math_x_height(self.size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let mut y = self.var_delimiter(d, self.size, x.height + x.depth + clr + t)?;
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

    /// §738
    fn make_math_accent(&mut self, q: &mut Noad, fam: u8, ch: u8) -> Result<(), Confusion> {
        let Some((f, mut i)) = self.fetch(fam, ch) else {
            return Ok(());
        };
        let mut c = ch;
        // §741: compute the amount of skew.
        let mut s = 0;
        if let Field::Char { .. } = q.nucleus
            && let Some((cf, _, g)) = self.fetch_field(&mut q.nucleus)
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
        // done1:
        let nucleus_char = matches!(q.nucleus, Field::Char { .. });
        let nucleus = if nucleus_char {
            q.nucleus.clone()
        } else {
            core::mem::take(&mut q.nucleus)
        };
        let mut x = self.clean_box(nucleus, cramped(self.style))?;
        let w = x.width;
        let mut h = x.height;
        // §740: switch to a larger accent if available and appropriate.
        while let Tag::List(y) = i.tag {
            match self.glyph(f, y) {
                Some(g) if g.width <= w => {
                    i = g;
                    c = y;
                }
                _ => break,
            }
        }
        let x_height = self.env.param(f, 5);
        let mut delta = if h < x_height { h } else { x_height };
        if (q.sup != Field::Empty || q.sub != Field::Empty) && nucleus_char {
            // §742: swap the subscript and superscript into box `x`.
            let mut n = Noad::new(Kind::Ord);
            n.nucleus = core::mem::take(&mut q.nucleus);
            n.sup = core::mem::take(&mut q.sup);
            n.sub = core::mem::take(&mut q.sub);
            x = self.clean_box(Field::Mlist(vec![Item::Noad(Box::new(n))]), self.style)?;
            delta = delta + x.height - h;
            h = x.height;
        }
        let mut y = self.char_box(f, c);
        y.shift = s + half(w - y.width);
        y.width = 0;
        let xw = x.width;
        let mut y = self.vpack(vec![boxed(y), kern(-delta), boxed(x)])?;
        y.width = xw;
        if y.height < h {
            // §739
            y.list.insert(0, kern(h - y.height));
            y.height = h;
        }
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
        if thickness == 0 {
            // §745
            let t = self.default_rule_thickness();
            let clr = if style < TEXT { 7 * t } else { 3 * t };
            let d = half(clr - ((shift_up - x.depth) - (z.height - shift_down)));
            if d > 0 {
                shift_up += d;
                shift_down += d;
            }
        } else {
            // §746
            let clr = if style < TEXT {
                3 * thickness
            } else {
                thickness
            };
            delta = half(thickness);
            let axis = self.axis_height(size);
            let delta1 = clr - ((shift_up - x.depth) - (axis + delta));
            let delta2 = clr - ((axis - delta) - (z.height - shift_down));
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
            if let Some((f, _, mut g)) = self.fetch_field(&mut q.nucleus) {
                if self.style < TEXT
                    && let Tag::List(c) = g.tag
                    && let Some(i) = self.glyph(f, c)
                {
                    g = i;
                    if let Field::Char { ch, .. } = &mut q.nucleus {
                        *ch = c;
                    }
                }
                delta = g.italic;
            }
            let mut x = self.clean_box(core::mem::take(&mut q.nucleus), self.style)?;
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
        let style = self.style;
        let sup_empty = q.sup == Field::Empty;
        let sub_empty = q.sub == Field::Empty;
        let x = self.clean_box(core::mem::take(&mut q.sup), sup_style(style))?;
        let y = self.clean_box(core::mem::take(&mut q.nucleus), style)?;
        let z = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
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
            let Some((cf, _, g)) = self.fetch_field(&mut q.nucleus) else {
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
                if ci.next == pch && ci.skip <= LigKern::STOP {
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
                                ch: ci.remainder,
                            }
                        } // =:|, =:|>
                        2 | 6 => {
                            p.nucleus = Field::Char {
                                fam,
                                ch: ci.remainder,
                            }
                        } // |=:, |=:>
                        3 | 7 | 11 => {
                            // |=:|, |=:|>, |=:|>>
                            let mut r = Noad::new(Kind::Ord);
                            r.nucleus = if ci.op < 11 {
                                Field::Char {
                                    fam,
                                    ch: ci.remainder,
                                }
                            } else {
                                Field::TextChar {
                                    fam,
                                    ch: ci.remainder,
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
                                ch: ci.remainder,
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

    /// §756: attach the scripts of `q` to its hlist `p`.
    fn make_scripts(
        &mut self,
        q: &mut Noad,
        p: &mut Vec<Node>,
        delta: Scaled,
    ) -> Result<(), Confusion> {
        let style = self.style;
        let size = self.size;
        let (mut shift_up, mut shift_down);
        if let Some(Node::Glyphs(_)) = p.first() {
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
        let script_space = self.params.script_space;
        let x = if q.sup == Field::Empty {
            // §757
            let mut x = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
            x.width += script_space;
            shift_down = shift_down.max(self.mathsy(16, size));
            let clr = x.height - (self.math_x_height(size) * 4).abs() / 5;
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            x
        } else {
            // §758
            let mut x = self.clean_box(core::mem::take(&mut q.sup), sup_style(style))?;
            x.width += script_space;
            let clr = if style % 2 == 1 {
                self.mathsy(15, size)
            } else if style < TEXT {
                self.mathsy(13, size)
            } else {
                self.mathsy(14, size)
            };
            shift_up = shift_up.max(clr);
            let clr = x.depth + self.math_x_height(size).abs() / 4;
            shift_up = shift_up.max(clr);
            if q.sub == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // §759
                let mut y = self.clean_box(core::mem::take(&mut q.sub), sub_style(style))?;
                y.width += script_space;
                shift_down = shift_down.max(self.mathsy(17, size));
                let clr = 4 * self.default_rule_thickness()
                    - ((shift_up - x.depth) - (y.height - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    let clr = (self.math_x_height(size) * 4).abs() / 5 - (shift_up - x.depth);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                x.shift = delta; // superscript is `delta` to the right of the subscript
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
