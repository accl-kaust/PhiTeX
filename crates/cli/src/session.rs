//! A document's session: one `phitex_red::api::Build`, kept for the
//! process's life. The Build is the document's state; the session holds
//! only what is given to it (the main file's text, the editor's unsaved
//! buffers, the written files handed back to a new Build) and writes what
//! it made. The cross-run fixpoint is the Build's own loop-back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::tex::{self, Engine, Tree};

/// What a session does now, for a terminal's spinner (`ui.rs`).
#[derive(Default)]
pub struct Status {
    phase: Mutex<String>,
}

impl Status {
    pub fn set(&self, phase: impl Into<String>) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase.into();
    }

    pub fn phase(&self) -> String {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// What to build, and how.
#[derive(Clone, Debug)]
pub struct Job {
    /// The format's engine (with `ini`, only its paths: kpathsea's
    /// program name).
    pub engine: Engine,
    /// `-ini`: the file is the whole job, no format.
    pub ini: bool,
    /// With `ini`: e-TeX's extended mode (`-etex`, or `*` before the name).
    pub etex: bool,
    /// The main file as the command line names it (the terminal's line).
    pub file: String,
    /// Where it is.
    pub path: PathBuf,
    pub jobname: String,
    /// Where the outputs go (`.`: here).
    pub out_dir: PathBuf,
    pub threads: usize,
    /// `-synctex`.
    pub synctex: bool,
    /// `-recorder`: `JOB.fls`.
    pub recorder: bool,
    /// The PDF's date (`SOURCE_DATE_EPOCH`, else now).
    pub pdf_date: i64,
}

/// One run of a build.
#[derive(Clone, Debug, Default)]
pub struct RunInfo {
    pub wall: Duration,
    /// Why it ran: `cold` or `edit`.
    pub why: String,
    /// Nodes reduced (not kept from the last build).
    pub commands_run: u64,
    pub pages: usize,
    pub pages_reused: usize,
}

/// What one build made.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// The PDF (pdfTeX), else empty.
    pub pdf: Vec<u8>,
    /// The DVI (TeX, e-TeX), else empty.
    pub dvi: Vec<u8>,
    pub pages: usize,
    /// The pages shipped as the last build shipped them.
    pub pages_reused: usize,
    pub errors: Vec<String>,
    /// The terminal's and the log's text.
    pub log: String,
    /// The files TeX wrote.
    pub writes: Vec<(String, String)>,
}

/// What a build made.
pub struct Report {
    pub out: Output,
    pub runs: Vec<RunInfo>,
    /// The written files still changing when the Build stopped.
    pub unsettled: Vec<String>,
    pub notes: Vec<String>,
    pub wall: Duration,
}

impl Report {
    /// The output file's extension and bytes (none: no pages).
    pub fn main_output(&self) -> Option<(&'static str, &[u8])> {
        if !self.out.pdf.is_empty() {
            Some(("pdf", &self.out.pdf))
        } else if !self.out.dvi.is_empty() {
            Some(("dvi", &self.out.dvi))
        } else {
            None
        }
    }

    /// The pages shipped.
    pub fn pages(&self) -> usize {
        self.out.pages
    }

    /// TeX's history: 0 spotless, 1 warnings, 2 errors.
    pub fn history(&self) -> i32 {
        if !self.out.errors.is_empty() {
            2
        } else if self.warned() {
            1
        } else {
            0
        }
    }

    fn warned(&self) -> bool {
        !crate::warnings::summarize(&self.log_text()).is_empty()
    }

    /// The document job's transcript.
    pub fn log_text(&self) -> String {
        self.out.log.clone()
    }
}

/// What was written.
#[derive(Default)]
pub struct Written {
    /// The PDF or DVI file.
    pub main: Option<PathBuf>,
    /// Every file written, with its bytes' hash.
    pub files: Vec<(PathBuf, u64)>,
}

