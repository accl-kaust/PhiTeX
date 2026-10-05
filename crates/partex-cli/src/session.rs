//! Incremental rebuilds: a job keeps checkpoints of the whole engine, and
//! after an edit it resumes from the latest checkpoint the edit leaves
//! valid, instead of starting over.
//!
//! During a session all effects go through [`SessionHost`]: files read are
//! logged with their contents, and output files and the terminal are kept
//! in memory (written to disk when a build is done). A checkpoint is a
//! clone of the engine; cloning the host records how much of each log and
//! output existed then, so restoring a checkpoint rewinds them.
//!
//! A checkpoint is valid for new file contents if everything it has read
//! is unchanged: files read whole (fonts, formats, files already closed)
//! must be identical, and each file still open must be unchanged up to its
//! read position. Resuming then gives exactly the output of a fresh run,
//! since TeX's state at the checkpoint is a function of what it read.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use partex_core::diag::Diagnostic;
use partex_core::dviout::DviWriter;
use partex_core::pageir::Page;
use partex_core::track::Cell;
use partex_core::{DateTime, FileKind, Host, OpenedFile, Params, Step, Tex, WriteId};

use partex_core::persist::{Loader, Persist, Saver};

use crate::intervals::{Interval, IntervalReads, LineEvent, SessionTracker};

/// A session's engine.
type Engine = Tex<SessionHost, SessionTracker>;

use crate::native::{NativeHost, with_suffix};

/// What a lookup found: the resolved name and the contents.
type Found = (Vec<u8>, Arc<[u8]>);

/// A file lookup and what it found.
#[derive(Clone)]
struct Read {
    name: Vec<u8>,
    kind: Query,
    found: Option<Found>,
}

/// What a lookup asked of a file: its contents (as a file of a kind), or
/// when it was last changed (`\pdffilemoddate`, found as the date's text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Query {
    File(FileKind),
    ModDate,
}

#[derive(Clone)]
struct Output {
    name: Vec<u8>,
    bytes: Vec<u8>,
}

struct Shared {
    base: NativeHost,
    reads: Vec<Read>,
    outputs: Vec<Output>,
    term: Vec<u8>,
    /// The pages written to the DVI file.
    pages: Vec<Page>,
    /// The structured diagnostics (errors, and warnings and notes when the
    /// base host asks for them), in the order TeX reported them.
    diags: Vec<Diagnostic>,
    /// Files whose contents a lookup found are known to be what is on disk
    /// while the file's stamp is this (`Session::changes`), by path.
    verified: Verified,
}

/// Contents known to be a file's while it has a stamp, by path.
type Verified = HashMap<Vec<u8>, Vec<(Arc<[u8]>, crate::native::Stamp)>>;

/// The session's shared logs behind a lock, so that a host (and the
/// engines of a session's checkpoints) can move between threads. The
/// methods keep `RefCell`'s names: a session runs one engine at a time,
/// so the lock is never contended.
struct Locked<T>(RwLock<T>);

impl<T> Locked<T> {
    fn new(v: T) -> Self {
        Self(RwLock::new(v))
    }

    fn borrow(&self) -> RwLockReadGuard<'_, T> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn borrow_mut(&self) -> RwLockWriteGuard<'_, T> {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// A session's host can move to and be shared between threads.
const _: fn() = || {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<SessionHost>();
};

/// How much of the shared logs existed when a host was cloned.
#[derive(Clone)]
struct Mark {
    reads: usize,
    outputs: Vec<usize>,
    term: usize,
    pages: usize,
    diags: usize,
}

impl Mark {
    /// The logs' start.
    fn start() -> Self {
        Mark {
            reads: 0,
            outputs: Vec::new(),
            term: 0,
            pages: 0,
            diags: 0,
        }
    }
}

/// The shared logs, saved around a trial run.
struct Logs {
    reads: Vec<Read>,
    outputs: Vec<Output>,
    term: Vec<u8>,
    pages: Vec<Page>,
    diags: Vec<Diagnostic>,
}

impl Logs {
    /// These logs up to `m` (as `Shared::rewind` leaves them).
    fn prefix(&self, m: &Mark) -> Self {
        Self {
            reads: self.reads[..m.reads.min(self.reads.len())].to_vec(),
            outputs: self
                .outputs
                .iter()
                .zip(&m.outputs)
                .map(|(o, &len)| Output {
                    name: o.name.clone(),
                    bytes: o.bytes[..len.min(o.bytes.len())].to_vec(),
                })
                .collect(),
            term: self.term[..m.term.min(self.term.len())].to_vec(),
            pages: self.pages[..m.pages.min(self.pages.len())].to_vec(),
            diags: self.diags[..m.diags.min(self.diags.len())].to_vec(),
        }
    }

    /// Whether these logs are what `saved` logged up to `end`: the same
    /// lookups, finding the same files (with the same contents, or the old
    /// and new contents of one of `changed`), and the same outputs,
    /// terminal lines, pages and diagnostics. (Backdating compares effects
    /// as well as the state: the state hash leaves out what was written.)
    fn same_as(&self, saved: &Logs, end: &Mark, changed: &[OldNew]) -> bool {
        let found_as = |now: &Option<Found>, then: &Option<Found>| match (now, then) {
            (None, None) => true,
            (Some((n, a)), Some((m, b))) => {
                n == m
                    && (a[..] == b[..]
                        || changed
                            .iter()
                            .any(|(old, new)| b[..] == old[..] && a[..] == new[..]))
            }
            _ => false,
        };
        let reads = saved.reads.get(..end.reads).is_some_and(|then| {
            then.len() == self.reads.len()
                && then.iter().zip(&self.reads).all(|(t, n)| {
                    t.name == n.name && t.kind == n.kind && found_as(&n.found, &t.found)
                })
        });
        let outputs = self.outputs.len() == end.outputs.len()
            && self
                .outputs
                .iter()
                .zip(&saved.outputs)
                .zip(&end.outputs)
                .all(|((n, t), &len)| n.name == t.name && t.bytes.get(..len) == Some(&n.bytes[..]));
        let term = saved.term.get(..end.term) == Some(&self.term[..]);
        let pages = saved.pages.get(..end.pages) == Some(&self.pages[..]);
        let diags = saved.diags.get(..end.diags) == Some(&self.diags[..]);
        if std::env::var_os("PARTEX_WATCH_DEBUG").is_some()
            && !(reads && outputs && term && pages && diags)
        {
            eprintln!(
                "partex:   effects the same: lookups {reads} ({} vs {}), outputs {outputs}, terminal {term}, pages {pages}, diagnostics {diags}",
                self.reads.len(),
                end.reads
            );
            for (i, (t, n)) in saved.reads.iter().zip(&self.reads).enumerate() {
                if !(t.name == n.name && t.kind == n.kind && found_as(&n.found, &t.found)) {
                    eprintln!(
                        "partex:   lookup {i}: {} {:?} / {} {:?}, found {:?} / {:?}",
                        String::from_utf8_lossy(&n.name),
                        n.kind,
                        String::from_utf8_lossy(&t.name),
                        t.kind,
                        n.found
                            .as_ref()
                            .map(|f| (String::from_utf8_lossy(&f.0).into_owned(), f.1.len())),
                        t.found
                            .as_ref()
                            .map(|f| (String::from_utf8_lossy(&f.0).into_owned(), f.1.len()))
                    );
                }
            }
        }
        reads && outputs && term && pages && diags
    }
}

impl Shared {
    fn logs(&self) -> Logs {
        Logs {
            reads: self.reads.clone(),
            outputs: self.outputs.clone(),
            term: self.term.clone(),
            pages: self.pages.clone(),
            diags: self.diags.clone(),
        }
    }

    fn take_logs(&mut self) -> Logs {
        Logs {
            reads: std::mem::take(&mut self.reads),
            outputs: std::mem::take(&mut self.outputs),
            term: std::mem::take(&mut self.term),
            pages: std::mem::take(&mut self.pages),
            diags: std::mem::take(&mut self.diags),
        }
    }

    fn set_logs(&mut self, l: Logs) {
        self.reads = l.reads;
        self.outputs = l.outputs;
        self.term = l.term;
        self.pages = l.pages;
        self.diags = l.diags;
    }

    fn mark(&self) -> Mark {
        Mark {
            reads: self.reads.len(),
            outputs: self.outputs.iter().map(|o| o.bytes.len()).collect(),
            term: self.term.len(),
            pages: self.pages.len(),
            diags: self.diags.len(),
        }
    }

    fn rewind(&mut self, m: &Mark) {
        self.reads.truncate(m.reads);
        self.outputs.truncate(m.outputs.len());
        for (o, &len) in self.outputs.iter_mut().zip(&m.outputs) {
            o.bytes.truncate(len);
        }
        self.term.truncate(m.term);
        self.pages.truncate(m.pages);
        self.diags.truncate(m.diags);
    }

    /// A lookup, served by an output of this run if TeX wrote the file,
    /// in the native host's order: the output directory first (the name,
    /// then with the format's suffix), then kpathsea's (the name with the
    /// suffix added if it lacks it, then as given: `\input x.aux` finds an
    /// `x.aux.tex` first), each as written by this run or else on disk.
    fn lookup(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        if let Some(p) = self.base.in_output_dir(name) {
            for cand in [p.clone(), with_suffix(&p, kind)] {
                if let Some(o) = self.outputs.iter().rev().find(|o| o.name == cand) {
                    return Some(OpenedFile {
                        name: o.name.clone(),
                        contents: Arc::from(&o.bytes[..]),
                    });
                }
                if std::fs::metadata(crate::native::path(&cand)).is_ok_and(|m| m.is_file()) {
                    return self.base.read_file(name, kind);
                }
            }
        }
        let suffixed = with_suffix(name, kind);
        if let Some(f) = self.output_file(name, &suffixed, kind) {
            return Some(f);
        }
        if suffixed != name
            && let Some(f) = self.output_file(name, name, kind)
        {
            // (unless the suffixed name is on disk: kpathsea finds it first;
            // its lookup of that name also tries the name as given, which
            // would find the disk's copy of this very output)
            let on_disk = self
                .base
                .read_file(&suffixed, kind)
                .filter(|g| g.name.ends_with(&suffixed));
            return Some(on_disk.unwrap_or(f));
        }
        self.base.read_file(name, kind)
    }

    /// The output of this run named `written` (asked for as `name`),
    /// named as kpathsea finds that file on disk if it is there (a
    /// directory of the search path can be the current one by another
    /// name: `TEXINPUTS=$PWD:`).
    fn output_file(&mut self, name: &[u8], written: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let o = self.outputs.iter().rev().find(|o| o.name == written)?;
        let contents = Arc::from(&o.bytes[..]);
        let out_name = o.name.clone();
        if name.starts_with(b"/") || name.starts_with(b".") {
            return Some(OpenedFile {
                name: out_name,
                contents,
            });
        }
        let mut shown = b"./".to_vec();
        shown.extend_from_slice(&out_name);
        if let Some(disk) = self.base.read_file(written, kind)
            && same_file(&disk.name, &out_name)
        {
            shown = disk.name;
        }
        Some(OpenedFile {
            name: shown,
            contents,
        })
    }
}

/// The host of a session (see the module documentation).
pub struct SessionHost {
    shared: Arc<Locked<Shared>>,
    mark: Mark,
}

impl Clone for SessionHost {
    fn clone(&self) -> Self {
        let mark = self.shared.borrow().mark();
        Self {
            shared: self.shared.clone(),
            mark,
        }
    }
}

impl Host for SessionHost {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut sh = self.shared.borrow_mut();
        let found = sh.lookup(name, kind);
        sh.reads.push(Read {
            name: name.to_vec(),
            kind: Query::File(kind),
            found: found.as_ref().map(|f| (f.name.clone(), f.contents.clone())),
        });
        found
    }

    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        let mut sh = self.shared.borrow_mut();
        // (openclose.c's `open_output`: in the output directory)
        let n = with_suffix(name, kind);
        let n = sh.base.in_output_dir(&n).unwrap_or(n);
        let id = WriteId(u32::try_from(sh.outputs.len()).ok()?);
        sh.outputs.push(Output {
            name: n.clone(),
            bytes: Vec::new(),
        });
        Some((id, n))
    }

    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        let mut sh = self.shared.borrow_mut();
        if let Some(o) = sh.outputs.get_mut(file.0 as usize) {
            o.bytes.extend_from_slice(bytes);
        }
    }

    fn close(&mut self, _file: WriteId) {}

    fn page_written(&mut self, page: &Page) {
        self.shared.borrow_mut().pages.push(page.clone());
    }

    fn term_write(&mut self, bytes: &[u8]) {
        self.shared.borrow_mut().term.extend_from_slice(bytes);
    }

    fn diagnostic(&mut self, d: &Diagnostic) {
        self.shared.borrow_mut().diags.push(d.clone());
    }

    fn notes(&self) -> bool {
        self.shared.borrow().base.notes
    }

    fn shipping(&mut self, count0: i32) {
        let mut sh = self.shared.borrow_mut();
        if !sh.base.notes {
            return;
        }
        sh.diags.push(Diagnostic {
            severity: partex_core::diag::Severity::Note,
            code: crate::events::PAGE,
            message: count0.to_string().into_bytes(),
            help: Vec::new(),
            frames: Vec::new(),
            suggestions: Vec::new(),
            boxed: None,
        });
    }

    /// No terminal input in a session (as if it were at its end).
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn now(&self) -> DateTime {
        self.shared.borrow().base.now()
    }

    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        crate::zlib::deflate_stream(level, data)
    }

    fn cache_get(&mut self, key: u128) -> Option<Vec<u8>> {
        self.shared.borrow_mut().base.cache_get(key)
    }

    fn cache_put(&mut self, key: u128, value: &[u8]) {
        self.shared.borrow_mut().base.cache_put(key, value);
    }

    fn cached(&mut self, key: u128) -> Option<partex_core::host::Memo> {
        self.shared.borrow_mut().base.cached(key)
    }

    fn cache(&mut self, key: u128, value: partex_core::host::Memo) {
        self.shared.borrow_mut().base.cache(key, value);
    }

    fn creation_date(&mut self) -> Vec<u8> {
        self.shared.borrow_mut().base.creation_date()
    }

    // (not logged: a rebuild does not notice that a file's date changed)
    /// Logged as a lookup, as any other file read: an edit changes the
    /// date, and the rebuild must see it.
    fn file_mod_date(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let mut sh = self.shared.borrow_mut();
        let date = sh.base.file_mod_date(name);
        sh.reads.push(Read {
            name: name.to_vec(),
            kind: Query::ModDate,
            found: date.as_ref().map(|d| (name.to_vec(), Arc::from(&d[..]))),
        });
        date
    }
}

/// The first offset where `a` and `b` differ (`None` if equal).
fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    if a == b {
        return None;
    }
    Some(
        a.iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len())),
    )
}

/// The magnification in the postamble of the finished DVI file `dvi`
/// (§642).
fn dvi_mag(dvi: &[u8]) -> Option<i32> {
    const POST: u8 = 248;
    const ID_BYTE: u8 = 2;
    let four =
        |i: usize| -> Option<i32> { Some(i32::from_be_bytes(dvi.get(i..i + 4)?.try_into().ok()?)) };
    // post_post: its pointer to `post`, the id byte, then 4 to 7 223's
    let end = dvi.len() - dvi.iter().rev().take_while(|&&b| b == 223).count();
    if end < 5 || dvi[end - 1] != ID_BYTE {
        return None;
    }
    let post = usize::try_from(four(end - 5)?).ok()?;
    if dvi.get(post) != Some(&POST) {
        return None;
    }
    four(post + 13)
}

/// Write `pages` and finish the DVI file with `writer` (which has
/// written `bytes`), as TeX would: the writer after `n` pages and the
/// file's length then, for each `n` `wanted`. The magnification is that
/// of the finished file `old`.
fn rewrite_dvi(
    writer: &DviWriter,
    bytes: &mut Vec<u8>,
    pages: &[Page],
    old: &[u8],
    wanted: &HashSet<usize>,
) -> Option<HashMap<usize, (DviWriter, usize)>> {
    let mag = dvi_mag(old)?;
    let mut w = writer.clone();
    let mut snapshots = HashMap::new();
    if wanted.contains(&0) {
        snapshots.insert(0, (w.clone(), bytes.len()));
    }
    for (n, page) in (1..).zip(pages) {
        w.page(page).ok()?;
        bytes.extend(w.drain());
        if wanted.contains(&n) {
            snapshots.insert(n, (w.clone(), bytes.len()));
        }
    }
    w.finish(mag).ok()?;
    bytes.extend(w.drain());
    Some(snapshots)
}

