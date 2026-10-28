//! A digest of the writer's state: equal for equal states whatever
//! steps made them, so a caller can key a fold on the state itself.
//!
//! The state is hashed through its `Debug` form, field by field. The
//! large immutable blobs (font files, encodings, image data) are hashed
//! by content once, memoized by their allocation (a `Weak` checks the
//! allocation is still the same one).

use std::collections::HashMap;
use std::fmt;
use std::hash::{DefaultHasher, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use crate::PdfState;

/// A 128-bit hash: two `SipHash`es, the second keyed by a word written
/// first (a state's digest is its identity where a link resumes or a
/// tail is kept: 64 bits made a collision a wrong file).
pub(crate) struct H128(DefaultHasher, DefaultHasher);

impl H128 {
    pub(crate) fn new() -> H128 {
        let mut b = DefaultHasher::new();
        b.write_u64(0x9e37_79b9_7f4a_7c15);
        H128(DefaultHasher::new(), b)
    }
    pub(crate) fn write(&mut self, x: &[u8]) {
        self.0.write(x);
        self.1.write(x);
    }
    pub(crate) fn write_u64(&mut self, x: u64) {
        self.0.write_u64(x);
        self.1.write_u64(x);
    }
    pub(crate) fn write_u128(&mut self, x: u128) {
        self.0.write_u128(x);
        self.1.write_u128(x);
    }
    pub(crate) fn write_usize(&mut self, x: usize) {
        self.0.write_usize(x);
        self.1.write_usize(x);
    }
    pub(crate) fn finish(&self) -> u128 {
        (u128::from(self.0.finish()) << 64) | u128::from(self.1.finish())
    }
}

type Memo = Mutex<HashMap<usize, (Weak<[u8]>, u128)>>;

fn memo() -> &'static Memo {
    static M: OnceLock<Memo> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The content hash of a blob, memoized by its allocation.
fn blob_hash(b: &Arc<[u8]>) -> u128 {
    let key = Arc::as_ptr(b).cast::<u8>() as usize;
    let mut m = memo().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((w, h)) = m.get(&key)
        && w.upgrade().is_some_and(|a| Arc::ptr_eq(&a, b))
    {
        return *h;
    }
    let mut s = H128::new();
    s.write_usize(b.len());
    s.write(b);
    let h = s.finish();
    if m.len() > 4096 {
        m.retain(|_, (w, _)| w.strong_count() > 0);
    }
    m.insert(key, (Arc::downgrade(b), h));
    h
}

/// A blob as the digest sees it: its length and content hash.
pub(crate) struct Blob<'a>(pub &'a Arc<[u8]>);

impl fmt::Debug for Blob<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "blob({},{:x})", self.0.len(), blob_hash(self.0))
    }
}

/// Bytes as the digest sees them: their length and content hash.
pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl fmt::Debug for Bytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = H128::new();
        s.write(self.0);
        write!(f, "bytes({},{:x})", self.0.len(), s.finish())
    }
}

/// Token text the state gathers (`\pdfcatalog`, `\pdfinfo` and the
/// rest, concatenated): a digest formats it as its length and content
/// hash ([`Bytes`]), not byte by byte.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Toks(pub Vec<u8>);

impl std::ops::Deref for Toks {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl fmt::Debug for Toks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if FAST.with(std::cell::Cell::get) {
            Bytes(&self.0).fmt(f)
        } else {
            self.0.fmt(f)
        }
    }
}

/// The hash of `x`'s `Debug` form.
pub(crate) fn debug_hash(x: &dyn fmt::Debug) -> u128 {
    let mut w = Writer::default();
    let _ = fmt::write(&mut w, format_args!("{x:?}"));
    w.finish()
}

/// An optional blob.
pub(crate) fn opt(b: Option<&Arc<[u8]>>) -> Option<Blob<'_>> {
    b.map(Blob)
}

/// The formatted text, hashed whole at the end (one hash write, not
/// one for every piece the formatter writes: tens of thousands a
/// digest of the state).
#[derive(Default)]
struct Writer(String);

