//! The content-addressed store (DESIGN.md §7.9): blobs named by the hash
//! of their bytes, packed, and roots that name what a saved build needs.
//!
//! Layout under the store directory (`PARTEX_STORE_DIR`, else `store`
//! in `partex.toml`, else `$XDG_CACHE_HOME/partex/store`, else
//! `~/.cache/partex/store`; `PARTEX_STORE=0` turns it off):
//!
//! - `packs/<hash>.pack`: blobs one after the other; `<hash>` is that of
//!   the pack's bytes, so a pack never changes once written. Each blob is
//!   a byte saying how it is kept, then its bytes: 0 as they are, 1
//!   compressed (`lz.rs`; `PARTEX_STORE_COMPRESS=0` writes all as they
//!   are). A blob's name is the hash of its bytes, not of what is kept.
//! - `packs/<hash>.idx`: the pack's index, a blob hash, offset and length
//!   per blob (16 + 8 + 4 bytes each).
//! - `packs/<hash>.kids`: the blobs each of the pack's blobs refers to,
//!   in the index's order (a count, then their hashes; kept as a blob
//!   is, compressed or not): what a blob keeps
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
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// The first bytes of a root.
const ROOT_MAGIC: &[u8; 16] = b"partex-store/5\0\0";

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
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("partex/store"))
        })
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/partex/store")))
}

fn max_bytes() -> u64 {
    std::env::var("PARTEX_STORE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4 << 30)
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
    let b = unkeep(std::fs::read(kids_path(dir, name)).ok()?)?;
    let mut at = 0;
    let mut out = Vec::with_capacity(idx.len());
    for (h, _, _) in idx {
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
        out.push((h, kids));
    }
    (at == b.len()).then_some(out)
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

/// Write a root `key` for `root`, which refers to blobs `refs`: those of
/// `new` (not stored yet, each with the blobs it refers to) and of
/// `stored`. What the root reaches is kept; the updated [`Stored`] (the
/// packs the root now names).
#[allow(clippy::too_many_lines)] // (one pass over the packs)
pub fn save(
    dir: &Path,
    key: u128,
    root: &[u8],
    new: &[partex_core::persist::MerkleBlob],
    refs: &[u128],
    stored: &Stored,
) -> std::io::Result<(Stored, Saved)> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(dir.join("packs"))?;
    std::fs::create_dir_all(dir.join("roots"))?;
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
    let new_kids: HashMap<u128, &[u128], ByName> =
        new.iter().map(|(h, _, k)| (*h, k.as_slice())).collect();
    let kids_of = |h: &u128| -> Option<&[u128]> {
        new_kids
            .get(h)
            .copied()
            .or_else(|| old.get(h).map(|k| &k[..]))
    };
    let t_kids = t0.elapsed();
    // (what the root reaches)
    let mut live: std::collections::HashSet<u128, ByName> =
        std::collections::HashSet::with_capacity_and_hasher(
            stored.places.len() + new.len(),
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
    let live = &live;
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
    let mut pack: Vec<u8> = Vec::new();
    let mut index: Vec<(u128, u64, u32)> = Vec::new();
    let mut out = Saved {
        live: live.len(),
        phases: vec![("references", t_kids), ("liveness", t0.elapsed())],
        ..Saved::default()
    };
    let mut put = |h: u128, b: &[u8], pack: &mut Vec<u8>| {
        index.push((
            h,
            pack.len() as u64,
            u32::try_from(b.len()).unwrap_or(u32::MAX),
        ));
        pack.extend_from_slice(b);
    };
    // (the new blobs kept, on several threads when there are many)
    let fresh: Vec<&partex_core::persist::MerkleBlob> =
        new.iter().filter(|(h, _, _)| live.contains(h)).collect();
    let compress = compressing();
    let total_new: usize = fresh.iter().map(|(_, b, _)| b.len()).sum();
    let threads = if total_new < (8 << 20) {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(4, std::num::NonZero::get)
            .min(8)
    };
    let per = fresh.len().div_ceil(threads).max(1);
    let kept: Vec<Vec<(u128, Vec<u8>)>> = std::thread::scope(|sc| {
        let jobs: Vec<_> = fresh
            .chunks(per)
            .map(|part| {
                sc.spawn(move || {
                    part.iter()
                        .map(|(h, b, _)| (*h, kept_form(b, compress)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| j.join().unwrap_or_default())
            .collect()
    });
    for (h, k) in kept.iter().flatten() {
        put(*h, k, &mut pack);
        out.new_blobs += 1;
        out.new_bytes += k.len() as u64;
    }
    out.new_raw_bytes = total_new as u64;
    if kept.iter().map(Vec::len).sum::<usize>() != fresh.len() {
        return Err(std::io::Error::other("a blob was not kept"));
    }
    out.phases.push(("compress", t0.elapsed()));
    // (the live blobs of the packs let go, read back)
    let mut files: BTreeMap<u128, File> = BTreeMap::new();
    let mut moving: Vec<(u128, (u128, u64, u32))> = live
        .iter()
        .filter_map(|h| Some((*h, *stored.places.get(h)?)))
        .filter(|(_, (p, _, _))| !keep.contains(p))
        .collect();
    moving.sort_unstable();
    for (h, (p, off, len)) in moving {
        let h = &h;
        let f = match files.entry(p) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(File::open(pack_path(dir, p))?)
            }
        };
        let mut buf = vec![0; len as usize];
        f.read_exact_at(&mut buf, off)?;
        put(*h, &buf, &mut pack);
        out.moved_bytes += u64::from(len);
    }
    let mut places: HashMap<u128, (u128, u64, u32)> = stored
        .places
        .iter()
        .filter(|(_, (p, _, _))| keep.contains(p))
        .map(|(h, v)| (*h, *v))
        .collect();
    let mut names: Vec<u128> = keep.iter().copied().collect();
    if !pack.is_empty() {
        let name = partex_core::persist::blob_hash(&pack);
        let mut idx = Vec::with_capacity(index.len() * ENTRY);
        let mut kid_bytes = Vec::new();
        for (h, off, len) in &index {
            idx.extend_from_slice(&h.to_le_bytes());
            idx.extend_from_slice(&off.to_le_bytes());
            idx.extend_from_slice(&len.to_le_bytes());
            let k = kids_of(h).unwrap_or(&[]);
            kid_bytes.extend_from_slice(&u32::try_from(k.len()).unwrap_or(0).to_le_bytes());
            for x in k {
                kid_bytes.extend_from_slice(&x.to_le_bytes());
            }
        }
        out.phases.push(("move, index", t0.elapsed()));
        // (the pack and its references before its index: an index names
        // only packs there)
        write_atomic(&pack_path(dir, name), &pack)?;
        write_atomic(&kids_path(dir, name), &kept_form(&kid_bytes, compress))?;
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
    out.root_bytes = r.len();
    out.packs = names.len();
    out.phases.push(("write", t0.elapsed()));
    collect(dir, max_bytes());
    out.phases.push(("collect", t0.elapsed()));
    // (what the blobs kept refer to, for the next save)
    let kids = Arc::new(OnceLock::new());
    if stored.kids.get().is_some_and(Option::is_some) {
        let next: Kids = places
            .keys()
            .filter_map(|h| {
                let k = match new_kids.get(h) {
                    Some(k) => Arc::from(*k),
                    None => old.get(h)?.clone(),
                };
                Some((*h, k))
            })
            .collect();
        let _ = kids.set(Some(next));
    }
    out.phases.push(("references kept", t0.elapsed()));
    Ok((Stored { places, kids }, out))
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
