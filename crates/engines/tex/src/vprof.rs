//! Vertical profiles (docs/spec-predicted-breaks.md §2, slice S1).
//!
//! A piece's profile is its main-list items in contribution order,
//! reduced to what the page builder (§980–§1010) reads of them: a box's
//! kind, height and depth (and the interline parameters in force, once
//! while they do not change), glue, kerns, penalties, marks and
//! whatsits by position, an insertion's class, size, split parameters
//! and the profile of its own list, and the builder's markers. One
//! profile item per raw item. A piece contributes its profile as one
//! unit of the accumulator [`VPROF`], at its position.
//!
//! [`ScanState::fold_items`] is the page builder over profiles: the same
//! [`PageBuilder::contribute`] on reduced nodes (a box with no list),
//! the box 255 not packaged ([`PageBuilder::measure_only`]); around it
//! the bookkeeping of [`crate::pages::PageState::fold_items`] (§679's
//! interline glue from the previous depth, deletions, markers). The
//! scan steps (`crate::pages::Scan`) run it; where a page fires, the
//! step builds the page from the items and compares the decisions.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use phitex_defuse::{ValueId, Values};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{BoxKind, BoxNode, Insert, KernKind, List, Node, Rule, WhatsitSize};
use phitex_typeset::page::{Fired, InsertClass, Outcome, PageBuilder, PageIns, PageParams};

use crate::build::FoldState;
use crate::nodes::{self, Side};
use crate::val::{glue_of, int_at, int_of, list_of, order};

/// The accumulator of the pieces' profiles (one unit per piece).
pub(crate) const VPROF: &str = "vprof";

// the profile items' tags
const P_VBOX: i64 = 1; // kind, h, d (after P_ILP when the parameters change)
const P_ILP: i64 = 2; // baselineskip glue4, lineskip glue4, lineskiplimit
const P_BOX: i64 = 3; // kind (0 h, 1 v, 2 rule), h, d
const P_GLUE: i64 = 4; // glue4
const P_KERN: i64 = 5; // w
const P_PEN: i64 = 6; // n
const P_MARK: i64 = 7;
const P_WHAT: i64 = 8;
const P_WHATS: i64 = 9; // h, d
const P_INS: i64 = 10; // n, h, d, float cost, split top skip glue4, len, items
const P_PD: i64 = 11; // d
const P_DEL: i64 = 12;
const P_BP: i64 = 13; // k
const P_PP: i64 = 14; // (the parameters reaching the scan)
const P_PPV: i64 = 15; // vsize, maxdepth, topskip glue4, holdinginserts
const P_SET: i64 = 16; // i, x
const P_INSP: i64 = 17; // n (the registers reaching the scan)
const P_INSPV: i64 = 18; // n, box h+d or -1, is hbox, count, dimen, skip glue4
const P_END: i64 = 19; // len, items
const P_ABORT: i64 = 20;
const P_SF: i64 = 21; // kind, n
const P_OPAQUE: i64 = 22; // tag

fn put_glue(out: &mut Vec<i64>, g: &Glue) {
    out.extend([
        i64::from(g.width),
        i64::from(g.stretch),
        i64::from(g.shrink),
        (g.stretch_order as i64) * 4 + g.shrink_order as i64,
    ]);
}

/// A piece's raw main-list items as a profile (one item per raw item).
pub(crate) fn encode(v: &Values, raw: &[ValueId]) -> Vec<i64> {
    let mut out = Vec::with_capacity(raw.len() * 4);
    let mut ilp: Option<(Glue, Glue, i32)> = None;
    for &x in raw {
        push_item(v, x, &mut out, &mut ilp);
    }
    out
}

/// The profile as a value: a list of inline integers.
pub(crate) fn value(v: &Values, ints: &[i64]) -> ValueId {
    v.list(ints.iter().map(|&i| v.int(i)).collect())
}

/// A piece's unit of [`VPROF`]: `[piece, profile]`, the piece's node
/// (its raw id) and its profile ([`value`]).
pub(crate) fn unit(v: &Values, piece: i64, ints: &[i64]) -> ValueId {
    v.list(vec![v.int(piece), value(v, ints)])
}

