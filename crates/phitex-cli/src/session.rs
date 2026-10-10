//! A document's session: one `phitex_flow::Build`, kept for the process's
//! life, and the jobs it runs: the format (cached, `format.rs`), then the
//! document on it. The Build is the document's state; the session holds
//! only what is given to it (the main file's text, the editor's unsaved
//! buffers, the written files handed back as new versions) and writes
//! what it made.
//!
//! **The cross-run fixpoint** (AGENTS_BRIEF §7): run k reads what run
//! k−1 wrote, as new versions of those files (`Build::set_file`), with
//! BibTeX and makeindex between runs; it stops when the written files are
//! the versions the run read, after at most [`FIXPOINT_RUNS`] runs, and
//! says which files did not settle. (Agent B is putting this loop into
//! phitex-flow; [`Session::fixpoint`] is the one place that calls it.)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Options, Output, Reads};

use crate::tex::{self, Engine, Tree};

/// The runs a fixpoint may take.
pub const FIXPOINT_RUNS: usize = 5;

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
    /// 0 batch, 1 nonstop, 2 scroll, 3 errorstop.
    pub interaction: i32,
    pub threads: usize,
    /// Run to the fixpoint (else once, as TeX runs).
    pub fixpoint: bool,
    /// BibTeX and makeindex between runs.
    pub tools: bool,
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
    /// Why it ran: `cold`, `edit`, or the files that changed.
    pub why: String,
    pub commands_run: u64,
    pub commands: u64,
    pub pieces_ran: usize,
    pub pieces: usize,
    pub pages: usize,
    pub pages_reused: usize,
}

/// What a build made.
pub struct Report {
    pub out: Output,
    pub runs: Vec<RunInfo>,
    /// The written files still changing at the last run (the fixpoint's
    /// bound reached: they oscillate, or need more runs).
    pub unsettled: Vec<String>,
    /// What the tools said.
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
        self.out.stats.pages
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
        let name = format!("x.{}", self.main_output().map_or("dvi", |(e, _)| e));
        self.out
            .job_logs(&Limits::TEX_LIVE, &name)
            .pop()
            .map(|l| l.log)
            .unwrap_or_default()
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
    build: Build,
    /// The main file's text, as TeX's characters.
    text: String,
    /// The written files given to the build as new versions (the last
    /// run's), and the tools' outputs.
    given: BTreeMap<String, String>,
    /// The tools' other files (`.blg`, `.ilg`), to write.
    side: BTreeMap<String, Vec<u8>>,
    tools: crate::tools::Tools,
    /// The format the build starts from.
    pub format: Option<PathBuf>,
    builds: usize,
    /// `JOB.synctex.gz`, kept between builds (`synctex.rs`).
    pub sync: crate::synctex::Writer,
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

/// How a build's reads that find a definition being remade go
/// (`PHITEX_READS=suspend|speculate`; else `edit`'s default: cold builds
/// suspend, edits speculate).
fn reads(edit: bool) -> Reads {
    match std::env::var("PHITEX_READS").as_deref() {
        Ok("suspend") => Reads::Suspend,
        Ok("speculate") => Reads::Speculate,
        _ if edit => Reads::Speculate,
        _ => Reads::Suspend,
    }
}

impl Session {
    /// A session for `job`: TeX Live's paths, the format (made if the
    /// cache has none), the main file read. `note` hears what takes time.
    pub fn new(job: Job, note: &mut dyn FnMut(&str)) -> Result<Session, String> {
        let tree = Tree::open(&job.engine);
        let text = std::fs::read(&job.path).map_err(|e| format!("{}: {e}", job.path.display()))?;
        let mut build = Build::new(phitex_flow::tfm::null_font());
        // (every file by kpathsea as the engine's program — TEXINPUTS from
        // the environment, `.` first —, the output directory before it, as
        // web2c's `open_input` tries `-output-directory` first)
        build.set_kpse(tree.kpse.clone(), tree.prog);
        let mut dirs = Vec::new();
        if job.out_dir != Path::new(".") {
            dirs.push(job.out_dir.clone());
        }
        build.set_input_dirs(dirs);
        build.set_jobname(&job.jobname);
        build.set_limits(Limits::TEX_LIVE);
        build.set_interaction(job.interaction);
        build.set_pdf_date(job.pdf_date);
        let mut format = None;
        if !job.ini {
            let (bytes, path) = crate::format::ensure(&job.engine, &tree, job.threads, note)?;
            build
                .set_format(&bytes)
                .map_err(|e| format!("{}: the format does not read: {e}", path.display()))?;
            build.set_interaction_option(Some(job.interaction));
            build.set_terminal(&[&format!("&{} {}", job.engine.name, job.file)]);
            format = Some(path);
        }
        Ok(Session {
            job,
            build,
            text: tex::chars(&text),
            given: BTreeMap::new(),
            side: BTreeMap::new(),
            tools: crate::tools::Tools::default(),
            format,
            builds: 0,
            sync: crate::synctex::Writer::default(),
        })
    }

