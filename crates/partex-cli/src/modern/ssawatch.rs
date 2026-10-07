//! `phitex watch --ssa` (experimental; DESIGN 2.5 and 4.8): the watch on
//! the dynamic-SSA runtime (`partex_core::ssa`), the engine the Overleaf
//! extension runs, instead of the machine runtime. The terminal, the
//! viewer, the keys and the outputs are the machine watch's; only the
//! runtime differs.
//!
//! - *The cold build* is the job run once on the runtime
//!   (`ssa::run_applying`), then the trips that settle the job's own files
//!   (`.aux`, `.toc`, BibTeX and makeindex as the build's own nodes), one
//!   at a time (`ssa::settle` bounded to one more trip), each linked,
//!   written and shown as a machine watch's pass is. There is no store:
//!   every `watch --ssa` begins with one.
//! - *An edit* runs one trip (`ssa::rebuild_trips`, one trip at most: one
//!   `pdflatex` run, as an editor's keystroke), whose pages are written
//!   and shown at once; then the trips that follow settle while nothing
//!   newer is saved, as the extension's idle settle does.
//! - *A newer save* stops the trip under way at the next step boundary
//!   (`SsaTracker::cancel`, a poll of the inputs' stamps at most every
//!   `PARTEX_WATCH_POLL_MS`): its work is kept (`ssa::pending`) and the
//!   next rebuild goes on with it and the new edit. Between trips a save is
//!   looked for before the next begins.
//! - *A rebuild it cannot make* (`unsupported`) or one past its deadline
//!   (the last cold build's time; `PARTEX_SSA_DEADLINE_MS`, `0`: none) is
//!   built again cold.
//! - The files are written by the SSA link (`SsaLinker`), each renamed
//!   into place whole: the PDF on disk is always a complete one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use partex_core::Tex;
use partex_core::diag::{Diagnostic, Severity};
use partex_core::ssa::{self, SsaTracker};

use super::{
    BUSY, INTERRUPTED, Input, Options, Target, Viewer, answer, ensure_format, load_estimate,
    machine_finish, main_output, next_inputs, poll_period, save_estimate,
};
use crate::events::{Phase, Progress};
use crate::native::{NativeHost, Stamp};
use crate::render::{Rebuild, Renderer};
use crate::term;

type SsaTex = Tex<NativeHost, SsaTracker>;

/// A newer save was found while a trip ran (the cancel's answer).
static SAVED: AtomicBool = AtomicBool::new(false);
/// The inputs a running trip's cancel looks at, with the stamps they had.
static WATCHED: Mutex<Vec<(Vec<u8>, Option<Stamp>)>> = Mutex::new(Vec::new());
/// When the cancel last looked (the trips' clock, ns), and how often it
/// may.
static LOOKED: AtomicU64 = AtomicU64::new(0);
static POLL_NS: AtomicU64 = AtomicU64::new(50_000_000);

/// `SsaTracker::cancel`, asked at each step boundary of a trip: whether
/// an input was saved since the trip began (by its stamp, looked at most
/// every `POLL_NS`).
fn saved() -> bool {
    if SAVED.load(Relaxed) {
        return true;
    }
    let now = crate::clock_ns();
    if now.saturating_sub(LOOKED.load(Relaxed)) < POLL_NS.load(Relaxed) {
        return false;
    }
    LOOKED.store(now, Relaxed);
    let any = WATCHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|(p, s)| crate::native::stamp(p) != *s);
    if any {
        SAVED.store(true, Relaxed);
    }
    any
}

/// A file the build read, as the watch last found it.
struct Seen {
    stamp: Option<Stamp>,
    /// Its bytes, for a project file (a relative path that is no output):
    /// an edit is named by the first line it changed.
    bytes: Option<Arc<[u8]>>,
}

/// The files the build read, looked at for edits.
#[derive(Default)]
struct Inputs {
    files: BTreeMap<Vec<u8>, Seen>,
    /// Looks so far: the TeX tree's files and the files not found are
    /// looked at every tenth.
    tick: u32,
    /// The files not found that appeared, each named once.
    appeared: std::collections::BTreeSet<Vec<u8>>,
}

