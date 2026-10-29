//! Shelf (shelf.PhiTeX.org): TeX Live's files in packs, for an engine
//! with no TeX installation (the frontends' wasm core, native `phitex`
//! with `files = "shelf"`; docs/spec-editor-protocol.md §5).
//!
//! - **The index** ([`Index`]): every file by its texmf path
//!   (`tex/latex/fontspec/fontspec.sty`), a row `path TAB pack TAB
//!   deps...`: its pack, then for each engine of `release.json`'s `deps`
//!   the packs its loading reads (comma-separated; hints only: a missing
//!   one costs a round trip, never a wrong file). Schema 4 adds the pack
//!   table (`pack TAB sha256 TAB bytes`, [`Index::with_pack_table`]): every
//!   pack is checked against it before use ([`Index::check`]), and one
//!   that differs is refused, never read. Schema 3 has none.
//! - **The rule** a name resolves by ([`Index::resolve`]), kpathsea's per
//!   engine (`release.json`'s `search`: per engine and kpathsea format,
//!   the ordered path prefixes, TeX Live's texmf.cnf):
//!   - a name with a `/` is a path: TeX Live's absolute prefix ([`TL`])
//!     dropped, the rest looked up as is;
//!   - else its basename's paths: the first prefix of the engine's list
//!     for the format that holds any wins (at any depth under it), and
//!     within one prefix the lexicographically smallest path; none under
//!     any prefix: not found (kpathsea would not find it either);
//!   - a format the release has no list for (or an engine it does not
//!     name): the smallest path of all with that basename.
//! - **Packs** ([`unpack`]): gzip of `u32 n, (u32 len, path, u32 len,
//!   bytes) × n`, little-endian.
//! - **The files a build reads** ([`Files`]): a name resolved by the
//!   index, its pack and the packs its loading reads asked of the host's
//!   [`PackSource`] (once each, by whichever thread needs one first; the
//!   others wait), every file of each kept by path; Shelf's files named
//!   `/usr/share/texmf-dist/<path>` to the engine. A name the index lacks
//!   is absent at once and never leaves the machine. Each pack asked, in,
//!   or failed is told ([`Files::on_fetch`]: the session's progress).
//!
//! One source per run: a build's TeX files are Shelf's (and the project's
//! own), or TeX Live's, never one after the other.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use serde_json::Value;

/// TeX Live's tree, as a native build names the files it reads: Shelf's
/// files are named under it.
pub const TL: &str = "/usr/share/texmf-dist/";

/// The schemas this crate reads.
pub const SCHEMAS: [u64; 2] = [3, 4];

/// A pack's SHA-256 and size, as the pack table has them.
type Sum = ([u8; 32], u64);

struct Row {
    pack: Box<str>,
    /// Per engine of `Index::engines`, the packs its loading reads.
    deps: Box<[Box<str>]>,
}

/// Shelf's index of one release.
pub struct Index {
    rows: HashMap<Box<str>, Row>,
    /// Basename to its paths, sorted.
    by_base: HashMap<Box<str>, Vec<Box<str>>>,
    /// Engine to kpathsea format to path prefixes, in search order.
    search: HashMap<String, HashMap<String, Vec<String>>>,
    engines: Vec<String>,
    /// The pack table (schema 4): each pack's SHA-256 and size.
    table: Option<HashMap<Box<str>, Sum>>,
    release: String,
}

/// Shelf's engine name for a program (`pdflatex`, `xelatex`, ...).
#[must_use]
pub fn engine(e: &str) -> &'static str {
    match e {
        "xelatex" | "xetex" => "xetex",
        "lualatex" | "luatex" => "luatex",
        _ => "pdftex",
    }
}

