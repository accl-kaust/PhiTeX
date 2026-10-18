//! Breaking vertical lists into pages (§967–§1028): `prune_page_top`,
//! `vert_break`, `vsplit`, and the page builder's decisions (`build_page`
//! and `fire_up`) with e-TeX's `\lastnodetype` and saved discards.
//!
//! The page builder owns the current page. The caller contributes nodes
//! one by one and acts on [`Outcome::Fire`]: run `\output` (or ship the
//! page out), then contribute again what [`Fired`] hands back. Marks stay
//! opaque: the caller derives `\topmark`, `\firstmark` and `\botmark`
//! (and e-TeX's classes) from [`Fired::marks`].

use crate::arith::{MAX_DIMEN, Scaled, badness, x_over_n};
use crate::glue::{Glue, Order};
use crate::linebreak::{AWFUL_BAD, EJECT_PENALTY, INF_PENALTY};
use crate::node::{Insert, List, Node, param};
use crate::pack::{NATURAL, PackParams, Packed, Spec, vpack, vpackage};

/// `deplorable` (§974).
pub const DEPLORABLE: i32 = 100_000;

/// `prune_page_top(p, s)` (§968, e-TeX's form): deletes glue, kern and
/// penalty nodes before the first box or rule and puts `\splittopskip`
/// glue before it. Returns the list and the deleted nodes when `save`.
///
/// # Panics
/// On nodes that cannot be in a vertical list (`confusion("pruning")`).
#[must_use]
pub fn prune_page_top(list: List, split_top_skip: Glue, save: bool) -> (List, List) {
    let mut out = List::with_capacity(list.len() + 1);
    let mut discards = List::new();
    let mut it = list.into_iter();
    while let Some(p) = it.next() {
        match p {
            Node::Box(_) | Node::Rule(_) => {
                // §969
                let height = p.box_dims().map_or(0, |d| d.1);
                let width = if split_top_skip.width > height {
                    split_top_skip.width - height
                } else {
                    0
                };
                out.push(Node::param_glue(
                    param::SPLIT_TOP_SKIP,
                    Glue {
                        width,
                        ..split_top_skip
                    },
                ));
                out.push(p);
                out.extend(it);
                break;
            }
            Node::Whatsit(..) | Node::Language { .. } | Node::Mark(_) | Node::Ins(_) => out.push(p),
            Node::Glue(_) | Node::Kern(..) | Node::Penalty(_) => {
                if save {
                    discards.push(p);
                }
            }
            p => panic!("pruning: {p:?}"),
        }
    }
    (out, discards)
}

/// The result of [`vert_break`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertBreak {
    /// The index of the best break, `None` for the end of the list.
    pub best: Option<usize>,
    /// `best_height_plus_depth` (§971).
    pub best_height_plus_depth: Scaled,
    /// "Infinite glue shrinkage found in box being split" (§976); the
    /// glue was made finite in the list.
    pub infinite_shrink_error: bool,
}

