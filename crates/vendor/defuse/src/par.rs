//! The one module with `unsafe`: shared access that the borrow checker
//! cannot see is disjoint.
//!
//! A wide batch is applied in slices, each by its own overlay engine (see
//! `Engine::apply_sliced`), and the slices are merged in parallel. Three
//! tools serve that, each with its own invariant:
//!
//! - `Base`: the frozen engine an overlay reads through. An overlay holds
//!   only what its slice wrote and reads everything else from the engine
//!   as it was before the batch. Invariant (upheld by
//!   `Engine::apply_sliced`, the only place a `Base` is made): a `Base`
//!   points into an engine that is borrowed shared for the whole life of
//!   every overlay holding it, and the overlays are consumed (their local
//!   parts moved out, their bases never read again) before that engine is
//!   borrowed mutably to merge them.
//! - `scatter`: writes `(index, value)` parts into one slice concurrently;
//!   it checks before writing that no index is out of bounds or repeated.
//! - `Disjoint`: element access to one buffer from several threads, each
//!   reaching only its own elements: the pieces of a split order tree
//!   replayed in parallel (`tree::Arena::pieces`, the only maker).

/// A pointer to the frozen copy of a structure an overlay falls back to.
pub(crate) struct Base<T>(*const T);

// SAFETY: a `Base` is only read through, and only while the structure it
// points to is shared-borrowed and unchanged (see the module invariant);
// sharing or sending it is sharing `&T`, which needs `T: Sync`.
unsafe impl<T: Sync> Send for Base<T> {}
// SAFETY: as above.
unsafe impl<T: Sync> Sync for Base<T> {}

impl<T> Base<T> {
    pub(crate) const fn none() -> Self {
        Base(std::ptr::null())
    }

    pub(crate) fn new(t: &T) -> Self {
        Base(std::ptr::from_ref(t))
    }

    #[inline]
    pub(crate) fn is_none(&self) -> bool {
        self.0.is_null()
    }

    /// The base, if this is an overlay.
    #[inline]
    pub(crate) fn get(&self) -> Option<&T> {
        // SAFETY: non-null only while the base is shared-borrowed and
        // unchanged, for longer than `self` is used (module invariant).
        unsafe { self.0.as_ref() }
    }
}

impl<T> Default for Base<T> {
    fn default() -> Self {
        Self::none()
    }
}

impl<T> Clone for Base<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Base<T> {}

/// Writes `(index, value)` pairs into `dst`, the parts concurrently through
/// `exec`. Panics (before writing anything) if an index is out of bounds
/// or written twice: the parts are then disjoint, so each element has one
/// writer.
pub(crate) fn scatter<T: Send>(dst: &mut [T], parts: Vec<Vec<(usize, T)>>, exec: &mut dyn crate::sched::Executor) {
    let mut all: Vec<usize> = parts.iter().flatten().map(|e| e.0).collect();
    all.sort_unstable();
    assert!(all.windows(2).all(|w| w[0] != w[1]), "scatter: an index written twice");
    assert!(
        all.last().is_none_or(|&i| i < dst.len()),
        "scatter: an index out of bounds"
    );
    let base = Shared(dst.as_mut_ptr());
    let parts: Vec<std::sync::Mutex<Vec<(usize, T)>>> = parts.into_iter().map(std::sync::Mutex::new).collect();
    exec.for_each(parts.len(), &|p| {
        let items = std::mem::take(&mut *parts[p].lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (i, v) in items {
            // SAFETY: `i < dst.len()` and no other part writes `i` (both
            // checked above), and `dst` is borrowed mutably for the whole
            // call: this is the only access to element `i`.
            unsafe {
                *base.get().add(i) = v;
            }
        }
    });
}

/// A pointer shared by the parts of a `scatter`.
struct Shared<T>(*mut T);

impl<T> Shared<T> {
    fn get(&self) -> *mut T {
        self.0
    }
}

// SAFETY: the parts of a `scatter` write disjoint elements (checked), and
// sending values of `T` between threads needs `T: Send`.
unsafe impl<T: Send> Sync for Shared<T> {}

/// Mutable access to the elements of one buffer from several threads, each
/// touching its own elements only: how the pieces of a split tree are
/// written in parallel in place (see `tree::Pieces`).
///
/// Invariant (upheld by `tree::Arena::pieces`, the only maker): the buffer
/// is borrowed mutably for as long as any copy of the view exists, and the
/// holders of the copies access disjoint sets of elements (each piece of a
/// split tree reaches only its own nodes, and allocates only from the
/// nodes reserved for it).
pub(crate) struct Disjoint<T> {
    ptr: *mut T,
    len: usize,
}

impl<T> Clone for Disjoint<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Disjoint<T> {}

// SAFETY: each holder touches only its own elements (the invariant above),
// so sharing the view is sending those elements to one thread each.
unsafe impl<T: Send> Send for Disjoint<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Sync for Disjoint<T> {}

impl<T> Disjoint<T> {
    pub(crate) fn new(v: &mut [T]) -> Self {
        Disjoint {
            ptr: v.as_mut_ptr(),
            len: v.len(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> &T {
        assert!(i < self.len, "disjoint view: index out of bounds");
        // SAFETY: in bounds, the buffer is alive (borrowed for the view's
        // life), and element `i` is this holder's alone (the invariant).
        unsafe { &*self.ptr.add(i) }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub(crate) fn get_mut(&self, i: usize) -> &mut T {
        assert!(i < self.len, "disjoint view: index out of bounds");
        // SAFETY: as in `get`; no other reference to element `i` exists
        // while this one lives, since only this holder reaches it.
        unsafe { &mut *self.ptr.add(i) }
    }
}