/// The kpathsea format of a file by its name's extension (the engine asks
/// by name only).
#[must_use]
pub fn format_of(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map_or("", |(_, e)| e)
        .to_ascii_lowercase();
    match ext.as_str() {
        "tfm" => "tfm",
        "vf" => "vf",
        "pfb" | "pfa" => "type1",
        "enc" => "enc",
        "map" => "map",
        "otf" => "opentype",
        "ttf" | "ttc" => "truetype",
        "bst" | "csf" => "bst",
        "bib" => "bib",
        "ist" => "ist",
        "tec" => "misc",
        _ => "tex",
    }
}

/// `s`'s hex digits as bytes (none: not 64 of them).
fn hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (k, o) in out.iter_mut().enumerate() {
        let d = |c: u8| char::from(c).to_digit(16);
        *o = u8::try_from(d(b[2 * k])? * 16 + d(b[2 * k + 1])?).ok()?;
    }
    Some(out)
}

impl Index {
    /// The index from its TSV (gunzipped) and the release's
    /// `release.json`.
    #[must_use]
    pub fn new(tsv: &str, release: &Value) -> Index {
        let mut rows = HashMap::new();
        let mut by_base: HashMap<Box<str>, Vec<Box<str>>> = HashMap::new();
        for line in tsv.lines() {
            let mut cols = line.split('\t');
            let (Some(path), Some(pack)) = (cols.next(), cols.next()) else {
                continue;
            };
            // (a row has a path and a pack)
            if path.is_empty() || pack.is_empty() {
                continue;
            }
            let base = path.rsplit_once('/').map_or(path, |(_, b)| b);
            by_base.entry(base.into()).or_default().push(path.into());
            rows.insert(
                path.into(),
                Row {
                    pack: pack.into(),
                    deps: cols.map(Into::into).collect(),
                },
            );
        }
        for paths in by_base.values_mut() {
            paths.sort_unstable();
        }
        let search = serde_json::from_value(release["search"].clone()).unwrap_or_default();
        let engines = serde_json::from_value(release["deps"].clone()).unwrap_or_default();
        Index {
            rows,
            by_base,
            search,
            engines,
            table: None,
            release: release["release"].as_str().unwrap_or("").to_owned(),
        }
    }

    /// The index with schema 4's pack table (its TSV, gunzipped: `pack
    /// TAB sha256 TAB bytes`): every pack checked against it.
    ///
    /// # Errors
    /// A row that is not one.
    pub fn with_pack_table(mut self, tsv: &str) -> Result<Index, String> {
        let mut t = HashMap::new();
        for (k, line) in tsv.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
            let mut cols = line.split('\t');
            let row = (|| {
                let pack = cols.next()?;
                let sum = hex32(cols.next()?)?;
                let bytes = cols.next()?.parse::<u64>().ok()?;
                Some((pack, sum, bytes))
            })();
            let Some((pack, sum, bytes)) = row else {
                return Err(format!("the pack table's line {}: {line:?}", k + 1));
            };
            t.insert(pack.into(), (sum, bytes));
        }
        self.table = Some(t);
        Ok(self)
    }

    /// The release (`tl2026.6`).
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// `name`'s texmf path for `engine` (Shelf's name) and kpathsea format
    /// `format`, or none.
    #[must_use]
    pub fn resolve(&self, name: &str, format: &str, engine: &str) -> Option<&str> {
        if name.contains('/') {
            let p = name
                .strip_prefix(TL)
                .unwrap_or_else(|| name.strip_prefix("./").unwrap_or(name));
            return self.rows.get_key_value(p).map(|(k, _)| &**k);
        }
        let paths = self.by_base.get(name)?;
        let Some(prefixes) = self.search.get(engine).and_then(|f| f.get(format)) else {
            return paths.first().map(|p| &**p);
        };
        prefixes.iter().find_map(|pre| {
            let pre = pre.trim_end_matches('/');
            paths
                .iter()
                .find(|p| p.strip_prefix(pre).is_some_and(|r| r.starts_with('/')))
                .map(|p| &**p)
        })
    }

    /// The pack that holds `path` (its id names its content).
    #[must_use]
    pub fn pack_of(&self, path: &str) -> Option<&str> {
        self.rows.get(path).map(|r| &*r.pack)
    }

    /// The packs to fetch for `path` with `engine`: its own, then what its
    /// loading reads.
    #[must_use]
    pub fn packs(&self, path: &str, engine: &str) -> Vec<&str> {
        let Some(row) = self.rows.get(path) else {
            return Vec::new();
        };
        let deps = self
            .engines
            .iter()
            .position(|e| e == engine)
            .and_then(|k| row.deps.get(k))
            .map_or("", |d| &**d);
        std::iter::once(&*row.pack)
            .chain(deps.split(',').filter(|d| !d.is_empty()))
            .collect()
    }

    /// Whether pack `id`'s bytes as served (`raw`) are the pack table's
    /// (schema 3: no table, every pack taken).
    ///
    /// # Errors
    /// Why not: another size, another digest, a pack the table lacks.
    pub fn check(&self, id: &str, raw: &[u8]) -> Result<(), String> {
        let Some(t) = &self.table else {
            return Ok(());
        };
        let Some((sum, bytes)) = t.get(id) else {
            return Err(format!("pack {id} is not in the release's pack table"));
        };
        if raw.len() as u64 != *bytes {
            return Err(format!(
                "pack {id} has {} bytes, the pack table {bytes}",
                raw.len()
            ));
        }
        if phitex_codecs::sha256::sha256(raw) != *sum {
            return Err(format!("pack {id}'s SHA-256 is not the pack table's"));
        }
        Ok(())
    }
}

