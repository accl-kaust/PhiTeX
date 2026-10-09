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
pub fn append_to_vlist(list: &mut Vec<Item>, prev_depth: &mut i32, b: &Arc<BoxNode>, inter: &crate::item::Interline) {
    if *prev_depth > IGNORE_DEPTH {
        let d = inter.baselineskip.width - *prev_depth - b.h;
        if d < inter.lineskiplimit {
            list.push(crate::item::param_glue(&inter.lineskip, 0));
        } else {
            let g = Glue { width: d, ..*inter.baselineskip };
            list.push(Item::Glue { g: Arc::new(g), sub: 2, zero: false });
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
/// last item if it is of that kind (not one a discretionary replaces).
pub fn remove_last(list: &mut Vec<Item>, k: u8) {
    let hit = match list.last() {
        Some(Item::Penalty(_)) => k == 0,
        Some(Item::Kern { .. }) => k == 1,
        Some(Item::Glue { .. }) => k == 2,
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
}

impl Scan {
    pub fn feed(&mut self, item: Item) {
        self.fed += 1;
        if self.ended {
            return;
        }
        let mut depth = self.prev_depth.unwrap_or(IGNORE_DEPTH);
        let pd = &mut depth;
        match item {
            Item::BuildPage(call) => {
                let from = self.pages_made;
                self.build_page(&call);
                self.marks.push((from, self.pages_made));
                self.last_call = Some(call);
            }
            Item::End(call) => {
                let from = self.pages_made;
                self.its_all_over(&call);
                self.marks.push((from, self.pages_made));
                self.ended = true;
            }
            Item::Shipout(s) => {
                self.pages.push(Page { b: s.b.clone(), ship: s.ship.clone() });
                self.pages_made += 1;
            }
            Item::Append(b, inter) => {
                let mut v = Vec::new();
                append_to_vlist(&mut v, pd, &b, &inter);
                self.contrib.extend(v);
            }
            Item::PrevDepth(d) => *pd = d,
            Item::Remove(k) => {
                let mut v: Vec<Item> = self.contrib.drain(..).collect();
                remove_last(&mut v, k);
                self.contrib.extend(v);
            }
            _ => self.contrib.push_back(item),
        }
        self.prev_depth = Some(depth);
    }

    /// The end of the document: if it did not end with `\end`, as if it
    /// had (with the last page-builder call's parameters).
    pub fn finish(&mut self) {
        let from = self.pages_made;
        if !self.ended
            && let Some(call) = self.last_call.clone()
        {
            self.its_all_over(&call);
        }
        self.ended = true;
        self.marks.push((from, self.pages_made));
    }

    /// tex.web 1054: `its_all_over`, until the page and the contributions
    /// are empty.
    fn its_all_over(&mut self, call: &PageCall) {
        for _ in 0..1000 {
            if self.page.is_empty() && self.contrib.is_empty() {
                return;
            }
            let b = BoxNode { w: call.hsize, ..BoxNode::default() };
            self.contrib.push_back(Item::Box(Arc::new(b)));
            let fill = Glue { stretch: 65536, stretch_order: 2, ..Glue::default() };
            self.contrib.push_back(Item::Glue { g: Arc::new(fill), sub: 0, zero: false });
            self.contrib.push_back(Item::Penalty(-0o10_000_000_000));
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
                        let w = if call.topskip.width > h { call.topskip.width - h } else { 0 };
                        let g = Glue { width: w, ..*call.topskip };
                        self.contrib.push_front(Item::Glue { g: Arc::new(g), sub: 10, zero: false });
                        continue;
                    }
                    // (§1002)
                    self.total += self.depth + h;
                    self.depth = d;
                    self.contribute();
                    continue;
                }
                Item::Glue { .. } | Item::Kern { .. } | Item::Penalty(_) => {
                    if self.page_contents < BOX_THERE {
                        self.contrib.pop_front();
                        continue;
                    }
                    match &p {
                        Item::Glue { .. } => {
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
                            } else if matches!(self.contrib.get(1), Some(Item::Glue { .. })) {
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
            if matches!(p, Item::Glue { .. } | Item::Kern { .. }) {
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
            Item::Glue { g, .. } => {
                self.stretch[usize::from(g.stretch_order.min(3))] += g.stretch;
                self.shrink += g.shrink;
                self.total += self.depth + g.width;
            }
            _ => {}
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
        }
    }

    /// §1012, §1017: the page up to the best break becomes `\box255`, what
    /// follows goes back to the contributions, and the default output
    /// ships the box (§1023).
    fn fire_up(&mut self, call: &PageCall) {
        let mut rest: Vec<Item> = self.page.drain(self.best.min(self.page.len())..).collect();
        // (§1013: the break's penalty becomes 10000)
        if let Some(Item::Penalty(_)) = rest.first() {
            rest[0] = Item::Penalty(10000);
        } else if rest.is_empty()
            && let Some(Item::Penalty(_)) = self.contrib.front()
        {
            self.contrib[0] = Item::Penalty(10000);
        }
        for it in rest.into_iter().rev() {
            self.contrib.push_front(it);
        }
        let items = std::mem::take(&mut self.page);
        let b = vpack(items, Spec::Exactly(self.best_size), self.page_max_depth, &Params::QUIET).b;
        // (§991: a new current page)
        self.page_contents = EMPTY;
        self.depth = 0;
        self.page_max_depth = 0;
        self.pages.push(Page { b: Arc::new(b), ship: call.ship.clone() });
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
