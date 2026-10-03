//! Part 22: Input stacks and states (§300–§320) and part 23: Maintaining
//! the input stacks (§321–§331), with line input (§31, web2c's
//! `input_line`) and `term_input` (§71).

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::node::Tokens;

use crate::host::Host;
use crate::mem::NULL;
use crate::print::PSEUDO;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// §300: one level of the input stack. A token-list level holds its list,
/// a shared value (DESIGN 7.17.12); `loc` is the index of the next token
/// in it, `NULL` once it is read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InStateRecord {
    pub list: Option<partex_engine::node::Tokens>,
    pub state: i32,
    pub index: i32,
    pub start: i32,
    pub loc: i32,
    pub limit: i32,
    pub name: i32,
}

partex_engine::persist_struct!(InStateRecord {
    list,
    state,
    index,
    start,
    loc,
    limit,
    name
});

/// e-TeX: a `\scantokens` pseudo file, a value (DESIGN 3.5): its lines, shared, and the index of the next one to read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PseudoFile {
    pub lines: Arc<[Vec<u8>]>,
    pub next: usize,
}

partex_engine::persist_struct!(PseudoFile { lines, next });

/// A stack as a value (DESIGN 3.5): an entry with the value of the
/// entries below it, so a push shares what is below and a pop returns
/// to it.
#[derive(Debug)]
pub(crate) struct Chain<T> {
    pub item: T,
    pub below: Option<Arc<Chain<T>>>,
}

/// The values of an array stack's prefixes, kept beside it while they
/// hold (DESIGN 3.5): `known[i]` is the value of `stack[..=i]`. A push or
/// pop at `i` forgets it, so the known ones are a prefix.
#[derive(Clone, Debug)]
pub(crate) struct Prefixes<T> {
    known: Vec<Option<Arc<Chain<T>>>>,
}

impl<T> Default for Prefixes<T> {
    fn default() -> Self {
        Prefixes { known: Vec::new() }
    }
}

impl<T: Clone> Prefixes<T> {
    /// Entry `i` changed: its value is no longer known.
    #[inline]
    pub(crate) fn forget(&mut self, i: usize) {
        if let Some(l) = self.known.get_mut(i) {
            *l = None;
        }
    }

    /// The value of `stack`, making only the entries not known.
    pub(crate) fn value(&mut self, stack: &[T]) -> Option<Arc<Chain<T>>> {
        let n = stack.len();
        if self.known.len() < n {
            self.known.resize(n, None);
        }
        let mut i = n;
        while i > 0 && self.known[i - 1].is_none() {
            i -= 1;
        }
        let mut below = i.checked_sub(1).and_then(|j| self.known[j].clone());
        for (j, item) in stack.iter().enumerate().skip(i) {
            let l = Arc::new(Chain {
                item: item.clone(),
                below,
            });
            self.known[j] = Some(Arc::clone(&l));
            below = Some(l);
        }
        below
    }

    /// Put `value`, of `depth` entries, into `stack`, and know it again.
    pub(crate) fn put(&mut self, value: Option<&Arc<Chain<T>>>, depth: usize, stack: &mut Vec<T>)
    where
        T: Default,
    {
        if stack.len() < depth {
            stack.resize(depth, T::default());
        }
        self.known.clear();
        self.known.resize(depth, None);
        let mut l = value;
        let mut j = depth;
        while let Some(x) = l {
            j -= 1;
            stack[j].clone_from(&x.item);
            self.known[j] = Some(Arc::clone(x));
            l = x.below.as_ref();
        }
        debug_assert_eq!(j, 0);
    }
}

/// Whether the chains `a` and `b` (of the same depth) agree by `f`,
/// entry by entry, down to an entry they share.
pub(crate) fn same_chain<T>(
    mut a: Option<&Arc<Chain<T>>>,
    mut b: Option<&Arc<Chain<T>>>,
    f: impl Fn(&T, &T) -> bool,
) -> bool {
    loop {
        match (a, b) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if Arc::ptr_eq(x, y) {
                    return true;
                }
                if !f(&x.item, &y.item) {
                    return false;
                }
                a = x.below.as_ref();
                b = y.below.as_ref();
            }
            _ => return false,
        }
    }
}

/// The open file levels below the top one (DESIGN 3.5): each level's
/// file and position, the line number saved when the level above it
/// opened, e-TeX's group and condition depths and `\everyeof` flag, and
/// the names. They do not change while a level above them is open.
#[derive(Clone, Default)]
pub(crate) struct FileLevels {
    pub files: Vec<Option<AlphaFile>>,
    pub lines: Vec<i32>,
    pub grp: Vec<i32>,
    pub ifs: Vec<usize>,
    pub eof: Vec<bool>,
    pub names: Vec<i32>,
    pub full_names: Vec<i32>,
}

