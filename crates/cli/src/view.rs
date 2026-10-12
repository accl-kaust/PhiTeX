//! The live viewer: `phitex watch` serves its pages to a browser on
//! 127.0.0.1, and tells it after each build which pages changed, so the
//! browser draws again only those.
//!
//! - **Server.** A thread accepts connections on a free port; each is
//!   served on a thread of its own (`ws.rs`: HTTP, then a `tungstenite`
//!   WebSocket). Every
//!   path begins with a random token, so no other page in the browser can
//!   read the document.
//! - **Pages.** The build's PDF bytes (a DVI build's made a PDF by
//!   dvipdfmx for the viewer: `dvipdf.rs`) at `doc.pdf`, and each page's
//!   hash as the build gives it. The browser draws the pages itself with
//!   MuPDF (`mupdf/`, its WebAssembly build as published, and
//!   `viewer/src/mupdf.ts`). After a build, `{"event":"settled"}`: the
//!   browser loads the new PDF and draws again the pages whose hashes
//!   changed.
//! - **Protocol.** The Overleaf extension's core requests (`session.ts`'s
//!   `CoreReq`) as JSON over the WebSocket: `{"id":N,"op":…}`, answered
//!   `{"id":N,"ok":…,"json":…}` (`serde_json`); unasked, events
//!   `settled`, `preparing`, `diagnostics`, `console` (each message the
//!   terminal shows, and its status line). Ops: `open`, `status`, `pages`,
//!   `diagnostics`, `console` (the messages so far), `rebuild`, `source` (a
//!   double-click's or an error's place: the editor opens there), and the
//!   editor's buffer: `set_file {file, text}` and `edit {file, start, end,
//!   text}` (UTF-16 offsets) become the Build's new versions of the file,
//!   built as an edit. `origins` answers a page's characters with their
//!   source lines (the build's SyncTeX records, `synctex.rs`): what a
//!   double-click finds and forward search highlights. The diff ops
//!   (`diff_*`) say they are not there.

use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::ws;

/// The viewer's page and its script (`scripts/viewer-bundle.sh`).
const INDEX: &[u8] = include_bytes!("../viewer/index.html");
const SCRIPT: &[u8] = include_bytes!("../viewer/viewer.js");

/// MuPDF's WebAssembly build, as published (`mupdf` on npm, its `dist/`,
/// fetched pinned into `upstream/mupdf/` by `scripts/fetch-mupdf.sh`): the
/// browser draws the pages with it.
const MUPDF: [(&str, &str, &[u8]); 3] = [
    (
        "mupdf.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../../upstream/mupdf/dist/mupdf.js"),
    ),
    (
        "mupdf-wasm.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../../../upstream/mupdf/dist/mupdf-wasm.js"),
    ),
    (
        "mupdf-wasm.wasm",
        "application/wasm",
        include_bytes!("../../../upstream/mupdf/dist/mupdf-wasm.wasm"),
    ),
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a build left, for the viewer.
pub struct Built {
    /// The PDF to show (none: no pages, or a DVI that dvipdfmx refused).
    pub pdf: Option<Arc<[u8]>>,
    /// Its pages' hashes, first first (the build's: what changed).
    pub hashes: Vec<u64>,
    /// TeX's history (0 spotless … 2 errors).
    pub history: i32,
    pub ms: f64,
    /// Its errors and warnings, each a JSON object.
    pub diagnostics: Vec<Value>,
    /// The time from the change (a save, an editor's keystroke) to now
    /// (none: no change, a cold build): the viewer adds its drawing to it
    /// for the ⚡ chip.
    pub since_ms: Option<f64>,
}

/// What the browsers ask the watch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Build again (the console's Rebuild).
    Rebuild,
    /// A change reached the screen this long after it was made.
    Painted(u64),
    /// File `file`'s text is the editor's buffer from now on.
    SetFile { file: String, text: String },
    /// Characters `start..end` (UTF-16 code units) of `file`'s buffer
    /// replaced by `text`.
    Edit {
        file: String,
        start: usize,
        end: usize,
        text: String,
    },
}

