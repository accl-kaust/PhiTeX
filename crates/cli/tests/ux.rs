//! The command line as people and editors meet it: file names with
//! spaces, the terminal and signals, standard output against standard
//! error, `--json`, `config`, exit statuses, completions, the language
//! server's lifecycle and diagnostics.

#![allow(clippy::too_many_lines)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_phitex"))
}

/// A fresh directory for a test's run.
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("ux")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d
}

/// phitex in `dir`, its settings, state and cache in `dir`'s own
/// folders (none of the user's), stopped after ten minutes.
fn phitex(dir: &Path) -> Command {
    let mut c = Command::new("timeout");
    c.arg("600").arg(bin());
    let home = dir.join(".home");
    c.current_dir(dir)
        .env(
            "PHITEX_CACHE_DIR",
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
        )
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .env_remove("PHITEX_COMPAT")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("CI");
    c
}

fn run(dir: &Path, args: &[&str]) -> Output {
    phitex(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("phitex runs")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const ARTICLE: &str = "\\documentclass{article}\n\\begin{document}\n\\section{One}\\label{one}\nHello, see~\\ref{one}.\n\\end{document}\n";

/// A `file:` URI of `p`, its bytes percent-encoded where a URI needs it.
fn uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(char::from(b));
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// An LSP message to the server, framed.
fn lsp_send(w: &mut impl std::io::Write, v: &Value) {
    let b = v.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{b}", b.len()).expect("the server reads");
    w.flush().expect("the server reads");
}

/// The server's messages, read on a thread of their own; anything on
/// standard output but a framed message fails.
fn lsp_messages(out: std::process::ChildStdout) -> Receiver<Value> {
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
            if tx.send(serde_json::from_slice(&b).expect("JSON")).is_err() {
                return;
            }
        }
    });
    rx
}

