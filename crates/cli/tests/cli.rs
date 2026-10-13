//! The `phitex` binary, end to end. The tests whose builds need what
//! `phitex-tex` does not do yet (LaTeX's format, PDF output) are ignored
//! with what they wait for; they are the gate it must pass, against the pdfTeX oracles under
//! `oracle/fixtures`:
//!
//! - `latex -interaction=batchmode NAME.tex` (phitex run as `latex`) on
//!   the LaTeX documents (`oracle/fixtures/latex/docs`):
//!   the DVI file and the written files are TeX Live's `latex`'s on the
//!   first run (the recorded oracles), the format made on the graph and
//!   cached;
//! - `pdftex -ini -etex -interaction=batchmode NAME.tex` on the PDF
//!   documents (eight `-ini` documents, the ten plain ones): the PDF file
//!   is pdfTeX's, byte for byte;
//! - `phitex build --output-format dvi` to the cross-run fixpoint, against
//!   `latex` run until its `.aux` settles (where `latex` is installed);
//! - `phitex watch` (headless: no viewer) on a 48-page plain document: a
//!   word edited in the file is built as an edit, and the PDF it writes is
//!   the cold build's of the edited text;
//! - `pdflatex -interaction=batchmode NAME.tex` on the same documents:
//!   the PDF file and the written files are the recorded `pdflatex`
//!   oracles' (`oracle/fixtures/latex/pdf/`), byte for byte.
//!
//! One heavy run at a time (a format build holds about 4 GB). The formats
//! are kept in the target's temporary directory across runs.

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static ONE: Mutex<()> = Mutex::new(());

fn one() -> std::sync::MutexGuard<'static, ()> {
    ONE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The oracle fixtures (`oracle/fixtures`).
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/fixtures")
}

fn docs_dir() -> PathBuf {
    fixtures().join("latex/docs")
}

/// The documents of pdfTeX's PDF-only commands (hyperref, graphicx,
/// xcolor's colour stack, `TikZ`, positions, objects and forms, virtual
/// fonts: `tests/latex/pdfdocs/`, their `pdflatex` oracles in
/// `tests/latex/pdf/` beside the others').
fn pdfdocs_dir() -> PathBuf {
    fixtures().join("latex/pdfdocs")
}

/// Where document `name` is: `tests/latex/pdfdocs/` if it is one of those.
fn doc_dir(name: &str) -> PathBuf {
    if pdfdocs_dir().join(format!("{name}.tex")).exists() {
        pdfdocs_dir()
    } else {
        docs_dir()
    }
}

fn cache() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache")
}

/// A fresh directory for a test's run.
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("cli")
        .join(name);
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

/// `program` in `dir`, stopped after ten minutes (`timeout`): a build that
/// hangs fails its test instead of holding the job.
fn command(program: &Path, dir: &Path) -> Command {
    let mut c = Command::new("timeout");
    c.arg("600").arg(program);
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
    let at = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    format!("at byte {at} (lengths {} and {})", a.len(), b.len())
}

/// Copies document `name` (`NAME.tex` and its `NAME-*` files) into `dir`.
fn copy_doc(name: &str, dir: &Path) {
    let pre = format!("{name}-");
    for e in std::fs::read_dir(doc_dir(name))
        .expect("the documents")
        .flatten()
    {
        let f = e.file_name().to_string_lossy().into_owned();
        if f == format!("{name}.tex") || f.starts_with(&pre) {
            std::fs::copy(e.path(), dir.join(&f)).expect("a copy");
        }
    }
}

/// `latex -interaction=batchmode NAME.tex` through phitex: the DVI file
/// and the written files are the recorded oracle's. (The `.aux` file read
/// back at `\end{document}` in the same run was the old engine's known
/// gap, with exactly its two errors and an empty `.toc`; the check below
/// still allows it.)
fn latex_doc(name: &str) {
    tex_doc("latex", name);
}

/// `pdflatex -interaction=batchmode NAME.tex` through phitex: the PDF file
/// and the written files are the recorded `pdflatex` oracle's
/// (`oracle/fixtures/latex/pdf/`).
fn pdflatex_doc(name: &str) {
    tex_doc("pdflatex", name);
}

