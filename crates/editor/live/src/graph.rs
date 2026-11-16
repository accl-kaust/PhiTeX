//! The persisted graph's file (docs/spec-persisted-graph.md §3, §7, step
//! 3): a session's build saved whole in the document's cache directory
//! ([`crate::host::Key::Doc`] `graph`) when a build or a watch ends, and
//! loaded by the next session of the same document when its keys hold.
//! The command line keeps graphs by default (`graph` in `phitex.toml`,
//! `PHITEX_GRAPH` over it); a session of this library, when its host
//! says so ([`crate::session::Session::keep_graphs`]), else with
//! `PHITEX_GRAPH=1`. Its first build is then an edit
//! ([`crate::session::Session`]).
//!
//! The file, all or nothing:
//! - its head: [`MAGIC`], [`VERSION`], the payload's length, the chunks'
//!   size and count, and each chunk's hash ([`crate::tex::words_hash`],
//!   which tells any change of one 8-byte word, a flipped bit included);
//!   a short file, a hash wrong or a length out of step is refused before
//!   anything is decoded;
//! - its payload: the keys ([`Key`], each by name, so a refusal names the
//!   one that differs), the format's key, the manifest ([`Manifest`]: the
//!   main text, the files the builds read with their stat keys, the files
//!   read outside the environment), and the build ([`phitex_tex::Build::save`]).
//!
//! Written to a temporary file, synced and renamed by the host
//! ([`crate::host::Store::put_durable`]): a crash leaves the old file or
//! the new one.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crate::host::Host;

/// What `PHITEX_GRAPH` says, set: whether builds load and save their
/// graph (`0` off; `1`, `sync` or anything else on). Over the command
/// line's `graph` setting; a session's default where its host says
/// nothing ([`on`]).
#[must_use]
pub fn env() -> Option<bool> {
    static ON: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("PHITEX_GRAPH")
            .filter(|x| !x.is_empty())
            .map(|x| x != "0")
    })
}

/// A session's default before its host says ([`crate::session::Session::keep_graphs`]):
/// `PHITEX_GRAPH=1`.
#[must_use]
pub fn on() -> bool {
    env() == Some(true)
}

/// Whether `phitex build` saves its graph before it ends
/// (`PHITEX_GRAPH=sync`), not after it has ended for its caller (the
/// default: the build in a worker process, which saves when the waiting
/// one has gone, spec §6).
#[must_use]
pub fn sync() -> bool {
    std::env::var_os("PHITEX_GRAPH").is_some_and(|x| x == "sync")
}

const MAGIC: &[u8; 8] = b"PXGFILE\x01";
/// The file's layout's version: any change of it bumps it (the build's
/// own bytes carry their own, under [`phitex_tex::engine_id`]).
pub const VERSION: u32 = 1;
/// The chunks hashed apart.
const CHUNK: usize = 1 << 20;
const HEAD: usize = 8 + 4 + 8 + 4 + 4;

/// The variables of the process's environment that change what a build
/// makes, read by the engine or the host outside the options (one table,
/// the key's: never asked ad hoc). Their values are among the keys.
pub const KEY_VARS: &[&str] = &[
    // (`\pdfcreationdate`, the tools' dates: read by the engine, not given
    // by the session)
    "SOURCE_DATE_EPOCH",
    "FORCE_SOURCE_DATE",
    // (the finder's search paths, beside those the job gives)
    "TEXINPUTS",
    "TEXMFHOME",
    "BIBINPUTS",
    "BSTINPUTS",
    "TEXMF_OUTPUT_DIRECTORY",
    // (the engine's switches that change a run's product or the graph's
    // shape)
    "PHITEX_BOXSCOPE",
    "PHITEX_HINTS",
    "PHITEX_CUT_MIN",
    "PHITEX_SPEC_WINDOW",
    "PHITEX_INCR_DOC",
    "PHITEX_RUNS_PER_NODE",
    "PHITEX_XETEX",
    "PHITEX_GS_FONTS",
    "PHITEX_COMPAT",
    "PHITEX_SHELF_RELEASE",
];

/// The keys: each by name, with its value as text. Any one that differs
/// means no load.
pub type Key = Vec<(String, String)>;

