//! The engine on the dynamic-SSA runtime (`DESIGN.md` §7.17.10 step 2):
//! the runtime (`partex_ssa`) is the memo and the recorder, the engine
//! owns its state, and the engine's [`Tracker`] is the runtime's
//! recorder. `PARTEX_SSA=1` runs a build as the root call with a call
//! per paragraph (from one clean point to the next, `Tex::clean_point`)
//! and a tokenizer call for the line each paragraph starts on.
//!
//! Addresses name a family and a slot ([`Slot`]), never a control
//! sequence's name. Versions (DESIGN 7.17.12's first convention): an
//! `eqtb`, `text` or `next` slot's is its content, made by the writing
//! accessor after the store ([`Tracker::wrote`]) and by the tables' bulk
//! writers (`Tex::version_tables`), kept in an array beside the entries,
//! so a read is an index; the source a call consumes is read relative to
//! the call's start ([`Fam::Source`], [`Fam::Line`]), a lookup by the
//! interned name ([`Fam::Name`]), web2c's string search by its result
//! ([`Fam::Search`]) and a file by its contents ([`Fam::Load`]); a font's
//! fields, the table of loaded fonts and hyphenation's patterns,
//! exceptions and exception words ([`Fam::Font`], [`Fam::FontTable`],
//! [`Fam::Hyph`]) are versioned like the tables, by content, in arrays
//! beside them; a slot of another family (streams, the random generator)
//! has a revision, bumped at each write and tagged with the build
//! (revisions do not compare across builds, whose engines start afresh).
//!
//! A hit is not yet applied: the body runs either way (`probe`), and
//! check mode (`PARTEX_SSA_CHECK=1`) compares, at each hit, the state an
//! applied hit would leave (the entry state with the record's writes, which
//! cover only the tables) with the state the body left, by 7.17.12's value
//! rows (`Tex::value_rows`, scratch rows left out), and tests the arrays:
//! every table read's version against the slot's content.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

use partex_ssa::runtime::RecId;
use partex_ssa::{Config, Cx, Found, Loc, Machine, Runtime, Store, Value, Version};

use crate::host::Host;
use crate::run::{CleanPoint, Step};
use crate::tex::Tex;
use crate::track::{Cell, LineCodes, Output, Row, Tracker, line_tokens};

mod rebuild;
mod view;

pub use rebuild::{RebuildReport, Trips, rebuild, rebuild_log, rebuild_trips, settle};
pub use view::{dag, step_trace, view};

/// A slot's family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fam {
    Eqtb,
    Hash,
    HashNext,
    Font,
    FontTable,
    Read,
    Out,
    Random,
    Str,
    /// A line of the source, relative to the call's start: the input
    /// level, the codes it was tokenized under, and its rank after the
    /// line the call started on ([`src_slot`]); its version is the line's
    /// tokens (7.17.4).
    Source,
    /// A line number, relative to the call's start: the input level and
    /// the number less that level's line at the start ([`line_slot`]).
    Line,
    /// Where a control sequence's name is in the hash table (an interned
    /// name).
    Name,
    /// A file loaded by name (an interned name): its contents.
    Load,
    /// web2c's `search_string` for a string's characters (an interned
    /// name): the string it found.
    Search,
    /// String `n` of the pool, by number: its bytes (`track::Row::Str`).
    Pool,
    /// An allocator: 0 the pool's end (`str_ptr`), 1 the hash's
    /// (`hash_used`), 2 its extra area's (`hash_high`), each versioned by
    /// its value; 3 the conditionals' state and 4 the current marks, by
    /// their contents.
    Alloc,
    /// A read the recorder cannot relate to the call's start: never equal
    /// across builds.
    Unknown,
    /// A field of `cur_list` (`track::list`), the nest (slot
    /// `track::list::COUNT`), and the alignment state's fields after it
    /// (`track::align`): versioned by the values themselves.
    List,
    /// The save stack (`track::save`): its pointers, e-TeX's chain, and
    /// its entries, versioned by the values themselves.
    Save,
    /// Hyphenation (`track::hyph`): the patterns and the exception table
    /// as a whole, versioned at the write.
    Hyph,
    /// The glyphs PDF ship `n` used (`track::Row::Glyphs`): an append.
    Glyphs,
    /// An exception's word (an interned `hyph::exception_key`): what the
    /// exception table holds for it, its positions or none (§930).
    HyphWord,
    /// The PDF writer's tables (`pdf::val::field`), each a value carrying
    /// its version.
    Pdf,
    /// The DVI writer's tables (`pdf::val::dvi_field`).
    Dvi,
    /// A field of the page builder's state, or `\\splitdiscards`
    /// (`track::page`): versioned by the values themselves.
    Page,
    /// The conditionals (§489): the persistent stack with `if_limit`,
    /// `cur_if` and `if_line`.
    Cond,
    /// Current mark `t` of class `c`, at slot `5c + t` (§382).
    Mark,
    /// The page's `k`-th node (`track::Row::PageNode`): an append,
    /// versioned by the node.
    PageNode,
}

/// Codes id of a [`Fam::Source`] line versioned by its bytes.
const BYTES: u32 = 0;
/// Codes id of a [`Fam::Source`] line past the end of its file.
const EOF: u32 = 0xff_ffff;

/// The address of line `k` of input level `level` under codes `codes`.
fn src_slot(level: usize, codes: u32, k: u32) -> Slot {
    let level = i64::try_from(level.min(255)).unwrap_or(255);
    Slot(
        Fam::Source,
        level << 56 | i64::from(codes & 0xff_ffff) << 32 | i64::from(k),
    )
}

/// The address of a line number `d` after level `level`'s at the start.
fn line_slot(level: usize, d: i32) -> Slot {
    let level = i64::try_from(level.min(255)).unwrap_or(255);
    Slot(Fam::Line, level << 32 | i64::from(d.cast_unsigned()))
}

/// The low 32 bits of `x`, as the `i32` [`line_slot`] packed.
fn low32(x: i64) -> i32 {
    i32::try_from(x << 32 >> 32).unwrap_or(0)
}

/// An address: a family and a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot(pub Fam, pub i64);

impl Slot {
    fn row(r: Row) -> Slot {
        match r {
            Row::Str(n) => Slot(Fam::Pool, i64::try_from(n).unwrap_or(i64::MAX)),
            Row::Cond => Slot(Fam::Cond, 0),
            Row::Mark(k) => Slot(Fam::Mark, i64::from(k)),
            Row::Scalar(k) => Slot(Fam::Alloc, i64::from(k)),
            Row::List(f) => Slot(Fam::List, i64::from(f)),
            Row::Nest => Slot(Fam::List, i64::from(crate::track::list::COUNT)),
            Row::Align(f) => Slot(Fam::List, i64::from(crate::track::list::COUNT + 1 + f)),
            Row::Page(f) => Slot(Fam::Page, i64::from(f)),
            Row::Save(k) => Slot(Fam::Save, i64::from(k)),
            Row::Font(k) => Slot(Fam::Font, i64::from(k)),
            Row::FontTable => Slot(Fam::FontTable, 0),
            Row::Hyph(k) => Slot(Fam::Hyph, i64::from(k)),
            Row::Pdf(f) => Slot(Fam::Pdf, i64::from(f)),
            Row::Dvi(f) => Slot(Fam::Dvi, i64::from(f)),
            Row::Glyphs(n) => Slot(Fam::Glyphs, i64::from(n)),
            Row::PageNode(k) => Slot(Fam::PageNode, i64::from(k)),
            Row::Out(n) => Slot(Fam::Out, i64::from(n)),
            Row::Read(n) => Slot(Fam::Read, i64::from(n)),
            Row::Random => Slot(Fam::Random, 0),
            // (versioned by the answer read; a probe's is never equal)
            Row::Clock => Slot(Fam::Unknown, 1 << 20),
        }
    }

    fn of(c: Cell) -> Slot {
        let (f, i) = match c {
            Cell::Eqtb(p) => (Fam::Eqtb, p),
            Cell::Hash(p) => (Fam::Hash, p),
            Cell::HashNext(p) => (Fam::HashNext, p),
            Cell::Font(f) => (Fam::Font, f),
            Cell::FontTable => (Fam::FontTable, 0),
            Cell::Read(n) => (Fam::Read, n),
            Cell::Out(n) => (Fam::Out, n),
            Cell::Random => (Fam::Random, 0),
            Cell::Str(h) => (Fam::Str, h),
        };
        Slot(f, i64::from(i))
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fam = match self.0 {
            Fam::Eqtb => "eqtb",
            Fam::Hash => "hash",
            Fam::HashNext => "hashnext",
            Fam::Font => "font",
            Fam::FontTable => "fonttable",
            Fam::Read => "read",
            Fam::Out => "out",
            Fam::Random => "random",
            Fam::Str => "str",
            Fam::Source => {
                let (l, c, k) = (self.1 >> 56, self.1 >> 32 & 0xff_ffff, self.1 & 0xffff_ffff);
                return write!(f, "source:{l}.{k}/{c}");
            }
            Fam::Line => {
                return write!(f, "line:{}{:+}", self.1 >> 32, low32(self.1));
            }
            Fam::Name => "name",
            Fam::Load => "load",
            Fam::Search => "search",
            Fam::Pool => "pool",
            Fam::Alloc => "alloc",
            Fam::Unknown => "unknown",
            Fam::List => "list",
            Fam::Save => "save",
            Fam::Hyph => "hyph",
            Fam::HyphWord => "hyphword",
            Fam::Pdf => "pdf",
            Fam::Dvi => "dvi",
            Fam::Page => "page",
            Fam::Cond => "cond",
            Fam::Mark => "mark",
            Fam::Glyphs => "glyphs",
            Fam::PageNode => "pagenode",
        };
        write!(f, "{fam}:{}", self.1)
    }
}

/// A value as the runtime keeps it: its version and, for a record's
/// write or a call's result, the value itself, shared (DESIGN 7.17.2: a
/// hit's writes are put in place; 7.17.12: a pointer store per written
/// field, the value shared, never copied or thawed).
#[derive(Clone, Debug)]
pub struct SVal(pub Version, pub(crate) Option<alloc::sync::Arc<SValue>>);

impl PartialEq for SVal {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}

impl Eq for SVal {}

impl SVal {
    /// A value known by its version.
    #[must_use]
    pub fn ver(v: Version) -> Self {
        SVal(v, None)
    }

    /// Value `x`, versioned `v`, held.
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "an engine lives on one thread; the value is an Arc for sharing"
    )]
    pub(crate) fn held(v: Version, x: SValue) -> Self {
        SVal(v, Some(alloc::sync::Arc::new(x)))
    }
}

/// The value a slot holds, by family (`SVal`'s): what `Store::set` puts
/// back.
#[derive(Debug)]
pub(crate) enum SValue {
    /// An eqtb entry or a register above 255: the word, its level where
    /// `xeq_level` keeps it, the value it holds.
    Word {
        w: crate::mem::MemoryWord,
        level: Option<i32>,
        obj: Option<crate::objs::Obj>,
    },
    /// A hash slot's text or link, a scalar slot, a scalar field.
    Int(i32),
    /// A string of the pool.
    Bytes(alloc::sync::Arc<[u8]>),
    /// `cur_list`'s fields.
    Nodes(partex_engine::nodelist::NodeList),
    Mlist(Vec<partex_engine::math::Item>),
    Noad(Option<alloc::boxed::Box<partex_engine::math::Noad>>),
    LrSave(Vec<u8>),
    LrBox(Option<partex_engine::node::BoxNode>),
    /// The enclosing levels of the nest.
    Nest(Vec<crate::nest::ListStateRecord>),
    /// A save stack entry.
    Save {
        w: crate::mem::MemoryWord,
        obj: Option<crate::objs::Obj>,
        eqtb: bool,
    },
    /// e-TeX's chain of saved registers above 255.
    XChain {
        level: i32,
        chain: Vec<crate::xregs::Saved>,
        outer: Vec<Vec<crate::xregs::Saved>>,
    },
    /// The conditionals.
    Cond {
        stack: crate::conds::CondStack,
        limit: i32,
        cur_if: i32,
        line: i32,
    },
    /// A current mark.
    Mark(Option<partex_engine::node::Tokens>),
    /// Where a paragraph call ended in the source (its result).
    Pos(Position),
    /// A field of the families whose fields are the engine's structures
    /// (`values.rs`: the page builder, the alignment, the writers, the
    /// streams, the random generator).
    Field(crate::values::Field),
}

/// Where a paragraph call ended, relative to where it began (DESIGN
/// 7.17.12: the input position is the call's result).
#[derive(Clone, Debug)]
pub(crate) struct Position {
    /// Per input level open at the start: the lines read after its line
    /// then, and whether it ended.
    pub(crate) levels: Vec<(u32, bool)>,
    /// Which of those levels were files (the others, the terminal's and
    /// `\\scantokens`', read nothing a position counts).
    pub(crate) files: Vec<bool>,
    /// The input depth (`in_open`, `input_ptr`) at the start and the end,
    /// and whether the call opened a file.
    pub(crate) depth: (usize, usize, usize, usize),
    pub(crate) opened: bool,
    /// At the end: the current level a file's, its line in the buffer
    /// (`start..=limit`), `loc` and `limit` from `start`, and the scanner
    /// state.
    pub(crate) file: bool,
    pub(crate) line: Vec<u8>,
    pub(crate) loc: i32,
    pub(crate) limit: i32,
    pub(crate) state: i32,
}

impl Position {
    /// Whether a hit can set the input to this position: the lines the
    /// call read are its own level's, and the levels are the ones it began
    /// with (a file opened or ended inside the call, and a token list left
    /// above the file, need the input state as a value: the next rows).
    pub(crate) fn settable(&self) -> bool {
        let (d0, p0, d1, p1) = self.depth;
        let top = self.levels.len().saturating_sub(1);
        self.file
            && !self.opened
            && d0 == d1
            && p0 == p1
            && self.levels.iter().enumerate().all(|(j, &(k, closed))| {
                !self.files.get(j).copied().unwrap_or(false) || (!closed && (j == top || k == 0))
            })
            && self.files.get(top).copied().unwrap_or(false)
    }
}

impl Value for SVal {
    fn version(&self) -> Version {
        self.0
    }
}

