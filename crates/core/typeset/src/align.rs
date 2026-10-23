//! Alignment (§768–§812), the pure parts: packaging an entry and a row
//! into unset boxes (§796–§799), the final column widths with spanned
//! columns (§801–§803), the prototype row (§804) and setting the glue of
//! every row and entry (§805–§811).
//!
//! The caller keeps one [`AlignColumn`] per preamble column and the
//! tabskip glue between columns (`n + 1` of them for `n` columns).

use std::collections::BTreeMap;

use crate::arith::Scaled;
use crate::fonts::Fonts;
use crate::glue::{Glue, GlueSet, GlueSign, Order, OrderTotals};
use crate::node::{BoxKind, BoxNode, GlueKind, GlueNode, List, Node, RUNNING, Unset};
use crate::pack::{NATURAL, PackParams, Packed, Spec, hpack, vpack, vpackage};

/// `null_flag` (§138): no entry has been seen for a column.
pub const NULL_FLAG: Scaled = RUNNING;

/// `tab_skip_code` (§224).
pub const TAB_SKIP: u8 = 11;

/// An alignrecord's widths (§797): the widest entry of this column
/// alone, and the widest entries that start here and span more
/// columns, by the number of further columns spanned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignColumn {
    /// `width`: `w_ii`, [`NULL_FLAG`] if no entry was seen.
    pub width: Scaled,
    /// The span nodes: `j - i` to `w_ij`.
    pub spans: BTreeMap<u16, Scaled>,
}

impl Default for AlignColumn {
    fn default() -> Self {
        AlignColumn {
            width: NULL_FLAG,
            spans: BTreeMap::new(),
        }
    }
}

impl AlignColumn {
    /// Records an entry of width `w` spanning `span` further columns
    /// (§796, §798).
    pub fn record(&mut self, span: u16, w: Scaled) {
        if span == 0 {
            if w > self.width {
                self.width = w;
            }
        } else {
            let e = self.spans.entry(span).or_insert(w);
            if *e < w {
                *e = w;
            }
        }
    }
}

/// `round` as web2c computes it (`zround`, texmfmp.c).
fn zround(r: f64) -> Scaled {
    if r > 2_147_483_647.0 {
        i32::MAX
    } else if r < -2_147_483_647.0 {
        -i32::MAX
    } else {
        // in range by the tests above
        #[allow(clippy::cast_possible_truncation)]
        if r >= 0.0 {
            (r + 0.5).floor() as i32
        } else {
            (r - 0.5).ceil() as i32
        }
    }
}

/// Packages an entry (§796): `hpack` natural for `\halign` (its
/// adjustments are returned), `vpackage` natural with depth limit 0 for
/// `\valign`; the width (or height) is recorded in `column` for an entry
/// spanning `span` further columns.
#[must_use]
pub fn pack_entry(
    list: List,
    halign: bool,
    span: u16,
    column: &mut AlignColumn,
    fonts: &dyn Fonts,
    params: &PackParams,
) -> (Unset, List) {
    let (u, adjust) = if halign {
        let p = hpack(list, NATURAL, fonts, params, true);
        (p.node, p.adjustments)
    } else {
        (vpackage(list, NATURAL, 0, params).node, List::new())
    };
    let w = if halign { u.width } else { u.height };
    column.record(span, w);
    let mut stretch = OrderTotals::default();
    let mut shrink = OrderTotals::default();
    totals(&u.list, halign, &mut stretch, &mut shrink);
    let so = stretch.highest();
    let sh = shrink.highest();
    (
        Unset {
            width: u.width,
            height: u.height,
            depth: u.depth,
            span_count: span,
            stretch: stretch.get(so),
            stretch_order: so,
            shrink: shrink.get(sh),
            shrink_order: sh,
            list: u.list,
        },
        adjust,
    )
}

/// The glue totals hpack or vpack found (§656, §671): only glue counts.
fn totals(list: &[Node], halign: bool, stretch: &mut OrderTotals, shrink: &mut OrderTotals) {
    for n in list {
        match n {
            Node::Glue(g) => {
                stretch.add(g.spec.stretch_order, g.spec.stretch);
                shrink.add(g.spec.shrink_order, g.spec.shrink);
            }
            Node::Disc(d) if halign => totals(&d.replace, halign, stretch, shrink),
            _ => {}
        }
    }
}

