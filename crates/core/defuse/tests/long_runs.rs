//! Cost-aware speculation (`Engine::set_long_runs`): a node whose runs
//! are long does not run ahead of a node before it still in work; without
//! the limit it does.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops(Arc<Mutex<Vec<(String, Instant, Instant)>>>);

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let tag = v.as_str(cx.args()[0]).unwrap();
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(30));
        self.0.lock().unwrap().push((tag, t0, Instant::now()));
        Ok(ValueId::UNIT)
    }
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync), out: &mut Vec<phitex_defuse::RunResult>) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

/// Whether `l` started before `a` ended.
fn ahead(limit: bool) -> bool {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops(log.clone())));
    let u = eng.new_unit();
    let (a, l) = (eng.values().str("a"), eng.values().str("l"));
    eng.push(u, op, vec![a]);
    let ln: NodeId = eng.push(u, op, vec![l]);
    if limit {
        eng.set_long_runs(Some(Duration::from_millis(10)));
        eng.set_run_time_hint(ln, Duration::from_secs(1));
    }
    eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 16), &mut Threads(2)).expect("evaluate");
    let log = log.lock().unwrap();
    let end_a = log.iter().find(|x| x.0 == "a").unwrap().2;
    let start_l = log.iter().find(|x| x.0 == "l").unwrap().1;
    start_l < end_a
}

#[test]
fn a_long_node_waits_for_the_node_before_it() {
    assert!(ahead(false), "without the limit it runs alongside");
    assert!(!ahead(true), "with it, after");
}
