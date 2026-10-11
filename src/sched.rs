//! Pluggable scheduling, speculation policy and execution.

use crate::engine::{NIL, NodeId, RunResult, Slot};

/// One unit of work handed to an [`Executor`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Job {
    pub node: NodeId,
    /// May read predicted (not yet final) values, and change program shape
    /// provisionally.
    pub speculative: bool,
    pub(crate) slot: Slot,
}

impl Job {
    #[must_use]
    pub fn new(node: NodeId, speculative: bool) -> Self {
        Job {
            node,
            speculative,
            slot: NIL,
        }
    }
}

/// A node that could run speculatively.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    pub node: NodeId,
    pub(crate) slot: Slot,
    /// It ran before: its previous bindings and structure are predictions.
    pub has_prediction: bool,
    /// It is waiting on a value or pending structure.
    pub waiting: bool,
}

/// Chooses what runs next and what is speculated on. Policy affects speed
/// only, never results.
pub trait Scheduler {
    /// Speculative candidates offered per step (0 disables speculation).
    fn speculation_budget(&self) -> usize {
        0
    }

    /// Global cap on outstanding unconfirmed speculative runs.
    fn max_outstanding(&self) -> usize {
        4096
    }

    /// Step limit for one speculative run.
    fn fuel(&self) -> u64 {
        10_000
    }

    /// Fill `batch` from `ready` (safe to run) and `candidates` (may run
    /// speculatively). At least one ready node always runs.
    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>);
}

/// Runs a batch of jobs, possibly concurrently; results in index order.
pub trait Executor {
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>);
}

/// Runs jobs one after another on the calling thread.
#[derive(Default, Clone, Copy, Debug)]
pub struct Sequential;

impl Executor for Sequential {
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> RunResult + Sync), out: &mut Vec<RunResult>) {
        out.extend((0..n).map(run));
    }
}

/// When to speculate.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Speculation {
    Off,
    /// Only nodes with a prediction from a previous run.
    Predicted,
    /// Any candidate.
    All,
}

/// Runs every ready node; speculates on up to `budget` candidates per step.
#[derive(Clone, Debug)]
pub struct Greedy {
    pub speculation: Speculation,
    pub budget: usize,
    pub outstanding: usize,
    pub fuel: u64,
}

impl Default for Greedy {
    fn default() -> Self {
        Greedy {
            speculation: Speculation::Off,
            budget: 0,
            outstanding: 4096,
            fuel: 10_000,
        }
    }
}

impl Greedy {
    #[must_use]
    pub fn speculative(speculation: Speculation, budget: usize) -> Self {
        Greedy {
            speculation,
            budget,
            ..Greedy::default()
        }
    }
}

impl Scheduler for Greedy {
    fn speculation_budget(&self) -> usize {
        if self.speculation == Speculation::Off {
            0
        } else {
            self.budget
        }
    }

    fn max_outstanding(&self) -> usize {
        self.outstanding
    }

    fn fuel(&self) -> u64 {
        self.fuel
    }

    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>) {
        batch.extend(ready.iter().map(|&n| Job::new(n, false)));
        for c in candidates {
            if self.speculation == Speculation::All || c.has_prediction {
                batch.push(Job::new(c.node, true));
            }
        }
    }
}

/// A small deterministic PRNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n`.
    ///
    /// # Panics
    ///
    /// If `n == 0`.
    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next_u64() % n as u64).expect("fits")
    }

    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next_u64() % den < num
    }
}

/// Random subsets of ready nodes and candidates, random fuel: for checking
/// that results are schedule-independent.
#[derive(Clone, Debug)]
pub struct RandomScheduler {
    pub rng: Rng,
    pub speculate: bool,
    pub budget: usize,
}

impl RandomScheduler {
    #[must_use]
    pub fn new(seed: u64, speculate: bool) -> Self {
        RandomScheduler {
            rng: Rng::new(seed),
            speculate,
            budget: 16,
        }
    }
}

impl Scheduler for RandomScheduler {
    fn speculation_budget(&self) -> usize {
        if self.speculate { self.budget } else { 0 }
    }

    fn max_outstanding(&self) -> usize {
        64
    }

    fn fuel(&self) -> u64 {
        // Sometimes tiny, to exercise fuel exhaustion.
        if self.speculate { 3 + (self.rng.0 % 64) } else { 10_000 }
    }

    fn select(&mut self, ready: &[NodeId], candidates: &[Candidate], batch: &mut Vec<Job>) {
        for &n in ready {
            if self.rng.chance(1, 2) {
                batch.push(Job::new(n, self.speculate && self.rng.chance(1, 4)));
            }
        }
        for c in candidates {
            if self.rng.chance(1, 2) {
                batch.push(Job::new(c.node, true));
            }
        }
        for i in (1..batch.len()).rev() {
            let j = self.rng.below(i + 1);
            batch.swap(i, j);
        }
    }
}
