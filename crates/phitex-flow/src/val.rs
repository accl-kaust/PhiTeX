//! Names, values and versions.
//!
//! A [`Key`] is anything a document can define. Its meaning at a point is
//! its reaching definition: a [`Val`]. Values are immutable; whether two
//! are the same is identity first (the same `Arc`), then equality.

use std::fmt;
use std::sync::Arc;

pub use crate::prim::{Al, Code, Conv, DimenPar, GluePar, IfKind, IntPar, Mat, MathP, Prim, Reg, Shorthand, TokPar};
use crate::build::CsTable;
use crate::tfm::Font;

/// A token.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Tok {
    /// A character with its category.
    Char(u8, char),
    /// A control sequence, by its name's number (`build::cs_id`).
    Cs(u32),
    /// An active character.
    Active(char),
    /// `#n` in a macro's parameter text or body. In a parameter text, the
    /// parameter character it was written with (TeX's `match` token keeps
    /// it, tex.web 289); in a body always `#` (an `out_param` token has
    /// none: it shows as the last one its macro's display showed, 294).
    Param(u8, char),
    /// `\noexpand`'s mark: the token after it is not expanded, once
    /// (tex.web 358). Only ever in rewritten input, never in a value.
    DontExpand,
}

impl Tok {
    pub fn cat(&self) -> Option<u8> {
        if let Tok::Char(c, _) = self { Some(*c) } else { None }
    }
    pub fn is_char(&self, cat: u8, ch: char) -> bool {
        matches!(self, Tok::Char(c, x) if *c == cat && *x == ch)
    }
    /// The key a control sequence or active character names.
    pub fn key(&self) -> Option<Key> {
        match self {
            Tok::Cs(n) => Some(Key::Cs(*n)),
            Tok::Active(c) => Some(Key::Active(*c)),
            _ => None,
        }
    }
}


