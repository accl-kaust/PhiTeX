//! A build's errors on the terminal as compilers report theirs
//! (annotate-snippets): the file and line, the line as TeX read it with
//! the culprit underlined, the macros it was in as notes, TeX's help as
//! the help note. Read from the transcript, where TeX lays each error out
//! (`! message`, the context levels two lines each, `l.N` with the line
//! split where it was read, the help to the blank line); phitex-red's
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
                let end = rest.find(|c: char| c.is_whitespace() || c == '(' || c == ')').unwrap_or(rest.len());
                let name = &rest[..end];
                if !name.is_empty() && (name.starts_with('/') || name.starts_with('.') || name.contains('.')) {
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
        rest.chars().next().filter(|c| !c.is_whitespace()).map(|c| format!("\\{c}"))
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
        .filter(|(at, _)| !word || !text[at + cs.len()..].starts_with(|c: char| c.is_ascii_alphabetic()))
        .map(|(at, _)| at)
        .last()
}

/// The place of the error in its line: `read` is what TeX had read of it
/// (a control word printed with a space after it), `rest` what follows.
fn place(p: &mut Problem, read: &str, rest: &str) {
    let read = read.strip_suffix(' ').filter(|r| last_token(r) > 1).unwrap_or(read);
    p.text = format!("{read}{rest}");
    let named = named_cs(&p.message).and_then(|cs| find_cs(&p.text, &cs).map(|at| (at, at + cs.len())));
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

/// The problem as a compiler's report.
pub fn render(p: &Problem, color: bool) -> String {
    let origin = p.file.clone().unwrap_or_default();
    let notes: Vec<String> = p
        .context
        .iter()
        .map(|c| if c == "..." { "more levels above (\\errorcontextlines)".to_owned() } else { format!("in {c}") })
        .collect();
    let help = p.help.join("\n");
    let at = p.line.filter(|_| p.text.is_empty()).map(|n| format!("at {origin}:{n}"));
    let mut m = Level::Error.title(&p.message);
    if let Some(n) = p.line.filter(|_| !p.text.is_empty()) {
        let end = p.end.min(p.text.len()).max(1);
        let start = p.start.min(end - 1);
        m = m.snippet(
            Snippet::source(&p.text)
                .line_start(n)
                .origin(&origin)
                .fold(false)
                .annotation(Level::Error.span(start..end)),
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
    let r = if color { Renderer::styled() } else { Renderer::plain() };
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
        assert_eq!(named_cs("Undefined control sequence \\x"), Some("\\x".into()));
        assert_eq!(named_cs("Missing $ inserted"), None);
        assert_eq!(find_cs("\\abc \\ab", "\\ab"), Some(5));
    }
}
