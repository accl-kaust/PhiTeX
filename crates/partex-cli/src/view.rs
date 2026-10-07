//! The live viewer (DESIGN 4.8): `phitex watch` serves its pages to a
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
//! - **Pages while the build runs.** Each page the engine ships comes here
//!   as it is shipped (`Host::stream_shipped`, [`shipped`]), its stream
//!   and resources, and is drawn from a PDF of its own
//!   (`partex_core::pagepdf`) by the same reader: a page that differs from
//!   the last build's is pushed (`{"event":"page","k":…,"hash":…,
//!   "pages":…}`) with a provisional hash, so the browser shows it while
//!   later pages are typeset; the build's PDF, when it is in, replaces it.
//! - **Status.** While a build runs, `{"event":"progress","pass":N,
//!   "pages":K,"phase":…,"ms":T}` (the pass, the pages it shipped so far,
//!   what it does, the time since it began), at most ten a second; when it
//!   is in, `{"event":"diagnostics","items":[…]}` (its errors and
//!   warnings, `snippet::json`'s), before `settled`, and the `diagnostics`
//!   op gives the last build's again.
//! - **Protocol.** The Overleaf extension's core requests (its
//!   `session.ts` `CoreReq` and `CoreRes`) as JSON over the WebSocket: a
//!   request `{"id":N,"op":…}`, its reply `{"id":N,"ok":…,"json":…,
//!   "draws":…}`; unasked, events `{"event":"settled"}` (a build is in:
//!   lay out the pages again) and `{"event":"preparing","on":…}`. The
//!   ops: `open`, `pages` (the hashes), `png` (a page's draw list, the
//!   name kept from the extension), `status`, `origins` (a page's glyphs
//!   and their sources, when the build recorded them), `edit`; and the
//!   CLI's `source` (a double-click's source: the editor opens there).
//!   Forward search: `GET /TOKEN/sync?file=F&line=L` (what `phitex sync
//!   F:L` asks) pushes `{"event":"sync","file":…,"lo":…,"hi":…,"at":…}`,
//!   the line's bytes, and the page highlights the glyphs that came from
//!   them. The address is kept in a file per directory, for `phitex sync`
//!   ([`address_file`]).
//! - **Page.** `viewer/index.html` and `viewer/viewer.js`, the bundle of
//!   the extension's viewer (`viewer.ts`, `page2.ts`) and the CLI's host
//!   (`viewer/cli.ts`), made by `scripts/viewer-bundle.sh`.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use partex_core::pagepdf::ShippedStream;

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
    /// Its errors and warnings, as JSON (`snippet::json`).
    pub diagnostics: Vec<String>,
}

/// What the build running is doing, for the `progress` event.
#[derive(Clone, Default, PartialEq)]
struct Status {
    /// The pass (from 1; 0 before the first begins).
    pass: usize,
    /// What it does (`typesetting`, `loading`, `linking`, …).
    phase: &'static str,
    /// The engine's pages shipped when the pass began (`progress::BOARD`'s
    /// count is the process's).
    pages_base: u64,
}

/// A build's glyph origins and where the glyphs are: what `origins`
/// answers (`Tex::origins` and the PDF's places, as the extension core's
/// `Session::origins`).
pub struct Origins {
    /// The files, by id: a project's file by its path from the watch's
    /// directory, another as the job asked for it.
    files: Vec<String>,
    /// Each page's glyphs.
    pages: Vec<Vec<Placed>>,
}

/// A glyph: x and y (PDF points from the top left), then file
/// (`u32::MAX`: none), start, end, synthesized.
type Placed = (f32, f32, u32, u32, u32, bool);

