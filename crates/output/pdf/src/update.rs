//! The file as it is written while a document is edited: an exact base
//! and appended update sections (PDF 1.7 §7.5.6, incremental updates).
//!
//! A link's result is a [`Layout`]: the chunks the records and the tail
//! wrote, in order, and the state that numbered their objects; the
//! exact file is its chunks placed and the cross-reference section
//! after them ([`Layout::exact`], today's bytes). An [`Updater`] keeps
//! what the file on disk holds, object by object (the layout last
//! written: each object's bytes, the file's length, the last
//! cross-reference section's offset), and makes the section that turns
//! it into a newer layout ([`Updater::section`]): the objects whose
//! bytes changed, written plain after the file's end (one in an object
//! stream before overridden), a cross-reference section for those only
//! (a stream where the base has one, else a table) and a trailer with
//! `/Prev`. Object numbers are the layout's, pdfTeX's: a new object's is
//! past the base's `/Size`.
//!
//! What changed is found by the chunks: a link copies an unchanged
//! record's chunks from the last one (the same allocation), so the two
//! layouts' chunk lists share a prefix and a suffix by pointer; only
//! the objects that begin between them (and the one running into them)
//! are compared, byte by byte with their last written bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::{Chunk, PdfBytes, PdfError, Tail};

/// A link's file before its cross-reference section: the chunks the
/// state flushed and the tail's.
#[derive(Clone, Debug)]
pub struct Layout {
    bytes: Arc<PdfBytes>,
    tail: Arc<Tail>,
    exact: Arc<OnceLock<Result<Arc<Vec<u8>>, PdfError>>>,
}

/// Where an object is in a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Free,
    /// Written directly: its bytes are its span of the chunks.
    Direct,
    /// In object stream `.0` at index `.1`.
    In(usize, u32),
}

impl Layout {
    /// The layout of `bytes` (what the state flushed) and `tail` (its
    /// end).
    #[must_use]
    pub fn new(tail: Arc<Tail>, bytes: PdfBytes) -> Layout {
        Layout {
            bytes: Arc::new(bytes),
            tail,
            exact: Arc::default(),
        }
    }

    /// The exact file (made once, shared).
    ///
    /// # Errors
    /// As [`Tail::link`].
    pub fn exact(&self) -> Result<Arc<Vec<u8>>, PdfError> {
        self.exact
            .get_or_init(|| self.tail.link(&self.bytes).map(Arc::new))
            .clone()
    }

    /// The exact file if made already.
    #[must_use]
    pub fn exact_made(&self) -> Option<Arc<Vec<u8>>> {
        self.exact.get().and_then(|r| r.as_ref().ok().cloned())
    }

    /// A hash of the file's bytes, from its chunks' (each chunk's hashed
    /// once while a layout holds it: an edit hashes its new ones).
    #[must_use]
    pub fn hash(&self) -> u64 {
        chunks_hash(self.chunks())
    }

    /// Whether `o` is the same file as [`Layout::hash`] sees it, compared
    /// whole: the same chunks, each the same bytes with the same objects
    /// (a file cut into other chunks counts as another, as its hash may).
    #[must_use]
    pub fn same_chunks(&self, o: &Layout) -> bool {
        self.count() == o.count()
            && self
                .chunks()
                .zip(o.chunks())
                .all(|(a, b)| (Arc::ptr_eq(&a.bytes, &b.bytes) || a.bytes == b.bytes) && a.objs == b.objs)
    }

