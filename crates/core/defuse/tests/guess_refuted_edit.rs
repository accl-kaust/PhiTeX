//! A reader that took a definer's previous value while the definer re-ran
//! sees the definer's new value: `F4 [v]` defines `c = v`; `F5` reads `c`,
//! sleeps, defines `c + 1`; `F8` (reads `[c]`) reads `c`. Edit `F4`.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Speculation, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "f4" | "f5" => vec![(v.field("c"), Locality::Local)],
            "w" => vec![(v.field("ps"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn claim(
        &self,
        a: &[ValueId],
        v: &Values,
        _: &mut dyn Iterator<Item = (phitex_defuse::OpId, Vec<ValueId>)>,
    ) -> phitex_defuse::Claim {
        if matches!(tag(v, a).as_str(), "f5" | "f8") {
            phitex_defuse::Claim::Pending
        } else {
            phitex_defuse::Claim::None
        }
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(match tag(v, a).as_str() {
            "f5" | "f8" => vec![v.field("c")],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let c = v.field("c");
        match tag(v, &a).as_str() {
            "f4" => {
                cx.define(c, a[1]);
                Ok(ValueId::UNIT)
            }
            "w" => {
                std::thread::sleep(std::time::Duration::from_millis(60));
                cx.define(v.field("ps"), a[1]);
                Ok(ValueId::UNIT)
            }
            t @ ("f5" | "f8") => {
                // Resumed: what it read before the stop stays loaded.
                let x = match cx.parked() {
                    Some(tok) => *tok.downcast::<i64>().unwrap(),
                    None => cx.use_field(c)?.and_then(|p| v.as_int(p)).unwrap_or(0),
                };
                if t == "f5" {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    cx.define(c, v.int(x + 1));
                }
                cx.claim(0);
                cx.narrow(phitex_defuse::MayDefine::Names(vec![c, v.field("d")]));
                cx.park(Box::new(x));
                let _ = cx.use_field(v.field("ps"))?;
                Ok(v.int(x))
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

struct Pool;

impl phitex_defuse::Executor for Pool {
    fn parallelism(&self) -> usize {
        4
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        let res: Vec<phitex_defuse::RunResult> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..n).map(|i| s.spawn(move || run(i))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        out.extend(res);
    }
}

#[test]
fn a_guessed_previous_value_is_refuted_by_the_new_one() {
    for mode in [Speculation::Off, Speculation::Predicted, Speculation::All] {
        for _ in 0..5 {
            let mut eng = Engine::new();
            let op = eng.register(Arc::new(Ops));
            let u = eng.new_unit();
            let s = |e: &Engine, t: &str| e.values().str(t);
            let (f4, f5, f8, three) = (s(&eng, "f4"), s(&eng, "f5"), s(&eng, "f8"), eng.values().int(3));
            let a = eng.push(u, op, vec![f4, three]);
            let (w, one) = (s(&eng, "w"), eng.values().int(1));
            let wn = eng.push(u, op, vec![w, one]);
            eng.push(u, op, vec![f5]);
            let r = eng.push(u, op, vec![f8]);
            eng.evaluate(&mut phitex_defuse::Greedy::speculative(mode, 256), &mut Pool)
                .unwrap();
            assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(4));
            let four = eng.values().int(4);
            eng.replace(a, op, vec![f4, four]);
            let two = eng.values().int(2);
            eng.replace(wn, op, vec![w, two]);
            eng.evaluate(&mut phitex_defuse::Greedy::speculative(mode, 256), &mut Pool)
                .unwrap();
            assert_eq!(
                eng.output(r).and_then(|x| eng.values().as_int(x)),
                Some(5),
                "{mode:?}: F8 sees F5's new c"
            );
        }
    }
}
