//! A document's session, kept for the process's life: one
//! [`phitex_tex::Build`], the document's only state. The first build is
//! an edit from nothing, every later one an edit of the main text as it
//! is now; the session writes what the build made.
//!
//! Every format runs from its ini file as the prelude (`tex.ini`,
//! `etex.ini`, `pdfetex.ini`, `latex.ini`, `pdflatex.ini`), its evaluated
//! state kept in the cache; `-ini` jobs have none. DVI or PDF out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phitex_tex::{Build, Finder, Options};

use crate::cache::Cache;
use crate::tex::{self, Engine};

/// What a session does now, for a terminal's spinner (`ui.rs`), and
/// the phases it went through, each with its time (the watch's summary).
#[derive(Default)]
pub struct Status {
    now: Mutex<Phases>,
    live: Mutex<Option<Live>>,
}

/// The engine at work: its progress (what each worker runs, the jobs done
/// of those known) and the main file's line starts, for the places.
#[derive(Clone)]
pub struct Live {
    pub progress: Arc<phitex_tex::Progress>,
    pub file: String,
    pub lines: Arc<Vec<usize>>,
}

#[derive(Default)]
struct Phases {
    /// The phase running, and since when (none: between phases).
    current: Option<(String, Instant)>,
    done: Vec<(String, Duration)>,
}

impl Status {
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
    /// `--exact-pdf`: the PDF's objects numbered as pdfTeX numbers them
    /// (false: the relaxed mode, the default).
    pub exact_pdf: bool,
    /// `--spec`: when the engine runs steps ahead of their inputs (none:
    /// its default).
    pub spec: Option<String>,
    /// `-synctex`.
    pub synctex: bool,
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
    /// Nodes the build has after it (those not run were kept).
    pub live: usize,
    /// The engine's threads, and its work summed over them (none: no
    /// clock, wasm): busy / wall is how parallel the run was.
    pub threads: usize,
    pub busy: Duration,
    pub engine_wall: Duration,
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
    /// The engine's waits by kind, the most-waited nodes (id, waits), and
    /// the steps held back at a gate.
    pub waits_by_kind: Vec<(&'static str, u64)>,
    pub top_waited: Vec<(u64, u64)>,
    pub gated: u64,
    /// The steps run ahead of their inputs, those confirmed, those run
    /// again, the raises, the runs out of fuel; the most steps ready at
    /// once and the mean per scheduler step.
    pub speculations: u64,
    pub confirmations: u64,
    pub mispredictions: u64,
    pub raises: u64,
    pub fuel_outs: u64,
    pub max_ready: usize,
    pub ready_mean: f64,
    /// The parser's hints: nodes bounded, nodes that may define anything,
    /// hints whose assumptions failed, definitions outside a bound, the
    /// stops and pending waits.
    pub hints: [u64; 6],
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
    /// The PDF (pdfTeX), else empty.
    pub pdf: Vec<u8>,
    /// The DVI (TeX, e-TeX), else empty.
    pub dvi: Vec<u8>,
    pub pages: usize,
    /// The pages (from 1) that differ from the last build's.
    pub pages_changed: Vec<usize>,
    /// Each page's hash, first first.
    pub page_hashes: Vec<u64>,
    pub errors: Vec<String>,
    /// The terminal's and the log's text.
    pub log: String,
    /// The files TeX wrote.
    pub writes: Vec<(String, String)>,
}

/// What a build made.
pub struct Report {
    pub out: Output,
    /// The first line (from 1) of the main text that differs from the
    /// last build's (none: cold, or the main text unchanged).
    pub edit_line: Option<usize>,
    pub runs: Vec<RunInfo>,
    /// The written files still changing when the Build stopped.
    pub unsettled: Vec<String>,
    pub notes: Vec<String>,
    pub wall: Duration,
    /// The engine's own account of why it stopped (its state dumped: for
    /// `-v` and a bug report, not the error shown).
    pub internal: Option<String>,
    /// The errors and warnings at their places, by file then byte.
    pub diagnostics: Vec<phitex_tex::Diagnostic>,
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
    /// The main text the last build built.
    built_text: Option<String>,
    /// The last build's pages' hashes.
    page_hashes: Vec<u64>,
    /// kpathsea as the job's program: the build's finder.
    tree: Arc<tex::Tree>,
    /// kpathsea as the format's build sees it: without the current
    /// directory (fmtutil builds formats in a scratch directory).
    format_tree: Arc<tex::Tree>,
    builds: usize,
    /// The hashes of the files the last write wrote (an unchanged file is
    /// not written again).
    written: Mutex<BTreeMap<PathBuf, u64>>,
    /// What the session does now, for a spinner.
    status: Arc<Status>,
    /// Unsaved editor buffers, by file name (`None`: as on disk again),
    /// which the finder reads first.
    buffers: Arc<Mutex<Vec<(String, Option<String>)>>>,
    /// The files other than the main one changed since the last build.
    changed: Vec<String>,
    /// The formats' evaluated states (none: no cache).
    cache: Option<Cache>,
    /// The texts sent to the build while it runs, by version.
    absorb: Absorb,
    /// When the change came that the last build took last while it ran.
    absorbed_at: Option<Instant>,
}

/// The main text sent to a running build from another thread: the build
/// takes the latest between its steps ([`phitex_tex::EditSender`]). Each
/// text sent is kept with when it came, until a result is for it or a
/// later one.
#[derive(Clone, Default)]
pub struct Absorb {
    tx: phitex_tex::EditSender,
    sent: Arc<Mutex<BTreeMap<u64, (String, Instant)>>>,
}

impl Absorb {
    /// Sends the main text (TeX's characters): its version.
    pub fn send(&self, text: &str, came: Instant) -> u64 {
        let mut m = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        let v = self.tx.send(text);
        m.insert(v, (text.to_owned(), came));
        v
    }

