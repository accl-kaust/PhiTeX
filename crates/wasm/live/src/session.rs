//! The open document and what the host's exports ([`crate::abi`]) call:
//! each `ph_*` export's work, on slices and strings, safe.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use phitex_live::document::Document;
use phitex_live::lsp::Server;
use phitex_live::session::{Job, Session, Status};
use phitex_live::view::Viewer;
use phitex_shelf::{self as shelf, Fetch};

use serde_json::{Value, json};

use crate::files::Files;
use crate::{THREADED, abi, files, live, shell, store};

/// The host, as the files ask it.
struct Asks;

impl files::Host for Asks {
    fn want(&self, name: &str) {
        abi::want(name);
    }

    fn fetch(&self, pack: &str) {
        abi::fetch(pack);
    }
}

static ASKS: Asks = Asks;

fn event_text(s: &str) {
    abi::event(s);
}

fn event(v: &Value) {
    event_text(&v.to_string());
}

fn shell_hook() -> &'static Arc<shell::Hook> {
    static HOOK: OnceLock<Arc<shell::Hook>> = OnceLock::new();
    HOOK.get_or_init(|| Arc::new(shell::Hook::new(abi::shell)))
}

fn files() -> &'static Arc<Files> {
    static FILES: OnceLock<Arc<Files>> = OnceLock::new();
    FILES.get_or_init(|| {
        let f = Arc::<Files>::default();
        f.set_host(&ASKS);
        f
    })
}

fn kept() -> &'static Arc<store::Kept> {
    static KEPT: OnceLock<Arc<store::Kept>> = OnceLock::new();
    KEPT.get_or_init(|| Arc::new(store::Kept::new(abi::store_get, abi::store_put)))
}

