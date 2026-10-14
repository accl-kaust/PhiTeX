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

/// The page writers' counter is the process's: the tests that build
/// relaxed documents take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
#[ignore = "heavy: the LaTeX format; meaningful once exact pdflatex is byte-identical (the prelude does not load latex.ltx to the end yet)"]
fn relaxed_pdflatex() {
    let _turn = turn();
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
    let _turn = turn();
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
    let _turn = turn();
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

/// The font commands in relaxed mode (`\pdffontattr`,
/// `\pdfincludechars`, `\pdffontobjnum`): `misc.tex` without
/// `\pdffontname` (which stays exact) equals the exact build after
/// renumbering.
#[test]
fn relaxed_font_commands() {
    let _turn = turn();
    let dir = pdf_fixtures();
    let src = common::read(&dir.join("misc.tex")).replace(
        "\\pdffontname\\r,\\pdffontobjnum\\r,\\pdffontsize\\r,\\pdffontname\\b,\\pdffontobjnum\\b",
        "\\pdffontobjnum\\r,\\pdffontsize\\r,\\pdffontobjnum\\b",
    );
    assert!(!src.contains("\\pdffontname"));
    let relaxed_b = common::build(relaxed("misc", &dir), None, &src);
    assert_eq!(relaxed_b.pdf_exact_reason(), None, "relaxed mode expected");
    let exact = Options {
        epoch: Some(0),
        ..common::options("./misc.tex", "misc", "", &dir)
    };
    let want = common::build(exact, None, &src).output().0;
    let got = relaxed_b.output().0;
    if let Some(d) = phitex_pdf::norm::first_difference(&got, &want) {
        panic!("misc relaxed: {d}");
    }
}

/// No page step reads another in relaxed mode either: the carry (a
/// colour pushed on one page and popped pages later, a link across a
/// page break, the page numbers) reaches every page from the chain's
/// fold of the steps' own deltas, so a cold build of a dozen plain
/// pages settles in two loop rounds, not one page per round; the file
/// equals the exact build after renumbering; an edit on page 1 re-runs
/// that page's step only.
#[test]
fn relaxed_pages_read_no_other_step() {
    let _turn = turn();
    let dir = pdf_fixtures();
    let mut src = String::from(
        "\\pdfoutput=1 \\pdfcompresslevel=0 \\hsize=6in \\vsize=8.5in \\parindent=20pt\n",
    );
    for k in 0..240 {
        let extra = match k {
            20 => "\\pdfcolorstack0 push{1 0 0 rg}",
            200 => "\\pdfcolorstack0 pop",
            60 => "\\pdfstartlink user{/Subtype/Link/A<</S/URI/URI(x)>>}",
            75 => "\\pdfendlink",
            _ => "",
        };
        let _ = writeln!(
            src,
            "{extra}Paragraph {k} has some words in it, enough to fill a few lines of text set \
             with plain's macros, so that the paragraph builder has work to do and the \
             pages fill up one after another.\n"
        );
    }
    src.push_str("\\bye\n");
    let prelude = common::plain();
    let mut b = common::build(relaxed("carry", &dir), Some(&prelude), &src);
    assert_eq!(b.pdf_exact_reason(), None, "relaxed mode expected");
    let rounds = b.stats.loop_rounds;
    let pages = b.pages().len();
    eprintln!("carry cold: {rounds} loop rounds, {pages} pages");
    assert!(pages >= 8, "{pages} pages");
    let exact = Options {
        epoch: Some(0),
        ..common::options("./carry.tex", "carry", "", &dir)
    };
    let want = common::build(exact, Some(&prelude), &src).output().0;
    if let Some(d) = phitex_pdf::norm::first_difference(&b.output().0, &want) {
        panic!("carry relaxed: {d}");
    }
    assert!(rounds <= 2, "{rounds} loop rounds");
    let text = src.replacen(
        "Paragraph 3 has some words",
        "Paragraph 3 has some wordz",
        1,
    );
    b.edit(&text);
    let steps: u64 = b.stats.page_steps_completed.iter().sum();
    let cold = common::build(relaxed("carry", &dir), Some(&prelude), &text);
    assert_eq!(
        common::first_difference(&b.output().0, &cold.output().0),
        None,
        "the edit differs from the relaxed cold build"
    );
    eprintln!("carry edit: {steps} page steps");
    assert!(steps <= 1, "{steps} page steps re-run");
}
