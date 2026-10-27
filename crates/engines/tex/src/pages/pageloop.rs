//! Layer 4: the page builder as **one node** over the whole text
//! (docs/spec-layers.md §2, §4–§5; the rules in `SPEC.md` §2).
//!
//! [`Pages`] is pushed once, after the mark where the text ends. Its run
//! folds the pieces' profiles ([`crate::vprof::VPROF`]) in text order,
//! one contributor at a time, running TeX's page builder (§980–§1010)
//! over them, microseconds a paragraph. Where a page fires it notes the
//! page ([`Page`]: where every item on it came from and the builder's
//! decisions), defines the page as a field of its own
//! ([`PAGE_OF`]`:<piece>:<nth>`, read by the page's finisher), and places
//! the finisher right after the piece the page fired in. At the text's
//! end it defines [`super::PAGES_END`].
//!
//! **Checkpoints, not a chain.** The loop's state after every fire is a
//! [`Checkpoint`]; the run keeps them all in its [`LoopState`] and stores
//! that with its record ([`phitex_defuse::Ctx::park`]). A run that meets
//! a contributor not committed yet (a hole) parks there: what it fired
//! stands, and the run when the hole fills goes on from the last
//! checkpoint. A run asked for because a contribution moved
//! ([`phitex_defuse::Cause::Range`], its site) goes back to the last
//! checkpoint whose cursor stands before the site, and stops early once
//! the pages it fires again agree with the ones it had (same [`Page`]
//! values from the same cursor): the pages after are unchanged, their
//! finishers re-asked by nothing. Every run re-emits every page's
//! definition and placement it has (the ids are reused from the
//! checkpoints), so the engine keeps the finishers whose page did not
//! move.
//!
//! **What this node never does.** It runs no TeX text, reads no page
//! parameter but through the markers the pieces put on their profiles
//! and the records at the pieces ([`super::page_params_at`]), reads no
//! finisher's state but its give-back through the placement
//! ([`phitex_defuse::Ctx::placed_output`]), and waits for nothing.
//!
//! Identity: one node, arguments `[]`; a page's key is `(piece, nth)`,
//! the piece it fired in and its place among the pages fired in that
//! piece. State is in definitions and the parked [`LoopState`]; nothing
//! of it is an argument of any node.

use phitex_defuse::{Cause, Ctx, Effect, MayDefine, NodeId, Operation, Stop, ValueId, Values};

use super::{Page, PageKey, Span, Src};
use crate::ops::{Shared, ops};
use crate::run::{R, Run};
use crate::vprof::ScanState;

/// The page of a fired page, as its finisher reads it:
/// `pages:page:<piece raw id>:<nth>`, defined by the [`Pages`] node,
/// read `At::End(unit)` (the loop node is the last of the unit).
pub(crate) const PAGE_OF: &str = "pages:page";

/// The loop's state after a fire: everything the next fold needs, and
/// the page it fired, so a run resumes here without folding what came
/// before.
#[derive(Clone)]
pub(crate) struct Checkpoint {
    /// The page this checkpoint ends: its key and its content (the value
    /// defined as [`PAGE_OF`]; reused unchanged when the page did not
    /// move).
    pub key: PageKey,
    pub page: ValueId,
    /// The placement index of the page's finisher in this run's order
    /// (the pages in fire order: checkpoint `i` is placement `i`).
    pub placement: usize,
    /// The builder after the fire, before §1026's give-back is folded:
    /// the state the next page begins with once the finisher's output
    /// (or its guess) is folded on ([`super::after_fire`]).
    pub after: ScanState,
    /// The last contributor folded when the page fired (the cursor of
    /// `next_contribution`), and how far into its units/items the fire
    /// was (the rest of that contributor is the next page's first input).
    pub cursor: Option<NodeId>,
    pub cursor_item: usize,
    /// Where the items of the next page's input so far came from
    /// (`SPEC.md` §2.1 "every fire is noted with its sources"), the
    /// routines' material by page, the marks in force, `\deadcycles`,
    /// the best-break penalties to fold as 10000, the last give-backs by
    /// routine and page class for the guess (`SPEC.md` §2.2): the
    /// `PageLoop` fields of the old glue that are not the builder's own
    /// state, carried verbatim.
    pub carry: super::LoopCarry,
}

/// The parked state of the loop: its checkpoints in fire order and where
/// the fold stands after the last one. Encoded as one value
/// ([`LoopState::encode`]) for `Ctx::park`; the page values inside it are
/// the ids defined as [`PAGE_OF`].
#[derive(Clone, Default)]
pub(crate) struct LoopState {
    pub checkpoints: Vec<Checkpoint>,
    /// The fold after the last checkpoint, up to the hole or the end
    /// (`None`: nothing folded past the last checkpoint).
    pub tail: Option<(ScanState, Option<NodeId>, usize, super::LoopCarry)>,
    /// `\end` ended the job (§1054): the pages are complete.
    pub ended: bool,
}

