//! The content-addressed store (DESIGN.md §7.9): blobs named by the hash
//! of their bytes, packed, and roots that name what a saved build needs.
//!
//! Layout under the store directory (`PARTEX_STORE_DIR`, else `store`
//! in `phitex.toml`, else `$XDG_CACHE_HOME/phitex/store`, else
//! `~/.cache/phitex/store`; `PARTEX_STORE=0` turns it off):
//!
//! - `packs/<hash>.pack`: blobs one after the other; `<hash>` is that of
//!   the pack's bytes, so a pack never changes once written. Each blob is
//!   a byte saying how it is kept, then its bytes: 0 as they are, 1
//!   compressed (`lz.rs`; `PARTEX_STORE_COMPRESS=0` writes all as they
//!   are). A blob's name is the hash of its bytes, not of what is kept.
//! - `packs/<hash>.idx`: the pack's index, a blob hash, offset and length
//!   per blob (16 + 8 + 4 bytes each).
//! - `packs/<hash>.kids`: the blobs each of the pack's blobs refers to,
//!   in the index's order (a count, then their hashes), in frames of
//!   about 4 MB, each a length and then its bytes kept as a blob is,
//!   compressed or not (a save writes them as it goes): what a blob keeps
//!   alive. A save keeps the blobs its root reaches, so a part saved
//!   before and referred to again keeps what it refers to.
//! - `roots/<key>`: what a saved build needs: the packs it reads from,
//!   the hash of the rest, then its root bytes. Named by the job's
//!   identity; replaced whole.
//!
//! What is read is checked: a blob against its name, a root against its
//! hash (anything else is as if it were not there).
//!
//! Every file is written whole to a temporary name and renamed into
//! place, so a reader sees all of it or none. Blobs are read with
//! positioned reads of what is asked for (`read_exact_at`), not mapped:
//! no `unsafe`, and a pack is never rewritten, so a read never meets a
//! file changing under it.
//!
//! The directory is bounded (`PARTEX_STORE_MAX` bytes, 4 GiB by
//! default): after a save the least recently used roots go, then the
//! packs no root names. A save moves the live blobs of a pack that is
//! mostly dead into its new pack, so dead blobs do not pile up.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::hash::Hasher;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use partex_core::StableHasher;

/// The first bytes of a root.
const ROOT_MAGIC: &[u8; 16] = b"partex-store/6\0\0";

/// A blob as kept in a pack (see the module's comment).
fn kept_form(b: &[u8], compress: bool) -> Vec<u8> {
    if compress && b.len() >= 128 {
        let e = crate::lz::encode(b);
        // (kept compressed only if that saves an eighth)
        if e.len() < b.len() - b.len() / 8 {
            let mut v = Vec::with_capacity(e.len() + 1);
            v.push(1);
            v.extend_from_slice(&e);
            return v;
        }
    }
    let mut v = Vec::with_capacity(b.len() + 1);
    v.push(0);
    v.extend_from_slice(b);
    v
}

/// A blob's bytes from how a pack keeps them.
fn unkeep(mut k: Vec<u8>) -> Option<Vec<u8>> {
    match *k.first()? {
        0 => {
            k.remove(0);
            Some(k)
        }
        1 => crate::lz::decode(&k[1..]),
        _ => None,
    }
}

/// Whether saves compress blobs (`PARTEX_STORE_COMPRESS=0`: no).
fn compressing() -> bool {
    !std::env::var("PARTEX_STORE_COMPRESS").is_ok_and(|v| v == "0")
}

/// Bytes per index entry: hash, offset, length.
const ENTRY: usize = 16 + 8 + 4;

/// The store's directory, if the store is on.
pub fn dir(configured: Option<&Path>) -> Option<PathBuf> {
    if std::env::var_os("PARTEX_STORE").is_some_and(|v| v == "0") {
        return None;
    }
    std::env::var_os("PARTEX_STORE_DIR")
        .map(PathBuf::from)
        .or_else(|| configured.map(Path::to_path_buf))
        .or_else(|| crate::cache::user_cache("phitex").map(|d| d.join("store")))
}

/// Forget the build saved under `key` (`phitex clean`): its root goes,
/// and the packs no other root names go with the next [`collect`].
/// Whether there was one.
pub fn forget(dir: &Path, key: u128) -> bool {
    std::fs::remove_file(root_path(dir, key)).is_ok()
}

/// The bound on the store's size.
pub fn max() -> u64 {
    max_bytes()
}

