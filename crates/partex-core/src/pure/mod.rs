//! Pure SSA mode (DESIGN 3.17): the TeX layer on the φ core
//! (`crates/phi`), `PHITEX_SSA_PURE=1`.
//!
//! Each command of `main_control` is one step of the document's unfold.
//! The engine runs the command against its own arrays, a cache of the
//! core's names; the step then replays the command's accesses to the
//! core in order: each name read is checked against the definition that
//! reaches the step (a stale cache entry is loaded and the command run
//! again), each write is a definition, each save level a group. What is
//! not a name is the step's state ([`state::PState`]): the input stack,
//! the nest, the save stack, the conditionals.

pub mod tracker;
mod version;
mod lang;
mod state;

pub use lang::{DOC, Doc, Engine, OUTPUT, Op, Stats, TexLang, Val, install, uninstall};
pub use state::PState;
pub use tracker::PureTracker;
mod driver;
pub use driver::Build;
