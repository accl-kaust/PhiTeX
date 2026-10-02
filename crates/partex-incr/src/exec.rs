//! Executors (`DESIGN.md` §7.14): results always in input order, so the
//! schedule cannot influence output. [`Sequential`] is the reference.
//!
//! This is the only implementation: `partex-core` re-exports it, so the
//! engine and the runtime share one trait.

use alloc::vec::Vec;

pub trait Executor: Sync {
    /// Run `a` and `b`, possibly in parallel.
    fn join<A, B, RA, RB>(&self, a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send;

    /// Apply `f` to every item, possibly in parallel; results in input
    /// order.
    fn map<T, R, F>(&self, items: Vec<T>, f: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Sync;

    /// How many items can run at once.
    fn width(&self) -> usize;
}

/// Everything on the calling thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sequential;

impl Executor for Sequential {
    fn join<A, B, RA, RB>(&self, a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        (a(), b())
    }

    fn map<T, R, F>(&self, items: Vec<T>, f: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Sync,
    {
        items.into_iter().map(f).collect()
    }

    fn width(&self) -> usize {
        1
    }
}

/// Scoped `std` threads pulling items from a shared counter (dynamic load
/// balancing: regions differ in cost).
#[cfg(feature = "threads")]
#[derive(Clone, Copy, Debug)]
pub struct Threads(pub usize);

#[cfg(feature = "threads")]
impl Threads {
    /// As many threads as the machine offers.
    #[must_use]
    pub fn available() -> Self {
        Self(std::thread::available_parallelism().map_or(1, core::num::NonZero::get))
    }
}

#[cfg(feature = "threads")]
impl Executor for Threads {
    /// # Panics
    ///
    /// If `b` panics on its thread.
    fn join<A, B, RA, RB>(&self, a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        if self.0 <= 1 {
            return (a(), b());
        }
        std::thread::scope(|s| {
            let hb = s.spawn(b);
            let ra = a();
            (ra, hb.join().expect("join: the second closure panicked"))
        })
    }

    /// # Panics
    ///
    /// If `f` panics on a worker thread.
    fn map<T, R, F>(&self, items: Vec<T>, f: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Sync,
    {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        let n = items.len();
        if n <= 1 || self.0 <= 1 {
            return items.into_iter().map(f).collect();
        }
        let inputs: Vec<Mutex<Option<T>>> =
            items.into_iter().map(|t| Mutex::new(Some(t))).collect();
        let outputs: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        let t = inputs[i].lock().unwrap().take().unwrap();
                        let r = f(t);
                        *outputs[i].lock().unwrap() = Some(r);
                    }
                });
            }
        });
        outputs
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .collect()
    }

    fn width(&self) -> usize {
        self.0.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn order_is_kept() {
        let v: Vec<u32> = (0..100).collect();
        let want: Vec<u32> = v.iter().map(|x| x * 3).collect();
        assert_eq!(Sequential.map(v.clone(), |x| x * 3), want);
        #[cfg(feature = "threads")]
        assert_eq!(Threads(7).map(v, |x| x * 3), want);
        assert_eq!(Sequential.map(vec![1], |x: i32| x), vec![1]);
        assert_eq!(Sequential.map(vec![3, 1, 2], |x| x * 10), vec![30, 10, 20]);
        assert_eq!(Sequential.join(|| 1, || 2), (1, 2));
        #[cfg(feature = "threads")]
        assert_eq!(Threads(2).join(|| 1, || 2), (1, 2));
    }
}
