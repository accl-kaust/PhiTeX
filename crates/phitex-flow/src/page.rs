//! Vertical material and pages.
//!
//! A piece's material is what its nodes contributed to the main vertical
//! list, in order (`Para`: vertical items, a paragraph's lines, more
//! vertical items). The page builder is a fold over all of it in document
//! order: it appends each item to TeX's contribution list (interline glue
//! from the depth before, tex.web 679), and where a command called the page
//! builder (`Item::BuildPage`, with what TeX reads there) it moves the
//! contributions to the page exactly as tex.web part 45 does (§980–§1028:
//! insertions, marks, `\outputpenalty`, `\deadcycles`; where `\output` is
//! not empty the fold *fires*: it hands its definitions and the routine to
//! its environment ([`FoldEnv`]), which runs the routine as nodes at that
//! place in the document and gives back what it left; otherwise the
//! default output ships `\box255`).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness};
use crate::item::{BoxNode, Ins, Item, PageCall, Para, ShipParams};
use crate::linebreak::Line;
use crate::pack::{Params, Spec, vpack};
use crate::graph::output::{self, DEAD_CYCLES, Marks, OutputReads};
use crate::val::{DimenPar, Glue, IntPar, Key, Tok, TokPar, Val};

/// What the fold reads and makes happen where the page builder is called
/// (the build's: reads resolve at the calling node's place; a fire's
/// definitions enter the names index there).
pub trait FoldEnv {
    /// The value of `key` where the page builder is called now.
    fn read(&mut self, key: &Key) -> Val;
    /// A fire (tex.web 1012): the fold defines `defs` here, then — with
    /// `routine` — the output routine runs; returns what it left.
    fn fire(&mut self, defs: Vec<(Key, Val)>, routine: Option<Routine>) -> Fired;
}

/// An output routine to run where the page builder fires (tex.web 1025):
/// what `output::output_entry` makes its entry state from.
pub struct Routine {
    /// `\output`'s tokens, braces included.
    pub toks: Arc<Vec<Tok>>,
    /// The page fired (`\box255`).
    pub page: Page,
    /// What the fold read and computed at the fire.
    pub reads: OutputReads,
}

/// What a fire gave back (tex.web 1026).
#[derive(Default)]
pub struct Fired {
    /// The routine's vertical list (its `\shipout`s in order among it).
    pub items: Vec<Item>,
    /// `\deadcycles` after the routine.
    pub dead_cycles: Option<i32>,
    /// What the routine wrote to the log.
    pub log: Vec<String>,
    pub errors: Vec<String>,
    /// The definitions reached a node already read: the fold stops after
    /// this call (the scan resumes there).
    pub halt: bool,
}

/// A fold that never fires a routine nor reads anything (tests, and
/// documents without `\output`): every read is the initial value.
pub struct NoEnv;

impl FoldEnv for NoEnv {
    fn read(&mut self, key: &Key) -> Val {
        crate::val::initial(key, &crate::tfm::null_font(), true, None)
    }
    fn fire(&mut self, _defs: Vec<(Key, Val)>, _routine: Option<Routine>) -> Fired {
        Fired::default()
    }
}

/// tex.web 1009: a page insertion record (`page_ins_head`'s list).
#[derive(Clone, Debug)]
struct PageIns {
    n: u8,
    split_up: bool,
    height: i32,
    /// Indices into the current page: the last insertion of the class on
    /// it, the last one on it at the best break so far, and the one being
    /// split there with where its list breaks (`None`: at its end).
    last_ins: Option<usize>,
    best_ins: Option<usize>,
    broken_ins: Option<usize>,
    broken_ptr: Option<usize>,
}

/// One log entry of a call that fired a routine: a page shipped by the
/// default output, or a line the routine wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallLog {
    Page(u32),
    Text(String),
}

const MAX_DIMEN: i32 = 0o7_777_777_777;
const EJECT_PENALTY: i32 = -10000;
const INSERTS_ONLY: u8 = 1;

fn int(v: &Val) -> i32 {
    match v {
        Val::Int(i) => *i,
        Val::Glue(g) => g.width,
        _ => 0,
    }
}

/// tex.web 106: `x_over_n` (truncating).
fn x_over_n(x: i32, n: i32) -> i32 {
    if n == 0 { 0 } else { x / n }
}

