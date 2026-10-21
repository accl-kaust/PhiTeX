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
        if c % step == 0 && self.len() * 4 > cap * 3 {
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

impl<T> Table<T> {
    /// Frees the generations replaced by growth (their values live on in
    /// the newest one): `&mut self` excludes every reader that could
    /// still be walking them.
    pub fn reclaim(&mut self) {
        let all = self.grow.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        if all.len() <= 1 {
            return;
        }
        let last = all.pop().expect("a generation");
        for t in all.drain(..) {
            // SAFETY: each generation was allocated once with `Box`; the
            // newest no longer links to it (`grow_from` clears `prev`) and
            // its values were moved into the newest by pointer.
            drop(unsafe { Box::from_raw(t) });
        }
        all.push(last);
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

const CHUNK_BITS: u32 = 12;
const CHUNK: usize = 1 << CHUNK_BITS;
const MAX_CHUNKS: usize = 1 << 18;

/// An append-only vector with fixed-address records: `get(i)` needs no
/// lock while other threads push.
pub struct Arena<T> {
    chunks: Box<[AtomicPtr<T>]>,
    len: AtomicUsize,
    push: Mutex<()>,
}

// SAFETY: records are created by `push` (moved in from any thread) and
// shared as `&T`.
unsafe impl<T: Send + Sync> Send for Arena<T> {}
// SAFETY: as above.
unsafe impl<T: Send + Sync> Sync for Arena<T> {}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    #[must_use]
    pub fn new() -> Arena<T> {
        Arena {
            chunks: (0..MAX_CHUNKS).map(|_| AtomicPtr::new(ptr::null_mut())).collect(),
            len: AtomicUsize::new(0),
            push: Mutex::new(()),
        }
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// Record `i` (`i < len()`).
    pub fn get(&self, i: usize) -> &T {
        assert!(i < self.len(), "arena index {i} out of range");
        let c = self.chunks[i >> CHUNK_BITS].load(Ordering::Acquire);
        // SAFETY: chunks below `len` are allocated and initialised (the
        // `Release` store of `len` in `push` follows the writes), and are
        // freed only in `drop`.
        unsafe { &*c.add(i & (CHUNK - 1)) }
    }

    /// Appends records made by `make(index)`; returns the first index.
    pub fn push_with(&self, n: usize, mut make: impl FnMut(usize) -> T) -> usize {
        let _g = self.push.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let start = self.len.load(Ordering::Relaxed);
        for i in start..start + n {
            let ci = i >> CHUNK_BITS;
            assert!(ci < MAX_CHUNKS, "arena full");
            let mut c = self.chunks[ci].load(Ordering::Relaxed);
            if c.is_null() {
                let mut v: Vec<std::mem::MaybeUninit<T>> = Vec::with_capacity(CHUNK);
                // SAFETY: MaybeUninit needs no initialisation.
                unsafe { v.set_len(CHUNK) };
                c = Box::into_raw(v.into_boxed_slice()).cast::<T>();
                self.chunks[ci].store(c, Ordering::Release);
            }
            // SAFETY: slot `i` is inside the chunk and not yet visible to
            // readers (`len` has not passed it).
            unsafe { c.add(i & (CHUNK - 1)).write(make(i)) };
        }
        self.len.store(start + n, Ordering::Release);
        start
    }
}

impl<T> Drop for Arena<T> {
    fn drop(&mut self) {
        let n = *self.len.get_mut();
        for ci in 0..MAX_CHUNKS {
            let c = *self.chunks[ci].get_mut();
            if c.is_null() {
                break;
            }
            let live = n.saturating_sub(ci * CHUNK).min(CHUNK);
            for j in 0..live {
                // SAFETY: the first `live` records of this chunk were written.
                unsafe { c.add(j).drop_in_place() };
            }
            // SAFETY: allocated in `push_with` as a boxed slice of CHUNK.
            drop(unsafe {
                Box::from_raw(ptr::slice_from_raw_parts_mut(
                    c.cast::<std::mem::MaybeUninit<T>>(),
                    CHUNK,
                ))
            });
        }
    }
}

/// A borrowed `&T` handed to a watcher thread that the borrower joins
/// before the borrow ends (`observe` builds only).
#[cfg(feature = "observe")]
pub(crate) struct Lent<T: Sync>(*const T);

#[cfg(feature = "observe")]
// SAFETY: `T: Sync`, and the lender joins the thread before `T` goes
// away (`observe::Watch` joins on drop and borrows `T` for its lifetime).
unsafe impl<T: Sync> Send for Lent<T> {}

#[cfg(feature = "observe")]
impl<T: Sync> Lent<T> {
    pub(crate) fn new(t: &T) -> Self {
        Lent(t)
    }
    pub(crate) fn get(&self) -> &T {
        // SAFETY: see the `Send` impl: the referent outlives the thread.
        unsafe { &*self.0 }
    }
}

/// SIGINT / SIGTERM set a flag the watcher polls (`observe`, unix).
#[cfg(all(feature = "observe", unix))]
pub(crate) mod signals {
    use std::sync::atomic::{AtomicI32, Ordering};

    pub(crate) static CAUGHT: AtomicI32 = AtomicI32::new(0);

    unsafe extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }

    extern "C" fn on_signal(sig: i32) {
        CAUGHT.store(sig, Ordering::SeqCst);
    }

    pub(crate) const SIGINT: i32 = 2;
    pub(crate) const SIGTERM: i32 = 15;

    /// Installs the handlers; returns the previous ones.
    pub(crate) fn install() -> [usize; 2] {
        let h = on_signal as extern "C" fn(i32) as usize;
        // SAFETY: `signal` is async-signal-safe to install, and the handler
        // only stores to an atomic.
        unsafe { [signal(SIGINT, h), signal(SIGTERM, h)] }
    }

    /// Puts back what `install` replaced.
    pub(crate) fn restore(prev: [usize; 2]) {
        // SAFETY: restores the handlers `install` returned.
        unsafe {
            signal(SIGINT, prev[0]);
            signal(SIGTERM, prev[1]);
        }
    }
}

/// A read-mostly lock whose readers do not share a cache line: each thread
/// reads under its own padded slot, and a writer takes every slot. For the
/// lists every definition query reads and only work transitions write.
pub(crate) struct BrLock<T> {
    slots: Box<[Padded]>,
    data: std::cell::UnsafeCell<T>,
}

#[repr(align(128))]
struct Padded(std::sync::RwLock<()>);

const BR_SLOTS: usize = 32;

// SAFETY: `data` is reached only under a slot's read lock (shared) or
// under every slot's write lock (exclusive), so it is shared as by a RwLock.
unsafe impl<T: Send + Sync> Sync for BrLock<T> {}
// SAFETY: as above; moving the lock moves the data.
unsafe impl<T: Send> Send for BrLock<T> {}

fn br_slot() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static SLOT: usize = NEXT.fetch_add(1, Ordering::Relaxed) % BR_SLOTS;
    }
    SLOT.with(|s| *s)
}

