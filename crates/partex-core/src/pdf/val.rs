//! The output family's values (DESIGN 7.17.12, the `dvi`, `pdf`,
//! `tounicode` and `fontmap` rows): the DVI and PDF writers' tables as
//! values of the structures' convention. Each table is a field with an
//! address of its own (`track::Row::Pdf(f)`, `track::Row::Dvi(f)`), and
//! a persistent or `Arc`-shared value carrying its version, made from its
//! parts when the value is made:
//!
//! - [`VMap`] and [`VSet`]: `partex-ssa`'s [`PMap`], whose version is the
//!   sum of one hash per entry, so a write versions what it changed;
//! - [`VTab`]: a table by index in shared chunks, copied on write, whose
//!   version is the sum of its elements' (each versioned once per writing
//!   routine, when the routine that changed it is done);
//! - [`Val`]: a record owned while the writer changes it, shared once
//!   its version is made, so a record of the runtime can hold it and
//!   putting it back is one pointer store.
//!
//! The bytes the writers hand to the host are effects
//! (`Tracker::output`), never read.
//!
//! A routine that changes a table (`ship_out`, a `\pdf…` command, the
//! job's end) is a call (DESIGN 7.17.2: every operation is a call), run
//! as a *writer scope* ([`Tex::writer_scope`]). Its reads are the fields
//! at its start, each with the version its last writer made; its writes
//! are the fields it made, versioned from their parts when the call ends
//! (a call's writes are its outputs). Inside, the writer's own code
//! changes the tables in place, and a read of a field the call itself
//! wrote is not an input. Fields a few commands store whole are read and
//! written at the access ([`Tex::writer_read`], [`Tex::writer_wrote`]).
//!
//! `ship_out` (§638) is a recorded call whose scope is the call itself
//! ([`Tex::scoped_call`]): the scopes of its parts (`fix_pdfoutput`,
//! `pdf_ship_out`, §640's) are its own, and the fields they wrote are
//! versioned when it ends, as its writes. A call begun inside an open
//! scope (`write_out`, §1370, inside `ship_out` or an `\immediate`
//! `\pdfxform`) reads through the same rule as any other call: the
//! fields the scope wrote so far are versioned first, as the scope's
//! writes before the child, and the child starts with no scope open.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::Hash;

use partex_ssa::{PMap, Value, Version};

use crate::host::Host;
use crate::tex::Tex;
use crate::track::{Row, Tracker};

/// The PDF writer's fields (`Row::Pdf`), by table.
pub(crate) mod field {
    /// `\pdfmatch`'s result, which `\pdflastmatch` reads.
    pub(crate) const LAST_MATCH: u8 = 0;
    /// The object table: its entries (pdfTeX's numbering, §7.16's
    /// `Numbering`), the lists by type, `obj_ptr`.
    pub(crate) const OBJS: u8 = 1;
    /// The object table's lookup trees (avlstuff.c).
    pub(crate) const OBJ_TREES: u8 = 2;
    /// The destination names (§7.16's `Dests`).
    pub(crate) const DESTS: u8 = 3;
    /// The writer: where the file is, the bytes not handed out yet, the
    /// object streams, the values fixed at the first page.
    pub(crate) const OUT: u8 = 4;
    /// `pdf_obj_count`, `pdf_xform_count`, `pdf_ximage_count`.
    pub(crate) const OBJ_COUNT: u8 = 5;
    pub(crate) const XFORM_COUNT: u8 = 6;
    pub(crate) const XIMAGE_COUNT: u8 = 7;
    /// The `\pdflast…` values and `\pdfretval`: `LAST + k` for
    /// `PdfLast` `k`.
    pub(crate) const LAST: u8 = 8;
    /// `\pdfinfo`, `\pdfcatalog`, `\pdfnames`, `\pdftrailer`,
    /// `\pdftrailerid`.
    pub(crate) const INFO_TOKS: u8 = 18;
    pub(crate) const CATALOG_TOKS: u8 = 19;
    pub(crate) const NAMES_TOKS: u8 = 20;
    pub(crate) const TRAILER_TOKS: u8 = 21;
    pub(crate) const TRAILER_ID_TOKS: u8 = 22;
    /// `pdf_catalog_openaction`.
    pub(crate) const CATALOG_OPENACTION: u8 = 23;
    /// The outlines' `pdf_first_outline`, `pdf_last_outline`,
    /// `pdf_parent_outline`.
    pub(crate) const OUTLINES: u8 = 24;
    /// `pdf_space_font_name`.
    pub(crate) const SPACE_FONT_NAME: u8 = 25;
    /// `pdf_font_attr`.
    pub(crate) const FONT_ATTR: u8 = 26;
    /// `pdf_font_nobuiltin_tounicode`.
    pub(crate) const NOBUILTIN_TOUNICODE: u8 = 27;
    /// The color, position and matrix stacks.
    pub(crate) const STACKS: u8 = 28;
    /// The state of shipping: the page's, and what pages leave to the
    /// next (the page tree's counts, the fonts' order of first use).
    pub(crate) const SHIP: u8 = 29;
    /// What the writer keeps per font: used, size, object, map entry,
    /// type, and the glyphs used (§7.16's `Glyphs`).
    pub(crate) const PDF_FONTS: u8 = 30;
    /// writeenc.c's encodings read (`fe_tree`'s glyph names).
    pub(crate) const ENCODINGS: u8 = 31;
    /// writefont.c's font trees.
    pub(crate) const FONTW: u8 = 32;
    /// `\pdfglyphtounicode`'s table.
    pub(crate) const TOUNICODE: u8 = 33;
    /// The font map (loaded from map files).
    pub(crate) const FONTMAP: u8 = 34;
    /// The TFM names whose map entries were used.
    pub(crate) const FONTS_MAPPED: u8 = 35;
    /// The documents open for PDF inclusion (pdftoepdf.cc's
    /// `PdfDocument`s: their images to write, their objects copied).
    pub(crate) const EPDF: u8 = 36;
    /// The fields end here.
    pub(crate) const COUNT: u8 = 37;
}