/// In `text`, the last "… bytes)." number (TeX's "Output written on"
/// line) from `was` to `is`, digit for digit across line breaks (a text
/// without that line is left as it is). False if the number of digits
/// differs (the lines would break elsewhere).
fn patch_byte_count(text: &mut [u8], was: usize, is: usize) -> bool {
    let (was, is) = (was.to_string(), is.to_string());
    if was.len() != is.len() {
        return false;
    }
    let Some(end) = text.windows(8).rposition(|w| w == b" bytes).") else {
        return true; // not shown here (batch mode on the terminal)
    };
    let mut at = Vec::new();
    let mut i = end;
    while i > 0 && at.len() < was.len() {
        i -= 1;
        match text[i] {
            b'0'..=b'9' => at.push(i),
            b'\n' => {}
            _ => break,
        }
    }
    at.reverse();
    if at.len() != was.len() || at.iter().map(|&i| text[i]).ne(was.bytes()) {
        return false;
    }
    for (&i, d) in at.iter().zip(is.bytes()) {
        text[i] = d;
    }
    true
}

/// What a build did.
#[derive(Debug)]
pub struct Report {
    pub history: i32,
    /// Commands run by this build, and by the job in all.
    pub commands: u64,
    pub total_commands: u64,
    pub elapsed: Duration,
    /// Early cutoff: the rebuild reached a state the previous build had
    /// been in (after this many commands of the job) and reused the rest.
    pub cut_at: Option<u64>,
    /// Why it ran what it ran: the first changed input, and where early
    /// cutoff first failed and on what.
    pub why: Vec<String>,
}

/// The previous build, while a rebuild runs: early cutoff compares with
/// its later checkpoints and reuses its outputs.
/// Where a splice moved the outputs of the previous build's later
/// checkpoints.
struct Moves {
    /// Where the previous build was at the checkpoint joined.
    mark: Mark,
    /// Commands this run has begun.
    commands: u64,
    /// Reads and pages this run has logged.
    reads: usize,
    pages: usize,
    diags: usize,
    /// Where the old and the new terminal output after the cut begin;
    /// the same for each output but the DVI file.
    term: (usize, usize),
    outputs: Vec<Option<(usize, usize)>>,
    /// The DVI file, and its writer after each number of pages.
    dvi: Option<usize>,
    /// The PDF file.
    pdf: Option<usize>,
    writers: HashMap<usize, (DviWriter, usize)>,
    /// A PDF object stream rendered again (`Session::objstm_rewrite`): the
    /// record as it is now, and in the previous build's file positions,
    /// where the old object ended and how much longer the new one is.
    objstm: Option<(partex_core::ObjStmWritten, i64, i64)>,
}

/// A state's hash, and its parts' (see `Engine::state_hash_parts`).
type Hashed = (u128, Vec<(&'static str, u128)>);

struct Previous {
    /// The checkpoints after the one the rebuild resumed from, with their
    /// state hashes once computed.
    checkpoints: Vec<(u64, Engine)>,
    hashes: Vec<Option<Hashed>>,
    /// Their input hashes once computed (`Engine::input_hash`: a cheap
    /// test before the full one).
    input_hashes: Vec<Option<u128>>,
    /// Differences told so far (`PARTEX_WATCH_DEBUG`).
    explained: u32,
    /// The first failed cutoff: where, and the parts of the state that
    /// differed (for [`Report::why`]); taken by `drive`.
    blocked: Option<String>,
    blocked_seen: bool,
    outputs: Vec<Output>,
    term: Vec<u8>,
    diags: Vec<Diagnostic>,
    reads: Vec<Read>,
    /// The pages written after the first `pages_from`.
    pages: Vec<Page>,
    pages_from: usize,
    /// Files whose contents changed: the old and the new.
    replaced: Vec<OldNew>,
    /// Reads from this index on found the same files as now.
    unchanged_from: usize,
    total_commands: u64,
    history: i32,
    /// What each interval between due checkpoints read, by the commands
    /// before it (`intervals.rs`).
    intervals: Vec<(u64, Interval)>,
    /// The last object stream rendered again, by the checkpoints asked.
    objstm_rewrites: std::cell::RefCell<Option<RewriteAsked>>,
    /// Read-set cutoff: the state met differs from the previous build's
    /// checkpoint only in these eqtb cells, which the rest of the previous
    /// build never read; its later checkpoints take them from the engine.
    overlay: Option<(Engine, Differs)>,
    /// With a read-set cutoff: the first command of the previous build's
    /// first interval that reads what differs (DESIGN.md §7.4), if any:
    /// a splice stops before it and the job goes on from there.
    stop: Option<u64>,
    /// The last reason read-set cutoff gave for not cutting off
    /// (`PARTEX_WATCH_DEBUG`).
    last_refusal: Option<String>,
    /// The eqtb cells the previous build wrote from the checkpoint resumed
    /// from on, as far as `intervals[..old_written.0]` go.
    old_written: (usize, HashSet<i32>),
    /// After a failed full-hash confirmation: checkpoints to pass before
    /// the next, and how many failed so far (the wait doubles).
    confirm_wait: (u32, u32),
}

/// A job that can be rebuilt incrementally.
pub struct Session {
    params: Params,
    command_line: Vec<u8>,
    shared: Arc<Locked<Shared>>,
    /// Checkpoints in order, with the number of commands before each.
    checkpoints: Vec<(u64, Engine)>,
    /// Commands between checkpoints.
    every: u64,
    total_commands: u64,
    history: i32,
    /// When the current drive began (`PARTEX_WATCH_DEBUG` timings).
    t_drive: Instant,
    /// The engines' tracker, and what it found for each interval of this
    /// build (see `Previous::intervals`).
    tracker: SessionTracker,
    intervals: Vec<(u64, Interval)>,
    /// Where the tracker began recording.
    interval_from: u64,
    /// The eqtb cells this build wrote since it resumed (or began): with
    /// the previous build's, the only ones in which the two can differ.
    written_since: HashSet<i32>,
    /// Commands of the previous build a drive joined instead of running.
    skipped: u64,
    /// At most this many checkpoints are kept (`PARTEX_WATCH_CHECKPOINTS`):
    /// past it the one whose neighbours are closest goes, and new ones
    /// closer than `spacing` (half the even spacing) to the last are not
    /// kept (`PARTEX_EVEN_CHECKPOINTS=0`: thinned to a spacing 1.5 times
    /// the last instead). (By commands, not by due
    /// checkpoints: those crowd where many files are opened.) Convergence
    /// is still tested at every due checkpoint.
    budget: usize,
    spacing: u64,
    /// A build or rebuild ran since the session was loaded or saved.
    changed: bool,
    /// The current rebuild's [`Report::why`] so far.
    why: Vec<String>,
    /// The saved checkpoints not loaded yet.
    saved: Option<Saved>,
    /// Where the last rebuild resumed (commands before it).
    resumed_at: Option<u64>,
    /// Checkpoints kept where one of the document's own input files began
    /// or ended, by command: thinning keeps them (`file_boundary`).
    pinned: HashSet<u64>,
    /// The open input files at the last due checkpoint, by address.
    open_at_step: Vec<usize>,
    /// Checkpoints kept between paragraphs where the last rebuild ran
    /// again (`Tex::set_dense`), by command: thinning keeps them until the
    /// next rebuild; and how many more this rebuild may keep.
    dense_pins: HashSet<u64>,
    dense_left: usize,
    /// What `write_outputs` last wrote, by name: length and hash.
    written: std::cell::RefCell<HashMap<Vec<u8>, (usize, u64)>>,
}

/// A saved session's checkpoints not loaded yet (DESIGN.md §5.3), all
/// before the loaded ones.
struct Saved {
    bytes: Vec<u8>,
    pool: std::ops::Range<usize>,
    spans: Vec<(usize, usize)>,
    /// The pool's values loaded so far (the logs' file contents first).
    shared: partex_core::persist::Shared,
    checkpoints: Vec<SavedCheckpoint>,
    /// Files changed since, applied to each as it is loaded.
    replaced: Vec<OldNew>,
}

struct SavedCheckpoint {
    at: u64,
    mark: Mark,
    /// The files open, and where (to tell validity without loading).
    open: Vec<(Arc<[u8]>, usize)>,
    segment: std::ops::Range<usize>,
}

/// Whether a checkpoint that had done `reads` lookups, with the files
/// `open`, is valid after the changes `diffs`: each changed read is of a
/// file still open there and changed only past where it was read.
fn valid_for(diffs: &[Change], reads: usize, open: &[(Arc<[u8]>, usize)]) -> bool {
    diffs[..reads].iter().all(|c| match c.diff {
        None => true,
        Some(d) => c.replace.as_ref().is_some_and(|(old, _)| {
            open.iter()
                .any(|(data, pos)| Arc::ptr_eq(data, old) && d >= *pos)
        }),
    })
}

/// A saved session as one file: the segments (the logs, then each
/// checkpoint), the pool, and where each of its values is.
fn pack(segments: &[Vec<u8>], pool: &partex_core::persist::Pool, stats: bool) -> Vec<u8> {
    let mut out = SAVED_MAGIC.to_vec();
    let mut piece = |b: &[u8]| {
        out.extend_from_slice(&(b.len() as u64).to_le_bytes());
        out.extend_from_slice(b);
    };
    piece(&(segments.len() as u64).to_le_bytes());
    for seg in segments {
        piece(seg);
    }
    piece(&pool.bytes);
    let spans: Vec<u8> = pool
        .spans
        .iter()
        .flat_map(|&(a, n)| [a as u64, n as u64])
        .flat_map(u64::to_le_bytes)
        .collect();
    piece(&spans);
    if stats {
        // (`PARTEX_PERSIST_STATS`: where the bytes go)
        let mut by: Vec<_> = pool.sizes.iter().flatten().collect();
        by.sort_by_key(|x| std::cmp::Reverse(x.1.1));
        for (what, (n, bytes)) in by.iter().take(12) {
            eprintln!("partex: saved pool {bytes:>10} bytes in {n:>6} {what}");
        }
        eprintln!(
            "partex: saved logs {} bytes, checkpoints {:?} bytes, pool {} bytes in {} values",
            segments[0].len(),
            segments[1..].iter().map(Vec::len).collect::<Vec<_>>(),
            pool.bytes.len(),
            pool.spans.len()
        );
    }
    out
}

type Unpacked = (
    Vec<std::ops::Range<usize>>,
    std::ops::Range<usize>,
    Vec<(usize, usize)>,
);

/// What `pack` wrote: where the segments and the pool are, and the spans.
fn unpack(bytes: &[u8]) -> Option<Unpacked> {
    let word = |b: &[u8]| usize::try_from(u64::from_le_bytes(b.try_into().ok()?)).ok();
    let mut pieces = Vec::new();
    let mut at = SAVED_MAGIC.len();
    if bytes.get(..at)? != SAVED_MAGIC {
        return None;
    }
    while at < bytes.len() {
        let n = word(bytes.get(at..at + 8)?)?;
        let range = at + 8..(at + 8).checked_add(n)?;
        bytes.get(range.clone())?;
        at = range.end;
        pieces.push(range);
    }
    let count = word(bytes.get(pieces.first()?.clone())?)?;
    if count == 0 || pieces.len() != count + 3 {
        return None;
    }
    let spans = bytes[pieces[count + 2].clone()]
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| Some((word(&c[..8])?, word(&c[8..])?)))
        .collect::<Option<Vec<_>>>()?;
    Some((pieces[1..=count].to_vec(), pieces[count + 1].clone(), spans))
}

/// Checkpoints a saved session keeps.
const SAVED_CHECKPOINTS: usize = 8;

/// The saved form's version (with the executable qualifying cache keys,
/// this only tells the format from others under the same key).
const SAVED_MAGIC: &[u8] = b"partex-session/1";

/// Whether two names of found files are the same file (`./x`, `x` and
/// `$PWD/x`).
/// Whether lookup `r` found the output file named `name`.
fn reads_output(r: &Read, name: &[u8]) -> bool {
    let bare = |n: &[u8]| n.strip_prefix(b"./").unwrap_or(n).to_vec();
    r.found
        .as_ref()
        .is_some_and(|(n, _)| bare(n) == bare(name) || same_file(n, name))
}

fn same_file(a: &[u8], b: &[u8]) -> bool {
    let canon = |n: &[u8]| std::fs::canonicalize(crate::native::path(n)).ok();
    a == b || canon(a).is_some_and(|c| Some(c) == canon(b))
}

/// The parts of the state in which `new` and `old` differ, the PDF
/// writer's by field.
fn state_difference(new: &Engine, old: &Engine) -> String {
    let differ =
        |a: Vec<(&'static str, u128)>, b: Vec<(&'static str, u128)>| -> Vec<&'static str> {
            a.iter()
                .zip(&b)
                .filter(|(x, y)| x.1 != y.1)
                .map(|(x, _)| x.0)
                .collect()
        };
    let parts = differ(new.state_hash_parts(), old.state_hash_parts());
    let mut out = parts.join(", ");
    if parts.contains(&"pdf") {
        let fields = differ(new.pdf_hash_parts(), old.pdf_hash_parts());
        let _ = write!(out, " (pdf: {})", fields.join(", "));
    }
    out
}

/// The eqtb locations among an interval's cell numbers (`intervals.rs`:
/// hash slots are numbered past them).
fn eqtb_locations(cells: &[u32]) -> impl Iterator<Item = i32> + '_ {
    cells
        .iter()
        .filter_map(|&i| i32::try_from(i).ok())
        .filter(|&l| IntervalReads::eqtb_index(l).is_some_and(|i| i32::try_from(i) == Ok(l)))
        .filter(|&l| l < 1 << 20)
}

/// What differs between a rebuild and the previous build's checkpoint
/// where they met (read-set cutoff): eqtb cells, and names (hash slots and
/// strings, when a control sequence of another name was made: a
/// `\label` renamed).
#[derive(Clone, Default)]
struct Differs {
    cells: Vec<i32>,
    names: Option<partex_core::Names>,
    /// How the characters the two builds' PDF fonts have used differ (read
    /// only at the end).
    chars: Vec<partex_core::CharsDiffer>,
    /// The objects waiting in the PDF object stream, as the rebuild wrote
    /// them (output: the previous build's later checkpoints take them
    /// until it writes the stream).
    objstm: Option<partex_core::ObjStmDiffer>,
}

impl Differs {
    fn is_empty(&self) -> bool {
        self.cells.is_empty()
            && self.names.is_none()
            && self.chars.is_empty()
            && self.objstm.is_none()
    }

    /// The tracked cells a later interval must not read.
    fn tracked(&self) -> Vec<Cell> {
        let mut out: Vec<Cell> = self.cells.iter().map(|&l| Cell::Eqtb(l)).collect();
        if let Some(n) = &self.names {
            out.extend(n.texts.iter().map(|&p| Cell::Hash(p)));
            out.extend(n.links.iter().map(|&p| Cell::HashNext(p)));
            out.extend(n.searched.iter().copied());
        }
        out
    }

    /// Give checkpoint `cp` of the previous build what differs, from
    /// `from` (the rebuild where they met).
    fn apply(&self, cp: &mut Engine, from: &Engine) -> bool {
        (self.cells.is_empty() || cp.import_cells(from, &self.cells))
            && self.names.as_ref().is_none_or(|n| cp.import_names(from, n))
            && cp.import_chars(&self.chars)
            && self.objstm.as_ref().is_none_or(|o| cp.import_objstm(o))
    }
}

/// Whether switch `name` is on (on unless set to `0`).
fn switch_on(name: &str) -> bool {
    std::env::var_os(name).is_none_or(|v| v != "0")
}

thread_local! {
    /// Sub-hashes shared between the state hashes a session computes: its
    /// checkpoints and the job running on from one share most chunks
    /// (`hashmemo.rs`).
    static HASH_MEMO: std::cell::RefCell<partex_core::StateHashMemo> =
        std::cell::RefCell::default();
}

/// `t.state_hash()`, memoized across the session's states
/// (`PARTEX_HASH_MEMO=0`: not; the same value).
fn hash_of(t: &Engine) -> u128 {
    if switch_on("PARTEX_HASH_MEMO") {
        HASH_MEMO.with(|m| t.state_hash_memo(&mut m.borrow_mut()))
    } else {
        t.state_hash()
    }
}