    /// The main file's text from now on (TeX's characters).
    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    /// File `name`'s text from now on (an editor's unsaved buffer, TeX's
    /// characters), or none: as on disk again.
    pub fn set_buffer(&mut self, name: &str, text: Option<&str>) {
        match text {
            Some(t) => self.build.set_file(name, t),
            None => self.build.remove_file(name),
        }
    }

    fn etex(&self) -> bool {
        if self.job.ini { self.job.etex } else { self.job.engine.etex }
    }

    /// The build's source: the main file's text (with a format, as the
    /// job that loads it, `graph::jobs`).
    fn src(&self) -> String {
        if self.job.ini { self.text.clone() } else { jobs(&[&self.text]) }
    }

    /// What a run reads as file `name`: the version given, else the disk's
    /// (the output directory first, then here).
    fn read_back(&self, name: &str) -> Option<String> {
        if let Some(t) = self.given.get(name) {
            return Some(t.clone());
        }
        [self.job.out_dir.join(name), PathBuf::from(name)]
            .iter()
            .find_map(|p| if p.is_file() { std::fs::read(p).ok() } else { None })
            .map(|b| tex::chars(&b))
    }

    fn give(&mut self, name: &str, text: String) {
        self.build.set_file(name, &text);
        self.given.insert(name.to_owned(), text);
    }

    /// Builds: the first time cold, then as an edit of what changed; to
    /// the fixpoint if the job says so.
    pub fn build(&mut self) -> Report {
        let edit = self.builds > 0;
        self.builds += 1;
        let opts = Options { threads: self.job.threads.max(1), reads: reads(edit), etex: self.etex(), ..Options::default() };
        let bound = if self.job.fixpoint { FIXPOINT_RUNS } else { 1 };
        self.fixpoint(opts, bound, if edit { "edit" } else { "cold" })
    }

