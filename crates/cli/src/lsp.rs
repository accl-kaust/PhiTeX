//! `phitex watch --lsp`: the watch as a language server, for any editor
//! with an LSP client (Neovim, Emacs's eglot and lsp-mode, Helix, Zed, VS
//! Code). The same session as `phitex watch`, its standard input and
//! output speaking the Language Server Protocol (JSON-RPC 2.0, each
//! message after a `Content-Length` header) instead of keys and a status
//! line; nothing else is written to standard output. The terminal's
//! messages go to the viewer's console, as always, and to the editor as
//! `window/logMessage`; the viewer still runs, in the browser.
//!
//! - **Start.** `initialize` comes first ([`Lsp::begin`]): its root
//!   (`rootUri`, else the first of `workspaceFolders`) is where the watch
//!   runs; the main file is the watch's argument, else `phitex.toml`'s,
//!   else `initializationOptions.main`, else the one `.tex` file at the
//!   root with a `\documentclass` (`main.tex` among several). Answered
//!   ([`Lsp::accept`]) with: text synchronised incrementally, positions
//!   in UTF-16 (the viewer's `edit` counts in it already), and the
//!   commands `phitex.openViewer`, `phitex.rebuild` and
//!   `phitex.showOnPage`. The viewer's address is logged.
//! - **Buffers.** `didOpen`, `didChange` (ranged or whole) and `didClose`
//!   are what the viewer's `set_file` and `edit` are ([`Ask::SetFile`],
//!   [`Ask::Edit`], and [`Ask::Closed`]: the file as on disk again),
//!   asked of the watch the way the viewer asks: while a file is open in
//!   the editor its buffer owns it ([`Lsp::owns`]), the disk's changes of
//!   it are not read until it is closed. `didSave` brings nothing the
//!   buffer did not.
//! - **After a build** ([`Lsp::built`]): `textDocument/publishDiagnostics`
//!   for each file with problems (the viewer's problems list: errors,
//!   warnings, the engine's diagnostics; one without a place on the main
//!   file's first line), an empty list for the files that had some and no
//!   longer do, and for the main file always (the editor knows a build
//!   is in). While a build runs, `$/progress` with the watch's phase
//!   (a token made by `window/workDoneProgress/create`, where the client
//!   allows it).
//! - **Inverse search.** The viewer's `source` (a double-click, an
//!   error's place) is `window/showDocument` with the place selected and
//!   the focus taken ([`Lsp::show`]); the `--editor` command where the
//!   client cannot show documents.
//! - **Forward search.** The client's notification `phitex/cursor
//!   {textDocument: {uri}, position}` (sent as the cursor moves, debounced
//!   by the client), or the command `phitex.showOnPage` with the same
//!   argument, is [`Ask::Cursor`]: the watch finds the place on the pages
//!   and the viewer reveals it (`view.rs`'s `reveal` event).
//! - **End.** `shutdown` is answered; `exit`, or the end of standard
//!   input, ends the watch as `q` does.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

use crate::view::{Ask, Built};

/// The commands the server runs (`workspace/executeCommand`).
pub const COMMANDS: [&str; 3] = ["phitex.openViewer", "phitex.rebuild", "phitex.showOnPage"];

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

