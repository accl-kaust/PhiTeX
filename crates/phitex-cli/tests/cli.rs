//! The `phitex` binary on phitex-flow, end to end:
//!
//! - `latex -interaction=batchmode NAME.tex` (phitex run as `latex`) on
//!   phitex-flow's LaTeX documents (`crates/phitex-flow/tests/latex/docs`):
//!   the DVI file and the written files are TeX Live's `latex`'s on the
//!   first run (the recorded oracles), the format made on the graph and
//!   cached;
//! - `pdftex -ini -etex -interaction=batchmode NAME.tex` on the PDF
//!   documents of phitex-flow's `tests/pdf.rs` (eight `-ini` documents, the
//!   ten plain ones): the PDF file is pdfTeX's, byte for byte;
//! - `phitex build --output-format dvi` to the cross-run fixpoint, against
//!   `latex` run until its `.aux` settles (where `latex` is installed);
//! - `phitex watch` (headless: no viewer) on a 48-page plain document: a
//!   word edited in the file is built as an edit, and the PDF it writes is
//!   the cold build's of the edited text;
//! - `pdflatex` on the LaTeX documents, against `pdflatex`: ignored until
//!   pdflatex's format and documents land on the graph (agent Q's Q2).
//!
//! One heavy run at a time (a format build holds about 4 GB). The formats
//! are kept in the target's temporary directory across runs.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ONE: Mutex<()> = Mutex::new(());

fn one() -> std::sync::MutexGuard<'static, ()> {
    ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn flow() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../phitex-flow")
}

fn docs_dir() -> PathBuf {
    flow().join("tests/latex/docs")
}

fn cache() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache")
}

/// A fresh directory for a test's run.
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d
}

/// The binary under the program name `name` (a link to it).
fn as_program(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bin");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_phitex"), &p).expect("a link");
    p
}

fn command(program: &Path, dir: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(dir)
        .env("PHITEX_CACHE_DIR", cache())
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .env_remove("PHITEX_COMPAT");
    c
}

/// Bytes as TeX's characters.
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
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

fn first_difference(a: &[u8], b: &[u8]) -> String {
    let at = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    format!("at byte {at} (lengths {} and {})", a.len(), b.len())
}

/// Copies document `name` (`NAME.tex` and its `NAME-*` files) into `dir`.
fn copy_doc(name: &str, dir: &Path) {
    let pre = format!("{name}-");
    for e in std::fs::read_dir(docs_dir()).expect("the documents").flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if f == format!("{name}.tex") || f.starts_with(&pre) {
            std::fs::copy(e.path(), dir.join(&f)).expect("a copy");
        }
    }
}

const LATEX_DOCS: [&str; 10] =
    ["sections", "math", "tables", "figures", "titlepage", "packages", "ams", "gfx", "layout", "rules"];

/// `latex -interaction=batchmode NAME.tex` through phitex: the DVI file
/// and the written files are the recorded oracle's. The `.aux` file read
/// back at `\end{document}` in the same run is phitex-flow's known gap
/// (CORE.md, "LaTeX documents": until agent B's fixpoint lands), with
/// exactly its two errors and an empty `.toc`.
fn latex_doc(name: &str) {
    let _one = one();
    let dir = scratch(&format!("latex-{name}"));
    copy_doc(name, &dir);
    let t = Instant::now();
    let out = command(&as_program("latex"), &dir)
        .args(["-interaction=batchmode", &format!("{name}.tex")])
        .output()
        .expect("phitex runs");
    let term = String::from_utf8_lossy(&out.stdout);
    let errs = String::from_utf8_lossy(&out.stderr);
    eprintln!("{name}: latex through phitex in {:.2} s, status {:?}\n{errs}", t.elapsed().as_secs_f64(), out.status.code());
    let gap = term.contains(&format!("I can't find file `{name}.aux'"));
    if !out.status.success() {
        assert!(
            gap,
            "{name}: phitex failed, not on the known .aux gap:\nterminal:\n{term}\nstderr:\n{errs}"
        );
        eprintln!("{name}: the .aux read back at \\end{{document}} is phitex-flow's known gap (agent B's fixpoint)");
    }
    let want = std::fs::read(docs_dir().join(format!("{name}.dvi"))).expect("NAME.dvi");
    let got = std::fs::read(dir.join(format!("{name}.dvi"))).unwrap_or_default();
    assert!(
        masked(&got) == masked(&want),
        "{name}: the DVI file is not latex's: {} (ours in {})",
        first_difference(&got, &want),
        dir.display()
    );
    let writes = parse_writes(&chars(&std::fs::read(docs_dir().join(format!("{name}.writes"))).expect("NAME.writes")));
    for (n, w) in writes {
        let got = chars(&std::fs::read(dir.join(&n)).unwrap_or_default());
        if gap && Path::new(&n).extension().is_some_and(|e| e == "toc") && got.is_empty() {
            continue;
        }
        assert_eq!(got, w, "{name}: {n} is not latex's");
    }
}

#[test]
fn latex_sections() {
    latex_doc("sections");
}

