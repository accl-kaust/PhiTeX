//! A build's errors on the terminal as compilers report theirs
//! (annotate-snippets): the file and line, the line as TeX read it with
//! the culprit underlined, the macros it was in as notes, TeX's help as
//! the help note. Read from the transcript, where TeX lays each error out
//! (`! message`, the context levels two lines each, `l.N` with the line
//! split where it was read, the help to the blank line); the session's
//! `Output::errors` are the messages, matched to these displays.

use annotate_snippets::{Level, Renderer, Snippet};

use crate::ui::Ui;

/// One error as the transcript shows it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Problem {
    /// TeX's message (without `! ` and the final period).
    pub message: String,
    /// The file open at the error (the transcript's parentheses).
    pub file: Option<String>,
    pub line: Option<usize>,
    /// The line as TeX read it and the rest of it; the culprit at
    /// `start..end` (bytes).
    pub text: String,
    pub start: usize,
    pub end: usize,
    /// The levels above the line, innermost first (`\a #1->\b {#1}`).
    pub context: Vec<String>,
    pub help: Vec<String>,
    /// A warning, not an error.
    pub warning: bool,
    /// What the underline says.
    pub label: Option<String>,
}

/// The engine's diagnostic as a problem: its line cut from `source` (the
/// file's bytes; none: not readable, the place only), the culprit at its
/// bytes.
pub fn from_diagnostic(
    d: &phitex_tex::Diagnostic,
    source: Option<&[u8]>,
    verbose: bool,
) -> Problem {
    let mut p = Problem {
        message: d.message.trim_end_matches('.').to_owned(),
        file: Some(d.file.trim_start_matches("./").to_owned()),
        line: Some(d.line),
        help: d
            .help
            .iter()
            .flat_map(|h| h.lines())
            .map(str::to_owned)
            .collect(),
        warning: d.severity == phitex_tex::Severity::Warning,
        ..Problem::default()
    };
    // (TeX's help for an undefined name speaks to its prompt, `type I`:
    // the underline says it instead; the names it may have meant stay)
    if d.message.starts_with("Undefined control sequence") && !verbose {
        p.help.retain(|h| h.starts_with("Did you mean"));
        p.label = Some("not defined".to_owned());
    }
    if let Some(src) = source.filter(|s| d.range.0 <= s.len()) {
        let (a, b) = (d.range.0, d.range.1.clamp(d.range.0, src.len()));
        let start = src[..a]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |k| k + 1);
        let end = src[a..]
            .iter()
            .position(|&c| c == b'\n' || c == b'\r')
            .map_or(src.len(), |k| a + k);
        let line = &src[start..end];
        // (the text as UTF-8 when it is, else a byte at a time, those
        // past 127 as TeX prints them (§49, `^^d2`): the renderer counts
        // columns in characters and slices at them, so the text shown is
        // one byte a character; the underline's bytes are counted in it)
        let lossy = std::str::from_utf8(line).is_err();
        let text = if lossy {
            printable(line)
        } else {
            String::from_utf8_lossy(line).into_owned()
        };
        let pos = |k: usize| {
            if lossy {
                printable(&line[..k]).len()
            } else {
                k
            }
        };
        p.start = pos(a - start);
        p.end = pos(b.min(end) - start).max(p.start + 1);
        // (a whole token at least: the control sequence's name, a word)
        if b <= a {
            let rest = &line[a - start..];
            let n = match rest.first() {
                Some(b'\\') => {
                    1 + rest[1..]
                        .iter()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .count()
                        .max(1)
                }
                Some(_) => rest
                    .iter()
                    .take_while(|c| !c.is_ascii_whitespace())
                    .count()
                    .max(1),
                None => 1,
            };
            p.end = pos((a - start + n).min(line.len())).max(p.start + 1);
        }
        p.text = text;
    }
    p
}

/// Bytes as TeX prints them (§49): those past 127 as `^^` and two
/// lowercase hex digits, the rest as they are.
fn printable(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &c in b {
        if c < 128 {
            s.push(char::from(c));
        } else {
            let _ = std::fmt::Write::write_fmt(&mut s, format_args!("^^{c:02x}"));
        }
    }
    s
}