    /// The bytes before the cross-reference section.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len() + self.tail.chunk.bytes.len()
    }

    /// Whether there are no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn count(&self) -> usize {
        self.bytes.chunks.len() + 1
    }

    fn chunk(&self, i: usize) -> &Chunk {
        self.bytes.chunks.get(i).unwrap_or(&self.tail.chunk)
    }

    fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.bytes
            .chunks
            .iter()
            .chain(std::iter::once(&self.tail.chunk))
    }

    /// The last object's number (the cross-reference section's own
    /// not among them).
    fn last(&self) -> usize {
        self.tail.st.sys_obj_ptr()
    }

    fn place(&self, n: usize) -> Place {
        let st = &self.tail.st;
        if n == 0 || n > self.last() || !st.is_written(n) {
            return Place::Free;
        }
        let o = &st.objs[n];
        if o.os_idx == -1 {
            Place::Direct
        } else {
            Place::In(
                usize::try_from(o.offset).unwrap_or(0),
                u32::try_from(o.os_idx).unwrap_or(0),
            )
        }
    }

    /// The objects in object streams, by stream, with their indices.
    fn by_stream(&self) -> BTreeMap<usize, Vec<(usize, u32)>> {
        let mut out: BTreeMap<usize, Vec<(usize, u32)>> = BTreeMap::new();
        for n in 1..=self.last() {
            if let Place::In(t, i) = self.place(n) {
                out.entry(t).or_default().push((n, i));
            }
        }
        out
    }

    fn streams(&self) -> bool {
        self.tail.st.os.enable
    }

    /// The bytes from place `a` to place `b` (chunk, offset; `b` past
    /// the end: to the end).
    fn span(&self, a: (usize, usize), b: (usize, usize)) -> Vec<u8> {
        let mut out = Vec::new();
        for c in a.0..=b.0.min(self.count() - 1) {
            let bytes = &self.chunk(c).bytes;
            let from = if c == a.0 { a.1 } else { 0 };
            let to = if c == b.0 {
                b.1.min(bytes.len())
            } else {
                bytes.len()
            };
            if from < to {
                out.extend_from_slice(&bytes[from..to]);
            }
        }
        out
    }

    /// The objects that begin in chunks `lo..hi`, each with its bytes (to
    /// where the next object begins, which may be in a chunk past `hi`).
    fn spans(&self, lo: usize, hi: usize) -> BTreeMap<usize, Vec<u8>> {
        let mut starts: Vec<(usize, (usize, usize))> = Vec::new();
        for c in lo..hi {
            for &(n, at) in &self.chunk(c).objs {
                starts.push((n, (c, at)));
            }
        }
        let mut end = (self.count(), 0);
        for c in hi..self.count() {
            if let Some(&(_, at)) = self.chunk(c).objs.first() {
                end = (c, at);
                break;
            }
        }
        let mut out = BTreeMap::new();
        for (k, &(n, a)) in starts.iter().enumerate() {
            let b = starts.get(k + 1).map_or(end, |s| s.1);
            out.insert(n, self.span(a, b));
        }
        out
    }
}

/// The chunks' hash from each chunk's, the last call's kept: a chunk
/// at the same place from the start or from the end as one of its
/// chunks, the same allocation, is not hashed again.
fn chunks_hash<'a>(chunks: impl Iterator<Item = &'a Chunk>) -> u64 {
    type Memo = Mutex<Vec<(Arc<[u8]>, u64)>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let mut m = MEMO
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let chunks: Vec<&Chunk> = chunks.collect();
    let (n, k) = (chunks.len(), m.len());
    let mut kept = Vec::with_capacity(n);
    let mut words = Vec::with_capacity(8 * n);
    for (i, c) in chunks.iter().enumerate() {
        let same = |j: usize| m.get(j).filter(|e| Arc::ptr_eq(&e.0, &c.bytes)).map(|e| e.1);
        let h = same(i)
            .or_else(|| (k + i).checked_sub(n).and_then(same))
            .unwrap_or_else(|| crate::finish::hash64(&c.bytes));
        words.extend_from_slice(&h.to_le_bytes());
        kept.push((c.bytes.clone(), h));
    }
    *m = kept;
    crate::finish::hash64(&words)
}

/// A cross-reference entry: free, at an offset, or in an object stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Free,
    At(u64),
    In(usize, u32),
}

/// What an update gives the file.
#[derive(Debug)]
pub enum Section {
    /// Nothing changed.
    Same,
    /// The bytes to append.
    Append(Vec<u8>),
}

/// What the file holds and how to bring it to a newer layout.
#[derive(Debug)]
pub struct Updater {
    held: Layout,
    len: u64,
    xref: u64,
    size: usize,
    /// The base's `/ID` (an update's trailer repeats the original's).
    id: Arc<[u8]>,
}

impl Updater {
    /// The state after `layout`'s exact file (`exact`, its bytes) is
    /// written.
    #[must_use]
    pub fn after_exact(layout: Layout, exact: &[u8]) -> Updater {
        let xref = startxref(exact).unwrap_or(0);
        let last = layout.last();
        let size = if layout.streams() { last + 2 } else { last + 1 };
        let id = layout.tail.id.clone();
        Updater {
            held: layout,
            len: exact.len() as u64,
            xref,
            size,
            id,
        }
    }

    /// The file's length.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the file is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The layout the file holds.
    #[must_use]
    pub fn held(&self) -> &Layout {
        &self.held
    }

