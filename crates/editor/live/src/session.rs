//! A document's session, kept for the process's life: one
//! [`phitex_tex::Build`], the document's only state. The first build is
//! an edit from nothing, every later one an edit of the main text as it
//! is now; the session writes what the build made.
//!
//! Every format runs from its ini file as the prelude (`tex.ini`,
//! `etex.ini`, `pdfetex.ini`, `latex.ini`, `pdflatex.ini`), its evaluated
//! state kept in the host's store; `-ini` jobs have none. DVI or PDF out.
//!
//! The files, the store and the outputs are the host's ([`crate::host`]):
//! the session decides what is read, kept and written, the host does it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phitex_tex::{Build, Finder, Options};

use crate::host::{Host, Key, Records, Sink, Store};
use crate::tex::{self, Engine};

/// What a session does now, for a terminal's spinner (`ui.rs`), and
/// the phases it went through, each with its time (the watch's summary).
#[derive(Default)]
pub struct Status {
    now: Mutex<Phases>,
    live: Mutex<Option<Live>>,
    /// The build's progress records (the viewer's `progress` event, the
    /// language server's): [`crate::progress`].
    meter: Arc<crate::progress::Meter>,
}

/// The engine at work: its progress (what each worker runs, the jobs done
/// of those known) and the main file's line starts, for the places.
#[derive(Clone)]
pub struct Live {
    pub progress: Arc<phitex_tex::Progress>,
    pub file: String,
    /// The main text, and its line starts made when a place is first
    /// shown (an edit too quick to show one never counts them).
    pub text: Arc<str>,
    pub lines: Arc<std::sync::OnceLock<Vec<usize>>>,
    /// The pages the last build made (0: none, a cold build).
    pub pages_before: usize,
    /// The engine's threads (the idle ones are these less the running).
    pub threads: usize,
}

impl Live {
    /// The line (from 1) of byte `at` of the main text.
    pub fn line_of(&self, at: u64) -> usize {
        self.lines
            .get_or_init(|| {
                std::iter::once(0)
                    .chain(
                        self.text
                            .bytes()
                            .enumerate()
                            .filter(|c| c.1 == b'\n')
                            .map(|c| c.0 + 1),
                    )
                    .collect()
            })
            .partition_point(|&b| b as u64 <= at)
    }
}

#[derive(Default)]
struct Phases {
    /// The phase running, and since when (none: between phases).
    current: Option<(String, Instant)>,
    done: Vec<(String, Duration)>,
}

impl Status {
    /// The build's progress records: listened to by the viewer and the
    /// language server, told the fetches by the host's files.
    pub fn meter(&self) -> &Arc<crate::progress::Meter> {
        &self.meter
    }

    /// Phase `phase` from now on (empty: none); the last one ends.
    pub fn set(&self, phase: impl Into<String>) {
        let phase = phase.into();
        let mut p = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((name, t)) = p.current.take() {
            p.done.push((name, t.elapsed()));
        }
        if !phase.is_empty() {
            p.current = Some((phase, Instant::now()));
        }
    }

    /// The phase running named `phase` from now on, its time kept.
    pub fn relabel(&self, phase: impl Into<String>) {
        let mut p = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((name, _)) = &mut p.current {
            *name = phase.into();
        }
    }

    pub fn phase(&self) -> String {
        self.now
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .current
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    /// The phases ended, each with its time, and the one running with its
    /// time so far (last).
    pub fn phases(&self) -> (Vec<(String, Duration)>, Option<(String, Duration)>) {
        let p = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        (
            p.done.clone(),
            p.current.as_ref().map(|(n, t)| (n.clone(), t.elapsed())),
        )
    }

    /// The engine's progress from now on (none: the engine is not running).
    pub fn set_live(&self, l: Option<Live>) {
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = l;
    }

    pub fn live(&self) -> Option<Live> {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The phases ended so far, forgotten.
    pub fn take(&self) -> Vec<(String, Duration)> {
        std::mem::take(&mut self.now.lock().unwrap_or_else(PoisonError::into_inner).done)
    }
}

/// What to build, and how.
#[derive(Clone, Debug)]
#[allow(dead_code, reason = "etex: the engine has no such option yet")]
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
    /// The engine's threads (none: `PHITEX_THREADS`, else all the cores).
    pub threads: Option<usize>,
    /// `--spec`: when the engine runs steps ahead of their inputs (none:
    /// its default).
    pub spec: Option<String>,
    /// Long pieces' readers wait for them on learned fields
    /// ([`phitex_tex::Options::wait_fields`]; on unless `--no-wait-long`).
    pub wait_long: bool,
    /// The document nodes' records ([`phitex_tex::Options::noderec`])
    /// read and stored (`--node-records`; off by default: amsldoc's cold
    /// build was 20% slower at `-j 8` and `-j 24` with them, and they
    /// gave the engine no prediction it reads).
    pub node_records: bool,
    /// `-synctex`.
    pub synctex: bool,
    /// Whether the build keeps the pages' glyph origins (the viewer's
    /// `origins`; on with `synctex` too).
    pub origins: bool,
    /// `-recorder`: `JOB.fls`.
    pub recorder: bool,
    /// `-shell-escape`: `\write18` runs its commands.
    pub shell_escape: bool,
    /// The PDF's date (`SOURCE_DATE_EPOCH`, else now).
    pub pdf_date: i64,
    /// The files the job reads back (`.aux`, `.toc`) to their fixpoint, as
    /// `latexmk` runs LaTeX until they settle; else one run.
    pub fixpoint: bool,
    /// The cache's size limit (`cache-limit`).
    pub cache_limit: Option<u64>,
    /// An edit's diagnostics at their places (a viewer's, `--diagnostics`);
    /// else an edit reports the engine's own, at no place, only.
    pub edit_diagnostics: bool,
}

/// One run of a build.
#[derive(Clone, Debug, Default)]
pub struct RunInfo {
    pub wall: Duration,
    /// The texts of the main file the build took while it ran.
    pub absorbed: usize,
    /// Why it ran: `cold` or `edit`.
    pub why: String,
    /// Nodes reduced (not kept from the last build).
    pub commands_run: u64,
    /// The loop-back rounds (latexmk's runs of LaTeX).
    pub rounds: u64,
    /// Nodes the build has after it (those not run were kept).
    pub live: usize,
    /// The engine's threads, and its work summed over them (none: no
    /// clock, wasm): busy / wall is how parallel the run was.
    pub threads: usize,
    pub busy: Duration,
    pub engine_wall: Duration,
    /// Runs in all (a node run again counts again), and of the busy time
    /// the runs thrown away (not their node's last).
    pub runs: u64,
    pub wasted: Duration,
    /// The longest chain of nodes each waiting on the last (none: not
    /// known): nodes run / it is the most parallel the run could be.
    pub critical_path: Option<usize>,
    /// The most nodes running at once.
    pub peak_running: usize,
    /// The batches of jobs the engine ran, those run on the calling thread
    /// (one job, or one thread), and the largest.
    pub batches: u64,
    pub batches_inline: u64,
    pub max_batch: usize,
    /// The steps run ahead of their inputs, those confirmed, those run
    /// again, the runs out of fuel; the most steps ready at once.
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    pub fuel_outs: u64,
    pub max_ready: usize,
    /// The parser's hints: nodes bounded, nodes that may define anything,
    /// hints whose assumptions failed, definitions outside a bound, the
    /// stops and pending waits.
    pub hints: [u64; 6],
    /// Memory after the build: resident set (kB; 0: not known), the value
    /// store's payload (bytes), the collections made.
    pub rss_kb: u64,
    pub values_bytes: u64,
    pub collections: u64,
}

impl RunInfo {
    /// How parallel the run was: `4 threads · 1.6× parallel (at most
    /// 2.1×)`, the engine's work over its wall time, and its nodes over
    /// its longest chain (none: one thread, or no clock).
    /// The engine's work over its wall time (none: one thread, no clock).
    pub fn speedup(&self) -> Option<f64> {
        (self.threads >= 2 && !self.engine_wall.is_zero())
            .then(|| self.busy.as_secs_f64() / self.engine_wall.as_secs_f64())
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn parallel(&self) -> Option<String> {
        if self.threads < 2 || self.engine_wall.is_zero() {
            return None;
        }
        let p = self.busy.as_secs_f64() / self.engine_wall.as_secs_f64();
        let mut s = format!("{} threads · {p:.1}× parallel", self.threads);
        // (the chain is the whole document's: on an edit that ran fewer
        // nodes than it, the bound says nothing)
        if let Some(c) = self
            .critical_path
            .filter(|&c| c > 0 && self.commands_run >= c as u64)
        {
            let _ = std::fmt::Write::write_fmt(
                &mut s,
                format_args!(" (at most {:.1}×)", self.commands_run as f64 / c as f64),
            );
        }
        Some(s)
    }
}

/// What one build made.
#[derive(Clone, Debug, Default)]
pub struct Output {
    /// The PDF (pdfTeX), else empty (the engine's bytes, shared).
    pub pdf: Arc<Vec<u8>>,
    /// The DVI (TeX, e-TeX), else empty.
    pub dvi: Arc<Vec<u8>>,
    pub pages: usize,
    /// The pages (from 1) that differ from the last build's.
    pub pages_changed: Vec<usize>,
    /// Each page's hash, first first.
    pub page_hashes: Vec<u64>,
    pub errors: Vec<String>,
    /// TeX's history is `error_message_issued`: an error, or a `\\show`
    /// of any kind, which is one without a line of its own (§1293).
    pub history_error: bool,
    /// The terminal's and the log's text.
    pub log: String,
    /// The files TeX wrote.
    pub writes: Vec<(String, String)>,
    /// The files `\write18`'s commands made (`None`: removed), as
    /// pdflatex leaves them in its directory.
    pub shell_files: Vec<(String, Option<Vec<u8>>)>,
    /// The PDF file's layout, where the session writes update sections
    /// ([`Session::updates`]; `pdf` is then empty).
    pub layout: Option<phitex_pdf::Layout>,
}

/// What a build made.
pub struct Report {
    pub out: Output,
    /// The first line (from 1) of the main text that differs from the
    /// last build's (none: cold, or the main text unchanged).
    pub edit_line: Option<usize>,
    pub runs: Vec<RunInfo>,
    /// The engine's warnings at no place, said for a reader
    /// ([`crate::failure::warning`]): the page layout not settled.
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
    pub wall: Duration,
    /// The engine's own account of why it stopped (its state dumped: for
    /// `-v` and a bug report, not the error shown).
    pub internal: Option<String>,
    /// The errors and warnings at their places, by file then byte.
    pub diagnostics: Vec<phitex_tex::Diagnostic>,
    /// The format could not be had (its ini file not found): the one
    /// error says why.
    pub format_failed: bool,
}

/// An engine's stop said in a sentence: what gave out, without its dump
/// (`RunawayCreation { node: …, depth: 4097, … }` as "it nested deeper than
/// its limit (4097 levels)").
pub fn engine_stop(e: &str) -> String {
    let num = |key: &str| {
        e.split(key).nth(1).and_then(|r| {
            let d: String = r
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!d.is_empty()).then_some(d)
        })
    };
    let what = if e.contains("RunawayCreation") {
        format!(
            "it nested deeper than its limit ({} levels)",
            num("depth:").unwrap_or_else(|| "4096".to_owned())
        )
    } else if e.contains("Deadline") {
        "it ran past its time limit".to_owned()
    } else {
        // (the first clause, before any dump)
        let first = e.lines().next().unwrap_or(e);
        let cut = first
            .find([':', '[', '{', '('])
            .map_or(first, |k| &first[..k]);
        cut.trim().to_owned()
    };
    format!(
        "PhiTeX's engine stopped: {what}. This is a bug in PhiTeX, not in the document (-v shows the engine's details)"
    )
}

impl Report {
    /// The output file's extension and bytes (none: no pages).
    pub fn main_output(&self) -> Option<(&'static str, &[u8])> {
        if !self.out.pdf.is_empty() {
            Some(("pdf", self.out.pdf.as_slice()))
        } else if !self.out.dvi.is_empty() {
            Some(("dvi", self.out.dvi.as_slice()))
        } else {
            None
        }
    }

