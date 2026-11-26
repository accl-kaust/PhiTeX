//! The persisted graph, the client's half (docs/spec-persisted-graph.md
//! §2.2, step 2): [`Build::save`] writes a build at its quiet point as
//! bytes, the engine's graph ([`phitex_defuse::Engine::export`]) with the client's
//! tables; [`Build::load`] makes a build of them that stands as the saved
//! one stood: its first evaluation is an ordinary edit, and with nothing
//! changed it runs nothing.
//!
//! The importing store holds every value where the saving store held it
//! (`Values::stable_raw`), so a table's value is saved as its stable id
//! and found again by [`Values::from_stable`]; each is given to the
//! engine's export as a root, so the import has it.
//!
//! Each item of the client's state is saved, made again on the load
//! from what is saved (the document's analysis, the environment's base,
//! the provider), taken from the options the load is given (the finder,
//! the threads, the tools), or dropped as a cache: the inventory is in
//! docs/spec-persisted-graph.md §2.2.

use super::{
    Build, Cached, Defs, DocFile, DocNode, Field, FxHashMap, Graph, Incr, NodeId, Options,
    Speculation, Src, Stats, UnitId, ValueId, Values, cut_text, prelude_defs, prelude_env,
};
use crate::exec::LockWait as _;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 8] = b"PXBUILD\x01";
/// The client's encoding's version: any change of its layout bumps it.
const VERSION: u32 = 4;
const HASH_SEED: u64 = 0x7078_6275_696c_6431;

type R<T> = Result<T, String>;

/// What a load did, and how long its parts took (ms).
#[derive(Debug, Default, Clone)]
pub struct LoadStats {
    pub bytes: u64,
    pub engine_bytes: u64,
    pub client_bytes: u64,
    pub import_ms: f64,
    pub client_ms: f64,
    pub analysis_ms: f64,
    /// Of the client's: the link's pages and commands, their fonts made
    /// again of their records.
    pub link_ms: f64,
    pub total_ms: f64,
    pub values: u64,
    pub nodes: u64,
    pub records: u64,
    pub reads: u64,
    /// `PHITEX_GRAPH_VERIFY`: the reads checked and those failing.
    pub verified: Option<(u64, u64)>,
}

/// The client's tables written: integers as LEB128, values by their
/// stable ids, each given to the engine's export as a root.
pub(crate) struct Enc<'a> {
    pub(crate) v: &'a Values,
    out: Vec<u8>,
    roots: Vec<ValueId>,
}

impl Enc<'_> {
    pub(crate) fn u(&mut self, mut x: u64) {
        while x >= 0x80 {
            self.out.push(u8::try_from(x & 0x7f).unwrap_or(0) | 0x80);
            x >>= 7;
        }
        self.out.push(u8::try_from(x).unwrap_or(0));
    }
    pub(crate) fn n(&mut self, x: usize) {
        self.u(x as u64);
    }
    /// A section's mark: a decoder out of step stops at the next one.
    pub(crate) fn tag(&mut self, k: u8) {
        self.out.push(0xa0 | k);
    }
    pub(crate) fn i(&mut self, x: i64) {
        self.u(((x << 1) ^ (x >> 63)).cast_unsigned());
    }
    pub(crate) fn b(&mut self, x: bool) {
        self.out.push(u8::from(x));
    }
    pub(crate) fn byte(&mut self, x: u8) {
        self.out.push(x);
    }
    pub(crate) fn by(&mut self, b: &[u8]) {
        self.n(b.len());
        self.out.extend_from_slice(b);
    }
    pub(crate) fn s(&mut self, s: &str) {
        self.by(s.as_bytes());
    }
    pub(crate) fn os(&mut self, s: Option<&str>) {
        self.b(s.is_some());
        if let Some(s) = s {
            self.s(s);
        }
    }
    pub(crate) fn x(&mut self, x: ValueId) {
        self.roots.push(x);
        let s = self.v.stable_raw(x);
        self.u(s);
    }
    pub(crate) fn ox(&mut self, x: Option<ValueId>) {
        self.b(x.is_some());
        if let Some(x) = x {
            self.x(x);
        }
    }
    pub(crate) fn f(&mut self, f: Field) {
        self.x(f.0);
    }
    pub(crate) fn fxs(&mut self, xs: &[(Field, ValueId)]) {
        self.n(xs.len());
        for &(f, x) in xs {
            self.f(f);
            self.x(x);
        }
    }
    pub(crate) fn strs(&mut self, xs: &[String]) {
        self.n(xs.len());
        for x in xs {
            self.s(x);
        }
    }
    pub(crate) fn u64s(&mut self, xs: &[u64]) {
        self.n(xs.len());
        for &x in xs {
            self.u(x);
        }
    }
}

/// The client's tables read back: every read checked against the end.
pub(crate) struct Dec<'a> {
    pub(crate) v: &'a Values,
    b: &'a [u8],
    i: usize,
}

impl Dec<'_> {
    pub(crate) fn u(&mut self) -> R<u64> {
        let mut x = 0u64;
        let mut shift = 0;
        loop {
            let c = *self.b.get(self.i).ok_or("the client's tables cut short")?;
            self.i += 1;
            if shift > 63 {
                return Err("an integer too long".into());
            }
            x |= u64::from(c & 0x7f) << shift;
            if c & 0x80 == 0 {
                return Ok(x);
            }
            shift += 7;
        }
    }
    pub(crate) fn n(&mut self) -> R<usize> {
        let x = usize::try_from(self.u()?).map_err(|_| "a count out of range")?;
        if x > self.b.len().saturating_sub(self.i).saturating_mul(8) + 64 {
            return Err(format!(
                "a count past the bytes left ({x} at byte {} of {})",
                self.i,
                self.b.len()
            ));
        }
        Ok(x)
    }
    /// A place or a length that counts no item of the bytes (a byte
    /// of a text, nodes live).
    pub(crate) fn z(&mut self) -> R<usize> {
        usize::try_from(self.u()?).map_err(|_| "a place out of range".into())
    }
    pub(crate) fn tag(&mut self, k: u8) -> R<()> {
        let at = self.i;
        if self.b.get(at) == Some(&(0xa0 | k)) {
            self.i += 1;
            return Ok(());
        }
        Err(format!(
            "the client's tables out of step at section {k} (byte {at})"
        ))
    }
    pub(crate) fn i(&mut self) -> R<i64> {
        let x = self.u()?;
        Ok((x >> 1).cast_signed() ^ -(x & 1).cast_signed())
    }
    pub(crate) fn i32(&mut self) -> R<i32> {
        i32::try_from(self.i()?).map_err(|_| "an i32 out of range".into())
    }
    pub(crate) fn b(&mut self) -> R<bool> {
        Ok(self.byte()? != 0)
    }
    pub(crate) fn byte(&mut self) -> R<u8> {
        let c = *self.b.get(self.i).ok_or("the client's tables cut short")?;
        self.i += 1;
        Ok(c)
    }
    pub(crate) fn by(&mut self) -> R<Vec<u8>> {
        let n = self.n()?;
        let e = self
            .i
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or("bytes cut short")?;
        let out = self.b[self.i..e].to_vec();
        self.i = e;
        Ok(out)
    }
    pub(crate) fn s(&mut self) -> R<String> {
        String::from_utf8(self.by()?).map_err(|_| "a string not UTF-8".into())
    }
    pub(crate) fn os(&mut self) -> R<Option<String>> {
        Ok(if self.b()? { Some(self.s()?) } else { None })
    }
    pub(crate) fn x(&mut self) -> R<ValueId> {
        let s = self.u()?;
        Ok(self.v.from_stable(s))
    }
    pub(crate) fn ox(&mut self) -> R<Option<ValueId>> {
        Ok(if self.b()? { Some(self.x()?) } else { None })
    }
    pub(crate) fn f(&mut self) -> R<Field> {
        Ok(Field(self.x()?))
    }
    pub(crate) fn fxs(&mut self) -> R<Vec<(Field, ValueId)>> {
        let n = self.n()?;
        (0..n).map(|_| Ok((self.f()?, self.x()?))).collect()
    }
    pub(crate) fn strs(&mut self) -> R<Vec<String>> {
        let n = self.n()?;
        (0..n).map(|_| self.s()).collect()
    }
    pub(crate) fn u64s(&mut self) -> R<Vec<u64>> {
        let n = self.n()?;
        (0..n).map(|_| self.u()).collect()
    }
    pub(crate) fn node(&mut self) -> R<NodeId> {
        Ok(NodeId::from_raw(self.u()?))
    }
}

