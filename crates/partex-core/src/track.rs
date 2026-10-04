//! State access tracking (see `DESIGN.md`, "State tracking").
//!
//! The core reads and writes engine state only through accessors that call a
//! [`Tracker`]. Reference builds use [`Untracked`], whose hooks are empty and
//! inline away; the memoization layers plug in a recording tracker. Hooks take
//! `&self` so that read accessors can stay `&self`; a recording tracker keeps
//! its log behind interior mutability.

/// A unit of observable engine state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cell {
    /// One `eqtb` entry (meaning, register, parameter, code table entry).
    Eqtb(i32),
    /// One hash-table slot's name (`text`).
    Hash(i32),
    /// What can change about loaded font `f`: its `\fontdimen`
    /// parameters, `\hyphenchar`, `\skewchar`, pdfTeX's character codes
    /// and expansion. (Its metrics are fixed once it is loaded.)
    Font(i32),
    /// Which fonts are loaded, with their names and sizes: what `\font`
    /// looks up before it loads a font, and what numbers new fonts get.
    FontTable,
    /// `\read` stream `n` (0–15): whether it is open, and how far it has
    /// been read.
    Read(i32),
    /// `\write` stream `n` (0–15): whether it is open.
    Out(i32),
    /// pdfTeX's random number generator.
    Random,
    /// The strings with the characters that hash to this (web2c's
    /// `search_string`, which looks a file name up among the pool's
    /// strings): where two builds' strings differ, only a search for one
    /// of their characters can tell.
    Str(i32),
    /// One hash-table slot's link (`next`): what a lookup reads to go on
    /// past the slot. (Apart from the name: another name made at the end
    /// of a chain changes only its last slot's link, which a lookup that
    /// finds that slot's own name never reads.)
    HashNext(i32),
}

partex_engine::persist_enum!(Cell {
    Eqtb(a0),
    Hash(a0),
    Font(a0),
    FontTable,
    Read(a0),
    Out(a0),
    Random,
    Str(a0),
    HashNext(a0)
});

/// A slot of the tables beside eqtb and the hash (DESIGN 7.17.12, the
/// tables' convention): the string pool by number, and the allocators
/// whose value decides the number the next string or control sequence
/// gets.
/// A query of the host that is not a load (DESIGN 7.17.3, "A query is
/// asked again"): what a rebuild asks again.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Query {
    /// `\pdffilemoddate`: the date of the file the name finds.
    ModDate(alloc::sync::Arc<[u8]>),
    /// The host's time: the job's start where it is printed, the
    /// creation date (`Tex::clock_answer`).
    Now,
    /// The timer: `\pdfelapsedtime`, `\pdfresettimer`.
    Timer,
    /// The timer's start at the job's start, a query of the first step,
    /// not asked again (DESIGN 7.17.3, "Not built").
    TimerStart,
    /// A line from the terminal (§71): not asked outside a run.
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Row {
    /// String `n` of the pool: its bytes.
    Str(usize),
    /// A scalar slot ([`scalar`]): its version is its value.
    Scalar(u16),
    /// The conditionals' state: the stack, `if_limit`, `cur_if`,
    /// `if_line` (§489).
    Cond,
    /// Current mark `t` of class `c` (§382, e-TeX's `\\marks`), at slot
    /// `5c + t`: a shared token list carrying its version.
    Mark(u32),
    /// A field of a level of the semantic nest (§213, [`list`]): level
    /// `d`'s field `f` at `d * list::STRIDE + f`, level 0 the outer one
    /// and `cur_list` the deepest. A value, versioned by the value itself
    /// (the list carries its version). A level's fields stay where they
    /// are while levels above it come and go.
    List(u32),
    /// The enclosing levels of the nest (§211).
    Nest,
    /// A field of the alignment state (§770, [`align`]): a value,
    /// versioned by the value itself.
    Align(u8),
    /// A field of the page builder's state (§980–§982, [`page`]), or
    /// e-TeX's `\\splitdiscards`: a value, versioned by the value itself.
    Page(u8),
    /// The save stack (§268, [`save`]): its pointers, e-TeX's chain of
    /// registers above 255, and entry `p` at slot `save::ENTRY + p`.
    Save(u32),
    /// A field of a font slot ([`font`]): field `k` of slot `f` is
    /// `f * font::FIELDS + k` (§549–§550).
    Font(u32),
    /// The table of loaded fonts (§549): the fonts in the order loaded,
    /// `font_ptr`, `fmem_ptr`.
    FontTable,
    /// Hyphenation ([`hyph`]): the patterns and the exception table as a
    /// whole (§920–§941); a word's entry is addressed by the word
    /// ([`Tracker::hyph_word_read`]).
    Hyph(u32),
    /// A table of the PDF writer (`pdf::val::field`): a value carrying
    /// its version, made when the routine that writes it is done.
    Pdf(u8),
    /// A table of the DVI writer (`pdf::val::dvi_field`, less
    /// `pdf::val::DVI`).
    Dvi(u8),
    /// The glyphs PDF ship `n` used, by font (`pdf::ship::Ship::glyphs`):
    /// an append (DESIGN 7.17.3 item 5), read only by the job's end.
    Glyphs(u32),
    /// The page's `k`-th node since it began (§980): an append (DESIGN
    /// 7.17.3 item 5), read by what reads the page whole.
    PageNode(u32),
    /// `\\write` stream `n`'s file (`streams::LOG` for the log): the
    /// address its lines are stored to, or none (§1370, §534).
    Out(u8),
    /// `\\openin` stream `n` (§480): the file's contents as loaded and
    /// the position in them.
    Read(u8),
    /// pdfTeX's random number generator (pdfTeX §110).
    Random,
    /// The host's answer to a query that is not a load, read during a
    /// call: the clock (`\\pdfelapsedtime`), the creation
    /// date, a file's date, a line from the terminal (§71). Its version
    /// is the answer, and no other build's is equal to it.
    Clock,
    /// A sealed line's contents (`seal.rs`), by its key: what is inside a
    /// line box, written when the line breaker seals it and read where
    /// it is opened (shipping out, `\\unhbox`, a display's width).
    Sealed(u64),
}

