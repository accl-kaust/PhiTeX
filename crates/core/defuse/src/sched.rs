//! Workers: one deque each, stealing, no centre.
//!
//! A node marked dirty by a commit goes on the committing worker's own
//! deque, so the reader of a value runs on the thread that just made it,
//! at once. An idle worker steals from the others, then from the injector
//! (the client's pushes and edits), then parks. The evaluation ends when
//! `outstanding` (dirty plus running nodes) is zero: no step, no round, no
//! barrier. Nothing waits for a node.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_deque::{Injector, Stealer, Worker as Deque};

use crate::api::{EvalError, NodeId};
use crate::engine::Engine;
use crate::lock::Mx;
use crate::node::{Node, Wake};

pub(crate) struct Pool {
    pub(crate) n: usize,
    injector: Injector<NodeId>,
    stealers: Vec<Stealer<NodeId>>,
    /// Dirty plus running nodes; zero ends the evaluation.
    outstanding: AtomicUsize,
    idle: AtomicUsize,
    park: (Mutex<()>, Condvar),
    stop: AtomicBool,
    failed: Mx<Option<EvalError>>,
}

/// One worker's deque and counters (nanoseconds).
pub(crate) struct Worker {
    pub(crate) ix: usize,
    local: Deque<NodeId>,
    pub(crate) runs: AtomicU64,
    pub(crate) op_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) commit_max_ns: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) idle_ns: AtomicU64,
    pub(crate) lock_wait_ns: AtomicU64,
    pub(crate) lock_wait_max_ns: AtomicU64,
    pub(crate) steals: AtomicU64,
    pub(crate) woken: AtomicU64,
    pub(crate) relative_resolved: AtomicU64,
    pub(crate) predictions_confirmed: AtomicU64,
    pub(crate) predictions_refuted: AtomicU64,
    pub(crate) shaped: AtomicU64,
    pub(crate) commits: AtomicU64,
}

impl Pool {
    /// A pool of `n` workers and their deques.
    pub(crate) fn new(n: usize) -> (Pool, Vec<Worker>) {
        todo!()
    }

    /// Runs until no node is dirty or running, on `n` scoped threads, the
    /// caller's thread one of them. The workers come back with their counts.
    pub(crate) fn run(&self, eng: &Engine, workers: Vec<Worker>) -> Result<Vec<Worker>, EvalError> {
        todo!()
    }

    /// Queues a dirty node: on `w`'s deque, or the injector from outside.
    pub(crate) fn push(&self, w: Option<&Worker>, n: NodeId) {
        todo!()
    }

    pub(crate) fn outstanding(&self) -> usize {
        todo!()
    }
}

/// Marks a node dirty for `wake`'s reason and queues it, unless it is
/// already dirty, inert or gone. A running node becomes `RunningDirty`: its
/// reads return `Stop` from now, its commit goes through, and it is queued
/// after. Returns whether it was newly marked.
pub(crate) fn mark_dirty(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    todo!()
}

/// One node's run and commit on worker `w`: state to `Running`, a `Ctx`,
/// the operation (panics caught into `EvalError::OpPanicked`), the commit,
/// then `Clean` or, if re-asked meanwhile, `Dirty` and queued again. A
/// `Stop` from the run is a refuted run: queued again without a commit.
pub(crate) fn work(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
    todo!()
}
