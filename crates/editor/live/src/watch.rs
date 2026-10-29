//! The watch's edits, shared by `phitex watch` and the frontends' wasm
//! core: the editor's buffers ([`Edits`]: the viewer's `set_file` and
//! `edit`, an LSP editor's open documents) given to the session as the
//! files' new versions, a viewer's other asks sorted out, and what a build
//! leaves for the viewers (its diagnostics as the viewer's problems, the
//! PDF it is shown, where the editor's cursor is on the pages). How the
//! asks reach the watch, when it builds and what it prints are the host's
//! (the CLI's `watch.rs` loop, [`crate::document::Document::step`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

use crate::session::{Report, Session};
use crate::tex;
use crate::view::{Ask, Viewer};

/// The editor's buffers, as Unicode text, by file name (from the
/// watch's directory), and the main file's: its name, and its path for
/// the host to read it.
pub struct Edits {
    pub buffers: HashMap<String, String>,
    main: String,
    main_path: PathBuf,
}

/// What an ask did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// File `.0` has a new version (`FILE (editor)`, why the next build
    /// runs).
    Edited(String),
    /// Build again (the viewer's Rebuild).
    Rebuild,
    /// A change reached the screen this long after it was made, in
    /// microseconds.
    Painted(u64),
    /// A diff's ask: the host's (it diffs).
    Diff(Ask),
    /// Nothing to build (the cursor shown, a name that is no file of the
    /// project).
    Nothing,
}

impl Edits {
    /// The buffers of a watch on main file `main` (its name from the
    /// watch's directory) at `main_path`.
    pub fn new(main: &str, main_path: &Path) -> Edits {
        Edits {
            buffers: HashMap::new(),
            main: main.to_owned(),
            main_path: main_path.to_path_buf(),
        }
    }

    /// File `file`'s version from now on given to the session: its
    /// buffer, else as on disk (the host's).
    pub fn set(&self, s: &mut Session, file: &str) {
        let text = self.buffers.get(file).map(|b| tex_text(b));
        if file == self.main {
            // (a main file not there to read, removed and not written
            // again: its last text stands, not an empty one)
            let Some(t) = text.or_else(|| {
                let t0 = Instant::now();
                let b = s.host().read(&self.main_path)?;
                phitex_tex::timeline::note("watch: the main file read", t0.elapsed());
                let t0 = Instant::now();
                let t = tex::chars_owned(b);
                phitex_tex::timeline::note("watch: its characters", t0.elapsed());
                Some(t)
            }) else {
                return;
            };
            let t0 = Instant::now();
            s.set_text(t);
            phitex_tex::timeline::note("watch: the text set (shared)", t0.elapsed());
        } else {
            s.set_buffer(file, text.as_deref());
        }
    }

    /// File `file`'s text now: its buffer, else as on disk (the host's).
    fn text_of(&self, s: &Session, file: &str) -> String {
        self.buffers.get(file).cloned().unwrap_or_else(|| {
            s.host()
                .read(Path::new(file))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        })
    }

    /// Ask `a` applied: a buffer's change given to the session, the
    /// editor's cursor shown by `viewer`, the rest said for the host.
    pub fn apply(&mut self, s: &mut Session, viewer: Option<&Viewer>, a: Ask) -> Applied {
        let file = match a {
            Ask::Painted(us) => return Applied::Painted(us),
            Ask::Rebuild => return Applied::Rebuild,
            Ask::DiffStart { .. } | Ask::DiffShow(_) | Ask::DiffRestyle { .. } | Ask::DiffStop => {
                return Applied::Diff(a);
            }
            Ask::SetFile { file, text } => {
                self.buffers.insert(file.clone(), text);
                file
            }
            Ask::Closed { file } => {
                self.buffers.remove(&file);
                file
            }
            Ask::Cursor { file, line, col } => {
                if let Some(v) = viewer {
                    let text = self.text_of(s, &file);
                    let (found, words) = locate(s, &file, &text, line, col);
                    v.reveal(found, &words, &file, line + 1);
                }
                return Applied::Nothing;
            }
            Ask::Edit {
                file,
                start,
                end,
                text,
            } => {
                let cur = self.text_of(s, &file);
                self.buffers
                    .insert(file.clone(), splice_utf16(&cur, start, end, &text));
                file
            }
        };
        if file.is_empty() || file.starts_with('/') || file.contains("..") {
            self.buffers.remove(&file);
            return Applied::Nothing;
        }
        self.set(s, &file);
        Applied::Edited(file)
    }
}