/// A path as the terminal shows it (`./ch1.tex` is `ch1.tex`).
fn shown(p: &[u8]) -> String {
    String::from_utf8_lossy(p.strip_prefix(b"./").unwrap_or(p)).into_owned()
}

/// Whether `p` is the project's (relative), not the TeX tree's.
fn project(p: &[u8]) -> bool {
    !p.starts_with(b"/")
}

impl Inputs {
    /// After a link: each file the build read as it read it (its stamp
    /// just before, its bytes: a save after that is an edit, even one made
    /// while the build ran), and the job's own files as the link left them
    /// (not edits). A file the host kept nothing of (its times under 2 s
    /// old when read: a project just copied) is taken as it is now.
    fn refresh(&mut self, host: &NativeHost) {
        for p in host.read_paths() {
            let ours = project(&p) && !host.is_output(&p);
            let known = self.files.contains_key(&p);
            let seen = self.files.entry(p.clone()).or_insert(Seen {
                stamp: None,
                bytes: None,
            });
            if let Some((stamp, bytes)) = host.read_as(&p) {
                seen.stamp = Some(stamp);
                seen.bytes = ours.then_some(bytes);
            } else if !known {
                seen.stamp = crate::native::stamp(&p);
                seen.bytes = ours
                    .then(|| std::fs::read(crate::native::path(&p)).ok())
                    .flatten()
                    .map(Arc::from);
            }
            if host.as_written(&p) {
                seen.stamp = crate::native::stamp(&p);
            }
        }
    }

    /// The files edited since they were last looked at, each with the
    /// first line it changed (`ch05.tex:31`); every file (`all`: the TeX
    /// tree's, and the files not found that are there now) or the
    /// project's, and the others every tenth look. A file saved as it
    /// was, or as the link wrote it, is no edit.
    fn changed(&mut self, host: &NativeHost, all: bool) -> Vec<String> {
        self.tick = self.tick.wrapping_add(1);
        let all = all || self.tick.is_multiple_of(10);
        let mut out = Vec::new();
        for (p, seen) in &mut self.files {
            if !all && !project(p) {
                continue;
            }
            let now = crate::native::stamp(p);
            if now == seen.stamp {
                continue;
            }
            seen.stamp = now;
            if host.as_written(p) {
                continue;
            }
            let new: Option<Arc<[u8]>> = (project(p) && !host.is_output(p))
                .then(|| std::fs::read(crate::native::path(p)).ok())
                .flatten()
                .map(Arc::from);
            let line = match (&seen.bytes, &new) {
                (Some(old), Some(new)) if old[..] == new[..] => continue,
                (Some(old), Some(new)) => {
                    let same = old
                        .iter()
                        .zip(new.iter())
                        .take_while(|(a, b)| a == b)
                        .count();
                    Some(old[..same].split(|&c| c == b'\n').count())
                }
                _ => None,
            };
            seen.bytes = new;
            out.push(match line {
                Some(l) => format!("{}:{l}", shown(p)),
                None => shown(p),
            });
        }
        if all
            && let Some(p) = host.appeared()
            && self.appeared.insert(p.clone())
        {
            out.push(shown(&p));
        }
        out
    }

    /// The project's files the build read that it does not write, with
    /// their stamps: what a running trip's cancel looks at.
    fn watched(&self, host: &NativeHost) -> Vec<(Vec<u8>, Option<Stamp>)> {
        self.files
            .iter()
            .filter(|(p, _)| project(p) && !host.is_output(p))
            .map(|(p, s)| (p.clone(), s.stamp))
            .collect()
    }

    /// Whether one of those was saved since.
    fn saved_since(&self, host: &NativeHost) -> bool {
        self.watched(host)
            .iter()
            .any(|(p, s)| crate::native::stamp(p) != *s)
    }
}

