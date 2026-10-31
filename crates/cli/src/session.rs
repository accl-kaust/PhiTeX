//! The session on this machine: `phitex_live`'s (the document's one
//! Build, its records and outputs) given TeX Live through kpathsea, the
//! cache directory as its store and the output directory as its sink.
//! `phitex build`, `watch` and the TeX-compatible command line run it as
//! [`Session`], which is `phitex_live::session::Session` with the PDF file
//! a watch keeps as update sections ([`UpdateFile`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phitex_live::host::{Host, Key, Sink, Store};
pub use phitex_live::session::{Job, Report, RunInfo, Status, shared, threads, waiting_notes};
use phitex_tex::{Finder, ToolFinder};

use crate::cache::Cache;
use crate::tex;

/// A document's session on this machine (`phitex_live`'s, which it
/// derefs to).
pub struct Session {
    live: phitex_live::session::Session,
    out: Arc<Out>,
    files: tex::Files,
}

impl std::ops::Deref for Session {
    type Target = phitex_live::session::Session;
    fn deref(&self) -> &Self::Target {
        &self.live
    }
}

impl std::ops::DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.live
    }
}

impl Session {
    /// A session for `job`, its TeX files from `files`: the main file
    /// read.
    pub fn new(
        job: Job,
        files: tex::Files,
        status: Arc<Status>,
        note: &mut dyn FnMut(&str),
    ) -> Result<Session, String> {
        if files == tex::Files::Shelf {
            return Session::on_shelf(job, status, note);
        }
        // (the `ls-R` databases kept in the cache: read, not parsed, by
        // the next process while they are unchanged)
        phitex_kpse::set_db_cache(Some(tex::cache_dir().join("kpse")));
        // (kpathsea opened for the job and for the format on threads of
        // their own while the main file is read)
        let (text, tree, format_tree) = std::thread::scope(|s| {
            let tree = s.spawn(|| Arc::new(tex::Tree::open(&job.engine)));
            let format_tree = s.spawn(|| Arc::new(tex::Tree::open_format(&job.engine)));
            let text = std::fs::read(&job.path).map_err(|e| format!("{}: {e}", job.path.display()));
            phitex_tex::timeline::lap("session: the main file read");
            let text = text.map(|t| Arc::<str>::from(tex::chars_owned(t)));
            phitex_tex::timeline::lap("session: its characters");
            let tree = tree.join().expect("kpathsea opened");
            let format_tree = format_tree.join().expect("kpathsea opened for the format");
            phitex_tex::timeline::lap("session: kpathsea opened (texmf.cnf, ls-R; the format's)");
            (text, tree, format_tree)
        });
        let text = text?;
        let (store, out) = Self::store_and_out(&job, note);
        let host = Arc::new(Machine { tree, format_tree });
        Ok(Session {
            live: phitex_live::session::Session::new(job, text, host, store, out.clone(), status),
            out,
            files,
        })
    }

    /// A session whose TeX files are Shelf's (and the project's own):
    /// no kpathsea.
    fn on_shelf(
        job: Job,
        status: Arc<Status>,
        note: &mut dyn FnMut(&str),
    ) -> Result<Session, String> {
        let text = std::fs::read(&job.path)
            .map(|t| Arc::<str>::from(tex::chars_owned(t)))
            .map_err(|e| format!("{}: {e}", job.path.display()))?;
        let files = crate::shelf::files(job.engine.name, status.meter())?;
        // (XeTeX's fonts by name: the release's font index, or why not)
        if job.engine.xetex() {
            crate::shelf::font_index(&files)?;
        }
        let (store, out) = Self::store_and_out(&job, note);
        let host = Arc::new(crate::shelf::ShelfHost { files });
        Ok(Session {
            live: phitex_live::session::Session::new(job, text, host, store, out.clone(), status),
            out,
            files: tex::Files::Shelf,
        })
    }

