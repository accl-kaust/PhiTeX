//! `PARTEX_MACHINE=1`: the job run through the language-independent
//! runtime (`partex_incr::Build`) as a `TexMachine` (DESIGN.md §7.0),
//! its output linked from the regions' effects (§7.6). With
//! `PARTEX_MACHINE_EDIT=path|from|to` the build is followed by a rebuild
//! after replacing `from` by `to` in the input file whose path ends with
//! `path`, and the rebuild's output is written instead (several rebuilds
//! separated by `;;`; with `PARTEX_MACHINE_STOP=n` every one but the last
//! stops early at its n-th look, as a watch does for a newer edit, and
//! with `n,k` only the first k do).
//!
//! The host is part of the machine's state: it serves input files as
//! cells (the first contents served for each path, shared by every clone,
//! unless a rebuild's starting state overrides them) and keeps the
//! `\write` files in memory, since the job may read them back. Nothing is
//! written to disk until the link.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use partex_core::machine::{CellHost, CellTracker, MCell, TexMachine};
use partex_core::{DateTime, FileKind, Host, OpenedFile, Params, Tex, WriteId};
use partex_incr::Build;

use crate::native::{NativeHost, with_suffix};

// (builds kept across processes: `persisted.rs`)
#[path = "persisted.rs"]
pub mod persisted;
#[path = "quick.rs"]
pub mod quick;

type Files = BTreeMap<Vec<u8>, Arc<[u8]>>;
/// Files not found, by cell key: the name and kind looked for.
type Missed = BTreeMap<Vec<u8>, (Vec<u8>, FileKind)>;
/// Where the lines of the files served begin, by name (with the contents
/// they were found in): the last two contents of each, as a rebuild asks
/// for the lines of the old and the new contents in turn (an edit that
/// moved every later line compares each, which would otherwise find the
/// lines of the whole file again for each line).
type Lines = BTreeMap<Vec<u8>, Vec<partex_core::machine::FileLines>>;

#[derive(Clone)]
pub struct MachineHost {
    native: Arc<Mutex<NativeHost>>,
    served: Arc<Mutex<Files>>,
    overrides: Arc<Files>,
    lines: Arc<Mutex<Lines>>,
    /// Output files by id: the name they were opened with.
    opened: BTreeMap<u32, Vec<u8>>,
    /// `XeTeX`'s PDF files among them: what is written is the XDV, which
    /// xdvipdfmx makes the PDF of when the files are written out
    /// (`FileKind::XdvPipe`).
    piped: BTreeSet<u32>,
    /// What was written to them through the host (the `\write` files).
    written: BTreeMap<u32, WrittenBuf>,
    /// Appended to and read back since the machine last looked.
    appended: BTreeMap<u32, Vec<u8>>,
    read_back: Vec<u32>,
    next_id: u32,
    reads: Vec<Vec<u8>>,
    /// Files looked for and not found, by their cell's key ([`miss_key`]):
    /// the name and kind looked for (a watch looks again, and the file
    /// appearing changes the cell).
    missed: Arc<Mutex<Missed>>,
    /// The terminal lines read so far (shared: a region run again reads
    /// the same lines), and how many this state has read.
    term_lines: Arc<Mutex<Vec<Option<Vec<u8>>>>>,
    term_pos: usize,
    now: DateTime,
    epoch: (i32, i32),
    /// The clock was read since the machine last looked.
    clock_read: ReadFlag,
    /// Content-keyed memos shared by every clone (`PARTEX_MACHINE_MEMO=0`:
    /// none).
    memo: Option<Arc<MemoStore>>,
    /// The fonts' slots by identity, shared by every clone and every run
    /// of the build (`Host::font_slot`).
    fonts: Arc<Mutex<FontSlots>>,
    /// The output files a command (`\write18`) removed since the job last
    /// opened them, shared by every clone: the link leaves them removed.
    removed: Arc<Mutex<BTreeSet<Vec<u8>>>>,
}

/// Fonts' slots by what they are (`Host::font_slot`): a font keeps its
/// slot in every run of a build however many fonts were loaded before it,
/// and a slot, once given, is never given to another font. In a cold
/// build every slot is tex.web's next number. `PARTEX_MACHINE_FONTSLOTS=0`
/// turns it off (every font at tex.web's next number).
#[derive(Clone, Debug, Default)]
struct FontSlots {
    on: bool,
    slots: BTreeMap<u128, i32>,
    /// Above every slot given.
    next: i32,
}

impl partex_core::persist::Persist for FontSlots {
    fn save(&self, s: &mut partex_core::persist::Saver) {
        self.on.save(s);
        self.slots.save(s);
        self.next.save(s);
    }
    fn load(l: &mut partex_core::persist::Loader) -> Option<Self> {
        use partex_core::persist::Persist;
        Some(Self {
            on: Persist::load(l)?,
            slots: Persist::load(l)?,
            next: Persist::load(l)?,
        })
    }
}

/// The clock as a file the host serves (no path begins with a NUL): the
/// day and minute of `\time`, `\day`, `\month` and `\year`, the PDF
/// creation date, the start of `\pdfelapsedtime`, and the settings that
/// fix them. Reading any of them reads this file, a cell like any other:
/// a build saved on another day finds it changed, and re-runs only what
/// read it (DESIGN.md §7.9).
const CLOCK: &[u8] = b"\0clock";

/// The clock's contents (see [`CLOCK`]).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Clock {
    now: DateTime,
    creation: Vec<u8>,
    epoch: (i32, i32),
    /// `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE`, as written (empty:
    /// neither is set).
    fixed: String,
}

impl Clock {
    /// The clock now.
    fn read(native: &mut NativeHost) -> Self {
        Self {
            now: native.now(),
            creation: native.creation_date(),
            epoch: native.seconds_and_micros(),
            fixed: match (
                std::env::var("SOURCE_DATE_EPOCH"),
                std::env::var("FORCE_SOURCE_DATE"),
            ) {
                (Err(_), Err(_)) => String::new(),
                (e, f) => format!("{} {}", e.unwrap_or_default(), f.unwrap_or_default()),
            },
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let d = &self.now;
        let mut v = format!(
            "{:04}-{:02}-{:02} {}\n{} {}\n{}\n",
            d.year, d.month, d.day, d.minutes, self.epoch.0, self.epoch.1, self.fixed
        )
        .into_bytes();
        v.extend_from_slice(&self.creation);
        v
    }

    fn parse(b: &[u8]) -> Option<Self> {
        let mut parts = b.splitn(4, |&c| c == b'\n');
        let (date, epoch, fixed, creation) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let date = std::str::from_utf8(date).ok()?;
        let (ymd, minutes) = date.split_once(' ')?;
        let mut ymd = ymd.split('-').map(str::parse::<i32>);
        let now = DateTime {
            year: ymd.next()?.ok()?,
            month: ymd.next()?.ok()?,
            day: ymd.next()?.ok()?,
            minutes: minutes.parse().ok()?,
        };
        let (s, us) = std::str::from_utf8(epoch).ok()?.split_once(' ')?;
        Some(Self {
            now,
            creation: creation.to_vec(),
            epoch: (s.parse().ok()?, us.parse().ok()?),
            fixed: std::str::from_utf8(fixed).ok()?.to_owned(),
        })
    }

    /// The clock a build saved as `old` reads now: the same unless the day
    /// or the settings changed (a session keeps its clock through a day,
    /// as a resident session does); then today's, always with the old
    /// start of `\pdfelapsedtime` (every state holds it, so a new one
    /// would run all again; a machine's elapsed time is that start
    /// against itself).
    fn since(old: &[u8], native: &mut NativeHost) -> Option<Vec<u8>> {
        let now = Self::read(native);
        let Some(was) = Self::parse(old) else {
            return Some(now.bytes());
        };
        let same_day = (was.now.year, was.now.month, was.now.day)
            == (now.now.year, now.now.month, now.now.day);
        if same_day && now.fixed == was.fixed {
            return None;
        }
        let new = Self {
            epoch: was.epoch,
            ..now
        };
        Some(new.bytes()).filter(|b| b.as_slice() != old)
    }
}

/// What a no-op restart's check asks of the host ([`quick::check`]).
impl quick::Look for NativeHost {
    fn clock_changed(&mut self, old: &[u8]) -> bool {
        Clock::since(old, self).is_some()
    }

