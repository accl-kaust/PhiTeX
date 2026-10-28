//! A live document without a loop of its own: the session, its viewer
//! and the editor's buffers, driven by a host that calls in (the
//! frontends' wasm core, from its session thread). The host answers the
//! viewer's requests ([`Document::request`]), pushes the editor's changes
//! ([`Document::push`]), and calls [`Document::step`] to have them built:
//! the step takes what was asked, builds when something changed (the
//! first step always: the cold build), writes the outputs through the
//! session's sink, and returns the events the viewers are told
//! (`preparing`, `diagnostics`, `settled`, `console`, `reveal`, ...) and
//! `{"event": "build", ...}`, the build's facts. `phitex watch` runs the
//! same pieces ([`crate::watch`], [`crate::view`]) in its own loop, which
//! also takes the disk's changes and a build's edits while it runs.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::{Value, json};

use crate::session::Session;
use crate::view::{Ask, Built, Viewer};
use crate::watch::{Applied, Edits, diagnostics, viewer_pdf};

/// A session with its viewer and the editor's buffers.
pub struct Document {
    session: Session,
    viewer: Arc<Viewer>,
    edits: Edits,
    /// The viewer's events since the last step.
    events: Arc<Mutex<Vec<Value>>>,
    builds: usize,
    /// The last build's main output: its extension and bytes.
    output: Option<(&'static str, Arc<Vec<u8>>)>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Document {
    /// A document of `session` (its main file named `main` from the
    /// project's directory), not built yet.
    pub fn new(session: Session, main: &str) -> Document {
        let viewer = Arc::new(Viewer::default());
        let events: Arc<Mutex<Vec<Value>>> = Arc::default();
        {
            let events = events.clone();
            viewer.on_emit(Box::new(move |text: &str| {
                if let Ok(v) = serde_json::from_str(text) {
                    lock(&events).push(v);
                }
            }));
        }
        let path = session.job.path.clone();
        Document {
            session,
            viewer,
            edits: Edits::new(main, &path),
            events,
            builds: 0,
            output: None,
        }
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
                lock(&self.events)
                    .push(json!({"event": "failed", "build": b, "why": why, "message": first}));
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
        lock(&self.events).push(json!({
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

    /// The records the builds changed, kept (when the host is idle).
    pub fn idle(&mut self) {
        self.session.flush_records();
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

    /// The cold build at the first step, an editor's buffer built at the
    /// next, the viewer's state and events as `phitex watch` gives them.
    #[test]
    fn steps_build_the_buffers() {
        let job = Job {
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
        };
        let sink = Arc::new(Memory::default());
        let s = Session::new(
            job,
            Arc::from(pages(1)),
            Arc::new(Nothing),
            None,
            sink.clone(),
            Arc::new(Status::default()),
        );
        let mut d = Document::new(s, "doc.tex");
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
}
