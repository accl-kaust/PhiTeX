//! BibTeX 0.99e, run in process: reads a job's `.aux` file, its `.bst`
//! style and `.bib` databases, and produces the `.bbl` and `.blg` files and
//! the terminal transcript `bibtex` would.
//!
//! A port of bibtex.web's behaviour with TeX Live's changes (bibtex.ch):
//! eight-bit input, dynamic capacities, `-terse` and `-min-crossrefs`, the
//! `Capacity:` line and usage statistics in the `.blg`. `§N` is a
//! bibtex.web section. Data structures are native: strings are shared
//! values rather than a string pool, and capacities grow without the
//! `Reallocated …` lines web2c logs (masked in comparisons, like TeX's
//! memory statistics).
//!
//! `no_std` + `alloc`: files come through [`Files`].

#![no_std]
// Keep bibtex.web's names (`sp_ptr`, `sp_xptr1`, …) and its control
// structure, so the port can be read against the WEB source.
#![allow(
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::struct_field_names,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

extern crate alloc;

mod aux;
mod bib;
mod bst;
mod builtins;
mod exec;
mod session;
mod table;

pub use session::{Session, Stats};

use alloc::vec::Vec;

use table::{Loc, Str, Table};

/// Where the program's input files come from.
pub trait Files {
    /// An `.aux` file, by its name as written (no path search).
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>>;
    /// A style file, looked up as kpathsea's `bst` format (the name has no
    /// extension; the lookup adds `.bst`).
    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>>;
    /// A database, looked up as kpathsea's `bib` format (the lookup adds
    /// `.bib` unless the name has it).
    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>>;
}

/// The run's settings: `bibtex`'s options and its `texmf.cnf` values.
#[derive(Clone, Debug)]
pub struct Options {
    /// `-terse`: progress lines go to the log only.
    pub terse: bool,
    /// `-min-crossrefs`.
    pub min_crossrefs: i64,
    /// `max_strings` (and so `hash_size`), for the `Capacity:` line.
    pub max_strings: i64,
    pub ent_str_size: usize,
    pub glob_str_size: usize,
    pub max_print_line: usize,
    /// What follows the banner, e.g. ` (TeX Live 2026)`.
    pub version: Vec<u8>,
}

impl Default for Options {
    /// TeX Live's `texmf.cnf` values for `bibtex`.
    fn default() -> Self {
        Self {
            terse: false,
            min_crossrefs: 2,
            max_strings: 200_000,
            ent_str_size: 500,
            glob_str_size: 200_000,
            max_print_line: 79,
            version: b" (TeX Live 2026)".to_vec(),
        }
    }
}

/// An output file: its name and contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutFile {
    pub name: Vec<u8>,
    pub contents: Vec<u8>,
}

/// What a run produced.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Everything written to the terminal (standard output).
    pub term: Vec<u8>,
    /// The `.blg` file (absent if the `.aux` file could not be read).
    pub blg: Option<OutFile>,
    /// The `.bbl` file (absent if the `.aux` file could not be read).
    pub bbl: Option<OutFile>,
    /// 0 spotless, 1 warnings, 2 errors, 3 fatal.
    pub history: u8,
    /// The process exit status.
    pub status: i32,
}

// §31: lexical classes
const ILLEGAL: u8 = 0;
const WHITE_SPACE: u8 = 1;
const ALPHA: u8 = 2;
const NUMERIC: u8 = 3;
const SEP_CHAR: u8 = 4;
const OTHER_LEX: u8 = 5;

/// §32 (and bibtex.ch: carriage return is white space, bytes ≥ 128 are
/// letters).
const LEX_CLASS: [u8; 256] = {
    let mut t = [OTHER_LEX; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = ILLEGAL;
        i += 1;
    }
    t[0x7f] = ILLEGAL;
    t[b'\t' as usize] = WHITE_SPACE;
    t[13] = WHITE_SPACE;
    t[b' ' as usize] = WHITE_SPACE;
    t[b'~' as usize] = SEP_CHAR;
    t[b'-' as usize] = SEP_CHAR;
    let mut i = b'0' as usize;
    while i <= b'9' as usize {
        t[i] = NUMERIC;
        i += 1;
    }
    let mut i = b'A' as usize;
    while i <= b'Z' as usize {
        t[i] = ALPHA;
        t[i + 32] = ALPHA;
        i += 1;
    }
    let mut i = 0x80;
    while i < 256 {
        t[i] = ALPHA;
        i += 1;
    }
    t
};

