//! Gates for pdfTeX's primitives: the expandable commands and random
//! numbers under INITEX (`oracle/fixtures/pdf/pdf*.tex` with `.log`
//! oracles, run as `core.tex`), and what `\shipout` resolves of
//! `\pdfsavepos` (`savepos.tex`: the `\write` stream equals pdfTeX's
//! `savepos.pos`).
//!
//! The `.pdf` oracles need a PDF output, which this crate does not have
//! yet (it writes DVI): their byte comparison is the backend's gap.

mod common;

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn log_gate(n: &str) -> Result<(), String> {
    let dir = common::fixtures().join("pdf");
    let src = common::read(&dir.join(format!("{n}.tex")));
    let want = common::read(&dir.join(format!("{n}.log")));
    let want = want.trim_end_matches('\n');
    let doc = format!("{PRELUDE}{src}\\end\n");
    let opts = common::options("./core.tex", "texput", "", &dir);
    let b = common::build(opts, None, &doc);
    let (_, log) = b.output();
    let log = log.trim_end_matches('\n');
    match common::first_difference(log.as_bytes(), want.as_bytes()) {
        None => Ok(()),
        Some(d) => Err(format!("pdf/{n}: FAIL {d}")),
    }
}

#[test]
fn expandable() {
    log_gate("pdfexpand").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn random() {
    log_gate("pdfrandom").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn savepos() {
    let dir = common::fixtures().join("pdf");
    let src = common::read(&dir.join("savepos.tex"));
    let want = common::read(&dir.join("savepos.pos"));
    let opts = common::options("./savepos.tex", "savepos", "", &dir);
    let b = common::build(opts, None, &src);
    let mut got = String::new();
    for (_, t) in b.writes().into_iter().filter(|(k, _)| *k == 1) {
        got.push_str(&t);
        got.push('\n');
    }
    if let Some(d) = common::first_difference(got.as_bytes(), want.as_bytes()) {
        panic!("pdf/savepos.pos: FAIL {d}");
    }
}
