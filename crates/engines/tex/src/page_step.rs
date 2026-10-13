//! The light page step (OUTPUT.md §4.2a): a command that reaches a
//! `build_page` call (§994) with nothing after it leaves the fold to this
//! node, its continuation, so the command itself never reads the page.

use phitex_defuse::{Ctx, Effect, Field, Locality, Operation, Stop, ValueId, Values};

use crate::ops::{Shared, client_only, ops, run_node};

/// The light page step (OUTPUT.md §4.2a): the fold of the main list's
/// contributions at a `build_page` call the command before it deferred.
pub(crate) struct PageStep(pub Shared);

impl Operation for PageStep {
    fn name(&self) -> &'static str {
        "pagestep"
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
        run_node(cx, ops(&self.0), |r| r.page_fold(0))
    }
}