/// §33: which characters may appear in identifiers.
const LEGAL_ID: [bool; 256] = {
    let mut t = [true; 256];
    let mut i = 0;
    while i < 0x20 {
        t[i] = false;
        i += 1;
    }
    let bad = b" \t\"#%'(),={}";
    let mut i = 0;
    while i < bad.len() {
        t[bad[i] as usize] = false;
        i += 1;
    }
    t
};

fn lex(c: u8) -> u8 {
    LEX_CLASS[c as usize]
}

/// §89: how an identifier scan ended.
const ID_NULL: u8 = 0;
const SPECIFIED_CHAR_ADJACENT: u8 = 1;
const OTHER_CHAR_ADJACENT: u8 = 2;
const WHITE_ADJACENT: u8 = 3;

// §18: history values
const SPOTLESS: u8 = 0;
const WARNING_MESSAGE: u8 = 1;
const ERROR_MESSAGE: u8 = 2;
const FATAL_MESSAGE: u8 = 3;

/// Why processing stopped early: bibtex.web's jumps out of procedures.
enum Stop {
    /// `goto close_up_shop`, after a fatal error.
    Fatal,
    /// `goto bst_done`: the style file ended while skipping to a blank
    /// line after an error.
    BstDone,
}

type R<T = ()> = Result<T, Stop>;

/// Something printable, for [`Bib::print`].
trait Piece {
    fn put(&self, out: &mut Vec<u8>);
}

impl Piece for &str {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self.as_bytes());
    }
}

impl Piece for &[u8] {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
}

impl Piece for Str {
    fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(self);
    }
}

/// A character (`xchr[c]`).
struct Ch(u8);

impl Piece for Ch {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(self.0);
    }
}

impl Piece for i64 {
    fn put(&self, out: &mut Vec<u8>) {
        let mut buf = [0u8; 24];
        let mut i = buf.len();
        let mut n = self.unsigned_abs();
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        if *self < 0 {
            i -= 1;
            buf[i] = b'-';
        }
        out.extend_from_slice(&buf[i..]);
    }
}

impl Piece for usize {
    fn put(&self, out: &mut Vec<u8>) {
        (*self as i64).put(out);
    }
}

/// An input file being read line by line.
#[derive(Clone, Default)]
struct Reader {
    data: Vec<u8>,
    pos: usize,
}

impl Reader {
    fn new(data: Vec<u8>) -> Self {
        Self { data, pos: 0 }
    }

    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// Which file `input_ln` reads.
#[derive(Clone, Copy)]
enum Src {
    Aux,
    Bst,
    Bib,
}

/// An open `.aux` file (§104).
struct AuxFrame {
    file: Reader,
    name: Str,
    line: i64,
}

/// A literal on the stack (§291). Strings remember whether they were made
/// by the command being executed (bibtex.web's strings at or above
/// `cmd_str_ptr`), which decides whether a global string variable copies
/// them (§359).
#[derive(Clone)]
enum Lit {
    Int(i64),
    Str(Str, bool),
    Fn(Loc),
    Missing(Str),
    Empty,
}

// §291: literal types, for messages
const STK_INT: u8 = 0;
const STK_STR: u8 = 1;
const STK_FN: u8 = 2;

// §156: function classes
const BUILT_IN: u8 = 0;
const WIZ_DEFINED: u8 = 1;
const INT_LITERAL: u8 = 2;
const STR_LITERAL: u8 = 3;
const FIELD: u8 = 4;
const INT_ENTRY_VAR: u8 = 5;
const STR_ENTRY_VAR: u8 = 6;
const INT_GLOBAL_VAR: u8 = 7;
const STR_GLOBAL_VAR: u8 = 8;

/// §160: markers in wizard-defined function bodies, and in `type_list`
/// (§219: `empty` and `undefined`).
const QUOTE_NEXT_FN: Loc = 0;
const END_OF_DEF: Loc = Loc::MAX;
const EMPTY: Loc = 0;
const UNDEFINED: Loc = Loc::MAX;

/// §216: the character that ends an entry string.
const END_OF_STRING: u8 = 0x7f;

/// The program's state (bibtex.web's globals).
struct Bib<'f> {
    files: &'f mut dyn Files,
    opts: Options,
    term: Vec<u8>,
    log: Vec<u8>,
    bbl: Vec<u8>,
    history: u8,
    err_count: i64,
    /// The warnings and errors marked, counted (a session's history is
    /// made from its calls' counts).
    n_warn: i64,
    n_err: i64,
    /// A session's run: the execution commands are calls (`session.rs`).
    inc: Option<alloc::boxed::Box<session::Inc>>,
    t: Table,

