//! `phitex watch --lsp`: the watch as a language server, for any editor
//! with an LSP client (Neovim, Emacs's eglot and lsp-mode, Helix, Zed, VS
//! Code). The same session as `phitex watch`, its standard input and
//! output speaking the Language Server Protocol (JSON-RPC 2.0, each
//! message after a `Content-Length` header) instead of keys and a status
//! line; nothing else is written to standard output. The server is
//! `phitex_live::lsp::Server` (docs/spec-editor-protocol.md), the same the
//! wasm core runs; this is its transport and its host on this machine:
//!
//! - **Start.** `initialize` comes first ([`Lsp::begin`]): its root
//!   (`rootUri`, else the first of `workspaceFolders`) is where the watch
//!   runs; the main file is the watch's argument, else `phitex.toml`'s,
//!   else `initializationOptions.main`, else the one `.tex` file at the
//!   root with a `\documentclass` (`main.tex` among several), else the
//!   first `.tex` document opened (it, if it has a `\documentclass`, else
//!   the one `.tex` file with one beside it): an editor's root may be the
//!   repository's, above the TeX project's folder. Answered once the
//!   watch runs ([`Lsp::accept`]), or with why it cannot.
//! - **Messages.** Read on a thread of their own ([`Lsp::serve`]) and
//!   handed to the server; what it asks of the watch ([`Ask`]) is queued
//!   and the watch woken ([`Lsp::on_ask`], [`Lsp::take_asks`]).
//! - **The host's commands**: `phitex.openViewer` (the browser opened on
//!   the viewer).
//! - **End.** `exit`, or the end of standard input, ends the watch as `q`
//!   does.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use phitex_live::lsp::{LspHost, Server, path_of};
use serde_json::{Value, json};

use crate::view::{Ask, Built};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The next message on `r` (none: its end).
fn read(r: &mut impl BufRead) -> Option<Value> {
    loop {
        let mut len = None;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).ok()? == 0 {
                return None;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.trim().eq_ignore_ascii_case("content-length")
            {
                len = v.trim().parse::<usize>().ok();
            }
        }
        // (a header without its length: nothing to read, the next one)
        let Some(n) = len else { continue };
        let mut b = vec![0; n];
        r.read_exact(&mut b).ok()?;
        if let Ok(v) = serde_json::from_slice(&b) {
            return Some(v);
        }
    }
}

/// `v` written to standard output after its `Content-Length` header.
fn write(out: &Mutex<std::io::Stdout>, v: &Value) {
    let body = v.to_string();
    let mut o = lock(out);
    let _ = write!(o, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = o.flush();
}

/// The watch, as the server's host: its asks queued and the watch woken.
struct Watch {
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The watch's directory (where its file names start).
    root: Mutex<PathBuf>,
    viewer: Mutex<Option<String>>,
}

impl LspHost for Watch {
    fn ask(&self, a: Ask) {
        lock(&self.asks).push(a);
        if let Some(w) = lock(&self.wake).as_ref() {
            w();
        }
    }

    fn command(&self, name: &str, _arg: &Value) -> Option<Result<Value, String>> {
        match name {
            "phitex.openViewer" => Some(match lock(&self.viewer).clone() {
                Some(url) if crate::view::open_browser(&url) => Ok(json!(url)),
                Some(url) => Err(format!("could not start a browser: open {url}")),
                None => Err("no viewer (--no-view)".to_owned()),
            }),
            _ => None,
        }
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        path.canonicalize().ok()
    }

    fn exists(&self, name: &str) -> bool {
        lock(&self.root).join(name).exists()
    }

    fn engine(&self) -> &'static str {
        "native"
    }
}

/// The watch's language server.
pub struct Lsp {
    server: Arc<Server>,
    watch: Arc<Watch>,
    /// Standard input, as `initialize` left it (what came after it may
    /// be read already), until served.
    input: Mutex<Option<BufReader<std::io::Stdin>>>,
    /// Whether `initialize` left the main file unnamed: the first `.tex`
    /// document opened names it ([`Lsp::main_from_open`]).
    wants_main: bool,
    /// The messages read before the watch began, served first.
    pending: Mutex<Vec<Value>>,
}

