//! TeX's files from Shelf (`files = "shelf"` in `phitex.toml`, or
//! `--shelf`; docs/spec-editor-protocol.md §5): `phitex-shelf`'s index
//! and files, their packs fetched over HTTP(S) (`ureq`, `rustls`) and
//! kept in the package cache, the cache directory's `shelf/<release>/`:
//! `release.json`, the index and the pack table (fetched once a release,
//! then read from there: offline), and `h/<pack>.pack`, every pack
//! fetched, kept across runs and documents. Clearing it is safe: what a
//! build needs is fetched again.
//!
//! - **One source a run.** A build's TeX files are the project's own
//!   (its directory, first, as kpathsea's `.`) and Shelf's, never TeX
//!   Live's ([`ShelfHost`]); with `files = "texlive"` (the default),
//!   kpathsea's, never Shelf's. No fallback from one to the other.
//! - **The release** is this PhiTeX's ([`RELEASE`]): two machines with
//!   the same PhiTeX build a document from the same files.
//! - **Where from**: [`HOSTS`], Shelf's public host for the protocol
//!   this PhiTeX reads (`phitex_shelf::SCHEMAS`) by its domain, then by
//!   Cloudflare's name for it where a network refuses the domain, or
//!   `PHITEX_SHELF_URL`
//!   (the release's directory, as `https://HOST/tl2026/`: a mirror, a
//!   release being made). Needed only while the index or a pack is not in
//!   the cache.
//! - **Checked**: every pack against the release's pack table before it
//!   is used or kept (`phitex_shelf::Index::check`).
//! - **XeTeX's fonts by name** ([`ShelfHost::xetex`]): the release's
//!   font index ([`FONT_INDEX`], `phitex-otf`'s PXFI, made where the
//!   release is built, with fontconfig, whose list order XeTeX's name
//!   matching follows: Shelf's `build.py` `FONT_INDEX`), its paths
//!   relative to the tree named under TeX Live's (`/usr/share/texmf-dist/
//!   fonts/...`, as xelatex's XDV names them), its faces read from Shelf's
//!   packs as a name finds them. Fetched and kept with the release's
//!   packs: one a release, no fontconfig here.
//! - **Told**: each pack asked and in, the session's progress records
//!   (`Meter::fetch`); a pack that could not be had (unreachable,
//!   refused), the terminal's warning and the editor's message
//!   (`Meter::unreachable`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use phitex_live::host::Host;
use phitex_live::session::Job;
use phitex_shelf::{Fetch, Files, Index, PackSource};
use phitex_tex::{Finder, ToolFinder};

use crate::cache::{Cache, SHELF, Stats};

/// Where a release keeps its font index (Shelf's `build.py`
/// `FONT_INDEX_PATH`, a pack of its own).
pub const FONT_INDEX: &str = "fonts/misc/partex/fontindex.pxfi";

/// The Shelf release this PhiTeX builds from (its index and packs): two
/// machines with the same PhiTeX build a document from the same files.
pub const RELEASE: &str = "tl2026.9";

/// The release built from: [`RELEASE`], or `PHITEX_SHELF_RELEASE` (a
/// release being made, checked before it is this binary's).
pub fn release() -> String {
    std::env::var("PHITEX_SHELF_RELEASE")
        .ok()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| RELEASE.to_owned())
}

/// The packs read from the cache this process, and those fetched.
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// The package cache's directory: `<cache>/shelf/<release>/`.
pub fn dir() -> PathBuf {
    crate::tex::cache_dir().join(SHELF).join(release())
}

/// Where the package cache is, what it holds, and its packs found and
/// fetched this process.
pub fn stats() -> (PathBuf, Stats, u64, u64) {
    let d = dir();
    let st = Cache {
        dir: d.clone(),
        limit: 1,
    }
    .stats();
    (
        d,
        st,
        HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
    )
}

/// The package cache emptied (every release's): the bytes freed.
pub fn clear() -> Result<u64, String> {
    let d = crate::tex::cache_dir().join(SHELF);
    let held = Cache {
        dir: d.clone(),
        limit: 1,
    }
    .stats()
    .bytes;
    match std::fs::remove_dir_all(&d) {
        Ok(()) => Ok(held),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("the package cache: {}: {e}", d.display())),
    }
}

