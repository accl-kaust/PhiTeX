//! The transcript (§54–§71): log events laid out as `print` lays them
//! out in the `.log` file (`selector=log_only`, batch mode), lines of
//! `max_print_line` (79) characters.
//!
//! The layout is the client's: it is a fold over the whole event stream
//! (`file_offset` is the only state), done once on the stream the graph
//! made.

use phitex_defuse::{ValueId, Values};

use crate::val::{int_of, list_of, str_of};

const MAX_PRINT_LINE: usize = 79;

/// TeX Live's `error_line` and `half_error_line` (texmf.cnf): a context
/// line's limits (§317).
const ERROR_LINE: usize = 79;
const HALF_ERROR_LINE: usize = 50;

/// A log event's kind: §306's runaway list, before the error it is of
/// (`crate::context`).
pub(crate) const RUNAWAY: &str = "runaway";
/// A log event's kind: an error's context and help, after it
/// (`crate::context`).
pub(crate) const CONTEXT: &str = "errctx";
/// The context alone, the hundredth error's (§82: the job ends before
/// the help).
pub(crate) const CONTEXT_ONLY: &str = "errctx0";

/// Character `c` as `print` puts it in the log (§59, [`Log::print_char`]
/// apart from the new-line character): itself, or its `^^` form.
pub(crate) fn printable(c: u32, out: &mut String) {
    if (32..127).contains(&c) || c >= 256 {
        out.push(char::from_u32(c).unwrap_or('?'));
    } else if c < 128 {
        out.push_str("^^");
        out.push(char::from_u32(if c < 64 { c + 64 } else { c - 64 }).unwrap_or('?'));
    } else {
        let hex = |d: u32| char::from_digit(d, 16).unwrap_or('0');
        out.push_str("^^");
        out.push(hex(c / 16));
        out.push(hex(c % 16));
    }
}

/// The line break of a `box` event's text: no character TeX prints.
pub(crate) const BOX_LINE: char = '\u{10FFFD}';

/// The `.log` writer: `file_offset` and the text.
#[derive(Default)]
pub struct Log {
    pub text: String,
    offset: usize,
}

impl Log {
    /// `print_ln` (§57).
    pub fn print_ln(&mut self) {
        self.text.push('\n');
        self.offset = 0;
    }

    /// `print_raw_char` (§58).
    fn raw(&mut self, c: char) {
        self.text.push(c);
        self.offset += 1;
        if self.offset == MAX_PRINT_LINE {
            self.print_ln();
        }
    }

    /// `print` of one character (§59): the newline character ends the
    /// line, others print as themselves or in `^^` form.
    pub fn print_char(&mut self, c: u32, nl: i64) {
        if i64::from(c) == nl {
            self.print_ln();
            return;
        }
        // §49: characters 32..126 print as themselves, others in `^^`
        // form
        if (32..127).contains(&c) {
            self.raw(char::from_u32(c).unwrap_or('?'));
        } else if c < 128 {
            self.raw('^');
            self.raw('^');
            self.raw(char::from_u32(if c < 64 { c + 64 } else { c - 64 }).unwrap_or('?'));
        } else if c < 256 {
            let hex = |d: u32| char::from_digit(d, 16).unwrap_or('0');
            self.raw('^');
            self.raw('^');
            self.raw(hex(c / 16));
            self.raw(hex(c % 16));
        } else {
            self.raw(char::from_u32(c).unwrap_or('?'));
        }
    }

    /// `print` of a string (§59).
    pub fn print(&mut self, s: &str, nl: i64) {
        for c in s.chars() {
            self.print_char(u32::from(c), nl);
        }
    }

    /// Characters printed as they are (`print_char` directly, §58).
    fn print_plain(&mut self, s: &str) {
        for c in s.chars() {
            self.raw(c);
        }
    }

    /// `print_nl` (§62).
    pub fn print_nl(&mut self, s: &str, nl: i64) {
        if self.offset > 0 {
            self.print_ln();
        }
        self.print(s, nl);
    }

