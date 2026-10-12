//! A document's session, kept for the process's life: one
//! [`phitex_tex::Build`], the document's only state. The first build is
//! an edit from nothing, every later one an edit of the main text as it
//! is now; the session writes what the build made.
//!
//! Native today: `tex` (plain.tex through `tex.ini`) and `-ini` jobs, DVI
//! out (`phitex-tex` writes no PDF yet, `\pdfoutput` or not). The other
//! formats (`etex`, LaTeX's) come with its next slices, and until then a
//! build says so.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phitex_tex::{Build, Finder, Options};

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

/// What to build, and how.
#[derive(Clone, Debug)]
#[allow(
    dead_code,
    reason = "threads and etex: the engine has no such options yet"
)]
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
    /// `-shell-escape`: `\write18` runs its commands.
    pub shell_escape: bool,
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
    /// The document's state: none before the first build.
    build: Option<Build>,
    /// kpathsea as the job's program: the build's finder.
    tree: Arc<tex::Tree>,
    builds: usize,
    /// The hashes of the files the last write wrote (an unchanged file is
    /// not written again).
    written: Mutex<BTreeMap<PathBuf, u64>>,
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
        let tree = Arc::new(tex::Tree::open(&job.engine));
        Ok(Session {
            job,
            text: tex::chars(&text),
            build: None,
            tree,
            builds: 0,
            written: Mutex::new(BTreeMap::new()),
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

    /// Builds: the first time from nothing, then as an edit of the main
    /// text as it is now. A job the engine cannot run yet reports why, as
    /// its one error.
    pub fn build(&mut self) -> Report {
        let t0 = Instant::now();
        let why = if self.build.is_some() { "edit" } else { "cold" };
        self.builds += 1;
        self.status.set(why);
        let r = self.run(why, t0);
        self.status.set("");
        r
    }

    fn run(&mut self, why: &str, t0: Instant) -> Report {
        let fail = |e: String| Report {
            out: Output {
                errors: vec![e],
                ..Output::default()
            },
            runs: vec![RunInfo {
                wall: t0.elapsed(),
                why: why.to_owned(),
                ..RunInfo::default()
            }],
            unsettled: Vec::new(),
            notes: Vec::new(),
            wall: t0.elapsed(),
        };
        let text = self.text_now();
        if let Some(b) = &mut self.build {
            b.edit(&text);
        } else {
            let prelude = match self.prelude() {
                Ok(p) => p,
                Err(e) => return fail(e),
            };
            let opts = Options {
                name: self.printed_name(),
                jobname: self.job.jobname.clone(),
                comment: dvi_comment(self.job.pdf_date),
                finder: self.finder(),
            };
            self.build = Some(Build::with(opts, prelude.as_deref(), &text));
        }
        let b = self.build.as_ref().expect("built above");
        let (dvi, log) = b.output();
        let s = &b.stats;
        let errors = log
            .lines()
            .filter(|l| l.starts_with("! "))
            .map(str::to_owned)
            .collect();
        let pages = dvi_pages(&dvi);
        let wall = t0.elapsed();
        Report {
            out: Output {
                dvi,
                pages,
                errors,
                log,
                ..Output::default()
            },
            runs: vec![RunInfo {
                wall,
                why: why.to_owned(),
                commands_run: s.nodes_run,
                pages,
                pages_reused: 0,
            }],
            unsettled: Vec::new(),
            notes: vec![format!(
                "{} nodes run of {} live, {} runs, {} cutoffs",
                s.nodes_run, s.live, s.runs, s.cutoffs
            )],
            wall,
        }
    }

    /// The main text with the editor's buffer for it, if there is one.
    fn text_now(&self) -> String {
        let name = self.job.path.to_string_lossy();
        self.buffers
            .iter()
            .rev()
            .find(|(n, _)| *n == self.job.file || *n == name)
            .and_then(|(_, t)| t.clone())
            .unwrap_or_else(|| self.text.clone())
    }

    /// The format's text: none for `-ini`; `tex`'s is its ini file
    /// (`\input plain \dump`). The other formats wait for the engine.
    fn prelude(&self) -> Result<Option<String>, String> {
        if self.job.ini {
            return Ok(None);
        }
        if self.job.engine.name != "tex" {
            return Err(format!(
                "{}: its format is not native yet (phitex-tex runs plain TeX; e-TeX's and LaTeX's \
                 formats come with its next slices): use tex, or -ini",
                self.job.engine.name
            ));
        }
        let ini = self.job.engine.ini;
        let p = self
            .tree
            .input(ini)
            .ok_or_else(|| format!("can't find {ini} (kpathsea)"))?;
        let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(Some(tex::chars(&b)))
    }

    /// The main file's name as TeX prints it (`./story.tex`).
    fn printed_name(&self) -> String {
        let p = self.job.path.to_string_lossy();
        if p.starts_with('/') || p.starts_with("./") || p.starts_with("../") {
            p.into_owned()
        } else {
            format!("./{p}")
        }
    }

    /// Files by kpathsea, as TeX names them.
    fn finder(&self) -> Finder {
        let tree = self.tree.clone();
        Arc::new(move |name: &str| {
            let p = tree.input(name)?;
            let bytes = std::fs::read(&p).ok()?;
            Some((p.to_string_lossy().into_owned(), bytes))
        })
    }

    /// Writes what the build made: `JOB.pdf` or `JOB.dvi`, the files
    /// TeX wrote, `JOB.log`, `JOB.fls` (`-recorder`).
    pub fn write(&self, r: &Report) -> Result<Written, String> {
        let dir = &self.job.out_dir;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut files = Vec::new();
        let mut main = None;
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        let mut put = |name: &str, bytes: &[u8]| -> Result<PathBuf, String> {
            let p = dir.join(name);
            let h = tex::Fnv::new().add(bytes).0;
            // (bytes the last write wrote, still there: not written again)
            if written.get(&p) != Some(&h) || !p.is_file() {
                let tmp = dir.join(format!(".{name}.phitex-tmp"));
                std::fs::write(&tmp, bytes)
                    .and_then(|()| std::fs::rename(&tmp, &p))
                    .map_err(|e| format!("can't write {}: {e}", p.display()))?;
                written.insert(p.clone(), h);
            }
            files.push((p.clone(), h));
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
    if job.shell_escape {
        v.push("-shell-escape: the native engine runs no \\write18 yet".to_owned());
    }
    v
}

/// Shared bytes of a report's output, for the viewer.
pub fn shared(b: &[u8]) -> Arc<[u8]> {
    Arc::from(b)
}

/// The DVI preamble's comment TeX writes: `" TeX output YYYY.MM.DD:HHMM"`
/// (§617), of `epoch` in UTC (TeX Live's with `SOURCE_DATE_EPOCH`).
fn dvi_comment(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // (days to a civil date: Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        " TeX output {y}.{m:02}.{d:02}:{:02}{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// The pages of a DVI file: the postamble's total (§642), found from the
/// end (§643: `post_post`, `q`, id, then the 223s).
fn dvi_pages(dvi: &[u8]) -> usize {
    let mut k = dvi.len();
    while k > 0 && dvi[k - 1] == 223 {
        k -= 1;
    }
    let Some(q) = k.checked_sub(5).and_then(|i| dvi.get(i..i + 4)) else {
        return 0;
    };
    let q = u32::from_be_bytes([q[0], q[1], q[2], q[3]]) as usize;
    dvi.get(q + 27..q + 29)
        .map_or(0, |t| usize::from(u16::from_be_bytes([t[0], t[1]])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment() {
        assert_eq!(dvi_comment(0), " TeX output 1970.01.01:0000");
        assert_eq!(dvi_comment(1_791_810_000), " TeX output 2026.10.12:1300");
    }
}
