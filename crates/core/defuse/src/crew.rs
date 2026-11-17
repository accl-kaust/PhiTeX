//! The engine's threads, kept for the life of the process: an
//! evaluation's own thread and its pool's workers run on them, so what a
//! run keeps per thread (its client's state tables, names and decodings)
//! is there for the next evaluation, a watch's next edit, as it was for
//! the run before. A thread made for each evaluation started every edit
//! from empty caches: on the user's Grail paper a word edit's runs grew
//! their state tables from nothing to every control sequence they
//! touched, 25 ms of the edit (a finisher's preload 9 ms of it, 0.2 ms in
//! the cold build).
//!
//! [`scope`] is [`std::thread::scope`] over these threads: each job
//! spawned starts at once on a thread of its own (an idle one, else one
//! made), may borrow from outside the scope, and has ended when `scope`
//! returns. The pool's workers wait for each other, so a job that waited
//! for a thread to come free could hold the evaluation for ever: none
//! waits.
//!
//! [`end`] ends them (a wasm core's end, its host's workers): every thread
//! at rest returns, and every one still working returns once its job is
//! done; a job given after it still runs, on a thread that then returns.

use std::any::Any;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Whether this target runs threads (WebAssembly without atomics,
/// wasm32-wasip1, does not).
const THREADED: bool = !cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));

/// The threads at rest, the one that last worked on top: the next job goes
/// to it, its caches the warmest (one condition shared by all gave a job to
/// the thread that had waited longest, so each edit's runs began cold).
struct Crew {
    idle: Mutex<Vec<Arc<Slot>>>,
    /// The threads made so far ([`Crew::start`] makes up to a count).
    made: std::sync::atomic::AtomicUsize,
    /// [`end`] called: no thread comes to rest again (set with `idle`
    /// held, read with it or with the thread's own slot held).
    ended: AtomicBool,
}

/// A thread's own place to be given a job.
#[derive(Default)]
struct Slot {
    job: Mutex<Option<Job>>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn crew() -> &'static Crew {
    static C: OnceLock<Crew> = OnceLock::new();
    C.get_or_init(Crew::new)
}

/// The process's crew ends ([`Crew::end`]).
pub fn end() {
    crew().end();
}

/// The process's crew made up to `n` threads ([`Crew::start`]).
pub fn start(n: usize) {
    crew().start(n);
}

impl Crew {
    fn new() -> Crew {
        Crew {
            idle: Mutex::new(Vec::new()),
            made: std::sync::atomic::AtomicUsize::new(0),
            ended: AtomicBool::new(false),
        }
    }

    /// The crew's threads end: each at rest returns now, each working once
    /// its job is done, and none comes to rest again (a job given later runs
    /// on a thread of its own, which then returns). For a host that ends the
    /// whole process's threads (a wasm core's end: its workers are the host's
    /// and leave only when their threads return); for good.
    fn end(&self) {
        let idle = {
            let mut idle = lock(&self.idle);
            self.ended.store(true, Ordering::SeqCst);
            std::mem::take(&mut *idle)
        };
        for slot in idle {
            // (under the slot's lock: its thread has checked `ended` and waits, or will see it)
            let _held = lock(&slot.job);
            slot.cv.notify_one();
        }
    }

    /// Threads made, each straight to rest, until `n` have been made in
    /// all (none where there are no threads, nor once ended).
    fn start(&'static self, n: usize) {
        if !THREADED {
            return;
        }
        while !self.ended.load(Ordering::SeqCst)
            && self
                .made
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |m| (m < n).then_some(m + 1))
                .is_ok()
        {
            let made = std::thread::Builder::new()
                .name("defuse-worker".to_owned())
                .stack_size(crate::WORKER_STACK)
                .spawn(move || self.serve());
            if made.is_err() {
                self.made.fetch_sub(1, Ordering::SeqCst);
                return;
            }
        }
    }

    /// `job` on a thread at once: the idle one that worked last, else a new
    /// one it starts on.
    fn submit(&'static self, job: Job) {
        // (a target without threads, WebAssembly without atomics: the job
        // run in place, on the caller's stack, its main stack as large as
        // a worker's: `crates/wasm/cli/build.rs`; only what needs no second
        // thread comes here, the pool at one worker)
        if !THREADED {
            job();
            return;
        }
        let slot = lock(&self.idle).pop();
        if let Some(slot) = slot {
            *lock(&slot.job) = Some(job);
            slot.cv.notify_one();
            return;
        }
        self.made.fetch_add(1, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("defuse-worker".to_owned())
            .stack_size(crate::WORKER_STACK)
            .spawn(move || {
                job();
                self.serve();
            })
            .expect("a worker thread");
    }

    /// A thread's life after its first job: the jobs, one after another.
    fn serve(&'static self) {
        let slot = Arc::new(Slot::default());
        loop {
            {
                let mut idle = lock(&self.idle);
                if self.ended.load(Ordering::SeqCst) {
                    return;
                }
                idle.push(slot.clone());
            }
            let job = {
                let mut j = lock(&slot.job);
                loop {
                    if let Some(job) = j.take() {
                        break job;
                    }
                    // (ended while at rest: taken off the idle list by `end`, no job to come)
                    if self.ended.load(Ordering::SeqCst) {
                        return;
                    }
                    j = slot.cv.wait(j).unwrap_or_else(PoisonError::into_inner);
                }
            };
            job();
        }
    }
}

/// The jobs of a [`scope`] not ended, and the first panic among them.
struct Left {
    n: Mutex<usize>,
    done: Condvar,
    panic: Mutex<Option<Box<dyn Any + Send>>>,
}