/// The DVI writer's fields (`Row::Dvi`), numbered from [`DVI`] in a
/// scope's bits.
pub(crate) mod dvi_field {
    /// The engine's side: the preamble written, the file, whether the
    /// host's page sink writes it, its length bound, too long.
    pub(crate) const FILE: u8 = super::DVI;
    /// The fonts defined (with whether each was used).
    pub(crate) const FONTS: u8 = super::DVI + 1;
    /// `total_pages`, `max_v`, `max_h`, `max_push`.
    pub(crate) const TOTALS: u8 = super::DVI + 2;
    /// Where the file is: its offsets, the bytes not written yet, the
    /// movements still in the buffer, the last `bop`, the open levels.
    pub(crate) const WRITER: u8 = super::DVI + 3;
    /// The fields end here.
    pub(crate) const END: u8 = super::DVI + 4;
}

/// Where the DVI fields begin among a scope's bits.
pub(crate) const DVI: u8 = 48;

/// A scope's bit for field `f`.
#[must_use]
pub(crate) const fn bit(f: u8) -> u64 {
    1 << f
}

/// The object table's three fields.
pub(crate) const OBJECTS: u64 = bit(field::OBJS) | bit(field::OBJ_TREES) | bit(field::DESTS);

/// What writing an object to the file changes: the table (an object
/// stream is an object) and the writer.
pub(crate) const WRITING: u64 = OBJECTS | bit(field::OUT);

/// What shipping a box out as a page or a form changes (the fields
/// stored whole at the access, `\pdfsavepos`'s, apart): the objects,
/// the writer, the stacks, the state of shipping, the fonts and their
/// glyphs, the encodings and font trees, the font map and the entries
/// used.
pub(crate) const SHIPPING: u64 = WRITING
    | bit(field::STACKS)
    | bit(field::SHIP)
    | bit(field::PDF_FONTS)
    | bit(field::ENCODINGS)
    | bit(field::FONTW)
    | bit(field::FONTMAP)
    | bit(field::FONTS_MAPPED);

/// What shipping reads besides (`\pdfspacefont`'s font).
pub(crate) const SHIPPING_READS: u64 = SHIPPING | bit(field::SPACE_FONT_NAME);

/// Every PDF field (the job's end).
pub(crate) const PDF_ALL: u64 = (1 << field::COUNT) - 1;

/// Every DVI field.
pub(crate) const DVI_ALL: u64 =
    bit(dvi_field::FILE) | bit(dvi_field::FONTS) | bit(dvi_field::TOTALS) | bit(dvi_field::WRITER);

/// Whether versions are made (a recorded build: `node::VERSIONS`); a
/// plain run keeps no element versions and pays nothing for them.
#[inline]
pub(crate) fn versions_on() -> bool {
    partex_engine::node::VERSIONS.load(core::sync::atomic::Ordering::Relaxed)
}

/// A value with the version it was made with.
#[derive(Clone, Debug)]
pub(crate) struct Ver<T> {
    v: T,
    ver: Version,
}

impl<T: Hash> Ver<T> {
    fn new(v: T) -> Self {
        let ver = Version::of(&v);
        Ver { v, ver }
    }
}

impl<T: Clone> Value for Ver<T> {
    fn version(&self) -> Version {
        self.ver
    }
}

/// A persistent map whose version is made from its entries' when each is
/// written (`partex-ssa`'s [`PMap`]: the sum of a hash per entry, the
/// same whatever order made it). A clone shares everything.
#[derive(Clone)]
pub(crate) struct VMap<K, V>(PMap<K, Ver<V>>);

impl<K: Clone + Eq + Hash, V: Clone + Hash> Default for VMap<K, V> {
    fn default() -> Self {
        Self(PMap::new())
    }
}

impl<K: Clone + Eq + Hash, V: Clone + Hash> VMap<K, V> {
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub(crate) fn get(&self, k: &K) -> Option<&V> {
        self.0.get(k).map(|v| &v.v)
    }

    #[must_use]
    pub(crate) fn contains_key(&self, k: &K) -> bool {
        self.0.get(k).is_some()
    }

    /// Bind `k` to `v`; the previous value.
    pub(crate) fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.0.insert(k, Ver::new(v)).map(|v| v.v)
    }

    pub(crate) fn remove(&mut self, k: &K) -> Option<V> {
        self.0.remove(k).map(|v| v.v)
    }

    pub(crate) fn clear(&mut self) {
        self.0 = PMap::new();
    }

    /// The map's version (made at each write).
    #[must_use]
    pub(crate) fn version(&self) -> u128 {
        self.0.version().0
    }

    /// The entries, in an order fixed by their keys' hashes.
    #[must_use]
    pub(crate) fn entries(&self) -> Vec<(&K, &V)> {
        self.0
            .entries()
            .into_iter()
            .map(|(k, v)| (k, &v.v))
            .collect()
    }

    /// The entries, in an order fixed by their keys' hashes.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries().into_iter()
    }

    /// The entries in the order of their keys.
    #[must_use]
    pub(crate) fn sorted(&self) -> Vec<(&K, &V)>
    where
        K: Ord,
    {
        let mut e = self.entries();
        e.sort_by(|a, b| a.0.cmp(b.0));
        e
    }
}

impl<K: Clone + Eq + Hash, V: Clone + Hash> PartialEq for VMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.version() == other.version()
    }
}

