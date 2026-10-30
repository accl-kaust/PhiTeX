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

/// An LSP message to the server, framed (`Content-Length`).
fn lsp_send(w: &mut impl std::io::Write, v: &serde_json::Value) {
    let b = v.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{b}", b.len()).expect("the server reads");
    w.flush().expect("the server reads");
}

/// The server's messages, read on a thread of their own.
fn lsp_messages(out: std::process::ChildStdout) -> std::sync::mpsc::Receiver<serde_json::Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(out);
        loop {
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 {
                    return;
                }
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h
                    .split_once(':')
                    .expect("a header, nothing else on standard output");
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().expect("a length");
                }
            }
            let mut b = vec![0; len];
            std::io::Read::read_exact(&mut r, &mut b).expect("the message");
            let v = serde_json::from_slice(&b).expect("JSON");
            if tx.send(v).is_err() {
                return;
            }
        }
    });
    rx
}

/// The server's next message `what` says is the one (those before it
/// kept in `seen`).
fn lsp_until(
    rx: &std::sync::mpsc::Receiver<serde_json::Value>,
    seen: &mut Vec<serde_json::Value>,
    what: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let end = Instant::now() + Duration::from_secs(600);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(m) => {
                seen.push(m.clone());
                if what(&m) {
                    return m;
                }
            }
            Err(e) => panic!(
                "the server ended or was silent ({e}); it sent:\n{}",
                seen.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }
}

/// The viewer's WebSocket (a browser's): requests answered, events kept.
struct Viewer {
    ws: tungstenite::WebSocket<std::net::TcpStream>,
    events: Vec<serde_json::Value>,
}

impl Viewer {
    fn connect(url: &str) -> Viewer {
        let rest = url.trim().strip_prefix("http://").expect("an http URL");
        let (addr, _) = rest.split_once('/').expect("a path");
        let tcp = std::net::TcpStream::connect(addr).expect("the viewer answers");
        let (ws, _) = tungstenite::client(format!("ws://{rest}ws"), tcp).expect("a WebSocket");
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("a timeout");
        Viewer {
            ws,
            events: Vec::new(),
        }
    }

    /// The next message `what` says is the one (events kept).
    fn until(&mut self, what: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let end = Instant::now() + Duration::from_secs(120);
        while Instant::now() < end {
            match self.ws.read() {
                Ok(tungstenite::Message::Text(t)) => {
                    let v: serde_json::Value = serde_json::from_str(t.as_str()).expect("JSON");
                    if what(&v) {
                        return v;
                    }
                    if v.get("event").is_some() {
                        self.events.push(v);
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => panic!("the viewer's socket: {e}"),
            }
        }
        panic!(
            "the viewer said nothing of the kind; its events: {:?}",
            self.events
        );
    }

    fn ask(&mut self, req: &serde_json::Value) -> serde_json::Value {
        self.ws
            .send(tungstenite::Message::text(req.to_string()))
            .expect("the request");
        let id = req["id"].clone();
        self.until(|v| v["id"] == id && v.get("event").is_none())
    }
}

/// `phitex watch --lsp`: an editor's language client over pipes, on a
/// plain TeX document in DVI mode (the engine's page text, which forward
/// search reads until origins, is a DVI build's; the viewer shows it as
/// dvipdfmx's PDF). Its
/// unsaved buffer (`didOpen`, `didChange`) is what is built: an edit that
/// makes an error is published at its line, the fix clears it and the
/// pages change; a double-click in the viewer (its `source`) is
/// `window/showDocument` at that line; `phitex/cursor` on a word of the
/// second page, with a selection, is the viewer's `follow` and `select`
/// in bytes of the buffer; `shutdown` and `exit` end
/// the watch with status 0. Nothing but LSP messages on standard output.
#[test]
#[allow(clippy::too_many_lines, reason = "one editor's session, in order")]
fn watch_lsp() {
    use serde_json::json;
    let _one = one();
    let body = "Alpha bravo charlie delta.\n\\vfill\\eject\nEcho foxtrot golf hotel.\n\\bye\n";
    let text = plain() + body;
    let line_of = |w: &str| {
        text[..text.find(w).expect("the word")]
            .matches('\n')
            .count()
    };
    let (first, second) = (line_of("Alpha bravo"), line_of("Echo foxtrot"));
    let dir = scratch("watch-lsp");
    std::fs::write(dir.join("doc.tex"), &text).expect("the document");
    let root = dir.canonicalize().expect("the directory");
    let uri = format!("file://{}/doc.tex", root.display());
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["watch", "--lsp", "--ini", "--etex", "-j", "4", "doc.tex"])
        .env("XDG_RUNTIME_DIR", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let mut to = child.stdin.take().expect("its input");
    let rx = lsp_messages(child.stdout.take().expect("its output"));
    let mut seen = Vec::new();
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null,
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {"window": {"showDocument": {"support": true}, "workDoneProgress": true}},
        }}),
    );
    let init = lsp_until(&rx, &mut seen, |m| m["id"] == 1);
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["textDocumentSync"]["change"], 2, "{init}");
    assert_eq!(caps["positionEncoding"], "utf-16", "{init}");
    assert!(
        caps["executeCommandProvider"]["commands"]
            .as_array()
            .is_some_and(|c| c.contains(&json!("phitex.showOnPage"))),
        "{init}"
    );
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    let logged = lsp_until(&rx, &mut seen, |m| {
        m["method"] == "window/logMessage"
            && m["params"]["message"]
                .as_str()
                .is_some_and(|t| t.contains("http://127.0.0.1:"))
    });
    let msg = logged["params"]["message"].as_str().unwrap_or_default();
    let url: String = msg[msg.find("http://").expect("the URL")..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    // (the cold build in: the main file's diagnostics published, none)
    let is_main = |m: &serde_json::Value| {
        m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri.as_str()
    };
    let cold = lsp_until(&rx, &mut seen, is_main);
    assert_eq!(cold["params"]["diagnostics"], json!([]), "{cold}");
    let mut viewer = Viewer::connect(&url);
    let before = viewer.ask(&json!({"id": 1, "op": "pages"}))["json"]["pages"].clone();
    assert_eq!(
        before.as_array().map(Vec::len),
        Some(2),
        "two pages: {before}"
    );
    // (the buffer opened as on disk; an unsaved edit makes an error)
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "tex", "version": 1, "text": text},
        }}),
    );
    let at = |l: usize, c: usize| json!({"line": l, "character": c});
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"range": {"start": at(first, 0), "end": at(first, 0)}, "text": "\\undefinedmacro "}],
        }}),
    );
    let bad = lsp_until(&rx, &mut seen, |m| {
        is_main(m)
            && m["params"]["diagnostics"]
                .as_array()
                .is_some_and(|d| d.iter().any(|d| d["severity"] == 1))
    });
    let err = bad["params"]["diagnostics"]
        .as_array()
        .and_then(|d| d.iter().find(|d| d["severity"] == 1))
        .expect("an error")
        .clone();
    assert_eq!(
        err["range"]["start"]["line"], first,
        "the error's line: {err}"
    );
    assert!(
        err["message"]
            .as_str()
            .is_some_and(|m| m.contains("Undefined control sequence")),
        "{err}"
    );
    // (the fix, with a word of page 2 changed first: whatever build
    // takes the first change alone still has the error)
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": uri, "version": 3},
            "contentChanges": [
                {"range": {"start": at(second, 0), "end": at(second, 4)}, "text": "Xray"},
                {"range": {"start": at(first, 0), "end": at(first, 16)}, "text": ""},
            ],
        }}),
    );
    let fixed = lsp_until(&rx, &mut seen, |m| {
        is_main(m)
            && m["params"]["diagnostics"]
                .as_array()
                .is_some_and(|d| d.iter().all(|d| d["severity"] != 1))
    });
    eprintln!("fixed: {fixed}");
    let after = viewer.ask(&json!({"id": 2, "op": "pages"}))["json"]["pages"].clone();
    assert_eq!(before[0], after[0], "page 1 is not edited");
    assert_ne!(before[1], after[1], "page 2 did not change");
    assert_eq!(
        std::fs::read_to_string(dir.join("doc.tex")).expect("the file"),
        text,
        "the file on disk changed"
    );
    // (a double-click's place in the viewer: shown by the editor)
    let r = viewer
        .ask(&json!({"id": 3, "op": "source", "file": "doc.tex", "line": second + 1, "col": 3}));
    assert_eq!(r["ok"], true, "{r}");
    let show = lsp_until(&rx, &mut seen, |m| m["method"] == "window/showDocument");
    assert_eq!(show["params"]["uri"], uri.as_str(), "{show}");
    assert_eq!(
        show["params"]["selection"]["start"],
        at(second, 2),
        "{show}"
    );
    assert_eq!(show["params"]["takeFocus"], true, "{show}");
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": show["id"], "result": {"success": true}}),
    );
    // (the editor's cursor on a word of page 2, a selection with it: the
    // viewer told their bytes in the buffer, the glyph origins' space, in
    // the origins' name of the file)
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "phitex/cursor", "params": {
            "textDocument": {"uri": uri}, "position": at(second, 7),
            "range": {"start": at(second, 5), "end": at(second, 12)},
        }}),
    );
    let start: usize = text.split_inclusive('\n').take(second).map(str::len).sum();
    let follow = viewer.until(|v| v["event"] == "follow");
    assert_eq!(
        follow,
        json!({"event": "follow", "file": "doc.tex", "at": start + 7,
            "word": [start + 5, start + 12], "line": [start, start + "Xray foxtrot golf hotel.".len()]}),
        "{follow}"
    );
    let select = viewer.until(|v| v["event"] == "select");
    assert_eq!(
        select,
        json!({"event": "select", "file": "doc.tex", "from": start + 5, "to": start + 12}),
        "{select}"
    );
    // (the last build's origins, made at the watch's first idle point
    // after it, before it took the cursor)
    let o = viewer.ask(&json!({"id": 4, "op": "origins", "page": 1}));
    assert!(
        o["json"]["files"]
            .as_array()
            .is_some_and(|f| f.iter().any(|n| n == "doc.tex")),
        "the origins name the file as the events do: {o}"
    );
    assert!(viewer.events.iter().all(|e| e["event"] != "reveal"));
    // (the end, as the editor ends it)
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": null}),
    );
    let down = lsp_until(&rx, &mut seen, |m| m["id"] == 9);
    assert!(down.get("error").is_none(), "{down}");
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().expect("the watch") {
            break s;
        }
        assert!(Instant::now() < end, "the watch did not end after exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the watch ended with {status}");
}

