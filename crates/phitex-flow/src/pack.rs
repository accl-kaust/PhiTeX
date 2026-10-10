//! Packaging (tex.web part 33, §644–§679), copied as pure functions: a
//! list and a specification in, a box and what TeX reports about it out.
//! The report is printed by whoever logs it (`show`).

use crate::arith::badness;
use crate::item::{BoxNode, GlueSet, Item, RUNNING};

/// §644: the size a box is packed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    /// `to` this size.
    Exactly(i32),
    /// The natural size plus this much (`spread`; 0 for natural).
    Additional(i32),
}

impl Spec {
    pub const NATURAL: Spec = Spec::Additional(0);
}

/// The parameters packaging reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// `\hbadness` or `\vbadness`.
    pub badness: i32,
    /// `\hfuzz` or `\vfuzz`.
    pub fuzz: i32,
    /// `\overfullrule` (hpack only).
    pub overfull_rule: i32,
}

impl Params {
    /// No report at all (tex.web 1017's page box).
    pub const QUIET: Params = Params { badness: 10000, fuzz: 0o7_777_777_777, overfull_rule: 0 };
}

/// §660–§667, §674–§678: what TeX reports about a packed box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Underfull(i32),
    Loose(i32),
    Tight(i32),
    /// This much too wide or too high.
    Overfull(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    pub b: BoxNode,
    /// `last_badness`.
    pub badness: i32,
    pub report: Option<Report>,
}

/// §646: glue totals by order.
#[derive(Default)]
struct Totals {
    stretch: [i32; 4],
    shrink: [i32; 4],
}

fn order(t: &[i32; 4]) -> u8 {
    if t[3] != 0 {
        3
    } else if t[2] != 0 {
        2
    } else {
        u8::from(t[1] != 0)
    }
}

/// etex.web's `end_LR_type`: the subtype of the node ending a math
/// node of subtype `s` (`L_code*(s div L_code)+end_M_code`).
pub fn end_lr_type(s: u8) -> u8 {
    4 * (s / 4) + 3
}

/// The subtype of a math node (etex.web: `lr + after`).
pub fn math_subtype(lr: u8, after: bool) -> u8 {
    lr + u8::from(after)
}

/// TeXXeT's checks of `hpack` (etex.web §1438–§1440), with `\TeXXeTstate`
/// positive: an end node matching no begin node becomes an explicit kern;
/// the begin nodes left open get their end nodes at the list's end. The
/// number of problems (missing ends ×10000, extra ends), which TeX
/// reports in the box's diagnostic (not yet: CORE.md).
pub fn lr_fix(list: &mut Vec<Item>) -> u32 {
    // (`before` at the bottom never matches)
    let mut stack: Vec<u8> = vec![0];
    let mut problems = 0;
    for it in list.iter_mut() {
        if let Item::Math { w, after, lr } = *it {
            let s = math_subtype(lr, after);
            if after {
                if stack.last() == Some(&end_lr_type(s)) {
                    stack.pop();
                } else {
                    problems += 1;
                    *it = Item::Kern { w, sub: 1 };
                }
            } else {
                stack.push(end_lr_type(s));
            }
        }
    }
    while let Some(&e) = stack.last() {
        if e == 0 {
            break;
        }
        list.push(Item::Math { w: 0, after: true, lr: e - 1 });
        problems += 10000;
        stack.pop();
    }
    problems
}

/// §649: `hpack`. Marks, insertions and adjustments' material move out to
/// `adjust` if given (§655).
pub fn hpack(mut list: Vec<Item>, spec: Spec, params: &Params, adjust: Option<&mut Vec<Item>>) -> Packed {
    if let Some(adj) = adjust {
        let (moved, kept): (Vec<Item>, Vec<Item>) = list
            .into_iter()
            .partition(|i| matches!(i, Item::Mark(_) | Item::Marks(..) | Item::Ins(_) | Item::Adjust(_)));
        for m in moved {
            match m {
                Item::Adjust(l) => adj.extend(l.iter().cloned()),
                other => adj.push(other),
            }
        }
        list = kept;
    }
    let (mut h, mut d, mut x) = (0i32, 0i32, 0i32);
    let mut t = Totals::default();
    let mut boxed = |w: i32, ht: i32, dp: i32, s: i32, x: &mut i32| {
        *x += w;
        h = h.max(ht - s);
        d = d.max(dp + s);
    };
    for it in &list {
        match it {
            Item::Char(f, c) => {
                if let Some(g) = f.glyph(i32::from(*c)) {
                    boxed(g.width, g.height, g.depth, 0, &mut x);
                }
            }
            Item::Lig(l) => {
                if let Some(g) = l.font.glyph(i32::from(l.c)) {
                    boxed(g.width, g.height, g.depth, 0, &mut x);
                }
            }
            Item::Box(b) => boxed(b.w, b.h, b.d, b.shift, &mut x),
            Item::Unset(u) => boxed(u.w, u.h, u.d, 0, &mut x),
            Item::Rule { w, h: rh, d: rd } => boxed(*w, *rh, *rd, 0, &mut x),
            Item::Glue { g, .. } => {
                x += g.width;
                t.stretch[usize::from(g.stretch_order)] += g.stretch;
                t.shrink[usize::from(g.shrink_order)] += g.shrink;
            }
            Item::Leaders(l) => {
                // (§656: and the leader box's height and depth)
                let (lh, ld) = match &l.b {
                    Item::Box(b) => (b.h, b.d),
                    Item::Rule { h, d, .. } => (*h, *d),
                    _ => (0, 0),
                };
                boxed(0, lh, ld, 0, &mut x);
                x += l.g.width;
                t.stretch[usize::from(l.g.stretch_order)] += l.g.stretch;
                t.shrink[usize::from(l.g.shrink_order)] += l.g.shrink;
            }
            Item::Kern { w, .. } | Item::Math { w, .. } => x += w,
            // (pdfTeX: an image's or a form's reference, §1586)
            Item::Whatsit(wh) => {
                if let Some((ww, wh, wd)) = wh.dims() {
                    boxed(ww, wh, wd, 0, &mut x);
                }
            }
            _ => {}
        }
    }
    let w = match spec {
        Spec::Exactly(w) => w,
        Spec::Additional(s) => x + s,
    };
    // (a box is kept long: no spare capacity)
    list.shrink_to_fit();
    let mut b = BoxNode { vertical: false, w, h, d, shift: 0, list, glue: GlueSet::default() };
    let (badness, report) = set_glue(&mut b, w - x, &t, params, true);
    Packed { b, badness, report }
}