/// The fields of a font slot, by [`Row::Font`] slot (DESIGN 7.17.12's
/// `fonts` row: the metrics by identity, the settable fields values of
/// their own).
pub mod font {
    /// The metrics (§539–§545) by identity, with the tags and widths of a
    /// font changed in place (`\pdfnoligatures`, `\tagcode`,
    /// `\letterspacefont`), its name and size.
    pub const METRICS: u32 = 0;
    /// `\fontdimen` (§558, `param`).
    pub const PARAMS: u32 = 1;
    /// `\hyphenchar`, `\skewchar` (§549).
    pub const HYPHEN_CHAR: u32 = 2;
    pub const SKEW_CHAR: u32 = 3;
    /// pdfTeX's expansion of the font and its chain of expanded fonts.
    pub const EXPAND: u32 = 4;
    /// The interword glue (§1042, `font_glue`).
    pub const GLUE: u32 = 5;
    /// pdfTeX's character codes, `CODES + code` (`fonts::Code`).
    pub const CODES: u32 = 8;
    /// The fields of a slot.
    pub const FIELDS: u32 = 16;
}

/// Hyphenation's slots, by [`Row::Hyph`] slot.
pub mod hyph {
    /// The patterns (§920–§921): the linked trie while INITEX builds it
    /// (§943–§966), the packed trie after, and e-TeX's saved hyphenation
    /// codes.
    pub const PATTERNS: u32 = 0;
    /// The exception table as a whole (§926, §940: `hyph_count`,
    /// `hyph_next`, the map): what an insertion reads and writes. A word's
    /// entry is addressed by the word ([`super::Tracker::hyph_word_read`]).
    pub const EXCEPTIONS: u32 = 1;
    /// The slots end here.
    pub const COUNT: u32 = 2;
}

/// The save stack's slots, by [`Row::Save`] slot.
pub mod save {
    pub const SAVE_PTR: u32 = 0;
    pub const CUR_LEVEL: u32 = 1;
    pub const CUR_GROUP: u32 = 2;
    pub const CUR_BOUNDARY: u32 = 3;
    /// e-TeX's saved registers above 255 (`xregs.rs`'s chain).
    pub const XCHAIN: u32 = 4;
    /// Entry `p` is slot `ENTRY + p`.
    pub const ENTRY: u32 = 8;
}

/// The fields of `cur_list`, by [`Row::List`] slot.
pub mod list {
    pub const LIST: u8 = 0;
    pub const MLIST: u8 = 1;
    pub const MODE: u8 = 2;
    pub const PG: u8 = 3;
    pub const ML: u8 = 4;
    pub const PREV_DEPTH: u8 = 5;
    pub const SPACE_FACTOR: u8 = 6;
    pub const CLANG: u8 = 7;
    pub const INCOMPLEAT: u8 = 8;
    pub const MIDDLE: u8 = 9;
    pub const LR_SAVE: u8 = 10;
    pub const LR_BOX: u8 = 11;
    /// The fields end here; the nest is slot `COUNT`.
    pub const COUNT: u8 = 12;
    /// The slots of a level's fields are this far apart: level 0's, the
    /// nest's and the alignment's ([`super::align`]) come below the first.
    pub const STRIDE: u32 = 32;

    /// The slot of level `d`'s field `f`.
    #[must_use]
    pub fn slot(d: usize, f: u8) -> u32 {
        u32::try_from(d)
            .unwrap_or(u32::MAX)
            .saturating_mul(STRIDE)
            .saturating_add(u32::from(f))
    }
}

/// The fields of the alignment state (DESIGN 7.17.12's `align` row), by
/// [`Row::Align`] slot.
pub mod align {
    /// `cur_align` (§770): the current column.
    pub const COLUMN: u8 = 0;
    /// `cur_span` (§770): the column where the current span began.
    pub const SPAN: u8 = 1;
    /// `cur_loop` (§770): where a periodic preamble repeats.
    pub const LOOP: u8 = 2;
    /// The material migrating out of the current row (`cur_head`, §770).
    pub const ADJUST: u8 = 3;
    /// The preamble's columns: their templates, `extra_info` and widths
    /// (§769).
    pub const COLUMNS: u8 = 4;
    /// The preamble's tabskip glue (§778).
    pub const TABSKIPS: u8 = 5;
    /// The enclosing alignments (the alignment stack, §770).
    pub const STACK: u8 = 6;
    /// The fields of the current alignment.
    pub const LEVEL: [u8; 6] = [COLUMN, SPAN, LOOP, ADJUST, COLUMNS, TABSKIPS];
    /// The fields end here.
    pub const COUNT: u8 = 7;
}

