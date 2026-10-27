//! Pages: layers 2 to 5 of docs/spec-layers.md where they meet the main
//! vertical list, on today's engine.
//!
//! - **Layer 2, the pieces** ([`piece`]). A text piece (an `exec` node,
//!   or a `resume` piece) typesets in parallel with every other and never
//!   reads the page. The items it puts on the main vertical list are its
//!   own contributions, delimited by candidate markers where tex.web calls
//!   `build_page` (§994; [`Run::finish_candidate`]). It ends at every
//!   paragraph end it can end at (a cut: its rest is a [`Resume`] placed
//!   after it), and at its end it gives its vertical profile and its
//!   items as units of two accumulators ([`crate::vprof::VPROF`],
//!   [`PITEMS`]).
//! - **Layer 3, the lines** (`crate::parbreak`). A paragraph's line
//!   breaking is a node of its own, a child of the piece that ended the
//!   paragraph; its lines are its own units of the same two accumulators.
//! - **Layer 4, the page builder** ([`pageloop`]). **One** node,
//!   [`Pages`], placed after the text's end: TeX's page builder
//!   (§980–§1010) as a loop over the profiles, contributor by contributor
//!   ([`phitex_defuse::Ctx::next_contribution`]), microseconds a
//!   paragraph. Where a page fires it defines the page ([`Page`]: where
//!   each item on it came from, the builder's decisions) as a field of
//!   its own keyed by [`PageKey`], and places the finisher right after
//!   the piece the page fired in; at the text's end it defines
//!   [`PAGES_END`]. Its state after every fire is a checkpoint it parks
//!   with its record ([`phitex_defuse::Ctx::park`]): a hole, a moved
//!   contribution or a finisher's give-back re-run it from the checkpoint
//!   before, never from the start, and never as a chain of nodes.
//! - **Layer 5, the finishers** ([`finisher`]). One [`Finish`] a page,
//!   named by its [`PageKey`]: it reads its [`Page`] from the loop's
//!   definition, assembles it from the pieces' items by path and the
//!   routines' material by value ([`replay`]), runs the output routine
//!   (§1012–§1026, [`routine`]) as one run of its own, and ships the
//!   page. Finishers run in parallel: what one reads of the one before
//!   is a field, read on its prediction ([`Finish::predicted_defs`]).
//! - **Feedback up the layers** ([`feedback`]): every read of a higher
//!   layer's result by a lower one is a named case, predicted and
//!   validated, never a wait in a chain.
//!
//! On the cells engine (`IncEngine2`'s `docs/mergeless.md`) the layers become
//! ranked units and the finishers need no `follow_after_any`; the code
//! that depends on today's placement is marked "(today's engine)".

use crate::exec::LockWait as _;
use phitex_defuse::{
    Ctx, Effect, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{List, Node};
use phitex_typeset::pack::{PackParams, Spec};
use phitex_typeset::page::{Contents, Fired, InsertClass, Outcome, PageBuilder, PageParams};

use crate::build::{FoldState, IGNORE_DEPTH, Mode, fold_item};
use crate::fields::Par;
use crate::input::{Line, Seg};
use crate::nodes::{self, Side};
use crate::ops::{Shared, ops, run_node};
use crate::run::{Halt, R, Run};
use crate::runstate::{ContSaved, ContSlot, ContState, Loc, State};
use crate::val::{
    glue_of, glue_value, group, int_at, int_of, list_of, str_of, toks_of, toks_value,
};

mod feedback;
mod finisher;
mod pageloop;
mod piece;
mod replay;
mod routine;
mod seal;

pub(crate) use feedback::*;
pub(crate) use finisher::*;
pub(crate) use pageloop::*;
pub(crate) use piece::*;
pub(crate) use replay::*;
pub(crate) use routine::*;
pub(crate) use seal::*;

/// A page's key: the piece its chain of fires began in and its place
/// among the pages of that chain (a page fired in the rest of a piece cut
/// at the page before, a resume node, counts on from that piece: re-cut,
/// the rest is placed anew and the key stands). Identity of its
/// finisher; the name of its field (`pages:page:<piece raw id>:<nth>`,
/// `pages:out:<piece>:<nth>`). An earlier change of the text moves no
/// later page's key unless the page moves to another piece.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct PageKey {
    pub piece: phitex_defuse::NodeId,
    pub nth: u32,
}

