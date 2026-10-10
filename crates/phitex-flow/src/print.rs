//! TeX's printing of numbers, dimensions and glue (tex.web 65-69, 103,
//! 177-178), and of the log (tex.web 57-62, 1280, 1370), as pure functions.

use crate::val::{Glue, Tok};

/// A scaled value, rounded to five digits (tex.web 103).
pub fn scaled(out: &mut String, s: i32) {
    let mut s = i64::from(s);
    if s < 0 {
        out.push('-');
        s = -s;
    }
    let unity = 65536i64;
    out.push_str(&(s / unity).to_string());
    out.push('.');
    s = 10 * (s % unity) + 5;
    let mut delta = 10i64;
    loop {
        if delta > unity {
            s += 0o100_000 - 50000; // (round the last digit)
        }
        out.push(char::from(b'0' + (s / unity) as u8));
        s = 10 * (s % unity);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
}

/// A glue component with its order (tex.web 177).
fn glue_part(out: &mut String, d: i32, order: u8, unit: &str) {
    scaled(out, d);
    if order > 3 {
        out.push_str("foul");
    } else if order > 0 {
        out.push_str("fil");
        for _ in 1..order {
            out.push('l');
        }
    } else {
        out.push_str(unit);
    }
}

/// A glue specification (tex.web 178).
pub fn spec(g: &Glue, unit: &str) -> String {
    let mut out = String::new();
    scaled(&mut out, g.width);
    out.push_str(unit);
    if g.stretch != 0 {
        out.push_str(" plus ");
        glue_part(&mut out, g.stretch, g.stretch_order, unit);
    }
    if g.shrink != 0 {
        out.push_str(" minus ");
        glue_part(&mut out, g.shrink, g.shrink_order, unit);
    }
    out
}

/// Lowercase roman numerals; nothing for a nonpositive number (tex.web 69).
pub fn roman(out: &mut String, mut n: i32) {
    const S: &[u8] = b"m2d5c2l5x2v5i";
    let mut j = 0;
    let mut v = 1000;
    loop {
        while n >= v {
            out.push(char::from(S[j]));
            n -= v;
        }
        if n <= 0 {
            return;
        }
        let mut k = j + 2;
        let mut u = v / i32::from(S[k - 1] - b'0');
        if S[k - 1] == b'2' {
            k += 2;
            u /= i32::from(S[k - 1] - b'0');
        }
        if n + u >= v {
            out.push(char::from(S[k]));
            n += u;
        } else {
            j += 2;
            v /= i32::from(S[j - 1] - b'0');
        }
    }
}

/// `"` and uppercase hex digits (tex.web 67).
pub fn hex(out: &mut String, n: i32) {
    use std::fmt::Write as _;
    out.push('"');
    let _ = write!(out, "{n:X}");
}

/// A string as the tokens `\the` and friends make of it: characters of
/// category 12, spaces of category 10 (tex.web 464).
pub fn str_toks(s: &str) -> Vec<Tok> {
    // (as many tokens as characters, at most one a byte: one allocation)
    let mut v = Vec::with_capacity(s.len());
    v.extend(s.chars().map(|c| if c == ' ' { Tok::Char(10, ' ') } else { Tok::Char(12, c) }));
    v
}

/// The sizes TeX was compiled with that shape its output (tex.web 11):
/// the lengths of the lines it prints (`max_print_line`) and of an error's
/// context lines (`error_line`, `half_error_line`), and the DVI buffer
/// whose flushes TRIP's DVI file shows (`dvi_buf_size`; the memory
/// bound is `Options::main_memory`). Fixed
/// for a build's life: the program's constants, not the document's
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub error_line: usize,
    pub half_error_line: usize,
    pub max_print_line: usize,
    /// The DVI writer's buffer (`dvi_buf_size`): where `w`/`x`/`y`/`z`
    /// movements stop being reused (`dvi_gone`).
    pub dvi_buf_size: i32,
}

impl Limits {
    /// TeX Live's `tex` (texmf.cnf).
    pub const TEX_LIVE: Limits =
        Limits { error_line: 79, half_error_line: 50, max_print_line: 79, dvi_buf_size: 16384 };
    /// Knuth's TRIP test (tripman.tex, Appendix A, step 2).
    pub const TRIP: Limits =
        Limits { error_line: 64, half_error_line: 32, max_print_line: 72, dvi_buf_size: 800 };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::TEX_LIVE
    }
}

/// The longest line TeX writes to its log (`max_print_line`).
pub const MAX_PRINT_LINE: usize = 79;

