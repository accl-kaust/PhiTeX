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

use std::any::Any;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Whether this target runs threads (WebAssembly without atomics,
/// wasm32-wasip1, does not).
const THREADED: bool = !cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));

/// The threads and the jobs not yet taken.
struct Crew {
    st: Mutex<Waiting>,
    cv: Condvar,
}

struct Waiting {
    jobs: VecDeque<Job>,
    /// Threads waiting for a job.
    idle: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn crew() -> &'static Crew {
    static C: OnceLock<Crew> = OnceLock::new();
    C.get_or_init(|| Crew {
        st: Mutex::new(Waiting {
            jobs: VecDeque::new(),
            idle: 0,
        }),
        cv: Condvar::new(),
    })
}

impl Crew {
    /// `job` on a thread at once: an idle one (more idle than jobs
    /// waiting), else a new one it starts on.
    fn submit(&'static self, job: Job) {
        // (a target without threads, WebAssembly without atomics: the job
        // run in place, on the caller's stack, its main stack as large as
        // a worker's: `crates/wasm/build.rs`; only what needs no second
        // thread comes here, the pool at one worker)
        if !THREADED {
            job();
            return;
        }
        let mut st = lock(&self.st);
        if st.idle > st.jobs.len() {
            st.jobs.push_back(job);
            drop(st);
            self.cv.notify_one();
            return;
        }
        drop(st);
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
        loop {
            let job = {
                let mut st = lock(&self.st);
                loop {
                    if let Some(j) = st.jobs.pop_front() {
                        break j;
                    }
                    st.idle += 1;
                    st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
                    st.idle -= 1;
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