/// Anything a document can define: every quantity TeX keeps in its
/// tables is a name with a reaching definition.
#[derive(Clone, Debug, PartialOrd, Ord)]
pub enum Key {
    /// A control sequence, by its name's number.
    Cs(u32),
    Active(char),
    /// A character's category code.
    Cat(char),
    Lccode(char),
    Uccode(char),
    Sfcode(char),
    Mathcode(char),
    Delcode(char),
    IntPar(IntPar),
    DimenPar(DimenPar),
    GluePar(GluePar),
    TokPar(TokPar),
    Count(u16),
    Dimen(u16),
    Skip(u16),
    Muskip(u16),
    Toks(u16),
    Box(u16),
    Parshape,
    /// The current font.
    Font,
    /// `\fontdimen n` of a font (`Font::key`), global.
    FontDimen(Arc<str>, u32),
    /// `\hyphenchar`, `\skewchar` of a font (global).
    HyphenChar(Arc<str>),
    SkewChar(Arc<str>),
    /// The name a font was last defined by (`font_id_text`, global).
    FontIdent(Arc<str>),
    /// Whether a font is loaded (global; read by `\font`, which loads it
    /// once: its parameters are defined then, never again).
    FontLoaded(Arc<str>),
    /// `\textfont` (size 0), `\scriptfont` (1), `\scriptscriptfont` (2) of a
    /// family.
    FamFont(u8, u8),
    /// The bytes of the input file of this exact name (`Val::Bytes`, or
    /// `Val::Void` if there is none): answered by the build's `Env`, never
    /// defined. Reading a file is a read of it.
    File(Arc<str>),
    /// `\openin` stream `n` (0–15; 16 is the terminal): `Val::Stream` while
    /// open, `Val::Void` when closed. Always defined globally (TeX's
    /// `read_open` has no save stack); each `\read` defines it anew,
    /// advanced by its lines.
    ReadStream(u8),
    /// The job's name (`\jobname`): answered by the `Env`.
    Job,
    /// The number of lines of its file before the reading piece's first
    /// line (`\inputlineno` adds the line within the piece): answered by
    /// the `Env` per piece, never defined, so a changed line count above
    /// re-runs only the nodes that read it.
    LineBase,
    /// The split marks of every class (`\splitfirstmark`, `\splitbotmark`,
    /// e-TeX's `\splitfirstmarks n`, `\splitbotmarks n`): one value,
    /// `Val::SplitMarks`, defined globally by each `\vsplit` (which clears
    /// every class's, etex.web's `vsplit_init`).
    SplitMarks,
    /// The interaction mode (`\batchmode` 0 … `\errorstopmode` 3; e-TeX's
    /// `\interactionmode`), global.
    Interaction,
    /// Whether the format is e-TeX's extended mode (1) or TeX's (0): a
    /// value of the format no document defines (`Options::etex`).
    ETeXMode,
    /// A language's hyphenation patterns (`\patterns`, global).
    Patterns(u8),
    /// A language's hyphenation exceptions (`\hyphenation`, global).
    Exceptions(u8),
    /// e-TeX's `\interlinepenalties` (0), `\clubpenalties` (1),
    /// `\widowpenalties` (2), `\displaywidowpenalties` (3):
    /// `Val::Penalties`.
    PenaltiesArray(u8),
    /// The page builder's quantities a document reads and assigns
    /// (`graph::output::PAGE_GOAL` …): `\pagegoal` … `\pagedepth`
    /// (0–7, `page_so_far`), `\deadcycles` (8), `\insertpenalties` (9).
    /// Assigned directly (at the level of the definition
    /// replaced); an output routine's run defines them at its start.
    Page(u8),
    /// `\topmark` (0), `\firstmark` (1), `\botmark` (2) of a mark class
    /// (e-TeX's `\marks`; TeX's are class 0): global, defined where the
    /// page builder fires (an output routine's run). The split marks are
    /// `Key::SplitMarks`.
    Mark(u8, u16),
    /// The format's identification (`format_ident`, tex.web 1328:
    /// ` (preloaded format=JOB Y.M.D)`), defined globally by `\dump`: what
    /// the next job of a session names in its banner.
    Format,
    /// The magnification fixed by its first use (`mag_set`, tex.web 286):
    /// 0 until a `true` dimension or the first page; defined globally.
    MagSet,
    /// How many `\fontdimen`s a font has (`font_params`, global): 0 as
    /// loaded (`Font::nparams`); more once the most recently loaded font's
    /// were extended (tex.web 579).
    FontParams(Arc<str>),
    /// The most recently loaded font (`font_ptr`'s, global: `Val::Name` of
    /// its key; none before any `\font` loads one).
    LastFont,
    /// The job has ended (`\end`, `\dump`: 1), global: every piece's
    /// first node reads it, so the pieces after the end read nothing more.
    Ended,
    /// The main vertical list's last item (tex.web 424: the contributions'
    /// tail, or the page builder's `last_glue`, `last_penalty`,
    /// `last_kern` once it took them): `Val::Tail`, defined globally by
    /// each node contributing to the list; `\lastskip` and the like read
    /// it where their piece has contributed nothing yet.
    MainTail,
    /// The main vertical list's `\prevdepth` (`Val::Int`), or the
    /// paragraph whose last line's depth it is, its lines not broken yet
    /// (`Val::Para`): defined globally by each node at the outer level
    /// that sets it; read where the reader state says `page::MAIN_DEPTH`.
    MainDepth,
    /// pdfTeX's per-character codes of a font (global, as pdfTeX keeps them
    /// beside the font): `\lpcode` (2), `\rpcode` (3), `\efcode` (4),
    /// `\knbscode` (7), `\stbscode` (8), `\shbscode` (9), `\knbccode`
    /// (10), `\knaccode` (11) — `Mat::FontInt`'s codes, pdftex.web's
    /// `*_code_base` — each the font's table (`Val::Penalties`, 256 values
    /// or empty: every code its default, `micro::code`); `\pdfnoligatures`
    /// (6): `Val::Int(1)` once given.
    FontCode(u8, Arc<str>),
    /// `\pdffontexpand`'s parameters of a font (`micro::Expand`'s
    /// `Val::Penalties`; empty: not expanded), global.
    FontExpand(Arc<str>),
    /// `\pdffontattr`'s text of a font (`Val::Name`), global.
    FontAttr(Arc<str>),
    /// How many fonts `\pdfcopyfont` and `\letterspacefont` made from a
    /// font so far (`Val::Int`), global: the next one's number.
    FontCopies(Arc<str>),
}