/// What the session keeps beside the build (docs/spec-persisted-graph.md
/// §2.2.3, §3.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The main file's text the build is for (TeX's characters).
    pub text: String,
    /// Every name the builds asked the finder for: whether it was found,
    /// where, and whether it is the document's own (not the distribution's).
    pub inputs: Vec<(String, bool, Option<String>, bool)>,
    /// The stat keys of the files found, as they were when read.
    pub stamps: Vec<(String, Vec<u8>)>,
    /// What the builds read outside the environment ([`Outside`]).
    pub outside: OutsideLog,
    /// The cold build's wall (ms): an edit's stuck bound is made of it.
    pub cold_ms: u64,
}

/// The lookups and reads XeTeX's fonts and its PDF's writer made through
/// the host, not through the engine's environment (their files are not
/// `file:` fields): each lookup with its answer, each file read with
/// whether it was there and its stat key when first read (empty: read
/// under two keys).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutsideLog {
    pub finds: BTreeMap<(u8, String, u8, String), Option<String>>,
    pub reads: BTreeMap<(u8, String), (bool, Option<Vec<u8>>)>,
}

impl OutsideLog {
    /// The PDF writer's lookups and reads dropped (its file made again:
    /// it does them all again).
    pub fn writer_dropped(&mut self) {
        self.finds.retain(|k, _| k.0 != DPX);
        self.reads.retain(|k, _| k.0 != DPX);
    }
}

/// [`OutsideLog`], shared by the wrappers that fill it.
pub type Outside = Arc<Mutex<OutsideLog>>;

/// A lookup's kind in [`OutsideLog::finds`].
const FONTS: u8 = 1;
const DPX: u8 = 2;

/// File `path` read, its key `k` taken before the read: whether it was
/// there.
fn note_read(log: &Outside, kind: u8, path: &str, k: Option<Vec<u8>>, found: bool) {
    let mut l = log.lock().unwrap_or_else(PoisonError::into_inner);
    match l.reads.get_mut(&(kind, path.to_owned())) {
        Some(old) if *old != (found, k.clone()) => *old = (found, Some(Vec::new())),
        Some(_) => {}
        None => {
            l.reads.insert((kind, path.to_owned()), (found, k));
        }
    }
}

fn note_find(log: &Outside, key: (u8, String, u8, String), found: Option<&str>) {
    let mut l = log.lock().unwrap_or_else(PoisonError::into_inner);
    let found = found.map(str::to_owned);
    match l.finds.get(&key) {
        // (answered otherwise within the session: never the same again)
        Some(x) if *x != found => {
            l.finds.insert(key, Some("\u{0}moved".to_owned()));
        }
        Some(_) => {}
        None => {
            l.finds.insert(key, found);
        }
    }
}

fn kpse_code(f: phitex_tex::KpseFormat) -> u8 {
    use phitex_tex::KpseFormat as K;
    match f {
        K::OpenType => 0,
        K::TrueType => 1,
        K::Type1 => 2,
        K::MiscFonts => 3,
    }
}

fn kpse_of(c: u8) -> Option<phitex_tex::KpseFormat> {
    use phitex_tex::KpseFormat as K;
    [K::OpenType, K::TrueType, K::Type1, K::MiscFonts]
        .get(usize::from(c))
        .copied()
}

/// [`phitex_xdvipdfmx::io::Format`] by its code ([`dpx_code`]).
const DPX_FORMATS: [phitex_xdvipdfmx::io::Format; 16] = {
    use phitex_xdvipdfmx::io::Format as F;
    [
        F::Fontmap,
        F::Type1,
        F::TrueType,
        F::OpenType,
        F::Cmap,
        F::Sfd,
        F::Enc,
        F::Tfm,
        F::Ofm,
        F::Vf,
        F::Ovf,
        F::Pict,
        F::Tex,
        F::ProgramText,
        F::ProgramBinary,
        F::Pk,
    ]
};

/// A format's code, its place in [`DPX_FORMATS`] (every one listed: a
/// new one is no match here until it is).
fn dpx_code(f: phitex_xdvipdfmx::io::Format) -> u8 {
    use phitex_xdvipdfmx::io::Format as F;
    match f {
        F::Fontmap => 0,
        F::Type1 => 1,
        F::TrueType => 2,
        F::OpenType => 3,
        F::Cmap => 4,
        F::Sfd => 5,
        F::Enc => 6,
        F::Tfm => 7,
        F::Ofm => 8,
        F::Vf => 9,
        F::Ovf => 10,
        F::Pict => 11,
        F::Tex => 12,
        F::ProgramText => 13,
        F::ProgramBinary => 14,
        F::Pk => 15,
    }
}

