//! The partex engine: TeX's typesetting on typed, owned data.
//!
//! This is the redesigned core (see `DESIGN.md`). It shares no data
//! layout with tex.web: nodes are an `enum` in owned lists, boxes are
//! immutable shared trees, fonts are parsed metrics. What it keeps from
//! TeX is the semantics, integer arithmetic included, so its results are
//! the same boxes, breaks and pages. `partex-core` drives it: it reads the
//! input, runs the commands and prints what TeX prints.
//!
//! `no_std` + `alloc`, like the reference: every effect goes through the
//! caller.

#![no_std]
// TeX's arithmetic is specified with octal constants (`0o200000` = 2^16)
// and short formula variables (`x`, `s`, `t`, `z` of §108, §571); keeping
// them makes the code checkable against the specification.
#![allow(clippy::unreadable_literal, clippy::many_single_char_names)]
// TeX's paired names (`hyf_char`/`hyf_bchar`, `cur_l`/`cur_r`) stay.
#![allow(clippy::similar_names)]
// Byte-exact algorithms (TFM parsing, DVI movements, list output) read
// best as the single procedures they are specified as.
#![allow(clippy::too_many_lines)]

extern crate alloc;

pub mod align;
pub mod bugs;
pub mod builder;
pub mod codec;
pub mod dviout;
pub mod expand;
pub mod fofi;
#[rustfmt::skip]
pub mod fofi_tables;
pub mod font;
pub mod gfxfont;
pub mod hyph;
pub mod inflate;
pub mod linebreak;
pub mod lr;
pub mod margin;
pub mod math;
pub mod md5;
pub mod native;
pub mod node;
pub mod nodelist;
pub mod origin;
pub mod pack;
pub mod page;
pub mod pageir;
pub mod pdfread;
pub mod pdftext;
pub mod persist;
pub mod png;
pub mod regex;
pub mod scaled;
pub mod stablehash;
pub mod web;
pub mod zlib;

pub use scaled::Scaled;