/// A unit's piece and profile.
pub(crate) fn unit_parts(v: &Values, x: ValueId) -> (i64, Vec<i64>) {
    let xs = list_of(v, x);
    let piece = xs.first().map_or(-1, |&p| int_of(v, p));
    let ints = xs
        .get(1)
        .map(|&p| list_of(v, p).into_iter().map(|y| int_of(v, y)).collect())
        .unwrap_or_default();
    (piece, ints)
}

fn i32_at(v: &Values, x: ValueId, k: usize) -> i32 {
    i32::try_from(int_at(v, x, k)).unwrap_or(0)
}

fn push_item(v: &Values, x: ValueId, out: &mut Vec<i64>, ilp: &mut Option<(Glue, Glue, i32)>) {
    match nodes::tag(v, x) {
        nodes::T_VBOX => {
            let ys = list_of(v, x);
            let b = ys.get(1).copied().unwrap_or(ValueId::UNIT);
            let p = (
                glue_of(v, ys.get(2).copied()),
                glue_of(v, ys.get(3).copied()),
                i32_at(v, x, 4),
            );
            if *ilp != Some(p) {
                out.push(P_ILP);
                put_glue(out, &p.0);
                put_glue(out, &p.1);
                out.push(i64::from(p.2));
                *ilp = Some(p);
            }
            out.extend([
                P_VBOX,
                i64::from(!nodes::box_is_h(v, b)),
                i64::from(nodes::box_dim(v, b, 3)),
                i64::from(nodes::box_dim(v, b, 2)),
            ]);
        }
        nodes::T_PD => out.extend([P_PD, int_at(v, x, 1)]),
        nodes::T_DELETE => out.push(P_DEL),
        nodes::T_BUILD_PAGE => out.extend([P_BP, int_at(v, x, 1)]),
        nodes::T_PAGE_PARAMS => {
            if list_of(v, x).len() == 1 {
                out.push(P_PP);
            } else {
                let p = crate::pages::params_of(v, x);
                out.extend([P_PPV, i64::from(p.vsize), i64::from(p.max_depth)]);
                put_glue(out, &p.top_skip);
                out.push(i64::from(p.holding_inserts));
            }
        }
        nodes::T_SET_PAGE => out.extend([P_SET, int_at(v, x, 1), int_at(v, x, 2)]),
        nodes::T_INS_PARAMS => {
            if list_of(v, x).len() == 2 {
                out.extend([P_INSP, int_at(v, x, 1)]);
            } else {
                let (n, c) = crate::pages::class_of(v, x);
                out.extend([
                    P_INSPV,
                    i64::from(n),
                    c.box_height_plus_depth.map_or(-1, i64::from),
                    i64::from(c.box_is_hbox),
                    i64::from(c.count),
                    i64::from(c.dimen),
                ]);
                put_glue(out, &c.skip);
            }
        }
        nodes::T_END => {
            let xs = list_of(v, x);
            out.extend([
                P_END,
                i64::try_from(xs.len().saturating_sub(1)).unwrap_or(0),
            ]);
            for &y in xs.iter().skip(1) {
                push_item(v, y, out, ilp);
            }
        }
        nodes::T_ABORT => out.push(P_ABORT),
        nodes::T_SF => out.extend([P_SF, int_at(v, x, 1), int_at(v, x, 2)]),
        t @ (nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT | nodes::T_SPACE) => {
            out.extend([P_OPAQUE, t]);
        }
        t => match nodes::decode_shallow(v, x, &mut Side::default()) {
            Some(n) => push_node(&n, out),
            None => out.extend([P_OPAQUE, t]),
        },
    }
}

