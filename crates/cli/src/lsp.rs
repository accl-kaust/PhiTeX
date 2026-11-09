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
//!   the viewer), `phitex.stop` (the watch ends as `q` ends it),
//!   `phitex.savePdf` (the last PDF copied), `phitex.clearCache` and
//!   `phitex.cacheStats` (the build cache, `cache.rs`; the package cache,
//!   Shelf's); the settings ([`Lsp::on_configure`]) a new session where
//!   they change it.
//! - **End.** `exit`, or the end of standard input, ends the watch as `q`
//!   does. Before a main file was found ([`Lsp::main_from_open`]) the
//!   process ends at once: status 0 after `shutdown`, else 1, as the
//!   protocol says.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use phitex_live::lsp::{LspHost, Server, Settings, path_of};
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

/// The last build's PDF, as `phitex.savePdf` copies it.
#[derive(Clone)]
pub enum Pdf {
    /// The output file.
    File(PathBuf),
    /// A DVI build's PDF made for the viewer.
    Bytes(Arc<[u8]>),
    /// A PDF no file holds (a watch writing none): its exact file made
    /// when asked.
    Layout(phitex_pdf::Layout),
}

/// The watch, as the server's host: its asks queued and the watch woken.
struct Watch {
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The watch's directory (where its file names start).
    root: Mutex<PathBuf>,
    viewer: Mutex<Option<String>>,
    /// The watch's end (`exit`'s, `phitex.stop`'s).
    quit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// The settings changed: the watch told.
    configure: Mutex<Option<Box<dyn Fn(Settings) + Send>>>,
    pdf: Mutex<Option<Pdf>>,
}

/// The caches as `phitex.cacheStats` gives them: the build cache's and
/// the package cache's (docs/spec-editor-protocol.md §3.3).
pub fn cache_stats() -> Value {
    let dir = crate::tex::cache_dir();
    let build = crate::cache::Cache {
        dir: dir.clone(),
        limit: 1,
    }
    .stats();
    let (hits, misses) = crate::cache::Cache::hits();
    let packages = crate::shelf::stats();
    let one = |scope: &str, location: String, st: crate::cache::Stats, hits: u64, misses: u64| {
        json!({
            "scope": scope,
            "location": location,
            "bytes": st.bytes,
            "entries": st.entries,
            "last_used": st.last_used.map(crate::cache::iso8601),
            "hits": hits,
            "misses": misses,
        })
    };
    json!({"caches": [
        one("build", crate::cache::home_relative(&dir), build, hits, misses),
        one(
            "packages",
            crate::cache::home_relative(&packages.0),
            packages.1,
            packages.2,
            packages.3,
        ),
    ]})
}

/// `phitex.clearCache`'s `scope` (`build`, the default; `packages`;
/// `all`) emptied: `{freed_bytes}`.
pub fn clear_cache(scope: &str) -> Result<Value, String> {
    let build = || {
        crate::cache::Cache {
            dir: crate::tex::cache_dir(),
            limit: 1,
        }
        .clear_build()
        .map_err(|e| format!("the build cache: {e}"))
    };
    let freed = match scope {
        "build" => build()?,
        "packages" => crate::shelf::clear()?,
        "all" => build()? + crate::shelf::clear()?,
        s => {
            return Err(format!(
                "phitex.clearCache: no scope {s:?} (build, packages, all)"
            ));
        }
    };
    Ok(json!({"freed_bytes": freed}))
}

impl Watch {
    /// The last PDF copied to `arg`'s `path` (from the watch's
    /// directory): `{path, bytes}`.
    fn save_pdf(&self, arg: &Value) -> Result<Value, String> {
        let Some(to) = arg["path"].as_str().filter(|p| !p.is_empty()) else {
            return Err("phitex.savePdf: {path} where the PDF goes".to_owned());
        };
        let to = lock(&self.root).join(to);
        let bytes: Vec<u8> = match lock(&self.pdf).clone() {
            Some(Pdf::File(p)) => std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?,
            Some(Pdf::Bytes(b)) => b.to_vec(),
            Some(Pdf::Layout(l)) => l.exact().map_err(|e| format!("{e}"))?.to_vec(),
            None => return Err("no PDF yet (a DVI build's is made for the viewer only)".to_owned()),
        };
        std::fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        Ok(json!({"path": to.display().to_string(), "bytes": bytes.len()}))
    }
}

impl LspHost for Watch {
    /// The viewer's hidden kinds, from the user's phitex.toml (the `prefs`
    /// op's file), read as each build is published.
    fn hidden(&self) -> Vec<String> {
        crate::config::prefs(None)
            .ok()
            .and_then(|v| serde_json::from_value(v["problems-hidden"].clone()).ok())
            .unwrap_or_default()
    }

    fn ask(&self, a: Ask) {
        lock(&self.asks).push(a);
        if let Some(w) = lock(&self.wake).as_ref() {
            w();
        }
    }