/// The release's directory on Shelf's public host (docs/spec-distribution.md
/// §2), the host naming the protocol, `v1` for schema 1: by its domain,
/// then the same Pages project by Cloudflare's name, for networks that
/// refuse phitex.org's names (some universities' filters reset the TLS
/// connection on the name). The packs are checked by SHA-256 whichever
/// answers.
pub const HOSTS: [&str; 2] = [
    "https://v1.shelf.phitex.org/tl2026/",
    "https://shelf-phitex-v1.pages.dev/tl2026/",
];

/// The release's directories to ask, in order, each ending in `/`:
/// `PHITEX_SHELF_URL` alone, else [`HOSTS`].
fn base_urls() -> Vec<String> {
    match std::env::var("PHITEX_SHELF_URL") {
        Ok(u) if !u.trim().is_empty() => {
            let u = u.trim();
            vec![if u.ends_with('/') {
                u.to_owned()
            } else {
                format!("{u}/")
            }]
        }
        _ => HOSTS.iter().map(|&h| h.to_owned()).collect(),
    }
}

/// The host of [`base_urls`] asked first: the last one that answered
/// (one a request could not reach hands it to the next, round to the
/// first after the last).
static HOST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_mins(2)))
            .timeout_connect(Some(std::time::Duration::from_secs(10)))
            .http_status_as_error(true)
            .build()
            .new_agent()
    })
}

/// `name` of the release's directory on the server: its bytes. Each
/// host is asked once, from [`HOST`] on; one that cannot be reached (no
/// connection, TLS refused, the connection dropped) is passed over for
/// the next, here and for the requests after; one that answers, with
/// anything, is the answer. A failure is this request's only: the next
/// asks the hosts again (one dropped connection had made every later
/// fetch of the process fail, the Shelf release's check, run 41296048903).
fn get(name: &str) -> Result<Vec<u8>, String> {
    use std::sync::atomic::Ordering;
    let bases = base_urls();
    let mut failed = Vec::new();
    let first = HOST.load(Ordering::Relaxed) % bases.len();
    let (mut r, url) = 'hosts: {
        for i in 0..bases.len() {
            let k = (first + i) % bases.len();
            let url = format!("{}{name}", bases[k]);
            match agent().get(&url).call() {
                Ok(r) => break 'hosts (r, url),
                Err(e @ ureq::Error::StatusCode(_)) => return Err(format!("{url}: {e}")),
                Err(e) => {
                    failed.push(format!("{url}: {e}"));
                    let next = (k + 1) % bases.len();
                    let _ = HOST.compare_exchange(k, next, Ordering::Relaxed, Ordering::Relaxed);
                }
            }
        }
        return Err(format!(
            "{name}: no Shelf host reached: {}",
            failed.join("; ")
        ));
    };
    r.body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

/// `bytes` written to `p` whole (a temporary file renamed: processes
/// share the cache).
fn keep(p: &Path, bytes: &[u8]) -> Result<(), String> {
    let d = p.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = p.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

/// File `name` of the release: from the package cache, else fetched and
/// kept there.
fn cached(name: &str) -> Result<Vec<u8>, String> {
    let p = dir().join(name);
    if let Ok(b) = std::fs::read(&p) {
        return Ok(b);
    }
    let b = get(name)?;
    keep(&p, &b)?;
    Ok(b)
}

/// The release's index (with its pack table): read once a
/// process, from the package cache (fetched once a release).
pub fn index() -> Result<Arc<Index>, String> {
    static INDEX: OnceLock<Result<Arc<Index>, String>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let rel = release();
            let meta = cached(&format!("release-{rel}.json"))?;
            let meta: serde_json::Value = serde_json::from_slice(&meta)
                .map_err(|e| format!("Shelf's release-{rel}.json: {e}"))?;
            let schema = meta["schema"].as_u64().unwrap_or(0);
            if !phitex_shelf::SCHEMAS.contains(&schema) {
                return Err(format!(
                    "Shelf's {rel} is schema {schema}: this PhiTeX reads schema 1 (v1.shelf.phitex.org)"
                ));
            }
            let text = |name: &str| -> Result<String, String> {
                let gz = cached(name)?;
                let b =
                    phitex_shelf::gunzip(&gz).ok_or_else(|| format!("Shelf's {name}: not gzip"))?;
                String::from_utf8(b).map_err(|_| format!("Shelf's {name}: not UTF-8"))
            };
            let index_name = meta["index"]
                .as_str()
                .ok_or("Shelf's release.json names no index")?;
            let table = meta["pack_table"]
                .as_str()
                .ok_or("Shelf's release.json names no pack table")?;
            let ix = Index::new(&text(index_name)?, &meta).with_pack_table(&text(table)?)?;
            Ok(Arc::new(ix))
        })
        .clone()
}

