//! Boxes, glue and the other nodes of lists (§133–§160) as values, and
//! back as `phitex-typeset` nodes.
//!
//! A node is a `List` whose first element is its type. Marks and whatsits
//! carry their token lists; as typeset nodes they are numbers into a
//! [`Side`] table.

use phitex_defuse::{Value, ValueId, Values};
use phitex_typeset::glue::{Glue, GlueSet, GlueSign};
use phitex_typeset::node::{
    BoxKind, BoxNode, Disc, GlueKind, GlueNode, Insert, KernKind, Ligature, List, MathKind, Node,
    Rule, Unset,
};

use crate::val::{glue_of, glue_value, int_of, list_of, node_id};

pub(crate) const T_CHAR: i64 = 0;
pub(crate) const T_BOX: i64 = 1;
pub(crate) const T_RULE: i64 = 2;
pub(crate) const T_INS: i64 = 3;
pub(crate) const T_MARK: i64 = 4;
pub(crate) const T_ADJUST: i64 = 5;
pub(crate) const T_LIG: i64 = 6;
pub(crate) const T_DISC: i64 = 7;
pub(crate) const T_WHATSIT: i64 = 8;
pub(crate) const T_LANGUAGE: i64 = 9;
pub(crate) const T_MATH: i64 = 10;
pub(crate) const T_GLUE: i64 = 11;
pub(crate) const T_KERN: i64 = 12;
pub(crate) const T_PENALTY: i64 = 13;
/// `[T_UNSET, span, width, height, depth, stretch, its order, shrink,
/// its order, list]` (dimensions where a box has them).
pub(crate) const T_UNSET: i64 = 14;
/// "Remove the last node of the list" (`\unskip` and its kind, §1105).
pub(crate) const T_DELETE: i64 = 99;
/// A point where tex.web calls `build_page` (§994).
pub(crate) const T_BUILD_PAGE: i64 = 98;
/// A space factor transition in a horizontal list (§1034, §1043): `[t,
/// kind, n]`, kind 0 sets the factor to `n`, kind 1 is §1034's update by
/// an `sf_code` `n` above 1000 (1000 if the factor was below 1000, else
/// `n`). Not a node: the list's reader folds the transitions
/// ([`crate::run::Run::list_items`]).
pub(crate) const T_SF: i64 = 97;
/// An interword space whose glue waits for the space factor (§1041–§1044):
/// `[t, spaceskip, xspaceskip, fontdimen 2, 3, 4, 7]`, the values in force
/// where the space was; the list's reader makes the glue.
pub(crate) const T_SPACE: i64 = 96;
/// A box appended to a vertical list (§679): `[t, box, baselineskip,
/// lineskip, lineskiplimit]`, the parameters in force where it was; the
/// list's reader puts the interline glue before it from the previous
/// depth.
pub(crate) const T_VBOX: i64 = 95;
/// A previous depth transition (`\\prevdepth`, a rule, an alignment's
/// end): `[t, depth]`.
pub(crate) const T_PD: i64 = 94;
/// `\end` (§1054): `[tag, null box, fill glue, penalty]`, the material
/// the page builder appends each time round until the page is empty.
pub(crate) const T_END: i64 = 93;
/// The page parameters a run changed, for the page builder's fold:
/// `[tag, vsize, maxdepth, topskip, holdinginserts]` (§980).
pub(crate) const T_PAGE_PARAMS: i64 = 92;
/// Where a document node read the marks (§386): `[tag, node]`; the page
/// builder's fold publishes the marks in force there as the node's
/// `marks:before:<node>` loop-back.
pub(crate) const T_MARKS_AT: i64 = 91;
/// A document node's `\immediate\write` or `\immediate\openout` to a
/// job-written file (§1370, §1374): `[tag, entry]`, the `streams` entry
/// (`[name]` or `[name, text]`); the page builder's fold places it
/// among the shipped pages, so the writer assembles the file in TeX's
/// order.
pub(crate) const T_WRITE_AT: i64 = 88;