/// `p` as a `file:` URI (its bytes past the unreserved ones and `/`
/// percent-encoded).
pub fn uri_of(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(char::from(b));
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// The path of a `file:` URI (none: another scheme).
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let b = rest.as_bytes();
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
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
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

/// The words around UTF-16 column `col` of source line `line`, as the
/// page would show them, longest first: what forward search looks for on
/// the pages until the engine records origins. The line is cut at its
/// comment; control sequences, math and TeX's specials break it into
/// pieces (their text is not the page's), braces go; in the piece at the
/// cursor (else the nearest), windows of 6 to 2 words holding the word at
/// the cursor, centred first, then that word alone if it is long enough
/// to be telling.
pub fn needles(line: &str, col: usize) -> Vec<String> {
    // (the cursor as a char index)
    let mut at = line.chars().count();
    let mut u = 0;
    for (k, c) in line.chars().enumerate() {
        if u >= col {
            at = k;
            break;
        }
        u += c.len_utf16();
    }
    // (the pieces: (first char index, words with their char indices))
    let cs: Vec<char> = line.chars().collect();
    let mut pieces: Vec<Vec<(usize, String)>> = vec![Vec::new()];
    let mut word = String::new();
    let mut word_at = 0;
    let mut math = false;
    let end_word = |pieces: &mut Vec<Vec<(usize, String)>>, word: &mut String, word_at: usize| {
        if !word.is_empty() {
            pieces
                .last_mut()
                .expect("a piece")
                .push((word_at, std::mem::take(word)));
        }
    };
    let mut k = 0;
    while k < cs.len() {
        let c = cs[k];
        if c == '%' {
            break;
        }
        let brk = |pieces: &mut Vec<Vec<(usize, String)>>| {
            if pieces.last().is_some_and(|p| !p.is_empty()) {
                pieces.push(Vec::new());
            }
        };
        if c == '\\' {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
            k += 1;
            // (a control word and its star, or one control symbol)
            if cs.get(k).is_some_and(char::is_ascii_alphabetic) {
                while cs
                    .get(k)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == '@')
                {
                    k += 1;
                }
                if cs.get(k) == Some(&'*') {
                    k += 1;
                }
            } else {
                k += 1;
            }
            continue;
        }
        if c == '$' {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
            math = !math;
        } else if math || "&#^_[]".contains(c) {
            end_word(&mut pieces, &mut word, word_at);
            brk(&mut pieces);
        } else if c.is_whitespace() || c == '~' {
            end_word(&mut pieces, &mut word, word_at);
        } else if c != '{' && c != '}' {
            if word.is_empty() {
                word_at = k;
            }
            word.push(c);
        }
        k += 1;
    }
    end_word(&mut pieces, &mut word, word_at);
    pieces.retain(|p| !p.is_empty());
    // (the piece at the cursor: the one whose word starts last at or
    // before it, else the first)
    let Some(piece) = pieces
        .iter()
        .rev()
        .find(|p| p[0].0 <= at)
        .or_else(|| pieces.first())
    else {
        return Vec::new();
    };
    let w = piece.iter().rposition(|(s, _)| *s <= at).unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for n in (2..=6).rev().filter(|&n| n <= piece.len()) {
        // (the windows of n words holding word w, centred first)
        let lo = w.saturating_sub(n - 1);
        let hi = w.min(piece.len() - n);
        let mut starts: Vec<usize> = (lo..=hi).collect();
        let mid = w.saturating_sub(n / 2).clamp(lo, hi);
        starts.sort_by_key(|&s| s.abs_diff(mid));
        for s in starts {
            let t = piece[s..s + n]
                .iter()
                .map(|(_, w)| w.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    let alone = &piece[w].1;
    if alone.chars().count() >= 5 && !out.contains(alone) {
        out.push(alone.clone());
    }
    out
}

struct Shared {
    out: Mutex<std::io::Stdout>,
    /// The watch's directory (where its file names start) and main file.
    root: Mutex<(PathBuf, String)>,
    /// What the client said it can do: show documents, make progress
    /// tokens.
    show_document: bool,
    work_done: bool,
    /// The documents the editor has open, by the watch's name: their
    /// texts, kept as the editor changes them (its positions are in them).
    docs: Mutex<HashMap<String, String>>,
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// The URIs of the last diagnostics published, not empty.
    published: Mutex<HashSet<String>>,
    /// The token of the progress shown now, if one is.
    progress: Mutex<Option<String>>,
    viewer: Mutex<Option<String>>,
    next: AtomicU64,
}

impl Shared {
    fn send(&self, v: &Value) {
        let body = v.to_string();
        let mut o = lock(&self.out);
        let _ = write!(o, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = o.flush();
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&self, method: &str, params: Value) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({"jsonrpc": "2.0", "id": format!("phitex-{id}"), "method": method, "params": params}));
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn fail(&self, id: &Value, code: i32, message: &str) {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        );
    }

    fn log(&self, kind: u8, text: &str) {
        self.notify("window/logMessage", json!({"type": kind, "message": text}));
    }

    fn ask(&self, a: Ask) {
        lock(&self.asks).push(a);
        if let Some(w) = lock(&self.wake).as_ref() {
            w();
        }
    }

    /// The watch's name of the file at `uri` (from its directory), if it
    /// is in it.
    fn name(&self, uri: &str) -> Option<String> {
        let p = path_of(uri)?;
        let root = lock(&self.root).0.clone();
        let rel = p
            .strip_prefix(&root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| {
                let c = p.canonicalize().ok()?;
                c.strip_prefix(&root).ok().map(Path::to_path_buf)
            })?;
        let n = rel.to_string_lossy().into_owned();
        (!n.is_empty() && !n.starts_with('.') && !n.contains("..")).then_some(n)
    }

    /// The URI of the file a diagnostic or the viewer names (from the
    /// watch's directory, or absolute; `name.tex` for a `name` that is
    /// not there).
    fn uri(&self, file: &str) -> String {
        let root = lock(&self.root).0.clone();
        let f = file.trim_start_matches("./");
        let p = root.join(f);
        let p = if p.exists() || Path::new(f).extension().is_some() {
            p
        } else {
            root.join(format!("{f}.tex"))
        };
        uri_of(&p)
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
}

/// The watch's language server.
pub struct Lsp {
    shared: Arc<Shared>,
    /// `initialize`'s id, until answered.
    init: Mutex<Option<Value>>,
    /// Standard input, as `initialize` left it (what came after it may
    /// be read already), until served.
    input: Mutex<Option<BufReader<std::io::Stdin>>>,
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
        let mut input = BufReader::new(std::io::stdin());
        let shared = |show_document, work_done| Shared {
            out: Mutex::new(std::io::stdout()),
            root: Mutex::new((PathBuf::new(), String::new())),
            show_document,
            work_done,
            docs: Mutex::default(),
            asks: Mutex::default(),
            wake: Mutex::new(None),
            published: Mutex::default(),
            progress: Mutex::new(None),
            viewer: Mutex::new(None),
            next: AtomicU64::new(1),
        };
        loop {
            let Some(m) = read(&mut input) else {
                std::process::exit(crate::EXIT_FAILED);
            };
            let method = m["method"].as_str().unwrap_or("");
            if method == "exit" {
                std::process::exit(crate::EXIT_FAILED);
            }
            if method != "initialize" {
                if m.get("id").is_some() {
                    shared(false, false).fail(&m["id"], -32002, "not initialized");
                }
                continue;
            }
            let p = &m["params"];
            let caps = &p["capabilities"]["window"];
            let s = shared(
                caps["showDocument"]["support"].as_bool().unwrap_or(false),
                caps["workDoneProgress"].as_bool().unwrap_or(false),
            );
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
                s.fail(
                    &m["id"],
                    -32603,
                    &format!("can't enter {}: {e}", r.display()),
                );
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
                shared: Arc::new(s),
                init: Mutex::new(Some(m["id"].clone())),
                input: Mutex::new(Some(input)),
            };
        }
    }

    /// The watch cannot run: `initialize` answered with why, or, answered
    /// already, the editor shown it; the watch ends.
    pub fn refuse(&self, e: &str) -> ! {
        let e = format!("phitex watch: {e}");
        match lock(&self.init).take() {
            Some(id) => self.shared.fail(&id, -32603, &e),
            None => self
                .shared
                .notify("window/showMessage", json!({"type": 1, "message": e})),
        }
        std::process::exit(crate::EXIT_FAILED);
    }

    /// `initialize` answered: the watch runs in `root` on `main`.
    pub fn accept(&self, root: &Path, main: &str) {
        *lock(&self.shared.root) = (
            root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            main.to_owned(),
        );
        if let Some(id) = lock(&self.init).take() {
            self.shared.reply(
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
    }

    /// Call `wake` when the editor asks the watch for something.
    pub fn on_ask(&self, wake: Box<dyn Fn() + Send>) {
        *lock(&self.shared.wake) = Some(wake);
    }

    /// What the editor asked for since last taken.
    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut *lock(&self.shared.asks))
    }

    /// Whether file `name` is open in the editor (its buffer owns it: the
    /// disk's changes of it are not read).
    pub fn owns(&self, name: &str) -> bool {
        lock(&self.shared.docs).contains_key(name)
    }

    /// The viewer is at `url`: logged, and opened by `phitex.openViewer`.
    pub fn viewer(&self, url: &str) {
        *lock(&self.shared.viewer) = Some(url.to_owned());
        self.shared.log(
            3,
            &format!("phitex watch: the viewer is at {url} (phitex.openViewer opens it)"),
        );
    }

    /// Where the terminal's messages go (`Ui::tap`, beside the viewer's
    /// console): lines, notes, warnings and errors logged; the spinner's
    /// text as the build's progress.
    pub fn console(&self) -> Box<dyn Fn(&Value) + Send> {
        let s = self.shared.clone();
        Box::new(move |item: &Value| {
            let text = item["text"].as_str().unwrap_or("");
            match item["kind"].as_str().unwrap_or("") {
                "error" => s.log(1, text),
                "warning" => s.log(2, text),
                "note" => s.log(3, text),
                "line" => s.log(4, text),
                "busy" if s.work_done => {
                    let mut p = lock(&s.progress);
                    match (&*p, text.is_empty()) {
                        (None, false) => {
                            let token =
                                format!("phitex-build-{}", s.next.fetch_add(1, Ordering::Relaxed));
                            s.request("window/workDoneProgress/create", json!({"token": token}));
                            s.notify("$/progress", json!({"token": token, "value": {"kind": "begin", "title": "PhiTeX", "message": text}}));
                            *p = Some(token);
                        }
                        (Some(t), false) => {
                            s.notify(
                                "$/progress",
                                json!({"token": t, "value": {"kind": "report", "message": text}}),
                            );
                        }
                        (Some(t), true) => {
                            s.notify("$/progress", json!({"token": t, "value": {"kind": "end"}}));
                            *p = None;
                        }
                        (None, true) => {}
                    }
                }
                _ => {}
            }
        })
    }

    /// A build is in: its problems published by file (the viewer's items,
    /// `watch.rs`'s `diagnostics`), those of files that had some and no
    /// longer do cleared, the main file's always.
    pub fn built(&self, b: &Built) {
        let s = &self.shared;
        let main = lock(&s.root).1.clone();
        let mut by: HashMap<String, Vec<Value>> = HashMap::new();
        by.insert(s.uri(&main), Vec::new());
        for d in &b.diagnostics {
            let file = d["file"].as_str().unwrap_or(&main);
            by.entry(s.uri(file)).or_default().push(lsp_diagnostic(d));
        }
        let mut published = lock(&s.published);
        for gone in published.iter() {
            by.entry(gone.clone()).or_default();
        }
        published.clear();
        for (uri, ds) in by {
            if !ds.is_empty() {
                published.insert(uri.clone());
            }
            s.notify(
                "textDocument/publishDiagnostics",
                json!({"uri": uri, "diagnostics": ds}),
            );
        }
    }

    /// The editor shows `file` (the watch's name) at `line` and character
    /// `col` (from 1), the place selected and the focus taken: whether the
    /// client can (else the `--editor` command opens it).
    pub fn show(&self, file: &str, line: usize, col: usize) -> bool {
        let s = &self.shared;
        if !s.show_document {
            return false;
        }
        let line0 = line.saturating_sub(1);
        // (characters as UTF-16 units, where the editor's buffer is known)
        let mut ch = col.saturating_sub(1);
        if let Some(d) = lock(&s.docs).get(file)
            && let Some(l) = d.lines().nth(line0)
        {
            ch = l.chars().take(ch).map(char::len_utf16).sum();
        }
        let at = json!({"line": line0, "character": ch});
        s.request(
            "window/showDocument",
            json!({"uri": s.uri(file), "takeFocus": true, "selection": {"start": at, "end": at}}),
        );
        true
    }

    /// Serve the editor's messages on a thread of their own; `quit` ends
    /// the watch (`exit`, or the end of standard input).
    pub fn serve(&self, quit: Box<dyn Fn() + Send>) {
        let s = self.shared.clone();
        let input = lock(&self.input).take();
        std::thread::Builder::new()
            .name("phitex-lsp".into())
            .spawn(move || {
                let mut input = input.unwrap_or_else(|| BufReader::new(std::io::stdin()));
                while let Some(m) = read(&mut input) {
                    if !message(&s, &m) {
                        break;
                    }
                }
                quit();
            })
            .expect("a thread");
    }
}

/// A viewer problem (`watch.rs`'s `diagnostics`) as an LSP diagnostic:
/// its line, the culprit's columns where it has an excerpt (counted in
/// UTF-16 in the excerpt's line), its help under its message.
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

/// One message from the editor handled; false: it said to end.
fn message(s: &Shared, m: &Value) -> bool {
    let method = m["method"].as_str().unwrap_or("");
    let id = m.get("id");
    let p = &m["params"];
    match method {
        // (a response to one of ours, or a notification of no interest)
        ""
        | "initialized"
        | "$/cancelRequest"
        | "$/setTrace"
        | "textDocument/didSave"
        | "workspace/didChangeConfiguration"
        | "workspace/didChangeWatchedFiles" => {}
        "exit" => return false,
        "shutdown" => {
            if let Some(id) = id {
                s.reply(id, Value::Null);
            }
        }
        "textDocument/didOpen" => {
            let d = &p["textDocument"];
            if let (Some(uri), Some(text)) = (d["uri"].as_str(), d["text"].as_str())
                && let Some(file) = s.name(uri)
            {
                lock(&s.docs).insert(file.clone(), text.to_owned());
                s.ask(Ask::SetFile {
                    file,
                    text: text.to_owned(),
                });
            }
        }
        "textDocument/didChange" => {
            let Some(file) = p["textDocument"]["uri"].as_str().and_then(|u| s.name(u)) else {
                return true;
            };
            let n = |v: &Value| {
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0)
            };
            for c in p["contentChanges"].as_array().into_iter().flatten() {
                let text = c["text"].as_str().unwrap_or("").to_owned();
                let mut docs = lock(&s.docs);
                let Some(d) = docs.get_mut(&file) else { break };
                if c.get("range").is_some() {
                    let r = &c["range"];
                    let start = offset(d, n(&r["start"]["line"]), n(&r["start"]["character"]));
                    let end = offset(d, n(&r["end"]["line"]), n(&r["end"]["character"]));
                    *d = crate::watch::splice_utf16(d, start, end, &text);
                    drop(docs);
                    s.ask(Ask::Edit {
                        file: file.clone(),
                        start,
                        end,
                        text,
                    });
                } else {
                    d.clone_from(&text);
                    drop(docs);
                    s.ask(Ask::SetFile {
                        file: file.clone(),
                        text,
                    });
                }
            }
        }
        "textDocument/didClose" => {
            if let Some(file) = p["textDocument"]["uri"].as_str().and_then(|u| s.name(u))
                && lock(&s.docs).remove(&file).is_some()
            {
                s.ask(Ask::Closed { file });
            }
        }
        "phitex/cursor" => {
            if let Some((file, line, col)) = s.place(p) {
                s.ask(Ask::Cursor { file, line, col });
            }
        }
        "workspace/executeCommand" => {
            let Some(id) = id else { return true };
            match p["command"].as_str().unwrap_or("") {
                "phitex.openViewer" => match lock(&s.viewer).clone() {
                    Some(url) if crate::view::open_browser(&url) => s.reply(id, json!(url)),
                    Some(url) => s.fail(id, -32603, &format!("could not start a browser: open {url}")),
                    None => s.fail(id, -32603, "no viewer (--no-view)"),
                },
                "phitex.rebuild" => {
                    s.ask(Ask::Rebuild);
                    s.reply(id, Value::Null);
                }
                "phitex.showOnPage" => match s.place(&p["arguments"][0]) {
                    Some((file, line, col)) => {
                        s.ask(Ask::Cursor { file, line, col });
                        s.reply(id, Value::Null);
                    }
                    None => s.fail(id, -32602, "phitex.showOnPage: {textDocument: {uri}, position} of a file of the project"),
                },
                c => s.fail(id, -32601, &format!("no command {c:?} here")),
            }
        }
        _ => {
            if let Some(id) = id {
                s.fail(id, -32601, &format!("no method {method:?} here"));
            }
        }
    }
    true
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

    #[test]
    fn words_around_the_cursor() {
        let l = r"Some \emph{quite} long words, here $x^2$ and there % a comment";
        let col = l.find("words").unwrap();
        let n = needles(l, col);
        assert_eq!(n[0], "quite long words, here");
        assert!(n.contains(&"long words,".to_owned()), "{n:?}");
        assert!(
            n.iter().all(|w| !w.contains("comment") && !w.contains('x')),
            "{n:?}"
        );
        let n = needles(l, l.find("there").unwrap());
        assert_eq!(n[0], "and there");
        assert_eq!(n.last().unwrap(), "there");
        assert_eq!(needles("% only a comment", 3), Vec::<String>::new());
    }
}
