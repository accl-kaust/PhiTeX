//! The one module with `unsafe`: two lock-free containers the engine reads
//! from every worker without taking a lock.
//!
//! - [`Table`]: an insert-only concurrent hash map from non-zero `u64` keys
//!   to values that live as long as the table (the value interner).
//! - [`Arena`]: an append-only vector of fixed-address records (the node and
//!   entry records), readable by index while other threads append.
//!
//! Both never move or free a record before the container is dropped, which
//! is what makes handing out `&T` from a shared reference sound.
//!
//! - [`SendMut`]: a borrowed `&mut T` carried into a mutex the workers
//!   share (the clean core's scheduler in its flow).

use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

/// One generation of the table: open addressing, linear probing. A slot's
/// key is written once (CAS from 0), its value pointer once after that.
struct Inner<T> {
    mask: usize,
    keys: Box<[AtomicU64]>,
    vals: Box<[AtomicPtr<T>]>,
    /// The generation being copied into this one (null once copied).
    prev: AtomicPtr<Inner<T>>,
}

impl<T> Inner<T> {
    fn new(cap: usize) -> Inner<T> {
        Inner {
            mask: cap - 1,
            keys: (0..cap).map(|_| AtomicU64::new(0)).collect(),
            vals: (0..cap).map(|_| AtomicPtr::new(ptr::null_mut())).collect(),
            prev: AtomicPtr::new(ptr::null_mut()),
        }
    }

    fn start(&self, key: u64) -> usize {
        // The keys are hashes already; fold the high bits in.
        #[allow(clippy::cast_possible_truncation)]
        let h = (key ^ (key >> 29) ^ (key >> 47)) as usize;
        h & self.mask
    }

    /// The value pointer for `key`, waiting out an insert in progress.
    fn find(&self, key: u64) -> Option<*mut T> {
        if key == 0 {
            return None;
        }
        let mut i = self.start(key);
        loop {
            let k = self.keys[i].load(Ordering::Acquire);
            if k == key {
                loop {
                    let p = self.vals[i].load(Ordering::Acquire);
                    if !p.is_null() {
                        return Some(p);
                    }
                    std::hint::spin_loop();
                }
            }
            if k == 0 {
                return None;
            }
            i = (i + 1) & self.mask;
        }
    }

    /// Claims a slot for `key`: `Ok(slot)` if this call claimed it,
    /// `Err(pointer)` if the key was there already.
    fn claim(&self, key: u64) -> Result<usize, *mut T> {
        let mut i = self.start(key);
        loop {
            let k = self.keys[i].load(Ordering::Acquire);
            if k == key {
                return Err(self.wait(i));
            }
            if k == 0 {
                match self.keys[i].compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return Ok(i),
                    Err(k2) if k2 == key => return Err(self.wait(i)),
                    Err(_) => {}
                }
            }
            i = (i + 1) & self.mask;
        }
    }

    fn wait(&self, i: usize) -> *mut T {
        loop {
            let p = self.vals[i].load(Ordering::Acquire);
            if !p.is_null() {
                return p;
            }
            std::hint::spin_loop();
        }
    }

    /// Puts `key -> p` in unless the key is there; returns what is there.
    fn put(&self, key: u64, p: *mut T) -> *mut T {
        match self.claim(key) {
            Ok(i) => {
                self.vals[i].store(p, Ordering::Release);
                p
            }
            Err(q) => q,
        }
    }
}

/// Counter stripes: one per thread (round robin), each on its own cache
/// line, so inserting threads do not share one counter's line.
const STRIPES: usize = 64;

#[repr(align(128))]
#[derive(Default)]
struct Stripe(AtomicUsize);

/// This thread's counter stripe.
fn stripe() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static MINE: usize = NEXT.fetch_add(1, Ordering::Relaxed) % STRIPES;
    }
    MINE.with(|m| *m)
}

/// An insert-only concurrent map from non-zero keys to `T`.
pub struct Table<T> {
    cur: AtomicPtr<Inner<T>>,
    count: Box<[Stripe]>,
    grow: Mutex<Vec<*mut Inner<T>>>,
}

// SAFETY: the table owns its values (`T: Send` moves them in from any
// thread) and hands out `&T` to any thread (`T: Sync`); its own state is
// atomics and a mutex.
unsafe impl<T: Send + Sync> Send for Table<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync> Sync for Table<T> {}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Table<T> {
    #[must_use]
    pub fn new() -> Table<T> {
        let first = Box::into_raw(Box::new(Inner::new(1 << 12)));
        Table {
            cur: AtomicPtr::new(first),
            count: (0..STRIPES).map(|_| Stripe::default()).collect(),
            grow: Mutex::new(vec![first]),
        }
    }

    /// The number of values (the inserts that won).
    pub fn len(&self) -> usize {
        self.count.iter().map(|c| c.0.load(Ordering::Relaxed)).sum()
    }

