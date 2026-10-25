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
//! - **Layer 4, the page builder** ([`builder`]). **One** node,
//!   [`Pages`], pushed where the job begins: TeX's page builder
//!   (§980–§1010) as a loop over the profiles, piece by piece
//!   ([`phitex_defuse::Ctx::acc_next`]), microseconds a paragraph. Where
//!   a page fires it notes the page ([`FinishIn`]: where each item on it
//!   came from, the builder's decisions); at its end it places every
//!   page's finisher right after the piece the page fired in and defines
//!   [`PAGES_END`]. No scan per page, no continuation, no chain of page
//!   nodes.
//! - **Layer 5, the finishers** ([`finish`]). One [`Finish`] a page: it
//!   assembles its page from its [`FinishIn`] alone (the pieces' items by
//!   range, the routine's material by value), runs the output routine
//!   (§1012–§1026) as one run of its own, and ships the page. Finishers
//!   run in parallel: what one reads of the one before is a field, read
//!   on its prediction ([`FinDefs::predict`]).
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

mod builder;
mod feedback;
mod finish;
mod piece;
mod seal;

pub(crate) use builder::*;
pub(crate) use feedback::*;
pub(crate) use finish::*;
pub(crate) use piece::*;
pub(crate) use seal::*;

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

/// Where the page after a finisher's begins, defined by that finisher for
/// the text after it: `[anchor, j, normal, sf, pd, since_bp, params,
/// profile]`, the piece the page fired in (the text after it folds the
/// profiles after that piece), the pages fired, the ordinary ones, the
/// main list's fold, the page parameters in force, and what the next
/// page begins with as a profile (the held-over insertions, the routine's
/// list, the material carried, the rest of the piece). Read only by
/// `\pagegoal`, `\pagetotal` and their kin in the text
/// ([`Feedback::PageSoFar`], [`Run::page_dimen`]): nothing else reads a
/// finisher's view of the next page.
pub(crate) const PAGE_START: &str = "pages:start";

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
            let k = PageNode::of(self.ops, self.v, self.cx.op(), &self.cx.args());
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

/// The field every finisher defines at its position: what its routine
/// gave back ([`RTN`]'s value) as `[piece, nth, output]`, the piece its
/// page fired in and the pages fired in it before. The step after a
/// finisher reads it there; one naming another page (its finisher not
/// yet in) is no answer: the step runs on its guess and is re-asked when
/// its finisher's comes in.
pub(crate) const PAGE_OUT: &str = "pages:out";

/// One unit from every finisher's run, at its position: the finishers
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
    // (read as the count against the pages: an answer that stays the
    // same while the count grows short of them)
    let p = cx.path(&[phitex_defuse::Step::Len, phitex_defuse::Step::Cmp(n)]);
    let c = cx.fold_path(v.field(FIN_DONE), None, None, p)?;
    Ok(c.and_then(|c| v.as_int(c)).is_some_and(|c| c >= 0))
}

/// [`PAGE_OUT`]'s value.
pub(crate) fn page_out_value(v: &Values, piece: phitex_defuse::NodeId, nth: u32, out: ValueId) -> ValueId {
    v.list(vec![v.int(i64::from_ne_bytes(piece.raw().to_ne_bytes())), v.int(i64::from(nth)), out])
}

/// The output in a [`PAGE_OUT`] value, where it names page `(piece,
/// nth)`.
pub(crate) fn page_out_of(v: &Values, x: ValueId, piece: phitex_defuse::NodeId, nth: u32) -> Option<ValueId> {
    match list_of(v, x)[..] {
        [p, n, out] if int_of(v, p) == i64::from_ne_bytes(piece.raw().to_ne_bytes()) && int_of(v, n) == i64::from(nth) => Some(out),
        _ => None,
    }
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
