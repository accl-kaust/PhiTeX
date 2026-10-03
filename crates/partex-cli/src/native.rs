//! Native [`Host`]: real filesystem and terminal.
//!
//! File lookup goes through `partex-kpse`, the native kpathsea port.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, Write};

use partex_core::dviout::{DviWriter, Summary, TooLong};
use partex_core::pageir::Page;
use partex_core::{DateTime, FileKind, Host, OpenedFile, PageSink, WriteId};

use crate::dvithread::DviThread;

pub struct NativeHost {
    files: HashMap<WriteId, File>,
    next_id: u32,
    /// The file each handle was given for, and the count of opens
    /// (`opens`) at its last open: a step that runs again opens a file on
    /// its handle again (`open_write_again`), and a file opened since a
    /// link was emptied by its open ([`NativeHost::opened_after`]).
    given: HashMap<WriteId, (Vec<u8>, u64)>,
    opens: u64,
    clock: crate::clock::Clock,
    kpse: partex_kpse::Kpse,
    /// `-output-directory`: where output files go, and where input files
    /// are looked for first.
    pub output_dir: Option<Vec<u8>>,
    /// Write the DVI file on a thread of its own.
    pub dvi_thread: bool,
    dvi: Option<DviThread>,
    /// Ask the engine for warnings and notes as diagnostics (the modern
    /// command line's renderer groups them).
    pub notes: bool,
    /// Where live progress goes (pages as they are shipped out).
    pub live: Option<crate::events::LiveSink>,
    /// A directory searched for formats before kpathsea's path (the
    /// modern command line's own formats).
    pub formats: Option<std::path::PathBuf>,
    /// Content-keyed memos (`Host::cached`: Type 1 subsets), kept for the
    /// process, so a session's rebuilds share them
    /// (`PARTEX_T1_CACHE=0`: none).
    memo: Option<std::collections::BTreeMap<u128, partex_core::host::Memo>>,
    /// The lookups and files as last made and read, for
    /// [`Host::unchanged`] (`PARTEX_STAT_CACHE=0`: none).
    seen: Option<Seen>,
}

/// What [`Host::unchanged`] answers from (DESIGN 7.17.3, "A rebuild's
/// file checks cost the files that changed").
#[derive(Default)]
struct Seen {
    /// What each path held when last read, with its stamp taken just
    /// before: none for a file whose times were under 2 s old then.
    files: phitex_doc::FxMap<Vec<u8>, (Stamp, std::sync::Arc<[u8]>)>,
    /// Each name's last lookup, by kind (a name is looked up as one kind,
    /// mostly).
    lookups: phitex_doc::FxMap<Vec<u8>, Vec<(FileKind, Lookup)>>,
    /// The directories of those, numbered (as `crate::inotify` knows
    /// them), and their names by number.
    dir_ids: phitex_doc::FxMap<Vec<u8>, u32>,
    dir_names: Vec<Vec<u8>>,
    /// The directories of those, watched (`crate::inotify`): a load none
    /// of whose names had an event since it was last found as it was
    /// (`Lookup::checked`) needs no `stat`; `unwatched`: there is no
    /// watcher (`PARTEX_INOTIFY=0`, or none).
    watch: Option<crate::inotify::Watcher>,
    unwatched: bool,
}

impl Seen {
    /// Directory `d`'s number.
    fn dir_id(&mut self, d: &[u8]) -> u32 {
        if let Some(&id) = self.dir_ids.get(d) {
            return id;
        }
        let id = u32::try_from(self.dir_names.len()).unwrap_or(u32::MAX);
        self.dir_ids.insert(d.to_vec(), id);
        self.dir_names.push(d.to_vec());
        id
    }

    /// Name `name`'s last lookup as `kind`.
    fn lookup(&mut self, name: &[u8], kind: FileKind) -> Option<&mut Lookup> {
        let ks = self.lookups.get_mut(name)?;
        ks.iter_mut().find(|(k, _)| *k == kind).map(|(_, l)| l)
    }
}