    /// The cache as the session's store, and its outputs' directory.
    fn store_and_out(job: &Job, note: &mut dyn FnMut(&str)) -> (Option<Arc<dyn Store>>, Arc<Out>) {
        let cache = Cache::open(job.cache_limit).ok();
        phitex_tex::timeline::lap("session: the cache opened");
        if let Some(c) = &cache
            && let Some(w) = c.unkept()
        {
            note(&w);
        }
        let store = cache.map(|cache| {
            Arc::new(Kept {
                cache,
                doc: job.path.clone(),
            }) as Arc<dyn Store>
        });
        let out = Arc::new(Out {
            dir: job.out_dir.clone(),
            written: Mutex::new(BTreeMap::new()),
            last_main: Mutex::new(None),
            updates: Mutex::new(None),
        });
        (store, out)
    }

    /// Where the session's TeX files come from.
    pub fn files(&self) -> tex::Files {
        self.files
    }

    /// The PDF file written as update sections (a watch's, but with
    /// `--exact`): what it holds ([`UpdateFile`]).
    pub fn updates(&self) -> Option<Arc<Mutex<UpdateFile>>> {
        self.out
            .updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The PDF file written as update sections from now on (`u`), or
    /// exact.
    pub fn set_updates(&mut self, u: Option<Arc<Mutex<UpdateFile>>>) {
        *self
            .out
            .updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = u;
    }
}

/// TeX Live: kpathsea as the job's program, and as the format's build
/// sees it (without the current directory: fmtutil builds formats in a
/// scratch directory).
struct Machine {
    tree: Arc<tex::Tree>,
    format_tree: Arc<tex::Tree>,
}

impl Host for Machine {
    fn locate(&self, name: &str) -> Option<PathBuf> {
        self.tree.input(name)
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }

    /// Whether two names are one file on disk.
    fn same_file(&self, a: &str, b: &str) -> bool {
        match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    }

    fn canonical(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn has_format_file(&self, name: &str) -> bool {
        self.format_tree.input(name).is_some()
    }

    /// The format's files, by kpathsea outside the document's directory.
    fn format_finder(&self) -> Option<Finder> {
        let tree = self.format_tree.clone();
        Some(Arc::new(move |name: &str| {
            let p = tree.input(name)?;
            let bytes = std::fs::read(&p).ok()?;
            Some((p.to_string_lossy().into_owned(), bytes))
        }))
    }

    /// The format's files located as [`Host::format_finder`] finds them,
    /// keyed by their stat, none read: a warm start checks the format
    /// against them without reading them.
    fn content_key(&self) -> Option<phitex_tex::ContentKey> {
        let tree = self.format_tree.clone();
        Some(Arc::new(move |name: &str| {
            let p = tree.input(name)?.to_string_lossy().into_owned();
            let k = phitex_tex::file_stat_key(&p);
            (!k.is_empty()).then_some((p, k))
        }))
    }

    /// The tools' support files (bibliography styles and databases,
    /// index styles), by kpathsea in each one's format.
    fn tool_finder(&self) -> Option<ToolFinder> {
        let tree = self.tree.clone();
        Some(Arc::new(
            move |search: phitex_tex::ToolSearch, name: &str| {
                let p = tree.tool_input(search, name)?;
                let bytes = std::fs::read(&p).ok()?;
                Some((p.to_string_lossy().into_owned(), bytes))
            },
        ))
    }

    fn bibtex(&self) -> phitex_bibtex::Options {
        crate::tools::bib_options(&mut crate::tex::kpse("bibtex", ""), false, 2)
    }

    /// METAFONT's runs kept in the cache directory.
    fn mktex(&self) -> Option<phitex_tex::Mktex> {
        Some(crate::mkfont::maker())
    }

    /// XeTeX's native fonts: TeX Live's, by file and by name.
    fn xetex(&self) -> Option<phitex_tex::XeTeX> {
        Some(crate::xefonts::xetex())
    }

    /// The commands neither in process nor refused: run as processes,
    /// each in a scratch directory of its own.
    fn shell_hook(&self, job: &Job) -> Option<Arc<dyn phitex_tex::ShellHook>> {
        Some(Arc::new(crate::shell::ProcessHook::new(
            job.path
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
            crate::tex::kpse("pdftex", "pdftex").exported(),
        )))
    }

    fn xdv_files(&self, dir: &Path) -> Option<phitex_live::dvipdf::FilesFor> {
        Some(crate::dvipdf::xdv_files(dir))
    }

    fn dvi_to_pdf(&self, dvi: &[u8], pdf_name: &str, date: i64) -> Result<Vec<u8>, String> {
        crate::dvipdf::dvi_to_pdf(dvi, pdf_name, date)
    }

    fn failure_report(
        &self,
        file: &str,
        why: &str,
        threads: usize,
        internal: &str,
        errors: &[String],
    ) -> Option<String> {
        crate::failure::report_file(file, why, threads, internal, errors)
            .map(|p| crate::cache::home_relative(&p))
    }

    /// The process's resident set, Linux only: VmRSS in kB.
    fn resident_kb(&self) -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("VmRSS:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            })
            .unwrap_or(0)
    }

    fn progress(&self, progress: &Arc<phitex_tex::Progress>) {
        snapshots(progress);
    }
}

/// The cache directory as the session's store: the formats and their
/// package loads' records, the document's files under its own directory.
struct Kept {
    cache: Cache,
    /// The main file, whose directory in the cache is the document's.
    doc: PathBuf,
}

impl Kept {
    fn path(&self, key: Key<'_>) -> PathBuf {
        match key {
            Key::Doc(f) => self.cache.doc_dir(&self.doc).join(f),
            Key::Format(n, f) => self.cache.format_file(n, f),
        }
    }
}

impl Store for Kept {
    fn get(&self, key: Key<'_>) -> Option<Vec<u8>> {
        self.cache.get(&self.path(key))
    }

