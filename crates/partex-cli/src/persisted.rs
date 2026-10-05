//! Machine-mode builds kept across processes (DESIGN.md §7.9,
//! "Persisted machine builds"): a watch or build saves its recorded
//! build to the content-addressed store (`store.rs`) after its outputs
//! are written, on a thread of its own; the next process with the same
//! identity loads it, looks at the inputs on disk and rebuilds only what
//! their changes reach. `PARTEX_STORE=0` turns it off.
//!
//! The identity is everything a build depends on besides its input
//! files: this executable (path, length, time), the directory, the engine
//! command line and parameters, the environment TeX and partex read, and
//! the day unless `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE=1` fix the
//! dates (as a resident or `-converge` session keeps its clock). The
//! input files are not in it: a loaded build compares every file it read
//! with the disk, and what differs is an edit. Loading checks the
//! starting and final states against their saved digests; anything
//! malformed, missing or different is a cold build, never a wrong one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use partex_core::machine::{LazyBody, SnapBody};
use std::thread::JoinHandle;
use std::time::Instant;

use partex_core::Params;
use partex_core::machine::{Part, Parts, SavedChunk};
use partex_core::persist::{Known, Loaded, Loader, Persist, Saver};
use partex_incr::Trace;

use super::{Build, Machine, MachineHost, NativeHost, Watch};
use crate::store;

/// Shared values of at least this many bytes are blobs of their own.
const MIN_BLOB: usize = 64;

/// What a root holds first (its layout's version).
const ROOT_TAG: &[u8] = b"partex machine build/13";

/// The store a watch saves to, and the save running.
pub struct Keeper {
    dir: PathBuf,
    key: u128,
    /// What the store keeps for this job, as a save needs it.
    kept: Arc<Mutex<Kept>>,
    saving: Option<JoinHandle<()>>,
    /// A rebuild ran since the last save began.
    pub dirty: bool,
    /// The generation of the build last saved: one saved
    /// again unchanged is not.
    saved: Option<u32>,
}

/// What the store keeps for a job, as the next save needs it: the blobs
/// in its packs (not written again), the runs of regions saved (referred
/// to again while unchanged) and the values of the last save or load by
/// address (not encoded again).
#[derive(Default)]
struct Kept {
    stored: store::Stored,
    chunks: Vec<SavedChunk>,
    known: Known,
}

/// The store's directory from `partex.toml` (`store = "dir"`).
static CONFIGURED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Take the store's directory from `partex.toml`.
pub fn configure(dir: PathBuf) {
    let _ = CONFIGURED.set(dir);
}

/// This build of partex: its path, length and time.
fn exe_id() -> Option<(Vec<u8>, u64, u128)> {
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let time = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((exe.into_os_string().into_encoded_bytes(), meta.len(), time))
}

/// The job's identity (see the module documentation).
fn identity(params: &Params, command_line: &[u8]) -> Option<u128> {
    // (switches that do not change what is built are not identity; nor is
    // the clock, which the job reads as a file: `machinehost::CLOCK`)
    let quiet = |k: &str| {
        k.starts_with("PARTEX_STORE")
            || k.starts_with("PARTEX_WATCH")
            || matches!(
                k,
                "PARTEX_CACHE_DIR" | "PARTEX_REPORT" | "SOURCE_DATE_EPOCH" | "FORCE_SOURCE_DATE"
            )
    };
    let env: Vec<(Vec<u8>, Vec<u8>)> = crate::resident::job_env()
        .into_iter()
        .filter(|(k, _)| !quiet(&k.to_string_lossy()))
        .map(|(k, v)| (k.into_encoded_bytes(), v.into_encoded_bytes()))
        .collect();
    let mut p = Saver::new();
    params.save(&mut p);
    let dir = std::env::current_dir().ok()?;
    Some(partex_core::persist_hash(&(
        ROOT_TAG,
        exe_id()?,
        dir.as_os_str().as_encoded_bytes(),
        command_line,
        p.into_bytes(),
        env,
    )))
}

impl Keeper {
    /// The store's directory and this job's key in it.
    pub fn place(&self) -> (PathBuf, u128) {
        (self.dir.clone(), self.key)
    }

