//! A tiny plain document recorded through `pdftex -ini` and analyzed:
//! the critical path is shorter than the step count, and the `\def` on
//! line 1 is a dependency of the macro's use on line 3.
//!
//! The lines are known because each line opens a group (`{}`): the log
//! reports input lines at group boundaries, not per step.

use std::io::BufReader;
use std::path::PathBuf;

use phitex_trace::analyze::Graph;
use phitex_trace::parse::Options;
use phitex_trace::record::{Engine, record};
use phitex_trace::step::Kind;

const DOC: &str = "{}\\def\\foo{Hello world}\n% a comment\n{}\\foo\\ and again \\foo.\n\\bye\n";

fn scratch() -> PathBuf {
    let base = std::env::var_os("CLAUDE_JOB_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target"),
        |d| PathBuf::from(d).join("tmp"),
    );
    base.join("phitex-trace-test")
}

#[test]
fn def_on_line_one_feeds_its_use_on_line_three() {
    let dir = scratch();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let main = dir.join("tiny.tex");
    std::fs::write(&main, DOC).expect("the document");
    let out = dir.join("tiny.trace.jsonl");
    let recorded = record(
        &main,
        Engine::Pdftex,
        &dir.join("run"),
        &out,
        Options::default(),
    )
    .expect("pdftex runs");
    assert_eq!(
        recorded.status,
        Some(0),
        "pdftex failed; see {}",
        recorded.log.display()
    );
    assert!(recorded.steps > 10);

    let g = Graph::load(BufReader::new(
        std::fs::File::open(&out).expect("the trace"),
    ))
    .expect("a graph");
    let n = g.steps();
    assert!(
        g.critical_path() < n as u64,
        "critical path {} of {n} steps",
        g.critical_path()
    );
    assert!(g.critical_path() >= 2);

    let def = (0..n)
        .find(|&i| g.kind(i) == Kind::Primitive && g.name(i) == "\\def" && g.line(i) == Some(1))
        .expect("the \\def on line 1");
    let uses: Vec<usize> = (0..n)
        .filter(|&i| g.kind(i) == Kind::Macro && g.name(i) == "\\foo")
        .collect();
    assert_eq!(uses.len(), 2, "two calls of \\foo");
    for u in uses {
        assert_eq!(g.line(u), Some(3));
        assert!(
            g.deps(u)
                .any(|(from, field)| from == def && field == "cs:\\foo"),
            "the call of \\foo depends on its \\def"
        );
    }
}