/// The server's next message `what` says is the one; the server's own
/// requests answered `null` on the way.
fn lsp_until(
    rx: &Receiver<Value>,
    to: &mut impl std::io::Write,
    seen: &mut Vec<Value>,
    what: impl Fn(&Value) -> bool,
) -> Value {
    let end = Instant::now() + Duration::from_secs(300);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(m) => {
                seen.push(m.clone());
                if m.get("method").is_some() && m.get("id").is_some() {
                    let result = if m["method"] == "workspace/configuration" {
                        json!([{}])
                    } else {
                        Value::Null
                    };
                    lsp_send(
                        to,
                        &json!({"jsonrpc": "2.0", "id": m["id"], "result": result}),
                    );
                }
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

/// A language server session on `root`: started, initialized.
struct Lsp {
    child: std::process::Child,
    to: std::process::ChildStdin,
    rx: Receiver<Value>,
    seen: Vec<Value>,
}

impl Lsp {
    fn start(root: &Path, opts: &Value) -> Lsp {
        let mut child = phitex(root)
            .args(["watch", "--lsp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("phitex watch --lsp runs");
        let to = child.stdin.take().expect("its input");
        let rx = lsp_messages(child.stdout.take().expect("its output"));
        let mut l = Lsp {
            child,
            to,
            rx,
            seen: Vec::new(),
        };
        l.send(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "processId": null,
                "rootUri": uri(root),
                "capabilities": {"workspace": {"configuration": true}},
                "initializationOptions": opts,
            }}),
        );
        let init = l.until(|m| m["id"] == 1);
        assert!(init.get("error").is_none(), "{init}");
        l.send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        l
    }

    fn send(&mut self, v: &Value) {
        lsp_send(&mut self.to, v);
    }

    fn until(&mut self, what: impl Fn(&Value) -> bool) -> Value {
        lsp_until(&self.rx, &mut self.to, &mut self.seen, what)
    }

    /// `shutdown`, `exit`: the exit status.
    fn end(mut self) -> std::process::ExitStatus {
        self.send(&json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": null}));
        self.until(|m| m["id"] == 99);
        self.send(&json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
        wait(&mut self.child, 60)
    }
}

fn wait(child: &mut std::process::Child, secs: u64) -> std::process::ExitStatus {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(s) = child.try_wait().expect("the child") {
            return s;
        }
        assert!(Instant::now() < end, "it did not end in {secs} s");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A main file whose name has spaces and letters past ASCII, in a
/// folder whose name has them too, builds as pdflatex builds it
/// (`\jobname` is `"my paper é"`, quoted as web2c quotes it, so LaTeX's
/// `\jobname.aux` is one file): no error, the PDF and the `.aux` under
/// the whole name, and no `my.tex` from a name cut at its first space.
/// The same by `-o` into such a folder, and by the language server.
#[test]
fn file_names_with_spaces() {
    let top = scratch("spaces");
    let dir = top.join("a folder ü");
    std::fs::create_dir_all(&dir).expect("the folder");
    std::fs::write(dir.join("my paper é.tex"), ARTICLE).expect("the document");
    let out = run(&top, &["build", "a folder ü/my paper é.tex"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    for f in ["my paper é.pdf", "my paper é.aux", "my paper é.log"] {
        assert!(dir.join(f).is_file(), "no {f}: {}", text(&out.stderr));
    }
    assert!(!dir.join("my.tex").exists(), "a name cut at its space");
    let aux = std::fs::read_to_string(dir.join("my paper é.aux")).expect("the aux");
    assert!(aux.contains("\\newlabel{one}"), "{aux}");
    let log = std::fs::read_to_string(dir.join("my paper é.log")).expect("the log");
    assert!(
        !log.contains("undefined"),
        "the reference did not settle:\n{log}"
    );

    let out = run(
        &top,
        &["build", "-o", "out dir é", "a folder ü/my paper é.tex"],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(top.join("out dir é/my paper é.pdf").is_file());
    assert!(!top.join("out dir é/my.tex").exists());

    let mut l = Lsp::start(&dir, &json!({"main": "my paper é.tex"}));
    let main = uri(&dir.join("my paper é.tex"));
    l.send(
        &json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": main, "languageId": "latex", "version": 1, "text": ARTICLE},
        }}),
    );
    let d = l.until(|m| {
        m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == main.as_str()
    });
    assert_eq!(d["params"]["diagnostics"], json!([]), "{d}");
    assert!(l.end().success());
}

/// A one-page job without a format (`--ini --etex`).
const TINY: &str = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x Hi\\end\n";

/// The pid of the phitex whose command line holds `mark`.
fn pid_of(mark: &str) -> Option<u32> {
    let end = Instant::now() + Duration::from_secs(60);
    while Instant::now() < end {
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            let cmd = text(&cmd).replace('\0', " ");
            if cmd.contains("phitex")
                && cmd.contains(" watch ")
                && cmd.contains(mark)
                && !cmd.contains("script")
                && !cmd.contains("stty")
                && !cmd.starts_with("timeout")
            {
                return Some(pid);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// A process's state letter (`/proc/PID/stat`: `T` stopped).
fn state(pid: u32) -> char {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    s.rsplit(')')
        .next()
        .and_then(|r| r.trim().chars().next())
        .unwrap_or('?')
}

/// The modes of the terminal `pid` reads (`stty -a` of its standard input).
fn modes(pid: u32) -> String {
    let tty = std::fs::read_link(format!("/proc/{pid}/fd/0")).expect("its terminal");
    let out = Command::new("stty")
        .arg("-F")
        .arg(&tty)
        .arg("-a")
        .output()
        .expect("stty");
    text(&out.stdout)
}

fn has(modes: &str, flag: &str) -> bool {
    modes.split_whitespace().any(|w| w == flag)
}

/// A watch on a terminal (`script` gives it one): Ctrl-Z suspends it with
/// the terminal put back (echo, line editing), SIGCONT gives it the keys
/// again, and SIGTERM (as SIGHUP, SIGINT, SIGQUIT) ends it with the
/// terminal put back, as the shell after it finds it.
#[test]
fn watch_puts_the_terminal_back() {
    let dir = scratch("terminal");
    std::fs::write(dir.join("term-mark.tex"), TINY).expect("the document");
    let line = format!(
        "{} watch --ini --etex term-mark.tex; stty -a > modes-after.txt",
        bin().display()
    );
    let mut script = Command::new("script")
        .args(["-qefc", &line, "/dev/null"])
        .current_dir(&dir)
        .env(
            "PHITEX_CACHE_DIR",
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
        )
        .env("XDG_CONFIG_HOME", dir.join(".home/config"))
        .env("XDG_STATE_HOME", dir.join(".home/state"))
        .env("TERM", "xterm")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("script runs");
    let mut keys = script.stdin.take().expect("its input");
    let pid = pid_of("term-mark.tex").expect("the watch");
    // (its keys one at a time: the watch's mode)
    let end = Instant::now() + Duration::from_secs(60);
    while has(&modes(pid), "icanon") {
        assert!(Instant::now() < end, "the watch never took the keys");
        std::thread::sleep(Duration::from_millis(100));
    }
    keys.write_all(b"\x1a").expect("Ctrl-Z");
    keys.flush().expect("Ctrl-Z");
    let end = Instant::now() + Duration::from_secs(30);
    while state(pid) != 'T' {
        assert!(Instant::now() < end, "Ctrl-Z did not suspend the watch");
        std::thread::sleep(Duration::from_millis(50));
    }
    let m = modes(pid);
    assert!(
        has(&m, "icanon") && has(&m, "echo"),
        "suspended in the watch's mode:\n{m}"
    );
    Command::new("kill")
        .args(["-CONT", &pid.to_string()])
        .status()
        .expect("kill");
    let end = Instant::now() + Duration::from_secs(30);
    while has(&modes(pid), "icanon") {
        assert!(
            Instant::now() < end,
            "fg did not give the watch its keys again"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill");
    wait(&mut script, 60);
    let after = std::fs::read_to_string(dir.join("modes-after.txt")).expect("stty after the watch");
    assert!(
        has(&after, "icanon") && has(&after, "echo") && has(&after, "isig"),
        "the terminal after a SIGTERM:\n{after}"
    );
}

/// A watch whose standard input is at its end from the start (`phitex
/// watch &`, nohup, a service) keeps watching: an edit is built after
/// it, and it ends at a signal, not at the end of its input.
#[test]
fn watch_outlives_the_end_of_its_input() {
    let dir = scratch("eof");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let mut child = phitex(&dir)
        .args(["watch", "--json", "--no-view", "--ini", "--etex", "doc.tex"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("phitex watch runs");
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let out = child.stdout.take().expect("its output");
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str(&l) {
                let _ = tx.send(v);
            }
        }
    });
    let build = |n: u64| {
        let end = Instant::now() + Duration::from_secs(300);
        loop {
            let v = rx
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| panic!("no build {n}: {e}"));
            if v["event"] == "build" && v["build"] == n {
                return v;
            }
        }
    };
    build(1);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().expect("the watch").is_none(),
        "the watch quit at its input's end"
    );
    std::fs::write(dir.join("doc.tex"), TINY.replace("Hi", "Hello")).expect("an edit");
    build(2);
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    wait(&mut child, 60);
}

/// `phitex config` is a command's answer: its listing on standard
/// output, nothing on standard error; `--json` one JSON object, the
/// value that holds and where it comes from; `news --json` one JSON line.
#[test]
fn config_and_news_answer_on_stdout() {
    let dir = scratch("config-stdout");
    let out = run(&dir, &["config"]);
    assert!(out.status.success());
    let listing = text(&out.stdout);
    assert!(
        listing.contains("synctex") && listing.contains("update-check"),
        "{listing}"
    );
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    let out = run(&dir, &["config", "synctex"]);
    assert!(text(&out.stdout).contains("synctex"));
    let out = run(&dir, &["config", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    let synctex = v["settings"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["key"] == "synctex"))
        .expect("synctex listed")
        .clone();
    assert_eq!(synctex["from"], "default", "{synctex}");
    assert!(run(&dir, &["config", "synctex", "on"]).status.success());
    let out = run(&dir, &["config", "--json", "synctex"]);
    let v: Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    assert_eq!(v["settings"][0]["value"], "true", "{v}");
    assert_eq!(v["settings"][0]["from"], "yours", "{v}");
    let out = run(&dir, &["news", "--json"]);
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout)
        .expect("UTF-8")
        .lines()
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let v: Value = serde_json::from_str(lines[0]).expect("JSON");
    assert_eq!(v["event"], "news", "{v}");
    assert!(v["version"].is_string(), "{v}");
}

/// What phitex prints on a terminal (`script` gives it one) for `line`,
/// run by sh in `dir` with the test's settings folders.
fn on_a_terminal(dir: &Path, line: &str) -> String {
    let log = dir.join("typescript.txt");
    let status = Command::new("script")
        .args(["-qefc", line])
        .arg(&log)
        .current_dir(dir)
        .env(
            "PHITEX_CACHE_DIR",
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
        )
        .env("XDG_CONFIG_HOME", dir.join(".home/config"))
        .env("XDG_STATE_HOME", dir.join(".home/state"))
        .env("TERM", "xterm")
        .env_remove("CI")
        .env_remove("PHITEX_NO_UPDATE_CHECK")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("script runs");
    assert!(status.success(), "{line}");
    text(&std::fs::read(&log).expect("the typescript"))
}

/// The update check's question shows once per version on a terminal,
/// not after every build.
#[test]
fn update_note_once_per_version() {
    let dir = scratch("update-note");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let b = bin().display().to_string();
    let line = format!(
        "{b} build --ini --etex doc.tex; {b} build --ini --etex doc.tex; {b} build --ini --etex doc.tex"
    );
    let shown = on_a_terminal(&dir, &line);
    assert_eq!(
        shown.matches("phitex config update-check on").count(),
        1,
        "{shown}"
    );
    std::fs::write(dir.join(".home/state/phitex/update-asked"), "0.0.1")
        .expect("an older version asked");
    let shown = on_a_terminal(&dir, &format!("{b} build --ini --etex doc.tex"));
    assert_eq!(
        shown.matches("phitex config update-check on").count(),
        1,
        "a new version asks again:\n{shown}"
    );
}

/// What a reader sees says "clean build", never "cold", and shows no
/// engine counters: not `build -v`'s lines, not a watch's rows and quit
/// line, not `--help-all`; `-vv` adds the counters.
#[test]
fn reader_text_says_clean_build() {
    let dir = scratch("wording");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let out = run(&dir, &["build", "-v", "--ini", "--etex", "doc.tex"]);
    let err = text(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("clean build"), "{err}");
    for jargon in [
        "cold",
        "nodes",
        "cutoffs",
        "guesses refuted",
        "engine tables",
    ] {
        assert!(!err.contains(jargon), "-v says {jargon}:\n{err}");
    }
    let out = run(&dir, &["build", "-vv", "--ini", "--etex", "doc.tex"]);
    assert!(text(&out.stderr).contains("nodes"), "-vv has the counters");
    let mut w = phitex(&dir)
        .args(["watch", "--no-view", "--ini", "--etex", "doc.tex"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("phitex watch runs");
    let mut keys = w.stdin.take().expect("its input");
    let err = w.stderr.take().expect("its errors");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    let mut lines = Vec::new();
    let end = Instant::now() + Duration::from_secs(300);
    while !lines.iter().any(|l: &String| l.starts_with("ok:")) {
        lines.push(
            rx.recv_timeout(end.saturating_duration_since(Instant::now()))
                .expect("a build"),
        );
    }
    keys.write_all(b"q\n").expect("q");
    wait(&mut w, 60);
    lines.extend(rx.try_iter());
    let shown = lines.join("\n");
    assert!(shown.contains("clean build"), "{shown}");
    assert!(
        !shown.contains("cold") && !shown.contains("nodes"),
        "{shown}"
    );
    let help = text(&run(&dir, &["--help-all"]).stdout);
    assert!(!help.contains("cold") && !help.contains("graph"), "{help}");
}

/// An option where it means nothing is an error naming it, not ignored:
/// a watch's option on `build`, a build's on `news`; each command's help
/// lists only its own options.
#[test]
fn options_only_where_they_mean_something() {
    let dir = scratch("scoped");
    for (args, opt) in [
        (&["build", "--lsp", "x.tex"][..], "--lsp"),
        (&["build", "--open", "x.tex"], "--open"),
        (&["news", "--shelf"], "--shelf"),
        (&["config", "--engine", "tex"], "--engine"),
        (&["clean", "--all", "x.tex"], "--all"),
    ] {
        let out = run(&dir, args);
        assert!(!out.status.success(), "{args:?} accepted");
        assert!(
            text(&out.stderr).contains(opt),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    let news = text(&run(&dir, &["news", "--help"]).stdout);
    assert!(
        !news.contains("--engine") && !news.contains("--open"),
        "{news}"
    );
    let build = text(&run(&dir, &["build", "--help"]).stdout);
    assert!(
        build.contains("--engine") && !build.contains("--open"),
        "{build}"
    );
    let watch = text(&run(&dir, &["watch", "--help"]).stdout);
    assert!(
        watch.contains("--engine") && watch.contains("--open"),
        "{watch}"
    );
}

/// `phitex config` checks a value before it keeps it, the error naming
/// the key and what it takes; `--unset` takes a key out; a project's
/// value of a key that is yours alone is listed as ignored.
#[test]
fn config_checks_values() {
    let dir = scratch("config-checks");
    for (k, v, says) in [
        ("engine", "banana", "pdflatex"),
        ("update-channel", "beta", "stable, nightly"),
        ("update-check", "maybe", "on or off"),
        ("threads", "lots", "a number"),
        ("dark-pdf", "dim", "off, on, theme"),
        ("cache-limit", "huge", "a size"),
    ] {
        let out = run(&dir, &["config", k, v]);
        let err = text(&out.stderr);
        assert!(!out.status.success(), "{k} = {v} kept");
        assert!(
            err.contains(k) && err.contains(says) && err.contains(v),
            "{k}: {err}"
        );
    }
    let user = dir.join(".home/config/phitex/phitex.toml");
    assert!(!user.exists() || !std::fs::read_to_string(&user).unwrap().contains("banana"));
    assert!(run(&dir, &["config", "engine", "xelatex"]).status.success());
    assert!(run(&dir, &["config", "synctex", "on"]).status.success());
    assert!(run(&dir, &["config", "--unset", "engine"]).status.success());
    let kept = std::fs::read_to_string(&user).expect("your settings");
    assert!(
        !kept.contains("engine") && kept.contains("synctex"),
        "{kept}"
    );
    std::fs::write(user, "engine = \"banana\"\n").expect("a value set by hand");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let err = text(&run(&dir, &["build", "doc.tex"]).stderr);
    assert!(
        err.contains("banana") && err.contains("phitex.toml"),
        "{err}"
    );
    std::fs::write(dir.join(".home/config/phitex/phitex.toml"), "").expect("empty");
    std::fs::write(
        dir.join("phitex.toml"),
        "shell-escape = true\nsynctex = true\n",
    )
    .expect("a project");
    let v: Value = serde_json::from_slice(&run(&dir, &["config", "--json"]).stdout).expect("JSON");
    let row = |k: &str| {
        v["settings"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["key"] == k))
            .expect("listed")
            .clone()
    };
    assert_eq!(row("shell-escape")["project_ignored"], true);
    assert_eq!(row("shell-escape")["from"], "default");
    assert_eq!(row("synctex")["from"], "project");
    assert!(text(&run(&dir, &["config"]).stdout).contains("ignored"));
}

/// The exit statuses: 0 done, 1 the document's errors, 2 PhiTeX could
/// not start (a usage error, a missing file, a wrong setting), as
/// `--help-all` says; `phitex completions SHELL` writes a script,
/// `phitex help` and `phitex help build` the help.
#[test]
fn exit_statuses_completions_help() {
    let dir = scratch("exits");
    std::fs::write(dir.join("good.tex"), TINY).expect("the document");
    std::fs::write(dir.join("bad.tex"), "\\undefinedcs\n\\end\n").expect("the document");
    let code = |args: &[&str]| run(&dir, args).status.code();
    assert_eq!(code(&["build", "--ini", "--etex", "good.tex"]), Some(0));
    assert_eq!(code(&["build", "--ini", "--etex", "bad.tex"]), Some(1));
    assert_eq!(code(&["build", "nothere.tex"]), Some(2));
    assert_eq!(code(&["build", "--frob", "good.tex"]), Some(2));
    assert_eq!(code(&["bulid", "good.tex"]), Some(2));
    assert_eq!(code(&["config", "engine", "banana"]), Some(2));
    assert_eq!(code(&["--compat=pdflatex", "-frob", "x.tex"]), Some(1));
    let help = text(&run(&dir, &["--help-all"]).stdout);
    assert!(
        help.contains("2 PhiTeX could not start") && help.contains("3 PhiTeX failed"),
        "{help}"
    );
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = run(&dir, &["completions", shell]);
        assert!(out.status.success(), "{shell}: {}", text(&out.stderr));
        let script = text(&out.stdout);
        assert!(
            script.contains("phitex") && script.contains("watch"),
            "{shell}"
        );
    }
    assert_eq!(code(&["completions", "tcsh"]), Some(2));
    let out = run(&dir, &["help"]);
    assert!(out.status.success() && text(&out.stdout).contains("Commands:"));
    let out = run(&dir, &["help", "build"]);
    assert!(out.status.success() && text(&out.stdout).contains("--engine"));
}

/// NO_COLOR wins over CLICOLOR_FORCE (no-color.org), in the help too;
/// CLICOLOR_FORCE alone colours a pipe; --color=never wins over both.
#[test]
fn no_color_wins() {
    let dir = scratch("colours");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    let esc = |o: &Output| o.stderr.contains(&0x1b) || o.stdout.contains(&0x1b);
    let with = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = phitex(&dir);
        for (k, v) in env {
            c.env(k, v);
        }
        c.args(args)
            .stdin(Stdio::null())
            .output()
            .expect("phitex runs")
    };
    let build = ["build", "--ini", "--etex", "doc.tex"];
    assert!(esc(&with(&[("CLICOLOR_FORCE", "1")], &build)));
    assert!(!esc(&with(
        &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")],
        &build
    )));
    assert!(!esc(&with(
        &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")],
        &["--help"]
    )));
    let mut never = vec!["--color=never"];
    never.extend(build);
    assert!(!esc(&with(&[("CLICOLOR_FORCE", "1")], &never)));
}

/// `phitex clean` needs no TeX Live: it reads no TeX file.
#[test]
fn clean_without_texlive() {
    let dir = scratch("clean-no-tl");
    std::fs::write(dir.join("doc.tex"), TINY).expect("the document");
    std::fs::write(dir.join("doc.log"), "a log").expect("an output");
    let out = phitex(&dir)
        .args(["clean", "doc.tex"])
        .env("TEXMFCNF", "/nonexistent")
        .stdin(Stdio::null())
        .output()
        .expect("phitex runs");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!dir.join("doc.log").exists());
    let out = phitex(&dir)
        .args(["build", "doc.tex"])
        .env("TEXMFCNF", "/nonexistent")
        .stdin(Stdio::null())
        .output()
        .expect("phitex runs");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
}
