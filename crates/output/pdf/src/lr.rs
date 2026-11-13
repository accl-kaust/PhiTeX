//! e-TeX's TeX--XeT over [`phitex_typeset`] nodes: a right-to-left
//! segment of an hlist reversed as the output reaches it. The writer
//! (`crate::ship`) and the TeX side's walk for `\\pdfsavepos` take the same
//! positions from it.

use std::collections::VecDeque;

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::{GlueKind, GlueNode, KernKind, MathKind, Node};

/// An item of an hlist being output with TeX--XeT: a node, e-TeX's edge
/// node (a change of direction: `cur_h` moves by `width`, the new left
/// edge is `dist` beyond), or leaders glue whose width `reverse` has
/// fixed.
pub enum Lr {
    /// A node.
    N(Node),
    /// An edge node.
    Edge {
        /// The direction after it.
        dir: u8,
        /// How far `cur_h` moves.
        width: Scaled,
        /// The new left edge, beyond `cur_h`.
        dist: Scaled,
        /// Whether it begins a reversed segment (else it ends one): the
        /// glyph origins' reading order, not the output's.
        open: bool,
    },
    /// Leaders of a fixed width.
    Fixed(GlueNode),
}

/// `list`'s nodes, a discretionary's replacement text in its place (the
/// nodes TeX's list has after the discretionary node).
pub fn flatten_into(list: &[Node], q: &mut VecDeque<Lr>) {
    for n in list {
        match n {
            Node::Disc(d) => flatten_into(&d.replace, q),
            n => q.push_back(Lr::N(n.clone())),
        }
    }
}

/// What [`reverse`] asks of the box being output.
pub trait LrWidths {
    /// The width of character `c` of font `f`.
    fn char_wd(&mut self, f: FontId, c: u16) -> Scaled;
    /// Glue `g` as the box sets it, with its share of the rounding.
    fn glue(&mut self, g: &GlueNode) -> Scaled;
    /// Whether the box's setting stretches or shrinks `g`.
    fn glue_sets(&self, g: &GlueNode) -> bool;
}

/// e-TeX's `reverse`: the nodes of `q` up to the end of the segment
/// (all of them when `t` is `None`) in reverse order, `t` last, with
/// glue set as it is output, `cur_h` advanced by their widths; `lr` is
/// e-TeX's LR stack.
#[allow(clippy::too_many_lines)]
pub fn reverse(
    out: &mut impl LrWidths,
    cur_h: &mut Scaled,
    cur_dir: u8,
    q: &mut VecDeque<Lr>,
    mut t: Option<Lr>,
    lr: &mut Vec<u8>,
) -> VecDeque<Lr> {
    let mut l: VecDeque<Lr> = VecDeque::new();
    let mut m = 0u32;
    let mut n = 0u32;
    loop {
        while let Some(it) = q.pop_front() {
            let mut node = match it {
                Lr::N(node) => node,
                Lr::Fixed(g) => {
                    *cur_h += g.spec.width;
                    l.push_front(Lr::Fixed(g));
                    continue;
                }
                other @ Lr::Edge { .. } => {
                    l.push_front(other);
                    continue;
                }
            };
            let rule_wd = match &mut node {
                Node::Char { font, ch } => out.char_wd(*font, *ch),
                Node::Lig(lg) => {
                    let wd = out.char_wd(lg.font, lg.ch);
                    node = Node::Char {
                        font: lg.font,
                        ch: lg.ch,
                    };
                    wd
                }
                Node::Box(bx) => bx.width,
                Node::Rule(r) => r.width,
                Node::Kern(w, _) => *w,
                Node::Native(nw) => nw.width,
                Node::Whatsit(_, Some(sz)) => sz.width,
                Node::Glue(g) => {
                    let wd = out.glue(g);
                    // "Handle a glue node for mixed direction"
                    if out.glue_sets(g) {
                        if matches!(
                            g.kind,
                            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
                        ) {
                            let mut g = g.clone();
                            g.spec.width = wd;
                            *cur_h += wd;
                            l.push_front(Lr::Fixed(g));
                            continue;
                        }
                        node = Node::Kern(wd, KernKind::Normal);
                    }
                    wd
                }
                Node::Math(k, w) => {
                    let (k, w) = (*k, *w);
                    if k.is_end() {
                        if lr.last() == Some(&k.end_type()) {
                            lr.pop();
                            if n > 0 {
                                n -= 1;
                                node = Node::Math(k.flipped(), w);
                            } else {
                                node = Node::Kern(w, KernKind::Normal);
                                if m > 0 {
                                    m -= 1;
                                } else {
                                    // "Finish the reversed hlist segment"
                                    let h = *cur_h;
                                    if let Some(Lr::Edge { dir, .. }) = t {
                                        l.push_back(Lr::Edge {
                                            dir,
                                            width: w,
                                            dist: -h - w,
                                            open: false,
                                        });
                                    }
                                    return l;
                                }
                            }
                        } else {
                            node = Node::Kern(w, KernKind::Normal);
                        }
                    } else {
                        lr.push(k.end_type());
                        if n > 0 || k.dir() != cur_dir {
                            n += 1;
                            node = Node::Math(k.flipped(), w);
                        } else {
                            node = Node::Kern(w, KernKind::Normal);
                            m += 1;
                        }
                    }
                    w
                }
                _ => {
                    l.push_front(Lr::N(node));
                    continue;
                }
            };
            *cur_h += rule_wd;
            if matches!(node, Node::Kern(..)) && (rule_wd == 0 || (l.is_empty() && t.is_none())) {
                continue;
            }
            l.push_front(Lr::N(node));
        }
        if t.is_none() && m == 0 && n == 0 {
            break;
        }
        // "manufacture one missing math node"
        let top = lr.last().copied().unwrap_or(0);
        q.push_back(Lr::N(Node::Math(MathKind::of_subtype(i64::from(top)), 0)));
        if top == 0 {
            // (an empty stack: nothing left to close)
            q.clear();
            break;
        }
    }
    if let Some(t) = t.take() {
        l.push_back(t);
    }
    l
}
