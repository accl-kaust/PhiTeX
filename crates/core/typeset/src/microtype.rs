//! pdfTeX's character protrusion and font expansion (pdftex.web's
//! "HZ-algorithm" and margin kerning): the protrusion of a line's edge
//! characters (`char_pw`, `find_protchar_left`, `find_protchar_right`),
//! a character's stretch and shrink in its expanded fonts
//! (`char_stretch`, `char_shrink`, `kern_stretch`, `kern_shrink`), the
//! font a character is substituted with (`do_subst_font`), and `hpack`
//! with `cal_expand_ratio` and `subst_ex_font`.

use crate::arith::{Scaled, divide_scaled, ext_xn_over_d, round_xn_over_d};
use crate::fonts::{Expansion, FontId, Fonts, Left, LigKern};
use crate::glue::Order;
use crate::node::{BoxKind, KernKind, List, Node};
use crate::pack::{PackParams, Packed, Spec, hpack};

/// The character of a character or ligature node (`lig_char`).
#[must_use]
pub fn char_of(n: &Node) -> Option<(FontId, u16)> {
    match n {
        Node::Char { font, ch } => Some((*font, *ch)),
        Node::Lig(l) => Some((l.font, l.ch)),
        _ => None,
    }
}

/// `char_pw` of character `ch` of `font` at a side (`left_side` if
/// `left`).
#[must_use]
pub fn pw_of(fonts: &dyn Fonts, font: FontId, ch: u16, left: bool) -> Scaled {
    let c = if left {
        fonts.lp_code(font, ch)
    } else {
        fonts.rp_code(font, ch)
    };
    if c == 0 {
        return 0;
    }
    round_xn_over_d(fonts.quad(font), c, 1000)
}

/// `char_pw(p, side)`: node `n`'s protrusion, and its character (what
/// `last_leftmost_char` or `last_rightmost_char` becomes), if it is a
/// character or a ligature.
#[must_use]
pub fn char_pw(fonts: &dyn Fonts, n: Option<&Node>, left: bool) -> (Scaled, Option<(FontId, u16)>) {
    // XeTeX: a native word's first or last glyph, a glyph node's glyph
    // (its codes are by glyph)
    if let Some(Node::Native(w)) = n {
        let g = if left { w.glyphs.first() } else { w.glyphs.last() };
        return match g {
            Some(g) => (pw_of(fonts, w.font, g.gid, left), Some((w.font, g.gid))),
            None => (0, None),
        };
    }
    match n.and_then(char_of) {
        Some((f, c)) => (pw_of(fonts, f, c, left), Some((f, c))),
        None => (0, None),
    }
}

/// An hlist node with nothing in it and no size.
fn empty_hbox(n: &Node) -> bool {
    matches!(n, Node::Box(b) if b.kind == BoxKind::H && b.width == 0 && b.height == 0 && b.depth == 0 && b.list.is_empty())
}

/// An hlist node with a list (`type(l)=hlist_node` and `list_ptr(l)<>null`).
fn hlist_with_list(n: &Node) -> Option<&List> {
    match n {
        Node::Box(b) if b.kind == BoxKind::H && !b.list.is_empty() => Some(&b.list),
        _ => None,
    }
}

/// Whether glue is `zero_glue` (the shared spec of zero glue: glue of no
/// width, stretch or shrink whose spec is not its own, [`crate::node::GlueNode::own_spec`]:
/// `\hskip 0pt` is no `zero_glue`, and LaTeX's section number after
/// `\hskip\z@` in `\@hangfrom` does not protrude).
fn zero_glue(n: &Node) -> bool {
    matches!(n, Node::Glue(g) if !g.own_spec && g.spec.width == 0 && g.spec.stretch == 0 && g.spec.shrink == 0
        && g.spec.stretch_order == Order::Normal && g.spec.shrink_order == Order::Normal)
}

