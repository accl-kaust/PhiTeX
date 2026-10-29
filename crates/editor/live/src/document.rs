//! A live document without a loop of its own: the session, its viewer
//! and the editor's buffers, driven by a host that calls in (the
//! frontends' wasm core, from its session thread). The host answers the
//! viewer's requests ([`Document::request`]), pushes the editor's changes
//! ([`Document::push`]), and calls [`Document::step`] to have them built:
//! the step takes what was asked, builds when something changed (the
//! first step always: the cold build), writes the outputs through the
//! session's sink, and returns the events the viewers are told
//! (`preparing`, `diagnostics`, `settled`, `console`, `reveal`, ...) and
//! `{"event": "build", ...}`, the build's facts; or, made with
//! [`Document::new_with_emit`], gives each event to the host as it
//! happens. The main file's edits reach a build while it runs: every ask
//! of the main file (a request's, a push's, or one from another thread
//! through [`Document::absorber`]) is sent to the session's running build
//! as it comes ([`crate::session::Absorb`]), as `phitex watch` sends them.
//! `phitex watch` runs the same pieces ([`crate::watch`], [`crate::view`])
//! in its own loop, which also takes the disk's changes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::{Value, json};

use crate::host::Host;
use crate::session::{Absorb, Session};
use crate::view::{Ask, Built, Viewer};
use crate::watch::{Applied, Edits, diagnostics, splice_utf16, tex_text, viewer_pdf};

/// Where a document's events go: collected for the step's return, or the
/// host's, each as it happens.
type Emit = Arc<Mutex<Box<dyn FnMut(Value) + Send>>>;