    /// What `error` prints after its message (§90): each level of the
    /// context in the two lines of §317, the help lines, `print_ln`
    /// twice ([`crate::context`]'s text: a level's prefix, its text
    /// before the place read and after, printed already).
    fn context(&mut self, text: &str, help: bool, nl: i64) {
        use crate::context::{ERRHELP, LEVEL, NL, PART};
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            rest = &rest[c.len_utf8()..];
            let end = rest.find([LEVEL, NL, ERRHELP]).unwrap_or(rest.len());
            let (part, more) = rest.split_at(end);
            rest = more;
            if c == NL {
                self.print_nl(part, -1);
                continue;
            }
            if c == ERRHELP {
                self.print_ln();
                self.print(part, nl);
                continue;
            }
            if c != LEVEL {
                continue;
            }
            let mut ps = part.split(PART);
            let prefix = ps.next().unwrap_or_default();
            let a: Vec<char> = ps.next().unwrap_or_default().chars().collect();
            let b: Vec<char> = ps.next().unwrap_or_default().chars().collect();
            self.print_nl(prefix, -1);
            // §317, with `first_count` where the place read is and
            // `trick_count` past it (§316)
            let l = prefix.chars().count();
            let fc = a.len();
            let tally = fc + b.len();
            let trick = (fc + 1 + ERROR_LINE - HALF_ERROR_LINE).max(ERROR_LINE);
            let m = if tally < trick {
                tally - fc
            } else {
                trick - fc
            };
            let (p, n) = if l + fc <= HALF_ERROR_LINE {
                (0, l + fc)
            } else {
                self.print_plain("...");
                (l + fc - HALF_ERROR_LINE + 3, HALF_ERROR_LINE)
            };
            for &c in &a[p.min(fc)..] {
                self.raw(c);
            }
            self.print_ln();
            for _ in 0..n {
                self.raw(' ');
            }
            let q = if m + n <= ERROR_LINE {
                m
            } else {
                ERROR_LINE.saturating_sub(n + 3)
            };
            for &c in &b[..q.min(b.len())] {
                self.raw(c);
            }
            if m + n > ERROR_LINE {
                self.print_plain("...");
            }
        }
        if help {
            self.print_ln();
            self.print_ln();
        }
    }

    /// §306: `print_nl("Runaway ")`, what and `?`, `print_ln`, the list
    /// so far (printed already, `crate::context`).
    fn runaway(&mut self, text: &str, nl: i64) {
        let (what, list) = text.split_once('\n').unwrap_or((text, ""));
        self.print_nl("", nl);
        self.print_plain(what);
        self.print_ln();
        self.print_plain(list);
    }

    /// `print_err` and the message's `.` (§73): `fatal_error`'s (§93)
    /// "Emergency stop", the text its help line, in the context event
    /// that follows.
    fn error(&mut self, text: &str, nl: i64) {
        self.print_nl("! ", nl);
        if text.starts_with("*** (") {
            self.print_plain("Emergency stop");
        } else {
            self.print_plain(text);
        }
        self.raw('.');
        self.print_ln();
    }

    /// One event.
    pub fn event(&mut self, kind: &str, text: &str, nl: i64) {
        let len = text.chars().count();
        match kind {
            "msg" => {
                // §1280
                if len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if self.offset > 0 {
                    self.raw(' ');
                }
                self.print(text, nl);
            }
            "errmsg" => {
                self.print_nl("! ", nl);
                self.print(text, nl);
                self.raw('.');
                self.print_ln();
            }
            "error" => self.error(text, nl),
            RUNAWAY => self.runaway(text, nl),
            CONTEXT | CONTEXT_ONLY => self.context(text, kind == CONTEXT, nl),
            "wlog" => {
                // §1370
                self.print_nl("", nl);
                self.print(text, nl);
                self.print_ln();
            }
            "show" => {
                // §1294
                self.print_nl("> ", nl);
                self.print(text, nl);
            }
            "box" => {
                // begin_diagnostic … end_diagnostic(true) (§245) around
                // lines each begun by `print_ln` (`show_box`, §182, and
                // `show_activities`, §218)
                let mut lines = text.split(BOX_LINE);
                self.print_nl(lines.next().unwrap_or_default(), nl);
                for l in lines {
                    self.print_ln();
                    self.print(l, nl);
                }
                self.print_nl("", nl);
                self.print_ln();
            }
            // §663 and §675: `print_ln`, the report and where, `print_ln`;
            // an `\hbox`'s `short_display` and `print_ln`; then the box
            // `show_box`ed between `begin_diagnostic` and
            // `end_diagnostic(true)`
            HREPORT | VREPORT => {
                let mut parts = text.split(BOX_LINE);
                self.print_ln();
                self.print_nl(parts.next().unwrap_or_default(), nl);
                self.print_ln();
                if kind == HREPORT {
                    self.print(parts.next().unwrap_or_default(), nl);
                    self.print_ln();
                }
                if let Some(first) = parts.next() {
                    self.print(first, nl);
                }
                for l in parts {
                    self.print_ln();
                    self.print(l, nl);
                }
                self.print_nl("", nl);
                self.print_ln();
            }
            "diag" => {
                // begin_diagnostic … end_diagnostic(false) (§245)
                self.print_nl(text, nl);
                self.print_nl("", nl);
            }
            "open" => {
                // §537
                if len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if self.offset > 0 {
                    self.raw(' ');
                }
                self.raw('(');
                self.print(text, nl);
            }
            "close" => self.raw(')'),
            "ship" => {
                // §638
                if self.offset > 0 {
                    self.raw(' ');
                }
                self.print_plain(text);
            }
            "shipend" => self.raw(']'),
            "openout" => {
                // web2c's §1374: `print_nl`, the name, `print_nl("")`,
                // `print_ln`
                self.print_nl(text, nl);
                self.print_nl("", nl);
                self.print_ln();
            }
            "println" => self.print_ln(),
            "fatal" => {}
            // `pdftex_fail` (pdfTeX's `utils.c`): `println`, its line,
            // `println`; the job ends there ([`LayoutMemo::finish`]'s end)
            // (and xdvipdfmx's fatal error, on xelatex's terminal: laid
            // out the same; XeTeX's own words end the log, [`driver_end`];
            // either whole on its line, the line the front ends show)
            FAIL | DRIVER => {
                self.print_ln();
                self.text.push_str(text);
                self.print_ln();
            }
            _ => self.print(text, nl),
        }
    }
}