/// The pages of the last build.
#[derive(Default)]
struct Pages {
    pdf: Option<Arc<[u8]>>,
    hashes: Vec<u64>,
    history: i32,
    ms: f64,
}

struct Client {
    out: Mutex<WebSocket<TcpStream>>,
}

impl Client {
    fn send(&self, text: &str) -> bool {
        lock(&self.out).send(Message::text(text)).is_ok()
    }
}

struct Shared {
    token: String,
    pages: Mutex<Pages>,
    clients: Mutex<Vec<Arc<Client>>>,
    /// The directory the watch runs in: the project's files are named from
    /// it.
    root: PathBuf,
    editor: Option<crate::editor::Editor>,
    diagnostics: Mutex<Value>,
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    building: AtomicBool,
    /// The terminal's messages so far (the last [`CONSOLE`]), and its
    /// status line: what a browser that joins shows first.
    console: Mutex<(std::collections::VecDeque<Value>, Value)>,
    /// Told how many browsers are connected when that changes.
    on_clients: Mutex<Option<Box<dyn Fn(usize) + Send>>>,
}

/// The messages a browser that joins is shown.
const CONSOLE: usize = 2000;

/// The live viewer of a watch.
pub struct View {
    shared: Arc<Shared>,
    url: String,
}

/// A random token for the paths (from the kernel, else the clock and the
/// process).
fn token() -> String {
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .is_ok();
    if !ok {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let h = crate::tex::Fnv::new()
            .add(&t.to_le_bytes())
            .add(&std::process::id().to_le_bytes())
            .0;
        b[..8].copy_from_slice(&h.to_le_bytes());
        b[8..].copy_from_slice(&h.rotate_left(29).to_le_bytes());
    }
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

impl View {
    /// Serve the viewer on 127.0.0.1, on `port` (none: a free one).
    pub fn start(
        editor: Option<crate::editor::Editor>,
        port: Option<u16>,
    ) -> std::io::Result<View> {
        let port = port.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            token: token(),
            pages: Mutex::new(Pages::default()),
            clients: Mutex::new(Vec::new()),
            root: std::env::current_dir()?,
            editor,
            diagnostics: Mutex::new(json!([])),
            asks: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            building: AtomicBool::new(false),
            console: Mutex::new((std::collections::VecDeque::new(), Value::Null)),
            on_clients: Mutex::new(None),
        });
        let url = format!("http://127.0.0.1:{port}/{}/", shared.token);
        let s = shared.clone();
        std::thread::Builder::new()
            .name("phitex-view".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    let _ = conn.set_nodelay(true);
                    let s = s.clone();
                    let _ = std::thread::Builder::new()
                        .name("phitex-view-conn".into())
                        .spawn(move || serve(&s, conn));
                }
            })?;
        Ok(View { shared, url })
    }

    /// The page's address.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Call `wake` when a browser asks the watch for something.
    pub fn on_ask(&self, wake: Box<dyn Fn() + Send>) {
        *lock(&self.shared.wake) = Some(wake);
    }

    /// Call `f` with how many browsers are connected when that changes.
    pub fn on_clients(&self, f: Box<dyn Fn(usize) + Send>) {
        *lock(&self.shared.on_clients) = Some(f);
    }

    /// Where the terminal's messages go ([`crate::ui::Ui::tap`]): kept,
    /// and pushed to the browsers as `{"event":"console","item":…}`.
    pub fn console(&self) -> Box<dyn Fn(Value) + Send> {
        let s = self.shared.clone();
        Box::new(move |item: Value| {
            {
                let mut c = lock(&s.console);
                if item["kind"] == "status" {
                    c.1 = item.clone();
                } else {
                    if c.0.len() == CONSOLE {
                        c.0.pop_front();
                    }
                    c.0.push_back(item.clone());
                }
            }
            broadcast(&s, &json!({"event": "console", "item": item}).to_string());
        })
    }

    /// What the browsers asked for since last taken.
    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut *lock(&self.shared.asks))
    }

    /// A build began (`true`) or ended.
    pub fn building(&self, on: bool) {
        self.shared.building.store(on, Ordering::Relaxed);
        broadcast(
            &self.shared,
            &json!({"event": "preparing", "on": on}).to_string(),
        );
    }

    /// A build is in: its PDF and pages' hashes kept, the browsers told
    /// (they draw the pages that changed). The pages (from 1) whose hashes
    /// changed, and how many there are.
    pub fn built(&self, b: &Built) -> (Vec<usize>, usize) {
        let mut pages = lock(&self.shared.pages);
        let changed: Vec<usize> = b
            .hashes
            .iter()
            .enumerate()
            .filter(|&(k, h)| pages.hashes.get(k) != Some(h))
            .map(|(k, _)| k + 1)
            .collect();
        pages.pdf.clone_from(&b.pdf);
        pages.history = b.history;
        pages.ms = b.ms;
        let n = b.hashes.len();
        pages.hashes.clone_from(&b.hashes);
        drop(pages);
        let items = Value::from(b.diagnostics.clone());
        broadcast(
            &self.shared,
            &json!({"event": "diagnostics", "items": items}).to_string(),
        );
        *lock(&self.shared.diagnostics) = items;
        broadcast(
            &self.shared,
            &json!({"event": "settled", "since_ms": b.since_ms}).to_string(),
        );
        (changed, n)
    }
}