impl<K: Clone + Eq + Hash, V: Clone + Hash> Eq for VMap<K, V> {}

impl<K: Clone + Eq + Hash, V: Clone + Hash> Hash for VMap<K, V> {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        (self.len(), self.version()).hash(h);
    }
}

impl<K, V> core::fmt::Debug for VMap<K, V>
where
    K: Clone + Eq + Hash + Ord + core::fmt::Debug,
    V: Clone + Hash + core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.sorted()).finish()
    }
}

/// Saved as its trie's nodes, each shared (a node kept by several
/// snapshots is saved once: a snapshot of a map that changed a few
/// entries costs the nodes on their paths, and a map that did not change,
/// a reference); each entry's version made again when loaded.
impl<K, V> partex_engine::persist::Persist for VMap<K, V>
where
    K: Clone + Eq + Hash + Ord + partex_engine::persist::Persist + Send + Sync + 'static,
    V: Clone + Hash + partex_engine::persist::Persist + Send + Sync + 'static,
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.len().save(s);
        if let Some(r) = self.0.root() {
            save_node(r, s);
        }
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let n = usize::load(l)?;
        let root = if n > 0 { Some(load_node(l)?) } else { None };
        Some(Self(PMap::from_root(root, n)?))
    }
}

type MapNode<K, V> = partex_ssa::pmap::HNode<K, Ver<V>>;

/// A [`VMap`]'s node: its slots, then each entry (a leaf, a node below,
/// or keys whose hashes collide).
fn save_node<K, V>(n: &Arc<MapNode<K, V>>, s: &mut partex_engine::persist::Saver)
where
    K: Clone + Eq + Hash + partex_engine::persist::Persist + Send + Sync + 'static,
    V: Clone + Hash + partex_engine::persist::Persist + Send + Sync + 'static,
{
    use partex_engine::persist::{Persist, Pin};
    use partex_ssa::pmap::Entry;
    s.share(
        Arc::as_ptr(n).cast::<()>() as usize,
        1,
        "map node",
        || Pin::Send(alloc::boxed::Box::new(n.clone())),
        |s| {
            n.bitmap().save(s);
            for e in n.entries() {
                match e {
                    Entry::Leaf(_, k, v) => {
                        s.enc.u8(0);
                        k.save(s);
                        v.v.save(s);
                    }
                    Entry::Sub(sub) => {
                        s.enc.u8(1);
                        save_node(sub, s);
                    }
                    Entry::Collide(xs) => {
                        s.enc.u8(2);
                        xs.len().save(s);
                        for (_, k, v) in xs {
                            k.save(s);
                            v.v.save(s);
                        }
                    }
                }
            }
        },
    );
}

fn load_node<K, V>(l: &mut partex_engine::persist::Loader) -> Option<Arc<MapNode<K, V>>>
where
    K: Clone + Eq + Hash + partex_engine::persist::Persist + Send + Sync + 'static,
    V: Clone + Hash + partex_engine::persist::Persist + Send + Sync + 'static,
{
    use partex_engine::persist::Persist;
    use partex_ssa::pmap::{Entry, HNode};
    let n = l.share(|l| {
        let bitmap = u32::load(l)?;
        let mut entries = Vec::with_capacity(bitmap.count_ones() as usize);
        for _ in 0..bitmap.count_ones() {
            entries.push(match l.dec.u8()? {
                0 => {
                    let k = K::load(l)?;
                    Entry::leaf(k, Ver::new(V::load(l)?))
                }
                1 => Entry::Sub(load_node(l)?),
                2 => {
                    let m = usize::load(l)?;
                    let mut kvs = Vec::with_capacity(m.min(64));
                    for _ in 0..m {
                        let k = K::load(l)?;
                        kvs.push((k, Ver::new(V::load(l)?)));
                    }
                    Entry::collide(kvs)
                }
                _ => return None,
            });
        }
        Some(Arc::new(HNode::from_parts(bitmap, entries)?))
    })?;
    l.note(Arc::as_ptr(&n).cast::<()>() as usize, 1, &n);
    Some(n)
}

/// A persistent set, versioned as [`VMap`].
#[derive(Clone)]
pub(crate) struct VSet<K>(VMap<K, ()>);

impl<K: Clone + Eq + Hash> Default for VSet<K> {
    fn default() -> Self {
        Self(VMap::default())
    }
}

impl<K: Clone + Eq + Hash> VSet<K> {
    #[must_use]
    pub(crate) fn contains(&self, k: &K) -> bool {
        self.0.contains_key(k)
    }

    /// Add `k`: whether it was not there.
    pub(crate) fn insert(&mut self, k: K) -> bool {
        self.0.insert(k, ()).is_none()
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub(crate) fn version(&self) -> u128 {
        self.0.version()
    }

    /// The elements in order.
    #[must_use]
    pub(crate) fn sorted(&self) -> Vec<&K>
    where
        K: Ord,
    {
        self.0.sorted().into_iter().map(|(k, ())| k).collect()
    }
}

impl<K: Clone + Eq + Hash> PartialEq for VSet<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K: Clone + Eq + Hash> Eq for VSet<K> {}

impl<K: Clone + Eq + Hash> Hash for VSet<K> {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        self.0.hash(h);
    }
}

impl<K: Clone + Eq + Hash + Ord + core::fmt::Debug> core::fmt::Debug for VSet<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_set().entries(self.sorted()).finish()
    }
}

impl<K> partex_engine::persist::Persist for VSet<K>
where
    K: Clone + Eq + Hash + Ord + partex_engine::persist::Persist + Send + Sync + 'static,
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.0.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(VMap::load(l)?))
    }
}

/// An element of a [`VTab`]: its version by content.
pub(crate) trait Element: Clone {
    fn element_version(&self) -> u128;
}

