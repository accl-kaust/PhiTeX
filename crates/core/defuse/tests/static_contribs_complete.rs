//! A complete static contribution list (`Operation::contribs_complete`):
//! nodes `e` declare all their contributions (to `lines`), so readers `s`
//! of `pos` (named in their read hint) pass them while they are in work:
//! a reader runs in the same batch as the `e` before it. Without the flag
//! the list is per field and a reader is held until every `e` before it
//! is done. Results equal a cold build either way.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

/// The executor's calls so far (the batch a run is in).
static BATCH: AtomicU64 = AtomicU64::new(0);
/// Runs started and ended so far, in order.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// A run: its tag, index, batch, and when it started and ended (`SEQ`).
type Seen = (String, i64, u64, u64, u64);

struct Ops {
    complete: bool,
    seen: Mutex<Vec<Seen>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "s").then(|| vec![v.field("pos")])
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        // (`p` declares its contribution to `pos`; readers none.)
        match tag(v, a).as_str() {
            "e" => Some(vec![(v.field("lines"), v.int(1))]),
            "p" => Some(vec![(v.field("pos"), v.int(3))]),
            _ => Some(Vec::new()),
        }
    }
    fn contribs_complete(&self, _a: &[ValueId], _v: &Values) -> bool {
        self.complete
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let started = SEQ.fetch_add(1, Relaxed);
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        let out = match t.as_str() {
            "e" => {
                cx.contribute(v.field("lines"), v.int(1));
                ValueId::UNIT
            }
            "p" => {
                cx.contribute(v.field("pos"), v.int(3));
                ValueId::UNIT
            }
            _ => cx.acc_range(v.field("pos"), None)?,
        };
        let i = v.as_int(a[1]).unwrap_or(-1);
        self.seen.lock().unwrap().push((t, i, BATCH.load(Relaxed), started, SEQ.fetch_add(1, Relaxed)));
        Ok(out)
    }
}

struct Threads;

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        BATCH.fetch_add(1, Relaxed);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("every job ran")));
    }
}

fn build(complete: bool) -> (Engine, Arc<Ops>, Vec<phitex_defuse::NodeId>) {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops {
        complete,
        seen: Mutex::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let (l, p) = (eng.values().field("lines"), eng.values().field("pos"));
    eng.declare_accumulator(l, Arc::new(phitex_defuse::SumMonoid));
    eng.declare_accumulator(p, Arc::new(phitex_defuse::SumMonoid));
    let u = eng.new_unit();
    let a = vec![eng.values().str("p"), eng.values().int(0)];
    eng.push(u, op, a);
    let mut rs = Vec::new();
    for i in 0..20 {
        let a = vec![eng.values().str("e"), eng.values().int(i)];
        eng.push(u, op, a);
        let a = vec![eng.values().str("s"), eng.values().int(i)];
        rs.push(eng.push(u, op, a));
    }
    (eng, ops, rs)
}

fn outs(eng: &Engine, rs: &[phitex_defuse::NodeId]) -> Vec<Option<i64>> {
    rs.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_complete_list_lets_readers_of_other_fields_pass() {
    let (mut cold, _, crs) = build(false);
    cold.evaluate_sequential().unwrap();
    let want = outs(&cold, &crs);
    assert!(want.iter().all(|x| *x == Some(3)), "{want:?}");
    for complete in [false, true] {
        let (mut eng, ops, rs) = build(complete);
        let r = eng
            .evaluate(
                &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
                &mut Threads,
            )
            .unwrap();
        assert_eq!(outs(&eng, &rs), want, "complete {complete}");
        assert_eq!(r.stops, 0, "complete {complete}");
        let seen = ops.seen.lock().unwrap().clone();
        let run = |t: &str, i: i64| seen.iter().find(|x| x.0 == t && x.1 == i);
        if complete {
            // Some reader ran in the batch of the `e` just before it.
            let beside = (0..20).any(|i| run("s", i).zip(run("e", i)).is_some_and(|(s, e)| s.2 == e.2));
            assert!(beside, "complete: {seen:?}");
        } else {
            // Every reader started after the `e` before it ended (in a
            // later batch, or after it in the lane).
            let held = (0..20).all(|i| run("s", i).zip(run("e", i)).is_none_or(|(s, e)| s.3 > e.4));
            assert!(held, "not complete: {seen:?}");
        }
    }
}
