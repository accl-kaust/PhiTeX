//! Structured diagnostics: TeX's errors as data, for humans and tools.
//!
//! TeX's own report (`! Undefined control sequence.` plus the two-line
//! context in the transcript) stays byte-identical to the oracle. Alongside
//! it, every `error` builds a [`Diagnostic`] and hands it to
//! [`Host::diagnostic`]: a stable code, the message and help, the full input
//! stack as frames (file lines with columns, macro expansions with the text
//! read so far and still to come), and suggestions. Collecting it only
//! prints into a private buffer, so the transcript cannot change.
//!
//! [`Diagnostic::render`] is the canonical text form (rustc-style); the
//! CLI adds color on top of it.
//!
//! A host that asks for them ([`Host::notes`]) also gets warnings (TeX's
//! overfull and underfull box reports) and notes (the text of each
//! `\message` and of each `\write` to the terminal) the same way, with
//! only the file levels of the input stack as frames.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::host::Host;
use crate::input::{InStateRecord, ux};
use crate::mem::NULL;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::*;

/// How bad it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// TeX recovers and continues (`error`).
    Error,
    /// TeX stops (`fatal_error`, `overflow`, `confusion`, 100 errors).
    Fatal,
    /// A report TeX prints without stopping: an overfull or underfull box.
    Warning,
    /// Text a document shows on the terminal: `\message` (code `message`)
    /// and `\write` to the terminal (code `write`).
    Note,
}

/// What TeX's report of an overfull, underfull, tight or loose box says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxWarning {
    /// The lines the report names (`in paragraph at lines 3--5`; one line
    /// for `detected at line 7` and while `\output` is active).
    pub lines: (i32, i32),
    /// For an overfull box, how much too wide or too high (in sp); for the
    /// others, the badness.
    pub amount: i32,
    /// An hbox's contents as the report shows them (`short_display`).
    pub excerpt: Vec<u8>,
}

/// One level of TeX's input stack, innermost first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    /// What was read of this level so far (`show_context`'s first line).
    pub before: Vec<u8>,
    /// What is still to be read (its second line).
    pub after: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// A line of a source file. `column` is 0-based, in bytes of `before`.
    File { name: Vec<u8>, line: i32 },
    /// The terminal (`<*>` or an `<insert>`ed line).
    Terminal { inserted: bool },
    /// `\read` stream `n`, or `\read16` from the terminal (`n = 16`).
    Read(i32),
    /// The expansion of a macro.
    Macro { name: Vec<u8> },
    /// Another token list: `argument`, `template`, `to be read again`,
    /// `everypar`, `output`, ... as `show_context` names them.
    TokenList(&'static str),
}

/// An error as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable kebab-case identifier, e.g. `undefined-control-sequence`.
    pub code: &'static str,
    /// The message as TeX prints it after `! `, without the final period.
    pub message: Vec<u8>,
    /// TeX's help lines (or the `\errhelp` text).
    pub help: Vec<Vec<u8>>,
    pub frames: Vec<Frame>,
    /// Likely fixes, e.g. similar control sequence names.
    pub suggestions: Vec<Vec<u8>>,
    /// For a box warning, what its report says.
    pub boxed: Option<BoxWarning>,
}

/// The state of diagnostic collection inside the engine.
#[derive(Clone, Default, Hash)]
pub(crate) struct DiagState {
    /// Copy what `print_char` prints into `buf`.
    pub(crate) capturing: bool,
    pub(crate) buf: Vec<u8>,
    /// The message of the pending error.
    pub(crate) message: Vec<u8>,
}

partex_engine::persist_struct!(DiagState {
    capturing,
    buf,
    message
});