fn max_bytes() -> u64 {
    std::env::var("PARTEX_STORE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4 << 30)
}

/// How long a saved build or cached value is kept unused
/// (`PARTEX_STORE_DAYS` days, 30 by default; 0 for ever).
pub fn max_age() -> Option<std::time::Duration> {
    let days: u64 = std::env::var("PARTEX_STORE_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    (days > 0).then(|| std::time::Duration::from_secs(days * 24 * 3600))
}

fn hex(h: u128) -> String {
    format!("{h:032x}")
}

/// Write `bytes` to `path` whole (a temporary file renamed into place).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Mark `path` as used (for the least-recently-used bound).
fn touch(path: &Path) {
    if let Ok(f) = File::options().append(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Where a blob is: its pack (by position in the root's list), offset
/// and length.
type Place = (usize, u64, u32);

/// A root opened for reading: its packs, their indexes and the root's
/// bytes.
pub struct Opened {
    packs: Vec<(u128, File)>,
    places: HashMap<u128, Place>,
    /// The root's bytes.
    pub root: Vec<u8>,
    /// Bytes read from packs so far.
    pub read: std::sync::atomic::AtomicU64,
}

impl Opened {
    /// The blob named `h`, read from its pack.
    pub fn get(&self, h: u128) -> Option<Vec<u8>> {
        let &(p, off, len) = self.places.get(&h)?;
        let (_, file) = self.packs.get(p)?;
        // (positioned reads of a shared file: threads read at once)
        let mut buf = vec![0; len as usize];
        file.read_exact_at(&mut buf, off).ok()?;
        self.read
            .fetch_add(u64::from(len), std::sync::atomic::Ordering::Relaxed);
        let buf = unkeep(buf)?;
        (partex_core::persist::blob_hash(&buf) == h).then_some(buf)
    }

    /// The blobs this root's packs hold, by hash, with their packs.
    pub fn stored(&self) -> Stored {
        Stored {
            places: self
                .places
                .iter()
                .map(|(h, &(p, off, len))| (*h, (self.packs[p].0, off, len)))
                .collect(),
            kids: Arc::default(),
        }
    }
}

/// A hasher for blob names, which are hashes already: their low bits.
#[derive(Default, Clone, Copy)]
struct Named(u64);

impl std::hash::Hasher for Named {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(*b)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }
    #[allow(clippy::cast_possible_truncation)] // (the low bits)
    fn write_u128(&mut self, n: u128) {
        self.0 ^= n as u64;
    }
}

type ByName = std::hash::BuildHasherDefault<Named>;

/// What each kept blob refers to.
type Kids = HashMap<u128, Arc<[u128]>, ByName>;

/// The blobs kept in packs, by hash: pack name, offset, length (what a
/// save need not write again), and what they refer to, once read (kept
/// in memory from save to save: `PARTEX_STORE_KIDS_CACHE=0` reads them
/// from the packs every time).
#[derive(Clone, Default)]
pub struct Stored {
    places: HashMap<u128, (u128, u64, u32)>,
    kids: Arc<OnceLock<Option<Kids>>>,
}

impl Stored {
    /// The hashes of the blobs kept.
    pub fn hashes(&self) -> BTreeSet<u128> {
        self.places.keys().copied().collect()
    }

    /// What the kept blobs refer to, read from the packs' lists the
    /// first time (`None`: a list does not read back). A session reads
    /// them on a thread of its own after a load, while it rebuilds.
    /// Read what the kept blobs refer to now (a session does, on a
    /// thread of its own after a load, for the next save).
    pub fn warm(&self, dir: &Path) {
        let _ = self.kids(dir);
    }

    fn kids(&self, dir: &Path) -> Option<&Kids> {
        let cache = !std::env::var("PARTEX_STORE_KIDS_CACHE").is_ok_and(|v| v == "0");
        if !cache {
            return None;
        }
        self.kids
            .get_or_init(|| read_all_kids(dir, &self.places))
            .as_ref()
    }
}

/// What the blobs of the packs named in `places` refer to.
fn read_all_kids(dir: &Path, places: &HashMap<u128, (u128, u64, u32)>) -> Option<Kids> {
    let packs: BTreeSet<u128> = places.values().map(|v| v.0).collect();
    let mut kids = Kids::with_capacity_and_hasher(places.len(), ByName::default());
    for p in packs {
        for (h, k) in read_kids(dir, p)? {
            kids.insert(h, Arc::from(k));
        }
    }
    Some(kids)
}

fn pack_path(dir: &Path, name: u128) -> PathBuf {
    dir.join("packs").join(format!("{}.pack", hex(name)))
}

fn idx_path(dir: &Path, name: u128) -> PathBuf {
    dir.join("packs").join(format!("{}.idx", hex(name)))
}

fn kids_path(dir: &Path, name: u128) -> PathBuf {
    dir.join("packs").join(format!("{}.kids", hex(name)))
}

/// Read what each blob of a pack refers to (`None`: unreadable).
fn read_kids(dir: &Path, name: u128) -> Option<Vec<(u128, Vec<u128>)>> {
    let idx = read_index(dir, name)?;
    let file = std::fs::read(kids_path(dir, name)).ok()?;
    let mut out = Vec::with_capacity(idx.len());
    let mut hashes = idx.into_iter().map(|(h, _, _)| h);
    let mut f = 0;
    while f < file.len() {
        let len = u32::from_le_bytes(file.get(f..f + 4)?.try_into().ok()?) as usize;
        let b = unkeep(file.get(f + 4..f + 4 + len)?.to_vec())?;
        f += 4 + len;
        let mut at = 0;
        while at < b.len() {
            let n = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
            at += 4;
            let kids = b
                .get(at..at + n.checked_mul(16)?)?
                .as_chunks::<16>()
                .0
                .iter()
                .map(|c| u128::from_le_bytes(*c))
                .collect();
            at += n * 16;
            out.push((hashes.next()?, kids));
        }
    }
    hashes.next().is_none().then_some(out)
}

fn root_path(dir: &Path, key: u128) -> PathBuf {
    dir.join("roots").join(hex(key))
}

/// Read a pack's index.
fn read_index(dir: &Path, name: u128) -> Option<Vec<(u128, u64, u32)>> {
    let b = std::fs::read(idx_path(dir, name)).ok()?;
    if b.len() % ENTRY != 0 {
        return None;
    }
    Some(
        b.as_chunks::<ENTRY>()
            .0
            .iter()
            .map(|e| {
                let h = u128::from_le_bytes(e[..16].try_into().unwrap_or_default());
                let off = u64::from_le_bytes(e[16..24].try_into().unwrap_or_default());
                let len = u32::from_le_bytes(e[24..28].try_into().unwrap_or_default());
                (h, off, len)
            })
            .collect(),
    )
}

/// Open the root `key`: `None` if there is none (or it is malformed, or
/// a pack it names is gone).
pub fn open(dir: &Path, key: u128) -> Option<Opened> {
    let path = root_path(dir, key);
    let b = std::fs::read(&path).ok()?;
    let rest = b.strip_prefix(&ROOT_MAGIC[..])?;
    let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let names_end = 4 + n.checked_mul(16)?;
    let names: Vec<u128> = rest
        .get(4..names_end)?
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| u128::from_le_bytes(*c))
        .collect();
    let sum = u128::from_le_bytes(rest.get(names_end..names_end + 16)?.try_into().ok()?);
    let names_end = names_end + 16;
    if partex_core::persist::blob_hash(&rest[names_end..]) != sum {
        return None;
    }
    let mut places = HashMap::new();
    for (i, &name) in names.iter().enumerate() {
        for (h, off, len) in read_index(dir, name)? {
            places.insert(h, (i, off, len));
        }
        touch(&pack_path(dir, name));
        touch(&idx_path(dir, name));
    }
    touch(&path);
    // (the packs opened now: one removed later, by another job's
    // eviction, stays readable through its open file)
    let mut packs = Vec::with_capacity(names.len());
    for n in names {
        packs.push((n, File::open(pack_path(dir, n)).ok()?));
    }
    Some(Opened {
        packs,
        places,
        root: rest[names_end..].to_vec(),
        read: std::sync::atomic::AtomicU64::new(0),
    })
}

/// What a save wrote.
#[derive(Debug, Default)]
pub struct Saved {
    pub new_blobs: usize,
    /// The new blobs' bytes as kept (compressed), and as they are.
    pub new_bytes: u64,
    pub new_raw_bytes: u64,
    pub moved_bytes: u64,
    pub root_bytes: usize,
    pub packs: usize,
    pub live: usize,
    /// When each phase ended, from the start (for `PARTEX_STORE_DEBUG`).
    pub phases: Vec<(&'static str, std::time::Duration)>,
}

/// Bytes of references a save gathers into a frame of a pack's `.kids`
/// file (in the tests, little: a test's references span frames).
const KIDS_FRAME: usize = if cfg!(test) { 64 } else { 4 << 20 };

/// Bytes of new blobs a save compresses at once before writing them (in
/// the tests, little: a test's blobs span batches, in little space).
const SAVE_BATCH: usize = if cfg!(test) { 1 << 16 } else { 64 << 20 };

/// A save's new pack, written as its blobs come (a [`Saver::merkle`]'s
/// sink, [`PackWriter::add`]), to a temporary file: a batch of
/// [`SAVE_BATCH`] bytes compressed at a time, on several threads when it
/// is large, each blob let go once written. A save then holds each blob
/// once, and only until its batch is written, not every blob as encoded,
/// as kept and as the pack at once. [`PackWriter::finish`] writes the
/// root; a writer dropped before that leaves nothing behind.
///
/// [`Saver::merkle`]: partex_core::persist::Saver::merkle
pub struct PackWriter {
    dir: PathBuf,
    t0: std::time::Instant,
    /// The pack's temporary file and its references' (`None` once
    /// renamed or removed).
    tmp: Option<(PathBuf, PathBuf)>,
    file: std::io::BufWriter<File>,
    kids_file: std::io::BufWriter<File>,
    /// The references of the blobs written since the last frame.
    frame: Vec<u8>,
    /// The pack's name so far: the hash of its bytes.
    name: StableHasher,
    index: Vec<(u128, u64, u32)>,
    len: u64,
    /// What each new blob refers to.
    kids: Kids,
    batch: Vec<(u128, Vec<u8>)>,
    batch_bytes: usize,
    compress: bool,
    /// The first write that failed (the rest are not tried).
    error: Option<std::io::Error>,
    out: Saved,
}

impl PackWriter {
    /// A new pack in store `dir`.
    pub fn new(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir.join("packs"))?;
        std::fs::create_dir_all(dir.join("roots"))?;
        // (temporary names no collection takes: `.tmp`)
        let tmp = |what: &str| {
            dir.join("packs")
                .join(format!("save-{what}.tmp{}", std::process::id()))
        };
        let tmp = (tmp("pack"), tmp("kids"));
        let file = std::io::BufWriter::new(File::create(&tmp.0)?);
        let kids_file = std::io::BufWriter::new(File::create(&tmp.1)?);
        Ok(Self {
            dir: dir.to_path_buf(),
            t0: std::time::Instant::now(),
            tmp: Some(tmp),
            file,
            kids_file,
            frame: Vec::new(),
            name: StableHasher::new(),
            index: Vec::new(),
            len: 0,
            kids: Kids::with_hasher(ByName::default()),
            batch: Vec::new(),
            batch_bytes: 0,
            compress: compressing(),
            error: None,
            out: Saved::default(),
        })
    }

    /// Add a new blob (each once: a blob added again is not written again).
    pub fn add(&mut self, (h, bytes, kids): partex_core::persist::MerkleBlob) {
        if self.kids.insert(h, Arc::from(kids)).is_some() {
            return;
        }
        self.out.new_raw_bytes += bytes.len() as u64;
        self.batch_bytes += bytes.len();
        self.batch.push((h, bytes));
        if self.batch_bytes >= SAVE_BATCH {
            self.write_batch();
        }
    }

    /// Compress the batch and write it to the pack.
    fn write_batch(&mut self) {
        let batch = std::mem::take(&mut self.batch);
        let bytes = std::mem::take(&mut self.batch_bytes);
        if batch.is_empty() || self.error.is_some() {
            return;
        }
        let threads = if bytes < (8 << 20) {
            1
        } else {
            std::thread::available_parallelism()
                .map_or(4, std::num::NonZero::get)
                .min(8)
        };
        let per = batch.len().div_ceil(threads).max(1);
        let compress = self.compress;
        let kept: Vec<Vec<(u128, Vec<u8>)>> = std::thread::scope(|sc| {
            let jobs: Vec<_> = batch
                .chunks(per)
                .map(|part| {
                    sc.spawn(move || {
                        part.iter()
                            .map(|(h, b)| (*h, kept_form(b, compress)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|j| j.join().unwrap_or_default())
                .collect()
        });
        if kept.iter().map(Vec::len).sum::<usize>() != batch.len() {
            self.error = Some(std::io::Error::other("a blob was not kept"));
            return;
        }
        drop(batch);
        for (h, k) in kept.iter().flatten() {
            let kids = self.kids.get(h).cloned().unwrap_or_default();
            if let Err(e) = self.put(*h, k, &kids) {
                self.error = Some(e);
                return;
            }
            self.out.new_blobs += 1;
            self.out.new_bytes += k.len() as u64;
        }
    }

    /// Append blob `h`, kept as `b`, which refers to `kids`.
    fn put(&mut self, h: u128, b: &[u8], kids: &[u128]) -> std::io::Result<()> {
        self.index
            .push((h, self.len, u32::try_from(b.len()).unwrap_or(u32::MAX)));
        self.file.write_all(b)?;
        self.name.write(b);
        self.len += b.len() as u64;
        self.frame
            .extend_from_slice(&u32::try_from(kids.len()).unwrap_or(0).to_le_bytes());
        for k in kids {
            self.frame.extend_from_slice(&k.to_le_bytes());
        }
        if self.frame.len() >= KIDS_FRAME {
            self.write_frame()?;
        }
        Ok(())
    }

    /// Write the references gathered since the last frame as a frame.
    fn write_frame(&mut self) -> std::io::Result<()> {
        if self.frame.is_empty() {
            return Ok(());
        }
        let k = kept_form(&self.frame, self.compress);
        self.frame.clear();
        self.kids_file
            .write_all(&u32::try_from(k.len()).unwrap_or(u32::MAX).to_le_bytes())?;
        self.kids_file.write_all(&k)
    }

    /// Remove the temporary files, if they are still there.
    fn discard(&mut self) {
        if let Some((pack, kids)) = self.tmp.take() {
            let _ = std::fs::remove_file(pack);
            let _ = std::fs::remove_file(kids);
        }
    }

    /// Write a root `key` for `root`, which refers to blobs `refs`: those
    /// added (each with the blobs it refers to) and those of `stored`.
    /// What the root reaches is kept; the updated [`Stored`] (the packs the
    /// root now names), and what the save wrote.
    #[allow(clippy::too_many_lines)] // (one pass over the packs)
    pub fn finish(
        mut self,
        key: u128,
        root: &[u8],
        refs: &[u128],
        stored: &Stored,
    ) -> std::io::Result<(Stored, Saved)> {
        self.write_batch();
        if let Some(e) = self.error.take() {
            self.discard();
            return Err(e);
        }
        let t0 = self.t0;
        self.out.phases.push(("blobs", t0.elapsed()));
        let dir = self.dir.clone();
        let dir = dir.as_path();
        // What each blob refers to: the new ones', the stored packs' (kept
        // from the last save or read now).
        let read_now;
        let old: &Kids = if let Some(k) = stored.kids(dir) {
            k
        } else {
            read_now = read_all_kids(dir, &stored.places)
                .ok_or_else(|| std::io::Error::other("a pack's references do not read back"))?;
            &read_now
        };
        let new_kids = std::mem::take(&mut self.kids);
        let kids_of = |h: &u128| -> Option<&[u128]> {
            new_kids.get(h).or_else(|| old.get(h)).map(|k| &k[..])
        };
        self.out.phases.push(("references", t0.elapsed()));
        // (what the root reaches)
        let mut live: std::collections::HashSet<u128, ByName> =
            std::collections::HashSet::with_capacity_and_hasher(
                stored.places.len() + new_kids.len(),
                ByName::default(),
            );
        let mut todo: Vec<u128> = refs.to_vec();
        while let Some(h) = todo.pop() {
            if live.insert(h)
                && let Some(k) = kids_of(&h)
            {
                todo.extend(k.iter().copied());
            }
        }
        // Live bytes per stored pack; a mostly dead pack's live blobs move.
        let mut total: BTreeMap<u128, u64> = BTreeMap::new();
        let mut alive: BTreeMap<u128, u64> = BTreeMap::new();
        for (h, &(p, _, len)) in &stored.places {
            *total.entry(p).or_default() += u64::from(len);
            if live.contains(h) {
                *alive.entry(p).or_default() += u64::from(len);
            }
        }
        let keep: BTreeSet<u128> = total
            .iter()
            .filter(|(p, t)| {
                let a = alive.get(p).copied().unwrap_or(0);
                // (a pack less than half alive is rewritten)
                a > 0 && 2 * a >= **t
            })
            .map(|(p, _)| *p)
            .collect();
        self.out.live = live.len();
        self.out.phases.push(("liveness", t0.elapsed()));
        // (the live blobs of the packs let go, read back)
        let moved = (|| -> std::io::Result<()> {
            let mut files: BTreeMap<u128, File> = BTreeMap::new();
            let mut moving: Vec<(u128, (u128, u64, u32))> = live
                .iter()
                .filter_map(|h| Some((*h, *stored.places.get(h)?)))
                .filter(|(_, (p, _, _))| !keep.contains(p))
                .collect();
            moving.sort_unstable();
            let mut buf = Vec::new();
            for (h, (p, off, len)) in moving {
                let f = match files.entry(p) {
                    std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::btree_map::Entry::Vacant(e) => {
                        e.insert(File::open(pack_path(dir, p))?)
                    }
                };
                buf.resize(len as usize, 0);
                f.read_exact_at(&mut buf, off)?;
                self.put(h, &buf, kids_of(&h).unwrap_or(&[]))?;
                self.out.moved_bytes += u64::from(len);
            }
            self.write_frame()?;
            self.file.flush()?;
            self.kids_file.flush()
        })();
        if let Err(e) = moved {
            self.discard();
            return Err(e);
        }
        let mut places: HashMap<u128, (u128, u64, u32)> = stored
            .places
            .iter()
            .filter(|(_, (p, _, _))| keep.contains(p))
            .map(|(h, v)| (*h, *v))
            .collect();
        let mut names: Vec<u128> = keep.iter().copied().collect();
        let index = std::mem::take(&mut self.index);
        if index.is_empty() {
            self.discard();
        } else {
            let name = self.name.finish128();
            let mut idx = Vec::with_capacity(index.len() * ENTRY);
            for (h, off, len) in &index {
                idx.extend_from_slice(&h.to_le_bytes());
                idx.extend_from_slice(&off.to_le_bytes());
                idx.extend_from_slice(&len.to_le_bytes());
            }
            self.out.phases.push(("move, index", t0.elapsed()));
            // (the pack and its references before its index: an index
            // names only packs there)
            if let Some((pack, kids)) = self.tmp.clone() {
                std::fs::rename(&pack, pack_path(dir, name))?;
                std::fs::rename(&kids, kids_path(dir, name))?;
                self.tmp = None;
            }
            write_atomic(&idx_path(dir, name), &idx)?;
            for (h, off, len) in index {
                places.insert(h, (name, off, len));
            }
            names.push(name);
        }
        let tail = root;
        let mut r = Vec::with_capacity(ROOT_MAGIC.len() + 4 + names.len() * 16 + 16 + tail.len());
        r.extend_from_slice(ROOT_MAGIC);
        r.extend_from_slice(&u32::try_from(names.len()).unwrap_or(0).to_le_bytes());
        for n in &names {
            r.extend_from_slice(&n.to_le_bytes());
        }
        r.extend_from_slice(&partex_core::persist::blob_hash(tail).to_le_bytes());
        r.extend_from_slice(tail);
        write_atomic(&root_path(dir, key), &r)?;
        self.out.root_bytes = r.len();
        self.out.packs = names.len();
        self.out.phases.push(("write", t0.elapsed()));
        collect(dir, max_bytes());
        self.out.phases.push(("collect", t0.elapsed()));
        // (what the blobs kept refer to, for the next save)
        let kids = Arc::new(OnceLock::new());
        if stored.kids.get().is_some_and(Option::is_some) {
            let next: Kids = places
                .keys()
                .filter_map(|h| {
                    let k = new_kids.get(h).or_else(|| old.get(h))?.clone();
                    Some((*h, k))
                })
                .collect();
            let _ = kids.set(Some(next));
        }
        self.out.phases.push(("references kept", t0.elapsed()));
        Ok((Stored { places, kids }, std::mem::take(&mut self.out)))
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        self.discard();
    }
}

/// The packs root file `p` names (none if it is not a root).
fn root_packs(p: &Path) -> Vec<String> {
    use std::io::Read;
    let head = (|| {
        let mut f = File::open(p).ok()?;
        let mut h = [0; ROOT_MAGIC.len() + 4];
        f.read_exact(&mut h).ok()?;
        if h[..ROOT_MAGIC.len()] != ROOT_MAGIC[..] {
            return None;
        }
        let n = u32::from_le_bytes(h[ROOT_MAGIC.len()..].try_into().ok()?) as usize;
        let mut names = vec![0; n.checked_mul(16)?];
        f.read_exact(&mut names).ok()?;
        Some(names)
    })();
    head.unwrap_or_default()
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| hex(u128::from_le_bytes(*c)))
        .collect()
}

/// Keep the store under `max` bytes: the least recently used roots go
/// first, then every pack no root names (and is older than a minute: a
/// save running elsewhere may be about to name it).
pub fn collect(dir: &Path, max: u64) {
    let list = |d: &Path| -> Vec<(std::time::SystemTime, u64, PathBuf)> {
        std::fs::read_dir(d)
            .map(|es| {
                es.filter_map(Result::ok)
                    .filter(|e| !e.file_name().to_string_lossy().contains(".tmp"))
                    .filter_map(|e| {
                        let m = e.metadata().ok()?;
                        Some((m.modified().ok()?, m.len(), e.path()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut roots = list(&dir.join("roots"));
    let packs = list(&dir.join("packs"));
    // (the packs each root names, from its head)
    let heads: Vec<Vec<String>> = roots.iter().map(|(_, _, p)| root_packs(p)).collect();
    let size = |roots: &[(std::time::SystemTime, u64, PathBuf)], heads: &[Vec<String>]| {
        let named: BTreeSet<String> = heads.iter().flatten().cloned().collect();
        let bytes = roots.iter().map(|r| r.1).sum::<u64>()
            + packs
                .iter()
                .filter(|(_, _, p)| {
                    p.file_stem()
                        .is_some_and(|s| named.contains(&*s.to_string_lossy()))
                })
                .map(|p| p.1)
                .sum::<u64>();
        (bytes, named)
    };
    let mut order: Vec<usize> = (0..roots.len()).collect();
    order.sort_by(|&a, &b| roots[a].cmp(&roots[b]));
    let mut heads: Vec<Vec<String>> = order.iter().map(|&i| heads[i].clone()).collect();
    roots = order.iter().map(|&i| roots[i].clone()).collect();
    // (a saved build unused for the age bound goes whatever the size:
    // the documents not built for a month are not kept for ever)
    if let Some(age) = max_age() {
        let stale = std::time::SystemTime::now() - age;
        while roots.first().is_some_and(|r| r.0 < stale) {
            let (_, _, p) = roots.remove(0);
            heads.remove(0);
            let _ = std::fs::remove_file(p);
        }
    }
    let (mut bytes, mut named) = size(&roots, &heads);
    while bytes > max && !roots.is_empty() {
        let (_, _, p) = roots.remove(0);
        heads.remove(0);
        let _ = std::fs::remove_file(p);
        (bytes, named) = size(&roots, &heads);
    }
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    for (t, _, p) in packs {
        let unnamed = p
            .file_stem()
            .is_some_and(|s| !named.contains(&*s.to_string_lossy()));
        if unnamed && t < old {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use partex_core::persist::blob_hash;

    /// Write a root `key` for `root`, which refers to blobs `refs`: those of
    /// `new` (not stored yet, each with the blobs it refers to) and of
    /// `stored` ([`PackWriter`], given every new blob at once).
    fn save(
        dir: &Path,
        key: u128,
        root: &[u8],
        new: Vec<partex_core::persist::MerkleBlob>,
        refs: &[u128],
        stored: &Stored,
    ) -> std::io::Result<(Stored, Saved)> {
        let mut w = PackWriter::new(dir)?;
        for b in new {
            w.add(b);
        }
        w.finish(key, root, refs, stored)
    }

    fn blob(bytes: Vec<u8>, kids: Vec<u128>) -> partex_core::persist::MerkleBlob {
        (blob_hash(&bytes), bytes, kids)
    }

    /// A save writes its pack as it goes, in batches; what it saved reads
    /// back, and a second save that refers to the first's blobs keeps them
    /// (moving the live ones of a pack mostly dead).
    #[test]
    fn a_forgotten_root_is_gone_and_its_packs_with_it() {
        let dir = std::env::temp_dir().join(format!("phitex-store-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let own = blob(b"this job's only".repeat(20), Vec::new());
        let shared = blob(b"another job's".repeat(20), Vec::new());
        let (ho, hs) = (own.0, shared.0);
        save(&dir, 1, b"this job", vec![own], &[ho], &Stored::default()).unwrap();
        save(
            &dir,
            2,
            b"another job",
            vec![shared],
            &[hs],
            &Stored::default(),
        )
        .unwrap();
        let packs = || std::fs::read_dir(dir.join("packs")).unwrap().count();
        let before = packs();
        assert!(forget(&dir, 1));
        assert!(!forget(&dir, 1));
        assert!(open(&dir, 1).is_none());
        // (the other job's build is untouched)
        assert_eq!(
            open(&dir, 2).unwrap().get(hs).unwrap(),
            b"another job's".repeat(20)
        );
        // (the collection takes the packs no root names, once old enough)
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        for e in std::fs::read_dir(dir.join("packs")).unwrap() {
            let f = File::options()
                .append(true)
                .open(e.unwrap().path())
                .unwrap();
            f.set_modified(old).unwrap();
        }
        collect(&dir, u64::MAX);
        assert!(packs() < before);
        assert_eq!(
            open(&dir, 2).unwrap().get(hs).unwrap(),
            b"another job's".repeat(20)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A saved build unused for longer than the age bound goes at the next
    /// collection, however small the store; one used recently stays.
    #[test]
    fn an_old_saved_build_expires() {
        let dir = std::env::temp_dir().join(format!("phitex-store-age-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = blob(b"an old document".repeat(20), Vec::new());
        let b = blob(b"a recent document".repeat(20), Vec::new());
        let (ha, hb) = (a.0, b.0);
        save(&dir, 1, b"old", vec![a], &[ha], &Stored::default()).unwrap();
        save(&dir, 2, b"recent", vec![b], &[hb], &Stored::default()).unwrap();
        let days = max_age().expect("the default bound").as_secs() + 3600;
        let f = File::options()
            .append(true)
            .open(root_path(&dir, 1))
            .unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(days))
            .unwrap();
        collect(&dir, u64::MAX);
        assert!(open(&dir, 1).is_none());
        assert_eq!(
            open(&dir, 2).unwrap().get(hb).unwrap(),
            b"a recent document".repeat(20)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_read_back() {
        let dir = std::env::temp_dir().join(format!("partex-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // (small blobs and one past a batch, incompressible)
        let mut x = 1u64;
        let big: Vec<u8> = (0..SAVE_BATCH + 10)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (x >> 56) as u8
            })
            .collect();
        let leaf = blob(b"a leaf, referred to".repeat(20), Vec::new());
        let large = blob(big, vec![leaf.0]);
        let gone = blob(b"no root reaches this one".repeat(20), Vec::new());
        let (hl, hb, hg) = (leaf.0, large.0, gone.0);
        let (stored, saved) = save(
            &dir,
            1,
            b"root one",
            vec![leaf, large, gone],
            &[hb],
            &Stored::default(),
        )
        .unwrap();
        // (every new blob is written, as it comes: one the root does not
        // reach goes with its pack)
        assert_eq!(saved.new_blobs, 3);
        let o = open(&dir, 1).unwrap();
        assert_eq!(o.root, b"root one");
        assert_eq!(o.get(hl).unwrap(), b"a leaf, referred to".repeat(20));
        assert_eq!(o.get(hb).unwrap().len(), SAVE_BATCH + 10);
        // (no temporary file left behind)
        let names: Vec<String> = std::fs::read_dir(dir.join("packs"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
        // (what each blob refers to reads back, over several frames)
        let kids = read_all_kids(&dir, &stored.places).unwrap();
        assert_eq!(kids.len(), 3);
        assert_eq!(&kids[&hb][..], &[hl]);
        assert!(kids[&hl].is_empty() && kids[&hg].is_empty());
        // A second save refers to the leaf only: the large blob is dead,
        // so the first pack is mostly dead and the leaf moves.
        let extra = blob(b"a new blob".repeat(20), vec![hl]);
        let he = extra.0;
        let (_, saved) = save(&dir, 1, b"root two", vec![extra], &[he], &stored).unwrap();
        assert_eq!((saved.new_blobs, saved.packs), (1, 1));
        assert!(saved.moved_bytes > 0);
        let o = open(&dir, 1).unwrap();
        assert_eq!(o.root, b"root two");
        assert_eq!(o.get(hl).unwrap(), b"a leaf, referred to".repeat(20));
        assert_eq!(o.get(he).unwrap(), b"a new blob".repeat(20));
        assert!(o.get(hb).is_none());
        assert!(o.get(hg).is_none());
        // A writer dropped before its root leaves no file.
        let mut w = PackWriter::new(&dir).unwrap();
        w.add(blob(b"never saved".repeat(20), Vec::new()));
        drop(w);
        let names: Vec<String> = std::fs::read_dir(dir.join("packs"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.contains(".tmp")), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