impl Key {
    /// The register `n` of a bank.
    pub fn reg(r: Reg, n: u16) -> Key {
        match r {
            Reg::Count => Key::Count(n),
            Reg::Dimen => Key::Dimen(n),
            Reg::Skip => Key::Skip(n),
            Reg::Muskip => Key::Muskip(n),
            Reg::Toks => Key::Toks(n),
        }
    }
}

impl std::hash::Hash for Key {
    /// A name hashes as its spelling, any other key as one word (every
    /// read hashes its key: the common cases must be cheap and inlined).
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        match self {
            Key::Cat(x) => h.write_u64((2 << 40) | u64::from(u32::from(*x))),
            Key::FontDimen(n, k) => {
                n.hash(h);
                h.write_u64((20 << 40) | u64::from(*k));
            }
            Key::FontCode(k, n) => {
                n.hash(h);
                h.write_u64((66 << 40) | u64::from(*k));
            }
            Key::HyphenChar(n)
            | Key::SkewChar(n)
            | Key::FontIdent(n)
            | Key::FontLoaded(n)
            | Key::File(n)
            | Key::FontParams(n)
            | Key::FontExpand(n)
            | Key::FontAttr(n)
            | Key::FontCopies(n) => {
                n.hash(h);
                h.write_u64(self.word());
            }
            _ => h.write_u64(self.word()),
        }
    }
}

impl PartialEq for Key {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Key::HyphenChar(a), Key::HyphenChar(b))
            | (Key::SkewChar(a), Key::SkewChar(b))
            | (Key::FontIdent(a), Key::FontIdent(b))
            | (Key::FontLoaded(a), Key::FontLoaded(b))
            | (Key::File(a), Key::File(b))
            | (Key::FontParams(a), Key::FontParams(b))
            | (Key::FontExpand(a), Key::FontExpand(b))
            | (Key::FontAttr(a), Key::FontAttr(b))
            | (Key::FontCopies(a), Key::FontCopies(b)) => Arc::ptr_eq(a, b) || a == b,
            (Key::FontCode(k, a), Key::FontCode(l, b)) => k == l && (Arc::ptr_eq(a, b) || a == b),
            (Key::Cat(a), Key::Cat(b)) => a == b,
            (Key::FontDimen(a, k), Key::FontDimen(b, l)) => k == l && (Arc::ptr_eq(a, b) || a == b),
            (x, y) if x.word() == u64::MAX || y.word() == u64::MAX => false,
            _ => self.word() == other.word(),
        }
    }
}

impl Eq for Key {}

