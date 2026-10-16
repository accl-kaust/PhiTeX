//! Pages: where the document's pages finish, and their output routines,
//! as nodes of the graph; every page value moves by def-use.
//!
//! - A text piece (an `exec` node, or a `resume` piece) typesets in
//!   parallel with every other, and never reads the page state. The
//!   items it puts on the main vertical list are its own contributions,
//!   delimited by candidate markers at the points where tex.web calls
//!   `build_page` (§994; [`Run::finish_candidate`]). At its end it
//!   places its fold after it ([`Fold`], its items as the argument): a
//!   node of its own that reads the version of the page state reaching
//!   it ([`PS`]: the page so far as §980–§991 keep it, the main list's
//!   fold, the page parameters in force, the contributions not looked at
//!   yet), folds the items in, candidate by candidate (§994–§1010, the
//!   pure builder of `phitex_typeset::page`), and defines the next
//!   version. The chain of the page is the folds and the finishers; the
//!   pieces run beside it. A piece with nothing for the main list places
//!   no fold: the next reader gets the version before it.
//! - A page that finishes at a candidate is a finisher node ([`Finish`])
//!   placed after the piece, its page as its arguments (`[j, data,
//!   carried, locals]`). It runs the output routine (§1012–§1026) as one
//!   run of its own, defines what the routine defines (its `\global`s,
//!   the marks, `\outputpenalty`, `\deadcycles`, [`SHIPPED`]) for
//!   everything after it, and at its end folds what the routine left
//!   (the held-over insertions, its list, the material carried over) as
//!   the next page's start, defining the next version of the state. A
//!   page that finishes right there is a finisher after it, and so on: a
//!   hundred-page paragraph is a hundred finishers in a row.
//! - A page that finishes at a candidate inside a piece, with the piece
//!   going on after it (a definition, an item, a line after the
//!   candidate), gets its finisher after the piece all the same, the
//!   rest of the piece's items carried past it (the fold folds them
//!   after the routine's, §1026). In tex.web the routine runs at the
//!   candidate, between the piece's two parts; here it runs after both.
//!   That is the same unless the routine changes what the second part
//!   read, or reads what the second part defined (or a binding a scope
//!   the second part closed hid): the finisher checks ([`Run::check_cut`],
//!   from the piece's reads and definitions handed down through the
//!   fold) and, when it is not, asks for a cut (`cutreq`). The client
//!   declares the piece's `cut:<node>` loop-back and the piece runs
//!   again next round: it folds in its own run ([`Run::fold_here`]),
//!   stopping at that candidate (its second pass; what the first pass
//!   did after the candidate is dropped), and its continuation
//!   ([`Run::cont_encode`]: the input left, the groups and lists open,
//!   the local bindings of those groups) is the argument of a `resume`
//!   piece ([`Resume`]) placed after the finisher, which reads what the
//!   finisher defined as any node does. The bindings local to the groups
//!   open at the cut go to the finisher as its `locals`: the routine
//!   runs inside those groups in tex.web and sees them.
//! - Nothing reads the whole list, nothing is fed back to a later round,
//!   nothing is replayed: the chain is one state version per piece that
//!   touches the main list and one finisher per page, each placed by the
//!   node that finds it.

use std::collections::VecDeque;

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::page::{
    Contents, Fired, InsertClass, Outcome, PageBuilder, PageIns, PageParams,
};
use phitex_typeset::pack::{PackParams, Spec};

use crate::build::{FoldState, IGNORE_DEPTH, Mode, fold_item};
use crate::fields::Par;
use crate::input::{Line, Seg};
use crate::nodes::{self, Side};
use crate::ops::{Shared, ops, run_node_parking};
use crate::run::{Halt, R, Run};
use crate::runstate::{ContSaved, ContSlot, ContState, Loc, State};
use crate::val::{glue_of, glue_value, group, int_at, int_of, list_of, str_of, toks_of, toks_value};

/// The page state's versions: defined by every piece that puts something
/// on the main list and by every finisher, read where it reaches.
pub(crate) const PS: &str = "ps";

/// What a finisher's output routine gives back to the main list
/// (§1026): `[markers, list]`, the run's own main-list items (page
/// parameter markers) and the routine's list, without the held-over
/// insertions (the fold continuation after the finisher has them).
/// Read by that continuation; predicted (a routine that gives nothing
/// back gives the same value every page).
pub(crate) const RTN: &str = "rtn";

/// What a fold that fired leaves to its continuation: `[held, carried,
/// rest]` (a field, not the continuation's arguments: an edit in the
/// rest of a piece re-runs the continuation instead of re-making it and
/// everything it placed).
pub(crate) const CONT: &str = "pages:cont";

/// Whether the page chain runs through the finishers as before the
/// predicted breaks (the finisher folds what its routine left).
pub(crate) fn fold_chain() -> bool {
    crate::exec::env_flag!("PHITEX_FOLD_CHAIN")
}

/// Defined by a `\shipout`: the mark that a page shipped before.
pub(crate) const SHIPPED: &str = "pages:shipped";

/// `max_dimen` (§421).
pub(crate) const MAX_DIMEN: i32 = 0o7777777777;

/// The fields the pages define through a text piece (beyond what its
/// hint can name).
pub(crate) fn is_page_field(name: &str) -> bool {
    name == PS || name == SHIPPED || name == CONT
}

/// What a run had done when it passed a candidate: at its end, a page
/// finishing at its last candidate finishes at the end of the piece iff
/// nothing of this changed after it; a finisher's check of a page
/// finishing at an earlier one takes what the run defined after it
/// (`stamp`, `gids`: [`State::defined_since`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CandMark {
    pub k: u32,
    pub stamp: u64,
    pub gids: u32,
    pub contribs: usize,
    pub kids: usize,
    pub lines: i64,
    /// The state table's shape at the candidate (definitions at the base
    /// level, saves, local definitions in groups, the clock) and the
    /// nest's depth: a group closed after the candidate restores without
    /// advancing the clock, and the page was finished inside it.
    pub sig: (usize, usize, usize, u64),
    pub nest: usize,
}

/// A finisher's page: its index, the material carried over past it
/// (the fold's §1017 contributions), what the routine leaves for the
/// next page (§1026: the held-over insertions, then its list), and the
/// rest of the piece the page finished inside of (its items after the
/// candidate, folded after the routine's).
#[derive(Clone, Debug, Default)]
pub(crate) struct FinCtx {
    pub j: i64,
    pub carried: Vec<ValueId>,
    pub pending: Vec<ValueId>,
    pub rest: Vec<ValueId>,
    /// How many of `pending` are the held-over insertions (§1022).
    pub held_n: usize,
    /// The `rtn` the finisher defined.
    pub rtn: Option<ValueId>,
}

// -------------------------------------------------------------- state

/// A version of the page builder's state (§980–§991), with the main
/// list's fold and the contributions not yet looked at.
pub(crate) struct PageState {
    /// Pages finished before.
    pub j: i64,
    pub pb: PageBuilder,
    /// The marks and whatsits of `pb.page` (decoded with it).
    pub side: Side,
    /// The main list's fold where the state is (the previous depth, the
    /// items since the last candidate).
    pub fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    pub params: Option<ValueId>,
    /// Contributions after the last candidate: folded items and markers,
    /// in order; the next candidate takes them first (§994).
    pub pending: Vec<ValueId>,
    /// The encoded nodes of `pb.page` and `pb.discards` up to their
    /// lengths here: the nodes on the page are never changed until the
    /// page fires (`build` then starts a new builder and clears these),
    /// so an encode re-encodes only the nodes after them.
    pub page_ids: Vec<ValueId>,
    pub discard_ids: Vec<ValueId>,
    /// The decisions of the page the last `build` fired (kept for the
    /// scan check, [`crate::vprof::check`]).
    pub fired: Option<crate::vprof::Decision>,
    /// The pages fired so far whose `\outputpenalty` is above -10001
    /// (the ordinary ones: the finishers' predictions step page
    /// counters by these, [`FinDefs::predict`]).
    pub normal: i64,
}

/// A page finishing: at candidate `k`, the finisher's page data, the
/// material to carry over past it, and the items after the candidate
/// (not folded: the finisher's fold takes them after the routine's).
pub(crate) struct Fire {
    pub k: u32,
    pub data: ValueId,
    pub carried: Vec<ValueId>,
    pub rest: Vec<ValueId>,
}

fn some(x: ValueId) -> Option<ValueId> {
    (x != ValueId::UNIT).then_some(x)
}

fn opt_ix(k: i64) -> Option<usize> {
    (k >= 0).then(|| usize::try_from(k).unwrap_or(0))
}

fn ix_int(v: &Values, k: Option<usize>) -> ValueId {
    v.int(k.map_or(-1, |k| i64::try_from(k).unwrap_or(i64::MAX)))
}

impl PageState {
    pub(crate) fn fresh(j: i64) -> PageState {
        PageState {
            j,
            pb: PageBuilder::new(),
            side: Side::default(),
            fold: FoldState {
                sf: 1000,
                pd: IGNORE_DEPTH,
                since_bp: 0,
            },
            params: None,
            pending: Vec::new(),
            page_ids: Vec::new(),
            discard_ids: Vec::new(),
            fired: None,
            normal: 0,
        }
    }

    /// The state a value holds (`None`, or anything else: the document's
    /// start).
    pub(crate) fn decode(v: &Values, x: Option<ValueId>) -> PageState {
        let Some(x) = x else {
            return PageState::fresh(0);
        };
        let xs = list_of(v, x);
        if xs.len() < 27 {
            return PageState::fresh(0);
        }
        let n = |i: usize| int_of(v, xs[i]);
        let n32 = |i: usize| i32::try_from(n(i)).unwrap_or(0);
        let mut side = Side::default();
        let mut page_ids = list_of(v, xs[20]);
        // (the page builder measures its items: a box's list stays
        // encoded, and the encode puts it back)
        let page: List = page_ids
            .iter()
            .filter_map(|&y| nodes::decode_shallow(v, y, &mut side))
            .collect();
        let mut discard_ids = list_of(v, xs[21]);
        let discards: List = discard_ids
            .iter()
            .filter_map(|&y| nodes::decode_shallow(v, y, &mut side))
            .collect();
        // (a node the decoder skipped: no reuse, the encode does it all)
        if page.len() != page_ids.len() {
            page_ids.clear();
        }
        if discards.len() != discard_ids.len() {
            discard_ids.clear();
        }
        let mut pb = PageBuilder::new();
        pb.page = page;
        pb.contents = match n(1) {
            1 => Contents::InsertsOnly,
            2 => Contents::BoxThere,
            _ => Contents::Empty,
        };
        for i in 0..8 {
            pb.so_far[i] = n32(2 + i);
        }
        pb.max_depth = n32(10);
        pb.best_break = opt_ix(n(11));
        pb.least_cost = n32(12);
        pb.best_size = n32(13);
        pb.last_glue = some(xs[14]).map(|g| glue_of(v, Some(g)));
        pb.last_penalty = n32(15);
        pb.last_kern = n32(16);
        pb.last_node_type = n32(17);
        pb.insert_penalties = n32(18);
        pb.ins = list_of(v, xs[19])
            .into_iter()
            .map(|e| {
                let ys = list_of(v, e);
                let g = |i: usize| ys.get(i).map_or(-1, |&y| int_of(v, y));
                PageIns {
                    n: u16::try_from(g(0)).unwrap_or(0),
                    split_up: g(1) != 0,
                    height: i32::try_from(g(2)).unwrap_or(0),
                    broken_ptr: opt_ix(g(3)),
                    broken_ins: opt_ix(g(4)),
                    last_ins: opt_ix(g(5)),
                    best_ins: opt_ix(g(6)),
                }
            })
            .collect();
        pb.discards = discards;
        PageState {
            j: n(0),
            pb,
            side,
            fold: FoldState {
                sf: n32(22),
                pd: n32(23),
                since_bp: usize::try_from(n(24)).unwrap_or(0),
            },
            params: some(xs[25]),
            pending: list_of(v, xs[26]),
            page_ids,
            discard_ids,
            fired: None,
            normal: xs.get(27).map_or(0, |&x| int_of(v, x)),
        }
    }