/// `vert_break(p, h, d)` (§970): the best place to break `list` for a box
/// of height `h` and maximum depth `d`.
///
/// # Panics
/// On nodes that cannot be in a vertical list.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn vert_break(list: &mut List, h: Scaled, d: Scaled) -> VertBreak {
    let mut active = [0 as Scaled; 6];
    let mut prev_dp: Scaled = 0;
    let mut least_cost = AWFUL_BAD;
    let mut result = VertBreak {
        best: None,
        best_height_plus_depth: 0,
        infinite_shrink_error: false,
    };
    let mut prev: Option<usize> = None; // `prev_p`; the first node is no legal break
    let mut i = 0;
    loop {
        // §972, §973
        enum Next {
            Break(i32),
            NotFound,
            UpdateHeights,
        }
        let next = match list.get(i) {
            None => Next::Break(EJECT_PENALTY),
            Some(p) => match p {
                Node::Box(_) | Node::Rule(_) => {
                    let (_, ht, dp, _) = p.box_dims().unwrap_or_default();
                    active[0] += prev_dp + ht;
                    prev_dp = dp;
                    Next::NotFound
                }
                Node::Whatsit(_, Some(s)) => {
                    // pdfTeX: "Process whatsit p in vert_break loop"
                    active[0] += prev_dp + s.height;
                    prev_dp = s.depth;
                    Next::NotFound
                }
                Node::Whatsit(..) | Node::Language { .. } | Node::Mark(_) | Node::Ins(_) => {
                    Next::NotFound
                }
                Node::Glue(_) => {
                    if prev.is_some_and(|q| list[q].precedes_break()) {
                        Next::Break(0)
                    } else {
                        Next::UpdateHeights
                    }
                }
                Node::Kern(..) => {
                    if matches!(list.get(i + 1), Some(Node::Glue(_))) {
                        Next::Break(0)
                    } else {
                        Next::UpdateHeights
                    }
                }
                Node::Penalty(pen) => Next::Break(*pen),
                p => panic!("vertbreak: {p:?}"),
            },
        };
        let mut update = matches!(next, Next::UpdateHeights);
        if let Next::Break(pi) = next {
            // §974
            if pi < INF_PENALTY {
                // §975
                let mut b = if active[0] < h {
                    if active[2] != 0 || active[3] != 0 || active[4] != 0 {
                        0
                    } else {
                        badness(h - active[0], active[1])
                    }
                } else if active[0] - h > active[5] {
                    AWFUL_BAD
                } else {
                    badness(active[0] - h, active[5])
                };
                if b < AWFUL_BAD {
                    b = if pi <= EJECT_PENALTY {
                        pi
                    } else if b < crate::arith::INF_BAD {
                        b + pi
                    } else {
                        DEPLORABLE
                    };
                }
                if b <= least_cost {
                    result.best = if i < list.len() { Some(i) } else { None };
                    least_cost = b;
                    result.best_height_plus_depth = active[0] + prev_dp;
                }
                if b == AWFUL_BAD || pi <= EJECT_PENALTY {
                    return result;
                }
            }
            update = matches!(list.get(i), Some(Node::Glue(_) | Node::Kern(..)));
        }
        if update {
            // §976
            let w = match &mut list[i] {
                Node::Kern(w, _) => *w,
                Node::Glue(g) => {
                    active[1 + g.spec.stretch_order as usize] += g.spec.stretch;
                    active[5] += g.spec.shrink;
                    if g.spec.shrink_order != Order::Normal && g.spec.shrink != 0 {
                        result.infinite_shrink_error = true;
                        g.spec.shrink_order = Order::Normal;
                    }
                    g.spec.width
                }
                _ => unreachable!(),
            };
            active[0] += prev_dp + w;
            prev_dp = 0;
        }
        // not_found:
        if prev_dp > d {
            active[0] += prev_dp - d;
            prev_dp = d;
        }
        prev = Some(i);
        i += 1;
    }
}

/// The result of [`vsplit`].
#[derive(Clone, Debug, PartialEq)]
pub struct VSplit {
    /// The split-off box (`vpackage(p, h, exactly, split_max_depth)`).
    pub page: Packed,
    /// What remains of the box, `None` if it was entirely extracted.
    pub rest: Option<crate::node::BoxNode>,
    /// The marks in the split-off part, in order (`\splitfirstmark` is
    /// the first, `\splitbotmark` the last).
    pub marks: Vec<u32>,
    /// The nodes pruned from the top of the rest (e-TeX's
    /// `\splitdiscards`, when saving).
    pub discards: List,
    /// From [`vert_break`].
    pub infinite_shrink_error: bool,
}