const BITS: usize = 6;
const CHUNK: usize = 1 << BITS;
const MASK: usize = CHUNK - 1;

/// One element's part of a [`VTab`]'s sum.
#[inline]
fn contribution(i: usize, v: u128) -> u128 {
    Version::node(0x7674_6162, &[Version(i as u128), Version(v)]).0
}

/// A chunk of a [`VTab`]: its elements with their versions.
type Chunk<T> = Arc<Vec<(T, u128)>>;

/// A table by index: the elements in shared chunks of 64, copied on
/// write (a clone shares them all), each element with the version it was
/// made with, the table's version the sum of the elements' parts. A
/// change through [`VTab::get_mut`] versions the element again when its
/// routine is done ([`VTab::settle`]), so a routine that changes an
/// element many times (a font's glyphs) versions it once.
#[derive(Clone)]
pub(crate) struct VTab<T> {
    chunks: Arc<Vec<Chunk<T>>>,
    len: usize,
    sum: u128,
    /// Elements changed since the last [`VTab::settle`] (with versions
    /// on).
    dirty: Vec<u32>,
}

impl<T: Element> Default for VTab<T> {
    fn default() -> Self {
        Self {
            chunks: Arc::default(),
            len: 0,
            sum: 0,
            dirty: Vec::new(),
        }
    }
}

impl<T: Element> VTab<T> {
    #[must_use]
    pub(crate) fn from_vec(items: Vec<T>) -> Self {
        let mut t = Self::default();
        for x in items {
            t.push(x);
        }
        t
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    #[must_use]
    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        (i < self.len).then(|| &self.chunks[i >> BITS][i & MASK].0)
    }

    /// Element `i`, to change: versioned again at [`VTab::settle`].
    #[inline]
    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i >= self.len {
            return None;
        }
        let k = u32::try_from(i).unwrap_or(u32::MAX);
        if versions_on() && self.dirty.last() != Some(&k) {
            self.dirty.push(k);
        }
        let c = Arc::make_mut(Arc::make_mut(&mut self.chunks).get_mut(i >> BITS)?);
        Some(&mut c[i & MASK].0)
    }

    pub(crate) fn push(&mut self, x: T) {
        let i = self.len;
        let v = if versions_on() {
            x.element_version()
        } else {
            0
        };
        let chunks = Arc::make_mut(&mut self.chunks);
        if i & MASK == 0 {
            chunks.push(Arc::new(Vec::with_capacity(CHUNK)));
        }
        Arc::make_mut(chunks.last_mut().expect("a chunk")).push((x, v));
        self.len += 1;
        if versions_on() {
            self.sum = self.sum.wrapping_add(contribution(i, v));
        }
    }

    /// Keep the first `n` elements.
    pub(crate) fn truncate(&mut self, n: usize) {
        if n >= self.len {
            return;
        }
        self.settle();
        if versions_on() {
            for i in n..self.len {
                let v = self.chunks[i >> BITS][i & MASK].1;
                self.sum = self.sum.wrapping_sub(contribution(i, v));
            }
        }
        let chunks = Arc::make_mut(&mut self.chunks);
        chunks.truncate(n.div_ceil(CHUNK));
        if let Some(c) = chunks.last_mut() {
            Arc::make_mut(c).truncate(n - (n - 1) / CHUNK * CHUNK);
        }
        self.len = n;
    }

    /// Resize to `n`, new elements `x`.
    pub(crate) fn resize(&mut self, n: usize, x: T) {
        if n < self.len {
            self.truncate(n);
        }
        while self.len + 1 < n {
            self.push(x.clone());
        }
        if self.len < n {
            self.push(x);
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The last element.
    #[must_use]
    pub(crate) fn last(&self) -> Option<&T> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|c| c.iter().map(|e| &e.0))
    }

    /// Every element, to change (each versioned again at the settle).
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        if versions_on() {
            self.dirty = (0..u32::try_from(self.len).unwrap_or(u32::MAX)).collect();
        }
        Arc::make_mut(&mut self.chunks)
            .iter_mut()
            .flat_map(|c| Arc::make_mut(c).iter_mut().map(|e| &mut e.0))
    }

    /// The elements, copied.
    #[must_use]
    pub(crate) fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// The changed elements versioned again (their routine is done).
    pub(crate) fn settle(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let mut d = core::mem::take(&mut self.dirty);
        d.sort_unstable();
        d.dedup();
        let chunks = Arc::make_mut(&mut self.chunks);
        for i in d {
            let i = i as usize;
            if i >= self.len {
                continue;
            }
            let c = Arc::make_mut(&mut chunks[i >> BITS]);
            let e = &mut c[i & MASK];
            let v = e.0.element_version();
            self.sum = self
                .sum
                .wrapping_sub(contribution(i, e.1))
                .wrapping_add(contribution(i, v));
            e.1 = v;
        }
    }

    /// Every element versioned from its content (a table made wholesale:
    /// a plain run's, a loaded state's).
    pub(crate) fn remake(&mut self) {
        self.dirty.clear();
        let mut sum = 0u128;
        let chunks = Arc::make_mut(&mut self.chunks);
        let mut i = 0;
        for c in chunks.iter_mut() {
            for e in Arc::make_mut(c).iter_mut() {
                e.1 = e.0.element_version();
                sum = sum.wrapping_add(contribution(i, e.1));
                i += 1;
            }
        }
        self.sum = sum;
    }

    fn of_sum(&self, sum: u128) -> u128 {
        Version::node(0x7674_6162_0000 ^ self.len as u64, &[Version(sum)]).0
    }

    /// The table's version: from the elements' as made, with those
    /// changed since the last settle versioned now (check mode's test of
    /// a change that bypassed its writer, which then differs from the
    /// version the writer stored).
    #[must_use]
    pub(crate) fn version(&self) -> u128 {
        let mut sum = self.sum;
        let mut seen: Vec<u32> = Vec::new();
        for &i in &self.dirty {
            if seen.contains(&i) {
                continue;
            }
            seen.push(i);
            let i = i as usize;
            if let Some(e) = self.chunks.get(i >> BITS).and_then(|c| c.get(i & MASK)) {
                sum = sum
                    .wrapping_sub(contribution(i, e.1))
                    .wrapping_add(contribution(i, e.0.element_version()));
            }
        }
        self.of_sum(sum)
    }

    /// The version made from every element's content (check mode).
    #[must_use]
    pub(crate) fn content_version(&self) -> u128 {
        let sum = self.iter().enumerate().fold(0u128, |s, (i, e)| {
            s.wrapping_add(contribution(i, e.element_version()))
        });
        self.of_sum(sum)
    }
}

