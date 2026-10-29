//! The language server every editor speaks to PhiTeX
//! (docs/spec-editor-protocol.md): one implementation, without its
//! transport. [`Server::handle`] takes one JSON-RPC message; the
//! server's messages go out through the callback it was made with. The
//! native watch wraps it in `Content-Length` framing over standard input
//! and output (`phitex watch --lsp`, the CLI's `lsp.rs`), the wasm core in
//! its `ph_lsp` export and `phitex.lsp` import ([`crate::document::Document::lsp`]).
//!
//! What the editor asks becomes the viewer's asks ([`Ask`]: a buffer set,
//! edited or closed, the cursor, a build), given to the host
//! ([`LspHost::ask`]); what the host tells the server (a build's start,
//! its progress records, its end, the console, a source place) becomes
//! the editor's notifications and requests. Native and wasm feed it the
//! same calls from the same records ([`crate::progress`]).
//!
//! - **Start.** `initialize` ([`Server::initialize`]): the root (`rootUri`,
//!   else the first of `workspaceFolders`, else `rootPath`), where file
//!   names start: a `file:` root a directory, any other a URI prefix; the
//!   client's capabilities. Answered ([`Server::answer_init`]) with: text
//!   synchronised incrementally, positions in UTF-16 (the viewer's `edit`
//!   counts in it already), the commands. A host may hold the answer until
//!   it knows the main file ([`Server::accept`], the native watch).
//! - **Buffers.** `didOpen`, `didChange` (ranged or whole) and `didClose`
//!   are [`Ask::SetFile`], [`Ask::Edit`] and [`Ask::Closed`] (the file as
//!   on disk again); while a file is open its buffer owns it
//!   ([`Server::owns`]). `didSave` brings nothing the buffer did not.
//! - **Builds.** [`Server::building`] (a build began),
//!   [`Server::progress`] (its records) and [`Server::built`]: the
//!   problems published by file (`textDocument/publishDiagnostics`, the
//!   main file's always, those of files that no longer have any
//!   cleared), `$/progress` (the first build always, a later one once it
//!   has run [`SHOW_AFTER`]) and `phitex/status`.
//! - **Inverse search.** A source place (a double-click, an error's) is
//!   `window/showDocument`, the place selected and the focus taken
//!   ([`Server::show`]).
//! - **Forward search.** `phitex/cursor {textDocument: {uri}, position}`
//!   or the command `phitex.showOnPage` with the same argument is
//!   [`Ask::Cursor`].
//! - **End.** `shutdown` answered; `exit` ends ([`Server::handle`]'s
//!   false).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::progress::{Phase, Progress};
use crate::view::{Ask, Built};

/// The commands the server runs (`workspace/executeCommand`).
pub const COMMANDS: [&str; 3] = ["phitex.openViewer", "phitex.rebuild", "phitex.showOnPage"];

/// A build later than the first shows its progress once it has run this
/// long (typing never flickers).
pub const SHOW_AFTER: Duration = Duration::from_millis(400);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the server asks of its host.
pub trait LspHost: Send + Sync {
    /// The editor asks the session for `a` (as the viewer would).
    fn ask(&self, a: Ask);

    /// Command `name` with argument `arg` (the first of
    /// `executeCommand`'s arguments, null if none), run by the host
    /// (`phitex.openViewer`, ...): its result, or why it failed; none: no
    /// such command here.
    fn command(&self, _name: &str, _arg: &Value) -> Option<Result<Value, String>> {
        None
    }

    /// `path` as the host names it whole (the CLI's canonical path): a
    /// URI's file under the root by a link (none: as it is).
    fn canonical(&self, _path: &Path) -> Option<PathBuf> {
        None
    }

    /// Whether the project's file `name` exists (a diagnostic's file
    /// named without its `.tex`).
    fn exists(&self, _name: &str) -> bool {
        true
    }

    /// `phitex/status`'s engine: `native` or `wasm`.
    fn engine(&self) -> &'static str;
}

/// `p` as a `file:` URI (its bytes past the unreserved ones and `/`
/// percent-encoded).
pub fn uri_of(p: &Path) -> String {
    format!("file://{}", encode(&p.to_string_lossy()))
}