/// tex.web 54's `selector`: where printing goes.
pub const NO_PRINT: u8 = 16;
pub const TERM_ONLY: u8 = 17;
pub const LOG_ONLY: u8 = 18;
pub const TERM_AND_LOG: u8 = 19;

/// The marker in a raw entry that sets the selector to `sel`
/// (`crate::show::SEL`).
pub fn sel(sel: u8) -> char {
    char::from_u32(u32::from(crate::show::SEL) + u32::from(sel - NO_PRINT)).unwrap_or(crate::show::SEL)
}

/// The log TeX writes for these entries (`\message` texts, and
/// `\immediate\write` lines ending in `\n`), in batch mode, after `start`
/// on the current line: lines broken at 79 characters, a `\message` after
/// a space or on a line of its own if it is long (tex.web 1280), a written
/// line on lines of its own (tex.web 1370). Characters below 32 and 127
/// print as `^^` codes. An entry marked [`ENDJOB`] comes after `end` (the
/// main file's ` )`): what `\end` writes last (tex.web 1335).
pub fn log(start: &str, entries: &[String], end: &str) -> String {
    let lim = Limits::TEX_LIVE;
    let mut p = Printer::new(&lim);
    p.print(start);
    p.job(entries, end, "");
    p.log
}

/// What one job of a session printed: its transcript file and what its
/// terminal showed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobLog {
    pub log: String,
    pub term: String,
}

/// The logs of a session of jobs (`crate::graph::JOB` begins each), laid
/// out with `lim`: each job's entries from its job-start entry
/// ([`crate::show::JOB`]) to the next, the main file's ` )`, the lines
/// `\end` wrote, then `foots[k]` (raw: what `close_files_and_terminate`
/// writes, tex.web 1333). Entries before the first job-start entry are
/// the first job's.
pub fn session(entries: &[String], lim: &Limits, foots: &[String]) -> Vec<JobLog> {
    let mut jobs: Vec<Vec<String>> = vec![Vec::new()];
    for e in entries {
        if e.starts_with(crate::show::JOB) {
            if !jobs.last().is_some_and(Vec::is_empty) {
                jobs.push(Vec::new());
            }
            jobs.last_mut().expect("a job").push(e[crate::show::JOB.len_utf8()..].to_string());
        } else {
            jobs.last_mut().expect("a job").push(e.clone());
        }
    }
    jobs.iter()
        .enumerate()
        .map(|(k, es)| {
            let mut p = Printer::new(lim);
            p.job(es, " )", foots.get(k).map_or("", String::as_str));
            JobLog { log: p.log, term: p.term }
        })
        .collect()
}

/// Marks a log entry that goes after the main file's ` )`
/// (`final_cleanup`'s lines, tex.web 1335).
pub const ENDJOB: char = '\u{E006}';

/// The printing of tex.web 54–62 into the two files a run writes, the
/// transcript and the terminal, each with its own line length count.
struct Printer<'a> {
    lim: &'a Limits,
    log: String,
    term: String,
    file_offset: usize,
    term_offset: usize,
    selector: u8,
    /// The selectors saved by `SEL_PUSH`.
    saved: Vec<u8>,
}