    fn put(&self, key: Key<'_>, bytes: &[u8]) {
        let _ = self.cache.put(&self.path(key), bytes, &[]);
    }

    fn format(&self, name: &str) -> Option<([u8; 32], Vec<u8>)> {
        self.cache.format(name)
    }

    fn put_format(&self, name: &str, key: &[u8; 32], bytes: &[u8]) {
        let _ = self.cache.put_format(name, key, bytes);
    }
}

/// The output directory as the session's sink.
struct Out {
    dir: PathBuf,
    /// The hashes of the files the last write wrote (an unchanged file is
    /// not written again).
    written: Mutex<BTreeMap<PathBuf, u64>>,
    /// The main output last written, and where ([`Out::put_main`]).
    last_main: Mutex<Option<(PathBuf, Arc<Vec<u8>>)>>,
    /// The PDF file written as update sections, if it is.
    updates: Mutex<Option<Arc<Mutex<UpdateFile>>>>,
}

impl Sink for Out {
    fn begin(&self) -> Result<(), String> {
        let dir = &self.dir;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
    }

    fn put(&self, name: &str, bytes: &[u8], h: u64) -> Result<PathBuf, String> {
        let p = self.dir.join(name);
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        // (bytes the last write wrote, still there: not written again)
        if written.get(&p) != Some(&h) || !p.is_file() {
            let tmp = p.with_file_name(format!(
                ".{}.phitex-tmp",
                p.file_name()
                    .map_or_else(Default::default, |f| f.to_string_lossy())
            ));
            std::fs::write(&tmp, bytes)
                .and_then(|()| std::fs::rename(&tmp, &p))
                .map_err(|e| format!("can't write {}: {e}", p.display()))?;
            written.insert(p.clone(), h);
        }
        Ok(p)
    }

