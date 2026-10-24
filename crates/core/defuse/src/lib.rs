//! The engine: an incremental, parallel, field-level def-use evaluator.
//!
//! Written from `docs/linear-core.md`, the toy language in `examples/common`
//! and the test suites in `tests/`. It knows nothing of TeX: no TeX name,
//! type or rule enters it, and no crate here depends on one.

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
    Brackets, Candidate, CapPolicy, Cause, Claim, EarlyLoop, LoopOptions, PathId, RunAudit, Step, WastedRun, Edit, EditQueue, EditSource, Effect, EnvBase, EnvProvider, EvalError, Hold, Hop,
    Executor, FieldAt, Greedy, Job, Lifted, Limits, Locality, LoopStart, MayDefine, Monoid, NoEdits, NodeId,
    NodeInfo, NodeState, OpId, Operation, ProjId, Projection, RandomScheduler, Report, Rerun, RoundTimes, LinkTimes, MergeTimes, RunResult,
    Scheduler, SeqMonoid, Sequential, Settled, Speculation, Stop, SumMonoid, UnitId, Until, WaitKind, WaitOn, Waiter,
};
pub use clean::{Ctx, Engine, Part, View};
pub use values::{Field, FxHashMap, FxHashSet, FxHasher, Rng, SeqNode, Value, ValueId, Values, hash_bytes, hash_words};
