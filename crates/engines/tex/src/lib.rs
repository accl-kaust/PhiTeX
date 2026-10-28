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
mod assemble;
mod build;
mod collect;
mod control;
mod defer;
mod etex;
mod exec;
mod expand;
mod fields;
mod fmtenv;
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
mod origins;
mod ops;
mod pages;
mod parbreak;
mod pdfout;
mod pdfprim;
mod pkgrec;
mod prims;
mod print;
mod prof;
mod run;
mod scan;
mod shell;
mod state;
pub mod timeline;
mod tools;
mod val;
mod vprof;
mod xetex;
mod native;
mod xpic;

pub use api::{
    Build, ContentKey, Diagnostic, EditSender, Finder, Hint, Options, PageSettled, RunReport as Report, Severity,
    Speculation, Stats, ToolFinder, ToolSearch, byte_ends, engine_id, file_stat_key,
};
pub use shell::{ShellHook, ShellRan, TEXLIVE_COMMANDS, texlive_commands};
pub use exec::{Progress, SpawnWorker, set_spawn_worker};
pub use phitex_otf::{FontSource, KpseFormat};
pub use phitex_otf::index::FontIndex;
pub use xetex::{FontFiles, XeTeX};
pub use origins::{OriginsSnapshot, PageOrigins};

/// Whether this build has the timing and counter lines (`profile`).
pub const PROFILE: bool = cfg!(feature = "profile");
/// Whether this build has the traces, dumps and checks (`observe`).
pub const OBSERVE: bool = cfg!(feature = "observe");

/// Whether the timing lines are asked for: a `profile` build with
/// `PHITEX_TEX_TIME` set (read once).
#[must_use]
pub fn tex_time() -> bool {
    exec::prof_flag!("PHITEX_TEX_TIME")
}
pub use hints::{paragraph_units, set_hints_enabled};
pub(crate) use phitex_defuse::FxHashMap;
pub use pkgrec::summary as pkgrec_summary;