fn put_fin_defs(e: &mut Enc<'_>, d: &crate::pages::FinDefs) {
    e.fxs(&d.prev);
    e.fxs(&d.last);
    e.i(d.prev_j);
    e.i(d.last_j);
    e.n(d.seen.len());
    for &f in &d.seen {
        e.f(f);
    }
    e.n(d.recent.len());
    for &(f, x, j, same) in &d.recent {
        e.f(f);
        e.x(x);
        e.i(j);
        e.b(same);
    }
    e.ox(d.gdefs);
    e.n(d.pages.len());
    for (&(a, b, c), (j, defs, pno)) in &d.pages {
        e.i(a);
        e.i(b);
        e.i(c);
        e.i(*j);
        e.fxs(defs);
        e.b(pno.is_some());
        e.i(pno.unwrap_or(0));
    }
    for p in [d.prev_pno, d.last_pno] {
        e.b(p.is_some());
        e.i(p.unwrap_or(0));
    }
    e.b(d.rels.is_some());
    if let Some(rs) = &d.rels {
        e.n(rs.len());
        for &(f, k) in rs {
            e.f(f);
            e.i(k);
        }
    }
}

fn get_fin_defs(d: &mut Dec<'_>) -> R<crate::pages::FinDefs> {
    let prev = d.fxs()?;
    let last = d.fxs()?;
    let prev_j = d.i()?;
    let last_j = d.i()?;
    let n = d.n()?;
    let seen = (0..n).map(|_| d.f()).collect::<R<Vec<_>>>()?;
    let n = d.n()?;
    let recent = (0..n)
        .map(|_| Ok((d.f()?, d.x()?, d.i()?, d.b()?)))
        .collect::<R<Vec<_>>>()?;
    let gdefs = d.ox()?;
    let n = d.n()?;
    let mut pages = std::collections::BTreeMap::new();
    for _ in 0..n {
        let k = (d.i()?, d.i()?, d.i()?);
        let j = d.i()?;
        let defs = d.fxs()?;
        let has = d.b()?;
        let p = d.i()?;
        pages.insert(k, (j, defs, has.then_some(p)));
    }
    let mut pno = [None; 2];
    for p in &mut pno {
        let has = d.b()?;
        let x = d.i()?;
        *p = has.then_some(x);
    }
    let rels = if d.b()? {
        let n = d.n()?;
        Some(
            (0..n)
                .map(|_| Ok((d.f()?, d.i()?)))
                .collect::<R<Vec<_>>>()?,
        )
    } else {
        None
    };
    Ok(crate::pages::FinDefs {
        prev,
        last,
        prev_j,
        last_j,
        seen,
        recent,
        gdefs,
        pages,
        prev_pno: pno[0],
        last_pno: pno[1],
        rels,
    })
}

fn put_fin_table(e: &mut Enc<'_>, t: &FxHashMap<u64, crate::pages::FinDefs>) {
    let mut ks: Vec<&u64> = t.keys().collect();
    ks.sort_unstable();
    e.n(ks.len());
    for k in ks {
        e.u(*k);
        put_fin_defs(e, &t[k]);
    }
}

fn get_fin_table(d: &mut Dec<'_>) -> R<FxHashMap<u64, crate::pages::FinDefs>> {
    let n = d.n()?;
    let mut t = FxHashMap::default();
    for _ in 0..n {
        let k = d.u()?;
        t.insert(k, get_fin_defs(d)?);
    }
    Ok(t)
}

fn put_doc_nodes(e: &mut Enc<'_>, xs: &[(u64, DocNode)]) {
    e.n(xs.len());
    for (k, n) in xs {
        e.u(*k);
        match *n {
            DocNode::Doc(s, t) => {
                e.u(0);
                e.n(s);
                e.n(t);
            }
            DocNode::Ini(k) => {
                e.u(1);
                e.n(k);
            }
        }
    }
}

fn get_doc_nodes(d: &mut Dec<'_>) -> R<Vec<(u64, DocNode)>> {
    let n = d.n()?;
    (0..n)
        .map(|_| {
            let k = d.u()?;
            Ok(match d.u()? {
                0 => (k, DocNode::Doc(d.z()?, d.z()?)),
                1 => (k, DocNode::Ini(d.z()?)),
                _ => return Err("a document node of unknown kind".to_owned()),
            })
        })
        .collect()
}

fn same_doc_nodes(a: &[(u64, DocNode)], b: &[(u64, DocNode)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.0 == y.0
                && match (&x.1, &y.1) {
                    (DocNode::Doc(s, e), DocNode::Doc(t, f)) => s == t && e == f,
                    (DocNode::Ini(s), DocNode::Ini(t)) => s == t,
                    _ => false,
                }
        })
}

fn put_lines(e: &mut Enc<'_>, m: &FxHashMap<u64, i64>) {
    let mut xs: Vec<(&u64, &i64)> = m.iter().collect();
    xs.sort_unstable();
    e.n(xs.len());
    for (&k, &x) in xs {
        e.u(k);
        e.i(x);
    }
}

fn get_lines(d: &mut Dec<'_>) -> R<FxHashMap<u64, i64>> {
    let n = d.n()?;
    (0..n).map(|_| Ok((d.u()?, d.i()?))).collect()
}

fn put_chunks(e: &mut Enc<'_>, c: &crate::chunks::Chunks) {
    e.n(c.cs.len());
    for k in &c.cs {
        e.n(k.start);
        e.n(k.end);
        e.u(u64::from(k.line));
        e.u(u64::from(k.lines));
        e.u(k.h);
        e.u(k.id);
    }
    e.u(c.next);
}

fn get_chunks(d: &mut Dec<'_>) -> R<crate::chunks::Chunks> {
    let n = d.n()?;
    let cs = (0..n)
        .map(|_| {
            Ok(crate::chunks::Chunk {
                start: d.z()?,
                end: d.z()?,
                line: u32::try_from(d.u()?).map_err(|_| "a chunk's line")?,
                lines: u32::try_from(d.u()?).map_err(|_| "a chunk's lines")?,
                h: d.u()?,
                id: d.u()?,
            })
        })
        .collect::<R<Vec<_>>>()?;
    Ok(crate::chunks::Chunks { cs, next: d.u()? })
}