/// Early cutoff at the previous build's checkpoint `j` by what the two
/// builds wrote since the checkpoint resumed from (`written`: this
/// build's): the eqtb cells that differ, and the names
/// (`PARTEX_NAME_CUTOFF=0`: not), if a full hash confirms (with them
/// taken from `tex` into the checkpoint) and the rest of the previous
/// build does not read them before its next interval (`partial`: a
/// splice may stop before the first that does, `p.stop`; else none may);
/// `Some(Err)` says why not, `None` waits after a failed confirmation.
fn cheap_cutoff(
    tex: &Engine,
    p: &mut Previous,
    j: usize,
    written: &HashSet<i32>,
    partial: bool,
) -> Option<Result<Differs, String>> {
    let old = &p.checkpoints[j].1;
    let at = p.checkpoints[j].0;
    p.stop = None;
    // what the previous build wrote up to checkpoint `j` (a cursor that
    // goes forward; back from scratch)
    let (mut k, mut set) = std::mem::take(&mut p.old_written);
    if k > 0 && p.intervals.get(k - 1).is_some_and(|(_, iv)| iv.to > at) {
        (k, set) = (0, HashSet::new());
    }
    while let Some((_, iv)) = p.intervals.get(k)
        && iv.to <= at
    {
        set.extend(eqtb_locations(&iv.written));
        k += 1;
    }
    let mut cells: Vec<i32> = written
        .union(&set)
        .copied()
        .filter(|&l| tex.eqtb_cell_hash(l) != old.eqtb_cell_hash(l))
        .collect();
    p.old_written = (k, set);
    cells.sort_unstable();
    let mut differs = Differs {
        cells,
        ..Differs::default()
    };
    let reads = |differs: &Differs, p: &Previous| -> Result<Option<u64>, String> {
        match first_reader(&differs.tracked(), &p.intervals, at, p.total_commands)? {
            Some((first, cell)) if !partial || first == at => Err(format!(
                "the interval at command {first} reads {}",
                cell_name(tex, cell)
            )),
            reader => Ok(reader.map(|(first, _)| first)),
        }
    };
    if let Err(why) = reads(&differs, p) {
        return Some(Err(why));
    }
    if p.confirm_wait.0 > 0 {
        p.confirm_wait.0 -= 1;
        return None;
    }
    let t_confirm = Instant::now();
    let hash = hash_of(tex);
    let mut candidate = None;
    let ok = if differs.cells.is_empty() {
        p.hashes[j]
            .get_or_insert_with(|| (hash_of(old), Vec::new()))
            .0
            == hash
    } else {
        let mut cand = old.clone();
        let ok = cand.import_cells(tex, &differs.cells) && hash_of(&cand) == hash;
        candidate = Some(cand);
        ok
    };
    let ok = ok || {
        // another name made: the hash slots and strings too; characters
        // the rebuild's fonts have used that the previous build's had not
        let names = switch_on("PARTEX_NAME_CUTOFF")
            .then(|| tex.name_differences(old))
            .flatten()
            .filter(|n| !n.slots.is_empty());
        let chars = switch_on("PARTEX_CHARS_CUTOFF")
            .then(|| tex.chars_differ(old))
            .flatten()
            .filter(|c| !c.is_empty());
        let objstm = (partial && switch_on("PARTEX_OBJSTM_CUTOFF"))
            .then(|| tex.objstm_differ(old))
            .flatten();
        (names.is_some() || chars.is_some() || objstm.is_some()) && {
            let mut cand = candidate.unwrap_or_else(|| old.clone());
            let ok = names.as_ref().is_none_or(|n| cand.import_names(tex, n))
                && cand.import_chars(chars.as_deref().unwrap_or(&[]))
                && objstm.as_ref().is_none_or(|o| cand.import_objstm(o))
                && hash_of(&cand) == hash;
            differs.names = names;
            differs.chars = chars.unwrap_or_default();
            differs.objstm = objstm;
            ok
        }
    };
    if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
        eprintln!(
            "partex: confirmation at command {} ({}): {:.1} ms",
            tex.commands(),
            if ok { "the same" } else { "differs" },
            t_confirm.elapsed().as_secs_f64() * 1e3
        );
    }
    if ok {
        return Some(reads(&differs, p).map(|stop| {
            p.stop = stop;
            differs
        }));
    }
    // (the wait doubles, to at most 4 checkpoints: with one after each
    // page, a longer wait replays pages that would have met)
    p.confirm_wait.1 += 1;
    p.confirm_wait.0 = 1 << p.confirm_wait.1.min(2);
    Some(Err(
        "the state differs beyond the cells written (the full hash)".to_owned(),
    ))
}

/// A copy of checkpoint `cp`, where it was (a clone of the host takes
/// where the job is now).
fn clone_checkpoint(cp: &Engine) -> Engine {
    let mut c = cp.clone();
    c.host_mut().mark = cp.host().mark.clone();
    c
}

/// What differs (`d`, where the rebuild met the previous build's
/// checkpoint `j`), for its later checkpoint `k`: less what the intervals
/// between wrote, which `k` holds as the previous build had it.
fn overlay_at(p: &Previous, j: usize, k: usize, d: &Differs) -> Differs {
    let (from, to) = (p.checkpoints[j].0, p.checkpoints[k].0);
    let between: Vec<&Interval> = p
        .intervals
        .iter()
        .filter(|(at, iv)| *at >= from && iv.to <= to)
        .map(|(_, iv)| iv)
        .collect();
    let untouched = |c: Cell| {
        !between
            .iter()
            .any(|iv| interval_has_cell(&iv.written, &iv.written_far, c))
    };
    Differs {
        cells: d
            .cells
            .iter()
            .copied()
            .filter(|&l| untouched(Cell::Eqtb(l)))
            .collect(),
        names: d.names.as_ref().map(|n| partex_core::Names {
            slots: n
                .slots
                .iter()
                .copied()
                .filter(|&s| untouched(Cell::Hash(s)) && untouched(Cell::HashNext(s)))
                .collect(),
            ..n.clone()
        }),
        // (a character the rebuild no longer used that the previous build
        // used again by `k` is in both)
        chars: d
            .chars
            .iter()
            .map(|font| {
                let mut font = *font;
                for iv in &between {
                    for (f, used) in &iv.chars {
                        if *f == font.font {
                            for (r, u) in font.removed.iter_mut().zip(used) {
                                *r &= !u;
                            }
                        }
                    }
                }
                font
            })
            .collect(),
        // (none once the previous build wrote the stream: the splice
        // renders it again, `Session::objstm_rewrite`)
        objstm: d.objstm.clone().filter(|o| {
            !between
                .iter()
                .any(|iv| iv.objstms.iter().any(|r| r.num == o.cur()))
        }),
    }
}

/// Where the rest of the previous build (`intervals` from command `from`
/// to its end, `total`) first reads one of `cells` before writing it:
/// the interval's first command and the cell, or `None` if it never
/// does.
fn first_reader(
    cells: &[Cell],
    intervals: &[(u64, Interval)],
    from: u64,
    total: u64,
) -> Result<Option<(u64, Cell)>, String> {
    let mut live = cells.to_vec();
    let mut at = from;
    while at < total && !live.is_empty() {
        let Some((_, iv)) = intervals.iter().find(|(f, _)| *f == at) else {
            return Err(format!("no record of the interval at command {at}"));
        };
        if !iv.recorded || iv.to <= at {
            return Err(format!("the interval at command {at} was not recorded"));
        }
        if let Some(&c) = live
            .iter()
            .find(|&&c| interval_has_cell(&iv.exposed, &iv.far, c))
        {
            return Ok(Some((at, c)));
        }
        live.retain(|&c| !interval_has_cell(&iv.written, &iv.written_far, c));
        at = iv.to;
    }
    Ok(None)
}

/// A tracked cell's name, for messages.
fn cell_name(tex: &Engine, c: Cell) -> String {
    match c {
        Cell::Eqtb(l) => tex.eqtb_loc_name(l),
        Cell::Hash(p) => format!("the hash slot of {}", tex.eqtb_loc_name(p)),
        Cell::HashNext(p) => format!("the hash link of {}", tex.eqtb_loc_name(p)),
        c => format!("{c:?}"),
    }
}

/// Whether an interval's record (`cells`, `far`) holds cell `c`.
fn interval_has_cell(cells: &[u32], far: &[Cell], c: Cell) -> bool {
    IntervalReads::cell_index(c).map_or_else(
        || far.binary_search(&c).is_ok(),
        |i| cells.binary_search(&i).is_ok(),
    )
}

/// What `partex why` says of a read-set cutoff at `tex` with `cells`
/// differing.
fn cutoff_note(tex: &Engine, d: &Differs, stop: Option<u64>) -> String {
    let cells = d.tracked();
    let names: Vec<String> = cells.iter().take(6).map(|&c| cell_name(tex, c)).collect();
    format!(
        "read-set cutoff at command {}: {} {} ({}{}), which the rest of the last build {}",
        tex.commands(),
        cells.len(),
        if cells.len() == 1 {
            "cell differs"
        } else {
            "cells differ"
        },
        names.join(" "),
        if cells.len() > 6 { " …" } else { "" },
        stop.map_or_else(
            || "never read".to_owned(),
            |r| format!("reads first at command {r}")
        )
    )
}

/// No cutoff at `tex`, for `why`: the first reason goes to `partex why`;
/// `PARTEX_WATCH_DEBUG` prints each reason when it changes.
fn refuse(tex: &Engine, p: &mut Previous, why: &str) {
    if !p.blocked_seen {
        p.blocked_seen = true;
        p.blocked = Some(format!("no cutoff at command {}: {why}", tex.commands()));
    }
    if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
        let short = why.split(" (").next().unwrap_or(why).to_owned();
        if p.last_refusal.as_ref() != Some(&short) {
            eprintln!("partex: cutoff at command {}: no: {why}", tex.commands());
            p.last_refusal = Some(short);
        }
    }
}

/// Where the input of two states at the same position differs (the line
/// being read, as LaTeX's look-ahead leaves an edited line in the buffer).
fn input_difference(new: &Engine, old: &Engine) -> String {
    let ((a, la), (b, lb)) = (new.buffer_view(), old.buffer_view());
    if a == b && la == lb {
        return "the input differs (the stack or the files left)".to_owned();
    }
    let i = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let show = |v: &[u32]| {
        v[i.saturating_sub(20)..(i + 20).min(v.len())]
            .iter()
            .map(|&c| char::from_u32(c).unwrap_or('\u{FFFD}'))
            .collect::<String>()
    };
    format!(
        "the input differs (the buffer at {i}: {:?} / {:?})",
        show(&a),
        show(&b)
    )
}

/// `PARTEX_WATCH_DEBUG`: where two states at the same input position
/// differ.
fn explain_difference(new: &Engine, old: &Engine, interval: Option<&Interval>) {
    let (a, b) = (new.state_hash_parts(), old.state_hash_parts());
    let parts: Vec<_> = a
        .iter()
        .zip(&b)
        .filter(|(x, y)| x.1 != y.1)
        .map(|(x, _)| x.0)
        .collect();
    eprintln!(
        "partex: at command {}, the state differs in: {}",
        new.commands(),
        parts.join(", ")
    );
    // Could the interval after this checkpoint have been skipped: does it
    // read none of the eqtb cells that differ (and nothing else differs)?
    if parts.iter().all(|&p| p == "tables" || p == "pdf") {
        let olds: HashMap<i32, u128> = old.eqtb_cell_hashes().into_iter().collect();
        let news: HashMap<i32, u128> = new.eqtb_cell_hashes().into_iter().collect();
        let differ: Vec<i32> = olds
            .keys()
            .chain(news.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|l| olds.get(l) != news.get(l))
            .collect();
        match interval {
            None => eprintln!("partex:   skip? no record of the interval after it"),
            Some(iv) => {
                let exposed: std::collections::HashSet<u32> = iv.exposed.iter().copied().collect();
                let far: std::collections::HashSet<i32> = iv
                    .far
                    .iter()
                    .filter_map(|c| match c {
                        partex_core::track::Cell::Eqtb(p) => Some(*p),
                        _ => None,
                    })
                    .collect();
                let read: Vec<_> = differ
                    .iter()
                    .filter(|&&l| {
                        IntervalReads::eqtb_index(l)
                            .map_or_else(|| far.contains(&l), |i| exposed.contains(&i))
                    })
                    .map(|&l| new.eqtb_loc_name(l))
                    .collect();
                let pdf = parts.contains(&"pdf");
                eprintln!(
                    "partex:   skip? {} eqtb cells differ, the interval reads {} of them{}{}: {}",
                    differ.len(),
                    read.len(),
                    if pdf {
                        " (and the PDF state differs)"
                    } else {
                        ""
                    },
                    if read.is_empty() && !pdf {
                        " -> SKIPPABLE"
                    } else if read.is_empty() {
                        " -> SKIPPABLE but for the PDF state"
                    } else {
                        ""
                    },
                    read.iter().take(8).cloned().collect::<Vec<_>>().join(" ")
                );
            }
        }
    }
    if parts.contains(&"pdf") {
        let (a, b) = (new.pdf_hash_parts(), old.pdf_hash_parts());
        let f: Vec<_> = a
            .iter()
            .zip(&b)
            .filter(|(x, y)| x.1 != y.1)
            .map(|(x, _)| x.0)
            .collect();
        eprintln!("partex:   pdf fields: {}", f.join(", "));
        eprintln!("partex:   pdf objects: {}", new.pdf_objs_difference(old));
    }
    if parts.contains(&"tables") {
        let old_engine = old;
        let old: HashMap<i32, u128> = old.eqtb_cell_hashes().into_iter().collect();
        let differing: Vec<i32> = new
            .eqtb_cell_hashes()
            .into_iter()
            .filter(|(l, h)| old.get(l) != Some(h))
            .map(|(l, _)| l)
            .collect();
        for &l in differing.iter().take(4) {
            eprintln!(
                "partex:   {} = {:?} / {:?}",
                new.eqtb_loc_name(l),
                new.cell_raw(Cell::Eqtb(l)),
                old_engine.cell_raw(Cell::Eqtb(l))
            );
        }
        let cells: Vec<_> = differing.iter().map(|&l| new.eqtb_loc_name(l)).collect();
        eprintln!(
            "partex:   eqtb cells ({}): {}",
            cells.len(),
            cells.iter().take(25).cloned().collect::<Vec<_>>().join(" ")
        );
    }
}

