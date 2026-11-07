//! The live viewer's transport: `phitex watch` serves its pages to a
//! browser on 127.0.0.1, and tells it after each build which pages
//! changed, so the browser draws again only those. The viewer's state and
//! protocol are `phitex_live::view`'s ([`Viewer`], which [`View`] derefs
//! to); this is how they reach a browser.
//!
//! - **Server.** A thread accepts connections on the document's port
//!   ([`doc_port`]: from its main file's path, the next one up if taken);
//!   each is served on a thread of its own (`ws.rs`: HTTP, then a
//!   `tungstenite` WebSocket). Every path begins with a random token, so no
//!   other page in the browser can read the document; the token is kept in
//!   the document's cache folder ([`doc_token`]), so the address stays the
//!   same from one watch to the next and a tab still open joins again.
//! - **Pages.** Drawn here as strips (`phitex_live::view`'s
//!   [`Viewer::frame`], `phitex_render`): `frame/K/S` is page K (from 0)
//!   at S thousandths of a pixel a point, its bands' keys and places, its
//!   text and links; `strips/KEY,…` the pixels of the bands the browser
//!   does not have. The browser keeps the bands it has by key and composes
//!   each page from them. After a build, `{"event":"settled"}`: the
//!   browser asks again for the pages whose hashes changed, and only their
//!   new bands travel. `doc.pdf` is the build's PDF (a DVI build's made a
//!   PDF by dvipdfmx: `dvipdf.rs`), for downloading.
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
/// Selects a selection made on the pages (file, start and end line and
/// column, from 1) in the editor: whether it did.
type Select = Box<dyn Fn(&str, (usize, usize), (usize, usize)) -> bool + Send>;

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
    /// Selects a selection on the pages in the editor (LSP).
    lsp_select: Arc<Mutex<Option<Select>>>,
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

/// The ports a document's viewer is served on: above the registered
/// services most machines run (1024-19999 holds many) and below Linux's
/// and macOS's ephemeral ranges (32768 and 49152 up).
const PORTS: std::ops::Range<u16> = 20000..30000;

/// The port document `main`'s viewer tries first: from its absolute
/// path, so the same document is served at the same address.
fn doc_port(main: &std::path::Path) -> u16 {
    let p = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    let h = crate::tex::Fnv::new().add(p.to_string_lossy().as_bytes()).0;
    let n = u64::from(PORTS.end - PORTS.start);
    PORTS.start + u16::try_from(h % n).unwrap_or(0)
}

/// Document `main`'s token: kept in its cache folder (made the first
/// time, random, readable by the user alone), never derived from the
/// path: a token a page could guess would let it read the document. A
/// fresh one where the cache can't keep it.
fn doc_token(main: &std::path::Path) -> String {
    let Ok(c) = crate::cache::Cache::open(None) else {
        return token();
    };
    let f = c.doc_dir(main).join("viewer-token");
    if let Ok(t) = std::fs::read_to_string(&f)
        && t.len() == 32
        && t.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return t;
    }
    let t = token();
    if let Some(d) = f.parent()
        && std::fs::create_dir_all(d).is_ok()
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        if let Ok(mut w) = o.open(&f) {
            let _ = std::io::Write::write_all(&mut w, t.as_bytes());
        }
    }
    t
}

/// A listener on 127.0.0.1: `port` if given (an error if taken), else
/// document `main`'s ([`doc_port`]) or the first free one above it in
/// [`PORTS`], else any free one.
fn listen(port: Option<u16>, main: &std::path::Path) -> std::io::Result<TcpListener> {
    if let Some(p) = port {
        return TcpListener::bind(("127.0.0.1", p));
    }
    let first = doc_port(main);
    let n = PORTS.end - PORTS.start;
    for k in 0..64u16 {
        let p = PORTS.start + (first - PORTS.start + k) % n;
        if let Ok(l) = TcpListener::bind(("127.0.0.1", p)) {
            return Ok(l);
        }
    }
    TcpListener::bind(("127.0.0.1", 0))
}