impl Writer {
    fn finish(&self) -> u128 {
        let mut h = H128::new();
        h.write(self.0.as_bytes());
        h.finish()
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.push_str(s);
        Ok(())
    }
}

impl PdfState {
    /// A digest of the whole state: two states with equal content have
    /// equal digests, whatever steps made them; a change anywhere in the
    /// state changes it (up to hash collisions). The large collections
    /// keep running sums of their elements' hashes, updated by the
    /// elements changed since the last digest, so a step costs the size
    /// of what it changed.
    #[must_use]
    pub fn digest(&mut self) -> u128 {
        let d = self.digest_sans_chars();
        with_chars(d, self)
    }

    /// [`PdfState::digest`] but for the characters the fonts used: two
    /// states that differ only in the characters their pages used (what
    /// only the end of the file, the fonts' subsets, reads) have equal
    /// ones. The characters are kept out of the fonts' element hashes,
    /// so this formats nothing more than the digest does.
    #[must_use]
    pub fn digest_sans_chars(&mut self) -> u128 {
        let (core, os) = self.digest_parts();
        let mut s = H128::new();
        s.write_u128(core);
        s.write_u128(os);
        s.finish()
    }

    /// [`PdfState::digest_sans_chars`] in two parts: the digest of the
    /// state but for the bytes of the objects in the object stream being
    /// filled (their count and length in, their content out), and the
    /// hash of those bytes. They are read only where the stream is
    /// written: two states equal in the first part are equal in all once
    /// the same steps have written that stream ([`PdfState::os_stream`]).
    #[must_use]
    pub fn digest_parts(&mut self) -> (u128, u128) {
        self.refresh_digests();
        SANS_OS.with(|c| c.set(true));
        let core = fast_hash(self);
        SANS_OS.with(|c| c.set(false));
        let mut s = H128::new();
        s.write(self.os_bytes());
        (core, s.finish())
    }

    /// The digest of the whole state formatted in full: the oracle of
    /// [`PdfState::digest`] (equal states, equal digests, for both).
    #[doc(hidden)]
    #[must_use]
    pub fn digest_full(&self) -> u128 {
        let mut w = Writer::default();
        let _ = fmt::write(&mut w, format_args!("{self:?}"));
        with_chars(w.finish(), self)
    }
}

/// `d` with the characters each font used hashed in (the fonts' own
/// formatting leaves them out).
fn with_chars(d: u128, st: &PdfState) -> u128 {
    let mut s = H128::new();
    s.write_u128(d);
    for (f, fs) in st.fonts.iter() {
        if fs.chars.iter().any(|&c| c) {
            s.write_u64(u64::from(*f));
            for c in fs.chars.chunks(64) {
                s.write_u64(c.iter().fold(0u64, |a, &b| (a << 1) | u64::from(b)));
            }
        }
    }
    s.finish()
}

// ---- incremental digests of the large collections ----

