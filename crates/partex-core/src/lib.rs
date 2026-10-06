//! The partex interpreter core.
//!
//! `no_std` + `alloc` on purpose: the core has no ambient access to files,
//! environment, clock or processes. Every effect goes through [`Host`], so the
//! same code runs natively and on `wasm32-unknown-unknown` with identical output.
//!
//! Modules named after tex.web parts are faithful ports; `§N` in comments is
//! tex.web section N (web2c's additions are cited as merged-source sections,
//! see `scripts/merge-web.sh`).

#![no_std]
// While the port is incomplete many routines have no caller yet.
// Remove once main_control (part 45) is ported.
#![allow(dead_code)]
// Ported routines keep tex.web's variable names (`j`, `k`, `u`, `v`,
// `cv_backup`/`cvl_backup`, …).
#![allow(clippy::many_single_char_names, clippy::similar_names)]
// Numeric constants are written as in tex.web (`@'7777777777` = `0o7777777777`),
// and the engine struct holds tex.web's boolean globals.
#![allow(clippy::unreadable_literal, clippy::struct_excessive_bools)]
// Keep tex.web's control structure recognizable: long procedures
// (`main_control`, `line_break`), `if` chains and explicit range tests stay
// as written in WEB; eqtb constants are used wholesale.
#![allow(
    clippy::too_many_lines,
    clippy::comparison_chain,
    clippy::manual_range_contains,
    clippy::wildcard_imports
)]

extern crate alloc;

pub use partex_engine::{dviout, pageir, persist};

pub mod diag;
pub mod displist;
pub mod effects;
pub mod exec;
mod flat;
pub mod host;
pub mod params;
pub mod progress;
mod skipcache;
pub mod ssa;
pub mod track;

mod adapter;
mod align;
mod arith;
mod build;
mod bulk;
mod charset;
mod cmds;
mod conds;
mod cow;
mod display;
mod dvi;
mod eqtb;
mod equiv;
mod error;
mod etex;
mod expand;
pub use expand::WATCHDOG;
mod expr;
mod files;
mod fontexp;
mod fontmap;
mod fonts;
mod format;
mod hash;
mod hashmemo;
mod hyph;
mod icu_tables;
mod input;
mod journal;
mod linebreak;
pub mod machine;
mod maincontrol;
mod mathcodes;
mod mathmode;
mod mem;
mod memo;
mod native;
mod nest;
mod nodes;
mod objs;
mod overlay;
mod pack;
mod page;
mod pdf;
mod pdfconv;
mod prefixed;
mod prim;
mod prims;
mod print;
mod random;
mod reflect;
mod relaxed;
mod run;
mod sanitize;
pub use sanitize::{mask_statistics, statistics_line};
mod save;
mod save_state;
mod scalars;
mod scan;
mod scanner;
pub mod seal;
pub mod shell;
pub mod srcmap;
pub mod statehash;
mod streams;
mod strings;
pub mod synctex;
mod tex;
mod tfm;
mod tok;
mod tokens;
mod toklists;
mod u64map;
mod values;
mod web;
mod wide;
mod xetex;
mod xmain;
mod xpic;
mod xregs;

#[cfg(test)]
mod testing;

pub use charset::Translation;
pub use exec::{Executor, Sequential};
pub use flat::set_flat_live;
pub use host::{DateTime, FileKind, Host, OpenedFile, PageSink, Ran, WriteId};
pub use journal::{set_rebase, set_written_blocks};
pub use memo::MemoStats;
pub use overlay::{CharsDiffer, Names};
pub use params::{Flavor, Params};
pub use partex_engine::bugs::PdftexBugs;
pub use partex_engine::stablehash::StableHasher;
pub use pdf::out::{ObjStmDiffer, ObjStmWritten};
pub use run::Step;
pub use srcmap::{GlyphOrigin, StreamOrgs};
pub use statehash::{OutputFiles, StateHashMemo};
pub use tex::{Jump, Tex};
pub use tok::{set_arg_pool, set_changed_ids};
pub use track::{Cell, Tracker, Untracked};

/// The stable 128-bit hash partex keys its caches by.
pub fn persist_hash<T: core::hash::Hash + ?Sized>(v: &T) -> u128 {
    partex_engine::stablehash::StableHasher::of(v)
}
