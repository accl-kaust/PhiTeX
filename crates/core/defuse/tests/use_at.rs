//! Many `Ctx::use_field_at` readers: re-making a position node re-runs only
//! its readers, under every schedule.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

const NODES: usize = 5000;
const READERS: usize = 700;
const FIELDS: usize = 20;

/// `["t", i]` a text node, defining `f{i % 20}` as `i` when `i % 7 == 0`;
/// `["r", j]` reads `f{j % 20}` at position node `pos(j)`, outputs the
/// value (or -1).
struct Ops {
    ids: Mutex<Vec<NodeId>>,
    runs: Vec<AtomicU64>,
}

fn pos(j: usize) -> usize {
    ((j * 37) % 350) * 14 + 3
}

fn field(v: &Values, k: usize) -> Field {
    v.field(&format!("f{k}"))
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        Some(if v.as_str(a[0]).as_deref() == Some("t") && i % 7 == 0 {
            vec![(field(v, i % FIELDS), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        if v.as_str(a[0]).as_deref() == Some("t") {
            self.runs[i].fetch_add(1, Relaxed);
            if i % 7 == 0 {
                cx.define(field(v, i % FIELDS), a[1]);
            }
            return Ok(ValueId::UNIT);
        }
        self.runs[NODES + i].fetch_add(1, Relaxed);
        let at = self.ids.lock().unwrap()[pos(i)];
        let got = cx.use_field_at(field(v, i % FIELDS), Some(at))?;
        Ok(v.int(match got {
            phitex_defuse::FieldAt::Def(x, _) => v.as_int(x).unwrap_or(-1),
            _ => -1,
        }))
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

fn eval(eng: &mut Engine, sched: &str) {
    match sched {
        "seq" => eng.evaluate_sequential(),
        "spec" => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 4096),
            &mut phitex_defuse::Sequential,
        ),
        "random" => eng.evaluate(&mut phitex_defuse::RandomScheduler::new(5, true), &mut phitex_defuse::Sequential),
        _ => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 4096),
            &mut Threads,
        ),
    }
    .expect("evaluate");
}

/// What reader `j` sees with position node `pos(j)` there: the last
/// definer of its field at or before the position.
fn want(j: usize) -> i64 {
    let k = j % FIELDS;
    (0..=pos(j))
        .rev()
        .find(|&i| i % 7 == 0 && i % FIELDS == k)
        .map_or(-1, |i| i64::try_from(i).unwrap())
}

#[test]
fn remaking_a_position_node_reruns_only_its_readers() {
    let mut finals = Vec::new();
    for sched in ["seq", "spec", "random", "par"] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops {
            ids: Mutex::new(Vec::new()),
            runs: (0..NODES + READERS).map(|_| AtomicU64::new(0)).collect(),
        });
        let op = eng.register(ops.clone());
        let u = eng.new_unit();
        let ids: Vec<NodeId> = (0..NODES)
            .map(|i| {
                let v = eng.values();
                let a = vec![v.str("t"), v.int(i64::try_from(i).unwrap())];
                eng.push(u, op, a)
            })
            .collect();
        *ops.ids.lock().unwrap() = ids.clone();
        let rs: Vec<NodeId> = (0..READERS)
            .map(|j| {
                let v = eng.values();
                let a = vec![v.str("r"), v.int(i64::try_from(j).unwrap())];
                eng.push(u, op, a)
            })
            .collect();
        eval(&mut eng, sched);
        let outs = |eng: &Engine| -> Vec<i64> {
            rs.iter()
                .map(|&r| eng.output(r).and_then(|x| eng.values().as_int(x)).unwrap())
                .collect()
        };
        let got = outs(&eng);
        for (j, &g) in got.iter().enumerate() {
            assert_eq!(g, want(j), "{sched}: reader {j}");
        }
        let ran = || -> Vec<usize> {
            (0..NODES + READERS)
                .filter(|&i| ops.runs[i].swap(0, Relaxed) > 0)
                .collect()
        };
        ran();
        // Re-make the position node of reader 100 (a plain text node): it goes and a new
        // node with the same text takes its place.
        let p = pos(100);
        let readers: Vec<usize> = (0..READERS).filter(|&j| pos(j) == p).collect();
        assert!(readers.len() >= 2);
        eng.remove(ids[p]);
        let a = {
            let v = eng.values();
            vec![v.str("t"), v.int(i64::try_from(p).unwrap())]
        };
        let fresh = eng.insert_before(ids[p + 1], op, a);
        // Its readers see the position gone; the producer names the new node.
        ops.ids.lock().unwrap()[p] = fresh;
        eval(&mut eng, sched);
        let want_ran: Vec<usize> = std::iter::once(p).chain(readers.iter().map(|&j| NODES + j)).collect();
        assert_eq!(ran(), want_ran, "{sched}: re-made position");
        assert_eq!(outs(&eng), got, "{sched}: values");
        finals.push(outs(&eng));
    }
    assert!(finals.windows(2).all(|w| w[0] == w[1]), "schedules differ");
}
