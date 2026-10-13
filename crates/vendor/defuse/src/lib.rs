//! defuse: an incremental, parallel, field-level def-use evaluator.
//!
//! Rewritten from `docs/linear-core.md`, the toy language in `examples/common`
//! and the test suites in `tests/`, without reference to the previous core.

// Engine code keeps short local names (s: slot, u: unit, f: field, r:
// record) and long state machines in one function each.
#![deny(unsafe_code)]
#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

mod api;
mod core;
pub mod obs;
#[allow(unsafe_code)]
mod sync;
mod values;

pub use api::{
    Brackets, Candidate, Cause, Claim, Edit, EditQueue, EditSource, Effect, EnvProvider, EvalError, Executor, Greedy,
    Job, Lifted, Limits, Locality, LoopStart, MayDefine, Monoid, NoEdits, NodeId, NodeInfo, NodeState, OpId, Operation,
    RandomScheduler, Report, RunResult, Scheduler, SeqMonoid, Sequential, Settled, Speculation, Stop, SumMonoid,
    UnitId, Until, WaitKind, WaitOn, Waiter,
};
pub use core::ctx::Ctx;
pub use core::edit::Engine;
pub use values::{Field, FxHashMap, FxHashSet, FxHasher, Rng, SeqNode, Value, ValueId, Values, hash_bytes, hash_words};