/// tex.web 970–976: `vert_break` — where to break the vertical list `list`
/// for height `h` with `d` the maximum depth: the breakpoint (`None`: its
/// end) and `best_height_plus_depth`.
pub fn vert_break(list: &[Item], h: i32, d: i32) -> (Option<usize>, i32) {
    let mut least_cost = AWFUL_BAD;
    let (mut cur_height, mut prev_dp) = (0i32, 0i32);
    let mut stretch = [0i32; 4];
    let mut shrink = 0i32;
    let mut best_place = None;
    let mut best_hd = 0;
    let mut i = 0;
    loop {
        let pi;
        let p = list.get(i);
        let mut update = false;
        match p {
            None => pi = EJECT_PENALTY,
            Some(Item::Box(_) | Item::Rule { .. }) => {
                let (bh, bd) = match p {
                    Some(Item::Box(b)) => (b.h, b.d),
                    Some(Item::Rule { h, d, .. }) => (*h, *d),
                    _ => unreachable!(),
                };
                cur_height += prev_dp + bh;
                prev_dp = bd;
                if prev_dp > d {
                    cur_height += prev_dp - d;
                    prev_dp = d;
                }
                i += 1;
                continue;
            }
            Some(Item::Glue { .. }) => {
                if i > 0 && list[i - 1].precedes_break() {
                    pi = 0;
                } else {
                    pi = 10000;
                    update = true;
                }
            }
            Some(Item::Kern { .. }) => {
                if matches!(list.get(i + 1), Some(Item::Glue { .. })) {
                    pi = 0;
                } else {
                    pi = 10000;
                    update = true;
                }
            }
            Some(Item::Penalty(x)) => pi = *x,
            Some(_) => {
                if prev_dp > d {
                    cur_height += prev_dp - d;
                    prev_dp = d;
                }
                i += 1;
                continue;
            }
        }
        if !update && pi < 10000 {
            // (§974–975)
            let b = if cur_height < h {
                if stretch[1] != 0 || stretch[2] != 0 || stretch[3] != 0 { 0 } else { badness(h - cur_height, stretch[0]) }
            } else if cur_height - h > shrink {
                AWFUL_BAD
            } else {
                badness(cur_height - h, shrink)
            };
            let b = if b < AWFUL_BAD {
                if pi <= EJECT_PENALTY {
                    pi
                } else if b < INF_BAD {
                    b + pi
                } else {
                    DEPLORABLE
                }
            } else {
                b
            };
            if b <= least_cost {
                best_place = if p.is_none() { None } else { Some(i) };
                least_cost = b;
                best_hd = cur_height + prev_dp;
            }
            if b == AWFUL_BAD || pi <= EJECT_PENALTY {
                return (best_place, best_hd);
            }
        }
        // (§976: a glue or kern updates the heights)
        match p {
            Some(Item::Glue { g, .. }) => {
                stretch[usize::from(g.stretch_order.min(3))] += g.stretch;
                shrink += g.shrink;
                cur_height += prev_dp + g.width;
                prev_dp = 0;
            }
            Some(Item::Kern { w, .. }) => {
                cur_height += prev_dp + w;
                prev_dp = 0;
            }
            _ => {}
        }
        if prev_dp > d {
            cur_height += prev_dp - d;
            prev_dp = d;
        }
        i += 1;
    }
}

/// tex.web 968: `prune_page_top` — the glue, kerns and penalties before
/// the first box or rule go; `\splittopskip` (`top`) glue comes before it.
pub fn prune_page_top(list: Vec<Item>, top: &Glue) -> Vec<Item> {
    let mut out = Vec::with_capacity(list.len() + 1);
    let mut it = list.into_iter();
    for x in it.by_ref() {
        match &x {
            Item::Box(_) | Item::Rule { .. } => {
                let h = match &x {
                    Item::Box(b) => b.h,
                    Item::Rule { h, .. } => *h,
                    _ => 0,
                };
                let w = if top.width > h { top.width - h } else { 0 };
                out.push(Item::Glue { g: Arc::new(Glue { width: w, ..*top }), sub: 11, zero: false });
                out.push(x);
                break;
            }
            Item::Glue { .. } | Item::Kern { .. } | Item::Penalty(_) => {}
            _ => out.push(x),
        }
    }
    out.extend(it);
    out
}

/// `\prevdepth` meaning "no interline glue" (tex.web 212).
pub const IGNORE_DEPTH: i32 = -65_536_000;

/// A paragraph's lines.
pub type Lines = Arc<Vec<Arc<Line>>>;

/// tex.web 679: `append_to_vlist`, onto `list`.
pub fn append_to_vlist(
    list: &mut Vec<Item>,
    prev_depth: &mut i32,
    b: &Arc<BoxNode>,
    inter: &crate::item::Interline,
) {
    if *prev_depth > IGNORE_DEPTH {
        let d = inter.baselineskip.width - *prev_depth - b.h;
        if d < inter.lineskiplimit {
            list.push(crate::item::param_glue(&inter.lineskip, 0));
        } else {
            let g = Glue {
                width: d,
                ..*inter.baselineskip
            };
            list.push(Item::Glue {
                g: Arc::new(g),
                sub: 2,
                zero: false,
            });
        }
    }
    list.push(Item::Box(b.clone()));
    *prev_depth = b.d;
}

/// What a list's gatherer does with an item emitted into a vertical list:
/// appends it, or applies it (interline glue, `\prevdepth`, a removal).
pub fn push_vertical(list: &mut Vec<Item>, prev_depth: &mut i32, it: Item) {
    match it {
        Item::Append(b, inter) => append_to_vlist(list, prev_depth, &b, &inter),
        Item::PrevDepth(d) => *prev_depth = d,
        Item::Remove(k) => remove_last(list, k),
        // (the page builder's: not in a list)
        Item::PageSoFar(..) => {}
        _ => list.push(it),
    }
}

/// tex.web 1105: `\unpenalty` (0), `\unkern` (1), `\unskip` (2) remove the
/// last item if it is of that kind (not one a discretionary replaces);
/// `\lastbox` (3) a box (tex.web 1081).
pub fn remove_last(list: &mut Vec<Item>, k: u8) {
    let hit = match list.last() {
        Some(Item::Penalty(_)) => k == 0,
        Some(Item::Kern { .. }) => k == 1,
        Some(Item::Glue { .. } | Item::Leaders(_)) => k == 2,
        Some(Item::Box(_)) => k == 3,
        _ => false,
    };
    if !hit {
        return;
    }
    // (tex.web 1105: an item a discretionary replaces stays)
    let n = list.len();
    for (j, it) in list.iter().enumerate() {
        if let Item::Disc(d) = it
            && j + usize::from(d.replace) >= n - 1
            && j < n - 1
        {
            return;
        }
    }
    list.pop();
}