    /// The cross-run fixpoint, at most `bound` runs.
    fn fixpoint(&mut self, opts: Options, bound: usize, first: &str) -> Report {
        let t0 = Instant::now();
        let src = self.src();
        let mut runs: Vec<RunInfo> = Vec::new();
        let mut notes = Vec::new();
        let mut why = first.to_owned();
        let mut changed: Vec<String> = Vec::new();
        let mut last = None;
        for k in 1..=bound {
            let t = Instant::now();
            let out = self.build.update(&src, opts);
            let s = &out.stats;
            runs.push(RunInfo {
                wall: t.elapsed(),
                why: std::mem::take(&mut why),
                commands_run: s.commands_run,
                commands: s.commands,
                pieces_ran: s.ran,
                pieces: s.pieces,
                pages: s.pages,
                pages_reused: s.pages_reused,
            });
            changed.clear();
            let mut new: Vec<(String, String)> = Vec::new();
            for (n, t) in &out.writes {
                if self.read_back(n).as_deref() != Some(t.as_str()) {
                    changed.push(n.clone());
                    new.push((n.clone(), t.clone()));
                }
            }
            if self.job.tools {
                let dir = self.job.out_dir.clone();
                let made = {
                    let given = &self.given;
                    let known = |n: &str| {
                        given.contains_key(n) || dir.join(n).is_file() || Path::new(n).is_file()
                    };
                    self.tools.after_run(&out.writes, &dir, &known)
                };
                for m in made {
                    notes.push(m.line);
                    if let Some((n, b)) = m.side {
                        self.side.insert(n, b);
                    }
                    if let Some(text) = m.text
                        && self.read_back(&m.file).as_deref() != Some(text.as_str())
                    {
                        changed.push(m.file.clone());
                        self.give(&m.file, text);
                    }
                }
            }
            last = Some(out);
            if !self.job.fixpoint || changed.is_empty() || k == bound {
                break;
            }
            for (n, t) in new {
                self.give(&n, t);
            }
            why.clone_from(&changed.join(", "));
            why.push_str(" changed");
        }
        let out = last.expect("a run");
        Report {
            out,
            runs,
            unsettled: if self.job.fixpoint { changed } else { Vec::new() },
            notes,
            wall: t0.elapsed(),
        }
    }

    /// Writes what the build made: `JOB.pdf` or `JOB.dvi`, the files
    /// TeX wrote, the tools' files, `JOB.log`, `JOB.fls` (`-recorder`).
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
            if self.job.synctex {
                let (n, b) = self.sync.file(&self.build, &self.job.jobname, &self.job.path, &r.out.files_read, self.job.threads);
                put(&n, &b)?;
            }
        }
        for (n, t) in &r.out.writes {
            put(n, &tex::bytes(t))?;
        }
        for (n, t) in &self.given {
            if !r.out.writes.iter().any(|(m, _)| m == n) {
                put(n, &tex::bytes(t))?;
            }
        }
        for (n, b) in &self.side {
            put(n, b)?;
        }
        let out_name = format!("{}.{ext}", self.job.jobname);
        if let Some(l) = r.out.job_logs(&Limits::TEX_LIVE, &out_name).last() {
            put(&format!("{}.log", self.job.jobname), &tex::bytes(&l.log))?;
        }
        if self.job.recorder {
            let fls = self.recorder(r, &out_name);
            put(&format!("{}.fls", self.job.jobname), fls.as_bytes())?;
        }
        Ok(Written { main, files })
    }

    /// `JOB.fls` as `-recorder` writes it, of what is known: the format,
    /// the main file, the outputs. (The files a build read are not
    /// reported by phitex-flow yet: asked for as `Output::files_read`.)
    fn recorder(&self, r: &Report, out_name: &str) -> String {
        let mut s = String::new();
        let cwd = std::env::current_dir().unwrap_or_default();
        s.push_str(&format!("PWD {}\n", cwd.display()));
        if let Some(f) = &self.format {
            s.push_str(&format!("INPUT {}\n", f.display()));
        }
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
        let name = format!("x.{}", r.main_output().map_or("dvi", |(e, _)| e));
        r.out.job_logs(&Limits::TEX_LIVE, &name).pop().map(|l| l.term).unwrap_or_default()
    }
}

/// The notes a job's options that phitex-flow cannot do yet give, once.
pub fn waiting_notes(job: &Job) -> Vec<String> {
    let mut v = Vec::new();
    if job.recorder {
        v.push(
            "-recorder: the .fls lists the format, the main file and the outputs; the files \
             a build read wait on phitex-flow reporting them (Output::files_read, asked)"
                .to_owned(),
        );
    }
    v
}

/// Shared bytes of a report's output, for the viewer.
pub fn shared(b: &[u8]) -> Arc<[u8]> {
    Arc::from(b)
}
