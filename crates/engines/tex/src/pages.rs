//! Pages: where the document's pages finish, and their output routines,
//! as nodes of the graph; every page value moves by def-use
//! (docs/spec-predicted-breaks.md, S2).
//!
//! - A text piece (an `exec` node, or a `resume` piece) typesets in
//!   parallel with every other, and never reads the page. The items it
//!   puts on the main vertical list are its own contributions, delimited
//!   by candidate markers at the points where tex.web calls `build_page`
//!   (§994; [`Run::finish_candidate`]). At its end it defines what it
//!   leaves the next piece of the main list (the previous depth, the
//!   space factor, the last item: [`crate::build::MAIN_LAST`], read from
//!   the piece before only where its items leave it standing), gives its vertical
//!   profile and its items as units of two accumulators
//!   ([`crate::vprof::VPROF`], [`PITEMS`]) and places its scan step
//!   ([`Scan`]) after it.
//! - The scan is TeX's page builder (§980–§1010) over the profiles, one
//!   node a block of pieces ([`block_end`], [`Run::scan_step`]): from
//!   the scan state ([`SCAN`]) where its block begins, defined by the
//!   continuation of the page before, it folds the profiles; for each
//!   page firing in them it places the page's finisher ([`Finish`]) and
//!   continuation right after the piece the page fires in, its break
//!   ([`Run::place_break`]: its range and candidate, no items) their
//!   argument, and goes on from the continuation's state (its run's, or
//!   its prediction: the finisher's routine giving back what it is
//!   predicted to). The finisher and the continuation assemble the page
//!   from the pieces' items by the break's range with the same builder
//!   ([`Run::assemble_page`], [`PageState::fold_items`]: `\box255`, the
//!   insertions, the marks), so an edit inside a page that moves no
//!   break runs no scan.
//! - A finisher runs the output routine (§1012–§1026) as one run of its
//!   own on its page, defines what the routine defines for everything
//!   after it, and what it gives back to the main list ([`RTN`]); the
//!   continuation after it reads that on the finisher's prediction, so
//!   the scan does not wait for the routine.
//! - A piece ends at every page-builder candidate it can end at (inside
//!   a group or conditional of its own only where the page is made to
//!   fire, which the rest after the routine reads): its continuation
//!   ([`Run::cont_encode`]) is the material of a `resume` piece
//!   ([`Resume`], [`RESUME`]) placed after it (after the finisher of a
//!   page firing there), which reads the state there forward and what
//!   the page's finisher leaves: a cut is a forward definition, never a
//!   loop-back. A finisher whose page fires at a candidate inside a
//!   piece checks its routine against what the piece read and defined
//!   after it ([`Run::check_cut`]). The piece's PDF
//!   records after the candidate go after the page in the line
//!   (`crate::pdfout::order_line`).
//! - Nothing reads the whole list, nothing is replayed: one scan state
//!   per page, one finisher per page, a step per piece reading only the
//!   profiles since its page began.

use phitex_defuse::{
    Claim, Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{Contents, Fired, InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::{FoldState, IGNORE_DEPTH, Mode, fold_item};
use crate::fields::Par;
use crate::input::{Line, Seg};
use crate::nodes::{self, Side};
use crate::ops::{Shared, ops, run_node_parking};
use crate::run::{Halt, R, Run};
use crate::runstate::{ContSaved, ContSlot, ContState, Loc, State};
use crate::val::{
    glue_of, glue_value, group, int_at, int_of, list_of, str_of, toks_of, toks_value,
};

/// What a finisher's output routine gives back to the main list
/// (§1026): `[markers, list]`, the run's own main-list items (page
/// parameter markers) and the routine's list, without the held-over
/// insertions (the fold continuation after the finisher has them).
/// Read by that continuation; predicted (a routine that gives nothing
/// back gives the same value every page).
pub(crate) const RTN: &str = "rtn";

/// What page `j`'s routine gives back, as its finisher defines it beside
/// [`RTN`]: what page `j`'s continuation reads (a field of its own per
/// page, so the read does not hang on the order of the finisher and the
/// continuation placed after the same piece).
fn rtn_field(v: &Values, j: i64) -> Field {
    v.field(&format!("rtn:{j}"))
}

/// The pieces' main-list items, one unit per piece beside its profile:
/// `[piece, last, items, reads, defs, locals]` (its raw node id, its
/// last candidate when nothing came after it, its items with their
/// markers resolved, what a finisher's check needs, [`Run::check_cut`],
/// and for a piece cut at its last candidate the bindings local to the
/// groups open there, which the routine sees). Read by range where a
/// page fires: the page's real items.
pub(crate) const PITEMS: &str = "pitems";

/// A cut's continuation, `[cont, cutting node, text kind, claim guess,
/// candidate]`
/// ([`Run::cont_encode`]): defined by the piece that cuts, read by the
/// resume the piece places after its scan step ([`Run::fold_here`]). A
/// field, not the resume's arguments: the input left
/// changes with any edit before the document's end, and a resume made
/// anew re-makes everything after it (spec-predicted-breaks §7).
pub(crate) const RESUME: &str = "pages:resume";

/// The fields a finisher defined globally, one unit per finisher: read
/// by range by the resume after a cut (a global definition replaces
/// the group's local value, §279). A finisher declares its unit as the
/// last run of its routine gave it, a guess ([`Finish::static_contribs`]).
pub(crate) const GDEFS: &str = "gdefs";

/// The scan's state (spec-predicted-breaks §2, S2): defined where the
/// scan continues after a page fired ([`Scan`] after the finisher), read
/// by every scan step after it: `[anchor, j, normal, sf, pd, since_bp,
/// params, profile, material]`, the defining node (the steps read the
/// pieces' profiles after it), the pages fired, the ordinary ones, the
/// main list's fold where the page fired, the page parameters in force
/// there, and what the next page begins with (the routine's markers,
/// the held-over insertions, the routine's list, the material carried
/// over, the rest of the piece) as a profile and as items. No version
/// per piece: a step that fires no page defines nothing.
pub(crate) const SCAN: &str = "scan:state";

/// Where a block's scan ended its block ([`Run::scan_step`]): `[scan
/// state, material]` after its last page, `()` where a continuation of it
/// places the next page itself, `[BLOCK_AT_CONT, j, scan]` where its last
/// page `j`'s continuation sits after it (the page fired in the piece
/// that placed the scan) and defines the state the next block's scan
/// reads, by its page's field ([`scan_page_field`]); read by the next
/// block's scan.
pub(crate) const SCAN_BLOCK: &str = "scan:block";

/// Where the scan state reaching a block's scan stands among the pages
/// (`scan:at`): `[j, normal]`, the next page's index and the ordinary
/// pages fired before it. Not in the scan state ([`SCAN`],
/// [`SCAN_BLOCK`]): a page added before them changes these and nothing
/// a block's scan decides from, and a scan that fires no page reads
/// none; defined by a block's scan that fired pages (after its last),
/// or that began at a continuation's state ([`BLOCK_AT_CONT`]).
const SCAN_AT: &str = "scan:at";

/// [`SCAN_BLOCK`]'s mark for a block whose end is its last page's
/// continuation's.
const BLOCK_AT_CONT: i64 = -1;

/// Where page `j`'s continuation, placed by the block's scan `placer`,
/// leaves the scan (`scan:page:<j>:<placer>`): `[state, material,
/// break]`, defined by that continuation, or by the last of the
/// continuations the pages firing in its material place. The scan reads
/// it as an ordinary read and goes on from it for that break
/// (spec-final §4: a forward definition, checked within the evaluation;
/// a state equal to the one the scan ran on stops the wave there).
fn scan_page_field(v: &Values, j: i64, placer: i64) -> Field {
    v.field(&format!("scan:page:{j}:{}", placer.cast_unsigned()))
}

/// A continuation's page `(j, placer, break)` of [`scan_page_field`]
/// (arguments 5, 6 and 7): its own, placed by a block's scan, or the one
/// of the continuation whose material fired its page.
fn cont_chain(v: &Values, args: &[ValueId]) -> Option<(i64, i64, ValueId)> {
    if args.first().is_none_or(|&x| int_of(v, x) != -2) {
        return None;
    }
    let (Some(&by), Some(&j0), Some(&b0)) = (args.get(5), args.get(6), args.get(7)) else {
        return None;
    };
    Some((int_of(v, j0), int_of(v, by), b0))
}

/// What the next page begins with as items (the routine's markers, the
/// held-over insertions, its list, the material carried over, the rest
/// of the piece), defined beside [`SCAN`] by the same continuation and
/// read where a page is assembled ([`Run::assemble_page`], a piece's own
/// fold): the steps read the profile in [`SCAN`] only, so a change in
/// the items that leaves the profile alone wakes no step.
pub(crate) const SCAN_MAT: &str = "scan:material";

/// The material page `j` begins with, as the continuation of page
/// `j - 1` defines it beside [`SCAN_MAT`]: what page `j`'s finisher and
/// continuation read. A field of its own per page: what they read does
/// not hang on the order of the nodes placed after the piece the page
/// fires in (a finisher placed again after its page's continuation read
/// that continuation's material, and lost the page's first lines).
fn scan_mat_field(v: &Values, j: i64) -> Field {
    v.field(&format!("scan:mat:{j}"))
}

/// A continuation's page (its argument 1).
fn cont_page(v: &Values, args: &[ValueId]) -> Option<i64> {
    args.first()
        .is_some_and(|&x| int_of(v, x) == -2)
        .then(|| args.get(1).map(|&x| int_of(v, x)))
        .flatten()
}

/// The page nodes' declared contributions: none to the pieces' units
/// ([`crate::vprof::VPROF`], [`PITEMS`]), so a scan step's read of a
/// window, and its read hint's gate, do not wait for a scan step, a
/// finisher or a page's placement still to run, only for the pieces.
pub(crate) fn no_units(v: &Values) -> Vec<(Field, ValueId)> {
    vec![
        (v.field(crate::vprof::VPROF), ValueId::NIL),
        (v.field(PITEMS), ValueId::NIL),
    ]
}

/// [`no_units`] as a run gives them (a run contributing otherwise than
/// its text declared runs the windows' readers again).
pub(crate) fn give_no_units(r: &mut Run<'_, '_>) {
    for (f, x) in no_units(r.v) {
        r.muts += 1;
        r.contribs.push((f, x));
    }
}

/// The page nodes, by what their own code folds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageNode {
    /// A scan step after a piece ([`Run::scan_window`], and the page
    /// whose continuation it predicts, [`Run::cont_state`]).
    Step,
    /// A scan's continuation after a finisher ([`Run::cont_state`]).
    Cont,
    /// A finisher ([`Run::finisher_args`], [`Run::preload_candidate`]).
    Finish,
    /// The rest of a cut piece ([`Run::gdefs_after`], where it loads its
    /// continuation).
    Resume,
}

impl PageNode {
    /// The page node of op `op` with arguments `args`, if one.
    pub(crate) fn of(
        o: &crate::run::Ops,
        v: &Values,
        op: OpId,
        args: &[ValueId],
    ) -> Option<PageNode> {
        if op == o.scan {
            let cont = args.first().is_some_and(|&x| int_of(v, x) == -2);
            Some(if cont { PageNode::Cont } else { PageNode::Step })
        } else if op == o.finish {
            Some(PageNode::Finish)
        } else if op == o.resume {
            Some(PageNode::Resume)
        } else {
            None
        }
    }

    /// The accumulators the node's own code folds (reads by range): the
    /// pieces' profiles and items where it builds or assembles a page,
    /// the finishers' global definitions after a cut. Its read hint names
    /// each of them: the engine's gate holds the node while a contributor
    /// before it is in work, where its run would start, stop on the fold
    /// and run again. [`Run::fold_own`] folds only these (checked under
    /// `observe`). (What the commands of a text fold is the text's: its
    /// hint's, [`crate::hints::reads`].)
    pub(crate) const fn folds(self) -> &'static [&'static str] {
        match self {
            PageNode::Step | PageNode::Cont => &[crate::vprof::VPROF, PITEMS],
            PageNode::Finish => &[crate::vprof::VPROF, PITEMS, GDEFS],
            PageNode::Resume => &[GDEFS],
        }
    }

    /// The read hint: the fields `gate` the node cannot run without, and
    /// [`PageNode::folds`].
    fn reads(self, v: &Values, gate: &[&str]) -> Option<Vec<Field>> {
        Some(
            gate.iter()
                .copied()
                .chain(self.folds().iter().copied())
                .map(|f| v.field(f))
                .collect(),
        )
    }
}

impl Run<'_, '_> {
    /// The fold of accumulator `name` over the window `(from, to]` (up
    /// to this node where `to` is none), read by a page node's own code:
    /// one of the node's [`PageNode::folds`], which its read hint names.
    ///
    /// # Panics
    ///
    /// Under `observe`, where the field is not among them: the hint would
    /// leave the fold out, and the node start while a contributor before
    /// it is in work.
    fn fold_own(
        &mut self,
        name: &'static str,
        from: Option<phitex_defuse::NodeId>,
        to: Option<phitex_defuse::NodeId>,
    ) -> R<ValueId> {
        if cfg!(feature = "observe") {
            let k = PageNode::of(self.ops, self.v, self.cx.op(), &self.cx.args());
            assert!(
                k.is_some_and(|k| k.folds().contains(&name)),
                "node {} ({k:?}) folds {name}, which its read hint leaves out (PageNode::folds)",
                self.cx.node().raw()
            );
        }
        let f = self.v.field(name);
        Ok(match to {
            None => self.cx.acc_range(f, from)?,
            Some(_) => self.cx.acc_range_to(f, from, to)?,
        })
    }
}

/// A location's value at a candidate in a piece's check data
/// ([`Run::check_data`]) that the piece had neither read nor defined
/// before: what reached the piece, which its page's finisher reads before
/// it ([`phitex_defuse::Ctx::use_field_before`]).
const BEFORE_PIECE: &str = "\u{1}before the piece";

/// `max_dimen` (§421).
pub(crate) const MAX_DIMEN: i32 = 0o7777777777;

/// The fields the pages define through a text piece (beyond what its
/// hint can name).
pub(crate) fn is_page_field(name: &str) -> bool {
    PAGE_FIELDS.contains(&name)
}

/// The fields a piece with items on the main list may define for the
/// pages (its hint names them, `crate::hints`): what it leaves the next
/// piece of the main list (the previous depth, the space factor, the
/// last item), its continuation where its run ends at a candidate, the
/// conditionals it leaves open.
pub(crate) const PAGE_FIELDS: [&str; 6] = [
    crate::build::MAIN_PD,
    crate::build::MAIN_SF,
    crate::build::MAIN_LAST,
    crate::build::MAIN_SINCE,
    RESUME,
    crate::run::COND_STACK,
];

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
    /// Whether the run could end at the candidate, its rest a resume
    /// ([`Run::finish_candidate`]): not inside a group or a conditional
    /// of its own, with input left.
    pub cuttable: bool,
}

/// A finisher's page: its index and what the routine leaves for the
/// next page (§1026: the held-over insertions, then its list; the
/// scan's continuation after the finisher folds them, with the material
/// carried over and the rest of the piece).
#[derive(Clone, Debug, Default)]
pub(crate) struct FinCtx {
    pub j: i64,
    pub pending: Vec<ValueId>,
    /// How many of `pending` are the held-over insertions (§1022).
    pub held_n: usize,
    /// The `rtn` the finisher defined.
    pub rtn: Option<ValueId>,
    /// Its arguments in full ([`Run::finisher_args`]).
    pub args: Vec<ValueId>,
    /// Its page's break in its piece (`[piece, candidate]`) and whether
    /// the piece goes on after it (the break is inside it).
    pub brk: Option<(i64, i64, bool)>,
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
    /// The page parameters in force as values, until a marker names
    /// them (a state taken over from the scan over profiles,
    /// [`Run::assemble_narrow`], where the marker in force is not read).
    pub params_pre: Option<PageParams>,
    /// Contributions after the last candidate: folded items and markers,
    /// in order; the next candidate takes them first (§994).
    pub pending: Vec<ValueId>,
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
    /// `\box255` as the builder packed it, native (spec-final A4): its
    /// line boxes are the pieces' values, never encoded again; `data`
    /// holds `()` in its place.
    pub b255: Option<(phitex_typeset::node::BoxNode, Side)>,
    pub carried: Vec<ValueId>,
    pub rest: Vec<ValueId>,
    /// The best break's index on the page, and the insertions held over
    /// ([`crate::vprof::ScanFire`]'s).
    pub best: usize,
    pub held: usize,
}

fn some(x: ValueId) -> Option<ValueId> {
    (x != ValueId::UNIT).then_some(x)
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
            params_pre: None,
            pending: Vec::new(),
            normal: 0,
        }
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

    /// The page fired at candidate `k` (§1017: what the page could not
    /// take goes first in what is carried, then `more`), the builder
    /// set for the next page.
    fn fired(&mut self, v: &Values, f: &Fired, k: u32, more: Vec<ValueId>) -> Fire {
        let mut carried: Vec<ValueId> = f
            .contributions
            .iter()
            .map(|n| nodes::encode(v, n, &self.side))
            .collect();
        carried.extend(more);
        let (best, held) = (f.best, f.held_inserts.len());
        let data = page_data(v, f, &self.side);
        let b255 = Some((f.box255.node.clone(), self.side.clone()));
        self.pb = PageBuilder::new();
        self.side = Side::default();
        self.j += 1;
        if f.output_penalty > -10001 {
            self.normal += 1;
        }
        self.fold.since_bp = 0;
        Fire {
            k,
            data,
            b255,
            carried,
            rest: Vec::new(),
            best,
            held,
        }
    }

    /// The page fired at candidate `k` of a later piece at its known best
    /// break (`best`, `\outputpenalty` there, `\holdinginserts` positive
    /// where it fired), the page built as far as the break
    /// ([`PageBuilder::fire_known`]): its finisher's page from the items
    /// before the break alone. `None` where the page is not built to it.
    pub(crate) fn fire_known(
        &mut self,
        v: &Values,
        best: usize,
        output_penalty: i32,
        holding: bool,
        k: u32,
    ) -> Option<Fire> {
        let mut params = self.params_pre.unwrap_or_else(|| {
            self.params.map_or(
                PageParams {
                    vsize: 0,
                    max_depth: 0,
                    top_skip: Glue::ZERO,
                    holding_inserts: 0,
                    saving_vdiscards: false,
                },
                |m| params_of(v, m),
            )
        });
        params.holding_inserts = i32::from(holding);
        let f = self.pb.fire_known(best, output_penalty, &params)?;
        Some(self.fired(v, &f, k, Vec::new()))
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
        let mut unresolved: Vec<u16> = Vec::new();
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
                if list_of(v, x).len() == 2 {
                    unresolved.push(u16::try_from(int_at(v, x, 1)).unwrap_or(0));
                } else {
                    classes.push(class_of(v, x));
                }
                false
            }
            _ => true,
        });
        // (a marker no step resolved: the registers as the fold has them)
        for n in unresolved {
            classes.push((n, live(n)?));
        }
        if page_params.is_some() {
            self.params = page_params;
            self.params_pre = None;
        }
        let params = self.params_pre.unwrap_or_else(|| {
            self.params.map_or(
                PageParams {
                    vsize: 0,
                    max_depth: 0,
                    top_skip: Glue::ZERO,
                    holding_inserts: 0,
                    saving_vdiscards: false,
                },
                |m| params_of(v, m),
            )
        });
        // (the contributions as a stack, the next on top: a `Vec`)
        let mut queue: Vec<ValueId> = std::mem::take(buf);
        queue.reverse();
        let mut end_seen = None;
        while let Some(x) = queue.pop() {
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
                        queue.push(y);
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
                    .last()
                    .and_then(|&y| nodes::decode(v, y, &mut Side::default()))
            } else {
                None
            };
            let mut halt = None;
            let out = {
                let mut class = |n: u16| {
                    classes.iter().find(|c| c.0 == n).map_or_else(
                        || match live(n) {
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
                        },
                        |c| c.1,
                    )
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
                    queue.push(x);
                    break;
                }
                Outcome::Fire(f) => {
                    if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                        eprintln!(
                            "page-trace: page {} fires at candidate {k}: vsize {} maxdepth {} (marker {})",
                            self.j,
                            params.vsize,
                            params.max_depth,
                            self.params
                                .map_or_else(|| "none".to_owned(), |m| v.render(m))
                        );
                    }
                    let mut more: Vec<ValueId> = queue.drain(..).rev().collect();
                    more.extend(end_seen);
                    buf.clear();
                    return Ok(Some(self.fired(v, &f, k, more)));
                }
            }
        }
        queue.reverse();
        *buf = queue;
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
    list_of(v, data)
        .get(4)
        .is_some_and(|&x| int_of(v, x) <= -10001)
}