    fn command(&self, name: &str, arg: &Value) -> Option<Result<Value, String>> {
        Some(match name {
            "phitex.openViewer" => match lock(&self.viewer).clone() {
                Some(url) if crate::view::open_browser(&url) => Ok(json!(url)),
                Some(url) => Err(format!("could not start a browser: open {url}")),
                None => Err("no viewer (--no-view)".to_owned()),
            },
            "phitex.stop" => {
                let quit = lock(&self.quit).clone();
                if let Some(q) = quit {
                    q();
                }
                Ok(Value::Null)
            }
            "phitex.savePdf" => self.save_pdf(arg),
            "phitex.clearCache" => clear_cache(arg["scope"].as_str().unwrap_or("build")),
            "phitex.cacheStats" => Ok(cache_stats()),
            _ => return None,
        })
    }

    fn read(&self, name: &str) -> Option<String> {
        let b = std::fs::read(lock(&self.root).join(name)).ok()?;
        Some(String::from_utf8_lossy(&b).into_owned())
    }

    fn configure(&self, s: &Settings) {
        if let Some(f) = lock(&self.configure).as_ref() {
            f(s.clone());
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

/// The `.tex` files of `dir` (not below it), sorted.
fn tex_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "tex") && p.is_file())
        .collect();
    v.sort();
    v
}

