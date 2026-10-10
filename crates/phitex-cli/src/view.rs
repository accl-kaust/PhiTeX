//! The live viewer: `phitex watch` serves its pages to a browser on
//! 127.0.0.1, and tells it after each build which pages changed, so the
//! browser draws again only those.
//!
//! - **Server.** A thread accepts connections on a free port; each is
//!   served on a thread of its own (`ws.rs`: HTTP, then a WebSocket). Every
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
//!   `{"id":N,"ok":…,"json":…,"draws":…}`; unasked, events `settled`,
//!   `preparing`, `diagnostics`, `sync`. Ops: `open`, `status`, `pages`,
//!   `png` (a page's draw list), `diagnostics`, `outline`, `source` (a
//!   double-click's or an error's place: the editor opens there), and the
//!   editor's buffer: `set_file {file, text}` and `edit {file, start, end,
//!   text}` (UTF-16 offsets) become the Build's new versions of the file,
//!   built as an edit. `origins` answers a page's characters with their
//!   source lines (the build's SyncTeX records, `synctex.rs`): what a
//!   double-click finds and forward search highlights. The diff ops
//!   (`diff_*`) say they are not there.
//! - **Forward search.** `GET /TOKEN/sync?file=F&line=L` (what `phitex
//!   sync F:L` asks) pushes `{"event":"sync",…}`; the address is kept in a
//!   file per directory ([`address_file`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::json::{self, Value, quote};
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
    pub diagnostics: Vec<String>,
}

/// What the browsers ask the watch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// File `file`'s text is the editor's buffer from now on.
    SetFile { file: String, text: String },
    /// Characters `start..end` (UTF-16 code units) of `file`'s buffer
    /// replaced by `text`.
    Edit { file: String, start: usize, end: usize, text: String },
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
    out: Mutex<TcpStream>,
}

