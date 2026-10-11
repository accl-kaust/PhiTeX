//! TeX's semantics on the reduction core, shared by every engine.
//!
//! Tokens are a 21-bit character with its catcode, or a control sequence;
//! pdfTeX simply never makes a character above 255. Expansion, macros,
//! conditionals, registers, grouping, modes and the input stack are
//! reductions on `phitex-reduce`; the algorithms they need come from
//! `phitex-typeset`.
//!
//! What differs between engines is asked of an [`Engine`]: how input bytes
//! become characters, which primitives exist, how fonts load, what is
//! written. An engine adds behaviour through it, never by forking what is
//! here.

/// What makes an engine that engine.
pub trait Engine: Send + Sync {
    /// Its program name (`pdftex`, `xetex`, `luatex`).
    fn name(&self) -> &'static str;
}