/// `b` gunzipped (its first member; RFC 1952), or none: not gzip, or cut.
#[must_use]
pub fn gunzip(b: &[u8]) -> Option<Vec<u8>> {
    if b.get(..3)? != [0x1f, 0x8b, 8] {
        return None;
    }
    let flags = *b.get(3)?;
    let mut at = 10;
    // (FEXTRA, FNAME, FCOMMENT, FHCRC)
    if flags & 4 != 0 {
        at += 2 + usize::from(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]));
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            at += b.get(at..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let mut out = Vec::with_capacity(b.len() * 4);
    phitex_codecs::inflate::inflate_raw_len(b.get(at..)?, &mut out)?;
    Some(out)
}

/// A pack's files from its container's bytes (gunzipped): none if it is
/// not one.
#[must_use]
pub fn unpack(b: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let u32_at = |at: usize| -> Option<usize> {
        let w: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(w)).ok()
    };
    let n = u32_at(0)?;
    let mut at = 4;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let k = u32_at(at)?;
        let name = String::from_utf8(b.get(at + 4..at + 4 + k)?.to_vec()).ok()?;
        at += 4 + k;
        let l = u32_at(at)?;
        out.push((name, b.get(at + 4..at + 4 + l)?.to_vec()));
        at += 4 + l;
    }
    Some(out)
}

/// Where packs come from: the host's (native: HTTPS and a cache on disk;
/// wasm: the JS host's fetch).
pub trait PackSource: Send + Sync {
    /// Pack `id`'s bytes as Shelf serves them (gzip of the container).
    ///
    /// # Errors
    /// Why it could not be had.
    fn pack(&self, id: &str) -> Result<Vec<u8>, String>;

    /// Packs `ids`, each's bytes or why not, in their order: by default
    /// one after the other; a source that can fetch them at once does.
    fn packs(&self, ids: &[&str]) -> Vec<Result<Vec<u8>, String>> {
        ids.iter().map(|id| self.pack(id)).collect()
    }
}

/// What [`Files`] tells of a pack: asked for `name`, in, or failed (why).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch<'a> {
    Asked {
        name: &'a str,
        pack: &'a str,
    },
    Arrived {
        name: &'a str,
        pack: &'a str,
    },
    Failed {
        name: &'a str,
        pack: &'a str,
        why: &'a str,
    },
}

