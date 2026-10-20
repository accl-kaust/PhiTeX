//! A reader passes a definition entry of a node in work that its
//! declaration does not cover ("gone once it commits"); the node's commit
//! makes the same entry again. The reader must not keep the older
//! definition the entry shadows.
//!
//! `S` defines `c = 1`; `Q` (declared to define only `d`) narrows to `d`
//! and defines `c = 5` anyway; `R` reads `c` after both. When `x` changes,
//! `Q` and `R` run again: `Q` runs on its text's declaration again (its
//! refutation forgotten), so `R`, running meanwhile, passes `Q`'s entry and
//! reads `S`'s `c = 1`. `Q`'s commit re-makes `c = 5` unchanged.

use std::sync::Arc;
use std::time::Duration;

use defuse::{Ctx, Engine, Field, Locality, MayDefine, Operation, Speculation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "s" => Some(vec![(v.field("c"), Locality::Local)]),
            "q" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        if tag(v, a) == "q" {
            MayDefine::Names(vec![v.field("d")])
        } else {
            MayDefine::Any
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "s" => {
                cx.define(v.field("c"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "q" => {
                let x = cx.use_field(v.field("x"))?.and_then(|x| v.as_int(x));
                cx.narrow(MayDefine::Names(vec![v.field("d")]));
                if x == Some(2) {
                    std::thread::sleep(Duration::from_millis(300));
                }
                cx.define(v.field("c"), v.int(5));
                Ok(ValueId::UNIT)
            }
            _ => {
                let x = cx.use_field(v.field("x"))?.and_then(|x| v.as_int(x));
                if x == Some(2) {
                    std::thread::sleep(Duration::from_millis(60));
                }
                Ok(cx.use_field(v.field("c"))?.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

struct Pool;

impl defuse::Executor for Pool {
    fn parallelism(&self) -> usize {
        8
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let res: Vec<defuse::RunResult> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..n).map(|i| s.spawn(move || run(i))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        out.extend(res);
    }
}

fn check(spec: Speculation) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let (ts, tq, tr) = (s(&eng, "s"), s(&eng, "q"), s(&eng, "r"));
    let x = eng.values().field("x");
    eng.push(u, op, vec![ts]);
    eng.push(u, op, vec![tq]);
    let r = eng.push(u, op, vec![tr]);
    let one = eng.values().int(1);
    eng.set_environment([(x, one)]);
    eng.evaluate(&mut defuse::Greedy::speculative(spec, 512), &mut Pool)
        .expect("first evaluation");
    assert_eq!(eng.output(r).and_then(|v| eng.values().as_int(v)), Some(5), "R reads Q's c");
    let two = eng.values().int(2);
    eng.set_environment([(x, two)]);
    eng.evaluate(&mut defuse::Greedy::speculative(spec, 512), &mut Pool)
        .expect("second evaluation");
    assert_eq!(
        eng.output(r).and_then(|v| eng.values().as_int(v)),
        Some(5),
        "R still reads Q's c (not S's, which Q's shadows)"
    );
}

#[test]
fn a_passed_entry_made_again_reaches_its_readers() {
    check(Speculation::Off);
}

#[test]
fn a_passed_entry_made_again_reaches_its_readers_speculating() {
    check(Speculation::All);
}
