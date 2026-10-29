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
//! - **Forward search.** `phitex/cursor {textDocument: {uri}, position,
//!   range?}` (a selection is the cursor with its range) or the command
//!   `phitex.showOnPage` is [`Ask::Cursor`], in bytes of the file's text
//!   (the buffer held here, else the file's), the glyph origins' space:
//!   the viewer shows it from the origins.
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
/// The editor protocol's version (docs/spec-editor-protocol.md §1),
/// answered in `initialize` as `capabilities.experimental.phitex.protocol`:
/// raised when a message the editor sends or reads changes its meaning
/// incompatibly, so an extension can tell an older or newer `phitex lsp`
/// from the one it was written for and say so.
pub const PROTOCOL: u64 = 1;

pub const COMMANDS: [&str; 7] = [
    "phitex.openViewer",
    "phitex.rebuild",
    "phitex.showOnPage",
    "phitex.stop",
    "phitex.savePdf",
    "phitex.clearCache",
    "phitex.cacheStats",
];

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

    /// The text of the project's file `name` (an outline asked of a file
    /// the editor has not opened); none: not readable.
    fn read(&self, _name: &str) -> Option<String> {
        None
    }

    /// The settings changed ([`Settings`]: those the editor gave, the
    /// rest none): the host applies them (a new session where the
    /// threads, the main file or the files' source differ).
    fn configure(&self, _s: &Settings) {}

    /// `phitex/status`'s engine: `native` or `wasm`.
    fn engine(&self) -> &'static str;
}

/// The byte of `text` at LSP position `p` (`{line, character}`, UTF-16
/// from 0): a column past its line's end is its end, a line past the
/// text's the text's end.
fn byte_at(text: &str, p: &Value) -> Option<usize> {
    let n = |v: &Value| v.as_u64().and_then(|n| usize::try_from(n).ok());
    let (line, col) = (n(&p["line"])?, n(&p["character"])?);
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let src = text[start..].split('\n').next().unwrap_or("");
    let mut units = 0;
    Some(
        start
            + src
                .char_indices()
                .find(|&(_, c)| {
                    units += c.len_utf16();
                    units > col
                })
                .map_or(src.len(), |(i, _)| i),
    )
}

/// The word around byte `at` of `text` (bytes; none: not in one), as the
/// viewer's `wordBytes` takes it: letters, digits, `'`, `-` and every
/// byte of a character past ASCII.
fn word_at(text: &[u8], at: usize) -> Option<(usize, usize)> {
    let w = |c: u8| c >= 0x80 || c.is_ascii_alphanumeric() || c == b'\'' || c == b'-';
    let (mut lo, mut hi) = (at.min(text.len()), at.min(text.len()));
    while lo > 0 && w(text[lo - 1]) {
        lo -= 1;
    }
    while hi < text.len() && w(text[hi]) {
        hi += 1;
    }
    (lo < hi).then_some((lo, hi))
}

/// The line of byte `at` of `text`, without its end (bytes).
fn line_at(text: &str, at: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let at = at.min(b.len());
    let lo = b[..at]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |i| i + 1);
    let hi = b[at..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |i| at + i);
    (lo, hi)
}

/// The editor's settings, section `phitex` (docs/spec-editor-protocol.md
/// §3.5): none where it gave none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// The engine's threads (none, 0 or null: not given).
    pub threads: Option<usize>,
    /// The main file, from the root.
    pub main: Option<String>,
    /// When the client sends `phitex/cursor` (`cursor`, `selection`,
    /// `dblclick`): the client's.
    pub follow: Option<String>,
    /// Where TeX's files come from: `texlive` or `shelf`.
    pub files: Option<String>,
}

impl Settings {
    /// The settings of `v` (`{threads, main, follow, files}`), values of
    /// other types ignored.
    pub fn of(v: &Value) -> Settings {
        let text = |k: &str| v[k].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
        Settings {
            threads: v["threads"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n > 0),
            main: text("main"),
            follow: text("follow")
                .filter(|f| ["cursor", "selection", "dblclick"].contains(&f.as_str())),
            files: text("files").filter(|f| f == "texlive" || f == "shelf"),
        }
    }

