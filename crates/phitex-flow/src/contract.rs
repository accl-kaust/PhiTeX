//! The contract every module codes against (AGENTS_BRIEF.md).
//!
//! A command is a function of what it reads through [`Cx`]. It never sees
//! a world: no table of meanings, no stack of saved values, no "current
//! state". It asks for the reaching definition of a [`Key`] (the graph
//! answers, or parks the reader until it can), defines keys, consumes its
//! reader's input, and builds material.
//!
//! Changing this file changes every module: agents propose a change to the
//! lead, who lands it on `flow` for everyone.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

// ---------------------------------------------------------------- names

/// An interned spelling (a control sequence's name). Equal spellings have
/// equal symbols in one run; a symbol is never part of a version (versions
/// hash spellings).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Sym(pub u32);

/// A per-character code table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Code {
    Cat,
    Lc,
    Uc,
    Sf,
    Math,
    Del,
}

/// A register bank.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Reg {
    Count,
    Dimen,
    Skip,
    Muskip,
    Toks,
    Box,
}

/// Anything a document can define: the thing a reaching definition is
/// looked up for. This is the language's namespace, not a storage layout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Key {
    /// A control sequence.
    Cs(Sym),
    /// An active character.
    Active(u32),
    /// A character's code in a table (`\catcode`65`).
    Code(Code, u32),
    /// A register (`\count3`).
    Reg(Reg, u16),
    /// A parameter (`\tolerance`, `\hsize`, `\everypar`): its number is
    /// the defs module's table (`defs::par`).
    Par(u16),
    /// The current font.
    Font,
    /// A math family's font in a size (`\textfont2`): (size 0..3, family).
    FamFont(u8, u8),
    /// A font's dimension or character data, keyed by font and number
    /// (`\fontdimen`, `\hyphenchar`, `\skewchar`): set per font, global.
    FontParam(FontId, u16),
    /// The interaction mode, `\deadcycles`, `\insertpenalties` and other
    /// globals the language defines: numbered in `defs`.
    Special(u16),
}

/// The part of a value a read depends on. A reader of one field is not
/// woken by a change to another.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Field {
    /// The whole value.
    #[default]
    All,
    /// A box's or list's dimensions (width, height, depth, shift, glue
    /// set): what the page builder reads.
    Dims,
    /// A box's or list's contents.
    Contents,
    /// A meaning's class only (`\outer`-ness, expandable or not, macro or
    /// primitive): what a token-only scan (a macro argument) reads.
    Class,
}

// ---------------------------------------------------------------- tokens

/// A token: a character with its category, or a control sequence. Packed
/// in one word: bit 31 set is a control sequence (`Sym` in the low bits);
/// otherwise category (4 bits) << 21 | character (21 bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tok(pub u32);

impl Tok {
    const CS: u32 = 1 << 31;
    #[must_use]
    pub fn char(cat: u8, c: u32) -> Tok {
        debug_assert!(cat < 16 && c < (1 << 21));
        Tok((u32::from(cat) << 21) | c)
    }
    #[must_use]
    pub fn cs(s: Sym) -> Tok {
        debug_assert!(s.0 < Self::CS);
        Tok(Self::CS | s.0)
    }
    #[must_use]
    pub fn as_cs(self) -> Option<Sym> {
        (self.0 & Self::CS != 0).then_some(Sym(self.0 & !Self::CS))
    }
    /// (category, character), for a character token.
    #[must_use]
    pub fn as_char(self) -> Option<(u8, u32)> {
        (self.0 & Self::CS == 0).then_some(((self.0 >> 21) as u8, self.0 & 0x1F_FFFF))
    }
}

impl fmt::Debug for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.as_cs(), self.as_char()) {
            (Some(s), _) => write!(f, "\\{}", s.0),
            (_, Some((c, ch))) => write!(f, "{}{c}", char::from_u32(ch).unwrap_or('?')),
            _ => unreachable!(),
        }
    }
}

/// A token list, shared.
pub type Toks = Arc<[Tok]>;

// ---------------------------------------------------------------- values

/// A loaded font's identity: a hash of its name, size and file contents,
/// stable across runs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FontId(pub u64);

/// A primitive's number (the defs/material modules' table) and its
/// modifier (TeX's `chr_code`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Prim {
    pub cmd: u16,
    pub chr: u32,
}

/// A macro.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Macro {
    pub long: bool,
    pub outer: bool,
    pub protected: bool,
    /// The parameter text, with `#n` as match tokens (defs' encoding).
    pub params: Toks,
    pub body: Toks,
}

/// What a control sequence or active character means.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Meaning {
    #[default]
    Undefined,
    Prim(Prim),
    Macro(Arc<Macro>),
    /// `\let\x=a`: a character with its category.
    Char(u8, u32),
    /// `\chardef`, `\mathchardef`, `\countdef` and the like: a primitive
    /// with a fixed operand (`Prim.chr`), e.g. a register number.
    Shorthand(Prim),
    /// `\font\x=...`: selects a font.
    Font(FontId),
}

