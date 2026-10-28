//! The live viewer's protocol, without its transport: the state a
//! viewer is shown (the last build's pages and PDF, its diagnostics, the
//! host's messages, the diff) and the answers to its requests. The CLI
//! serves it over a WebSocket (`phitex watch`'s `view.rs`: HTTP, the
//! token, the browsers), the frontends' wasm core over its host's calls;
//! the events ([`Viewer::on_emit`]) go to every viewer either way.
//!
//! - **Requests.** The Overleaf extension's core requests (`session.ts`'s
//!   `CoreReq`) as JSON, `{"id":N,"op":…}`, answered `{"id":N,"ok":…,
//!   "json":…}` ([`Viewer::answer`]). Ops: `open`, `status`, `pages`,
//!   `diagnostics`, `console` (the messages so far), `rebuild`, `source` (a
//!   double-click's or an error's place: the host shows it,
//!   [`Viewer::on_goto`]), and the editor's buffer: `set_file {file, text}`
//!   and `edit {file, start, end, text}` (UTF-16 offsets) become the
//!   Build's new versions of the file, built as an edit ([`Ask`], taken by
//!   the watch). `origins` answers a page's characters with the source
//!   bytes each came from (what a double-click finds and forward search
//!   highlights; docs/note-origins.md): `{files, files_from, n, file,
//!   start, end, synth, x, y}`, each array base64 of its little-endian
//!   words (`u32`, `f32`), `files` the names from the client's
//!   `files_from` on (the table only grows). The diff ops (`diff_*`) are
//!   asked of the host, which diffs.
//! - **Events.** `settled` (a build is in: the viewer loads the PDF again
//!   and draws the pages whose hashes changed), `origins` before it where
//!   the host tells it (`{pages, edits}`: the pages whose origins changed,
//!   from 0, and the edits since the last as `[file, at, delta]`, the
//!   viewer shifting the offsets it keeps, [`OriginsChanged`]),
//!   `preparing`, `diagnostics`,
//!   `console` (each message the host shows, and its status line), `diff`,
//!   `written`, and `reveal` (forward search: `{"event":"reveal","page":K,
//!   "text":…,"texts":[…],"file":…,"line":L}`, the editor's cursor at line
//!   `L` (from 1) of `file`; `texts` are the words around it as a page would
//!   show them, longest first, and the build found `text`, one of them, on
//!   page `K` (from 1); the viewer finds the words there in MuPDF's text
//!   and marks them, with `K` null (no page found: a PDF build, whose page
//!   text the engine does not read) the first of `texts` it finds, nearest
//!   the place it last revealed).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

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
    /// The pages' glyph origins and the files they name (none: not
    /// kept, `phitex_tex::Build::page_origins`).
    pub origins: Option<Origins>,
    /// The pages whose origins changed and the edits' deltas, for a viewer
    /// that keeps the origins it asked for (none: not told; the event
    /// `origins`).
    pub changed: Option<OriginsChanged>,
}

/// The pages' glyph origins (`phitex_tex::Build::all_page_origins`) and
/// the files they name (`origin_files`).
#[derive(Clone, Default)]
pub struct Origins {
    pub files: Vec<String>,
    pub pages: Vec<Option<Arc<phitex_tex::PageOrigins>>>,
}

/// What changed in the pages' origins since a viewer last asked
/// (`phitex_tex::Build::origins_changed`): the pages (from 0) whose
/// answer changed, and the edits as `(file, at, delta)` in
/// [`Origins::files`] indices: a byte at or past `at` of the file as it
/// was is now `delta` further.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OriginsChanged {
    pub pages: Vec<usize>,
    pub edits: Vec<(u32, u32, i64)>,
}

/// What the browsers ask the watch for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Build again (the console's Rebuild).
    Rebuild,
    /// A change reached the screen this long after it was made.
    Painted(u64),
    /// File `file`'s text is the editor's buffer from now on.
    SetFile {
        file: String,
        text: String,
    },
    /// Characters `start..end` (UTF-16 code units) of `file`'s buffer
    /// replaced by `text`.
    Edit {
        file: String,
        start: usize,
        end: usize,
        text: String,
    },
    /// Diff version `rev` against `to` (none: the files as they are).
    DiffStart {
        rev: String,
        to: Option<String>,
    },
    /// Show the diff's pages (true) or the document's.
    DiffShow(bool),
    /// The diff's look: markup, subtype, the added and deleted colours
    /// (none: as it is).
    DiffRestyle {
        markup: Option<String>,
        subtype: Option<String>,
        add: Option<String>,
        del: Option<String>,
    },
    DiffStop,
    /// The editor's buffer of `file` is gone (closed): the file as on
    /// disk again.
    Closed {
        file: String,
    },
    /// The editor's cursor is at `line` and UTF-16 column `col` (from 0)
    /// of `file`: shown on the pages.
    Cursor {
        file: String,
        line: usize,
        col: usize,
    },
}