/// `cp_skipable`: what a line's margin passes over to its protruding
/// character.
#[must_use]
pub fn cp_skipable(n: &Node, xe: bool) -> bool {
    match n {
        Node::Ins(_) | Node::Mark(_) | Node::Adjust(_) | Node::Penalty(_) => true,
        // (a whatsit but a reference to an image or a form; XeTeX's
        // passes over none)
        Node::Language { .. } => !xe,
        Node::Whatsit(_, size) => !xe && size.is_none(),
        Node::Disc(d) => d.pre.is_empty() && d.post.is_empty() && d.replace.is_empty(),
        Node::Math(_, w) => *w == 0,
        Node::Kern(w, k) => *w == 0 || *k == KernKind::Normal,
        Node::Glue(_) => zero_glue(n),
        Node::Box(_) => empty_hbox(n),
        _ => false,
    }
}

/// `find_protchar_left(l, d)`: from `list[at]` on, into hlists, the
/// first node not skipable; with `d`, the discardables at a break
/// passed first.
#[must_use]
pub fn find_protchar_left(list: &[Node], at: usize, d: bool, xe: bool) -> Option<&Node> {
    let mut cl: &[Node] = list;
    let mut i = at;
    if i >= cl.len() {
        return None;
    }
    if i + 1 < cl.len() && empty_hbox(&cl[i]) {
        // (a paragraph's start with \parindent=0pt)
        i += 1;
    } else if d {
        while i + 1 < cl.len() && cl[i].is_discardable() {
            i += 1;
        }
    }
    let mut stack: Vec<(&[Node], usize)> = Vec::new();
    let mut run = true;
    loop {
        let t = (cl.as_ptr(), i);
        while run && let Some(l) = hlist_with_list(&cl[i]) {
            stack.push((cl, i));
            cl = l;
            i = 0;
        }
        while run && cp_skipable(&cl[i], xe) {
            while i + 1 >= cl.len()
                && let Some((pl, pi)) = stack.pop()
            {
                // (that box is not visited again)
                cl = pl;
                i = pi;
            }
            if i + 1 < cl.len() {
                i += 1;
            } else if stack.is_empty() {
                run = false;
            }
        }
        if t == (cl.as_ptr(), i) {
            break;
        }
    }
    Some(&cl[i])
}

/// `find_protchar_right(l, r)`: from `list[r]` back to `list[l]`, into
/// hlists, the first node not skipable.
#[must_use]
pub fn find_protchar_right(list: &[Node], l: usize, r: usize, xe: bool) -> Option<&Node> {
    if r >= list.len() {
        return None;
    }
    let (mut cl, mut l, mut r): (&[Node], usize, usize) = (list, l.min(r), r);
    let mut stack: Vec<(&[Node], usize, usize)> = Vec::new();
    let mut run = true;
    loop {
        let t = (cl.as_ptr(), r);
        while run && let Some(b) = hlist_with_list(&cl[r]) {
            stack.push((cl, l, r));
            cl = b;
            l = 0;
            r = b.len() - 1;
        }
        while run && cp_skipable(&cl[r], xe) {
            while r == l
                && let Some((pl, ll, rr)) = stack.pop()
            {
                cl = pl;
                l = ll;
                r = rr;
            }
            if r != l {
                r -= 1;
            } else if stack.is_empty() {
                run = false;
            }
        }
        if t == (cl.as_ptr(), r) {
            break;
        }
    }
    Some(&cl[r])
}

/// `fix_expand_value(f, e)`: the multiple of the step nearest to `e`,
/// within the limits.
#[must_use]
pub fn fix_expand_value(x: &Expansion, e: i32) -> i32 {
    if e == 0 {
        return 0;
    }
    let (mut e, neg, max) = if e < 0 {
        (-e, true, x.shrink.map_or(0, |s| -s.1))
    } else {
        (e, false, x.stretch.map_or(0, |s| s.1))
    };
    if e > max {
        e = max;
    } else if x.step > 0 && e % x.step > 0 {
        e = x.step * round_xn_over_d(e, 1, x.step);
    }
    if neg { -e } else { e }
}

