//! `MakeIndex` 2.18, run in process: reads a job's `.idx` files and an
//! optional `.ist` style, and produces the `.ind` and `.ilg` files and the
//! terminal transcript `makeindex` would.
//!
//! A port of TeX Live's makeindexk behaviour (mkind.c, scanid.c, scanst.c,
//! sortid.c, qsort.c, genind.c): the same scanning, the same comparison
//! function and sort algorithm (its comparison count is in the `.ilg`, and
//! the comparisons mark duplicates, so the order of comparisons is
//! behaviour), the same output. C strings become byte vectors; the static
//! buffers whose stale contents C can read keep their persistence.
//! Locale-dependent paths (`-L`, `-T`) compare as the C locale does.
//!
//! `no_std` + `alloc`: files come through [`Files`].

#![no_std]
// Keep makeindexk's names and control structure, so the port can be read
// against the C source.
#![allow(
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::single_match_else,
    clippy::naive_bytecount,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::struct_field_names,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

extern crate alloc;

mod idx;
mod ind;
mod session;
mod sort;
mod sty;

pub use session::{Session, Stats};

use alloc::vec::Vec;

/// Where the program's files come from.
pub trait Files {
    /// An input file (`.idx`, `.log`) by name, if it exists and is readable.
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>>;
    /// `access(name, R_OK)`.
    fn readable(&mut self, name: &[u8]) -> bool;
    /// A style, looked up as kpathsea's `ist` format: the name found and
    /// the contents (`None` inside when found but unreadable).
    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)>;
    /// kpathsea's `kpse_out_name_ok`: `Err` with what it prints if refused.
    fn out_name_ok(&mut self, name: &[u8]) -> Result<(), Vec<u8>>;
    /// Standard input (`-i`, or no input files).
    fn stdin(&mut self) -> Vec<u8>;
}

/// A file the run writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutFile {
    pub name: Vec<u8>,
    pub contents: Vec<u8>,
}

/// What a run produced.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Standard error: progress, errors and usage lines.
    pub stderr: Vec<u8>,
    /// Standard output (the index, when it goes there).
    pub stdout: Vec<u8>,
    pub ind: Option<OutFile>,
    pub ilg: Option<OutFile>,
    /// Exit status.
    pub status: i32,
}

pub(crate) const EOF: i32 = -1;
const NUL: u8 = 0;
const LFD: i32 = b'\n' as i32;
const TAB: i32 = b'\t' as i32;
const SPC: i32 = b' ' as i32;

pub(crate) const ARGUMENT_MAX: usize = 10240;
pub(crate) const ARRAY_MAX: usize = 1024;
pub(crate) const FIELD_MAX: usize = 3;
pub(crate) const NUMBER_MAX: usize = 99;
pub(crate) const PAGEFIELD_MAX: usize = 10;
pub(crate) const PAGETYPE_MAX: usize = 5;
pub(crate) const STRING_MAX: usize = 999;

pub(crate) const EMPTY: i32 = -9999;
pub(crate) const ROML: i32 = 0;
pub(crate) const ROMU: i32 = 1;
pub(crate) const ARAB: i32 = 2;
pub(crate) const ALPL: i32 = 3;
pub(crate) const ALPU: i32 = 4;
pub(crate) const DUPLICATE: i32 = 9999;
pub(crate) const SYMBOL: i32 = -1;
pub(crate) const ALPHA: i32 = -2;

const DOT_MAX: i32 = 1000;
const CMP_MAX: i32 = 1500;

const USAGE: &str = "Usage: %s [-ilqrcgLT] [-s sty] [-o ind] [-t log] [-p num] [idx0 idx1 ...]\n";

/// An input stream read as makeindex's `mk_getc` reads (a CR before an LF
/// is dropped); `raw`/`unget` are the stdio calls `fscanf` makes.
#[derive(Default)]
pub(crate) struct Reader {
    data: Vec<u8>,
    pos: usize,
    lookahead: i32,
}