/// `vsplit(n, h)` (§977) on the list of a vbox.
#[must_use]
pub fn vsplit(
    mut list: List,
    h: Scaled,
    split_max_depth: Scaled,
    split_top_skip: Glue,
    saving_vdiscards: bool,
    params: &PackParams,
) -> VSplit {
    let vb = vert_break(&mut list, h, split_max_depth);
    let q = vb.best.unwrap_or(list.len());
    let rest = list.split_off(q);
    // §979
    let marks = list
        .iter()
        .filter_map(|n| match n {
            Node::Mark(m) => Some(*m),
            _ => None,
        })
        .collect();
    let (rest, discards) = prune_page_top(rest, split_top_skip, saving_vdiscards);
    let rest = (!rest.is_empty()).then(|| vpack(rest, NATURAL, params).node);
    VSplit {
        page: vpackage(list, Spec::Exactly(h), split_max_depth, params),
        rest,
        marks,
        discards,
        infinite_shrink_error: vb.infinite_shrink_error,
    }
}

/// What the current page holds (§980).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contents {
    /// Only marks and whatsits.
    #[default]
    Empty,
    /// Insertions too.
    InsertsOnly,
    /// A box or rule.
    BoxThere,
}

/// What the page builder reads from eqtb for an insertion class `n`, at
/// the moment an `\insert n` is contributed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertClass {
    /// Height plus depth of `\box n`, or `None` if void.
    pub box_height_plus_depth: Option<Scaled>,
    /// Whether `\box n` holds an hbox (`ensure_vbox` complains and
    /// voids it, §993).
    pub box_is_hbox: bool,
    /// `\count n`.
    pub count: i32,
    /// `\dimen n`.
    pub dimen: Scaled,
    /// `\skip n`.
    pub skip: Glue,
}

/// The page builder's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageParams {
    /// `\vsize`.
    pub vsize: Scaled,
    /// `\maxdepth`.
    pub max_depth: Scaled,
    /// `\topskip`.
    pub top_skip: Glue,
    /// `\holdinginserts`.
    pub holding_inserts: i32,
    /// e-TeX's `\savingvdiscards > 0`.
    pub saving_vdiscards: bool,
}

/// A page insertion record (§981).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageIns {
    pub n: u16,
    pub split_up: bool,
    pub height: Scaled,
    /// Where the insertion `broken_ins` would break (index in its list),
    /// `None` at its end.
    pub broken_ptr: Option<usize>,
    /// The page index of the insertion that was tentatively split.
    pub broken_ins: Option<usize>,
    pub last_ins: Option<usize>,
    pub best_ins: Option<usize>,
}

/// The errors `build_page` would report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageDiag {
    /// "Infinite glue shrinkage found on current page" (§1004).
    InfiniteShrink,
    /// "Infinite glue shrinkage inserted from \skip n" (§1009).
    InfiniteShrinkInsert(u16),
    /// "Insertions can only be added to a vbox" (§993): `\box n` must be
    /// voided.
    InsertIntoHbox(u16),
    /// From splitting an insertion (§976).
    InfiniteShrinkSplit,
}

/// What became of a contributed node.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It went to the page or was discarded.
    Done,
    /// A kern at the end of the contributions: contribute it again when
    /// its successor is known (§1000).
    Wait(Node),
    /// The page is complete.
    Fire(Box<Fired>),
}

/// The completed page (`fire_up`, §1012–§1023).
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    /// `\box255`: `vpackage(page, best_size, exactly, page_max_depth)`,
    /// with reports suppressed.
    pub box255: Packed,
    /// `\outputpenalty` (§1013).
    pub output_penalty: i32,
    /// The marks on the page, in order.
    pub marks: Vec<u32>,
    /// For each insertion class that receives material: the nodes to
    /// append to `\box n` (made a vbox if void), which is then
    /// `vpack`ed natural (§1018–§1021).
    pub inserts: Vec<(u16, List)>,
    /// Insertions held over (§1022): they are on the page while
    /// `\output` runs, and go to the contributions before everything
    /// else.
    pub held_inserts: List,
    /// To put back at the front of the contribution list, after the
    /// output (§1017).
    pub contributions: List,
    /// `best_page_break`'s index on the page (§1014).
    pub best: usize,
    /// The page indices of the marks before the break, in order.
    pub mark_at: Vec<usize>,
    /// The insertion classes' records as the fire found them (§1018:
    /// `best_ins_ptr`, the split's `broken_ptr`/`broken_ins`).
    pub ins_records: Vec<PageIns>,
}