/// A fired page as a finisher gets it: `[\box255, inserts, marks, held,
/// \outputpenalty]` (§1012–§1022).
fn page_data(v: &Values, f: &Fired, side: &Side) -> ValueId {
    // (`\box255` stays native, [`Fire::b255`])
    let _ = side;
    let b255 = ValueId::UNIT;
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
        self.finish_candidate_at(false)
    }

    /// [`Run::finish_candidate`]; `newgraf`: the candidate of a
    /// paragraph's start (§1091), after its level is pushed, its indent
    /// box appended and `\everypar` put on the input.
    pub(crate) fn finish_candidate_at(&mut self, newgraf: bool) -> R<()> {
        if !self.at_outer()? {
            return Ok(());
        }
        let k = self.cand;
        // (§994: `build_page` with nothing contributed does nothing, so a
        // page fires only at a candidate with items contributed since the
        // one before: the last of this run, or for a resume's first one
        // the candidate its piece was cut at, where nothing was pending;
        // a piece's first candidate otherwise may see items an earlier
        // piece left after its last)
        let fresh = match self.cand_marks.last() {
            Some(m) => self.main_items_since(m.contribs),
            None => self.cx.op() != self.ops.resume || self.main_items_since(0),
        };
        self.note_candidate(k)?;
        self.cand += 1;
        let cuttable = fresh && self.cuttable_here(newgraf);
        self.cand_marks.push(CandMark {
            k,
            stamp: self.st.stamp(),
            gids: self.st.gid_mark(),
            contribs: self.contribs.len(),
            kids: self.kids.len(),
            lines: self.lines_entered,
            sig: self.st.sig(),
            nest: self.nest.len(),
            cuttable,
        });
        self.st.mark_candidate();
        if crate::exec::obs_flag!("PHITEX_CAND_TRACE") {
            self.trace_candidate(k, newgraf, fresh, cuttable);
        }
        // (every candidate with text after it is a cut, as every
        // paragraph end is: the rest of the piece is a continuation
        // placed after it, after the finisher of a page firing there,
        // which reads the state there forward and what a routine leaves.
        // Nothing decides it after the piece: no page's finisher reaches
        // back into the piece it fires in)
        // (inside a group or a conditional of the run's own, only where
        // the page is made to fire, above: elsewhere the piece runs on
        // through the candidate, and a page firing there has its
        // finisher check the rest, [`Run::check_cut`])
        if cuttable {
            if self.open_span() && crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
                eprintln!(
                    "page-trace: node {} cut inside its own span at candidate {k}: nest {:?} conds {}",
                    self.cx.node().raw(),
                    self.nest
                        .iter()
                        .map(|l| (l.group, l.owns))
                        .collect::<Vec<_>>(),
                    self.conds.len()
                );
            }
            self.cut = Some(k);
            return Err(Halt::Cut);
        }
        Ok(())
    }

    /// What the builder reads of eqtb at candidate `k` noted on the main
    /// list (§987's page parameters, §1008's registers of the insertion
    /// classes put on it since the last candidate), then its marker.
    fn note_candidate(&mut self, k: u32) -> R<()> {
        let v = self.v;
        if self.st.take_page_params() {
            if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
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
        Ok(())
    }

    /// Whether the run can end at the candidate it is at, its rest a
    /// resume (items contributed since the one before aside).
    fn cuttable_here(&mut self, newgraf: bool) -> bool {
        let op = self.cx.op();
        // (inside a group or a conditional of the run's own only where
        // the builder is made to fire: a penalty of `eject_penalty` or
        // less put on the list since the candidate before, §1005; the
        // routine runs there, and the rest reads what it defines (a page
        // so far saved in a box, a column height), so the rest is a
        // node after the routine. Never inside a list of the run's own)
        // (a paragraph's start is a cut too, its level just pushed and
        // carried by the continuation with its list and the input
        // `\everypar` left (§1091): the page firing there has its
        // finisher before the paragraph's commands, which read what the
        // routine leaves, as they come after it in tex.web)
        let par_start = newgraf
            && self.conds.is_empty()
            && self.nest.last().is_some_and(|l| l.owns && l.group == 0)
            && self.nest.len() == 1;
        let open_ok = !self.open_span()
            || par_start
            || (self.nest.iter().all(|l| !l.owns && l.group != 0)
                && self.forced_since(self.cand_marks.last().map_or(0, |m| m.contribs)));
        (op == self.ops.exec || op == self.ops.resume)
            && (self.top || self.doc)
            && open_ok
            && self.has_input()
    }

    /// `PHITEX_CAND_TRACE`: the candidate `k` and what decided its cut.
    fn trace_candidate(&mut self, k: u32, newgraf: bool, fresh: bool, cuttable: bool) {
        let v = self.v;
        let op = self.cx.op();
        if self.cand_marks.len() == 1 {
            let args = self.cx.args();
            let first = args
                .first()
                .map(|&x| v.render(x))
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>();
            eprintln!("cand-text: node {} {first}", self.cx.node().raw());
        }
        eprintln!(
            "cand: node {} op {} k {k} newgraf {newgraf} fresh {fresh} input {} nest {:?} conds {} forced {} cuttable {cuttable}",
            self.cx.node().raw(),
            if op == self.ops.exec {
                "exec"
            } else if op == self.ops.resume {
                "resume"
            } else {
                "other"
            },
            self.has_input(),
            self.nest
                .iter()
                .map(|l| (l.mode, l.group, l.owns))
                .collect::<Vec<_>>(),
            self.conds.len(),
            self.forced_since(
                self.cand_marks
                    .len()
                    .checked_sub(2)
                    .and_then(|i| self.cand_marks.get(i))
                    .map_or(0, |m| m.contribs)
            ),
        );
    }

    /// Whether this run put an item on the main list (a node, or the
    /// removal of one) in its contributions from the `from`-th on; the
    /// markers for the builder and the writer are not items.
    fn main_items_since(&self, from: usize) -> bool {
        let v = self.v;
        let f = v.field("items");
        self.contribs
            .get(from..)
            .unwrap_or_default()
            .iter()
            .filter(|&&(g, _)| g == f)
            .any(|&(_, s)| {
                v.seq_to_vec(s).iter().any(|&e| {
                    let xs = list_of(v, e);
                    xs.len() == 2
                        && int_of(v, xs[0]) == 0
                        && matches!(
                            nodes::tag(v, xs[1]),
                            0..=nodes::T_UNSET
                                | nodes::T_DELETE
                                | nodes::T_VBOX
                                | nodes::T_END
                                | nodes::T_ABORT
                        )
                })
            })
    }

    /// Whether this run put a penalty of `eject_penalty` or less on the
    /// main list in its contributions from the `from`-th on (§1005: the
    /// builder fires the page there, unless the page holds no box).
    fn forced_since(&self, from: usize) -> bool {
        let v = self.v;
        let f = v.field("items");
        self.contribs
            .get(from..)
            .unwrap_or_default()
            .iter()
            .filter(|&&(g, _)| g == f)
            .any(|&(_, s)| {
                v.seq_to_vec(s).iter().any(|&e| {
                    let xs = list_of(v, e);
                    xs.len() == 2
                        && int_of(v, xs[0]) == 0
                        && nodes::tag(v, xs[1]) == nodes::T_PENALTY
                        && list_of(v, xs[1])
                            .get(1)
                            .is_some_and(|&p| int_of(v, p) <= -10_000)
                })
            })
    }

    /// The siblings this run's text says it takes: an exec node's hint
    /// (`None` where the text does not say), a resume's guess (its
    /// fourth argument).
    pub(crate) fn claim_guess(&self) -> Option<usize> {
        let v = self.v;
        let args = self.cx.args();
        let op = self.cx.op();
        if op == self.ops.resume {
            return args
                .get(3)
                .and_then(|&x| v.as_int(x))
                .and_then(|n| usize::try_from(n).ok());
        }
        if op != self.ops.exec {
            return None;
        }
        crate::hints::hint_of_args(&self.ops.hints, v, &args)
            .bound
            .as_ref()
            .and_then(|b| b.claim)
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
    pub(crate) fn own_outer_items(&self) -> Vec<ValueId> {
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
    /// (The line of PDF records is ordered by the link instead: a
    /// piece's records carry their candidate, `pdfout::order_line`.)
    fn streams_since(&self, from: usize) -> Vec<Field> {
        let v = self.v;
        let f = v.field("streams");
        let mut out: Vec<Field> = Vec::new();
        for &(g, x) in self.contribs.get(from..).unwrap_or_default() {
            if g != f {
                continue;
            }
            for e in v.seq_to_vec(x) {
                // (an entry tagged with its page: its own; a candidate
                // marker: none)
                if crate::api::is_cand_marker(v, e) {
                    continue;
                }
                let entry = list_of(v, crate::api::stream_entry(v, e));
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
        let c = self.insert_class(n)?;
        Ok(self.ins_marker(n, &c))
    }

    /// A `T_INS_PARAMS` marker with class `n`'s registers `c`.
    fn ins_marker(&self, n: u16, c: &InsertClass) -> ValueId {
        let v = self.v;
        v.list(vec![
            v.int(nodes::T_INS_PARAMS),
            v.int(i64::from(n)),
            v.int(c.box_height_plus_depth.map_or(-1, i64::from)),
            v.int(i64::from(c.box_is_hbox)),
            v.int(i64::from(c.count)),
            v.int(i64::from(c.dimen)),
            glue_value(v, &c.skip),
        ])
    }

    /// Whether this run defined one of class `n`'s registers (its box,
    /// `\count`, `\dimen` or `\skip`).
    fn ins_registers_defined(&mut self, n: u16) -> bool {
        let bl = self.box_loc(i64::from(n));
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
        let names = [
            "param:vsize",
            "param:maxdepth",
            "param:topskip",
            "param:holdinginserts",
        ];
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
    /// defines what it leaves the next piece of the main list
    /// ([`Run::define_main_tail`]), gives its
    /// profile and its items as units ([`Run::contribute_units`]) and
    /// places its scan step ([`Scan`]) after it; its own run never reads
    /// the page. A piece whose run ended at a page-builder candidate with
    /// input after it leaves the rest to a resume piece
    /// ([`Run::fold_here`], spec-final A1). A piece with nothing
    /// for the main list touches nothing. A finisher's end defines what
    /// its routine gives back ([`RTN`]).
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
        if fin {
            return self.define_rtn(raw);
        }
        if raw.is_empty() {
            return Ok(());
        }
        // (a piece whose run ended at a candidate: the resume is its to
        // place)
        if self.cut.is_none() {
            self.define_main_tail(&raw)?;
            return self.fold_apart(raw);
        }
        self.fold_here(raw)
    }

    /// A finisher's end: what its routine gives back to the main list
    /// ([`RTN`]); the fold continuation placed after it folds that.
    #[allow(clippy::unnecessary_wraps)]
    fn define_rtn(&mut self, raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        let c = self.fin.clone().unwrap_or_default();
        let list = c.pending.get(c.held_n..).unwrap_or_default().to_vec();
        let x = v.list(vec![v.list(raw), v.list(list)]);
        self.cx.define_with(v.field(RTN), x, Locality::Global);
        let j = self.cx.args().first().map_or(0, |&y| int_of(v, y));
        self.cx.define_with(rtn_field(v, j), x, Locality::Global);
        if let Some(c) = self.fin.as_mut() {
            c.rtn = Some(x);
        }
        Ok(())
    }

    /// A page found firing (spec-redesign §1): its break (`[j, read at,
    /// src, piece, candidate, rest, origin]`:
    /// [`Run::assemble_page`]) and its
    /// finisher, placed right after the piece the page fires in (`at`,
    /// [`Ctx::follow_after`]; after this node where it fires in this
    /// node's own material), with its position and the break as its
    /// arguments (`[j, …, routine hint, ordinary pages fired, special,
    /// break]`). Nothing of the page's items: the finisher and the
    /// continuation assemble it from the break. `origin`: for a page
    /// firing in a continuation's material, the piece whose items after
    /// an earlier page's candidate that material holds (`[piece, its
    /// last candidate, what it read, what it defined after each
    /// candidate, its locals at its last]`): the page's finisher checks
    /// the piece there as for a page firing in its own items
    /// ([`Run::check_cut`]).
    ///
    /// (Why the page's index `j` and its ordinary pages stay in the
    /// arguments, so that a page added before re-places the finishers and
    /// continuations after it: the finisher's predicted definitions,
    /// [`FinDefs::predict`], step page counters by the ordinary pages
    /// between, from its arguments alone; without them a cold build's
    /// finishers wait on one another's counters. Those finishers run
    /// again anyway where a page is added: their pages' numbers differ.
    /// The scan state leaves them out, [`SCAN_AT`].)
    #[allow(clippy::too_many_arguments)]
    fn place_break(
        &mut self,
        at: Option<phitex_defuse::NodeId>,
        here: bool,
        j: i64,
        src: ValueId,
        pid: i64,
        k: u32,
        rest: usize,
        normal: i64,
        special: bool,
        origin: Option<ValueId>,
        classes: Option<&[u16]>,
    ) -> R<(ValueId, Vec<ValueId>)> {
        let v = self.v;
        let text = self.output_hint()?;
        // (where what the page's builder reads is read: right after the
        // piece it fires in, before its finisher; this node for a page
        // firing in this node's own material)
        let read_at = at.unwrap_or_else(|| self.cx.node());
        // (where the page stands among the pages, which its state `src`
        // leaves out, [`SCAN_AT`]: the ordinary pages before it and
        // after it, `normal` counting it where it is one)
        let b = v.list(vec![
            v.int(j),
            v.int(i64::from_ne_bytes(read_at.raw().to_ne_bytes())),
            src,
            v.int(pid),
            v.int(i64::from(k)),
            v.int(i64::try_from(rest).unwrap_or(-1)),
            origin.unwrap_or(ValueId::UNIT),
            v.int(normal - i64::from(!special)),
            v.int(normal),
        ]);
        let mut pos = vec![v.int(j)];
        pos.extend([ValueId::UNIT; 8]);
        pos.extend([text, v.int(normal), v.int(i64::from(special)), b]);
        // (the insertion classes on the page, where the scan knows them:
        // the boxes of the others its fire leaves alone, [`fin_classes`])
        if let Some(cs) = classes {
            pos.push(v.list(cs.iter().map(|&n| v.int(i64::from(n))).collect()));
        }
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "place finisher of page {j} (piece {} candidate {k}) by {} after {:?}",
                pid.cast_unsigned(),
                self.cx.node().raw(),
                at.map(phitex_defuse::NodeId::raw)
            );
        }
        match at {
            Some(at) if !here => {
                self.cx.follow_after(at, self.ops.finish, pos.clone());
            }
            _ => {
                self.cx.follow(self.ops.finish, pos.clone());
            }
        }
        Ok((b, pos))
    }

    /// The continuation of page `j` (break `b`) placed after its
    /// finisher, with the scan state it is predicted to define (`pred`,
    /// [`Scan::defs`]): the next page's scan runs on that instead of
    /// waiting for it. Arguments `[-2, j, state, material, break]`.
    fn place_cont(
        &mut self,
        at: Option<phitex_defuse::NodeId>,
        here: bool,
        j: i64,
        pred: Option<(ValueId, ValueId)>,
        b: ValueId,
        chain: Option<(i64, i64, ValueId)>,
    ) {
        let v = self.v;
        // (its page, the block's scan that placed it and its break: the
        // field it leaves the scan in, [`scan_page_field`]; a page firing
        // in a continuation's material: the chain that continuation is in
        // goes on. The predicted state and material are its arguments 2
        // and 3: its static hooks are a function of its text, and a scan
        // running again on another prediction places another node)
        let chain = match at {
            Some(_) => Some((j, i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()), b)),
            None => chain,
        };
        let (px, pm) = pred.unwrap_or((ValueId::UNIT, ValueId::UNIT));
        let mut args = vec![v.int(-2), v.int(j), px, pm, b];
        if let Some((j0, by, b0)) = chain {
            args.extend([v.int(by), v.int(j0), b0]);
        }
        match at {
            Some(at) if !here => {
                self.cx.follow_after(at, self.ops.scan, args);
            }
            _ => {
                self.cx.follow(self.ops.scan, args);
            }
        }
    }

    /// A page assembled from its break (`b`, [`Run::place_break`]): the
    /// state where it began and its material (`src`), the items of the
    /// pieces after the anchor up to piece `pid` (the break's range: the
    /// window ends at its piece, whatever comes after it), folded with
    /// the page builder; what a class's registers and `\deadcycles` give
    /// read where the break was found. `(state after, the fire, the piece's
    /// unit)`, the unit empty for a page firing in the material; `None`
    /// where the items fire no page at the break (a scan defect, said).
    #[allow(clippy::too_many_lines)]
    fn assemble_page(
        &mut self,
        b: &[ValueId],
        mat: Option<ValueId>,
        narrow: bool,
    ) -> R<Option<(PageState, Fire, Vec<ValueId>)>> {
        let v = self.v;
        let n = |i: usize| b.get(i).map_or(0, |&x| int_of(v, x));
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(n(1).to_ne_bytes()));
        let (pid, k, rest) = (n(3), n(4), n(5));
        let mut s = ScanIn::decode(v, b.get(2).copied().and_then(some));
        // (where the page stands, which its state leaves out: the break's)
        s.j = n(0);
        s.normal = n(7);
        let mut raws = Vec::new();
        let mut raw_pids: Vec<i64> = Vec::new();
        let mut own = Vec::new();
        // (the step that found the break reads what it reads there where
        // it is, for its continuation's prediction: a read at a node is
        // of a node before the reader)
        let here = at == self.cx.node();
        let mut check: Option<(PageState, Fire, Vec<ValueId>)> = None;
        // (a finisher's page whose items before the best break end before
        // the piece it fires in, [`Run::best_window`]: those items alone,
        // and the firing piece's unit for its check; what lies between
        // the break and the fire is the next page's, §1017)
        let upto = if !narrow && mat.is_none() && pid != -2 {
            self.best_window(&s, pid, k, here, at)?
        } else {
            None
        };
        if let Some(sf) = &upto
            && let Some((bp, prev)) = sf.upto
        {
            let node =
                |x: i64| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(x.to_ne_bytes()));
            let unit_pid = |u: ValueId| list_of(v, u).first().map_or(-1, |&x| int_of(v, x));
            s.material = match self.eng_read(scan_mat_field(v, n(0)))? {
                Some(m) => list_of(v, m),
                None => Vec::new(),
            };
            if bp != crate::vprof::MATERIAL {
                let mut items = v.seq_to_vec(self.fold_own(PITEMS, s.anchor, Some(node(bp)))?);
                let Some(last) = items.iter().position(|&x| unit_pid(x) == bp) else {
                    self.scan_defect(&format!("piece {bp} is not in its range"));
                    return Ok(None);
                };
                items.truncate(last + 1);
                for &x in &items {
                    let mut raw = unit_items(v, x);
                    self.resolve_params_of(unit_pid(x), &mut raw)?;
                    raw_pids.push(unit_pid(x));
                    raws.push(raw);
                }
            }
            let tail = v.seq_to_vec(self.fold_own(
                PITEMS,
                prev.map(node).or(s.anchor),
                Some(node(pid)),
            )?);
            let Some(&mine) = tail.iter().find(|&&x| unit_pid(x) == pid) else {
                self.scan_defect(&format!("piece {pid} is not in its range"));
                return Ok(None);
            };
            own = list_of(v, mine);
        } else if pid != -2 {
            // (the material the page began with, as items, where the break
            // was found; a page firing in a continuation's material has it
            // in its `src`)
            // (as the step predicting the page's continuation has it,
            // `mat`; else as it reaches here: a finisher and a
            // continuation sit after the continuation of the page before
            // (and the pages firing in its material), which defines it,
            // also where both pages break in one piece)
            s.material = match mat {
                Some(m) => list_of(v, m),
                None => self
                    .eng_read(scan_mat_field(v, n(0)))?
                    .map(|x| list_of(v, x))
                    .unwrap_or_default(),
            };
            // (what goes on past the page from the pieces it needs only,
            // [`Run::assemble_narrow`]: an edit in a piece before them that
            // leaves its profile alone runs the page's finisher, not its
            // continuation)
            if narrow && let Some(r) = self.assemble_narrow(&s, pid, k, rest, here, at)? {
                if !crate::exec::obs_flag!("PHITEX_NARROW_CHECK") {
                    return Ok(Some(r));
                }
                check = Some(r);
            }
            // (the window ends at the break's piece: neither the step
            // that found the break, nor this page's finisher, nor
            // anything after the piece is in it, so the page's nodes do
            // not wait on one another)
            let to = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
            let mut items = v.seq_to_vec(self.fold_own(PITEMS, s.anchor, Some(to))?);
            let unit_pid = |u: ValueId| list_of(v, u).first().map_or(-1, |&x| int_of(v, x));
            if crate::exec::obs_flag!("PHITEX_ASSEMBLE_TRACE") {
                eprintln!(
                    "assemble {} (op {:?}): page {}, anchor {:?}, piece {pid}, candidate {k}, rest {rest}, material {} items, window {:?}",
                    self.cx.node().raw(),
                    self.cx.args().first().map(|&x| int_of(v, x)),
                    n(0),
                    s.anchor.map(phitex_defuse::NodeId::raw),
                    s.material.len(),
                    items
                        .iter()
                        .map(|&u| (unit_pid(u), unit_items(v, u).len()))
                        .collect::<Vec<_>>()
                );
            }
            let Some(last) = items.iter().position(|&u| unit_pid(u) == pid) else {
                self.scan_defect(&format!("piece {pid} is not in its range"));
                return Ok(None);
            };
            items.truncate(last + 1);
            for &u in &items {
                let mut raw = unit_items(v, u);
                self.resolve_params_of(unit_pid(u), &mut raw)?;
                raw_pids.push(unit_pid(u));
                raws.push(raw);
            }
            own = list_of(v, items[last]);
        }
        // (a node reads what reaches its own place by its own reads: a
        // position read is of an earlier node)
        let dead = if !(has_end(v, &s.material) || raws.iter().any(|r| has_end(v, r))) {
            0
        } else if here {
            self.cint(crate::fields::cell::DEADCYCLES)?
        } else {
            let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
            match self.cx.use_field_at(f, Some(at))? {
                phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                phitex_defuse::FieldAt::Unset => 0,
                phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
            }
        };
        let mut ps = s.page_state();
        // (an insertion class's registers read where the scan over the
        // profiles reads them, [`RunReaching`]: the material's at the
        // state's anchor, each piece's at that piece; read elsewhere, a
        // finisher between them gives other registers, and the folds
        // part)
        let fire = {
            let mut pos: Option<phitex_defuse::NodeId> = match s.anchor {
                Some(a) => Some(a),
                None if here => None,
                None => Some(at),
            };
            let last = raws.len();
            let mut out = {
                let mut live = |c: u16| match pos {
                    Some(p) if p != self.cx.node() => self.insert_class_at(c, p),
                    _ => self.insert_class(c),
                };
                ps.fold_items(v, &s.material, dead, &mut live)?
                    .map(|f| (0, f))
            };
            for (i, raw) in raws.iter().enumerate() {
                if out.is_some() {
                    break;
                }
                pos = Some(phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(
                    raw_pids[i].to_ne_bytes(),
                )));
                let mut live = |c: u16| match pos {
                    Some(p) if p != self.cx.node() => self.insert_class_at(c, p),
                    _ => self.insert_class(c),
                };
                out = ps.fold_items(v, raw, dead, &mut live)?.map(|f| (i + 1, f));
                if crate::exec::obs_str!("PHITEX_DEFECT_PAGE").and_then(|x| x.parse::<i64>().ok())
                    == Some(n(0))
                {
                    eprintln!(
                        "item folds unit {i} at {} (node {}): page {} goal {} total {} best {:?} fired {:?}",
                        raw_pids[i],
                        self.cx.node().raw(),
                        ps.pb.page.len(),
                        ps.pb.so_far[0],
                        ps.pb.so_far[1],
                        ps.pb.best_break,
                        out.as_ref().map(|(_, f)| (f.k, f.best))
                    );
                }
            }
            match (out, &upto) {
                // (the page built as far as its known break, which a later
                // piece's candidate fired)
                (None, Some(sf)) => ps.fire_known(v, sf.best, sf.output_penalty, sf.holding, sf.k),
                (Some((i, f)), None) if i == last => Some(f),
                _ => None,
            }
        };
        let Some(fire) = fire else {
            // (what the two folds started from, to tell a state from its
            // material: the profile of the material read against the
            // state's own)
            let mp = crate::vprof::encode(v, &s.material);
            eprintln!(
                "phitex: defect detail at node {} (op {:?}, speculative {}): page {}, state anchor {:?}, state profile {} ints, material {} items (its profile {} ints, {}), window {:?}",
                self.cx.node().raw(),
                self.cx.args().first().map(|&x| int_of(v, x)),
                self.cx.is_speculative(),
                n(0),
                s.anchor.map(phitex_defuse::NodeId::raw),
                s.prof.len(),
                s.material.len(),
                mp.len(),
                if mp == s.prof {
                    "the same"
                } else {
                    "DIFFERENT"
                },
                raws.iter().map(Vec::len).collect::<Vec<_>>()
            );
            if cfg!(feature = "observe") {
                self.defect_explain(&s, &raws, &raw_pids, dead, b, mat.is_some())?;
            }
            if crate::exec::obs_flag!("PHITEX_DEFECT_DUMP") {
                let show = |raw: &[ValueId]| -> String {
                    raw.iter()
                        .map(|&x| {
                            let t = nodes::tag(v, x);
                            if t == nodes::T_VBOX || t == nodes::T_BOX {
                                let b = if t == nodes::T_VBOX {
                                    list_of(v, x).get(1).copied().unwrap_or(ValueId::UNIT)
                                } else {
                                    x
                                };
                                format!(
                                    "{t}[h{} d{}]",
                                    nodes::box_dim(v, b, 3),
                                    nodes::box_dim(v, b, 2)
                                )
                            } else {
                                format!("{t}:{}", v.render(x).chars().take(60).collect::<String>())
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                eprintln!("defect dump material: {}", show(&s.material));
                // (both folds again, side by side, unit by unit)
                {
                    let mut ips = s.page_state();
                    let mut pst = s.state(v);
                    let mut rd = RunReaching(
                        self,
                        s.anchor.map(|a| i64::from_ne_bytes(a.raw().to_ne_bytes())),
                    );
                    let mprof = crate::vprof::decode(&s.prof);
                    let pf = pst.fold_items(&mprof, dead, &mut rd).map(|f| f.k);
                    let mut lv = |c: u16| rd.0.insert_class(c);
                    let _ = (&mut lv, pf);
                    eprintln!(
                        "defect folds material: profile page {} total {} fired {:?}",
                        pst.pb.page.len(),
                        pst.pb.so_far[1],
                        pf
                    );
                    for (i, raw) in raws.iter().enumerate() {
                        let mut pi = crate::vprof::decode(&crate::vprof::encode(v, raw));
                        rd.0.resolve_prof_of(raw_pids[i], &mut pi)?;
                        rd.1 = Some(raw_pids[i]);
                        let f = pst.fold_items(&pi, dead, &mut rd).map(|f| (f.k, f.best));
                        eprintln!(
                            "defect folds unit {i}: profile page {} goal {} total {} best {:?} fired {:?}",
                            pst.pb.page.len(),
                            pst.pb.so_far[0],
                            pst.pb.so_far[1],
                            pst.pb.best_break,
                            f
                        );
                    }
                    let _ = &mut ips;
                }
                for (i, raw) in raws.iter().enumerate() {
                    eprintln!("defect dump unit {i}: {}", show(raw));
                    eprintln!(
                        "defect dump unit {i} profile: {:?}",
                        crate::vprof::decode(&crate::vprof::encode(v, raw))
                    );
                }
            }
            self.scan_defect(&format!(
                "page {} fires in piece {pid} at candidate {k} by the profiles, not by the items",
                n(0) + 1
            ));
            return Ok(None);
        };
        if upto.is_none()
            && (i64::from(fire.k) != k || i64::try_from(fire.rest.len()).ok() != Some(rest))
        {
            self.scan_defect(&format!(
                "page {} fires in piece {pid} at candidate {k} ({rest} items after) by the profiles, at candidate {} ({} after) by the items",
                n(0) + 1,
                fire.k,
                fire.rest.len()
            ));
        }
        // (`PHITEX_NARROW_CHECK`: the narrow assembly against this one)
        if let Some((nps, nf, nown)) = check {
            let diffs = [
                ("j", nps.j != ps.j),
                ("normal", nps.normal != ps.normal),
                ("fold", nps.fold != ps.fold),
                ("params", nps.params != ps.params),
                ("carried", nf.carried != fire.carried),
                ("rest", nf.rest != fire.rest),
                ("k", nf.k != fire.k),
                (
                    "held",
                    list_of(v, nf.data).get(3) != list_of(v, fire.data).get(3),
                ),
                ("own", nown != own),
            ];
            let bad: Vec<&str> = diffs.iter().filter(|d| d.1).map(|d| d.0).collect();
            if nf.carried != fire.carried {
                eprintln!(
                    "scan: narrow check at piece {}: carried {} {:?} against {} {:?}",
                    n(3),
                    nf.carried.len(),
                    nf.carried
                        .iter()
                        .map(|&x| nodes::tag(v, x))
                        .collect::<Vec<_>>(),
                    fire.carried.len(),
                    fire.carried
                        .iter()
                        .map(|&x| nodes::tag(v, x))
                        .collect::<Vec<_>>()
                );
            }
            eprintln!(
                "scan: narrow check at piece {}: {}",
                n(3),
                if bad.is_empty() {
                    "same".to_owned()
                } else {
                    format!("DIFFERS in {bad:?}")
                }
            );
        }
        Ok(Some((ps, fire, own)))
    }

    /// Where a page's items before its best break end, when that is
    /// before the piece `pid` it fires in at candidate `k`: the window's
    /// profiles folded as the scan folds them ([`Run::scan_window`]),
    /// its fire with [`crate::vprof::ScanFire::upto`] set; `None` where
    /// the break is in the firing piece (the whole window's items are
    /// the page's). The profiles are what the break rests on: a change
    /// in them runs the finisher again; a change in the items between
    /// the break and the fire, the next page's, does not.
    fn best_window(
        &mut self,
        s: &ScanIn,
        pid: i64,
        k: i64,
        here: bool,
        at: phitex_defuse::NodeId,
    ) -> R<Option<crate::vprof::ScanFire>> {
        let v = self.v;
        let to = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let units = v.seq_to_vec(self.fold_own(crate::vprof::VPROF, s.anchor, Some(to))?);
        let parts: Vec<(i64, Vec<crate::vprof::PItem>)> = units
            .iter()
            .map(|&u| {
                let (p, ints) = crate::vprof::unit_parts(v, u);
                (p, crate::vprof::decode(&ints))
            })
            .collect();
        if parts.len() < 2 || parts.last().map(|x| x.0) != Some(pid) {
            return Ok(None);
        }
        let material = crate::vprof::decode(&s.prof);
        // (`\deadcycles` as the scan's fold reads it)
        let dead = if !(crate::vprof::has_end(&material)
            || parts.iter().any(|x| crate::vprof::has_end(&x.1)))
        {
            0
        } else if here {
            self.cint(crate::fields::cell::DEADCYCLES)?
        } else {
            let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
            match self.cx.use_field_at(f, Some(at))? {
                phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                phitex_defuse::FieldAt::Unset => 0,
                phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
            }
        };
        let mut rd = RunReaching(
            self,
            s.anchor.map(|a| i64::from_ne_bytes(a.raw().to_ne_bytes())),
        );
        let mut st = s.state(v);
        if st.fold_items(&material, dead, &mut rd).is_some() {
            return Ok(None);
        }
        st.note_piece(crate::vprof::MATERIAL);
        let last = parts.len() - 1;
        for (i, (p, items)) in parts.into_iter().enumerate() {
            let mut items = items;
            rd.0.resolve_prof_of(p, &mut items)?;
            rd.1 = Some(p);
            if let Some(f) = st.fold_items(&items, dead, &mut rd) {
                return Ok((i == last && i64::from(f.k) == k && f.upto.is_some()).then_some(f));
            }
            st.note_piece(p);
        }
        Ok(None)
    }

    /// [`Run::assemble_page`] for what goes on past the page (the state
    /// after it, the material carried and held over, the rest of its
    /// piece), from the items of the last pieces of its window only: the
    /// profiles folded up to a piece (the scan's builder, the page's
    /// items as profiles), the items from there on, where the page's
    /// items from the best break on are all those pieces' (§1017), no
    /// insertion is held over (§1022), and the page parameters in force
    /// are named by a marker there or by none before. A change in a
    /// piece before them that leaves its profile alone then runs only
    /// the page's finisher, not what reads the material past the page.
    /// `None`: the whole window's items are needed.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn assemble_narrow(
        &mut self,
        s: &ScanIn,
        pid: i64,
        k: i64,
        rest: i64,
        here: bool,
        at: phitex_defuse::NodeId,
    ) -> R<Option<(PageState, Fire, Vec<ValueId>)>> {
        use crate::vprof::PItem;
        let v = self.v;
        let to = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let units = v.seq_to_vec(self.fold_own(crate::vprof::VPROF, s.anchor, Some(to))?);
        if units.len() < 2 {
            return Ok(narrow_miss(1, pid));
        }
        let parts: Vec<(i64, Vec<PItem>)> = units
            .iter()
            .map(|&u| {
                let (p, ints) = crate::vprof::unit_parts(v, u);
                (p, crate::vprof::decode(&ints))
            })
            .collect();
        if parts.last().map(|x| x.0) != Some(pid) {
            return Ok(narrow_miss(2, pid));
        }
        // (`\deadcycles` as the whole fold reads it)
        let dead =
            if !(has_end(v, &s.material) || parts.iter().any(|x| crate::vprof::has_end(&x.1))) {
                0
            } else if here {
                self.cint(crate::fields::cell::DEADCYCLES)?
            } else {
                let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
                match self.cx.use_field_at(f, Some(at))? {
                    phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                    phitex_defuse::FieldAt::Unset => 0,
                    phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
                }
            };
        // the profiles folded, the builder kept where each piece begins
        let mut snaps: Vec<crate::vprof::ScanState> = Vec::with_capacity(parts.len());
        let fired = {
            let mut rd = RunReaching(
                self,
                s.anchor.map(|a| i64::from_ne_bytes(a.raw().to_ne_bytes())),
            );
            let mut st = s.state(v);
            if st
                .fold_items(&crate::vprof::decode(&s.prof), dead, &mut rd)
                .is_some()
            {
                return Ok(narrow_miss(3, pid));
            }
            let mut fired = None;
            for (i, (p, items)) in parts.iter().enumerate() {
                let mut items = items.clone();
                rd.0.resolve_prof_of(*p, &mut items)?;
                rd.1 = Some(*p);
                snaps.push(st.clone());
                if let Some(f) = st.fold_items(&items, dead, &mut rd) {
                    fired = Some((i, f));
                    break;
                }
            }
            fired
        };
        let Some((last, sf)) = fired else {
            return Ok(narrow_miss(4, pid));
        };
        if last + 1 != parts.len()
            || i64::from(sf.k) != k
            || i64::try_from(sf.rest).ok() != Some(rest)
            || sf.held > 0
        {
            return Ok(narrow_miss(5, pid));
        }
        // (the latest piece the items can begin at: no contribution
        // pending from before it, the page before it ending before the
        // best break)
        let Some(u) = (1..=last)
            .rev()
            .find(|&u| snaps[u].pending.is_empty() && snaps[u].pb.page.len() <= sf.best)
        else {
            return Ok(narrow_miss(6, pid));
        };
        let marker_before = s
            .material
            .iter()
            .any(|&x| nodes::tag(v, x) == nodes::T_PAGE_PARAMS)
            || parts[..u]
                .iter()
                .any(|x| x.1.iter().any(|y| matches!(y, PItem::Params(_))));
        let from =
            phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(parts[u - 1].0.to_ne_bytes()));
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "scan {}: narrow assembly of the page firing in piece {pid}: items of pieces {:?}",
                self.cx.node().raw(),
                parts[u..].iter().map(|p| p.0).collect::<Vec<_>>()
            );
        }
        let items = v.seq_to_vec(self.fold_own(PITEMS, Some(from), Some(to))?);
        let unit_pid = |x: ValueId| list_of(v, x).first().map_or(-1, |&y| int_of(v, y));
        if items.len() != parts.len() - u
            || items
                .iter()
                .zip(&parts[u..])
                .any(|(&x, p)| unit_pid(x) != p.0)
        {
            return Ok(narrow_miss(7, pid));
        }
        let snap = &snaps[u];
        let mut ps = PageState::fresh(snap.j);
        ps.pb = snap.pb.clone();
        ps.fold = snap.fold;
        ps.normal = snap.normal;
        ps.params = s.params;
        if marker_before {
            ps.params_pre = Some(snap.params.unwrap_or(PageParams {
                vsize: 0,
                max_depth: 0,
                top_skip: Glue::ZERO,
                holding_inserts: 0,
                saving_vdiscards: false,
            }));
        }
        let mut out = None;
        for (i, &x) in items.iter().enumerate() {
            let mut raw = unit_items(v, x);
            self.resolve_params_of(unit_pid(x), &mut raw)?;
            let f = {
                let mut live = |c: u16| {
                    if here {
                        self.insert_class(c)
                    } else {
                        self.insert_class_at(c, at)
                    }
                };
                ps.fold_items(v, &raw, dead, &mut live)?
            };
            if let Some(f) = f {
                if i + 1 != items.len() {
                    return Ok(narrow_miss(8, pid));
                }
                out = Some(f);
            }
        }
        let Some(fire) = out else {
            return Ok(narrow_miss(9, pid));
        };
        if i64::from(fire.k) != k
            || i64::try_from(fire.rest.len()).ok() != Some(rest)
            || fire.best != sf.best
            || fire.held > 0
            || ps.params_pre.is_some()
        {
            return Ok(narrow_miss(10, pid));
        }
        let own = items.last().map(|&x| list_of(v, x)).unwrap_or_default();
        if crate::exec::obs_flag!("PHITEX_NARROW_CHECK") {
            eprintln!(
                "scan: narrow at piece {pid}: from part {u} of {}, page {} items there, best {}, carried {} ({:?}), rest {}",
                parts.len(),
                snaps[u].pb.page.len(),
                sf.best,
                fire.carried.len(),
                fire.carried
                    .iter()
                    .map(|&x| nodes::tag(v, x))
                    .collect::<Vec<_>>(),
                fire.rest.len()
            );
        }
        Ok(Some((ps, fire, own)))
    }

    /// A page where the scan's decisions and the builder's on the items
    /// differ: said loudly, counted ([`SCAN_REAL_DIFFS`]); the page is
    /// the items'.
    /// (`observe`) A page the profiles fire and the items do not: what
    /// the two folds started from and where they part. The state's
    /// profile against its material's, item by item, the differing ones
    /// with both values; where each came from (the break's placer, the
    /// state's anchor, the material's field or argument); then both
    /// folds replayed side by side, piece by piece, with the candidate
    /// each fires at, the first piece where they disagree named.
    #[allow(clippy::too_many_lines)]
    fn defect_explain(
        &mut self,
        s: &ScanIn,
        raws: &[Vec<ValueId>],
        raw_pids: &[i64],
        dead: i64,
        b: &[ValueId],
        mat_arg: bool,
    ) -> R<()> {
        let v = self.v;
        let n = |i: usize| b.get(i).map_or(0, |&x| int_of(v, x));
        let me = self.cx.node().raw();
        let short = |x: &crate::vprof::PItem| -> String {
            let t = format!("{x:?}");
            if t.len() > 140 {
                format!("{}...", &t[..140])
            } else {
                t
            }
        };
        // (1) the state's profile against its material's
        let sp = crate::vprof::decode(&s.prof);
        let mp = crate::vprof::decode(&crate::vprof::encode(v, &s.material));
        eprintln!(
            "defect {me}: page {} (break placed by node {}, piece {}, candidate {}); state: page {} anchor {:?}, {} profile items; material ({}): {} items, {} profile items",
            n(0),
            n(1).cast_unsigned(),
            n(3),
            n(4),
            s.j,
            s.anchor.map(phitex_defuse::NodeId::raw),
            sp.len(),
            if mat_arg {
                "the step's argument".to_owned()
            } else {
                format!("field scan:mat:{}", n(0))
            },
            s.material.len(),
            mp.len()
        );
        let mut shown = 0;
        for i in 0..sp.len().max(mp.len()) {
            let a = sp.get(i).map_or_else(|| "-".to_owned(), &short);
            let c = mp.get(i).map_or_else(|| "-".to_owned(), &short);
            if a != c {
                eprintln!("defect {me}:   item {i}: state profile {a} | material {c}");
                shown += 1;
                if shown >= 16 {
                    eprintln!("defect {me}:   ...");
                    break;
                }
            }
        }
        // (3) both folds replayed: the profiles (as the scan) and the
        // items (as the assembly), piece by piece
        let mut pst = s.state(v);
        let mut ps = s.page_state();
        let anchor = s.anchor.map(|a| i64::from_ne_bytes(a.raw().to_ne_bytes()));
        let pf = {
            let mut rd = RunReaching(self, anchor);
            pst.fold_items(&sp, dead, &mut rd).map(|f| f.k)
        };
        let mut pos = s.anchor;
        let itf = {
            let node = self.cx.node();
            let mut live = |c: u16| match pos {
                Some(p) if p != node => self.insert_class_at(c, p),
                _ => self.insert_class(c),
            };
            ps.fold_items(v, &s.material, dead, &mut live)?.map(|f| f.k)
        };
        eprintln!(
            "defect {me}:   material: profile fold total {} goal {} fires {pf:?} | items fold total {} goal {} fires {itf:?}",
            pst.pb.so_far[1], pst.pb.so_far[0], ps.pb.so_far[1], ps.pb.so_far[0]
        );
        let mut split = pf != itf;
        for (i, raw) in raws.iter().enumerate() {
            let pid = raw_pids[i];
            let mut pi = crate::vprof::decode(&crate::vprof::encode(v, raw));
            self.resolve_prof_of(pid, &mut pi)?;
            let pf = {
                let mut rd = RunReaching(self, Some(pid));
                pst.fold_items(&pi, dead, &mut rd).map(|f| (f.k, f.rest))
            };
            pos = Some(phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(
                pid.to_ne_bytes(),
            )));
            let itf = {
                let node = self.cx.node();
                let mut live = |c: u16| match pos {
                    Some(p) if p != node => self.insert_class_at(c, p),
                    _ => self.insert_class(c),
                };
                ps.fold_items(v, raw, dead, &mut live)?
                    .map(|f| (f.k, f.rest.len()))
            };
            let differ = pf != itf || pst.pb.so_far[1] != ps.pb.so_far[1];
            eprintln!(
                "defect {me}:   piece {pid}: profile total {} goal {} best {:?} fires {pf:?} | items total {} goal {} best {:?} fires {itf:?}{}",
                pst.pb.so_far[1],
                pst.pb.so_far[0],
                pst.pb.best_break,
                ps.pb.so_far[1],
                ps.pb.so_far[0],
                ps.pb.best_break,
                if differ && !split {
                    "  <-- the folds part here"
                } else {
                    ""
                }
            );
            split |= differ;
            if pf.is_some() || itf.is_some() {
                break;
            }
        }
        Ok(())
    }

    fn scan_defect(&self, what: &str) {
        let n = SCAN_REAL_DIFFS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        eprintln!(
            "phitex: page break at node {}: {what} ({n} so far)",
            self.cx.node().raw()
        );
    }

    /// Insertion class `n`'s registers (§1008) as they reach the node
    /// `at` ([`phitex_defuse::Ctx::use_field_at`]), what the page
    /// builder read where the break was found; as they reach this run
    /// where `at` is gone or is this run's own node.
    fn insert_class_at(&mut self, n: u16, at: phitex_defuse::NodeId) -> R<InsertClass> {
        if at == self.cx.node() {
            return self.insert_class(n);
        }
        let v = self.v;
        let mut xs = Vec::with_capacity(4);
        let bf = self.box_field(i64::from(n));
        let mut fields = vec![bf];
        for r in 0..3 {
            let l = self.reg_loc(r, i32::from(n));
            fields.push(self.loc_field(l));
        }
        for f in fields {
            match self.cx.use_field_at(f, Some(at))? {
                phitex_defuse::FieldAt::Def(x, _) => xs.push(Some(x)),
                phitex_defuse::FieldAt::Unset => xs.push(None),
                phitex_defuse::FieldAt::Gone => return self.insert_class(n),
            }
        }
        // (a box register's value: `[shift, box]`, the box unit when void)
        let b = xs[0]
            .and_then(|x| list_of(v, x).get(1).copied())
            .filter(|&b| b != ValueId::UNIT);
        Ok(InsertClass {
            box_height_plus_depth: b.map(|b| nodes::box_dim(v, b, 3) + nodes::box_dim(v, b, 2)),
            box_is_hbox: b.is_some_and(|b| nodes::box_is_h(v, b)),
            count: crate::val::i32_of(v, xs[1]),
            dimen: crate::val::i32_of(v, xs[2]),
            skip: glue_of(v, xs[3]),
        })
    }

    /// A finisher's arguments in full from its page's break (`b`) and its
    /// position (`pos`): `[j, data, carried, locals, rest, piece, k, reads,
    /// defs, hint, normal, special]`, the page assembled
    /// ([`Run::assemble_page`]); the check of a page finishing inside the
    /// piece with commands after it ([`Run::check_cut`]) from the piece's
    /// unit. Its position where the page cannot be assembled.
    fn finisher_args(&mut self, b: &[ValueId], pos: &[ValueId]) -> R<Vec<ValueId>> {
        let v = self.v;
        let Some((_, mut fire, own)) = self.assemble_page(b, None, false)? else {
            return Ok(pos.to_vec());
        };
        self.fin_box = fire.b255.take();
        let mut pid = b.get(3).map_or(-2, |&x| int_of(v, x));
        // (a page firing in the rest of a piece carried in a
        // continuation's material: checked against that piece, its
        // origin, `place_break`)
        let own = match b.get(6).filter(|&&o| pid == -2 && o != ValueId::UNIT) {
            Some(&o) => {
                let xs = list_of(v, o);
                pid = xs.first().map_or(-2, |&x| int_of(v, x));
                vec![
                    xs.first().copied().unwrap_or(ValueId::UNIT),
                    xs.get(1).copied().unwrap_or(ValueId::UNIT),
                    ValueId::UNIT,
                    xs.get(2).copied().unwrap_or(ValueId::UNIT),
                    xs.get(3).copied().unwrap_or(ValueId::UNIT),
                    xs.get(4).copied().unwrap_or(ValueId::UNIT),
                    xs.get(5).copied().unwrap_or(ValueId::UNIT),
                ]
            }
            None => own,
        };
        let a = |i: usize| own.get(i).copied().unwrap_or(ValueId::UNIT);
        let at_end = pid == -2 || i64::from(fire.k) == int_of(v, a(1));
        let locals = if pid == -2 {
            Vec::new()
        } else {
            list_of(v, a(5))
        };
        let mut args = vec![
            pos.first().copied().unwrap_or(ValueId::UNIT),
            fire.data,
            v.list(Vec::new()),
            v.list(locals),
        ];
        let mut cuttable = false;
        if crate::exec::obs_flag!("PHITEX_CAND_TRACE") {
            eprintln!(
                "fin-place: page {} piece {} candidate {} last {} inside {}",
                pos.first().map_or(-1, |&x| int_of(v, x)),
                pid.cast_unsigned(),
                fire.k,
                int_of(v, a(1)),
                !at_end
            );
        }
        if at_end {
            args.extend([ValueId::UNIT; 5]);
        } else {
            // (`[names, cuttable]` at the candidate)
            let at_k = list_of(v, a(4))
                .get(usize::try_from(fire.k).unwrap_or(usize::MAX))
                .map(|&x| list_of(v, x))
                .unwrap_or_default();
            let defs_k = at_k.first().copied().unwrap_or(ValueId::UNIT);
            cuttable = at_k.get(1).is_some_and(|&x| int_of(v, x) != 0);
            args.extend([
                v.list(Vec::new()),
                v.int(pid),
                v.int(i64::from(fire.k)),
                a(3),
                defs_k,
            ]);
        }
        args.extend(pos.get(9..12).unwrap_or_default().iter().copied());
        args.resize(12, ValueId::UNIT);
        args.push(v.int(i64::from(cuttable)));
        Ok(args)
    }

    /// The piece's units: its vertical profile ([`crate::vprof::VPROF`])
    /// and its items ([`PITEMS`]), at its position.
    fn contribute_units(
        &mut self,
        raw: &[ValueId],
        last: Option<u32>,
        reads: ValueId,
        defs: ValueId,
        locals: Vec<ValueId>,
        cut: bool,
    ) {
        let v = self.v;
        let ints = crate::vprof::encode(v, raw);
        crate::vprof::count_unit(&ints);
        let pid = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        self.contribute(
            crate::vprof::VPROF,
            v.seq_unit(crate::vprof::unit(v, pid, &ints)),
        );
        let it = v.list(vec![
            v.int(pid),
            v.int(last.map_or(-1, i64::from)),
            v.list(raw.to_vec()),
            reads,
            defs,
            v.list(locals),
            // (cut at `last` where a finisher asked: the finisher of the
            // page firing there asks again, [`Run::finisher_args`])
            v.int(i64::from(cut)),
        ]);
        self.contribute(PITEMS, v.seq_unit(it));
    }

    /// What the piece leaves the next one of the main list
    /// ([`crate::build::MAIN_PD`], [`crate::build::MAIN_SF`],
    /// [`crate::build::MAIN_LAST`]): each defined where its items change
    /// it, from its items alone where they decide it (equal folded onto
    /// the document's start and onto a start no document has), else
    /// folded onto the one value reaching the piece, read: a paragraph's
    /// end is its own, and the pieces read one another's only where an
    /// item leaves the previous standing (spec-redesign §1, exec row).
    fn define_main_tail(&mut self, raw: &[ValueId]) -> R<()> {
        use crate::build::{MAIN_LAST, MAIN_PD, MAIN_SF, MAIN_SINCE, MainTail};
        let v = self.v;
        if raw.is_empty() {
            return Ok(());
        }
        let p = MainTail::probe(v, raw.len());
        let a = MainTail::START.after(v, raw);
        let b = p.after(v, raw);
        // (each field: the items' own value where they decide it;
        // nothing where they leave it as it was, the definition before
        // reaching on; else the value reaching here, read, folded)
        if a.pd == b.pd {
            self.cx
                .define_with(v.field(MAIN_PD), v.int(i64::from(a.pd)), Locality::Global);
        } else if b.pd != p.pd {
            let pd = self
                .eng_read(v.field(MAIN_PD))?
                .map_or(MainTail::START.pd, |x| {
                    i32::try_from(int_of(v, x)).unwrap_or(MainTail::START.pd)
                });
            let t = MainTail {
                pd,
                ..MainTail::START
            }
            .after(v, raw);
            self.cx
                .define_with(v.field(MAIN_PD), v.int(i64::from(t.pd)), Locality::Global);
        }
        if a.sf == b.sf {
            self.cx
                .define_with(v.field(MAIN_SF), v.int(i64::from(a.sf)), Locality::Global);
        } else if b.sf != p.sf {
            let sf = self
                .eng_read(v.field(MAIN_SF))?
                .map_or(1000, |x| i32::try_from(int_of(v, x)).unwrap_or(1000));
            let t = MainTail {
                sf,
                ..MainTail::START
            }
            .after(v, raw);
            self.cx
                .define_with(v.field(MAIN_SF), v.int(i64::from(t.sf)), Locality::Global);
        }
        // (the last item and the contributions since the last candidate
        // apart: a piece that changes one and leaves the other reads
        // nothing of the piece before)
        let last_own = a.last == b.last;
        let since_own = a.since == b.since;
        if last_own {
            self.cx
                .define_with(v.field(MAIN_LAST), a.last_value(), Locality::Global);
        }
        if since_own {
            self.cx
                .define_with(v.field(MAIN_SINCE), a.since_value(v), Locality::Global);
        }
        let last_moves = !last_own && b.last != p.last;
        let since_moves = !since_own && b.since != p.since;
        // (the count moved by the items' own number alone, no candidate
        // among them and nothing before reached into, `\\bigskip` after
        // a paragraph: at least that many, the count before not read,
        // `MainTail::since_at_least`)
        let adds = since_moves && !last_moves && b.since.checked_sub(p.since) == Some(a.since);
        if adds {
            self.cx.define_with(
                v.field(MAIN_SINCE),
                MainTail::since_at_least(v, a.since),
                Locality::Global,
            );
        }
        if last_moves || (since_moves && !adds) {
            let last = self.eng_read(v.field(MAIN_LAST))?;
            let since = self.eng_read(v.field(MAIN_SINCE))?;
            let (last, since, at_least) = MainTail::of_last(v, last, since);
            let t = if at_least {
                // (a deletion reaching back past a count known only as
                // a lower bound: the list itself, a real dependency)
                self.main_tail_full(raw)?
            } else {
                let pd = self
                    .eng_read(v.field(MAIN_PD))?
                    .map_or(MainTail::START.pd, |x| {
                        i32::try_from(int_of(v, x)).unwrap_or(MainTail::START.pd)
                    });
                MainTail {
                    last,
                    since,
                    pd,
                    ..MainTail::START
                }
                .after(v, raw)
            };
            if last_moves {
                self.cx
                    .define_with(v.field(MAIN_LAST), t.last_value(), Locality::Global);
            }
            if since_moves {
                self.cx
                    .define_with(v.field(MAIN_SINCE), t.since_value(v), Locality::Global);
            }
        }
        Ok(())
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

    /// The page parameters (§980) reaching the position just after node
    /// `at`, as a `T_PAGE_PARAMS` marker: what a piece's `[tag]` marker
    /// means, read where the piece is (the record `pagepar`, or its cells
    /// where it is not defined there, as [`Run::eng_read`] reads them).
    fn page_params_at(&mut self, at: i64) -> R<ValueId> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes()));
        let b = crate::fields::B_PAGE;
        let rf = self.ops.fields.slot_field(crate::fields::record_loc(b));
        let n = crate::fields::record_len(b);
        let cells = match self.cx.use_field_at(rf, Some(at))? {
            phitex_defuse::FieldAt::Def(x, _) => Some(list_of(v, x)).filter(|xs| xs.len() == n),
            phitex_defuse::FieldAt::Gone => return self.page_params_marker(),
            phitex_defuse::FieldAt::Unset => None,
        };
        let absent = crate::fields::record_absent(v);
        let mut xs = Vec::with_capacity(n);
        for i in 0..n {
            let x = if let Some(c) = &cells {
                (c[i] != absent).then_some(c[i])
            } else {
                let loc =
                    crate::runstate::Loc::try_from(crate::fields::record_cell(b, i)).unwrap_or(0);
                let f = self.loc_field(loc);
                match self.cx.use_field_at(f, Some(at))? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    _ => None,
                }
            };
            xs.push(x);
        }
        let int = |x: Option<ValueId>| v.int(i64::from(crate::val::i32_of(v, x)));
        Ok(v.list(vec![
            v.int(nodes::T_PAGE_PARAMS),
            int(xs[0]),
            int(xs[1]),
            glue_value(v, &glue_of(v, xs[2])),
            int(xs[3]),
        ]))
    }

    /// `raw`'s `[tag]` page parameter markers, of piece `pid`, given the
    /// parameters reaching it ([`Run::page_params_at`]), and its `[tag,
    /// n]` insertion class markers the class's registers reaching it
    /// ([`Run::insert_class_at`]).
    fn resolve_params_of(&mut self, pid: i64, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let mut m = None;
        let mut classes: Vec<(u16, ValueId)> = Vec::new();
        for x in raw.iter_mut() {
            if nodes::tag(v, *x) == nodes::T_INS_PARAMS && list_of(v, *x).len() == 2 {
                // (§1008: the class's registers as they reach the piece)
                let n = u16::try_from(int_at(v, *x, 1)).unwrap_or(0);
                *x = if let Some(&(_, c)) = classes.iter().find(|c| c.0 == n) {
                    c
                } else {
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
                    let c = self.insert_class_at(n, at)?;
                    let c = self.ins_marker(n, &c);
                    classes.push((n, c));
                    c
                };
                continue;
            }
            if nodes::tag(v, *x) == nodes::T_PAGE_PARAMS && list_of(v, *x).len() == 1 {
                let mm = if let Some(mm) = m {
                    mm
                } else {
                    let mm = self.page_params_at(pid)?;
                    m = Some(mm);
                    mm
                };
                *x = mm;
            }
        }
        Ok(())
    }

    /// A profile's `[tag]` page parameter items, of piece `pid`, given the
    /// parameters reaching it, and its `[tag, n]` insertion class items
    /// the class's registers reaching it: what [`Run::resolve_params_of`]
    /// gives the piece's items (a marker resolved where the build reads
    /// it would read the registers at a later piece, past a finisher
    /// that changed them, and the scan's decision would part from the
    /// items')
    fn resolve_prof_of(&mut self, pid: i64, items: &mut [crate::vprof::PItem]) -> R<()> {
        let mut m = None;
        let mut classes: Vec<(u16, InsertClass)> = Vec::new();
        for x in items.iter_mut() {
            if let crate::vprof::PItem::InsParams(n, c @ None) = x {
                let n = *n;
                *c = Some(if let Some(&(_, k)) = classes.iter().find(|k| k.0 == n) {
                    k
                } else {
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
                    let k = self.insert_class_at(n, at)?;
                    classes.push((n, k));
                    k
                });
                continue;
            }
            if let crate::vprof::PItem::Params(p @ None) = x {
                let pp = if let Some(pp) = m {
                    pp
                } else {
                    let mm = self.page_params_at(pid)?;
                    let pp = params_of(self.v, mm);
                    m = Some(pp);
                    pp
                };
                *p = Some(pp);
            }
        }
        Ok(())
    }

    /// What a finisher's check needs when a page can finish inside the
    /// piece with commands after it ([`Run::check_cut`]): the locations
    /// the run read from the engine with the values it read, and per
    /// candidate the names the run defined after it (and its local
    /// definitions at any time: a scope closing after the candidate hides
    /// them from a finisher placed after the piece). Units otherwise.
    fn check_data(&mut self, last: Option<u32>) -> (ValueId, ValueId) {
        let v = self.v;
        let n = self.cand_marks.len();
        let inside_possible = !(n == 1 && last == Some(0)) && n > 0;
        if !inside_possible {
            return (ValueId::UNIT, ValueId::UNIT);
        }
        let mut rs: Vec<ValueId> = self
            .st
            .read_values()
            .into_iter()
            .map(|(l, x)| {
                let f = self.loc_field(l);
                v.list(vec![f.0, opt_val(x)])
            })
            .collect();
        // (a meaning read by its class only: no value a routine's
        // definition can equal, so any global definition of it by the
        // routine counts as a change, [`Run::check_cut`])
        let class = v.str("\u{1}class");
        rs.extend(
            self.class_read_fields()
                .into_iter()
                .map(|f| v.list(vec![f.0, class])),
        );
        // (by field, never by location: a location's number is the order
        // the job's runs first met its field in, which the threads decide,
        // so a value holding them in that order differs between two
        // builds of one text)
        rs.sort_by_key(|x| list_of(v, *x).first().map(|f| f.raw()));
        let gids = self.st.gid_mark();
        let locals: Vec<(Loc, Field)> = self
            .st
            .defined_since(v, 0, gids)
            .into_iter()
            .filter(|(_, _, l)| *l == Locality::Local)
            .map(|(l, _, _)| (l, self.loc_field(l)))
            .collect();
        let marks = self.cand_marks.clone();
        // (a location defined after a candidate with its value there,
        // where the run had it: the page's routine, which runs at the
        // candidate, reads that, [`Run::finish_node`]'s preloads; no
        // value: the routine reading it after the run cuts the piece,
        // [`Run::check_cut`])
        let at = self.st.at_candidates(v);
        let mut per = Vec::with_capacity(n);
        for (i, m) in marks.iter().enumerate() {
            if Some(m.k) == last {
                per.push(v.list(vec![v.list(Vec::new()), v.int(0)]));
                continue;
            }
            // (`[value]` or `[]` (absent) where the run had it; one it had
            // neither read nor defined before is what reached the piece,
            // [`BEFORE_PIECE`], the finisher reads it there)
            let kv_of = |l: Loc| match at.get(i).and_then(|a| a.get(&l).copied()) {
                Some(Some(x)) => v.list(x.into_iter().collect()),
                Some(None) => v.str(BEFORE_PIECE),
                None => ValueId::UNIT,
            };
            let mut names: Vec<(ValueId, bool, ValueId)> = locals
                .iter()
                .map(|&(l, f)| (f.0, false, kv_of(l)))
                .collect();
            for (l, _, _) in self.st.defined_since(v, m.stamp, m.gids) {
                let stands = self.st.def_stands(l);
                names.push((self.loc_field(l).0, stands, kv_of(l)));
            }
            // (a binding local to a group of the run open at the
            // candidate, restored at the group's close after it: the
            // routine runs inside that group and sees the binding, which
            // the run's end no longer holds, §1012)
            if let Some(a) = at.get(i) {
                let mut restored: Vec<Loc> = a
                    .iter()
                    .filter_map(|(&l, &x)| x.map(|x| (l, x)))
                    .filter(|&(l, x)| self.st.id_of(l, v) != x)
                    .map(|(l, _)| l)
                    .collect();
                restored.sort_unstable();
                for l in restored {
                    names.push((self.loc_field(l).0, false, kv_of(l)));
                }
            }
            names.extend(
                self.streams_since(m.contribs)
                    .into_iter()
                    .map(|f| (f.0, false, ValueId::UNIT)),
            );
            names.sort_unstable_by_key(|x| (x.0.raw(), !x.1));
            names.dedup_by_key(|x| x.0);
            // (with whether the run could end there: a finisher of a page
            // firing at a candidate where it could not runs its routine
            // as if there, and the piece's definitions after it stand)
            let names = v.list(
                names
                    .into_iter()
                    .map(|(f, g, kv)| v.list(vec![f, v.int(i64::from(g)), kv]))
                    .collect(),
            );
            per.push(v.list(vec![names, v.int(i64::from(m.cuttable))]));
        }
        (v.list(rs), v.list(per))
    }

    /// A piece's end when no page needs finding in its own run: its
    /// insertion class markers given the registers where it ends, its
    /// units, its scan step placed after it. Its page parameter markers
    /// stay `[tag]`: the steps read the parameters reaching it where it
    /// is ([`Run::page_params_at`]), so a piece reads none (an output
    /// routine setting `\vsize` would refute every piece after it).
    fn fold_apart(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        let last = self.last_candidate();
        // (what its commands read, before the reads that resolve its
        // markers: those are no command's)
        let (reads, defs) = self.check_data(last);
        // (its insertion class markers that are not its own stay `[tag,
        // n]`: read where the piece is by the step that folds it,
        // [`Run::resolve_params_of`], as its page parameters are; a
        // piece with a footnote does not wait for the routine before it
        // that empties the class's box)
        self.pin_reaching_page_params(&mut raw)?;
        self.contribute_units(&raw, last, reads, defs, Vec::new(), false);
        // (the scan is a block's: the block's last piece places it, and
        // the document's last, `\end`; the pieces before fold in its
        // window)
        if block_end(self.cx.node().raw()) || raw.iter().any(|&x| nodes::tag(v, x) == nodes::T_END)
        {
            self.cx.follow(self.ops.scan, vec![v.int(-3)]);
        }
        Ok(())
    }

    /// A piece cut where a finisher asked ([`Run::check_cut`],
    /// `cut:<node>`): the first pass that passed the candidate goes
    /// again stopped there ([`Run::finish_candidate`]); the second gives
    /// its items up to the candidate as its units, places its scan step
    /// (the page is the scan's to find, as for any piece: the same nodes
    /// whichever run finds it) and the resume piece after them, which
    /// runs the rest from the continuation ([`RESUME`],
    /// [`Run::cont_encode`]). Nothing here reads the page: a cut at a
    /// candidate where no page finishes any more splits the piece and
    /// changes nothing else. A stop here parks the run: the end is a
    /// command of its own ([`Run::begin_command`]).
    fn fold_here(&mut self, raw: Vec<ValueId>) -> R<()> {
        let t0 = std::time::Instant::now();
        let r = self.fold_here_(raw);
        phitex_pdf::phase::add(18, t0);
        r
    }

    fn fold_here_(&mut self, mut raw: Vec<ValueId>) -> R<()> {
        let v = self.v;
        // (its input read up to the candidate: the siblings it takes are
        // known, and so are the fields it defines)
        let n = self.claim_count();
        if n != 0 {
            self.cx.claim(n);
        }
        let mut defs = self.published_fields();
        defs.extend(PAGE_FIELDS.map(|f| v.field(f)));
        self.cx.narrow(MayDefine::Names(defs));
        self.begin_command();
        self.define_main_tail(&raw)?;
        let Some(k) = self.cut else { return Ok(()) };
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
            eprintln!("  cut {}: at candidate {k}", self.cx.node().raw());
        }
        self.pin_reaching_page_params(&mut raw)?;
        let (cont, locals) = self.cont_encode()?;
        self.contribute_units(&raw, Some(k), ValueId::UNIT, ValueId::UNIT, locals, true);
        let flags = i64::from(self.top) | (i64::from(self.doc) << 1);
        let pid = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        // (the scan step after a cut: the piece is its block's last)
        self.cx.follow(self.ops.scan, vec![v.int(-3)]);
        // (the siblings the rest of the piece will take where the piece's
        // text says so: what it takes less what it took before the cut;
        // the nodes after the resume run on that instead of waiting for
        // the resume's run, and a run that takes others restructures
        // them; unknown, -1: decided by the resume's run)
        let rest = self
            .claim_guess()
            .map_or(-1, |k| i64::try_from(k.saturating_sub(n)).unwrap_or(-1));
        self.cx.define_with(
            v.field(RESUME),
            v.list(vec![
                cont,
                pid,
                v.int(flags),
                v.int(rest),
                v.int(i64::from(k)),
            ]),
            Locality::Global,
        );
        // (the rest of the piece after its scan step: the finisher and
        // continuation of a page firing at the cut, or at a candidate of
        // the piece before it, the step places right after itself, so
        // before the rest, which reads what their routines leave; a
        // `\write18`'s tool node the run ended at right before it,
        // `Run::tool_command`)
        if let Some(t) = self.tool_cut {
            self.cx.follow(self.ops.toolnode, list_of(v, t));
        }
        self.cx.follow(
            self.ops.resume,
            vec![ValueId::UNIT, v.int(flags), ValueId::UNIT, v.int(rest)],
        );
        Ok(())
    }

    /// A finisher whose page's break is inside its piece (at a
    /// candidate the piece could not end at, a group or conditional of
    /// its own open there): what the piece defined after the candidate
    /// put back where the routine left it as it was ([`Run::check_cut`]).
    fn split_piece(&mut self, args: &[ValueId]) -> R<()> {
        let Some((_, _, inside)) = self.fin.as_ref().and_then(|c| c.brk) else {
            return Ok(());
        };
        if inside {
            let _ = self.check_cut(args)?;
        }
        Ok(())
    }

    /// A finisher placed after the piece its page finished inside of
    /// (arguments past the fourth: the rest of the piece's items, the
    /// piece's node, the candidate, what the piece read, what it defined
    /// after the candidate): whether the routine, run here instead of at
    /// the candidate, changed what the piece's commands after it saw or
    /// saw what they defined: a piece ends at every candidate it can
    /// end at ([`Run::finish_candidate`]), so this is a break where it
    /// could not, and is said (`PHITEX_TEX_TRACE`); the piece's
    /// definitions after the candidate the routine left as they were
    /// there are put back for the routine's own reads.
    fn check_cut(&mut self, args: &[ValueId]) -> R<Option<(i64, i64)>> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        if args.len() < 9 || a(5) == ValueId::UNIT {
            return Ok(None);
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
        read.extend(self.class_read_fields().into_iter().map(|f| f.0));
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
        let mut kept = Vec::new();
        for e in list_of(v, defs) {
            let xs = list_of(v, e);
            let Some(&f) = xs.first() else { continue };
            let stands = xs.get(1).is_some_and(|&g| int_of(v, g) != 0);
            if why.is_none() && read.contains(&f) {
                why = Some(format!("after {}", str_of(v, f)));
            }
            if let Some(&(_, y)) = gdefs.iter().find(|(g, _)| *g == f)
                && stands
            {
                // (the routine left it as it was at the candidate: its
                // definition changes nothing there)
                let at_k = xs
                    .get(2)
                    .filter(|&&kv| kv != ValueId::UNIT)
                    .map(|&kv| list_of(v, kv).first().copied());
                if at_k == Some(Some(y)) {
                    kept.push(Field(f));
                } else {
                    restore.push(Field(f));
                }
            }
        }
        // (a piece cut at the candidate defines nothing after it: nothing
        // to give way to, but where the routine left the location as it
        // was there; one that cannot end there, a group or conditional
        // of its own open, runs through and is never asked)
        let cuttable = args.get(12).is_some_and(|&x| int_of(v, x) != 0);
        if why.is_some() {
            restore.clear();
        }
        restore.extend(kept);
        if crate::exec::obs_flag!("PHITEX_CAND_TRACE") {
            eprintln!(
                "cut-check: finisher {} piece {} candidate {} cuttable {cuttable} why {:?}",
                self.cx.node().raw(),
                int_of(v, pid).cast_unsigned(),
                int_of(v, k),
                why
            );
        }
        if !cuttable {
            why = None;
        }
        for f in restore {
            let x = self.eng_read(f)?;
            let loc = self.loc_of(f);
            self.set_loc(loc, crate::runstate::Entry::raw(x), true);
        }
        let Some(why) = why else { return Ok(None) };
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") || crate::exec::prof_flag!("PHITEX_TEX_TIME")
        {
            eprintln!(
                "  routine changes a piece's rest it cannot cut: piece {} at candidate {} ({why})",
                int_of(v, pid),
                int_of(v, k)
            );
        }
        Ok(Some((int_of(v, pid), int_of(v, k))))
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
        let mut head = raw[..last].to_vec();
        self.resolve_page_params(&mut head)?;
        let s = ScanIn::decode(v, self.eng_read(v.field(SCAN))?);
        let units = v.seq_to_vec(self.cx.acc_range(v.field(crate::vprof::VPROF), s.anchor)?);
        let dead = self.cint(crate::fields::cell::DEADCYCLES)?;
        let mut st = s.state(v);
        let mut items = vec![crate::vprof::decode(&s.prof)];
        for &u in &units {
            let (pid, ints) = crate::vprof::unit_parts(v, u);
            let mut x = crate::vprof::decode(&ints);
            self.resolve_prof_of(pid, &mut x)?;
            items.push(x);
        }
        items.push(crate::vprof::decode(&crate::vprof::encode(v, &head)));
        let mut rd = RunReaching(self, None);
        for x in &items {
            if st.fold_items(x, dead, &mut rd).is_some() {
                break;
            }
        }
        if st.pb.contents == Contents::Empty {
            return Ok(if m == 0 { MAX_DIMEN } else { 0 });
        }
        Ok(st.pb.so_far[usize::try_from(m).unwrap_or(0).min(7)])
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
        // (spec-final A4: `\box255` from the builder's native box, its
        // line boxes their own values: a box made once, here)
        let b255 = match self.fin_box.take() {
            Some((b, side)) => nodes::encode_box(v, &b, &side),
            None => a(0),
        };
        let (inserts, marks, held) = (list_of(v, a(1)), list_of(v, a(2)), list_of(v, a(3)));
        let output_penalty = data.get(4).map_or(0, |&x| int_of(v, x));
        if crate::exec::obs_flag!("PHITEX_FIRE_TEXT") {
            // (the page's first letters, to tell the pages apart)
            fn text(list: &[Node], out: &mut String) {
                for n in list {
                    if out.len() >= 60 {
                        return;
                    }
                    match n {
                        Node::Char { ch, .. } => {
                            out.push(char::from_u32(u32::from(*ch)).unwrap_or('?'));
                        }
                        Node::Lig(l) => {
                            for &c in &l.original {
                                out.push(char::from_u32(u32::from(c)).unwrap_or('?'));
                            }
                        }
                        Node::Box(b) => {
                            text(&b.list, out);
                            out.push(' ');
                        }
                        Node::Glue(_) => out.push(' '),
                        _ => {}
                    }
                }
            }
            let mut t = String::new();
            if let Some(Node::Box(b)) = nodes::decode(v, b255, &mut Side::default()) {
                text(&b.list, &mut t);
            }
            eprintln!(
                "fire-text: node {} page {} penalty {output_penalty} held {} spec {}: {t:?}",
                self.cx.node().raw(),
                self.fin.as_ref().map_or(-1, |c| c.j),
                held.len(),
                self.cx.is_speculative()
            );
        }
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
        for (name, m) in [
            ("mark:top", top),
            ("mark:first", first),
            ("mark:bot", new_bot),
        ] {
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
            let l = self.box_loc(n);
            self.set_cell(l, v.list(vec![v.int(0), b]), true);
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
    /// happen now (§1366–§1374). It defines nothing for the text after
    /// (spec-final §3.2: no shipped mark).
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
        let timed = crate::exec::prof_flag!("PHITEX_TEX_TIME");
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
        // (spec-final §3.2: the page's own font usage, by id; the end
        // node resolves names and files from the fonts' fields)
        let mut ids: Vec<u32> = view.fonts.ids();
        ids.sort_unstable();
        let fonts: Vec<ValueId> = ids.iter().map(|&f| v.int(i64::from(f))).collect();
        let cs = v.list(counts.iter().map(|&c| v.int(i64::from(c))).collect());
        let pdf = self.pdf_page_params(pos.pdf)?;
        let page = v.list(vec![b, cs, v.list(fonts), pdf, v.list(pdf_data)]);
        let t_fonts = t0.elapsed();
        phitex_pdf::phase::add(3, t_p3);
        let t_p4 = std::time::Instant::now();
        // (the page's hash: its value, what it is made of, known on the
        // write side every round; its PDF side runs once, at the end)
        if let Node::Box(bx) = &page_box
            && pos.pdf
        {
            crate::pdfout::ship(self, page, bx, &side, &ids)?;
        }
        let shown = Some(page.raw());
        phitex_pdf::phase::add(4, t_p4);
        if timed {
            crate::exec::later(format!(
                "shipout: decode_us {} positions_us {} whatsits_us {} fonts_us {} pdf_us {}",
                t_decode.as_micros(),
                t_pos.checked_sub(t_decode).unwrap().as_micros(),
                t_whats.checked_sub(t_pos).unwrap().as_micros(),
                t_fonts.checked_sub(t_whats).unwrap().as_micros(),
                t0.elapsed().checked_sub(t_fonts).unwrap().as_micros()
            ));
        }
        let su = v.seq_unit(page);
        self.contribute("pages", su);
        if let Some(shown) = shown {
            let h = v.seq_unit(v.int(i64::from_ne_bytes(shown.to_ne_bytes())));
            self.contribute("pagehash", h);
        }
        self.log("shipend", "]")?;
        self.set_cell(crate::fields::cell::DEADCYCLES, v.int(0), true);
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
            let f = self.ops.fields.param(v, p);
            let toks = toks_of(v, self.get_f(f)?);
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

fn file_frames(v: &Values, f: &crate::run::FileRest) -> Vec<ValueId> {
    // (a file read as units, not read yet: the file as its units, a lazy
    // file again where the frames are decoded, `crate::run::units_frame`)
    if let Some(u) = crate::run::units_frame(v, f) {
        return vec![u];
    }
    vec![v.list(vec![
        v.str("f"),
        f.id,
        v.int(i64::try_from(f.off).unwrap_or(0)),
        f.name,
    ])]
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
        // (each token level's file line: of the innermost file read
        // below it, §304, a file it is inside staying open until the
        // level is read to its end; below them all, the text's)
        let here_of = |c: Option<crate::run::FileLine>| -> Vec<ValueId> {
            match c {
                Some(crate::run::FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
                Some(crate::run::FileLine::Piece(name)) => vec![name, v.int(-1)],
                None => Vec::new(),
            }
        };
        let base = self.text_line();
        let mut cur = base;
        let mut level_here = Vec::with_capacity(self.input.levels.len());
        for l in &self.input.levels {
            match l.kind {
                crate::input::kind::TOKS => {}
                crate::input::kind::LINE => {
                    if let Some(c) = crate::run::line_of(self.input.line(*l)) {
                        cur = Some(c);
                    }
                }
                _ => {
                    cur = Some(crate::run::FileLine::Known(i64::from(
                        self.input.file(*l).line,
                    )));
                }
            }
            level_here.push(here_of(cur));
        }
        let toks_frame = |toks: &[crate::val::Tok], here: &[ValueId]| {
            let mut t = vec![v.str("t"), toks_value(v, toks)];
            t.extend(here.iter().copied());
            v.list(t)
        };
        let base_here = here_of(base);
        let mut frames = Vec::new();
        for (i, l) in self.input.levels.iter().enumerate().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    let r = self.input.arena.get(l.rest());
                    if !r.is_empty() {
                        frames.push(toks_frame(r, &level_here[i]));
                    }
                }
                crate::input::kind::LINE => {
                    if let Some(f) = line_frame(v, self.input.line(*l)) {
                        frames.push(f);
                    }
                }
                _ => frames.extend(file_frames(v, self.input.file(*l))),
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
                        frames.push(toks_frame(t, &base_here));
                    }
                }
                Seg::File(f) => frames.extend(file_frames(v, f)),
            }
        }
        let nest: Vec<ValueId> = self
            .nest
            .iter()
            .map(|l| {
                let (lang, packed) = l.par.map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| {
                    (v.int(a), v.int(b))
                });
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
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} at a cut: {name}: {} (open groups {open:?})",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        let c = self.st.cont(v, &open);
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            let hit = c.slots.iter().find(|s| s.loc == l);
            eprintln!(
                "cont {} encodes {name}: {:?}",
                self.cx.node().raw(),
                hit.map(|s| (
                    s.cur.map(|x| v.render(x)),
                    s.loaded,
                    s.save_gid,
                    s.ldef,
                    s.group_local
                ))
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
                let after = v.int(self.made_after(s.ldef.map(|d| d.0)));
                overrides.push(v.list(vec![f.0, opt_val(base), after]));
                locals.push(v.list(vec![f.0, opt_val(s.cur), after]));
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
            // (the run's mutation count is compared only with itself
            // within one command, `Run::rewind`: not carried, an edit
            // before the cut would change the continuation for nothing)
            v.int(0),
            v.int(i64::from(self.read_back_done)),
            v.int(i64::from(self.astate.is_some())),
            v.int(self.astate.unwrap_or(0)),
            v.int(self.aopen),
            v.int(i64::from(self.claim_rest)),
            v.int(i64::from(self.hint_void)),
            v.int(i64::from(self.no_pull)),
            v.int(i64::from(self.rec_load)),
            v.int(i64::from(self.cond_loaded || self.cond_stop)),
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
        if crate::exec::obs_flag!("PHITEX_CONT_PARTS") {
            let xs = list_of(v, cont);
            let st = list_of(v, xs[4]);
            eprintln!(
                "cont-parts {} k {}: {:?} state {:?} misc {:?}",
                self.cx.node().raw(),
                self.cand,
                xs.iter().map(|x| x.raw() % 100_000).collect::<Vec<_>>(),
                st.iter().map(|x| x.raw() % 100_000).collect::<Vec<_>>(),
                list_of(v, xs[5])
                    .iter()
                    .map(|&x| int_of(v, x))
                    .collect::<Vec<_>>()
            );
        }
        Ok((cont, locals))
    }

    /// A resume piece's start: the continuation `x` ([`Run::cont_encode`])
    /// loaded; the base-level values the open groups' bindings hide
    /// checked against the engine's reaching here (a global definition
    /// between the cut and here, the finisher's, replaces the binding and
    /// makes the groups' saves inert, §283).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn cont_load(&mut self, x: ValueId, pid: ValueId) -> R<()> {
        let v = self.v;
        let xs = list_of(v, x);
        let a = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
        let frames = self.resolve_units(&[a(0)])?;
        self.input.text = crate::run::decode_segs(v, &frames);
        // (where each segment was read: a token frame's file line)
        self.cont_lines = frames
            .first()
            .map(|&l| list_of(v, l))
            .unwrap_or_default()
            .iter()
            .filter(|&&f| crate::run::first_seg(v, &[v.list(vec![f])]).is_some())
            .map(|&f| crate::run::frame_line(v, f))
            .collect();
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
        if let Some(name) = crate::exec::obs_str!("PHITEX_CONT_SHOW") {
            let l = self.loc_of(v.field(name));
            eprintln!(
                "cont {} loaded: {name}: {}",
                self.cx.node().raw(),
                self.st.debug_marks(l)
            );
        }
        // the fields the finishers between the cut and this resume
        // defined globally (`gdefs`, the window after the cutting node)
        let gdefs: Vec<GDef> = match Some(pid).filter(|&p| p != ValueId::UNIT) {
            Some(p) if self.cx.op() == self.ops.resume => self.gdefs_after(int_of(v, p))?,
            _ => Vec::new(),
        };
        let piece = int_of(v, pid);
        for e in list_of(v, a(6)) {
            let ys = list_of(v, e);
            if ys.len() < 2 {
                continue;
            }
            let f = Field(ys[0]);
            let x0 = some(ys[1]);
            let x1 = self.eng_read(f)?;
            let after = ys.get(2).map_or(-1, |&c| int_of(v, c));
            // (§279: a global definition replaces the group's local value
            // whatever it defines, the value outside the group too: by
            // the definition, not the value; a routine's at a candidate
            // the binding was made after left it standing)
            if replaced(&gdefs, f, piece, after) {
                let loc = self.loc_of(f);
                self.st.override_global(loc, x1);
                continue;
            }
            if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
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
        self.muts = 0;
        self.read_back_done = n(10) != 0;
        self.astate = (n(11) != 0).then(|| n(12));
        self.aopen = n(13);
        self.claim_rest = n(14) != 0;
        self.hint_void = n(15) != 0;
        self.no_pull = n(16) != 0;
        self.rec_load = n(17) != 0;
        self.cond_loaded = n(18) != 0;
        Ok(())
    }
}