/// A node as a profile item.
pub(crate) fn push_node(n: &Node, out: &mut Vec<i64>) {
    match n {
        Node::Box(b) => out.extend([
            P_BOX,
            i64::from(b.kind != BoxKind::H),
            i64::from(b.height),
            i64::from(b.depth),
        ]),
        Node::Rule(r) => out.extend([P_BOX, 2, i64::from(r.height), i64::from(r.depth)]),
        Node::Glue(g) => {
            out.push(P_GLUE);
            put_glue(out, &g.spec);
        }
        Node::Kern(w, _) => out.extend([P_KERN, i64::from(*w)]),
        Node::Penalty(p) => out.extend([P_PEN, i64::from(*p)]),
        Node::Mark(_) => out.push(P_MARK),
        Node::Whatsit(_, Some(s)) => out.extend([P_WHATS, i64::from(s.height), i64::from(s.depth)]),
        Node::Whatsit(..) | Node::Language { .. } => out.push(P_WHAT),
        Node::Ins(i) => {
            out.extend([
                P_INS,
                i64::from(i.number),
                i64::from(i.height),
                i64::from(i.depth),
                i64::from(i.float_cost),
            ]);
            put_glue(out, &i.split_top_skip);
            out.push(i64::try_from(i.list.len()).unwrap_or(0));
            for y in &i.list {
                push_node(y, out);
            }
        }
        // (not on a vertical list: the builder would stop on it)
        _ => out.extend([P_OPAQUE, -1]),
    }
}

// ---------------------------------------------------------- decoding

/// A profile item.
#[derive(Clone, Debug)]
pub(crate) enum PItem {
    Node(Node),
    VBox {
        node: Node,
        bs: Glue,
        ls: Glue,
        lsl: i32,
    },
    Pd(i32),
    Delete,
    Build(u32),
    /// `None`: the parameters reaching the scan.
    Params(Option<PageParams>),
    SetPage(i64, i32),
    /// `None`: the registers reaching the scan.
    InsParams(u16, Option<InsertClass>),
    End(Vec<PItem>),
    Abort,
    Sf(i64, i32),
    Opaque(i64),
}

struct Reader<'a> {
    xs: &'a [i64],
    at: usize,
    ilp: (Glue, Glue, i32),
}