impl Origins {
    /// From a build's origins (its files by id, each page's glyphs in the
    /// order the page shows them) and its PDF, in directory `root`.
    #[allow(
        dead_code,
        reason = "the watch runtimes record no origins yet (DESIGN 4.8)"
    )]
    #[must_use]
    pub fn of(
        root: &std::path::Path,
        files: &[String],
        pages: &[Vec<partex_core::GlyphOrigin>],
        pdf: &Arc<[u8]>,
    ) -> Origins {
        let files = files
            .iter()
            .map(|n| {
                let n = n.strip_prefix("./").unwrap_or(n);
                [n.to_owned(), format!("{n}.tex")]
                    .into_iter()
                    .find(|c| !c.starts_with('/') && root.join(c).is_file())
                    .unwrap_or_else(|| n.to_owned())
            })
            .collect();
        let doc = phitex_draw::Pdf::open(pdf);
        #[allow(clippy::cast_possible_truncation, reason = "points, to a hundredth")]
        let pages = pages
            .iter()
            .enumerate()
            .map(|(k, os)| {
                let places = doc.as_ref().map(|d| d.glyph_places(k)).unwrap_or_default();
                os.iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let (x, y) = places.get(i).copied().unwrap_or((0.0, 0.0));
                        (x as f32, y as f32, o.file, o.start, o.end, o.synthesized)
                    })
                    .collect()
            })
            .collect();
        Origins { files, pages }
    }
}

/// Where the viewer of the watch in `root` keeps its address (for
/// `phitex sync`): `$XDG_RUNTIME_DIR/phitex-view/`, else the temporary
/// directory's, a file named by the directory's hash.
#[must_use]
pub fn address_file(root: &std::path::Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let base = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let h = partex_core::persist_hash(&root.to_string_lossy().as_bytes());
    Some(base.join("phitex-view").join(format!("{h:032x}")))
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

/// The bytes of line `line` (from 1) of `text`, without its end, and the
/// byte of column `col` (characters, from 1) in it.
fn line_bytes(text: &[u8], line: usize, col: usize) -> Option<(usize, usize, usize)> {
    let mut lo = 0;
    for _ in 1..line {
        lo += text.get(lo..)?.iter().position(|&c| c == b'\n')? + 1;
    }
    let hi = lo
        + text[lo..]
            .iter()
            .position(|&c| c == b'\n')
            .unwrap_or(text.len() - lo);
    let at = String::from_utf8_lossy(&text[lo..hi])
        .char_indices()
        .nth(col.saturating_sub(1))
        .map_or(hi, |(i, _)| lo + i);
    Some((lo, hi, at.min(hi)))
}

/// Whether pages are wanted as they are shipped (a viewer runs).
static TAPPING: AtomicBool = AtomicBool::new(false);
/// The build the pages shipped belong to (one more at each build's start
/// and end: a page of a build that ended is dropped).
static SERIAL: AtomicU64 = AtomicU64::new(0);
/// Where the pages shipped go: the viewer's thread that hashes them.
static TAP: Mutex<Option<mpsc::Sender<Tapped>>> = Mutex::new(None);

/// A stream shipped, of build `serial`: page `page`, or a form.
struct Tapped {
    serial: u64,
    page: Option<usize>,
    stream: ShippedStream,
}

/// Whether the hosts hand the viewer each stream as it is shipped
/// (`Host::wants_streams`).
pub fn tapping() -> bool {
    TAPPING.load(Ordering::Relaxed)
}

/// A stream shipped by the build running (`Host::stream_shipped`): page
/// `page` (from 0), or a form. Only queued: the engine goes on at once.
pub fn shipped(page: Option<usize>, stream: ShippedStream) {
    if let Some(tx) = lock(&TAP).as_ref() {
        let serial = SERIAL.load(Ordering::Relaxed);
        let _ = tx.send(Tapped {
            serial,
            page,
            stream,
        });
    }
}

/// How long a rebuild runs before the pages it ships are shown: one that
/// is in by then shows only its PDF's (a cold build's are shown at once).
const LIVE_AFTER: Duration = Duration::from_millis(250);

/// A page shipped by the build running, not yet in a PDF.
struct Live {
    /// Its hash as the browsers see it: the PDF reader's of its own PDF,
    /// marked as provisional (never a built page's).
    hash: u64,
    page: ShippedStream,
    /// The forms it draws (theirs too), as they were when it was shipped.
    forms: Arc<HashMap<i32, ShippedStream>>,
}

/// A file by its name and kind.
type FileKey = (Vec<u8>, partex_core::FileKind);

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
    /// `XeTeX`: each page's glyph runs (its glyphs are drawn from them).
    runs: Option<Arc<Vec<Vec<partex_xdvipdfmx::api::GlyphRun>>>>,
    /// The pages the build running shipped that differ from the last
    /// build's, by page; shown once `live_open`.
    live: Vec<Option<Live>>,
    live_open: bool,
    /// When the build running began.
    started: Option<Instant>,
}

