//! TeX Live's `expand_depth` (10000, tex.ch §366): expansions nested in
//! one another end the job with pdfTeX's "TeX capacity exceeded, sorry
//! [expansion depth=10000]" (accl job 21384, TeX Live 2026's pdftex on
//! `\def\a{\number\a}`), not with the thread's stack. Each nesting is a
//! frame of the run's Rust stack, so the deepest legal nesting must fit
//! the stack of every thread the engine runs on (`WORKER_STACK`).

mod common;

use phitex_tex::Options;

/// (on the test's own thread, std's 2 MiB stack: the engine runs the
/// evaluation on threads of its own, `phitex_defuse::WORKER_STACK`)
fn build(name: &str, threads: usize, src: &str) -> phitex_tex::Build {
    let dir = common::scratch(&format!("{name}-{threads}"));
    let opts = Options {
        threads: Some(threads),
        ..common::options("./d.tex", "d", "", &dir)
    };
    common::build(opts, Some(&common::plain()), src)
}

/// 9990 expansions nested, each scanning the next, on the deepest paths
/// measured (`\expanded` 1024 bytes a nesting, `\number` 336, accl job
/// 21489): legal in pdfTeX, and on one thread and eight.
#[test]
fn the_deepest_legal_nesting_runs_on_every_thread() {
    let n = 9990;
    for (name, src, want) in [
        ("number", format!("\\message{{[{}0]}}\n\\bye\n", "\\number".repeat(n)), "[0]"),
        ("expanded", format!("\\message{{[{}x{}]}}\n\\bye\n", "\\expanded{".repeat(n), "}".repeat(n)), "[x]"),
        ("numexpr", format!("\\message{{[\\number{}1{}]}}\n\\bye\n", "\\numexpr".repeat(n), "\\relax".repeat(n)), "[1]"),
    ] {
        for threads in [1, 8] {
            let b = build(name, threads, &src);
            let (_, log) = b.output();
            assert!(!b.fatal(), "{name} -j{threads}: {:?}", b.diagnostics());
            assert!(log.contains(want), "{name} -j{threads}: {log}");
        }
    }
}

/// `\def\a{\number\a}`: the nesting has no end, and the job ends at the
/// limit with pdfTeX's message.
#[test]
fn an_endless_nesting_ends_at_the_expansion_depth() {
    for threads in [1, 8] {
        let b = build("over", threads, "\\def\\a{\\number\\a}\\message{[\\a]}\n\\bye\n");
        let (_, log) = b.output();
        assert!(b.fatal(), "-j{threads}: {:?}", b.diagnostics());
        assert!(
            log.contains("TeX capacity exceeded, sorry [expansion depth=10000]"),
            "-j{threads}: {log}"
        );
    }
}