/// The page builder's state (§980–§991).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageBuilder {
    /// The current page.
    pub page: List,
    /// `page_contents`.
    pub contents: Contents,
    /// `page_so_far`: goal, total, stretch (four orders), shrink, depth.
    pub so_far: [Scaled; 8],
    /// `page_max_depth`.
    pub max_depth: Scaled,
    pub best_break: Option<usize>,
    /// `least_page_cost`.
    pub least_cost: i32,
    pub best_size: Scaled,
    /// `\lastskip` (`None` is `max_halfword`).
    pub last_glue: Option<Glue>,
    /// `\lastpenalty`.
    pub last_penalty: i32,
    /// `\lastkern`.
    pub last_kern: Scaled,
    /// e-TeX's `\lastnodetype` (-1 for none).
    pub last_node_type: i32,
    /// `\insertpenalties`.
    pub insert_penalties: i32,
    pub ins: Vec<PageIns>,
    /// e-TeX's `\pagediscards`.
    pub discards: List,
    /// What `build_page` complained about.
    pub diags: Vec<PageDiag>,
    /// The decisions only: `fire_up` does not package `\box255` (its
    /// `box255` is empty). The scan over vertical profiles sets it.
    pub measure_only: bool,
}

/// e-TeX's `\lastnodetype` of a node: tex.web's type plus one (etex.ch).
fn node_type(n: &Node) -> i32 {
    match n {
        Node::Char { .. } => 0,
        Node::Box(b) => {
            if b.kind == crate::node::BoxKind::H {
                1
            } else {
                2
            }
        }
        Node::Rule(_) => 3,
        Node::Ins(_) => 4,
        Node::Mark(_) => 5,
        Node::Adjust(_) => 6,
        Node::Lig(_) => 7,
        Node::Disc(_) => 8,
        Node::Whatsit(..) | Node::Language { .. } => 9,
        Node::Math(..) => 10,
        Node::Glue(_) => 11,
        Node::Kern(..) => 12,
        Node::Penalty(_) => 13,
        Node::Unset(_) => 14,
    }
}

fn scale_count(h: Scaled, count: i32) -> Scaled {
    if count == 1000 {
        h
    } else {
        x_over_n(h, 1000).value.wrapping_mul(count)
    }
}

impl PageBuilder {
    /// A builder with an empty page (§991).
    #[must_use]
    pub fn new() -> PageBuilder {
        PageBuilder {
            least_cost: AWFUL_BAD,
            last_node_type: -1,
            ..PageBuilder::default()
        }
    }

    /// `page_goal`.
    #[must_use]
    pub fn goal(&self) -> Scaled {
        self.so_far[0]
    }

    /// `page_total`.
    #[must_use]
    pub fn total(&self) -> Scaled {
        self.so_far[1]
    }

    /// `page_depth`.
    #[must_use]
    pub fn depth(&self) -> Scaled {
        self.so_far[7]
    }

    /// `freeze_page_specs(s)` (§987).
    fn freeze(&mut self, s: Contents, p: &PageParams) {
        self.contents = s;
        self.so_far = [p.vsize, 0, 0, 0, 0, 0, 0, 0];
        self.max_depth = p.max_depth;
        self.least_cost = AWFUL_BAD;
    }

    /// Ends the output routine (§1026): `\insertpenalties` back to zero
    /// and the saved discards flushed.
    pub fn output_done(&mut self) {
        self.insert_penalties = 0;
        self.discards.clear();
    }

    fn page_tail_precedes_break(&self) -> bool {
        // `type(page_head)=glue_node` (§988)
        self.page.last().is_some_and(Node::precedes_break)
    }