    fn found(&mut self, name: &[u8], kind: FileKind) -> bool {
        self.read_file(name, kind).is_some()
    }
}

/// A flag set through `&self` (`Host::now` reads the clock), copied with
/// its host.
#[derive(Default)]
struct ReadFlag(std::sync::atomic::AtomicBool);

impl Clone for ReadFlag {
    fn clone(&self) -> Self {
        Self(std::sync::atomic::AtomicBool::new(
            self.0.load(std::sync::atomic::Ordering::Relaxed),
        ))
    }
}

impl ReadFlag {
    fn set(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn take(&self) -> bool {
        self.0.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
}

/// A `\write` file's contents, in shared pieces and a short tail: every
/// snapshot holds the host, and a flat buffer would be copied whole into
/// each one that appends after it.
#[derive(Clone, Default)]
struct WrittenBuf {
    pieces: Vec<Arc<[u8]>>,
    tail: Vec<u8>,
}

impl WrittenBuf {
    const PIECE: usize = 4096;

    fn extend(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        if self.tail.len() >= Self::PIECE {
            self.pieces.push(Arc::from(core::mem::take(&mut self.tail)));
        }
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(
            self.pieces.iter().map(|p| p.len()).sum::<usize>() + self.tail.len(),
        );
        for p in &self.pieces {
            v.extend_from_slice(p);
        }
        v.extend_from_slice(&self.tail);
        v
    }
}

/// Content-keyed memos (`Host::cached`), up to a size.
#[derive(Default)]
pub struct MemoStore {
    map: Mutex<BTreeMap<u128, partex_core::host::Memo>>,
    deflated: Mutex<Deflated>,
}

/// The streams the link compressed, by their contents (`Host::deflate`):
/// a page's content stream is compressed again only when its bytes
/// change. Kept across processes in the store (`Watch`'s
/// `deflated_file`): a build after an edit compresses only the pages that
/// changed, as a watch does.
#[derive(Default)]
struct Deflated {
    /// The file the table is kept in, and whether it was read.
    file: Option<std::path::PathBuf>,
    read: bool,
    /// Each stream compressed (by its key), and whether this process
    /// used it: those are kept.
    streams: BTreeMap<u128, (Arc<[u8]>, bool)>,
    /// The bytes of the streams used.
    used: usize,
}

/// The bound on the bytes of compressed streams kept for the next
/// process.
const DEFLATED_MAX: usize = 1 << 29;

impl Deflated {
    /// The table kept in `file`, read once.
    fn read(&mut self) {
        if self.read {
            return;
        }
        self.read = true;
        let Some(bytes) = self.file.as_ref().and_then(|f| std::fs::read(f).ok()) else {
            return;
        };
        // (each stream: its key, its length, its bytes; a short file
        // ends the table where it breaks)
        let mut rest = &bytes[..];
        while rest.len() >= 20 {
            let key = u128::from_le_bytes(rest[..16].try_into().unwrap_or_default());
            let n = u32::from_le_bytes(rest[16..20].try_into().unwrap_or_default()) as usize;
            let Some(z) = rest.get(20..20 + n) else { break };
            self.streams.entry(key).or_insert((Arc::from(z), false));
            rest = &rest[20 + n..];
        }
    }

    /// Stream `key`, marked used.
    fn get(&mut self, key: u128) -> Option<Arc<[u8]>> {
        self.read();
        let (z, used) = self.streams.get_mut(&key)?;
        if !*used {
            *used = true;
            self.used += z.len();
        }
        Some(z.clone())
    }

    fn put(&mut self, key: u128, z: Arc<[u8]>) {
        let n = z.len();
        if self.used + n <= DEFLATED_MAX
            && self.streams.insert(key, (z, true)).is_none_or(|(_, u)| !u)
        {
            self.used += n;
        }
    }

    /// Write the streams used to the file (whole: renamed into place).
    fn write(&self) {
        let Some(file) = &self.file else { return };
        let mut out = Vec::with_capacity(self.used + 20 * self.streams.len());
        for (key, (z, used)) in &self.streams {
            if *used && let Ok(n) = u32::try_from(z.len()) {
                out.extend_from_slice(&key.to_le_bytes());
                out.extend_from_slice(&n.to_le_bytes());
                out.extend_from_slice(z);
            }
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, &out).is_ok() && std::fs::rename(&tmp, file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

impl MemoStore {
    fn deflated(&self) -> std::sync::MutexGuard<'_, Deflated> {
        self.deflated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn get(&self, key: u128) -> Option<partex_core::host::Memo> {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
    }

    fn put(&self, key: u128, value: partex_core::host::Memo) {
        let mut map = self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // (a bound, not a policy: forget everything past it)
        if map.len() >= 1 << 16 {
            map.clear();
        }
        map.insert(key, value);
    }
}

impl MachineHost {
    /// Keep the streams compressed in `file` across processes
    /// ([`Deflated`]): read before the first stream is compressed.
    fn keep_deflated(&self, file: std::path::PathBuf) {
        if let Some(memo) = &self.memo {
            let mut d = memo.deflated();
            if d.file.as_ref() != Some(&file) {
                d.file = Some(file);
                d.read = false;
            }
        }
    }

    /// Write the streams this process used to the file they are kept in.
    fn write_deflated(&self) {
        if let Some(memo) = &self.memo {
            memo.deflated().write();
        }
    }

    fn new(mut native: NativeHost) -> Self {
        let clock = Clock::read(&mut native);
        let (now, epoch) = (clock.now, clock.epoch);
        let served: Files = [(CLOCK.to_vec(), Arc::from(clock.bytes()))].into();
        Self {
            native: Arc::new(Mutex::new(native)),
            served: Arc::new(Mutex::new(served)),
            overrides: Arc::default(),
            lines: Arc::default(),
            opened: BTreeMap::new(),
            piped: BTreeSet::new(),
            written: BTreeMap::new(),
            appended: BTreeMap::new(),
            read_back: Vec::new(),
            next_id: 0,
            reads: Vec::new(),
            missed: Arc::default(),
            term_lines: Arc::default(),
            term_pos: 0,
            now,
            epoch,
            clock_read: ReadFlag::default(),
            memo: (!std::env::var("PARTEX_MACHINE_MEMO").is_ok_and(|v| v == "0"))
                .then(Arc::default),
            fonts: Arc::new(Mutex::new(FontSlots {
                on: !std::env::var("PARTEX_MACHINE_FONTSLOTS").is_ok_and(|v| v == "0"),
                ..FontSlots::default()
            })),
            removed: Arc::default(),
        }
    }

    /// The clock this state reads (marked read).
    fn clock(&self) -> Option<Clock> {
        self.clock_read.set();
        self.file(CLOCK).and_then(|b| Clock::parse(&b))
    }

    /// The output files a command removed since the job last opened them.
    fn removed_files(&self) -> BTreeSet<Vec<u8>> {
        self.removed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn native(&self) -> std::sync::MutexGuard<'_, NativeHost> {
        self.native
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The paths served so far.
    fn served_paths(&self) -> Vec<Vec<u8>> {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

/// The cell key of a file looked for as `name` of `kind` and not found
/// (no path begins with a NUL).
fn miss_key(name: &[u8], kind: FileKind) -> Vec<u8> {
    let mut k = format!("\0{kind:?}\0").into_bytes();
    k.extend_from_slice(name);
    k
}

/// `name` without leading `./` (`./texsys.aux` is `texsys.aux`).
fn undotted(mut name: &[u8]) -> &[u8] {
    while let Some(rest) = name.strip_prefix(b"./") {
        name = rest;
    }
    name
}

impl MachineHost {
    /// The file this job wrote last under one of `names` (`same` compares
    /// a written name with an asked one, both without a leading `./`),
    /// read back.
    fn written_file(
        &mut self,
        names: &[Option<Vec<u8>>],
        same: impl Fn(&[u8], &[u8]) -> bool,
    ) -> Option<OpenedFile> {
        for n in names.iter().flatten() {
            if let Some((&id, o)) = self
                .opened
                .iter()
                .rev()
                .find(|(_, o)| same(undotted(o), undotted(n)))
                && let Some(w) = self.written.get(&id)
            {
                self.read_back.push(id);
                // (named as kpathsea finds a file in the current
                // directory, as the session host does; a name with a
                // directory, as under `-output-directory`, as it is; the
                // name written, which a case-insensitive match differs from)
                let n = [&n[..n.len() - undotted(n).len()], undotted(o)].concat();
                let name = if n.starts_with(b"/") || n.starts_with(b".") || n.contains(&b'/') {
                    n
                } else {
                    [&b"./"[..], &n[..]].concat()
                };
                return Some(OpenedFile {
                    name,
                    contents: Arc::from(w.to_vec()),
                });
            }
        }
        None
    }
}

impl Host for MachineHost {
    fn wants_streams(&self) -> bool {
        crate::view::tapping()
    }

    fn stream_shipped(&mut self, page: Option<usize>, stream: partex_core::pagepdf::ShippedStream) {
        crate::view::shipped(page, stream);
    }

    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        // a file this job wrote (the last opened with that name)
        let out = self.native().in_output_dir(name);
        let names = [
            out.clone(),
            out.map(|o| with_suffix(&o, kind)),
            Some(name.to_vec()),
            Some(with_suffix(name, kind)),
        ];
        if let Some(f) = self.written_file(&names, |o, n| o == n) {
            return Some(f);
        }
        let found = self.native().read_file(name, kind);
        let Some(f) = found else {
            // (kpathsea's case-insensitive search, `texmf_casefold_search`,
            // of a name it found nowhere: a file this job wrote under
            // another case)
            if self.native().casefold()
                && let Some(f) = self.written_file(&names, |o, n| {
                    let ((od, ob), (nd, nb)) = (crate::inotify::split(o), crate::inotify::split(n));
                    od == nd && ob.eq_ignore_ascii_case(nb)
                })
            {
                return Some(f);
            }
            // (a file not found is a cell too: absent until it appears)
            let key = miss_key(name, kind);
            if let Some(c) = self.overrides.get(&key) {
                // (it appeared: found where a rebuild's state says)
                self.reads.push(key.clone());
                let name = self
                    .native()
                    .in_output_dir(name)
                    .unwrap_or_else(|| name.to_vec());
                return Some(OpenedFile {
                    name,
                    contents: c.clone(),
                });
            }
            self.missed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.clone(), (name.to_vec(), kind));
            self.reads.push(key);
            return None;
        };
        let contents = match self.overrides.get(&f.name) {
            Some(c) => c.clone(),
            None => self
                .served
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(f.name.clone())
                .or_insert(f.contents)
                .clone(),
        };
        self.reads.push(f.name.clone());
        Some(OpenedFile {
            name: f.name,
            contents,
        })
    }

    fn open_write(&mut self, name: &[u8], kind: FileKind) -> Option<(WriteId, Vec<u8>)> {
        let n = with_suffix(name, kind);
        let n = self.native().in_output_dir(&n).unwrap_or(n);
        let id = self.next_id;
        self.next_id += 1;
        self.removed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&n);
        self.opened.insert(id, n.clone());
        if kind == FileKind::XdvPipe {
            self.piped.insert(id);
        }
        Some((WriteId(id), n))
    }

    fn runs_commands(&self) -> bool {
        true
    }

    fn synctex_name(&mut self, found: &[u8]) -> Vec<u8> {
        crate::native::synctex_name(found)
    }

    fn output_name(&mut self, name: &[u8], kind: FileKind) -> Vec<u8> {
        let n = with_suffix(name, kind);
        self.native().in_output_dir(&n).unwrap_or(n)
    }

    fn out_name_ok(&mut self, name: &[u8]) -> bool {
        self.native().out_name_ok(name)
    }

    /// A command, run by the native host over the job's files: this host
    /// keeps them in memory until the link, so each name's last opened
    /// file is put on disk first; the files the command removes stay
    /// removed (DESIGN 3.7, "Commands").
    fn system(
        &mut self,
        command: &[u8],
        inputs: &[(Vec<u8>, Arc<[u8]>)],
    ) -> Option<partex_core::Ran> {
        let mut last: BTreeMap<&[u8], u32> = BTreeMap::new();
        for (id, name) in &self.opened {
            last.insert(name, *id);
        }
        for (name, id) in last {
            let bytes = self
                .written
                .get(&id)
                .map(WrittenBuf::to_vec)
                .unwrap_or_default();
            let p = crate::native::path(name);
            if std::fs::read(&p).ok().as_deref() != Some(&bytes[..]) {
                let _ = std::fs::write(&p, &bytes);
            }
        }
        let ran = self.native().system(command, inputs)?;
        let mut removed = self
            .removed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for n in &ran.removed {
            removed.insert(self.native().in_output_dir(n).unwrap_or_else(|| n.clone()));
        }
        drop(removed);
        Some(ran)
    }

    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        self.written.entry(file.0).or_default().extend(bytes);
        self.appended
            .entry(file.0)
            .or_default()
            .extend_from_slice(bytes);
    }

    fn close(&mut self, _: WriteId) {}

    fn term_write(&mut self, _: &[u8]) {}

    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        let lines = self.term_lines.clone();
        let mut lines = lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.term_pos == lines.len() {
            let line = self.native().term_read_line();
            lines.push(line);
        }
        self.term_pos += 1;
        lines[self.term_pos - 1].clone()
    }

    fn now(&self) -> DateTime {
        self.clock().map_or(self.now, |c| c.now)
    }

    fn deflate(&mut self, level: i32, data: &[u8]) -> Option<Vec<u8>> {
        // (content-keyed: a page or font shipped again compresses alike,
        // in this process or, through the store, the next)
        let Some(memo) = &self.memo else {
            return crate::zlib::deflate_stream(level, data);
        };
        let key = partex_core::StableHasher::of(&(b"deflate", level, data));
        if let Some(z) = memo.deflated().get(key) {
            return Some(z.to_vec());
        }
        let z = crate::zlib::deflate_stream(level, data)?;
        memo.deflated().put(key, Arc::from(&z[..]));
        Some(z)
    }

    fn cached(&mut self, key: u128) -> Option<partex_core::host::Memo> {
        self.memo.as_ref()?.get(key)
    }

    fn cache(&mut self, key: u128, value: partex_core::host::Memo) {
        if let Some(memo) = &self.memo {
            memo.put(key, value);
        }
    }

    fn font_slot(&mut self, ident: u128, fresh: i32) -> i32 {
        let mut r = self
            .fonts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !r.on {
            return fresh;
        }
        if let Some(&s) = r.slots.get(&ident) {
            return s;
        }
        let s = fresh.max(r.next);
        r.next = s + 1;
        r.slots.insert(ident, s);
        s
    }

    fn creation_date(&mut self) -> Vec<u8> {
        match self.clock() {
            Some(c) => c.creation,
            None => self.native().creation_date(),
        }
    }

    fn file_mod_date(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.native().file_mod_date(name)
    }

    fn seconds_and_micros(&mut self) -> (i32, i32) {
        self.clock().map_or(self.epoch, |c| c.epoch)
    }

    fn notes(&self) -> bool {
        self.native().notes
    }
}

impl CellHost for MachineHost {
    fn nanos(&self) -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        u64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(0)
    }
    fn take_reads(&mut self) -> Vec<Vec<u8>> {
        if self.clock_read.take() {
            self.reads.push(CLOCK.to_vec());
        }
        std::mem::take(&mut self.reads)
    }

    fn file(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.overrides.get(path).cloned().or_else(|| {
            self.served
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(path)
                .cloned()
        })
    }

    fn set_file(&mut self, path: &[u8], contents: Option<Arc<[u8]>>) {
        let o = Arc::make_mut(&mut self.overrides);
        match contents {
            Some(c) => {
                o.insert(path.to_vec(), c);
            }
            None => {
                o.remove(path);
            }
        }
    }

    fn lines(&self, path: &[u8]) -> Option<partex_core::machine::FileLines> {
        let data = self.file(path)?;
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let kept = lines.entry(path.to_vec()).or_default();
        if let Some((_, starts)) = kept.iter().find(|(d, _)| Arc::ptr_eq(d, &data)) {
            return Some((data, starts.clone()));
        }
        let starts = partex_core::machine::line_starts(&data);
        kept.insert(0, (data.clone(), starts.clone()));
        kept.truncate(2);
        Some((data, starts))
    }

    fn digest(&self) -> u128 {
        // (what the files hold is `Written` cells')
        let mut h = partex_core::StableHasher::new();
        std::hash::Hash::hash(&(self.next_id, &self.opened, self.term_pos), &mut h);
        h.finish128()
    }

    fn clone_state_from(&self, snapshot: &Self) -> Self {
        let mut h = snapshot.clone();
        h.overrides = self.overrides.clone();
        // (what the `\write` files hold is cells of their own: this
        // state's)
        h.written = self.written.clone();
        h.reads.clear();
        h.appended.clear();
        h.read_back.clear();
        h
    }

    fn written(&self, id: u32) -> Option<Arc<Vec<u8>>> {
        self.written.get(&id).map(|w| Arc::new(w.to_vec()))
    }

    fn written_ids(&self) -> Vec<u32> {
        self.written.keys().copied().collect()
    }

    fn append_written(&mut self, id: u32, bytes: &[u8]) {
        self.written.entry(id).or_default().extend(bytes);
    }

    fn reads_back(&self, name: &[u8]) -> bool {
        let name = undotted(name);
        self.opened
            .iter()
            .any(|(id, o)| undotted(o) == name && self.written.contains_key(id))
    }

    fn take_written(&mut self) -> (BTreeMap<u32, Vec<u8>>, Vec<u32>) {
        (
            std::mem::take(&mut self.appended),
            std::mem::take(&mut self.read_back),
        )
    }
}

type Machine = TexMachine<MachineHost>;

/// Write the outputs and end: the build is not torn down (the process
/// exits; freeing every trace would take longer than the link).
fn finish(b: Build<Machine>) -> i32 {
    let h = write_outputs(&b);
    std::mem::forget(b);
    h
}

/// The in-process path's link after a rebuild, as a watch session links
/// (`Session::write`): nothing when no region's effects changed since the
/// last link (the files are as they were), else the regions put in
/// resolved again and the others taken from the last link
/// (`effects::link_cached`), and only the files whose bytes changed
/// written. `PARTEX_LINK_SPLICE=0`: a full link and every file written,
/// each time, as before.
#[derive(Default)]
struct Linker {
    cache: partex_core::effects::LinkCache,
    last: Option<partex_core::effects::Linked>,
    /// The bytes last written to each file, by their hash.
    written: BTreeMap<Vec<u8>, u128>,
    /// The last link's time, and its write's.
    timed: (std::time::Duration, std::time::Duration),
    /// The hash of the `SyncTeX` file last written.
    synctex: Option<u128>,
}

impl Linker {
    /// A linker with the cold build `b`'s output linked and written, as a
    /// watch links it before the first edit.
    fn cold(b: &mut Build<Machine>) -> Self {
        let mut l = Self::default();
        l.link_write(b, true);
        l
    }

    /// After a rebuild: its link, as a watch links it (one edit's link);
    /// none for a rebuild stopped for a newer edit (`done` false).
    fn after_rebuild(&mut self, b: &mut Build<Machine>, done: bool) {
        self.timed = Default::default();
        if done {
            self.link_write(b, false);
        }
    }

    /// The last link's terminal text out; the job's `history`.
    fn finish(self, b: Build<Machine>) -> i32 {
        if let Some(l) = &self.last {
            let _ = std::io::Write::write_all(&mut std::io::stdout(), &l.term);
        }
        let h = b.final_state().history().unwrap_or(3);
        std::mem::forget(b);
        h
    }

    /// Link `b`'s output and write what changed; the job's `history`. The
    /// message is `bench/edits.sh`'s link column unless `quiet` (the cold
    /// build's link, which a watch makes before the first edit).
    fn link_write(&mut self, b: &mut Build<Machine>, quiet: bool) -> i32 {
        let splice = !std::env::var("PARTEX_LINK_SPLICE").is_ok_and(|v| v == "0");
        let t = Instant::now();
        let changed = b.take_effects_changed();
        let touched = b.take_touched();
        let reuse = splice && !changed && self.last.is_some();
        let mut times = partex_core::effects::LinkTimes::default();
        let fin = b.final_state();
        if !reuse {
            let threads = partex_incr::Threads::available();
            let mut deflating = fin.tex().host().clone();
            let mut deflate = |level, data: &[u8]| deflating.deflate(level, data);
            let linked = if splice {
                let fx: Vec<(u64, &[partex_core::effects::Effect])> = b
                    .keyed_traces()
                    .map(|(k, t)| (k, t.effects.as_slice()))
                    .collect();
                let touched = |k: u64| touched.as_ref().is_none_or(|t| t.contains(&k));
                partex_core::effects::link_cached_timed(
                    &fx,
                    &touched,
                    &mut self.cache,
                    &threads,
                    &mut deflate,
                    &clock,
                    &mut times,
                )
            } else {
                let fx: Vec<&[partex_core::effects::Effect]> =
                    b.traces().map(|t| t.effects.as_slice()).collect();
                partex_core::effects::link(&fx, &threads, &mut deflate)
            };
            match linked {
                Ok(l) => self.last = Some(l),
                Err(e) => {
                    eprintln!("phitex: the link step failed: {e:?}");
                    std::process::exit(3);
                }
            }
        }
        let t_link = t.elapsed();
        let Some(linked) = &self.last else {
            return 3;
        };
        if splice
            && (cfg!(debug_assertions)
                || std::env::var("PARTEX_MACHINE_SANITIZE").is_ok_and(|v| v == "1"))
        {
            check_full_link(b, linked);
        }
        let host = fin.tex().host();
        let t_write = Instant::now();
        let (mut bytes_out, mut pdf_bytes) = (0usize, 0usize);
        let removed = host.removed_files();
        for (id, name) in host.opened.iter().filter(|(_, n)| !removed.contains(*n)) {
            let bytes: std::borrow::Cow<'_, [u8]> = match linked.files.get(id) {
                Some(b) => std::borrow::Cow::Borrowed(b),
                None => match host.written.get(id) {
                    Some(w) => std::borrow::Cow::Owned(w.to_vec()),
                    None => continue,
                },
            };
            if name.ends_with(b".pdf") {
                pdf_bytes = bytes.len();
            }
            // (a file from a link taken again is as last written)
            if reuse && linked.files.contains_key(id) && self.written.contains_key(name) {
                continue;
            }
            let h = quick::hash(&bytes);
            if splice && self.written.get(name) == Some(&h) {
                continue;
            }
            let bytes = host.piped_out(*id, name, bytes);
            let _ = std::fs::write(crate::native::path(name), &bytes);
            bytes_out += bytes.len();
            self.written.insert(name.clone(), h);
        }
        if !reuse || self.synctex.is_none() {
            self.synctex = write_synctex(b, self.synctex);
        }
        let t_write = t_write.elapsed();
        self.timed = (t_link, t_write);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if quiet {
            eprintln!("phitex: machine: the cold build's output in {ms:.1} ms");
        } else {
            eprintln!(
                "phitex: machine: linked {} regions in {ms:.1} ms",
                b.stats.regions
            );
        }
        if partex_core::machine::CUT_TIMING.load(std::sync::atomic::Ordering::Relaxed) {
            let how = if reuse {
                "the last link taken again"
            } else if splice {
                "linked, cached"
            } else {
                "linked in full"
            };
            let written = (t_write, bytes_out, pdf_bytes);
            report_link(quiet, how, &self.cache, &times, t_link, written);
        }
        fin.history().unwrap_or(3)
    }
}

/// Panic unless `linked` is what a full link of `b`'s output makes, files
/// and terminal text (the sanitizer's check of [`Linker`]).
fn check_full_link(b: &Build<Machine>, linked: &partex_core::effects::Linked) {
    let fx: Vec<&[partex_core::effects::Effect]> =
        b.traces().map(|t| t.effects.as_slice()).collect();
    let mut deflating = b.final_state().tex().host().clone();
    let full = partex_core::effects::link(&fx, &partex_incr::Threads::available(), &mut |l, d| {
        deflating.deflate(l, d)
    });
    assert!(
        full.as_ref()
            .is_ok_and(|f| f.files == linked.files && f.term == linked.term),
        "sanitizer: the link taken again or cached is not a full link's"
    );
}

/// `PARTEX_CUT_TIMING`: a link's phases ([`Linker`]); `written` is the
/// time the files took, the bytes written and the PDF's size.
fn report_link(
    quiet: bool,
    how: &str,
    cache: &partex_core::effects::LinkCache,
    times: &partex_core::effects::LinkTimes,
    t_link: std::time::Duration,
    written: (std::time::Duration, usize, usize),
) {
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let ms_of = |ns: u64| ns as f64 / 1e6;
    let (t_write, bytes_out, pdf_bytes) = written;
    eprintln!(
        "phitex: machine: link timing{}: {how} ({} regions resolved, {} as last time); ms: numbering {:.1}, resolve {:.1}, layout {:.1} (object streams {:.1}, cross-reference {:.1}), lengths {:.1}, copy {:.1}; link {:.1}, files hashed and written {:.1} ({bytes_out} bytes written); the PDF {pdf_bytes} bytes",
        if quiet { " (cold)" } else { "" },
        cache.resolved,
        cache.reused,
        ms_of(times.numbering),
        ms_of(times.resolve),
        ms_of(times.layout),
        ms_of(times.objstm),
        ms_of(times.xref),
        ms_of(times.lengths),
        ms_of(times.copy),
        t_link.as_secs_f64() * 1e3,
        t_write.as_secs_f64() * 1e3,
    );
}

/// The build's `SyncTeX` file (DESIGN 4.5): its regions' events rendered
/// as the final state ends the job, its names in the output directory:
/// the file's, the other kind's (an earlier run's, removed), and its bytes
/// (none: no file, both removed). `None` without a job name.
pub(crate) fn synctex_file(b: &Build<Machine>) -> Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
    let fin = b.final_state().tex();
    let events = b
        .traces()
        .flat_map(|t| t.effects.iter())
        .filter_map(|e| match e {
            partex_core::effects::Effect::Synctex(v) => Some(v.iter()),
            _ => None,
        })
        .flatten();
    let f = fin.synctex_regions_file(events, &mut |text| crate::zlib::deflate_once(6, text))?;
    let native = fin.host().native();
    let at = |n: Vec<u8>| native.in_output_dir(&n).unwrap_or(n);
    Some((at(f.name), at(f.other), f.bytes))
}

/// Write `bytes` to `path` whole: renamed into place, so a reader never
/// finds half a file.
fn write_whole(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("partex-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Write the build's `SyncTeX` file ([`synctex_file`]) and remove the
/// other kind's (or both, with no file); `last` is the hash of the bytes
/// last written, which are not written again. The new hash.
fn write_synctex(b: &Build<Machine>, last: Option<u128>) -> Option<u128> {
    let (name, other, bytes) = synctex_file(b)?;
    let remove = |n: &[u8]| {
        let p = crate::native::path(n);
        if p.exists() {
            let _ = std::fs::remove_file(p);
        }
    };
    remove(&other);
    let Some(bytes) = bytes else {
        remove(&name);
        return None;
    };
    let h = quick::hash(&bytes);
    let p = crate::native::path(&name);
    if last != Some(h) || !p.exists() {
        let _ = write_whole(&p, &bytes);
    }
    Some(h)
}

/// Link a build's output and write its files; the job's `history`.
fn write_outputs(b: &Build<Machine>) -> i32 {
    write_synctex(b, None);
    let fx: Vec<&[partex_core::effects::Effect]> =
        b.traces().map(|t| t.effects.as_slice()).collect();
    link_and_write(&fx, b.final_state())
}

/// Link the regions' effects `fx` and write the files of the run that
/// ended in `fin`; the job's `history`.
fn link_and_write(fx: &[&[partex_core::effects::Effect]], fin: &Machine) -> i32 {
    let threads = partex_incr::Threads::available();
    let t = Instant::now();
    // (the host's deflate, memoized by content: the link compresses the
    // streams, most of them as the last link did)
    let mut deflating = fin.tex().host().clone();
    let linked = partex_core::effects::link(fx, &threads, &mut |level, data| {
        deflating.deflate(level, data)
    });
    let host = fin.tex().host();
    let linked = match linked {
        Ok(l) => l,
        Err(e) => {
            eprintln!("phitex: the link step failed: {e:?}");
            std::process::exit(3);
        }
    };
    let written: BTreeMap<u32, Vec<u8>> = host
        .written
        .iter()
        .filter(|(id, _)| !linked.files.contains_key(id))
        .map(|(id, w)| (*id, w.to_vec()))
        .collect();
    let removed = host.removed_files();
    for (id, bytes) in linked.files.iter().chain(&written) {
        if let Some(name) = host.opened.get(id)
            && !removed.contains(name)
        {
            let bytes = host.piped_out(*id, name, bytes.into());
            let _ = std::fs::write(crate::native::path(name), bytes);
        }
    }
    eprintln!(
        "phitex: machine: linked {} regions in {:.1} ms",
        fx.len(),
        t.elapsed().as_secs_f64() * 1e3
    );
    let _ = std::io::Write::write_all(&mut std::io::stdout(), &linked.term);
    fin.history().unwrap_or(3)
}

/// The resident set now and at its peak (`PARTEX_MACHINE_RSS=1`; Linux).
pub(crate) fn rss() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .map_or("?", |l| l[name.len()..].trim())
            .to_string()
    };
    format!("rss {} peak {}", field("VmRSS:"), field("VmHWM:"))
}

/// Nanoseconds since the first call (the runtime's clock).
fn clock() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn config() -> partex_incr::build::Config {
    // (a cold region costs a snapshot and a hash of what changed in it,
    // so cold regions are coarse; where a rebuild re-executes, fine)
    let var = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    partex_incr::build::Config {
        grain: var("PARTEX_MACHINE_GRAIN", 131_072),
        fine_grain: var("PARTEX_MACHINE_FINE_GRAIN", 1024),
        grow: var("PARTEX_MACHINE_GROW", 1),
        file_cut: var("PARTEX_MACHINE_FILE_CUT", 16_384),
        sanitize: std::env::var("PARTEX_MACHINE_SANITIZE").is_ok_and(|v| v == "1"),
        clock: Some(clock),
        audit: std::env::var("PARTEX_MACHINE_AUDIT").is_ok_and(|v| v == "1"),
        ..partex_incr::build::Config::default()
    }
}

/// The cells an edit of the file `key` from `old` to `new` changes: the
/// file, and every line that differs (past the shorter end, every line of
/// the longer).
fn changed_cells(key: &Arc<[u8]>, old: &[u8], new: &[u8]) -> Vec<MCell> {
    let mut changed = vec![MCell::File(key.clone())];
    let (a, b) = (
        partex_core::track::file_lines(old),
        partex_core::track::file_lines(new),
    );
    for k in 0..a.len().max(b.len()) {
        let la = a.get(k).map(|l| &old[l.0..l.2]);
        let lb = b.get(k).map(|l| &new[l.0..l.2]);
        if la != lb {
            changed.push(MCell::Line(
                key.clone(),
                u32::try_from(k).unwrap_or(u32::MAX),
            ));
        }
    }
    changed
}

/// Where a rebuild's final state differs from a fresh build's (for
/// debugging).
fn final_differences(b: &Build<Machine>, cfg: &partex_incr::build::Config) {
    // (where the rebuild's final state differs from a fresh build's)
    let fresh = Build::new(b.initial().clone(), cfg);
    let (x, y) = (
        b.final_state().tex().state_hash_parts(),
        fresh.final_state().tex().state_hash_parts(),
    );
    for (p, q) in x.iter().zip(&y) {
        if p.1 != q.1 {
            eprintln!("phitex: machine: final states differ in {}", p.0);
        }
    }
    let (bt, ft) = (b.final_state().tex(), fresh.final_state().tex());
    eprintln!("phitex: machine: final pdf: {}", bt.pdf_objs_difference(ft));
    eprintln!(
        "phitex: machine: glyphs: {}; fresh: {}",
        b.final_state()
            .glyph_differences(fresh.final_state(), b.traces()),
        b.final_state()
            .glyph_differences(fresh.final_state(), fresh.traces())
    );
    {
        use partex_incr::Machine as _;
        eprintln!(
            "phitex: machine: digests {:?} {:?}",
            b.final_state().digest(),
            fresh.final_state().digest()
        );
    }
    for c in bt.eqtb_differences(ft).into_iter().take(10) {
        if let partex_core::track::Cell::Eqtb(p) = c {
            eprintln!(
                "phitex: machine: eqtb differs: {} {:?} {:?}",
                bt.eqtb_loc_name(p),
                bt.eqtb_cell_hash(p),
                ft.eqtb_cell_hash(p)
            );
        }
    }
}

/// The edits `spec` applied ([`edited_with_maps`]): the starting state
/// and the cells they change, with `b` renamed first through how the
/// changed files' lines moved (`PARTEX_MACHINE_RENAME=1`; otherwise `b`
/// is left as it is).
fn edited(b: &mut Build<Machine>, spec: &str) -> (Machine, Vec<MCell>) {
    let (new, changed, maps) = edited_with_maps(b, spec);
    partex_core::machine::rename_lines(b, &maps);
    (new, changed)
}

/// The starting state after the edits `spec` (`path|from|to`, several
/// separated by `||`: in the input file whose path ends with `path`,
/// the first `from` becomes `to`), the cells they change, and with
/// `PARTEX_MACHINE_RENAME=1` how each changed file's lines moved
/// (`LineMap`; the changed lines are then the hunk's new ones).
fn edited_with_maps(
    b: &Build<Machine>,
    spec: &str,
) -> (Machine, Vec<MCell>, Vec<partex_core::machine::LineMap>) {
    let (new, changed) = edited_by_index(b, spec);
    if !partex_core::machine::renaming() {
        return (new, changed, Vec::new());
    }
    // (per changed file: the old contents, the edited ones, one hunk)
    let host = b.initial().tex().host();
    let mut maps = Vec::new();
    let mut cells = Vec::new();
    for c in &changed {
        let MCell::File(key) = c else { continue };
        let old = host.file(key).unwrap_or_else(|| Arc::from(&b""[..]));
        let now = new
            .tex()
            .host()
            .file(key)
            .unwrap_or_else(|| Arc::from(&b""[..]));
        let (map, lines) = line_hunk(key, &old, &now);
        cells.push(MCell::File(key.clone()));
        cells.extend(lines);
        maps.push(map);
    }
    (new, cells, maps)
}

/// The common prefix and suffix of two contents' lines: the map of the
/// lines between (`LineMap`), and the new lines in the hunk as cells.
fn line_hunk(
    key: &Arc<[u8]>,
    old: &[u8],
    new: &[u8],
) -> (partex_core::machine::LineMap, Vec<MCell>) {
    let (was, now) = (
        partex_core::track::file_lines(old),
        partex_core::track::file_lines(new),
    );
    let old_line = |k: usize| &old[was[k].0..was[k].2];
    let new_line = |k: usize| &new[now[k].0..now[k].2];
    let mut prefix = 0;
    while prefix < was.len() && prefix < now.len() && old_line(prefix) == new_line(prefix) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < was.len() - prefix
        && suffix < now.len() - prefix
        && old_line(was.len() - 1 - suffix) == new_line(now.len() - 1 - suffix)
    {
        suffix += 1;
    }
    let num = |k: usize| u32::try_from(k).unwrap_or(u32::MAX);
    let map = partex_core::machine::LineMap {
        file: key.clone(),
        prefix: num(prefix),
        old_end: num(was.len() - suffix),
        new_end: num(now.len() - suffix),
    };
    let cells = (prefix..now.len() - suffix)
        .map(|k| MCell::Line(key.clone(), num(k)))
        .collect();
    (map, cells)
}

/// [`edited_with_maps`] with the changed lines by index (without renaming: every
/// line after an insertion changed).
fn edited_by_index(b: &Build<Machine>, spec: &str) -> (Machine, Vec<MCell>) {
    let mut new = b.initial().clone();
    let mut changed = Vec::new();
    // (the starting state's files: after a rebuild that stopped early the
    // final state is still the one before it)
    let host = b.initial().tex().host();
    for edit in spec.split("||") {
        let mut parts = edit.splitn(3, '|');
        let (Some(path), Some(from), Some(to)) = (parts.next(), parts.next(), parts.next()) else {
            eprintln!("phitex: PARTEX_MACHINE_EDIT is path|from|to[||path|from|to...]");
            std::process::exit(2);
        };
        let Some(key) = host
            .served_paths()
            .into_iter()
            .find(|p| p.ends_with(path.as_bytes()))
        else {
            eprintln!("phitex: machine: no input file ends with {path}");
            std::process::exit(2);
        };
        let old = new
            .tex()
            .host()
            .file(&key)
            .unwrap_or_else(|| Arc::from(&b""[..]));
        let text = String::from_utf8_lossy(&old).replacen(from, to, 1);
        if text.as_bytes() == &old[..] {
            eprintln!("phitex: machine: `{from}` is not in {path}");
            std::process::exit(2);
        }
        new.tex_mut()
            .host_mut()
            .set_file(&key, Some(Arc::from(text.as_bytes())));
        let original = host.file(&key).unwrap_or_else(|| Arc::from(&b""[..]));
        changed.retain(|c| !matches!(c, MCell::File(k) | MCell::Line(k, _) if **k == *key));
        changed.extend(changed_cells(&key.into(), &original, text.as_bytes()));
    }
    (new, changed)
}

/// `PARTEX_MACHINE_ROUNDS=threads`: the edited job run in parallel rounds
/// on `threads` threads, warm from the build `b` (a previous session's
/// regions, merged into `PARTEX_MACHINE_REGIONS` regions, by default four
/// per thread), and its output written.
fn rounds(b: &Build<Machine>, new: &Machine, threads: usize) -> i32 {
    let regions = std::env::var("PARTEX_MACHINE_REGIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4 * threads);
    let t = Instant::now();
    let warm = partex_incr::rounds::Warm::from_build(b, regions);
    let cfg = partex_incr::rounds::Config {
        parts: regions,
        holes: partex_incr::rounds::Holes::Off,
        ..partex_incr::rounds::Config::default()
    };
    let merged = t.elapsed();
    let t = Instant::now();
    let out = partex_incr::rounds::run_warm(new, &cfg, &partex_incr::Threads(threads), Some(&warm));
    eprintln!(
        "phitex: machine: rounds on {threads} threads in {:.2} s ({} regions merged in {:.2} s): {:?}",
        t.elapsed().as_secs_f64(),
        warm.regions(),
        merged.as_secs_f64(),
        out.stats
    );
    let fx: Vec<&[partex_core::effects::Effect]> = out
        .traces
        .iter()
        .map(|(t, _)| t.effects.as_slice())
        .collect();
    let h = link_and_write(&fx, &out.final_state);
    std::mem::forget(out);
    h
}

/// The parts of the state a replay of the build's regions from its start
/// gets wrong: the first region whose guards fail, and what in `Rest` or
/// the input stack differs from the state it really began in; with
/// `PARTEX_MACHINE_REPLAYCHECK=2`, the first region after which the
/// replayed eqtb differs from its own exit (a write the tracker lost).
/// For the sanitizer's diagnostics (`PARTEX_MACHINE_REPLAYCHECK=1`).
/// [`replay_differences`], with `PARTEX_MACHINE_REPLAYCHECK` set.
fn replay_check(b: &Build<Machine>) {
    if std::env::var("PARTEX_MACHINE_REPLAYCHECK").is_ok_and(|v| v != "0") {
        replay_differences(b);
    }
}

#[allow(clippy::too_many_lines)] // (diagnostics, one pass)
fn replay_differences(b: &Build<Machine>) {
    use partex_incr::{Machine as _, version_of};
    let mut s = b.initial().clone();
    let mut prev: Option<&partex_incr::Trace<Machine>> = None;
    for (i, t) in b.traces().enumerate() {
        let failed: Vec<&MCell> = t
            .guards
            .iter()
            .filter(|(c, v)| version_of(&s.get(c)) != *v)
            .map(|(c, _)| c)
            .collect();
        if !failed.is_empty() {
            eprintln!(
                "phitex: machine: a replay fails region {i}'s guards on {} cells: {:?}",
                failed.len(),
                &failed[..failed.len().min(6)]
            );
            for c in failed.iter().take(4) {
                if let MCell::Eqtb(p) = c {
                    eprintln!(
                        "phitex: machine:   {} replayed {} ",
                        s.tex().eqtb_loc_name(*p),
                        s.tex().cs_debug_at(*p)
                    );
                }
            }
            // (the state the region really began in: its predecessor's
            // exit snapshot)
            if let Some(entry) = prev.and_then(|p| {
                for c in failed.iter().take(4) {
                    if let MCell::Eqtb(q) = c
                        && let Some(e) = p
                            .writes
                            .iter()
                            .find(|(x, _, _)| *x == MCell::Rest)
                            .and_then(|(_, v, _)| v.rest_tex())
                    {
                        eprintln!(
                            "phitex: machine:   {} really {} ",
                            e.eqtb_loc_name(*q),
                            e.cs_debug_at(*q)
                        );
                    }
                }
                p.writes
                    .iter()
                    .find(|(c, _, _)| *c == MCell::Rest)
                    .and_then(|(_, v, _)| v.rest_tex())
            }) {
                for (x, y) in s
                    .tex()
                    .rest_hash_parts()
                    .iter()
                    .zip(entry.rest_hash_parts())
                {
                    if x.1 != y.1 {
                        eprintln!("phitex: machine: replayed Rest differs in {}", x.0);
                    }
                }
                let (a, e) = (s.tex().input_stack_debug(), entry.input_stack_debug());
                for (k, (x, y)) in a.iter().zip(&e).enumerate() {
                    if x != y {
                        eprintln!("phitex: machine: input record {k}: replayed {x} / real {y}");
                    }
                }
                if a.len() != e.len() {
                    eprintln!(
                        "phitex: machine: input stacks {} / {} deep",
                        a.len(),
                        e.len()
                    );
                }
                for (x, y) in s.tex().pdf_hash_parts().iter().zip(entry.pdf_hash_parts()) {
                    if x.1 != y.1 {
                        eprintln!("phitex: machine: replayed pdf differs in {}", x.0);
                    }
                }
            }
            return;
        }
        t.apply(&mut s, &[]);
        prev = Some(t);
        if std::env::var("PARTEX_MACHINE_REPLAYCHECK").is_ok_and(|v| v == "2")
            && let Some(exit) = t
                .writes
                .iter()
                .find(|(c, _, _)| *c == MCell::Rest)
                .and_then(|(_, v, _)| v.rest_tex())
        {
            // (the first region after which the replayed eqtb differs
            // from the region's own exit)
            let d = s.tex().eqtb_differences(&exit);
            if !d.is_empty() {
                eprintln!(
                    "phitex: machine: after region {i} a replay's eqtb differs in {} cells",
                    d.len()
                );
                for c in d.iter().take(4) {
                    if let partex_core::track::Cell::Eqtb(p) = c {
                        eprintln!(
                            "phitex: machine:   {}: replayed {} / exit {}; written: {}",
                            s.tex().eqtb_loc_name(*p),
                            s.tex().cs_debug_at(*p),
                            exit.cs_debug_at(*p),
                            t.writes.iter().any(|(c, _, _)| *c == MCell::Eqtb(*p))
                        );
                    }
                }
                return;
            }
        }
    }
    let fin = b.final_state();
    for (x, y) in s.digest_parts().iter().zip(fin.digest_parts()) {
        if x.1 != y.1 {
            eprintln!(
                "phitex: machine: a replay differs from the final state in {}",
                x.0
            );
        }
    }
    let (a, f) = (s.tex(), fin.tex());
    eprintln!(
        "phitex: machine: replayed font slots: {}",
        a.font_slot_differences(f)
    );
    eprintln!(
        "phitex: machine: replayed fonts: {} / {} slots, order {} / {}, font_ptr {} / {}",
        a.font_slot_count(),
        f.font_slot_count(),
        a.font_order_len(),
        f.font_order_len(),
        a.font_count(),
        f.font_count()
    );
}

/// What an edit changed in the final state's `Rest` for good: why no
/// early cutoff (for debugging).
fn lasting_differences(before: &Machine, after: &Machine) {
    // (what stays different after the edit: why no early cutoff)
    let (x, y) = (
        before.tex().rest_hash_parts(),
        after.tex().rest_hash_parts(),
    );
    for (p, q) in x.iter().zip(&y) {
        if p.1 != q.1 {
            eprintln!("phitex: machine: the edit changed {} for good", p.0);
        }
    }
    eprintln!(
        "phitex: machine: before {}",
        before.tex().input_files_debug()
    );
    eprintln!("phitex: machine: after {}", after.tex().input_files_debug());
    eprintln!(
        "phitex: machine: pdf {}",
        before.tex().pdf_objs_difference(after.tex())
    );
    for (p, q) in before
        .tex()
        .pdf_hash_parts()
        .iter()
        .zip(&after.tex().pdf_hash_parts())
    {
        if p.1 != q.1 {
            eprintln!("phitex: machine: the edit changed pdf {} for good", p.0);
        }
    }
}

/// The machine's switches, from the environment (`PARTEX_MACHINE_*=0`
/// turns one off).
/// The rebuild's statistics line, and its cuts' timing.
fn report_rebuild(
    done: bool,
    t: Instant,
    stats: &impl std::fmt::Debug,
) -> Option<partex_core::machine::CutTiming> {
    eprintln!(
        "phitex: machine: {} in {:.3} s: {stats:?}",
        if done { "rebuilt" } else { "stopped" },
        t.elapsed().as_secs_f64(),
    );
    cut_timing("rebuild")
}

/// `PARTEX_CUT_TIMING`: one keystroke's timeline, from the edit to the
/// files written, in milliseconds by phase; `wall` is the keystroke's
/// (the edit applied through the link, before the idle work), `idle` the
/// composition after it (`between_edits`). The phases sum to `wall`
/// but for what the report calls unaccounted.
fn report_keystroke(
    stats: &partex_incr::build::Stats,
    cut: &partex_core::machine::CutTiming,
    [edit, rebuild, link, write, idle, wall]: [std::time::Duration; 6],
) {
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let ms = |ns: u64| ns as f64 / 1e6;
    let d = |x: std::time::Duration| x.as_secs_f64() * 1e3;
    let cuts =
        cut.tok_commit + cut.jvec_commits + cut.flat_commits + cut.rest_hash + cut.snapshot_of;
    // (the snapshot each replay takes is in the cuts' sums and in replay)
    let replay_cut = cut.replay[3];
    let run_cuts = cuts.saturating_sub(replay_cut);
    let restore = cut.replay[0];
    let replay_rest = cut.replay[4].saturating_sub(restore);
    let replay_other = stats.replay_ns.saturating_sub(cut.replay[4]);
    let [start, splice, drop] = stats.other_parts_ns;
    let other = stats.other_ns.saturating_sub(start + splice + drop);
    let inside = stats.run_ns + stats.replay_ns + stats.compare_ns + stats.other_ns;
    let parts = d(edit) + ms(inside) + d(link) + d(write);
    eprintln!(
        "phitex: machine: keystroke timeline, ms: edit applied and diffed {:.2}; start (initial state, changed cells marked) {:.2}; TeX re-run {:.2} and its {} cuts {:.2}; {} replays: restore_rest {:.2}, the rest of replay_exit (cells set, its snapshot) {:.2}, other replay (exit patches, single regions) {:.2}; compare {:.2}; splice {:.2}; old states dropped {:.2}; other {:.2}; rebuild not in its stats {:.2}; link {:.2}; write {:.2}; wall {:.2} (unaccounted {:.2}); idle work after it {:.2}",
        d(edit),
        ms(start),
        ms(stats.run_ns.saturating_sub(run_cuts)),
        cut.cuts.saturating_sub(cut.replays),
        ms(run_cuts),
        cut.replays,
        ms(restore),
        ms(replay_rest),
        ms(replay_other),
        ms(stats.compare_ns),
        ms(splice),
        ms(drop),
        ms(other),
        d(rebuild) - ms(inside),
        d(link),
        d(write),
        d(wall),
        d(wall) - parts - (d(rebuild) - ms(inside)),
        d(idle),
    );
}

/// `PARTEX_CUT_TIMING=1`: what the cuts since the last call cost, by
/// piece (DESIGN.md §7.16.3), in microseconds per cut.
fn cut_timing(what: &str) -> Option<partex_core::machine::CutTiming> {
    if !partex_core::machine::CUT_TIMING.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let t = partex_core::machine::cut_timing_take();
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let us = |ns: u64| ns as f64 / t.cuts.max(1) as f64 / 1e3;
    eprintln!(
        "phitex: machine: cut timing, {what}: {} cuts; µs per cut: token store commit {:.1}, token store clone {:.1}, JVec commits {:.1}, Flat commits {:.1}, Rest hash {:.1}, Snapshot::of {:.1}; at the last cut {} lists, {} free, {} pooled",
        t.cuts,
        us(t.tok_commit),
        us(t.tok_clone),
        us(t.jvec_commits),
        us(t.flat_commits),
        us(t.rest_hash),
        us(t.snapshot_of),
        t.lists,
        t.free,
        t.pooled
    );
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let per = |x: u64| x as f64 / t.cuts.max(1) as f64;
    for (j, name) in ["eqtb", "hash", "save_stack"].iter().enumerate() {
        let v = t.jvec[j];
        eprintln!(
            "phitex: machine: cut timing, {what}: JVec {name}: µs per cut walk {:.1}, compare {:.1}, copy {:.1}; {} chunks, dirty {:.1} per cut (at most {}), copied {:.1}",
            us(v[0]),
            us(v[1]),
            us(v[2]),
            v[3],
            per(v[4]),
            v[5],
            per(v[6])
        );
    }
    eprintln!(
        "phitex: machine: cut timing, {what}: save_ptr at the last cut {}",
        t.save_ptr
    );
    eprintln!(
        "phitex: machine: cut timing, {what}: Snapshot::of's clone {:.1} µs per cut",
        us(t.clone_whole)
    );
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let per_restore = |ns: u64| ns as f64 / t.restores.max(1) as f64 / 1e3;
    for (name, v) in partex_core::machine::RESTORE_PIECES.iter().zip(t.restore) {
        eprintln!(
            "phitex: machine: cut timing, {what}: restore ({} of them): {name}: {:.1} µs per restore",
            t.restores,
            per_restore(v)
        );
    }
    for (name, v) in partex_core::machine::REPLAY_PIECES.iter().zip(t.replay) {
        #[allow(clippy::cast_precision_loss, reason = "a report")]
        let per = v as f64 / t.replays.max(1) as f64 / 1e3;
        eprintln!(
            "phitex: machine: cut timing, {what}: replay_exit ({} of them): {name}: {per:.1} µs each",
            t.replays
        );
    }
    for (name, v) in partex_core::machine::CLONE_GROUPS
        .iter()
        .zip(t.clone_groups)
    {
        eprintln!(
            "phitex: machine: cut timing, {what}: clone of {name}: {:.1} µs per cut",
            us(v)
        );
    }
    let names = [
        "str_pool",
        "str_start",
        "buffer",
        "input_stack",
        "param_stack",
    ];
    for (i, name) in names.iter().enumerate() {
        eprintln!(
            "phitex: machine: cut timing, {what}: Flat {name}: {:.1} µs per cut; at the last cut {} bytes, {} live",
            us(t.flat[i]),
            t.flat_bytes[i],
            t.flat_live[i]
        );
    }
    Some(t)
}

/// The switches of the process, from the environment: how `Rest` and the
/// other cells are made (`partex_core::statehash`'s and `machine`'s
/// statics). A saved build's digest depends on them, so they are set
/// before one is loaded (`persisted`), not only when a machine
/// is made: else a load in a process whose environment turned one off
/// hashed its starting state with the defaults and never matched.
fn process_switches() {
    let off = |name: &str| std::env::var(name).is_ok_and(|v| v == "0");
    partex_core::machine::CUT_TIMING_DETAIL.store(
        std::env::var("PARTEX_CUT_TIMING").is_ok_and(|v| v == "2"),
        std::sync::atomic::Ordering::Relaxed,
    );
    partex_core::machine::CUT_TIMING.store(
        std::env::var("PARTEX_CUT_TIMING").is_ok_and(|v| v == "1" || v == "2"),
        std::sync::atomic::Ordering::Relaxed,
    );
    partex_core::machine::set_soft_reads(!off("PARTEX_MACHINE_SOFTREADS"));
    partex_core::machine::set_thaw_by_content(!off("PARTEX_MACHINE_THAW_CONTENT"));
    partex_core::machine::set_keep_tracker(!off("PARTEX_MACHINE_KEEP_TRACKER"));
    partex_core::machine::set_clean_cuts(!off("PARTEX_MACHINE_CLEAN_CUTS"));
    partex_core::machine::set_page_cells(!off("PARTEX_MACHINE_PAGE_CELL"));
    partex_core::machine::set_pdf_last_cells(!off("PARTEX_MACHINE_PDF_LAST"));
    partex_core::machine::set_pdf_word_cells(!off("PARTEX_MACHINE_PDF_WORDS"));
    partex_core::machine::set_mark_cells(!off("PARTEX_MACHINE_MARKS"));
    partex_core::machine::set_relocate(!off("PARTEX_MACHINE_RELOCATE"));
    partex_core::machine::set_ship_layer(!off("PARTEX_MACHINE_SHIP_LAYER"));
    partex_core::machine::set_tree_names(!off("PARTEX_MACHINE_TREE_NAMES"));
    partex_core::machine::set_num_answers(!off("PARTEX_MACHINE_NUM_ANSWERS"));
    partex_core::machine::set_canon(
        if std::env::var("PARTEX_MACHINE_POISON").is_ok_and(|v| v == "1") {
            2
        } else {
            u8::from(!off("PARTEX_MACHINE_CANON"))
        },
    );
    // (not with `SyncTeX`: a renamed region's nodes would keep the lines
    // they were placed at, DESIGN 4.5)
    partex_core::machine::set_position_cells(
        std::env::var("PARTEX_MACHINE_RENAME").is_ok_and(|v| v == "1")
            && crate::origins::synctex_option().is_none(),
    );
    partex_core::machine::set_known_rest(
        !off("PARTEX_MACHINE_KNOWN_REST"),
        ["PARTEX_MACHINE_SANITIZE", "PARTEX_MACHINE_VERIFY_REST"]
            .iter()
            .any(|n| std::env::var(n).is_ok_and(|v| v == "1")),
    );
}

/// The switches of machine `m` (its own, and the process's).
fn switches(m: &mut TexMachine<MachineHost>) {
    let off = |name: &str| std::env::var(name).is_ok_and(|v| v == "0");
    process_switches();
    m.set_replay_exit(!off("PARTEX_MACHINE_REPLAY_EXIT"));
    m.set_seal_lines(!off("PARTEX_MACHINE_SEAL"));
    m.set_skips(!off("PARTEX_MACHINE_SKIPS"));
    m.set_thaw_from(!off("PARTEX_MACHINE_THAW"));
    let t = m.tex_mut();
    t.set_symbolic_object_streams(!off("PARTEX_MACHINE_OBJSTM"));
    t.set_canon_strings(!off("PARTEX_MACHINE_STRINGS"));
    t.set_cs_by_name(!off("PARTEX_MACHINE_CSNAMES"));
    t.set_name_cells(!off("PARTEX_MACHINE_NAMES"));
    t.set_probe_names(!off("PARTEX_MACHINE_PROBENAMES"));
    t.set_font_cells(!off("PARTEX_MACHINE_FONTCELLS"));
    t.set_obj_cells(!off("PARTEX_MACHINE_OBJCELLS"));
    t.set_virtual_objects(!off("PARTEX_MACHINE_VIRTOBJ"));
}

/// Run the job as a machine (see the module documentation).
#[allow(clippy::many_single_char_names)] // (b a build, t a clock, …)
pub fn run(native: NativeHost, params: Params, command_line: &[u8]) -> i32 {
    let cfg = config();
    let host = MachineHost::new(native);
    let mut tex = Tex::new(host, CellTracker::default(), params);
    crate::origins::setup_synctex(&mut tex);
    let mut m = TexMachine::new(tex, command_line);
    switches(&mut m);
    if std::env::var("PARTEX_MACHINE_SPLITPROBE").is_ok_and(|v| v == "1") {
        split_probe(&m);
        return 0;
    }
    let t = Instant::now();
    let mut b = Build::new(m, &cfg);
    eprintln!(
        "phitex: machine: built in {:.2} s: {:?}",
        t.elapsed().as_secs_f64(),
        b.stats
    );
    let cold_levels = print_census(&b);
    if std::env::var("PARTEX_MACHINE_PARTS").is_ok_and(|v| v == "9") {
        dump_numbering_observers(&b);
    }
    let Ok(edit) = std::env::var("PARTEX_MACHINE_EDIT") else {
        return finish(b);
    };
    // (what a watch session does while it waits for the first edit: the
    // indexes, and, with `PARTEX_MACHINE_REFINE=1`, fine regions made in
    // parallel; off by default, as a fine region costs a snapshot, which
    // over a whole document is many times a plain run's memory: a first
    // edit in a coarse region re-runs it finely, only there)
    let t = Instant::now();
    let before = cfg
        .sanitize
        .then(|| partex_incr::Machine::digest(b.final_state()));
    let (refined, phases) = if std::env::var("PARTEX_MACHINE_REFINE").is_ok_and(|v| v == "1") {
        b.refine(
            cfg.fine_grain,
            4 * cfg.fine_grain,
            cfg.flat,
            &partex_incr::Threads::available(),
            cfg.clock,
        )
    } else {
        (0, [0; 3])
    };
    let refine = t.elapsed();
    // (the coarse regions replaced: dropped while waiting)
    drop(b.take_garbage());
    if let Some(d) = before {
        replay_check(&b);
        b.check_replay(&d);
        eprintln!("phitex: machine: the refined regions replay to the same state");
    }
    let t = Instant::now();
    b.index();
    eprintln!(
        "phitex: machine: refined {refined} regions in {:.2} s ({phases:?} ns), {} now; indexed in {:.2} s",
        refine.as_secs_f64(),
        b.stats.regions,
        t.elapsed().as_secs_f64()
    );
    if edit == "-" {
        // (no edit: the refined build's own output)
        return finish(b);
    }
    // (successive rebuilds, as a watch session makes them, separated by
    // `;;`)
    let edits: Vec<&str> = edit.split(";;").collect();
    let coarsen: u32 = std::env::var("PARTEX_MACHINE_COARSEN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let last = edits.len() - 1;
    // (the cold build's output, linked as a watch links it before the
    // first edit: the rebuild's link then costs what changed)
    let mut linker = Linker::cold(&mut b);
    for (i, edit) in edits.into_iter().enumerate() {
        let t_edit = Instant::now();
        let (new, changed) = edited(&mut b, edit);
        let edit_time = t_edit.elapsed();
        if std::env::var("PARTEX_MACHINE_PARTS").is_ok_and(|v| v == "4") {
            convergence(&b, &new);
        }
        if i == last
            && let Some(threads) = std::env::var("PARTEX_MACHINE_ROUNDS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
        {
            return rounds(&b, &new, threads);
        }
        let before = std::env::var("PARTEX_MACHINE_PARTS")
            .is_ok_and(|v| v == "3")
            .then(|| b.final_state().clone());
        let step = Keystroke {
            t_edit,
            edit_time,
            i,
            last,
            coarsen,
        };
        step.rebuild(&mut b, &mut linker, new, &changed, &cfg);
        if let Some(before) = before {
            lasting_differences(&before, b.final_state());
        }
        dump_after_rebuild(&b, &cold_levels, edit);
    }
    if std::env::var("PARTEX_MACHINE_PARTS").is_ok_and(|v| v == "1") {
        final_differences(&b, &cfg);
    }
    linker.finish(b)
}

/// One keystroke of the in-process path: when its edit began, how long
/// applying it took, its index and the last one's, the coarsening after.
struct Keystroke {
    t_edit: Instant,
    edit_time: std::time::Duration,
    i: usize,
    last: usize,
    coarsen: u32,
}

impl Keystroke {
    /// The rebuild, its link and the idle work after it, as a watch does
    /// them, and (`PARTEX_CUT_TIMING`) the keystroke's timeline.
    fn rebuild(
        &self,
        b: &mut Build<Machine>,
        linker: &mut Linker,
        new: Machine,
        changed: &[MCell],
        cfg: &partex_incr::build::Config,
    ) {
        let t = Instant::now();
        let done = rebuild_stopping(b, new, changed, cfg, (self.i, self.i == self.last));
        let rebuild = t.elapsed();
        let cut = report_rebuild(done, t, &b.stats);
        linker.after_rebuild(b, done);
        if std::env::var("PARTEX_MACHINE_PARTS").is_ok_and(|v| v == "10") {
            dump_rest_differences(b);
        }
        let (wall, t) = (self.t_edit.elapsed(), Instant::now());
        between_edits(b, cfg, self.coarsen);
        if let Some(cut) = cut {
            let (link, write) = linker.timed;
            let times = [self.edit_time, rebuild, link, write, t.elapsed(), wall];
            report_keystroke(&b.stats, &cut, times);
        }
    }
}

/// Rebuild `i`; with `PARTEX_MACHINE_STOP=n`, unless it is the `last`, one
/// that stops at its `n`-th look, as a watch's does when a newer edit
/// arrives (`n,k`: only the first `k` rebuilds stop). Whether it ran to
/// its end.
fn rebuild_stopping(
    b: &mut Build<Machine>,
    new: Machine,
    changed: &[MCell],
    cfg: &partex_incr::build::Config,
    (i, last): (usize, bool),
) -> bool {
    let spec = std::env::var("PARTEX_MACHINE_STOP").unwrap_or_default();
    let mut parts = spec.split(',');
    let after: Option<u64> = parts.next().and_then(|v| v.parse().ok());
    let first: Option<usize> = parts.next().and_then(|v| v.parse().ok());
    let after = after.filter(|_| !last && first.is_none_or(|k| i < k));
    let looks = std::cell::Cell::new(0u64);
    let stop = || {
        looks.set(looks.get() + 1);
        after.is_some_and(|n| looks.get() >= n)
    };
    let done = b.rebuild_or_stop(new, changed, cfg, &stop);
    replay_check(b);
    if cfg.audit {
        dump_audit(b, i + 1);
    }
    done
}

/// What a watch session does while it waits for the next edit: fine
/// regions no recent edit re-ran merged back to the grain
/// (`PARTEX_MACHINE_COARSEN`: after how many rebuilds, 0: never), and what
/// was replaced dropped.
fn between_edits(b: &mut Build<Machine>, cfg: &partex_incr::build::Config, coarsen: u32) {
    if coarsen > 0 {
        let t = Instant::now();
        let merged = b.coarsen(cfg.grain, coarsen);
        if merged > 0 && cfg.sanitize {
            let d = partex_incr::Machine::digest(b.final_state());
            b.check_replay(&d);
        }
        if merged > 0 {
            eprintln!(
                "phitex: machine: coarsened {merged} regions in {:.3} s, {} now",
                t.elapsed().as_secs_f64(),
                b.stats.regions
            );
        }
    }
    drop(b.take_garbage());
    if std::env::var("PARTEX_MACHINE_RSS").is_ok_and(|v| v == "1") {
        eprintln!("phitex: machine: {}", rss());
    }
}

/// After a cold build: the candidate boundaries its steps returned and
/// the regions cut at them, by level (3 a clean point, 2 a file edge, 1
/// any other command read from a file). The cuts' levels by exit, kept
/// for [`dump_regions`] after an edit.
fn print_census(b: &Build<Machine>) -> BTreeMap<partex_core::machine::Pos, u8> {
    let _ = cut_timing("cold build"); // (with the census: both are the cold build's)
    let c = b.final_state().census();
    eprintln!(
        "phitex: machine: candidates by level 0-3 {:?} (clean: {} outer, {} paragraph starts), cut at {:?}",
        c.seen, c.clean[0], c.clean[1], c.cut
    );
    c.levels.clone()
}

/// `PARTEX_MACHINE_PARTS=9`: after a cold build, each region that
/// observed the numbering, and how (for debugging).
fn dump_numbering_observers(b: &Build<Machine>) {
    let obs = &b.final_state().census().observers;
    for (i, t) in b.traces().enumerate() {
        if let Some((f, o, w, n)) = obs.get(&t.exit) {
            eprintln!(
                "phitex: region {i}: cost {} observes the numbering: final_num {f}, of_final {o}, whole {w}, {n} distinct",
                t.cost
            );
        }
    }
}

/// The candidate level a region was cut at: as the last rebuild cut it,
/// or else as the cold build did (`cold`); `?` at the job's end.
fn cut_level(
    b: &Build<Machine>,
    cold: &BTreeMap<partex_core::machine::Pos, u8>,
    t: &partex_incr::Trace<Machine>,
) -> String {
    b.final_state()
        .census()
        .levels
        .get(&t.exit)
        .or_else(|| cold.get(&t.exit))
        .map_or_else(|| "?".to_owned(), u8::to_string)
}

/// `PARTEX_MACHINE_PARTS=5`, `6`, `7`: what a rebuild did, by region
/// (for debugging).
fn dump_after_rebuild(
    b: &Build<Machine>,
    cold: &BTreeMap<partex_core::machine::Pos, u8>,
    edit: &str,
) {
    match std::env::var("PARTEX_MACHINE_PARTS").as_deref() {
        Ok("5") => dump_regions(b, cold, edit),
        Ok("6") => dump_written(b),
        Ok("7") => dump_why_dirty(b),
        _ => {}
    }
}

/// `PARTEX_MACHINE_PARTS=10`: after a rebuild, at each old boundary the
/// re-execution passed, where the new run's `Rest` differs from the old
/// run's there (the old regions it replaced, from the garbage): the
/// parts that differ, as a machine hashes `Rest`, with details (for
/// debugging; it takes the garbage).
fn dump_rest_differences(b: &mut Build<Machine>) {
    let old = b.take_garbage().traces;
    let rest = |t: &partex_incr::Trace<Machine>| {
        t.writes
            .iter()
            .find(|(c, _, _)| *c == MCell::Rest)
            .and_then(|(_, v, h)| v.rest_tex().map(|x| (x, *h)))
    };
    let host = b.initial().tex().host().clone();
    let served = |n: &[u8]| {
        !n.is_empty()
            && host.file(n).is_some()
            && !partex_core::machine::CellHost::reads_back(&host, n)
    };
    let new: Vec<&partex_incr::Trace<Machine>> = b.traces().collect();
    for (i, t) in new.iter().enumerate() {
        let Some(o) = old.iter().find(|o| o.exit == t.exit) else {
            continue;
        };
        let (Some((x, hx)), Some((y, hy))) = (rest(o), rest(t)) else {
            continue;
        };
        if hx == hy {
            eprintln!("phitex: rest at the exit of region {i}: equal");
            continue;
        }
        let (px, py) = (
            x.rest_hash_parts_served(&served),
            y.rest_hash_parts_served(&served),
        );
        let parts: Vec<&str> = px
            .iter()
            .zip(&py)
            .filter(|(p, q)| p.1 != q.1)
            .map(|(p, _)| p.0)
            .collect();
        eprintln!(
            "phitex: rest at the exit of region {i} (line {}): [{}]; lines: {}; scalars: {}; save stack: {}; pdf: {}",
            t.exit.line,
            parts.join(", "),
            x.line_copies_difference(&y),
            x.scalars_difference(&y),
            if parts.contains(&"tables") {
                x.save_stack_difference(&y)
            } else {
                String::new()
            },
            if parts.contains(&"pdf") {
                x.pdf_objs_difference(&y)
            } else {
                String::new()
            },
        );
    }
}

/// `PARTEX_MACHINE_PARTS=7`: each region the last rebuild re-executed
/// from, and the cells of D it read (for debugging).
fn dump_why_dirty(b: &Build<Machine>) {
    for (i, why) in &b.why_dirty {
        let names: Vec<String> = why
            .iter()
            .take(8)
            .map(|c| match c {
                MCell::Eqtb(p) => b.final_state().tex().eqtb_loc_name(*p),
                c => format!("{c:?}").chars().take(60).collect(),
            })
            .collect();
        eprintln!(
            "phitex: dirty region {i}: {} cells of D read: {names:?}",
            why.len()
        );
    }
}

/// A cell's kind, for the audit's ranking: the variant, with the
/// `\pdflast…` value's number (few, and each a refinement of its own).
fn cell_kind(c: &MCell) -> String {
    match c {
        MCell::PdfLast(k) => format!("PdfLast({k})"),
        MCell::PdfWord(k) => format!("PdfWord({k})"),
        c => {
            let s = format!("{c:?}");
            s.split('(').next().unwrap_or_default().to_owned()
        }
    }
}

/// The fields in which two `Rest` values differ
/// ([`partex_core::Tex::rest_field_differences`]; empty if either holds
/// no engine).
fn rest_fields(
    old: Option<&<Machine as partex_incr::Machine>::Value>,
    new: Option<&<Machine as partex_incr::Machine>::Value>,
) -> BTreeSet<String> {
    use partex_core::machine::MValue;
    match (
        old.and_then(MValue::rest_tex),
        new.and_then(MValue::rest_tex),
    ) {
        (Some(o), Some(m)) => {
            // (the files a machine serves by lines, as it hashes `Rest`)
            let host = o.host().clone();
            let served = |n: &[u8]| {
                !n.is_empty()
                    && host.file(n).is_some()
                    && !partex_core::machine::CellHost::reads_back(&host, n)
            };
            o.rest_field_differences(&m, &served).into_iter().collect()
        }
        _ => BTreeSet::new(),
    }
}

/// With `PARTEX_MACHINE_AUDIT=1`, after rebuild `n`: each old region it
/// re-executed, why (the cells of D its span's first region read, and the
/// parts of `Rest` that differed at its entry, by field where
/// [`partex_core::Tex::rest_field_differences`] tells), and how the
/// re-execution ended:
/// - *spurious*: as the old run did ([`partex_incr::Audit::spurious`]);
/// - *carried*: the same effects, and what differs at its end differed at
///   its entry already and neither run changed it in the span: a field
///   of `Rest` that differs at both ends and that neither run's span
///   changed, or an accumulating cell to which both added the same. The
///   span only passed on a difference it found; were that difference
///   a cell of its own, which the span does not write, it would not have
///   re-run for it (if it did not read it);
/// - *real*: anything else.
///
/// Then, for each cause, the regions and commands it made re-run in each
/// class (a span with two causes counts for both; `sole`: its only
/// cause). The rebuild compares a span as a whole, so its later regions
/// take its first region's cause.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)] // (one report)
fn dump_audit(b: &Build<Machine>, n: usize) {
    #[derive(Default)]
    struct Tally {
        spurious: (usize, u64),
        carried: (usize, u64),
        sole: (usize, u64),
        real: (usize, u64),
    }
    // (the regions run again that observed the numbering, and how)
    let obs = &b.final_state().census().observers;
    for (i, t) in b.traces().enumerate() {
        if t.born == b.generation()
            && let Some((f, o, w, k)) = obs.get(&t.exit)
        {
            eprintln!(
                "phitex: audit: rebuild {n} new region {i} (cost {}) observed the numbering: final_num {f}, of_final {o}, whole {w}, {k} distinct",
                t.cost
            );
        }
    }
    for (i, why, c) in &b.unrelocated {
        let what = match c {
            MCell::Eqtb(p) => format!("eqtb {}", b.final_state().tex().eqtb_loc_name(*p)),
            MCell::Origin(o) if *o >= 0 => format!(
                "Origin({o}) {}",
                b.final_state()
                    .tex()
                    .eqtb_loc_name(partex_core::reloc::loc_of_origin(*o))
            ),
            c => format!("{c:?}"),
        };
        eprintln!("phitex: audit: rebuild {n} region {i} not relocated: {why} {what}");
    }
    let mut tally: BTreeMap<String, Tally> = BTreeMap::new();
    let mut sums = [(0usize, 0u64); 3]; // (spurious, carried, all)
    for a in &b.audit {
        let mut causes: Vec<String> = Vec::new();
        let mut shown: Vec<String> = Vec::new();
        // (where the save stacks differ, if they do at the entry)
        let mut tables = String::new();
        if !a.synced {
            causes.push("unsynced".to_owned());
        }
        // (`Rest` at the entry, in the old run and in S)
        let mut rest_entry = None;
        for (c, values) in &a.why {
            match (c, values) {
                (MCell::Rest, Some((old, new))) => {
                    let fields = rest_fields(old.as_ref(), new.as_ref());
                    if fields.is_empty() {
                        // (the version differs, no part does)
                        causes.push("Rest(no part)".to_owned());
                    }
                    if fields.contains("rest.tables")
                        && let (Some(o), Some(m)) = (
                            old.as_ref()
                                .and_then(partex_core::machine::MValue::rest_tex),
                            new.as_ref()
                                .and_then(partex_core::machine::MValue::rest_tex),
                        )
                    {
                        tables = format!(" save stack {{{}}}", o.save_stack_difference(&m));
                    }
                    causes.extend(fields.iter().cloned());
                    rest_entry = Some((old, new, fields));
                }
                (MCell::Eqtb(p), _) => {
                    causes.push("Eqtb".to_owned());
                    shown.push(b.final_state().tex().eqtb_loc_name(*p));
                }
                (c, _) => causes.push(cell_kind(c)),
            }
        }
        causes.sort();
        causes.dedup();
        // (what differs at the end, and whether each was only carried)
        let mut carried_fields: BTreeSet<String> = BTreeSet::new();
        let mut ends: Vec<String> = Vec::new();
        let mut all_carried = a.same_effects;
        for x in &a.differ {
            let carried = match (&x.cell, &x.values, &rest_entry) {
                (MCell::Rest, Some((old_exit, new_exit)), Some((old_in, new_in, entry))) => {
                    let exit = rest_fields(old_exit.as_ref(), new_exit.as_ref());
                    // (the fields each run changed in the span)
                    let old_run = rest_fields(old_in.as_ref(), old_exit.as_ref());
                    let new_run = rest_fields(new_in.as_ref(), new_exit.as_ref());
                    let ok = x.before
                        && !exit.is_empty()
                        && exit.iter().all(|f| {
                            entry.contains(f) && !old_run.contains(f) && !new_run.contains(f)
                        });
                    ends.push(format!(
                        "Rest{{{}}}",
                        exit.iter().cloned().collect::<Vec<_>>().join(" ")
                    ));
                    if ok {
                        carried_fields.extend(exit);
                    }
                    ok
                }
                (c, None, _) => {
                    ends.push(cell_kind(c));
                    x.before && x.same_delta
                }
                (c, _, _) => {
                    ends.push(cell_kind(c));
                    false
                }
            };
            all_carried &= carried;
        }
        let class = if a.spurious() {
            0
        } else if all_carried {
            1
        } else {
            2
        };
        let regions = a.old_costs.len();
        for (i, sum) in sums.iter_mut().enumerate() {
            if i == class || i == 2 {
                sum.0 += regions;
                sum.1 += a.cost;
            }
        }
        ends.truncate(8);
        eprintln!(
            "phitex: audit: rebuild {n} region {} ({} commands, old {}): {} causes [{}]{}{} differ at the end [{}{}]{}{}",
            a.first,
            a.cost,
            a.old_costs[0],
            ["spurious", "carried", "real"][class],
            causes.join(", "),
            if shown.is_empty() {
                String::new()
            } else {
                format!(" eqtb {:?}", &shown[..shown.len().min(6)])
            },
            if carried_fields.is_empty() {
                String::new()
            } else {
                format!(
                    " carried [{}]",
                    carried_fields
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            ends.join(", "),
            if a.differ.len() > 8 {
                format!(", {} more", a.differ.len() - 8)
            } else {
                String::new()
            },
            if a.same_effects {
                ""
            } else {
                " effects differ"
            },
            tables,
        );
        for (m, c) in a.old_costs.iter().enumerate().skip(1) {
            eprintln!(
                "phitex: audit: rebuild {n} region {} (old {c}): in the span of region {}, its cause",
                a.first + m,
                a.first
            );
        }
        for c in &causes {
            let t = tally.entry(c.clone()).or_default();
            let slot = match class {
                0 => &mut t.spurious,
                1 => &mut t.carried,
                _ => &mut t.real,
            };
            slot.0 += regions;
            slot.1 += a.cost;
            if class < 2 && causes.len() == 1 {
                t.sole.0 += regions;
                t.sole.1 += a.cost;
            }
        }
    }
    eprintln!(
        "phitex: audit: rebuild {n}: {} spans, {} old regions, {} commands re-run; spurious {} regions {} commands; carried {} regions {} commands; relocated {} regions; ms: run {:.0}, replay {:.0} ({} restores), compare {:.0}, other {:.0}",
        b.audit.len(),
        sums[2].0,
        sums[2].1,
        sums[0].0,
        sums[0].1,
        sums[1].0,
        sums[1].1,
        b.stats.relocated_regions,
        b.stats.run_ns as f64 / 1e6,
        b.stats.replay_ns as f64 / 1e6,
        b.stats.restores,
        b.stats.compare_ns as f64 / 1e6,
        b.stats.other_ns as f64 / 1e6,
    );
    let mut ranked: Vec<(&String, &Tally)> = tally.iter().collect();
    let key = |t: &Tally| (t.spurious.1 + t.carried.1, t.real.1);
    ranked.sort_by(|x, y| (key(y.1), x.0).cmp(&(key(x.1), y.0)));
    for (c, t) in ranked {
        eprintln!(
            "phitex: audit: rebuild {n} cause {c}: spurious {} regions {} commands; carried {} regions {} commands (sole cause {} regions {} commands); real {} regions {} commands",
            t.spurious.0,
            t.spurious.1,
            t.carried.0,
            t.carried.1,
            t.sole.0,
            t.sole.1,
            t.real.0,
            t.real.1
        );
    }
}

/// `PARTEX_MACHINE_PARTS=6`: the regions that read `\write` files back,
/// and the last ones that append to them (for debugging).
fn dump_written(b: &Build<Machine>) {
    // (the regions that read or append to `\write` files)
    for (i, t) in b.traces().enumerate() {
        let g: Vec<_> = t
            .guards
            .iter()
            .filter(|(c, _)| matches!(c, MCell::Written(_)))
            .collect();
        let w: Vec<_> = t
            .writes
            .iter()
            .filter(|(c, _, _)| matches!(c, MCell::Written(_)))
            .map(|(c, _, v)| (c, v))
            .collect();
        if !g.is_empty() || (!w.is_empty() && i + 12 > b.stats.regions) {
            eprintln!(
                "phitex: region {i} born {}: reads {g:?} appends {w:?}",
                t.born
            );
        }
    }
}

/// `PARTEX_MACHINE_PARTS=5`: the regions around the lines of the file an
/// edit changed, with their costs (for debugging).
fn dump_regions(b: &Build<Machine>, cold: &BTreeMap<partex_core::machine::Pos, u8>, edit: &str) {
    let path = edit.split('|').next().unwrap_or("");
    let traces: Vec<&partex_incr::Trace<Machine>> = b.traces().collect();
    let lines = |t: &partex_incr::Trace<Machine>| -> Vec<u32> {
        t.guards
            .iter()
            .filter_map(|(c, _)| match c {
                MCell::Line(f, k) if f.ends_with(path.as_bytes()) => Some(*k),
                _ => None,
            })
            .collect()
    };
    let Some(first) = traces.iter().position(|t| !lines(t).is_empty()) else {
        return;
    };
    for (i, t) in traces
        .iter()
        .enumerate()
        .skip(first.saturating_sub(3))
        .take(40)
    {
        let l = lines(t);
        eprintln!(
            "phitex: region {i}: cost {} guards {} writes {} lines {:?}..{:?} effects {} cut at level {}",
            t.cost,
            t.guards.len(),
            t.writes.len(),
            l.first(),
            l.last(),
            t.effects.len(),
            cut_level(b, cold, t)
        );
    }
}

/// The lines of the main file `path` where a cold split may begin: after
/// `\\begin{document}`, lines that begin with a sectioning or input
/// command.
fn split_lines(path: &str) -> Vec<i32> {
    let Ok(text) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut body = false;
    let mut out = Vec::new();
    for (i, l) in text.split(|&b| b == b'\n').enumerate() {
        let l = l.trim_ascii_start();
        if l.starts_with(b"\\begin{document}") {
            body = true;
            continue;
        }
        let heads: [&[u8]; 6] = [
            b"\\chapterfile",
            b"\\include{",
            b"\\input{",
            b"\\chapter",
            b"\\part",
            b"\\section",
        ];
        if body && heads.iter().any(|h| l.starts_with(h)) {
            out.push(i32::try_from(i + 1).unwrap_or(i32::MAX));
        }
    }
    out
}

/// What differs between a guessed and a true state (for [`split_probe`]).
#[allow(clippy::many_single_char_names)] // (g a guess, t the truth, …)
fn state_difference(guess: &Machine, truth: &Machine) -> String {
    use partex_incr::{Machine as _, version_of};
    let (g, t) = (guess, truth);
    let mut parts: Vec<&str> = Vec::new();
    let (x, y) = (g.tex().rest_hash_parts(), t.tex().rest_hash_parts());
    parts.extend(
        x.iter()
            .zip(&y)
            .filter(|(p, q)| p.1 != q.1)
            .map(|(p, _)| p.0),
    );
    let (x, y) = (g.tex().pdf_hash_parts(), t.tex().pdf_hash_parts());
    parts.extend(
        x.iter()
            .zip(&y)
            .filter(|(p, q)| p.1 != q.1)
            .map(|(p, _)| p.0),
    );
    let eq = g.tex().eqtb_differences(t.tex());
    let names: Vec<String> = eq
        .iter()
        .take(12)
        .filter_map(|c| match c {
            partex_core::track::Cell::Eqtb(p) => Some(t.tex().eqtb_loc_name(*p)),
            _ => None,
        })
        .collect();
    let rest = version_of(&g.get(&MCell::Rest)) == version_of(&t.get(&MCell::Rest));
    let glyphs = version_of(&g.get(&MCell::Glyphs)) != version_of(&t.get(&MCell::Glyphs));
    format!(
        "at {} rest-equal {rest} parts [{}] eqtb {} {names:?} glyphs {glyphs}",
        g.at() == t.at(),
        parts.join(", "),
        eq.len(),
    )
}

/// `PARTEX_MACHINE_SPLITPROBE=1`: run the job sequentially; at each top-
/// level split line of the main file (`split_lines`), compare the true
/// state with two guesses at the same position: the state at the first
/// split line (a cold split's guess), and the state at the split before.
/// What differs is what a parallel round's guards would fail on.
fn split_probe(m: &Machine) {
    use partex_incr::{Machine as _, Step};
    let main = std::env::args()
        .rev()
        .find(|a| {
            std::path::Path::new(a)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("tex"))
        })
        .unwrap_or_default();
    let lines = split_lines(&main);
    eprintln!(
        "phitex: split: {} split lines in {main}: {lines:?}",
        lines.len()
    );
    let mut m = m.clone();
    let mut r = Null;
    let mut first: Option<Machine> = None;
    let mut prev: Option<Machine> = None;
    let mut next = 0;
    let mut steps: u64 = 0;
    let mut last_step = 0;
    let t0 = Instant::now();
    loop {
        let st = m.step(&mut r);
        steps += 1;
        match st {
            Step::Halt => break,
            Step::Candidate(_) => {}
            _ => continue,
        }
        let Some(l) = m.top_line() else { continue };
        while next < lines.len() && lines[next] < l {
            next += 1;
        }
        if next >= lines.len() || lines[next] != l {
            continue;
        }
        next += 1;
        eprintln!(
            "phitex: split: line {l} at step {steps} (+{}) {:.1} s",
            steps - last_step,
            t0.elapsed().as_secs_f64()
        );
        last_step = steps;
        for (what, base) in [("first", &first), ("prev", &prev)] {
            if let Some(b) = base {
                let mut g = b.clone();
                g.adopt_position(&m);
                eprintln!("phitex: split:   vs {what}: {}", state_difference(&g, &m));
            }
        }
        if first.is_none() {
            first = Some(m.clone());
        }
        prev = Some(m.clone());
    }
    eprintln!(
        "phitex: split: {steps} steps in {:.1} s",
        t0.elapsed().as_secs_f64()
    );
}

/// A recorder that keeps nothing (for [`convergence`]).
struct Null;

impl partex_incr::Recorder<Machine> for Null {
    fn read(&mut self, _: &MCell, _: Option<&<Machine as partex_incr::Machine>::Value>) {}
    fn write(&mut self, _: &MCell, _: &<Machine as partex_incr::Machine>::Value) {}
    fn effect(&mut self, _: <Machine as partex_incr::Machine>::Effect) {}
    fn force(
        &mut self,
        _: partex_incr::Hole,
    ) -> partex_incr::Forced<<Machine as partex_incr::Machine>::Value> {
        partex_incr::Forced::Suspend
    }
    fn wants_values(&self) -> bool {
        false
    }
}

/// `PARTEX_MACHINE_PARTS=4`: after an edit, where the edited run's state
/// differs from the old run's at each of the old run's region entries
/// (matched by position), until they agree again (for measuring what
/// The details of the parts of `Rest` that differ (for debugging).
fn parts_detail(old: &Machine, m: &Machine, parts: &[&str], eq: &[partex_core::track::Cell]) {
    eprintln!(
        "phitex: converge: xf {}",
        old.tex().xregs_fonts_difference(m.tex())
    );
    eprintln!(
        "phitex: converge: fonts {}",
        old.tex().font_slot_differences(m.tex())
    );
    if !eq.is_empty() {
        eprintln!(
            "phitex: converge: eqtb {}",
            old.tex().eqtb_diff_detail(m.tex(), eq)
        );
    }
    if parts.contains(&"out") {
        eprintln!(
            "phitex: converge: out {}",
            old.tex().pdf_out_difference(m.tex())
        );
    }
    if parts.contains(&"tables") || parts.contains(&"objs") {
        eprintln!(
            "phitex: converge: numbering {}",
            old.tex().numbering_difference(m.tex())
        );
    }
    if parts.iter().any(|p| matches!(*p, "ship" | "objs" | "out")) {
        eprintln!(
            "phitex: converge: pdf {}",
            old.tex().pdf_objs_difference(m.tex())
        );
    }
    if parts.iter().any(|p| p.starts_with("scalars")) {
        eprintln!(
            "phitex: converge: scalars {}",
            old.tex().scalars_difference(m.tex())
        );
    }
    if parts.contains(&"tables") {
        eprintln!(
            "phitex: converge: save stack {}",
            old.tex().save_stack_difference(m.tex())
        );
    }
}

/// stops early cutoff).
#[allow(clippy::many_single_char_names)] // (b a build, m a machine, …)
fn convergence(b: &Build<Machine>, new: &Machine) {
    use partex_incr::{Machine as _, Step, version_of};
    let traces: Vec<&partex_incr::Trace<Machine>> = b.traces().collect();
    let mut at: BTreeMap<<Machine as partex_incr::Machine>::Boundary, Vec<usize>> = BTreeMap::new();
    for (i, t) in traces.iter().enumerate() {
        at.entry(t.entry).or_default().push(i);
    }
    let mut old = b.initial().clone();
    let mut next = 0; // the old region `old` is at the entry of
    let mut m = new.clone();
    let mut r = Null;
    let mut steps: u64 = 0;
    let mut first: Option<u64> = None;
    let mut same_run = 0;
    loop {
        let step = m.step(&mut r);
        steps += 1;
        match step {
            Step::Halt => break,
            Step::Candidate(_) => {}
            _ => continue,
        }
        let Some(&j) = at
            .get(&m.at())
            .and_then(|v| v.iter().find(|&&j| j >= next.max(1)))
        else {
            continue;
        };
        while next < j {
            traces[next].apply(&mut old, &[]);
            next += 1;
        }
        m.prepare_cut(&mut r);
        let (vo, vn) = (
            version_of(&old.get(&MCell::Rest)),
            version_of(&m.get(&MCell::Rest)),
        );
        let eq = old.tex().eqtb_differences(m.tex());
        if let Ok(cs) = std::env::var("PARTEX_PROBE_CS") {
            eprintln!(
                "phitex: converge: \\{cs} at region {j}: old {} / new {}",
                old.tex().cs_debug(cs.as_bytes()),
                m.tex().cs_debug(cs.as_bytes())
            );
        }
        let glyphs = version_of(&old.get(&MCell::Glyphs)) != version_of(&m.get(&MCell::Glyphs));
        if vo == vn && eq.is_empty() && !glyphs {
            if first.is_some() {
                same_run += 1;
                eprintln!("phitex: converge: region {j} at step {steps}: same");
                if same_run >= 3 && std::env::var("PARTEX_MACHINE_PROBE_ALL").is_err() {
                    break;
                }
            }
            continue;
        }
        same_run = 0;
        let since = *first.get_or_insert(steps);
        let mut parts: Vec<&str> = Vec::new();
        if vo != vn {
            let (x, y) = (old.tex().rest_hash_parts(), m.tex().rest_hash_parts());
            parts.extend(
                x.iter()
                    .zip(&y)
                    .filter(|(p, q)| p.1 != q.1)
                    .map(|(p, _)| p.0),
            );
            let (x, y) = (old.tex().pdf_hash_parts(), m.tex().pdf_hash_parts());
            parts.extend(
                x.iter()
                    .zip(&y)
                    .filter(|(p, q)| p.1 != q.1)
                    .map(|(p, _)| p.0),
            );
        }
        let names: Vec<String> = eq
            .iter()
            .take(6)
            .filter_map(|c| match c {
                partex_core::track::Cell::Eqtb(p) => Some(old.tex().eqtb_loc_name(*p)),
                _ => None,
            })
            .collect();
        if parts.contains(&"page") {
            eprintln!(
                "phitex: converge: page skeleton equal: {}",
                old.tex().page_skeleton_hash() == m.tex().page_skeleton_hash()
            );
        }
        parts_detail(&old, &m, &parts, &eq);
        eprintln!(
            "phitex: converge: region {j} at step {steps} (+{}): rest [{}] eqtb {} {:?} glyphs {glyphs}",
            steps - since,
            parts.join(", "),
            eq.len(),
            names
        );
    }
    eprintln!("phitex: converge: {steps} steps");
}

/// What a watch build or rebuild did: its terminal text, history,
/// diagnostics, output files (names and lengths), and a report line per
/// pass.
#[derive(Clone)]
pub struct Outcome {
    pub term: Vec<u8>,
    pub history: i32,
    pub diagnostics: Vec<partex_core::diag::Diagnostic>,
    pub outputs: Vec<(Vec<u8>, usize)>,
    pub reports: Vec<String>,
    /// The job's own files it read that its last pass still changed, after
    /// [`PASSES`] passes (empty: it reached its fixpoint).
    pub unsettled: Vec<String>,
}

/// A file that differs on disk from what the build read
/// ([`Watch::changes`]).
struct Found {
    key: Vec<u8>,
    now: Arc<[u8]>,
    /// Whether it takes a stamp once applied (a file read; not a file
    /// not found before, nor the clock), and the modification time it
    /// was read at.
    stamped: bool,
    time: Option<std::time::SystemTime>,
    /// As the watch last wrote it: the job's own file (a pass's `.aux`),
    /// not an edit.
    own: bool,
}

/// A watch session in machine mode (the default for `phitex watch`,
/// DESIGN.md §6.1): a recorded build, rebuilt after the files it read
/// change, each rebuild run to its fixpoint (the job's own files it read
/// back, BibTeX and makeindex between passes) as a further rebuild, and
/// the output files written when their bytes change.
pub struct Watch {
    b: Build<Machine>,
    cfg: partex_incr::build::Config,
    coarsen: u32,
    /// Each input's modification time when its contents were last found
    /// to be what the build read, or applied (a file the job read and the
    /// watch wrote since keeps its stamp until a rebuild applies it).
    stamps: BTreeMap<Vec<u8>, Option<std::time::SystemTime>>,
    /// The bytes last written to each output file (by [`quick::hash`]).
    written: BTreeMap<Vec<u8>, u128>,
    /// The files this watch wrote, by their real paths: the hash of the
    /// bytes and the modification time they got. A file the job reads
    /// that is as the watch last wrote it is the job's own (a pass's
    /// `.aux`), never an edit: it is applied with the next edit, and a
    /// job that does not settle in [`PASSES`] does not rebuild again for
    /// it ([`Watch::changed`]).
    own: BTreeMap<std::path::PathBuf, (u128, Option<std::time::SystemTime>)>,
    /// The real paths of the files the build read, by key, once found.
    real: BTreeMap<Vec<u8>, std::path::PathBuf>,
    /// The no-op restart's record being written ([`quick::record`]).
    quick: Option<std::thread::JoinHandle<()>>,
    /// The linked outputs of the last pass: name to bytes.
    files: Vec<(Vec<u8>, Vec<u8>)>,
    /// The last link's output, taken again while no region's effects
    /// change ([`Build::take_effects_changed`]).
    linked: Option<partex_core::effects::Linked>,
    /// Each region's effects with pdfTeX's numbers, as the last link
    /// resolved them (`PARTEX_MACHINE_LINK_CACHE=0`: none kept).
    link_cache: partex_core::effects::LinkCache,
    tick: u32,
    /// Rebuilds audited (`PARTEX_MACHINE_AUDIT=1`), to number them.
    audits: usize,
    /// A rebuild ran since the last [`Watch::idle`].
    idle: bool,
    /// The store the build is kept in across processes.
    keeper: Option<persisted::Keeper>,
    /// What the last rebuild's edits changed: each file, and the first
    /// line it changed at (`paper.tex:18`).
    last_changes: Vec<String>,
    /// The `SyncTeX` file as the last link's regions made it
    /// ([`synctex_file`]).
    synctex: Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)>,
}

/// Passes of a watch rebuild at most (as `-converge`).
pub const PASSES: usize = 5;

/// The real path of the file named `name` (a key of a file read, or a
/// name written or found), if there is one, kept in `real` once found.
fn real_path(
    real: &mut BTreeMap<Vec<u8>, std::path::PathBuf>,
    name: &[u8],
) -> Option<std::path::PathBuf> {
    if let Some(r) = real.get(name) {
        return Some(r.clone());
    }
    let r = std::fs::canonicalize(crate::native::path(name)).ok()?;
    real.insert(name.to_vec(), r.clone());
    Some(r)
}

impl Watch {
    /// Build the job, to its fixpoint, and write its outputs.
    pub fn new(
        native: NativeHost,
        params: Params,
        command_line: &[u8],
        between: &mut crate::Between,
        observe: &mut dyn FnMut(crate::events::Progress),
    ) -> (Self, Outcome) {
        let cfg = config();
        let host = MachineHost::new(native);
        let mut tex = Tex::new(host, CellTracker::default(), params);
        crate::origins::setup_synctex(&mut tex);
        let mut m = TexMachine::new(tex, command_line);
        switches(&mut m);
        observe(crate::events::Progress::PassStart(1));
        observe(crate::events::Progress::Phase(crate::events::Phase::Cold));
        let t = Instant::now();
        let b = Build::new(m, &cfg);
        let elapsed = t.elapsed();
        // (a report for debugging, not for the modern command line's
        // terminal unless asked for)
        if std::env::var_os("PARTEX_WATCH_DEBUG").is_some()
            || partex_core::machine::CUT_TIMING.load(std::sync::atomic::Ordering::Relaxed)
        {
            print_census(&b);
        }
        let mut w = Self::from_build(b);
        w.cfg = cfg;
        let first = (
            format!(
                "phitex: machine: built in {:.1} ms: {} regions",
                elapsed.as_secs_f64() * 1e3,
                w.b.stats.regions
            ),
            w.report(elapsed),
        );
        let out = w.converge(first, between, observe);
        // (the indexes, which a rebuild needs, while nothing is asked)
        w.b.index();
        (w, out)
    }

    /// A watch over build `b` (recorded or loaded), nothing written yet.
    fn from_build(b: Build<Machine>) -> Self {
        let coarsen = std::env::var("PARTEX_MACHINE_COARSEN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        Self {
            b,
            cfg: config(),
            coarsen,
            stamps: BTreeMap::new(),
            written: BTreeMap::new(),
            own: BTreeMap::new(),
            real: BTreeMap::new(),
            quick: None,
            files: Vec::new(),
            linked: None,
            link_cache: partex_core::effects::LinkCache::default(),
            tick: 0,
            audits: 0,
            idle: false,
            keeper: None,
            last_changes: Vec::new(),
            synctex: None,
        }
    }

    /// What the last rebuild's edits changed: each file, and the first
    /// line it changed at (`paper.tex:18`).
    pub fn last_changes(&self) -> &[String] {
        &self.last_changes
    }

    /// `changes` as the terminal names them: each file and the first line
    /// that differs from what the build read (a time is no file's).
    fn describe(&self, changes: &[(Vec<u8>, Arc<[u8]>)]) -> Vec<String> {
        let init = self.b.initial().tex().host();
        changes
            .iter()
            .filter(|(k, _)| k.as_slice() != CLOCK)
            .map(|(k, now)| {
                // (a file that appeared: its name)
                let name = k.rsplit(|&c| c == 0).next().unwrap_or(k);
                let name = String::from_utf8_lossy(undotted(name)).into_owned();
                match init.file(k) {
                    Some(old) => {
                        let same = old
                            .iter()
                            .zip(now.iter())
                            .take_while(|(a, b)| a == b)
                            .count();
                        let line = old[..same].split(|&c| c == b'\n').count();
                        format!("{name}:{line}")
                    }
                    None => name,
                }
            })
            .collect()
    }

    /// Whether an input was edited on disk (by modification time; the
    /// files under absolute paths, the TeX tree's, and the files not
    /// found every tenth call). The job's own files as the watch wrote
    /// them are not edits.
    pub fn changed(&mut self) -> bool {
        self.tick = self.tick.wrapping_add(1);
        let all = self.tick.is_multiple_of(10);
        let init = self.b.initial().tex().host().clone();
        for key in init.served_paths() {
            // (the clock and the font index are no files)
            if key.starts_with(b"\0") || (!all && key.starts_with(b"/")) {
                continue;
            }
            let p = crate::native::path(&key);
            let time = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            match self.stamps.get(&key).copied() {
                None => {
                    self.stamps.insert(key, time);
                }
                Some(seen) if seen == time => {}
                // (as the watch wrote it, by its time)
                Some(_) if time.is_some() && self.own_at(&key, time) => {}
                Some(_) => return true,
            }
        }
        if all {
            for (_, (name, kind)) in Self::missed(&init) {
                if let Some(f) = init.native().read_file(&name, kind)
                    && !self.own_bytes(&f.name, &f.contents)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Rebuild after an input was edited, to the fixpoint (the job's own
    /// files as the watch wrote them, if the last rebuild did not settle,
    /// applied with the edits). A newer edit that arrives while it runs
    /// stops it at the next region boundary (DESIGN.md §7.11), and it goes
    /// on from there with that edit too: the outputs are written only from
    /// a rebuild that ran to its end (`PARTEX_WATCH_CANCEL=0`: each edit's
    /// rebuild runs to its end). `None` if nothing was edited.
    pub fn rebuild(
        &mut self,
        between: &mut crate::Between,
        observe: &mut dyn FnMut(crate::events::Progress),
    ) -> Option<Outcome> {
        let found = self.changes();
        // (none, or only the job's own files: they wait for an edit)
        if found.iter().all(|f| f.own) {
            return None;
        }
        let edits: Vec<(Vec<u8>, Arc<[u8]>)> = found
            .iter()
            .filter(|f| !f.own)
            .map(|f| (f.key.clone(), f.now.clone()))
            .collect();
        self.last_changes = self.describe(&edits);
        let changes = self.take(found);
        observe(crate::events::Progress::PassStart(1));
        let (first, _) = self.apply_to_the_end(changes, observe);
        Some(self.converge(first, between, observe))
    }

    /// [`Watch::apply`], stopped for each newer edit that arrives while it
    /// runs and gone on with it, until one runs to its end
    /// (`PARTEX_WATCH_CANCEL=0`: the first runs to its end). Whether a
    /// newer edit superseded it (told to `observe`; the pass is then the
    /// first of a new rebuild, [`Watch::last_changes`] its edits).
    fn apply_to_the_end(
        &mut self,
        mut changes: Vec<(Vec<u8>, Arc<[u8]>)>,
        observe: &mut dyn FnMut(crate::events::Progress),
    ) -> ((String, crate::session::Report), bool) {
        let stoppable = !std::env::var("PARTEX_WATCH_CANCEL").is_ok_and(|v| v == "0");
        let mut lines = Vec::new();
        let mut superseded = false;
        loop {
            let (line, report, done) = self.apply_or_stop(&changes, stoppable);
            if !done && std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!("{line}");
            }
            lines.push(line);
            if done {
                return ((lines.join("\n"), report), superseded);
            }
            // (what changed since: nothing, if only a time did; the rebuild
            // then goes on from where it stopped)
            let found = self.changes();
            self.supersede(&found, observe);
            superseded |= found.iter().any(|f| !f.own);
            changes = self.take(found);
        }
    }

    /// If `found` has an edit (not only the job's own files), the build
    /// under way is superseded: its edits become the rebuild's, and
    /// `observe` is told, then that pass 1 begins again.
    fn supersede(&mut self, found: &[Found], observe: &mut dyn FnMut(crate::events::Progress)) {
        let edits: Vec<(Vec<u8>, Arc<[u8]>)> = found
            .iter()
            .filter(|f| !f.own)
            .map(|f| (f.key.clone(), f.now.clone()))
            .collect();
        if edits.is_empty() {
            return;
        }
        self.last_changes = self.describe(&edits);
        if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
            eprintln!(
                "phitex: machine: superseded by {}",
                self.last_changes.join(", ")
            );
        }
        observe(crate::events::Progress::Superseded(&self.last_changes));
        observe(crate::events::Progress::PassStart(1));
    }

    /// The files not found that a rebuild's starting state does not serve
    /// yet, by key: the name and kind looked for.
    fn missed(init: &MachineHost) -> Missed {
        init.missed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(k, _)| !init.overrides.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The inputs that differ on disk from what the build read: the clock,
    /// the files read (those whose modification time is not their stamp,
    /// read whole; the TeX tree's too) and the files not found that now
    /// are, each marked if the watch wrote it so. The files found as the
    /// build read them are stamped; the others are stamped when applied
    /// ([`Watch::take`]).
    fn changes(&mut self) -> Vec<Found> {
        let init = self.b.initial().tex().host().clone();
        let mut out = Vec::new();
        if let Some(old) = init.file(CLOCK)
            && let Some(now) = Clock::since(&old, &mut init.native())
        {
            out.push(Found {
                key: CLOCK.to_vec(),
                now: Arc::from(now),
                stamped: false,
                time: None,
                own: false,
            });
        }
        for key in init.served_paths() {
            if key == CLOCK {
                continue;
            }
            // (an input that is no file, such as the font index: by what it
            // is now, as a store's build may have read another)
            if key.starts_with(b"\0") {
                if let Some(now) = crate::native::not_a_file(&key)
                    && init.file(&key).is_none_or(|old| *old != *now)
                {
                    out.push(Found {
                        key,
                        now,
                        stamped: false,
                        time: None,
                        own: false,
                    });
                }
                continue;
            }
            let p = crate::native::path(&key);
            let time = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            if self.stamps.get(&key) == Some(&time) {
                continue;
            }
            let now = match std::fs::read(&p) {
                Ok(now) => now,
                // (a file read that is gone: its readers run again, and find
                // it nowhere, as TeX would; one that comes back has a time
                // again, and its contents are read then)
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && init.file(&key).is_some_and(|old| !old.is_empty()) =>
                {
                    out.push(Found {
                        key,
                        now: Arc::from(Vec::new()),
                        stamped: true,
                        time,
                        own: false,
                    });
                    continue;
                }
                Err(_) => {
                    self.stamps.insert(key, time);
                    continue;
                }
            };
            if init.file(&key).is_some_and(|old| *old == *now) {
                self.stamps.insert(key, time);
                continue;
            }
            let own = self.own_bytes(&key, &now);
            out.push(Found {
                key,
                now: Arc::from(now),
                stamped: true,
                time,
                own,
            });
        }
        for (key, (name, kind)) in Self::missed(&init) {
            if let Some(f) = init.native().read_file(&name, kind) {
                let own = self.own_bytes(&f.name, &f.contents);
                out.push(Found {
                    key,
                    now: f.contents,
                    stamped: false,
                    time: None,
                    own,
                });
            }
        }
        out
    }

    /// `found`, to apply: each stamped with the time it was read at.
    fn take(&mut self, found: Vec<Found>) -> Vec<(Vec<u8>, Arc<[u8]>)> {
        found
            .into_iter()
            .map(|f| {
                if f.stamped {
                    self.stamps.insert(f.key.clone(), f.time);
                }
                (f.key, f.now)
            })
            .collect()
    }

    /// Whether the file named `name` has the modification time the watch
    /// gave it when it last wrote it.
    fn own_at(&mut self, name: &[u8], time: Option<std::time::SystemTime>) -> bool {
        real_path(&mut self.real, name)
            .and_then(|r| self.own.get(&r))
            .is_some_and(|(_, t)| *t == time)
    }

    /// Whether `bytes`, the contents of the file named `name`, are what the
    /// watch last wrote to it.
    fn own_bytes(&mut self, name: &[u8], bytes: &[u8]) -> bool {
        let Some(r) = real_path(&mut self.real, name) else {
            return false;
        };
        self.own
            .get(&r)
            .is_some_and(|(h, _)| *h == quick::hash(bytes))
    }

    /// What the last build or rebuild ran, as a session reports it
    /// (commands are the regions' costs).
    fn report(&self, elapsed: std::time::Duration) -> crate::session::Report {
        crate::session::Report {
            history: self.b.final_state().history().unwrap_or(3),
            commands: self.b.stats.executed_cost,
            total_commands: self.b.traces().map(|t| t.cost).sum(),
            elapsed,
            cut_at: None,
            why: Vec::new(),
        }
    }

    /// Rebuild with `changes` applied to the starting state: the report
    /// line and the report.
    fn apply(&mut self, changes: &[(Vec<u8>, Arc<[u8]>)]) -> (String, crate::session::Report) {
        let (line, report, _) = self.apply_or_stop(changes, false);
        (line, report)
    }

    /// [`Watch::apply`], stopping early (`stoppable`) if a file of the job
    /// it read changes on disk meanwhile (by modification time, looked at
    /// between re-executed spans, at most every `PARTEX_WATCH_POLL_MS`,
    /// 50 ms): whether it ran to its end.
    fn apply_or_stop(
        &mut self,
        changes: &[(Vec<u8>, Arc<[u8]>)],
        stoppable: bool,
    ) -> (String, crate::session::Report, bool) {
        let mut new = self.b.initial().clone();
        let mut cells = Vec::new();
        for (key, now) in changes {
            let old = new.tex().host().file(key);
            new.tex_mut().host_mut().set_file(key, Some(now.clone()));
            match old {
                Some(old) => cells.extend(changed_cells(&key.as_slice().into(), &old, now)),
                None => cells.push(MCell::File(key.as_slice().into())),
            }
        }
        let t = Instant::now();
        // (the job's own files as the changes left them, what a newer edit
        // changes; not the TeX tree's, under absolute paths, which the
        // watch looks at every tenth time: a look at hundreds of files
        // every few milliseconds costs a long rebuild seconds)
        let watched: Vec<(std::path::PathBuf, Option<std::time::SystemTime>)> = if stoppable {
            self.stamps
                .iter()
                .filter(|(k, _)| !k.starts_with(b"/"))
                .map(|(k, t)| (crate::native::path(k), *t))
                .collect()
        } else {
            Vec::new()
        };
        // (as often as the watch looks for edits)
        let every = std::time::Duration::from_millis(
            std::env::var("PARTEX_WATCH_POLL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50),
        );
        let looked = std::cell::Cell::new(Instant::now());
        // (the looks, and when one found a newer edit)
        let looks = std::cell::Cell::new(0u32);
        let found = std::cell::Cell::new(None);
        let stop = || {
            if !stoppable || looked.get().elapsed() < every {
                return false;
            }
            looked.set(Instant::now());
            looks.set(looks.get() + 1);
            let edited = watched
                .iter()
                .any(|(p, t)| std::fs::metadata(p).and_then(|m| m.modified()).ok() != *t);
            if edited {
                found.set(Some(t.elapsed()));
            }
            edited
        };
        let before = t.elapsed();
        let done = self.b.rebuild_or_stop(new, &cells, &self.cfg, &stop);
        let elapsed = t.elapsed();
        if let Some(at) = found.get()
            && std::env::var_os("PARTEX_WATCH_DEBUG").is_some()
        {
            eprintln!(
                "phitex: machine: a newer edit found at {:.1} ms (look {}), the rebuild stopped at {:.1} ms{}",
                at.as_secs_f64() * 1e3,
                looks.get(),
                elapsed.as_secs_f64() * 1e3,
                if self.b.stats.given_up_cost > 0 {
                    format!(
                        ", a span given up after {} commands",
                        self.b.stats.given_up_cost
                    )
                } else {
                    String::new()
                }
            );
            let st = &self.b.stats;
            let ms = |ns: u64| std::time::Duration::from_nanos(ns).as_secs_f64() * 1e3;
            eprintln!(
                "phitex: machine: (run {:.1} ms, replay {:.1} ms, start {:.1} ms, splice {:.1} ms; before the rebuild {:.1} ms)",
                ms(st.run_ns),
                ms(st.replay_ns),
                ms(st.other_parts_ns[0]),
                ms(st.other_parts_ns[1]),
                before.as_secs_f64() * 1e3
            );
        }
        if self.cfg.audit {
            self.audits += 1;
            dump_audit(&self.b, self.audits);
        }
        let report = self.report(elapsed);
        let line = format!(
            "phitex: machine: {} in {:.1} ms: {} of {} regions re-run ({} changed)",
            if done {
                "rebuilt"
            } else {
                "stopped for a newer edit"
            },
            elapsed.as_secs_f64() * 1e3,
            self.b.stats.recorded_regions,
            self.b.stats.regions,
            changes
                .iter()
                .map(|(k, _)| {
                    // (a file that appeared: its name)
                    let name = k.rsplit(|&c| c == 0).next().unwrap_or(k);
                    String::from_utf8_lossy(undotted(name)).into_owned()
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        self.idle = true;
        (line, report, done)
    }

    /// What to do while no edit is waiting (after a rebuild's outputs are
    /// written): merge the fine regions no recent rebuild re-ran, and drop
    /// what the rebuilds replaced. Whether there was anything to do.
    pub fn idle(&mut self) -> bool {
        if !std::mem::take(&mut self.idle) {
            return false;
        }
        if self.coarsen > 0 {
            self.b.coarsen(self.cfg.grain, self.coarsen);
        }
        drop(self.b.take_garbage());
        self.save();
        true
    }

    /// Write the outputs of the build as it is, then rebuild while the job's
    /// own files it read changed (after BibTeX and makeindex), up to
    /// [`PASSES`].
    fn converge(
        &mut self,
        first: (String, crate::session::Report),
        between: &mut crate::Between,
        observe: &mut dyn FnMut(crate::events::Progress),
    ) -> Outcome {
        let (line, mut report) = first;
        let mut reports = vec![line];
        let mut passes = 1;
        loop {
            observe(crate::events::Progress::Pass(passes, Some(&report)));
            observe(crate::events::Progress::Phase(
                crate::events::Phase::Linking,
            ));
            let (term, diagnostics) = match self.write() {
                Ok(x) => x,
                Err(e) => {
                    reports.push(format!("phitex: writing the outputs failed: {e}"));
                    return self.outcome(Vec::new(), Vec::new(), reports, Some(3));
                }
            };
            let ending = |ext: &[u8]| -> Vec<(Vec<u8>, Arc<[u8]>)> {
                self.files
                    .iter()
                    .filter(|(n, _)| n.ends_with(ext))
                    .map(|(n, c)| (n.clone(), Arc::from(&c[..])))
                    .collect()
            };
            let mut tools = crate::bibtex::after_pass(&mut between.bib, &ending(b".aux"));
            tools.extend(crate::makeindex::after_pass(
                &mut between.idx,
                &ending(b".idx"),
            ));
            for t in &tools {
                observe(crate::events::Progress::Tool(t));
            }
            reports.extend(tools);
            self.own_tools(between);
            if passes == PASSES {
                // (the job's own files it read that this pass changed are
                // left for the next edit; an edit made meanwhile is not
                // stamped, so the watch sees it)
                let mut unsettled: Vec<String> = self
                    .changes()
                    .into_iter()
                    .filter(|f| f.own)
                    .map(|f| {
                        let name = f.key.rsplit(|&c| c == 0).next().unwrap_or(&f.key);
                        String::from_utf8_lossy(undotted(name)).into_owned()
                    })
                    .collect();
                unsettled.sort();
                unsettled.dedup();
                if !unsettled.is_empty() {
                    reports.push(format!(
                        "phitex: machine: {} still changed after {PASSES} passes",
                        unsettled.join(", ")
                    ));
                }
                let mut out = self.outcome(term, diagnostics, reports, None);
                out.unsettled = unsettled;
                self.record_quick(&out);
                return out;
            }
            let found = self.changes();
            if found.is_empty() {
                let out = self.outcome(term, diagnostics, reports, None);
                self.record_quick(&out);
                return out;
            }
            // (the pass's PDF is complete: shown while the next settles)
            observe(crate::events::Progress::Settling(passes));
            // (a save since: this pass is the first of its rebuild, with the
            // job's own files as this one left them)
            let edited = found.iter().any(|f| !f.own);
            // (why the next pass runs: the job's own files this one changed)
            let mut again: Vec<String> = found
                .iter()
                .map(|f| {
                    let name = f.key.rsplit(|&c| c == 0).next().unwrap_or(&f.key);
                    String::from_utf8_lossy(undotted(name)).into_owned()
                })
                .collect();
            again.sort();
            again.dedup();
            if edited {
                self.supersede(&found, observe);
                passes = 1;
            } else {
                passes += 1;
                observe(crate::events::Progress::Again(&again));
                observe(crate::events::Progress::PassStart(passes));
            }
            let changes = self.take(found);
            let ((line, r), superseded) = self.apply_to_the_end(changes, observe);
            if superseded {
                passes = 1;
            }
            report = r;
            reports.push(if edited || superseded {
                line
            } else {
                line.replace("rebuilt", "converged a pass")
            });
        }
    }

    /// What the tools (BibTeX, makeindex) wrote since, taken from
    /// `between`: the job's own files, as what the link wrote, not edits.
    fn own_tools(&mut self, between: &mut crate::Between) {
        let tooled = std::mem::take(&mut between.bib.written)
            .into_iter()
            .chain(std::mem::take(&mut between.idx.written));
        for (p, h) in tooled {
            let time = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            if let Ok(real) = std::fs::canonicalize(&p) {
                self.own.insert(real, (h, time));
            }
        }
    }

    /// Record what the build read and wrote, and its result `out`, for a
    /// restart with nothing changed ([`quick`]), on a thread of its own
    /// (it hashes the inputs and looks at them on disk).
    fn record_quick(&mut self, out: &Outcome) {
        let Some(k) = &self.keeper else { return };
        if let Some(h) = self.quick.take() {
            let _ = h.join();
        }
        if !quick::on() || !self.b.settled() {
            return;
        }
        let (dir, key) = k.place();
        let init = self.b.initial().tex().host();
        let served = init.served_paths();
        let mut inputs: Vec<(Vec<u8>, Arc<[u8]>)> = served
            .iter()
            .filter(|k| *k != CLOCK)
            .filter_map(|k| init.file(k).map(|f| (k.clone(), f)))
            .collect();
        // (a file not found at first and found since, as a first build's
        // .aux: an input by the path it is found at now, if that is what
        // the build read; else no record)
        for (k, read) in init.overrides.iter() {
            if !k.starts_with(b"\0") || served.contains(k) {
                continue;
            }
            let asked = init
                .missed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(k)
                .cloned();
            match asked.and_then(|(name, kind)| init.native().read_file(&name, kind)) {
                Some(f) if f.contents[..] == read[..] => inputs.push((f.name, read.clone())),
                _ => {
                    quick::forget(&dir, key);
                    return;
                }
            }
        }
        let missed: Vec<(Vec<u8>, Vec<u8>, FileKind)> = init
            .missed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(k, _)| !init.overrides.contains_key(*k))
            .map(|(k, (n, kind))| (k.clone(), n.clone(), *kind))
            .collect();
        let outputs: Vec<(Vec<u8>, u128)> = self
            .files
            .iter()
            .filter_map(|(n, _)| self.written.get(n).map(|h| (n.clone(), *h)))
            .collect();
        let b = quick::Build {
            inputs,
            missed,
            clock: init.file(CLOCK).map(|c| c.to_vec()),
            outputs,
            outcome: out.clone(),
        };
        self.quick = Some(std::thread::spawn(move || quick::record(&dir, key, &b)));
    }

    /// Wait for the no-op restart's record being written.
    fn finish_quick(&mut self) {
        if let Some(h) = self.quick.take() {
            let _ = h.join();
        }
    }

    fn outcome(
        &self,
        term: Vec<u8>,
        diagnostics: Vec<partex_core::diag::Diagnostic>,
        reports: Vec<String>,
        history: Option<i32>,
    ) -> Outcome {
        Outcome {
            term,
            history: history.unwrap_or_else(|| self.b.final_state().history().unwrap_or(3)),
            diagnostics,
            outputs: self
                .files
                .iter()
                .map(|(n, b)| (n.clone(), b.len()))
                .collect(),
            reports,
            unsettled: Vec::new(),
        }
    }

    /// Link the build's effects into `self.linked`, resolving again the
    /// regions `touched` names (`None`: all) and taking the others from
    /// the last link (`PARTEX_MACHINE_LINK_CACHE=0`: all resolved).
    fn link(&mut self, touched: Option<&std::collections::BTreeSet<u64>>) -> std::io::Result<()> {
        let threads = partex_incr::Threads::available();
        let mut deflating = self.b.final_state().tex().host().clone();
        if let Some(file) = self.deflated_file() {
            deflating.keep_deflated(file);
        }
        let mut deflate = |level, data: &[u8]| deflating.deflate(level, data);
        let cached = !std::env::var("PARTEX_MACHINE_LINK_CACHE").is_ok_and(|v| v == "0");
        let linked = if cached {
            let fx: Vec<(u64, &[partex_core::effects::Effect])> = self
                .b
                .keyed_traces()
                .map(|(k, t)| (k, t.effects.as_slice()))
                .collect();
            let changed = |k: u64| touched.is_none_or(|t| t.contains(&k));
            partex_core::effects::link_cached(
                &fx,
                &changed,
                &mut self.link_cache,
                &threads,
                &mut deflate,
            )
        } else {
            self.link_cache = partex_core::effects::LinkCache::default();
            let fx: Vec<&[partex_core::effects::Effect]> =
                self.b.traces().map(|t| t.effects.as_slice()).collect();
            partex_core::effects::link(&fx, &threads, &mut deflate)
        };
        match linked {
            Ok(l) => self.linked = Some(l),
            Err(e) => {
                // (linked again next time)
                self.linked = None;
                return Err(std::io::Error::other(format!(
                    "the link step failed: {e:?}"
                )));
            }
        }
        Ok(())
    }

    /// Link the build's output and write the files whose bytes changed
    /// (the PDF or DVI file first, renamed into place); the terminal text
    /// and the diagnostics. If no region's effects changed since the last
    /// link, its output is taken again (`PARTEX_MACHINE_LINK_REUSE=0`:
    /// linked every time): only the `\write` files may have changed.
    fn write(&mut self) -> std::io::Result<(Vec<u8>, Vec<partex_core::diag::Diagnostic>)> {
        let t = Instant::now();
        let changed = self.b.take_effects_changed();
        let reuse = !changed
            && self.linked.is_some()
            && !std::env::var("PARTEX_MACHINE_LINK_REUSE").is_ok_and(|v| v == "0");
        // (the regions put in since the last link: those resolved again)
        let touched = self.b.take_touched();
        if !reuse {
            self.link(touched.as_ref())?;
        }
        let t_link = t.elapsed();
        let Some(linked) = &self.linked else {
            return Err(std::io::Error::other("the link step failed"));
        };
        let host = self.b.final_state().tex().host();
        let mut files: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
        for (id, name) in &host.opened {
            let (bytes, from_link) = match linked.files.get(id) {
                Some(b) => (b.clone(), true),
                None => match host.written.get(id) {
                    Some(w) => (w.to_vec(), false),
                    None => continue,
                },
            };
            let bytes = host.piped_out(*id, name, bytes.into()).into_owned();
            files.push((name.clone(), bytes, from_link));
        }
        // (the `SyncTeX` file, rendered from the regions' events: again only
        // when they changed)
        if !reuse || self.synctex.is_none() {
            self.synctex = synctex_file(&self.b);
        }
        if let Some((name, other, bytes)) = &self.synctex {
            for n in [Some(other), bytes.is_none().then_some(name)]
                .into_iter()
                .flatten()
            {
                let p = crate::native::path(n);
                if p.exists() {
                    std::fs::remove_file(&p)?;
                }
            }
            if let Some(b) = bytes {
                files.push((name.clone(), b.clone(), true));
            }
        }
        let main = |n: &[u8]| n.ends_with(b".pdf") || n.ends_with(b".dvi");
        // (renamed into place whole: a viewer or an editor reads them as
        // they change)
        let whole = |n: &[u8]| main(n) || n.ends_with(b".synctex.gz") || n.ends_with(b".synctex");
        files.sort_by_key(|(n, _, _)| !main(n));
        for (name, bytes, from_link) in &files {
            let p = crate::native::path(name);
            // (a file from a link taken again is as last written)
            if *from_link && reuse && self.written.contains_key(name) && p.exists() {
                continue;
            }
            let h = quick::hash(bytes);
            if self.written.get(name) == Some(&h) && p.exists() {
                continue;
            }
            if whole(name) {
                write_whole(&p, bytes)?;
            } else {
                std::fs::write(&p, bytes)?;
            }
            self.written.insert(name.clone(), h);
            // (the job's own file from now on, while it stays as written)
            let time = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            if let Some(real) = real_path(&mut self.real, name) {
                self.own.insert(real, (h, time));
            }
        }
        if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
            eprintln!(
                "phitex: machine: {} in {:.1} ms ({} regions resolved, {} as last time), files written in {:.1} ms",
                if reuse {
                    "the last link taken again"
                } else {
                    "linked"
                },
                t_link.as_secs_f64() * 1e3,
                self.link_cache.resolved,
                self.link_cache.reused,
                t.elapsed().saturating_sub(t_link).as_secs_f64() * 1e3
            );
        }
        let files: Vec<(Vec<u8>, Vec<u8>)> = files.into_iter().map(|(n, b, _)| (n, b)).collect();
        self.files = files;
        // (a page shipped is a note, as a session logs it)
        let mut diagnostics = linked.diagnostics.clone();
        if self.b.final_state().tex().host().notes() {
            diagnostics.extend(linked.pages.iter().map(|c| partex_core::diag::Diagnostic {
                severity: partex_core::diag::Severity::Note,
                code: crate::events::PAGE,
                message: c.to_string().into_bytes(),
                help: Vec::new(),
                frames: Vec::new(),
                suggestions: Vec::new(),
                boxed: None,
            }));
        }
        Ok((linked.term.clone(), diagnostics))
    }
}

/// A machine's host kept with its snapshots (DESIGN.md §7.9): the state
/// each clone holds; what every clone shares is saved once
/// ([`MachineHost::save_shared`]).
impl partex_core::machine::StoreHost for MachineHost {
    fn save_own(&self, s: &mut partex_core::persist::Saver) {
        use partex_core::persist::Persist;
        let Self {
            native: _,
            served: _,
            overrides,
            lines: _,
            opened,
            piped,
            written,
            appended,
            read_back,
            next_id,
            reads,
            missed: _,
            term_lines: _,
            term_pos,
            now,
            epoch,
            clock_read,
            memo: _,
            fonts: _,
            removed: _,
        } = self;
        overrides.save(s);
        opened.save(s);
        piped.save(s);
        written.len().save(s);
        for (id, w) in written {
            id.save(s);
            w.pieces.save(s);
            w.tail.save(s);
        }
        appended.save(s);
        read_back.save(s);
        next_id.save(s);
        reads.save(s);
        term_pos.save(s);
        now.save(s);
        epoch.save(s);
        clock_read.clone().take().save(s);
    }

    fn load_own(&self, l: &mut partex_core::persist::Loader) -> Option<Self> {
        use partex_core::persist::Persist;
        let overrides = Persist::load(l)?;
        let opened = Persist::load(l)?;
        let piped = Persist::load(l)?;
        let n = usize::load(l)?;
        let mut written = BTreeMap::new();
        for _ in 0..n {
            let id = u32::load(l)?;
            let w = WrittenBuf {
                pieces: Persist::load(l)?,
                tail: Persist::load(l)?,
            };
            written.insert(id, w);
        }
        Some(Self {
            native: self.native.clone(),
            served: self.served.clone(),
            overrides,
            lines: self.lines.clone(),
            opened,
            piped,
            written,
            appended: Persist::load(l)?,
            read_back: Persist::load(l)?,
            next_id: Persist::load(l)?,
            reads: Persist::load(l)?,
            missed: self.missed.clone(),
            term_lines: self.term_lines.clone(),
            term_pos: Persist::load(l)?,
            now: Persist::load(l)?,
            epoch: Persist::load(l)?,
            clock_read: {
                let f = ReadFlag::default();
                if bool::load(l)? {
                    f.set();
                }
                f
            },
            memo: self.memo.clone(),
            fonts: self.fonts.clone(),
            removed: self.removed.clone(),
        })
    }
}

impl MachineHost {
    /// What output file `id` (`name`) holds, written `bytes`: `XeTeX`'s
    /// XDV made a PDF, as `xdvipdfmx -q -E -o NAME` with the XDV piped in
    /// (`FileKind::XdvPipe`), every other file as written.
    fn piped_out<'a>(
        &self,
        id: u32,
        name: &[u8],
        bytes: std::borrow::Cow<'a, [u8]>,
    ) -> std::borrow::Cow<'a, [u8]> {
        if self.piped.contains(&id) {
            crate::dpxfiles::xdv_to_pdf(&bytes, name).0.into()
        } else {
            bytes
        }
    }

    /// Save what every clone of this host shares: the files served, the
    /// files not found, the terminal lines read.
    pub fn save_shared(&self, s: &mut partex_core::persist::Saver) {
        use partex_core::persist::Persist;
        let lock = |m: &Mutex<Files>| {
            m.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        lock(&self.served).save(s);
        self.missed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .save(s);
        self.term_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .save(s);
        self.fonts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .save(s);
    }

    /// A template host over `native` with the shared parts
    /// [`MachineHost::save_shared`] saved (each state loaded takes them);
    /// `native` back if they do not load.
    #[allow(clippy::result_large_err)] // (the host, moved back)
    pub fn load_shared(
        native: NativeHost,
        l: &mut partex_core::persist::Loader,
    ) -> Result<Self, NativeHost> {
        use partex_core::persist::Persist;
        let parts = (|| {
            let served: Files = Persist::load(l)?;
            let missed: Missed = Persist::load(l)?;
            let term: Vec<Option<Vec<u8>>> = Persist::load(l)?;
            let fonts: FontSlots = Persist::load(l)?;
            Some((served, missed, term, fonts))
        })();
        let Some((served, missed, term, fonts)) = parts else {
            return Err(native);
        };
        let mut h = Self::new(native);
        h.served = Arc::new(Mutex::new(served));
        h.missed = Arc::new(Mutex::new(missed));
        h.term_lines = Arc::new(Mutex::new(term));
        // (the build's slots, whether they are on as it was built)
        h.fonts = Arc::new(Mutex::new(fonts));
        Ok(h)
    }

    /// The native host back, from the last clone of this host.
    ///
    /// # Panics
    ///
    /// If another clone is alive.
    pub fn into_native(self) -> NativeHost {
        Arc::try_unwrap(self.native)
            .ok()
            .expect("the last clone of the host")
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