/// The engine's functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Func {
    /// The job's start (§1332–§1337: the format's load, the first line),
    /// to the first clean point: the fold's first step (7.17.9).
    Start,
    /// A step of the fold: from one clean point to the next (7.17.9),
    /// named by the tokens of the line it begins on and the offset in them.
    Step,
    /// The tokenizer over the line a step begins on (§343–§356), named by
    /// its bytes; its result is the line's tokens under the codes it read.
    Tokenize,
    /// §649: `hpack`, named by the list, the spec, whether material
    /// migrates, pdfTeX's expansion of a line's fonts, and the kind of
    /// context its report names.
    Hpack,
    /// §668: `vpack`, named by the list, the spec, the depth limit and
    /// the kind of context its report names.
    Vpack,
    /// §815: `line_break`, named by the paragraph's list and whether a
    /// display interrupts it; its children are the lines' `hpack`s
    /// (§889).
    LineBreak,
    /// §638: `ship_out`, named by the box; its writes are the DVI and
    /// PDF writers' tables and its effects the page's bytes (its scope is
    /// the call, `pdf::val`).
    ShipOut,
    /// §1370: `write_out`, named by the `\write` node's stream and
    /// tokens; its effects are the line's bytes and its store (7.17.5).
    WriteOut,
    /// §994: a step of `build_page`, one contribution (§996–§1008, and
    /// the fire, §1012–§1022, when the page is complete), named by the
    /// page so far, the node and, for a kern, what follows it.
    PageStep,
    /// §1012's end: the output routine, the user's (§1025 to §1026) or
    /// the default (§1023), named by `\box255`, the insertion boxes the
    /// fire filled and `\outputpenalty`.
    Output,
    /// pdfTeX's `write_fontdescriptor` (writefont.c) at the job's end:
    /// a font descriptor with its font file stream, named by its tree's
    /// key and the entry.
    FontFile,
    /// `write_enc`: an encoding's differences, named by its tree's key
    /// and the entry.
    Encoding,
    /// `write_fontdictionary`: a font dictionary with its `ToUnicode`
    /// map, named by its tree's key and the entry.
    FontDict,
}

impl Func {
    /// How many routines there are.
    pub const COUNT: usize = 13;
    /// The routines by index ([`SsaReport::routines`]).
    pub const ALL: [Func; Func::COUNT] = [
        Func::Start,
        Func::Step,
        Func::Tokenize,
        Func::Hpack,
        Func::Vpack,
        Func::LineBreak,
        Func::ShipOut,
        Func::WriteOut,
        Func::PageStep,
        Func::Output,
        Func::FontFile,
        Func::Encoding,
        Func::FontDict,
    ];

    /// Whether a hit of the routine is applied (DESIGN 7.17.3, "Hits
    /// applied inside a step that runs again", item 4): its record holds
    /// everything a call makes, with nothing handed back to its caller
    /// outside its writes and effects.
    #[must_use]
    pub const fn applies(self) -> bool {
        matches!(self, Func::FontFile | Func::Encoding | Func::FontDict)
    }

    /// Whether a call of the routine has a frame and a record outside
    /// check mode (DESIGN 4.3 item 2): the steps, and the pure
    /// typesetting calls that apply or are to apply (`line_break`, the
    /// packs, the page steps, `ship_out`, the fonts). The others
    /// (`tokenize`, `write_out`, the output routine) run in the frame of
    /// the call around them, their reads and writes its own; in check
    /// mode, and with `PARTEX_SSA_LEAN=0`, every routine has its record.
    #[must_use]
    pub const fn recorded(self) -> bool {
        !matches!(self, Func::Tokenize | Func::WriteOut | Func::Output)
    }
}

/// A routine's counts ([`SsaReport::routines`]): its calls and the hits
/// among them in a build, and the records made for it so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoutineCount {
    pub calls: u64,
    pub hits: u64,
    pub records: u64,
}

impl fmt::Display for Func {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Func::Start => "start",
            Func::Step => "step",
            Func::Tokenize => "tokenize",
            Func::Hpack => "hpack",
            Func::Vpack => "vpack",
            Func::LineBreak => "line_break",
            Func::ShipOut => "ship_out",
            Func::WriteOut => "write_out",
            Func::PageStep => "page_step",
            Func::Output => "output",
            Func::FontFile => "font_file",
            Func::Encoding => "encoding",
            Func::FontDict => "font_dict",
        })
    }
}

/// An effect (7.17.2: what the call logged, printed or wrote).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    /// The bytes the call's own body made for one of the engine's
    /// outputs (the log, the terminal, a `\write` file, the DVI or PDF
    /// file), or a page given to the host's page sink, named by the
    /// output (`Tracker::output`): what check mode compares.
    Bytes(Output, Vec<u8>),
    /// A step's effects in the link's form (§7.6), gathered at the step's
    /// end: what the build's files are linked from (DESIGN 7.17.3).
    Step(StepEffects),
}

/// A step's effects, shared, with their version (made once, from their
/// saved form).
#[derive(Clone, Debug)]
pub struct StepEffects(pub Version, pub alloc::sync::Arc<[crate::effects::Effect]>);

impl StepEffects {
    fn new(fx: Vec<crate::effects::Effect>) -> Self {
        let mut s = partex_engine::persist::Saver::default();
        partex_engine::persist::Persist::save(&fx, &mut s);
        StepEffects(Version::of(&s.into_bytes()), fx.into())
    }
}

impl PartialEq for StepEffects {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}

impl Eq for StepEffects {}

impl core::hash::Hash for StepEffects {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        self.0.hash(h);
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::Bytes(o, b) => write!(f, "{o} {:?}", String::from_utf8_lossy(b)),
            Effect::Step(e) => write!(f, "step {} effects {}", e.1.len(), e.0),
        }
    }
}

/// The engine as a [`Machine`]: its bodies are the engine's own code,
/// run between [`Runtime::begin`] and [`Runtime::end`], so `run` is
/// never called.
#[derive(Clone, Copy, Debug, Default)]
pub struct TexSsa;

impl Machine for TexSsa {
    type Addr = Slot;
    type Val = SVal;
    type Effect = Effect;
    type Func = Func;

    fn run<C: Cx<Self>>(&self, _f: Func, _args: &[SVal], _cx: &mut C) -> SVal {
        SVal::ver(Version::ABSENT)
    }

    fn dense(a: &Slot) -> Option<(usize, usize)> {
        let f = match a.0 {
            // (each table a family of its own: two tables in one would
            // share their slots' stamps, and a write of one slot would
            // hide the first read of the other from its call)
            Fam::Eqtb | Fam::Pool | Fam::Alloc | Fam::Pdf | Fam::Dvi | Fam::Glyphs => {
                return table_at(a.0, a.1).map(|(f, i)| {
                    (
                        match f {
                            0 => 0,
                            3 => 6,
                            4 => 7,
                            5 => 8,
                            9 => 13,
                            10 => 14,
                            _ => 15,
                        },
                        i,
                    )
                });
            }
            Fam::Hash => 1,
            Fam::HashNext => 2,
            Fam::Font => 3,
            Fam::Read => 4,
            Fam::Out => 5,
            Fam::List => 9,
            Fam::Save => 10,
            Fam::FontTable => 11,
            Fam::Hyph => 12,
            _ => return None,
        };
        usize::try_from(a.1)
            .ok()
            .filter(|&i| i < 1 << 24)
            .map(|i| (f, i))
    }
}

/// A slot's counter in [`RecState::verified`] and [`RecState::noted`]:
/// its family, with the allocators' slots each apart.
#[must_use]
pub fn count_ix(s: Slot) -> usize {
    if s.0 == Fam::Alloc {
        return 18 + usize::try_from(s.1).unwrap_or(9).min(9);
    }
    (s.0 as usize).min(17)
}

/// A counter's name ([`count_ix`]).
#[must_use]
pub fn count_name(i: usize) -> alloc::string::String {
    const ALLOC: [&str; 10] = [
        "str_ptr",
        "hash_used",
        "hash_high",
        "cond",
        "marks",
        "last_badness",
        "output_active",
        "term_offset",
        "file_offset",
        "alloc?",
    ];
    const FAMS: [Fam; 18] = [
        Fam::Eqtb,
        Fam::Hash,
        Fam::HashNext,
        Fam::Font,
        Fam::FontTable,
        Fam::Read,
        Fam::Out,
        Fam::Random,
        Fam::Str,
        Fam::Source,
        Fam::Line,
        Fam::Name,
        Fam::Load,
        Fam::Search,
        Fam::Pool,
        Fam::Alloc,
        Fam::Unknown,
        Fam::List,
    ];
    if i >= 18 {
        return alloc::format!("alloc:{}", ALLOC[(i - 18).min(9)]);
    }
    alloc::format!(
        "{:?}",
        FAMS.iter()
            .find(|f| **f as usize == i)
            .copied()
            .unwrap_or(Fam::Unknown)
    )
}

/// A table family's array and a slot's index in it: eqtb (0) below the
/// registers above 255, the registers above 255 (3, by their offset from
/// `EXT_BASE`), `text` (1), `next` (2), the pool's strings (4), the
/// allocators (5), the fonts' fields (6), the font table (7),
/// hyphenation (8), the PDF (9) and DVI (10) writers' tables, the
/// streams' files (11, the log's at `streams::LOG`), the `\openin`
/// streams (12) and the random generator (13).
/// The conditionals' stamp, after the list family's slots (`Slot::row`).
const COND_STAMP: usize =
    crate::track::list::COUNT as usize + 1 + crate::track::align::COUNT as usize;

#[inline]
fn table_at(f: Fam, a: i64) -> Option<(usize, usize)> {
    let ext = i64::from(crate::xregs::EXT_BASE);
    let (t, i) = match f {
        Fam::Eqtb if a >= ext => (3, usize::try_from(a - ext).ok()?),
        Fam::Eqtb => (0, usize::try_from(a).ok()?),
        Fam::Hash => (1, usize::try_from(a).ok()?),
        Fam::HashNext => (2, usize::try_from(a).ok()?),
        Fam::Pool => (4, usize::try_from(a).ok()?),
        Fam::Alloc => (5, usize::try_from(a).ok()?),
        Fam::Font => (6, usize::try_from(a).ok()?),
        Fam::FontTable => (7, usize::try_from(a).ok()?),
        Fam::Hyph => (8, usize::try_from(a).ok()?),
        Fam::Pdf => (9, usize::try_from(a).ok()?),
        Fam::Dvi => (10, usize::try_from(a).ok()?),
        Fam::Glyphs => (14, usize::try_from(a).ok()?),
        Fam::Out => (11, usize::try_from(a).ok()?),
        Fam::Read => (12, usize::try_from(a).ok()?),
        Fam::Random => (13, usize::try_from(a).ok()?),
        _ => return None,
    };
    (i < 1 << 24).then_some((t, i))
}

/// The versions of the slots.
///
/// The tables (eqtb with `xeq_level` and the registers above 255, the
/// hash's `text` and `next`, the pool, the scalar slots, the fonts' fields
/// and their table, hyphenation) keep a version array beside their entries:
/// the writing accessor stores the value and then gives its content
/// version ([`Tracker::wrote`]), so an equal write leaves the version
/// (backdating) and a read is an index (DESIGN 7.17.12). A writer that
/// stores a table wholesale (the engine as made, a format's load) versions
/// it whole (`Tex::version_tables`). The other families (the streams, the
/// random generator) keep revisions tagged with the build.
#[derive(Default)]
pub struct Versions {
    /// Content versions of the table slots, by [`table_at`] (0: not
    /// versioned yet).
    table: [Vec<u128>; 15],
    /// Revisions of the other families' slots.
    revs: BTreeMap<Slot, u64>,
    rev: u64,
    /// The build, which tags revisions.
    epoch: u64,
}

impl Versions {
    fn revision(&self, s: Slot) -> Version {
        let r = self.revs.get(&s).copied().unwrap_or(0);
        Version((u128::from(self.epoch) << 64) | u128::from(r) | (1 << 127))
    }

    /// A table slot's version, if its array has one.
    fn known(&self, s: Slot) -> Option<Version> {
        let (f, i) = table_at(s.0, s.1)?;
        self.known_at(f, i)
    }

    /// [`Versions::known`] of the slot at index `i` of table `f`.
    #[inline]
    fn known_at(&self, f: usize, i: usize) -> Option<Version> {
        let v = *self.table[f].get(i)?;
        (v != 0).then_some(Version(v))
    }

    /// A write of `s`: a table slot's version comes with [`Versions::set`]
    /// once stored; another family's revision moves.
    fn wrote(&mut self, s: Slot) {
        if table_at(s.0, s.1).is_none() {
            self.rev += 1;
            self.revs.insert(s, self.rev);
        }
    }

    /// Table slot `s` holds content `v` (made at its write).
    fn set(&mut self, s: Slot, v: u128) {
        let Some((f, i)) = table_at(s.0, s.1) else {
            return;
        };
        let t = &mut self.table[f];
        if i >= t.len() {
            t.resize((i + 1).next_power_of_two(), 0);
        }
        t[i] = v | 1;
    }
}

/// An input level open when the call began.
struct Level {
    /// The file's data (its address), and where its next line begins.
    data: usize,
    next: usize,
    /// Lines read since the start, and the one in the buffer (its rank and
    /// start), if any.
    k: u32,
    cur: Option<(u32, usize)>,
    closed: bool,
    /// The level's line number at the start.
    line: i32,
}

/// The source as the open call reads it (7.17.4): relative to its start.
#[derive(Default)]
struct Src {
    /// By input level (0 unused).
    levels: Vec<Level>,
    /// Files the call opened (their data's addresses): their lines are
    /// the load's.
    opened: Vec<usize>,
    /// The catcode generation at the start.
    generation: u64,
    /// The input stack's depth at the start.
    input_ptr: usize,
}

/// Where the line of `data` that begins at `from` ends (without its
/// trailing spaces) and where the next begins (`file_lines`' rule).
fn line_bounds(data: &[u8], from: usize) -> (usize, usize) {
    let mut i = from;
    while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
        i += 1;
    }
    let mut end = i;
    while end > from && data[end - 1] == b' ' {
        end -= 1;
    }
    if i < data.len() {
        if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
            i += 1;
        }
        i += 1;
    }
    (end, i)
}

/// A line's version under `codes`: its tokens, or (`None`, the tokenizer
/// would read it otherwise) a version no tokens have.
fn tokens_version(line: &[u8], codes: &LineCodes) -> Version {
    match line_tokens(line, codes) {
        Some(t) => Version::of(&(1u8, t)),
        None => Version::of(&(2u8, line)),
    }
}

fn bytes_version(line: &[u8]) -> Version {
    Version::of(&(0u8, line))
}