    /// The store for this job, if it is on.
    pub fn new(params: &Params, command_line: &[u8]) -> Option<Self> {
        let dir = store::dir(CONFIGURED.get().map(PathBuf::as_path))?;
        Some(Self {
            dir,
            key: identity(params, command_line)?,
            kept: Arc::default(),
            saving: None,
            dirty: false,
            saved: None,
        })
    }

    /// The build saved for this job, loaded with `native` as its host
    /// (or `native` back if there is none that loads), and a report line.
    #[allow(clippy::result_large_err, clippy::cast_precision_loss)] // (the host, moved back; MB)
    #[allow(clippy::too_many_lines)] // (with its timing)
    pub fn load(&mut self, native: NativeHost) -> Result<(Build<Machine>, String), NativeHost> {
        let t = Instant::now();
        let Some(opened) = store::open(&self.dir, self.key) else {
            if debug() {
                eprintln!(
                    "partex: store: no saved build for this job ({:032x}) that reads back whole",
                    self.key
                );
            }
            return Err(native);
        };
        let t_open = t.elapsed();
        let opened = Arc::new(opened);
        let fetch = |h: u128| opened.get(h);
        let mut l = Loader::merkle(&opened.root, &fetch, Loaded::new());
        if l.take(ROOT_TAG.len()) != Some(ROOT_TAG) {
            return Err(native);
        }
        let template = MachineHost::load_shared(native, &mut l)?;
        let t_shared = t.elapsed();
        // (snapshots loaded where first used, unless PARTEX_STORE_LAZY=0)
        let lazy = std::env::var_os("PARTEX_STORE_LAZY").is_none_or(|v| v != "0");
        let ctx = lazy.then(|| {
            Arc::new(LazyCtx {
                opened: opened.clone(),
                loaded: Mutex::new(Loaded::new()),
                template: template.clone(),
            })
        });
        let make = |h: u128| -> Box<dyn LazyBody<MachineHost>> {
            Box::new(StoredBody {
                h,
                ctx: ctx.clone().expect("lazy"),
                body: OnceLock::new(),
            })
        };
        let make = ctx
            .is_some()
            .then_some(&make as partex_core::machine::MakeLazy<MachineHost>);
        let cx = partex_core::machine::Cx::new(&template, make);
        let parts = partex_core::machine::load_parts(&mut l, &cx);
        let mut known = l.take_known();
        // (the blobs the starting state loaded: the final state's too, as
        // a rule)
        let loaded = l.into_loaded();
        drop(cx);
        let t_parts = t.elapsed();
        let mut timing = String::new();
        let b = parts.and_then(|p| {
            let rest = Rest {
                opened: &opened,
                template: &template,
                make,
                ctx: ctx.clone(),
                kept: self.kept.clone(),
            };
            load_rest(p, &rest, loaded, &mut known, &mut timing)
        });
        // (every state loaded holds the template's native host: dropped
        // before it is taken back)
        drop(ctx);
        let b = b.and_then(|(b, chunks)| {
            if std::env::var_os("PARTEX_STORE_CHECK").is_none() {
                return Ok((b, chunks));
            }
            let (n, bad) = partex_core::machine::check_snapshots(&b);
            eprintln!("partex: store: {n} snapshots checked, {bad} differ");
            if bad == 0 {
                Ok((b, chunks))
            } else {
                Err("a snapshot differs from its version")
            }
        });
        let (b, chunks) = match b {
            Ok(b) => b,
            Err(why) => {
                if debug() {
                    eprintln!("partex: store: the saved build does not load: {why}");
                }
                return Err(template.into_native());
            }
        };
        {
            let mut k = self
                .kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            k.stored = opened.stored();
            // (what the stored blobs refer to, read while the build goes
            // on: the next save needs it)
            let (stored, dir) = (k.stored.clone(), self.dir.clone());
            std::thread::spawn(move || {
                stored.warm(&dir);
            });
            k.chunks = chunks;
            // (the final state's, if it loads later, are added then)
            k.known.extend(known);
        }
        let read = opened.read.load(std::sync::atomic::Ordering::Relaxed);
        if debug() {
            eprintln!(
                "partex: store: load: open {:.0} ms, host {:.0} ms, root {:.0} ms,{timing}",
                t_open.as_secs_f64() * 1e3,
                t_shared.saturating_sub(t_open).as_secs_f64() * 1e3,
                t_parts.saturating_sub(t_shared).as_secs_f64() * 1e3,
            );
            eprintln!("partex: store: {:?}", partex_core::machine::census(&b));
        }
        let line = format!(
            "partex: machine: loaded the saved build in {:.1} ms ({:.1} ms to open): {} regions, {:.1} MB read",
            t.elapsed().as_secs_f64() * 1e3,
            t_open.as_secs_f64() * 1e3,
            b.stats.regions,
            (read + opened.root.len() as u64) as f64 / 1e6,
        );
        Ok((b, line))
    }