/// Pages `1 3 4 5` as `1, 3–5`.
pub fn ranges(ks: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < ks.len() {
        let mut j = i;
        while j + 1 < ks.len() && ks[j + 1] == ks[j] + 1 {
            j += 1;
        }
        out.push(if i == j {
            ks[i].to_string()
        } else {
            format!("{}–{}", ks[i], ks[j])
        });
        i = j + 1;
    }
    out.join(", ")
}

fn broadcast(s: &Shared, text: &str) {
    lock(&s.clients).retain(|c| c.send(text));
}

/// Serve one connection: an HTTP request, or a WebSocket.
fn serve(s: &Arc<Shared>, conn: TcpStream) {
    let Ok(mut w) = conn.try_clone() else { return };
    let mut r = BufReader::new(conn);
    let Some(req) = ws::read_request(&mut r) else {
        return;
    };
    if req.method != "GET" {
        let _ = ws::respond(
            &mut w,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only\n",
        );
        return;
    }
    let prefix = format!("/{}/", s.token);
    let Some(rest) = req.route().strip_prefix(&prefix) else {
        let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        return;
    };
    let rest = rest.to_owned();
    match rest.as_str() {
        "" | "index.html" => {
            let _ = ws::respond(&mut w, "200 OK", "text/html; charset=utf-8", INDEX);
        }
        // (whether the watch is here, before the browser opens a WebSocket)
        "ping" => {
            let _ = ws::respond(&mut w, "200 OK", "text/plain", b"phitex watch\n");
        }
        "viewer.js" => {
            let _ = ws::respond(&mut w, "200 OK", "text/javascript; charset=utf-8", SCRIPT);
        }
        "doc.pdf" => {
            let pdf = lock(&s.pages).pdf.clone();
            let _ = match pdf {
                Some(p) => ws::respond(&mut w, "200 OK", "application/pdf", &p),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"no PDF yet\n"),
            };
        }
        "ws" if req.is_upgrade() => {
            if let Ok(sock) = ws::upgrade(r.into_inner(), &req) {
                socket(s, sock);
            }
        }
        f if f.starts_with("mupdf/") => {
            let _ = match MUPDF.iter().find(|(n, _, _)| *n == &f[6..]) {
                Some((_, ctype, b)) => ws::respond(&mut w, "200 OK", ctype, b),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n"),
            };
        }
        _ => {
            let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        }
    }
}