    /// Version `v`'s text and when it came, the earlier ones forgotten.
    fn take(&self, v: u64) -> Option<(String, Instant)> {
        let mut m = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        let got = m.get(&v).cloned();
        m.retain(|&k, _| k > v);
        got
    }
}

/// The job's clock as engine `e` reads it, in seconds whose UTC fields are
/// the job's date and time: `SOURCE_DATE_EPOCH` (TeX Live's engines but
/// Knuth's `tex`, which ignores it), else the local time now.
pub fn pdf_date(e: &Engine) -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .filter(|_| e.name != "tex")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            let now = jiff::Timestamp::now();
            let off = jiff::tz::TimeZone::system().to_offset(now);
            now.as_second() + i64::from(off.seconds())
        })
}

impl Session {
    /// A session for `job`: the main file read.
    pub fn new(
        job: Job,
        status: Arc<Status>,
        note: &mut dyn FnMut(&str),
    ) -> Result<Session, String> {
        let text = std::fs::read(&job.path).map_err(|e| format!("{}: {e}", job.path.display()))?;
        let tree = Arc::new(tex::Tree::open(&job.engine));
        let format_tree = Arc::new(tex::Tree::open_format(&job.engine));
        let cache = Cache::open(job.cache_limit).ok();
        if let Some(c) = &cache
            && let Some(w) = c.unkept()
        {
            note(&w);
        }
        Ok(Session {
            job,
            text: tex::chars(&text),
            build: None,
            built_text: None,
            page_hashes: Vec::new(),
            tree,
            format_tree,
            builds: 0,
            written: Mutex::new(BTreeMap::new()),
            status,
            buffers: Arc::default(),
            changed: Vec::new(),
            cache,
            absorb: Absorb::default(),
            absorbed_at: None,
        })
    }