/// The fields of the page builder's state (DESIGN 7.17.12's `page` row,
/// §980–§982) and e-TeX's `split_disc` (§977), by [`Row::Page`] slot:
/// `\\pagegoal` reads one field's version. The builder's fields and the
/// two views of its list a step reads are numbered by the engine
/// (`builder::field`, what a step's `builder::Access` names).
pub mod page {
    pub use partex_engine::builder::field::{
        BEST_BREAK, BEST_SIZE, CONTENTS, DISCARDS, INS, INSERT_PENALTIES, LAST_GLUE, LAST_KERN,
        LAST_NODE_TYPE, LAST_PENALTY, LEAST_COST, LIST, LIST_LEN, LIST_TAIL, MAX_DEPTH, SO_FAR,
    };
    /// The builder's fields end here.
    pub const BUILDER: u8 = partex_engine::builder::field::COUNT;
    /// e-TeX's `split_disc` (`\\splitdiscards`, §977).
    pub const SPLIT_DISCARDS: u8 = 23;
    /// The slots end here.
    pub const COUNT: u8 = 24;
}

/// The scalar rows of DESIGN 7.17.12, by slot ([`Row::Scalar`]).
pub mod scalar {
    /// The pool's end, `str_ptr` (§38).
    pub const STR_TOP: u16 = 0;
    /// `SyncTeX`'s tag counter (`synctex.rs`).
    pub const SYNCTEX_TAGS: u16 = 3;
    /// `SyncTeX`'s controller's flags in SSA mode (`synctex.rs`: off,
    /// content ready, warned), for its warnings in program order.
    pub const SYNCTEX_FLAGS: u16 = 4;
    /// The hash's allocator, `hash_used` (§256).
    pub const HASH_USED: u16 = 1;
    /// The allocator of the hash's extra area, `hash_high` (web2c).
    pub const HASH_HIGH: u16 = 2;
    /// `last_badness` (§646, `\\badness`).
    pub const LAST_BADNESS: u16 = 5;
    /// `output_active` (§989).
    pub const OUTPUT_ACTIVE: u16 = 6;
    /// `term_offset` and `file_offset` (§54): the columns where the next
    /// print breaks its line.
    pub const TERM_OFFSET: u16 = 7;
    pub const FILE_OFFSET: u16 = 8;
    /// `selector` (§54).
    pub const SELECTOR: u16 = 9;
    /// `interaction` (§73).
    pub const INTERACTION: u16 = 10;
    /// `history` and `error_count` (§76).
    pub const HISTORY: u16 = 11;
    pub const ERROR_COUNT: u16 = 12;
    /// `shown_mode` (§213).
    pub const SHOWN_MODE: u16 = 13;
    /// `mag_set` (§286).
    pub const MAG_SET: u16 = 14;
    /// `align_state` (§309).
    pub const ALIGN_STATE: u16 = 15;
    /// `dead_cycles` (§592).
    pub const DEAD_CYCLES: u16 = 16;
    /// `after_token` (§1266).
    pub const AFTER_TOKEN: u16 = 17;
    /// `long_help_seen` (§1281).
    pub const LONG_HELP_SEEN: u16 = 18;
    /// `job_name`, `log_name`, `output_file_name`, `log_opened` (§527,
    /// §532).
    pub const JOB_NAME: u16 = 19;
    pub const LOG_NAME: u16 = 20;
    pub const OUTPUT_FILE_NAME: u16 = 21;
    pub const LOG_OPENED: u16 = 22;
    /// `open_parens` (§304).
    pub const OPEN_PARENS: u16 = 23;
    /// The job's start (§241) and `\\pdfelapsedtime`'s (the host's
    /// clock).
    pub const SYS_TIME: u16 = 24;
    pub const SYS_DAY: u16 = 25;
    pub const SYS_MONTH: u16 = 26;
    pub const SYS_YEAR: u16 = 27;
    pub const EPOCH_S: u16 = 28;
    pub const EPOCH_US: u16 = 29;
    /// The next glue lineage (`objs.rs`: glue's identity, as data).
    pub const GLUE_LINEAGE: u16 = 30;
    /// `write_open[j]` (§1342), `WRITE_OPEN + j` for `j` in 0..18.
    pub const WRITE_OPEN: u16 = 32;
    /// `read_open[j]` (§480), `READ_OPEN + j` for `j` in 0..17.
    pub const READ_OPEN: u16 = 64;
    /// The results of the recorded routines, each versioned by its value
    /// (DESIGN 7.17.12's boundary rule: what is live where a routine ends
    /// is its result, `cur_box` for a pack): `hpack`'s box with what
    /// migrated out of it and its glue totals (§649), `vpack`'s box
    /// (§668), `line_break`'s last line, `just_box` (§815); a
    /// `build_page` step's nodes back in front of the contributions and
    /// what follows it (§994), and the output routine's nodes in front of
    /// them (§1026).
    pub const HPACK_RESULT: u16 = 88;
    pub const VPACK_RESULT: u16 = 89;
    pub const LINE_BREAK_RESULT: u16 = 90;
    pub const PAGE_STEP_RESULT: u16 = 91;
    pub const OUTPUT_RESULT: u16 = 92;
    /// The slots end here.
    pub const COUNT: u16 = 96;
}

/// The version of a scalar row's value: the value itself, shifted clear
/// of the bit the recorder marks versions with (so no two values share a
/// version).
#[must_use]
pub fn scalar_version(v: usize) -> u128 {
    (v as u128) << 1
}

/// A scalar row's version, for a value that may be negative.
#[must_use]
pub fn scalar_version_i32(v: i32) -> u128 {
    scalar_version(v.cast_unsigned() as usize)
}