/// The bytes [`Build::save_more`] copies or hashes between two asks of
/// its `pause` (whole words).
pub const SAVE_PIECE: usize = 256 << 10;

/// Where a save under way stands ([`Save`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SavePart {
    /// The client's part k written ([`Build::put_client_part`]).
    Client(usize),
    /// The engine's graph exported ([`phitex_defuse::Engine::export_more`]).
    Engine,
    CopyEngine,
    HashClient,
    CopyClient,
    Done,
}

/// A save of a build under way ([`Build::save_start`]), made a slice at a
/// time by [`Build::save_more`]. Dropped before its end, nothing of it is
/// left.
pub struct Save {
    part: SavePart,
    /// The document file next, in the client's part of them.
    j: usize,
    /// The client's bytes, and the values they name (the engine's roots).
    client: Vec<u8>,
    roots: Vec<ValueId>,
    eng: Option<phitex_defuse::Export>,
    eng_bytes: Vec<u8>,
    out: Vec<u8>,
    /// The next byte copied or hashed.
    i: usize,
    hashing: phitex_defuse::HashBytes,
    client_ms: f64,
    engine_ms: f64,
}

impl Save {
    /// Whether the save is whole ([`Build::save_more`] gave its bytes).
    #[must_use]
    pub fn done(&self) -> bool {
        self.part == SavePart::Done
    }
}

impl Build {
    /// The build as its quiet point leaves it, as bytes
    /// ([`Build::load`] makes a build of them that stands as this one
    /// stands): the engine's graph ([`phitex_defuse::Engine::export`]) and the client's
    /// tables. Call between evaluations. Nothing of the build changes.
    #[must_use]
    pub fn save(&self) -> Vec<u8> {
        self.save_timed().0
    }

    /// [`Build::save`], with how long the client's tables and the
    /// engine's export took (ms).
    ///
    /// # Panics
    ///
    /// Never: nothing stops it ([`Build::save_until`]).
    #[must_use]
    pub fn save_timed(&self) -> (Vec<u8>, f64, f64) {
        self.save_until(&|| false).expect("a save nothing stops")
    }

    /// [`Build::save_timed`], given up once `stop` says so (asked between
    /// the client's parts, between the engine's nodes and entries,
    /// [`phitex_defuse::Engine::export_until`], and between pieces of the
    /// bytes): none then, nothing of the build changed.
    #[must_use]
    pub fn save_until(&self, stop: &dyn Fn() -> bool) -> Option<(Vec<u8>, f64, f64)> {
        if stop() {
            return None;
        }
        // (one slice, to its end unless `stop` says so: then given up)
        self.save_more(&mut self.save_start(), stop)
    }

    /// A save of the build ([`Build::save`]) to be made in slices
    /// ([`Build::save_more`]), between which the caller may do other work
    /// as long as nothing changes the build: no evaluation, no edit. (The
    /// wasm core without threads saves a slice a step, its host's requests
    /// answered between them; an edit drops the save.) The same bytes as
    /// [`Build::save`] at the end.
    #[must_use]
    pub fn save_start(&self) -> Save {
        Save {
            part: SavePart::Client(0),
            j: 0,
            client: Vec::new(),
            roots: Vec::new(),
            eng: None,
            eng_bytes: Vec::new(),
            out: Vec::new(),
            i: 0,
            hashing: phitex_defuse::HashBytes::new(HASH_SEED),
            client_ms: 0.0,
            engine_ms: 0.0,
        }
    }

    /// More of save `s` ([`Build::save_start`]): its bytes, with how long
    /// the client's tables and the engine's export took (ms, the slices'
    /// times summed), once it is whole; else none (`pause` said so: call
    /// again to go on). `pause` is asked between the client's parts and
    /// its document files, every few of the engine's nodes and entries
    /// ([`phitex_defuse::Engine::export_more`]) and between pieces of
    /// [`SAVE_PIECE`] bytes; each call writes something, whatever it says.
    ///
    /// # Panics
    ///
    /// Never: the engine's export is begun before its part.
    #[must_use]
    pub fn save_more(&self, s: &mut Save, pause: &dyn Fn() -> bool) -> Option<(Vec<u8>, f64, f64)> {
        loop {
            match s.part {
                SavePart::Client(k) => {
                    let t0 = std::time::Instant::now();
                    let mut e = Enc {
                        v: self.g.eng.values(),
                        out: std::mem::take(&mut s.client),
                        roots: std::mem::take(&mut s.roots),
                    };
                    let whole = self.put_client_part(&mut e, k, &mut s.j, pause);
                    s.client = e.out;
                    s.roots = e.roots;
                    s.client_ms += t0.elapsed().as_secs_f64() * 1e3;
                    if whole {
                        s.j = 0;
                        s.part = if k + 1 < Self::CLIENT_PARTS {
                            SavePart::Client(k + 1)
                        } else {
                            s.roots.sort_unstable();
                            s.roots.dedup();
                            s.eng = Some(self.g.eng.export_start(&s.roots));
                            s.roots = Vec::new();
                            SavePart::Engine
                        };
                    }
                    if pause() {
                        return None;
                    }
                }
                SavePart::Engine => {
                    let t0 = std::time::Instant::now();
                    let x = s.eng.as_mut().expect("the engine's export begun");
                    let eng = self.g.eng.export_more(x, pause);
                    s.engine_ms += t0.elapsed().as_secs_f64() * 1e3;
                    let eng = eng?;
                    s.eng = None;
                    let mut out = Vec::with_capacity(8 + 4 + 8 + eng.len() + 16 + s.client.len());
                    out.extend_from_slice(MAGIC);
                    out.extend_from_slice(&VERSION.to_le_bytes());
                    out.extend_from_slice(&(eng.len() as u64).to_le_bytes());
                    s.out = out;
                    s.eng_bytes = eng;
                    s.i = 0;
                    s.part = SavePart::CopyEngine;
                }
                SavePart::CopyEngine => {
                    while s.eng_bytes.len() - s.i > SAVE_PIECE {
                        s.out.extend_from_slice(&s.eng_bytes[s.i..s.i + SAVE_PIECE]);
                        s.i += SAVE_PIECE;
                        if pause() {
                            return None;
                        }
                    }
                    s.out.extend_from_slice(&s.eng_bytes[s.i..]);
                    // (copied: its memory given back)
                    s.eng_bytes = Vec::new();
                    s.out
                        .extend_from_slice(&(s.client.len() as u64).to_le_bytes());
                    s.i = 0;
                    s.part = SavePart::HashClient;
                }
                SavePart::HashClient => {
                    while s.client.len() - s.i > SAVE_PIECE {
                        s.hashing.more(&s.client[s.i..s.i + SAVE_PIECE]);
                        s.i += SAVE_PIECE;
                        if pause() {
                            return None;
                        }
                    }
                    let h = s.hashing.end(&s.client[s.i..]);
                    s.out.extend_from_slice(&h.to_le_bytes());
                    s.i = 0;
                    s.part = SavePart::CopyClient;
                }
                SavePart::CopyClient => {
                    while s.client.len() - s.i > SAVE_PIECE {
                        s.out.extend_from_slice(&s.client[s.i..s.i + SAVE_PIECE]);
                        s.i += SAVE_PIECE;
                        if pause() {
                            return None;
                        }
                    }
                    s.out.extend_from_slice(&s.client[s.i..]);
                    s.client = Vec::new();
                    s.part = SavePart::Done;
                    return Some((std::mem::take(&mut s.out), s.client_ms, s.engine_ms));
                }
                SavePart::Done => return None,
            }
        }
    }