/// `s` with its bytes past the unreserved ones and `/` percent-encoded.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `s` percent-decoded.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| char::from(c).to_digit(16);
        if b[i] == b'%'
            && let (Some(&h), Some(&l)) = (b.get(i + 1), b.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path of a `file:` URI (none: another scheme).
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    Some(PathBuf::from(decode(rest)))
}

/// The UTF-16 offset in `text` of LSP position `line`, `character` (a
/// character past the line's end is its end).
pub fn offset(text: &str, line: usize, character: usize) -> usize {
    let mut off = 0;
    let mut rest = text;
    for _ in 0..line {
        match rest.find('\n') {
            Some(i) => {
                off += rest[..=i].encode_utf16().count();
                rest = &rest[i + 1..];
            }
            None => return off + rest.encode_utf16().count(),
        }
    }
    let l = rest.split('\n').next().unwrap_or("");
    off + character.min(l.encode_utf16().count())
}

/// Where the project's file names start.
#[derive(Default)]
struct Root {
    /// A directory (`file:` roots), else a URI prefix; no `/` at its end.
    base: String,
    file: bool,
}

/// The client's capabilities the server uses.
#[derive(Default, Clone, Copy)]
struct Caps {
    show_document: bool,
    work_done: bool,
}

/// The last build's facts, as `phitex/status` gives them.
#[derive(Default, Clone, Copy)]
struct Facts {
    ms: f64,
    pages: usize,
    errors: usize,
    warnings: usize,
}

/// The build under way.
struct Run {
    began: Instant,
    /// Its `$/progress` token, once shown.
    token: Option<String>,
    /// Whether its progress is shown (the first build, or one past
    /// [`SHOW_AFTER`]).
    shown: bool,
}

#[derive(Default)]
struct State {
    root: Root,
    main: String,
    caps: Caps,
    /// `initialize`'s id, until answered.
    init: Option<Value>,
    /// The documents the editor has open, by name: their texts, kept as
    /// the editor changes them (its positions are in them).
    docs: HashMap<String, String>,
    /// The URIs of the last diagnostics published, not empty.
    published: HashSet<String>,
    run: Option<Run>,
    builds: u64,
    last: Facts,
    /// The last `phitex/status` state sent.
    state: &'static str,
    viewer: Option<String>,
}

/// The language server.
pub struct Server {
    out: Box<dyn Fn(&Value) + Send + Sync>,
    host: Arc<dyn LspHost>,
    st: Mutex<State>,
    next: AtomicU64,
}

impl Server {
    /// A server whose messages go to `out`, asking `host`.
    pub fn new(out: Box<dyn Fn(&Value) + Send + Sync>, host: Arc<dyn LspHost>) -> Server {
        Server {
            out,
            host,
            st: Mutex::default(),
            next: AtomicU64::new(1),
        }
    }

