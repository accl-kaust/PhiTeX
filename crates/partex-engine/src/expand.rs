//! pdfTeX's font expansion: how much a line's characters (and the kerns
//! between them) can stretch or shrink by using expanded versions of
//! their fonts, and packing a line with the fonts that make it fit
//! (`hpack` with `cal_expand_ratio`, then `subst_ex_font`).
//!
//! Expanded fonts are real fonts, loaded the first time a line needs one
//! ([`ExpandEnv::get_expand_font`]), as pdfTeX loads them: fonts are
//! numbered in the order they are loaded, and those numbers name the
//! fonts in PDF output.

use alloc::vec::Vec;

use crate::Scaled;
use crate::node::Order;
use crate::node::{FontId, Node, push_char};
use crate::pack::{self, Fonts, Packed, Spec};
use crate::scaled::{ext_xn_over_d, round_xn_over_d};

/// What pdfTeX keeps about a font's expansion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    /// `pdf_font_step`: 0 if the font is not expandable.
    pub step: i32,
    /// `pdf_font_stretch`, `pdf_font_shrink`: the font expanded by its
    /// stretch limit and shrunk by its shrink limit.
    pub stretch: Option<FontId>,
    pub shrink: Option<FontId>,
    /// `pdf_font_expand_ratio`: how much this font is expanded.
    pub ratio: i32,
}

/// What font expansion needs besides the fonts.
pub trait ExpandEnv: Fonts {
    fn expansion(&self, f: FontId) -> Expansion;
    /// `\efcode` of character `c` of `f`.
    fn ef_code(&self, f: FontId, c: u8) -> i32;
    /// `\lpcode` (`left`) or `\rpcode` of character `c` of `f`.
    fn margin_code(&self, f: FontId, c: u8, left: bool) -> i32;
    /// pdfTeX's `get_expand_font`: font `f` expanded by `e` (a nonzero
    /// multiple of its step), loaded now if it is new.
    fn get_expand_font(&mut self, f: FontId, e: i32) -> FontId;
}

fn ratio_of(env: &impl ExpandEnv, f: Option<FontId>) -> i32 {
    f.map_or(0, |k| env.expansion(k).ratio)
}

/// pdfTeX's `fix_expand_value`: the multiple of `f`'s step nearest to
/// `e`, within its limits.
fn fix_expand_value(env: &impl ExpandEnv, f: FontId, e: i32) -> i32 {
    if e == 0 {
        return 0;
    }
    let x = env.expansion(f);
    let (neg, mut e, max) = if e < 0 {
        (true, -e, -ratio_of(env, x.shrink))
    } else {
        (false, e, ratio_of(env, x.stretch))
    };
    if e > max {
        e = max;
    } else if x.step > 0 && e % x.step > 0 {
        e = x.step * round_xn_over_d(e, 1, x.step);
    }
    if neg { -e } else { e }
}

/// pdfTeX's `expand_font`: `f` expanded by about `e`.
fn expand_font(env: &mut impl ExpandEnv, f: FontId, e: i32) -> FontId {
    if e == 0 {
        return f;
    }
    let e = fix_expand_value(env, f, e);
    if e == 0 {
        return f;
    }
    env.get_expand_font(f, e)
}

/// pdfTeX's `do_subst_font` for character `c` of `f`: its font when the
/// line is expanded by `ratio` thousandths of the fonts' limits.
pub fn subst_font(env: &mut impl ExpandEnv, f: FontId, c: u8, ratio: i32) -> FontId {
    let ef = env.ef_code(f, c);
    if ef == 0 {
        return f;
    }
    let x = env.expansion(f);
    match (x.stretch, x.shrink) {
        (Some(k), _) if ratio > 0 => {
            let limit = env.expansion(k).ratio;
            expand_font(env, f, ext_xn_over_d(ratio * ef, limit, 1_000_000))
        }
        (_, Some(k)) if ratio < 0 => {
            let limit = -env.expansion(k).ratio;
            expand_font(env, f, ext_xn_over_d(ratio * ef, limit, 1_000_000))
        }
        _ => f,
    }
}