/// Glue (and muglue).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Glue {
    pub width: i32,
    pub stretch: i32,
    pub stretch_order: u8,
    pub shrink: i32,
    pub shrink_order: u8,
}

/// A box's dimensions: what `Field::Dims` reads.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Dims {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub shift: i32,
}

/// One item of a horizontal, vertical or math list. The material module
/// owns the full set; this is the shape every module can match on.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Item {
    Char { font: FontId, c: u32 },
    Lig { font: FontId, c: u32, orig: Toks },
    Box(Arc<BoxVal>),
    Rule(Dims),
    Glue { glue: Glue, kind: u8, leader: Option<Arc<BoxVal>> },
    Kern { width: i32, kind: u8 },
    Penalty(i32),
    Disc { pre: Arc<[Item]>, post: Arc<[Item]>, replace: u16 },
    Mark { class: u16, toks: Toks },
    Ins { n: u8, height: i32, depth: i32, split_top: Glue, split_max: i32, float_cost: i32, list: Arc<[Item]> },
    Adjust(Arc<[Item]>),
    Math { kind: u8, width: i32 },
    /// A whatsit (`\write`, `\special`, PDF nodes): material's encoding.
    Whatsit(Arc<Whatsit>),
    /// Math-list items, before conversion: material's encoding.
    Noad(Arc<[u8]>),
}

/// A whatsit: its kind and payload.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Whatsit {
    pub kind: u16,
    pub toks: Toks,
    pub data: Arc<[u8]>,
}

/// A box: its dimensions and its contents, separate fields.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct BoxVal {
    pub vertical: bool,
    pub dims: Dims,
    pub glue_set: (u8, u8, u64), // (sign, order, f64 bits)
    pub contents: Arc<[Item]>,
}

/// A value: what a definition binds a key to.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Val {
    #[default]
    Void,
    Int(i32),
    Dimen(i32),
    Glue(Glue),
    Toks(Toks),
    Meaning(Meaning),
    Font(FontId),
    Box(Option<Arc<BoxVal>>),
    /// A source file's contents (a named source).
    Text(Arc<[u8]>),
}

/// A field's version: a hash of its content and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Ver(pub u64);

impl Val {
    /// The version of `field` of this value: content only (never a
    /// position, line, level or id). A `Sym` inside is hashed by spelling
    /// via `spell`.
    #[must_use]
    pub fn ver(&self, field: Field, spell: &dyn Fn(Sym) -> Arc<[u8]>) -> Ver {
        let mut h = Fx(0);
        match (self, field) {
            (Val::Box(Some(b)), Field::Dims) => {
                1u8.hash(&mut h);
                b.dims.hash(&mut h);
            }
            (Val::Box(Some(b)), Field::Contents) => {
                2u8.hash(&mut h);
                hash_items(&b.contents, &mut h, spell);
            }
            (Val::Meaning(m), Field::Class) => {
                3u8.hash(&mut h);
                std::mem::discriminant(m).hash(&mut h);
                if let Meaning::Macro(x) = m {
                    (x.outer, x.long, x.protected).hash(&mut h);
                }
                if let Meaning::Prim(p) = m {
                    p.cmd.hash(&mut h);
                }
            }
            (Val::Toks(t), _) => {
                4u8.hash(&mut h);
                hash_toks(t, &mut h, spell);
            }
            (Val::Meaning(Meaning::Macro(x)), _) => {
                5u8.hash(&mut h);
                (x.long, x.outer, x.protected).hash(&mut h);
                hash_toks(&x.params, &mut h, spell);
                hash_toks(&x.body, &mut h, spell);
            }
            (Val::Box(Some(b)), Field::All) => {
                6u8.hash(&mut h);
                (b.vertical, b.dims, b.glue_set).hash(&mut h);
                hash_items(&b.contents, &mut h, spell);
            }
            (v, _) => {
                7u8.hash(&mut h);
                v.hash(&mut h);
            }
        }
        Ver(h.0)
    }
}

fn hash_toks(t: &[Tok], h: &mut Fx, spell: &dyn Fn(Sym) -> Arc<[u8]>) {
    t.len().hash(h);
    for &x in t {
        match x.as_cs() {
            Some(s) => {
                0xFFu8.hash(h);
                spell(s).hash(h);
            }
            None => x.0.hash(h),
        }
    }
}

fn hash_items(items: &[Item], h: &mut Fx, spell: &dyn Fn(Sym) -> Arc<[u8]>) {
    items.len().hash(h);
    for i in items {
        match i {
            Item::Mark { class, toks } => {
                class.hash(h);
                hash_toks(toks, h, spell);
            }
            Item::Whatsit(w) => {
                w.kind.hash(h);
                hash_toks(&w.toks, h, spell);
                w.data.hash(h);
            }
            Item::Lig { font, c, orig } => {
                (font, c).hash(h);
                hash_toks(orig, h, spell);
            }
            Item::Box(b) => {
                (b.vertical, b.dims, b.glue_set).hash(h);
                hash_items(&b.contents, h, spell);
            }
            other => other.hash(h),
        }
    }
}