impl LoopState {
    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        todo!("LoopState::encode: a list value; the Page ids as they are; ScanState and LoopCarry through the old glue's Enc (builder.rs at bc394b811, `encode_state`/`encode_fire`), moved here")
    }

    pub(crate) fn decode(v: &Values, x: ValueId) -> Option<LoopState> {
        todo!("LoopState::decode: the inverse; None on a shape this version does not know (then the run starts from nothing)")
    }

    /// The last checkpoint whose cursor stands before `site` (the node a
    /// moved contribution is at): the run goes back to it. `None`: before
    /// the first page.
    pub(crate) fn before(&self, cx: &Ctx<'_>, site: NodeId) -> Option<usize> {
        todo!("the last i with checkpoints[i].cursor before `site` (Ctx::before); the checkpoint's own cursor contributor counts as before only if `site` is a later node")
    }
}

/// The one page-builder node. Arguments: none. Pushed after the text's
/// end mark by the kick ([`super::kick_args`]: `[KICK, mark]` places it
/// with `follow_after(mark)` and ends).
pub(crate) struct Pages(pub Shared);

impl Operation for Pages {
    fn name(&self) -> &'static str {
        "pages"
    }

    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        // (the page fields it defines are named per page; the engine
        // takes the family by its prefix)
        MayDefine::Names(vec![v.field(super::PAGES_END), v.field(PAGE_OF)])
    }

    fn sibling(&self) -> bool {
        false
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }

    fn contributes(&self, _: &[ValueId], v: &Values, f: phitex_defuse::Field) -> bool {
        !super::from_pieces(v, f)
    }

    /// The loop (`SPEC.md` §2.1–§2.2), from the right checkpoint:
    ///
    /// 1. `state` = the parked [`LoopState`] (`Ctx::resumed`), else empty.
    /// 2. Where to start: on `Cause::Range { by, .. }` or
    ///    `Cause::Changed { site: Some(by), .. }` go back to
    ///    `state.before(by)` (the pages from there on are re-fired; the
    ///    ones before stand); on a hole that filled (`Cause::Range` whose
    ///    site is at or after the tail's cursor) go on from the tail; on
    ///    `Cause::Output { of }` (a finisher's give-back moved) go back to
    ///    the checkpoint of that finisher's page; else from the start.
    /// 3. Fold contributors (`next_contribution(VPROF, cursor)`), each
    ///    unit's items through [`super::fold_unit`] with the builder
    ///    ([`ScanState::fold_items`]); at a fire make the [`Page`]
    ///    ([`super::page_of_fire`]), push the checkpoint, and fold §1026's
    ///    give-back: the finisher's output if it has run
    ///    (`Ctx::placed_output(i)`), else the guess ([`super::guess_give_back`]).
    ///    **Early stop:** a re-fired page whose [`Page`] value equals the
    ///    checkpoint's at the same cursor ends the re-fold: the old
    ///    checkpoints after it stand as they were.
    /// 4. A hole (`next_contribution` stops, `Ctx::stopped_at`): keep the
    ///    tail, park, define and place what is fired, return.
    /// 5. `\end` (§1054) with page and contributions empty and
    ///    `\deadcycles` 0: `ended`; define [`super::PAGES_END`] = pages.
    /// 6. Every run: define every checkpoint's page as
    ///    `PAGE_OF:<piece>:<nth>` (global), place every finisher in fire
    ///    order (`follow_after(piece, finish, Finish::args(key, page))`,
    ///    placement index = checkpoint index), park the state.
    ///
    /// Invariants (observe builds assert them): checkpoints' keys are
    /// unique and in text order; `placement == index`; a page's finisher
    /// is placed after the page's piece; no `pages:loop`, no step, no
    /// node of this operation but this one.
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        if let Some(at) = super::kick_of(cx.values(), cx.args()) {
            cx.follow_after(at, o.pages, Vec::new());
            return Ok(ValueId::UNIT);
        }
        let mut r = Run::new(cx, o);
        r.page_loop()
    }
}

impl Run<'_, '_> {
    /// [`Pages::run`]'s steps 1–6.
    pub(crate) fn page_loop(&mut self) -> Result<ValueId, Stop> {
        todo!("page_loop: SPEC.md §2; the fold of one unit is the old glue's `fold_unit`/`fold_items`/`fire_at`/`after_fire` over `lp` (builder.rs at bc394b811), moved here with `PageLoop` split into ScanState + LoopCarry; the step-chain parts (`cut`, `encode_state` of Pending, `pages:loop`, `step_log`, `kick` chaining) are gone")
    }

    /// The [`Page`] of a fire: `SPEC.md` §2.3 "its page", from the
    /// builder's state where the page began and the sources folded since.
    pub(crate) fn page_of_fire(&mut self, key: PageKey, fire: &super::LoopFire, carry: &super::LoopCarry) -> R<Page> {
        todo!("page_of_fire: FinishIn::args' content as a Page (builder.rs `fire_at`'s FinishIn construction), without `j` and without any later page's content")
    }

    /// §1026's give-back of page `i`'s routine as the loop folds it: the
    /// finisher's output where it has run (`Ctx::placed_output(i)`), else
    /// the guess (`SPEC.md` §2.2 "what the routine gives back").
    pub(crate) fn guess_give_back(&mut self, i: usize, carry: &super::LoopCarry) -> R<ValueId> {
        todo!("guess_give_back: builder.rs `rtn_guess`/`step_rtn`/`last_rtn` rules, keyed by routine hint and page class; a page fired in a routine's material gives nothing back; \\deadcycles never guessed from a run")
    }
}