/// Hooks called by state accessors.
pub trait Tracker {
    /// Whether the value hooks below are called. Accessors test it at
    /// compile time, so a tracker without it pays for no extra loads.
    const VALUES: bool = false;
    /// Whether [`Tracker::macro_call`] is called (a TeX-level profiler).
    const PROFILE: bool = false;
    /// Whether [`Tracker::read`] records anything. Without, the core may
    /// answer from caches that skip reads (`skipcache.rs`).
    const READS: bool = true;
    /// Whether the line hooks below are called (token-level input
    /// dependencies, DESIGN.md §7.2).
    const LINES: bool = false;
    /// Whether the save-stack hooks below are called
    /// ([`Tracker::soft_read`]).
    const SOFT_READS: bool = false;
    /// Whether the token store records the ids of the lists it marks
    /// changed (`tok.rs`, `TokStore::changed`), so that a snapshot visits
    /// those instead of scanning every list's flag. A tracker whose
    /// engine takes snapshots at every cut sets it (the machine's); one
    /// that takes none, or few, does not, and its token path stores the
    /// flag with no test.
    const CHANGED_IDS: bool = false;
    /// Whether a control sequence lookup tells [`Tracker::name_lookup`]
    /// (the name and where it was found) instead of reading
    /// `Cell::Str` of its characters, and hash slots are read with their
    /// content ([`Tracker::read_content`]).
    const NAMES: bool = false;
    /// Whether a lookup where only the token is wanted (an argument's or
    /// a body's token, an assignment's target) reads the control
    /// sequence's class ([`Tracker::read_class`]) instead of its meaning.
    const CLASSES: bool = false;
    /// Before a read of a cell.
    fn read(&self, cell: Cell);
    /// A local assignment at group level `level` to `cell`, whose old
    /// value it may save (and, in e-TeX, compares with the new one): the
    /// value inside the group is the new one and, after it, the old one
    /// either way, so the region depends on the old value only if the
    /// group is still open at its end (the save stack holds it).
    fn soft_read(&self, cell: Cell, _level: i32) {
        self.read(cell);
    }
    /// `eqtb[cell]`'s value is saved at group level `level`.
    fn saved(&self, _cell: Cell, _level: i32) {}
    /// The value saved at group level `level` is back in `eqtb[cell]`.
    fn restored(&self, _cell: Cell, _level: i32) {}
    /// Group level `level` ended.
    fn group_end(&self, _level: i32) {}
    /// Before a write of a cell.
    fn write(&self, cell: Cell);
    /// After a read of `eqtb` or hash word `cell`, with its bits.
    fn read_value(&self, _cell: Cell, _bits: u64) {}
    /// After a read of `eqtb` word `cell` (with [`Tracker::VALUES`]):
    /// `content` hashes its value by content (`Tex::cell_content`), for a
    /// tracker that keeps versions; called lazily, so it costs nothing to
    /// a tracker that does not call it.
    fn read_content(&self, _cell: Cell, _content: impl FnOnce() -> u128) {}
    /// After a write of `eqtb` or hash word `cell`: its bits before and after.
    fn write_value(&self, _cell: Cell, _old: u64, _new: u64) {}
    /// (With [`Tracker::VALUES`].) After a write of `cell`, once the
    /// accessor has stored the value: the content version of what the slot
    /// holds now, made at the write from the content written (DESIGN
    /// 7.17.12: a table keeps a version array beside its entries; an equal
    /// write gives the same version, which is backdating).
    fn wrote(&self, _cell: Cell, _version: u128) {}
    /// (With [`Tracker::VALUES`].) A read of table row `row` ([`Row`]):
    /// `content` gives its content version, for check mode's test of the
    /// version its last write made.
    fn row_read(&self, _row: Row, _content: impl FnOnce() -> u128) {}
    /// (With [`Tracker::VALUES`].) Table row `row` was written and holds
    /// content version `version` now (made at the write).
    fn row_wrote(&self, _row: Row, _version: u128) {}
    /// Font slot `f` was made (loaded, expanded, copied) by the run now.
    fn font_loaded(&self, _f: i32) {}
    /// Whether loaded font `f` is one the program has made by now, for
    /// `\font`'s search of the fonts loaded (§1260): a rebuild's step run
    /// again does not find what a later step, or its own older run, made.
    fn font_visible(&self, _f: i32) -> bool {
        true
    }
    /// Whether font `f` is the newest the program has made by now (§579:
    /// only it may grow its parameters), if the tracker knows (`None`:
    /// the table's last loaded font is).
    fn font_newest(&self, _f: i32) -> Option<bool> {
        None
    }
    /// (With [`Tracker::VALUES`].) Table row `row` holds content version
    /// `version`, stored wholesale past the accessors (a format's load):
    /// its version, not a write of the running call.
    fn row_made(&self, _row: Row, _version: u128) {}
    /// (With [`Tracker::VALUES`].) A read of structure row `row` (a value
    /// that carries its version: `cur_list`'s fields, the nest):
    /// `version` gives it, made from the value.
    fn value_read(&self, _row: Row, _version: impl FnOnce() -> u128) {}
    /// (With [`Tracker::VALUES`].) The host answered query `q`, the
    /// answer's version `answer` (a `Row::Clock` read too).
    fn queried(&self, _q: Query, _answer: u128) {}
    /// (With [`Tracker::VALUES`].) The job's end made each font's glyphs
    /// used, the union of the ships' rows ([`Row::Glyphs`]), of version
    /// `union`: a rebuild makes the union again when a ship's glyphs
    /// change, and runs the end only if it differs (DESIGN 4.3, "The
    /// job's end").
    fn glyphs_united(&self, _union: u128) {}
    /// (With [`Tracker::VALUES`].) The handle the running step's last run
    /// opened file `name` on, if the step runs again: the engine asks the
    /// host for it again (`Host::open_write_again`, DESIGN 7.17.3). Each
    /// is given once, the run's opens of a name in their order.
    fn reopen(&self, _name: &[u8], _kind: crate::host::FileKind) -> Option<crate::host::WriteId> {
        None
    }
    /// (With [`Tracker::VALUES`].) Structure row `row` was written (its
    /// value, with its version, is the engine's).
    fn value_wrote(&self, _row: Row) {}
    /// (With [`Tracker::VALUES`].) The engine calls `f` over inputs of
    /// versions `args` (DESIGN 7.17.2): a recorder evaluates it by name,
    /// verifying the reads of the records it has against `view`, and
    /// records the body that runs until [`Tracker::call_end`].
    /// Returns the record of a hit to put in place instead of running
    /// the body (hits applied, `ssa::apply_hit`): then no call is open.
    fn call_begin(
        &self,
        _f: crate::ssa::Func,
        _args: &[u128],
        _view: &dyn crate::ssa::EngineView,
    ) -> Option<u32> {
        None
    }
    /// The recorder, for the engine's code that puts a hit in place.
    fn ssa(&self) -> Option<&crate::ssa::SsaTracker> {
        None
    }
    /// (With [`Tracker::VALUES`].) The call [`Tracker::call_begin`] opened
    /// returned.
    fn call_end(&self, _view: &dyn crate::ssa::EngineView) {}
    /// (With [`Tracker::VALUES`].) web2c's `search_string` looked for an
    /// older string with the characters `name` (the file-name reuse of
    /// `\\input` and `\\font`) and found string `found` (0: none): a read
    /// by content, verified by searching again (`Tex::peek_search`).
    fn string_search(&self, _name: &[u8], _found: i32) {}
    /// A file opened at input level `depth` (`Some(name)`), or the file at
    /// that level ended (`None`).
    fn file(&self, _depth: usize, _name: Option<&[u8]>) {}
    /// A page is about to be shipped out.
    fn page(&self) {}
    /// (With [`Tracker::VALUES`].) Main control begins command `n`
    /// (§1030), with the input at level `depth`, line `line`, the group
    /// level `level`, and `outer` whether it begins at an outer clean
    /// point (outer vertical mode, the nest and the contributions empty,
    /// the input a file, no output routine): an observer's clock (the
    /// event log, `PARTEX_EVENTS`).
    fn command(&self, _n: u64, _depth: usize, _line: i32, _level: i32, _outer: bool) {}
    /// (With [`Tracker::VALUES`].) Whether main control stops before
    /// command `n`, a checkpoint: a rebuild's run of a step that read a
    /// value it must not, and ran past its budget (`ssa::rebuild`, "A
    /// run that read a later definition").
    fn stop_due(&self, _n: u64) -> bool {
        false
    }
    /// (With [`Tracker::VALUES`].) The open step's name, for the keys of
    /// the lines it seals (`seal.rs`): the same at each run of the step,
    /// and no other step's.
    fn step_salt(&self) -> u64 {
        0
    }
    /// Before an access to the page builder's state (§980–§982: the
    /// page, its totals, insertions, `last_glue` and friends, e-TeX's
    /// `page_disc`), a write if `write` (which reads it too).
    fn page_access(&self, _write: bool) {}
    /// Before a read or a write of pdfTeX's `last_item` value `k`
    /// (`pdf::PdfLast`: `\pdflastlink` and its siblings).
    fn pdf_last_access(&self, _k: u8, _write: bool) {}
    /// Macro `cs` is about to be expanded.
    fn macro_call(&self, _cs: i32) {}
    /// Macro `cs` has its arguments, whose tokens hash to `hash`.
    fn macro_args(&self, _cs: i32, _hash: u64) {}
    /// The last read of `cell` was a lookup the reader does not depend on
    /// (the target of `\\def`, `\\let` and the like, pdfTeX §1215).
    fn retract(&self, _cell: Cell) {}
    /// A lookup of control sequence `p` where only its token is wanted,
    /// whose meaning is of class 0 (`skipcache::token_class`): what
    /// `get_next` and the reader do with the token depends on the class
    /// alone ([`Tracker::CLASSES`]).
    fn read_class(&self, _p: i32) {}
    /// The class of control sequence `p`'s meaning changed.
    fn class_wrote(&self, _p: i32) {}
    /// A blank line of a file became `\\par` outside definitions,
    /// arguments and skipped text: a point a cold build could start from
    /// (DESIGN.md §7.6).
    fn blank_line(&self) {}
    /// Input file `data` was opened to be read by lines (`\\input`,
    /// `\\openin`).
    fn lines_open(&self, _data: &alloc::sync::Arc<[u8]>) {}
    /// The line of `data` that begins at byte `from` was read into the
    /// buffer: the tracker's catcode generation now. (A `\\read` line
    /// never ends: its tokens are not followed.)
    fn line_start(&self, _data: &[u8], _from: usize) -> u64 {
        0
    }
    /// That `\\input` line, begun at generation `generation`, is done: the
    /// next line of its file was read or the file ended. `codes` gives
    /// what it was tokenized under, if the generation is still current.
    fn line_end(
        &self,
        _data: &[u8],
        _from: usize,
        _generation: u64,
        _codes: impl FnOnce() -> Option<LineCodes>,
    ) {
    }
    /// An error context showed the line of `data` that begins at `from`.
    fn line_shown(&self, _data: &[u8], _from: usize) {}
    /// (With [`Tracker::VALUES`].) Line number `value` of the file at input
    /// level `level` was read (`\\inputlineno`, §424) or printed (an error's
    /// context, a box's report, a conditional's line): a read of the source
    /// position.
    fn line_number(&self, _level: usize, _value: i32) {}
    /// (With [`Tracker::VALUES`].) A lookup of the control sequence named
    /// `name` found it at hash slot `found` (0: not there), before any
    /// insertion.
    fn name_lookup(&self, _name: &[u8], _found: i32) {}
    /// (With [`Tracker::VALUES`].) §930: a look for the word `key`
    /// (`hyph::exception_key`: its `\\lccode`s and language) in the
    /// exception table found what `version` is the version of (its
    /// positions', or `hyph::NO_EXCEPTION`): a read of the word's entry,
    /// verified by looking again.
    fn hyph_word_read(&self, _key: &[u8], _version: u128) {}
    /// (With [`Tracker::VALUES`].) §940: the word `key` was entered in the
    /// exception table (after its table's write): a write of the word's
    /// entry.
    fn hyph_word_wrote(&self, _key: &[u8]) {}
    /// (With [`Tracker::VALUES`].) File `name` was looked up to be read
    /// as a file of `kind` (`\\input`, `\\openin`, a font map): its
    /// contents, if found.
    fn load(
        &self,
        _name: &[u8],
        _kind: crate::host::FileKind,
        _contents: Option<&alloc::sync::Arc<[u8]>>,
    ) {
    }
    /// (With [`Tracker::VALUES`].) [`Tracker::load`] of a file to be read
    /// by lines (`\\input`, `\\openin`): what the reader takes of its
    /// contents is the lines it reads and the ends it meets
    /// ([`Tracker::eof_read`]); the load itself, whether the file is there.
    fn load_lines(
        &self,
        name: &[u8],
        kind: crate::host::FileKind,
        contents: Option<&alloc::sync::Arc<[u8]>>,
    ) {
        self.load(name, kind, contents);
    }
    /// (With [`Tracker::LINES`].) A read of the next line of `data` found
    /// its end, at byte `pos`: what lines added there change.
    fn eof_read(&self, _data: &[u8], _pos: usize) {}
    /// (With [`Tracker::VALUES`].) What a load of `name` (`\\input`,
    /// `\\openin`) reads when the build holds the name's value: a name
    /// the job stores, inside a rebuild (DESIGN 7.17.3, "A load reads the
    /// store, not the file"), its contents or its absence; `None`: the
    /// host's file.
    fn stored(&self, _name: &[u8]) -> Option<Option<alloc::sync::Arc<[u8]>>> {
        None
    }
    /// (With [`Tracker::VALUES`].) File `name` was opened for writing
    /// by `\\write` stream `stream` (`\\openout`, §1374): a store made
    /// (DESIGN 7.17.5).
    fn store_open(&self, _stream: u8, _name: &[u8]) {}
    /// (With [`Tracker::VALUES`].) The files the job stored with
    /// `\openout` (not a command's) whose store reaches the running
    /// step's point, with their contents there (DESIGN 3.7, "Commands"):
    /// what a command run there is handed of the job's own files.
    fn stores_reaching(&self) -> alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::sync::Arc<[u8]>)> {
        alloc::vec::Vec::new()
    }
    /// (With [`Tracker::VALUES`].) A command the running step ran made
    /// or changed file `name`, now `contents`: a store of the step, as an
    /// `\openout`, a `\write` of each of its lines and a `\closeout`
    /// would make (DESIGN 3.7, "Commands").
    fn command_wrote(&self, _name: &[u8], _contents: &[u8]) {}
    /// (With [`Tracker::VALUES`].) Whether the running step's run has read
    /// a slot a later definition holds, not placed: it will be dropped and
    /// made again, so a command it would run is not run (DESIGN 3.7,
    /// "Commands").
    fn run_doomed(&self) -> bool {
        false
    }
    /// (With [`Tracker::VALUES`].) A command the running step ran removed
    /// file `name`: loads after it find none (DESIGN 3.7, "Commands").
    fn command_removed(&self, _name: &[u8]) {}
    /// (With [`Tracker::VALUES`].) A line (without its end) written to
    /// file `name` by the `\\write` stream that stores to it (the stream's
    /// name as the engine holds it, restored with the stream in a
    /// rebuild): a store to it (7.17.5).
    fn store_line(&self, _name: &[u8], _line: &[u8]) {}
    /// (With [`Tracker::VALUES`].) Bytes of output `what` made: an effect
    /// of the running call.
    fn output(&self, _what: Output, _bytes: &[u8]) {}
}