/// A lookup: the path it found (and its directory's number), and its
/// trail, the candidates it tried and did not find, by directory.
struct Lookup {
    found: Option<(Vec<u8>, u32)>,
    trail: Vec<Missed>,
    /// When `unchanged` last found it as it was (the watcher's count).
    checked: Option<u64>,
}

/// The candidates a lookup tried in one directory (by its number) and
/// did not find, and the directory's stamp after the search (none for a
/// directory that is not there), `kept` if it was over 2 s old: while
/// the directory has that stamp, none of them is there. For a directory
/// that is not there, `anchor`: the nearest one above it that is, and
/// the name in it that would make it be (`out/figs/data`: `out` and
/// `figs`), which the watcher watches for it.
struct Missed {
    dir: u32,
    anchor: Option<(u32, Vec<u8>)>,
    stamp: Option<Stamp>,
    kept: bool,
    files: Vec<Vec<u8>>,
}

/// How old a file's times must be for its stamp to be kept: a later write
/// within the same clock tick could leave them alike (git's racy
/// entries; `quick.rs`'s rule).
const RACY_NS: i128 = 2_000_000_000;

impl NativeHost {
    pub fn new(clock: crate::clock::Clock, kpse: partex_kpse::Kpse) -> Self {
        Self {
            files: HashMap::new(),
            next_id: 0,
            given: HashMap::new(),
            opens: 0,
            clock,
            kpse,
            output_dir: None,
            dvi_thread: true,
            dvi: None,
            notes: false,
            live: None,
            formats: None,
            memo: (!std::env::var("PARTEX_T1_CACHE").is_ok_and(|v| v == "0"))
                .then(std::collections::BTreeMap::new),
            seen: std::env::var_os("PARTEX_STAT_CACHE")
                .is_none_or(|v| v != "0")
                .then(Seen::default),
        }
    }

    /// Read the file at `p`, keeping its contents and the stamp taken
    /// before reading them if its times are old enough. A file whose stamp
    /// is the one kept is as it was: its contents are handed out again,
    /// the same `Arc`, which the engine's caches know by identity (the
    /// 5 MB font map, read again whenever the step that ships the first
    /// page runs again).
    fn read_at(&mut self, p: &[u8]) -> Option<std::sync::Arc<[u8]>> {
        let st = self.seen.as_ref().and_then(|_| stamp(p));
        if let (Some(seen), Some(s)) = (&self.seen, st)
            && let Some((kept, c)) = seen.files.get(p)
            && *kept == s
        {
            return Some(c.clone());
        }
        let contents: std::sync::Arc<[u8]> = std::fs::read(path(p)).ok()?.into();
        if let Some(seen) = &mut self.seen {
            match st.filter(quiet) {
                Some(st) => {
                    seen.files.insert(p.to_vec(), (st, contents.clone()));
                }
                None => {
                    seen.files.remove(p);
                }
            }
        }
        Some(contents)
    }