/// FxHash: fast, deterministic across runs and platforms.
struct Fx(u64);
impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }
}

// ---------------------------------------------------------------- material

/// The mode of the list a reader is building.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Mode {
    #[default]
    Vertical,
    InternalVertical,
    Horizontal,
    RestrictedHorizontal,
    Math,
    DisplayMath,
}

/// The kind of a group (TeX's group codes, as the language defines them).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Group {
    Simple,
    Hbox,
    AdjustedHbox,
    Vbox,
    Vtop,
    Align,
    NoAlign,
    Output,
    Math,
    Disc,
    Insert,
    Vcenter,
    MathChoice,
    SemiSimple,
    MathShift,
    MathLeft,
}

// ---------------------------------------------------------------- effects

/// Something a build produces outside its values, ordered by position
/// among its kind and joined at the end (AGENTS_BRIEF.md 7).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Effect {
    /// A page shipped: the box, and the `\count0..9` it was shipped with.
    Shipout { page: Arc<BoxVal>, counts: [i32; 10] },
    /// Text to stream `n` (16 and up: the terminal and log).
    Write { stream: u8, text: Arc<[u8]> },
    OpenOut { stream: u8, name: Arc<[u8]> },
    CloseOut { stream: u8 },
    /// Terminal and log output.
    Log(Arc<[u8]>),
}

// ---------------------------------------------------------------- Cx

/// How a reader's input continues after a pushed list (TeX's token-list
/// types, as far as a command needs to say).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Push {
    /// A macro's expansion (memoised by the meaning's version and the
    /// arguments).
    Macro,
    /// Inserted tokens (`\everypar`, `\the` results, back-up).
    Inserted,
}

/// All a command sees. Implemented by the graph (module `graph`), which
/// records every read as an operand of the node being built, parks the
/// reader when a read is not available yet, and wakes it when it is.
///
/// A command must not keep anything between calls except in what it
/// defines, emits or effects: commands are pure functions of their reads.
pub trait Cx {
    // --- the reader's input (its own; never shared)
    /// The next token, unexpanded; `None` at the end of the reader's span.
    fn next(&mut self) -> Option<Tok>;
    /// Puts a token back to be read next.
    fn back(&mut self, t: Tok);
    /// Inserts a list to be read next.
    fn push(&mut self, toks: Toks, how: Push);
    /// The source line of the reader's position: a read like any other
    /// (`\inputlineno`, error context), never part of a version.
    fn line(&mut self) -> u32;
    /// Opens a named source (`\input`): its contents, a value read like
    /// any other; `None` if there is no such file.
    fn source(&mut self, name: &[u8]) -> Option<Arc<[u8]>>;
    /// Starts reading a source's text next (an `\input` continuation).
    fn input(&mut self, name: &[u8], text: Arc<[u8]>);
    /// Ends the current input file (`\endinput`).
    fn end_input(&mut self);

    // --- names
    fn sym(&mut self, spelling: &[u8]) -> Sym;
    fn spelling(&self, s: Sym) -> Arc<[u8]>;
    /// The reaching definition of `k`, the part `f` of it. Parks the
    /// reader until it is known.
    fn read(&mut self, k: Key, f: Field) -> Val;
    /// Defines `k` here: local to the innermost group, or global.
    fn define(&mut self, k: Key, v: Val, global: bool);
    fn begin_group(&mut self, g: Group);
    /// Closes the innermost group: emits nothing; later reads resolve to
    /// older definitions. Returns the group's kind.
    fn end_group(&mut self) -> Group;
    /// The innermost group's kind, if any.
    fn group(&mut self) -> Option<Group>;

    // --- material
    fn mode(&mut self) -> Mode;
    /// Appends to the list being built.
    fn emit(&mut self, item: Item);
    /// Starts a new list in `mode` (a box, a paragraph, a math formula).
    fn push_list(&mut self, mode: Mode);
    /// Ends the innermost list and returns it, a value.
    fn pop_list(&mut self) -> Arc<[Item]>;
    /// The list being built so far (`\lastbox`, `\lastskip`, `\unskip`
    /// read and change its tail).
    fn tail(&mut self) -> Option<Item>;
    fn pop_tail(&mut self) -> Option<Item>;
    /// The list fields TeX keeps per list (`\prevdepth`, `\spacefactor`,
    /// `\prevgraf`, `\clubpenalties` state ...): material's numbering.
    fn list_field(&mut self, n: u16) -> i32;
    fn set_list_field(&mut self, n: u16, v: i32);

    // --- effects
    fn effect(&mut self, e: Effect);
}

/// A command: what a primitive does when the reader meets it. `chr` is the
/// primitive's modifier.
pub type Command = fn(cx: &mut dyn Cx, chr: u32);

/// An expandable primitive: rewrites the input (pushes tokens) and changes
/// no definitions.
pub type Expander = fn(cx: &mut dyn Cx, chr: u32);
