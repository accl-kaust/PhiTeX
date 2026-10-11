//! The log parser: pdfTeX's e-TeX tracing log, streamed line by line, to
//! steps.
//!
//! What the log shows and how it is read:
//!
//! - `{\foo}` / `{vertical mode: \foo}` (`\tracingcommands`): a primitive
//!   executed, a character acted on (`{the letter A}`), a font selected,
//!   a group character; a step starts here.
//! - `{changing X=old}` `{into X=new}`, `{globally changing ...}`,
//!   `{reassigning X=v}` (`\tracingassigns`): a write of field `X` by the
//!   current step.
//! - `{restoring X=v}`, `{retaining X=v}` (`\tracingrestores`): writes of
//!   the current (group-ending) step; a restore models the group end as
//!   writing the outer value back.
//! - `{entering KIND group (level N) at line L}` / `{leaving KIND group
//!   (level N) entered at line L}` (`\tracinggroups`): the depth and,
//!   on entering, the input line.
//! - `\foo #1->body` then `#1<-arg` lines (`\tracingmacros`): a macro
//!   call with its arguments; token-list parameters (`\output->...`) too.
//! - `{\ifnum: (level N) entered on line L}`, `{true}`/`{false}`,
//!   `{\else: ...}`, `{\fi: ...}` (`\tracingifs`): conditional steps
//!   and the input line.
//! - `(./file.tex` and `)` on their own lines: the file stack (best
//!   effort); `l.N` error contexts: the input line.
//!
//! What it does not show, see the crate documentation.

use std::io::{self, BufRead};

use crate::fields::field_of;
use crate::hash::fnv1a;
use crate::prims;
use crate::step::{Consumes, Kind, MacroCall, Step, Write};

/// The parser's options.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Add by rule the reads the log can't show: every step after a
    /// catcode write reads `catcode:*`; a step whose reads are unknown
    /// (`"?"`) is also marked as reading `seq:prev`, which every step
    /// writes, so that the analyzer chains it to the step before it.
    pub conservative: bool,
}

/// The parser's state across lines.
pub struct Parser<W: FnMut(Step) -> io::Result<()>> {
    opts: Options,
    sink: W,
    next_id: u64,
    pending: Option<Step>,
    depth: u32,
    line: Option<u32>,
    /// The log's parentheses: a file opened, or `None` for a `(` of text.
    files: Vec<Option<String>>,
    /// A macro line seen, its arguments still to come.
    macro_args: Vec<String>,
    macro_arity: u32,
    catcode_written: bool,
    /// The current `\escapechar`: `None` when control sequences print
    /// without one (`\escapechar=-1`, as microtype sets it).
    escape: Option<char>,
    /// Lines the parser didn't recognise (counted, not kept).
    pub unrecognised: u64,
    /// Print unrecognised lines to stderr (`PHITEX_TRACE_DEBUG`).
    debug: bool,
    /// Inside a `{...}` diagnostic whose value spans lines.
    in_value: bool,
}

impl<W: FnMut(Step) -> io::Result<()>> Parser<W> {
    pub fn new(opts: Options, sink: W) -> Self {
        Self {
            opts,
            sink,
            next_id: 0,
            pending: None,
            depth: 0,
            line: None,
            files: Vec::new(),
            macro_args: Vec::new(),
            macro_arity: 0,
            catcode_written: false,
            escape: Some('\\'),
            unrecognised: 0,
            debug: std::env::var_os("PHITEX_TRACE_DEBUG").is_some(),
            in_value: false,
        }
    }