/// An output the engine makes, each an effect of the call that made it
/// (DESIGN 7.17.2) and put back through its own sink when a hit is
/// applied (`ssa::replay_effects`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Output {
    /// The log file's bytes (§56 `wlog`).
    Log,
    /// The terminal's bytes (§56 `wterm`).
    Term,
    /// `\\write` stream `n`'s file (§1370).
    Write(u8),
    /// The DVI file's bytes (§597 `write_dvi`).
    Dvi,
    /// The DVI writer handed to the host's page sink (§617), saved.
    DviStart,
    /// A page queued at the page sink (§640), saved.
    DviPage,
    /// A page written at once by the page sink, saved.
    DviPageNow,
    /// The page sink told to finish the file (§642), with `\\mag`.
    DviFinish,
    /// The PDF file's bytes (pdfTeX §681).
    Pdf,
}

impl core::fmt::Display for Output {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Output::Log => f.write_str("log"),
            Output::Term => f.write_str("term"),
            Output::Write(n) => write!(f, "write{n}"),
            Output::Dvi => f.write_str("dvi"),
            Output::DviStart => f.write_str("dvi start"),
            Output::DviPage => f.write_str("dvi page"),
            Output::DviPageNow => f.write_str("dvi page now"),
            Output::DviFinish => f.write_str("dvi finish"),
            Output::Pdf => f.write_str("pdf"),
        }
    }
}

