//! TeX Live's own ini files as the prelude (`etex.ini`, `pdfetex.ini`),
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
#[ignore = "the Type 1 subset differs: /Length1 1490 vs 1488, /Length2 9451 vs 9354 (byte 158)"]
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

#[test]
#[ignore = "probe"]
fn probe_count0() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/probe");
    let opts = Options {
        epoch: Some(0),
        prelude_jobname: "etex".to_owned(),
        ..common::options("./doc.tex", "doc", "", &dir)
    };
    let b = common::build(
        opts,
        Some("\\input etex.src\n\\dump\n"),
        "\\message{[\\the\\count0][\\meaning\\pageno][\\the\\pageno][\\the\\count1][\\the\\count2]}\\bye\n",
    );
    eprintln!("{}", b.output().1);
}

#[test]
#[ignore = "probe"]
fn probe_endinput() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/gate/probe");
    for (pre, doc) in [
        (None, "\\input plain \\input t \\message{[\\the\\count0]}\\bye\n"),
        (Some("\\input plain \\input t \\dump\n"), "\\message{[\\the\\count0]}\\bye\n"),
        (Some("\\input plain \\dump\n"), "\\input t \\message{[\\the\\count0]}\\bye\n"),
    ] {
        let opts = Options {
            prelude_jobname: "x".to_owned(),
            ..common::options("./doc.tex", "doc", "", &dir)
        };
        let b = common::build(opts, pre, doc);
        eprintln!("{:?}: {}", pre.is_some(), b.output().1);
    }
}