/// How a build's run of trips ended.
enum End {
    /// It settled (or reached the bound, these names still changing) after
    /// `passes` trips, and its files are written.
    Done {
        passes: usize,
        unsettled: Vec<String>,
    },
    /// A newer save stopped it: its work waits for the next rebuild.
    Superseded,
    /// It cannot go on as a rebuild (why): a cold build instead.
    Cold(&'static str),
    /// The edit changed nothing the build read.
    Nothing,
}

/// The watch's build on the SSA runtime.
#[allow(clippy::struct_excessive_bools, reason = "its switches")]
struct Ssa {
    params: partex_core::Params,
    command_line: Vec<u8>,
    formats: Option<std::path::PathBuf>,
    tex: Option<SsaTex>,
    linker: crate::SsaLinker,
    between: crate::Between,
    native: Option<ssa::NativeTools>,
    /// Trips of a build at most (`PARTEX_SSA_TRIPS`, 5: latexmk's bound).
    bound: usize,
    /// Hits applied in a cold build (`PARTEX_SSA_APPLY=1`) and in rebuilds
    /// (unless `PARTEX_SSA_APPLY=0`).
    apply_cold: bool,
    apply: bool,
    /// How long the last cold build took: a rebuild's trip past it stops,
    /// and the build goes cold.
    cold_ms: f64,
    inputs: Inputs,
    /// The last link's terminal text and diagnostics, the last trip's
    /// history.
    term: Vec<u8>,
    diagnostics: Vec<Diagnostic>,
    history: i32,
    /// What the rebuild under way serves: each file edited and the first
    /// line it changed.
    last_changes: Vec<String>,
    /// The build under way: when its first trip's files were written, and
    /// the steps and commands its trips ran.
    first: Option<Instant>,
    steps: usize,
    commands: u64,
    /// The trips whose files the last link wrote.
    passes: usize,
    /// The commands of the last cold build's first trip: the job's, which
    /// a rebuild's pass line counts its own against.
    job_commands: u64,
    debug: bool,
    /// Glyph origins recorded, for the live viewer's double-click and
    /// `phitex sync` (only with a viewer: they cost).
    origins: bool,
    /// The hash of the `SyncTeX` file last written.
    synctex: Option<u128>,
}

/// Write the build's `SyncTeX` file (`Tex::synctex_file`) renamed into
/// place whole, unless its bytes are `last`'s (their hash), and remove the
/// other kind's (or both, with no file); the file is the link's output.
/// The new hash.
fn write_synctex(
    tex: &mut SsaTex,
    linker: &mut crate::SsaLinker,
    last: Option<u128>,
) -> Option<u128> {
    let f = tex.synctex_file()?;
    let host = tex.host_mut();
    let at = |n: &[u8]| host.in_output_dir(n).unwrap_or_else(|| n.to_vec());
    let (name, other) = (at(&f.name), at(&f.other));
    for n in [Some(&other), f.bytes.is_none().then_some(&name)]
        .into_iter()
        .flatten()
    {
        let p = crate::native::path(n);
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
        linker.outputs.remove(n);
    }
    let bytes = f.bytes?;
    let h = partex_core::StableHasher::of(&bytes[..]);
    if last != Some(h) || !crate::native::path(&name).exists() {
        if crate::write_atomic(&crate::native::path(&name), &bytes).is_err() {
            return None;
        }
        host.note_written(&name);
    }
    linker.outputs.insert(name, bytes.len());
    Some(h)
}

/// Tell the renderer and the viewer.
fn tell(ren: &Renderer, viewer: &Viewer, p: &Progress) {
    ren.progress(p);
    viewer.progress(p);
}

/// A trip's report, for a pass line: `commands` of `total` in `ns`.
fn pass_report(history: i32, commands: u64, total: u64, ns: u64) -> crate::session::Report {
    crate::session::Report {
        history,
        commands,
        total_commands: total.max(commands),
        elapsed: Duration::from_nanos(ns),
        cut_at: None,
        why: Vec::new(),
    }
}

impl Ssa {
    /// The watch of the job the engine command line (`crate::args`) sets
    /// up, its formats in `formats`; nothing built yet.
    fn new(formats: Option<std::path::PathBuf>) -> Self {
        let job = crate::setup();
        let (_, apply) = crate::rebuild_switches();
        Ssa {
            params: job.params,
            command_line: job.command_line.into_bytes(),
            formats,
            tex: None,
            linker: crate::SsaLinker::default(),
            between: crate::Between::default(),
            native: crate::ssa_native(),
            bound: crate::ssa_trips(),
            apply_cold: std::env::var("PARTEX_SSA_APPLY").is_ok_and(|v| v == "1"),
            apply,
            cold_ms: 0.0,
            inputs: Inputs::default(),
            term: Vec::new(),
            diagnostics: Vec::new(),
            history: 0,
            last_changes: Vec::new(),
            first: None,
            steps: 0,
            commands: 0,
            passes: 0,
            job_commands: 0,
            debug: std::env::var_os("PARTEX_WATCH_DEBUG").is_some(),
            origins: false,
            synctex: None,
        }
    }