/// Packs from the package cache, else the server (and then kept): each
/// checked by the files before use, kept only once checked.
struct Packs {
    index: Arc<Index>,
}

impl PackSource for Packs {
    fn pack(&self, id: &str) -> Result<Vec<u8>, String> {
        let p = dir().join("h").join(format!("{id}.pack"));
        if let Ok(b) = std::fs::read(&p) {
            HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(b);
        }
        MISSES.fetch_add(1, Ordering::Relaxed);
        let b = get(&format!("h/{id}.pack"))?;
        // (kept only as the pack table has it: what differs is refused,
        // never kept)
        self.index.check(id, &b)?;
        keep(&p, &b)?;
        Ok(b)
    }

    /// Fetched at once, a thread each (eight at most at a time).
    fn packs(&self, ids: &[&str]) -> Vec<Result<Vec<u8>, String>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(8) {
            std::thread::scope(|s| {
                let hs: Vec<_> = chunk
                    .iter()
                    .map(|id| s.spawn(move || self.pack(id)))
                    .collect();
                for h in hs {
                    out.push(
                        h.join()
                            .unwrap_or_else(|_| Err("a fetch panicked".to_owned())),
                    );
                }
            });
        }
        out
    }
}

/// Shelf's files for `engine` (the program's name), its fetches told to
/// the session's `meter`.
pub fn files(
    engine: &str,
    meter: &Arc<phitex_live::progress::Meter>,
) -> Result<Arc<Files>, String> {
    let index = index()?;
    let source = Arc::new(Packs {
        index: index.clone(),
    });
    let files = Arc::new(Files::new(index, source, phitex_shelf::engine(engine)));
    let m = meter.clone();
    files.on_fetch(Box::new(move |f| match *f {
        Fetch::Asked { name, pack } => m.fetch(name, pack, false),
        Fetch::Arrived { name, pack } => m.fetch(name, pack, true),
        // (said: the terminal's warning, the editor's message)
        Fetch::Failed { name, pack, why } => {
            m.fetch(name, pack, true);
            m.unreachable(&format!("{name}: {why}"));
        }
    }));
    Ok(files)
}

/// A project's file `name` (from the directory the build runs in, as
/// kpathsea's `.` finds it, or an absolute name): its path, where it is
/// readable and not a directory (kpathsea's `READABLE`: `/dev/null` is
/// one).
fn project(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    (std::fs::metadata(p).is_ok_and(|m| !m.is_dir()) && std::fs::File::open(p).is_ok())
        .then(|| p.to_path_buf())
}

/// dvipdfmx's `format` as kpathsea names it (a release's `search` key;
/// one it has no list for finds the name's smallest path).
fn dpx_format(format: phitex_live::dvipdf::Format) -> &'static str {
    use phitex_live::dvipdf::Format as F;
    match format {
        F::Fontmap => "map",
        F::Type1 => "type1",
        F::TrueType => "truetype",
        F::OpenType => "opentype",
        F::Enc => "enc",
        F::Tfm => "tfm",
        F::Vf => "vf",
        F::Sfd => "sfd",
        F::Ofm => "ofm",
        F::Ovf => "ovf",
        F::Cmap => "cmap",
        F::Pict => "pict",
        F::Tex => "tex",
        F::ProgramText => "other text files",
        F::ProgramBinary => "other binary files",
        F::Pk => "pk",
    }
}