    /// Parses a whole log.
    pub fn parse<R: BufRead>(&mut self, reader: R) -> io::Result<()> {
        let mut buf = Vec::new();
        let mut reader = reader;
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let s = String::from_utf8_lossy(&buf);
            self.line(s.trim_end_matches(['\n', '\r']))?;
        }
        self.finish()
    }

    /// Flushes the last step.
    pub fn finish(&mut self) -> io::Result<()> {
        self.flush_macro();
        self.flush()
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(mut step) = self.pending.take() {
            if prims::reads_its_target(&step.name) {
                let targets: Vec<String> = step.writes.iter().map(|w| w.field.clone()).collect();
                step.reads.extend(targets);
            }
            if self.opts.conservative {
                if self.catcode_written && step.kind != Kind::GroupEnd {
                    step.reads.push("catcode:*".to_owned());
                }
                if step.reads.iter().any(|r| r == "?") {
                    step.reads.push("seq:prev".to_owned());
                }
                step.writes.push(Write {
                    field: "seq:prev".to_owned(),
                    global: true,
                    value: step.id,
                });
            }
            (self.sink)(step)?;
        }
        Ok(())
    }

    fn begin(
        &mut self,
        kind: Kind,
        name: &str,
        reads: &[&str],
        consumes: Option<Consumes>,
    ) -> io::Result<()> {
        self.flush_macro();
        self.flush()?;
        let id = self.next_id;
        self.next_id += 1;
        self.pending = Some(Step {
            id,
            kind,
            name: name.to_owned(),
            line: self.line,
            file: self.files.iter().rev().find_map(Clone::clone),
            depth: self.depth,
            reads: reads.iter().map(|s| (*s).to_owned()).collect(),
            writes: Vec::new(),
            consumes,
            macro_call: None,
        });
        Ok(())
    }

    /// A macro call whose arguments were collected: emits it.
    fn flush_macro(&mut self) {
        if let Some(step) = self.pending.as_mut()
            && step.kind == Kind::Macro
            && let Some(call) = step.macro_call.as_mut()
            && call.args_text_hash == 0
        {
            let mut h = fnv1a(b"");
            for a in &self.macro_args {
                h = crate::hash::fnv1a_with(h, a.as_bytes());
                h = crate::hash::fnv1a_with(h, b"\0");
            }
            call.args_text_hash = h;
            call.args = self.macro_arity;
            if self.macro_arity > 0 {
                step.consumes = Some(Consumes::Known);
            }
            self.macro_args.clear();
            self.macro_arity = 0;
        }
    }

    /// A control sequence's name as the log prints it, with a backslash:
    /// after `\escapechar=-1` the log prints `expandafter` and `MT@feat`.
    fn normalise(&self, name: &str) -> String {
        // A `restoring` line prints after the restore, with the escape
        // character restored: a leading backslash is always one.
        match self.escape {
            _ if name.starts_with('\\') => name.to_owned(),
            Some(c) if name.starts_with(c) => format!("\\{}", &name[c.len_utf8()..]),
            Some(_) | None => format!("\\{name}"),
        }
    }

    /// Whether `inner` names a control sequence, with the current escape.
    fn is_cs(&self, inner: &str) -> bool {
        inner.starts_with('\\')
            || match self.escape {
                Some('\\') => false,
                Some(c) => {
                    inner.starts_with(c)
                        || inner.starts_with(|c: char| c.is_alphabetic() || c == '@')
                }
                None => inner.starts_with(|c: char| c.is_alphabetic() || c == '@'),
            }
    }

    fn track_escape(&mut self, field: &str, value: &str) {
        if field == "param:\\escapechar" {
            self.escape = value
                .trim()
                .parse::<i32>()
                .ok()
                .and_then(|n| u32::try_from(n).ok())
                .and_then(char::from_u32);
        }
    }

    fn write(&mut self, text: &str, global: bool, with_value: bool) {
        let (name, value) = split_assignment(text);
        if with_value
            && let Some(step) = self.pending.as_mut()
            && let Some(w) = step.writes.last_mut()
            && w.value == 0
        {
            // `into X=new` completes the `changing X=old` before it. (The
            // name may already print without an escape character when
            // the assignment was to `\escapechar` itself.)
            w.value = fnv1a(value.as_bytes());
            let field = w.field.clone();
            self.track_escape(&field, value);
            return;
        }
        let name = if name == "current font" {
            name.to_owned()
        } else {
            self.normalise(name)
        };
        let field = field_of(&name);
        if with_value {
            self.track_escape(&field, value);
        }
        if field.starts_with("catcode:") {
            self.catcode_written = true;
        }
        if self.pending.is_none() {
            // An assignment outside any traced command (the `\everyjob`
            // or a `\tracingcommands=0` region): give it a step of its
            // own.
            let _ = self.begin(Kind::Primitive, "assignment", &[], None);
        }
        let step = self.pending.as_mut().expect("a step");
        step.writes.push(Write {
            field,
            global,
            value: if with_value {
                fnv1a(value.as_bytes())
            } else {
                0
            },
        });
    }

    fn note_unrecognised(&mut self, raw: &str) {
        self.unrecognised += 1;
        if self.debug {
            eprintln!("unrecognised: {raw}");
        }
    }

    /// One line of the log.
    pub fn line(&mut self, raw: &str) -> io::Result<()> {
        if raw.is_empty() {
            return Ok(());
        }
        // A macro argument, or a macro line, or its continuation.
        if let Some(rest) = raw.strip_prefix('#')
            && let Some(p) = rest.find("<-")
            && rest[..p].bytes().all(|b| b.is_ascii_digit())
            && self.pending.as_ref().is_some_and(|s| s.kind == Kind::Macro)
        {
            self.macro_args.push(rest[p + 2..].to_owned());
            return Ok(());
        }
        if self.is_cs(raw)
            && let Some(p) = raw.find("->")
            && raw[..p].find(' ').is_none_or(|sp| {
                sp + 1 == p
                    || raw.as_bytes()[..p]
                        .get(sp + 1)
                        .is_some_and(|&b| b == b'#' || b == b' ' || b == b'-')
            })
        {
            let head = &raw[..p];
            // A control word prints with a space after it (`\bye ->`), a
            // control symbol without (`\\->`); parameters follow (`#1#2`).
            let name_len = head[1..]
                .find([' ', '#'])
                .map_or(head.len(), |i| i + 1)
                .max(2.min(head.len()));
            let name = self.normalise(&head[..name_len]);
            let name = name.as_str();
            let params = &head[name_len..];
            // TeX prints the parameter text with the arguments it takes:
            // `#1#2`, `#1.#2`; count the `#n`s.
            let arity = params.matches('#').count();
            let read = field_of(name);
            self.begin(Kind::Macro, name, &[], None)?;
            let Some(step) = self.pending.as_mut() else {
                return Ok(());
            };
            step.reads.push(read);
            step.macro_call = Some(MacroCall {
                r#macro: name.to_owned(),
                args_text_hash: 0,
                args: 0,
            });
            self.macro_arity = u32::try_from(arity).unwrap_or(u32::MAX);
            return Ok(());
        }
        if let Some(n) = raw.strip_prefix("l.")
            && let Some(p) = n.find(|c: char| !c.is_ascii_digit())
            && p > 0
        {
            self.line = n[..p].parse().ok();
            return Ok(());
        }
        if let Some(inner) = raw.strip_prefix('{') {
            // A value with line breaks in it (`{into \\foo=...` continuing
            // on the next lines) is read from its first line; the rest is
            // skipped below (`in_value`).
            self.in_value = !raw.ends_with('}');
            return self.braced(inner.strip_suffix('}').unwrap_or(inner));
        }
        if self.in_value {
            self.in_value = !raw.ends_with('}');
            return Ok(());
        }
        // File opens and closes print at the start of a line, or after a
        // page's `]`; anything else with parentheses is text.
        if let Some(p) = raw.rfind(']')
            && raw.starts_with('[')
        {
            self.parens(&raw[p + 1..]);
        } else if raw.starts_with(['(', ')']) {
            self.parens(raw);
        }
        Ok(())
    }

    /// File opens and closes on a line of the log.
    fn parens(&mut self, raw: &str) {
        let b = raw.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'(' => {
                    let start = i + 1;
                    let mut j = start;
                    while j < b.len() && b[j] != b' ' && b[j] != b')' && b[j] != b'(' {
                        j += 1;
                    }
                    let name = &raw[start..j];
                    let is_file =
                        name.contains('/') || (name.contains('.') && !name.ends_with('.'));
                    if self.debug && is_file {
                        eprintln!("open {name}: {raw}");
                    }
                    self.files.push(is_file.then(|| name.to_owned()));
                    i = j;
                }
                b')' => {
                    let popped = self.files.pop();
                    if self.debug {
                        eprintln!("close {popped:?}: {raw}");
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }

    fn braced(&mut self, inner: &str) -> io::Result<()> {
        if inner.starts_with('/') {
            // A font map file loaded while shipping out.
            return Ok(());
        }
        let mut inner = inner;
        if let Some(p) = inner.find(": ")
            && MODES.contains(&&inner[..p])
        {
            inner = &inner[p + 2..];
        }
        if let Some(t) = inner.strip_prefix("globally changing ") {
            self.write(t, true, false);
        } else if let Some(t) = inner.strip_prefix("changing ") {
            self.write(t, false, false);
        } else if let Some(t) = inner.strip_prefix("into ") {
            self.write(t, false, true);
        } else if let Some(t) = inner.strip_prefix("globally reassigning ") {
            self.write(t, true, true);
        } else if let Some(t) = inner.strip_prefix("reassigning ") {
            self.write(t, false, true);
        } else if let Some(t) = inner.strip_prefix("restoring ") {
            self.restore(t, false)?;
        } else if let Some(t) = inner.strip_prefix("retaining ") {
            self.restore(t, true)?;
        } else if let Some(t) = inner.strip_prefix("entering ") {
            if let Some((level, line)) = level_and_line(t, " at line ") {
                self.depth = level;
                self.line = Some(line);
            }
        } else if let Some(t) = inner.strip_prefix("leaving ") {
            if let Some((level, _)) = level_and_line(t, " entered at line ") {
                self.depth = level.saturating_sub(1);
            }
        } else if inner == "true" || inner == "false" || inner.starts_with("case ") {
            // The conditional's outcome; nothing to record.
        } else if self.is_cs(inner)
            && let Some(p) = inner.find(": ")
            && (inner[p..].contains("(level ") || inner[p..].contains("entered"))
        {
            let name = &self.normalise(&inner[..p]);
            let rest = &inner[p + 2..];
            if let Some(l) = rest.rfind(" entered on line ")
                && let Ok(n) = rest[l + " entered on line ".len()..].trim().parse::<u32>()
                && !rest.starts_with('\\')
            {
                // `\ifnum: (level 1) entered on line 4`: the test itself.
                self.line = Some(n);
            }
            self.begin(Kind::If, name, &["?"], None)?;
        } else if let Some(t) = inner.strip_prefix("begin-group character ") {
            self.begin(Kind::GroupBegin, t, &[], None)?;
        } else if let Some(t) = inner.strip_prefix("end-group character ") {
            self.begin(Kind::GroupEnd, t, &[], None)?;
        } else if let Some(t) = inner.strip_prefix("select font ") {
            self.begin(Kind::Font, t, &[], None)?;
        } else if inner.starts_with('\\')
            || (self.escape != Some('\\')
                && inner != "undefined"
                && char_class(inner).is_none()
                && self.is_cs(inner))
        {
            let name = self.normalise(inner);
            let inner = name.as_str();
            let (reads, consumes) = prims::reads(inner);
            self.begin(Kind::Primitive, inner, reads, consumes)?;
            if inner == "\\par" && self.depth == 0 {
                let step = self.pending.as_mut().expect("a step");
                step.reads
                    .extend(prims::page_reads().iter().map(|s| (*s).to_owned()));
            }
        } else if let Some(class) = char_class(inner) {
            self.begin(Kind::Char, class, prims::char_reads(class), None)?;
        } else if inner == "undefined" {
            self.begin(Kind::Primitive, "undefined", &[], None)?;
        } else {
            self.note_unrecognised(inner);
        }
        Ok(())
    }

    fn restore(&mut self, text: &str, retaining: bool) -> io::Result<()> {
        // Restores follow the end-group character or `\endgroup`; after a
        // box or output group closed by the engine itself they may follow
        // another step: then the restores form a step of their own.
        let attach = self.pending.as_ref().is_some_and(|s| {
            s.kind == Kind::GroupEnd || (s.kind == Kind::Primitive && s.name.starts_with("\\end"))
        });
        if !attach {
            self.begin(Kind::GroupEnd, "}", &[], None)?;
        }
        let (name, value) = split_assignment(text);
        let name = if name == "current font" {
            name.to_owned()
        } else {
            self.normalise(name)
        };
        let field = field_of(&name);
        self.track_escape(&field, value);
        if field.starts_with("catcode:") {
            self.catcode_written = true;
        }
        let step = self.pending.as_mut().expect("a step");
        step.writes.push(Write {
            field,
            global: retaining,
            value: fnv1a(value.as_bytes()),
        });
        Ok(())
    }
}

/// The modes `\tracingcommands` prefixes a command with on a mode change
/// (tex.web §211).
const MODES: &[&str] = &[
    "vertical mode",
    "horizontal mode",
    "display math mode",
    "no mode",
    "internal vertical mode",
    "restricted horizontal mode",
    "math mode",
];

/// `X=v` into (`X`, `v`); an active `=` is its own name.
fn split_assignment(text: &str) -> (&str, &str) {
    match text.find('=') {
        Some(0) => ("=", &text[2..]),
        Some(p) => (&text[..p], &text[p + 1..]),
        None => (text, ""),
    }
}

/// `KIND group (level N) at line L` into (N, L).
fn level_and_line(text: &str, sep: &str) -> Option<(u32, u32)> {
    let l = text.find("(level ")?;
    let rest = &text[l + "(level ".len()..];
    let close = rest.find(')')?;
    let level = rest[..close].parse().ok()?;
    let line = rest[close..]
        .strip_prefix(')')?
        .strip_prefix(sep)?
        .trim()
        .parse()
        .ok()?;
    Some((level, line))
}

/// The class of a character command (`the letter A` -> `the letter`).
fn char_class(inner: &str) -> Option<&'static str> {
    const CLASSES: &[&str] = &[
        "the letter",
        "the character",
        "blank space",
        "math shift character",
        "alignment tab character",
        "subscript character",
        "superscript character",
        "macro parameter character",
        "end of alignment template",
        "insert",
    ];
    CLASSES.iter().copied().find(|c| inner.starts_with(c))
}