    /// Save `b` on a thread of its own (a copy of it, its regions merged
    /// to `grain`: the build goes on changing, and a finer one costs more
    /// to keep than it saves), unless a save is running (then
    /// [`Keeper::dirty`] says one is due).
    pub fn save_in_background(&mut self, b: &Build<Machine>, grain: u64) {
        // (a rebuild that stopped early is not a build to keep)
        if self.saved == Some(b.generation()) || !b.settled() {
            return;
        }
        if self.saving.as_ref().is_some_and(|h| !h.is_finished()) {
            self.dirty = true;
            return;
        }
        if let Some(h) = self.saving.take() {
            let _ = h.join();
        }
        self.dirty = false;
        self.saved = Some(b.generation());
        let mut copy = copy_build(b);
        let (dir, key, kept) = (self.dir.clone(), self.key, self.kept.clone());
        self.saving = Some(std::thread::spawn(move || {
            coarsen(&mut copy, grain);
            save(&dir, key, &copy, &kept);
            // (the copy's snapshots dropped here, off the watch's path)
        }));
    }

    /// Wait for the save running, then save `b` if a rebuild ran since it
    /// began: at the end, after the result.
    pub fn finish(&mut self, b: &Build<Machine>, grain: u64) {
        if let Some(h) = self.saving.take() {
            let _ = h.join();
        }
        if self.dirty && self.saved != Some(b.generation()) && b.settled() {
            self.dirty = false;
            self.saved = Some(b.generation());
            let mut copy = copy_build(b);
            coarsen(&mut copy, grain);
            save(&self.dir, self.key, &copy, &self.kept);
        }
    }
}

/// Merge `b`'s regions to `grain` (all of them, however recent), unless
/// `PARTEX_STORE_COARSEN=0`.
fn coarsen(b: &mut Build<Machine>, grain: u64) {
    if std::env::var_os("PARTEX_STORE_COARSEN").is_some_and(|v| v == "0") {
        return;
    }
    b.coarsen(grain, 0);
    drop(b.take_garbage());
}

/// Where snapshots kept in the store are loaded from when first used.
struct LazyCtx {
    opened: Arc<store::Opened>,
    /// The blobs loaded so far, shared by the snapshots loaded.
    loaded: Mutex<Loaded>,
    template: MachineHost,
}

impl LazyCtx {
    fn load(&self, h: u128) -> Option<SnapBody<MachineHost>> {
        let mut loaded = self
            .loaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = self.opened.get(h)?;
        let fetch = |x: u128| self.opened.get(x);
        let mut l = Loader::merkle(&bytes, &fetch, std::mem::take(&mut *loaded));
        let b = partex_core::machine::load_snapshot_body(&mut l, &self.template);
        *loaded = l.into_loaded();
        b
    }
}

/// A snapshot in the store, loaded the first time it is used.
struct StoredBody {
    h: u128,
    ctx: Arc<LazyCtx>,
    body: OnceLock<SnapBody<MachineHost>>,
}

impl LazyBody<MachineHost> for StoredBody {
    fn body(&self) -> &SnapBody<MachineHost> {
        self.body.get_or_init(|| {
            // (the store's packs are open and never rewritten, and what is
            // read is checked: failing here means the disk failed)
            self.ctx.load(self.h).unwrap_or_else(|| {
                panic!(
                    "partex: a saved snapshot ({:032x}) no longer reads back from the store; rerun with PARTEX_STORE=0",
                    self.h
                )
            })
        })
    }
}

