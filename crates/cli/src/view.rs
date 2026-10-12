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
//!   dvipdfmx for the viewer: `dvipdf.rs`): each page's hash
//!   (`phitex_draw`, its content and size, pages whose bytes did not move
//!   keeping theirs), and a page's draw list when the browser asks, kept
//!   by hash. After a build, `{"event":"settled"}`: the browser asks for
//!   the pages whose hashes changed; the watch prints which (kept and
//!   redrawn, first pages first).
//! - **Protocol.** The Overleaf extension's core requests (`session.ts`'s
//!   `CoreReq`) as JSON over the WebSocket: `{"id":N,"op":…}`, answered
//!   `{"id":N,"ok":…,"json":…,"draws":…}` (`serde_json`); unasked, events
//!   `settled`, `preparing`, `diagnostics`. Ops: `open`, `status`, `pages`,
//!   `png` (a page's draw list), `diagnostics`, `outline`, `source` (a
//!   double-click's or an error's place: the editor opens there), and the
//!   editor's buffer: `set_file {file, text}` and `edit {file, start, end,
//!   text}` (UTF-16 offsets) become the Build's new versions of the file,
//!   built as an edit. `origins` answers a page's characters with their
//!   source lines (the build's SyncTeX records, `synctex.rs`): what a
//!   double-click finds and forward search highlights. The diff ops
//!   (`diff_*`) say they are not there.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::ws;

/// The viewer's page and its script (`scripts/viewer-bundle.sh`).
const INDEX: &[u8] = include_bytes!("../viewer/index.html");
const SCRIPT: &[u8] = include_bytes!("../viewer/viewer.js");

/// Fonts the viewer draws text in when a page's font has no outlines
/// (`page2.ts`'s `FILES`), found by kpathsea.
const FONTS: [&str; 6] = [
    "lmroman10-regular.otf",
    "lmroman10-bold.otf",
    "lmroman10-italic.otf",
    "lmroman10-bolditalic.otf",
    "lmmono10-regular.otf",
    "latinmodern-math.otf",
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a build left, for the viewer.
pub struct Built {
    /// The PDF to show (none: no pages, or a DVI that dvipdfmx refused).
    pub pdf: Option<Arc<[u8]>>,
    /// TeX's history (0 spotless … 2 errors).
    pub history: i32,
    pub ms: f64,
    /// Its errors and warnings, each a JSON object.
    pub diagnostics: Vec<Value>,
}

/// What the browsers ask the watch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
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
    sums: Vec<phitex_draw::PageSum>,
    hashes: Vec<u64>,
    history: i32,
    ms: f64,
    /// Draw lists, by page hash (those of the pages there are now).
    draws: HashMap<u64, Arc<str>>,
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
    /// Parsed font programs, across pages and builds.
    fonts: Mutex<phitex_draw::Fonts>,
    /// The viewer's text fonts, by file name, read when first asked.
    text_fonts: Mutex<BTreeMap<String, Option<Arc<[u8]>>>>,
    kpse: Mutex<Option<phitex_kpse::Kpse>>,
    /// Log each page drawn (`PHITEX_VIEW_LOG=1`).
    log: bool,
    /// The directory the watch runs in: the project's files are named from
    /// it.
    root: PathBuf,
    editor: Option<crate::editor::Editor>,
    diagnostics: Mutex<Value>,
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    building: AtomicBool,
}

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
            fonts: Mutex::new(phitex_draw::Fonts::new()),
            text_fonts: Mutex::new(BTreeMap::new()),
            kpse: Mutex::new(None),
            log: std::env::var("PHITEX_VIEW_LOG").is_ok_and(|v| v == "1"),
            root: std::env::current_dir()?,
            editor,
            diagnostics: Mutex::new(json!([])),
            asks: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            building: AtomicBool::new(false),
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

    /// A build is in: its PDF's pages hashed, the browsers told (they ask
    /// for the pages that changed). The pages (from 1) whose hashes
    /// changed, and how many there are.
    pub fn built(&self, b: &Built) -> (Vec<usize>, usize) {
        let mut pages = lock(&self.shared.pages);
        let first = match (&pages.pdf, &b.pdf) {
            (Some(old), Some(new)) => Some(
                old.iter()
                    .zip(new.iter())
                    .take_while(|(x, y)| x == y)
                    .count(),
            ),
            _ => None,
        };
        let sums = b
            .pdf
            .as_ref()
            .and_then(phitex_draw::Pdf::open)
            .map(|p| p.hashes_since(&pages.sums, first))
            .unwrap_or_default();
        let hashes: Vec<u64> = sums.iter().map(|s| s.hash).collect();
        let changed: Vec<usize> = hashes
            .iter()
            .enumerate()
            .filter(|&(k, h)| pages.hashes.get(k) != Some(h))
            .map(|(k, _)| k + 1)
            .collect();
        pages.draws.retain(|h, _| hashes.contains(h));
        pages.pdf.clone_from(&b.pdf);
        pages.sums = sums;
        pages.history = b.history;
        pages.ms = b.ms;
        let n = hashes.len();
        pages.hashes = hashes;
        drop(pages);
        let items = Value::from(b.diagnostics.clone());
        broadcast(
            &self.shared,
            &json!({"event": "diagnostics", "items": items}).to_string(),
        );
        *lock(&self.shared.diagnostics) = items;
        broadcast(&self.shared, "{\"event\":\"settled\"}");
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
        f if f.starts_with("fonts/") => {
            let _ = match text_font(s, &f[6..]) {
                Some(b) => ws::respond(&mut w, "200 OK", "font/otf", &b),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"no such font\n"),
            };
        }
        _ => {
            let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        }
    }
}