impl Client {
    fn send(&self, text: &str) -> bool {
        ws::send_text(&mut *lock(&self.out), text).is_ok()
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
    kpse: Mutex<Option<partex_kpse::Kpse>>,
    /// Log each page drawn (`PHITEX_VIEW_LOG=1`).
    log: bool,
    /// The directory the watch runs in: the project's files are named from
    /// it.
    root: PathBuf,
    editor: Option<crate::editor::Editor>,
    diagnostics: Mutex<Arc<str>>,
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    building: AtomicBool,
    /// The last build's SyncTeX records.
    sync: Mutex<Option<Arc<crate::synctex::Doc>>>,
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
        let h = crate::tex::Fnv::new().add(&t.to_le_bytes()).add(&std::process::id().to_le_bytes()).0;
        b[..8].copy_from_slice(&h.to_le_bytes());
        b[8..].copy_from_slice(&h.rotate_left(29).to_le_bytes());
    }
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Where the viewer of the watch in `root` keeps its address (for
/// `phitex sync`): `$XDG_RUNTIME_DIR/phitex-view/`, else the temporary
/// directory's, a file named by the directory's hash.
pub fn address_file(root: &std::path::Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let h = crate::tex::Fnv::new().add(root.to_string_lossy().as_bytes()).0;
    Some(base.join("phitex-view").join(format!("{h:016x}")))
}

impl View {
    /// Serve the viewer on 127.0.0.1, on a free port (`PHITEX_VIEW_PORT`
    /// to choose it).
    pub fn start(editor: Option<crate::editor::Editor>) -> std::io::Result<View> {
        let port: u16 = std::env::var("PHITEX_VIEW_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(0);
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
            diagnostics: Mutex::new(Arc::from("[]")),
            asks: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            building: AtomicBool::new(false),
            sync: Mutex::new(None),
        });
        let url = format!("http://127.0.0.1:{port}/{}/", shared.token);
        if let Some(f) = address_file(&shared.root) {
            let _ = f.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&f, &url);
        }
        let s = shared.clone();
        std::thread::Builder::new().name("phitex-view".into()).spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let _ = conn.set_nodelay(true);
                let s = s.clone();
                let _ = std::thread::Builder::new().name("phitex-view-conn".into()).spawn(move || serve(&s, conn));
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

    /// The last build's SyncTeX records (for `origins`).
    pub fn set_sync(&self, doc: Option<Arc<crate::synctex::Doc>>) {
        *lock(&self.shared.sync) = doc;
    }

    /// A build began (`true`) or ended.
    pub fn building(&self, on: bool) {
        self.shared.building.store(on, Ordering::Relaxed);
        broadcast(&self.shared, &format!("{{\"event\":\"preparing\",\"on\":{on}}}"));
    }

    /// A build is in: its PDF's pages hashed, the browsers told (they ask
    /// for the pages that changed). The pages (from 1) whose hashes
    /// changed, and how many there are.
    pub fn built(&self, b: &Built) -> (Vec<usize>, usize) {
        let mut pages = lock(&self.shared.pages);
        let first = match (&pages.pdf, &b.pdf) {
            (Some(old), Some(new)) => Some(old.iter().zip(new.iter()).take_while(|(x, y)| x == y).count()),
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
        let items: Arc<str> = format!("[{}]", b.diagnostics.join(",")).into();
        *lock(&self.shared.diagnostics) = items.clone();
        broadcast(&self.shared, &format!("{{\"event\":\"diagnostics\",\"items\":{items}}}"));
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
        out.push(if i == j { ks[i].to_string() } else { format!("{}–{}", ks[i], ks[j]) });
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
    let Some(req) = ws::read_request(&mut r) else { return };
    if req.method != "GET" {
        let _ = ws::respond(&mut w, "405 Method Not Allowed", "text/plain", b"GET only\n");
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
        "sync" => {
            let (status, text) = forward(s, &req);
            let _ = ws::respond(&mut w, status, "text/plain; charset=utf-8", text.as_bytes());
        }
        "ws" if req.is_upgrade() => {
            if ws::upgrade(&mut w, &req).is_ok() {
                socket(s, w, r);
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
        k.find_file(name.as_bytes(), partex_kpse::Format::OpenType, false)
            .and_then(|p| std::fs::read(&*String::from_utf8_lossy(&p)).ok())
            .map(Arc::from)
    };
    lock(&s.text_fonts).insert(name.to_owned(), f.clone());
    f
}

/// A WebSocket's requests, answered in order until it closes.
fn socket(s: &Arc<Shared>, w: TcpStream, mut r: BufReader<TcpStream>) {
    let client = Arc::new(Client { out: Mutex::new(w) });
    lock(&s.clients).push(client.clone());
    loop {
        let msg = ws::read_message(&mut r, &mut |p| {
            let _ = ws::send_pong(&mut *lock(&client.out), p);
        });
        let text = match msg {
            Ok(ws::Message::Text(t)) => t,
            Ok(ws::Message::Binary(_)) => continue,
            Ok(ws::Message::Close) | Err(_) => break,
        };
        let Some(req) = json::parse(&text) else { continue };
        let reply = answer(s, &req);
        if !client.send(&reply) {
            break;
        }
    }
    let _ = ws::send_close(&mut *lock(&client.out));
    lock(&s.clients).retain(|c| !Arc::ptr_eq(c, &client));
}

fn hashes_json(hs: &[u64]) -> String {
    let v: Vec<String> = hs.iter().map(|h| format!("\"{h:016x}\"")).collect();
    format!("[{}]", v.join(","))
}

/// Page `k`'s draw list and hash, drawn now or kept.
fn draws(s: &Shared, k: usize) -> Option<(Arc<str>, u64)> {
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
        eprintln!("viewer: page {} ({h:016x}) drawn in {:.1} ms, {} KB", k + 1, t.elapsed().as_secs_f64() * 1e3, d.len() / 1024);
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

/// The reply to request `req` (`session.ts`'s `CoreRes`, with its `id`).
fn answer(s: &Shared, req: &Value) -> String {
    let id = req.get("id").and_then(Value::index).unwrap_or(0);
    let op = req.get("op").and_then(Value::str).unwrap_or("");
    let page = req.get("page").and_then(Value::index);
    let text = |k: &str| req.get(k).and_then(Value::str).map(str::to_owned);
    let state = || {
        let p = lock(&s.pages);
        (p.hashes.clone(), p.history, p.ms)
    };
    let mut out = format!("{{\"id\":{id},\"ok\":");
    match op {
        "open" | "status" => {
            let (hs, history, ms) = state();
            let _ = write!(
                out,
                "true,\"json\":{{\"pages\":{},\"hashes\":{},\"history\":{history},\"build_ms\":{ms},\
                 \"engine\":\"phitex-flow\",\"fetches\":true,\"missing\":[],\"pending\":{},\"undefined_names\":[]}}}}",
                hs.len(),
                hashes_json(&hs),
                u8::from(s.building.load(Ordering::Relaxed))
            );
        }
        "pages" => {
            let (hs, _, _) = state();
            let _ = write!(out, "true,\"json\":{{\"pages\":{}}}}}", hashes_json(&hs));
        }
        "png" => match page.and_then(|k| draws(s, k)) {
            Some((d, h)) => {
                let _ = write!(out, "true,\"draws\":{d},\"json\":{{\"hash\":\"{h:016x}\"}}}}");
            }
            None => out.push_str("false,\"error\":\"no such page\"}"),
        },
        "set_file" | "edit" => {
            let file = text("file").unwrap_or_default();
            let body = text("text").unwrap_or_default();
            let a = if op == "set_file" {
                Ask::SetFile { file, text: body }
            } else {
                Ask::Edit {
                    file,
                    start: req.get("start").and_then(Value::index).unwrap_or(0),
                    end: req.get("end").and_then(Value::index).unwrap_or(0),
                    text: body,
                }
            };
            ask(s, a);
            // (answered with the last build's pages; the edit's build
            // pushes `settled` when it is in)
            let (hs, history, ms) = state();
            let painted = page.and_then(|k| draws(s, k));
            let _ = write!(
                out,
                "true,\"json\":{{\"ok\":true,\"pages\":{},\"history\":{history},\"painted_hash\":{},\
                 \"paint_ms\":{ms},\"total_ms\":{ms},\"stats\":{{\"rebuilt\":0,\"reused\":0,\"passes\":1}}}}",
                hs.len(),
                painted.as_ref().map_or("null".to_owned(), |(_, h)| format!("\"{h:016x}\""))
            );
            if let Some((d, _)) = painted {
                let _ = write!(out, ",\"draws\":{d}");
            }
            out.push('}');
        }
        "origins" => {
            let _ = write!(out, "true,\"json\":{}}}", origins(s, page.unwrap_or(0)));
        }
        "source" => {
            let file = text("file").unwrap_or_default();
            let at = req
                .get("line")
                .and_then(Value::index)
                .map(|l| (l, req.get("col").and_then(Value::index).unwrap_or(1)));
            let start = req.get("start").and_then(Value::index).unwrap_or(0);
            let _ = write!(out, "{}}}", to_source(s, &file, start, at));
        }
        "diagnostics" => {
            let d = lock(&s.diagnostics).clone();
            let _ = write!(out, "true,\"json\":{{\"items\":{d}}}}}");
        }
        "outline" => {
            let pdf = lock(&s.pages).pdf.clone();
            let items = pdf.as_ref().and_then(phitex_draw::Pdf::open).map_or_else(|| "[]".to_owned(), |d| d.outline());
            let _ = write!(out, "true,\"json\":{{\"items\":{items}}}}}");
        }
        "diff" => out.push_str("true,\"json\":{}}"),
        o if o.starts_with("diff_") => {
            out.push_str("false,\"error\":\"the diff against a past version is not in the flow CLI yet\"}");
        }
        "trace" | "check" => out.push_str("true,\"json\":{\"ok\":true,\"ms\":0}}"),
        _ => {
            let _ = write!(out, "false,\"error\":{}}}", quote(&format!("no op {op:?} here")));
        }
    }
    out
}

/// The line and column (from 1; the column in characters) of byte `at`
/// of `text`.
fn line_col(text: &[u8], at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let before = &text[..at];
    let line = before.split(|&c| c == b'\n').count();
    let start = before.iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1);
    let col = String::from_utf8_lossy(&before[start..]).chars().count() + 1;
    (line, col)
}

/// The bytes of line `line` (from 1) of `text`, without its end, and the
/// byte of column `col` (characters, from 1) in it.
fn line_bytes(text: &[u8], line: usize, col: usize) -> Option<(usize, usize, usize)> {
    let mut lo = 0;
    for _ in 1..line {
        lo += text.get(lo..)?.iter().position(|&c| c == b'\n')? + 1;
    }
    let hi = lo + text[lo..].iter().position(|&c| c == b'\n').unwrap_or(text.len() - lo);
    let at = String::from_utf8_lossy(&text[lo..hi])
        .char_indices()
        .nth(col.saturating_sub(1))
        .map_or(hi, |(i, _)| lo + i);
    Some((lo, hi, at.min(hi)))
}

/// A double-click's source, byte `start` of `file`, or an error's line
/// and column `at`: shown, and opened in the editor if there is one. The
/// reply's tail (`ok` and the rest).
fn to_source(s: &Shared, file: &str, start: usize, at: Option<(usize, usize)>) -> String {
    if file.is_empty() || file.starts_with('/') || file.contains("..") {
        return "false,\"error\":\"not a file of the project\"".into();
    }
    let Ok(text) = std::fs::read(s.root.join(file)) else {
        return "false,\"error\":\"no such file\"".into();
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
    format!("true,\"json\":{{\"place\":{}}}", quote(&place))
}

/// Forward search: `sync?file=F&line=L[&col=C]`, the browsers told to show
/// that line.
fn forward(s: &Shared, req: &ws::Request) -> (&'static str, String) {
    let file = req.param("file").unwrap_or_default();
    let line: usize = req.param("line").and_then(|l| l.parse().ok()).unwrap_or(1);
    let col: usize = req.param("col").and_then(|c| c.parse().ok()).unwrap_or(1);
    let path = std::fs::canonicalize(&file).or_else(|_| std::fs::canonicalize(s.root.join(&file)));
    let root = std::fs::canonicalize(&s.root).unwrap_or_else(|_| s.root.clone());
    let Some(rel) = path.ok().and_then(|p| p.strip_prefix(&root).ok().map(|r| r.to_string_lossy().into_owned())) else {
        return ("404 Not Found", format!("{file}: not a file of the project in {}\n", s.root.display()));
    };
    let Some((lo, hi, at)) = std::fs::read(root.join(&rel)).ok().and_then(|t| line_bytes(&t, line, col)) else {
        return ("404 Not Found", format!("{rel}: no line {line}\n"));
    };
    let ev = format!("{{\"event\":\"sync\",\"file\":{},\"lo\":{lo},\"hi\":{hi},\"at\":{at}}}", quote(&rel));
    let n = {
        let mut cs = lock(&s.clients);
        cs.retain(|c| c.send(&ev));
        cs.len()
    };
    if n == 0 {
        return ("503 Service Unavailable", "no viewer is open\n".into());
    }
    ("200 OK", format!("{rel}:{line} sent to {n} viewer(s)\n"))
}

/// Page `k`'s characters with their sources (`session.ts`'s `origins`:
/// `{files, g}`, each glyph `[x, y, file, start, end, synth]`, points from
/// the page's top left, `start..end` the bytes of its source line): from
/// the last build's SyncTeX records. Files are named from the project's
/// directory (others by their paths: the viewer has none of them).
fn origins(s: &Shared, k: usize) -> String {
    let Some(doc) = lock(&s.sync).clone() else { return "{\"files\":[],\"g\":[]}".into() };
    let root = std::fs::canonicalize(&s.root).unwrap_or_else(|_| s.root.clone());
    let files: Vec<String> = doc
        .inputs
        .iter()
        .map(|n| {
            let p = PathBuf::from(n.replace("/./", "/"));
            let p = std::fs::canonicalize(&p).unwrap_or(p);
            p.strip_prefix(&root).map_or_else(|_| n.clone(), |r| r.to_string_lossy().into_owned())
        })
        .collect();
    // (each file's line starts, read once)
    let mut starts: HashMap<u32, Option<Vec<usize>>> = HashMap::new();
    let mut g = String::new();
    for gl in doc.pages.get(k).map(|p| p.as_slice()).unwrap_or_default() {
        let ls = starts.entry(gl.tag).or_insert_with(|| {
            let f = files.get((gl.tag as usize).checked_sub(1)?)?;
            let text = std::fs::read(root.join(f)).ok()?;
            let mut v = vec![0];
            v.extend(text.iter().enumerate().filter(|&(_, &c)| c == b'\n').map(|(i, _)| i + 1));
            v.push(text.len() + 1);
            Some(v)
        });
        let (lo, hi) = ls
            .as_ref()
            .and_then(|v| Some((*v.get((gl.line as usize).checked_sub(1)?)?, v.get(gl.line as usize)?.saturating_sub(1))))
            .unwrap_or((0, 0));
        if !g.is_empty() {
            g.push(',');
        }
        let _ = write!(g, "[{:.2},{:.2},{},{lo},{hi},0]", gl.x, gl.y, i64::from(gl.tag) - 1);
    }
    let fs: Vec<String> = files.iter().map(|f| quote(f)).collect();
    format!("{{\"files\":[{}],\"g\":[{g}]}}", fs.join(","))
}

/// `phitex sync FILE:LINE[:COL]`: the viewer of the watch running here
/// shows that line. The exit status.
pub fn sync_command(place: &str) -> i32 {
    let parsed = place.rsplit_once(':').and_then(|(rest, last)| {
        let last: usize = last.parse().ok()?;
        Some(match rest.rsplit_once(':').map(|(f, l)| (f, l.parse::<usize>())) {
            Some((f, Ok(l))) => (f.to_owned(), l, last),
            _ => (rest.to_owned(), last, 1),
        })
    });
    let Some((file, line, col)) = parsed else {
        eprintln!("phitex sync: give FILE:LINE[:COL]");
        return 2;
    };
    let file = std::fs::canonicalize(&file).map_or(file, |p| p.to_string_lossy().into_owned());
    let here = std::env::current_dir().unwrap_or_default();
    let url = here.ancestors().find_map(|d| address_file(d).and_then(|f| std::fs::read_to_string(f).ok()));
    let Some(url) = url else {
        eprintln!("phitex sync: no `phitex watch` with a viewer runs here");
        return 1;
    };
    let enc = |v: &str| -> String {
        v.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_./".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    };
    let Some(rest) = url.trim().strip_prefix("http://") else { return 1 };
    let (addr, path) = rest.split_once('/').unwrap_or((rest, ""));
    let req = format!(
        "GET /{path}sync?file={}&line={line}&col={col} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n",
        enc(&file)
    );
    let answer = TcpStream::connect(addr).and_then(|mut c| {
        std::io::Write::write_all(&mut c, req.as_bytes())?;
        let mut a = String::new();
        std::io::Read::read_to_string(&mut c, &mut a)?;
        Ok(a)
    });
    match answer {
        Ok(a) => {
            let (head, body) = a.split_once("\r\n\r\n").unwrap_or((&a, ""));
            let ok = head.starts_with("HTTP/1.1 200");
            if ok {
                print!("{body}");
            } else {
                eprint!("phitex sync: {body}");
            }
            i32::from(!ok)
        }
        Err(e) => {
            eprintln!("phitex sync: the watch's viewer did not answer ({e})");
            1
        }
    }
}

/// Show `url` in the user's browser (`BROWSER`, else the desktop's
/// opener). Whether one was started.
pub fn open_browser(url: &str) -> bool {
    let cmd = std::env::var("BROWSER").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| {
        if cfg!(target_os = "macos") { "open".into() } else { "xdg-open".into() }
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
        assert_eq!(line_bytes(t, 2, 3), Some((3, 8, 6)));
        assert_eq!(line_bytes(t, 9, 1), None);
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
