//! The engine: a mergeless, parallel, incremental def-use evaluator.
//!
//! # The picture
//! A document is one line of nodes. Each node is a pure function of what it
//! reads: the definitions of fields reaching its position, the contributions
//! made before it, the environment. It defines fields, contributes to
//! sequences, places children and continuations after itself, and outputs a
//! value. Nothing here knows what a node computes.
//!
//! A read of field `f` at position `p` gives the reaching definition: the last
//! definition of `f` before `p`, or the environment's value. A definition
//! landing at `q` re-asks only the readers of `f` between `q` and the next
//! definition of `f`. Every node runs at once on what it can read now, records
//! what it saw, and is final when what it saw still reaches it. Nothing waits
//! for anything: there is no wait, hold, park, careful mode, round or step.
//!
//! # The structure (what makes it mergeless)
//! There is no world. The line is cut into segments, bounded ranges of
//! positions ([`order`], [`segment`]). A segment owns its nodes, its
//! definitions, its readers, its contributions and their fold. A commit locks
//! the committing node's own segment and nothing else ([`commit`]); the walk
//! forward that re-asks readers takes read locks of later segments one at a
//! time and flips atomics on nodes. A per-field index ([`index`]) names the
//! segments defining each field, so neither a read nor a commit visits a
//! segment that has nothing of its field. Workers own deques and steal
//! ([`sched`]); there is no central queue, no step, no round, no barrier, no
//! merge and no rebase.
//!
//! # The serial truths, handled without waiting
//! A definition that is "what reached me, plus c" ([`Ctx::define_rel`]) is
//! kept as a relation and recomputed in place when its input changes, without
//! running its node; and once an operation has made such a definition, every
//! pending node of that operation is given the same relation ahead of its run
//! ([`commit::learn_shape`]), so a chain of a thousand counters costs a
//! thousand additions and its readers run side by side. A fold over
//! contributions ([`Ctx::fold`]) is a monoid over per-segment sums, so a step
//! of a page-like fold is logarithmic in the document.
//!
//! # Rules
//! No TeX name, type or rule enters this crate. No lock is held across a run
//! of an operation or across two segments. Every lock in the crate is listed
//! at the top of [`lock`]; a new one needs a row there.

#![deny(unsafe_code)]
#![allow(clippy::many_single_char_names, clippy::too_many_lines, clippy::too_many_arguments)]

mod api;
mod commit;
mod ctx;
mod engine;
mod index;
mod lock;
mod node;
pub mod obs;
mod order;
mod read;
mod sched;
mod segment;
mod table;
mod values;

pub use api::{
    At, Brackets, Cause, EditSource, Effect, EnvProvider, EvalError, FieldAt, Limits, Locality,
    MayDefine, Monoid, NoEdits, NodeId, NodeInfo, NodeState, OpId, Operation, PathId, ProjId, Projection, Report,
    SeqMonoid, Settled, Step, Stop, SumMonoid, Times, UnitId,
};
pub use ctx::{Ctx, View};
pub use engine::Engine;
pub use values::{Field, FxHashMap, FxHashSet, FxHasher, SeqNode, Value, ValueId, Values, hash_bytes, hash_words};
