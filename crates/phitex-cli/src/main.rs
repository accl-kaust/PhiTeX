//! `phitex`: TeX documents built and watched on phitex-flow, the dataflow
//! graph over content-versioned values (AGENTS_BRIEF.md, CORE.md).
//!
//! - Bare `phitex` is the modern command line (`modern.rs`): `phitex
//!   build`, `watch`, `check`, `clean`, `sync`, `outline`.
//! - Under an engine's name (`pdflatex`, `latex`, `pdftex`, `etex`, `tex`)
//!   or with `--compat=NAME`, it is that engine (`compat.rs`): web2c's
//!   command line, one run.
//! - As `bibtex` or `makeindex` (or `phitex -bibtex`, `-makeindex`), those
//!   programs, in process (`tools.rs`).
//!
//! Every build is a session of one `phitex_flow::Build` (`session.rs`):
//! the format (made on the graph, cached: `format.rs`), then the document
//! on it; a watch's edits are updates of the same Build.

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

mod compat;
mod config;
mod dvipdf;
mod editor;
#[cfg(not(target_family = "wasm"))]
mod external;
mod format;
mod json;
mod modern;
mod outline;
mod session;
mod synctex;
mod tex;
mod tools;
mod view;
mod warnings;
mod watch;
mod ws;

#[cfg(all(feature = "jemalloc", not(target_family = "wasm")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The threads a build runs on by default: all the cores.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

fn main() {
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