    /// A host for a cold build: the job's, capturing its terminal and
    /// diagnostics, running its commands as nodes.
    fn host(&self) -> NativeHost {
        let mut host = crate::setup().host;
        host.formats.clone_from(&self.formats);
        host.notes = true;
        host.commands = Some(crate::native::Commands::default());
        host.capture = Some(crate::native::Captured::default());
        crate::make_output_dir(&host);
        host
    }

    /// `f` with the build's trips set up (`max` trips at most).
    fn trips<R>(
        &mut self,
        max: usize,
        f: impl FnOnce(&mut SsaTex, &mut ssa::Trips<'_, NativeHost>) -> R,
    ) -> R {
        let mut tools = crate::ssa_tools(&mut self.between);
        let mut t = ssa::Trips {
            max,
            tools: &mut tools,
            native: self.native.as_ref(),
            clock: Some(crate::clock_ns),
        };
        let tex = self.tex.as_mut().expect("an engine");
        f(tex, &mut t)
    }

    /// Before a trip: its cancel (a newer save) and its deadline.
    fn arm(&self) {
        let Some(tex) = &self.tex else { return };
        SAVED.store(false, Relaxed);
        LOOKED.store(crate::clock_ns(), Relaxed);
        *WATCHED.lock().unwrap_or_else(PoisonError::into_inner) = self.inputs.watched(tex.host());
        let t = tex.tracker();
        t.cancel.set(Some(saved));
        let ms = std::env::var("PARTEX_SSA_DEADLINE_MS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(self.cold_ms.max(1000.0));
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "milliseconds to nanoseconds"
        )]
        let ns = (ms * 1e6) as u64;
        let clock: fn() -> u64 = crate::clock_ns;
        t.deadline
            .set((ms > 0.0).then(|| (clock, crate::clock_ns().saturating_add(ns))));
    }

    /// After a trip: no cancel, no deadline.
    fn disarm(&self) {
        if let Some(tex) = &self.tex {
            tex.tracker().cancel.set(None);
            tex.tracker().deadline.set(None);
        }
    }

    /// The tools' lines of a trip's report, told.
    fn tools(ren: &Renderer, viewer: &Viewer, r: &ssa::RebuildReport) {
        for l in &r.tools {
            tell(ren, viewer, &Progress::Tool(l));
        }
    }

    /// Link the build's files and write them; its terminal text and
    /// diagnostics kept, a page note for each page (the summary counts
    /// them).
    fn link(&mut self, ren: &Renderer, viewer: &Viewer) {
        tell(ren, viewer, &Progress::Phase(Phase::Linking));
        let tex = self.tex.as_mut().expect("an engine");
        if let Some(c) = &mut tex.host_mut().capture {
            c.term.clear();
            c.diagnostics.clear();
        }
        let lr = self.linker.link(tex);
        self.linker.write_produced(tex);
        self.synctex = write_synctex(tex, &mut self.linker, self.synctex);
        if self.debug {
            eprintln!("phitex: ssa watch: link {:.1} ms: {}", lr.link_ms, lr.how);
        }
        let host = tex.host_mut();
        if let Some(c) = &mut host.capture {
            self.term = std::mem::take(&mut c.term);
            self.diagnostics = std::mem::take(&mut c.diagnostics);
        }
        self.diagnostics
            .extend(self.linker.pages.iter().map(|count0| Diagnostic {
                severity: Severity::Note,
                code: crate::events::PAGE,
                message: count0.to_string().into_bytes(),
                help: Vec::new(),
                frames: Vec::new(),
                suggestions: Vec::new(),
                boxed: None,
            }));
        self.inputs.refresh(host);
        self.first.get_or_insert_with(Instant::now);
        if self.origins {
            let tex = self.tex.as_mut().expect("an engine");
            let pdf = self
                .linker
                .outputs
                .keys()
                .find(|n| n.ends_with(b".pdf"))
                .and_then(|n| std::fs::read(crate::native::path(n)).ok());
            if let Some(pdf) = pdf
                && let Ok(root) = std::env::current_dir()
            {
                let files = tex.origin_files();
                let pages = tex.origin_pages();
                viewer.set_origins(crate::view::Origins::of(&root, &files, pages, pdf.into()));
            }
        }
    }