    /// The section that turns the file into `new`'s objects, appended
    /// after the file's bytes; the state then holds `new`.
    #[must_use]
    pub fn section(&mut self, new: &Layout) -> Section {
        let old = &self.held;
        let (oc, nc) = (old.count(), new.count());
        let same = |i: usize, j: usize| Arc::ptr_eq(&old.chunk(i).bytes, &new.chunk(j).bytes);
        let mut p = 0;
        while p < oc && p < nc && same(p, p) {
            p += 1;
        }
        if p == oc && p == nc {
            self.held = new.clone();
            return Section::Same;
        }
        let mut s = 0;
        while s < oc - p && s < nc - p && same(oc - 1 - s, nc - 1 - s) {
            s += 1;
        }
        // (the object begun last before the chunks that differ runs into
        // them: from its chunk on)
        let from = |l: &Layout| {
            (0..p)
                .rev()
                .find(|&c| !l.chunk(c).objs.is_empty())
                .unwrap_or(p)
        };
        let olds = old.spans(from(old), oc - s);
        let news = new.spans(from(new), nc - s);
        let mut rows: BTreeMap<usize, Row> = BTreeMap::new();
        let mut body: Vec<u8> = Vec::new();
        let mut streams: BTreeSet<usize> = BTreeSet::new();
        let at = |body: &Vec<u8>| Row::At(self.len + body.len() as u64);
        for &n in olds.keys().chain(news.keys()).collect::<BTreeSet<_>>() {
            match new.place(n) {
                Place::Direct => {
                    let Some(b) = news.get(&n) else {
                        // (an object that begins where the chunks are the
                        // same: never, but found from the start if so)
                        let all = new.spans(0, nc);
                        let b = all.get(&n).cloned().unwrap_or_default();
                        rows.insert(n, at(&body));
                        body.extend_from_slice(&b);
                        streams.insert(n);
                        continue;
                    };
                    let was = (old.place(n) == Place::Direct)
                        .then(|| olds.get(&n))
                        .flatten();
                    if was != Some(b) {
                        rows.insert(n, at(&body));
                        body.extend_from_slice(b);
                        streams.insert(n);
                    }
                }
                Place::In(t, i) => {
                    if old.place(n) != Place::In(t, i) {
                        rows.insert(n, Row::In(t, i));
                    }
                }
                Place::Free => {
                    if old.place(n) != Place::Free {
                        rows.insert(n, Row::Free);
                    }
                }
            }
        }
        // an object stream written again, or one no longer
        restream(old, new, &olds, &news, &streams, &mut rows, &mut |b: &[u8]| {
            let r = at(&body);
            body.extend_from_slice(b);
            r
        });
        // the objects past the new layout's last
        for n in new.last() + 1..=old.last() {
            if old.place(n) != Place::Free {
                rows.insert(n, Row::Free);
            }
        }
        if rows.is_empty() {
            self.held = new.clone();
            return Section::Same;
        }
        let out = self.trailer(new, rows, body);
        self.len += out.len() as u64;
        self.held = new.clone();
        Section::Append(out)
    }

