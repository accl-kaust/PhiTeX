//! Gates of the relaxed PDF mode (`crates/output/pdf/RELAXED.md`):
//! the LaTeX oracles after renumbering (gate 2; the PDF fixtures' part
//! is `pdftex.rs`'s `relaxed_documents`), edits against the relaxed
//! cold build with the page writers counted (gate 3), the same file at
//! 1, 4 and 16 threads (gate 4).
//!
//! The LaTeX ones are heavy (the format): ignored by default; run with
//! `--ignored`, `PHITEX_GATE=name` for one document.

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use phitex_tex::Options;

fn pdf_fixtures() -> PathBuf {
    common::fixtures().join("pdf")
}

fn relaxed(name: &str, dir: &Path) -> Options {
    Options {
        epoch: Some(0),
        exact_objects: false,
        ..common::options(&format!("./{name}.tex"), name, "", dir)
    }
}

/// Gate 2 on the LaTeX oracles: `pdflatex` in relaxed mode equals
/// pdfTeX's file after renumbering; a document built in exact mode
/// instead says why.
#[test]
#[ignore = "heavy: the LaTeX format"]
fn relaxed_pdflatex() {
    let oracles = common::fixtures().join("latex/pdf");
    let docs = common::fixtures().join("latex/docs");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut ns: Vec<String> = std::fs::read_dir(&oracles)
        .expect("the oracles")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".pdf").map(str::to_owned)
        })
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    ns.sort();
    let mut failed = Vec::new();
    for name in ns {
        let dir = if docs.join(format!("{name}.tex")).exists() {
            docs.clone()
        } else {
            common::fixtures().join("latex/pdfdocs")
        };
        let want = std::fs::read(oracles.join(format!("{name}.pdf"))).expect("the oracle");
        let src = common::read(&dir.join(format!("{name}.tex")));
        let opts = Options {
            prelude_jobname: "pdflatex".to_owned(),
            ..relaxed(&name, &dir)
        };
        let b = common::build(opts, Some("\\input pdflatex.ini\n"), &src);
        let (pdf, _) = b.output();
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}.relaxed.pdf"), &pdf);
        }
        let mode = b
            .pdf_exact_reason()
            .map_or("relaxed".to_owned(), |w| format!("exact ({w})"));
        match phitex_pdf::norm::first_difference(&pdf, &want) {
            None => eprintln!("ok {name} [{mode}]: equal after renumbering"),
            Some(d) => {
                eprintln!("FAIL {name} [{mode}]: {d}");
                failed.push(name);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// Gate 3: an edit on one page re-runs that page's writer only, and the
/// file equals the relaxed cold build byte for byte.
#[test]
fn relaxed_page_edit() {
    let dir = pdf_fixtures();
    let mut pages = String::new();
    for k in 1..=6 {
        let _ = writeln!(pages, "Page {k} text.\\par\\vfill\\penalty-10000");
    }
    let src = format!(
        "\\pdfoutput=1 \\pdfcompresslevel=0 \\catcode`\\{{=1 \\catcode`\\}}=2 \
         \\font\\tenrm=cmr10 \\tenrm \\hsize=300pt \\vsize=400pt \\parfillskip=0pt plus 1fil\n\
         \\output={{\\shipout\\box255}}\n{pages}\\end\n"
    );
    let mut b = common::build(relaxed("edit", &dir), None, &src);
    assert_eq!(b.pdf_exact_reason(), None, "relaxed mode expected");
    let text = src.replacen("Page 3 text", "Page 3 txet", 1);
    let before = phitex_tex::pdf_pages_written();
    b.edit(&text);
    let written = phitex_tex::pdf_pages_written() - before;
    let cold = common::build(relaxed("edit", &dir), None, &text);
    assert_eq!(
        common::first_difference(&b.output().0, &cold.output().0),
        None,
        "the edit differs from the relaxed cold build"
    );
    eprintln!("page writers run by the edit: {written}");
    assert_eq!(written, 1, "the edit re-ran {written} page writers");
}

/// Gate 4: the relaxed file is the same at 1, 4 and 16 threads, on
/// every PDF fixture.
#[test]
fn relaxed_threads() {
    let dir = pdf_fixtures();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the fixtures")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".tex").map(str::to_owned)
        })
        .filter(|n| dir.join(format!("{n}.pdf")).exists())
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for name in names {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let run = |t: usize| {
            let opts = Options {
                threads: Some(t),
                ..relaxed(&name, &dir)
            };
            common::build(opts, None, &src).output().0
        };
        let one = run(1);
        for t in [4, 16] {
            if let Some(d) = common::first_difference(&run(t), &one) {
                eprintln!("FAIL {name} at {t} threads: {d}");
                failed.push(format!("{name}@{t}"));
            }
        }
    }
    assert!(failed.is_empty(), "differ across threads: {failed:?}");
}