pub struct Session {
    pub job: Job,
    /// The main file's text, as TeX's characters.
    text: String,
    /// The files the last build wrote (a new Build's warm start).
    given: BTreeMap<String, String>,
    builds: usize,
    /// What the session does now, for a spinner.
    status: Arc<Status>,
    /// The Build (made at the first build).
    red: Option<phitex_red::api::Build>,
    /// Unsaved buffers given before the Build was made.
    buffers: Vec<(String, Option<String>)>,
}

/// `SOURCE_DATE_EPOCH`, else the time now.
pub fn pdf_date() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
        })
}

impl Session {
    /// A session for `job`: the main file read.
    pub fn new(job: Job, status: Arc<Status>, _note: &mut dyn FnMut(&str)) -> Result<Session, String> {
        if job.engine.xetex {
            return Err(format!("{}: XeTeX is not on phitex-red yet", job.engine.name));
        }
        let text = std::fs::read(&job.path).map_err(|e| format!("{}: {e}", job.path.display()))?;
        Ok(Session {
            job,
            text: tex::chars(&text),
            given: BTreeMap::new(),
            builds: 0,
            status,
            red: None,
            buffers: Vec::new(),
        })
    }

    /// The main file's text from now on (TeX's characters).
    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    /// File `name`'s text from now on (an editor's unsaved buffer, TeX's
    /// characters), or none: as on disk again.
    pub fn set_buffer(&mut self, name: &str, text: Option<&str>) {
        match &mut self.red {
            Some(r) => r.edit(phitex_red::api::Edit::File { name: Arc::from(name), text: text.map(Arc::from) }),
            None => self.buffers.push((name.to_owned(), text.map(str::to_owned))),
        }
    }

    /// Builds: the first time cold, then as an edit of the main text as it
    /// is now; the Build's loop-back fixpoint replaces the runs.
    pub fn build(&mut self) -> Report {
        use phitex_red::api;
        let t0 = Instant::now();
        let why = if self.builds > 0 { "edit" } else { "cold" };
        self.builds += 1;
        self.status.set(why);
        let text: Arc<str> = Arc::from(self.text.as_str());
        let mut errors = Vec::new();
        let out = if let Some(b) = &mut self.red {
            Some(b.update(vec![api::Edit::Main(text)]))
        } else {
            let e = &self.job.engine;
            let etex = if self.job.ini { self.job.etex } else { e.etex };
            let engine = if e.name.starts_with("pdf") {
                api::Engine::Pdftex
            } else if etex {
                api::Engine::Etex
            } else {
                api::Engine::Tex
            };
            let format = if self.job.ini {
                api::Format::Ini
            } else if e.name.contains("latex") {
                api::Format::Latex
            } else {
                api::Format::Plain
            };
            let tree = Arc::new(Tree::open(e));
            let out_dir = self.job.out_dir.clone();
            let finder: api::Finder = Arc::new(move |name: &str| {
                let p = [out_dir.join(name)]
                    .into_iter()
                    .find(|p| out_dir != Path::new(".") && p.is_file())
                    .or_else(|| tree.input(name))?;
                let bytes = std::fs::read(&p).ok()?;
                Some((Arc::from(p.to_string_lossy().as_ref()), Arc::from(bytes)))
            });
            let opts = api::Options {
                engine,
                format,
                job_name: Arc::from(self.job.jobname.as_str()),
                main_name: Arc::from(self.job.file.as_str()),
                finder,
                threads: self.job.threads,
                givens: BTreeMap::new(),
                previous: self.given.iter().map(|(k, v)| (Arc::from(k.as_str()), Arc::from(v.as_str()))).collect(),
            };
            match api::Build::new(opts, text) {
                Ok(mut b) => {
                    for (name, text) in self.buffers.drain(..) {
                        b.edit(api::Edit::File { name: Arc::from(name.as_str()), text: text.map(|t| Arc::from(t.as_str())) });
                    }
                    let o = b.update(Vec::new());
                    self.red = Some(b);
                    Some(o)
                }
                Err(e) => {
                    errors.push(e);
                    None
                }
            }
        };
        let o = out.unwrap_or_default();
        errors.extend(o.diags.iter().map(|d| d.msg.clone()));
        let writes: Vec<(String, String)> = o.written.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        for (n, t) in &writes {
            self.given.insert(n.clone(), t.clone());
        }
        let wall = t0.elapsed();
        self.status.set("");
        Report {
            out: Output {
                dvi: o.dvi,
                pdf: o.pdf,
                pages: o.pages,
                pages_reused: o.pages - o.changed_pages.len(),
                errors,
                log: o.log.concat(),
                writes,
            },
            runs: vec![RunInfo {
                wall,
                why: why.to_owned(),
                commands_run: o.nodes_reduced as u64,
                pages: o.pages,
                pages_reused: o.pages - o.changed_pages.len(),
            }],
            unsettled: o.unsettled.iter().map(ToString::to_string).collect(),
            notes: Vec::new(),
            wall,
        }
    }

