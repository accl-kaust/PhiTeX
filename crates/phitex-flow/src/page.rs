//! Vertical material and pages.
//!
//! A piece's material is what its nodes contributed to the main vertical
//! list, in order (`Para`: vertical items, a paragraph's lines, more
//! vertical items). The page builder is a fold over all of it in document
//! order: it appends each item to TeX's contribution list (interline glue
//! from the depth before, tex.web 679), and where a command called the page
//! builder (`Item::BuildPage`, with what TeX reads there) it moves the
//! contributions to the page exactly as tex.web part 45 does (§980–§1028,
//! without insertions, marks' classes or `\output`: the default output
//! ships `\box255`).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness};
use crate::item::{BoxNode, Item, PageCall, Para, ShipParams};
use crate::linebreak::Line;
use crate::pack::{Params, Spec, vpack};
use crate::val::Glue;

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
    pub fn feed(&mut self, item: Item, org: Org) {
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
                self.cur_call = Some(call.clone());
                self.build_page(&call);
                self.mark(from);
                self.last_call = Some(call);
                self.prev_call = Some(org);
                self.unsafe_since.clear();
            }
            Item::End(call) => {
                let from = self.pages_made;
                self.cur_call = Some(call.clone());
                self.its_all_over(&call);
                self.mark(from);
                self.ended = true;
            }
            Item::Halt => self.ended = true,
            Item::Shipout(s) => {
                self.unsafe_since.push(org);
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
                self.contrib.push_back(item);
                self.contrib_org.push_back(self.cur);
            }
        }
        self.prev_depth = Some(depth);
    }

    /// The end of the document (`end`: its origin): if it did not end with
    /// `\end`, as if it had (with the last page-builder call's parameters).
    pub fn finish(&mut self, end: Org) {
        let from = self.pages_made;
        if !self.ended
            && let Some(call) = self.last_call.clone()
        {
            self.cur = (end, self.prev_depth.unwrap_or(IGNORE_DEPTH));
            self.cur_call = Some(call.clone());
            self.pending = None;
            self.its_all_over(&call);
        }
        self.ended = true;
        self.mark(from);
    }

    fn push_contrib(&mut self, it: Item) {
        self.contrib.push_back(it);
        self.contrib_org.push_back(self.cur);
    }

    /// tex.web 1054: `its_all_over`, until the page and the contributions
    /// are empty.
    fn its_all_over(&mut self, call: &PageCall) {
        for _ in 0..1000 {
            if self.page.is_empty() && self.contrib.is_empty() {
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
            self.build_page(call);
        }
    }

    /// §987
    fn freeze_page_specs(&mut self, call: &PageCall) {
        self.page_contents = BOX_THERE;
        self.page_goal = call.vsize;
        self.page_max_depth = call.maxdepth;
        self.depth = 0;
        self.total = 0;
        self.stretch = [0; 4];
        self.shrink = 0;
        self.least_cost = AWFUL_BAD;
    }

    /// §994: `build_page`.
    fn build_page(&mut self, call: &PageCall) {
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
                            self.freeze_page_specs(call);
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
                let c = if b < AWFUL_BAD {
                    if pi <= -10000 {
                        pi
                    } else if b < INF_BAD {
                        b + pi
                    } else {
                        DEPLORABLE
                    }
                } else {
                    b
                };
                if c <= self.least_cost {
                    self.best = self.page.len();
                    self.best_size = self.page_goal;
                    self.least_cost = c;
                }
                if c == AWFUL_BAD || pi <= -10000 {
                    self.fire_up(call);
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

    /// §1012, §1017: the page up to the best break becomes `\box255`, what
    /// follows goes back to the contributions, and the default output
    /// ships the box (§1023).
    fn fire_up(&mut self, call: &PageCall) {
        let at = self.best.min(self.page.len());
        let mut rest: Vec<Item> = self.page.drain(at..).collect();
        let rest_org: Vec<(Org, i32)> = self.page_org.drain(at..).collect();
        // (§1013: the break's penalty becomes 10000)
        if let Some(Item::Penalty(_)) = rest.first() {
            rest[0] = Item::Penalty(10000);
        } else if rest.is_empty()
            && let Some(Item::Penalty(_)) = self.contrib.front()
        {
            self.contrib[0] = Item::Penalty(10000);
        }
        for (it, o) in rest.into_iter().zip(rest_org).rev() {
            self.contrib.push_front(it);
            self.contrib_org.push_front(o);
        }
        let items = std::mem::take(&mut self.page);
        self.page_org.clear();
        // (where the fold can start again: the first contribution left, if
        // everything left was fed since the last call and nothing fed
        // since it depends on what came before it)
        let k = self.pages_made;
        let resume = if let Some(&(s, d)) = self.contrib_org.front() {
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
        self.orgs.push(PageOrg {
            call: self.cur.0,
            call_idx: self.marks_base + self.marks.len() as u32,
            resume,
            last_call,
        });
        if let Some(r) = resume {
            self.just = Some((k, r));
        }
        let b = vpack(
            items,
            Spec::Exactly(self.best_size),
            self.page_max_depth,
            &Params::QUIET,
        )
        .b;
        // (§991: a new current page)
        self.page_contents = EMPTY;
        self.depth = 0;
        self.page_max_depth = 0;
        self.pages.push(Page {
            b: Arc::new(b),
            ship: call.ship.clone(),
        });
        self.pages_made += 1;
    }
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
