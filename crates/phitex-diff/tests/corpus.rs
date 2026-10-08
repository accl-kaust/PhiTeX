//! The corpus of synthetic old/new pairs (`tests/corpus/<case>/{old,new}`),
//! without TeX: every pair diffs; the output's braces balance outside
//! comments and verbatim; every change's ranges are its text in its
//! files; the preamble is the new one's. (`tests/compile.sh` compiles
//! them with pdflatex and compares with latexdiff.)

use phitex_diff::{ChangeKind, Dir, Files, Options, diff};
use phitex_syntax::{Green, SyntaxKind, lex};
use std::path::Path;

fn corpus() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut cases: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    cases.sort();
    cases
}

/// Whether every group of `g` is closed and no `}` is stray.
fn balanced(g: &Green) -> bool {
    g.children().iter().all(|c| match c.kind() {
        SyntaxKind::Other => false,
        SyntaxKind::Group => {
            c.children()
                .last()
                .is_some_and(|l| l.kind() == SyntaxKind::Brace)
                && c.children().len() >= 2
                && balanced(c)
        }
        _ => true,
    })
}

#[test]
fn every_pair() {
    for case in corpus() {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let old = Dir(case.join("old"));
        let new = Dir(case.join("new"));
        let d = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        assert!(balanced(&lex(&d.tex)), "{name}: braces");
        // (the new preamble, then the definitions)
        let main = new.read("main.tex").unwrap();
        let begin = main.find("\\begin{document}").unwrap();
        assert!(
            d.tex.contains("%DIF PREAMBLE EXTENSION ADDED BY LATEXDIFF"),
            "{name}"
        );
        let pre = &main[..begin];
        let first_line = pre.lines().next().unwrap();
        assert!(d.tex.starts_with(first_line), "{name}: preamble");
        assert!(!d.changes.is_empty(), "{name}: no change found");
        for c in &d.changes {
            for (files, loc, text) in [(&old, &c.old, &c.old_text), (&new, &c.new, &c.new_text)] {
                if loc.start == loc.end {
                    continue;
                }
                let src = files.read(&loc.file).unwrap();
                assert_eq!(&src[loc.start..loc.end], text, "{name}: {c:?}");
            }
            match c.kind {
                ChangeKind::Add => assert!(c.old_text.is_empty() && !c.new_text.is_empty()),
                ChangeKind::Del => assert!(!c.old_text.is_empty() && c.new_text.is_empty()),
                ChangeKind::Change => assert!(!c.old_text.is_empty() && !c.new_text.is_empty()),
            }
            assert!(c.out.end <= d.tex.len());
        }
        // (deterministic)
        let again = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        assert_eq!(again.tex, d.tex, "{name}");
    }
}

/// A version diffed against itself has no change, and its markup is its
/// text.
#[test]
fn identity() {
    for case in corpus() {
        let new = Dir(case.join("new"));
        let d = diff(&new, &new, "main.tex", &Options::default()).unwrap();
        assert!(d.changes.is_empty(), "{}", case.display());
        let body = &d.tex[d.tex.find("\\begin{document}").unwrap()..];
        assert!(!body.contains("\\DIF"), "{}", case.display());
    }
}

/// A main file only the new version has: all added.
#[test]
fn new_document() {
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/newdoc");
    let d = diff(
        &Dir(case.join("old")),
        &Dir(case.join("new")),
        "main.tex",
        &Options::default(),
    )
    .unwrap();
    assert!(d.changes.iter().all(|c| c.kind == ChangeKind::Add));
}

/// The multi-file pair: the deleted input's text is deleted where it was
/// input, the new file's added, each change in its own file.
#[test]
fn multifile() {
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/multifile");
    let d = diff(
        &Dir(case.join("old")),
        &Dir(case.join("new")),
        "main.tex",
        &Options::default(),
    )
    .unwrap();
    let files: Vec<(&str, &str)> = d
        .changes
        .iter()
        .map(|c| (c.old.file.as_str(), c.new.file.as_str()))
        .collect();
    assert!(
        files.contains(&("chapters/methods.tex", "chapters/results.tex")),
        "{files:?}"
    );
    assert!(
        files.contains(&("chapters/intro.tex", "chapters/intro.tex")),
        "{files:?}"
    );
    assert!(
        d.tex
            .contains("\\DIFdel{This file is no longer input by the new version}")
    );
    // (the preamble's own input followed, unmarked)
    assert!(d.tex.contains("\\newcommand{\\C}"));
    assert_eq!(d.changes[0].section.as_deref(), Some("Introduction"));
}
