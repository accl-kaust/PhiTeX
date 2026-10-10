//! `\vsplit` (tex.web §967–§979), copied as pure functions: the best break
//! of a vertical list for a height ([`vert_break`]), the top of what
//! follows a break ([`prune_page_top`]), and the split of a list
//! ([`vsplit`]) — a function of a box's list, the height, `\splitmaxdepth`
//! and `\splittopskip`.

use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness};
use crate::item::{Item, Leaders};
use crate::val::{Glue, GluePar, SplitMark};

const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
const DEPLORABLE: i32 = 100_000;

/// `it` with its glue specification `g` (a glue item, or glue with
/// leaders).
fn with_glue(it: &Item, g: Glue) -> Item {
    match it {
        Item::Leaders(l) => Item::Leaders(Arc::new(Leaders { g: Arc::new(g), ..(**l).clone() })),
        Item::Glue { sub, .. } => Item::Glue { g: Arc::new(g), sub: *sub, zero: false },
        other => other.clone(),
    }
}

/// §970: `vert_break`: where to break `list` for height `h`, depths at
/// most `d`: the index of the item the break is at (`list.len()`: the
/// end). Glue whose shrinkability is infinite is made finite in `list`, as
/// TeX does after its error (then `true`).
pub fn vert_break(list: &mut [Item], h: i32, d: i32) -> (usize, bool) {
    // (`active_height[1..6]`: the height, stretch by order, shrink)
    let mut ah = [0i32; 7];
    let mut prev_dp = 0i32;
    let mut least_cost = AWFUL_BAD;
    let mut best = list.len();
    let mut bad_shrink = false;
    // (an initial glue is not a legal breakpoint: `prev_p` starts at `p`)
    let mut prev = 0usize;
    let mut i = 0usize;
    loop {
        // (§972, §973)
        let mut pi = None;
        let mut heights = false;
        if i >= list.len() {
            pi = Some(EJECT_PENALTY);
        } else {
            match &list[i] {
                Item::Box(b) => {
                    ah[1] = ah[1].wrapping_add(prev_dp).wrapping_add(b.h);
                    prev_dp = b.d;
                }
                Item::Rule { h: rh, d: rd, .. } => {
                    ah[1] = ah[1].wrapping_add(prev_dp).wrapping_add(*rh);
                    prev_dp = *rd;
                }
                // (pdfTeX: an image's or a form's reference, §1588)
                Item::Whatsit(w) if w.dims().is_some() => {
                    let (_, wh, wd) = w.dims().unwrap_or_default();
                    ah[1] = ah[1].wrapping_add(prev_dp).wrapping_add(wh);
                    prev_dp = wd;
                }
                Item::Kern { .. } => {
                    if list.get(i + 1).is_some_and(Item::is_glue) {
                        pi = Some(0);
                    } else {
                        heights = true;
                    }
                }
                Item::Penalty(x) => pi = Some(*x),
                it if it.is_glue() => {
                    if list[prev].precedes_break() && prev != i {
                        pi = Some(0);
                    } else {
                        heights = true;
                    }
                }
                _ => {}
            }
        }
        if let Some(pi) = pi {
            // (§974)
            if pi < INF_PENALTY {
                let mut b = if ah[1] < h {
                    if ah[3] != 0 || ah[4] != 0 || ah[5] != 0 { 0 } else { badness(h - ah[1], ah[2]) }
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
                    best = i;
                    least_cost = b;
                }
                if b == AWFUL_BAD || pi <= EJECT_PENALTY {
                    return (best, bad_shrink);
                }
            }
            if i < list.len() && (list[i].is_glue() || matches!(list[i], Item::Kern { .. })) {
                heights = true;
            }
        }
        if heights {
            // (§976)
            let (w, fixed) = match &list[i] {
                Item::Kern { w, .. } => (*w, None),
                it => {
                    let g = it.glue().map(|g| **g).unwrap_or_default();
                    let o = 2 + usize::from(g.stretch_order.min(3));
                    ah[o] = ah[o].wrapping_add(g.stretch);
                    ah[6] = ah[6].wrapping_add(g.shrink);
                    let fixed = (g.shrink_order != 0 && g.shrink != 0)
                        .then(|| with_glue(it, Glue { shrink_order: 0, ..g }));
                    (g.width, fixed)
                }
            };
            if let Some(x) = fixed {
                bad_shrink = true;
                list[i] = x;
            }
            ah[1] = ah[1].wrapping_add(prev_dp).wrapping_add(w);
            prev_dp = 0;
        }
        // (`not_found`)
        if prev_dp > d {
            ah[1] = ah[1].wrapping_add(prev_dp - d);
            prev_dp = d;
        }
        prev = i;
        i += 1;
    }
}

/// §968: `prune_page_top`: glue, kerns and penalties before the first box
/// or rule go, and `\splittopskip` (less the box's height) goes before
/// it.
pub fn prune_page_top(list: Vec<Item>, top: &Glue) -> Vec<Item> {
    let mut out = Vec::with_capacity(list.len() + 1);
    let mut it = list.into_iter();
    while let Some(x) = it.next() {
        let h = match &x {
            Item::Box(b) => Some(b.h),
            Item::Rule { h, .. } => Some(*h),
            _ => None,
        };
        if let Some(h) = h {
            let w = if top.width > h { top.width - h } else { 0 };
            let g = Glue { width: w, ..*top };
            out.push(Item::Glue { g: Arc::new(g), sub: GluePar::Splittopskip as u8 + 1, zero: false });
            out.push(x);
            out.extend(it);
            return out;
        }
        if !(x.is_glue() || matches!(x, Item::Kern { .. } | Item::Penalty(_))) {
            out.push(x);
        }
    }
    out
}

/// What `\vsplit` makes of a list.
#[derive(Debug)]
pub struct Split {
    /// The part split off (before the break), unpacked.
    pub first: Vec<Item>,
    /// What stays, its top pruned.
    pub rest: Vec<Item>,
    /// The first and last marks of each class in the part split off
    /// (`Item::Mark` class 0, `Item::Marks`), by class.
    pub marks: Vec<SplitMark>,
    /// Glue of infinite shrinkability was met (an error in TeX).
    pub bad_shrink: bool,
}

/// §977–§979: `list` split for height `h` with depths at most `d`;
/// `top` is `\splittopskip`.
pub fn vsplit(list: &[Item], h: i32, d: i32, top: &Glue) -> Split {
    let mut l = list.to_vec();
    let (q, bad_shrink) = vert_break(&mut l, h, d);
    let rest = l.split_off(q.min(l.len()));
    let mut marks: Vec<SplitMark> = Vec::new();
    for it in &l {
        let (n, t) = match it {
            Item::Mark(t) => (0, t),
            Item::Marks(n, t) => (*n, t),
            _ => continue,
        };
        match marks.binary_search_by_key(&n, |m| m.0) {
            Ok(k) => marks[k].2 = t.clone(),
            Err(k) => marks.insert(k, (n, t.clone(), t.clone())),
        }
    }
    Split { first: l, rest: prune_page_top(rest, top), marks, bad_shrink }
}
