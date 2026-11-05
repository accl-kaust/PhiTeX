//! A worker whose commit queues a short run and, after it, a long one runs
//! the long one itself, on the caches its own run left, and an idle worker
//! takes the short one (`Pool::keep_longest`): before, the short one took
//! the worker and the long one went at once to another, where a long run
//! costs up to twice its time (tikz-cd's `\tableofcontents` run after the
//! read-back child, mc 80254).

use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Operation, Stop, ValueId};

/// `[name, x, us]`: works `us` microseconds, reads "a" unless it is "a"
/// itself (then defines it as x); notes the thread it ran on by name.
struct Work(Mutex<Vec<(String, std::thread::ThreadId)>>);

impl Operation for Work {
    fn name(&self) -> &'static str {
        "work"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let name = v.as_str(cx.args()[0]).unwrap_or_default();
        let us = v.as_int(cx.args()[2]).unwrap_or(0).unsigned_abs();
        let a = v.field("a");
        let x = if name == "a" {
            cx.define(a, cx.args()[1]);
            cx.args()[1]
        } else {
            cx.read(a)?.unwrap_or(ValueId::UNIT)
        };
        let t = Instant::now();
        while t.elapsed() < Duration::from_micros(us) {
            std::hint::spin_loop();
        }
        self.0
            .lock()
            .unwrap()
            .push((name.to_owned(), std::thread::current().id()));
        Ok(x)
    }
}

#[test]
fn the_long_run_a_commit_queues_stays_on_its_worker() {
    let e = Engine::new();
    let v = e.values();
    let work = Arc::new(Work(Mutex::new(Vec::new())));
    let op = e.register(work.clone());
    let u = e.new_unit();
    let a = e.push(u, op, vec![v.str("a"), v.int(1), v.int(3000)]);
    e.push(u, op, vec![v.str("short"), v.int(0), v.int(600)]);
    e.push(u, op, vec![v.str("long"), v.int(0), v.int(30_000)]);
    e.evaluate(4).unwrap();
    for k in 2..6 {
        work.0.lock().unwrap().clear();
        e.replace(a, op, vec![v.str("a"), v.int(k), v.int(3000)]);
        let r = e.evaluate(4).unwrap();
        assert!(e.check_invariants().is_empty());
        let runs = work.0.lock().unwrap().clone();
        let on = |n: &str| runs.iter().find(|x| x.0 == n).map(|x| x.1);
        assert!(on("short").is_some() && on("long").is_some(), "{runs:?} {r:?}");
        assert_eq!(on("long"), on("a"), "the long run on the definer's worker: {runs:?}");
    }
}