/// A finisher's unit of [`GDEFS`]: the fields its routine defined
/// globally, and where its page fired, the piece and the candidate
/// ([`fin_at`]).
pub(crate) struct GDef {
    names: Vec<ValueId>,
    pid: i64,
    k: i64,
}

/// Where a finisher's page fired, from its position's break (argument
/// 12, [`Run::place_break`]): the piece and its candidate; a page firing
/// in a continuation's material, the piece that material came from and
/// a candidate after all of its own (`i64::MAX`).
fn fin_at(v: &Values, pos: &[ValueId]) -> (i64, i64) {
    let b = pos.get(12).map(|&x| list_of(v, x)).unwrap_or_default();
    let pid = b.get(3).map_or(-2, |&x| int_of(v, x));
    if pid != -2 {
        return (pid, b.get(4).map_or(i64::MAX, |&x| int_of(v, x)));
    }
    let origin = b
        .get(6)
        .filter(|&&o| o != ValueId::UNIT)
        .and_then(|&o| list_of(v, o).first().map(|&p| int_of(v, p)));
    (origin.unwrap_or(-2), i64::MAX)
}

/// A finisher's [`GDEFS`] unit: `[names, piece, candidate]`, nothing
/// where it defined nothing globally.
fn gdefs_unit(v: &Values, names: ValueId, (pid, k): (i64, i64)) -> ValueId {
    if names == ValueId::NIL {
        return ValueId::NIL;
    }
    v.seq_unit(v.list(vec![names, v.int(pid), v.int(k)]))
}