/// A copy of `b`, sharing its states' chunks.
fn copy_build(b: &Build<Machine>) -> Build<Machine> {
    let (initial, fin, seq, generation, changed) = b.parts();
    Build::from_parts(
        initial.clone(),
        fin.clone(),
        seq.into_iter().map(|(k, t)| (k, t.clone())).collect(),
        generation,
        changed.clone(),
    )
}

/// Save `b` to the store (errors are reported, not fatal: it is a cache):
/// writing only what the store lacks, and encoding only what the last
/// save or load did not know (runs of regions unchanged since are
/// referred to by their blobs; values it knew by their addresses).
#[allow(clippy::cast_precision_loss)] // (MB)
fn save(dir: &std::path::Path, key: u128, b: &Build<Machine>, kept: &Mutex<Kept>) {
    let t = Instant::now();
    let (have, chunks, known) = {
        let mut k = kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            k.stored.clone(),
            k.chunks.clone(),
            std::mem::take(&mut k.known),
        )
    };
    let fresh = std::env::var_os("PARTEX_STORE_INCREMENTAL").is_some_and(|v| v == "0");
    let (known, chunks) = if fresh {
        (Known::default(), Vec::new())
    } else {
        (known, chunks)
    };
    let reused: std::collections::HashMap<u128, u128> =
        chunks.iter().map(|c| (c.fingerprint, c.hash)).collect();
    let knew = known.len();
    // (the blobs written to the store's new pack as they are made)
    let pack = match store::PackWriter::new(dir) {
        Ok(w) => std::rc::Rc::new(std::cell::RefCell::new(w)),
        Err(e) => {
            if debug() {
                eprintln!("partex: store: saving failed: {e}");
            }
            return;
        }
    };
    let mut s = Saver::merkle_known(MIN_BLOB, have.hashes(), known);
    let sink = pack.clone();
    s.merkle_sink(Box::new(move |b| sink.borrow_mut().add(b)));
    s.raw(ROOT_TAG);
    b.initial().tex().host().save_shared(&mut s);
    let Some(runs) = partex_core::machine::save_build(b, &mut s, &|fp| reused.get(&fp).copied())
    else {
        if debug() {
            eprintln!("partex: store: this build cannot be saved");
        }
        return;
    };
    let refs = s.merkle_roots();
    let next_known = s.take_known();
    let (_, root) = s.into_merkle();
    let t_ser = t.elapsed();
    let Ok(pack) = std::rc::Rc::try_unwrap(pack).map(std::cell::RefCell::into_inner) else {
        unreachable!("the saver, which held the pack's other handle, is gone");
    };
    let runs_reused = runs
        .iter()
        .filter(|r| reused.contains_key(&r.fingerprint))
        .count();
    match pack.finish(key, &root, &refs, &have) {
        Ok((now, saved)) => {
            {
                let mut k = kept
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                k.stored = now;
                k.chunks.clone_from(&runs);
                k.known = next_known;
            }
            if debug() {
                eprintln!(
                    "partex: store: saved in {:.1} ms ({:.1} ms to encode and write the new blobs, {knew} values known, {runs_reused} of {} runs as they were): {} live blobs, {} new ({:.1} MB, {:.1} MB kept), {:.1} MB moved, root {:.1} MB, {} packs",
                    t.elapsed().as_secs_f64() * 1e3,
                    t_ser.as_secs_f64() * 1e3,
                    runs.len(),
                    saved.live,
                    saved.new_blobs,
                    saved.new_raw_bytes as f64 / 1e6,
                    saved.new_bytes as f64 / 1e6,
                    saved.moved_bytes as f64 / 1e6,
                    saved.root_bytes as f64 / 1e6,
                    saved.packs,
                );
                let phases: Vec<String> = saved
                    .phases
                    .iter()
                    .map(|(n, d)| format!("{n} {:.0}", d.as_secs_f64() * 1e3))
                    .collect();
                eprintln!(
                    "partex: store: writing, ms from its start: {}",
                    phases.join(", ")
                );
            }
        }
        Err(e) => {
            if debug() {
                eprintln!("partex: store: saving failed: {e}");
            }
        }
    }
}

/// A run of regions loaded, and what it was saved as.
type Run = (Vec<(u64, Trace<Machine>)>, Option<SavedChunk>);