/// A job of a scope's: `f` until it has run; dropped, run or not, it
/// drops `f` and only then counts itself ended, so nothing `f` borrows
/// is touched once [`scope`] has returned.
struct Task<F> {
    f: Option<F>,
    left: Arc<Left>,
}

impl<F> Drop for Task<F> {
    fn drop(&mut self) {
        drop(self.f.take());
        let mut n = lock(&self.left.n);
        *n -= 1;
        if *n == 0 {
            self.left.done.notify_all();
        }
    }
}

/// A scope's handle ([`scope`]): jobs spawned on it may borrow what
/// outlives the scope.
pub(crate) struct Scope<'scope, 'env: 'scope> {
    left: Arc<Left>,
    _scope: PhantomData<&'scope mut &'scope ()>,
    _env: PhantomData<&'env mut &'env ()>,
}

impl<'scope> Scope<'scope, '_> {
    /// Runs `f` at once on a thread of the crew's.
    pub(crate) fn spawn<F: FnOnce() + Send + 'scope>(&'scope self, f: F) {
        *lock(&self.left.n) += 1;
        let mut task = Task {
            f: Some(f),
            left: self.left.clone(),
        };
        let job: Box<dyn FnOnce() + Send + 'scope> = Box::new(move || {
            if let Some(f) = task.f.take()
                && let Err(p) = catch_unwind(AssertUnwindSafe(f))
            {
                lock(&task.left.panic).get_or_insert(p);
            }
        });
        // SAFETY: `scope` returns only once every job spawned on it has
        // ended, when its body unwinds too, and a job ends when its
        // `Task` is dropped: after the job has run, or unrun (a thread
        // that could not be made), `f` dropped first (`Task::drop`). So
        // nothing the job borrows ('scope) is used past `scope`, as
        // `std::thread::scope` spawns borrowing closures.
        #[allow(unsafe_code)]
        let job: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Job>(job) };
        crew().submit(job);
    }
}

/// [`std::thread::scope`] on the crew's threads: `f` runs here, the jobs
/// it spawns each on a thread of their own; returns once every one has
/// ended, a panic in `f` or in a job going on from here then.
pub(crate) fn scope<'env, F, T>(f: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let s = Scope {
        left: Arc::new(Left {
            n: Mutex::new(0),
            done: Condvar::new(),
            panic: Mutex::new(None),
        }),
        _scope: PhantomData,
        _env: PhantomData,
    };
    let r = catch_unwind(AssertUnwindSafe(|| f(&s)));
    {
        let mut n = lock(&s.left.n);
        while *n > 0 {
            n = s.left.done.wait(n).unwrap_or_else(PoisonError::into_inner);
        }
    }
    match r {
        Err(p) => resume_unwind(p),
        Ok(t) => {
            if let Some(p) = lock(&s.left.panic).take() {
                resume_unwind(p);
            }
            t
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    /// Tells `Sender` when its thread ends (a thread-local's drop runs as
    /// the thread returns).
    struct Gone(Sender<&'static str>, &'static str);

    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = self.0.send(self.1);
        }
    }

    thread_local! {
        static GONE: std::cell::RefCell<Option<Gone>> = const { std::cell::RefCell::new(None) };
    }

    /// A job that marks its thread: `tx` hears `name` when the thread ends.
    fn marking(tx: &Sender<&'static str>, name: &'static str, ran: Sender<()>) -> Job {
        let tx = tx.clone();
        Box::new(move || {
            GONE.with(|g| *g.borrow_mut() = Some(Gone(tx, name)));
            let _ = ran.send(());
        })
    }

    fn heard(rx: &Receiver<&'static str>) -> Option<&'static str> {
        rx.recv_timeout(Duration::from_secs(10)).ok()
    }

    /// A crew's end ends its threads (a wasm core's workers leave only
    /// when their threads return; issue #46: they waited at rest for ever,
    /// and the host's process did not end): a thread at rest returns at
    /// once, one working returns when its job is done, and a job given
    /// after the end still runs, on a thread that then returns.
    #[test]
    fn end_returns_every_thread() {
        let crew: &'static Crew = Box::leak(Box::new(Crew::new()));
        let (tx, rx) = channel();
        // (one working: its job waits for `go`)
        let (go_tx, go) = channel::<()>();
        let (ran, done) = channel();
        let tx2 = tx.clone();
        crew.submit(Box::new(move || {
            GONE.with(|g| *g.borrow_mut() = Some(Gone(tx2, "working")));
            let _ = ran.send(());
            let _ = go.recv();
        }));
        done.recv_timeout(Duration::from_secs(10)).expect("the job began");
        // (one at rest: another thread, its job done, waiting for the next)
        let (ran, done) = channel();
        crew.submit(marking(&tx, "rested", ran));
        done.recv_timeout(Duration::from_secs(10)).expect("the job ran");
        let t = std::time::Instant::now();
        while lock(&crew.idle).is_empty() {
            assert!(t.elapsed() < Duration::from_secs(10), "the thread came to rest");
            std::thread::yield_now();
        }
        crew.end();
        assert_eq!(heard(&rx), Some("rested"), "the thread at rest returned at the end");
        go_tx.send(()).unwrap();
        assert_eq!(heard(&rx), Some("working"), "the working thread returned after its job");
        let (ran, done) = channel();
        crew.submit(marking(&tx, "after", ran));
        done.recv_timeout(Duration::from_secs(10))
            .expect("a job given after the end ran");
        assert_eq!(heard(&rx), Some("after"), "its thread returned");
        assert!(lock(&crew.idle).is_empty(), "no thread at rest");
    }
}
