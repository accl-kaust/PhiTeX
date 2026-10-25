//! A document's text kept as paragraph units ([`phitex_source::Source`]
//! cut by [`Paragraphs`]), with the hint analysis cached per chunk: a run
//! of units analysed together (one unit, unless a conditional stays open
//! at its end, which the analysis of a slice must see whole).
//!
//! A chunk is read beginning in the regime the chunk before it ended in
//! (its catcodes, [`pass::Start`]), read from that chunk's text: a
//! catcode change ends no more than its chunk. Each character's catcode
//! a hint assumes is checked where its node runs (`hints::verify`), so
//! a regime the text before did not leave refutes the hint, never the
//! output.
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

use crate::exec::LockWait as _;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use phitex_source::pass::{self, Definitions, FileKind};
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
    /// The regime it was read beginning in, and the one it ends in
    /// ([`regime`]).
    start: Option<pass::Start>,
    end: Option<pass::Start>,
    /// The names its analysis looked up in the definitions before it,
    /// with what those gave each ([`pass::Definitions::name_digest`]):
    /// under other definitions that give each the same, the analysis
    /// is the same ([`reads_hold`]).
    reads: Reads,
}

/// An analysis's read set: each name it looked up, its mark and its
/// digest ([`pass::Definitions::name_stamp`], `name_digest`).
type Reads = Arc<[(String, u64, u64)]>;

/// Whether every name of `reads` is what it was under `defs`: an
/// analysis made under other definitions holds under these. A name
/// whose mark is the same is (no change since); else its digest says.
fn reads_hold(reads: &[(String, u64, u64)], defs: &Defs) -> bool {
    reads
        .iter()
        .all(|(n, s, d)| defs.name_stamp(n) == *s || defs.name_digest(n) == *d)
}

/// A chunk's analysis made afresh or kept ([`Incr::walk`]): its hints,
/// its slices' keys, its read set, whether it was made afresh.
type Made = (Arc<TextHints>, Arc<[u64]>, Reads, bool);