/// Parses a log into steps, calling `sink` for each.
pub fn parse_log<R: BufRead, W: FnMut(Step) -> io::Result<()>>(
    reader: R,
    opts: Options,
    sink: W,
) -> io::Result<u64> {
    let mut p = Parser::new(opts, sink);
    p.parse(reader)?;
    Ok(p.unrecognised)
}

/// Parses a log held in memory (for tests).
///
/// # Panics
///
/// Never: the sink doesn't fail.
#[must_use]
pub fn parse_str(log: &str, opts: Options) -> Vec<Step> {
    let mut v = Vec::new();
    parse_log(log.as_bytes(), opts, |s| {
        v.push(s);
        Ok(())
    })
    .expect("in-memory parse");
    v
}

#[cfg(test)]
mod tests {
    use super::{Options, parse_str};
    use crate::step::{Consumes, Kind};

    #[test]
    fn assignments_attach_to_the_primitive() {
        let log = "{vertical mode: \\count}\n{changing \\count1=0}\n{into \\count1=7}\n{\\global}\n{\\dimen}\n{globally changing \\dimen3=0.0pt}\n{into \\dimen3=2.0pt}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].name, "\\count");
        assert_eq!(s[0].writes.len(), 1);
        assert_eq!(s[0].writes[0].field, "count:1");
        assert!(!s[0].writes[0].global);
        assert_ne!(s[0].writes[0].value, 0);
        assert_eq!(s[2].writes[0].field, "dimen:3");
        assert!(s[2].writes[0].global);
    }

