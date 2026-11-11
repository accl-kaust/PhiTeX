//! Where an evaluation's runs go. Work that comes one node at a time stays
//! on one thread, however many the evaluation may use (the same runs on
//! several take half as long again); work that waits behind a long run
//! goes to another at once.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

/// `[from, to]`: `to` := `from` + 1; the threads it ran on are kept.
struct Step(Arc<Mutex<HashSet<ThreadId>>>);

impl Operation for Step {
    fn name(&self) -> &'static str {
        "step"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.lock().unwrap().insert(std::thread::current().id());
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        let y = cx.values().int(cx.values().as_int(x).unwrap_or(0) + 1);
        cx.define(Field(cx.args()[1]), y);
        Ok(y)
    }
}

/// `[k]`: with k = 1, runs until `n` runs of it stand at once, checking in
/// as a TeX run does (`Ctx::tick`); only another thread can end it. With
/// k = 0, ends at once.
struct Meet(Arc<AtomicUsize>, usize);

impl Operation for Meet {
    fn name(&self) -> &'static str {
        "meet"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[0]) == Some(0) {
            return Ok(ValueId::UNIT);
        }
        self.0.fetch_add(1, Ordering::SeqCst);
        let t = Instant::now();
        while self.0.load(Ordering::SeqCst) < self.1 {
            cx.tick(1)?;
            assert!(t.elapsed() < Duration::from_secs(60), "no other thread came");
            std::thread::yield_now();
        }
        Ok(ValueId::UNIT)
    }
}

/// `[name, x]`: `name` := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

#[test]
fn a_chain_stays_on_one_thread() {
    let threads = Arc::new(Mutex::new(HashSet::new()));
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let step = e.register(Arc::new(Step(threads.clone())));
    let u = e.new_unit();
    let head = e.push(u, def, vec![v.field("x0").0, v.int(1)]);
    for i in 0..200 {
        e.push(
            u,
            step,
            vec![v.field(&format!("x{i}")).0, v.field(&format!("x{}", i + 1)).0],
        );
    }
    e.evaluate(8).unwrap();
    for k in 2..6 {
        threads.lock().unwrap().clear();
        e.replace(head, def, vec![v.field("x0").0, v.int(k)]);
        let r = e.evaluate(8).unwrap();
        assert_eq!(r.runs, 201, "{r:?}");
        assert_eq!(threads.lock().unwrap().len(), 1, "{r:?}");
        assert_eq!(r.steals, 0, "{r:?}");
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// An edit's few nodes all begin on the first worker's queue: the ones
/// behind a long run reach the others as it checks in.
#[test]
fn work_behind_a_long_run_goes_to_another_thread() {
    for n in [2, 3, 8] {
        let e = Engine::new();
        let v = e.values();
        let meet = e.register(Arc::new(Meet(Arc::new(AtomicUsize::new(0)), n)));
        let u = e.new_unit();
        let nodes: Vec<_> = (0..n).map(|_| e.push(u, meet, vec![v.int(0)])).collect();
        e.evaluate(n).unwrap();
        for &x in &nodes {
            e.replace(x, meet, vec![v.int(1)]);
        }
        let r = e.evaluate(n).unwrap();
        assert_eq!(r.runs, n as u64, "{r:?}");
    }
}