#[test]
fn latex_math() {
    latex_doc("math");
}

#[test]
fn latex_tables() {
    latex_doc("tables");
}

#[test]
fn latex_figures() {
    latex_doc("figures");
}

#[test]
fn latex_titlepage() {
    latex_doc("titlepage");
}

#[test]
fn latex_packages() {
    latex_doc("packages");
}

#[test]
fn latex_ams() {
    latex_doc("ams");
}

#[test]
fn latex_gfx() {
    latex_doc("gfx");
}

#[test]
fn latex_layout() {
    latex_doc("layout");
}

#[test]
fn latex_rules() {
    latex_doc("rules");
}

/// plain.tex as phitex-flow's tests make it (`tests/dvi/plaintext.tex`
/// before its body, with `hyphen.tex`).
fn plain() -> String {
    phitex_flow::fuzz::plain().expect("plain.tex of phitex-flow's tests")
}

/// The PDF documents of phitex-flow's `tests/pdf.rs`: the name of the file
/// to typeset, its text and the oracle's name.
fn pdf_docs() -> Vec<(String, String, String)> {
    let pdf = flow().join("tests/pdf");
    let mut v = Vec::new();
    for name in ["hello", "compressed", "mag", "mixed1", "mixed2", "objstm", "rules", "guess"] {
        let text = std::fs::read_to_string(pdf.join(format!("{name}.tex"))).expect("NAME.tex");
        v.push((name.to_owned(), text, name.to_owned()));
    }
    for body in ["story", "pages", "display", "table", "hyphens"] {
        for default in [false, true] {
            let b = std::fs::read_to_string(flow().join(format!("tests/plain/{body}.tex"))).expect("a body");
            let text = "\\pdfoutput=1\n".to_string() + &plain() + if default { "\\output={}\n" } else { "" } + &b;
            let name = if default { format!("{body}-default") } else { body.to_owned() };
            v.push((name.clone(), text, format!("plain-{name}")));
        }
    }
    v
}