/// `fin_row` (§799): a row as an unset box; its glue fields are zero.
#[must_use]
pub fn pack_row(list: List, halign: bool, fonts: &dyn Fonts, params: &PackParams) -> Unset {
    let b = if halign {
        hpack(list, NATURAL, fonts, params, false).node
    } else {
        vpack(list, NATURAL, params).node
    };
    Unset {
        width: b.width,
        height: b.height,
        depth: b.depth,
        list: b.list,
        ..Unset::default()
    }
}

/// §801–§803: the final column widths. A column no entry used alone gets
/// width zero and zero tabskip after it.
///
/// # Panics
/// If `tabskips.len() != columns.len() + 1`.
pub fn column_widths(columns: &mut [AlignColumn], tabskips: &mut [Glue]) -> Vec<Scaled> {
    assert_eq!(tabskips.len(), columns.len() + 1);
    let n = columns.len();
    for i in 0..n {
        if columns[i].width == NULL_FLAG {
            // §802
            columns[i].width = 0;
            tabskips[i + 1] = Glue::ZERO;
        }
        let spans = std::mem::take(&mut columns[i].spans);
        if spans.is_empty() {
            continue;
        }
        // §803
        let t = columns[i].width + tabskips[i + 1].width;
        if i + 1 < n {
            let next = &mut columns[i + 1];
            for (span, w) in spans {
                let w = w - t;
                if span == 1 {
                    if w > next.width {
                        next.width = w;
                    }
                } else {
                    let e = next.spans.entry(span - 1).or_insert(w);
                    if w > *e {
                        *e = w;
                    }
                }
            }
        }
    }
    columns.iter().map(|c| c.width).collect()
}

/// §804: the prototype row: tabskip glue and unset boxes of the column
/// widths, packaged to `spec` (`\halign to`, `spread`) with no overfull
/// rule. For `\valign` the widths are heights.
#[must_use]
pub fn prototype(
    widths: &[Scaled],
    tabskips: &[Glue],
    spec: Spec,
    halign: bool,
    fonts: &dyn Fonts,
    params: &PackParams,
) -> Packed {
    let mut list = List::with_capacity(2 * widths.len() + 1);
    for (i, w) in widths.iter().enumerate() {
        list.push(tab_glue(tabskips[i]));
        list.push(Node::Unset(if halign {
            Box::new(Unset {
                width: *w,
                ..Unset::default()
            })
        } else {
            Box::new(Unset {
                height: *w,
                ..Unset::default()
            })
        }));
    }
    list.push(tab_glue(tabskips[widths.len()]));
    if halign {
        let p = PackParams {
            overfull_rule: 0,
            ..*params
        };
        hpack(list, spec, fonts, &p, false)
    } else {
        let mut packed = vpack(list, spec, params);
        for n in &mut packed.node.list {
            if let Node::Unset(u) = n {
                u.width = u.height;
                u.height = 0;
            }
        }
        packed
    }
}

fn tab_glue(g: Glue) -> Node {
    Node::Glue(GlueNode {
        spec: g,
        kind: GlueKind::Param(TAB_SKIP),
        leader: None,
    })
}

/// §805–§811: sets the rows (unset boxes) of `list` against the
/// prototype `proto`, makes running rule dimensions those of the
/// prototype, and shifts both by `offset` (`display_indent` in displays).
#[must_use]
pub fn set_alignment(
    list: List,
    proto: &BoxNode,
    halign: bool,
    offset: Scaled,
    fonts: &dyn Fonts,
) -> List {
    let mut out = List::with_capacity(list.len());
    for q in list {
        match q {
            Node::Unset(row) => out.push(Node::Box(Box::new(set_row(*row, proto, halign, offset)))),
            Node::Rule(mut r) => {
                // §806
                if r.width == RUNNING {
                    r.width = proto.width;
                }
                if r.height == RUNNING {
                    r.height = proto.height;
                }
                if r.depth == RUNNING {
                    r.depth = proto.depth;
                }
                if offset == 0 {
                    out.push(Node::Rule(r));
                } else {
                    let mut b = hpack(
                        vec![Node::Rule(r)],
                        NATURAL,
                        fonts,
                        &PackParams::default(),
                        false,
                    )
                    .node;
                    b.shift = offset;
                    out.push(Node::Box(Box::new(b)));
                }
            }
            n => out.push(n),
        }
    }
    out
}

