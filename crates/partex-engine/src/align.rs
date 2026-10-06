//! Finishing an alignment (tex.web part 37, §800–§812).
//!
//! When `\halign` or `\valign` ends, the column widths are known: the
//! widest entry of each column, and the spans spread over the columns
//! they cover. [`preamble`] makes the preamble list of those widths, the
//! caller packs it to the requested size (§804, `hpack` or `vpack`, a
//! routine of its own and a recorded call, DESIGN 7.17.2), and
//! [`set_rows`] sets every row and entry to the packed prototype, as
//! functions of the columns, the tabskip glue and the list of rows.

use alloc::vec::Vec;

use crate::Scaled;
use crate::node::{BoxNode, GlueSign, GlueSpec, Node, Order, RUNNING, Unset};
use crate::pack::Confusion;
use crate::scaled::zround;

/// §769: the width of a column that no row used.
pub const NULL_FLAG: Scaled = -0o10000000000;
/// §224: `tab_skip_code`.
const TAB_SKIP: u8 = 11;

/// §770, §797: a column of the preamble.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Column {
    /// The widest entry (`NULL_FLAG`: none).
    pub width: Scaled,
    /// `(n, w)`: entries starting here span `n+1` columns and need width
    /// `w`; sorted by `n`, all `n>0`.
    pub spans: Vec<(u16, Scaled)>,
}

crate::persist_struct!(Column { width, spans });

/// §800: the parameters of the alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// `\valign` (rows are vertical).
    pub vertical: bool,
    /// `\displayindent` in displays, else 0.
    pub shift: Scaled,
    /// e-TeX's engines (pdfTeX, XeTeX): the rows are display lines (their
    /// `box_lr` is `dlist`).
    pub display: bool,
    /// e-TeX's engines (a cell's `box_lr` is 0, not its span count).
    pub etex: bool,
}

fn unset_node(width: Scaled, height: Scaled) -> Node {
    Node::Unset(alloc::boxed::Box::new(Unset {
        width,
        height,
        ..Unset::default()
    }))
}

/// §801–§804: determine the widths of `columns`, whose tabskip glue is
/// `tabskips` (one more than columns; the glue after a column no entry
/// used becomes zero, §802), and make the preamble list for the caller
/// to pack (§804): the tabskip glue around an unset node of each
/// column's width, or, for `\valign` (`vertical`), of its height.
///
/// # Errors
/// A confusion if `tabskips` does not fit `columns`.
pub fn preamble(
    mut columns: Vec<Column>,
    tabskips: &mut [GlueSpec],
    vertical: bool,
) -> Result<Vec<Node>, Confusion> {
    if tabskips.len() != columns.len() + 1 {
        return Err(Confusion("align"));
    }
    // §801: determine the column widths.
    for j in 0..columns.len() {
        if columns[j].width == NULL_FLAG {
            // §802
            columns[j].width = 0;
            tabskips[j + 1] = GlueSpec::ZERO_GLUE;
        }
        let spans = core::mem::take(&mut columns[j].spans);
        if spans.is_empty() {
            continue;
        }
        // §803: merge the spans into the next column's.
        let t = columns[j].width + tabskips[j + 1].width;
        let next = columns.get_mut(j + 1).ok_or(Confusion("align"))?;
        for (n, w) in spans {
            let w = w - t;
            if n == 1 {
                if w > next.width {
                    next.width = w;
                }
            } else {
                match next.spans.binary_search_by_key(&(n - 1), |s| s.0) {
                    Ok(k) => {
                        if w > next.spans[k].1 {
                            next.spans[k].1 = w;
                        }
                    }
                    Err(k) => next.spans.insert(k, (n - 1, w)),
                }
            }
        }
    }
    // §804: the preamble list, to be packaged.
    let mut pre = Vec::with_capacity(2 * columns.len() + 1);
    for (j, c) in columns.iter().enumerate() {
        pre.push(tab(tabskips[j]));
        pre.push(if vertical {
            unset_node(0, c.width)
        } else {
            unset_node(c.width, 0)
        });
    }
    pre.push(tab(tabskips[columns.len()]));
    Ok(pre)
}

/// §804: the packed preamble of a `\valign` with its unset nodes' heights
/// as their widths again (the widths [`set_rows`] reads).
pub fn prototype_widths(p: &mut BoxNode) {
    for n in &mut p.list {
        if let Node::Unset(u) = n {
            u.width = u.height;
            u.height = 0;
        }
    }
}

/// §805–§806: set the glue in all the unset boxes of `list` to the packed
/// preamble `p` (its unset nodes' widths the columns', [`prototype_widths`]);
/// a rule's running dimensions become `p`'s, and in a display
/// (`params.shift` nonzero) `pack_rule` packs it (§806: `hpack(q,
/// natural)`, a routine of the caller's) to be shifted.
///
/// # Errors
/// A confusion if a row's entries do not fit `p`.
pub fn set_rows(
    list: Vec<Node>,
    p: &BoxNode,
    tabskips: &[GlueSpec],
    params: Params,
    mut pack_rule: impl FnMut(Node) -> BoxNode,
) -> Result<Vec<Node>, Confusion> {
    // §805: set the glue in all the unset boxes of the list.
    let mut out = Vec::with_capacity(list.len());
    for n in list {
        match n {
            Node::Unset(q) => {
                out.push(Node::Box((set_unset_box(*q, p, tabskips, params)?).share()));
            }
            Node::Rule {
                mut width,
                mut height,
                mut depth,
                sync,
            } => {
                // §806
                if width == RUNNING {
                    width = p.width;
                }
                if height == RUNNING {
                    height = p.height;
                }
                if depth == RUNNING {
                    depth = p.depth;
                }
                // (the rule changed in place: its place kept)
                let r = Node::Rule {
                    width,
                    height,
                    depth,
                    sync,
                };
                if params.shift == 0 {
                    out.push(r);
                } else {
                    let mut b = pack_rule(r);
                    b.shift = params.shift;
                    out.push(Node::Box(b.share()));
                }
            }
            n => out.push(n),
        }
    }
    Ok(out)
}

