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

/// A log event's kind's mark: the event was made while
/// `\tracingonline` was positive, so what TeX prints of it between
/// `begin_diagnostic` and `end_diagnostic` is on the terminal too
/// (`Run::online_kind`). The log is laid out as without it.
pub(crate) const ONLINE: char = '+';

/// The terminal beside the log ([`Log::with_terminal`]): its text,
/// `term_offset`, and whether what is printed now goes to it
/// (`selector` `term_and_log`, else `log_only`).
#[derive(Default)]
struct Terminal {
    text: String,
    offset: usize,
    on: bool,
}

/// The `.log` writer: `file_offset` and the text; with a terminal
/// beside it, `term_offset` and the terminal's text, as TeX prints to
/// both (§57–§62: one `print_ln` for both, each wrapped at its own
/// offset, `print_nl` when either has a partial line).
#[derive(Default)]
pub struct Log {
    pub text: String,
    offset: usize,
    term: Option<Terminal>,
}

impl Log {
    /// `f` with `selector` `term_only`: the terminal alone, its
    /// `print_nl` by its own column; the log untouched.
    fn term_only(&mut self, f: impl FnOnce(&mut Log)) {
        let Some(term) = self.term.take() else {
            return;
        };
        let mut t = Log {
            text: String::new(),
            offset: 0,
            term: Some(Terminal { on: true, ..term }),
        };
        f(&mut t);
        self.term = t.term;
    }

    /// A log with the terminal beside it, both at a line's start.
    fn with_terminal() -> Log {
        Log {
            term: Some(Terminal {
                on: true,
                ..Terminal::default()
            }),
            ..Log::default()
        }
    }

    /// `selector` for what follows: the terminal too (`term_and_log`),
    /// or the log alone (`log_only`).
    fn online(&mut self, on: bool) {
        if let Some(t) = &mut self.term {
            t.on = on;
        }
    }

    /// `print_ln` (§57).
    pub fn print_ln(&mut self) {
        self.text.push('\n');
        self.offset = 0;
        if let Some(t) = self.term.as_mut().filter(|t| t.on) {
            t.text.push('\n');
            t.offset = 0;
        }
    }

    /// `print_raw_char` (§58): each stream wrapped at its own offset.
    fn raw(&mut self, c: char) {
        self.text.push(c);
        self.offset += 1;
        if self.offset == MAX_PRINT_LINE {
            self.text.push('\n');
            self.offset = 0;
        }
        if let Some(t) = self.term.as_mut().filter(|t| t.on) {
            t.text.push(c);
            t.offset += 1;
            if t.offset == MAX_PRINT_LINE {
                t.text.push('\n');
                t.offset = 0;
            }
        }
    }