    /// The state as a value. The page's nodes up to `page_ids` are
    /// taken as encoded (they did not change: [`PageState::page_ids`]),
    /// the rest encoded, and the ids kept for the next encode.
    pub(crate) fn encode(&mut self, v: &Values) -> ValueId {
        let ids = |nodes: &List, known: &mut Vec<ValueId>, side: &Side| {
            if known.len() > nodes.len() {
                known.clear();
            }
            let from = known.len();
            known.extend(nodes[from..].iter().map(|n| nodes::encode(v, n, side)));
            v.list(known.clone())
        };
        let page = ids(&self.pb.page, &mut self.page_ids, &self.side);
        let discards = ids(&self.pb.discards, &mut self.discard_ids, &self.side);
        let i = |x: i64| v.int(x);
        let pb = &self.pb;
        let mut xs = vec![
            i(self.j),
            i(match pb.contents {
                Contents::Empty => 0,
                Contents::InsertsOnly => 1,
                Contents::BoxThere => 2,
            }),
        ];
        xs.extend(pb.so_far.iter().map(|&d| i(i64::from(d))));
        xs.push(i(i64::from(pb.max_depth)));
        xs.push(ix_int(v, pb.best_break));
        xs.push(i(i64::from(pb.least_cost)));
        xs.push(i(i64::from(pb.best_size)));
        xs.push(
            pb.last_glue
                .as_ref()
                .map_or(ValueId::UNIT, |g| glue_value(v, g)),
        );
        xs.push(i(i64::from(pb.last_penalty)));
        xs.push(i(i64::from(pb.last_kern)));
        xs.push(i(i64::from(pb.last_node_type)));
        xs.push(i(i64::from(pb.insert_penalties)));
        xs.push(v.list(
            pb.ins
                .iter()
                .map(|p| {
                    v.list(vec![
                        i(i64::from(p.n)),
                        i(i64::from(p.split_up)),
                        i(i64::from(p.height)),
                        ix_int(v, p.broken_ptr),
                        ix_int(v, p.broken_ins),
                        ix_int(v, p.last_ins),
                        ix_int(v, p.best_ins),
                    ])
                })
                .collect(),
        ));
        xs.push(page);
        xs.push(discards);
        xs.push(i(i64::from(self.fold.sf)));
        xs.push(i(i64::from(self.fold.pd)));
        xs.push(i(i64::try_from(self.fold.since_bp).unwrap_or(0)));
        xs.push(self.params.unwrap_or(ValueId::UNIT));
        xs.push(v.list(self.pending.clone()));
        xs.push(i(self.normal));
        v.list(xs)
    }

    /// The items of a run folded in: candidate by candidate
    /// ([`PageState::build`]), the rest left pending. `dead` is
    /// `\deadcycles` where the run is; `live` answers for an insertion
    /// class no marker of the run gives.
    pub(crate) fn fold_items(
        &mut self,
        v: &Values,
        raw: &[ValueId],
        dead: i64,
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        let mut buf = std::mem::take(&mut self.pending);
        for (i, &x) in raw.iter().enumerate() {
            match nodes::tag(v, x) {
                nodes::T_BUILD_PAGE => {
                    let k = u32::try_from(int_at(v, x, 1)).unwrap_or(0);
                    self.fold.since_bp = 0;
                    if let Some(mut f) = self.build(v, &mut buf, k, dead, live)? {
                        f.rest = raw[i + 1..].to_vec();
                        return Ok(Some(f));
                    }
                }
                nodes::T_SET_PAGE | nodes::T_INS_PARAMS | nodes::T_END | nodes::T_ABORT => {
                    buf.push(x);
                }
                _ => fold_item(v, 0, x, &mut self.fold, &mut buf),
            }
        }
        self.pending = buf;
        Ok(None)
    }

    /// One `build_page` call (§994) over the contributions `buf` at
    /// candidate `k`: what the page cannot take stays in `buf`.
    #[allow(clippy::too_many_lines)]
    fn build(
        &mut self,
        v: &Values,
        buf: &mut Vec<ValueId>,
        k: u32,
        dead: i64,
        live: &mut dyn FnMut(u16) -> R<InsertClass>,
    ) -> R<Option<Fire>> {
        // §1245: `\pagegoal=` and its kin set the page so far at once;
        // §1008: an insertion class's registers as the call reads them;
        // §987: the page parameters as the call reads them (the last
        // marker: the one the run put at this candidate), for every
        // contribution it takes
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        let mut page_params = None;
        let so_far = &mut self.pb.so_far;
        buf.retain(|&x| match nodes::tag(v, x) {
            nodes::T_PAGE_PARAMS => {
                page_params = Some(x);
                false
            }
            nodes::T_SET_PAGE => {
                let i = usize::try_from(int_at(v, x, 1)).unwrap_or(8);
                if i < 8 {
                    so_far[i] = i32::try_from(int_at(v, x, 2)).unwrap_or(0);
                }
                false
            }
            nodes::T_INS_PARAMS => {
                classes.push(class_of(v, x));
                false
            }
            _ => true,
        });
        if page_params.is_some() {
            self.params = page_params;
        }
        let params = self.params.map_or(
            PageParams {
                vsize: 0,
                max_depth: 0,
                top_skip: Glue::ZERO,
                holding_inserts: 0,
                saving_vdiscards: false,
            },
            |m| params_of(v, m),
        );
        let mut queue: VecDeque<ValueId> = std::mem::take(buf).into();
        let mut end_seen = None;
        while let Some(x) = queue.pop_front() {
            match nodes::tag(v, x) {
                // §1054: `\end` ends the job once the page and the
                // contributions are empty and no dead cycle pends; else
                // it fills the page out and comes round again (the
                // marker carried past the finisher)
                nodes::T_END => {
                    end_seen = Some(x);
                    if self.pb.page.is_empty() && queue.is_empty() && dead == 0 {
                        continue;
                    }
                    let xs = list_of(v, x);
                    for &y in xs.iter().skip(1).rev() {
                        queue.push_front(y);
                    }
                    continue;
                }
                // §1335: the main list ends there; the page is not shipped
                nodes::T_ABORT => {
                    buf.clear();
                    return Ok(None);
                }
                nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT => continue,
                _ => {}
            }
            let Some(p) = nodes::decode_shallow(v, x, &mut self.side) else {
                continue;
            };
            // §1000: only a kern looks at what follows it
            let next = if matches!(p, Node::Kern(..)) {
                queue
                    .front()
                    .and_then(|&y| nodes::decode(v, y, &mut Side::default()))
            } else {
                None
            };
            let mut halt = None;
            let out = {
                let mut class = |n: u16| {
                    classes
                        .iter()
                        .find(|c| c.0 == n)
                        .map(|c| c.1)
                        .unwrap_or_else(|| match live(n) {
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
                        })
                };
                self.pb.contribute(p, next.as_ref(), &params, &mut class)
            };
            if let Some(h) = halt {
                return Err(h);
            }
            match out {
                Outcome::Done => {}
                // the kern waits for its successor, with everything after
                Outcome::Wait(_) => {
                    queue.push_front(x);
                    break;
                }
                Outcome::Fire(f) => {
                    if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
                        eprintln!(
                            "page-trace: page {} fires at candidate {k}: vsize {} maxdepth {} (marker {})",
                            self.j,
                            params.vsize,
                            params.max_depth,
                            self.params.map_or_else(|| "none".to_owned(), |m| v.render(m))
                        );
                    }
                    // §1017: what the page could not take goes first
                    let mut carried: Vec<ValueId> = f
                        .contributions
                        .iter()
                        .map(|n| nodes::encode(v, n, &self.side))
                        .collect();
                    carried.extend(queue.drain(..));
                    if let Some(e) = end_seen {
                        carried.push(e);
                    }
                    let data = page_data(v, &f, &self.side);
                    if crate::vprof::checking() {
                        self.fired = Some(crate::vprof::Decision::of(&f, &self.pb, carried.len()));
                    }
                    self.pb = PageBuilder::new();
                    self.side = Side::default();
                    self.page_ids.clear();
                    self.discard_ids.clear();
                    self.j += 1;
                    if f.output_penalty > -10001 {
                        self.normal += 1;
                    }
                    self.fold.since_bp = 0;
                    buf.clear();
                    return Ok(Some(Fire {
                        k,
                        data,
                        carried,
                        rest: Vec::new(),
                    }));
                }
            }
        }
        *buf = queue.into();
        Ok(None)
    }
}

/// A `T_PAGE_PARAMS` marker's parameters (§980).
pub(crate) fn params_of(v: &Values, m: ValueId) -> PageParams {
    let xs = list_of(v, m);
    let g = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    PageParams {
        vsize: i32::try_from(g(1)).unwrap_or(0),
        max_depth: i32::try_from(g(2)).unwrap_or(0),
        top_skip: glue_of(v, xs.get(3).copied()),
        holding_inserts: i32::try_from(g(4)).unwrap_or(0),
        saving_vdiscards: false,
    }
}

/// A `T_INS_PARAMS` marker: `[tag, n, box h+d or -1, box is hbox, count,
/// dimen, skip]` (§1008's reads of eqtb for class `n`).
pub(crate) fn class_of(v: &Values, m: ValueId) -> (u16, InsertClass) {
    let xs = list_of(v, m);
    let g = |i: usize| xs.get(i).map_or(0, |&y| int_of(v, y));
    let h = g(2);
    (
        u16::try_from(g(1)).unwrap_or(0),
        InsertClass {
            box_height_plus_depth: (h >= 0).then(|| i32::try_from(h).unwrap_or(0)),
            box_is_hbox: g(3) != 0,
            count: i32::try_from(g(4)).unwrap_or(0),
            dimen: i32::try_from(g(5)).unwrap_or(0),
            skip: glue_of(v, xs.get(6).copied()),
        },
    )
}

/// Whether a fired page's `\outputpenalty` is -10001 or less.
fn fire_special(v: &Values, data: ValueId) -> bool {
    list_of(v, data).get(4).is_some_and(|&x| int_of(v, x) <= -10001)
}

/// A fired page as a finisher gets it: `[\box255, inserts, marks, held,
/// \outputpenalty]` (§1012–§1022).
fn page_data(v: &Values, f: &Fired, side: &Side) -> ValueId {
    let b255 = nodes::encode_box(v, &f.box255.node, side);
    let inserts: Vec<ValueId> = f
        .inserts
        .iter()
        .map(|(n, list)| {
            let l: Vec<ValueId> = list.iter().map(|x| nodes::encode(v, x, side)).collect();
            v.list(vec![v.int(i64::from(*n)), v.list(l)])
        })
        .collect();
    let marks: Vec<ValueId> = f
        .marks
        .iter()
        .map(|&k| side.marks.get(k as usize).copied().unwrap_or(ValueId::UNIT))
        .collect();
    let held: Vec<ValueId> = f
        .held_inserts
        .iter()
        .map(|n| nodes::encode(v, n, side))
        .collect();
    v.list(vec![
        b255,
        v.list(inserts),
        v.list(marks),
        v.list(held),
        v.int(i64::from(f.output_penalty)),
    ])
}

// ---------------------------------------------------------- the runs

impl Run<'_, '_> {
    fn ps_field(&self) -> Field {
        self.v.field(PS)
    }

    /// Whether the run is on the main vertical list, at most a paragraph
    /// just begun above it (`nest_ptr <= 1` of §1091, §1200; `mode =
    /// vmode` of §1076, §1094, §1100, §1103, §812).
    fn at_outer(&mut self) -> R<bool> {
        let base = self.base_mode()?;
        if base.lid != 0 || base.km != 1 {
            return Ok(false);
        }
        let owning: Vec<(i64, i32)> = self
            .nest
            .iter()
            .filter(|l| l.owns)
            .map(|l| (l.group, l.mode))
            .collect();
        // (§1145's `nest_ptr=1`: a display's math level right above the
        // outer vertical list, the paragraph before it broken into lines)
        Ok(match owning[..] {
            [] => true,
            [(g, m)] => (g == 0 && m.abs() == 102) || (g == group::MATH_SHIFT && m == 203),
            _ => false,
        })
    }