/// Map TeX's message to a stable code. Messages that interpolate text are
/// matched by prefix.
#[must_use]
pub fn code_for(message: &[u8]) -> &'static str {
    const CODES: &[(&[u8], &str)] = &[
        (b"Undefined control sequence", "undefined-control-sequence"),
        (b"Missing $ inserted", "missing-math-shift"),
        (b"Display math should end with $$", "display-math-end"),
        (b"Missing { inserted", "missing-left-brace"),
        (b"Missing } inserted", "missing-right-brace"),
        (b"Extra }, or forgotten ", "extra-right-brace"),
        (b"Too many }'s", "too-many-right-braces"),
        (b"Missing number", "missing-number"),
        (b"Illegal unit of measure", "illegal-unit"),
        (
            b"Missing control sequence inserted",
            "missing-control-sequence",
        ),
        (b"Missing \\endcsname inserted", "missing-endcsname"),
        (b"Missing = inserted", "missing-equals"),
        (b"Use of ", "macro-pattern-mismatch"),
        (b"Argument of ", "extra-brace-in-argument"),
        (b"Paragraph ended before ", "paragraph-ended-in-argument"),
        (b"File ended while scanning", "eof-while-scanning"),
        (b"Forbidden control sequence found", "outer-in-scan"),
        (b"Incomplete \\if", "incomplete-conditional"),
        (b"Extra \\", "extra-command"),
        (b"You can't use `", "not-allowed-in-mode"),
        (b"Misplaced ", "misplaced"),
        (b"Arithmetic overflow", "arithmetic-overflow"),
        (b"Dimension too large", "dimension-too-large"),
        (b"Number too big", "number-too-big"),
        (b"Bad ", "bad-value"),
        (b"Font ", "font"),
        (b"Missing font identifier", "missing-font"),
        (b"Improper ", "improper"),
        (b"Incompatible magnification", "magnification"),
        (b"Illegal magnification", "magnification"),
        (b"TeX capacity exceeded", "capacity-exceeded"),
        (b"This can't happen", "internal-error"),
        (b"I can't go on meeting you like this", "internal-error"),
        (b"Emergency stop", "emergency-stop"),
        (b"Interruption", "interrupted"),
        (b"I can't find file", "file-not-found"),
        (b"I can't write on file", "file-not-writable"),
        (b"Infinite glue shrinkage", "infinite-shrinkage"),
        (b"Unbalanced ", "unbalanced"),
        (b"Output loop", "output-loop"),
        (b"Undefined ", "undefined"),
    ];
    CODES
        .iter()
        .find(|(p, _)| message.starts_with(p))
        .map_or("tex-error", |&(_, c)| c)
}

