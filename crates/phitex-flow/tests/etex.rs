//! Conformance with e-TeX: each program in `tests/etex/` runs on the graph
//! in e-TeX's extended mode (`Options::etex`) and must give exactly the
//! log that `etex -ini -etex` gives (`tests/etex/NAME.log`, recorded from
//! it as `tests/core.rs` records TeX's; `PHITEX_FLOW_BLESS=1` records
//! anew). A program whose name ends in `_errors` makes errors on purpose:
//! there the graph's errors must be TeX's (`! …` lines, in order) and the
//! log, without TeX's error blocks, must hold the same words.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::build::{CUT_LINES, MAIN_MEMORY};
use phitex_flow::{Build, Font, Options, Order, Reads};

/// What every program begins with (`tests/core.rs`' prelude).
const PRELUDE: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\$=3 \\catcode`\\&=4 \\catcode`\\#=6 \
\\catcode`\\^=7 \\catcode`\\~=13 \\hsize=468pt \\vsize=643pt \\parindent=20pt \\maxdepth=4pt \
\\baselineskip=12pt \\topskip=10pt \\tolerance=200 \\linepenalty=10 %\n";

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/etex")
}

fn opts() -> Options {
    Options { etex: true, ..Options::default() }
}

/// The schedules each program runs with (`tests/dvi.rs`'s, in extended
/// mode).
const RUNS: [Options; 5] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Priority, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Random(1), dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES },
    Options { threads: 8, reads: Reads::Speculate, order: Order::Random(7), dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: CUT_LINES },
];

/// The files programs read (`\input`, `\openin`): `inc-*.tex` and `*.dat`
/// in `tests/etex/`.
fn inputs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir())
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        n.starts_with("inc-") || p.extension().is_some_and(|e| e == "dat")
    });
    v
}

/// The graph's log of a program, as TeX lays it out, and its errors.
fn graph_log(src: &str, opts: Options) -> (String, Vec<String>) {
    let mut b = Build::new(font());
    b.set_input_dirs(vec![dir()]);
    let o = b.update(&format!("{PRELUDE}{src}"), opts);
    (phitex_flow::print::log("(./core.tex", &o.log, " )"), o.errors)
}

/// e-TeX's log of a program, if there is an `etex`.
fn etex_log(name: &str, src: &str) -> Option<String> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-etex-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for p in inputs() {
        std::fs::copy(&p, tmp.join(p.file_name()?)).ok()?;
    }
    std::fs::write(tmp.join("core.tex"), format!("{PRELUDE}{src}\\end\n")).ok()?;
    let st = Command::new("etex")
        .args(["-ini", "-etex", "-interaction=batchmode", "-jobname=texput", "core.tex"])
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join("texput.log")).ok();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    assert!(st.status.success() || log.contains("No pages of output") || name.ends_with("_errors"), "etex failed:\n{log}");
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("**"))? + 1;
    let end = lines.iter().position(|l| l.starts_with("No pages of output") || l.starts_with("Output written"))?;
    Some(lines[start..end].join("\n"))
}

/// The recorded log of a program (checked against a fresh run of `etex`
/// where there is one; recorded anew with `PHITEX_FLOW_BLESS`).
fn expected(name: &str, src: &str) -> String {
    let path = dir().join(format!("{name}.log"));
    let tex = etex_log(name, src);
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        let t = tex.as_ref().expect("PHITEX_FLOW_BLESS needs an etex binary");
        std::fs::write(&path, format!("{t}\n")).unwrap();
    }
    let e = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no {}", path.display()));
    let e = e.strip_suffix('\n').unwrap_or(&e).to_string();
    if let Some(t) = &tex {
        assert_eq!(*t, e, "{name}: etex's log is not the recorded one");
    }
    e
}

fn check(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    let expected = expected(name, &src);
    for o in RUNS {
        let (got, errors) = graph_log(&src, o);
        let got = got.strip_suffix('\n').unwrap_or(&got).to_string();
        assert!(errors.is_empty(), "{name} ({o:?}): errors on the graph: {errors:?}");
        if got != expected {
            let (g, e): (Vec<&str>, Vec<&str>) = (got.lines().collect(), expected.lines().collect());
            let at = g.iter().zip(&e).position(|(a, b)| a != b).unwrap_or(g.len().min(e.len()));
            panic!(
                "{name} ({o:?}): the graph's log differs from etex's at line {}:\n graph: {:?}\n  etex: {:?}\n--- graph:\n{got}\n--- etex:\n{expected}",
                at + 1,
                g.get(at),
                e.get(at)
            );
        }
    }
}

/// TeX's log without its error blocks (from a `! ` line to the empty line
/// after its help), and the errors' first lines.
fn split_errors(log: &str) -> (String, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut in_error = false;
    for l in log.lines() {
        if in_error {
            if l.is_empty() {
                in_error = false;
            }
            continue;
        }
        if let Some(e) = l.strip_prefix("! ") {
            errors.push(e.to_string());
            in_error = true;
            continue;
        }
        out.push(l);
    }
    (out.join("\n"), errors)
}