/// Marks and whatsits met while decoding, numbered.
#[derive(Clone, Default, Debug)]
pub(crate) struct Side {
    pub marks: Vec<ValueId>,
    pub whats: Vec<ValueId>,
}

fn i(v: &Values, x: i64) -> ValueId {
    crate::val::int_id(v, x)
}

fn n32(v: &Values, x: ValueId) -> i32 {
    i32::try_from(int_of(v, x)).unwrap_or(0)
}

pub(crate) fn tag(v: &Values, x: ValueId) -> i64 {
    match &*v.get(x) {
        Value::List(xs) => xs.first().map_or(-1, |&t| int_of(v, t)),
        _ => -1,
    }
}

fn glue_kind_code(k: GlueKind) -> i64 {
    match k {
        GlueKind::Normal => 0,
        GlueKind::CondMath => 1,
        GlueKind::Mu => 2,
        GlueKind::ALeaders => 3,
        GlueKind::CLeaders => 4,
        GlueKind::XLeaders => 5,
        GlueKind::Param(c) => 100 + i64::from(c),
    }
}

fn glue_kind(c: i64) -> GlueKind {
    match c {
        1 => GlueKind::CondMath,
        2 => GlueKind::Mu,
        3 => GlueKind::ALeaders,
        4 => GlueKind::CLeaders,
        5 => GlueKind::XLeaders,
        c if c >= 100 => GlueKind::Param(u8::try_from(c - 100).unwrap_or(0)),
        _ => GlueKind::Normal,
    }
}

fn kern_kind(c: i64) -> KernKind {
    match c {
        1 => KernKind::Explicit,
        2 => KernKind::AccKern,
        3 => KernKind::Mu,
        _ => KernKind::Normal,
    }
}

fn kern_code(k: KernKind) -> i64 {
    match k {
        KernKind::Normal => 0,
        KernKind::Explicit => 1,
        KernKind::AccKern => 2,
        KernKind::Mu => 3,
    }
}

pub(crate) fn encode_list(v: &Values, list: &[Node], side: &Side) -> ValueId {
    v.list(list.iter().map(|n| encode(v, n, side)).collect())
}

pub(crate) fn encode_box(v: &Values, b: &BoxNode, side: &Side) -> ValueId {
    v.list(vec![
        i(v, T_BOX),
        i(v, i64::from(b.kind != BoxKind::H)),
        i(v, i64::from(b.width)),
        i(v, i64::from(b.height)),
        i(v, i64::from(b.depth)),
        i(v, i64::from(b.shift)),
        i(
            v,
            match b.glue.sign {
                GlueSign::Normal => 0,
                GlueSign::Stretching => 1,
                GlueSign::Shrinking => 2,
            },
        ),
        i(v, b.glue.order as i64),
        i(v, i64::from_ne_bytes(b.glue.ratio.to_bits().to_ne_bytes())),
        encode_list(v, &b.list, side),
    ])
}

/// A node of fixed shape (a character, glue without leaders, a kern, a
/// penalty), through the worker's cache in a run ([`node_id`]).
fn encode_fixed(v: &Values, n: &Node) -> Option<ValueId> {
    Some(match n {
        Node::Char { font, ch } => {
            let (f, c) = (i64::from(*font), i64::from(*ch));
            node_id(v, (T_CHAR, [f, c, 0], None), || {
                v.list(vec![i(v, T_CHAR), i(v, f), i(v, c)])
            })
        }
        Node::Glue(g) if g.leader.is_none() => {
            let k = glue_kind_code(g.kind);
            node_id(v, (T_GLUE, [k, 0, 0], Some(g.spec)), || {
                v.list(vec![
                    i(v, T_GLUE),
                    glue_value(v, &g.spec),
                    i(v, k),
                    ValueId::UNIT,
                ])
            })
        }
        Node::Kern(w, k) => {
            let (w, k) = (i64::from(*w), kern_code(*k));
            node_id(v, (T_KERN, [w, k, 0], None), || {
                v.list(vec![i(v, T_KERN), i(v, w), i(v, k)])
            })
        }
        Node::Penalty(p) => {
            let p = i64::from(*p);
            node_id(v, (T_PENALTY, [p, 0, 0], None), || {
                v.list(vec![i(v, T_PENALTY), i(v, p)])
            })
        }
        _ => return None,
    })
}

