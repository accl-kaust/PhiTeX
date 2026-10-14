//! TeX's semantics on the reduction core, shared by every engine.
//!
//! Tokens are a 21-bit character with its catcode, or a control sequence;
//! pdfTeX simply never makes a character above 255. Expansion, macros,
//! conditionals, registers, grouping, modes and the input stack are
//! reductions on `phitex-defuse`; the algorithms they need come from
//! `phitex-typeset`.
//!
//! What differs between engines is asked of an [`Engine`]: how input bytes
//! become characters, which primitives exist, how fonts load, what is
//! written. An engine adds behaviour through it, never by forking what is
//! here.
//!
//! `PRIMITIVES.md` beside this crate catalogues every `TeX82`, e-TeX and
//! pdfTeX primitive as a node spec on the core (reads, defines, scope,
//! claim, unfold, contribution, hazards) and lists what the core lacks.

// tex.web's names (p, q, r, s, t) and its octal and hex codes, kept
#![allow(clippy::many_single_char_names, clippy::unreadable_literal)]

/// What makes an engine that engine.
pub trait Engine: Send + Sync {
    /// Its program name (`pdftex`, `xetex`, `luatex`).
    fn name(&self) -> &'static str;
}

mod align;
mod api;
mod build;
mod control;
mod etex;
mod exec;
mod expand;
mod fields;
mod fonts;
mod getnext;
mod hints;
mod incr;
mod input;
mod inputs;
mod log;
mod math;
mod nest;
mod nodes;
mod ops;
mod page;
mod pageinline;
mod pdfout;
mod pdfprim;
mod pdfrelaxed;
mod planinc;
mod prims;
mod print;
mod run;
mod runstate;
mod scan;
mod val;

pub use api::{
    Build, Diagnostic, EditSender, Finder, Options, PageSettled, Severity, Speculation, Stats,
    guess_wkeys,
};
pub use exec::{Progress, SpawnWorker, set_spawn_worker};
pub use hints::set_hints_enabled;
pub use pdfrelaxed::pages_written as pdf_pages_written;
pub(crate) use phitex_defuse::FxHashMap;
