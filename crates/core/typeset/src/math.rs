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
use crate::node::{BoxKind, BoxNode, GlueKind, GlueNode, KernKind, List, Node, RUNNING, Rule};
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
        /// Character.
        ch: u16,
    },
    /// `math_text_char`: a character in the middle of a word (§752).
    MathTextChar {
        /// Family.
        fam: u8,
        /// Character.
        ch: u16,
    },
    /// `sub_box`: a box, shifted down by its shift amount.
    SubBox(BoxNode),
    /// `sub_mlist`: a formula (possibly empty, which differs from
    /// `Empty`).
    SubMlist(MList),
}

/// A delimiter field (§683): small and large variants; family 0 and
/// character 0 together mean none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Delimiter {
    /// `small_fam`.
    pub small_fam: u8,
    /// `small_char`.
    pub small_char: u16,
    /// `large_fam`.
    pub large_fam: u8,
    /// `large_char`.
    pub large_char: u16,
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
    /// `\mathaccent`: the accent's family and character.
    Accent(u8, u16),
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
        ch: u16,
    },
    /// `char_warning`: the character is not in the font (§722).
    MissingChar {
        /// Font.
        font: FontId,
        /// Character.
        ch: u16,
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
}

/// The result of `fetch` (§722).
struct Fetched {
    f: FontId,
    c: u16,
    i: Option<CharInfo>,
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
        self.p.fam_fnt[size][usize::from(fam)]
    }

    /// `mathsy(n)` at `size` (§700).
    fn mathsy(&self, n: u16, size: usize) -> Scaled {
        self.fam_fnt(2, size).map_or(0, |f| self.fonts.param(f, n))
    }

    /// `mathex(n)` at `cur_size` (§701).
    fn mathex(&self, n: u16) -> Scaled {
        self.fam_fnt(3, self.cur_size)
            .map_or(0, |f| self.fonts.param(f, n))
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

    /// `fetch(a)` (§722): empties the field on failure.
    fn fetch(&mut self, a: &mut Field) -> Fetched {
        let (fam, c) = match a {
            Field::MathChar { fam, ch } | Field::MathTextChar { fam, ch } => (*fam, *ch),
            _ => unreachable!("fetch of a field that is no character"),
        };
        let Some(f) = self.fam_fnt(fam, self.cur_size) else {
            self.diags.push(MathDiag::UndefinedFamily {
                size: self.cur_size,
                fam,
                ch: c,
            });
            *a = Field::Empty;
            return Fetched { f: 0, c, i: None };
        };
        let i = self.fonts.char_info(f, c);
        if i.is_none() {
            self.diags.push(MathDiag::MissingChar { font: f, ch: c });
            *a = Field::Empty;
        }
        Fetched { f, c, i }
    }

    /// `char_box(f, c)` (§709).
    fn char_box(&self, f: FontId, c: u16) -> BoxNode {
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

    /// `var_delimiter(d, s, v)` (§706).
    fn var_delimiter(&self, d: Delimiter, s: usize, v: Scaled) -> BoxNode {
        let mut best: Option<(FontId, u16)> = None;
        let mut ext = None;
        let mut w = 0;
        'found: for (z, x) in [(d.small_fam, d.small_char), (d.large_fam, d.large_char)] {
            // §707
            if z == 0 && x == 0 {
                continue;
            }
            for size in (0..=s).rev() {
                let Some(g) = self.fam_fnt(z, size) else {
                    continue;
                };
                // §708
                let mut y = x;
                while let Some(q) = self.fonts.char_info(g, y) {
                    if let CharTag::Extensible { top, mid, bot, rep } = q.tag {
                        best = Some((g, y));
                        ext = Some((top, mid, bot, rep));
                        break 'found;
                    }
                    let u = q.height + q.depth;
                    if u > w {
                        best = Some((g, y));
                        w = u;
                        if u >= v {
                            break 'found;
                        }
                    }
                    match q.tag {
                        CharTag::List(n) => y = n,
                        _ => break,
                    }
                }
            }
        }
        let mut b = match (best, ext) {
            (Some((f, _)), Some((top, mid, bot, rep))) => {
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
                    let p = self.char_box(f, c);
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
            (Some((f, c)), None) => self.char_box(f, c),
            (None, _) => BoxNode {
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

    /// `make_radical` (§737).
    fn make_radical(&mut self, q: &mut Noad, d: Delimiter) {
        let x = self.clean_box(&q.nucleus, self.cur_style.cramped());
        let t = self.default_rule_thickness();
        let mut clr = if self.cur_style < Style::TEXT {
            t + self.math_x_height(self.cur_size).abs() / 4
        } else {
            t + t.abs() / 4
        };
        let mut y = self.var_delimiter(d, self.cur_size, x.height + x.depth + clr + t);
        let delta = y.depth - (x.height + x.depth + clr);
        if delta > 0 {
            clr += half(delta);
        }
        y.shift = -(x.height + clr);
        let yh = y.height;
        let over = Self::overbar(x, clr, yh);
        q.nucleus = Field::SubBox(self.hpack_nat(vec![Node::Box(Box::new(y)), Node::Box(Box::new(over))]));
    }

    /// `make_math_accent` (§738).
    fn make_math_accent(&mut self, q: &mut Noad, fam: u8, ch: u16) {
        let mut acc = Field::MathChar { fam, ch };
        let a = self.fetch(&mut acc);
        let Some(mut i) = a.i else { return };
        let (mut c, f) = (a.c, a.f);
        // §741
        let mut s = 0;
        if matches!(q.nucleus, Field::MathChar { .. }) {
            let n = self.fetch(&mut q.nucleus);
            if n.i.is_some()
                && let Ok(skew) = u16::try_from((self.p.skew_char)(n.f))
                && let Some(LigKern::Kern(k)) = self.fonts.lig_kern(n.f, Left::Char(n.c), skew)
            {
                s = k;
            }
        }
        let mut x = self.clean_box(&q.nucleus, self.cur_style.cramped());
        let w = x.width;
        let mut h = x.height;
        // §740
        while let CharTag::List(y) = i.tag {
            let Some(i2) = self.fonts.char_info(f, y) else {
                break;
            };
            if i2.width > w {
                break;
            }
            c = y;
            i = i2;
        }
        let x_height = self.fonts.param(f, 5);
        let mut delta = if h < x_height { h } else { x_height };
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
        let mut y = self.char_box(f, c);
        y.shift = s + half(w - y.width);
        y.width = 0;
        let xw = x.width;
        let mut yv = Self::vpack_nat(vec![Node::Box(Box::new(y)), new_kern(-delta), Node::Box(Box::new(x))]);
        yv.width = xw;
        if yv.height < h {
            // §739
            yv.list.insert(0, new_kern(h - yv.height));
            yv.height = h;
        }
        q.nucleus = Field::SubBox(yv);
    }

    /// `make_fraction` (§743); returns `new_hlist`.
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
        if thickness == 0 {
            // §745
            let t = self.default_rule_thickness();
            let clr = if self.cur_style < Style::TEXT {
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
            let clr = if self.cur_style < Style::TEXT {
                3 * thickness
            } else {
                thickness
            };
            delta = half(thickness);
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
            if self.cur_style < Style::TEXT
                && let CharTag::List(c) = tag(fc.i)
                && let Some(i) = self.fonts.char_info(fc.f, c)
            {
                fc.c = c;
                fc.i = Some(i);
                if let Field::MathChar { ch, .. } = &mut q.nucleus {
                    *ch = c;
                }
            }
            delta = fc.i.map_or(0, |i| i.italic);
            let mut x = self.clean_box(&q.nucleus, self.cur_style);
            if q.subscr != Field::Empty && !limits {
                x.width -= delta;
            }
            x.shift = half(x.height - x.depth) - self.axis_height(self.cur_size);
            q.nucleus = Field::SubBox(x);
        }
        if !limits {
            return (delta, None);
        }
        // §750
        let x = self.clean_box(&q.supscr, self.cur_style.sup());
        let y = self.clean_box(&q.nucleus, self.cur_style);
        let z = self.clean_box(&q.subscr, self.cur_style.sub());
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
            let Some(instr) = self.fonts.lig_kern(cur.f, Left::Char(cur.c), pc) else {
                return;
            };
            match instr {
                LigKern::Kern(k) => {
                    rest.push_front(MItem::Node(new_kern(k)));
                    return;
                }
                LigKern::Lig(op, rem) => {
                    let set_q = |q: &mut Noad, c: u16| {
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
                                p.nucleus = Field::MathChar { fam: pf, ch: rem };
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

    /// `make_scripts(q, delta)` (§756); appends to `hlist`.
    fn make_scripts(&mut self, q: &Noad, hlist: &mut List, delta: Scaled) {
        let (mut shift_up, mut shift_down) = if let Some(Node::Char { .. }) = hlist.first() {
            (0, 0)
        } else {
            let z = self.hpack_nat(hlist.clone());
            let t = if self.cur_style < Style::SCRIPT { 1 } else { 2 };
            (z.height - self.mathsy(18, t), z.depth + self.mathsy(19, t))
        };
        let s = self.cur_size;
        let x = if q.supscr == Field::Empty {
            // §757
            let mut x = self.clean_box(&q.subscr, self.cur_style.sub());
            x.width += self.p.script_space;
            shift_down = shift_down.max(self.mathsy(16, s));
            let clr = x.height - (self.math_x_height(s) * 4).abs() / 5;
            shift_down = shift_down.max(clr);
            x.shift = shift_down;
            x
        } else {
            // §758
            let mut x = self.clean_box(&q.supscr, self.cur_style.sup());
            x.width += self.p.script_space;
            let clr = if self.cur_style.0 % 2 == 1 {
                self.mathsy(15, s)
            } else if self.cur_style < Style::TEXT {
                self.mathsy(13, s)
            } else {
                self.mathsy(14, s)
            };
            shift_up = shift_up.max(clr);
            let clr = x.depth + self.math_x_height(s).abs() / 4;
            shift_up = shift_up.max(clr);
            if q.subscr == Field::Empty {
                x.shift = -shift_up;
                x
            } else {
                // §759
                let mut y = self.clean_box(&q.subscr, self.cur_style.sub());
                y.width += self.p.script_space;
                shift_down = shift_down.max(self.mathsy(17, s));
                let mut clr = 4 * self.default_rule_thickness()
                    - ((shift_up - x.depth) - (y.height - shift_down));
                if clr > 0 {
                    shift_down += clr;
                    clr = (self.math_x_height(s) * 4).abs() / 5 - (shift_up - x.depth);
                    if clr > 0 {
                        shift_up += clr;
                        shift_down -= clr;
                    }
                }
                x.shift = delta;
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
                NoadKind::Accent(fam, ch) => self.make_math_accent(&mut noad, fam, ch),
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
                        if let Some(i) = fc.i {
                            delta = i.italic;
                            let mut p = vec![Node::Char {
                                font: fc.f,
                                ch: fc.c,
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
                    self.make_scripts(&noad, &mut p, delta);
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
    };
    let h = ctx.mlist_to_hlist(mlist, style, penalties);
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
