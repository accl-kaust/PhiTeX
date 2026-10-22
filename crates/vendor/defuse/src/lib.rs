//! defuse: an incremental, parallel, field-level def-use evaluator.
//!
//! Rewritten from `docs/linear-core.md`, the toy language in `examples/common`
//! and the test suites in `tests/`, without reference to the previous core.

// Engine code keeps short local names (s: slot, u: unit, f: field, r:
// record) and long state machines in one function each.
#![deny(unsafe_code)]
#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

mod api;
mod clean;
pub mod obs;
#[cfg(feature = "observe")]
mod record;
#[allow(unsafe_code)]
mod sync;
mod values;

pub use api::{
    Brackets, Candidate, CapPolicy, Cause, Claim, Edit, EditQueue, EditSource, Effect, EnvBase, EnvProvider, EvalError, Hold,
    Executor, FieldAt, Greedy, Job, Lifted, Limits, Locality, LoopStart, MayDefine, Monoid, NoEdits, NodeId,
    NodeInfo, NodeState, OpId, Operation, ProjId, Projection, RandomScheduler, Report, Rerun, RoundTimes, LinkTimes, MergeTimes, RunResult,
    Scheduler, SeqMonoid, Sequential, Settled, Speculation, Stop, SumMonoid, UnitId, Until, WaitKind, WaitOn, Waiter,
};
pub use clean::{Ctx, Engine};
pub use values::{Field, FxHashMap, FxHashSet, FxHasher, Rng, SeqNode, Value, ValueId, Values, hash_bytes, hash_words};
