//! Repros from the `TikZ` manual run (agent tikzman) in the CI image, whose
//! TeX Live has the language collections: its `language.dat` loads the
//! koi8-r files of `ruhyphen` (`catkoi.tex`, `koi2t2a.tex`, `ruhyphal.tex`)
//! while LaTeX's format is built (its 8-bit `\input` bytes:
//! `inputs::input_file_bytes_past_127_are_one_character_each`).

mod common;

use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// An error in the prelude (the format's text) is the prelude's: a
/// diagnostic at its place, not errors placed at the document's first
/// line. (In the CI image the LaTeX format stops after 100 errors in
/// `ruhyphal.tex`; `\documentclass{article}` then showed 2 errors at
/// `p0.tex:1:1` and none of the format's.)
#[test]
fn prelude_errors_are_reported_at_the_prelude() {
    let dir = scratch("tikzman-prelude");
    let prelude = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\\undefinedinprelude\n\\dump\n";
    let doc = "\\message{[doc]}\\end\n";
    let b = common::build(
        common::options("./p.tex", "p", "", &dir),
        Some(prelude),
        doc,
    );
    let ds = b.diagnostics();
    for d in &ds {
        eprintln!("{:?} {}:{}: {}", d.severity, d.file, d.line, d.message);
    }
    assert!(
        ds.iter().any(|d| d.severity == phitex_tex::Severity::Error
            && d.file != "./p.tex"
            && d.message.contains("Undefined control sequence")),
        "no diagnostic at the prelude's undefined control sequence"
    );
    assert!(
        !ds.iter()
            .any(|d| d.severity == phitex_tex::Severity::Error && d.file == "./p.tex"),
        "the prelude's error placed in the document"
    );
}

/// The engine's own diagnostics (an undeclared definition, a hint's
/// regime violated) are not TeX's errors: not in the log, not counted
/// with them (TeX's hundred), and the PDF is written. `envs.tex` (the
/// LaTeX fixtures) had undeclared definitions of `cs:if@minipage`,
/// `param:clubpenalty` and `toks:12` logged as `! ...` errors; the
/// corpus saw clsguide/usrguide end without a PDF on such diagnostics.
#[test]
fn engine_diagnostics_are_not_errors() {
    let dir = common::fixtures().join("latex/pdfdocs");
    let src = common::read(&dir.join("envs.tex"));
    let opts = phitex_tex::Options {
        epoch: Some(0),
        prelude_jobname: "pdflatex".to_owned(),
        ..common::options("./envs.tex", "envs", "", &dir)
    };
    let b = common::build(opts, Some("\\input pdflatex.ini\n"), &src);
    let (pdf, log) = b.output();
    let ds = b.diagnostics();
    for d in ds
        .iter()
        .filter(|d| d.severity == phitex_tex::Severity::Diagnostic)
    {
        eprintln!("diagnostic {}:{}: {}", d.file, d.line, d.message);
        assert!(
            !log.contains(&format!("! {}", d.message)),
            "the engine's diagnostic in the log as TeX's error: {}",
            d.message
        );
    }
    for d in &ds {
        let engine = d.message.starts_with("undeclared definition")
            || d.message.starts_with("hint regime violated");
        assert!(
            !engine || d.severity == phitex_tex::Severity::Diagnostic,
            "an engine check as {:?}: {}",
            d.severity,
            d.message
        );
    }
    // (TeX's own errors in envs.tex, "Float(s) lost" on dev at the time,
    // are another defect's: not asserted here)
    assert!(!pdf.is_empty(), "no PDF");
    eprintln!("{} engine diagnostics", b.engine_diagnostics());
}

/// The other side of the split: an engine failure that means the result
/// is not the document's (here no fixpoint after `max_rounds`) stays an
/// error (the CLI's "failed", exit 1), never a diagnostic.
#[test]
fn engine_failures_stay_errors() {
    let dir = scratch("tikzman-nofixpoint");
    let _ = std::fs::remove_file(dir.join("main.aux"));
    let doc = "\\newread\\r \\openin\\r=\\jobname.aux \\ifeof\\r \\def\\ref{??}\\else\\closein\\r \\input \\jobname.aux \\fi\n\\newwrite\\w \\immediate\\openout\\w=\\jobname.aux \\immediate\\write\\w{\\relax}\n\nRef: \\ref.\\par\n\\write\\w{\\string\\def\\string\\ref{\\if1\\ref 2\\else 1\\fi}}\n\n\\end\n";
    let opts = phitex_tex::Options {
        fixpoint: true,
        max_rounds: 8,
        ..common::options("./main.tex", "main", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), doc);
    let ds = b.diagnostics();
    let nofix: Vec<_> = ds
        .iter()
        .filter(|d| d.message.contains("no fixpoint"))
        .collect();
    assert!(!nofix.is_empty(), "{ds:#?}");
    assert!(
        nofix
            .iter()
            .all(|d| d.severity == phitex_tex::Severity::Error),
        "{nofix:#?}"
    );
    assert_eq!(b.engine_diagnostics(), 0, "{ds:#?}");
}