/// Whether a group-local binding of `f` the run of `piece` made after
/// its candidate `after` (-1: before its first, or before the run) is
/// replaced by a global definition of one of the routines `gdefs` (§279:
/// at every level): one of a page firing in another piece's text, or in
/// this piece's at a candidate the binding was made before.
fn replaced(gdefs: &[GDef], f: Field, piece: i64, after: i64) -> bool {
    gdefs
        .iter()
        .any(|g| g.names.contains(&f.0) && (g.pid != piece || after < g.k))
}

impl Run<'_, '_> {
    /// The [`GDEFS`] units of the finishers between node `pid` and this
    /// one.
    fn gdefs_after(&mut self, pid: i64) -> R<Vec<GDef>> {
        let v = self.v;
        let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
        let acc = self.fold_own(GDEFS, Some(at), None)?;
        Ok(v.seq_to_vec(acc)
            .into_iter()
            .filter_map(|e| match list_of(v, e)[..] {
                [n, p, k] => Some(GDef {
                    names: list_of(v, n),
                    pid: int_of(v, p),
                    k: int_of(v, k),
                }),
                _ => None,
            })
            .collect())
    }

    /// The last of the run's candidates passed before the definition
    /// with stamp `st` (-1: before the first, or none).
    fn made_after(&self, st: Option<u64>) -> i64 {
        let Some(st) = st else { return -1 };
        let n = self.cand_marks.iter().filter(|m| m.stamp < st).count();
        i64::try_from(n).unwrap_or(0) - 1
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
    /// Per field a run defined: the value the latest such run (by page)
    /// left, its page, and whether the run before that defined it to
    /// the same value ([`FinDefs::class_boxes`]).
    pub recent: Vec<(Field, ValueId, i64, bool)>,
    /// What the last run contributed to `gdefs` (the fields it defined
    /// globally, [`Run::contribute_global_defs`]): the next finisher's
    /// declared contribution, a guess ([`Finish::static_contribs`]).
    pub gdefs: Option<ValueId>,
}

impl FinDefs {
    /// The fields any run of the routine defined.
    fn names(&self) -> Vec<Field> {
        self.seen.clone()
    }

