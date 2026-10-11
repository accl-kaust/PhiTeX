//! The reduction core: TeX-agnostic.
//!
//! A computation is a graph of pending thunks. Reducing one records what it
//! read; after an edit, a thunk whose reads did not change keeps its result,
//! and only what an edit reaches is reduced again. Files a job writes and
//! reads back (`.aux`, `.toc`) are a loop-back, reduced to a fixpoint.
//! Threads reduce independent thunks; the schedule never changes a result.
//!
//! Rule: this crate depends on nothing TeX. A test drives it with a toy
//! language; if the core needs a TeX type, the design has leaked.