/// The document that inputs `file` (`\input`, `\include`, `\subfile`,
/// through other inputs too: a chapter's part): a `.tex` file with a
/// `\documentclass` in `file`'s folder or one of the three above it (a
/// project's folder holds no `phitex.toml` or `.git` above it), its names
/// read from its own folder or the main file's. None: no file inputs it.
fn includer(file: &Path) -> Option<PathBuf> {
    let file = file.canonicalize().ok()?;
    let mut dirs = Vec::new();
    for d in file.ancestors().skip(1).take(4) {
        dirs.push(d.to_path_buf());
        if d.join("phitex.toml").exists() || d.join(".git").exists() {
            break;
        }
    }
    let candidates: Vec<PathBuf> = dirs.iter().flat_map(|d| tex_files(d)).collect();
    let names = |t: &str| -> Vec<String> {
        let mut out: Vec<String> = crate::config::braced_inputs(t)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut rest = t;
        while let Some(i) = rest.find("\\subfile{") {
            rest = &rest[i + 9..];
            if let Some(j) = rest.find('}') {
                out.push(rest[..j].trim().to_owned());
            }
        }
        out
    };
    let names_file = |from: &Path, n: &str, target: &Path| {
        let p = Path::new(n);
        let with = if p.extension().is_some() {
            p.to_path_buf()
        } else {
            p.with_extension("tex")
        };
        dirs.iter()
            .map(PathBuf::as_path)
            .chain(from.parent())
            .any(|d| d.join(&with).canonicalize().is_ok_and(|c| c == target))
    };
    // (from the file up its chain of inputs, at most four links)
    let mut target = file;
    for _ in 0..4 {
        let by = candidates.iter().find_map(|c| {
            let t = String::from_utf8_lossy(&std::fs::read(c).ok()?).into_owned();
            let c = c.canonicalize().ok()?;
            (c != target && names(&t).iter().any(|n| names_file(&c, n, &target)))
                .then_some((c, t.contains("\\documentclass")))
        });
        match by {
            Some((c, true)) => return Some(c),
            Some((c, false)) => target = c,
            None => return None,
        }
    }
    None
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

/// The main file named by the first `.tex` document opened on `input`
/// ([`Lsp::main_from_open`]), and the messages read meanwhile (to be
/// served once the watch runs). Err: the editor ended the session first,
/// the exit status the protocol gives it: 0 after `shutdown`, else 1 (an
/// `exit` without it, or the end of the input).
fn wait_main(s: &Server, input: &mut impl BufRead) -> Result<(PathBuf, Vec<Value>), i32> {
    let mut pending = Vec::new();
    let mut shut_down = false;
    let end = |shut_down: bool| if shut_down { 0 } else { crate::EXIT_FAILED };
    loop {
        let Some(m) = read(input) else {
            return Err(end(shut_down));
        };
        match m["method"].as_str().unwrap_or("") {
            "exit" => return Err(end(shut_down)),
            "shutdown" => {
                shut_down = true;
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
        let text = d["text"].as_str().unwrap_or("");
        let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        // (the file names its main one, `% !TEX root = ../paper.tex`)
        if let (_, Some(r)) = crate::config::magic_comments(text)
            && dir.join(&r).is_file()
        {
            return Ok((dir.join(r), pending));
        }
        if text.contains("\\documentclass") {
            return Ok((path, pending));
        }
        // (a file the main one inputs, opened first: that main file,
        // beside it or above, never the file itself or none)
        if let Some(main) = includer(&path) {
            return Ok((main, pending));
        }
        if let Some(n) = find_main(&dir) {
            return Ok((dir.join(n), pending));
        }
        s.show_message(
            2,
            &format!(
                "phitex watch: {} has no \\documentclass, nor one .tex file beside it: open the main file",
                path.display()
            ),
        );
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
            quit: Mutex::new(None),
            configure: Mutex::new(None),
            pdf: Mutex::new(None),
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
        let (main, pending) = match wait_main(s, &mut input) {
            Ok(found) => found,
            Err(code) => std::process::exit(code),
        };
        *lock(&self.input) = Some(input);
        *lock(&self.pending) = pending;
        // (the watch runs in the main file's folder: its inputs are named
        // from there, and the editor's root may be a folder below it)
        if let Some(dir) = main.parent()
            && std::env::set_current_dir(dir).is_ok()
            && let Some(name) = main.file_name()
        {
            return name.to_string_lossy().into_owned();
        }
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

    /// Call `f` with the settings when the editor changes them.
    pub fn on_configure(&self, f: Box<dyn Fn(Settings) + Send>) {
        *lock(&self.watch.configure) = Some(f);
    }

    /// The settings the editor gave so far.
    pub fn settings(&self) -> Settings {
        self.server.settings()
    }

    /// The last build's PDF, for `phitex.savePdf`.
    pub fn set_pdf(&self, pdf: Option<Pdf>) {
        *lock(&self.watch.pdf) = pdf;
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

    /// The diagnostics a build left for later, made now
    /// ([`Server::diagnostics`]).
    pub fn diagnostics(&self, d: &[serde_json::Value]) {
        self.server.diagnostics(d);
    }

    /// The editor shows `file` (the watch's name) at `line` and character
    /// `col` (from 1): whether the client can (else the `--editor` command
    /// opens it).
    pub fn show(&self, file: &str, line: usize, col: usize) -> bool {
        self.server.show(file, line, col)
    }

    /// The editor selects `file` from `from` to `to` (line, character,
    /// from 1), a selection made on the pages: whether the client can.
    pub fn select(&self, file: &str, from: (usize, usize), to: (usize, usize)) -> bool {
        self.server.select(file, from, to)
    }

    /// Serve the editor's messages on a thread of their own; `quit` ends
    /// the watch (`exit`, or the end of standard input).
    pub fn serve(&self, quit: Box<dyn Fn() + Send + Sync>) {
        let quit: Arc<dyn Fn() + Send + Sync> = Arc::from(quit);
        *lock(&self.watch.quit) = Some(quit.clone());
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

    /// Messages framed as an editor sends them.
    fn framed(ms: &[Value]) -> std::io::Cursor<Vec<u8>> {
        let mut raw = Vec::new();
        for m in ms {
            let body = m.to_string();
            raw.extend(format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes());
        }
        std::io::Cursor::new(raw)
    }

    /// A server on the watch's host, its messages kept.
    fn server() -> (Server, Arc<Mutex<Vec<Value>>>) {
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s = sent.clone();
        let watch = Arc::new(Watch {
            asks: Mutex::default(),
            wake: Mutex::new(None),
            root: Mutex::new(PathBuf::new()),
            viewer: Mutex::new(None),
            quit: Mutex::new(None),
            configure: Mutex::new(None),
            pdf: Mutex::new(None),
        });
        let server = Server::new(Box::new(move |v: &Value| lock(&s).push(v.clone())), watch);
        (server, sent)
    }

    /// An editor that ends the session while the server still waits for
    /// a main file: exit status 0 after `shutdown` (the protocol's rule;
    /// Neovim reports any other), 1 after an `exit` or an end without it.
    #[test]
    fn ending_before_a_main_file() {
        let shutdown = json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"});
        let exit = json!({"jsonrpc": "2.0", "method": "exit"});
        let (s, sent) = server();
        let mut r = framed(&[shutdown.clone(), exit.clone()]);
        assert_eq!(wait_main(&s, &mut r).err(), Some(0));
        assert_eq!(lock(&sent)[0]["id"], 2, "shutdown answered");
        assert_eq!(lock(&sent)[0]["result"], Value::Null);
        let (s, _) = server();
        assert_eq!(wait_main(&s, &mut framed(&[shutdown])).err(), Some(0));
        let (s, _) = server();
        assert_eq!(
            wait_main(&s, &mut framed(&[exit])).err(),
            Some(crate::EXIT_FAILED)
        );
        let (s, _) = server();
        assert_eq!(
            wait_main(&s, &mut framed(&[])).err(),
            Some(crate::EXIT_FAILED)
        );
    }

    /// The first `.tex` document opened with a `\documentclass` is the
    /// main file; what came before it is kept to be served.
    #[test]
    fn the_first_document_opened_names_the_main_file() {
        let (s, _) = server();
        let mut r = framed(&[
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///nowhere/a.bib", "text": "@book{x}"}}}),
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///nowhere/paper.tex", "text": "\\documentclass{article}"}}}),
        ]);
        let (main, pending) = wait_main(&s, &mut r).expect("a main file");
        assert_eq!(main, Path::new("/nowhere/paper.tex"));
        assert_eq!(pending.len(), 3);
    }
}
