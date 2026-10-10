//! LaTeX documents on the graph, against TeX Live's `latex` (pdfTeX in DVI
//! mode with the `latex` format) on each document's first run, with no
//! `.aux` present: references print `??` on both sides, so the cross-run
//! fixpoint is not needed.
//!
//! Our side is a build on the saved format (`Build::set_format`): the
//! definitions `latex.ini`'s job (as `fmtutil` runs it: `pdftexconfig`,
//! `\pdfoutput=0`, `latex.ltx`, `\dump`) reaches at its `\dump`, made once
//! per test run (`Build::format`), given to the document's build as
//! definitions before its first piece; the document's job is `&latex
//! NAME`, with the directories the oracle read its files from (`latex
//! -recorder`) and the `texsys.aux` the format's job writes and reads back
//! (`tests/latex/`, CORE.md). Compared: the DVI file, its preamble's dated
//! comment masked, and the files the document's job writes (its `.aux`,
//! `.toc`); the log is diagnostics only. `format_is_the_prelude` holds the
//! format to the session it stands for (`latex.ini`'s job, then the
//! document's, in one build), byte for byte, log included.
//!
//! The documents are in `tests/latex/docs/`, each with what `latex` made:
//! `NAME.dvi`, `NAME.writes` (each file written: a line `=== FILE N`, then
//! its N characters) and `NAME.dirs` (`ini PATH`: `latex.ini`; `input
//! DIR`, `font DIR`). Where `latex` runs they are checked against a fresh
//! run (`oracles_are_latex`); `PHITEX_FLOW_BLESS=1` records them anew.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use phitex_flow::build::MAIN_MEMORY;
use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Font, Options, Order, Output, Reads};

/// The documents.
const DOCS: [&str; 6] = ["sections", "math", "tables", "figures", "titlepage", "packages"];

/// The schedules: one thread; eight suspending, in priority order and in
/// a random one; eight speculating, in priority order and in a random one.
const RUNS: [Options; 5] = [
    run(1, Reads::Suspend, Order::Priority),
    run(8, Reads::Suspend, Order::Priority),
    run(8, Reads::Speculate, Order::Priority),
    run(8, Reads::Suspend, Order::Random(1)),
    run(8, Reads::Speculate, Order::Random(7)),
];

const fn run(threads: usize, reads: Reads, order: Order) -> Options {
    Options { threads, reads, order, dump: false, etex: true, main_memory: MAIN_MEMORY, cut_lines: phitex_flow::build::CUT_LINES }
}

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

/// The directories a document's oracle read from (its `NAME.dirs`, then
/// the format's, `tests/latex/dirs.txt`, then the TeX Live tree's fonts),
/// and `latex.ini`.
fn dirs_of(name: &str) -> (Vec<PathBuf>, Vec<PathBuf>, PathBuf) {
    let want = recorded(name);
    let mut ini = None;
    let (mut inputs, mut fonts) = (vec![latex_dir()], Vec::new());
    for l in &want.dirs {
        match l.split_once(' ') {
            Some(("ini", p)) => ini = Some(PathBuf::from(p)),
            Some(("input", d)) => inputs.push(PathBuf::from(d)),
            Some(("font", d)) => fonts.push(PathBuf::from(d)),
            _ => {}
        }
    }
    for d in std::fs::read_to_string(latex_dir().join("dirs.txt")).expect("tests/latex/dirs.txt").lines() {
        let d = PathBuf::from(d);
        if !inputs.contains(&d) {
            inputs.push(d);
        }
    }
    fonts.extend(phitex_flow::tfm::default_dirs());
    (inputs, fonts, ini.expect("ini in NAME.dirs"))
}

/// A build reading what document `name`'s oracle read, as job `job`.
fn build_for(name: &str, job: &str) -> Build {
    let (inputs, fonts, _) = dirs_of(name);
    let mut b = Build::new(font());
    b.set_font_dirs(fonts);
    b.set_input_dirs(inputs);
    b.set_jobname(job);
    b.set_limits(Limits::TEX_LIVE);
    b
}