/// XeTeX's font files as the host gives them, each lookup and read noted
/// in `log`.
pub struct KeyedFonts {
    pub inner: phitex_tex::FontFiles,
    pub host: Arc<dyn Host>,
    pub log: Outside,
}

impl phitex_tex::FontSource for KeyedFonts {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        let k = self.host.stamp(Path::new(path));
        let b = self.inner.read(path);
        note_read(&self.log, FONTS, path, k, b.is_some());
        b
    }

    fn find_file(&self, name: &str, format: phitex_tex::KpseFormat) -> Option<String> {
        let found = self.inner.find_file(name, format);
        note_find(
            &self.log,
            (FONTS, name.to_owned(), kpse_code(format), String::new()),
            found.as_deref(),
        );
        found
    }
}

/// The PDF writer's files (XeTeX's xdvipdfmx) as the host gives them,
/// each lookup and read noted in `log`.
pub fn keyed_dpx(
    files: crate::dvipdf::FilesFor,
    host: Arc<dyn Host>,
    log: Outside,
) -> crate::dvipdf::FilesFor {
    Arc::new(move || -> Arc<dyn phitex_xdvipdfmx::io::Files> {
        Arc::new(KeyedDpx {
            inner: files(),
            host: host.clone(),
            log: log.clone(),
        })
    })
}

struct KeyedDpx {
    inner: Arc<dyn phitex_xdvipdfmx::io::Files>,
    host: Arc<dyn Host>,
    log: Outside,
}

impl phitex_xdvipdfmx::io::Files for KeyedDpx {
    fn find(
        &self,
        name: &[u8],
        format: phitex_xdvipdfmx::io::Format,
        progname: &[u8],
    ) -> Option<Vec<u8>> {
        let found = self.inner.find(name, format, progname);
        note_find(
            &self.log,
            (
                DPX,
                String::from_utf8_lossy(name).into_owned(),
                dpx_code(format),
                String::from_utf8_lossy(progname).into_owned(),
            ),
            found.as_deref().map(String::from_utf8_lossy).as_deref(),
        );
        found
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        let p = String::from_utf8_lossy(path);
        let k = self.host.stamp(Path::new(&*p));
        let b = self.inner.read(path);
        note_read(&self.log, DPX, &p, k, b.is_some());
        b
    }

    fn apply_filter(&self, template: &[u8], input: &[u8], version: i32) -> Option<Arc<[u8]>> {
        self.inner.apply_filter(template, input, version)
    }

    fn gs_font(&self, name: &[u8]) -> Option<Arc<[u8]>> {
        self.inner.gs_font(name)
    }

    fn stamp(&self, path: &[u8]) -> Option<Vec<u8>> {
        self.inner.stamp(path)
    }

    fn kept(&self, file: &Arc<[u8]>, what: &[u8], variant: &[u8]) -> Option<Arc<[u8]>> {
        self.inner.kept(file, what, variant)
    }

    fn keep(&self, file: &Arc<[u8]>, what: &[u8], variant: &[u8], made: Arc<[u8]>) {
        self.inner.keep(file, what, variant, made);
    }
}