/// §807–§811 for one row.
#[allow(clippy::too_many_lines)]
fn set_row(row: Unset, p: &BoxNode, halign: bool, o: Scaled) -> BoxNode {
    let mut q = BoxNode {
        kind: if halign { BoxKind::H } else { BoxKind::V },
        width: if halign { p.width } else { row.width },
        height: if halign { row.height } else { p.height },
        depth: row.depth,
        shift: o,
        glue: p.glue,
        list: Vec::with_capacity(row.list.len()),
    };
    let proto: &[Node] = &p.list;
    let mut si = 1; // `s`, in the prototype
    let mut entries = row.list.into_iter();
    // the row's first node is tabskip glue
    if let Some(g) = entries.next() {
        q.list.push(g);
    }
    while let Some(r) = entries.next() {
        let Node::Unset(r) = r else {
            q.list.push(r);
            continue;
        };
        // §808
        let width_of = |n: &Node| match n {
            Node::Unset(u) => u.width,
            _ => 0,
        };
        let mut t = width_of(&proto[si]);
        let w = t;
        let mut extra = List::new();
        for _ in 0..r.span_count {
            // §809
            si += 1;
            let Node::Glue(g) = &proto[si] else {
                panic!("align: prototype out of step")
            };
            let v = g.spec;
            extra.push(tab_glue(v));
            t += v.width;
            match p.glue.sign {
                GlueSign::Stretching if v.stretch_order == p.glue.order => {
                    t += zround(p.glue.ratio * f64::from(v.stretch));
                }
                GlueSign::Shrinking if v.shrink_order == p.glue.order => {
                    t -= zround(p.glue.ratio * f64::from(v.shrink));
                }
                _ => {}
            }
            si += 1;
            let ws = width_of(&proto[si]);
            extra.push(Node::Box(Box::new(if halign {
                BoxNode {
                    width: ws,
                    ..BoxNode::default()
                }
            } else {
                BoxNode {
                    kind: BoxKind::V,
                    height: ws,
                    ..BoxNode::default()
                }
            })));
            t += ws;
        }
        // §810, §811
        let natural = if halign { r.width } else { r.height };
        let glue = match t.cmp(&natural) {
            std::cmp::Ordering::Equal => GlueSet::default(),
            std::cmp::Ordering::Greater => GlueSet {
                ratio: if r.stretch == 0 {
                    0.0
                } else {
                    f64::from(t - natural) / f64::from(r.stretch)
                },
                sign: GlueSign::Stretching,
                order: r.stretch_order,
            },
            std::cmp::Ordering::Less => GlueSet {
                ratio: if r.shrink == 0 {
                    0.0
                } else if r.shrink_order == Order::Normal && natural - t > r.shrink {
                    1.0
                } else {
                    f64::from(natural - t) / f64::from(r.shrink)
                },
                sign: GlueSign::Shrinking,
                order: r.shrink_order,
            },
        };
        let b = if halign {
            BoxNode {
                kind: BoxKind::H,
                width: w,
                height: q.height,
                depth: q.depth,
                shift: 0,
                glue,
                list: r.list,
            }
        } else {
            BoxNode {
                kind: BoxKind::V,
                width: q.width,
                height: w,
                depth: r.depth,
                shift: 0,
                glue,
                list: r.list,
            }
        };
        q.list.push(Node::Box(Box::new(b)));
        q.list.extend(extra);
        // the tabskip glue after the entry (or spanned group)
        if let Some(g) = entries.next() {
            q.list.push(g);
        }
        si += 2;
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testfont::TestFonts;

    const PT: Scaled = 0x1_0000;

    #[test]
    fn spans_widen_the_last_column_they_cover() {
        // three columns: a 10pt and 20pt entry alone, a 50pt entry over
        // columns 1-2, a 100pt entry over all three; tabskips 1pt
        let mut cols = vec![AlignColumn::default(); 3];
        cols[0].record(0, 10 * PT);
        cols[1].record(0, 20 * PT);
        cols[0].record(1, 50 * PT);
        cols[0].record(2, 100 * PT);
        let mut tabs = vec![Glue::fixed(PT); 4];
        let w = column_widths(&mut cols, &mut tabs);
        // w1 = 10; w2 = max(20, 50-11) = 39; w3 = 100-11-40 = 49
        assert_eq!(w, vec![10 * PT, 39 * PT, 49 * PT]);
        assert_eq!(tabs[3], Glue::fixed(PT));
    }

    #[test]
    fn unused_columns_vanish_with_their_tabskip() {
        let mut cols = vec![AlignColumn::default(); 2];
        cols[0].record(1, 30 * PT);
        let mut tabs = vec![Glue::fixed(PT); 3];
        let w = column_widths(&mut cols, &mut tabs);
        // column 1 had no entry of its own: width 0 and tabskip 0, so
        // column 2 gets all 30pt
        assert_eq!(w, vec![0, 30 * PT]);
        assert_eq!(tabs[1], Glue::ZERO);
    }

    #[test]
    fn rows_take_the_prototype_and_entries_their_columns() {
        let fonts = TestFonts::default();
        let pp = PackParams::default();
        let rule = |w| {
            Node::Rule(crate::node::Rule {
                width: w,
                height: 5 * PT,
                depth: PT,
            })
        };
        let mut cols = vec![AlignColumn::default(); 2];
        let hss = Node::glue(Glue {
            stretch: PT,
            stretch_order: Order::Fil,
            shrink: PT,
            shrink_order: Order::Fil,
            ..Glue::ZERO
        });
        let (e1, _) = pack_entry(
            vec![rule(10 * PT), hss.clone()],
            true,
            0,
            &mut cols[0],
            &fonts,
            &pp,
        );
        let (e2, _) = pack_entry(vec![rule(20 * PT)], true, 0, &mut cols[1], &fonts, &pp);
        let (e3, _) = pack_entry(vec![rule(4 * PT), hss], true, 1, &mut cols[0], &fonts, &pp);
        let tab = Glue::new(2 * PT, PT, 0);
        let row = |es: Vec<Unset>| {
            let mut l = vec![tab_glue(tab)];
            for e in es {
                l.push(Node::Unset(Box::new(e)));
                l.push(tab_glue(tab));
            }
            pack_row(l, true, &fonts, &pp)
        };
        let r1 = row(vec![e1, e2]);
        let r2 = row(vec![e3]);
        let mut tabs = vec![tab; 3];
        let w = column_widths(&mut cols, &mut tabs);
        assert_eq!(w, vec![10 * PT, 20 * PT]);
        let proto = prototype(&w, &tabs, Spec::Exactly(39 * PT), true, &fonts, &pp).node;
        assert_eq!(proto.width, 39 * PT);
        assert!((proto.glue.ratio - 1.0).abs() < 1e-12);
        let out = set_alignment(
            vec![Node::Unset(Box::new(r1)), Node::Unset(Box::new(r2))],
            &proto,
            true,
            0,
            &fonts,
        );
        let Node::Box(b2) = &out[1] else { panic!() };
        assert_eq!((b2.width, b2.height, b2.depth), (39 * PT, 5 * PT, PT));
        // the spanning entry: width of column 1, glue set as for 10+2+1+20pt
        let Node::Box(e) = &b2.list[1] else { panic!() };
        assert_eq!(e.width, 10 * PT);
        let t = 10 * PT + 2 * PT + PT + 20 * PT;
        assert!((e.glue.ratio - f64::from(t - 4 * PT) / f64::from(PT)).abs() < 1e-9);
        // followed by tabskip glue and an empty box for column 2
        assert!(matches!(&b2.list[3], Node::Box(x) if x.width == 20 * PT && x.list.is_empty()));
        assert_eq!(b2.list.len(), 5);
    }
}
