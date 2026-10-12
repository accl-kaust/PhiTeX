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
    let log = common::log_excerpt(&log);
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

/// The PDF documents with links, annotations, objects, forms or images:
/// what the writer does not have yet (PDF-BACKEND-GAP).
const BACKEND_GAPS: &[&str] = &[
    "annot",
    "bera",
    "jpgres",
    "obj",
    "outline",
    "readback",
    "rerun",
    "thread",
    "vf",
    "xform",
    "ximage",
    "ximage2",
    "trailerid",
];

/// The `stream ... endstream` bodies of a PDF file.
fn streams(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = find(&b[i..], b"stream\n") {
        let s = i + k + 7;
        let Some(e) = find(&b[s..], b"endstream") else {
            break;
        };
        out.push(b[s..s + e].to_vec());
        i = s + e + 9;
    }
    out
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// The documents that do not pass yet: un-ignored as they are fixed.
/// The documents not yet byte-identical: berasizes (virtual fonts) and
/// pageref (object order of `\pdfpageref`: the writer as a fold); the
/// rest use the writer's slice-2 methods (links, outlines, objects,
/// forms, images, object streams) at call sites phitex-tex has not wired
/// yet.
const PENDING: &[&str] = &[
    "berasizes",
    "pageref",
    "csform",
    "jpeg",
    "linkbreak",
    "objstm2",
    "outlinetree",
    "ownpdf",
    "ownpdf0",
    "png",
    "png8",
];

/// Gate 1: `pdftex -ini` on `oracle/fixtures/pdf/*.tex` with a `.pdf`
/// oracle: byte-identical; for the documents on a backend gap, whether
/// each of the oracle's streams (pages, fonts) is ours.
#[test]
fn pdf_documents() {
    pdf_gate(false);
}

#[test]
#[ignore = "berasizes: virtual fonts; pageref: object order (the writer as a fold); nine documents on unwired writer call sites"]
fn pdf_documents_pending() {
    pdf_gate(true);
}

fn pdf_gate(pending: bool) {
    let dir = common::fixtures().join("pdf");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("the fixtures")
        .flatten()
        .filter_map(|e| {
            let f = e.file_name().to_string_lossy().into_owned();
            f.strip_suffix(".tex").map(str::to_owned)
        })
        .filter(|n| dir.join(format!("{n}.pdf")).exists())
        .filter(|n| PENDING.contains(&n.as_str()) == pending)
        .filter(|n| only.as_ref().is_none_or(|o| o == n))
        .collect();
    names.sort();
    let mut failed = Vec::new();
    for name in names {
        let src = common::read(&dir.join(format!("{name}.tex")));
        let want = std::fs::read(dir.join(format!("{name}.pdf"))).expect("the oracle");
        let opts = phitex_tex::Options {
            epoch: Some(0),
            ..common::options(&format!("./{name}.tex"), &name, "", &dir)
        };
        let b = common::build(opts, None, &src);
        let (pdf, _) = b.output();
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}.pdf"), &pdf);
        }
        match common::first_difference(&pdf, &want) {
            None => eprintln!("ok {name}: byte-identical"),
            Some(d) => {
                let gap = BACKEND_GAPS.contains(&name.as_str());
                let ours = streams(&pdf);
                let theirs = streams(&want);
                let missing = theirs.iter().filter(|s| !ours.contains(s)).count();
                eprintln!(
                    "{} {name}: {d}\n  {missing} of the oracle's {} streams not ours",
                    if gap { "gap" } else { "FAIL" },
                    theirs.len()
                );
                if !gap {
                    failed.push(name);
                }
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}
