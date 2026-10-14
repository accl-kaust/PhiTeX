//! Range readers re-fold on a re-made node: a reader runs again only when
//! the fold over its window differs from what it saw.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, SumMonoid, ValueId, Values};

/// `["c", i, x, field]` contributes `x` to `field`; `["r", i, field]`
/// outputs the fold of `field` before it. Runs are counted per node index.
struct Acc {
    runs: Vec<AtomicU64>,
}

impl Operation for Acc {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        let out = if v.as_str(a[0]).as_deref() == Some("c") {
            cx.contribute(Field(a[3]), a[2]);
            ValueId::UNIT
        } else {
            cx.acc_range(Field(a[2]), None)?
        };
        self.runs[i].fetch_add(1, Relaxed);
        Ok(out)
    }
}

struct Threads;

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
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

const N: usize = 1200;

fn eval(eng: &mut Engine, sched: &str) {
    match sched {
        "seq" => eng.evaluate_sequential(),
        "spec" => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 4096),
            &mut defuse::Sequential,
        ),
        "random" => eng.evaluate(&mut defuse::RandomScheduler::new(3, true), &mut defuse::Sequential),
        _ => eng.evaluate(
            &mut defuse::Greedy::speculative(defuse::Speculation::All, 4096),
            &mut Threads,
        ),
    }
    .expect("evaluate");
}

/// Every sixth node a reader (of A, or of B for every fourth reader), the
/// rest contributors of 1 to A (B for every hundredth).
fn build() -> (Engine, Arc<Acc>, OpId, Vec<NodeId>, Field, Field) {
    let mut eng = Engine::new();
    let (fa, fb) = (eng.values().field("A"), eng.values().field("B"));
    eng.declare_accumulator(fa, Arc::new(SumMonoid));
    eng.declare_accumulator(fb, Arc::new(SumMonoid));
    let op = Arc::new(Acc {
        runs: (0..N + 8).map(|_| AtomicU64::new(0)).collect(),
    });
    let id = eng.register(op.clone());
    let u = eng.new_unit();
    let ids = (0..N)
        .map(|i| {
            let v = eng.values();
            let ii = v.int(i64::try_from(i).unwrap());
            let a = if i % 6 == 5 {
                let f = if (i / 6) % 4 == 3 { fb } else { fa };
                vec![v.str("r"), ii, f.0]
            } else {
                let f = if i % 100 == 7 { fb } else { fa };
                vec![v.str("c"), ii, v.int(1), f.0]
            };
            eng.push(u, id, a)
        })
        .collect();
    (eng, op, id, ids, fa, fb)
}

fn reader_of(i: usize, fa: Field, fb: Field) -> Option<Field> {
    (i % 6 == 5).then_some(if (i / 6) % 4 == 3 { fb } else { fa })
}

fn ran(op: &Acc) -> Vec<usize> {
    (0..op.runs.len())
        .filter(|&i| op.runs[i].swap(0, Relaxed) > 0)
        .collect()
}

fn outs(eng: &Engine, ids: &[NodeId]) -> Vec<Option<i64>> {
    ids.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_remade_contributor_reruns_only_the_readers_whose_fold_changed() {
    let mut finals = Vec::new();
    for sched in ["seq", "spec", "random", "par"] {
        let (mut eng, op, id, ids, fa, fb) = build();
        eval(&mut eng, sched);
        ran(&op);
        let c = |eng: &Engine, i: usize, x: i64, extra: bool| {
            let v = eng.values();
            let mut a = vec![v.str("c"), v.int(i64::try_from(i).unwrap()), v.int(x), fa.0];
            if extra {
                a.push(v.str("again"));
            }
            a
        };
        // Re-made with the same contribution: only node 10 runs.
        let a = c(&eng, 10, 1, true);
        eng.replace(ids[10], id, a);
        eval(&mut eng, sched);
        assert_eq!(ran(&op), vec![10], "{sched}: identical contribution");
        // Its contribution changes: node 10 and exactly the A readers after it.
        let a = c(&eng, 10, 2, false);
        eng.replace(ids[10], id, a);
        eval(&mut eng, sched);
        let want: Vec<usize> = std::iter::once(10)
            .chain((11..N).filter(|&i| reader_of(i, fa, fb) == Some(fa)))
            .collect();
        assert_eq!(ran(&op), want, "{sched}: changed contribution");
        // An insert contributing 1 to A before node 600: it and the A
        // readers after it.
        let a = {
            let v = eng.values();
            vec![v.str("c"), v.int(i64::try_from(N).unwrap()), v.int(1), fa.0]
        };
        eng.insert_before(ids[600], id, a);
        eval(&mut eng, sched);
        let want: Vec<usize> = (600..N)
            .filter(|&i| reader_of(i, fa, fb) == Some(fa))
            .chain(std::iter::once(N))
            .collect();
        assert_eq!(ran(&op), want, "{sched}: insert");
        // Removing a contributor of B at 807: the B readers after it.
        eng.remove(ids[807]);
        eval(&mut eng, sched);
        let want: Vec<usize> = (808..N).filter(|&i| reader_of(i, fa, fb) == Some(fb)).collect();
        assert_eq!(ran(&op), want, "{sched}: remove");
        // The folds are those a cold build gives.
        let mut live = ids.clone();
        live.remove(807);
        let got = outs(&eng, &live);
        for (k, &i) in (0..N).filter(|&i| i != 807).collect::<Vec<_>>().iter().enumerate() {
            if let Some(f) = reader_of(i, fa, fb) {
                let before = |j: usize| j < i && j != 807 && reader_of(j, fa, fb).is_none();
                let mut want = i64::try_from(
                    (0..i)
                        .filter(|&j| before(j) && (if j % 100 == 7 { fb } else { fa }) == f)
                        .count(),
                )
                .unwrap();
                if f == fa && i > 10 {
                    want += 1; // node 10 contributes 2
                    if i > 600 {
                        want += 1; // the insert
                    }
                }
                assert_eq!(got[k], Some(want), "{sched}: reader {i}");
            }
        }
        finals.push(got);
    }
    assert!(finals.windows(2).all(|w| w[0] == w[1]), "schedules differ");
}
