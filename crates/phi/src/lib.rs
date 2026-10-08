//! The φ core (DESIGN 7): a TeX-agnostic, incremental and parallel SSA
//! dataflow core.
//!
//! A client implements [`Lang`]: its values and its ops, pure functions
//! of their operands. The [`Graph`] evaluates them, builds itself by
//! evaluating steps, and after an edit runs again exactly what a changed
//! version reaches.

#![allow(clippy::missing_panics_doc, clippy::cast_possible_truncation)]

pub mod graph;
pub mod lang;
pub mod seq;
pub mod text;
pub mod value;
pub mod ver;

pub use graph::{Arg, Args, Config, END, Graph, Kind, Local, NameId, NodeId, Report, StepCx};
pub use lang::{Chain, Class, Entry, Fam, Lang, Slot, Step};
pub use seq::{ElemId, Hunk, Measure, Seq};
pub use value::{Proj, Sel, Value};
pub use text::{Dump, Line};
pub use ver::Ver;
