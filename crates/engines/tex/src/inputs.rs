//! A file `\input` opens as a document's regions (`docs/TARGET_GRAPH.md`):
//! whatever route reaches the primitive (`\input name`, LaTeX's
//! `\input{...}`, `\@input`, `\InputIfFileExists`, `\include`, a file
//! the job wrote and reads back), a document run that reaches a document
//! file with nothing of its own open but groups it can leave as frames
//! stops there, and the rest of the file becomes region nodes, one per
//! slice of the hint pass over it (`Run::leftover`,
//! `Run::doc_file_regions`); what the wrapper does after the file follows
//! them. A file of the distribution, or one that ends itself early or
//! reads its own line numbers or `\everyeof` (`\endinput`,
//! `\inputlineno`, `\everyeof`), is read inline by the run.

use phitex_source::pass::FileKind;

/// Whether a found file is a document's, sliceable into regions.
pub(crate) fn document_file(path: &str, text: &str) -> bool {
    !path.contains("texmf")
        && crate::hints::kind_of(text, FileKind::from_path(path)) == FileKind::Document
        && !["\\endinput", "\\inputlineno", "\\everyeof"]
            .iter()
            .any(|c| text.contains(c))
}