/// Selector value that prints into the diagnostic buffer. Above every
/// selector of tex.web (`new_string = 21`), so `print` treats it like the
/// internal selectors: characters go out raw.
pub(crate) const DIAG: i32 = NEW_STRING + 1;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Called by `print_err` after the `! ` prefix.
    pub(crate) fn diag_begin_message(&mut self) {
        if let Some(d) = &mut self.diag {
            d.message.clear();
            d.buf.clear();
            d.capturing = true;
        }
    }

    /// Run `f` with output going to a fresh buffer; return what it printed.
    pub(crate) fn diag_print(&mut self, f: impl FnOnce(&mut Self)) -> Vec<u8> {
        let (old_selector, old_tally) = (self.selector(), self.tally);
        let (old_first, old_trick) = (self.first_count, self.trick_count);
        self.set_selector(DIAG);
        if let Some(d) = &mut self.diag {
            d.buf.clear();
        }
        f(self);
        self.set_selector(old_selector);
        self.tally = old_tally;
        self.first_count = old_first;
        self.trick_count = old_trick;
        self.diag
            .as_mut()
            .map(|d| core::mem::take(&mut d.buf))
            .unwrap_or_default()
    }

    /// Called at the start of `error` (and the fatal paths): build the
    /// diagnostic and hand it to the host.
    pub(crate) fn diag_report(&mut self, severity: Severity) {
        let Some(d) = &mut self.diag else { return };
        if !d.capturing && d.buf.is_empty() {
            return; // an `error` without `print_err` (e.g. a nested call)
        }
        d.capturing = false;
        let mut message = core::mem::take(&mut d.buf);
        while message.last() == Some(&b'.') || message.last() == Some(&b'\n') {
            message.pop();
        }
        let help = if self.use_err_help {
            let p = self.equiv_toks(ERR_HELP_LOC).cloned().unwrap_or_default();
            let text = self.diag_print(|t| t.token_show(&p));
            text.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect()
        } else {
            (0..self.help_ptr)
                .rev()
                .map(|i| self.help_line[i].to_vec())
                .collect()
        };
        let code = code_for(&message);
        let suggestions = if code == "undefined-control-sequence" {
            self.similar_control_sequences(self.cur_cs)
        } else {
            Vec::new()
        };
        let frames = self.diag_frames();
        let diagnostic = Diagnostic {
            severity,
            code,
            message,
            help,
            frames,
            suggestions,
            boxed: None,
        };
        self.out_diagnostic(diagnostic);
    }

    /// Whether warnings and notes go to the host.
    pub(crate) fn notes_wanted(&self) -> bool {
        self.diag.is_some() && self.host.notes()
    }

    /// Hand the host a warning or note (see the module documentation).
    pub(crate) fn diag_note(
        &mut self,
        severity: Severity,
        code: &'static str,
        message: Vec<u8>,
        boxed: Option<BoxWarning>,
    ) {
        let frames = self.diag_file_frames();
        let diagnostic = Diagnostic {
            severity,
            code,
            message,
            help: Vec::new(),
            frames,
            suggestions: Vec::new(),
            boxed,
        };
        self.out_diagnostic(diagnostic);
    }

    /// §660–§667, §674–§678 as data: the warning for packaging report `r`
    /// (of an hbox if `horizontal`), after TeX has printed it.
    pub(crate) fn diag_box(
        &mut self,
        r: partex_engine::pack::Report,
        horizontal: bool,
        node: &partex_engine::node::BoxNode,
    ) {
        use partex_engine::pack::Report;
        if !self.notes_wanted() {
            return;
        }
        let what = if horizontal { "hbox" } else { "vbox" };
        let (kind, amount) = match r {
            Report::Underfull { badness } | Report::Loose { badness } if badness > 100 => {
                ("Underfull", badness)
            }
            Report::Underfull { badness } | Report::Loose { badness } => ("Loose", badness),
            Report::Tight { badness } => ("Tight", badness),
            Report::Overfull { excess } => ("Overfull", excess),
        };
        let mut message = alloc::format!("{kind} \\{what} (");
        if kind == "Overfull" {
            let pt = self.diag_print(|t| t.print_scaled(amount));
            let too = if horizontal { "wide" } else { "high" };
            let _ = write!(message, "{}pt too {too})", lossy(&pt));
        } else {
            let _ = write!(message, "badness {amount})");
        }
        let (line, begin) = (self.line, self.pack_begin_line);
        let lines = if self.output_active() {
            message.push_str(" has occurred while \\output is active");
            (line, line)
        } else if begin != 0 {
            let at = if begin > 0 { "paragraph" } else { "alignment" };
            let _ = write!(message, " in {at} at lines {}--{line}", begin.abs());
            (begin.abs(), line)
        } else {
            let _ = write!(message, " detected at line {line}");
            (line, line)
        };
        let excerpt = if horizontal {
            let font = self.font_in_short_display;
            self.font_in_short_display = NULL_FONT;
            let text = self.diag_print(|t| t.short_display(&node.list));
            self.font_in_short_display = font;
            text
        } else {
            Vec::new()
        };
        let code = match (kind, horizontal) {
            ("Overfull", true) => "overfull-hbox",
            ("Overfull", false) => "overfull-vbox",
            ("Underfull", true) => "underfull-hbox",
            ("Underfull", false) => "underfull-vbox",
            ("Tight", true) => "tight-hbox",
            ("Tight", false) => "tight-vbox",
            (_, true) => "loose-hbox",
            (_, false) => "loose-vbox",
        };
        let boxed = BoxWarning {
            lines,
            amount,
            excerpt,
        };
        self.diag_note(Severity::Warning, code, message.into_bytes(), Some(boxed));
    }

    /// The file levels of the input stack, innermost first, without their
    /// text (a warning's or note's location).
    fn diag_file_frames(&mut self) -> Vec<Frame> {
        self.input_stack[self.input_ptr] = self.cur_input.clone();
        let mut frames = Vec::new();
        for level in (0..=self.input_ptr).rev() {
            let r: InStateRecord = self.input_stack[level].clone();
            if r.state == TOKEN_LIST || r.name <= 17 {
                continue;
            }
            let index = ux(r.index);
            let name_str = self.full_source_filename_stack[index];
            let name_str = if name_str == 0 { r.name } else { name_str };
            let name = self.diag_print(|t| t.slow_print(name_str));
            let line = if index == self.in_open {
                self.line
            } else {
                self.line_stack[index + 1]
            };
            frames.push(Frame {
                kind: FrameKind::File { name, line },
                before: Vec::new(),
                after: Vec::new(),
            });
        }
        frames
    }

    /// The input stack as frames, innermost first (like `show_context`,
    /// but every level and without truncation).
    fn diag_frames(&mut self) -> Vec<Frame> {
        let saved = self.cur_input.clone();
        self.input_stack[self.input_ptr] = self.cur_input.clone();
        let mut frames = Vec::new();
        for level in (0..=self.input_ptr).rev() {
            let r: InStateRecord = self.input_stack[level].clone();
            self.cur_input = r.clone();
            let frame = if r.state == TOKEN_LIST {
                self.diag_token_frame(&r)
            } else {
                self.diag_line_frame(&r)
            };
            frames.push(frame);
        }
        self.cur_input = saved;
        frames
    }

    fn diag_token_frame(&mut self, r: &InStateRecord) -> Frame {
        let kind = match r.index {
            PARAMETER => FrameKind::TokenList("argument"),
            U_TEMPLATE | V_TEMPLATE => FrameKind::TokenList("template"),
            BACKED_UP if r.loc == NULL => FrameKind::TokenList("recently read"),
            BACKED_UP => FrameKind::TokenList("to be read again"),
            INSERTED => FrameKind::TokenList("inserted text"),
            MACRO => {
                let name = self.diag_print(|t| t.sprint_cs(r.name));
                FrameKind::Macro { name }
            }
            OUTPUT_TEXT => FrameKind::TokenList("output"),
            EVERY_PAR_TEXT => FrameKind::TokenList("everypar"),
            EVERY_MATH_TEXT => FrameKind::TokenList("everymath"),
            EVERY_DISPLAY_TEXT => FrameKind::TokenList("everydisplay"),
            EVERY_HBOX_TEXT => FrameKind::TokenList("everyhbox"),
            EVERY_VBOX_TEXT => FrameKind::TokenList("everyvbox"),
            EVERY_JOB_TEXT => FrameKind::TokenList("everyjob"),
            EVERY_CR_TEXT => FrameKind::TokenList("everycr"),
            MARK_TEXT => FrameKind::TokenList("mark"),
            WRITE_TEXT => FrameKind::TokenList("write"),
            _ => FrameKind::TokenList("?"),
        };
        let list = r.list.clone().unwrap_or_default();
        let mut split = None;
        let text = self.diag_print(|t| {
            t.trick_count = 1_000_000;
            t.first_count = -1;
            t.show_token_list(&list, r.loc, 100_000);
            split = usize::try_from(t.first_count).ok();
        });
        let at = split.unwrap_or(text.len()).min(text.len());
        Frame {
            kind,
            before: text[..at].to_vec(),
            after: text[at..].to_vec(),
        }
    }

    fn diag_line_frame(&mut self, r: &InStateRecord) -> Frame {
        let kind = if r.name > 17 {
            let index = ux(r.index);
            let name_str = self.full_source_filename_stack[index];
            let name_str = if name_str == 0 { r.name } else { name_str };
            let name = self.diag_print(|t| t.slow_print(name_str));
            // The current line of an outer file is saved on `line_stack`.
            let line = if index == self.in_open {
                self.line
            } else {
                self.line_stack[index + 1]
            };
            FrameKind::File { name, line }
        } else if r.name == 0 {
            FrameKind::Terminal {
                inserted: r.index != 0,
            }
        } else {
            FrameKind::Read(r.name - 1)
        };
        // As in §318: the line ends before a final `end_line_char`.
        let start = ux(r.start);
        let limit = ux(r.limit.max(r.start - 1));
        let end = if i32::from(self.buffer[limit]) == self.int_par(END_LINE_CHAR_CODE) {
            limit
        } else {
            limit + 1
        }
        .max(start);
        let loc = ux(r.loc.clamp(r.start, i32::try_from(end).unwrap_or(0)));
        let before = self.diag_print(|t| {
            for k in start..loc {
                t.print(i32::from(t.buffer[k]));
            }
        });
        let after = self.diag_print(|t| {
            for k in loc..end.max(loc) {
                t.print(i32::from(t.buffer[k]));
            }
        });
        Frame {
            kind,
            before,
            after,
        }
    }

    /// Defined control sequences whose names are close to that of `cs`.
    fn similar_control_sequences(&self, cs: i32) -> Vec<Vec<u8>> {
        if cs < HASH_BASE || cs > self.eqtb_top {
            return Vec::new();
        }
        // (the tables peeked at, not read: the host's suggestions are no
        // read of the job's, which an SSA step would record for every
        // name of the hash at each undefined control sequence)
        let Ok(s) = usize::try_from(self.peek_text(cs)) else {
            return Vec::new();
        };
        if s < 256 || s >= self.str_ptr {
            return Vec::new();
        }
        let name = self.str_bytes(s).to_vec();
        let max = (name.len() / 3).clamp(1, 3);
        let mut best: Vec<(usize, Vec<u8>)> = Vec::new();
        for p in HASH_BASE..=self.hash_top.min(self.eqtb_top) {
            if p == cs || self.peek_eqtb(p).b0() == UNDEFINED_CS {
                continue;
            }
            let Ok(t) = usize::try_from(self.peek_text(p)) else {
                continue;
            };
            if t < 256 || t >= self.str_ptr {
                continue;
            }
            let cand = self.str_bytes(t);
            if cand.len().abs_diff(name.len()) > max {
                continue;
            }
            let d = edit_distance(&name, cand);
            if d <= max {
                best.push((d, cand.to_vec()));
            }
        }
        best.sort();
        best.into_iter().take(3).map(|(_, n)| n).collect()
    }
}