    /// The files the links wrote that are there, with their lengths.
    fn outputs(&self) -> Vec<(Vec<u8>, usize)> {
        self.linker
            .outputs
            .keys()
            .filter_map(|n| {
                let m = std::fs::metadata(crate::native::path(n)).ok()?;
                Some((n.clone(), usize::try_from(m.len()).unwrap_or(usize::MAX)))
            })
            .collect()
    }

    /// The cold build: the job from its start on a new engine, then the
    /// trips that settle it.
    fn cold(&mut self, ren: &Renderer, viewer: &Viewer) -> End {
        // (the old engine's records go first: they are the memory)
        drop(self.tex.take());
        self.linker = crate::SsaLinker {
            atomic: true,
            ..crate::SsaLinker::default()
        };
        let host = self.host();
        tell(ren, viewer, &Progress::PassStart(1));
        tell(ren, viewer, &Progress::Phase(Phase::Cold));
        let t = Instant::now();
        let mut tex = Tex::new(host, crate::ssa_tracker(), self.params.clone());
        tex.set_window(crate::ssa_window());
        // (the steps' `SyncTeX` events, rendered after each link; glyph
        // origins for the viewer)
        crate::origins::setup_synctex(&mut tex);
        if self.origins {
            tex.set_origins(true);
        }
        self.synctex = None;
        // (a cold build runs to its first trip's end: a save meanwhile is
        // taken by the trips after it)
        tex.tracker().cancel.set(None);
        tex.tracker().deadline.set(None);
        let r = ssa::run_applying(&mut tex, &self.command_line, false, 0, self.apply_cold);
        self.history = r.history;
        self.commands += r.commands;
        self.steps += tex.tracker().rec.borrow().rt.fold.order.len();
        self.tex = Some(tex);
        let ns = u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX);
        tell(
            ren,
            viewer,
            &Progress::Pass(1, Some(&pass_report(r.history, r.commands, r.commands, ns))),
        );
        self.job_commands = r.commands;
        let end = self.settle(ren, viewer, 1);
        if matches!(end, End::Done { .. }) {
            self.cold_ms = t.elapsed().as_secs_f64() * 1e3;
            // (the heap the build freed given back, and what the first
            // rebuild would decode decoded now)
            crate::heap::trim();
            if let Some(tex) = &self.tex {
                crate::ready_for_rebuilds(tex, true);
            }
        }
        end
    }

    /// An edit: one trip, what the edits reach (with the work a stopped
    /// trip left), then the trips that settle it.
    fn edit(&mut self, ren: &Renderer, viewer: &Viewer) -> End {
        if self.tex.is_none() {
            return End::Cold("no engine");
        }
        tell(ren, viewer, &Progress::PassStart(1));
        self.arm();
        let apply = self.apply;
        let rr = self.trips(1, |tex, t| ssa::rebuild_trips(tex, false, apply, t));
        self.disarm();
        Self::tools(ren, viewer, &rr);
        if let Some(why) = rr.unsupported {
            return End::Cold(why);
        }
        if let Some(why) = rr.stopped {
            return if rr.cancelled {
                End::Superseded
            } else {
                End::Cold(why)
            };
        }
        if rr.edits == 0 && !rr.resumed && rr.steps_run == 0 && rr.tools.is_empty() {
            return End::Nothing;
        }
        self.history = rr.history;
        self.steps += rr.steps_run;
        self.commands += rr.commands;
        let ns = rr.trip_ns.first().copied().unwrap_or(0);
        tell(
            ren,
            viewer,
            &Progress::Pass(
                1,
                Some(&pass_report(rr.history, rr.commands, self.job_commands, ns)),
            ),
        );
        self.settle(ren, viewer, 1)
    }