    /// A finish candidate: tex.web calls `build_page` here. The run
    /// notes what the builder reads of eqtb at this call (§987's page
    /// parameters, §1008's registers of the insertion classes it put on
    /// the list since the last candidate), then the candidate marker.
    /// At the candidate it is to stop at (its second pass after a cut),
    /// the run ends.
    pub(crate) fn finish_candidate(&mut self) -> R<()> {
        if !self.at_outer()? {
            return Ok(());
        }
        let v = self.v;
        let k = self.cand;
        if self.st.take_page_params() {
            if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: node {} candidate {k}: page params changed, vsize {}",
                    self.cx.node().raw(),
                    self.ipar(Par::vsize)?
                );
            }
            self.put_page_params();
        } else if k == 0 && !self.own_page_params_put() {
            // (the parameters reaching the piece, unless an item of it
            // already noted the ones it set, `Run::append`: that marker
            // is later in force and this one, after it, would undo it)
            self.put_page_params_reaching();
        }
        for n in std::mem::take(&mut self.ins_pending) {
            // (the class's registers as this run left them when it
            // defined one; else as they reach the fold, read there:
            // `Run::resolve_page_params`)
            let own = self.ins_registers_defined(n);
            let m = if own {
                self.ins_params_marker(n)?
            } else {
                v.list(vec![v.int(nodes::T_INS_PARAMS), v.int(i64::from(n))])
            };
            self.contribute("items", v.seq_unit(v.list(vec![v.int(0), m])));
        }
        let m = v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(i64::from(k))]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), m])));
        self.cand += 1;
        self.cand_marks.push(CandMark {
            k,
            stamp: self.st.stamp(),
            gids: self.st.gid_mark(),
            contribs: self.contribs.len(),
            kids: self.kids.len(),
            lines: self.lines_entered,
            sig: self.st.sig(),
            nest: self.nest.len(),
        });
        if self.stop_at == Some(k) {
            return Err(Halt::Cut);
        }
        Ok(())
    }

    /// Whether this run put a page parameter marker on the main list.
    fn own_page_params_put(&self) -> bool {
        let v = self.v;
        self.own_outer_items()
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS)
    }

    /// The items this run put on the main list, in order (its `items`
    /// contributions tagged with list 0).
    fn own_outer_items(&self) -> Vec<ValueId> {
        let v = self.v;
        let f = v.field("items");
        let mut out = Vec::new();
        for &(g, s) in &self.contribs {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(s) {
                let xs = list_of(v, e);
                if xs.len() == 2 && int_of(v, xs[0]) == 0 {
                    out.push(xs[1]);
                }
            }
        }
        out
    }

    /// The files this run wrote, opened or closed (its `streams`
    /// contributions from the `from`-th contribution on), as the fields
    /// `wstream:<name>`: a page finishing at a candidate before them
    /// ships its `\write`s before them in TeX's order ([`Run::check_cut`]).
    fn streams_since(&self, from: usize) -> Vec<Field> {
        let v = self.v;
        let f = v.field("streams");
        let mut out: Vec<Field> = Vec::new();
        for &(g, x) in self.contribs.get(from..).unwrap_or_default() {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(x) {
                let xs = list_of(v, e);
                // (`[j, entry]`: an entry tagged with its page)
                let entry = match xs.first() {
                    Some(&j) if v.as_int(j).is_some() => xs.get(1).map(|&y| list_of(v, y)).unwrap_or_default(),
                    _ => xs,
                };
                if let Some(&n) = entry.first() {
                    let w = v.field(&format!("wstream:{}", str_of(v, n)));
                    if !out.contains(&w) {
                        out.push(w);
                    }
                }
            }
        }
        out
    }

    /// A `T_INS_PARAMS` marker with class `n`'s registers as this run
    /// sees them (§1008).
    pub(crate) fn ins_params_marker(&mut self, n: u16) -> R<ValueId> {
        let v = self.v;
        let c = self.insert_class(n)?;
        Ok(v.list(vec![
            v.int(nodes::T_INS_PARAMS),
            v.int(i64::from(n)),
            v.int(c.box_height_plus_depth.map_or(-1, i64::from)),
            v.int(i64::from(c.box_is_hbox)),
            v.int(i64::from(c.count)),
            v.int(i64::from(c.dimen)),
            glue_value(v, &c.skip),
        ]))
    }

    /// Whether this run defined one of class `n`'s registers (its box,
    /// `\count`, `\dimen` or `\skip`).
    fn ins_registers_defined(&mut self, n: u16) -> bool {
        let b = self.box_field(i64::from(n));
        let bl = self.loc_of(b);
        if self.st.defined_after(bl, 0) {
            return true;
        }
        (0..3).any(|r| {
            let l = self.reg_loc(r, i32::from(n));
            self.st.defined_after(l, 0)
        })
    }

    pub(crate) fn insert_class(&mut self, n: u16) -> R<InsertClass> {
        let b = self.box_reg(i32::from(n))?;
        let v = self.v;
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: self.reg_int(0, n)?,
            dimen: self.reg_int(1, n)?,
            skip: glue_of(v, self.reg(2, i32::from(n))?),
        })
    }

    /// The `[tag]` page parameter markers of `raw` (the parameters
    /// reaching the piece, [`Run::put_page_params_reaching`]) given their
    /// values, when the piece defined a page parameter itself: the
    /// marker means the values in force where the piece began, and both
    /// the run's own state at its end and the fold after it hold the
    /// piece's later values (multicol sets `\vsize`: a first pass and a
    /// pass stopped at the candidate resolved them differently and never
    /// agreed). A piece that leaves them alone reads nothing here.
    fn pin_reaching_page_params(&mut self, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let names = ["param:vsize", "param:maxdepth", "param:topskip", "param:holdinginserts"];
        let mut defined = false;
        for n in names {
            let l = self.loc_of(v.field(n));
            if self.st.defined_after(l, 0) {
                defined = true;
            }
        }
        let has = raw
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS && list_of(v, x).len() == 1);
        if !defined || !has {
            return Ok(());
        }
        let mut xs = Vec::with_capacity(4);
        for n in names {
            xs.push(self.eng_read(v.field(n))?);
        }
        let int = |x: Option<ValueId>| x.map_or(0, |x| int_of(v, x));
        let marker = v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            v.int(int(xs[0])),
            v.int(int(xs[1])),
            glue_value(v, &glue_of(v, xs[2])),
            v.int(int(xs[3])),
        ]);
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
                *x = marker;
            }
        }
        Ok(())
    }

    /// The run's end on the page. A piece with items for the main list
    /// places its fold ([`Fold`]: the items folded into the page state
    /// reaching that node, the next version defined, the finisher placed
    /// where a page finishes) and runs no further; its own run never
    /// reads the page state. A piece a finisher asked to cut
    /// (`cut:<node>`, [`Run::read_cut_request`]) folds here instead, as a
    /// finisher does ([`Run::fold_here`]): a page finishing inside it is
    /// a cut, the run going again stopped at that candidate. A piece
    /// with nothing for the main list touches nothing.
    pub(crate) fn page_fold(&mut self) -> R<()> {
        let op = self.cx.op();
        let piece = (op == self.ops.exec || op == self.ops.resume) && (self.top || self.doc);
        let fin = op == self.ops.finish;
        if !piece && !fin {
            return Ok(());
        }
        // (`\end` inside groups: what they own goes to the main list)
        if self.claim_rest {
            self.end_open();
        }
        let raw = self.own_outer_items();
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") && self.stop_at.is_some() {
            eprintln!(
                "  page fold {}: pass stopped at {:?}: {} items, {} candidates, piece {piece}, top {} doc {}, contribs {}",
                self.cx.node().raw(),
                self.stop_at,
                raw.len(),
                self.cand_marks.len(),
                self.top,
                self.doc,
                self.contribs.len()
            );
        }
        if piece && raw.is_empty() {
            return Ok(());
        }
        if fin && !fold_chain() {
            return self.define_rtn(raw);
        }
        // (a pass stopped at its candidate folds here whatever it did
        // before it: the finisher and the resume are its to place)
        if piece && self.stop_at.is_none() && self.cut_req.is_none() && !self.needs_own_fold()? {
            return self.fold_apart(raw);
        }
        let mut prof = None;
        self.fold_here(raw, fin, &mut prof)?;
        if let Some(ints) = prof {
            self.contribute_vprof(&ints);
        }
        Ok(())
    }

    /// A finisher's end: what its routine gives back to the main list
    /// ([`RTN`]); the fold continuation placed after it folds that.
    fn define_rtn(&mut self, raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        let c = self.fin.clone().unwrap_or_default();
        let list = c.pending.get(c.held_n..).unwrap_or_default().to_vec();
        let x = v.list(vec![v.list(raw), v.list(list)]);
        self.cx.define_with(v.field(RTN), x, Locality::Global);
        if let Some(c) = self.fin.as_mut() {
            c.rtn = Some(x);
        }
        Ok(())
    }

    /// After a finisher placed by a fold: the fold continuation
    /// ([`Fold`] with `last` -2) that folds what the routine gives back,
    /// then `carried` and `rest`, the held-over insertions first
    /// (§1026), the page state's next version.
    fn place_continuation(&mut self, data: ValueId, carried: Vec<ValueId>, rest: Vec<ValueId>) {
        if fold_chain() {
            return;
        }
        let v = self.v;
        let held = list_of(v, data).get(3).copied().unwrap_or_else(|| v.list(Vec::new()));
        let x = v.list(vec![held, v.list(carried), v.list(rest)]);
        self.cx.define_with(v.field(CONT), x, Locality::Global);
        self.cx.follow(
            self.ops.fold,
            vec![v.list(Vec::new()), v.int(-2)],
        );
    }

    /// The piece's vertical profile, one unit of [`crate::vprof::VPROF`]
    /// at its position.
    fn contribute_vprof(&mut self, ints: &[i64]) {
        let v = self.v;
        crate::vprof::count_unit(ints);
        let x = crate::vprof::value(v, ints);
        self.contribute(crate::vprof::VPROF, v.seq_unit(x));
    }

    /// Whether a page finishing inside this piece would need a cut, as
    /// far as the finishers so far tell (`Ops::fin_touch`): the piece
    /// read a field a routine defines (the page number), or defined
    /// after its first candidate (or locally anywhere) a field a routine
    /// reads. Then the piece folds in its own run: a finisher's check
    /// would ask for the cut a round later, at the chain's cost.
    fn needs_own_fold(&mut self) -> R<bool> {
        let n = self.cand_marks.len();
        if n == 0 || (n == 1 && self.last_candidate() == Some(0)) {
            return Ok(false);
        }
        let touch = self.ops.fin_touch.clone();
        let t = touch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if t.reads.is_empty() && t.defs.is_empty() {
            return Ok(false);
        }
        let v = self.v;
        for l in self.st.read_locs() {
            let f = self.loc_field(l);
            if t.defs.contains(&f) {
                if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
                    eprintln!("  own fold {}: reads {}", self.cx.node().raw(), str_of(v, f.0));
                }
                return Ok(true);
            }
        }
        let gids = self.st.gid_mark();
        let m0 = self.cand_marks[0];
        let mut defs: Vec<Loc> = self
            .st
            .defined_since(v, m0.stamp, m0.gids)
            .into_iter()
            .map(|d| d.0)
            .collect();
        defs.extend(
            self.st
                .defined_since(v, 0, gids)
                .into_iter()
                .filter(|d| d.2 == Locality::Local)
                .map(|d| d.0),
        );
        let mut defs: Vec<Field> = defs.into_iter().map(|l| self.loc_field(l)).collect();
        defs.extend(self.streams_since(m0.contribs));
        for f in defs {
            if t.reads.contains(&f) {
                if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
                    eprintln!("  own fold {}: defines {}", self.cx.node().raw(), str_of(v, f.0));
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The candidate the run passed last, when nothing happened after
    /// it: a page finishing there finishes at the piece's end. A group
    /// that closed after it is something (its local definitions, in force
    /// at the candidate, are gone at the end: amsbook's
    /// `\cleardoublepage` is `\clearpage{\pagestyle{empty}...\newpage}`).
    fn last_candidate(&self) -> Option<u32> {
        self.cand_marks.last().and_then(|m| {
            (m.stamp == self.st.stamp()
                && m.sig == self.st.sig()
                && m.gids == self.st.gid_mark()
                && m.nest == self.nest.len()
                && m.contribs == self.contribs.len()
                && m.kids == self.kids.len()
                && m.lines == self.lines_entered)
                .then_some(m.k)
        })
    }

    /// A piece's fold placed after it ([`Fold`]), with what the
    /// finisher's check needs ([`Run::check_cut`]) when a page can finish
    /// inside the piece with commands after it: the locations the run
    /// read from the engine with the values it read, and per candidate
    /// the names the run defined after it (and its local definitions at
    /// any time: a scope closing after the candidate hides them from a
    /// finisher placed after the piece).
    fn fold_apart(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        self.pin_reaching_page_params(&mut raw)?;
        let v = self.v;
        let last = self.last_candidate();
        let n = self.cand_marks.len();
        let inside_possible = !(n == 1 && last == Some(0)) && n > 0;
        let (reads, defs) = if inside_possible {
            let rs: Vec<ValueId> = self
                .st
                .read_values()
                .into_iter()
                .map(|(l, x)| {
                    let f = self.loc_field(l);
                    v.list(vec![f.0, opt_val(x)])
                })
                .collect();
            let gids = self.st.gid_mark();
            let locals: Vec<Field> = self
                .st
                .defined_since(v, 0, gids)
                .into_iter()
                .filter(|(_, _, l)| *l == Locality::Local)
                .map(|(l, _, _)| self.loc_field(l))
                .collect();
            let marks = self.cand_marks.clone();
            let mut per = Vec::with_capacity(n);
            for m in &marks {
                if Some(m.k) == last {
                    per.push(v.list(Vec::new()));
                    continue;
                }
                let mut names: Vec<(ValueId, bool)> =
                    locals.iter().map(|f| (f.0, false)).collect();
                for (l, _, _) in self.st.defined_since(v, m.stamp, m.gids) {
                    let stands = self.st.def_stands(l);
                    names.push((self.loc_field(l).0, stands));
                }
                names.extend(self.streams_since(m.contribs).into_iter().map(|f| (f.0, false)));
                names.sort_unstable_by_key(|x| (x.0.raw(), !x.1));
                names.dedup_by_key(|x| x.0);
                per.push(v.list(
                    names
                        .into_iter()
                        .map(|(f, g)| v.list(vec![f, v.int(i64::from(g))]))
                        .collect(),
                ));
            }
            (v.list(rs), v.list(per))
        } else {
            (ValueId::UNIT, ValueId::UNIT)
        };
        let ints = crate::vprof::encode(v, &raw);
        self.contribute_vprof(&ints);
        let pid = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        self.cx.follow(
            self.ops.fold,
            vec![
                v.list(raw),
                v.int(last.map_or(-1, i64::from)),
                pid,
                reads,
                defs,
            ],
        );
        Ok(())
    }

    /// The fold in this run: its items folded into the page state
    /// reaching it, the next version defined, and the finisher (and the
    /// resume piece after a cut) placed where a page finishes. A stop
    /// here parks the run: the fold is a command of its own
    /// ([`Run::begin_command`]).
    #[allow(clippy::too_many_lines)]
    fn fold_here(&mut self, raw: Vec<ValueId>, fin: bool, prof: &mut Option<Vec<i64>>) -> R<()> {
        let t0 = std::time::Instant::now();
        let r = self.fold_here_(raw, fin, prof);
        phitex_pdf::phase::add(18, t0);
        r
    }

    #[allow(clippy::too_many_lines)]
    fn fold_here_(&mut self, mut raw: Vec<ValueId>, fin: bool, prof: &mut Option<Vec<i64>>) -> R<()> {
        let v = self.v;
        // (its input read out: the siblings it takes are known, and so
        // are the fields it defines: what its state publishes and the
        // page state; a park at the page state's read below holds no
        // other sibling and no reader of another field, a finisher's
        // `MayDefine::Any` and a piece's hint bound narrowed to them)
        let n = self.claim_count();
        if crate::exec::env_flag!("PHITEX_DEBUG_CLAIMS") {
            eprintln!("claim: node {} claims {n} at its fold (pulled {})", self.cx.node().raw(), self.pulled);
        }
        self.cx.claim(n);
        let mut defs = self.published_fields();
        defs.push(v.field(PS));
        defs.push(v.field(crate::build::VTAIL));
        defs.push(v.field(SHIPPED));
        defs.push(v.field(CONT));
        if let Some(list) = crate::exec::env_str!("PHITEX_DEBUG_NODES")
            && list.split(',').any(|x| x.trim().parse::<u64>().ok() == Some(self.cx.node().raw()))
        {
            let names: Vec<String> = defs.iter().map(|f| crate::val::field_name(v, *f)).collect();
            eprintln!("narrow {} (resumed {}): {names:?}", self.cx.node().raw(), self.resumed);
        }
        self.cx.narrow(MayDefine::Names(defs));
        self.begin_command();
        let f = self.ps_field();
        let ps_in = self.eng_read(f)?;
        // (the run's own main-list items, before a finisher's are joined
        // by what the page left)
        self.define_vtail(&raw)?;
        // (before the state is taken: a stop here leaves it for the
        // attempt that resumes)
        let dead = self.deadcycles_for(&raw)?;
        // (a finisher's items are built after its routine, with the
        // parameters as the routine leaves them, §987: its own state)
        if !fin {
            self.pin_reaching_page_params(&mut raw)?;
            *prof = Some(crate::vprof::encode(v, &raw));
        }
        self.resolve_page_params(&mut raw)?;
        let mut timer = FoldTimer::new(raw.len());
        let cache = self.ops.page_states.clone();
        let mut ps = timer.decode(|| {
            ps_in
                .and_then(|x| cache.take(x))
                .unwrap_or_else(|| PageState::decode(v, ps_in))
        });
        let scan_in = crate::vprof::checking().then(|| crate::vprof::ScanState::of(v, &ps));
        if fin {
            // §1026: the held-over insertions and the routine's list,
            // then the contributions put back, and the page built again;
            // then the rest of the piece the page finished inside of
            let c = self.fin.clone().unwrap_or_default();
            raw.extend(c.pending);
            raw.extend(c.carried);
            raw.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
            raw.extend(c.rest);
        }
        let fire = {
            let mut live = |n: u16| self.insert_class(n);
            timer.fold(|| ps.fold_items(v, &raw, dead, &mut live))?
        };
        if let Some(scan) = scan_in {
            let items = match prof.as_ref() {
                Some(ints) if !fin => crate::vprof::decode(ints),
                _ => crate::vprof::decode(&crate::vprof::encode(v, &raw)),
            };
            self.scan_check(scan, &items, dead, &mut ps, fire.as_ref());
        }
        self.mark.active = false;
        let Some(fire) = fire else {
            // a second pass whose state no longer finishes the page at
            // its candidate (the version reaching it changed since the
            // first pass read it): nothing placed, the run goes again
            // from the top as a first pass
            if self.stop_at.is_some() {
                return Ok(());
            }
            let e = timer.encode(|| ps.encode(v));
            self.cx.define_with(f, e, Locality::Global);
            cache.put(e, ps);
            return Ok(());
        };
        // (a second pass stopped at the candidate: the piece goes on
        // after it, whatever the pass did not run)
        let at_end = fin || (self.stop_at != Some(fire.k) && self.last_candidate() == Some(fire.k));
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            eprintln!(
                "  fold {}: page {} fires at candidate {} (at end {at_end}, stop_at {:?}, cut asked {:?})",
                self.cx.node().raw(),
                ps.j,
                fire.k,
                self.stop_at,
                self.cut_req
            );
        }
        if !at_end {
            self.cut = Some(fire.k);
            if self.stop_at != Some(fire.k) {
                // the second pass, stopped at the candidate, places it
                return Ok(());
            }
        }
        let e = ps.encode(v);
        self.cx.define_with(f, e, Locality::Global);
        let j = ps.j - 1;
        let normal = ps.normal;
        cache.put(e, ps);
        let (cont, locals) = if at_end {
            (None, Vec::new())
        } else {
            let (c, l) = self.cont_encode()?;
            (Some(c), l)
        };
        let text = self.output_hint()?;
        let mut fargs = vec![v.int(j), fire.data, v.list(if fold_chain() { fire.carried.clone() } else { Vec::new() }), v.list(locals)];
        fargs.extend([ValueId::UNIT; 5]);
        fargs.push(text);
        fargs.push(v.int(normal));
        fargs.push(v.int(i64::from(fire_special(v, fire.data))));
        self.cx.follow(self.ops.finish, fargs);
        if !fin {
            self.place_continuation(fire.data, fire.carried, Vec::new());
        }
        if let Some(cont) = cont {
            let flags = i64::from(self.top) | (i64::from(self.doc) << 1);
            // (the cutting node: the finishers between it and the resume
            // tell which fields they defined globally, `gdefs`)
            let pid = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
            self.cx.follow(self.ops.resume, vec![cont, v.int(flags), pid]);
        }
        Ok(())
    }

    /// A piece's start: whether a finisher asked for a cut of this node
    /// (`cut:<node>`, an environment field the client sets from the
    /// finishers' `cutreq` contributions, [`Run::check_cut`]), at which
    /// candidate.
    pub(crate) fn read_cut_request(&mut self) -> R<()> {
        let args = self.cx.args();
        let k = args.get(1).map_or(0, |&t| int_of(self.v, t));
        if k & 3 == 0 {
            return Ok(());
        }
        let f = self.v.field(&format!("cut:{}", self.cx.node().raw()));
        self.cut_req = self
            .cx
            .use_field(f)?
            .and_then(|x| u32::try_from(int_of(self.v, x)).ok());
        Ok(())
    }

    /// A finisher placed after the piece its page finished inside of
    /// (arguments past the fourth: the rest of the piece's items, the
    /// piece's node, the candidate, what the piece read, what it defined
    /// after the candidate): whether the routine, run here instead of at
    /// the candidate, changed what the piece's commands after it saw or
    /// saw what they defined. If so, a cut is asked for: the piece runs
    /// again next round with the routine between its two parts.
    fn check_cut(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        if args.len() < 9 {
            return Ok(());
        }
        let (pid, k, reads, defs) = (a(5), a(6), a(7), a(8));
        let mut gdefs: Vec<(ValueId, ValueId)> = Vec::new();
        for (loc, x, l) in self.st.published(v, self.rec_load) {
            if l == Locality::Global {
                gdefs.push((self.loc_field(loc).0, x));
            }
        }
        let mut read: Vec<ValueId> = self
            .st
            .read_locs()
            .into_iter()
            .map(|l| self.loc_field(l).0)
            .collect();
        // (the page placed by this finisher's `place` node reads the
        // writer's state where the finisher stands: a writer command the
        // piece ran after the candidate came before the page's objects)
        if self.placed_page {
            read.push(v.field("pdf:state").0);
        }
        // (the files the page's `\write`s went to: a write, open or
        // close the piece made after the candidate came after them)
        read.extend(self.streams_since(0).into_iter().map(|f| f.0));
        let mut why = None;
        for e in list_of(v, reads) {
            let xs = list_of(v, e);
            if let [f, x] = xs[..]
                && let Some(&(_, y)) = gdefs.iter().find(|(g, _)| *g == f)
                && y != x
            {
                why = Some(format!("changed {}", str_of(v, f)));
                break;
            }
        }
        // (a location both defined, the piece's definition standing at
        // its end: the piece's is the later one in tex.web, so the
        // routine's gives way to it: the value reaching here put back)
        let mut restore = Vec::new();
        if why.is_none() {
            for e in list_of(v, defs) {
                let xs = list_of(v, e);
                let Some(&f) = xs.first() else { continue };
                let stands = xs.get(1).is_some_and(|&g| int_of(v, g) != 0);
                if read.contains(&f) {
                    why = Some(format!("after {}", str_of(v, f)));
                    break;
                }
                if gdefs.iter().any(|(g, _)| *g == f) && stands {
                    restore.push(Field(f));
                }
            }
        }
        if why.is_none() {
            for f in restore {
                let x = self.eng_read(f)?;
                let loc = self.loc_of(f);
                self.set_loc(loc, x.map_or(crate::runstate::Native::Absent, crate::runstate::Native::Raw), true);
            }
            return Ok(());
        }
        let Some(why) = why else { return Ok(()) };
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") || crate::exec::env_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "  cut asked: piece {} at candidate {} ({why})",
                int_of(v, pid),
                int_of(v, k)
            );
        }
        self.contribute("cutreq", v.seq_unit(v.list(vec![pid, k])));
        Ok(())
    }

    /// `\pagegoal`, `\pagetotal`, … (§421): the page state reaching this
    /// piece with its own items up to its last candidate folded in (the
    /// contributions after it are not on the page), read, not defined.
    pub(crate) fn page_dimen(&mut self, m: i32) -> R<i32> {
        let v = self.v;
        let raw = self.own_outer_items();
        let last = raw
            .iter()
            .rposition(|&x| nodes::tag(v, x) == nodes::T_BUILD_PAGE)
            .map_or(0, |i| i + 1);
        let f = self.ps_field();
        let mut ps = PageState::decode(v, self.eng_read(f)?);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let mut head = raw[..last].to_vec();
        self.resolve_page_params(&mut head)?;
        {
            let mut live = |n: u16| self.insert_class(n);
            let _ = ps.fold_items(v, &head, dead, &mut live)?;
        }
        if ps.pb.contents == Contents::Empty {
            return Ok(if m == 0 { MAX_DIMEN } else { 0 });
        }
        Ok(ps.pb.so_far[usize::try_from(m).unwrap_or(0).min(7)])
    }

    /// The page a finisher finishes (its first argument); `usize::MAX`
    /// in any other run.
    pub(crate) fn page_index(&self) -> usize {
        self.fin
            .as_ref()
            .and_then(|c| usize::try_from(c.j).ok())
            .unwrap_or(usize::MAX)
    }

    /// `fire_up` (§1012–§1025) for a finisher's page: the marks and the
    /// insertion boxes defined, `\deadcycles` counted, then the output
    /// routine pushed to run on this run's nest in its group (`true`), or
    /// the page shipped by the default routine (§1023; also after too
    /// many dead cycles, §1024), the held-over insertions pending.
    #[allow(clippy::too_many_lines)]
    fn fire_page(&mut self, d: ValueId) -> R<bool> {
        let v = self.v;
        let data = list_of(v, d);
        let a = |i: usize| data.get(i).copied().unwrap_or(ValueId::UNIT);
        let (b255, inserts, marks, held) =
            (a(0), list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if let Some(c) = self.fin.as_mut() {
            c.held_n = held.len();
        }
        // §1012: the marks
        let mut top = self
            .get_loc(crate::fields::cell::MARK_TOP)?
            .filter(|&x| x != ValueId::UNIT);
        let bot = self
            .get_loc(crate::fields::cell::MARK_BOT)?
            .filter(|&x| x != ValueId::UNIT);
        let mut first = None;
        let mut new_bot = bot;
        if bot.is_some() {
            top = bot;
        }
        crate::etex::fire_marks_values(self, &marks, None)?;
        for &m in &marks {
            if crate::etex::is_class_mark(v, Some(m)) {
                continue;
            }
            if first.is_none() {
                first = Some(m);
            }
            new_bot = Some(m);
        }
        if top.is_some() && first.is_none() {
            first = top;
        }
        for (name, m) in [("mark:top", top), ("mark:first", first), ("mark:bot", new_bot)] {
            self.set(name, m.unwrap_or(ValueId::UNIT), true);
        }
        // §1018–§1021: the insertion boxes
        for e in inserts {
            let xs = list_of(v, e);
            let n = xs.first().map_or(0, |&x| int_of(v, x));
            let old = self.box_reg(i32::try_from(n).unwrap_or(0))?;
            let mut s2 = Side::default();
            let mut l: List = match old.and_then(|b| nodes::decode(v, b, &mut s2)) {
                Some(Node::Box(b)) => b.list,
                _ => Vec::new(),
            };
            for &x in &xs.get(1).map(|&l| list_of(v, l)).unwrap_or_default() {
                if let Some(nd) = nodes::decode(v, x, &mut s2) {
                    l.push(nd);
                }
            }
            let p = phitex_typeset::pack::vpack(l, Spec::Additional(0), &PackParams::default());
            let b = nodes::encode_box(v, &p.node, &s2);
            self.set(&format!("box:{n}"), v.list(vec![v.int(0), b]), true);
        }
        self.set("param:outputpenalty", v.int(output_penalty), true);
        let output = self.par(Par::output)?;
        let mut toks = toks_of(v, output);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let max_dead = i64::from(self.param("maxdeadcycles")?);
        if !toks.is_empty() && dead >= max_dead {
            // §1024
            self.log(
                "error",
                &format!("Output loop---{dead} consecutive dead cycles"),
            )?;
            toks.clear();
        }
        if toks.is_empty() {
            // §1023: the default output routine: the held-over insertions
            // go back to the contributions, the page is shipped
            if let Some(c) = self.fin.as_mut() {
                c.pending = held;
            }
            self.ship_out(b255)?;
            return Ok(false);
        }
        // §1025
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(dead + 1), true);
        self.set_cell(
            crate::fields::cell::BOX255,
            v.list(vec![v.int(0), b255]),
            true,
        );
        self.push_list(-1, group::OUTPUT, vec![v.list(held)]);
        self.normal_paragraph()?;
        // (the routine's token list carries its braces, §1226: the group
        // is open already, so its `{` is not read; its `}` ends it)
        toks.remove(0);
        self.push_toks(toks);
        Ok(true)
    }

    /// The output group's `}` (§1026): the held-over insertions and the
    /// routine's list are what the next page sees first (folded at the
    /// run's end); `\box255` must be void; a page parameter the routine
    /// set governs the next page's breaks.
    pub(crate) fn output_end(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let (items, _) = self.list_items(md.lid)?;
        let mut left = md.extra.first().map(|&x| list_of(v, x)).unwrap_or_default();
        self.close_group(md)?;
        if self.st.take_page_params() {
            self.put_page_params();
        }
        if self.box_reg(255)?.is_some() {
            // §1028
            self.log("error", "Output routine didn't use all of \\box255")?;
            self.set_cell(
                crate::fields::cell::BOX255,
                v.list(vec![v.int(0), ValueId::UNIT]),
                true,
            );
        }
        self.set_cell(crate::fields::cell::INSPEN, v.int(0), true);
        left.extend(items);
        if let Some(c) = self.fin.as_mut() {
            c.pending = left;
        }
        Ok(())
    }

    /// `ship_out` (§638–§640): the page goes to the `pages` accumulator
    /// with its `\count0..9` and fonts; its `\write`s and `\openout`s
    /// happen now (§1366–§1374); [`SHIPPED`] is defined for the text
    /// after.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn ship_out(&mut self, b: ValueId) -> R<()> {
        let v = self.v;
        let mut counts = [0i32; 10];
        for (k, c) in counts.iter_mut().enumerate() {
            *c = self.reg_int(0, k)?;
        }
        let mut j = 9;
        while j > 0 && counts[j] == 0 {
            j -= 1;
        }
        let mut s = String::from("[");
        for (k, c) in counts.iter().enumerate().take(j + 1) {
            s.push_str(&c.to_string());
            if k < j {
                s.push('.');
            }
        }
        self.log("ship", &s)?;
        let timed = crate::exec::env_flag!("PHITEX_TEX_TIME");
        let t0 = std::time::Instant::now();
        let mut side = Side::default();
        // (the page's boxes are decoded once, here, for the writer: their
        // decodings are not kept for a later decode)
        let prev = crate::val::NO_KEEP.with(|c| c.replace(true));
        let decoded = nodes::decode(v, b, &mut side);
        crate::val::NO_KEEP.with(|c| c.set(prev));
        let Some(Node::Box(bx)) = decoded else {
            return Ok(());
        };
        let t_decode = t0.elapsed();
        phitex_pdf::phase::add(1, t0);
        let t_p2 = std::time::Instant::now();
        // (the positions walk only for a whatsit that needs one: a
        // `\write` expands without positions; a plain page skips the
        // walk, the second copy of the box it makes and the font view)
        let needs_pos = side.whats.iter().any(|&w| {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            !matches!(kind.as_str(), "write" | "open" | "close")
        });
        let pos = if needs_pos {
            crate::pdfprim::positions(self, &bx)?
        } else {
            crate::pdfprim::Positions {
                at: phitex_defuse::FxHashMap::default(),
                page_height: 0,
                pdf: self.param("pdfoutput")? > 0,
            }
        };
        let t_pos = t0.elapsed();
        // what the PDF writer makes of the page's pdfTeX whatsits
        let mut pdf_data = Vec::new();
        // §1366: the whatsits in the order `hlist_out`/`vlist_out` meet them
        for (wi, w) in side.whats.clone().into_iter().enumerate() {
            let xs = list_of(v, w);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            match kind.as_str() {
                "write" => {
                    let k = i32::try_from(xs.get(1).map_or(0, |&y| int_of(v, y))).unwrap_or(0);
                    let toks = toks_of(v, xs.get(2).copied());
                    self.write_out(k, &toks)?;
                }
                "open" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    let name = xs.get(2).map(|&y| str_of(v, y)).unwrap_or_default();
                    self.open_out(n, &name)?;
                }
                "close" => {
                    let n = xs.get(1).map_or(0, |&y| int_of(v, y));
                    self.set(&format!("write:{n}"), ValueId::UNIT, true);
                }
                k => {
                    let at = u32::try_from(wi).ok().and_then(|i| pos.at.get(&i).copied());
                    crate::pdfprim::at_shipout(self, k, at, &pos)?;
                    if pos.pdf
                        && let Some(d) = crate::pdfprim::writer_data(self, &xs)?
                    {
                        let h = i64::try_from(wi).unwrap_or(0);
                        pdf_data.push(v.list(vec![v.int(h), d]));
                    }
                }
            }
        }
        let t_whats = t0.elapsed();
        phitex_pdf::phase::add(2, t_p2);
        let t_p3 = std::time::Instant::now();
        let page_box = Node::Box(bx);
        let view = self.font_view(std::slice::from_ref(&page_box))?;
        let mut fonts = Vec::new();
        let mut ids: Vec<u32> = view.fonts.keys().copied().collect();
        ids.sort_unstable();
        for &f in &ids {
            let info = self.get(&format!("fontinfo:{f}"))?;
            let Some(i) = crate::fonts::info(v, info) else {
                continue;
            };
            let Some(file) = i.file else { continue };
            fonts.push(v.list(vec![
                v.int(i64::from(f)),
                v.str(&i.name),
                file,
                v.int(i64::from(i.s)),
            ]));
        }
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let page = v.list(vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)]);
        let t_fonts = t0.elapsed();
        phitex_pdf::phase::add(3, t_p3);
        let t_p4 = std::time::Instant::now();
        let shown = match &page_box {
            Node::Box(bx) if pos.pdf => crate::pdfout::ship(self, page, bx, &side, &ids)?,
            _ => page.raw(),
        };
        phitex_pdf::phase::add(4, t_p4);
        if timed {
            eprintln!(
                "shipout: decode_us {} positions_us {} whatsits_us {} fonts_us {} pdf_us {}",
                t_decode.as_micros(),
                (t_pos - t_decode).as_micros(),
                (t_whats - t_pos).as_micros(),
                (t_fonts - t_whats).as_micros(),
                (t0.elapsed() - t_fonts).as_micros()
            );
        }
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
        self.contribute("pagehash", h);
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
        let j = i64::try_from(self.page_index()).unwrap_or(-1);
        self.set(SHIPPED, v.int(j), true);
        phitex_pdf::phase::add(15, t0);
        Ok(())
    }

    /// The parameters pdfTeX reads at each page (pdfTeX §750–§757), with
    /// whether the file is PDF and the colour stacks made so far.
    fn pdf_page_params(&mut self, pdf: bool) -> R<ValueId> {
        let v = self.v;
        let mut xs = vec![v.int(i64::from(pdf))];
        for p in [
            "pdfhorigin",
            "pdfvorigin",
            "pdfpagewidth",
            "pdfpageheight",
            "hoffset",
            "voffset",
            "pdfcompresslevel",
            "pdfdecimaldigits",
            "pdfminorversion",
            "pdfmajorversion",
            "mag",
        ] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        xs.push(
            self.get_loc(crate::fields::cell::PDF_COLORSTACKS)?
                .unwrap_or(ValueId::UNIT),
        );
        for p in ["pdfobjcompresslevel", "pdfgentounicode"] {
            xs.push(v.int(i64::from(self.param(p)?)));
        }
        for p in ["pdfpageattr", "pdfpagesattr", "pdfpageresources"] {
            let toks = toks_of(v, self.get(&format!("param:{p}"))?);
            let mut s = String::new();
            self.show_toks_into(&mut s, &toks)?;
            xs.push(v.str(&s));
        }
        Ok(v.list(xs))
    }
}

