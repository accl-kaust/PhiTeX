//! Names, values and versions.
//!
//! A [`Key`] is anything a document can define. Its meaning at a point is
//! its reaching definition: a [`Val`]. Values are immutable; whether two
//! are the same is identity first (the same `Arc`), then equality.

use std::fmt;
use std::sync::Arc;

pub use crate::prim::{Al, Code, Conv, DimenPar, GluePar, IfKind, IntPar, Mat, MathP, Prim, Reg, Shorthand, TokPar, Xe};
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
    /// The key a control sequence or active character names. (Inlined:
    /// where the caller sees the variant, the key's drop is no call.)
    #[inline]
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
    /// (0–7, `page_so_far`), `\deadcycles` (8), `\insertpenalties` (9);
    /// pdfTeX's `\pdfinsertht n` (`n`, 10–254: the class's height on the
    /// page so far; classes 0–9 are not read, CORE.md).
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
    /// What the last `\pdfmatch` matched, for `\pdflastmatch` (global,
    /// pdfTeX's `match_string`, `pmatch`): 0 the string searched
    /// (`Val::Name`), 1 the submatches (`Val::Penalties`: start, end per
    /// subexpression up to the subcount, -1 for none; empty: the match
    /// failed, or none yet).
    LastMatch(u8),
    /// The token TeX inserts to end a paragraph (`par_token`, pdfTeX's
    /// `\partokenname`, global): `Val::Toks` of that one token; empty:
    /// `\par`.
    ParToken,
    /// pdfTeX's `\pdfglyphtounicode` entry of a glyph name (global, its
    /// `glyph_unicode_tree`): `Val::Int` (a code; -1 none), `Val::Name` (a
    /// string of hexadecimal digits); read by the PDF file's ToUnicode
    /// maps at the end of the job. The empty name is defined (`Val::Int`)
    /// once any entry is: the tree exists.
    GlyphUnicode(Arc<str>),
    /// pdfTeX's read-backs set by commands (global variables, never
    /// grouped): `\pdflastobj` (1), `\pdflastxform` (2), `\pdflastximage`
    /// (3), `\pdflastximagepages` (4), `\pdflastannot` (5), `\pdfretval`
    /// (8), `\pdflastximagecolordepth` (9), `\pdflastlink` (11): the
    /// codes of `prim::PDF_ITEMS`. Defined by the command, with the value
    /// the fold answered it (`PdfAns`).
    PdfLast(u8),
    /// The fold's answer to a PDF event (`pdf::what::Event`): in scope
    /// (0 a piece's nodes; 1 an output routine's) and fire (the routine's
    /// ordinal among the job's fires; 0 for a piece's nodes), of the
    /// command at node `node`, field `slot`
    /// (`index * ANSWER_FIELDS + field`). Answered by the build from the
    /// last fold (`Val::Int`; `Val::Void` before any), never defined.
    PdfAns(u32, u32, u32, u32),
    /// The random generator's state (`Val::Penalties`: the 55 values,
    /// `j_random`, the seed), global; `Val::Void` until first used, then
    /// seeded from `PdfSeed`.
    PdfRandom,
    /// The build's givens: the random seed pdfTeX takes from the clock,
    /// the elapsed time `\pdfelapsedtime` reads (both 0 unless given),
    /// the creation date (`\pdfcreationdate`, `D:…`).
    PdfSeed,
    PdfClock,
    PdfDate,
    // ---- XeTeX's
    /// `\XeTeXinterchartoks a b` (`Val::Toks`): the tokens put between a
    /// character of class `a` and one of class `b` (4095: the boundary).
    InterChar(u16, u16),
    /// The script a native font's last word was shaped in (XeTeX's
    /// `hb_buffer` script, which sets the next word's default direction):
    /// `Val::Int` (0: none), defined globally by the nodes setting words
    /// in the font (its key, `Font::key`).
    NativeDir(Arc<str>),
    /// How the input is decoded from the next line on
    /// (`\XeTeXinputencoding`; `Val::Int`, `graph::xetex`'s modes), and
    /// files opened later (`\XeTeXdefaultencoding`): global, as XeTeX sets
    /// them in place.
    InputEncoding,
    DefaultEncoding,
    /// `\XeTeXlinebreaklocale` (`Val::Name`; empty: none): global, as
    /// XeTeX assigns it in place (no save stack).
    LinebreakLocale,
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
            Key::PdfAns(a, b, c, d) => {
                h.write_u64((80 << 40) | (u64::from(*a) << 20) ^ u64::from(*b));
                h.write_u64((u64::from(*c) << 32) | u64::from(*d));
            }
            Key::FontCode(k, n) => {
                n.hash(h);
                h.write_u64((70 << 40) | u64::from(*k));
            }
            Key::HyphenChar(n)
            | Key::SkewChar(n)
            | Key::FontIdent(n)
            | Key::FontLoaded(n)
            | Key::File(n)
            | Key::FontParams(n)
            | Key::FontExpand(n)
            | Key::FontAttr(n)
            | Key::FontCopies(n)
            | Key::GlyphUnicode(n)
            | Key::NativeDir(n) => {
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
            | (Key::FontCopies(a), Key::FontCopies(b))
            | (Key::GlyphUnicode(a), Key::GlyphUnicode(b))
            | (Key::NativeDir(a), Key::NativeDir(b)) => Arc::ptr_eq(a, b) || a == b,
            (Key::FontCode(k, a), Key::FontCode(l, b)) => k == l && (Arc::ptr_eq(a, b) || a == b),
            (Key::Cat(a), Key::Cat(b)) => a == b,
            (Key::FontDimen(a, k), Key::FontDimen(b, l)) => k == l && (Arc::ptr_eq(a, b) || a == b),
            (Key::PdfAns(a, b, c, d), Key::PdfAns(x, y, z, w)) => a == x && b == y && c == z && d == w,
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
            Key::FontDimen(..) | Key::FontCode(..) | Key::PdfAns(..) => u64::MAX,
            Key::FontExpand(_) => w(67, 0),
            Key::FontAttr(_) => w(68, 0),
            Key::FontCopies(_) => w(69, 0),
            Key::LastMatch(k) => w(71, u64::from(*k)),
            Key::ParToken => w(72, 0),
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
            Key::GlyphUnicode(_) => w(66, 0),
            Key::PdfLast(k) => w(81, u64::from(*k)),
            Key::PdfRandom => w(82, 0),
            Key::PdfSeed => w(83, 0),
            Key::PdfClock => w(84, 0),
            Key::PdfDate => w(85, 0),
            Key::NativeDir(_) => w(87, 0),
            Key::InterChar(a, b) => w(88, (u64::from(*a) << 16) | u64::from(*b)),
            Key::InputEncoding => w(89, 0),
            Key::DefaultEncoding => w(90, 0),
            Key::LinebreakLocale => w(91, 0),
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
    /// XeTeX's `\Umathchardef`, `\Umathcharnumdef`: a 32-bit math code
    /// (`fam<<24 | class<<21 | usv`).
    XMathCharGiven(u32),
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
            Key::LastMatch(k) => write!(f, "\\pdflastmatch ({k})"),
            Key::ParToken => write!(f, "\\partokenname"),
            Key::LastFont => write!(f, "font_ptr"),
            Key::Format => write!(f, "format_ident"),
            Key::MagSet => write!(f, "mag_set"),
            Key::Ended => write!(f, "the end of the job"),
            Key::MainTail => write!(f, "the main vertical list's last item"),
            Key::MainDepth => write!(f, "the main vertical list's \\prevdepth"),
            Key::GlyphUnicode(x) => write!(f, "\\pdfglyphtounicode ({x})"),
            Key::PdfLast(k) => write!(f, "pdfTeX's read-back {k}"),
            Key::PdfAns(a, b, c, d) => write!(f, "the PDF fold's answer ({a}, {b}, {c}, {d})"),
            Key::PdfRandom => write!(f, "the random generator"),
            Key::PdfSeed => write!(f, "the random seed given"),
            Key::PdfClock => write!(f, "the clock given"),
            Key::PdfDate => write!(f, "the creation date given"),
            Key::InterChar(a, b) => write!(f, "\\XeTeXinterchartoks{a} {b}"),
            Key::NativeDir(x) => write!(f, "the last script shaped ({x})"),
            Key::InputEncoding => write!(f, "\\XeTeXinputencoding"),
            Key::DefaultEncoding => write!(f, "\\XeTeXdefaultencoding"),
            Key::LinebreakLocale => write!(f, "\\XeTeXlinebreaklocale"),
            Key::Interaction => write!(f, "\\interactionmode"),
            Key::ETeXMode => write!(f, "e-TeX mode"),
            Key::Patterns(l) => write!(f, "\\patterns (language {l})"),
            Key::Exceptions(l) => write!(f, "\\hyphenation (language {l})"),
            Key::PenaltiesArray(k) => write!(
                f,
                "\\{}penalties",
                ["interline", "club", "widow", "displaywidow"][usize::from(*k).min(3)]
            ),
            Key::Page(k) if *k > 9 => write!(f, "\\pdfinsertht{k}"),
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
            Val::Meaning(Meaning::XMathCharGiven(c)) => write!(f, "\\Umathchar{c}"),
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
pub fn initial(key: &Key, font: &Arc<Font>, etex: bool, cs: Option<&CsTable>) -> Val {
    initial_in(key, font, etex, false, cs)
}

/// [`initial`] for an engine: with `xetex`, XeTeX's IniTeX (xetex.web
/// §232, §240): XeTeX's primitives named (with e-TeX's), its math codes
/// (`fam<<24 | class<<21 | usv`: digits class 7, letters class 7 family
/// 1), its space factor codes holding the character class above 16 bits
/// (class 0), its parameters' initial values.
pub fn initial_in(key: &Key, _font: &Arc<Font>, etex: bool, xetex: bool, cs: Option<&CsTable>) -> Val {
    let c32 = |c: &char| u32::from(*c) as i32;
    if xetex {
        match key {
            Key::Cs(crate::build::CS_NULLFONT) => {}
            Key::Cs(n) => {
                let Some(cs) = cs else { return Val::Meaning(Meaning::Undefined) };
                let n = cs.name(*n);
                return Val::Meaning(
                    Prim::by_name(&n)
                        .or_else(|| Prim::by_name_etex(&n))
                        .or_else(|| Prim::by_name_xetex(&n))
                        .or_else(|| Prim::internal(&n))
                        .map_or(Meaning::Undefined, Meaning::Prim),
                );
            }
            Key::Mathcode(c) => {
                return Val::Int(match c {
                    '0'..='9' => c32(c) + (7 << 21),
                    'A'..='Z' | 'a'..='z' => c32(c) + (1 << 24) + (7 << 21),
                    _ => c32(c),
                });
            }
            Key::InterChar(..) => return Val::Toks(Arc::new(Vec::new())),
            Key::NativeDir(_) | Key::InputEncoding | Key::DefaultEncoding => return Val::Int(0),
            Key::LinebreakLocale => return Val::Name(Arc::from("")),
            _ => {}
        }
    }
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
        Key::TokPar(_) | Key::Toks(_) | Key::Mark(..) | Key::ParToken | Key::InterChar(..) => Val::Toks(Arc::new(Vec::new())),
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
        | Key::Interaction
        | Key::NativeDir(_)
        | Key::InputEncoding
        | Key::DefaultEncoding
        | Key::PdfLast(_)
        | Key::PdfSeed
        | Key::PdfClock => Val::Int(0),
        Key::GluePar(_) | Key::Skip(_) | Key::Muskip(_) => Val::Glue(Arc::new(Glue::default())),
        Key::Box(_)
        | Key::File(_)
        | Key::ReadStream(_)
        | Key::MainTail
        | Key::GlyphUnicode(_)
        | Key::PdfAns(..)
        | Key::PdfRandom => Val::Void,
        Key::Parshape => Val::Parshape(Arc::new(Vec::new())),
        Key::Font | Key::FamFont(..) => Val::Font(crate::tfm::null_font()),
        Key::HyphenChar(_) => Val::Int(i32::from(b'-')),
        Key::SkewChar(_) => Val::Int(-1),
        Key::FontIdent(_) | Key::LastFont => Val::Name(Arc::from("nullfont")),
        Key::Job => Val::Name(Arc::from("texput")),
        Key::SplitMarks => Val::SplitMarks(Arc::from(Vec::new())),
        // (and no `\pdffontattr` text)
        Key::Format | Key::FontAttr(_) | Key::LastMatch(0) | Key::LinebreakLocale | Key::PdfDate => Val::Name(Arc::from("")),
        // (above, 0: the job not ended; no magnification fixed yet; batch
        // mode, as the oracles run TeX: `-interaction=batchmode`; a build
        // gives its own, `Build::set_interaction`: IniTeX's
        // `error_stop_mode` is 3)
        Key::MainDepth => Val::Int(crate::page::IGNORE_DEPTH),
        Key::ETeXMode => Val::Int(i32::from(etex)),
        Key::Patterns(_) => Val::Patterns(crate::hyph::Patterns::new()),
        Key::Exceptions(_) => Val::Exceptions(crate::hyph::Exceptions::new()),
        // (and pdfTeX's codes, `\pdffontexpand`'s parameters: none given)
        Key::PenaltiesArray(_) | Key::FontCode(..) | Key::FontExpand(_) | Key::LastMatch(_) => {
            Val::Penalties(Arc::from([]))
        }
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

/// One key of each kind, for the tag tests (`Key::word`'s here, the
/// format's in `format.rs`): `key_index` has no wildcard, so a new kind
/// does not compile until it has an index, and `every_key_once` fails
/// until it has a key here.
#[cfg(test)]
pub(crate) fn every_key() -> Vec<Key> {
    use crate::prim::{DimenPar, GluePar, IntPar, TokPar};
    let f: Arc<str> = Arc::from("cmr10");
    vec![
        Key::Cs(5),
        Key::Active('a'),
        Key::Cat('a'),
        Key::Lccode('a'),
        Key::Uccode('a'),
        Key::Sfcode('a'),
        Key::Mathcode('a'),
        Key::Delcode('a'),
        Key::IntPar(IntPar::Tolerance),
        Key::DimenPar(DimenPar::Hsize),
        Key::GluePar(GluePar::Parskip),
        Key::TokPar(TokPar::Output),
        Key::Count(1),
        Key::Dimen(1),
        Key::Skip(1),
        Key::Muskip(1),
        Key::Toks(1),
        Key::Box(1),
        Key::Parshape,
        Key::Font,
        Key::FontDimen(f.clone(), 1),
        Key::HyphenChar(f.clone()),
        Key::SkewChar(f.clone()),
        Key::FontIdent(f.clone()),
        Key::FontLoaded(f.clone()),
        Key::FamFont(0, 1),
        Key::File(Arc::from("a.tex")),
        Key::ReadStream(1),
        Key::Job,
        Key::LineBase,
        Key::SplitMarks,
        Key::Interaction,
        Key::ETeXMode,
        Key::Patterns(0),
        Key::Exceptions(0),
        Key::PenaltiesArray(0),
        Key::Page(0),
        Key::Mark(0, 1),
        Key::Format,
        Key::MagSet,
        Key::FontParams(f.clone()),
        Key::LastFont,
        Key::Ended,
        Key::MainTail,
        Key::MainDepth,
        Key::GlyphUnicode(f.clone()),
        Key::FontCode(crate::micro::EF, f.clone()),
        Key::FontExpand(f.clone()),
        Key::FontAttr(f.clone()),
        Key::FontCopies(f.clone()),
        Key::LastMatch(0),
        Key::ParToken,
        Key::InterChar(0, 1),
        Key::NativeDir(f.clone()),
        Key::InputEncoding,
        Key::DefaultEncoding,
        Key::LinebreakLocale,
        Key::PdfLast(1),
        Key::PdfAns(0, 0, 1, 0),
        Key::PdfRandom,
        Key::PdfSeed,
        Key::PdfClock,
        Key::PdfDate,
    ]
}

/// Each kind of key's place in [`every_key`] (no wildcard: see there).
#[cfg(test)]
pub(crate) fn key_index(k: &Key) -> usize {
    match k {
        Key::Cs(_) => 0,
        Key::Active(_) => 1,
        Key::Cat(_) => 2,
        Key::Lccode(_) => 3,
        Key::Uccode(_) => 4,
        Key::Sfcode(_) => 5,
        Key::Mathcode(_) => 6,
        Key::Delcode(_) => 7,
        Key::IntPar(_) => 8,
        Key::DimenPar(_) => 9,
        Key::GluePar(_) => 10,
        Key::TokPar(_) => 11,
        Key::Count(_) => 12,
        Key::Dimen(_) => 13,
        Key::Skip(_) => 14,
        Key::Muskip(_) => 15,
        Key::Toks(_) => 16,
        Key::Box(_) => 17,
        Key::Parshape => 18,
        Key::Font => 19,
        Key::FontDimen(..) => 20,
        Key::HyphenChar(_) => 21,
        Key::SkewChar(_) => 22,
        Key::FontIdent(_) => 23,
        Key::FontLoaded(_) => 24,
        Key::FamFont(..) => 25,
        Key::File(_) => 26,
        Key::ReadStream(_) => 27,
        Key::Job => 28,
        Key::LineBase => 29,
        Key::SplitMarks => 30,
        Key::Interaction => 31,
        Key::ETeXMode => 32,
        Key::Patterns(_) => 33,
        Key::Exceptions(_) => 34,
        Key::PenaltiesArray(_) => 35,
        Key::Page(_) => 36,
        Key::Mark(..) => 37,
        Key::Format => 38,
        Key::MagSet => 39,
        Key::FontParams(_) => 40,
        Key::LastFont => 41,
        Key::Ended => 42,
        Key::MainTail => 43,
        Key::MainDepth => 44,
        Key::GlyphUnicode(_) => 45,
        Key::FontCode(..) => 46,
        Key::FontExpand(_) => 47,
        Key::FontAttr(_) => 48,
        Key::FontCopies(_) => 49,
        Key::LastMatch(_) => 50,
        Key::ParToken => 51,
        Key::InterChar(..) => 52,
        Key::NativeDir(_) => 53,
        Key::InputEncoding => 54,
        Key::DefaultEncoding => 55,
        Key::LinebreakLocale => 56,
        Key::PdfLast(_) => 57,
        Key::PdfAns(..) => 58,
        Key::PdfRandom => 59,
        Key::PdfSeed => 60,
        Key::PdfClock => 61,
        Key::PdfDate => 62,
    }
}

/// The tag numbers (CORE.md "Tag numbers"): every kind of key is in
/// [`every_key`] once, and no two kinds share `Key::word`'s tag (the bits
/// above 40, which `Hash` and `Eq` compare).
#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn every_key_once() {
        let mut ix: Vec<usize> = every_key().iter().map(key_index).collect();
        ix.sort_unstable();
        let want: Vec<usize> = (0..ix.len()).collect();
        assert_eq!(ix, want, "every_key: one key of each kind, each kind's index in key_index");
    }

    #[test]
    fn key_word_tags_are_distinct() {
        let mut seen: std::collections::BTreeMap<u64, Key> = std::collections::BTreeMap::new();
        for k in every_key() {
            // (a font dimension is compared by its font's name: no tag)
            if k.word() == u64::MAX {
                continue;
            }
            let tag = k.word() >> 40;
            if let Some(o) = seen.insert(tag, k.clone()) {
                panic!("Key::word tag {tag}: {o:?} and {k:?}");
            }
        }
    }
}
