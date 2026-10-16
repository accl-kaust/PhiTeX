//! A run that narrows its definition set without a field and then defines
//! it anyway: a reader released by the narrow re-runs with that value.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, MayDefine, Operation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "f4" => Some(vec![(v.field("c"), Locality::Local)]),
            "f5" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        if tag(v, a) == "f5" {
            MayDefine::Names(vec![v.field("c"), v.field("d")])
        } else {
            MayDefine::Any
        }
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "f8").then(|| vec![v.field("c")])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "f4" => {
                cx.define(v.field("c"), v.int(4));
                Ok(ValueId::UNIT)
            }
            "f5" => {
                cx.narrow(MayDefine::Names(vec![v.field("d")]));
                std::thread::sleep(std::time::Duration::from_millis(50));
                cx.define(v.field("c"), v.int(5));
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_field(v.field("c"))?.unwrap_or(ValueId::UNIT)),
        }
    }
}

struct Pool;

impl defuse::Executor for Pool {
    fn parallelism(&self) -> usize {
        4
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

#[test]
fn a_reader_released_by_a_narrow_sees_a_definition_outside_it() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let (f4, f5, f8) = (s(&eng, "f4"), s(&eng, "f5"), s(&eng, "f8"));
    eng.push(u, op, vec![f4]);
    eng.push(u, op, vec![f5]);
    let r = eng.push(u, op, vec![f8]);
    let _ = eng.evaluate(&mut defuse::Greedy::default(), &mut Pool);
    assert_eq!(
        eng.output(r).and_then(|x| eng.values().as_int(x)),
        Some(5),
        "F8 reads F5's c"
    );
}