    /// The parts of the client's tables ([`Build::put_client_part`]).
    const CLIENT_PARTS: usize = 15;

    /// Part `k` of the client's tables written, each part its mark after it
    /// but the last: whole (true), or, the document files' part, up to a
    /// file after which `pause` said so (false: called again, it goes on
    /// at file `*j`).
    #[allow(clippy::too_many_lines)]
    fn put_client_part(
        &self,
        e: &mut Enc<'_>,
        k: usize,
        j: &mut usize,
        pause: &dyn Fn() -> bool,
    ) -> bool {
        let g = &self.g;
        let o = &g.ops;
        match k {
            0 => {
                // (the build)
                e.u(u64::from(self.unit.raw()));
                e.n(self.nodes.len());
                for (k, n, src) in &self.nodes {
                    let Src::Doc(s, t) = *src;
                    e.u(*k);
                    e.u(n.raw());
                    e.n(s);
                    e.n(t);
                }
                put_chunks(e, &self.ini);
                put_lines(e, &self.dlines);
                e.u(self.dvi.raw());
                e.u(self.end.raw());
                e.u64s(&self.settled);
                e.b(self.doc);
                e.s(&self.text);
                e.b(self.canon);
                e.os(self.raw.as_deref());
                e.u(self.settles);
                e.b(self.loads_ran);
                e.b(self.evaluated);
                e.u(self.stages);
                e.u(self.stats.version);
                e.n(self.stats.live);
                // (the output file the end node made, by its bytes: a build that
                // runs nothing after its load gives it again)
                let linked = self
                    .end_file()
                    .and_then(|x| crate::assemble::linked_of(g.eng.values(), x));
                e.b(linked.is_some());
                if let Some((h, b)) = &linked {
                    e.u(*h);
                    e.by(b);
                }
                e.tag(1);
            }
            1 => {
                // (the graph's tables)
                let mut start: Vec<(Field, ValueId)> =
                    g.start_env.iter().map(|(&f, &x)| (f, x)).collect();
                start.sort_unstable_by_key(|x| x.0);
                e.fxs(&start);
                e.strs(&g.eval_errors);
                e.strs(&g.eval_warnings);
                e.strs(&g.no_fixpoint);
                e.n(g.shell_store.len());
                for (k, x) in &g.shell_store {
                    e.s(k);
                    e.os(x.as_deref());
                }
                e.fxs(&g.env_pushed);
                e.n(g.absent_pushed.len());
                for &f in &g.absent_pushed {
                    e.f(f);
                }
                let mut mp: Vec<(&u64, &Arc<str>)> = g.marks_pushed.iter().collect();
                mp.sort_unstable_by_key(|x| x.0);
                e.n(mp.len());
                for (k, s) in mp {
                    e.u(*k);
                    e.s(s);
                }
                e.ox(g.xpages_pruned);
                e.u(g.env_calls);
                e.fxs(&g.job);
                e.fxs(&g.files);
                e.n(g.reads.len());
                for (n, h) in &g.reads {
                    e.s(n);
                    e.u(*h);
                }
                for xs in [&g.queries, &g.tool_reads] {
                    e.n(xs.len());
                    for (n, b) in xs {
                        e.s(n);
                        e.b(*b);
                    }
                }
                e.n(g.absent.len());
                for &f in &g.absent {
                    e.f(f);
                }
                e.strs(&g.written);
                e.n(g.seeds.len());
                for (a, b) in &g.seeds {
                    e.s(a);
                    e.s(b);
                }
                e.by(&g.made_seeds);
                e.strs(&g.opened_last);
                {
                    let m = g.marks.0.locked();
                    e.n(m.0.len());
                    for (l, h, c) in &m.0 {
                        e.u(u64::from(*l));
                        e.u(*h);
                        e.s(c);
                    }
                    e.b(m.1);
                }
                e.n(g.wkeys_guess.len());
                for (n, ks) in &g.wkeys_guess {
                    e.s(n);
                    e.strs(ks);
                }
                e.tag(2);
            }
            2 => {
                // (the ops' tables: the texts' analyses registered)
                let reg = crate::hints::registered(&o.hints);
                e.n(reg.len());
                for (k, cs) in &reg {
                    e.x(ValueId::from_raw(*k));
                    e.n(cs.len());
                    for (s, th) in cs.iter() {
                        e.n(*s);
                        e.by(&th.encode());
                    }
                }
                e.tag(3);
            }
            3 => {
                // (the loads' records and what the build learnt of them)
                e.b(g.loads.is_some());
                if let Some(l) = &g.loads {
                    let s = l.persisted();
                    e.by(&s.recs);
                    e.strs(&s.asked);
                    e.strs(&s.changed);
                    e.n(s.now.len());
                    for (k, h) in &s.now {
                        e.s(k);
                        e.u(*h);
                    }
                    e.n(s.predicted.len());
                    for (k, rv) in &s.predicted {
                        e.s(k);
                        e.n(rv.len());
                        for (n, gl, b) in rv {
                            e.s(n);
                            e.b(*gl);
                            e.by(b);
                        }
                    }
                    for x in s.given {
                        e.u(x);
                    }
                }
                e.tag(4);
            }
            4 => {
                // (the cut document files, a part of their own each)
                {
                    let docs = o.docs.locked();
                    let mut names: Vec<&String> = docs.keys().collect();
                    names.sort();
                    if *j == 0 {
                        e.n(names.len());
                    }
                    while let Some(name) = names.get(*j) {
                        let df = docs[*name].locked();
                        e.s(name);
                        e.s(&df.path);
                        e.s(&df.text);
                        put_doc_nodes(e, &df.slices);
                        e.n(df.readers.len());
                        for r in &df.readers {
                            e.u(r.raw());
                        }
                        let mut sh: Vec<(&u64, &(ValueId, i64))> = df.shifts.iter().collect();
                        sh.sort_unstable_by_key(|x| x.0);
                        e.n(sh.len());
                        for (k, &(x, d)) in sh {
                            e.u(*k);
                            e.x(x);
                            e.i(d);
                        }
                        let mut pl: Vec<(&u64, &Vec<NodeId>)> = df.placed.iter().collect();
                        pl.sort_unstable_by_key(|x| x.0);
                        e.n(pl.len());
                        for (k, ns) in pl {
                            e.u(*k);
                            e.u64s(&ns.iter().map(|n| n.raw()).collect::<Vec<_>>());
                        }
                        e.n(df.args.len());
                        for a in &df.args {
                            e.b(a.is_some());
                            if let Some(a) = a {
                                e.n(a.len());
                                for &x in a {
                                    e.x(x);
                                }
                            }
                        }
                        put_lines(e, &df.dlines);
                        *j += 1;
                        if *j < names.len() && pause() {
                            return false;
                        }
                    }
                }
                e.tag(5);
            }
            5 => {
                {
                    let rl = o.rest_lines.locked();
                    let mut xs: Vec<(u64, ValueId, i64)> = rl
                        .iter()
                        .map(|(&x, &k)| (e.v.stable_raw(x), x, k))
                        .collect();
                    xs.sort_unstable();
                    e.n(xs.len());
                    for (_, x, k) in xs {
                        e.x(x);
                        e.i(k);
                    }
                }
                e.tag(6);
            }
            6 => {
                {
                    let syms = o.syms.snapshot();
                    e.n(syms.len());
                    for (a, b, c, n) in syms {
                        e.i(i64::from(a));
                        e.u(b);
                        e.i(i64::from(c));
                        e.b(n.is_some());
                        if let Some((x, y)) = n {
                            e.s(&x);
                            e.s(&y);
                        }
                    }
                }
                e.tag(7);
            }
            7 => {
                put_fin_table(e, &o.fin_defs.locked());
                put_fin_table(e, &o.fin_defs_start.locked());
                e.tag(8);
            }
            8 => {
                {
                    let fc = o.fin_class.locked();
                    let mut xs: Vec<(&crate::pages::FinPageKey, &crate::run::FinClass)> =
                        fc.iter().collect();
                    xs.sort_unstable_by_key(|x| *x.0);
                    e.n(xs.len());
                    for (&(a, b, c), &(h, pen, j)) in xs {
                        e.i(a);
                        e.i(b);
                        e.i(c);
                        e.x(h);
                        e.i(i64::from(pen));
                        e.i(j);
                    }
                }
                e.tag(9);
            }
            9 => {
                {
                    let cs = o
                        .counters
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let mut xs: Vec<(&&str, &Field)> = cs.iter().collect();
                    xs.sort_unstable_by_key(|x| *x.0);
                    e.n(xs.len());
                    for (n, &f) in xs {
                        e.s(n);
                        e.f(f);
                    }
                }
                e.tag(10);
            }
            10 => {
                {
                    let sl = o
                        .scope_learned
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let mut xs: Vec<(&String, &bool)> = sl.iter().collect();
                    xs.sort_unstable();
                    e.n(xs.len());
                    for (n, &b) in xs {
                        e.s(n);
                        e.b(b);
                    }
                }
                e.tag(11);
            }
            11 => {
                {
                    let rk = o.rel_keys.locked();
                    let mut xs: Vec<(&i64, &(u64, u64))> = rk.iter().collect();
                    xs.sort_unstable();
                    e.n(xs.len());
                    for (&k, &(a, b)) in xs {
                        e.i(k);
                        e.u(a);
                        e.u(b);
                    }
                }
                e.tag(12);
            }
            12 => {
                e.u(o.units_folded.load(Relaxed));
                e.u(o.siblings_pulled.load(Relaxed));
                e.u(o.give_back_guesses[0].load(Relaxed));
                e.u(o.give_back_guesses[1].load(Relaxed));
                e.u(o.edit_stall_ms.load(Relaxed));
                // (the process's pictures refused and met with more, `crate::pic`)
                let (refused, many) = crate::pic::learned();
                e.u64s(&refused);
                e.u64s(&many);
                e.tag(13);
            }
            13 => {
                // (the link's pages and read-time commands, docs/spec-persisted-graph.md
                // §2.2.4)
                crate::pdfout::save_link(e, &o.pdf_cmds);
                e.tag(14);
            }
            14 => {
                // (the pages' glyph origins, where they are kept: what their
                // writers showed, the SyncTeX file's and the viewer's source)
                e.b(o.origins.load(Relaxed));
                if o.origins.load(Relaxed) {
                    crate::origins::save_kept(e, e.v.instance(), &self.shown_ids());
                }
            }
            _ => {}
        }
        true
    }

