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

fn stuck(b: &phitex_tex::Build) -> Option<String> {
    b.diagnostics().into_iter().map(|d| d.message).find(|m| m.contains("stuck:"))
}

#[test]
fn a_run_that_makes_no_progress_ends_the_build_stuck() {
    for threads in [1, 8] {
        // (an expansion that never ends and assigns nothing: the command
        // count rule or the clock, whichever comes first)
        let (b, t) = build("expand", threads, "Before the loop.\n\n\\def\\a{\\a}\\a\n\nAfter it.\n\n\\bye\n");
        let m = stuck(&b).unwrap_or_else(|| panic!("-j{threads}: no stuck error after {t:?}: {:?}", b.diagnostics().iter().map(|d| &d.message).collect::<Vec<_>>()));
        assert!(t < Duration::from_secs(30), "-j{threads}: stuck found after {t:?}: {m}");
        assert!(m.contains("the main file byte 28"), "-j{threads}: the place is named, the loop's line: {m}");
        // (a loop that assigns at every step and reads no new line: the
        // clock, 3 s here)
        let (b, t) = build(
            "assign",
            threads,
            "Before the loop.\n\n\\count1=0 \\loop\\advance\\count1 by 1 \\iftrue\\repeat\n\nAfter it.\n\n\\bye\n",
        );
        let m = stuck(&b).unwrap_or_else(|| panic!("-j{threads}: no stuck error after {t:?}"));
        assert!(m.contains("read no new line of its input"), "-j{threads}: {m}");
        assert!(t < Duration::from_secs(30), "-j{threads}: stuck found after {t:?}: {m}");
    }
    // (a document that ends is not stuck)
    let mut src = String::new();
    for i in 0..200 {
        let _ = write!(src, "Paragraph {i}, long enough to make a line or two when set in the plain format's ten point roman.\n\n");
    }
    src.push_str("\\bye\n");
    let (b, _) = build("plain", 8, &src);
    assert_eq!(stuck(&b), None);
    assert!(!b.output().0.is_empty(), "the document's output is made");
}
