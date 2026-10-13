//! An incremental, parallel, field-level def-use graph evaluator.
//!
//! Programs are units of ordered nodes: pure applications of client
//! [`Operation`]s. A node uses fields, defines fields, produces an output,
//! may unfold into child nodes (spliced in at its position), may claim the
//! nodes that follow it as its argument text, and may open or close scopes.
//!
//! The static order is only for binding, never for execution. A use binds to
//! its reaching definition (the nearest earlier visible definition), found in
//! O(log n) from bracket summaries; a node waits only for the value of that
//! definition, or for pending structure (a definition set, claim or scope
//! effect that depends on a value) that its own binding walk crosses.
//! Structure that can be read from the text (claim extents, scope brackets)
//! comes from client hooks before anything runs.
//!
//! Edits re-bind exactly the affected uses and mark exactly the affected
//! nodes; evaluation re-runs those with early cutoff. Scheduling
//! ([`Scheduler`]) and execution ([`Executor`]) are pluggable; speculative
//! runs read predictions, may change program shape provisionally, and are
//! confirmed in bulk when the structure or value they assumed resolves.
//!
//! Node ids and value ids are content hashes, so results, ids and the
//! canonical report counters are the same under every schedule.
//!
//! No dependencies, no `unsafe`, no threads: builds for
//! `wasm32-unknown-unknown`.
//!
//! Profiling builds (`--features profile`) add counters, timed spans and a
//! Chrome trace ([`obs`]); without the feature they compile to nothing.
#![forbid(unsafe_code)]
// Treap and graph code reads best with the conventional short names
// (t, l, r, x, s, ...).
#![allow(clippy::many_single_char_names)]

mod ctx;
mod engine;
mod hash;
pub mod obs;
mod sched;
mod tree;
mod value;

pub use ctx::{Ctx, Stop};
pub use engine::{
    Brackets, Cause, Claim, Edit, EditQueue, EditSource, Effect, Engine, EnvProvider, EvalError, Lifted, Limits,
    Locality, LoopStart, MayDefine, Monoid, NoEdits, NodeId, NodeInfo, NodeState, OpId, Operation, Report, RunResult,
    SeqMonoid, Settled, SumMonoid, UnitId, Until, WaitKind, WaitOn, Waiter,
};
pub use hash::{FxHashMap, FxHashSet, FxHasher, hash_bytes, hash_words};
pub use sched::{Candidate, Executor, Greedy, Job, RandomScheduler, Rng, Scheduler, Sequential, Speculation};
pub use value::{Field, SeqNode, Value, ValueId, Values};

impl Engine {
    /// Evaluates with every ready node run in order, without speculation.
    ///
    /// # Errors
    ///
    /// See [`Engine::evaluate`].
    pub fn evaluate_sequential(&mut self) -> Result<Report, EvalError> {
        self.evaluate(&mut Greedy::default(), &mut Sequential)
    }
}
