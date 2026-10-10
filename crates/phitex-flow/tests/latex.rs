//! LaTeX's format, built on the graph: TeX Live's `latex.ltx` (and every
//! file it reads: `texsys.cfg`, expl3, the font definitions, `hyphen.cfg`
//! and the patterns, the Unicode data) against `etex -ini -etex latex.ltx`
//! (TeX Live's `etex` is pdfTeX writing DVI), with the date forced
//! (`SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1`, the graph's own date). The
//! log is compared from the first file up to `\dump`'s ` (preloaded
//! format=…)` line (the statistics after it describe TeX's memory); the
//! format file is not compared.
//!
//! The input directories are those the oracle read its files from
//! (kpathsea's, as `-recorder` lists them), recorded in
//! `tests/latex/dirs.txt`; `tests/latex/` itself comes first, holding the
//! `texsys.aux` the run writes and reads back at once (a file written by
//! `\immediate\write` is not yet readable by `\openin` in the same run on
//! the graph: CORE.md, left for the core). TeX's outputs are recorded in
//! `tests/latex/`; where an `etex` binary is found they are checked
//! against a fresh run; `PHITEX_FLOW_BLESS=1` records them anew.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::build::{CUT_LINES, MAIN_MEMORY};
use phitex_flow::{Build, Font, Options, Order, Reads};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/latex")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// The log from the line after `**latex.ltx` to `\dump`'s ` (preloaded
/// format=…)` line.
fn cut(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("**"))? + 1;
    let end = lines.iter().position(|l| l.starts_with(" (preloaded format="))?;
    Some(lines[start..=end].join("\n"))
}

/// What `etex -ini -etex latex.ltx` makes, if there is an `etex`: the log
/// (cut), the directories it read files from (in the order it first read
/// one from each; the main file's first) and the `texsys.aux` it wrote.
fn etex() -> Option<(String, Vec<String>, Vec<u8>)> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-latex-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    let st = Command::new("etex")
        .args(["-ini", "-etex", "-recorder", "-interaction=batchmode", "latex.ltx"])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let log = std::fs::read_to_string(tmp.join("latex.log")).ok();
    let fls = std::fs::read_to_string(tmp.join("latex.fls")).unwrap_or_default();
    let aux = std::fs::read(tmp.join("texsys.aux")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);
    let log = log?;
    assert!(st.status.success(), "etex failed:\n{log}");
    let mut dirs: Vec<String> = Vec::new();
    for l in fls.lines() {
        let Some(f) = l.strip_prefix("INPUT ") else { continue };
        let p = Path::new(f);
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !p.is_absolute() || f.starts_with("/dev/") || ["tfm", "fmt", "cnf"].contains(&ext.as_str()) {
            continue;
        }
        let d = p.parent()?.to_string_lossy().into_owned();
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    Some((cut(&log)?, dirs, aux))
}

/// The main memory the runs may hold: TeX Live's (`graph::words_of`
/// counts material and the nodes a run holds, not the definitions it
/// makes: LaTeX redefines much).
const MEMORY: u64 = MAIN_MEMORY;

/// The schedules (`tests/etex.rs`'s).
const RUNS: [Options; 5] = [
    Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MEMORY, cut_lines: CUT_LINES },
    Options { threads: 8, reads: Reads::Suspend, order: Order::Priority, dump: false, etex: true, main_memory: MEMORY, cut_lines: CUT_LINES },
    Options {
        threads: 8,
        reads: Reads::Speculate,
        order: Order::Priority,
        dump: false,
        etex: true,
        main_memory: MEMORY,
        cut_lines: CUT_LINES,
    },
    Options {
        threads: 8,
        reads: Reads::Suspend,
        order: Order::Random(1),
        dump: false,
        etex: true,
        main_memory: MEMORY,
        cut_lines: CUT_LINES,
    },
    Options {
        threads: 8,
        reads: Reads::Speculate,
        order: Order::Random(7),
        dump: false,
        etex: true,
        main_memory: MEMORY,
        cut_lines: CUT_LINES,
    },
];

/// The recorded oracle: the log, the directories, `texsys.aux`.
fn recorded() -> (String, Vec<String>) {
    let log = std::fs::read_to_string(dir().join("latex.log")).expect("tests/latex/latex.log");
    let dirs = std::fs::read_to_string(dir().join("dirs.txt")).expect("tests/latex/dirs.txt");
    (log.strip_suffix('\n').unwrap_or(&log).to_string(), dirs.lines().map(str::to_string).collect())
}

/// The recorded oracle is what `etex` makes now; `PHITEX_FLOW_BLESS=1`
/// records it.
#[test]
fn oracles_are_etex() {
    let Some((log, dirs, aux)) = etex() else { return };
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(dir().join("latex.log"), format!("{log}\n")).unwrap();
        std::fs::write(dir().join("dirs.txt"), dirs.join("\n") + "\n").unwrap();
        std::fs::write(dir().join("texsys.aux"), &aux).unwrap();
    }
    let (want_log, want_dirs) = recorded();
    assert_eq!(log, want_log, "etex's log is not the recorded one");
    assert_eq!(dirs, want_dirs, "etex read its files from other directories");
    assert_eq!(aux, std::fs::read(dir().join("texsys.aux")).unwrap(), "etex's texsys.aux is not the recorded one");
}

/// The format's log on the graph is etex's, at every schedule, without
/// errors. Each schedule runs in a process of its own (this test, run
/// again with `PHITEX_FLOW_LATEX_RUN` naming it): a build of the format
/// holds about 5 GB, and the memory one build gave back is not all
/// reusable by the next in the same process (the allocator's per-thread
/// arenas), which would pass the sandbox's 16 GB.
#[test]
fn latex_format() {
    if let Ok(k) = std::env::var("PHITEX_FLOW_LATEX_RUN") {
        format_at(RUNS[k.parse::<usize>().expect("a schedule")]);
        return;
    }
    for (k, o) in RUNS.iter().enumerate() {
        let st = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", "latex_format", "--nocapture", "--test-threads=1"])
            .env("PHITEX_FLOW_LATEX_RUN", k.to_string())
            .status()
            .expect("the test binary runs");
        assert!(st.success(), "({o:?}) the format on the graph failed (its message above)");
    }
}

/// [`latex_format`] at schedule `o`.
fn format_at(o: Options) {
    let (want, dirs) = recorded();
    let main = Path::new(&dirs[0]).join("latex.ltx");
    let src = std::fs::read_to_string(&main).expect("latex.ltx");
    let mut inputs = vec![dir()];
    inputs.extend(dirs.iter().map(PathBuf::from));
    let mut b = Build::new(font());
    b.set_input_dirs(inputs);
    b.set_jobname("latex");
    let out = b.update(&src, o);
    assert!(out.errors.is_empty(), "({o:?}) errors on the graph: {:?}", &out.errors[..out.errors.len().min(20)]);
    let log = phitex_flow::print::log(&format!("({}", main.display()), &out.log, " )");
    let got = log.lines().take_while(|l| !l.starts_with(" (preloaded format=")).count();
    let lines: Vec<&str> = log.lines().collect();
    let log = lines[..(got + 1).min(lines.len())].join("\n");
    if log != want {
        let (g, e): (Vec<&str>, Vec<&str>) = (log.lines().collect(), want.lines().collect());
        let at = g.iter().zip(&e).position(|(a, b)| a != b).unwrap_or(g.len().min(e.len()));
        panic!(
            "({o:?}) the log differs at line {} of {}:\n graph: {:?}\n   TeX: {:?}",
            at + 1,
            e.len(),
            g.get(at),
            e.get(at)
        );
    }
}
