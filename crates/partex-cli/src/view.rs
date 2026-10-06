//! The live viewer (DESIGN 4.8): `partex watch` serves its pages to a
//! browser on 127.0.0.1, and tells it after each build which pages
//! changed, so the browser draws again only those, where they are.
//!
//! - **Server.** A thread accepts connections on a free port; each is
//!   served on a thread of its own (`ws.rs`: HTTP, then a WebSocket). Every
//!   path begins with a random token, so no other page in the browser can
//!   read the document.
//! - **Pages.** The PDF the build wrote is read back after each build:
//!   each page's hash (`phitex_draw`, its content and size), and a page's
//!   draw list (`Draws2`, `"v":2`) when the browser asks for it, kept by
//!   hash. A build reads nothing of this: the PDF is written as before.
//! - **Protocol.** The Overleaf extension's core requests (its
//!   `session.ts` `CoreReq` and `CoreRes`) as JSON over the WebSocket: a
//!   request `{"id":N,"op":…}`, its reply `{"id":N,"ok":…,"json":…,
//!   "draws":…}`; unasked, events `{"event":"settled"}` (a build is in:
//!   lay out the pages again) and `{"event":"preparing","on":…}`. The
//!   ops: `open`, `pages` (the hashes), `png` (a page's draw list, the
//!   name kept from the extension), `status`, `origins`, `edit`.
//! - **Page.** `viewer/index.html` and `viewer/viewer.js`, the bundle of
//!   the extension's viewer (`viewer.ts`, `page2.ts`) and the CLI's host
//!   (`viewer/cli.ts`), made by `scripts/viewer-bundle.sh`.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::json::{self, Value};
use crate::origins::json_str;
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
    /// The PDF it wrote (none: a DVI job, or no page).
    pub pdf: Option<PathBuf>,
    /// TeX's history (0 spotless … 3 fatal).
    pub history: i32,
    /// How long the build took.
    pub ms: f64,
}

/// The pages of the last build.
#[derive(Default)]
struct Pages {
    /// Builds so far.
    generation: u64,
    pdf: Option<Arc<[u8]>>,
    sums: Vec<phitex_draw::PageSum>,
    hashes: Vec<u64>,
    history: i32,
    ms: f64,
    /// Draw lists, by page hash (those of the pages there are now).
    draws: HashMap<u64, Arc<str>>,
}

/// A browser connected.
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
    /// Where the text fonts are looked up.
    host: Mutex<crate::native::NativeHost>,
    /// The watch's live area: lines for the user.
    live: Arc<crate::live::Live>,
    /// Log each page sent (`PARTEX_VIEW_LOG=1`).
    log: bool,
    building: AtomicBool,
    /// When the last build was in, to time the pages sent after it.
    built_at: Mutex<Instant>,
    sent: AtomicU64,
}

/// The live viewer of a watch.
pub struct View {
    shared: Arc<Shared>,
    url: String,
}

/// A random token for the paths (from the kernel, else the clock and
/// the process).
fn token() -> String {
    let mut b = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .is_ok();
    if !ok {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let h = partex_core::persist_hash(&(t, std::process::id()));
        b = h.to_le_bytes();
    }
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

impl View {
    /// Serve the viewer on 127.0.0.1, on a free port (`PARTEX_VIEW_PORT`
    /// to choose it). `host` finds the viewer's text fonts.
    pub fn start(
        host: crate::native::NativeHost,
        live: Arc<crate::live::Live>,
    ) -> std::io::Result<View> {
        let port: u16 = std::env::var("PARTEX_VIEW_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            token: token(),
            pages: Mutex::new(Pages::default()),
            clients: Mutex::new(Vec::new()),
            fonts: Mutex::new(phitex_draw::Fonts::new()),
            text_fonts: Mutex::new(BTreeMap::new()),
            host: Mutex::new(host),
            live,
            log: std::env::var("PARTEX_VIEW_LOG").is_ok_and(|v| v == "1"),
            building: AtomicBool::new(false),
            built_at: Mutex::new(Instant::now()),
            sent: AtomicU64::new(0),
        });
        let url = format!("http://127.0.0.1:{port}/{}/", shared.token);
        let s = shared.clone();
        std::thread::Builder::new()
            .name("partex-view".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    // (small frames, answered at once: no Nagle delay)
                    let _ = conn.set_nodelay(true);
                    let s = s.clone();
                    let _ = std::thread::Builder::new()
                        .name("partex-view-conn".into())
                        .spawn(move || serve(&s, conn));
                }
            })?;
        Ok(View { shared, url })
    }

    /// The page's address.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A rebuild began (`true`) or ended.
    pub fn building(&self, on: bool) {
        self.shared.building.store(on, Ordering::Relaxed);
        broadcast(
            &self.shared,
            &format!("{{\"event\":\"preparing\",\"on\":{on}}}"),
        );
    }

    /// A build is in: its PDF read back, the pages' hashes made, and the
    /// browsers told (they ask for the pages that changed).
    pub fn built(&self, b: &Built) {
        let t = Instant::now();
        let data: Option<Arc<[u8]>> = b
            .pdf
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .map(Into::into);
        let mut pages = lock(&self.shared.pages);
        let first = match (&pages.pdf, &data) {
            (Some(old), Some(new)) => Some(
                old.iter()
                    .zip(new.iter())
                    .take_while(|(a, b)| a == b)
                    .count(),
            ),
            _ => None,
        };
        let sums = data
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
        pages.generation += 1;
        pages.pdf = data;
        pages.sums = sums;
        pages.history = b.history;
        pages.ms = b.ms;
        let n = hashes.len();
        pages.hashes = hashes;
        drop(pages);
        *lock(&self.shared.built_at) = Instant::now();
        if self.shared.log {
            self.shared.live.note(&format!(
                "viewer: {} of {n} pages changed {} (hashed in {:.1} ms)",
                changed.len(),
                ranges(&changed),
                t.elapsed().as_secs_f64() * 1e3
            ));
        }
        broadcast(&self.shared, "{\"event\":\"settled\"}");
    }
}

