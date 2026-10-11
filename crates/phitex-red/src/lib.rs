//! TeX as graph reduction of pending thunks. See `CONSTRUCTS.md`.
#![allow(clippy::pedantic)]

// contract
pub mod tok;
pub mod val;
pub mod item;
pub mod term;
pub mod env;
pub mod ctx;
pub mod prim;
pub mod pool;
// core
pub mod graph;
pub mod memo;
pub mod sched;
pub mod edit;
pub mod fixpoint;
// expansion
pub mod tokenize;
pub mod input;
pub mod macros;
pub mod scan;
pub mod expand;
pub mod cond;
pub mod assign;
pub mod groups;
pub mod arith;
// typesetting
pub mod list;
pub mod pack;
pub mod boxes;
pub mod linebreak;
pub mod hyph;
pub mod math;
pub mod align;
pub mod font;
// pages
pub mod page;
pub mod output;
pub mod io;
pub mod pdfprim;
pub mod derive;
// back end
pub mod pdf;
pub mod pdffont;
pub mod image;
pub mod dvi;
pub mod diag;
pub mod api;