/// `expand_font(f, e)`: font `f` expanded by about `e` thousandths
/// (`fix_expand_value`), made where it is not yet; `f` for none.
#[must_use]
pub fn expand_font(fonts: &dyn Fonts, f: FontId, x: &Expansion, e: i32) -> FontId {
    if e == 0 {
        return f;
    }
    let e = fix_expand_value(x, e);
    if e == 0 {
        return f;
    }
    fonts.expand_font(f, e)
}

/// `do_subst_font` of character `ch` of `font` at `ex_ratio` (thousandths
/// of the font's limit, signed): the font it is set in.
#[must_use]
pub fn subst_font(fonts: &dyn Fonts, font: FontId, ch: u16, ex_ratio: i32) -> FontId {
    let ef = fonts.ef_code(font, ch);
    if ef == 0 {
        return font;
    }
    let Some(x) = fonts.expansion(font) else {
        return font;
    };
    match (x.stretch, x.shrink) {
        (Some((_, lim)), _) if ex_ratio > 0 => expand_font(
            fonts,
            font,
            &x,
            ext_xn_over_d(ex_ratio * ef, lim, 1_000_000),
        ),
        (_, Some((_, lim))) if ex_ratio < 0 => expand_font(
            fonts,
            font,
            &x,
            ext_xn_over_d(ex_ratio * ef, -lim, 1_000_000),
        ),
        _ => font,
    }
}

/// `do_subst_font(p, ex_ratio)` on a node: a character, a ligature (its
/// characters too), a discretionary's pre- and post-break characters.
pub fn subst_node(fonts: &dyn Fonts, n: &mut Node, ex_ratio: i32) {
    match n {
        Node::Char { font, ch } => *font = subst_font(fonts, *font, *ch, ex_ratio),
        Node::Lig(l) => l.font = subst_font(fonts, l.font, l.ch, ex_ratio),
        Node::Disc(d) => {
            for r in d.pre.iter_mut().chain(d.post.iter_mut()) {
                if matches!(r, Node::Char { .. } | Node::Lig(_)) {
                    subst_node(fonts, r, ex_ratio);
                }
            }
        }
        _ => {}
    }
}

/// `char_stretch(f, c)`.
#[must_use]
pub fn char_stretch(fonts: &dyn Fonts, f: FontId, c: u16) -> Scaled {
    let Some((k, _)) = fonts.expansion(f).and_then(|x| x.stretch) else {
        return 0;
    };
    let ef = fonts.ef_code(f, c);
    if ef <= 0 {
        return 0;
    }
    let dw = fonts.char_width(k, c) - fonts.char_width(f, c);
    if dw > 0 {
        round_xn_over_d(dw, ef, 1000)
    } else {
        0
    }
}

/// `char_shrink(f, c)`.
#[must_use]
pub fn char_shrink(fonts: &dyn Fonts, f: FontId, c: u16) -> Scaled {
    let Some((k, _)) = fonts.expansion(f).and_then(|x| x.shrink) else {
        return 0;
    };
    let ef = fonts.ef_code(f, c);
    if ef <= 0 {
        return 0;
    }
    let dw = fonts.char_width(f, c) - fonts.char_width(k, c);
    if dw > 0 {
        round_xn_over_d(dw, ef, 1000)
    } else {
        0
    }
}

/// `get_kern(f, lc, rc)`: the kern between two characters of a font.
#[must_use]
pub fn get_kern(fonts: &dyn Fonts, f: FontId, lc: u16, rc: u16) -> Scaled {
    match fonts.lig_kern(f, Left::Char(lc), rc) {
        Some(LigKern::Kern(w)) => w,
        _ => 0,
    }
}

