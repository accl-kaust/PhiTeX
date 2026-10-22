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
/// A sealed line's list as a unit's shape has it ([`unit_shape`]):
/// `[tag, piece, k, fonts, head]`, the piece and the index of the line's
/// item in its items, its characters' fonts (sorted ids), its head's
/// hash ([`head_hash`]). Decoded, its box stays shallow ([`decode_box`]);
/// a finisher puts the line's list in its place where a value leaves
/// its run (`crate::pages::seal`).
pub(crate) const T_SEALED: i64 = 84;
/// `\end` (§1054): `[tag, null box, fill glue, penalty]`, the material
/// the page builder appends each time round until the page is empty.
pub(crate) const T_END: i64 = 93;
/// A fatal error (§93): `[tag]`; the main list ends there, the page
/// the builder holds is not shipped (§1335).
pub(crate) const T_ABORT: i64 = 89;
/// The page parameters a run changed, for the page builder's fold:
/// `[tag, vsize, maxdepth, topskip, holdinginserts]` (§980).
pub(crate) const T_PAGE_PARAMS: i64 = 92;
/// A document node's `\immediate\write` or `\immediate\openout` to a
/// job-written file (§1370, §1374): `[tag, entry]`, the `streams` entry
/// (`[name]` or `[name, text]`); the page builder's fold places it
/// among the shipped pages, so the writer assembles the file in TeX's
/// order.
pub(crate) const T_WRITE_AT: i64 = 88;
/// A font a document node loaded (`read_font_info`, §1257): `[tag, f]`;
/// the page builder's fold tells the writer which page shipped first
/// after it, as pdfTeX numbers the fonts a page's virtual fonts load
/// after the fonts loaded before that `\shipout` (`fplan`).
pub(crate) const T_FONT_AT: i64 = 87;
/// An `\immediate\pdfobj` a document node wrote after a page fired
/// (pdfTeX §1355): `[tag, n]`; the page builder's fold tells the writer
/// which page it follows (`oplan`), where the finish writes object `n`.
pub(crate) const T_PDFW_AT: i64 = 85;
/// `\\pagegoal=` and its kin (§1245): `[tag, i, x]`, `page_so_far[i] := x`
/// at the next candidate (the builder's state is as of the last one).
pub(crate) const T_SET_PAGE: i64 = 91;
/// The registers of an insertion class as the builder reads them at the
/// candidate after the `\\insert` (§1008): `[tag, n, box h+d or -1, box
/// is hbox, count, dimen, skip]`.
pub(crate) const T_INS_PARAMS: i64 = 90;

/// Marks and whatsits met while decoding, numbered; and the lists of
/// the boxes decoded shallow ([`decode_shallow`]), numbered.
#[derive(Clone, Default, Debug)]
pub(crate) struct Side {
    pub marks: Vec<ValueId>,
    pub whats: Vec<ValueId>,
    pub lazy: Vec<ValueId>,
    /// Per shallow box (by `lazy`'s number): the box's own value and its
    /// head as decoded, so the box goes back as that value while its
    /// head is unchanged (spec-final §3.1: a line box the page builder
    /// moves onto a page is never encoded again).
    pub lazy_box: Vec<(ValueId, [i64; 8])>,
    /// Per sealed line whose stand-in carries a skeleton ([`T_SEALED`]),
    /// by its shallow box's number: its whatsits, decoded where the line
    /// is, so the whatsits are numbered on this table in walk order.
    pub skel: Vec<Skel>,
}

/// A sealed line's skeleton as a decoding met it ([`Side::skel`]): the
/// boxes on the way to its whatsits and the whatsits (handles `base ..
/// base + n`), the rest of its list left out.
#[derive(Clone, Debug)]
pub(crate) struct Skel {
    /// The line's shallow box's number (`Side::lazy`).
    pub k: u32,
    /// The handle of its first whatsit.
    pub base: u32,
    /// How many whatsits it has.
    pub n: u32,
    /// The skeleton's nodes.
    pub list: List,
}

/// The handles of a shallow box's list stand-in: `LAZY + k` is
/// `side.lazy[k]` (a whatsit handle no list has: those number the
/// decode's whatsits from 0).
const LAZY: u32 = 0xF000_0000;

/// The value of a shallow box's list, if `b` is one ([`decode_shallow`]).
fn lazy_list(b: &BoxNode, side: &Side) -> Option<ValueId> {
    match b.list.as_slice() {
        [Node::Whatsit(h, None)] if *h >= LAZY => side.lazy.get((*h - LAZY) as usize).copied(),
        _ => None,
    }
}

/// The value a shallow box was decoded from, while its head is as it
/// was ([`Side::lazy_box`]).
fn lazy_value(b: &BoxNode, side: &Side) -> Option<ValueId> {
    match b.list.as_slice() {
        [Node::Whatsit(h, None)] if *h >= LAZY => side
            .lazy_box
            .get((*h - LAZY) as usize)
            .filter(|(_, head)| *head == head_words(b))
            .map(|e| e.0),
        _ => None,
    }
}

/// A box's head as its value has it (the words after the tag, before
/// the list).
fn head_words(b: &BoxNode) -> [i64; 8] {
    [
        i64::from(b.kind != BoxKind::H),
        i64::from(b.width),
        i64::from(b.height),
        i64::from(b.depth),
        i64::from(b.shift),
        match b.glue.sign {
            GlueSign::Normal => 0,
            GlueSign::Stretching => 1,
            GlueSign::Shrinking => 2,
        },
        b.glue.order as i64,
        i64::from_ne_bytes(b.glue.ratio.to_bits().to_ne_bytes()),
    ]
}

fn i(v: &Values, x: i64) -> ValueId {
    crate::val::int_id(v, x)
}

fn n32(v: &Values, x: ValueId) -> i32 {
    i32::try_from(int_of(v, x)).unwrap_or(0)
}