    // §41–43, §80: the input line and scanning
    buffer: Vec<u8>,
    last: usize,
    buf_ptr1: usize,
    buf_ptr2: usize,
    scan_result: u8,
    token_value: i64,

    // §104: the .aux files
    aux: Vec<AuxFrame>,
    top_lev_str: Str,

    // §117, §124
    bib_list: Vec<Str>,
    bib_files: Vec<Option<Reader>>,
    bib_ptr: usize,
    num_bib_files: usize,
    bib_seen: bool,
    bst_seen: bool,
    bst_str: Option<Str>,
    bst_file: Reader,

    // §129: cite keys
    cite_list: Vec<Str>,
    cite_ptr: usize,
    entry_cite_ptr: usize,
    num_cites: usize,
    old_num_cites: usize,
    citation_seen: bool,
    all_entries: bool,
    all_marker: usize,

    // §147, §161: the style
    bbl_line_num: i64,
    bst_line_num: i64,
    wiz_loc: Loc,
    wiz_functions: Vec<Loc>,
    impl_fn_num: i64,
    num_ent_ints: usize,
    num_ent_strs: usize,
    num_fields: usize,
    num_pre_defined_fields: usize,
    crossref_num: usize,
    num_glb_strs: usize,
    /// `glb_str_ptr`/`global_strs`: a string of the database or style
    /// (kept as it is), or a copy made from a computed string.
    glb_strs: Vec<(Option<Str>, Vec<u8>)>,
    entry_seen: bool,
    read_seen: bool,
    read_performed: bool,
    reading_completed: bool,
    read_completed: bool,

    // §219: the database
    bib_line_num: i64,
    type_list: Vec<Loc>,
    entry_exists: Vec<bool>,
    /// `cite_info` holding cite keys (for `\citation{*}`, §227) …
    cite_info_str: Vec<Str>,
    /// … and holding cross-reference counts (§264).
    cite_info: Vec<i64>,
    field_info: Vec<Option<Str>>,
    s_preamble: Vec<Str>,
    num_preamble_strings: usize,
    type_exists: bool,
    store_entry: bool,
    store_field: bool,
    at_bib_command: bool,
    command_num: i64,
    right_outer_delim: u8,
    right_str_delim: u8,
    cur_macro_loc: Loc,
    entry_type_loc: Loc,
    field_name_loc: Loc,
    /// The field value being scanned (`field_vl_str`, an alias of `ex_buf`).
    field_vl: Vec<u8>,

    // §290: execution
    lit_stack: Vec<Lit>,
    mess_with_entries: bool,
    sorted_cites: Vec<usize>,
    sort_key_num: usize,
    entry_ints: Vec<i64>,
    entry_strs: Vec<Vec<u8>>,
    ex_buf: Vec<u8>,
    ex_buf_length: usize,
    out_buf: Vec<u8>,
    out_buf_length: usize,
    brace_level: i64,
    prev_colon: bool,
    name_buf: Vec<u8>,
    name_tok: Vec<usize>,
    name_sep_char: Vec<u8>,
    s_null: Str,
    b_default: Loc,
    b_skip: Loc,
    execution_count: [i64; builtins::NUM_BLT_IN_FNS],
    blt_in_loc: [Loc; builtins::NUM_BLT_IN_FNS],
}

