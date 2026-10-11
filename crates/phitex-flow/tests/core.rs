//! Conformance with TeX: each program in `tests/core/` uses only
//! expansion, assignments and `\message`/`\immediate\write` (no
//! typesetting), runs on the graph (`Build::update`, its `Output::log`
//! laid out by `print::log`), and must give exactly the log that `tex
//! -ini` gives (`tests/core/NAME.log`: the lines after `**core.tex`, up to
//! the end of the file, recorded from TeX). Where a `tex` binary is found,
//! the recorded log is checked against a fresh run of it too;
//! `PHITEX_FLOW_BLESS=1` records it anew.
//!
//! Both run each program after the same prelude (category codes for
//! braces and parameters, and a few parameters: the graph's initial state
//! is `IniTeX`'s, `val::initial`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options};

/// What every program begins with.
const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \
\\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \
\\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/core")
}

/// The graph's log of a program, as TeX lays it out (its files found in
/// `tests/core/`).
fn graph_log(src: &str) -> (String, Vec<String>) {
    let mut b = Build::new(font());
    b.set_input_dirs(vec![dir()]);
    let o = b.update(&format!("{PRELUDE}{src}"), Options::default());
    (phitex_flow::print::log("(./core.tex", &o.log, " )"), o.errors)
}

/// TeX's log of a program, if there is a `tex` (the data files of
/// `tests/core/`, `*.dat`, beside it).
fn tex_log(name: &str, src: &str) -> Option<String> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-core-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for e in std::fs::read_dir(dir()).ok()?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "dat") {
            std::fs::copy(&p, tmp.join(e.file_name())).ok()?;
        }
    }
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

programs!(macros, expandafter, edef, conds, arith, the, case, groups, futurelet, plain, boxlevel, readin);

/// The graph's own form: an `\ifnum` is a test node, its taken arm's
/// definitions the register's only ones (no φ at `\fi`); expandable
/// primitives and macro calls are steps, never nodes.
#[test]
fn a_conditional_is_its_test_and_expansion_makes_no_node() {
    let src = "\\count1=0 \\def\\x{1}\\ifnum\\x=1 \\count1=5 \\else\\count1=6 \\fi\n\
               \\expandafter\\def\\csname a\\endcsname{A}\\count2=\\number\\count1 \\csname relax\\endcsname\n\
               \\edef\\b{\\the\\count2\\a}\\message{\\romannumeral\\count2 \\b}\n";
    let o = Build::new(font()).update(&format!("{PRELUDE}{src}"), Options { dump: true, ..Options::default() });
    assert!(o.errors.is_empty(), "{:?}", o.errors);
    assert!(!o.dump.contains("φ("), "a φ:\n{}", o.dump);
    assert_eq!(o.log, ["v5A"]);
    let steps = ["\\expandafter", "\\csname", "\\number", "\\the", "\\romannumeral", "\\x", "\\b", "\\a"];
    let mut commands = Vec::new();
    for l in o.dump.lines().filter(|l| l.starts_with("  n")) {
        let head = l.split(" | ").next().unwrap_or("");
        let cmd = head.split_whitespace().nth(2).unwrap_or("");
        assert!(!steps.contains(&cmd), "a step became a node: {l}");
        commands.push(cmd.to_string());
    }
    // (the commands: the assignments, the conditional's nodes — `\else`
    // ends the taken arm and skips to `\fi` —, `\def`,
    // `\relax` from `\csname`, `\edef`, `\message`)
    for c in ["\\count", "\\ifnum", "\\else", "\\def", "\\relax", "\\edef", "\\message"] {
        assert!(commands.iter().any(|x| x == c), "no {c} node: {commands:?}");
    }
}

/// Builds plain.tex, applies `edits` one after the other, and checks each
/// incremental build's log and errors against a cold build of the text.
fn edits_give_the_cold_log(threads: usize, edits: &[(&str, &str)]) {
    let src = std::fs::read_to_string(dir().join("plain.tex")).unwrap();
    let o = Options { threads, ..Options::default() };
    let mut b = Build::new(font());
    let first = b.update(&src, o);
    assert!(first.errors.is_empty(), "{threads} threads, cold: {:?}", first.errors);
    let mut text = src.clone();
    for &(from, to) in edits {
        assert!(text.contains(from), "{from}");
        text = text.replacen(from, to, 1);
        let e = b.update(&text, o);
        let cold = Build::new(font()).update(&text, o);
        assert_eq!(e.errors, cold.errors, "after {from:?} => {to:?} at {threads} threads");
        assert!(e.log == cold.log, "after {from:?} => {to:?} at {threads} threads: another log");
    }
}

/// An edit gives what a cold build of the edited text gives, the log
/// included: plain.tex edited in a definition read later, a macro, a
/// catcode, a conditional's arm.
#[test]
fn an_edit_gives_the_cold_log() {
    edits_give_the_cold_log(
        1,
        &[
            ("\\mathchardef\\@M=10000", "\\mathchardef\\@M=10001"),
            ("\\def\\magstephalf{1095 }", "\\def\\magstephalf{1096 }\\message{\\magstephalf}"),
            ("\\catcode`@=11", "\\catcode`@=11 \\message{at}"),
            ("\\footrue \\iffoo", "\\foofalse \\iffoo"),
        ],
    );
}

/// When a name read by two nodes of a piece changes, both readers read
/// it again (`\global\advance\count18` inside `\newfam`: `\myfam=\fam6`).
#[test]
fn an_edit_rereads_every_reader_of_a_changed_name() {
    edits_give_the_cold_log(1, &[("\\count18=3 ", "\\count18=5 ")]);
}

/// A cold build of `plain.tex` at 4 threads: every reader of
/// `\maxdimen` in the probes piece reads its definition once installed.
#[test]
fn plain_at_four_threads() {
    edits_give_the_cold_log(4, &[]);
}

/// A kept node is checked by the scope of the definition it read, not
/// only by its value: `\box1` at depth 3 makes box 1 void at the level of
/// the `\setbox` it replaces; an edit above moves that `\setbox` (the
/// same box) from the second group out to the first, so the `\box1` node,
/// at the same depth and reading an equal box, must run again — its void
/// now outlives the second group.
#[test]
fn a_kept_node_reads_the_scope_of_its_definition() {
    let a = format!("{PRELUDE}{{{{\\setbox1=\\hbox to5pt{{}}{{\\box1}}}}\\message{{\\the\\wd1}}}}\\message{{\\the\\wd1}}\n");
    let b = format!("{PRELUDE}{{\\setbox1=\\hbox to5pt{{}}{{{{\\box1}}}}\\message{{\\the\\wd1}}}}\\message{{\\the\\wd1}}\n");
    for threads in [1, 8] {
        let o = Options { threads, ..Options::default() };
        let mut bld = Build::new(font());
        bld.update(&a, o);
        let e = bld.update(&b, o);
        let cold = Build::new(font()).update(&b, o);
        assert!(e.errors.is_empty() && cold.errors.is_empty(), "{:?} {:?}", e.errors, cold.errors);
        assert_eq!(cold.log[..2], ["0.0pt", "0.0pt"], "box 1 is void at both messages (TeX)");
        assert!(e.log == cold.log && e.dvi == cold.dvi, "{threads} threads: {:?}, cold {:?}", e.log, cold.log);
    }
}
