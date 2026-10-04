//! pdfTeX's character protrusion: finding the characters at the margins
//! of a line (`find_protchar_left`, `find_protchar_right`) and how far
//! they protrude (`char_pw`).
//!
//! pdfTeX walks tex.web's linked lists, where the nodes a discretionary
//! replaces follow it; here they are inside it, so the walks step through
//! positions ([`At`]) that visit them in tex.web's order.

use alloc::vec::Vec;

use crate::Scaled;
use crate::node::{FontId, Node};
use crate::scaled::round_xn_over_d;

/// pdfTeX's `auto_kern` kern subtype.
const AUTO_KERN: u8 = 3;
/// §155: the `normal` kern subtype.
const KERN_NORMAL: u8 = 0;

/// A node of a list in tex.web's order: node `i`, or the `j`th node that
/// the discretionary at `i` replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub i: usize,
    pub j: Option<usize>,
}

impl At {
    #[must_use]
    pub fn node(i: usize) -> At {
        At { i, j: None }
    }
}

fn replaced(list: &[Node], i: usize) -> &[Node] {
    match &list[i] {
        Node::Disc(d) => &d.replace,
        _ => &[],
    }
}

/// The node at `a`.
#[must_use]
pub fn get(list: &[Node], a: At) -> &Node {
    match a.j {
        None => &list[a.i],
        Some(j) => &replaced(list, a.i)[j],
    }
}

/// tex.web's `link`: the position after `a`.
#[must_use]
pub fn next(list: &[Node], a: At) -> Option<At> {
    let n = replaced(list, a.i).len();
    let j = a.j.map_or(0, |j| j + 1);
    if j < n {
        Some(At { i: a.i, j: Some(j) })
    } else {
        (a.i + 1 < list.len()).then(|| At::node(a.i + 1))
    }
}

/// pdfTeX's `prev_rightmost`: the position before `a`.
#[must_use]
pub fn prev(list: &[Node], a: At) -> Option<At> {
    match a.j {
        Some(0) => Some(At::node(a.i)),
        Some(j) => Some(At {
            i: a.i,
            j: Some(j - 1),
        }),
        None => {
            let i = a.i.checked_sub(1)?;
            let n = replaced(list, i).len();
            Some(At {
                i,
                j: n.checked_sub(1),
            })
        }
    }
}

/// The last position of `list`.
#[must_use]
pub fn last(list: &[Node]) -> Option<At> {
    prev(list, At::node(list.len()))
}

/// pdfTeX's `cp_skipable`: nodes the margin searches pass over.
#[must_use]
pub fn cp_skipable(n: &Node) -> bool {
    match n {
        Node::Ins(_) | Node::Mark(_) | Node::Adjust(_) | Node::Penalty(_) => true,
        // (except references to images and forms)
        Node::Whatsit(w) => w.ref_dims().is_none(),
        Node::Disc(d) => d.pre.is_empty() && d.post.is_empty() && d.replace.is_empty(),
        Node::Math { width, .. } => *width == 0,
        Node::Kern { width, subtype, .. } => {
            *width == 0 || *subtype == KERN_NORMAL || *subtype == AUTO_KERN
        }
        Node::Glue { spec, .. } => spec.shared_zero,
        Node::Leaders(l) => l.spec.shared_zero,
        Node::Box(_) => empty_hbox(n),
        _ => false,
    }
}

/// An hlist node with no dimensions and no list (as `\parindent=0pt`
/// makes).
fn empty_hbox(n: &Node) -> bool {
    matches!(n, Node::Box(b) if !b.vertical && b.width == 0 && b.height == 0
        && b.depth == 0 && b.list.is_empty())
}

/// An hlist node with a list, which the searches look into.
fn hlist_with_list(n: &Node) -> Option<&[Node]> {
    match n {
        Node::Box(b) if !b.vertical && !b.list.is_empty() => Some(&b.list),
        _ => None,
    }
}

/// tex.web's `non_discardable` (§148), or a character.
fn char_or_non_discardable(n: &Node) -> bool {
    !matches!(
        n,
        Node::Glue { .. }
            | Node::Leaders(_)
            | Node::Kern { .. }
            | Node::Penalty(_)
            | Node::Math { .. }
            | Node::Unset(_)
            | Node::MarginKern { .. }
    )
}

