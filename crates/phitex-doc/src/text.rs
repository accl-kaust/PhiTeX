//! Text as TeX reads and writes it: a title as `\write` prints it, a key,
//! a comma list, an integer expression, a path.

use phitex_syntax::{Green, SyntaxKind, lex};

/// A token list as TeX's `\write` prints it (tex.web §262, §294): a
/// control word followed by a space, the spaces TeX's input skips gone (a
/// run of spaces is one, those after a control word and at a line's start
/// none, a comment and its line's end nothing), an end of line a space.
#[must_use]
pub fn written(src: &str) -> String {
    // (the common case: nothing to change)
    if !src
        .bytes()
        .any(|c| matches!(c, b'\\' | b'%' | b'\n' | b'\t' | b'\r'))
        && !src.contains("  ")
        && !src.starts_with(' ')
    {
        return src.to_owned();
    }
    let green = lex(src);
    let mut out = String::with_capacity(src.len());
    let mut st = State::Line;
    write_tokens(&green, src, &mut 0, &mut out, &mut st);
    out
}

/// TeX's input states (§303): mid-line, skipping blanks (after a control
/// word or a space), at a line's start.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Mid,
    Skip,
    Line,
}

fn write_tokens(g: &Green, src: &str, at: &mut usize, out: &mut String, st: &mut State) {
    for c in g.children() {
        let t = &src[*at..*at + c.len()];
        match c.kind() {
            SyntaxKind::Group | SyntaxKind::Para => write_tokens(c, src, at, out, st),
            SyntaxKind::Cs => {
                out.push_str(t);
                // (a control word, or a control space, is followed by a
                // space and skips the blanks after it)
                if t[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
                    out.push(' ');
                    *st = State::Skip;
                } else if &t[1..] == " " {
                    *st = State::Skip;
                } else {
                    *st = State::Mid;
                }
            }
            SyntaxKind::Space => {
                if *st == State::Mid {
                    out.push(' ');
                    *st = State::Skip;
                }
            }
            SyntaxKind::Newline => {
                match *st {
                    State::Mid => out.push(' '),
                    State::Line => out.push_str("\\par "),
                    State::Skip => {}
                }
                *st = State::Line;
            }
            SyntaxKind::Comment => {
                // (the line's end after it is not read either)
                *st = State::Skip;
            }
            _ => {
                out.push_str(t);
                *st = State::Mid;
            }
        }
        if !matches!(c.kind(), SyntaxKind::Group | SyntaxKind::Para) {
            *at += c.len();
        }
    }
}

/// A title for display: comments gone, white space one space, trimmed.
#[must_use]
pub fn display(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut space = false;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '%' => {
                // (a comment, and its line's end)
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            ' ' | '\t' | '\n' | '\r' => space = true,
            c => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
    }
    out
}

/// A key as LaTeX makes it (a label's, a reference's): the text as
/// written, its spaces kept (`\label{a b}` and `\ref{a b}` meet,
/// `\ref{ a}` does not).
#[must_use]
pub fn key(src: &str) -> String {
    written(src)
}

/// A comma list's items (citations, cleveref's references, packages),
/// trimmed, the empty ones dropped; commas inside braces do not split.
#[must_use]
pub fn list(src: &str) -> Vec<String> {
    let src = written(src);
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in src.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                push_item(&mut out, &src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    push_item(&mut out, &src[start..]);
    out
}

fn push_item(out: &mut Vec<String>, item: &str) {
    let item = item.trim();
    if !item.is_empty() {
        out.push(item.to_owned());
    }
}

/// An integer as `\setcounter` reads it, if the layer can tell: a number,
/// or `\numexpr` over numbers (`+ - * /` and parentheses, `/` rounding as
/// e-TeX's does).
#[must_use]
pub fn integer(src: &str) -> Option<i64> {
    let w = written(src);
    let mut s = w.trim();
    if let Some(rest) = s.strip_prefix("\\numexpr") {
        s = rest.trim_end();
        s = s.strip_suffix("\\relax").unwrap_or(s);
    }
    let toks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    let mut p = Expr { toks: &toks, i: 0 };
    let v = p.sum()?;
    (p.i == toks.len()).then_some(v)
}

struct Expr<'a> {
    toks: &'a [char],
    i: usize,
}

impl Expr<'_> {
    fn peek(&self) -> Option<char> {
        self.toks.get(self.i).copied()
    }

    fn sum(&mut self) -> Option<i64> {
        let mut v = self.product()?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.i += 1;
            let w = self.product()?;
            v = if op == '+' {
                v.checked_add(w)?
            } else {
                v.checked_sub(w)?
            };
        }
        Some(v)
    }

    fn product(&mut self) -> Option<i64> {
        let mut v = self.atom()?;
        while let Some(op @ ('*' | '/')) = self.peek() {
            self.i += 1;
            let w = self.atom()?;
            v = if op == '*' {
                v.checked_mul(w)?
            } else {
                // (e-TeX rounds a quotient, halves away from zero)
                if w == 0 {
                    return None;
                }
                let q = (2 * v.abs() + w.abs()) / (2 * w.abs());
                if (v < 0) == (w < 0) { q } else { -q }
            };
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<i64> {
        match self.peek()? {
            '-' => {
                self.i += 1;
                self.atom().map(|v| -v)
            }
            '+' => {
                self.i += 1;
                self.atom()
            }
            '(' => {
                self.i += 1;
                let v = self.sum()?;
                (self.peek() == Some(')')).then(|| {
                    self.i += 1;
                    v
                })
            }
            c if c.is_ascii_digit() => {
                let mut v: i64 = 0;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    v = v.checked_mul(10)?.checked_add(i64::from(d))?;
                    self.i += 1;
                }
                Some(v)
            }
            _ => None,
        }
    }
}