pub(crate) fn encode(v: &Values, n: &Node, side: &Side) -> ValueId {
    if let Some(x) = encode_fixed(v, n) {
        return x;
    }
    match n {
        Node::Char { .. } | Node::Kern(..) | Node::Penalty(_) => unreachable!("fixed shape"),
        Node::Box(b) => encode_box(v, b, side),
        Node::Rule(r) => v.list(vec![
            i(v, T_RULE),
            i(v, i64::from(r.width)),
            i(v, i64::from(r.height)),
            i(v, i64::from(r.depth)),
        ]),
        Node::Ins(x) => v.list(vec![
            i(v, T_INS),
            i(v, i64::from(x.number)),
            i(v, i64::from(x.height)),
            i(v, i64::from(x.depth)),
            glue_value(v, &x.split_top_skip),
            i(v, i64::from(x.float_cost)),
            encode_list(v, &x.list, side),
        ]),
        Node::Mark(k) => v.list(vec![
            i(v, T_MARK),
            side.marks
                .get(*k as usize)
                .copied()
                .unwrap_or(ValueId::UNIT),
        ]),
        Node::Adjust(l) => v.list(vec![i(v, T_ADJUST), encode_list(v, l, side)]),
        Node::Lig(l) => v.list(vec![
            i(v, T_LIG),
            i(v, i64::from(l.font)),
            i(v, i64::from(l.ch)),
            v.list(l.original.iter().map(|&c| i(v, i64::from(c))).collect()),
            i(v, i64::from(l.left_hit)),
            i(v, i64::from(l.right_hit)),
        ]),
        Node::Disc(d) => v.list(vec![
            i(v, T_DISC),
            encode_list(v, &d.pre, side),
            encode_list(v, &d.post, side),
            encode_list(v, &d.replace, side),
        ]),
        Node::Whatsit(k, dims) => {
            let mut xs = vec![
                i(v, T_WHATSIT),
                side.whats
                    .get(*k as usize)
                    .copied()
                    .unwrap_or(ValueId::UNIT),
            ];
            if let Some(s) = dims {
                xs.extend([s.width, s.height, s.depth].map(|d| i(v, i64::from(d))));
            }
            v.list(xs)
        }
        Node::Language { lang, lhm, rhm } => v.list(vec![
            i(v, T_LANGUAGE),
            i(v, i64::from(*lang)),
            i(v, i64::from(*lhm)),
            i(v, i64::from(*rhm)),
        ]),
        Node::Math(k, w) => v.list(vec![
            i(v, T_MATH),
            i(v, i64::from(*k == MathKind::After)),
            i(v, i64::from(*w)),
        ]),
        Node::Glue(g) => v.list(vec![
            i(v, T_GLUE),
            glue_value(v, &g.spec),
            i(v, glue_kind_code(g.kind)),
            g.leader
                .as_ref()
                .map_or(ValueId::UNIT, |l| encode(v, l, side)),
        ]),
        Node::Unset(u) => v.list(vec![
            i(v, T_UNSET),
            i(v, i64::from(u.span_count)),
            i(v, i64::from(u.width)),
            i(v, i64::from(u.height)),
            i(v, i64::from(u.depth)),
            i(v, i64::from(u.stretch)),
            i(v, u.stretch_order as i64),
            i(v, i64::from(u.shrink)),
            i(v, u.shrink_order as i64),
            encode_list(v, &u.list, side),
        ]),
    }
}

pub(crate) fn decode_list(v: &Values, x: ValueId, side: &mut Side) -> List {
    list_of(v, x)
        .into_iter()
        .filter_map(|n| decode(v, n, side))
        .collect()
}

