//! The process's table of control sequence names (`val::CsId`): what a
//! long session interns, and what an internal name past its range is.
//! A program of its own: the table is the process's, and the counts are
//! read with no other test's builds beside them (one test at a time,
//! [`ONE`]).

mod common;

use std::path::{Path, PathBuf};

use phitex_tex::Options;

/// The tests one at a time: each reads or fills the process's table.
static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn prelude(_pdf: bool) -> String {
    "\\input pdflatex.ini\n".to_owned()
}

/// pdflatex's options, the format cached under `target/cache` as the
/// latex tests keep it.
fn pdf_opts(name: &str, dir: &Path) -> Options {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/cache/pdflatex.fmt");
    Options {
        epoch: Some(0),
        prelude_jobname: "pdflatex".to_owned(),
        prelude_bytes: std::fs::read(cache).ok(),
        ..common::options(&format!("./{name}.tex"), name, "", dir)
    }
}

/// An internal name met once the process has numbered more internal
/// names than their range holds is still frozen: the end of an alignment
/// template (`\u{10FFFE}endv:<char>`, made where the template's tab is
/// first met) ends the cell. Its frozenness was its number's, and a
/// session that had numbered 32768 internal names first (a symbol per
/// deferred counter read, per build) took the end for an undefined
/// control sequence: "Undefined control sequence", then "Missing \cr
/// inserted" a hundred times and no PDF (#32).
#[test]
fn an_internal_name_numbered_past_its_range_is_still_frozen() {
    let _one = ONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-frozen-past");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for i in 0..40_000 {
        phitex_tex::intern_name(&format!("\u{10FFFE}test-past-range\u{2}{i}"));
    }
    // (`!` a tab: its template end, `endv:33`, first made here)
    let doc = "\\documentclass{article}\n\\begin{document}\n\\catcode`\\!=4\n\\halign{#\\hfil!\\hfil#\\cr a!b\\cr c!d\\cr}\n\\begin{tabular}{cc}e!f\\\\ g!h\\end{tabular}\n\\end{document}\n";
    let b = common::build(pdf_opts("past", &dir), Some(&prelude(true)), doc);
    let errors: Vec<String> = b
        .diagnostics()
        .into_iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(!b.output().0.is_empty(), "no PDF");
}
