//! Names, values and versions.
//!
//! A [`Key`] is anything a document can define. Its meaning at a point is
//! its reaching definition: a [`Val`]. Values are immutable; whether two
//! are the same is identity first (the same `Arc`), then equality.

use std::fmt;
use std::sync::Arc;

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
            Tok::Char(_, c) => write!(f, "{c}"),
            Tok::Cs(n) => write!(f, "\\{n}"),
            Tok::Active(c) => write!(f, "{c}"),
            Tok::Param(n) => write!(f, "#{n}"),
        }
    }
}

/// A parameter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Param {
    Hsize,
    Vsize,
    Parindent,
    Baselineskip,
    Parskip,
    Topskip,
    Maxdepth,
    Tolerance,
    Linepenalty,
}

impl Param {
    pub const ALL: [Param; 9] = [
        Param::Hsize,
        Param::Vsize,
        Param::Parindent,
        Param::Baselineskip,
        Param::Parskip,
        Param::Topskip,
        Param::Maxdepth,
        Param::Tolerance,
        Param::Linepenalty,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Param::Hsize => "hsize",
            Param::Vsize => "vsize",
            Param::Parindent => "parindent",
            Param::Baselineskip => "baselineskip",
            Param::Parskip => "parskip",
            Param::Topskip => "topskip",
            Param::Maxdepth => "maxdepth",
            Param::Tolerance => "tolerance",
            Param::Linepenalty => "linepenalty",
        }
    }
    pub fn is_int(self) -> bool {
        matches!(self, Param::Tolerance | Param::Linepenalty)
    }
    /// The value before any definition (plain TeX's, roughly).
    pub fn initial(self) -> i32 {
        const PT: i32 = 65536;
        match self {
            Param::Hsize => 468 * PT,
            Param::Vsize => 643 * PT,
            Param::Parindent => 20 * PT,
            Param::Baselineskip => 12 * PT,
            Param::Parskip => 0,
            Param::Topskip => 10 * PT,
            Param::Maxdepth => 4 * PT,
            Param::Tolerance => 200,
            Param::Linepenalty => 10,
        }
    }
}

/// Anything a document can define.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Key {
    Cs(Arc<str>),
    Active(char),
    /// A character's category code.
    Cat(char),
    Param(Param),
    /// The current font.
    Font,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Cs(n) => write!(f, "\\{n}"),
            Key::Active(c) => write!(f, "active {c}"),
            Key::Cat(c) => write!(f, "\\catcode`{}", c.escape_debug()),
            Key::Param(p) => write!(f, "\\{}", p.name()),
            Key::Font => write!(f, "font"),
        }
    }
}

/// A primitive.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Prim {
    Def,
    Gdef,
    Global,
    Let,
    Ifx,
    Iftrue,
    Iffalse,
    Else,
    Fi,
    Catcode,
    Par,
    Relax,
    Indent,
    Noindent,
    /// `\ ` (control space): an interword space.
    Space,
    Param(Param),
}

impl Prim {
    pub fn expandable(self) -> bool {
        matches!(self, Prim::Ifx | Prim::Iftrue | Prim::Iffalse | Prim::Else | Prim::Fi)
    }
    pub fn by_name(n: &str) -> Option<Prim> {
        Some(match n {
            "def" => Prim::Def,
            "gdef" => Prim::Gdef,
            "global" => Prim::Global,
            "let" => Prim::Let,
            "ifx" => Prim::Ifx,
            "iftrue" => Prim::Iftrue,
            "iffalse" => Prim::Iffalse,
            "else" => Prim::Else,
            "fi" => Prim::Fi,
            "catcode" => Prim::Catcode,
            "par" => Prim::Par,
            "relax" => Prim::Relax,
            "indent" => Prim::Indent,
            "noindent" => Prim::Noindent,
            " " => Prim::Space,
            _ => return Param::ALL.iter().find(|p| p.name() == n).map(|&p| Prim::Param(p)),
        })
    }
}

/// A macro: its parameter text and body (`Tok::Param` marks `#n`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Macro {
    pub params: Vec<Tok>,
    pub body: Vec<Tok>,
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
}

/// A value: what a definition binds a key to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Val {
    Int(i32),
    Cat(u8),
    Meaning(Meaning),
    Font(Arc<Font>),
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Cat(c) => write!(f, "cat {c}"),
            Val::Font(x) => write!(f, "font {}", x.name),
            Val::Meaning(Meaning::Undefined) => write!(f, "undefined"),
            Val::Meaning(Meaning::Prim(p)) => write!(f, "primitive {p:?}"),
            Val::Meaning(Meaning::Char(c, x)) => write!(f, "the character {x} (cat {c})"),
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

/// A key's value before any definition.
pub fn initial(key: &Key, font: &Arc<Font>) -> Val {
    match key {
        Key::Cs(n) => Val::Meaning(Prim::by_name(n).map_or(Meaning::Undefined, Meaning::Prim)),
        Key::Active(_) => Val::Meaning(Meaning::Undefined),
        Key::Cat(c) => Val::Cat(initial_cat(*c)),
        Key::Param(p) => Val::Int(p.initial()),
        Key::Font => Val::Font(font.clone()),
    }
}

/// The category codes before any `\catcode` (braces, `#` and `$` set as
/// plain TeX sets them).
pub fn initial_cat(c: char) -> u8 {
    match c {
        '\\' => 0,
        '{' => 1,
        '}' => 2,
        '$' => 3,
        '&' => 4,
        '\r' => 5,
        '#' => 6,
        '^' => 7,
        '%' => 14,
        ' ' => 10,
        '\u{7f}' => 15,
        '\0' => 9,
        '~' => 13,
        c if c.is_ascii_alphabetic() => 11,
        _ => 12,
    }
}