pub(crate) struct BrRead<'a, T> {
    _g: std::sync::RwLockReadGuard<'a, ()>,
    data: &'a T,
}

impl<T> std::ops::Deref for BrRead<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

pub(crate) struct BrWrite<'a, T> {
    _g: Vec<std::sync::RwLockWriteGuard<'a, ()>>,
    data: &'a mut T,
}

impl<T> std::ops::Deref for BrWrite<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> std::ops::DerefMut for BrWrite<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<T> BrLock<T> {
    pub(crate) fn new(v: T) -> Self {
        BrLock {
            slots: (0..BR_SLOTS).map(|_| Padded(std::sync::RwLock::new(()))).collect(),
            data: std::cell::UnsafeCell::new(v),
        }
    }

    pub(crate) fn read(&self) -> BrRead<'_, T> {
        let g = self.slots[br_slot()]
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: a read slot is held: no writer holds every slot.
        BrRead {
            _g: g,
            data: unsafe { &*self.data.get() },
        }
    }

    pub(crate) fn write(&self) -> BrWrite<'_, T> {
        let g: Vec<_> = self
            .slots
            .iter()
            .map(|s| s.0.write().unwrap_or_else(std::sync::PoisonError::into_inner))
            .collect();
        // SAFETY: every slot is held for writing: no reader, no other writer.
        BrWrite {
            _g: g,
            data: unsafe { &mut *self.data.get() },
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

    #[test]
    fn arena_indexes_stay_put() {
        let a: Arena<String> = Arena::new();
        let s = a.push_with(10_000, |i| i.to_string());
        assert_eq!(s, 0);
        assert_eq!(a.get(9_999), "9999");
        assert_eq!(a.push_with(1, |_| "x".into()), 10_000);
    }
}
