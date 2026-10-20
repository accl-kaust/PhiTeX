//! The clean core (`docs/clean-core.md`): persistent per-round worlds,
//! snapshot reads, validated merges.

mod batch;
mod build;
mod claims;
mod seq;
mod core;
mod ctx;
mod engine;
mod eval;
pub(crate) mod ptree;
mod world;

pub use ctx::Ctx;
pub use engine::Engine;