thread_local! {
    /// Set while [`PdfState::digest`] formats the state: a tracked
    /// collection then prints its running sum instead of its content.
    static FAST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set while [`PdfState::digest_parts`] formats the state: the object
    /// stream's bytes are then left out (their length kept).
    static SANS_OS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Element hashes computed, by collection (for tests).
    static HASHED: std::cell::RefCell<std::collections::BTreeMap<&'static str, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// The number of element hashes the digests computed in collection
/// `name` on this thread so far (`fonts`, `objs`, ...).
#[cfg(test)]
pub(crate) fn element_hashes(name: &str) -> u64 {
    HASHED.with(|h| h.borrow().get(name).copied().unwrap_or(0))
}

fn element_hash(name: &'static str, x: &dyn fmt::Debug) -> u128 {
    HASHED.with(|h| *h.borrow_mut().entry(name).or_default() += 1);
    let mut w = Writer::default();
    let _ = fmt::write(&mut w, format_args!("{x:?}"));
    w.finish()
}

/// A vector whose digest is kept up to date by the elements changed:
/// the sum of the hashes of (index, element).
#[derive(Clone)]
pub(crate) struct TVec<T> {
    name: &'static str,
    items: Vec<T>,
    hashes: Vec<u128>,
    dirty: Vec<usize>,
    sum: u128,
}

impl<T: fmt::Debug> TVec<T> {
    pub(crate) fn new(name: &'static str) -> Self {
        TVec {
            name,
            items: Vec::new(),
            hashes: Vec::new(),
            dirty: Vec::new(),
            sum: 0,
        }
    }
    pub(crate) fn push(&mut self, x: T) {
        self.dirty.push(self.items.len());
        self.items.push(x);
    }
    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.hashes.clear();
        self.dirty.clear();
        self.sum = 0;
    }
    /// Brings the sum up to date.
    pub(crate) fn refresh(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let mut d = std::mem::take(&mut self.dirty);
        d.sort_unstable();
        d.dedup();
        for i in d {
            let Some(x) = self.items.get(i) else { continue };
            let h = element_hash(self.name, &(i, x));
            if i < self.hashes.len() {
                self.sum = self.sum.wrapping_sub(self.hashes[i]);
                self.hashes[i] = h;
            } else {
                // pushes come in order: the hashes grow to i
                self.hashes.resize(i, 0);
                self.hashes.push(h);
            }
            self.sum = self.sum.wrapping_add(h);
        }
    }
}

impl<T> std::ops::Deref for TVec<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.items
    }
}

impl<T> std::ops::Index<usize> for TVec<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.items[i]
    }
}

impl<T> std::ops::IndexMut<usize> for TVec<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        self.dirty.push(i);
        &mut self.items[i]
    }
}

impl<T: fmt::Debug> fmt::Debug for TVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if FAST.with(std::cell::Cell::get) {
            write!(f, "#{:x}/{}", self.sum, self.items.len())
        } else {
            self.items.fmt(f)
        }
    }
}

/// A map whose digest is kept up to date by the entries changed: the
/// sum of the hashes of (key, value). The entries and their hashes are
/// shared between clones until one of them changes (a writer kept at a
/// record and the writer going on share a table neither changed: the
/// glyph names' Unicode table a format loads, thousands of entries, is
/// copied by no clone).
#[derive(Clone)]
pub(crate) struct TMap<K, V> {
    name: &'static str,
    map: Arc<std::collections::BTreeMap<K, V>>,
    hashes: Arc<std::collections::BTreeMap<K, u128>>,
    dirty: std::collections::BTreeSet<K>,
    sum: u128,
}

impl<K: Ord + Clone + fmt::Debug, V: Clone + fmt::Debug> TMap<K, V> {
    pub(crate) fn new(name: &'static str) -> Self {
        TMap {
            name,
            map: Arc::new(std::collections::BTreeMap::new()),
            hashes: Arc::new(std::collections::BTreeMap::new()),
            dirty: std::collections::BTreeSet::new(),
            sum: 0,
        }
    }
    pub(crate) fn insert(&mut self, k: K, v: V) -> Option<V> {
        self.dirty.insert(k.clone());
        Arc::make_mut(&mut self.map).insert(k, v)
    }
    pub(crate) fn remove(&mut self, k: &K) -> Option<V> {
        if !self.map.contains_key(k) {
            return None;
        }
        self.dirty.insert(k.clone());
        Arc::make_mut(&mut self.map).remove(k)
    }
    pub(crate) fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        if !self.map.contains_key(k) {
            return None;
        }
        self.dirty.insert(k.clone());
        Arc::make_mut(&mut self.map).get_mut(k)
    }
    /// `entry(k).or_insert_with(f)`.
    pub(crate) fn get_or_insert_with(&mut self, k: K, f: impl FnOnce() -> V) -> &mut V {
        self.dirty.insert(k.clone());
        Arc::make_mut(&mut self.map).entry(k).or_insert_with(f)
    }
    /// The map's content as its digest and its size (equal maps, equal
    /// keys): a key to memoize a function of the map by.
    pub(crate) fn content_key(&mut self) -> (u128, usize) {
        self.refresh();
        (self.sum, self.map.len())
    }

    /// Brings the sum up to date.
    pub(crate) fn refresh(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let hashes = Arc::make_mut(&mut self.hashes);
        for k in std::mem::take(&mut self.dirty) {
            if let Some(old) = hashes.remove(&k) {
                self.sum = self.sum.wrapping_sub(old);
            }
            if let Some(v) = self.map.get(&k) {
                let h = element_hash(self.name, &(&k, v));
                self.sum = self.sum.wrapping_add(h);
                hashes.insert(k, h);
            }
        }
    }
}

