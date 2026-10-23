//! Packaging: hpack and vpack (§644–§679).

use crate::arith::{MAX_DIMEN, Scaled, badness};
use crate::fonts::Fonts;
use crate::glue::{GlueSet, GlueSign, Order, OrderTotals};
use crate::node::{BoxKind, BoxNode, List, Node, RUNNING, Rule};

/// A box size specification (§644).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    /// `to w`: `exactly`.
    Exactly(Scaled),
    /// `spread w`: `additional`; `Additional(0)` is `natural`.
    Additional(Scaled),
}

/// `natural` (§644).
pub const NATURAL: Spec = Spec::Additional(0);

/// What tex.web would report about a packaged box (§660–§667, §674–§678).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// Badness above the threshold, over 100.
    Underfull(i32),
    /// Badness above the threshold, at most 100.
    Loose(i32),
    /// Shrunk with badness above the threshold.
    Tight(i32),
    /// Too wide (or high) by this amount.
    Overfull(Scaled),
}

/// Packaging parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackParams {
    /// `\hbadness` or `\vbadness`.
    pub badness: i32,
    /// `\hfuzz` or `\vfuzz`.
    pub fuzz: Scaled,
    /// `\overfullrule` (hpack only).
    pub overfull_rule: Scaled,
}

impl Default for PackParams {
    /// plain TeX's values.
    fn default() -> Self {
        PackParams {
            badness: 1000,
            fuzz: 6554,
            overfull_rule: 5 * 0x1_0000,
        }
    }
}

/// The result of packaging.
#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    /// The box.
    pub node: BoxNode,
    /// `last_badness` (`\badness`).
    pub badness: i32,
    /// The diagnostic tex.web would print, if any.
    pub report: Option<Report>,
    /// Material moved out by `adjust_tail` (§655): insertions, marks and
    /// vadjust contents, in order.
    pub adjustments: List,
}

#[derive(Default)]
struct Totals {
    stretch: OrderTotals,
    shrink: OrderTotals,
}

/// Accumulates hlist dimensions (§651–§656); returns (h, d, x).
fn scan_hlist(
    list: &[Node],
    fonts: &dyn Fonts,
    t: &mut Totals,
    hdx: &mut (Scaled, Scaled, Scaled),
) {
    // (the font of the run of characters being measured, and its table)
    let mut run: Option<(u32, Option<&[crate::fonts::Whd]>)> = None;
    for p in list {
        match p {
            Node::Char { font, ch } => {
                let table = match run {
                    Some((f, t)) if f == *font => t,
                    _ => {
                        let t = fonts.whd_table(*font);
                        run = Some((*font, t));
                        t
                    }
                };
                match table {
                    Some(t) => {
                        // §654
                        let (w, h, d) = t.get(usize::from(*ch)).copied().unwrap_or((0, 0, 0));
                        hdx.2 += w;
                        hdx.0 = hdx.0.max(h);
                        hdx.1 = hdx.1.max(d);
                    }
                    None => char_dims(fonts, *font, *ch, hdx),
                }
            }
            // §652
            Node::Lig(l) => char_dims(fonts, l.font, l.ch, hdx),
            Node::Box(_) | Node::Rule(_) | Node::Unset(_) => {
                // §653
                let (w, ht, dp, s) = p.box_dims().unwrap_or_default();
                hdx.2 += w;
                hdx.0 = hdx.0.max(ht - s);
                hdx.1 = hdx.1.max(dp + s);
            }
            Node::Glue(g) => {
                // §656
                hdx.2 += g.spec.width;
                t.stretch.add(g.spec.stretch_order, g.spec.stretch);
                t.shrink.add(g.spec.shrink_order, g.spec.shrink);
                if let Some(l) = &g.leader {
                    let (_, ht, dp, _) = l.box_dims().unwrap_or_default();
                    hdx.0 = hdx.0.max(ht);
                    hdx.1 = hdx.1.max(dp);
                }
            }
            Node::Kern(w, _) | Node::Math(_, w) => hdx.2 += w,
            Node::Whatsit(_, Some(s)) => {
                // pdfTeX: "Incorporate a whatsit node into an hbox"
                hdx.2 += s.width;
                hdx.0 = hdx.0.max(s.height);
                hdx.1 = hdx.1.max(s.depth);
            }
            // The replacement text is part of the hlist in tex.web.
            Node::Disc(d) => scan_hlist(&d.replace, fonts, t, hdx),
            Node::Ins(_)
            | Node::Mark(_)
            | Node::Adjust(_)
            | Node::Whatsit(_, None)
            | Node::Language { .. }
            | Node::Penalty(_) => {}
        }
    }
}

