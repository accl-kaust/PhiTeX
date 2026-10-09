//! Vertical material and pages.
//!
//! A piece's material is its paragraphs' lines with the glue between them;
//! before its first line stands a **join**, resolved from the dimensions of
//! the line before it (in whichever piece that is) and of the line after:
//! no `\prevdepth` state. The page builder is a scan over the material in
//! order that reads dimensions only (tex.web part 45, without inserts or
//! output routines yet).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness, clamp};
use crate::graph::Para;
use crate::linebreak::{GlueSet, Line, set_glue};

/// The page parameters a paragraph was read with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PageParams {
    pub vsize: i32,
    pub topskip: i32,
    pub maxdepth: i32,
}

/// An item of the vertical material.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum VItem {
    Line(Arc<Line>, PageParams),
    Glue {
        w: i32,
        st: i32,
        st_ord: u8,
        sh: i32,
    },
    /// The interline glue before a piece's first line, resolved from its
    /// neighbours' dimensions.
    Join {
        baselineskip: i32,
    },
    Penalty(i32),
}

/// Interline glue (tex.web 679), `\lineskiplimit` 0pt, `\lineskip` 1pt.
pub fn interline(prev_d: i32, h: i32, bs: i32) -> VItem {
    let b = bs - prev_d - h;
    let w = if b < 0 { 65536 } else { b };
    VItem::Glue {
        w,
        st: 0,
        st_ord: 0,
        sh: 0,
    }
}

/// A paragraph's lines.
pub type Lines = Arc<Vec<Arc<Line>>>;

/// A piece's vertical material, from its paragraphs and their lines.
pub fn material(paras: &[(Arc<Para>, Lines)]) -> Vec<VItem> {
    let mut out = Vec::new();
    let mut prev: Option<i32> = None;
    for (p, lines) in paras {
        let pp = PageParams {
            vsize: p.vsize,
            topskip: p.topskip,
            maxdepth: p.maxdepth,
        };
        out.push(VItem::Glue {
            w: p.parskip,
            st: 0,
            st_ord: 0,
            sh: 0,
        });
        for l in lines.iter() {
            match prev {
                None => out.push(VItem::Join {
                    baselineskip: p.baselineskip,
                }),
                Some(d) => out.push(interline(d, l.h, p.baselineskip)),
            }
            out.push(VItem::Line(l.clone(), pp));
            prev = Some(l.d);
        }
    }
    out
}

/// A page: its items packed to `vsize`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub items: Vec<VItem>,
    pub h: i32,
    pub d: i32,
    pub glue: GlueSet,
}

/// Where an item of the material came from: the piece (by position) and
/// the item's index in that piece's material.
pub type Org = (u32, u32);

/// Where a page came from: the origin of its first line (everything before
/// it after the break before is discarded, so a scan started there with
/// nothing reproduces the page), and the origin of the item that made the
/// page go (what its break was decided on).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageOrg {
    pub start: Org,
    pub decided: Org,
}

/// The page builder's scan state.
#[derive(Default)]
pub struct Scan {
    page: Vec<VItem>,
    /// The origins of `page`'s items.
    page_org: Vec<Org>,
    /// The origin of the current page's first line.
    start: Org,
    /// Set when a page starts: the origin of its first line.
    pub started: Option<Org>,
    contents: bool,
    goal: i32,
    maxdepth: i32,
    total: i64,
    st: [i64; 4],
    sh: i64,
    depth: i32,
    least: i32,
    best: Option<usize>,
    prev_depth: Option<i32>,
    pending_join: Option<i32>,
    /// Pages made and not yet taken, with where they came from.
    pub pages: Vec<Page>,
    pub orgs: Vec<PageOrg>,
    /// Items fed (the scan's work).
    pub fed: u64,
}

impl Scan {
    pub fn feed(&mut self, item: VItem, org: Org) {
        let mut q = VecDeque::from([(item, org)]);
        while let Some((it, o)) = q.pop_front() {
            self.fed += 1;
            self.step(it, o, &mut q);
        }
    }

    /// The end of the document (piece position `end`): fill and force the
    /// last page out.
    pub fn finish(&mut self, end: u32) {
        self.feed(
            VItem::Glue {
                w: 0,
                st: 65536,
                st_ord: 2,
                sh: 0,
            },
            (end, 0),
        );
        self.feed(VItem::Penalty(-(1 << 30)), (end, 1));
    }