/// What of [`OutsideLog`] moved now, asked of `fonts` and `dpx` as this
/// session has them (none: no XeTeX, and any noted is moved): the first
/// of XeTeX's fonts' lookups answered otherwise or files whose key moved
/// (`Err`: the engine's runs read them, unrecorded: no load); else
/// whether one of the PDF writer's did (`Ok(true)`: its file made again,
/// [`phitex_tex::Build::relink`]).
///
/// # Errors
///
/// A font file of the engine's moved.
pub fn outside_moved(
    log: &OutsideLog,
    host: &dyn Host,
    fonts: Option<&phitex_tex::FontFiles>,
    dpx: Option<&crate::dvipdf::FilesFor>,
) -> Result<bool, String> {
    let dpx = dpx.map(|f| f());
    let mut writer = false;
    for ((kind, name, fmt, prog), was) in &log.finds {
        let now = match *kind {
            FONTS => fonts.zip(kpse_of(*fmt)).map(|(f, k)| f.find_file(name, k)),
            DPX => dpx
                .as_ref()
                .zip(DPX_FORMATS.get(usize::from(*fmt)))
                .map(|(f, &k)| {
                    f.find(name.as_bytes(), k, prog.as_bytes())
                        .map(|p| String::from_utf8_lossy(&p).into_owned())
                }),
            _ => None,
        };
        if now.as_ref() != Some(was) {
            if *kind == DPX {
                writer = true;
                continue;
            }
            return Err(format!("the font file {name} is found elsewhere"));
        }
    }
    for ((kind, path), (found, was)) in &log.reads {
        let now = host.stamp(Path::new(path));
        // (one read: its key, not empty, the same; one not there: still
        // not there)
        let same = if *found {
            was.as_ref().is_some_and(|k| !k.is_empty()) && now == *was
        } else {
            was.as_ref().is_none_or(|k| !k.is_empty()) && now.is_none()
        };
        if !same {
            if *kind == DPX {
                writer = true;
                continue;
            }
            return Err(format!("the font file {path} changed"));
        }
    }
    Ok(writer)
}

/// The file's parts, read back.
#[derive(Debug)]
pub struct Parts<'a> {
    pub key: Key,
    pub prelude: [u8; 32],
    pub manifest: Manifest,
    pub build: &'a [u8],
}

/// The file: `key`, the format's key `prelude`, `manifest` and the
/// build's bytes `build`, hashed by chunk.
#[must_use]
pub fn encode(key: &Key, prelude: &[u8; 32], manifest: &Manifest, build: &[u8]) -> Vec<u8> {
    let mut e = Enc(Vec::new());
    e.n(key.len());
    for (k, v) in key {
        e.s(k);
        e.s(v);
    }
    e.0.extend_from_slice(prelude);
    e.s(&manifest.text);
    e.n(manifest.inputs.len());
    for (n, found, p, ours) in &manifest.inputs {
        e.s(n);
        e.b(*found);
        e.os(p.as_deref());
        e.b(*ours);
    }
    e.n(manifest.stamps.len());
    for (p, k) in &manifest.stamps {
        e.s(p);
        e.by(k);
    }
    e.n(manifest.outside.finds.len());
    for ((kind, name, fmt, prog), found) in &manifest.outside.finds {
        e.0.push(*kind);
        e.s(name);
        e.0.push(*fmt);
        e.s(prog);
        e.os(found.as_deref());
    }
    e.n(manifest.outside.reads.len());
    for ((kind, p), (found, k)) in &manifest.outside.reads {
        e.0.push(*kind);
        e.s(p);
        e.b(*found);
        e.b(k.is_some());
        if let Some(k) = k {
            e.by(k);
        }
    }
    e.u(manifest.cold_ms);
    let mut payload = e.0;
    payload.reserve(build.len());
    payload.extend_from_slice(build);
    let chunks = payload.len().div_ceil(CHUNK);
    let mut out = Vec::with_capacity(HEAD + 8 * chunks + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&u32::try_from(CHUNK).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&u32::try_from(chunks).unwrap_or(u32::MAX).to_le_bytes());
    for c in payload.chunks(CHUNK) {
        out.extend_from_slice(&crate::tex::words_hash(c).to_le_bytes());
    }
    out.extend_from_slice(&payload);
    out
}