/// Pages `1 3 4 5` as `1, 3–5`.
fn ranges(ks: &[usize]) -> String {
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
#[allow(clippy::many_single_char_names)]
fn serve(s: &Arc<Shared>, conn: TcpStream) {
    let Ok(w) = conn.try_clone() else { return };
    let mut w = w;
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
            match pdf {
                Some(p) => {
                    let _ = ws::respond(&mut w, "200 OK", "application/pdf", &p);
                }
                None => {
                    let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"no PDF yet\n");
                }
            }
        }
        "ws" if req.is_upgrade() => {
            if ws::upgrade(&mut w, &req).is_ok() {
                socket(s, w, r);
            }
        }
        f if f.starts_with("fonts/") => {
            let name = &f[6..];
            match text_font(s, name) {
                Some(b) => {
                    let _ = ws::respond(&mut w, "200 OK", "font/otf", &b);
                }
                None => {
                    let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"no such font\n");
                }
            }
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
    let mut cache = lock(&s.text_fonts);
    if let Some(f) = cache.get(name) {
        return f.clone();
    }
    let f = partex_core::Host::read_file(
        &mut *lock(&s.host),
        name.as_bytes(),
        partex_core::FileKind::OpenType,
    )
    .map(|f| f.contents);
    cache.insert(name.to_owned(), f.clone());
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
        let Some(req) = json::parse(&text) else {
            continue;
        };
        let reply = answer(s, &req);
        if !client.send(&reply) {
            break;
        }
    }
    let _ = ws::send_close(&mut *lock(&client.out));
    lock(&s.clients).retain(|c| !Arc::ptr_eq(c, &client));
}

/// The pages' hashes as JSON strings.
fn hashes_json(hs: &[u64]) -> String {
    let v: Vec<String> = hs.iter().map(|h| format!("\"{h:016x}\"")).collect();
    format!("[{}]", v.join(","))
}

/// Page `k`'s draw list and hash, drawn now or kept.
#[allow(clippy::many_single_char_names)]
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
    let d: Arc<str> = phitex_draw::Pdf::open(&pdf)?
        .draw(k, &mut lock(&s.fonts))?
        .into();
    if s.log {
        let after = lock(&s.built_at).elapsed();
        s.live.note(&format!(
            "viewer: page {} ({h:016x}) drawn in {:.1} ms, {} KB, sent {:.0} ms after the build",
            k + 1,
            t.elapsed().as_secs_f64() * 1e3,
            d.len() / 1024,
            after.as_secs_f64() * 1e3
        ));
    }
    s.sent.fetch_add(1, Ordering::Relaxed);
    lock(&s.pages).draws.insert(h, d.clone());
    Some((d, h))
}

/// The reply to request `req` (`session.ts`'s `CoreRes`, with its `id`).
fn answer(s: &Shared, req: &Value) -> String {
    let id = req.get("id").and_then(Value::index).unwrap_or(0);
    let op = req.get("op").and_then(Value::str).unwrap_or("");
    let page = req.get("page").and_then(Value::index);
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
                "true,\"json\":{{\"pages\":{},\"hashes\":{},\"history\":{history},\
                 \"build_ms\":{ms},\"engine\":\"partex\",\"fetches\":true,\"missing\":[],\
                 \"pending\":0,\"undefined_names\":[]}}}}",
                hs.len(),
                hashes_json(&hs)
            );
        }
        "pages" => {
            let (hs, _, _) = state();
            let _ = write!(out, "true,\"json\":{{\"pages\":{}}}}}", hashes_json(&hs));
        }
        "png" => match page.and_then(|k| draws(s, k)) {
            Some((d, h)) => {
                let _ = write!(
                    out,
                    "true,\"draws\":{d},\"json\":{{\"hash\":\"{h:016x}\"}}}}"
                );
            }
            None => out.push_str("false,\"error\":\"no such page\"}"),
        },
        "edit" => {
            // (the files are watched on disk: an edit is answered with
            // the last build's pages, the one in view drawn)
            let (hs, history, ms) = state();
            let painted = page.and_then(|k| draws(s, k));
            let _ = write!(
                out,
                "true,\"json\":{{\"pages\":{},\"history\":{history},\"painted_hash\":{},\
                 \"paint_ms\":{ms},\"total_ms\":{ms},\"stats\":{{\"rebuilt\":0,\"reused\":0,\"passes\":1}}}}",
                hs.len(),
                painted
                    .as_ref()
                    .map_or("null".to_owned(), |(_, h)| format!("\"{h:016x}\""))
            );
            if let Some((d, _)) = painted {
                let _ = write!(out, ",\"draws\":{d}");
            }
            out.push('}');
        }
        "origins" => out.push_str("true,\"json\":{\"files\":[],\"g\":[]}}"),
        "set_file" | "trace" | "check" => out.push_str("true,\"json\":{\"ok\":true,\"ms\":0}}"),
        _ => {
            out.push_str("false,\"error\":");
            let mut e = Vec::new();
            json_str(&mut e, &format!("no op {op:?} here"));
            out.push_str(&String::from_utf8_lossy(&e));
            out.push('}');
        }
    }
    out
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
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