    /// Contributes `p` to the current page (§994–§1010); `next` is the
    /// node that follows it on the contribution list, if any. `class`
    /// answers for `\insert n`.
    ///
    /// # Panics
    /// On nodes that cannot be in the main vertical list.
    #[allow(clippy::too_many_lines)]
    pub fn contribute(
        &mut self,
        mut p: Node,
        next: Option<&Node>,
        params: &PageParams,
        class: &mut dyn FnMut(u16) -> InsertClass,
    ) -> Outcome {
        // §996
        self.last_glue = None;
        self.last_penalty = 0;
        self.last_kern = 0;
        self.last_node_type = node_type(&p);
        match &p {
            Node::Glue(g) => self.last_glue = Some(g.spec),
            Node::Penalty(n) => self.last_penalty = *n,
            Node::Kern(w, _) => self.last_kern = *w,
            _ => {}
        }
        // §1000
        let pi: Option<i32>;
        let mut update_heights = false;
        match &p {
            Node::Box(_) | Node::Rule(_) => {
                let (_, ht, dp, _) = p.box_dims().unwrap_or_default();
                if self.contents < Contents::BoxThere {
                    // §1001
                    if self.contents == Contents::Empty {
                        self.freeze(Contents::BoxThere, params);
                    } else {
                        self.contents = Contents::BoxThere;
                    }
                    let ts = params.top_skip;
                    let width = if ts.width > ht { ts.width - ht } else { 0 };
                    let q = Node::param_glue(param::TOP_SKIP, Glue { width, ..ts });
                    return match self.contribute(q, Some(&p), params, class) {
                        Outcome::Fire(mut f) => {
                            f.contributions.push(p);
                            Outcome::Fire(f)
                        }
                        _ => self.contribute(p, next, params, class),
                    };
                }
                // §1002
                self.so_far[1] += self.so_far[7] + ht;
                self.so_far[7] = dp;
                pi = None;
            }
            Node::Whatsit(_, Some(s)) => {
                // pdfTeX: "Prepare to move whatsit p to the current
                // page": its size counts, the page stays empty
                self.so_far[1] += self.so_far[7] + s.height;
                self.so_far[7] = s.depth;
                pi = None;
            }
            Node::Whatsit(..) | Node::Language { .. } | Node::Mark(_) => pi = None,
            Node::Glue(_) => {
                if self.contents < Contents::BoxThere {
                    return self.discard(p, params);
                }
                if self.page_tail_precedes_break() {
                    pi = Some(0);
                } else {
                    pi = None;
                    update_heights = true;
                }
            }
            Node::Kern(..) => {
                if self.contents < Contents::BoxThere {
                    return self.discard(p, params);
                }
                match next {
                    None => return Outcome::Wait(p),
                    Some(Node::Glue(_)) => pi = Some(0),
                    Some(_) => {
                        pi = None;
                        update_heights = true;
                    }
                }
            }
            Node::Penalty(n) => {
                if self.contents < Contents::BoxThere {
                    return self.discard(p, params);
                }
                pi = Some(*n);
            }
            Node::Ins(_) => {
                self.append_insertion(&mut p, params, class);
                pi = None;
            }
            p => panic!("page: {p:?}"),
        }
        if let Some(pi) = pi {
            // §1005
            if pi < INF_PENALTY {
                // §1007
                let (goal, total, shrink) = (self.so_far[0], self.so_far[1], self.so_far[6]);
                let b = if total < goal {
                    if self.so_far[3] != 0 || self.so_far[4] != 0 || self.so_far[5] != 0 {
                        0
                    } else {
                        badness(goal - total, self.so_far[2])
                    }
                } else if total - goal > shrink {
                    AWFUL_BAD
                } else {
                    badness(total - goal, shrink)
                };
                let mut c = if b < AWFUL_BAD {
                    if pi <= EJECT_PENALTY {
                        pi
                    } else if b < crate::arith::INF_BAD {
                        b + pi + self.insert_penalties
                    } else {
                        DEPLORABLE
                    }
                } else {
                    b
                };
                if self.insert_penalties >= 10000 {
                    c = AWFUL_BAD;
                }
                if c <= self.least_cost {
                    self.best_break = Some(self.page.len());
                    self.best_size = goal;
                    self.least_cost = c;
                    for r in &mut self.ins {
                        r.best_ins = r.last_ins;
                    }
                }
                if c == AWFUL_BAD || pi <= EJECT_PENALTY {
                    return Outcome::Fire(Box::new(self.fire_up(p, params)));
                }
            }
            update_heights = matches!(p, Node::Glue(_) | Node::Kern(..));
        }
        if update_heights {
            // §1004
            let w = match &mut p {
                Node::Kern(w, _) => *w,
                Node::Glue(g) => {
                    self.so_far[2 + g.spec.stretch_order as usize] += g.spec.stretch;
                    self.so_far[6] += g.spec.shrink;
                    if g.spec.shrink_order != Order::Normal && g.spec.shrink != 0 {
                        self.diags.push(PageDiag::InfiniteShrink);
                        g.spec.shrink_order = Order::Normal;
                    }
                    g.spec.width
                }
                _ => unreachable!(),
            };
            self.so_far[1] += self.so_far[7] + w;
            self.so_far[7] = 0;
        }
        // contribute: §1003
        if self.so_far[7] > self.max_depth {
            self.so_far[1] += self.so_far[7] - self.max_depth;
            self.so_far[7] = self.max_depth;
        }
        // §998
        self.page.push(p);
        Outcome::Done
    }