/// The file names `line` opens with `(` and closes with `)`, as TeX
/// prints them, applied to `stack`.
fn track_files(line: &str, stack: &mut Vec<String>) {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' => {
                let rest = &line[i + 1..];
                let end = rest
                    .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
                    .unwrap_or(rest.len());
                let name = &rest[..end];
                if !name.is_empty()
                    && (name.starts_with('/') || name.starts_with('.') || name.contains('.'))
                {
                    stack.push(name.to_owned());
                    i += 1 + end;
                    continue;
                }
            }
            b')' => {
                stack.pop();
            }
            _ => {}
        }
        i += 1;
    }
}

/// `l.N read`: the line number and what TeX had read of the line.
fn line_number(s: &str) -> Option<(usize, &str)> {
    let rest = s.strip_prefix("l.")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let n = rest[..digits].parse().ok()?;
    match rest[digits..].strip_prefix(' ') {
        Some(read) => Some((n, read)),
        None if rest.len() == digits => Some((n, "")),
        None => None,
    }
}

/// The control sequence `msg` names (`Undefined control sequence \foo`).
fn named_cs(msg: &str) -> Option<String> {
    let i = msg.find('\\')?;
    let rest = &msg[i + 1..];
    let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
    if letters > 0 {
        Some(format!("\\{}", &rest[..letters]))
    } else {
        rest.chars()
            .next()
            .filter(|c| !c.is_whitespace())
            .map(|c| format!("\\{c}"))
    }
}

/// The bytes of the last token of `s`: a control word, a control symbol,
/// or one character.
fn last_token(s: &str) -> usize {
    let b = s.as_bytes();
    let n = b.len();
    if n == 0 {
        return 0;
    }
    let mut i = n;
    while i > 0 && b[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    if i > 0 && b[i - 1] == b'\\' && i < n {
        return n - i + 1;
    }
    if n >= 2 && b[n - 2] == b'\\' && b[n - 1].is_ascii() {
        return 2;
    }
    s.chars().last().map_or(0, char::len_utf8)
}

/// Where `cs` is last in `text` as a token (a control word not followed
/// by a letter).
fn find_cs(text: &str, cs: &str) -> Option<usize> {
    let word = cs.len() > 2 || cs.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic);
    text.match_indices(cs)
        .filter(|(at, _)| {
            !word || !text[at + cs.len()..].starts_with(|c: char| c.is_ascii_alphabetic())
        })
        .map(|(at, _)| at)
        .last()
}

/// The place of the error in its line: `read` is what TeX had read of it
/// (a control word printed with a space after it), `rest` what follows.
fn place(p: &mut Problem, read: &str, rest: &str) {
    let read = read
        .strip_suffix(' ')
        .filter(|r| last_token(r) > 1)
        .unwrap_or(read);
    p.text = format!("{read}{rest}");
    let named =
        named_cs(&p.message).and_then(|cs| find_cs(&p.text, &cs).map(|at| (at, at + cs.len())));
    (p.start, p.end) = named.unwrap_or_else(|| {
        let n = last_token(read);
        (read.len() - n, read.len())
    });
}

/// The errors transcript `log` lays out; `main` is the file open when the
/// transcript names none.
pub fn parse(log: &str, main: &str) -> Vec<Problem> {
    let lines: Vec<&str> = log.lines().collect();
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(msg) = lines[i].strip_prefix("! ") else {
            track_files(lines[i], &mut stack);
            i += 1;
            continue;
        };
        let mut p = Problem {
            message: msg.trim_end_matches('.').to_owned(),
            file: Some(stack.last().cloned().unwrap_or_else(|| main.to_owned())),
            ..Problem::default()
        };
        i += 1;
        while i < lines.len() && !lines[i].is_empty() {
            let c = lines[i];
            if let Some((n, read)) = line_number(c) {
                p.line = Some(n);
                let under = lines.get(i + 1).copied().filter(|u| !u.is_empty());
                place(&mut p, read, under.map_or("", str::trim_start));
                i += if under.is_some() { 2 } else { 1 };
                while i < lines.len() && !lines[i].is_empty() {
                    p.help.push(lines[i].to_owned());
                    i += 1;
                }
                break;
            }
            let mut entry = c.trim_end().to_owned();
            if c.starts_with('\\') || c.starts_with('<') {
                // (the level's two lines: before and after the point read)
                if let Some(under) = lines.get(i + 1).filter(|u| !u.is_empty()) {
                    let after = under.trim();
                    if !after.is_empty() {
                        entry.push_str(" ‸ ");
                        entry.push_str(after);
                    }
                    i += 1;
                }
            }
            p.context.push(entry);
            i += 1;
        }
        out.push(p);
    }
    out
}

