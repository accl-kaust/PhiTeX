//! A build that stops making progress ends stuck (`EvalError::Stuck`,
//! `Run::watchdog`), named and reported, never waited on: a one-letter
//! edit of amsldoc once left a page's run going for minutes at 5 GB and
//! growing, the watch waiting on it with no way out. The limit is 3 s here
//! (`Options::stuck_after`).

mod common;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use phitex_tex::Options;

fn build(name: &str, threads: usize, src: &str) -> (phitex_tex::Build, Duration) {
    let dir = common::scratch(&format!("stuck-{name}-{threads}"));
    let opts = Options {
        threads: Some(threads),
        stuck_after: Some(Duration::from_secs(3)),
        ..common::options("./d.tex", "d", "", &dir)
    };
    let t = Instant::now();
    let b = common::build(opts, Some(&common::plain()), src);
    (b, t.elapsed())
}

/// A build of `src` with the clock's bound at 300 s, the default, and
/// the given memory and time limits.
fn build_limited(
    name: &str,
    threads: usize,
    memory: Option<u64>,
    time: Option<Duration>,
    src: &str,
) -> (phitex_tex::Build, Duration) {
    let dir = common::scratch(&format!("stuck-{name}-{threads}"));
    let opts = Options {
        threads: Some(threads),
        stuck_after: Some(Duration::from_secs(300)),
        memory_limit: memory,
        time_limit: time,
        ..common::options("./d.tex", "d", "", &dir)
    };
    let t = Instant::now();
    let b = common::build(opts, Some(&common::plain()), src);
    (b, t.elapsed())
}

fn stuck(b: &phitex_tex::Build) -> Option<String> {
    b.diagnostics()
        .into_iter()
        .map(|d| d.message)
        .find(|m| m.contains("stuck:"))
}

#[test]
fn a_run_that_makes_no_progress_ends_the_build_stuck() {
    for threads in [1, 8] {
        // (an expansion that never ends and assigns nothing: the command
        // count rule or the clock, whichever comes first)
        let (b, t) = build(
            "expand",
            threads,
            "Before the loop.\n\n\\def\\a{\\a}\\a\n\nAfter it.\n\n\\bye\n",
        );
        let m = stuck(&b).unwrap_or_else(|| {
            panic!(
                "-j{threads}: no stuck error after {t:?}: {:?}",
                b.diagnostics()
                    .iter()
                    .map(|d| &d.message)
                    .collect::<Vec<_>>()
            )
        });
        assert!(
            t < Duration::from_secs(30),
            "-j{threads}: stuck found after {t:?}: {m}"
        );
        assert!(
            m.contains("the main file byte 28"),
            "-j{threads}: the place is named, the loop's line: {m}"
        );
        // (a loop that assigns at every step and reads no new line: the
        // clock, 3 s here)
        let (b, t) = build(
            "assign",
            threads,
            "Before the loop.\n\n\\count1=0 \\loop\\advance\\count1 by 1 \\iftrue\\repeat\n\nAfter it.\n\n\\bye\n",
        );
        let m = stuck(&b).unwrap_or_else(|| panic!("-j{threads}: no stuck error after {t:?}"));
        assert!(
            m.contains("read no new line of its input"),
            "-j{threads}: {m}"
        );
        assert!(
            t < Duration::from_secs(30),
            "-j{threads}: stuck found after {t:?}: {m}"
        );
    }
    // (a document that ends is not stuck)
    let mut src = String::new();
    for i in 0..200 {
        let _ = write!(
            src,
            "Paragraph {i}, long enough to make a line or two when set in the plain format's ten point roman.\n\n"
        );
    }
    src.push_str("\\bye\n");
    let (b, _) = build("plain", 8, &src);
    assert_eq!(stuck(&b), None);
    assert!(!b.output().0.is_empty(), "the document's output is made");
}

/// A run past the memory limit ends the build stuck, and so does one past
/// the build's time limit, inside the run: a compat build with an `.aux`
/// on disk read an endless file of empty lines, grew past 10 GB in 45 s,
/// and neither stopped it (the limit was half the host's memory, past the
/// job's cgroup, and the time limit was checked only between runs).
#[test]
fn a_run_past_the_memory_or_time_limit_ends_the_build_stuck() {
    // (a loop that assigns at every step and reads no new line: the
    // clock's bound is 300 s here, so only the limit under test stops it)
    let src = "Before the loop.\n\n\\count1=0 \\loop\\advance\\count1 by 1 \\iftrue\\repeat\n\nAfter it.\n\n\\bye\n";
    for threads in [1, 8] {
        let (b, t) = build_limited(
            "memory",
            threads,
            Some(1 << 20),
            Some(Duration::from_secs(600)),
            src,
        );
        let m = stuck(&b).unwrap_or_else(|| {
            panic!(
                "-j{threads}: no stuck error after {t:?}: {:?}",
                b.diagnostics()
                    .iter()
                    .map(|d| &d.message)
                    .collect::<Vec<_>>()
            )
        });
        assert!(
            m.contains("past the limit of"),
            "-j{threads}: the memory named: {m}"
        );
        assert!(
            m.contains("the main file byte"),
            "-j{threads}: the place named: {m}"
        );
        assert!(
            t < Duration::from_secs(30),
            "-j{threads}: stuck found after {t:?}: {m}"
        );
        let (b, t) = build_limited(
            "time",
            threads,
            Some(u64::MAX),
            Some(Duration::from_secs(3)),
            src,
        );
        let m = stuck(&b).unwrap_or_else(|| {
            panic!(
                "-j{threads}: no stuck error after {t:?}: {:?}",
                b.diagnostics()
                    .iter()
                    .map(|d| &d.message)
                    .collect::<Vec<_>>()
            )
        });
        assert!(
            m.contains("time limit of 3 s"),
            "-j{threads}: the time limit named: {m}"
        );
        assert!(
            t < Duration::from_secs(30),
            "-j{threads}: stuck found after {t:?}: {m}"
        );
    }
}
