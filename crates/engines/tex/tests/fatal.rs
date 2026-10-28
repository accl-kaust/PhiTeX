//! A fatal error ends the job (§93, pdfTeX's `fatal_error_stop`): nothing
//! after the piece that hit it runs. Before, the rest of the document ran
//! on, 65 times a node at ten threads on a paper that needed another
//! engine (fontspec under pdflatex): 754035 runs, 9.6 GB resident, 88 s,
//! for a build that pdfTeX ends in a second.

mod common;

use phitex_tex::Options;

/// `n` paragraphs after a `\read16` in nonstop mode, which is fatal:
/// "*** (cannot \read from terminal in nonstop modes)".
fn doc(n: usize) -> String {
    let mut s = String::new();
    for i in 0..2 {
        s.push_str(&format!("Paragraph {i} before the error, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches.\n\n"));
    }
    s.push_str("\\batchmode\\read16 to\\x\n\n");
    for i in 0..n {
        s.push_str(&format!("Paragraph {i} after the error, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches, which pdfTeX never reads.\n\n"));
    }
    s.push_str("\\bye\n");
    s
}

fn build(threads: usize) -> phitex_tex::Build {
    let dir = common::scratch(&format!("fatal-{threads}"));
    let opts = Options {
        threads: Some(threads),
        ..common::options("./d.tex", "d", "", &dir)
    };
    common::build(opts, Some(&common::plain()), &doc(300))
}

/// One thread: the fatal piece runs, the evaluation halts, and of the 300
/// paragraphs after it none runs.
#[test]
fn a_fatal_error_ends_the_job_on_one_thread() {
    let b = build(1);
    assert!(b.fatal(), "the job ended on the fatal error: {:?}", b.stats.runs_by_op);
    assert!(b.stats.halted, "the evaluation was halted: {:?}", b.stats.runs_by_op);
    let exec = common::runs_of(&b, "exec");
    assert!(exec <= 4, "the pieces before the error and the error's, no more: {exec} exec runs, {:?}", b.stats.runs_by_op);
}

/// Eight threads: pieces that had started before the fatal run committed
/// finish, nothing else runs; far fewer than the paragraphs, and no page.
#[test]
fn a_fatal_error_ends_the_job_on_eight_threads() {
    let b = build(8);
    assert!(b.fatal(), "{:?}", b.stats.runs_by_op);
    assert!(b.stats.halted, "{:?}", b.stats.runs_by_op);
    let (exec, live) = (common::runs_of(&b, "exec"), b.nodes_by_op("exec") as u64);
    assert!(exec < live / 2, "at most the pieces that had started: {exec} runs of {live} pieces, {:?}", b.stats.runs_by_op);
    assert_eq!(common::runs_of(&b, "finish"), 0, "no page is finished after a fatal error: {:?}", b.stats.runs_by_op);
}