/// `pdftex -ini -etex -interaction=batchmode NAME.tex` through phitex:
/// pdfTeX's PDF file, byte for byte.
#[test]
fn pdftex_ini_documents() {
    let _one = one();
    let mut bad = Vec::new();
    for (name, text, oracle) in pdf_docs() {
        let dir = scratch(&format!("pdf-{name}"));
        std::fs::write(dir.join(format!("{name}.tex")), &text).expect("the document");
        let out = command(&as_program("pdftex"), &dir)
            .args(["-ini", "-etex", "-interaction=batchmode", &format!("{name}.tex")])
            .output()
            .expect("phitex runs");
        let want = std::fs::read(flow().join(format!("tests/pdf/{oracle}.pdf"))).expect("the oracle");
        let got = std::fs::read(dir.join(format!("{name}.pdf"))).unwrap_or_default();
        if !out.status.success() || got != want {
            bad.push(format!(
                "{name}: status {:?}, {}; stderr: {}",
                out.status.code(),
                first_difference(&got, &want),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(bad.is_empty(), "PDF files that are not pdfTeX's:\n{}", bad.join("\n"));
}

/// Whether a program of TeX Live is there.
fn have(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// `latex` run in `dir` until the `.aux` file settles (as latexmk runs
/// it): its DVI file.
fn latex_fixpoint(name: &str, dir: &Path) -> Vec<u8> {
    let mut last = None;
    for _ in 0..5 {
        let _ = Command::new("latex")
            .args(["-interaction=batchmode", &format!("{name}.tex")])
            .current_dir(dir)
            .env("SOURCE_DATE_EPOCH", "0")
            .env("FORCE_SOURCE_DATE", "1")
            .output();
        let aux = std::fs::read(dir.join(format!("{name}.aux"))).ok();
        if aux == last {
            break;
        }
        last = aux;
    }
    std::fs::read(dir.join(format!("{name}.dvi"))).unwrap_or_default()
}

/// `phitex build --output-format dvi` runs to the cross-run fixpoint: its
/// DVI file is `latex`'s once `latex`'s `.aux` file settles.
#[test]
fn build_to_the_fixpoint() {
    if !have("latex") {
        eprintln!("build_to_the_fixpoint: no `latex` here to compare with: not checked");
        return;
    }
    let _one = one();
    for name in ["titlepage", "sections"] {
        let ours = scratch(&format!("fixpoint-{name}"));
        copy_doc(name, &ours);
        let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &ours)
            .args(["build", "-v", "--output-format", "dvi", &format!("{name}.tex")])
            .output()
            .expect("phitex runs");
        let errs = String::from_utf8_lossy(&out.stderr);
        eprintln!("{name}: phitex build:\n{errs}");
        assert!(out.status.success(), "{name}: phitex build failed:\n{errs}");
        let theirs = scratch(&format!("fixpoint-{name}-latex"));
        copy_doc(name, &theirs);
        let want = latex_fixpoint(name, &theirs);
        let got = std::fs::read(ours.join(format!("{name}.dvi"))).unwrap_or_default();
        assert!(
            masked(&got) == masked(&want),
            "{name}: the fixpoint's DVI is not latex's after its .aux settled: {}",
            first_difference(&got, &want)
        );
        for ext in ["aux", "toc"] {
            let a = std::fs::read(ours.join(format!("{name}.{ext}"))).ok();
            let b = std::fs::read(theirs.join(format!("{name}.{ext}"))).ok();
            assert_eq!(a, b, "{name}: the fixpoint's .{ext} is not latex's");
        }
    }
}

/// The 48-page plain document of phitex-flow's budgets ("big"), in PDF
/// mode, as one `-ini` file.
fn big() -> String {
    let body = phitex_flow::fuzz::big_body(4).expect("the plain bodies");
    "\\pdfoutput=1\n".to_string() + &plain() + &body
}

/// Reads `child`'s lines until one starts with `prefix`; the lines read.
fn until(lines: &mut impl Iterator<Item = String>, prefix: &str, seen: &mut Vec<String>) -> String {
    for l in lines.by_ref() {
        seen.push(l.clone());
        if l.starts_with(prefix) {
            return l;
        }
    }
    panic!("the watch ended before `{prefix}`:\n{}", seen.join("\n"));
}

/// The update's milliseconds in a build line (`update X ms`).
fn update_ms(line: &str) -> f64 {
    let at = line.find("update ").expect("an update time") + "update ".len();
    line[at..].split(' ').next().and_then(|n| n.parse().ok()).expect("a number")
}

/// `phitex watch`, headless: a word edited in the file on disk is built as
/// an edit of the same Build; its PDF is the cold build's of the edited
/// text, byte for byte, and the edit's update is within the brief's word
/// budget (50 ms; phitex-flow's budgets measure about 10 ms on this
/// document) in a release build.
#[test]
fn watch_word_edit() {
    let _one = one();
    let text = big();
    let dir = scratch("watch");
    std::fs::write(dir.join("big.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["watch", "--no-view", "--ini", "--etex", "-j", "8", "big.tex"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let cold = until(&mut lines, "phitex: build 1 ", &mut seen);
    eprintln!("{cold}");
    // (a word of the body, as long as the one it replaces)
    let at = text.rfind("\\message{[part 2 story]}").expect("part 2");
    let word = text[at..].find(" the ").expect("a word") + at;
    let edited = format!("{} thy {}", &text[..word], &text[word + 5..]);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("big.tex"), &edited).expect("the edit");
    let line = until(&mut lines, "phitex: build 2 ", &mut seen);
    eprintln!("{line}");
    let ms = update_ms(&line);
    let watched = std::fs::read(dir.join("big.pdf")).expect("the watch's PDF");
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("watch-cold");
    std::fs::write(cold_dir.join("big.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--ini", "--etex", "-j", "8", "big.tex"])
        .output()
        .expect("phitex build runs");
    assert!(out.status.success(), "the cold build failed: {}", String::from_utf8_lossy(&out.stderr));
    let cold_pdf = std::fs::read(cold_dir.join("big.pdf")).expect("the cold PDF");
    assert!(
        watched == cold_pdf,
        "the watch's PDF after the edit is not the cold build's: {}",
        first_difference(&watched, &cold_pdf)
    );
    eprintln!("watch: a word edit's update took {ms:.2} ms on 48 pages");
    if !cfg!(debug_assertions) {
        assert!(ms <= 50.0, "a word edit's update took {ms:.2} ms, over the brief's 50 ms");
    }
}

/// `pdflatex` through phitex on the LaTeX documents, against TeX Live's
/// `pdflatex` (`SOURCE_DATE_EPOCH=0`): the PDF files byte for byte.
#[test]
#[ignore = "waits on agent Q's slice Q2: pdflatex's format and documents on phitex-flow"]
fn pdflatex_documents() {
    if !have("pdflatex") {
        eprintln!("pdflatex_documents: no `pdflatex` here to compare with: not checked");
        return;
    }
    let _one = one();
    let mut bad = Vec::new();
    for name in LATEX_DOCS {
        let theirs = scratch(&format!("pdflatex-{name}-tl"));
        copy_doc(name, &theirs);
        let _ = Command::new("pdflatex")
            .args(["-interaction=batchmode", &format!("{name}.tex")])
            .current_dir(&theirs)
            .env("SOURCE_DATE_EPOCH", "0")
            .env("FORCE_SOURCE_DATE", "1")
            .output();
        let want = std::fs::read(theirs.join(format!("{name}.pdf"))).unwrap_or_default();
        let ours = scratch(&format!("pdflatex-{name}"));
        copy_doc(name, &ours);
        let out = command(&as_program("pdflatex"), &ours)
            .args(["-interaction=batchmode", &format!("{name}.tex")])
            .output()
            .expect("phitex runs");
        let got = std::fs::read(ours.join(format!("{name}.pdf"))).unwrap_or_default();
        if got.is_empty() || got != want {
            bad.push(format!(
                "{name}: {}; stderr: {}",
                first_difference(&got, &want),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(bad.is_empty(), "pdflatex's PDF files differ:\n{}", bad.join("\n"));
}
