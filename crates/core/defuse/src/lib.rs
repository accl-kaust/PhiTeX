//! This repository's side of the engine.
//!
//! The engine is `defuse`: an incremental, parallel, field-level def-use
//! graph evaluator, written without knowledge of TeX and vendored under
//! `crates/vendor/defuse` as a git subtree. Nothing under that path is
//! edited in this repository; what we need from it is asked for as an
//! addition and pulled in with `git subtree pull`.
//!
//! This crate re-exports `defuse` and is the home of what we add around it
//! that still knows nothing of TeX: helpers for naming fields, the replay
//! of recorded workloads (`crates/tools/trace`), and the workspace's lint
//! and target conformance checks.
//!
//! Rule: this crate depends on nothing TeX. If it needs a TeX type, the
//! design has leaked.

pub use defuse::*;