/// The diff the viewer is told of: its state (`compare.rs`'s), whether
/// its pages are the ones shown, its PDF, pages and marked-up text.
#[derive(Default)]
struct Diff {
    state: Value,
    shown: bool,
    pdf: Option<Arc<[u8]>>,
    hashes: Vec<u64>,
    tex: Option<String>,
}

/// The pages of the last build.
#[derive(Default)]
struct Pages {
    pdf: Option<Arc<[u8]>>,
    hashes: Vec<u64>,
    history: i32,
    ms: f64,
    origins: Option<Origins>,
}

/// A viewer's state and its requests' answers.
pub struct Viewer {
    pages: Mutex<Pages>,
    diagnostics: Mutex<Value>,
    asks: Mutex<Vec<Ask>>,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    building: AtomicBool,
    /// The host's messages so far (the last [`CONSOLE`]), and its status
    /// line: what a viewer that joins shows first.
    console: Mutex<(std::collections::VecDeque<Value>, Value)>,
    diff: Mutex<Diff>,
    /// The commits of the project's history (the diff's picker).
    log: Mutex<Option<Box<dyn Fn(usize, usize) -> Result<Vec<Value>, String> + Send>>>,
    /// Shows a source place (file, line, column from 1, and the place as
    /// `FILE:LINE:COL`): the host's editor.
    goto: Mutex<Option<Box<dyn Fn(&str, usize, usize, &str) + Send>>>,
    /// The bytes of a file of the project, by its name.
    read: Mutex<Option<Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>>>,
    /// Where the events go (every viewer).
    emit: Mutex<Option<Box<dyn Fn(&str) + Send>>>,
    /// What every ask goes through before it is queued ([`Viewer::route_asks`]).
    route: Mutex<Option<Box<dyn Fn(Ask) -> Ask + Send>>>,
}

/// The messages a viewer that joins is shown.
const CONSOLE: usize = 2000;

impl Default for Viewer {
    fn default() -> Viewer {
        Viewer {
            pages: Mutex::new(Pages::default()),
            diagnostics: Mutex::new(json!([])),
            asks: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            building: AtomicBool::new(false),
            console: Mutex::new((std::collections::VecDeque::new(), Value::Null)),
            diff: Mutex::new(Diff::default()),
            log: Mutex::new(None),
            goto: Mutex::new(None),
            read: Mutex::new(None),
            emit: Mutex::new(None),
            route: Mutex::new(None),
        }
    }
}

impl Viewer {
    /// The events from now on go to `emit` (each a JSON text).
    pub fn on_emit(&self, emit: Box<dyn Fn(&str) + Send>) {
        *lock(&self.emit) = Some(emit);
    }

    fn emit(&self, text: &str) {
        if let Some(e) = lock(&self.emit).as_ref() {
            e(text);
        }
    }

    /// A `source` place shown by `goto(file, line, col, place)`.
    pub fn on_goto(&self, goto: Box<dyn Fn(&str, usize, usize, &str) + Send>) {
        *lock(&self.goto) = Some(goto);
    }

    /// The project's files read by `read` (a `source` place's line).
    pub fn on_read(&self, read: Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>) {
        *lock(&self.read) = Some(read);
    }

    /// The PDF the viewer loads (`doc.pdf`): the diff's while its pages
    /// are the ones shown, else the last build's.
    pub fn pdf(&self) -> Option<Arc<[u8]>> {
        let d = lock(&self.diff);
        if d.shown {
            d.pdf.clone()
        } else {
            lock(&self.pages).pdf.clone()
        }
    }

    /// The diff's PDF and marked-up text (none: no diff yet).
    pub fn diff_pdf(&self) -> Option<Arc<[u8]>> {
        lock(&self.diff).pdf.clone()
    }

    pub fn diff_tex(&self) -> Option<String> {
        lock(&self.diff).tex.clone()
    }

