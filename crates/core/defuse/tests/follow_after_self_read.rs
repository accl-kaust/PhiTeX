//! A node that places a definer before itself ([`phitex_defuse::Ctx::follow_after`])
//! reads what that definer defines. `s k` reads `f` and places `c k`
//! (defines `f = k`) right after the anchor `a`, which comes before `s`:
//! a first run reads `f` unset, the placed definer defines it before `s`,
//! so `s` must run again and end with `k`, as a sequential evaluation
//! does. The definer's definition set is pending (bounded by a prefix),
//! listed, or predicted wrongly, per schedule.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, MayDefine, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: std::sync::OnceLock<OpId>,
    anchor: std::sync::Mutex<Option<NodeId>>,
    /// 0: pending set under a prefix; 1: listed; 2: pending and predicted wrongly.
    mode: u8,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "c" if self.mode == 1 => Some(vec![(v.field("f:x"), Locality::Local)]),
            "c" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        match tag(v, a).as_str() {
            "c" => MayDefine::Prefixes(vec!["f:".into()]),
            _ => MayDefine::Nothing,
        }
    }
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (self.mode == 2 && tag(v, a) == "c").then(|| vec![(v.field("f:x"), v.int(-7))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "c" => {
                cx.define(v.field("f:x"), a[1]);
                Ok(a[1])
            }
            "s" => {
                let x = cx.use_field(v.field("f:x"))?;
                let at = self.anchor.lock().unwrap().expect("anchor");
                cx.follow_after(at, *self.me.get().unwrap(), vec![v.str("c"), a[1]]);
                Ok(x.unwrap_or(v.int(-1)))
            }
            "r" => Ok(cx.use_field(v.field("f:x"))?.unwrap_or(v.int(-1))),
            _ => Ok(ValueId::UNIT),
        }
    }
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
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

fn eval(eng: &mut Engine, i: usize) {
    match i % 5 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64),
            &mut Threads(1 + i % 4),
        ),
        2 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 64),
            &mut Threads(1 + i % 4),
        ),
        3 => eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut Threads(1 + i % 4)),
        _ => eng.evaluate(
            &mut phitex_defuse::RandomScheduler::new(i as u64, true),
            &mut phitex_defuse::Sequential,
        ),
    }
    .expect("evaluate");
}

fn out(eng: &Engine, n: NodeId) -> Option<i64> {
    eng.output(n).and_then(|x| eng.values().as_int(x))
}

#[test]
fn a_definer_placed_before_its_reader_reaches_it() {
    for i in 0..300 {
        let ops = Arc::new(Ops {
            me: std::sync::OnceLock::new(),
            anchor: std::sync::Mutex::new(None),
            mode: (i / 5 % 3) as u8,
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let st = |e: &Engine, t: &str| e.values().str(t);
        let a = st(&eng, "a");
        let an = eng.push(u, op, vec![a]);
        *ops.anchor.lock().unwrap() = Some(an);
        let r = st(&eng, "r");
        let r0 = eng.push(u, op, vec![r]);
        let (s, k) = (st(&eng, "s"), eng.values().int(5));
        let sn = eng.push(u, op, vec![s, k]);
        let r1 = eng.push(u, op, vec![r]);
        eval(&mut eng, i);
        let what = format!("schedule {i} mode {}", ops.mode);
        assert_eq!(out(&eng, sn), Some(5), "{what}: s");
        assert_eq!(out(&eng, r0), Some(5), "{what}: reader between");
        assert_eq!(out(&eng, r1), Some(5), "{what}: reader after");
        assert!(eng.check_reads().is_empty(), "{what}: {:?}", eng.check_reads());
        // A new value for the definer: s places it again and reads it.
        let k = eng.values().int(9);
        eng.replace(sn, op, vec![s, k]);
        eval(&mut eng, i + 1);
        assert_eq!(out(&eng, sn), Some(9), "{what} edit: s");
        assert_eq!(out(&eng, r1), Some(9), "{what} edit: reader after");
        assert!(eng.check_reads().is_empty(), "{what} edit: {:?}", eng.check_reads());
    }
}