/// The suffix kpathsea adds in dvipdfmx's `format` to a name without one.
fn dpx_suffix(format: phitex_live::dvipdf::Format) -> &'static str {
    use phitex_live::dvipdf::Format as F;
    match format {
        F::Fontmap => ".map",
        F::Type1 => ".pfb",
        F::TrueType => ".ttf",
        F::OpenType => ".otf",
        F::Enc => ".enc",
        F::Tfm => ".tfm",
        F::Vf => ".vf",
        F::Sfd => ".sfd",
        F::Ofm => ".ofm",
        F::Ovf => ".ovf",
        F::Cmap | F::Pict | F::Tex | F::ProgramText | F::ProgramBinary | F::Pk => "",
    }
}

/// The machine with Shelf's files: the project's own first, then
/// Shelf's, never TeX Live's.
pub struct ShelfHost {
    pub files: Arc<Files>,
    /// The search paths a `.latexmkrc` set (TEXINPUTS, BIBINPUTS,
    /// BSTINPUTS), over the project's directory and Shelf's.
    pub inputs: Arc<Inputs>,
}

/// A `.latexmkrc`'s search paths as folders on disk: each variable's
/// folders in order (a `//` element's subfolders after it, by name),
/// `None` where the default search goes (the project's directory, then
/// Shelf). A variable not set: the default search alone.
#[derive(Debug, Default)]
pub struct Inputs {
    tex: Option<Vec<Option<PathBuf>>>,
    bib: Option<Vec<Option<PathBuf>>>,
    bst: Option<Vec<Option<PathBuf>>>,
}

impl Inputs {
    /// The folders of `vars` (a job's `inputs`).
    pub fn of(vars: &[(String, String)]) -> Inputs {
        let dirs = |var: &str| {
            let v = vars.iter().find(|(k, _)| k == var)?;
            let mut out = Vec::new();
            for e in phitex_live::latexmkrc::elements(&v.1) {
                match e {
                    phitex_live::latexmkrc::Element::Default => out.push(None),
                    phitex_live::latexmkrc::Element::Dir { path, sub } => {
                        let p = expand_home(&path);
                        if sub {
                            subdirs(&p, &mut out, 0);
                        } else {
                            out.push(Some(p));
                        }
                    }
                }
            }
            Some(out)
        };
        Inputs {
            tex: dirs("TEXINPUTS"),
            bib: dirs("BIBINPUTS"),
            bst: dirs("BSTINPUTS"),
        }
    }
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix('~'), crate::dirs::home()) {
        (Some(rest), Some(h)) => PathBuf::from(format!("{}{rest}", h.to_string_lossy())),
        _ => PathBuf::from(p),
    }
}