/// `kern_stretch(p)` (`stretch`) or `kern_shrink(p)`: a font kern `w`
/// between characters `l` (`prev_char_p`, just before it) and `r` (just
/// after it).
#[must_use]
pub fn kern_fw(
    fonts: &dyn Fonts,
    l: Option<(FontId, u16)>,
    w: Scaled,
    r: Option<(FontId, u16)>,
    stretch: bool,
) -> Scaled {
    let (Some((fl, cl)), Some((fr, cr))) = (l, r) else {
        return 0;
    };
    if fl != fr {
        return 0;
    }
    let Some(x) = fonts.expansion(fl) else {
        return 0;
    };
    let lim = if stretch { x.stretch } else { x.shrink };
    let Some((k, _)) = lim else { return 0 };
    let d = get_kern(fonts, k, cl, cr);
    let ef = fonts.ef_code(fl, cl);
    if stretch {
        round_xn_over_d(d - w, ef, 1000)
    } else {
        round_xn_over_d(w - d, ef, 1000)
    }
}

/// The nodes of an hlist as pdfTeX's list has them: a discretionary's
/// replacement after it (`replace_count`).
fn flat(list: &[Node]) -> Vec<&Node> {
    let mut out = Vec::with_capacity(list.len());
    for n in list {
        out.push(n);
        if let Node::Disc(d) = n {
            out.extend(d.replace.iter());
        }
    }
    out
}

/// `hpack(p, w, cal_expand_ratio)` (`post_line_break` with
/// `\pdfadjustspacing>0`): the line packed to `width`; where its glue
/// alone would set it, it is set so; where the fonts' expansion can make
/// up the difference instead (`font_expand_ratio`, the stretch or shrink
/// of its characters and font kerns), its characters are set in their
/// expanded fonts (`subst_ex_font`) and it is packed again.
#[must_use]
pub fn hpack_expand(
    mut list: List,
    width: Scaled,
    fonts: &dyn Fonts,
    params: &PackParams,
    adjust: bool,
) -> Packed {
    let adjustments = if adjust {
        crate::pack::take_adjustments(&mut list)
    } else {
        List::new()
    };
    // the first pass (`cal_expand_ratio`)
    let (mut font_stretch, mut font_shrink) = (0, 0);
    {
        let fl = flat(&list);
        let mut prev: Option<usize> = None;
        for (i, n) in fl.iter().enumerate() {
            match n {
                Node::Char { .. } | Node::Lig(_) => {
                    let (f, c) = char_of(n).unwrap_or_default();
                    prev = Some(i);
                    font_stretch += char_stretch(fonts, f, c);
                    font_shrink += char_shrink(fonts, f, c);
                }
                Node::Kern(w, KernKind::Margin { left, font, ch }) => {
                    let k = subst_font(fonts, *font, *ch, 1000);
                    if k != *font {
                        font_stretch = font_stretch - w - pw_of(fonts, k, *ch, *left);
                    }
                    let k = subst_font(fonts, *font, *ch, -1000);
                    if k != *font {
                        font_shrink = font_shrink - w - pw_of(fonts, k, *ch, *left);
                    }
                }
                Node::Kern(w, KernKind::Normal) => {
                    let l = prev.filter(|&p| p + 1 == i).and_then(|p| char_of(fl[p]));
                    let r = fl.get(i + 1).and_then(|r| char_of(r));
                    font_stretch += kern_fw(fonts, l, *w, r, true);
                    font_shrink += kern_fw(fonts, l, *w, r, false);
                }
                _ => {}
            }
        }
    }
    let m = crate::pack::measure(&list, fonts);
    let x = width - m.natural;
    let ratio = if x > 0 && m.stretch_order == Order::Normal && font_stretch > 0 {
        Some(divide_scaled(x, font_stretch, 3))
    } else if x < 0 && m.shrink_order == Order::Normal && font_shrink > 0 {
        Some(divide_scaled(x, font_shrink, 3))
    } else {
        None
    };
    let mut packed = match ratio {
        None => hpack(list, Spec::Exactly(width), fonts, params, false),
        // (the first pass's box: its glue not set, no report)
        Some(0) => Packed {
            node: crate::node::BoxNode {
                kind: BoxKind::H,
                width,
                height: m.height,
                depth: m.depth,
                list,
                ..crate::node::BoxNode::default()
            },
            badness: 0,
            report: None,
            adjustments: List::new(),
        },
        Some(r) => {
            subst_ex_font(&mut list, r.clamp(-1000, 1000), fonts);
            hpack(list, Spec::Exactly(width), fonts, params, false)
        }
    };
    packed.adjustments = adjustments;
    packed
}