/// Optimal string alignment distance (Levenshtein plus transpositions).
fn edit_distance(a: &[u8], b: &[u8]) -> usize {
    let n = b.len();
    let mut prev2 = alloc::vec![0; n + 1];
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = alloc::vec![0; n + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=n {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
        }
        core::mem::swap(&mut prev2, &mut prev);
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

impl Diagnostic {
    /// The canonical text form:
    ///
    /// ```text
    /// error[undefined-control-sequence]: Undefined control sequence
    ///   --> paper.tex:12:7
    ///    |
    /// 12 | Hello \fooo world
    ///    |       ^^^^^ still to read: ` world`
    ///    = in expansion of \greet: `\fooo` | `{}`
    ///    = help: The control sequence at the end of the top line ...
    ///    = did you mean `\foo`?
    /// ```
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Fatal => "fatal",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        let _ = writeln!(out, "{sev}[{}]: {}", self.code, lossy(&self.message));
        // The innermost file line is the primary location.
        let primary = self
            .frames
            .iter()
            .position(|f| matches!(f.kind, FrameKind::File { .. } | FrameKind::Terminal { .. }));
        if let Some(i) = primary {
            let f = &self.frames[i];
            // The token that caused the error is the tail of `before` that
            // the innermost frame backed up, or the last word read.
            let col = f.before.len();
            let label = if let FrameKind::File { name, line } = &f.kind {
                let _ = writeln!(out, "  --> {}:{line}:{}", lossy(name), col + 1);
                alloc::format!("{line}")
            } else {
                let _ = writeln!(out, "  --> <terminal>:{}", col + 1);
                String::from("*")
            };
            let pad = " ".repeat(label.len());
            let _ = writeln!(out, "{pad} |");
            let _ = writeln!(out, "{label} | {}{}", lossy(&f.before), lossy(&f.after));
            let tok = last_token(&f.before);
            let _ = writeln!(
                out,
                "{pad} | {}{}",
                " ".repeat(col - tok),
                "^".repeat(tok.max(1))
            );
            for g in &self.frames[..i] {
                let what = match &g.kind {
                    FrameKind::Macro { name } => alloc::format!("in expansion of {}", lossy(name)),
                    FrameKind::TokenList(k) => alloc::format!("in <{k}>"),
                    _ => continue,
                };
                let _ = writeln!(
                    out,
                    "{pad} = {what}: `{}` | `{}`",
                    lossy(&g.before),
                    lossy(&g.after)
                );
            }
            for g in &self.frames[i + 1..] {
                if let FrameKind::File { name, line } = &g.kind {
                    let _ = writeln!(out, "{pad} = included from {}:{line}", lossy(name));
                }
            }
            write_tail(&mut out, &pad, self);
        } else {
            write_tail(&mut out, "", self);
        }
        out
    }
}

fn write_tail(out: &mut String, pad: &str, d: &Diagnostic) {
    for (i, h) in d.help.iter().enumerate() {
        let lead = if i == 0 { "help:" } else { "     " };
        let _ = writeln!(out, "{pad} = {lead} {}", lossy(h));
    }
    for s in &d.suggestions {
        let _ = writeln!(out, "{pad} = did you mean `\\{}`?", lossy(s));
    }
}

/// Length of the last token of `s`: a control word, `\x`, or one char.
fn last_token(s: &[u8]) -> usize {
    let n = s.len();
    if n == 0 {
        return 0;
    }
    let mut i = n;
    while i > 0 && s[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    if i > 0 && s[i - 1] == b'\\' && i < n {
        return n - i + 1;
    }
    if n >= 2 && s[n - 2] == b'\\' {
        return 2;
    }
    1
}

fn lossy(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SCROLL_MODE;
    use crate::testing::{engine, term_output};

    /// `Hello \greet world` on line 12 of `paper.tex`, where
    /// `\def\greet{\fooo{}}` and `\foo` is defined but `\fooo` is not.
    #[test]
    fn undefined_control_sequence_in_a_macro() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.interaction = SCROLL_MODE;
        t.no_new_control_sequence = false;
        let cs = |t: &mut Tex<_, _>, name: &[u8]| {
            t.buffer[..name.len()].copy_from_slice(name);
            t.id_lookup(0, name.len()).unwrap()
        };
        let foo = cs(&mut t, b"foo");
        t.set_eq_type(foo, CALL);
        let fooo = cs(&mut t, b"fooo");
        let greet = cs(&mut t, b"greet");

        // The file level.
        for &c in b"paper.tex" {
            t.append_char(c);
        }
        let name = i32::try_from(t.make_string().unwrap()).unwrap();
        let line = b"Hello \\greet world\r";
        t.buffer[1..=line.len()].copy_from_slice(line);
        t.set_int_par(END_LINE_CHAR_CODE, 13);
        t.in_open = 1;
        t.full_source_filename_stack[1] = name;
        t.cur_input = InStateRecord {
            list: None,
            state: MID_LINE,
            index: 1,
            start: 1,
            loc: 13,
            limit: 19,
            name,
        };
        t.line = 12;

        // The macro level: `->\fooo{}` with \fooo just read.
        let rc = t.tok_from(&[
            END_MATCH * 256,
            CS_TOKEN_FLAG + fooo,
            LEFT_BRACE * 256 + 123,
            RIGHT_BRACE * 256 + 125,
        ]);
        let after = 2; // the `{`
        t.begin_token_list(rc, MACRO).unwrap();
        t.cur_input.name = greet;
        t.cur_input.loc = after;
        t.cur_cs = fooo;

        let transcript = term_output(&mut t, |t| {
            t.print_err(b"Undefined control sequence");
            t.help(&[
                b"The control sequence at the end of the top line",
                b"of your error message was never \\def'ed.",
            ]);
            t.error().unwrap();
        });
        // TeX's own report is untouched. Oracle: the same input through
        // `tex -ini` in `\scrollmode` (the help goes to the log only).
        assert_eq!(
            core::str::from_utf8(&transcript).unwrap(),
            "! Undefined control sequence.\n\
             \\greet ->\\fooo \n               {}\n\
             l.12 Hello \\greet\n                  world\n"
        );

        let d = &t.host.diagnostics[0];
        assert_eq!(d.code, "undefined-control-sequence");
        assert_eq!(d.suggestions, [b"foo".to_vec()]);
        assert_eq!(
            d.render(),
            "error[undefined-control-sequence]: Undefined control sequence\n\
             \x20 --> paper.tex:12:13\n\
             \x20  |\n\
             12 | Hello \\greet world\n\
             \x20  |       ^^^^^^\n\
             \x20  = in expansion of \\greet: `->\\fooo ` | `{}`\n\
             \x20  = help: The control sequence at the end of the top line\n\
             \x20  =       of your error message was never \\def'ed.\n\
             \x20  = did you mean `\\foo`?\n"
        );
    }
}

partex_engine::persist_enum!(Severity {
    Error,
    Fatal,
    Warning,
    Note
});
partex_engine::persist_struct!(BoxWarning {
    lines,
    amount,
    excerpt
});
partex_engine::persist_struct!(Frame {
    kind,
    before,
    after
});
partex_engine::persist_enum!(FrameKind {
    File { name, line },
    Terminal { inserted },
    Read(a0),
    Macro { name },
    TokenList(a0),
});
partex_engine::persist_struct!(Diagnostic {
    severity,
    code,
    message,
    help,
    frames,
    suggestions,
    boxed
});