    /// The main file's text from now on (TeX's characters).
    pub fn set_text(&mut self, text: String) {
        self.text = text;
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
            && self.built_text.as_deref() == Some(self.text_now().as_str())
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
            unsettled: Vec::new(),
            notes: Vec::new(),
            wall: t0.elapsed(),
            internal,
            diagnostics: Vec::new(),
        };
        let mut text = self.text_now();
        self.absorbed_at = None;
        let mut edit_line = self
            .built_text
            .as_deref()
            .and_then(|old| first_difference_line(old, &text));
        let changed = std::mem::take(&mut self.changed);
        let reread = self
            .build
            .as_ref()
            .map(|b| self.reads_of(b, &changed))
            .unwrap_or_default();
        // (an evaluation error panics, naming the nodes: the build is
        // reported failed and dropped, the next one cold)
        // (the main file's line starts: a worker's place as file:line)
        let lines: Arc<Vec<usize>> = Arc::new(
            std::iter::once(0)
                .chain(text.char_indices().filter(|c| c.1 == '\n').map(|c| c.0 + 1))
                .collect(),
        );
        let file = plain_name(&self.job.path.to_string_lossy());
        let live = |progress| Live {
            progress,
            file: file.clone(),
            lines: lines.clone(),
        };
        let ran = crate::ui::catch(|| -> Result<(), String> {
            if let Some(b) = &mut self.build {
                self.status.set_live(Some(live(b.progress())));
                self.status.set("engine");
                for name in &reread {
                    b.edit_file(name, None);
                }
                // (sent as a version of its own: what another thread sends
                // while it runs is taken between the engine's steps)
                self.absorb.send(&text, Instant::now());
                b.absorb_pending();
            } else {
                self.status.set("format");
                let (prelude, cached) = self.prelude()?;
                // (a format not cached is built first, in this build: slow
                // until the kernel is fast, so a longer time limit for it;
                // none would leave a stuck evaluation running for ever)
                let first = prelude.is_some() && cached.is_none();
                self.status.set(if first {
                    format!("building the {} format (first time)", self.job.engine.name)
                } else {
                    "engine".to_owned()
                });
                let workers = self
                    .job
                    .threads
                    .or_else(|| std::env::var("PHITEX_THREADS").ok()?.parse().ok())
                    .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from));
                let progress = Arc::new(phitex_tex::Progress::new(workers));
                self.status.set_live(Some(live(progress.clone())));
                let opts = Options {
                    name: self.printed_name(),
                    jobname: self.job.jobname.clone(),
                    comment: dvi_comment(self.job.pdf_date),
                    finder: self.finder(),
                    prelude_finder: Some(self.format_finder()),
                    epoch: Some(self.job.pdf_date),
                    fixpoint: self.job.fixpoint,
                    prelude_jobname: self.job.engine.name.to_owned(),
                    prelude_bytes: cached.as_ref().map(|(_, b)| b.clone()),
                    threads: self.job.threads,
                    progress: Some(progress),
                    // (--exact-pdf: pdfTeX's numbering; else the relaxed mode,
                    // pages written apart, the same pages after renumbering:
                    // crates/output/pdf/RELAXED.md)
                    exact_objects: self.job.exact_pdf,
                    speculation: match self.job.spec.as_deref() {
                        Some("off") => phitex_tex::Speculation::Off,
                        Some("all") => phitex_tex::Speculation::All,
                        Some("predicted") => phitex_tex::Speculation::Predicted,
                        _ => Options::default().speculation,
                    },
                    wkeys: self.load_wkeys(),
                    pagefx: self
                        .wkeys_path()
                        .and_then(|p| self.cache.as_ref()?.get(&p.with_file_name("pagefx")))
                        .unwrap_or_default(),
                    // (the document's own last values, else what the first
                    // document built with this format learned)
                    pgst: self
                        .wkeys_path()
                        .and_then(|p| self.cache.as_ref()?.get(&p.with_file_name("pgst")))
                        .or_else(|| {
                            let c = self.cache.as_ref()?;
                            let name = self.format_name(prelude.as_deref()?);
                            c.get(&c.format_file(&name, "pgst1"))
                        })
                        .unwrap_or_default(),
                    // (the package loads' records: machine-wide, beside the
                    // format, from every document built with it)
                    pkgrec: self
                        .pkgrec_path()
                        .and_then(|p| self.cache.as_ref()?.get(&p))
                        .unwrap_or_default(),
                    ..Options::default()
                };
                let opts = Options {
                    edits: Some(self.absorb.tx.clone()),
                    time_limit: if first {
                        Some(Duration::from_mins(30))
                    } else {
                        opts.time_limit
                    },
                    ..opts
                };
                let b = Build::with(opts, prelude.as_deref(), &text);
                self.store_prelude(&b, prelude.as_deref(), cached.map(|(k, _)| k));
                self.build = Some(b);
            }
            Ok(())
        });
        self.status.set_live(None);
        match ran {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return fail(e, None),
            Err(e) => {
                self.build = None;
                self.built_text = None;
                return fail(engine_stop(&e), Some(e));
            }
        }
        // (the result is for the latest text the build took)
        if let Some(b) = &self.build
            && let Some((t, came)) = self.absorb.take(b.version())
            && t != text
        {
            edit_line = self
                .built_text
                .as_deref()
                .and_then(|old| first_difference_line(old, &t));
            text = t;
            self.absorbed_at = Some(came);
            self.text.clone_from(&text);
        }
        if let Some(b) = &self.build {
            self.store_wkeys(b);
            self.store_pkgrec(b);
            // (`PHITEX_CHAIN_DUMP=1`: the critical chain in full, and its
            // waits, for measurements)
            if std::env::var_os("PHITEX_CHAIN_DUMP").is_some() {
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
            if std::env::var_os("PHITEX_PKG_STATS").is_some() {
                let s = &b.stats;
                eprintln!(
                    "PKG loads {} claimed {} recorded {} optimistic {} runs {} at-once {} span {:.3} on-critical {} refuted {} undeclared {} critical-path {:?} threads {} engine-wall {:.3} speculations {} confirmations {} mispredictions {} predicted-confirmed {} predicted-refuted {}",
                    s.pkg_loads,
                    s.pkg_claimed,
                    s.pkg_recorded,
                    s.pkg_optimistic,
                    s.pkg_runs,
                    s.pkg_max_concurrent,
                    s.pkg_wall.as_secs_f64(),
                    s.pkg_on_critical,
                    s.declarations_refuted,
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
        // (a long session: what the old texts' runs memoized and nothing
        // reaches any more goes, so the tables stay bounded)
        let mut gc = (Duration::ZERO, 0);
        if std::env::var_os("PHITEX_NO_GC").is_none()
            && let Some(b) = &mut self.build
        {
            let t = Instant::now();
            gc.1 = b.collect_garbage();
            gc.0 = t.elapsed();
        }
        self.status.set("output");
        let b = self.build.as_ref().expect("built above");
        let (bytes, log) = b.output();
        let pdf = b.is_pdf();
        let s = &b.stats;
        let diagnostics = b.diagnostics();
        // (and the evaluation's own errors — no fixpoint, a stop — which
        // the log does not have: errors at no place, without context)
        // (the format's own errors first, with their places: a format
        // that stopped on them is not the oracle's, whatever follows)
        let errors = b
            .prelude_diagnostics()
            .iter()
            .map(|d| format!("! {} ({}:{})", d.message, d.file, d.line))
            .chain(
                log.lines()
                    .filter(|l| l.starts_with("! "))
                    .map(str::to_owned),
            )
            .chain(
                diagnostics
                    .iter()
                    .filter(|d| {
                        d.severity == phitex_tex::Severity::Error
                            && d.range == (0, 0)
                            && d.context.is_empty()
                    })
                    .map(|d| d.message.lines().next().unwrap_or("").to_owned()),
            )
            .collect();
        // (a PDF file's pages: not known until the engine reports them;
        // the viewer counts the PDF's itself and draws each page again)
        let hashes = b.pages();
        let pages = hashes.len();
        let writes = b
            .files()
            .into_iter()
            .map(|(n, t)| (plain_name(&n), t))
            .collect();
        let pages_changed: Vec<usize> = (0..hashes.len())
            .filter(|&k| self.page_hashes.get(k) != Some(&hashes[k]))
            .map(|k| k + 1)
            .collect();
        self.page_hashes.clone_from(&hashes);
        self.built_text = Some(text);
        let wall = t0.elapsed();
        Report {
            out: Output {
                pdf: if pdf { bytes.clone() } else { Vec::new() },
                dvi: if pdf { Vec::new() } else { bytes },
                writes,
                pages,
                pages_changed,
                page_hashes: hashes,
                errors,
                log,
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
                live: s.live,
                threads: s.threads,
                busy: s.busy,
                engine_wall: s.wall,
                critical_path: s.critical_path,
                peak_running: s.peak_running,
                batches: s.batches,
                batches_inline: s.batches_inline,
                max_batch: s.jobs_max_batch,
                waits_by_kind: s.waits_by_kind.clone(),
                top_waited: s.top_waited.clone(),
                gated: s.gated,
                speculations: s.speculations,
                confirmations: s.confirmations,
                mispredictions: s.mispredictions,
                raises: s.raises,
                fuel_outs: s.fuel_outs,
                max_ready: s.max_ready,
                ready_mean: s.ready_mean(),
                hints: [
                    s.nodes_bounded,
                    s.nodes_any,
                    s.regime_violations,
                    s.undeclared_definitions,
                    s.stops,
                    s.pending_waits,
                ],
            }],
            internal: None,
            diagnostics,
            unsettled: Vec::new(),
            notes: std::iter::once(format!(
                "{} nodes run of {} live, {} runs, {} cutoffs, {} loop rounds",
                s.nodes_run, s.live, s.runs, s.cutoffs, s.loop_rounds
            ))
            .chain(std::iter::once({
                let (nodes, units, values) = b.sizes();
                // (the process's resident set, Linux only: VmRSS in kB)
                let rss = std::fs::read_to_string("/proc/self/status")
                    .ok()
                    .and_then(|t| {
                        t.lines()
                            .find_map(|l| l.strip_prefix("VmRSS:"))
                            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
                    })
                    .unwrap_or(0);
                format!(
                    "engine tables: {nodes} nodes, {units} units, {values} values, {rss} kB resident, {} us collecting ({} nodes freed)",
                    gc.0.as_micros(),
                    gc.1
                )
            }))
            .chain(
                b.pdf_exact_reason()
                    .map(|r| format!("PDF objects numbered as pdfTeX's: {r}")),
            )
            .collect(),
            wall,
        }
    }

    /// File `name`'s bytes as a diagnostic names it: the main text as the
    /// build read it, another file from the disk (none: not readable).
    pub fn source(&self, name: &str) -> Option<Vec<u8>> {
        let n = plain_name(name);
        if n == plain_name(&self.job.path.to_string_lossy()) {
            return self.built_text.as_deref().map(tex::bytes);
        }
        std::fs::read(&n).ok()
    }

    /// The main text with the editor's buffer for it, if there is one.
    fn text_now(&self) -> String {
        let name = plain_name(&self.job.path.to_string_lossy());
        self.buffers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .rev()
            .find(|(n, _)| *n == plain_name(&self.job.file) || *n == name)
            .and_then(|(_, t)| t.clone())
            .unwrap_or_else(|| self.text.clone())
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
        self.format_tree
            .input(ini)
            .ok_or_else(|| format!("can't find {ini} (kpathsea)"))?;
        let text = format!("\\input {ini}\n");
        let cached = self
            .cache
            .as_ref()
            .and_then(|c| c.format(&self.format_name(&text)));
        Ok((Some(text), cached))
    }

    /// The format's name in the cache: its engine, this binary's sources'
    /// hash (another build's format is never read), its text's hash.
    fn format_name(&self, text: &str) -> String {
        format!(
            "{}-{}-{:016x}",
            self.job.engine.name,
            env!("PHITEX_BUILD_ID"),
            tex::Fnv::new().add(text.as_bytes()).0
        )
    }

    /// The prelude's evaluated state stored, unless the cache has it
    /// under the same key.
    /// The cache's file of the document's read-back key sets
    /// ([`Build::wkeys`]): the next session's round-one guesses (beside
    /// it, `pagefx`: [`Build::export_pagefx`]).
    fn wkeys_path(&self) -> Option<PathBuf> {
        Some(self.cache.as_ref()?.doc_dir(&self.job.path).join("wkeys"))
    }

    /// The key sets the last session stored: one file per line, its name
    /// then its keys, separated by NUL.
    fn load_wkeys(&self) -> Vec<(String, Vec<String>)> {
        let (Some(c), Some(p)) = (&self.cache, self.wkeys_path()) else {
            return Vec::new();
        };
        let Some(b) = c.get(&p) else {
            return Vec::new();
        };
        String::from_utf8_lossy(&b)
            .lines()
            .filter_map(|l| {
                let mut xs = l.split('\0').map(str::to_owned);
                Some((xs.next()?, xs.collect()))
            })
            .collect()
    }

    fn store_wkeys(&self, b: &Build) {
        let (Some(c), Some(p)) = (&self.cache, self.wkeys_path()) else {
            return;
        };
        let mut out = String::new();
        for (n, ks) in b.wkeys() {
            out.push_str(&n);
            for k in ks {
                out.push('\0');
                out.push_str(&k);
            }
            out.push('\n');
        }
        if c.get(&p).as_deref() != Some(out.as_bytes()) {
            let _ = c.put(&p, out.as_bytes(), &[]);
        }
        let fx = b.export_pagefx();
        let p = p.with_file_name("pagefx");
        if c.get(&p).as_deref() != Some(fx.as_slice()) {
            let _ = c.put(&p, &fx, &[]);
        }
        let st = b.export_pgst();
        let p = p.with_file_name("pgst");
        if c.get(&p).as_deref() != Some(st.as_slice()) {
            let _ = c.put(&p, &st, &[]);
        }
    }

    /// The cache's file of the package loads' records
    /// ([`Build::export_pkgrec`]): beside the format, shared by every
    /// document built with it.
    fn pkgrec_path(&self) -> Option<PathBuf> {
        if self.job.ini {
            return None;
        }
        let c = self.cache.as_ref()?;
        let text = format!("\\input {}\n", self.job.engine.ini);
        Some(c.format_file(&self.format_name(&text), "pkgrec"))
    }

    fn store_pkgrec(&self, b: &Build) {
        let (Some(c), Some(p)) = (&self.cache, self.pkgrec_path()) else {
            return;
        };
        let recs = b.export_pkgrec();
        if !recs.is_empty() && c.get(&p).as_deref() != Some(recs.as_slice()) {
            let _ = c.put(&p, &recs, &[]);
        }
    }

    fn store_prelude(&self, b: &Build, prelude: Option<&str>, had: Option<[u8; 32]>) {
        let (Some(c), Some(text)) = (&self.cache, prelude) else {
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
            let _ = c.put_format(&self.format_name(text), &key, &bytes);
            if std::env::var_os("PHITEX_TEX_TIME").is_some() {
                eprintln!(
                    "time: format stored: export {exported:?}, write {:?} ({} bytes)",
                    t.elapsed().saturating_sub(exported),
                    bytes.len()
                );
            }
        }
        // the page chain's values of the first document built with this
        // format: a later document's round-one guesses
        let p = c.format_file(&self.format_name(text), "pgst1");
        if c.get(&p).is_none() {
            let st = b.export_pgst();
            if !st.is_empty() {
                let _ = c.put(&p, &st, &[]);
            }
        }
    }

    /// The names the build asked the finder for that stand for one of
    /// `changed` (as found, or as named).
    fn reads_of(&self, b: &Build, changed: &[String]) -> Vec<String> {
        if changed.is_empty() {
            return Vec::new();
        }
        b.reads()
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| {
                let found = self.tree.input(n).map(|p| plain_name(&p.to_string_lossy()));
                changed.iter().any(|c| {
                    *c == plain_name(n)
                        || *c == format!("{}.tex", plain_name(n))
                        || found.as_ref() == Some(c)
                        || found.as_deref().is_some_and(|f| same_file(f, c))
                })
            })
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

    /// The format's files, by kpathsea outside the document's directory.
    fn format_finder(&self) -> Finder {
        let tree = self.format_tree.clone();
        Arc::new(move |name: &str| {
            let p = tree.input(name)?;
            let bytes = std::fs::read(&p).ok()?;
            Some((p.to_string_lossy().into_owned(), bytes))
        })
    }

    /// Files by kpathsea, as TeX names them: an editor's buffer for one
    /// first.
    fn finder(&self) -> Finder {
        let tree = self.tree.clone();
        let buffers = self.buffers.clone();
        Arc::new(move |name: &str| {
            let p = tree.input(name)?;
            let shown = p.to_string_lossy().into_owned();
            let key = plain_name(&shown);
            let buffered = buffers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .rev()
                .find(|(n, _)| *n == key || same_file(n, &key))
                .and_then(|(_, t)| t.clone());
            let bytes = match buffered {
                Some(t) => tex::bytes(&t),
                None => std::fs::read(&p).ok()?,
            };
            Some((shown, bytes))
        })
    }

    /// Writes what the build made: `JOB.pdf` or `JOB.dvi` (with `main_too`),
    /// the files TeX wrote, `JOB.log`, `JOB.fls` (`-recorder`).
    pub fn write(&self, r: &Report, main_too: bool) -> Result<Written, String> {
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
        if let Some((e, b)) = r.main_output().filter(|_| main_too) {
            main = Some(put(&format!("{}.{e}", self.job.jobname), b)?);
        }
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
        if let Some(b) = &self.build {
            for (n, found) in b.reads() {
                if let Some(p) = self.tree.input(&n).filter(|_| found) {
                    s.push_str(&format!("INPUT {}\n", p.display()));
                }
            }
        }
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
    if job.synctex {
        v.push("no SyncTeX yet (-synctex)".to_owned());
    }
    if job.shell_escape {
        v.push("-shell-escape: the native engine runs no \\write18 yet".to_owned());
    }
    v
}

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

/// Whether two names are one file on disk.
fn same_file(a: &str, b: &str) -> bool {
    match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
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

/// The first line (from 1) where `b` differs from `a` (none: equal).
#[allow(clippy::naive_bytecount, reason = "once a build, on one text")]
fn first_difference_line(a: &str, b: &str) -> Option<usize> {
    let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y);
    let at = match at {
        Some(i) => i,
        None if a.len() == b.len() => return None,
        None => a.len().min(b.len()),
    };
    Some(
        b.as_bytes()[..at.min(b.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count()
            + 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