impl Reader<'_> {
    fn next(&mut self) -> i64 {
        let x = self.xs.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        x
    }

    fn n32(&mut self) -> i32 {
        i32::try_from(self.next()).unwrap_or(0)
    }

    fn glue(&mut self) -> Glue {
        let (width, stretch, shrink, o) = (self.n32(), self.n32(), self.n32(), self.next());
        Glue {
            width,
            stretch,
            stretch_order: order(o / 4),
            shrink,
            shrink_order: order(o % 4),
        }
    }

    fn node(&mut self, t: i64) -> Option<Node> {
        Some(match t {
            P_BOX => {
                let (k, height, depth) = (self.next(), self.n32(), self.n32());
                if k == 2 {
                    Node::Rule(Rule {
                        width: 0,
                        height,
                        depth,
                    })
                } else {
                    Node::Box(BoxNode {
                        kind: if k == 0 { BoxKind::H } else { BoxKind::V },
                        height,
                        depth,
                        ..BoxNode::default()
                    })
                }
            }
            P_GLUE => Node::glue(self.glue()),
            P_KERN => Node::Kern(self.n32(), KernKind::Explicit),
            P_PEN => Node::Penalty(self.n32()),
            P_MARK => Node::Mark(0),
            P_WHAT => Node::Whatsit(0, None),
            P_WHATS => {
                let (height, depth) = (self.n32(), self.n32());
                Node::Whatsit(
                    0,
                    Some(WhatsitSize {
                        width: 0,
                        height,
                        depth,
                    }),
                )
            }
            P_INS => {
                let (number, height, depth, float_cost) =
                    (self.next(), self.n32(), self.n32(), self.n32());
                let split_top_skip = self.glue();
                let len = usize::try_from(self.next()).unwrap_or(0);
                let mut list = List::with_capacity(len);
                for _ in 0..len {
                    let t = self.next();
                    match self.node(t) {
                        Some(n) => list.push(n),
                        None => {
                            self.next();
                        }
                    }
                }
                Node::Ins(Insert {
                    number: u16::try_from(number).unwrap_or(0),
                    height,
                    depth,
                    split_top_skip,
                    float_cost,
                    list,
                })
            }
            _ => return None,
        })
    }

    fn item(&mut self) -> PItem {
        let mut t = self.next();
        if t == P_ILP {
            self.ilp = (self.glue(), self.glue(), self.n32());
            t = self.next();
        }
        match t {
            P_VBOX => {
                let (k, height, depth) = (self.next(), self.n32(), self.n32());
                PItem::VBox {
                    node: Node::Box(BoxNode {
                        kind: if k == 0 { BoxKind::H } else { BoxKind::V },
                        height,
                        depth,
                        ..BoxNode::default()
                    }),
                    bs: self.ilp.0,
                    ls: self.ilp.1,
                    lsl: self.ilp.2,
                }
            }
            P_PD => PItem::Pd(self.n32()),
            P_DEL => PItem::Delete,
            P_BP => PItem::Build(u32::try_from(self.next()).unwrap_or(0)),
            P_PP => PItem::Params(None),
            P_PPV => {
                let (vsize, max_depth) = (self.n32(), self.n32());
                let top_skip = self.glue();
                PItem::Params(Some(PageParams {
                    vsize,
                    max_depth,
                    top_skip,
                    holding_inserts: self.n32(),
                    saving_vdiscards: false,
                }))
            }
            P_SET => PItem::SetPage(self.next(), self.n32()),
            P_INSP => PItem::InsParams(u16::try_from(self.next()).unwrap_or(0), None),
            P_INSPV => {
                let n = u16::try_from(self.next()).unwrap_or(0);
                let h = self.next();
                let (is_h, count, dimen) = (self.next(), self.n32(), self.n32());
                let skip = self.glue();
                PItem::InsParams(
                    n,
                    Some(InsertClass {
                        box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
                        box_is_hbox: is_h != 0,
                        count,
                        dimen,
                        skip,
                    }),
                )
            }
            P_END => {
                let n = usize::try_from(self.next()).unwrap_or(0);
                PItem::End((0..n).map(|_| self.item()).collect())
            }
            P_ABORT => PItem::Abort,
            P_SF => PItem::Sf(self.next(), self.n32()),
            P_OPAQUE => PItem::Opaque(self.next()),
            t => self.node(t).map_or(PItem::Opaque(-2), PItem::Node),
        }
    }
}

/// A profile's items.
pub(crate) fn decode(xs: &[i64]) -> Vec<PItem> {
    let mut r = Reader {
        xs,
        at: 0,
        ilp: (Glue::ZERO, Glue::ZERO, 0),
    };
    let mut out = Vec::new();
    while r.at < xs.len() {
        out.push(r.item());
    }
    out
}

fn is_marker(x: &PItem) -> bool {
    match x {
        PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => true,
        PItem::Opaque(t) => matches!(*t, nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT),
        _ => false,
    }
}

// --------------------------------------------------------------- scan

/// The decisions of a page fired (§1012–§1022), as both builders give
/// them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decision {
    pub best: usize,
    pub least_cost: i32,
    pub best_size: i32,
    pub output_penalty: i32,
    pub mark_at: Vec<usize>,
    pub ins_records: Vec<PageIns>,
    pub inserts: Vec<(u16, usize)>,
    pub held: usize,
    pub carried: usize,
}

impl Decision {
    pub(crate) fn of(f: &Fired, pb: &PageBuilder, carried: usize) -> Decision {
        Decision {
            best: f.best,
            least_cost: pb.least_cost,
            best_size: pb.best_size,
            output_penalty: f.output_penalty,
            mark_at: f.mark_at.clone(),
            ins_records: f.ins_records.clone(),
            inserts: f.inserts.iter().map(|(n, l)| (*n, l.len())).collect(),
            held: f.held_inserts.len(),
            carried,
        }
    }
}

/// A page fired by the scan: at candidate `k`, the decisions, the
/// profile items after the candidate.
pub(crate) struct ScanFire {
    pub k: u32,
    pub decision: Decision,
    pub rest: usize,
}

