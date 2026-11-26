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

/// The event told when the document outgrows the core's memory
/// (`heap.rs`): the core aborts next. A static text: told with the
/// allocator's lock held.
pub(crate) const LOST_MEMORY: &str = r#"{"event":"lost","why":"memory","message":"The document needs more memory than PhiTeX's engine has here: the build stopped."}"#;

/// Every panic told to the host before the core aborts (the wasm core is
/// built with `panic = "abort"`: a panic anywhere ends it), as the event
/// `{"event":"lost","why":"panic","message":…}`: a host whose threads'
/// trap does not reach it still says why the document is gone. Then the
/// default hook (the message on stderr).
fn tell_panics() {
    static SET: std::sync::Once = std::sync::Once::new();
    SET.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            event(&json!({"event": "lost", "why": "panic", "message": info.to_string()}));
            default(info);
        }));
    });
}

/// The least time between two of the host's turns while a step runs
/// ([`let_host_in`]).
const TURN_EVERY: std::time::Duration = std::time::Duration::from_millis(40);

/// Without threads, while a step builds: the host's turn, at most every
/// [`TURN_EVERY`], as the import `phitex.yield()` (no arguments; no
/// event). A host that switches stacks (JSPI: `ph_step` entered as
/// `WebAssembly.promising`, the `yield` import a `WebAssembly.Suspending`
/// that answers with a promise) answers the requests that came meanwhile (`ph_request`,
/// `ph_frame`, `ph_strips`, `ph_lsp`) before the step goes on: the pages
/// the build streamed drawn while it runs, as the old core's slices let
/// it. A host that does not makes `yield` a no-op. Called with no lock held
/// that those answers take (the viewer's, the files', the open
/// document's): the engine's tick ([`phitex_tex::Progress::on_tick`])
/// and the files read ([`crate::live::Shelf`]).
pub(crate) fn let_host_in() {
    if THREADED || !STEPPING.load(std::sync::atomic::Ordering::Relaxed) || !ON_STEP.get() {
        return;
    }
    let now = std::time::Instant::now();
    if LAST.with(|l| l.get().is_some_and(|t| now - t < TURN_EVERY)) {
        return;
    }
    abi::yield_turn();
    LAST.with(|l| l.set(Some(std::time::Instant::now())));
}