/// A node of an hlist in pdfTeX's order: a top-level node, or the
/// `j`th of a discretionary's replacement.
#[derive(Clone, Copy)]
struct At(usize, Option<usize>);

fn node_at(list: &List, a: At) -> Option<&Node> {
    match (list.get(a.0)?, a.1) {
        (n, None) => Some(n),
        (Node::Disc(d), Some(j)) => d.replace.get(j),
        _ => None,
    }
}

fn node_at_mut(list: &mut List, a: At) -> Option<&mut Node> {
    match (list.get_mut(a.0)?, a.1) {
        (n, None) => Some(n),
        (Node::Disc(d), Some(j)) => d.replace.get_mut(j),
        _ => None,
    }
}

/// `hpack(p, w, subst_ex_font)`'s substitutions: every character of the
/// line in its font expanded at `ratio`, its margin kerns and font kerns
/// as the substituted characters give them.
fn subst_ex_font(list: &mut List, ratio: i32, fonts: &dyn Fonts) {
    let mut order = Vec::with_capacity(list.len());
    for (i, n) in list.iter().enumerate() {
        order.push(At(i, None));
        if let Node::Disc(d) = n {
            order.extend((0..d.replace.len()).map(|j| At(i, Some(j))));
        }
    }
    // (`prev_char_p`: the last character, substituted, and its place)
    let mut prev: Option<(FontId, u16, usize)> = None;
    for (k, &a) in order.iter().enumerate() {
        let next = order
            .get(k + 1)
            .and_then(|&b| node_at(list, b))
            .and_then(char_of);
        let Some(n) = node_at_mut(list, a) else {
            continue;
        };
        match n {
            Node::Char { .. } | Node::Lig(_) | Node::Disc(_) => {
                subst_node(fonts, n, ratio);
                if let Some((f, c)) = char_of(n) {
                    prev = Some((f, c, k));
                }
            }
            Node::Kern(w, KernKind::Margin { left, font, ch }) => {
                *font = subst_font(fonts, *font, *ch, ratio);
                *w = -pw_of(fonts, *font, *ch, *left);
            }
            Node::Kern(w, KernKind::Normal) => {
                let l = prev.filter(|p| p.2 + 1 == k).map(|p| (p.0, p.1));
                if kern_fw(fonts, l, *w, next, ratio > 0) != 0
                    && let (Some((fl, cl)), Some((_, cr))) = (l, next)
                {
                    *w = get_kern(fonts, fl, cl, cr);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_expansion_to_the_step() {
        let x = Expansion {
            stretch: Some((1, 20)),
            shrink: Some((2, -20)),
            step: 5,
        };
        assert_eq!(fix_expand_value(&x, 12), 10);
        assert_eq!(fix_expand_value(&x, 13), 15);
        assert_eq!(fix_expand_value(&x, -30), -20);
        assert_eq!(fix_expand_value(&x, 0), 0);
    }

    #[test]
    fn skips_to_the_protruding_character() {
        let list = vec![
            Node::Penalty(0),
            Node::Box(Box::new(crate::node::BoxNode {
                list: vec![Node::Mark(1), Node::Char { font: 1, ch: 65 }],
                ..crate::node::BoxNode::default()
            })),
            Node::Char { font: 1, ch: 66 },
            Node::Kern(0, KernKind::Explicit),
        ];
        assert_eq!(
            find_protchar_left(&list, 0, true, false).and_then(char_of),
            Some((1, 65))
        );
        assert_eq!(
            find_protchar_right(&list, 0, 3, false).and_then(char_of),
            Some((1, 66))
        );
    }
}