impl Key {
    /// Every key but a name as one word: its kind, and its character,
    /// parameter or number.
    #[inline]
    fn word(&self) -> u64 {
        let w = |tag: u64, x: u64| (tag << 40) | x;
        let c = |x: &char| u64::from(u32::from(*x));
        match self {
            Key::FontDimen(..) | Key::FontCode(..) => u64::MAX,
            Key::FontExpand(_) => w(67, 0),
            Key::FontAttr(_) => w(68, 0),
            Key::FontCopies(_) => w(69, 0),
            Key::Cs(n) => w(63, u64::from(*n)),
            // (a font's: the font's name is hashed too)
            Key::HyphenChar(_) => w(21, 0),
            Key::SkewChar(_) => w(22, 0),
            Key::FontIdent(_) => w(23, 0),
            Key::FontLoaded(_) => w(24, 0),
            Key::File(_) => w(30, 0),
            Key::FontParams(_) => w(38, 0),
            Key::LastFont => w(39, 0),
            Key::Ended => w(40, 0),
            Key::MainTail => w(64, 0),
            Key::MainDepth => w(65, 0),
            Key::Active(x) => w(1, c(x)),
            Key::Cat(x) => w(2, c(x)),
            Key::Lccode(x) => w(3, c(x)),
            Key::Uccode(x) => w(4, c(x)),
            Key::Sfcode(x) => w(5, c(x)),
            Key::Mathcode(x) => w(6, c(x)),
            Key::Delcode(x) => w(7, c(x)),
            Key::IntPar(p) => w(8, *p as u64),
            Key::DimenPar(p) => w(9, *p as u64),
            Key::GluePar(p) => w(10, *p as u64),
            Key::TokPar(p) => w(11, *p as u64),
            Key::Count(n) => w(12, u64::from(*n)),
            Key::Dimen(n) => w(13, u64::from(*n)),
            Key::Skip(n) => w(14, u64::from(*n)),
            Key::Muskip(n) => w(15, u64::from(*n)),
            Key::Toks(n) => w(16, u64::from(*n)),
            Key::Box(n) => w(17, u64::from(*n)),
            Key::Parshape => w(18, 0),
            Key::Font => w(19, 0),
            Key::FamFont(s, f) => w(25, (u64::from(*s) << 8) | u64::from(*f)),
            Key::Interaction => w(26, 0),
            Key::ETeXMode => w(27, 0),
            Key::Patterns(l) => w(28, u64::from(*l)),
            Key::Exceptions(l) => w(29, u64::from(*l)),
            Key::PenaltiesArray(k) => w(31, u64::from(*k)),
            Key::Page(k) => w(32, u64::from(*k)),
            Key::Mark(k, c) => w(33, (u64::from(*k) << 16) | u64::from(*c)),
            Key::ReadStream(n) => w(34, u64::from(*n)),
            Key::Job => w(35, 0),
            Key::LineBase => w(36, 0),
            Key::SplitMarks => w(37, 0),
            Key::MagSet => w(42, 0),
            Key::Format => w(41, 0),
        }
    }
}


/// A macro: its parameter text and body (`Tok::Param` marks `#n`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Macro {
    pub params: Vec<Tok>,
    /// Shared with the calls reading it (`RState`'s pending input).
    pub body: Arc<[Tok]>,
    pub long: bool,
    pub outer: bool,
    /// e-TeX's `\protected`: not expanded where `\edef` and `\write`
    /// expand.
    pub protected: bool,
}

/// What a control sequence or active character means.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Meaning {
    #[default]
    Undefined,
    Prim(Prim),
    Macro(Arc<Macro>),
    /// `\let\x=a`.
    Char(u8, char),
    /// `\chardef`.
    CharGiven(u32),
    /// `\mathchardef`.
    MathCharGiven(u32),
    /// `\countdef` and friends: the register the name stands for.
    CountRef(u16),
    DimenRef(u16),
    SkipRef(u16),
    MuskipRef(u16),
    ToksRef(u16),
    /// A font identifier (`\font\x=...`; `\nullfont`): selecting it
    /// defines `Key::Font`.
    Font(Arc<Font>),
}

/// A glue specification (tex.web 150).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Glue {
    pub width: i32,
    pub stretch: i32,
    pub shrink: i32,
    pub stretch_order: u8,
    pub shrink_order: u8,
}