impl PageKey {
    pub(crate) fn field(self, v: &Values, family: &str) -> Field {
        v.field(&format!("{family}:{}:{}", self.piece.raw(), self.nth))
    }
}

/// A fired page as the loop defines it and the finisher reads it
/// (`SPEC.md` §2.3 "its page"): the old `FinishIn` without `j` (no page
/// index: a page's content is its own) and without any other page's
/// content. Encoded as one value by [`Page::encode`]/[`Page::decode`]
/// (the old `FinishIn::args`/`of`, moved).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Page {
    /// The page's identity ([`PageKey`]).
    pub key: PageKey,
    /// The contributor the page fired in: the finisher is placed right
    /// after it, its window ends there, and the pages fired in it before
    /// this one give its place among the finishers placed after it.
    pub at: phitex_defuse::NodeId,
    /// The ordinary pages fired up to this one, with it where it is one.
    pub normal: i64,
    /// Whether `\outputpenalty` is -10001 or less (§1013).
    pub special: bool,
    /// The output routine's hint where the page fired
    /// ([`crate::hints::routine_hint`]), encoded; unit for the default
    /// routine.
    pub hint: ValueId,
    /// The insertion classes on the page (§1018: the boxes its fire
    /// fills).
    pub classes: Vec<u16>,
    /// The candidate it fired at (§994's `build_page` call).
    pub k: u32,
    /// Whether it fired in the material the page before left (the
    /// routine's, the material carried), before any item of its piece
    /// after that page's candidate.
    pub in_material: bool,
    /// The window of [`PITEMS`] its spans read: from after this node (none:
    /// the unit's start) to its piece.
    pub after: Option<phitex_defuse::NodeId>,
    /// Where each item the builder took for the page came from, in order,
    /// from where the page began to its fire: the held-over insertions and
    /// the routine's material of the page before first, the material
    /// carried, then the pieces' items.
    pub spans: Vec<Span>,
    /// The routines' material the spans name ([`Src::Rtn`]), by page:
    /// the pages before this one counted back from it (1 the page just
    /// before), as the spans name them.
    pub rtns: Vec<(i64, ValueId)>,
    /// The builder's state where the page began: the main list's fold
    /// and the page parameters in force (a `T_PAGE_PARAMS` marker).
    pub fold: FoldState,
    pub params: Option<ValueId>,
    /// The builder's decisions where it fired (§1005–§1017): the best
    /// break's index on the page, `\outputpenalty` there, whether
    /// `\holdinginserts` was positive, the insertions held over.
    pub best: u32,
    pub output_penalty: i32,
    pub holding: bool,
    pub held: u32,
    /// The marks in force where the page began (`\topmark` and its
    /// classes, §1012): `[[class, mark]…]`.
    pub top_marks: ValueId,
    /// How many of the spans the post-output build of the page before
    /// takes (§1026: the routine's material and the held-over insertions
    /// as they are, a piece's items carried folded again with their
    /// builds and markers spent), and what that build reads besides: the
    /// insertion classes' registers on its material (`T_INS_PARAMS`
    /// markers) and its candidate. None for the first page.
    pub post: u32,
    pub post_markers: Vec<ValueId>,
    pub post_k: u32,
    /// The piece the page before fired in (its raw id, -1 none) and
    /// `\deadcycles` as its routine left it ([`RTN`]'s third element):
    /// what §1054's `\end` reads in the post-output build and in the
    /// rest of that piece.
    pub prev_fire: (i64, i64),
    /// The items of its spans that are the best break of a page before,
    /// a penalty: 10000 since that page fired (§1013).
    pub inf: Vec<Src>,
    /// The version of each piece its spans name, as the page builder
    /// folded it (the piece's [`crate::vprof::VPROF`] unit, which its
    /// [`PITEMS`] unit holds too): a finisher reading a unit of another
    /// version runs on a page fired before that piece ran again, and
    /// makes no page ([`Run::assemble_page`]).
    pub vers: Vec<(i64, ValueId)>,
}

