//! A fatal error ends the job (§93, pdfTeX's `fatal_error_stop`): nothing
//! after the piece that hit it runs. Before, the rest of the document ran
//! on, 65 times a node at ten threads on a paper that needed another
//! engine (fontspec under pdflatex): 754035 runs, 9.6 GB resident, 88 s,
//! for a build that pdfTeX ends in a second.

mod common;

use std::fmt::Write as _;

use phitex_tex::Options;

/// `n` paragraphs after a `\read16` in nonstop mode, which is fatal:
/// "*** (cannot \read from terminal in nonstop modes)".
fn doc(n: usize) -> String {
    let mut s = String::new();
    for i in 0..2 {
        let _ = write!(s, "Paragraph {i} before the error, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches.\n\n");
    }
    s.push_str("\\batchmode\\read16 to\\x\n\n");
    for i in 0..n {
        let _ = write!(s, "Paragraph {i} after the error, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches, which pdfTeX never reads.\n\n");
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
    let exec = common::runs_of(&b, "exec");
    assert!(exec <= 4, "the pieces before the error and the error's, no more: {exec} exec runs, {:?}", b.stats.runs_by_op);
}

/// Eight threads: pieces that had started before the fatal run committed
/// finish, nothing else runs; far fewer than the paragraphs, and no page.
#[test]
fn a_fatal_error_ends_the_job_on_eight_threads() {
    let b = build(8);
    assert!(b.fatal(), "{:?}", b.stats.runs_by_op);
    let (exec, live) = (common::runs_of(&b, "exec"), b.nodes_by_op("exec") as u64);
    assert!(exec < live / 2, "at most the pieces that had started: {exec} runs of {live} pieces, {:?}", b.stats.runs_by_op);
    assert_eq!(common::runs_of(&b, "finish"), 0, "no page is finished after a fatal error: {:?}", b.stats.runs_by_op);
}

/// The paragraphs after the error in an included file, placed by the
/// piece that inputs them, not pushed: the claim after a fatal error
/// takes them too (the paper's case: 11000 pieces of included files).
#[test]
fn a_fatal_error_ends_the_job_before_an_included_file() {
    for threads in [1, 8] {
        let dir = common::scratch(&format!("fatal-input-{threads}"));
        let mut body = String::new();
        for i in 0..300 {
            let _ = write!(body, "Paragraph {i} of the included file, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches, which pdfTeX never reads.\n\n");
        }
        std::fs::write(dir.join("body.tex"), body).unwrap();
        let src = "Paragraph before the error, long enough to make a few lines when set in the plain format's ten point roman over a text width of six and a half inches.\n\n\\batchmode\\read16 to\\x\n\n\\input body\n\n\\bye\n";
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let b = common::build(opts, Some(&common::plain()), src);
        assert!(b.fatal(), "-j{threads}: {:?}", b.stats.runs_by_op);
        // (whether the file's pieces were staged before the error or the
        // file was never read: nothing after the error runs)
        let exec = common::runs_of(&b, "exec");
        assert!(exec <= 6, "-j{threads}: the pieces before the error and the error's, no more: {exec} exec runs, {:?}", b.stats.runs_by_op);
        assert_eq!(common::runs_of(&b, "finish"), 0, "-j{threads}: {:?}", b.stats.runs_by_op);
    }
}

/// The job's text ending inside the text an `\else` skips: TeX's end of
/// the file (§362) checks it (§336), "Incomplete \iftrue; all text was
/// ignored after line n", puts a `\fi` there, and the input then ends for
/// good (§360): "*** (job aborted, no legal \end found)". Before, the skip
/// returned at the end of the input as if at an `\else`, and `\else`'s
/// loop (§510) skipped again for ever: the build never ended, at one
/// thread as at sixteen (an output routine's `\else` at sixteen threads,
/// amsldoc, 1 build in about 300). Oracle: pdftex (TeX Live 2026, accl job
/// 21617), the same two lines in the log and no output file.
#[test]
fn the_input_ending_inside_a_skipped_else_ends_the_job() {
    let cases = [("\\iftrue x\\else y\n", 1), ("\\def\\x{\\iftrue a\\else b}\n\\x\nc\n", 2)];
    for (k, (src, line)) in cases.into_iter().enumerate() {
        for threads in [1, 8] {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let dir = common::scratch(&format!("skip-end-{k}-{threads}"));
                let opts = Options {
                    threads: Some(threads),
                    ..common::options("./d.tex", "d", "", &dir)
                };
                let b = common::build(opts, Some(&common::plain()), src);
                let _ = tx.send((b.fatal(), b.output().1));
            });
            let (fatal, log) = rx
                .recv_timeout(std::time::Duration::from_secs(120))
                .unwrap_or_else(|_| panic!("case {k} -j{threads}: the build did not end in 120 s"));
            let want = format!("! Incomplete \\iftrue; all text was ignored after line {line}.");
            assert!(log.contains(&want), "case {k} -j{threads}: {want:?} not in the log:\n{log}");
            assert!(fatal && log.contains("*** (job aborted, no legal \\end found)"), "case {k} -j{threads}: the job is aborted:\n{log}");
        }
    }
}

/// A finisher's run reads its output routine only. A routine that leaves
/// an `\iffalse` open skips to its input's end (TeX would skip on into the
/// text after the page's break, which is the pieces' to read): the run
/// starves there. Before, `run_all` took the text's kind from `args[1]`
/// for every operation, a finisher's being its page's index in its piece,
/// so the finisher of a piece's second page ran as the job's text and
/// took the document's pieces after it (amsldoc at sixteen threads: a
/// finisher whose skip overran its routine pulled 11 of them).
#[test]
fn a_finisher_reads_its_routine_only() {
    let mut src = String::from("\\output={\\shipout\\box255 \\iffalse}\n");
    for i in 0..400 {
        let _ = write!(src, "Word {i} of the long paragraph runs on. ");
    }
    src.push_str("\n\nAfter one.\n\nAfter two.\n\nAfter three.\n\n\\bye\n");
    for threads in [1, 8] {
        let dir = common::scratch(&format!("fin-routine-{threads}"));
        let opts = Options {
            threads: Some(threads),
            ..common::options("./d.tex", "d", "", &dir)
        };
        let b = common::build(opts, Some(&common::plain()), &src);
        assert_eq!(b.nodes_held_by("finish"), 0, "-j{threads}: the document's nodes a finisher holds");
    }
}