/// A value: what a definition binds a key to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Val {
    /// An integer, a dimension or a character code.
    Int(i32),
    Cat(u8),
    Meaning(Meaning),
    Font(Arc<Font>),
    Glue(Arc<Glue>),
    Toks(Arc<Vec<Tok>>),
    /// `\parshape`: (indent, length) per line.
    Parshape(Arc<Vec<(i32, i32)>>),
    /// A penalties array (e-TeX's `\interlinepenalties`…): its values; none
    /// is TeX's null.
    Penalties(Arc<[i32]>),
    /// A void box register.
    Void,
    /// A box register's box.
    Box(Arc<crate::item::BoxNode>),
    /// A name (`Key::FontIdent`).
    Name(Arc<str>),
    /// A language's patterns (`Key::Patterns`).
    Patterns(crate::hyph::Patterns),
    /// A language's exceptions (`Key::Exceptions`).
    Exceptions(crate::hyph::Exceptions),
    /// A file's bytes (`Key::File`).
    Bytes(Arc<[u8]>),
    /// An open read stream (`Key::ReadStream`).
    Stream(Arc<Stream>),
    /// The split marks (`Key::SplitMarks`): per class with a mark in the
    /// part the last `\vsplit` split off, by class, its first and last.
    SplitMarks(Arc<[SplitMark]>),
    /// A list's last item (`Key::MainTail`).
    Tail(Arc<crate::item::Item>),
    /// A paragraph before line breaking (`Key::MainDepth`).
    Para(Arc<crate::item::Para>),
    /// What a read of a meaning's outer-ness alone saw
    /// (`graph::Field::Outer`): only ever a piece's record of such a read,
    /// never a definition.
    Outer(bool),
}

impl Val {
    /// Whether this is the meaning of an `\outer` macro (tex.web 336:
    /// `cur_cmd>=outer_call`).
    pub fn is_outer(&self) -> bool {
        matches!(self, Val::Meaning(Meaning::Macro(m)) if m.outer) || matches!(self, Val::Outer(true))
    }

    /// Whether a read that saw this would see the same in `now`: equal, or
    /// for a read of the outer-ness alone, as outer.
    pub fn seen_as(&self, now: &Val) -> bool {
        match self {
            Val::Outer(o) => now.is_outer() == *o,
            _ => self == now,
        }
    }
}

/// A class's split marks: the class, the first mark, the last.
pub type SplitMark = (u16, Arc<Vec<Tok>>, Arc<Vec<Tok>>);

/// An open `\read` stream: a file's bytes and where its next line begins
/// (tex.web 480: `read_open` is `just_open` until a line has been read,
/// then `normal`). A value: `\read` defines the stream anew, advanced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stream {
    pub data: Arc<[u8]>,
    /// The byte offset of the next line.
    pub next: usize,
    /// A line has been read (`normal`; else `just_open`).
    pub normal: bool,
}

/// A token, a key or a value as text: control sequences by their names
/// (`CsTable`: a number is shown as the name it stands for).
pub struct Shown<'a, T>(pub &'a T, pub &'a CsTable);

impl fmt::Display for Shown<'_, Tok> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Tok::Char(_, c) | Tok::Active(c) => write!(f, "{c}"),
            Tok::Cs(n) => write!(f, "\\{}", self.1.name(*n)),
            Tok::Param(n, c) => write!(f, "{c}{n}"),
            Tok::DontExpand => Ok(()),
        }
    }
}