impl Page {
    /// The page as one value (the old `FinishIn::args`' page and its
    /// node's place, moved).
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let raw = |n: phitex_defuse::NodeId| v.int(i64::from_ne_bytes(n.raw().to_ne_bytes()));
        v.list(vec![
            raw(self.key.piece),
            v.int(i64::from(self.k)),
            v.int(i64::from(self.in_material)),
            self.after.map_or_else(|| v.int(-1), raw),
            v.list(self.spans.iter().flat_map(|s| span_ints(v, s.src, s.len)).collect()),
            v.list(self.rtns.iter().flat_map(|&(j, x)| [v.int(j), x]).collect()),
            v.int(i64::from(self.fold.sf)),
            v.int(i64::from(self.fold.pd)),
            v.int(i64::try_from(self.fold.since_bp).unwrap_or(0)),
            self.params.unwrap_or(ValueId::UNIT),
            v.int(i64::from(self.best)),
            v.int(i64::from(self.output_penalty)),
            v.int(i64::from(self.holding)),
            v.int(i64::from(self.held)),
            self.top_marks,
            v.int(i64::from(self.key.nth)),
            v.int(i64::from(self.post)),
            v.list(self.post_markers.clone()),
            v.int(i64::from(self.post_k)),
            v.int(self.prev_fire.0),
            v.int(self.prev_fire.1),
            v.list(self.inf.iter().flat_map(|&src| span_ints(v, src, 1)).collect()),
            v.list(self.vers.iter().flat_map(|&(p, x)| [v.int(p), x]).collect()),
            self.hint,
            v.int(self.normal),
            v.int(i64::from(self.special)),
            v.list(self.classes.iter().map(|&n| v.int(i64::from(n))).collect()),
            raw(self.at),
        ])
    }

    /// [`Page::encode`]'s page; `None` on any other value.
    pub(crate) fn decode(v: &Values, x: ValueId) -> Option<Page> {
        let p = list_of(v, x);
        if p.len() != 28 {
            return None;
        }
        let n = |i: usize| int_of(v, p[i]);
        let ints = |i: usize| list_of(v, p[i]);
        Some(Page {
            key: PageKey {
                piece: node_of(n(0)),
                nth: u32::try_from(n(15)).unwrap_or(0),
            },
            at: node_of(n(27)),
            normal: n(24),
            special: n(25) != 0,
            hint: p[23],
            classes: ints(26).into_iter().filter_map(|x| u16::try_from(int_of(v, x)).ok()).collect(),
            k: u32::try_from(n(1)).unwrap_or(0),
            in_material: n(2) != 0,
            after: (n(3) != -1).then(|| node_of(n(3))),
            spans: ints(4).as_chunks::<5>().0.iter().filter_map(|c| span_of_ints(v, c)).collect(),
            rtns: ints(5).as_chunks::<2>().0.iter().map(|c| (int_of(v, c[0]), c[1])).collect(),
            fold: FoldState {
                sf: i32::try_from(n(6)).unwrap_or(1000),
                pd: i32::try_from(n(7)).unwrap_or(IGNORE_DEPTH),
                since_bp: usize::try_from(n(8)).unwrap_or(0),
            },
            params: some(p[9]),
            best: u32::try_from(n(10)).unwrap_or(0),
            output_penalty: i32::try_from(n(11)).unwrap_or(0),
            holding: n(12) != 0,
            held: u32::try_from(n(13)).unwrap_or(0),
            top_marks: p[14],
            post: u32::try_from(n(16)).unwrap_or(0),
            post_markers: ints(17),
            post_k: u32::try_from(n(18)).unwrap_or(0),
            prev_fire: (n(19), n(20)),
            inf: ints(21).as_chunks::<5>().0.iter().filter_map(|c| span_of_ints(v, c).map(|s| s.src)).collect(),
            vers: ints(22).as_chunks::<2>().0.iter().map(|c| (int_of(v, c[0]), c[1])).collect(),
        })
    }
}

/// A span in a page's value, inline: `tag, a, b, c, len` (tags 0 piece,
/// 1 routine, 2 held), flat in the list.
fn span_ints(v: &Values, src: Src, len: u32) -> [ValueId; 5] {
    let (t, a, b, c) = match src {
        Src::Piece { piece, item } => (0, piece, i64::from(item), 0),
        Src::Rtn { page, item } => (1, page, i64::from(item), 0),
        Src::Held { piece, item, from } => (2, piece, i64::from(item), i64::from(from)),
    };
    [v.int(t), v.int(a), v.int(b), v.int(c), v.int(i64::from(len))]
}

/// [`span_ints`]' span.
fn span_of_ints(v: &Values, c: &[ValueId]) -> Option<Span> {
    let n = |i: usize| c.get(i).map_or(0, |&y| int_of(v, y));
    let u = |i: usize| u32::try_from(n(i)).unwrap_or(0);
    let src = match n(0) {
        0 => Src::Piece { piece: n(1), item: u(2) },
        1 => Src::Rtn { page: n(1), item: u(2) },
        2 => Src::Held { piece: n(1), item: u(2), from: u(3) },
        _ => return None,
    };
    Some(Span { src, len: u(4) })
}

