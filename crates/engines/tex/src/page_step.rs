//! The light page step (OUTPUT.md §4.2a): a command that reaches a
//! `build_page` call (§994) with nothing after it leaves the fold to this
//! node, its continuation, so the command itself never reads the page.

use phitex_defuse::{Ctx, Effect, Field, Locality, MayDefine, Operation, Stop, ValueId, Values};

use crate::ops::{Shared, client_only, ops, run_node};

/// The light page step (OUTPUT.md §4.2a): the fold of the main list's
/// contributions at a `build_page` call the command before it deferred.
///
/// With an output routine (`any` false) the step defines only the page
/// builder's fields: the routine itself runs in its children (`OutPage`
/// and the routine's text), which declare their own. With none (§1023,
/// `any`), the step ships the page itself, and a shipout may define
/// anything: `Any`, a separate operation so the runs count apart
/// (`page_steps_any`).
pub(crate) struct PageStep {
    pub shared: Shared,
    pub any: bool,
}

/// What a step with an output routine defines (§1012–§1025): the fold's
/// state, the marks (e-TeX's classes too), the insertion boxes (`box:255` goes to the routine
/// as an argument), `\outputpenalty` and `\deadcycles`.
const STEP_DEFINES: [&str; 6] = [
    "page:state",
    "mark:",
    "marks:",
    "box:",
    "param:outputpenalty",
    "deadcycles",
];

impl Operation for PageStep {
    fn name(&self) -> &'static str {
        if self.any { "pagestep_any" } else { "pagestep" }
    }
    fn may_define(&self, _: &[ValueId], _: &Values) -> MayDefine {
        if self.any {
            MayDefine::Any
        } else {
            MayDefine::Prefixes(STEP_DEFINES.iter().map(|&p| p.to_owned()).collect())
        }
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define_field(&self, _: &[ValueId], v: &Values, f: Field) -> bool {
        client_only(v, f)
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::Pending
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        #[allow(clippy::redundant_closure_for_method_calls)]
        run_node(cx, ops(&self.shared), |r| r.page_fold())
    }
}