fn tab(spec: GlueSpec) -> Node {
    Node::Glue {
        spec,
        subtype: TAB_SKIP + 1,
        sync: crate::origin::Side(0),
    }
}

/// The column widths of the prototype box `p`.
fn widths(p: &BoxNode) -> Vec<Scaled> {
    p.list
        .iter()
        .filter_map(|n| match n {
            Node::Unset(u) => Some(u.width),
            _ => None,
        })
        .collect()
}

/// §807: set the unset row `q` and the entries in it.
fn set_unset_box(
    q: Unset,
    p: &BoxNode,
    tabskips: &[GlueSpec],
    params: Params,
) -> Result<BoxNode, Confusion> {
    let widths = widths(p);
    let mut row = BoxNode {
        vertical: params.vertical,
        width: q.width,
        height: q.height,
        depth: q.depth,
        shift: params.shift,
        glue_set: p.glue_set,
        glue_sign: p.glue_sign,
        glue_order: p.glue_order,
        subtype: if params.display {
            crate::lr::DLIST
        } else {
            u8::try_from(q.span_count).unwrap_or(0)
        },
        list: Vec::with_capacity(q.list.len()),
        seal: None,
        ver: 0,
        // (the unset row made a box: its place kept)
        sync: q.sync,
    };
    if params.vertical {
        row.height = p.height;
    } else {
        row.width = p.width;
    }
    let mut col = 0; // the column of the next entry
    let mut at_entry = false; // after the tabskip glue before an entry
    for n in q.list {
        if !at_entry {
            row.list.push(n);
            at_entry = true;
            continue;
        }
        let Node::Unset(r) = n else {
            return Err(Confusion("align entry"));
        };
        // §808
        let w = *widths.get(col).ok_or(Confusion("align entry"))?;
        let mut t = w;
        let mut blanks = Vec::new();
        for _ in 0..r.span_count {
            // §809
            col += 1;
            let v = tabskips[col];
            blanks.push(tab(v));
            t += v.width;
            match p.glue_sign {
                GlueSign::Stretching if v.stretch_order == p.glue_order => {
                    t += zround(p.glue_set * f64::from(v.stretch));
                }
                GlueSign::Shrinking if v.shrink_order == p.glue_order => {
                    t -= zround(p.glue_set * f64::from(v.shrink));
                }
                _ => {}
            }
            let ws = *widths.get(col).ok_or(Confusion("align entry"))?;
            t += ws;
            blanks.push(Node::Box(
                (if params.vertical {
                    BoxNode {
                        vertical: true,
                        height: ws,
                        ..BoxNode::default()
                    }
                } else {
                    BoxNode {
                        width: ws,
                        ..BoxNode::default()
                    }
                })
                .share(),
            ));
        }
        let mut b = BoxNode {
            vertical: params.vertical,
            width: r.width,
            height: r.height,
            depth: r.depth,
            shift: 0,
            glue_set: 0.0,
            glue_sign: GlueSign::Normal,
            glue_order: Order::Normal,
            subtype: if params.etex {
                0 // (e-TeX clears the span count, for `ship_out`)
            } else {
                u8::try_from(r.span_count).unwrap_or(0)
            },
            list: Vec::new(),
            seal: None,
            ver: 0,
            sync: r.sync,
        };
        let d;
        if params.vertical {
            // §811
            b.width = q.width;
            d = r.height;
            b.height = w;
        } else {
            // §810
            b.height = q.height;
            b.depth = q.depth;
            d = r.width;
            b.width = w;
        }
        set_unset_glue(&mut b, &r, t, d);
        b.list = r.list;
        row.list.push(Node::Box(b.share()));
        row.list.extend(blanks);
        col += 1;
        at_entry = false;
    }
    Ok(row)
}

/// §810, §811: set the glue of entry `r` of natural size `d` as if its
/// size were `t`.
#[allow(clippy::comparison_chain)] // as §810 is written
fn set_unset_glue(b: &mut BoxNode, r: &Unset, t: Scaled, d: Scaled) {
    if t == d {
        b.glue_sign = GlueSign::Normal;
        b.glue_order = Order::Normal;
        b.glue_set = 0.0;
    } else if t > d {
        b.glue_sign = GlueSign::Stretching;
        b.glue_order = r.stretch_order;
        b.glue_set = if r.stretch == 0 {
            0.0
        } else {
            f64::from(t - d) / f64::from(r.stretch)
        };
    } else {
        b.glue_order = r.shrink_order;
        b.glue_sign = GlueSign::Shrinking;
        b.glue_set = if r.shrink == 0 {
            0.0
        } else if b.glue_order == Order::Normal && d - t > r.shrink {
            1.0
        } else {
            f64::from(d - t) / f64::from(r.shrink)
        };
    }
}