/// The underline `start..end` (bytes of `text`) on whole characters:
/// the renderer slices `text` at both ends, and a line held a character
/// a byte (Latin-1 text, not UTF-8) has characters of two bytes, so a
/// byte count from the transcript or a `+ 1` can fall inside one. The
/// start goes back to its character's first byte, the end on to the
/// next boundary, and the span covers one character at least.
fn span(text: &str, start: usize, end: usize) -> (usize, usize) {
    let floor = |mut k: usize| {
        k = k.min(text.len());
        while !text.is_char_boundary(k) {
            k -= 1;
        }
        k
    };
    let ceil = |mut k: usize| {
        k = k.min(text.len());
        while !text.is_char_boundary(k) {
            k += 1;
        }
        k
    };
    let mut a = floor(start);
    let mut b = ceil(end.max(a));
    if a == b {
        match text[a..].chars().next() {
            Some(c) => b = a + c.len_utf8(),
            None => {
                a = text[..a]
                    .chars()
                    .next_back()
                    .map_or(a, |c| a - c.len_utf8());
            }
        }
    }
    (a, b)
}

/// The problem as a compiler's report.
pub fn render(p: &Problem, color: bool) -> String {
    let origin = p.file.clone().unwrap_or_default();
    let notes: Vec<String> = p
        .context
        .iter()
        .map(|c| {
            if c == "..." {
                "more levels above (\\errorcontextlines)".to_owned()
            } else {
                format!("in {c}")
            }
        })
        .collect();
    let help = p.help.join("\n");
    let at = p
        .line
        .filter(|_| p.text.is_empty())
        .map(|n| format!("at {origin}:{n}"));
    let level = if p.warning {
        Level::Warning
    } else {
        Level::Error
    };
    let mut m = level.title(&p.message);
    if let Some(n) = p.line.filter(|_| !p.text.is_empty()) {
        let (start, end) = span(&p.text, p.start, p.end);
        m = m.snippet(
            Snippet::source(&p.text)
                .line_start(n)
                .origin(&origin)
                .fold(false)
                .annotation(match &p.label {
                    Some(l) => level.span(start..end).label(l),
                    None => level.span(start..end),
                }),
        );
    }
    if let Some(at) = &at {
        m = m.footer(Level::Note.title(at));
    }
    for n in &notes {
        m = m.footer(Level::Note.title(n));
    }
    if !help.is_empty() {
        m = m.footer(Level::Help.title(&help));
    }
    let r = if color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    r.render(m).to_string()
}

/// The build's errors on the terminal: each of `errors` as the transcript
/// lays it out, else as its line; the first ten (`-v`: a hundred).
pub fn show(ui: &Ui, errors: &[String], log: &str, main: &str) {
    let max = if ui.verbose > 0 { 100 } else { 10 };
    let problems = parse(log, main);
    let mut used = vec![false; problems.len()];
    for e in errors.iter().take(max) {
        let m = bare(e);
        let key = m.trim_end_matches('.');
        match (0..problems.len()).find(|&k| !used[k] && problems[k].message == key) {
            Some(k) => {
                used[k] = true;
                ui.line(&render(&problems[k], ui.st.color));
            }
            None => ui.error(m),
        }
    }
}

/// The build's errors (or `warnings`) at their places, each as a
/// compiler's report.
pub fn rendered(
    s: &crate::session::Session,
    r: &crate::session::Report,
    warnings: bool,
    color: bool,
    verbose: bool,
) -> Vec<String> {
    let want = if warnings {
        phitex_tex::Severity::Warning
    } else {
        phitex_tex::Severity::Error
    };
    r.diagnostics
        .iter()
        .filter(|d| d.severity == want)
        .map(|d| {
            render(
                &from_diagnostic(d, s.source(&d.file).as_deref(), verbose),
                color,
            )
        })
        .collect()
}

