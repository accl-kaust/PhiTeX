//! The live viewer's transport: `phitex watch` serves its pages to a
//! browser on 127.0.0.1, and tells it after each build which pages
//! changed, so the browser draws again only those. The viewer's state and
//! protocol are `phitex_live::view`'s ([`Viewer`], which [`View`] derefs
//! to); this is how they reach a browser.
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
//! - **Protocol.** `phitex_live::view`'s requests and events, as JSON over
//!   the WebSocket: each request answered on its socket, each event sent
//!   to every browser. A `source` (a double-click's or an error's place)
//!   is shown by the editor that speaks LSP to the watch where there is
//!   one ([`View::on_source`]), else opened by the `--editor` command.

use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use tungstenite::{Message, WebSocket};

pub use phitex_live::view::{Ask, Built, Viewer, ranges};

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

struct Client {
    out: Mutex<WebSocket<TcpStream>>,
}

impl Client {
    fn send(&self, text: &str) -> bool {
        lock(&self.out).send(Message::text(text)).is_ok()
    }
}

/// A source place's showing by an LSP editor: whether it did.
type Goto = Box<dyn Fn(&str, usize, usize) -> bool + Send>;

struct Shared {
    token: String,
    clients: Arc<Mutex<Vec<Arc<Client>>>>,
    /// Told how many browsers are connected when that changes.
    on_clients: Mutex<Option<Box<dyn Fn(usize) + Send>>>,
    viewer: Arc<Viewer>,
}

/// The live viewer of a watch.
pub struct View {
    shared: Arc<Shared>,
    url: String,
    /// Shows a source place (file, line, column from 1) in the editor
    /// instead of the `--editor` command, when it says it did.
    lsp: Arc<Mutex<Option<Goto>>>,
}

impl std::ops::Deref for View {
    type Target = Viewer;
    fn deref(&self) -> &Viewer {
        &self.shared.viewer
    }
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
    /// Serve the viewer on 127.0.0.1, on `port` (none: a free one); a
    /// source place opened by `editor`.
    pub fn start(
        editor: Option<crate::editor::Editor>,
        port: Option<u16>,
    ) -> std::io::Result<View> {
        let port = port.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        // (the directory the watch runs in: the project's files are named
        // from it)
        let root = std::env::current_dir()?;
        let viewer = Arc::new(Viewer::default());
        let clients: Arc<Mutex<Vec<Arc<Client>>>> = Arc::default();
        {
            let clients = clients.clone();
            viewer.on_emit(Box::new(move |text: &str| {
                lock(&clients).retain(|c| c.send(text));
            }));
        }
        viewer.on_read(Box::new(move |file: &str| {
            std::fs::read(root.join(file)).ok()
        }));
        let lsp: Arc<Mutex<Option<Goto>>> = Arc::default();
        {
            let lsp = lsp.clone();
            viewer.on_goto(Box::new(
                move |file: &str, line: usize, col: usize, place: &str| {
                    if lock(&lsp).as_ref().is_some_and(|g| g(file, line, col)) {
                        eprintln!("source: {place} (shown in the editor)");
                        return;
                    }
                    match &editor {
                        Some(e) => match e.open(file, line, col) {
                            Ok(()) => eprintln!("source: {place} (opened in the editor)"),
                            Err(err) => eprintln!("source: {place}: the editor: {err}"),
                        },
                        None => eprintln!("source: {place} (--editor or PHITEX_EDITOR opens it)"),
                    }
                },
            ));
        }
        let shared = Arc::new(Shared {
            token: token(),
            clients,
            on_clients: Mutex::new(None),
            viewer,
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
        Ok(View { shared, url, lsp })
    }

    /// The page's address.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Where the terminal's messages go ([`crate::ui::Ui::tap`]): kept,
    /// and pushed to the browsers as `{"event":"console","item":…}`.
    pub fn console(&self) -> Box<dyn Fn(Value) + Send> {
        self.shared.viewer.console()
    }

    /// Call `f` with how many browsers are connected when that changes.
    pub fn on_clients(&self, f: Box<dyn Fn(usize) + Send>) {
        *lock(&self.shared.on_clients) = Some(f);
    }

    /// A `source` place shown by `goto(file, line, col)` (an editor that
    /// speaks LSP to the watch) where it returns true, else by the
    /// `--editor` command.
    pub fn on_source(&self, goto: Goto) {
        *lock(&self.lsp) = Some(goto);
    }
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
        "diff.pdf" => {
            let pdf = s.viewer.diff_pdf();
            let _ = match pdf {
                Some(p) => ws::respond(&mut w, "200 OK", "application/pdf", &p),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"no diff yet\n"),
            };
        }
        "diff.tex" => {
            let tex = s.viewer.diff_tex();
            let _ = match tex {
                Some(t) => ws::respond(
                    &mut w,
                    "200 OK",
                    "application/x-tex; charset=utf-8",
                    t.as_bytes(),
                ),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"no diff yet\n"),
            };
        }
        "doc.pdf" => {
            // (the diff's pages while they are the ones shown)
            let pdf = s.viewer.pdf();
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
        let reply = s.viewer.answer(&req);
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