    /// These settings with `newer`'s given ones over them.
    pub fn with(&self, newer: &Settings) -> Settings {
        Settings {
            threads: newer.threads.or(self.threads),
            main: newer.main.clone().or_else(|| self.main.clone()),
            follow: newer.follow.clone().or_else(|| self.follow.clone()),
            files: newer.files.clone().or_else(|| self.files.clone()),
        }
    }
}

/// What a button of a `window/showMessageRequest` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// A build ([`Ask::Rebuild`]).
    Build,
    /// A cold build ([`Ask::Cold`]).
    Cold,
}

/// What needs the user, said once until it changes
/// ([`Server::trouble`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trouble {
    /// The engine failed (a panic, its stop).
    Engine,
    /// The format could not be had.
    Format,
    /// A package source cannot be reached (Shelf).
    Unreachable,
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
    /// Whether the client answers `workspace/configuration`.
    configuration: bool,
    /// The settings given (`initializationOptions`, then the client's).
    settings: Settings,
    /// The server's requests still unanswered that want their answer:
    /// `workspace/configuration`'s (none) or a message's buttons.
    pending: HashMap<String, Option<Vec<(String, Action)>>>,
    /// What was last said of each trouble (said again only when it
    /// changes).
    said: HashMap<Trouble, String>,
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
        st.configuration = p["capabilities"]["workspace"]["configuration"]
            .as_bool()
            .unwrap_or(false);
        st.settings = Settings::of(&p["initializationOptions"]);
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
                    "documentSymbolProvider": true,
                    "experimental": {"phitex": {"protocol": PROTOCOL}},
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
    /// The cursor `{textDocument, position, range?}` of `params` as
    /// [`Ask::Cursor`]: its bytes in the file's text, the buffer held
    /// here (else the file's, the host's), which the glyph origins' offsets
    /// are in.
    fn cursor(&self, params: &Value) -> Option<Ask> {
        let file = self.name(params["textDocument"]["uri"].as_str()?)?;
        let text = lock(&self.st)
            .docs
            .get(&file)
            .cloned()
            .or_else(|| self.host.read(&file))?;
        let at = byte_at(&text, &params["position"])?;
        let select = match &params["range"] {
            Value::Null => None,
            r => Some((byte_at(&text, &r["start"])?, byte_at(&text, &r["end"])?)),
        }
        .map(|(a, b)| (a.min(b), a.max(b)));
        Some(Ask::Cursor {
            word: word_at(text.as_bytes(), at),
            line: line_at(&text, at),
            file,
            at,
            select,
        })
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
            self.starting();
            self.begin_progress("Starting");
        }
    }

    /// `phitex/status {state: "starting"}`, the first status, sent once:
    /// at `initialized` or at the first build's start, whichever comes
    /// first (the watch may begin its first build before the client says
    /// `initialized`); never after another state.
    fn starting(&self) {
        if lock(&self.st).state.is_empty() {
            self.status("starting");
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
        let (mut engine, mut format) = (None, None);
        for d in &b.diagnostics {
            match d["severity"].as_str() {
                Some("error") => facts.errors += 1,
                Some("warning") => facts.warnings += 1,
                _ => {}
            }
            let message = d["message"].as_str().unwrap_or("").to_owned();
            if d["code"] == "engine" {
                engine_failed = true;
                engine.get_or_insert(message);
            } else if d["code"] == "format" {
                format.get_or_insert(message);
            }
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
        for (t, m) in [(Trouble::Engine, engine), (Trouble::Format, format)] {
            match m {
                Some(m) => self.trouble(t, &m),
                None => self.trouble_over(t),
            }
        }
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

    /// The settings as given so far (`initializationOptions`, then the
    /// client's).
    pub fn settings(&self) -> Settings {
        lock(&self.st).settings.clone()
    }

    /// `workspace/configuration` asked for section `phitex`, where the
    /// client answers it.
    fn ask_configuration(&self) {
        if !lock(&self.st).configuration {
            return;
        }
        let id = self.request(
            "workspace/configuration",
            json!({"items": [{"section": "phitex"}]}),
        );
        lock(&self.st).pending.insert(id, None);
    }

    /// The client's settings `given`, over those so far: the host told
    /// when they changed.
    fn configure(&self, given: &Settings) {
        let now = {
            let mut st = lock(&self.st);
            let now = st.settings.with(given);
            if now == st.settings {
                return;
            }
            st.settings = now.clone();
            now
        };
        self.host.configure(&now);
    }

    /// The client's answer `result` to our request `id`.
    fn answered(&self, id: &str, result: &Value) {
        let Some(what) = lock(&self.st).pending.remove(id) else {
            return;
        };
        match what {
            // (workspace/configuration's: one item, ours)
            None => {
                if result[0].is_object() {
                    self.configure(&Settings::of(&result[0]));
                }
            }
            Some(buttons) => {
                let title = result["title"].as_str().unwrap_or("");
                match buttons.iter().find(|(t, _)| t == title).map(|b| b.1) {
                    Some(Action::Build) => self.host.ask(Ask::Rebuild),
                    Some(Action::Cold) => self.host.ask(Ask::Cold),
                    None => {}
                }
            }
        }
    }

    /// `trouble` said to the user (`window/showMessageRequest`, with what
    /// helps as its buttons), unless the same was said last.
    pub fn trouble(&self, trouble: Trouble, message: &str) {
        let (kind, text, buttons) = match trouble {
            Trouble::Engine => (
                1,
                format!("PhiTeX: the engine failed: {message}"),
                vec![("Rebuild".to_owned(), Action::Cold)],
            ),
            Trouble::Format => (
                1,
                format!("PhiTeX: the format did not build: {message}"),
                vec![("Rebuild".to_owned(), Action::Cold)],
            ),
            Trouble::Unreachable => (
                2,
                format!("PhiTeX: a package could not be had: {message}"),
                vec![("Retry".to_owned(), Action::Build)],
            ),
        };
        {
            let mut st = lock(&self.st);
            if st.said.get(&trouble) == Some(&text) {
                return;
            }
            st.said.insert(trouble, text.clone());
        }
        let actions: Vec<Value> = buttons.iter().map(|(t, _)| json!({"title": t})).collect();
        let id = self.request(
            "window/showMessageRequest",
            json!({"type": kind, "message": text, "actions": actions}),
        );
        lock(&self.st).pending.insert(id, Some(buttons));
    }

    /// `trouble` is over (a build went well): said again if it comes back.
    fn trouble_over(&self, trouble: Trouble) {
        lock(&self.st).said.remove(&trouble);
    }

    /// One message from the editor handled; false: it said to end
    /// (`exit`).
    pub fn handle(&self, m: &Value) -> bool {
        let method = m["method"].as_str().unwrap_or("");
        let id = m.get("id");
        let p = &m["params"];
        match method {
            // (a response to one of ours)
            "" => {
                if let Some(id) = id.and_then(Value::as_str) {
                    self.answered(id, &m["result"]);
                }
            }
            // (notifications of no interest)
            "$/cancelRequest"
            | "$/setTrace"
            | "textDocument/didSave"
            | "workspace/didChangeWatchedFiles" => {}
            "workspace/didChangeConfiguration" => {
                let given = &p["settings"]["phitex"];
                if given.is_object() {
                    self.configure(&Settings::of(given));
                } else {
                    self.ask_configuration();
                }
            }
            "textDocument/documentSymbol" => {
                if let Some(id) = id {
                    let text = p["textDocument"]["uri"]
                        .as_str()
                        .and_then(|u| self.name(u))
                        .and_then(|f| {
                            let open = lock(&self.st).docs.get(&f).cloned();
                            open.or_else(|| self.host.read(&f))
                        });
                    match text {
                        Some(t) => self.reply(id, Value::Array(outline(&t))),
                        None => self.reply(id, Value::Null),
                    }
                }
            }
            "initialize" => {
                self.initialize(m);
                self.answer_init();
            }
            "initialized" => {
                self.starting();
                self.ask_configuration();
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
                if let Some(a) = self.cursor(p) {
                    self.host.ask(a);
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
                self.host.ask(Ask::Cold);
                self.reply(id, Value::Null);
            }
            // (said before the host ends: a client tells a stop from a
            // crash by it)
            "phitex.stop" => {
                self.reply(id, Value::Null);
                self.status("stopped");
                self.host.command(name, arg);
            }
            "phitex.showOnPage" => match self.cursor(arg) {
                Some(a) => {
                    self.host.ask(a);
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

/// The sectioning commands, their levels and symbol kinds
/// (`Namespace` 3 for parts and chapters, `Module` 2 below).
const HEADINGS: [(&str, usize, u8); 7] = [
    ("part", 0, 3),
    ("chapter", 1, 3),
    ("section", 2, 2),
    ("subsection", 3, 2),
    ("subsubsection", 4, 2),
    ("paragraph", 5, 2),
    ("subparagraph", 6, 2),
];

/// What the outline finds in a text: a heading (its level, kind, title)
/// or a label, from byte `start` to `end` (the command and its argument).
struct Found {
    heading: Option<(usize, u8)>,
    name: String,
    start: usize,
    end: usize,
}

/// The braced argument at byte `at` of `t` (spaces before it passed
/// over): its text and the byte after it.
fn braced(t: &[u8], mut at: usize) -> Option<(String, usize)> {
    while at < t.len() && (t[at] == b' ' || t[at] == b'\t' || t[at] == b'\n') {
        at += 1;
    }
    if t.get(at) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let from = at + 1;
    while at < t.len() {
        match t[at] {
            b'\\' => at += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let inner = String::from_utf8_lossy(&t[from..at]);
                    return Some((
                        inner.split_whitespace().collect::<Vec<_>>().join(" "),
                        at + 1,
                    ));
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The bracketed optional argument at byte `at` of `t`, passed over: the
/// byte after it (`at` if there is none).
fn past_optional(t: &[u8], at: usize) -> usize {
    if t.get(at) != Some(&b'[') {
        return at;
    }
    let mut depth = 0usize;
    let mut i = at;
    while i < t.len() {
        match t[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b'{' => {
                if let Some((_, after)) = braced(t, i) {
                    i = after;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    at
}

/// A text's headings and labels in order, its comments passed over.
fn found(text: &str) -> Vec<Found> {
    let t = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'%' => {
                while i < t.len() && t[i] != b'\n' {
                    i += 1;
                }
            }
            b'\\' => {
                let from = i;
                let mut j = i + 1;
                while j < t.len() && t[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word = &text[i + 1..j];
                if word.is_empty() {
                    // (a control symbol: `\%`, `\\`: passed over whole)
                    i = j + 1;
                    continue;
                }
                let heading = HEADINGS.iter().find(|h| h.0 == word);
                if heading.is_some() || word == "label" {
                    let mut k = j;
                    if heading.is_some() && t.get(k) == Some(&b'*') {
                        k += 1;
                    }
                    let k = if heading.is_some() {
                        past_optional(t, k)
                    } else {
                        k
                    };
                    if let Some((name, end)) = braced(t, k) {
                        out.push(Found {
                            heading: heading.map(|h| (h.1, h.2)),
                            name: if name.is_empty() {
                                format!("\\{word}")
                            } else {
                                name
                            },
                            start: from,
                            end,
                        });
                        i = end;
                        continue;
                    }
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

/// A text's outline as LSP `DocumentSymbol`s (docs/spec-editor-protocol.md
/// §3.4): its headings, each holding the headings below it and its
/// labels, its range to the next heading of its level or above.
pub fn outline(text: &str) -> Vec<Value> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(k, _)| k + 1))
        .collect();
    let pos = |b: usize| {
        let line = starts.partition_point(|&s| s <= b) - 1;
        let col: usize = text[starts[line]..b].encode_utf16().count();
        json!({"line": line, "character": col})
    };
    let symbol = |f: &Found, end: usize| {
        let kind = f.heading.map_or(14, |h| h.1);
        json!({
            "name": f.name,
            "kind": kind,
            "range": {"start": pos(f.start), "end": pos(end)},
            "selectionRange": {"start": pos(f.start), "end": pos(f.end)},
            "children": [],
        })
    };
    // (the headings open, innermost last: each with its level and symbol)
    let mut open: Vec<(usize, Value)> = Vec::new();
    let mut top: Vec<Value> = Vec::new();
    let close = |open: &mut Vec<(usize, Value)>, top: &mut Vec<Value>, end: usize| {
        if let Some((_, mut v)) = open.pop() {
            v["range"]["end"] = pos(end);
            match open.last_mut() {
                Some((_, parent)) => {
                    if let Some(c) = parent["children"].as_array_mut() {
                        c.push(v);
                    }
                }
                None => top.push(v),
            }
        }
    };
    for f in found(text) {
        let v = symbol(&f, f.end);
        if let Some((level, _)) = f.heading {
            while open.last().is_some_and(|o| o.0 >= level) {
                close(&mut open, &mut top, f.start);
            }
            open.push((level, v));
        } else if let Some((_, parent)) = open.last_mut() {
            if let Some(c) = parent["children"].as_array_mut() {
                c.push(v);
            }
        } else {
            top.push(v);
        }
    }
    while !open.is_empty() {
        close(&mut open, &mut top, text.len());
    }
    top
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
                    at: 4,
                    word: Some((3, 5)),
                    line: (3, 5),
                    select: None,
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

    /// The cursor's and the selection's places in bytes of the buffer
    /// (UTF-16 columns past a character outside ASCII, a selection made
    /// backwards, a column past its line's end).
    #[test]
    fn cursor_places_in_bytes() {
        let t = "\u{e9}t\u{e9} d'or-x\n\u{1f600}ab\n";
        let pos = |line: u64, character: u64| json!({"line": line, "character": character});
        assert_eq!(byte_at(t, &pos(0, 0)), Some(0));
        assert_eq!(byte_at(t, &pos(0, 2)), Some(3));
        assert_eq!(byte_at(t, &pos(0, 99)), Some(12));
        assert_eq!(byte_at(t, &pos(1, 2)), Some(17));
        assert_eq!(byte_at(t, &pos(9, 0)), Some(t.len()));
        assert_eq!(word_at(t.as_bytes(), 3), Some((0, 5)));
        assert_eq!(word_at(t.as_bytes(), 7), Some((6, 12)));
        assert_eq!(word_at(t.as_bytes(), 5), Some((0, 5)));
        assert_eq!(word_at(b"a  b", 2), None);
        assert_eq!(line_at(t, 16), (13, 19));
        let (s, host, _) = server();
        init(&s, "file:///p", false);
        s.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///p/a.tex", "text": t}}}));
        lock(&host.0).clear();
        s.handle(
            &json!({"jsonrpc": "2.0", "method": "phitex/cursor", "params": {
            "textDocument": {"uri": "file:///p/a.tex"}, "position": pos(1, 3),
            "range": {"start": pos(1, 3), "end": pos(0, 4)}}}),
        );
        assert_eq!(
            *lock(&host.0),
            [Ask::Cursor {
                file: "a.tex".into(),
                at: 18,
                word: Some((13, 19)),
                line: (13, 19),
                select: Some((6, 18)),
            }]
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

    /// The outline of a text: parts, sections and below nested by level,
    /// labels under their heading, starred and short-titled headings,
    /// comments passed over, ranges to the next heading at or above the
    /// level, positions in UTF-16.
    #[test]
    fn outline_nests_headings_and_labels() {
        let t = "\\label{top}\n\\section{Intro}\\label{s:intro}\n% \\section{Gone}\n\\subsection*[short]{A {nested} title}\ntext 😀\n\\section{Two}\n\\paragraph{P}\n";
        let o = outline(t);
        let names = |v: &Value| v["name"].as_str().unwrap_or("").to_owned();
        assert_eq!(
            o.iter().map(names).collect::<Vec<_>>(),
            ["top", "Intro", "Two"]
        );
        assert_eq!(o[0]["kind"], 14);
        let intro = &o[1];
        assert_eq!(intro["kind"], 2);
        assert_eq!(
            intro["children"]
                .as_array()
                .unwrap()
                .iter()
                .map(names)
                .collect::<Vec<_>>(),
            ["s:intro", "A {nested} title"]
        );
        assert_eq!(intro["range"]["start"], json!({"line": 1, "character": 0}));
        assert_eq!(intro["range"]["end"], json!({"line": 5, "character": 0}));
        assert_eq!(
            intro["selectionRange"]["end"],
            json!({"line": 1, "character": 15})
        );
        let sub = &intro["children"][1];
        assert_eq!(sub["range"]["end"], json!({"line": 5, "character": 0}));
        let two = &o[2];
        assert_eq!(two["children"][0]["name"], "P");
        assert_eq!(two["range"]["end"], json!({"line": 7, "character": 0}));
        assert_eq!(outline("no headings at all"), Vec::<Value>::new());
    }

    /// `textDocument/documentSymbol` answers the open buffer's outline
    /// (the capability advertised).
    #[test]
    fn document_symbols_of_the_buffer() {
        let (s, _, sent) = server();
        init(&s, "file:///p", false);
        assert_eq!(
            lock(&sent)[0]["result"]["capabilities"]["documentSymbolProvider"],
            true
        );
        assert_eq!(
            lock(&sent)[0]["result"]["capabilities"]["experimental"]["phitex"]["protocol"],
            PROTOCOL
        );
        s.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": "file:///p/a.tex", "text": "\\chapter{One}\n\\section{Sub}\n"}}}));
        s.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "textDocument/documentSymbol", "params": {"textDocument": {"uri": "file:///p/a.tex"}}}));
        let r = lock(&sent).last().unwrap().clone();
        assert_eq!(r["id"], 4);
        assert_eq!(r["result"][0]["name"], "One");
        assert_eq!(r["result"][0]["kind"], 3);
        assert_eq!(r["result"][0]["children"][0]["name"], "Sub");
    }

    /// A host that runs every command, keeping them, and its settings.
    #[derive(Default)]
    struct Runs {
        asks: Mutex<Vec<Ask>>,
        ran: Mutex<Vec<(String, Value)>>,
        settings: Mutex<Vec<Settings>>,
    }

    impl LspHost for Runs {
        fn ask(&self, a: Ask) {
            lock(&self.asks).push(a);
        }
        fn command(&self, name: &str, arg: &Value) -> Option<Result<Value, String>> {
            lock(&self.ran).push((name.to_owned(), arg.clone()));
            match name {
                "phitex.clearCache" => Some(Ok(json!({"freed_bytes": 12}))),
                "phitex.stop" => Some(Ok(Value::Null)),
                _ => None,
            }
        }
        fn configure(&self, s: &Settings) {
            lock(&self.settings).push(s.clone());
        }
        fn engine(&self) -> &'static str {
            "native"
        }
    }

    fn running() -> (Server, Arc<Runs>, Arc<Mutex<Vec<Value>>>) {
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s = sent.clone();
        let host = Arc::new(Runs::default());
        let server = Server::new(Box::new(move |v| lock(&s).push(v.clone())), host.clone());
        (server, host, sent)
    }

    fn execute(s: &Server, id: u64, command: &str, args: Value) {
        s.handle(
            &json!({"jsonrpc": "2.0", "id": id, "method": "workspace/executeCommand",
            "params": {"command": command, "arguments": args}}),
        );
    }

    /// The commands: every one advertised; rebuild a cold build; stop
    /// answered and `stopped` said before the host ends; the host's
    /// answered with its result; one it has not, `-32601`.
    #[test]
    fn commands() {
        let (s, host, sent) = running();
        init(&s, "file:///p", false);
        let caps =
            lock(&sent)[0]["result"]["capabilities"]["executeCommandProvider"]["commands"].clone();
        for c in [
            "phitex.stop",
            "phitex.rebuild",
            "phitex.savePdf",
            "phitex.clearCache",
            "phitex.cacheStats",
            "phitex.openViewer",
            "phitex.showOnPage",
        ] {
            assert!(caps.as_array().unwrap().contains(&json!(c)), "{c}: {caps}");
        }
        lock(&sent).clear();
        execute(&s, 2, "phitex.rebuild", json!([]));
        assert_eq!(*lock(&host.asks), [Ask::Cold]);
        execute(&s, 3, "phitex.clearCache", json!([{"scope": "packages"}]));
        assert_eq!(
            lock(&sent).last().unwrap()["result"],
            json!({"freed_bytes": 12})
        );
        assert_eq!(
            lock(&host.ran)[0],
            ("phitex.clearCache".to_owned(), json!({"scope": "packages"}))
        );
        execute(&s, 4, "phitex.cacheStats", json!([]));
        assert_eq!(lock(&sent).last().unwrap()["error"]["code"], -32601);
        lock(&sent).clear();
        execute(&s, 5, "phitex.stop", json!([]));
        let got = lock(&sent).clone();
        assert_eq!(got[0]["id"], 5);
        assert_eq!(got[1]["params"]["state"], "stopped");
        assert_eq!(lock(&host.ran).last().unwrap().0, "phitex.stop");
    }

    /// The settings: `initializationOptions` first, the client's answer to
    /// `workspace/configuration` over them (the host told what changed),
    /// a change notification with settings applied at once, one without
    /// asked again.
    #[test]
    fn settings_from_the_client() {
        let (s, host, sent) = running();
        s.handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "rootUri": "file:///p",
                "capabilities": {"workspace": {"configuration": true}},
                "initializationOptions": {"main": "paper.tex", "threads": 4},
            }}),
        );
        assert_eq!(s.settings().main.as_deref(), Some("paper.tex"));
        assert_eq!(s.settings().threads, Some(4));
        s.handle(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        let ask = lock(&sent)
            .iter()
            .find(|m| m["method"] == "workspace/configuration")
            .cloned()
            .expect("asked");
        assert_eq!(ask["params"]["items"][0]["section"], "phitex");
        s.handle(&json!({"jsonrpc": "2.0", "id": ask["id"], "result": [{"threads": null, "main": null, "follow": "selection", "files": "shelf"}]}));
        let told = lock(&host.settings).clone();
        assert_eq!(told.len(), 1);
        assert_eq!(
            told[0],
            Settings {
                threads: Some(4),
                main: Some("paper.tex".into()),
                follow: Some("selection".into()),
                files: Some("shelf".into())
            }
        );
        s.handle(&json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {"phitex": {"threads": 2}}}}));
        assert_eq!(lock(&host.settings).last().unwrap().threads, Some(2));
        // (the same again: nothing told)
        s.handle(&json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": {"phitex": {"threads": 2}}}}));
        assert_eq!(lock(&host.settings).len(), 2);
        lock(&sent).clear();
        s.handle(&json!({"jsonrpc": "2.0", "method": "workspace/didChangeConfiguration", "params": {"settings": null}}));
        assert_eq!(lock(&sent)[0]["method"], "workspace/configuration");
    }

    /// A trouble said once until it changes, with its buttons; the
    /// button's answer acted on; an engine failure is one, and said again
    /// after a good build.
    #[test]
    fn troubles_said_once_with_their_actions() {
        let (s, host, sent) = running();
        init(&s, "file:///p", false);
        lock(&sent).clear();
        s.trouble(Trouble::Unreachable, "dns");
        s.trouble(Trouble::Unreachable, "dns");
        let got = lock(&sent).clone();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["method"], "window/showMessageRequest");
        assert_eq!(got[0]["params"]["type"], 2);
        assert_eq!(got[0]["params"]["actions"], json!([{"title": "Retry"}]));
        s.handle(&json!({"jsonrpc": "2.0", "id": got[0]["id"], "result": {"title": "Retry"}}));
        assert_eq!(*lock(&host.asks), [Ask::Rebuild]);
        let mut b = built(0, 1);
        b.diagnostics = vec![
            json!({"severity": "error", "code": "engine", "message": "it ran past its time limit"}),
        ];
        lock(&sent).clear();
        s.building();
        s.built(&b);
        s.built(&b);
        let asks: Vec<Value> = lock(&sent)
            .iter()
            .filter(|m| m["method"] == "window/showMessageRequest")
            .cloned()
            .collect();
        assert_eq!(asks.len(), 1, "once");
        assert_eq!(
            asks[0]["params"]["message"],
            "PhiTeX: the engine failed: it ran past its time limit"
        );
        s.handle(&json!({"jsonrpc": "2.0", "id": asks[0]["id"], "result": {"title": "Rebuild"}}));
        assert_eq!(lock(&host.asks).last(), Some(&Ask::Cold));
        s.built(&built(1, 0));
        lock(&sent).clear();
        s.built(&b);
        assert!(
            lock(&sent)
                .iter()
                .any(|m| m["method"] == "window/showMessageRequest"),
            "said again"
        );
    }
}