impl<T: Element> core::ops::Index<usize> for VTab<T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        assert!(
            i < self.len,
            "index {i} out of range for length {}",
            self.len
        );
        &self.chunks[i >> BITS][i & MASK].0
    }
}

impl<T: Element> core::ops::IndexMut<usize> for VTab<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        let len = self.len;
        self.get_mut(i)
            .unwrap_or_else(|| panic!("index {i} out of range for length {len}"))
    }
}

impl<'a, T: Element> IntoIterator for &'a VTab<T> {
    type Item = &'a T;
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = &'a T> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

impl<T: Element + PartialEq> PartialEq for VTab<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<T: Element + Hash> Hash for VTab<T> {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        self.len.hash(h);
        for e in self {
            e.hash(h);
        }
    }
}

impl<T: Element + core::fmt::Debug> core::fmt::Debug for VTab<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Saved as it is kept: its chunks, shared (`persist::save_seq`: a
/// snapshot of a table that changed a few elements costs their chunks),
/// each element with its version, and the table's sum. A table with
/// elements changed since its last settle is saved settled.
impl<T: Element + partex_engine::persist::Persist + Send + Sync + 'static>
    partex_engine::persist::Persist for VTab<T>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        if !self.dirty.is_empty() {
            let mut t = self.clone();
            t.settle();
            t.save(s);
            return;
        }
        partex_engine::persist::save_seq(&self.chunks, s);
        self.len.save(s);
        self.sum.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let chunks: Vec<Chunk<T>> = partex_engine::persist::load_seq(l)?;
        let len = usize::load(l)?;
        let sum = u128::load(l)?;
        // (every chunk full but the last, which is not empty)
        let shaped = chunks.len() == len.div_ceil(CHUNK)
            && chunks
                .iter()
                .enumerate()
                .all(|(i, c)| c.len() == (len - i * CHUNK).min(CHUNK));
        shaped.then(|| Self {
            chunks: Arc::new(chunks),
            len,
            sum,
            dirty: Vec::new(),
        })
    }
}

/// A record of the output family: the version of its part `i` (a record
/// with one part has part 0), made from its content.
pub(crate) trait Record: Clone {
    /// How many parts it has (at most 4).
    const PARTS: usize = 1;
    fn part_version(&self, part: usize) -> u128;
}

/// Records versioned by their hash (every field in it).
macro_rules! record_by_hash {
    ($($t:ty),* $(,)?) => {$(
        impl $crate::pdf::val::Record for $t {
            fn part_version(&self, _: usize) -> u128 {
                partex_ssa::Version::of(self).0
            }
        }
    )*};
}

pub(crate) use record_by_hash;

record_by_hash!(Option<Vec<i32>>, Option<Vec<u8>>);

/// A record of the output family: owned while the writer changes it in
/// place, shared once its routine is done and its version made
/// ([`Val::share`]), so the value a record holds is this `Arc` with its
/// version and putting it back is one pointer store ([`Val::put`]). The
/// first change after a share takes the value back, copying it only if
/// a record still holds it.
pub(crate) struct Val<T> {
    /// The value while its writer changes it.
    own: Option<alloc::boxed::Box<T>>,
    /// The value once shared, with its parts' versions (exactly one of
    /// the two is there).
    shared: Option<(Arc<T>, [u128; 4])>,
}

impl<T> Val<T> {
    pub(crate) fn new(v: T) -> Self {
        Val {
            own: Some(alloc::boxed::Box::new(v)),
            shared: None,
        }
    }
}

impl<T: Record> Val<T> {
    /// The value, shared, with its versions made now if it changed.
    pub(crate) fn share(&mut self) -> Arc<T> {
        if let Some(b) = self.own.take() {
            let mut v = [0; 4];
            for (i, x) in v.iter_mut().enumerate().take(T::PARTS) {
                *x = b.part_version(i);
            }
            self.shared = Some((Arc::from(b), v));
        }
        self.shared.as_ref().expect("a value").0.clone()
    }

    /// Put `a`, whose versions are `v`, back (a hit's store).
    pub(crate) fn put(&mut self, a: Arc<T>, v: [u128; 4]) {
        self.own = None;
        self.shared = Some((a, v));
    }

    /// Part `i`'s version: as made at the share, or (changed since) from
    /// the content now.
    #[must_use]
    pub(crate) fn version(&self, i: usize) -> u128 {
        match &self.shared {
            Some((_, v)) => v[i],
            None => self.part_version(i),
        }
    }

    #[cold]
    #[inline(never)]
    fn take_back(&mut self) {
        if let Some((a, _)) = self.shared.take() {
            let v = Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone());
            self.own = Some(alloc::boxed::Box::new(v));
        }
    }
}