thread_local! {
    /// `ph_step`'s own build is what runs now, not a call the host made
    /// in its turn while that build was suspended ([`host_turn`]): only
    /// the build may yield. A JSPI host suspends at a yield only inside
    /// `ph_step` (entered through `WebAssembly.promising`); a yield made
    /// inside another export (`ph_provide` reading the files, as it gave
    /// one the build looked for, in the host's task between its turn's
    /// end and the build's resumption) suspended outside it: the
    /// browser's SuspendError, the open lost (a .latexmkrc's `//` search
    /// path, Chrome). (The yield is its own import, never `event`: V8
    /// throws at a Suspending import called outside `promising`, so the
    /// events every export tells go through a plain one.)
    static ON_STEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A host's call (every export but `ph_step`): no yield made while it
/// runs ([`ON_STEP`]), the build's own state given back after it.
pub(crate) fn host_turn<R>(f: impl FnOnce() -> R) -> R {
    let was = ON_STEP.replace(false);
    let r = f();
    ON_STEP.set(was);
    r
}

thread_local! {
    /// The host's last turn, or the step's start ([`let_host_in`]): a
    /// step shorter than [`TURN_EVERY`] (an edit's) gives none.
    static LAST: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// Without threads: a step runs ([`step_once`]). The host's calls made
/// in its turns ([`let_host_in`]) find the document out of its place:
/// a step asked then waits for this one's end, an open is refused, a
/// close drops the document once the step is done.
static STEPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// The host takes back the file it gave as `name` (deleted from the
/// project): looked up from now on as if never given, by the default
/// search (Shelf's, where the project's copy shadowed one), the host
/// asked again where there is no index ([`Files::withdraw`]). A Rebuild,
/// or the viewer's `changed` of the name, then builds without it.
pub(crate) fn withdraw(name: &str) {
    files().withdraw(name);
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

/// Opens a document: JSON `{main, text, engine?, threads?, ini?, shell?,
/// date?}` (`text`: the main file's text, as an editor holds it;
/// `engine`: the program, pdflatex by default, latex, xelatex, pdftex,
/// tex...; `ini`: the text is the whole job, no format; `shell`: the host
/// runs `\write18`'s allowed commands, else they are refused; `date`: the
/// job's local date and time as seconds whose UTC fields they are, 0 for
/// TeX Live's `SOURCE_DATE_EPOCH=0`, else now in UTC). The project's
/// other files are given before (`ph_provide`). Answered at once (0; 1:
/// not JSON; 2: no session thread; 3: an engine not known, why in the
/// answer's bytes); the session thread builds and tells its events. A
/// document open before is closed.
#[allow(clippy::too_many_lines)]
pub(crate) fn open(req: &[u8]) -> i32 {
    // (the core ended: no session thread again)
    if ENDED.load(std::sync::atomic::Ordering::SeqCst) {
        return 2;
    }
    // (a step under way, in the host's turn: the open waits for its end)
    if STEPPING.load(std::sync::atomic::Ordering::Relaxed) {
        *lock(&OUTPUT) = b"a build is under way: open again once ph_step returns".to_vec();
        return 4;
    }
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
    // (the persisted graph: the host keeps it, `ph_graph_give` before the
    // open, `ph_graph` once told it is saved; off unless the host says)
    let graph = req["graph"].as_bool().unwrap_or(false);
    tell_panics();
    close();
    let generation = new_open(graph);
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
    // (the job's date, `\today`'s and the PDF's: the host's `date`, in
    // seconds whose UTC fields are its local date and time (a browser's
    // `Date`: TeX Live's clock is the local time), or 0 for fixed bytes;
    // else now, in UTC. Taken once, at the open: every build of the
    // session has the same, as a watch's. It was 0 always, and every
    // title page said "January 1, 1970", the Diff's too)
    let date = req["date"].as_i64().unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
    });
    let mut job = job(&main, engine, ini, threads, date);
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
    session.keep_graphs(graph);
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
            .spawn(move || run(doc, &w, &o, generation));
        if started.is_err() {
            return 2;
        }
    } else {
        // (no threads: kept for the host to step, the cold build its first step)
        wake.set(false);
        *lock(&STEPPED) = Some((doc, Idle::new(generation)));
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
    /// `exact`: the engine (`$pdf_mode`, `$pdflatex`) and `SyncTeX`.
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
fn job(
    main: &str,
    engine: phitex_live::tex::Engine,
    ini: bool,
    threads: Option<usize>,
    date: i64,
) -> Job {
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
        pdf_date: date,
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
/// for a quiet moment.) The persisted graph is saved after the first
/// build, then once nothing has been asked for [`GRAPH_IDLE`] after a
/// build ([`save_slice`], [`Idle::left`]).
fn run(mut doc: Document, wake: &Wake, output: &Output, generation: u64) {
    let mut idle = Idle::new(generation);
    loop {
        step(&mut doc, output, &mut idle);
        let mut s = lock(&wake.state);
        // (work nobody waits for, the build's origins: the next step's,
        // unless something was asked first)
        while !s.0 && !s.1 && !doc.pending() {
            if !idle.due {
                s = wake.cv.wait(s).unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let left = idle.left();
            if !left.is_zero() {
                s = wake
                    .cv
                    .wait_timeout(s, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            drop(s);
            // (to its end, unless something is asked meanwhile: given up)
            while save_slice(&doc, wake, &mut idle, None) {}
            s = lock(&wake.state);
        }
        if s.1 {
            return;
        }
        s.0 = false;
    }
}

/// How long nothing is asked after a build before its graph is saved
/// again ([`save_slice`]; the first build's is saved at once): a long
/// pause in the work, not every pause in the typing. A save costs the
/// core's memory and a second or so of a thread (Grail: 1.2 s, 61 MB),
/// and an edit that comes meanwhile gives it up, so it is kept rare.
const GRAPH_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// The persisted graph's save, as the session thread (or, without
/// threads, the host's steps) keeps it: due after a build that left one
/// to save, since when.
struct Idle {
    /// The open this session is (`new_open`): a save made after another
    /// open is dropped, never given as the new document's.
    generation: u64,
    due: bool,
    since: std::time::Instant,
    /// The last graph's size: the room a save asks for first.
    last_bytes: usize,
    /// A save of this open's ended (saved, or nothing to save): the next
    /// waits [`GRAPH_IDLE`]; before it, the first build's is saved at once.
    saved: bool,
    /// The save under way, without threads: a slice a step ([`save_slice`]).
    saving: Option<Saving>,
}

/// A save under way ([`save_slice`]): the session's, and its slices' count,
/// summed time and longest.
struct Saving {
    save: phitex_live::session::GraphSave,
    slices: u32,
    work: std::time::Duration,
    longest: std::time::Duration,
}

impl Idle {
    fn new(generation: u64) -> Idle {
        Idle {
            generation,
            due: false,
            since: std::time::Instant::now(),
            last_bytes: 0,
            saved: false,
            saving: None,
        }
    }

    /// How long until the due save: none after the first build, else
    /// what is left of [`GRAPH_IDLE`] since the last thing asked.
    fn left(&self) -> std::time::Duration {
        if self.saved {
            GRAPH_IDLE.saturating_sub(self.since.elapsed())
        } else {
            std::time::Duration::ZERO
        }
    }
}

/// The opens so far; the graph saved for the last one, until the host
/// takes it (`ph_graph`), with the open it is of.
static GRAPHS: Mutex<(u64, Option<Vec<u8>>, bool)> = Mutex::new((0, None, false));

/// A document opens (`graph`: its graph kept): its number. Whatever an
/// earlier one saved and the host did not take is dropped, and graphs
/// are given to the host from now on only if `graph`.
fn new_open(graph: bool) -> u64 {
    let mut g = lock(&GRAPHS);
    g.0 += 1;
    g.1 = None;
    g.2 = graph;
    if !graph {
        kept().give_graph(None);
    }
    g.0
}

/// The graph the host saved for the document about to open (`ph_graph_give`;
/// empty: none): its first build loads it where its keys hold
/// (`phitex_live::graph`), else builds clean.
pub(crate) fn graph_give(bytes: &[u8]) {
    kept().give_graph((!bytes.is_empty()).then(|| bytes.to_vec()));
}

/// The graph saved last (told by the event `{"event": "graph", "saved": N}`),
/// handed to the host in the answer's bytes: 1, or 0 (none since).
pub(crate) fn graph_take() -> i32 {
    match lock(&GRAPHS).1.take() {
        Some(b) => {
            *lock(&OUTPUT) = b;
            1
        }
        None => 0,
    }
}

/// How long a slice of a save runs without threads ([`save_slice`]): the
/// host's turn comes after it, so an edit typed meanwhile waits at most
/// about this long (and the item or piece of bytes under way when it is
/// up: [`phitex_live::session::Session::graph_save_more`]).
const SAVE_SLICE: std::time::Duration = std::time::Duration::from_millis(4);

/// A slice of the build's persisted graph's save (docs/spec-persisted-graph.md,
/// the wasm core): at idle, given up for anything asked meanwhile (the
/// next edit waits for no save: nothing stored), and begun only where the
/// memory has room for it ([`crate::heap::room_for`]: a save never takes
/// what the document needs). With threads (`slice` none) the save runs to
/// its end in one slice on the session thread; without, a slice runs
/// `slice` long, a step of the host's ([`step_once`]), and the host takes
/// what is asked between two. Once whole, the bytes kept for the host,
/// which is told (`{"event": "graph", "saved": N, ...}`) and takes them
/// (`ph_graph`). Whether the save is still under way (call again).
fn save_slice(
    doc: &Document,
    wake: &Wake,
    idle: &mut Idle,
    slice: Option<std::time::Duration>,
) -> bool {
    let t0 = std::time::Instant::now();
    if idle.saving.is_none() {
        idle.due = false;
        // (a save's peak: the engine's bytes and the file's, each about the
        // graph's size, the vectors' growth beside: three times the last
        // graph's, or the session's guess)
        let guess = doc.session().graph_bytes_guess().unwrap_or(0);
        let want = idle.last_bytes.max(guess).saturating_mul(3);
        if !crate::heap::room_for(want) {
            idle.saved = true;
            event(&json!({"event": "graph", "notes": [format!(
                "graph: not saved: no room for {} MB in the engine's memory",
                want >> 20
            )]}));
            return false;
        }
        let Ok(save) = doc.session().graph_save_start() else {
            // (nothing to save: the graph as loaded, or the build not quiet)
            idle.saved = true;
            event(&json!({"event": "graph", "notes": doc.session().graph_notes()}));
            return false;
        };
        idle.saving = Some(Saving {
            save,
            slices: 0,
            work: std::time::Duration::ZERO,
            longest: std::time::Duration::ZERO,
        });
        // (begun: `saved` or `given_up` follows)
        event(&json!({"event": "graph", "saving": true}));
    }
    let asked = || {
        let s = lock(&wake.state);
        s.0 || s.1
    };
    let pause = || asked() || slice.is_some_and(|d| t0.elapsed() >= d);
    let sv = idle.saving.as_mut().expect("a save under way");
    let saved = doc.session().graph_save_more(&mut sv.save, &pause);
    let took = t0.elapsed();
    sv.slices += 1;
    sv.work += took;
    sv.longest = sv.longest.max(took);
    if saved.is_none() {
        if !asked() {
            return true;
        }
        // (an edit came: saved after its build, at the next pause)
        give_up(doc, idle);
        return false;
    }
    let sv = idle.saving.take().expect("a save under way");
    idle.saved = true;
    let notes = doc.session().graph_notes();
    let Some(bytes) = store::Kept::take_graph() else {
        event(&json!({"event": "graph", "notes": notes}));
        return false;
    };
    idle.last_bytes = bytes.len();
    let n = bytes.len();
    let mut g = lock(&GRAPHS);
    if g.0 != idle.generation || !g.2 {
        return false;
    }
    g.1 = Some(bytes);
    drop(g);
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    event(&json!({
        "event": "graph",
        "saved": n,
        "ms": ms(sv.work),
        "slices": sv.slices,
        "slice_ms": ms(sv.longest),
        "notes": notes,
    }));
    false
}

/// The save under way given up (something asked: a build comes): nothing
/// stored, the next save due at the next pause. Told to the host
/// (`{"event": "graph", "given_up": true, ...}`: its slices, their time
/// and the longest), which keeps nothing for it.
fn give_up(doc: &Document, idle: &mut Idle) {
    if let Some(sv) = idle.saving.take() {
        let _ = doc.session().graph_save_given_up(sv.save);
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
        event(&json!({
            "event": "graph",
            "given_up": true,
            "ms": ms(sv.work),
            "slices": sv.slices,
            "slice_ms": ms(sv.longest),
            "notes": doc.session().graph_notes(),
        }));
    }
    idle.due = true;
    idle.since = std::time::Instant::now();
}

/// One step of the document (phitex-live's `Document::step`): what was
/// asked taken, built if anything changed, its events to the host; a
/// build's graph due for a save at the next pause, and what became of
/// the graph the session loaded (its first build's) told.
fn step(doc: &mut Document, output: &Output, idle: &mut Idle) {
    let events = doc.step();
    let built = events
        .iter()
        .any(|e| e["event"] == "build" || e["event"] == "failed");
    if built {
        *lock(output) = doc.output();
    }
    // (the graph's news first, then the build's: a host that has what it
    // waits for at the build's event knows by then whether it was loaded)
    if built && lock(&GRAPHS).2 {
        idle.due = true;
        let notes = doc.session().graph_notes();
        let dropped = kept().graph_dropped();
        if !notes.is_empty() || dropped {
            let loaded = notes.iter().any(|n| n.starts_with("graph: loaded"));
            event(&json!({"event": "graph", "notes": notes, "loaded": loaded, "dropped": dropped}));
        }
    }
    for e in &events {
        event(e);
    }
    // (anything that came: the pause starts again)
    if built || !events.is_empty() {
        idle.since = std::time::Instant::now();
    }
}

/// The open document, without threads: stepped by the host (`ph_step`),
/// with its graph's save.
static STEPPED: Mutex<Option<(Document, Idle)>> = Mutex::new(None);

/// Without threads (wasm32-wasip1), the host's turn of the session thread:
/// the open document stepped once if something was asked since (its first
/// step the cold build) or work nobody waits for is left (the build's
/// origins, a step of their own after the build's), its events told.
/// 1: stepped, step again soon (until 0; a save under way: its next
/// slice, [`SAVE_SLICE`] long, the host's requests answered first); 2: stepped, and only work
/// nobody waits for is left: step again once the requests that came are
/// answered (the pages asked after a build drawn before its origins are
/// made); 3: nothing asked, and the build's graph is to be saved once
/// nothing has been for [`GRAPH_IDLE`] (a save due now is a 1:
/// [`crate::step::answer`]): step again after that long (that
/// step begins its save, a slice a step, given up at the step that finds
/// something asked);
/// 0: nothing asked, or no document; with threads, always 0 (the session
/// thread steps it).
pub(crate) fn step_once() -> i32 {
    use std::sync::atomic::Ordering::Relaxed;
    // (asked in the host's turn of a step under way: nothing taken, the
    // ask left for the step after it, which that step's answer asks for)
    if STEPPING.load(Relaxed) {
        return 0;
    }
    let Some((wake, output)) = lock(&OPEN)
        .as_ref()
        .map(|o| (o.wake.clone(), o.output.clone()))
    else {
        return 0;
    };
    let (pending, due, saving) = lock(&STEPPED)
        .as_ref()
        .map_or((false, None, false), |(d, i)| {
            (d.pending(), i.due.then(|| i.left()), i.saving.is_some())
        });
    let save = {
        let mut s = lock(&wake.state);
        if s.1 {
            return 0;
        }
        if !s.0 && !pending {
            match due {
                _ if saving => true,
                None => return 0,
                Some(left) if !left.is_zero() => return 3,
                Some(_) => true,
            }
        } else {
            s.0 = false;
            false
        }
    };
    // (taken out while it runs: an ask the host answers in its call may lock the rest)
    let Some((mut doc, mut idle)) = lock(&STEPPED).take() else {
        return 0;
    };
    STEPPING.store(true, Relaxed);
    LAST.with(|l| l.set(Some(std::time::Instant::now())));
    ON_STEP.set(true);
    if save {
        save_slice(&doc, &wake, &mut idle, Some(SAVE_SLICE));
    } else {
        // (an edit, or the build's work after it: the save under way given up)
        if idle.saving.is_some() {
            give_up(&doc, &mut idle);
        }
        step(&mut doc, &output, &mut idle);
    }
    ON_STEP.set(false);
    STEPPING.store(false, Relaxed);
    // (closed in a turn of the host's while it ran: dropped)
    let closed = lock(&wake.state).1;
    if closed {
        return 0;
    }
    let asked = lock(&wake.state).0;
    let pending = doc.pending();
    let after = crate::step::After {
        saving: idle.saving.is_some(),
        save_in: (idle.due && !asked && !pending).then(|| idle.left()),
        later: pending && !asked,
        asked,
        was_save: save,
    };
    *lock(&STEPPED) = Some((doc, idle));
    crate::step::answer(after)
}

/// The open document closed: its session thread ends after the step it
/// is in (a build runs to its end).
pub(crate) fn close() {
    if let Some(o) = lock(&OPEN).take() {
        o.wake.set(true);
    }
    lock(&STEPPED).take();
}

/// The core ended ([`end`]): no document opens again.
static ENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The core ends, for good: every thread of its own returns from
/// `wasi_thread_start`, so the host's workers leave (a host joins them;
/// a Node host then exits by itself). The open document closed (its
/// session thread returns after the step it is in); every ask waiting for
/// the host (a file, a pack, the store's bytes, a `\write18` command)
/// answered as not had, and none made from now on, nor anything put in
/// the store; the engine's threads kept between builds (the defuse crew,
/// the client's pools) told to end, each returning at once if at rest,
/// else once its job is done. A build under way runs to its end on what
/// it has, then its threads return too. Thread 0's own: it waits for
/// nothing.
pub(crate) fn end() {
    ENDED.store(true, std::sync::atomic::Ordering::SeqCst);
    close();
    files().end();
    kept().end();
    shell_hook().end();
    phitex_tex::end_threads();
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

/// The engine's threads for documents opened on `n` threads (the open's
/// `threads`), started now, before the first open (`ph_threads`, once the
/// memory is sealed): the client's pool and the engine's kept threads
/// ([`phitex_tex::start_threads`]), each a worker the host starts, the
/// module instantiated in it. Started at the first build or graph load
/// they cost it their start (a fresh core's load of amsldoc's graph:
/// about 0.5 s of its 1.5); started with the core, they are at rest by
/// then. Made on a thread of their own: thread 0 never waits, and the
/// workers start as the host's event loop turns. 0: begun; 1: not (no
/// threads, one thread, the core ended, or its memory not sealed yet).
pub(crate) fn start_threads(n: usize) -> i32 {
    if !THREADED
        || n <= 1
        || ENDED.load(std::sync::atomic::Ordering::SeqCst)
        || !crate::heap::sealed()
    {
        return 1;
    }
    spawn_workers();
    let started = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || phitex_tex::start_threads(n));
    i32::from(started.is_err())
}

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