impl Reader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            pos: 0,
            lookahead: -2,
        }
    }

    pub(crate) fn raw(&mut self) -> i32 {
        let c = self.data.get(self.pos).map_or(EOF, |&c| i32::from(c));
        if c != EOF {
            self.pos += 1;
        }
        c
    }

    pub(crate) fn unget(&mut self, c: i32) {
        if c != EOF {
            self.pos -= 1;
        }
    }

    /// `mk_getc`.
    pub(crate) fn getc(&mut self) -> i32 {
        let mut ch = if self.lookahead == -2 {
            self.raw()
        } else {
            self.lookahead
        };
        self.lookahead = if ch == b'\r'.into() { self.raw() } else { -2 };
        if self.lookahead == LFD {
            ch = self.lookahead;
            self.lookahead = -2;
        }
        ch
    }
}

/// `KFIELD`: an index entry.
#[derive(Clone, Default)]
pub(crate) struct Field {
    /// Sort keys.
    pub sf: [Vec<u8>; FIELD_MAX],
    /// Actual keys.
    pub af: [Vec<u8>; FIELD_MAX],
    pub group: i32,
    /// The page number as written.
    pub lpg: Vec<u8>,
    pub npg: [i32; PAGEFIELD_MAX],
    pub count: usize,
    pub typ: i32,
    pub encap: Vec<u8>,
    /// Which input file.
    pub fn_: usize,
    pub lc: i32,
}

/// The style's settings (scanst.c's globals).
pub(crate) struct Style {
    pub idx_keyword: Vec<u8>,
    pub idx_aopen: u8,
    pub idx_aclose: u8,
    pub idx_level: u8,
    pub idx_ropen: u8,
    pub idx_rclose: u8,
    pub idx_quote: u8,
    pub idx_actual: u8,
    pub idx_encap: u8,
    pub idx_escape: u8,
    pub preamble: Vec<u8>,
    pub postamble: Vec<u8>,
    pub prelen: i32,
    pub postlen: i32,
    pub setpage_open: Vec<u8>,
    pub setpage_close: Vec<u8>,
    pub setpagelen: i32,
    pub group_skip: Vec<u8>,
    pub skiplen: i32,
    pub headings_flag: i32,
    pub heading_pre: Vec<u8>,
    pub heading_suf: Vec<u8>,
    pub headprelen: i32,
    pub headsuflen: i32,
    pub symhead_pos: Vec<u8>,
    pub symhead_neg: Vec<u8>,
    pub numhead_pos: Vec<u8>,
    pub numhead_neg: Vec<u8>,
    pub item_r: [Vec<u8>; FIELD_MAX],
    pub item_u: [Vec<u8>; FIELD_MAX],
    pub item_x: [Vec<u8>; FIELD_MAX],
    pub ilen_r: [i32; FIELD_MAX],
    pub ilen_u: [i32; FIELD_MAX],
    pub ilen_x: [i32; FIELD_MAX],
    pub delim_p: [Vec<u8>; FIELD_MAX],
    pub delim_n: Vec<u8>,
    pub delim_r: Vec<u8>,
    pub delim_t: Vec<u8>,
    pub suffix_2p: Vec<u8>,
    pub suffix_3p: Vec<u8>,
    pub suffix_mp: Vec<u8>,
    pub encap_p: Vec<u8>,
    pub encap_i: Vec<u8>,
    pub encap_s: Vec<u8>,
    pub linemax: i32,
    pub indent_length: i32,
    pub indent_space: Vec<u8>,
    pub page_comp: Vec<u8>,
    pub page_offset: [i32; PAGETYPE_MAX],
    pub page_prec: Vec<u8>,
}

