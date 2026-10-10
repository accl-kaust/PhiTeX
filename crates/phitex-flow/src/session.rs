//! A session: TeX run several times in a row, each run (job) reading the
//! terminal's lines and the format the previous one dumped — as Knuth's
//! TRIP test runs it (tripman.tex, Appendix A: `tex -ini` with
//! `trip1.in`, then `tex` with `trip2.in`). On the graph it is one build
//! whose text is the jobs' main files in order (`graph::jobs`): a later
//! job's reads resolve to what the earlier job's `\dump` reached, as any
//! later piece's do. No format file, no second engine.
//!
//! The files the session writes are values joined at the end; a file
//! written and read back in the same run (TRIP's `tripos.tex`) is read as
//! what the run wrote of it before the reading node: a read the build's
//! fold of the written files validates (CORE.md "Reads of folds").

use std::path::PathBuf;
use std::sync::Arc;

use crate::build::{Build, Options};
use crate::print::{JobLog, Limits};
use crate::tfm::Font;

/// What a session is given.
#[derive(Clone, Debug)]
pub struct Session {
    /// Each job's main file, in order.
    pub jobs: Vec<String>,
    /// The job's name (`\jobname`).
    pub jobname: String,
    /// The lines typed at the terminal, all jobs' in order.
    pub terminal: Vec<String>,
    /// The interaction mode before any definition (3: IniTeX's).
    pub interaction: i32,
    pub limits: Limits,
    /// Where `\font` finds TFM files; where `\input` and `\openin` find
    /// files before any is written.
    pub font_dirs: Vec<PathBuf>,
    pub input_dirs: Vec<PathBuf>,
    /// The font the build is made with (its name loads it).
    pub font: Arc<Font>,
}

/// What a session made.
#[derive(Clone, Debug, Default)]
pub struct SessionOut {
    pub dvi: Vec<u8>,
    pub logs: Vec<JobLog>,
    pub writes: Vec<(String, String)>,
    pub errors: Vec<String>,
    /// The builds it took: one (the written files read back settle within
    /// the build).
    pub runs: usize,
}

impl Session {
    /// Runs the session: one build. A file it writes and reads back in the
    /// same run (TRIP's `tripos.tex`) is read as what was written of it
    /// before the reading node (CORE.md "Reads of folds").
    pub fn run(&self, opts: Options) -> SessionOut {
        let texts: Vec<&str> = self.jobs.iter().map(String::as_str).collect();
        let src = crate::graph::jobs(&texts);
        let mut b = Build::new(self.font.clone());
        b.set_font_dirs(self.font_dirs.clone());
        b.set_input_dirs(self.input_dirs.clone());
        b.set_jobname(&self.jobname);
        let lines: Vec<&str> = self.terminal.iter().map(String::as_str).collect();
        b.set_terminal(&lines);
        b.set_interaction(self.interaction);
        b.set_limits(self.limits);
        let o = b.update(&src, opts);
        SessionOut {
            logs: o.job_logs(&self.limits, &format!("{}.dvi", self.jobname)),
            dvi: o.dvi,
            writes: o.writes,
            errors: o.errors,
            runs: 1,
        }
    }
}
