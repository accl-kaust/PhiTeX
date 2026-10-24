//! `Ctx::follow_after`: a node placed after an earlier node by id sits at
//! that point of the text. `it k` outputs `k`; `rd` reads `e`; `fold`
//! reads the items' outputs and places `ev k` (defines `e = k`) right
//! after each even item. Text: it0 rd it1 rd it2 rd fold rd.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: std::sync::OnceLock<OpId>,
    items: std::sync::Mutex<Vec<NodeId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "ev" => vec![(v.field("e"), Locality::Local)],
            "it" => vec![(v.field(&format!("o{}", v.as_int(a[2]).unwrap_or(0))), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "it" => {
                cx.define(v.field(&format!("o{}", v.as_int(a[2]).unwrap_or(0))), a[1]);
                Ok(a[1])
            }
            "ev" => {
                cx.define(v.field("e"), a[1]);
                Ok(ValueId::UNIT)
            }
            "fold" => {
                let items = self.items.lock().unwrap().clone();
                for (j, n) in items.into_iter().enumerate() {
                    let out = cx.use_field(v.field(&format!("o{j}")))?.unwrap_or(ValueId::UNIT);
                    if let Some(k) = v.as_int(out)
                        && k % 2 == 0
                    {
                        cx.follow_after(n, *self.me.get().unwrap(), vec![v.str("ev"), out]);
                    }
                }
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_field(v.field("e"))?.unwrap_or(v.int(-1))),
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

fn eval(eng: &mut Engine, i: usize) {
    match i % 4 {
        3 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64),
            &mut Threads,
        ),
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64),
            &mut phitex_defuse::Sequential,
        ),
        _ => eng.evaluate(
            &mut phitex_defuse::RandomScheduler::new(i as u64, true),
            &mut phitex_defuse::Sequential,
        ),
    }
    .expect("evaluate");
}

#[test]
fn a_node_placed_after_an_earlier_node_reads_and_defines_there() {
    for i in 0..200 {
        let ops = Arc::new(Ops {
            me: std::sync::OnceLock::new(),
            items: std::sync::Mutex::new(Vec::new()),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let s = |e: &Engine, t: &str| e.values().str(t);
        let mut rds = Vec::new();
        let mut items = Vec::new();
        for k in [0, 1, 2] {
            let kv = eng.values().int(k);
            items.push(eng.push(u, op, vec![s(&eng, "it"), kv, kv]));
            rds.push(eng.push(u, op, vec![s(&eng, "rd")]));
        }
        *ops.items.lock().unwrap() = items.clone();
        eng.push(u, op, vec![s(&eng, "fold")]);
        rds.push(eng.push(u, op, vec![s(&eng, "rd")]));
        eval(&mut eng, i);
        let got = |e: &Engine| {
            rds.iter()
                .map(|&n| e.output(n).and_then(|x| e.values().as_int(x)))
                .collect::<Vec<_>>()
        };
        assert_eq!(got(&eng), [Some(0), Some(0), Some(2), Some(2)], "schedule {i}");
        assert!(eng.check_reads().is_empty());
        // it2 odd: its event goes.
        let (three, two) = (eng.values().int(3), eng.values().int(2));
        eng.replace(items[2], op, vec![s(&eng, "it"), three, two]);
        eval(&mut eng, i + 1);
        assert_eq!(
            got(&eng),
            [Some(0), Some(0), Some(0), Some(0)],
            "schedule {i} after edit"
        );
        // it1 even: an event after it.
        let (four, one) = (eng.values().int(4), eng.values().int(1));
        eng.replace(items[1], op, vec![s(&eng, "it"), four, one]);
        eval(&mut eng, i + 2);
        assert_eq!(
            got(&eng),
            [Some(0), Some(4), Some(4), Some(4)],
            "schedule {i} after second edit"
        );
        assert!(eng.check_reads().is_empty());
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