// ------------------------------------------------------ continuations

/// A line of the input left, as a text segment of a node's arguments
/// (`decode_seg`): the characters from its position on, its state, and
/// whether its end is still due.
pub(crate) fn line_frame(v: &Values, s: &Line) -> Option<ValueId> {
    let end_due = s.ends && s.endch < -1;
    if s.loc >= s.buf.len() && !end_due {
        return None;
    }
    let text: String = s.buf[s.loc.min(s.buf.len())..]
        .iter()
        .map(|&c| char::from_u32(c).unwrap_or('?'))
        .collect();
    let starts = s.starts && s.endch == -2;
    let mut xs = vec![
        v.str("s"),
        v.str(&text),
        v.int(i64::from(s.state)),
        v.int(-2),
        v.int(i64::from(starts) | (i64::from(end_due) << 1) | (i64::from(s.rescanned) << 2)),
    ];
    if let Some((name, at)) = s.origin {
        let at = at + s.loc.min(s.buf.len());
        xs.push(name);
        xs.push(v.int(i64::try_from(at).unwrap_or(0)));
        if s.lineno > 0 {
            xs.push(v.int(i64::from(s.lineno)));
        }
    }
    Some(v.list(xs))
}

fn file_frame(v: &Values, f: &crate::run::FileRest) -> ValueId {
    v.list(vec![
        v.str("f"),
        f.id,
        v.int(i64::try_from(f.off).unwrap_or(0)),
        f.name,
    ])
}

