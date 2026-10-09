//! TeX as a suspending dataflow graph over content-versioned values.
//!
//! The design is `AGENTS_BRIEF.md` at the repository root; read it first.
//! A document is a program: readers turn source into nodes, each command a
//! pure function of the field versions it reads ([`contract::Cx`]); the
//! graph owns every value; N equal threads run whatever a scheduler picks.
//! Nothing here depends on any `partex-*` crate (`tests/wall.rs`).

#![allow(
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::doc_markdown
)]

pub mod contract;

/// Agent A: `Cx` backed by the graph: values, fields, versions, reaching
/// definitions with scopes, park and wake, validation, edits.
pub mod graph;
/// Agent B: definitions: macros and expansion, `\let`, codes, parameters,
/// registers, `\the`, `\csname`, conditionals with φ.
pub mod defs;
/// Agent C: material: characters, fonts, lists, boxes, glue, the
/// paragraph builder, math.
pub mod material;
/// Agent D: readers (characters into tokens), entries, threads, parking,
/// the scheduler.
pub mod reader;
/// Agent E: the page builder's scan, output routines, shipout, writes.
pub mod layout;