/// The lines of `data` as `input_ln` reads them: where each begins, where
/// its text ends (trailing spaces removed) and where the next begins.
#[must_use]
pub fn file_lines(data: &[u8]) -> alloc::vec::Vec<(usize, usize, usize)> {
    let mut out = alloc::vec::Vec::new();
    let mut i = 0;
    while i < data.len() {
        let from = i;
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
        out.push((from, end, i));
    }
    out
}

/// Whether a write to eqtb location `p` can change how lines are read
/// into tokens (a category code, `\\endlinechar`).
#[inline]
#[must_use]
pub fn tokenizes(p: i32) -> bool {
    use partex_engine::web::{CAT_CODE_BASE, END_LINE_CHAR_CODE, INT_BASE};
    (CAT_CODE_BASE..CAT_CODE_BASE + 256).contains(&p) || p == INT_BASE + END_LINE_CHAR_CODE
}

/// What turns a line of a file into tokens (TeX §343–357): the category
/// codes and `\\endlinechar`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineCodes {
    pub cat: [u8; 256],
    pub end_line_char: i32,
}

/// One token of a line, as [`line_tokens`] gives them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LineToken {
    /// A character token (category, character).
    Char(u8, u8),
    /// An active character.
    Active(u8),
    /// A control sequence, by name (single-character ones too).
    Cs(alloc::vec::Vec<u8>),
    /// An empty line's `\\par`.
    Par,
}

