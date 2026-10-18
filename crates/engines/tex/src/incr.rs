//! A document's text kept as paragraph units ([`phitex_source::Source`]
//! cut by [`Paragraphs`]), with the hint analysis cached per chunk: a run
//! of units analysed together (one unit, unless a conditional, a
//! `\makeatletter` or `\ExplSyntaxOn` or a `\catcode` stays open at its
//! end, which the analysis of a slice must see whole).
//!
//! A chunk's analysis depends on its text and on the definitions in
//! force before it: the format's and those of the earlier chunks that
//! define anything. The latter are named by a key, the hash chain of the
//! digests of what the defining chunks before it define
//! ([`Definitions::digest`]): an edit inside a chunk that defines the
//! same things analyses that chunk again and keeps every analysis after
//! it (spec-redesign §7.11: the hint pass over the changed units only). An edit splices the units
//! ([`Source::edit`], from the common prefix and suffix of the old and new
//! texts); a chunk whose first unit kept its identity, whose units are
//! the same and whose key is the same keeps its analysis, the others are
//! analysed again.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use phitex_source::pass::{Definitions, FileKind};
use phitex_source::{Paragraphs, Source, UnitId};

use crate::hints::{Defs, TextHints};

/// A chunk's analysis, under the key of the definitions before it.
struct Entry {
    units: Vec<UnitId>,
    /// Its length in bytes.
    len: usize,
    key: u64,
    th: Arc<TextHints>,
    /// Its slices' keys ([`Chunk::keys`]).
    keys: Arc<[u64]>,
    /// The definitions after it (its key and set): the incoming ones
    /// when it defines nothing.
    out: (u64, Defs),
    /// Whether nothing stays open at its end ([`Open::closed`]).
    closed: bool,
    /// The digest of its own definitions ([`Definitions::digest`]), or
    /// `None` when it defines nothing: the key of the definitions after
    /// it chains this, not its text.
    own: Option<u64>,
}

/// One chunk of the document as analysed: where it begins in the text,
/// and its analysis (slices and hints relative to that start).
pub(crate) struct Chunk {
    pub start: usize,
    pub th: Arc<TextHints>,
    /// Its slices' keys (`api::key`), a slice at the chunk's start
    /// taken as beginning a line: made when the chunk is analysed.
    pub keys: Arc<[u64]>,
}

/// The document's units and their chunks' analyses.
pub(crate) struct Incr {
    src: Source,
    text: String,
    /// The format's definitions.
    defs: Defs,
    /// The chunks' analyses, in the text's order (parallel to `chunks`).
    cache: Vec<Entry>,
    /// The chunks of the text, in order.
    pub chunks: Vec<Chunk>,
    /// How many chunks the last text analysed again.
    pub analysed: usize,
    /// File ids for the definitions of the document's chunks.
    next_file: u32,
    /// The workers its chunks are analysed on (`crate::exec::Coarse`).
    pub threads: usize,
}

impl Incr {
    pub(crate) fn new(defs: Defs) -> Incr {
        Incr {
            src: Source::with_split("", Arc::new(Paragraphs)),
            text: String::new(),
            defs,
            cache: Vec::new(),
            chunks: Vec::new(),
            analysed: 0,
            next_file: 2_000_000_000,
            threads: 1,
        }
    }

    /// The document's slices, in the whole text's bytes.
    #[cfg(test)]
    pub(crate) fn slices(&self) -> Vec<(usize, usize)> {
        self.chunks
            .iter()
            .flat_map(|c| {
                c.th.slices
                    .iter()
                    .map(move |&(s, e)| (c.start + s, c.start + e))
            })
            .collect()
    }

    /// The chunk holding byte `s` of the text, and `s` in it.
    pub(crate) fn chunk_at(&self, s: usize) -> (&Chunk, usize) {
        let k = self.chunks.partition_point(|c| c.start <= s).max(1) - 1;
        let c = &self.chunks[k];
        (c, s - c.start)
    }