/// Run BibTeX on `aux` (as given on its command line) with `opts`.
pub fn run(aux: &[u8], opts: &Options, files: &mut dyn Files) -> Outcome {
    let mut b = Bib::new(opts.clone(), files);
    b.pre_def_certain_strings();
    let Some((blg_name, bbl_name)) = b.get_the_top_level_aux_file_name(aux) else {
        return Outcome {
            term: b.term,
            blg: None,
            bbl: None,
            history: SPOTLESS,
            status: 1,
        };
    };
    // §10 (bibtex.ch)
    let mut banner = b"This is BibTeX, Version 0.99e".to_vec();
    banner.extend_from_slice(&b.opts.version);
    if b.opts.terse {
        b.log.extend_from_slice(&banner);
        b.log.push(b'\n');
    } else {
        b.print_ln(&[&&banner[..]]);
    }
    let max_strings = b.opts.max_strings;
    let hash_size = max_strings.max(5000);
    let line = [
        &"Capacity: max_strings=" as &dyn Piece,
        &max_strings,
        &", hash_size=",
        &hash_size,
        &", hash_prime=",
        &hash_prime(hash_size),
    ];
    b.log_ln(&line);
    // §10: read the .aux file, then read and execute the .bst file
    let _ = b.read_aux().and_then(|()| b.read_bst());
    b.clean_up();
    let status = if b.history > WARNING_MESSAGE {
        i32::from(b.history)
    } else {
        0
    };
    Outcome {
        term: b.term,
        blg: Some(OutFile {
            name: blg_name,
            contents: b.log,
        }),
        bbl: Some(OutFile {
            name: bbl_name,
            contents: b.bbl,
        }),
        history: b.history,
        status,
    }
}

/// bibtex.ch's `compute_hash_prime`: the least prime at least 85% of
/// `hash_size`.
fn hash_prime(hash_size: i64) -> i64 {
    let want = (hash_size / 20) * 17;
    let mut p = 2.max(want);
    loop {
        if (2..=p.isqrt()).all(|d| p % d != 0) {
            return p;
        }
        p += 1;
    }
}

impl<'f> Bib<'f> {
    fn new(opts: Options, files: &'f mut dyn Files) -> Self {
        let empty: Str = Str::from(&b""[..]);
        Self {
            files,
            opts,
            term: Vec::new(),
            log: Vec::new(),
            bbl: Vec::new(),
            history: SPOTLESS,
            err_count: 0,
            n_warn: 0,
            n_err: 0,
            inc: None,
            t: Table::new(),
            buffer: Vec::new(),
            last: 0,
            buf_ptr1: 0,
            buf_ptr2: 0,
            scan_result: ID_NULL,
            token_value: 0,
            aux: Vec::new(),
            top_lev_str: empty.clone(),
            bib_list: Vec::new(),
            bib_files: Vec::new(),
            bib_ptr: 0,
            num_bib_files: 0,
            bib_seen: false,
            bst_seen: false,
            bst_str: None,
            bst_file: Reader::default(),
            cite_list: Vec::new(),
            cite_ptr: 0,
            entry_cite_ptr: 0,
            num_cites: 0,
            old_num_cites: 0,
            citation_seen: false,
            all_entries: false,
            all_marker: 0,
            bbl_line_num: 0,
            bst_line_num: 0,
            wiz_loc: 0,
            wiz_functions: Vec::new(),
            impl_fn_num: 0,
            num_ent_ints: 0,
            num_ent_strs: 0,
            num_fields: 0,
            num_pre_defined_fields: 0,
            crossref_num: 0,
            num_glb_strs: 0,
            glb_strs: Vec::new(),
            entry_seen: false,
            read_seen: false,
            read_performed: false,
            reading_completed: false,
            read_completed: false,
            bib_line_num: 0,
            type_list: Vec::new(),
            entry_exists: Vec::new(),
            cite_info_str: Vec::new(),
            cite_info: Vec::new(),
            field_info: Vec::new(),
            s_preamble: Vec::new(),
            num_preamble_strings: 0,
            type_exists: false,
            store_entry: false,
            store_field: false,
            at_bib_command: false,
            command_num: 0,
            right_outer_delim: 0,
            right_str_delim: 0,
            cur_macro_loc: 0,
            entry_type_loc: 0,
            field_name_loc: 0,
            field_vl: Vec::new(),
            lit_stack: Vec::new(),
            mess_with_entries: false,
            sorted_cites: Vec::new(),
            sort_key_num: 0,
            entry_ints: Vec::new(),
            entry_strs: Vec::new(),
            ex_buf: Vec::new(),
            ex_buf_length: 0,
            out_buf: Vec::new(),
            out_buf_length: 0,
            brace_level: 0,
            prev_colon: false,
            name_buf: Vec::new(),
            name_tok: Vec::new(),
            name_sep_char: Vec::new(),
            s_null: empty,
            b_default: 0,
            b_skip: 0,
            execution_count: [0; builtins::NUM_BLT_IN_FNS],
            blt_in_loc: [0; builtins::NUM_BLT_IN_FNS],
        }
    }