/// A WebSocket's requests, answered in order until it closes. Its reads
/// and the broadcasts take turns on the one lock (pings are answered by
/// `tungstenite` inside the reads).
fn socket(s: &Arc<Shared>, sock: WebSocket<TcpStream>) {
    let client = Arc::new(Client {
        out: Mutex::new(sock),
    });
    let n = {
        let mut c = lock(&s.clients);
        c.push(client.clone());
        c.len()
    };
    clients(s, n);
    loop {
        let msg = lock(&client.out).read();
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) => break,
            Err(e) if ws::timed_out(&e) => {
                // (the lock let go a moment: a waiting broadcast goes first)
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            Err(_) => break,
            Ok(_) => continue,
        };
        let Ok(req) = serde_json::from_str::<Value>(text.as_str()) else {
            continue;
        };
        let reply = answer(s, &req);
        if !client.send(&reply) {
            break;
        }
    }
    let _ = lock(&client.out).close(None);
    let n = {
        let mut c = lock(&s.clients);
        c.retain(|c| !Arc::ptr_eq(c, &client));
        c.len()
    };
    clients(s, n);
}

fn clients(s: &Shared, n: usize) {
    if let Some(f) = lock(&s.on_clients).as_ref() {
        f(n);
    }
}

fn hashes_json(hs: &[u64]) -> Value {
    hs.iter().map(|h| format!("{h:016x}")).collect()
}

fn ask(s: &Shared, a: Ask) {
    lock(&s.asks).push(a);
    if let Some(w) = lock(&s.wake).as_ref() {
        w();
    }
}

/// A number as an index or count (none if negative or not whole).
fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok())
}

/// The reply to request `req` (`session.ts`'s `CoreRes`, with its `id`).
fn answer(s: &Shared, req: &Value) -> String {
    let id = req.get("id").and_then(index).unwrap_or(0);
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    let page = req.get("page").and_then(index);
    let text = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
    let state = || {
        let p = lock(&s.pages);
        (p.hashes.clone(), p.history, p.ms)
    };
    let ok = |json: Value| json!({"id": id, "ok": true, "json": json});
    let fail = |e: &str| json!({"id": id, "ok": false, "error": e});
    let reply = match op {
        "open" | "status" => {
            let (hs, history, ms) = state();
            ok(json!({
                "pages": hs.len(),
                "hashes": hashes_json(&hs),
                "history": history,
                "build_ms": ms,
                "engine": "phitex",
                "fetches": true,
                "missing": [],
                "pending": u8::from(s.building.load(Ordering::Relaxed)),
                "undefined_names": [],
            }))
        }
        "pages" => ok(json!({"pages": hashes_json(&state().0)})),
        "png" => fail("the pages are drawn in the browser (MuPDF), from doc.pdf"),
        "set_file" | "edit" => {
            let file = text("file").unwrap_or_default();
            let body = text("text").unwrap_or_default();
            let a = if op == "set_file" {
                Ask::SetFile { file, text: body }
            } else {
                Ask::Edit {
                    file,
                    start: req.get("start").and_then(index).unwrap_or(0),
                    end: req.get("end").and_then(index).unwrap_or(0),
                    text: body,
                }
            };
            ask(s, a);
            // (answered with the last build's pages; the edit's build
            // pushes `settled` when it is in)
            let (hs, history, ms) = state();
            ok(json!({
                "ok": true,
                "pages": hs.len(),
                "history": history,
                "painted_hash": null,
                "paint_ms": ms,
                "total_ms": ms,
                "stats": {"rebuilt": 0, "reused": 0, "passes": 1},
            }))
        }
        "origins" => ok(origins(s, page.unwrap_or(0))),
        "source" => {
            let file = text("file").unwrap_or_default();
            let at = req
                .get("line")
                .and_then(index)
                .map(|l| (l, req.get("col").and_then(index).unwrap_or(1)));
            let start = req.get("start").and_then(index).unwrap_or(0);
            match to_source(s, &file, start, at) {
                Ok(place) => ok(json!({"place": place})),
                Err(e) => fail(e),
            }
        }
        "diagnostics" => ok(json!({"items": lock(&s.diagnostics).clone()})),
        "painted" => {
            // (in microseconds: Ask stays Eq)
            if let Some(ms) = req.get("ms").and_then(Value::as_f64) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                ask(s, Ask::Painted((ms.max(0.0) * 1e3) as u64));
            }
            ok(json!({}))
        }
        "rebuild" => {
            ask(s, Ask::Rebuild);
            ok(json!({}))
        }
        "console" => {
            let c = lock(&s.console);
            ok(json!({"items": c.0.iter().collect::<Vec<_>>(), "status": c.1}))
        }
        // (the browser reads the outline from the PDF itself, with MuPDF)
        "outline" => ok(json!({"items": []})),
        "diff" => ok(json!({})),
        o if o.starts_with("diff_") => {
            fail("the diff against a past version is not in the CLI yet")
        }
        "trace" | "check" => ok(json!({"ok": true, "ms": 0})),
        _ => fail(&format!("no op {op:?} here")),
    };
    reply.to_string()
}