    /// Every ask from now on (a request's, [`Viewer::answer`], or a host's,
    /// [`Viewer::push`]) goes through `route` before it is queued, in the
    /// order they came (the live document's main-file edits sent to a
    /// running build, [`crate::document::Document::absorber`]).
    pub fn route_asks(&self, route: Box<dyn Fn(Ask) -> Ask + Send>) {
        *lock(&self.route) = Some(route);
    }

    /// Call `wake` when a browser asks the watch for something.
    pub fn on_ask(&self, wake: Box<dyn Fn() + Send>) {
        *lock(&self.wake) = Some(wake);
    }

    /// Where the host's messages go (the CLI's `Ui::tap`): kept,
    /// and pushed to the browsers as `{"event":"console","item":…}`.
    pub fn console(self: &Arc<Self>) -> Box<dyn Fn(Value) + Send> {
        let s = self.clone();
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
            s.emit(&json!({"event": "console", "item": item}).to_string());
        })
    }

    /// The editor's cursor, at `line` (from 1) of `file`, shown on the
    /// page the build `found` with the words it found there (none: the
    /// browser looks for `texts`, the words around the cursor, longest
    /// first).
    pub fn reveal(
        &self,
        found: Option<(usize, String)>,
        texts: &[String],
        file: &str,
        line: usize,
    ) {
        let (page, text) = match found {
            Some((k, t)) => (Some(k), t),
            None => (None, texts.first().cloned().unwrap_or_default()),
        };
        self.emit(
            &json!({"event": "reveal", "page": page, "text": text, "texts": texts, "file": file, "line": line})
                .to_string(),
        );
    }

    /// The history the diff's picker lists, from `log(skip, limit)`.
    pub fn on_log(&self, log: Box<dyn Fn(usize, usize) -> Result<Vec<Value>, String> + Send>) {
        *lock(&self.log) = Some(log);
    }

    /// The diff is (still) under way: its state, PDF, pages and text; the
    /// browsers told, and given its pages if they are the ones shown.
    pub fn diff(&self, state: Value, pdf: Option<Arc<[u8]>>, hashes: Vec<u64>, tex: String) {
        let mut d = lock(&self.diff);
        let redraw = d.shown && (d.hashes != hashes || d.pdf.is_none());
        d.pdf = pdf;
        d.hashes = hashes;
        d.tex = Some(tex);
        d.state = state;
        let shown = d.shown;
        if let Some(o) = d.state.as_object_mut() {
            o.insert("shown".into(), Value::Bool(shown));
        }
        let msg = json!({"event": "diff", "diff": d.state}).to_string();
        drop(d);
        self.emit(&msg);
        if redraw {
            self.emit(&json!({"event": "settled", "since_ms": null}).to_string());
        }
    }

    /// The diff's pages shown (`on`) or the document's: the browsers draw
    /// the ones now shown.
    pub fn diff_shown(&self, on: bool) {
        let mut d = lock(&self.diff);
        let on = on && d.tex.is_some();
        if d.shown == on {
            return;
        }
        d.shown = on;
        if let Some(o) = d.state.as_object_mut() {
            o.insert("shown".into(), Value::Bool(on));
        }
        let msg = json!({"event": "diff", "diff": d.state}).to_string();
        drop(d);
        self.emit(&msg);
        self.emit(&json!({"event": "settled", "since_ms": null}).to_string());
    }

    /// Whether the diff's pages are the ones shown.
    pub fn diff_is_shown(&self) -> bool {
        lock(&self.diff).shown
    }

    /// No diff any more: the document's pages shown.
    pub fn diff_stopped(&self) {
        let shown = std::mem::take(&mut *lock(&self.diff)).shown;
        self.emit(&json!({"event": "diff", "diff": {}}).to_string());
        if shown {
            self.emit(&json!({"event": "settled", "since_ms": null}).to_string());
        }
    }

    /// Ask `a` of the watch, as a viewer's request would (an editor's
    /// change pushed by the host).
    pub fn push(&self, a: Ask) {
        self.ask(a);
    }

    /// What the browsers asked for since last taken.
    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut *lock(&self.asks))
    }

    /// A build began (`true`) or ended.
    pub fn building(&self, on: bool) {
        self.building.store(on, Ordering::Relaxed);
        self.emit(&json!({"event": "preparing", "on": on}).to_string());
    }

    /// The PDF served replaced by the same pages' exact file (written
    /// when idle over the updated one), the browsers told.
    pub fn pdf_replaced(&self, pdf: Option<Arc<[u8]>>) {
        lock(&self.pages).pdf = pdf;
        self.emit(&json!({"event": "written", "exact": true}).to_string());
    }

    /// A build is in: its PDF and pages' hashes kept, the viewers told
    /// (they draw the pages that changed). The pages (from 1) whose hashes
    /// changed, and how many there are.
    pub fn built(&self, b: &Built) -> (Vec<usize>, usize) {
        let mut pages = lock(&self.pages);
        let changed: Vec<usize> = b
            .hashes
            .iter()
            .enumerate()
            .filter(|&(k, h)| pages.hashes.get(k) != Some(h))
            .map(|(k, _)| k + 1)
            .collect();
        pages.pdf.clone_from(&b.pdf);
        pages.origins.clone_from(&b.origins);
        pages.history = b.history;
        pages.ms = b.ms;
        let n = b.hashes.len();
        pages.hashes.clone_from(&b.hashes);
        drop(pages);
        let items = Value::from(b.diagnostics.clone());
        self.emit(&json!({"event": "diagnostics", "items": items}).to_string());
        *lock(&self.diagnostics) = items;
        if let Some(c) = &b.changed {
            self.emit(&json!({"event": "origins", "pages": c.pages, "edits": c.edits}).to_string());
        }
        self.emit(&json!({"event": "settled", "since_ms": b.since_ms}).to_string());
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

fn hashes_json(hs: &[u64]) -> Value {
    hs.iter().map(|h| format!("{h:016x}")).collect()
}

impl Viewer {
    fn ask(&self, a: Ask) {
        // (routed and queued under the queue's lock: the asks keep their
        // order, whatever thread sends them)
        let mut q = lock(&self.asks);
        let a = match lock(&self.route).as_ref() {
            Some(r) => r(a),
            None => a,
        };
        q.push(a);
        drop(q);
        if let Some(w) = lock(&self.wake).as_ref() {
            w();
        }
    }
}

/// A number as an index or count (none if negative or not whole).
fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok())
}

