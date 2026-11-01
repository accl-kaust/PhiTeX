//! A build's errors on the terminal as compilers report theirs
//! (annotate-snippets): the file and line, the line as TeX read it with
//! the culprit underlined, the macros it was in as notes, TeX's help as
//! the help note. Read from the transcript, where TeX lays each error out
//! (`! message`, the context levels two lines each, `l.N` with the line
//! split where it was read, the help to the blank line); the session's
//! `Output::errors` are the messages, matched to these displays.

use annotate_snippets::{Level, Renderer, Snippet};

use crate::ui::Ui;

pub use phitex_live::problems::*;
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
    // (the engine's failures are shown on their own lines before)
    for e in errors
        .iter()
        .filter(|e| phitex_live::session::transcript_error(e))
        .take(max)
    {
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
    rendered_of(s, &r.diagnostics, warnings, color, verbose)
}

/// [`rendered`] of the diagnostics `diagnostics` of a build of `s`.
pub fn rendered_of(
    s: &crate::session::Session,
    diagnostics: &[phitex_tex::Diagnostic],
    warnings: bool,
    color: bool,
    verbose: bool,
) -> Vec<String> {
    // (the engine's own diagnostics are shown with the warnings: they
    // are not TeX's errors)
    // (each file's source read once, not once a diagnostic: the main
    // text is the whole document)
    let mut sources: std::collections::HashMap<&str, Option<Vec<u8>>> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for d in diagnostics.iter().filter(|d| shown(d, warnings)) {
        let src = sources
            .entry(d.file.as_str())
            .or_insert_with(|| s.source(&d.file));
        out.push(render(&from_diagnostic(d, src.as_deref(), verbose), color));
    }
    out
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
}