/// The tokens `line` (as `input_ln` leaves it, without the end-of-line
/// character) gives under `codes`, read from its start (state
/// `new_line`), if TeX would read it without changing the buffer or
/// stopping: `None` for `^^` notation and invalid characters.
#[must_use]
pub fn line_tokens(line: &[u8], codes: &LineCodes) -> Option<alloc::vec::Vec<LineToken>> {
    line_tokens_from(line, codes, 0)
}

/// [`line_tokens`] from a scanner state (0 `new_line`, 1 `mid_line`, 2
/// `skip_blanks`, §303): the rest of a line read from where a call starts.
#[must_use]
pub fn line_tokens_from(
    line: &[u8],
    codes: &LineCodes,
    start: u8,
) -> Option<alloc::vec::Vec<LineToken>> {
    use partex_engine::web::{
        ACTIVE_CHAR, CAR_RET, COMMENT, ESCAPE, IGNORE, INVALID_CHAR, LETTER, SPACER, SUP_MARK,
    };
    #[derive(PartialEq)]
    enum State {
        NewLine,
        MidLine,
        SkipBlanks,
    }
    let mut buf = line.to_vec();
    if let Ok(c) = u8::try_from(codes.end_line_char) {
        buf.push(c);
    }
    let cat = |c: u8| i32::from(codes.cat[usize::from(c)]);
    let expanded = |k: usize| cat(buf[k]) == SUP_MARK && buf.get(k + 1) == Some(&buf[k]);
    let mut out = alloc::vec::Vec::new();
    let mut state = match start {
        1 => State::MidLine,
        2 => State::SkipBlanks,
        _ => State::NewLine,
    };
    let mut loc = 0;
    while loc < buf.len() {
        let c = buf[loc];
        if expanded(loc) {
            return None;
        }
        loc += 1;
        match cat(c) {
            ESCAPE => {
                if loc >= buf.len() {
                    out.push(LineToken::Cs(alloc::vec::Vec::new()));
                    continue;
                }
                let first = cat(buf[loc]);
                state = if first == LETTER || first == SPACER {
                    State::SkipBlanks
                } else {
                    State::MidLine
                };
                let mut k = loc + 1;
                if first == LETTER {
                    while k < buf.len() && cat(buf[k]) == LETTER {
                        k += 1;
                    }
                    if k < buf.len() && expanded(k) {
                        return None;
                    }
                } else if expanded(loc) {
                    return None;
                }
                out.push(LineToken::Cs(buf[loc..k].to_vec()));
                loc = k;
            }
            ACTIVE_CHAR => {
                state = State::MidLine;
                out.push(LineToken::Active(c));
            }
            SPACER => {
                if state == State::MidLine {
                    state = State::SkipBlanks;
                    out.push(LineToken::Char(10, b' '));
                }
            }
            CAR_RET => {
                loc = buf.len();
                match state {
                    State::NewLine => out.push(LineToken::Par),
                    State::MidLine => out.push(LineToken::Char(10, b' ')),
                    State::SkipBlanks => {}
                }
            }
            COMMENT => loc = buf.len(),
            IGNORE => {}
            INVALID_CHAR => return None,
            k => {
                state = State::MidLine;
                out.push(LineToken::Char(u8::try_from(k).ok()?, c));
            }
        }
    }
    Some(out)
}

/// No tracking: reference mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct Untracked;

impl Tracker for Untracked {
    const READS: bool = false;
    #[inline(always)]
    fn read(&self, _: Cell) {}
    #[inline(always)]
    fn write(&self, _: Cell) {}
}

impl<H: crate::host::Host, T: Tracker> crate::tex::Tex<H, T> {
    /// Print line number `value` of input level `level` (`usize::MAX`: a
    /// level not known, such as a paragraph's first line), a read of the
    /// source position (DESIGN.md §7.17.4).
    pub(crate) fn print_line_no(&mut self, level: usize, value: i32) {
        if T::VALUES {
            self.tracker.line_number(level, value);
        }
        self.print_int(value);
    }