fn v(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

impl Default for Style {
    fn default() -> Self {
        let item = |a: &str| [v(a), v("\n    \\subitem "), v("\n      \\subsubitem ")];
        Self {
            idx_keyword: v("\\indexentry"),
            idx_aopen: b'{',
            idx_aclose: b'}',
            idx_level: b'!',
            idx_ropen: b'(',
            idx_rclose: b')',
            idx_quote: b'"',
            idx_actual: b'@',
            idx_encap: b'|',
            idx_escape: b'\\',
            preamble: v("\\begin{theindex}\n"),
            postamble: v("\n\n\\end{theindex}\n"),
            prelen: 1,
            postlen: 3,
            setpage_open: v("\n  \\setcounter{page}{"),
            setpage_close: v("}\n"),
            setpagelen: 2,
            group_skip: v("\n\n  \\indexspace\n"),
            skiplen: 3,
            headings_flag: 0,
            heading_pre: Vec::new(),
            heading_suf: Vec::new(),
            headprelen: 0,
            headsuflen: 0,
            symhead_pos: v("Symbols"),
            symhead_neg: v("symbols"),
            numhead_pos: v("Numbers"),
            numhead_neg: v("numbers"),
            item_r: item("\n  \\item "),
            item_u: item(""),
            item_x: item(""),
            ilen_r: [1, 1, 1],
            ilen_u: [0, 1, 1],
            ilen_x: [0, 1, 1],
            delim_p: [v(", "), v(", "), v(", ")],
            delim_n: v(", "),
            delim_r: v("--"),
            delim_t: Vec::new(),
            suffix_2p: Vec::new(),
            suffix_3p: Vec::new(),
            suffix_mp: Vec::new(),
            encap_p: v("\\"),
            encap_i: v("{"),
            encap_s: v("}"),
            linemax: 72,
            indent_length: 16,
            indent_space: v("\t\t"),
            page_comp: v("-"),
            page_offset: [0, 10000, 20000, 20026, 30026],
            page_prec: v("rnaRA"),
        }
    }
}

/// A fatal error: the run stops (`exit(1)`).
pub(crate) struct Fatal;

pub(crate) type R<T = ()> = Result<T, Fatal>;

/// A makeindex run.
pub(crate) struct Mk<'a> {
    files: &'a mut dyn Files,
    pub out: Outcome,
    pgm: Vec<u8>,
    version: Vec<u8>,
    pub ilg_to_stderr: bool,
    pub ind_to_stdout: bool,
    pub ilg: Vec<u8>,
    pub ind: Vec<u8>,
    pub ind_fn: Vec<u8>,
    ilg_fn: Vec<u8>,

    pub letter_ordering: bool,
    pub compress_blanks: bool,
    pub merge_page: bool,
    pub init_page: bool,
    pub even_odd: i32,
    pub verbose: bool,
    pub german_sort: bool,
    pub thai_sort: bool,
    pub locale_sort: bool,
    fn_no: i32,
    pub idx_dot: bool,
    pub idx_tt: i32,
    pub idx_et: i32,
    pub idx_gt: i32,
    pub pageno: Vec<u8>,
    need_version: bool,
    base: Vec<u8>,

    pub st: Style,
    pub sty_fn: Vec<u8>,
    pub sty: Reader,

    /// The input file names, by entry `fn_`.
    pub idx_names: Vec<Vec<u8>>,
    pub idx_fn: usize,
    pub idx: Reader,
    pub idx_lc: i32,
    pub idx_tc: i32,
    pub idx_ec: i32,
    pub idx_dc: i32,
    pub comp_len: usize,
    /// scanid.c's static `key` buffer (stale bytes past its end stay).
    pub key: Vec<u8>,
    /// scanid.c's static `no` buffer.
    pub no: Vec<u8>,
    pub type_guess: [i32; PAGEFIELD_MAX + 1],
    pub entries: Vec<Field>,
    /// The sorted entries (`idx_key`).
    pub idx_key: Vec<usize>,
    pub idx_gc: i64,
    /// A session's run: the output's blocks, by what they read
    /// (`session.rs`).
    pub(crate) memo: Option<session::Memo>,
}

/// Formatting pieces for messages.
pub(crate) enum P<'s> {
    S(&'s [u8]),
    I(i64),
}