/// `PARTEX_WATCH_DEBUG`: how long since `t0`.
fn debug_time(what: &str, t0: Instant) {
    if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
        eprintln!("partex: {what}: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    }
}

impl Session {
    pub fn new(params: Params, command_line: Vec<u8>, base: NativeHost, every: u64) -> Self {
        Self {
            params,
            command_line,
            shared: Arc::new(Locked::new(Shared {
                base,
                reads: Vec::new(),
                outputs: Vec::new(),
                term: Vec::new(),
                pages: Vec::new(),
                diags: Vec::new(),
                verified: HashMap::new(),
            })),
            checkpoints: Vec::new(),
            every: every.max(1),
            total_commands: 0,
            history: 0,
            t_drive: Instant::now(),
            budget: std::env::var("PARTEX_WATCH_CHECKPOINTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(256)
                .max(2),
            spacing: 0,
            tracker: SessionTracker::default(),
            intervals: Vec::new(),
            interval_from: 0,
            written_since: HashSet::new(),
            skipped: 0,
            changed: false,
            why: Vec::new(),
            saved: None,
            resumed_at: None,
            pinned: HashSet::new(),
            open_at_step: Vec::new(),
            dense_pins: HashSet::new(),
            dense_left: 0,
            written: std::cell::RefCell::default(),
        }
    }

    /// Run the job from the start.
    pub fn build(&mut self) -> Report {
        let _p = crate::timeline::phase("build");
        let t0 = Instant::now();
        self.checkpoints.clear();
        self.saved = None;
        self.resumed_at = None;
        self.spacing = 0;
        self.intervals.clear();
        crate::intervals::clear(&self.tracker);
        self.interval_from = 0;
        self.written_since.clear();
        {
            let mut sh = self.shared.borrow_mut();
            sh.reads.clear();
            sh.outputs.clear();
            sh.term.clear();
            sh.pages.clear();
            sh.diags.clear();
        }
        let host = SessionHost {
            shared: self.shared.clone(),
            mark: self.shared.borrow().mark(),
        };
        let mut tex = Tex::new(
            host,
            crate::intervals::share(&self.tracker),
            self.params.clone(),
        );
        tex.set_checkpoint_interval(self.every);
        tex.record_objstms(switch_on("PARTEX_OBJSTM_LINK"));
        let step = tex.start(&self.command_line);
        let (history, commands, _) = self.drive(tex, step, None);
        self.total_commands = commands;
        self.history = history;
        self.changed = true;
        Report {
            history,
            commands,
            total_commands: commands,
            elapsed: t0.elapsed(),
            cut_at: None,
            why: Vec::new(),
        }
    }

    /// Run the job again after its inputs changed, from the latest valid
    /// checkpoint (`None` if none changed).
    #[allow(clippy::too_many_lines, reason = "the phases of a rebuild, in order")]
    pub fn rebuild(&mut self) -> Option<Report> {
        let _p = crate::timeline::phase("pass");
        let t0 = Instant::now();
        let mut diffs = self.changes();
        debug_time("changes", t0);
        if diffs.iter().all(|d| d.diff.is_none()) {
            return None;
        }
        self.changed = true;
        let unseen = match self.invisible_edit(&diffs, t0) {
            Ok(r) => return Some(r),
            Err(why) => why,
        };
        self.load_needed(&diffs);
        let trial = self.absorb(&mut diffs);
        debug_time("changes and backdating", t0);
        self.debug_changes(&diffs);
        self.why = self.why_changed(&diffs).into_iter().chain(unseen).collect();
        let k = self.latest_valid(&diffs);
        // The previous build's later checkpoints still see the old files:
        // early cutoff compares with them.
        let old = self.checkpoints.split_off(k.map_or(0, |k| k + 1));
        let mark = match k {
            Some(k) => self.checkpoints[k].1.host().mark.clone(),
            None => Mark::start(),
        };
        // (the intervals before the checkpoint resumed from stay)
        let later = self.intervals_from(k.map_or(0, |k| self.checkpoints[k].0));
        let previous = {
            let mut sh = self.shared.borrow_mut();
            Previous {
                hashes: vec![None; old.len()],
                input_hashes: vec![None; old.len()],
                checkpoints: old,
                outputs: sh.outputs.clone(),
                term: sh.term.clone(),
                diags: sh.diags.clone(),
                reads: sh.reads.clone(),
                pages_from: mark.pages,
                pages: sh.pages.split_off(mark.pages),
                replaced: diffs
                    .iter()
                    .filter(|d| d.diff.is_some())
                    .filter_map(|d| d.replace.clone())
                    .collect(),
                unchanged_from: diffs
                    .iter()
                    .rposition(|d| d.diff.is_some())
                    .map_or(0, |i| i + 1),
                total_commands: self.total_commands,
                history: self.history,
                explained: 0,
                blocked: None,
                blocked_seen: false,
                overlay: None,
                stop: None,
                last_refusal: None,
                old_written: (0, HashSet::new()),
                confirm_wait: (0, 0),
                intervals: later,
                objstm_rewrites: std::cell::RefCell::default(),
            }
        };
        debug_time("previous build kept", t0);
        self.replace_inputs(&diffs);
        self.shared.borrow_mut().rewind(&mark);
        debug_time("inputs replaced", t0);
        let Some(k) = k else {
            return Some(self.rebuild_from_start(previous, t0));
        };
        let start = self.checkpoints[k].0;
        self.resumed_at = Some(start);
        // (the backdating trial that failed ran the first interval)
        self.start_dense();
        let (mut tex, step) = if let Some((tex, logs)) = trial {
            self.shared.borrow_mut().set_logs(logs);
            (tex, Step::Checkpoint)
        } else {
            let mut tex = self.checkpoints[k].1.clone();
            crate::intervals::clear(&self.tracker);
            self.interval_from = start;
            self.written_since.clear();
            let step = tex.resume();
            (tex, step)
        };
        {
            // The log keeps the lookups as they are now (those before the
            // checkpoint: this run logged its own after it).
            let mut sh = self.shared.borrow_mut();
            for (r, d) in sh.reads.iter_mut().zip(&diffs).take(mark.reads) {
                if d.diff.is_some() {
                    r.found.clone_from(&d.now);
                }
            }
        }
        debug_time("resumed", t0);
        tex.set_dense(self.dense_left > 0);
        let (history, commands, cut_at) = self.drive(tex, step, Some(previous));
        self.dense_left = 0;
        self.total_commands = commands;
        self.history = history;
        Some(Report {
            history,
            // (the commands run: not those joined from the previous build)
            commands: commands - start - self.skipped,
            total_commands: commands,
            elapsed: t0.elapsed(),
            cut_at,
            why: std::mem::take(&mut self.why),
        })
    }

    /// Source text as values (`tokendeps.rs`, DESIGN.md §7.2): if every
    /// changed file changed only in lines that read as the same tokens as
    /// before, take the new files as read and run nothing. Otherwise why
    /// not, if it was tried.
    fn invisible_edit(&mut self, diffs: &[Change], t0: Instant) -> Result<Report, Option<String>> {
        let r = self.try_invisible_edit(diffs, t0);
        if r.is_err() {
            // (the rebuild goes on with the new files: so do the events)
            for d in diffs {
                if let (Some(_), Some((old, new))) = (d.diff, &d.replace) {
                    crate::tokendeps::remap(&mut self.intervals, &self.tracker, old, new);
                }
            }
        }
        r
    }

    fn try_invisible_edit(
        &mut self,
        diffs: &[Change],
        t0: Instant,
    ) -> Result<Report, Option<String>> {
        if !crate::tokendeps::enabled()
            || !self.tracker.follows_lines()
            || self.intervals.is_empty()
        {
            return Err(None);
        }
        let refuse = |why: String| {
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!("partex: {why}");
            }
            Some(why)
        };
        if self.saved.is_some() {
            return Err(refuse(
                "tokens not compared: the saved session's lines are not known".to_owned(),
            ));
        }
        crate::tokendeps::covers(&self.intervals, self.total_commands)
            .map_err(|why| refuse(format!("tokens not compared: {why}")))?;
        debug_time("tokens: covered", t0);
        let files = self
            .changed_files(diffs)
            .map_err(|why| refuse(format!("tokens not compared: {why}")))?;
        debug_time("tokens: files", t0);
        self.tracker.prune(&self.intervals);
        debug_time("tokens: pruned", t0);
        if let Some(held) = self.stray_copy(diffs, &files) {
            return Err(refuse(format!(
                "tokens not compared: {held} saw a changed file's old contents elsewhere"
            )));
        }
        // (changed lines by file name: each lookup of a file has its own
        // copy of it)
        debug_time("tokens: stray copies", t0);
        let mut lines: HashMap<&str, usize> = HashMap::new();
        let mut others = Vec::new();
        for (old, new, lookups, name) in &files {
            let (n, other) = crate::tokendeps::invisible(
                &self.intervals,
                &self.tracker,
                old,
                new,
                *lookups,
                name,
            )
            .map_err(|why| refuse(format!("tokens differ: {why}")))?;
            let most = lines.entry(name).or_default();
            *most = (*most).max(n);
            if other {
                others.push(old.clone());
            }
        }
        debug_time("tokens compared", t0);
        let maps: Vec<_> = files
            .iter()
            .map(|(old, new, _, _)| {
                let map = crate::tokendeps::line_map(old, new);
                (old.clone(), new.clone(), map)
            })
            .collect();
        let targets: Vec<Option<Arc<[u8]>>> = {
            let sh = self.shared.borrow();
            sh.reads
                .iter()
                .zip(diffs)
                .map(|(r, d)| {
                    if d.diff.is_some() { &d.now } else { &r.found }
                        .as_ref()
                        .map(|f| f.1.clone())
                })
                .collect()
        };
        let (trials, met) = self
            .backdate_lookups(&others, &maps, &targets)
            .map_err(|why| refuse(format!("tokens the same, but {why}")))?;
        debug_time("other lookups backdated", t0);
        let dropped = self.take_edit(diffs, &maps, met);
        let lines: usize = lines.values().sum();
        Ok(Report {
            history: self.history,
            commands: 0,
            total_commands: self.total_commands,
            elapsed: t0.elapsed(),
            cut_at: None,
            why: vec![format!(
                "{lines} changed line{} read as the same tokens under their category codes: \
                 nothing to run{}",
                if lines == 1 { "" } else { "s" },
                if dropped > 0 {
                    format!(" ({dropped} checkpoints in a changed line dropped)")
                } else {
                    String::new()
                }
            )]
            .into_iter()
            .chain((trials > 0).then(|| {
                format!("{trials} intervals with other lookups of the files run again: the same")
            }))
            .collect(),
        })
    }

    /// Take the changed files (`maps`) as read: checkpoints and events move
    /// to the new contents (a checkpoint in a changed line is dropped: the
    /// number dropped), the lookups find them, and the checkpoints `met`
    /// backdating ran through join the others.
    fn take_edit(&mut self, diffs: &[Change], maps: &[FileMap], met: Vec<(u64, Engine)>) -> usize {
        let before = self.checkpoints.len();
        for (old, new, map) in maps {
            let at = |p: usize| map.get(&p).copied();
            self.checkpoints
                .retain_mut(|(_, cp)| cp.remap_input(old, new, at));
            crate::tokendeps::remap(&mut self.intervals, &self.tracker, old, new);
        }
        let dropped = before.saturating_sub(self.checkpoints.len());
        for (at, cp) in met {
            let k = self.checkpoints.partition_point(|c| c.0 < at);
            if self.checkpoints.get(k).is_none_or(|c| c.0 != at) {
                self.checkpoints.insert(k, (at, cp));
            }
        }
        let mut sh = self.shared.borrow_mut();
        for (r, d) in sh.reads.iter_mut().zip(diffs) {
            if d.diff.is_some() {
                r.found.clone_from(&d.now);
            }
        }
        dropped
    }

    /// The files the changed lookups found, each once (a lookup has its
    /// own copy): old and new contents, how many lookups found that copy,
    /// and the name looked up; or why the edit is not one of lines of
    /// input files.
    fn changed_files(&self, diffs: &[Change]) -> Result<Vec<ChangedFile>, String> {
        let sh = self.shared.borrow();
        let mut files: Vec<ChangedFile> = Vec::new();
        for (i, d) in diffs.iter().enumerate() {
            if d.diff.is_none() {
                continue;
            }
            let name = String::from_utf8_lossy(&sh.reads[i].name).into_owned();
            let (Some((old, new)), Some((was, _)), Some((is, _))) =
                (d.replace.clone(), &sh.reads[i].found, &d.now)
            else {
                return Err(format!("{name} found or lost"));
            };
            if was != is {
                return Err(format!("{name} is found elsewhere"));
            }
            if sh.reads[i].kind != Query::File(FileKind::Tex) {
                return Err(format!("{name} is not TeX input"));
            }
            if !files.iter().any(|f| Arc::ptr_eq(&f.0, &old)) {
                let lookups = sh
                    .reads
                    .iter()
                    .filter(|r| r.found.as_ref().is_some_and(|(_, a)| Arc::ptr_eq(a, &old)))
                    .count();
                files.push((old, new, lookups, name));
            }
        }
        Ok(files)
    }

    /// Events and checkpoints name a file by its contents' address: a copy
    /// of old contents at another address (a checkpoint, or the events of
    /// a build spliced in, that got the file from another lookup) would
    /// neither be checked nor move to the new contents. Where one is, if
    /// anywhere. (A file a lookup found unchanged is another file,
    /// whatever its contents.)
    fn stray_copy(&self, diffs: &[Change], files: &[ChangedFile]) -> Option<&'static str> {
        let sh = self.shared.borrow();
        let unchanged: Vec<&Arc<[u8]>> = sh
            .reads
            .iter()
            .zip(diffs)
            .filter(|(_, d)| d.diff.is_none())
            .filter_map(|(r, _)| r.found.as_ref().map(|(_, a)| a))
            .collect();
        let stray = |a: &Arc<[u8]>| {
            !files.iter().any(|f| Arc::ptr_eq(&f.0, a))
                && files
                    .iter()
                    .any(|f| f.0.len() == a.len() && f.0[..] == a[..])
                && !unchanged.iter().any(|u| Arc::ptr_eq(u, a))
        };
        if self
            .checkpoints
            .iter()
            .any(|(_, cp)| cp.open_inputs().iter().any(|(d, _)| stray(d)))
        {
            Some("a checkpoint")
        } else if self.tracker.pins().iter().any(stray) {
            Some("the lines read")
        } else if sh
            .reads
            .iter()
            .any(|r| r.found.as_ref().is_some_and(|(_, a)| stray(a)))
        {
            Some("the lookups")
        } else {
            None
        }
    }

    /// Backdating at the level of values: each interval between kept
    /// checkpoints that looked up one of the files `others` other than to
    /// read it by lines, run again from its checkpoint with the files as
    /// they are now (`maps`: old, new, and where lines moved), must reach
    /// the next checkpoint, its files moved too, in the same state, having
    /// logged the same lookups (but for the new contents), outputs,
    /// terminal lines, pages and diagnostics. The number of intervals run
    /// and, as that run met them, checkpoints just before and after the
    /// lookups (the next time only the stretch between them runs again;
    /// their files are those of `targets`, what each lookup finds once the
    /// edit is taken), or why not.
    #[allow(clippy::type_complexity)]
    fn backdate_lookups(
        &mut self,
        others: &[Arc<[u8]>],
        maps: &[FileMap],
        targets: &[Option<Arc<[u8]>>],
    ) -> Result<(usize, Vec<(u64, Engine)>), String> {
        if others.is_empty() {
            return Ok((0, Vec::new()));
        }
        // the checkpoint before each lookup, and the first and last lookup
        // after it
        let mut ks: std::collections::BTreeMap<usize, (usize, usize)> =
            std::collections::BTreeMap::new();
        {
            let sh = self.shared.borrow();
            for (i, r) in sh.reads.iter().enumerate() {
                if r.found
                    .as_ref()
                    .is_some_and(|(_, a)| others.iter().any(|o| Arc::ptr_eq(a, o)))
                {
                    let k = self
                        .checkpoints
                        .iter()
                        .rposition(|c| c.1.host().mark.reads <= i)
                        .ok_or("a lookup came before the first checkpoint")?;
                    let e = ks.entry(k).or_insert((i, i));
                    e.1 = i;
                }
            }
        }
        let moved = |cp: &Engine| {
            let mut cp = cp.clone();
            maps.iter()
                .all(|(old, new, map)| cp.remap_input(old, new, |p| map.get(&p).copied()))
                .then_some(cp)
        };
        let saved = self.shared.borrow().logs();
        let mut result = Ok(ks.len());
        let mut met = Vec::new();
        for (&k, &(lo, hi)) in &ks {
            let Some(next) = self.checkpoints.get(k + 1) else {
                result = Err("a lookup came after the last checkpoint".to_owned());
                break;
            };
            let at = next.0;
            let (Some(mut tex), Some(target)) = (moved(&self.checkpoints[k].1), moved(&next.1))
            else {
                result = Err("a checkpoint is inside a changed line".to_owned());
                break;
            };
            let mark = self.checkpoints[k].1.host().mark.clone();
            let end = next.1.host().mark.clone();
            // (from the logs as they were, whatever an earlier rerun left)
            self.shared.borrow_mut().set_logs(saved.prefix(&mark));
            crate::intervals::clear(&self.tracker);
            let t = Instant::now();
            let mut step = tex.resume();
            let (mut before, mut after) = (None, None);
            while step == Step::Checkpoint && tex.commands() < at {
                let reads = self.shared.borrow().reads.len();
                if reads <= lo {
                    before = Some((tex.commands(), tex.clone()));
                } else if reads > hi && after.is_none() {
                    after = Some((tex.commands(), tex.clone()));
                }
                step = tex.resume();
            }
            let same_state = step == Step::Checkpoint
                && tex.commands() == at
                && tex.position_key() == target.position_key()
                && hash_of(&tex) == hash_of(&target);
            let same_logs = same_state && {
                let changed: Vec<OldNew> = maps
                    .iter()
                    .map(|(o, n, _)| (o.clone(), n.clone()))
                    .collect();
                // (put back: the checkpoints kept read the lookups it made)
                let now = self.shared.borrow_mut().take_logs();
                let same = now.same_as(&saved, &end, &changed);
                self.shared.borrow_mut().set_logs(now);
                same
            };
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!(
                    "partex: lookup interval {}..{at} run again: {} ({:.1} ms)",
                    self.checkpoints[k].0,
                    match (same_state, same_logs) {
                        (true, true) => "the same",
                        (true, false) => "the same state, other effects",
                        _ => "differs",
                    },
                    t.elapsed().as_secs_f64() * 1e3
                );
                if !same_state && step == Step::Checkpoint {
                    eprintln!("partex:   {}", state_difference(&tex, &target));
                }
            }
            if !same_logs {
                result = Err(format!(
                    "the interval at command {} that looked a file up ran otherwise",
                    self.checkpoints[k].0
                ));
                break;
            }
            met.extend(self.adopt(before.into_iter().chain(after), mark.reads, targets));
        }
        crate::intervals::clear(&self.tracker);
        self.shared.borrow_mut().set_logs(saved);
        result.map(|n| (n, met))
    }

    /// `PARTEX_CHECK_OFFSETS`: every PDF object `tex` has at a byte offset
    /// must begin there in the PDF file as logged (a stale offset table
    /// writes a cross-reference table that points elsewhere).
    fn check_offsets(&self, tex: &Engine, what: &str) {
        let Some(pdf) = tex.output_files().pdf else {
            return;
        };
        let sh = self.shared.borrow();
        let Some(out) = sh.outputs.get(pdf.0 as usize) else {
            return;
        };
        for (k, off) in tex.pdf_object_offsets() {
            let Ok(at) = usize::try_from(off) else {
                continue;
            };
            if at >= out.bytes.len() {
                continue;
            }
            let want = format!("{k} 0 obj");
            if !out.bytes[at..].starts_with(want.as_bytes()) {
                eprintln!(
                    "partex: {what} at command {}: object {k} is not at byte {at} ({} bytes)",
                    tex.commands(),
                    out.bytes.len()
                );
                return;
            }
        }
    }

    /// The intervals from command `start` on, taken out.
    fn intervals_from(&mut self, start: u64) -> Vec<(u64, Interval)> {
        let all = std::mem::take(&mut self.intervals);
        let (kept, later): (Vec<_>, Vec<_>) = all.into_iter().partition(|(at, _)| *at < start);
        self.intervals = kept;
        later
    }

    /// Checkpoints `cps` a run from the lookup numbered `from` on met, with
    /// the files it looked up as the lookups will have them (`targets`).
    fn adopt(
        &self,
        cps: impl Iterator<Item = (u64, Engine)>,
        from: usize,
        targets: &[Option<Arc<[u8]>>],
    ) -> Vec<(u64, Engine)> {
        let sh = self.shared.borrow();
        let same: Vec<OldNew> = sh.reads[from..]
            .iter()
            .zip(&targets[from..])
            .filter_map(|(r, t)| Some((r.found.as_ref()?.1.clone(), t.clone()?)))
            .filter(|(a, b)| !Arc::ptr_eq(a, b) && a[..] == b[..])
            .collect();
        cps.map(|(at, mut cp)| {
            for (a, b) in &same {
                cp.replace_input(a, b);
            }
            // (what it wrote belongs to the intervals run from it)
            let _ = cp.take_objstms_written();
            (at, cp)
        })
        .collect()
    }

    /// Files open at a checkpoint continue with their new contents (the
    /// part read so far is the same); in those not loaded yet, once they
    /// are.
    fn replace_inputs(&mut self, diffs: &[Change]) {
        for (_, cp) in &mut self.checkpoints {
            for d in diffs {
                if let (Some(_), Some((old, new))) = (d.diff, &d.replace) {
                    cp.replace_input(old, new);
                }
            }
        }
        if let Some(saved) = &mut self.saved {
            for d in diffs {
                if let (Some(_), Some((old, new))) = (d.diff, &d.replace) {
                    for c in &mut saved.checkpoints {
                        for (data, _) in &mut c.open {
                            if Arc::ptr_eq(data, old) {
                                data.clone_from(new);
                            }
                        }
                    }
                    saved.replaced.push((old.clone(), new.clone()));
                }
            }
        }
    }

    /// Run until the job ends, keeping checkpoints, or until it reaches
    /// a state of the `previous` build (then its outputs are reused): the
    /// history, the number of commands of the job, and where it was cut
    /// off.
    fn drive(
        &mut self,
        mut tex: Engine,
        mut step: Step,
        mut previous: Option<Previous>,
    ) -> (i32, u64, Option<u64>) {
        let mut cut = None;
        self.t_drive = Instant::now();
        self.skipped = 0;
        loop {
            match step {
                Step::Checkpoint => {
                    let mut interval = crate::intervals::take(&self.tracker);
                    interval.to = tex.commands();
                    interval.chars = tex.take_chars_shipped();
                    interval.objstms = tex.take_objstms_written();
                    let recorded = interval.recorded;
                    self.written_since.extend(eqtb_locations(&interval.written));
                    self.intervals.push((self.interval_from, interval));
                    self.interval_from = tex.commands();
                    let written = recorded.then_some(&self.written_since);
                    // (a PDF splice can stop before the previous build
                    // reads what differs: `PARTEX_PARTIAL_SPLICE=0`, not)
                    let partial =
                        tex.output_files().pdf.is_some() && switch_on("PARTEX_PARTIAL_SPLICE");
                    let met = previous
                        .as_mut()
                        .and_then(|p| Self::converged(&tex, p, written, partial));
                    if let Some(b) = previous.as_mut().and_then(|p| p.blocked.take()) {
                        self.why.push(b);
                    }
                    self.keep_due(&mut tex, met.is_some());
                    if let Some(p) = previous.as_mut()
                        && let Some(j) = met
                    {
                        let at = tex.commands();
                        if tex.output_files().pdf.is_none() {
                            if self.splice(&tex, p, j, None) {
                                let rest = p.total_commands - p.checkpoints[j].0;
                                self.skipped += rest;
                                return (p.history, at + rest, Some(at));
                            }
                        } else if let Some(last) = self.splice_limit(p, j)
                            && self.splice(&tex, p, j, Some(last))
                        {
                            // A PDF file ends with offsets of everything in
                            // it: go on from the previous build's last
                            // checkpoint, moved, and let the job write them.
                            // Or from the last before it reads what differs
                            // (or an output this run wrote otherwise), and
                            // meet it again after (DESIGN.md §7.4).
                            let (moved_to, cp) = self.checkpoints.last().expect("kept");
                            self.skipped += moved_to - at;
                            self.dense_left = 0;
                            cut.get_or_insert((at, *moved_to));
                            debug_time("run to the cut and splice", self.t_drive);
                            tex = cp.clone();
                            tex.set_dense(false);
                            if std::env::var_os("PARTEX_CHECK_OFFSETS").is_some() {
                                self.check_offsets(&tex, "spliced");
                            }
                            crate::intervals::clear(&self.tracker);
                            self.interval_from = *moved_to;
                            if !partial || last + 1 >= p.checkpoints.len() {
                                previous = None;
                            }
                        }
                    }
                    let _p = crate::timeline::phase("interval");
                    step = tex.resume();
                }
                Step::Finished(history) => {
                    debug_time("run to the end", self.t_drive);
                    // the tail after the last checkpoint: read-set cutoff
                    // needs what the whole rest of a build read
                    let mut interval = crate::intervals::take(&self.tracker);
                    interval.to = tex.commands();
                    interval.chars = tex.take_chars_shipped();
                    interval.objstms = tex.take_objstms_written();
                    self.intervals.push((self.interval_from, interval));
                    if previous.as_ref().is_some_and(|p| !p.blocked_seen) {
                        self.why.push(
                            "no cutoff: every later checkpoint comes before a changed read"
                                .to_owned(),
                        );
                    }
                    if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                        let n = self.checkpoints.len();
                        for (at, cp) in &self.checkpoints[n.saturating_sub(4)..] {
                            let open: Vec<_> =
                                cp.open_inputs().iter().map(|(d, p)| d.len() - p).collect();
                            eprintln!(
                                "partex: kept checkpoint at {at}, open files with bytes left {open:?}"
                            );
                        }
                        eprintln!("partex: the job ended at {}", tex.commands());
                    }
                    // (with a cut, the commands between it and where the
                    // job went on were not run)
                    return (history, tex.commands(), cut.map(|(at, _)| at));
                }
            }
        }
    }

    /// Keep a checkpoint of `tex`, thinning the kept ones to the budget.
    fn keep(&mut self, tex: &mut Engine) {
        let _p = crate::timeline::phase("checkpoint").map(|mut p| {
            p.detail(format!("at command {}", tex.commands()));
            p
        });
        let mut cp = tex.snapshot();
        if std::env::var_os("PARTEX_CHECK_OFFSETS").is_some() {
            self.check_offsets(tex, "kept");
        }
        if std::env::var_os("PARTEX_PERSIST_CHECK").is_some() {
            // (keep the state as loaded back from its saved form, so
            // resuming from it is tested too)
            let seed: Vec<Arc<[u8]>> = self
                .shared
                .borrow()
                .reads
                .iter()
                .filter_map(|r| r.found.as_ref().map(|f| f.1.clone()))
                .collect();
            match cp.round_trip(&seed) {
                Some(t) if t.state_hash() == cp.state_hash() => cp = t,
                Some(t) => {
                    let (a, b) = (t.state_hash_parts(), cp.state_hash_parts());
                    let parts: Vec<_> = a
                        .iter()
                        .zip(&b)
                        .filter(|(x, y)| x.1 != y.1)
                        .map(|(x, _)| x.0)
                        .collect();
                    explain_difference(&t, &cp, None);
                    panic!(
                        "checkpoint at {}: loaded state differs in {}",
                        tex.commands(),
                        parts.join(", ")
                    );
                }
                None => panic!(
                    "checkpoint at {}: cannot be saved or loaded",
                    tex.commands()
                ),
            }
        }
        if std::env::var_os("PARTEX_CLONE_STATS").is_some()
            && let Some((_, last)) = self.checkpoints.last()
        {
            // (`PARTEX_CLONE_STATS`: what a checkpoint does not share with
            // the one before, by part)
            let mut s = Saver::with_sizes();
            last.save_state(&mut s);
            let before: HashMap<_, _> = s.sizes().into_iter().collect();
            cp.save_state(&mut s);
            let mut more: Vec<_> = s
                .sizes()
                .into_iter()
                .map(|(n, b)| (b - before.get(n).copied().unwrap_or(0), n))
                .collect();
            more.sort_unstable_by(|a, b| b.cmp(a));
            let total: usize = more.iter().map(|m| m.0).sum();
            eprintln!(
                "partex: checkpoint at {}: {total} bytes unshared: {:?}",
                tex.commands(),
                &more[..more.len().min(8)]
            );
        }
        self.checkpoints.push((tex.commands(), cp));
        if self.checkpoints.len() > self.budget && switch_on("PARTEX_EVEN_CHECKPOINTS") {
            // Evenly: drop the one whose neighbours are closest, never the
            // first or the last (a splice into a PDF file runs on from it).
            // (Thinning to a growing spacing left 55 of 256 on a 295-page
            // book, a text edit replaying ten pages.)
            while self.checkpoints.len() > self.budget.max(2) {
                let n = self.checkpoints.len();
                let Some(i) = (1..n - 1)
                    .filter(|&i| {
                        let at = self.checkpoints[i].0;
                        !self.pinned.contains(&at) && !self.dense_pins.contains(&at)
                    })
                    .min_by_key(|&i| self.checkpoints[i + 1].0 - self.checkpoints[i - 1].0)
                else {
                    break;
                };
                self.checkpoints.remove(i);
            }
            // (new ones closer than half the even spacing are not kept)
            let span = self.checkpoints.last().map_or(0, |c| c.0) - self.checkpoints[0].0;
            self.spacing = span / u64::try_from(2 * self.budget).unwrap_or(1);
        } else if self.checkpoints.len() > self.budget {
            let span = self.checkpoints.last().map_or(0, |c| c.0) - self.checkpoints[0].0;
            let even = span / u64::try_from(self.budget).unwrap_or(1);
            self.spacing = (self.spacing * 3 / 2).max(even).max(1);
            let (mut last, end) = (None, self.checkpoints.len() - 1);
            let mut i = 0;
            self.checkpoints.retain(|(at, _)| {
                // (and the last: a splice into a PDF file runs on from it)
                let keep = last.is_none_or(|l| at - l >= self.spacing) || i == end;
                if keep {
                    last = Some(*at);
                }
                i += 1;
                keep
            });
        }
    }

    /// Resume points between paragraphs where this rebuild runs again,
    /// until it meets the previous build: the next edit near this one
    /// resumes from the paragraph (`Tex::set_dense`;
    /// `PARTEX_DENSE_CHECKPOINTS=0`: none). The last rebuild's go.
    fn start_dense(&mut self) {
        self.dense_pins.clear();
        self.dense_left = if switch_on("PARTEX_DENSE_CHECKPOINTS") {
            48
        } else {
            0
        };
    }

    /// At a checkpoint `tex` stopped at: keep it if it met the previous
    /// build, is at a file boundary or between paragraphs where this
    /// rebuild runs again (both pinned), or is far enough from the last.
    fn keep_due(&mut self, tex: &mut Engine, met: bool) {
        let last = self.checkpoints.last().map_or(0, |c| c.0);
        let boundary = self.file_boundary(tex);
        let dense = tex.dense_checkpoint() && self.dense_left > 0;
        if met || boundary || dense || tex.commands().saturating_sub(last) >= self.spacing {
            self.keep(tex);
            if boundary {
                self.pinned.insert(tex.commands());
            }
            if dense {
                self.dense_left -= 1;
                self.dense_pins.insert(tex.commands());
            }
        }
    }

    /// Whether `tex`, at a due checkpoint, is where one of the document's
    /// own input files (found in the job's directory, not the TeX tree)
    /// began or ended since the last: a checkpoint kept there, and kept by
    /// thinning, leaves an edit of that file only the few commands around
    /// its lookups to run again when backdating them (`\include` looks a
    /// chapter up by `\pdffilesize` between two chapters; a whole interval
    /// between evenly spaced checkpoints ran 150,000 commands). Not in the
    /// preamble's packages (`PARTEX_FILE_CHECKPOINTS=0`: never).
    fn file_boundary(&mut self, tex: &Engine) -> bool {
        if !switch_on("PARTEX_FILE_CHECKPOINTS") {
            return false;
        }
        let open: Vec<usize> = tex
            .open_inputs()
            .iter()
            .map(|(d, _)| crate::intervals::file_id(d))
            .collect();
        if open == self.open_at_step {
            return false;
        }
        let before = std::mem::replace(&mut self.open_at_step, open);
        let sh = self.shared.borrow();
        let local = |id: &usize| {
            sh.reads.iter().any(|r| {
                r.found.as_ref().is_some_and(|(path, data)| {
                    crate::intervals::file_id(data) == *id
                        && r.kind == Query::File(FileKind::Tex)
                        && !path.starts_with(b"/")
                })
            })
        };
        let now = &self.open_at_step;
        now.iter().filter(|f| !before.contains(f)).any(local)
            || before.iter().filter(|f| !now.contains(f)).any(local)
    }

    /// A checkpoint of the previous build whose state `tex` is in, with
    /// the same input left to read, or whose state differs from it only in
    /// cells the rest of the previous build never reads (read-set cutoff;
    /// `written`: the cells this build wrote since it resumed, if
    /// recorded; `partial`: the rest of the previous build may read them,
    /// its part before the first interval that does being joined, see
    /// `p.stop`).
    fn converged(
        tex: &Engine,
        p: &mut Previous,
        written: Option<&HashSet<i32>>,
        partial: bool,
    ) -> Option<usize> {
        let key = tex.position_key();
        p.stop = None;
        let mut input_hash = None;
        // (`PARTEX_WATCH_DEBUG` also explains where no cutoff is allowed;
        // it never changes a decision)
        let debug = p.explained < 40 && std::env::var_os("PARTEX_WATCH_DEBUG").is_some();
        if debug && !p.checkpoints.iter().any(|c| c.1.position_key() == key) {
            let near = p
                .checkpoints
                .iter()
                .min_by_key(|c| c.0.abs_diff(tex.commands()))
                .map(|c| (c.0, c.1.position_key()));
            eprintln!(
                "partex: at command {} no checkpoint is where this run is: {key:?}; nearest {near:?}",
                tex.commands()
            );
        }
        for j in 0..p.checkpoints.len() {
            let old = &p.checkpoints[j].1;
            if old.position_key() != key {
                continue;
            }
            // everything the previous build read from there on must be
            // unchanged, and the open files must go on the same way
            let allowed = old.host().mark.reads >= p.unchanged_from
                && tex
                    .open_inputs()
                    .iter()
                    .zip(old.open_inputs())
                    .all(|((a, i), (b, j))| a[*i..] == b[j..]);
            if allowed {
                // (the input differs: so does the state, without hashing it)
                let input = *input_hash.get_or_insert_with(|| tex.input_hash());
                let decision =
                    if *p.input_hashes[j].get_or_insert_with(|| old.input_hash()) != input {
                        Err(input_difference(tex, old))
                    } else if let Some(written) = written {
                        // only cells either build wrote since the checkpoint
                        // resumed from can differ (`cheap_cutoff`)
                        match cheap_cutoff(tex, p, j, written, partial) {
                            None => continue,
                            Some(r) => r,
                        }
                    } else if p.hashes[j]
                        .get_or_insert_with(|| (hash_of(old), Vec::new()))
                        .0
                        == hash_of(tex)
                    {
                        Ok(Differs::default())
                    } else {
                        Err(format!(
                            "the state differs in {}",
                            state_difference(tex, old)
                        ))
                    };
                match decision {
                    Ok(d) => {
                        if !d.is_empty() {
                            p.blocked = Some(cutoff_note(tex, &d, p.stop));
                            p.blocked_seen = true;
                            p.overlay = Some((tex.clone(), d));
                            *p.objstm_rewrites.borrow_mut() = None;
                        }
                        return Some(j);
                    }
                    Err(why) => refuse(tex, p, &why),
                }
            }
            if debug {
                p.explained += 1;
                if !allowed {
                    eprintln!("partex: (no cutoff allowed here: a later read changed)");
                }
                let (at, old) = (p.checkpoints[j].0, &p.checkpoints[j].1);
                let interval = p.intervals.iter().find(|(f, _)| *f == at).map(|(_, iv)| iv);
                explain_difference(tex, old, interval);
            }
        }
        None
    }

    /// How far a PDF splice at the previous build's checkpoint `j` goes:
    /// as far as `read_back_limit` says, and only to its last checkpoint
    /// before the first interval that reads what differs (`p.stop`);
    /// `None` if not past `j`, or if the checkpoint there cannot take what
    /// differs.
    fn splice_limit(&self, p: &Previous, j: usize) -> Option<usize> {
        let mut last = self.read_back_limit(p, j)?;
        if let Some(r) = p.stop {
            last = (j..=last).rev().find(|&k| p.checkpoints[k].0 <= r)?;
        }
        // (and before it writes the object stream the rebuild's objects
        // wait in, unless that stream can be rendered again)
        if let Some(o) = p.overlay.as_ref().and_then(|(_, d)| d.objstm.as_ref())
            && Self::objstm_rewrite(p, j, last).is_none()
        {
            last = (j..=last)
                .take_while(|&k| k == j || p.checkpoints[k].1.objstm_fits(o))
                .last()?;
        }
        if last <= j {
            return None;
        }
        // (the checkpoint the job goes on from must take it)
        if let Some((from, d)) = &p.overlay {
            let mut cp = p.checkpoints[last].1.clone();
            if !overlay_at(p, j, last, d).apply(&mut cp, from) {
                return None;
            }
        }
        Some(last)
    }

    /// The object stream the rebuild's waiting objects go to, if the
    /// previous build wrote it between its checkpoints `j` and `last`:
    /// the record, and the object's bytes rendered again with those
    /// objects (packing and compression are output: DESIGN.md §7.4 item
    /// 7). `None` if it wrote none there, or if rendering the old record
    /// does not give the old bytes (`PARTEX_OBJSTM_LINK=0`: always).
    fn objstm_rewrite(p: &Previous, j: usize, last: usize) -> Option<ObjStmRewrite> {
        // (the splice asks again what its limit asked: rendered once)
        if let Some((key, r)) = &*p.objstm_rewrites.borrow()
            && *key == (j, last)
        {
            return r.clone();
        }
        let r = Self::objstm_rewrite_now(p, j, last);
        *p.objstm_rewrites.borrow_mut() = Some(((j, last), r.clone()));
        r
    }

    fn objstm_rewrite_now(p: &Previous, j: usize, last: usize) -> Option<ObjStmRewrite> {
        let (_, d) = p.overlay.as_ref()?;
        let o = d.objstm.as_ref()?;
        let (from, to) = (p.checkpoints[j].0, p.checkpoints[last].0);
        let rec = p
            .intervals
            .iter()
            .filter(|(at, iv)| *at >= from && iv.to <= to)
            .flat_map(|(_, iv)| &iv.objstms)
            .find(|r| r.num == o.cur())?;
        let pdf = p.checkpoints[j].1.output_files().pdf?.0 as usize;
        let old = p
            .outputs
            .get(pdf)?
            .bytes
            .get(usize::try_from(rec.begin).ok()?..usize::try_from(rec.end).ok()?)?;
        let deflate = |level: i32, data: &[u8]| crate::zlib::deflate_stream(level, data);
        if rec.render(deflate) != old {
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!("partex: object stream {} rendered otherwise", rec.num);
            }
            return None;
        }
        let new = rec.with_differ(o)?;
        let bytes = new.render(deflate);
        Some(Arc::new((rec.clone(), new, bytes)))
    }

    /// How far a PDF splice at the previous build's checkpoint `j` can go:
    /// its last checkpoint, or, if an output this run has written differs
    /// from the previous build's so far (a `\label` edit changes the
    /// `.aux` file), its last checkpoint before it reads that output back
    /// (LaTeX's `\enddocument` inputs the `.aux` file): the job goes on
    /// from there, reading what this run wrote.
    fn read_back_limit(&self, p: &Previous, j: usize) -> Option<usize> {
        let last = p.checkpoints.len().checked_sub(1)?;
        let mark = &p.checkpoints[j].1.host().mark;
        let sh = self.shared.borrow();
        let differs: Vec<&[u8]> = sh
            .outputs
            .iter()
            .zip(&p.outputs)
            .zip(&mark.outputs)
            .filter(|((now, prev), cut)| {
                prev.bytes.get(..**cut).is_none_or(|b| *b != now.bytes[..])
            })
            .map(|((_, prev), _)| &prev.name[..])
            .collect();
        if differs.is_empty() {
            return Some(last);
        }
        let Some(r) = p
            .reads
            .iter()
            .enumerate()
            .skip(mark.reads)
            .find(|(_, r)| differs.iter().any(|n| reads_output(r, n)))
            .map(|(i, _)| i)
        else {
            return Some(last);
        };
        (j..=last)
            .rev()
            .find(|&k| p.checkpoints[k].1.host().mark.reads <= r)
    }

    /// Finish the job with the previous build's outputs from its
    /// checkpoint `j` on, after what this run has written, and keep its
    /// later checkpoints. False (and nothing changed) if they cannot be
    /// joined.
    ///
    /// The DVI file's later pages are written again, by this run's
    /// writer: where TeX's buffer is flushed decides some of their bytes
    /// (§611, §601), and that moves with the length of the file.
    /// (`upto`: splice only as far as the previous build's checkpoint
    /// `upto`, not to its end; its later checkpoints are dropped.)
    #[allow(
        clippy::too_many_lines,
        reason = "one pass over every output, in order"
    )]
    fn splice(&mut self, tex: &Engine, p: &mut Previous, j: usize, upto: Option<usize>) -> bool {
        let _p = crate::timeline::phase("splice");
        let old = &p.checkpoints[j].1;
        let mark = old.host().mark.clone();
        let old_gone = old.output_written();
        let (nu, ou) = (tex.output_files(), old.output_files());
        let dvi = nu.dvi.map(|d| d.0 as usize);
        let pdf = nu.pdf.map(|d| d.0 as usize);
        if ou.dvi.map(|d| d.0 as usize) != dvi
            || ou.pdf.map(|d| d.0 as usize) != pdf
            || (upto.is_some() && dvi.is_some())
        {
            return false;
        }
        let end = upto.map(|l| p.checkpoints[l].1.host().mark.clone());
        // (with `upto` before the last, the previous build stays whole: the
        // job may meet it again)
        let last = upto.unwrap_or(p.checkpoints.len() - 1);
        let partial = last + 1 < p.checkpoints.len();
        // the later checkpoints kept, by how many pages they are past `j`
        let kept: Vec<usize> = (j + 1..=last).collect();
        let pages_at = |k: usize| p.checkpoints[k].1.host().mark.pages - mark.pages;
        let later = mark.pages - p.pages_from; // (in `p.pages`)
        let mut sh = self.shared.borrow_mut();
        if sh.outputs.len() != mark.outputs.len()
            || sh
                .outputs
                .iter()
                .zip(&p.outputs)
                .any(|(a, b)| a.name != b.name)
        {
            return false;
        }
        let mut outputs = Vec::with_capacity(p.outputs.len());
        // for each output this run has: where the old and the new bytes
        // after the cut begin
        let mut cuts = Vec::new();
        let mut dvi_lengths = None;
        // the writer after `n` pages, and the file's length then
        let mut writers = HashMap::new();
        let upto_len = |i: usize, b: &[u8]| {
            end.as_ref().map_or(b.len(), |e| {
                e.outputs.get(i).copied().unwrap_or(0).min(b.len())
            })
        };
        let n_outputs = end.as_ref().map_or(p.outputs.len(), |e| e.outputs.len());
        // (an object stream the previous build wrote in the part joined,
        // rendered again with this run's objects)
        let rewrite = upto.and_then(|l| Self::objstm_rewrite(p, j, l));
        let mut objstm = None;
        for (i, prev) in p.outputs.iter().enumerate().take(n_outputs) {
            let Some(now) = sh.outputs.get(i) else {
                // opened after the checkpoint
                outputs.push(Output {
                    name: prev.name.clone(),
                    bytes: prev.bytes[..upto_len(i, &prev.bytes)].to_vec(),
                });
                continue;
            };
            let mut bytes = now.bytes.clone();
            if dvi == Some(i) {
                let wanted = kept.iter().map(|&k| pages_at(k)).collect();
                let Some(w) = tex.dvi_writer() else {
                    return false;
                };
                let Some(snapshots) =
                    rewrite_dvi(w, &mut bytes, &p.pages[later..], &prev.bytes, &wanted)
                else {
                    return false;
                };
                writers = snapshots;
                dvi_lengths = Some((prev.bytes.len(), bytes.len()));
                cuts.push(None);
            } else {
                let cut = mark.outputs[i];
                // a file this run reads back must say the same so far (not
                // the log, nor the PDF file, whose offsets move): one that
                // differs is joined only if the previous build's part
                // spliced in never reads it
                if nu.log.map(|l| l.0 as usize) != Some(i)
                    && pdf != Some(i)
                    && prev.bytes.get(..cut).is_none_or(|b| *b != bytes[..])
                {
                    let reads_end = end.as_ref().map_or(p.reads.len(), |e| e.reads);
                    let from = mark.reads.min(reads_end);
                    if p.reads[from..reads_end.min(p.reads.len())]
                        .iter()
                        .any(|r| reads_output(r, &prev.name))
                    {
                        return false;
                    }
                }
                cuts.push(Some((cut, bytes.len())));
                let stop = upto_len(i, &prev.bytes);
                let is = bytes.len();
                bytes.extend_from_slice(prev.bytes.get(cut..stop).unwrap_or(&[]));
                if pdf == Some(i)
                    && let Some(rw) = &rewrite
                    && let (rec, newrec, new) = &**rw
                {
                    let (Ok(b), Ok(e)) = (usize::try_from(rec.begin), usize::try_from(rec.end))
                    else {
                        return false;
                    };
                    if b < cut || e > stop {
                        return false;
                    }
                    bytes.splice(b - cut + is..e - cut + is, new.iter().copied());
                    let longer = i64::try_from(new.len()).unwrap_or(0) - (rec.end - rec.begin);
                    let delta = i64::try_from(is).unwrap_or(0) - i64::try_from(cut).unwrap_or(0);
                    let mut now = newrec.clone();
                    now.begin = rec.begin + delta;
                    now.end = rec.end + delta + longer;
                    objstm = Some((now, rec.end, longer));
                }
            }
            outputs.push(Output {
                name: prev.name.clone(),
                bytes,
            });
        }
        let mut term = sh.term.clone();
        let term_cut = (mark.term, term.len());
        let term_end = end.as_ref().map_or(p.term.len(), |e| e.term);
        term.extend_from_slice(p.term.get(term_cut.0..term_end).unwrap_or(&[]));
        // The last lines say how long the DVI file is.
        if let Some((was, is)) = dvi_lengths
            && was != is
        {
            let log = nu.log.map(|l| l.0 as usize);
            let fixed = log.is_none_or(|l| patch_byte_count(&mut outputs[l].bytes, was, is))
                && patch_byte_count(&mut term, was, is);
            if !fixed {
                return false;
            }
        }
        let (sh_reads_at, pages_now) = (sh.reads.len(), sh.pages.len());
        let diags_at = sh.diags.len();
        let diags_end = end.as_ref().map_or(p.diags.len(), |e| e.diags);
        sh.diags
            .extend_from_slice(p.diags.get(mark.diags..diags_end).unwrap_or(&[]));
        sh.outputs = outputs;
        sh.term = term;
        let reads_end = end.as_ref().map_or(p.reads.len(), |e| e.reads);
        let tail_reads = p.reads.get(mark.reads..reads_end).unwrap_or(&[]).to_vec();
        sh.reads.extend(tail_reads);
        let pages_end = end
            .as_ref()
            .map_or(p.pages.len(), |e| e.pages - p.pages_from);
        if partial {
            sh.pages.extend_from_slice(&p.pages[later..pages_end]);
        } else {
            sh.pages.extend(p.pages.drain(later..pages_end));
        }
        drop(sh);

        let moves = Moves {
            mark,
            commands: tex.commands(),
            reads: sh_reads_at,
            pages: pages_now,
            diags: diags_at,
            term: term_cut,
            outputs: cuts,
            dvi,
            pdf,
            writers,
            objstm,
        };
        self.keep_later(p, (j, upto), &moves, tex, old_gone);
        true
    }

    /// Keep the previous build's checkpoints after `j` (up to `upto`, the
    /// job going on from there; the previous build then stays whole if
    /// that is not its last): they go on from where their outputs have
    /// moved to (`moves`).
    /// (`met` met the previous build at `j`, whose PDF writer was at
    /// `old_gone` written: the kept checkpoints' offsets follow.)
    #[allow(clippy::too_many_lines, reason = "one pass over the kept checkpoints")]
    fn keep_later(
        &mut self,
        p: &mut Previous,
        (j, upto): (usize, Option<usize>),
        moves: &Moves,
        met: &Engine,
        old_gone: i64,
    ) {
        let mark = &moves.mark;
        let was_at = p.checkpoints[j].0;
        // The files open where the builds met, as this build got them: the
        // previous build's checkpoints and line events have the same
        // contents at other addresses, which would keep token-level
        // dependencies (`tokendeps.rs`) from knowing them for one file.
        let same_files: Vec<OldNew> = if self.tracker.follows_lines() {
            p.checkpoints[j]
                .1
                .open_inputs()
                .into_iter()
                .zip(met.open_inputs())
                .filter(|((a, _), (b, _))| !Arc::ptr_eq(a, b) && a[..] == b[..])
                .map(|((a, _), (b, _))| (a, b))
                .collect()
        } else {
            Vec::new()
        };
        // (read-set cutoff: what each later interval wrote, which a later
        // checkpoint then holds as the previous build had it)
        let overlay = p.overlay.take();
        *p.objstm_rewrites.borrow_mut() = None;
        let last = upto.unwrap_or(p.checkpoints.len() - 1);
        let partial = last + 1 < p.checkpoints.len();
        // (the job records its intervals from checkpoint `upto` on)
        let stop_at = upto.map_or(u64::MAX, |l| p.checkpoints[l].0);
        let overlays: Vec<Option<Differs>> = (j + 1..=last)
            .map(|k| overlay.as_ref().map(|(_, d)| overlay_at(p, j, k, d)))
            .collect();
        let intervals = if partial {
            p.intervals.clone()
        } else {
            std::mem::take(&mut p.intervals)
        };
        let delta = met.output_written() - old_gone;
        let extra = moves.objstm.as_ref().map(|(_, at, d)| (*at, *d));
        let more = |x: i64| extra.map_or(0, |(at, d)| if x >= at { d } else { 0 });
        for (at, mut iv) in intervals {
            if at >= was_at && at < stop_at {
                // (object streams: where they are now, and the one
                // rendered again as it is)
                for r in &mut iv.objstms {
                    match &moves.objstm {
                        Some((now, _, _)) if now.num == r.num => r.clone_from(now),
                        _ => {
                            r.begin += delta + more(r.begin);
                            r.end += delta + more(r.end);
                        }
                    }
                }
                iv.to = iv.to - was_at + moves.commands;
                self.intervals.push((at - was_at + moves.commands, iv));
            }
        }
        for (a, b) in &same_files {
            crate::tokendeps::remap(&mut self.intervals, &self.tracker, a, b);
        }
        let taken: Vec<(u64, Engine)> = if partial {
            p.checkpoints[j + 1..=last]
                .iter()
                .map(|(at, cp)| (*at, clone_checkpoint(cp)))
                .collect()
        } else {
            p.checkpoints.drain(j + 1..).collect()
        };
        for (k, (commands, mut cp)) in taken.into_iter().enumerate() {
            if let Some((from, _)) = &overlay
                && let Some(Some(d)) = overlays.get(k)
                && !d.is_empty()
            {
                if !d.apply(&mut cp, from) {
                    continue;
                }
                // (frozen again, its lists shared, if cells or names thawed
                // them; glyphs used and the object stream thaw nothing)
                if !d.cells.is_empty() || d.names.is_some() {
                    cp = clone_checkpoint(&cp);
                }
            }
            let m = &cp.host().mark;
            let pages = m.pages - mark.pages;
            let mut now = Mark {
                reads: m.reads - mark.reads + moves.reads,
                outputs: m.outputs.clone(),
                term: m.term - moves.term.0 + moves.term.1,
                pages: pages + moves.pages,
                diags: m.diags - mark.diags + moves.diags,
            };
            let mut ok = true;
            for (i, c) in moves.outputs.iter().enumerate() {
                match c {
                    Some((was, is)) => {
                        let past = extra.filter(|_| moves.pdf == Some(i)).map_or(0, |(at, d)| {
                            if i64::try_from(now.outputs[i]).unwrap_or(0) >= at {
                                d
                            } else {
                                0
                            }
                        });
                        now.outputs[i] = usize::try_from(
                            i64::try_from(now.outputs[i] - was + is).unwrap_or(0) + past,
                        )
                        .unwrap_or(0);
                    }
                    None if moves.dvi == Some(i) => match moves.writers.get(&pages) {
                        Some((w, len)) => {
                            cp.set_dvi_writer(w.clone());
                            now.outputs[i] = *len;
                        }
                        None => ok = false,
                    },
                    None => {}
                }
            }
            if !ok {
                continue;
            }
            // (their files changed before where they are)
            for (was, is) in &p.replaced {
                cp.replace_input_before(was, is);
            }
            for (a, b) in &same_files {
                cp.replace_input(a, b);
            }
            cp.relocate_output_past(met, old_gone, extra);
            cp.host_mut().mark = now;
            if std::env::var_os("PARTEX_CHECK_OFFSETS").is_some() {
                self.check_offsets(&cp, "moved");
            }
            self.checkpoints
                .push((commands - was_at + moves.commands, cp));
        }
    }

    /// Every logged lookup, done again.
    fn changes(&self) -> Vec<Change> {
        let mut sh = self.shared.borrow_mut();
        let mut seen: HashMap<(Vec<u8>, u8), Option<Found>> = HashMap::new();
        let mut out = Vec::with_capacity(sh.reads.len());
        // (a file found where it was, with the stamp it had when its
        // contents were last found the same, is unchanged: its contents
        // are not read again; each lookup keeps its own copy.
        // `PARTEX_STAT_CACHE=0`: every file is read)
        let stat_cache = switch_on("PARTEX_STAT_CACHE");
        let before = std::mem::take(&mut sh.verified);
        let mut verified = Verified::new();
        let mut located: HashMap<(Vec<u8>, u8), Option<Vec<u8>>> = HashMap::new();
        for i in 0..sh.reads.len() {
            let (name, kind) = (sh.reads[i].name.clone(), sh.reads[i].kind);
            let key = (name.clone(), kind_code(kind));
            if stat_cache
                && let (Query::File(k), Some((p, a))) = (kind, sh.reads[i].found.clone())
                && let Some(st) = crate::native::stamp(&p)
                && before
                    .get(&p)
                    .is_some_and(|v| v.iter().any(|(b, s)| Arc::ptr_eq(&a, b) && *s == st))
                && located
                    .entry(key.clone())
                    .or_insert_with(|| sh.base.locate(&name, k))
                    .as_ref()
                    == Some(&p)
            {
                verified.entry(p.clone()).or_default().push((a.clone(), st));
                out.push(Change {
                    diff: None,
                    now: Some((p, a.clone())),
                    replace: Some((a.clone(), a)),
                });
                continue;
            }
            let st = match (&sh.reads[i].found, stat_cache) {
                (Some((p, _)), true) => crate::native::stamp(p),
                _ => None,
            };
            let now = if let Some(v) = seen.get(&key) {
                v.clone()
            } else {
                // (files this run wrote are not what the next run starts
                // with: look on disk)
                let v = match kind {
                    Query::File(kind) => {
                        sh.base.read_file(&name, kind).map(|f| (f.name, f.contents))
                    }
                    Query::ModDate => sh
                        .base
                        .file_mod_date(&name)
                        .map(|d| (name.clone(), Arc::from(&d[..]))),
                };
                seen.insert(key, v.clone());
                v
            };
            let old = &sh.reads[i].found;
            let diff = match (old, &now) {
                (None, None) => None,
                (Some((pa, a)), Some((pb, b))) if pa == pb || same_file(pa, pb) => first_diff(a, b),
                _ => Some(0),
            };
            let replace = match (old, &now) {
                (Some((_, a)), Some((_, b))) => Some((a.clone(), b.clone())),
                _ => None,
            };
            // (the stamp before it was read: a change since shows)
            if let (None, Some(st), Some((pa, a)), Some((pb, _))) = (diff, st, old, &now)
                && pa == pb
            {
                verified
                    .entry(pa.clone())
                    .or_default()
                    .push((a.clone(), st));
            }
            out.push(Change { diff, now, replace });
        }
        sh.verified = verified;
        out
    }

    /// `PARTEX_WATCH_DEBUG`: which reads changed, and the reads each
    /// checkpoint had seen.
    fn debug_changes(&self, diffs: &[Change]) {
        if std::env::var_os("PARTEX_WATCH_DEBUG").is_none() {
            return;
        }
        let sh = self.shared.borrow();
        for (i, d) in diffs.iter().enumerate() {
            if let Some(at) = d.diff {
                eprintln!(
                    "partex: changed read {i} {} at byte {at} ({:?}, contents at {:?})",
                    String::from_utf8_lossy(&sh.reads[i].name),
                    sh.reads[i].kind,
                    d.replace.as_ref().map(|r| crate::intervals::file_id(&r.0))
                );
            }
        }
        let marks: Vec<usize> = self
            .checkpoints
            .iter()
            .map(|c| c.1.host().mark.reads)
            .collect();
        eprintln!(
            "partex: checkpoint reads {:?}",
            &marks[..marks.len().min(40)]
        );
        eprintln!(
            "partex: {} checkpoints, spacing {} commands, at {:?}",
            self.checkpoints.len(),
            self.spacing,
            self.checkpoints.iter().map(|c| c.0).collect::<Vec<_>>()
        );
    }

    /// The changed reads, and the first of them.
    fn why_changed(&self, diffs: &[Change]) -> Option<String> {
        let sh = self.shared.borrow();
        let mut changed = diffs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.diff.map(|at| (i, at)));
        let (i, at) = changed.next()?;
        Some(format!(
            "{} changed reads, the first of {} at byte {at}",
            1 + changed.count(),
            String::from_utf8_lossy(&sh.reads[i].name)
        ))
    }

    /// Rebuild from the start (no checkpoint is valid), still cutting off
    /// early against the `previous` build.
    fn rebuild_from_start(&mut self, previous: Previous, t0: Instant) -> Report {
        let host = SessionHost {
            shared: self.shared.clone(),
            mark: self.shared.borrow().mark(),
        };
        let mut tex = Tex::new(
            host,
            crate::intervals::share(&self.tracker),
            self.params.clone(),
        );
        tex.set_checkpoint_interval(self.every);
        tex.record_objstms(switch_on("PARTEX_OBJSTM_LINK"));
        crate::intervals::clear(&self.tracker);
        self.interval_from = 0;
        self.written_since.clear();
        let step = tex.start(&self.command_line);
        let (history, commands, cut_at) = self.drive(tex, step, Some(previous));
        self.total_commands = commands;
        self.history = history;
        Report {
            history,
            commands: commands - self.skipped,
            total_commands: commands,
            elapsed: t0.elapsed(),
            cut_at,
            why: std::mem::take(&mut self.why),
        }
    }

    /// Take as unchanged the changed reads that made no difference: while
    /// the first checkpoint the changes leave invalid is blocked only by
    /// reads of files closed again before it (existence and size probes,
    /// say), run the interval before it again and compare the state
    /// reached with it (backdating, as in Salsa). A changed file still
    /// open at the checkpoint is never taken as unchanged.
    ///
    /// A trial that does not match is where the rebuild goes on from: it
    /// ran from the checkpoint the rebuild resumes at, with the files as
    /// they are now (returned with its logs).
    fn absorb(&mut self, diffs: &mut [Change]) -> Option<(Engine, Logs)> {
        loop {
            let k = self.latest_valid(diffs);
            let next = k.map_or(0, |k| k + 1);
            if next >= self.checkpoints.len() {
                return None;
            }
            let lo = k.map_or(0, |k| self.checkpoints[k].1.host().mark.reads);
            let (at, old) = &self.checkpoints[next];
            let hi = old.host().mark.reads;
            let open = old.open_inputs();
            let open_ahead = |c: &Change| {
                c.replace.as_ref().is_some_and(|(was, _)| {
                    open.iter()
                        .any(|(data, pos)| Arc::ptr_eq(data, was) && c.diff >= Some(*pos))
                })
            };
            let blocking: Vec<usize> = (lo..hi)
                .filter(|&i| diffs[i].diff.is_some() && !open_ahead(&diffs[i]))
                .collect();
            if blocking.is_empty() || blocking.iter().any(|&i| diffs[i].replace.is_none()) {
                return None; // (a file found or lost: no state to compare)
            }
            let at = *at;
            // (where it was: a plain clone's host marks the logs as they
            // are now, and the effects compared would end there)
            let mut old = clone_checkpoint(old);
            for d in diffs.iter() {
                if let (Some(_), Some((was, is))) = (d.diff, &d.replace) {
                    old.replace_input(was, is);
                }
            }
            let saved = self.shared.borrow().logs();
            crate::intervals::clear(&self.tracker);
            self.interval_from = k.map_or(0, |k| self.checkpoints[k].0);
            self.written_since.clear();
            let t = Instant::now();
            let trial = self.run_interval(k, diffs, at);
            let logs = self.shared.borrow_mut().take_logs();
            let changed: Vec<OldNew> = diffs
                .iter()
                .filter(|d| d.diff.is_some())
                .filter_map(|d| d.replace.clone())
                .collect();
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!(
                    "partex:   trial from reads {lo} to {hi}: logged {} reads, saved {}; names now {:?}; then {:?}",
                    logs.reads.len(),
                    saved.reads.len(),
                    logs.reads
                        .iter()
                        .skip(lo)
                        .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                        .collect::<Vec<_>>(),
                    saved
                        .reads
                        .iter()
                        .skip(lo)
                        .take(hi - lo)
                        .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                        .collect::<Vec<_>>()
                );
            }
            let same_logs = logs.same_as(&saved, &old.host().mark, &changed);
            self.shared.borrow_mut().set_logs(saved);
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                let from = k.map_or(0, |k| self.checkpoints[k].0);
                eprintln!(
                    "partex: backdating trial {from}..{at} ({} reads blocking): {:.1} ms",
                    blocking.len(),
                    t.elapsed().as_secs_f64() * 1e3
                );
            }
            let same = same_logs
                && trial.as_ref().is_some_and(|t| {
                    t.commands() == at
                        && t.position_key() == old.position_key()
                        && hash_of(t) == hash_of(&old)
                });
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() && !same {
                eprintln!("partex: backdating trial differs");
                eprintln!(
                    "partex:   logs the same: {same_logs}; state: {}",
                    trial.as_ref().map_or_else(
                        || "no trial".to_owned(),
                        |t| format!(
                            "at {} (not {}), {}",
                            t.commands(),
                            at,
                            state_difference(t, &old)
                        )
                    )
                );
            }
            if !same {
                return trial.filter(|_| k.is_some()).map(|t| (t, logs));
            }
            let mut sh = self.shared.borrow_mut();
            for i in blocking {
                diffs[i].diff = None;
                sh.reads[i].found.clone_from(&diffs[i].now);
            }
        }
    }

    /// Run from checkpoint `k` (or the start) with the files as they are
    /// now, to the first checkpoint at or past command `at`: the engine
    /// there. (The kept checkpoints are thinned: others come between.)
    fn run_interval(&mut self, k: Option<usize>, diffs: &[Change], at: u64) -> Option<Engine> {
        let mut tex = if let Some(k) = k {
            // (rewound first: a clone's host marks the logs as they are)
            let mark = self.checkpoints[k].1.host().mark.clone();
            self.shared.borrow_mut().rewind(&mark);
            let mut tex = self.checkpoints[k].1.clone();
            for d in diffs {
                if let (Some(_), Some((was, is))) = (d.diff, &d.replace) {
                    tex.replace_input(was, is);
                }
            }
            tex
        } else {
            self.shared.borrow_mut().rewind(&Mark::start());
            let host = SessionHost {
                shared: self.shared.clone(),
                mark: self.shared.borrow().mark(),
            };
            let mut tex = Tex::new(
                host,
                crate::intervals::share(&self.tracker),
                self.params.clone(),
            );
            tex.set_checkpoint_interval(self.every);
            tex.record_objstms(switch_on("PARTEX_OBJSTM_LINK"));
            tex
        };
        let mut step = if k.is_some() {
            tex.resume()
        } else {
            tex.start(&self.command_line)
        };
        while step == Step::Checkpoint && tex.commands() < at {
            step = tex.resume();
        }
        (step == Step::Checkpoint).then_some(tex)
    }

    /// The index of the latest checkpoint the changes leave valid.
    fn latest_valid(&self, diffs: &[Change]) -> Option<usize> {
        let rules = self.line_rules(diffs);
        (0..self.checkpoints.len()).rev().find(|&k| {
            let (at, tex) = &self.checkpoints[k];
            let (reads, open) = (tex.host().mark.reads, tex.open_inputs());
            valid_for(diffs, reads, &open) || self.valid_by_lines(diffs, &rules, reads, &open, *at)
        })
    }

    /// Source text as values for resuming (DESIGN.md §7.2): a changed
    /// file read by lines, and closed or not yet read past the change,
    /// leaves a checkpoint valid if everything before it read the old
    /// contents only as lines (each lookup opened it for lines: `\input`,
    /// `\openin`; LaTeX's `\IfFileExists` opens and closes it unread) and
    /// only lines before the first difference. For each changed lookup:
    /// the first command of the first interval that read a line at or
    /// past the difference (or showed one, or whose lines moved), and the
    /// file's opens for lines by the end of each interval. `None` where
    /// the intervals do not record the whole job
    /// (`PARTEX_LINE_RESUME=0`: always).
    fn line_rules(&self, diffs: &[Change]) -> Vec<Option<LineRule>> {
        let covered = crate::tokendeps::covers(&self.intervals, self.total_commands);
        let usable =
            switch_on("PARTEX_LINE_RESUME") && self.tracker.follows_lines() && covered.is_ok();
        if let (Err(why), true) = (covered, std::env::var_os("PARTEX_WATCH_DEBUG").is_some()) {
            eprintln!("partex: no line rules: {why}");
        }
        diffs
            .iter()
            .map(|c| {
                let (Some(d), Some((old, new)), true) = (c.diff, &c.replace, usable) else {
                    return None;
                };
                // (events name the old contents, or the new ones once a
                // failed token comparison moved them, `tokendeps::remap`)
                let ids = [
                    crate::intervals::file_id(old),
                    crate::intervals::file_id(new),
                ];
                let past = |f: usize, s: usize| {
                    let data = if f == ids[0] { old } else { new };
                    let end = data[s.min(data.len())..]
                        .iter()
                        .position(|&b| b == b'\n' || b == b'\r')
                        .map_or(data.len(), |n| s + n);
                    end >= d
                };
                let mut rule = LineRule {
                    until: u64::MAX,
                    opens: Vec::new(),
                };
                let mut opens = 0;
                for (from, iv) in &self.intervals {
                    for e in iv.lines.iter().filter(|e| ids.contains(&e.file())) {
                        let bad = match *e {
                            LineEvent::Open(_) => {
                                opens += 1;
                                false
                            }
                            LineEvent::Start(f, s)
                            | LineEvent::End(f, s, _)
                            | LineEvent::Shown(f, s) => past(f, s),
                            LineEvent::Poison(_) => true,
                        };
                        if bad && rule.until == u64::MAX {
                            rule.until = *from;
                        }
                    }
                    rule.opens.push((iv.to, opens));
                }
                if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                    eprintln!(
                        "partex: line rule: valid up to command {}, {opens} opens for lines",
                        rule.until
                    );
                }
                Some(rule)
            })
            .collect()
    }

    /// Whether the checkpoint at command `at`, after the first `reads`
    /// lookups, with files `open`, is valid by `line_rules`.
    fn valid_by_lines(
        &self,
        diffs: &[Change],
        rules: &[Option<LineRule>],
        reads: usize,
        open: &[(Arc<[u8]>, usize)],
        at: u64,
    ) -> bool {
        let sh = self.shared.borrow();
        diffs[..reads].iter().enumerate().all(|(i, c)| {
            let (Some(d), Some((old, _))) = (c.diff, &c.replace) else {
                return c.diff.is_none();
            };
            if open
                .iter()
                .any(|(data, pos)| Arc::ptr_eq(data, old) && d >= *pos)
            {
                return true;
            }
            let Some(r) = rules.get(i).and_then(Option::as_ref) else {
                return false;
            };
            // (open, but read past the change: its line was read)
            let looked_up = sh.reads[..reads]
                .iter()
                .filter(|r| r.found.as_ref().is_some_and(|(_, f)| Arc::ptr_eq(f, old)))
                .count();
            at <= r.until
                && !open.iter().any(|(data, _)| Arc::ptr_eq(data, old))
                && looked_up <= r.opens_by(at)
        })
    }

    /// The diagnostics of the last build, in TeX's order.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.shared.borrow().diags.clone()
    }

    /// The terminal transcript of the last build.
    pub fn terminal(&self) -> Vec<u8> {
        self.shared.borrow().term.clone()
    }

    /// The output files of the last build: names and lengths.
    pub fn outputs(&self) -> Vec<(Vec<u8>, usize)> {
        let sh = self.shared.borrow();
        sh.outputs
            .iter()
            .map(|o| (o.name.clone(), o.bytes.len()))
            .collect()
    }

    /// Write the output files and return the terminal transcript.
    pub fn write_outputs(&self) -> std::io::Result<Vec<u8>> {
        let t0 = Instant::now();
        let sh = self.shared.borrow();
        let fast = switch_on("PARTEX_FAST_WRITE");
        // The PDF (or DVI) file first, whole at once (renamed into place,
        // so a viewer never reads half of it); then the rest. A file whose
        // bytes are what was last written, and still there, is not written
        // again (a viewer does not reload an unchanged PDF). No fsync.
        let not_main = |o: &&Output| !(o.name.ends_with(b".pdf") || o.name.ends_with(b".dvi"));
        let mut order: Vec<&Output> = sh.outputs.iter().collect();
        if fast {
            order.sort_by_key(not_main);
        }
        let mut written = self.written.borrow_mut();
        for o in order {
            let p = crate::native::path(&o.name);
            if !fast {
                std::fs::write(&p, &o.bytes)?;
                continue;
            }
            let key = (o.bytes.len(), {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                o.bytes.hash(&mut h);
                h.finish()
            });
            let there = std::fs::metadata(&p).is_ok_and(|m| m.len() == o.bytes.len() as u64);
            if there && written.get(&o.name) == Some(&key) {
                continue;
            }
            if not_main(&o) {
                std::fs::write(&p, &o.bytes)?;
            } else {
                let mut tmp = o.name.clone();
                tmp.extend_from_slice(b".partex-tmp");
                let tmp = crate::native::path(&tmp);
                std::fs::write(&tmp, &o.bytes)?;
                std::fs::rename(&tmp, &p)?;
            }
            written.insert(o.name.clone(), key);
        }
        debug_time("outputs written", t0);
        Ok(sh.term.clone())
    }

    /// The files the job wrote whose names end in `ext`: names and
    /// contents.
    pub fn outputs_ending(&self, ext: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let sh = self.shared.borrow();
        sh.outputs
            .iter()
            .filter(|o| o.name.ends_with(ext))
            .map(|o| (o.name.clone(), o.bytes.clone()))
            .collect()
    }

    /// The files the job read (to watch).
    pub fn inputs(&self) -> Vec<std::path::PathBuf> {
        let sh = self.shared.borrow();
        let mut v: Vec<_> = sh
            .reads
            .iter()
            .filter_map(|r| r.found.as_ref().map(|(n, _)| crate::native::path(n)))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn checkpoints(&self) -> usize {
        self.checkpoints.len() + self.saved.as_ref().map_or(0, |s| s.checkpoints.len())
    }

    /// The history of the last build (of the loaded one, after `load`).
    pub fn history(&self) -> i32 {
        self.history
    }

    /// Whether a build or rebuild ran since the session was loaded.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// The session in a form [`Session::load`] takes, for another process
    /// to rebuild from (DESIGN.md §5.3): the logs, and a few checkpoints
    /// (see `saved_checkpoints`). Pooled: values shared by the logs and
    /// the checkpoints (file contents, the engines' copy-on-write chunks)
    /// are written once, and each checkpoint can be loaded alone. `None`
    /// if a checkpoint cannot be saved.
    pub fn save(&mut self) -> Option<Vec<u8>> {
        let t0 = Instant::now();
        self.load_all();
        debug_time("save: loaded all", t0);
        let stats = std::env::var_os("PARTEX_PERSIST_STATS").is_some();
        let mut s = Saver::pooled();
        if stats {
            s.pool_sizes();
        }
        {
            let sh = self.shared.borrow();
            sh.reads.len().save(&mut s);
            for r in &sh.reads {
                r.name.save(&mut s);
                kind_code(r.kind).save(&mut s);
                r.found.save(&mut s);
            }
            sh.outputs.len().save(&mut s);
            for o in &sh.outputs {
                o.name.save(&mut s);
                o.bytes.save(&mut s);
            }
            sh.term.save(&mut s);
            sh.pages.save(&mut s);
            sh.diags.len().save(&mut s);
            for d in &sh.diags {
                crate::events::save_diagnostic(d, &mut s);
            }
        }
        self.total_commands.save(&mut s);
        self.history.save(&mut s);
        let kept = self.saved_checkpoints();
        kept.len().save(&mut s);
        for &i in &kept {
            let (at, cp) = &self.checkpoints[i];
            at.save(&mut s);
            let m = &cp.host().mark;
            (m.reads, m.outputs.clone(), m.term, m.pages, m.diags).save(&mut s);
            cp.open_inputs().save(&mut s);
        }
        let mut segments = vec![s.take_segment()];
        for &i in &kept {
            if !self.checkpoints[i].1.save_state(&mut s) {
                return None;
            }
            segments.push(s.take_segment());
        }
        debug_time("save: encoded", t0);
        Some(pack(&segments, &s.into_pool(), stats))
    }

    /// Which checkpoints to save (DESIGN.md §5.3): the next edit is most
    /// likely near the last, so those just before where the last rebuild
    /// resumed, the last (a PDF splice goes on from it), and the rest
    /// spread over the job (a few deep ones cover the preamble).
    fn saved_checkpoints(&self) -> Vec<usize> {
        let n = self.checkpoints.len();
        if n <= SAVED_CHECKPOINTS {
            return (0..n).collect();
        }
        let mut v = vec![n - 1];
        if let Some(at) = self.resumed_at {
            let k = self.checkpoints.partition_point(|c| c.0 <= at);
            v.extend((k.saturating_sub(3)..k).rev());
        }
        let spread = SAVED_CHECKPOINTS.saturating_sub(v.len()).max(1);
        v.extend((0..spread).map(|i| i * (n - 1) / spread));
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Take the state `save` wrote (in a fresh session of the same job):
    /// the next [`Session::rebuild`] goes on from it, loading checkpoints
    /// as it needs them. `false` (the session left fresh) if it cannot be
    /// read.
    pub fn load(&mut self, bytes: Vec<u8>) -> bool {
        let loaded = self.load_saved(bytes).is_some();
        if !loaded {
            let mut sh = self.shared.borrow_mut();
            sh.reads.clear();
            sh.outputs.clear();
            sh.term.clear();
            sh.pages.clear();
            sh.diags.clear();
            drop(sh);
            self.saved = None;
        }
        self.changed = false;
        loaded
    }

    fn load_saved(&mut self, bytes: Vec<u8>) -> Option<()> {
        let (segments, pool, spans) = unpack(&bytes)?;
        let mut l = Loader::pooled(
            &bytes[segments[0].clone()],
            &bytes[pool.clone()],
            &spans,
            Vec::new(),
        );
        let n: usize = Persist::load(&mut l)?;
        let mut reads = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            reads.push(Read {
                name: Persist::load(&mut l)?,
                kind: kind_from_code(Persist::load(&mut l)?)?,
                found: Persist::load(&mut l)?,
            });
        }
        let n: usize = Persist::load(&mut l)?;
        let mut outputs = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            outputs.push(Output {
                name: Persist::load(&mut l)?,
                bytes: Persist::load(&mut l)?,
            });
        }
        let term: Vec<u8> = Persist::load(&mut l)?;
        let pages: Vec<Page> = Persist::load(&mut l)?;
        let n: usize = Persist::load(&mut l)?;
        let mut diags = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            diags.push(crate::events::load_diagnostic(&mut l)?);
        }
        let total_commands: u64 = Persist::load(&mut l)?;
        let history: i32 = Persist::load(&mut l)?;
        let n: usize = Persist::load(&mut l)?;
        if n + 1 != segments.len() {
            return None;
        }
        let mut checkpoints = Vec::with_capacity(n);
        for seg in &segments[1..] {
            let at: u64 = Persist::load(&mut l)?;
            let (reads, outputs, term, pages, diags) = Persist::load(&mut l)?;
            let open: Vec<(Arc<[u8]>, usize)> = Persist::load(&mut l)?;
            checkpoints.push(SavedCheckpoint {
                at,
                mark: Mark {
                    reads,
                    outputs,
                    term,
                    pages,
                    diags,
                },
                open,
                segment: seg.clone(),
            });
        }
        if !l.at_end() {
            return None;
        }
        let shared = l.into_shared();
        let mut sh = self.shared.borrow_mut();
        sh.reads = reads;
        sh.outputs = outputs;
        sh.term = term;
        sh.pages = pages;
        sh.diags = diags;
        drop(sh);
        self.checkpoints.clear();
        self.saved = Some(Saved {
            pool,
            spans,
            shared,
            checkpoints,
            replaced: Vec::new(),
            bytes,
        });
        self.total_commands = total_commands;
        self.history = history;
        Some(())
    }

    /// Before a rebuild: load the saved checkpoints it may use, unless a
    /// loaded one is valid. From the latest the changes leave valid on
    /// (the later ones are compared with for early cutoff); all if none
    /// is valid.
    fn load_needed(&mut self, diffs: &[Change]) {
        let _p = crate::timeline::phase("load checkpoints");
        let Some(saved) = &self.saved else { return };
        if self.latest_valid(diffs).is_some() {
            return;
        }
        let k = (0..saved.checkpoints.len()).rev().find(|&k| {
            let c = &saved.checkpoints[k];
            valid_for(diffs, c.mark.reads, &c.open)
        });
        self.load_from(k.unwrap_or(0));
    }

    fn load_all(&mut self) {
        self.load_from(0);
    }

    /// Load the saved checkpoints from the `k`th on (before the loaded
    /// ones: those saved come first).
    fn load_from(&mut self, k: usize) {
        let Some(saved) = &mut self.saved else { return };
        let t0 = Instant::now();
        let mut shared = std::mem::take(&mut saved.shared);
        let mut loaded = Vec::new();
        for c in saved.checkpoints.drain(k..) {
            let host = SessionHost {
                shared: self.shared.clone(),
                mark: c.mark,
            };
            let mut l = Loader::pooled(
                &saved.bytes[c.segment],
                &saved.bytes[saved.pool.clone()],
                &saved.spans,
                shared,
            );
            let tex = Engine::load_state(&mut l, host, crate::intervals::share(&self.tracker));
            let ok = l.at_end();
            shared = l.into_shared();
            // (one that cannot be loaded is dropped with those before it:
            // they are all kept or none, in order)
            let Some(mut tex) = tex.filter(|_| ok) else {
                loaded.clear();
                continue;
            };
            tex.set_checkpoint_interval(self.every);
            tex.record_objstms(switch_on("PARTEX_OBJSTM_LINK"));
            for (old, new) in &saved.replaced {
                tex.replace_input(old, new);
            }
            loaded.push((c.at, tex));
        }
        saved.shared = shared;
        let n = loaded.len();
        loaded.append(&mut self.checkpoints);
        self.checkpoints = loaded;
        if saved.checkpoints.is_empty() {
            self.saved = None;
        }
        debug_time(&format!("loaded {n} saved checkpoints"), t0);
    }
}