/// A document chunk's regime as kept: `None` for a document's own.
fn regime(s: &pass::Start) -> Option<pass::Start> {
    (*s != pass::Start::of(FileKind::Document)).then_some(*s)
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
        let begins = self.cache[k].start;
        let (own, end) = crate::hints::own_definitions_from(t, FileKind::Document, begins.as_ref());
        crate::timeline::note("incr: the chunk's own definitions", t1.elapsed());
        // (the regime it leaves is where the next chunk begins)
        if regime(&end) != self.cache[k].end {
            return false;
        }
        let digest = crate::hints::defines_any(&own).then(|| own.digest());
        if digest != self.cache[k].own {
            return false;
        }
        let defs = if k == 0 {
            self.defs.clone()
        } else {
            self.cache[k - 1].out.1.clone()
        };
        // (the atoms the edit did not reach keep their hints,
        // [`crate::hints::Reuse`]; the names the last analysis looked up
        // stay in its read set with those looked up now)
        let old_th = self.cache[k].th.clone();
        let old_reads = self.cache[k].reads.clone();
        let reuse = crate::hints::Reuse {
            th: &old_th,
            edit: (a - start, b - start, c - start),
        };
        let (th, keys, reads) = analyse_reusing(t, &defs, Some(own), begins.as_ref(), Some((&reuse, &old_reads)));
        let e = &mut self.cache[k];
        e.units = new_units;
        e.len = new_len;
        e.th = th.clone();
        e.keys = keys.clone();
        e.reads = reads;
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
                stale: None,
            });
            off += len;
            i = j;
        }
        debug_assert_eq!(off, self.text.len(), "the units are the text");
        parts
    }

    /// The definitions before each chunk of `parts` (key and set) and
    /// after it, in order; the chunks to analyse again (the cached ones
    /// under other definitions, or begun in another regime, dropped);
    /// each chunk's own key and the regimes it begins and ends in. The
    /// chunks are of `text`, after the format's definitions `base`. A
    /// chunk not cached has its own definitions in `owns` (read
    /// beginning in a document's regime, ending in `ends`), read again
    /// here when it begins in another; a chunk that defines anything
    /// takes the next file id.
    fn chain(
        text: &str,
        base: &Defs,
        next_file: &mut u32,
        parts: &mut [Part],
        owns: &[Mutex<Option<Definitions>>],
        ends: &[Option<pass::Start>],
    ) -> Chained {
        let n = parts.len();
        let mut c = Chained {
            ins: Vec::with_capacity(n),
            todo: Vec::new(),
            outs: Vec::with_capacity(n),
            owns: Vec::with_capacity(n),
            starts: Vec::with_capacity(n),
            ends: Vec::with_capacity(n),
        };
        let (mut key, mut defs) = (0u64, base.clone());
        let mut start: Option<pass::Start> = None;
        for (k, p) in parts.iter_mut().enumerate() {
            c.ins.push((key, defs.clone()));
            c.starts.push(start);
            // (what it defines is its text's alone, read in the regime
            // it begins in: a cached analysis under other definitions
            // says it too)
            let cached_own = p
                .cached
                .as_ref()
                .filter(|e| e.start == start)
                .map(|e| (e.own, e.end));
            // (under other definitions before it: its definitions after
            // it made again, its analysis kept where what it looked up
            // is the same, [`reads_hold`])
            if p.cached.as_ref().is_some_and(|e| e.start != start) {
                p.cached = None;
            } else if p.cached.as_ref().is_some_and(|e| e.key != key) {
                p.stale = p.cached.take();
            }
            let (out, own, end) = if let Some(e) = &p.cached {
                (e.out.clone(), e.own, e.end)
            } else {
                c.todo.push(k);
                let t = &text[p.start..p.start + p.len];
                let (own, end) = cached_own.unwrap_or_else(|| {
                    let mut slot = owns[k].locked();
                    let end = match &*slot {
                        Some(_) if start.is_none() => ends[k],
                        _ => {
                            let (d, e) = crate::hints::own_definitions_from(
                                t,
                                FileKind::Document,
                                start.as_ref(),
                            );
                            *slot = Some(d);
                            regime(&e)
                        }
                    };
                    let digest = slot
                        .as_ref()
                        .filter(|d| crate::hints::defines_any(d))
                        .map(Definitions::digest);
                    (digest, end)
                });
                if let Some(d_own) = own {
                    let t1 = std::time::Instant::now();
                    let mut d: Definitions = (*defs).clone();
                    crate::timeline::note(
                        "incr: the definitions before a chunk copied",
                        t1.elapsed(),
                    );
                    d.harvest_from(t, FileKind::Document, *next_file, start.as_ref());
                    *next_file = next_file.wrapping_add(1);
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    key.hash(&mut h);
                    d_own.hash(&mut h);
                    ((h.finish(), Arc::new(d)), own, end)
                } else {
                    ((key, defs.clone()), None, end)
                }
            };
            (key, defs) = (out.0, out.1.clone());
            c.outs.push(Some(out));
            c.owns.push(own);
            c.ends.push(end);
            start = end;
        }
        c
    }

    /// The chunks of the units, analysed where their text, incoming
    /// definitions or regime changed. In three passes: the chunks'
    /// bounds (what a unit leaves open, the text alone) and, for the
    /// chunks not cached, their own definitions, on the pool; the
    /// definitions in force before each chunk, a chain through the
    /// chunks that define anything (in order: each adds to the earlier
    /// ones'), with the regime each begins in; the chunks' analyses
    /// under them, on the pool.
    #[allow(clippy::too_many_lines)]
    fn walk(&mut self) {
        let t0 = std::time::Instant::now();
        let ids: Vec<UnitId> = self.src.units().collect();
        let cache = std::mem::take(&mut self.cache);
        let mut parts = self.parts(&ids, cache);
        let t_parts = t0.elapsed();
        let text = self.text.as_str();
        let chunk_text = |p: &Part| &text[p.start..p.start + p.len];
        let threads = self.threads;
        // the own definitions of the chunks not cached, read beginning in
        // a document's regime (whether they define anything: what a
        // cached one's analysis says)
        let fresh: Vec<usize> = (0..parts.len())
            .filter(|&k| parts[k].cached.is_none())
            .collect();
        // (on the executor, spec-final §5.1: no threads outside it)
        let mut ex = crate::exec::Coarse(threads);
        let made_owns: Vec<(Mutex<Option<Definitions>>, Option<pass::Start>)> =
            crate::exec::map_on(&mut ex, fresh.len(), &|x| {
                let (d, e) = crate::hints::own_definitions_from(
                    chunk_text(&parts[fresh[x]]),
                    FileKind::Document,
                    None,
                );
                (Mutex::new(Some(d)), regime(&e))
            });
        let t_owns = t0.elapsed();
        let mut owns: Vec<Mutex<Option<Definitions>>> =
            (0..parts.len()).map(|_| Mutex::new(None)).collect();
        let mut ends: Vec<Option<pass::Start>> = vec![None; parts.len()];
        for (x, (d, e)) in made_owns.into_iter().enumerate() {
            owns[fresh[x]] = d;
            ends[fresh[x]] = e;
        }
        let mut c = Self::chain(
            text,
            &self.defs,
            &mut self.next_file,
            &mut parts,
            &owns,
            &ends,
        );
        let t_chain = t0.elapsed();
        // the analyses of the chunks not kept
        let analyse_todo = |x: usize| {
            let k = c.todo[x];
            if let Some(e) = &parts[k].stale
                && reads_hold(&e.reads, &c.ins[k].1)
            {
                return (e.th.clone(), e.keys.clone(), e.reads.clone(), false);
            }
            let own = owns[k].locked().take();
            let (th, keys, reads) = analyse(
                chunk_text(&parts[k]),
                &c.ins[k].1,
                own,
                c.starts[k].as_ref(),
            );
            (th, keys, reads, true)
        };
        // (the analyses' summed time, for `PHITEX_TEX_TIME`)
        let busy = std::sync::atomic::AtomicU64::new(0);
        let made: Vec<Made> = crate::exec::map_on(&mut ex, c.todo.len(), &|x| {
            let t = std::time::Instant::now();
            let r = analyse_todo(x);
            busy.fetch_add(
                u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::Relaxed,
            );
            r
        });
        let t_made = t0.elapsed();
        let analysed = made.iter().filter(|m| m.3).count();
        let mut made = made.into_iter();
        let mut chunks = Vec::with_capacity(parts.len());
        for (k, p) in parts.into_iter().enumerate() {
            let e = if let Some(e) = p.cached {
                e
            } else {
                let (th, keys, reads, _) = made.next().expect("every chunk analysed");
                Entry {
                    units: ids[p.units.clone()].to_vec(),
                    len: p.len,
                    key: c.ins[k].0,
                    th,
                    keys,
                    out: c.outs[k].take().expect("the definitions after it"),
                    closed: p.closed,
                    own: c.owns[k],
                    start: c.starts[k],
                    end: c.ends[k],
                    reads,
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

/// What [`Incr::chain`] gives, by chunk: the definitions before it (key
/// and set) and after it, its own key (its own definitions' digest), the regimes it
/// begins and ends in; and the chunks to analyse again.
struct Chained {
    ins: Vec<(u64, Defs)>,
    todo: Vec<usize>,
    outs: Vec<Option<(u64, Defs)>>,
    owns: Vec<Option<u64>>,
    starts: Vec<Option<pass::Start>>,
    ends: Vec<Option<pass::Start>>,
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

/// [`pass::Definitions::name_digest`] of `n` whose mark is `stamp`: the
/// mark is unique in the process and definitions with the same mark give
/// the name the same digest, so it is hashed once per mark, not again
/// for every chunk analysed (a chunk re-analysed reads a hundred names).
fn name_digest(defs: &Defs, n: &str, stamp: u64) -> u64 {
    thread_local! {
        static MEMO: std::cell::RefCell<phitex_defuse::FxHashMap<(String, u64), u64>> =
            std::cell::RefCell::new(phitex_defuse::FxHashMap::default());
    }
    if stamp == 0 {
        return defs.name_digest(n);
    }
    let key = (n.to_owned(), stamp);
    if let Some(d) = MEMO.with(|m| m.borrow().get(&key).copied()) {
        return d;
    }
    let d = defs.name_digest(n);
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= 1 << 16 {
            m.clear();
        }
        m.insert(key, d);
    });
    d
}

/// A chunk's analysis under the definitions `defs` before it (given its
/// own, [`crate::hints::own_definitions`], when harvested already): its
/// hints, its slices' keys (a slice at its start taken as beginning a
/// line).
fn analyse(
    t: &str,
    defs: &Defs,
    own: Option<Definitions>,
    start: Option<&pass::Start>,
) -> (Arc<TextHints>, Arc<[u64]>, Reads) {
    analyse_reusing(t, defs, own, start, None)
}

/// [`analyse`] keeping what an edit did not reach of the chunk's last
/// analysis ([`crate::hints::Reuse`]), whose read set (`Reads`) the new
/// one keeps too.
fn analyse_reusing(
    t: &str,
    defs: &Defs,
    own: Option<Definitions>,
    start: Option<&pass::Start>,
    reuse: Option<(&crate::hints::Reuse<'_>, &Reads)>,
) -> (Arc<TextHints>, Arc<[u64]>, Reads) {
    let (th, mut names) = match reuse {
        Some((r, _)) => crate::hints::text_hints_reads_reusing(t, FileKind::Document, defs, own, start, r),
        None => crate::hints::text_hints_reads(t, FileKind::Document, defs, own, start),
    };
    if let Some((_, old)) = reuse {
        let mut seen: phitex_defuse::FxHashSet<&str> = names.iter().map(String::as_str).collect();
        let more: Vec<String> = old.iter().filter(|(n, _, _)| seen.insert(n.as_str())).map(|(n, _, _)| n.clone()).collect();
        names.extend(more);
    }
    let reads: Reads = names
        .into_iter()
        .map(|n| {
            let s = defs.name_stamp(&n);
            let d = name_digest(defs, &n, s);
            (n, s, d)
        })
        .collect();
    let keys = th
        .slices
        .iter()
        .map(|&(s, e)| {
            let starts = s == 0 || t.as_bytes()[s - 1] == b'\n';
            crate::api::key(&t[s..e], starts)
        })
        .collect();
    (th, keys, reads)
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
    /// Its cached analysis made under other definitions before it
    /// (the same text and regime): kept where its reads hold
    /// ([`reads_hold`]).
    stale: Option<Entry>,
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
/// syntactic count, on the safe side: a conditional not closed keeps the
/// chunk going; a catcode change does not, the next chunk beginning in
/// the regime it leaves, [`regime`]; what a macro of a format changes is
/// the hint pass's guess, verified by the runs).
#[derive(Default)]
struct Open {
    conds: usize,
}

impl Open {
    fn closed(&self) -> bool {
        self.conds == 0
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

    /// A chunk's analysis is keyed on what it looked up, not on every
    /// definition before it: an edit of a definition analyses again the
    /// defining chunk and the chunks that look its name up, and keeps
    /// the others (their hints the same as a cold walk's).
    #[test]
    fn a_definition_edit_analyses_its_readers_only() {
        let doc = |body: &str| {
            format!(
                "\\def\\a{{{body}}}\n\n\\def\\b{{y}}\n\n\\a\n\none two\n\nthree\n\n\\b four\n\nfive \\a\n\nsix\n"
            )
        };
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text(&doc("x"));
        let n = inc.chunks.len();
        assert_eq!(n, 8);
        inc.set_text(&doc("\\vskip 1pt"));
        // (the defining chunk and the two that call `\a`)
        assert_eq!(inc.analysed, 3, "of {n} chunks");
        let mut fresh = Incr::new(crate::hints::no_defs().clone());
        fresh.set_text(&doc("\\vskip 1pt"));
        assert_eq!(fresh.slices(), inc.slices());
        for (a, b) in fresh.chunks.iter().zip(&inc.chunks) {
            for &(s, _) in &a.th.slices {
                assert_eq!(
                    crate::hints::slice_hint_of(&a.th, s),
                    crate::hints::slice_hint_of(&b.th, s)
                );
            }
        }
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

    /// A catcode change ends no more than its chunk: a plain TeX text
    /// that makes `@` a letter first is chunked per paragraph, each chunk
    /// after it read beginning in the regime it left (its hints assume
    /// `@` a letter, which the runs check), and an edit in one of them
    /// analyses that one again.
    #[test]
    fn a_catcode_change_ends_its_chunk() {
        let mut inc = Incr::new(crate::hints::no_defs().clone());
        inc.set_text("\\catcode`\\@=11\n\na\n\n\\def\\x@y{b}\\x@y\n\nc\n");
        assert_eq!(inc.chunks.len(), 4);
        assert!(inc.cache[0].start.is_none());
        assert!(inc.cache[1..].iter().all(|e| e.start.is_some()));
        let h = crate::hints::Hint::decode(&crate::hints::slice_hint_of(&inc.chunks[2].th, 0));
        assert!(
            h.bound
                .as_ref()
                .is_some_and(|b| b.cats.contains(&(b'@', 11))),
            "{h:?}"
        );
        inc.set_text("\\catcode`\\@=11\n\na\n\n\\def\\x@y{b}\\x@y\n\nc d\n");
        assert_eq!(inc.analysed, 1);
        assert_eq!(inc.chunks.len(), 4);
    }
}
