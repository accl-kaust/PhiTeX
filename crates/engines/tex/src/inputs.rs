//! A document's `\input` files as paragraph regions of the document
//! (`docs/TARGET_GRAPH.md`): a slice of a document's text that is
//! exactly `\input name` (the primitive: the format leaves `\input`
//! undefined as a macro) of a document file the finder resolves is
//! replaced, in the graph, by that file's own slices, each a node with
//! its hint, in document order; recursively, to depth 8.
//!
//! The primitive `\input` (§537) only starts reading the file and its
//! end (§362) only stops reading it, so the file's regions in place of
//! the command run as TeX runs them. What `\input` does besides (the
//! log's `(name`, `\inputlineno`, `\endinput`, e-TeX's `\everyeof`) the
//! regions do not, so a file using `\endinput`, `\inputlineno` or
//! `\everyeof`, and every other case, stays as before: one node running
//! `\input`, the file read in its run. Those are: a computed name
//! (`\input\foo`), a call inside a group or an argument (not a slice of
//! its own), LaTeX's `\input{...}` and `\include{...}` (macros: their
//! wrappers' hooks, `\clearpage` and `.aux` handling run around the
//! file in one run), a file not found or not a document's (`.sty`, a
//! file under a TEXMF tree).

use std::sync::Arc;

use phitex_defuse::{ValueId, Values};
use phitex_source::pass::FileKind;

use crate::api::Finder;
use crate::hints::{Defs, TextHints};

/// A node's text: a slice of the document or of a file it inputs.
pub(crate) struct Piece {
    /// Its identity for edits (`api::key`, prefixed by the file).
    pub key: String,
    text: Arc<str>,
    s: usize,
    e: usize,
    /// The file's path (`None`: the document).
    file: Option<Arc<str>>,
    hints: Arc<TextHints>,
}

impl Piece {
    /// The node's arguments: a top-level document node's text with its
    /// origin, and its hint.
    pub(crate) fn args(&self, v: &Values, doc_name: &str) -> Vec<ValueId> {
        let starts = self.s == 0 || self.text.as_bytes()[self.s - 1] == b'\n';
        let name = v.str(self.file.as_deref().unwrap_or(doc_name));
        let mut a =
            crate::run::slice_args_in_file(v, &self.text[self.s..self.e], starts, name, self.s);
        a[1] = v.int(3);
        a.push(crate::hints::hint_arg(v, &self.hints, self.s));
        a
    }
}

/// The file name of a slice that is exactly `\input name` or
/// `\input{name}` (`.tex` added to a name without an extension, §537).
fn target(piece: &str) -> Option<String> {
    let rest = piece.trim().strip_prefix("\\input")?;
    if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '@') {
        return None;
    }
    let rest = rest.trim_start();
    let name = match rest.strip_prefix('{') {
        Some(b) => b.strip_suffix('}')?,
        None => rest,
    };
    if name.is_empty()
        || name.contains(|c: char| {
            c.is_whitespace() || matches!(c, '\\' | '{' | '}' | '%' | '#' | '$' | '&' | '^' | '~')
        })
    {
        return None;
    }
    Some(if name.contains('.') {
        name.to_owned()
    } else {
        format!("{name}.tex")
    })
}

/// Whether a text may have its `\input`s sliced: it does not redefine
/// `\input` itself.
fn plain_input(text: &str) -> bool {
    ![
        "\\def\\input",
        "\\let\\input",
        "\\edef\\input",
        "\\gdef\\input",
        "\\xdef\\input",
    ]
    .iter()
    .any(|d| text.contains(d))
}

/// Whether a found file is a document's, sliceable into regions.
fn document_file(path: &str, text: &str) -> bool {
    !path.contains("texmf")
        && crate::hints::kind_of(text, FileKind::from_path(path)) == FileKind::Document
        && !["\\endinput", "\\inputlineno", "\\everyeof"]
            .iter()
            .any(|c| text.contains(c))
}

/// The nodes' texts of `text` (analysed as `hints`): its slices, an
/// `\input` slice replaced by its file's ([`target`]). `read` gets the
/// names of the files sliced in.
pub(crate) fn pieces(
    v: &Values,
    text: &str,
    hints: &Arc<TextHints>,
    defs: &Defs,
    finder: Option<&Finder>,
    read: &mut Vec<String>,
) -> Vec<Piece> {
    let finder = finder.filter(|_| !defs.is_defined("input") && plain_input(text));
    let mut out = Vec::new();
    let mut seen = Vec::new();
    add(
        v,
        Arc::from(text),
        None,
        hints,
        defs,
        finder,
        0,
        &mut seen,
        read,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn add(
    v: &Values,
    text: Arc<str>,
    file: Option<Arc<str>>,
    hints: &Arc<TextHints>,
    defs: &Defs,
    finder: Option<&Finder>,
    depth: u32,
    seen: &mut Vec<String>,
    read: &mut Vec<String>,
    out: &mut Vec<Piece>,
) {
    for &(s, e) in &hints.slices {
        let piece = &text[s..e];
        if let Some(f) = finder
            && depth < 8
            && let Some(name) = target(piece)
            && !seen.contains(&name)
            && let Some((path, bytes)) = f(&name)
        {
            let t: String = bytes.iter().map(|&b| char::from(b)).collect();
            let t = if t.is_empty() || t.ends_with('\n') {
                t
            } else {
                format!("{t}\n")
            };
            if document_file(&path, &t) && plain_input(&t) {
                let h = crate::hints::text_hints(&t, FileKind::Document, defs);
                crate::hints::register_text(v.str(&path), &h);
                read.push(name.clone());
                seen.push(name);
                add(
                    v,
                    Arc::from(t.as_str()),
                    Some(Arc::from(path.as_str())),
                    &h,
                    defs,
                    Some(f),
                    depth + 1,
                    seen,
                    read,
                    out,
                );
                seen.pop();
                continue;
            }
        }
        let starts = s == 0 || text.as_bytes()[s - 1] == b'\n';
        let key = match &file {
            None => format!("{}{piece}", u8::from(starts)),
            Some(p) => format!("\u{1}{p}\u{1}{}{piece}", u8::from(starts)),
        };
        out.push(Piece {
            key,
            text: text.clone(),
            s,
            e,
            file: file.clone(),
            hints: hints.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::target;

    #[test]
    fn targets() {
        assert_eq!(target("\\input chap1 ").as_deref(), Some("chap1.tex"));
        assert_eq!(target("\\input{a.tex}\n").as_deref(), Some("a.tex"));
        assert_eq!(target("\\input\\foo "), None);
        assert_eq!(target("\\inputlineno"), None);
        assert_eq!(target("\\input a % x\n"), None);
    }
}
