//! What a new user met at a terminal (the newbie tester's session of
//! 2026-11-25): the help's order, a chapter built for its document, the
//! main file picked, Ctrl-C and q in a watch, the viewer opened, a folder
//! watch's tree and its one "nothing written" line, `phitex doctor`, and
//! an LSP client with no `PhiTeX` plugin. TeX's files are TeX Live's where
//! its `kpsewhich` is on the path (the gate's machines, which reach no
//! network), else Shelf's (a machine with no TeX Live).

#![allow(clippy::too_many_lines)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_phitex"))
}

/// Whether TeX Live is here: its `kpsewhich` on the path.
fn texlive() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("kpsewhich").is_file()))
}

/// A fresh directory for a test's run, its settings saying `files =
/// "shelf"` where there is no TeX Live. (Shelf's always: the gate, with no
/// network, failed every test here at Shelf's release index.)
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("ux-term")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".home/config/phitex")).expect("a scratch directory");
    std::fs::write(
        d.join(".home/config/phitex/phitex.toml"),
        if texlive() { "" } else { "files = \"shelf\"\n" },
    )
    .expect("the settings");
    d
}

/// The environment of a phitex run in `dir`: its settings, state and
/// cache its own.
fn env(c: &mut Command, dir: &Path) {
    let home = dir.join(".home");
    c.env(
        "PHITEX_CACHE_DIR",
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
    )
    .env("XDG_CONFIG_HOME", home.join("config"))
    .env("XDG_STATE_HOME", home.join("state"))
    .env("XDG_DATA_HOME", home.join("data"))
    .env("XDG_RUNTIME_DIR", home.join("run"))
    .env("SOURCE_DATE_EPOCH", "0")
    .env("FORCE_SOURCE_DATE", "1")
    .env_remove("PHITEX_COMPAT")
    .env_remove("NO_COLOR")
    .env_remove("CLICOLOR_FORCE")
    .env_remove("CI");
    let _ = std::fs::create_dir_all(home.join("run"));
}