impl<'s> From<&'s str> for P<'s> {
    fn from(s: &'s str) -> Self {
        P::S(s.as_bytes())
    }
}

impl<'s> From<&'s [u8]> for P<'s> {
    fn from(s: &'s [u8]) -> Self {
        P::S(s)
    }
}

impl<'s> From<&'s Vec<u8>> for P<'s> {
    fn from(s: &'s Vec<u8>) -> Self {
        P::S(s)
    }
}

impl From<i32> for P<'_> {
    fn from(n: i32) -> Self {
        P::I(n.into())
    }
}

/// `printf` with `%s`, `%d`, `%ld` and `%c` (a one-byte string piece).
pub(crate) fn fmt(f: &str, args: &[P]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut args = args.iter();
    let b = f.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 1 < b.len() {
            let mut j = i + 1;
            while b[j] == b'l' {
                j += 1;
            }
            match (b[j], args.next()) {
                (b's' | b'c' | b'd', Some(P::S(s))) => out.extend_from_slice(s),
                (_, Some(P::I(n))) => out.extend_from_slice(alloc::format!("{n}").as_bytes()),
                _ => {}
            }
            i = j + 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// A C string: the bytes before the first NUL.
pub(crate) fn cstr(s: &[u8]) -> &[u8] {
    s.iter().position(|&c| c == NUL).map_or(s, |n| &s[..n])
}

/// Byte `i` of a C string, NUL past its end.
pub(crate) fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(NUL)
}

/// `TOLOWER` (the C locale).
pub(crate) fn tolower(c: u8) -> u8 {
    c.to_ascii_lowercase()
}

/// `TOUPPER` (the C locale).
pub(crate) fn toupper(c: u8) -> u8 {
    c.to_ascii_uppercase()
}

/// `strtoint`: decimal digits, no checks.
pub(crate) fn strtoint(s: &[u8]) -> i32 {
    cstr(s).iter().fold(0i32, |v, &c| {
        v.wrapping_mul(10).wrapping_add(i32::from(c) - 48)
    })
}

/// `sscanf("%d")` / `strtol` truncated to `int`, of an all-digit string.
fn atoi_sat(s: &[u8]) -> Option<i32> {
    if s.is_empty() {
        return None;
    }
    let mut v: i64 = 0;
    for &c in s {
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    Some(v as i32)
}

/// `ISSYMBOL` (on a signed `char`).
fn is_symbol(c: u8) -> bool {
    (b'!'..=b'@').contains(&c) || (b'['..=b'`').contains(&c) || (b'{'..=b'~').contains(&c)
}

/// `group_type`.
pub(crate) fn group_type(s: &[u8]) -> i32 {
    let s = cstr(s);
    if s.iter().all(u8::is_ascii_digit) {
        atoi_sat(s).unwrap_or(0)
    } else if is_symbol(at(s, 0)) {
        SYMBOL
    } else {
        ALPHA
    }
}

impl Mk<'_> {
    /// Write to the transcript.
    pub(crate) fn ilg_put(&mut self, s: &[u8]) {
        if self.ilg_to_stderr {
            self.out.stderr.extend_from_slice(s);
        } else {
            self.ilg.extend_from_slice(s);
        }
    }

    /// `MESSAGE`: the transcript, and the terminal when verbose.
    pub(crate) fn message(&mut self, f: &str, args: &[P]) {
        let s = fmt(f, args);
        if self.verbose {
            self.out.stderr.extend_from_slice(&s);
        }
        self.ilg_put(&s);
    }

    /// `FATAL`: the message and usage on the terminal, then exit 1.
    pub(crate) fn fatal(&mut self, f: &str, args: &[P]) -> Fatal {
        let s = fmt(f, args);
        self.out.stderr.extend_from_slice(&s);
        let u = fmt(USAGE, &[P::S(&self.pgm)]);
        self.out.stderr.extend_from_slice(&u);
        Fatal
    }

    /// `IDX_DOT`.
    pub(crate) fn idx_dot_tick(&mut self, max: i32) {
        self.idx_dot = true;
        let first = self.idx_dc == 0;
        self.idx_dc += 1;
        if first {
            if self.verbose {
                self.out.stderr.push(b'.');
            }
            self.ilg_put(b".");
        }
        if self.idx_dc == max {
            self.idx_dc = 0;
        }
    }

    /// `DONE`.
    pub(crate) fn done(&mut self, a: i32, b: &str, c: i32, d: &str) {
        self.message(
            "done (%d %s, %d %s).\n",
            &[a.into(), b.into(), c.into(), d.into()],
        );
    }

    /// The newline an error message starts with after dots.
    pub(crate) fn error_start(&mut self) {
        if self.idx_dot {
            self.ilg_put(b"\n");
            self.idx_dot = false;
        }
    }

    fn put_version(&mut self) {
        let pgm = self.pgm.clone();
        self.message("This is %s, ", &[P::S(&pgm)]);
        let version = self.version.clone();
        self.message("%s.\n", &[P::S(&version)]);
        self.need_version = false;
    }

    /// `open_sty`.
    fn open_sty(&mut self, name: &[u8]) -> R {
        match self.files.style(name) {
            None => Err(self.fatal("Index style file %s not found.\n", &[name.into()])),
            Some((found, _)) if found.len() >= STRING_MAX => {
                Err(self.fatal("Style file name %s too long.\n", &[P::S(&found)]))
            }
            Some((found, None)) => {
                Err(self.fatal("Could not open style file %s.\n", &[P::S(&found)]))
            }
            Some((found, Some(data))) => {
                self.sty_fn = found;
                self.sty = Reader::new(data);
                Ok(())
            }
        }
    }

    /// `check_idx`: set `base`; with `open`, read the file.
    fn check_idx(&mut self, name: &[u8], open: bool) -> R {
        let ext = name.iter().rposition(|&c| c == b'.');
        let with_ext = ext.is_some_and(|e| e != 0 && !name[e + 1..].contains(&b'/'));
        let base = if with_ext {
            &name[..ext.unwrap_or(0)]
        } else {
            name
        };
        let base = cstr(base).to_vec();
        if base.len() >= STRING_MAX {
            return Err(self.fatal(
                "Index file name %s too long (max %d).\n",
                &[P::S(&base), (STRING_MAX as i32).into()],
            ));
        }
        self.base = base;
        let try_open = |mk: &mut Self, n: &[u8]| -> bool {
            if open {
                match mk.files.read(n) {
                    Some(d) => {
                        mk.idx = Reader::new(d);
                        mk.idx_names.push(n.to_vec());
                        mk.idx_fn = mk.idx_names.len() - 1;
                        true
                    }
                    None => false,
                }
            } else {
                mk.files.readable(n)
            }
        };
        if try_open(self, name) {
            return Ok(());
        }
        if with_ext {
            return Err(self.fatal("Input index file %s not found.\n", &[name.into()]));
        }
        let mut alt = self.base.clone();
        alt.extend_from_slice(b".idx");
        if try_open(self, &alt) {
            return Ok(());
        }
        let base = self.base.clone();
        Err(self.fatal(
            "Couldn't find input index file %s nor %s.\n",
            &[P::S(&base), P::S(&alt)],
        ))
    }

    fn with_base(&self, ext: &str) -> Vec<u8> {
        let mut n = self.base.clone();
        n.extend_from_slice(ext.as_bytes());
        n
    }

    /// Whether `name` may be written; its refusal message goes to the
    /// terminal.
    fn out_ok(&mut self, name: &[u8]) -> bool {
        match self.files.out_name_ok(name) {
            Ok(()) => true,
            Err(msg) => {
                self.out.stderr.extend_from_slice(&msg);
                false
            }
        }
    }

    /// `check_all`.
    fn check_all(
        &mut self,
        name: &[u8],
        ind: Option<&[u8]>,
        ilg: Option<&[u8]>,
        log_given: bool,
    ) -> R {
        self.check_idx(name, true)?;
        self.ind_fn = ind.map_or_else(|| self.with_base(".ind"), <[u8]>::to_vec);
        let ind_fn = self.ind_fn.clone();
        if !self.out_ok(&ind_fn) {
            return Err(self.fatal("Can't create output index file %s.\n", &[P::S(&ind_fn)]));
        }
        self.out.ind = Some(OutFile::default());
        self.ilg_fn = ilg.map_or_else(|| self.with_base(".ilg"), <[u8]>::to_vec);
        let ilg_fn = self.ilg_fn.clone();
        if !self.out_ok(&ilg_fn) {
            return Err(self.fatal("Can't create transcript file %s.\n", &[P::S(&ilg_fn)]));
        }
        self.out.ilg = Some(OutFile::default());
        if log_given {
            let log_fn = self.with_base(".log");
            match self.files.read(&log_fn) {
                None => {
                    return Err(self.fatal("Source log file %s not found.\n", &[P::S(&log_fn)]));
                }
                Some(log) => self.find_pageno(&log, &log_fn),
            }
        }
        Ok(())
    }

    /// `find_pageno`: the last `[N` in the log.
    fn find_pageno(&mut self, log: &[u8], log_fn: &[u8]) {
        let len = log.len();
        let found = (0..len.saturating_sub(1))
            .rev()
            .find(|&k| log[k] == b'[' && log[k + 1].is_ascii_digit());
        let start = match found {
            Some(k) => Some(k + 1),
            None if len >= 2 && log[0] == b'[' => Some(1),
            None => None,
        };
        match start {
            Some(mut pos) => {
                let mut get = || {
                    let c = log.get(pos).map_or(EOF, |&c| i32::from(c));
                    pos += 1;
                    c
                };
                let mut c = get();
                while c == SPC {
                    c = get();
                }
                self.pageno.clear();
                loop {
                    self.pageno.push(c as u8);
                    c = get();
                    if !(0..256).contains(&c) || !(c as u8).is_ascii_digit() {
                        break;
                    }
                }
            }
            None => {
                let s = fmt(
                    "Couldn't find any page number in %s...ignored\n",
                    &[log_fn.into()],
                );
                self.ilg_put(&s);
                self.init_page = false;
            }
        }
    }

    /// `process_idx`.
    fn process_idx(
        &mut self,
        fns: &[Vec<u8>],
        mut use_stdin: bool,
        sty_given: bool,
        ind: Option<&[u8]>,
        ilg: Option<&[u8]>,
        log_given: bool,
    ) -> R {
        let (mut ind_given, mut ilg_given) = (ind.is_some(), ilg.is_some());
        if self.fn_no == -1 {
            use_stdin = true;
        } else {
            self.check_all(&fns[0], ind, ilg, log_given)?;
            self.put_version();
            if sty_given {
                self.scan_sty();
            }
            if self.german_sort && self.st.idx_quote == b'"' {
                return Err(self.fatal(
                    "Option -g invalid, quote character must be different from '%c'.\n",
                    &["\"".into()],
                ));
            }
            self.scan_idx();
            ind_given = true;
            ilg_given = true;
            for f in &fns[1..] {
                self.check_idx(f, true)?;
                self.scan_idx();
            }
        }
        if use_stdin {
            let data = self.files.stdin();
            self.idx = Reader::new(data);
            self.idx_names.push(b"stdin".to_vec());
            self.idx_fn = self.idx_names.len() - 1;
            if ind_given {
                if let Some(n) = ind.filter(|_| self.out.ind.is_none()) {
                    self.ind_fn = n.to_vec();
                }
                let ind_fn = self.ind_fn.clone();
                if !self.out_ok(&ind_fn) {
                    return Err(
                        self.fatal("Can't create output index file %s.\n", &[P::S(&ind_fn)])
                    );
                }
                self.out.ind.get_or_insert_with(OutFile::default);
            } else {
                self.ind_fn = b"stdout".to_vec();
                self.ind_to_stdout = true;
            }
            if ilg_given {
                if let Some(n) = ilg.filter(|_| self.out.ilg.is_none()) {
                    self.ilg_fn = n.to_vec();
                }
                let ilg_fn = self.ilg_fn.clone();
                if !self.out_ok(&ilg_fn) {
                    return Err(self.fatal("Can't create transcript file %s.\n", &[P::S(&ilg_fn)]));
                }
                self.out.ilg.get_or_insert_with(OutFile::default);
            } else {
                self.ilg_fn = b"stderr".to_vec();
                self.ilg_to_stderr = true;
                self.verbose = false;
            }
            if self.fn_no == -1 && sty_given {
                self.scan_sty();
            }
            if self.german_sort && self.st.idx_quote == b'"' {
                return Err(self.fatal(
                    "Option -g ignored, quote character must be different from '%c'.\n",
                    &["\"".into()],
                ));
            }
            if self.need_version {
                self.put_version();
            }
            self.scan_idx();
            self.fn_no += 1;
        }
        Ok(())
    }

    /// `main`, after the program name.
    fn main(&mut self, args: &[Vec<u8>]) -> R {
        let mut fns: Vec<Vec<u8>> = Vec::new();
        let (mut use_stdin, mut sty_given, mut log_given) = (false, false, false);
        let (mut ind, mut ilg): (Option<Vec<u8>>, Option<Vec<u8>>) = (None, None);
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg.first() == Some(&b'-') {
                if arg.len() == 1 {
                    break;
                }
                for &c in cstr(&arg[1..]) {
                    match c {
                        b'i' => use_stdin = true,
                        b'l' => self.letter_ordering = true,
                        b'r' => self.merge_page = false,
                        b'q' => self.verbose = false,
                        b'c' => self.compress_blanks = true,
                        b's' => {
                            let Some(n) = args.next() else {
                                return Err(self.fatal("Expected -s <stylefile>\n", &[]));
                            };
                            self.open_sty(n)?;
                            sty_given = true;
                        }
                        b'o' => {
                            let Some(n) = args.next() else {
                                return Err(self.fatal("Expected -o <ind>\n", &[]));
                            };
                            ind = Some(n.clone());
                        }
                        b't' => {
                            let Some(n) = args.next() else {
                                return Err(self.fatal("Expected -t <logfile>\n", &[]));
                            };
                            ilg = Some(n.clone());
                        }
                        b'p' => {
                            let Some(n) = args.next() else {
                                return Err(self.fatal("Expected -p <num>\n", &[]));
                            };
                            if n.len() >= NUMBER_MAX {
                                return Err(self.fatal("Page number too high\n", &[]));
                            }
                            self.pageno = cstr(n).to_vec();
                            self.init_page = true;
                            match &self.pageno[..] {
                                b"even" => (log_given, self.even_odd) = (true, 2),
                                b"odd" => (log_given, self.even_odd) = (true, 1),
                                b"any" => (log_given, self.even_odd) = (true, 0),
                                _ => {}
                            }
                        }
                        b'g' => self.german_sort = true,
                        b'L' => self.locale_sort = true,
                        b'T' => (self.thai_sort, self.locale_sort) = (true, true),
                        _ => {
                            return Err(self.fatal("Unknown option -%c.\n", &[P::S(&[c])]));
                        }
                    }
                }
            } else if self.fn_no < ARRAY_MAX as i32 {
                self.check_idx(arg, false)?;
                fns.push(arg.clone());
                self.fn_no += 1;
            } else {
                return Err(self.fatal(
                    "Too many input files (max %d).\n",
                    &[(ARRAY_MAX as i32).into()],
                ));
            }
        }
        if self.fn_no == 0 && !sty_given {
            let tmp = self.with_base(".mst");
            if self.files.readable(&tmp) {
                self.open_sty(&tmp)?;
                sty_given = true;
            }
        }
        self.process_idx(
            &fns,
            use_stdin,
            sty_given,
            ind.as_deref(),
            ilg.as_deref(),
            log_given,
        )?;
        self.idx_gt = self.idx_tt - self.idx_et;
        if self.fn_no > 0 {
            let s = fmt(
                "Overall %d files read (%d entries accepted, %d rejected).\n",
                &[
                    (self.fn_no + 1).into(),
                    self.idx_gt.into(),
                    self.idx_et.into(),
                ],
            );
            if self.verbose {
                self.out.stderr.extend_from_slice(&s);
            }
            self.ilg_put(&s);
        }
        let ind_fn = self.ind_fn.clone();
        if self.idx_gt > 0 {
            if self.entries.is_empty() {
                return Err(self.fatal("No valid index entries collected.\n", &[]));
            }
            self.idx_key = (0..self.idx_gt as usize).collect();
            self.sort_idx();
            self.gen_ind();
            self.message("Output written in %s.\n", &[P::S(&ind_fn)]);
        } else {
            self.message("Nothing written in %s.\n", &[P::S(&ind_fn)]);
        }
        let ilg_fn = self.ilg_fn.clone();
        self.message("Transcript written in %s.\n", &[P::S(&ilg_fn)]);
        Ok(())
    }
}

