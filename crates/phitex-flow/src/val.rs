//! Names, values and versions.
//!
//! A [`Key`] is anything a document can define. Its meaning at a point is
//! its reaching definition: a [`Val`]. Values are immutable; whether two
//! are the same is identity first (the same `Arc`), then equality.

use std::fmt;
use std::sync::Arc;

pub use crate::prim::{Code, Conv, DimenPar, GluePar, IfKind, IntPar, Mat, MathP, Prim, Reg, Shorthand, TokPar};
use crate::tfm::Font;

/// A token.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Tok {
    /// A character with its category.
    Char(u8, char),
    /// A control sequence.
    Cs(Arc<str>),
    /// An active character.
    Active(char),
    /// `#n` in a macro's parameter text or body.
    Param(u8),
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
            Tok::Cs(n) => Some(Key::Cs(n.clone())),
            Tok::Active(c) => Some(Key::Active(*c)),
            _ => None,
        }
    }
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Char(_, c) | Tok::Active(c) => write!(f, "{c}"),
            Tok::Cs(n) => write!(f, "\\{n}"),
            Tok::Param(n) => write!(f, "#{n}"),
            Tok::DontExpand => Ok(()),
        }
    }
}

/// Anything a document can define: every quantity TeX keeps in its
/// tables is a name with a reaching definition.
#[derive(Clone, Debug, PartialOrd, Ord)]
pub enum Key {
    Cs(Arc<str>),
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
    FontDimen(Arc<str>, u16),
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
    /// A file's text, by its name (`\input` reads it: a changed file runs
    /// the `\input` node again).
    File(Arc<str>),
    /// `\topmark` (0), `\firstmark` (1), `\botmark` (2): the page
    /// builder's marks, defined where it fires.
    CurMark(u8),
    /// `\deadcycles` (0), `\insertpenalties` (1): the page builder's
    /// counts, as the fold hands them to an output routine (assignments
    /// are global, tex.web 1246).
    PageInt(u8),
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
            Key::Cs(n) => n.hash(h),
            Key::Cat(x) => h.write_u64((2 << 40) | u64::from(u32::from(*x))),
            Key::FontDimen(n, k) => {
                n.hash(h);
                h.write_u64((20 << 40) | u64::from(*k));
            }
            Key::HyphenChar(n) | Key::SkewChar(n) | Key::FontIdent(n) | Key::FontLoaded(n) | Key::File(n) => {
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
            (Key::Cs(a), Key::Cs(b))
            | (Key::HyphenChar(a), Key::HyphenChar(b))
            | (Key::SkewChar(a), Key::SkewChar(b))
            | (Key::FontIdent(a), Key::FontIdent(b))
            | (Key::FontLoaded(a), Key::FontLoaded(b))
            | (Key::File(a), Key::File(b)) => Arc::ptr_eq(a, b) || a == b,
            (Key::Cat(a), Key::Cat(b)) => a == b,
            (Key::FontDimen(a, k), Key::FontDimen(b, l)) => k == l && (Arc::ptr_eq(a, b) || a == b),
            (Key::Cs(_), _) | (_, Key::Cs(_)) => false,
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
            Key::Cs(_) | Key::FontDimen(..) => u64::MAX,
            // (a font's: the font's name is hashed too)
            Key::HyphenChar(_) => w(21, 0),
            Key::SkewChar(_) => w(22, 0),
            Key::FontIdent(_) => w(23, 0),
            Key::FontLoaded(_) => w(24, 0),
            Key::File(_) => w(26, 0),
            Key::CurMark(k) => w(27, u64::from(*k)),
            Key::PageInt(k) => w(28, u64::from(*k)),
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
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Cs(n) => write!(f, "\\{n}"),
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
            Key::File(x) => write!(f, "file {x}"),
            Key::CurMark(k) => write!(f, "\\{}mark", ["top", "first", "bot"][usize::from(*k).min(2)]),
            Key::PageInt(k) => write!(f, "\\{}", if *k == 0 { "deadcycles" } else { "insertpenalties" }),
        }
    }
}

/// A macro: its parameter text and body (`Tok::Param` marks `#n`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Macro {
    pub params: Vec<Tok>,
    pub body: Vec<Tok>,
    pub long: bool,
    pub outer: bool,
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
    /// A void box register.
    Void,
    /// A box register's box.
    Box(Arc<crate::item::BoxNode>),
    /// A name (`Key::FontIdent`).
    Name(Arc<str>),
    /// A file's text (`Key::File`); `None`: there is no such file.
    Text(Option<Arc<str>>),
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Cat(c) => write!(f, "cat {c}"),
            Val::Font(x) => write!(f, "font {}", x.name),
            Val::Glue(g) => write!(f, "{}", crate::print::spec(g, "pt")),
            Val::Toks(t) => {
                write!(f, "{{")?;
                for x in t.iter().take(24) {
                    write!(f, "{x}")?;
                }
                if t.len() > 24 {
                    write!(f, "...")?;
                }
                write!(f, "}}")
            }
            Val::Parshape(p) => write!(f, "parshape {}", p.len()),
            Val::Void => write!(f, "void"),
            Val::Box(b) => write!(f, "{}box({}+{})x{}", if b.vertical { "v" } else { "h" }, b.h, b.d, b.w),
            Val::Name(n) => write!(f, "{n}"),
            Val::Text(None) => write!(f, "no file"),
            Val::Text(Some(t)) => write!(f, "text ({} bytes)", t.len()),
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
                    write!(f, "{t}")?;
                }
                write!(f, "->")?;
                for t in m.body.iter().take(24) {
                    write!(f, "{t}")?;
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
pub fn initial(key: &Key, _font: &Arc<Font>) -> Val {
    let c32 = |c: &char| u32::from(*c) as i32;
    match key {
        Key::Cs(n) if &**n == "nullfont" => Val::Meaning(Meaning::Font(crate::tfm::null_font())),
        Key::Cs(n) => Val::Meaning(Prim::by_name(n).or_else(|| Prim::internal(n)).map_or(Meaning::Undefined, Meaning::Prim)),
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
        Key::TokPar(_) | Key::Toks(_) | Key::CurMark(_) => Val::Toks(Arc::new(Vec::new())),
        // (`\fontdimen`: the null font's, tex.web 552; a loaded font's are
        // defined by its `\font`)
        Key::DimenPar(_)
        | Key::Count(_)
        | Key::Dimen(_)
        | Key::FontDimen(..)
        | Key::FontLoaded(_)
        | Key::PageInt(_) => Val::Int(0),
        Key::GluePar(_) | Key::Skip(_) | Key::Muskip(_) => Val::Glue(Arc::new(Glue::default())),
        Key::Box(_) => Val::Void,
        Key::Parshape => Val::Parshape(Arc::new(Vec::new())),
        Key::Font | Key::FamFont(..) => Val::Font(crate::tfm::null_font()),
        Key::HyphenChar(_) => Val::Int(i32::from(b'-')),
        Key::SkewChar(_) => Val::Int(-1),
        Key::FontIdent(_) => Val::Name(Arc::from("nullfont")),
        // (a file's text is the build's, `Build::set_file`: the names
        // index answers it)
        Key::File(_) => Val::Text(None),
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