impl Pages {
    /// Page `k`'s hash as the browsers see it: the build running's, if it
    /// shipped the page differently and it is shown, else the last
    /// build's.
    fn shown(&self, k: usize) -> Option<u64> {
        self.live_page(k)
            .map(|l| l.hash)
            .or_else(|| self.hashes.get(k).copied())
    }

    /// Page `k` of the build running, if it is shown.
    fn live_page(&self, k: usize) -> Option<&Live> {
        self.live_open
            .then(|| self.live.get(k).and_then(Option::as_ref))
            .flatten()
    }

    /// Every page's hash as the browsers see it (pages the build running
    /// shipped past the last build's end included; 0: none yet).
    fn shown_all(&self) -> Vec<u64> {
        let n = if self.live_open {
            self.hashes.len().max(self.live.len())
        } else {
            self.hashes.len()
        };
        (0..n).map(|k| self.shown(k).unwrap_or(0)).collect()
    }

    /// A build begins: nothing of it shown yet.
    fn begin(&mut self) {
        self.live.clear();
        self.live_open = false;
        self.started = Some(Instant::now());
    }
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
    /// The files pages shipped while the build runs need (their fonts'
    /// programs and encodings), read once.
    files: Mutex<HashMap<FileKey, Option<Arc<[u8]>>>>,
    /// `XeTeX`'s glyph runs' fonts, read once.
    faces: Mutex<phitex_draw::xetex::Faces>,
    /// The viewer's text fonts, by file name, read when first asked.
    text_fonts: Mutex<BTreeMap<String, Option<Arc<[u8]>>>>,
    /// Where the text fonts are looked up.
    host: Mutex<crate::native::NativeHost>,
    /// The watch's live area: lines for the user.
    live: Arc<crate::live::Live>,
    /// Log each page sent (`PARTEX_VIEW_LOG=1`).
    log: bool,
    building: AtomicBool,
    /// The directory the watch runs in: the project's files are named from
    /// it.
    root: PathBuf,
    /// Where a double-click's source is opened.
    editor: Option<crate::editor::Editor>,
    /// The last build's glyph origins, if it recorded them.
    origins: Mutex<Option<Arc<Origins>>>,
    /// When the last build was in, to time the pages sent after it.
    built_at: Mutex<Instant>,
    /// What the build running is doing, and the last `progress` sent.
    status: Mutex<(Status, Option<String>)>,
    /// The last build's diagnostics, a JSON array.
    diagnostics: Mutex<Arc<str>>,
    /// The PDF the builds write (a pass that settles is shown from it).
    pdf_path: Mutex<Option<PathBuf>>,
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
        editor: Option<crate::editor::Editor>,
    ) -> std::io::Result<View> {
        let port: u16 = std::env::var("PARTEX_VIEW_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let mut pages = Pages::default();
        // (the first build begins now)
        pages.begin();
        let shared = Arc::new(Shared {
            token: token(),
            pages: Mutex::new(pages),
            clients: Mutex::new(Vec::new()),
            fonts: Mutex::new(phitex_draw::Fonts::new()),
            files: Mutex::new(HashMap::new()),
            faces: Mutex::new(phitex_draw::xetex::Faces::new()),
            text_fonts: Mutex::new(BTreeMap::new()),
            host: Mutex::new(host),
            live,
            log: std::env::var("PARTEX_VIEW_LOG").is_ok_and(|v| v == "1"),
            building: AtomicBool::new(false),
            root: std::env::current_dir()?,
            editor,
            origins: Mutex::new(None),
            built_at: Mutex::new(Instant::now()),
            sent: AtomicU64::new(0),
            status: Mutex::new((Status::default(), None)),
            diagnostics: Mutex::new(Arc::from("[]")),
            pdf_path: Mutex::new(None),
        });
        crate::dpxfiles::keep_glyph_runs();
        let url = format!("http://127.0.0.1:{port}/{}/", shared.token);
        if let Some(f) = address_file(&shared.root) {
            let _ = f.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&f, &url);
        }
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("phitex-view-pages".into())
            .spawn(move || live_pages(&s, &rx))?;
        *lock(&TAP) = Some(tx);
        TAPPING.store(true, Ordering::Relaxed);
        let s = shared.clone();
        std::thread::Builder::new()
            .name("phitex-view-progress".into())
            .spawn(move || progress_ticks(&s))?;
        let s = shared.clone();
        std::thread::Builder::new()
            .name("phitex-view".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    // (small frames, answered at once: no Nagle delay)
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
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The last build's glyph origins (the browsers ask for them again).
    #[allow(
        dead_code,
        reason = "the watch runtimes record no origins yet (DESIGN 4.8)"
    )]
    pub fn set_origins(&self, o: Option<Origins>) {
        *lock(&self.shared.origins) = o.map(Arc::new);
        broadcast(&self.shared, "{\"event\":\"switched\"}");
    }

    /// What the build running is doing (`events::Progress`): sent to the
    /// browsers by the progress thread, at most ten times a second.
    pub fn progress(&self, p: &crate::events::Progress) {
        use crate::events::{Phase, Progress};
        let mut st = lock(&self.shared.status);
        match p {
            Progress::PassStart(n) => {
                st.0.pass = *n;
                st.0.phase = "typesetting";
                st.0.pages_base = partex_core::progress::BOARD.snapshot().pages;
            }
            Progress::Tool(_) => st.0.phase = "tool",
            Progress::Phase(ph) => {
                st.0.phase = match ph {
                    Phase::Cold => "typesetting",
                    Phase::Loading => "loading",
                    Phase::Linking => "linking",
                    Phase::Writing => "writing",
                    Phase::Saving => "saving",
                };
            }
            Progress::Superseded(_) => st.0.phase = "superseded",
            Progress::Again(_) | Progress::Pass(..) | Progress::Settling(_) => {}
        }
        drop(st);
        match p {
            Progress::Superseded(_) => self.superseded(),
            Progress::Settling(n) => self.settling(*n),
            _ => {}
        }
    }

    /// A rebuild began (`true`) or ended.
    pub fn building(&self, on: bool) {
        if on {
            SERIAL.fetch_add(1, Ordering::Relaxed);
            lock(&self.shared.pages).begin();
            *lock(&self.shared.status) = (Status::default(), None);
        }
        self.shared.building.store(on, Ordering::Relaxed);
        broadcast(
            &self.shared,
            &format!("{{\"event\":\"preparing\",\"on\":{on}}}"),
        );
    }

    /// A build is in: its PDF read back, the pages' hashes made, and the
    /// browsers told (they ask for the pages that changed).
    pub fn built(&self, b: &Built) {
        lock(&self.shared.pdf_path).clone_from(&b.pdf);
        self.read_pdf(b.pdf.as_ref(), b.history, b.ms);
        let items: Arc<str> = format!("[{}]", b.diagnostics.join(",")).into();
        *lock(&self.shared.diagnostics) = items.clone();
        *lock(&self.shared.status) = (Status::default(), None);
        broadcast(
            &self.shared,
            &format!("{{\"event\":\"diagnostics\",\"items\":{items}}}"),
        );
        broadcast(&self.shared, "{\"event\":\"settled\"}");
    }

    /// Pass `pass` of the build running wrote its outputs, and passes
    /// follow to settle it: its PDF (complete, of the build so far) is
    /// shown as a build's is, the browsers told it is settling (they ask
    /// for the pages that changed and stay `building`), and the next
    /// pass's pages are shown as they ship.
    pub fn settling(&self, pass: usize) {
        let pdf = lock(&self.shared.pdf_path).clone();
        let Some(pdf) = pdf else { return };
        let (history, ms) = {
            let p = lock(&self.shared.pages);
            (
                p.history,
                p.started.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1e3),
            )
        };
        let started = lock(&self.shared.pages).started;
        self.read_pdf(Some(&pdf), history, ms);
        // (still building: the progress goes on from when it began)
        {
            let mut p = lock(&self.shared.pages);
            p.begin();
            p.started = started.or(p.started);
        }
        broadcast(
            &self.shared,
            &format!("{{\"event\":\"settled\",\"settling\":true,\"pass\":{pass}}}"),
        );
    }

    /// Where the build running writes its PDF, before it is in (a pass
    /// that settles shows it: [`View::settling`]).
    pub fn set_pdf(&self, pdf: Option<PathBuf>) {
        *lock(&self.shared.pdf_path) = pdf;
    }

    /// A newer save stopped the build running: the browsers are told,
    /// and what it shipped of its pass is no longer shown.
    pub fn superseded(&self) {
        {
            let mut p = lock(&self.shared.pages);
            let started = p.started;
            p.begin();
            p.started = started.or(p.started);
        }
        SERIAL.fetch_add(1, Ordering::Relaxed);
        broadcast(&self.shared, "{\"event\":\"superseded\"}");
    }

    /// Read the PDF at `pdf` back and hash its pages (the build's
    /// `history`, after `ms`): the pages the browsers are to ask for.
    fn read_pdf(&self, pdf: Option<&PathBuf>, history: i32, ms: f64) {
        let t = Instant::now();
        // (the pages shipped since are this build's, in its PDF)
        SERIAL.fetch_add(1, Ordering::Relaxed);
        let data: Option<Arc<[u8]>> = pdf.and_then(|p| std::fs::read(p).ok()).map(Into::into);
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
        // (a run's draws depend on the runs, not only on the page's content)
        let runs = crate::dpxfiles::take_glyph_runs().map(Arc::new);
        if runs.is_some() || pages.runs.is_some() {
            pages.draws.clear();
        }
        pages.runs = runs;
        pages.draws.retain(|h, _| hashes.contains(h));
        pages.generation += 1;
        pages.pdf = data;
        pages.sums = sums;
        pages.history = history;
        pages.ms = ms;
        pages.live.clear();
        pages.live_open = false;
        pages.started = None;
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
    }
}

