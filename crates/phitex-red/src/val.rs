//! Names (`Key`) and values (`Val`). A value is immutable.
use std::sync::Arc;

use crate::item::BoxNode;
use crate::prim::PrimId;
use crate::tok::{Cs, Tok, Toks};

pub type FontId = u32;
/// A file's name as the job asked for it (after kpse lookup: the found name).
pub type FileName = Arc<str>;

/// Everything a document can define or the build can answer.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Key {
    Cs(Cs),
    Active(char),
    Cat(char),
    Lc(char),
    Uc(char),
    Sf(char),
    MathCode(char),
    DelCode(char),
    /// Parameters by index (`prim::ip`, `prim::dp`, `prim::gp`, `prim::mp`, `prim::tp`).
    IntPar(u16),
    DimenPar(u16),
    GluePar(u16),
    MuGluePar(u16),
    TokPar(u16),
    Count(u16),
    Dimen(u16),
    Skip(u16),
    Muskip(u16),
    Toks(u16),
    Box(u16),
    Parshape,
    /// e-TeX `\interlinepenalties` 0, `\clubpenalties` 1, `\widowpenalties` 2, `\displaywidowpenalties` 3.
    PenShape(u8),
    CurFont,
    /// (size 0 text, 1 script, 2 scriptscript; family)
    FamFont(u8, u8),
    FontDimen(FontId, u16),
    HyphenChar(FontId),
    SkewChar(FontId),
    /// pdfTeX per-font per-char codes: (which: lp rp ef kn…, font).
    FontCode(u8, FontId),
    Patterns(u8),
    Exceptions(u8),
    Interaction,
    /// Global: a font file loaded under this name/size → its id.
    FontLoaded(Arc<str>, i32),
    /// An input file's bytes (`Val::Bytes`/`Val::Void`). Files the job writes
    /// are answered by the fixpoint driver (loop-back), never from disk.
    File(FileName),
    ReadStream(u8),
    WriteOpen(u8),
    /// The open conditionals (`Val::Conds`).
    Conds,
    /// `\afterassignment`'s token (`Val::Toks`, empty: none).
    AfterAssignment,
    /// List-scope values (live in the innermost list scope).
    SpaceFactor,
    PrevDepth,
    PrevGraf,
    Clang,
    /// Page builder quantities 0..=7 `\pagegoal`…`\pagedepth`, 8 `\deadcycles`,
    /// 9 `\insertpenalties`: answered by the page fold.
    Page(u8),
    /// (class, 0 top 1 first 2 bot 3 splitfirst 4 splitbot)
    Mark(u16, u8),
    /// The tail of the current list (answered from emits, not env).
    Tail,
    /// pdfTeX's read-backs (`\pdflastobj` …) and state.
    PdfLast(u8),
    PdfRandom,
    /// Build givens: job name, seed, clock, creation date.
    JobName,
    Given(u8),
    /// `\inputlineno` of the reading node (answered by the graph from spans).
    LineNo,
    /// The job has ended (`\end`/`\dump`).
    Ended,
    /// Any other global state a module needs: (module tag, number). Tags are
    /// assigned in `prim::tag`.
    Other(u8, u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Glue {
    pub width: i32,
    pub stretch: i32,
    pub shrink: i32,
    pub stretch_order: u8,
    pub shrink_order: u8,
}

/// A macro: a function value.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Macro {
    /// Parameter text: `Tok::Param(n)` marks parameters, other tokens delimit.
    /// A trailing `{` (the `#{` form) is in `brace_end`.
    pub params: Toks,
    pub body: Toks,
    pub brace_end: bool,
    pub long: bool,
    pub outer: bool,
    pub protected: bool,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Meaning {
    #[default]
    Undefined,
    Prim(PrimId),
    Macro(Arc<Macro>),
    /// `\let\x=a`
    Char { cat: u8, ch: char },
    CharGiven(u32),
    MathCharGiven(u32),
    /// `\countdef` etc.: (bank 0 count 1 dimen 2 skip 3 muskip 4 toks, n)
    Reg(u8, u16),
    Font(FontId),
}

/// An open conditional.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cond {
    /// The `\if` primitive's code (`\currentiftype`).
    pub kind: u16,
    /// 0 in the test / `\if` part, 1 then-arm taken (`\else` skips), 2 else-arm, 3 `\ifcase` arm.
    pub state: u8,
    pub line: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Val {
    Int(i32),
    Glue(Glue),
    Toks(Toks),
    Meaning(Meaning),
    Box(Option<Arc<BoxNode>>),
    Font(FontId),
    Parshape(Arc<[(i32, i32)]>),
    Ints(Arc<[i32]>),
    Name(Arc<str>),
    Bytes(Arc<[u8]>),
    Void,
    Conds(Arc<[Cond]>),
    Patterns(crate::hyph::Patterns),
    Exceptions(crate::hyph::Exceptions),
    Stream(Arc<crate::io::Stream>),
    /// Anything module-private (compared by `Datum::same`).
    Data(Arc<dyn Datum>),
}

impl Eq for Val {}

pub trait Datum: std::fmt::Debug + Send + Sync + std::any::Any {
    fn same(&self, other: &dyn Datum) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

impl PartialEq for dyn Datum {
    fn eq(&self, o: &Self) -> bool {
        self.same(o)
    }
}

impl Val {
    pub fn int(&self) -> i32 {
        match self {
            Val::Int(n) => *n,
            _ => 0,
        }
    }
    pub fn toks(&self) -> Toks {
        match self {
            Val::Toks(t) => t.clone(),
            _ => Arc::from(Vec::<Tok>::new()),
        }
    }
    pub fn glue(&self) -> Glue {
        match self {
            Val::Glue(g) => *g,
            _ => Glue::default(),
        }
    }
}