/// The page builder's state over profiles.
pub(crate) struct ScanState {
    pub j: i64,
    /// The pages fired so far whose `\outputpenalty` is above -10001
    /// ([`crate::pages::PageState::normal`]).
    pub normal: i64,
    pub pb: PageBuilder,
    pub fold: FoldState,
    pub params: Option<PageParams>,
    pub pending: Vec<PItem>,
}

/// What the scan reads where it runs: the page parameters and an
/// insertion class's registers in force.
pub(crate) trait Reaching {
    fn params(&mut self) -> PageParams;
    fn class(&mut self, n: u16) -> InsertClass;
}

impl ScanState {
    /// The state just after a page fired (§1017: an empty page, the
    /// contributions all carried), as the scan state defines it.
    pub(crate) fn fresh(
        j: i64,
        normal: i64,
        fold: FoldState,
        params: Option<PageParams>,
    ) -> ScanState {
        let mut pb = PageBuilder::new();
        pb.measure_only = true;
        ScanState {
            j,
            normal,
            pb,
            fold,
            params,
            pending: Vec::new(),
        }
    }

    /// [`crate::pages::PageState::fold_items`] over profile items.
    pub(crate) fn fold_items(
        &mut self,
        items: &[PItem],
        dead: i64,
        rd: &mut dyn Reaching,
    ) -> Option<ScanFire> {
        let mut buf = std::mem::take(&mut self.pending);
        for (i, x) in items.iter().enumerate() {
            match x {
                PItem::Build(k) => {
                    self.fold.since_bp = 0;
                    if let Some(mut f) = self.build(&mut buf, *k, dead, rd) {
                        f.rest = items.len() - i - 1;
                        return Some(f);
                    }
                }
                PItem::SetPage(..) | PItem::InsParams(..) | PItem::End(_) | PItem::Abort => {
                    buf.push(x.clone());
                }
                _ => self.fold_item(x, &mut buf),
            }
        }
        self.pending = buf;
        None
    }

    /// [`crate::build::fold_item`] on the main list.
    fn fold_item(&mut self, x: &PItem, out: &mut Vec<PItem>) {
        let st = &mut self.fold;
        match x {
            PItem::Delete => {
                if st.since_bp > 0 {
                    if let Some(i) = out.iter().rposition(|y| !is_marker(y)) {
                        out.remove(i);
                    }
                    st.since_bp -= 1;
                }
            }
            PItem::Params(_) | PItem::SetPage(..) | PItem::InsParams(..) => out.push(x.clone()),
            PItem::Opaque(t)
                if matches!(*t, nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT) =>
            {
                out.push(x.clone());
            }
            PItem::Build(_) => st.since_bp = 0,
            PItem::Pd(d) => st.pd = *d,
            PItem::VBox { node, bs, ls, lsl } => {
                let ht = node.box_dims().map_or(0, |d| d.1);
                if let Some(g) =
                    phitex_typeset::linebreak::interline_glue(st.pd, ht, *bs, *ls, *lsl)
                {
                    out.push(PItem::Node(g));
                    st.since_bp += 1;
                }
                out.push(PItem::Node(node.clone()));
                st.since_bp += 1;
                st.pd = node.box_dims().map_or(0, |d| d.2);
            }
            PItem::Sf(k, s) => {
                st.sf = if *k == 0 || st.sf >= 1000 { *s } else { 1000 };
            }
            _ => {
                out.push(x.clone());
                st.since_bp += 1;
            }
        }
    }