    #[test]
    fn groups_and_restores() {
        let log = "{begin-group character {}\n{entering simple group (level 1) at line 4}\n{\\count}\n{changing \\count1=7}\n{into \\count1=5}\n{end-group character }}\n{restoring \\count1=7}\n{retaining \\dimen0=1.0pt}\n{leaving simple group (level 1) entered at line 4}\n{\\par}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s[0].kind, Kind::GroupBegin);
        assert_eq!(s[0].depth, 0);
        assert_eq!(s[1].depth, 1);
        assert_eq!(s[1].line, Some(4));
        assert_eq!(s[2].kind, Kind::GroupEnd);
        assert_eq!(s[2].writes.len(), 2);
        assert_eq!(s[2].writes[0].field, "count:1");
        assert!(!s[2].writes[0].global);
        assert!(s[2].writes[1].global);
        assert_eq!(s[3].depth, 0);
    }

    #[test]
    fn macro_call_with_arguments() {
        let log = "{\\relax}\n\n\\foo #1#2->\\count 1=#1 #2\n#1<-7\n#2<-x y\n{\\count}\n{changing \\count1=0}\n{into \\count1=7}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s.len(), 3);
        let m = &s[1];
        assert_eq!(m.kind, Kind::Macro);
        assert_eq!(m.reads, vec!["cs:\\foo".to_owned()]);
        let call = m.macro_call.as_ref().expect("a call");
        assert_eq!(call.r#macro, "\\foo");
        assert_eq!(call.args, 2);
        assert_ne!(call.args_text_hash, 0);
        assert_eq!(m.consumes, Some(Consumes::Known));
        // The same call again hashes the same; other arguments differ.
        let t = parse_str(
            "\\foo #1#2->\\count 1=#1 #2\n#1<-7\n#2<-x y\n",
            Options::default(),
        );
        assert_eq!(
            t[0].macro_call.as_ref().expect("a call").args_text_hash,
            call.args_text_hash
        );
        let u = parse_str(
            "\\foo #1#2->\\count 1=#1 #2\n#1<-8\n#2<-x y\n",
            Options::default(),
        );
        assert_ne!(
            u[0].macro_call.as_ref().expect("a call").args_text_hash,
            call.args_text_hash
        );
    }

    #[test]
    fn output_token_list_is_a_macro_step_reading_the_parameter() {
        let s = parse_str("\\output->{\\plainoutput }\n", Options::default());
        assert_eq!(s[0].kind, Kind::Macro);
        assert_eq!(s[0].reads, vec!["param:\\output".to_owned()]);
        assert_eq!(s[0].consumes, None);
    }

    #[test]
    fn conditionals_and_lines() {
        let log = "{\\ifnum: (level 1) entered on line 4}\n{true}\n{the letter y}\n{\\else: \\ifnum (level 1) entered on line 4}\n{\\fi: \\ifnum (level 1) entered on line 4}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].kind, Kind::If);
        assert_eq!(s[0].name, "\\ifnum");
        assert_eq!(s[0].line, Some(4));
        assert_eq!(s[0].reads, vec!["?".to_owned()]);
        assert_eq!(s[1].kind, Kind::Char);
        assert_eq!(s[1].name, "the letter");
        assert!(s[1].reads.contains(&"font:current".to_owned()));
        assert_eq!(s[2].name, "\\else");
        assert_eq!(s[3].name, "\\fi");
    }

    #[test]
    fn error_context_line_and_files() {
        let log = "(./main.tex (./sub.tex\n{\\par}\n! Undefined control sequence.\nl.12 \\foo\n{\\relax}\n)\n{\\par}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s[0].file.as_deref(), Some("./sub.tex"));
        assert_eq!(s[0].line, None);
        assert_eq!(s[1].line, Some(12));
        assert_eq!(s[2].file.as_deref(), Some("./main.tex"));
    }

    #[test]
    fn font_and_conservative() {
        let log = "{\\catcode}\n{changing \\catcode64=12}\n{into \\catcode64=11}\n{select font cmr10}\n{reassigning current font=\\x}\n{the letter A}\n{\\the}\n";
        let s = parse_str(log, Options { conservative: true });
        assert_eq!(s[1].kind, Kind::Font);
        assert_eq!(s[1].writes[0].field, "font:current");
        assert!(s[2].reads.contains(&"catcode:*".to_owned()));
        assert!(s[3].reads.contains(&"seq:prev".to_owned()));
        let plain = parse_str(log, Options::default());
        assert!(!plain[2].reads.iter().any(|r| r == "catcode:*"));
    }

    #[test]
    fn advance_reads_its_target() {
        let log = "{\\advance}\n{globally changing \\count0=1}\n{into \\count0=2}\n";
        let s = parse_str(log, Options::default());
        assert_eq!(s[0].reads, vec!["?".to_owned(), "count:0".to_owned()]);
    }
}

#[cfg(test)]
mod escape_tests {
    use super::{Options, parse_str};
    use crate::step::Kind;

    #[test]
    fn names_without_an_escape_character() {
        let log = "{\\escapechar}\n{changing \\escapechar=92}\n{into escapechar=-1}\n{expandafter}\n\nMT@feat ->pr\n{def}\n{changing MT@tempa=undefined}\n{into MT@tempa=macro:->x}\n{ifnum: (level 9) entered on line 23}\n{true}\n{escapechar}\n{changing escapechar=-1}\n{into escapechar=92}\n{\\relax}\n{\\escapechar}\n{changing \\escapechar=92}\n{into escapechar=-1}\n{restoring \\escapechar=92}\n{\\ifmmode: (level 2) entered on line 54}\n{case 2}\n";
        let s = parse_str(log, Options::default());
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "\\escapechar",
                "\\expandafter",
                "\\MT@feat",
                "\\def",
                "\\ifnum",
                "\\escapechar",
                "\\relax",
                "\\escapechar",
                "}",
                "\\ifmmode"
            ]
        );
        assert_eq!(s[2].kind, Kind::Macro);
        assert_eq!(s[2].reads, vec!["cs:\\MT@feat".to_owned()]);
        assert_eq!(s[3].writes[0].field, "cs:\\MT@tempa");
        assert_eq!(s[4].line, Some(23));
    }
}