impl Viewer {
    /// The reply to request `req` (`session.ts`'s `CoreRes`, with its `id`):
    /// what it asks of the watch pushed as an [`Ask`] (the host told,
    /// [`Viewer::on_ask`]), the rest answered from the state here.
    pub fn answer(&self, req: &Value) -> String {
        let s = self;
        let id = req.get("id").and_then(index).unwrap_or(0);
        let op = req.get("op").and_then(Value::as_str).unwrap_or("");
        let page = req.get("page").and_then(index);
        let text = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_owned);
        let state = || {
            let p = lock(&s.pages);
            let d = lock(&s.diff);
            let hashes = if d.shown {
                d.hashes.clone()
            } else {
                p.hashes.clone()
            };
            (hashes, p.history, p.ms)
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
                s.ask(a);
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
            "origins" => ok(origins(
                s,
                page.unwrap_or(0),
                req.get("files_from").and_then(index).unwrap_or(0),
            )),
            "source" => {
                let file = text("file").unwrap_or_default();
                let at = req
                    .get("line")
                    .and_then(index)
                    .map(|l| (l, req.get("col").and_then(index).unwrap_or(1)));
                let start = req.get("start").and_then(index).unwrap_or(0);
                match s.to_source(&file, start, at) {
                    Ok(place) => ok(json!({"place": place})),
                    Err(e) => fail(e),
                }
            }
            "diagnostics" => ok(json!({"items": lock(&s.diagnostics).clone()})),
            "painted" => {
                // (in microseconds: Ask stays Eq)
                if let Some(ms) = req.get("ms").and_then(Value::as_f64) {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    s.ask(Ask::Painted((ms.max(0.0) * 1e3) as u64));
                }
                ok(json!({}))
            }
            "rebuild" => {
                s.ask(Ask::Rebuild);
                ok(json!({}))
            }
            "console" => {
                let c = lock(&s.console);
                ok(json!({"items": c.0.iter().collect::<Vec<_>>(), "status": c.1}))
            }
            // (the browser reads the outline from the PDF itself, with MuPDF)
            "outline" => ok(json!({"items": []})),
            "diff" => ok(lock(&s.diff).state.clone()),
            "diff_log" => {
                let skip = req.get("skip").and_then(index).unwrap_or(0);
                let limit = req.get("limit").and_then(index).unwrap_or(100);
                match lock(&s.log).as_ref().map(|f| f(skip, limit)) {
                    Some(Ok(commits)) => ok(json!({"commits": commits})),
                    Some(Err(e)) => fail(&e),
                    None => fail("no history here"),
                }
            }
            "diff_start" => match text("rev") {
                Some(rev) => {
                    s.ask(Ask::DiffStart {
                        rev,
                        to: text("to"),
                    });
                    ok(json!({}))
                }
                None => fail("diff_start: which version (rev)?"),
            },
            "diff_show" => {
                s.ask(Ask::DiffShow(text("which").as_deref() != Some("current")));
                ok(json!({}))
            }
            "diff_restyle" => {
                s.ask(Ask::DiffRestyle {
                    markup: text("markup"),
                    subtype: text("subtype"),
                    add: text("add_color"),
                    del: text("del_color"),
                });
                ok(json!({}))
            }
            "diff_stop" => {
                s.ask(Ask::DiffStop);
                ok(json!({}))
            }
            o if o.starts_with("diff_") => fail(&format!("no op {o:?} here")),
            "trace" | "check" => ok(json!({"ok": true, "ms": 0})),
            _ => fail(&format!("no op {op:?} here")),
        };
        reply.to_string()
    }
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