/// A log event's kind: pdfTeX's fatal `pdftex_fail`, its line the event's
/// text (`!pdfTeX error: pdflatex (file ptmr8r): Font ptmr8r at 600 not
/// found`), the output's (`crate::assemble`).
pub(crate) const FAIL: &str = "fail";

/// Log events' kinds: an `\hbox`'s or a `\vbox`'s report (§663,
/// §675), its parts apart by [`BOX_LINE`]: the report and where, an
/// `\hbox`'s `short_display`, the `show_box` lines.
/// In a report's text: a line's number, made when the log is laid out
/// from the event's marks (`[node, line in its text, ...]`) and the lines
/// before each node's text the client gives ([`LayoutMemo::update`]).
pub(crate) const LINE_MARK: char = '\u{10FFFC}';

pub(crate) const HREPORT: &str = "hreport";
pub(crate) const VREPORT: &str = "vreport";

/// A log event's kind: the PDF driver's fatal error (xdvipdfmx's
/// `ERROR`, `xdvipdfmx:fatal: ...`), its line the event's text: XeTeX
/// made no PDF file (`crate::xdvpdf`).
pub(crate) const DRIVER: &str = "driver";

/// What XeTeX says when its driver failed (xetex.ch, §642's change:
/// `dvi_close` gave xdvipdfmx's exit status, 1, as `pclose` does), the
/// PDF file named, `history` then `output_failure`.
pub(crate) fn driver_end(pdf: &str) -> String {
    format!("Error 256 (driver return code) generating output;\nfile {pdf} may not be valid.")
}

/// A fatal error in a log, the worst last: TeX's (§93), TeX's in a PDF
/// job (pdfTeX §1335 says no PDF file was produced), pdfTeX's
/// `pdftex_fail` (no PDF file either, said otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Fatal {
    Tex,
    TexPdf,
    Fail,
    /// XeTeX's driver failed: no PDF file, the job not ended by it.
    Driver,
}

/// A document's log kept from one build to the next ([`LayoutMemo::update`],
/// [`LayoutMemo::finish`]): its events, the log's length and column after
/// each, and the fatal errors among them (`(event, kind)`: TeX's, TeX's
/// in a PDF job, or pdfTeX's `pdftex_fail`).
#[derive(Default)]
pub(crate) struct LayoutMemo {
    name: String,
    events: Vec<ValueId>,
    after: Vec<(usize, usize)>,
    fatal: Vec<(usize, Fatal)>,
    text: String,
    offset: usize,
    /// The events with [`LINE_MARK`]s: the event, its marks, the numbers
    /// they made.
    marked: Vec<(usize, Vec<i64>, Vec<i64>)>,
}