impl<T> core::ops::Deref for Val<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        match &self.own {
            Some(b) => b,
            None => &self.shared.as_ref().expect("a value").0,
        }
    }
}

impl<T: Record> core::ops::DerefMut for Val<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        if self.own.is_none() {
            self.take_back();
        }
        self.own.as_deref_mut().expect("a value")
    }
}

impl<T: Clone> Clone for Val<T> {
    fn clone(&self) -> Self {
        Val {
            own: self.own.clone(),
            shared: self.shared.clone(),
        }
    }
}

impl<T: Default> Default for Val<T> {
    fn default() -> Self {
        Val::new(T::default())
    }
}

impl<T: PartialEq> PartialEq for Val<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Eq> Eq for Val<T> {}

impl<T: Hash> Hash for Val<T> {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        (**self).hash(h);
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Val<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

/// Saved as the shared value it is (each once, however many snapshots
/// hold it) with its parts' versions; loaded shared, as it was.
impl<T: Record + partex_engine::persist::Persist + Send + Sync + 'static>
    partex_engine::persist::Persist for Val<T>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        let a = match (&self.shared, &self.own) {
            (Some((a, _)), _) => a.clone(),
            (None, Some(b)) => Arc::new((**b).clone()),
            (None, None) => unreachable!("a value"),
        };
        a.save(s);
        for i in 0..T::PARTS {
            self.version(i).save(s);
        }
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let a = Arc::<T>::load(l)?;
        let mut v = [0; 4];
        for x in v.iter_mut().take(T::PARTS) {
            *x = u128::load(l)?;
        }
        Some(Val {
            own: None,
            shared: Some((a, v)),
        })
    }
}

/// A destination name and its object (`dest_names`' element).
impl Element for (Arc<[u8]>, i32) {
    fn element_version(&self) -> u128 {
        Version::of(self).0
    }
}

/// An open writer scope (not state: empty at every call boundary).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Scope {
    depth: u32,
    /// The fields the open scopes wrote, by [`bit`].
    dirty: u64,
}

impl partex_engine::persist::Persist for Scope {
    fn save(&self, _: &mut partex_engine::persist::Saver) {}
    fn load(_: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self::default())
    }
}

