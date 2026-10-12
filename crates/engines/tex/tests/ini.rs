//! TeX Live's own ini files as the prelude (`tex.ini`, `etex.ini`,
//! `pdfetex.ini`),
//! then `doc.tex` (which `\input`s `part.tex`): the output equals
//! `SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 etex doc.tex` (DVI) and
//! `pdftex doc.tex` (PDF), in `oracle/fixtures/plain/ini`.

mod common;

use phitex_tex::Options;

fn run(ini: &str, jobname: &str, comment: &str) -> Vec<u8> {
    let dir = common::fixtures().join("plain/ini");
    let src = common::read(&dir.join("doc.tex"));
    let opts = Options {
        epoch: Some(0),
        prelude_jobname: jobname.to_owned(),
        ..common::options("./doc.tex", "doc", comment, &dir)
    };
    let b = common::build(opts, Some(&format!("\\input {ini}\n")), &src);
    b.output().0
}

/// Knuth's `tex` (`tex.ini`): its DVI comment keeps the real date, so
/// the comment is the oracle's.
#[test]
fn tex_ini() {
    let dir = common::fixtures().join("plain/ini");
    let want = std::fs::read(dir.join("doc-tex.dvi")).expect("the oracle");
    let got = run("tex.ini", "tex", &common::dvi_comment(&want));
    if let Some(d) = common::first_difference(&got, &want) {
        panic!("tex doc.dvi: {d}");
    }
}

#[test]
fn etex_ini() {
    let dir = common::fixtures().join("plain/ini");
    let want = std::fs::read(dir.join("doc-etex.dvi")).expect("the oracle");
    let got = run("etex.ini", "etex", &common::dvi_comment(&want));
    if let Some(d) = common::first_difference(&got, &want) {
        panic!("etex doc.dvi: {d}");
    }
}

#[test]
fn pdfetex_ini() {
    let dir = common::fixtures().join("plain/ini");
    let want = std::fs::read(dir.join("doc-pdfetex.pdf")).expect("the oracle");
    let got = run("pdfetex.ini", "pdftex", "");
    if let Some(d) = common::first_difference(&got, &want) {
        panic!(
            "pdftex doc.pdf ({} bytes, want {}): {d}",
            got.len(),
            want.len()
        );
    }
}
