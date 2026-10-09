//! Conformance with TeX: each program in `tests/core/` uses only
//! expansion, assignments and `\message`/`\immediate\write` (no
//! typesetting), runs on the graph (`Build::update`, its `Output::log`
//! laid out by `print::log`), and must give exactly the log that `tex
//! -ini` gives (`tests/core/NAME.log`: the lines after `**core.tex`, up to
//! the end of the file, recorded from TeX). Where a `tex` binary is found,
//! the recorded log is checked against a fresh run of it too;
//! `PHITEX_FLOW_BLESS=1` records it anew.
//!
//! TeX runs each program after a prelude giving it the graph's initial
//! state where that differs from `IniTeX`'s (the category codes and
//! parameters the minimal system's documents rely on: `val::initial`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options};

/// What makes `tex -ini`'s state the graph's initial one.
const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \
\\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \
\\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../partex-core/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/core")
}

/// The graph's log of a program, as TeX lays it out.
fn graph_log(src: &str) -> (String, Vec<String>) {
    let o = Build::new(font()).update(src, Options::default());
    (phitex_flow::print::log("(./core.tex", &o.log, " )"), o.errors)
}

/// TeX's log of a program, if there is a `tex`.
fn tex_log(name: &str, src: &str) -> Option<String> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-core-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::write(tmp.join("core.tex"), format!("{PRELUDE}{src}\\end\n")).ok()?;
    let st = Command::new("tex")
        .args(["-ini", "-interaction=batchmode", "-jobname=texput", "core.tex"])
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join("texput.log")).ok();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    assert!(st.status.success() || log.contains("No pages of output"), "tex failed:\n{log}");
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("**"))? + 1;
    let end = lines.iter().position(|l| l.starts_with("No pages of output") || l.starts_with("Output written"))?;
    Some(lines[start..end].join("\n"))
}

fn check(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    let expected_path = dir().join(format!("{name}.log"));
    let tex = tex_log(name, &src);
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        let t = tex.as_ref().expect("PHITEX_FLOW_BLESS needs a tex binary");
        std::fs::write(&expected_path, format!("{t}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(&expected_path).unwrap_or_else(|_| panic!("no {}", expected_path.display()));
    let expected = expected.strip_suffix('\n').unwrap_or(&expected);
    if let Some(t) = &tex {
        assert_eq!(t, expected, "{name}: TeX's log is not the recorded one");
    }
    let (got, errors) = graph_log(&src);
    // (TeX's log goes on after the file's end: our last newline is its)
    let got = got.strip_suffix('\n').unwrap_or(&got).to_string();
    assert!(errors.is_empty(), "{name}: errors on the graph: {errors:?}");
    if got != expected {
        let (g, e): (Vec<&str>, Vec<&str>) = (got.lines().collect(), expected.lines().collect());
        let at = g.iter().zip(&e).position(|(a, b)| a != b).unwrap_or(g.len().min(e.len()));
        let at = if g.get(at).is_none() && e.get(at).is_none() { g.len().saturating_sub(1) } else { at };
        panic!(
            "{name}: the graph's log differs from TeX's at line {}:\n graph: {:?}\n   TeX: {:?}\n--- graph:\n{got:?}\n--- TeX:\n{expected:?}",
            at + 1,
            g.get(at),
            e.get(at)
        );
    }
}

macro_rules! programs {
    ($($name:ident),* $(,)?) => {
        $(#[test] fn $name() { check(stringify!($name)); })*
    };
}

programs!(macros, expandafter, edef, conds, arith, the, case, groups, futurelet, plain);