/// The bytes the last answer left for the host.
pub(crate) static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The last build's main output: its extension and bytes.
type Output = Mutex<Option<(&'static str, Arc<Vec<u8>>)>>;

/// The open document, as thread 0 reaches it: its viewer (requests
/// answered, asks queued), its outputs, and its session thread's wake.
struct Open {
    viewer: Arc<Viewer>,
    lsp: Arc<Server>,
    sink: Arc<live::Memory>,
    wake: Arc<Wake>,
    output: Arc<Output>,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

/// What wakes the session thread: something asked (`.0`), or the
/// document closed (`.1`).
#[derive(Default)]
struct Wake {
    state: Mutex<(bool, bool)>,
    cv: Condvar,
}

impl Wake {
    fn set(&self, closed: bool) {
        let mut s = lock(&self.state);
        if closed {
            s.1 = true;
        } else {
            s.0 = true;
        }
        drop(s);
        self.cv.notify_all();
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The host's answer to `want`: `found` 0, the file is nowhere; else its
/// path (`path..`, UTF-8) and bytes.
pub(crate) fn provide(name: &str, file: Option<(String, Vec<u8>)>) {
    files().provide(name, file);
}

/// The host's answer to `fetch`: Shelf's pack `id` gunzipped (natively, by
/// the host, off this module), or `ok` 0 (it could not be had).
pub(crate) fn pack(id: &str, data: Option<&[u8]>) {
    files().pack(id, data);
}

/// The host's answer to `store_get`: the bytes it stored under `key`, or
/// `found` 0 (none).
pub(crate) fn stored(key: &str, data: Option<&[u8]>) {
    kept().stored(key, data);
}

/// The host's answer to `\write18` call `id` (its frame, shell.rs).
pub(crate) fn shell_done(id: u32, frame: &[u8]) {
    shell_hook().done(id, frame);
}

/// Shelf's index (its TSV, gunzipped), `release.json` and the release's
/// pack table (its TSV, gunzipped: every pack checked against it before
/// use), and the names of the extension's bundled texmf/ (one a line):
/// names are resolved here from now on. 0: done; 1: `release.json` is not
/// JSON; 2: the pack table is not one (why in the answer's bytes).
pub(crate) fn set_index(tsv: &[u8], release: &[u8], table: &[u8], bundled: &str) -> i32 {
    let Ok(release) = serde_json::from_slice::<Value>(release) else {
        return 1;
    };
    let index = match shelf::Index::new(&String::from_utf8_lossy(tsv), &release)
        .with_pack_table(&String::from_utf8_lossy(table))
    {
        Ok(ix) => ix,
        Err(e) => {
            *lock(&OUTPUT) = e.into_bytes();
            return 2;
        }
    };
    let names = bundled
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    files().set_index(Some(index), names);
    0
}

/// Opens a document: JSON `{main, text, engine?, threads?, ini?, shell?}`
/// (`text`: the main file's text, as an editor holds it; `engine`: the
/// program, pdflatex by default, latex, xelatex, pdftex, tex...; `ini`:
/// the text is the whole job, no format; `shell`: the host runs
/// `\write18`'s allowed commands, else they are refused). The project's
/// other files are given before (`ph_provide`). Answered at once (0; 1:
/// not JSON; 2: no session thread; 3: an engine not known, why in the
/// answer's bytes); the session thread builds and tells its events. A
/// document open before is closed.
pub(crate) fn open(req: &[u8]) -> i32 {
    let Ok(req) = serde_json::from_slice::<Value>(req) else {
        return 1;
    };
    let main = req["main"].as_str().unwrap_or("main.tex").to_owned();
    let body = req["text"].as_str().unwrap_or("");
    let threads = req["threads"]
        .as_u64()
        .and_then(|t| usize::try_from(t).ok());
    let ini = req["ini"].as_bool().unwrap_or(false);
    // (the project's .latexmkrc, as Overleaf reads it: its search paths
    // always, its engine and SyncTeX with `exact`, never -shell-escape)
    let rc = latexmk(&req);
    let engine_name = req["engine"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| rc.engine.clone())
        .unwrap_or_else(|| "pdflatex".to_owned());
    let engine = match phitex_live::tex::engine(&engine_name) {
        Ok(e) => e,
        Err(e) => {
            *lock(&OUTPUT) = e.into_bytes();
            return 3;
        }
    };
    let shell = req["shell"].as_bool().unwrap_or(false);
    close();
    spawn_workers();
    // (the main file is the project's too: what the editor's edits apply
    // to before it has a buffer, and a double-click's source)
    files().provide(&main, Some((format!("./{main}"), body.as_bytes().to_vec())));
    let host = Arc::new(live::Shelf {
        files: files().clone(),
        engine: shelf::engine(engine.name),
        shell: shell.then(|| Arc::clone(shell_hook()) as Arc<dyn phitex_tex::ShellHook>),
    });
    let sink = Arc::new(live::Memory::default());
    let mut job = job(&main, engine, ini, threads);
    if let Some(on) = rc.synctex {
        job.synctex = on;
    }
    files().set_search(rc.search);
    if !rc.notes.is_empty() {
        event(&json!({"event": "latexmkrc", "notes": rc.notes}));
    }
    let mut session = Session::new(
        job,
        Arc::from(phitex_live::watch::tex_text(body)),
        host,
        Some(kept().clone() as Arc<dyn phitex_live::host::Store>),
        sink.clone(),
        Arc::new(Status::default()),
    );
    // (as the watch's: no log laid out after each build)
    session.watching = true;
    // (Shelf's fetches, the build's progress: the loading card's packages, $/progress, a
    // pack that could not be had said)
    let meter = session.status().meter().clone();
    files().on_fetch(Some(Arc::new(move |f: &Fetch<'_>| match *f {
        Fetch::Asked { name, pack } => meter.fetch(name, pack, false),
        Fetch::Arrived { name, pack } => meter.fetch(name, pack, true),
        Fetch::Failed { why, .. } => meter.unreachable(why),
    })));
    let doc = Document::new(session, &main);
    // (its language server: the editor's messages in by `ph_lsp`, the server's out to the
    // host; what only the host does, an event to it)
    let lsp = doc.lsp(
        Box::new(|m: &Value| abi::lsp(&m.to_string())),
        Some(Box::new(|command: &str, arg: &Value| match command {
            // (stopped: the server has said so; the host closes the document)
            "phitex.stop" => {
                event(&json!({"event": "stop"}));
                Some(Ok(Value::Null))
            }
            // (threads, main or files changed: the host opens the document again with them)
            "phitex/configure" => {
                event(&json!({"event": "configure", "settings": arg}));
                Some(Ok(Value::Null))
            }
            // (the caches are the host's: it answers clearCache and cacheStats before they come here)
            _ => None,
        })),
    );
    let viewer = doc.viewer().clone();
    // (the viewer's events to the host as they happen, not at the step's
    // end: `preparing` while the build runs)
    viewer.on_emit(Box::new(event_text));
    let wake = Arc::new(Wake::default());
    let w = wake.clone();
    viewer.on_ask(Box::new(move || w.set(false)));
    viewer.on_read(Box::new(|name| files().read(name).map(|b| b.to_vec())));
    // (a source place, a double-click's: shown in the editor, window/showDocument)
    let server = lsp.clone();
    viewer.on_goto(Box::new(move |file, line, col, _place| {
        server.show(file, line, col);
    }));
    // (text selected on the pages: its source selected in the editor, the
    // focus left on the pages, window/showDocument with the range)
    let server = lsp.clone();
    viewer.on_select(Box::new(move |file, from, to| {
        server.select(file, from, to);
    }));
    let output = Arc::new(Output::default());
    if THREADED {
        let (w, o) = (wake.clone(), output.clone());
        // (the document runs here, as on a native main thread: its stack as
        // large; a stack has no guard page in wasm, an overflow writes over
        // the heap)
        let started = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || run(doc, &w, &o));
        if started.is_err() {
            return 2;
        }
    } else {
        // (no threads: kept for the host to step, the cold build its first step)
        wake.set(false);
        *lock(&STEPPED) = Some(doc);
    }
    *lock(&OPEN) = Some(Open {
        viewer,
        lsp,
        sink,
        wake,
        output,
    });
    0
}

/// What a project's latexmk rc files give the open document.
#[derive(Debug, Default)]
struct Latexmk {
    /// TEXINPUTS's search (none: the default).
    search: Option<crate::files::Search>,
    /// `exact`: the engine ($pdf_mode, $pdflatex) and SyncTeX.
    engine: Option<String>,
    synctex: Option<bool>,
    /// What was not applied, each a line naming the file and the line.
    notes: Vec<String>,
}

/// The working directory the rc files are read in: Overleaf's compile
/// directory (latexmk runs there; `abs_path` answers under it). Folders
/// under it are the project's.
const COMPILE_DIR: &str = "/compile";

/// The open request's `latexmk` (`{files: {path: text}, exact?}`: the
/// project's rc files by path from its root, `latexmkrc` or `.latexmkrc`
/// there read first, any file one of them `do`es given too) and
/// `projectFiles` (every path of the project: a `//` folder's subfolders).
fn latexmk(req: &Value) -> Latexmk {
    use phitex_live::latexmkrc as rc;
    struct Given<'a>(&'a serde_json::Map<String, Value>);
    impl rc::RcFiles for Given<'_> {
        fn read(&self, path: &str) -> Option<String> {
            let rel = path.strip_prefix(COMPILE_DIR)?.strip_prefix('/')?;
            self.0.get(rel)?.as_str().map(str::to_owned)
        }
    }
    let Some(given) = req["latexmk"]["files"].as_object() else {
        return Latexmk::default();
    };
    let exact = req["latexmk"]["exact"].as_bool().unwrap_or(false);
    let w = rc::Where {
        cwd: COMPILE_DIR.into(),
        ..rc::Where::default()
    };
    let r = rc::read(&rc::order(&w), COMPILE_DIR, &BTreeMap::new(), &Given(given));
    let mut out = Latexmk {
        notes: r.notes.iter().map(ToString::to_string).collect(),
        ..Latexmk::default()
    };
    if let Some((_, at)) = &r.default_files {
        out.notes.push(format!(
            "{at}: @default_files not applied: the project's main document is the one built"
        ));
    }
    if exact {
        let a = r.exact();
        out.notes.extend(a.notes.iter().map(ToString::to_string));
        out.engine = a.engine;
        out.synctex = a.synctex;
        if let Some((true, at)) = a.shell_escape {
            out.notes.push(format!(
                "{at}: -shell-escape not applied: never from a .latexmkrc in the browser"
            ));
        }
        for (d, at) in [a.out_dir, a.aux_dir].into_iter().flatten() {
            out.notes.push(format!(
                "{at}: {d}: not applied: the browser keeps the outputs in memory"
            ));
        }
    } else {
        out.notes
            .extend(r.not_applied(Some("the exact latexmk setting")));
    }
    if let Some(v) = r.var("TEXINPUTS") {
        let project = req["projectFiles"].as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|p| p.trim_start_matches("./").to_owned())
                .collect::<Vec<_>>()
        });
        let mut elements = Vec::new();
        for e in rc::elements(v) {
            match e {
                rc::Element::Default => elements.push(e),
                rc::Element::Dir { path, sub } => {
                    let rel = if path == COMPILE_DIR {
                        Some(String::new())
                    } else {
                        path.strip_prefix(COMPILE_DIR)
                            .and_then(|r| r.strip_prefix('/'))
                            .map(str::to_owned)
                    };
                    match rel {
                        Some(rel) => {
                            if sub && project.is_none() {
                                out.notes.push(format!(
                                    "TEXINPUTS: {path}//: its subfolders not searched (the host \
                                     gave no projectFiles)"
                                ));
                            }
                            elements.push(rc::Element::Dir { path: rel, sub });
                        }
                        None => out.notes.push(format!(
                            "TEXINPUTS: {path}: not searched: outside the project"
                        )),
                    }
                }
            }
        }
        out.search = Some(crate::files::Search { elements, project });
    }
    for v in ["BIBINPUTS", "BSTINPUTS"] {
        if r.var(v).is_some() {
            out.notes.push(format!(
                "{v} not applied: the browser finds BibTeX's files in the project's root and Shelf"
            ));
        }
    }
    out
}