/// The line and column (from 1; the column in characters) of byte `at`
/// of `text`.
fn line_col(text: &[u8], at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let before = &text[..at];
    let line = before.split(|&c| c == b'\n').count();
    let start = before
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |p| p + 1);
    let col = String::from_utf8_lossy(&before[start..]).chars().count() + 1;
    (line, col)
}

/// A double-click's source, byte `start` of `file`, or an error's line
/// and column `at`: shown, and opened in the editor if there is one. The
/// place, `FILE:LINE:COL`.
fn to_source(
    s: &Shared,
    file: &str,
    start: usize,
    at: Option<(usize, usize)>,
) -> Result<String, &'static str> {
    if file.is_empty() || file.starts_with('/') || file.contains("..") {
        return Err("not a file of the project");
    }
    let Ok(text) = std::fs::read(s.root.join(file)) else {
        return Err("no such file");
    };
    let (line, col) = at.unwrap_or_else(|| line_col(&text, start));
    let place = format!("{file}:{line}:{col}");
    match &s.editor {
        Some(e) => match e.open(file, line, col) {
            Ok(()) => eprintln!("source: {place} (opened in the editor)"),
            Err(err) => eprintln!("source: {place}: the editor: {err}"),
        },
        None => eprintln!("source: {place} (--editor or PHITEX_EDITOR opens it)"),
    }
    Ok(place)
}

/// Page `k`'s characters with their sources (`session.ts`'s `origins`:
/// `{files, g}`): none, no engine records origins yet.
fn origins(_s: &Shared, _k: usize) -> Value {
    json!({"files": [], "g": []})
}

/// Start viewer program `cmd` (its words; `{file}` where the file goes,
/// else the file last) on `file`, not waited for.
pub fn open_with(cmd: &str, file: &std::path::Path) -> std::io::Result<()> {
    let f = file.display().to_string();
    let mut words: Vec<String> = cmd.split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return Err(std::io::Error::other("an empty command"));
    }
    if !words.iter().any(|w| w.contains("{file}")) {
        words.push("{file}".to_owned());
    }
    let words: Vec<String> = words.iter().map(|w| w.replace("{file}", &f)).collect();
    let mut c = std::process::Command::new(&words[0])
        .args(&words[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || c.wait());
    Ok(())
}

/// Show `url` in the user's browser (`BROWSER`, else the desktop's
/// opener). Whether one was started.
pub fn open_browser(url: &str) -> bool {
    let cmd = std::env::var("BROWSER")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "open".into()
            } else {
                "xdg-open".into()
            }
        });
    std::process::Command::new(cmd)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut c| {
            std::thread::spawn(move || c.wait());
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_columns() {
        let t = "ab\nçd e\n\nlast".as_bytes();
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 3), (2, 1));
        assert_eq!(line_col(t, 6), (2, 3));
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
