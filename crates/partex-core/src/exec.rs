//! Parallelism abstraction (see `DESIGN.md` §7.14, "Executor").
//!
//! The core never names a threading library. The one implementation lives
//! in the language-independent runtime (`partex_incr::exec`), which the
//! region rounds use too; the core re-exports it. Results are always
//! consumed in program order, so the schedule cannot influence output;
//! [`Sequential`] is the reference.

pub use partex_incr::exec::{Executor, Sequential};