/// Run `makeindex` with the command-line arguments `args` (after the
/// program name). `version` is what the banner prints after the program
/// name, e.g. `version 2.18 [TeX Live 2026] (kpathsea + Thai support)`.
pub fn run(args: &[Vec<u8>], version: &[u8], files: &mut dyn Files) -> Outcome {
    run_with(args, version, files, None).0
}

/// [`run`], its output's blocks looked up in `memo` (a session's): the
/// outcome and the blocks made.
pub(crate) fn run_with(
    args: &[Vec<u8>],
    version: &[u8],
    files: &mut dyn Files,
    memo: Option<session::Memo>,
) -> (Outcome, Option<session::Memo>) {
    let mut mk = Mk {
        files,
        out: Outcome::default(),
        pgm: b"makeindex".to_vec(),
        version: version.to_vec(),
        ilg_to_stderr: false,
        ind_to_stdout: false,
        ilg: Vec::new(),
        ind: Vec::new(),
        ind_fn: Vec::new(),
        ilg_fn: Vec::new(),
        letter_ordering: false,
        compress_blanks: false,
        merge_page: true,
        init_page: false,
        even_odd: -1,
        verbose: true,
        german_sort: false,
        thai_sort: false,
        locale_sort: false,
        fn_no: -1,
        idx_dot: true,
        idx_tt: 0,
        idx_et: 0,
        idx_gt: 0,
        pageno: Vec::new(),
        need_version: true,
        base: Vec::new(),
        st: Style::default(),
        sty_fn: Vec::new(),
        sty: Reader::default(),
        idx_names: Vec::new(),
        idx_fn: 0,
        idx: Reader::default(),
        idx_lc: 0,
        idx_tc: 0,
        idx_ec: 0,
        idx_dc: 0,
        comp_len: 0,
        key: alloc::vec![0; ARGUMENT_MAX + 2],
        no: alloc::vec![0; NUMBER_MAX + 1],
        type_guess: [EMPTY; PAGEFIELD_MAX + 1],
        entries: Vec::new(),
        idx_key: Vec::new(),
        idx_gc: 0,
        memo,
    };
    let status = i32::from(mk.main(args).is_err());
    let mut out = core::mem::take(&mut mk.out);
    out.status = status;
    if let Some(f) = &mut out.ind {
        f.name.clone_from(&mk.ind_fn);
        f.contents = core::mem::take(&mut mk.ind);
    } else if mk.ind_to_stdout {
        out.stdout = core::mem::take(&mut mk.ind);
    }
    if let Some(f) = &mut out.ilg {
        f.name.clone_from(&mk.ilg_fn);
        f.contents = core::mem::take(&mut mk.ilg);
    }
    (out, mk.memo.take())
}