    /// The document is now `text`: the units are spliced and the chunks
    /// analysed again where their text or incoming definitions changed.
    pub(crate) fn set_text(&mut self, text: &str) {
        let t0 = std::time::Instant::now();
        if text != self.text {
            self.splice(text);
            text.clone_into(&mut self.text);
        } else if !self.chunks.is_empty() || text.is_empty() {
            self.analysed = 0;
            return;
        }
        let t1 = t0.elapsed();
        crate::timeline::note("incr: the units spliced (texts compared, copied)", t1);
        self.walk();
        if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
            eprintln!(
                "incr: units spliced {t1:?}, walked {:?}",
                t0.elapsed().saturating_sub(t1)
            );
        }
    }

    /// [`Incr::set_text`] where the text changed in one range only,
    /// `(start, old end, new end)` (its bytes before and after the same):
    /// the old text not compared, edited in place.
    pub(crate) fn set_text_edit(&mut self, text: &str, (a, b, c): (usize, usize, usize)) {
        let t0 = std::time::Instant::now();
        if a == b && a == c && text.len() == self.text.len() {
            if !self.chunks.is_empty() || text.is_empty() {
                self.analysed = 0;
                return;
            }
        } else if self.text.is_empty() || self.src.start().is_none() || b > self.text.len() {
            return self.set_text(text);
        } else {
            let (Some(from), Some(to)) = (self.src.at_byte(a), self.src.at_byte(b)) else {
                return self.set_text(text);
            };
            let splice = self.src.edit(from, to, &text[a..c]);
            self.text.replace_range(a..b, &text[a..c]);
            crate::timeline::note("incr: the units spliced (in place)", t0.elapsed());
            // (an edit inside one chunk that keeps its bounds and what it
            // defines: that chunk analysed again, the others kept where
            // they are, no walk over the text)
            if self.walk_one(&splice, a, b, c) {
                crate::timeline::note("incr: one chunk analysed again", t0.elapsed());
                return;
            }
        }
        self.walk();
    }

    /// [`Incr::walk`] for an edit of the old bytes `a..b` (now `a..c`)
    /// whose spliced units (`splice`) are all in one chunk: that chunk's
    /// bounds scanned again, its own definitions made again, and, where
    /// it still ends at its last unit and defines what it defined, it is
    /// analysed again under the definitions before it; the chunks after
    /// it move by the length's change. `false`: the edit reaches another
    /// chunk, or the chunk's bounds or definitions changed (a whole walk).
    fn walk_one(&mut self, splice: &phitex_source::Splice, a: usize, b: usize, c: usize) -> bool {
        if self.chunks.len() != self.cache.len() || self.chunks.is_empty() {
            return false;
        }
        let k = self.chunks.partition_point(|ch| ch.start <= a).max(1) - 1;
        let (start, len) = (self.chunks[k].start, self.cache[k].len);
        if b >= start + len {
            return false;
        }
        // (the removed units are a run of the chunk's)
        let units = &self.cache[k].units;
        let Some(i) = splice
            .removed
            .first()
            .and_then(|r| units.iter().position(|u| u == r))
        else {
            return false;
        };
        let n = splice.removed.len();
        if i + n > units.len() || units[i..i + n] != splice.removed[..] {
            return false;
        }
        let mut new_units = units[..i].to_vec();
        new_units.extend_from_slice(&splice.inserted);
        new_units.extend_from_slice(&units[i + n..]);
        if new_units.is_empty() || new_units.iter().any(|&u| !self.src.contains(u)) {
            return false;
        }
        // (its bounds: closed first at its last unit, or open at the
        // text's end as before)
        let last = k + 1 == self.chunks.len();
        let mut st = Open::default();
        for (j, &u) in new_units.iter().enumerate() {
            st.scan(self.src.text(u));
            if st.closed() && j + 1 < new_units.len() {
                return false;
            }
        }
        let closed = st.closed();
        if !closed && !last {
            return false;
        }
        let new_len = len + c - b;
        let t = &self.text[start..start + new_len];
        let t1 = std::time::Instant::now();
        let own = crate::hints::own_definitions(t, FileKind::Document);
        crate::timeline::note("incr: the chunk's own definitions", t1.elapsed());
        let digest = crate::hints::defines_any(&own).then(|| own.digest());
        if digest != self.cache[k].own {
            return false;
        }
        let defs = if k == 0 {
            self.defs.clone()
        } else {
            self.cache[k - 1].out.1.clone()
        };
        let (th, keys) = analyse(t, &defs, Some(own));
        let e = &mut self.cache[k];
        e.units = new_units;
        e.len = new_len;
        e.th = th.clone();
        e.keys = keys.clone();
        e.closed = closed;
        self.chunks[k] = Chunk { start, th, keys };
        if c != b {
            for ch in &mut self.chunks[k + 1..] {
                ch.start = ch.start + c - b;
            }
        }
        self.analysed = 1;
        true
    }

    fn splice(&mut self, text: &str) {
        let old = &self.text;
        if old.is_empty() || self.src.start().is_none() {
            self.src = Source::with_split(text, Arc::new(Paragraphs));
            return;
        }
        let (a, b) = (old.as_bytes(), text.as_bytes());
        let mut p = common_prefix(a, b);
        while !old.is_char_boundary(p) || !text.is_char_boundary(p) {
            p -= 1;
        }
        let max = a.len().min(b.len()) - p;
        let mut s = common_suffix(&a[a.len() - max..], &b[b.len() - max..]);
        while !old.is_char_boundary(a.len() - s) || !text.is_char_boundary(b.len() - s) {
            s -= 1;
        }
        let (Some(from), Some(to)) = (self.src.at_byte(p), self.src.at_byte(a.len() - s)) else {
            self.src = Source::with_split(text, Arc::new(Paragraphs));
            return;
        };
        let _ = self.src.edit(from, to, &text[p..b.len() - s]);
    }

    /// The chunks of the units `ids`, with the cached analyses (`cache`,
    /// the last walk's in order) of the chunks whose units are the same.
    /// The cache is gone through in order beside the units: a chunk of
    /// the units kept is found where the last walk had it, past those of
    /// units the edit removed, without a search.
    fn parts(&self, ids: &[UnitId], cache: Vec<Entry>) -> Vec<Part> {
        let mut parts: Vec<Part> = Vec::with_capacity(cache.len() + 1);
        let mut old: Vec<Option<Entry>> = cache.into_iter().map(Some).collect();
        let mut oj = 0usize;
        let (mut i, mut off) = (0, 0);
        while i < ids.len() {
            // (the cached chunk that would begin at unit `i`: the next one
            // in order, those of removed units or of units before `i`
            // passed)
            while let Some(Some(e)) = old.get(oj) {
                let u0 = e.units[0];
                if u0 == ids[i] {
                    break;
                }
                if self.src.contains(u0) && self.src.index(u0) > i {
                    break;
                }
                oj += 1;
            }
            // (a cached chunk of the same first unit is what a scan
            // finds when its units follow and it closed or ends the text)
            let cached = match old.get_mut(oj) {
                Some(slot)
                    if slot.as_ref().is_some_and(|e| {
                        e.units[0] == ids[i]
                            && ids[i..].starts_with(&e.units)
                            && (e.closed || i + e.units.len() == ids.len())
                    }) =>
                {
                    oj += 1;
                    slot.take()
                }
                _ => None,
            };
            let (j, closed) = if let Some(e) = &cached {
                (i + e.units.len(), e.closed)
            } else {
                let mut st = Open::default();
                let mut j = i;
                loop {
                    st.scan(self.src.text(ids[j]));
                    j += 1;
                    if st.closed() || j == ids.len() {
                        break;
                    }
                }
                (j, st.closed())
            };
            let len: usize = match &cached {
                Some(e) => e.len,
                None => ids[i..j].iter().map(|&u| self.src.text(u).len()).sum(),
            };
            parts.push(Part {
                units: i..j,
                start: off,
                len,
                closed,
                cached,
            });
            off += len;
            i = j;
        }
        debug_assert_eq!(off, self.text.len(), "the units are the text");
        parts
    }

    /// The definitions before each chunk of `parts` (key and set) and
    /// after it, in order; the chunks to analyse again (the cached ones
    /// under other definitions dropped). The chunks are of `text`, after
    /// the format's definitions `base`; a chunk that defines anything
    /// (whether one not cached does: `defines`) takes the next file id.
    #[allow(clippy::type_complexity)]
    fn chain(
        text: &str,
        base: &Defs,
        next_file: &mut u32,
        parts: &mut [Part],
        own_digest: &dyn Fn(usize) -> Option<u64>,
    ) -> (
        Vec<(u64, Defs)>,
        Vec<usize>,
        Vec<Option<(u64, Defs)>>,
        Vec<Option<u64>>,
    ) {
        // the definitions before each chunk, in order: a cached chunk
        // under the same ones keeps its analysis
        let mut ins: Vec<(u64, Defs)> = Vec::with_capacity(parts.len());
        let mut todo: Vec<usize> = Vec::new();
        let mut outs: Vec<Option<(u64, Defs)>> = Vec::with_capacity(parts.len());
        let mut owns: Vec<Option<u64>> = Vec::with_capacity(parts.len());
        let (mut key, mut defs) = (0u64, base.clone());
        for (k, p) in parts.iter_mut().enumerate() {
            ins.push((key, defs.clone()));
            // (what it defines is its text's alone: a cached analysis
            // under other definitions says it too)
            let cached_own = p.cached.as_ref().map(|e| e.own);
            if p.cached.as_ref().is_some_and(|e| e.key != key) {
                p.cached = None;
            }
            let (out, own) = if let Some(e) = &p.cached {
                (e.out.clone(), e.own)
            } else {
                todo.push(k);
                let own = cached_own.unwrap_or_else(|| own_digest(k));
                if let Some(d_own) = own {
                    let t = &text[p.start..p.start + p.len];
                    let mut d: Definitions = (*defs).clone();
                    d.harvest(t, FileKind::Document, *next_file);
                    *next_file = next_file.wrapping_add(1);
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut h);
                    d_own.hash(&mut h);
                    ((h.finish(), Arc::new(d)), own)
                } else {
                    ((key, defs.clone()), None)
                }
            };
            (key, defs) = (out.0, out.1.clone());
            outs.push(Some(out));
            owns.push(own);
        }
        (ins, todo, outs, owns)
    }

    /// The chunks of the units, analysed where their text or incoming
    /// definitions changed. In three passes: the chunks' bounds (what a
    /// unit leaves open, the text alone) and, for the chunks not cached,
    /// their own definitions, on the pool; the definitions in force
    /// before each chunk, a chain through the chunks that define
    /// anything (in order: each adds to the earlier ones'); the chunks'
    /// analyses under them, on the pool.
    fn walk(&mut self) {
        let t0 = std::time::Instant::now();
        let ids: Vec<UnitId> = self.src.units().collect();
        let cache = std::mem::take(&mut self.cache);
        let mut parts = self.parts(&ids, cache);
        let t_parts = t0.elapsed();
        let text = self.text.as_str();
        let chunk_text = |p: &Part| &text[p.start..p.start + p.len];
        let threads = self.threads;
        // the own definitions of the chunks not cached (whether they
        // define anything: what a cached one's analysis says)
        let fresh: Vec<usize> = (0..parts.len())
            .filter(|&k| parts[k].cached.is_none())
            .collect();
        // (on the executor, spec-final §5.1: no threads outside it)
        let mut ex = crate::exec::Coarse(crate::exec::Threads::new(threads));
        let owns: Vec<Mutex<Option<Definitions>>> =
            crate::exec::map_on(&mut ex, fresh.len(), &|x| {
                Mutex::new(Some(crate::hints::own_definitions(
                    chunk_text(&parts[fresh[x]]),
                    FileKind::Document,
                )))
            });
        let t_owns = t0.elapsed();
        let mut own_of: Vec<Option<usize>> = vec![None; parts.len()];
        for (x, &k) in fresh.iter().enumerate() {
            own_of[k] = Some(x);
        }
        let (ins, todo, mut outs, own_digests) =
            Self::chain(text, &self.defs, &mut self.next_file, &mut parts, &|k| {
                let x = own_of[k]?;
                owns[x]
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .filter(|d| crate::hints::defines_any(d))
                    .map(Definitions::digest)
            });
        let t_chain = t0.elapsed();
        // the analyses of the chunks not kept
        let analyse_todo = |x: usize| {
            let k = todo[x];
            let own = own_of[k].and_then(|o| {
                owns[o]
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
            });
            analyse(chunk_text(&parts[k]), &ins[k].1, own)
        };
        // (the analyses' summed time, for `PHITEX_TEX_TIME`)
        let busy = std::sync::atomic::AtomicU64::new(0);
        let made: Vec<(Arc<TextHints>, Arc<[u64]>)> =
            crate::exec::map_on(&mut ex, todo.len(), &|x| {
                let t = std::time::Instant::now();
                let r = analyse_todo(x);
                busy.fetch_add(
                    u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX),
                    std::sync::atomic::Ordering::Relaxed,
                );
                r
            });
        let t_made = t0.elapsed();
        let analysed = todo.len();
        let mut made = made.into_iter();
        let mut chunks = Vec::with_capacity(parts.len());
        for (k, p) in parts.into_iter().enumerate() {
            let e = if let Some(e) = p.cached {
                e
            } else {
                let (th, keys) = made.next().expect("every chunk analysed");
                Entry {
                    units: ids[p.units.clone()].to_vec(),
                    len: p.len,
                    key: ins[k].0,
                    th,
                    keys,
                    out: outs[k].take().expect("the definitions after it"),
                    closed: p.closed,
                    own: own_digests[k],
                }
            };
            chunks.push(Chunk {
                start: p.start,
                th: e.th.clone(),
                keys: e.keys.clone(),
            });
            self.cache.push(e);
        }
        self.chunks = chunks;
        self.analysed = analysed;
        walk_note(
            self.chunks.len(),
            analysed,
            [t_parts, t_owns, t_chain, t_made, t0.elapsed()],
            std::time::Duration::from_nanos(busy.load(std::sync::atomic::Ordering::Relaxed)),
        );
    }
}

