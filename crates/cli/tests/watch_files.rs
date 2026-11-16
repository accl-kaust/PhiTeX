//! `phitex watch` sees every file its build read change on disk, on every
//! system (Linux's inotify, macOS's `FSEvents`, Windows's
//! `ReadDirectoryChangesW`): an image, a figure in a subfolder, a file read
//! through a symbolic link to a folder elsewhere and an `\input` file,
//! each saved over in place and by renaming a new file over it (an
//! editor's atomic save), the document's text untouched. Each change is
//! a new build whose PDF is the clean build's of the files as they are
//! then. Headless and as an editor's language server (`--lsp`, the
//! document open in the editor, its folder named through a symbolic
//! link as an editor that opened it so has it). No TeX Live: `-ini`
//! documents, the images pdfTeX's own fixtures, a `texmf.cnf` of one
//! line ([`env`]).

use std::fmt::Write as _;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a change may take to reach the PDF.
const CHANGE_LIMIT: Duration = Duration::from_secs(30);

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_phitex")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../oracle/fixtures/pdf")
        .join(name)
}

/// A fresh directory for a test's run.
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("watch-files")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d
}

/// Two images, their PDFs' streams different.
fn images() -> [Vec<u8>; 2] {
    ["png-gray.png", "png-rgb16.png"].map(|n| std::fs::read(fixture(n)).expect("a PNG fixture"))
}

/// The two texts of the `\input` file.
const PARTS: [&str; 2] = ["\\pdfinfo{/Subject (one)}\n", "\\pdfinfo{/Subject (two)}\n"];

/// The files a build reads, as the project names them, and whether the
/// system can make the symbolic link the last one is read through.
fn files() -> Vec<&'static str> {
    let mut f = vec!["part.tex", "fig.png", "figs/sub.png"];
    if cfg!(unix) {
        f.push("linked/far.png");
    }
    f
}

/// The document: `part.tex` input, the images shipped.
fn document() -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\pdfoutput=1 \\pdfcompresslevel=0\n\
         \\pdfpagewidth=100bp \\pdfpageheight=100bp\n\
         \\input part\n\
         \\shipout\\hbox{",
    );
    for f in files().iter().filter(|f| !is_tex(f)) {
        let _ = writeln!(
            s,
            "\\pdfximage width 20bp {{{f}}}\\pdfrefximage\\pdflastximage"
        );
    }
    s.push_str("}\n\\end\n");
    s
}

/// The project in `root`: `doc.tex` and its files, each in its state
/// `state[i]` (0 or 1); `linked` a symbolic link to `../elsewhere`
/// (beside `root`) on Unix.
fn lay_out(root: &Path, state: &[usize]) {
    std::fs::create_dir_all(root.join("figs")).expect("figs");
    std::fs::write(root.join("doc.tex"), document()).expect("the document");
    if cfg!(unix) {
        let far = root.with_file_name(format!(
            "{}-elsewhere",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&far).expect("the folder elsewhere");
        if !root.join("linked").exists() {
            link(&far, &root.join("linked"));
        }
    }
    for (f, &k) in files().iter().zip(state) {
        std::fs::write(root.join(f), content(f, k)).expect("a file");
    }
}

/// A symbolic link at `at` to `to` (Unix only: Windows's need a
/// privilege).
fn link(to: &Path, at: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(to, at).expect("a symbolic link");
    #[cfg(not(unix))]
    unreachable!("no links made off Unix: {} {}", to.display(), at.display());
}

fn is_tex(file: &str) -> bool {
    Path::new(file).extension().is_some_and(|e| e == "tex")
}

fn content(file: &str, k: usize) -> Vec<u8> {
    if is_tex(file) {
        PARTS[k].as_bytes().to_vec()
    } else {
        images()[k].clone()
    }
}

/// `c` run as the tests run it: its own home and cache, the dates fixed,
/// the raw file events logged, and a TeX Live of one `texmf.cnf` that
/// names a tree with nothing in it (a `-ini` document reads only its
/// own files).
fn env<'a>(c: &'a mut Command, dir: &Path) -> &'a mut Command {
    let home = dir.join("home");
    let cnf = dir.join("texmf-cnf");
    let tree = dir.join("texmf");
    let _ = std::fs::create_dir_all(&home);
    let _ = std::fs::create_dir_all(&cnf);
    let _ = std::fs::create_dir_all(&tree);
    let _ = std::fs::write(
        cnf.join("texmf.cnf"),
        format!(
            "TEXMF = {}\nTEXINPUTS = .\n",
            tree.to_string_lossy().replace('\\', "/")
        ),
    );
    c.env("PHITEX_CACHE_DIR", dir.join("cache"))
        .env("TEXMFCNF", &cnf)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("APPDATA", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env("XDG_RUNTIME_DIR", &home)
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .env("PHITEX_WATCH_EVENTS", "1")
        .env_remove("PHITEX_COMPAT")
        .env_remove("TEXINPUTS")
}