/// A piece's contribution to the main vertical list, from its nodes'
/// `Para`s and their paragraphs' lines.
pub fn material(paras: &[(Arc<Para>, Lines)]) -> Vec<Item> {
    let mut out = Vec::new();
    for (p, lines) in paras {
        out.extend(p.before.iter().cloned());
        if let Some(lb) = &p.lb {
            for l in lines.iter() {
                out.push(Item::Append(l.b.clone(), lb.inter.clone()));
                out.extend(l.adjust.iter().cloned());
                if let Some(x) = l.pen {
                    out.push(Item::Penalty(x));
                }
            }
        }
        out.extend(p.after.iter().cloned());
    }
    out
}

/// A page: the box shipped, and what shipping it reads.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Page {
    pub b: Arc<BoxNode>,
    pub ship: Arc<ShipParams>,
}

/// Where an item of the material came from: the piece (by position) and
/// the item's index in that piece's material.
pub type Org = (u32, u32);

/// Where a page came from in the material, for resuming the fold at it on
/// the next build (memoisation by equality of inputs).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PageOrg {
    /// The page-builder call (or `\shipout`) that made it, and the call's
    /// index in `Scan::marks`.
    pub call: Org,
    pub call_idx: u32,
    /// Where the fold can start again, with nothing, to make the pages
    /// after it: the origin of the first contribution left when it was
    /// made, and `\prevdepth` just before that item was fed. `None` when
    /// the contributions also hold items fed before the previous call, or
    /// a removal or `\shipout` was fed between that origin and the call
    /// (tex.web 1017: after `fire_up` the current page is empty; the
    /// contributions are then what was fed since the origin, glue, kerns
    /// and penalties before the next box are discarded, and a fold started
    /// there sees the same items with the same depth before them).
    pub resume: Option<(Org, i32)>,
    /// The last page-builder call before the restart point (what `finish`
    /// would use).
    pub last_call: Option<Arc<PageCall>>,
}

const EMPTY: u8 = 0;
const BOX_THERE: u8 = 2;
const DEPLORABLE: i32 = 100_000;

/// The page builder's fold.
#[derive(Default)]
pub struct Scan {
    /// The contribution list.
    contrib: VecDeque<Item>,
    /// The main vertical list's `\prevdepth`.
    prev_depth: Option<i32>,
    /// The current page.
    page: Vec<Item>,
    page_contents: u8,
    page_goal: i32,
    page_max_depth: i32,
    /// `page_so_far[1..6]`: total, stretch by order, shrink.
    total: i32,
    stretch: [i32; 4],
    shrink: i32,
    depth: i32,
    least_cost: i32,
    best: usize,
    best_size: i32,
    ended: bool,
    last_call: Option<Arc<PageCall>>,
    /// Pages made and not yet taken.
    pub pages: Vec<Page>,
    /// For each page-builder call (`BuildPage`, `End`) in order, and for
    /// the end of the document: the pages it shipped (indices into all the
    /// pages made, explicit `\shipout`s included).
    pub marks: Vec<(u32, u32)>,
    pages_made: u32,
    /// Items fed (the scan's work).
    pub fed: u64,
    /// The origin of each item of `contrib` and `page`, with the depth
    /// before it was fed.
    contrib_org: VecDeque<(Org, i32)>,
    page_org: Vec<(Org, i32)>,
    /// The origin of the item being fed, of the last call, and of the
    /// removals and `\shipout`s fed since.
    cur: (Org, i32),
    prev_call: Option<Org>,
    unsafe_since: Vec<Org>,
    /// The call being made.
    cur_call: Option<Arc<PageCall>>,
    /// A page (its number) made when nothing was left to contribute: it
    /// resumes at the next item fed.
    pending: Option<u32>,
    /// For each page made and not yet taken, where it came from; the
    /// number of the first of them, and of the first mark.
    orgs: Vec<PageOrg>,
    orgs_base: u32,
    marks_base: u32,
    /// Restart points found for pages already taken (`(page, point)`).
    pub late: Vec<(u32, (Org, i32))>,
    /// The last restart point found, with its page's number.
    pub just: Option<(u32, (Org, i32))>,
    /// The first mark pushed takes this as its first page (a fold resumed
    /// inside a call: the call's pages before the resumption are kept).
    first_from: Option<u32>,
    /// Each mark class's `\topmark`, `\firstmark`, `\botmark` (tex.web
    /// 382, etex.web's classes), by class.
    cur_marks: Vec<(u16, Marks)>,
    /// Whether a mark was ever met (its values are then defined at each
    /// fire).
    marked: bool,
    dead_cycles: i32,
    insert_penalties: i32,
    /// The page insertion records, by class.
    ins: Vec<PageIns>,
    /// Errors the page builder gave (`\box255 is not void`, …).
    pub errors: Vec<String>,
    /// For each call (by its mark's index) that fired a routine: its log.
    pub call_logs: Vec<(u32, Vec<CallLog>)>,
    /// A fire's definitions reached a node already read: stop feeding.
    pub halt: bool,
    /// A routine fired, an insertion or a mark came: the fold's restart
    /// points are not recorded (the fold is made again from its start).
    pub fancy: bool,
    /// The first page the current call made.
    call_start: u32,
}