fn doc_text(name: &str) -> String {
    chars(&std::fs::read(docs_dir().join(format!("{name}.tex"))).expect("NAME.tex"))
}

/// One heavy build at a time (a build of `latex.ltx` holds about 4 GB).
static ONE: Mutex<()> = Mutex::new(());

/// The saved format, made once: `latex.ini`'s job (`fmtutil`'s
/// `pdftexconfig`, `\pdfoutput=0`, `latex.ltx`, `\dump`) as job `latex`,
/// its definitions at the `\dump` (`Build::format`).
static FORMAT: Mutex<Option<Arc<Vec<u8>>>> = Mutex::new(None);

fn format_file() -> Arc<Vec<u8>> {
    let mut f = FORMAT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(b) = &*f {
        return b.clone();
    }
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, _, ini) = dirs_of("sections");
    let ini = chars(&std::fs::read(ini).expect("latex.ini"));
    let mut b = build_for("sections", "latex");
    b.set_terminal(&["latex.ini"]);
    let t0 = std::time::Instant::now();
    let out = b.update(&jobs(&[&ini]), RUNS[0]);
    assert!(out.errors.is_empty(), "latex.ini's job: {:?}", &out.errors[..out.errors.len().min(20)]);
    let t1 = std::time::Instant::now();
    let bytes = Arc::new(b.format().expect("the format"));
    eprintln!(
        "latex.fmt: built in {:.1} s, saved in {:.0} ms, {} bytes",
        (t1 - t0).as_secs_f64(),
        t1.elapsed().as_secs_f64() * 1e3,
        bytes.len()
    );
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-docs");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("latex.fmt"), &*bytes);
    *f = Some(bytes.clone());
    bytes
}

/// Document `name` on the saved format at schedule `o`, as job `job`
/// (the terminal's line `&latex NAME`).
fn on_format(name: &str, job: &str, text: &str, o: Options) -> (Build, Output) {
    let fmt = format_file();
    let mut b = build_for(name, job);
    let t0 = std::time::Instant::now();
    b.set_format(&fmt).expect("the format reads");
    let t1 = std::time::Instant::now();
    b.set_terminal(&[&format!("&latex {name}")]);
    let out = b.update(&jobs(&[text]), o);
    eprintln!(
        "{name} ({o:?}): format read in {:.0} ms, cold {:.0} ms",
        (t1 - t0).as_secs_f64() * 1e3,
        t1.elapsed().as_secs_f64() * 1e3
    );
    (b, out)
}

/// The errors but the core gap's (CORE.md "LaTeX documents": `\enddocument`
/// reads back the `.aux` file the job wrote and closed,
/// `\@@input\jobname.aux`; a file is read from the input directories only,
/// so it is not found and, in batch mode, the terminal TeX asks for
/// another name ends the job: after the last page is shipped and the
/// `.aux` file written), and whether the gap was met.
fn errors_but_gap<'a>(out: &'a Output, job: &str) -> (Vec<&'a String>, bool) {
    let aux = format!("I can't find file `{job}.aux'");
    let gap = out.errors.iter().any(|e| e.ends_with(&aux));
    let errors = out
        .errors
        .iter()
        .filter(|e| !(gap && (e.ends_with(&aux) || e.ends_with("End of file on the terminal!"))))
        .collect();
    (errors, gap)
}

/// The document on the saved format at schedule `o`: its DVI file and
/// written files are `latex`'s.
fn doc_at(name: &str, o: Options) {
    let want = recorded(name);
    let (_, out) = on_format(name, name, &doc_text(name), o);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("latex-docs");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join(format!("{name}.dvi")), &out.dvi);
    for (i, l) in out.job_logs(&Limits::TEX_LIVE, &format!("{name}.dvi")).iter().enumerate() {
        let _ = std::fs::write(d.join(format!("{name}.{i}.log")), &l.log);
    }
    let (errors, gap) = errors_but_gap(&out, name);
    assert!(errors.is_empty(), "{name} ({o:?}): errors on the graph: {:?}", &errors[..errors.len().min(20)]);
    // (the same gap: the `.toc` lines are written while the `.aux` file is
    // read back, `\@writefile`; without it the `.toc` stays empty)
    let writes: Vec<(String, String)> = out
        .writes
        .iter()
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

