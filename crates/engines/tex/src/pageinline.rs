//! The page builder of a text run as one sequential node (a job's text
//! in INITEX's regime, without a format): `build_page` (§994–§1017) at
//! the points tex.web calls it, and `fire_up` (§1012–§1025) inline on
//! the run's own nest, so pages are shipped (and the PDF writer's
//! objects numbered) in TeX's order among the text's own commands.
//! A document's pages are the `Breaks` node's (`page.rs`).

use std::collections::VecDeque;

use phitex_defuse::ValueId;
use phitex_typeset::glue::Glue;
use phitex_typeset::node::Node;
use phitex_typeset::page::{InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::{FoldState, fold_item};
use crate::nodes::{self, Side};
use crate::run::{R, Run};
use crate::val::{group, int_of, list_of};

/// The page builder's state in the run: the current page, its side
/// table, the contributions not yet moved to it (folded), and the main
/// list's fold.
pub(crate) struct Inline {
    pb: PageBuilder,
    side: Side,
    queue: VecDeque<ValueId>,
    fold: FoldState,
}

impl Inline {
    pub(crate) fn new() -> Self {
        Inline {
            pb: PageBuilder::new(),
            side: Side::default(),
            queue: VecDeque::new(),
            fold: FoldState {
                sf: 1000,
                pd: crate::build::IGNORE_DEPTH,
                since_bp: 0,
            },
        }
    }
}

/// Moves the run's contributions to the main list (`items` of list 0)
/// into the page builder's queue, folded; a previous depth transition
/// stays for the main list's readers (`\prevdepth`, the next box's
/// interline glue).
fn drain(r: &mut Run<'_, '_>, pg: &mut Inline) {
    let v = r.v;
    let f = v.field("items");
    let mut raw = Vec::new();
    r.contribs.retain(|&(g, s)| {
        if g != f {
            return true;
        }
        let xs = v.seq_to_vec(s);
        let main = xs.iter().all(|&x| {
            let ys = list_of(v, x);
            ys.len() == 2 && int_of(v, ys[0]) == 0
        });
        if main {
            for x in xs {
                raw.push(list_of(v, x)[1]);
            }
        }
        !main
    });
    if raw.is_empty() {
        return;
    }
    let mut out = Vec::new();
    for n in raw {
        fold_item(v, 0, n, &mut pg.fold, &mut out);
    }
    pg.fold.since_bp = 0;
    // (the `Breaks` fold's markers place nothing here: the page
    // parameters are read live, the marks and writes are in order)
    pg.queue.extend(out.into_iter().filter(|&x| {
        !matches!(
            nodes::tag(v, x),
            nodes::T_PAGE_PARAMS
                | nodes::T_MARKS_AT
                | nodes::T_WRITE_AT
                | nodes::T_FONT_AT
                | nodes::T_PDFW_AT
                | nodes::T_PDFQ_AT
                | nodes::T_DEFS_AT
                | nodes::T_LOCALS_AT
                | nodes::T_FLUSH_AT
        )
    }));
    let pd = v.list(vec![v.int(nodes::T_PD), v.int(i64::from(pg.fold.pd))]);
    let e = v.list(vec![v.int(0), pd]);
    r.contribs.push((f, v.seq_unit(e)));
}

/// Whether an output routine is active (§994 returns then).
fn output_active(r: &Run<'_, '_>) -> bool {
    r.nest.iter().any(|l| l.group == group::OUTPUT)
}

/// How many levels own a list (tex.web's `nest_ptr`).
fn nest_ptr(r: &Run<'_, '_>) -> usize {
    r.nest.iter().filter(|l| l.owns).count()
}

impl Run<'_, '_> {
    /// `if mode=vmode then build_page` (§1060, §1076, §1094, §1100,
    /// §1103, §812): the main vertical list is the current one.
    pub(crate) fn build_page_outer(&mut self) -> R<()> {
        if self.pg.is_some() && nest_ptr(self) == 0 {
            self.build_page()?;
        }
        Ok(())
    }

    /// `if nest_ptr=1 then build_page` (§1091, §1200): a paragraph just
    /// begun on the main vertical list.
    pub(crate) fn build_page_par(&mut self) -> R<()> {
        if self.pg.is_some() && nest_ptr(self) == 1 {
            self.build_page()?;
        }
        Ok(())
    }

    /// `build_page` (§994): the contributions go to the page; a page
    /// that fires is shipped, or its output routine started (the run
    /// reads its tokens next; §1026 resumes here).
    pub(crate) fn build_page(&mut self) -> R<()> {
        let Some(mut pg) = self.pg.take() else {
            return Ok(());
        };
        let res = self.build_page_with(&mut pg);
        self.pg = Some(pg);
        res
    }

    fn build_page_with(&mut self, pg: &mut Inline) -> R<()> {
        let v = self.v;
        drain(self, pg);
        if output_active(self) {
            return Ok(());
        }
        let params = PageParams {
            vsize: self.param("vsize")?,
            max_depth: self.param("maxdepth")?,
            top_skip: self.glue_param("topskip")?,
            holding_inserts: self.param("holdinginserts")?,
            saving_vdiscards: false,
        };
        while let Some(x) = pg.queue.pop_front() {
            if matches!(
                nodes::tag(v, x),
                nodes::T_PAGE_PARAMS
                    | nodes::T_MARKS_AT
                    | nodes::T_END
                    | nodes::T_FONT_AT
                    | nodes::T_PDFW_AT
                    | nodes::T_PDFQ_AT
            ) {
                continue;
            }
            let Some(p) = nodes::decode(v, x, &mut pg.side) else {
                continue;
            };
            // §1000: only a kern looks at what follows it
            let next = if matches!(p, Node::Kern(..)) {
                pg.queue
                    .front()
                    .and_then(|&y| nodes::decode(v, y, &mut Side::default()))
            } else {
                None
            };
            let mut halt = None;
            let out = {
                let mut class = |n: u16| match self.insert_class(n) {
                    Ok(c) => c,
                    Err(h) => {
                        halt.get_or_insert(h);
                        InsertClass {
                            box_height_plus_depth: None,
                            box_is_hbox: false,
                            count: 0,
                            dimen: 0,
                            skip: Glue::ZERO,
                        }
                    }
                };
                pg.pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            if let Some(h) = halt {
                return Err(h);
            }
            if let Outcome::Fire(f) = out {
                // §1017: what the page could not take goes on first
                for n in f.contributions.iter().rev() {
                    pg.queue.push_front(nodes::encode(v, n, &pg.side));
                }
                let data = self.page_data(&f, &pg.side)?;
                pg.pb = PageBuilder::new();
                pg.side = Side::default();
                if self.fire_page(data, None)? {
                    // the routine runs next in this run's input
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// `its_all_over`'s test (§1054) for a text paging inline: the
    /// page and the contributions are empty and no dead cycle pends.
    pub(crate) fn all_over_inline(&mut self) -> R<bool> {
        let Some(mut pg) = self.pg.take() else {
            return Ok(true);
        };
        drain(self, &mut pg);
        let empty = pg.pb.page.is_empty() && pg.queue.is_empty();
        self.pg = Some(pg);
        Ok(empty && self.cint(crate::fields::cell::DEADCYCLES)? == 0)
    }
}

/// §1026's end for a text paging inline: the held-over insertions and
/// the routine's list go before the contributions, and the page is
/// built again.
pub(crate) fn output_done(r: &mut Run<'_, '_>, left: Vec<ValueId>) -> R<()> {
    if let Some(pg) = r.pg.as_mut() {
        for x in left.into_iter().rev() {
            pg.queue.push_front(x);
        }
    }
    r.build_page()
}

/// At the run's end: what is still on the page or waiting goes back to
/// the main list (the `Breaks` node's), in order.
pub(crate) fn finish(r: &mut Run<'_, '_>) {
    let Some(mut pg) = r.pg.take() else {
        return;
    };
    drain(r, &mut pg);
    let v = r.v;
    let mut rest: Vec<ValueId> = pg
        .pb
        .page
        .iter()
        .map(|n| nodes::encode(v, n, &pg.side))
        .collect();
    rest.extend(pg.queue);
    for x in rest {
        r.append(0, x);
    }
}
