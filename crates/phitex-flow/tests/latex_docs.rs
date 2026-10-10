//! LaTeX documents on the graph, against TeX Live's `latex` (pdfTeX in DVI
//! mode with the `latex` format) on each document's first run, with no
//! `.aux` present: references print `??` on both sides, so the cross-run
//! fixpoint is not needed.
//!
//! Our side is one build, a session of two jobs (`graph::jobs`): the
//! format's (`latex.ini` as `fmtutil` runs it: `pdftexconfig`,
//! `\pdfoutput=0`, `latex.ltx`, `\dump`) and the document's (`&latex
//! NAME`), with the directories the oracle read its files from (`latex
//! -recorder`) and the `texsys.aux` the format's job writes and reads back
//! (`tests/latex/`, CORE.md). Compared: the DVI file, its preamble's dated
//! comment masked, and the files the document's job writes (its `.aux`,
//! `.toc`); the log is diagnostics only.
//!
//! The documents are in `tests/latex/docs/`, each with what `latex` made:
//! `NAME.dvi`, `NAME.writes` (each file written: a line `=== FILE N`, then
//! its N characters) and `NAME.dirs` (`ini PATH`: `latex.ini`; `input
//! DIR`, `font DIR`). Where `latex` runs they are checked against a fresh
//! run (`oracles_are_latex`); `PHITEX_FLOW_BLESS=1` records them anew.
//!
//! A build holds about 4 GB (`latex.ltx`'s 3.1M nodes): each schedule runs
//! in a process of its own (this test, run again with
//! `PHITEX_FLOW_DOC_RUN`), one document at a time.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use phitex_flow::build::MAIN_MEMORY;
use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Font, Options, Order, Reads};

/// The documents.
const DOCS: [&str; 6] = ["sections", "math", "tables", "figures", "titlepage", "packages"];

/// The schedules: one thread, eight, and eight speculating in a random
/// order.
const RUNS: [Options; 3] = [
    Options {
        threads: 1,
        reads: Reads::Suspend,
        order: Order::Priority,
        dump: false,
        etex: true,
        main_memory: MAIN_MEMORY,
    },
    Options {
        threads: 8,
        reads: Reads::Suspend,
        order: Order::Priority,
        dump: false,
        etex: true,
        main_memory: MAIN_MEMORY,
    },
    Options {
        threads: 8,
        reads: Reads::Speculate,
        order: Order::Random(7),
        dump: false,
        etex: true,
        main_memory: MAIN_MEMORY,
    },
];

fn latex_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/latex")
}