    /// done1: §999 with e-TeX's saved discards.
    fn discard(&mut self, p: Node, params: &PageParams) -> Outcome {
        if params.saving_vdiscards {
            self.discards.push(p);
        }
        Outcome::Done
    }

    /// §1008–§1010.
    fn append_insertion(
        &mut self,
        p: &mut Node,
        params: &PageParams,
        class: &mut dyn FnMut(u16) -> InsertClass,
    ) {
        let Node::Ins(ins) = p else { unreachable!() };
        if self.contents == Contents::Empty {
            self.freeze(Contents::InsertsOnly, params);
        }
        let n = ins.number;
        let cls = class(n);
        let k = match self.ins.binary_search_by_key(&n, |r| r.n) {
            Ok(k) => k,
            Err(k) => {
                // §1009
                let mut height = cls.box_height_plus_depth.unwrap_or(0);
                if cls.box_is_hbox {
                    self.diags.push(PageDiag::InsertIntoHbox(n));
                    height = 0;
                }
                self.ins.insert(
                    k,
                    PageIns {
                        n,
                        split_up: false,
                        height,
                        broken_ptr: None,
                        broken_ins: None,
                        last_ins: None,
                        best_ins: None,
                    },
                );
                let q = cls.skip;
                let h = scale_count(height, cls.count);
                self.so_far[0] = self.so_far[0] - h - q.width;
                self.so_far[2 + q.stretch_order as usize] += q.stretch;
                self.so_far[6] += q.shrink;
                if q.shrink_order != Order::Normal && q.shrink != 0 {
                    self.diags.push(PageDiag::InfiniteShrinkInsert(n));
                }
                k
            }
        };
        let page_index = self.page.len();
        let r = &mut self.ins[k];
        if r.split_up {
            self.insert_penalties += ins.float_cost;
            return;
        }
        r.last_ins = Some(page_index);
        let (goal, total, depth, shrink) = (
            self.so_far[0],
            self.so_far[1],
            self.so_far[7],
            self.so_far[6],
        );
        let delta = goal - total - depth + shrink;
        let h = scale_count(ins.height, cls.count);
        if (h <= 0 || h <= delta) && ins.height + r.height <= cls.dimen {
            self.so_far[0] -= h;
            r.height += ins.height;
            return;
        }
        // §1010
        let mut w = if cls.count <= 0 {
            MAX_DIMEN
        } else {
            let w = goal - total - depth;
            if cls.count == 1000 {
                w
            } else {
                x_over_n(w, cls.count).value.wrapping_mul(1000)
            }
        };
        if w > cls.dimen - r.height {
            w = cls.dimen - r.height;
        }
        let vb = vert_break(&mut ins.list, w, ins.depth);
        if vb.infinite_shrink_error {
            self.diags.push(PageDiag::InfiniteShrinkSplit);
        }
        r.height += vb.best_height_plus_depth;
        let bhpd = scale_count(vb.best_height_plus_depth, cls.count);
        self.so_far[0] -= bhpd;
        r.split_up = true;
        r.broken_ptr = vb.best;
        r.broken_ins = Some(page_index);
        self.insert_penalties += match vb.best {
            None => EJECT_PENALTY,
            Some(q) => match ins.list[q] {
                Node::Penalty(pen) => pen,
                _ => 0,
            },
        };
    }