/// The rest of a saved build, from its parts: the final state on a thread
/// of its own, the runs of regions on the others, then (while the final
/// state may still be loading) the regions' indexes. The build and its
/// runs as saved (`known`: the values loaded from blobs, added to).
/// What [`load_rest`] loads with.
struct Rest<'a> {
    opened: &'a Arc<store::Opened>,
    template: &'a MachineHost,
    make: Option<partex_core::machine::MakeLazy<'a, MachineHost>>,
    ctx: Option<Arc<LazyCtx>>,
    kept: Arc<Mutex<Kept>>,
}

/// Load the final state (a blob) on this thread: with the values it
/// loaded, known by address for the next save, and how long it took.
#[allow(clippy::many_single_char_names)] // (the usual names)
fn load_final_blob(
    h: u128,
    opened: &store::Opened,
    template: &MachineHost,
    ctx: Option<&Arc<LazyCtx>>,
    loaded: Loaded,
) -> Result<(Machine, Known), &'static str> {
    let t = Instant::now();
    let make = |h: u128| -> Box<dyn LazyBody<MachineHost>> {
        Box::new(StoredBody {
            h,
            ctx: ctx.cloned().expect("lazy"),
            body: OnceLock::new(),
        })
    };
    let make = ctx
        .is_some()
        .then_some(&make as partex_core::machine::MakeLazy<MachineHost>);
    let fetch = |x: u128| opened.get(x);
    let bytes = opened.get(h).ok_or("the final state is missing")?;
    let cx = partex_core::machine::Cx::new(template, make);
    let mut l = Loader::merkle(&bytes, &fetch, loaded);
    let (f, d) = partex_core::machine::load_final(&mut l, &cx)?;
    let t_load = t.elapsed();
    partex_core::machine::check_digest(&f, d)?;
    if debug() {
        eprintln!(
            "partex: store: final state loaded in {:.0} ms, checked in {:.0} ms",
            t_load.as_secs_f64() * 1e3,
            t.elapsed().saturating_sub(t_load).as_secs_f64() * 1e3
        );
    }
    Ok((f, l.take_known()))
}

/// Whether the final state loads while a rebuild begins
/// (`PARTEX_STORE_FINAL_LATER=0`: before).
fn final_later() -> bool {
    !std::env::var("PARTEX_STORE_FINAL_LATER").is_ok_and(|v| v == "0")
}