    /// [`Host::read_file`]'s lookup and read, each candidate tried and not
    /// found noted in `trail`; `Err` for a file found but not read.
    fn find_read(
        &mut self,
        name: &[u8],
        kind: FileKind,
        trail: &mut Vec<Vec<u8>>,
    ) -> Result<Option<OpenedFile>, ()> {
        let note = |trail: &mut Vec<Vec<u8>>, p: &[u8]| trail.push(p.to_vec());
        // openclose.c's `open_input`: the output directory first
        if let Some(p) = self.in_output_dir(name) {
            let tries = [p.clone(), with_suffix(&p, kind)];
            for p in tries {
                if !std::fs::metadata(path(&p)).is_ok_and(|m| m.is_dir())
                    && let Some(contents) = self.read_at(&p)
                {
                    return Ok(Some(OpenedFile { name: p, contents }));
                }
                note(trail, &p);
            }
        }
        if kind == FileKind::Fmt
            && let Some(dir) = &self.formats
        {
            let p = dir
                .join(path(&with_suffix(name, kind)))
                .into_os_string()
                .into_encoded_bytes();
            if let Some(contents) = self.read_at(&p) {
                return Ok(Some(OpenedFile { name: p, contents }));
            }
            note(trail, &p);
        }
        let format = match kind {
            FileKind::Tex => partex_kpse::Format::Tex,
            FileKind::Tfm => partex_kpse::Format::Tfm,
            FileKind::Fmt => partex_kpse::Format::Fmt,
            FileKind::FontMap => partex_kpse::Format::FontMap,
            FileKind::Type1 => partex_kpse::Format::Type1,
            FileKind::Enc => partex_kpse::Format::Enc,
            FileKind::Vf => partex_kpse::Format::Vf,
            FileKind::TrueType => partex_kpse::Format::TrueType,
            FileKind::Other => {
                let Some(contents) = self.read_at(name) else {
                    note(trail, name);
                    return Ok(None);
                };
                return Ok(Some(OpenedFile {
                    name: name.to_vec(),
                    contents,
                }));
            }
        };
        // web2c's `open_input` asks with `must_exist` true for \input and
        // fonts (\openin passes false; the difference only matters for
        // files missing from ls-R).
        let (found, t) = self.kpse.find_file_trail(name, format, true);
        trail.extend(t);
        let Some(found) = found else {
            return Ok(None);
        };
        let contents = self.read_at(&found).ok_or(())?;
        Ok(Some(OpenedFile {
            name: found,
            contents,
        }))
    }

    fn dvi(&mut self) -> &mut DviThread {
        self.dvi.as_mut().expect("the page sink was started")
    }
}

impl PageSink for NativeHost {
    fn start(&mut self, writer: DviWriter, file: WriteId) {
        // (the engine writes the file only through the sink from now on)
        let f = self.files[&file]
            .try_clone()
            .expect("cloning a file handle");
        self.dvi = Some(DviThread::spawn(writer, f));
    }

    fn page(&mut self, page: Page) -> Page {
        self.dvi().page(page)
    }

    fn page_now(&mut self, page: Page) -> (Page, Result<i32, TooLong>) {
        self.dvi().page_now(page)
    }

    fn finish(&mut self, mag: i32) -> Result<Summary, TooLong> {
        let r = self.dvi().finish(mag);
        self.dvi = None;
        r
    }
}

pub fn path(name: &[u8]) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(name).into()
}

/// The name an output file gets (web2c adds the default suffix).
pub fn with_suffix(name: &[u8], kind: FileKind) -> Vec<u8> {
    let suffix: &[u8] = match kind {
        FileKind::Tex => b".tex",
        FileKind::Tfm => b".tfm",
        FileKind::Fmt => b".fmt",
        FileKind::FontMap
        | FileKind::Type1
        | FileKind::Enc
        | FileKind::Vf
        | FileKind::TrueType
        | FileKind::Other => b"",
    };
    let mut n = name.to_vec();
    if !suffix.is_empty() && !name.ends_with(suffix) {
        n.extend_from_slice(suffix);
    }
    n
}

impl NativeHost {
    /// How many files were opened for writing so far.
    pub fn opens(&self) -> u64 {
        self.opens
    }

    /// Whether handle `id`'s file was opened after the first `count`
    /// opens ([`NativeHost::opens`]): its open emptied it.
    pub fn opened_after(&self, id: WriteId, count: u64) -> bool {
        self.given.get(&id).is_some_and(|&(_, at)| at > count)
    }

    /// `name` in the output directory, if there is one and `name` is
    /// relative (openclose.c).
    pub fn in_output_dir(&self, name: &[u8]) -> Option<Vec<u8>> {
        let dir = self
            .output_dir
            .as_ref()
            .filter(|_| !name.starts_with(b"/"))?;
        let mut p = dir.clone();
        p.push(b'/');
        p.extend_from_slice(name);
        Some(p)
    }
}