    /// The boxes of the insertion classes on a page (`classes`, a
    /// finisher's fourteenth argument): its fire defines each (§1018),
    /// and the value the routine leaves is predicted where the last two
    /// runs that defined it left the same (`\box\footins` emptied).
    fn class_boxes(&self, v: &Values, classes: Option<ValueId>) -> Vec<(Field, Option<ValueId>)> {
        let Some(cs) = classes else {
            return Vec::new();
        };
        list_of(v, cs)
            .iter()
            .filter_map(|&c| {
                let n = int_of(v, c);
                (0..255).contains(&n).then(|| {
                    let f = v.field(&format!("box:{n}"));
                    let x = self.recent.iter().find(|r| r.0 == f && r.3).map(|r| r.1);
                    (f, x)
                })
            })
            .collect()
    }

    /// The definitions both runs made with the same value: stable
    /// across pages, so a reader may run on them; a page number or a
    /// mark, changing every page, is not predicted (a wrong prediction
    /// runs its readers again).
    fn stable(&self, v: &Values) -> Vec<(Field, ValueId)> {
        self.last
            .iter()
            .filter(|d| {
                self.prev.contains(d)
                    && predictable(v, &crate::val::field_name(v, d.0), d.1, Some(d.1))
            })
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
                // (the next finisher reads the outgoing state of this one
                // only as a prediction, spec-final §3.5: a value not
                // stable across pages the last run's, a guess that runs
                // its readers again where wrong; `\deadcycles` only as a
                // stable 0, [`predictable`], and no mark that changed,
                // [`varies_by_page`])
                let name = crate::val::field_name(v, f);
                let before = self.prev.iter().find(|d| d.0 == f).map(|d| d.1);
                if !predictable(v, &name, x, before) {
                    return None;
                }
                // (one run of the routine so far, spec-final §9b: its
                // values carried on, a page counter stepping by the pages
                // between; a wrong guess runs its readers again, a piece
                // never parks on a finisher)
                let Some(&(_, y)) = self.prev.iter().find(|d| d.0 == f) else {
                    if !name.starts_with("count:") {
                        return Some((f, x));
                    }
                    let b = v.as_int(x)?;
                    return Some((f, v.int(b.checked_add(j - self.last_j)?)));
                };
                if x == y {
                    return Some((f, x));
                }
                // (a mark or the penalty the page fired at that differed
                // between the last two runs: the page's own, not guessed)
                if varies_by_page(&name) {
                    return None;
                }
                // (registers only: a page counter; not a hash, a penalty)
                if !name.starts_with("count:") {
                    return Some((f, x));
                }
                let (Some(a), Some(b)) = (v.as_int(y), v.as_int(x)) else {
                    return Some((f, x));
                };
                let Some(d) = b.checked_sub(a) else {
                    return Some((f, x));
                };
                if dj <= 0 || d % dj != 0 {
                    return Some((f, x));
                }
                let x = (d / dj).checked_mul(j - self.last_j)?.checked_add(b)?;
                Some((f, v.int(x)))
            })
            .collect()
    }
}

