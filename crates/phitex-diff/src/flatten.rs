//! `\input` and `\include` followed and flattened into one text (as
//! latexdiff `--flatten`), with a map from the flat text back to each
//! file's bytes.

use crate::Files;
use phitex_syntax::{Green, SyntaxKind, lex};

/// A run of the flat text that came from one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    /// Where it is in the flat text.
    pub flat: usize,
    pub len: usize,
    /// The file (an index into [`Flat::files`]) and where it is there.
    pub file: usize,
    pub src: usize,
}

/// A flattened document.
#[derive(Clone, Debug, Default)]
pub struct Flat {
    pub text: String,
    /// The files read, the main one first.
    pub files: Vec<String>,
    /// The runs of `text`, in order; bytes between runs (a line end added
    /// after a file without one) come from no file.
    pub segs: Vec<Seg>,
}

impl Flat {
    /// Where flat offset `pos` is: a file and an offset in it. A position
    /// between runs is the end of the run before it.
    #[must_use]
    pub fn locate(&self, pos: usize) -> (usize, usize) {
        let i = self.segs.partition_point(|s| s.flat <= pos);
        if i == 0 {
            return (0, pos);
        }
        let s = &self.segs[i - 1];
        (s.file, s.src + (pos - s.flat).min(s.len))
    }

    /// Where flat range `a..b` is: a file and a range in it (cut at the end
    /// of `a`'s file run if it spans files).
    #[must_use]
    pub fn locate_range(&self, a: usize, b: usize) -> (usize, usize, usize) {
        let (file, start) = self.locate(a);
        let i = self.segs.partition_point(|s| s.flat <= a);
        let end = if i == 0 {
            b
        } else {
            let s = &self.segs[i - 1];
            if b <= s.flat + s.len {
                s.src + (b - s.flat)
            } else {
                s.src + s.len
            }
        };
        (file, start, end.max(start))
    }
}

/// How deep `\input` may nest (a cycle stops there).
const MAX_DEPTH: usize = 32;

/// Flatten `main`, read from `files`.
///
/// # Errors
///
/// If `main` cannot be read.
pub fn flatten(files: &dyn Files, main: &str) -> Result<Flat, String> {
    let text = files
        .read(main)
        .ok_or_else(|| format!("can't read {main}"))?;
    let dir = match main.rfind('/') {
        Some(k) => main[..=k].to_owned(),
        None => String::new(),
    };
    let mut f = Flat {
        files: vec![main.to_owned()],
        ..Flat::default()
    };
    let mut stack = vec![main.to_owned()];
    put_file(files, &dir, &text, 0, &mut f, &mut stack);
    Ok(f)
}

/// Append file number `file` (text `text`), following its inputs.
fn put_file(
    files: &dyn Files,
    dir: &str,
    text: &str,
    file: usize,
    f: &mut Flat,
    stack: &mut Vec<String>,
) {
    let mut inputs = Vec::new();
    find_inputs(&lex(text), text, 0, &mut inputs);
    let mut from = 0;
    for inp in inputs {
        if stack.len() >= MAX_DEPTH {
            continue;
        }
        let Some((path, body)) = resolve(files, dir, &inp.name) else {
            continue;
        };
        if stack.contains(&path) {
            continue;
        }
        put_run(f, file, text, from, inp.start);
        let k = f.files.iter().position(|p| *p == path).unwrap_or_else(|| {
            f.files.push(path.clone());
            f.files.len() - 1
        });
        if inp.include {
            f.text.push_str("\\clearpage{}");
        }
        stack.push(path);
        put_file(files, dir, &body, k, f, stack);
        stack.pop();
        // (TeX ends the file's last line, then reads the rest of the
        // `\input` line: a line end there is that line's, so the file's
        // own last one goes, or the two would make a blank line; anything
        // else follows a line end, after a comment that may end the file)
        let rest = &text[inp.end..];
        if rest.starts_with('\n') || rest.starts_with("\r\n") {
            if f.text.ends_with('\n') {
                f.text.pop();
                if let Some(s) = f.segs.last_mut()
                    && s.flat + s.len > f.text.len()
                {
                    s.len -= 1;
                }
            }
        } else if !f.text.ends_with('\n') {
            f.text.push('\n');
        }
        if inp.include {
            f.text.push_str("\\clearpage{}");
        }
        from = inp.end;
    }
    put_run(f, file, text, from, text.len());
}

