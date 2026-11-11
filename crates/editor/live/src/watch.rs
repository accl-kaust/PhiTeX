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
    /// Build again from nothing ([`Ask::Cold`]): the session's build
    /// dropped already.
    Cold,
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

    /// The main file's name (from the project's directory).
    pub fn main(&self) -> &str {
        &self.main
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
            Ask::Transcript => {
                if let Some(v) = viewer {
                    v.set_transcript(&s.transcript());
                }
                return Applied::Nothing;
            }
            // (the pages a source place's lines went to, when asked)
            Ask::PageOf { file, at } => {
                if let Some(v) = viewer {
                    let pages = s.pages_of_source(&file, at);
                    v.put_page_of(file, at, pages);
                }
                return Applied::Nothing;
            }
            // (the citation under a point, when asked: no cards rather
            // than an error, whatever the document holds)
            Ask::Cites { page, x, y } => {
                if let Some(v) = viewer {
                    let cites = s.cites_at(page, f32::from_bits(x), f32::from_bits(y));
                    v.put_cites(page, x, y, json!({ "cites": cites }));
                }
                return Applied::Nothing;
            }
            // (a page's origins, made for that page alone, when asked)
            Ask::Origins(k) => {
                if let Some(v) = viewer {
                    let (files, page) = s.page_origins(k).unwrap_or_default();
                    v.put_origins(k, files, page);
                }
                return Applied::Nothing;
            }
            Ask::Cold => {
                s.forget_build();
                return Applied::Cold;
            }
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
            Ask::Cursor {
                file,
                at,
                word,
                line,
                select,
            } => {
                if let Some(v) = viewer {
                    v.follow(&file, at, word, line);
                    if let Some((a, b)) = select {
                        v.select(&file, a, b);
                    }
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

/// The PDF a viewer is shown, kept as the host keeps a watch's file: the
/// exact file, then an update section after each build whose PDF is a
/// layout ([`Report`]'s `out.layout`), the exact file again when idle
/// ([`ViewFile::compact`]). A build's file is its last one's and the
/// objects that changed: nothing read again from disk, no whole file made
/// again, however long the document.
#[derive(Default)]
pub struct ViewFile {
    /// What the file holds, its bytes, and those bytes as last given out
    /// (none: nothing yet).
    at: Option<(phitex_pdf::Updater, Vec<u8>, Arc<[u8]>)>,
    /// Whether it is the exact file of what it holds.
    exact: bool,
}

impl ViewFile {
    /// The file brought to layout `l`: its bytes (the last ones, shared,
    /// where nothing changed).
    ///
    /// # Errors
    /// The exact file's link's (the first layout's).
    pub fn update(&mut self, l: &phitex_pdf::Layout) -> Result<Arc<[u8]>, String> {
        if let Some((u, bytes, shown)) = self.at.as_mut() {
            if let phitex_pdf::Section::Append(b) = u.section(l) {
                bytes.extend_from_slice(&b);
                *shown = Arc::from(bytes.as_slice());
                self.exact = false;
            }
            return Ok(shown.clone());
        }
        let exact = l.exact().map_err(|e| format!("{e}"))?;
        Ok(self.exact_of(l.clone(), &exact))
    }

    fn exact_of(&mut self, l: phitex_pdf::Layout, exact: &[u8]) -> Arc<[u8]> {
        let shown: Arc<[u8]> = Arc::from(exact);
        self.at = Some((
            phitex_pdf::Updater::after_exact(l, exact),
            exact.to_vec(),
            shown.clone(),
        ));
        self.exact = true;
        shown
    }

    /// The layout whose exact file [`ViewFile::compact`] makes, where the
    /// file is not exact: made (the expensive part, on any thread) with
    /// [`phitex_pdf::Layout::exact`], then taken by `compact`.
    pub fn to_compact(&self) -> Option<phitex_pdf::Layout> {
        match &self.at {
            Some((u, ..)) if !self.exact => Some(u.held().clone()),
            _ => None,
        }
    }

    /// The file made the exact file of what it holds, where it is not:
    /// its bytes then.
    ///
    /// # Errors
    /// The exact file's link's.
    pub fn compact(&mut self) -> Result<Option<Arc<[u8]>>, String> {
        let Some(l) = self.to_compact() else {
            return Ok(None);
        };
        let exact = l.exact().map_err(|e| format!("{e}"))?;
        Ok(Some(self.exact_of(l, &exact)))
    }
}

/// The PDF a viewer is shown of build `r`: its PDF; a DVI's made a PDF
/// by the host (dvipdfmx); a PDF given as a layout, `view`'s file
/// brought to it where there is one, else as the host wrote it
/// (`written`). None: no pages.
///
/// # Errors
/// The DVI's conversion's.
pub fn viewer_pdf(
    s: &Session,
    r: &Report,
    view: Option<&mut ViewFile>,
    written: impl FnOnce() -> Option<Vec<u8>>,
) -> Result<Option<Arc<[u8]>>, String> {
    Ok(match r.main_output() {
        None if let (Some(l), Some(v)) = (&r.out.layout, view) => Some(v.update(l)?),
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
    kinds(items(s, r, log))
}

/// The diagnostics a build left for later ([`Report::diagnostics_pending`])
/// for the viewer, made now ([`Session::diagnostics_now`]): as
/// [`diagnostics`] gives a build's own.
pub fn late_diagnostics(s: &Session, ds: &[phitex_tex::Diagnostic]) -> Vec<Value> {
    kinds(placed(s, ds))
}

/// A build's errors given at once (its diagnostics left for later,
/// [`Report::diagnostics_pending`]) with the last build's other problems,
/// until those are made again: the warnings shown stay while typing.
pub fn with_last_warnings(now: Vec<Value>, last: &[Value]) -> Vec<Value> {
    let mut v: Vec<Value> = now
        .into_iter()
        .filter(|d| d["severity"] == "error")
        .collect();
    v.extend(last.iter().filter(|d| d["severity"] != "error").cloned());
    v
}

/// (each one's kind, the filters' (problems::kind_of): the viewer's and
/// the editor's alike)
fn kinds(mut v: Vec<Value>) -> Vec<Value> {
    for d in &mut v {
        let k = crate::problems::kind_of(
            d["severity"].as_str().unwrap_or(""),
            d["message"].as_str().unwrap_or(""),
        );
        d["kind"] = json!(k);
    }
    v
}

fn items(s: &Session, r: &Report, log: &str) -> Vec<Value> {
    if r.internal.is_none() && !r.diagnostics.is_empty() {
        return placed(s, &r.diagnostics);
    }
    others(r, log)
}

/// The engine's diagnostics at their places, each a whole problem.
fn placed(s: &Session, ds: &[phitex_tex::Diagnostic]) -> Vec<Value> {
    {
        // (each file's text read once, not once a diagnostic: the main
        // file's made again for each of Grail's warnings, 3 ms of every
        // keystroke in the browser's core)
        let mut sources: std::collections::HashMap<&str, Option<Vec<u8>>> =
            std::collections::HashMap::new();
        ds
            .iter()
            .map(|d| {
                let src = sources.entry(d.file.as_str()).or_insert_with(|| s.source(&d.file));
                let p = crate::problems::from_diagnostic(d, src.as_deref(), false);
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
                    // (the culprit's bytes in its file: the viewer's
                    // underline on the pages, `boxesOf`)
                    "range": [d.range.0, d.range.1],
                    "col": excerpt.as_ref().map(|_| chars(p.start) + 1),
                    "excerpt": excerpt,
                    "context": [],
                    "included": [],
                    "box": null,
                    "detail": d.context,
                })
            })
            .collect()
    }
}

/// A build's errors and the transcript's warnings, summarised: where the
/// engine placed none.
fn others(r: &Report, log: &str) -> Vec<Value> {
    let mut v: Vec<Value> = r
        .out
        .errors
        .iter()
        .map(|e| match &r.internal {
            // (the engine's stop: its sentence, the dump under it)
            Some(d) => json!({"severity": "error", "code": "engine", "message": e, "detail": d}),
            // (the format could not be had: the editor is told apart)
            None if r.format_failed => {
                json!({"severity": "error", "code": "format", "message": e})
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_splices() {
        assert_eq!(splice_utf16("héllo", 1, 2, "e"), "hello");
        assert_eq!(splice_utf16("a😀b", 3, 3, "c"), "a😀cb");
        assert_eq!(splice_utf16("ab", 5, 9, "c"), "abc");
    }
}
