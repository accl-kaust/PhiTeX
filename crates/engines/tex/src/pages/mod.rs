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

mod builder;
mod feedback;
mod finish;
mod piece;

pub(crate) use builder::*;
pub(crate) use feedback::*;
pub(crate) use finish::*;
pub(crate) use piece::*;

/// The pieces' main-list items, one unit per piece (and per paragraph's
/// lines, `crate::parbreak`) beside its profile: `[piece, last, items,
/// reads, defs, locals, cut]` (its raw node id, its last candidate when
/// nothing came after it, its items with their markers resolved, what a
/// finisher's check needs, [`Run::check_cut`], and for a piece cut at its
/// last candidate the bindings local to the groups open there, which the
/// routine sees). Read by range by a finisher: its page's real items.
pub(crate) const PITEMS: &str = "pitems";

/// A cut's continuation, `[cont, cutting node, text kind, claim guess,
/// candidate]`
/// ([`Run::cont_encode`]): defined by the piece that cuts, read by the
/// resume the piece places after itself ([`Run::fold_here`]). A field,
/// not the resume's arguments: the input left changes with any edit
/// before the document's end, and a resume made anew re-makes everything
/// after it (spec-predicted-breaks §7).
pub(crate) const RESUME: &str = "pages:resume";

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

/// The finishers whose routine gave back to the main list other than the
/// page builder guessed (§1026; [`FinishIn::rtn_guess`]): one unit
/// `[j, rtn]` per such finisher, none from the others (each declares none,
/// [`Finish::static_contribs`]). Read whole by the page builder's node,
/// which folds page `j + 1` on the real material where a unit says so
/// ([`Feedback::RoutineGivesBack`]).
pub(crate) const PAGE_MISS: &str = "pages:miss";

/// A `T_SET_PAGE` marker's slot for `\deadcycles` set by the text
/// (§1246): after the eight of `\pagegoal` and its kin (§1245).
pub(crate) const DEAD_SLOT: i64 = 8;

/// [`PAGE_MISS`]'s fold: a sequence, read past its contributors still in
/// work as they last contributed (nothing, for a node that never did,
/// [`phitex_defuse::Monoid::speculate`]). Only finishers contribute to
/// it, and only where they miss: the page builder reads it whole at its
/// loop's start without waiting for the text after it, and runs again
/// where a finisher's miss comes in.
pub(crate) struct MissMonoid;

impl phitex_defuse::Monoid for MissMonoid {
    fn empty(&self, _: &Values) -> ValueId {
        ValueId::NIL
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.seq_concat(a, b)
    }
    fn speculate(&self) -> bool {
        true
    }
}

/// The page accumulators' declared contributions of a node that gives
/// none of them: so a range read of a window holding it does not wait
/// for it, only for the pieces (and the finishers' misses, guessed none).
pub(crate) fn no_units(v: &Values) -> Vec<(Field, ValueId)> {
    vec![
        (v.field(crate::vprof::VPROF), ValueId::NIL),
        (v.field(PITEMS), ValueId::NIL),
        (v.field(PAGE_MISS), ValueId::NIL),
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
    /// profiles and the finishers' misses where the pages are built, a
    /// page's items where it is assembled, the finishers' global
    /// definitions after a cut. Its read hint names each of them: the
    /// engine's gate holds the node while a contributor before it is in
    /// work, where its run would start, stop on the fold and run again.
    /// [`Run::fold_own`] folds only these (checked under `observe`).
    pub(crate) const fn folds(self) -> &'static [&'static str] {
        match self {
            PageNode::Pages => &[crate::vprof::VPROF, PAGE_MISS],
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

    /// The read hint: the fields `gate` the node cannot run without, and
    /// [`PageNode::folds`].
    pub(crate) fn reads(self, v: &Values, gate: &[&str]) -> Vec<Field> {
        gate.iter()
            .copied()
            .chain(self.folds().iter().copied())
            .map(|f| v.field(f))
            .collect()
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
            None => self.cx.acc_range(f, from)?,
            Some(_) => self.cx.acc_range_to(f, from, to)?,
        })
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