pub(crate) fn decode_box(v: &Values, xs: &[ValueId], side: &mut Side) -> BoxNode {
    let g = |k: usize| xs.get(k).map_or(0, |&x| n32(v, x));
    BoxNode {
        kind: if g(1) == 0 { BoxKind::H } else { BoxKind::V },
        width: g(2),
        height: g(3),
        depth: g(4),
        shift: g(5),
        glue: GlueSet {
            sign: match g(6) {
                1 => GlueSign::Stretching,
                2 => GlueSign::Shrinking,
                _ => GlueSign::Normal,
            },
            order: crate::val::order(i64::from(g(7))),
            ratio: f64::from_bits(u64::from_ne_bytes(
                xs.get(8).map_or(0, |&x| int_of(v, x)).to_ne_bytes(),
            )),
        },
        list: xs
            .get(9)
            .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
    }
}

pub(crate) fn decode(v: &Values, x: ValueId, side: &mut Side) -> Option<Node> {
    if let Some(n) = crate::val::leaf_of(v, x) {
        return Some(n);
    }
    let n = decode_new(v, x, side)?;
    let leaf = match &n {
        Node::Char { .. }
        | Node::Rule(_)
        | Node::Kern(..)
        | Node::Penalty(_)
        | Node::Math(..)
        | Node::Language { .. } => true,
        Node::Glue(g) => g.leader.is_none(),
        _ => false,
    };
    if leaf {
        crate::val::keep_leaf(v, x, &n);
    }
    Some(n)
}

#[allow(clippy::too_many_lines)]
fn decode_new(v: &Values, x: ValueId, side: &mut Side) -> Option<Node> {
    let xs = match &*v.get(x) {
        Value::List(xs) => xs.clone(),
        _ => return None,
    };
    let g = |k: usize| xs.get(k).map_or(0, |&y| n32(v, y));
    let t = xs.first().map_or(-1, |&y| int_of(v, y));
    Some(match t {
        T_CHAR => Node::Char {
            font: u32::try_from(g(1)).unwrap_or(0),
            ch: u16::try_from(g(2)).unwrap_or(0),
        },
        T_BOX => Node::Box(decode_box(v, &xs, side)),
        T_RULE => Node::Rule(Rule {
            width: g(1),
            height: g(2),
            depth: g(3),
        }),
        T_INS => Node::Ins(Insert {
            number: u16::try_from(g(1)).unwrap_or(0),
            height: g(2),
            depth: g(3),
            split_top_skip: glue_of(v, xs.get(4).copied()),
            float_cost: g(5),
            list: xs
                .get(6)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
        }),
        T_MARK => {
            side.marks.push(xs.get(1).copied().unwrap_or(ValueId::UNIT));
            Node::Mark(u32::try_from(side.marks.len() - 1).unwrap_or(0))
        }
        T_ADJUST => Node::Adjust(
            xs.get(1)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
        ),
        T_LIG => Node::Lig(Ligature {
            font: u32::try_from(g(1)).unwrap_or(0),
            ch: u16::try_from(g(2)).unwrap_or(0),
            original: xs
                .get(3)
                .map(|&l| {
                    list_of(v, l)
                        .into_iter()
                        .map(|c| u16::try_from(int_of(v, c)).unwrap_or(0))
                        .collect()
                })
                .unwrap_or_default(),
            left_hit: g(4) != 0,
            right_hit: g(5) != 0,
        }),
        T_DISC => Node::Disc(Disc {
            pre: xs
                .get(1)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
            post: xs
                .get(2)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
            replace: xs
                .get(3)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
        }),
        T_WHATSIT => {
            side.whats.push(xs.get(1).copied().unwrap_or(ValueId::UNIT));
            // a size: pdfTeX's `\pdfrefxform`, `\pdfrefximage`
            let dims = (xs.len() >= 5).then(|| phitex_typeset::node::WhatsitSize {
                width: g(2),
                height: g(3),
                depth: g(4),
            });
            Node::Whatsit(u32::try_from(side.whats.len() - 1).unwrap_or(0), dims)
        }
        T_LANGUAGE => Node::Language {
            lang: u16::try_from(g(1)).unwrap_or(0),
            lhm: u8::try_from(g(2)).unwrap_or(0),
            rhm: u8::try_from(g(3)).unwrap_or(0),
        },
        T_MATH => Node::Math(
            if g(1) == 0 {
                MathKind::Before
            } else {
                MathKind::After
            },
            g(2),
        ),
        T_GLUE => Node::Glue(GlueNode {
            spec: glue_of(v, xs.get(1).copied()),
            kind: glue_kind(i64::from(g(2))),
            leader: xs
                .get(3)
                .filter(|&&l| l != ValueId::UNIT)
                .and_then(|&l| decode(v, l, side))
                .map(Box::new),
        }),
        T_KERN => Node::Kern(g(1), kern_kind(i64::from(g(2)))),
        T_PENALTY => Node::Penalty(g(1)),
        T_UNSET => Node::Unset(Unset {
            span_count: u16::try_from(g(1)).unwrap_or(0),
            width: g(2),
            height: g(3),
            depth: g(4),
            stretch: g(5),
            stretch_order: crate::val::order(i64::from(g(6))),
            shrink: g(7),
            shrink_order: crate::val::order(i64::from(g(8))),
            list: xs
                .get(9)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
        }),
        _ => return None,
    })
}