impl Viewer {
    /// A double-click's source, byte `start` of `file`, or an error's
    /// line and column `at`: shown by the host ([`Viewer::on_goto`]). The
    /// place, `FILE:LINE:COL`.
    fn to_source(
        &self,
        file: &str,
        start: usize,
        at: Option<(usize, usize)>,
    ) -> Result<String, &'static str> {
        if file.is_empty() || file.starts_with('/') || file.contains("..") {
            return Err("not a file of the project");
        }
        let Some(text) = lock(&self.read).as_ref().and_then(|r| r(file)) else {
            return Err("no such file");
        };
        let (line, col) = at.unwrap_or_else(|| line_col(&text, start));
        let place = format!("{file}:{line}:{col}");
        if let Some(g) = lock(&self.goto).as_ref() {
            g(file, line, col, &place);
        }
        Ok(place)
    }
}

/// Page `k`'s characters with their sources (the module's `origins`):
/// none where the build kept none.
fn origins(s: &Viewer, k: usize, files_from: usize) -> Value {
    let p = lock(&s.pages);
    match &p.origins {
        Some(o) => origins_json(o, k, files_from),
        None => json!({"files": [], "files_from": files_from, "n": 0}),
    }
}

/// [`origins`]'s answer from the pages' origins `o`.
fn origins_json(o: &Origins, k: usize, files_from: usize) -> Value {
    let files: Vec<&String> = o.files.iter().skip(files_from).collect();
    let Some(Some(g)) = o.pages.get(k) else {
        return json!({"files": files, "files_from": files_from, "n": 0});
    };
    let words = |xs: &mut dyn Iterator<Item = u32>| {
        let mut b = Vec::new();
        for x in xs {
            b.extend_from_slice(&x.to_le_bytes());
        }
        base64(&b)
    };
    json!({
        "files": files,
        "files_from": files_from,
        "n": g.len(),
        "file": words(&mut g.file.iter().copied()),
        "start": words(&mut g.start.iter().copied()),
        "end": words(&mut g.end.iter().copied()),
        "synth": words(&mut g.synth.iter().copied()),
        "x": words(&mut g.x.iter().map(|x| x.to_bits())),
        "y": words(&mut g.y.iter().map(|y| y.to_bits())),
    })
}

/// `b` in base64 (RFC 4648, padded).
fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, sh) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() {
                char::from(A[(n >> sh & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
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
    fn base64_as_rfc_4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// A page's origins as the viewer gets them: little-endian words in
    /// base64, the files from `files_from` on.
    #[test]
    fn origins_answer() {
        let g = phitex_tex::PageOrigins {
            file: vec![1, u32::MAX],
            start: vec![5, 0],
            end: vec![6, 0],
            synth: vec![0],
            x: vec![1.0, 2.0],
            y: vec![3.0, 4.0],
            w: vec![0.5, 0.0],
            size: vec![10.0, 0.0],
        };
        let o = Origins {
            files: vec!["a.tex".into(), "b.tex".into()],
            pages: vec![Some(Arc::new(g))],
        };
        let v = origins_json(&o, 0, 1);
        assert_eq!(v["n"], 2);
        assert_eq!(v["files"], json!(["b.tex"]));
        assert_eq!(v["file"], base64(&[1, 0, 0, 0, 255, 255, 255, 255]));
        assert_eq!(v["start"], base64(&[5, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(origins_json(&o, 3, 0)["n"], 0);
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