/// Whether a finisher's definition `x` of field `name`, which the run
/// before left as `y` (`None`: no run before), is predicted
/// ([`FinDefs::predict`]). `\deadcycles` only as 0 where both runs left
/// it 0 (both shipped their page, §1023): the fire steps it and a
/// `\shipout` clears it (§1012), so what a run leaves depends on the
/// page, and at `\end` it decides whether the page chain goes on
/// (§1054). Guessed as 1, as a page's run left it that shipped nothing,
/// a dead cycle predicted the next page fired by `\end`, whose finisher
/// predicted another: the chain grew without end on guesses while the
/// page it stood on was still to run. A guessed 0 ends a chain at most,
/// which the run, where wrong, starts again.
fn predictable(v: &Values, name: &str, x: ValueId, y: Option<ValueId>) -> bool {
    name != "deadcycles" || (y == Some(x) && v.as_int(x) == Some(0))
}

/// The fields a page's own items decide (the marks, §1012, and the
/// penalty the page fired at, §1013): predicted only where the last two
/// runs left them equal (a document without marks), never as the last
/// run's value where they differed ([`FinDefs::predict`]).
fn varies_by_page(name: &str) -> bool {
    name.starts_with("mark") || name == "param:outputpenalty"
}

/// Whether a finisher with arguments `args` may define field `f`, as
/// far as its page's insertion classes tell (the fourteenth argument,
/// [`Run::place_break`]): a box register below 255 is defined by the
/// fire only for a class on the page (§1018), and by the routine only
/// where its text may (its hint `t`); any other field, and any field
/// where the classes are not known, may be.
fn fin_may_box(v: &Values, args: &[ValueId], t: ValueId, f: Field) -> bool {
    let Some(&cs) = args.get(13) else {
        return true;
    };
    let name = crate::val::field_name(v, f);
    let Some(n) = name
        .strip_prefix("box:")
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|&n| (0..255).contains(&n))
    else {
        return true;
    };
    if list_of(v, cs).iter().any(|&c| int_of(v, c) == n) {
        return true;
    }
    match crate::hints::Hint::decode(&str_of(v, t)).may_define() {
        MayDefine::Nothing => false,
        MayDefine::Any => true,
        MayDefine::Prefixes(ps) => ps.iter().any(|p| name.starts_with(p.as_str())),
        MayDefine::Names(ns) => ns.contains(&f),
    }
}

/// What a finisher defines besides its routine's definitions
/// (`fire_page`, `output_end`, `ship_out`): field prefixes.
/// The fields every finisher defines itself, whatever its routine.
const FIN_OWN: [&str; 7] = [
    RTN,
    "deadcycles",
    "param:outputpenalty",
    "inspen",
    "badness",
    "box:255",
    crate::pdfout::PAGE_KEYS,
];

const FINISHER_FIELDS: &[&str] = &[
    "deadcycles",
    "mark:",
    "param:outputpenalty",
    "inspen",
    "box:",
    "badness",
    "write:",
    "pdf:",
];

/// A finisher's entry in `Ops::fin_defs`: its routine (the hint `t`).
fn fin_key(_: &Values, t: ValueId, _: &[ValueId]) -> u64 {
    t.raw()
}

/// Finishers created with a prediction of their definitions, and
/// without (reported with `PHITEX_TEX_TIME`).
pub(crate) static FIN_PREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static FIN_UNPREDICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

impl Finish {
    /// What the last runs of the routine defined, carried to page `j`
    /// ([`FinDefs::predict`]), and always what the routine gives back to
    /// the main list ([`RTN`]): the continuation after the finisher
    /// reads it, and with it guessed the scan goes on to the next page
    /// without waiting for the routine (spec-redesign §1, finish row).
    /// The guess is the last run's value, or nothing given back (the
    /// routine that ships its page, §1026); a wrong guess runs the
    /// continuation again.
    fn predicted_defs_(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if !ops(&self.0)
            .fin_predict
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return None;
        }
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        let j = args.get(10).map_or(0, |&x| int_of(v, x));
        let o = ops(&self.0);
        let m = o
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = (t != ValueId::UNIT)
            .then(|| m.get(&fin_key(v, t, args)))
            .flatten();
        let mut out = d
            .map(|d| if false { d.stable(v) } else { d.predict(v, j) })
            .unwrap_or_default();
        out.retain(|e| fin_may_box(v, args, t, e.0));
        if let Some(d) = d {
            for (f, x) in d.class_boxes(v, args.get(13).copied()) {
                if let Some(x) = x
                    && !out.iter().any(|e| e.0 == f)
                {
                    out.push((f, x));
                }
            }
        }
        let rtn = v.field(RTN);
        if !out.iter().any(|e| e.0 == rtn) {
            let last = d.and_then(|d| d.last.iter().find(|e| e.0 == rtn).map(|e| e.1));
            out.push((
                rtn,
                last.unwrap_or_else(|| v.list(vec![v.list(Vec::new()), v.list(Vec::new())])),
            ));
        }
        if let Some(&(_, x)) = out.iter().find(|e| e.0 == rtn) {
            let pj = args.first().map_or(0, |&y| int_of(v, y));
            out.push((rtn_field(v, pj), x));
        }
        Some(out)
    }

    /// The finisher's declared contribution to `gdefs` (what it defines
    /// globally, read by the resume after it as a guess, spec-redesign
    /// §1 resume row): the last run's of the routine, else none; a run
    /// giving another runs the resume again.
    fn gdefs_guess(&self, args: &[ValueId], v: &Values) -> ValueId {
        let t = args.get(9).copied().unwrap_or(ValueId::UNIT);
        if t == ValueId::UNIT {
            return ValueId::NIL;
        }
        ops(&self.0)
            .fin_defs_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .and_then(|d| d.gdefs)
            .map_or(ValueId::NIL, |x| gdefs_unit(v, x, fin_at(v, args)))
    }
}

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }
    /// What the last run of the same routine defined, and the fields a
    /// finisher sets itself (spec-final §9b): a definition list, each
    /// with its prediction ([`Finish::predicted_defs`]), so that a reader
    /// after the finisher (the next finisher, the continuation reading
    /// [`RTN`]) finds its site and runs on the prediction instead of
    /// waiting; a run defining outside it widens it at its commit and
    /// its readers run again (a field the routine defines on some pages
    /// only is not claimed for every page). None before any run of the
    /// routine.
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let t = *args.get(9)?;
        if t == ValueId::UNIT {
            return None;
        }
        let mut names: Vec<Field> = ops(&self.0)
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&fin_key(v, t, args))
            .map(|d| {
                let mut ns: Vec<Field> = d
                    .last
                    .iter()
                    .map(|e| e.0)
                    .filter(|&f| fin_may_box(v, args, t, f))
                    .collect();
                for (f, _) in d.class_boxes(v, args.get(13).copied()) {
                    if !ns.contains(&f) {
                        ns.push(f);
                    }
                }
                ns
            })?;
        // (and what it gives back to the main list, always predicted)
        let rtn = v.field(RTN);
        if !names.contains(&rtn) {
            names.push(rtn);
        }
        names.push(rtn_field(v, args.first().map_or(0, |&y| int_of(v, y))));
        // (the keys of the pages it ships, its PDF sides' read)
        names.push(v.field(crate::pdfout::PAGE_KEYS));
        ops(&self.0).fields.add_record_fields(&mut names);
        Some(names.into_iter().map(|f| (f, Locality::Global)).collect())
    }
    /// The output routine's hint (encoded, the tenth argument:
    /// [`crate::hints::routine_hint`]) and the fields a finisher defines
    /// itself; anything where the routine's text cannot be bounded, or
    /// the hint is not given.
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let Some(&t) = args.get(9) else {
            return MayDefine::Any;
        };
        if t == ValueId::UNIT {
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
        if let Some(mut names) = known {
            names.retain(|&f| fin_may_box(v, args, t, f));
            for n in FIN_OWN {
                names.push(v.field(n));
            }
            names.push(rtn_field(v, args.first().map_or(0, |&y| int_of(v, y))));
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
        ps.push(RTN.to_owned());
        ps.push("rtn:".to_owned());
        crate::fields::add_record_prefixes(&mut ps);
        MayDefine::Prefixes(ps)
    }
    /// No pieces' units ([`no_units`]), and its global definitions as
    /// the last run of the routine gave them ([`Finish::gdefs_guess`]).
    fn static_contribs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let mut out = no_units(v);
        out.push((v.field(GDEFS), self.gdefs_guess(args, v)));
        Some(out)
    }
    /// What the last finisher running the same routine defined, with
    /// its values: the text after this one runs on them, and runs again
    /// only where a value differs (the page number's readers). The page
    /// state and the shipped mark are left out: the chain reads them.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let r = self.predicted_defs_(args, v);
        let c = if r.is_some() {
            &FIN_PREDICTED
        } else {
            &FIN_UNPREDICTED
        };
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    /// What it folds ([`PageNode::folds`]) only: its break is its
    /// argument, and the material its page begins with ([`SCAN_MAT`]) it
    /// reads in its run, where the continuation before it is predicted
    /// ([`Scan::predicted_defs`]): the engine's gate takes no prediction.
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        PageNode::Finish.reads(v, &[])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let t_fin = std::time::Instant::now();
        let _ = phitex_pdf::phase::take();
        let pos = cx.args();
        let o = ops(&self.0);
        let out = run_node_parking(cx, o, true, |r| {
            if crate::exec::obs_flag!("PHITEX_FIN_TRACE") {
                let j = pos.first().map_or(0, |&x| int_of(r.v, x));
                eprintln!(
                    "fin-trace: node {} page {j} spec {} resumed {}",
                    r.cx.node().raw(),
                    r.cx.is_speculative(),
                    r.resumed
                );
            }
            if !r.resumed {
                let v = r.v;
                // (its page's break, defined by the node that found it,
                // the node before it; the page assembled from the items
                // by the break's range; its own arguments are its
                // position)
                let b = pos
                    .get(12)
                    .map(|&x| list_of(v, x))
                    .filter(|xs| xs.len() >= 6 && xs.first() == pos.first());
                let at = b
                    .as_ref()
                    .map(|b| (int_of(v, b[3]), int_of(v, b[4])))
                    .filter(|x| x.0 != -2);
                let args = match b {
                    Some(b) => r.finisher_args(&b, &pos)?,
                    None => pos.clone(),
                };
                let brk = match (args.get(5), args.get(6)) {
                    (Some(&p), Some(&k)) if p != ValueId::UNIT && args.len() >= 9 => {
                        Some((int_of(v, p), int_of(v, k), true))
                    }
                    _ => at.map(|(p, k)| (p, k, false)),
                };
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                r.fin = Some(FinCtx {
                    j: int_of(v, a(0)),
                    pending: Vec::new(),
                    held_n: 0,
                    rtn: None,
                    args: args.clone(),
                    brk,
                });
                r.preload_candidate(&args)?;
                if let Some(names) = crate::exec::obs_str!("PHITEX_FIN_SHOW") {
                    let mut shown = Vec::new();
                    for n in names.split(',') {
                        let x = r.get(n)?;
                        shown.push(format!(
                            "{n}={}",
                            x.map_or_else(
                                || "undefined".to_owned(),
                                |x| crate::val::show_def(v, x)
                            )
                        ));
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
            give_no_units(r);
            let args = r.fin.as_ref().map(|c| c.args.clone()).unwrap_or_default();
            r.split_piece(&args)?;
            let g = r.contribute_global_defs();
            r.record_finisher_defs(&args, g);
            Ok(())
        });
        if phitex_pdf::phase::on() {
            phitex_pdf::phase::add(0, t_fin);
            let a = phitex_pdf::phase::take();
            #[allow(clippy::cast_precision_loss)]
            let us: Vec<String> = a
                .iter()
                .map(|&n| format!("{:.1}", n as f64 / 1000.0))
                .collect();
            crate::exec::later(format!("finphase: {}", us.join(" ")));
        }
        out
    }
}

impl Run<'_, '_> {
    /// A finisher's state where its page fired, before the routine
    /// runs: the bindings local to the groups open at the candidate
    /// (its fourth argument), and what the piece defined after the
    /// candidate as it was there, where the piece had it (its ninth): the
    /// routine runs at the candidate, before those definitions. One the
    /// piece had not read stays as it reaches here; the routine reading
    /// it cuts the piece ([`Run::check_cut`]).
    fn preload_candidate(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let locals = list_of(v, a(3));
        // (the routines of the pages firing in the piece before this one,
        // placed after it before this finisher: a binding they defined
        // globally is gone at every level where it was made before
        // their candidate, §279)
        let piece = self.fin.as_ref().and_then(|c| c.brk).map(|b| b.0);
        let gdefs = match piece {
            Some(p) if !locals.is_empty() => self.gdefs_after(p)?,
            _ => Vec::new(),
        };
        for e in locals {
            let ys = list_of(v, e);
            if let [f, x, ..] = ys[..] {
                let after = ys.get(2).map_or(-1, |&c| int_of(v, c));
                if replaced(&gdefs, Field(f), piece.unwrap_or(-2), after) {
                    continue;
                }
                let loc = self.loc_of(Field(f));
                self.st.preload(loc, some(x));
            }
        }
        if args.len() < 9 {
            return Ok(());
        }
        let piece =
            phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(int_of(v, a(5)).to_ne_bytes()));
        let before = v.str(BEFORE_PIECE);
        for e in list_of(v, a(8)) {
            let ys = list_of(v, e);
            let [f, _, kv] = ys[..] else { continue };
            if kv == ValueId::UNIT {
                continue;
            }
            let x = if kv == before {
                match self.cx.use_field_before(Field(f), piece)? {
                    phitex_defuse::FieldAt::Def(x, _) => Some(x),
                    phitex_defuse::FieldAt::Unset => None,
                    phitex_defuse::FieldAt::Gone => continue,
                }
            } else {
                list_of(v, kv).first().copied()
            };
            let loc = self.loc_of(Field(f));
            self.st.preload(loc, x);
        }
        Ok(())
    }

    /// The fields this finisher defined globally, for the resume after
    /// it ([`Run::cont_load`]): a group-local value the resume carries
    /// is replaced by a global definition even of an equal value.
    fn contribute_global_defs(&mut self) -> ValueId {
        let v = self.v;
        let names: Vec<ValueId> = self
            .st
            .published(v, self.rec_load)
            .into_iter()
            .filter(|d| d.2 == Locality::Global)
            .map(|d| self.loc_field(d.0).0)
            .collect();
        // (none given as nothing, as the declared guess says it,
        // [`Finish::static_contribs`])
        let x = if names.is_empty() {
            ValueId::NIL
        } else {
            v.list(names)
        };
        let at = fin_at(v, &self.cx.args());
        self.contribute(GDEFS, gdefs_unit(v, x, at));
        x
    }

    /// The definitions this finisher publishes, kept for the next one
    /// running the same routine ([`Finish::predicted_defs`]).
    fn record_finisher_defs(&mut self, args: &[ValueId], gdefs: ValueId) {
        let v = self.v;
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
        // (`PHITEX_FIN_FIELD=name`: what each finisher's run left of that
        // field, for the predictions)
        if let Some(name) = crate::exec::obs_str!("PHITEX_FIN_FIELD") {
            let f = v.field(name);
            eprintln!(
                "fin-field page {j} node {}: {name} {}",
                self.cx.node().raw(),
                out.iter().find(|d| d.0 == f).map_or_else(
                    || "not defined".to_owned(),
                    |d| v.render(d.1).chars().take(80).collect()
                )
            );
        }
        let mut m = self
            .ops
            .fin_defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = m.entry(fin_key(v, t, args)).or_default();
        e.gdefs = Some(gdefs);
        for &(f, x) in &out {
            if !e.seen.contains(&f) {
                e.seen.push(f);
            }
            match e.recent.iter_mut().find(|r| r.0 == f) {
                Some(r) if j >= r.2 => *r = (f, x, j, r.1 == x),
                Some(_) => {}
                None => e.recent.push((f, x, j, true)),
            }
        }
        if crate::exec::obs_flag!("PHITEX_PRED_SHOW") {
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
        // (the two latest pages: finishers commit in any order; a page's
        // run again replaces its own entry, never makes the other: two
        // runs of one page agree on what changes from page to page, a
        // column kept, a mark, and would predict it for the next page)
        if j == e.last_j && !e.last.is_empty() {
            e.last = out;
        } else if j == e.prev_j && !e.prev.is_empty() {
            e.prev = out;
        } else if j > e.last_j || e.last.is_empty() {
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
    /// What it cannot run without (spec-final §9b): the continuation the
    /// cut piece defined; the engine's gate holds it until that is there
    /// instead of a run parking on it. And what it folds
    /// ([`PageNode::folds`]).
    fn reads(&self, _: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        PageNode::Resume.reads(v, &[RESUME])
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
    /// The siblings the rest of the cut piece takes, as the piece guessed
    /// them (the fourth argument, [`Run::claim_guess`]): a guess, which
    /// its run's claim replaces.
    fn claim(
        &self,
        args: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        match args.get(3).and_then(|&x| v.as_int(x)) {
            Some(0) => Claim::None,
            Some(n) if n > 0 => Claim::Count(usize::try_from(n).unwrap_or(0)),
            _ => Claim::Pending,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        let t0 = std::time::Instant::now();
        let out = run_node_parking(cx, o, true, |r| {
            if !r.resumed {
                let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
                let (cont, pid) = if a(0) == ValueId::UNIT {
                    let x = r.eng_read(r.v.field(RESUME))?;
                    let xs = x.map(|x| list_of(r.v, x)).unwrap_or_default();
                    (
                        xs.first().copied().unwrap_or(ValueId::UNIT),
                        xs.get(1).copied().unwrap_or(ValueId::UNIT),
                    )
                } else {
                    (a(0), a(2))
                };
                r.cont_load(cont, pid)?;
            }
            r.run_all()
        });
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") && t0.elapsed().as_millis() > 200 {
            let v = cx.values();
            let text = v.render(args.first().copied().unwrap_or(ValueId::UNIT));
            let cut: String = text.chars().take(600).collect();
            crate::exec::later(format!(
                "longrun: resume {} {:.0} ms: {cut}",
                cx.node().raw(),
                t0.elapsed().as_secs_f64() * 1e3
            ));
        }
        out
    }
}

/// What a scan reads where a fold runs: the fold's run, and the piece
/// whose end the registers are read at (none: where the run is; a
/// block's scan reads past nothing it placed itself).
struct RunReaching<'r, 'c, 'a>(&'r mut Run<'c, 'a>, Option<i64>);

impl crate::vprof::Reaching for RunReaching<'_, '_, '_> {
    fn params(&mut self) -> PageParams {
        self.0.reach_reads += 1;
        let v = self.0.v;
        let m = match self.1 {
            Some(at) => self.0.page_params_at(at),
            None => self.0.page_params_marker(),
        };
        m.map_or(
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
        self.0.reach_reads += 1;
        let c = match self.1 {
            Some(at) => self.0.insert_class_at(
                n,
                phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(at.to_ne_bytes())),
            ),
            None => self.0.insert_class(n),
        };
        c.unwrap_or(InsertClass {
            box_height_plus_depth: None,
            box_is_hbox: false,
            count: 0,
            dimen: 0,
            skip: Glue::ZERO,
        })
    }
}

// ------------------------------------------------------------- scan

/// A scan step (spec-predicted-breaks §2, S2): TeX's page builder
/// (§980–§1010) over the pieces' vertical profiles, from the last page
/// fired on. Arguments `[kind]`, its position only (§7): -3, after a
/// piece (the step of that piece), or -2, after a finisher (the scan's
/// continuation).
///
/// - A piece's step is held until the pieces before it have run (its
///   read hint names the profiles, [`crate::vprof::VPROF`]: the engine
///   holds it while a contributor before it is in work; the page nodes
///   declare they contribute none, [`no_units`]), reads the scan state
///   ([`SCAN`]) and the profiles of the pieces after the state's anchor
///   up to its own (the units of that window), and runs the builder
///   over the material the page began with and the units. A step that
///   fires no page defines nothing: the scan's state has one version per
///   page, not per piece. Where a page fires in its own piece, the step
///   defines its break and places its finisher and the continuation
///   ([`Run::place_break`]); it reads no items.
/// - A continuation assembles its page from the break
///   ([`Run::assemble_page`]: what the page leaves, the held-over
///   insertions, the material carried, the rest of the piece), reads
///   what the routine gave back ([`RTN`], on the finisher's prediction),
///   and defines the scan state the next page begins with, or places the
///   next page's break when it fires within that material.
pub(crate) struct Scan(pub Shared);

impl Operation for Scan {
    fn name(&self) -> &'static str {
        "scan"
    }
    /// A continuation placed with its predicted scan state
    /// ([`Run::place_cont`], in its arguments 2 and 3) defines that state and its
    /// page's field: a reader finds them and runs on the prediction
    /// ([`Scan::predicted_defs`]).
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let (j, by, _) = cont_chain(v, args)?;
        (args.get(2).is_some_and(|&x| x != ValueId::UNIT)).then(|| {
            vec![
                (v.field(SCAN), Locality::Global),
                (v.field(SCAN_MAT), Locality::Global),
                (scan_mat_field(v, j + 1), Locality::Global),
                (scan_page_field(v, j, by), Locality::Global),
            ]
        })
    }
    /// The page fields and the scan state: a step not run yet may fire
    /// a page and place a continuation that defines it, so a step's read
    /// waits for the steps before it still to run; a step's run narrows
    /// this to what it defined ([`Run::scan_node`]).
    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        // (a continuation never defines where a block ends: a block's
        // scan, gated on that, waits for the scan of the block before
        // only)
        if args.first().is_some_and(|&x| int_of(v, x) == -2) {
            let mut names = vec![v.field(SCAN), v.field(SCAN_MAT)];
            if let Some(j) = cont_page(v, args) {
                names.push(scan_mat_field(v, j + 1));
            }
            if let Some((j0, by, _)) = cont_chain(v, args) {
                names.push(scan_page_field(v, j0, by));
            }
            return MayDefine::Names(names);
        }
        MayDefine::Names(vec![
            v.field(SCAN),
            v.field(SCAN_MAT),
            v.field(SCAN_BLOCK),
            v.field(SCAN_AT),
        ])
    }
    /// A step is held until the scan state is there and its piece has
    /// run (what the piece leaves the main list, which it defines at its
    /// end), and both kinds while a contributor to what they fold
    /// ([`PageNode::folds`]) before them is in work.
    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        if args.first().is_some_and(|&x| int_of(v, x) == -2) {
            // (it assembles its page as the finisher does, reading the
            // material the page began with in its run, on the prediction
            // of the continuation before it; the break is its argument)
            return PageNode::Cont.reads(v, &[]);
        }
        // (not the scan state: the engine's gate does not take a
        // definer's prediction, a run's read does, and the continuation
        // before it is predicted, [`Run::place_cont`]; the state where
        // its block begins, which the block's scan before defines and
        // nothing predicts: waited for at the gate, not parked on)
        PageNode::Step.reads(v, &[crate::build::MAIN_LAST, SCAN_BLOCK])
    }
    /// No pieces' units: a window holding a step does not wait for it.
    fn static_contribs(&self, _: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(no_units(v))
    }
    /// A continuation's scan state as the block's scan that placed it
    /// predicted it (its arguments 2 and 3), with its page's field; else as the
    /// guess cache has it for its page (spec-final §3.4): the steps after
    /// it run on that instead of waiting; a different real one runs them
    /// again.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        if args.first().is_none_or(|&x| int_of(v, x) != -2) {
            return None;
        }
        if let (Some((j, by, b)), Some(&x), Some(&mat)) =
            (cont_chain(v, args), args.get(2), args.get(3))
            && x != ValueId::UNIT
        {
            // (where the state after the page stands: the next page, the
            // ordinary pages up to this one, its break's)
            let bs = list_of(v, b);
            let at = v.list(vec![
                v.int(j + 1),
                bs.get(8).copied().unwrap_or_else(|| v.int(0)),
            ]);
            return Some(vec![
                (v.field(SCAN), x),
                (v.field(SCAN_MAT), mat),
                (scan_mat_field(v, j + 1), mat),
                (scan_page_field(v, j, by), v.list(vec![x, mat, b, at])),
            ]);
        }
        let j = int_of(v, *args.get(1)?);
        let (st, mat) = *ops(&self.0)
            .scan_guess_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&j)?;
        Some(vec![
            (v.field(SCAN), st),
            (v.field(SCAN_MAT), mat),
            (scan_mat_field(v, j + 1), mat),
        ])
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let args = cx.args();
        let o = ops(&self.0);
        run_node_parking(cx, o, false, |r| r.scan_node(&args))
    }
}