    // §3: printing, to the log and the terminal

    fn print(&mut self, pieces: &[&dyn Piece]) {
        let mut v = Vec::new();
        for p in pieces {
            p.put(&mut v);
        }
        self.log.extend_from_slice(&v);
        self.term.extend_from_slice(&v);
    }

    fn print_ln(&mut self, pieces: &[&dyn Piece]) {
        self.print(pieces);
        self.print_newline();
    }

    fn print_newline(&mut self) {
        self.log.push(b'\n');
        self.term.push(b'\n');
    }

    /// bibtex.ch's `log_pr`: the log only.
    fn log_pr(&mut self, pieces: &[&dyn Piece]) {
        for p in pieces {
            p.put(&mut self.log);
        }
    }

    fn log_ln(&mut self, pieces: &[&dyn Piece]) {
        self.log_pr(pieces);
        self.log.push(b'\n');
    }

    /// A progress line: to the terminal too unless `-terse`.
    fn progress(&mut self, pieces: &[&dyn Piece]) {
        if self.opts.terse {
            self.log_ln(pieces);
        } else {
            self.print_ln(pieces);
        }
    }

    // §18
    fn mark_warning(&mut self) {
        self.n_warn += 1;
        if self.history == WARNING_MESSAGE {
            self.err_count += 1;
        } else if self.history == SPOTLESS {
            self.history = WARNING_MESSAGE;
            self.err_count = 1;
        }
    }

    fn mark_error(&mut self) {
        self.n_err += 1;
        if self.history < ERROR_MESSAGE {
            self.history = ERROR_MESSAGE;
            self.err_count = 1;
        } else {
            self.err_count += 1;
        }
    }

    /// §45: `confusion`.
    fn confusion(&mut self, s: &str) -> Stop {
        self.print(&[&s]);
        self.print_ln(&[&"---this can't happen"]);
        self.print_ln(&[&"*Please notify the BibTeX maintainer*"]);
        self.history = FATAL_MESSAGE;
        Stop::Fatal
    }

    /// §44: `overflow`.
    fn overflow(&mut self, what: &str, n: i64) -> Stop {
        self.print(&[&"Sorry---you've exceeded BibTeX's "]);
        self.history = FATAL_MESSAGE;
        self.print_ln(&[&what, &n]);
        Stop::Fatal
    }

    // §47: reading a line into `buffer`

    fn reader(&mut self, src: Src) -> &mut Reader {
        match src {
            Src::Aux => &mut self.aux.last_mut().expect("an open .aux file").file,
            Src::Bst => &mut self.bst_file,
            Src::Bib => self.bib_files[self.bib_ptr].get_or_insert_with(Reader::default),
        }
    }

    /// `input_ln`: the next line (web2c ends a line at a line feed or a
    /// carriage return and consumes one character), without trailing white
    /// space; false at the end of the file.
    fn input_ln(&mut self, src: Src) -> bool {
        self.last = 0;
        let mut buffer = core::mem::take(&mut self.buffer);
        let f = self.reader(src);
        if f.eof() {
            self.buffer = buffer;
            return false;
        }
        let mut last = 0;
        while let Some(&c) = f.data.get(f.pos) {
            if c == b'\n' || c == b'\r' {
                break;
            }
            set(&mut buffer, last, c);
            last += 1;
            f.pos += 1;
        }
        f.pos += 1;
        // (scanning looks one character past the line, at what an earlier
        // line left there)
        if buffer.len() < last + 2 {
            buffer.resize(last + 2, 0);
        }
        while last > 0 && lex(buffer[last - 1]) == WHITE_SPACE {
            last -= 1;
        }
        self.buffer = buffer;
        self.last = last;
        true
    }

    fn eof(&mut self, src: Src) -> bool {
        self.reader(src).eof()
    }

    // §80–94: scanning the input line

    fn scan_char(&self) -> u8 {
        self.buffer.get(self.buf_ptr2).copied().unwrap_or(0)
    }

    fn token(&self) -> &[u8] {
        &self.buffer[self.buf_ptr1..self.buf_ptr2]
    }