/// `PHITEX_TEX_TIME`: a walk's chunks, those analysed, and its phases'
/// ends `t` (bounds, own definitions, chain, analyses, assembled), the
/// analyses' summed time `busy`.
fn walk_note(
    chunks: usize,
    analysed: usize,
    t: [std::time::Duration; 5],
    busy: std::time::Duration,
) {
    {
        use crate::timeline::note;
        let d = |k: usize| t[k].saturating_sub(t[k - 1]);
        note(&format!("incr walk: {chunks} chunks' bounds"), t[0]);
        note("incr walk: own definitions", d(1));
        note("incr walk: the chain of definitions", d(2));
        note(&format!("incr walk: {analysed} chunks analysed"), d(3));
        note("incr walk: assembled", d(4));
    }
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        let d = |k: usize| t[k].saturating_sub(t[k - 1]);
        eprintln!(
            "incr walk: {chunks} chunks ({analysed} analysed), bounds {:?}, own definitions {:?}, chain {:?}, analyses {:?} (busy {busy:?}), assembled {:?}",
            t[0],
            d(1),
            d(2),
            d(3),
            d(4)
        );
    }
}

/// A chunk's analysis under the definitions `defs` before it (given its
/// own, [`crate::hints::own_definitions`], when harvested already): its
/// hints, its slices' keys (a slice at its start taken as beginning a
/// line).
fn analyse(t: &str, defs: &Defs, own: Option<Definitions>) -> (Arc<TextHints>, Arc<[u64]>) {
    let th = crate::hints::text_hints_with(t, FileKind::Document, defs, own);
    let keys = th
        .slices
        .iter()
        .map(|&(s, e)| {
            let starts = s == 0 || t.as_bytes()[s - 1] == b'\n';
            crate::api::key(&t[s..e], starts)
        })
        .collect();
    (th, keys)
}