/// The clean build's PDF of the project with its files in `state`.
fn clean(dir: &Path, state: &[usize]) -> Vec<u8> {
    let root = dir.join(format!(
        "clean-{}",
        state.iter().map(ToString::to_string).collect::<String>()
    ));
    let pdf = root.join("doc.pdf");
    if let Ok(b) = std::fs::read(&pdf) {
        return b;
    }
    lay_out(&root, state);
    let out = env(&mut Command::new(bin()), dir)
        .current_dir(&root)
        .args(["build", "--no-synctex", "--ini", "--etex", "doc.tex"])
        .output()
        .expect("phitex build runs");
    assert!(
        out.status.success(),
        "the clean build of {state:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(&pdf).expect("the clean PDF")
}

/// A running watch, its standard error kept in a file and its standard
/// output drained (its lines kept), killed when dropped.
struct Watch {
    child: Child,
    err: PathBuf,
    out: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Watch {
    fn said(&self) -> String {
        format!(
            "its output:\n{}\nits standard error:\n{}",
            self.out.lock().unwrap().join("\n"),
            std::fs::read_to_string(&self.err).unwrap_or_default()
        )
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(dir: &Path, cwd: &Path, args: &[&str], lsp: bool) -> Watch {
    let err = dir.join(if lsp { "lsp.stderr" } else { "watch.stderr" });
    let mut child = env(&mut Command::new(bin()), dir)
        .current_dir(cwd)
        .arg("watch")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&err).expect("a file for its errors"))
        .spawn()
        .expect("phitex watch runs");
    let out: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
    let stdout = child.stdout.take().expect("its output");
    let kept = out.clone();
    std::thread::spawn(move || {
        if !lsp {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                kept.lock().unwrap().push(l);
            }
            return;
        }
        // (the server's messages, framed)
        let mut r = BufReader::new(stdout);
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
                if let Some((k, v)) = h.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut b = vec![0; len];
            if r.read_exact(&mut b).is_err() {
                return;
            }
            kept.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&b).into_owned());
        }
    });
    Watch { child, err, out }
}

fn lsp_send(w: &mut Watch, v: &serde_json::Value) {
    let b = v.to_string();
    let i = w.child.stdin.as_mut().expect("its input");
    write!(i, "Content-Length: {}\r\n\r\n{b}", b.len()).expect("the server reads");
    i.flush().expect("the server reads");
}