/// The progress thread: while a build runs, what it is doing pushed to
/// the browsers when it changed, ten times a second at most.
fn progress_ticks(s: &Shared) {
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let Some(started) = lock(&s.pages).started else {
            continue;
        };
        let board = partex_core::progress::BOARD.snapshot();
        let mut st = lock(&s.status);
        let phase = if board.finishing && st.0.phase == "typesetting" {
            "finishing"
        } else if st.0.phase.is_empty() {
            "typesetting"
        } else {
            st.0.phase
        };
        let pages = board.pages.saturating_sub(st.0.pages_base);
        let key = format!("{}:{pages}:{phase}", st.0.pass);
        if st.1.as_deref() == Some(key.as_str()) {
            continue;
        }
        st.1 = Some(key);
        let pass = st.0.pass.max(1);
        drop(st);
        broadcast(
            s,
            &format!(
                "{{\"event\":\"progress\",\"pass\":{pass},\"pages\":{pages},\"phase\":\"{phase}\",\"ms\":{:.0}}}",
                started.elapsed().as_secs_f64() * 1e3
            ),
        );
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
    let (pdf, h, runs, live) = {
        let p = lock(&s.pages);
        let h = p.shown(k)?;
        if let Some(d) = p.draws.get(&h) {
            return Some((d.clone(), h));
        }
        match p.live_page(k) {
            Some(l) => (None, h, None, Some((l.page.clone(), l.forms.clone()))),
            None => (Some(p.pdf.clone()?), h, p.runs.clone(), None),
        }
    };
    let t = Instant::now();
    // (`at`: the page in the PDF drawn)
    let (doc, at) = match (&pdf, live) {
        (Some(pdf), _) => (phitex_draw::Pdf::open(pdf)?, k),
        (None, Some((page, forms))) => {
            // (a page of the build running: from a PDF of its own, its
            // fonts' whole programs read here)
            let bytes = partex_core::pagepdf::page_pdf(
                &page,
                &|n| forms.get(&n).cloned(),
                &mut |name, kind| read_file(s, name, kind),
            );
            (phitex_draw::Pdf::open(&bytes.into())?, 0)
        }
        (None, None) => return None,
    };
    let d: Arc<str> = match &runs {
        // (`XeTeX`: the glyphs from the runs, the fonts read from their files)
        Some(runs) => {
            let page = runs.get(k).map_or(&[][..], Vec::as_slice);
            let mut faces = lock(&s.faces);
            let mut extra = |f0: usize, height: f64| {
                phitex_draw::xetex::extra(page, height, f0, &mut faces, &mut |f| {
                    std::fs::read(crate::native::path(f)).ok().map(Into::into)
                })
            };
            doc.draw_with(at, &mut lock(&s.fonts), Some(&mut extra))?
        }
        None => doc.draw(at, &mut lock(&s.fonts))?,
    }
    .into();
    if s.log {
        let after = lock(&s.built_at).elapsed();
        s.live.note(&format!(
            "viewer: page {} ({h:016x}) drawn in {:.1} ms, {} KB, sent {:.0} ms after {}",
            k + 1,
            t.elapsed().as_secs_f64() * 1e3,
            d.len() / 1024,
            after.as_secs_f64() * 1e3,
            if pdf.is_some() {
                "the build"
            } else {
                "the last build (shipped, the build running)"
            }
        ));
    }
    s.sent.fetch_add(1, Ordering::Relaxed);
    lock(&s.pages).draws.insert(h, d.clone());
    Some((d, h))
}