    /// The main output written at `name`: where the file is the last one
    /// written here (same length on disk), only from the first byte
    /// that differs, in place; else whole (through a temporary file).
    fn put_main(&self, name: &str, b: &Arc<Vec<u8>>, h: u64) -> Result<PathBuf, String> {
        use std::io::{Seek, SeekFrom, Write};
        const B: usize = 1 << 16;
        let p = self.dir.join(name);
        let p = p.as_path();
        let err = |e: std::io::Error| format!("can't write {}: {e}", p.display());
        let mut last = self
            .last_main
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let done = |last: &mut Option<(PathBuf, Arc<Vec<u8>>)>| {
            *last = Some((p.to_path_buf(), b.clone()));
            self.written
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(p.to_path_buf(), h);
            Ok(p.to_path_buf())
        };
        if let Some((lp, old)) = last.as_ref()
            && lp == p
            && std::fs::metadata(p).is_ok_and(|m| m.len() == old.len() as u64)
        {
            if Arc::ptr_eq(old, b) {
                self.written
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(p.to_path_buf(), h);
                return Ok(p.to_path_buf());
            }
            // (the first differing byte, a block at a time)
            let n = old.len().min(b.len());
            let mut at = 0;
            while at < n && old[at..(at + B).min(n)] == b[at..(at + B).min(n)] {
                at += B;
            }
            while at < n && old[at] == b[at] {
                at += 1;
            }
            if at < b.len() || old.len() != b.len() {
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .open(p)
                    .map_err(err)?;
                f.seek(SeekFrom::Start(at as u64)).map_err(err)?;
                f.write_all(&b[at..]).map_err(err)?;
                f.set_len(b.len() as u64).map_err(err)?;
            }
            return done(&mut last);
        }
        let name = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let tmp = p.with_file_name(format!(".{name}.phitex-tmp"));
        std::fs::write(&tmp, b.as_slice())
            .and_then(|()| std::fs::rename(&tmp, p))
            .map_err(err)?;
        done(&mut last)
    }

    fn updates(&self) -> bool {
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// The PDF file at `name` brought to layout `l`: an update section
    /// appended where the file holds an earlier layout (as its
    /// [`UpdateFile`] says, and as long as its length on disk), else the
    /// exact file written whole.
    fn put_update(&self, name: &str, l: &phitex_pdf::Layout) -> Result<PathBuf, String> {
        use std::io::Write;
        let p = self.dir.join(name);
        let Some(u) = self
            .updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return Err(format!("{}: no update sections here", p.display()));
        };
        let err = |e: std::io::Error| format!("can't write {}: {e}", p.display());
        let mut g = u.lock().unwrap_or_else(PoisonError::into_inner);
        let on_disk = std::fs::metadata(&p).map(|m| m.len()).ok();
        if let Some((q, up)) = g.at.as_mut()
            && *q == p
            && on_disk == Some(up.len())
        {
            if let phitex_pdf::Section::Append(b) = up.section(l) {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&p)
                    .and_then(|mut f| f.write_all(&b))
                    .map_err(err)?;
                g.exact = false;
                g.writes += 1;
            }
            return Ok(p);
        }
        let exact = l.exact().map_err(|e| format!("{e}"))?;
        let name = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let tmp = p.with_file_name(format!(".{name}.phitex-tmp"));
        std::fs::write(&tmp, exact.as_slice())
            .and_then(|()| std::fs::rename(&tmp, &p))
            .map_err(err)?;
        g.at = Some((
            p.clone(),
            phitex_pdf::Updater::after_exact(l.clone(), &exact),
        ));
        g.exact = true;
        g.writes += 1;
        Ok(p)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let p = self.dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("can't write {}: {e}", p.display()))
    }

    fn dir_of(&self, name: &str) -> Result<(), String> {
        match self.dir.join(name).parent() {
            Some(d) => {
                std::fs::create_dir_all(d).map_err(|e| format!("can't make {}: {e}", d.display()))
            }
            None => Ok(()),
        }
    }

    fn remove(&self, name: &str) {
        let _ = std::fs::remove_file(self.dir.join(name));
    }

    fn get(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(name)).ok()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn here(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_default()
    }
}

/// The PDF file a watch writes: the exact file, then an update section
/// after each edit ([`phitex_pdf::Updater`]), the exact file again once
/// the editor is idle ([`compact`]).
#[derive(Default)]
pub struct UpdateFile {
    /// Where, and what it holds (none: nothing written yet).
    pub at: Option<(PathBuf, phitex_pdf::Updater)>,
    /// Whether it is the exact file of what it holds.
    pub exact: bool,
    /// Its writes so far (a compaction of an older state is dropped).
    pub writes: u64,
}

impl UpdateFile {
    /// Whether the file on disk is the exact one: written so, or with no
    /// update sections at all (a DVI file, written whole each build).
    pub fn is_exact(&self) -> bool {
        self.exact || self.at.is_none()
    }
}

