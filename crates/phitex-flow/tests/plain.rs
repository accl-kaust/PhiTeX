//! The plain-TeX gate: documents that begin with plain.tex's source (the
//! prelude of `tests/dvi/plaintext.tex`, with TeX Live's `hyphen.tex` where
//! plain.tex says `\input hyphen`) followed by a body in
//! `tests/plain/NAME.tex`, checked against `tex -ini` on the same text:
//! the DVI file byte for byte (the preamble's date masked) and the log's
//! lines, at the five schedules of `tests/dvi.rs`.
//!
//! Each body runs twice: as written, with plain's `\output` (`NAME.dvi`,
//! `NAME.log`), and with `\output={}` first, TeX's default output
//! (`NAME-default.dvi`, `NAME-default.log`). Only the documents the graph
//! sets exactly today are tests; the others are recorded oracles (checked
//! against `tex` by [`oracles_are_tex`]) waiting for what [`WAITING`]
//! names. `PHITEX_FLOW_BLESS=1` records them all anew.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::build::MAIN_MEMORY;
use phitex_flow::{Build, Font, Options, Order, Reads};

/// Every document, and what the graph needs before it is a test (`""`:
/// it is one).
const WAITING: [(&str, &str); 10] = [
    ("story", ""),
    ("story-default", ""),
    ("pages", ""),
    ("pages-default", ""),
    ("display", ""),
    ("display-default", ""),
    ("table", ""),
    ("table-default", ""),
    ("hyphens", ""),
    ("hyphens-default", ""),
];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plain")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../partex-core/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// plain.tex: `tests/dvi/plaintext.tex` before its body, with `hyphen.tex`.
fn plain() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let s = std::fs::read_to_string(root.join("tests/dvi/plaintext.tex")).unwrap();
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = std::fs::read_to_string(root.join("tests/hyph/hyphen.tex")).unwrap();
    s[..at].replace("% (\\input hyphen: no hyphenation patterns on the graph yet)\n", &h)
}

/// The document `name` (`NAME-default`: the body after `\output={}`).
fn doc(name: &str) -> String {
    let (body, default) = match name.strip_suffix("-default") {
        Some(b) => (b, true),
        None => (name, false),
    };
    let b = std::fs::read_to_string(dir().join(format!("{body}.tex"))).unwrap();
    plain() + if default { "\\output={}\n" } else { "" } + &b
}

/// The log's lines after `**NAME.tex` up to `Output written` (or `No pages`).
fn log_body(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("**"))? + 1;
    let end = lines.iter().position(|l| l.starts_with("No pages of output") || l.starts_with("Output written"))?;
    Some(lines[start..end].join("\n"))
}

/// TeX's DVI file and log of a document, if there is a `tex`.
fn tex(name: &str, src: &str) -> Option<(Vec<u8>, String)> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-plain-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join(format!("{name}.tex")), src).ok()?;
    let st = Command::new("tex")
        .args(["-ini", "-interaction=batchmode", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join(format!("{name}.log"))).ok();
    let dvi = std::fs::read(tmp.join(format!("{name}.dvi"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    assert!(st.status.success(), "tex failed on {name}:\n{log}");
    Some((dvi, log_body(&log)?))
}

/// A DVI file with its preamble's comment masked.
fn masked(dvi: &[u8]) -> Vec<u8> {
    let mut d = dvi.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'?';
        }
    }
    d
}

/// The schedules (as `tests/dvi.rs`).
const RUNS: [Options; 5] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: false, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: false, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: false, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: false, main_memory: MAIN_MEMORY },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: false, main_memory: MAIN_MEMORY },
];

fn recorded(name: &str) -> (Vec<u8>, String) {
    let dvi = std::fs::read(dir().join(format!("{name}.dvi"))).unwrap_or_else(|_| panic!("no {name}.dvi"));
    let log = std::fs::read_to_string(dir().join(format!("{name}.log"))).unwrap_or_else(|_| panic!("no {name}.log"));
    (dvi, log.strip_suffix('\n').unwrap_or(&log).to_string())
}

/// The graph sets document `name` as TeX does, at every schedule.
fn check(name: &str) {
    check_runs(name, &RUNS);
}

/// [`check`] at the schedules `runs`.
fn check_runs(name: &str, runs: &[Options]) {
    assert!(WAITING.iter().any(|(n, w)| *n == name && w.is_empty()), "{name} is not a test yet");
    let src = doc(name);
    let (want_dvi, want_log) = recorded(name);
    for &o in runs {
        let out = Build::new(font()).update(&src, o);
        assert!(out.errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", out.errors);
        let log = phitex_flow::print::log(&format!("(./{name}.tex"), &out.log, " )");
        let log = log.strip_suffix('\n').unwrap_or(&log);
        if log != want_log {
            let (g, e): (Vec<&str>, Vec<&str>) = (log.lines().collect(), want_log.lines().collect());
            let at = g.iter().zip(&e).position(|(a, b)| a != b).unwrap_or(g.len().min(e.len()));
            panic!(
                "{name} ({o:?}): the log differs at line {}:\n graph: {:?}\n   TeX: {:?}",
                at + 1,
                g.get(at),
                e.get(at)
            );
        }
        assert!(masked(&out.dvi) == masked(&want_dvi), "{name} ({o:?}): the DVI file differs");
    }
}

/// Every recorded oracle is what `tex -ini` makes of its document (the
/// waiting ones too); `PHITEX_FLOW_BLESS=1` records them.
#[test]
fn oracles_are_tex() {
    for (name, _) in WAITING {
        let Some((dvi, log)) = tex(name, &doc(name)) else { return };
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            std::fs::write(dir().join(format!("{name}.dvi")), &dvi).unwrap();
            std::fs::write(dir().join(format!("{name}.log")), format!("{log}\n")).unwrap();
        }
        let (want_dvi, want_log) = recorded(name);
        assert!(masked(&dvi) == masked(&want_dvi), "{name}: TeX's DVI is not the recorded one");
        assert_eq!(log, want_log, "{name}: TeX's log is not the recorded one");
    }
}

#[test]
fn pages_default() {
    check("pages-default");
}

#[test]
fn display_default() {
    check("display-default");
}

#[test]
fn hyphens_default() {
    check("hyphens-default");
}

#[test]
fn pages() {
    check("pages");
}

#[test]
fn display() {
    check("display");
}

#[test]
fn hyphens() {
    check("hyphens");
}

#[test]
fn story() {
    check("story");
}

#[test]
fn story_default() {
    check("story-default");
}

#[test]
fn table() {
    check("table");
}

/// Every schedule: a piece running on the prediction that plain's `\m@ne`
/// is still 0 loops in `\settabs`'s `\loop` until the prediction is
/// refuted and the run abandoned.
#[test]
fn table_default() {
    check("table-default");
}
