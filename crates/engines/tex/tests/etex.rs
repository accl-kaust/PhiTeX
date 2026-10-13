//! Gate: `oracle/fixtures/etex/*.tex` under INITEX in e-TeX's extended
//! mode (pdfTeX's, as the oracle ran them): the log's lines from
//! `(./core.tex` on equal pdfTeX's, and the DVI file where the oracle
//! wrote one (`<name>.dvi`).

mod common;

const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn run(names: &[&str]) {
    let dir = common::fixtures().join("etex");
    let only = std::env::var("PHITEX_ETEX").ok();
    let mut failed = Vec::new();
    for &n in names {
        if only.as_ref().is_some_and(|o| o != n) {
            continue;
        }
        let src = common::read(&dir.join(format!("{n}.tex")));
        let want = common::read(&dir.join(format!("{n}.log")));
        let want = want.trim_end_matches('\n');
        let doc = format!("{PRELUDE}{src}\\end\n");
        // a fixture that ships pages has the oracle's DVI too: equal bytes
        let want_dvi = std::fs::read(dir.join(format!("{n}.dvi"))).ok();
        let comment = want_dvi
            .as_deref()
            .map(common::dvi_comment)
            .unwrap_or_default();
        let opts = common::options("./core.tex", "texput", &comment, &dir);
        let b = common::build(opts, None, &doc);
        let (dvi, log) = b.output();
        let log = common::log_excerpt(&log);
        let mut diff = common::first_difference(log.as_bytes(), want.as_bytes());
        if diff.is_none()
            && let Some(w) = &want_dvi
        {
            diff = common::first_difference(&dvi, w).map(|d| format!("DVI {d}"));
        }
        match diff {
            None => eprintln!(
                "etex/{n}: ok ({} nodes run, {} rounds, {} loop rounds, restarted_by {:?})",
                b.stats.nodes_run, b.stats.rounds, b.stats.loop_rounds, b.stats.restarted_by
            ),
            Some(d) => {
                eprintln!("etex/{n}: FAIL {d}");
                failed.push(n);
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

#[test]
fn expressions() {
    run(&["expr", "glue"]);
}

#[test]
fn tokens() {
    run(&["detok", "scan", "version"]);
}

#[test]
fn conditionals_groups() {
    run(&["ifs", "groups", "interaction"]);
}

#[test]
fn lists() {
    run(&["lastnode", "pens"]);
}

#[test]
fn marks_penalties() {
    run(&["marks", "linepens"]);
}

#[test]
fn middle() {
    run(&["middle"]);
}

/// `\inputlineno` counts the lines of every file together, not each
/// file's own (and a pseudo file's).
#[test]
#[ignore = "\\inputlineno is not per file"]
fn io() {
    run(&["io"]);
}

/// The error context (`l.4 ...`), `\showbox`, paragraph badness
/// reports and pdfTeX's font expansion and protrusion are not in this
/// crate yet.
#[test]
#[ignore = "error contexts, \\showbox, font expansion"]
fn diagnostics() {
    run(&["show", "expr_errors", "ignoreerr", "pdfmatch", "micro"]);
}