fn put_run(f: &mut Flat, file: usize, text: &str, a: usize, b: usize) {
    if a >= b {
        return;
    }
    f.segs.push(Seg {
        flat: f.text.len(),
        len: b - a,
        file,
        src: a,
    });
    f.text.push_str(&text[a..b]);
}

/// An `\input{name}` or `\include{name}` at `start..end`.
struct Input {
    start: usize,
    end: usize,
    name: String,
    include: bool,
}

/// The inputs in green node `g` at `at` (comments and verbatim runs are
/// not read: the CST has them as tokens).
#[allow(clippy::many_single_char_names)]
fn find_inputs(g: &Green, text: &str, at: usize, out: &mut Vec<Input>) {
    let kids = g.children();
    let mut pos = at;
    let mut offs = Vec::with_capacity(kids.len());
    for c in kids {
        offs.push(pos);
        pos += c.len();
    }
    let mut i = 0;
    while i < kids.len() {
        let c = &kids[i];
        let o = offs[i];
        if c.kind() == SyntaxKind::Group {
            find_inputs(c, text, o, out);
        } else if c.kind() == SyntaxKind::Cs {
            let cs = &text[o..o + c.len()];
            if matches!(cs, "\\input" | "\\include" | "\\subfile") {
                let mut j = i + 1;
                while j < kids.len() && kids[j].kind() == SyntaxKind::Space {
                    j += 1;
                }
                if j < kids.len() && kids[j].kind() == SyntaxKind::Group {
                    let g = &text[offs[j]..offs[j] + kids[j].len()];
                    let name = g
                        .trim_start_matches('{')
                        .trim_end_matches('}')
                        .trim()
                        .to_owned();
                    out.push(Input {
                        start: o,
                        end: offs[j] + kids[j].len(),
                        name,
                        include: cs == "\\include",
                    });
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
}

/// The file `\input{name}` reads: relative to the main file's directory,
/// then to the project's root; as named, then with `.tex`.
fn resolve(files: &dyn Files, dir: &str, name: &str) -> Option<(String, String)> {
    let mut cands = Vec::new();
    for base in [dir, ""] {
        let p = normalize(&format!("{base}{name}"));
        if !std::path::Path::new(&p)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("tex"))
        {
            cands.push(format!("{p}.tex"));
        }
        cands.push(p);
    }
    cands
        .into_iter()
        .find_map(|p| files.read(&p).map(|t| (p, t)))
}

/// A relative path without `.` and `..` components.
#[must_use]
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn follows_inputs() {
        let mut files = BTreeMap::new();
        files.insert(
            "main.tex".to_owned(),
            "A\\input{a}B % \\input{b}\n\\include{sub/c}".to_owned(),
        );
        files.insert("a.tex".to_owned(), "aa\\input{sub/c.tex}".to_owned());
        files.insert("sub/c.tex".to_owned(), "c\n".to_owned());
        let f = flatten(&files, "main.tex").unwrap();
        assert_eq!(
            f.text,
            "Aaac\nB % \\input{b}\n\\clearpage{}c\n\\clearpage{}"
        );
        assert_eq!(f.files, ["main.tex", "a.tex", "sub/c.tex"]);
        // (the second `a` is a.tex's byte 1; `B` is main's byte 11)
        assert_eq!(f.locate(2), (1, 1));
        let b = f.text.find('B').unwrap();
        assert_eq!(f.locate(b), (0, 10));
        assert_eq!(f.locate_range(b, b + 3), (0, 10, 13));
    }
}
