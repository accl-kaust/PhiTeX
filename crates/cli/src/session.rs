//! A document's session, kept for the process's life. The session holds
//! only what is given to it (the main file's text, the editor's unsaved
//! buffers, the files the last build wrote) and writes what the build
//! made. There is no native engine yet: `phitex-tex` on `phitex-defuse`
//! is being written, and until it lands every build reports that.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::tex::{self, Engine};

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
        self.phase
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What to build, and how. (Some fields are read only by the native
/// engine to come: `phitex-tex` on `phitex-defuse`.)
#[derive(Clone, Debug)]
#[allow(dead_code)]
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
    /// The files the last build wrote (a new build's warm start; kept
    /// for the native engine to come).
    #[allow(dead_code)]
    given: BTreeMap<String, String>,
    builds: usize,
    /// What the session does now, for a spinner.
    status: Arc<Status>,
    /// Unsaved editor buffers, by file name (`None`: as on disk again).
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
    pub fn new(
        job: Job,
        status: Arc<Status>,
        _note: &mut dyn FnMut(&str),
    ) -> Result<Session, String> {
        let text = std::fs::read(&job.path).map_err(|e| format!("{}: {e}", job.path.display()))?;
        Ok(Session {
            job,
            text: tex::chars(&text),
            given: BTreeMap::new(),
            builds: 0,
            status,
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
        self.buffers.retain(|(n, _)| n != name);
        self.buffers
            .push((name.to_owned(), text.map(str::to_owned)));
    }

    /// Builds: the first time cold, then as an edit of the main text as it
    /// is now. With no native engine, the report carries one error and
    /// nothing else; `given` and `buffers` are kept for the engine to come.
    pub fn build(&mut self) -> Report {
        let t0 = Instant::now();
        let why = if self.builds > 0 { "edit" } else { "cold" };
        self.builds += 1;
        self.status.set(why);
        let errors = vec![format!(
            "{}: no native engine yet (phitex-tex on phitex-defuse is being written); run TeX Live's {0} instead",
            self.job.engine.name
        )];
        let wall = t0.elapsed();
        self.status.set("");
        Report {
            out: Output {
                errors,
                ..Output::default()
            },
            runs: vec![RunInfo {
                wall,
                why: why.to_owned(),
                ..RunInfo::default()
            }],
            unsettled: Vec::new(),
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
        let ext = r
            .main_output()
            .map_or(self.job.engine.output.ext(), |(e, _)| e);
        if let Some((e, b)) = r.main_output() {
            main = Some(put(&format!("{}.{e}", self.job.jobname), b)?);
        }
        for (n, t) in &r.out.writes {
            put(n, &tex::bytes(t))?;
        }
        put(
            &format!("{}.log", self.job.jobname),
            &tex::bytes(&r.out.log),
        )?;
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
        s.push_str(&format!(
            "OUTPUT {}\n",
            out(&format!("{}.log", self.job.jobname))
        ));
        s
    }

    /// What the terminal shows of the document's job (TeX's own output).
    pub fn terminal(r: &Report) -> String {
        r.out.log.clone()
    }
}

/// The notes a job's options that the native engine cannot do yet give, once.
pub fn waiting_notes(job: &Job) -> Vec<String> {
    let mut v = Vec::new();
    if job.recorder {
        v.push(
            "-recorder: the .fls lists the main file and the outputs, not the files a build read"
                .to_owned(),
        );
    }
    if job.synctex {
        v.push("-synctex: the native engine writes no SyncTeX yet".to_owned());
    }
    v
}

/// Shared bytes of a report's output, for the viewer.
pub fn shared(b: &[u8]) -> Arc<[u8]> {
    Arc::from(b)
}