/// `dir` and its subfolders, depth first, each folder's by name (hidden
/// ones left out, as kpathsea leaves them; at most 32 deep).
fn subdirs(dir: &Path, out: &mut Vec<Option<PathBuf>>, depth: usize) {
    if !dir.is_dir() {
        return;
    }
    out.push(Some(dir.to_path_buf()));
    if depth >= 32 {
        return;
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    subs.sort();
    for s in subs {
        subdirs(&s, out, depth + 1);
    }
}

/// `name` along `dirs` (none: the default search alone), `default` where
/// its empty element is; an absolute name or one from `./` or `../` is
/// not searched (kpathsea's rule).
fn along<T>(
    dirs: Option<&Vec<Option<PathBuf>>>,
    name: &str,
    found: impl Fn(PathBuf) -> Option<T>,
    default: impl Fn() -> Option<T>,
) -> Option<T> {
    let Some(dirs) = dirs else {
        return default();
    };
    if name.starts_with('/') || name.starts_with("./") || name.starts_with("../") {
        return default();
    }
    dirs.iter().find_map(|d| match d {
        Some(d) => project(&d.join(name).to_string_lossy()).and_then(&found),
        None => default(),
    })
}

impl ShelfHost {
    /// The host on `files`, the job's search variables `vars` applied.
    pub fn new(files: Arc<Files>, vars: &[(String, String)]) -> ShelfHost {
        ShelfHost {
            files,
            inputs: Arc::new(Inputs::of(vars)),
        }
    }

    fn plain(files: Arc<Files>) -> ShelfHost {
        ShelfHost {
            files,
            inputs: Arc::default(),
        }
    }
}

/// The release's font index from `files` ([`FONT_INDEX`]), its relative
/// paths named under TeX Live's tree.
///
/// # Errors
/// The release has none, or it is not one.
pub fn font_index(files: &Files) -> Result<phitex_tex::FontIndex, String> {
    let (_, b) = files.find(&format!("{}{FONT_INDEX}", phitex_shelf::TL)).ok_or_else(|| {
        format!(
            "Shelf's {} has no font index ({FONT_INDEX}): XeTeX's fonts by name need a release made with one",
            files.index().release()
        )
    })?;
    let mut ix = phitex_tex::FontIndex::from_bytes(&b)
        .ok_or_else(|| format!("Shelf's {FONT_INDEX} is not a font index (PXFI)"))?;
    for e in &mut ix.entries {
        if !e.path.starts_with('/') {
            e.path = Arc::from(format!("{}{}", phitex_shelf::TL, e.path));
        }
    }
    Ok(ix)
}

/// kpathsea's suffix for XeTeX's font `format`, added to a name without
/// one.
fn font_suffix(format: phitex_tex::KpseFormat) -> &'static str {
    match format {
        phitex_tex::KpseFormat::OpenType => ".otf",
        phitex_tex::KpseFormat::TrueType => ".ttf",
        phitex_tex::KpseFormat::Type1 => ".pfb",
        phitex_tex::KpseFormat::MiscFonts => "",
    }
}

/// The name and the kpathsea format of a finder's key
/// ([`phitex_tex::Finder`]): the font side's in the format its extension
/// names, TeX's own (`\\input`, `\\openin`) in the `tex` format.
fn lookup(key: &str) -> (&str, &'static str) {
    match phitex_tex::font_name(key) {
        Some(name) => (name, phitex_shelf::format_of(name)),
        None => (key, "tex"),
    }
}

impl ShelfHost {
    fn find(&self, name: &str) -> Option<(String, Vec<u8>)> {
        if let Some(p) = project(name) {
            let b = std::fs::read(&p).ok()?;
            return Some((p.to_string_lossy().into_owned(), b));
        }
        self.files.find(name).map(|(p, b)| (p, b.to_vec()))
    }
}

impl Host for ShelfHost {
    fn locate(&self, name: &str) -> Option<PathBuf> {
        along(self.inputs.tex.as_ref(), name, Some, || {
            project(name).or_else(|| {
                self.files
                    .find_in(name, "tex")
                    .map(|(p, _)| PathBuf::from(p))
            })
        })
    }

    fn locate_font(&self, name: &str) -> Option<PathBuf> {
        project(name).or_else(|| self.files.locate(name).map(PathBuf::from))
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        let s = path.to_string_lossy();
        match self.files.read(&s) {
            Some(b) => Some(b.to_vec()),
            None if s.starts_with(phitex_shelf::TL) => None,
            None => std::fs::read(path).ok(),
        }
    }

    /// A watch's exact `wait`: the project's files the engine was given.
    fn fed(&self, path: &Path, bytes: &[u8]) {
        crate::review::fed(path, bytes);
    }

    /// A pack's file: its release's, the same for the session; the
    /// project's: as on disk ([`crate::session::disk_stamp`]).
    fn stamp(&self, path: &Path) -> Option<Vec<u8>> {
        let s = path.to_string_lossy();
        if s.starts_with(phitex_shelf::TL) {
            return self.files.read(&s).is_some().then(|| b"shelf".to_vec());
        }
        crate::session::disk_stamp(path)
    }