/// The input's value beside the engine's stacks and buffer (DESIGN 3.5),
/// kept when the tracker keeps values.
#[derive(Clone, Default)]
pub(crate) struct InputValues {
    /// The input stack's prefixes.
    pub levels: Prefixes<InStateRecord>,
    /// The parameter stack's prefixes.
    pub params: Prefixes<Option<Tokens>>,
    /// The start in the buffer of each open file level's line, by level.
    pub starts: Vec<usize>,
    /// The buffer below the top file level's line (the lines of the
    /// levels below it, which do not change while it is open), once made.
    pub below: Option<Arc<[u8]>>,
    /// The file levels below the top one, once made.
    pub files: Option<Arc<FileLevels>>,
}

/// The input's value (DESIGN 3.5), as `Tex::input_value` takes it.
#[derive(Clone)]
pub(crate) struct InputValue {
    /// The levels below the current one, and how many.
    pub levels: Option<Arc<Chain<InStateRecord>>>,
    pub depth: usize,
    /// The parameters, and how many.
    pub params: Option<Arc<Chain<Option<Tokens>>>>,
    pub np: usize,
    /// The buffer below the top file level's line, and where it starts.
    pub below: Arc<[u8]>,
    pub from: usize,
    /// The file levels below the top one.
    pub files: Arc<FileLevels>,
}

/// §25: an `alpha_file` open for reading. The host hands over whole files;
/// `pos` is the read position. For an `\input` level read by lines with
/// a tracker that follows them, the line in the buffer: where it began
/// and the tracker's catcode generation then (`line_open`).
#[derive(Clone, Default)]
pub(crate) struct AlphaFile {
    pub(crate) data: Arc<[u8]>,
    pub(crate) pos: usize,
    pub(crate) line_from: usize,
    pub(crate) line_generation: u64,
    pub(crate) line_open: bool,
    /// Lines read so far (`input_ln`): with the contents, where `pos` is.
    pub(crate) lines: u32,
    /// The file's name as the host resolved it (empty for the terminal).
    pub(crate) name: Arc<[u8]>,
    /// With glyph origins kept (`srcmap.rs`): where the command main
    /// control last took from this level began, in `data` (a
    /// synthesized glyph's range starts there).
    pub(crate) call: usize,
}