/// pdfTeX's `char_stretch`: how much character `c` of `f` can stretch.
pub fn char_stretch(env: &impl ExpandEnv, f: FontId, c: u8) -> Scaled {
    let ef = env.ef_code(f, c);
    match env.expansion(f).stretch {
        Some(k) if ef > 0 => {
            let dw = env.font(k).width(c) - env.font(f).width(c);
            if dw > 0 {
                round_xn_over_d(dw, ef, 1000)
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// pdfTeX's `char_shrink`.
pub fn char_shrink(env: &impl ExpandEnv, f: FontId, c: u8) -> Scaled {
    let ef = env.ef_code(f, c);
    match env.expansion(f).shrink {
        Some(k) if ef > 0 => {
            let dw = env.font(f).width(c) - env.font(k).width(c);
            if dw > 0 {
                round_xn_over_d(dw, ef, 1000)
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// pdfTeX's `kern_stretch` (`stretch`) or `kern_shrink` of a kern of
/// width `w` between characters `l` and `r` (as tex.web links them: none
/// unless they are right next to it).
pub fn kern_change(
    env: &impl ExpandEnv,
    l: Option<(FontId, u8)>,
    w: Scaled,
    r: Option<(FontId, u8)>,
    stretch: bool,
) -> Scaled {
    let (Some((lf, lc)), Some((rf, rc))) = (l, r) else {
        return 0;
    };
    let x = env.expansion(lf);
    let k = if stretch { x.stretch } else { x.shrink };
    match k {
        Some(k) if lf == rf => {
            let d = env.font(k).kern_between(lc, rc);
            let dw = if stretch { d - w } else { w - d };
            round_xn_over_d(dw, env.ef_code(lf, lc), 1000)
        }
        _ => 0,
    }
}

/// pdfTeX's `char_pw` for character `c` of `f`.
pub fn char_pw(env: &impl ExpandEnv, f: FontId, c: u8, left: bool) -> Scaled {
    crate::margin::char_pw(env.margin_code(f, c, left), env.font(f).param(6))
}

/// One node of a line as tex.web links it, for `hpack`'s walks: the
/// characters of a run one by one, a discretionary followed by the nodes
/// it replaces.
#[derive(Clone, Copy, Debug)]
enum Item {
    Char(FontId, u8),
    /// A character of a discretionary's pre-break or post-break text,
    /// which `hpack` substitutes at the discretionary.
    DiscChar(FontId, u8),
    Kern(Scaled),
    Margin {
        width: Scaled,
        left: bool,
        font: FontId,
        ch: u8,
    },
    Other,
}

fn push_items(items: &mut Vec<Item>, n: &Node) {
    match n {
        Node::Glyphs(g) => items.extend(g.chars().iter().map(|&c| Item::Char(g.font, c))),
        Node::Ligature(l) => items.push(Item::Char(l.font, l.ch)),
        Node::Kern { width, subtype: 0 } => items.push(Item::Kern(*width)),
        Node::MarginKern {
            width,
            left,
            font,
            ch,
        } => items.push(Item::Margin {
            width: *width,
            left: *left,
            font: *font,
            ch: *ch,
        }),
        Node::Disc(d) => {
            for n in d.pre.iter().chain(&d.post) {
                match n {
                    Node::Glyphs(g) => {
                        items.extend(g.chars().iter().map(|&c| Item::DiscChar(g.font, c)));
                    }
                    Node::Ligature(l) => items.push(Item::DiscChar(l.font, l.ch)),
                    _ => {}
                }
            }
            items.push(Item::Other);
            for n in &d.replace {
                push_items(items, n);
            }
        }
        _ => items.push(Item::Other),
    }
}

/// The characters before and after an item.
type Near = (Option<(FontId, u8)>, Option<(FontId, u8)>);

/// The characters next to each item (on the main list), as tex.web's
/// `prev_char_p` and `link` see them for a kern.
fn neighbours(items: &[Item]) -> Vec<Near> {
    let main: Vec<usize> = (0..items.len())
        .filter(|&i| !matches!(items[i], Item::DiscChar(..)))
        .collect();
    let char_at = |i: usize| match items[i] {
        Item::Char(f, c) => Some((f, c)),
        _ => None,
    };
    let mut out = alloc::vec![(None, None); items.len()];
    for (k, &i) in main.iter().enumerate() {
        let prev = k.checked_sub(1).and_then(|k| char_at(main[k]));
        let next = main.get(k + 1).and_then(|&j| char_at(j));
        out[i] = (prev, next);
    }
    out
}

/// §649 with pdfTeX's `cal_expand_ratio`, then `subst_ex_font`: pack a
/// line to width `w`, expanding its fonts if glue alone would have to
/// stretch or shrink (finitely) and its characters can.
pub fn hpack_line(
    mut list: Vec<Node>,
    w: Scaled,
    params: &pack::Params,
    env: &mut impl ExpandEnv,
    adjust: Option<&mut Vec<Node>>,
) -> Packed {
    let mut items = Vec::with_capacity(list.len() * 4);
    for n in &list {
        push_items(&mut items, n);
    }
    let near = neighbours(&items);
    // the cal_expand_ratio pass
    let (mut font_stretch, mut font_shrink) = (0, 0);
    for (i, item) in items.iter().enumerate() {
        match *item {
            Item::Char(f, c) => {
                font_stretch += char_stretch(env, f, c);
                font_shrink += char_shrink(env, f, c);
            }
            Item::Kern(width) => {
                let (l, r) = near[i];
                font_stretch += kern_change(env, l, width, r, true);
                font_shrink += kern_change(env, l, width, r, false);
            }
            Item::Margin {
                width,
                left,
                font,
                ch,
            } => {
                let k = subst_font(env, font, ch, 1000);
                if k != font {
                    font_stretch -= width + char_pw(env, k, ch, left);
                }
                let k = subst_font(env, font, ch, -1000);
                if k != font {
                    font_shrink -= width + char_pw(env, k, ch, left);
                }
            }
            Item::DiscChar(..) | Item::Other => {}
        }
    }
    let (natural, stretch, shrink) = pack::natural_width(&list, &*env);
    let x = w - natural;
    let ratio = if x > 0 && pack::order(&stretch) == Order::Normal && font_stretch > 0 {
        crate::scaled::divide_scaled(x, font_stretch, 3).map_or(0, |(q, _)| q)
    } else if x < 0 && pack::order(&shrink) == Order::Normal && font_shrink > 0 {
        crate::scaled::divide_scaled(x, font_shrink, 3).map_or(0, |(q, _)| q)
    } else {
        0
    };
    if ratio != 0 {
        list = substitute(list, &items, &near, ratio.clamp(-1000, 1000), env);
    }
    pack::hpack(list, Spec::Exactly(w), params, &*env, adjust)
}

/// The `subst_ex_font` pass: the fonts of the line's characters, and the
/// kerns and margin kerns that change with them, for expansion `ratio`.
fn substitute(
    list: Vec<Node>,
    items: &[Item],
    near: &[Near],
    ratio: i32,
    env: &mut impl ExpandEnv,
) -> Vec<Node> {
    // first what changes, in tex.web's order (fonts load in it)
    let mut fonts = Vec::with_capacity(items.len());
    let mut widths = Vec::new();
    let mut prev_char: Option<(FontId, u8)> = None;
    for (i, item) in items.iter().enumerate() {
        match *item {
            Item::Char(f, c) => {
                let k = subst_font(env, f, c, ratio);
                fonts.push(k);
                prev_char = Some((k, c));
            }
            Item::DiscChar(f, c) => fonts.push(subst_font(env, f, c, ratio)),
            Item::Kern(width) => {
                // (the character before has its new font by now)
                let (before, r) = near[i];
                let l = before.and(prev_char);
                let k = kern_change(env, l, width, r, ratio > 0);
                widths.push(match (k, l, r) {
                    (0, _, _) | (_, None, _) | (_, _, None) => width,
                    (_, Some((lf, lc)), Some((_, rc))) => env.font(lf).kern_between(lc, rc),
                });
            }
            Item::Margin { left, font, ch, .. } => {
                let k = subst_font(env, font, ch, ratio);
                fonts.push(k);
                widths.push(-char_pw(env, k, ch, left));
            }
            Item::Other => {}
        }
    }
    let mut fonts = fonts.into_iter();
    let mut widths = widths.into_iter();
    let mut out = Vec::with_capacity(list.len() + 4);
    for n in list {
        apply(n, &mut out, &mut fonts, &mut widths);
    }
    out
}

/// The next substituted font (`f` if there is none).
fn next_font(fonts: &mut impl Iterator<Item = FontId>, f: FontId) -> FontId {
    fonts.next().unwrap_or(f)
}

/// Put the characters of `part` on `out` with their substituted fonts.
fn apply_text(part: Vec<Node>, out: &mut Vec<Node>, fonts: &mut impl Iterator<Item = FontId>) {
    for n in part {
        match n {
            Node::Glyphs(g) => {
                for &c in g.chars() {
                    push_char(out, next_font(fonts, g.font), c);
                }
            }
            Node::Ligature(mut l) => {
                l.font = next_font(fonts, l.font);
                out.push(Node::Ligature(l));
            }
            n => out.push(n),
        }
    }
}

/// Put node `n`, with the changes `substitute` found for it, on `out`.
fn apply(
    n: Node,
    out: &mut Vec<Node>,
    fonts: &mut impl Iterator<Item = FontId>,
    widths: &mut impl Iterator<Item = Scaled>,
) {
    match n {
        Node::Glyphs(_) | Node::Ligature(_) => apply_text(alloc::vec![n], out, fonts),
        Node::Kern { width, subtype: 0 } => out.push(Node::Kern {
            width: widths.next().unwrap_or(width),
            subtype: 0,
        }),
        Node::MarginKern {
            width,
            left,
            ch,
            font,
        } => out.push(Node::MarginKern {
            font: next_font(fonts, font),
            width: widths.next().unwrap_or(width),
            left,
            ch,
        }),
        Node::Disc(mut d) => {
            let (pre, post) = (core::mem::take(&mut d.pre), core::mem::take(&mut d.post));
            apply_text(pre, &mut d.pre, fonts);
            apply_text(post, &mut d.post, fonts);
            let mut replace = Vec::with_capacity(d.replace.len());
            for n in core::mem::take(&mut d.replace) {
                apply(n, &mut replace, fonts, widths);
            }
            d.replace = replace;
            out.push(Node::Disc(d));
        }
        n => out.push(n),
    }
}