/// The final state, still loading on its own thread: what a build takes
/// when it first needs it. If it does not load after all, the run is
/// made again from the starting state (slow, and never seen but on a
/// failing disk: its blobs were read back and checked).
fn final_when_needed(
    job: std::thread::JoinHandle<Result<(Machine, Known), &'static str>>,
    initial: Machine,
    kept: Arc<Mutex<Kept>>,
) -> partex_incr::MakeLater<Machine> {
    Box::new(move || {
        let t = Instant::now();
        match job.join().unwrap_or(Err("the final state's thread failed")) {
            Ok((f, known)) => {
                kept.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .known
                    .extend(known);
                if debug() {
                    eprintln!(
                        "partex: store: final state waited for {:.0} ms",
                        t.elapsed().as_secs_f64() * 1e3
                    );
                }
                f
            }
            Err(why) => {
                eprintln!(
                    "partex: store: the saved final state does not load ({why}); running the job again for it"
                );
                let cfg = partex_incr::build::Config::default();
                Build::new(initial, &cfg).final_state().clone()
            }
        }
    })
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // (with its timing)
fn load_rest(
    p: Parts<MachineHost>,
    rest: &Rest<'_>,
    loaded: Loaded,
    known: &mut Known,
    timing: &mut String,
) -> Result<(Build<Machine>, Vec<SavedChunk>), &'static str> {
    use std::fmt::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Rest {
        opened,
        template,
        make,
        ..
    } = *rest;
    let t = Instant::now();
    let Parts {
        initial,
        fin,
        chunks,
        generation,
        changed,
    } = p;
    let fetch = |h: u128| opened.get(h);
    let n = chunks.len();
    // (the final state, if a blob, loads on a thread of its own that may
    // outlast this call: a rebuild takes it when it needs it)
    let (fin, later) = match fin {
        Part::Blob(h) if final_later() => {
            let (opened, template, ctx) = (Arc::clone(opened), template.clone(), rest.ctx.clone());
            let job = std::thread::spawn(move || {
                load_final_blob(h, &opened, &template, ctx.as_ref(), loaded)
            });
            (None, Some(job))
        }
        fin => (Some((fin, loaded)), None),
    };
    let slots: Vec<Mutex<Option<Part<partex_core::machine::Regions<MachineHost>>>>> =
        chunks.into_iter().map(|c| Mutex::new(Some(c))).collect();
    let done: Vec<Mutex<Option<Result<Run, &'static str>>>> =
        (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    // (threads for the regions: `PARTEX_STORE_THREADS`, else up to 7)
    let workers = std::env::var("PARTEX_STORE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(4, std::num::NonZero::get)
                .clamp(2, 8)
                - 1
        })
        .max(1);
    let scoped = std::thread::scope(|sc| -> Result<_, &'static str> {
        let fin_thread = fin.map(|(fin, loaded)| {
            sc.spawn(|| {
                let t = Instant::now();
                let r = match fin {
                    Part::Here(f) => Ok((f, Known::default())),
                    Part::Blob(h) => {
                        load_final_blob(h, opened, template, rest.ctx.as_ref(), loaded)
                    }
                };
                (r, t.elapsed())
            })
        });
        let run_threads: Vec<_> = (0..workers)
            .map(|_| {
                sc.spawn(|| {
                    let cx = partex_core::machine::Cx::new(template, make);
                    let mut loaded = Loaded::new();
                    let mut known = Known::default();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        let part = slots[i].lock().ok().and_then(|mut s| s.take());
                        let r = match part {
                            None => Err("a region does not load"),
                            Some(Part::Here(run)) => Ok((run, None)),
                            Some(Part::Blob(h)) => match opened.get(h) {
                                None => Err("a part is missing"),
                                Some(bytes) => {
                                    let mut l =
                                        Loader::merkle(&bytes, &fetch, std::mem::take(&mut loaded));
                                    let r = partex_core::machine::load_chunk(&mut l, &cx);
                                    known.extend(l.take_known());
                                    loaded = l.into_loaded();
                                    r.map(|run| {
                                        let c = partex_core::machine::chunk_of(&run, h);
                                        (run, Some(c))
                                    })
                                    .ok_or("a region does not load")
                                }
                            },
                        };
                        if let Ok(mut d) = done[i].lock() {
                            *d = Some(r);
                        }
                    }
                    known
                })
            })
            .collect();
        let mut known_runs = Known::default();
        for h in run_threads {
            if let Ok(k) = h.join() {
                known_runs.extend(k);
            }
        }
        let t_runs = t.elapsed();
        // (the regions in order, and their indexes, while the final state
        // may still be loading)
        let mut seq = Vec::new();
        let mut saved = Vec::with_capacity(n);
        let mut failed = None;
        for d in &done {
            match d.lock().ok().and_then(|mut d| d.take()) {
                Some(Ok((run, c))) => {
                    seq.extend(run);
                    saved.extend(c);
                }
                Some(Err(e)) => failed = failed.or(Some(e)),
                None => failed = failed.or(Some("a region does not load")),
            }
        }
        let t_ix = Instant::now();
        let index = failed
            .is_none()
            .then(|| partex_incr::Index::of(seq.iter().map(|(k, t)| (*k, t))));
        let t_ix = t_ix.elapsed();
        let fin = match fin_thread {
            Some(j) => {
                let (fin, t_fin) = j
                    .join()
                    .unwrap_or((Err("the final state does not load"), t.elapsed()));
                if let Some(e) = failed {
                    return Err(e);
                }
                let (fin, known_fin) = fin?;
                known_runs.extend(known_fin);
                Some((fin, t_fin))
            }
            None => None,
        };
        if let Some(e) = failed {
            return Err(e);
        }
        Ok((fin, known_runs, t_runs, seq, saved, index, t_ix))
    });
    let (fin, known_rest, t_runs, seq, saved, index, t_ix) = match scoped {
        Ok(s) => s,
        Err(e) => {
            // (the final state's thread holds the starting state's host:
            // done before the caller takes it back)
            if let Some(j) = later {
                let _ = j.join();
            }
            return Err(e);
        }
    };
    known.extend(known_rest);
    let (b, t_fin) = match (fin, later) {
        (Some((fin, t_fin)), _) => (
            partex_core::machine::assemble(initial, fin, seq, generation, changed, index)?,
            t_fin,
        ),
        (None, Some(job)) => {
            let make = final_when_needed(job, initial.clone(), rest.kept.clone());
            let b = partex_core::machine::assemble_later(
                initial, make, seq, generation, changed, index,
            )?;
            (b, std::time::Duration::ZERO)
        }
        (None, None) => return Err("the final state does not load"),
    };
    let _ = write!(
        timing,
        " final {:.0} ms (0: later), regions {:.0} ms, index {:.0} ms ({} runs, {} threads), in all {:.0} ms",
        t_fin.as_secs_f64() * 1e3,
        t_runs.as_secs_f64() * 1e3,
        t_ix.as_secs_f64() * 1e3,
        n,
        workers + 1,
        t.elapsed().as_secs_f64() * 1e3,
    );
    Ok((b, saved))
}