/// The scan state a step starts from ([`SCAN`]).
struct ScanIn {
    anchor: Option<phitex_defuse::NodeId>,
    j: i64,
    normal: i64,
    fold: FoldState,
    /// The page parameters in force (a `T_PAGE_PARAMS` marker).
    params: Option<ValueId>,
    /// What the page begins with, as a profile and as items.
    prof: Vec<i64>,
    material: Vec<ValueId>,
}

impl ScanIn {
    fn decode(v: &Values, x: Option<ValueId>) -> ScanIn {
        let xs = x.map(|x| list_of(v, x)).unwrap_or_default();
        if xs.len() < 9 {
            return ScanIn {
                anchor: None,
                j: 0,
                normal: 0,
                fold: crate::build::IGNORE_DEPTH_STATE,
                params: None,
                prof: Vec::new(),
                material: Vec::new(),
            };
        }
        let n = |i: usize| int_of(v, xs[i]);
        ScanIn {
            anchor: Some(phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(
                n(0).to_ne_bytes(),
            ))),
            j: n(1),
            normal: n(2),
            fold: FoldState {
                sf: i32::try_from(n(3)).unwrap_or(1000),
                pd: i32::try_from(n(4)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(5)).unwrap_or(0),
            },
            params: some(xs[6]),
            prof: list_of(v, xs[7])
                .into_iter()
                .map(|y| int_of(v, y))
                .collect(),
            material: list_of(v, xs[8]),
        }
    }

    /// The scan's builder where the page began.
    fn state(&self, v: &Values) -> crate::vprof::ScanState {
        crate::vprof::ScanState::fresh(
            self.j,
            self.normal,
            self.fold,
            self.params.map(|m| params_of(v, m)),
        )
    }

    /// The real builder where the page began.
    fn page_state(&self) -> PageState {
        let mut ps = PageState::fresh(self.j);
        ps.normal = self.normal;
        ps.fold = self.fold;
        ps.params = self.params;
        ps
    }
}

/// [`Run::assemble_narrow`] declining (`PHITEX_SCAN_TRACE`: why).
fn narrow_miss(why: u32, pid: i64) -> Option<(PageState, Fire, Vec<ValueId>)> {
    if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
        eprintln!("scan: narrow assembly declined ({why}) at piece {pid}");
    }
    None
}

/// A [`PITEMS`] unit's items.
fn unit_items(v: &Values, u: ValueId) -> Vec<ValueId> {
    list_of(v, u)
        .get(2)
        .map(|&x| list_of(v, x))
        .unwrap_or_default()
}

/// Whether items hold an `\end` marker (the builder then reads
/// `\deadcycles`, §1054).
fn has_end(v: &Values, raw: &[ValueId]) -> bool {
    raw.iter().any(|&x| nodes::tag(v, x) == nodes::T_END)
}

/// The pieces of a scan's block, about (spec-final §9b): a piece ends a
/// block where its node's id says so ([`block_end`]), content-keyed (an
/// edit moves no other block's end).
const BLOCK: u64 = 64;

/// Whether the piece `node` ends a block of pieces ([`BLOCK`]): it then
/// places the block's scan ([`Run::scan_step`]).
fn block_end(node: u64) -> bool {
    let mut z = node.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)).is_multiple_of(BLOCK)
}

/// What a continuation does ([`Run::cont_state`]).
enum Cont {
    Nothing,
    /// The next page's break in its material: `[j, src, k, rest,
    /// normal, special, origin]` as [`Run::place_break`] takes them.
    Fire(i64, ValueId, u32, usize, i64, bool, Option<ValueId>),
    /// The scan state and the material it defines, and where it stands
    /// among the pages (`[j, normal]`, [`SCAN_AT`]).
    State(ValueId, ValueId, ValueId),
}

/// What a finisher placed at `pos` is predicted to give back to the
/// main list ([`RTN`], [`Finish::predicted_defs`]).
fn rtn_guess(o: &crate::run::Ops, v: &Values, pos: &[ValueId]) -> ValueId {
    let t = pos.get(9).copied().unwrap_or(ValueId::UNIT);
    let last = (t != ValueId::UNIT)
        .then(|| {
            o.fin_defs_start
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&fin_key(v, t, pos))
                .and_then(|d| d.last.iter().find(|e| e.0 == v.field(RTN)).map(|e| e.1))
        })
        .flatten();
    last.unwrap_or_else(|| v.list(vec![v.list(Vec::new()), v.list(Vec::new())]))
}

/// What a scan node defined.
enum Placed {
    Nothing,
    /// The scan state ([`SCAN`]).
    State,
    /// A page's break ([`Run::place_break`]).
    Page,
}