/// The numbers `marks` name (`[node, line in its text, ...]`).
fn mark_lines(marks: &[i64], dline: &dyn Fn(u64) -> Option<i64>) -> Vec<i64> {
    marks
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[n, k]| dline(n.cast_unsigned()).unwrap_or(0) + k)
        .collect()
}

impl LayoutMemo {
    /// The log of a document: the file opened, the events (laid out
    /// again from the first that differs from the last ones'); the worst
    /// fatal error among them, if any.
    pub(crate) fn update(
        &mut self,
        v: &Values,
        name: &str,
        events: Option<ValueId>,
        dline: &dyn Fn(u64) -> Option<i64>,
    ) -> Option<Fatal> {
        let evs = events.map(|e| v.seq_to_vec(e)).unwrap_or_default();
        let mut same = if self.name == name {
            self.events
                .iter()
                .zip(&evs)
                .take_while(|(a, b)| a == b)
                .count()
        } else {
            0
        };
        // (and to the first report whose lines moved)
        if let Some(m) = self
            .marked
            .iter()
            .take_while(|m| m.0 < same)
            .find(|m| mark_lines(&m.1, dline) != m.2)
        {
            same = m.0;
        }
        self.marked.retain(|m| m.0 < same);
        let mut log = Log::default();
        if same == 0 {
            log.raw('(');
            log.print(name, -1);
            self.after.clear();
        } else {
            let (len, offset) = self.after[same - 1];
            self.text.truncate(len);
            log.text = std::mem::take(&mut self.text);
            log.offset = offset;
            self.after.truncate(same);
        }
        self.fatal.retain(|f| f.0 < same);
        let (fatal, pdf, fail, driver) = (v.str("fatal"), v.str("pdf"), v.str(FAIL), v.str(DRIVER));
        for (k, &x) in evs.iter().enumerate().skip(same) {
            let xs = list_of(v, x);
            if xs.first() == Some(&fatal) {
                self.fatal.push((
                    k,
                    if xs.get(1) == Some(&pdf) {
                        Fatal::TexPdf
                    } else {
                        Fatal::Tex
                    },
                ));
            } else if xs.first() == Some(&fail) {
                self.fatal.push((k, Fatal::Fail));
            } else if xs.first() == Some(&driver) {
                self.fatal.push((k, Fatal::Driver));
            }
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            let mut text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
            let nl = xs.get(2).map_or(-1, |&n| int_of(v, n));
            if let Some(&ms) = xs.get(3) {
                let marks: Vec<i64> = list_of(v, ms).iter().map(|&m| int_of(v, m)).collect();
                let lines = mark_lines(&marks, dline);
                for l in &lines {
                    text = text.replacen(LINE_MARK, &l.to_string(), 1);
                }
                self.marked.push((k, marks, lines));
            }
            if kind == CONTEXT || kind == CONTEXT_ONLY {
                // (the terminal's line: what the job was given, `main.tex`
                // for `./main.tex`)
                let mut cmd = String::new();
                for c in name.strip_prefix("./").unwrap_or(name).chars() {
                    printable(u32::from(c), &mut cmd);
                }
                text = text.replace(crate::context::CMDLINE, &cmd);
            }
            log.event(&kind, &text, nl);
            self.after.push((log.text.len(), log.offset));
        }
        name.clone_into(&mut self.name);
        self.events = evs;
        self.text = log.text;
        self.offset = log.offset;
        self.fatal.iter().map(|f| f.1).max()
    }

    /// The log [`LayoutMemo::update`] laid out, then the end
    /// (`final_cleanup`, §1335), then `end` on a line of its own (§642).
    pub(crate) fn finish(&mut self, end: &str) -> String {
        let mut log = Log {
            text: std::mem::take(&mut self.text),
            offset: self.offset,
        };
        let len = log.text.len();
        log.print(" )", -1);
        // (each of its lines `print_nl`'s)
        for l in end.split('\n') {
            log.print_nl(l, -1);
        }
        let out = log.text.clone();
        log.text.truncate(len);
        self.text = log.text;
        out
    }
}