    /// `str_lookup(buffer, buf_ptr1, token_len, ilk, do_insert)`.
    fn insert_token(&mut self, ilk: u8) -> (Loc, bool) {
        self.t
            .insert(ilk, &self.buffer[self.buf_ptr1..self.buf_ptr2])
    }

    /// `str_lookup(buffer, buf_ptr1, token_len, ilk, dont_insert)`.
    fn find_token(&self, ilk: u8) -> Option<Loc> {
        self.t.find(ilk, &self.buffer[self.buf_ptr1..self.buf_ptr2])
    }

    fn token_len(&self) -> usize {
        self.buf_ptr2 - self.buf_ptr1
    }

    /// §83–88: scan until the end of the line or a character satisfying
    /// `stop`; whether one was found before the end.
    fn scan_until(&mut self, stop: impl Fn(u8) -> bool) -> bool {
        self.buf_ptr1 = self.buf_ptr2;
        while self.buf_ptr2 < self.last && !stop(self.scan_char()) {
            self.buf_ptr2 += 1;
        }
        self.buf_ptr2 < self.last
    }

    fn scan1(&mut self, c1: u8) -> bool {
        self.scan_until(|c| c == c1)
    }

    fn scan1_white(&mut self, c1: u8) -> bool {
        self.scan_until(|c| c == c1 || lex(c) == WHITE_SPACE)
    }

    fn scan2(&mut self, c1: u8, c2: u8) -> bool {
        self.scan_until(|c| c == c1 || c == c2)
    }

    fn scan2_white(&mut self, c1: u8, c2: u8) -> bool {
        self.scan_until(|c| c == c1 || c == c2 || lex(c) == WHITE_SPACE)
    }

    fn scan3(&mut self, c1: u8, c2: u8, c3: u8) -> bool {
        self.scan_until(|c| c == c1 || c == c2 || c == c3)
    }

    /// §88
    fn scan_alpha(&mut self) -> bool {
        self.buf_ptr1 = self.buf_ptr2;
        while self.buf_ptr2 < self.last && lex(self.scan_char()) == ALPHA {
            self.buf_ptr2 += 1;
        }
        self.token_len() != 0
    }

    /// §90
    fn scan_identifier(&mut self, c1: u8, c2: u8, c3: u8) {
        self.buf_ptr1 = self.buf_ptr2;
        if lex(self.scan_char()) != NUMERIC {
            while self.buf_ptr2 < self.last && LEGAL_ID[self.scan_char() as usize] {
                self.buf_ptr2 += 1;
            }
        }
        let c = self.scan_char();
        self.scan_result = if self.token_len() == 0 {
            ID_NULL
        } else if lex(c) == WHITE_SPACE || self.buf_ptr2 == self.last {
            WHITE_ADJACENT
        } else if c == c1 || c == c2 || c == c3 {
            SPECIFIED_CHAR_ADJACENT
        } else {
            OTHER_CHAR_ADJACENT
        };
    }

    /// §92
    fn scan_nonneg_integer(&mut self) -> bool {
        self.buf_ptr1 = self.buf_ptr2;
        self.token_value = 0;
        while self.buf_ptr2 < self.last && lex(self.scan_char()) == NUMERIC {
            self.token_value = self
                .token_value
                .wrapping_mul(10)
                .wrapping_add(i64::from(self.scan_char() - b'0'));
            self.buf_ptr2 += 1;
        }
        self.token_len() != 0
    }

    /// §93
    fn scan_integer(&mut self) -> bool {
        self.buf_ptr1 = self.buf_ptr2;
        let sign_length = usize::from(self.scan_char() == b'-');
        if sign_length == 1 {
            self.buf_ptr2 += 1;
        }
        self.token_value = 0;
        while self.buf_ptr2 < self.last && lex(self.scan_char()) == NUMERIC {
            self.token_value = self
                .token_value
                .wrapping_mul(10)
                .wrapping_add(i64::from(self.scan_char() - b'0'));
            self.buf_ptr2 += 1;
        }
        if sign_length == 1 {
            self.token_value = -self.token_value;
        }
        self.token_len() != sign_length
    }

    /// §94
    fn scan_white_space(&mut self) -> bool {
        while self.buf_ptr2 < self.last && lex(self.scan_char()) == WHITE_SPACE {
            self.buf_ptr2 += 1;
        }
        self.buf_ptr2 < self.last
    }