/// A logged lookup done again.
struct Change {
    /// Where the contents first differ (0: found elsewhere or not at all).
    diff: Option<usize>,
    now: Option<Found>,
    /// The old and new contents, if found both times.
    replace: Option<OldNew>,
}

type OldNew = (Arc<[u8]>, Arc<[u8]>);

/// The checkpoints an object stream rewrite was asked for, and the answer.
type RewriteAsked = ((usize, usize), Option<ObjStmRewrite>);

/// An object stream the previous build wrote, and it rendered again with
/// the rebuild's objects: the old record, the new, the new object's bytes
/// (`Session::objstm_rewrite`).
type ObjStmRewrite = Arc<(
    partex_core::ObjStmWritten,
    partex_core::ObjStmWritten,
    Vec<u8>,
)>;

/// How far a changed file read by lines leaves checkpoints valid
/// (`Session::line_rules`).
struct LineRule {
    /// The first command of the first interval that read a line at or past
    /// the first difference: checkpoints up to it are valid.
    until: u64,
    /// The file's opens for lines by the end of each interval.
    opens: Vec<(u64, usize)>,
}

impl LineRule {
    /// The opens for lines before command `at`.
    fn opens_by(&self, at: u64) -> usize {
        let k = self.opens.partition_point(|&(to, _)| to <= at);
        k.checked_sub(1).map_or(0, |k| self.opens[k].1)
    }
}

