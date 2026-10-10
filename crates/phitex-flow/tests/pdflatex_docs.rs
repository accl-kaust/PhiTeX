//! LaTeX documents in PDF mode on the graph, against TeX Live's
//! `pdflatex` on each document's first run (`SOURCE_DATE_EPOCH=0
//! FORCE_SOURCE_DATE=1`, no `.aux` present): the PDF file byte for byte
//! and the files the job writes, at five schedules. The documents are
//! `tests/latex_docs.rs`'s (`tests/latex/docs/`); our side is a build on
//! the saved format of `pdflatex.ini`'s job (as `fmtutil` runs it:
//! `pdftexconfig`, `latex.ltx`, `\dump`, job `pdflatex`), with the
//! directories the oracle read from (`pdflatex -recorder`). The oracles are
//! in `tests/latex/pdf/` (`NAME.pdf`, `NAME.writes`, `NAME.dirs`), checked
//! against a fresh `pdflatex` run where there is one
//! (`oracles_are_pdflatex`); `PHITEX_FLOW_BLESS=1` records them anew.

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use phitex_flow::build::MAIN_MEMORY;
use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Font, Options, Order, Output, Reads};

/// The documents.
const DOCS: [&str; 10] =
    ["sections", "math", "tables", "figures", "titlepage", "packages", "ams", "gfx", "layout", "rules"];

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

/// The documents of pdfTeX's PDF-only commands (`tests/latex/pdfdocs/`):
/// hyperref, graphicx's three kinds of image, xcolor, TikZ, positions
/// written to the `.aux` file, objects and forms by hand.
const PDFDOCS: [&str; 7] = ["hyper", "graphics", "color", "tikz", "zsavepos", "objs", "vfont"];

/// The directory of document `name`.
fn doc_dir(name: &str) -> PathBuf {
    if PDFDOCS.contains(&name) { latex_dir().join("pdfdocs") } else { docs_dir() }
}

fn docs_dir() -> PathBuf {
    latex_dir().join("docs")
}

fn pdf_dir() -> PathBuf {
    latex_dir().join("pdf")
}

fn font() -> Arc<Font> {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
    Arc::new(Font::parse("cmr10", &b).unwrap())
}

/// Bytes as TeX's characters (one each).
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// What `pdflatex` made of a document.
#[derive(PartialEq, Debug, Default)]
struct Made {
    pdf: Vec<u8>,
    writes: Vec<(String, String)>,
    /// `ini PATH`, `input DIR`, `font DIR` lines.
    dirs: Vec<String>,
}

/// A document's files: `NAME.tex` and the `NAME-*` files it reads (a
/// picture).
fn doc_files(name: &str) -> Vec<String> {
    let mut v = vec![format!("{name}.tex")];
    let pre = format!("{name}-");
    for e in std::fs::read_dir(doc_dir(name)).expect("tests/latex/docs").flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if f.starts_with(&pre) {
            v.push(f);
        }
    }
    v.sort();
    v
}