    /// Whether no value is in.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn inner(&self) -> &Inner<T> {
        // SAFETY: every generation stays allocated until the table drops.
        unsafe { &*self.cur.load(Ordering::Acquire) }
    }

    /// The value under `key`.
    pub fn get(&self, key: u64) -> Option<&T> {
        let mut t: *const Inner<T> = self.inner();
        loop {
            // SAFETY: generations are freed only in `drop`.
            let g = unsafe { &*t };
            if let Some(p) = g.find(key) {
                // SAFETY: values are freed only in `drop`.
                return Some(unsafe { &*p });
            }
            let prev = g.prev.load(Ordering::Acquire);
            if prev.is_null() {
                return None;
            }
            t = prev;
        }
    }

    /// The value under `key`, made by `make` if absent (`make` may run and
    /// lose a race; its value is then dropped).
    pub fn get_or_insert(&self, key: u64, make: impl FnOnce() -> T) -> &T {
        debug_assert!(key != 0);
        if let Some(x) = self.get(key) {
            return x;
        }
        let fresh = Box::into_raw(Box::new(make()));
        let mut t = self.cur.load(Ordering::Acquire);
        loop {
            // SAFETY: generations are freed only in `drop`.
            let g = unsafe { &*t };
            let got = g.put(key, fresh);
            if got != fresh {
                // Lost the race: someone else's value stands.
                // SAFETY: `fresh` was never shared.
                drop(unsafe { Box::from_raw(fresh) });
                // SAFETY: values are freed only in `drop`.
                return unsafe { &*got };
            }
            // A newer generation may have been published while we
            // inserted (after its copy passed our slot): insert there too.
            let now = self.cur.load(Ordering::Acquire);
            if now == t {
                break;
            }
            t = now;
        }
        // (Counted on this thread's stripe; the stripes are summed every
        // `step` inserts of it, so a table grows at most a sixteenth past
        // three quarters full.)
        let c = self.count[stripe()].0.fetch_add(1, Ordering::Relaxed) + 1;
        // SAFETY: as above.
        let cap = unsafe { (*t).mask } + 1;
        let step = (cap / (STRIPES * 16)).max(1);
        if c.is_multiple_of(step) && self.len() * 4 > cap * 3 {
            self.grow_from(t);
        }
        // SAFETY: values are freed only in `drop`.
        unsafe { &*fresh }
    }

    fn grow_from(&self, t: *mut Inner<T>) {
        let mut all = self.grow.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.cur.load(Ordering::Acquire) != t {
            return;
        }
        // SAFETY: generations are freed only in `drop`.
        let old = unsafe { &*t };
        // (Four times while small, where growth comes often and is cheap;
        // twice once large, where the slots dominate memory.)
        let grow = if old.mask + 1 < 1 << 20 { 4 } else { 2 };
        let new = Box::into_raw(Box::new(Inner::new((old.mask + 1) * grow)));
        // SAFETY: `new` is fresh and owned by `all` from here.
        unsafe { (*new).prev.store(t, Ordering::Release) };
        all.push(new);
        self.cur.store(new, Ordering::Release);
        // SAFETY: as above.
        let ng = unsafe { &*new };
        for i in 0..=old.mask {
            let k = old.keys[i].load(Ordering::Acquire);
            if k != 0 {
                let p = old.wait(i);
                ng.put(k, p);
            }
        }
        ng.prev.store(ptr::null_mut(), Ordering::Release);
    }
}

impl<T> Drop for Table<T> {
    fn drop(&mut self) {
        let all = std::mem::take(self.grow.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner));
        // The newest generation holds every value (see `get_or_insert`).
        if let Some(&last) = all.last() {
            // SAFETY: we own every generation; `&mut self` excludes readers.
            let g = unsafe { &*last };
            for i in 0..=g.mask {
                if g.keys[i].load(Ordering::Relaxed) != 0 {
                    let p = g.vals[i].load(Ordering::Relaxed);
                    if !p.is_null() {
                        // SAFETY: each value is in the newest generation once.
                        drop(unsafe { Box::from_raw(p) });
                    }
                }
            }
        }
        for t in all {
            // SAFETY: each generation was allocated once with `Box`.
            drop(unsafe { Box::from_raw(t) });
        }
    }
}

/// A `&mut T` that may cross to another thread inside a mutex: the
/// clean core's flow keeps its scheduler (a `dyn Scheduler`, not `Send`)
/// in the mutex its workers share and calls it only while holding it.
#[allow(dead_code)]
pub(crate) struct SendMut<'a, T: ?Sized> {
    p: *mut T,
    _b: std::marker::PhantomData<&'a mut T>,
}

// SAFETY: a `SendMut` is reached only through `&mut self` (so by one thread
// at a time, the one holding the mutex it lives in), it borrows its `T` for
// its whole life, and the `T`s it carries (schedulers) keep no state tied to
// a thread.
unsafe impl<T: ?Sized> Send for SendMut<'_, T> {}

#[allow(dead_code)]
impl<'a, T: ?Sized> SendMut<'a, T> {
    pub(crate) fn new(r: &'a mut T) -> Self {
        SendMut {
            p: std::ptr::from_mut(r),
            _b: std::marker::PhantomData,
        }
    }

    pub(crate) fn with<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: borrowed for 'a and reached only by the holder of
        // `&mut self` (see the `Send` impl).
        f(unsafe { &mut *self.p })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_grows_and_keeps_everything() {
        let t: Table<u64> = Table::new();
        std::thread::scope(|s| {
            for w in 0..4u64 {
                let t = &t;
                s.spawn(move || {
                    for i in 1..20_000u64 {
                        let k = i * 4 + w;
                        assert_eq!(*t.get_or_insert(k, || k * 10), k * 10);
                    }
                });
            }
        });
        for i in 4..80_000u64 {
            assert_eq!(t.get(i).copied(), Some(i * 10), "{i}");
        }
    }

}