/// The character a search stopped at, if it stopped at one: the first
/// (`left`) or last character of a run, or a ligature's.
#[must_use]
pub fn char_at(n: &Node, left: bool) -> Option<(FontId, u8)> {
    match n {
        Node::Glyphs(g) => {
            let cs = g.chars();
            let c = if left { cs.first() } else { cs.last() };
            c.map(|&c| (g.font, c))
        }
        Node::Ligature(l) => Some((l.font, l.ch)),
        _ => None,
    }
}

/// pdfTeX's `find_protchar_left` from `start` in `list` (skipping the
/// discardable items a break leaves if `discardables`): the character
/// that protrudes into the left margin, if the search ends at one.
#[must_use]
pub fn find_protchar_left(list: &[Node], start: At, discardables: bool) -> Option<(FontId, u8)> {
    char_at(protchar_left(list, start, discardables), true)
}

/// The node `find_protchar_left` ends at.
#[must_use]
pub fn protchar_left(list: &[Node], start: At, discardables: bool) -> &Node {
    let mut l = start;
    if let Some(n) = next(list, l).filter(|_| empty_hbox(get(list, l))) {
        l = n; // for a paragraph start with \parindent=0pt
    } else if discardables {
        // (standard discardables at a line break, The TeXbook p. 95)
        while let Some(n) = next(list, l) {
            if char_or_non_discardable(get(list, l)) {
                break;
            }
            l = n;
        }
    }
    let mut cur = list;
    let mut stack: Vec<(&[Node], At)> = Vec::new();
    let mut run = true;
    loop {
        let t = (cur.as_ptr(), l);
        while run && let Some(inner) = hlist_with_list(get(cur, l)) {
            stack.push((cur, l));
            cur = inner;
            l = At::node(0);
        }
        while run && cp_skipable(get(cur, l)) {
            while next(cur, l).is_none()
                && let Some((up, at)) = stack.pop()
            {
                (cur, l) = (up, at); // don't visit this node again
            }
            if let Some(n) = next(cur, l) {
                l = n;
            } else if stack.is_empty() {
                run = false;
            }
        }
        if t == (cur.as_ptr(), l) {
            break;
        }
    }
    get(cur, l)
}

/// pdfTeX's `find_protchar_right`: from `r` back to `l` in `list`, the
/// character that protrudes into the right margin, if the search ends at
/// one.
#[must_use]
pub fn find_protchar_right(list: &[Node], l: At, r: Option<At>) -> Option<(FontId, u8)> {
    protchar_right(list, l, r).and_then(|n| char_at(n, false))
}

/// The node `find_protchar_right` ends at.
#[must_use]
pub fn protchar_right(list: &[Node], l: At, r: Option<At>) -> Option<&Node> {
    let (mut l, mut r) = (l, r?);
    let mut cur = list;
    let mut stack: Vec<(&[Node], At, At)> = Vec::new();
    let mut run = true;
    loop {
        let t = (cur.as_ptr(), r);
        while run && let Some(inner) = hlist_with_list(get(cur, r)) {
            stack.push((cur, l, r));
            cur = inner;
            l = At::node(0);
            r = last(inner)?;
        }
        while run && cp_skipable(get(cur, r)) {
            while r == l
                && let Some((up, ul, ur)) = stack.pop()
            {
                (cur, l, r) = (up, ul, ur); // don't visit this node again
            }
            if r != l {
                r = prev(cur, r)?;
            } else if stack.is_empty() {
                run = false;
            }
        }
        if t == (cur.as_ptr(), r) {
            break;
        }
    }
    Some(get(cur, r))
}

/// pdfTeX's `char_pw`: how far character `c` of font `f` protrudes,
/// given its `\lpcode` or `\rpcode` and the font's quad.
#[must_use]
pub fn char_pw(code: i32, quad: Scaled) -> Scaled {
    if code == 0 {
        0
    } else {
        round_xn_over_d(quad, code, 1000)
    }
}