    /// The pages shipped.
    pub fn pages(&self) -> usize {
        self.out.pages
    }

    /// The engine's own diagnostics (an undeclared definition, a hint's
    /// regime violated): counted apart from TeX's errors, never part of
    /// the history or the exit status.
    pub fn engine_diagnostics(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == phitex_tex::Severity::Diagnostic)
            .count()
    }

    /// TeX's history: 0 spotless, 1 warnings, 2 errors.
    pub fn history(&self) -> i32 {
        if !self.out.errors.is_empty() || self.out.history_error {
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
    /// The main file's text, as TeX's characters (shared, not copied,
    /// on its way to the engine).
    text: Arc<str>,
    /// The document's state: none before the first build.
    build: Option<Build>,
    /// The main text the last build built.
    built_text: Option<Arc<str>>,
    /// The last build's pages' hashes.
    page_hashes: Vec<u64>,
    /// The machine: the files the build reads (the CLI's kpathsea), the
    /// services it calls.
    host: Arc<dyn Host>,
    /// The bytes kept between sessions (none: nothing kept).
    store: Option<Records>,
    /// Where the outputs go.
    sink: Arc<dyn Sink>,
    builds: usize,
    /// The written keys, the package records, the written files' names
    /// and the document nodes' records last stored in the cache
    /// ([`Session::store_wkeys`], [`Session::store_pkgrec`],
    /// [`Session::store_wnames`], [`Session::store_noderec`]).
    #[allow(clippy::type_complexity)]
    stored: Mutex<(
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    )>,
    /// What the written keys and names last stored were made from
    /// ([`Build::wkeys_source`]): the same, nothing to make or store.
    wkeys_from: Mutex<Option<u64>>,
    /// What the session does now, for a spinner.
    status: Arc<Status>,
    /// Unsaved editor buffers, by file name (`None`: as on disk again),
    /// which the finder reads first.
    buffers: Arc<Mutex<Vec<(String, Option<String>)>>>,
    /// The files other than the main one changed since the last build.
    changed: Vec<String>,
    /// Every name the builds since the last cold one asked the finder
    /// for, in any evaluation, whether it was found the last time
    /// ([`Build::reads`] is the last evaluation's only), and the file
    /// the host found for it, looked up once, when the name came (or
    /// when it was first found): an edit compares names, not lookups.
    inputs: Vec<(String, bool, Option<PathBuf>)>,
    /// Whether the package loads' and the document nodes' records
    /// changed since last stored ([`Session::flush_records`], called when
    /// a build settles: a no-op unless a load changed them).
    records_dirty: bool,
    /// The texts sent to the build while it runs, by version.
    absorb: Absorb,
    /// When the change came that the last build took last while it ran.
    absorbed_at: Option<Instant>,
    /// A watch's session: no log laid out after a build (`--log`: with
    /// the exact file when idle, [`Session::write_log`]).
    pub watching: bool,
    /// `JOB.synctex.gz`'s members made last (`-synctex`).
    synctex: Mutex<crate::synctex::Writer>,
    /// The main text's lines counted by block ([`LineMarks`]).
    lines: LineMarks,
}

/// The lines before each block of [`LineMarks::BLOCK`] bytes of the
/// last text counted, those of the blocks an edit left as they were
/// kept: an edit's first changed line counted from its block's start,
/// not from the text's.
#[derive(Default)]
struct LineMarks {
    /// The lines before block `k`'s start, for the first blocks.
    marks: Vec<usize>,
}

impl LineMarks {
    const BLOCK: usize = 1 << 20;

    /// The lines before byte `n` of `t`, whose bytes before `n` are the
    /// last text's (an edit's first change at `n`); the blocks from `n`
    /// on forgotten.
    fn before(&mut self, t: &str, n: usize) -> usize {
        let k = n / Self::BLOCK;
        while self.marks.len() <= k {
            let j = self.marks.len();
            let m = match j.checked_sub(1) {
                Some(i) => {
                    self.marks[i] + newlines(&t.as_bytes()[i * Self::BLOCK..j * Self::BLOCK])
                }
                None => 0,
            };
            self.marks.push(m);
        }
        self.marks.truncate(k + 1);
        self.marks[k] + newlines(&t.as_bytes()[k * Self::BLOCK..n])
    }
}

/// The main text sent to a running build from another thread: the build
/// takes the latest between its steps ([`phitex_tex::EditSender`]). Each
/// text sent is kept with when it came, until a result is for it or a
/// later one.
#[derive(Clone, Default)]
pub struct Absorb {
    tx: phitex_tex::EditSender,
    sent: Arc<Mutex<BTreeMap<u64, (Arc<str>, Instant)>>>,
}

impl Absorb {
    /// Sends the main text (TeX's characters): its version.
    pub fn send(&self, text: &str, came: Instant) -> u64 {
        self.send_shared(Arc::from(text), came)
    }

    /// [`Absorb::send`], the text shared.
    pub fn send_shared(&self, text: Arc<str>, came: Instant) -> u64 {
        self.send_edit(text, None, came)
    }

    /// [`Absorb::send_shared`] with what is known of the change
    /// ([`phitex_tex::Hint`]).
    pub fn send_edit(&self, text: Arc<str>, hint: Option<phitex_tex::Hint>, came: Instant) -> u64 {
        let mut m = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        let v = self.tx.send_edit(text.clone(), hint);
        m.insert(v, (text, came));
        v
    }

    /// Version `v`'s text and when it came, the earlier ones forgotten.
    fn take(&self, v: u64) -> Option<(Arc<str>, Instant)> {
        let mut m = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        let got = m.get(&v).cloned();
        m.retain(|&k, _| k > v);
        got
    }
}

/// The engine's threads: `-j N`, else `PHITEX_THREADS`, else every
/// logical CPU this process may run on (hyperthreads included: Linux's
/// affinity mask and cgroup quota, `available_parallelism`), as the
/// engine itself counts them.
pub fn threads(given: Option<usize>) -> usize {
    given
        .filter(|&n| n > 0)
        .or_else(|| {
            std::env::var("PHITEX_THREADS")
                .ok()?
                .parse()
                .ok()
                .filter(|&n| n > 0)
        })
        .unwrap_or_else(phitex_tex::exec_cpus)
}

impl Session {
    /// A session for `job`, its main file's text `text` (TeX's
    /// characters), on `host`, keeping what it keeps in `store`, writing
    /// to `sink`.
    pub fn new(
        job: Job,
        text: Arc<str>,
        host: Arc<dyn Host>,
        store: Option<Arc<dyn Store>>,
        sink: Arc<dyn Sink>,
        status: Arc<Status>,
    ) -> Session {
        Session {
            job,
            text,
            build: None,
            built_text: None,
            page_hashes: Vec::new(),
            host,
            store: store.map(Records::new),
            sink,
            builds: 0,
            stored: Mutex::new((None, None, None, None)),
            wkeys_from: Mutex::new(None),
            status,
            buffers: Arc::default(),
            changed: Vec::new(),
            inputs: Vec::new(),
            records_dirty: false,
            absorb: Absorb::default(),
            absorbed_at: None,
            watching: false,
            synctex: Mutex::default(),
            lines: LineMarks::default(),
        }
    }

    /// Its records read and written under stamp `stamp`
    /// ([`Records::stamped`]: another engine's, in tests).
    #[cfg(test)]
    pub(crate) fn stamp_records(&mut self, stamp: u64) {
        if let Some(r) = &mut self.store {
            *r = r.restamped(stamp);
        }
    }

    /// The host: the files the build reads, the services it calls.
    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// What the session does now: its phases and its progress records
    /// ([`Status::meter`]).
    pub fn status(&self) -> &Arc<Status> {
        &self.status
    }

    /// The host's sink, where the session writes.
    pub fn sink(&self) -> &Arc<dyn Sink> {
        &self.sink
    }

    /// The main file's text from now on (TeX's characters).
    pub fn set_text(&mut self, text: String) {
        self.text = Arc::from(text);
    }

    /// File `name` changed: its text from now on is `text` (an editor's
    /// unsaved buffer, TeX's characters), or none: as on disk.
    pub fn set_buffer(&mut self, name: &str, text: Option<&str>) {
        let name = plain_name(name);
        let mut bufs = self.buffers.lock().unwrap_or_else(PoisonError::into_inner);
        bufs.retain(|(n, _)| *n != name);
        bufs.push((name.clone(), text.map(str::to_owned)));
        drop(bufs);
        self.changed.push(name);
    }

    /// The next build cold: the engine's build dropped (its format kept
    /// in the store), every file read again (`phitex.rebuild`).
    pub fn forget_build(&mut self) {
        self.reread_all();
        self.build = None;
        self.built_text = None;
    }

    /// Every file the builds read is to be read again from disk at the
    /// next build (`r` in watch: a change made outside, unseen).
    pub fn reread_all(&mut self) {
        self.changed
            .extend(self.inputs.iter().map(|(n, ..)| plain_name(n)));
    }

    /// A handle to send the main text to a build while it runs, from
    /// another thread (the cold build's included: it takes them too).
    pub fn absorb(&self) -> Absorb {
        self.absorb.clone()
    }

    /// Whether the last build is for the files as they are now (an edit
    /// sent while it ran was taken by it).
    pub fn up_to_date(&self) -> bool {
        self.build.is_some()
            && self.changed.is_empty()
            && self
                .built_text
                .as_ref()
                .is_some_and(|b| same_text(b, &self.text_now()))
    }

    /// When the latest change the last build took came, if it took one
    /// sent while it ran.
    pub fn absorbed_at(&self) -> Option<Instant> {
        self.absorbed_at
    }

    /// Builds: the first time from nothing, then as an edit of the main
    /// text as it is now. A job the engine cannot run yet reports why, as
    /// its one error.
    pub fn build(&mut self) -> Report {
        let t0 = Instant::now();
        let why = if self.build.is_some() { "edit" } else { "cold" };
        self.builds += 1;
        let r = self.run(why, t0);
        self.status.set("");
        r
    }

    fn run(&mut self, why: &str, t0: Instant) -> Report {
        let fail = |e: String, internal: Option<String>| Report {
            out: Output {
                errors: vec![e],
                ..Output::default()
            },
            runs: vec![RunInfo {
                wall: t0.elapsed(),
                why: why.to_owned(),
                ..RunInfo::default()
            }],
            edit_line: None,
            warnings: Vec::new(),
            notes: Vec::new(),
            wall: t0.elapsed(),
            internal,
            diagnostics: Vec::new(),
            format_failed: false,
        };
        let mut text = self.text_now();
        phitex_tex::timeline::lap("session: the text now (copied)");
        self.absorbed_at = None;
        // (what changed, against the text the last build took: the first
        // changed line, and the engine told the ends the texts share)
        let ends = self.built_text.as_ref().map(|old| {
            (
                old.clone(),
                phitex_tex::byte_ends(old.as_bytes(), text.as_bytes()),
            )
        });
        let mut edit_line = ends.as_ref().and_then(|(old, (p, _))| {
            (old.len() != text.len() || *p < old.len()).then(|| self.lines.before(&text, *p) + 1)
        });
        phitex_tex::timeline::lap("session: the first changed line (texts compared)");
        let changed = std::mem::take(&mut self.changed);
        let reread = if self.build.is_some() {
            self.reads_of(&changed)
        } else {
            Vec::new()
        };
        let cold = self.build.is_none();
        // (an evaluation error panics, naming the nodes: the build is
        // reported failed and dropped, the next one cold)
        // (the main file's line starts: a worker's place as file:line)
        let lines: Arc<std::sync::OnceLock<Vec<usize>>> = Arc::default();
        phitex_tex::timeline::lap("session: files to read again, the line starts");
        let file = plain_name(&self.job.path.to_string_lossy());
        let pages_before = self.page_hashes.len();
        let n_threads = threads(self.job.threads);
        let live = |progress| Live {
            progress,
            file: file.clone(),
            text: text.clone(),
            lines: lines.clone(),
            pages_before,
            threads: n_threads,
        };
        let ran = crate::panics::catch(|| -> Result<(), String> {
            if let Some(b) = &mut self.build {
                let progress = b.progress();
                self.status.meter().build_begins(progress.evaluation());
                self.status.set_live(Some(live(progress)));
                self.status.set("engine");
                // (evaluated with the text, one evaluation: its stats
                // count the nodes the files' change ran)
                for name in &reread {
                    b.stage_file(name, None);
                }
                // (sent as a version of its own: what another thread sends
                // while it runs is taken between the engine's steps)
                self.absorb.send_edit(
                    text.clone(),
                    ends.as_ref().map(|(old, (p, s))| (old.clone(), *p, *s)),
                    Instant::now(),
                );
                phitex_tex::timeline::lap("session: the text sent to the engine (copied)");
                b.absorb_pending();
                phitex_tex::timeline::lap("engine: the rest of the edit");
            } else {
                // (the format phase lasts until the engine has it, read
                // from the cache or made: the engine says when,
                // `Progress::on_format`, and the document's phase begins)
                self.status.set("format");
                let (prelude, cached) = self.prelude()?;
                phitex_tex::timeline::lap("session: the format found, read from the cache");
                // (a format not cached is built first, in this build: slow
                // until the kernel is fast, so a longer time limit for it;
                // none would leave a stuck evaluation running for ever)
                let first = prelude.is_some() && cached.is_none();
                let workers = threads(self.job.threads);
                let xe = self.job.engine.xetex();
                let progress = Arc::new(phitex_tex::Progress::new(workers));
                // (its pages the session's progress records)
                self.status.meter().build_begins(0);
                self.status.meter().watch(&progress);
                {
                    let (status, made) = (self.status.clone(), format!("{} format, made", self.job.engine.name));
                    progress.on_format(Box::new(move |ready| {
                        if ready {
                            status.set("engine");
                        } else {
                            status.relabel(made.as_str());
                        }
                        status.meter().format(ready);
                    }));
                }
                self.host.progress(&progress);
                self.status.set_live(Some(live(progress.clone())));
                let xetex = if xe {
                    self.host.xetex().map(|x| phitex_tex::XeTeX {
                        pdf: self.xdv_pdf(),
                        ..x
                    })
                } else {
                    None
                };
                phitex_tex::timeline::lap("session: XeTeX's fonts and PDF files");
                let finder = self.finder();
                let prelude_finder = Some(self.format_finder());
                phitex_tex::timeline::lap("session: the finders");
                let (content_key, tool_finder, bibtex) = (
                    self.host.content_key(),
                    self.host.tool_finder(),
                    self.host.bibtex(),
                );
                phitex_tex::timeline::lap("session: the content key, tools, bibtex");
                let shell_hook = self.host.shell_hook(&self.job);
                phitex_tex::timeline::lap("session: the shell hook");
                let opts = Options {
                    name: self.printed_name(),
                    jobname: self.job.jobname.clone(),
                    comment: if xe {
                        dvi_comment(self.job.pdf_date).replacen(" TeX", " XeTeX", 1)
                    } else {
                        dvi_comment(self.job.pdf_date)
                    },
                    // (XeTeX's native fonts: TeX Live's, by file and by
                    // name; its PDF written from the pages, as xelatex has
                    // xdvipdfmx make it, unless the XDV is the output)
                    xetex,
                    finder,
                    prelude_finder,
                    content_key,
                    tool_finder,
                    bibtex,
                    epoch: Some(self.job.pdf_date),
                    fixpoint: self.job.fixpoint,
                    prelude_jobname: self.job.engine.name.to_owned(),
                    prelude_bytes: cached.as_ref().map(|(_, b)| b.clone()),
                    threads: self.job.threads,
                    progress: Some(progress),
                    // (pdfTeX's `\pdfshellescape`: 1 with -shell-escape,
                    // else TeX Live's restricted default)
                    shell_escape: if self.job.shell_escape { 1 } else { 2 },
                    // (the commands neither in process nor refused: run as
                    // processes, each in a scratch directory of its own)
                    shell_hook,
                    speculation: match self.job.spec.as_deref() {
                        Some("off") => phitex_tex::Speculation::Off,
                        Some("all") => phitex_tex::Speculation::All,
                        Some("predicted") => phitex_tex::Speculation::Predicted,
                        _ => Options::default().speculation,
                    },
                    wkeys: self.load_wkeys(),
                    written_init: self.written_init(),
                    // (the package loads' records: machine-wide, beside the
                    // format, from every document built with it)
                    pkgrec: self
                        .pkgrec_key()
                        .and_then(|n| self.store.as_ref()?.get(Key::Format(&n, "pkgrec")))
                        .unwrap_or_default(),
                    // (the document nodes' records: the document's own)
                    noderec: self
                        .noderec_kept()
                        .then(|| self.store.as_ref()?.get(Key::Doc("noderec")))
                        .flatten()
                        .unwrap_or_default(),
                    node_records: self.job.node_records,
                    // (a recorded node's outputs predicted from its record,
                    // so its readers run ahead on them)
                    pkg_predict: true,
                    origins: self.job.origins || self.job.synctex,
                    ..Options::default()
                };
                let opts = Options {
                    edits: Some(self.absorb.tx.clone()),
                    // (`PHITEX_TIME_LIMIT_SECS`: a longer limit for a
                    // measurement of a document the kernel is still slow on)
                    time_limit: match std::env::var("PHITEX_TIME_LIMIT_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                    {
                        Some(secs) => Some(Duration::from_secs(secs)),
                        None if first => Some(Duration::from_mins(30)),
                        None => opts.time_limit,
                    },
                    ..opts
                };
                phitex_tex::timeline::lap("session: the options (records read)");
                let b = Build::with(opts, prelude.as_deref(), &text);
                phitex_tex::timeline::lap("engine: the build returned");
                self.store_prelude(&b, prelude.as_deref(), cached.map(|(k, _)| k));
                phitex_tex::timeline::lap("session: the format stored (when new)");
                self.build = Some(b);
            }
            Ok(())
        });
        self.status.set_live(None);
        match ran {
            Ok(Ok(())) => {}
            // (the format's ini file not found: the one way here)
            Ok(Err(e)) => {
                return Report {
                    format_failed: true,
                    ..fail(e, None)
                };
            }
            Err(e) => {
                self.build = None;
                self.built_text = None;
                return fail(engine_stop(&e), Some(e));
            }
        }
        // (the names the build asked for, in every evaluation since the
        // cold one: the files a later change of one is matched against)
        if let Some(b) = &self.build {
            if cold {
                // (an edit may take what the cold build took, five times
                // over, at least a minute, before it is stuck: held to
                // what this document has shown it needs, not to a clock
                // a slow machine or a large document would trip;
                // `PHITEX_STUCK_SECS` over it)
                if std::env::var_os("PHITEX_STUCK_SECS").is_none() {
                    b.set_stuck_after(Some((t0.elapsed() * 5).max(Duration::from_secs(60))));
                }
                self.inputs.clear();
            }
            for (n, found) in b.reads() {
                if let Some(e) = self.inputs.iter_mut().find(|(m, ..)| *m == n) {
                    if found && e.2.is_none() {
                        e.2 = self.host.locate(&n);
                    }
                    e.1 = found;
                } else {
                    let p = found.then(|| self.host.locate(&n)).flatten();
                    self.inputs.push((n, found, p));
                }
            }
        }
        // (the result is for the latest text the build took)
        if let Some(b) = &self.build
            && let Some((t, came)) = self.absorb.take(b.version())
            && !same_text(&t, &text)
        {
            edit_line = self
                .built_text
                .as_deref()
                .and_then(|old| first_difference_line(old, &t));
            text = t;
            self.lines = LineMarks::default();
            self.absorbed_at = Some(came);
            self.text.clone_from(&text);
        }
        phitex_tex::timeline::lap("session: the text the result is for");
        // (`PHITEX_TEX_TIME`: the session's phases after the engine's)
        let mut phases: Vec<(&str, Duration)> = vec![("engine", t0.elapsed())];
        let mut tp = Instant::now();
        let mut lap = |phases: &mut Vec<(&'static str, Duration)>, name: &'static str| {
            phases.push((name, tp.elapsed()));
            phitex_tex::timeline::lap(&format!("session: {name}"));
            tp = Instant::now();
        };
        if let Some(b) = &self.build {
            // (both made from the written files' accumulator: the same
            // as last stored, nothing to make or store)
            let from = b.wkeys_source();
            let same = {
                let mut last = self
                    .wkeys_from
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let same = from.is_some() && *last == from;
                *last = from;
                same
            };
            if !same {
                self.store_wkeys(b);
                self.store_wnames(b);
            }
            lap(&mut phases, "written keys");
            // (no load ran: its records are as last stored; else stored
            // when the build settles, [`Session::flush_records`])
            if b.pkgrec_changed() {
                self.records_dirty = true;
            }
            // (`PHITEX_CHAIN_DUMP=1`: the critical chain in full, and its
            // waits, for measurements)
            if phitex_tex::OBSERVE && std::env::var_os("PHITEX_CHAIN_DUMP").is_some() {
                for l in b.critical_chain_lines() {
                    eprintln!("CHAIN {}", l.replace('\n', " | "));
                }
                for l in b.chain_waits() {
                    eprintln!("WAIT {}", l.replace('\n', " | "));
                }
                for (t, md, cause, reads) in b.node_declarations() {
                    eprintln!("DECL\t{md}\t{cause}\t{reads}\t{t}");
                }
            }
            // (the package loads' counters, for measurements)
            if phitex_tex::PROFILE && std::env::var_os("PHITEX_PKG_STATS").is_some() {
                let s = &b.stats;
                eprintln!(
                    "PKG loads {} claimed {} recorded {} optimistic {} runs {} at-once {} span {:.3} on-critical {} undeclared {} critical-path {:?} threads {} engine-wall {:.3} speculations {} confirmations {} mispredictions {} predicted-confirmed {} predicted-refuted {}",
                    s.pkg_loads,
                    s.pkg_claimed,
                    s.pkg_recorded,
                    s.pkg_optimistic,
                    s.pkg_runs,
                    s.pkg_max_concurrent,
                    s.pkg_wall.as_secs_f64(),
                    s.pkg_on_critical,
                    s.undeclared_definitions,
                    s.critical_path,
                    s.threads,
                    s.wall.as_secs_f64(),
                    s.speculations,
                    s.confirmations,
                    s.mispredictions,
                    s.predictions_confirmed,
                    s.predictions_refuted
                );
            }
        }
        lap(&mut phases, "records");
        let gc = (Duration::ZERO, 0);
        self.status.set("output");
        let b = self.build.as_ref().expect("built above");
        // (a watch lays no log out; a file written as update sections
        // takes the link's layout, not the exact bytes)
        let layout = self.sink.updates().then(|| b.pdf_layout()).flatten();
        let (bytes, log) = if layout.is_some() {
            (Arc::default(), String::new())
        } else if self.watching {
            (b.output_bytes(), String::new())
        } else {
            b.output_shared()
        };
        lap(&mut phases, "output");
        let pdf = layout.is_some() || b.is_pdf();
        let s = &b.stats;
        let diagnostics = if why == "edit" && !self.job.edit_diagnostics {
            b.unplaced_diagnostics()
        } else {
            b.diagnostics()
        };
        // (the engine's warnings at no place: the page layout not
        // settled; the front ends print them)
        let warnings: Vec<String> = diagnostics
            .iter()
            .filter(|d| {
                d.severity == phitex_tex::Severity::Warning
                    && d.range == (0, 0)
                    && d.context.is_empty()
            })
            .map(|d| crate::failure::warning(&d.message))
            .collect();
        lap(&mut phases, "diagnostics");
        // (and the evaluation's own errors — a stop — which the log does
        // not have: errors at no place, without context, each said as a
        // sentence, `crate::failure`; their whole text is the report's
        // `internal`, for -v. The files read back still changing at
        // latexmk's maximum runs are the document's failure, not the
        // engine's: an error, the output written, no report)
        let no_fixpoint = b.no_fixpoint();
        let failures: Vec<&str> = diagnostics
            .iter()
            .filter(|d| {
                d.severity == phitex_tex::Severity::Error
                    && d.range == (0, 0)
                    && d.context.is_empty()
                    && !no_fixpoint.contains(&d.message)
            })
            .map(|d| d.message.as_str())
            .collect();
        let internal = (!failures.is_empty()).then(|| failures.join("\n"));
        // (the format's own errors first, with their places: a format
        // that stopped on them is not the oracle's, whatever follows)
        // (the engine's failures first — a deadline, no fixpoint, an
        // evaluation error: the result is not the document's — then the
        // format's errors, then TeX's; a failure's first line, cut, as
        // its own error: it never hides behind TeX's)
        let history_error = b.errors_logged();
        let errors: Vec<String> = diagnostics
            .iter()
            .filter(|d| {
                d.severity == phitex_tex::Severity::Error
                    && d.range == (0, 0)
                    && d.context.is_empty()
            })
            .map(|d| {
                if phitex_tex::OBSERVE && std::env::var_os("PHITEX_EVAL_DUMP").is_some() {
                    eprintln!("eval-dump: {}", d.message);
                }
                if no_fixpoint.contains(&d.message) {
                    crate::failure::warning(&d.message)
                } else {
                    crate::failure::explain(&d.message)
                }
            })
            .chain(
                b.prelude_diagnostics()
                    .iter()
                    .map(|d| format!("! {} ({}:{})", d.message, d.file, d.line)),
            )
            .chain(if self.watching {
                b.error_lines()
            } else {
                log.lines()
                    .filter(|l| l.starts_with("! "))
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
        // (a PDF file's pages: not known until the engine reports them;
        // the viewer counts the PDF's itself and draws each page again)
        lap(&mut phases, "errors");
        let hashes = b.pages();
        let pages = hashes.len();
        let writes = b
            .files()
            .into_iter()
            .map(|(n, t)| (plain_name(&n), t))
            .collect();
        let shell_files = b.shell_files();
        let pages_changed: Vec<usize> = (0..hashes.len())
            .filter(|&k| self.page_hashes.get(k) != Some(&hashes[k]))
            .map(|k| k + 1)
            .collect();
        self.page_hashes.clone_from(&hashes);
        self.built_text = Some(text);
        lap(&mut phases, "pages and files");
        // (the output's bytes moved to their place, not copied)
        let (pdf_bytes, dvi_bytes) = if pdf {
            (bytes, Arc::default())
        } else {
            (Arc::default(), bytes)
        };
        // (XeTeX's pages are a PDF the engine wrote, as xelatex has
        // xdvipdfmx make it: an XDV here is one no host's files could make
        // a PDF of)
        let mut errors = errors;
        if self.job.engine.output == tex::Output::Xdv && !dvi_bytes.is_empty() {
            errors.push("no xdvipdfmx here: XeTeX's XDV is not made a PDF".to_owned());
        }
        let wall = t0.elapsed();
        if phitex_tex::tex_time() {
            let shown: Vec<String> = phases.iter().map(|(n, d)| format!("{n} {d:?}")).collect();
            eprintln!("session phases ({why}): {}", shown.join(", "));
        }
        let mut report = Report {
            out: Output {
                pdf: pdf_bytes,
                dvi: dvi_bytes,
                writes,
                shell_files,
                pages,
                pages_changed,
                page_hashes: hashes,
                errors,
                history_error,
                log,
                layout,
            },
            edit_line,
            runs: vec![RunInfo {
                wall,
                // (a text taken before the engine's first step, while the
                // format loaded, is not among the engine's)
                absorbed: s
                    .absorbed
                    .len()
                    .max(usize::from(self.absorbed_at.is_some())),
                why: why.to_owned(),
                commands_run: s.nodes_run,
                rounds: s.loop_rounds,
                live: s.live,
                threads: s.threads,
                // (the engine's own measure: the client's is a profile
                // build's only, zero in a release)
                busy: if s.busy.is_zero() { s.engine_busy } else { s.busy },
                engine_wall: s.wall,
                runs: s.runs,
                wasted: s.engine_wasted,
                critical_path: s.critical_path,
                peak_running: s.peak_running,
                batches: s.batches,
                batches_inline: s.batches_inline,
                max_batch: s.jobs_max_batch,
                speculations: s.speculations,
                confirmations: s.confirmations,
                mispredictions: s.mispredictions,
                fuel_outs: s.fuel_outs,
                max_ready: s.max_ready,
                hints: [
                    s.nodes_bounded,
                    s.nodes_any,
                    s.regime_violations,
                    s.undeclared_definitions,
                    s.stops,
                    s.pending_waits,
                ],
                rss_kb: s.rss_kb,
                values_bytes: s.values_bytes,
                collections: s.collections,
            }],
            internal,
            diagnostics,
            warnings,
            format_failed: false,
            notes: std::iter::once(format!(
                "{} nodes run of {} live, {} runs, {} cutoffs, {} loop rounds (runs by pass {:?}), realised cp {} nodes; guesses refuted: {} claims, {} effects, {} claim siblings, {} definitions; {} nodes placed, {} freed",
                s.nodes_run,
                s.live,
                s.runs,
                s.cutoffs,
                s.loop_rounds,
                s.pass_runs,
                s.realised_critical_path,
                s.claims_refuted,
                s.effects_refuted,
                s.claim_siblings,
                s.defs_refuted,
                s.created,
                s.freed
            ))
            .chain(std::iter::once({
                let (nodes, units, values) = b.sizes();
                // (the process's resident set, as the host knows it)
                let rss = self.host.resident_kb();
                format!(
                    "engine tables: {nodes} nodes, {units} units, {values} values, {rss} kB resident, {} us collecting ({} nodes freed)",
                    gc.0.as_micros(),
                    gc.1
                )
            }))
            .collect(),
            wall,
        };
        // (the engine failed: its account written for a bug report and
        // named after the errors; the engine dropped, so the next build
        // starts cold instead of running the failed node again)
        if let Some(internal) = report.internal.clone() {
            // (`the main file byte N`, the engine's place in the main
            // text, said as its line)
            if let Some(text) = &self.built_text {
                let file = self.job.file.clone();
                let at_line = |s: &str| -> String {
                    let key = "the main file byte ";
                    let Some(k) = s.find(key) else {
                        return s.to_owned();
                    };
                    let rest = &s[k + key.len()..];
                    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                    let Ok(pos) = digits.parse::<usize>() else {
                        return s.to_owned();
                    };
                    let lineno = text[..pos.min(text.len())].matches('\n').count() + 1;
                    format!("{}{file}:{lineno}{}", &s[..k], &rest[digits.len()..])
                };
                report.out.errors = report.out.errors.iter().map(|e| at_line(e)).collect();
                report.internal = Some(at_line(&internal));
            }
            let internal = report.internal.clone().unwrap_or(internal);
            let threads = report.runs.last().map_or(0, |x| x.threads);
            if let Some(p) = self.host.failure_report(
                &self.job.file,
                why,
                threads,
                &internal,
                &report.out.errors,
            ) {
                report.out.errors.push(format!("the report for it: {p}"));
            }
            self.build = None;
            self.built_text = None;
        }
        report
    }

    /// File `name`'s bytes as a diagnostic names it: the main text as the
    /// build read it, another file from the host (none: not readable).
    pub fn source(&self, name: &str) -> Option<Vec<u8>> {
        let n = plain_name(name);
        if n == plain_name(&self.job.path.to_string_lossy()) {
            return self.built_text.as_deref().map(tex::bytes);
        }
        self.host.read(Path::new(&n))
    }

    /// The pages' glyph origins and the files they name, where the build
    /// keeps them (the viewer's `origins`).
    pub fn origins(&self) -> Option<crate::view::Origins> {
        let b = self
            .build
            .as_ref()
            .filter(|_| self.job.origins || self.job.synctex)?;
        Some(crate::view::Origins {
            pages: b.all_page_origins(),
            files: b.origin_files(),
        })
    }

    /// The pages whose origins changed since the last ask and the edits
    /// since then, `(file, at, delta)` in [`crate::view::Origins`]'
    /// file indices ([`phitex_tex::Build::origins_changed`]: a viewer
    /// that keeps the pages' origins shifts its offsets by the deltas and
    /// asks again for those pages only); none where the build keeps no
    /// origins.
    pub fn origins_changed(&self) -> Option<crate::view::OriginsChanged> {
        let b = self
            .build
            .as_ref()
            .filter(|_| self.job.origins || self.job.synctex)?;
        let (pages, edits) = b.origins_changed();
        Some(crate::view::OriginsChanged { pages, edits })
    }

    /// The main text with the editor's buffer for it, if there is one.
    fn text_now(&self) -> Arc<str> {
        let name = plain_name(&self.job.path.to_string_lossy());
        self.buffers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .rev()
            .find(|(n, _)| *n == plain_name(&self.job.file) || *n == name)
            .and_then(|(_, t)| t.as_deref().map(Arc::from))
            .unwrap_or_else(|| self.text.clone())
    }

    /// XeTeX's PDF as xelatex makes it (`xdvipdfmx -q -E -o JOB.pdf`,
    /// dated as the job), its files the host's for the document's
    /// directory; none for `-no-pdf`, or a host without dvipdfmx's files.
    fn xdv_pdf(&self) -> Option<phitex_tex::XdvPdf> {
        if self.job.engine.output != tex::Output::Xdv {
            return None;
        }
        let dir = self
            .job
            .path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Some(phitex_tex::XdvPdf {
            name: format!("{}.pdf", self.job.jobname),
            date: self.job.pdf_date,
            files: self.host.xdv_files(&dir)?,
            live: self.sink.updates(),
        })
    }

    /// The format's text (its ini file read, as `-ini` reads it: none
    /// for `-ini` jobs), and its evaluated state from the cache with the
    /// key it was stored with.
    #[allow(clippy::type_complexity, reason = "one caller")]
    fn prelude(&self) -> Result<(Option<String>, Option<([u8; 32], Vec<u8>)>), String> {
        if self.job.ini {
            return Ok((None, None));
        }
        let ini = self.job.engine.ini;
        if !self.host.has_format_file(ini) {
            return Err(format!("can't find {ini} (kpathsea)"));
        }
        let text = format!("\\input {ini}\n");
        let cached = self
            .store
            .as_ref()
            .and_then(|c| c.format(&self.format_name(&text)));
        Ok((Some(text), cached))
    }

    /// The format's name in the cache: its engine, the engine's identity
    /// (another engine's format is never read; a binary that changed only
    /// elsewhere reads it), its text's hash.
    fn format_name(&self, text: &str) -> String {
        format!(
            "{}-{:016x}-{:016x}",
            self.job.engine.name,
            phitex_tex::engine_id(),
            tex::Fnv::new().add(text.as_bytes()).0
        )
    }

    /// The key sets the last session stored (the store's `wkeys`, the
    /// document's read-back key sets, [`Build::wkeys`]: the next
    /// session's round-one guesses): one file per line, its name then
    /// its keys, separated by NUL.
    fn load_wkeys(&self) -> Vec<(String, Vec<String>)> {
        let Some(c) = &self.store else {
            return Vec::new();
        };
        c.get(Key::Doc("wkeys"))
            .map(|b| wkeys_of(&b))
            .unwrap_or_default()
    }

    fn store_wkeys(&self, b: &Build) {
        let Some(c) = &self.store else {
            return;
        };
        let out = wkeys_bytes(&b.wkeys());
        // (the bytes last stored here: the cache's file not read again
        // to see that nothing changed)
        let mut last = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        if last.0.as_deref() == Some(out.as_bytes()) {
            return;
        }
        if c.get(Key::Doc("wkeys")).as_deref() != Some(out.as_bytes()) {
            c.put(Key::Doc("wkeys"), out.as_bytes());
        }
        last.0 = Some(out.into_bytes());
    }

    /// The files the job writes as an earlier run left them in the
    /// output directory ([`Options::written_init`], a fixpoint's): those
    /// the last session's job wrote, or the last run's log names, that
    /// are there. Their bytes as TeX
    /// reads them, one character each, as [`Session::write`] wrote them.
    ///
    /// latexmk's first run: the job's `.aux` file, written as latexmk
    /// writes it where there is none ([`LATEXMK_AUX`]), else as it is.
    fn written_init(&self) -> Vec<(String, String)> {
        if !self.job.fixpoint {
            return Vec::new();
        }
        let read = |n: &str| -> Option<String> {
            let bytes = self.sink.get(n)?;
            Some(bytes.iter().map(|&c| char::from(c)).collect())
        };
        // (the store's `wnames`: the names of the files the document's job
        // wrote in the last session, [`Build::files`])
        let mut out: Vec<(String, String)> = match &self.store {
            Some(c) => c
                .get(Key::Doc("wnames"))
                .map(|b| {
                    wnames_of(&b)
                        .into_iter()
                        .filter(|n| out_name_ok(n))
                        .filter_map(|n| {
                            let t = read(&n)?;
                            Some((n, t))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        // (and those the last run's log says it opened for writing, §1374's
        // `\openoutN = `name'.`: a run by another TeX, or by a session
        // whose cache is gone)
        let log = read(&format!("{}.log", self.job.jobname)).unwrap_or_default();
        for l in log.lines() {
            let Some(n) = l
                .strip_prefix("\\openout")
                .and_then(|r| r.split_once(" = `"))
                .and_then(|(_, r)| r.strip_suffix("'."))
            else {
                continue;
            };
            if out_name_ok(n)
                && !out.iter().any(|(m, _)| m == n)
                && let Some(t) = read(n)
            {
                out.push((n.to_owned(), t));
            }
        }
        let aux = format!("{}.aux", self.job.jobname);
        if !out.iter().any(|(n, _)| *n == aux) {
            let text = read(&aux).unwrap_or_else(|| LATEXMK_AUX.to_owned());
            out.push((aux, text));
        }
        out
    }

    fn store_wnames(&self, b: &Build) {
        let Some(c) = &self.store else {
            return;
        };
        let names: Vec<String> = b.files().into_iter().map(|(n, _)| n).collect();
        let out = wnames_bytes(&names);
        let mut last = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        if last.2.as_deref() == Some(out.as_bytes()) {
            return;
        }
        if c.get(Key::Doc("wnames")).as_deref() != Some(out.as_bytes()) {
            c.put(Key::Doc("wnames"), out.as_bytes());
        }
        last.2 = Some(out.into_bytes());
    }

    /// The format the package loads' records are kept beside
    /// ([`Build::export_pkgrec`]: the store's `pkgrec` of it, shared by
    /// every document built with it); none for `-ini` jobs.
    fn pkgrec_key(&self) -> Option<String> {
        if self.job.ini {
            return None;
        }
        let text = format!("\\input {}\n", self.job.engine.ini);
        Some(self.format_name(&text))
    }

    /// Whether the document nodes' records are kept
    /// ([`Build::export_noderec`]: the store's `noderec`, the document's
    /// own).
    fn noderec_kept(&self) -> bool {
        !self.job.ini && self.job.node_records
    }

    /// The package loads' and the document nodes' records stored, when
    /// a build changed them since last stored: once the session is idle
    /// (`phitex build` after its build, `watch` when no event came for a
    /// while and when it quits), not after every build.
    pub fn flush_records(&mut self) {
        if !self.records_dirty {
            return;
        }
        self.records_dirty = false;
        if let Some(b) = &self.build {
            self.store_pkgrec(b);
            self.store_noderec(b);
        }
    }

    fn store_pkgrec(&self, b: &Build) {
        let (Some(c), Some(n)) = (&self.store, self.pkgrec_key()) else {
            return;
        };
        let recs = b.export_pkgrec();
        let mut last = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        if last.1.as_deref() == Some(recs.as_slice()) {
            return;
        }
        if !recs.is_empty() && c.get(Key::Format(&n, "pkgrec")).as_deref() != Some(recs.as_slice())
        {
            c.put(Key::Format(&n, "pkgrec"), &recs);
        }
        last.1 = Some(recs);
    }

    fn store_noderec(&self, b: &Build) {
        let Some(c) = self.store.as_ref().filter(|_| self.noderec_kept()) else {
            return;
        };
        let recs = b.export_noderec();
        let mut last = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        if last.3.as_deref() == Some(recs.as_slice()) {
            return;
        }
        if !recs.is_empty() && c.get(Key::Doc("noderec")).as_deref() != Some(recs.as_slice()) {
            c.put(Key::Doc("noderec"), &recs);
        }
        last.3 = Some(recs);
    }

    fn store_prelude(&self, b: &Build, prelude: Option<&str>, had: Option<[u8; 32]>) {
        let (Some(c), Some(text)) = (&self.store, prelude) else {
            return;
        };
        let key = b.prelude_key();
        // (none: the format's evaluation did not finish cleanly, and is not
        // kept: its errors would come back at every run)
        if had != Some(key) {
            let t = Instant::now();
            let Some(bytes) = b.export_prelude() else {
                return;
            };
            let exported = t.elapsed();
            c.put_format(&self.format_name(text), &key, &bytes);
            if phitex_tex::tex_time() {
                eprintln!(
                    "time: format stored: export {exported:?}, write {:?} ({} bytes)",
                    t.elapsed().saturating_sub(exported),
                    bytes.len()
                );
            }
        }
    }

    /// The names the builds asked the finder for (`inputs`)
    /// that stand for one of `changed` (as found, or as named): by the
    /// names and the files kept, no lookup; a changed name equal to
    /// none is one of the files of its file name on disk.
    fn reads_of(&self, changed: &[String]) -> Vec<String> {
        if changed.is_empty() {
            return Vec::new();
        }
        let found: Vec<Option<String>> = self
            .inputs
            .iter()
            .map(|(_, _, p)| p.as_ref().map(|p| plain_name(&p.to_string_lossy())))
            .collect();
        let mut hit = vec![false; self.inputs.len()];
        for c in changed {
            let mut any = false;
            for (i, (n, ..)) in self.inputs.iter().enumerate() {
                let n = plain_name(n);
                if *c == n || *c == format!("{n}.tex") || found[i].as_ref() == Some(c) {
                    hit[i] = true;
                    any = true;
                }
            }
            if any {
                continue;
            }
            let base = Path::new(c).file_name();
            for (i, f) in found.iter().enumerate() {
                if let Some(f) = f
                    && Path::new(f).file_name() == base
                    && self.host.same_file(f, c)
                {
                    hit[i] = true;
                }
            }
        }
        self.inputs
            .iter()
            .zip(hit)
            .filter(|(_, h)| *h)
            .map(|((n, ..), _)| n.clone())
            .collect()
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

    /// The format's files: the host's format finder, else the job's.
    fn format_finder(&self) -> Finder {
        if let Some(f) = self.host.format_finder() {
            return f;
        }
        let host = self.host.clone();
        Arc::new(move |name: &str| {
            let p = host.locate(name)?;
            let bytes = host.read(&p)?;
            Some((p.to_string_lossy().into_owned(), bytes))
        })
    }

    /// Files by the host, as TeX names them: an editor's buffer for one
    /// first.
    fn finder(&self) -> Finder {
        let host = self.host.clone();
        let buffers = self.buffers.clone();
        Arc::new(move |name: &str| {
            let p = host.locate(name)?;
            let shown = p.to_string_lossy().into_owned();
            let key = plain_name(&shown);
            let buffered = buffers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .rev()
                .find(|(n, _)| *n == key || host.same_file(n, &key))
                .and_then(|(_, t)| t.clone());
            let bytes = match buffered {
                Some(t) => tex::bytes(&t),
                None => host.read(&p)?,
            };
            Some((shown, bytes))
        })
    }

    /// Writes what the build made: `JOB.pdf` or `JOB.dvi` (with `main_too`),
    /// the files TeX wrote, `JOB.log`, `JOB.fls` (`-recorder`), through
    /// the host's sink.
    pub fn write(&self, r: &Report, main_too: bool) -> Result<Written, String> {
        let sink = &self.sink;
        sink.begin()?;
        phitex_tex::timeline::lap("write: the output directory");
        let mut files = Vec::new();
        let mut main = None;
        // (XeTeX's -no-pdf: the XDV file as `.xdv`)
        let xdv = |e: &'static str| {
            if e == "dvi" && self.job.engine.output == tex::Output::XdvFile {
                "xdv"
            } else {
                e
            }
        };
        let ext = r
            .main_output()
            .map_or(self.job.engine.output.ext(), |(e, _)| xdv(e));
        if let (Some(l), true, true) = (&r.out.layout, sink.updates(), main_too) {
            let p = sink.put_update(&format!("{}.pdf", self.job.jobname), l)?;
            phitex_tex::timeline::lap("write: the main output (an update section)");
            files.push((p.clone(), tex::ANY_BYTES));
            main = Some(p);
        }
        if let Some((e, _)) = r.main_output().filter(|_| main_too) {
            let shared = if r.out.pdf.is_empty() {
                &r.out.dvi
            } else {
                &r.out.pdf
            };
            let h = tex::fingerprint(shared);
            let p = sink.put_main(&format!("{}.{}", self.job.jobname, xdv(e)), shared, h)?;
            phitex_tex::timeline::lap("write: the main output");
            files.push((p.clone(), h));
            main = Some(p);
        }
        let mut put = |name: &str, bytes: &[u8]| -> Result<PathBuf, String> {
            let h = tex::fingerprint(bytes);
            let p = sink.put(name, bytes, h)?;
            files.push((p.clone(), h));
            Ok(p)
        };
        for (n, t) in &r.out.writes {
            // (TeX Live's `openout_any = p`: never outside the output
            // directory, never a dot file; a document can't overwrite
            // ~/.bashrc with `\openout`)
            if !out_name_ok(n) {
                eprintln!("phitex: not writing to {n} (openout_any = p)");
                continue;
            }
            put(n, &tex::bytes(t))?;
        }
        for (n, b) in &r.out.shell_files {
            if !out_name_ok(n) {
                continue;
            }
            match b {
                Some(b) => {
                    sink.dir_of(n)?;
                    put(n, b)?;
                }
                None => sink.remove(n),
            }
        }
        if !self.watching {
            put(
                &format!("{}.log", self.job.jobname),
                &tex::bytes(&r.out.log),
            )?;
        }
        if self.job.recorder {
            let fls = self.recorder(r, &format!("{}.{ext}", self.job.jobname));
            put(&format!("{}.fls", self.job.jobname), fls.as_bytes())?;
        }
        // (`-synctex`: from the pages' glyph origins, the files named from
        // the main file's directory)
        if self.job.synctex
            && let Some(b) = &self.build
        {
            let root = match self.job.path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let root = self.host.canonical(&root);
            let gz = self
                .synctex
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write(b, &root);
            if let Some(gz) = gz {
                put(&format!("{}.synctex.gz", self.job.jobname), &gz)?;
            }
            phitex_tex::timeline::lap("write: the SyncTeX file");
        }
        phitex_tex::timeline::lap("write: the log, the files TeX wrote");
        Ok(Written { main, files })
    }

    /// The build's transcript as it is now, the job's `.log` (laid out
    /// here when a watch's builds skipped it: the viewer's "View log",
    /// `--report`).
    #[must_use]
    pub fn transcript(&self) -> String {
        self.build
            .as_ref()
            .map(|b| b.output_shared().1)
            .unwrap_or_default()
    }

    /// `JOB.log` written from the build as it is now (a watch's `--log`,
    /// when idle).
    ///
    /// # Errors
    /// The sink's.
    pub fn write_log(&self) -> Result<(), String> {
        let Some(b) = &self.build else {
            return Ok(());
        };
        let (_, log) = b.output_shared();
        self.sink
            .write(&format!("{}.log", self.job.jobname), &tex::bytes(&log))
    }

    /// `JOB.fls` as `-recorder` writes it, of what is known: the main
    /// file, the outputs.
    fn recorder(&self, r: &Report, out_name: &str) -> String {
        let mut s = String::new();
        let cwd = self.sink.here();
        s.push_str(&format!("PWD {}\n", cwd.display()));
        s.push_str(&format!("INPUT {}\n", self.job.path.display()));
        if self.build.is_some() {
            for (_, found, p) in &self.inputs {
                if let Some(p) = p.as_ref().filter(|_| *found) {
                    s.push_str(&format!("INPUT {}\n", p.display()));
                }
            }
        }
        let out = |n: &str| self.sink.path(n).display().to_string();
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
pub fn waiting_notes(_job: &Job) -> Vec<String> {
    Vec::new()
}

/// The `.aux` file latexmk writes before its first run where the job has
/// none (latexmk.pl, `set_trivial_aux_fdb`): what LaTeX writes for a
/// one-page document without cross-references, so that such a document
/// needs one run, not two.
const LATEXMK_AUX: &str = "\\relax \n\\gdef \\@abspage@last{1}\n";

/// Whether a file the document writes may be written under `name`
/// (kpathsea's `kpse_out_name_ok` in its paranoid mode, TeX Live's
/// default `openout_any = p`): a relative name, no `..` component, no
/// component that is a dot file.
fn out_name_ok(name: &str) -> bool {
    let p = Path::new(name);
    !p.is_absolute()
        && !name.starts_with('~')
        && p.components().all(|c| match c {
            std::path::Component::Normal(s) => !s.to_string_lossy().starts_with('.'),
            std::path::Component::CurDir => true,
            _ => false,
        })
}

/// A file's name without a leading `./`.
fn plain_name(n: &str) -> String {
    n.strip_prefix("./").unwrap_or(n).to_owned()
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

/// Whether two texts are equal (the same shared text at once).
fn same_text(a: &Arc<str>, b: &Arc<str>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

/// The line ends in `b`, eight bytes at a time.
fn newlines(b: &[u8]) -> usize {
    let (words, rest) = b.as_chunks::<8>();
    let mut c = 0usize;
    for w in words {
        let v = u64::from_ne_bytes(*w) ^ 0x0a0a_0a0a_0a0a_0a0a;
        let x = (v & 0x7f7f_7f7f_7f7f_7f7f).wrapping_add(0x7f7f_7f7f_7f7f_7f7f);
        c += (!(x | v | 0x7f7f_7f7f_7f7f_7f7f)).count_ones() as usize;
    }
    c + rest.iter().map(|&b| usize::from(b == b'\n')).sum::<usize>()
}

/// The first line (from 1) where `b` differs from `a` (none: equal):
/// the first differing byte found a block at a time, the line ends
/// before it counted a word at a time.
fn first_difference_line(a: &str, b: &str) -> Option<usize> {
    const B: usize = 4096;
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let m = x.len().min(y.len());
    let mut at = 0;
    while at + B <= m && x[at..at + B] == y[at..at + B] {
        at += B;
    }
    while at < m && x[at] == y[at] {
        at += 1;
    }
    if at == m && x.len() == y.len() {
        return None;
    }
    let (words, rest) = y[..at].as_chunks::<8>();
    let mut n = 0usize;
    for w in words {
        let v = u64::from_ne_bytes(*w) ^ 0x0a0a_0a0a_0a0a_0a0a;
        let t = (v & 0x7f7f_7f7f_7f7f_7f7f).wrapping_add(0x7f7f_7f7f_7f7f_7f7f);
        n += (!(t | v | 0x7f7f_7f7f_7f7f_7f7f)).count_ones() as usize;
    }
    n += rest.iter().map(|&c| usize::from(c == b'\n')).sum::<usize>();
    Some(n + 1)
}

/// The store's `wkeys` ([`Session::load_wkeys`]): one file per line, its
/// name then its keys, separated by NUL. (Its encoding is
/// [`crate::host::RECORDS_VERSION`]'s.)
fn wkeys_bytes(sets: &[(String, Vec<String>)]) -> String {
    let mut out = String::new();
    for (n, ks) in sets {
        out.push_str(n);
        for k in ks {
            out.push('\0');
            out.push_str(k);
        }
        out.push('\n');
    }
    out
}

fn wkeys_of(b: &[u8]) -> Vec<(String, Vec<String>)> {
    String::from_utf8_lossy(b)
        .lines()
        .filter_map(|l| {
            let mut xs = l.split('\0').map(str::to_owned);
            Some((xs.next()?, xs.collect()))
        })
        .collect()
}

/// The store's `wnames` ([`Session::written_init`]): a name a line.
/// (Its encoding is [`crate::host::RECORDS_VERSION`]'s.)
fn wnames_bytes(names: &[String]) -> String {
    let mut out = String::new();
    for n in names {
        out.push_str(n);
        out.push('\n');
    }
    out
}

fn wnames_of(b: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(b)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document's records' encodings as [`crate::host::RECORDS_VERSION`]
    /// 1 has them: a change to them fails here, and is made with that
    /// version bumped (and these bytes with it), so that a record of the
    /// old encoding is not read as the new.
    #[test]
    fn records_encodings_are_locked() {
        assert_eq!(crate::host::RECORDS_VERSION, 1);
        let sets = vec![
            ("doc.aux".to_owned(), vec!["r@a".to_owned(), "b".to_owned()]),
            ("doc.toc".to_owned(), Vec::new()),
        ];
        let w = wkeys_bytes(&sets);
        assert_eq!(w, "doc.aux\0r@a\0b\ndoc.toc\n");
        assert_eq!(wkeys_of(w.as_bytes()), sets);
        let names = vec!["doc.aux".to_owned(), "sub/c.aux".to_owned()];
        let n = wnames_bytes(&names);
        assert_eq!(n, "doc.aux\nsub/c.aux\n");
        assert_eq!(wnames_of(n.as_bytes()), names);
    }

    #[test]
    fn edit_lines() {
        assert_eq!(first_difference_line("a\nb\nc", "a\nb\nc"), None);
        assert_eq!(first_difference_line("a\nb\nc", "a\nx\nc"), Some(2));
        assert_eq!(first_difference_line("a\n", "a\nb\n"), Some(2));
        assert_eq!(first_difference_line("x", "y"), Some(1));
    }

    #[test]
    fn comment() {
        assert_eq!(dvi_comment(0), " TeX output 1970.01.01:0000");
        assert_eq!(dvi_comment(1_791_810_000), " TeX output 2026.10.12:1300");
    }
}

#[cfg(test)]
mod out_name_tests {
    use super::out_name_ok;

    #[test]
    fn written_files_stay_in_the_output_directory() {
        for ok in ["paper.aux", "./paper.toc", "sub/chap.aux"] {
            assert!(out_name_ok(ok), "{ok}");
        }
        for bad in [
            "/home/u/.bashrc",
            "../x.tex",
            "a/../../x",
            ".bashrc",
            "sub/.hidden",
            "~/x",
        ] {
            assert!(!out_name_ok(bad), "{bad}");
        }
    }
}