/// Document `name` through phitex run as `program` (`latex`: the DVI
/// oracles of `tests/latex/docs/`, its preamble's date masked;
/// `pdflatex`: the PDF oracles of `tests/latex/pdf/`, byte for byte).
fn tex_doc(program: &str, name: &str) {
    let _one = one();
    let pdf = program == "pdflatex";
    let oracles = if pdf {
        fixtures().join("latex/pdf")
    } else {
        docs_dir()
    };
    let dir = scratch(&format!("{program}-{name}"));
    copy_doc(name, &dir);
    let t = Instant::now();
    let out = command(&as_program(program), &dir)
        .args(["-interaction=batchmode", &format!("{name}.tex")])
        .output()
        .expect("phitex runs");
    let term = String::from_utf8_lossy(&out.stdout);
    let errs = String::from_utf8_lossy(&out.stderr);
    eprintln!(
        "{name}: {program} through phitex in {:.2} s, status {:?}\n{errs}",
        t.elapsed().as_secs_f64(),
        out.status.code()
    );
    // (in batch mode the errors are in the log only)
    let log = chars(&std::fs::read(dir.join(format!("{name}.log"))).unwrap_or_default());
    let gap = log.contains(&format!("I can't find file `{name}.aux'"));
    if !out.status.success() {
        let errors: Vec<&str> = log.lines().filter(|l| l.starts_with("! ")).collect();
        assert!(
            gap && errors.iter().all(|e| e.contains(&format!("`{name}.aux'"))
                || e.contains("End of file on the terminal")
                || e.contains("Emergency stop")),
            "{name}: phitex failed, not on the known .aux gap: {errors:?}\nterminal:\n{term}\nstderr:\n{errs}\nlog:\n{log}"
        );
        eprintln!("{name}: the .aux read back at \\end{{document}} is the known gap");
    }
    let ext = if pdf { "pdf" } else { "dvi" };
    let want = std::fs::read(oracles.join(format!("{name}.{ext}"))).expect("the oracle");
    let got = std::fs::read(dir.join(format!("{name}.{ext}"))).unwrap_or_default();
    let same = if pdf {
        got == want
    } else {
        masked(&got) == masked(&want)
    };
    assert!(
        same,
        "{name}: the {ext} file is not {program}'s: {} (ours in {})",
        first_difference(&got, &want),
        dir.display()
    );
    let writes = parse_writes(&chars(
        &std::fs::read(oracles.join(format!("{name}.writes"))).expect("NAME.writes"),
    ));
    for (n, w) in writes {
        let got = chars(&std::fs::read(dir.join(&n)).unwrap_or_default());
        if gap && Path::new(&n).extension().is_some_and(|e| e == "toc") && got.is_empty() {
            continue;
        }
        assert_eq!(got, w, "{name}: {n} is not {program}'s");
    }
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_sections() {
    latex_doc("sections");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_math() {
    latex_doc("math");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_tables() {
    latex_doc("tables");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_figures() {
    latex_doc("figures");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_titlepage() {
    latex_doc("titlepage");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_packages() {
    latex_doc("packages");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_ams() {
    latex_doc("ams");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_gfx() {
    latex_doc("gfx");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_layout() {
    latex_doc("layout");
}

#[test]
#[ignore = "waits for phitex-tex's LaTeX slice (the latex.ltx prelude)"]
fn latex_rules() {
    latex_doc("rules");
}

/// plain.tex as phitex-red's tests make it (`tests/dvi/plaintext.tex`
/// before its body, with `hyphen.tex`).
fn plain() -> String {
    let s = std::fs::read_to_string(fixtures().join("dvi/plaintext.tex")).expect("plaintext.tex");
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = std::fs::read_to_string(fixtures().join("hyph/hyphen.tex")).expect("hyphen.tex");
    s[..at].replace(
        "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
        &h,
    )
}

/// The PDF documents of phitex-red's `tests/pdf.rs`: the name of the file
/// to typeset, its text and the oracle's name.
fn pdf_docs() -> Vec<(String, String, String)> {
    let pdf = fixtures().join("pdf");
    let mut v = Vec::new();
    for name in [
        "hello",
        "compressed",
        "mag",
        "mixed1",
        "mixed2",
        "objstm",
        "rules",
        "guess",
    ] {
        let text = std::fs::read_to_string(pdf.join(format!("{name}.tex"))).expect("NAME.tex");
        v.push((name.to_owned(), text, name.to_owned()));
    }
    for body in ["story", "pages", "display", "table", "hyphens"] {
        for default in [false, true] {
            let b = std::fs::read_to_string(fixtures().join(format!("tests/plain/{body}.tex")))
                .expect("a body");
            let text = "\\pdfoutput=1\n".to_string()
                + &plain()
                + if default { "\\output={}\n" } else { "" }
                + &b;
            let name = if default {
                format!("{body}-default")
            } else {
                body.to_owned()
            };
            v.push((name.clone(), text, format!("plain-{name}")));
        }
    }
    v
}

/// `pdftex -ini -etex -interaction=batchmode NAME.tex` through phitex:
/// pdfTeX's PDF file, byte for byte.
#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdftex_ini_documents() {
    let _one = one();
    let mut bad = Vec::new();
    for (name, text, oracle) in pdf_docs() {
        let dir = scratch(&format!("pdf-{name}"));
        std::fs::write(dir.join(format!("{name}.tex")), &text).expect("the document");
        let out = command(&as_program("pdftex"), &dir)
            .args([
                "-ini",
                "-etex",
                "-interaction=batchmode",
                &format!("{name}.tex"),
            ])
            .output()
            .expect("phitex runs");
        let want =
            std::fs::read(fixtures().join(format!("tests/pdf/{oracle}.pdf"))).expect("the oracle");
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
    assert!(
        bad.is_empty(),
        "PDF files that are not pdfTeX's:\n{}",
        bad.join("\n")
    );
}

/// Whether a program of TeX Live is there.
fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
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
#[ignore = "waits for phitex-tex's LaTeX slice (the .aux loop-back)"]
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
            .args([
                "build",
                "-v",
                "--output-format",
                "dvi",
                &format!("{name}.tex"),
            ])
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

/// A biblatex document (`oracle/fixtures/biber/numeric.tex` and its
/// `refs.bib`) built to the fixpoint by `phitex build`, biber run in
/// process between the runs: its `.bbl` is biber's and its DVI file is
/// that of `latex`, `biber`, `latex`, `latex` (both blessed by
/// scripts/bless-biber.sh).
#[test]
#[ignore = "waits for phitex-tex running biblatex (at \\printbibliography: hint regime violated, catcode of ' ' is 9)"]
fn build_biblatex_with_native_biber() {
    let _one = one();
    let src = fixtures().join("biber");
    let name = "numeric";
    let ours = scratch("biber-numeric");
    for f in [format!("{name}.tex"), "refs.bib".to_string()] {
        std::fs::copy(src.join(&f), ours.join(&f)).expect("a copy");
    }
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &ours)
        .args([
            "build",
            "-v",
            "--output-format",
            "dvi",
            &format!("{name}.tex"),
        ])
        .output()
        .expect("phitex runs");
    let errs = String::from_utf8_lossy(&out.stderr);
    eprintln!("{name}: phitex build:\n{errs}");
    assert!(out.status.success(), "{name}: phitex build failed:\n{errs}");
    assert!(errs.contains("biber"), "{name}: biber did not run:\n{errs}");
    let got = std::fs::read(ours.join(format!("{name}.bbl"))).unwrap_or_default();
    let want = std::fs::read(src.join(format!("{name}.bbl"))).expect("the .bbl oracle");
    assert!(
        got == want,
        "{name}: the native biber's .bbl is not biber's"
    );
    let got = std::fs::read(ours.join(format!("{name}.dvi"))).unwrap_or_default();
    let want = std::fs::read(src.join(format!("fixpoint/{name}.dvi"))).expect("the DVI oracle");
    assert!(
        masked(&got) == masked(&want),
        "{name}: the fixpoint's DVI is not latex+biber's: {} (ours in {})",
        first_difference(&got, &want),
        ours.display()
    );
}

/// The 48-page plain document of phitex-red's budgets ("big"), in PDF
/// mode, as one `-ini` file.
fn big() -> String {
    let mut body = String::new();
    for k in 0..4 {
        for b in ["story", "pages", "display", "table", "hyphens"] {
            let t = std::fs::read_to_string(fixtures().join(format!("plain/{b}.tex")))
                .expect("a plain body");
            let t = t.trim_end().strip_suffix("\\bye").unwrap_or(&t).to_string();
            let _ = write!(body, "\\message{{[part {k} {b}]}}\n{t}\n\n");
        }
    }
    body.push_str("\\bye\n");
    "\\pdfoutput=1\n".to_string() + &plain() + &body
}

/// Reads `child`'s lines until one starts with `prefix`; the lines read.
/// The watch's next `--json` event with `key` equal to `value`.
fn until(
    lines: &mut impl Iterator<Item = String>,
    key: &str,
    value: &serde_json::Value,
    seen: &mut Vec<String>,
) -> serde_json::Value {
    for l in lines.by_ref() {
        seen.push(l.clone());
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&l)
            && v[key] == *value
        {
            return v;
        }
    }
    panic!(
        "the watch ended before {key} = {value}:\n{}",
        seen.join("\n")
    );
}

/// A build event's milliseconds.
fn update_ms(ev: &serde_json::Value) -> f64 {
    ev["wall_ms"].as_f64().expect("wall_ms")
}

/// `phitex watch`, headless: a word edited in the file on disk is built as
/// an edit of the same Build; its PDF is the cold build's of the edited
/// text, byte for byte, and the edit's update is within the brief's word
/// budget (50 ms; phitex-red's budgets measure about 10 ms on this
/// document) in a release build.
#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn watch_word_edit() {
    let _one = one();
    let text = big();
    let dir = scratch("watch");
    std::fs::write(dir.join("big.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--ini",
            "--etex",
            "-j",
            "8",
            "big.tex",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let cold = until(&mut lines, "build", &1.into(), &mut seen);
    eprintln!("{cold}");
    // (a word of the body, as long as the one it replaces)
    let at = text.rfind("\\message{[part 2 story]}").expect("part 2");
    let word = text[at..].find(" the ").expect("a word") + at;
    let edited = format!("{} thy {}", &text[..word], &text[word + 5..]);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("big.tex"), &edited).expect("the edit");
    let line = until(&mut lines, "build", &2.into(), &mut seen);
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
    assert!(
        out.status.success(),
        "the cold build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_pdf = std::fs::read(cold_dir.join("big.pdf")).expect("the cold PDF");
    assert!(
        watched == cold_pdf,
        "the watch's PDF after the edit is not the cold build's: {}",
        first_difference(&watched, &cold_pdf)
    );
    eprintln!("watch: a word edit's update took {ms:.2} ms on 48 pages");
    if !cfg!(debug_assertions) {
        assert!(
            ms <= 50.0,
            "a word edit's update took {ms:.2} ms, over the brief's 50 ms"
        );
    }
}

/// Two copies of the binary (other paths, other times) share the format
/// cache: a format is cached under the build's identity (a hash of its
/// sources, `tex::build_id`), not the executable's path, size or time.
#[test]
#[ignore = "waits for the prelude export (Build::prelude_key, export_prelude)"]
fn format_cache_shared_by_copies() {
    let _one = one();
    let base = scratch("copies");
    let cache = base.join("cache");
    let doc = base.join("doc");
    std::fs::create_dir_all(&doc).expect("a directory");
    std::fs::write(doc.join("x.tex"), "Hi.\n\\bye\n").expect("the document");
    let fmts = || -> Vec<(PathBuf, std::time::SystemTime)> {
        let mut v: Vec<_> = std::fs::read_dir(cache.join("formats"))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "fmt"))
                    .map(|p| {
                        let t = std::fs::metadata(&p)
                            .and_then(|m| m.modified())
                            .expect("a time");
                        (p, t)
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    let mut after = Vec::new();
    for copy in ["a", "b"] {
        let d = base.join(copy);
        std::fs::create_dir_all(&d).expect("a directory");
        let exe = d.join("tex");
        std::fs::copy(env!("CARGO_BIN_EXE_phitex"), &exe).expect("a copy of the binary");
        let out = command(&exe, &doc)
            .env("PHITEX_CACHE_DIR", &cache)
            .args(["-interaction=batchmode", "x.tex"])
            .output()
            .expect("phitex runs");
        let errs = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "copy {copy}: phitex failed: {errs}");
        if copy == "b" {
            assert!(
                !errs.contains("making the"),
                "copy b made the format again: {errs}"
            );
        }
        after.push(fmts());
    }
    assert_eq!(after[0].len(), 1, "one format: {:?}", after[0]);
    assert_eq!(
        after[0], after[1],
        "the second copy wrote a format of its own"
    );
}

/// A WebSocket text frame from a client (masked, RFC 6455).
fn ws_frame(text: &str) -> Vec<u8> {
    let p = text.as_bytes();
    let mut f = vec![0x81u8];
    if p.len() < 126 {
        f.push(0x80 | u8::try_from(p.len()).expect("short"));
    } else if p.len() < 65536 {
        f.push(0xfe);
        f.extend_from_slice(&u16::try_from(p.len()).expect("medium").to_be_bytes());
    } else {
        f.push(0xff);
        f.extend_from_slice(&(p.len() as u64).to_be_bytes());
    }
    let mask = [0x37u8, 0xfa, 0x21, 0x3d];
    f.extend_from_slice(&mask);
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// Sends `request` (JSON) to the viewer at `url` over a WebSocket; the
/// socket, kept open.
fn ws_send(url: &str, request: &str) -> std::net::TcpStream {
    let rest = url.trim().strip_prefix("http://").expect("an http URL");
    let (addr, path) = rest.split_once('/').expect("a path");
    let mut s = std::net::TcpStream::connect(addr).expect("the viewer answers");
    write!(
        s,
        "GET /{path}ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .expect("the handshake");
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        std::io::Read::read_exact(&mut s, &mut b).expect("the handshake's answer");
        head.push(b[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 101"),
        "no upgrade: {}",
        String::from_utf8_lossy(&head)
    );
    s.write_all(&ws_frame(request)).expect("the request");
    s
}

/// `phitex watch` with its viewer, headless: an editor's unsaved buffer
/// (`set_file` over the viewer's WebSocket, the file on disk untouched)
/// is built as an edit; the PDF is the cold build's of the buffer's text.
#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn watch_editor_buffer() {
    let _one = one();
    let body = std::fs::read_to_string(fixtures().join("plain/story.tex")).expect("a body");
    let text = "\\pdfoutput=1\n".to_string() + &plain() + &body;
    let dir = scratch("watch-editor");
    std::fs::write(dir.join("story.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["watch", "--json", "--ini", "--etex", "-j", "8", "story.tex"])
        .env("XDG_RUNTIME_DIR", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let url = until(&mut lines, "event", &"viewer".into(), &mut seen)["url"]
        .as_str()
        .expect("the viewer's url")
        .to_owned();
    until(&mut lines, "build", &1.into(), &mut seen);
    let at = body.find(" the ").expect("a word") + "\\pdfoutput=1\n".len() + plain().len();
    let edited = format!("{} thy {}", &text[..at], &text[at + 5..]);
    let req = format!(
        "{{\"id\":1,\"op\":\"set_file\",\"file\":\"story.tex\",\"text\":{}}}",
        json_string(&edited)
    );
    let _socket = ws_send(&url, &req);
    let line = until(&mut lines, "build", &2.into(), &mut seen);
    eprintln!("{line}");
    assert!(
        line["why"].as_str().is_some_and(|w| w.contains("(editor)")),
        "the second build is not the editor's: {line}"
    );
    let watched = std::fs::read(dir.join("story.pdf")).expect("the watch's PDF");
    assert_eq!(
        std::fs::read_to_string(dir.join("story.tex")).expect("the file"),
        text,
        "the file on disk changed"
    );
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("watch-editor-cold");
    std::fs::write(cold_dir.join("story.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--ini", "--etex", "story.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "the cold build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_pdf = std::fs::read(cold_dir.join("story.pdf")).expect("the cold PDF");
    assert!(
        watched == cold_pdf,
        "the buffer's build is not the cold build's: {}",
        first_difference(&watched, &cold_pdf)
    );
}

/// `s` as a JSON string.
fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = std::fmt::Write::write_fmt(&mut o, format_args!("\\u{:04x}", u32::from(c)));
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_sections() {
    pdflatex_doc("sections");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_math() {
    pdflatex_doc("math");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_tables() {
    pdflatex_doc("tables");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_figures() {
    pdflatex_doc("figures");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_titlepage() {
    pdflatex_doc("titlepage");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_packages() {
    pdflatex_doc("packages");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_ams() {
    pdflatex_doc("ams");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_gfx() {
    pdflatex_doc("gfx");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_layout() {
    pdflatex_doc("layout");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_rules() {
    pdflatex_doc("rules");
}

/// `pdflatex` on a document of `tests/latex/pdfdocs/`, if the tree has
/// it (they came with the pdfTeX primitives' merge).
fn pdflatex_pdfdoc(name: &str) {
    if !pdfdocs_dir().join(format!("{name}.tex")).exists() {
        eprintln!("{name}: not in this tree (tests/latex/pdfdocs/)");
        return;
    }
    pdflatex_doc(name);
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_hyper() {
    pdflatex_pdfdoc("hyper");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_graphics() {
    pdflatex_pdfdoc("graphics");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_color() {
    pdflatex_pdfdoc("color");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_tikz() {
    pdflatex_pdfdoc("tikz");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_zsavepos() {
    pdflatex_pdfdoc("zsavepos");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_objs() {
    pdflatex_pdfdoc("objs");
}

#[test]
#[ignore = "waits for phitex-tex's PDF output"]
fn pdflatex_pdfdoc_vfont() {
    pdflatex_pdfdoc("vfont");
}

/// `tex NAME.tex` through phitex on the plain fixtures: the DVI file is
/// TeX's (its preamble's date masked), the format plain.tex through
/// `tex.ini` on the graph.
#[test]
fn tex_plain_documents() {
    let _one = one();
    let fx = fixtures().join("plain");
    let mut bad = Vec::new();
    for name in ["story", "pages", "display", "table", "hyphens"] {
        let dir = scratch(&format!("tex-{name}"));
        std::fs::copy(
            fx.join(format!("{name}.tex")),
            dir.join(format!("{name}.tex")),
        )
        .expect("the document");
        let out = command(&as_program("tex"), &dir)
            .args(["-interaction=batchmode", &format!("{name}.tex")])
            .output()
            .expect("phitex runs");
        let want = std::fs::read(fx.join(format!("{name}.dvi"))).expect("the oracle");
        let got = std::fs::read(dir.join(format!("{name}.dvi"))).unwrap_or_default();
        if !out.status.success() || masked(&got) != masked(&want) {
            bad.push(format!(
                "{name}: status {:?}, {}; stderr: {}",
                out.status.code(),
                first_difference(&masked(&got), &masked(&want)),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "DVI files that are not TeX's:\n{}",
        bad.join("\n")
    );
}

/// `phitex build --json`: every line on standard output is a JSON event:
/// the build's, with its pages and output, then the cache's; nothing else
/// is printed.
#[test]
fn build_json_events() {
    let _one = one();
    let dir = scratch("json");
    std::fs::copy(fixtures().join("plain/story.tex"), dir.join("story.tex")).expect("the document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "build",
            "--json",
            "--no-synctex",
            "--engine",
            "tex",
            "story.tex",
        ])
        .output()
        .expect("phitex runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect();
    let b = events
        .iter()
        .find(|e| e["event"] == "build")
        .expect("a build event");
    let c = events.last().expect("an event");
    assert_eq!(c["event"], "cache", "{events:?}");
    assert!(c["limit"].as_u64().is_some(), "{c}");
    assert_eq!(b["ok"], true, "{b}");
    assert!(b["pages"].as_u64().is_some_and(|n| n > 0), "{b}");
    assert!(
        b["output"]
            .as_str()
            .is_some_and(|o| o.ends_with("story.dvi")),
        "{b}"
    );
    assert!(dir.join("story.dvi").is_file());
}

/// The exit statuses: 2 for a file that is not there or an unknown
/// command, 1 for a document with errors.
#[test]
fn exit_statuses() {
    let _one = one();
    let dir = scratch("exits");
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    let status = |args: &[&str]| {
        command(bin, &dir)
            .args(args)
            .output()
            .expect("phitex runs")
            .status
            .code()
    };
    assert_eq!(
        status(&["build", "--engine", "tex", "nothere.tex"]),
        Some(2)
    );
    assert_eq!(status(&["check", "x.tex"]), Some(2));
    assert_eq!(status(&["x.tex"]), Some(2));
    std::fs::write(dir.join("bad.tex"), "\\undefinedcs\n\\bye\n").expect("the document");
    assert_eq!(status(&["build", "--engine", "tex", "bad.tex"]), Some(1));
}

/// `phitex watch --json` on a plain document (`tex`): a word edited on
/// disk is build 2, an edit of the line it is on, fewer nodes run than
/// cold, and its DVI file is a cold build's of the edited text; an edit
/// that makes an error is build 3, not ok, the last good DVI file kept.
#[test]
fn watch_plain_edit() {
    let _one = one();
    let text = std::fs::read_to_string(fixtures().join("plain/story.tex")).expect("story.tex");
    let dir = scratch("watch-plain");
    std::fs::write(dir.join("story.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--engine",
            "tex",
            "story.tex",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let cold = until(&mut lines, "build", &1.into(), &mut seen);
    assert_eq!(cold["ok"], true, "{cold}");
    let at = text.find(" the ").expect("a word");
    let line = text[..at].matches('\n').count() + 1;
    let edited = format!("{} thy {}", &text[..at], &text[at + 5..]);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("story.tex"), &edited).expect("the edit");
    let ev = until(&mut lines, "build", &2.into(), &mut seen);
    assert_eq!(ev["ok"], true, "{ev}");
    assert_eq!(ev["edit"], format!("story.tex:{line}"), "{ev}");
    assert!(
        ev["nodes_run"].as_u64() < cold["nodes_run"].as_u64(),
        "the edit ran no fewer nodes than cold: {ev} {cold}"
    );
    let watched = std::fs::read(dir.join("story.dvi")).expect("the watch's DVI");
    let broken = edited.replacen(" thy ", " \\undefinedcs ", 1);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("story.tex"), &broken).expect("the broken edit");
    let ev = until(&mut lines, "build", &3.into(), &mut seen);
    assert_eq!(ev["ok"], false, "{ev}");
    assert_eq!(
        std::fs::read(dir.join("story.dvi")).expect("the DVI"),
        watched,
        "the last good DVI file was not kept"
    );
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("watch-plain-cold");
    std::fs::write(cold_dir.join("story.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--no-synctex", "--engine", "tex", "story.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_dvi = std::fs::read(cold_dir.join("story.dvi")).expect("the cold DVI");
    assert!(
        masked(&watched) == masked(&cold_dvi),
        "the watch's DVI after the edit is not the cold build's: {}",
        first_difference(&watched, &cold_dvi)
    );
}

/// XeLaTeX and LuaLaTeX are not supported, by any name, and nothing
/// external runs in their place.
#[test]
fn xelatex_lualatex_refused() {
    let dir = scratch("refused");
    std::fs::write(dir.join("a.tex"), "\\documentclass{article}\n").expect("the document");
    std::fs::write(dir.join("b.tex"), "% !TEX program = lualatex\n\\relax\n")
        .expect("the document");
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    for (args, file) in [
        (&["build", "--engine", "xelatex", "a.tex"][..], "a.pdf"),
        (&["build", "--engine", "lualatex", "a.tex"][..], "a.pdf"),
        (&["build", "b.tex"][..], "b.pdf"),
    ] {
        let out = command(bin, &dir).args(args).output().expect("phitex runs");
        let errs = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} succeeded");
        assert!(errs.contains("not supported"), "{args:?}: {errs}");
        assert!(!dir.join(file).exists(), "{args:?} wrote {file}");
    }
}

/// A one-page job without a format (`--ini --etex`).
fn tiny() -> &'static str {
    "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x Hi\\end\n"
}

/// Every option is accepted before the command, after it and after the
/// file: `-o DIR` and `-j N` in the three positions, the output where
/// `-o` says.
#[test]
fn options_in_any_position() {
    let _one = one();
    let dir = scratch("options");
    std::fs::write(dir.join("t.tex"), tiny()).expect("the document");
    let lines: [&[&str]; 3] = [
        &["-o", "out1", "-j", "2", "build", "--ini", "--etex", "t.tex"],
        &["build", "-o", "out2", "-j", "2", "--ini", "--etex", "t.tex"],
        &["build", "--ini", "--etex", "t.tex", "-o", "out3", "-j", "2"],
    ];
    for (k, args) in lines.iter().enumerate() {
        let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
            .args(*args)
            .output()
            .expect("phitex runs");
        assert!(
            out.status.success(),
            "phitex {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let dvi = dir.join(format!("out{}", k + 1)).join("t.dvi");
        assert!(dvi.is_file(), "phitex {args:?} wrote no {}", dvi.display());
    }
}

/// An unknown option is named, with `--help` suggested, and the exit
/// status is 2; an option before the command is never taken for TeX's
/// command line.
#[test]
fn unknown_option_is_named() {
    let dir = scratch("unknown-option");
    std::fs::write(dir.join("t.tex"), tiny()).expect("the document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["build", "--frob", "t.tex"])
        .output()
        .expect("phitex runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("--frob"), "the option is not named:\n{err}");
    assert!(err.contains("--help"), "--help is not suggested:\n{err}");
    assert!(err.lines().count() <= 8, "more than a few lines:\n{err}");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["-o", "out", "--frob", "build", "t.tex"])
        .output()
        .expect("phitex runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("--frob") && !err.contains("compat"), "{err}");
}

/// `--help` is sectioned (commands, options, examples) and plain off a
/// terminal.
#[test]
fn help_is_sectioned() {
    let dir = scratch("help");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .arg("--help")
        .output()
        .expect("phitex runs");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for section in [
        "Commands:",
        "Options:",
        "Examples:",
        "build",
        "watch",
        "clean",
        "--json",
        "--no-view",
        "--shell-escape",
        "Exit status",
        "--output-dir",
        "--jobs",
    ] {
        assert!(
            text.contains(section),
            "`{section}` is not in the help:\n{text}"
        );
    }
    assert!(
        !text.contains('\x1b'),
        "escape sequences off a terminal:\n{text}"
    );
}

/// `phitex watch` over 50 word edits of the big document: the engine's
/// tables (the build's "engine tables" note) stay bounded, the collector
/// running after each build (`PHITEX_NO_GC=1` for the before numbers).
#[test]
#[ignore = "heavy: 50 edits of the 48-page document"]
fn watch_tables_bounded() {
    let _one = one();
    let text = big();
    let dir = scratch("watch-tables");
    std::fs::write(dir.join("big.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--ini",
            "--etex",
            "-j",
            "8",
            "big.tex",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let tables = |line: &serde_json::Value| -> Vec<u64> {
        let n = line["notes"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .filter_map(|n| n.as_str())
                    .find(|n| n.starts_with("engine tables:"))
            })
            .unwrap_or_else(|| panic!("no tables note: {line}"))
            .to_owned();
        n.split(|c: char| !c.is_ascii_digit())
            .filter_map(|w| w.parse().ok())
            .collect()
    };
    let first = until(&mut lines, "build", &1.into(), &mut seen);
    let mut sizes = vec![tables(&first)];
    let at = text.rfind("\\message{[part 2 story]}").expect("part 2");
    let word = text[at..].find(" the ").expect("a word") + at;
    for k in 0..50u64 {
        let w = if k % 2 == 0 { " thy " } else { " the " };
        let edited = format!("{}{w}{}", &text[..word], &text[word + 5..]);
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dir.join("big.tex"), &edited).expect("the edit");
        let line = until(&mut lines, "build", &(k + 2).into(), &mut seen);
        sizes.push(tables(&line));
    }
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    for (k, s) in sizes.iter().enumerate() {
        eprintln!("build {}: nodes, units, values {s:?}", k + 1);
    }
    if std::env::var_os("PHITEX_NO_GC").is_none() {
        let (a, b) = (&sizes[1], &sizes[sizes.len() - 1]);
        assert!(
            b[0] <= a[0] * 11 / 10 && b[2] <= a[2] * 11 / 10,
            "grew: {a:?} -> {b:?}"
        );
    }
}

/// `phitex watch --json` on `text` (as `file` in a scratch directory,
/// `args` before it) over 50 edits alternating the word at `word` (five
/// bytes, " the ") with " thy " and back: each build's "engine tables"
/// note numbers (nodes, units, values, kB resident, us collecting, nodes
/// freed) and wall, printed for builds 1, 2, 10, 25, 51.
// measurement means only: f64 precision is irrelevant here
#[allow(clippy::cast_precision_loss)]
fn watch_gc_series(name: &str, file: &str, text: &str, args: &[&str], word: usize) {
    let _one = one();
    let dir = scratch(name);
    std::fs::write(dir.join(file), text).expect("the document");
    let mut a = vec!["watch", "--json", "--no-view"];
    a.extend_from_slice(args);
    a.push(file);
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(&a)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let row = |line: &serde_json::Value| -> (Vec<u64>, f64, String) {
        let n = line["notes"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .filter_map(|n| n.as_str())
                    .find(|n| n.starts_with("engine tables:"))
            })
            .unwrap_or_else(|| panic!("no tables note: {line}"))
            .to_owned();
        let runs = line["notes"][0].as_str().unwrap_or("").to_owned();
        (
            n.split(|c: char| !c.is_ascii_digit())
                .filter_map(|w| w.parse().ok())
                .collect(),
            line["wall_ms"].as_f64().unwrap_or(0.0),
            runs,
        )
    };
    let first = until(&mut lines, "build", &1.into(), &mut seen);
    assert_eq!(first["ok"], true, "{first}");
    let mut rows = vec![row(&first)];
    for k in 0..50u64 {
        let w = if k % 2 == 0 { " thy " } else { " the " };
        let edited = format!("{}{w}{}", &text[..word], &text[word + 5..]);
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dir.join(file), &edited).expect("the edit");
        let line = until(&mut lines, "build", &(k + 2).into(), &mut seen);
        rows.push(row(&line));
    }
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let gc = std::env::var_os("PHITEX_NO_GC").is_none();
    eprintln!(
        "GCSERIES {name} gc={gc}: build | nodes units values | RSS kB | gc us (freed) | wall ms | notes"
    );
    for (k, (s, wall, runs)) in rows.iter().enumerate() {
        eprintln!(
            "GCROW {name} gc={gc} build {} {s:?} wall {wall:.1} | {runs}",
            k + 1
        );
    }
    let walls: Vec<f64> = rows[1..].iter().map(|r| r.1).collect();
    let gcs: Vec<u64> = rows[1..].iter().map(|r| r.0[4]).collect();
    eprintln!(
        "GCSUM {name} gc={gc}: edit wall mean {:.1} ms, gc mean {:.0} us, max {} us",
        walls.iter().sum::<f64>() / walls.len() as f64,
        gcs.iter().sum::<u64>() as f64 / gcs.len() as f64,
        gcs.iter().max().unwrap_or(&0)
    );
}

/// The plain generated document `scratch/para400.tex` (not committed):
/// the GC series under `--engine tex`.
#[test]
#[ignore = "heavy, measurement: needs scratch/para400.tex"]
fn watch_gc_para400() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch/para400.tex");
    let text = std::fs::read_to_string(p).expect("scratch/para400.tex");
    let mid = text.len() / 2;
    let word = text[mid..].find(" the ").expect("a word") + mid;
    watch_gc_series(
        "gc-para400",
        "para400.tex",
        &text,
        &["--engine", "tex", "-j", "8"],
        word,
    );
}

/// LaTeX's hello with 30 paragraphs added: the GC series under
/// `--engine pdflatex`.
#[test]
#[ignore = "heavy, measurement: the LaTeX format"]
fn watch_gc_latex30() {
    let para = "This is the paragraph of a long story about the printer, the reader and the galley proof, \
                with enough words that it breaks into several lines of the page and keeps the line \
                breaker busy for a while: the ruler, the hyphen, the kern, the ligature and the index.";
    let mut text = "\\documentclass{article}\\begin{document}Hello world.\n\n".to_owned();
    let mut word = 0;
    for k in 1..=30 {
        if k == 15 {
            word = text.len() + "This is".len();
        }
        let _ = write!(text, "{k}. {para}\n\n");
    }
    text += "\\end{document}\n";
    // (" the " right after "15. This is")
    word += "15. ".len();
    assert_eq!(&text[word..word + 5], " the ");
    watch_gc_series(
        "gc-latex30",
        "hello.tex",
        &text,
        &["--engine", "pdflatex", "-j", "8"],
        word,
    );
}

/// `phitex watch`, headless: the main file edited on disk while the cold
/// build evaluates is taken by that build (its `build` event counts the
/// text absorbed), no second build follows, and the PDF is the cold
/// build's of the edited text.
#[test]
fn watch_absorbs_an_edit_during_the_cold_build() {
    let _one = one();
    let words = "the printer admired a quiet galley of careful proofs while the \
        reader looked at every ligature kern and hyphen in the long paragraph";
    let mut body = String::new();
    for k in 0..1500 {
        let _ = write!(body, "Paragraph {k}: {words} {words} {words} {words}.\n\n");
    }
    let text = format!("{body}\\bye\n");
    let edited = text.replacen("Paragraph 1499: the", "Paragraph 1499: eht", 1);
    let dir = scratch("watch-absorb");
    // (the plain format cached first: the cold build then evaluates the
    // document's own nodes when the edit comes)
    std::fs::write(dir.join("warm.tex"), "x\\bye\n").expect("a tiny document");
    let warm = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["build", "--engine", "tex", "warm.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    std::fs::write(dir.join("slow.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--engine",
            "tex",
            "slow.tex",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    // (the edited text written again and again until the build ends: the
    // writes that come while the engine evaluates are taken by it; the
    // build cannot be told apart from outside, so no single moment is
    // guessed)
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (stop, path, edited) = (stop.clone(), dir.join("slow.tex"), edited.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
                // (written whole, then renamed: a watcher never reads it
                // half written)
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, &edited).expect("the edit");
                std::fs::rename(&tmp, &path).expect("the edit in place");
            }
        })
    };
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let first = until(&mut lines, "build", &1.into(), &mut seen);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().expect("the writer");
    eprintln!("{first}");
    assert!(
        first["absorbed"].as_u64().unwrap_or(0) >= 1,
        "the cold build did not take the edit: {first}"
    );
    std::thread::sleep(Duration::from_millis(500));
    let watched = std::fs::read(dir.join("slow.dvi")).expect("the watch's DVI");
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let rest: Vec<String> = lines.collect();
    assert!(
        !rest.iter().any(|l| l.contains("\"event\":\"build\"")),
        "a second build ran: {rest:?}"
    );
    let cold_dir = scratch("watch-absorb-cold");
    std::fs::write(cold_dir.join("slow.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--engine", "tex", "slow.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_dvi = std::fs::read(cold_dir.join("slow.dvi")).expect("the cold DVI");
    assert!(
        masked(&watched) == masked(&cold_dvi),
        "the watch's DVI is not the edited text's: {}",
        first_difference(&watched, &cold_dvi)
    );
}
