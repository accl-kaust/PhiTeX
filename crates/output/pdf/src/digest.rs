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

type Memo = Mutex<HashMap<usize, (Weak<[u8]>, u64)>>;

fn memo() -> &'static Memo {
    static M: OnceLock<Memo> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The content hash of a blob, memoized by its allocation.
fn blob_hash(b: &Arc<[u8]>) -> u64 {
    let key = Arc::as_ptr(b).cast::<u8>() as usize;
    let mut m = memo().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((w, h)) = m.get(&key)
        && w.upgrade().is_some_and(|a| Arc::ptr_eq(&a, b))
    {
        return *h;
    }
    let mut s = DefaultHasher::new();
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

/// An optional blob.
pub(crate) fn opt(b: Option<&Arc<[u8]>>) -> Option<Blob<'_>> {
    b.map(Blob)
}

struct Writer(DefaultHasher);

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

impl PdfState {
    /// A digest of the whole state: two states with equal content have
    /// equal digests, whatever steps made them; a change anywhere in the
    /// state changes it (up to hash collisions).
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut w = Writer(DefaultHasher::new());
        let _ = fmt::write(&mut w, format_args!("{self:?}"));
        w.0.finish()
    }
}