fn char_dims(fonts: &dyn Fonts, font: u32, ch: u16, hdx: &mut (Scaled, Scaled, Scaled)) {
    // §654
    let (w, h, d) = fonts.char_whd(font, ch);
    hdx.2 += w;
    hdx.0 = hdx.0.max(h);
    hdx.1 = hdx.1.max(d);
}

/// Moves insertions, marks and adjust material out of `list` (§655).
fn take_adjustments(list: &mut List) -> List {
    // (most lists have none: then nothing moves)
    if !list
        .iter()
        .any(|n| matches!(n, Node::Adjust(_) | Node::Ins(_) | Node::Mark(_)))
    {
        return List::new();
    }
    let mut out = List::new();
    let mut kept = List::with_capacity(list.len());
    for n in list.drain(..) {
        match n {
            Node::Adjust(a) => out.extend(a),
            Node::Ins(_) | Node::Mark(_) => out.push(n),
            _ => kept.push(n),
        }
    }
    *list = kept;
    out
}

/// `hpack(p, w, m)` (§649). With `adjust`, tex.web's `adjust_tail` is
/// non-null: insertions, marks and vadjust material move to
/// [`Packed::adjustments`].
#[must_use]
pub fn hpack(
    mut list: List,
    spec: Spec,
    fonts: &dyn Fonts,
    params: &PackParams,
    adjust: bool,
) -> Packed {
    let adjustments = if adjust {
        take_adjustments(&mut list)
    } else {
        List::new()
    };
    let mut t = Totals::default();
    let mut hdx = (0, 0, 0);
    scan_hlist(&list, fonts, &mut t, &mut hdx);
    let (h, d, x) = hdx;
    let mut node = BoxNode {
        kind: BoxKind::H,
        height: h,
        depth: d,
        ..BoxNode::default()
    };
    let (width, badness, report) = set_glue(&mut node.glue, x, spec, &t, params, !list.is_empty());
    node.width = width;
    // §666: the overfull rule
    if let Some(Report::Overfull(excess)) = report
        && params.overfull_rule > 0
        && excess > params.fuzz
    {
        list.push(Node::Rule(Rule {
            width: params.overfull_rule,
            height: RUNNING,
            depth: RUNNING,
        }));
    }
    node.list = list;
    Packed {
        node,
        badness,
        report,
        adjustments,
    }
}

/// `vpackage(p, h, m, l)` (§668); `vpack` is `max_depth = MAX_DIMEN`.
///
/// # Panics
/// On a character in the list (tex.web's `confusion("vpack")`).
#[must_use]
pub fn vpackage(list: List, spec: Spec, max_depth: Scaled, params: &PackParams) -> Packed {
    let mut t = Totals::default();
    let (mut w, mut d, mut x): (Scaled, Scaled, Scaled) = (0, 0, 0);
    for p in &list {
        match p {
            Node::Char { .. } | Node::Lig(_) => panic!("vpack: character in a vertical list"),
            Node::Box(_) | Node::Rule(_) | Node::Unset(_) => {
                // §670
                let (wd, ht, dp, s) = p.box_dims().unwrap_or_default();
                x += d + ht;
                d = dp;
                w = w.max(wd + s);
            }
            Node::Glue(g) => {
                // §671
                x += d;
                d = 0;
                x += g.spec.width;
                t.stretch.add(g.spec.stretch_order, g.spec.stretch);
                t.shrink.add(g.spec.shrink_order, g.spec.shrink);
                if let Some(l) = &g.leader {
                    let (wd, ..) = l.box_dims().unwrap_or_default();
                    w = w.max(wd);
                }
            }
            Node::Kern(k, _) => {
                x += d + k;
                d = 0;
            }
            Node::Whatsit(_, Some(s)) => {
                // pdfTeX: "Incorporate a whatsit node into a vbox"
                x += d + s.height;
                d = s.depth;
                w = w.max(s.width);
            }
            _ => {}
        }
    }
    let depth = if d > max_depth {
        x += d - max_depth;
        max_depth
    } else {
        d
    };
    let mut node = BoxNode {
        kind: BoxKind::V,
        width: w,
        depth,
        ..BoxNode::default()
    };
    let (height, badness, report) = set_glue(&mut node.glue, x, spec, &t, params, !list.is_empty());
    node.height = height;
    node.list = list;
    Packed {
        node,
        badness,
        report,
        adjustments: List::new(),
    }
}