    /// Writes what the build made: `JOB.pdf` or `JOB.dvi`, the files
    /// TeX wrote, `JOB.log`, `JOB.fls` (`-recorder`).
    pub fn write(&self, r: &Report) -> Result<Written, String> {
        let dir = &self.job.out_dir;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut files = Vec::new();
        let mut main = None;
        let mut put = |name: &str, bytes: &[u8]| -> Result<PathBuf, String> {
            let p = dir.join(name);
            let tmp = dir.join(format!(".{name}.phitex-tmp"));
            std::fs::write(&tmp, bytes)
                .and_then(|()| std::fs::rename(&tmp, &p))
                .map_err(|e| format!("can't write {}: {e}", p.display()))?;
            files.push((p.clone(), tex::Fnv::new().add(bytes).0));
            Ok(p)
        };
        let ext = r.main_output().map_or(self.job.engine.output.ext(), |(e, _)| e);
        if let Some((e, b)) = r.main_output() {
            main = Some(put(&format!("{}.{e}", self.job.jobname), b)?);
        }
        for (n, t) in &r.out.writes {
            put(n, &tex::bytes(t))?;
        }
        put(&format!("{}.log", self.job.jobname), &tex::bytes(&r.out.log))?;
        if self.job.recorder {
            let fls = self.recorder(r, &format!("{}.{ext}", self.job.jobname));
            put(&format!("{}.fls", self.job.jobname), fls.as_bytes())?;
        }
        Ok(Written { main, files })
    }

    /// `JOB.fls` as `-recorder` writes it, of what is known: the main
    /// file, the outputs.
    fn recorder(&self, r: &Report, out_name: &str) -> String {
        let mut s = String::new();
        let cwd = std::env::current_dir().unwrap_or_default();
        s.push_str(&format!("PWD {}\n", cwd.display()));
        s.push_str(&format!("INPUT {}\n", self.job.path.display()));
        let dir = &self.job.out_dir;
        let out = |n: &str| dir.join(n).display().to_string();
        for (n, _) in &r.out.writes {
            s.push_str(&format!("OUTPUT {}\n", out(n)));
        }
        if r.main_output().is_some() {
            s.push_str(&format!("OUTPUT {}\n", out(out_name)));
        }
        s.push_str(&format!("OUTPUT {}\n", out(&format!("{}.log", self.job.jobname))));
        s
    }

    /// What the terminal shows of the document's job (TeX's own output).
    pub fn terminal(r: &Report) -> String {
        r.out.log.clone()
    }
}

/// The notes a job's options that phitex-red cannot do yet give, once.
pub fn waiting_notes(job: &Job) -> Vec<String> {
    let mut v = Vec::new();
    if job.recorder {
        v.push("-recorder: the .fls lists the main file and the outputs, not the files a build read".to_owned());
    }
    if job.synctex {
        v.push("-synctex: phitex-red writes no SyncTeX yet".to_owned());
    }
    v
}

/// Shared bytes of a report's output, for the viewer.
pub fn shared(b: &[u8]) -> Arc<[u8]> {
    Arc::from(b)
}