fn doc(name: &str) {
    for o in RUNS {
        doc_at(name, o);
    }
}

/// A document on the saved format is the same document after
/// `latex.ini`'s job in one session (the format's source run as prelude
/// text): the same DVI file, written files and log, byte for byte, at
/// every schedule (the session's at one thread: its schedules are
/// `latex_format`'s and the session tests' business).
#[test]
fn format_is_the_prelude() {
    let name = "sections";
    let text = doc_text(name);
    let fmt = format_file();
    let session = {
        let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (_, _, ini) = dirs_of(name);
        let ini = chars(&std::fs::read(ini).expect("latex.ini"));
        let mut b = build_for(name, "latex");
        b.set_terminal(&["latex.ini", &format!("&latex {name}")]);
        b.update(&jobs(&[&ini, &text]), RUNS[0])
    };
    assert!(!fmt.is_empty());
    let logs = session.job_logs(&Limits::TEX_LIVE, "latex.dvi");
    assert_eq!(logs.len(), 2, "two jobs");
    let writes: Vec<(String, String)> = session.writes.iter().filter(|(n, _)| n != "texsys.aux").cloned().collect();
    for o in RUNS {
        let (_, out) = on_format(name, "latex", &text, o);
        assert!(out.dvi == session.dvi, "{name} ({o:?}): the DVI file is not the session's");
        assert_eq!(out.writes, writes, "{name} ({o:?}): the written files are not the session's");
        let log = out.job_logs(&Limits::TEX_LIVE, "latex.dvi");
        assert_eq!(log.len(), 1, "one job");
        assert_eq!(log[0].log, logs[1].log, "{name} ({o:?}): the log is not the session's second job's");
        // (an error names its piece by position: the session's has the
        // format's job's pieces before it)
        let bare = |es: &[String]| -> Vec<String> {
            es.iter()
                .map(|e| {
                    e.split_once(": ")
                        .filter(|(p, _)| p.starts_with("piece "))
                        .map_or(e.clone(), |(_, m)| m.to_string())
                })
                .collect()
        };
        assert_eq!(bare(&out.errors), bare(&session.errors), "{name} ({o:?}): the errors are not the session's");
    }
}

/// An edit on a build that started from the saved format is the same as
/// a cold build of the edited text on it (DVI, written files, log), at
/// every schedule.
#[test]
fn format_edit_is_cold() {
    let name = "sections";
    let text = doc_text(name);
    let edited = text.replacen("bold words", "strong words", 1).replacen(
        "\\subsection{Lists}",
        "A paragraph put in, long enough to take a line of its own and a little more than that.\n\n\\subsection{Lists}",
        1,
    );
    assert_ne!(edited, text);
    let fmt = format_file();
    for o in RUNS {
        let (mut b, _) = on_format(name, name, &text, o);
        let e = b.update(&jobs(&[&edited]), o);
        let mut c = build_for(name, name);
        c.set_format(&fmt).expect("the format reads");
        c.set_terminal(&[&format!("&latex {name}")]);
        let cold = c.update(&jobs(&[&edited]), o);
        assert!(e.dvi == cold.dvi, "{name} ({o:?}): the edit's DVI file is not the cold build's");
        assert_eq!(e.writes, cold.writes, "{name} ({o:?}): the edit's written files are not the cold build's");
        assert_eq!(e.log, cold.log, "{name} ({o:?}): the edit's log is not the cold build's");
        assert_eq!(e.errors, cold.errors, "{name} ({o:?}): the edit's errors are not the cold build's");
    }
}

/// The format's file reads back as the same definitions: written again,
/// the same bytes.
#[test]
fn format_reads_back() {
    let fmt = format_file();
    let cs = phitex_flow::build::CsTable::new();
    let f = phitex_flow::format::decode(&fmt, &cs).expect("the format reads");
    let again = phitex_flow::format::encode(&f.defs, &f.fonts, &cs).expect("the format writes");
    assert!(again == *fmt, "the format written again differs ({} bytes, {} before)", again.len(), fmt.len());
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