/// The recorder's state apart from the runtime: what a [`View`] reads.
#[derive(Default)]
pub struct RecState {
    pub vers: Versions,
    /// The catcode generation: bumped by each write that changes how lines
    /// tokenize.
    generation: u64,
    /// The open call's source, if a call relative to the source is open.
    src: Option<Src>,
    /// Interned: the codes lines were tokenized under (index 0 unused), the
    /// control sequence names looked up, the files loaded (with their
    /// contents' version now, and the kind of file each was looked up
    /// as, so it is looked up again the same way).
    codes: Vec<LineCodes>,
    codes_ix: BTreeMap<u128, u32>,
    names: Vec<Vec<u8>>,
    names_ix: BTreeMap<Vec<u8>, u32>,
    loads: Vec<(Vec<u8>, Version, crate::host::FileKind)>,
    loads_ix: BTreeMap<Vec<u8>, u32>,
    searches: Vec<Vec<u8>>,
    searches_ix: BTreeMap<Vec<u8>, u32>,
    /// The exceptions' words looked up or entered ([`Fam::HyphWord`]).
    words: Vec<Vec<u8>>,
    words_ix: BTreeMap<Vec<u8>, u32>,
    /// The calls opened through `Tracker::call_begin` by routine
    /// ([`Func::ALL`]'s order): this build's calls and hits, and the
    /// records made.
    pub routines: [RoutineCount; Func::COUNT],
    /// Check mode: reads whose version from a table's array differed from
    /// the slot's content (a write that bypassed its accessor), by family.
    pub stale: BTreeMap<Fam, u64>,
    /// The first slots read stale.
    pub stale_first: Vec<Slot>,

    /// Reads verified by family (the lookups' comparisons: `View::version`
    /// calls), and reads noted by family, with the allocators' slots (the
    /// pool's end, the hash's, the conditionals, the marks) apart: counts
    /// for the report, not a mechanism.
    pub verified: [core::cell::Cell<u64>; 28],
    pub noted: [u64; 28],
    /// Files this build opened for writing: a load of one reads what the
    /// build wrote (the φ, not built), never equal across builds.
    written: alloc::collections::BTreeSet<Vec<u8>>,
    /// Output made since the last call boundary, a run per output:
    /// effects of the call open now (`Recorder::flush_output`).
    pending: Vec<(Output, Vec<u8>)>,
    /// Check mode: the rows a hit's record and its body wrote differently
    /// (with the count and the first call), the first slots, and the hits
    /// checked.
    pub check_diffs: BTreeMap<&'static str, (u64, String)>,
    pub write_diffs: Vec<String>,
    pub calls_checked: u64,
    /// The fold's steps as a rebuild finds them (DESIGN 7.17.3): the
    /// files and the lines each step read, where each left the input.
    pub(crate) steps: rebuild::Steps,
    /// The commands each step's last run ran, by step id: the steps'
    /// costs in the dependency graph ([`dag`]).
    pub step_commands: Vec<u64>,
}