pub(crate) fn tag(v: &Values, x: ValueId) -> i64 {
    v.with_list(x, |xs| xs.first().map_or(-1, |&t| int_of(v, t)))
        .unwrap_or(-1)
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
    // (a line's interword glue is one node many times: the last glue
    // encoded is kept)
    let mut seen: Option<(&Node, ValueId)> = None;
    let mut xs = Vec::with_capacity(list.len());
    for n in list {
        let x = match (n, seen) {
            (Node::Glue(g), Some((Node::Glue(h), x))) if g == h => x,
            (Node::Glue(g), _) if g.leader.is_none() => {
                let x = encode(v, n, side);
                seen = Some((n, x));
                x
            }
            _ => encode(v, n, side),
        };
        xs.push(x);
    }
    v.list(xs)
}

pub(crate) fn encode_box(v: &Values, b: &BoxNode, side: &Side) -> ValueId {
    if let Some(x) = lazy_value(b, side) {
        side_seen(SEEN_LAZY);
        return x;
    }
    if let Some(l) = lazy_list(b, side) {
        side_seen(SEEN_LAZY);
        return encode_box_with(v, b, l);
    }
    encode_box_with(v, b, encode_list(v, &b.list, side))
}

/// Box `b` with `list`, the value of its list (already encoded).
pub(crate) fn encode_box_with(v: &Values, b: &BoxNode, list: ValueId) -> ValueId {
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
        list,
    ])
}