/// `phitex watch --lsp`'s progress (docs/spec-editor-protocol.md §2):
/// the cold build's `$/progress` (a token made, begun with the title
/// `PhiTeX`, ended with what it made) and `phitex/status` (`starting`, then
/// `ready` with its pages, the native engine); the viewer's own
/// `progress` events come from the same records (here: the editor's).
#[test]
fn watch_lsp_progress() {
    use serde_json::json;
    let _one = one();
    let text = plain() + "Alpha.\\vfill\\eject Bravo.\\vfill\\eject Charlie.\\bye\n";
    let dir = scratch("watch-lsp-progress");
    std::fs::write(dir.join("doc.tex"), &text).expect("the document");
    let root = dir.canonicalize().expect("the directory");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--lsp",
            "--no-view",
            "--ini",
            "--etex",
            "-j",
            "4",
            "doc.tex",
        ])
        .env("XDG_RUNTIME_DIR", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let mut to = child.stdin.take().expect("its input");
    let rx = lsp_messages(child.stdout.take().expect("its output"));
    let mut seen = Vec::new();
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null,
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {"window": {"workDoneProgress": true}},
        }}),
    );
    lsp_until(&rx, &mut seen, |m| m["id"] == 1);
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    let ready = lsp_until(&rx, &mut seen, |m| {
        m["method"] == "phitex/status" && m["params"]["state"] == "ready"
    });
    assert_eq!(ready["params"]["pages"], 3, "{ready}");
    assert_eq!(ready["params"]["errors"], 0, "{ready}");
    assert_eq!(ready["params"]["engine"], "native", "{ready}");
    let create = seen
        .iter()
        .find(|m| m["method"] == "window/workDoneProgress/create")
        .expect("a progress token made");
    let token = create["params"]["token"].clone();
    let progress: Vec<&serde_json::Value> = seen
        .iter()
        .filter(|m| m["method"] == "$/progress" && m["params"]["token"] == token)
        .collect();
    assert_eq!(progress[0]["params"]["value"]["kind"], "begin", "{seen:?}");
    assert_eq!(
        progress[0]["params"]["value"]["title"], "PhiTeX",
        "{seen:?}"
    );
    let end = &progress.last().expect("an end")["params"]["value"];
    assert_eq!(end["kind"], "end", "{seen:?}");
    assert!(
        end["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Built in ") && m.ends_with(", 3 pages")),
        "{end}"
    );
    assert!(
        seen.iter()
            .any(|m| m["method"] == "phitex/status" && m["params"]["state"] == "starting"),
        "{seen:?}"
    );
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": null}),
    );
    lsp_until(&rx, &mut seen, |m| m["id"] == 9);
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().expect("the watch") {
            break s;
        }
        assert!(Instant::now() < end, "the watch did not end after exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the watch ended with {status}");
}

