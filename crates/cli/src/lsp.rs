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

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use phitex_live::lsp::{LspHost, Server, Settings, Trouble, path_of};
use phitex_live::progress::Progress;
use serde_json::{Value, json};

use crate::view::{Ask, Built};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The next message on `r` (none: its end).
pub(crate) fn read(r: &mut impl BufRead) -> Option<Value> {
    read_capped(r, usize::MAX)
}

/// The next message on `r`, its body at most `max` bytes (none: its end,
/// or a longer one).
pub(crate) fn read_capped(r: &mut impl BufRead, max: usize) -> Option<Value> {
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
        if n > max {
            return None;
        }
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
    /// The editors' servers: the one on standard input and output (id 0,
    /// `--lsp`), those attached over the watch's socket ([`crate::attach`]).
    servers: Mutex<Vec<(u64, Arc<Server>)>>,
    /// The client whose cursor or edit came last: a place on the pages is
    /// shown in it.
    focus: Mutex<Option<u64>>,
    /// Each file's last editor: another client's edit of it is its whole
    /// text (the last edit wins; its ranges are in that client's text).
    writers: Mutex<HashMap<String, u64>>,
    /// The main file, from the root, once the watch runs.
    main: Mutex<Option<String>>,
    /// The last diagnostics, for a client that attaches later.
    diags: Mutex<Vec<Value>>,
    /// The next attached client's id.
    next: AtomicU64,
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

impl Watch {
    fn new() -> Arc<Watch> {
        Arc::new(Watch {
            asks: Mutex::default(),
            wake: Mutex::new(None),
            root: Mutex::new(PathBuf::new()),
            viewer: Mutex::new(None),
            quit: Mutex::new(None),
            configure: Mutex::new(None),
            pdf: Mutex::new(None),
            servers: Mutex::default(),
            focus: Mutex::new(None),
            writers: Mutex::default(),
            main: Mutex::new(None),
            diags: Mutex::default(),
            next: AtomicU64::new(1),
        })
    }

    /// The servers now (none locked while they are called).
    fn servers(&self) -> Vec<(u64, Arc<Server>)> {
        lock(&self.servers).clone()
    }

    /// The viewer's hidden kinds, from the user's phitex.toml (the `prefs`
    /// op's file), read as each build is published.
    fn hidden(&self) -> Vec<String> {
        crate::config::prefs(None)
            .ok()
            .and_then(|v| serde_json::from_value(v["problems-hidden"].clone()).ok())
            .unwrap_or_default()
    }

    fn push(&self, a: Ask) {
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

    fn exists(&self, name: &str) -> bool {
        lock(&self.root).join(name).exists()
    }
}

/// One editor's host: the watch, as one client sees it. Its edits and
/// cursor are the watch's asks; the stdio client (`--lsp`, id 0) owns
/// the watch (`phitex.stop` ends it, its settings start a new session),
/// an attached one only itself (`phitex.stop` and `exit` detach it).
struct Client {
    watch: Arc<Watch>,
    id: u64,
    attached: bool,
    server: OnceLock<Weak<Server>>,
    /// An attached client's connection, closed (its `phitex.stop`).
    close: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Client {
    fn new(watch: &Arc<Watch>, id: u64, attached: bool) -> Arc<Client> {
        Arc::new(Client {
            watch: watch.clone(),
            id,
            attached,
            server: OnceLock::new(),
            close: Mutex::new(None),
        })
    }

    fn text(&self, file: &str) -> Option<String> {
        self.server.get()?.upgrade()?.text(file)
    }
}

impl LspHost for Client {
    fn hidden(&self) -> Vec<String> {
        self.watch.hidden()
    }

    fn ask(&self, a: Ask) {
        let w = &self.watch;
        let a = match a {
            // (another client edited the file last: this one's ranges are
            // in its own text, so its whole text)
            Ask::Edit {
                file,
                start,
                end,
                text,
            } => {
                let last = lock(&w.writers).insert(file.clone(), self.id);
                match self.text(&file) {
                    Some(whole) if last.is_some_and(|l| l != self.id) => {
                        Ask::SetFile { file, text: whole }
                    }
                    _ => Ask::Edit {
                        file,
                        start,
                        end,
                        text,
                    },
                }
            }
            Ask::SetFile { file, text } => {
                lock(&w.writers).insert(file.clone(), self.id);
                Ask::SetFile { file, text }
            }
            // (closed here, open in another client: its text)
            Ask::Closed { file } => {
                let other = w
                    .servers()
                    .into_iter()
                    .find(|(id, s)| *id != self.id && s.owns(&file));
                match other.and_then(|(id, s)| Some((id, s.text(&file)?))) {
                    Some((id, text)) => {
                        lock(&w.writers).insert(file.clone(), id);
                        Ask::SetFile { file, text }
                    }
                    None => {
                        lock(&w.writers).remove(&file);
                        Ask::Closed { file }
                    }
                }
            }
            a => a,
        };
        if matches!(
            a,
            Ask::Cursor { .. } | Ask::Edit { .. } | Ask::SetFile { .. }
        ) {
            *lock(&w.focus) = Some(self.id);
        }
        w.push(a);
    }

    fn command(&self, name: &str, arg: &Value) -> Option<Result<Value, String>> {
        if self.attached && name == "phitex.stop" {
            if let Some(c) = lock(&self.close).as_ref() {
                c();
            }
            return Some(Ok(Value::Null));
        }
        self.watch.command(name, arg)
    }

    fn read(&self, name: &str) -> Option<String> {
        self.watch.read(name)
    }

    fn configure(&self, s: &Settings) {
        // (the watch is its terminal's or its first editor's: an attached
        // client's settings change nothing of it)
        if !self.attached {
            self.watch.configure(s);
        }
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        path.canonicalize().ok()
    }

    fn exists(&self, name: &str) -> bool {
        self.watch.exists(name)
    }

    fn engine(&self) -> &'static str {
        "native"
    }
}

/// The watch's language server.
pub struct Lsp {
    /// The editor on standard input and output (`--lsp`); none: a
    /// terminal's watch, which editors may attach to.
    server: Option<Arc<Server>>,
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
        let watch = Watch::new();
        let o = out.clone();
        let client = Client::new(&watch, 0, false);
        let server = Arc::new(Server::new(
            Box::new(move |v: &Value| write(&o, v)),
            client.clone(),
        ));
        let _ = client.server.set(Arc::downgrade(&server));
        lock(&watch.servers).push((0, server.clone()));
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
                server: Some(server),
                watch,
                input: Mutex::new(Some(input)),
                wants_main: file.is_none() && !configured,
                pending: Mutex::default(),
            };
        }
    }

    /// A terminal's watch: no editor on standard input; editors attach
    /// over its socket ([`crate::attach`]) once it runs.
    pub fn terminal() -> Lsp {
        Lsp {
            server: None,
            watch: Watch::new(),
            input: Mutex::new(None),
            wants_main: false,
            pending: Mutex::default(),
        }
    }

    /// Whether an editor runs this watch on standard input and output
    /// (`--lsp`): no keys, no terminal.
    pub fn editor(&self) -> bool {
        self.server.is_some()
    }

    /// Whether any editor is connected now (the stdio one or one
    /// attached): the work only an editor reads is worth doing.
    pub fn served(&self) -> bool {
        !lock(&self.watch.servers).is_empty()
    }

    /// The watch cannot run: with an editor on standard input,
    /// `initialize` answered with why, or, answered already, the editor
    /// shown it, and the watch ends; else nothing (the terminal says it).
    pub fn refuse(&self, e: &str) {
        if let Some(s) = &self.server {
            s.refuse(&format!("phitex watch: {e}"));
            crate::attach::forget();
            std::process::exit(crate::EXIT_FAILED);
        }
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
        let s = self.server.as_ref().expect("an editor on standard input");
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

    /// The watch runs in `root` on `main`: the stdio editor's
    /// `initialize` answered, editors attached told, and the watch's
    /// socket served and published ([`crate::attach`]).
    pub fn accept(&self, root: &Path, main: &str) {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        root.clone_into(&mut lock(&self.watch.root));
        *lock(&self.watch.main) = Some(main.to_owned());
        for (id, s) in self.watch.servers() {
            if id == 0 {
                s.accept(&root, main);
            } else {
                s.set_main(main);
            }
        }
        listen(&self.watch);
    }

    /// Call `wake` when the editor asks the watch for something.
    pub fn on_ask(&self, wake: Box<dyn Fn() + Send>) {
        *lock(&self.watch.wake) = Some(wake);
    }

    /// Call `f` with the settings when the editor changes them.
    pub fn on_configure(&self, f: Box<dyn Fn(Settings) + Send>) {
        *lock(&self.watch.configure) = Some(f);
    }

    /// `window/showMessage` of `kind` to the stdio editor, whose
    /// settings the watch takes.
    pub fn show_message(&self, kind: u8, text: &str) {
        if let Some(s) = &self.server {
            s.show_message(kind, text);
        }
    }

    /// The settings the stdio editor gave so far.
    pub fn settings(&self) -> Settings {
        self.server
            .as_ref()
            .map(|s| s.settings())
            .unwrap_or_default()
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
        self.watch.servers().iter().any(|(_, s)| s.owns(name))
    }

    /// The viewer is at `url`: logged, opened by `phitex.openViewer`,
    /// and published for editors that attach.
    pub fn viewer(&self, url: &str) {
        *lock(&self.watch.viewer) = Some(url.to_owned());
        for (_, s) in self.watch.servers() {
            s.viewer(url);
        }
        crate::attach::publish(&info(&self.watch));
    }

    /// A build's progress record, to every editor.
    pub fn progress(&self, p: &Progress) {
        for (_, s) in self.watch.servers() {
            s.progress(p);
        }
    }

    /// What needs the user ([`Server::trouble`]), to every editor.
    pub fn trouble(&self, t: Trouble, why: &str) {
        for (_, s) in self.watch.servers() {
            s.trouble(t, why);
        }
    }

    /// Where the terminal's messages go (`Ui::tap`, beside the viewer's
    /// console): lines, notes, warnings and errors logged, in every
    /// editor.
    pub fn console(&self) -> Box<dyn Fn(&Value) + Send> {
        let w = self.watch.clone();
        Box::new(move |item: &Value| {
            for (_, s) in w.servers() {
                s.console(item);
            }
        })
    }

    /// A build began.
    pub fn building(&self) {
        for (_, s) in self.watch.servers() {
            s.building();
        }
    }

    /// A build is in ([`Server::built`]).
    pub fn built(&self, b: &Built) {
        if let Some(d) = &b.diagnostics {
            d.clone_into(&mut lock(&self.watch.diags));
        }
        for (_, s) in self.watch.servers() {
            s.built(b);
        }
    }

    /// The diagnostics a build left for later, made now
    /// ([`Server::diagnostics`]).
    pub fn diagnostics(&self, d: &[serde_json::Value]) {
        d.clone_into(&mut lock(&self.watch.diags));
        for (_, s) in self.watch.servers() {
            s.diagnostics(d);
        }
    }

    /// The editor whose cursor or edit came last (else every editor)
    /// does `f`: whether one could.
    fn in_focus(&self, f: impl Fn(&Server) -> bool) -> bool {
        let servers = self.watch.servers();
        let focus = *lock(&self.watch.focus);
        if let Some((_, s)) = servers.iter().find(|(id, _)| Some(*id) == focus)
            && f(s)
        {
            return true;
        }
        servers
            .iter()
            .filter(|(id, _)| Some(*id) != focus)
            .fold(false, |any, (_, s)| f(s) || any)
    }

    /// The editor shows `file` (the watch's name) at `line` and character
    /// `col` (from 1): whether a client can (else the `--editor` command
    /// opens it). The client that moved last shows it, else every one.
    pub fn show(&self, file: &str, line: usize, col: usize) -> bool {
        self.in_focus(|s| s.show(file, line, col))
    }

    /// The editor selects `file` from `from` to `to` (line, character,
    /// from 1), a selection made on the pages: whether a client can.
    pub fn select(&self, file: &str, from: (usize, usize), to: (usize, usize)) -> bool {
        self.in_focus(|s| s.select(file, from, to))
    }

    /// Serve the editor's messages on a thread of their own; `quit` ends
    /// the watch (`exit`, or the end of standard input).
    pub fn serve(&self, quit: Box<dyn Fn() + Send + Sync>) {
        let quit: Arc<dyn Fn() + Send + Sync> = Arc::from(quit);
        *lock(&self.watch.quit) = Some(quit.clone());
        let Some(s) = self.server.clone() else {
            return;
        };
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

/// The watch as its discovery file says it ([`crate::attach::publish`]):
/// `{pid, main, root, viewer, version, started}`, the socket's own
/// fields added there.
fn info(w: &Watch) -> Value {
    let root = lock(&w.root).clone();
    let main = lock(&w.main).as_deref().map(|m| root.join(m));
    json!({
        "pid": std::process::id(),
        "main": main.map(|m| m.display().to_string()),
        "root": root.display().to_string(),
        "viewer": lock(&w.viewer).clone(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Serve the watch's socket (once, from the first session that runs):
/// each connection that opens with `initialize` and the token is an
/// editor attached ([`attached`]).
fn listen(watch: &Arc<Watch>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let mut started = false;
    ONCE.call_once(|| started = true);
    if !started {
        crate::attach::publish(&info(watch));
        return;
    }
    let w = watch.clone();
    let opened = crate::attach::open(&info(watch), move |first, input, out| {
        attached(&w, &first, input, out);
    });
    if let Err(e) = opened {
        // (a watch that cannot be attached to is still a watch)
        for (_, s) in watch.servers() {
            s.log(2, &format!("phitex watch: editors cannot attach: {e}"));
        }
    }
}

/// An editor attached: `first` its `initialize` (the token checked), its
/// messages read from `input` and its answers written to `out`, until it
/// ends (`exit`, its connection closed, `phitex.stop`); the watch goes
/// on.
fn attached(watch: &Arc<Watch>, first: &Value, mut input: impl BufRead, out: std::net::TcpStream) {
    let id = watch.next.fetch_add(1, Ordering::Relaxed);
    let closer = out.try_clone().ok();
    let out = Mutex::new(out);
    let client = Client::new(watch, id, true);
    let server = Arc::new(Server::new(
        Box::new(move |v: &Value| {
            let body = v.to_string();
            let mut o = lock(&out);
            let sent =
                write!(o, "Content-Length: {}\r\n\r\n{body}", body.len()).and_then(|()| o.flush());
            // (a client that does not read: its connection closed, the
            // watch never held up by it)
            if sent.is_err() {
                let _ = o.shutdown(std::net::Shutdown::Both);
            }
        }),
        client.clone(),
    ));
    let _ = client.server.set(Arc::downgrade(&server));
    if let Some(c) = closer {
        *lock(&client.close) = Some(Box::new(move || {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }));
    }
    server.initialize(first);
    server.set_attached();
    let root = lock(&watch.root).clone();
    let main = lock(&watch.main).clone().unwrap_or_default();
    server.accept(&root, &main);
    if let Some(url) = lock(&watch.viewer).clone() {
        server.viewer(&url);
    }
    let diags = lock(&watch.diags).clone();
    server.diagnostics(&diags);
    lock(&watch.servers).push((id, server.clone()));
    while let Some(m) = read(&mut input) {
        if !server.handle(&m) {
            break;
        }
    }
    // (detached: its buffers are the watch's no more, unless another
    // client has them open)
    lock(&watch.servers).retain(|(i, _)| *i != id);
    {
        let mut focus = lock(&watch.focus);
        if *focus == Some(id) {
            *focus = None;
        }
    }
    for file in server.files() {
        client.ask(Ask::Closed { file });
    }
    if let Some(c) = lock(&client.close).as_ref() {
        c();
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
        let watch = Watch::new();
        let server = Server::new(
            Box::new(move |v: &Value| lock(&s).push(v.clone())),
            Client::new(&watch, 0, false),
        );
        (server, sent)
    }

    /// A client of `watch` (`id`, attached or not) as `attached` makes
    /// one, its messages dropped.
    fn client_of(watch: &Arc<Watch>, id: u64, attached: bool) -> Arc<Server> {
        let c = Client::new(watch, id, attached);
        let s = Arc::new(Server::new(Box::new(|_: &Value| {}), c.clone()));
        let _ = c.server.set(Arc::downgrade(&s));
        s.initialize(&json!({"id": 1, "params": {"rootUri": "file:///p", "capabilities": {}}}));
        s.accept(Path::new("/p"), "a.tex");
        lock(&watch.servers).push((id, s.clone()));
        s
    }

    /// Two editors on one watch: each one's buffer edits as its own;
    /// an edit after the other's is its whole text (the last edit wins);
    /// the cursor's client is the one in focus; a file one closes that
    /// the other has open is the other's text, else the disk's again.
    #[test]
    fn two_editors_on_one_watch() {
        let w = Watch::new();
        let (a, b) = (client_of(&w, 0, false), client_of(&w, 1, true));
        let open = |text: &str| json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///p/a.tex", "text": text}}});
        let change = |c: u64, text: &str| {
            json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": "file:///p/a.tex"},
            "contentChanges": [{"range": {"start": {"line": 0, "character": c}, "end": {"line": 0, "character": c}}, "text": text}]}})
        };
        a.handle(&open("one"));
        b.handle(&open("one"));
        a.handle(&change(3, "!"));
        a.handle(&change(4, "?"));
        b.handle(&change(0, ">"));
        let asks = std::mem::take(&mut *lock(&w.asks));
        let kinds: Vec<String> = asks
            .iter()
            .map(|x| match x {
                Ask::SetFile { text, .. } => format!("set {text}"),
                Ask::Edit { start, text, .. } => format!("edit {start} {text}"),
                _ => "other".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            ["set one", "set one", "set one!", "edit 4 ?", "set >one"]
        );
        assert_eq!(*lock(&w.focus), Some(1));
        a.handle(
            &json!({"jsonrpc": "2.0", "method": "phitex/cursor", "params": {
            "textDocument": {"uri": "file:///p/a.tex"}, "position": {"line": 0, "character": 1}}}),
        );
        assert_eq!(*lock(&w.focus), Some(0));
        lock(&w.asks).clear();
        // (b closes it, a has it open: a's text)
        b.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didClose", "params": {"textDocument": {"uri": "file:///p/a.tex"}}}));
        assert!(matches!(&lock(&w.asks)[..], [Ask::SetFile { text, .. }] if text == "one!?"));
        lock(&w.asks).clear();
        a.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didClose", "params": {"textDocument": {"uri": "file:///p/a.tex"}}}));
        assert!(matches!(&lock(&w.asks)[..], [Ask::Closed { .. }]));
    }

    /// An attached client's `phitex.stop` and settings touch only itself:
    /// the watch neither ends nor starts a new session.
    #[test]
    fn an_attached_client_owns_only_itself() {
        let w = Watch::new();
        let quits = Arc::new(AtomicU64::new(0));
        let q = quits.clone();
        *lock(&w.quit) = Some(Arc::new(move || {
            q.fetch_add(1, Ordering::Relaxed);
        }));
        let configured = Arc::new(AtomicU64::new(0));
        let c = configured.clone();
        *lock(&w.configure) = Some(Box::new(move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        }));
        let b = client_of(&w, 1, true);
        b.handle(&json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {"phitex": {"threads": 8}}}}));
        b.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/executeCommand", "params": {"command": "phitex.stop"}}));
        assert_eq!(quits.load(Ordering::Relaxed), 0);
        assert_eq!(configured.load(Ordering::Relaxed), 0);
        let a = client_of(&w, 0, false);
        a.handle(&json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {"phitex": {"threads": 8}}}}));
        a.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/executeCommand", "params": {"command": "phitex.stop"}}));
        assert_eq!(quits.load(Ordering::Relaxed), 1);
        assert_eq!(configured.load(Ordering::Relaxed), 1);
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