/// Record `id`'s effects and stores, its children's included, in program
/// order: each one's version and a short view of it (check mode).
fn outputs(rt: &Runtime<TexSsa>, id: RecId) -> Vec<(Version, String)> {
    fn walk(rt: &Runtime<TexSsa>, id: RecId, out: &mut Vec<(Version, String)>) {
        for it in &rt.record(id).items {
            match it {
                partex_ssa::runtime::Item::Out(e) => {
                    let view: String = alloc::format!("{e}").chars().take(60).collect();
                    out.push((Version::of(e), view));
                }
                partex_ssa::runtime::Item::Open(a) => {
                    out.push((Version::of(&(1u8, a)), alloc::format!("open {a}")));
                }
                partex_ssa::runtime::Item::Store(a, v) => {
                    out.push((Version::of(&(2u8, a, v.0)), alloc::format!("store {a}")));
                }
                partex_ssa::runtime::Item::Call(c) => walk(rt, *c, out),
                partex_ssa::runtime::Item::Wrote(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(rt, id, &mut out);
    out
}

impl RecState {
    /// The exception's word `key` as an address ([`Fam::HyphWord`]).
    fn word(&mut self, key: &[u8]) -> u32 {
        let (id, fresh) = Recorder::intern(&mut self.words_ix, self.words.len(), key);
        if fresh {
            self.words.push(key.to_vec());
        }
        id
    }

    /// Check mode's test of a hit: the writes of its record `hit` against
    /// those of the body that ran, `body`, slot by slot (what an applied
    /// hit would store is what the body stored).
    fn compare_writes(
        &mut self,
        rt: &Runtime<TexSsa>,
        hit: RecId,
        body: RecId,
        name: &str,
    ) -> bool {
        self.calls_checked += 1;
        // (the effects a hit emits again against those the body made)
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        record_effects(rt, hit, &mut ea);
        record_effects(rt, body, &mut eb);
        // (by output, each one's bytes joined: runs may split apart at
        // different boundaries)
        let joined = |v: &[Effect], k: Output| -> Vec<u8> {
            v.iter()
                .filter_map(|e| match e {
                    Effect::Bytes(o, b) if *o == k => Some(b.iter().copied()),
                    _ => None,
                })
                .flatten()
                .collect()
        };
        let kinds: alloc::collections::BTreeSet<Output> = ea
            .iter()
            .chain(&eb)
            .filter_map(|e| match e {
                Effect::Bytes(o, _) => Some(*o),
                Effect::Step(_) => None,
            })
            .collect();
        if kinds.iter().any(|&k| joined(&ea, k) != joined(&eb, k)) {
            let e = self
                .check_diffs
                .entry("effects: output bytes")
                .or_insert((0, String::from(name)));
            e.0 += 1;
        }
        let map =
            |id| -> BTreeMap<Slot, Option<SVal>> { rt.record(id).writes.iter().cloned().collect() };
        let (a, b) = (map(hit), map(body));
        let mut fams: alloc::collections::BTreeSet<&'static str> =
            alloc::collections::BTreeSet::new();
        for (s, v) in &a {
            if b.get(s) != Some(v) {
                fams.insert(fam_row(s.0));
                if self.write_diffs.len() < 12 {
                    self.write_diffs.push(alloc::format!(
                        "{name}: {s} hit {:?} body {:?}",
                        v.as_ref().map(|x| x.0),
                        b.get(s).map(|x| x.as_ref().map(|y| y.0))
                    ));
                }
            }
        }
        for (s, v) in &b {
            if !a.contains_key(s) {
                fams.insert(fam_row(s.0));
                if self.write_diffs.len() < 12 {
                    self.write_diffs.push(alloc::format!(
                        "{name}: {s} hit none body {:?}",
                        v.as_ref().map(|x| x.0)
                    ));
                }
            }
        }
        // (the effects and stores, in program order: what a hit emits
        // again, 7.17.12)
        let (ea, eb) = (outputs(rt, hit), outputs(rt, body));
        if ea != eb {
            fams.insert("effects");
            if self.write_diffs.len() < 12 {
                let at = ea.iter().zip(&eb).take_while(|(x, y)| x == y).count();
                self.write_diffs.push(alloc::format!(
                    "{name}: effects differ at {at} of {} and {}: hit {} body {}",
                    ea.len(),
                    eb.len(),
                    ea.get(at).map_or_else(String::new, |x| x.1.clone()),
                    eb.get(at).map_or_else(String::new, |x| x.1.clone()),
                ));
            }
        }
        if !fams.is_empty() && self.write_diffs.len() < 14 {
            let show = |id| {
                let r = rt.record(id);
                let reads: Vec<String> = r
                    .reads
                    .iter()
                    .filter(|(l, _)| !matches!(l.addr().0, Fam::Eqtb))
                    .take(40)
                    .map(|(l, v)| alloc::format!("{l}={:x}", v.0 & 0xffff))
                    .collect();
                alloc::format!("{} reads: {}", r.reads.len(), reads.join(" "))
            };
            self.write_diffs.push(alloc::format!("hit {}", show(hit)));
            self.write_diffs.push(alloc::format!("body {}", show(body)));
        }
        let same = fams.is_empty();
        for f in fams {
            let e = self.check_diffs.entry(f).or_insert((0, String::from(name)));
            e.0 += 1;
        }
        same
    }
}

/// The recorder: the runtime and the slots' versions.
pub struct Recorder {
    pub rt: Runtime<TexSsa>,
    pub st: RecState,
    /// Whether the engine's reads and writes are being recorded.
    on: bool,
}

impl Recorder {
    #[must_use]
    pub fn new() -> Self {
        let mut rt = Runtime::new(Config::default());
        rt.set_default_value(SVal::ver(Version::ABSENT));
        Recorder {
            rt,
            st: RecState {
                codes: alloc::vec![LineCodes {
                    cat: [0; 256],
                    end_line_char: -1
                }],
                ..RecState::default()
            },
            on: false,
        }
    }

    /// The output made since the last call boundary, noted as the open
    /// call's effects (before a call begins or ends).
    fn flush_output(&mut self) {
        if self.st.pending.is_empty() {
            return;
        }
        let pending = core::mem::take(&mut self.st.pending);
        if !self.on {
            return;
        }
        for (what, bytes) in pending {
            self.rt.note_effect(Effect::Bytes(what, bytes));
        }
    }

    fn intern_codes(&mut self, c: &LineCodes) -> u32 {
        let key = Version::of(c).0;
        if let Some(&i) = self.st.codes_ix.get(&key) {
            return i;
        }
        let i = u32::try_from(self.st.codes.len())
            .unwrap_or(EOF - 1)
            .min(EOF - 1);
        self.st.codes.push(*c);
        self.st.codes_ix.insert(key, i);
        i
    }

    fn intern(ix: &mut BTreeMap<Vec<u8>, u32>, len: usize, name: &[u8]) -> (u32, bool) {
        if let Some(&i) = ix.get(name) {
            return (i, false);
        }
        let i = u32::try_from(len).unwrap_or(u32::MAX);
        ix.insert(name.to_vec(), i);
        (i, true)
    }

    fn note(&mut self, s: Slot, v: Version) {
        self.st.noted[count_ix(s)] += 1;
        self.rt.note_read(&Loc::State(s), v);
    }

    /// A line of level `j` done: its read, by its tokens under `codes` (if
    /// they held through the line) or its bytes.
    fn line_done(&mut self, j: usize, k: u32, line: &[u8], codes: Option<&LineCodes>) {
        if k == 0 {
            // (the call's name holds the line's tokens under the codes at
            // the start; if they changed while it was read, its bytes)
            if codes.is_none() {
                self.note(src_slot(j, BYTES, 0), bytes_version(line));
            }
            return;
        }
        match codes {
            Some(c) => {
                let id = self.intern_codes(c);
                let v = tokens_version(line, &self.st.codes[id as usize]);
                self.note(src_slot(j, id, k), v);
            }
            None => self.note(src_slot(j, BYTES, k), bytes_version(line)),
        }
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

/// The engine's tracker that records into the runtime.
///
/// A read of a table slot is noted once per call: the slot's stamp is the
/// call generation it was last noted in, and the generation moves at every
/// call boundary ([`SsaTracker::boundary`]), so a repeated read in the same
/// call is a compare of two integers, with no borrow of the recorder and no
/// map (the runtime would drop it anyway: 7.17.2's first read).
pub struct SsaTracker {
    pub rec: RefCell<Recorder>,
    generation: core::cell::Cell<u32>,
    stamps: [Vec<core::cell::Cell<u32>>; 15],
    /// The allocators' and scalars' write stamps (a write noted once per
    /// call).
    wstamps: [core::cell::Cell<u32>; 96],
    /// The structure rows' read and write stamps (a read or a write noted
    /// once per call).
    vstamps: [core::cell::Cell<u32>; 32],
    vwstamps: [core::cell::Cell<u32>; 32],
    /// The page family's (`track::page`).
    pstamps: [core::cell::Cell<u32>; 32],
    pwstamps: [core::cell::Cell<u32>; 32],
    /// The calls the engine opened (`Tracker::call_begin`), innermost
    /// last: the record a hit found, the call's name for reports, and
    /// whether it has a frame ([`Func::recorded`]).
    calls: RefCell<Vec<(Option<RecId>, Func, bool)>>,
    /// Bytes printed since the last call boundary (log 0, terminal 1): the
    /// running call's effect, noted at the next boundary; whether they are
    /// kept (the recorder is on).
    /// Hits of the routines that apply them ([`Func::applies`]) are put
    /// in place: the body of a hit does not run. On in a rebuild unless
    /// `PARTEX_SSA_APPLY=0`; in a cold build only with `PARTEX_SSA_APPLY=1`.
    pub(crate) apply: core::cell::Cell<bool>,
    /// Hits put in place, and the commands they stand for.
    pub applied: core::cell::Cell<u64>,
    pub skipped: core::cell::Cell<u64>,
    /// The save stack's (sized with the stack).
    sstamps: Vec<core::cell::Cell<u32>>,
    swstamps: Vec<core::cell::Cell<u32>>,
    /// Check mode: each table read's version compared with the content.
    check: bool,
    /// Records only for the steps and the routines [`Func::recorded`]
    /// names, and a step's record without its own reads (DESIGN 4.3 item
    /// 2; on by default). Off (`PARTEX_SSA_LEAN=0`), every routine has a
    /// record and every step keeps its reads, as in check mode, which
    /// does so either way.
    lean: bool,
    /// Writes whose version could not be stored (the recorder was busy).
    pub lost: core::cell::Cell<u64>,
    /// The steps' reads and writes timed by the engine's commands (a
    /// measurement for [`dag`], `PARTEX_SSA_DAG`; off by default).
    timed: bool,
}

impl SsaTracker {
    /// Time each step's reads and writes by the engine's commands
    /// (`partex_ssa::open::StepTimes`, printed by [`dag`]).
    pub fn set_timed(&mut self, on: bool) {
        self.timed = on;
        self.rec.get_mut().rt.set_timing(on);
    }

    /// Whether routine `f` gets a frame and a record ([`SsaTracker::lean`]).
    fn framed(&self, f: Func) -> bool {
        self.check || !self.lean || f.recorded()
    }

    /// Whether a step's frame keeps its own reads.
    fn step_keeps_reads(&self) -> bool {
        self.check || !self.lean
    }

    /// Record lean (the default) or, `false`, every routine and every
    /// step's reads ([`SsaTracker::lean`]).
    pub fn set_lean(&mut self, on: bool) {
        self.lean = on;
    }

    #[must_use]
    pub fn new(rec: Recorder) -> Self {
        SsaTracker {
            rec: RefCell::new(rec),
            generation: core::cell::Cell::new(1),
            stamps: Default::default(),
            wstamps: core::array::from_fn(|_| core::cell::Cell::new(0)),
            vstamps: core::array::from_fn(|_| core::cell::Cell::new(0)),
            vwstamps: core::array::from_fn(|_| core::cell::Cell::new(0)),
            pstamps: core::array::from_fn(|_| core::cell::Cell::new(0)),
            pwstamps: core::array::from_fn(|_| core::cell::Cell::new(0)),
            calls: RefCell::new(Vec::new()),
            apply: core::cell::Cell::new(false),
            applied: core::cell::Cell::new(0),
            skipped: core::cell::Cell::new(0),
            sstamps: Vec::new(),
            swstamps: Vec::new(),
            check: false,
            lean: true,
            lost: core::cell::Cell::new(0),
            timed: false,
        }
    }

    /// The output made since the last boundary, noted as effects of the
    /// call open now (before a call begins or ends, and before a hit is
    /// put in place).
    pub fn flush_effects(&self) {
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            r.flush_output();
        }
    }

    /// Whether a call is looked up by name before its body runs: only
    /// where a hit is used (put in place, or compared in check mode). A
    /// hit found and not used would cost its walk and save nothing.
    fn probing(&self, f: Func) -> bool {
        self.check || (self.apply.get() && f.applies())
    }

    /// A call began or ended: every slot is unread in the call now open.
    pub fn boundary(&self) {
        self.generation
            .set(self.generation.get().wrapping_add(1).max(1));
    }

    fn wstamp(&self, s: Slot) -> Option<&core::cell::Cell<u32>> {
        if s.0 != Fam::Alloc {
            return None;
        }
        self.wstamps.get(usize::try_from(s.1).ok()?)
    }

    /// A structure row's read (or write) stamp: the list family's slots
    /// (`cur_list`'s fields, the nest, the alignment's), then the
    /// conditionals' one slot after them; the page and the save stack
    /// have arrays of their own.
    fn value_stamp(&self, s: Slot, write: bool) -> Option<&core::cell::Cell<u32>> {
        let i = usize::try_from(s.1).ok()?;
        match (s.0, write) {
            (Fam::Save, false) => self.sstamps.get(i),
            (Fam::Save, true) => self.swstamps.get(i),
            (Fam::Page, false) => self.pstamps.get(i),
            (Fam::Page, true) => self.pwstamps.get(i),
            (Fam::List, false) => self.vstamps.get(i),
            (Fam::List, true) => self.vwstamps.get(i),
            (Fam::Cond, false) => self.vstamps.get(COND_STAMP),
            (Fam::Cond, true) => self.vwstamps.get(COND_STAMP),
            _ => None,
        }
    }

    #[inline]
    fn stamp(&self, s: Slot) -> Option<&core::cell::Cell<u32>> {
        let (f, i) = table_at(s.0, s.1)?;
        self.stamps[f].get(i)
    }

    /// End the engine's calls still open: the job ended inside them (an
    /// output routine a fatal error stopped).
    fn end_open_calls(&self, view: &dyn EngineView) {
        while !self.calls.borrow().is_empty() {
            crate::track::Tracker::call_end(self, view);
        }
    }
}

impl SsaTracker {
    /// A read of table slot `s`: once per call (its stamp), with the
    /// version its array holds; in check mode, `content` tests it.
    #[inline]
    fn read_slot(&self, s: Slot, content: impl FnOnce() -> u128) {
        // (the slot's table and index, found once for its stamp and its
        // version)
        let at = table_at(s.0, s.1);
        let stamp = at.and_then(|(f, i)| self.stamps[f].get(i));
        let generation = self.generation.get();
        if stamp.is_some_and(|c| c.get() == generation) {
            return;
        }
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        // (every slot has the version its last writer made; none: a slot
        // past the tables, which no write reaches, never equal)
        let v = at
            .and_then(|(f, i)| r.st.vers.known_at(f, i))
            .unwrap_or_else(|| r.st.vers.revision(s));
        if self.check && Version(content() | 1) != v {
            // (the test of 7.17.12's convention: a store that bypassed its
            // accessor leaves the version behind the content)
            *r.st.stale.entry(s.0).or_default() += 1;
            if r.st.stale_first.len() < 12 && !r.st.stale_first.contains(&s) {
                r.st.stale_first.push(s);
            }
        }
        r.st.noted[count_ix(s)] += 1;
        r.rt.note_read(&Loc::State(s), v);
        if let Some(c) = stamp {
            c.set(generation);
        }
    }
}

impl Tracker for SsaTracker {
    const VALUES: bool = true;
    const LINES: bool = true;
    const NAMES: bool = true;

    fn command(&self, n: u64, _depth: usize, _line: i32, _level: i32, _outer: bool) {
        // (the steps' times, [`SsaTracker::set_timed`])
        if self.timed
            && let Ok(mut r) = self.rec.try_borrow_mut()
        {
            r.rt.set_clock(n);
        }
    }

    fn read(&self, cell: Cell) {
        if matches!(
            cell,
            Cell::Eqtb(_)
                | Cell::Hash(_)
                | Cell::HashNext(_)
                | Cell::Str(_)
                | Cell::Font(_)
                | Cell::FontTable
        ) {
            // (a table slot with its version, in `read_content`; the pool's
            // search by its result, in `string_search`; a font's fields and
            // the font table by field, in `row_read`)
            return;
        }
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let s = Slot::of(cell);
        let v = r.st.vers.revision(s);
        r.st.noted[count_ix(s)] += 1;
        r.rt.note_read(&Loc::State(s), v);
    }

    fn read_content(&self, cell: Cell, content: impl FnOnce() -> u128) {
        self.read_slot(Slot::of(cell), content);
    }

    fn row_read(&self, row: Row, content: impl FnOnce() -> u128) {
        self.read_slot(Slot::row(row), content);
    }

    fn row_wrote(&self, row: Row, version: u128) {
        let s = Slot::row(row);
        // (a slot written again in the same call is noted once: its
        // version is the array's at the call's end)
        let stamp = self.wstamp(s);
        let generation = self.generation.get();
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            r.st.vers.set(s, version);
            if r.on && stamp.is_none_or(|c| c.get() != generation) {
                r.rt.note_write(&s);
                if let Some(c) = stamp {
                    c.set(generation);
                }
            }
        } else {
            self.lost.set(self.lost.get() + 1);
        }
    }

    fn queried(&self, q: crate::track::Query, answer: u128) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if r.on && r.rt.open_step_id().is_some() {
            r.st.steps.queried(q, answer);
        }
    }

    fn glyphs_united(&self, union: u128) {
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            r.st.steps.glyphs_united(union);
        } else {
            self.lost.set(self.lost.get() + 1);
        }
    }

    fn reopen(&self, name: &[u8], kind: crate::host::FileKind) -> Option<crate::host::WriteId> {
        self.rec.try_borrow_mut().ok()?.st.steps.reopen(name, kind)
    }

    fn value_read(&self, row: Row, version: impl FnOnce() -> u128) {
        let s = Slot::row(row);
        let stamp = self.value_stamp(s, false);
        let generation = self.generation.get();
        if stamp.is_some_and(|c| c.get() == generation) {
            return;
        }
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        r.note(s, Version(version()));
        if let Some(c) = stamp {
            c.set(generation);
        }
    }

    fn value_wrote(&self, row: Row) {
        let s = Slot::row(row);
        let stamp = self.value_stamp(s, true);
        let generation = self.generation.get();
        if stamp.is_some_and(|c| c.get() == generation) {
            return;
        }
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            if r.on {
                r.rt.note_write(&s);
                if let Some(c) = stamp {
                    c.set(generation);
                }
            }
        } else {
            self.lost.set(self.lost.get() + 1);
        }
    }

    fn call_begin(&self, f: Func, args: &[u128], view: &dyn EngineView) -> Option<u32> {
        if !self.framed(f) {
            // (no frame, record or probe: the body runs in the frame of
            // the call around it, DESIGN 4.3 item 2)
            let Ok(mut r) = self.rec.try_borrow_mut() else {
                return None;
            };
            if !r.on {
                return None;
            }
            r.st.routines[f as usize].calls += 1;
            drop(r);
            self.calls.borrow_mut().push((None, f, false));
            return None;
        }
        self.flush_effects();
        {
            let Ok(mut r) = self.rec.try_borrow_mut() else {
                return None;
            };
            if !r.on {
                return None;
            }
            let rr = &mut *r;
            rr.flush_output();
            let args: Vec<Version> = args.iter().map(|&a| Version(a)).collect();
            // (a lookup only where its hit is used: put in place, or
            // compared in check mode; otherwise the body runs either way)
            let found = if self.probing(f) {
                rr.rt.probe(
                    &View {
                        tex: view,
                        rec: &rr.st,
                    },
                    f,
                    &args,
                )
            } else {
                Found::New
            };
            let hit = match found {
                Found::Hit(id) => Some(id),
                _ => None,
            };
            let c = &mut rr.st.routines[f as usize];
            c.calls += 1;
            c.hits += u64::from(hit.is_some());
            if let Some(id) = hit
                && self.apply.get()
                && f.applies()
                && !self.check
            {
                // (put in place by the engine, `apply_hit`: no call opens)
                return Some(id);
            }
            rr.rt.begin_quiet(f, args, hit.is_some());
            self.calls.borrow_mut().push((hit, f, true));
        }
        self.boundary();
        None
    }

    fn ssa(&self) -> Option<&SsaTracker> {
        Some(self)
    }

    fn call_end(&self, view: &dyn EngineView) {
        if self.calls.borrow().last().is_some_and(|c| !c.2) {
            // (a call with no frame: nothing to close)
            self.calls.borrow_mut().pop();
            return;
        }
        self.flush_effects();
        {
            let Some((hit, f, _)) = self.calls.borrow_mut().pop() else {
                return;
            };
            let Ok(mut r) = self.rec.try_borrow_mut() else {
                return;
            };
            let rr = &mut *r;
            let live = rr.rt.live_records();
            rr.flush_output();
            let body = rr.rt.end(
                &View {
                    tex: view,
                    rec: &rr.st,
                },
                SVal::ver(Version::ABSENT),
            );
            if rr.rt.live_records() > live {
                rr.st.routines[f as usize].records += 1;
            }
            if let (true, Some(hit)) = (self.check, hit) {
                rr.st
                    .compare_writes(&rr.rt, hit, body, &alloc::format!("{f}"));
            }
        }
        self.boundary();
    }

    fn row_made(&self, row: Row, version: u128) {
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            r.st.vers.set(Slot::row(row), version);
        } else {
            self.lost.set(self.lost.get() + 1);
        }
    }

    fn wrote(&self, cell: Cell, version: u128) {
        if let Ok(mut r) = self.rec.try_borrow_mut() {
            r.st.vers.set(Slot::of(cell), version);
        } else {
            self.lost.set(self.lost.get() + 1);
        }
    }

    fn string_search(&self, name: &[u8], found: i32) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let rr = &mut *r;
        let (id, fresh) = Recorder::intern(&mut rr.st.searches_ix, rr.st.searches.len(), name);
        if fresh {
            rr.st.searches.push(name.to_vec());
        }
        rr.note(Slot(Fam::Search, i64::from(id)), Version::of(&found));
    }

    fn write(&self, cell: Cell) {
        if matches!(cell, Cell::Font(_) | Cell::FontTable) {
            // (a font's fields and the font table are written by field, in
            // `row_wrote`)
            return;
        }
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if let Cell::Eqtb(p) = cell
            && crate::track::tokenizes(p)
        {
            r.st.generation += 1;
        }
        let s = Slot::of(cell);
        r.st.vers.wrote(s);
        if r.on {
            r.rt.note_write(&s);
        }
    }

    fn line_start(&self, data: &[u8], from: usize) -> u64 {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return 0;
        };
        let generation = r.st.generation;
        if !r.on {
            return generation;
        }
        if r.rt.open_step_id().is_some() {
            // (the lines a step read: where an edit makes it dirty)
            r.st.steps.line_read(data, from);
        }
        let Some(src) = r.st.src.as_mut() else {
            return generation;
        };
        let addr = data.as_ptr() as usize;
        if let Some(l) = src
            .levels
            .iter_mut()
            .find(|l| !l.closed && l.data == addr && l.next == from)
        {
            l.k += 1;
            l.cur = Some((l.k, from));
            l.next = line_bounds(data, from).1;
        } else if !src.opened.contains(&addr) {
            // (a line of a file the call neither started in nor opened: a
            // `\read` of a stream opened before it)
            let v = r.st.vers.revision(Slot(Fam::Unknown, 0));
            r.note(Slot(Fam::Unknown, 0), v);
        }
        generation
    }

    fn line_end(
        &self,
        data: &[u8],
        from: usize,
        generation: u64,
        codes: impl FnOnce() -> Option<LineCodes>,
    ) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let addr = data.as_ptr() as usize;
        let Some(src) = r.st.src.as_mut() else { return };
        let Some((j, k)) = src.levels.iter_mut().enumerate().find_map(|(j, l)| {
            (l.data == addr && l.cur.is_some_and(|c| c.1 == from))
                .then(|| (j, l.cur.take().map_or(0, |c| c.0)))
        }) else {
            return;
        };
        let codes = if k == 0 {
            (generation == src.generation && r.st.generation == generation).then_some(LineCodes {
                cat: [0; 256],
                end_line_char: -1,
            })
        } else if r.st.generation == generation {
            codes()
        } else {
            None
        };
        let line = &data[from..line_bounds(data, from).0];
        r.line_done(j, k, line, codes.as_ref());
    }

    fn file(&self, depth: usize, name: Option<&[u8]>) {
        if name.is_some() {
            return;
        }
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let Some(src) = r.st.src.as_mut() else { return };
        let Some(l) = src.levels.get_mut(depth).filter(|l| !l.closed) else {
            return;
        };
        l.closed = true;
        let k = l.k + 1;
        r.note(src_slot(depth, EOF, k), Version::ABSENT);
    }

    fn line_number(&self, level: usize, value: i32) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let Some(src) = r.st.src.as_ref() else { return };
        let v = Version::of(&value);
        // (the step keeps where each level it read the number of was at
        // its start: DESIGN 7.17.3, "A line number read is kept by
        // position")
        let open: Vec<(usize, usize)> = if r.rt.open_step_id().is_none() {
            Vec::new()
        } else if level == usize::MAX {
            src.levels
                .iter()
                .skip(1)
                .filter(|l| l.data != 0)
                .map(|l| (l.data, l.next))
                .collect()
        } else {
            src.levels
                .get(level)
                .filter(|l| l.data != 0)
                .map(|l| (l.data, l.next))
                .into_iter()
                .collect()
        };
        let reads: Vec<Slot> = if level == usize::MAX {
            // (a number whose level is not known: relative to every level
            // open at the start)
            (1..src.levels.len())
                .map(|j| line_slot(j, value.wrapping_sub(src.levels[j].line)))
                .collect()
        } else if level < src.levels.len() {
            alloc::vec![line_slot(level, value.wrapping_sub(src.levels[level].line))]
        } else {
            Vec::new() // (a file the call opened: its lines are the load's)
        };
        for s in reads {
            r.note(s, v);
        }
        for (addr, pos) in open {
            r.st.steps.number_read(addr, pos);
        }
    }

    fn name_lookup(&self, name: &[u8], found: i32) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let rr = &mut *r;
        let (id, fresh) = Recorder::intern(&mut rr.st.names_ix, rr.st.names.len(), name);
        if fresh {
            rr.st.names.push(name.to_vec());
        }
        rr.note(Slot(Fam::Name, i64::from(id)), Version::of(&found));
    }

    fn hyph_word_read(&self, key: &[u8], version: u128) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        if !r.on {
            return;
        }
        let rr = &mut *r;
        let id = rr.st.word(key);
        rr.note(Slot(Fam::HyphWord, i64::from(id)), Version(version));
    }

    fn hyph_word_wrote(&self, key: &[u8]) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            self.lost.set(self.lost.get() + 1);
            return;
        };
        if !r.on {
            return;
        }
        let rr = &mut *r;
        let id = rr.st.word(key);
        rr.rt.note_write(&Slot(Fam::HyphWord, i64::from(id)));
    }

    fn load(
        &self,
        name: &[u8],
        kind: crate::host::FileKind,
        contents: Option<&alloc::sync::Arc<[u8]>>,
    ) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        let rr = &mut *r;
        let v = contents.map_or(Version::ABSENT, |c| Version::of(&c[..]));
        let (id, fresh) = Recorder::intern(&mut rr.st.loads_ix, rr.st.loads.len(), name);
        if fresh {
            rr.st.loads.push((name.to_vec(), v, kind));
        } else {
            rr.st.loads[id as usize].1 = v;
            rr.st.loads[id as usize].2 = kind;
        }
        // (whether it read the φ, not the build's own store: DESIGN 7.17.3,
        // "A load reads the store, not the file")
        let key = rr
            .rt
            .open_step_id()
            .map(|j| rr.rt.fold.steps[j as usize].key);
        let phi = key.is_none_or(|k| rr.st.steps.reads_phi(&rr.rt.fold, id, k));
        if let Some(c) = contents {
            rr.st.steps.loaded(id, c, phi);
        }
        if !rr.on {
            return;
        }
        // (the open step's load, with what it found)
        if key.is_some() {
            rr.st.steps.load_seen(id, v, phi);
        }
        if let (Some(src), Some(c)) = (rr.st.src.as_mut(), contents) {
            src.opened.push(c.as_ptr() as usize);
        }
        let v = if rr.st.written.contains(name) {
            rr.st.vers.revision(Slot(Fam::Unknown, 0))
        } else {
            v
        };
        rr.note(Slot(Fam::Load, i64::from(id)), v);
    }

    fn stored(&self, name: &[u8]) -> Option<Option<alloc::sync::Arc<[u8]>>> {
        let r = self.rec.try_borrow().ok()?;
        let id = *r.st.loads_ix.get(name)?;
        let key = r.rt.fold.steps[r.rt.open_step_id()? as usize].key;
        r.st.steps.served(&r.rt.fold, id, key)
    }

    fn store_open(&self, _stream: u8, name: &[u8]) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            return;
        };
        let rr = &mut *r;
        rr.st.written.insert(name.to_vec());
        let (id, fresh) = Recorder::intern(&mut rr.st.loads_ix, rr.st.loads.len(), name);
        if fresh {
            rr.st
                .loads
                .push((name.to_vec(), Version::ABSENT, crate::host::FileKind::Tex));
        }
        let a = Slot(Fam::Load, i64::from(id));
        if rr.on {
            rr.flush_output();
            rr.rt.note_open(&a);
            rr.st.steps.store_opened(id);
        }
    }

    fn store_line(&self, name: &[u8], line: &[u8]) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            self.lost.set(self.lost.get() + 1);
            return;
        };
        let rr = &mut *r;
        if !rr.on {
            return;
        }
        // (the file's address, by name as a load names it, `Fam::Load`: a
        // file's store and its loads are one address, 7.17.5)
        if let Some(&id) = rr.st.loads_ix.get(name) {
            let a = Slot(Fam::Load, i64::from(id));
            // (in program order with the effects before it)
            rr.flush_output();
            rr.rt.note_store(&a, SVal::ver(Version::of(line)));
            rr.st.steps.store_line(id, line);
        }
    }

    fn output(&self, what: Output, bytes: &[u8]) {
        let Ok(mut r) = self.rec.try_borrow_mut() else {
            self.lost.set(self.lost.get() + 1);
            return;
        };
        if !r.on {
            return;
        }
        // (one run per output: the outputs are separate files, so only
        // each one's own order matters, and the terminal and the log,
        // written byte by byte in turn, stay one run each)
        let p = &mut r.st.pending;
        match p.iter_mut().find(|(w, _)| *w == what) {
            Some((_, b)) => b.extend_from_slice(bytes),
            None => p.push((what, bytes.to_vec())),
        }
    }
}

