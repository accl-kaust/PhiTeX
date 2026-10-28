//! A file `\input` opens as a document's regions (`docs/TARGET_GRAPH.md`):
//! whatever route reaches the primitive (`\input name`, LaTeX's
//! `\input{...}`, `\@input`, `\InputIfFileExists`, `\include`, a file
//! the job wrote and reads back), a document run that reaches a document
//! file with nothing of its own open but groups it can leave as frames
//! stops there, and the rest of the file becomes region nodes, one per
//! slice of the hint pass over it (`Run::leftover`,
//! `Run::doc_file_regions`); what the wrapper does after the file follows
//! them; every file is sliced the same way, wherever it was found
//! (spec-redesign §3, §4). A file that ends itself early or reads its
//! own line numbers or `\everyeof` (the primitives `\endinput`,
//! `\inputlineno`, `\everyeof`: what a slice cannot do apart from its
//! file's run) is read inline by the run.

use phitex_source::pass::FileKind;

/// Whether a found file is a document's, sliceable into regions.
pub(crate) fn document_file(path: &str, text: &str) -> bool {
    // (a file of the TeX distribution is never the document's text: it is
    // read as one load, in order, as a package is. glyphtounicode.tex, a
    // 300000-line table of \pdfglyphtounicode calls that acmart's
    // microtype setup inputs, looked like text and was cut into 11000
    // pieces run side by side; on a paper whose preamble then failed, each
    // ran 60 times on the broken state: 754035 runs, 9.6 GB resident)
    if distribution_file(path) {
        return false;
    }
    crate::hints::kind_of(text, FileKind::from_path(path)) == FileKind::Document
        && document_text(text)
}

/// Whether `path` is a file of the TeX distribution (under a texmf tree),
/// read as one load however its text looks: the test every route to a
/// file's pieces makes (a file the client cuts, `api::cut_doc_file`; one
/// a run opens with `\input`, `getnext::doc_file`).
pub(crate) fn distribution_file(path: &str) -> bool {
    path.contains("/texmf")
}

/// Whether a text the job wrote can be sliced into regions as a
/// document's: its kind from its text alone, and none of the commands
/// that depend on reading it as one file.
pub(crate) fn document_text(text: &str) -> bool {
    crate::hints::kind_of(text, FileKind::Document) == FileKind::Document
        && !["\\endinput", "\\inputlineno", "\\everyeof"]
            .iter()
            .any(|c| text.contains(c))
}