enum Pack {
    /// Asked of the source; not in yet.
    Asked,
    /// In (its files kept), or failed (told).
    Done,
}

#[derive(Default)]
struct State {
    /// Shelf's files by texmf path, from the packs in.
    by_path: HashMap<String, Arc<Vec<u8>>>,
    packs: HashMap<String, Pack>,
}

type Hook = Box<dyn Fn(&Fetch<'_>) + Send + Sync>;

/// The files a build reads from Shelf, for one engine.
pub struct Files {
    index: Arc<Index>,
    source: Arc<dyn PackSource>,
    engine: &'static str,
    state: Mutex<State>,
    answered: Condvar,
    hook: Mutex<Option<Hook>>,
}

impl Files {
    /// The files of `index`'s release for `engine` (Shelf's name:
    /// [`engine`]), their packs `source`'s.
    #[must_use]
    pub fn new(index: Arc<Index>, source: Arc<dyn PackSource>, engine: &'static str) -> Files {
        Files {
            index,
            source,
            engine,
            state: Mutex::default(),
            answered: Condvar::new(),
            hook: Mutex::new(None),
        }
    }

    /// The index.
    #[must_use]
    pub fn index(&self) -> &Arc<Index> {
        &self.index
    }

    /// Each pack asked, in, or failed told to `hook` (on the thread that
    /// fetches it).
    pub fn on_fetch(&self, hook: Hook) {
        *lock(&self.hook) = Some(hook);
    }

    fn tell(&self, f: &Fetch<'_>) {
        if let Some(h) = lock(&self.hook).as_ref() {
            h(f);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// `name`'s texmf path (none: not Shelf's).
    #[must_use]
    pub fn path_of(&self, name: &str) -> Option<&str> {
        self.path_in(name, format_of(name))
    }

    /// `name`'s texmf path in kpathsea format `format` (a caller that
    /// knows it: dvipdfmx's `cmap`, `sfd`, `other text files`, ...).
    #[must_use]
    pub fn path_in(&self, name: &str, format: &str) -> Option<&str> {
        self.index.resolve(name, format, self.engine)
    }

    /// Where file `name` is (`/usr/share/texmf-dist/<path>`), its pack
    /// and the packs its loading reads fetched (once: waiting for another
    /// thread's fetch of the same); none: the index lacks it, or its pack
    /// could not be had.
    pub fn locate(&self, name: &str) -> Option<String> {
        self.find(name).map(|(p, _)| p)
    }

    /// [`Files::locate`] and the file's bytes.
    pub fn find(&self, name: &str) -> Option<(String, Arc<Vec<u8>>)> {
        self.find_in(name, format_of(name))
    }

    /// [`Files::find`] in kpathsea format `format`.
    pub fn find_in(&self, name: &str, format: &str) -> Option<(String, Arc<Vec<u8>>)> {
        let rel = self.path_in(name, format)?.to_owned();
        let ids: Vec<String> = self
            .index
            .packs(&rel, self.engine)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let own = ids.first()?.clone();
        // (the packs nobody asked for yet asked here, at once)
        let mine: Vec<&str> = {
            let mut s = self.lock();
            ids.iter()
                .filter(|id| {
                    let new = !s.packs.contains_key(id.as_str());
                    if new {
                        s.packs.insert((*id).clone(), Pack::Asked);
                    }
                    new
                })
                .map(String::as_str)
                .collect()
        };
        if !mine.is_empty() {
            for id in &mine {
                self.tell(&Fetch::Asked { name, pack: id });
            }
            let got = self.source.packs(&mine);
            for (id, raw) in mine.iter().zip(got) {
                let files = raw.and_then(|raw| {
                    self.index.check(id, &raw)?;
                    gunzip(&raw)
                        .and_then(|b| unpack(&b))
                        .ok_or_else(|| format!("pack {id} is not a pack"))
                });
                let done: Result<(), String> = files.as_ref().map(|_| ()).map_err(Clone::clone);
                {
                    let mut s = self.lock();
                    if let Ok(fs) = files {
                        for (path, bytes) in fs {
                            s.by_path.entry(path).or_insert_with(|| Arc::new(bytes));
                        }
                    }
                    s.packs.insert((*id).to_owned(), Pack::Done);
                }
                self.answered.notify_all();
                match &done {
                    Ok(()) => self.tell(&Fetch::Arrived { name, pack: id }),
                    Err(why) => self.tell(&Fetch::Failed {
                        name,
                        pack: id,
                        why,
                    }),
                }
            }
        }
        let mut s = self.lock();
        loop {
            if let Some(b) = s.by_path.get(&rel) {
                return Some((format!("{TL}{rel}"), b.clone()));
            }
            if matches!(s.packs.get(&own), Some(Pack::Done) | None) {
                return None;
            }
            s = self
                .answered
                .wait(s)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The bytes of the file at `path`, as [`Files::locate`] named it
    /// (none: not one of Shelf's fetched).
    #[must_use]
    pub fn read(&self, path: &str) -> Option<Arc<Vec<u8>>> {
        let rel = path.strip_prefix(TL)?;
        self.lock().by_path.get(rel).cloned()
    }

    /// The path and content key of the file `name` would be, from the
    /// index alone, nothing fetched (a warm start checks the format
    /// against them): its key the id of its pack (which names the pack's
    /// content) and its texmf path.
    #[must_use]
    pub fn key(&self, name: &str) -> Option<(String, Vec<u8>)> {
        let rel = self.path_of(name)?;
        let pack = self.index.pack_of(rel)?;
        Some((
            format!("{TL}{rel}"),
            [pack.as_bytes(), b"\0", rel.as_bytes()].concat(),
        ))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A pack's container from its files (as Shelf's build makes them), and
/// gzip'd as served (stored blocks): for tests and tools that make a
/// release.
#[must_use]
pub fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&u32::try_from(files.len()).unwrap_or(0).to_le_bytes());
    for (name, bytes) in files {
        b.extend_from_slice(&u32::try_from(name.len()).unwrap_or(0).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(0).to_le_bytes());
        b.extend_from_slice(bytes);
    }
    gzip_stored(&b)
}

/// `b` as a gzip member of stored deflate blocks (no compression).
#[must_use]
pub fn gzip_stored(b: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let mut chunks = b.chunks(65_535).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        out.push(u8::from(chunks.peek().is_none()));
        let n = u16::try_from(c.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&crc32(b).to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(b.len() & 0xffff_ffff)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    out
}

/// CRC-32 (IEEE), gzip's.
fn crc32(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &x in b {
        c ^= u32::from(x);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// The SHA-256 of `b` in hex, as the pack table has it.
#[must_use]
pub fn sha256_hex(b: &[u8]) -> String {
    phitex_codecs::sha256::sha256(b)
        .iter()
        .fold(String::new(), |mut s, x| {
            use std::fmt::Write as _;
            let _ = write!(s, "{x:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn release() -> Value {
        serde_json::json!({
            "release": "tl-test",
            "deps": ["pdftex", "xetex"],
            "search": {
                "pdftex": {"tex": ["tex/latex", "tex/generic", "tex"], "tfm": ["fonts/tfm"]},
                "xetex": {"tex": ["tex/xelatex", "tex/latex", "tex"]}
            }
        })
    }

    fn index() -> Index {
        let tsv = "tex/latex/base/article.cls\tbase-1\t\t\n\
                   tex/xelatex/xunicode/xunicode.sty\txu-1\t\t\n\
                   tex/latex/xunicode/xunicode.sty\txu-2\t\t\n\
                   tex/latex/amsmath/amsmath.sty\tamsmath-1\tamsfonts-1,base-1\t\n\
                   fonts/tfm/public/cm/cmr10.tfm\tcm-1\t\t\n";
        Index::new(tsv, &release())
    }

    #[test]
    fn resolves_by_the_engines_search_order() {
        let ix = index();
        assert_eq!(
            ix.resolve("xunicode.sty", "tex", "xetex"),
            Some("tex/xelatex/xunicode/xunicode.sty")
        );
        assert_eq!(
            ix.resolve("xunicode.sty", "tex", "pdftex"),
            Some("tex/latex/xunicode/xunicode.sty")
        );
        assert_eq!(
            ix.resolve("cmr10.tfm", "tfm", "pdftex"),
            Some("fonts/tfm/public/cm/cmr10.tfm")
        );
        // (no list for the format: the smallest path)
        assert_eq!(
            ix.resolve("cmr10.tfm", "tfm", "xetex"),
            Some("fonts/tfm/public/cm/cmr10.tfm")
        );
        // (a path, TeX Live's prefix dropped)
        assert_eq!(
            ix.resolve(
                "/usr/share/texmf-dist/tex/latex/base/article.cls",
                "tex",
                "pdftex"
            ),
            Some("tex/latex/base/article.cls")
        );
        assert_eq!(ix.resolve("nowhere.sty", "tex", "pdftex"), None);
        // (a tfm under a tex prefix is not found by kpathsea's tfm search)
        assert_eq!(ix.resolve("article.cls", "tfm", "pdftex"), None);
        assert_eq!(ix.release(), "tl-test");
    }

    #[test]
    fn packs_are_its_own_then_its_deps() {
        let ix = index();
        assert_eq!(
            ix.packs("tex/latex/amsmath/amsmath.sty", "pdftex"),
            ["amsmath-1", "amsfonts-1", "base-1"]
        );
        assert_eq!(
            ix.packs("tex/latex/amsmath/amsmath.sty", "xetex"),
            ["amsmath-1"]
        );
        assert_eq!(ix.packs("tex/nothing.sty", "pdftex"), Vec::<&str>::new());
    }

    #[test]
    fn format_by_extension() {
        assert_eq!(format_of("cmr10.TFM"), "tfm");
        assert_eq!(format_of("article.cls"), "tex");
        assert_eq!(format_of("plain.bst"), "bst");
    }

    #[test]
    fn packs_round_trip_and_are_checked() {
        let big = vec![0u8; 70_000];
        let raw = pack(&[("tex/a.sty", b"A"), ("tex/b.sty", &big)]);
        let files = unpack(&gunzip(&raw).unwrap()).unwrap();
        assert_eq!(files[0], ("tex/a.sty".to_owned(), b"A".to_vec()));
        assert_eq!(files[1].1.len(), 70_000);
        let table = format!("p-1\t{}\t{}\n", sha256_hex(&raw), raw.len());
        let ix = index().with_pack_table(&table).unwrap();
        assert_eq!(ix.check("p-1", &raw), Ok(()));
        let mut bad = raw.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(ix.check("p-1", &bad).unwrap_err().contains("SHA-256"));
        assert!(ix.check("p-1", &raw[1..]).unwrap_err().contains("bytes"));
        assert!(ix.check("p-2", &raw).is_err());
        assert!(index().with_pack_table("p\tnot-hex\t1\n").is_err());
        // (schema 3: no table, taken)
        assert_eq!(index().check("p-2", &raw), Ok(()));
        assert_eq!(gunzip(b"not gzip"), None);
    }

    /// Packs from memory, counting what was asked.
    struct Memory {
        packs: HashMap<String, Vec<u8>>,
        asked: AtomicUsize,
    }

    impl PackSource for Memory {
        fn pack(&self, id: &str) -> Result<Vec<u8>, String> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.packs
                .get(id)
                .cloned()
                .ok_or_else(|| format!("no pack {id}"))
        }
    }

    /// A name found through its pack, the packs its loading reads fetched
    /// with it (each once, threads that need one waiting for it), every
    /// fetch told; a name the index lacks absent at once; a pack that
    /// differs from the table refused.
    #[test]
    fn files_fetch_each_pack_once() {
        let am = pack(&[("tex/latex/amsmath/amsmath.sty", b"\\amsmath")]);
        let fonts = pack(&[("tex/latex/amsfonts/amsfonts.sty", b"\\fonts")]);
        let base = pack(&[("tex/latex/base/article.cls", b"\\article")]);
        let table = [
            ("amsmath-1", &am),
            ("amsfonts-1", &fonts),
            ("base-1", &base),
        ]
        .iter()
        .fold(String::new(), |mut t, (id, b)| {
            use std::fmt::Write as _;
            let _ = writeln!(t, "{id}\t{}\t{}", sha256_hex(b), b.len());
            t
        });
        let tsv = "tex/latex/amsmath/amsmath.sty\tamsmath-1\tamsfonts-1,base-1\t\n\
                   tex/latex/amsfonts/amsfonts.sty\tamsfonts-1\t\t\n\
                   tex/latex/base/article.cls\tbase-1\t\t\n\
                   fonts/tfm/public/cm/cmr10.tfm\tcm-1\t\t\n";
        let ix = Arc::new(Index::new(tsv, &release()).with_pack_table(&table).unwrap());
        let src = Arc::new(Memory {
            packs: [
                ("amsmath-1".to_owned(), am),
                ("amsfonts-1".to_owned(), fonts),
                ("base-1".to_owned(), base),
                // (not the table's: no row for it)
                (
                    "cm-1".to_owned(),
                    pack(&[("fonts/tfm/public/cm/cmr10.tfm", b"x")]),
                ),
            ]
            .into(),
            asked: AtomicUsize::new(0),
        });
        let files = Arc::new(Files::new(ix, src.clone(), "pdftex"));
        let told: Arc<Mutex<Vec<String>>> = Arc::default();
        let t = told.clone();
        files.on_fetch(Box::new(move |f| {
            lock(&t).push(match f {
                Fetch::Asked { pack, .. } => format!("asked {pack}"),
                Fetch::Arrived { pack, .. } => format!("in {pack}"),
                Fetch::Failed { pack, .. } => format!("failed {pack}"),
            });
        }));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let f = files.clone();
                std::thread::spawn(move || f.find("amsmath.sty"))
            })
            .collect();
        for th in threads {
            let (p, b) = th.join().unwrap().expect("found");
            assert_eq!(p, "/usr/share/texmf-dist/tex/latex/amsmath/amsmath.sty");
            assert_eq!(&**b, b"\\amsmath");
        }
        assert_eq!(src.asked.load(Ordering::Relaxed), 3, "each pack once");
        // (a dependency's file: in already, nothing asked)
        assert_eq!(
            files.locate("article.cls").as_deref(),
            Some("/usr/share/texmf-dist/tex/latex/base/article.cls")
        );
        assert_eq!(src.asked.load(Ordering::Relaxed), 3);
        assert_eq!(
            files
                .read("/usr/share/texmf-dist/tex/latex/amsfonts/amsfonts.sty")
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"\\fonts"[..])
        );
        assert_eq!(files.locate("nowhere.sty"), None);
        assert_eq!(
            src.asked.load(Ordering::Relaxed),
            3,
            "never left the machine"
        );
        assert_eq!(files.locate("cmr10.tfm"), None, "a pack not the table's");
        let told = lock(&told).clone();
        assert_eq!(
            told.iter().filter(|t| t.starts_with("asked")).count(),
            4,
            "{told:?}"
        );
        assert!(told.contains(&"in amsfonts-1".to_owned()), "{told:?}");
        assert!(told.contains(&"failed cm-1".to_owned()), "{told:?}");
        let (k, key) = files.key("article.cls").unwrap();
        assert_eq!(k, "/usr/share/texmf-dist/tex/latex/base/article.cls");
        assert_eq!(key, b"base-1\0tex/latex/base/article.cls");
    }
}
