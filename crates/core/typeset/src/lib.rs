//! Knuth's typesetting algorithms as pure functions: values in, values out.
//!
//! Scaled arithmetic and glue (tex.web §99–§110), hpack and vpack
//! (§644–§679), line breaking (§813–§890), hyphenation (§891–§965), math
//! layout (§699–§767), alignment (§768–§812), the page builder's decisions
//! (§980–§1028).
//!
//! Rule: no dependency on `phitex-reduce` or `phitex-tex`. A function here
//! is given every value it needs; whatever must be asked of the document
//! (a register, a macro) is asked by `phitex-tex`, which then calls in.
//! Shared by every engine; each function carries its oracle tests per
//! engine.

// tex.web's octal constants and one-letter variables are kept so that the
// code reads against the cited sections.
#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::similar_names
)]

pub mod arith;
pub mod fonts;
pub mod glue;
pub mod hyph;
pub mod linebreak;
pub mod node;
pub mod pack;

#[cfg(test)]
pub(crate) mod testfont;