    fn same_file(&self, a: &str, b: &str) -> bool {
        match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        }
    }

    fn canonical(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// A pack's file (the cache's fonts too).
    fn distribution(&self, path: &Path) -> bool {
        path.to_string_lossy().starts_with(phitex_shelf::TL)
            || path.starts_with(crate::tex::cache_dir())
    }

    fn has_format_file(&self, name: &str) -> bool {
        self.files.path_of(name).is_some()
    }

    /// The format's files: Shelf's only (a format is built from TeX's
    /// files, not the project's, as fmtutil builds it elsewhere).
    fn format_finder(&self) -> Option<Finder> {
        let f = self.files.clone();
        Some(Arc::new(move |key: &str| {
            let (name, format) = lookup(key);
            f.find_in(name, format).map(|(p, b)| (p, b.to_vec()))
        }))
    }

    /// The format's files keyed by their content (the index's sums) and
    /// path, none fetched: a warm start checks the format against them
    /// without fetching them.
    fn content_key(&self) -> Option<phitex_tex::ContentKey> {
        let f = self.files.clone();
        Some(Arc::new(move |key: &str| {
            let (name, format) = lookup(key);
            f.key_in(name, format)
        }))
    }

    /// The release's own format of that name (made by the release for
    /// this engine's identity), its pack fetched and kept with the rest.
    fn published_format(&self, name: &str) -> Option<Vec<u8>> {
        self.files.published_format(name)
    }

    fn publishes_format(&self, name: &str) -> bool {
        self.files.publishes_format(name)
    }

    /// BibTeX's and makeindex's files, the format's suffix added where the
    /// name has none: the project's, else Shelf's.
    /// METAFONT's runs (on Shelf's sources) kept in the cache directory.
    fn mktex(&self) -> Option<phitex_tex::Mktex> {
        Some(crate::mkfont::maker())
    }

    fn tool_finder(&self) -> Option<ToolFinder> {
        let f = self.files.clone();
        let inputs = self.inputs.clone();
        Some(Arc::new(
            move |search: phitex_tex::ToolSearch, name: &str| {
                let ext = match search {
                    phitex_tex::ToolSearch::BibStyle => ".bst",
                    phitex_tex::ToolSearch::BibData => ".bib",
                    phitex_tex::ToolSearch::IndexStyle => ".ist",
                    // (Ghostscript's fonts by their file names, where
                    // Shelf serves them)
                    phitex_tex::ToolSearch::GsFont => {
                        return gs_font_path(name)
                            .and_then(|p| ShelfHost::plain(f.clone()).find(&p));
                    }
                    // (METAFONT's inputs and web2c's files, in their
                    // formats whatever the suffix: `mf.ini`, `cp227.tcx`)
                    // (`MFINPUTS` has the project's directory first)
                    phitex_tex::ToolSearch::Mf => {
                        if let Some(p) = project(name) {
                            let b = std::fs::read(&p).ok()?;
                            return Some((p.to_string_lossy().into_owned(), b));
                        }
                        return f.find_in(name, "mf").map(|(p, b)| (p, b.to_vec()));
                    }
                    phitex_tex::ToolSearch::Web2c => {
                        return f.find_in(name, "web2c").map(|(p, b)| (p, b.to_vec()));
                    }
                };
                let name = if name.ends_with(ext) {
                    name.to_owned()
                } else {
                    format!("{name}{ext}")
                };
                // (BIBINPUTS, BSTINPUTS: a `.latexmkrc`'s folders)
                let dirs = match search {
                    phitex_tex::ToolSearch::BibStyle => inputs.bst.as_ref(),
                    phitex_tex::ToolSearch::BibData => inputs.bib.as_ref(),
                    _ => None,
                };
                let read = |p: PathBuf| {
                    let b = std::fs::read(&p).ok()?;
                    Some((p.to_string_lossy().into_owned(), b))
                };
                along(dirs, &name, read, || {
                    ShelfHost::plain(f.clone()).find(&name)
                })
            },
        ))
    }

    /// The commands neither in process nor refused: run as processes,
    /// each in a scratch directory of its own, without TeX Live's
    /// variables (no kpathsea here).
    fn shell_hook(&self, job: &Job) -> Option<Arc<dyn phitex_tex::ShellHook>> {
        Some(Arc::new(
            crate::shell::ProcessHook::new(
                job.path
                    .parent()
                    .filter(|d| !d.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
                std::iter::empty(),
            )
            .with_vars(&job.inputs),
        ))
    }

    /// dvipdfmx's files: the project's (pictures), else Shelf's by name in
    /// its format (its suffix added where the name has none).
    fn dpx_finder(&self) -> Option<Arc<phitex_live::dvipdf::DpxFinder>> {
        let (f1, f2) = (self.files.clone(), self.files.clone());
        Some(Arc::new(
            phitex_live::dvipdf::DpxFinder::new(
                Box::new(move |name, format, _prog| {
                    let n = String::from_utf8_lossy(name).into_owned();
                    if let Some(p) = project(&n) {
                        return Some(p.to_string_lossy().into_owned().into_bytes());
                    }
                    let suffix = dpx_suffix(format);
                    let n = if suffix.is_empty() || n.contains('.') {
                        n
                    } else {
                        format!("{n}{suffix}")
                    };
                    f1.find_in(&n, dpx_format(format))
                        .map(|(p, _)| p.into_bytes())
                }),
                Box::new(move |path| {
                    let p = String::from_utf8_lossy(path).into_owned();
                    match f2.read(&p) {
                        Some(b) => Some(Arc::from(b.as_slice())),
                        None if p.starts_with(phitex_shelf::TL) => {
                            f2.find(&p).map(|(_, b)| Arc::from(b.as_slice()))
                        }
                        None => std::fs::read(&p).ok().map(Arc::from),
                    }
                }),
            )
            .with_gs_font({
                let f3 = self.files.clone();
                Box::new(move |name| {
                    // (Ghostscript's fonts by their file names, where
                    // Shelf serves them)
                    let p = gs_font_path(std::str::from_utf8(name).ok()?)?;
                    ShelfHost::plain(f3.clone())
                        .find(&p)
                        .map(|(_, b)| Arc::from(b.as_slice()))
                })
            })
            .with_stamp({
                let f4 = self.files.clone();
                Box::new(move |path| {
                    let p = std::str::from_utf8(path).ok()?;
                    ShelfHost::plain(f4.clone()).stamp(Path::new(p))
                })
            }),
        ))
    }

    /// XeTeX's fonts: the release's font index ([`font_index`]), the
    /// faces and `[file]` names from Shelf's packs (the project's own
    /// first), TECkit's mappings the same.
    fn xetex(&self) -> Option<phitex_tex::XeTeX> {
        let ix = font_index(&self.files).ok()?;
        let (f1, f2) = (self.files.clone(), self.files.clone());
        Some(phitex_live::xefonts::xetex_of(
            Box::new(move |path| match f1.read(path) {
                Some(b) => Some(Arc::from(b.as_slice())),
                None if path.starts_with(phitex_shelf::TL) => {
                    f1.find(path).map(|(_, b)| Arc::from(b.as_slice()))
                }
                None => std::fs::read(path).ok().map(Arc::from),
            }),
            Box::new(move |name, format| {
                if let Some(p) = project(name) {
                    return Some(p.to_string_lossy().into_owned());
                }
                let suffix = font_suffix(format);
                let kind = match format {
                    phitex_tex::KpseFormat::OpenType => "opentype",
                    phitex_tex::KpseFormat::TrueType => "truetype",
                    phitex_tex::KpseFormat::Type1 => "type1",
                    phitex_tex::KpseFormat::MiscFonts => "misc",
                };
                if !suffix.is_empty()
                    && !name.contains('.')
                    && let Some((p, _)) = f2.find_in(&format!("{name}{suffix}"), kind)
                {
                    return Some(p);
                }
                f2.find_in(name, kind).map(|(p, _)| p)
            }),
            Some(ix),
        ))
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
}

/// Where Shelf serves Ghostscript's font `name` (a file name, no
/// directory): [`phitex_shelf::GS_FONTS`].
fn gs_font_path(name: &str) -> Option<String> {
    (!name.is_empty() && !name.contains('/')).then(|| format!("{}/{name}", phitex_shelf::GS_FONTS))
}