    /// `fire_up(c)` (§1012–§1023): `c` is the node being contributed.
    #[allow(clippy::too_many_lines)]
    fn fire_up(&mut self, mut c: Node, params: &PageParams) -> Fired {
        let best = self.best_break.unwrap_or(self.page.len());
        let ins_records = self.ins.clone();
        // §1013
        let target = if best == self.page.len() {
            &mut c
        } else {
            &mut self.page[best]
        };
        let output_penalty = if let Node::Penalty(pen) = target {
            let v = *pen;
            *pen = INF_PENALTY;
            v
        } else {
            INF_PENALTY
        };
        // §1014
        let mut page = std::mem::take(&mut self.page);
        let mut rest = page.split_off(best);
        rest.push(c);
        self.insert_penalties = 0;
        let mut marks = Vec::new();
        let mut mark_at = Vec::new();
        let mut inserts: Vec<(u16, List)> = Vec::new();
        let mut held = List::new();
        let mut kept = List::with_capacity(page.len());
        let holding = params.holding_inserts > 0;
        for (idx, p) in page.into_iter().enumerate() {
            match p {
                Node::Ins(mut ins) if !holding => {
                    // §1020
                    let k = self
                        .ins
                        .iter()
                        .position(|r| r.n == ins.number)
                        .expect("insertion without page record");
                    let r = &mut self.ins[k];
                    let mut wait = true;
                    if r.best_ins.is_some() {
                        wait = false;
                        let slot = if let Some(s) = inserts.iter().position(|(n, _)| *n == r.n) {
                            s
                        } else {
                            inserts.push((r.n, List::new()));
                            inserts.len() - 1
                        };
                        if r.best_ins == Some(idx) {
                            // §1021
                            if r.split_up && r.broken_ins == Some(idx) {
                                if let Some(q) = r.broken_ptr {
                                    let tail = ins.list.split_off(q);
                                    inserts[slot].1.append(&mut ins.list);
                                    let (rem, _) = prune_page_top(tail, ins.split_top_skip, false);
                                    if !rem.is_empty() {
                                        let b = vpack(rem.clone(), NATURAL, &PackParams::default())
                                            .node;
                                        ins.height = b.height + b.depth;
                                        ins.list = rem;
                                        wait = true;
                                    }
                                } else {
                                    inserts[slot].1.append(&mut ins.list);
                                }
                            } else {
                                inserts[slot].1.append(&mut ins.list);
                            }
                            r.best_ins = None;
                        } else {
                            inserts[slot].1.append(&mut ins.list);
                        }
                    }
                    // §1022
                    if wait {
                        held.push(Node::Ins(ins));
                        self.insert_penalties += 1;
                    }
                }
                Node::Mark(m) => {
                    marks.push(m);
                    mark_at.push(idx);
                    kept.push(Node::Mark(m));
                }
                p => kept.push(p),
            }
        }
        // §1017
        let quiet = PackParams {
            badness: crate::arith::INF_BAD,
            fuzz: MAX_DIMEN,
            overfull_rule: 0,
        };
        let box255 = if self.measure_only {
            Packed {
                node: crate::node::BoxNode::default(),
                badness: 0,
                report: None,
                adjustments: List::new(),
            }
        } else {
            vpackage(kept, Spec::Exactly(self.best_size), self.max_depth, &quiet)
        };
        // §991, §1019
        self.contents = Contents::Empty;
        self.last_glue = None;
        self.last_penalty = 0;
        self.last_kern = 0;
        self.last_node_type = -1;
        self.so_far[7] = 0;
        self.max_depth = 0;
        self.best_break = None;
        self.ins.clear();
        Fired {
            box255,
            output_penalty,
            marks,
            inserts,
            held_inserts: held,
            contributions: rest,
            best,
            mark_at,
            ins_records,
        }
    }
}