/// The pieces' main-list items, one unit per piece (and per paragraph's
/// lines, `crate::parbreak`) beside its profile: `[piece, last, items,
/// unit, defs, locals, 0, shape, profile]` (its raw node id, its last
/// candidate when nothing came after it, its items with their markers
/// resolved, its state at each candidate for a finisher of a page firing
/// at one inside it, [`Run::check_data`], for a piece cut at its last
/// candidate the bindings local to the groups open there, which the
/// routine sees, its items' shape, [`crate::nodes::unit_shape`], unit
/// where no line is sealed, and its profile's unit). Read by range by a
/// finisher: its page's real items.
pub(crate) const PITEMS: &str = "pitems";

/// The fields a finisher defined globally, one unit per finisher: read
/// by range by the resume after a cut (a global definition replaces
/// the group's local value, §279). A finisher declares its unit as the
/// last run of its routine gave it, a guess ([`Finish::static_contribs`]).
pub(crate) const GDEFS: &str = "gdefs";

/// Where the pages end (`pages:end`): defined by the page builder's node
/// ([`Pages`]) once every page's finisher is placed. Read by what needs
/// every finisher in its range before it runs at all: the written files'
/// nodes (`crate::ops::Files`), whose text is a loop-back definition and
/// must not move after its readers ran again on it, and the output's
/// writers (`crate::assemble`).
pub(crate) const PAGES_END: &str = "pages:end";

/// A `T_SET_PAGE` marker's slot for `\deadcycles` set by the text
/// (§1246): after the eight of `\pagegoal` and its kin (§1245).
pub(crate) const DEAD_SLOT: i64 = 8;

/// A sequence read past its contributors still in work as they last
/// contributed (nothing, for a node that never did,
/// [`phitex_defuse::Monoid::speculate`]); a contribution that comes in
/// otherwise re-asks the reader. For an accumulator few nodes contribute
/// to, read where waiting for every node in the window would chain the
/// readers on the text: the fonts' loads (`crate::pdfout::FONT_LOADS`,
/// a page written apart's provisional numbers, which the link corrects).
pub(crate) struct MissMonoid;

impl phitex_defuse::Monoid for MissMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        ValueId::NIL
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.seq_concat(a, b)
    }
}

/// The page accumulators' declared contributions of a node that gives
/// none of them: so a range read of a window holding it does not wait
/// for it, only for the pieces.
pub(crate) fn no_units(v: &Values) -> Vec<(Field, ValueId)> {
    vec![
        (v.field(crate::vprof::VPROF), ValueId::NIL),
        (v.field(PITEMS), ValueId::NIL),
    ]
}

/// The page nodes, by what their own code folds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageNode {
    /// The page builder ([`Pages`]): one node.
    Pages,
    /// A page's finisher ([`Finish`]).
    Finish,
    /// The rest of a cut piece ([`Run::gdefs_after`], where it loads its
    /// continuation).
    Resume,
}

impl PageNode {
    /// The page node of op `op` with arguments `args`, if one.
    pub(crate) fn of(
        o: &crate::run::Ops,
        _v: &Values,
        op: OpId,
        _args: &[ValueId],
    ) -> Option<PageNode> {
        if op == o.pages {
            Some(PageNode::Pages)
        } else if op == o.finish {
            Some(PageNode::Finish)
        } else if op == o.resume {
            Some(PageNode::Resume)
        } else {
            None
        }
    }