/// The main file to build in `root` when nothing names one: the one
/// `.tex` file there with a `\documentclass`, `main.tex` among several.
fn find_main(root: &Path) -> Option<String> {
    let mut found: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.ends_with(".tex")
                && std::fs::read(root.join(n))
                    .is_ok_and(|b| String::from_utf8_lossy(&b).contains("\\documentclass"))
        })
        .collect();
    found.sort();
    match found.len() {
        1 => found.pop(),
        _ => found.into_iter().find(|n| n == "main.tex"),
    }
}

impl Lsp {
    /// Read standard input until `initialize` (answering anything else
    /// before it as the protocol says), enter its root, and name the main
    /// file in `file` if nothing has (`phitex.toml` may yet).
    pub fn begin(file: &mut Option<String>) -> Lsp {
        let out = Arc::new(Mutex::new(std::io::stdout()));
        let mut input = BufReader::new(std::io::stdin());
        let watch = Arc::new(Watch {
            asks: Mutex::default(),
            wake: Mutex::new(None),
            root: Mutex::new(PathBuf::new()),
            viewer: Mutex::new(None),
        });
        let o = out.clone();
        let server = Arc::new(Server::new(
            Box::new(move |v: &Value| write(&o, v)),
            watch.clone(),
        ));
        loop {
            let Some(m) = read(&mut input) else {
                std::process::exit(crate::EXIT_FAILED);
            };
            let method = m["method"].as_str().unwrap_or("");
            if method == "exit" {
                std::process::exit(crate::EXIT_FAILED);
            }
            if method != "initialize" {
                if let Some(id) = m.get("id") {
                    write(
                        &out,
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32002, "message": "not initialized"}}),
                    );
                }
                continue;
            }
            server.initialize(&m);
            let p = &m["params"];
            // (a file named on the command line is the launching
            // directory's, whatever the root)
            if let Some(f) = file.as_mut()
                && Path::new(f.as_str()).is_relative()
                && let Ok(cwd) = std::env::current_dir()
            {
                *f = cwd.join(&*f).to_string_lossy().into_owned();
            }
            let root = p["rootUri"]
                .as_str()
                .or_else(|| p["workspaceFolders"][0]["uri"].as_str())
                .and_then(path_of)
                .or_else(|| p["rootPath"].as_str().map(PathBuf::from));
            if let Some(r) = &root
                && let Err(e) = std::env::set_current_dir(r)
            {
                server.refuse(&format!("can't enter {}: {e}", r.display()));
                std::process::exit(crate::EXIT_FAILED);
            }
            let here = std::env::current_dir().unwrap_or_default();
            let configured = crate::config::find(&here)
                .ok()
                .flatten()
                .is_some_and(|(_, c)| c.main.is_some());
            if file.is_none() && !configured {
                *file = p["initializationOptions"]["main"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| find_main(&here));
            }
            return Lsp {
                server,
                watch,
                input: Mutex::new(Some(input)),
                wants_main: file.is_none() && !configured,
                pending: Mutex::default(),
            };
        }
    }

    /// The watch cannot run: `initialize` answered with why, or, answered
    /// already, the editor shown it; the watch ends.
    pub fn refuse(&self, e: &str) -> ! {
        self.server.refuse(&format!("phitex watch: {e}"));
        std::process::exit(crate::EXIT_FAILED);
    }

    /// Whether nothing named the main file: [`Lsp::main_from_open`]
    /// does.
    pub fn wants_main(&self) -> bool {
        self.wants_main
    }

    /// The main file, named by the first `.tex` document the editor opens
    /// (an editor's root is its own choice, often the repository's, above
    /// the TeX project's folder): that document if it has a
    /// `\documentclass`, else the one `.tex` file with one in its
    /// directory ([`find_main`]); its path. `initialize` is answered
    /// first; what the editor sends meanwhile is served once the watch
    /// runs.
    pub fn main_from_open(&self) -> String {
        let s = &self.server;
        s.answer_init();
        s.log(
            3,
            "phitex watch: no main file here (no argument, phitex.toml or initializationOptions.main): the first .tex document opened names it",
        );
        let mut input = lock(&self.input)
            .take()
            .unwrap_or_else(|| BufReader::new(std::io::stdin()));
        let mut pending = Vec::new();
        let main = loop {
            let Some(m) = read(&mut input) else {
                std::process::exit(crate::EXIT_FAILED);
            };
            match m["method"].as_str().unwrap_or("") {
                "exit" => std::process::exit(crate::EXIT_FAILED),
                "shutdown" => {
                    s.handle(&m);
                    continue;
                }
                "textDocument/didOpen" => {}
                _ => {
                    pending.push(m);
                    continue;
                }
            }
            let d = &m["params"]["textDocument"];
            let path = d["uri"].as_str().and_then(path_of);
            pending.push(m.clone());
            let Some(path) = path.filter(|p| p.extension().is_some_and(|e| e == "tex")) else {
                continue;
            };
            if d["text"]
                .as_str()
                .is_some_and(|t| t.contains("\\documentclass"))
            {
                break path;
            }
            let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
            if let Some(n) = find_main(&dir) {
                break dir.join(n);
            }
            s.show_message(
                2,
                &format!(
                    "phitex watch: {} has no \\documentclass, nor one .tex file beside it: open the main file",
                    path.display()
                ),
            );
        };
        *lock(&self.input) = Some(input);
        *lock(&self.pending) = pending;
        main.to_string_lossy().into_owned()
    }

    /// `initialize` answered: the watch runs in `root` on `main`.
    pub fn accept(&self, root: &Path, main: &str) {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        root.clone_into(&mut lock(&self.watch.root));
        self.server.accept(&root, main);
    }

    /// Call `wake` when the editor asks the watch for something.
    pub fn on_ask(&self, wake: Box<dyn Fn() + Send>) {
        *lock(&self.watch.wake) = Some(wake);
    }

    /// What the editor asked for since last taken.
    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut *lock(&self.watch.asks))
    }

    /// Whether file `name` is open in the editor (its buffer owns it: the
    /// disk's changes of it are not read).
    pub fn owns(&self, name: &str) -> bool {
        self.server.owns(name)
    }

    /// The viewer is at `url`: logged, and opened by `phitex.openViewer`.
    pub fn viewer(&self, url: &str) {
        *lock(&self.watch.viewer) = Some(url.to_owned());
        self.server.viewer(url);
    }

    /// The server, as the session's progress and the watch's builds
    /// reach it.
    pub fn server(&self) -> &Arc<Server> {
        &self.server
    }

    /// Where the terminal's messages go (`Ui::tap`, beside the viewer's
    /// console): lines, notes, warnings and errors logged.
    pub fn console(&self) -> Box<dyn Fn(&Value) + Send> {
        let s = self.server.clone();
        Box::new(move |item: &Value| s.console(item))
    }

    /// A build began.
    pub fn building(&self) {
        self.server.building();
    }

    /// A build is in ([`Server::built`]).
    pub fn built(&self, b: &Built) {
        self.server.built(b);
    }

    /// The editor shows `file` (the watch's name) at `line` and character
    /// `col` (from 1): whether the client can (else the `--editor` command
    /// opens it).
    pub fn show(&self, file: &str, line: usize, col: usize) -> bool {
        self.server.show(file, line, col)
    }

    /// Serve the editor's messages on a thread of their own; `quit` ends
    /// the watch (`exit`, or the end of standard input).
    pub fn serve(&self, quit: Box<dyn Fn() + Send>) {
        let s = self.server.clone();
        let input = lock(&self.input).take();
        let pending = std::mem::take(&mut *lock(&self.pending));
        std::thread::Builder::new()
            .name("phitex-lsp".into())
            .spawn(move || {
                // (what came while the main file was waited for first)
                for m in &pending {
                    if !s.handle(m) {
                        quit();
                        return;
                    }
                }
                let mut input = input.unwrap_or_else(|| BufReader::new(std::io::stdin()));
                while let Some(m) = read(&mut input) {
                    if !s.handle(&m) {
                        break;
                    }
                }
                quit();
            })
            .expect("a thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let raw = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}Content-Length: 2\r\n\r\n{{}}",
            body.len()
        );
        let mut r = std::io::Cursor::new(raw.into_bytes());
        assert_eq!(read(&mut r).unwrap()["method"], "initialize");
        assert_eq!(read(&mut r).unwrap(), json!({}));
        assert!(read(&mut r).is_none());
    }
}
