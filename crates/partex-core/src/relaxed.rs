//! Interior mutability that keeps the engine `Sync`: atomics with relaxed
//! ordering, for state that `&self` hooks update (DESIGN.md §5.3).
//!
//! The engine runs on one thread; a snapshot of it may be read by several
//! (the runtime's rounds and link, §7.0, need `Machine: Sync`), and a
//! `Cell` or `RefCell` anywhere in it would forbid that. A relaxed load or
//! store compiles to a plain one, so these cost what `Cell` does.

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize};

macro_rules! relaxed {
    ($(#[$m:meta])* $name:ident, $atomic:ty, $t:ty) => {
        $(#[$m])*
        #[derive(Default)]
        pub(crate) struct $name($atomic);

        impl $name {
            #[inline]
            pub(crate) const fn new(v: $t) -> Self {
                Self(<$atomic>::new(v))
            }
            #[inline]
            pub(crate) fn get(&self) -> $t {
                self.0.load(Relaxed)
            }
            #[inline]
            pub(crate) fn set(&self, v: $t) {
                self.0.store(v, Relaxed);
            }
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                Self::new(self.get())
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                self.get().fmt(f)
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.get() == other.get()
            }
        }

        impl Eq for $name {}
    };
}

relaxed!(
    /// A `Cell<bool>` that is `Sync`.
    Flag,
    AtomicBool,
    bool
);
relaxed!(
    /// A `Cell<usize>` that is `Sync`.
    Word,
    AtomicUsize,
    usize
);
relaxed!(
    /// A `Cell<u64>` that is `Sync`.
    U64,
    AtomicU64,
    u64
);
relaxed!(
    /// A `Cell<u32>` that is `Sync`.
    U32,
    AtomicU32,
    u32
);
relaxed!(
    /// A `Cell<i32>` that is `Sync`.
    I32,
    AtomicI32,
    i32
);

/// A vector of `T` that `&self` may append to up to its capacity, set
/// with [`Log::reserve`] (which takes `&mut self`). An append past it
/// fails. A clone copies what was appended, not the capacity.
#[derive(Default)]
pub(crate) struct Log<T: Slot> {
    items: alloc::vec::Vec<T::Atom>,
    len: Word,
}

/// An element type of [`Log`], stored as atomics.
pub(crate) trait Slot: Copy {
    type Atom: Default;
    fn load(a: &Self::Atom) -> Self;
    fn store(a: &Self::Atom, v: Self);
}

impl Slot for i32 {
    type Atom = I32;
    fn load(a: &I32) -> Self {
        a.get()
    }
    fn store(a: &I32, v: Self) {
        a.set(v);
    }
}

impl Slot for u64 {
    type Atom = U64;
    fn load(a: &U64) -> Self {
        a.get()
    }
    fn store(a: &U64, v: Self) {
        a.set(v);
    }
}

impl Slot for (u64, u64, u32) {
    type Atom = (U64, U64, U32);
    fn load(a: &Self::Atom) -> Self {
        (a.0.get(), a.1.get(), a.2.get())
    }
    fn store(a: &Self::Atom, v: Self) {
        a.0.set(v.0);
        a.1.set(v.1);
        a.2.set(v.2);
    }
}

impl Slot for (u64, u64, u64) {
    type Atom = (U64, U64, U64);
    fn load(a: &Self::Atom) -> Self {
        (a.0.get(), a.1.get(), a.2.get())
    }
    fn store(a: &Self::Atom, v: Self) {
        a.0.set(v.0);
        a.1.set(v.1);
        a.2.set(v.2);
    }
}

impl<T: Slot> Log<T> {
    /// Make room for `n` elements in all.
    pub(crate) fn reserve(&mut self, n: usize) {
        if self.items.len() < n {
            self.items.resize_with(n, T::Atom::default);
        }
    }

    /// Make room for exactly `n` elements in all, as a fresh log given
    /// [`Log::reserve`]`(n)` has, without writing the room it keeps (what
    /// lies past the length is never read).
    pub(crate) fn resize(&mut self, n: usize) {
        self.items.resize_with(n, T::Atom::default);
    }

    /// The room made (a test's).
    #[cfg(test)]
    pub(crate) fn room(&self) -> usize {
        self.items.len()
    }

    /// Append `v`; `false` if there is no room.
    #[inline]
    pub(crate) fn push(&self, v: T) -> bool {
        let n = self.len.get();
        match self.items.get(n) {
            Some(a) => {
                T::store(a, v);
                self.len.set(n + 1);
                true
            }
            None => false,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len.get()
    }

    /// Whether there is a last element and `f` holds for it.
    #[inline]
    pub(crate) fn last_is(&self, f: impl FnOnce(&T) -> bool) -> bool {
        let n = self.len();
        n > 0 && f(&T::load(&self.items[n - 1]))
    }

    /// The last element, if any.
    #[inline]
    pub(crate) fn last(&self) -> Option<T> {
        let n = self.len();
        (n > 0).then(|| T::load(&self.items[n - 1]))
    }

    /// Replace the last element (if any).
    #[inline]
    pub(crate) fn set_last(&self, v: T) {
        let n = self.len();
        if n > 0 {
            T::store(&self.items[n - 1], v);
        }
    }

    pub(crate) fn clear(&self) {
        self.len.set(0);
    }

    /// Drop the last element (if any).
    pub(crate) fn pop(&self) {
        let n = self.len();
        if n > 0 {
            self.len.set(n - 1);
        }
    }

    /// The elements appended, copied out.
    pub(crate) fn to_vec(&self) -> alloc::vec::Vec<T> {
        self.items[..self.len()].iter().map(T::load).collect()
    }
}

impl<T: Slot> Clone for Log<T> {
    fn clone(&self) -> Self {
        let items = self.items[..self.len()]
            .iter()
            .map(|a| {
                let b = T::Atom::default();
                T::store(&b, T::load(a));
                b
            })
            .collect();
        Self {
            items,
            len: self.len.clone(),
        }
    }
}

impl<T: Slot + core::fmt::Debug> core::fmt::Debug for Log<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.to_vec()).finish()
    }
}

impl partex_engine::persist::Persist for Flag {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.get().save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self::new(bool::load(l)?))
    }
}