/// `phitex watch --lsp`'s editor features (docs/spec-editor-protocol.md
/// §3): the commands advertised; the settings asked for after
/// `initialized`; `documentSymbol` of a file not opened (read from
/// disk); `savePdf` (a DVI build's PDF, made for the viewer);
/// `cacheStats` and `clearCache`; `rebuild` a cold build; a settings
/// change that changes the threads a new session, the viewer's address
/// kept; `stop` said and the watch ended cleanly.
#[test]
#[allow(clippy::too_many_lines, reason = "one editor's session, in order")]
fn watch_lsp_commands() {
    use serde_json::json;
    let _one = one();
    let text = plain() + "Alpha.\\vfill\\eject Bravo.\\bye\n";
    let dir = scratch("watch-lsp-commands");
    std::fs::write(dir.join("doc.tex"), &text).expect("the document");
    std::fs::write(
        dir.join("notes.tex"),
        "\\section{One}\\label{s:one}\n\\subsection{Two}\n",
    )
    .expect("a file not built");
    let root = dir.canonicalize().expect("the directory");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["watch", "--lsp", "--ini", "--etex", "-j", "4", "doc.tex"])
        .env("XDG_RUNTIME_DIR", &dir)
        .env("PHITEX_CACHE_DIR", dir.join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let mut to = child.stdin.take().expect("its input");
    let rx = lsp_messages(child.stdout.take().expect("its output"));
    let mut seen = Vec::new();
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null,
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {"workspace": {"configuration": true}},
        }}),
    );
    let init = lsp_until(&rx, &mut seen, |m| m["id"] == 1);
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["documentSymbolProvider"], true, "{init}");
    for c in [
        "phitex.stop",
        "phitex.rebuild",
        "phitex.savePdf",
        "phitex.clearCache",
        "phitex.cacheStats",
    ] {
        assert!(
            caps["executeCommandProvider"]["commands"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(c))),
            "{c}: {init}"
        );
    }
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    let conf = lsp_until(&rx, &mut seen, |m| m["method"] == "workspace/configuration");
    assert_eq!(conf["params"]["items"][0]["section"], "phitex", "{conf}");
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": conf["id"], "result": [{"threads": null, "main": null, "follow": "cursor", "files": null}]}),
    );
    let ready =
        |m: &serde_json::Value| m["method"] == "phitex/status" && m["params"]["state"] == "ready";
    lsp_until(&rx, &mut seen, ready);
    let url = seen
        .iter()
        .find_map(|m| {
            let t = m["params"]["message"].as_str()?;
            Some(
                t[t.find("http://")?..]
                    .split_whitespace()
                    .next()?
                    .to_owned(),
            )
        })
        .expect("the viewer's address");
    // (the outline of a file not opened)
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol", "params": {
            "textDocument": {"uri": format!("file://{}/notes.tex", root.display())}}}),
    );
    let sym = lsp_until(&rx, &mut seen, |m| m["id"] == 2);
    assert_eq!(sym["result"][0]["name"], "One", "{sym}");
    assert_eq!(sym["result"][0]["children"][0]["name"], "s:one", "{sym}");
    assert_eq!(sym["result"][0]["children"][1]["name"], "Two", "{sym}");
    let exec = |to: &mut std::process::ChildStdin, id: u64, c: &str, args: serde_json::Value| {
        lsp_send(
            to,
            &json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand",
                "params": {"command": c, "arguments": args}}),
        );
    };
    exec(&mut to, 3, "phitex.savePdf", json!([{"path": "saved.pdf"}]));
    let saved = lsp_until(&rx, &mut seen, |m| m["id"] == 3);
    assert!(saved["result"]["bytes"].as_u64() > Some(0), "{saved}");
    let pdf = std::fs::read(dir.join("saved.pdf")).expect("the PDF saved");
    assert!(pdf.starts_with(b"%PDF-"), "a PDF");
    exec(&mut to, 4, "phitex.cacheStats", json!([]));
    let held_now = lsp_until(&rx, &mut seen, |m| m["id"] == 4);
    let caches = &held_now["result"]["caches"];
    assert_eq!(caches[0]["scope"], "build", "{held_now}");
    assert_eq!(caches[1]["scope"], "packages", "{held_now}");
    for k in [
        "location",
        "bytes",
        "entries",
        "last_used",
        "hits",
        "misses",
    ] {
        assert!(caches[0].get(k).is_some(), "{k}: {held_now}");
    }
    let held = caches[0]["bytes"].as_u64().unwrap_or(0);
    exec(&mut to, 5, "phitex.clearCache", json!([{"scope": "build"}]));
    let cleared = lsp_until(&rx, &mut seen, |m| m["id"] == 5);
    assert_eq!(cleared["result"]["freed_bytes"], held, "{cleared}");
    exec(
        &mut to,
        6,
        "phitex.clearCache",
        json!([{"scope": "nothing"}]),
    );
    let bad = lsp_until(&rx, &mut seen, |m| m["id"] == 6);
    assert_eq!(bad["error"]["code"], -32603, "{bad}");
    // (a cold build, asked)
    seen.clear();
    exec(&mut to, 7, "phitex.rebuild", json!([]));
    lsp_until(&rx, &mut seen, ready);
    // (the threads changed: a new session, its build in, the viewer kept)
    seen.clear();
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {"phitex": {"threads": 2}}}}),
    );
    lsp_until(&rx, &mut seen, |m| {
        m["method"] == "window/logMessage"
            && m["params"]["message"]
                .as_str()
                .is_some_and(|t| t.contains("a new session"))
    });
    let again = lsp_until(&rx, &mut seen, ready);
    assert_eq!(again["params"]["pages"], 2, "{again}");
    assert!(
        !seen.iter().any(|m| m["params"]["message"]
            .as_str()
            .is_some_and(|t| t.contains("http://") && !t.contains(&url))),
        "the viewer's address changed: {seen:?}"
    );
    // (stop: said, then the end)
    exec(&mut to, 8, "phitex.stop", json!([]));
    lsp_until(&rx, &mut seen, |m| m["id"] == 8);
    lsp_until(&rx, &mut seen, |m| {
        m["method"] == "phitex/status" && m["params"]["state"] == "stopped"
    });
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().expect("the watch") {
            break s;
        }
        assert!(
            Instant::now() < end,
            "the watch did not end after phitex.stop"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the watch ended with {status}");
}