    fn send(&self, v: &Value) {
        (self.out)(v);
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// A request of the server's (its id `phitex-N`, answered or not).
    fn request(&self, method: &str, params: Value) -> String {
        let id = format!("phitex-{}", self.next.fetch_add(1, Ordering::Relaxed));
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn fail(&self, id: &Value, code: i32, message: &str) {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        );
    }

    /// `window/logMessage` of `kind` (1 an error ... 4 a line).
    pub fn log(&self, kind: u8, text: &str) {
        self.notify("window/logMessage", json!({"type": kind, "message": text}));
    }

    /// `window/showMessage` of `kind` (1 an error, 2 a warning, 3 info).
    pub fn show_message(&self, kind: u8, text: &str) {
        self.notify("window/showMessage", json!({"type": kind, "message": text}));
    }

    /// `initialize` (message `m`) read: the root, the client's
    /// capabilities; its answer held ([`Server::answer_init`]).
    pub fn initialize(&self, m: &Value) {
        let p = &m["params"];
        let caps = &p["capabilities"]["window"];
        let root = p["rootUri"]
            .as_str()
            .or_else(|| p["workspaceFolders"][0]["uri"].as_str())
            .map(|u| match path_of(u) {
                Some(path) => Root {
                    base: path.to_string_lossy().trim_end_matches('/').to_owned(),
                    file: true,
                },
                None => Root {
                    base: decode(u).trim_end_matches('/').to_owned(),
                    file: false,
                },
            })
            .or_else(|| {
                p["rootPath"].as_str().map(|r| Root {
                    base: r.trim_end_matches('/').to_owned(),
                    file: true,
                })
            });
        let mut st = lock(&self.st);
        st.caps = Caps {
            show_document: caps["showDocument"]["support"].as_bool().unwrap_or(false),
            work_done: caps["workDoneProgress"].as_bool().unwrap_or(false),
        };
        if let Some(r) = root {
            st.root = r;
        }
        st.init = Some(m["id"].clone());
    }

    /// `initialize` answered with the server's capabilities (once).
    pub fn answer_init(&self) {
        let Some(id) = lock(&self.st).init.take() else {
            return;
        };
        self.reply(
            &id,
            json!({
                "capabilities": {
                    "positionEncoding": "utf-16",
                    "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
                    "executeCommandProvider": {"commands": COMMANDS},
                },
                "serverInfo": {"name": "phitex", "version": env!("CARGO_PKG_VERSION")},
            }),
        );
    }

    /// The session cannot run: `initialize` answered with why, or,
    /// answered already, the editor shown it.
    pub fn refuse(&self, e: &str) {
        match lock(&self.st).init.take() {
            Some(id) => self.fail(&id, -32603, e),
            None => self.show_message(1, e),
        }
    }

    /// The session runs in directory `root` on `main` (its name from
    /// there): `initialize` answered.
    pub fn accept(&self, root: &Path, main: &str) {
        {
            let mut st = lock(&self.st);
            st.root = Root {
                base: root.to_string_lossy().trim_end_matches('/').to_owned(),
                file: true,
            };
            main.clone_into(&mut st.main);
        }
        self.answer_init();
    }

    /// The main file, its name from the root.
    pub fn set_main(&self, main: &str) {
        main.clone_into(&mut lock(&self.st).main);
    }

    /// Whether file `name` is open in the editor (its buffer owns it).
    pub fn owns(&self, name: &str) -> bool {
        lock(&self.st).docs.contains_key(name)
    }

    /// The viewer is at `url`: logged, and opened by `phitex.openViewer`.
    pub fn viewer(&self, url: &str) {
        lock(&self.st).viewer = Some(url.to_owned());
        self.log(
            3,
            &format!("phitex watch: the viewer is at {url} (phitex.openViewer opens it)"),
        );
    }

    /// The viewer's address, if there is one.
    pub fn viewer_url(&self) -> Option<String> {
        lock(&self.st).viewer.clone()
    }

    /// One of the host's messages (the viewer's console items): lines,
    /// notes, warnings and errors logged.
    pub fn console(&self, item: &Value) {
        let text = item["text"].as_str().unwrap_or("");
        match item["kind"].as_str().unwrap_or("") {
            "error" => self.log(1, text),
            "warning" => self.log(2, text),
            "note" => self.log(3, text),
            "line" => self.log(4, text),
            _ => {}
        }
    }

    /// The watch's name of the file at `uri` (from the root), if it is
    /// under it.
    fn name(&self, uri: &str) -> Option<String> {
        let root = {
            let st = lock(&self.st);
            (st.root.base.clone(), st.root.file)
        };
        let rel = if root.1 {
            let p = path_of(uri)?;
            let base = Path::new(&root.0);
            p.strip_prefix(base)
                .ok()
                .map(Path::to_path_buf)
                .or_else(|| {
                    let c = self.host.canonical(&p)?;
                    c.strip_prefix(base).ok().map(Path::to_path_buf)
                })?
                .to_string_lossy()
                .into_owned()
        } else {
            decode(uri)
                .strip_prefix(&format!("{}/", root.0))?
                .to_owned()
        };
        (!rel.is_empty() && !rel.starts_with('.') && !rel.contains("..")).then_some(rel)
    }

    /// The URI of the file a diagnostic or the viewer names (from the
    /// root, or absolute; `name.tex` for a `name` that is not there).
    fn uri(&self, file: &str) -> String {
        let (base, is_file) = {
            let st = lock(&self.st);
            (st.root.base.clone(), st.root.file)
        };
        let f = file.trim_start_matches("./");
        let f = if Path::new(f).extension().is_some() || self.host.exists(f) {
            f.to_owned()
        } else {
            format!("{f}.tex")
        };
        if is_file {
            uri_of(&Path::new(&base).join(f))
        } else {
            format!("{base}/{}", encode(&f))
        }
    }

    /// The `{textDocument, position}` of `params` as the watch's file,
    /// line and UTF-16 column.
    fn place(&self, params: &Value) -> Option<(String, usize, usize)> {
        let file = self.name(params["textDocument"]["uri"].as_str()?)?;
        let n = |v: &Value| v.as_u64().and_then(|n| usize::try_from(n).ok());
        let line = n(&params["position"]["line"])?;
        let col = n(&params["position"]["character"])?;
        Some((file, line, col))
    }

    /// `phitex/status` with `state` and the last build's facts.
    fn status(&self, state: &'static str) {
        let last = {
            let mut st = lock(&self.st);
            st.state = state;
            st.last
        };
        self.notify(
            "phitex/status",
            json!({
                "state": state,
                "ms": last.ms,
                "pages": last.pages,
                "errors": last.errors,
                "warnings": last.warnings,
                "engine": self.host.engine(),
            }),
        );
    }

    /// A build began.
    pub fn building(&self) {
        let first = {
            let mut st = lock(&self.st);
            st.builds += 1;
            let first = st.builds == 1;
            st.run = Some(Run {
                began: Instant::now(),
                token: None,
                shown: first,
            });
            first
        };
        if first {
            self.begin_progress("Starting");
        }
    }

    /// The build's `$/progress` begun with `message` (where the client
    /// makes tokens).
    fn begin_progress(&self, message: &str) {
        let token = {
            let mut st = lock(&self.st);
            if !st.caps.work_done {
                return;
            }
            let Some(run) = st.run.as_mut() else { return };
            if run.token.is_some() {
                return;
            }
            let t = format!("phitex-build-{}", self.next.fetch_add(1, Ordering::Relaxed));
            run.token = Some(t.clone());
            t
        };
        self.request("window/workDoneProgress/create", json!({"token": token}));
        self.notify(
            "$/progress",
            json!({"token": token, "value": {"kind": "begin", "title": "PhiTeX", "message": message, "cancellable": false}}),
        );
    }

    /// A progress record of the build under way: reported where its
    /// progress is shown, and its phase as `phitex/status` when it
    /// changed.
    pub fn progress(&self, p: &Progress) {
        let (message, percentage, state, changed) = {
            let mut st = lock(&self.st);
            let last_pages = st.last.pages;
            let Some(run) = st.run.as_mut() else { return };
            if !run.shown && run.began.elapsed() >= SHOW_AFTER {
                run.shown = true;
            }
            if !run.shown {
                return;
            }
            let (message, percentage) = match p.phase {
                Phase::Fetching => {
                    let (d, t) = (p.done.unwrap_or(0), p.total.unwrap_or(0));
                    (
                        format!(
                            "Fetching {} ({d} of {t})",
                            p.name
                                .as_deref()
                                .or(p.pack.as_deref())
                                .unwrap_or("a package")
                        ),
                        (t > 0).then(|| d * 100 / t),
                    )
                }
                Phase::Typesetting => {
                    let message = if p.pass > 1 {
                        format!("Typesetting page {} (pass {})", p.pages.max(1), p.pass)
                    } else {
                        format!("Typesetting page {}", p.pages.max(1))
                    };
                    let last = u64::try_from(last_pages).unwrap_or(0);
                    (
                        message,
                        (p.pass == 1 && last > 0).then(|| (p.pages * 100 / last).min(99)),
                    )
                }
            };
            let state = p.phase.name();
            let changed = st.state != state;
            (message, percentage, state, changed)
        };
        self.begin_progress(&message);
        let token = lock(&self.st).run.as_ref().and_then(|r| r.token.clone());
        if let Some(t) = token {
            let mut value = json!({"kind": "report", "message": message});
            if let Some(pc) = percentage {
                value["percentage"] = json!(pc);
            }
            self.notify("$/progress", json!({"token": t, "value": value}));
        }
        if changed {
            self.status(if state == "fetching" {
                "fetching"
            } else {
                "typesetting"
            });
        }
    }

    /// A build is in: its problems published by file (the viewer's items,
    /// `watch::diagnostics`), those of files that had some and no longer
    /// do cleared, the main file's always; its progress ended and
    /// `phitex/status` told.
    pub fn built(&self, b: &Built) {
        let main = lock(&self.st).main.clone();
        let mut by: HashMap<String, Vec<Value>> = HashMap::new();
        by.insert(self.uri(&main), Vec::new());
        let mut facts = Facts {
            ms: b.ms,
            pages: b.hashes.len(),
            ..Facts::default()
        };
        let mut engine_failed = false;
        for d in &b.diagnostics {
            match d["severity"].as_str() {
                Some("error") => facts.errors += 1,
                Some("warning") => facts.warnings += 1,
                _ => {}
            }
            engine_failed |= d["code"] == "engine";
            let file = d["file"].as_str().unwrap_or(&main);
            by.entry(self.uri(file))
                .or_default()
                .push(lsp_diagnostic(d));
        }
        let gone: Vec<String> = {
            let mut st = lock(&self.st);
            let g = st.published.drain().collect();
            for (uri, ds) in &by {
                if !ds.is_empty() {
                    st.published.insert(uri.clone());
                }
            }
            g
        };
        for g in gone {
            by.entry(g).or_default();
        }
        for (uri, ds) in by {
            self.notify(
                "textDocument/publishDiagnostics",
                json!({"uri": uri, "diagnostics": ds}),
            );
        }
        let token = {
            let mut st = lock(&self.st);
            st.last = facts;
            st.run.take().and_then(|r| r.token)
        };
        let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        if let Some(t) = token {
            let message = if engine_failed {
                "The engine failed".to_owned()
            } else if facts.errors > 0 {
                plural(facts.errors, "error")
            } else {
                format!(
                    "Built in {:.0} ms, {}",
                    facts.ms,
                    plural(facts.pages, "page")
                )
            };
            self.notify(
                "$/progress",
                json!({"token": t, "value": {"kind": "end", "message": message}}),
            );
        }
        self.status(if engine_failed || facts.errors > 0 {
            "failed"
        } else {
            "ready"
        });
    }

    /// The editor shows `file` (from the root) at `line` and character
    /// `col` (from 1), the place selected and the focus taken: whether
    /// the client can.
    pub fn show(&self, file: &str, line: usize, col: usize) -> bool {
        let line0 = line.saturating_sub(1);
        // (characters as UTF-16 units, where the editor's buffer is known)
        let mut ch = col.saturating_sub(1);
        {
            let st = lock(&self.st);
            if !st.caps.show_document {
                return false;
            }
            if let Some(d) = st.docs.get(file)
                && let Some(l) = d.lines().nth(line0)
            {
                ch = l.chars().take(ch).map(char::len_utf16).sum();
            }
        }
        let at = json!({"line": line0, "character": ch});
        self.request(
            "window/showDocument",
            json!({"uri": self.uri(file), "takeFocus": true, "selection": {"start": at, "end": at}}),
        );
        true
    }

    /// One message from the editor handled; false: it said to end
    /// (`exit`).
    pub fn handle(&self, m: &Value) -> bool {
        let method = m["method"].as_str().unwrap_or("");
        let id = m.get("id");
        let p = &m["params"];
        match method {
            // (a response to one of ours, or a notification of no interest)
            ""
            | "$/cancelRequest"
            | "$/setTrace"
            | "textDocument/didSave"
            | "workspace/didChangeConfiguration"
            | "workspace/didChangeWatchedFiles" => {}
            "initialize" => {
                self.initialize(m);
                self.answer_init();
            }
            "initialized" => {
                let starting = lock(&self.st).builds == 0;
                if starting {
                    self.status("starting");
                }
            }
            "exit" => return false,
            "shutdown" => {
                if let Some(id) = id {
                    self.reply(id, Value::Null);
                }
            }
            "textDocument/didOpen" => {
                let d = &p["textDocument"];
                if let (Some(uri), Some(text)) = (d["uri"].as_str(), d["text"].as_str())
                    && let Some(file) = self.name(uri)
                {
                    lock(&self.st).docs.insert(file.clone(), text.to_owned());
                    self.host.ask(Ask::SetFile {
                        file,
                        text: text.to_owned(),
                    });
                }
            }
            "textDocument/didChange" => {
                let Some(file) = p["textDocument"]["uri"].as_str().and_then(|u| self.name(u))
                else {
                    return true;
                };
                let n = |v: &Value| {
                    v.as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .unwrap_or(0)
                };
                for c in p["contentChanges"].as_array().into_iter().flatten() {
                    let text = c["text"].as_str().unwrap_or("").to_owned();
                    let mut st = lock(&self.st);
                    let Some(d) = st.docs.get_mut(&file) else {
                        break;
                    };
                    if c.get("range").is_some() {
                        let r = &c["range"];
                        let start = offset(d, n(&r["start"]["line"]), n(&r["start"]["character"]));
                        let end = offset(d, n(&r["end"]["line"]), n(&r["end"]["character"]));
                        *d = crate::watch::splice_utf16(d, start, end, &text);
                        drop(st);
                        self.host.ask(Ask::Edit {
                            file: file.clone(),
                            start,
                            end,
                            text,
                        });
                    } else {
                        d.clone_from(&text);
                        drop(st);
                        self.host.ask(Ask::SetFile {
                            file: file.clone(),
                            text,
                        });
                    }
                }
            }
            "textDocument/didClose" => {
                if let Some(file) = p["textDocument"]["uri"].as_str().and_then(|u| self.name(u))
                    && lock(&self.st).docs.remove(&file).is_some()
                {
                    self.host.ask(Ask::Closed { file });
                }
            }
            "phitex/cursor" => {
                if let Some((file, line, col)) = self.place(p) {
                    self.host.ask(Ask::Cursor { file, line, col });
                }
            }
            "workspace/executeCommand" => {
                if let Some(id) = id {
                    self.command(id, p);
                }
            }
            _ => {
                if let Some(id) = id {
                    self.fail(id, -32601, &format!("no method {method:?} here"));
                }
            }
        }
        true
    }

    /// `workspace/executeCommand` request `id` answered.
    fn command(&self, id: &Value, p: &Value) {
        let name = p["command"].as_str().unwrap_or("");
        let arg = &p["arguments"][0];
        match name {
            "phitex.rebuild" => {
                self.host.ask(Ask::Rebuild);
                self.reply(id, Value::Null);
            }
            "phitex.showOnPage" => match self.place(arg) {
                Some((file, line, col)) => {
                    self.host.ask(Ask::Cursor { file, line, col });
                    self.reply(id, Value::Null);
                }
                None => self.fail(
                    id,
                    -32602,
                    "phitex.showOnPage: {textDocument: {uri}, position} of a file of the project",
                ),
            },
            c => match self.host.command(c, arg) {
                Some(Ok(v)) => self.reply(id, v),
                Some(Err(e)) => self.fail(id, -32603, &e),
                None => self.fail(id, -32601, &format!("no command {c:?} here")),
            },
        }
    }
}

/// A viewer problem (`watch::diagnostics`) as an LSP diagnostic: its
/// line, the culprit's columns where it has an excerpt (counted in UTF-16
/// in the excerpt's line), its help under its message.
fn lsp_diagnostic(d: &Value) -> Value {
    let n = |v: &Value| v.as_u64().and_then(|n| usize::try_from(n).ok());
    let line = n(&d["line"]).unwrap_or(1).saturating_sub(1);
    let (mut a, mut b) = (0, 0);
    if let Some(t) = d["excerpt"]["text"].as_str() {
        let start = n(&d["excerpt"]["start"]).unwrap_or(0);
        let len = n(&d["excerpt"]["len"]).unwrap_or(0);
        let u = |k: usize| -> usize { t.chars().take(k).map(char::len_utf16).sum() };
        (a, b) = (u(start), u(start + len));
    }
    let mut message = d["message"].as_str().unwrap_or("").to_owned();
    for h in d["help"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        message.push('\n');
        message.push_str(h);
    }
    if message.is_empty() {
        "a problem".clone_into(&mut message);
    }
    let severity = match d["severity"].as_str() {
        Some("error") => 1,
        Some("warning") => 2,
        _ => 3,
    };
    json!({
        "range": {"start": {"line": line, "character": a}, "end": {"line": line, "character": b}},
        "severity": severity,
        "code": d["code"],
        "source": "phitex",
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        let p = Path::new("/home/a b/ü.tex");
        let u = uri_of(p);
        assert_eq!(u, "file:///home/a%20b/%C3%BC.tex");
        assert_eq!(path_of(&u).unwrap(), p);
        assert_eq!(
            path_of("file://localhost/x/y.tex").unwrap(),
            Path::new("/x/y.tex")
        );
        assert!(path_of("untitled:1").is_none());
    }

    #[test]
    fn positions() {
        let t = "ab\nc😀d\n\nlast";
        assert_eq!(offset(t, 0, 0), 0);
        assert_eq!(offset(t, 1, 0), 3);
        assert_eq!(offset(t, 1, 3), 6);
        assert_eq!(offset(t, 1, 99), 7);
        assert_eq!(offset(t, 3, 2), 11);
        assert_eq!(offset(t, 9, 0), 13);
    }

    /// A host that keeps what it was asked.
    #[derive(Default)]
    struct Asks(Mutex<Vec<Ask>>);

    impl LspHost for Asks {
        fn ask(&self, a: Ask) {
            lock(&self.0).push(a);
        }
        fn engine(&self) -> &'static str {
            "wasm"
        }
    }

    /// A server on a host that keeps its asks, its messages kept.
    fn server() -> (Server, Arc<Asks>, Arc<Mutex<Vec<Value>>>) {
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s = sent.clone();
        let host = Arc::new(Asks::default());
        let server = Server::new(Box::new(move |v| lock(&s).push(v.clone())), host.clone());
        (server, host, sent)
    }

    fn init(s: &Server, root: &str, work_done: bool) {
        s.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "rootUri": root,
            "capabilities": {"window": {"workDoneProgress": work_done, "showDocument": {"support": true}}},
        }}));
        s.handle(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
    }

    fn methods(sent: &[Value]) -> Vec<String> {
        sent.iter()
            .map(|v| v["method"].as_str().unwrap_or("(reply)").to_owned())
            .collect()
    }

    /// A non-`file:` root (the Overleaf extension's) names files by what
    /// follows it; buffers become the viewer's asks.
    #[test]
    fn a_uri_prefix_root_names_files() {
        let (s, host, sent) = server();
        init(&s, "overleaf://p1/", false);
        assert_eq!(lock(&sent)[0]["id"], 1, "initialize answered");
        assert_eq!(lock(&sent)[1]["params"]["state"], "starting");
        assert_eq!(lock(&sent)[1]["params"]["engine"], "wasm");
        s.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "overleaf://p1/ch%201.tex", "text": "ab\ncd"}}}));
        s.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": {"uri": "overleaf://p1/ch%201.tex"},
            "contentChanges": [{"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}, "text": "X"}]}}));
        s.handle(&json!({"jsonrpc": "2.0", "method": "phitex/cursor", "params": {"textDocument": {"uri": "overleaf://p1/ch%201.tex"}, "position": {"line": 1, "character": 1}}}));
        assert_eq!(
            *lock(&host.0),
            [
                Ask::SetFile {
                    file: "ch 1.tex".into(),
                    text: "ab\ncd".into()
                },
                Ask::Edit {
                    file: "ch 1.tex".into(),
                    start: 3,
                    end: 4,
                    text: "X".into()
                },
                Ask::Cursor {
                    file: "ch 1.tex".into(),
                    line: 1,
                    col: 1
                },
            ]
        );
        assert!(s.owns("ch 1.tex"));
        assert!(s.show("ch 1.tex", 2, 1));
        assert_eq!(
            lock(&sent).last().unwrap()["params"]["uri"],
            "overleaf://p1/ch%201.tex"
        );
    }

    fn record(phase: Phase, pass: u64, pages: u64) -> Progress {
        Progress {
            phase,
            pass,
            pages,
            name: (phase == Phase::Fetching).then(|| "amsmath.sty".to_owned()),
            pack: (phase == Phase::Fetching).then(|| "amsmath-1".to_owned()),
            done: (phase == Phase::Fetching).then_some(1),
            total: (phase == Phase::Fetching).then_some(4),
        }
    }

    fn built(pages: usize, errors: usize) -> Built {
        Built {
            pdf: None,
            hashes: vec![0; pages],
            history: if errors > 0 { 2 } else { 0 },
            ms: 67.4,
            diagnostics: (0..errors)
                .map(|_| json!({"severity": "error", "message": "Undefined control sequence", "line": 3}))
                .collect(),
            since_ms: None,
            origins: None,
            changed: None,
        }
    }

    /// The first build's progress shown from its start, reported from
    /// the records, ended with what it made; a later quick build shows
    /// none, `phitex/status` after it all the same; a slow one shows its
    /// records past 400 ms.
    #[test]
    fn progress_first_build_always_later_ones_past_400_ms() {
        let (s, _, sent) = server();
        init(&s, "file:///p", true);
        s.set_main("main.tex");
        lock(&sent).clear();
        s.building();
        s.progress(&record(Phase::Fetching, 1, 0));
        s.progress(&record(Phase::Typesetting, 1, 2));
        s.built(&built(3, 0));
        let got = lock(&sent).clone();
        assert_eq!(
            methods(&got),
            [
                "window/workDoneProgress/create",
                "$/progress",
                "$/progress",
                "phitex/status",
                "$/progress",
                "phitex/status",
                "textDocument/publishDiagnostics",
                "$/progress",
                "phitex/status",
            ]
        );
        let token = got[0]["params"]["token"].clone();
        assert_eq!(got[1]["params"]["value"]["kind"], "begin");
        assert_eq!(got[1]["params"]["value"]["title"], "PhiTeX");
        assert_eq!(got[1]["params"]["token"], token);
        assert_eq!(
            got[2]["params"]["value"],
            json!({"kind": "report", "message": "Fetching amsmath.sty (1 of 4)", "percentage": 25})
        );
        assert_eq!(got[3]["params"]["state"], "fetching");
        assert_eq!(got[4]["params"]["value"]["message"], "Typesetting page 2");
        assert_eq!(got[5]["params"]["state"], "typesetting");
        assert_eq!(got[6]["params"]["uri"], "file:///p/main.tex");
        assert_eq!(
            got[7]["params"]["value"],
            json!({"kind": "end", "message": "Built in 67 ms, 3 pages"})
        );
        assert_eq!(
            got[8]["params"],
            json!({"state": "ready", "ms": 67.4, "pages": 3, "errors": 0, "warnings": 0, "engine": "wasm"})
        );
        // (a keystroke's build: no progress, the status after it)
        lock(&sent).clear();
        s.building();
        s.progress(&record(Phase::Typesetting, 1, 1));
        s.built(&built(3, 1));
        let got = lock(&sent).clone();
        assert_eq!(
            methods(&got),
            ["textDocument/publishDiagnostics", "phitex/status"]
        );
        assert_eq!(got[1]["params"]["state"], "failed");
        assert_eq!(got[1]["params"]["errors"], 1);
        // (a slow one: its records past 400 ms shown, the percentage of
        // the last build's pages)
        lock(&sent).clear();
        s.building();
        lock(&s.st).run.as_mut().unwrap().began -= SHOW_AFTER;
        s.progress(&record(Phase::Typesetting, 1, 1));
        s.built(&built(1, 2));
        let got = lock(&sent).clone();
        assert_eq!(got[2]["params"]["value"]["percentage"], 33);
        assert_eq!(got.last().unwrap()["params"]["state"], "failed");
        assert!(
            got.iter()
                .any(|v| v["params"]["value"]["message"] == "2 errors"),
            "{got:?}"
        );
    }

    /// Without the client's `workDoneProgress`: no `$/progress`, the
    /// status all the same.
    #[test]
    fn no_progress_without_the_clients_tokens() {
        let (s, _, sent) = server();
        init(&s, "file:///p", false);
        lock(&sent).clear();
        s.building();
        s.progress(&record(Phase::Typesetting, 1, 1));
        s.built(&built(1, 0));
        assert_eq!(
            methods(&lock(&sent)),
            [
                "phitex/status",
                "textDocument/publishDiagnostics",
                "phitex/status"
            ]
        );
    }
}
