//! Breaking vertical lists into pages (tex.web part 44, §967–§979).
//!
//! `vert_break`, `prune_page_top` and `vsplit` as pure functions; the page
//! builder (part 45) builds on them.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::Scaled;
use crate::node::{BoxNode, GlueSpec, Node, Order, Tokens};
use crate::pack::Confusion;
use crate::scaled::{INF_BAD, badness};

/// §157
const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
/// §833, §974
const AWFUL_BAD: i32 = 0o7777777777;
const DEPLORABLE: i32 = 100_000;
/// §224: `split_top_skip_code`.
const SPLIT_TOP_SKIP: u8 = 10;

/// §970: where to break a vlist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertBreak {
    /// The index of the best break node; `None`: at the end.
    pub at: Option<usize>,
    /// `best_height_plus_depth`.
    pub best_height_plus_depth: Scaled,
    /// How many times "Infinite glue shrinkage found in box being split"
    /// was reported (the glue has been made finite in the list).
    pub infinite_shrink: u32,
}

/// `precedes_break` (§148) in a vlist.
pub(crate) fn precedes_break(n: &Node) -> bool {
    matches!(
        n,
        Node::Glyphs(_)
            | Node::Box(_)
            | Node::Rule { .. }
            | Node::Ins(_)
            | Node::Mark(_)
            | Node::Adjust(_)
            | Node::Ligature(_)
            | Node::Disc(_)
            | Node::Whatsit(_)
    )
}

/// §970: find the best place to break `list` for height `h` and maximum
/// depth `d`.
pub fn vert_break(list: &mut [Node], h: Scaled, d: Scaled) -> Result<VertBreak, Confusion> {
    let mut least_cost = AWFUL_BAD;
    let mut ah = [0; 7]; // `active_height`
    let mut prev_dp = 0;
    let mut best = VertBreak {
        at: None,
        best_height_plus_depth: 0,
        infinite_shrink: 0,
    };
    let mut i = 0;
    loop {
        // §972: if node `i` is a legal breakpoint, check if this break is
        // the best known, and stop at the end or if the page-so-far is
        // already too full to accept more stuff.
        enum Next {
            NotFound,
            UpdateHeights,
            Break(i32),
        }
        let next = match list.get(i) {
            None => Next::Break(EJECT_PENALTY),
            // §973
            Some(Node::Box(b)) => {
                ah[1] += prev_dp + b.height;
                prev_dp = b.depth;
                Next::NotFound
            }
            Some(Node::Rule { height, depth, .. }) => {
                ah[1] += prev_dp + height;
                prev_dp = *depth;
                Next::NotFound
            }
            Some(Node::Glue { .. } | Node::Leaders(_)) => {
                // (an initial glue node is not a legal breakpoint)
                if i > 0 && precedes_break(&list[i - 1]) {
                    Next::Break(0)
                } else {
                    Next::UpdateHeights
                }
            }
            Some(Node::Kern { .. }) => {
                if matches!(list.get(i + 1), Some(Node::Glue { .. } | Node::Leaders(_))) {
                    Next::Break(0)
                } else {
                    Next::UpdateHeights
                }
            }
            Some(Node::Penalty(pi)) => Next::Break(*pi),
            Some(Node::Whatsit(_) | Node::Mark(_) | Node::Ins(_)) => Next::NotFound,
            Some(_) => return Err(Confusion("vertbreak")),
        };
        let mut update = matches!(next, Next::UpdateHeights);
        if let Next::Break(pi) = next {
            // §974: check if node `i` is a new champion breakpoint.
            if pi < INF_PENALTY {
                // §975: compute the badness, using `awful_bad` if the box
                // is too full.
                let mut b = if ah[1] < h {
                    if ah[3] != 0 || ah[4] != 0 || ah[5] != 0 {
                        0
                    } else {
                        badness(h - ah[1], ah[2])
                    }
                } else if ah[1] - h > ah[6] {
                    AWFUL_BAD
                } else {
                    badness(ah[1] - h, ah[6])
                };
                if b < AWFUL_BAD {
                    if pi <= EJECT_PENALTY {
                        b = pi;
                    } else if b < INF_BAD {
                        b += pi;
                    } else {
                        b = DEPLORABLE;
                    }
                }
                if b <= least_cost {
                    best.at = (i < list.len()).then_some(i);
                    least_cost = b;
                    best.best_height_plus_depth = ah[1] + prev_dp;
                }
                if b == AWFUL_BAD || pi <= EJECT_PENALTY {
                    return Ok(best);
                }
            }
            update = matches!(
                list.get(i),
                Some(Node::Glue { .. } | Node::Leaders(_) | Node::Kern { .. })
            );
        }
        if update {
            // update_heights: §976
            let width = match &mut list[i] {
                Node::Kern { width, .. } => *width,
                Node::Glue { spec, .. } => finite(spec, &mut ah, &mut best),
                Node::Leaders(l) => finite(&mut l.spec, &mut ah, &mut best),
                _ => unreachable!(),
            };
            ah[1] += prev_dp + width;
            prev_dp = 0;
        }
        // not_found:
        if prev_dp > d {
            ah[1] += prev_dp - d;
            prev_dp = d;
        }
        i += 1;
    }
}

