//! A live document, as `phitex watch` keeps it and the frontends' wasm
//! core runs it: the same code in both.
//!
//! - [`session`]: the document's session, one [`phitex_tex::Build`], its
//!   first build cold and every later one an edit, its records kept and
//!   its outputs written through the host; [`synctex`], its
//!   `JOB.synctex.gz` from the pages' glyph origins.
//! - [`host`]: what the host injects: the files ([`host::Host`]: the CLI's
//!   kpathsea, the frontends' Shelf), the bytes kept between sessions
//!   ([`host::Store`]), where the outputs go ([`host::Sink`]); and, on the
//!   Build's own options, the shell hook. Nothing here reads a file,
//!   starts a process or watches the disk.
//! - [`dvipdf`]: a DVI or XDV made a PDF for the viewer (dvipdfmx in
//!   process), its files through the host's finder.
//! - [`view`]: the viewer's protocol (its requests answered, its events,
//!   the state it is shown), without its transport.
//! - [`watch`]: the editor's buffers given to the session, a build's
//!   diagnostics, PDF and cursor for the viewers.
//! - [`xefonts`]: XeTeX's fonts for a host without fontconfig or
//!   kpathsea, from its files and a font index it ships.
//! - [`document`]: the three driven by a host that calls in (the wasm
//!   core): requests, the editor's changes, a step that builds.
//! - [`lsp`]: the language server every editor speaks, without its
//!   transport (docs/spec-editor-protocol.md); [`progress`], a build's
//!   progress records, which the viewer's `progress` event and the
//!   server's `$/progress` and `phitex/status` are made from.
//! - [`tex`], [`problems`], [`failure`], [`warnings`], [`panics`]: what the
//!   session says of a build (the engines, its problems, its failures and
//!   warnings for a reader, a panic caught).

#![allow(
    clippy::many_single_char_names,
    clippy::bool_to_int_with_if,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::should_implement_trait,
    clippy::return_self_not_must_use,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::format_push_string,
    clippy::type_complexity,
    clippy::struct_excessive_bools
)]

pub mod document;
pub mod dvipdf;
pub mod failure;
pub mod host;
pub mod lsp;
pub mod panics;
pub mod problems;
pub mod progress;
pub mod session;
pub mod synctex;
pub mod tex;
pub mod view;
pub mod warnings;
pub mod watch;
pub mod xefonts;