    /// Text put on both streams as it is (no wrapping, as `tex_printf`
    /// writes it), the offsets left alone.
    fn push_str(&mut self, s: &str) {
        self.text.push_str(s);
        if let Some(t) = self.term.as_mut().filter(|t| t.on) {
            t.text.push_str(s);
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
        let term = self.term.as_ref().is_some_and(|t| t.on && t.offset > 0);
        if self.offset > 0 || term {
            self.print_ln();
        }
        self.print(s, nl);
    }

    /// What `error` prints after its message (§90): each level of the
    /// context in the two lines of §317, the help lines, `print_ln`
    /// twice ([`crate::context`]'s text: a level's prefix, its text
    /// before the place read and after, printed already).
    fn context(&mut self, text: &str, help: bool) {
        use crate::context::{LEVEL, NL, PART};
        // (§90: the help lines after the last level, and the `print_ln`
        // after them, the log's alone: `decr(selector)` with the
        // terminal on)
        let helps_at = text.rfind(LEVEL).map_or(0, |k| k + LEVEL.len_utf8());
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            let at = text.len() - rest.len();
            rest = &rest[c.len_utf8()..];
            let end = rest.find([LEVEL, NL]).unwrap_or(rest.len());
            let (part, more) = rest.split_at(end);
            rest = more;
            if c == NL {
                let help_line = help && at >= helps_at;
                if help_line {
                    self.online(false);
                }
                self.print_nl(part, -1);
                self.online(true);
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
            self.online(false);
            self.print_ln();
            self.online(true);
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

    /// One event: on the terminal too unless it is the log's alone
    /// (`\wlog`; a diagnostic, a shown box, web2c's `\openout` line
    /// without [`ONLINE`]; a box report's display, its report and short
    /// display on both).
    pub fn event(&mut self, kind: &str, text: &str, nl: i64) {
        let (kind, online) = match kind.strip_suffix(ONLINE) {
            Some(k) => (k, true),
            None => (kind, false),
        };
        let log_only = match kind {
            "wlog" => true,
            "diag" | "box" | "openout" => !online,
            _ => false,
        };
        if log_only {
            self.online(false);
        }
        self.event_on(kind, text, nl, online);
        self.online(true);
    }

    fn event_on(&mut self, kind: &str, text: &str, nl: i64, online: bool) {
        let len = text.chars().count();
        // (`term_offset`: 0 without a terminal, as in batch mode, where
        // nothing is printed on it)
        let term = self.term.as_ref().map_or(0, |t| t.offset);
        let partial = self.offset > 0 || term > 0;
        match kind {
            "msg" => {
                // §1280
                if term + len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if partial {
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
            CONTEXT | CONTEXT_ONLY => self.context(text, kind == CONTEXT),
            "wlog" | "write" => {
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
                // (`begin_diagnostic`: the box shown, and the end, on the
                // terminal only with \tracingonline positive)
                if !online {
                    self.online(false);
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
                if term + len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if partial {
                    self.raw(' ');
                }
                self.raw('(');
                self.print(text, nl);
            }
            "close" => self.raw(')'),
            "ship" => {
                // §638
                if term > MAX_PRINT_LINE - 9 {
                    self.print_ln();
                } else if partial {
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
                self.push_str(text);
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

/// Event `x` of a log as it is laid out: its kind, its text (each
/// [`LINE_MARK`] its line's number, the context's command line the main
/// file's name), `\newlinechar`, and its marks with the lines they made.
#[allow(clippy::type_complexity, reason = "one tuple, two callers")]
fn event_parts(
    v: &Values,
    x: ValueId,
    name: &str,
    dline: &dyn Fn(u64) -> Option<i64>,
) -> (String, String, i64, Option<(Vec<i64>, Vec<i64>)>) {
    let xs = list_of(v, x);
    let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
    let mut text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
    let nl = xs.get(2).map_or(-1, |&n| int_of(v, n));
    let mut marked = None;
    if let Some(&ms) = xs.get(3) {
        let marks: Vec<i64> = list_of(v, ms).iter().map(|&m| int_of(v, m)).collect();
        let lines = mark_lines(&marks, dline);
        for l in &lines {
            text = text.replacen(LINE_MARK, &l.to_string(), 1);
        }
        marked = Some((marks, lines));
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
    (kind, text, nl, marked)
}

/// What the terminal says besides the events ([`terminal`]).
pub(crate) struct TermParts<'a> {
    /// The banner and what is said before the log opens.
    pub head: &'a str,
    /// The log's end: §642's line, or a fatal error's.
    pub end: &'a str,
    /// The log's name, as `Transcript written on` says it.
    pub log_name: &'a str,
    /// What pdfTeX reads as fonts are first used, each by the page
    /// (0 the first) whose shipping reads it: the font map, an encoding
    /// file (`{PATH}`, said before the page's `]`).
    pub reads: Vec<(i64, String)>,
    /// The font files embedded, as pdfTeX says them when it writes them
    /// at the end (`<PATH>`, a whole one's `<<PATH>>`), one after another.
    pub fonts: String,
}

/// A job's terminal as TeX prints it with the terminal on
/// (`nonstop_mode`, `scroll_mode`): the head, the events laid out on the
/// log and the terminal at once (a line's start on either depends on
/// both columns: `print_nl`, `\message`, a file's open, a page shipped;
/// the log's alone not on the terminal), the font map and encoding files
/// in the page where a font first needs them, ` )`, `(see the transcript file for additional
/// information)` when the job's history is not spotless (§1335: an error,
/// or a diagnostic the terminal did not show), the font files, the end,
/// then `Transcript written on LOG.` (§1333).
pub(crate) fn terminal(
    v: &Values,
    name: &str,
    events: Option<ValueId>,
    dline: &dyn Fn(u64) -> Option<i64>,
    t: &TermParts<'_>,
) -> String {
    let mut log = Log::with_terminal();
    log.raw('(');
    log.print(name, -1);
    let mut page = 0i64;
    let mut spotless = true;
    for x in events.map(|e| v.seq_to_vec(e)).unwrap_or_default() {
        let (kind, text, nl, _) = event_parts(v, x, name, dline);
        let online = kind.ends_with(ONLINE);
        let bare = kind.trim_end_matches(ONLINE);
        // (§90's `error` and §245's `begin_diagnostic` with the terminal
        // left out: the history no longer spotless)
        if bare == CONTEXT
            || bare == CONTEXT_ONLY
            || (!online && matches!(bare, "diag" | "box"))
            || (!online && (bare == HREPORT || bare == VREPORT))
        {
            spotless = false;
        }
        if bare == "shipend" {
            for (_, r) in t.reads.iter().filter(|(p, _)| *p == page) {
                log.print(&format!("{{{r}}}"), -1);
            }
            page += 1;
        }
        log.event(&kind, &text, nl);
    }
    log.print(" )", -1);
    if !spotless {
        // (`selector:=term_only`: `print_nl` by the terminal's column)
        log.term_only(|l| l.print_nl("(see the transcript file for additional information)", -1));
    }
    log.print(&t.fonts, -1);
    for l in t.end.split('\n') {
        log.print_nl(l, -1);
    }
    // (§1333: the log closed; then on the terminal alone `print_nl`, the
    // transcript's name, `print_ln`)
    log.term_only(|l| {
        l.print_nl(&format!("Transcript written on {}.", t.log_name), -1);
        l.print_ln();
    });
    let term = log.term.take().map(|t| t.text).unwrap_or_default();
    format!("{}{term}", t.head)
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
            let (kind, text, nl, marked) = event_parts(v, x, name, dline);
            if let Some((marks, lines)) = marked {
                self.marked.push((k, marks, lines));
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
            term: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn term(log: &Log) -> String {
        log.term
            .as_ref()
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    /// The log's alone stay off the terminal: `\wlog`, a diagnostic, an
    /// `\openout` line; with [`ONLINE`] a diagnostic is on both. Each
    /// stream wraps at its own column, and `print_nl` breaks both when
    /// either has a partial line.
    #[test]
    fn terminal_beside_the_log() {
        let mut log = Log::with_terminal();
        log.event("msg", "Hello", -1);
        log.event("wlog", "File: x.sty", -1);
        log.event("diag", "{vertical mode: \\par}", -1);
        log.event("openout", "\\openout1 = `doc.aux'.", -1);
        log.event(&format!("diag{ONLINE}"), "{horizontal mode}", -1);
        assert_eq!(
            log.text,
            "Hello\nFile: x.sty\n{vertical mode: \\par}\n\\openout1 = `doc.aux'.\n\n\n{horizontal mode}\n"
        );
        assert_eq!(term(&log), "Hello\n{horizontal mode}\n");
        let mut log = Log::with_terminal();
        log.event("msg", &"x".repeat(85), -1);
        assert_eq!(
            term(&log),
            format!("\n{}\n{}", "x".repeat(79), "x".repeat(6))
        );
    }
}
