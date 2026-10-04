//! The link that costs the changed chunks (DESIGN 3.8; 4.3, item 4).
//!
//! [`Splice`] keeps the last link's layout: the chunks in program order,
//! each one's pieces of each file and the marks of the objects it writes,
//! an offset tree per file (a Fenwick tree over the chunks' lengths in
//! that file), the object streams as rendered, the cross-reference
//! sections and the byte counts. A link is given the steps whose chunks
//! changed since the last one, and costs those:
//! - a chunk replaced costs its effects and `O(log n)` per file it writes;
//!   chunks inserted or removed shift the others, and the trees are made
//!   again (`O(n)`, without looking at any chunk's effects);
//! - an object stream is rendered again only when a chunk holding one of
//!   its objects changed;
//! - an object's offset is a prefix sum of its file's tree and the offset
//!   of its mark in its chunk, so the cross-reference section is rendered
//!   from the tree, and only when something before it moved;
//! - each file comes out as its pieces with its length and the first byte
//!   that changed ([`SpliceOut`], [`Splice::write_from`]), for a writer to
//!   write from there.
//!
//! The files are [`super::link`]'s, byte for byte (a debug build checks
//! each link against a full one). Virtual object numbers are not laid out
//! here: with them, [`Splice::link`] says so and the caller links in full.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{Effect, LinkError, Stream, render_objstm};
use crate::diag::Diagnostic;
use crate::host::{FileKind, WriteId};
use crate::pdf::xref::Deflate;

/// One step whose chunks changed since the last link: its key in program
/// order, and its chunks in order, each with its version (`None`: the
/// step left the build).
#[derive(Clone, Debug)]
pub struct StepChunks {
    pub step: u32,
    pub order: u64,
    pub chunks: Option<Vec<(u128, Arc<[Effect]>)>>,
}

/// A link's result: each file's length and the first of its bytes that
/// changed since the last link (`None`: none), the terminal's text and
/// the files opened, as [`super::Linked`] has them (the diagnostics, the
/// pages and the closes are [`Splice::each_diagnostic`],
/// [`Splice::pages`] and [`Splice::closed`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpliceOut {
    pub files: BTreeMap<u32, (u64, Option<u64>)>,
    pub term: Vec<u8>,
    pub opened: Vec<(WriteId, Vec<u8>, FileKind)>,
}

/// What the last link did, for the report.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpliceStats {
    /// Chunks put in (replaced or new), and taken out without a
    /// replacement.
    pub chunks_in: usize,
    pub chunks_out: usize,
    /// The chunks live after the link.
    pub chunks: usize,
    /// Whether chunks moved (inserted or removed): the trees made again.
    pub reindexed: bool,
    /// Object streams rendered, and the bytes they gave to deflate.
    pub streams: usize,
    pub stream_bytes: usize,
    /// Cross-reference sections rendered, and their entries.
    pub xrefs: usize,
    pub xref_entries: usize,
    /// The clock when the changed chunks were placed (their bytes and
    /// offsets final: an edited page's content stream is ready), when the
    /// object streams they reach were rendered, and at the end.
    pub placed_at: u64,
    pub streams_at: u64,
    pub done_at: u64,
    /// Time in deflate (object streams, cross-reference sections).
    pub deflate_ns: u64,
}

/// Prefix sums over chunk positions (1-based inside).
#[derive(Clone, Debug, Default)]
struct Fenwick {
    t: Vec<u64>,
}

impl Fenwick {
    fn build(vals: &[u64]) -> Self {
        let n = vals.len();
        let mut t = alloc::vec![0u64; n + 1];
        t[1..].copy_from_slice(vals);
        for i in 1..=n {
            let j = i + i.isolate_lowest_one();
            if j <= n {
                t[j] += t[i];
            }
        }
        Fenwick { t }
    }

    /// The sum over positions `[0, p)`.
    fn prefix(&self, p: usize) -> u64 {
        let mut i = p.min(self.t.len().saturating_sub(1));
        let mut s = 0;
        while i > 0 {
            s += self.t[i];
            i &= i - 1;
        }
        s
    }

    fn add(&mut self, p: usize, d: i64) {
        let mut i = p + 1;
        while i < self.t.len() {
            self.t[i] = self.t[i].wrapping_add_signed(d);
            i += i.isolate_lowest_one();
        }
    }

    fn total(&self) -> u64 {
        self.prefix(self.t.len().saturating_sub(1))
    }

    /// The position whose span holds byte `x` (the number of positions
    /// whose spans end at or before it): `n` past the end.
    fn find(&self, x: u64) -> usize {
        let n = self.t.len().saturating_sub(1);
        let mut pos = 0;
        let mut rem = x;
        let mut step = n.checked_next_power_of_two().unwrap_or(0);
        if step > n {
            step /= 2;
        }
        while step > 0 {
            if pos + step <= n && self.t[pos + step] <= rem {
                pos += step;
                rem -= self.t[pos];
            }
            step /= 2;
        }
        pos
    }
}

/// A piece of a file, or an object's mark, in a chunk's order.
#[derive(Clone, Copy, Debug)]
enum Item {
    /// A `Write` effect's bytes (file, effect index).
    Bytes(u32, u32),
    /// A piece rendered at the link (file, index in `owned`).
    Owned(u32, u32),
    /// Object `num` begins `ahead` bytes on (file, num, ahead).
    Mark(u32, i32, u64),
}

/// An object stream's event (file first).
#[derive(Clone, Copy, Debug)]
enum Os {
    Start(u32, i32),
    /// Bytes (effect index).
    Bytes(u32, u32),
    /// Stream `num` closes, at `level`; rendered into `owned[idx]`.
    Close(u32, i32, i32, u32),
}

/// A piece of the terminal's text.
#[derive(Clone, Copy, Debug)]
enum TermItem {
    /// A `Term` effect (its index).
    Text(u32),
    /// A byte count, rendered into `owned[idx]`.
    Length(u32),
}