    /// The tracker, to read what it recorded.
    pub fn tracker(&self) -> &T {
        &self.tracker
    }

    /// Readable names of `cells`: control sequences by name, registers and
    /// parameters by a control sequence that points at them (`\\c@page`,
    /// `\\tolerance`), anything else by its location. For reports; reads
    /// state through the tracker.
    pub fn cell_names(&self, cells: &[Cell]) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
        use crate::web::{
            ACTIVE_BASE, ASSIGN_DIMEN, ASSIGN_GLUE, ASSIGN_INT, ASSIGN_MU_GLUE, ASSIGN_TOKS,
            EQTB_SIZE, GLUE_BASE, HASH_BASE, NULL_CS, SINGLE_BASE,
        };
        use alloc::collections::BTreeMap;
        use alloc::vec::Vec;
        let is_cs = |p: i32| {
            (HASH_BASE..GLUE_BASE).contains(&p) || (p > EQTB_SIZE && p < crate::xregs::EXT_BASE)
        };
        let name = |p: i32| -> Vec<u8> {
            let mut v = b"\\".to_vec();
            if p < SINGLE_BASE {
                v = b"~".to_vec();
                v.push(u8::try_from(p - ACTIVE_BASE).unwrap_or(b'?'));
            } else if p < NULL_CS {
                v.push(u8::try_from(p - SINGLE_BASE).unwrap_or(b'?'));
            } else if p == NULL_CS {
                v.extend_from_slice(b"csname\\endcsname");
            } else {
                let t = self.text(p);
                v.extend_from_slice(self.str_bytes(usize::try_from(t).unwrap_or(0)));
            }
            v
        };
        // Locations named by `\countdef`-like and primitive meanings.
        let mut by_loc: BTreeMap<i32, i32> = BTreeMap::new();
        for p in (HASH_BASE..GLUE_BASE).chain(EQTB_SIZE + 1..=self.eqtb_top) {
            let w = self.eqtb(p);
            if matches!(
                w.b0(),
                ASSIGN_INT | ASSIGN_DIMEN | ASSIGN_GLUE | ASSIGN_MU_GLUE | ASSIGN_TOKS
            ) && self.text(p) != 0
            {
                by_loc.entry(w.rh()).or_insert(p);
            }
        }
        cells
            .iter()
            .map(|&c| match c {
                Cell::Eqtb(p) if p < GLUE_BASE && p >= ACTIVE_BASE || is_cs(p) => name(p),
                Cell::Eqtb(p) => by_loc
                    .get(&p)
                    .map_or_else(|| eqtb_range_name(p), |&q| name(q)),
                Cell::Hash(p) => {
                    let mut v = b"hash:".to_vec();
                    v.extend_from_slice(&name(p));
                    v
                }
                Cell::Font(f) => {
                    let mut v = b"font:".to_vec();
                    v.extend_from_slice(&name(crate::web::FONT_ID_BASE + f));
                    v
                }
                Cell::FontTable => b"the font table".to_vec(),
                Cell::Read(n) => alloc::format!("\\read{n}").into_bytes(),
                Cell::Out(n) => alloc::format!("\\write{n}").into_bytes(),
                Cell::Random => b"the random number generator".to_vec(),
                Cell::HashNext(p) => {
                    let mut v = b"hash link:".to_vec();
                    v.extend_from_slice(&name(p));
                    v
                }
                Cell::Str(h) => alloc::format!("the strings hashing to {h:#x}").into_bytes(),
            })
            .collect()
    }
}

/// A location in `eqtb` outside the control sequences, by its region and
/// offset (`catcode 64`, `count 0`).
fn eqtb_range_name(p: i32) -> alloc::vec::Vec<u8> {
    use crate::web::{
        BOX_BASE, CAT_CODE_BASE, CHAR_SUB_CODE_BASE, COUNT_BASE, CUR_FONT_LOC, DEL_CODE_BASE,
        DIMEN_BASE, EQTB_SIZE, GLUE_BASE, INT_BASE, LC_CODE_BASE, LOCAL_BASE, MATH_CODE_BASE,
        MU_SKIP_BASE, SCALED_BASE, SF_CODE_BASE, SKIP_BASE, TOKS_BASE, UC_CODE_BASE,
    };
    // Each region ends where the next begins.
    let regions = [
        (GLUE_BASE, "gluepar"),
        (SKIP_BASE, "skip"),
        (MU_SKIP_BASE, "muskip"),
        (LOCAL_BASE, "local"),
        (TOKS_BASE, "toks"),
        (BOX_BASE, "box"),
        (CUR_FONT_LOC, "font/math"),
        (CAT_CODE_BASE, "catcode"),
        (LC_CODE_BASE, "lccode"),
        (UC_CODE_BASE, "uccode"),
        (SF_CODE_BASE, "sfcode"),
        (MATH_CODE_BASE, "mathcode"),
        (CHAR_SUB_CODE_BASE, "charsub"),
        (INT_BASE, "intpar"),
        (COUNT_BASE, "count"),
        (DEL_CODE_BASE, "delcode"),
        (DIMEN_BASE, "dimenpar"),
        (SCALED_BASE, "dimen"),
    ];
    match regions.iter().rev().find(|&&(lo, _)| lo <= p) {
        Some(&(lo, what)) if p <= EQTB_SIZE => alloc::format!("{what} {}", p - lo).into_bytes(),
        _ => alloc::format!("eqtb[{p}]").into_bytes(),
    }
}