/// The file's parts, every chunk's hash checked before any is decoded.
///
/// # Errors
///
/// Not a graph file, another version, cut short or longer, a chunk's
/// hash wrong, a part out of range.
pub fn decode(b: &[u8]) -> Result<Parts<'_>, String> {
    if b.len() < HEAD || &b[..8] != MAGIC {
        return Err("not a graph file".into());
    }
    let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap_or([0; 4]));
    let ver = u32_at(8);
    if ver != VERSION {
        return Err(format!("a graph file of version {ver}, not {VERSION}"));
    }
    let len = u64::from_le_bytes(b[12..20].try_into().unwrap_or([0; 8]));
    let (chunk, chunks) = (u32_at(20) as usize, u32_at(24) as usize);
    let len = usize::try_from(len).map_err(|_| "a length out of range")?;
    if chunk == 0 || chunks != len.div_ceil(chunk) {
        return Err("the chunks' count is not the payload's".into());
    }
    let start = chunks
        .checked_mul(8)
        .and_then(|h| h.checked_add(HEAD))
        .ok_or("a length out of range")?;
    if start.checked_add(len) != Some(b.len()) {
        return Err(format!(
            "the file is {} bytes, its head says {}",
            b.len(),
            start.saturating_add(len)
        ));
    }
    let payload = &b[start..];
    for (k, c) in payload.chunks(chunk).enumerate() {
        let at = HEAD + 8 * k;
        let h = u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
        if crate::tex::words_hash(c) != h {
            return Err(format!("chunk {k}'s hash is wrong"));
        }
    }
    let mut d = Dec { b: payload, i: 0 };
    let n = d.n()?;
    let mut key = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        key.push((d.s()?, d.s()?));
    }
    let prelude: [u8; 32] = d.take(32)?.try_into().map_err(|_| "the format's key")?;
    let text = d.s()?;
    let n = d.n()?;
    let mut inputs = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        inputs.push((d.s()?, d.b()?, d.os()?, d.b()?));
    }
    let n = d.n()?;
    let mut stamps = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        stamps.push((d.s()?, d.by()?.to_vec()));
    }
    let mut outside = OutsideLog::default();
    for _ in 0..d.n()? {
        let kind = d.byte()?;
        let name = d.s()?;
        let fmt = d.byte()?;
        let prog = d.s()?;
        let found = d.os()?;
        outside.finds.insert((kind, name, fmt, prog), found);
    }
    for _ in 0..d.n()? {
        let kind = d.byte()?;
        let p = d.s()?;
        let found = d.b()?;
        let k = if d.b()? { Some(d.by()?.to_vec()) } else { None };
        outside.reads.insert((kind, p), (found, k));
    }
    let cold_ms = d.u()?;
    let build = &payload[d.i..];
    Ok(Parts {
        key,
        prelude,
        manifest: Manifest {
            text,
            inputs,
            stamps,
            outside,
            cold_ms,
        },
        build,
    })
}

/// The first key of `now` the file's `saved` differ in, by name (none:
/// all hold).
#[must_use]
pub fn key_differs(saved: &Key, now: &Key) -> Option<String> {
    let s: BTreeMap<&str, &str> = saved
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let n: BTreeMap<&str, &str> = now.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    n.iter()
        .find(|(k, v)| s.get(*k) != Some(*v))
        .map(|(k, _)| (*k).to_owned())
        .or_else(|| {
            s.keys()
                .find(|k| !n.contains_key(*k))
                .map(|k| (*k).to_owned())
        })
}

struct Enc(Vec<u8>);

impl Enc {
    fn u(&mut self, mut x: u64) {
        while x >= 0x80 {
            self.0.push(u8::try_from(x & 0x7f).unwrap_or(0) | 0x80);
            x >>= 7;
        }
        self.0.push(u8::try_from(x).unwrap_or(0));
    }
    fn n(&mut self, x: usize) {
        self.u(x as u64);
    }
    fn b(&mut self, x: bool) {
        self.0.push(u8::from(x));
    }
    fn by(&mut self, b: &[u8]) {
        self.n(b.len());
        self.0.extend_from_slice(b);
    }
    fn s(&mut self, s: &str) {
        self.by(s.as_bytes());
    }
    fn os(&mut self, s: Option<&str>) {
        self.b(s.is_some());
        if let Some(s) = s {
            self.s(s);
        }
    }
}