fn opt_val(x: Option<ValueId>) -> ValueId {
    x.unwrap_or(ValueId::UNIT)
}

fn gid_int(v: &Values, g: Option<u32>) -> ValueId {
    v.int(g.map_or(-1, i64::from))
}

fn int_gid(k: i64) -> Option<u32> {
    (k >= 0).then(|| u32::try_from(k).unwrap_or(0))
}

impl Run<'_, '_> {
    /// The run stopped at a candidate, as a value for the resume piece:
    /// the input left (the token levels' rests, top first, then the text
    /// not reached), the levels of the nest with their lists, the
    /// conditionals open, the state table's groups (their local bindings
    /// and saves), the base-level values of the locations those bindings
    /// hide (the resume checks them against the engine's: a global
    /// definition in between stands, §283), and the rest of the run's
    /// counters. Also the group-local bindings themselves, for the
    /// finisher.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn cont_encode(&mut self) -> R<(ValueId, Vec<ValueId>)> {
        let v = self.v;
        let here: Vec<ValueId> = match self.file_line_here() {
            Some(crate::run::FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
            Some(crate::run::FileLine::Piece(name)) => vec![name, v.int(-1)],
            None => Vec::new(),
        };
        let toks_frame = |toks: &[crate::val::Tok]| {
            let mut t = vec![v.str("t"), toks_value(v, toks)];
            t.extend(here.iter().copied());
            v.list(t)
        };
        let mut frames = Vec::new();
        for l in self.input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    let r = self.input.arena.get(l.rest());
                    if !r.is_empty() {
                        frames.push(toks_frame(r));
                    }
                }
                crate::input::kind::LINE => {
                    if let Some(f) = line_frame(v, self.input.line(*l)) {
                        frames.push(f);
                    }
                }
                _ => frames.push(file_frame(v, self.input.file(*l))),
            }
        }
        for s in self.input.text_rest() {
            match s {
                Seg::Line(l) => {
                    if let Some(f) = line_frame(v, l) {
                        frames.push(f);
                    }
                }
                Seg::Toks(t) => {
                    if !t.is_empty() {
                        frames.push(toks_frame(t));
                    }
                }
                Seg::File(f) => frames.push(file_frame(v, f)),
            }
        }
        let nest: Vec<ValueId> = self
            .nest
            .iter()
            .map(|l| {
                let (lang, packed) = l
                    .par
                    .map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| (v.int(a), v.int(b)));
                v.list(vec![
                    v.int(i64::from(l.mode)),
                    v.int(l.group),
                    v.int(l.lid),
                    v.int(i64::from(l.owns)),
                    v.list(l.list.iter().map(|x| x.to_value(v)).collect()),
                    v.int(i64::from(l.fold.sf)),
                    v.int(i64::from(l.fold.pd)),
                    v.int(i64::try_from(l.fold.since_bp).unwrap_or(0)),
                    lang,
                    packed,
                    v.int(l.prev_graf),
                    v.list(l.extra.clone()),
                    v.int(i64::from(l.gid)),
                    v.int(i64::try_from(l.save_base).unwrap_or(0)),
                    v.int(i64::try_from(l.def_base).unwrap_or(0)),
                    v.int(i64::from_ne_bytes(l.open_stamp.to_ne_bytes())),
                    toks_value(v, &l.ag),
                ])
            })
            .collect();
        let conds = v.list(self.conds.iter().map(|&c| v.int(i64::from(c))).collect());
        let codes = v.list(self.if_codes.iter().map(|&c| v.int(i64::from(c))).collect());
        // the state table's groups
        let open: Vec<u32> = self
            .nest
            .iter()
            .filter(|l| l.group != 0)
            .map(|l| l.gid)
            .collect();
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(&name));
            eprintln!(
                "cont {} at a cut: {name}: {} (open groups {open:?})",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        let c = self.st.cont(v, &open);
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(&name));
            let hit = c.slots.iter().find(|s| s.loc == l);
            eprintln!(
                "cont {} encodes {name}: {:?}",
                self.cx.node().raw(),
                hit.map(|s| (s.cur.map(|x| v.render(x)), s.loaded, s.save_gid, s.ldef, s.group_local))
            );
        }
        let mut slots = Vec::new();
        let mut overrides = Vec::new();
        let mut locals = Vec::new();
        for s in &c.slots {
            let f = self.loc_field(s.loc);
            if s.group_local {
                let base = match s.base {
                    Some(x) => x,
                    None => self.eng_read(f)?,
                };
                overrides.push(v.list(vec![f.0, opt_val(base)]));
                locals.push(v.list(vec![f.0, opt_val(s.cur)]));
            }
            let (stamp, local, gid) = s.ldef.map_or((-1, 0, None), |(st, l, g)| {
                (i64::from_ne_bytes(st.to_ne_bytes()), i64::from(l), g)
            });
            slots.push(v.list(vec![
                f.0,
                opt_val(s.cur),
                v.int(i64::from(s.loaded)),
                v.int(i64::from(s.save_gid)),
                v.int(i64::from_ne_bytes(s.save_stamp.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.ginert.to_ne_bytes())),
                v.int(stamp),
                v.int(local),
                gid_int(v, gid),
            ]));
        }
        let mut saves = Vec::new();
        for s in &c.save {
            let f = self.loc_field(s.loc);
            saves.push(v.list(vec![
                f.0,
                opt_val(s.prev),
                v.int(i64::from(s.prev_loaded)),
                v.int(i64::from(s.gid)),
                v.int(i64::from_ne_bytes(s.stamp.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.at.to_ne_bytes())),
                v.int(i64::from_ne_bytes(s.ldef.0.to_ne_bytes())),
                v.int(i64::from(s.ldef.1)),
                gid_int(v, s.ldef.2),
            ]));
        }
        let mut defined = Vec::new();
        for &l in &c.defined {
            defined.push(self.loc_field(l).0);
        }
        let state = v.list(vec![
            v.list(slots),
            v.list(saves),
            v.list(defined),
            v.int(i64::from_ne_bytes(c.stamp.to_ne_bytes())),
            v.int(i64::from(c.next_gid)),
        ]);
        let misc = v.list(vec![
            v.int(self.next_lid),
            v.int(self.last_par.0),
            v.int(self.last_par.1),
            v.int(self.lines_entered),
            v.int(i64::from_ne_bytes(self.lines_read.to_ne_bytes())),
            v.int(i64::from(self.eof)),
            v.int(i64::from(self.radix)),
            v.int(self.cur_order as i64),
            v.int(i64::try_from(self.pdf_occ).unwrap_or(0)),
            v.int(i64::from_ne_bytes(self.irreversible.to_ne_bytes())),
            v.int(i64::from(self.read_back_done)),
            v.int(i64::from(self.astate.is_some())),
            v.int(self.astate.unwrap_or(0)),
            v.int(self.aopen),
            v.int(i64::from(self.claim_rest)),
            v.int(i64::from(self.hint_void)),
            v.int(i64::from(self.no_pull)),
            v.int(i64::from(self.rec_load)),
        ]);
        let cont = v.list(vec![
            v.list(frames),
            v.list(nest),
            conds,
            codes,
            state,
            misc,
            v.list(overrides),
        ]);
        Ok((cont, locals))
    }

    /// A resume piece's start: the continuation `x` ([`Run::cont_encode`])
    /// loaded; the base-level values the open groups' bindings hide
    /// checked against the engine's reaching here (a global definition
    /// between the cut and here, the finisher's, replaces the binding and
    /// makes the groups' saves inert, §283).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn cont_load(&mut self, x: ValueId) -> R<()> {
        let v = self.v;
        let xs = list_of(v, x);
        let a = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
        self.input.text = crate::run::decode_segs(v, &[a(0)]);
        self.input.text_at = 0;
        // the state table first: the levels index its saves
        let st = list_of(v, a(4));
        let sa = |i: usize| st.get(i).copied().unwrap_or(ValueId::UNIT);
        let u = |x: ValueId| u64::from_ne_bytes(int_of(v, x).to_ne_bytes());
        let mut slots = Vec::new();
        for e in list_of(v, sa(0)) {
            let ys = list_of(v, e);
            if ys.len() < 9 {
                continue;
            }
            let loc = self.loc_of(Field(ys[0]));
            let stamp = int_of(v, ys[6]);
            slots.push(ContSlot {
                loc,
                cur: some(ys[1]),
                loaded: int_of(v, ys[2]) != 0,
                save_gid: u32::try_from(int_of(v, ys[3])).unwrap_or(0),
                save_stamp: u(ys[4]),
                ginert: u(ys[5]),
                ldef: (stamp >= 0).then(|| {
                    (
                        u64::from_ne_bytes(stamp.to_ne_bytes()),
                        int_of(v, ys[7]) != 0,
                        int_gid(int_of(v, ys[8])),
                    )
                }),
                base: None,
                group_local: false,
            });
        }
        let mut save = Vec::new();
        for e in list_of(v, sa(1)) {
            let ys = list_of(v, e);
            if ys.len() < 6 {
                continue;
            }
            let loc = self.loc_of(Field(ys[0]));
            save.push(ContSaved {
                loc,
                prev: some(ys[1]),
                prev_loaded: int_of(v, ys[2]) != 0,
                gid: u32::try_from(int_of(v, ys[3])).unwrap_or(0),
                stamp: u(ys[4]),
                at: u(ys[5]),
                ldef: if ys.len() >= 9 {
                    (u(ys[6]), int_of(v, ys[7]) != 0, int_gid(int_of(v, ys[8])))
                } else {
                    (0, false, None)
                },
            });
        }
        let mut defined = Vec::new();
        for f in list_of(v, sa(2)) {
            defined.push(self.loc_of(Field(f)));
        }
        // (the locations above were computed in the run's first state:
        // a field past the static cells has its location there, kept)
        let mut ns = State::from_cont(ContState {
            slots,
            save,
            defined,
            stamp: u(sa(3)),
            next_gid: u32::try_from(int_of(v, sa(4))).unwrap_or(0),
        });
        ns.adopt_spill(&self.st);
        self.st = ns;
        if let Some(name) = crate::exec::env_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(&name));
            eprintln!(
                "cont {} loaded: {name}: {}",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        // the fields the finishers between the cut and this resume
        // defined globally (`gdefs`, the window after the cutting node)
        let gdefs: Vec<ValueId> = match self.cx.args().get(2).copied() {
            Some(p) if self.cx.op() == self.ops.resume => {
                let pid = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, p).to_ne_bytes()));
                let acc = self.cx.acc_range(v.field("gdefs"), Some(pid))?;
                v.seq_to_vec(acc).into_iter().flat_map(|e| list_of(v, e)).collect()
            }
            _ => Vec::new(),
        };
        for e in list_of(v, a(6)) {
            let ys = list_of(v, e);
            if ys.len() < 2 {
                continue;
            }
            let f = Field(ys[0]);
            let x0 = some(ys[1]);
            let x1 = self.eng_read(f)?;
            // (§279: a global definition replaces the group's local value
            // whatever it defines, the value outside the group too: by
            // the definition, not the value)
            if gdefs.contains(&f.0) {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
                continue;
            }
            if crate::exec::env_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: resume {} override check {}: at the cut {:?}, now {:?}",
                    self.cx.node().raw(),
                    str_of(v, f.0),
                    x0.map(|x| v.render(x)),
                    x1.map(|x| v.render(x))
                );
            }
            if x1 != x0 {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
            }
        }
        for e in list_of(v, a(1)) {
            let ys = list_of(v, e);
            if ys.len() < 17 {
                continue;
            }
            let n = |i: usize| int_of(v, ys[i]);
            let mut l = crate::nest::Level::new(
                i32::try_from(n(0)).unwrap_or(1),
                n(1),
                n(2),
                n(3) != 0,
                list_of(v, ys[11]),
            );
            l.list = crate::nest::of_values(list_of(v, ys[4]));
            l.fold = FoldState {
                sf: i32::try_from(n(5)).unwrap_or(1000),
                pd: i32::try_from(n(6)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(7)).unwrap_or(0),
            };
            l.par = (ys[8] != ValueId::UNIT).then(|| (n(8), n(9)));
            l.prev_graf = n(10);
            l.gid = u32::try_from(n(12)).unwrap_or(0);
            l.save_base = usize::try_from(n(13)).unwrap_or(0);
            l.def_base = usize::try_from(n(14)).unwrap_or(0);
            l.open_stamp = u(ys[15]);
            l.ag = toks_of(v, Some(ys[16]));
            self.nest.push(l);
        }
        self.conds = list_of(v, a(2))
            .iter()
            .map(|&c| u8::try_from(int_of(v, c)).unwrap_or(0))
            .collect();
        self.if_codes = list_of(v, a(3))
            .iter()
            .map(|&c| i32::try_from(int_of(v, c)).unwrap_or(0))
            .collect();
        let m = list_of(v, a(5));
        let n = |i: usize| m.get(i).map_or(0, |&y| int_of(v, y));
        self.next_lid = n(0);
        self.last_par = (n(1), n(2));
        self.lines_entered = n(3);
        self.lines_read = u64::from_ne_bytes(n(4).to_ne_bytes());
        self.eof = n(5) != 0;
        self.radix = i32::try_from(n(6)).unwrap_or(0);
        self.cur_order = crate::val::order(n(7));
        self.pdf_occ = usize::try_from(n(8)).unwrap_or(0);
        self.irreversible = u64::from_ne_bytes(n(9).to_ne_bytes());
        self.read_back_done = n(10) != 0;
        self.astate = (n(11) != 0).then(|| n(12));
        self.aopen = n(13);
        self.claim_rest = n(14) != 0;
        self.hint_void = n(15) != 0;
        self.no_pull = n(16) != 0;
        self.rec_load = n(17) != 0;
        Ok(())
    }
}