/// One of the viewer's text fonts, by file name.
fn text_font(s: &Shared, name: &str) -> Option<Arc<[u8]>> {
    if !FONTS.contains(&name) {
        return None;
    }
    if let Some(f) = lock(&s.text_fonts).get(name) {
        return f.clone();
    }
    let f = {
        let mut k = lock(&s.kpse);
        let k = k.get_or_insert_with(|| crate::tex::kpse("phitex", "pdftex"));
        k.find_file(name.as_bytes(), phitex_kpse::Format::OpenType, false)
            .and_then(|p| std::fs::read(&*String::from_utf8_lossy(&p)).ok())
            .map(Arc::from)
    };
    lock(&s.text_fonts).insert(name.to_owned(), f.clone());
    f
}

/// A WebSocket's requests, answered in order until it closes. Its reads
/// and the broadcasts take turns on the one lock (pings are answered by
/// `tungstenite` inside the reads).
fn socket(s: &Arc<Shared>, sock: WebSocket<TcpStream>) {
    let client = Arc::new(Client {
        out: Mutex::new(sock),
    });
    lock(&s.clients).push(client.clone());
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
    lock(&s.clients).retain(|c| !Arc::ptr_eq(c, &client));
}

fn hashes_json(hs: &[u64]) -> Value {
    hs.iter().map(|h| format!("{h:016x}")).collect()
}

/// Page `k`'s draw list and hash, drawn now or kept.
fn draw_list(s: &Shared, k: usize) -> Option<(Arc<str>, u64)> {
    let (pdf, h) = {
        let p = lock(&s.pages);
        let h = *p.hashes.get(k)?;
        if let Some(d) = p.draws.get(&h) {
            return Some((d.clone(), h));
        }
        (p.pdf.clone()?, h)
    };
    let t = Instant::now();
    let doc = phitex_draw::Pdf::open(&pdf)?;
    let d: Arc<str> = doc.draw(k, &mut lock(&s.fonts))?.into();
    if s.log {
        eprintln!(
            "viewer: page {} ({h:016x}) drawn in {:.1} ms, {} KB",
            k + 1,
            t.elapsed().as_secs_f64() * 1e3,
            d.len() / 1024
        );
    }
    lock(&s.pages).draws.insert(h, d.clone());
    Some((d, h))
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
/// A page's draw list (JSON already) is put in as it is.
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
    let mut draws = None;
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
        "png" => match page.and_then(|k| draw_list(s, k)) {
            Some((d, h)) => {
                draws = Some(d);
                ok(json!({"hash": format!("{h:016x}")}))
            }
            None => fail("no such page"),
        },
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
            let painted = page.and_then(|k| draw_list(s, k));
            let hash = painted.as_ref().map(|(_, h)| format!("{h:016x}"));
            draws = painted.map(|(d, _)| d);
            ok(json!({
                "ok": true,
                "pages": hs.len(),
                "history": history,
                "painted_hash": hash,
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
        "outline" => {
            let pdf = lock(&s.pages).pdf.clone();
            let items = pdf
                .as_ref()
                .and_then(phitex_draw::Pdf::open)
                .and_then(|d| serde_json::from_str::<Value>(&d.outline()).ok())
                .unwrap_or_else(|| json!([]));
            ok(json!({"items": items}))
        }
        "diff" => ok(json!({})),
        o if o.starts_with("diff_") => {
            fail("the diff against a past version is not in the CLI yet")
        }
        "trace" | "check" => ok(json!({"ok": true, "ms": 0})),
        _ => fail(&format!("no op {op:?} here")),
    };
    let mut out = reply.to_string();
    if let Some(d) = draws {
        out.pop();
        let _ = write!(out, ",\"draws\":{d}}}");
    }
    out
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