/// Whether the box value `b` is an hbox.
pub(crate) fn box_is_h(v: &Values, b: ValueId) -> bool {
    match &*v.get(b) {
        Value::List(xs) => xs.get(1).is_some_and(|&k| int_of(v, k) == 0),
        _ => false,
    }
}

/// A box's width (`m=1`), depth (2) or height (3) (§420).
pub(crate) fn box_dim(v: &Values, b: ValueId, m: i32) -> i32 {
    let k = match m {
        1 => 2,
        3 => 3,
        _ => 4,
    };
    match &*v.get(b) {
        Value::List(xs) => xs.get(k).map_or(0, |&x| n32(v, x)),
        _ => 0,
    }
}

/// The box with one dimension replaced.
pub(crate) fn box_with_dim(v: &Values, b: ValueId, m: i32, d: i32) -> ValueId {
    let k = match m {
        1 => 2,
        3 => 3,
        _ => 4,
    };
    match &*v.get(b) {
        Value::List(xs) => {
            let mut ys = xs.to_vec();
            if k < ys.len() {
                ys[k] = v.int(i64::from(d));
            }
            v.list(ys)
        }
        _ => b,
    }
}

/// A glue node of `g`.
pub(crate) fn glue_node(v: &Values, g: &Glue, kind: GlueKind) -> ValueId {
    v.list(vec![
        i(v, T_GLUE),
        glue_value(v, g),
        i(v, glue_kind_code(kind)),
        ValueId::UNIT,
    ])
}

pub(crate) fn penalty_node(v: &Values, p: i32) -> ValueId {
    v.list(vec![i(v, T_PENALTY), i(v, i64::from(p))])
}

pub(crate) fn kern_node(v: &Values, w: i32, k: KernKind) -> ValueId {
    v.list(vec![i(v, T_KERN), i(v, i64::from(w)), i(v, kern_code(k))])
}

pub(crate) fn marker(v: &Values, t: i64) -> ValueId {
    v.list(vec![i(v, t)])
}

/// The glue of a glue node value.
pub(crate) fn glue_of_node(v: &Values, x: ValueId) -> Glue {
    match &*v.get(x) {
        Value::List(xs) if tag(v, x) == T_GLUE => glue_of(v, xs.get(1).copied()),
        _ => Glue::ZERO,
    }
}

pub(crate) fn int_field(v: &Values, x: ValueId, k: usize) -> i32 {
    match &*v.get(x) {
        Value::List(xs) => xs.get(k).map_or(0, |&y| n32(v, y)),
        _ => 0,
    }
}