// -------------------------------------------------------- operations

/// A page's finisher: the output routine for one page (§1012–§1026)
/// where the page finishes. Arguments `[j, data, carried, locals]`: the
/// page's index, its material (`\box255`, the insertion boxes, the
/// marks, the held-over insertions, `\outputpenalty`), what the fold
/// carries past it, and the bindings local to the groups open at the
/// cut (the routine runs inside them). A finisher placed after the
/// piece its page finished inside of has five more: `[rest, piece,
/// k, reads, defs]`, the piece's items after the candidate and what
/// its check needs ([`Run::check_cut`]). It reads what reaches its
/// place as any node does and defines what the routine defines; its
/// page is shipped from here.
pub(crate) struct Finish(pub Shared);

/// What the finishers of a build read and defined (`Ops::fin_touch`).
#[derive(Default)]
pub(crate) struct FinTouch {
    pub reads: phitex_defuse::FxHashSet<Field>,
    pub defs: phitex_defuse::FxHashSet<Field>,
}

/// What the last two finishers running one routine defined
/// (`Ops::fin_defs`): the earlier run's definitions, then the last's.
#[derive(Clone, Debug, Default)]
pub(crate) struct FinDefs {
    pub prev: Vec<(Field, ValueId)>,
    pub last: Vec<(Field, ValueId)>,
    /// The pages of the runs `prev` and `last` were.
    pub prev_j: i64,
    pub last_j: i64,
    /// Every field a run of the routine defined so far: a routine
    /// defines some on some pages only (a float page, a page with a
    /// list continued), not on the last two.
    pub seen: Vec<Field>,
}