impl<'a> Printer<'a> {
    fn new(lim: &'a Limits) -> Printer<'a> {
        Printer {
            lim,
            log: String::new(),
            term: String::new(),
            file_offset: 0,
            term_offset: 0,
            selector: LOG_ONLY,
            saved: Vec::new(),
        }
    }

    /// One job's entries, `end` after them (with the selector the first
    /// late entry asks for), the late entries, then `foot`.
    fn job(&mut self, entries: &[String], end: &str, foot: &str) {
        let (late, now): (Vec<&String>, Vec<&String>) = entries.iter().partition(|e| e.starts_with(ENDJOB));
        for e in now {
            // (the hundredth error ends the log there, without `end`: §82)
            if self.entry(e) {
                self.foot(foot);
                return;
            }
        }
        if let Some(first) = late.first()
            && let Some(c) = first[ENDJOB.len_utf8()..].strip_prefix(crate::show::RAW).and_then(|r| r.chars().next())
            && let Some(s) = sel_of(c)
        {
            self.selector = s;
        }
        self.print(end);
        for e in late {
            if self.entry(&e[ENDJOB.len_utf8()..]) {
                break;
            }
        }
        self.foot(foot);
    }

    fn foot(&mut self, foot: &str) {
        if !foot.is_empty() {
            let mut f = String::from(crate::show::RAW);
            f.push_str(foot.strip_prefix(crate::show::RAW).unwrap_or(foot));
            self.entry(&f);
        }
    }

    /// One entry of the log; true if it ends the log (`show::HALT`).
    fn entry(&mut self, e: &str) -> bool {
        use crate::show::{CTX, HALT, LN, MSG, MSG_END, NL, OPEN, RAW, SEL_POP, SEL_PUSH, SHIP, SP, WLOG};
        let p = self;
        // (each entry says where it prints; none: the log only)
        p.selector = LOG_ONLY;
        if let Some(raw) = e.strip_prefix(RAW) {
            // (print operations: `print_ln`, `print_nl`, a space if the
            // line is not empty, characters)
            // (an error's context line: its label's length, what was read,
            // what was not)
            let mut ctx: Option<(usize, String, Option<String>)> = None;
            // (a message, a file's name, a page's number: what is printed
            // after the rule that decides how its line begins)
            let mut msg: Option<(char, String)> = None;
            let mut wlog = false;
            for c in raw.chars() {
                if c == CTX {
                    ctx = match ctx.take() {
                        None => Some((p.offset(), String::new(), None)),
                        Some((l, r, None)) => Some((l, r, Some(String::new()))),
                        Some((l, r, Some(u))) => {
                            p.context(l, &r, &u);
                            None
                        }
                    };
                    continue;
                }
                if let Some((_, r, u)) = &mut ctx {
                    u.as_mut().unwrap_or(r).push(c);
                    continue;
                }
                if c == MSG_END {
                    if let Some((kind, s)) = msg.take() {
                        p.lead(kind, &s);
                    }
                    continue;
                }
                if let Some((_, s)) = &mut msg {
                    s.push(c);
                    continue;
                }
                if wlog {
                    // (tex.web 1334's `wlog`: straight into the transcript)
                    if c == WLOG {
                        wlog = false;
                    } else if c == LN {
                        p.log.push('\n');
                    } else {
                        p.log.push(c);
                    }
                    continue;
                }
                if let Some(s) = sel_of(c) {
                    p.selector = s;
                    continue;
                }
                match c {
                    HALT => return true,
                    LN => p.ln(),
                    NL => p.nl(),
                    SP => {
                        if p.offset() > 0 {
                            p.char(' ');
                        }
                    }
                    SEL_PUSH => p.saved.push(p.selector),
                    SEL_POP => {
                        if let Some(s) = p.saved.pop() {
                            p.selector = s;
                        }
                    }
                    MSG | OPEN | SHIP => msg = Some((c, String::new())),
                    WLOG => wlog = true,
                    _ => p.print_one(c),
                }
            }
        } else if let Some(line) = e.strip_suffix('\n') {
            if p.offset() > 0 {
                p.ln();
            }
            p.print(line);
            p.ln();
        } else {
            p.lead(MSG, e);
        }
        false
    }

    /// The offset of the line being printed (TeX's `file_offset`, or
    /// `term_offset` when only the terminal is printed to).
    fn offset(&self) -> usize {
        if self.selector == TERM_ONLY { self.term_offset } else { self.file_offset }
    }

    /// A message (tex.web 1280), a file's name (537) or a page's number
    /// (638): how its line begins — on a line of its own if it would not
    /// fit on the terminal's, else after a space if either line is begun —
    /// then it.
    fn lead(&mut self, kind: char, s: &str) {
        use crate::show::{MSG, OPEN};
        let len = s.chars().count();
        let max = self.lim.max_print_line;
        let long = match kind {
            MSG | OPEN => self.term_offset + len > max.saturating_sub(2),
            _ => self.term_offset > max.saturating_sub(9),
        };
        if long {
            self.ln();
        } else if self.term_offset > 0 || self.file_offset > 0 {
            self.char(' ');
        }
        match kind {
            MSG => {}
            OPEN => self.char('('),
            _ => self.char('['),
        }
        for c in s.chars() {
            self.print_one(c);
        }
    }

    /// tex.web 57: `print_ln`.
    fn ln(&mut self) {
        match self.selector {
            TERM_AND_LOG => {
                self.term.push('\n');
                self.log.push('\n');
                self.term_offset = 0;
                self.file_offset = 0;
            }
            LOG_ONLY => {
                self.log.push('\n');
                self.file_offset = 0;
            }
            TERM_ONLY => {
                self.term.push('\n');
                self.term_offset = 0;
            }
            _ => {}
        }
    }

    /// tex.web 62: `print_nl`'s new line, if a line it prints to is begun.
    fn nl(&mut self) {
        if (self.term_offset > 0 && self.selector % 2 == 1) || (self.file_offset > 0 && self.selector >= LOG_ONLY) {
            self.ln();
        }
    }

    /// tex.web 58: `print_char`.
    fn char(&mut self, c: char) {
        let max = self.lim.max_print_line;
        let to_term = |p: &mut Self| {
            p.term.push(c);
            p.term_offset += 1;
            if p.term_offset == max {
                p.term.push('\n');
                p.term_offset = 0;
            }
        };
        match self.selector {
            TERM_AND_LOG => {
                to_term(self);
                self.log.push(c);
                self.file_offset += 1;
                if self.file_offset == max {
                    self.log.push('\n');
                    self.file_offset = 0;
                }
            }
            LOG_ONLY => {
                self.log.push(c);
                self.file_offset += 1;
                if self.file_offset == max {
                    self.log.push('\n');
                    self.file_offset = 0;
                }
            }
            TERM_ONLY => to_term(self),
            _ => {}
        }
    }

    /// tex.web 59: a character as `print` shows it (tex.web 49: `^^`
    /// codes for the unprintable ones).
    fn print_one(&mut self, c: char) {
        let mut b = [0u8; 4];
        for x in printable_char(c, &mut b).chars() {
            self.char(x);
        }
    }

    fn print(&mut self, s: &str) {
        for c in s.chars() {
            self.print_one(c);
        }
    }

    /// tex.web 316–317: a context line after its label (`l` characters):
    /// what was read on the first line (its last `half_error_line`
    /// characters), what was not on the second, under it.
    fn context(&mut self, l: usize, read: &str, unread: &str) {
        let error_line = self.lim.error_line;
        let half_error_line = self.lim.half_error_line;
        let printed = |s: &str, out: &mut Vec<char>| {
            let mut b = [0u8; 4];
            for c in s.chars() {
                out.extend(printable_char(c, &mut b).chars());
            }
        };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        printed(read, &mut a);
        printed(unread, &mut b);
        // (the pseudoprinting: `trick_buf` keeps what is printed while
        // `tally<trick_count`, at `tally mod error_line`)
        let mut buf = vec![' '; error_line];
        let first_count = a.len();
        let trick_count = (first_count + 1 + error_line - half_error_line).max(error_line);
        for (tally, &c) in a.iter().chain(&b).enumerate() {
            if tally < trick_count {
                buf[tally % error_line] = c;
            }
        }
        let tally = a.len() + b.len();
        let m = if tally < trick_count { tally - first_count } else { trick_count - first_count };
        let (p, n) = if l + first_count <= half_error_line {
            (0, l + first_count)
        } else {
            self.print("...");
            (l + first_count - half_error_line + 3, half_error_line)
        };
        for q in p..first_count {
            self.char(buf[q % error_line]);
        }
        self.ln();
        for _ in 0..n {
            self.char(' ');
        }
        let p = if m + n <= error_line { first_count + m } else { first_count + (error_line - n - 3) };
        for q in first_count..p {
            self.char(buf[q % error_line]);
        }
        if m + n > error_line {
            self.print("...");
        }
    }
}

/// The selector a marker sets (`sel`).
fn sel_of(c: char) -> Option<u8> {
    let k = u32::from(c).checked_sub(u32::from(crate::show::SEL))?;
    (k < 4).then(|| NO_PRINT + k as u8)
}

/// tex.web 49: how character `c` prints — itself, or `^^` and the
/// character 64 away (below 64, and 127), or `^^` and two lowercase hex
/// digits (128–255).
pub fn printable_char(c: char, buf: &mut [u8; 4]) -> &str {
    let k = u32::from(c);
    if k < 32 || k == 127 {
        buf[0] = b'^';
        buf[1] = b'^';
        buf[2] = if k < 64 { (k + 64) as u8 } else { (k - 64) as u8 };
        std::str::from_utf8(&buf[..3]).unwrap_or("?")
    } else if (128..256).contains(&k) {
        let hex = |d: u32| if d < 10 { b'0' + d as u8 } else { b'a' + (d - 10) as u8 };
        buf[0] = b'^';
        buf[1] = b'^';
        buf[2] = hex(k / 16);
        buf[3] = hex(k % 16);
        std::str::from_utf8(&buf[..4]).unwrap_or("?")
    } else {
        c.encode_utf8(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        let s = |v| {
            let mut o = String::new();
            scaled(&mut o, v);
            o
        };
        assert_eq!(s(65536), "1.0");
        assert_eq!(s(-32768), "-0.5");
        assert_eq!(s(1), "0.00002");
        assert_eq!(s(0x3fff_ffff), "16383.99998");
        let r = |v| {
            let mut o = String::new();
            roman(&mut o, v);
            o
        };
        assert_eq!(r(1984), "mcmlxxxiv");
        assert_eq!(r(0), "");
        assert_eq!(
            spec(&Glue { width: 65536, stretch: 65536, stretch_order: 2, ..Glue::default() }, "pt"),
            "1.0pt plus 1.0fill"
        );
    }
}