/// A path without `.` and `x/..` (relative to the main file's directory,
/// or absolute).
#[must_use]
pub fn normalize(path: &str) -> String {
    let abs = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if abs { format!("/{joined}") } else { joined }
}

/// The directory a path is in (`""` for the main file's).
#[must_use]
pub fn dirname(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// `name` in `dir`.
#[must_use]
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || name.starts_with('/') {
        normalize(name)
    } else {
        normalize(&format!("{dir}/{name}"))
    }
}

/// The files TeX tries for `\input{name}` (web2c's kpathsea: `.tex`
/// appended first unless it is there, then the name as given; suffixes
/// compared with case, as kpathsea compares them).
#[must_use]
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub fn tex_candidates(name: &str) -> Vec<String> {
    let name = normalize(name.trim());
    if name.ends_with(".tex") {
        vec![name]
    } else {
        vec![format!("{name}.tex"), name]
    }
}

/// The graphics extensions pdfTeX's driver tries, in its order
/// (`pdftex.def`'s `\Gin@extensions`).
pub const GRAPHICS_EXTENSIONS: &[&str] = &[
    ".pdf", ".png", ".jpg", ".mps", ".jpeg", ".jbig2", ".jb2", ".PDF", ".PNG", ".JPG", ".JPEG",
    ".JBIG2", ".JB2", ".eps",
];

/// The files `\includegraphics{name}` tries: as named if it has a
/// graphics extension, else with each.
#[must_use]
pub fn graphics_candidates(name: &str) -> Vec<String> {
    let name = name.trim();
    let base = name.rsplit('/').next().unwrap_or(name);
    let has_ext = base
        .rfind('.')
        .is_some_and(|i| GRAPHICS_EXTENSIONS.contains(&&base[i..]));
    if has_ext {
        vec![normalize(name)]
    } else {
        let mut v: Vec<String> = GRAPHICS_EXTENSIONS
            .iter()
            .map(|e| normalize(&format!("{name}{e}")))
            .collect();
        v.push(normalize(name));
        v
    }
}

/// `key=value` in a key-value list (`label=lst:x` in listings' options),
/// braces around the value removed.
#[must_use]
pub fn keyval(src: &str, key: &str) -> Option<String> {
    for item in list(src) {
        if let Some((k, v)) = item.split_once('=')
            && k.trim() == key
        {
            let v = v.trim();
            let v = v
                .strip_prefix('{')
                .and_then(|v| v.strip_suffix('}'))
                .unwrap_or(v);
            return Some(v.trim().to_owned());
        }
    }
    None
}

/// The number of newlines in a text.
#[must_use]
pub fn newlines(s: &str) -> u32 {
    u32::try_from(s.bytes().filter(|&c| c == b'\n').count()).unwrap_or(u32::MAX)
}

/// An integer in Roman numerals, as LaTeX's `\@Roman` writes it.
#[must_use]
pub fn roman(mut n: i64) -> String {
    if n <= 0 {
        return String::new();
    }
    let mut s = String::new();
    for (v, r) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// An integer as a letter, as LaTeX's `\@Alph` writes it (1 is A; out of
/// range, nothing: LaTeX stops with an error).
#[must_use]
pub fn alph(n: i64) -> String {
    match u8::try_from(n) {
        Ok(k @ 1..=26) => char::from(b'A' + k - 1).to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_like_tex() {
        assert_eq!(written("Plain title"), "Plain title");
        assert_eq!(
            written("exact in \\(\\ell_\\infty\\)"),
            "exact in \\(\\ell _\\infty \\)"
        );
        assert_eq!(written("\\S V-A"), "\\S V-A");
        assert_eq!(written("a  b\n  c%x\nd"), "a b cd");
        assert_eq!(written("\\textsf{e}Place"), "\\textsf {e}Place");
        assert_eq!(written("\\tfrac12"), "\\tfrac 12");
    }

    #[test]
    fn display_and_lists() {
        assert_eq!(display("  A\n  title % note\n end "), "A title end");
        assert_eq!(list("a, b,{c,d} ,, e"), ["a", "b", "{c,d}", "e"]);
        assert_eq!(
            keyval("caption={A, b},label=lst:x", "label").as_deref(),
            Some("lst:x")
        );
        assert_eq!(keyval("caption=x", "label"), None);
    }

    #[test]
    fn integers() {
        assert_eq!(integer("14"), Some(14));
        assert_eq!(integer(" -3 "), Some(-3));
        assert_eq!(integer("\\numexpr15-1\\relax"), Some(14));
        assert_eq!(integer("\\numexpr (2+3)*4 - 7/2\\relax"), Some(16));
        assert_eq!(integer("\\value{chapter}"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(normalize("./a/../b/c.tex"), "b/c.tex");
        assert_eq!(normalize("../x"), "../x");
        assert_eq!(join("ch", "../fig/a"), "fig/a");
        assert_eq!(tex_candidates("ch01"), ["ch01.tex", "ch01"]);
        assert_eq!(tex_candidates("figs/a.tex"), ["figs/a.tex"]);
        assert_eq!(graphics_candidates("f/x.jpg"), ["f/x.jpg"]);
        assert_eq!(graphics_candidates("f/x")[0], "f/x.pdf");
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(alph(3), "C");
    }
}
