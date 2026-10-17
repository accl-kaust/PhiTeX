//! A speculative run's narrowing holds only from its commit: `m0` defines
//! `m = 2`; `e [k]` (slow) defines `y = k`; `d` may define `m` and, from
//! its run, defines `m = y + 1` when `y >= 5` (narrowing to `[m]`), else
//! nothing (narrowing to `[]`); `f` declares `reads [m]`. Editing `e` from
//! 1 to 5: a speculative `d` that read the old `y` narrows to `[]`; `f`
//! must not take that as final and read the old `m`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, MayDefine, Operation, Stop, ValueId, Values};

struct Ops {
    f_old: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "m0" => Some(vec![(v.field("m"), Locality::Global)]),
            "y0" => Some(vec![(v.field("y"), Locality::Local)]),
            "e" => None,
            "d" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        match tag(v, a).as_str() {
            "d" => MayDefine::Names(vec![v.field("m")]),
            _ => MayDefine::Any,
        }
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "f").then(|| vec![v.field("m")])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "y0" => {
                cx.define(v.field("y"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "m0" => {
                cx.define_with(v.field("m"), v.int(2), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "e" => {
                std::thread::sleep(std::time::Duration::from_millis(30));
                cx.define(v.field("y"), a[1]);
                Ok(ValueId::UNIT)
            }
            "d" => {
                let y = cx.use_field(v.field("y"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                if y >= 5 {
                    cx.define_with(v.field("m"), v.int(y + 1), Locality::Global);
                    cx.narrow(MayDefine::Names(vec![v.field("m")]));
                } else {
                    cx.narrow(MayDefine::Names(Vec::new()));
                }
                Ok(ValueId::UNIT)
            }
            _ => {
                let m = cx.use_field(v.field("m"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                if m == 2 {
                    self.f_old.fetch_add(1, Relaxed);
                }
                Ok(v.int(m))
            }
        }
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

#[test]
fn a_speculative_narrowing_does_not_release_a_gate_before_its_commit() {
    for spec in [defuse::Speculation::All, defuse::Speculation::Predicted] {
        for _ in 0..10 {
            let ops = Arc::new(Ops {
                f_old: AtomicU64::new(0),
            });
            let mut eng = Engine::new();
            let op = eng.register(ops.clone());
            let u = eng.new_unit();
            let s = |e: &Engine, t: &str| e.values().str(t);
            eng.push(u, op, vec![s(&eng, "m0")]);
            eng.push(u, op, vec![s(&eng, "y0")]);
            let one = eng.values().int(1);
            let en = eng.push(u, op, vec![s(&eng, "e"), one]);
            eng.push(u, op, vec![s(&eng, "d")]);
            let fnode = eng.push(u, op, vec![s(&eng, "f")]);
            eng.evaluate(&mut defuse::Greedy::speculative(spec, 64), &mut Threads)
                .unwrap();
            assert_eq!(eng.output(fnode), Some(eng.values().int(2)));
            let old = ops.f_old.load(Relaxed);
            let five = eng.values().int(5);
            eng.replace(en, op, vec![s(&eng, "e"), five]);
            let rep = eng
                .evaluate(&mut defuse::Greedy::speculative(spec, 64), &mut Threads)
                .unwrap();
            assert_eq!(eng.output(fnode), Some(eng.values().int(6)));
            assert_eq!(
                ops.f_old.load(Relaxed),
                old,
                "{spec:?}: f read the old m: {:?}",
                rep.rerun_causes
            );
        }
    }
}