/// `vpack(p, h, m)` (§668).
#[must_use]
pub fn vpack(list: List, spec: Spec, params: &PackParams) -> Packed {
    vpackage(list, spec, MAX_DIMEN, params)
}

/// §657–§667 and §672–§678: sets the glue; returns the packed dimension,
/// `last_badness` and the report.
fn set_glue(
    glue: &mut GlueSet,
    natural: Scaled,
    spec: Spec,
    t: &Totals,
    params: &PackParams,
    nonempty: bool,
) -> (Scaled, i32, Option<Report>) {
    let w = match spec {
        Spec::Exactly(w) => w,
        Spec::Additional(w) => natural + w,
    };
    let x = w - natural;
    if x == 0 {
        *glue = GlueSet::default();
        return (w, 0, None);
    }
    if x > 0 {
        // §658
        let o = t.stretch.highest();
        let total = t.stretch.get(o);
        *glue = if total == 0 {
            GlueSet {
                ratio: 0.0,
                sign: GlueSign::Normal,
                order: o,
            }
        } else {
            GlueSet {
                ratio: f64::from(x) / f64::from(total),
                sign: GlueSign::Stretching,
                order: o,
            }
        };
        if o == Order::Normal && nonempty {
            // §660
            let b = badness(x, t.stretch.get(Order::Normal));
            let report = (b > params.badness).then_some({
                if b > 100 {
                    Report::Underfull(b)
                } else {
                    Report::Loose(b)
                }
            });
            return (w, b, report);
        }
        return (w, 0, None);
    }
    // §664
    let o = t.shrink.highest();
    let total = t.shrink.get(o);
    *glue = if total == 0 {
        GlueSet {
            ratio: 0.0,
            sign: GlueSign::Normal,
            order: o,
        }
    } else {
        GlueSet {
            ratio: f64::from(-x) / f64::from(total),
            sign: GlueSign::Shrinking,
            order: o,
        }
    };
    if total < -x && o == Order::Normal && nonempty {
        glue.ratio = 1.0;
        // §666
        let excess = -x - t.shrink.get(Order::Normal);
        let report =
            (excess > params.fuzz || params.badness < 100).then_some(Report::Overfull(excess));
        return (w, 1_000_000, report);
    }
    if o == Order::Normal && nonempty {
        // §667
        let b = badness(-x, t.shrink.get(Order::Normal));
        return (w, b, (b > params.badness).then_some(Report::Tight(b)));
    }
    (w, 0, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arith::UNITY;
    use crate::glue::Glue;
    use crate::node::{BoxNode, GlueKind, GlueNode};
    use crate::testfont::TestFonts;

    fn fonts() -> TestFonts {
        TestFonts::default()
            .with_char(0, 65, 5 * UNITY, 7 * UNITY, 0)
            .with_char(0, 103, 4 * UNITY, 4 * UNITY, 2 * UNITY)
    }

    fn ch(c: u16) -> Node {
        Node::Char { font: 0, ch: c }
    }

    #[test]
    fn natural_hbox() {
        let p = PackParams::default();
        let r = hpack(
            vec![
                ch(65),
                Node::glue(Glue::new(3 * UNITY, UNITY, UNITY)),
                ch(103),
            ],
            NATURAL,
            &fonts(),
            &p,
            false,
        );
        assert_eq!(
            (r.node.width, r.node.height, r.node.depth),
            (12 * UNITY, 7 * UNITY, 2 * UNITY)
        );
        assert_eq!(r.node.glue, GlueSet::default());
        assert_eq!(r.report, None);
    }

    #[test]
    fn stretch_and_underfull() {
        let p = PackParams::default();
        let list = vec![
            ch(65),
            Node::glue(Glue::new(3 * UNITY, UNITY, UNITY)),
            ch(103),
        ];
        let r = hpack(list.clone(), Spec::Exactly(14 * UNITY), &fonts(), &p, false);
        assert_eq!(r.node.glue.sign, GlueSign::Stretching);
        assert!((r.node.glue.ratio - 2.0).abs() < f64::EPSILON);
        assert_eq!(r.badness, 800);
        assert_eq!(r.report, None);
        let r = hpack(
            list.clone(),
            Spec::Additional(3 * UNITY),
            &fonts(),
            &p,
            false,
        );
        assert_eq!(r.badness, 2698);
        assert_eq!(r.report, Some(Report::Underfull(2698)));
        let tight = PackParams { badness: 50, ..p };
        let r = hpack(list, Spec::Additional(UNITY), &fonts(), &tight, false);
        assert_eq!(r.report, Some(Report::Loose(100)));
    }

    #[test]
    fn overfull_gets_rule() {
        let p = PackParams::default();
        let list = vec![
            ch(65),
            Node::glue(Glue::new(3 * UNITY, UNITY, UNITY)),
            ch(103),
        ];
        let r = hpack(list, Spec::Exactly(10 * UNITY), &fonts(), &p, false);
        assert_eq!(r.report, Some(Report::Overfull(UNITY)));
        assert_eq!(r.badness, 1_000_000);
        assert!((r.node.glue.ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(r.node.list.len(), 4);
        assert!(matches!(
            r.node.list[3],
            Node::Rule(Rule { width: 327_680, .. })
        ));
    }

    #[test]
    fn infinite_glue_wins() {
        let p = PackParams::default();
        let fil = Glue {
            stretch: UNITY,
            stretch_order: Order::Fil,
            ..Glue::ZERO
        };
        let list = vec![
            ch(65),
            Node::glue(Glue::new(0, 9 * UNITY, 0)),
            Node::glue(fil),
        ];
        let r = hpack(list, Spec::Exactly(100 * UNITY), &fonts(), &p, false);
        assert_eq!(r.node.glue.order, Order::Fil);
        assert!((r.node.glue.ratio - 95.0).abs() < f64::EPSILON);
        assert_eq!((r.badness, r.report), (0, None));
    }

    #[test]
    fn shifted_boxes_and_adjustments() {
        let p = PackParams::default();
        let inner = BoxNode {
            width: UNITY,
            height: 3 * UNITY,
            depth: UNITY,
            shift: 2 * UNITY,
            ..BoxNode::default()
        };
        let list = vec![
            Node::Box(inner),
            Node::Mark(7),
            Node::Adjust(vec![Node::Penalty(5), Node::Penalty(6)]),
            Node::Kern(UNITY, crate::node::KernKind::Explicit),
        ];
        let r = hpack(list, NATURAL, &fonts(), &p, true);
        assert_eq!(
            (r.node.width, r.node.height, r.node.depth),
            (2 * UNITY, UNITY, 3 * UNITY)
        );
        assert_eq!(
            r.adjustments,
            vec![Node::Mark(7), Node::Penalty(5), Node::Penalty(6)]
        );
        assert_eq!(r.node.list.len(), 2);
    }

    #[test]
    fn vpack_depth_limit_and_leaders() {
        let p = PackParams::default();
        let b = |h, d, w| {
            Node::Box(BoxNode {
                kind: BoxKind::H,
                width: w,
                height: h,
                depth: d,
                ..BoxNode::default()
            })
        };
        let leader = Node::Glue(GlueNode {
            spec: Glue::new(2 * UNITY, 0, 0),
            kind: GlueKind::ALeaders,
            leader: Some(Box::new(Node::Rule(Rule {
                width: 20 * UNITY,
                height: RUNNING,
                depth: RUNNING,
            }))),
        });
        let list = vec![
            b(5 * UNITY, 2 * UNITY, 10 * UNITY),
            leader,
            b(4 * UNITY, 3 * UNITY, 8 * UNITY),
        ];
        let r = vpackage(list.clone(), NATURAL, UNITY, &p);
        // 5+2+2+4 = 13, depth 3 limited to 1: height 15
        assert_eq!(
            (r.node.height, r.node.depth, r.node.width),
            (15 * UNITY, UNITY, 20 * UNITY)
        );
        let r = vpack(list, Spec::Exactly(10 * UNITY), &p);
        assert_eq!(r.report, Some(Report::Overfull(3 * UNITY)));
    }
}
