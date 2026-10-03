//! Errors as rustc-style snippets (DESIGN.md, "Command line and
//! terminal"), from the engine's structured diagnostics: the source line
//! with a caret under what TeX was reading, the macro backtrace folded to
//! its ends, and TeX's help with `-v`.
//!
//! ```text
//! error[undefined-control-sequence]: Undefined control sequence \fooo
//!   --> paper.tex:12:13
//!    |
//! 12 | Hello \greet world
//!    |       ^^^^^^ did you mean `\foo`?
//!    = in expansion of \greet
//! ```

use std::fmt::Write as _;

use partex_core::diag::{Diagnostic, Frame, FrameKind, Severity};

use crate::render::Style;

/// How many macro levels a folded backtrace shows.
const MACROS_SHOWN: usize = 3;

/// The widest source line shown; longer ones are cut around the caret.
const WIDTH: usize = 100;

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Length in bytes of the last token of `s`: a control word (with the
/// space TeX prints after it), `\x`, or one character.
fn last_token(s: &[u8]) -> usize {
    let t = s.strip_suffix(b" ").unwrap_or(s);
    let n = t.len();
    if n == 0 {
        return 0;
    }
    let mut i = n;
    while i > 0 && t[i - 1].is_ascii_alphabetic() {
        i -= 1;
    }
    if i > 0 && t[i - 1] == b'\\' && i < n {
        return n - i + 1;
    }
    if n >= 2 && t[n - 2] == b'\\' {
        return 2;
    }
    1
}

/// The control sequence an error is about, when its message does not
/// name it: the token TeX had just read in the innermost level.
fn culprit(d: &Diagnostic) -> Option<String> {
    if d.code != "undefined-control-sequence" {
        return None;
    }
    let f = d.frames.first()?;
    let before = f.before.strip_suffix(b" ").unwrap_or(&f.before);
    let tok = &before[before.len() - last_token(before).min(before.len())..];
    tok.starts_with(b"\\").then(|| lossy(tok))
}

/// A file name as the user wrote it.
fn shown(name: &[u8]) -> String {
    let n = lossy(name);
    n.strip_prefix("./").map_or(n.clone(), str::to_owned)
}

/// `line` cut to [`WIDTH`] characters around character column `col`:
/// the text and the column in it.
fn window(line: &str, col: usize) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= WIDTH {
        return (line.to_owned(), col);
    }
    let start = col.saturating_sub(WIDTH / 2);
    let end = (start + WIDTH).min(chars.len());
    let mut s = String::new();
    let mut c = col - start;
    if start > 0 {
        s.push('…');
        c += 1;
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    (s, c)
}

/// What a macro-level frame is called in a backtrace (LaTeX's robust
/// commands' inner macros, `\textbf␣`, as the command).
fn frame_name(f: &Frame) -> Option<String> {
    match &f.kind {
        FrameKind::Macro { name } => {
            let n = lossy(name);
            let n = n.trim_end();
            (!n.is_empty() && n != "\\").then(|| n.to_owned())
        }
        FrameKind::TokenList(k) if *k != "to be read again" && *k != "recently read" => {
            Some(format!("<{k}>"))
        }
        _ => None,
    }
}

/// Error `d` for the terminal; `verbose` unfolds the backtrace and adds
/// TeX's help.
#[must_use]
pub fn error(d: &Diagnostic, s: Style, verbose: u8) -> String {
    let mut out = String::new();
    let sev = match d.severity {
        Severity::Fatal => "fatal",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Error => "error",
    };
    let (mut message, notes) = message_lines(&lossy(&d.message));
    if let Some(cs) = culprit(d) {
        let _ = write!(message, " {cs}");
    }
    let _ = writeln!(
        out,
        "{}: {}",
        s.red(&format!("{sev}[{}]", code(d))),
        s.bold(&message)
    );
    let d = &Diagnostic {
        suggestions: suggestions(d),
        ..d.clone()
    };
    let primary = d
        .frames
        .iter()
        .position(|f| matches!(f.kind, FrameKind::File { .. } | FrameKind::Terminal { .. }));
    let Some(i) = primary else {
        tail(&mut out, "", d, &notes, s, verbose);
        return out;
    };
    let f = &d.frames[i];
    let before = lossy(&f.before);
    let col = before.chars().count();
    let (start, len) = span(d, &before, &d.frames[..i]);
    let label = if let FrameKind::File { name, line } = &f.kind {
        let _ = writeln!(
            out,
            "{} {}:{line}:{}",
            s.blue("  -->"),
            shown(name),
            start + 1
        );
        line.to_string()
    } else {
        let _ = writeln!(out, "{} <terminal>:{}", s.blue("  -->"), start + 1);
        String::from("*")
    };
    let pad = " ".repeat(label.len());
    let bar = s.blue(&format!("{pad} |"));
    let (line, at) = window(&format!("{before}{}", lossy(&f.after)), col);
    // (the span in the window: it ends where TeX was, `at`, unless the
    // culprit is earlier in the line)
    let start = (start + at).saturating_sub(col).max(usize::from(at < col));
    let len = len.min(line.chars().count().saturating_sub(start)).max(1);
    let hint = d
        .suggestions
        .first()
        .map(|x| format!(" did you mean `\\{}`?", lossy(x)))
        .unwrap_or_default();
    let _ = writeln!(out, "{bar}");
    let _ = writeln!(out, "{} {line}", s.blue(&format!("{label} |")));
    let _ = writeln!(
        out,
        "{bar} {}{}",
        " ".repeat(start),
        s.red(&format!("{}{hint}", "^".repeat(len)))
    );
    let eq = s.blue(&format!("{pad} ="));
    backtrace(&mut out, &eq, &d.frames[..i], verbose);
    for g in &d.frames[i + 1..] {
        if let FrameKind::File { name, line } = &g.kind {
            let _ = writeln!(out, "{eq} included from {}:{line}", shown(name));
        }
    }
    tail(&mut out, &pad, d, &notes, s, verbose);
    out
}

/// What the carets mark in `before` (the line as far as TeX read it),
/// in characters: the control sequence an error is about where the line
/// has it, else the call of the macro the error happened in (`macros`,
/// innermost first), else the last token TeX read. Its start and length.
fn span(d: &Diagnostic, before: &str, macros: &[Frame]) -> (usize, usize) {
    let n = before.chars().count();
    // (the last place `needle` begins, as a word: not the start of a
    // longer control word)
    let find = |needle: &str| -> Option<usize> {
        let mut end = before.len();
        while let Some(k) = before[..end].rfind(needle) {
            let after = before[k + needle.len()..].chars().next();
            let word = needle.chars().last().is_some_and(char::is_alphabetic);
            if !(word && after.is_some_and(char::is_alphabetic)) {
                return Some(before[..k].chars().count());
            }
            end = k;
        }
        None
    };
    if let Some(cs) = culprit(d)
        && let Some(k) = find(&cs)
    {
        return (k, cs.chars().count());
    }
    let call = macros.iter().rev().find_map(|f| match &f.kind {
        FrameKind::Macro { name } => {
            let name = lossy(name);
            let name = name.trim_end();
            (name.len() > 1).then(|| name.to_owned())
        }
        _ => None,
    });
    if let Some(k) = call.and_then(|c| find(&c)) {
        return (k, n - k);
    }
    let tok = before
        .get(before.len() - last_token(before.as_bytes())..)
        .map_or(1, |t| t.chars().count().max(1));
    (n.saturating_sub(tok), tok)
}

/// The macro levels of an error (`frames`, innermost first), folded to
/// the document's outermost and innermost unless `verbose`.
fn backtrace(out: &mut String, eq: &str, frames: &[Frame], verbose: u8) {
    // The macro levels, outermost first.
    let mut levels: Vec<(String, &Frame)> = frames
        .iter()
        .rev()
        .filter_map(|g| frame_name(g).map(|n| (n, g)))
        .collect();
    levels.dedup_by(|a, b| a.0 == b.0);
    if verbose > 0 {
        for (name, g) in levels.iter().rev() {
            let _ = writeln!(
                out,
                "{eq} in {name}: `{}` | `{}`",
                lossy(&g.before),
                lossy(&g.after)
            );
        }
    } else {
        // (folded: the document's macros, not a format's internals)
        let names: Vec<&str> = levels
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !n.contains('@') && !n.starts_with("\\Generic"))
            .collect();
        let hidden = levels.len() - names.len().min(MACROS_SHOWN);
        let chain = if names.len() <= MACROS_SHOWN {
            names.join(" -> ")
        } else {
            format!(
                "{} -> … -> {}",
                names[0],
                names[names.len() - (MACROS_SHOWN - 1)..].join(" -> ")
            )
        };
        let more = if hidden > 0 {
            format!(" ({hidden} frames hidden, -v to show)")
        } else {
            String::new()
        };
        if !names.is_empty() {
            let _ = writeln!(out, "{eq} in expansion of {chain}{more}");
        } else if hidden > 0 {
            let _ = writeln!(out, "{eq} in macros{more}");
        }
    }
}