/// A file a page shipped while the build runs needs (a font's program or
/// encoding), found as the build finds it, read once.
fn read_file(s: &Shared, name: &[u8], kind: partex_core::FileKind) -> Option<Arc<[u8]>> {
    let key = (name.to_vec(), kind);
    if let Some(f) = lock(&s.files).get(&key) {
        return f.clone();
    }
    let f = partex_core::Host::read_file(&mut *lock(&s.host), name, kind).map(|f| f.contents);
    lock(&s.files).insert(key, f.clone());
    f
}

/// The viewer's thread for the pages shipped while a build runs: each
/// form kept, each page hashed (the PDF reader's hash of a PDF of its own,
/// without its fonts' programs, which no page's hash covers), and a page
/// that differs from the last build's pushed to the browsers, once the
/// build has run [`LIVE_AFTER`] (a cold build's at once).
fn live_pages(s: &Shared, rx: &mpsc::Receiver<Tapped>) {
    let mut forms: HashMap<i32, ShippedStream> = HashMap::new();
    loop {
        // (a rebuild's pages wait until it has run long enough)
        let wait = {
            let p = lock(&s.pages);
            match p.started {
                Some(t) if !p.live_open && p.live.iter().any(Option::is_some) => {
                    Some(LIVE_AFTER.saturating_sub(t.elapsed()))
                }
                _ => None,
            }
        };
        let got = match wait {
            Some(w) => rx.recv_timeout(w),
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let tapped = match got {
            Ok(t) => Some(t),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if let Some(t) = tapped
            && t.serial == SERIAL.load(Ordering::Relaxed)
        {
            match (t.page, t.stream.form()) {
                (Some(k), _) => live_page(s, &forms, k, t.stream),
                (None, Some(n)) => {
                    forms.insert(n, t.stream);
                }
                (None, None) => {}
            }
        }
        open_live(s);
    }
}

/// Page `k`, shipped by the build running: kept and pushed if it differs
/// from the last build's.
#[allow(clippy::many_single_char_names)]
fn live_page(s: &Shared, forms: &HashMap<i32, ShippedStream>, k: usize, page: ShippedStream) {
    // (the forms it draws, theirs too, as they are now)
    let mut mine = HashMap::new();
    let mut todo: Vec<i32> = page.forms().collect();
    while let Some(n) = todo.pop() {
        if mine.contains_key(&n) {
            continue;
        }
        if let Some(f) = forms.get(&n) {
            todo.extend(f.forms());
            mine.insert(n, f.clone());
        }
    }
    let bytes = partex_core::pagepdf::page_pdf(&page, &|n| mine.get(&n).cloned(), &mut |_, _| None);
    let Some(h) = phitex_draw::Pdf::open(&bytes.into()).and_then(|d| d.hashes().first().copied())
    else {
        return;
    };
    let mut p = lock(&s.pages);
    let before = p.shown(k);
    let live = (p.hashes.get(k) != Some(&h)).then(|| Live {
        // (never a built page's hash: the PDF's page replaces it)
        #[allow(clippy::cast_possible_truncation, reason = "a hash's low half")]
        hash: partex_core::persist_hash(&(h, "shipped")) as u64,
        page,
        forms: Arc::new(mine),
    });
    if p.live.len() <= k {
        p.live.resize_with(k + 1, || None);
    }
    p.live[k] = live;
    let after = p.shown(k);
    if p.live_open && after != before {
        let n = p.shown_all().len();
        let started = p.started;
        drop(p);
        push_page(s, k, after.unwrap_or(0), n, started);
    }
}

/// The pages shipped by the build running shown, once it has run
/// [`LIVE_AFTER`] (a cold build's at once): each pushed.
fn open_live(s: &Shared) {
    let mut p = lock(&s.pages);
    let due = p.started.is_some_and(|t| t.elapsed() >= LIVE_AFTER) || p.hashes.is_empty();
    if p.live_open || !due || p.live.iter().all(Option::is_none) {
        return;
    }
    p.live_open = true;
    let n = p.shown_all().len();
    let ks: Vec<(usize, u64)> = (0..p.live.len())
        .filter_map(|k| p.live_page(k).map(|l| (k, l.hash)))
        .collect();
    let started = p.started;
    drop(p);
    for (k, h) in ks {
        push_page(s, k, h, n, started);
    }
}

/// Tell the browsers page `k` is now at hash `h`, of `n` pages.
fn push_page(s: &Shared, k: usize, h: u64, n: usize, started: Option<Instant>) {
    if s.log {
        s.live.note(&format!(
            "viewer: page {} shipped, {:.0} ms into the build",
            k + 1,
            started.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1e3)
        ));
    }
    broadcast(
        s,
        &format!("{{\"event\":\"page\",\"k\":{k},\"hash\":\"{h:016x}\",\"pages\":{n}}}"),
    );
}

/// The reply to request `req` (`session.ts`'s `CoreRes`, with its `id`).
fn answer(s: &Shared, req: &Value) -> String {
    let id = req.get("id").and_then(Value::index).unwrap_or(0);
    let op = req.get("op").and_then(Value::str).unwrap_or("");
    let page = req.get("page").and_then(Value::index);
    let state = || {
        let p = lock(&s.pages);
        (p.shown_all(), p.history, p.ms)
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
        "origins" => {
            let o = lock(&s.origins).clone();
            let _ = write!(
                out,
                "true,\"json\":{}}}",
                origins_json(s, o.as_deref(), page)
            );
        }
        "source" => {
            let file = req.get("file").and_then(Value::str).unwrap_or("");
            let start = req.get("start").and_then(Value::index).unwrap_or(0);
            // (an error's place: a line and column, not a byte)
            let at = req
                .get("line")
                .and_then(Value::index)
                .map(|l| (l, req.get("col").and_then(Value::index).unwrap_or(1)));
            let _ = write!(out, "{}}}", to_source(s, file, start, at));
        }
        "diagnostics" => {
            let d = lock(&s.diagnostics).clone();
            let _ = write!(out, "true,\"json\":{{\"items\":{d}}}}}");
        }
        // (the document's outline, its bookmarks, from the last build's
        // PDF: `[{"t":title,"p":page,"y":top,"k":[…]}]`, for a contents
        // sidebar; `[]` if it has none or there is no PDF yet)
        "outline" => {
            let pdf = lock(&s.pages).pdf.clone();
            let items = pdf
                .as_ref()
                .and_then(phitex_draw::Pdf::open)
                .map_or_else(|| "[]".to_owned(), |d| d.outline());
            let _ = write!(out, "true,\"json\":{{\"items\":{items}}}}}");
        }
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

/// Page `page`'s glyphs with their sources (`{"files":[…],"g":[[x, y,
/// file, start, end, synthesized],…]}`, file -1: none), as the
/// extension core's `origins` gives them; none without origins.
fn origins_json(s: &Shared, o: Option<&Origins>, page: Option<usize>) -> String {
    let (Some(o), Some(k)) = (o, page) else {
        return "{\"files\":[],\"g\":[]}".into();
    };
    let mut out = String::from("{\"files\":[");
    for (i, f) in o.files.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let mut b = Vec::new();
        json_str(&mut b, f);
        out.push_str(&String::from_utf8_lossy(&b));
    }
    out.push_str("],\"g\":[");
    for (i, &(x, y, f, a, b, synth)) in o
        .pages
        .get(k)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .enumerate()
    {
        if i > 0 {
            out.push(',');
        }
        let f = if f == u32::MAX { -1 } else { i64::from(f) };
        let _ = write!(out, "[{x:.2},{y:.2},{f},{a},{b},{}]", u8::from(synth));
    }
    out.push_str("]}");
    let _ = s;
    out
}

/// A double-click's source, byte `start` of `file` (from the watch's
/// directory), or an error's line and column `at`: shown, and opened in
/// the editor if there is one. The reply's tail (`ok` and the rest).
fn to_source(s: &Shared, file: &str, start: usize, at: Option<(usize, usize)>) -> String {
    let path = s.root.join(file);
    if file.is_empty() || file.starts_with('/') || file.contains("..") {
        return "false,\"error\":\"not a file of the project\"".into();
    }
    let Ok(text) = std::fs::read(&path) else {
        return "false,\"error\":\"no such file\"".into();
    };
    let (line, col) = at.unwrap_or_else(|| line_col(&text, start));
    let place = format!("{file}:{line}:{col}");
    match &s.editor {
        Some(e) => match e.open(file, line, col) {
            Ok(()) => s
                .live
                .note(&format!("source: {place} (opened in the editor)")),
            Err(err) => s.live.warn("Editor", &format!("{place}: {err}")),
        },
        None => s.live.note(&format!(
            "source: {place} (--editor or PHITEX_EDITOR opens it)"
        )),
    }
    let mut b = Vec::new();
    json_str(&mut b, &place);
    format!(
        "true,\"json\":{{\"place\":{}}}",
        String::from_utf8_lossy(&b)
    )
}

/// Forward search: `sync?file=F&line=L[&col=C]`, the browsers told to
/// show the glyphs of that line. The status and the answer.
fn forward(s: &Shared, req: &ws::Request) -> (&'static str, String) {
    let file = req.param("file").unwrap_or_default();
    let line: usize = req.param("line").and_then(|l| l.parse().ok()).unwrap_or(1);
    let col: usize = req.param("col").and_then(|c| c.parse().ok()).unwrap_or(1);
    // (a path as given, absolute or from the directory `phitex sync` ran in,
    // named from the watch's)
    let path = std::fs::canonicalize(&file).or_else(|_| std::fs::canonicalize(s.root.join(&file)));
    let root = std::fs::canonicalize(&s.root).unwrap_or_else(|_| s.root.clone());
    let Some(rel) = path.ok().and_then(|p| {
        p.strip_prefix(&root)
            .ok()
            .map(|r| r.to_string_lossy().into_owned())
    }) else {
        return (
            "404 Not Found",
            format!(
                "{file}: not a file of the project in {}\n",
                s.root.display()
            ),
        );
    };
    let Some((lo, hi, at)) = std::fs::read(root.join(&rel))
        .ok()
        .and_then(|t| line_bytes(&t, line, col))
    else {
        return ("404 Not Found", format!("{rel}: no line {line}\n"));
    };
    let mut b = Vec::new();
    json_str(&mut b, &rel);
    let ev = format!(
        "{{\"event\":\"sync\",\"file\":{},\"lo\":{lo},\"hi\":{hi},\"at\":{at}}}",
        String::from_utf8_lossy(&b)
    );
    let n = {
        let mut cs = lock(&s.clients);
        cs.retain(|c| c.send(&ev));
        cs.len()
    };
    if n == 0 {
        return ("503 Service Unavailable", "no viewer is open\n".into());
    }
    ("200 OK", format!("{rel}:{line} shown in {n} viewer(s)\n"))
}

/// `phitex sync FILE:LINE[:COL]`: the viewer of the watch running here
/// shows that line. The exit status.
pub fn sync_command(place: &str) -> i32 {
    let parsed = place.rsplit_once(':').and_then(|(rest, last)| {
        let last: usize = last.parse().ok()?;
        Some(
            match rest.rsplit_once(':').map(|(f, l)| (f, l.parse::<usize>())) {
                Some((f, Ok(l))) => (f.to_owned(), l, last),
                _ => (rest.to_owned(), last, 1),
            },
        )
    });
    let Some((file, line, col)) = parsed else {
        eprintln!("phitex sync: give FILE:LINE[:COL]");
        return 2;
    };
    let file = std::fs::canonicalize(&file).map_or(file, |p| p.to_string_lossy().into_owned());
    let here = std::env::current_dir().unwrap_or_default();
    // (the watch of this directory, or of one above)
    let url = here
        .ancestors()
        .find_map(|d| address_file(d).and_then(|f| std::fs::read_to_string(f).ok()));
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
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return 1;
    };
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
        // (ç is two bytes, one column)
        assert_eq!(line_col(t, 6), (2, 3));
        assert_eq!(line_bytes(t, 2, 3), Some((3, 8, 6)));
        assert_eq!(line_bytes(t, 3, 1), Some((9, 9, 9)));
        assert_eq!(line_bytes(t, 4, 9), Some((10, 14, 14)));
        assert_eq!(line_bytes(t, 9, 1), None);
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