    /// The accumulators the node's own code folds (reads by range): the
    /// profiles where the pages are built, a
    /// page's items where it is assembled, the finishers' global
    /// definitions after a cut. Its read hint names each of them: the
    /// engine's gate holds the node while a contributor before it is in
    /// work, where its run would start, stop on the fold and run again.
    /// [`Run::fold_own`] folds only these (checked under `observe`).
    pub(crate) const fn folds(self) -> &'static [&'static str] {
        match self {
            PageNode::Pages => &[crate::vprof::VPROF],
            PageNode::Finish => &[PITEMS, GDEFS],
            PageNode::Resume => &[GDEFS],
        }
    }

    /// Whether the node's run runs a text (a routine's, the rest of a cut
    /// piece's), whose commands contribute as a text's do.
    pub(crate) const fn runs_text(self) -> bool {
        matches!(self, PageNode::Finish | PageNode::Resume)
    }

    /// Whether the node's declared contributions ([`Operation::
    /// static_contribs`]: its own code's, [`no_units`]) are all it
    /// contributes: so where it runs no text. Then the engine passes it
    /// by in a window of any other accumulator while it is in work.
    /// [`Run::finish`] checks a run against it (under `observe`).
    pub(crate) const fn contribs_complete(self) -> bool {
        !self.runs_text()
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
    pub(crate) fn fold_own(
        &mut self,
        name: &'static str,
        from: Option<phitex_defuse::NodeId>,
        to: Option<phitex_defuse::NodeId>,
    ) -> R<ValueId> {
        if cfg!(feature = "observe") {
            let k = PageNode::of(self.ops, self.v, self.cx.op(), self.cx.args());
            assert!(
                k.is_some_and(|k| k.folds().contains(&name)),
                "node {} ({k:?}) folds {name}, which its read hint leaves out (PageNode::folds)",
                self.cx.node().raw()
            );
        }
        let f = self.v.field(name);
        Ok(match to {
            None => self.cx.fold(f, from, None)?,
            Some(to) => self.cx.fold_through(f, from, to)?,
        })
    }

    /// A view on `name`'s contributions after `from` up to and with
    /// `to`'s own.
    pub(crate) fn view_through(
        &mut self,
        name: &str,
        from: Option<phitex_defuse::NodeId>,
        to: phitex_defuse::NodeId,
    ) -> R<phitex_defuse::View> {
        let f = self.v.field(name);
        Ok(self.cx.view_through(f, from, to)?)
    }
}

/// Whether `f` is what the pieces give the page builder (their profiles and
/// items): the page steps, finishers and page writers never contribute it
/// (`Operation::contributes`), so a step's search for the next piece passes
/// over them while they run.
pub(crate) fn from_pieces(v: &Values, f: Field) -> bool {
    f == v.field(crate::vprof::VPROF) || f == v.field(PITEMS) || f == v.field(LINK_ITEMS)
}

/// The pieces' items again ([`PITEMS`]'s units, each its own
/// contribution), as the link reads them whole: a `crate::val::CatMonoid`
/// fold, not a sequence's.
pub(crate) const LINK_ITEMS: &str = "pdf:items";

/// One from every finisher's run, at its position, summed: the finishers
/// that have run, which the writers count against [`PAGES_END`]'s pages
/// ([`all_finished`]).
pub(crate) const FIN_DONE: &str = "pages:fin";

/// Whether the pages have ended and every page's finisher has run: read
/// as the pages fired and the count of finishers run against them, so a
/// writer before that gives nothing and runs again once the count is in.
pub(crate) fn all_finished(cx: &mut phitex_defuse::Ctx<'_>) -> Result<bool, phitex_defuse::Stop> {
    let v = cx.values();
    let Some(n) = cx.read(v.field(PAGES_END))? else {
        return Ok(false);
    };
    // (read as the count against the pages, a sum: an answer that stays
    // the same while the count grows short of them)
    let p = cx.path(&[phitex_defuse::Step::Cmp(n)]);
    let t0 = std::time::Instant::now();
    let c = cx.fold_path(v.field(FIN_DONE), None, None, p)?;
    crate::timeline::note("pages: all finished (the fold)", t0.elapsed());
    Ok(c.and_then(|c| v.as_int(c)).is_some_and(|c| c >= 0))
}

/// [`PAGE_OUT`]'s value.
pub(crate) fn page_out_value(v: &Values, piece: phitex_defuse::NodeId, nth: u32, out: ValueId) -> ValueId {
    v.list(vec![v.int(i64::from_ne_bytes(piece.raw().to_ne_bytes())), v.int(i64::from(nth)), out])
}

/// A location's value at a candidate in a piece's check data
/// ([`Run::check_data`]) that the piece had neither read nor defined
/// before: what reached the piece, which its page's finisher reads before
/// it ([`phitex_defuse::Ctx::use_field_before`]).
pub(crate) const BEFORE_PIECE: &str = "\u{1}before the piece";

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
/// last item), the conditionals it leaves open.
pub(crate) const PAGE_FIELDS: [&str; 5] = [
    crate::build::MAIN_PD,
    crate::build::MAIN_SF,
    crate::build::MAIN_LAST,
    crate::build::MAIN_SINCE,
    crate::run::COND_STACK,
];