    /// The pages' identities in the link, in order (`pdfout::SHOWN`).
    fn shown_ids(&self) -> Vec<u64> {
        let v = self.g.eng.values();
        self.g
            .eng
            .acc_value(self.unit, v.field(crate::pdfout::SHOWN))
            .and_then(|x| v.seq_to_vec(x).first().map(|&l| crate::val::list_of(v, l)))
            .unwrap_or_default()
            .into_iter()
            .map(|x| u64::from_ne_bytes(crate::val::int_of(v, x).to_ne_bytes()))
            .collect()
    }

    /// A build of the bytes [`Build::save`] wrote, in this process, with
    /// `opts` (the finder, the threads, the tools, the sender of edits)
    /// and the format of `prelude`, standing as the saved build stood:
    /// its next evaluation is an edit, which runs nothing where nothing
    /// changed. Nothing is installed before the whole is decoded and
    /// checked; with `PHITEX_GRAPH_VERIFY=1` every read of the loaded
    /// graph is asked again ([`phitex_defuse::Engine::verify_reads`]) and one failing is
    /// an error.
    ///
    /// # Errors
    ///
    /// The bytes refused (cut, a hash or the version wrong, a part out of
    /// range), the engine's graph refused ([`phitex_defuse::Engine::import`]), or the
    /// document's analysis made again not the saved one.
    #[allow(clippy::too_many_lines)]
    pub fn load(opts: Options, prelude: Option<&str>, bytes: &[u8]) -> R<(Build, LoadStats)> {
        let t0 = std::time::Instant::now();
        let mut st = LoadStats {
            bytes: bytes.len() as u64,
            ..LoadStats::default()
        };
        let mut c = 0usize;
        let mut take = |n: usize| -> R<&[u8]> {
            let e = c
                .checked_add(n)
                .filter(|&e| e <= bytes.len())
                .ok_or("the graph cut short")?;
            let s = &bytes[c..e];
            c = e;
            Ok(s)
        };
        if take(8)? != MAGIC {
            return Err("not a saved build".into());
        }
        let ver = u32::from_le_bytes(take(4)?.try_into().map_err(|_| "a version")?);
        if ver != VERSION {
            return Err(format!("a saved build of version {ver}, not {VERSION}"));
        }
        let le = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
        let n_eng = usize::try_from(le(take(8)?)).map_err(|_| "a length")?;
        let eng_bytes = take(n_eng)?;
        let n_cl = usize::try_from(le(take(8)?)).map_err(|_| "a length")?;
        let h = le(take(8)?);
        let client = take(n_cl)?;
        if phitex_defuse::hash_bytes(HASH_SEED, client) != h {
            return Err("the client's tables' hash is wrong".into());
        }
        st.engine_bytes = n_eng as u64;
        st.client_bytes = n_cl as u64;
        // (the format, as a build of it finds it)
        let (cached, _check) = prelude_env(prelude, &opts);
        let defs = prelude_defs(&cached, prelude, opts.prelude_finder());
        let mut g = Graph::new(
            opts.time_limit,
            opts.threads,
            opts.stuck_after,
            opts.memory_limit,
        );
        // (the graph first, before anything else is made in the store: its
        // values go where they were, `Values::stable_raw`)
        let t1 = std::time::Instant::now();
        let imported = g.eng.import(eng_bytes).map_err(|e| e.to_string())?;
        st.import_ms = t1.elapsed().as_secs_f64() * 1e3;
        st.values = imported.stats.values;
        st.nodes = imported.stats.nodes;
        st.records = imported.stats.records;
        st.reads = imported.stats.reads;
        Self::configure(&mut g, &opts);
        let t2 = std::time::Instant::now();
        let mut b = Self::get_client(g, opts, cached, defs, client, &mut st)?;
        st.client_ms = t2.elapsed().as_secs_f64() * 1e3;
        if crate::exec::env_flag!("PHITEX_GRAPH_VERIFY") {
            let rc = b.g.eng.verify_reads();
            st.verified = Some((rc.reads, rc.failed));
            if rc.failed > 0 {
                return Err(format!(
                    "{} of {} reads of the loaded graph do not hold",
                    rc.failed, rc.reads
                ));
            }
        }
        b.progress_offsets();
        st.total_ms = t0.elapsed().as_secs_f64() * 1e3;
        Ok((b, st))
    }

