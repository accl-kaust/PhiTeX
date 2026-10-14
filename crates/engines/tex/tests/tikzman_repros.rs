//! Repros from the `TikZ` manual run (agent tikzman) in the CI image, whose
//! TeX Live has the language collections: its `language.dat` loads the
//! koi8-r files of `ruhyphen` (`catkoi.tex`, `koi2t2a.tex`, `ruhyphal.tex`)
//! while LaTeX's format is built. Each is `#[ignore]`d with the defect it
//! shows until its fix lands; the expected behaviour is pdfTeX's
//! (`pdftex -ini -etex`).

mod common;

use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The main text as TeX reads it: one char per byte.
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// Bytes >= 0x80 in an `\input` file (koi8-r, as `catkoi.tex` has them):
/// catcode and lccode assignments of the control symbols of bytes 0xC1,
/// 0xC2, 0xD2, and a `\patterns` entry of them. pdfTeX reads one char per
/// byte: no error, no page ("No pages of output."). The same lines in the
/// main file are already right.
#[test]
#[ignore = "8-bit bytes in an \\input file are read as their UTF-8 encoding (\\<0xC1> becomes \\^^c3 ^^81): undefined control sequences, text typeset; breaks LaTeX's format where language.dat loads ruhyphen"]
fn input_file_8bit_bytes_are_one_char_each() {
    let dir = scratch("tikzman-8bit");
    std::fs::write(
        dir.join("k7in.tex"),
        b"\\catcode `\\\xc1=12 \\catcode `\\\xc2=12 \\catcode `\\\xd2=12\n\
          \\lccode `\\\xc1=224 \\lccode `\\\xc2=225 \\lccode `\\\xd2=240\n\
          \\patterns{\n.\xc1\xc21\xd2\n}\n",
    )
    .unwrap();
    let doc = latin1(
        b"\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\\input k7in\n\
          \\message{[\\the\\catcode\"C1/\\the\\lccode\"D2]}\n\\end\n",
    );
    let b = common::build(common::options("./k7.tex", "k7", "", &dir), None, &doc);
    let log = b.output().1;
    assert!(
        !log.contains("\n! "),
        "errors where pdfTeX has none:\n{log}"
    );
    assert!(
        log.contains("[12/240]"),
        "catcode/lccode of the bytes:\n{log}"
    );
    assert!(
        b.pages().is_empty(),
        "pages shipped where pdfTeX has none:\n{log}"
    );
}

/// An error in the prelude (the format's text) is the prelude's: a
/// diagnostic at its place, not errors placed at the document's first
/// line. (In the CI image the LaTeX format stops after 100 errors in
/// `ruhyphal.tex`; `\documentclass{article}` then showed 2 errors at
/// `p0.tex:1:1` and none of the format's.)
#[test]
#[ignore = "prelude errors are hidden: a broken format shows up only as errors placed at the document's line 1"]
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