/// §668: `vpackage` with the depth at most `max_depth` (`vpack`: no limit).
pub fn vpack(list: Vec<Item>, spec: Spec, max_depth: i32, params: &Params) -> Packed {
    let (mut w, mut d, mut x) = (0i32, 0i32, 0i32);
    let mut t = Totals::default();
    for it in &list {
        match it {
            Item::Box(b) => {
                x += d + b.h;
                d = b.d;
                w = w.max(b.w + b.shift);
            }
            Item::Unset(u) => {
                x += d + u.h;
                d = u.d;
                w = w.max(u.w);
            }
            Item::Rule { w: rw, h, d: rd } => {
                x += d + h;
                d = *rd;
                w = w.max(*rw);
            }
            Item::Glue { g, .. } => {
                x += d + g.width;
                d = 0;
                t.stretch[usize::from(g.stretch_order)] += g.stretch;
                t.shrink[usize::from(g.shrink_order)] += g.shrink;
            }
            Item::Leaders(l) => {
                // (§671: and the leader box's width)
                x += d + l.g.width;
                d = 0;
                t.stretch[usize::from(l.g.stretch_order)] += l.g.stretch;
                t.shrink[usize::from(l.g.shrink_order)] += l.g.shrink;
                let lw = match &l.b {
                    Item::Box(b) => b.w,
                    Item::Rule { w, .. } => *w,
                    _ => 0,
                };
                w = w.max(lw);
            }
            Item::Kern { w: k, .. } => {
                x += d + k;
                d = 0;
            }
            // (pdfTeX: an image's or a form's reference, §1586)
            Item::Whatsit(wh) => {
                if let Some((ww, wh, wd)) = wh.dims() {
                    x += d + wh;
                    d = wd;
                    w = w.max(ww);
                }
            }
            _ => {}
        }
    }
    let depth = if d > max_depth {
        x += d - max_depth;
        max_depth.max(0)
    } else {
        d
    };
    let h = match spec {
        Spec::Exactly(h) => h,
        Spec::Additional(s) => x + s,
    };
    let mut b = BoxNode { vertical: true, w, h, d: depth, shift: 0, list, glue: GlueSet::default() };
    let (badness, report) = set_glue(&mut b, h - x, &t, params, false);
    Packed { b, badness, report }
}

/// §657–§667, §672–§678: the glue setting for excess `x`, the badness and
/// the report.
fn set_glue(b: &mut BoxNode, x: i32, t: &Totals, params: &Params, horizontal: bool) -> (i32, Option<Report>) {
    if x == 0 {
        return (0, None);
    }
    let empty = b.list.is_empty();
    if x > 0 {
        let o = order(&t.stretch);
        b.glue.order = o;
        if t.stretch[usize::from(o)] == 0 {
            b.glue.sign = 0;
        } else {
            b.glue.sign = 1;
            b.glue.ratio = f64::from(x) / f64::from(t.stretch[usize::from(o)]);
        }
        if o == 0 && !empty {
            let bad = badness(x, t.stretch[0]);
            if bad > params.badness {
                return (bad, Some(if bad > 100 { Report::Underfull(bad) } else { Report::Loose(bad) }));
            }
            return (bad, None);
        }
        return (0, None);
    }
    let o = order(&t.shrink);
    b.glue.order = o;
    if t.shrink[usize::from(o)] == 0 {
        b.glue.sign = 0;
    } else {
        b.glue.sign = -1;
        b.glue.ratio = f64::from(-x) / f64::from(t.shrink[usize::from(o)]);
    }
    if t.shrink[usize::from(o)] < -x && o == 0 && !empty {
        b.glue.ratio = 1.0;
        let excess = -x - t.shrink[0];
        if excess > params.fuzz || params.badness < 100 {
            if horizontal && params.overfull_rule > 0 && excess > params.fuzz {
                b.list.push(Item::Rule { w: params.overfull_rule, h: RUNNING, d: RUNNING });
            }
            return (1_000_000, Some(Report::Overfull(excess)));
        }
        return (1_000_000, None);
    }
    if o == 0 && !empty {
        let bad = badness(-x, t.shrink[0]);
        if bad > params.badness {
            return (bad, Some(Report::Tight(bad)));
        }
        return (bad, None);
    }
    (0, None)
}