impl fmt::Display for Shown<'_, Key> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Key::Cs(n) => write!(f, "\\{}", self.1.name(*n)),
            Key::Active(c) => write!(f, "active {c}"),
            Key::Cat(c) => write!(f, "\\catcode`{}", c.escape_debug()),
            Key::Lccode(c) => write!(f, "\\lccode`{}", c.escape_debug()),
            Key::Uccode(c) => write!(f, "\\uccode`{}", c.escape_debug()),
            Key::Sfcode(c) => write!(f, "\\sfcode`{}", c.escape_debug()),
            Key::Mathcode(c) => write!(f, "\\mathcode`{}", c.escape_debug()),
            Key::Delcode(c) => write!(f, "\\delcode`{}", c.escape_debug()),
            Key::IntPar(p) => write!(f, "\\{}", p.name()),
            Key::DimenPar(p) => write!(f, "\\{}", p.name()),
            Key::GluePar(p) => write!(f, "\\{}", p.name()),
            Key::TokPar(p) => write!(f, "\\{}", p.name()),
            Key::Count(n) => write!(f, "\\count{n}"),
            Key::Dimen(n) => write!(f, "\\dimen{n}"),
            Key::Skip(n) => write!(f, "\\skip{n}"),
            Key::Muskip(n) => write!(f, "\\muskip{n}"),
            Key::Toks(n) => write!(f, "\\toks{n}"),
            Key::Box(n) => write!(f, "\\box{n}"),
            Key::Parshape => write!(f, "\\parshape"),
            Key::Font => write!(f, "font"),
            Key::FontDimen(x, n) => write!(f, "\\fontdimen{n} ({x})"),
            Key::HyphenChar(x) => write!(f, "\\hyphenchar ({x})"),
            Key::SkewChar(x) => write!(f, "\\skewchar ({x})"),
            Key::FontIdent(x) => write!(f, "font_id_text ({x})"),
            Key::FontLoaded(x) => write!(f, "font loaded ({x})"),
            Key::FamFont(s, x) => write!(f, "\\{}font{x}", ["text", "script", "scriptscript"][usize::from(*s).min(2)]),
            Key::File(n) => write!(f, "file {n}"),
            Key::ReadStream(n) => write!(f, "read stream {n}"),
            Key::Job => write!(f, "\\jobname"),
            Key::LineBase => write!(f, "line base"),
            Key::SplitMarks => write!(f, "\\splitfirstmarks"),
            Key::FontParams(x) => write!(f, "font_params ({x})"),
            Key::FontCode(k, x) => write!(
                f,
                "\\{} ({x})",
                crate::micro::CODE_NAMES.get(usize::from(*k)).copied().unwrap_or("?code")
            ),
            Key::FontExpand(x) => write!(f, "\\pdffontexpand ({x})"),
            Key::FontAttr(x) => write!(f, "\\pdffontattr ({x})"),
            Key::FontCopies(x) => write!(f, "font copies ({x})"),
            Key::LastFont => write!(f, "font_ptr"),
            Key::Format => write!(f, "format_ident"),
            Key::MagSet => write!(f, "mag_set"),
            Key::Ended => write!(f, "the end of the job"),
            Key::MainTail => write!(f, "the main vertical list's last item"),
            Key::MainDepth => write!(f, "the main vertical list's \\prevdepth"),
            Key::Interaction => write!(f, "\\interactionmode"),
            Key::ETeXMode => write!(f, "e-TeX mode"),
            Key::Patterns(l) => write!(f, "\\patterns (language {l})"),
            Key::Exceptions(l) => write!(f, "\\hyphenation (language {l})"),
            Key::PenaltiesArray(k) => write!(
                f,
                "\\{}penalties",
                ["interline", "club", "widow", "displaywidow"][usize::from(*k).min(3)]
            ),
            Key::Page(k) => write!(
                f,
                "\\{}",
                [
                    "pagegoal",
                    "pagetotal",
                    "pagestretch",
                    "pagefilstretch",
                    "pagefillstretch",
                    "pagefilllstretch",
                    "pageshrink",
                    "pagedepth",
                    "deadcycles",
                    "insertpenalties"
                ][usize::from(*k).min(9)]
            ),
            Key::Mark(k, c) => {
                let n = ["topmark", "firstmark", "botmark", "splitfirstmark", "splitbotmark"][usize::from(*k).min(4)];
                if *c == 0 { write!(f, "\\{n}") } else { write!(f, "\\{n}s{c}") }
            }
        }
    }
}