impl Scan {
    /// A fold resumed at a page's restart point (`PageOrg::resume`): `pages`
    /// pages made before it, `\prevdepth` `prev_depth`, and the call in
    /// progress there having made its pages from `call_from` on.
    /// `marks` marks before it, and the last call `last_call`.
    pub fn resumed(
        pages: u32,
        prev_depth: i32,
        call_from: Option<u32>,
        marks: u32,
        last_call: Option<Arc<PageCall>>,
    ) -> Scan {
        Scan {
            prev_depth: Some(prev_depth),
            pages_made: pages,
            orgs_base: pages,
            marks_base: marks,
            first_from: call_from,
            last_call,
            ..Scan::default()
        }
    }

    /// The pages made since the last take, with where they came from.
    pub fn take_pages(&mut self) -> (Vec<Page>, Vec<PageOrg>) {
        self.orgs_base += self.orgs.len() as u32;
        (
            std::mem::take(&mut self.pages),
            std::mem::take(&mut self.orgs),
        )
    }

    /// Page `k`'s restart point is `r`.
    fn set_resume(&mut self, k: u32, r: (Org, i32)) {
        match k.checked_sub(self.orgs_base) {
            Some(i) => self.orgs[i as usize].resume = Some(r),
            None => self.late.push((k, r)),
        }
        self.just = Some((k, r));
    }

    fn mark(&mut self, from: u32) {
        let from = self.first_from.take().unwrap_or(from);
        self.marks.push((from, self.pages_made));
    }

    /// Feeds `item`, from `org` in the material.
    pub fn feed(&mut self, item: Item, org: Org, env: &mut dyn FoldEnv) {
        self.fed += 1;
        if self.ended {
            return;
        }
        let mut depth = self.prev_depth.unwrap_or(IGNORE_DEPTH);
        self.cur = (org, depth);
        if let Some(k) = self.pending.take() {
            let r = self.cur;
            self.set_resume(k, r);
        }
        let pd = &mut depth;
        match item {
            Item::BuildPage(call) => {
                let from = self.pages_made;
                self.call_start = from;
                self.cur_call = Some(call.clone());
                self.build_page(&call, env);
                self.mark(from);
                self.last_call = Some(call);
                self.prev_call = Some(org);
                self.unsafe_since.clear();
            }
            Item::End(call) => {
                let from = self.pages_made;
                self.call_start = from;
                self.cur_call = Some(call.clone());
                self.its_all_over(&call, env);
                self.mark(from);
                self.ended = true;
            }
            Item::Halt => self.ended = true,
            Item::Shipout(s) => {
                self.unsafe_since.push(org);
                self.dead_cycles = 0;
                self.pages.push(Page {
                    b: s.b.clone(),
                    ship: s.ship.clone(),
                });
                self.orgs.push(PageOrg {
                    call: org,
                    call_idx: u32::MAX,
                    resume: None,
                    last_call: None,
                });
                self.pages_made += 1;
            }
            Item::Append(b, inter) => {
                let mut v = Vec::new();
                append_to_vlist(&mut v, pd, &b, &inter);
                let c = self.cur;
                self.contrib_org.extend(v.iter().map(|_| c));
                self.contrib.extend(v);
            }
            Item::PrevDepth(d) => *pd = d,
            // (tex.web 1245–1246: the page so far changes now, before the
            // contributions after it are looked at)
            Item::PageSoFar(k, v) => match k {
                0 => self.page_goal = v,
                1 => self.total = v,
                2..=5 => self.stretch[usize::from(k - 2)] = v,
                6 => self.shrink = v,
                7 => self.depth = v,
                8 => self.dead_cycles = v,
                _ => self.insert_penalties = v,
            },
            Item::Remove(k) => {
                self.unsafe_since.push(org);
                let mut v: Vec<Item> = self.contrib.drain(..).collect();
                let n = v.len();
                remove_last(&mut v, k);
                if v.len() < n {
                    self.contrib_org.pop_back();
                }
                self.contrib.extend(v);
            }
            _ => {
                if matches!(item, Item::Mark(_) | Item::Ins(_)) {
                    self.fancy = true;
                }
                self.contrib.push_back(item);
                self.contrib_org.push_back(self.cur);
            }
        }
        self.prev_depth = Some(depth);
    }

    /// The end of the document (`end`: its origin): if it did not end with
    /// `\end`, as if it had (with the last page-builder call's parameters).
    pub fn finish(&mut self, end: Org, env: &mut dyn FoldEnv) {
        let from = self.pages_made;
        self.call_start = from;
        if !self.ended
            && let Some(call) = self.last_call.clone()
        {
            self.cur = (end, self.prev_depth.unwrap_or(IGNORE_DEPTH));
            self.cur_call = Some(call.clone());
            self.pending = None;
            self.its_all_over(&call, env);
        }
        self.ended = true;
        self.mark(from);
    }

    fn push_contrib(&mut self, it: Item) {
        self.contrib.push_back(it);
        self.contrib_org.push_back(self.cur);
    }