    /// The graph's settings from the options, as [`Build::with`] makes
    /// them.
    fn configure(g: &mut Graph, opts: &Options) {
        if let Some(x) = &opts.xetex {
            let _ = g.ops.xe.set(x.clone());
        }
        if let Some(n) = opts.cut_min {
            g.ops.cut_min.store(n, Relaxed);
        }
        g.ops.fin_predict.store(opts.finisher_predictions, Relaxed);
        g.ops.origins.store(opts.origins, Relaxed);
        g.ops.node_rec.store(opts.node_records, Relaxed);
        (*g.ops.out_ok.locked()).clone_from(&opts.out_name_ok);
        if let Some(s) = &opts.analyses {
            let _ = g.ops.hints.store.set(s.clone());
        }
        {
            let mut t = g.ops.tool_opts.locked();
            t.bibtex.clone_from(&opts.bibtex);
            t.mktex.clone_from(&opts.mktex);
            t.shell_hook.clone_from(&opts.shell_hook);
            t.shell_commands.clone_from(&opts.shell_commands);
        }
        g.shell_cap = opts.shell_passes;
        g.tool_finder.clone_from(&opts.tool_finder);
        if let Some(p) = &opts.progress {
            g.watch = crate::exec::Watch::new(g.threads, Some(p.clone()));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn get_client(
        mut g: Graph,
        opts: Options,
        cached: Arc<Cached>,
        defs: Defs,
        client: &[u8],
        st: &mut LoadStats,
    ) -> R<Build> {
        let v: &Values = g.eng.values();
        let mut d = Dec { v, b: client, i: 0 };
        let unit = UnitId::from_raw(u32::try_from(d.u()?).map_err(|_| "a unit")?);
        let n = d.n()?;
        let mut nodes = Vec::with_capacity(n);
        for _ in 0..n {
            let k = d.u()?;
            let id = d.node()?;
            let (s, t) = (d.z()?, d.z()?);
            nodes.push((k, id, Src::Doc(s, t)));
        }
        let ini = get_chunks(&mut d)?;
        let dlines = get_lines(&mut d)?;
        let dvi = d.node()?;
        let end = d.node()?;
        let settled = d.u64s()?;
        let doc = d.b()?;
        let text = d.s()?;
        let canon = d.b()?;
        let raw = d.os()?.map(Arc::<str>::from);
        let settles = d.u()?;
        let loads_ran = d.b()?;
        let evaluated = d.b()?;
        let stages = d.u()?;
        let version = d.u()?;
        let live = d.z()?;
        if d.b()? {
            let h = d.u()?;
            let b = d.by()?;
            crate::assemble::adopt_linked(v, h, Arc::new(b));
        }
        d.tag(1)?;
        g.start_env = d.fxs()?.into_iter().collect();
        g.eval_errors = d.strs()?;
        g.eval_warnings = d.strs()?;
        g.no_fixpoint = d.strs()?;
        let n = d.n()?;
        for _ in 0..n {
            let k = d.s()?;
            let x = d.os()?;
            g.shell_store.insert(k, x);
        }
        g.env_pushed = d.fxs()?;
        let n = d.n()?;
        g.absent_pushed = (0..n).map(|_| d.f()).collect::<R<_>>()?;
        let n = d.n()?;
        for _ in 0..n {
            let k = d.u()?;
            let s = d.s()?;
            g.marks_pushed.insert(k, Arc::from(s));
        }
        g.xpages_pruned = d.ox()?;
        g.env_calls = d.u()?;
        g.job = d.fxs()?;
        g.files = d.fxs()?;
        let n = d.n()?;
        g.reads = (0..n).map(|_| Ok((d.s()?, d.u()?))).collect::<R<_>>()?;
        let n = d.n()?;
        g.queries = (0..n).map(|_| Ok((d.s()?, d.b()?))).collect::<R<_>>()?;
        let n = d.n()?;
        g.tool_reads = (0..n).map(|_| Ok((d.s()?, d.b()?))).collect::<R<_>>()?;
        let n = d.n()?;
        g.absent = (0..n).map(|_| d.f()).collect::<R<_>>()?;
        g.written = d.strs()?;
        let n = d.n()?;
        g.seeds = (0..n).map(|_| Ok((d.s()?, d.s()?))).collect::<R<_>>()?;
        g.made_seeds = d.by()?;
        g.opened_last = d.strs()?;
        {
            let n = d.n()?;
            let ms = (0..n)
                .map(|_| {
                    Ok((
                        u32::try_from(d.u()?).map_err(|_| "a mark's line")?,
                        d.u()?,
                        Arc::<str>::from(d.s()?),
                    ))
                })
                .collect::<R<Vec<_>>>()?;
            let dirty = d.b()?;
            *g.marks.0.locked() = (ms, dirty);
        }
        let n = d.n()?;
        g.wkeys_guess = (0..n).map(|_| Ok((d.s()?, d.strs()?))).collect::<R<_>>()?;
        d.tag(2)?;
        // (the analyses registered)
        let n = d.n()?;
        let mut reg: Vec<(u64, crate::hints::Chunks)> = Vec::with_capacity(n);
        for _ in 0..n {
            let name = d.x()?;
            let k = d.n()?;
            let mut cs = Vec::with_capacity(k);
            for _ in 0..k {
                let s = d.z()?;
                let th = crate::hints::TextHints::decode(&d.by()?)
                    .ok_or("a registered analysis that does not decode")?;
                cs.push((s, Arc::new(th)));
            }
            reg.push((name.raw(), Arc::from(cs)));
        }
        d.tag(3)?;
        // (the loads' records)
        let optimistic = opts.speculation != Speculation::Off;
        if d.b()? {
            let recs = d.by()?;
            let asked = d.strs()?;
            let changed = d.strs()?;
            let n = d.n()?;
            let now = (0..n)
                .map(|_| Ok((d.s()?, d.u()?)))
                .collect::<R<Vec<_>>>()?;
            let n = d.n()?;
            let mut predicted = Vec::with_capacity(n);
            for _ in 0..n {
                let k = d.s()?;
                let m = d.n()?;
                let rv = (0..m)
                    .map(|_| Ok((d.s()?, d.b()?, d.by()?)))
                    .collect::<R<Vec<_>>>()?;
                predicted.push((k, rv));
            }
            let given = [d.u()?, d.u()?, d.u()?];
            let l = crate::pkgrec::LoadHints::new(
                crate::pkgrec::Recs::default(),
                opts.pkgrec_source.clone(),
                opts.finder.clone(),
                optimistic,
                opts.pkg_predict,
            );
            l.restore(crate::pkgrec::PkgState {
                recs,
                asked,
                changed,
                now,
                predicted,
                given,
            });
            g.loads = Some(Arc::new(l));
        }
        g.ops
            .pkg_loads
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&g.loads);
        *g.ops.defs.locked() = defs.clone();
        d.tag(4)?;
        // (the document files cut, their analyses made again)
        let ta = std::time::Instant::now();
        let n = d.n()?;
        for _ in 0..n {
            let name = d.s()?;
            let path = d.s()?;
            let ftext = d.s()?;
            let slices = get_doc_nodes(&mut d)?;
            let m = d.n()?;
            let readers = (0..m).map(|_| d.node()).collect::<R<Vec<_>>>()?;
            let m = d.n()?;
            let mut shifts = FxHashMap::default();
            for _ in 0..m {
                let k = d.u()?;
                shifts.insert(k, (d.x()?, d.i()?));
            }
            let m = d.n()?;
            let mut placed = FxHashMap::default();
            for _ in 0..m {
                let k = d.u()?;
                let ns = d.u64s()?.into_iter().map(NodeId::from_raw).collect();
                placed.insert(k, ns);
            }
            let m = d.n()?;
            let mut args = Vec::with_capacity(m);
            for _ in 0..m {
                args.push(if d.b()? {
                    let k = d.n()?;
                    Some((0..k).map(|_| d.x()).collect::<R<Vec<_>>>()?)
                } else {
                    None
                });
            }
            let fdlines = get_lines(&mut d)?;
            let mut incr = Incr::new(defs.clone());
            let again = cut_text(
                &g.ops.hints,
                v,
                &ftext,
                None,
                Some(&mut incr),
                Some(v.str(&path)),
            );
            if !same_doc_nodes(&again, &slices) {
                return Err(format!(
                    "the document file {name}'s analysis made again is not the saved one"
                ));
            }
            let df = DocFile {
                path,
                text: ftext,
                incr,
                slices,
                readers,
                shifts,
                placed,
                args,
                dlines: fdlines,
            };
            g.ops.docs.locked().insert(name, Arc::new(Mutex::new(df)));
        }
        d.tag(5)?;
        {
            let n = d.n()?;
            let mut rl = g.ops.rest_lines.locked();
            for _ in 0..n {
                let x = d.x()?;
                rl.insert(x, d.i()?);
            }
        }
        d.tag(6)?;
        {
            let n = d.n()?;
            let mut syms = Vec::with_capacity(n);
            for _ in 0..n {
                let a = d.i32()?;
                let b = d.u()?;
                let c = d.i32()?;
                let nm = if d.b()? {
                    Some((Box::<str>::from(d.s()?), Box::<str>::from(d.s()?)))
                } else {
                    None
                };
                syms.push((a, b, c, nm));
            }
            g.ops.syms.restore(syms);
        }
        d.tag(7)?;
        *g.ops.fin_defs.locked() = get_fin_table(&mut d)?;
        *g.ops.fin_defs_start.locked() = Arc::new(get_fin_table(&mut d)?);
        d.tag(8)?;
        {
            let n = d.n()?;
            let mut fc = g.ops.fin_class.locked();
            for _ in 0..n {
                let k = (d.i()?, d.i()?, d.i()?);
                let h = d.x()?;
                let pen = d.i32()?;
                let j = d.i()?;
                fc.insert(k, (h, pen, j));
            }
        }
        d.tag(9)?;
        {
            let n = d.n()?;
            let mut cs = g
                .ops
                .counters
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for _ in 0..n {
                let name = d.s()?;
                let f = d.f()?;
                cs.insert(crate::val::CsId::intern(&name).name(), f);
            }
        }
        d.tag(10)?;
        {
            let n = d.n()?;
            let mut sl = g
                .ops
                .scope_learned
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for _ in 0..n {
                let k = d.s()?;
                sl.insert(k, d.b()?);
            }
        }
        d.tag(11)?;
        {
            let n = d.n()?;
            let mut rk = g.ops.rel_keys.locked();
            for _ in 0..n {
                let k = d.i()?;
                rk.insert(k, (d.u()?, d.u()?));
            }
        }
        d.tag(12)?;
        g.ops.units_folded.store(d.u()?, Relaxed);
        g.ops.siblings_pulled.store(d.u()?, Relaxed);
        g.ops.give_back_guesses[0].store(d.u()?, Relaxed);
        g.ops.give_back_guesses[1].store(d.u()?, Relaxed);
        g.ops.edit_stall_ms.store(d.u()?, Relaxed);
        let refused = d.u64s()?;
        let many = d.u64s()?;
        crate::pic::learn(&refused, &many);
        d.tag(13)?;
        let tl = std::time::Instant::now();
        let link = crate::pdfout::load_link(&mut d)?;
        st.link_ms = tl.elapsed().as_secs_f64() * 1e3;
        d.tag(14)?;
        if d.b()? {
            crate::origins::load_kept(&mut d, v.instance())?;
        }
        if d.i != client.len() {
            return Err("bytes left after the client's tables".into());
        }
        // (the format's bindings the environment's base, as `job_env`
        // makes it: each made into the store when first asked, where the
        // saved graph has it)
        let made = Arc::new(cached.env.made());
        *g.ops
            .fmt_made
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(made.clone());
        {
            let (fmt, made) = (Arc::clone(&cached.env), Arc::clone(&made));
            let base: super::EnvBase = Arc::new(move |v, f| fmt.get_field(v, f, &made));
            g.env_base = Some(Arc::clone(&base));
            g.eng
                .set_env_provider(Box::new(move |v, f| base(v, f).map(Some)));
        }
        // (the document's analysis made again from its text: its slices
        // must be the saved nodes')
        let mut incr = Incr::new(defs);
        incr.threads = g.threads;
        if doc {
            let again = cut_text(&g.ops.hints, v, &text, None, Some(&mut incr), None);
            let saved: Vec<(u64, DocNode)> = nodes
                .iter()
                .map(|&(k, _, Src::Doc(s, e))| (k, DocNode::Doc(s, e)))
                .collect();
            if !same_doc_nodes(&again, &saved) {
                return Err("the document's analysis made again is not the saved one".into());
            }
        }
        // (the registry as saved: the analyses registered by the runs too)
        crate::hints::set_registered(&g.ops.hints, reg);
        link.install(v, &g.ops.pdf_cmds);
        // (the analyses' alone: the link's, between them, apart)
        st.analysis_ms = ta.elapsed().as_secs_f64() * 1e3 - st.link_ms;
        let edits = opts.edits.clone().unwrap_or_default();
        let stats = Stats {
            version,
            live,
            threads: g.threads,
            ..Stats::default()
        };
        Ok(Build {
            g,
            unit,
            opts,
            nodes,
            ini,
            dlines,
            dvi,
            end,
            prelude: cached,
            stats,
            edits,
            settled,
            doc,
            text,
            canon,
            raw,
            memo: Mutex::default(),
            lines: Mutex::default(),
            diag_memo: Mutex::default(),
            diags_last: Mutex::default(),
            settles,
            serial: super::next_serial(),
            incr,
            loads_ran,
            staged: false,
            date_due: None,
            evaluated,
            origins: Arc::default(),
            stages,
            spans_memo: Mutex::new(None),
        })
    }

    /// The build evaluated again with nothing changed (a check: a build
    /// loaded runs nothing, `crate::api::persist`): the nodes it ran.
    pub fn evaluate_unchanged(&mut self) -> u64 {
        self.staged = true;
        self.settle();
        self.stats.nodes_run
    }

    /// The output file's node run again with its text (its product
    /// compared as any run's): what its writer read outside the
    /// environment moved (XeTeX's PDF writer's files, a picture or a font
    /// it embeds, as a loaded graph finds them, `phitex_live::graph`),
    /// for the next evaluation.
    pub fn relink(&mut self) {
        let args = self
            .g
            .eng
            .node_info(self.dvi)
            .map(|i| i.args)
            .unwrap_or_default();
        self.g.eng.replace(self.dvi, self.g.dvi, args);
        self.staged = true;
    }

    /// [`Build::reask_environment`]'s tools' files: each the build holds
    /// (pushed as its own, so no stat key of the session's keys it and the
    /// provider's re-ask of its answers does not reach it) asked of the
    /// provider again, by its search; one that moved, came or went pushed,
    /// its readers re-asked.
    fn reask_tools(&mut self, out: &mut EnvReask) {
        let tools: Vec<(Field, Option<ValueId>)> = {
            let v = self.g.eng.values();
            let is_tool = |f: Field| v.with_str(f.0, |n| n.starts_with("tfile:")) == Some(true);
            self.g
                .files
                .iter()
                .map(|&(f, x)| (f, Some(x)))
                .chain(self.g.absent.iter().map(|&f| (f, None)))
                .filter(|&(f, _)| is_tool(f))
                .collect()
        };
        let mut moved = false;
        for (f, held) in tools {
            out.asked += 1;
            let now = self.g.eng.ask_provider(f);
            if now == held {
                continue;
            }
            out.moved
                .push(crate::val::field_name(self.g.eng.values(), f));
            self.g.files.retain(|&(h, _)| h != f);
            self.g.absent.retain(|&h| h != f);
            match now {
                Some(x) => self.g.files.push((f, x)),
                None => self.g.absent.push(f),
            }
            moved = true;
        }
        if moved {
            let env = self.g.environment(self.opts.fixpoint);
            self.g.push_env_as(env, true);
            self.staged = true;
        }
    }

    /// A loaded build's environment as this process gives it
    /// (docs/spec-persisted-graph.md §3.3), what moved staged as an edit's
    /// change is, for the next evaluation:
    /// - the job's bindings made again from the options (the date where
    ///   `SOURCE_DATE_EPOCH` gives none, the job's name, the TCX);
    /// - every answer the provider made of something outside the format
    ///   and the code ([`Graph::outside_answer`]) asked of it again
    ///   ([`phitex_defuse::Engine::reask_provided`]), a tool's file and a
    ///   cut file's lines among them, but the files the build holds by
    ///   their names (the session's manifest: their stat keys, their
    ///   names resolved again, [`Build::stage_file`] where one moved).
    ///   The rest, the format's (its bindings, the fields it does not
    ///   bind) and the code's, are keyed by the format's key and the
    ///   engine's identity.
    ///
    /// Call between [`Build::load`] and the first evaluation.
    pub fn reask_environment(&mut self) -> EnvReask {
        let mut out = EnvReask::default();
        // (the job's bindings. Its date alone moved (this process's now,
        // not the saved build's): kept until something else makes the
        // build run, as latexmk leaves a PDF whose sources did not change
        // as it is, its date the run's that made it; then the date is
        // this process's, its readers each re-asked at its own resolution
        // (`\time` the minute, `\day` the day, the PDF's date the second):
        // [`Build::date_due`])
        if let Some(base) = self.g.env_base.clone() {
            let v = self.g.eng.values();
            let fresh = super::by_field(super::job_sets(v, &base, &self.opts));
            if fresh != self.g.job {
                let moved: Vec<String> = fresh
                    .iter()
                    .filter(|e| !self.g.job.contains(e))
                    .map(|&(f, _)| crate::val::field_name(v, f))
                    .collect();
                if moved
                    .iter()
                    .all(|n| super::DATE_FIELDS.contains(&n.as_str()))
                {
                    self.date_due = Some(fresh);
                } else {
                    out.moved.extend(moved);
                    self.g.job = fresh;
                    let env = self.g.environment(self.opts.fixpoint);
                    self.g.push_env_as(env, true);
                    self.staged = true;
                }
            }
        }
        // (the provider as an evaluation installs it)
        let finder = self.opts.finder.clone();
        self.g.provide(&finder);
        self.reask_tools(&mut out);
        let v = self.g.eng.values();
        let held: phitex_defuse::FxHashSet<Field> = self
            .g
            .files
            .iter()
            .map(|&(f, _)| f)
            .chain(self.g.absent.iter().copied())
            .filter(|&f| v.with_str(f.0, |n| !n.starts_with("tfile:")) == Some(true))
            .collect();
        let (asked, moved) = self.g.eng.reask_provided(&|f| {
            held.contains(&f) || v.with_str(f.0, Graph::outside_answer) != Some(true)
        });
        out.asked += asked;
        out.moved
            .extend(moved.iter().map(|&(f, _)| crate::val::field_name(v, f)));
        // (a moved field the client holds, a tool's file: its table and
        // what it last pushed take the new answer, so the next push finds
        // nothing changed in it)
        for &(f, x) in &moved {
            let g = &mut self.g;
            g.files.retain(|&(h, _)| h != f);
            g.absent.retain(|&h| h != f);
            match x {
                Some(x) => g.files.push((f, x)),
                None => g.absent.push(f),
            }
            match (
                g.env_pushed
                    .binary_search_by_key(&f.0.raw(), |e| e.0.0.raw()),
                x,
            ) {
                (Ok(i), Some(x)) => g.env_pushed[i].1 = x,
                (Ok(i), None) => {
                    g.env_pushed.remove(i);
                }
                (Err(i), Some(x)) => g.env_pushed.insert(i, (f, x)),
                (Err(_), None) => {}
            }
            if x.is_none() {
                if let Err(i) = g
                    .absent_pushed
                    .binary_search_by_key(&f.0.raw(), |h| h.0.raw())
                {
                    g.absent_pushed.insert(i, f);
                }
            } else {
                g.absent_pushed.retain(|&h| h != f);
            }
        }
        if !moved.is_empty() {
            self.staged = true;
        }
        out.date_kept = self.date_due.is_some();
        out
    }
}

/// What [`Build::reask_environment`] found: the provider's answers asked
/// again, and the fields that moved (the job's bindings' and the
/// provider's), by name.
#[derive(Debug, Default, Clone)]
pub struct EnvReask {
    pub asked: usize,
    pub moved: Vec<String>,
    /// Whether only the job's date moved: the saved build's date kept
    /// until the build next runs ([`Build::date_due`]).
    pub date_kept: bool,
}