/// What the recorder reads of the engine to verify a read: the source
/// now, the lookups by content, and the structures' versions (made from
/// the values). Every engine is one, whatever its tracker, so the engine's
/// generic code can open a call (`Tracker::call_begin`).
pub trait EngineView {
    /// The open input levels, the line number of the top one and of level
    /// `j` below it (§304).
    fn in_open(&self) -> usize;
    fn line(&self) -> i32;
    fn line_stack_at(&self, j: usize) -> i32;
    /// The current line in the buffer (`start..limit`).
    fn cur_line(&self) -> &[u8];
    /// Input level `j`'s file and where its next line begins.
    fn file_at(&self, j: usize) -> Option<(&[u8], usize)>;
    /// web2c's string search and a control sequence's lookup, by content.
    fn search(&self, name: &[u8]) -> i32;
    fn lookup(&self, name: &[u8]) -> i32;
    /// The version of what the exception table holds for the word `key`
    /// (§930): its positions', or none.
    #[allow(clippy::ptr_arg, reason = "the map's key, looked up as it is")]
    fn hyph_word(&self, key: &Vec<u8>) -> u128;
    /// A field of `cur_list` (`track::list`), the nest, a save stack slot
    /// (`track::save`).
    fn list_version(&self, f: u8) -> u128;
    fn nest_ver(&self) -> u128;
    /// A field of the alignment state (`track::align`).
    fn align_ver(&self, f: u8) -> u128;
    /// A field of the page builder's state (`track::page`).
    fn page_ver(&self, f: u8) -> u128;
    /// The page's `k`-th node (`track::Row::PageNode`).
    fn page_node_ver(&self, k: usize) -> u128;
    fn save_version(&self, s: i64) -> u128;
    fn cond_ver(&self) -> u128;
    fn mark_ver(&self, s: i64) -> u128;
    /// The value slot `s` holds now (the families whose values the
    /// engine can put back; none for a read-only one).
    #[allow(private_interfaces, reason = "the values are the engine's")]
    fn value_of(&self, s: Slot) -> Option<alloc::sync::Arc<SValue>>;
}

impl<H: Host, T: Tracker> EngineView for Tex<H, T> {
    fn in_open(&self) -> usize {
        self.in_open
    }
    fn line(&self) -> i32 {
        self.line
    }
    fn line_stack_at(&self, j: usize) -> i32 {
        self.line_stack.get(j).copied().unwrap_or(0)
    }
    fn cur_line(&self) -> &[u8] {
        let start = usize::try_from(self.cur_input.start).unwrap_or(0);
        let limit = usize::try_from(self.cur_input.limit)
            .unwrap_or(0)
            .max(start);
        self.buffer.get(start..limit).unwrap_or_default()
    }
    fn file_at(&self, j: usize) -> Option<(&[u8], usize)> {
        self.input_file
            .get(j)
            .and_then(Option::as_ref)
            .map(|f| (&f.data[..], f.pos))
    }
    fn search(&self, name: &[u8]) -> i32 {
        self.peek_search(name)
    }
    fn lookup(&self, name: &[u8]) -> i32 {
        self.peek_lookup(name)
    }
    fn hyph_word(&self, key: &Vec<u8>) -> u128 {
        self.hyph.exceptions.word_version(key).0
    }
    fn list_version(&self, f: u8) -> u128 {
        self.list_field_version(f)
    }
    fn nest_ver(&self) -> u128 {
        self.nest_version()
    }
    fn align_ver(&self, f: u8) -> u128 {
        self.align_version(f)
    }
    fn page_ver(&self, f: u8) -> u128 {
        self.page_field_version(f)
    }
    fn page_node_ver(&self, k: usize) -> u128 {
        self.page_node_version(k)
    }
    fn save_version(&self, s: i64) -> u128 {
        self.save_row_version(s)
    }
    fn cond_ver(&self) -> u128 {
        self.cond_version()
    }
    fn mark_ver(&self, s: i64) -> u128 {
        let (c, t) = (s / 5, s % 5);
        self.mark_version(i32::try_from(c).unwrap_or(0), i32::try_from(t).unwrap_or(0))
    }
    #[allow(private_interfaces, reason = "the values are the engine's")]
    fn value_of(&self, s: Slot) -> Option<alloc::sync::Arc<SValue>> {
        slot_value(self, s).map(alloc::sync::Arc::new)
    }
}

/// The store the runtime verifies reads against: the versions, and the
/// engine for a content not cached and the source now.
struct View<'a> {
    tex: &'a dyn EngineView,
    rec: &'a RecState,
}

impl View<'_> {
    /// Input level `j`'s line number now.
    fn level_line(&self, j: usize) -> i32 {
        if j == self.tex.in_open() {
            self.tex.line()
        } else {
            self.tex.line_stack_at(j + 1)
        }
    }

    fn source(&self, s: Slot) -> Version {
        let tex = self.tex;
        let j = usize::try_from(s.1 >> 56).unwrap_or(0);
        let codes = u32::try_from(s.1 >> 32 & 0xff_ffff).unwrap_or(0);
        let k = u32::try_from(s.1 & 0xffff_ffff).unwrap_or(0);
        if k == 0 {
            // the line in the buffer, by its bytes
            return bytes_version(tex.cur_line());
        }
        let Some((data, pos)) = tex.file_at(j) else {
            return self.rec.vers.revision(Slot(Fam::Unknown, 0));
        };
        let mut from = pos;
        for _ in 1..k {
            if from >= data.len() {
                break;
            }
            from = line_bounds(data, from).1;
        }
        if from >= data.len() {
            return if codes == EOF {
                Version::ABSENT
            } else {
                Version::of(&3u8)
            };
        }
        let line = &data[from..line_bounds(data, from).0];
        match codes {
            BYTES => bytes_version(line),
            EOF => Version::of(&4u8),
            c => self
                .rec
                .codes
                .get(c as usize)
                .map_or(Version::of(&5u8), |c| tokens_version(line, c)),
        }
    }
}

impl Store<TexSsa> for View<'_> {
    fn version(&self, loc: &Loc<Slot>) -> Version {
        let s = *loc.addr();
        let rec = self.rec;
        let c = &rec.verified[count_ix(s)];
        c.set(c.get() + 1);
        match s.0 {
            Fam::Search => usize::try_from(s.1)
                .ok()
                .and_then(|i| rec.searches.get(i))
                .map_or(Version::of(&8u8), |n| Version::of(&self.tex.search(n))),
            Fam::Eqtb
            | Fam::Hash
            | Fam::HashNext
            | Fam::Pool
            | Fam::Alloc
            | Fam::Font
            | Fam::FontTable
            | Fam::Hyph
            | Fam::Pdf
            | Fam::Dvi
            | Fam::Out
            | Fam::Read
            | Fam::Random
            | Fam::Glyphs => rec.vers.known(s).unwrap_or_else(|| rec.vers.revision(s)),
            Fam::Source => self.source(s),
            Fam::Save => Version(self.tex.save_version(s.1)),
            Fam::Cond => Version(self.tex.cond_ver()),
            Fam::Mark => Version(self.tex.mark_ver(s.1)),
            Fam::List => {
                let f = u8::try_from(s.1).unwrap_or(0);
                let count = crate::track::list::COUNT;
                Version(match f.cmp(&count) {
                    core::cmp::Ordering::Equal => self.tex.nest_ver(),
                    core::cmp::Ordering::Greater => self.tex.align_ver(f - count - 1),
                    core::cmp::Ordering::Less => self.tex.list_version(f),
                })
            }
            Fam::Line => {
                let j = usize::try_from(s.1 >> 32).unwrap_or(0);
                let d = low32(s.1);
                Version::of(&self.level_line(j).wrapping_add(d))
            }
            Fam::Name => usize::try_from(s.1)
                .ok()
                .and_then(|i| rec.names.get(i))
                .map_or(Version::of(&6u8), |n| Version::of(&self.tex.lookup(n))),
            Fam::HyphWord => usize::try_from(s.1)
                .ok()
                .and_then(|i| rec.words.get(i))
                .map_or(Version::of(&9u8), |w| Version(self.tex.hyph_word(w))),
            Fam::Page => Version(self.tex.page_ver(u8::try_from(s.1).unwrap_or(0))),
            Fam::PageNode => Version(
                self.tex
                    .page_node_ver(usize::try_from(s.1).unwrap_or(usize::MAX)),
            ),
            Fam::Load => match usize::try_from(s.1).ok().and_then(|i| rec.loads.get(i)) {
                Some((n, ..)) if rec.written.contains(n) => {
                    rec.vers.revision(Slot(Fam::Unknown, 0))
                }
                Some((_, v, _)) => *v,
                None => Version::of(&7u8),
            },
            _ => rec.vers.revision(s),
        }
    }
    fn get(&self, a: &Slot) -> Option<SVal> {
        Some(SVal(self.version(&Loc::State(*a)), self.tex.value_of(*a)))
    }
    fn set(&mut self, _a: &Slot, _v: Option<SVal>) {}
}