    fn step(&mut self, it: VItem, org: Org, q: &mut VecDeque<(VItem, Org)>) {
        match it {
            VItem::Join { baselineskip } => self.pending_join = Some(baselineskip),
            VItem::Line(l, pp) => {
                if let Some(bs) = self.pending_join.take()
                    && let Some(pd) = self.prev_depth
                {
                    let g = interline(pd, l.h, bs);
                    q.push_front((VItem::Line(l, pp), org));
                    q.push_front((g, org));
                    return;
                }
                self.prev_depth = Some(l.d);
                if !self.contents {
                    self.contents = true;
                    self.start = org;
                    self.started = Some(org);
                    self.goal = pp.vsize;
                    self.maxdepth = pp.maxdepth;
                    self.least = AWFUL_BAD;
                    self.best = None;
                    let t = (pp.topskip - l.h).max(0);
                    self.add_glue(t, 0, 0, 0);
                    self.page.push(VItem::Glue {
                        w: t,
                        st: 0,
                        st_ord: 0,
                        sh: 0,
                    });
                    self.page_org.push(org);
                }
                self.total += i64::from(self.depth) + i64::from(l.h);
                self.depth = l.d;
                if self.depth > self.maxdepth {
                    self.total += i64::from(self.depth - self.maxdepth);
                    self.depth = self.maxdepth;
                }
                self.page.push(VItem::Line(l, pp));
                self.page_org.push(org);
            }
            VItem::Glue { w, st, st_ord, sh } => {
                if !self.contents {
                    return;
                }
                if matches!(self.page.last(), Some(VItem::Line(..)))
                    && let Some(best) = self.test(0)
                {
                    q.push_front((VItem::Glue { w, st, st_ord, sh }, org));
                    self.fire(best, org, q);
                    return;
                }
                self.add_glue(w, st, st_ord, sh);
                self.page.push(VItem::Glue { w, st, st_ord, sh });
                self.page_org.push(org);
            }
            VItem::Penalty(p) => {
                if !self.contents {
                    return;
                }
                if p < 10000
                    && let Some(best) = self.test(p)
                {
                    q.push_front((VItem::Penalty(p), org));
                    self.fire(best, org, q);
                    return;
                }
                self.page.push(VItem::Penalty(p));
                self.page_org.push(org);
            }
        }
    }

    fn add_glue(&mut self, w: i32, st: i32, st_ord: u8, sh: i32) {
        self.total += i64::from(self.depth) + i64::from(w);
        self.depth = 0;
        self.st[usize::from(st_ord)] += i64::from(st);
        self.sh += i64::from(sh);
    }

    /// A legal breakpoint here, with penalty `pi` (tex.web 1005): the best
    /// break so far, if the page must go now.
    fn test(&mut self, pi: i32) -> Option<usize> {
        let goal = i64::from(self.goal);
        let b = if self.total < goal {
            if self.st[1] + self.st[2] + self.st[3] != 0 {
                0
            } else {
                badness(clamp(goal - self.total), clamp(self.st[0]))
            }
        } else if self.total - goal > self.sh {
            AWFUL_BAD
        } else {
            badness(clamp(self.total - goal), clamp(self.sh))
        };
        let c = if b < AWFUL_BAD {
            if pi <= -10000 {
                pi
            } else if b < INF_BAD {
                b + pi
            } else {
                100_000
            }
        } else {
            b
        };
        if c <= self.least {
            self.best = Some(self.page.len());
            self.least = c;
        }
        if c == AWFUL_BAD || pi <= -10000 {
            Some(self.best.unwrap_or(self.page.len()))
        } else {
            None
        }
    }

    /// Ships the page up to `best`; what follows goes back to be read again.
    fn fire(&mut self, best: usize, decided: Org, q: &mut VecDeque<(VItem, Org)>) {
        let rest: Vec<VItem> = self.page.drain(best..).collect();
        let rest_org: Vec<Org> = self.page_org.drain(best..).collect();
        for it in rest.into_iter().zip(rest_org).rev() {
            q.push_front(it);
        }
        self.page_org.clear();
        let items = std::mem::take(&mut self.page);
        let (mut nat, mut st, mut sh, mut last_d) = (0i64, [0i64; 4], 0i64, 0i32);
        for it in &items {
            match it {
                VItem::Line(l, _) => {
                    nat += i64::from(last_d) + i64::from(l.h);
                    last_d = l.d;
                }
                VItem::Glue {
                    w,
                    st: s,
                    st_ord,
                    sh: k,
                } => {
                    nat += i64::from(last_d) + i64::from(*w);
                    last_d = 0;
                    st[usize::from(*st_ord)] += i64::from(*s);
                    sh += i64::from(*k);
                }
                _ => {}
            }
        }
        let mut d = last_d;
        if d > self.maxdepth {
            nat += i64::from(d - self.maxdepth);
            d = self.maxdepth;
        }
        let glue = set_glue(clamp(i64::from(self.goal) - nat), &st, &[sh, 0, 0, 0]);
        self.pages.push(Page {
            items,
            h: self.goal,
            d,
            glue,
        });
        self.orgs.push(PageOrg {
            start: self.start,
            decided,
        });
        self.contents = false;
        self.total = 0;
        self.depth = 0;
        self.st = [0; 4];
        self.sh = 0;
        self.best = None;
        self.least = AWFUL_BAD;
    }
}