partex_engine::persist_struct!(AlphaFile {
    data,
    pos,
    line_from,
    line_generation,
    line_open,
    lines,
    name,
    call
});

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The input files open now (`\input` levels and `\openin` streams)
    /// and how far each has been read: what a checkpoint taken now depends
    /// on (besides the files already closed, read whole).
    pub fn open_inputs(&self) -> alloc::vec::Vec<(Arc<[u8]>, usize)> {
        self.input_file
            .iter()
            .chain(&self.read_file)
            .flatten()
            .map(|f| (f.data.clone(), f.pos))
            .collect()
    }

    /// The buffer as the state hash sees it, with the input levels'
    /// `(start, limit)` (`PARTEX_WATCH_DEBUG`).
    pub fn buffer_view(&self) -> (alloc::vec::Vec<u8>, alloc::vec::Vec<(i32, i32)>) {
        let n = self.first.max(self.last);
        let levels = self.input_stack[..self.input_ptr]
            .iter()
            .chain(core::iter::once(&self.cur_input))
            .map(|r| (r.start, r.limit))
            .collect();
        (self.buffer.prefix(n).iter().copied().collect(), levels)
    }

    /// Continue reading `new` wherever `old` is open (a file changed after
    /// the part read so far).
    pub fn replace_input(&mut self, old: &Arc<[u8]>, new: &Arc<[u8]>) {
        for f in self
            .input_file
            .iter_mut()
            .chain(&mut self.read_file)
            .flatten()
        {
            if Arc::ptr_eq(&f.data, old) {
                f.data = new.clone();
            }
        }
    }

    /// Continue reading `new` wherever `old` is open, with each read
    /// position moved by `map` (an old line's start to the new one's,
    /// and whether that line changed; the end to the end): false, with
    /// nothing changed, if a level's line in the buffer changed or a
    /// position is not a line's start.
    pub fn remap_input(
        &mut self,
        old: &Arc<[u8]>,
        new: &Arc<[u8]>,
        map: impl Fn(usize) -> Option<(usize, bool)>,
    ) -> bool {
        let fits = |f: &AlphaFile, level: bool| {
            map(f.pos).is_some()
                && (!level || !f.line_open || map(f.line_from).is_some_and(|(_, c)| !c))
        };
        let ok = self
            .input_file
            .iter()
            .flatten()
            .all(|f| !Arc::ptr_eq(&f.data, old) || fits(f, true))
            && self
                .read_file
                .iter()
                .flatten()
                .all(|f| !Arc::ptr_eq(&f.data, old) || fits(f, false));
        if !ok {
            return false;
        }
        for f in self
            .input_file
            .iter_mut()
            .chain(&mut self.read_file)
            .flatten()
        {
            if Arc::ptr_eq(&f.data, old) {
                f.data = new.clone();
                f.pos = map(f.pos).map_or(f.pos, |m| m.0);
                if f.line_open {
                    f.line_from = map(f.line_from).map_or(f.line_from, |m| m.0);
                }
            }
        }
        true
    }

    /// Token-level input dependencies: the line in the buffer of the
    /// `\input` file at `index` is done.
    pub(crate) fn end_line_of(&mut self, index: usize) {
        if !T::LINES {
            return;
        }
        let Some(f) = self.input_file[index].as_mut() else {
            return;
        };
        if !f.line_open {
            return;
        }
        f.line_open = false;
        let (from, generation) = (f.line_from, f.line_generation);
        let f = self.input_file[index].as_ref().expect("open");
        self.tracker.line_end(&f.data, from, generation, || {
            if (0..=255).any(|c| usize::from(self.xord[c]) != c) {
                return None;
            }
            let mut cat = [0u8; 256];
            for (c, v) in cat.iter_mut().enumerate() {
                let p = CAT_CODE_BASE + i32::try_from(c).unwrap_or(0);
                *v = u8::try_from(self.peek_eqtb(p).rh()).unwrap_or(255);
            }
            let end_line_char = self.peek_eqtb(INT_BASE + END_LINE_CHAR_CODE).int();
            Some(crate::track::LineCodes { cat, end_line_char })
        });
    }

    /// Token-level input dependencies: `f` read the line from `from` into
    /// the buffer (for an `\input` level, `follow`).
    pub(crate) fn start_line(&self, f: &mut AlphaFile, from: usize, follow: bool) {
        if T::LINES {
            let generation = self.tracker.line_start(&f.data, from);
            if follow {
                f.line_from = from;
                f.line_generation = generation;
                f.line_open = true;
            }
        }
    }

    /// `f` met its end at byte `from`, no line there (a read of the next
    /// line that found none): what lines added there change.
    pub(crate) fn end_of_file(&self, f: &AlphaFile, from: usize) {
        if T::LINES {
            self.tracker.eof_read(&f.data, from);
        }
    }

    /// Continue reading `new` wherever `old` is open, with the same input
    /// left (a file changed in the part read so far, and each open file's
    /// rest ends both).
    pub fn replace_input_before(&mut self, old: &Arc<[u8]>, new: &Arc<[u8]>) {
        for f in self
            .input_file
            .iter_mut()
            .chain(&mut self.read_file)
            .flatten()
        {
            if Arc::ptr_eq(&f.data, old) {
                let rest = f.data.len() - f.pos;
                let pos = new.len() - rest;
                if f.line_open {
                    // (the line in the buffer moves with the rest, if it
                    // is still there; else its tokens are not followed)
                    let line_rest = f.data.len() - f.line_from;
                    let from = new.len().checked_sub(line_rest);
                    match from {
                        Some(from) if new.get(from..pos) == f.data.get(f.line_from..f.pos) => {
                            f.line_from = from;
                        }
                        _ => f.line_open = false,
                    }
                }
                f.data = new.clone();
                f.pos = pos;
            }
        }
    }

    // §302, §303: the fields of `cur_input`.
    #[inline]
    pub(crate) fn state(&self) -> i32 {
        self.cur_input.state
    }
    #[inline]
    pub(crate) fn token_type(&self) -> i32 {
        self.cur_input.index
    }
    #[inline]
    pub(crate) fn terminal_input(&self) -> bool {
        self.cur_input.name == 0
    }

    /// §31 (web2c's `input_line`): read the next line of `f` into
    /// `buffer[first..last)`. Lines end at LF, CR or CRLF; trailing spaces
    /// (not tabs) are removed; the bytes go through `xord`.
    pub(crate) fn input_ln(&mut self, f: &mut AlphaFile) -> Result<bool, Jump> {
        let buf_size = self.buffer.len() - 1;
        self.last = self.first;
        let data = &f.data;
        let mut i = f.pos;
        if T::LINES && self.log_lines && !f.name.is_empty() {
            // (a machine's line cells: line `lines` of the file, absent
            // at its end)
            self.line_log.push((f.name.clone(), f.lines));
        }
        if i >= data.len() {
            return Ok(false);
        }
        f.lines += 1;
        let mut terminated = false;
        while i < data.len() {
            let c = data[i];
            if c == b'\n' || c == b'\r' {
                terminated = true;
                break;
            }
            if self.last >= buf_size {
                // web2c aborts: "! Unable to read an entire line".
                return self.buffer_overflow();
            }
            self.buffer[self.last] = c;
            self.last += 1;
            i += 1;
        }
        if terminated {
            // If next char is LF of a CRLF, read it.
            if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            i += 1;
        }
        f.pos = i;
        self.buffer[self.last] = b' ';
        if self.last >= self.max_buf_stack {
            self.max_buf_stack = self.last;
        }
        while self.last > self.first && self.buffer[self.last - 1] == b' ' {
            self.last -= 1;
        }
        for k in self.first..=self.last {
            self.buffer[k] = self.xord[usize::from(self.buffer[k])];
        }
        Ok(true)
    }

    /// §35: report overflow of the input buffer, and abort.
    fn buffer_overflow<R>(&mut self) -> Result<R, Jump> {
        self.cur_input.loc = i32::try_from(self.first).unwrap_or(i32::MAX);
        self.cur_input.limit = i32::try_from(self.last).unwrap_or(i32::MAX) - 1;
        let n = self.params.buf_size;
        self.overflow(b"buffer size", n)
    }

    /// §31 for the terminal: read a line from the host.
    pub(crate) fn input_ln_terminal(&mut self) -> Result<bool, Jump> {
        match self.host.term_read_line() {
            None => Ok(false),
            Some(mut line) => {
                line.push(b'\n');
                let mut f = AlphaFile {
                    data: Arc::from(line),
                    ..AlphaFile::default()
                };
                self.input_ln(&mut f)
            }
        }
    }

    /// web2c's `print_buffer`: print `buffer[i]` (encTeX's `\mubytein`
    /// conversion is inactive without encTeX).
    pub(crate) fn print_buffer(&mut self, i: &mut usize) {
        self.print(i32::from(self.buffer[*i]));
        *i += 1;
    }

    /// §71: get a line of input from the terminal.
    pub(crate) fn term_input(&mut self) -> Result<(), Jump> {
        self.update_terminal(); // now the user sees the prompt for sure
        if !self.input_ln_terminal()? {
            self.cur_input.limit = 0;
            return self.fatal_error(b"End of file on the terminal!");
        }
        if T::VALUES {
            let line = self.buffer[self.first..self.last].to_vec();
            self.clock_read(crate::track::Query::Terminal, &line[..]);
        }
        self.term_offset = 0; // the user's line ended with <return>
        self.offsets_wrote(true, false);
        self.set_selector(self.selector() - 1); // prepare to echo the input
        let mut k = self.first;
        while k < self.last {
            self.print_buffer(&mut k);
        }
        self.print_ln();
        self.set_selector(self.selector() + 1); // restore previous status
        Ok(())
    }

    /// §71: `prompt_input(s)`.
    pub(crate) fn prompt_input(&mut self, s: &[u8]) -> Result<(), Jump> {
        self.print_str(s);
        self.term_input()
    }

    /// §306: warn about a runaway definition, argument, preamble or text.
    pub(crate) fn runaway(&mut self) {
        if self.scanner_status > SKIPPING {
            let p = match self.scanner_status {
                DEFINING => {
                    self.print_nl(b"Runaway definition");
                    self.def_ref.clone()
                }
                MATCHING => {
                    self.print_nl(b"Runaway argument");
                    self.arg_list.clone()
                }
                ALIGNING => {
                    self.print_nl(b"Runaway preamble");
                    self.preamble_list.clone()
                }
                _ => {
                    self.print_nl(b"Runaway text");
                    self.def_ref.clone()
                }
            };
            self.print_char(b'?');
            self.print_ln();
            let l = self.params.error_line - 10;
            self.show_token_list(&p, NULL, l);
        }
    }

    /// §311: print where the scanner is.
    pub(crate) fn show_context(&mut self) {
        self.base_ptr = self.input_ptr;
        self.input_stack[self.base_ptr] = self.cur_input.clone(); // store current state
        let mut nn = -1;
        let mut bottom_line = false;
        loop {
            self.cur_input = self.input_stack[self.base_ptr].clone(); // enter into the context
            // (e-TeX: a `\scantokens` pseudo file, names 18 and 19, is
            // not the bottom line)
            if self.state() != TOKEN_LIST && (self.cur_input.name > 19 || self.base_ptr == 0) {
                bottom_line = true;
            }
            let lines = self.int_par(ERROR_CONTEXT_LINES_CODE);
            if self.base_ptr == self.input_ptr || bottom_line || nn < lines {
                // §312: display the current context.
                if self.base_ptr == self.input_ptr
                    || self.state() != TOKEN_LIST
                    || self.token_type() != BACKED_UP
                    || self.cur_input.loc != NULL
                {
                    // we omit backed-up token lists that have already been read
                    if T::LINES
                        && self.state() != TOKEN_LIST
                        && self.cur_input.name > 19
                        && let Some(f) = &self.input_file[ux(self.cur_input.index)]
                    {
                        self.tracker.line_shown(&f.data, f.line_from);
                    }
                    self.display_context();
                    nn += 1;
                }
            } else if nn == lines {
                self.print_nl(b"...");
                nn += 1; // omitted if `error_context_lines<0`
            }
            if bottom_line {
                break;
            }
            self.base_ptr -= 1;
        }
        self.cur_input = self.input_stack[self.input_ptr].clone(); // restore original state
    }

    /// §312: the body of "Display the current context".
    fn display_context(&mut self) {
        self.tally = 0; // get ready to count characters
        let old_setting = self.selector();
        let l;
        if self.state() == TOKEN_LIST {
            // §314: print type of token list.
            match self.token_type() {
                PARAMETER => self.print_nl(b"<argument> "),
                U_TEMPLATE | V_TEMPLATE => self.print_nl(b"<template> "),
                BACKED_UP => {
                    if self.cur_input.loc == NULL {
                        self.print_nl(b"<recently read> ");
                    } else {
                        self.print_nl(b"<to be read again> ");
                    }
                }
                INSERTED => self.print_nl(b"<inserted text> "),
                MACRO => {
                    self.print_ln();
                    self.print_cs(self.cur_input.name);
                }
                OUTPUT_TEXT => self.print_nl(b"<output> "),
                EVERY_PAR_TEXT => self.print_nl(b"<everypar> "),
                EVERY_MATH_TEXT => self.print_nl(b"<everymath> "),
                EVERY_DISPLAY_TEXT => self.print_nl(b"<everydisplay> "),
                EVERY_HBOX_TEXT => self.print_nl(b"<everyhbox> "),
                EVERY_VBOX_TEXT => self.print_nl(b"<everyvbox> "),
                EVERY_JOB_TEXT => self.print_nl(b"<everyjob> "),
                EVERY_CR_TEXT => self.print_nl(b"<everycr> "),
                MARK_TEXT => self.print_nl(b"<mark> "),
                EVERY_EOF_TEXT => self.print_nl(b"<everyeof> "),
                WRITE_TEXT => self.print_nl(b"<write> "),
                _ => self.print_nl(b"?"), // this should never happen
            }
            // §319: pseudoprint the token list.
            l = self.begin_pseudoprint();
            let list = self.cur_input.list.clone().unwrap_or_default();
            self.show_token_list(&list, self.cur_input.loc, 100_000);
        } else {
            // §313: print location of current line.
            if self.cur_input.name <= 17 {
                if self.terminal_input() {
                    if self.base_ptr == 0 {
                        self.print_nl(b"<*>");
                    } else {
                        self.print_nl(b"<insert> ");
                    }
                } else {
                    self.print_nl(b"<read ");
                    if self.cur_input.name == 17 {
                        self.print_char(b'*');
                    } else {
                        self.print_int(self.cur_input.name - 1);
                    }
                    self.print_char(b'>');
                }
            } else {
                self.print_nl(b"l.");
                let index = ux(self.cur_input.index);
                if index == self.in_open {
                    self.print_line_no(self.in_open, self.line);
                } else {
                    // e-TeX: input from a pseudo file
                    self.print_line_no(index, self.line_stack[index + 1]);
                }
            }
            self.print_char(b' ');
            // §318: pseudoprint the line.
            l = self.begin_pseudoprint();
            let limit = ux(self.cur_input.limit);
            let j = if i32::from(self.buffer[limit]) == self.int_par(END_LINE_CHAR_CODE) {
                limit
            } else {
                limit + 1 // determine the effective end of the line
            };
            let mut i = ux(self.cur_input.start);
            // encTeX's `mubyte_keep`/`mubyte_start` save and restore is
            // inert without encTeX.
            if j > 0 {
                while i < j {
                    if i == ux(self.cur_input.loc) {
                        self.set_trick_count();
                    }
                    self.print_buffer(&mut i);
                }
            }
        }
        self.set_selector(old_setting); // stop pseudoprinting
        self.print_two_lines(l);
    }

    /// §316: `begin_pseudoprint`; returns `l`.
    fn begin_pseudoprint(&mut self) -> i32 {
        let l = self.tally;
        self.tally = 0;
        self.set_selector(PSEUDO);
        self.trick_count = 1_000_000;
        l
    }

    /// §317: print two lines using the tricky pseudoprinted information.
    fn print_two_lines(&mut self, l: i32) {
        let error_line = self.params.error_line;
        let half_error_line = self.params.half_error_line;
        if self.trick_count == 1_000_000 {
            self.set_trick_count(); // `set_trick_count` must be performed
        }
        let m = if self.tally < self.trick_count {
            self.tally - self.first_count
        } else {
            self.trick_count - self.first_count // context on line 2
        };
        let (p, n) = if l + self.first_count <= half_error_line {
            (0, l + self.first_count)
        } else {
            self.print_str(b"...");
            (l + self.first_count - half_error_line + 3, half_error_line)
        };
        for q in p..self.first_count {
            self.print_char(self.trick_buf[ux(q % error_line)]);
        }
        self.print_ln();
        for _ in 0..n {
            self.print_char(b' '); // print `n` spaces to begin line 2
        }
        let p = if m + n <= error_line {
            self.first_count + m
        } else {
            self.first_count + (error_line - n - 3)
        };
        for q in self.first_count..p {
            self.print_char(self.trick_buf[ux(q % error_line)]);
        }
        if m + n > error_line {
            self.print_str(b"...");
        }
    }

    /// §321: `push_input`: enter a new input level, save the old.
    #[inline]
    pub(crate) fn push_input(&mut self) -> Result<(), Jump> {
        if self.input_ptr > self.max_in_stack {
            return self.push_input_deeper();
        }
        self.push_input_now();
        Ok(())
    }

    /// `push_input` past the deepest level so far (§321's statistics and
    /// overflow test).
    #[cold]
    #[inline(never)]
    fn push_input_deeper(&mut self) -> Result<(), Jump> {
        self.max_in_stack = self.input_ptr;
        // (`>=`, not §321's `=`: a rebuild drops a run that overflowed and
        // runs the step again with `max_in_stack` left at the stack's size,
        // so the push at that size takes the quick path and the next one
        // comes here one past it, where tex.web, its overflow fatal, never
        // gets)
        if self.input_ptr >= ux(self.params.stack_size) {
            return self.overflow(b"input stack size", self.params.stack_size);
        }
        self.push_input_now();
        Ok(())
    }

    #[inline]
    fn push_input_now(&mut self) {
        // (the record moves to the stack; the new level starts as a copy of
        // its fields, without its list: no reference is counted)
        let c = &self.cur_input;
        let fresh = InStateRecord {
            list: None,
            state: c.state,
            index: c.index,
            start: c.start,
            loc: c.loc,
            limit: c.limit,
            name: c.name,
        };
        self.input_stack[self.input_ptr] = core::mem::replace(&mut self.cur_input, fresh); // stack the record
        if T::VALUES {
            self.input_values.levels.forget(self.input_ptr);
        }
        self.input_ptr += 1;
    }

    /// §322: `pop_input`: leave an input level, re-enter the old.
    #[inline]
    pub(crate) fn pop_input(&mut self) {
        self.input_ptr -= 1;
        self.cur_input = core::mem::take(&mut self.input_stack[self.input_ptr]);
        if T::VALUES {
            self.input_values.levels.forget(self.input_ptr);
        }
    }

    /// The input's value (DESIGN 3.5): the levels below the current one,
    /// the parameters, the buffer below the top file level's line with
    /// where that line starts, and the file levels below the top one. It
    /// makes only what changed since the last value: the levels and
    /// parameters pushed since, and the rest once per file level.
    pub(crate) fn input_value(&mut self) -> InputValue {
        let levels = self
            .input_values
            .levels
            .value(&self.input_stack[..self.input_ptr]);
        let np = ux(self.param_ptr).min(self.param_stack.len());
        let params = self.input_values.params.value(&self.param_stack[..np]);
        if self.input_values.starts.len() <= self.in_open {
            // (an engine loaded or placed without them)
            self.refill_starts();
        }
        let v = &mut self.input_values;
        let from = if self.in_open == 0 {
            0
        } else {
            v.starts[self.in_open]
        };
        let below = v
            .below
            .get_or_insert_with(|| self.buffer.prefix(from).iter().copied().collect())
            .clone();
        let n = self.in_open;
        let files = v
            .files
            .get_or_insert_with(|| {
                Arc::new(FileLevels {
                    files: self.input_file[..n].to_vec(),
                    lines: self.line_stack[..n].to_vec(),
                    grp: self.grp_stack[..n].to_vec(),
                    ifs: self.if_stack[..n].to_vec(),
                    eof: self.eof_seen[..n].to_vec(),
                    names: self.source_filename_stack[..n].to_vec(),
                    full_names: self.full_source_filename_stack[..n].to_vec(),
                })
            })
            .clone();
        InputValue {
            levels,
            depth: self.input_ptr,
            params,
            np,
            below,
            from,
            files,
        }
    }

    /// Put the input's value back (DESIGN 3.5): the levels and parameters
    /// into their stacks, the buffer's lines below the top file level's
    /// into the buffer, the file levels below the top one into their
    /// arrays, and the values shared again. `in_open` and `cur_input` are
    /// set first.
    pub(crate) fn set_input_value(&mut self, v: &InputValue) {
        self.input_values
            .levels
            .put(v.levels.as_ref(), v.depth, &mut self.input_stack);
        self.input_ptr = v.depth;
        self.input_values
            .params
            .put(v.params.as_ref(), v.np, &mut self.param_stack);
        self.param_ptr = i32::try_from(v.np).unwrap_or(0);
        for (i, &b) in v.below.iter().enumerate() {
            self.buffer[i] = b;
        }
        let f = &v.files;
        let n = f.files.len();
        for (j, x) in f.files.iter().enumerate() {
            self.input_file[j].clone_from(x);
        }
        self.line_stack[..n].copy_from_slice(&f.lines);
        self.grp_stack[..n].copy_from_slice(&f.grp);
        self.if_stack[..n].copy_from_slice(&f.ifs);
        self.eof_seen[..n].copy_from_slice(&f.eof);
        self.source_filename_stack[..n].copy_from_slice(&f.names);
        self.full_source_filename_stack[..n].copy_from_slice(&f.full_names);
        self.input_values.below = Some(Arc::clone(&v.below));
        self.input_values.files = Some(Arc::clone(&v.files));
        self.refill_starts();
    }

    /// The file levels' starts, from the stack's records.
    fn refill_starts(&mut self) {
        let v = &mut self.input_values;
        v.starts.clear();
        v.starts.resize(self.in_open + 1, 0);
        let recs = self.input_stack[..self.input_ptr].iter();
        for r in recs.chain([&self.cur_input]) {
            if r.state != TOKEN_LIST
                && r.index > 0
                && let Some(s) = v.starts.get_mut(ux(r.index))
            {
                *s = ux(r.start);
            }
        }
    }

    /// §323: start a new level of token-list input.
    #[inline]
    pub(crate) fn begin_token_list(&mut self, p: crate::tok::Tokens, t: i32) -> Result<(), Jump> {
        self.push_input()?;
        self.cur_input.state = TOKEN_LIST;
        self.cur_input.start = 0;
        let first = crate::tok::list_start(&p);
        self.cur_input.list = Some(p);
        self.cur_input.index = t;
        if t >= MACRO {
            // (the reference is not counted: see `delete_token_ref`)
            if t == MACRO {
                self.cur_input.limit = self.param_ptr; // `param_start`
            } else {
                self.cur_input.loc = first;
                if self.int_par(TRACING_MACROS_CODE) > 1 {
                    self.show_list_begun(t);
                }
            }
        } else {
            self.cur_input.loc = first;
        }
        Ok(())
    }

    /// §323's `\tracingmacros>1` display of a list begun.
    #[cold]
    #[inline(never)]
    fn show_list_begun(&mut self, t: i32) {
        self.begin_diagnostic();
        self.print_nl(b"");
        match t {
            MARK_TEXT => self.print_esc(b"mark"),
            WRITE_TEXT => self.print_esc(b"write"),
            _ => self.print_cmd_chr(ASSIGN_TOKS, t - OUTPUT_TEXT + OUTPUT_ROUTINE_LOC),
        }
        self.print_str(b"->");
        let list = self.cur_input.list.clone().unwrap_or_default();
        self.token_show(&list);
        self.end_diagnostic(false);
    }

    /// §323: `back_list(p)`.
    pub(crate) fn back_list(&mut self, p: crate::tok::Tokens) -> Result<(), Jump> {
        self.begin_token_list(p, BACKED_UP)
    }

    /// §323: `ins_list(p)`.
    pub(crate) fn ins_list(&mut self, p: crate::tok::Tokens) -> Result<(), Jump> {
        self.begin_token_list(p, INSERTED)
    }

    /// §324: leave a token-list input level.
    pub(crate) fn end_token_list(&mut self) -> Result<(), Jump> {
        self.memo.list_ended(self.input_ptr, self.token_type());
        if self.token_type() >= BACKED_UP {
            // token list to be deleted (the level's value is dropped: a list
            // no one else holds goes back to the pool)
            if self.token_type() <= INSERTED {
                if let Some(l) = self.cur_input.list.take() {
                    self.release_list(l);
                }
            } else if self.token_type() == MACRO {
                // parameters must be flushed
                while self.param_ptr > self.cur_input.limit {
                    self.param_ptr -= 1;
                    if T::VALUES {
                        self.input_values.params.forget(ux(self.param_ptr));
                    }
                    if let Some(l) = self.param_stack[ux(self.param_ptr)].take() {
                        self.release_list(l);
                    }
                }
            }
        } else if self.token_type() == U_TEMPLATE {
            if self.align_state() > 500_000 {
                self.set_align_state(0);
            } else {
                return self.fatal_error(b"(interwoven alignment preambles are not allowed)");
            }
        }
        self.pop_input();
        self.check_interrupt()
    }

    /// §325: undo one token of input.
    pub(crate) fn back_input(&mut self) -> Result<(), Jump> {
        // (with glyph origins: the token's, read before the input moves)
        let o = self.back_org();
        self.back_input_from(o)
    }

    /// [`Tex::back_input`] of a token whose origin is `o` (`srcmap.rs`).
    pub(crate) fn back_input_from(&mut self, o: partex_engine::origin::Org) -> Result<(), Jump> {
        while self.state() == TOKEN_LIST
            && self.cur_input.loc == NULL
            && self.token_type() != V_TEMPLATE
        {
            self.end_token_list()?; // conserve stack space
        }
        // (a pooled list: `tok.rs`, `pooled_list`)
        let t = self.cur_tok;
        let mut p = self.pooled_list(|b| b.push(t));
        if !o.is_none() {
            self.give_org(&mut p, &[o]);
        }
        if self.cur_tok < RIGHT_BRACE_LIMIT {
            if self.cur_tok < LEFT_BRACE_LIMIT {
                self.set_align_state(self.align_state() - 1);
            } else {
                self.set_align_state(self.align_state() + 1);
            }
        }
        self.push_input()?;
        self.cur_input.state = TOKEN_LIST;
        self.cur_input.start = 0;
        self.cur_input.list = Some(p);
        self.cur_input.index = BACKED_UP;
        self.cur_input.loc = 0; // that was `back_list(p)`, without procedure overhead
        Ok(())
    }

    /// §327: back up one token and call `error`.
    pub(crate) fn back_error(&mut self) -> Result<(), Jump> {
        self.ok_to_interrupt = false;
        self.back_input()?;
        self.ok_to_interrupt = true;
        self.error()
    }

    /// §327: back up one inserted token and call `error`.
    pub(crate) fn ins_error(&mut self) -> Result<(), Jump> {
        self.ok_to_interrupt = false;
        self.back_input()?;
        self.cur_input.index = INSERTED;
        self.ok_to_interrupt = true;
        self.error()
    }

    /// §328: start a new level of input for lines of characters.
    pub(crate) fn begin_file_reading(&mut self) -> Result<(), Jump> {
        if self.in_open == ux(self.params.max_in_open) {
            return self.overflow(b"text input levels", self.params.max_in_open);
        }
        if self.first == ux(self.params.buf_size) {
            return self.overflow(b"buffer size", self.params.buf_size);
        }
        self.in_open += 1;
        self.push_input()?;
        self.cur_input.index = i32::try_from(self.in_open).unwrap_or(0);
        self.source_filename_stack[self.in_open] = 0;
        self.full_source_filename_stack[self.in_open] = 0;
        self.line_stack[self.in_open] = self.line;
        self.eof_seen[self.in_open] = false;
        self.grp_stack[self.in_open] = self.cur_boundary();
        self.cond_read();
        self.if_stack[self.in_open] = self.cond_stack.len();
        self.cur_input.start = i32::try_from(self.first).unwrap_or(0);
        self.cur_input.state = MID_LINE;
        self.cur_input.name = 0; // `terminal_input` is now true
        if T::VALUES {
            let v = &mut self.input_values;
            if v.starts.len() <= self.in_open {
                v.starts.resize(self.in_open + 1, 0);
            }
            v.starts[self.in_open] = self.first;
            v.below = None;
            v.files = None;
        }
        Ok(())
    }

    /// §329: finish a level of line input.
    pub(crate) fn end_file_reading(&mut self) {
        self.first = ux(self.cur_input.start);
        let index = ux(self.cur_input.index);
        self.line = self.line_stack[index];
        if self.cur_input.name > 19 {
            self.end_line_of(index);
        }
        if self.cur_input.name == 18 || self.cur_input.name == 19 {
            self.pseudo_files.pop(); // e-TeX: close the pseudo file
        } else if self.cur_input.name > 17 {
            self.input_file[index] = None; // forget it
            if T::VALUES {
                self.tracker.file(index, None);
            }
            // (a file ended ends the open window at the next boundary:
            // DESIGN 4.3 item 1)
            self.window_event(crate::run::WindowEvent::FileEnded);
        }
        self.pop_input();
        self.in_open -= 1;
        if T::VALUES {
            self.input_values.below = None;
            self.input_values.files = None;
        }
    }

    /// §330: remove completed error-inserted lines from memory.
    pub(crate) fn clear_for_error_prompt(&mut self) {
        while self.state() != TOKEN_LIST
            && self.terminal_input()
            && self.input_ptr > 0
            && self.cur_input.loc > self.cur_input.limit
        {
            self.end_file_reading();
        }
        self.print_ln();
    }
}

/// A non-negative WEB integer as an index. (A negative one wraps to an
/// index past the end, failing the bounds check that follows.)
#[inline]
#[allow(
    clippy::cast_sign_loss,
    reason = "measured: the checked conversion showed in profiles"
)]
pub(crate) fn ux(i: i32) -> usize {
    i as usize
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, term_output};
    use crate::tex::Tex;

    /// Oracle: `! OK.` then `l.5 \showbox2` and 13 spaces (`error_line`
    /// is 64 in trip, but the line is short either way).
    #[test]
    fn show_context_of_a_file_line() {
        let mut t = engine();
        t.set_int_par(crate::web::END_LINE_CHAR_CODE, 13);
        let line = b"\\showbox2\r";
        t.buffer[1..=line.len()].copy_from_slice(line);
        t.cur_input.state = crate::web::MID_LINE;
        t.cur_input.name = 20;
        t.cur_input.start = 1;
        t.cur_input.limit = 10;
        t.cur_input.loc = 11;
        t.line = 5;
        let out = term_output(&mut t, Tex::show_context);
        assert_eq!(out, b"l.5 \\showbox2\n             ");
    }
}
