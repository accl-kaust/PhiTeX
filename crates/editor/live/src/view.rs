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
//!   `diagnostics`, `console` (the messages so far), `rebuild`, `frame`
//!   (a page as strips, [`Viewer::frame`]; their pixels are
//!   [`Viewer::strips`]), `text` (every page's, for finding), `outline`,
//!   `source` (a
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
//!   `building` (a build began or ended), `progress` (the build's progress record:
//!   docs/spec-editor-protocol.md §2.1, [`crate::progress`]), `diagnostics`,
//!   `console` (each message the host shows, and its status line), `diff`,
//!   `written`, and `reveal` (forward search: `{"event":"reveal","page":K,
//!   "text":…,"texts":[…],"file":…,"line":L}`, the editor's cursor at line
//!   `L` (from 1) of `file`; `texts` are the words around it as a page would
//!   show them, longest first, and the build found `text`, one of them, on
//!   page `K` (from 1); the viewer finds the words there in the page's text
//!   and marks them, with `K` null (no page found: a PDF build, whose page
//!   text the engine does not read) the first of `texts` it finds, nearest
//!   the place it last revealed).
//!   The pages are drawn here, as strips (`phitex_render`): the viewer
//!   loads no PDF.

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
    /// Build again from nothing: the session's build dropped, its format
    /// kept (the editor's `phitex.rebuild`).
    Cold,
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
    /// The editor's cursor at byte `at` of `file`'s text (the glyph
    /// origins' offsets), in word `word` (bytes; none: not in a word) of
    /// line `line` (bytes, without its end), and its selection `select`
    /// (bytes; none: the editor sent no range, empty: none selected):
    /// shown on the pages ([`Viewer::follow`], [`Viewer::select`]).
    Cursor {
        file: String,
        at: usize,
        word: Option<(usize, usize)>,
        line: (usize, usize),
        select: Option<(usize, usize)>,
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
/// [`Viewer::on_select`]'s: file, the start's and the end's (line, column), from 1.
pub type SelectFn = Box<dyn Fn(&str, (usize, usize), (usize, usize)) + Send>;

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
    /// A selection on the pages, selected in the editor: file, its start's
    /// and its end's line and column (from 1).
    select: Mutex<Option<SelectFn>>,
    /// The bytes of a file of the project, by its name.
    read: Mutex<Option<Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>>>,
    /// Where the events go (every viewer).
    emit: Mutex<Option<Box<dyn Fn(&str) + Send>>>,
    /// What every ask goes through before it is queued ([`Viewer::route_asks`]).
    route: Mutex<Option<Box<dyn Fn(Ask) -> Ask + Send>>>,
    /// The pages drawn as strips, their text and the outline
    /// (`phitex_render`, [`Viewer::frame`], [`Viewer::strips`]).
    strips: Mutex<phitex_render::Renderer>,
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
            select: Mutex::new(None),
            read: Mutex::new(None),
            emit: Mutex::new(None),
            route: Mutex::new(None),
            strips: Mutex::new(phitex_render::Renderer::new()),
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

    /// A selection on the pages (the `select` op: bytes of a file) given
    /// to `select` as lines and columns, for the editor to select.
    pub fn on_select(&self, select: SelectFn) {
        *lock(&self.select) = Some(select);
    }

    /// The project's files read by `read` (a `source` place's line).
    pub fn on_read(&self, read: Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>) {
        *lock(&self.read) = Some(read);
    }

    /// The bytes of the project's file `name`, as the host's `read` gives
    /// them (none: no such file, or no `read`).
    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        lock(&self.read).as_ref().and_then(|r| r(name))
    }

    /// Page `k` of the PDF shown ([`Viewer::pdf`]) at `scale` pixels a
    /// point, as strips (`phitex_render`): its size in pixels and points,
    /// its bands (each `[key, top, height]`, the key in hex), its text and
    /// links (`viewer/src/types.ts`' `Link`), and what drawing it took
    /// (`drawn` and `kept` bands, microseconds).
    pub fn frame(&self, k: usize, scale: f32) -> Option<Value> {
        use phitex_render::Target;
        if !(0.05..=16.0).contains(&scale) {
            return None;
        }
        let pdf = self.pdf()?;
        let t = std::time::Instant::now();
        let f = lock(&self.strips).frame(&pdf, k, scale)?;
        let us = t.elapsed().as_micros();
        let bands: Vec<Value> = f
            .bands
            .iter()
            .map(|b| json!([format!("{:016x}", b.key), b.y, b.h]))
            .collect();
        let links: Vec<Value> = f
            .links
            .iter()
            .map(|l| {
                let [a, b, c, d] = l.rect;
                match &l.target {
                    Target::Uri(u) => json!([a, b, c, d, u]),
                    Target::Page(p, y) => json!([a, b, c, d, p, y]),
                }
            })
            .collect();
        Some(json!({
            "pw": f.pw, "ph": f.ph, "w": f.w, "h": f.h,
            "bands": bands, "text": lines_json(&f.text), "links": links,
            "drawn": f.drawn, "kept": f.kept,
            "us": {"open": f.open_us, "probe": f.probe_us, "draw": f.draw_us, "all": us},
        }))
    }

    /// The strips of `keys` (hex, comma-separated, as [`Viewer::frame`]
    /// gives them): each one's width and height (`u32`, little-endian)
    /// and its RGBA pixels, in the order asked; one unknown (dropped since
    /// its frame) 0 by 0.
    pub fn strips(&self, keys: &str) -> Vec<u8> {
        let r = lock(&self.strips);
        let mut out = Vec::new();
        for key in keys.split(',').filter(|k| !k.is_empty()) {
            match u64::from_str_radix(key, 16).ok().and_then(|k| r.strip(k)) {
                Some(st) => {
                    out.extend_from_slice(&st.w.to_le_bytes());
                    out.extend_from_slice(&st.h.to_le_bytes());
                    out.extend_from_slice(&st.rgba);
                }
                None => out.extend_from_slice(&[0; 8]),
            }
        }
        out
    }

    /// [`Viewer::frame`] and the strips of its bands the viewer has not
    /// got, in one answer (a host whose requests queue on one thread: one
    /// round trip a page). `have`: the keys it has (hex, comma-separated;
    /// those of the page it last showed do). The bytes are the frame's
    /// JSON's length (`u32`, little-endian), the JSON (with `sent`: the
    /// keys whose strips follow), then those strips as [`Viewer::strips`]
    /// gives them.
    pub fn frame_with(&self, k: usize, scale: f32, have: &str) -> Option<Vec<u8>> {
        let mut f = self.frame(k, scale)?;
        let have: std::collections::HashSet<&str> =
            have.split(',').filter(|s| !s.is_empty()).collect();
        let mut sent: Vec<String> = Vec::new();
        for b in f["bands"].as_array().into_iter().flatten() {
            if let Some(key) = b[0].as_str()
                && !have.contains(key)
                && !sent.iter().any(|s| s == key)
            {
                sent.push(key.to_owned());
            }
        }
        let strips = self.strips(&sent.join(","));
        f["sent"] = json!(sent);
        let text = f.to_string();
        let mut out = Vec::with_capacity(4 + text.len() + strips.len());
        out.extend_from_slice(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&strips);
        Some(out)
    }

    /// Every page's text of the PDF shown, line by line (for finding).
    fn text_json(&self) -> Value {
        let Some(pdf) = self.pdf() else {
            return json!([]);
        };
        let mut r = lock(&self.strips);
        let n = r.pages(&pdf).unwrap_or(0);
        Value::Array(
            (0..n)
                .map(|k| {
                    r.text(&pdf, k)
                        .map_or_else(|| json!([]), |t| lines_json(&t))
                })
                .collect(),
        )
    }

    /// The outline of the PDF shown, as the viewer's entries
    /// (`viewer/src/outline.ts`' `Entry`: title, page, place, entries).
    fn outline_json(&self) -> Value {
        fn conv(es: &[phitex_render::Entry]) -> Value {
            Value::Array(
                es.iter()
                    .map(|e| json!({"t": e.title, "p": e.page, "y": e.y, "k": conv(&e.kids)}))
                    .collect(),
            )
        }
        let Some(pdf) = self.pdf() else {
            return json!([]);
        };
        conv(&lock(&self.strips).outline(&pdf))
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

    /// The editor's cursor at byte `at` of `file`'s text, in word `word`
    /// of line `line` (bytes, as [`Ask::Cursor`]): the browsers show it
    /// from the pages' glyph origins (`{event: "follow"}`, the session's
    /// `followBytes`).
    pub fn follow(
        &self,
        file: &str,
        at: usize,
        word: Option<(usize, usize)>,
        line: (usize, usize),
    ) {
        let word = word.map(|(a, b)| json!([a, b]));
        self.emit(
            &json!({"event": "follow", "file": file, "at": at, "word": word, "line": [line.0, line.1]})
                .to_string(),
        );
    }

    /// The editor's selection, bytes `from..to` of `file`'s text (empty:
    /// none): the browsers mark its glyphs on every page (`{event:
    /// "select"}`, the session's `selectBytes`).
    pub fn select(&self, file: &str, from: usize, to: usize) {
        self.emit(&json!({"event": "select", "file": file, "from": from, "to": to}).to_string());
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
    /// Whether an ask waits to be taken (an edit, a request of the
    /// watch's): work nobody waits for is put off while one does.
    pub fn has_asks(&self) -> bool {
        !lock(&self.asks).is_empty()
    }

    /// The pages' origins, made after the build that changed them was
    /// announced (off the typing path: when no ask waits, or when asked),
    /// and the viewers told which pages' origins changed.
    pub fn set_origins(&self, origins: Option<Origins>, changed: Option<OriginsChanged>) {
        lock(&self.pages).origins = origins;
        if let Some(c) = &changed {
            self.emit(&json!({"event": "origins", "pages": c.pages, "edits": c.edits}).to_string());
        }
    }

    pub fn take_asks(&self) -> Vec<Ask> {
        std::mem::take(&mut *lock(&self.asks))
    }

    /// A progress record of the build under way (the session's
    /// [`crate::progress::Meter`]): `{"event":"progress", ...}`.
    pub fn progress(&self, p: &crate::progress::Progress) {
        self.emit(&p.event().to_string());
    }

    /// A build began (`true`) or ended.
    pub fn building(&self, on: bool) {
        self.building.store(on, Ordering::Relaxed);
        self.emit(&json!({"event": "building", "on": on}).to_string());
    }

    /// The file on disk is exact already (nothing to write when idle):
    /// the browsers told, as when it is written.
    pub fn written_exact(&self) {
        self.emit(&json!({"event": "written", "exact": true}).to_string());
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
        // (a build announced without origins keeps the last ones until
        // they are made, `Viewer::set_origins`: never on the typing path)
        if b.origins.is_some() {
            pages.origins.clone_from(&b.origins);
        }
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

/// Lines of a page's text as the viewer takes them (`viewer/src/text.ts`'
/// `TextLine`).
fn lines_json(lines: &[phitex_render::TextLine]) -> Value {
    Value::Array(
        lines
            .iter()
            .map(|l| json!({"x": l.x, "y": l.y, "w": l.w, "size": l.size, "text": l.text, "xs": l.xs, "rs": l.rs}))
            .collect(),
    )
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
            "png" => fail("the pages are drawn as strips: frame and strips"),
            "frame" => {
                #[allow(clippy::cast_possible_truncation, reason = "a scale: pixels a point")]
                let scale = req.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                match s.frame(page.unwrap_or(0), scale) {
                    Some(f) => ok(f),
                    None => fail("no such page"),
                }
            }
            "text" => ok(json!({"pages": s.text_json()})),
            "outline" => ok(json!({"items": s.outline_json()})),
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
            "origins" => match origins(
                s,
                page.unwrap_or(0),
                req.get("files_from").and_then(index).unwrap_or(0),
            ) {
                Ok(v) => ok(v),
                Err(e) => fail(&e),
            },
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
            "select" => {
                let file = text("file").unwrap_or_default();
                let start = req.get("start").and_then(index).unwrap_or(0);
                let end = req.get("end").and_then(index).unwrap_or(start);
                match s.to_select(&file, start, end.max(start)) {
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

    /// A selection on the pages, bytes `start`..`end` of `file`: given to
    /// [`Viewer::on_select`]'s host as lines and columns. The place,
    /// `FILE:LINE:COL-LINE:COL`.
    fn to_select(&self, file: &str, start: usize, end: usize) -> Result<String, &'static str> {
        if file.is_empty() || file.starts_with('/') || file.contains("..") {
            return Err("not a file of the project");
        }
        let Some(text) = lock(&self.read).as_ref().and_then(|r| r(file)) else {
            return Err("no such file");
        };
        let (a, b) = (line_col(&text, start), line_col(&text, end));
        if let Some(f) = lock(&self.select).as_ref() {
            f(file, a, b);
        }
        Ok(format!("{file}:{}:{}-{}:{}", a.0, a.1, b.0, b.1))
    }
}

/// Page `k`'s characters with their sources (the module's `origins`):
/// none where the build kept none.
fn origins(s: &Viewer, k: usize, files_from: usize) -> Result<Value, String> {
    let p = lock(&s.pages);
    match &p.origins {
        Some(o) => origins_json(o, k, files_from),
        None => Err("the pages' origins are not made yet".into()),
    }
}

/// [`origins`]'s answer from the pages' origins `o`: page `k`'s, or not
/// made yet (a page still streaming, or past the pages there are). Never
/// an empty answer for a page whose origins are to come: a viewer keeps an
/// answer by the page's hash, and an empty one would stay (the frontends'
/// follow drew nothing on two pages answered while the first build
/// streamed).
fn origins_json(o: &Origins, k: usize, files_from: usize) -> Result<Value, String> {
    let files: Vec<&String> = o.files.iter().skip(files_from).collect();
    let Some(Some(g)) = o.pages.get(k) else {
        return Err(format!("page {k}'s origins are not made yet"));
    };
    let words = |xs: &mut dyn Iterator<Item = u32>| {
        let mut b = Vec::new();
        for x in xs {
            b.extend_from_slice(&x.to_le_bytes());
        }
        base64(&b)
    };
    Ok(json!({
        "files": files,
        "files_from": files_from,
        "n": g.len(),
        "file": words(&mut g.file.iter().copied()),
        "start": words(&mut g.start.iter().copied()),
        "end": words(&mut g.end.iter().copied()),
        "synth": words(&mut g.synth.iter().copied()),
        "x": words(&mut g.x.iter().map(|x| x.to_bits())),
        "y": words(&mut g.y.iter().map(|y| y.to_bits())),
    }))
}

/// `b` in base64 (RFC 4648, padded).
pub fn base64(b: &[u8]) -> String {
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

    /// A selection on the pages (the `select` op, bytes) reaches the host
    /// as lines and columns, for the editor to select; a name outside the
    /// project is refused.
    #[test]
    fn select_op() {
        let v = Viewer::default();
        v.on_read(Box::new(|f: &str| {
            (f == "a.tex").then(|| "one\ntwo three\n".as_bytes().to_vec())
        }));
        let got: Arc<Mutex<Vec<(String, (usize, usize), (usize, usize))>>> = Arc::default();
        let g = got.clone();
        v.on_select(Box::new(move |f: &str, a, b| {
            lock(&g).push((f.to_owned(), a, b))
        }));
        let r: Value = serde_json::from_str(
            &v.answer(&json!({"id": 1, "op": "select", "file": "a.tex", "start": 8, "end": 13})),
        )
        .unwrap();
        assert_eq!(r["json"]["place"], "a.tex:2:5-2:10", "{r}");
        assert_eq!(
            lock(&got).clone(),
            vec![("a.tex".to_owned(), (2, 5), (2, 10))]
        );
        let r: Value = serde_json::from_str(
            &v.answer(&json!({"id": 2, "op": "select", "file": "../x.tex", "start": 0, "end": 1})),
        )
        .unwrap();
        assert_eq!(r["ok"], false);
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
        let v = origins_json(&o, 0, 1).unwrap();
        assert_eq!(v["n"], 2);
        assert_eq!(v["files"], json!(["b.tex"]));
        assert_eq!(v["file"], base64(&[1, 0, 0, 0, 255, 255, 255, 255]));
        assert_eq!(v["start"], base64(&[5, 0, 0, 0, 0, 0, 0, 0]));
        // (a page whose origins are not made: not ready, never an empty answer)
        assert!(origins_json(&o, 3, 0).is_err());
    }

    #[test]
    fn page_ranges() {
        assert_eq!(ranges(&[1, 3, 4, 5, 7, 8]), "1, 3–5, 7–8");
        assert_eq!(ranges(&[]), "");
    }
}