impl FinDefs {
    /// The fields any run of the routine defined.
    fn names(&self) -> Vec<Field> {
        self.seen.clone()
    }

    /// The definitions both runs made with the same value: stable
    /// across pages, so a reader may run on them; a page number or a
    /// mark, changing every page, is not predicted (a wrong prediction
    /// runs its readers again).
    fn stable(&self) -> Vec<(Field, ValueId)> {
        self.last
            .iter()
            .filter(|d| self.prev.contains(d))
            .copied()
            .collect()
    }

    /// [`FinDefs::stable`] for the finisher of page `j`, with the
    /// integers that step uniformly by page (`\count0`, a page
    /// counter) carried on to page `j` (spec-predicted-breaks §3).
    fn predict(&self, v: &Values, j: i64) -> Vec<(Field, ValueId)> {
        let dj = self.last_j - self.prev_j;
        self.last
            .iter()
            .filter_map(|&(f, x)| {
                // (the marks are the page's text: equal on two pages by
                // chance; the scan knows them, spec §3)
                let name = crate::val::field_name(v, f);
                if name.starts_with("mark") || name.starts_with("cs:g__mark_") || name == "param:outputpenalty" {
                    return None;
                }
                let &(_, y) = self.prev.iter().find(|d| d.0 == f)?;
                if x == y {
                    return Some((f, x));
                }
                // (registers only: a page counter; not a hash, a penalty)
                if !name.starts_with("count:") {
                    return None;
                }
                let (a, b) = (v.as_int(y)?, v.as_int(x)?);
                let d = b.checked_sub(a)?;
                if dj <= 0 || d % dj != 0 {
                    return None;
                }
                let x = (d / dj).checked_mul(j - self.last_j)?.checked_add(b)?;
                Some((f, v.int(x)))
            })
            .collect()
    }
}

/// What a finisher defines besides its routine's definitions
/// (`fire_page`, `output_end`, `ship_out`): field prefixes.
const FINISHER_FIELDS: &[&str] = &[
    SHIPPED,
    "deadcycles",
    "mark:",
    "param:outputpenalty",
    "inspen",
    "box:",
    "badness",
    "write:",
    "pdf:",
];

/// A finisher's entry in `Ops::fin_defs`: its routine (the hint `t`)
/// and whether its page is an ordinary one or one fired at a penalty
/// of -10001 or less (`\outputpenalty` is in the page data): such a
/// routine run (LaTeX's float and `\clearpage` handling) defines other
/// things and ships nothing; predicted from runs of its own kind.
fn fin_key(v: &Values, t: ValueId, args: &[ValueId]) -> u64 {
    let special = crate::exec::env_flag!("PHITEX_FIN_KIND") && args.get(11).is_some_and(|&x| v.as_int(x) == Some(1));
    t.raw() ^ u64::from(special).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// Finishers created with a prediction of their definitions, and
/// without (reported with `PHITEX_TEX_TIME`).
pub(crate) static FIN_PREDICTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static FIN_UNPREDICTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Finish {
    fn predicted_defs_(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let &t = args.get(9)?;
        if t == ValueId::UNIT || crate::exec::env_flag!("PHITEX_NO_FIN_PREDICT") {
            return None;
        }
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let stable = ops(&self.0)
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .map(|d| if fold_chain() || crate::exec::env_flag!("PHITEX_NO_STEP") { d.stable() } else { d.predict(v, j) })?;
        (!stable.is_empty()).then_some(stable)
    }
}

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    /// The output routine's hint (encoded, the tenth argument:
    /// [`crate::hints::routine_hint`]) and the fields a finisher defines
    /// itself; anything where the routine's text cannot be bounded, or
    /// the hint is not given.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let Some(&t) = args.get(9) else {
            return MayDefine::Any;
        };
        if t == ValueId::UNIT || crate::exec::env_flag!("PHITEX_HINTS_OFF") {
            return MayDefine::Any;
        }
        // (a finisher ran this routine before: what it defined then,
        // by name, and the fields a finisher sets itself; a definition
        // outside them refutes the declaration, as any node's does, and
        // only its readers run again)
        let known = ops(&self.0)
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .map(FinDefs::names);
        if let Some(mut names) = known
            && !crate::exec::env_flag!("PHITEX_NO_FIN_PREDICT")
        {
            let chain = if fold_chain() { PS } else { RTN };
            for n in [chain, SHIPPED, "deadcycles", "param:outputpenalty", "inspen", "badness", "box:255"] {
                names.push(v.field(n));
            }
            ops(&self.0).fields.add_record_fields(&mut names);
            return MayDefine::Names(names);
        }
        let h = crate::hints::Hint::decode(&str_of(v, t));
        let mut ps: Vec<String> = match h.may_define() {
            MayDefine::Any => return MayDefine::Any,
            MayDefine::Prefixes(ps) => ps,
            MayDefine::Names(ns) => ns.iter().map(|&f| crate::val::field_name(v, f)).collect(),
            MayDefine::Nothing => Vec::new(),
        };
        ps.extend(FINISHER_FIELDS.iter().map(|&s| s.to_owned()));
        if fold_chain() {
            ps.push(PS.to_owned());
            ps.push(crate::build::VTAIL.to_owned());
        } else {
            ps.push(RTN.to_owned());
        }
        crate::fields::add_record_prefixes(&mut ps);
        MayDefine::Prefixes(ps)
    }
    /// What the last finisher running the same routine defined, with
    /// its values: the text after this one runs on them, and runs again
    /// only where a value differs (the page number's readers). The page
    /// state and the shipped mark are left out: the chain reads them.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.predicted_defs_(args, v);
        let c = if r.is_some() { &FIN_PREDICTED } else { &FIN_UNPREDICTED };
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let t_fin = std::time::Instant::now();
        let _ = phitex_pdf::phase::take();
        let args = cx.args();
        let o = ops(&self.0);
        let out = run_node_parking(cx, o, true, |r| {
            if crate::exec::env_flag!("PHITEX_FIN_TRACE") {
                let j = args.first().map_or(0, |&x| int_of(r.v, x));
                eprintln!("fin-trace: node {} page {j} spec {} resumed {}", r.cx.node().raw(), r.cx.is_speculative(), r.resumed);
            }
            if !r.resumed {
                let v = r.v;
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                r.fin = Some(FinCtx {
                    j: int_of(v, a(0)),
                    carried: list_of(v, a(2)),
                    pending: Vec::new(),
                    rest: list_of(v, a(4)),
                    held_n: 0,
                    rtn: None,
                });
                for e in list_of(v, a(3)) {
                    let ys = list_of(v, e);
                    if let [f, x] = ys[..] {
                        let loc = r.loc_of(Field(f));
                        r.st.preload(loc, some(x));
                    }
                }
                if let Some(names) = crate::exec::env_str!("PHITEX_FIN_SHOW") {
                    let mut shown = Vec::new();
                    for n in names.split(',') {
                        let x = r.get(n)?;
                        shown.push(format!("{n}={}", x.map_or_else(|| "undefined".to_owned(), |x| crate::val::show_def(v, x))));
                    }
                    let locals: Vec<String> = list_of(v, a(3))
                        .iter()
                        .filter_map(|&e| list_of(v, e).first().map(|&f| str_of(v, f)))
                        .collect();
                    eprintln!(
                        "finisher {} page {} (piece {}, candidate {}, locals {:?}, args {}): {}",
                        r.cx.node().raw(),
                        int_of(v, a(0)),
                        v.render(a(5)),
                        v.render(a(6)),
                        locals,
                        args.len(),
                        shown.join(", ")
                    );
                }
                r.fire_page(a(1))?;
            }
            r.run_all()?;
            let olds = r.fin_olds()?;
            r.check_cut(&args)?;
            r.record_finisher_defs(&args, &olds);
            r.contribute_global_defs();
            Ok(())
        });
        if phitex_pdf::phase::on() {
            phitex_pdf::phase::add(0, t_fin);
            let a = phitex_pdf::phase::take();
            let us: Vec<String> = a.iter().map(|&n| format!("{:.1}", n as f64 / 1000.0)).collect();
            eprintln!("finphase: {}", us.join(" "));
        }
        out
    }
}

impl Run<'_, '_> {
    /// The fields this finisher defined globally, for the resume after
    /// it ([`Run::cont_load`]): a group-local value the resume carries
    /// is replaced by a global definition even of an equal value.
    fn contribute_global_defs(&mut self) {
        let v = self.v;
        let names: Vec<ValueId> = self
            .st
            .published(v, self.rec_load)
            .into_iter()
            .filter(|d| d.2 == Locality::Global)
            .map(|d| self.loc_field(d.0).0)
            .collect();
        if !names.is_empty() {
            self.contribute("gdefs", v.seq_unit(v.list(names)));
        }
    }

    /// The values reaching this finisher of the fields its routine
    /// defined globally without reading them first (`\global\vsize=`):
    /// read before the commit, for [`Run::record_finisher_defs`].
    fn fin_olds(&mut self) -> R<Vec<(Loc, Option<ValueId>)>> {
        let v = self.v;
        let mut out = Vec::new();
        for (loc, _, l) in self.st.published(v, self.rec_load) {
            if l != Locality::Global || self.st.engine_value(loc).is_some() {
                continue;
            }
            let f = self.loc_field(loc);
            let name = crate::val::field_name(v, f);
            // (what the finisher sets itself, `fire_page` and `ship_out`,
            // counts as changed unread: reading it would make this
            // finisher wait on the last one's value)
            if is_page_field(&name) || FINISHER_FIELDS.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            out.push((loc, self.eng_read(f)?));
        }
        Ok(out)
    }

    /// The definitions this finisher publishes, kept for the next one
    /// running the same routine ([`Finish::predicted_defs`]), and the
    /// fields it read and whose value it changed, for the pieces'
    /// own-fold decision ([`Run::needs_own_fold`]): a global definition
    /// giving the value that reached the finisher changes nothing a piece
    /// sees (LaTeX's `\shipout` sets `\afterassignment` and its
    /// `\setbox` consumes it; the routine sets `\vsize` to the value it
    /// has).
    fn record_finisher_defs(&mut self, args: &[ValueId], olds: &[(Loc, Option<ValueId>)]) {
        let v = self.v;
        {
            let mut reads: Vec<Field> = self
                .st
                .read_locs()
                .into_iter()
                .map(|l| self.loc_field(l))
                .collect();
            reads.extend(self.streams_since(0));
            let mut gdefs: Vec<Field> = Vec::new();
            for (loc, x, l) in self.st.published(v, self.rec_load) {
                if l != Locality::Global {
                    continue;
                }
                let f = self.loc_field(loc);
                if is_page_field(&crate::val::field_name(v, f)) {
                    continue;
                }
                let old = match self.st.engine_value(loc) {
                    Some(e) => Some(e),
                    None => olds.iter().find(|o| o.0 == loc).map(|o| o.1),
                };
                if old != Some(Some(x)) {
                    gdefs.push(f);
                }
            }
            let mut t = self
                .ops
                .fin_touch
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            t.reads.extend(reads);
            t.defs.extend(gdefs);
        }
        let Some(&t) = args.get(9) else { return };
        // (a speculative run's definitions may be refuted: they would
        // seed the predictions of the finishers after it)
        if t == ValueId::UNIT || self.cx.is_speculative() {
            return;
        }
        let mut out = Vec::new();
        for (loc, x, _) in self.st.published(v, self.rec_load) {
            let f = self.loc_field(loc);
            let name = crate::val::field_name(v, f);
            if is_page_field(&name) {
                continue;
            }
            out.push((f, x));
        }
        if let Some(x) = self.fin.as_ref().and_then(|c| c.rtn) {
            out.push((v.field(RTN), x));
        }
        // (the ordinary pages fired up to this one: page counters step
        // by them)
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let mut m = self
            .ops
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = m.entry(fin_key(v, t, args)).or_default();
        for &(f, _) in &out {
            if !e.seen.contains(&f) {
                e.seen.push(f);
            }
        }
        if crate::exec::env_flag!("PHITEX_PRED_SHOW") {
            for (f, x) in e.predict(v, j) {
                let got = out.iter().find(|d| d.0 == f).map(|d| d.1);
                if got != Some(x) {
                    eprintln!(
                        "pred page {j}: {} predicted {} got {}",
                        crate::val::field_name(v, f),
                        v.render(x),
                        got.map_or_else(|| "nothing".to_owned(), |g| v.render(g))
                    );
                }
            }
        }
        // (the two latest pages: finishers commit in any order)
        if j >= e.last_j || e.last.is_empty() {
            e.prev = std::mem::take(&mut e.last);
            e.prev_j = e.last_j;
            e.last = out;
            e.last_j = j;
        } else if j > e.prev_j || e.prev.is_empty() {
            e.prev = out;
            e.prev_j = j;
        }
    }
}