/// A changed file's old and new contents, with where its lines moved
/// (`tokendeps::line_map`).
type FileMap = (Arc<[u8]>, Arc<[u8]>, HashMap<usize, (usize, bool)>);

/// A changed file as `Session::changed_files` gives it.
type ChangedFile = (Arc<[u8]>, Arc<[u8]>, usize, String);

fn kind_code(k: Query) -> u8 {
    match k {
        Query::File(FileKind::Tex) => 0,
        Query::File(FileKind::Tfm) => 1,
        Query::File(FileKind::Fmt) => 2,
        Query::File(FileKind::Other) => 3,
        Query::File(FileKind::FontMap) => 4,
        Query::File(FileKind::Type1) => 5,
        Query::File(FileKind::Enc) => 6,
        Query::File(FileKind::Vf) => 7,
        Query::File(FileKind::TrueType) => 8,
        Query::ModDate => 9,
        Query::File(FileKind::Bst) => 10,
        Query::File(FileKind::Bib) => 11,
        Query::File(FileKind::Ist) => 12,
        Query::File(FileKind::OpenType) => 13,
        Query::File(FileKind::MiscFonts) => 14,
        Query::File(FileKind::FontIndex) => 15,
    }
}

fn kind_from_code(c: u8) -> Option<Query> {
    Some(Query::File(match c {
        0 => FileKind::Tex,
        1 => FileKind::Tfm,
        2 => FileKind::Fmt,
        3 => FileKind::Other,
        4 => FileKind::FontMap,
        5 => FileKind::Type1,
        6 => FileKind::Enc,
        7 => FileKind::Vf,
        8 => FileKind::TrueType,
        9 => return Some(Query::ModDate),
        10 => FileKind::Bst,
        11 => FileKind::Bib,
        12 => FileKind::Ist,
        13 => FileKind::OpenType,
        14 => FileKind::MiscFonts,
        15 => FileKind::FontIndex,
        _ => return None,
    }))
}
