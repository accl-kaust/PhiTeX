//! TeX as a dataflow graph over content-versioned values: the minimal
//! end-to-end system (AGENTS_BRIEF.md).
//!
//! A document is cut into **pieces** (blank lines at brace depth 0). Each
//! piece is a pure function of the reaching definitions it reads: it
//! produces its own definitions and its paragraphs. A read is resolved
//! against the definitions earlier pieces exported; when an earlier piece
//! that could define the name has not run, the reader parks (or, under the
//! speculating policy, predicts that it does not, and is validated later).
//! Line breaking is a task per paragraph, memoised by content; the glue
//! between pieces is a join reading only the neighbours' dimensions; the
//! page builder is a scan reading dimensions; pages become DVI by a pure
//! function, memoised by content. N equal threads run whatever the
//! scheduler picks. A cold build is an edit from nothing.
//!
//! Depends on no `partex-*` crate (`tests/wall.rs`).

#![allow(
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools
)]

pub mod arith;
pub mod build;
pub mod dvi;
pub mod fuzz;
pub mod hash;
pub mod item;
pub mod linebreak;
pub mod page;
pub mod graph;
pub mod hyph;
pub mod logs;
pub mod mlist;
pub mod pack;
pub mod prim;
pub mod print;
pub mod session;
pub mod show;
pub mod split;
pub mod tfm;
pub mod val;

pub use build::{Build, Options, Order, Output, Reads, Stats};
pub use tfm::Font;
