//! The cross-run fixpoint (AGENTS_BRIEF §7): run k+1 reads what run k
//! wrote. A run is the same build updated again, the files run k wrote
//! given as a new version of those files ([`Build::set_file_bytes`]): an
//! edit of run k ("the `.aux` changed"), so the pieces that read them run
//! again and nothing else does. Nothing is copied into a state: the files
//! are inputs like any other. It stops when a run writes what the run
//! before it wrote (the bytes of every file), or after `max` runs (then
//! it oscillates: the files the last run changed say where).
//!
//! [`Fixpoint::run`] is one run, for a caller that does something between
//! runs (the CLI runs bibtex or makeindex when the `.aux` or `.idx` asks,
//! and gives their output with [`Build::set_file`]); [`fixpoint`] is the
//! loop with nothing in between.

use crate::build::{Build, Options, Output};

/// One run of a fixpoint.
pub struct Run {
    /// The run's output.
    pub out: Output,
    /// The written files that differ from the run before's, or that it
    /// wrote and this one did not (all of this run's for the first run):
    /// their names, in the order written.
    pub changed: Vec<String>,
    /// This run wrote what the one before it wrote (no file changed).
    /// (Only the files this build writes: a caller that changed another
    /// input between runs, a `.bbl`, runs again.)
    pub stable: bool,
}

/// The runs of a document so far: what the last one wrote.
#[derive(Default)]
pub struct Fixpoint {
    prev: Option<Vec<(String, String)>>,
    runs: usize,
}

/// A written file's bytes (a character each, as TeX writes them).
fn bytes(text: &str) -> Vec<u8> {
    text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect()
}

impl Fixpoint {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many runs so far.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.runs
    }

    /// Runs `src` once more: `b` updated, then what it wrote given to it
    /// as the next run's files (and what the run before wrote and this one
    /// did not, removed). The first run's files are all changed.
    pub fn run(&mut self, b: &mut Build, src: &str, opts: Options) -> Run {
        self.run_reading(b, src, opts, &|_| None)
    }

    /// [`Fixpoint::run`], a written file not given by a run before
    /// compared with what `read` says the run read as it (the CLI: the
    /// file on disk from an earlier job), so a first run can be stable.
    pub fn run_reading(&mut self, b: &mut Build, src: &str, opts: Options, read: &dyn Fn(&str) -> Option<String>) -> Run {
        let out = b.update(src, opts);
        let empty = Vec::new();
        let prev = self.prev.as_ref().unwrap_or(&empty);
        let mut changed: Vec<String> = out
            .writes
            .iter()
            .filter(|(n, t)| match prev.iter().find(|(m, _)| m == n) {
                Some((_, u)) => u != t,
                None => read(n).as_deref() != Some(t.as_str()),
            })
            .map(|(n, _)| n.clone())
            .collect();
        changed.extend(prev.iter().filter(|(n, _)| !out.writes.iter().any(|(m, _)| m == n)).map(|(n, _)| n.clone()));
        let stable = changed.is_empty();
        if let Some(prev) = &self.prev {
            for (n, _) in prev {
                if !out.writes.iter().any(|(m, _)| m == n) {
                    b.remove_file(n);
                }
            }
        }
        for (n, t) in &out.writes {
            b.set_file_bytes(n, &bytes(t));
        }
        self.prev = Some(out.writes.clone());
        self.runs += 1;
        Run { out, changed, stable }
    }
}

/// The runs of a fixpoint.
pub struct Runs {
    /// Each run's output, in order (the last is the result).
    pub outs: Vec<Output>,
    /// Each run's [`Run::changed`].
    pub changed: Vec<Vec<String>>,
    /// The last run wrote what the one before it wrote.
    pub stable: bool,
}

/// Builds `src` again until what it writes is what it wrote the run
/// before (at most `max` runs, at least one).
pub fn fixpoint(b: &mut Build, src: &str, opts: Options, max: usize) -> Runs {
    let mut f = Fixpoint::new();
    let mut runs = Runs { outs: Vec::new(), changed: Vec::new(), stable: false };
    loop {
        let r = f.run(b, src, opts);
        runs.outs.push(r.out);
        runs.changed.push(r.changed);
        runs.stable = r.stable;
        if r.stable || f.runs() >= max.max(1) {
            return runs;
        }
    }
}