impl<K, V> std::ops::Deref for TMap<K, V> {
    type Target = std::collections::BTreeMap<K, V>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for TMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if FAST.with(std::cell::Cell::get) {
            write!(f, "#{:x}/{}", self.sum, self.map.len())
        } else {
            self.map.fmt(f)
        }
    }
}

/// Whether the object stream's bytes are left out of the formatting
/// ([`PdfState::digest_parts`]).
pub(crate) fn sans_os() -> bool {
    SANS_OS.with(std::cell::Cell::get)
}

/// Formats `x` with the tracked collections as their sums.
pub(crate) fn fast_hash(x: &dyn fmt::Debug) -> u128 {
    FAST.with(|c| c.set(true));
    let mut w = Writer::default();
    let _ = fmt::write(&mut w, format_args!("{x:?}"));
    FAST.with(|c| c.set(false));
    w.finish()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use phitex_tfm::ScaledFont;

    use super::element_hashes;
    use crate::{FontState, FontType, PdfParams, PdfState};

    fn font(name: &[u8]) -> Arc<FontState> {
        Arc::new(FontState {
            name: name.to_vec(),
            metrics: ScaledFont {
                size: 655_360,
                design_size: 655_360,
                chars: Vec::new(),
                kerns: Vec::new(),
                params: Vec::new(),
            },
            metrics_hash: 0,
            map: None,
            file: Some(Arc::from(name.repeat(1000))),
            enc: None,
            used: false,
            size: 655_360,
            num: 0,
            chars: [false; 256],
            attr: Vec::new(),
            nobuiltin_tounicode: false,
            has_space: false,
            kind: FontType::Real,
            vf: None,
            expand: None,
        })
    }

    fn with_fonts() -> PdfState {
        let mut st = PdfState::new(PdfParams::default());
        for (k, n) in [(1, &b"cmr10"[..]), (2, b"cmbx10"), (3, b"cmti10")] {
            st.fonts.insert(k, font(n));
        }
        st
    }

    /// A step touching one font re-hashes that font only.
    #[test]
    fn one_font_one_hash() {
        let mut st = with_fonts();
        let d0 = st.digest();
        let n0 = element_hashes("fonts");
        st.font_mut(2).chars[65] = true;
        let d1 = st.digest();
        assert_eq!(element_hashes("fonts") - n0, 1);
        assert_ne!(d0, d1);
        // a step touching no font re-hashes none
        let n1 = element_hashes("fonts");
        st.info(b"/T (x)");
        let _ = st.digest();
        assert_eq!(element_hashes("fonts"), n1);
    }

    /// The incremental digest agrees with the full one on equality.
    #[test]
    fn incremental_matches_full() {
        let mut a = with_fonts();
        let mut b = with_fonts();
        let _ = b.digest();
        b.font_mut(1).chars[66] = true;
        let _ = b.digest();
        a.font_mut(1).chars[66] = true;
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.digest_full(), b.digest_full());
        b.font_mut(3).used = true;
        assert_ne!(a.digest(), b.digest());
        assert_ne!(a.digest_full(), b.digest_full());
        // undone: equal again, as the full digest says
        b.font_mut(3).used = false;
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.digest_full(), b.digest_full());
    }
}