/// A scalar field's version: its value.
fn int(v: i32) -> u128 {
    crate::track::scalar_version_i32(v)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The address of field `f`.
    fn writer_row(f: u8) -> Row {
        if f >= DVI {
            Row::Dvi(f - DVI)
        } else {
            Row::Pdf(f)
        }
    }

    /// A read of field `f` at the access: the version its last writer
    /// made (in check mode tested against the content). A field the open
    /// scope writes is the call's own.
    #[inline]
    pub(crate) fn writer_read(&self, f: u8) {
        if T::VALUES && self.pdf.scope.dirty & bit(f) == 0 {
            self.tracker
                .row_read(Self::writer_row(f), || self.writer_content(f));
        }
    }

    /// Field `f` was stored whole at the access: its version, made now
    /// from the value. Inside a scope that writes it, the scope's end
    /// versions it.
    #[inline]
    pub(crate) fn writer_wrote(&mut self, f: u8) {
        if T::VALUES && self.pdf.scope.dirty & bit(f) == 0 {
            self.writer_settle(f);
        }
    }

    /// Run `body`, a routine of the writers that reads the fields
    /// `reads` and writes the fields `writes` (by [`bit`]; a write is a
    /// read too, a table changed in part): the fields are read at its
    /// start, and the ones it wrote versioned from their parts when the
    /// outermost scope ends.
    #[inline]
    pub(crate) fn writer_scope<R>(
        &mut self,
        reads: u64,
        writes: u64,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if !T::VALUES {
            return body(self);
        }
        self.writer_enter(reads | writes, writes);
        let r = body(self);
        self.writer_leave();
        r
    }

    /// Run `body` as recorded call `f` named `name` (DESIGN 7.17.2),
    /// begun inside whatever writer scope is open, and itself the
    /// outermost scope of what its body writes: the fields its parts'
    /// scopes wrote are versioned when it ends, as its writes.
    pub(crate) fn scoped_call<R>(
        &mut self,
        f: crate::ssa::Func,
        name: u128,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if !T::VALUES {
            return body(self);
        }
        let outer = self.writer_child_begin();
        self.tracker.call_begin(f, &[name], self);
        self.writer_enter(0, 0);
        let r = body(self);
        self.writer_leave();
        self.obj_flush();
        self.tracker.call_end(self);
        self.pdf.scope = outer;
        r
    }

    /// Run `body` as recorded call `f` named `name`, a routine whose hit
    /// is applied (DESIGN 7.17.3, "Hits applied inside a step that runs
    /// again"), its body a writer scope that reads the fields `reads` and
    /// writes `writes`, begun as [`Tex::scoped_call`] begins. The step's
    /// effects in the link's form are cut into chunks at its start and,
    /// before its writes are versioned, at its end (item 2). A hit's
    /// writes are stored and its effects emitted again, and its body does
    /// not run.
    pub(crate) fn applied_call(
        &mut self,
        f: crate::ssa::Func,
        name: u128,
        reads: u64,
        writes: u64,
        body: impl FnOnce(&mut Self) -> Result<(), crate::tex::Jump>,
    ) -> Result<(), crate::tex::Jump> {
        if !T::VALUES {
            return body(self);
        }
        crate::ssa::cut_chunk(self);
        let outer = self.writer_child_begin();
        if let Some(id) = self.tracker.call_begin(f, &[name], self) {
            crate::ssa::apply_hit(self, id);
            self.pdf.scope = outer;
            return Ok(());
        }
        // (the objects it makes are named by its name, not by the step's
        // count: a hit applied makes none)
        let seq = self.pdf.objs.ssa_call_seed(name);
        self.writer_enter(reads | writes, writes);
        let r = body(self);
        crate::ssa::cut_chunk(self);
        self.writer_leave();
        self.pdf.objs.ssa_call_seed_end(seq);
        self.tracker.call_end(self);
        self.pdf.scope = outer;
        r
    }

    /// Before a call begun inside the open scope: the fields the scope
    /// wrote so far versioned now, as its writes before the child, and no
    /// scope open in the child. The scope, returned, is reopened when the
    /// child ends; its end versions its fields again as they are then.
    fn writer_child_begin(&mut self) -> Scope {
        self.obj_flush();
        let outer = self.pdf.scope;
        let mut left = outer.dirty;
        while left != 0 {
            let f = u8::try_from(left.trailing_zeros()).unwrap_or(0);
            left &= left - 1;
            self.writer_settle(f);
        }
        self.pdf.scope = Scope::default();
        outer
    }

    fn writer_enter(&mut self, reads: u64, writes: u64) {
        // (with virtual numbers the lookup trees' and the destinations'
        // fields are not read or written: their entries are slots)
        let (reads, writes) = if self.pdf.objs.ssa.on {
            let cells = !(bit(field::OBJ_TREES) | bit(field::DESTS));
            (reads & cells, writes & cells)
        } else {
            (reads, writes)
        };
        let mut fresh = reads & !self.pdf.scope.dirty;
        while fresh != 0 {
            let f = u8::try_from(fresh.trailing_zeros()).unwrap_or(0);
            fresh &= fresh - 1;
            self.tracker
                .row_read(Self::writer_row(f), || self.writer_content(f));
        }
        self.pdf.scope.dirty |= writes;
        self.pdf.scope.depth += 1;
    }

    fn writer_leave(&mut self) {
        self.pdf.scope.depth -= 1;
        if self.pdf.scope.depth > 0 {
            return;
        }
        self.obj_flush();
        let d = self.pdf.scope.dirty;
        let mut left = d;
        while left != 0 {
            let f = u8::try_from(left.trailing_zeros()).unwrap_or(0);
            left &= left - 1;
            self.writer_settle(f);
        }
        self.pdf.scope.dirty = 0;
    }

    /// Field `f` is made: its changed elements versioned, a record
    /// shared, its version made from its parts and given to the tracker.
    fn writer_settle(&mut self, f: u8) {
        use field::*;
        let p = &mut self.pdf;
        match f {
            LAST_MATCH => drop(p.last_match.share()),
            OBJS => p.objs.tab.settle(),
            DESTS => p.objs.dest_names.settle(),
            OUT => drop(p.out.share()),
            INFO_TOKS => drop(p.info_toks.share()),
            CATALOG_TOKS => drop(p.catalog_toks.share()),
            NAMES_TOKS => drop(p.names_toks.share()),
            TRAILER_TOKS => drop(p.trailer_toks.share()),
            TRAILER_ID_TOKS => drop(p.trailer_id_toks.share()),
            SPACE_FONT_NAME => drop(p.space_font_name.share()),
            STACKS => drop(p.stacks.share()),
            SHIP => drop(p.ship.st.share()),
            PDF_FONTS => p.ship.fonts.0.settle(),
            FONTW => drop(p.fontw.share()),
            EPDF => drop(p.epdf.share()),
            dvi_field::FONTS | dvi_field::TOTALS | dvi_field::WRITER => {
                if let Some(w) = self.dvi.writer.as_mut() {
                    drop(w.share());
                }
            }
            _ => {}
        }
        let v = self.writer_version(f);
        self.tracker.row_wrote(Self::writer_row(f), v);
    }

    /// Every field versioned from its content, as made wholesale (the
    /// engine as made, a format's load: `Tex::version_tables`), not a
    /// write of the running call.
    pub(crate) fn version_writers(&mut self) {
        if !T::VALUES {
            return;
        }
        self.pdf.objs.tab.remake();
        self.pdf.objs.dest_names.remake();
        self.pdf.ship.fonts.0.remake();
        self.pdf.scope = Scope::default();
        for f in (0..field::COUNT).chain(DVI..dvi_field::END) {
            self.tracker
                .row_made(Self::writer_row(f), self.writer_version(f));
        }
        // (the streams and the random generator, the output family's
        // other rows)
        self.version_streams();
    }

    /// Field `f`'s version now, made from its parts: a table's from the
    /// versions its elements were made with, a record's as made when it
    /// was shared (or, changed since, from its content).
    #[must_use]
    pub(crate) fn writer_version(&self, f: u8) -> u128 {
        use field::*;
        let p = &self.pdf;
        match f {
            LAST_MATCH => p.last_match.version(0),
            OBJS => p.objs.table_version(),
            OBJ_TREES => p.objs.trees_version(),
            DESTS => p.objs.dest_names.version(),
            OUT => p.out.version(0),
            OBJ_COUNT => int(p.obj_count),
            XFORM_COUNT => int(p.xform_count),
            XIMAGE_COUNT => int(p.ximage_count),
            k if (LAST..LAST + 10).contains(&k) => {
                int(p.last(super::PdfLast::ALL[usize::from(k - LAST)]))
            }
            INFO_TOKS => p.info_toks.version(0),
            CATALOG_TOKS => p.catalog_toks.version(0),
            NAMES_TOKS => p.names_toks.version(0),
            TRAILER_TOKS => p.trailer_toks.version(0),
            TRAILER_ID_TOKS => p.trailer_id_toks.version(0),
            CATALOG_OPENACTION => int(p.catalog_openaction),
            OUTLINES => Version::of(&(p.first_outline, p.last_outline, p.parent_outline)).0,
            SPACE_FONT_NAME => p.space_font_name.version(0),
            FONT_ATTR => p.font_attr.version(),
            NOBUILTIN_TOUNICODE => p.nobuiltin_tounicode.version(),
            STACKS => p.stacks.version(0),
            SHIP => p.ship.st.version(0),
            PDF_FONTS => p.ship.fonts.0.version(),
            ENCODINGS => p.ship.encodings.version(),
            FONTW => p.fontw.version(0),
            TOUNICODE => self.tounicode.version(),
            FONTMAP => self.fontmap.version(),
            FONTS_MAPPED => self.fonts_mapped.version(),
            EPDF => p.epdf.version(0),
            dvi_field::FILE => self.dvi.file_version(),
            dvi_field::FONTS | dvi_field::TOTALS | dvi_field::WRITER => self
                .dvi
                .writer
                .as_ref()
                .map_or(0, |w| w.version(usize::from(f - dvi_field::FONTS))),
            _ => 0,
        }
    }

    /// A step begins (SSA mode, virtual numbers): its objects are named
    /// by it.
    pub(crate) fn obj_step_begin(&mut self) {
        if T::VALUES && self.pdf.objs.ssa.on {
            let s = self.tracker.open_step();
            self.pdf.objs.ssa_step_begin(s);
        }
    }

    /// The step ends (SSA mode, virtual numbers): its objects' reads and
    /// writes noted, and its numbering events its append
    /// (`Row::PdfNum`), or none.
    pub(crate) fn obj_step_end(&mut self) {
        if !T::VALUES || !self.pdf.objs.ssa.on {
            return;
        }
        self.obj_flush();
        let Some(n) = self.pdf.objs.ssa.step.take() else {
            return;
        };
        let ev = core::mem::take(&mut self.pdf.objs.ssa.events);
        if ev.is_empty() {
            self.pdf.objs.ssa.steplogs.remove(&n);
        } else {
            let a: Arc<[super::vnum::NumEvent]> = ev.into();
            let v = Version::of(&(b"pdfnum", &a[..])).0;
            self.pdf.objs.ssa.steplogs.insert(n, (a, v));
            self.tracker.value_wrote(Row::PdfNum(n));
        }
    }

    /// The object table's reads and writes since the last flush noted, in
    /// order (SSA mode, virtual numbers): each entry and lookup tree
    /// entry a slot, read with the version it had before the first
    /// change, written with its version now.
    pub(crate) fn obj_flush(&mut self) {
        if !T::VALUES || !self.pdf.objs.ssa.on {
            return;
        }
        let (log, overflow, _) = self.pdf.objs.take_log();
        let before = core::mem::take(&mut self.pdf.objs.ssa.before);
        let names = core::mem::take(&mut self.pdf.objs.ssa.names);
        let mut read = alloc::collections::BTreeSet::new();
        let mut wrote = alloc::collections::BTreeSet::new();
        for x in log {
            if x >= 0 {
                if !wrote.contains(&x) && read.insert(x) {
                    let v = before
                        .get(&x)
                        .copied()
                        .unwrap_or_else(|| self.pdf.objs.cell_version(x));
                    self.tracker.value_read(Row::PdfObj(x), || v);
                }
            } else {
                wrote.insert(-1 - x);
            }
        }
        if overflow {
            // (the log ran out of room: every entry read)
            for k in self.pdf.objs.keys() {
                if !wrote.contains(&k) && !read.contains(&k) {
                    let v = before
                        .get(&k)
                        .copied()
                        .unwrap_or_else(|| self.pdf.objs.cell_version(k));
                    self.tracker.value_read(Row::PdfObj(k), || v);
                }
            }
        }
        for k in wrote {
            self.tracker.value_wrote(Row::PdfObj(k));
        }
        let mut nread = alloc::collections::BTreeSet::new();
        let mut nwrote = alloc::collections::BTreeSet::new();
        for (w, key, v) in names {
            if w {
                nwrote.insert(key);
            } else if !nwrote.contains(&key) && nread.insert(key) {
                self.tracker.value_read(Row::PdfName(key), || v);
            }
        }
        for key in nwrote {
            self.tracker.value_wrote(Row::PdfName(key));
        }
        self.pdf.objs.reserve_log();
    }

    /// TeX observes an object's number (SSA mode, virtual numbers): the
    /// numbering of the steps before the open one, each step's events a
    /// read, is made once per run of the step.
    pub(crate) fn obj_observe(&mut self) {
        if !T::VALUES || !self.pdf.objs.ssa.on {
            return;
        }
        self.obj_flush();
        if self.pdf.objs.ssa.prefix.is_none() {
            let ids = self.tracker.steps_before().unwrap_or_default();
            let mut n = super::vnum::Numbering::default();
            let mut reads = Vec::new();
            for id in ids {
                if let Some((ev, v)) = self.pdf.objs.ssa.steplogs.get(&id) {
                    for &e in ev.iter() {
                        n.step(e);
                    }
                    reads.push((id, *v));
                }
            }
            self.pdf.objs.ssa_set_prefix(Arc::new(n), reads.into());
        }
        let reads = self
            .pdf
            .objs
            .ssa
            .prefix
            .as_ref()
            .map(|p| p.1.clone())
            .unwrap_or_default();
        for &(id, v) in reads.iter() {
            self.tracker.value_read(Row::PdfNum(id), || v);
        }
    }

    /// Field `f`'s version from its content now, every element hashed
    /// again (check mode's test of the version its writer made).
    fn writer_content(&self, f: u8) -> u128 {
        use field::*;
        match f {
            OBJS => self.pdf.objs.table_content_version(),
            DESTS => self.pdf.objs.dest_names.content_version(),
            PDF_FONTS => self.pdf.ship.fonts.0.content_version(),
            _ => self.writer_version(f),
        }
    }
}
