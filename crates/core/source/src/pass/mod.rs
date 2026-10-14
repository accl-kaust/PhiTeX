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
mod hints;
mod lex;
mod report;
mod table;
#[cfg(test)]
mod tests;

use crate::syntax::Catcodes;
pub use atoms::{Atom, AtomKind};
pub use defs::{Arg, Definitions, Lit, OUTPUT, Shape};
pub use hints::{
    Assumption, Claim, Def, DefSet, Effect, Hints, Implicit, Mark, MayDefine, MeaningShape,
    ScopeHint, ScopeKind,
};
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
    /// A text read under INITEX's catcodes (a format's source, a document
    /// run without a format): only `\\`, `%`, letters and space have
    /// their usual catcodes until its own `\catcode` lines (§232).
    Initex,
}

impl FileKind {
    /// The kind a file's name says.
    #[must_use]
    pub fn from_path(path: &str) -> FileKind {
        let p = path.to_ascii_lowercase();
        if std::path::Path::new(&p)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ini"))
        {
            return FileKind::Initex;
        }
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
            FileKind::Initex => Catcodes::initex(),
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
    /// What it tells the engine.
    pub hints: Hints,
}

/// A catcode regime as far as the pass reads it: the catcode of each
/// ASCII byte under plain TeX's table with the regime's changes (`@`,
/// `_`, `:` letters; spaces ignored and `~` a space under expl3; the
/// shaping characters the text changed; INITEX's others), or none where
/// a `\catcode` of the text set a value the pass does not model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Regime {
    cats: [Option<u8>; 128],
    /// Spaces are ignored (expl3's).
    pub ignore_spaces: bool,
}

impl Regime {
    pub(crate) fn of(c: &Catcodes) -> Regime {
        let mut cats = [Some(12u8); 128];
        for (b, cat) in cats.iter_mut().enumerate() {
            let b = u8::try_from(b).unwrap_or(0);
            *cat = Some(match b {
                _ if c.is_unknown(b) => {
                    *cat = None;
                    continue;
                }
                b'$' | b'&' | b'^' | b'_' | b'~' | b'\t' if c.is_special12(b) => 12,
                b'\\' | b'{' | b'}' | b'#' | b'%' if c.is_other(b) => 12,
                b'\\' => 0,
                b'{' => 1,
                b'}' => 2,
                b'$' => 3,
                b'&' => 4,
                13 => 5,
                b'#' => 6,
                b'^' => 7,
                b'_' if !c.is_letter(b) => 8,
                0 => 9,
                b' ' | b'\t' if c.ignore_spaces => 9,
                b' ' | b'\t' => 10,
                b'~' if c.ignore_spaces => 10,
                b'~' => 13,
                b'%' => 14,
                127 => 15,
                _ if c.is_letter(b) => 11,
                _ if c.is_escape(b) => 0,
                _ if c.is_open(b) => 1,
                _ if c.is_close(b) => 2,
                _ => 12,
            });
        }
        Regime {
            cats,
            ignore_spaces: c.ignore_spaces,
        }
    }

    /// The catcode of ASCII byte `b` (`None`: not known to the regime; a
    /// byte above 127: 12).
    #[must_use]
    pub fn catcode(&self, b: u8) -> Option<u8> {
        self.cats.get(usize::from(b)).copied().unwrap_or(Some(12))
    }
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
    /// The hints of structure that is not a control sequence (`{`, `}`,
    /// `$`, `$$`, a blank line, a run of characters), outside definition
    /// bodies.
    pub marks: Vec<Mark>,
    /// The regime each token was read under, as change points: from
    /// token index `i` on.
    pub regimes: Vec<(usize, Regime)>,
}

impl Pass {
    /// The regime token `tok` was read under.
    #[must_use]
    pub fn regime_at(&self, tok: usize) -> &Regime {
        let k = self.regimes.partition_point(|r| r.0 <= tok);
        &self.regimes[k.saturating_sub(1).min(self.regimes.len() - 1)].1
    }

    /// Every item's and mark's hints, one JSON object per line in text
    /// order: `{"atom", "start", "end", "name", "hints"}` (`name` is the
    /// control sequence's name, or a mark's text).
    #[must_use]
    pub fn hints_jsonl(&self, text: &str) -> String {
        let mut o = String::new();
        let (mut i, mut m) = (0, 0);
        while i < self.items.len() || m < self.marks.len() {
            let take_item = m >= self.marks.len()
                || i < self.items.len() && self.items[i].atom < self.marks[m].atom;
            if take_item {
                let it = &self.items[i];
                hints::line_json(
                    &mut o,
                    it.atom,
                    &self.atoms[it.atom],
                    text,
                    Some(&it.name),
                    &it.hints,
                );
                i += 1;
            } else {
                let mk = &self.marks[m];
                hints::line_json(&mut o, mk.atom, &self.atoms[mk.atom], text, None, &mk.hints);
                m += 1;
            }
        }
        o
    }
}

/// The improvements the pass may use (all on by default; off, for the
/// corpus report's before/after).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// Extents of expl3's `:w` functions from l3kernel's conventions.
    pub w_signatures: bool,
    /// One step of `\expandafter` look-through.
    pub lookthrough: bool,
}

impl Default for Features {
    fn default() -> Features {
        Features {
            w_signatures: true,
            lookthrough: true,
        }
    }
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
    analyze_with(text, kind, scope, Features::default())
}

/// [`analyze`] with only `features`.
#[must_use]
pub fn analyze_with(text: &str, kind: FileKind, scope: Scope<'_>, features: Features) -> Pass {
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
    let items = classify::classify(text, &atoms, &lexed, scope, kind, features);
    let marks = hints::marks(text, &atoms, &lexed.tokens, &items);
    let regimes = lexed
        .regimes
        .iter()
        .map(|(i, c)| (*i, Regime::of(c)))
        .collect();
    Pass {
        tokens: lexed.tokens,
        hazards: lexed.hazards,
        atoms,
        braces,
        unmatched_closes: lexed.unmatched_closes,
        groups,
        items,
        marks,
        regimes,
    }
}