    /// [`crate::pages::PageState`]'s `build` over profile items.
    fn build(
        &mut self,
        buf: &mut Vec<PItem>,
        k: u32,
        dead: i64,
        rd: &mut dyn Reaching,
    ) -> Option<ScanFire> {
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        let mut page_params = None;
        let so_far = &mut self.pb.so_far;
        buf.retain(|x| match x {
            PItem::Params(p) => {
                page_params = Some(p.unwrap_or_else(|| rd.params()));
                false
            }
            PItem::SetPage(i, x) => {
                let i = usize::try_from(*i).unwrap_or(8);
                if i < 8 {
                    so_far[i] = *x;
                }
                false
            }
            PItem::InsParams(n, c) => {
                classes.push((*n, c.unwrap_or_else(|| rd.class(*n))));
                false
            }
            _ => true,
        });
        if page_params.is_some() {
            self.params = page_params;
        }
        let params = self.params.unwrap_or(PageParams {
            vsize: 0,
            max_depth: 0,
            top_skip: Glue::ZERO,
            holding_inserts: 0,
            saving_vdiscards: false,
        });
        let mut queue: VecDeque<PItem> = std::mem::take(buf).into();
        let mut end_seen = false;
        while let Some(x) = queue.pop_front() {
            let p = match x {
                PItem::End(ref ys) => {
                    end_seen = true;
                    if self.pb.page.is_empty() && queue.is_empty() && dead == 0 {
                        continue;
                    }
                    for y in ys.iter().rev() {
                        queue.push_front(y.clone());
                    }
                    continue;
                }
                PItem::Abort => {
                    buf.clear();
                    return None;
                }
                PItem::Node(ref n) => n.clone(),
                _ => continue,
            };
            let next = if matches!(p, Node::Kern(..)) {
                match queue.front() {
                    Some(PItem::Node(n)) => Some(n.clone()),
                    _ => None,
                }
            } else {
                None
            };
            let out = {
                let mut class = |n: u16| {
                    classes
                        .iter()
                        .find(|c| c.0 == n)
                        .map_or_else(|| rd.class(n), |c| c.1)
                };
                self.pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            match out {
                Outcome::Done => {}
                Outcome::Wait(_) => {
                    queue.push_front(x);
                    break;
                }
                Outcome::Fire(f) => {
                    let carried = f.contributions.len() + queue.len() + usize::from(end_seen);
                    let decision = Decision::of(&f, &self.pb, carried);
                    self.pb = PageBuilder::new();
                    self.pb.measure_only = true;
                    self.j += 1;
                    if f.output_penalty > -10001 {
                        self.normal += 1;
                    }
                    self.fold.since_bp = 0;
                    buf.clear();
                    return Some(ScanFire {
                        k,
                        decision,
                        rest: 0,
                    });
                }
            }
        }
        *buf = queue.into();
        None
    }
}

// -------------------------------------------------------------- sizes

static UNITS: AtomicU64 = AtomicU64::new(0);
static INTS: AtomicU64 = AtomicU64::new(0);
static LINES: AtomicU64 = AtomicU64::new(0);

/// A profile contributed: counted for the size report (units, bytes,
/// boxes appended with interline glue: the lines).
pub(crate) fn count_unit(ints: &[i64]) {
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        let lines = decode(ints)
            .iter()
            .filter(|x| matches!(x, PItem::VBox { .. }))
            .count() as u64;
        let u = UNITS.fetch_add(1, Relaxed) + 1;
        INTS.fetch_add(ints.len() as u64, Relaxed);
        LINES.fetch_add(lines, Relaxed);
        if u.is_power_of_two() || u.is_multiple_of(1000) {
            eprintln!("{}", size_report());
        }
    }
}

fn size_report() -> String {
    let (u, n, l) = (UNITS.load(Relaxed), INTS.load(Relaxed), LINES.load(Relaxed));
    format!(
        "vprof: units {u} bytes {} per unit {} lines {l} per line {}",
        n * 8,
        n * 8 / u.max(1),
        n * 8 / l.max(1)
    )
}

/// The first decision two builders differ in, for a diagnostic.
pub(crate) fn first_decision_diff(a: &Decision, b: &Decision) -> String {
    macro_rules! cmp {
        ($($f:ident),*) => {
            $(if a.$f != b.$f {
                return format!("{}: scan {:?} real {:?}", stringify!($f), a.$f, b.$f);
            })*
        };
    }
    cmp!(
        best,
        least_cost,
        best_size,
        output_penalty,
        mark_at,
        ins_records,
        inserts,
        held,
        carried
    );
    String::from("decision")
}

/// Whether profile items hold an `\end` marker (the scan then reads
/// `\deadcycles`, §1054).
pub(crate) fn has_end(items: &[PItem]) -> bool {
    items.iter().any(|x| matches!(x, PItem::End(_)))
}