fn debug() -> bool {
    std::env::var_os("PARTEX_STORE_DEBUG").is_some()
        || std::env::var_os("PARTEX_WATCH_DEBUG").is_some()
}

/// What [`Watch::open`] opened.
pub enum Opened {
    /// The build, loaded and brought up to date, or built afresh.
    Ready(Box<Watch>),
    /// Nothing changed since the last build (DESIGN.md §7.9, "No-op
    /// restarts"): its outputs and result stand. The saved build loads in
    /// the background if it is wanted.
    Unchanged(Box<Later>),
}

/// The build after a restart with nothing changed: loading, or not wanted.
pub struct Later {
    job: Option<LoadJob>,
    native: Option<NativeHost>,
    keeper: Option<Keeper>,
    params: Params,
    command_line: Vec<u8>,
    /// The outputs' hashes as the record has them (as they are on disk).
    written: std::collections::BTreeMap<Vec<u8>, u128>,
}

/// A saved build loading on its own thread, and the keeper back.
type LoadJob = JoinHandle<(Result<(Build<Machine>, String), NativeHost>, Keeper)>;

impl Opened {
    /// The watch, for the edits to come: after a restart with nothing
    /// changed, once the saved build is loaded (or, if it does not load,
    /// built afresh), and brought up to date with what changed since (the
    /// outcome then, if anything did).
    pub fn into_watch(
        self,
        between: &mut crate::Between,
        observe: &mut dyn FnMut(crate::events::Progress),
    ) -> (Watch, Option<super::Outcome>) {
        let l = match self {
            Self::Ready(w) => return (*w, None),
            Self::Unchanged(l) => *l,
        };
        let t = Instant::now();
        observe(crate::events::Progress::Phase(
            crate::events::Phase::Loading,
        ));
        let (loaded, keeper) = match (l.job, l.native) {
            (Some(job), _) => match job.join() {
                Ok((r, k)) => (r, Some(k)),
                Err(e) => std::panic::resume_unwind(e),
            },
            (None, Some(native)) => match l.keeper {
                Some(mut k) => {
                    let r = k.load(native);
                    (r, Some(k))
                }
                None => (Err(native), None),
            },
            (None, None) => unreachable!("a later build has its host"),
        };
        match loaded {
            Ok((b, line)) => {
                if debug() {
                    eprintln!(
                        "partex: store: the saved build waited for {:.0} ms ({line})",
                        t.elapsed().as_secs_f64() * 1e3
                    );
                }
                let mut w = Watch::from_build(b);
                w.keeper = keeper;
                w.written = l.written;
                // (what changed since the look at the files)
                let out = w.rebuild(between, observe);
                w.b.index();
                if out.is_some() {
                    w.save();
                }
                (w, out)
            }
            Err(native) => {
                let (mut w, out) = Watch::new(native, l.params, &l.command_line, between, observe);
                w.keeper = keeper;
                w.save();
                (w, Some(out))
            }
        }
    }

    /// At the end of the process, after the result: the saves finished.
    pub fn finish_saving(&mut self) {
        match self {
            Self::Ready(w) => w.finish_saving(),
            Self::Unchanged(l) => {
                if let Some(job) = l.job.take() {
                    let _ = job.join();
                }
            }
        }
    }
}