/// The job a document is opened with: the watch's, its outputs in memory.
fn job(main: &str, engine: phitex_live::tex::Engine, ini: bool, threads: Option<usize>) -> Job {
    Job {
        engine,
        ini,
        etex: engine.etex,
        file: main.to_owned(),
        path: PathBuf::from(main),
        jobname: main.strip_suffix(".tex").unwrap_or(main).to_owned(),
        out_dir: PathBuf::from("."),
        threads,
        spec: None,
        wait_long: true,
        node_records: false,
        synctex: false,
        // (the viewer's `origins`: a double-click's source, forward search)
        origins: true,
        recorder: false,
        shell_escape: false,
        // (as SOURCE_DATE_EPOCH=0: the same bytes every build)
        pdf_date: 0,
        // (the files read back, .aux and .toc, to their fixpoint, as the
        // watch runs them)
        fixpoint: true,
        cache_limit: None,
        edit_diagnostics: true,
        inputs: Vec::new(),
    }
}

/// The session thread: the document stepped whenever something is asked,
/// its events to the host, until it is closed. (The records a build
/// changed are stored as it settles, by the document: nothing is left
/// for a quiet moment.)
fn run(mut doc: Document, wake: &Wake, output: &Output) {
    loop {
        step(&mut doc, output);
        let mut s = lock(&wake.state);
        // (work nobody waits for, the build's origins: the next step's,
        // unless something was asked first)
        while !s.0 && !s.1 && !doc.pending() {
            s = wake.cv.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
        if s.1 {
            return;
        }
        s.0 = false;
    }
}

/// One step of the document (phitex-live's `Document::step`): what was
/// asked taken, built if anything changed, its events to the host.
fn step(doc: &mut Document, output: &Output) {
    let events = doc.step();
    if events
        .iter()
        .any(|e| e["event"] == "build" || e["event"] == "failed")
    {
        *lock(output) = doc.output();
    }
    for e in &events {
        event(e);
    }
}

/// The open document, without threads: stepped by the host (`ph_step`).
static STEPPED: Mutex<Option<Document>> = Mutex::new(None);

/// Without threads (wasm32-wasip1), the host's turn of the session thread:
/// the open document stepped once if something was asked since (its first
/// step the cold build) or work nobody waits for is left (the build's
/// origins, a step of their own after the build's), its events told.
/// 1: stepped, step again soon (until 0); 2: stepped, and only work
/// nobody waits for is left: step again once the requests that came are
/// answered (the pages asked after a build drawn before its origins are
/// made); 0: nothing asked, or no document; with threads, always 0 (the
/// session thread steps it).
pub(crate) fn step_once() -> i32 {
    let Some((wake, output)) = lock(&OPEN)
        .as_ref()
        .map(|o| (o.wake.clone(), o.output.clone()))
    else {
        return 0;
    };
    let pending = lock(&STEPPED).as_ref().is_some_and(Document::pending);
    {
        let mut s = lock(&wake.state);
        if s.1 || (!s.0 && !pending) {
            return 0;
        }
        s.0 = false;
    }
    // (taken out while it runs: an ask the host answers in its call may lock the rest)
    let Some(mut doc) = lock(&STEPPED).take() else {
        return 0;
    };
    step(&mut doc, &output);
    let later = doc.pending() && !lock(&wake.state).0;
    *lock(&STEPPED) = Some(doc);
    if later { 2 } else { 1 }
}

/// The open document closed: its session thread ends after the step it
/// is in (a build runs to its end).
pub(crate) fn close() {
    if let Some(o) = lock(&OPEN).take() {
        o.wake.set(true);
    }
    lock(&STEPPED).take();
}

/// A viewer's request (JSON, `{"id","op",...}`), answered: the reply
/// (`{"id","ok","json"|"error"}`) in the answer's bytes. 0: answered; 1:
/// not JSON; 2: no document open.
pub(crate) fn request(req: &[u8]) -> i32 {
    let Ok(req) = serde_json::from_slice::<Value>(req) else {
        return 1;
    };
    let Some(viewer) = lock(&OPEN).as_ref().map(|o| o.viewer.clone()) else {
        return 2;
    };
    *lock(&OUTPUT) = viewer.answer(&req).into_bytes();
    0
}

/// A message from the editor to the document's language server (JSON-RPC,
/// unframed): its answers and notifications go out by `phitex.lsp`. 0:
/// taken; 1: not JSON; 2: no document open; 3: taken, and it was `exit`
/// (the host closes the document).
pub(crate) fn lsp_message(m: &[u8]) -> i32 {
    let Ok(m) = serde_json::from_slice::<Value>(m) else {
        return 1;
    };
    let Some(server) = lock(&OPEN).as_ref().map(|o| o.lsp.clone()) else {
        return 2;
    };
    if server.handle(&m) { 0 } else { 3 }
}

/// The last build's main output in the answer's bytes: 1 a PDF, 2 a DVI,
/// 0 none (no build in, no pages).
pub(crate) fn main_output() -> i32 {
    let out = lock(&OPEN).as_ref().and_then(|o| lock(&o.output).clone());
    let Some((ext, b)) = out else {
        lock(&OUTPUT).clear();
        return 0;
    };
    *lock(&OUTPUT) = b.to_vec();
    if ext == "pdf" { 1 } else { 2 }
}

/// The PDF the viewer is shown (`doc.pdf`: the last build's; a DVI's only
/// where it was made one, not here yet) in the answer's bytes: 1, or 0
/// (none).
pub(crate) fn pdf() -> i32 {
    let pdf = lock(&OPEN).as_ref().and_then(|o| o.viewer.pdf());
    let found = pdf.is_some();
    *lock(&OUTPUT) = pdf.map(|b| b.to_vec()).unwrap_or_default();
    i32::from(found)
}

/// A file the build wrote (`name`: `doc.aux`, the
/// main output) in the answer's bytes: 1, or 0 (not written).
pub(crate) fn file(name: &str) -> i32 {
    let f = lock(&OPEN).as_ref().and_then(|o| o.sink.file(name));
    let found = f.is_some();
    *lock(&OUTPUT) = f.map(|b| b.to_vec()).unwrap_or_default();
    i32::from(found)
}

/// The strips of `keys` (hex, comma-separated, as a `frame` request's
/// bands name them: phitex-live's `Viewer::strips`) in the answer's bytes,
/// each its width and height (little-endian u32) then its RGBA pixels, in
/// the order asked (0 by 0: one no longer kept): 1, or 0 (no document).
pub(crate) fn strips(keys: &str) -> i32 {
    let Some(viewer) = lock(&OPEN).as_ref().map(|o| o.viewer.clone()) else {
        return 0;
    };
    *lock(&OUTPUT) = viewer.strips(keys);
    1
}

/// Page `page`'s frame at `scale` pixels a point with the strips the
/// viewer lacks (JSON `{page, scale, have, win?}`, `have` the keys it holds,
/// comma-separated, `win` the part of the page: phitex-live's
/// `Viewer::frame_with_in`) in the answer's
/// bytes: the frame JSON's length (little-endian u32), the JSON (its
/// `sent` the strips' keys), the strips as `ph_strips` gives them. 1, or 0
/// (no such page, no document, not JSON).
pub(crate) fn frame(req: &[u8]) -> i32 {
    let Ok(req) = serde_json::from_slice::<Value>(req) else {
        return 0;
    };
    let Some(viewer) = lock(&OPEN).as_ref().map(|o| o.viewer.clone()) else {
        return 0;
    };
    let page = req["page"]
        .as_u64()
        .and_then(|k| usize::try_from(k).ok())
        .unwrap_or(0);
    #[allow(clippy::cast_possible_truncation, reason = "a scale: pixels a point")]
    let scale = req["scale"].as_f64().unwrap_or(1.0) as f32;
    // (`win`: only that part of the page, fractions: left, top, right, bottom)
    #[allow(clippy::cast_possible_truncation, reason = "fractions of a page")]
    let win = req["win"]
        .as_array()
        .filter(|w| w.len() == 4)
        .and_then(|w| {
            let f = |i: usize| w[i].as_f64().map(|x| x as f32);
            Some([f(0)?, f(1)?, f(2)?, f(3)?])
        });
    match viewer.frame_with_in(page, scale, req["have"].as_str().unwrap_or(""), win) {
        Some(b) => {
            *lock(&OUTPUT) = b;
            1
        }
        None => 0,
    }
}

/// Every thread's stack: a native process's main thread's (wasm has no
/// guard page: an overflow would write over the heap unseen).
const STACK: usize = 8 << 20;

/// The engine's workers are std's threads, which wasi-threads starts as
/// the host's workers (set before the first build; a reactor runs no
/// `main`).
fn spawn_workers() {
    // (none without threads: the engine runs its work in place)
    if !THREADED {
        return;
    }
    phitex_tex::set_spawn_worker(Box::new(|f| {
        std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(f)
            .expect("a worker thread");
    }));
}