/// The exact file of what `f` holds written over it, through a
/// temporary file renamed (atomic), unless it is written already or
/// another write came meanwhile: whether it was written. The exact
/// bytes are made and written without the lock: a section an edit
/// appends meanwhile wins, and the next idle writes the exact file.
///
/// # Errors
/// The file system's.
pub fn compact(f: &Mutex<UpdateFile>) -> Result<bool, String> {
    let (p, layout, writes) = {
        let g = f.lock().unwrap_or_else(PoisonError::into_inner);
        match &g.at {
            Some((p, u)) if !g.exact => (p.clone(), u.held().clone(), g.writes),
            _ => return Ok(false),
        }
    };
    let exact = layout.exact().map_err(|e| format!("{e}"))?;
    let name = p
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = p.with_file_name(format!(".{name}.phitex-exact"));
    let err = |e: std::io::Error| format!("can't write {}: {e}", p.display());
    std::fs::write(&tmp, exact.as_slice()).map_err(err)?;
    let mut g = f.lock().unwrap_or_else(PoisonError::into_inner);
    if g.writes != writes {
        let _ = std::fs::remove_file(&tmp);
        return Ok(false);
    }
    std::fs::rename(&tmp, &p).map_err(err)?;
    g.at = Some((p, phitex_pdf::Updater::after_exact(layout, &exact)));
    g.exact = true;
    g.writes += 1;
    Ok(true)
}

/// The job's clock as engine `e` reads it, in seconds whose UTC fields are
/// the job's date and time: `SOURCE_DATE_EPOCH` (TeX Live's engines but
/// Knuth's `tex`, which ignores it), else the local time now.
pub fn pdf_date(e: &tex::Engine) -> i64 {
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

/// (measure) `PHITEX_SNAPSHOT_SECS=N`: every N seconds while the
/// session lives, a line on standard error: the seconds since the build
/// began, the jobs finished and known in the evaluation, the resident
/// set, and what each worker runs (`op@offset`, the byte offset in the
/// document; `op`: 1 a text node, 2 a box put, 3 an output page, 0
/// another) with how long the same job has been seen running (whether
/// a build is spinning or stuck, from outside it).
fn snapshots(progress: &Arc<phitex_tex::Progress>) {
    let Some(secs) = phitex_tex::OBSERVE
        .then(|| std::env::var("PHITEX_SNAPSHOT_SECS").ok())
        .flatten()
        .and_then(|x| x.parse::<u64>().ok())
        .filter(|&n| n > 0)
    else {
        return;
    };
    let weak = Arc::downgrade(progress);
    let t0 = Instant::now();
    let _ = std::thread::Builder::new()
        .name("phitex-snapshots".into())
        .spawn(move || {
            // (each slot's last job and when it was first seen)
            let mut seen: Vec<(Option<(u16, u64)>, Instant)> = Vec::new();
            loop {
                std::thread::sleep(Duration::from_secs(secs));
                let Some(p) = weak.upgrade() else { return };
                let now = Instant::now();
                let running = p.running();
                seen.resize(running.len(), (None, now));
                let mut jobs = Vec::new();
                for (k, r) in running.iter().enumerate() {
                    if seen[k].0 != *r {
                        seen[k] = (*r, now);
                    }
                    if let Some((op, off)) = r {
                        let age = now.duration_since(seen[k].1).as_secs();
                        let at = if *off == phitex_tex::Progress::INSIDE {
                            "inside".to_owned()
                        } else {
                            off.to_string()
                        };
                        jobs.push(format!("w{k} {op}@{at} {age}s"));
                    }
                }
                let rss = std::fs::read_to_string("/proc/self/status")
                    .ok()
                    .and_then(|t| {
                        t.lines()
                            .find_map(|l| l.strip_prefix("VmRSS:"))
                            .map(|v| v.trim().to_owned())
                    })
                    .unwrap_or_default();
                eprintln!(
                    "snapshot: {}s, jobs done {} of {} known, rss {rss}, running [{}]",
                    t0.elapsed().as_secs(),
                    p.done(),
                    p.total(),
                    jobs.join(", ")
                );
            }
        });
}
