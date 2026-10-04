//! Native [`Host`]: real filesystem and terminal.
//!
//! File lookup goes through `partex-kpse`, the native kpathsea port.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, Write};

use partex_core::dviout::{DviWriter, Summary, TooLong};
use partex_core::pageir::Page;
use partex_core::{DateTime, FileKind, Host, OpenedFile, PageSink, Ran, WriteId};

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
    /// Each output file as the link last left it (its stamp, by its
    /// path): a file the job writes that still has it was not edited
    /// since ([`Host::output_edited`]).
    outputs: HashMap<Vec<u8>, Stamp>,
}

/// A lookup's answer: the path found and its bytes.
type Answer = Option<(Vec<u8>, std::sync::Arc<[u8]>)>;

/// What [`Host::unchanged`] answers from (DESIGN 7.17.3, "A rebuild's
/// file checks cost the files that changed").
#[derive(Default)]
struct Seen {
    /// What each path held when last read, with its stamp taken just
    /// before: none for a file whose times were under 2 s old then.
    files: phitex_doc::FxMap<Vec<u8>, (Stamp, std::sync::Arc<[u8]>)>,
    /// Those files under 2 s old, kept until the next check of the loads
    /// ([`Host::unchanged`]): a rebuild that reads one again (a file's
    /// size asked for again and again) is handed the same bytes, shared,
    /// while its stamp is the same.
    racy: phitex_doc::FxMap<Vec<u8>, (Stamp, std::sync::Arc<[u8]>)>,
    /// Each name's answer since the last check of the loads, by kind,
    /// until a file is opened to write: a lookup asked again (a file's
    /// size asked for again and again) is answered as it was, without
    /// its search and `stat`s.
    again: phitex_doc::FxMap<Vec<u8>, Vec<(FileKind, Answer)>>,
    /// Whether a check of the loads was made ([`Host::unchanged`]: an SSA
    /// session's, at each rebuild and trip): `racy` and `again` are kept
    /// only from one to the next. A session that makes none (a plain
    /// build, machine mode, a run to convergence) reads a file again
    /// whenever it asks for it, as it may have changed since.
    checking: bool,
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

/// A lookup: the path it found (its directory's number, and where its
/// name in the directory begins), and its trail, the candidates it tried
/// and did not find, by directory.
struct Lookup {
    found: Option<(Vec<u8>, u32, usize)>,
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
/// `figs`), which the watcher watches for it. Each candidate's path is
/// kept with where its name in the directory begins.
struct Missed {
    dir: u32,
    anchor: Option<(u32, Vec<u8>)>,
    stamp: Option<Stamp>,
    kept: bool,
    files: Vec<(Vec<u8>, usize)>,
}

/// Where the name of `p` in its directory begins.
fn name_start(p: &[u8]) -> usize {
    p.len() - crate::inotify::split(p).1.len()
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
            formats: None,
            memo: (!std::env::var("PARTEX_T1_CACHE").is_ok_and(|v| v == "0"))
                .then(std::collections::BTreeMap::new),
            seen: std::env::var_os("PARTEX_STAT_CACHE")
                .is_none_or(|v| v != "0")
                .then(Seen::default),
            outputs: HashMap::new(),
        }
    }

