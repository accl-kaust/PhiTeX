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