/// A build's counts.
#[derive(Clone, Debug, Default)]
pub struct SsaReport {
    pub history: i32,
    pub paragraphs: u64,
    pub para_hits: u64,
    pub tokenize_hits: u64,
    pub tokenize_calls: u64,
    pub commands: u64,
    /// Commands inside paragraphs that hit: what applying them would skip.
    pub commands_in_hits: u64,
    /// Check mode: parts of the state that differed between an applied
    /// hit and its re-run, with the count and the first call's name.
    pub uncovered: BTreeMap<&'static str, (u64, String)>,
    pub checked: u64,
    /// Check mode: 7.17.12's value rows that are not values yet (compared
    /// at each hit as the entry state, since a record holds no write of
    /// them): exactly what a hit could not apply.
    pub not_values: Vec<&'static str>,
    /// Check mode: the first slots a hit's record and its body wrote
    /// differently.
    pub write_diffs: Vec<String>,
    /// The routines opened through `Tracker::call_begin`, by
    /// [`Func::ALL`]'s order: this build's calls and (probed) hits, and
    /// the records made for them so far.
    pub routines: [RoutineCount; Func::COUNT],
    /// Hits put in place (the paragraphs and the calls inside running
    /// ones), and the commands their bodies would have run.
    pub applied: u64,
    pub commands_skipped: u64,
}

struct Open {
    hit: bool,
    /// The record a hit found (check mode compares its writes with the
    /// body's).
    rec: Option<partex_ssa::runtime::RecId>,
    name: String,
    entry: Option<Vec<(&'static str, u128)>>,
    commands: u64,
}

/// Run a job on the runtime: the root call, a call per paragraph, a
/// tokenizer call for the line each starts on.
pub fn run<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    command_line: &[u8],
    check: bool,
    epoch: u64,
) -> SsaReport {
    run_applying(tex, command_line, check, epoch, false)
}

/// [`run`], with hits put in place if `apply` (DESIGN 7.17.2: the body of
/// a hit does not run; its writes are stored, its effects emitted again,
/// the input set to where it ended). Check mode runs every hit's body
/// either way and compares.
pub fn run_applying<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    command_line: &[u8],
    check: bool,
    epoch: u64,
    apply: bool,
) -> SsaReport {
    let mut rep = SsaReport::default();
    {
        // the files loaded by name, as they are now (their loads are reads)
        let names: Vec<(Vec<u8>, crate::host::FileKind)> = tex
            .tracker
            .rec
            .borrow()
            .st
            .loads
            .iter()
            .map(|l| (l.0.clone(), l.2))
            .collect();
        let now: Vec<Version> = names
            .iter()
            .map(|(n, k)| {
                tex.host
                    .read_file(n, *k)
                    .map_or(Version::ABSENT, |f| Version::of(&f.contents[..]))
            })
            .collect();
        let mut r = tex.tracker.rec.borrow_mut();
        for (l, v) in r.st.loads.iter_mut().zip(now) {
            l.1 = v;
        }
    }
    {
        let mut r = tex.tracker.rec.borrow_mut();
        r.st.vers = Versions {
            epoch,
            ..Versions::default()
        };
        r.st.generation = 0;
        r.st.src = None;
        r.st.noted = [0; 28];

        for c in &r.st.verified {
            c.set(0);
        }
        r.st.written.clear();
        // (a build from the job's start is a fold of its own: the last
        // build's steps are not its)
        r.rt.fold = partex_ssa::fold::Fold::default();
        r.rt.open_trip(0);
        // (the top level is a fold of steps and reads nothing, 7.17.9: the
        // job's start is the first step)
        r.rt.begin_step();
        // (a step's record keeps its own reads only to be compared, in
        // check mode: DESIGN 4.3 item 2)
        let args = alloc::vec![Version::of(&command_line)];
        if check || !tex.tracker.lean {
            r.rt.begin(Func::Start, args);
        } else {
            r.rt.begin_lean(Func::Start, args);
        }
        r.on = true;
    }
    tex.tracker.check = check;
    tex.tracker.apply.set(apply);
    // (the files are linked from the steps' effects, DESIGN 7.17.3)
    tex.set_effects(true);
    // (boxes carry versions, made when each becomes a shared value)
    partex_engine::node::VERSIONS.store(true, core::sync::atomic::Ordering::Relaxed);
    tex.remake_constant_lists();
    if check {
        rep.not_values = tex.value_rows().into_iter().map(|(n, _)| n).collect();
    }
    tex.size_stamps();
    // (the tables as the engine was made: their versions; a format's load
    // makes them again, `Tex::load_fmt_file`)
    tex.version_tables();
    tex.tracker.boundary();
    tex.set_stop_at_candidate(true);
    // (a fire is a step boundary, DESIGN §7.16.1)
    tex.set_defer_fire(true);
    let mut open = Some(Open {
        hit: false,
        rec: None,
        name: String::from("start"),
        entry: None,
        commands: 0,
    });
    let mut step = tex.start(command_line);
    loop {
        match step {
            Step::Checkpoint => {
                if matches!(
                    tex.clean_point(),
                    Some(CleanPoint::Outer | CleanPoint::Fire)
                ) {
                    close_paragraph(tex, &mut open, &mut rep, Close::Step);
                    open = Some(open_paragraph(tex, check, &mut rep, None));
                }
                step = tex.resume();
            }
            Step::Finished(h) => {
                rep.history = h;
                break;
            }
        }
    }
    tex.tracker.end_open_calls(&*tex);
    close_paragraph(tex, &mut open, &mut rep, Close::Last);
    tex.tracker.flush_effects();
    rep.applied = tex.tracker.applied.get();
    rep.commands_skipped = tex.tracker.skipped.get();
    let mut r = tex.tracker.rec.borrow_mut();
    // (the job's end: the root call's effects)
    r.flush_output();
    r.on = false;
    rep.checked += r.st.calls_checked;
    for (k, v) in core::mem::take(&mut r.st.check_diffs) {
        let e = rep.uncovered.entry(k).or_insert((0, v.1));
        e.0 += v.0;
    }
    rep.write_diffs = core::mem::take(&mut r.st.write_diffs);
    rep.routines = r.st.routines;
    for c in &mut r.st.routines {
        // (the next build counts its own calls; the records stay)
        c.calls = 0;
        c.hits = 0;
    }
    r.rt.close_trip();
    rep.commands = tex.commands();
    rep
}

/// Begin the fold's next step at a clean point (7.17.9): the tokenizer
/// over the line it begins on, then the step's call, named by the line's
/// tokens.
fn open_paragraph<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    check: bool,
    rep: &mut SsaReport,
    rerun: Option<partex_ssa::fold::StepId>,
) -> Open {
    tex.tracker.flush_effects();
    {
        let mut r = tex.tracker.rec.borrow_mut();
        match rerun {
            Some(j) => r.rt.rerun_step(j),
            None => {
                r.rt.begin_step();
            }
        }
        r.st.steps.run_begins(rerun);
    }
    // the line the paragraph starts on, its tokens, and the offset in them
    // (at a fire, the topmost file level's, under the token lists: DESIGN
    // §7.16.1, "The deferred fire's form")
    let file = if tex.cur_input.state == crate::web::TOKEN_LIST {
        tex.input_stack[..tex.input_ptr]
            .iter()
            .rev()
            .find(|r| r.state != crate::web::TOKEN_LIST)
            .cloned()
            .unwrap_or_default()
    } else {
        tex.cur_input.clone()
    };
    let start = usize::try_from(file.start).unwrap_or(0);
    let limit = usize::try_from(file.limit).unwrap_or(0).max(start);
    let loc = usize::try_from(file.loc)
        .unwrap_or(0)
        .clamp(start, limit + 1);
    let line: Vec<u8> = (start..limit).map(|i| tex.buffer[i]).collect();
    let bytes = Version::of(&line);
    // (the tokenizer has a frame and a record in check mode only, DESIGN
    // 4.3 item 2: otherwise the codes it reads are reads of the step, in
    // no frame of their own)
    let framed = tex.tracker.framed(Func::Tokenize);
    let tokens = {
        let found = if framed {
            let mut r = tex.tracker.rec.borrow_mut();
            let rr = &mut *r;
            rr.flush_output();
            let f = if tex.tracker.probing(Func::Tokenize) {
                rr.rt
                    .probe(&View { tex, rec: &rr.st }, Func::Tokenize, &[bytes])
            } else {
                Found::New
            };
            rr.rt.begin_quiet(
                Func::Tokenize,
                alloc::vec![bytes],
                matches!(f, Found::Hit(_)),
            );
            drop(r);
            tex.tracker.boundary();
            f
        } else {
            Found::New
        };
        rep.tokenize_calls += 1;
        if matches!(found, Found::Hit(_)) {
            rep.tokenize_hits += 1;
        }
        // the catcodes it reads, through the accessors (recorded)
        let mut codes = LineCodes {
            cat: [0; 256],
            end_line_char: tex.int_par(crate::web::END_LINE_CHAR_CODE),
        };
        let mut seen = [false; 256];
        for &c in line
            .iter()
            .chain(u8::try_from(codes.end_line_char).ok().as_ref())
        {
            if !core::mem::replace(&mut seen[usize::from(c)], true) {
                codes.cat[usize::from(c)] = u8::try_from(tex.cat_code(i32::from(c))).unwrap_or(15);
            }
        }
        // (a line the tokenizer does not read without changing the buffer,
        // `^^` notation or an invalid character: by its bytes)
        let v = tokens_version(&line, &codes);
        // (what the paragraph reads of the line: its rest from `loc`,
        // tokenized from the scanner's state under the codes now, with the
        // end-of-line character if it is not read yet; by its bytes where
        // it does not tokenize plainly)
        let past_end = loc > limit;
        let rest = &line[loc.saturating_sub(start).min(line.len())..];
        let state = match file.state {
            s if s == crate::web::MID_LINE => 1u8,
            s if s == crate::web::SKIP_BLANKS => 2,
            _ => 0,
        };
        let rest_codes = LineCodes {
            end_line_char: if past_end { -1 } else { codes.end_line_char },
            ..codes
        };
        let offset = match crate::track::line_tokens_from(rest, &rest_codes, state) {
            Some(t) => Version::of(&(1u8, t, past_end)),
            None => Version::of(&(2u8, rest, state, past_end, rest_codes.end_line_char)),
        };
        if framed {
            let mut r = tex.tracker.rec.borrow_mut();
            let rr = &mut *r;
            rr.flush_output();
            rr.rt.end(&View { tex, rec: &rr.st }, SVal::ver(v));
            drop(r);
            tex.tracker.boundary();
        }
        (v, offset)
    };
    // (the line's tokens and what the paragraph reads of it: a clean
    // point before the end-of-line character and one after it are two
    // places, the second one's call reading the next line)
    let mut args = alloc::vec![tokens.0, tokens.1];
    if tex.fire_pending {
        // (a step that begins with a fire)
        args.push(Version::of(&0x6669_7265u32));
    }
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    let found = if tex.tracker.probing(Func::Step) {
        rr.rt.probe(&View { tex, rec: &rr.st }, Func::Step, &args)
    } else {
        Found::New
    };
    let hit = matches!(found, Found::Hit(_));
    // (the step's name for check mode's reports)
    let name = if check {
        alloc::format!(
            "step({}, {}) at level {} line {} loc {} state {} `{}`",
            tokens.0,
            tokens.1,
            tex.in_open,
            tex.line,
            file.loc - file.start,
            file.state,
            String::from_utf8_lossy(&line)
        )
    } else {
        String::new()
    };
    rr.flush_output();
    // (its own reads kept only to be compared, in check mode: DESIGN 4.3
    // item 2; its reads from outside it are the fold's either way)
    if tex.tracker.step_keeps_reads() {
        rr.rt.begin_quiet(Func::Step, args, hit);
    } else {
        rr.rt.begin_lean(Func::Step, args);
    }
    rr.st.src = Some(source_at_start(tex, rr.st.generation));
    drop(r);
    tex.tracker.boundary();
    rep.paragraphs += 1;
    if hit {
        rep.para_hits += 1;
    }
    Open {
        hit,
        rec: match found {
            Found::Hit(id) => Some(id),
            _ => None,
        },
        name,
        entry: (check && hit).then(|| tex.value_rows()),
        commands: tex.commands(),
    }
}

/// How [`close_paragraph`] ends: the step with its call, the last step
/// (the job ended in it), or the call only, the step left open (a
/// rebuild validates its reads first, `rebuild::run_step`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Close {
    Step,
    Last,
    Call,
}

fn close_paragraph<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    open: &mut Option<Open>,
    rep: &mut SsaReport,
    how: Close,
) {
    let Some(o) = open.take() else { return };
    if o.hit {
        rep.commands_in_hits += tex.commands() - o.commands;
    }
    if let Some(entry) = &o.entry {
        rep.checked += 1;
        let exit = tex.value_rows();
        for ((name, a), (_, b)) in entry.iter().zip(exit.iter()) {
            if a != b {
                let e = rep.uncovered.entry(name).or_insert((0, o.name.clone()));
                e.0 += 1;
            }
        }
    }
    // (the step's last chunk of effects in the link's form: what the
    // files are linked from, DESIGN 7.17.3)
    cut_chunk(tex);
    // (where the step leaves the input, if it ends: its result)
    let input = (how != Close::Call).then(|| rebuild::InputState::of(tex, how == Close::Last));
    let mut r = tex.tracker.rec.borrow_mut();
    let rr = &mut *r;
    // (the output made in the call: its effects)
    rr.flush_output();
    let result = close_source(tex, rr);
    // (the commands the body ran: what a hit of its record stands for)
    rr.rt.note_cost(tex.commands() - o.commands);
    // (and the step's cost, its last run's: a dropped run's is not)
    if let Some(id) = rr.rt.open_step_id() {
        let c = &mut rr.st.step_commands;
        if c.len() <= id as usize {
            c.resize(id as usize + 1, 0);
        }
        c[id as usize] = tex.commands() - o.commands;
    }
    let body = rr.rt.end(&View { tex, rec: &rr.st }, result);
    if how == Close::Call {
        drop(r);
        tex.tracker.boundary();
        return;
    }
    // (the step ends with its call: its definitions and readers, 7.17.3)
    if let (Some(id), Some(input)) = (rr.rt.end_step(), input) {
        rebuild::step_closed(rr, id, input);
    }
    let mut stores = None;
    if let (true, Some(hit)) = (o.entry.is_some(), o.rec) {
        let same = rr.st.compare_writes(&rr.rt, hit, body, &o.name);
        if same {
            stores = Some(rr.rt.record(hit).writes.clone());
        }
    }
    drop(r);
    if let Some(writes) = stores {
        // (the hit's writes stored over the body's state, which holds the
        // same values: each store must leave the slot at the version the
        // record keeps, the test of `Store::set`)
        let mut rec = core::mem::take(&mut *tex.tracker.rec.borrow_mut());
        for (a, v) in &writes {
            let Some(v) = v else { continue };
            set_value(tex, &mut rec.st.vers, *a, v);
            let now = View {
                tex: &*tex,
                rec: &rec.st,
            }
            .version(&Loc::State(*a));
            if now != v.0 {
                let e = rec
                    .st
                    .check_diffs
                    .entry(set_row(a.0))
                    .or_insert((0, o.name.clone()));
                e.0 += 1;
            }
        }
        *tex.tracker.rec.borrow_mut() = rec;
    }
    tex.tracker.boundary();
}

