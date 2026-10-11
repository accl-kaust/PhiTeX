//! The thread pool pure applications run on (line breaking, packing, math,
//! page bodies, PDF streams). Implemented by `sched.rs`.
use std::sync::Arc;

/// A task's result, joined later. Joining runs other queued tasks while it
/// waits (no deadlock on nested joins).
pub trait Task<T>: Send {
    fn join(self: Box<Self>) -> T;
}

pub trait Pool: Send + Sync {
    /// Runs `f` on some worker (or inline when the pool has one thread).
    fn spawn_dyn(&self, f: Box<dyn FnOnce() -> Box<dyn std::any::Any + Send> + Send>) -> Box<dyn Task<Box<dyn std::any::Any + Send>>>;
    fn threads(&self) -> usize;
}

/// Typed helper over `Pool::spawn_dyn`.
pub fn spawn<T: Send + 'static>(p: &dyn Pool, f: impl FnOnce() -> T + Send + 'static) -> impl FnOnce() -> T {
    let t = p.spawn_dyn(Box::new(move || Box::new(f()) as Box<dyn std::any::Any + Send>));
    move || *t.join().downcast::<T>().expect("task type")
}

/// The pool a build runs with (set by the build; one thread if none).
pub type PoolRef = Arc<dyn Pool>;