/// A chunk, as laid out.
#[derive(Clone, Debug)]
struct Slot {
    step: u32,
    k: u32,
    order: u64,
    version: u128,
    fx: Arc<[Effect]>,
    items: Vec<Item>,
    /// The pieces rendered at the link: object streams, cross-reference
    /// sections, byte counts.
    owned: Vec<Vec<u8>>,
    os: Vec<Os>,
    /// The objects each rendered object stream holds, by its `owned`
    /// index (empty for other pieces).
    os_objs: Vec<Vec<i32>>,
    term: Vec<TermItem>,
    /// The byte counts: effect index, `owned` index.
    lengths: Vec<(u32, u32)>,
    /// The cross-reference sections: file, effect index, `owned` index.
    xrefs: Vec<(u32, u32, u32)>,
    /// Its diagnostics, pages, opens and closes (effect indices).
    misc: Vec<u32>,
    /// Virtual numbers (`Num` effects): a full link resolves them.
    virt: bool,
    /// Each file's bytes in this chunk, by file ([`Slot::relayout`]).
    lens: Vec<(u32, u64)>,
    /// Each object's mark: file, number, offset in this chunk's bytes of
    /// the file.
    marks: Vec<(u32, i32, u64)>,
}

fn idx(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

impl Slot {
    fn new(step: u32, k: u32, order: u64, version: u128, fx: &Arc<[Effect]>) -> Self {
        let mut s = Slot {
            step,
            k,
            order,
            version,
            fx: fx.clone(),
            items: Vec::new(),
            owned: Vec::new(),
            os: Vec::new(),
            os_objs: Vec::new(),
            term: Vec::new(),
            lengths: Vec::new(),
            xrefs: Vec::new(),
            misc: Vec::new(),
            virt: false,
            lens: Vec::new(),
            marks: Vec::new(),
        };
        for (i, e) in fx.iter().enumerate() {
            let i = idx(i);
            match e {
                Effect::Write { file, .. } => s.items.push(Item::Bytes(file.0, i)),
                Effect::Term(_) => s.term.push(TermItem::Text(i)),
                Effect::Diagnostic(_)
                | Effect::Shipping(_)
                | Effect::Close(_)
                | Effect::Open { .. } => s.misc.push(i),
                Effect::Num(..) => s.virt = true,
                // (written by the resolution of virtual numbers only; glyph
                // origins, read by `Tex::origins`)
                Effect::Origins(_)
                | Effect::ObjRef { .. }
                | Effect::ObjStmRef { .. }
                | Effect::FontLoad(_)
                | Effect::FontRef { .. }
                | Effect::ObjStmFontRef { .. }
                | Effect::StreamLength { .. }
                | Effect::Deflate { .. } => {}
                Effect::ObjStmStart { file, num } => s.os.push(Os::Start(file.0, *num)),
                Effect::ObjStmBytes { file, .. } => s.os.push(Os::Bytes(file.0, i)),
                Effect::ObjStm { file, num, level } => {
                    let o = s.own(Vec::new());
                    s.os.push(Os::Close(file.0, *num, *level, o));
                    s.items.push(Item::Owned(file.0, o));
                }
                Effect::PdfObject { file, num, ahead } => {
                    s.items.push(Item::Mark(file.0, *num, *ahead));
                }
                Effect::PdfXref { file, .. } => {
                    let o = s.own(Vec::new());
                    s.xrefs.push((file.0, i, o));
                    s.items.push(Item::Owned(file.0, o));
                }
                Effect::Length { stream, text, .. } => {
                    // (the digits assumed stand until the lengths are
                    // known: as many bytes)
                    let o = s.own(text.clone());
                    s.lengths.push((i, o));
                    match stream {
                        Stream::File(f) => s.items.push(Item::Owned(f.0, o)),
                        Stream::Term => s.term.push(TermItem::Length(o)),
                    }
                }
            }
        }
        s.relayout();
        s
    }

    fn own(&mut self, b: Vec<u8>) -> u32 {
        self.owned.push(b);
        self.os_objs.push(Vec::new());
        idx(self.owned.len() - 1)
    }

    fn bytes(&self, i: u32) -> &[u8] {
        match &self.fx[i as usize] {
            Effect::Write { bytes, .. }
            | Effect::Term(bytes)
            | Effect::ObjStmBytes { bytes, .. } => bytes,
            _ => &[],
        }
    }

    fn piece(&self, it: Item) -> Option<(u32, &[u8])> {
        match it {
            Item::Bytes(f, i) => Some((f, self.bytes(i))),
            Item::Owned(f, o) => Some((f, &self.owned[o as usize])),
            Item::Mark(..) => None,
        }
    }

    /// Each file's length in the chunk and each mark's offset, from its
    /// pieces as rendered now.
    fn relayout(&mut self) {
        let mut lens: Vec<(u32, u64)> = Vec::new();
        let mut marks = Vec::new();
        for &it in &self.items {
            if let Item::Mark(f, num, ahead) = it {
                let at = lens.iter().find(|x| x.0 == f).map_or(0, |x| x.1);
                marks.push((f, num, at + ahead));
            } else if let Some((f, b)) = self.piece(it) {
                let n = b.len() as u64;
                match lens.iter_mut().find(|x| x.0 == f) {
                    Some(x) => x.1 += n,
                    None => lens.push((f, n)),
                }
            }
        }
        lens.sort_unstable_by_key(|x| x.0);
        self.lens = lens;
        self.marks = marks;
    }

    fn len(&self, f: u32) -> u64 {
        self.lens
            .binary_search_by_key(&f, |x| x.0)
            .map_or(0, |i| self.lens[i].1)
    }

    /// The files whose object streams have events here.
    fn os_files(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .os
            .iter()
            .map(|e| match *e {
                Os::Start(f, _) | Os::Bytes(f, _) | Os::Close(f, ..) => f,
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn closes(&self, f: u32) -> bool {
        self.os
            .iter()
            .any(|e| matches!(*e, Os::Close(g, ..) if g == f))
    }
}

/// A file's offset tree, and how many chunks have pieces of it.
#[derive(Clone, Debug, Default)]
struct Tree {
    fen: Fenwick,
    count: usize,
}

/// The incremental link's state (module doc).
#[derive(Clone, Debug, Default)]
pub struct Splice {
    slots: Vec<Slot>,
    /// Each step's chunks: the first one's position and how many, by step.
    steps: BTreeMap<u32, (usize, usize)>,
    files: BTreeMap<u32, Tree>,
    /// The positions of the chunks with object-stream events, and of those
    /// that close an object stream, by file.
    os_at: BTreeMap<u32, BTreeSet<usize>>,
    closes: BTreeMap<u32, BTreeSet<usize>>,
    /// The positions of the chunks with terminal text, with diagnostics,
    /// pages, opens or closes, with byte counts, and with
    /// cross-reference sections.
    term_at: BTreeSet<usize>,
    misc_at: BTreeSet<usize>,
    len_at: BTreeSet<usize>,
    xref_at: BTreeSet<usize>,
    /// Each object's mark, by file and number: the chunk's position and
    /// the offset in it.
    marks: BTreeMap<u32, Vec<Option<(usize, u64)>>>,
    /// An object marked twice: the marks are made again from every chunk
    /// at each link (the last one counts, as in a full link).
    dup_marks: bool,
    /// Each object placed in an object stream, by file and number: the
    /// stream's number and the index in it.
    placed: BTreeMap<u32, Vec<Option<(i32, u8)>>>,
    /// The chunks with virtual numbers.
    virt: usize,
    /// A link was made, and its layout holds.
    live: bool,
    pub stats: SpliceStats,
}

/// A change that may have moved bytes of the chunk at a position, by file.
fn note(first: &mut BTreeMap<u32, usize>, f: u32, p: usize) {
    let e = first.entry(f).or_insert(p);
    *e = (*e).min(p);
}

impl Splice {
    /// Forget everything: the next link is a cold one.
    pub fn reset(&mut self) {
        *self = Splice::default();
    }

    /// The chunks, in program order: each one's step, place in its step
    /// and version (to check the layout against the build's).
    #[must_use]
    pub fn chunk_keys(&self) -> Vec<(u32, u32, u128)> {
        self.slots
            .iter()
            .map(|s| (s.step, s.k, s.version))
            .collect()
    }

    /// Link after the steps in `changes` changed (each step at most once,
    /// or its last entry counts); `keys`, when the build's keys in
    /// program order were made again, gives each step's. `Ok(None)`: the
    /// chunks have virtual object numbers, which this link does not
    /// resolve (the caller links in full; the layout stays usable).
    ///
    /// # Errors
    ///
    /// As [`super::link`].
    pub fn link(
        &mut self,
        changes: Vec<StepChunks>,
        keys: Option<&dyn Fn(u32) -> u64>,
        deflate: &mut Deflate<'_>,
        clock: &dyn Fn() -> u64,
    ) -> Result<Option<SpliceOut>, LinkError> {
        self.stats = SpliceStats::default();
        if let Some(keys) = keys {
            for s in &mut self.slots {
                s.order = keys(s.step);
            }
        }
        let mut by_step: BTreeMap<u32, StepChunks> = BTreeMap::new();
        for c in changes {
            by_step.insert(c.step, c);
        }
        // (where each file's bytes may have changed, by chunk position;
        // the chunks put in; the places chunks were taken out)
        let mut first: BTreeMap<u32, usize> = BTreeMap::new();
        let mut dirty: BTreeSet<usize> = BTreeSet::new();
        let mut cuts: Vec<(usize, Vec<u32>)> = Vec::new();
        let cold = !self.live;
        let structural = cold
            || by_step.values().any(|c| {
                let old = self.steps.get(&c.step).map_or(0, |x| x.1);
                let new = c.chunks.as_ref().map_or(0, Vec::len);
                old != new
            });
        if structural {
            self.splice(&by_step, &mut first, &mut dirty, &mut cuts);
        } else {
            for c in by_step.into_values() {
                let Some(&(pos, _)) = self.steps.get(&c.step) else {
                    continue;
                };
                for (k, (v, fx)) in c.chunks.unwrap_or_default().into_iter().enumerate() {
                    let p = pos + k;
                    if self.slots[p].version == v {
                        continue;
                    }
                    let new = Slot::new(c.step, idx(k), c.order, v, &fx);
                    // (the old chunk's object-stream events: the stream
                    // that held them closes at or after the next chunk)
                    let files = self.replace(p, new, &mut first);
                    cuts.push((p + 1, files));
                    dirty.insert(p);
                }
            }
        }
        if cold {
            // (everything laid out anew: every chunk put in)
            dirty = (0..self.slots.len()).collect();
            self.placed.clear();
            let files: Vec<u32> = self.files.keys().copied().collect();
            for f in files {
                first.insert(f, 0);
            }
        }
        self.stats.chunks_in = dirty.len();
        self.stats.chunks = self.slots.len();
        if self.virt > 0 {
            self.live = false;
            return Ok(None);
        }
        if self.dup_marks {
            self.remark();
        }
        self.stats.placed_at = clock();
        // 1. The object streams a change reached, rendered again.
        self.streams(&dirty, &cuts, &mut first, deflate);
        self.stats.streams_at = clock();
        self.stats.deflate_ns += self.stats.streams_at - self.stats.placed_at;
        // 2. The cross-reference sections after a change, rendered from
        //    the trees.
        let xrefs: Vec<usize> = self.xref_at.iter().copied().collect();
        for x in xrefs {
            self.xref(x, &dirty, &mut first, deflate, clock)?;
        }
        // 3. The byte counts, now that every length is known.
        let mut term_changed = false;
        let lens: Vec<usize> = self.len_at.iter().copied().collect();
        for l in lens {
            self.lengths(l, &mut first, &mut term_changed)?;
        }
        self.live = true;
        let out = self.out(&first);
        self.stats.done_at = clock();
        Ok(Some(out))
    }

    /// Replace the chunk at `p` by `new` (positions stay); the files whose
    /// object streams the old one had events of.
    fn replace(&mut self, p: usize, new: Slot, first: &mut BTreeMap<u32, usize>) -> Vec<u32> {
        let old = core::mem::replace(&mut self.slots[p], new);
        // (its old object streams' objects are no longer placed by it)
        self.unplace(&old);
        // (a file whose bytes and marks in the chunk are as they were did
        // not change here)
        for f in old.lens.iter().chain(&self.slots[p].lens).map(|x| x.0) {
            if !same_file(&old, &self.slots[p], f) {
                note(first, f, p);
            }
        }
        for &(f, n) in &old.lens {
            if let Some(t) = self.files.get_mut(&f) {
                t.fen.add(p, -i64::try_from(n).unwrap_or(0));
                t.count -= 1;
            }
        }
        for &(f, num, _) in &old.marks {
            if let Some(Some((q, _))) = self.marks.get(&f).and_then(|m| m.get(num_ix(num)))
                && *q == p
            {
                self.marks.get_mut(&f).expect("present")[num_ix(num)] = None;
            }
        }
        for f in old.os_files() {
            if let Some(s) = self.os_at.get_mut(&f) {
                s.remove(&p);
            }
            if let Some(s) = self.closes.get_mut(&f) {
                s.remove(&p);
            }
        }
        self.virt -= usize::from(old.virt);
        self.term_at.remove(&p);
        self.misc_at.remove(&p);
        self.len_at.remove(&p);
        self.xref_at.remove(&p);
        self.index(p);
        // (its object streams' events make no bytes here: the streams
        // they reach are rendered again, and changed there if they did)
        old.os_files()
    }

    /// Put the chunk at `p` in the indexes and trees (its lengths added).
    fn index(&mut self, p: usize) {
        let s = &self.slots[p];
        for &(f, n) in &s.lens {
            let t = self.files.entry(f).or_default();
            if t.fen.t.len() < self.slots.len() + 1 {
                // (a file new to the layout: its tree over every position)
                t.fen = Fenwick::build(&alloc::vec![0; self.slots.len()]);
            }
            t.fen.add(p, i64::try_from(n).unwrap_or(i64::MAX));
            t.count += 1;
        }
        for f in s.os_files() {
            self.os_at.entry(f).or_default().insert(p);
            if s.closes(f) {
                self.closes.entry(f).or_default().insert(p);
            }
        }
        if !s.term.is_empty() {
            self.term_at.insert(p);
        }
        if !s.misc.is_empty() {
            self.misc_at.insert(p);
        }
        if !s.lengths.is_empty() {
            self.len_at.insert(p);
        }
        if !s.xrefs.is_empty() {
            self.xref_at.insert(p);
        }
        self.virt += usize::from(s.virt);
        // (virtual numbers: a full link resolves them, and its marks are
        // not by pdfTeX's numbers)
        if !s.virt {
            let marks = s.marks.clone();
            for (f, num, at) in marks {
                self.mark(f, num, p, at);
            }
        }
    }

    fn mark(&mut self, f: u32, num: i32, p: usize, at: u64) {
        let m = self.marks.entry(f).or_default();
        let i = num_ix(num);
        if m.len() <= i {
            m.resize(i + 1, None);
        }
        if let Some((q, _)) = m[i]
            && q != p
        {
            self.dup_marks = true;
        }
        m[i] = Some((p, at));
    }

    /// The marks made again from every chunk, in program order (an object
    /// marked twice: the last mark counts).
    fn remark(&mut self) {
        self.marks.clear();
        self.dup_marks = false;
        for p in 0..self.slots.len() {
            if self.slots[p].virt {
                continue;
            }
            let marks = self.slots[p].marks.clone();
            for (f, num, at) in marks {
                let m = self.marks.entry(f).or_default();
                let i = num_ix(num);
                if m.len() <= i {
                    m.resize(i + 1, None);
                }
                if m[i].is_some_and(|(q, _)| q != p) {
                    self.dup_marks = true;
                }
                m[i] = Some((p, at));
            }
        }
    }

    /// The objects `s`'s object streams placed are no longer placed by
    /// them.
    fn unplace(&mut self, s: &Slot) {
        for e in &s.os {
            if let Os::Close(f, num, _, o) = *e {
                self.unplace_close(f, num, &s.os_objs[o as usize]);
            }
        }
    }

    fn unplace_close(&mut self, f: u32, num: i32, objs: &[i32]) {
        let Some(m) = self.placed.get_mut(&f) else {
            return;
        };
        for &obj in objs {
            if let Some(e) = m.get_mut(num_ix(obj))
                && e.is_some_and(|(s, _)| s == num)
            {
                *e = None;
            }
        }
    }

    /// Chunks inserted or removed: the new order made by a merge, and
    /// every index and tree made again (no chunk's effects looked at
    /// again).
    fn splice(
        &mut self,
        by_step: &BTreeMap<u32, StepChunks>,
        first: &mut BTreeMap<u32, usize>,
        dirty: &mut BTreeSet<usize>,
        cuts: &mut Vec<(usize, Vec<u32>)>,
    ) {
        let old = core::mem::take(&mut self.slots);
        let mut new: Vec<Slot> = Vec::new();
        for c in by_step.values() {
            for (k, (v, fx)) in c.chunks.iter().flatten().enumerate() {
                new.push(Slot::new(c.step, idx(k), c.order, *v, fx));
            }
        }
        new.sort_by_key(|s| (s.order, s.k));
        let mut merged = Vec::with_capacity(old.len() + new.len());
        let mut new = new.into_iter().peekable();
        let mut removed: Vec<Slot> = Vec::new();
        for s in old {
            while let Some(n) = new.peek()
                && (n.order, n.k) < (s.order, s.k)
            {
                dirty.insert(merged.len());
                merged.push(new.next().expect("peeked"));
            }
            if by_step.contains_key(&s.step) {
                for &(f, _) in &s.lens {
                    note(first, f, merged.len());
                }
                cuts.push((merged.len(), s.os_files()));
                removed.push(s);
            } else {
                merged.push(s);
            }
        }
        for n in new {
            dirty.insert(merged.len());
            merged.push(n);
        }
        for s in &removed {
            self.unplace(s);
        }
        self.stats.chunks_out = removed.len();
        for &p in dirty.iter() {
            for &(f, _) in &merged[p].lens {
                note(first, f, p);
            }
        }
        self.slots = merged;
        self.stats.reindexed = true;
        self.reindex();
    }

    /// Every index and tree made from the chunks as they are.
    fn reindex(&mut self) {
        self.steps.clear();
        self.files.clear();
        self.os_at.clear();
        self.closes.clear();
        self.term_at.clear();
        self.misc_at.clear();
        self.len_at.clear();
        self.xref_at.clear();
        self.virt = 0;
        let n = self.slots.len();
        let mut vals: BTreeMap<u32, (Vec<u64>, usize)> = BTreeMap::new();
        for (p, s) in self.slots.iter().enumerate() {
            let e = self.steps.entry(s.step).or_insert((p, 0));
            e.1 += 1;
            for &(f, len) in &s.lens {
                let v = vals.entry(f).or_insert_with(|| (alloc::vec![0; n], 0));
                v.0[p] = len;
                v.1 += 1;
            }
            for f in s.os_files() {
                self.os_at.entry(f).or_default().insert(p);
                if s.closes(f) {
                    self.closes.entry(f).or_default().insert(p);
                }
            }
            if !s.term.is_empty() {
                self.term_at.insert(p);
            }
            if !s.misc.is_empty() {
                self.misc_at.insert(p);
            }
            if !s.lengths.is_empty() {
                self.len_at.insert(p);
            }
            if !s.xrefs.is_empty() {
                self.xref_at.insert(p);
            }
            self.virt += usize::from(s.virt);
        }
        for (f, (v, count)) in vals {
            self.files.insert(
                f,
                Tree {
                    fen: Fenwick::build(&v),
                    count,
                },
            );
        }
        self.remark();
    }

    /// The chunk at `p` laid out again (its rendered pieces changed): its
    /// lengths' differences into the trees, its marks' offsets.
    fn relayout_at(&mut self, p: usize) {
        let before = self.slots[p].lens.clone();
        self.slots[p].relayout();
        let after = self.slots[p].lens.clone();
        let n = self.slots.len();
        for &(f, len) in &after {
            let was = before
                .binary_search_by_key(&f, |x| x.0)
                .map_or(0, |i| before[i].1);
            let t = self.files.entry(f).or_insert_with(|| Tree {
                fen: Fenwick::build(&alloc::vec![0; n]),
                count: 0,
            });
            if before.binary_search_by_key(&f, |x| x.0).is_err() {
                t.count += 1;
            }
            if len != was {
                t.fen.add(
                    p,
                    i64::try_from(len).unwrap_or(i64::MAX) - i64::try_from(was).unwrap_or(0),
                );
            }
        }
        for &(f, was) in &before {
            if after.binary_search_by_key(&f, |x| x.0).is_err()
                && let Some(t) = self.files.get_mut(&f)
            {
                t.fen.add(p, -i64::try_from(was).unwrap_or(0));
                t.count -= 1;
            }
        }
        if !self.slots[p].virt {
            let marks = self.slots[p].marks.clone();
            for (f, num, at) in marks {
                self.mark(f, num, p, at);
            }
        }
    }

    /// The object streams the changes reach, rendered again: every one a
    /// chunk put in closes, and the first one closed after each chunk put
    /// in or taken out, by each file whose object streams it had events
    /// of.
    fn streams(
        &mut self,
        dirty: &BTreeSet<usize>,
        cuts: &[(usize, Vec<u32>)],
        first: &mut BTreeMap<u32, usize>,
        deflate: &mut Deflate<'_>,
    ) {
        // (file, chunk, which close of the file in it: `None` all)
        let mut todo: BTreeSet<(usize, u32, bool)> = BTreeSet::new();
        let next_close = |sp: &Self, f: u32, from: usize| {
            sp.closes
                .get(&f)
                .and_then(|s| s.range(from..).next().copied())
        };
        for &p in dirty {
            let s = &self.slots[p];
            for f in s.os_files() {
                if s.closes(f) {
                    todo.insert((p, f, true));
                }
                if let Some(q) = next_close(self, f, p + 1) {
                    todo.insert((q, f, false));
                }
            }
        }
        for (p, files) in cuts {
            for &f in files {
                if let Some(q) = next_close(self, f, *p) {
                    todo.insert((q, f, false));
                }
            }
        }
        // (a dirty chunk renders all its closes)
        let todo: Vec<(usize, u32, bool)> = todo
            .iter()
            .copied()
            .filter(|&(q, f, all)| all || !todo.contains(&(q, f, true)))
            .collect();
        for (q, f, all) in todo {
            let closes: Vec<(usize, i32, i32, u32)> = self.slots[q]
                .os
                .iter()
                .enumerate()
                .filter_map(|(i, e)| match *e {
                    Os::Close(g, num, level, o) if g == f => Some((i, num, level, o)),
                    _ => None,
                })
                .collect();
            let n = if all {
                closes.len()
            } else {
                closes.len().min(1)
            };
            let mut changed = false;
            for &(i, num, level, o) in &closes[..n] {
                let (objs, data) = self.gather(q, i, f);
                let old_objs = core::mem::take(&mut self.slots[q].os_objs[o as usize]);
                self.unplace_close(f, num, &old_objs);
                let m = self.placed.entry(f).or_default();
                for (j, (obj, _)) in objs.iter().enumerate() {
                    let ix = num_ix(*obj);
                    if m.len() <= ix {
                        m.resize(ix + 1, None);
                    }
                    m[ix] = Some((num, u8::try_from(j).unwrap_or(0)));
                }
                self.stats.streams += 1;
                self.stats.stream_bytes += data.len();
                let ids: Vec<i32> = objs.iter().map(|x| x.0).collect();
                let bytes = render_objstm(num, level, &objs, data, deflate);
                let s = &mut self.slots[q];
                s.os_objs[o as usize] = ids;
                if s.owned[o as usize] != bytes {
                    s.owned[o as usize] = bytes;
                    changed = true;
                }
            }
            if changed {
                note(first, f, q);
                self.relayout_at(q);
            }
        }
    }

    /// Object stream `f`'s objects and bytes for the close at `os[i]` of
    /// the chunk at `q`: every event of the file since the close before.
    fn gather(&self, q: usize, i: usize, f: u32) -> super::ObjStmFill {
        let mut parts: Vec<(usize, usize)> = Vec::new();
        let mut p = q;
        let mut end = i;
        'back: loop {
            let os = &self.slots[p].os;
            for j in (0..end).rev() {
                match os[j] {
                    Os::Close(g, ..) if g == f => break 'back,
                    Os::Start(g, _) | Os::Bytes(g, _) if g == f => parts.push((p, j)),
                    _ => {}
                }
            }
            match self
                .os_at
                .get(&f)
                .and_then(|s| s.range(..p).next_back().copied())
            {
                Some(prev) => {
                    p = prev;
                    end = self.slots[p].os.len();
                }
                None => break,
            }
        }
        let mut objs = Vec::new();
        let mut data = Vec::new();
        for &(p, j) in parts.iter().rev() {
            match self.slots[p].os[j] {
                Os::Start(_, num) => objs.push((num, data.len())),
                Os::Bytes(_, e) => data.extend_from_slice(self.slots[p].bytes(e)),
                Os::Close(..) => {}
            }
        }
        (objs, data)
    }

    /// The cross-reference sections of the chunk at `x`, rendered again if
    /// it was put in or a byte of its file before it changed.
    fn xref(
        &mut self,
        x: usize,
        dirty: &BTreeSet<usize>,
        first: &mut BTreeMap<u32, usize>,
        deflate: &mut Deflate<'_>,
        clock: &dyn Fn() -> u64,
    ) -> Result<(), LinkError> {
        let xrefs = self.slots[x].xrefs.clone();
        let mut changed = false;
        for (f, e, o) in xrefs {
            // (offsets move only after a change at or before it)
            if !dirty.contains(&x) && first.get(&f).is_none_or(|&p| p > x) {
                continue;
            }
            let Effect::PdfXref { xref, .. } = &self.slots[x].fx[e as usize] else {
                continue;
            };
            // (the section begins where its piece does)
            let local = {
                let s = &self.slots[x];
                let mut at = 0u64;
                for &it in &s.items {
                    if let Item::Owned(_, k) = it
                        && k == o
                    {
                        break;
                    }
                    if let Some((g, b)) = s.piece(it)
                        && g == f
                    {
                        at += b.len() as u64;
                    }
                }
                at
            };
            let tree = self.files.get(&f);
            let base = tree.map_or(0, |t| t.fen.prefix(x));
            let at = i64::try_from(base + local).unwrap_or(i64::MAX);
            let missing = core::cell::Cell::new(None);
            let note_missing = |num| missing.set(missing.get().or(Some(num)));
            let marks = self.marks.get(&f);
            let placed = self.placed.get(&f);
            let t = clock();
            let bytes = xref.render(
                at,
                &mut |num| {
                    let m = marks.and_then(|m| m.get(num_ix(num)).copied().flatten());
                    if let Some((p, local)) = m {
                        let b = tree.map_or(0, |t| t.fen.prefix(p));
                        i64::try_from(b + local).unwrap_or(i64::MAX)
                    } else {
                        note_missing(num);
                        0
                    }
                },
                &mut |num| {
                    let v = placed.and_then(|m| m.get(num_ix(num)).copied().flatten());
                    v.unwrap_or_else(|| {
                        note_missing(num);
                        (0, 0)
                    })
                },
                deflate,
            );
            self.stats.deflate_ns += clock() - t;
            self.stats.xrefs += 1;
            self.stats.xref_entries += xref.entries.len();
            if let Some(num) = missing.get() {
                return Err(LinkError::Unplaced {
                    file: WriteId(f),
                    num,
                });
            }
            let s = &mut self.slots[x];
            if s.owned[o as usize] != bytes {
                s.owned[o as usize] = bytes;
                changed = true;
                note(first, f, x);
            }
        }
        if changed {
            self.relayout_at(x);
        }
        Ok(())
    }

    /// The byte counts of the chunk at `l`, written with the files'
    /// lengths now.
    fn lengths(
        &mut self,
        l: usize,
        first: &mut BTreeMap<u32, usize>,
        term_changed: &mut bool,
    ) -> Result<(), LinkError> {
        let lengths = self.slots[l].lengths.clone();
        for (e, o) in lengths {
            let Effect::Length {
                stream,
                file,
                assumed,
                text,
            } = &self.slots[l].fx[e as usize]
            else {
                continue;
            };
            let actual = self
                .files
                .get(&file.0)
                .filter(|t| t.count > 0)
                .map_or(0, |t| t.fen.total());
            let actual = i64::try_from(actual).unwrap_or(i64::MAX);
            let (old, new) = (
                alloc::format!("{assumed}").into_bytes(),
                alloc::format!("{actual}").into_bytes(),
            );
            if old.len() != new.len() {
                return Err(LinkError::LengthDigits {
                    file: *file,
                    assumed: *assumed,
                    actual,
                });
            }
            let mut digits = new.iter();
            let real: Vec<u8> = text
                .iter()
                .map(|&b| {
                    if b == b'\n' {
                        b
                    } else {
                        digits.next().copied().unwrap_or(b)
                    }
                })
                .collect();
            let stream = *stream;
            let s = &mut self.slots[l];
            if s.owned[o as usize] != real {
                s.owned[o as usize] = real;
                match stream {
                    Stream::File(f) => note(first, f.0, l),
                    Stream::Term => *term_changed = true,
                }
            }
        }
        Ok(())
    }

    /// The link's result from the layout.
    fn out(&self, first: &BTreeMap<u32, usize>) -> SpliceOut {
        let mut o = SpliceOut::default();
        for (&f, t) in &self.files {
            if t.count == 0 {
                continue;
            }
            let from = first.get(&f).map(|&p| t.fen.prefix(p));
            o.files.insert(f, (t.fen.total(), from));
        }
        for &p in &self.term_at {
            let s = &self.slots[p];
            for it in &s.term {
                match *it {
                    TermItem::Text(i) => o.term.extend_from_slice(s.bytes(i)),
                    TermItem::Length(k) => o.term.extend_from_slice(&s.owned[k as usize]),
                }
            }
        }
        self.each_misc(&mut |e| {
            if let Effect::Open { file, name, kind } = e {
                o.opened.push((*file, name.clone(), *kind));
            }
        });
        o
    }

    /// Every diagnostic, page, open and close, in program order.
    fn each_misc(&self, f: &mut dyn FnMut(&Effect)) {
        for &p in &self.misc_at {
            let s = &self.slots[p];
            for &i in &s.misc {
                f(&s.fx[i as usize]);
            }
        }
    }

    /// Hand each diagnostic to `f`, in program order.
    pub fn each_diagnostic(&self, f: &mut dyn FnMut(&Diagnostic)) {
        self.each_misc(&mut |e| {
            if let Effect::Diagnostic(d) = e {
                f(d);
            }
        });
    }

    /// The pages shipped out, by `\count0`, in order.
    #[must_use]
    pub fn pages(&self) -> Vec<i32> {
        let mut v = Vec::new();
        self.each_misc(&mut |e| {
            if let Effect::Shipping(c) = e {
                v.push(*c);
            }
        });
        v
    }

    /// The files closed, in order.
    #[must_use]
    pub fn closed(&self) -> Vec<WriteId> {
        let mut v = Vec::new();
        self.each_misc(&mut |e| {
            if let Effect::Close(f) = e {
                v.push(*f);
            }
        });
        v
    }

    /// File `f`'s length now.
    #[must_use]
    pub fn len(&self, f: u32) -> u64 {
        self.files.get(&f).map_or(0, |t| t.fen.total())
    }

    /// Hand file `f`'s bytes from byte `from` on to `put`, piece by piece,
    /// in order.
    pub fn write_from(&self, f: u32, from: u64, put: &mut dyn FnMut(&[u8])) {
        let Some(t) = self.files.get(&f) else {
            return;
        };
        let start = t.fen.find(from);
        let mut skip = from - t.fen.prefix(start);
        for s in &self.slots[start.min(self.slots.len())..] {
            if s.len(f) == 0 {
                continue;
            }
            for &it in &s.items {
                let Some((g, b)) = s.piece(it) else {
                    continue;
                };
                if g != f || b.is_empty() {
                    continue;
                }
                let n = b.len() as u64;
                if skip >= n {
                    skip -= n;
                    continue;
                }
                put(&b[usize::try_from(skip).unwrap_or(0)..]);
                skip = 0;
            }
        }
    }

    /// File `f`'s bytes.
    #[must_use]
    pub fn file(&self, f: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(usize::try_from(self.len(f)).unwrap_or(0));
        self.write_from(f, 0, &mut |b| v.extend_from_slice(b));
        v
    }
}

/// The bytes of file `f`'s pieces in chunk `s` (`None`: a piece is
/// rendered at the link).
fn pieces(s: &Slot, f: u32) -> Option<Vec<&[u8]>> {
    let mut v = Vec::new();
    for &it in &s.items {
        match it {
            Item::Bytes(g, i) if g == f => v.push(s.bytes(i)),
            Item::Owned(g, _) if g == f => return None,
            _ => {}
        }
    }
    Some(v)
}

/// Whether chunks `a` and `b` make the same bytes and marks of file `f`
/// (no piece of it rendered at the link).
fn same_file(a: &Slot, b: &Slot, f: u32) -> bool {
    if a.len(f) != b.len(f) {
        return false;
    }
    let marks = |s: &Slot| {
        s.marks
            .iter()
            .filter(|m| m.0 == f)
            .map(|m| (m.1, m.2))
            .collect::<Vec<_>>()
    };
    if marks(a) != marks(b) {
        return false;
    }
    let (Some(x), Some(y)) = (pieces(a, f), pieces(b, f)) else {
        return false;
    };
    // (the two runs of slices compared a stretch at a time)
    let (mut i, mut j, mut u, mut v) = (0, 0, 0, 0);
    loop {
        while i < x.len() && u == x[i].len() {
            i += 1;
            u = 0;
        }
        while j < y.len() && v == y[j].len() {
            j += 1;
            v = 0;
        }
        if i == x.len() || j == y.len() {
            return i == x.len() && j == y.len();
        }
        let n = (x[i].len() - u).min(y[j].len() - v);
        if x[i][u..u + n] != y[j][v..v + n] {
            return false;
        }
        u += n;
        v += n;
    }
}

fn num_ix(num: i32) -> usize {
    usize::try_from(num).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{Fenwick, Splice, StepChunks};
    use crate::effects::{Effect, Stream, link};
    use crate::host::WriteId;
    use crate::pdf::xref::{XEntry, Xref, XrefStream};

    #[test]
    fn fenwick_sums_and_finds() {
        let vals = [3u64, 0, 5, 0, 0, 7, 1];
        let mut f = Fenwick::build(&vals);
        for p in 0..=vals.len() {
            assert_eq!(f.prefix(p), vals[..p].iter().sum::<u64>());
        }
        assert_eq!(f.find(0), 0);
        assert_eq!(f.find(2), 0);
        assert_eq!(f.find(3), 2);
        assert_eq!(f.find(7), 2);
        assert_eq!(f.find(8), 5);
        assert_eq!(f.find(15), 6);
        assert_eq!(f.find(16), 7);
        f.add(1, 4);
        assert_eq!(f.prefix(2), 7);
        assert_eq!(f.find(3), 1);
        f.add(1, -4);
        assert_eq!(f.total(), 16);
    }

    const PDF: WriteId = WriteId(2);
    const LOG: WriteId = WriteId(1);

    fn w(f: WriteId, b: &str) -> Effect {
        Effect::Write {
            file: f,
            bytes: b.as_bytes().to_vec(),
        }
    }

    /// A page: its content object, its dictionary in an object stream,
    /// and a log line.
    fn page(n: i32, text: &str) -> Vec<Effect> {
        vec![
            w(LOG, &alloc::format!("[{n}")),
            Effect::PdfObject {
                file: PDF,
                num: 10 * n,
                ahead: 0,
            },
            w(PDF, &alloc::format!("{} 0 obj\n{text}\nendobj\n", 10 * n)),
            Effect::ObjStmStart {
                file: PDF,
                num: 10 * n + 1,
            },
            Effect::ObjStmBytes {
                file: PDF,
                bytes: alloc::format!("<</Page {n}>>").into_bytes(),
            },
            w(LOG, "]"),
        ]
    }

    fn close(num: i32) -> Vec<Effect> {
        vec![
            Effect::PdfObject {
                file: PDF,
                num,
                ahead: 0,
            },
            Effect::ObjStm {
                file: PDF,
                num,
                level: 0,
            },
        ]
    }

    fn end(objs: &[i32], streams: &[i32]) -> Vec<Effect> {
        let mut entries = vec![XEntry::Free(0)];
        let max = objs.iter().chain(streams).copied().max().unwrap_or(0) + 1;
        for k in 1..max {
            if objs.contains(&k) && k % 10 == 0 || streams.contains(&k) {
                entries.push(XEntry::Byte(k));
            } else if objs.contains(&k) {
                entries.push(XEntry::Placed(k));
            } else {
                entries.push(XEntry::Free(0));
            }
        }
        entries.push(XEntry::Byte(max));
        vec![
            Effect::PdfXref {
                file: PDF,
                xref: alloc::boxed::Box::new(Xref {
                    stream: Some(XrefStream {
                        num: max,
                        obj_ptr: max,
                        level: 0,
                    }),
                    entries,
                    tail: b"/Root 1 0 R\n".to_vec(),
                }),
            },
            w(LOG, "Output written ("),
            Effect::Length {
                stream: Stream::File(LOG),
                file: PDF,
                assumed: 1000,
                text: b"1000".to_vec(),
            },
            w(LOG, " bytes)."),
        ]
    }

    /// The chunks of a document of pages `texts` (a step each, an object
    /// stream closed every `per` pages and at the end), keyed by step.
    fn doc(texts: &[&str], per: usize) -> Vec<(u32, Vec<Effect>)> {
        let mut steps = Vec::new();
        let mut objs = Vec::new();
        let mut streams = Vec::new();
        let mut open = 0;
        for (i, t) in texts.iter().enumerate() {
            let n = i32::try_from(i).unwrap() + 1;
            let mut fx = page(n, t);
            objs.extend([10 * n, 10 * n + 1]);
            open += 1;
            if open == per {
                let s = 1000 + n;
                fx.extend(close(s));
                streams.push(s);
                open = 0;
            }
            steps.push((u32::try_from(i).unwrap(), fx));
        }
        let mut fx = Vec::new();
        if open > 0 {
            fx.extend(close(999));
            streams.push(999);
        }
        fx.extend(end(&objs, &streams));
        steps.push((u32::try_from(texts.len()).unwrap(), fx));
        steps
    }

    fn full(steps: &[(u32, Vec<Effect>)]) -> crate::effects::Linked {
        let all: Vec<&[Effect]> = steps.iter().map(|(_, v)| v.as_slice()).collect();
        link(&all, &crate::exec::Sequential, &mut |_, d: &[u8]| {
            Some(d.to_vec())
        })
        .unwrap()
    }

    fn version(fx: &[Effect]) -> u128 {
        let mut s = partex_engine::persist::Saver::default();
        partex_engine::persist::Persist::save(&fx.to_vec(), &mut s);
        partex_engine::stablehash::StableHasher::of(&s.into_bytes())
    }

    fn change(step: u32, fx: Option<&Vec<Effect>>) -> StepChunks {
        StepChunks {
            step,
            order: u64::from(step) * 16 + 16,
            chunks: fx.map(|fx| vec![(version(fx), Arc::from(fx.clone()))]),
        }
    }

    /// Link `steps` after `changed` (all, the first time), check it is a
    /// full link's, and return the first changed byte of the PDF file.
    fn check(sp: &mut Splice, steps: &[(u32, Vec<Effect>)], changed: &[u32]) -> Option<u64> {
        let changes = changed
            .iter()
            .map(|&s| change(s, steps.iter().find(|x| x.0 == s).map(|x| &x.1)))
            .collect();
        let out = sp
            .link(changes, None, &mut |_, d: &[u8]| Some(d.to_vec()), &|| 0)
            .unwrap()
            .unwrap();
        let want = full(steps);
        for (f, bytes) in &want.files {
            assert_eq!(&sp.file(*f), bytes, "file {f}");
            assert_eq!(out.files[f].0, bytes.len() as u64);
        }
        assert_eq!(out.files.len(), want.files.len());
        assert_eq!(out.term, want.term);
        assert_eq!(out.opened, want.opened);
        out.files[&PDF.0].1
    }

    #[test]
    fn a_page_edited_costs_its_chunk() {
        let mut texts = vec!["a", "bb", "ccc", "dddd", "eeeee", "f", "g"];
        let mut steps = doc(&texts, 3);
        let mut sp = Splice::default();
        let all: Vec<u32> = steps.iter().map(|x| x.0).collect();
        assert_eq!(check(&mut sp, &steps, &all), Some(0));
        // (nothing changed: nothing written)
        assert_eq!(check(&mut sp, &steps, &[]), None);
        // (page 5 longer: its chunk, its stream, the xref and the log's
        // count; the PDF changed from page 5 on)
        let was = full(&steps).files[&PDF.0].clone();
        texts[4] = "eeeee, longer";
        steps = doc(&texts, 3);
        let now = full(&steps).files[&PDF.0].clone();
        let diff = was.iter().zip(&now).position(|(a, b)| a != b).unwrap() as u64;
        let from = check(&mut sp, &steps, &[4]).unwrap();
        // (from the page's chunk on: its object's first line is as it was)
        assert!(from <= diff && diff - from < 16, "{from} {diff}");
        assert!(!sp.stats.reindexed);
        assert_eq!(sp.stats.chunks_in, 1);
        // (the same again, reverted)
        texts[4] = "eeeee";
        steps = doc(&texts, 3);
        check(&mut sp, &steps, &[4]);
    }

    #[test]
    fn pages_added_and_removed() {
        let mut texts = vec!["a", "bb", "ccc", "dddd", "eeeee"];
        let mut steps = doc(&texts, 2);
        let mut sp = Splice::default();
        let all: Vec<u32> = steps.iter().map(|x| x.0).collect();
        check(&mut sp, &steps, &all);
        // (a page more: every step from page 3 on is new; the last one's
        // id moves)
        texts.insert(2, "new");
        let steps2 = doc(&texts, 2);
        let mut ch: Vec<u32> = (2..u32::try_from(steps2.len()).unwrap()).collect();
        ch.push(u32::try_from(steps.len() - 1).unwrap());
        check(&mut sp, &steps2, &ch);
        assert!(sp.stats.reindexed);
        // (and removed again)
        texts.remove(2);
        steps = doc(&texts, 2);
        let mut ch: Vec<u32> = (2..u32::try_from(steps2.len()).unwrap()).collect();
        ch.push(u32::try_from(steps.len() - 1).unwrap());
        check(&mut sp, &steps, &ch);
    }

    /// Random edits of a document (pages changed, added, removed; the
    /// object streams' sizes changed), each link a full link's.
    #[test]
    fn random_edits_link_as_a_full_link() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % (n as u64)).unwrap()
        };
        let words = ["a", "bb", "ccc", "a longer line", "x", "", "yy yy"];
        let mut texts: Vec<alloc::string::String> = (0..12).map(|i| words[i % 7].into()).collect();
        let mut per = 3;
        let mut steps = doc(
            &texts
                .iter()
                .map(alloc::string::String::as_str)
                .collect::<Vec<_>>(),
            per,
        );
        let mut sp = Splice::default();
        let all: Vec<u32> = steps.iter().map(|x| x.0).collect();
        check(&mut sp, &steps, &all);
        for _ in 0..300 {
            match rand(10) {
                0..=4 => {
                    let i = rand(texts.len());
                    texts[i] = words[rand(7)].into();
                }
                5 => texts.insert(rand(texts.len() + 1), words[rand(7)].into()),
                6 if texts.len() > 1 => {
                    texts.remove(rand(texts.len()));
                }
                7 => per = 1 + rand(4),
                _ => {}
            }
            let new = doc(
                &texts
                    .iter()
                    .map(alloc::string::String::as_str)
                    .collect::<Vec<_>>(),
                per,
            );
            let mut changed = Vec::new();
            let n = steps.len().max(new.len());
            for s in 0..n {
                let (a, b) = (steps.get(s), new.get(s));
                if a.map(|x| &x.1) != b.map(|x| &x.1) {
                    changed.push(u32::try_from(s).unwrap());
                }
            }
            check(&mut sp, &new, &changed);
            steps = new;
        }
    }

    #[test]
    fn a_chunk_without_objects_moves_the_rest() {
        let texts = ["a", "bb", "ccc"];
        let mut steps = doc(&texts, 5);
        let mut sp = Splice::default();
        let all: Vec<u32> = steps.iter().map(|x| x.0).collect();
        check(&mut sp, &steps, &all);
        // (page 1's log text only: the PDF is as it was)
        steps[0].1[0] = w(LOG, "[1 more");
        assert_eq!(check(&mut sp, &steps, &[0]), None);
        // (page 2's object stream bytes only)
        steps[1].1[4] = Effect::ObjStmBytes {
            file: PDF,
            bytes: b"<</Page 2 /Rotate 90>>".to_vec(),
        };
        assert!(check(&mut sp, &steps, &[1]).is_some());
    }
}