fn kpsewhich(name: &str) -> Option<String> {
    let o = Command::new("kpsewhich").arg(name).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// `pdflatex NAME` in a directory of its own, if there is a `pdflatex`.
fn run_latex(name: &str) -> Option<Made> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-pdflatex-doc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    for f in doc_files(name) {
        std::fs::copy(doc_dir(name).join(&f), tmp.join(&f)).ok()?;
    }
    let st = Command::new("pdflatex")
        .args(["-interaction=batchmode", "-recorder", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(&tmp)
        .output()
        .ok()?;
    let fls = std::fs::read_to_string(tmp.join(format!("{name}.fls"))).unwrap_or_default();
    let pdf = std::fs::read(tmp.join(format!("{name}.pdf"))).ok();
    let mut made = Made::default();
    let mut add = |l: String| {
        if !made.dirs.contains(&l) {
            made.dirs.push(l);
        }
    };
    let ini = kpsewhich("pdflatex.ini")?;
    add(format!("ini {ini}"));
    for f in ["pdflatex.ini", "pdftexconfig.tex"] {
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
            if ["log", "pdf", "fls"].contains(&ext.as_str()) || made.writes.iter().any(|(n, _)| n == f) {
                continue;
            }
            let text = std::fs::read(tmp.join(f)).unwrap_or_default();
            made.writes.push((f.to_string(), chars(&text)));
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(st.status.success(), "pdflatex failed on {name}");
    made.pdf = pdf?;
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
    let d = pdf_dir();
    Made {
        pdf: std::fs::read(d.join(format!("{name}.pdf"))).expect("NAME.pdf"),
        writes: parse_writes(&chars(&std::fs::read(d.join(format!("{name}.writes"))).expect("NAME.writes"))),
        dirs: std::fs::read_to_string(d.join(format!("{name}.dirs")))
            .expect("NAME.dirs")
            .lines()
            .map(str::to_string)
            .collect(),
    }
}

/// The recorded oracles are what `pdflatex` makes now;
/// `PHITEX_FLOW_BLESS=1` records them.
#[test]
fn oracles_are_pdflatex() {
    // (every document, the failures listed at the end: one blessing run
    // records all it can)
    let mut bad = Vec::new();
    for name in DOCS.into_iter().chain(PDFDOCS) {
        let made = match std::panic::catch_unwind(|| run_latex(name)) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(_) => {
                bad.push(format!("{name}: pdflatex failed"));
                continue;
            }
        };
        if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
            let d = pdf_dir();
            let _ = std::fs::create_dir_all(&d);
            std::fs::write(d.join(format!("{name}.pdf")), &made.pdf).unwrap();
            let w: Vec<u8> = writes_text(&made.writes).chars().map(|c| u8::try_from(c).expect("a byte")).collect();
            std::fs::write(d.join(format!("{name}.writes")), w).unwrap();
            std::fs::write(d.join(format!("{name}.dirs")), made.dirs.join("\n") + "\n").unwrap();
        }
        let want = recorded(name);
        if made.pdf != want.pdf {
            bad.push(format!("{name}: pdflatex's PDF is not the recorded one"));
        }
        if made.writes != want.writes {
            bad.push(format!("{name}: pdflatex's written files are not the recorded ones"));
        }
        if made.dirs != want.dirs {
            bad.push(format!("{name}: pdflatex read its files from other directories"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// The directories a document's oracle read from (its `NAME.dirs`, then
/// the format's, `tests/latex/dirs.txt`, then the TeX Live tree's fonts),
/// and `latex.ini`.
fn dirs_of(name: &str) -> (Vec<PathBuf>, Vec<PathBuf>, PathBuf) {
    let want = recorded(name);
    let mut ini = None;
    // (the document's own directory first: its pictures, as `latex` finds
    // them beside it)
    let (mut inputs, mut fonts) = (vec![doc_dir(name), latex_dir()], Vec::new());
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
    common::give_kpse(&mut b, "pdflatex");
    b.set_input_dirs(inputs);
    b.set_jobname(job);
    b.set_limits(Limits::TEX_LIVE);
    b.set_pdf_date(0);
    // (as the oracle runs: `latex -interaction=batchmode`, over the
    // format's `\errorstopmode` at the document's job's start)
    b.set_interaction_option(Some(0));
    b
}

fn doc_text(name: &str) -> String {
    chars(&std::fs::read(doc_dir(name).join(format!("{name}.tex"))).expect("NAME.tex"))
}

/// One heavy build at a time (a build of `latex.ltx` holds about 4 GB).
static ONE: Mutex<()> = Mutex::new(());

/// The saved format, made once: `pdflatex.ini`'s job (`pdftexconfig`,
/// `latex.ltx`, `\dump`) as job `pdflatex`, its definitions at the
/// `\dump` (`Build::format`).
static FORMAT: Mutex<Option<Arc<Vec<u8>>>> = Mutex::new(None);

fn format_file() -> Arc<Vec<u8>> {
    let mut f = FORMAT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(b) = &*f {
        return b.clone();
    }
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, _, ini) = dirs_of("sections");
    let ini = chars(&std::fs::read(ini).expect("pdflatex.ini"));
    let mut b = build_for("sections", "pdflatex");
    b.set_terminal(&["pdflatex.ini"]);
    let t0 = std::time::Instant::now();
    let out = b.update(&jobs(&[&ini]), RUNS[0]);
    assert!(out.errors.is_empty(), "pdflatex.ini's job: {:?}", &out.errors[..out.errors.len().min(20)]);
    let t1 = std::time::Instant::now();
    let bytes = Arc::new(b.format().expect("the format"));
    eprintln!(
        "pdflatex.fmt: built in {:.1} s, saved in {:.0} ms, {} bytes",
        (t1 - t0).as_secs_f64(),
        t1.elapsed().as_secs_f64() * 1e3,
        bytes.len()
    );
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pdflatex-docs");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("pdflatex.fmt"), &*bytes);
    *f = Some(bytes.clone());
    bytes
}

/// Document `name` on the saved format at schedule `o`, as job `job`
/// (the terminal's line `&pdflatex NAME`).
fn on_format(name: &str, job: &str, text: &str, o: Options) -> (Build, Output) {
    let fmt = format_file();
    let mut b = build_for(name, job);
    let t0 = std::time::Instant::now();
    b.set_format(&fmt).expect("the format reads");
    let t1 = std::time::Instant::now();
    b.set_terminal(&[&format!("&pdflatex {name}")]);
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

/// The document on the saved format at schedule `o`: its PDF file and
/// written files are `pdflatex`'s.
fn doc_at(name: &str, o: Options) {
    let want = recorded(name);
    let (_, out) = on_format(name, name, &doc_text(name), o);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pdflatex-docs");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join(format!("{name}.pdf")), &out.pdf);
    for (i, l) in out.job_logs(&Limits::TEX_LIVE, &format!("{name}.pdf")).iter().enumerate() {
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
    assert_eq!(writes, want.writes, "{name} ({o:?}): the written files are not pdflatex's");
    if out.pdf != want.pdf {
        let at = out.pdf.iter().zip(&want.pdf).position(|(x, y)| x != y).unwrap_or(out.pdf.len().min(want.pdf.len()));
        let show = |x: &[u8]| String::from_utf8_lossy(&x[at.saturating_sub(100)..(at + 60).min(x.len())]).into_owned();
        panic!(
            "{name} ({o:?}): the PDF file is not pdflatex's ({} bytes, pdflatex's {}; ours in {}), at byte {at}:\n graph: {:?}\n pdflatex: {:?}",
            out.pdf.len(),
            want.pdf.len(),
            d.display(),
            show(&out.pdf),
            show(&want.pdf)
        );
    }
}

fn doc(name: &str) {
    for o in RUNS {
        doc_at(name, o);
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

#[test]
fn doc_ams() {
    doc("ams");
}

#[test]
fn doc_gfx() {
    doc("gfx");
}

#[test]
fn doc_layout() {
    doc("layout");
}

#[test]
fn doc_rules() {
    doc("rules");
}

#[test]
fn pdf_hyper() {
    doc("hyper");
}

#[test]
fn pdf_graphics() {
    doc("graphics");
}

#[test]
fn pdf_color() {
    doc("color");
}

#[test]
fn pdf_tikz() {
    doc("tikz");
}

#[test]
fn pdf_zsavepos() {
    doc("zsavepos");
}

#[test]
fn pdf_objs() {
    doc("objs");
}

#[test]
fn pdf_vfont() {
    doc("vfont");
}