    /// `body` (the objects written again, at the file's end) and the
    /// cross-reference section for `rows` with its trailer.
    fn trailer(
        &mut self,
        new: &Layout,
        mut rows: BTreeMap<usize, Row>,
        mut body: Vec<u8>,
    ) -> Vec<u8> {
        let t = &new.tail;
        let mut keys = Vec::new();
        keys.extend_from_slice(format!("/Root {} 0 R\n", t.root).as_bytes());
        if let Some(i) = t.info {
            keys.extend_from_slice(format!("/Info {i} 0 R\n").as_bytes());
        }
        keys.extend_from_slice(&t.extra);
        keys.extend_from_slice(&self.id);
        if new.streams() {
            let x = self.size.max(new.last() + 1);
            let xat = self.len + body.len() as u64;
            rows.insert(x, Row::At(xat));
            self.size = x + 1;
            let max = rows
                .values()
                .map(|r| match *r {
                    Row::Free => 0,
                    Row::At(o) => o,
                    Row::In(s, _) => s as u64,
                })
                .max()
                .unwrap_or(0);
            let w = (1..=8)
                .find(|&w| w == 8 || max >> (8 * w) == 0)
                .unwrap_or(8);
            let mut data = Vec::with_capacity(rows.len() * (w + 2));
            for r in rows.values() {
                let (k, v, g) = match *r {
                    Row::Free => (0u8, 0u64, 0u8),
                    Row::At(o) => (1, o, 0),
                    Row::In(s, i) => (2, s as u64, u8::try_from(i).unwrap_or(0)),
                };
                data.push(k);
                data.extend_from_slice(&v.to_be_bytes()[8 - w..]);
                data.push(g);
            }
            let mut d = format!(
                "{x} 0 obj\n<<\n/Type /XRef\n/Size {}\n/Index [{}]\n/W [1 {w} 1]\n",
                self.size,
                index(&rows)
            )
            .into_bytes();
            d.extend_from_slice(&keys);
            d.extend_from_slice(
                format!(
                    "\n/Prev {}\n/Length {}\n>>\nstream\n",
                    self.xref,
                    data.len()
                )
                .as_bytes(),
            );
            body.extend_from_slice(&d);
            body.extend_from_slice(&data);
            body.extend_from_slice(
                format!("\nendstream\nendobj\nstartxref\n{xat}\n%%EOF\n").as_bytes(),
            );
            self.xref = xat;
        } else {
            self.size = self.size.max(new.last() + 1);
            let xat = self.len + body.len() as u64;
            body.extend_from_slice(b"xref\n");
            for (a, n) in runs(&rows) {
                body.extend_from_slice(format!("{a} {n}\n").as_bytes());
                for k in a..a + n {
                    let line = match rows.get(&k).copied().unwrap_or(Row::Free) {
                        Row::At(o) => format!("{o:010} 00000 n \n"),
                        Row::Free | Row::In(..) => "0000000000 00000 f \n".to_owned(),
                    };
                    body.extend_from_slice(line.as_bytes());
                }
            }
            body.extend_from_slice(format!("trailer\n<< /Size {}\n", self.size).as_bytes());
            body.extend_from_slice(&keys);
            body.extend_from_slice(
                format!("\n/Prev {} >>\nstartxref\n{xat}\n%%EOF\n", self.xref).as_bytes(),
            );
            self.xref = xat;
        }
        body
    }
}

/// An object stream written again, or one no longer (`streams`: the
/// objects written again; `olds`, `news`: the objects compared, by their
/// bytes): its objects whose entries now name it otherwise, and those it
/// held before, where they are now (`put`: an object's bytes appended,
/// its entry).
fn restream(
    old: &Layout,
    new: &Layout,
    olds: &BTreeMap<usize, Vec<u8>>,
    news: &BTreeMap<usize, Vec<u8>>,
    streams: &BTreeSet<usize>,
    rows: &mut BTreeMap<usize, Row>,
    put: &mut dyn FnMut(&[u8]) -> Row,
) {
    let is_stream = |b: &[u8]| b[..b.len().min(64)].windows(13).any(|w| w == b"/Type /ObjStm");
    let touched: BTreeSet<usize> = streams
        .iter()
        .copied()
        .filter(|n| news.get(n).is_some_and(|b| is_stream(b)))
        .chain(
            olds.iter()
                .filter(|&(n, b)| is_stream(b) && news.get(n) != Some(b))
                .map(|(&n, _)| n),
        )
        .collect();
    if touched.is_empty() {
        return;
    }
    let (inn, ino) = (new.by_stream(), old.by_stream());
    for t in touched {
        for &(n, i) in inn.get(&t).into_iter().flatten() {
            if old.place(n) != Place::In(t, i) {
                rows.insert(n, Row::In(t, i));
            }
        }
        for &(n, i) in ino.get(&t).into_iter().flatten() {
            match new.place(n) {
                x if x == Place::In(t, i) || rows.contains_key(&n) => {}
                Place::In(u, j) => {
                    rows.insert(n, Row::In(u, j));
                }
                Place::Free => {
                    rows.insert(n, Row::Free);
                }
                Place::Direct => {
                    let b = new.spans(0, new.count()).remove(&n).unwrap_or_default();
                    rows.insert(n, put(&b));
                }
            }
        }
    }
}

/// The runs of consecutive numbers among `rows`' keys: (first, count).
fn runs(rows: &BTreeMap<usize, Row>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &k in rows.keys() {
        match out.last_mut() {
            Some((a, n)) if *a + *n == k => *n += 1,
            _ => out.push((k, 1)),
        }
    }
    out
}

/// `/Index`'s pairs.
fn index(rows: &BTreeMap<usize, Row>) -> String {
    runs(rows)
        .iter()
        .map(|(a, n)| format!("{a} {n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The offset after the file's last `startxref`.
fn startxref(b: &[u8]) -> Option<u64> {
    let tail = &b[b.len().saturating_sub(64)..];
    let k = tail.windows(9).rposition(|w| w == b"startxref")?;
    let s = std::str::from_utf8(&tail[k + 9..]).ok()?;
    s.split_whitespace().next()?.parse().ok()
}
