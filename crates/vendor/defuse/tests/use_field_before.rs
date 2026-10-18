//! `Ctx::use_field_before(f, a)`: the definition reaching node `a`'s start
//! (what `a` itself would read), against `use_field_at(f, a)`, just after
//! `a`. Program: `D` defines `x = k`, `A` defines `x = 2`, `R` reads both
//! at `A`. Edits of `D` reach `R` through the before-read only; with `A`
//! first in the unit nothing reaches its start.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, FieldAt, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops {
    a: Mutex<Option<NodeId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn int(v: &Values, r: FieldAt) -> i64 {
    match r {
        FieldAt::Def(x, _) => v.as_int(x).unwrap_or(-2),
        FieldAt::Unset => -1,
        FieldAt::Gone => -3,
    }
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "r" {
            Vec::new()
        } else {
            vec![(v.field("x"), Locality::Local)]
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) != "r" {
            cx.define(v.field("x"), a[1]);
            return Ok(a[1]);
        }
        let at = self.a.lock().unwrap().expect("A pushed");
        let before = int(v, cx.use_field_before(v.field("x"), at)?);
        let after = int(v, cx.use_field_at(v.field("x"), Some(at))?);
        Ok(v.int(before * 100 + after))
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
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
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

fn eval(eng: &mut Engine, i: usize) {
    if i.is_multiple_of(2) {
        eng.evaluate_sequential().expect("evaluate");
    } else {
        eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(1 + i % 4))
            .expect("evaluate");
    }
}

fn out(eng: &Engine, n: NodeId) -> i64 {
    eng.output(n).and_then(|x| eng.values().as_int(x)).unwrap()
}

#[test]
fn before_reads_the_start_of_an_earlier_node() {
    for i in 0..8 {
        let ops = Arc::new(Ops { a: Mutex::new(None) });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let u = eng.new_unit();
        let (d, k1) = (eng.values().str("d"), eng.values().int(1));
        let dn = eng.push(u, op, vec![d, k1]);
        let (a, k2) = (eng.values().str("a"), eng.values().int(2));
        let an = eng.push(u, op, vec![a, k2]);
        *ops.a.lock().unwrap() = Some(an);
        let r = eng.values().str("r");
        let rn = eng.push(u, op, vec![r]);
        eval(&mut eng, i);
        assert_eq!(out(&eng, rn), 102, "cold {i}");
        // Only the before-read sees D: an edit of D re-runs R.
        let k5 = eng.values().int(5);
        eng.replace(dn, op, vec![d, k5]);
        eval(&mut eng, i + 1);
        assert_eq!(out(&eng, rn), 502, "edit {i}");
        assert!(eng.check_reads().is_empty(), "{i}: {:?}", eng.check_reads());
        // D gone: A is first in the unit, nothing reaches its start.
        eng.remove(dn);
        eval(&mut eng, i);
        assert_eq!(out(&eng, rn), -98, "first {i}");
        assert!(eng.check_reads().is_empty(), "{i}: {:?}", eng.check_reads());
    }
}
