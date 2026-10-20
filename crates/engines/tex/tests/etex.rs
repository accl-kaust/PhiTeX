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

/// Catcodes set locally inside groups that read files line by line
/// (`\read`, `\readline`), use `\scantokens`/`\everyeof`, recurse, or
/// mix `\global` and local assignments come back to the run's own
/// values at the group's end (§282, §283).
#[test]
fn catcode_restore() {
    run(&["catrestore"]);
}

/// `\expandafter` over `\else`/`\fi` with e-TeX's conditionals
/// (`\unless`, `\ifcsname`, `\ifdefined`, `\detokenize` emptiness).
#[test]
fn expandafter_conditionals() {
    run(&["xcondx"]);
}

/// `\vsplit` clears the split marks first (§977, e-TeX's `vsplit_init`),
/// whatever the box holds: a split of the void box left by the one before
/// leaves none, as pdfTeX's log of `io.tex` has it.
#[test]
fn vsplit_of_a_void_box_clears_the_split_marks() {
    let dir = common::fixtures().join("etex");
    let src = "\\setbox1\\vbox{\\hrule height 5pt\\marks3{three-a}\\mark{zero}\\hrule height 5pt\\marks3{three-b}%\n\\penalty-10000 \\hrule height 5pt \\marks3{three-c}}\n\\setbox2\\vsplit1 to 10pt\n\\setbox2\\vsplit1 to 10pt\n\\message{[\\splitfirstmarks3][\\splitbotmarks3][\\splitfirstmark]}\n\\setbox2\\vsplit1 to 10pt\n\\message{[\\splitfirstmarks3]}\n";
    let doc = format!("{PRELUDE}{src}\\end\n");
    let opts = common::options("./core.tex", "texput", "", &dir);
    let b = common::build(opts, None, &doc);
    let log = b.output().1.replace('\n', "");
    assert!(log.contains("[three-c][three-c][] []"), "{log}");
}