    /// The link wrote output file `n` (its path): as it is now, it is the
    /// build's ([`Host::output_edited`]).
    pub fn note_written(&mut self, n: &[u8]) {
        match stamp(n) {
            Some(s) => {
                self.outputs.insert(n.to_vec(), s);
            }
            None => {
                self.outputs.remove(n);
            }
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
            && let Some((kept, c)) = seen
                .files
                .get(p)
                .or_else(|| seen.racy.get(p).filter(|_| seen.checking))
            && *kept == s
        {
            return Some(c.clone());
        }
        let contents: std::sync::Arc<[u8]> = std::fs::read(path(p)).ok()?.into();
        if let Some(seen) = &mut self.seen {
            if let Some(st) = st.filter(quiet) {
                seen.files.insert(p.to_vec(), (st, contents.clone()));
                seen.racy.remove(p);
            } else {
                seen.files.remove(p);
                // (too new to keep across checks: kept until the next one)
                match st {
                    Some(st) if seen.checking => {
                        seen.racy.insert(p.to_vec(), (st, contents.clone()))
                    }
                    _ => seen.racy.remove(p),
                };
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
        // openclose.c's `open_input`: the output directory first (not for
        // the build's BibTeX and makeindex, which look their styles and
        // databases up as the CLI's `bibtex` and `makeindex` do: by
        // kpathsea, from the working directory)
        if let Some(p) = self.in_output_dir(name).filter(|_| !tool_kind(kind)) {
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
            FileKind::Bst => partex_kpse::Format::Bst,
            FileKind::Bib => partex_kpse::Format::Bib,
            FileKind::Ist => partex_kpse::Format::Ist,
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

/// web2c's `generic_synctex_get_current_name`: the name `SyncTeX` gives
/// the input file found as `found`, absolute (the working directory, a
/// slash and `found`, when `found` is relative).
pub fn synctex_name(found: &[u8]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    if found.is_empty() || found.starts_with(b"/") {
        return found.to_vec();
    }
    let Ok(cwd) = std::env::current_dir() else {
        return found.to_vec();
    };
    let mut n = cwd.as_os_str().as_bytes().to_vec();
    n.push(b'/');
    n.extend_from_slice(found);
    n
}

/// A style or database of the build's BibTeX or makeindex.
fn tool_kind(kind: FileKind) -> bool {
    matches!(kind, FileKind::Bst | FileKind::Bib | FileKind::Ist)
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
        | FileKind::Bst
        | FileKind::Bib
        | FileKind::Ist
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

/// The stamps of the files under directory `root`, by their paths
/// relative to it, skipping hidden directories (`.git`) and what is past
/// 100 000 entries: what a command `\write18` runs may have written.
fn tree_stamps(root: &[u8]) -> HashMap<Vec<u8>, Stamp> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let mut out = HashMap::new();
    let mut todo: Vec<Vec<u8>> = vec![Vec::new()];
    let mut seen = 0usize;
    while let Some(rel) = todo.pop() {
        let dir = if rel.is_empty() {
            root.to_vec()
        } else {
            [root, b"/", &rel].concat()
        };
        let Ok(entries) = std::fs::read_dir(path(&dir)) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > 100_000 {
                return out;
            }
            let name = e.file_name();
            let name = name.as_bytes();
            let r = if rel.is_empty() {
                name.to_vec()
            } else {
                [&rel[..], b"/", name].concat()
            };
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if !name.starts_with(b".") {
                    todo.push(r);
                }
            } else if m.is_file() {
                out.insert(
                    r,
                    (
                        m.size(),
                        m.mtime(),
                        m.mtime_nsec(),
                        m.ctime(),
                        m.ctime_nsec(),
                        m.ino(),
                        m.dev(),
                    ),
                );
            }
        }
    }
    out
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
        if let Some(p) = self.in_output_dir(name).filter(|_| !tool_kind(kind)) {
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
            FileKind::Bst => partex_kpse::Format::Bst,
            FileKind::Bib => partex_kpse::Format::Bib,
            FileKind::Ist => partex_kpse::Format::Ist,
            FileKind::Other => return stamp(name).map(|_| name.to_vec()),
        };
        self.kpse.find_file(name, format, true)
    }
}

impl Host for NativeHost {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        if let Some(seen) = &self.seen
            && seen.checking
            && let Some((_, a)) = seen
                .again
                .get(name)
                .and_then(|ks| ks.iter().find(|(k, _)| *k == kind))
        {
            return a.as_ref().map(|(n, c)| OpenedFile {
                name: n.clone(),
                contents: c.clone(),
            });
        }
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
            let at = name_start(&c);
            match missed.last_mut() {
                Some(m) if seen.dir_names[m.dir as usize] == d => m.files.push((c, at)),
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
                        files: vec![(c, at)],
                    });
                }
            }
        }
        let found_at = f.as_ref().map(|f| {
            let d = seen.dir_id(crate::inotify::split(&f.name).0);
            (f.name.clone(), d, name_start(&f.name))
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
        if seen.checking {
            let answer = f.as_ref().map(|f| (f.name.clone(), f.contents.clone()));
            seen.again
                .entry(name.to_vec())
                .or_default()
                .push((kind, answer));
        }
        found.ok().flatten()
    }

    fn unchanged(&mut self, loads: &[partex_core::host::Load<'_>]) -> Vec<bool> {
        let Some(seen) = &mut self.seen else {
            return vec![false; loads.len()];
        };
        // (a file under 2 s old is read again from here on, and every
        // name looked up again)
        seen.racy.clear();
        seen.again.clear();
        seen.checking = true;
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
                        _ => w.quiet(m.dir, m.files.iter().map(|(f, at)| &f[*at..]), c),
                    }) && l
                        .found
                        .as_ref()
                        .is_none_or(|(p, d, at)| w.quiet(*d, [&p[*at..]], c))
                });
                let same = if quiet {
                    match (&l.found, last) {
                        (None, None) => true,
                        (Some((p, ..)), Some(last)) => seen.files.get(p).is_some_and(|(_, a)| {
                            std::sync::Arc::ptr_eq(a, last) || a[..] == last[..]
                        }),
                        _ => false,
                    }
                } else {
                    // (its directories watched before it is checked, so a
                    // change after the check is an event the next look takes)
                    if let Some(w) = w.as_mut() {
                        let found = l.found.as_ref().map(|&(_, d, _)| d);
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
                            || m.files.iter().all(|(f, _)| {
                                !std::fs::metadata(path(f)).is_ok_and(|m| m.is_file())
                            })
                    }) && match (&l.found, last) {
                        // (found where it was, with the stamp it had just
                        // before its contents were read; two names can find
                        // one file)
                        (None, None) => true,
                        (Some((p, ..)), Some(last)) => seen.files.get(p).is_some_and(|(st, a)| {
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
        if let Some(seen) = &mut self.seen {
            seen.racy.remove(&n);
            seen.again.clear();
        }
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
        if let Some(seen) = &mut self.seen {
            seen.racy.remove(&n);
            seen.again.clear();
        }
        let file = File::create(path(&n)).ok()?;
        self.opens += 1;
        *at = self.opens;
        self.files.insert(id, file);
        Some((id, n))
    }

    fn written_name(&mut self, name: &[u8]) -> Vec<u8> {
        // (kpathsea names a file found in the current directory `./name`)
        match self.in_output_dir(name) {
            Some(n) => n,
            None if name.contains(&b'/') => name.to_vec(),
            None => [&b"./"[..], name].concat(),
        }
    }

    fn output_edited(&mut self, name: &[u8]) -> bool {
        // (the file the job wrote under this name, in the output directory
        // if there is one; edited if it is not as the link left it)
        let n = self.in_output_dir(name).unwrap_or_else(|| name.to_vec());
        match self.outputs.get(&n) {
            Some(s) => stamp(&n).as_ref() != Some(s),
            None => true,
        }
    }

    fn open_write_later(
        &mut self,
        name: &[u8],
        kind: FileKind,
        again: Option<WriteId>,
    ) -> Option<(WriteId, Vec<u8>)> {
        let n = with_suffix(name, kind);
        let n = self.in_output_dir(&n).unwrap_or(n);
        // (the same handle for the same file; its file is left as it is:
        // not emptied, so `opened_after` does not count it)
        if let Some(id) = again
            && self.given.get(&id).is_some_and(|(p, _)| *p == n)
        {
            return Some((id, n));
        }
        let id = WriteId(self.next_id);
        self.next_id += 1;
        self.given.insert(id, (n.clone(), 0));
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

    /// web2c's `runsystem`'s `system`: `/bin/sh -c`, with kpathsea's
    /// variables in the environment (latexminted's `latexrestricted`
    /// finds TeX Live by `SELFAUTOLOC`) and `TEXMF_OUTPUT_DIRECTORY` for
    /// `-output-directory`. The files it made or changed are found by
    /// comparing the output directory's stamps before and after.
    fn system(
        &mut self,
        command: &[u8],
        inputs: &[(Vec<u8>, std::sync::Arc<[u8]>)],
    ) -> Option<Ran> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::process::ExitStatusExt;
        // (the job's files as the build holds them where the command runs)
        for (name, contents) in inputs {
            let p = self.in_output_dir(name).unwrap_or_else(|| name.clone());
            if std::fs::read(path(&p)).ok().as_deref() != Some(&contents[..]) {
                let _ = std::fs::write(path(&p), &contents[..]);
            }
        }
        let root = self.output_dir.clone().unwrap_or_else(|| b".".to_vec());
        // (files this host writes, as the DVI thread may while the command
        // runs, are not the command's)
        let ours: std::collections::HashSet<Vec<u8>> =
            self.given.values().map(|(n, _)| n.clone()).collect();
        let before = tree_stamps(&root);
        let _ = std::io::stdout().flush();
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(std::ffi::OsStr::from_bytes(command));
        for (k, v) in self.kpse.exported() {
            cmd.env(
                std::ffi::OsStr::from_bytes(k),
                std::ffi::OsStr::from_bytes(v),
            );
        }
        if let Some(d) = &self.output_dir {
            cmd.env("TEXMF_OUTPUT_DIRECTORY", std::ffi::OsStr::from_bytes(d));
        }
        // (`system`'s status: the shell's own 127 if it could not start;
        // its standard output goes to the terminal through the engine)
        let (status, stdout) = cmd
            .stdin(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_or((127 << 8, Vec::new()), |o| (o.status.into_raw(), o.stdout));
        if status != 0 {
            eprintln!("system returned with code {status}");
        }
        let mut wrote = Vec::new();
        let after = tree_stamps(&root);
        // (a file in the output directory is found by its name there, and
        // by the path to it: `-output-directory=out` gives `x` and `out/x`)
        let names = |rel: &[u8]| -> Vec<Vec<u8>> {
            if root == b"." {
                vec![rel.to_vec()]
            } else {
                vec![rel.to_vec(), [&root[..], b"/", rel].concat()]
            }
        };
        let removed = before
            .keys()
            .filter(|r| !after.contains_key(*r))
            .flat_map(|r| names(r))
            .collect();
        for (rel, st) in &after {
            let full = names(rel).pop().unwrap_or_default();
            if before.get(rel) == Some(st) || ours.contains(&full) || ours.contains(rel) {
                continue;
            }
            if let Some(seen) = &mut self.seen {
                seen.racy.remove(&full);
            }
            if let Ok(c) = std::fs::read(path(&full)) {
                let c: std::sync::Arc<[u8]> = std::sync::Arc::from(c);
                wrote.extend(names(rel).into_iter().map(|n| (n, c.clone())));
            }
        }
        // (a lookup made before the command is made again)
        if let Some(seen) = &mut self.seen {
            seen.again.clear();
        }
        Some(Ran {
            status,
            wrote,
            removed,
            stdout,
        })
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

    fn synctex_name(&mut self, found: &[u8]) -> Vec<u8> {
        synctex_name(found)
    }

    fn output_name(&mut self, name: &[u8], kind: FileKind) -> Vec<u8> {
        let n = with_suffix(name, kind);
        self.in_output_dir(&n).unwrap_or(n)
    }

    fn remove_output(&mut self, name: &[u8]) {
        let n = self.in_output_dir(name).unwrap_or_else(|| name.to_vec());
        let _ = std::fs::remove_file(path(&n));
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