    /// After trip `k`: its files written and shown, then the trips that
    /// follow, one at a time, until the job's own files settle (or the
    /// bound), each stopped by a newer save.
    fn settle(&mut self, ren: &Renderer, viewer: &Viewer, mut k: usize) -> End {
        let total = self.job_commands;
        let mut unsettled = Vec::new();
        let mut settled = false;
        loop {
            self.link(ren, viewer);
            self.passes = k;
            if settled || k >= self.bound {
                return End::Done {
                    passes: k,
                    unsettled,
                };
            }
            viewer.pass_written(&self.outputs(), k);
            if let Some(tex) = &self.tex
                && self.inputs.saved_since(tex.host())
            {
                return End::Superseded;
            }
            tell(ren, viewer, &Progress::PassStart(k + 1));
            self.arm();
            let apply = self.apply;
            let s = self.trips(2, |tex, t| ssa::settle(tex, false, apply, t, 0, 0));
            self.disarm();
            Self::tools(ren, viewer, &s);
            if let Some(why) = s.unsupported {
                return End::Cold(why);
            }
            if let Some(why) = s.stopped {
                return if s.cancelled {
                    End::Superseded
                } else {
                    End::Cold(why)
                };
            }
            if s.trips <= 1 {
                // (no load read other than what the last trip stored: the
                // files written are the fixpoint's)
                return End::Done {
                    passes: k,
                    unsettled,
                };
            }
            self.history = s.history;
            let (steps, commands) = (
                s.trip_steps.get(1).copied().unwrap_or(0),
                s.trip_commands.get(1).copied().unwrap_or(0),
            );
            self.steps += steps;
            self.commands += commands;
            let ns = s.trip_ns.get(1).copied().unwrap_or(0);
            k += 1;
            tell(
                ren,
                viewer,
                &Progress::Pass(k, Some(&pass_report(s.history, commands, total, ns))),
            );
            settled = s.settled;
            unsettled = s
                .unsettled
                .iter()
                .map(|n| shown(n.rsplit(|&c| c == b'/').next().unwrap_or(n)))
                .collect();
        }
    }

    /// Build (`cold`) or rebuild until the job settles, a newer save
    /// superseding the trip under way and a rebuild it cannot make built
    /// cold: what it did, `None` if the edit changed nothing.
    fn serve(
        &mut self,
        ren: &Renderer,
        viewer: &Viewer,
        mut cold: bool,
    ) -> Option<crate::machinehost::Outcome> {
        let t = Instant::now();
        let first_cold = cold;
        self.first = None;
        self.steps = 0;
        self.commands = 0;
        let mut reports = Vec::new();
        let (passes, unsettled) = loop {
            let end = if cold {
                self.cold(ren, viewer)
            } else {
                self.edit(ren, viewer)
            };
            match end {
                End::Done { passes, unsettled } => break (passes, unsettled),
                // (a cold build whose settling a save that changed nothing
                // stopped: its files are written)
                End::Nothing if first_cold => break (self.passes, Vec::new()),
                End::Nothing => return None,
                End::Superseded => {
                    let host = self.tex.as_ref().map(Tex::host);
                    if let Some(host) = host {
                        let changed = self.inputs.changed(host, false);
                        if !changed.is_empty() {
                            self.last_changes = changed;
                        }
                    }
                    tell(ren, viewer, &Progress::Superseded(&self.last_changes));
                    if self.debug {
                        eprintln!(
                            "phitex: ssa watch: superseded by {}",
                            self.last_changes.join(", ")
                        );
                    }
                    cold = false;
                }
                End::Cold(why) => {
                    reports.push(format!("phitex: ssa: rebuild stopped ({why}): built cold"));
                    cold = true;
                }
            }
        };
        let ms = |i: Instant| i.duration_since(t).as_secs_f64() * 1e3;
        let first = self.first.map_or(0.0, ms);
        let all = t.elapsed().as_secs_f64() * 1e3;
        reports.push(if first_cold {
            format!(
                "phitex: ssa: built in {all:.0} ms: first pages {first:.0} ms, {passes} passes, {} steps, {} commands",
                self.steps, self.commands
            )
        } else {
            format!(
                "phitex: ssa: rebuilt in {all:.0} ms: first pages {first:.0} ms, {passes} passes, {} steps run, {} commands",
                self.steps, self.commands
            )
        });
        if self.debug {
            for r in &reports {
                eprintln!("{r}");
            }
        }
        Some(crate::machinehost::Outcome {
            term: self.term.clone(),
            history: self.history,
            diagnostics: self.diagnostics.clone(),
            outputs: self.outputs(),
            reports,
            unsettled,
        })
    }
}

