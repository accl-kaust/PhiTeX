//! Record cells read by many nodes: `pathbench [mode] [n] [threads] [reps]`.
//! Node 0 defines five records `rec0..4` (lists of 256 integers), every
//! 1000th node redefines `rec0`; node `k` reads 16 cells of each record
//! (mode 0: the whole record, then indexes it; 1: `use_field_path` per
//! cell; 2: one view per record, `get` per cell) and defines `x{k}`.
//! Prints the cold wall and an edit's (one cell of `rec0` changed), the
//! best of `reps`.
use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Step, Stop, ValueId, Values};

struct Ops(u8);

const CELLS: u32 = 256;

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let k = v.as_int(a[0]).unwrap_or(0);
        let mut d = vec![(v.field(&format!("x{k}")), Locality::Local)];
        if k == 0 {
            d.extend((0..5).map(|r| (v.field(&format!("rec{r}")), Locality::Local)));
        } else if k % 1000 == 0 {
            d.push((v.field("rec0"), Locality::Local));
        }
        Some(d)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let k = v.as_int(a[0]).unwrap_or(0);
        if k == 0 {
            let base = v.as_int(a[1]).unwrap_or(0);
            for r in 0..5 {
                let xs = (0..i64::from(CELLS)).map(|i| v.int(base + i + r)).collect();
                cx.define(v.field(&format!("rec{r}")), v.list(xs));
            }
        } else if k % 1000 == 0 {
            let xs = (0..i64::from(CELLS)).map(|i| v.int(k + i)).collect();
            cx.define(v.field("rec0"), v.list(xs));
        }
        let mut s = 0i64;
        for r in 0..5 {
            let f = v.field(&format!("rec{r}"));
            let cells = (0..16u32).map(|j| (u32::try_from(k).unwrap_or(0).wrapping_mul(7) + j * 13) % CELLS);
            match self.0 {
                1 => {
                    for c in cells {
                        let p = cx.path(&[Step::Index(c)]);
                        s += cx.use_field_path(f, p)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                    }
                }
                2 => {
                    if let Some(m) = cx.view(f)? {
                        for c in cells {
                            s += cx.get(m, Step::Index(c)).and_then(|x| v.as_int(x)).unwrap_or(0);
                        }
                    }
                }
                _ => {
                    if let Some(x) = cx.use_field(f)? {
                        for c in cells {
                            s += v.with(x, |val| match val {
                                defuse::Value::List(l) => l.get(c as usize).and_then(|&e| v.as_int(e)),
                                _ => None,
                            })
                            .unwrap_or(0);
                        }
                    }
                }
            }
        }
        cx.define(v.field(&format!("x{k}")), v.int(s));
        Ok(ValueId::UNIT)
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(&mut self, n: usize, run: &(dyn Fn(usize) -> defuse::RunResult + Sync), out: &mut Vec<defuse::RunResult>) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
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

fn eval(eng: &mut Engine, th: usize) -> std::time::Duration {
    let t = std::time::Instant::now();
    if th == 1 {
        eng.evaluate_sequential().unwrap();
    } else {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 256), &mut Threads(th)).unwrap();
    }
    t.elapsed()
}

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let mode = u8::try_from(a.first().copied().unwrap_or(1)).unwrap_or(1);
    let n = a.get(1).copied().unwrap_or(20_000);
    let th = a.get(2).copied().unwrap_or(1);
    let reps = a.get(3).copied().unwrap_or(3);
    let (mut cold, mut edit) = (std::time::Duration::MAX, std::time::Duration::MAX);
    for _ in 0..reps {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops(mode)));
        let u = eng.new_unit();
        let mut first = None;
        for k in 0..n {
            let mut args = vec![eng.values().int(k as i64)];
            if k == 0 {
                args.push(eng.values().int(0));
            }
            let id = eng.push(u, op, args);
            first.get_or_insert(id);
        }
        cold = cold.min(eval(&mut eng, th));
        let args = vec![eng.values().int(0), eng.values().int(1)];
        eng.replace(first.unwrap(), op, args);
        edit = edit.min(eval(&mut eng, th));
    }
    println!("mode {mode} n {n} threads {th}: cold {:.1} ms, edit {:.1} ms", cold.as_secs_f64() * 1e3, edit.as_secs_f64() * 1e3);
}
