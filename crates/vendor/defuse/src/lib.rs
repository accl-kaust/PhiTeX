//! defuse: an incremental, parallel, field-level def-use evaluator.
//!
//! Rewritten from `docs/linear-core.md`, the toy language in `examples/common`
//! and the test suites in `tests/`, without reference to the previous core.

// Engine code keeps short local names (s: slot, u: unit, f: field, r:
// record) and long state machines in one function each.
#![deny(unsafe_code)]
#![cfg_attr(feature = "clean", allow(dead_code, unused_imports, unused_macros))]
#![allow(clippy::many_single_char_names, clippy::too_many_lines)]

mod api;
#[cfg(feature = "clean")]
#[allow(dead_code)]
mod clean;
#[cfg(not(feature = "clean"))]
mod core;
pub mod obs;
#[cfg(feature = "observe")]
mod record;
#[allow(unsafe_code)]
mod sync;
#[cfg(all(feature = "observe", not(feature = "clean")))]
mod trace;
mod values;

pub use api::{
    Brackets, Candidate, CapPolicy, Cause, Claim, Edit, EditQueue, EditSource, Effect, EnvProvider, EvalError,
    Executor, FieldAt, Greedy, Job, Lifted, Limits, Locality, LoopStart, MayDefine, MemoMiss, Monoid, NoEdits, NodeId,
    NodeInfo, NodeState, OpId, Operation, ProjId, Projection, RandomScheduler, Report, Rerun, RoundTimes, RunResult,
    Scheduler, SeqMonoid, Sequential, Settled, Speculation, Stop, SumMonoid, UnitId, Until, WaitKind, WaitOn, Waiter,
};
#[cfg(feature = "clean")]
pub use clean::{Ctx, Engine};
#[cfg(not(feature = "clean"))]
pub use core::ctx::Ctx;
#[cfg(not(feature = "clean"))]
pub use core::edit::Engine;
pub use values::{Field, FxHashMap, FxHashSet, FxHasher, Rng, SeqNode, Value, ValueId, Values, hash_bytes, hash_words};