/// `phitex watch --ssa`: build cold, then rebuild on the SSA runtime
/// whenever an input is saved. `ren` and `rx` are the watch's, reading
/// keys already.
pub(super) fn watch(
    opts: &Options,
    target: &Target,
    ren: &Renderer,
    rx: &mpsc::Receiver<Input>,
) -> ! {
    BUSY.store(true, Relaxed);
    ren.status("Compiling", &format!("{} ({})", target.file, target.engine));
    ren.status(
        "SSA",
        "experimental: the dynamic-SSA runtime, the Overleaf extension's; \
         no store, so each watch --ssa starts with a cold build",
    );
    let formats = ensure_format(&target.engine, Some(ren));
    crate::set_args(target.engine_args("nonstopmode"));
    let poll = poll_period(50);
    POLL_NS.store(u64::try_from(poll.as_nanos()).unwrap_or(u64::MAX), Relaxed);
    let mut w = Ssa::new(formats);
    let mut viewer = Viewer::start(opts, ren);
    // (the viewer's glyphs placed by the build's own origins)
    w.origins = viewer.live.is_some();
    viewer.glyph_origins = w.origins;
    ren.set_estimate(load_estimate());
    ren.start();
    let t = Instant::now();
    let out = w.serve(ren, &viewer, true).expect("a cold build serves");
    let mut history = out.history;
    let mut outputs = out.outputs.clone();
    machine_finish(target, ren, &out, None, true);
    viewer.built(&outputs, history, t, &out.diagnostics);
    save_estimate(ren);
    if opts.open {
        viewer.open(ren, target, &outputs);
    }
    BUSY.store(false, Relaxed);
    ren.watching(&target.file, main_output(&outputs).as_deref());
    loop {
        let inputs = next_inputs(rx, poll);
        if inputs.contains(&Input::Quit) {
            ren.close();
            term::exit(i32::from(history > 1));
        }
        let asked = inputs.contains(&Input::Rebuild);
        for &i in &inputs {
            answer(i, ren, target, &outputs, &mut viewer);
        }
        if !asked && !inputs.is_empty() {
            continue;
        }
        let changed = match &w.tex {
            Some(tex) => w.inputs.changed(tex.host(), asked),
            None => Vec::new(),
        };
        if changed.is_empty() {
            if asked {
                ren.event("nothing changed");
            }
            continue;
        }
        BUSY.store(true, Relaxed);
        ren.start_rebuild();
        viewer.building();
        let t = Instant::now();
        w.last_changes = changed;
        let out = w.serve(ren, &viewer, false);
        BUSY.store(false, Relaxed);
        let Some(out) = out else {
            ren.idle();
            viewer.built(&outputs, history, t, &w.diagnostics);
            if asked {
                ren.event("nothing changed");
            }
            ren.watching(&target.file, main_output(&outputs).as_deref());
            continue;
        };
        history = out.history;
        outputs.clone_from(&out.outputs);
        let changed = w.last_changes.clone();
        machine_finish(target, ren, &out, Some(Rebuild { changed }), true);
        viewer.built(&outputs, history, t, &out.diagnostics);
        ren.watching(&target.file, main_output(&outputs).as_deref());
        if INTERRUPTED.load(Relaxed) {
            ren.close();
            term::exit(i32::from(history > 1));
        }
    }
}