/// An error as the graph reports it, without the piece it is in.
fn without_piece(e: &str) -> &str {
    e.split_once(": ").filter(|(p, _)| p.starts_with("piece ")).map_or(e, |(_, m)| m)
}

fn check_errors(name: &str) {
    let src = std::fs::read_to_string(dir().join(format!("{name}.tex"))).unwrap();
    let (expected, tex_errors) = split_errors(&expected(name, &src));
    let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    for o in RUNS {
        let (got, errors) = graph_log(&src, o);
        // (the errors' blocks are in the graph's log too: compared as words
        // without them, as TeX's)
        let (got, _) = split_errors(&got);
        let errors: Vec<String> = errors.iter().map(|e| format!("{}.", without_piece(e))).collect();
        assert_eq!(errors, tex_errors, "{name} ({o:?}): the graph's errors are not etex's");
        assert_eq!(words(&got), words(&expected), "{name} ({o:?}): another log\n--- graph:\n{got}\n--- etex:\n{expected}");
    }
}

macro_rules! programs {
    ($($name:ident),* $(,)?) => {
        $(#[test] fn $name() { check(stringify!($name)); })*
    };
}

programs!(version, expr, detok, scan, ifs, groups, lastnode, glue, interaction, pens, show);

// (e-TeX with input and output: `\readline` on a stream, `\everyeof` after
// an `\input` file, a file inside `\scantokens`, `\endinput` in a
// `\scantokens` text, `\inputlineno` there, `\splitfirstmarks n` and
// `\splitbotmarks n`)
programs!(io);

#[test]
fn expr_errors() {
    check_errors("expr_errors");
}

/// In TeX's compatibility mode (`etex: false`) e-TeX's names are
/// undefined, and registers stop at 255.
#[test]
fn compatibility_mode_names_nothing_of_etex() {
    let src = "\\message{\\meaning\\numexpr,\\meaning\\protected}\\count256=1\n";
    let o = Build::new(font()).update(&format!("{PRELUDE}{src}"), Options { etex: false, ..Options::default() });
    assert_eq!(o.log[0], "undefined,undefined");
    let errors: Vec<&str> = o.errors.iter().map(|e| without_piece(e)).collect();
    assert_eq!(errors, ["Bad register code (256)"]);
}

/// e-TeX's expansion primitives are steps, never nodes; `\ifcsname`
/// defines nothing (unlike `\csname`).
#[test]
fn etex_steps_make_no_nodes_and_ifcsname_defines_nothing() {
    // (`\everyeof{\noexpand}`: a pseudo file ending inside `\edef` is
    // "File ended while scanning definition", tex.web 362, unless the
    // file's end is met by `\noexpand`)
    let src = "\\everyeof{\\noexpand}\\def\\a{A}\\edef\\b{\\unexpanded{\\a}\\detokenize{\\a}\\scantokens{x}\\numexpr1\\relax}%\n\
               \\count1=\\numexpr 2*3\\relax\\ifcsname zz\\endcsname\\else\\count2=1 \\fi\\message{\\b}\n";
    let o = Build::new(font()).update(&format!("{PRELUDE}{src}"), Options { dump: true, ..opts() });
    assert!(o.dump.contains("\\count2 = φ(1 | 0)"), "no φ for \\count2:\n{}", o.dump);
    for l in o.dump.lines().filter(|l| l.starts_with("  n")) {
        let head = l.split(" | ").next().unwrap_or("");
        let cmd = head.split_whitespace().nth(2).unwrap_or("");
        assert!(
            !["\\unexpanded", "\\detokenize", "\\scantokens", "\\numexpr"].contains(&cmd),
            "a step became a node: {l}"
        );
        assert!(!l.contains("defines \\zz"), "\\ifcsname defined a name: {l}");
    }
}

/// An edit gives what a cold build of the edited text gives.
#[test]
fn an_edit_gives_the_cold_log() {
    let src = std::fs::read_to_string(dir().join("ifs.tex")).unwrap();
    let src = format!("{PRELUDE}{src}");
    let mut b = Build::new(font());
    let first = b.update(&src, opts());
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let mut text = src.clone();
    for (from, to) in [("\\ifdefined\\relax", "\\ifdefined\\undefined"), ("\\iftrue \\message", "\\iffalse \\message")] {
        assert!(text.contains(from), "{from}");
        text = text.replacen(from, to, 1);
        let e = b.update(&text, opts());
        let cold = Build::new(font()).update(&text, opts());
        assert_eq!(e.errors, cold.errors, "after {from:?} => {to:?}");
        assert_eq!(e.log, cold.log, "after {from:?} => {to:?}");
    }
}