/// The scan's states after the prefixes of windows ([`Run::scan_step`]),
/// by the window's start and its units' values.
fn scan_memo() -> &'static std::sync::Mutex<phitex_defuse::FxHashMap<u64, crate::vprof::ScanState>>
{
    static M: std::sync::OnceLock<
        std::sync::Mutex<phitex_defuse::FxHashMap<u64, crate::vprof::ScanState>>,
    > = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Pages where the scan's decisions and the builder's on the items
/// differed (a scan defect; the page is built from the items).
static SCAN_REAL_DIFFS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Run<'_, '_> {
    /// A [`Scan`] node's run.
    #[allow(clippy::too_many_lines)]
    fn scan_node(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        give_no_units(self);
        let placed = if args.first().is_some_and(|&x| int_of(v, x) == -2) {
            self.scan_cont()?
        } else {
            self.scan_step()?
        };
        // (what it defined: a reader after it no longer waits for it)
        // (a page placed: the scan state too, until it commits the
        // continuation it places, which defines it, is not there for a
        // reader to wait for)
        let block = args.first().is_some_and(|&x| int_of(v, x) == -3);
        let mut names: Vec<Field> = match placed {
            Placed::Nothing => Vec::new(),
            Placed::State | Placed::Page => vec![v.field(SCAN), v.field(SCAN_MAT)],
        };
        if let (Placed::State, Some(j)) = (&placed, cont_page(v, args)) {
            names.push(scan_mat_field(v, j + 1));
        }
        if let (Placed::State, Some((j0, by, _))) = (&placed, cont_chain(v, args)) {
            names.push(scan_page_field(v, j0, by));
        }
        if block {
            names.push(v.field(SCAN_BLOCK));
            names.push(v.field(SCAN_AT));
        }
        self.cx.narrow(MayDefine::Names(names));
        Ok(())
    }

    /// The profiles of the window from scan state `src`'s anchor to
    /// this node folded with the page builder: the first page firing in
    /// them, `(piece, the fire, the builder after it, whether the piece
    /// is the window's last)`.
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    fn scan_window(
        &mut self,
        src: Option<ValueId>,
    ) -> R<Option<(i64, crate::vprof::ScanFire, crate::vprof::ScanState, bool)>> {
        let v = self.v;
        let s = ScanIn::decode(v, src);
        let units = v.seq_to_vec(self.fold_own(crate::vprof::VPROF, s.anchor, None)?);
        let trace = crate::exec::obs_flag!("PHITEX_SCAN_TRACE");
        if trace && units.is_empty() {
            eprintln!(
                "scan {}: from page {} (anchor {:?}), no units",
                self.cx.node().raw(),
                s.j,
                s.anchor.map(phitex_defuse::NodeId::raw)
            );
        }
        if units.is_empty() {
            return Ok(None);
        }
        if trace {
            eprintln!(
                "scan {}: from page {} (anchor {:?}), {} units, speculative {}",
                self.cx.node().raw(),
                s.j,
                s.anchor.map(phitex_defuse::NodeId::raw),
                units.len(),
                self.cx.is_speculative()
            );
        }
        // (spec-final §3.3: a page's steps read one window from its
        // anchor; each goes on from the longest prefix of it a step
        // before folded, a memo of the scan over the profiles, so a page
        // costs its profiles once and not once a step. What a prefix
        // read from the engine as it was folded (a unit's page
        // parameters, a class's registers) keeps it out of the memo: a
        // step reads that itself)
        let ids: Vec<u64> = units.iter().map(|u| u.raw()).collect();
        let material = crate::vprof::decode(&s.prof);
        let mut decoded: Vec<Option<(i64, Vec<crate::vprof::PItem>)>> = vec![None; units.len()];
        let mut needs = vec![false; units.len()];
        for (i, &u) in units.iter().enumerate() {
            let (pid, ints) = crate::vprof::unit_parts(v, u);
            let items = crate::vprof::decode(&ints);
            needs[i] = items.iter().any(|x| {
                matches!(
                    x,
                    crate::vprof::PItem::Params(None) | crate::vprof::PItem::InsParams(_, None)
                ) || crate::vprof::is_end(x)
            });
            decoded[i] = Some((pid, items));
        }
        let dead = if crate::vprof::has_end(&material)
            || decoded
                .iter()
                .flatten()
                .any(|d| crate::vprof::has_end(&d.1))
        {
            // (where the window's last piece ends: what reaches this scan
            // also holds what the finishers it placed before it define)
            let last_pid = decoded.iter().flatten().last().map(|d| d.0);
            match last_pid {
                Some(p) => {
                    let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
                    let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(p.to_ne_bytes()));
                    match self.cx.use_field_at(f, Some(at))? {
                        phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                        phitex_defuse::FieldAt::Unset => 0,
                        phitex_defuse::FieldAt::Gone => {
                            self.cint(crate::fields::cell::DEADCYCLES)?
                        }
                    }
                }
                None => self.cint(crate::fields::cell::DEADCYCLES)?,
            }
        } else {
            0
        };
        let h0 = phitex_defuse::hash_words(
            0x7363_616e_6d65_6d6f,
            &[
                v.instance(),
                src.map_or(0, ValueId::raw),
                dead.cast_unsigned(),
            ],
        );
        let mut keys = Vec::with_capacity(ids.len() + 1);
        keys.push(h0);
        for &u in &ids {
            let k = phitex_defuse::hash_words(*keys.last().unwrap_or(&h0), &[u]);
            keys.push(k);
        }
        // the longest prefix folded before, short of this step's unit
        let last = units.len() - 1;
        let (mut from, memo) = {
            let m = scan_memo()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (1..=last)
                .rev()
                .find_map(|k| m.get(&keys[k]).map(|st| (k, st.clone())))
                .map_or((0, None), |(k, st)| (k, Some(st)))
        };
        let mut clean = true;
        let (scan_fire, st) = {
            // (the registers a fold reads, as they reach the piece being
            // folded: the material where the page before fired)
            let mut rd = RunReaching(
                self,
                s.anchor.map(|a| i64::from_ne_bytes(a.raw().to_ne_bytes())),
            );
            let mut st = if let Some(st) = memo {
                st
            } else {
                from = 0;
                let mut st = s.state(rd.0.v);
                let reads0 = rd.0.reach_reads;
                if st.fold_items(&material, dead, &mut rd).is_some() {
                    // (the continuation fires on this material itself: a
                    // state this step read before that continuation ran)
                    return Ok(None);
                }
                clean = rd.0.reach_reads == reads0;
                st
            };
            let mut out = None;
            for i in from..units.len() {
                let Some((pid, mut items)) = decoded[i].take() else {
                    continue;
                };
                rd.0.resolve_prof_of(pid, &mut items)?;
                rd.1 = Some(pid);
                let reads0 = rd.0.reach_reads;
                let f = st.fold_items(&items, dead, &mut rd);
                clean = clean && !needs[i] && rd.0.reach_reads == reads0;
                if let Some(f) = f {
                    out = Some((pid, f, i == last));
                    break;
                }
                if clean && i < last {
                    let mut m = scan_memo()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if m.len() > 1 << 16 {
                        m.clear();
                    }
                    m.insert(keys[i + 1], st.clone());
                }
            }
            (out, st)
        };
        Ok(scan_fire.map(|(pid, sf, in_last)| (pid, sf, st, in_last)))
    }

    /// A block's scan (spec-final §9b): placed after the last piece of a
    /// block of pieces ([`block_end`]); from the scan state where the
    /// block begins (as it reaches the scan before, whose pages'
    /// continuations all sit before it) the pieces' profiles folded with
    /// the page builder, and for each page firing in them its finisher
    /// and continuation placed right after the piece it fires in
    /// ([`Ctx::follow_after`]); the next page goes on from the state that
    /// continuation defines as it reaches here (the continuation's run,
    /// or its prediction), else from its prediction made here (the
    /// finisher's routine giving back what it is predicted to), which a
    /// run of this node reads again once the continuation is there. The
    /// chain is the blocks', not the pages'.
    #[allow(clippy::too_many_lines)]
    fn scan_step(&mut self) -> R<Placed> {
        let v = self.v;
        // (where the block begins: the state the scan of the block
        // before ended its block in, [`SCAN_BLOCK`]; none before the
        // first block. Nothing its own continuations define is read: they
        // sit before it)
        // (where the block begins among the pages, [`SCAN_AT`]: read where
        // a page fires in it, from the continuation's field where the
        // block begins at one)
        let mut base: Option<(i64, i64)> = None;
        let mut define_at = false;
        let (mut src, mut mat) = match self.eng_read(v.field(SCAN_BLOCK))? {
            // (the block before ended in pages firing in continuations'
            // material whose end its scan does not know yet: nothing
            // here until it does, not a scan from the document's start)
            Some(x) if x == ValueId::UNIT => {
                if crate::exec::obs_flag!("PHITEX_BLOCK_TRACE") {
                    eprintln!(
                        "scan {}: the block before ends unknown: nothing (speculative {})",
                        self.cx.node().raw(),
                        self.cx.is_speculative()
                    );
                }
                self.cx
                    .define_with(v.field(SCAN_BLOCK), ValueId::UNIT, Locality::Global);
                return Ok(Placed::Nothing);
            }
            // (the block before ended in a page whose continuation sits
            // after its scan: the state that continuation (or the last of
            // the chain its material fires) leaves the scan in, on its
            // prediction until it runs: its page's own field, read by
            // name, not the scan state reaching this step, which the
            // continuations this step places before itself define: a
            // read of its own placements, another state each run)
            Some(x)
                if list_of(v, x)
                    .first()
                    .is_some_and(|&y| int_of(v, y) == BLOCK_AT_CONT) =>
            {
                let xs = list_of(v, x);
                let p = match xs[..] {
                    [_, j, by] => self.eng_read(scan_page_field(v, int_of(v, j), int_of(v, by)))?,
                    _ => None,
                };
                if crate::exec::obs_flag!("PHITEX_BLOCK_TRACE") {
                    eprintln!(
                        "scan {}: the block before ends at a continuation {:?}: {} (speculative {})",
                        self.cx.node().raw(),
                        xs.iter()
                            .skip(1)
                            .map(|&y| int_of(v, y).cast_unsigned())
                            .collect::<Vec<_>>(),
                        if p.is_some() {
                            "defined"
                        } else {
                            "not defined: nothing"
                        },
                        self.cx.is_speculative()
                    );
                }
                let Some(p) = p else {
                    self.cx
                        .define_with(v.field(SCAN_BLOCK), ValueId::UNIT, Locality::Global);
                    return Ok(Placed::Nothing);
                };
                let ps = list_of(v, p);
                let at = ps.get(3).map(|&a| list_of(v, a)).unwrap_or_default();
                base = Some((
                    at.first().map_or(0, |&x| int_of(v, x)),
                    at.get(1).map_or(0, |&x| int_of(v, x)),
                ));
                define_at = true;
                (
                    ps.first().copied().filter(|&y| y != ValueId::UNIT),
                    ps.get(1).copied().filter(|&y| y != ValueId::UNIT),
                )
            }
            Some(x) => {
                let xs = list_of(v, x);
                (
                    xs.first().copied().filter(|&y| y != ValueId::UNIT),
                    xs.get(1).copied().filter(|&y| y != ValueId::UNIT),
                )
            }
            None => (None, None),
        };
        let mut placed = Placed::Nothing;
        let trace = crate::exec::obs_flag!("PHITEX_SCAN_TRACE");
        let mut known = true;
        let mut after_me = false;
        let mut last_page = 0;
        // (a bound on the pages one block fires: a page fires on some
        // item, and an item's page goes on in a later piece at most)
        for _ in 0..4096 {
            let Some((pid, sf, st, in_last)) = self.scan_window(src)? else {
                break;
            };
            // (a page firing in the window's last piece, the one whose
            // run placed this step: its continuation sits after this
            // step (in that piece's extent), where this step's reads do
            // not reach; the next block's scan reads the scan state that
            // continuation defines instead of this step's prediction of
            // it, [`SCAN_BLOCK`])
            after_me = in_last;
            // (the window's state leaves out where it stands: the
            // page's index and the ordinary pages from where the block
            // began, or the page before in it)
            let (j0, n0) = if let Some(b) = base {
                b
            } else {
                let at = self.eng_read(v.field(SCAN_AT))?.map(|x| list_of(v, x));
                let at = at.unwrap_or_default();
                (
                    at.first().map_or(0, |&x| int_of(v, x)),
                    at.get(1).map_or(0, |&x| int_of(v, x)),
                )
            };
            let j = j0 + st.j - 1;
            let normal = n0 + st.normal;
            base = Some((j + 1, normal));
            define_at = true;
            last_page = j;
            if trace {
                eprintln!(
                    "scan {}: page {} fires in piece {pid} at candidate {}",
                    self.cx.node().raw(),
                    j + 1,
                    sf.k
                );
            }
            let at = phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes()));
            // (a page firing in the window's last piece, the one that
            // placed this step right after itself: its finisher and
            // continuation right after this step, before the rest of the
            // piece the piece placed after it where it cut, `fold_here`)
            let (b, pos) = self.place_break(
                Some(at),
                in_last,
                j,
                src.unwrap_or(ValueId::UNIT),
                pid,
                sf.k,
                sf.rest,
                normal,
                sf.output_penalty <= -10001,
                None,
                Some(&sf.classes),
            )?;
            // (the continuation's scan state: as it defines it for this
            // break, an ordinary read of a node this scan placed, which
            // runs this scan again where it differs; else as the scan
            // sees it, what the routine gives back its finisher's
            // prediction, and the continuation predicts it; none where a
            // page fires in the continuation's own material)
            let me = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
            let defined = self.eng_read(scan_page_field(v, j, me))?.and_then(|x| {
                let xs = list_of(v, x);
                (xs.get(2) == Some(&b)).then_some(())?;
                Some((*xs.first()?, *xs.get(1)?))
            });
            let pred = match defined {
                Some(p) => Some(p),
                None if false => None,
                None => {
                    let rtn = rtn_guess(self.ops, v, &pos);
                    // (the material its page begins with as this step
                    // has it, or its page's field where the step began
                    // from a block before: none before the first page)
                    let m = match (mat, src) {
                        (Some(m), _) => m,
                        (None, Some(_)) => self
                            .eng_read(scan_mat_field(v, j))?
                            .unwrap_or_else(|| v.list(Vec::new())),
                        (None, None) => v.list(Vec::new()),
                    };
                    match self.cont_state(&list_of(v, b), Some(rtn), Some(m))? {
                        Cont::State(x, m, _) => Some((x, m)),
                        _ => None,
                    }
                }
            };
            // (the prediction goes in the continuation's arguments only
            // where its state is not defined yet: once it is, the
            // continuation is placed without one, the same node from then
            // on, so a step running again does not place a new one each
            // time its guess moves)
            self.place_cont(
                Some(at),
                in_last,
                j,
                if defined.is_some() { None } else { pred },
                b,
                None,
            );
            placed = Placed::Page;
            let Some((x, m)) = pred else {
                known = false;
                break;
            };
            src = Some(x);
            mat = Some(m);
        }
        // (where this block ends, for the next block's scan)
        if crate::exec::obs_flag!("PHITEX_BLOCK_TRACE") {
            eprintln!(
                "scan {}: block ends {} (speculative {})",
                self.cx.node().raw(),
                if known { "known" } else { "UNKNOWN" },
                self.cx.is_speculative()
            );
        }
        // (a page firing in the window's last piece: its continuation,
        // or the last of the chain its material fires, sits after this
        // step and defines the state the block ends in, whether or not
        // this step could predict it: the next block reads that. Unknown
        // where the continuation sits before this step: this step reads
        // its state once it is defined, and runs again)
        let end = if after_me {
            let me = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
            v.list(vec![v.int(BLOCK_AT_CONT), v.int(last_page), v.int(me)])
        } else if !known {
            ValueId::UNIT
        } else {
            // (the state only: the material the next page begins with is
            // its page's field, [`scan_mat_field`], read by the block that
            // fires that page; carried here, an edit in it would run
            // every block's scan to the next page again)
            v.list(vec![src.unwrap_or(ValueId::UNIT), ValueId::UNIT])
        };
        self.cx
            .define_with(v.field(SCAN_BLOCK), end, Locality::Global);
        // (where the next block begins among the pages: where this one
        // fired pages, or began at a continuation's state; else as it
        // reached this one)
        if define_at
            && known
            && !after_me
            && let Some((j, n)) = base
        {
            self.cx.define_with(
                v.field(SCAN_AT),
                v.list(vec![v.int(j), v.int(n)]),
                Locality::Global,
            );
        }
        Ok(placed)
    }

    /// A scan continuation's run: its page assembled from the break
    /// ([`Run::assemble_page`]); the material the next page begins with
    /// (§1026: the routine's markers, the held-over insertions, its list,
    /// the material carried over, the page built again, the rest of the
    /// piece), its page parameter and insertion class markers resolved
    /// here; the page builder over it from the state after the page
    /// fired; the scan state defined, or the next page's break placed.
    #[allow(clippy::too_many_lines)]
    fn scan_cont(&mut self) -> R<Placed> {
        let v = self.v;
        let b = self
            .cx
            .args()
            .get(4)
            .map(|&x| list_of(v, x))
            .unwrap_or_default();
        self.scan_cont_(&b)
    }

    #[allow(clippy::too_many_lines)]
    fn scan_cont_(&mut self, b: &[ValueId]) -> R<Placed> {
        let v = self.v;
        let b = b.to_vec();
        match self.cont_state(&b, None, None)? {
            Cont::Nothing => Ok(Placed::Nothing),
            Cont::Fire(j, src, k, rest, normal, special, origin) => {
                let (b, _) = self.place_break(
                    None, false, j, src, -2, k, rest, normal, special, origin, None,
                )?;
                let chain = cont_chain(v, &self.cx.args());
                self.place_cont(None, false, j, None, b, chain);
                Ok(Placed::Page)
            }
            Cont::State(x, mat, at) => {
                self.cx.define_with(v.field(SCAN), x, Locality::Global);
                self.cx
                    .define_with(v.field(SCAN_MAT), mat, Locality::Global);
                if let Some(j) = cont_page(v, &self.cx.args()) {
                    self.cx
                        .define_with(scan_mat_field(v, j + 1), mat, Locality::Global);
                }
                // (where the page the block's scan placed leaves the scan,
                // for that page's break: the scan reads it and goes on
                // from it)
                if let Some((j0, by, b0)) = cont_chain(v, &self.cx.args()) {
                    if crate::exec::obs_flag!("PHITEX_BLOCK_TRACE") {
                        eprintln!(
                            "cont {}: defines the state of page {j0} placed by {}",
                            self.cx.node().raw(),
                            by.cast_unsigned()
                        );
                    }
                    self.cx.define_with(
                        scan_page_field(v, j0, by),
                        v.list(vec![x, mat, b0, at]),
                        Locality::Global,
                    );
                }
                // (the guess cache: what this page's continuation defined)
                if int_of(v, b.get(3).copied().unwrap_or(ValueId::UNIT)) != -2
                    && let Some(&j) = self.cx.args().get(1)
                {
                    self.ops
                        .scan_guess
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(int_of(v, j), (x, mat));
                }
                Ok(Placed::State)
            }
        }
    }

    /// What a continuation does after the page of break `b`: the scan
    /// state it defines, or the next page's break in its material. With
    /// `rtn` given (a firing step's prediction, [`Run::scan_step`]),
    /// what the routine gives back is that, read nowhere, and nothing is
    /// defined or placed; the state is `None` for a page firing in the
    /// continuation's own material (its node is not known here).
    #[allow(clippy::too_many_lines)]
    fn cont_state(&mut self, b: &[ValueId], rtn: Option<ValueId>, mat: Option<ValueId>) -> R<Cont> {
        let v = self.v;
        if b.len() < 6 {
            return Ok(Cont::Nothing);
        }
        let pid = int_of(v, b[3]);
        if rtn.is_some() && pid == -2 {
            return Ok(Cont::Nothing);
        }
        let Some((after, fire, own)) = self.assemble_page(b, mat, true)? else {
            return Ok(Cont::Nothing);
        };
        let at_end = pid != -2
            && own
                .get(1)
                .is_some_and(|&x| int_of(v, x) == i64::from(fire.k));
        let rest = if at_end { Vec::new() } else { fire.rest };
        let r = match rtn {
            Some(x) => Some(x),
            None => self.eng_read(rtn_field(v, int_of(v, b[0])))?,
        };
        let rs = r.map(|x| list_of(v, x)).unwrap_or_default();
        let mut material = rs.first().map(|&x| list_of(v, x)).unwrap_or_default();
        let held = list_of(v, fire.data)
            .get(3)
            .map(|&x| list_of(v, x))
            .unwrap_or_default();
        let carried = fire.carried;
        // (the insertion classes of the material carried and held over:
        // their registers as they are here, for the page built at once;
        // a step after this reads the material, not the registers where
        // it is)
        let mut classes: Vec<u16> = Vec::new();
        for &x in held.iter().chain(carried.iter()) {
            if let Some(Node::Ins(i)) = nodes::decode_shallow(v, x, &mut Side::default())
                && !classes.contains(&i.number)
            {
                classes.push(i.number);
            }
        }
        material.extend(held);
        material.extend(rs.get(1).map(|&x| list_of(v, x)).unwrap_or_default());
        material.extend(carried);
        // (a firing step's prediction reads the registers where its page
        // fires, the piece `pid`, as its fold does: not where the step
        // is, past the finishers and continuations it places itself,
        // whose definitions change with what it places from those reads.
        // The continuation reads them where it is, after the finisher,
        // and defines the state the step reads, [`scan_page_field`])
        let at =
            rtn.map(|_| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(pid.to_ne_bytes())));
        for k in classes {
            let m = match at {
                Some(at) => {
                    let c = self.insert_class_at(k, at)?;
                    self.ins_marker(k, &c)
                }
                None => self.ins_params_marker(k)?,
            };
            material.push(m);
        }
        material.push(v.list(vec![v.int(nodes::T_BUILD_PAGE), v.int(0)]));
        // (where the rest of a piece begins in the material, and that
        // piece: a page firing in it is checked against the piece,
        // `place_break`'s origin)
        let rest_at = material.len();
        let origin = if pid == -2 {
            b.get(6).copied().filter(|&o| o != ValueId::UNIT)
        } else {
            Some(v.list(vec![
                b[3],
                own.get(1).copied().unwrap_or(ValueId::UNIT),
                own.get(3).copied().unwrap_or(ValueId::UNIT),
                own.get(4).copied().unwrap_or(ValueId::UNIT),
                own.get(5).copied().unwrap_or(ValueId::UNIT),
                own.get(6).copied().unwrap_or(ValueId::UNIT),
            ]))
        };
        material.extend(rest);
        if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
            eprintln!(
                "cont {}: after page {} piece {} candidate {} at_end {at_end} rtn read {} routine's {} held+carried {} rest {}",
                self.cx.node().raw(),
                int_of(v, b[0]),
                pid.cast_unsigned(),
                fire.k,
                r.is_some(),
                rs.get(1).map_or(0, |&x| list_of(v, x).len()),
                rest_at.saturating_sub(1),
                material.len() - rest_at
            );
        }
        let dead = if let Some(at) = at {
            self.resolve_params_of(pid, &mut material)?;
            if has_end(v, &material) {
                let f = self.loc_field(crate::fields::cell::DEADCYCLES as Loc);
                match self.cx.use_field_at(f, Some(at))? {
                    phitex_defuse::FieldAt::Def(x, _) => int_of(v, x),
                    phitex_defuse::FieldAt::Unset => 0,
                    phitex_defuse::FieldAt::Gone => self.cint(crate::fields::cell::DEADCYCLES)?,
                }
            } else {
                0
            }
        } else {
            self.resolve_page_params(&mut material)?;
            self.deadcycles_for(&material)?
        };
        let s = ScanIn {
            anchor: None,
            j: after.j,
            normal: after.normal,
            fold: after.fold,
            params: after.params,
            prof: Vec::new(),
            material: Vec::new(),
        };
        // (the window of the next page's steps begins after the piece
        // the page fired in: nothing between it and this node gives
        // units (its step, the finisher, this node), so the state names
        // a node of the text, which an edit elsewhere keeps, and a
        // continuation made anew can be given it as its guess,
        // [`Ops::scan_guess`]; a page firing in a continuation's own
        // material has no piece: this node)
        let me = if pid == -2 {
            i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes())
        } else {
            pid
        };
        // (the state without where it stands among the pages, `j` and
        // the ordinary pages before: [`SCAN_AT`])
        let state = |prof: ValueId, material: Vec<ValueId>| {
            v.list(vec![
                v.int(me),
                v.int(0),
                v.int(0),
                v.int(i64::from(s.fold.sf)),
                v.int(i64::from(s.fold.pd)),
                v.int(i64::try_from(s.fold.since_bp).unwrap_or(0)),
                s.params.unwrap_or(ValueId::UNIT),
                prof,
                v.list(material),
            ])
        };
        let mut ps = s.page_state();
        let fire = {
            let mut live = |n: u16| match at {
                Some(at) => self.insert_class_at(n, at),
                None => self.insert_class(n),
            };
            ps.fold_items(v, &material, dead, &mut live)?
        };
        if let Some(fire) = fire {
            if crate::exec::obs_flag!("PHITEX_SCAN_TRACE") {
                eprintln!(
                    "scan {}: page {} fires in the continuation at candidate {}",
                    self.cx.node().raw(),
                    ps.j,
                    fire.k
                );
            }
            if rtn.is_some() {
                return Ok(Cont::Nothing);
            }
            // (its break: the material from the state before it, which
            // the page's finisher and continuation fold again)
            let in_rest = material.len() - fire.rest.len() > rest_at;
            let src = state(crate::vprof::value(v, &[]), material);
            return Ok(Cont::Fire(
                ps.j - 1,
                src,
                fire.k,
                fire.rest.len(),
                ps.normal,
                fire_special(v, fire.data),
                origin.filter(|_| in_rest),
            ));
        }
        // (the state is what the page builder decides from: the markers
        // that only order the writer's records among the pages (a write,
        // a font loaded, a PDF write, §1370) it passes over, and page
        // parameters set to the ones in force change nothing; the
        // material that holds them reaches the next page's finisher by
        // its own read, [`SCAN_MAT`]. A state holding them would differ
        // from a prediction on every page whose routine writes another
        // number of them or sets its parameters again)
        let mut in_force = s.params;
        let decides: Vec<ValueId> = material
            .iter()
            .copied()
            .filter(|&x| match nodes::tag(v, x) {
                nodes::T_WRITE_AT | nodes::T_FONT_AT | nodes::T_PDFW_AT => false,
                nodes::T_PAGE_PARAMS if list_of(v, x).len() > 1 => {
                    let same = in_force == Some(x);
                    in_force = Some(x);
                    !same
                }
                _ => true,
            })
            .collect();
        let prof = crate::vprof::encode(v, &decides);
        let x = state(crate::vprof::value(v, &prof), Vec::new());
        Ok(Cont::State(
            x,
            v.list(material),
            v.list(vec![v.int(s.j), v.int(s.normal)]),
        ))
    }
}
