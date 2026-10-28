//! `phitex`: TeX documents built and watched, TeX as graph reduction of
//! pending thunks.
//!
//! - Bare `phitex` is the modern command line (`modern.rs`): `phitex
//!   build`, `watch`, `clean`.
//! - Under an engine's name (`pdflatex`, `latex`, `pdftex`, `etex`, `tex`)
//!   or with `--compat=NAME`, it is that engine (`compat.rs`): web2c's
//!   command line, one run.
//! - As `bibtex` or `makeindex` (or `phitex -bibtex`, `-makeindex`), those
//!   programs, in process (`tools.rs`).
//!
//! Every build is a document session (`session.rs`); a watch's edits are
//! updates of the same session, on `phitex-tex`.

#![allow(
    clippy::many_single_char_names,
    clippy::doc_markdown,
    clippy::struct_excessive_bools,
    clippy::format_push_string,
    clippy::type_complexity,
    clippy::too_many_lines,
    clippy::bool_to_int_with_if,
    clippy::match_same_arms,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::needless_pass_by_value
)]

mod cache;
mod compare;
mod compat;
mod config;
mod dvipdf;
mod editor;
mod failure;
mod keys;
mod modern;
mod problems;
mod session;
mod tex;
mod tools;
mod ui;
mod view;
mod warnings;
mod watch;
mod ws;
mod xefonts;

#[cfg(all(feature = "jemalloc", not(target_family = "wasm")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The exit status of any failure, as pdflatex's (web2c exits 0 or 1):
/// TeX's errors, a usage, configuration or file error, PhiTeX's own
/// failure. Which one it was is on standard error.
pub const EXIT_FAILED: i32 = 1;

/// When the process reached `main` (the cold build's timeline begins
/// here, `PHITEX_TEX_TIME`).
pub static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    START.get_or_init(std::time::Instant::now);
    let mut raw = std::env::args();
    let argv0 = raw.next().unwrap_or_default();
    let args: Vec<String> = raw.collect();
    if tools::bibtex_wanted(&argv0, &args) {
        tools::bibtex_main(&args);
    }
    if tools::makeindex_wanted(&argv0, &args) {
        tools::makeindex_main(&args);
    }
    let env = std::env::var("PHITEX_COMPAT").ok();
    match compat::select(&argv0, args, env.as_deref()) {
        compat::Selection::Compat { progname, args } => compat::main(&progname, &args),
        compat::Selection::Modern(args) => modern::main(&args),
    }
}