/// What says a file is unchanged without reading it: size, times, inode.
pub type Stamp = (u64, i64, i64, i64, i64, u64, u64);

/// Whether a stamp's times are over [`RACY_NS`] old.
fn quiet(st: &Stamp) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX));
    let ns = |s: i64, n: i64| i128::from(s) * 1_000_000_000 + i128::from(n);
    now - ns(st.1, st.2).max(ns(st.3, st.4)) > RACY_NS
}

/// The stamp of the directory (or file) at `p`, if there is one.
/// The nearest directory above `d` (one that is not there) that is
/// there, and the name in it of the one below it on the way to `d`.
fn anchor(d: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut d = d;
    loop {
        let (up, name) = crate::inotify::split(d);
        if up == d {
            return None;
        }
        if dir_stamp(up).is_some() {
            return Some((up, name));
        }
        d = up;
    }
}

fn dir_stamp(p: &[u8]) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path(p)).ok()?;
    Some((
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.ino(),
        m.dev(),
    ))
}

/// The stamp of the file at `p`, if it is one (not a directory).
pub fn stamp(p: &[u8]) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path(p)).ok()?;
    (!m.is_dir()).then(|| {
        (
            m.size(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.ino(),
            m.dev(),
        )
    })
}

impl NativeHost {
    /// Where [`Host::read_file`] would find `name`, without reading it.
    pub fn locate(&mut self, name: &[u8], kind: FileKind) -> Option<Vec<u8>> {
        if let Some(p) = self.in_output_dir(name) {
            for p in [p.clone(), with_suffix(&p, kind)] {
                if stamp(&p).is_some() {
                    return Some(p);
                }
            }
        }
        if kind == FileKind::Fmt
            && let Some(dir) = &self.formats
        {
            let p = dir
                .join(path(&with_suffix(name, kind)))
                .into_os_string()
                .into_encoded_bytes();
            if stamp(&p).is_some() {
                return Some(p);
            }
        }
        let format = match kind {
            FileKind::Tex => partex_kpse::Format::Tex,
            FileKind::Tfm => partex_kpse::Format::Tfm,
            FileKind::Fmt => partex_kpse::Format::Fmt,
            FileKind::FontMap => partex_kpse::Format::FontMap,
            FileKind::Type1 => partex_kpse::Format::Type1,
            FileKind::Enc => partex_kpse::Format::Enc,
            FileKind::Vf => partex_kpse::Format::Vf,
            FileKind::TrueType => partex_kpse::Format::TrueType,
            FileKind::Other => return stamp(name).map(|_| name.to_vec()),
        };
        self.kpse.find_file(name, format, true)
    }
}

