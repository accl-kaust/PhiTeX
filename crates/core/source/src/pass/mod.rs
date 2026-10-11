//! The syntactic pass: TeX's structure read from text alone, and an honest
//! account of where it is not knowable without evaluating anything.
//!
//! A graph engine binds every use to the definition document order gives
//! it, and nothing waits except on what its binding crosses. For that, the
//! structure (argument extents, group boundaries) must come from the text
//! wherever it can; it is *pending* only where its extent depends on a
//! value at run time. This crate draws that line. It never executes
//! anything: there is no main loop over the document, no state that
//! commands change, no save stack. It reads.
//!
//! For one text, under an assumed catcode regime ([`FileKind`]), [`analyze`]
//! gives:
//!
//! 1. its tokens with positions ([`Token`]), read by `phitex-source`'s
//!    tokenizer under the regime in force, which changes where a change is
//!    syntactically visible (`\makeatletter`, `\ExplSyntaxOn`, a
//!    `\catcode` with literal operands, `.sty`/`.cls`/`.dtx` files);
//! 2. every `{`…`}` pair ([`BracePair`]): group boundaries and undelimited
//!    argument extents;
//! 3. the group and environment pairs visible in the same group of text
//!    ([`GroupPair`]: `\begingroup`/`\endgroup`, `\bgroup`/`\egroup`,
//!    `\begin{x}`/`\end{x}`);
//! 4. for every control sequence ([`Item`]), whether the extent of what it
//!    consumes is [`Status::Known`] (a primitive or kernel command of fixed
//!    syntax), [`Status::Predicted`] (a definition whose parameter text is
//!    visible, an expl3 signature) or [`Status::Pending`] (it depends on a
//!    value: a macro defined elsewhere, `\expandafter`, `\csname`, a
//!    lookahead, a number that reads into a macro), with its [`Cause`];
//! 5. the catcode hazard spans ([`Hazard`]): where the tokenization itself
//!    is predicted (re-tokenized under the visible new regime) or pending.
//!
//! [`Stats`] sums a pass up for the corpus report, including the longest
//! chain of pending structure ([`Stats::chain`]): pending items each inside
//! the reach of the one before, which bounds the engine's critical path.

mod atoms;
mod classify;
mod defs;
mod lex;
mod report;
mod table;
#[cfg(test)]
mod tests;

use crate::syntax::Catcodes;
pub use atoms::{Atom, AtomKind};
pub use defs::{Arg, Definitions, Lit, Shape};
pub use lex::{Hazard, TokKind, Token};
pub use report::Stats;

/// How far a piece of structure is known from the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    /// Fixed by the text and TeX's or the kernel's documented syntax.
    Known,
    /// Read from a definition visible in the same text (or the kernel's),
    /// or from a regime change visible in it: right unless something
    /// redefines it on the way, which the engine checks by binding.
    Predicted,
    /// Depends on a value only evaluation gives.
    Pending,
}

/// The regime a text starts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A document (`.tex`): plain catcodes, `@` other.
    Document,
    /// A package, class, format or documented source (`.sty`, `.cls`,
    /// `.ltx`, `.dtx`, pgf's `.code.tex`): `@` a letter.
    Package,
}

impl FileKind {
    /// The kind a file's name says.
    #[must_use]
    pub fn from_path(path: &str) -> FileKind {
        let p = path.to_ascii_lowercase();
        if [
            ".sty",
            ".cls",
            ".ltx",
            ".dtx",
            ".code.tex",
            ".def",
            ".clo",
            ".fd",
        ]
        .iter()
        .any(|e| p.ends_with(e))
        {
            FileKind::Package
        } else {
            FileKind::Document
        }
    }

    #[must_use]
    pub(crate) fn catcodes(self) -> Catcodes {
        match self {
            FileKind::Document => Catcodes::PLAIN,
            FileKind::Package => Catcodes::at_letter(),
        }
    }
}

/// Why something is pending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    /// The kind of cause, for counting ("macro defined elsewhere").
    pub kind: &'static str,
    /// The name involved, if any.
    pub detail: String,
}

/// A `{`…`}` pair: token indices of its braces (`close` is `None` when the
/// text ends first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BracePair {
    pub open: usize,
    pub close: Option<usize>,
    pub status: Status,
}

/// What kind of group a [`GroupPair`] is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupKind {
    BeginGroup,
    BGroup,
    Env(String),
}

/// A group or environment boundary written as commands: the atoms of its
/// opening and closing commands. Unmatched in its own brace group, its
/// other end is pending (it is in another macro's body, or computed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupPair {
    pub kind: GroupKind,
    pub open: Option<usize>,
    pub close: Option<usize>,
    pub status: Status,
}

/// A control sequence and what is known of the extent it consumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its atom ([`Pass::atoms`]).
    pub atom: usize,
    pub name: String,
    pub status: Status,
    pub cause: Option<Cause>,
    /// Whether it consumes anything (or might: a pending one).
    pub consumes: bool,
    /// The atom after the last one it consumes (as far as known).
    pub end: usize,
    /// Whether it opens a pending catcode hazard.
    pub hazard: bool,
}

/// What the pass found in one text.
#[derive(Debug)]
pub struct Pass {
    pub tokens: Vec<Token>,
    pub hazards: Vec<Hazard>,
    pub atoms: Vec<Atom>,
    pub braces: Vec<BracePair>,
    /// `}` tokens with no `{` open.
    pub unmatched_closes: Vec<usize>,
    pub groups: Vec<GroupPair>,
    pub items: Vec<Item>,
}

/// The definitions a text's uses may bind to: those of its own document
/// (all its files, bound by document order within a file) and the
/// kernel's (the format's: the last definition of each name).
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    pub document: &'a Definitions,
    /// Which of the document's files the text is.
    pub file: u32,
    pub prelude: &'a Definitions,
}

/// The syntactic pass over `text`, file `scope.file` of a document whose
/// definitions are `scope.document`.
#[must_use]
pub fn analyze(text: &str, kind: FileKind, scope: Scope<'_>) -> Pass {
    let lexed = lex::lex(text, kind, Some(scope));
    let atoms = atoms::atoms(text, &lexed.tokens);
    let braces = lexed
        .braces
        .iter()
        .map(|&(open, close)| {
            let status = lexed.tokens[open]
                .tz
                .max(close.map_or(Status::Pending, |c| lexed.tokens[c].tz));
            BracePair {
                open,
                close,
                status,
            }
        })
        .collect();
    let groups = atoms::group_pairs(text, &atoms, &lexed.tokens);
    let items = classify::classify(text, &atoms, &lexed, scope);
    Pass {
        tokens: lexed.tokens,
        hazards: lexed.hazards,
        atoms,
        braces,
        unmatched_closes: lexed.unmatched_closes,
        groups,
        items,
    }
}
