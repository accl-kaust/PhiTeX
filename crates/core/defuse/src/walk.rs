//! Bracket walks partitioned by position.
//!
//! A bracket change re-asks the readers after it whose answer moved
//! (`commit::walk_region`). Each walk under way owns a span of the line:
//! from its change to where the depths agree again, to the line's end
//! while that is not known or while it asks every reader after a place. A
//! change inside a span under way is handed to that span's walker, which,
//! done with its own, asks every reader after the first place handed to it
//! (`commit::reask_all_after`); a change in no span walks itself, beside
//! the walks of other spans. No walk waits for another, and no commit
//! waits for a walk.
//!
//! Why exact: a region walk compares the index without its change with
//! the index with it. That is the step from the index before the change
//! was filed to the one after it (the steps telescope, each reader asked
//! against the index as it is when asked) unless a change filed after it
//! moves the chains the walk read: one at or before its stop, as brackets
//! move only the chains after them. [`raced`] finds those, and then every
//! reader after the change is asked, which needs no region; a change
//! handed to a walker is walked that way too.
//!
//! Why: a cold build of a long book files hundreds of bracket changes
//! while others walk; with one engine-wide race check (any change
//! anywhere) each walk went on to the line's end, ~10^6 readers asked
//! each, 280 ms a walk, and a third of the build's wall had no run going
//! (book10 at -j8, 43 s for 5.7 s at -j1).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::api::{Brackets, NodeId};
use crate::commit::CommitStats;
use crate::engine::Engine;
use crate::lock::Mx;
use crate::node::Node;
use crate::order::Pos;
use crate::read;
use crate::sched::Worker;

/// The latest bracket changes (scope count, node, label), and the count
/// of the last one dropped.
pub(crate) type Filings = (std::collections::VecDeque<(u64, NodeId, Pos)>, u64);

/// A walk under way: its start (a node, or the label a node moved from),
/// its end (a node; None: the line's end), the places handed to it.
pub(crate) struct Walk {
    start: (Option<NodeId>, Pos),
    end: Mx<Option<NodeId>>,
    places: Mx<Vec<(Option<NodeId>, Pos)>>,
}

/// Walks under way at once, and the most seen.
static RUNNING: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);

/// The most bracket walks under way at once in this process.
#[must_use]
pub fn walks_most() -> usize {
    MOST.load(Ordering::Relaxed)
}

fn place(eng: &Engine, (n, at): (Option<NodeId>, Pos)) -> Pos {
    n.and_then(|n| read::pos_of(eng, n)).unwrap_or(at)
}

impl Walk {
    /// Its span ends at `end` (None: the line's end).
    pub(crate) fn bound(&self, end: Option<NodeId>) {
        *self.end.lock() = end;
    }

    fn span(&self, eng: &Engine) -> (Pos, Pos) {
        let lo = place(eng, self.start);
        let hi = self
            .end
            .lock()
            .and_then(|n| read::pos_of(eng, n))
            .unwrap_or(Pos(u64::MAX));
        (lo, hi)
    }
}

/// A bracket change of `by` from `old` to `new` at `at` (`own`: `by`
/// stands there; else the label it moved from), walked as the module
/// says. Taken with no segment's lock held: the list of walks is locked
/// before any segment's.
#[allow(clippy::too_many_arguments)]
pub(crate) fn change(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    own: bool,
    at: Pos,
    old: Brackets,
    new: Brackets,
    st: &mut CommitStats,
) {
    let start = (own.then_some(by.id), at);
    let me = {
        let mut g = eng.walks.lock();
        let x = place(eng, start);
        for k in g.iter() {
            let (lo, hi) = k.span(eng);
            if lo <= x && x <= hi {
                k.places.lock().push(start);
                return;
            }
        }
        let me = Arc::new(Walk {
            start,
            end: Mx::new(None),
            places: Mx::new(Vec::new()),
        });
        g.push(me.clone());
        me
    };
    let n = RUNNING.fetch_add(1, Ordering::AcqRel) + 1;
    MOST.fetch_max(n, Ordering::AcqRel);
    crate::commit::walk_region(eng, w, by, at, old, new, &me, st);
    loop {
        let from = {
            let mut g = eng.walks.lock();
            let mut ps = me.places.lock();
            if ps.is_empty() {
                drop(ps);
                g.retain(|k| !Arc::ptr_eq(k, &me));
                break;
            }
            *me.end.lock() = None;
            std::mem::take(&mut *ps)
                .into_iter()
                .map(|p| place(eng, p))
                .min()
                .unwrap_or(at)
        };
        crate::commit::reask_all_after(eng, w, by, from, st);
    }
    RUNNING.fetch_sub(1, Ordering::AcqRel);
}

/// Files the bracket change of `n` at `pos` as scope count `sg` (under
/// its segment's lock: a lock of its own, never held while taking one).
pub(crate) fn filed(eng: &Engine, sg: u64, n: NodeId, pos: Pos) {
    let mut f = eng.filings.lock();
    f.0.push_back((sg, n, pos));
    while f.0.len() > 1 << 16 {
        if let Some(x) = f.0.pop_front() {
            f.1 = x.0;
        }
    }
}

/// Whether a bracket change filed after scope count `sg` stands at or
/// before the stop (`stop` its node, `q` its label then): the region
/// walked for `sg`'s change may be wrong. One older than those kept
/// counts as such.
pub(crate) fn raced(eng: &Engine, sg: u64, stop: Option<NodeId>, q: Pos) -> bool {
    let later: Vec<(NodeId, Pos)> = {
        let f = eng.filings.lock();
        if f.1 > sg {
            return true;
        }
        f.0.iter().filter(|x| x.0 > sg).map(|x| (x.1, x.2)).collect()
    };
    if later.is_empty() {
        return false;
    }
    let q = stop.and_then(|n| read::pos_of(eng, n)).unwrap_or(q);
    later.into_iter().any(|(n, p)| read::pos_of(eng, n).unwrap_or(p) <= q)
}
