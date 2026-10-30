//! `--report` (CONTRIBUTING.md): a build's or a watch's account for a bug
//! report, written beside the outputs as `<job>.phitex-report.txt`.
//!
//! - **By default no text of the document:** PhiTeX's version and
//!   identities, the machine (OS, architecture, CPUs), the command and
//!   settings, where TeX's files come from; each build's kind, time, pages
//!   and history; each problem's severity, file, line and message (TeX's
//!   context lines, which quote the source, left out); the warnings by
//!   class; the engine's own account when it failed; the TeX Live files the
//!   job read (package names, public).
//! - **`--report=full`** adds the job's `.log` and the project's own source
//!   files, for a reporter willing to share them.
//! - Under `watch`, one report for the session, written again after each
//!   build.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::session::{Report as BuildReport, Session};

use crate::modern::{Options, Target};

/// Where a report goes: a new issue, its templates to choose from.
pub const ISSUES: &str = "https://github.com/accl-kaust/PhiTeX/issues/new/choose";

/// What a report holds: its head, then each build's account.
pub struct Report {
    full: bool,
    head: String,
    builds: Vec<String>,
    /// The last build's transcript (a full report's sources are the files
    /// it names).
    log: Option<String>,
}

/// The project's files a full report carries: TeX's and bibliographies'
/// sources in the document's directory and below, at most this much.
const FULL_MAX: u64 = 4 << 20;

impl Report {
    /// A report for target `t` run with options `o` (`full`: the log and
    /// the sources too).
    #[must_use]
    pub fn new(t: &Target, o: &Options, full: bool) -> Report {
        let mut h = String::new();
        let _ = writeln!(
            h,
            "PhiTeX report ({})",
            if full {
                "full: with the document's sources and log"
            } else {
                "no document text"
            }
        );
        let _ = writeln!(
            h,
            "phitex {}",
            crate::modern::version_text().replace('\n', "\n  ")
        );
        let _ = writeln!(
            h,
            "machine: {} {}, {} logical CPUs",
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::thread::available_parallelism().map_or(0, std::num::NonZero::get)
        );
        let args: Vec<String> = std::env::args().skip(1).collect();
        let _ = writeln!(h, "command: phitex {}", args.join(" "));
        let _ = writeln!(
            h,
            "settings: engine {}, threads {}, files {:?}, synctex {}, shell-escape {}, output-dir {}",
            t.engine.name,
            o.threads
                .map_or_else(|| "all".to_owned(), |n| n.to_string()),
            t.files,
            t.synctex,
            t.shell_escape,
            t.output_dir.as_deref().unwrap_or(".")
        );
        for w in &t.warnings {
            let _ = writeln!(h, "configuration: {w}");
        }
        Report {
            full,
            head: h,
            builds: Vec::new(),
            log: None,
        }
    }

    /// Build `r` of session `s` (`why`: cold, an edit's place) added.
    pub fn build(&mut self, s: &Session, r: &BuildReport, why: &str) {
        let mut b = String::new();
        let _ = writeln!(
            b,
            "build {}: {why}, {:.1} ms, {} pages, history {}, {} runs, {} errors",
            self.builds.len() + 1,
            r.wall.as_secs_f64() * 1e3,
            r.pages(),
            r.history(),
            r.runs.len(),
            r.out.errors.len()
        );
        for d in &r.diagnostics {
            let _ = writeln!(b, "  {:?} {}:{}: {}", d.severity, d.file, d.line, d.message);
            if self.full && !d.context.is_empty() {
                let _ = writeln!(b, "    {}", d.context.replace('\n', "\n    "));
            }
        }
        // (TeX's errors, their first lines: the rest quotes the source)
        for e in &r.out.errors {
            let first = e.lines().next().unwrap_or_default();
            let _ = writeln!(b, "  error: {first}");
        }
        let log = r.log_text();
        // (by class and count: the names they cite are the document's)
        for (class, n, _) in crate::warnings::summarize(&log) {
            let _ = writeln!(b, "  warnings: {n} × {class:?}");
        }
        for w in &r.warnings {
            let _ = writeln!(b, "  engine warning: {w}");
        }
        if let Some(i) = &r.internal {
            let _ = writeln!(b, "  engine's account:\n    {}", i.replace('\n', "\n    "));
        }
        let read = texlive_files(&log);
        if !read.is_empty() {
            let _ = writeln!(b, "  TeX Live files read: {}", read.join(" "));
        }
        if self.full {
            let _ = writeln!(b, "--- {}.log ---\n{log}", s.job.jobname);
        }
        self.builds.push(b);
        self.log = Some(log);
    }

    /// Written to `<job>.phitex-report.txt` in the outputs' directory (the
    /// sources appended for a full report); its path.
    ///
    /// # Errors
    ///
    /// If the file cannot be written.
    pub fn write(&self, s: &Session, t: &Target) -> Result<PathBuf, String> {
        let mut text = self.head.clone();
        for b in &self.builds {
            text.push('\n');
            text.push_str(b);
        }
        if self.full {
            let root = Path::new(&t.file)
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let root = if root.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                root
            };
            let mut left = FULL_MAX;
            let log = self.log.as_deref().unwrap_or_default();
            for p in sources(&root, log) {
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if body.len() as u64 > left {
                    let _ = writeln!(
                        text,
                        "\n--- {} (left out: the report's {} MB are full) ---",
                        p.display(),
                        FULL_MAX >> 20
                    );
                    continue;
                }
                left -= body.len() as u64;
                let _ = write!(text, "\n--- {} ---\n{body}", p.display());
            }
        }
        let path = s
            .job
            .out_dir
            .join(format!("{}.phitex-report.txt", s.job.jobname));
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

/// The TeX Live files a transcript names (each `(…/texmf-dist/…/name`),
/// by name, in order, once each: the classes and packages a job loaded.
fn texlive_files(log: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for w in log.split(|c: char| c == '(' || c.is_whitespace() || c == ')') {
        if !w.contains("texmf") {
            continue;
        }
        if let Some(name) = w.rsplit('/').next().filter(|n| n.contains('.'))
            && seen.insert(name.to_owned())
        {
            out.push(name.to_owned());
        }
    }
    out
}

/// The project's sources the job read: the files its transcript names
/// outside TeX Live (`(./chapter.tex`), the bibliographies in `root` and
/// below (BibTeX reads them, the transcript does not name them), and
/// `root`'s `phitex.toml`; sorted, once each.
fn sources(root: &Path, log: &str) -> Vec<PathBuf> {
    let source = |p: &Path| {
        p.extension().is_some_and(|x| {
            matches!(
                x.to_str(),
                Some("tex" | "bib" | "sty" | "cls" | "bst" | "ltx")
            )
        })
    };
    let mut out = std::collections::BTreeSet::new();
    for w in log.split(|c: char| c == '(' || c.is_whitespace() || c == ')') {
        let p = Path::new(w);
        if !w.contains("texmf") && source(p) && p.is_file() {
            out.insert(p.to_path_buf());
        }
    }
    let toml = root.join("phitex.toml");
    if toml.is_file() {
        out.insert(toml);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "bib") {
                out.insert(p);
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texlive_files_by_name() {
        let log = "(/usr/share/texmf-dist/tex/latex/base/article.cls\nDocument Class: article\n(/usr/share/texmf-dist/tex/latex/base/size10.clo)) (./paper.aux) (/usr/share/texmf-dist/tex/latex/base/article.cls)";
        assert_eq!(texlive_files(log), ["article.cls", "size10.clo"]);
    }
}