impl fmt::Display for Shown<'_, Val> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cs = self.1;
        match self.0 {
            Val::Int(i) => write!(f, "{i}"),
            Val::Cat(c) => write!(f, "cat {c}"),
            Val::Font(x) => write!(f, "font {}", x.name),
            Val::Glue(g) => write!(f, "{}", crate::print::spec(g, "pt")),
            Val::Toks(t) => {
                write!(f, "{{")?;
                for x in t.iter().take(24) {
                    write!(f, "{}", Shown(x, cs))?;
                }
                if t.len() > 24 {
                    write!(f, "...")?;
                }
                write!(f, "}}")
            }
            Val::Parshape(p) => write!(f, "parshape {}", p.len()),
            Val::Penalties(p) => write!(f, "penalties {}", p.len()),
            Val::Void => write!(f, "void"),
            Val::Box(b) => write!(f, "{}box({}+{})x{}", if b.vertical { "v" } else { "h" }, b.h, b.d, b.w),
            Val::Name(n) => write!(f, "{n}"),
            Val::Bytes(b) => write!(f, "{} bytes", b.len()),
            Val::Stream(x) => write!(f, "stream at {} of {}", x.next, x.data.len()),
            Val::SplitMarks(m) => write!(f, "split marks of {} classes", m.len()),
            Val::Tail(i) => write!(f, "the item {i:?}"),
            Val::Para(p) => write!(f, "a paragraph of {} items", p.hlist.len()),
            Val::Outer(o) => write!(f, "{}", if *o { "\\outer" } else { "not \\outer" }),
            Val::Patterns(p) => write!(f, "{} patterns", p.len()),
            Val::Exceptions(x) => write!(f, "{} exceptions", x.len()),
            Val::Meaning(Meaning::Font(x)) => write!(f, "select font {}", x.name),
            Val::Meaning(Meaning::Undefined) => write!(f, "undefined"),
            Val::Meaning(Meaning::Prim(p)) => write!(f, "primitive \\{}", p.name()),
            Val::Meaning(Meaning::Char(c, x)) => write!(f, "the character {x} (cat {c})"),
            Val::Meaning(Meaning::CharGiven(c)) => write!(f, "\\char{c}"),
            Val::Meaning(Meaning::MathCharGiven(c)) => write!(f, "\\mathchar{c}"),
            Val::Meaning(Meaning::CountRef(n)) => write!(f, "\\count{n}"),
            Val::Meaning(Meaning::DimenRef(n)) => write!(f, "\\dimen{n}"),
            Val::Meaning(Meaning::SkipRef(n)) => write!(f, "\\skip{n}"),
            Val::Meaning(Meaning::MuskipRef(n)) => write!(f, "\\muskip{n}"),
            Val::Meaning(Meaning::ToksRef(n)) => write!(f, "\\toks{n}"),
            Val::Meaning(Meaning::Macro(m)) => {
                write!(f, "macro:")?;
                for t in &m.params {
                    write!(f, "{}", Shown(t, cs))?;
                }
                write!(f, "->")?;
                for t in m.body.iter().take(24) {
                    write!(f, "{}", Shown(t, cs))?;
                }
                if m.body.len() > 24 {
                    write!(f, "...")?;
                }
                Ok(())
            }
        }
    }
}