/// A chunk of the units being walked: its units, where it begins and its
/// length in bytes, whether nothing stays open at its end, and its cached
/// analysis while that may be kept.
struct Part {
    units: std::ops::Range<usize>,
    start: usize,
    len: usize,
    closed: bool,
    cached: Option<Entry>,
}

/// The length of the prefix `a` and `b` share: block by block (a
/// memcmp each), then byte by byte in the first block that differs.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const B: usize = 4096;
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + B <= n && a[i..i + B] == b[i..i + B] {
        i += B;
    }
    i + a[i..n]
        .iter()
        .zip(&b[i..n])
        .take_while(|(x, y)| x == y)
        .count()
}

/// The length of the suffix `a` and `b` share ([`common_prefix`] from
/// the end).
fn common_suffix(a: &[u8], b: &[u8]) -> usize {
    const B: usize = 4096;
    let n = a.len().min(b.len());
    let (a, b) = (&a[a.len() - n..], &b[b.len() - n..]);
    let mut i = 0;
    while i + B <= n && a[n - i - B..n - i] == b[n - i - B..n - i] {
        i += B;
    }
    i + a[..n - i]
        .iter()
        .rev()
        .zip(b[..n - i].iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

/// The conditionals of TeX, e-TeX and pdfTeX (a document's `\newif`s
/// are added as they are met).
const CONDS: [&str; 26] = [
    "if",
    "ifcat",
    "ifnum",
    "ifdim",
    "ifodd",
    "ifvmode",
    "ifhmode",
    "ifmmode",
    "ifinner",
    "ifvoid",
    "ifhbox",
    "ifvbox",
    "ifx",
    "ifeof",
    "iftrue",
    "iffalse",
    "ifcase",
    "ifdefined",
    "ifcsname",
    "iffontchar",
    "ifincsname",
    "ifpdfprimitive",
    "ifpdfabsnum",
    "ifpdfabsdim",
    "ifabsnum",
    "ifabsdim",
];

/// What a chunk leaves open at a unit's end, by primitives only (a
/// syntactic count, on the safe side: a conditional not closed, or a
/// `\catcode` assignment, keeps the chunk going; what a macro of a
/// format changes is the hint pass's guess, verified by the runs).
#[derive(Default)]
struct Open {
    conds: usize,
    catcode: bool,
}

impl Open {
    fn closed(&self) -> bool {
        self.conds == 0 && !self.catcode
    }

    fn scan(&mut self, t: &str) {
        // (only control sequences open or close anything)
        if !t.contains('\\') {
            return;
        }
        let b = t.as_bytes();
        let mut k = 0;
        while k < b.len() {
            match b[k] {
                b'%' => {
                    k = b[k..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(b.len(), |x| k + x);
                }
                b'\\' => {
                    let s = k + 1;
                    let mut e = s;
                    while e < b.len() && b[e].is_ascii_alphabetic() {
                        e += 1;
                    }
                    if e == s {
                        k = s + 1;
                        continue;
                    }
                    let name = &t[s..e];
                    match name {
                        "fi" => self.conds = self.conds.saturating_sub(1),
                        "catcode" => self.catcode = true,
                        _ if CONDS.contains(&name) => self.conds += 1,
                        _ => {}
                    }
                    k = e;
                }
                _ => k += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The analysis of an edit of one word in the middle of the
    /// document named by `PHITEX_INCR_DOC`: the whole text's (before)
    /// against the chunks' (after), and the slices compared.
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_edit_doc() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let b = std::fs::read(path).expect("the document");
        let text: String = b.iter().map(|&c| char::from(c)).collect();
        let mid = text.len() / 2;
        let at = mid + text[mid..].find(' ').expect("a space");
        let edited = format!("{}XYZ{}", &text[..at], &text[at..]);
        let defs = crate::hints::no_defs().clone();
        let mut inc = Incr::new(defs.clone());
        let t0 = std::time::Instant::now();
        inc.set_text(&text);
        let cold = t0.elapsed();
        let cold_chunks = inc.analysed;
        let t0 = std::time::Instant::now();
        inc.set_text(&edited);
        let warm = t0.elapsed();
        let t0 = std::time::Instant::now();
        let whole = crate::hints::text_hints(&edited, FileKind::Document, &defs);
        let before = t0.elapsed();
        eprintln!(
            "incr: cold {cold:?} ({cold_chunks} chunks), edit {warm:?} ({} of {} chunks analysed); whole text {before:?}; slices {} (whole {})",
            inc.analysed,
            inc.chunks.len(),
            inc.slices().len(),
            whole.slices.len()
        );
        let mut fresh = Incr::new(defs);
        fresh.set_text(&edited);
        assert_eq!(
            fresh.slices(),
            inc.slices(),
            "an edit gives the cold slices"
        );
    }

    /// A build of the plain document named by `PHITEX_INCR_DOC` after
    /// plain TeX, then the edit of one word in its middle: the edit's
    /// wall and the nodes it ran (`PHITEX_TEX_TIME=1` prints the text's
    /// analysis).
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_edit_build() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures");
        let read = |p: &std::path::Path| -> String {
            std::fs::read(p)
                .expect("a file")
                .iter()
                .map(|&c| char::from(c))
                .collect()
        };
        let s = read(&fx.join("dvi/plaintext.tex"));
        let at = s.find("\\output={}\n").expect("plaintext.tex's body");
        let prelude = s[..at].replace(
            "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
            &read(&fx.join("hyph/hyphen.tex")),
        );
        let kp = std::sync::Mutex::new({
            let p = std::env::var("PATH").unwrap_or_default();
            std::env::split_paths(&p)
                .find(|d| d.join("tex").is_file())
                .map(|d| phitex_kpse::Kpse::new(&d, "tex", "tex"))
        });
        let finder: crate::Finder = Arc::new(move |name: &str| {
            let fmt = match name.rsplit('.').next() {
                Some("tfm") => phitex_kpse::Format::Tfm,
                Some("map") => phitex_kpse::Format::FontMap,
                Some("pfb" | "pfa") => phitex_kpse::Format::Type1,
                Some("enc") => phitex_kpse::Format::Enc,
                Some("vf") => phitex_kpse::Format::Vf,
                _ => phitex_kpse::Format::Tex,
            };
            let mut k = kp.lock().ok()?;
            let p = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
            let p = String::from_utf8(p).ok()?;
            Some((p.clone(), std::fs::read(&p).ok()?))
        });
        let text = read(std::path::Path::new(&path));
        let mid = text.len() / 2;
        let at = mid + text[mid..].find(' ').expect("a space");
        let edited = format!("{}XYZ{}", &text[..at], &text[at..]);
        let opts = crate::Options {
            finder,
            ..crate::Options::default()
        };
        let t0 = std::time::Instant::now();
        let mut b = crate::Build::with(opts, Some(&prelude), &text);
        eprintln!(
            "build: cold {:?}, {} nodes run",
            t0.elapsed(),
            b.stats.nodes_run
        );
        let t0 = std::time::Instant::now();
        b.edit(&edited);
        eprintln!(
            "build: edit {:?}, {} nodes run",
            t0.elapsed(),
            b.stats.nodes_run
        );
    }

    /// The cold analysis of the document named by `PHITEX_INCR_DOC`
    /// only (for a profile).
    #[test]
    #[ignore = "a measurement (a document given by path)"]
    fn incr_cold_doc() {
        let Some(path) = std::env::var_os("PHITEX_INCR_DOC") else {
            return;
        };
        let b = std::fs::read(path).expect("the document");
        let text: String = b.iter().map(|&c| char::from(c)).collect();
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text(&text);
        eprintln!("incr: cold, {} chunks", inc.chunks.len());
    }

    #[test]
    fn chunks_follow_conditionals() {
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text("a\n\n\\ifx a b\n\nc\\fi\n\nd\n");
        assert_eq!(inc.chunks.len(), 3);
        inc.set_text("a\n\n\\ifx a b\n\nc\\fi\n\nd e\n");
        assert_eq!(inc.analysed, 1);
        assert_eq!(inc.chunks.len(), 3);
    }
}