fn uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Waits for `pdf` to be `want`: the watch's PDF after `what`.
fn wait_for(w: &Watch, pdf: &Path, want: &[u8], what: &str) {
    let end = Instant::now() + CHANGE_LIMIT;
    loop {
        if std::fs::read(pdf).is_ok_and(|b| b == want) {
            return;
        }
        assert!(
            Instant::now() < end,
            "{what}: the watch's PDF is not the clean build's after {CHANGE_LIMIT:?} ({}); {}",
            if pdf.exists() {
                "a PDF there"
            } else {
                "no PDF"
            },
            w.said()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The changes, each file in place then by a rename over it, on the
/// watch `w` of the project at `root` (as the watch knows it: `at`).
fn change_each(dir: &Path, w: &Watch, at: &Path) {
    let pdf = at.join("doc.pdf");
    let mut state = vec![0; files().len()];
    wait_for(w, &pdf, &clean(dir, &state), "the first build");
    for (i, f) in files().iter().enumerate() {
        for how in [
            "in place",
            "in place in two writes 0.3 s apart",
            "by a rename over it",
            "removed and written anew",
        ] {
            let before = clean(dir, &state);
            state[i] = 1 - state[i];
            assert!(
                clean(dir, &state) != before,
                "{f}'s change changes no byte of the clean PDF"
            );
            let p = at.join(f);
            let bytes = content(f, state[i]);
            if how.starts_with("in place") {
                // (truncated and written, the same file; a program that
                // writes a large file in pieces, a pause between them)
                let mut h = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&p)
                    .expect("the file");
                if how == "in place" {
                    h.write_all(&bytes).expect("written");
                } else {
                    let half = bytes.len() / 2;
                    h.write_all(&bytes[..half]).expect("written");
                    h.flush().expect("written");
                    std::thread::sleep(Duration::from_millis(300));
                    h.write_all(&bytes[half..]).expect("written");
                }
            } else if how.starts_with("removed") {
                std::fs::remove_file(&p).expect("removed");
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(&p, &bytes).expect("written anew");
            } else {
                // (a new file beside it, renamed to its name)
                let tmp = p.with_file_name(".phitex-test-save.tmp");
                std::fs::write(&tmp, &bytes).expect("the new file");
                std::fs::rename(&tmp, &p).expect("renamed over");
            }
            wait_for(w, &pdf, &clean(dir, &state), &format!("{f} saved {how}"));
        }
    }
}

/// Headless: `phitex watch --write --exact-pdf doc.tex`.
#[test]
fn watch_rebuilds_on_every_file_saved_over() {
    let dir = scratch("headless");
    let root = dir.join("project");
    lay_out(&root, &vec![0; files().len()]);
    let mut w = start(
        &dir,
        &root,
        &[
            "--write",
            "--exact-pdf",
            "--json",
            "--no-view",
            "--ini",
            "--etex",
            "doc.tex",
        ],
        false,
    );
    change_each(&dir, &w, &root);
    if let Some(i) = w.child.stdin.as_mut() {
        let _ = i.write_all(b"q\n");
    }
}

/// As an editor's language server (`--lsp`), the document open in the
/// editor and its folder named as the editor has it: through a symbolic
/// link to it on Unix (a folder opened by another path: macOS's
/// `/var` for `/private/var`, a synced folder's link).
#[test]
fn watch_lsp_rebuilds_on_every_file_saved_over() {
    use serde_json::json;
    let dir = scratch("lsp");
    let real = dir.join("project");
    lay_out(&real, &vec![0; files().len()]);
    let root = if cfg!(unix) {
        let alias = dir.join("alias");
        link(&real, &alias);
        alias
    } else {
        real.clone()
    };
    let mut w = start(
        &dir,
        &root,
        &[
            "--write",
            "--exact-pdf",
            "--lsp",
            "--ini",
            "--etex",
            "doc.tex",
        ],
        true,
    );
    lsp_send(
        &mut w,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null,
            "rootUri": uri(&root),
            "capabilities": {"window": {"workDoneProgress": true}},
        }}),
    );
    lsp_send(
        &mut w,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    lsp_send(
        &mut w,
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri(&root.join("doc.tex")), "languageId": "latex", "version": 1, "text": document()},
        }}),
    );
    change_each(&dir, &w, &root);
    lsp_send(
        &mut w,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
    );
    lsp_send(&mut w, &json!({"jsonrpc": "2.0", "method": "exit"}));
}