/// A key's value before any definition: IniTeX's (tex.web 222-240, and
/// \year, \month, \day, \time as `SOURCE_DATE_EPOCH=0` gives them: the
/// DVI file's comment says 1970.01.01:0000). The font given to `Build` is
/// a file `\font` can load, not the initial font: that is `\nullfont`.
/// With `etex` (e-TeX's extended mode, etex.web §1337), e-TeX's
/// primitives are named too.
pub fn initial(key: &Key, _font: &Arc<Font>, etex: bool, cs: Option<&CsTable>) -> Val {
    let c32 = |c: &char| u32::from(*c) as i32;
    match key {
        Key::Cs(crate::build::CS_NULLFONT) => Val::Meaning(Meaning::Font(crate::tfm::null_font())),
        Key::Cs(n) => {
            let Some(cs) = cs else { return Val::Meaning(Meaning::Undefined) };
            let n = cs.name(*n);
            Val::Meaning(
                Prim::by_name(&n)
                    .or_else(|| if etex { Prim::by_name_etex(&n).or_else(|| Prim::by_name_pdftex(&n)) } else { None })
                    .or_else(|| Prim::internal(&n))
                    .map_or(Meaning::Undefined, Meaning::Prim),
            )
        }
        Key::Active(_) => Val::Meaning(Meaning::Undefined),
        Key::Cat(c) => Val::Cat(initial_cat(*c)),
        Key::Lccode(c) => Val::Int(match c {
            'A'..='Z' => c32(c) + 32,
            'a'..='z' => c32(c),
            _ => 0,
        }),
        Key::Uccode(c) => Val::Int(match c {
            'a'..='z' => c32(c) - 32,
            'A'..='Z' => c32(c),
            _ => 0,
        }),
        Key::Sfcode(c) => Val::Int(if c.is_ascii_uppercase() { 999 } else { 1000 }),
        Key::Mathcode(c) => Val::Int(match c {
            '0'..='9' => c32(c) + 0x7000,
            'A'..='Z' | 'a'..='z' => c32(c) + 0x7100,
            _ => c32(c),
        }),
        Key::Delcode(c) => Val::Int(if *c == '.' { 0 } else { -1 }),
        Key::IntPar(p) => Val::Int(p.initial()),
        Key::TokPar(_) | Key::Toks(_) | Key::Mark(..) => Val::Toks(Arc::new(Vec::new())),
        // (`\fontdimen`: the null font's, tex.web 552; a loaded font's are
        // defined by its `\font`)
        Key::DimenPar(p) => Val::Int(p.initial()),
        Key::Count(_)
        | Key::Dimen(_)
        | Key::FontDimen(..)
        | Key::FontLoaded(_)
        | Key::LineBase
        | Key::FontParams(_)
        | Key::FontCopies(_)
        | Key::Ended
        | Key::MagSet
        | Key::Interaction => Val::Int(0),
        Key::GluePar(_) | Key::Skip(_) | Key::Muskip(_) => Val::Glue(Arc::new(Glue::default())),
        Key::Box(_) | Key::File(_) | Key::ReadStream(_) | Key::MainTail => Val::Void,
        Key::Parshape => Val::Parshape(Arc::new(Vec::new())),
        Key::Font | Key::FamFont(..) => Val::Font(crate::tfm::null_font()),
        Key::HyphenChar(_) => Val::Int(i32::from(b'-')),
        Key::SkewChar(_) => Val::Int(-1),
        Key::FontIdent(_) | Key::LastFont => Val::Name(Arc::from("nullfont")),
        Key::Job => Val::Name(Arc::from("texput")),
        Key::SplitMarks => Val::SplitMarks(Arc::from(Vec::new())),
        // (and no `\pdffontattr` text)
        Key::Format | Key::FontAttr(_) => Val::Name(Arc::from("")),
        // (above, 0: the job not ended; no magnification fixed yet; batch
        // mode, as the oracles run TeX: `-interaction=batchmode`; a build
        // gives its own, `Build::set_interaction`: IniTeX's
        // `error_stop_mode` is 3)
        Key::MainDepth => Val::Int(crate::page::IGNORE_DEPTH),
        Key::ETeXMode => Val::Int(i32::from(etex)),
        Key::Patterns(_) => Val::Patterns(crate::hyph::Patterns::new()),
        Key::Exceptions(_) => Val::Exceptions(crate::hyph::Exceptions::new()),
        // (and pdfTeX's codes, `\pdffontexpand`'s parameters: none given)
        Key::PenaltiesArray(_) | Key::FontCode(..) | Key::FontExpand(_) => Val::Penalties(Arc::from([])),
        // (an empty page's, read outside an output routine: tex.web 421's
        // `max_dimen` for `\pagegoal`, else 0)
        Key::Page(k) => Val::Int(if *k == 0 { 0x3FFF_FFFF } else { 0 }),
    }
}

/// The category codes before any `\catcode`: IniTeX's (tex.web 232).
pub fn initial_cat(c: char) -> u8 {
    match c {
        '\\' => 0,
        '\r' => 5,
        '%' => 14,
        ' ' => 10,
        '\u{7f}' => 15,
        '\0' => 9,
        c if c.is_ascii_alphabetic() => 11,
        _ => 12,
    }
}