struct Dec<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let e = self
            .i
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or("the manifest cut short")?;
        let s = &self.b[self.i..e];
        self.i = e;
        Ok(s)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u(&mut self) -> Result<u64, String> {
        let mut x = 0u64;
        for k in 0..10 {
            let c = self.byte()?;
            x |= u64::from(c & 0x7f) << (7 * k);
            if c & 0x80 == 0 {
                return Ok(x);
            }
        }
        Err("a number too long".into())
    }
    fn n(&mut self) -> Result<usize, String> {
        usize::try_from(self.u()?).map_err(|_| "a count out of range".into())
    }
    fn b(&mut self) -> Result<bool, String> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            x => Err(format!("{x} is no flag")),
        }
    }
    fn by(&mut self) -> Result<&'a [u8], String> {
        let n = self.n()?;
        self.take(n)
    }
    fn s(&mut self) -> Result<String, String> {
        String::from_utf8(self.by()?.to_vec()).map_err(|_| "a name not UTF-8".into())
    }
    fn os(&mut self) -> Result<Option<String>, String> {
        if self.b()? {
            Ok(Some(self.s()?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Key, [u8; 32], Manifest, Vec<u8>) {
        let key = vec![
            ("engine".to_owned(), "1".to_owned()),
            ("main".to_owned(), "/d/a.tex".to_owned()),
        ];
        let mut outside = OutsideLog::default();
        outside.finds.insert(
            (FONTS, "lmroman10-regular".into(), 0, String::new()),
            Some("/f/lm.otf".into()),
        );
        outside
            .reads
            .insert((FONTS, "/f/lm.otf".into()), (true, Some(vec![1, 2, 3])));
        outside
            .reads
            .insert((DPX, "/f/lm.otf/rsrc".into()), (false, None));
        let m = Manifest {
            text: "\\documentclass{article}\n".into(),
            inputs: vec![
                (
                    "article.cls".into(),
                    true,
                    Some("/tl/article.cls".into()),
                    false,
                ),
                ("a.aux".into(), false, None, false),
            ],
            stamps: vec![("/tl/article.cls".into(), vec![9; 24])],
            outside,
            cold_ms: 1234,
        };
        // (a build of more than two chunks, its last one short)
        let build: Vec<u8> = (0..(2 * CHUNK + 777))
            .map(|i| u8::try_from(i * 131 % 251).unwrap_or(0))
            .collect();
        (key, [7; 32], m, build)
    }

    /// The parts written are the parts read.
    #[test]
    fn a_graph_file_reads_back() {
        let (key, pre, m, build) = sample();
        let f = encode(&key, &pre, &m, &build);
        let p = decode(&f).expect("read back");
        assert_eq!(p.key, key);
        assert_eq!(p.prelude, pre);
        assert_eq!(p.manifest, m);
        assert_eq!(p.build, &build[..]);
        assert_eq!(key_differs(&p.key, &key), None);
        let mut other = key.clone();
        other[1].1 = "/d/b.tex".into();
        assert_eq!(key_differs(&p.key, &other).as_deref(), Some("main"));
        other.push(("synctex".into(), "1".into()));
        other[1].1 = "/d/a.tex".into();
        assert_eq!(key_differs(&p.key, &other).as_deref(), Some("synctex"));
    }

    /// A file cut anywhere, or with any one bit flipped anywhere, is
    /// refused before any part is decoded (docs/spec-persisted-graph.md
    /// §7): the build is then a clean build, never a load.
    #[test]
    fn a_cut_or_flipped_graph_file_is_refused() {
        let (key, pre, m, build) = sample();
        let f = encode(&key, &pre, &m, &build);
        // (cut: at every chunk's bounds and their neighbours, and at a
        // spread of places)
        let mut cuts: Vec<usize> = (0..f.len()).step_by(4099).collect();
        for k in 0..=f.len() / CHUNK + 1 {
            let at = HEAD + k * CHUNK;
            cuts.extend([at.saturating_sub(1), at, at + 1]);
        }
        cuts.extend(0..HEAD + 40);
        for c in cuts.into_iter().filter(|&c| c < f.len()) {
            assert!(decode(&f[..c]).is_err(), "cut at {c} read");
        }
        // (longer)
        let mut g = f.clone();
        g.push(0);
        assert!(decode(&g).is_err(), "a byte more read");
        // (a bit flipped: every bit of the head and the hashes, and bits
        // spread over the payload, each of its words' bytes)
        let mut at: Vec<usize> = (0..HEAD + 8 * 3 + 64).collect();
        at.extend((HEAD + 24..f.len()).step_by(997));
        for i in at.into_iter().filter(|&i| i < f.len()) {
            for bit in 0..8 {
                let mut g = f.clone();
                g[i] ^= 1 << bit;
                assert!(decode(&g).is_err(), "bit {bit} of byte {i} flipped read");
            }
        }
        // (another file's head)
        let mut g = f.clone();
        g[..8].copy_from_slice(b"PXBUILD\x01");
        assert!(decode(&g).is_err());
    }
}