/// `phitex watch --lsp` started by an editor whose root is above the
/// TeX project (a repository's): no file named, no `phitex.toml`, no
/// `.tex` file at the root; `initialize` is answered all the same, and the
/// first `.tex` document opened names the main file (it has a
/// `\documentclass`, here in a comment: the document is plain TeX), whose
/// folder the watch runs in; its build's diagnostics are published, and
/// `shutdown`/`exit` end the watch with status 0.
#[test]
fn watch_lsp_main_from_open() {
    use serde_json::json;
    let _one = one();
    let text = "% \\documentclass is not needed: plain TeX\n".to_string()
        + &plain()
        + "Alpha bravo charlie delta.\n\\bye\n";
    let repo = scratch("watch-lsp-open");
    let dir = repo.join("paper");
    std::fs::create_dir_all(&dir).expect("the project's folder");
    std::fs::write(dir.join("doc.tex"), &text).expect("the document");
    let root = repo.canonicalize().expect("the directory");
    let uri = format!("file://{}/paper/doc.tex", root.display());
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &repo)
        .args(["watch", "--lsp", "--ini", "--etex", "-j", "4"])
        .env("XDG_RUNTIME_DIR", &repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let mut to = child.stdin.take().expect("its input");
    let rx = lsp_messages(child.stdout.take().expect("its output"));
    let mut seen = Vec::new();
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null,
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {},
        }}),
    );
    let init = lsp_until(&rx, &mut seen, |m| m["id"] == 1);
    assert!(init.get("error").is_none(), "{init}");
    assert_eq!(
        init["result"]["capabilities"]["textDocumentSync"]["change"], 2,
        "{init}"
    );
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "tex", "version": 1, "text": text},
        }}),
    );
    let cold = lsp_until(&rx, &mut seen, |m| {
        m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri.as_str()
    });
    assert_eq!(cold["params"]["diagnostics"], json!([]), "{cold}");
    assert!(
        dir.join("doc.dvi").exists(),
        "the output beside the main file"
    );
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "id": 9, "method": "shutdown", "params": null}),
    );
    lsp_until(&rx, &mut seen, |m| m["id"] == 9);
    lsp_send(
        &mut to,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().expect("the watch") {
            break s;
        }
        assert!(Instant::now() < end, "the watch did not end after exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the watch ended with {status}");
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

/// `phitex build --synctex` writes `JOB.synctex.gz` that TeX Live's
/// `synctex` tool reads as it reads pdflatex's (docs/alpha-checklist.md
/// §5): for every line of text, `synctex view` finds it on the same page
/// and baseline (within 1 bp) in both, and `synctex edit` at that place of
/// our PDF gives the line back: a PDF viewer's jumps (Skim, Zathura,
/// Okular, Sumatra) work both ways as with pdflatex.
#[test]
fn synctex_finds_what_pdflatexs_finds() {
    let _one = one();
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    let mut src = String::from("\\documentclass{article}\n\\begin{document}\n");
    let words = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"];
    let mut lines = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if i == 3 {
            src.push_str("\\newpage\n");
        }
        lines.push(src.lines().count() + 1);
        src.push_str(w);
        src.push_str(" is the word of this short paragraph.\n\n");
    }
    src.push_str("\\end{document}\n");
    let ours = scratch("synctex-phitex");
    let theirs = scratch("synctex-pdflatex");
    for d in [&ours, &theirs] {
        std::fs::write(d.join("doc.tex"), &src).expect("the document");
    }
    let out = command(bin, &ours)
        .args(["build", "--synctex", "doc.tex"])
        .output()
        .expect("phitex runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ours.join("doc.synctex.gz").is_file(), "no doc.synctex.gz");
    let out = Command::new("pdflatex")
        .args(["-synctex=1", "-interaction=nonstopmode", "doc.tex"])
        .current_dir(&theirs)
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .output()
        .expect("pdflatex (TeX Live) on the test machine");
    assert!(
        out.status.success(),
        "pdflatex: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // (synctex's answer: its first record's fields, by name)
    let ask = |dir: &Path, args: &[String]| -> std::collections::HashMap<String, String> {
        let out = Command::new("synctex")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("synctex (TeX Live) on the test machine");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut f = std::collections::HashMap::new();
        for l in text
            .lines()
            .skip_while(|l| !l.starts_with("SyncTeX result begin"))
        {
            if let Some((k, v)) = l.split_once(':')
                && !f.contains_key(k)
            {
                f.insert(k.to_owned(), v.to_owned());
            }
        }
        f
    };
    let num = |f: &std::collections::HashMap<String, String>, k: &str| -> f64 {
        f.get(k)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(f64::NAN)
    };
    for (w, line) in words.iter().zip(&lines) {
        let view = vec![
            "view".to_owned(),
            "-i".into(),
            format!("{line}:1:doc.tex"),
            "-o".into(),
            "doc.pdf".into(),
        ];
        let (a, b) = (ask(&ours, &view), ask(&theirs, &view));
        assert_eq!(
            a.get("Page"),
            b.get("Page"),
            "{w} (line {line}): the page, ours {a:?}, pdflatex's {b:?}"
        );
        assert!(
            (num(&a, "y") - num(&b, "y")).abs() <= 1.0,
            "{w} (line {line}): the baseline, ours {a:?}, pdflatex's {b:?}"
        );
        // (back: a point on the line's text, from pdflatex's box)
        let at = format!(
            "{}:{}:{}:doc.pdf",
            b["Page"].trim(),
            num(&b, "x") + 12.0,
            num(&b, "y") - 3.0
        );
        let edit = ask(&ours, &["edit".to_owned(), "-o".into(), at.clone()]);
        assert_eq!(
            edit.get("Line").map(|l| l.trim().to_owned()),
            Some(line.to_string()),
            "{w}: synctex edit at {at} in PhiTeX's: {edit:?}"
        );
    }
}

/// `phitex build` runs LaTeX as latexmk runs pdflatex: from nothing, a
/// one-page document without cross-references in one round (the `.aux`
/// latexmk writes before its first run, `set_trivial_aux_fdb`), one with
/// a reference in two, as latexmk; rebuilt with the files the build left
/// on disk, each in one round, as latexmk, with the same PDF.
#[test]
fn build_rounds_are_latexmks_runs() {
    let _one = one();
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    for (name, src, cold_rounds) in [
        (
            "hello",
            "\\documentclass{article}\n\\begin{document}\nHello, world.\n\\end{document}\n",
            1,
        ),
        (
            "label",
            "\\documentclass{article}\n\\begin{document}\n\\section{One}\\label{one}See section~\\ref{one} on page~\\pageref{one}.\n\\end{document}\n",
            2,
        ),
    ] {
        let dir = scratch(&format!("latexmk-rounds-{name}"));
        std::fs::write(dir.join(format!("{name}.tex")), src).expect("the document");
        let build = || {
            let out = command(bin, &dir)
                .args(["build", "--json", &format!("{name}.tex")])
                .output()
                .expect("phitex runs");
            assert!(
                out.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let ev = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .find(|e| e["event"] == "build")
                .expect("a build event");
            let pdf = std::fs::read(dir.join(format!("{name}.pdf"))).expect("the PDF");
            (ev["rounds"].as_u64().expect("its rounds"), pdf)
        };
        let (cold, pdf) = build();
        assert_eq!(cold, cold_rounds, "{name}: the rounds from nothing");
        let (again, pdf2) = build();
        assert_eq!(again, 1, "{name}: the rounds rebuilt with its files");
        assert!(
            pdf == pdf2,
            "{name}: the rebuild's PDF is not the first build's"
        );
    }
}

/// The exit statuses, as pdflatex's: 1 for a file that is not there, an
/// unknown command or a document with errors, 0 for `--help`.
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
        Some(1)
    );
    assert_eq!(status(&["check", "x.tex"]), Some(1));
    assert_eq!(status(&["x.tex"]), Some(1));
    assert_eq!(status(&["--help"]), Some(0));
    assert_eq!(status(&["--compat=pdflatex", "-help"]), Some(0));
    assert_eq!(status(&["--compat=pdflatex", "-frob", "x.tex"]), Some(1));
    std::fs::write(dir.join("bad.tex"), "\\undefinedcs\n\\bye\n").expect("the document");
    assert_eq!(status(&["build", "--engine", "tex", "bad.tex"]), Some(1));
}

/// `phitex watch --json` on a plain document (`tex`): a word edited on
/// disk is build 2, an edit of the line it is on, fewer nodes run than
/// cold, and its DVI file is a cold build's of the edited text; an edit
/// that makes an error is build 3, not ok, and its DVI file is written
/// as pdfTeX writes it (the pages shipped despite the error): a cold
/// build's of the broken text.
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
    let failed = std::fs::read(dir.join("story.dvi")).expect("the failed build's DVI");
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
    let broken_dir = scratch("watch-plain-broken");
    std::fs::write(broken_dir.join("story.tex"), &broken).expect("the broken document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &broken_dir)
        .args(["build", "--no-synctex", "--engine", "tex", "story.tex"])
        .output()
        .expect("phitex build runs");
    assert!(!out.status.success(), "the broken document built clean");
    let broken_dvi = std::fs::read(broken_dir.join("story.dvi")).expect("build's failed DVI");
    assert!(
        masked(&failed) == masked(&broken_dvi),
        "the watch's failed DVI is not phitex build's: {}",
        first_difference(&failed, &broken_dvi)
    );
}

/// `phitex watch`, headless: a save that renames the old file away and
/// writes a new one in pieces (vim's `backupcopy=no`, many editors) is
/// one edit of the whole new text: no build starts while no file is at
/// the name (it read an empty text, and the save's text came as an edit
/// absorbed into that build), the edit's line is the line changed.
#[test]
fn watch_save_by_rename_builds_the_new_text() {
    let _one = one();
    let text = std::fs::read_to_string(fixtures().join("plain/story.tex")).expect("story.tex");
    let dir = scratch("watch-rename-save");
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
    let path = dir.join("story.tex");
    let backup = dir.join("story.tex~");
    std::fs::rename(&path, &backup).expect("the old file renamed away");
    std::thread::sleep(Duration::from_millis(100));
    {
        let mut f = std::fs::File::create(&path).expect("the new file");
        for piece in edited.as_bytes().chunks(512) {
            f.write_all(piece).expect("a piece");
            f.flush().expect("flushed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    std::fs::remove_file(&backup).expect("the backup removed");
    let ev = until(&mut lines, "build", &2.into(), &mut seen);
    assert_eq!(ev["ok"], true, "{ev}");
    assert_eq!(
        ev["absorbed"], 0,
        "the build began before the save ended: {ev}"
    );
    assert_eq!(ev["edit"], format!("story.tex:{line}"), "{ev}");
    std::thread::sleep(Duration::from_millis(700));
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let rest: Vec<String> = lines.collect();
    assert!(
        !rest.iter().any(|l| l.contains("\"event\":\"build\"")),
        "a third build ran: {rest:?}"
    );
}

/// LuaLaTeX is not supported, by any name, nor found for a document
/// that needs it (luacode loaded, nothing named), and nothing external
/// runs in its place. (XeLaTeX is: `xelatex_fontspec`.)
#[test]
fn lualatex_refused() {
    let dir = scratch("refused");
    std::fs::write(dir.join("a.tex"), "\\documentclass{article}\n").expect("the document");
    std::fs::write(dir.join("b.tex"), "% !TEX program = lualatex\n\\relax\n")
        .expect("the document");
    std::fs::write(
        dir.join("c.tex"),
        "\\documentclass{article}\n\\usepackage{luacode}\n\\begin{document}x\\end{document}\n",
    )
    .expect("the document");
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    for (args, file, says) in [
        (
            &["build", "--engine", "lualatex", "a.tex"][..],
            "a.pdf",
            "not supported",
        ),
        (&["build", "b.tex"][..], "b.pdf", "not supported"),
        (
            &["build", "c.tex"][..],
            "c.pdf",
            "needs lualatex (it loads luacode)",
        ),
    ] {
        let out = command(bin, &dir).args(args).output().expect("phitex runs");
        let errs = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} succeeded");
        assert!(errs.contains(says), "{args:?}: {errs}");
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
/// status is pdflatex's 1; an option before the command is never taken for TeX's
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
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("--frob"), "the option is not named:\n{err}");
    assert!(err.contains("--help"), "--help is not suggested:\n{err}");
    assert!(err.lines().count() <= 8, "more than a few lines:\n{err}");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["-o", "out", "--frob", "build", "t.tex"])
        .output()
        .expect("phitex runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("--frob") && !err.contains("compat"), "{err}");
}

/// `--report` writes `<job>.phitex-report.txt` beside the outputs: the
/// build and its problems by file and line, and no text of the document
/// (TeX's context lines quote it, the warnings' names cite it); `=full`
/// adds the log and the sources the job read.
#[test]
fn report_has_no_document_text() {
    let dir = scratch("report");
    std::fs::write(
        dir.join("doc.tex"),
        "\\documentclass{article}\n\\begin{document}\nSecretWords \\ref{secretlabel} \\undefinedmacro.\n\\end{document}\n",
    )
    .unwrap();
    let bin = Path::new(env!("CARGO_BIN_EXE_phitex"));
    let out = command(bin, &dir)
        .args(["build", "--report", "doc.tex"])
        .output()
        .expect("phitex runs");
    assert!(String::from_utf8_lossy(&out.stderr).contains("doc.phitex-report.txt"));
    let r = std::fs::read_to_string(dir.join("doc.phitex-report.txt")).expect("the report");
    assert!(
        r.contains("build 1")
            && r.contains("doc.tex:3")
            && r.contains("Undefined control sequence"),
        "{r}"
    );
    for secret in ["SecretWords", "secretlabel", "undefinedmacro"] {
        assert!(
            !r.contains(secret),
            "the report quotes the document ({secret}):\n{r}"
        );
    }
    command(bin, &dir)
        .args(["build", "--report=full", "doc.tex"])
        .output()
        .expect("phitex runs");
    let r = std::fs::read_to_string(dir.join("doc.phitex-report.txt")).expect("the report");
    assert!(
        r.contains("--- doc.log ---") && r.contains("SecretWords"),
        "{r}"
    );
}

/// `--help` is short (the commands, the common options, where the rest
/// are: the user, 2026-10-30) and `--help-all` sectioned with every option
/// (commands, options, examples), both plain off a terminal.
#[test]
fn help_is_sectioned() {
    let dir = scratch("help");
    let help = |flag: &str| {
        let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
            .arg(flag)
            .output()
            .expect("phitex runs");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let short = help("--help");
    assert!(
        short.lines().count() <= 35 && short.contains("--help-all") && short.contains("--jobs"),
        "--help is not the short help:\n{short}"
    );
    assert!(
        !short.contains("--no-view"),
        "--help lists every option:\n{short}"
    );
    assert!(
        !short.contains('\x1b'),
        "escape sequences off a terminal:\n{short}"
    );
    let text = help("--help-all");
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

/// A document named `plain.tex` is not the format's `plain.tex`: the
/// pdfTeX format is built (and checked) away from the document's
/// directory, as fmtutil builds it in a scratch directory.
#[test]
fn document_named_plain() {
    let _one = one();
    let dir = scratch("document_named_plain");
    std::fs::write(dir.join("plain.tex"), "\\tenrm Hello\\bye\n").expect("the text");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["build", "-j", "1", "plain.tex"])
        .output()
        .expect("phitex runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // (the same text under another name: the same PDF but for the
    // file name's hash in /ID)
    std::fs::write(dir.join("other.tex"), "\\tenrm Hello\\bye\n").expect("the text");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["build", "-j", "1", "other.tex"])
        .output()
        .expect("phitex runs");
    assert!(out.status.success());
    let pdf = std::fs::read(dir.join("plain.pdf")).expect("plain.pdf");
    let other = std::fs::read(dir.join("other.pdf")).expect("other.pdf");
    assert_eq!(pdf.len(), other.len());
}

/// A build has no terminal (it runs as pdfTeX's `-interaction=nonstopmode`):
/// a `\read` from the terminal (§484) and a file not found (§530, LaTeX's
/// `\@missingfileerror` reading the terminal) end the job at once with
/// the fatal error, as `pdftex -interaction=nonstopmode` does
/// (`oracle/fixtures/nonstop/`), after the errors before them are logged.
#[test]
fn nonstop_fatal_errors_end_the_job() {
    let _one = one();
    // the LaTeX format, built once before the timed cases (a cold
    // format build is not what the 60 s bound measures)
    let seed = scratch("nonstop-seed");
    std::fs::write(
        seed.join("seed.tex"),
        "\\documentclass{article}\\begin{document}x\\end{document}\n",
    )
    .expect("the seed");
    let _ = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &seed)
        .args(["build", "seed.tex"])
        .output()
        .expect("phitex runs");
    for (name, fatal) in [
        (
            "errread",
            "*** (cannot \\read from terminal in nonstop modes)",
        ),
        ("missplain", "*** (job aborted, file error in nonstop mode)"),
        (
            "missing",
            "*** (cannot \\read from terminal in nonstop modes)",
        ),
    ] {
        let dir = scratch(&format!("nonstop-{name}"));
        std::fs::copy(
            fixtures().join(format!("nonstop/{name}.tex")),
            dir.join(format!("{name}.tex")),
        )
        .expect("the fixture");
        let t = Instant::now();
        let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
            .args(["build", &format!("{name}.tex")])
            .output()
            .expect("phitex runs");
        let errs = String::from_utf8_lossy(&out.stderr);
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "{name}: {:?}",
            t.elapsed()
        );
        assert!(!out.status.success(), "{name}: {errs}");
        assert!(errs.contains(fatal), "{name}: {errs}");
        let log = chars(&std::fs::read(dir.join(format!("{name}.log"))).unwrap_or_default());
        assert!(log.contains(fatal), "{name}: {log}");
        if name == "errread" {
            assert!(log.contains("! a first error."), "{name}: {log}");
        }
        // (pdfTeX writes no PDF file after a fatal error)
        assert!(log.contains("no output PDF file produced"), "{name}: {log}");
        assert!(!dir.join(format!("{name}.pdf")).exists(), "{name}");
    }
}

/// The XeTeX documents (`oracle/fixtures/xetex/docs`) and their
/// `xelatex` oracles (`oracle/fixtures/xetex/oracle/docs`: the first
/// run's XDV and PDF, `SOURCE_DATE_EPOCH=0`, `FORCE_SOURCE_DATE=1`, TeX
/// Live's OpenType and TrueType fonts only, as the CLI's font index has
/// them).
fn xetex_docs() -> PathBuf {
    fixtures().join("xetex/docs")
}

/// `xelatex -interaction=batchmode fontspec.tex` through phitex (one
/// run, as TeX Live's): the xelatex format built and cached, the XDV
/// made a PDF in process by xdvipdfmx, and that PDF is xelatex's, byte
/// for byte.
#[test]
#[ignore = "heavy: the xelatex format; run on accl"]
fn xelatex_fontspec() {
    let _one = one();
    let dir = scratch("xelatex-fontspec");
    std::fs::copy(xetex_docs().join("fontspec.tex"), dir.join("fontspec.tex")).expect("a copy");
    let t = Instant::now();
    let out = command(&as_program("xelatex"), &dir)
        .args(["-interaction=batchmode", "fontspec.tex"])
        .output()
        .expect("phitex runs as xelatex");
    eprintln!(
        "fontspec: xelatex through phitex in {:.2} s, status {:?}\n{}",
        t.elapsed().as_secs_f64(),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = chars(&std::fs::read(dir.join("fontspec.log")).unwrap_or_default());
    assert!(out.status.success(), "fontspec: phitex failed:\n{log}");
    let want =
        std::fs::read(fixtures().join("xetex/oracle/docs/fontspec.pdf")).expect("the oracle");
    let got = std::fs::read(dir.join("fontspec.pdf")).unwrap_or_default();
    assert!(
        got == want,
        "fontspec: the PDF is not xelatex's: {} (ours in {})",
        first_difference(&got, &want),
        dir.display()
    );
}

/// `phitex watch --engine xelatex`, headless, on the fontspec document
/// over a series of edits (words that change the glyphs the fonts embed,
/// a page added and taken away again), each written as an update section
/// of the PDF file (the idle time longer than the series): once the watch
/// is idle, the exact file it writes over the updated one is
/// `phitex build --engine xelatex`'s of the last text, byte for byte, and
/// each section before it left a file that ends as a PDF file does.
#[test]
#[ignore = "heavy: the xelatex format; run on accl"]
fn xelatex_watch_compacts_a_series() {
    let _one = one();
    let text = std::fs::read_to_string(xetex_docs().join("fontspec.tex")).expect("the document");
    let dir = scratch("xelatex-compact");
    std::fs::write(dir.join("fontspec.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--idle",
            "20000",
            "--engine",
            "xelatex",
            "-j",
            "8",
            "fontspec.tex",
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
    let pages = "\\newpage A page of its own, in Pagella: quick brown foxes jump.\n";
    let edits: [&dyn Fn(&str) -> String; 7] = [
        &|t| t.replacen("Pagella text", "Palatino words", 1),
        &|t| t.replacen("\\end{document}", &format!("{pages}\\end{{document}}"), 1),
        &|t| t.replacen("First item", "Erste Ítem", 1),
        &|t| t.replacen("quick brown foxes", "lazy dogs sleep", 1),
        &|t| {
            t.replacen(pages, "", 1)
                .replacen("lazy dogs sleep", "quick brown foxes", 1)
        },
        &|t| t.replacen("Second", "Zweite", 1),
        &|t| t.replacen("Palatino words", "Pagella text", 1),
    ];
    let mut now = text.clone();
    let mut sizes = Vec::new();
    for (k, e) in edits.iter().enumerate() {
        let next = e(&now);
        assert_ne!(next, now, "edit {k} changed nothing");
        now = next;
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dir.join("fontspec.tex"), &now).expect("the edit");
        let line = until(&mut lines, "build", &(k as u64 + 2).into(), &mut seen);
        assert_eq!(line["ok"], true, "edit {k}: {line}");
        let pdf = std::fs::read(dir.join("fontspec.pdf")).expect("the watch's PDF");
        assert!(
            pdf.starts_with(b"%PDF") && pdf.ends_with(b"%%EOF\n"),
            "edit {k}: the file is not a PDF file"
        );
        sizes.push(pdf.len());
    }
    let written = until(&mut lines, "event", &"written".into(), &mut seen);
    assert_eq!(written["exact"], true, "{written}");
    let watched = std::fs::read(dir.join("fontspec.pdf")).expect("the watch's PDF");
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("xelatex-compact-cold");
    std::fs::write(cold_dir.join("fontspec.tex"), &now).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--engine", "xelatex", "-j", "8", "fontspec.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "the cold build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_pdf = std::fs::read(cold_dir.join("fontspec.pdf")).expect("the cold PDF");
    eprintln!(
        "xelatex compaction: the file after each edit {sizes:?} bytes, the exact file {} bytes",
        watched.len()
    );
    assert!(
        sizes.windows(2).all(|w| w[0] < w[1]),
        "an edit wrote the file whole, not an update section: {sizes:?}"
    );
    assert!(
        watched == cold_pdf,
        "the compacted PDF is not the cold build's: {}",
        first_difference(&watched, &cold_pdf)
    );
}

/// `phitex watch --engine xelatex`, headless, on the fontspec document:
/// the cold build writes a PDF; a word edited in the file is built as an
/// edit, and the PDF it writes when idle is `phitex build --engine
/// xelatex`'s of the edited text.
#[test]
#[ignore = "heavy: the xelatex format; run on accl"]
fn xelatex_watch_edit() {
    let _one = one();
    let text = std::fs::read_to_string(xetex_docs().join("fontspec.tex")).expect("the document");
    let dir = scratch("xelatex-watch");
    std::fs::write(dir.join("fontspec.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--engine",
            "xelatex",
            "-j",
            "8",
            "fontspec.tex",
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
    let first = std::fs::read(dir.join("fontspec.pdf")).expect("the watch's first PDF");
    assert!(
        first.starts_with(b"%PDF"),
        "the watch's first PDF is not a PDF"
    );
    let edited = text.replacen("Pagella text", "Palatino words", 1);
    assert_ne!(edited, text);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("fontspec.tex"), &edited).expect("the edit");
    let line = until(&mut lines, "build", &2.into(), &mut seen);
    eprintln!("{line}");
    // (the edit is an update section of the file; the exact file is
    // written over it once the watch is idle)
    let written = until(&mut lines, "event", &"written".into(), &mut seen);
    assert_eq!(written["exact"], true, "{written}");
    let watched = std::fs::read(dir.join("fontspec.pdf")).expect("the watch's PDF");
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("xelatex-watch-cold");
    std::fs::write(cold_dir.join("fontspec.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--engine", "xelatex", "-j", "8", "fontspec.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "the cold build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cold_pdf = std::fs::read(cold_dir.join("fontspec.pdf")).expect("the cold PDF");
    assert!(watched != first, "the edit did not change the PDF");
    assert!(
        watched == cold_pdf,
        "the watch's PDF after the edit is not the cold build's: {}",
        first_difference(&watched, &cold_pdf)
    );
}

/// The TFM files of TeX Live the plain document names, at their texmf
/// paths.
fn plain_tfms(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut names: Vec<String> = text
        .split("\\font\\")
        .skip(1)
        .filter_map(|r| {
            let r = r.split_once('=')?.1;
            let n: String = r.chars().take_while(char::is_ascii_alphanumeric).collect();
            (!n.is_empty()).then_some(n)
        })
        .collect();
    names.sort();
    names.dedup();
    let mut files = Vec::new();
    for n in &names {
        let out = Command::new("kpsewhich")
            .arg(format!("{n}.tfm"))
            .output()
            .expect("kpsewhich (TeX Live) on the test machine");
        let p = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !p.is_empty() {
            files.push((
                format!("fonts/tfm/public/cm/{n}.tfm"),
                std::fs::read(&p).expect("a TFM"),
            ));
        }
    }
    files
}

/// A Shelf release of `files` (texmf path, bytes) in one pack, its table
/// and `release-tl2026.7.json` of schema 1, served over HTTP on 127.0.0.1
/// by a thread of the test's: the port, and how many requests it
/// answered. `tamper`: the pack served differs from the table's by its
/// last byte.
#[allow(clippy::too_many_lines, reason = "a release made and served, in order")]
fn shelf_server(
    files: &[(String, Vec<u8>)],
    tamper: bool,
) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::collections::HashMap;
    use std::io::Read as _;
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let pack = phitex_shelf::pack(&refs);
    let index: String = files.iter().fold(String::new(), |mut s, (n, _)| {
        let _ = writeln!(s, "{n}\tcm-1\t\t");
        s
    });
    let table = format!(
        "cm-1\t{}\t{}\n",
        phitex_shelf::sha256_hex(&pack),
        pack.len()
    );
    let release = serde_json::json!({
        "schema": 1, "release": "tl2026.7", "index": "index-tl2026.7.tsv.gz",
        "pack_table": "packs-tl2026.7.tsv.gz", "deps": ["pdftex", "xetex"],
        "search": {"pdftex": {"tex": ["tex/latex", "tex/generic", "tex"], "tfm": ["fonts/tfm"]}},
    });
    let mut served = pack.clone();
    if tamper {
        *served.last_mut().expect("a pack") ^= 1;
    }
    let site: HashMap<String, Vec<u8>> = [
        (
            "release-tl2026.7.json".to_owned(),
            release.to_string().into_bytes(),
        ),
        (
            "index-tl2026.7.tsv.gz".to_owned(),
            phitex_shelf::gzip_stored(index.as_bytes()),
        ),
        (
            "packs-tl2026.7.tsv.gz".to_owned(),
            phitex_shelf::gzip_stored(table.as_bytes()),
        ),
        ("h/cm-1.pack".to_owned(), served),
    ]
    .into();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let port = listener.local_addr().expect("its address").port();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = asked.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { continue };
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && c.read(&mut b).unwrap_or(0) == 1 {
                head.push(b[0]);
            }
            let line = String::from_utf8_lossy(&head)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            let path = line.split(' ').nth(1).unwrap_or("");
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = match path.strip_prefix("/tl2026/").and_then(|p| site.get(p)) {
                Some(body) => c
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .and_then(|()| c.write_all(body)),
                None => c.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                ),
            };
        }
    });
    (port, asked)
}

/// `phitex build --shelf`: TeX's files from Shelf (docs/spec-editor-protocol.md
/// §5), its index and pack fetched over HTTP, checked against the pack
/// table and kept in the package cache: the DVI is TeX Live's build's,
/// byte for byte; a second build reads the cache only (the server
/// asked nothing more); a pack that differs from the table is refused,
/// the build failing and saying so.
#[test]
fn build_from_shelf() {
    let _one = one();
    let text = plain() + "Alpha {\\bf bravo} {\\it charlie} $x^2$.\\bye\n";
    let run = |dir: &Path, args: &[&str], url: Option<u16>| {
        let mut c = command(Path::new(env!("CARGO_BIN_EXE_phitex")), dir);
        c.args(["build", "--ini", "--etex", "--output-format", "dvi"])
            .args(args)
            .arg("doc.tex")
            .env("PHITEX_CACHE_DIR", dir.join("cache"))
            .env_remove("PHITEX_SHELF_URL");
        if let Some(p) = url {
            c.env("PHITEX_SHELF_URL", format!("http://127.0.0.1:{p}/tl2026"));
        }
        c.output().expect("phitex runs")
    };
    let tl = scratch("shelf-texlive");
    std::fs::write(tl.join("doc.tex"), &text).expect("the document");
    let out = run(&tl, &[], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let want = std::fs::read(tl.join("doc.dvi")).expect("TeX Live's DVI");
    let dir = scratch("shelf-build");
    std::fs::write(dir.join("doc.tex"), &text).expect("the document");
    let (port, asked) = shelf_server(&plain_tfms(&text), false);
    let out = run(&dir, &["--shelf"], Some(port));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(dir.join("doc.dvi")).expect("Shelf's DVI"),
        want,
        "the DVI from Shelf's files is not TeX Live's"
    );
    let n = asked.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(n, 4, "release.json, the index, the pack table, one pack");
    assert!(
        dir.join("cache/shelf/tl2026.7/h/cm-1.pack").is_file(),
        "the pack kept"
    );
    // (again, the cache's only: no server)
    std::fs::remove_file(dir.join("doc.dvi")).expect("the DVI");
    let out = run(&dir, &["--shelf"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(dir.join("doc.dvi")).expect("the DVI again"),
        want
    );
    // (a pack that is not the table's: refused, never kept)
    let bad = scratch("shelf-tampered");
    std::fs::write(bad.join("doc.tex"), &text).expect("the document");
    let (port, _) = shelf_server(&plain_tfms(&text), true);
    let out = run(&bad, &["--shelf"], Some(port));
    assert!(!out.status.success(), "a tampered pack built");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SHA-256"),
        "the refusal not said: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !bad.join("cache/shelf/tl2026.7/h/cm-1.pack").exists(),
        "a tampered pack kept"
    );
}

/// XeTeX from Shelf (docs/spec-editor-protocol.md §5): a font by name
/// found through the release's font index (`fonts/misc/partex/
/// fontindex.pxfi`, its paths relative to the tree, as a release ships it),
/// the face read from the release's pack, the XDV naming it under TeX
/// Live's tree as xelatex's does; a release without a font index refuses
/// an XeTeX job at the start, saying why.
#[test]
fn xetex_from_shelf() {
    let _one = one();
    let out = Command::new("kpsewhich")
        .args(["-var-value", "TEXMFDIST"])
        .output()
        .expect("kpsewhich (TeX Live) on the test machine");
    let dist = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let rel = "fonts/opentype/public/lm/lmroman10-regular.otf";
    let face = std::fs::read(Path::new(&dist).join(rel)).expect("Latin Modern Roman");
    let abs = format!("{dist}/{rel}");
    let mut e = phitex_otf::index::FaceEntry::read(&abs, std::sync::Arc::from(face.clone()), 0);
    e.fc_names = Some(phitex_otf::xetex::fontmgr::fc_names(&e));
    let index = phitex_otf::index::FontIndex { entries: vec![e] }.relative_to(&dist);
    let text = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=\"Latin Modern Roman\" at 10pt \\x \\setbox0\\hbox{Hi}\\shipout\\box0 \\end\n";
    let run = |dir: &Path, port: u16| {
        std::fs::write(dir.join("doc.tex"), text).expect("the document");
        command(Path::new(env!("CARGO_BIN_EXE_phitex")), dir)
            .args([
                "build",
                "--shelf",
                "--ini",
                "--engine",
                "xetex",
                "--output-format",
                "xdv",
                "doc.tex",
            ])
            .env("PHITEX_CACHE_DIR", dir.join("cache"))
            .env(
                "PHITEX_SHELF_URL",
                format!("http://127.0.0.1:{port}/tl2026/"),
            )
            .output()
            .expect("phitex runs")
    };
    let dir = scratch("shelf-xetex");
    let (port, _) = shelf_server(
        &[
            (rel.to_owned(), face),
            (
                "fonts/misc/partex/fontindex.pxfi".to_owned(),
                index.to_bytes(),
            ),
        ],
        false,
    );
    let out = run(&dir, port);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let xdv = std::fs::read(dir.join("doc.xdv")).expect("the XDV");
    let named = format!("/usr/share/texmf-dist/{rel}");
    assert!(
        xdv.windows(named.len()).any(|w| w == named.as_bytes()),
        "the XDV names the face under TeX Live's tree"
    );
    // (no font index in the release: refused, said)
    let none = scratch("shelf-xetex-none");
    let (port, _) = shelf_server(&plain_tfms(&plain()), false);
    let out = run(&none, port);
    assert!(!out.status.success(), "built without a font index");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no font index"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `phitex watch`'s idle point says the file on disk is exact
/// (`{"event": "written", "exact": true}`) whether or not it had to be
/// compacted: a build whose output needs no compaction (a DVI file,
/// written whole) still has it said, once, after the build, as a client
/// waiting for the exact file expects (docs/spec-editor-protocol.md §2.1).
#[test]
fn watch_says_the_file_is_exact_with_nothing_to_compact() {
    let _one = one();
    let dir = scratch("watch-exact-said");
    std::fs::write(dir.join("doc.tex"), plain() + "Alpha.\\bye\n").expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "--idle",
            "200",
            "--ini",
            "--etex",
            "doc.tex",
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
    let written = until(&mut lines, "event", &"written".into(), &mut seen);
    assert_eq!(written["exact"], true, "{written}");
    assert_eq!(written["build"], 1, "{written}");
    assert!(
        written["output"]
            .as_str()
            .is_some_and(|o| o.ends_with("doc.dvi")),
        "{written}"
    );
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
}