/// §976: add glue `g`'s stretch and shrink to `ah`, making infinite
/// shrink finite; returns its width.
fn finite(g: &mut GlueSpec, ah: &mut [Scaled; 7], best: &mut VertBreak) -> Scaled {
    ah[2 + g.stretch_order.index()] += g.stretch;
    ah[6] += g.shrink;
    if g.shrink_order != Order::Normal && g.shrink != 0 {
        best.infinite_shrink += 1;
        g.shrink_order = Order::Normal;
    }
    g.width
}

/// §968: adjust the top of `list` after a page break: discard glue, kerns
/// and penalties before the first box or rule, and put `\splittopskip`
/// glue above it.
/// The discarded glue, kerns and penalties go to `discards` if given.
pub fn prune_page_top(
    list: Vec<Node>,
    split_top_skip: GlueSpec,
    mut discards: Option<&mut Vec<Node>>,
) -> Result<Vec<Node>, Confusion> {
    let mut out = Vec::with_capacity(list.len() + 1);
    let mut nodes = list.into_iter();
    while let Some(n) = nodes.next() {
        match n {
            Node::Box(_) | Node::Rule { .. } => {
                // §969: insert glue for `split_top_skip`.
                let height = match &n {
                    Node::Box(b) => b.height,
                    Node::Rule { height, .. } => *height,
                    _ => unreachable!(),
                };
                let width = if split_top_skip.width > height {
                    split_top_skip.width - height
                } else {
                    0
                };
                out.push(Node::Glue {
                    spec: GlueSpec {
                        width,
                        ..split_top_skip.copy()
                    },
                    subtype: SPLIT_TOP_SKIP + 1,
                    sync: crate::origin::Side(0),
                });
                out.push(n);
                out.extend(nodes);
                return Ok(out);
            }
            Node::Whatsit(_) | Node::Mark(_) | Node::Ins(_) => out.push(n),
            Node::Glue { .. } | Node::Leaders(_) | Node::Kern { .. } | Node::Penalty(_) => {
                if let Some(d) = discards.as_deref_mut() {
                    d.push(n);
                }
            }
            _ => return Err(Confusion("pruning")),
        }
    }
    Ok(out)
}

/// The parameters `\vsplit` reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplitParams {
    pub split_max_depth: Scaled,
    pub split_top_skip: GlueSpec,
    /// e-TeX's `\savingvdiscards>0` (for `\splitdiscards`).
    pub save_discards: bool,
}

/// What `\vsplit` produces, for the caller to pack (§977: `vpack` of the
/// rest, then `vpackage` of the split to the requested height, routines
/// of the caller's).
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    /// The list split off.
    pub split: Vec<Node>,
    /// What remains in the register (`None`: void).
    pub rest: Option<Vec<Node>>,
    /// `\splitfirstmark` and `\splitbotmark`, by mark class.
    pub marks: BTreeMap<i32, (Tokens, Tokens)>,
    /// e-TeX: the items discarded after the break, if saved.
    pub discards: Vec<Node>,
    /// Reports of infinite shrinkage (see [`VertBreak`]).
    pub infinite_shrink: u32,
}

/// §977: split a vbox's list at height `h`.
///
/// # Errors
/// A confusion if the list holds what a vlist cannot.
pub fn vsplit(v: &BoxNode, h: Scaled, p: &SplitParams) -> Result<Split, Confusion> {
    let mut list = v.list.clone();
    let brk = vert_break(&mut list, h, p.split_max_depth)?;
    let at = brk.at.unwrap_or(list.len());
    let rest = list.split_off(at);
    // §979: look at all the marks in nodes before the break.
    let mut marks: BTreeMap<i32, (Tokens, Tokens)> = BTreeMap::new();
    for n in &list {
        if let Node::Mark(m) = n {
            marks
                .entry(m.class)
                .and_modify(|(_, bot)| *bot = m.tokens.clone())
                .or_insert_with(|| (m.tokens.clone(), m.tokens.clone()));
        }
    }
    let mut discards = Vec::new();
    let rest = prune_page_top(
        rest,
        p.split_top_skip,
        p.save_discards.then_some(&mut discards),
    )?;
    let rest = if rest.is_empty() {
        None // the `eq_level` of the box stays the same
    } else {
        Some(rest)
    };
    Ok(Split {
        split: list,
        rest,
        marks,
        discards,
        infinite_shrink: brk.infinite_shrink,
    })
}

/// A box node for a packed result.
#[must_use]
pub fn boxed(b: BoxNode) -> Node {
    Node::Box(b.share())
}