/// A node of fixed shape (a character, glue without leaders, a kern, a
/// penalty), through the worker's cache in a run ([`node_id`]).
fn encode_fixed(v: &Values, n: &Node) -> Option<ValueId> {
    Some(match n {
        Node::Char { font, ch } => {
            let (f, c) = (i64::from(*font), i64::from(*ch));
            crate::val::char_node_id(v, *font, *ch, || {
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
        Node::Mark(k) => {
            side_seen(SEEN_MARK);
            v.list(vec![
                i(v, T_MARK),
                side.marks
                    .get(*k as usize)
                    .copied()
                    .unwrap_or(ValueId::UNIT),
            ])
        }
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
            side_seen(SEEN_MARK);
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
    let got = v.with_list(x, |xs| {
        let mut out: List = Vec::with_capacity(xs.len());
        // (the last glue decoded, kept: a line's interword glue is one
        // value many times)
        let mut last: Option<(ValueId, usize)> = None;
        for &n in xs {
            if let Some((id, k)) = last
                && id == n
            {
                let g = out[k].clone();
                out.push(g);
                continue;
            }
            if let Some(d) = decode(v, n, side) {
                if matches!(&d, Node::Glue(g) if g.leader.is_none()) {
                    last = Some((n, out.len()));
                }
                out.push(d);
            }
        }
        out
    });
    if let Some(out) = got {
        return out;
    }
    list_of(v, x)
        .into_iter()
        .filter_map(|n| decode(v, n, side))
        .collect()
}

pub(crate) fn decode_box(v: &Values, xs: &[ValueId], side: &mut Side) -> BoxNode {
    let mut b = decode_box_head(v, xs);
    // (a sealed line: shallow, its stand-in kept as its list)
    if let Some(&l) = xs.get(9)
        && is_sealed(v, l)
    {
        let k = u32::try_from(side.lazy.len()).unwrap_or(0);
        side.lazy.push(l);
        side.lazy_box.push((v.list_from(xs), head_words(&b)));
        b.list = vec![Node::Whatsit(LAZY + k, None)];
        // (its whatsits where the line is, in walk order: what the
        // shipping of the page does with them needs their data and
        // their order only, [`line_shape`])
        if let Some(sk) = sealed_of(v, l).map(|s| s.skel).filter(|&x| x != ValueId::UNIT) {
            let base = side.whats.len();
            let list = decode_list(v, sk, side);
            let n = side.whats.len() - base;
            side.skel.push(Skel {
                k,
                base: u32::try_from(base).unwrap_or(0),
                n: u32::try_from(n).unwrap_or(0),
                list,
            });
        }
        return b;
    }
    b.list = xs.get(9).map_or_else(Vec::new, |&l| {
        let before = crate::exec::obs_flag!("PHITEX_MADE_CHECK").then(|| side.clone());
        if let Some(m) = take_made(l, side) {
            checked_made(v, l, m, before, side)
        } else {
            made_stat(4);
            decode_list(v, l, side)
        }
    });
    b
}

/// (observe) `PHITEX_MADE_STATS`: plain lines, (unused), lines not
/// kept (full), taken, box lists decoded while shipping, lines with
/// marks, whatsits or shallow boxes kept and not kept.
pub(crate) static MADE_STATS: [std::sync::atomic::AtomicU64; 7] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 7];

/// Counts event `k` of [`MADE_STATS`] (under `observe`, while a page
/// is decoded for shipping for `k = 4`).
#[inline]
pub(crate) fn made_stat(k: usize) {
    if crate::exec::obs_flag!("PHITEX_MADE_STATS")
        && (k != 4 || TAKE_MADE.with(std::cell::Cell::get))
    {
        MADE_STATS[k].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The lists line breaking made, native, by their values' ids: a page
/// shipped takes its lines' lists from here instead of decoding them
/// from the values the line breaking encoded them to (ids are the
/// content's, so a list here is its value's decoding). Only lists that
/// decode without the side tables (no mark, whatsit, insertion,
/// adjustment, leaders or unset box anywhere in them); bounded by their
/// nodes, emptied when full.
struct Made {
    /// Each list with the values of its marks and whatsits, numbered
    /// in the order its value's decoding meets them ([`localize`]).
    lists: phitex_defuse::FxHashMap<u64, (List, Vec<ValueId>, Vec<ValueId>)>,
    nodes: usize,
}

/// Line `list`, encoded with `side`, made independent of it: each mark
/// and whatsit numbered in the order its value's decoding meets them,
/// their values in `marks` and `whats`, and each shallow box's list
/// decoded in place ([`decode_shallow`]): the list its value decodes to
/// but for those numbers ([`rebase`]). `false` where it cannot be.
pub(crate) fn localize(
    v: &Values,
    list: &mut [Node],
    side: &Side,
    marks: &mut Vec<ValueId>,
    whats: &mut Vec<ValueId>,
) -> bool {
    for n in list.iter_mut() {
        let ok = match n {
            Node::Mark(k) => {
                marks.push(
                    side.marks
                        .get(*k as usize)
                        .copied()
                        .unwrap_or(ValueId::UNIT),
                );
                *k = u32::try_from(marks.len() - 1).unwrap_or(0);
                true
            }
            Node::Whatsit(h, _) if *h >= LAZY => false,
            Node::Whatsit(h, _) => {
                whats.push(
                    side.whats
                        .get(*h as usize)
                        .copied()
                        .unwrap_or(ValueId::UNIT),
                );
                *h = u32::try_from(whats.len() - 1).unwrap_or(0);
                true
            }
            Node::Box(b) => match b.list.as_slice() {
                [Node::Whatsit(h, None)] if *h >= LAZY => {
                    let Some(&l) = side.lazy.get((*h - LAZY) as usize) else {
                        return false;
                    };
                    if is_sealed(v, l) {
                        return false;
                    }
                    let mut s = Side::default();
                    b.list = decode_list(v, l, &mut s);
                    localize(v, &mut b.list, &s, marks, whats)
                }
                _ => localize(v, &mut b.list, side, marks, whats),
            },
            Node::Ins(i) => localize(v, &mut i.list, side, marks, whats),
            Node::Adjust(l) => localize(v, l, side, marks, whats),
            Node::Disc(d) => {
                localize(v, &mut d.pre, side, marks, whats)
                    && localize(v, &mut d.post, side, marks, whats)
                    && localize(v, &mut d.replace, side, marks, whats)
            }
            Node::Unset(u) => localize(v, &mut u.list, side, marks, whats),
            Node::Glue(g) => match &mut g.leader {
                Some(l) => localize(v, std::slice::from_mut(&mut **l), side, marks, whats),
                None => true,
            },
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// A list [`localize`]d, its marks and whatsits put on `side` as its
/// value's decoding with `side` puts them, in the same order.
fn rebase(list: &mut [Node], marks: &[ValueId], whats: &[ValueId], side: &mut Side) {
    for n in list.iter_mut() {
        match n {
            Node::Mark(k) => {
                side.marks
                    .push(marks.get(*k as usize).copied().unwrap_or(ValueId::UNIT));
                *k = u32::try_from(side.marks.len() - 1).unwrap_or(0);
            }
            Node::Whatsit(h, _) => {
                side.whats
                    .push(whats.get(*h as usize).copied().unwrap_or(ValueId::UNIT));
                *h = u32::try_from(side.whats.len() - 1).unwrap_or(0);
            }
            Node::Box(b) => rebase(&mut b.list, marks, whats, side),
            Node::Ins(i) => rebase(&mut i.list, marks, whats, side),
            Node::Adjust(l) => rebase(l, marks, whats, side),
            Node::Disc(d) => {
                rebase(&mut d.pre, marks, whats, side);
                rebase(&mut d.post, marks, whats, side);
                rebase(&mut d.replace, marks, whats, side);
            }
            Node::Unset(u) => rebase(&mut u.list, marks, whats, side),
            Node::Glue(g) => {
                if let Some(l) = &mut g.leader {
                    rebase(std::slice::from_mut(&mut **l), marks, whats, side);
                }
            }
            _ => {}
        }
    }
}

static MADE: std::sync::Mutex<Option<Made>> = std::sync::Mutex::new(None);

/// The nodes [`MADE`] holds at most.
const MADE_NODES: usize = 1 << 22;

thread_local! {
    /// Set by an encoding that met a node of the side tables (a mark, a
    /// whatsit, a shallow box's list): its value decodes to another
    /// list than the one encoded, or touches the decoding's tables.
    static SIDE_SEEN: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// Set while a page is decoded for shipping ([`take_made`]).
    pub(crate) static TAKE_MADE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keeps line `list`, encoded as `id` by an [`encode_list`] that met
/// no node of the side tables ([`side_seen`]), for the page that ships
/// it; the list back where it is not kept (the store full: the lines
/// kept first ship first).
pub(crate) fn keep_made(
    id: ValueId,
    list: List,
    marks: Vec<ValueId>,
    whats: Vec<ValueId>,
) -> Option<List> {
    let mut g = MADE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = g.get_or_insert_with(|| Made {
        lists: phitex_defuse::FxHashMap::default(),
        nodes: 0,
    });
    if m.nodes + list.len() > MADE_NODES {
        made_stat(2);
        return Some(list);
    }
    m.nodes += list.len();
    if let Some(old) = m.lists.insert(id.raw(), (list, marks, whats)) {
        m.nodes -= old.0.len();
    }
    None
}

/// [`encode_list`] of `list`, and whether it met no node of the side
/// tables ([`SIDE_SEEN`]).
pub(crate) fn encode_list_side_free(v: &Values, list: &[Node], side: &Side) -> (ValueId, u8) {
    let was = SIDE_SEEN.with(|c| c.replace(0));
    let x = encode_list(v, list, side);
    let seen = SIDE_SEEN.with(|c| c.replace(was | c.get()));
    (x, seen)
}

/// [`SIDE_SEEN`]'s bit for a mark or a whatsit.
pub(crate) const SEEN_MARK: u8 = 1;
/// [`SIDE_SEEN`]'s bit for a shallow box's list ([`decode_shallow`]).
pub(crate) const SEEN_LAZY: u8 = 2;

/// Notes an encoding that met a node of the side tables ([`SIDE_SEEN`]).
#[inline]
fn side_seen(bit: u8) {
    SIDE_SEEN.with(|c| c.set(c.get() | bit));
}

/// Line list `id` as line breaking made it, taken, while a page is
/// decoded for shipping.
fn take_made(id: ValueId, side: &mut Side) -> Option<List> {
    if !TAKE_MADE.with(std::cell::Cell::get) {
        return None;
    }
    let mut g = MADE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let m = g.as_mut()?;
    let (mut l, marks, whats) = m.lists.remove(&id.raw())?;
    m.nodes -= l.len();
    drop(g);
    if !(marks.is_empty() && whats.is_empty()) {
        rebase(&mut l, &marks, &whats, side);
    }
    made_stat(3);
    Some(l)
}

/// A list taken from [`MADE`]; under `observe`, `PHITEX_MADE_CHECK`
/// checks it against its value's decoding.
fn checked_made(v: &Values, id: ValueId, m: List, before: Option<Side>, side: &Side) -> List {
    if let Some(mut s) = before {
        let d = decode_list(v, id, &mut s);
        assert!(
            d == m && s.marks == side.marks && s.whats == side.whats,
            "a made line list differs from its value's decoding"
        );
    }
    m
}

/// A box's dimensions and glue setting, its list empty.
fn decode_box_head(v: &Values, xs: &[ValueId]) -> BoxNode {
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
        list: Vec::new(),
    }
}

/// A node as a reader that only measures it sees it: a box with its
/// dimensions and glue setting, its list left encoded (one whatsit
/// `LAZY + k` standing for `side.lazy[k]`, which [`encode`] with the
/// same `side` puts back as it was); any other node decoded
/// ([`decode`]). Its readers do not look into the boxes (packaging,
/// the page builder): the writers and `\unvbox` decode a box's value.
pub(crate) fn decode_shallow(v: &Values, x: ValueId, side: &mut Side) -> Option<Node> {
    let head = v
        .with_list(x, |xs| <[ValueId; 10]>::try_from(xs).ok())
        .flatten();
    if let Some(xs) = head
        && int_of(v, xs[0]) == T_BOX
    {
        let k = u32::try_from(side.lazy.len()).unwrap_or(0);
        side.lazy.push(xs[9]);
        let mut b = decode_box_head(v, &xs);
        side.lazy_box.push((x, head_words(&b)));
        b.list = vec![Node::Whatsit(LAZY + k, None)];
        return Some(Node::Box(b));
    }
    decode(v, x, side)
}

/// A box's value decoded one level deep: its head, and its list's items
/// as [`decode_shallow`] gives them (a box among them left encoded, put
/// back as it was by [`encode`] with the same `side`). For a reader of
/// the top list only (`\vsplit`'s break, §970: it measures the boxes,
/// never looks into them). `None` if `x` is not a box.
pub(crate) fn decode_box_top(v: &Values, x: ValueId, side: &mut Side) -> Option<BoxNode> {
    let xs = v
        .with_list(x, |xs| <[ValueId; 10]>::try_from(xs).ok())
        .flatten()?;
    if int_of(v, xs[0]) != T_BOX {
        return None;
    }
    if is_sealed(v, xs[9]) {
        return match decode_shallow(v, x, side) {
            Some(Node::Box(b)) => Some(b),
            _ => None,
        };
    }
    let mut b = decode_box_head(v, &xs);
    b.list = list_of(v, xs[9])
        .into_iter()
        .filter_map(|n| decode_shallow(v, n, side))
        .collect();
    Some(b)
}

pub(crate) fn decode(v: &Values, x: ValueId, side: &mut Side) -> Option<Node> {
    // (a character, most of a list: its own direct-mapped cache)
    if let Some((font, ch)) = crate::val::char_node_of(v, x) {
        return Some(Node::Char { font, ch });
    }
    if let Some(n) = crate::val::leaf_of(v, x) {
        if let Node::Char { font, ch } = n {
            crate::val::keep_char_node(v, x, font, ch);
        }
        return Some(n);
    }
    let (marks, whats) = (side.marks.len(), side.whats.len());
    let n = decode_new(v, x, side)?;
    // a node that met no mark or whatsit decodes the same from its value
    // alone, a box or a list-holding node included
    // (a node holding a list is decoded once where it is read: copying
    // its tree into the cache and out again costs more than decoding it)
    let leaf = side.marks.len() == marks
        && side.whats.len() == whats
        && !matches!(
            n,
            Node::Box(_) | Node::Unset(_) | Node::Ins(_) | Node::Adjust(_) | Node::Disc(_)
        );
    if leaf {
        if let Node::Char { font, ch } = n {
            crate::val::keep_char_node(v, x, font, ch);
        }
        crate::val::keep_leaf(v, x, &n);
    }
    Some(n)
}

#[allow(clippy::too_many_lines)]
fn decode_new(v: &Values, x: ValueId, side: &mut Side) -> Option<Node> {
    v.with_list(x, |xs| decode_items(v, xs, side)).flatten()
}

/// [`decode_new`] on the value's elements, borrowed.
#[allow(clippy::too_many_lines)]
fn decode_items(v: &Values, xs: &[ValueId], side: &mut Side) -> Option<Node> {
    let g = |k: usize| xs.get(k).map_or(0, |&y| n32(v, y));
    let t = xs.first().map_or(-1, |&y| int_of(v, y));
    Some(match t {
        T_CHAR => Node::Char {
            font: u32::try_from(g(1)).unwrap_or(0),
            ch: u16::try_from(g(2)).unwrap_or(0),
        },
        T_BOX => Node::Box(decode_box(v, xs, side)),
        T_RULE => Node::Rule(Rule {
            width: g(1),
            height: g(2),
            depth: g(3),
        }),
        T_INS => Node::Ins(Box::new(Insert {
            number: u16::try_from(g(1)).unwrap_or(0),
            height: g(2),
            depth: g(3),
            split_top_skip: glue_of(v, xs.get(4).copied()),
            float_cost: g(5),
            list: xs
                .get(6)
                .map_or_else(Vec::new, |&l| decode_list(v, l, side)),
        })),
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
                    // (borrowed: the characters straight into the node)
                    v.with_list(l, |cs| {
                        cs.iter()
                            .map(|&c| u16::try_from(int_of(v, c)).unwrap_or(0))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        list_of(v, l)
                            .into_iter()
                            .map(|c| u16::try_from(int_of(v, c)).unwrap_or(0))
                            .collect()
                    })
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
        T_UNSET => Node::Unset(Box::new(Unset {
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
        })),
        _ => return None,
    })
}

/// Whether the box value `b` is an hbox.
pub(crate) fn box_is_h(v: &Values, b: ValueId) -> bool {
    v.with_list(b, |xs| xs.get(1).is_some_and(|&k| int_of(v, k) == 0))
        .unwrap_or(false)
}

/// A box's width (`m=1`), depth (2) or height (3) (§420).
pub(crate) fn box_dim(v: &Values, b: ValueId, m: i32) -> i32 {
    let k = match m {
        1 => 2,
        3 => 3,
        _ => 4,
    };
    v.with_list(b, |xs| xs.get(k).map_or(0, |&x| n32(v, x)))
        .unwrap_or(0)
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
    v.with_list(x, |xs| {
        if xs.first().is_some_and(|&t| int_of(v, t) == T_GLUE) {
            glue_of(v, xs.get(1).copied())
        } else {
            Glue::ZERO
        }
    })
    .unwrap_or(Glue::ZERO)
}

pub(crate) fn int_field(v: &Values, x: ValueId, k: usize) -> i32 {
    v.with_list(x, |xs| xs.get(k).map_or(0, |&y| n32(v, y)))
        .unwrap_or(0)
}

/// Whether list value `l` is a sealed line's stand-in ([`T_SEALED`]).
pub(crate) fn is_sealed(v: &Values, l: ValueId) -> bool {
    v.with_list(l, |ys| matches!(ys.len(), 5 | 6) && v.as_int(ys[0]) == Some(T_SEALED))
        .unwrap_or(false)
}

/// The skeleton of shallow box `b`, a sealed line's, as the decoding
/// with `side` met it ([`Side::skel`]); `None` where it has none.
pub(crate) fn sealed_skeleton<'a>(b: &BoxNode, side: &'a Side) -> Option<&'a [Node]> {
    match b.list.as_slice() {
        [Node::Whatsit(h, None)] if *h >= LAZY => {
            let k = *h - LAZY;
            let i = side.skel.binary_search_by_key(&k, |s| s.k).ok()?;
            Some(&side.skel[i].list)
        }
        _ => None,
    }
}

/// The handles of the sealed lines' whatsits on `side`: `(k, base, n)`
/// per line with a skeleton ([`Side::skel`]), by `k`, for
/// [`fill_sealed`].
pub(crate) fn sealed_bases(side: &Side) -> Vec<(u32, u32, u32)> {
    side.skel.iter().map(|s| (s.k, s.base, s.n)).collect()
}

/// The whatsits of `list` renumbered from `base` (a line decoded on a
/// table of its own put where its stand-in's skeleton numbered them).
fn shift_whats(list: &mut [Node], base: u32) {
    for n in list.iter_mut() {
        match n {
            Node::Whatsit(h, _) if *h < LAZY => *h += base,
            Node::Box(b) => shift_whats(&mut b.list, base),
            Node::Ins(i) => shift_whats(&mut i.list, base),
            Node::Adjust(l) => shift_whats(l, base),
            Node::Unset(u) => shift_whats(&mut u.list, base),
            Node::Disc(d) => {
                shift_whats(&mut d.pre, base);
                shift_whats(&mut d.post, base);
                shift_whats(&mut d.replace, base);
            }
            Node::Glue(g) => {
                if let Some(l) = &mut g.leader {
                    shift_whats(std::slice::from_mut(&mut **l), base);
                }
            }
            _ => {}
        }
    }
}

/// A sealed line's stand-in, read ([`T_SEALED`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sealed {
    /// The piece whose unit holds the line (its node's raw id).
    pub piece: i64,
    /// The line's item's index in the unit's items.
    pub k: u32,
    /// The fonts of its characters (a list of sorted ids).
    pub fonts: ValueId,
    /// A hash of its box's head as the piece made it.
    pub head: i64,
    /// Its skeleton ([`line_shape`]), or `UNIT`: the boxes on the way
    /// to its whatsits and the whatsits, in walk order.
    pub skel: ValueId,
}

/// The stand-in `l` read, if it is one.
pub(crate) fn sealed_of(v: &Values, l: ValueId) -> Option<Sealed> {
    v.with_list(l, |ys| {
        (matches!(ys.len(), 5 | 6) && v.as_int(ys[0]) == Some(T_SEALED)).then(|| Sealed {
            piece: v.as_int(ys[1]).unwrap_or(0),
            k: u32::try_from(v.as_int(ys[2]).unwrap_or(0)).unwrap_or(0),
            fonts: ys[3],
            head: v.as_int(ys[4]).unwrap_or(0),
            skel: ys.get(5).copied().unwrap_or(ValueId::UNIT),
        })
    })
    .flatten()
}

/// The hash of a box's head (its value's words before its list), in 56
/// bits (an integer kept in its value's id, never interned).
pub(crate) fn head_hash(head: &[ValueId]) -> i64 {
    let words: Vec<u64> = head.iter().map(|x| x.raw()).collect();
    i64::try_from(phitex_defuse::hash_words(0x7365_616c_6564_0001, &words) >> 8).unwrap_or(0)
}

/// The sealed lines in `list` (shallow boxes whose stand-ins `lazy`
/// holds, [`T_SEALED`], at any depth) given their glue settings and
/// lists in place: `line` gives a line's box as its piece made it (its
/// unit's items, `crate::pages::seal`). Each box filled is noted in
/// `saved` (its place among the boxes in the walk's order, its glue
/// setting and list before) for [`unfill_sealed`]. `false` where a line
/// is not found.
pub(crate) fn fill_sealed(
    v: &Values,
    list: &mut [Node],
    lazy: &[ValueId],
    bases: &[(u32, u32, u32)],
    line: &mut dyn FnMut(&Sealed) -> Option<ValueId>,
    seen: &mut usize,
    saved: &mut Vec<(usize, GlueSet, List)>,
) -> bool {
    for n in list.iter_mut() {
        let ok = match n {
            Node::Box(b) => {
                let at = *seen;
                *seen += 1;
                let stand_in = match b.list.as_slice() {
                    [Node::Whatsit(k, None)] if *k >= LAZY => {
                        let k = *k - LAZY;
                        lazy.get(k as usize).and_then(|&l| sealed_of(v, l)).map(|s| (k, s))
                    }
                    _ => None,
                };
                match stand_in {
                    Some((k, s)) => {
                        let Some(x) = line(&s) else { return false };
                        let Some(xs) = v.with_list(x, |xs| <[ValueId; 10]>::try_from(xs).ok()).flatten() else {
                            return false;
                        };
                        let mut side = Side::default();
                        let mut list = take_made(xs[9], &mut side).unwrap_or_else(|| decode_list(v, xs[9], &mut side));
                        // (its whatsits numbered where its skeleton's
                        // decoding numbered them on the page's table)
                        let (base, n) = bases
                            .binary_search_by_key(&k, |e| e.0)
                            .map_or((0, 0), |i| (bases[i].1, bases[i].2));
                        if side.whats.len() != n as usize {
                            return false;
                        }
                        if n > 0 {
                            shift_whats(&mut list, base);
                        }
                        let glue = std::mem::replace(&mut b.glue, decode_box_head(v, &xs).glue);
                        saved.push((at, glue, std::mem::replace(&mut b.list, list)));
                        true
                    }
                    None => fill_sealed(v, &mut b.list, lazy, bases, line, seen, saved),
                }
            }
            Node::Ins(i) => fill_sealed(v, &mut i.list, lazy, bases, line, seen, saved),
            Node::Adjust(l) => fill_sealed(v, l, lazy, bases, line, seen, saved),
            Node::Unset(u) => fill_sealed(v, &mut u.list, lazy, bases, line, seen, saved),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// The boxes [`fill_sealed`] filled back as they were (`saved`, in its
/// walk's order, taken from the back), so the box is the shape again.
pub(crate) fn unfill_sealed(list: &mut [Node], seen: &mut usize, saved: &mut std::collections::VecDeque<(usize, GlueSet, List)>) {
    for n in list.iter_mut() {
        match n {
            Node::Box(b) => {
                let at = *seen;
                *seen += 1;
                if saved.front().is_some_and(|s| s.0 == at) {
                    let (_, glue, l) = saved.pop_front().expect("a saved box");
                    b.glue = glue;
                    b.list = l;
                } else {
                    unfill_sealed(&mut b.list, seen, saved);
                }
            }
            Node::Ins(i) => unfill_sealed(&mut i.list, seen, saved),
            Node::Adjust(l) => unfill_sealed(l, seen, saved),
            Node::Unset(u) => unfill_sealed(&mut u.list, seen, saved),
            _ => {}
        }
    }
}

/// A few font ids, sorted, without an allocation for a line's usual
/// one to four ([`native_fonts`]).
#[derive(Default)]
pub(crate) struct FontIds {
    few: [u32; 8],
    n: usize,
    more: Vec<u32>,
}

impl FontIds {
    fn insert(&mut self, f: u32) {
        if !self.more.is_empty() {
            if let Err(i) = self.more.binary_search(&f) {
                self.more.insert(i, f);
            }
            return;
        }
        let s = &self.few[..self.n];
        let Err(i) = s.binary_search(&f) else { return };
        if self.n == self.few.len() {
            self.more = self.few.to_vec();
            self.more.insert(i, f);
            return;
        }
        self.few.copy_within(i..self.n, i + 1);
        self.few[i] = f;
        self.n += 1;
    }

    pub(crate) fn as_slice(&self) -> &[u32] {
        if self.more.is_empty() { &self.few[..self.n] } else { &self.more }
    }
}

/// The fonts of a native list (their ids, sorted), or `None` where it
/// holds a node other than characters, boxes of them, discretionaries,
/// glue without leaders, rules, kerns and the like ([`line_shape`]
/// decides those from the list's value).
pub(crate) fn native_fonts(list: &[Node]) -> Option<FontIds> {
    fn walk(l: &[Node], out: &mut FontIds) -> bool {
        let mut last = None;
        l.iter().all(|n| match n {
            Node::Char { font, .. } => {
                if last != Some(*font) {
                    out.insert(*font);
                    last = Some(*font);
                }
                true
            }
            Node::Lig(g) => {
                out.insert(g.font);
                true
            }
            Node::Box(b) => walk(&b.list, out),
            Node::Disc(d) => walk(&d.pre, out) && walk(&d.post, out) && walk(&d.replace, out),
            Node::Glue(g) => g.leader.is_none(),
            Node::Rule(_) | Node::Language { .. } | Node::Math(..) | Node::Kern(..) | Node::Penalty(_) => true,
            _ => false,
        })
    }
    let mut out = FontIds::default();
    walk(list, &mut out).then_some(out)
}

/// The fonts of box list `l`'s characters at any depth (sorted ids) and
/// its skeleton, or `None` where the shipping of its page would look
/// into it. By node kind and what shipping does with it: characters,
/// rules, glue, kerns and the like the page's writer alone reads (their
/// fonts named); leaders likewise (their box's whatsits would act once
/// per copy: such a line is not sealed); a mark nothing reads inside a
/// box; a whatsit acts when the page ships on its data, in the order
/// the walk meets it (a `\write`, `\openout`, `\closeout` expands
/// then; a link, an annotation, a destination, a thread, a literal, a
/// `\special` give the writer their data, the writer placing them where
/// it walks the line), kept with the boxes on the way to it as the
/// skeleton; one that reads its position at shipping
/// ([`crate::pdfprim::reads_position_at_shipout`]) is not sealed, nor is
/// one in a discretionary. An insertion, an adjustment, an unset box,
/// a node not known: not sealed.
fn line_shape(v: &Values, l: ValueId) -> Option<(ValueId, Option<ValueId>)> {
    let mut fonts = std::collections::BTreeSet::new();
    let mut skel = Vec::new();
    if !line_walk(v, l, &mut fonts, Some(&mut skel)) {
        return None;
    }
    let f = v.list(fonts.into_iter().map(|f| v.int(f)).collect());
    Some((f, (!skel.is_empty()).then(|| v.list(skel))))
}

/// [`line_shape`]'s walk of list `l`: `skel` where a whatsit may be
/// (none in a discretionary or leaders).
fn line_walk(
    v: &Values,
    l: ValueId,
    fonts: &mut std::collections::BTreeSet<i64>,
    mut skel: Option<&mut Vec<ValueId>>,
) -> bool {
    v.with_list(l, |xs| {
        xs.iter()
            .all(|&n| node_walk(v, n, fonts, skel.as_deref_mut()))
    })
    .unwrap_or(false)
}

fn node_walk(
    v: &Values,
    n: ValueId,
    fonts: &mut std::collections::BTreeSet<i64>,
    skel: Option<&mut Vec<ValueId>>,
) -> bool {
    v.with_list(n, |ys| {
        let t = ys.first().and_then(|&t| v.as_int(t)).unwrap_or(-1);
        let at = |k: usize| ys.get(k).copied().unwrap_or(ValueId::UNIT);
        match t {
            T_CHAR | T_LIG => {
                fonts.insert(v.as_int(at(1)).unwrap_or(0));
                true
            }
            T_BOX => match skel {
                Some(sk) => {
                    let mut inner = Vec::new();
                    if !line_walk(v, at(9), fonts, Some(&mut inner)) {
                        return false;
                    }
                    if !inner.is_empty() {
                        // (the box on the way: its kind, no measures)
                        let zero = v.int(0);
                        let mut b = vec![zero; 10];
                        b[0] = ys[0];
                        b[1] = at(1);
                        b[9] = v.list(inner);
                        sk.push(v.list(b));
                    }
                    true
                }
                None => line_walk(v, at(9), fonts, None),
            },
            T_DISC => {
                line_walk(v, at(1), fonts, None)
                    && line_walk(v, at(2), fonts, None)
                    && line_walk(v, at(3), fonts, None)
            }
            T_GLUE => at(3) == ValueId::UNIT || node_walk(v, at(3), fonts, None),
            T_WHATSIT => {
                let Some(sk) = skel else { return false };
                let kind = v
                    .with_list(at(1), |d| d.first().and_then(|&k| v.as_str(k)))
                    .flatten()
                    .unwrap_or_default();
                if crate::pdfprim::reads_position_at_shipout(&kind) {
                    return false;
                }
                sk.push(n);
                true
            }
            T_MARK | T_RULE | T_LANGUAGE | T_MATH | T_KERN | T_PENALTY => true,
            _ => false,
        }
    })
    .unwrap_or(false)
}

/// Piece `piece`'s items as their shape, made where the piece makes
/// its unit (`crate::pages`, `PITEMS`' `shape`): each box a `T_VBOX`
/// item appends whose list a page's writer need not look into
/// ([`line_shape`]) with its glue setting zero and a stand-in for its
/// list ([`T_SEALED`]: the piece, the item's index, the fonts, the
/// head's hash, and the skeleton of its whatsits where it has some);
/// the rest as they are. A finisher reading the shape depends on the
/// lines' dimensions, fonts and whatsits, not on their characters:
/// the page's writer reads those (`crate::pages::seal`). `None` where
/// no item is sealed (the shape is the items).
pub(crate) fn unit_shape(
    v: &Values,
    piece: i64,
    items: &[ValueId],
    known: &crate::FxHashMap<ValueId, ValueId>,
) -> Option<ValueId> {
    let mut changed = false;
    let out: Vec<ValueId> = items
        .iter()
        .enumerate()
        .map(|(k, &it)| {
            let shaped = v
                .with_list(it, |ys| {
                    if ys.len() != 5 || v.as_int(ys[0]) != Some(T_VBOX) {
                        return None;
                    }
                    let bx = v.with_list(ys[1], <[ValueId]>::to_vec)?;
                    if bx.len() != 10 || v.as_int(bx[0]) != Some(T_BOX) {
                        return None;
                    }
                    // (a line this run made: its fonts known from its nodes)
                    let (fonts, skel) = match known.get(&ys[1]) {
                        Some(&f) => (f, None),
                        None => line_shape(v, bx[9])?,
                    };
                    let zero = v.int(0);
                    let mut b = bx;
                    let head = head_hash(&b[..6]);
                    b[6] = zero;
                    b[7] = zero;
                    b[8] = zero;
                    let mut st = vec![
                        v.int(T_SEALED),
                        v.int(piece),
                        v.int(i64::try_from(k).unwrap_or(i64::MAX)),
                        fonts,
                        v.int(head),
                    ];
                    st.extend(skel);
                    b[9] = v.list(st);
                    let mut y = ys.to_vec();
                    y[1] = v.list(b);
                    Some(v.list(y))
                })
                .flatten();
            match shaped {
                Some(y) => {
                    changed = true;
                    y
                }
                None => it,
            }
        })
        .collect();
    changed.then(|| v.list(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: &Values, list: Vec<ValueId>) -> ValueId {
        let b = v.list(vec![
            v.int(T_BOX),
            v.int(0),
            v.int(100),
            v.int(7),
            v.int(2),
            v.int(0),
            v.int(1),
            v.int(0),
            v.int(5),
            v.list(list),
        ]);
        v.list(vec![v.int(T_VBOX), b, ValueId::UNIT, ValueId::UNIT, v.int(0)])
    }

    /// A line of characters is sealed (its glue setting zero, its list a
    /// stand-in naming the piece, the item and the fonts); a line with a
    /// whatsit that reads its position at shipping is not; a stand-in
    /// decodes as a shallow box and encodes back as it was.
    #[test]
    fn unit_shape_seals_plain_lines_only() {
        let v = Values::new();
        let ch = |f: i64, c: i64| v.list(vec![v.int(T_CHAR), v.int(f), v.int(c)]);
        let plain = line(&v, vec![ch(3, 65), ch(1, 66), ch(3, 67)]);
        let pos = v.list(vec![v.int(T_WHATSIT), v.list(vec![v.str("pdf:savepos")])]);
        let placed = line(&v, vec![ch(3, 65), pos]);
        let none = crate::FxHashMap::default();
        let shape = unit_shape(&v, 77, &[placed, plain], &none).expect("a line sealed");
        let items = list_of(&v, shape);
        assert_eq!(items[0], placed);
        let b = list_of(&v, list_of(&v, items[1])[1]);
        let real = list_of(&v, list_of(&v, plain)[1]);
        assert_eq!(b[..6], real[..6]);
        assert_eq!(v.as_int(b[8]), Some(0));
        let s = sealed_of(&v, b[9]).expect("a stand-in");
        assert_eq!((s.piece, s.k, s.head, s.skel), (77, 1, head_hash(&real[..6]), ValueId::UNIT));
        assert_eq!(list_of(&v, s.fonts).iter().map(|&f| v.as_int(f)).collect::<Vec<_>>(), [Some(1), Some(3)]);
        assert_eq!(unit_shape(&v, 77, &[placed], &none), None);
        let x = v.list(b.clone());
        let mut side = Side::default();
        let Some(Node::Box(d)) = decode(&v, x, &mut side) else { panic!("a box") };
        assert!(lazy_list(&d, &side).is_some_and(|l| is_sealed(&v, l)));
        assert_eq!(encode_box(&v, &d, &side), x);
    }

    /// A line with a `\write` and a link in a box in it is sealed with
    /// its skeleton: decoded, its whatsits are on the table in walk
    /// order after the page's before it, and its list filled in from the
    /// line numbers them there.
    #[test]
    fn whatsits_ride_in_the_skeleton() {
        fn handles(l: &[Node], hs: &mut Vec<u32>) {
            for n in l {
                match n {
                    Node::Whatsit(h, _) => hs.push(*h),
                    Node::Box(b) => handles(&b.list, hs),
                    _ => {}
                }
            }
        }
        let v = Values::new();
        let ch = |f: i64, c: i64| v.list(vec![v.int(T_CHAR), v.int(f), v.int(c)]);
        let what = |k: &str| v.list(vec![v.int(T_WHATSIT), v.list(vec![v.str(k), v.int(1)])]);
        let inner = list_of(&v, line(&v, vec![what("pdf:startlink"), ch(2, 66), what("pdf:endlink")]))[1];
        let ln = line(&v, vec![ch(3, 65), what("write"), inner, ch(3, 67)]);
        let none = crate::FxHashMap::default();
        let shape = unit_shape(&v, 5, &[ln], &none).expect("sealed");
        let b = list_of(&v, list_of(&v, list_of(&v, shape)[0])[1]);
        let s = sealed_of(&v, b[9]).expect("a stand-in");
        assert_ne!(s.skel, ValueId::UNIT);
        assert_eq!(list_of(&v, s.fonts).iter().map(|&f| v.as_int(f)).collect::<Vec<_>>(), [Some(2), Some(3)]);
        // a page: a whatsit, then the line
        let page = v.list(vec![
            v.int(T_BOX), v.int(1), v.int(0), v.int(0), v.int(0), v.int(0), v.int(0), v.int(0), v.int(0),
            v.list(vec![what("open"), v.list(b.clone())]),
        ]);
        let mut side = Side::default();
        let Some(Node::Box(mut pg)) = decode(&v, page, &mut side) else { panic!("a box") };
        let kinds: Vec<String> = side
            .whats
            .iter()
            .map(|&w| crate::val::str_of(&v, list_of(&v, w)[0]))
            .collect();
        assert_eq!(kinds, ["open", "write", "pdf:startlink", "pdf:endlink"]);
        let Node::Box(l) = &pg.list[1] else { panic!("the line") };
        assert_eq!(sealed_skeleton(l, &side).map(<[Node]>::len), Some(2));
        let bases = sealed_bases(&side);
        assert_eq!(bases, [(0, 1, 3)]);
        let real = list_of(&v, ln)[1];
        let mut saved = Vec::new();
        assert!(fill_sealed(&v, &mut pg.list, &side.lazy, &bases, &mut |_| Some(real), &mut 0, &mut saved));
        let Node::Box(l) = &pg.list[1] else { panic!("the line") };
        let mut hs = Vec::new();
        handles(&l.list, &mut hs);
        assert_eq!(hs, [1, 2, 3]);
    }
}
