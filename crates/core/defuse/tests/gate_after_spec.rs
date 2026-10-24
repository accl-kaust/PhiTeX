//! A gate holds until the definer commits: `e [k]` (slow) defines `y = k`;
//! `d` defines `m = y + 1` (it may run speculatively while `e` runs);
//! `f` declares `reads [m]` and reads `m`. After editing `e`, `f` runs once,
//! on the new `m`, never on the old one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops {
    undeclared: bool,
    f_runs: AtomicU64,
    f_old: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "e" => vec![(v.field("y"), Locality::Local)],
            "d" if self.undeclared => return None,
            "d" => vec![(v.field("m"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(match tag(v, a).as_str() {
            "f" => vec![v.field("m")],
            _ => return None,
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "e" => {
                std::thread::sleep(std::time::Duration::from_millis(30));
                cx.define(v.field("y"), a[1]);
                Ok(ValueId::UNIT)
            }
            "d" => {
                let y = cx.use_field(v.field("y"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.define(v.field("m"), v.int(y + 1));
                Ok(ValueId::UNIT)
            }
            _ => {
                let m = cx.use_field(v.field("m"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                self.f_runs.fetch_add(1, Relaxed);
                if m == 2 {
                    self.f_old.fetch_add(1, Relaxed);
                }
                Ok(v.int(m))
            }
        }
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

#[test]
fn a_gated_reader_waits_for_the_definers_commit_not_its_speculative_run() {
    for (spec, undeclared) in [
        (phitex_defuse::Speculation::All, false),
        (phitex_defuse::Speculation::Predicted, false),
        (phitex_defuse::Speculation::All, true),
        (phitex_defuse::Speculation::Predicted, true),
    ] {
        let ops = Arc::new(Ops {
            undeclared,
            f_runs: AtomicU64::new(0),
            f_old: AtomicU64::new(0),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let u = eng.new_unit();
        let s = |e: &Engine, t: &str| e.values().str(t);
        let one = eng.values().int(1);
        let en = eng.push(u, op, vec![s(&eng, "e"), one]);
        eng.push(u, op, vec![s(&eng, "d")]);
        let fnode = eng.push(u, op, vec![s(&eng, "f")]);
        eng.evaluate(&mut phitex_defuse::Greedy::speculative(spec, 64), &mut Threads)
            .unwrap();
        assert_eq!(eng.output(fnode), Some(eng.values().int(2)));
        let (before, old) = (ops.f_runs.load(Relaxed), ops.f_old.load(Relaxed));
        let five = eng.values().int(5);
        eng.replace(en, op, vec![s(&eng, "e"), five]);
        let rep = eng
            .evaluate(&mut phitex_defuse::Greedy::speculative(spec, 64), &mut Threads)
            .unwrap();
        assert_eq!(eng.output(fnode), Some(eng.values().int(6)));
        assert_eq!(
            ops.f_old.load(Relaxed),
            old,
            "{spec:?} undeclared {undeclared}: f read the old m: {:?}",
            rep.rerun_causes
        );
        assert_eq!(
            ops.f_runs.load(Relaxed),
            before + 1,
            "{spec:?} undeclared {undeclared}: {:?}",
            rep.rerun_causes
        );
    }
}