/// A piece's fold ([`Run::fold_apart`]): arguments `[items, last, piece,
/// reads, defs]`, the piece's items for the main list, the candidate
/// nothing followed (or -1), the piece's node, and what a finisher's
/// check needs (unit when no page can finish inside the piece with
/// commands after it). It reads the page state reaching it, folds the
/// items in, defines the next version, and places the finisher where a
/// page finishes, the items after that candidate carried to it.
pub(crate) struct Fold(pub Shared);

impl Operation for Fold {
    fn name(&self) -> &'static str {
        "fold"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(vec![v.field(PS), v.field(crate::build::VTAIL), v.field(CONT)])
    }
    /// The page state and `\deadcycles`: the node waits for them
    /// instead of starting and stopping.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        // (not what a finisher defines, `deadcycles` and `rtn`: a fold
        // waiting for them would wait for the finisher; it reads them
        // as any read, on the finisher's prediction)
        if !fold_chain() {
            return Some(vec![v.field(PS), v.field(crate::build::VTAIL), v.field("pagepar")]);
        }
        Some(vec![
            v.field(PS),
            v.field(crate::build::VTAIL),
            v.field("deadcycles"),
            // (the page parameters' record)
            v.field("pagepar"),
        ])
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        run_node_parking(cx, o, true, |r| r.fold_node(&args))
    }
}

impl Run<'_, '_> {
    /// `\deadcycles` where the fold is, when the fold needs it: only
    /// `\end` reads it (§1054); any other fold reads nothing a finisher
    /// defines, so a finisher placed anew does not run the folds after
    /// it again.
    fn deadcycles_for(&mut self, raw: &[ValueId]) -> R<i64> {
        let v = self.v;
        if raw.iter().any(|&x| nodes::tag(v, x) == nodes::T_END) {
            return self.cint(crate::fields::cell::DEADCYCLES);
        }
        Ok(0)
    }

    /// The main list's tail after this fold's items (`vtail`,
    /// [`crate::build::vtail_value`]): read where the fold is, defined.
    fn define_vtail(&mut self, raw: &[ValueId]) -> R<()> {
        let v = self.v;
        let vt = v.field(crate::build::VTAIL);
        let prev = self.eng_read(vt)?;
        let x = crate::build::vtail_value(v, prev, raw);
        self.cx.define_with(vt, x, Locality::Global);
        Ok(())
    }

    /// A [`Fold`] node's run.
    fn fold_node(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let mut raw = list_of(v, a(0));
        let last = int_of(v, a(1));
        let cont = last == -2;
        let f = self.ps_field();
        // (a stop at the read parks the run: it resumes here)
        self.begin_command();
        let ps_in = self.eng_read(f)?;
        // a continuation: what the routine gave back (its markers, the
        // held-over insertions, its list), then the material carried
        // over, the page built again, the rest of the piece (§1026)
        let mut own = None;
        if cont {
            let c = self.eng_read(v.field(CONT))?;
            let cs = c.map(|x| list_of(v, x)).unwrap_or_default();
            let part = |i: usize| cs.get(i).map(|&x| list_of(v, x)).unwrap_or_default();
            let r = self.eng_read(v.field(RTN))?;
            let rs = r.map(|x| list_of(v, x)).unwrap_or_default();
            let markers = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
            raw = markers.clone();
            raw.extend(part(0));
            raw.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
            raw.extend(part(1));
            raw.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
            raw.extend(part(2));
            own = Some(markers);
        }
        // (the scan check: the piece's profile, its unit of `vprof`)
        let unit = if crate::vprof::checking() && !cont {
            let pid = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, a(2)).to_ne_bytes()));
            let acc = self.cx.acc_range_to(v.field(crate::vprof::VPROF), None, Some(pid))?;
            v.seq_to_vec(acc).last().copied()
        } else {
            None
        };
        self.define_vtail(own.as_ref().unwrap_or(&raw))?;
        self.resolve_page_params(&mut raw)?;
        let dead = self.deadcycles_for(&raw)?;
        let mut timer = FoldTimer::new(raw.len());
        let cache = self.ops.page_states.clone();
        let mut ps = timer.decode(|| {
            ps_in
                .and_then(|x| cache.take(x))
                .unwrap_or_else(|| PageState::decode(v, ps_in))
        });
        let scan_in = crate::vprof::checking().then(|| crate::vprof::ScanState::of(v, &ps));
        let fire = {
            let mut live = |n: u16| self.insert_class(n);
            timer.fold(|| ps.fold_items(v, &raw, dead, &mut live))?
        };
        if let Some(scan) = scan_in {
            let items = match unit {
                Some(u) => crate::vprof::decode_value(v, u),
                None if cont => crate::vprof::decode(&crate::vprof::encode(v, &raw)),
                None => {
                    eprintln!("scan-check: fold node {}: no profile unit", self.cx.node().raw());
                    Vec::new()
                }
            };
            self.scan_check(scan, &items, dead, &mut ps, fire.as_ref());
        }
        let Some(fire) = fire else {
            let e = timer.encode(|| ps.encode(v));
            self.cx.define_with(f, e, Locality::Global);
            cache.put(e, ps);
            return Ok(());
        };
        let at_end = cont || i64::from(fire.k) == last;
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            eprintln!(
                "  fold {}: page {} fires at candidate {} (at end {at_end}, piece {})",
                self.cx.node().raw(),
                ps.j,
                fire.k,
                int_of(v, a(2))
            );
        }
        let e = ps.encode(v);
        self.cx.define_with(f, e, Locality::Global);
        let j = ps.j - 1;
        let normal = ps.normal;
        cache.put(e, ps);
        let mut fargs = vec![v.int(j), fire.data, v.list(if fold_chain() { fire.carried.clone() } else { Vec::new() }), v.list(Vec::new())];
        if at_end {
            fargs.extend([ValueId::UNIT; 5]);
        } else {
            let defs_k = list_of(v, a(4))
                .get(usize::try_from(fire.k).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(ValueId::UNIT);
            fargs.extend([
                v.list(if fold_chain() { fire.rest.clone() } else { Vec::new() }),
                a(2),
                v.int(i64::from(fire.k)),
                a(3),
                defs_k,
            ]);
        }
        fargs.push(self.output_hint()?);
        fargs.push(v.int(normal));
        fargs.push(v.int(i64::from(fire_special(v, fire.data))));
        self.cx.follow(self.ops.finish, fargs);
        let rest = if at_end && !cont { Vec::new() } else { fire.rest };
        self.place_continuation(fire.data, fire.carried, rest);
        Ok(())
    }

    /// The hint of the output routine in force (its tokens shown as
    /// source, read under the format's definitions), encoded: the
    /// finisher's bound ([`Finish`]). Unit for the default routine.
    fn output_hint(&mut self) -> R<ValueId> {
        let v = self.v;
        let out = self.par(Par::output)?;
        let toks = toks_of(v, out);
        if toks.is_empty() {
            return Ok(ValueId::UNIT);
        }
        let mut text = String::new();
        self.show_toks_into(&mut text, &toks)?;
        let h = {
            let defs = self
                .ops
                .defs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::hints::routine_hint(&text, &defs)
        };
        Ok(v.str(&h.encode()))
    }
}

/// The rest of a piece after a cut: arguments `[cont, flags]`, the
/// continuation ([`Run::cont_encode`]) and the piece's text kind. It
/// runs the input left in the groups left open, as the piece would have
/// gone on, reading what the finisher before it defined.
pub(crate) struct Resume(pub Shared);

impl Operation for Resume {
    fn name(&self) -> &'static str {
        "resume"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        MayDefine::Any
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }
    fn claim(
        &self,
        _: &[ValueId],
        _: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        Claim::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        let t0 = std::time::Instant::now();
        let out = run_node_parking(cx, o, true, |r| {
            if !r.resumed {
                r.cont_load(args.first().copied().unwrap_or(ValueId::UNIT))?;
                r.read_cut_request()?;
            }
            r.run_all()
        });
        if crate::exec::env_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            let v = cx.values();
            let text = v.render(args.first().copied().unwrap_or(ValueId::UNIT));
            let cut: String = text.chars().take(600).collect();
            eprintln!("longrun: resume {} {:.0} ms: {cut}", cx.node().raw(), t0.elapsed().as_secs_f64() * 1e3);
        }
        out
    }
}

/// (measure) `PHITEX_TEX_TIME`: one page fold's decode, fold and encode
/// time and its item count, printed as `pagefold:` when the fold ends
/// (a fold that fires prints no encode: the finisher defines the state).
struct FoldTimer {
    on: bool,
    items: usize,
    t: [std::time::Duration; 3],
}

impl FoldTimer {
    fn new(items: usize) -> FoldTimer {
        FoldTimer {
            on: crate::exec::env_flag!("PHITEX_TEX_TIME"),
            items,
            t: [std::time::Duration::ZERO; 3],
        }
    }

    fn timed<T>(&mut self, i: usize, f: impl FnOnce() -> T) -> T {
        if !self.on {
            return f();
        }
        let t0 = std::time::Instant::now();
        let r = f();
        self.t[i] += t0.elapsed();
        r
    }

    fn decode<T>(&mut self, f: impl FnOnce() -> T) -> T {
        self.timed(0, f)
    }

    fn fold<T>(&mut self, f: impl FnOnce() -> T) -> T {
        self.timed(1, f)
    }

    fn encode<T>(&mut self, f: impl FnOnce() -> T) -> T {
        self.timed(2, f)
    }
}

impl Drop for FoldTimer {
    fn drop(&mut self) {
        if self.on {
            eprintln!(
                "pagefold: items {} decode_us {} fold_us {} encode_us {}",
                self.items,
                self.t[0].as_micros(),
                self.t[1].as_micros(),
                self.t[2].as_micros()
            );
        }
    }
}

impl Run<'_, '_> {
    /// The scan over `items` from `scan` compared with the real fold
    /// that left `ps` and `fire` ([`crate::vprof::check`]).
    fn scan_check(
        &mut self,
        scan: crate::vprof::ScanState,
        items: &[crate::vprof::PItem],
        dead: i64,
        ps: &mut PageState,
        fire: Option<&Fire>,
    ) {
        let v = self.v;
        let real = crate::vprof::RealOutcome {
            fire: fire.map(|f| (f.k, ps.fired.take().expect("decisions of the page fired"), f.rest.len())),
            after: crate::vprof::snapshot(
                ps.j,
                &ps.pb,
                ps.fold,
                ps.params.map(|m| params_of(v, m)),
                ps.pending.len(),
            ),
        };
        let node = self.cx.node().raw();
        let mut rd = RunReaching(self);
        crate::vprof::check(node, scan, items, dead, &mut rd, &real);
    }
}

/// What a scan reads where a fold runs: the fold's run.
struct RunReaching<'r, 'c, 'a>(&'r mut Run<'c, 'a>);

impl crate::vprof::Reaching for RunReaching<'_, '_, '_> {
    fn params(&mut self) -> PageParams {
        let v = self.0.v;
        self.0.page_params_marker().map_or(
            PageParams {
                vsize: 0,
                max_depth: 0,
                top_skip: Glue::ZERO,
                holding_inserts: 0,
                saving_vdiscards: false,
            },
            |m| params_of(v, m),
        )
    }

    fn class(&mut self, n: u16) -> InsertClass {
        self.0.insert_class(n).unwrap_or(InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        })
    }
}

/// A build's page states handed from the fold that defines one to the
/// fold that reads it ([`Run::page_fold`]): keyed by the value, so the
/// reader takes exactly the state that value encodes and skips the
/// decode; a reader that finds nothing (taken already, or never put)
/// decodes the value.
#[derive(Default)]
pub(crate) struct PageStates(std::sync::Mutex<phitex_defuse::FxHashMap<ValueId, PageState>>);

impl PageStates {
    /// At most this many states wait to be taken (a state no fold
    /// takes, after a rerun, is dropped with the rest).
    const MAX: usize = 256;

    fn take(&self, x: ValueId) -> Option<PageState> {
        self.0.lock().ok()?.remove(&x)
    }

    fn put(&self, x: ValueId, ps: PageState) {
        if let Ok(mut m) = self.0.lock() {
            if m.len() >= Self::MAX {
                m.clear();
            }
            m.insert(x, ps);
        }
    }
}