fn run(dir: &Path, args: &[&str]) -> Output {
    let mut c = Command::new("timeout");
    c.arg("600").arg(bin()).args(args).current_dir(dir);
    env(&mut c, dir);
    c.stdin(Stdio::null()).output().expect("phitex runs")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A plain TeX document (built with --ini --etex: no format to make).
const TINY: &str = "\\pdfoutput=1 \\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x Hi\\end\n";

/// One with a `\\documentclass` it defines: a document, as LaTeX's are
/// found, built as plain TeX.
const DOC: &str = "\\pdfoutput=1 \\catcode`\\{=1 \\catcode`\\}=2 \\def\\documentclass{}\\documentclass\n\\font\\x=cmr10 \\x Hi\\end\n";

fn wait(child: &mut std::process::Child, secs: u64) -> Option<std::process::ExitStatus> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(s) = child.try_wait().expect("the child") {
            return Some(s);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A phitex command on a terminal (`script` gives it one), its keys
/// written to it, what it printed kept in `typescript.txt`.
struct Term {
    child: std::process::Child,
    keys: std::process::ChildStdin,
    log: PathBuf,
}

impl Term {
    fn start(dir: &Path, line: &str, vars: &[(&str, &str)], unset: &[&str]) -> Term {
        let log = dir.join("typescript.txt");
        let _ = std::fs::remove_file(&log);
        let mut c = Command::new("script");
        c.args(["-qefc", line]).arg(&log).current_dir(dir);
        env(&mut c, dir);
        c.env("TERM", "xterm")
            .env("COLUMNS", "100")
            .env("BROWSER", "true");
        for k in unset {
            c.env_remove(k);
        }
        for (k, v) in vars {
            c.env(k, v);
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("script runs");
        let keys = child.stdin.take().expect("its input");
        Term { child, keys, log }
    }

    fn said(&self) -> String {
        text(&std::fs::read(&self.log).unwrap_or_default())
    }

    /// Wait until what it printed holds `what` (its colours and the
    /// status lines' redraws left in).
    fn until(&self, what: &str, secs: u64) -> String {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let s = self.said();
            if s.contains(what) {
                return s;
            }
            assert!(Instant::now() < end, "never said {what:?}:\n{s}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn key(&mut self, k: &[u8]) {
        self.keys.write_all(k).expect("a key");
        self.keys.flush().expect("a key");
    }
}

/// `phitex` and `--help` lead with what `PhiTeX` is; the agents' loop is
/// the help's last section. `--help-all` gives each option one
/// description (clap printed the short one, then the long one, back to
/// back), and --spec offers no value that cannot be used.
#[test]
fn help_for_a_person_first() {
    let dir = scratch("help");
    let short = text(&run(&dir, &["--help"]).stdout);
    let first = short.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    assert!(first.starts_with("PhiTeX: "), "{short}");
    let agents = short.find("For agents:").expect("the agents' section");
    assert!(
        short.find("Options:").is_some_and(|o| o < agents),
        "{short}"
    );
    let all = text(&run(&dir, &["watch", "--help-all"]).stdout);
    for (short_help, long_help) in [
        (
            "pdflatex (default), xelatex",
            "pdflatex (the default for LaTeX)",
        ),
        (
            "Threads (default: 1)",
            "The engine's threads (default: PHITEX_THREADS",
        ),
        (
            "More detail: each pass, each warning, the cache",
            "More detail: each pass over the document",
        ),
        (
            "open the viewer in the browser (the default where",
            "open the viewer in the browser, or start",
        ),
    ] {
        assert!(all.contains(long_help), "{long_help}:\n{all}");
        assert!(
            !all.contains(short_help),
            "{short_help} beside its long help:\n{all}"
        );
    }
    assert!(!all.contains("not usable yet"), "{all}");
    assert!(!all.contains("predicted, all"), "{all}");
    assert!(all.contains("--no-open"), "{all}");
    let top = text(&run(&dir, &["--help-all"]).stdout);
    assert!(
        top.contains("Agents:") && top.contains("For agents:"),
        "{top}"
    );
    // (the hidden value still works, for whoever used it)
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let out = run(
        &dir,
        &["build", "--ini", "--etex", "--spec", "all", "doc.tex"],
    );
    assert!(
        !text(&out.stderr).contains("invalid value"),
        "{}",
        text(&out.stderr)
    );
}

/// A chapter named on the command line (no `\documentclass`, included by
/// another file) builds the document that includes it, said; never the
/// chapter alone as plain TeX.
#[test]
fn a_chapter_builds_its_document() {
    let dir = scratch("chapter");
    std::fs::create_dir_all(dir.join("chapters")).expect("a folder");
    // (plain TeX, built --ini: the `\documentclass` in a comment marks it
    // the document, as the editors find it)
    std::fs::write(
        dir.join("thesis.tex"),
        "% \\documentclass: the main file\n\\pdfoutput=1 \\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x\n\\input{chapters/intro}\n\\end\n",
    )
    .expect("the main file");
    std::fs::write(dir.join("chapters/intro.tex"), "Introduction.\n").expect("the chapter");
    let out = run(&dir, &["build", "--ini", "--etex", "chapters/intro.tex"]);
    let err = text(&out.stderr);
    assert!(
        err.contains("intro.tex is part of thesis.tex: building thesis.tex"),
        "{err}"
    );
    assert!(out.status.success(), "{err}");
    assert!(dir.join("thesis.pdf").is_file(), "{err}");
    assert!(!dir.join("chapters/intro.pdf").exists(), "{err}");
    // (a document of its own is built as named)
    std::fs::write(dir.join("chapters/alone.tex"), TINY).expect("a document");
    let err = text(&run(&dir, &["build", "--ini", "--etex", "chapters/alone.tex"]).stderr);
    assert!(!err.contains("is part of"), "{err}");
}

/// No file named and no phitex.toml: the one `.tex` file with a
/// `\documentclass` is built, said; several are listed, with the command
/// that names one.
#[test]
fn the_main_file_picked() {
    let dir = scratch("pick");
    std::fs::write(dir.join("paper.tex"), format!("% \\documentclass\n{TINY}"))
        .expect("the document");
    std::fs::write(dir.join("part.tex"), "Words.\n").expect("a part");
    let out = run(&dir, &["build", "--ini", "--etex"]);
    let err = text(&out.stderr);
    // (a comment's \documentclass is no document: none found)
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(
        err.contains("no .tex file here has a \\documentclass"),
        "{err}"
    );
    std::fs::write(dir.join("paper.tex"), DOC).expect("the document");
    let out = run(&dir, &["build", "--ini", "--etex"]);
    let err = text(&out.stderr);
    assert!(err.contains("no file named: building paper.tex"), "{err}");
    assert!(dir.join("paper.pdf").is_file(), "{err}");
    std::fs::write(dir.join("other.tex"), "\\documentclass{article}\n").expect("a second");
    for verb in ["build", "watch"] {
        let out = run(&dir, &[verb]);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{err}");
        assert!(
            err.contains("other.tex, paper.tex")
                && err.contains(&format!("phitex {verb} other.tex")),
            "{err}"
        );
    }
}

/// Ctrl-C quits a watch at once: nothing asked, nothing written. `q`
/// asks, its keys said (w writes main.pdf here), the hints cut at words.
#[test]
fn ctrl_c_quits_at_once_q_asks() {
    let dir = scratch("quit");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let line = format!("{} watch --no-open --ini --etex doc.tex", bin().display());
    let mut t = Term::start(&dir, &line, &[], &[]);
    t.until("1 page", 300);
    t.key(b"\x03");
    assert!(
        wait(&mut t.child, 30).is_some(),
        "Ctrl-C did not end the watch:\n{}",
        t.said()
    );
    let said = t.said();
    assert!(!said.contains("here?"), "Ctrl-C asked:\n{said}");
    assert!(!dir.join("doc.pdf").exists(), "Ctrl-C wrote the PDF");
    let mut t = Term::start(&dir, &line, &[], &[]);
    t.until("1 page", 300);
    t.key(b"q");
    let said = t.until("write doc.pdf here?", 30);
    assert!(said.contains("w (or y): write doc.pdf here"), "{said}");
    t.key(b"w");
    assert!(
        wait(&mut t.child, 60).is_some(),
        "w did not end the watch:\n{}",
        t.said()
    );
    assert!(
        dir.join("doc.pdf").is_file(),
        "w wrote no PDF:\n{}",
        t.said()
    );
}

/// A watch on a terminal with a display opens its viewer; over SSH, with
/// --no-open, or with no display it does not; the address is printed
/// each time.
#[test]
fn the_viewer_opens_where_there_is_a_display() {
    let dir = scratch("open");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let opened = dir.join("opened.txt");
    let browser = dir.join("browser.sh");
    std::fs::write(
        &browser,
        format!("#!/bin/sh\necho \"$1\" >> {}\n", opened.display()),
    )
    .expect("a browser");
    Command::new("chmod")
        .arg("+x")
        .arg(&browser)
        .status()
        .expect("chmod");
    let b = browser.to_string_lossy().into_owned();
    let line = format!("{} watch --ini --etex doc.tex", bin().display());
    for (vars, unset, extra, opens) in [
        (
            vec![("DISPLAY", ":0")],
            vec!["SSH_CONNECTION", "SSH_TTY", "WAYLAND_DISPLAY"],
            "",
            true,
        ),
        (
            vec![("DISPLAY", ":0"), ("SSH_CONNECTION", "1 2 3 4")],
            vec!["SSH_TTY"],
            "",
            false,
        ),
        (
            vec![("DISPLAY", ":0")],
            vec!["SSH_CONNECTION", "SSH_TTY"],
            " --no-open",
            false,
        ),
        (
            vec![],
            vec!["DISPLAY", "WAYLAND_DISPLAY", "SSH_CONNECTION", "SSH_TTY"],
            "",
            false,
        ),
    ] {
        let _ = std::fs::remove_file(&opened);
        let mut vars = vars;
        vars.push(("BROWSER", &b));
        let mut t = Term::start(&dir, &format!("{line}{extra}"), &vars, &unset);
        let said = t.until("1 page", 300);
        assert!(said.contains("http://127.0.0.1:"), "{said}");
        // (a browser the watch starts runs within its first seconds)
        std::thread::sleep(Duration::from_secs(5));
        t.key(b"\x03");
        assert!(wait(&mut t.child, 30).is_some(), "{}", t.said());
        assert_eq!(opened.is_file(), opens, "{vars:?} {extra}:\n{}", t.said());
    }
}

/// A folder watch's terminal lists the folder's documents before any is
/// opened; Enter starts the highlighted one; "nothing written here" is
/// said once for the watch, not once a document.
#[test]
fn a_folder_watch_from_the_terminal() {
    let dir = scratch("folder");
    for d in ["a", "b"] {
        std::fs::create_dir_all(dir.join(d)).expect("a folder");
        // (a \documentclass the plain TeX run defines: a document)
        std::fs::write(dir.join(d).join("doc.tex"), DOC).expect("a document");
    }
    let line = format!("{} watch --no-open --ini --etex .", bin().display());
    let mut t = Term::start(&dir, &line, &[], &[]);
    t.until("not started · Enter starts it", 60);
    // (↓ Enter: a/doc.tex; ↓ Enter: b/doc.tex)
    t.key(b"\x1b[B");
    std::thread::sleep(Duration::from_millis(300));
    t.key(b"\r");
    t.until("Built · 1 page", 300);
    t.key(b"\x1b[B");
    std::thread::sleep(Duration::from_millis(300));
    t.key(b"\r");
    t.until("b/doc.tex:", 120);
    std::thread::sleep(Duration::from_secs(3));
    t.key(b"\x03");
    assert!(wait(&mut t.child, 60).is_some(), "{}", t.said());
    let said = t.said();
    assert_eq!(said.matches("files: nothing written").count(), 1, "{said}");
}

/// `phitex doctor`: the program, its protocol, the running watches, and
/// the issue link at its end; `--json` one object.
#[test]
fn doctor_reports_the_setup() {
    let dir = scratch("doctor");
    let out = run(&dir, &["doctor"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let t = text(&out.stdout);
    assert!(t.starts_with("PhiTeX doctor"), "{t}");
    assert!(
        t.contains("editor protocol") && t.contains("Running watches"),
        "{t}"
    );
    assert!(
        t.trim_end()
            .ends_with("https://github.com/accl-kaust/PhiTeX/issues/new/choose"),
        "{t}"
    );
    let v: Value = serde_json::from_slice(&run(&dir, &["doctor", "--json"]).stdout).expect("JSON");
    assert!(v["version"].is_string() && v["watches"].is_array(), "{v}");
}

/// An LSP message to the server, framed.
fn lsp_send(w: &mut impl std::io::Write, v: &Value) {
    let b = v.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{b}", b.len()).expect("the server reads");
    w.flush().expect("the server reads");
}

/// The server's messages, read on a thread of their own.
fn lsp_messages(out: std::process::ChildStdout) -> std::sync::mpsc::Receiver<Value> {
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
                if let Some((k, v)) = h.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    len = v.trim().parse().expect("a length");
                }
            }
            let mut b = vec![0; len];
            std::io::Read::read_exact(&mut r, &mut b).expect("the message");
            if tx.send(serde_json::from_slice(&b).expect("JSON")).is_err() {
                return;
            }
        }
    });
    rx
}

fn uri(p: &Path) -> String {
    format!("file://{}", p.display())
}

/// An editor's LSP client with no `PhiTeX` plugin (vim-lsp) is shown the
/// viewer's address (window/showMessage); `PhiTeX`'s own plugins
/// (`initializationOptions.client`) are not. A problem typed in an
/// unsaved buffer of a chapter is at its column, not the line's start.
#[test]
fn lsp_viewer_address_and_columns() {
    for plugin in [false, true] {
        let dir = scratch(if plugin { "lsp-plugin" } else { "lsp-plain" });
        std::fs::write(
            dir.join("main.tex"),
            "% \\documentclass: the main file\n\\pdfoutput=1 \\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x Main.\n\\input{ch}\n\\end\n",
        )
        .expect("the main file");
        std::fs::write(dir.join("ch.tex"), "Chapter text.\n").expect("the chapter");
        let mut c = Command::new("timeout");
        c.arg("600")
            .arg(bin())
            .args(["watch", "--lsp", "--ini", "--etex"])
            .current_dir(&dir);
        env(&mut c, &dir);
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("phitex watch --lsp runs");
        let mut to = child.stdin.take().expect("its input");
        let rx = lsp_messages(child.stdout.take().expect("its output"));
        let opts = if plugin {
            json!({"client": {"name": "phitex.nvim", "version": "0"}})
        } else {
            json!({})
        };
        lsp_send(
            &mut to,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "processId": null, "rootUri": uri(&dir), "capabilities": {},
                "initializationOptions": opts,
            }}),
        );
        lsp_send(
            &mut to,
            &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        );
        let open = |to: &mut std::process::ChildStdin, name: &str, t: &str| {
            lsp_send(
                to,
                &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
                    "textDocument": {"uri": uri(&dir.join(name)), "languageId": "latex", "version": 0, "text": t}}}),
            );
        };
        open(
            &mut to,
            "main.tex",
            &std::fs::read_to_string(dir.join("main.tex")).unwrap(),
        );
        let typed = "Chapter text, a mistake typed at its end: \\foo\n";
        open(&mut to, "ch.tex", typed);
        let col = typed.find("\\foo").expect("the mistake");
        let mut seen = Vec::new();
        let mut shown = None;
        let mut placed = None;
        let end = Instant::now() + Duration::from_secs(300);
        while placed.is_none() || (!plugin && shown.is_none()) {
            let m = rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| panic!("{e}: {seen:#?}"));
            seen.push(m.clone());
            if m.get("id").is_some() && m.get("method").is_some() {
                lsp_send(
                    &mut to,
                    &json!({"jsonrpc": "2.0", "id": m["id"], "result": null}),
                );
            }
            if m["method"] == "window/showMessage"
                && m["params"]["message"]
                    .as_str()
                    .is_some_and(|t| t.contains("http://127.0.0.1:"))
            {
                shown = Some(m.clone());
            }
            if m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"]
                    .as_str()
                    .is_some_and(|u| u.ends_with("/ch.tex"))
                && let Some(d) = m["params"]["diagnostics"]
                    .as_array()
                    .and_then(|a| a.first())
            {
                placed = Some(d.clone());
            }
        }
        let d = placed.expect("the chapter's problem");
        assert_eq!(d["range"]["start"]["line"], 0, "{d}");
        assert_eq!(d["range"]["start"]["character"], col, "{d}");
        if plugin {
            // (a little longer: none comes)
            std::thread::sleep(Duration::from_secs(2));
            while let Ok(m) = rx.try_recv() {
                seen.push(m);
            }
            assert!(
                !seen.iter().any(|m| m["method"] == "window/showMessage"
                    && m["params"]["message"]
                        .as_str()
                        .is_some_and(|t| t.contains("http://127.0.0.1:"))),
                "a plugin was shown the address: {seen:#?}"
            );
        } else {
            let t = shown.expect("the address shown");
            assert!(
                t["params"]["message"]
                    .as_str()
                    .is_some_and(|t| t.contains("phitex session list")),
                "{t}"
            );
        }
        lsp_send(
            &mut to,
            &json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": null}),
        );
        lsp_send(
            &mut to,
            &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
        );
        if wait(&mut child, 60).is_none() {
            let _ = child.kill();
        }
    }
}