/// A session with its viewer and the editor's buffers.
pub struct Document {
    session: Session,
    viewer: Arc<Viewer>,
    edits: Edits,
    /// The viewer's events since the last step (none given the host).
    events: Arc<Mutex<Vec<Value>>>,
    emit: Emit,
    builds: usize,
    /// The last build's main output: its extension and bytes.
    output: Option<(&'static str, Arc<Vec<u8>>)>,
}

/// The main file's buffer as the asks left it, each change of it sent to
/// the session's build ([`Viewer::route_asks`]): an edit's offsets are of
/// the buffer as the asks before it left it, whatever thread sent them.
struct MainFeed {
    main: String,
    path: PathBuf,
    host: Arc<dyn Host>,
    absorb: Absorb,
    /// The editor's buffer (Unicode; none: as on disk).
    buffer: Mutex<Option<String>>,
}

impl MainFeed {
    /// `a` as it is queued: a main-file edit made its whole new buffer
    /// (`SetFile`), sent to the build first.
    fn route(&self, a: Ask) -> Ask {
        let mut b = lock(&self.buffer);
        let disk = || {
            self.host
                .read(&self.path)
                .map(|t| String::from_utf8_lossy(&t).into_owned())
        };
        let a = match a {
            Ask::SetFile { file, text } if file == self.main => {
                *b = Some(text.clone());
                Ask::SetFile { file, text }
            }
            Ask::Edit {
                file,
                start,
                end,
                text,
            } if file == self.main => {
                let cur = b.clone().or_else(disk).unwrap_or_default();
                let new = splice_utf16(&cur, start, end, &text);
                *b = Some(new.clone());
                Ask::SetFile { file, text: new }
            }
            Ask::Closed { file } if file == self.main => {
                *b = None;
                Ask::Closed { file }
            }
            other => return other,
        };
        // (the text sent under the buffer's lock: versions in the asks'
        // order; a main file not there to read is not sent empty)
        if let Some(t) = b.clone().or_else(disk) {
            self.absorb.send(&tex_text(&t), Instant::now());
        }
        a
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Document {
    /// A document of `session` (its main file named `main` from the
    /// project's directory), not built yet.
    /// Its events are collected and returned by [`Document::step`].
    pub fn new(session: Session, main: &str) -> Document {
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let collect = events.clone();
        let mut d = Document::new_with_emit(session, main, move |v| lock(&collect).push(v));
        d.events = events;
        d
    }

    /// A document whose events go to `emit` as they happen (on the thread
    /// that makes them: the step's, the build's), the viewer's
    /// (`preparing`, `diagnostics`, `settled`, ...) and the document's
    /// (`build`, `failed`); [`Document::step`] returns none. The viewer's
    /// own hook ([`Viewer::on_emit`]) is the document's: a host that sets
    /// it takes the events from the document.
    pub fn new_with_emit(
        session: Session,
        main: &str,
        emit: impl FnMut(Value) + Send + 'static,
    ) -> Document {
        let viewer = Arc::new(Viewer::default());
        let emit: Emit = Arc::new(Mutex::new(Box::new(emit)));
        {
            let emit = emit.clone();
            viewer.on_emit(Box::new(move |text: &str| {
                if let Ok(v) = serde_json::from_str(text) {
                    (*lock(&emit))(v);
                }
            }));
        }
        let path = session.job.path.clone();
        let feed = MainFeed {
            main: main.to_owned(),
            path: path.clone(),
            host: session.host().clone(),
            absorb: session.absorb(),
            buffer: Mutex::new(None),
        };
        viewer.route_asks(Box::new(move |a| feed.route(a)));
        Document {
            session,
            viewer,
            edits: Edits::new(main, &path),
            events: Arc::default(),
            emit,
            builds: 0,
            output: None,
        }
    }

    /// A handle that asks `a` of the document from any thread, as
    /// [`Document::push`] does: a main-file edit (`SetFile`, `Edit`,
    /// `Closed`) reaches a build running in [`Document::step`] at once
    /// (the build takes the latest text between its steps and its result
    /// is for it, as `phitex watch` absorbs an edit), and is taken by the
    /// next step too (no build again when the last one took it).
    pub fn absorber(&self) -> impl Fn(Ask) + Send + Sync + 'static {
        let v = self.viewer.clone();
        move |a| v.push(a)
    }

    fn event(&self, v: Value) {
        (*lock(&self.emit))(v);
    }

    /// The viewer: its state, and where a host's messages go
    /// ([`Viewer::console`]), its source places ([`Viewer::on_goto`]).
    pub fn viewer(&self) -> &Arc<Viewer> {
        &self.viewer
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// A viewer's request answered (`{"id","op",...}`); what it asks of
    /// the document is taken at the next step.
    pub fn request(&self, req: &Value) -> Value {
        serde_json::from_str(&self.viewer.answer(req)).unwrap_or(Value::Null)
    }

    /// An editor's change (or any ask), taken at the next step.
    pub fn push(&self, a: Ask) {
        self.viewer.push(a);
    }

    /// The last build's main output: its extension (`pdf`, `dvi`) and
    /// bytes (none: no build yet, or no pages).
    pub fn output(&self) -> Option<(&'static str, Arc<Vec<u8>>)> {
        self.output.clone()
    }

    /// What was asked since the last step taken, and built if anything
    /// changed (the first step: the cold build); the events since the
    /// last step.
    pub fn step(&mut self) -> Vec<Value> {
        let came = Instant::now();
        let mut why = Vec::new();
        let mut again = self.builds == 0;
        let mut asked = false;
        for a in self.viewer.take_asks() {
            match self.edits.apply(&mut self.session, Some(&self.viewer), a) {
                Applied::Edited(file) => {
                    why.push(format!("{file} (editor)"));
                    again = true;
                }
                Applied::Rebuild => {
                    why.push("asked in the viewer".to_owned());
                    asked = true;
                    again = true;
                }
                // (a diff is the host's; a painted page says nothing here)
                Applied::Diff(_) | Applied::Painted(_) | Applied::Nothing => {}
            }
        }
        if again && self.builds > 0 && !asked && self.session.up_to_date() {
            again = false;
        }
        if again {
            self.build(&why.join(", "), came);
        }
        std::mem::take(&mut *lock(&self.events))
    }

    fn build(&mut self, why: &str, came: Instant) {
        self.builds += 1;
        let b = self.builds;
        let v = self.viewer.clone();
        v.building(true);
        let s = &mut self.session;
        let r = match crate::panics::catch(|| s.build()) {
            Ok(r) => r,
            Err(m) => {
                let first = m.lines().next().unwrap_or("the engine panicked").to_owned();
                v.built(&Built {
                    pdf: v.pdf(),
                    hashes: Vec::new(),
                    history: 2,
                    ms: 0.0,
                    since_ms: None,
                    origins: None,
                    changed: None,
                    diagnostics: vec![
                        json!({"severity": "error", "code": "engine", "message": first}),
                    ],
                });
                v.building(false);
                self.event(json!({"event": "failed", "build": b, "why": why, "message": first}));
                return;
            }
        };
        let written = if r.internal.is_some() {
            Err(String::new())
        } else {
            s.write(&r, true)
        };
        let main = written.as_ref().ok().and_then(|w| w.main.clone());
        let pdf = viewer_pdf(s, &r, || {
            main.as_deref().and_then(|p: &Path| s.host().read(p))
        });
        let mut errors = r.out.errors.clone();
        let pdf = match pdf {
            Ok(p) => p,
            Err(e) => {
                errors.push(e);
                None
            }
        };
        if let Err(e) = &written
            && !e.is_empty()
        {
            errors.push(e.clone());
        }
        self.output = r.main_output().map(|(e, _)| {
            let bytes = if r.out.pdf.is_empty() {
                r.out.dvi.clone()
            } else {
                r.out.pdf.clone()
            };
            (e, bytes)
        });
        let log = format!("{}.log", s.job.jobname);
        v.built(&Built {
            pdf,
            hashes: r.out.page_hashes.clone(),
            history: r.history(),
            ms: r.wall.as_secs_f64() * 1e3,
            since_ms: Some(came.elapsed().as_secs_f64() * 1e3),
            origins: s.origins(),
            changed: s.origins_changed(),
            diagnostics: diagnostics(s, &r, &log),
        });
        v.building(false);
        // (the records the build changed, stored now: only when a load
        // changed them, compared with what is stored first; no idle timer)
        s.flush_records();
        self.event(json!({
            "event": "build",
            "build": b,
            "why": if why.is_empty() { "cold" } else { why },
            "ok": r.out.errors.is_empty(),
            "pages": r.pages(),
            "pages_changed": r.out.pages_changed,
            "wall_ms": r.wall.as_secs_f64() * 1e3,
            "errors": errors,
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::host::{Host, Sink};
    use crate::session::{Job, Status};

    /// No files: an `-ini` job reads none.
    struct Nothing;

    impl Host for Nothing {
        fn locate(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn read(&self, _path: &Path) -> Option<Vec<u8>> {
            None
        }
    }

    /// The outputs in memory.
    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, Vec<u8>>>);

    impl Sink for Memory {
        fn begin(&self) -> Result<(), String> {
            Ok(())
        }
        fn put(&self, name: &str, bytes: &[u8], _hash: u64) -> Result<PathBuf, String> {
            lock(&self.0).insert(name.to_owned(), bytes.to_vec());
            Ok(PathBuf::from(name))
        }
        fn put_main(&self, name: &str, bytes: &Arc<Vec<u8>>, hash: u64) -> Result<PathBuf, String> {
            self.put(name, bytes, hash)
        }
        fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
            self.put(name, bytes, 0).map(|_| ())
        }
        fn remove(&self, name: &str) {
            lock(&self.0).remove(name);
        }
        fn get(&self, name: &str) -> Option<Vec<u8>> {
            lock(&self.0).get(name).cloned()
        }
        fn path(&self, name: &str) -> PathBuf {
            PathBuf::from(name)
        }
    }

    fn pages(n: usize) -> String {
        let mut t = "\\catcode`\\{=1 \\catcode`\\}=2 ".to_owned();
        for _ in 0..n {
            t.push_str("\\shipout\\hbox{}");
        }
        t.push_str("\\end\n");
        t
    }

    fn job() -> Job {
        Job {
            engine: crate::tex::engine("etex").unwrap(),
            ini: true,
            etex: true,
            file: "doc.tex".into(),
            path: PathBuf::from("doc.tex"),
            jobname: "doc".into(),
            out_dir: PathBuf::from("."),
            threads: Some(2),
            spec: None,
            wait_long: false,
            node_records: false,
            synctex: false,
            origins: false,
            recorder: false,
            shell_escape: false,
            pdf_date: 0,
            fixpoint: false,
            cache_limit: None,
            edit_diagnostics: true,
        }
    }

    fn session(text: &str, sink: Arc<Memory>) -> Session {
        Session::new(
            job(),
            Arc::from(text),
            Arc::new(Nothing),
            None,
            sink,
            Arc::new(Status::default()),
        )
    }

    /// The cold build at the first step, an editor's buffer built at the
    /// next, the viewer's state and events as `phitex watch` gives them.
    #[test]
    fn steps_build_the_buffers() {
        let sink = Arc::new(Memory::default());
        let mut d = Document::new(session(&pages(1), sink.clone()), "doc.tex");
        let ev = d.step();
        let built = ev
            .iter()
            .find(|e| e["event"] == "build")
            .expect("the cold build");
        assert_eq!(built["pages"], 1, "{ev:?}");
        assert_eq!(built["ok"], true, "{ev:?}");
        assert!(ev.iter().any(|e| e["event"] == "settled"), "{ev:?}");
        assert_eq!(d.output().map(|o| o.0), Some("dvi"));
        assert!(sink.get("doc.dvi").is_some(), "the DVI written to the sink");
        // (nothing asked: nothing built)
        assert!(d.step().iter().all(|e| e["event"] != "build"));
        let r = d.request(&json!({"id": 7, "op": "set_file", "file": "doc.tex", "text": pages(3)}));
        assert_eq!(r["id"], 7, "{r}");
        let ev = d.step();
        let built = ev
            .iter()
            .find(|e| e["event"] == "build")
            .expect("the edit's build");
        assert_eq!(built["pages"], 3, "{ev:?}");
        assert_eq!(built["why"], "doc.tex (editor)", "{ev:?}");
        let p = d.request(&json!({"id": 8, "op": "pages"}));
        assert_eq!(p["json"]["pages"].as_array().map(Vec::len), Some(3), "{p}");
    }

    /// `n` pages after a loop of `k` steps (each an assignment): a build
    /// that takes a while.
    fn slow(k: u32, n: usize) -> String {
        let mut t = format!(
            "\\catcode`\\{{=1 \\catcode`\\}}=2 \\count1=0 \\def\\l{{\\advance\\count1 by1 \\ifnum\\count1<{k} \\expandafter\\l\\fi}}\\l "
        );
        for _ in 0..n {
            t.push_str("\\shipout\\hbox{}");
        }
        t.push_str("\\end\n");
        t
    }

    /// Events given as they happen ([`Document::new_with_emit`]): the same
    /// as a collecting document's step returns, in order, the step itself
    /// returning none.
    #[test]
    fn events_go_to_the_host_as_they_happen() {
        let mut collected =
            Document::new(session(&pages(2), Arc::new(Memory::default())), "doc.tex");
        let want: Vec<Value> = collected.step();
        let got: Arc<Mutex<Vec<Value>>> = Arc::default();
        let g = got.clone();
        let mut d = Document::new_with_emit(
            session(&pages(2), Arc::new(Memory::default())),
            "doc.tex",
            move |v| {
                lock(&g).push(v);
            },
        );
        assert!(d.step().is_empty(), "the events are the host's");
        let kinds = |vs: &[Value]| vs.iter().map(|e| e["event"].clone()).collect::<Vec<_>>();
        let got = lock(&got).clone();
        assert_eq!(kinds(&got), kinds(&want), "{got:?}");
        assert!(kinds(&got).contains(&json!("settled")) && kinds(&got).contains(&json!("build")));
    }

    /// An edit of the main file sent from another thread while a build
    /// runs ([`Document::absorber`]) is taken by that build: its result is
    /// the edited text's (its pages, its output the same as a cold build
    /// of that text), and the next step builds nothing.
    #[test]
    fn an_edit_sent_while_a_build_runs_is_taken_by_it() {
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        let ev = events.clone();
        let sink = Arc::new(Memory::default());
        let mut d = Document::new_with_emit(session(&pages(1), sink), "doc.tex", move |v| {
            if v["event"] == "preparing" && v["on"] == true {
                let _ = lock(&tx).send(());
            }
            lock(&ev).push(v);
        });
        d.step();
        assert_eq!(rx.recv().ok(), Some(()), "the cold build's");
        let edited = pages(3);
        let absorb = d.absorber();
        let e2 = edited.clone();
        // (sent once the edit's build has run a while)
        let sender = std::thread::spawn(move || {
            rx.recv().expect("the edit's build began");
            std::thread::sleep(std::time::Duration::from_millis(300));
            absorb(Ask::SetFile {
                file: "doc.tex".into(),
                text: e2,
            });
        });
        lock(&events).clear();
        d.push(Ask::SetFile {
            file: "doc.tex".into(),
            text: slow(4_000_000, 2),
        });
        let t = Instant::now();
        d.step();
        let wall = t.elapsed();
        sender.join().unwrap();
        let ev = lock(&events).clone();
        let builds: Vec<&Value> = ev.iter().filter(|e| e["event"] == "build").collect();
        assert_eq!(builds.len(), 1, "{ev:?}");
        assert_eq!(
            builds[0]["pages"], 3,
            "the edit sent while it ran (the build took {wall:?}): {ev:?}"
        );
        lock(&events).clear();
        d.step();
        assert!(
            lock(&events).iter().all(|e| e["event"] != "build"),
            "built already: {:?}",
            lock(&events)
        );
        let mut cold = Document::new(session(&edited, Arc::new(Memory::default())), "doc.tex");
        cold.step();
        assert_eq!(
            d.output().map(|o| o.1),
            cold.output().map(|o| o.1),
            "the cold build's output"
        );
    }

    /// An editor's ranged edits of the main file, from two threads in
    /// turn and a request between: each spliced into the buffer the asks
    /// before it left, the build of the text they make.
    #[test]
    fn ranged_edits_from_any_thread_splice_in_order() {
        let mut d = Document::new(session(&pages(1), Arc::new(Memory::default())), "doc.tex");
        d.step();
        let text = pages(1);
        d.push(Ask::SetFile {
            file: "doc.tex".into(),
            text: text.clone(),
        });
        let at = text.find("\\end").unwrap();
        let absorb = d.absorber();
        std::thread::spawn(move || {
            absorb(Ask::Edit {
                file: "doc.tex".into(),
                start: at,
                end: at,
                text: "\\shipout\\hbox{}".into(),
            });
        })
        .join()
        .unwrap();
        let r = d.request(&json!({"id": 1, "op": "edit", "file": "doc.tex", "start": at, "end": at, "text": "\\shipout\\hbox{}"}));
        assert_eq!(r["id"], 1, "{r}");
        let ev = d.step();
        let built = ev.iter().find(|e| e["event"] == "build").expect("built");
        assert_eq!(built["pages"], 3, "{ev:?}");
    }

    /// No files but dvipdfmx's finder, which finds nothing and counts
    /// what it was asked.
    struct Dpx(Arc<std::sync::atomic::AtomicUsize>);

    impl Host for Dpx {
        fn locate(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn read(&self, _path: &Path) -> Option<Vec<u8>> {
            None
        }
        fn dpx_finder(&self) -> Option<Arc<crate::dvipdf::DpxFinder>> {
            let n = self.0.clone();
            Some(Arc::new(crate::dvipdf::DpxFinder::new(
                Box::new(move |_name, _format, _prog| {
                    n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    None
                }),
                Box::new(|_path| None),
            )))
        }
    }

    /// A DVI build's viewer PDF made in process through the host's finder
    /// (no kpathsea): two pages of rules, dvipdfmx's files asked of the
    /// finder; a host without one says so.
    #[test]
    fn a_dvi_build_is_shown_as_a_pdf_through_the_hosts_finder() {
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let text = "\\catcode`\\{=1 \\catcode`\\}=2 \\shipout\\hbox{\\vrule width 20pt height 10pt}\\shipout\\hbox{\\vrule width 5pt height 5pt}\\end\n";
        let s = Session::new(
            job(),
            Arc::from(text),
            Arc::new(Dpx(asked.clone())),
            None,
            Arc::new(Memory::default()),
            Arc::new(Status::default()),
        );
        let mut d = Document::new(s, "doc.tex");
        let ev = d.step();
        let built = ev.iter().find(|e| e["event"] == "build").expect("built");
        assert_eq!(built["errors"], json!([]), "{ev:?}");
        assert_eq!(d.output().map(|o| o.0), Some("dvi"));
        let pdf = d.viewer().pdf().expect("the viewer's PDF");
        assert!(pdf.starts_with(b"%PDF-"), "{:?}", &pdf[..pdf.len().min(16)]);
        let pages = d.request(&json!({"id": 1, "op": "pages"}));
        assert_eq!(
            pages["json"]["pages"].as_array().map(Vec::len),
            Some(2),
            "{pages}"
        );
        assert!(
            asked.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "the finder asked"
        );
        // (a host without a finder: no PDF, said)
        let mut d = Document::new(session(text, Arc::new(Memory::default())), "doc.tex");
        let ev = d.step();
        let built = ev.iter().find(|e| e["event"] == "build").expect("built");
        assert!(
            built["errors"].to_string().contains("no dvipdfmx here"),
            "{ev:?}"
        );
    }

    /// XeTeX's fonts from closures and a relative index, as a wasm host
    /// gives them: TeX Live's Latin Modern Roman indexed with its path
    /// relative to `TEXMFDIST` (kpsewhich's, the test machine's), read by
    /// the host from its copy.
    struct XeHost {
        dist: PathBuf,
        index: Option<Vec<u8>>,
        /// The face's relative path read by XeTeX's fonts.
        read_face: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Host for XeHost {
        fn locate(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn read(&self, _path: &Path) -> Option<Vec<u8>> {
            None
        }
        fn xetex(&self) -> Option<phitex_tex::XeTeX> {
            let dist = self.dist.clone();
            let seen = self.read_face.clone();
            crate::xefonts::xetex(
                Box::new(move |p| {
                    assert!(!p.starts_with('/'), "a shipped index's path: {p}");
                    seen.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::fs::read(dist.join(p)).ok().map(Arc::from)
                }),
                Box::new(|_, _| None),
                self.index.as_deref(),
            )
            .ok()
        }
        fn dpx_finder(&self) -> Option<Arc<crate::dvipdf::DpxFinder>> {
            let (d1, d2) = (self.dist.clone(), self.dist.clone());
            Some(Arc::new(crate::dvipdf::DpxFinder::new(
                Box::new(move |name, _format, _prog| {
                    let n = String::from_utf8_lossy(name).into_owned();
                    d1.join(&n).is_file().then(|| n.into_bytes())
                }),
                Box::new(move |path| {
                    std::fs::read(d2.join(String::from_utf8_lossy(path).as_ref()))
                        .ok()
                        .map(Arc::from)
                }),
            )))
        }
    }

    /// An `-ini` XeTeX job loading a font by name: through the host's
    /// shipped index (relative paths) and its reads, the XDV names the
    /// face's relative path and xdvipdfmx, through the host's finder, makes
    /// the PDF with the face embedded; without the index the name is not
    /// found.
    #[test]
    fn xetex_finds_a_font_by_name_in_a_shipped_index() {
        let out = std::process::Command::new("kpsewhich")
            .args(["-var-value", "TEXMFDIST"])
            .output()
            .expect("kpsewhich (TeX Live) on the test machine");
        let dist = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        let rel = "fonts/opentype/public/lm/lmroman10-regular.otf";
        let data: Arc<[u8]> = Arc::from(std::fs::read(dist.join(rel)).expect("Latin Modern Roman"));
        let abs = dist.join(rel).to_string_lossy().into_owned();
        let mut e = phitex_otf::index::FaceEntry::read(&abs, data, 0);
        e.fc_names = Some(phitex_otf::xetex::fontmgr::fc_names(&e));
        let index = phitex_tex::FontIndex { entries: vec![e] }.relative_to(&dist.to_string_lossy());
        assert_eq!(&*index.entries[0].path, rel);
        let bytes = index.to_bytes();
        let text = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=\"Latin Modern Roman\" at 10pt \\x \\setbox0\\hbox{Hi}\\shipout\\box0 \\end\n";
        let read_face = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let run = |index: Option<Vec<u8>>| {
            let job = Job {
                engine: crate::tex::engine("xetex").unwrap(),
                ..job()
            };
            let s = Session::new(
                job,
                Arc::from(text),
                Arc::new(XeHost {
                    dist: dist.clone(),
                    index,
                    read_face: read_face.clone(),
                }),
                None,
                Arc::new(Memory::default()),
                Arc::new(Status::default()),
            );
            let mut d = Document::new(s, "doc.tex");
            let ev = d.step();
            let built = ev
                .iter()
                .find(|e| e["event"] == "build")
                .cloned()
                .expect("built");
            (built, d.output())
        };
        let (built, out) = run(Some(bytes));
        assert_eq!(built["errors"], json!([]), "{built}");
        assert_eq!(built["ok"], true, "{built}");
        assert_eq!(built["pages"], 1, "{built}");
        let (ext, pdf) = out.expect("the PDF");
        assert_eq!(ext, "pdf");
        assert!(pdf.starts_with(b"%PDF-"), "the PDF");
        assert!(
            read_face.load(std::sync::atomic::Ordering::Relaxed),
            "the face read by its relative path"
        );
        let (built, _) = run(None);
        assert_eq!(built["ok"], false, "{built}");
    }
}