    /// §82: `print_token`.
    fn print_token(&mut self) {
        let t: &[u8] = &self.buffer[self.buf_ptr1..self.buf_ptr2];
        let t = t.to_vec();
        self.print(&[&&t[..]]);
    }

    /// Lower-case the current token in place (`lower_case(buffer, …)`).
    fn lower_token(&mut self) {
        self.buffer[self.buf_ptr1..self.buf_ptr2].make_ascii_lowercase();
    }

    /// §95
    fn print_bad_input_line(&mut self) {
        let show = |c: u8| if lex(c) == WHITE_SPACE { b' ' } else { c };
        let before: Vec<u8> = (0..self.buf_ptr2)
            .map(|i| show(get(&self.buffer, i)))
            .collect();
        self.print(&[&" : ", &&before[..]]);
        self.print_newline();
        let pad = alloc::vec![b' '; self.buf_ptr2];
        let after: Vec<u8> = (self.buf_ptr2..self.last)
            .map(|i| show(get(&self.buffer, i)))
            .collect();
        self.print(&[&" : ", &&pad[..], &&after[..]]);
        self.print_newline();
        if (0..self.buf_ptr2).all(|i| lex(get(&self.buffer, i)) == WHITE_SPACE) {
            self.print_ln(&[&"(Error may have been on previous line)"]);
        }
        self.mark_error();
    }

    /// §96
    fn print_skipping_whatever_remains(&mut self) {
        self.print(&[&"I'm skipping whatever remains of this "]);
    }

    /// §128
    fn print_bst_name(&mut self) {
        let s = self.bst_str.clone().unwrap_or_else(|| self.s_null.clone());
        self.print_ln(&[&s, &".bst"]);
    }

    /// §121 (bibtex.ch: no second `.bib`).
    fn bib_name(&self) -> Vec<u8> {
        let mut s = self.bib_list[self.bib_ptr].to_vec();
        if !s.ends_with(b".bib") {
            s.extend_from_slice(b".bib");
        }
        s
    }

    fn print_bib_name(&mut self) {
        let s = self.bib_name();
        self.print_ln(&[&&s[..]]);
    }

    /// §455: clean up and leave.
    fn clean_up(&mut self) {
        if self.read_performed && !self.reading_completed {
            let line = self.bib_line_num;
            self.print(&[&"Aborted at line ", &line, &" of file "]);
            self.print_bib_name();
        }
        // §465: usage statistics
        let n = self.num_cites;
        self.log_pr(&[&"You've used ", &n]);
        self.log_ln(&[if n == 1 { &" entry," } else { &" entries," }]);
        let w = self.wiz_functions.len();
        self.log_ln(&[&"            ", &w, &" wiz_defined-function locations,"]);
        let (strings, chars) = self.t.pool_stats();
        self.log_ln(&[
            &"            ",
            &strings,
            &" strings with ",
            &chars,
            &" characters,",
        ]);
        let total: i64 = self.execution_count.iter().sum();
        self.log_ln(&[
            &"and the built_in function-call counts, ",
            &total,
            &" in all, are:",
        ]);
        for i in 0..builtins::NUM_BLT_IN_FNS {
            let name = self.t.text(self.blt_in_loc[i]).clone();
            let c = self.execution_count[i];
            self.log_ln(&[&name, &" -- ", &c]);
        }
        // §466
        let e = self.err_count;
        match self.history {
            WARNING_MESSAGE if e == 1 => self.print_ln(&[&"(There was 1 warning)"]),
            WARNING_MESSAGE => self.print_ln(&[&"(There were ", &e, &" warnings)"]),
            ERROR_MESSAGE if e == 1 => self.print_ln(&[&"(There was 1 error message)"]),
            ERROR_MESSAGE => self.print_ln(&[&"(There were ", &e, &" error messages)"]),
            FATAL_MESSAGE => self.print_ln(&[&"(That was a fatal error)"]),
            _ => {}
        }
    }
}

/// Store `c` at `v[i]`, growing `v` (bibtex.ch reallocates its buffers).
fn set<T: Default + Clone>(v: &mut Vec<T>, i: usize, c: T) {
    if i >= v.len() {
        v.resize(i + 1, T::default());
    }
    v[i] = c;
}

/// `v[i]`, or the default past the end.
fn get<T: Default + Copy>(v: &[T], i: usize) -> T {
    v.get(i).copied().unwrap_or_default()
}