/// The live steps' effects in program order: what the build's files are
/// linked from (DESIGN 7.17.3, `effects::link`), each chunk with its key,
/// its step's id and its place in the step (`step << 32 | k`), as the
/// steps kept them when their runs ended.
#[must_use]
pub fn step_effects(rec: &Recorder) -> Vec<(u64, StepEffects)> {
    let kept = &rec.st.steps.effects;
    let mut out = Vec::new();
    for &s in &rec.rt.fold.order {
        let Some(fx) = kept.get(s as usize) else {
            continue;
        };
        for (k, e) in fx.iter().enumerate() {
            out.push(((u64::from(s) << 32) | k as u64, e.clone()));
        }
    }
    out
}

/// The steps whose chunks changed since the last call, each once: its
/// key in program order and its chunks with their versions, or none if it
/// left the fold (DESIGN 4.3 item 4: the link takes what changed, not
/// every step's chunks as [`step_effects`] does).
pub fn take_step_changes(rec: &mut Recorder) -> Vec<crate::effects::StepChunks> {
    let mut ids = core::mem::take(&mut rec.st.steps.fx_changed);
    ids.sort_unstable();
    ids.dedup();
    let fold = &rec.rt.fold;
    ids.into_iter()
        .map(|s| {
            let step = fold.steps.get(s as usize);
            let live = step.is_some_and(|x| x.live);
            crate::effects::StepChunks {
                step: s,
                order: step.map_or(0, |x| x.key),
                chunks: live.then(|| {
                    rec.st
                        .steps
                        .effects
                        .get(s as usize)
                        .map(|v| v.iter().map(|e| (e.0.0, e.1.clone())).collect())
                        .unwrap_or_default()
                }),
            }
        })
        .collect()
}

/// How many times the fold's keys were made again (each step's key, for
/// the link: [`step_key`]).
#[must_use]
pub fn keys_renumbered(rec: &Recorder) -> u32 {
    rec.rt.fold.renumbered
}

/// Step `s`'s key in program order now.
#[must_use]
pub fn step_key(rec: &Recorder, s: u32) -> u64 {
    rec.rt.fold.steps.get(s as usize).map_or(0, |x| x.key)
}

/// A family's row name for check mode's report of a store that did not
/// put the recorded version back.
fn set_row(f: Fam) -> &'static str {
    match f {
        Fam::Eqtb => "set: eqtb",
        Fam::Hash | Fam::HashNext => "set: hash",
        Fam::Pool => "set: pool",
        Fam::Alloc => "set: scalar slots",
        Fam::List => "set: list",
        Fam::Save => "set: save stack",
        Fam::Cond => "set: conditionals",
        Fam::Mark => "set: marks",
        _ => "set: a family without values",
    }
}

/// A family's row name for check mode's report of writes that differ.
fn fam_row(f: Fam) -> &'static str {
    match f {
        Fam::Eqtb => "writes: eqtb",
        Fam::Hash => "writes: hash text",
        Fam::HashNext => "writes: hash next",
        Fam::Font => "writes: font",
        Fam::FontTable => "writes: font table",
        Fam::Read => "writes: read",
        Fam::Out => "writes: out",
        Fam::Random => "writes: random",
        Fam::Str => "writes: str",
        Fam::Pool => "writes: pool",
        Fam::Alloc => "writes: scalar slots",
        Fam::Hyph | Fam::HyphWord => "writes: hyph",
        Fam::Pdf => "writes: pdf",
        Fam::Dvi => "writes: dvi",
        _ => "writes: other",
    }
}

/// The input levels as a call starts: each file's data, where its next line
/// begins, its line number; the top level's line in the buffer is rank 0.
fn source_at_start<H: Host>(tex: &Tex<H, SsaTracker>, generation: u64) -> Src {
    let mut levels = Vec::with_capacity(tex.in_open + 1);
    for j in 0..=tex.in_open {
        let f = tex.input_file.get(j).and_then(Option::as_ref);
        let line = if j == tex.in_open {
            tex.line
        } else {
            tex.line_stack.get(j + 1).copied().unwrap_or(0)
        };
        levels.push(match f {
            Some(f) if j > 0 => Level {
                data: f.data.as_ptr() as usize,
                next: f.pos,
                k: 0,
                cur: (j == tex.in_open && f.line_open).then_some((0, f.line_from)),
                closed: false,
                line,
            },
            _ => Level {
                data: 0,
                next: usize::MAX,
                k: 0,
                cur: None,
                closed: true,
                line,
            },
        });
    }
    Src {
        levels,
        opened: Vec::new(),
        generation,
        input_ptr: tex.input_ptr,
    }
}

/// End the open call's source: the lines still in the buffer are read by
/// their bytes, and where the call ended (the lines each level consumed,
/// the depth, the offset in the last line) is its result.
fn close_source<H: Host>(tex: &Tex<H, SsaTracker>, rr: &mut Recorder) -> SVal {
    let Some(src) = rr.st.src.take() else {
        return SVal::ver(Version::ABSENT);
    };
    let changed = rr.st.generation != src.generation;
    let mut ended = Vec::with_capacity(src.levels.len());
    for (j, l) in src.levels.iter().enumerate() {
        ended.push((l.k, l.closed));
        let Some((k, from)) = l.cur else { continue };
        if k == 0 && !changed {
            continue;
        }
        let Some(f) = tex.input_file.get(j).and_then(Option::as_ref) else {
            continue;
        };
        if f.data.as_ptr() as usize != l.data || from > f.data.len() {
            continue;
        }
        let line = &f.data[from..line_bounds(&f.data, from).0];
        rr.line_done(j, k, line, None);
    }
    let offset = tex.cur_input.loc - tex.cur_input.start;
    let ver = Version::of(&(&ended, tex.in_open, offset));
    // (where the call ended, relative to where it began: what a hit sets
    // the input to)
    let c = &tex.cur_input;
    let (start, limit) = (
        usize::try_from(c.start).unwrap_or(0),
        usize::try_from(c.limit).unwrap_or(0),
    );
    let line = if c.limit >= c.start {
        (start..=limit).map(|i| tex.buffer[i]).collect()
    } else {
        Vec::new()
    };
    let files = src.levels.iter().map(|l| l.data != 0).collect();
    let pos = Position {
        levels: ended,
        files,
        depth: (
            src.levels.len() - 1,
            src.input_ptr,
            tex.in_open,
            tex.input_ptr,
        ),
        opened: !src.opened.is_empty(),
        file: c.state != crate::web::TOKEN_LIST && c.name > 19,
        line,
        loc: c.loc - c.start,
        limit: c.limit - c.start,
        state: c.state,
    };
    SVal::held(ver, SValue::Pos(pos))
}

impl<H: Host> Tex<H, SsaTracker> {
    /// The stamps that note a table read once per call, one per slot.
    fn size_stamps(&mut self) {
        let stamps = |n: usize| (0..n).map(|_| core::cell::Cell::new(0)).collect::<Vec<_>>();
        let hash = usize::try_from(crate::web::HASH_BASE).unwrap_or(0) + self.hash.len();
        self.tracker.stamps = [
            stamps(self.eqtb.len()),
            stamps(hash),
            stamps(hash),
            stamps(1 << 18),
            stamps(1 << 20),
            stamps(usize::from(crate::track::scalar::COUNT)),
            stamps(
                (usize::try_from(self.params.font_max).unwrap_or(0) + 1)
                    * crate::track::font::FIELDS as usize,
            ),
            stamps(1),
            stamps(crate::track::hyph::COUNT as usize),
            stamps(usize::from(crate::pdf::val::field::COUNT)),
            stamps(usize::from(
                crate::pdf::val::dvi_field::END - crate::pdf::val::DVI,
            )),
            stamps(usize::from(crate::streams::LOG) + 1),
            stamps(16),
            stamps(1),
            // (the ships' glyphs: each read once, by the job's end)
            Vec::new(),
        ];
        let save = self.save_stack.len() + 16;
        self.tracker.sstamps = stamps(save);
        self.tracker.swstamps = stamps(save);
    }

    /// Writes whose version the recorder could not take.
    #[must_use]
    pub fn lost_writes(&self) -> u64 {
        self.tracker.lost.get()
    }

    /// The recorder, to move into the next build's engine.
    pub fn take_recorder(&mut self) -> Recorder {
        core::mem::take(&mut *self.tracker.rec.borrow_mut())
    }
}

// Hits put in place (DESIGN 7.17.2, 7.17.12: a hit is nothing but
// stores). Each written slot's value, as a record keeps it
// ([`slot_value`]), and its store back ([`set_value`]), by family; the
// families on build-tagged revisions, and the reads by content (the source,
// the names), have no value to keep.

/// A scalar slot's field (`track::scalar`), read.
fn scalar_get<H: Host, T: Tracker>(t: &Tex<H, T>, k: u16) -> Option<i32> {
    use crate::track::scalar::*;
    let b = i32::from;
    Some(match k {
        STR_TOP => i32::try_from(t.str_ptr).unwrap_or(i32::MAX),
        HASH_USED => t.hash_used,
        HASH_HIGH => t.hash_high,
        LAST_BADNESS => t.last_badness,
        OUTPUT_ACTIVE => b(t.output_active),
        TERM_OFFSET => t.term_offset,
        FILE_OFFSET => t.file_offset,
        SELECTOR => t.selector,
        INTERACTION => t.interaction,
        HISTORY => t.history,
        ERROR_COUNT => t.error_count,
        SHOWN_MODE => t.shown_mode,
        MAG_SET => t.mag_set,
        ALIGN_STATE => t.align_state,
        DEAD_CYCLES => t.dead_cycles,
        AFTER_TOKEN => t.after_token,
        LONG_HELP_SEEN => b(t.long_help_seen),
        JOB_NAME => t.job_name,
        LOG_NAME => t.log_name,
        OUTPUT_FILE_NAME => t.output_file_name,
        LOG_OPENED => b(t.log_opened),
        OPEN_PARENS => t.open_parens,
        SYS_TIME => t.sys_time,
        SYS_DAY => t.sys_day,
        SYS_MONTH => t.sys_month,
        SYS_YEAR => t.sys_year,
        EPOCH_S => t.epoch.0,
        EPOCH_US => t.epoch.1,
        GLUE_LINEAGE => i32::try_from(t.glue_lineage).unwrap_or(i32::MAX),
        k if (WRITE_OPEN..WRITE_OPEN + 18).contains(&k) => {
            b(t.write_open[usize::from(k - WRITE_OPEN)])
        }
        k if (READ_OPEN..READ_OPEN + 17).contains(&k) => t.read_open[usize::from(k - READ_OPEN)],
        _ => return None,
    })
}

/// A scalar slot's field, stored.
fn scalar_set<H: Host, T: Tracker>(t: &mut Tex<H, T>, k: u16, v: i32) {
    use crate::track::scalar::*;
    match k {
        STR_TOP => {
            // (the strings the call made are its `Pool` writes, stored
            // first; a lower end flushes those it flushed)
            let v = usize::try_from(v).unwrap_or(0);
            if v < t.str_ptr {
                t.str_ptr = v;
                t.pool_ptr = t.str_start[v];
                t.str_index.truncate(v);
                t.strings_reopened();
            }
        }
        HASH_USED => t.hash_used = v,
        HASH_HIGH => t.hash_high = v,
        LAST_BADNESS => t.last_badness = v,
        OUTPUT_ACTIVE => t.output_active = v != 0,
        TERM_OFFSET => t.term_offset = v,
        FILE_OFFSET => t.file_offset = v,
        SELECTOR => t.selector = v,
        INTERACTION => t.interaction = v,
        HISTORY => t.history = v,
        ERROR_COUNT => t.error_count = v,
        SHOWN_MODE => t.shown_mode = v,
        MAG_SET => t.mag_set = v,
        ALIGN_STATE => t.align_state = v,
        DEAD_CYCLES => t.dead_cycles = v,
        AFTER_TOKEN => t.after_token = v,
        LONG_HELP_SEEN => t.long_help_seen = v != 0,
        JOB_NAME => t.job_name = v,
        LOG_NAME => t.log_name = v,
        OUTPUT_FILE_NAME => t.output_file_name = v,
        LOG_OPENED => t.log_opened = v != 0,
        OPEN_PARENS => t.open_parens = v,
        SYS_TIME => t.sys_time = v,
        SYS_DAY => t.sys_day = v,
        SYS_MONTH => t.sys_month = v,
        SYS_YEAR => t.sys_year = v,
        EPOCH_S => t.epoch.0 = v,
        EPOCH_US => t.epoch.1 = v,
        GLUE_LINEAGE => t.glue_lineage = u64::try_from(v).unwrap_or(0),
        k if (WRITE_OPEN..WRITE_OPEN + 18).contains(&k) => {
            t.write_open[usize::from(k - WRITE_OPEN)] = v != 0;
        }
        k if (READ_OPEN..READ_OPEN + 17).contains(&k) => {
            t.read_open[usize::from(k - READ_OPEN)] = v;
        }
        _ => {}
    }
}

/// Whether eqtb location `p`'s level is in `xeq_level` (regions 5 and 6,
/// and the count and dimen registers above 255).
fn word_level(p: i32) -> bool {
    use crate::xregs::{EXT_BASE, ext_reg, is_word_kind};
    if p >= EXT_BASE {
        return is_word_kind(ext_reg(p).0);
    }
    (crate::web::INT_BASE..=crate::web::EQTB_SIZE).contains(&p)
}