/// The PDF a viewer is shown of build `r`: its PDF; a DVI's made a PDF
/// by the host (dvipdfmx); a PDF file written as update sections, as the
/// host wrote it (`written`). None: no pages.
///
/// # Errors
/// The DVI's conversion's.
pub fn viewer_pdf(
    s: &Session,
    r: &Report,
    written: impl FnOnce() -> Option<Vec<u8>>,
) -> Result<Option<Arc<[u8]>>, String> {
    Ok(match r.main_output() {
        // (a file written as update sections: as it is)
        None if r.out.layout.is_some() => written().map(|b| crate::session::shared(&b)),
        Some(("pdf", b)) => Some(crate::session::shared(b)),
        Some((_, dvi)) => Some(crate::session::shared(&s.host().dvi_to_pdf(
            dvi,
            &format!("{}.pdf", s.job.jobname),
            s.job.pdf_date,
        )?)),
        None => None,
    })
}

/// `s` with UTF-16 code units `start..end` replaced by `text` (the
/// viewer's offsets).
pub fn splice_utf16(s: &str, start: usize, end: usize, text: &str) -> String {
    let at = |off: usize| {
        let mut u = 0;
        for (i, c) in s.char_indices() {
            if u >= off {
                return i;
            }
            u += c.len_utf16();
        }
        s.len()
    };
    let (a, b) = (at(start), at(end.max(start)));
    format!("{}{text}{}", &s[..a], &s[b..])
}

/// A Unicode text as the bytes of its file, read by TeX as characters.
pub fn tex_text(s: &str) -> String {
    tex::chars(s.as_bytes())
}

/// The diagnostics of a build for the viewer: the engine's, at their
/// places, each a whole problem (the viewer's `Problem`: its line, the
/// culprit underlined, TeX's context and help), else the transcript's.
pub fn diagnostics(s: &Session, r: &Report, log: &str) -> Vec<Value> {
    if r.internal.is_none() && !r.diagnostics.is_empty() {
        return r
            .diagnostics
            .iter()
            .map(|d| {
                let p = crate::problems::from_diagnostic(d, s.source(&d.file).as_deref(), false);
                let chars = |b: usize| p.text.get(..b.min(p.text.len())).map_or(0, |t| t.chars().count());
                let error = d.severity == phitex_tex::Severity::Error;
                let engine = d.severity == phitex_tex::Severity::Diagnostic;
                let excerpt = (!p.text.is_empty()).then(|| {
                    json!({"text": p.text, "start": chars(p.start), "len": chars(p.end).saturating_sub(chars(p.start))})
                });
                json!({
                    "severity": if error { "error" } else if engine { "diagnostic" } else { "warning" },
                    "code": "tex",
                    "message": d.message,
                    "notes": p.label.iter().collect::<Vec<_>>(),
                    "help": d.help.iter().flat_map(|h| h.lines()).collect::<Vec<_>>(),
                    "suggestions": [],
                    "file": p.file,
                    "line": d.line,
                    "col": excerpt.as_ref().map(|_| chars(p.start) + 1),
                    "excerpt": excerpt,
                    "context": [],
                    "included": [],
                    "box": null,
                    "detail": d.context,
                })
            })
            .collect();
    }
    let mut v: Vec<Value> = r
        .out
        .errors
        .iter()
        .map(|e| match &r.internal {
            // (the engine's stop: its sentence, the dump under it)
            Some(d) => json!({"severity": "error", "code": "engine", "message": e, "detail": d}),
            None => {
                json!({"severity": "error", "code": "tex", "message": crate::problems::bare(e)})
            }
        })
        .collect();
    for (k, n, first) in crate::warnings::summarize(&r.log_text()) {
        v.push(json!({
            "severity": "warning",
            "code": format!("{k:?}"),
            "message": format!("{n} × {}", first.first().cloned().unwrap_or_default()),
            "detail": format!("see {log}"),
        }));
    }
    v
}

/// Where the editor's cursor, line `line` and UTF-16 column `col` (from
/// 0) of `text` (file `file`'s buffer), is on the pages: the words around
/// it as a page would show them ([`needles`], longest first), and the
/// page (from 1) and word of the cursor's. The page is the one whose
/// glyphs came from the word's bytes (the engine's glyph origins,
/// `Build::glyphs_of`, docs/note-origins.md), else from its line's; where
/// the build keeps none, the first page whose text holds one of the
/// words (`Build::page_of_text`, a DVI build's: a PDF build's pages leave
/// the page loop as their writers' shapes, so there the browser looks for
/// the words itself, the page's text as the strips read it).
pub fn locate(
    s: &Session,
    file: &str,
    text: &str,
    line: usize,
    col: usize,
) -> (Option<(usize, String)>, Vec<String>) {
    let src = text.lines().nth(line).unwrap_or("");
    let words = needles(src, col);
    // (the line's bytes in the text, and the cursor's: the word of
    // letters around it, else the line)
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let mut units = 0;
    let at = start
        + src
            .char_indices()
            .find(|&(_, c)| {
                units += c.len_utf16();
                units > col
            })
            .map_or(src.len(), |(i, _)| i);
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c >= 0x80;
    let (mut lo, mut hi) = (at, at);
    while lo > start && word(b[lo - 1]) {
        lo -= 1;
    }
    while hi < start + src.len() && word(b[hi]) {
        hi += 1;
    }
    let found = [(lo, hi), (start, start + src.len())]
        .into_iter()
        .filter(|&(a, z)| a < z)
        .find_map(|(a, z)| {
            s.origin_page(file, a, z)
                .map(|k| (k + 1, text[a..z].to_owned()))
        })
        .or_else(|| {
            words
                .iter()
                .find_map(|w| s.page_of_text(w).map(|k| (k + 1, w.clone())))
        });
    (found, words)
}