impl Host for NativeHost {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut trail = Vec::new();
        let found = self.find_read(name, kind, &mut trail);
        let Some(seen) = &mut self.seen else {
            return found.ok().flatten();
        };
        let Ok(f) = &found else {
            if let Some(ks) = seen.lookups.get_mut(name) {
                ks.retain(|(k, _)| *k != kind);
            }
            return None;
        };
        // (the candidates by directory, each directory's stamp as it is
        // after the search: a file made in it since changes its times; one
        // under 2 s old could change within its clock tick, and is not
        // kept)
        let mut missed: Vec<Missed> = Vec::new();
        for c in trail {
            let d = crate::inotify::split(&c).0;
            match missed.last_mut() {
                Some(m) if seen.dir_names[m.dir as usize] == d => m.files.push(c),
                _ => {
                    let st = dir_stamp(d);
                    let dir = seen.dir_id(d);
                    let anchor = if st.is_none() {
                        anchor(d).map(|(a, n)| (seen.dir_id(a), n.to_vec()))
                    } else {
                        None
                    };
                    missed.push(Missed {
                        stamp: st,
                        kept: st.is_none_or(|s| quiet(&s)),
                        dir,
                        anchor,
                        files: vec![c],
                    });
                }
            }
        }
        let found_at = f.as_ref().map(|f| {
            let d = seen.dir_id(crate::inotify::split(&f.name).0);
            (f.name.clone(), d)
        });
        let l = Lookup {
            found: found_at,
            trail: missed,
            checked: None,
        };
        match seen.lookup(name, kind) {
            Some(old) => *old = l,
            None => seen
                .lookups
                .entry(name.to_vec())
                .or_default()
                .push((kind, l)),
        }
        found.ok().flatten()
    }

    fn unchanged(&mut self, loads: &[partex_core::host::Load<'_>]) -> Vec<bool> {
        let Some(seen) = &mut self.seen else {
            return vec![false; loads.len()];
        };
        // (the watcher's events since the last look taken first: a check
        // made now knows them all)
        if seen.watch.is_none() && !seen.unwatched {
            seen.watch = crate::inotify::Watcher::new();
            seen.unwatched = seen.watch.is_none();
        }
        if let Some(w) = seen.watch.as_mut() {
            w.look();
        }
        let now = seen.watch.as_ref().map(crate::inotify::Watcher::now);
        // (each directory looked at once)
        let mut dirs: std::collections::BTreeMap<u32, Option<Stamp>> =
            std::collections::BTreeMap::new();
        loads
            .iter()
            .map(|&(name, kind, last)| {
                let Some(l) = seen
                    .lookups
                    .get_mut(name)
                    .and_then(|ks| ks.iter_mut().find(|(k, _)| *k == kind))
                    .map(|(_, l)| l)
                else {
                    return false;
                };
                let w = &mut seen.watch;
                let names = &seen.dir_names;
                // (as it was when last checked, if none of its names had an
                // event since: its candidates still absent, its file as read)
                let quiet = w.as_ref().zip(l.checked).is_some_and(|(w, c)| {
                    l.trail.iter().all(|m| match (&m.stamp, &m.anchor) {
                        // (a directory not there, still not made)
                        (None, Some((a, name))) => w.quiet(*a, [&name[..]], c),
                        _ => w.quiet(m.dir, m.files.iter().map(|f| crate::inotify::split(f).1), c),
                    }) && l
                        .found
                        .as_ref()
                        .is_none_or(|(p, d)| w.quiet(*d, [crate::inotify::split(p).1], c))
                });
                let same = if quiet {
                    match (&l.found, last) {
                        (None, None) => true,
                        (Some((p, _)), Some(last)) => seen.files.get(p).is_some_and(|(_, a)| {
                            std::sync::Arc::ptr_eq(a, last) || a[..] == last[..]
                        }),
                        _ => false,
                    }
                } else {
                    // (its directories watched before it is checked, so a
                    // change after the check is an event the next look takes)
                    if let Some(w) = w.as_mut() {
                        let found = l.found.as_ref().map(|&(_, d)| d);
                        let dirs = l.trail.iter().map(|m| match (&m.stamp, &m.anchor) {
                            (None, Some((a, _))) => *a,
                            _ => m.dir,
                        });
                        for d in dirs.chain(found) {
                            w.watch(d, &names[d as usize]);
                        }
                    }
                    // (a directory with its stamp holds none of the
                    // candidates; in one changed since, each is looked at:
                    // not a file, as kpathsea's `readable_file` asks)
                    l.trail.iter().all(|m| {
                        let now = *dirs
                            .entry(m.dir)
                            .or_insert_with(|| dir_stamp(&names[m.dir as usize]));
                        (m.kept && m.stamp == now)
                            || m.files
                                .iter()
                                .all(|f| !std::fs::metadata(path(f)).is_ok_and(|m| m.is_file()))
                    }) && match (&l.found, last) {
                        // (found where it was, with the stamp it had just
                        // before its contents were read; two names can find
                        // one file)
                        (None, None) => true,
                        (Some((p, _)), Some(last)) => seen.files.get(p).is_some_and(|(st, a)| {
                            stamp(p) == Some(*st)
                                && (std::sync::Arc::ptr_eq(a, last) || a[..] == last[..])
                        }),
                        _ => false,
                    }
                };
                // (one found as it was is checked as of now; one that was
                // not is read again, and checked by its stamps next time)
                l.checked = if same { now } else { None };
                same
            })
            .collect()
    }

    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        let n = with_suffix(name, kind);
        // openclose.c's `open_output`: in the output directory
        let n = self.in_output_dir(&n).unwrap_or(n);
        let file = File::create(path(&n)).ok()?;
        let id = WriteId(self.next_id);
        self.next_id += 1;
        self.opens += 1;
        self.given.insert(id, (n.clone(), self.opens));
        self.files.insert(id, file);
        Some((id, n))
    }

    fn open_write_again(
        &mut self,
        name: &[u8],
        kind: FileKind,
        id: WriteId,
    ) -> Option<(WriteId, Vec<u8>)> {
        let n = with_suffix(name, kind);
        let n = self.in_output_dir(&n).unwrap_or(n);
        let Some((_, at)) = self.given.get_mut(&id).filter(|(p, _)| *p == n) else {
            return self.open_write(name, kind);
        };
        let file = File::create(path(&n)).ok()?;
        self.opens += 1;
        *at = self.opens;
        self.files.insert(id, file);
        Some((id, n))
    }

    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        if let Some(f) = self.files.get_mut(&file) {
            // TeX has no way to report write errors; neither do we.
            let _ = f.write_all(bytes);
        }
    }

    fn close(&mut self, file: WriteId) {
        self.files.remove(&file);
    }

    fn term_write(&mut self, bytes: &[u8]) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }

    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        match std::io::stdin().lock().read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                Some(line)
            }
        }
    }

    fn now(&self) -> DateTime {
        self.clock.start()
    }

    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        crate::zlib::deflate_stream(level, data)
    }

    fn creation_date(&mut self) -> Vec<u8> {
        self.clock.creation_date()
    }

    fn file_mod_date(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let found = self.kpse.find_file(name, partex_kpse::Format::Tex, true)?;
        let changed = std::fs::metadata(path(&found)).ok()?.modified().ok()?;
        let t = match changed.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_secs()).ok()?,
            Err(e) => -i64::try_from(e.duration().as_secs()).ok()?,
        };
        Some(self.clock.file_date(t))
    }

    fn cache_get(&mut self, key: u128) -> Option<Vec<u8>> {
        crate::cache::get(key)
    }

    fn cached(&mut self, key: u128) -> Option<partex_core::host::Memo> {
        self.memo.as_ref()?.get(&key).cloned()
    }

    fn cache(&mut self, key: u128, value: partex_core::host::Memo) {
        if let Some(memo) = &mut self.memo {
            // (a bound, not a policy: forget everything past it)
            if memo.len() >= 1 << 12 {
                memo.clear();
            }
            memo.insert(key, value);
        }
    }

    fn cache_put(&mut self, key: u128, value: &[u8]) {
        crate::cache::put(key, value);
    }

    fn page_sink(&mut self) -> Option<&mut dyn PageSink> {
        if self.dvi_thread { Some(self) } else { None }
    }

    fn seconds_and_micros(&mut self) -> (i32, i32) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // C's `integer` seconds: truncated like web2c's.
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        (t.as_secs() as i32, t.subsec_micros() as i32)
    }
}

/// What a native tool's terminal output says in a converge report: its last
/// line, or every line when it failed (the error comes before the usage).
pub fn tool_summary(term: &[u8], failed: bool) -> String {
    let lines: Vec<_> = term
        .split(|&c| c == b'\n')
        .filter(|l| !l.is_empty())
        .map(String::from_utf8_lossy)
        .collect();
    if failed {
        lines.join("\n  ")
    } else {
        lines
            .last()
            .map(|l| l.clone().into_owned())
            .unwrap_or_default()
    }
}
