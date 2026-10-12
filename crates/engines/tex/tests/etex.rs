//! Gate: `oracle/fixtures/etex/*.tex` under INITEX in e-TeX's extended
//! mode (pdfTeX's, as the oracle ran them): the log's lines from
//! `(./core.tex` on equal pdfTeX's.

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
        let opts = common::options("./core.tex", "texput", "", &dir);
        let b = common::build(opts, None, &doc);
        let (_, log) = b.output();
        let log = log.trim_end_matches('\n');
        match common::first_difference(log.as_bytes(), want.as_bytes()) {
            None => eprintln!("etex/{n}: ok ({} nodes run)", b.stats.nodes_run),
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

/// A node that closes a group and opens another (`\middle`, like
/// `\discretionary`'s and `\mathchoice`'s `}{`) does not see the
/// definitions of the groups around them: `\count1=7
/// \discretionary{}{\the\count1}{}` reads 5 from outside.
#[test]
#[ignore = "close-and-open brackets lose the enclosing groups' definitions"]
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
