//! The hint check mode (the user's invariant: a wrong hint never changes
//! the output). A node's hint, read from its text by the syntactic pass,
//! only decides what waits and what is predicted; every prediction is
//! checked when the node runs. So a build with every hint `Any`
//! (`PHITEX_HINTS=any`) and one with every hint wrong on purpose
//! (`PHITEX_HINTS=adversarial`: every node and output routine bounded to
//! define nothing, open no scope and claim no sibling) make the PDF of
//! the normal build, byte for byte. A difference is a hint trusted
//! without a check. Built at four threads, each mode with a cache of its
//! own (a cold build): amsldoc, the huaz repro (`word_across_cut`) and
//! ten TeX Live manuals identical to the oracle (where TeX Live's
//! documentation is installed, as in the CI image).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A document: its directory to copy, its main file and engine.
struct Doc {
    name: &'static str,
    dir: PathBuf,
    main: &'static str,
    engine: &'static str,
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn docs() -> Vec<Doc> {
    let mut out = vec![
        Doc {
            name: "amsldoc",
            dir: repo().join("bench/amsldoc"),
            main: "amsldoc.tex",
            engine: "pdflatex",
        },
        Doc {
            name: "word_across_cut",
            dir: repo().join("oracle/fixtures/corpus-repros"),
            main: "word_across_cut.tex",
            engine: "pdflatex",
        },
    ];
    let texlive = Path::new("/usr/share/doc/texlive");
    for (name, dir, main, engine) in [
        ("url", "latex/url", "url.tex", "pdflatex"),
        ("xurl", "latex/xurl", "xurl.tex", "xelatex"),
        ("geometry-samples", "latex/geometry", "geometry-samples.tex", "pdflatex"),
        ("checklistings", "latex/checklistings", "example.tex", "pdflatex"),
        ("verbatimbox", "latex/verbatimbox", "verbatimbox.tex", "pdflatex"),
        ("verbatimcopy", "latex/verbatimcopy", "verbatimcopy.tex", "pdflatex"),
        ("mcaption", "latex/mcaption", "example.tex", "pdflatex"),
        ("enumitem", "latex/enumitem", "enumitem.tex", "pdflatex"),
        ("iftex", "generic/iftex", "iftex.tex", "xelatex"),
        ("bredzenie", "latex/bredzenie", "bredzenie.tex", "xelatex"),
    ] {
        let d = texlive.join(dir);
        if d.join(main).is_file() {
            out.push(Doc {
                name,
                dir: d,
                main,
                engine,
            });
        } else {
            eprintln!("{name}: {} not installed, not built", d.display());
        }
    }
    out
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a scratch directory");
    for e in std::fs::read_dir(from).expect("the document's directory").flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).expect("a file of the document");
        }
    }
}

/// `doc` built cold at four threads with `PHITEX_HINTS` set to `mode`
/// (empty: unset): its PDF, and the terminal's last line.
fn build(doc: &Doc, mode: &str) -> (Vec<u8>, String) {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hints-check");
    let label = if mode.is_empty() { "normal" } else { mode };
    let dir = tmp.join(format!("{}-{label}", doc.name));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&doc.dir, &dir);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phitex"));
    cmd.args([
        "build",
        "--exact-pdf",
        "-j",
        "4",
        "--no-synctex",
        "--engine",
        doc.engine,
        doc.main,
    ])
    .current_dir(&dir)
    .env("PHITEX_CACHE_DIR", tmp.join(format!("cache-{label}")))
    .env("SOURCE_DATE_EPOCH", "1758800000")
    .env("FORCE_SOURCE_DATE", "1")
    .env("TZ", "UTC")
    .env("NO_COLOR", "1")
    .env_remove("PHITEX_COMPAT")
    .env_remove("PHITEX_HINTS");
    if !mode.is_empty() {
        cmd.env("PHITEX_HINTS", mode);
    }
    let out = cmd.output().expect("phitex runs");
    let pdf = std::fs::read(dir.join(doc.main.replace(".tex", ".pdf"))).unwrap_or_default();
    let term = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (pdf, term.lines().last().unwrap_or("").to_owned())
}

#[test]
fn a_wrong_hint_never_changes_the_output() {
    let mut bad = Vec::new();
    for doc in docs() {
        let (want, term) = build(&doc, "");
        assert!(!want.is_empty(), "{}: the normal build made no PDF: {term}", doc.name);
        for mode in ["any", "adversarial"] {
            let (got, term) = build(&doc, mode);
            if got != want {
                let n = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
                bad.push(format!(
                    "{} PHITEX_HINTS={mode}: the PDF differs from the normal build's at byte {n} ({} vs {} bytes): {term}",
                    doc.name,
                    got.len(),
                    want.len()
                ));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