/// `phitex session wait` after a file is put back in place (`cp
/// orig.tex main.tex`) to a text an earlier build read: it waits for the
/// build that reads it, never answers with that earlier build; `status`
/// then says the new build. Twenty times over, two texts in turn.
#[test]
fn session_wait_after_a_copy_in_place() {
    let dir = scratch("wait-cp");
    let alpha = TINY.replace("Hi", "Alpha");
    let beta = TINY.replace("Hi", "Beta");
    std::fs::write(dir.join("a.tex"), &alpha).expect("text a");
    std::fs::write(dir.join("b.tex"), &beta).expect("text b");
    std::fs::write(dir.join("doc.tex"), &alpha).expect("the document");
    let session = |args: &[&str]| {
        let mut all = vec!["session"];
        all.extend_from_slice(args);
        run(&dir, &all)
    };
    let out = session(&[
        "start",
        "doc.tex",
        "--timeout",
        "10m",
        "--",
        "--no-view",
        "--ini",
        "--etex",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let json = |args: &[&str]| -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = session(&all);
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {} {}", text(&out.stdout), text(&out.stderr)))
    };
    let mut last = json(&["wait", "--timeout", "120s"])["build"]
        .as_u64()
        .expect("a build");
    for k in 0..20 {
        let from = if k % 2 == 0 { "b.tex" } else { "a.tex" };
        // (as cp: the file truncated and written in place)
        std::fs::copy(dir.join(from), dir.join("doc.tex")).expect("cp");
        let w = json(&["wait", "--disk", "--timeout", "120s"]);
        let s = json(&["status"]);
        let n = w["build"].as_u64().unwrap_or_else(|| panic!("{k}: {w}"));
        assert!(
            n > last,
            "{k}: wait answered with build {n}, not one after {last}: {w}"
        );
        assert_eq!(s["build"], w["build"], "{k}: status {s} after wait {w}");
        last = n;
    }
    let _ = session(&["stop"]);
}

/// A beamer deck of four pages: frame 1, frame 2's two overlays, frame 3.
const DECK: &str = "\\documentclass{beamer}
\\begin{document}
\\begin{frame}{One}
First frame words.
\\end{frame}
\\begin{frame}{Two}
\\begin{itemize}
\\item<1-> Overlay item alpha
\\item<2-> Overlay item beta
\\end{itemize}
\\end{frame}
\\begin{frame}{Three}
Third frame words.
\\end{frame}
\\end{document}
";

/// `phitex session` on a beamer deck: `wait` and `status` end with the
/// pages changed and the command that draws them; `png --changed` draws
/// exactly the edited frame's pages (one PNG, or one sheet of its
/// overlays); `--page` ranges; `--at` a line on two overlays draws both
/// (`--overlay last` the last); a problem names the command undefined,
/// keeps TeX's context, and ends with the command that shows it.
#[test]
fn session_pages_of_a_beamer_deck() {
    let dir = scratch("deck");
    std::fs::write(dir.join("deck.tex"), DECK).expect("the deck");
    // (the deck's every build from this run's own: a graph a run before
    // saved restores pages with no glyph origins, and the overlays are
    // found by theirs)
    let session = |args: &[&str]| {
        let mut c = Command::new("timeout");
        c.arg("600")
            .arg(bin())
            .arg("session")
            .args(args)
            .current_dir(&dir);
        env(&mut c, &dir);
        let out = c
            .env("PHITEX_GRAPH", "0")
            .stdin(Stdio::null())
            .output()
            .expect("phitex runs");
        (out.status.code(), text(&out.stdout), text(&out.stderr))
    };
    let (code, _, err) = session(&["start", "deck.tex", "--timeout", "20m"]);
    assert_eq!(code, Some(0), "{err}");
    let edit = |from: &str, to: &str| {
        let t = std::fs::read_to_string(dir.join("deck.tex")).expect("the deck");
        assert!(t.contains(from), "{from}");
        std::fs::write(dir.join("deck.tex"), t.replace(from, to)).expect("an edit");
    };
    // (frame 3 edited: page 4 alone)
    edit("Third frame words.", "Third frame, other words.");
    let (_, out, err) = session(&["wait", "--timeout", "300s"]);
    assert!(
        out.contains("ok · page 4 changed · see it: phitex session png --changed"),
        "{out}{err}"
    );
    let (_, status, _) = session(&["status"]);
    assert!(
        status
            .lines()
            .last()
            .is_some_and(|l| l.contains("page 4 changed")),
        "{status}"
    );
    let (code, out, err) = session(&["png", "--changed"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.starts_with("page 4 (build"), "{out}");
    assert!(out.contains("page-4-build"), "{out}");
    // (no option: the same)
    let (_, out2, _) = session(&["png"]);
    assert!(out2.starts_with("page 4 (build"), "{out2}");
    // (frame 2 edited: both its overlays, one sheet)
    edit("\\begin{frame}{Two}", "\\begin{frame}{Two, renamed}");
    let (_, out, err) = session(&["wait", "--timeout", "300s"]);
    assert!(
        out.contains("pages 2–3 changed · see them: phitex session png --changed"),
        "{out}{err}"
    );
    let (code, out, err) = session(&["png", "--changed"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.starts_with("pages 2–3 on one sheet"), "{out}");
    let png = out
        .lines()
        .nth(1)
        .and_then(|l| l.split(" (").next())
        .expect("its path");
    assert!(Path::new(png).is_file(), "{out}");
    // (--page: ranges, and a page past the end refused)
    let (_, out, _) = session(&["png", "--page", "1-3"]);
    assert!(out.starts_with("pages 1–3 on one sheet"), "{out}");
    let (_, out, _) = session(&["png", "--page", "4"]);
    assert!(out.starts_with("page 4 (build"), "{out}");
    let (code, _, err) = session(&["png", "--page", "9"]);
    assert_ne!(code, Some(0));
    assert!(err.contains("the document has 4 pages"), "{err}");
    // (a line on both overlays: both drawn, or the last)
    let line = DECK
        .lines()
        .position(|l| l.contains("alpha"))
        .expect("the line")
        + 1;
    let at = format!("deck.tex:{line}");
    let (code, out, err) = session(&["png", "--at", &at]);
    assert_eq!(code, Some(0), "{err}");
    assert!(
        out.contains(&format!("{at}: on pages 2–3 (overlays")),
        "{out}"
    );
    let (code, out, err) = session(&["png", "--at", &at, "--overlay", "last"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains(&format!("{at}: page 3,")), "{out}");
    // (a problem in a frame: where to look, the frame's line)
    edit(
        "Third frame, other words.",
        "Third frame \\faSyncAlt{} words.",
    );
    let (_, out, _) = session(&["wait", "--timeout", "300s"]);
    assert!(
        out.contains("1 error · see it: phitex session png --at deck.tex:"),
        "{out}"
    );
    edit(
        "Third frame \\faSyncAlt{} words.",
        "Third frame, other words.",
    );
    // (one read from the file: the command named, TeX's context kept)
    edit(
        "\\documentclass{beamer}\n",
        "\\documentclass{beamer}\n\\faSyncAlt\n",
    );
    let (_, out, _) = session(&["wait", "--timeout", "300s"]);
    assert!(
        out.contains("see it: phitex session png --at deck.tex:2"),
        "{out}"
    );
    let (_, problems, _) = session(&["problems"]);
    assert!(
        problems.contains("Undefined control sequence \\faSyncAlt"),
        "{problems}"
    );
    assert!(problems.contains("| l.2 \\faSyncAlt"), "{problems}");
    assert!(
        problems.contains("see it: phitex session png --at deck.tex:2"),
        "{problems}"
    );
    let _ = session(&["stop"]);
}
