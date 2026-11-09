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
