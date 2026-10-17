//! Placing after `at` leaves `at` alone: `L` (first, defines many fields)
//! runs once; `B` (later) places `F` after `L`; `F` follows with `G`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: std::sync::OnceLock<OpId>,
    l: std::sync::OnceLock<NodeId>,
    l_runs: AtomicU64,
    effect: u8,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "l" => (0..20).map(|i| (v.field(&format!("p{i}")), Locality::Local)).collect(),
            "f" => vec![(v.field("q"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> defuse::Effect {
        match (self.effect, tag(v, a).as_str()) {
            (1, "l") => defuse::Effect::Open,
            (1, "end") => defuse::Effect::Close,
            _ => defuse::Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        match tag(v, &a).as_str() {
            "l" => {
                self.l_runs.fetch_add(1, Relaxed);
                if self.effect == 2 {
                    let _ = cx.following(0);
                }
                for i in 0..20 {
                    cx.define(v.field(&format!("p{i}")), v.int(i));
                }
                Ok(ValueId::UNIT)
            }
            "b" => {
                let p = cx.use_field(v.field("p3"))?;
                cx.follow_after(*self.l.get().unwrap(), op, vec![v.str("f")]);
                Ok(p.unwrap_or(ValueId::UNIT))
            }
            "f" => {
                cx.define(v.field("q"), v.int(9));
                cx.follow(op, vec![v.str("g")]);
                Ok(ValueId::UNIT)
            }
            "g" | "end" => Ok(ValueId::UNIT),
            _ => Ok(cx.use_field(v.field("q"))?.unwrap_or(v.int(-1))),
        }
    }
}

#[test]
fn placing_after_a_node_does_not_run_it_again() {
    for effect in [0u8, 1, 2] {
        for i in 0..20 {
            let ops = Arc::new(Ops {
                me: std::sync::OnceLock::new(),
                l: std::sync::OnceLock::new(),
                l_runs: AtomicU64::new(0),
                effect,
            });
            let mut eng = Engine::new();
            let op = eng.register(ops.clone());
            let _ = ops.me.set(op);
            let u = eng.new_unit();
            let s = |e: &Engine, t: &str| e.values().str(t);
            let l = eng.push(u, op, vec![s(&eng, "l")]);
            let _ = ops.l.set(l);
            for _ in 0..5 {
                eng.push(u, op, vec![s(&eng, "x")]);
            }
            eng.push(u, op, vec![s(&eng, "b")]);
            if effect == 1 {
                eng.push(u, op, vec![s(&eng, "end")]);
            }
            let r = match i % 3 {
                0 => eng.evaluate_sequential(),
                1 => eng.evaluate(
                    &mut defuse::Greedy::speculative(defuse::Speculation::All, 64),
                    &mut defuse::Sequential,
                ),
                _ => eng.evaluate(
                    &mut defuse::RandomScheduler::new(i as u64, true),
                    &mut defuse::Sequential,
                ),
            };
            let r = r.unwrap_or_else(|e| panic!("effect {effect} schedule {i}: {e}"));
            assert_eq!(
                ops.l_runs.load(Relaxed),
                1,
                "effect {effect} schedule {i}: {:?}",
                r.rerun_causes
            );
        }
    }
}
