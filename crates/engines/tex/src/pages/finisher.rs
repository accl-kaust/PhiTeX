//! Layer 5: one [`Finish`] node per page (docs/spec-layers.md §2; the
//! rules in `SPEC.md` §2.3, §3).
//!
//! A finisher is a child of the page loop, placed right after the piece
//! its page fired in and after the finishers of earlier pages fired in
//! the same piece, so its routine's definitions reach the text after the
//! page in TeX's order. Its arguments are its key and what the engine's
//! predictions need (`SPEC.md` §3.3) and nothing of the page's content:
//! the page is the loop node's definition [`super::PAGE_OF`]`:<piece>:<nth>`,
//! read at the unit's end ([`phitex_defuse::At::End`]). So a page whose
//! content moves re-asks its finisher; a finisher is made anew only when
//! a page appears or vanishes.
//!
//! The run: assemble the page from its [`Page`] alone (the pieces' items
//! through the `pitems` window, by path; the routines' material by
//! value), replay the builder on real nodes to the same fire ([`super::replay`]),
//! run the output routine on the piece's state at the candidate
//! ([`super::routine`], moved verbatim from the old glue), ship the page,
//! and publish: the routine's definitions at this position, the
//! give-back as the output (read by the loop through its placement) and
//! as [`PAGE_OUT`]`:<piece>:<nth>`, [`super::PAGE_START`] for the text's
//! page-state reads, [`super::GDEFS`], the page key for the PDF side,
//! [`super::FIN_DONE`].
//!
//! A stale finisher (a piece it names ran again since the loop folded it,
//! or its page's piece stands after it in the view) makes no page and
//! publishes nothing new (`Ctx::unchanged`), its output the loop's guess:
//! the loop re-fires the page on the new version and the finisher is
//! re-asked with the new page.

use phitex_defuse::{At, Ctx, Effect, MayDefine, Operation, Stop, ValueId, Values};

use super::{Page, PageKey};
use crate::ops::{Shared, ops};
use crate::run::{R, Run};

/// A page's give-back, defined by its finisher alone:
/// `pages:out:<piece>:<nth>` = `[markers, list, \deadcycles]` (`SPEC.md`
/// §3.2 "give-back"). The loop reads it through its placement
/// ([`phitex_defuse::Ctx::placed_output`]); the field is for the text's
/// tools and traces.
pub(crate) const PAGE_OUT: &str = "pages:out";

/// The finisher's arguments: `[piece, nth, hint, class, special]`, the
/// page's key and what [`Finish::predicted_defs`] and
/// [`Finish::predicted_rels`] key the prediction history by (`SPEC.md`
/// §3.3: by routine and page class, never by page content).
pub(crate) struct FinishArgs {
    pub key: PageKey,
    /// The output routine's hint where the page fired.
    pub hint: ValueId,
    /// `\outputpenalty`'s class: every ordinary page (`> -10001`) one
    /// class, each special penalty its own.
    pub class: i64,
    pub special: bool,
}

impl FinishArgs {
    pub(crate) fn encode(&self, v: &Values) -> Vec<ValueId> {
        todo!("FinishArgs::encode")
    }

    pub(crate) fn decode(v: &Values, args: &[ValueId]) -> Option<FinishArgs> {
        todo!("FinishArgs::decode")
    }
}

pub(crate) struct Finish(pub Shared);

impl Operation for Finish {
    fn name(&self) -> &'static str {
        "finish"
    }

    fn contributes(&self, _: &[ValueId], v: &Values, f: phitex_defuse::Field) -> bool {
        todo!("as the old glue's Finish::contributes: the finisher's contributions (gdefs, pages:fin, the PDF side's), never vprof/pitems")
    }

    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        todo!("as the old glue's Finish::may_define (finish.rs at bc394b811): the routine hint's declared names, the page fields")
    }

    /// `SPEC.md` §3.3: the history of this routine and page class
    /// (`FinDefs`), keyed by `(piece, nth)` for the page's own last run,
    /// else the routine's last two; never a page after it.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(phitex_defuse::Field, ValueId)>> {
        todo!("FinDefs::predict, moved verbatim")
    }

    /// Page counters as relations: +1 where the class's last run shipped.
    fn predicted_rels(&self, args: &[ValueId], v: &Values) -> Option<Vec<(phitex_defuse::Field, i64)>> {
        todo!("FinDefs::rels, moved verbatim")
    }

    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }

    /// 1. `args` = [`FinishArgs::decode`]; the page =
    ///    `read_at(PAGE_OF:<piece>:<nth>, At::End(unit))` decoded as
    ///    [`Page`]. Unset: the loop has not fired this page yet (the
    ///    placement is older than the loop's last commit): `unchanged`,
    ///    output unit.
    /// 2. Stale (`SPEC.md` §3.1 "stale placement"): `unchanged`, output
    ///    the guess the page carries.
    /// 3. [`Run::assemble_page`], [`super::replay`], [`super::routine`]
    ///    (`fire_page`, `output_end`, `ship_out`), then
    ///    [`Run::publish_page`].
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let o = ops(&self.0);
        let mut r = Run::new(cx, o);
        r.finish_page()
    }
}

impl Run<'_, '_> {
    /// [`Finish::run`]'s steps 1–3.
    pub(crate) fn finish_page(&mut self) -> Result<ValueId, Stop> {
        todo!("finish_page: the old glue's Finish::run and fin_page (finish.rs at bc394b811) with the page read from PAGE_OF at At::End(unit) instead of AfterOwn(step), and nothing read of any step")
    }

    /// `SPEC.md` §3.1: the `pitems` window `(after, piece]` read through
    /// paths, only the units the page's spans name, at the items they
    /// name; the own piece whole where the page fired in the material
    /// before it; markers resolved where each piece is. Returns what the
    /// replay folds and whether the placement is stale.
    pub(crate) fn assemble_page(&mut self, page: &Page) -> R<Option<super::Assembled>> {
        todo!("assemble_page: window_units + stale_placement + items_at + resolve_params_of, moved from finish.rs/builder.rs at bc394b811; the version check by the vprof unit's identity")
    }

    /// `SPEC.md` §3.2 "give-back", "globals", "where the next page
    /// begins", "count of finishers": the definitions and contributions
    /// at this position, the output.
    pub(crate) fn publish_page(&mut self, page: &Page, fired: &super::Shipped) -> R<ValueId> {
        todo!("publish_page: define_rtn, contribute_global_defs, define_page_start, pdf:pagekeys, pages:fin; PAGE_OUT:<piece>:<nth>; the output = the give-back value")
    }
}