/// An insertion's own height for `\insert` (§1100): the natural height
/// plus depth of its vertical list.
#[must_use]
pub fn insert_height(ins: &Insert) -> Scaled {
    let b = vpack(ins.list.clone(), NATURAL, &PackParams::default()).node;
    b.height + b.depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::BoxNode;

    const PT: Scaled = 0x1_0000;

    fn line(h: Scaled, d: Scaled) -> Node {
        Node::Box(Box::new(BoxNode {
            height: h,
            depth: d,
            ..BoxNode::default()
        }))
    }

    #[test]
    fn vert_break_takes_the_last_glue_that_fits() {
        // 8+2, glue 2, 8+2, glue 2, 8+2: 24pt fits two lines (8+2+2+8)
        let g = Node::glue(Glue::new(2 * PT, PT, 0));
        let mut l = vec![
            line(8 * PT, 2 * PT),
            g.clone(),
            line(8 * PT, 2 * PT),
            g,
            line(8 * PT, 2 * PT),
        ];
        let b = vert_break(&mut l, 24 * PT, 2 * PT);
        assert_eq!(b.best, Some(3));
        // §974: the height up to the break plus the depth before it
        assert_eq!(b.best_height_plus_depth, 22 * PT);
        assert!(!b.infinite_shrink_error);
    }

    #[test]
    fn prune_page_top_drops_discardables_and_inserts_split_top_skip() {
        let l = vec![
            Node::Penalty(0),
            Node::glue(Glue::fixed(PT)),
            line(3 * PT, PT),
        ];
        let (out, discards) = prune_page_top(l, Glue::fixed(10 * PT), true);
        assert_eq!(discards.len(), 2);
        assert_eq!(out.len(), 2);
        let Node::Glue(g) = &out[0] else { panic!() };
        assert_eq!(g.spec.width, 7 * PT);
    }

    #[test]
    fn the_page_fires_when_full() {
        let params = PageParams {
            vsize: 25 * PT,
            max_depth: 2 * PT,
            top_skip: Glue::fixed(10 * PT),
            holding_inserts: 0,
            saving_vdiscards: false,
        };
        let mut pb = PageBuilder::new();
        let mut class = |_| panic!("no inserts");
        let g = Node::glue(Glue::new(2 * PT, PT, 0));
        let mut items = Vec::new();
        for _ in 0..4 {
            items.push(line(8 * PT, 2 * PT));
            items.push(g.clone());
        }
        let mut fired = None;
        for (i, p) in items.iter().enumerate() {
            match pb.contribute(p.clone(), items.get(i + 1), &params, &mut class) {
                Outcome::Done => {}
                Outcome::Wait(_) => panic!(),
                Outcome::Fire(f) => {
                    fired = Some((i, f));
                    break;
                }
            }
        }
        let (i, f) = fired.expect("page fired");
        // topskip 10-8=2, line, glue, line, glue, line: 2+8+2+2+8+2+2+8 = 34
        // > 25, so the page breaks at the second glue (index 3), noticed
        // at the third glue (index 5)
        assert_eq!(i, 5);
        assert_eq!(f.output_penalty, INF_PENALTY);
        assert_eq!(f.box255.node.height, 25 * PT);
        // topskip glue and two lines, with the glue between them
        assert_eq!(f.box255.node.list.len(), 4);
        // the third line and the glue that fired are contributions again
        assert_eq!(f.contributions.len(), 3);
    }
}