/// An error without the piece it was met in (`piece K: …`).
pub fn bare(e: &str) -> &str {
    match e.split_once(": ") {
        Some((p, m)) if p.starts_with("piece ") => m,
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = concat!(
        "This is TeX\n",
        "(./paper.tex (/tex/article.cls)\n",
        "! Undefined control sequence \\undefinedcs.\n",
        "\\b #1->x #1 \\undefinedcs \n",
        "                        y\n",
        "...\n",
        "l.5 Hello \\a{w}\n",
        "               \n",
        "The control sequence at the end of the top line\n",
        "of your error message was never \\def'ed.\n",
        "\n",
        "! Missing $ inserted.\n",
        "<inserted text> \n",
        "                $\n",
        "l.9 word _\n",
        "          x\n",
        "I've inserted a begin-math/end-math symbol.\n",
        "\n",
        "[1] )\n"
    );

    #[test]
    fn parses_the_transcript() {
        let ps = parse(LOG, "main.tex");
        assert_eq!(ps.len(), 2);
        let p = &ps[0];
        assert_eq!(p.message, "Undefined control sequence \\undefinedcs");
        assert_eq!(p.file.as_deref(), Some("./paper.tex"));
        assert_eq!(p.line, Some(5));
        assert_eq!(p.text, "Hello \\a{w}");
        assert_eq!(&p.text[p.start..p.end], "}");
        assert_eq!(p.context, ["\\b #1->x #1 \\undefinedcs ‸ y", "..."]);
        assert_eq!(p.help.len(), 2);
        let q = &ps[1];
        assert_eq!(q.line, Some(9));
        assert_eq!(q.text, "word _x");
        assert_eq!(&q.text[q.start..q.end], "_");
        assert_eq!(q.context, ["<inserted text> ‸ $"]);
    }

    /// A line held a character a byte (koi8-r, not UTF-8): its bytes
    /// past 127 shown as `^^xx`, the underline on the control symbol, and
    /// the renderer never slices inside a character (it panicked: "end
    /// byte index 7 is not a char boundary; it is inside 'Ò'").
    #[test]
    fn latin1_line_renders() {
        let src = b"\\catcode `\\\xd2=12 \\lccode `\\\xd2=240\n";
        let d = phitex_tex::Diagnostic {
            file: "./k7in.tex".to_owned(),
            range: (10, 10),
            line: 1,
            severity: phitex_tex::Severity::Error,
            message: "Undefined control sequence".to_owned(),
            context: String::new(),
            help: None,
        };
        let p = from_diagnostic(&d, Some(src), false);
        assert_eq!(p.text, "\\catcode `\\^^d2=12 \\lccode `\\^^d2=240");
        assert_eq!(&p.text[p.start..p.end], "\\^^d2");
        let r = render(&p, false);
        assert!(r.contains("k7in.tex:1"), "{r}");
    }

    /// Underlines on whole characters, whatever the offsets say: inside a
    /// two-byte character, empty, at the end.
    #[test]
    fn spans_on_char_boundaries() {
        let t = "\u{d2}\u{d2}\u{d2}\\x";
        assert_eq!(span(t, 1, 1), (0, 2));
        assert_eq!(span(t, 3, 5), (2, 6));
        assert_eq!(span(t, 7, 7), (7, 8));
        assert_eq!(span(t, 8, 8), (7, 8));
        for (a, b) in [(0, 1), (1, 3), (5, 7), (7, 8), (2, 2)] {
            let mut p = Problem {
                message: "x".to_owned(),
                file: Some("u.tex".to_owned()),
                line: Some(1),
                text: t.to_owned(),
                start: a,
                end: b,
                ..Problem::default()
            };
            let _ = render(&p, false);
            p.text = format!("\u{e9}\u{e9} {t}");
            let _ = render(&p, false);
        }
    }

    #[test]
    fn names_the_culprit() {
        let log = "! Undefined control sequence \\foo.\nl.3 a \\foo\n           b\n\n";
        let p = &parse(log, "m.tex")[0];
        assert_eq!(p.file.as_deref(), Some("m.tex"));
        assert_eq!(&p.text[p.start..p.end], "\\foo");
        let r = render(p, false);
        assert!(r.contains("m.tex:3"), "{r}");
        assert!(r.contains("^^^^"), "{r}");
    }

    #[test]
    fn tokens() {
        assert_eq!(last_token("a \\foo"), 4);
        assert_eq!(last_token("a \\$"), 2);
        assert_eq!(last_token("ab"), 1);
        assert_eq!(
            named_cs("Undefined control sequence \\x"),
            Some("\\x".into())
        );
        assert_eq!(named_cs("Missing $ inserted"), None);
        assert_eq!(find_cs("\\abc \\ab", "\\ab"), Some(5));
    }
}