/// The code shown: LaTeX's `\errmessage`s get their own.
fn code(d: &Diagnostic) -> &'static str {
    let m = &d.message;
    if d.code != "tex-error" {
        d.code
    } else if m.starts_with(b"LaTeX Error:") {
        "latex-error"
    } else if m.starts_with(b"Package ") && m.windows(7).any(|w| w == b" Error:") {
        "package-error"
    } else if m.starts_with(b"Class ") && m.windows(7).any(|w| w == b" Error:") {
        "class-error"
    } else {
        d.code
    }
}

/// A message's first line, and the rest as notes, without LaTeX's
/// advice about its manual and the `H` command (for TeX's prompt).
fn message_lines(m: &str) -> (String, Vec<Vec<u8>>) {
    let mut lines = m.split('\n');
    let first = lines.next().unwrap_or("").trim_end().to_owned();
    let notes = lines
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("See the LaTeX manual")
                && !l.starts_with("See the ")
                && !l.starts_with("Type  H <return>")
                && !l.starts_with("Try typing  <return>")
        })
        .map(|l| l.as_bytes().to_vec())
        .collect();
    (first, notes)
}

/// The suggestions, each once (a robust command and its inner macro
/// are one).
fn suggestions(d: &Diagnostic) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for x in &d.suggestions {
        let t = x.strip_suffix(b" ").unwrap_or(x).to_vec();
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The rest of the message, TeX's help (with `-v`) and further
/// suggestions.
fn tail(out: &mut String, pad: &str, d: &Diagnostic, notes: &[Vec<u8>], s: Style, verbose: u8) {
    let eq = s.blue(&format!("{pad} ="));
    for n in notes {
        let _ = writeln!(out, "{eq} {}", lossy(n));
    }
    if verbose > 0 {
        for (i, h) in d.help.iter().enumerate() {
            let lead = if i == 0 { "help:" } else { "     " };
            let _ = writeln!(out, "{eq} {lead} {}", lossy(h));
        }
    }
    for x in d.suggestions.iter().skip(1) {
        let _ = writeln!(out, "{eq} or `\\{}`?", lossy(x));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: FrameKind, before: &str, after: &str) -> Frame {
        Frame {
            kind,
            before: before.as_bytes().to_vec(),
            after: after.as_bytes().to_vec(),
        }
    }

    fn macro_frame(name: &str, before: &str) -> Frame {
        frame(
            FrameKind::Macro {
                name: name.as_bytes().to_vec(),
            },
            before,
            "",
        )
    }

    /// The error of `diag.rs`'s test, as the terminal shows it.
    fn undefined() -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code: "undefined-control-sequence",
            message: b"Undefined control sequence".to_vec(),
            help: vec![b"The control sequence at the end of the top line".to_vec()],
            frames: vec![
                macro_frame("\\greet", "->\\fooo "),
                frame(
                    FrameKind::File {
                        name: b"./paper.tex".to_vec(),
                        line: 12,
                    },
                    "Hello \\greet",
                    " world",
                ),
            ],
            suggestions: vec![b"foo".to_vec()],
            boxed: None,
        }
    }

    #[test]
    fn rustc_style() {
        assert_eq!(
            error(&undefined(), Style::plain(), 0),
            "error[undefined-control-sequence]: Undefined control sequence \\fooo\n\
             \x20 --> paper.tex:12:7\n\
             \x20  |\n\
             12 | Hello \\greet world\n\
             \x20  |       ^^^^^^ did you mean `\\foo`?\n\
             \x20  = in expansion of \\greet\n"
        );
        let v = error(&undefined(), Style::plain(), 1);
        assert!(v.contains("   = in \\greet: `->\\fooo ` | ``\n"), "{v}");
        assert!(v.contains("   = help: The control sequence at the end of the top line\n"));
    }

    #[test]
    fn the_backtrace_folds() {
        let mut d = undefined();
        d.frames.splice(
            0..1,
            [
                macro_frame("\\e", "->\\fooo "),
                macro_frame("\\d", "->\\e "),
                macro_frame("\\c", "->\\d "),
                macro_frame("\\b", "->\\c "),
                macro_frame("\\a", "->\\b "),
            ],
        );
        let text = error(&d, Style::plain(), 0);
        assert!(
            text.contains(
                "   = in expansion of \\a -> … -> \\d -> \\e (2 frames hidden, -v to show)\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn the_carets_mark_the_culprit_or_the_call() {
        let file = |before: &str, after: &str| {
            frame(
                FrameKind::File {
                    name: b"./err.tex".to_vec(),
                    line: 7,
                },
                before,
                after,
            )
        };
        // (the culprit in the line, inside an argument: it is marked)
        let mut d = undefined();
        d.suggestions.clear();
        d.frames = vec![
            frame(
                FrameKind::TokenList("argument"),
                "undefined \\badmacro",
                " here",
            ),
            macro_frame("\\textbf ", "->\\protect \\textbf  "),
            file(
                "\\greet{world}, and an \\textbf{undefined \\badmacro here}",
                ".",
            ),
        ];
        let text = error(&d, Style::plain(), 0);
        assert!(text.contains("  --> err.tex:7:41\n"), "{text}");
        assert!(
            text.contains("  |                                         ^^^^^^^^^\n"),
            "{text}"
        );
        // (not in the line: the call it happened in)
        d.frames = vec![
            macro_frame("\\greet", "->Hello \\fooo"),
            file("\\greet{world}", ", and"),
        ];
        let text = error(&d, Style::plain(), 0);
        assert!(text.contains("  --> err.tex:7:1\n"), "{text}");
        assert!(text.contains("  | ^^^^^^^^^^^^^\n"), "{text}");
        // (a longer control word is not the culprit's place)
        d.frames = vec![
            macro_frame("\\x", "->\\badmacro "),
            file("\\badmacros \\x", ""),
        ];
        let text = error(&d, Style::plain(), 0);
        assert!(text.contains("  --> err.tex:7:12\n"), "{text}");
    }

    #[test]
    fn long_lines_are_cut_around_the_caret() {
        let line = "x".repeat(300);
        let (w, c) = window(&line, 150);
        assert_eq!(w.chars().count(), WIDTH + 2);
        assert_eq!(c, WIDTH / 2 + 1);
    }
}