impl Watch {
    /// The job's build: the one saved for it, rebuilt after what changed
    /// on disk since, else built afresh; to its fixpoint, its outputs
    /// written. The build is saved (in the background) once written.
    ///
    /// If nothing changed since the last build ([`super::quick`]), its
    /// result is the result, and nothing is loaded or written: the saved
    /// build loads in the background if `later` (a watch's first edit
    /// needs it), else not at all.
    pub fn open(
        native: NativeHost,
        params: Params,
        command_line: &[u8],
        between: &mut crate::Between,
        observe: &mut dyn FnMut(crate::events::Progress),
        later: bool,
    ) -> (Opened, super::Outcome) {
        let mut keeper = Keeper::new(&params, command_line);
        let mut native = native;
        if super::quick::on()
            && let Some(k) = &keeper
        {
            let t = Instant::now();
            let (dir, key) = k.place();
            if let Some(hit) = super::quick::check(&dir, key, &mut native) {
                let mut out = hit.outcome;
                out.reports = vec![format!(
                    "partex: machine: nothing changed since the last build ({} files looked at in {:.1} ms)",
                    hit.looked,
                    t.elapsed().as_secs_f64() * 1e3
                )];
                observe(crate::events::Progress::Pass(1, None));
                let (job, native) = match keeper.take() {
                    Some(mut k) if later => {
                        let job: LoadJob = std::thread::spawn(move || (k.load(native), k));
                        (Some(job), None)
                    }
                    k => {
                        keeper = k;
                        (None, Some(native))
                    }
                };
                return (
                    Opened::Unchanged(Box::new(Later {
                        job,
                        native,
                        keeper,
                        params,
                        command_line: command_line.to_vec(),
                        written: hit.written,
                    })),
                    out,
                );
            }
            if debug() {
                eprintln!(
                    "partex: store: something changed since the last build (looked in {:.1} ms)",
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
        }
        let loaded = match keeper.as_mut() {
            Some(k) => {
                observe(crate::events::Progress::Phase(
                    crate::events::Phase::Loading,
                ));
                k.load(native)
            }
            None => Err(native),
        };
        let native = match loaded {
            Ok((b, line)) => {
                let mut w = Self::from_build(b);
                w.keeper = keeper;
                observe(crate::events::Progress::PassStart(1));
                let t = Instant::now();
                let found = w.changes();
                let changes = w.take(found);
                let t_changes = t.elapsed();
                let generation = w.b.generation();
                let edited = !changes.is_empty();
                let first = if changes.is_empty() {
                    let report = w.report(std::time::Duration::ZERO);
                    (line, report)
                } else {
                    let (l2, report) = w.apply(&changes);
                    (format!("{line}\n{l2}"), report)
                };
                let t_apply = t.elapsed();
                let out = w.converge(first, between, observe);
                let t_converge = t.elapsed();
                w.b.index();
                if debug() {
                    eprintln!(
                        "partex: store: after the load: changes {:.0} ms, rebuild {:.0} ms, outputs {:.0} ms, index {:.0} ms",
                        t_changes.as_secs_f64() * 1e3,
                        t_apply.saturating_sub(t_changes).as_secs_f64() * 1e3,
                        t_converge.saturating_sub(t_apply).as_secs_f64() * 1e3,
                        t.elapsed().saturating_sub(t_converge).as_secs_f64() * 1e3,
                    );
                }
                // (a build that did not change is kept as it is)
                if edited || w.b.generation() != generation {
                    w.save();
                }
                return (Opened::Ready(Box::new(w)), out);
            }
            Err(native) => native,
        };
        let (mut w, out) = Self::new(native, params, command_line, between, observe);
        w.keeper = keeper;
        w.save();
        (Opened::Ready(Box::new(w)), out)
    }

    /// Save the build to the store in the background (if the store is on).
    pub fn save(&mut self) {
        if let Some(k) = &mut self.keeper {
            k.save_in_background(&self.b, self.cfg.grain);
        }
    }

    /// Finish saving: at the end of the process, after the result.
    pub fn finish_saving(&mut self) {
        self.finish_quick();
        if let Some(k) = &mut self.keeper {
            k.finish(&self.b, self.cfg.grain);
        }
    }
}