    /// tex.web 1054: `its_all_over`, until the page and the contributions
    /// are empty and no output routine is in a dead cycle.
    fn its_all_over(&mut self, call: &PageCall, env: &mut dyn FoldEnv) {
        for _ in 0..1000 {
            if self.page.is_empty() && self.contrib.is_empty() && self.dead_cycles == 0 {
                return;
            }
            let b = BoxNode {
                w: call.hsize,
                ..BoxNode::default()
            };
            self.push_contrib(Item::Box(Arc::new(b)));
            let fill = Glue {
                stretch: 65536,
                stretch_order: 2,
                ..Glue::default()
            };
            self.push_contrib(Item::Glue {
                g: Arc::new(fill),
                sub: 0,
                zero: false,
            });
            self.push_contrib(Item::Penalty(-0o10_000_000_000));
            self.build_page(call, env);
        }
    }

    /// §987
    fn freeze_page_specs(&mut self, call: &PageCall, s: u8) {
        self.page_contents = s;
        self.page_goal = call.vsize;
        self.page_max_depth = call.maxdepth;
        self.depth = 0;
        self.total = 0;
        self.stretch = [0; 4];
        self.shrink = 0;
        self.least_cost = AWFUL_BAD;
    }

    /// §994: `build_page`.
    fn build_page(&mut self, call: &PageCall, env: &mut dyn FoldEnv) {
        while let Some(p) = self.contrib.front().cloned() {
            // (§1000)
            let pi;
            match &p {
                Item::Box(_) | Item::Rule { .. } => {
                    let (h, d) = match &p {
                        Item::Box(b) => (b.h, b.d),
                        Item::Rule { h, d, .. } => (*h, *d),
                        _ => unreachable!(),
                    };
                    if self.page_contents < BOX_THERE {
                        // (§1001: the page begins: `\topskip` before it)
                        if self.page_contents == EMPTY {
                            self.freeze_page_specs(call, BOX_THERE);
                        } else {
                            self.page_contents = BOX_THERE;
                        }
                        let w = if call.topskip.width > h {
                            call.topskip.width - h
                        } else {
                            0
                        };
                        let g = Glue {
                            width: w,
                            ..*call.topskip
                        };
                        self.contrib.push_front(Item::Glue {
                            g: Arc::new(g),
                            sub: 10,
                            zero: false,
                        });
                        let o = self.contrib_org.front().copied().unwrap_or(self.cur);
                        self.contrib_org.push_front(o);
                        continue;
                    }
                    // (§1002)
                    self.total += self.depth + h;
                    self.depth = d;
                    self.contribute();
                    continue;
                }
                Item::Glue { .. } | Item::Leaders(_) | Item::Kern { .. } | Item::Penalty(_) => {
                    if self.page_contents < BOX_THERE {
                        self.contrib.pop_front();
                        self.contrib_org.pop_front();
                        continue;
                    }
                    match &p {
                        Item::Glue { .. } | Item::Leaders(_) => {
                            if self.page.last().is_some_and(Item::precedes_break) {
                                pi = 0;
                            } else {
                                self.update_heights(&p);
                                continue;
                            }
                        }
                        Item::Kern { .. } => {
                            if self.contrib.len() == 1 {
                                return;
                            } else if self.contrib.get(1).is_some_and(Item::is_glue) {
                                pi = 0;
                            } else {
                                self.update_heights(&p);
                                continue;
                            }
                        }
                        Item::Penalty(x) => pi = *x,
                        _ => unreachable!(),
                    }
                }
                Item::Ins(x) => {
                    self.page_insert(call, x, env);
                    self.contribute();
                    continue;
                }
                _ => {
                    // (marks and anything else: contributed)
                    self.contribute();
                    continue;
                }
            }
            // (§1005: a new champion breakpoint? time for a page break?)
            if pi < 10000 {
                let b = if self.total < self.page_goal {
                    if self.stretch[1] != 0 || self.stretch[2] != 0 || self.stretch[3] != 0 {
                        0
                    } else {
                        badness(self.page_goal - self.total, self.stretch[0])
                    }
                } else if self.total - self.page_goal > self.shrink {
                    AWFUL_BAD
                } else {
                    badness(self.total - self.page_goal, self.shrink)
                };
                let mut c = if b < AWFUL_BAD {
                    if pi <= -10000 {
                        pi
                    } else if b < INF_BAD {
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
                    self.best = self.page.len();
                    self.best_size = self.page_goal;
                    self.least_cost = c;
                    for r in &mut self.ins {
                        r.best_ins = r.last_ins;
                    }
                }
                if c == AWFUL_BAD || pi <= -10000 {
                    self.fire_up(env);
                    continue;
                }
            }
            if matches!(p, Item::Glue { .. } | Item::Leaders(_) | Item::Kern { .. }) {
                self.update_heights(&p);
            } else {
                self.contribute();
            }
        }
    }

    /// §1004, then §1003 and the link (`contribute`).
    fn update_heights(&mut self, p: &Item) {
        match p {
            Item::Kern { w, .. } => {
                self.total += self.depth + w;
            }
            _ => {
                if let Some(g) = p.glue() {
                    self.stretch[usize::from(g.stretch_order.min(3))] += g.stretch;
                    self.shrink += g.shrink;
                    self.total += self.depth + g.width;
                }
            }
        }
        self.depth = 0;
        self.contribute();
    }

    /// §1003, §998: the node at the contributions' head goes to the page.
    fn contribute(&mut self) {
        if self.depth > self.page_max_depth {
            self.total = self.total + self.depth - self.page_max_depth;
            self.depth = self.page_max_depth;
        }
        if let Some(p) = self.contrib.pop_front() {
            self.page.push(p);
            self.page_org
                .push(self.contrib_org.pop_front().unwrap_or(self.cur));
        }
    }

    /// §1008–§1010: an insertion arrives on the page (its class's
    /// registers read where the page builder is called).
    fn page_insert(&mut self, call: &PageCall, x: &Arc<Ins>, env: &mut dyn FoldEnv) {
        self.fancy = true;
        if self.page_contents == EMPTY {
            self.freeze_page_specs(call, INSERTS_ONLY);
        }
        let n = x.n;
        let k = u16::from(n);
        let ri = if let Some(i) = self.ins.iter().position(|r| r.n == n) {
            i
        } else {
            {
                // (§1009: a new record; the box's contents and `\skip n` count)
                let height = match env.read(&Key::Box(k)) {
                    Val::Box(b) => {
                        if !b.vertical {
                            self.errors.push(format!("Insertions can only be added to a vbox (\\box{n})"));
                        }
                        b.h + b.d
                    }
                    _ => 0,
                };
                let count = int(&env.read(&Key::Count(k)));
                let h = if count == 1000 { height } else { x_over_n(height, 1000) * count };
                let q = match env.read(&Key::Skip(k)) {
                    Val::Glue(g) => g,
                    _ => Arc::new(Glue::default()),
                };
                self.page_goal -= h + q.width;
                self.stretch[usize::from(q.stretch_order.min(3))] += q.stretch;
                self.shrink += q.shrink;
                if q.shrink_order != 0 && q.shrink != 0 {
                    self.errors.push(format!("Infinite glue shrinkage inserted from \\skip{n}"));
                }
                let at = self.ins.partition_point(|r| r.n < n);
                self.ins.insert(
                    at,
                    PageIns {
                        n,
                        split_up: false,
                        height,
                        last_ins: None,
                        best_ins: None,
                        broken_ins: None,
                        broken_ptr: None,
                    },
                );
                at
            }
        };
        if self.ins[ri].split_up {
            self.insert_penalties += x.float_cost;
            return;
        }
        let here = self.page.len();
        self.ins[ri].last_ins = Some(here);
        let delta = self.page_goal - self.total - self.depth + self.shrink;
        let count = int(&env.read(&Key::Count(k)));
        let h = if count == 1000 { x.height } else { x_over_n(x.height, 1000) * count };
        let dimen = int(&env.read(&Key::Dimen(k)));
        if (h <= 0 || h <= delta) && x.height + self.ins[ri].height <= dimen {
            self.page_goal -= h;
            self.ins[ri].height += x.height;
        } else {
            // (§1010: the best way to split it)
            let mut w = if count <= 0 {
                MAX_DIMEN
            } else {
                let mut w = self.page_goal - self.total - self.depth;
                if count != 1000 {
                    w = x_over_n(w, count) * 1000;
                }
                w
            };
            if w > dimen - self.ins[ri].height {
                w = dimen - self.ins[ri].height;
            }
            let (q, mut best) = vert_break(&x.list, w, x.depth);
            self.ins[ri].height += best;
            if count != 1000 {
                best = x_over_n(best, 1000) * count;
            }
            self.page_goal -= best;
            let r = &mut self.ins[ri];
            r.split_up = true;
            r.broken_ptr = q;
            r.broken_ins = Some(here);
            match q {
                None => self.insert_penalties += EJECT_PENALTY,
                Some(i) => {
                    if let Item::Penalty(p) = x.list[i] {
                        self.insert_penalties += p;
                    }
                }
            }
        }
    }

    /// §1012–§1023: the page up to the best break becomes `\box255`, its
    /// insertions go to their boxes (or are held over), its marks set
    /// `\firstmark` and `\botmark`, what follows goes back to the
    /// contributions; then the output routine fires (`env`), or the default
    /// output ships the box.
    fn fire_up(&mut self, env: &mut dyn FoldEnv) {
        // (§1012–1013: the marks, the break's penalty, the page so far as
        // the routine reads it, §991 having made its depth 0)
        let page_so_far = [
            self.page_goal,
            self.total,
            self.stretch[0],
            self.stretch[1],
            self.stretch[2],
            self.stretch[3],
            self.shrink,
            0,
        ];
        let at = self.best.min(self.page.len());
        let mut rest: Vec<Item> = self.page.drain(at..).collect();
        let rest_org: Vec<(Org, i32)> = self.page_org.drain(at..).collect();
        // (§1013: the break's penalty is `\outputpenalty`, and 10000 now)
        let mut out_pen = 10000;
        if let Some(Item::Penalty(x)) = rest.first() {
            out_pen = *x;
            rest[0] = Item::Penalty(10000);
        } else if rest.is_empty()
            && let Some(Item::Penalty(x)) = self.contrib.front()
        {
            out_pen = *x;
            self.contrib[0] = Item::Penalty(10000);
        }
        for (it, o) in rest.into_iter().zip(rest_org).rev() {
            self.contrib.push_front(it);
            self.contrib_org.push_front(o);
        }
        let routine = match env.read(&Key::TokPar(TokPar::Output)) {
            Val::Toks(t) if !t.is_empty() => Some(t),
            _ => None,
        };
        let fancy = self.fancy || routine.is_some();
        // (§1015: `\box255` must be void)
        if fancy && !matches!(env.read(&Key::Box(255)), Val::Void) {
            self.errors.push("\\box255 is not void".into());
        }
        self.insert_penalties = 0;
        let holding = !self.ins.is_empty() && int(&env.read(&Key::IntPar(IntPar::Holdinginserts))) > 0;
        // (§1018: the boxes of the classes with insertions on the page, as
        // queues)
        let mut boxes: Vec<(u8, Vec<Item>)> = Vec::new();
        if !holding {
            for r in &self.ins {
                if r.best_ins.is_some() {
                    let list = match env.read(&Key::Box(u16::from(r.n))) {
                        Val::Box(b) => b.list.clone(),
                        _ => Vec::new(),
                    };
                    boxes.push((r.n, list));
                }
            }
        }
        // (§1019–§1022: the page's items before the break)
        let items = std::mem::take(&mut self.page);
        let item_orgs = std::mem::take(&mut self.page_org);
        let mut kept = Vec::with_capacity(items.len());
        let mut held: Vec<(Item, (Org, i32))> = Vec::new();
        for (j, (it, o)) in items.into_iter().zip(item_orgs).enumerate() {
            match it {
                Item::Ins(x) if !holding => {
                    let Some(ri) = self.ins.iter().position(|r| r.n == x.n) else {
                        held.push((Item::Ins(x), o));
                        continue;
                    };
                    let mut wait = self.ins[ri].best_ins.is_none();
                    let mut keep = x.clone();
                    if !wait {
                        let bi = boxes.iter().position(|b| b.0 == x.n).expect("a queue box");
                        if self.ins[ri].best_ins == Some(j) {
                            // (§1021: the box is wrapped up, the insertion
                            // split if it was chosen to be)
                            let r = &self.ins[ri];
                            if r.split_up
                                && r.broken_ins == Some(j)
                                && let Some(q) = r.broken_ptr
                            {
                                boxes[bi].1.extend(x.list[..q].iter().cloned());
                                let remainder = prune_page_top(x.list[q..].to_vec(), &x.split_top);
                                if !remainder.is_empty() {
                                    let b = vpack(remainder, Spec::NATURAL, MAX_DIMEN, &Params::QUIET).b;
                                    keep = Arc::new(Ins { height: b.h + b.d, list: b.list, ..(*x).clone() });
                                    wait = true;
                                }
                            } else {
                                boxes[bi].1.extend(x.list.iter().cloned());
                            }
                            self.ins[ri].best_ins = None;
                        } else {
                            boxes[bi].1.extend(x.list.iter().cloned());
                        }
                    }
                    if wait {
                        held.push((Item::Ins(keep), o));
                        self.insert_penalties += 1;
                    }
                }
                other => kept.push(other),
            }
        }
        // (where the fold can start again: the first contribution left, if
        // everything left was fed since the last call and nothing fed
        // since it depends on what came before it)
        let k = self.pages_made;
        let resume = if fancy {
            None
        } else if let Some(&(s, d)) = self.contrib_org.front() {
            (self.prev_call.is_none_or(|c| s > c) && !self.unsafe_since.iter().any(|&u| u >= s))
                .then_some((s, d))
        } else {
            self.pending = Some(k);
            None
        };
        // (before the restart point the last call is the one before this
        // one, or this one if the point is after it)
        let last_call = if resume.is_some() {
            self.last_call.clone()
        } else {
            self.cur_call.clone()
        };
        let b = Arc::new(vpack(kept, Spec::Exactly(self.best_size), self.page_max_depth, &Params::QUIET).b);
        // (§1012–1014, each class)
        let had_marks = !self.cur_marks.is_empty();
        self.cur_marks = output::fire_marks(&self.cur_marks, &b);
        // (§991: a new current page)
        self.page_contents = EMPTY;
        self.depth = 0;
        self.page_max_depth = 0;
        self.ins.clear();
        // (the fold's definitions at the fire: `\outputpenalty` globally at
        // every fire, §1013; the insertion boxes, §1018–1022; the marks,
        // once there are any)
        let mut defs: Vec<(Key, Val)> = vec![(Key::IntPar(IntPar::Outputpenalty), Val::Int(out_pen))];
        // (the page is empty now: outside a routine `\pagegoal` reads
        // `max_dimen`, the others 0, tex.web 421)
        for k in 0..8u8 {
            defs.push((Key::Page(k), Val::Int(if k == 0 { MAX_DIMEN } else { 0 })));
        }
        for (n, list) in boxes {
            let b = vpack(list, Spec::NATURAL, MAX_DIMEN, &Params::QUIET).b;
            defs.push((Key::Box(u16::from(n)), Val::Box(Arc::new(b))));
        }
        if self.marked || had_marks || !self.cur_marks.is_empty() {
            self.marked = true;
            self.fancy = true;
            for (c, m) in &self.cur_marks {
                for (w, t) in m.iter().enumerate() {
                    defs.push((Key::Mark(w as u8, *c), Val::Toks(t.clone().unwrap_or_default())));
                }
            }
        }
        let call_idx = self.marks_base + self.marks.len() as u32;
        let dead_ok = routine.is_some() && {
            let max = int(&env.read(&Key::IntPar(IntPar::Maxdeadcycles)));
            if self.dead_cycles >= max {
                self.errors.push(format!("Output loop---{} consecutive dead cycles", self.dead_cycles));
                false
            } else {
                true
            }
        };
        if let (true, Some(toks)) = (dead_ok, routine) {
            // (§1025: the routine fires; its run defines `\box255`,
            // `\outputpenalty`, `\deadcycles` and the marks at its start)
            self.fancy = true;
            let reads = OutputReads {
                output_penalty: out_pen,
                insert_penalties: self.insert_penalties,
                dead_cycles: self.dead_cycles,
                marks: self.cur_marks.clone(),
                page_so_far,
            };
            self.dead_cycles += 1;
            let page = Page { b, ship: Arc::default() };
            let fired = env.fire(defs, Some(Routine { toks, page, reads }));
            self.errors.extend(fired.errors);
            self.halt |= fired.halt;
            if let Some(d) = fired.dead_cycles {
                self.dead_cycles = d;
            }
            // (§1026: its list, after the held-over insertions, goes before
            // the contributions; its `\shipout`s are pages)
            let made_before = self.pages_made;
            let mut log: Vec<CallLog> = fired.log.into_iter().map(CallLog::Text).collect();
            let mut list: Vec<(Item, (Org, i32))> = held;
            for it in fired.items {
                match it {
                    Item::Shipout(s) => {
                        self.unsafe_since.push(self.cur.0);
                        self.pages.push(Page { b: s.b.clone(), ship: s.ship.clone() });
                        self.orgs.push(PageOrg { call: self.cur.0, call_idx, resume: None, last_call: last_call.clone() });
                        self.pages_made += 1;
                    }
                    Item::BuildPage(_) | Item::End(_) => {}
                    other => list.push((other, self.cur)),
                }
            }
            for (it, o) in list.into_iter().rev() {
                self.contrib.push_front(it);
                self.contrib_org.push_front(o);
            }
            self.insert_penalties = 0;
            self.add_call_log(call_idx, &mut log, made_before);
            return;
        }
        // (§1023: the default output; `ship_out` makes `\deadcycles` 0)
        if self.fancy {
            defs.push((Key::Page(DEAD_CYCLES), Val::Int(0)));
        }
        let fired = env.fire(defs, None);
        self.errors.extend(fired.errors);
        self.halt |= fired.halt;
        for (it, o) in held.into_iter().rev() {
            self.contrib.push_front(it);
            self.contrib_org.push_front(o);
        }
        self.orgs.push(PageOrg { call: self.cur.0, call_idx, resume, last_call });
        if let Some(r) = resume {
            self.just = Some((k, r));
        }
        if self.call_logs.last().is_some_and(|c| c.0 == call_idx) {
            let mut l = vec![CallLog::Page(k)];
            self.add_call_log(call_idx, &mut l, k);
        }
        self.dead_cycles = 0;
        let ship = ship_params(env);
        self.pages.push(Page { b, ship });
        self.pages_made += 1;
    }

    /// Appends to the log of call `call_idx` (made at its first routine,
    /// with the default output's pages of the call before it).
    fn add_call_log(&mut self, call_idx: u32, l: &mut Vec<CallLog>, upto: u32) {
        match self.call_logs.last_mut() {
            Some(c) if c.0 == call_idx => c.1.append(l),
            _ => {
                let mut v: Vec<CallLog> = (self.call_start..upto).map(CallLog::Page).collect();
                v.append(l);
                self.call_logs.push((call_idx, v));
            }
        }
    }
}

/// What shipping a page reads (tex.web 638–641, 617), where the page
/// builder fires: the default output's `ship_out` (not every call's read:
/// an output routine's `\advancepageno` changes `\count0` at every page).
fn ship_params(env: &mut dyn FoldEnv) -> Arc<ShipParams> {
    let mut counts = [0; 10];
    for (k, c) in counts.iter_mut().enumerate() {
        *c = int(&env.read(&Key::Count(k as u16)));
    }
    let mut r = |k: Key| int(&env.read(&k));
    Arc::new(ShipParams {
        counts,
        hoffset: r(Key::DimenPar(DimenPar::Hoffset)),
        voffset: r(Key::DimenPar(DimenPar::Voffset)),
        mag: r(Key::IntPar(IntPar::Mag)),
        date: [r(Key::IntPar(IntPar::Year)), r(Key::IntPar(IntPar::Month)), r(Key::IntPar(IntPar::Day)), r(Key::IntPar(IntPar::Time))],
    })
}

/// The names the default output's shipping reads ([`ship_params`]).
pub fn ship_keys() -> Vec<Key> {
    let mut v: Vec<Key> = (0..10).map(Key::Count).collect();
    v.extend([
        Key::DimenPar(DimenPar::Hoffset),
        Key::DimenPar(DimenPar::Voffset),
        Key::IntPar(IntPar::Mag),
        Key::IntPar(IntPar::Year),
        Key::IntPar(IntPar::Month),
        Key::IntPar(IntPar::Day),
        Key::IntPar(IntPar::Time),
    ]);
    v
}

/// The log's `[n]` for a page shipped (tex.web 638): a raw entry.
pub fn page_mark(ship: &ShipParams) -> String {
    use crate::show::{RAW, SP};
    let mut s = String::new();
    s.push(RAW);
    s.push(SP);
    s.push('[');
    let mut j = 9;
    while ship.counts[j] == 0 && j > 0 {
        j -= 1;
    }
    for k in 0..=j {
        s.push_str(&ship.counts[k].to_string());
        if k < j {
            s.push('.');
        }
    }
    s.push(']');
    s
}