fn docs_dir() -> PathBuf {
    latex_dir().join("docs")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../partex-core/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// Bytes as TeX's characters (one each).
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// What `latex` made of a document.
#[derive(PartialEq, Debug, Default)]
struct Made {
    dvi: Vec<u8>,
    writes: Vec<(String, String)>,
    /// `ini PATH`, `input DIR`, `font DIR` lines.
    dirs: Vec<String>,
}

fn kpsewhich(name: &str) -> Option<String> {
    let o = Command::new("kpsewhich").arg(name).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// `latex NAME` in a directory of its own, if there is a `latex`.
fn run_latex(name: &str) -> Option<Made> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-latex-doc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::copy(docs_dir().join(format!("{name}.tex")), tmp.join(format!("{name}.tex"))).ok()?;
    let st = Command::new("latex")
        .args(["-interaction=batchmode", "-recorder", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let fls = std::fs::read_to_string(tmp.join(format!("{name}.fls"))).unwrap_or_default();
    let dvi = std::fs::read(tmp.join(format!("{name}.dvi"))).ok();
    let mut made = Made::default();
    let mut add = |l: String| {
        if !made.dirs.contains(&l) {
            made.dirs.push(l);
        }
    };
    let ini = kpsewhich("latex.ini")?;
    add(format!("ini {ini}"));
    for f in ["latex.ini", "pdftexconfig.tex"] {
        let p = kpsewhich(f)?;
        add(format!("input {}", Path::new(&p).parent()?.display()));
    }
    for l in fls.lines() {
        if let Some(f) = l.strip_prefix("INPUT ") {
            let p = Path::new(f);
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !p.is_absolute() || f.starts_with("/dev/") || ["fmt", "cnf", "map"].contains(&ext.as_str()) {
                continue;
            }
            let d = p.parent()?.display().to_string();
            add(if ext == "tfm" { format!("font {d}") } else { format!("input {d}") });
        } else if let Some(f) = l.strip_prefix("OUTPUT ") {
            let ext = Path::new(f).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
            if ["log", "dvi", "fls"].contains(&ext.as_str()) || made.writes.iter().any(|(n, _)| n == f) {
                continue;
            }
            let text = std::fs::read(tmp.join(f)).unwrap_or_default();
            made.writes.push((f.to_string(), chars(&text)));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(st.status.success(), "latex failed on {name}");
    made.dvi = dvi?;
    Some(made)
}

fn writes_text(w: &[(String, String)]) -> String {
    let mut s = String::new();
    for (n, t) in w {
        let _ = writeln!(s, "=== {n} {}", t.chars().count());
        s.push_str(t);
    }
    s
}

fn parse_writes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(head) = rest.strip_prefix("=== ") {
        let (line, after) = head.split_once('\n').expect("a header line");
        let (name, n) = line.rsplit_once(' ').expect("a name and a length");
        let n: usize = n.parse().expect("a length");
        let end = after.char_indices().nth(n).map_or(after.len(), |(i, _)| i);
        out.push((name.to_string(), after[..end].to_string()));
        rest = &after[end..];
    }
    out
}

/// The recorded oracle of a document.
fn recorded(name: &str) -> Made {
    let d = docs_dir();
    Made {
        dvi: std::fs::read(d.join(format!("{name}.dvi"))).expect("NAME.dvi"),
        writes: parse_writes(&chars(&std::fs::read(d.join(format!("{name}.writes"))).expect("NAME.writes"))),
        dirs: std::fs::read_to_string(d.join(format!("{name}.dirs")))
            .expect("NAME.dirs")
            .lines()
            .map(str::to_string)
            .collect(),
    }
}

/// The recorded oracles are what `latex` makes now; `PHITEX_FLOW_BLESS=1`
/// records them.
#[test]
fn oracles_are_latex() {
    for name in DOCS {
        let Some(made) = run_latex(name) else { return };
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            let d = docs_dir();
            std::fs::write(d.join(format!("{name}.dvi")), &made.dvi).unwrap();
            let w: Vec<u8> = writes_text(&made.writes).chars().map(|c| u8::try_from(c).expect("a byte")).collect();
            std::fs::write(d.join(format!("{name}.writes")), w).unwrap();
            std::fs::write(d.join(format!("{name}.dirs")), made.dirs.join("\n") + "\n").unwrap();
        }
        let want = recorded(name);
        assert!(masked(&made.dvi) == masked(&want.dvi), "{name}: latex's DVI is not the recorded one");
        assert_eq!(made.writes, want.writes, "{name}: latex's written files are not the recorded ones");
        assert_eq!(made.dirs, want.dirs, "{name}: latex read its files from other directories");
    }
}

/// A DVI file with its preamble's comment (the date) masked.
fn masked(dvi: &[u8]) -> Vec<u8> {
    let mut d = dvi.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'.';
        }
    }
    d
}

/// The document on the graph at schedule `o`: its DVI file and written
/// files are `latex`'s.
fn doc_at(name: &str, o: Options) {
    let want = recorded(name);
    let mut ini = None;
    let (mut inputs, mut fonts) = (vec![latex_dir()], Vec::new());
    for l in &want.dirs {
        match l.split_once(' ') {
            Some(("ini", p)) => ini = Some(p.to_string()),
            Some(("input", d)) => inputs.push(PathBuf::from(d)),
            Some(("font", d)) => fonts.push(PathBuf::from(d)),
            _ => {}
        }
    }
    // (the format's directories: those `latex.ltx` was read from)
    for d in std::fs::read_to_string(latex_dir().join("dirs.txt")).expect("tests/latex/dirs.txt").lines() {
        let d = PathBuf::from(d);
        if !inputs.contains(&d) {
            inputs.push(d);
        }
    }
    let ini = chars(&std::fs::read(ini.expect("ini in NAME.dirs")).expect("latex.ini"));
    let doc = chars(&std::fs::read(docs_dir().join(format!("{name}.tex"))).expect("NAME.tex"));
    // (and the TeX Live tree's, for the fonts the format loads)
    fonts.extend(phitex_flow::tfm::default_dirs());
    let mut b = Build::new(font());
    b.set_font_dirs(fonts);
    b.set_input_dirs(inputs);
    b.set_jobname(name);
    b.set_terminal(&["latex.ini", &format!("&latex {name}")]);
    b.set_limits(Limits::TEX_LIVE);
    let t0 = std::time::Instant::now();
    let out = b.update(&jobs(&[&ini, &doc]), o);
    eprintln!("{name} ({o:?}): {:.1} s", t0.elapsed().as_secs_f64());
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-docs");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join(format!("{name}.dvi")), &out.dvi);
    for (i, l) in out.job_logs(&Limits::TEX_LIVE, &format!("{name}.dvi")).iter().enumerate() {
        let _ = std::fs::write(d.join(format!("{name}.{i}.log")), &l.log);
    }
    // (a core gap, CORE.md "LaTeX documents": `\enddocument` reads back the
    // `.aux` file the job wrote and closed, `\@@input\jobname.aux`; a file
    // is read from the input directories only, so it is not found and,
    // in batch mode, the terminal TeX asks for another name ends the job:
    // after the last page is shipped and the `.aux` file written)
    let aux = format!("I can't find file `{name}.aux'");
    let gap = out.errors.iter().any(|e| e.ends_with(&aux));
    let errors: Vec<&String> = out
        .errors
        .iter()
        .filter(|e| !(gap && (e.ends_with(&aux) || e.ends_with("End of file on the terminal!"))))
        .collect();
    assert!(errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", &errors[..errors.len().min(20)]);
    // (the same gap: the `.toc` lines are written while the `.aux` file is
    // read back, `\@writefile`; without it the `.toc` stays empty)
    let writes: Vec<(String, String)> = out
        .writes
        .iter()
        .filter(|(n, _)| n != "texsys.aux")
        .map(|(n, t)| match want.writes.iter().find(|(m, _)| m == n) {
            Some((_, w)) if gap && Path::new(n).extension().is_some_and(|e| e == "toc") && t.is_empty() => {
                (n.clone(), w.clone())
            }
            _ => (n.clone(), t.clone()),
        })
        .collect();
    assert_eq!(writes, want.writes, "{name} ({o:?}): the written files are not latex's");
    assert!(
        masked(&out.dvi) == masked(&want.dvi),
        "{name} ({o:?}): the DVI file is not latex's ({} bytes, latex's {}; ours in {})",
        out.dvi.len(),
        want.dvi.len(),
        d.display()
    );
}

/// One document at a time (each schedule a process of its own holds a
/// build of the format).
static ONE: Mutex<()> = Mutex::new(());

fn doc(name: &str) {
    if let Ok(k) = std::env::var("PHITEX_FLOW_DOC_RUN") {
        doc_at(name, RUNS[k.parse::<usize>().expect("a schedule")]);
        return;
    }
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for (k, o) in RUNS.iter().enumerate() {
        let st = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", &format!("doc_{name}"), "--nocapture", "--test-threads=1"])
            .env("PHITEX_FLOW_DOC_RUN", k.to_string())
            .status()
            .expect("the test binary runs");
        assert!(st.success(), "{name} ({o:?}) failed on the graph (its message above)");
    }
}

#[test]
fn doc_sections() {
    doc("sections");
}

#[test]
fn doc_math() {
    doc("math");
}

#[test]
fn doc_tables() {
    doc("tables");
}

#[test]
fn doc_figures() {
    doc("figures");
}

#[test]
fn doc_titlepage() {
    doc("titlepage");
}

#[test]
fn doc_packages() {
    doc("packages");
}