/// The value slot `s` holds in `t` now, if its family keeps one.
fn slot_value<H: Host, T: Tracker>(t: &Tex<H, T>, s: Slot) -> Option<SValue> {
    use crate::track::list;
    use crate::track::save;
    let i32of = |x: i64| i32::try_from(x).unwrap_or(0);
    Some(match s.0 {
        Fam::Eqtb => {
            let p = i32of(s.1);
            SValue::Word {
                w: t.peek_eqtb(p),
                level: word_level(p).then(|| t.peek_xeq_level(p)),
                obj: t.peek_obj(p).cloned(),
            }
        }
        Fam::Hash | Fam::HashNext => {
            let i = usize::try_from(s.1 - i64::from(crate::web::HASH_BASE)).ok()?;
            let w = t.hash[i];
            SValue::Int(if s.0 == Fam::Hash { w.rh() } else { w.lh() })
        }
        Fam::Pool => {
            let n = usize::try_from(s.1).ok()?;
            if n >= t.str_ptr {
                return None;
            }
            SValue::Bytes(alloc::sync::Arc::from(t.str_bytes(n)))
        }
        Fam::Alloc => SValue::Int(scalar_get(t, u16::try_from(s.1).ok()?)?),
        Fam::List => {
            let l = &t.cur_list;
            match u8::try_from(s.1).ok()? {
                list::LIST => SValue::Nodes(l.list.clone()),
                list::MLIST => SValue::Mlist(l.mlist.clone()),
                list::MODE => SValue::Int(l.mode),
                list::PG => SValue::Int(l.pg),
                list::ML => SValue::Int(l.ml),
                list::PREV_DEPTH => SValue::Int(l.prev_depth),
                list::SPACE_FACTOR => SValue::Int(l.space_factor),
                list::CLANG => SValue::Int(l.clang),
                list::INCOMPLEAT => SValue::Noad(l.incompleat.clone()),
                list::MIDDLE => SValue::Int(i32::from(l.middle)),
                list::LR_SAVE => SValue::LrSave(l.lr_save.clone()),
                list::LR_BOX => SValue::LrBox(l.lr_box.clone()),
                list::COUNT => SValue::Nest(t.nest.clone()),
                // (the alignment's fields, after the nest)
                _ => SValue::Field(crate::values::value(t, s)?),
            }
        }
        Fam::Save => {
            let k = u32::try_from(s.1).ok()?;
            match k {
                save::SAVE_PTR => SValue::Int(t.save_ptr),
                save::CUR_LEVEL => SValue::Int(t.cur_level),
                save::CUR_GROUP => SValue::Int(t.cur_group),
                save::CUR_BOUNDARY => SValue::Int(t.cur_boundary),
                save::XCHAIN => SValue::XChain {
                    level: t.xregs.chain_level,
                    chain: t.xregs.chain.clone(),
                    outer: t.xregs.outer.clone(),
                },
                k if k >= save::ENTRY => {
                    let p = usize::try_from(k - save::ENTRY).ok()?;
                    SValue::Save {
                        w: if p < t.save_stack.len() {
                            t.save_stack[p]
                        } else {
                            crate::mem::MemoryWord::default()
                        },
                        obj: t.save_obj.get(p).cloned().flatten(),
                        eqtb: t.save_eqtb.get(p).copied().unwrap_or(false),
                    }
                }
                _ => return None,
            }
        }
        Fam::Cond => SValue::Cond {
            stack: t.cond_stack.clone(),
            limit: t.if_limit,
            cur_if: t.cur_if,
            line: t.if_line,
        },
        Fam::Mark => {
            let (c, k) = (i32of(s.1 / 5), usize::try_from(s.1 % 5).ok()?);
            SValue::Mark(t.cur_mark.get(&c).and_then(|m| m[k].clone()))
        }
        _ => SValue::Field(crate::values::value(t, s)?),
    })
}

/// Put value `v` back at slot `s` (`Store::set`): one store of the field,
/// the value shared; a table slot's version array takes the recorded
/// version.
fn set_value<H: Host, T: Tracker>(t: &mut Tex<H, T>, vers: &mut Versions, s: Slot, v: &SVal) {
    use crate::track::list;
    use crate::track::save;
    let Some(val) = v.1.as_deref() else {
        return;
    };
    let i32of = |x: i64| i32::try_from(x).unwrap_or(0);
    match (s.0, val) {
        (Fam::Eqtb, SValue::Word { w, level, obj }) => {
            let p = i32of(s.1);
            if p >= crate::xregs::EXT_BASE {
                t.xregs.set(p, *w);
                if let Some(l) = level {
                    t.xregs.set_level(p, *l);
                }
                t.xregs.set_obj(p, obj.clone());
            } else {
                let old = t.peek_eqtb(p);
                t.note_meaning(p, old, *w);
                let i = usize::try_from(p).unwrap_or(0);
                t.eqtb[i] = *w;
                if let Some(x) = t.eqtb_obj.get_mut(i) {
                    x.clone_from(obj);
                }
                if let Some(l) = level {
                    let j = usize::try_from(p - crate::web::INT_BASE).unwrap_or(0);
                    t.xeq_level[j] = *l;
                }
            }
            vers.set(s, v.0.0);
        }
        (Fam::Hash | Fam::HashNext, SValue::Int(x)) => {
            let i = usize::try_from(s.1 - i64::from(crate::web::HASH_BASE)).unwrap_or(0);
            if s.0 == Fam::Hash {
                t.hash[i].set_rh(*x);
            } else {
                t.hash[i].set_lh(*x);
            }
            vers.set(s, v.0.0);
        }
        (Fam::Pool, SValue::Bytes(b)) => {
            let n = usize::try_from(s.1).unwrap_or(0);
            if n < t.str_ptr {
                if t.str_bytes(n) == &b[..] {
                    vers.set(s, v.0.0);
                    return;
                }
                // (the call flushed string `n` and made another)
                t.str_ptr = n;
                t.pool_ptr = t.str_start[n];
                t.str_index.truncate(n);
                t.strings_reopened();
            }
            if n == t.str_ptr {
                t.pool_ptr = t.str_start[n];
                t.grow_pool(b.len());
                for &c in b.iter() {
                    t.str_pool[t.pool_ptr] = c;
                    t.pool_ptr += 1;
                }
                t.str_ptr += 1;
                t.grow_starts(t.str_ptr + 1);
                t.str_start[t.str_ptr] = t.pool_ptr;
            }
            vers.set(s, v.0.0);
        }
        (Fam::Alloc, SValue::Int(x)) => {
            scalar_set(t, u16::try_from(s.1).unwrap_or(u16::MAX), *x);
            vers.set(s, v.0.0);
        }
        (Fam::List, val) => {
            let l = &mut t.cur_list;
            match (u8::try_from(s.1).unwrap_or(0), val) {
                (list::LIST, SValue::Nodes(n)) => l.list = n.clone(),
                (list::MLIST, SValue::Mlist(m)) => l.mlist.clone_from(m),
                (list::MODE, SValue::Int(x)) => l.mode = *x,
                (list::PG, SValue::Int(x)) => l.pg = *x,
                (list::ML, SValue::Int(x)) => l.ml = *x,
                (list::PREV_DEPTH, SValue::Int(x)) => l.prev_depth = *x,
                (list::SPACE_FACTOR, SValue::Int(x)) => l.space_factor = *x,
                (list::CLANG, SValue::Int(x)) => l.clang = *x,
                (list::INCOMPLEAT, SValue::Noad(x)) => l.incompleat.clone_from(x),
                (list::MIDDLE, SValue::Int(x)) => l.middle = *x != 0,
                (list::LR_SAVE, SValue::LrSave(x)) => l.lr_save.clone_from(x),
                (list::LR_BOX, SValue::LrBox(x)) => l.lr_box.clone_from(x),
                (list::COUNT, SValue::Nest(n)) => t.nest.clone_from(n),
                (_, SValue::Field(x)) => {
                    crate::values::set(t, s, x);
                }
                _ => {}
            }
        }
        (Fam::Save, val) => match (u32::try_from(s.1).unwrap_or(u32::MAX), val) {
            (save::SAVE_PTR, SValue::Int(x)) => t.save_ptr = *x,
            (save::CUR_LEVEL, SValue::Int(x)) => t.cur_level = *x,
            (save::CUR_GROUP, SValue::Int(x)) => t.cur_group = *x,
            (save::CUR_BOUNDARY, SValue::Int(x)) => t.cur_boundary = *x,
            (
                save::XCHAIN,
                SValue::XChain {
                    level,
                    chain,
                    outer,
                },
            ) => {
                t.xregs.chain_level = *level;
                t.xregs.chain.clone_from(chain);
                t.xregs.outer.clone_from(outer);
            }
            (k, SValue::Save { w, obj, eqtb }) if k >= save::ENTRY => {
                let p = usize::try_from(k - save::ENTRY).unwrap_or(0);
                if p < t.save_stack.len() {
                    t.save_stack[p] = *w;
                }
                if t.save_obj.len() <= p && obj.is_some() {
                    t.save_obj.resize(p + 1, None);
                }
                if let Some(x) = t.save_obj.get_mut(p) {
                    x.clone_from(obj);
                }
                if t.save_eqtb.len() <= p && *eqtb {
                    t.save_eqtb.resize(p + 1, false);
                }
                if let Some(x) = t.save_eqtb.get_mut(p) {
                    *x = *eqtb;
                }
            }
            _ => {}
        },
        (
            Fam::Cond,
            SValue::Cond {
                stack,
                limit,
                cur_if,
                line,
            },
        ) => {
            t.cond_stack = stack.clone();
            t.if_limit = *limit;
            t.cur_if = *cur_if;
            t.if_line = *line;
        }
        (Fam::Mark, SValue::Mark(m)) => {
            let (c, k) = (i32of(s.1 / 5), usize::try_from(s.1 % 5).unwrap_or(0));
            t.cur_mark.entry(c).or_default()[k].clone_from(m);
        }
        // (a table family's version is in its array)
        (_, SValue::Field(x)) if crate::values::set(t, s, x) => vers.set(s, v.0.0),
        _ => {}
    }
}

/// The store a hit's writes go to: the engine, and the version arrays of
/// the tables.
struct Apply<'a, H: Host, T: Tracker> {
    tex: &'a mut Tex<H, T>,
    st: &'a mut RecState,
}

impl<H: Host, T: Tracker> Store<TexSsa> for Apply<'_, H, T> {
    fn version(&self, loc: &Loc<Slot>) -> Version {
        View {
            tex: &*self.tex,
            rec: &*self.st,
        }
        .version(loc)
    }
    fn get(&self, a: &Slot) -> Option<SVal> {
        View {
            tex: &*self.tex,
            rec: &*self.st,
        }
        .get(a)
    }
    fn set(&mut self, a: &Slot, v: Option<SVal>) {
        if let Some(v) = v {
            set_value(self.tex, &mut self.st.vers, *a, &v);
        }
    }
}

/// A record's effects and its children's, in program order.
fn record_effects(rt: &Runtime<TexSsa>, id: RecId, out: &mut Vec<Effect>) {
    for it in &rt.record(id).items {
        match it {
            partex_ssa::runtime::Item::Out(e) => out.push(e.clone()),
            partex_ssa::runtime::Item::Call(c) => record_effects(rt, *c, out),
            _ => {}
        }
    }
}

/// Emit effects again, each through its output's own sink, as the body
/// made them (not noted as effects a second time).
fn replay_effects<H: Host, T: Tracker>(t: &mut Tex<H, T>, effects: &[Effect]) {
    for e in effects {
        let Effect::Bytes(what, bytes) = e else {
            continue;
        };
        match *what {
            Output::Log => t.log_bytes_raw(bytes),
            Output::Term => t.term_bytes_raw(bytes),
            Output::Write(n) => t.write_bytes_raw(usize::from(n), bytes),
            Output::Dvi => t.dvi_bytes_raw(bytes),
            Output::Pdf => t.pdf_bytes_raw(bytes),
            Output::DviStart | Output::DviPage | Output::DviPageNow | Output::DviFinish => {
                t.page_sink_raw(*what, bytes);
            }
        }
    }
}

/// The open step's effects in the link's form made since its last chunk,
/// cut as a chunk: an effect of the call running now, and the step's next
/// chunk (DESIGN 7.17.3, "Hits applied inside a step that runs again",
/// item 2). Before a call that applies begins, and before its end.
pub(crate) fn cut_chunk<H: Host, T: Tracker>(t: &mut Tex<H, T>) {
    if t.tracker.ssa().is_none() {
        return;
    }
    let fx = t.take_effects();
    let Some(tr) = t.tracker.ssa() else {
        return;
    };
    tr.flush_effects();
    if fx.is_empty() {
        return;
    }
    let mut r = tr.rec.borrow_mut();
    let rr = &mut *r;
    let se = StepEffects::new(fx);
    rr.rt.note_effect(Effect::Step(se.clone()));
    rr.st.steps.cur_chunks.push(se);
}

/// A hit put in place: its writes stored, its effects back as the step's
/// chunks (or, with no effects in the link's form, printed again), and
/// its record the running call's child (its children reused unseen). The
/// body does not run. Returns the record's result and its cost (the
/// commands it stands for).
pub(crate) fn apply_hit<H: Host, T: Tracker>(t: &mut Tex<H, T>, id: RecId) -> Option<(SVal, u64)> {
    let tr = t.tracker.ssa()?;
    tr.flush_effects();
    let mut rec = core::mem::take(&mut *tr.rec.borrow_mut());
    let (result, cost, effects) = {
        let Recorder { rt, st, .. } = &mut rec;
        let result = rt.apply(&mut Apply { tex: t, st }, id);
        let mut effects = Vec::new();
        record_effects(rt, id, &mut effects);
        // (its chunks are the step's, in order)
        st.steps
            .cur_chunks
            .extend(effects.iter().filter_map(|e| match e {
                Effect::Step(se) => Some(se.clone()),
                Effect::Bytes(..) => None,
            }));
        (result, rt.record(id).cost, effects)
    };
    // (every output is an effect in the link's form, in the chunks; the
    // bytes by output are check mode's, printed again only without them)
    if t.effects.is_none() {
        replay_effects(t, &effects);
    }
    if let Some(tr) = t.tracker.ssa() {
        *tr.rec.borrow_mut() = rec;
        tr.boundary();
        tr.applied.set(tr.applied.get() + 1);
        tr.skipped.set(tr.skipped.get() + cost);
    }
    Some((result, cost))
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Set the input to where a paragraph call ended (DESIGN 7.17.12: the
    /// input position is the call's result): the top level's file advanced
    /// by the lines the call read, as §362 reads them, and its line in the
    /// buffer, `loc`, `limit` and the state as the call left them.
    pub(crate) fn apply_position(&mut self, pos: &Position) {
        let top = pos.levels.len().saturating_sub(1);
        let k = pos.levels.get(top).map_or(0, |l| l.0);
        for _ in 0..k {
            self.cur_input.state = crate::web::NEW_LINE;
            if self.next_line().is_err() {
                break;
            }
        }
        let start = usize::try_from(self.cur_input.start).unwrap_or(0);
        for (i, &b) in pos.line.iter().enumerate() {
            self.buffer[start + i] = b;
        }
        self.cur_input.limit = self.cur_input.start + pos.limit;
        self.first = usize::try_from(self.cur_input.limit + 1)
            .unwrap_or(0)
            .max(start);
        self.cur_input.loc = self.cur_input.start + pos.loc;
        self.cur_input.state = pos.state;
    }
}