impl View {
    /// Serve the viewer on 127.0.0.1, on `port` (none: document `main`'s,
    /// [`listen`]); a source place opened by `editor`.
    pub fn start(
        editor: Option<crate::editor::Editor>,
        port: Option<u16>,
        main: &std::path::Path,
    ) -> std::io::Result<View> {
        let listener = listen(port, main)?;
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
        let lsp_select: Arc<Mutex<Option<Select>>> = Arc::default();
        {
            let lsp_select = lsp_select.clone();
            viewer.on_select(Box::new(move |file: &str, a, b| {
                let place = format!("{file}:{}:{}-{}:{}", a.0, a.1, b.0, b.1);
                if lock(&lsp_select).as_ref().is_some_and(|f| f(file, a, b)) {
                    eprintln!("selection: {place} (selected in the editor)");
                } else {
                    eprintln!(
                        "selection: {place} (an editor speaking LSP to the watch selects it)"
                    );
                }
            }));
        }
        let shared = Arc::new(Shared {
            token: doc_token(main),
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
        Ok(View {
            shared,
            url,
            lsp,
            lsp_select,
        })
    }

    /// The viewer's state, shared (the session's progress records reach
    /// it from the engine's threads).
    pub fn viewer(&self) -> Arc<Viewer> {
        self.shared.viewer.clone()
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

    /// How many browsers are connected now, asked from any thread.
    pub fn clients_handle(&self) -> impl Fn() -> usize + Send + 'static {
        let c = self.shared.clients.clone();
        move || lock(&c).len()
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

    /// A selection on the pages selected by `select(file, from, to)` (an
    /// editor that speaks LSP to the watch) where it returns true.
    pub fn on_select_source(&self, select: Select) {
        *lock(&self.lsp_select) = Some(select);
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
        // (a page as strips: `frame/K/S`, page K from 0 at S thousandths of
        // a pixel a point, `frame/K/S/L,T,R,B` only that part of it drawn
        // (fractions of its width and height); `strips/KEY,KEY,...`, the
        // strips' pixels)
        f if f.starts_with("frame/") => {
            let body = f[6..].split_once('/').and_then(|(k, rest)| {
                let (sc, win) = rest
                    .split_once('/')
                    .map_or((rest, None), |(a, b)| (a, Some(b)));
                let scale = sc.parse::<f32>().ok()? / 1000.0;
                let win = match win {
                    Some(w) => {
                        let v: Vec<f32> = w
                            .split(',')
                            .map(str::parse)
                            .collect::<Result<_, _>>()
                            .ok()?;
                        Some(<[f32; 4]>::try_from(v).ok()?)
                    }
                    None => None,
                };
                s.viewer.frame_in(k.parse().ok()?, scale, win)
            });
            let _ = match body {
                Some(b) => ws::respond(
                    &mut w,
                    "200 OK",
                    "application/json",
                    b.to_string().as_bytes(),
                ),
                None => ws::respond(&mut w, "404 Not Found", "text/plain", b"no such page\n"),
            };
        }
        f if f.starts_with("strips/") => {
            let body = s.viewer.strips(&f[7..]);
            let _ = ws::respond(&mut w, "200 OK", "application/octet-stream", &body);
        }
        "ws" if req.is_upgrade() => {
            if let Ok(sock) = ws::upgrade(r.into_inner(), &req) {
                socket(s, sock);
            }
        }
        _ => {
            let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        }
    }
}

/// A WebSocket's requests, answered in order until it closes. The
/// socket is waited on for a request without its lock (a look at its
/// bytes, [`TcpStream::peek`]); only once bytes are there do the reads
/// take the lock, the broadcasts' too, so an event sent while no
/// request comes goes out at once. Waiting for a request under the lock,
/// each event waited for the read to give up ([`ws::POLL`], 20 ms): `r`
/// with nothing changed took 33 ms to reach the viewer, most of it six
/// events each waiting its turn (pings are answered by `tungstenite`
/// inside the reads).
fn socket(s: &Arc<Shared>, sock: WebSocket<TcpStream>) {
    let probe = sock.get_ref().try_clone().ok();
    let client = Arc::new(Client {
        out: Mutex::new(sock),
    });
    let n = {
        let mut c = lock(&s.clients);
        c.push(client.clone());
        c.len()
    };
    clients(s, n);
    'conn: loop {
        // (bytes there, or the socket closed or failed, looked at with the
        // lock let go; the reads below wait no longer than [`ws::DRAIN`])
        if let Some(p) = &probe {
            let _ = p.set_read_timeout(Some(ws::POLL));
            match p.peek(&mut [0u8; 1]) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => break,
            }
            let _ = p.set_read_timeout(Some(ws::DRAIN));
        }
        // (every message the bytes make, read under the lock and answered
        // with it let go: an answer sends under it)
        let mut texts = Vec::new();
        {
            let mut out = lock(&client.out);
            loop {
                match out.read() {
                    Ok(Message::Text(t)) => texts.push(t),
                    Ok(Message::Close(_)) => break 'conn,
                    Err(e) if ws::timed_out(&e) => break,
                    Err(_) => break 'conn,
                    Ok(_) => {}
                }
            }
        }
        if probe.is_none() {
            // (no look without the lock: the lock let go a moment, a
            // waiting broadcast goes first)
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        for text in texts {
            let Ok(req) = serde_json::from_str::<Value>(text.as_str()) else {
                continue;
            };
            // (in the broadcasts' turn: no event made after the answer goes
            // before it, `Viewer::answer_in_turn`)
            if !s.viewer.answer_in_turn(&req, |reply| client.send(reply)) {
                break 'conn;
            }
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
    let f =
        crate::editor::safe_file_arg(&file.display().to_string()).map_err(std::io::Error::other)?;
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
    use std::time::Duration;

    /// Events reach a connected viewer as they are made, while it sends
    /// nothing: the socket is waited on without its lock. Each event
    /// waited for the reader's poll to give up (20 ms), `r` with nothing
    /// changed 33 ms to the viewer for its six events.
    #[test]
    fn events_reach_a_quiet_viewer_at_once() {
        let v = View::start(None, Some(0), std::path::Path::new("/x/quiet/main.tex"))
            .expect("the viewer");
        let url = v.url.replace("http://", "ws://") + "ws";
        let host = url
            .trim_start_matches("ws://")
            .split('/')
            .next()
            .expect("host")
            .to_owned();
        let tcp = TcpStream::connect(&host).expect("connect");
        let (mut ws, _) = tungstenite::client(url.as_str(), tcp).expect("upgrade");
        // (the server has the client: its first broadcast reaches it)
        let t = std::time::Instant::now();
        while lock(&v.shared.clients).is_empty() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(50));
        let t = std::time::Instant::now();
        for _ in 0..10 {
            v.shared.viewer.written_exact();
        }
        let sent = t.elapsed();
        let mut got = 0;
        while got < 10 {
            if let Ok(Message::Text(_)) = ws.read() {
                got += 1;
            }
        }
        assert!(
            sent < Duration::from_millis(50),
            "ten events took {sent:?} to send to a quiet viewer"
        );
    }

    #[test]
    fn a_documents_port_is_its_own_and_in_range() {
        let a = doc_port(std::path::Path::new("/x/paper/main.tex"));
        assert_eq!(a, doc_port(std::path::Path::new("/x/paper/main.tex")));
        assert!(PORTS.contains(&a));
        assert_ne!(a, doc_port(std::path::Path::new("/x/thesis/main.tex")));
    }
}