/// The words around UTF-16 column `col` of source line `line`, as the
/// page would show them, longest first: what forward search looks for on
/// the pages until the engine records origins. The line is cut at its
/// comment; control sequences, math and TeX's specials break it into
/// pieces (their text is not the page's), braces go; in the piece at the
/// cursor (else the nearest), windows of 6 to 2 words holding the word at
/// the cursor, centred first, then that word alone if it is long enough
/// to be telling.
pub fn needles(line: &str, col: usize) -> Vec<String> {
    // (the cursor as a char index)
    let mut at = line.chars().count();
    let mut u = 0;
    for (k, c) in line.chars().enumerate() {
        if u >= col {
            at = k;
            break;
        }
        u += c.len_utf16();
    }
    // (the pieces: (first char index, words with their char indices))
    let cs: Vec<char> = line.chars().collect();
    let mut pieces: Vec<Vec<(usize, String)>> = vec![Vec::new()];
    let mut word = String::new();
    let mut word_at = 0;
    let mut math = false;
    let end_word = |pieces: &mut Vec<Vec<(usize, String)>>, word: &mut String, word_at: usize| {
        if !word.is_empty() {
            pieces
                .last_mut()
                .expect("a piece")
                .push((word_at, std::mem::take(word)));
        }
    };
    let mut k = 0;
    while k < cs.len() {
        let c = cs[k];
        if c == '%' {
            break;
        }
        let brk = |pieces: &mut Vec<Vec<(usize, String)>>| {
            if pieces.last().is_some_and(|p| !p.is_empty()) {
                pieces.push(Vec::new());
            }
        };
        if c == '\\' {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
            k += 1;
            // (a control word and its star, or one control symbol)
            if cs.get(k).is_some_and(char::is_ascii_alphabetic) {
                while cs
                    .get(k)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == '@')
                {
                    k += 1;
                }
                if cs.get(k) == Some(&'*') {
                    k += 1;
                }
            } else {
                k += 1;
            }
            continue;
        }
        if c == '$' {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
            math = !math;
        } else if math || "&#^_[]".contains(c) {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
        } else if c.is_whitespace() || c == '~' {
            end_word(&mut pieces, &mut word, word_at);
        } else if c != '{' && c != '}' {
            if word.is_empty() {
                word_at = k;
            }
            word.push(c);
        }
        k += 1;
    }
    end_word(&mut pieces, &mut word, word_at);
    pieces.retain(|p| !p.is_empty());
    // (the piece at the cursor: the one whose word starts last at or
    // before it, else the first)
    let Some(piece) = pieces
        .iter()
        .rev()
        .find(|p| p[0].0 <= at)
        .or_else(|| pieces.first())
    else {
        return Vec::new();
    };
    let w = piece.iter().rposition(|(s, _)| *s <= at).unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for n in (2..=6).rev().filter(|&n| n <= piece.len()) {
        // (the windows of n words holding word w, centred first)
        let lo = w.saturating_sub(n - 1);
        let hi = w.min(piece.len() - n);
        let mut starts: Vec<usize> = (lo..=hi).collect();
        let mid = w.saturating_sub(n / 2).clamp(lo, hi);
        starts.sort_by_key(|&s| s.abs_diff(mid));
        for s in starts {
            let t = piece[s..s + n]
                .iter()
                .map(|(_, w)| w.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    let alone = &piece[w].1;
    if alone.chars().count() >= 5 && !out.contains(alone) {
        out.push(alone.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_splices() {
        assert_eq!(splice_utf16("héllo", 1, 2, "e"), "hello");
        assert_eq!(splice_utf16("a😀b", 3, 3, "c"), "a😀cb");
        assert_eq!(splice_utf16("ab", 5, 9, "c"), "abc");
    }

    #[test]
    fn words_around_the_cursor() {
        let l = r"Some \emph{quite} long words, here $x^2$ and there % a comment";
        let col = l.find("words").unwrap();
        let n = needles(l, col);
        assert_eq!(n[0], "quite long words, here");
        assert!(n.contains(&"long words,".to_owned()), "{n:?}");
        assert!(
            n.iter().all(|w| !w.contains("comment") && !w.contains('x')),
            "{n:?}"
        );
        let n = needles(l, l.find("there").unwrap());
        assert_eq!(n[0], "and there");
        assert_eq!(n.last().unwrap(), "there");
        assert_eq!(needles("% only a comment", 3), Vec::<String>::new());
    }
}
