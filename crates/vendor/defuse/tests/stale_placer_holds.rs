//! A node placed by a placer that runs again for a change waits for the
//! placer's placements: a page builder `P` places a finisher after each
//! piece (`Ctx::follow_after_any`) with the piece's height in its
//! arguments; the finisher reads the piece's contribution (the fold since
//! the finisher before it, `Ctx::acc_range`) and must find the height its
//! arguments say. An
//! edit to a piece's height makes both `P` (its fold read) and the
//! finisher (its range read) run again: the finisher waits for `P` to
//! place it again with the new height, instead of running on its old
//! arguments.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

const N: usize = 8;

struct Ops {
    me: OnceLock<OpId>,
    /// Finisher runs whose arguments disagreed with the piece.
    stale: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn vprof(v: &Values) -> Field {
    v.field("acc:vprof")
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) != "piece").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        let f = vprof(v);
        match tag(v, &a).as_str() {
            "pages" => {
                let mut after = None;
                let mut n = 0;
                while let Some((x, h)) = cx.acc_next(f, after)? {
                    // (The piece's own height: the fold since the last.)
                    cx.follow_after_any(x, op, vec![v.str("fin"), v.int(n), h]);
                    after = Some(x);
                    n += 1;
                }
                Ok(v.int(n))
            }
            "fin" => {
                // Its piece: the fold since the finisher before it.
                let prev = cx.prev_sibling_where(&|_, args| v.as_str(args[0]).as_deref() == Some("fin"));
                let h = cx.acc_range(f, prev)?;
                if h != a[2] {
                    self.stale.fetch_add(1, Relaxed);
                }
                Ok(h)
            }
            _ => {
                cx.contribute(f, a[1]);
                Ok(a[1])
            }
        }
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<OnceLock<defuse::RunResult>> = (0..n).map(|_| OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n) {
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
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

#[test]
fn a_finisher_waits_for_its_placer_to_place_it_again() {
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
        stale: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let acc = vprof(eng.values());
    eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let p = vec![eng.values().str("pages")];
    let p = eng.push(u, op, p);
    let piece = |eng: &Engine, h: i64| {
        let v = eng.values();
        vec![v.str("piece"), v.int(h)]
    };
    let ids: Vec<NodeId> = (0..N)
        .map(|i| {
            let a = piece(&eng, i64::try_from(i).unwrap() + 1);
            eng.push(u, op, a)
        })
        .collect();
    let eval = |eng: &mut Engine| {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut Threads(4))
            .expect("evaluate")
    };
    eval(&mut eng);
    assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(N as i64));
    for (k, h) in [(2usize, 30i64), (5, 7), (2, 3)] {
        let a = piece(&eng, h);
        eng.replace(ids[k], op, a);
        let r = eval(&mut eng);
        assert_eq!(ops.stale.load(Relaxed), 0, "piece {k} to {h}: a finisher ran on its old arguments: {r:?}");
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
