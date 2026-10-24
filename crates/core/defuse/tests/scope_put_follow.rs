//! A scope opener whose run makes a follow node defining a local inside
//! the scope it opens: `A` defines `x = 1`; `O` opens a scope after itself
//! and follows `P`, which defines `x = 5` (local); readers `R` after `O`
//! (inside the scope) read 5; `C` closes the scope; `Z` after it reads 1.
//! The same in every schedule (readers wait for the new node `P`).

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use phitex_defuse::{
    Brackets, Claim, Ctx, Effect, Engine, Field, Locality, MayDefine, OpId, Operation, Stop, ValueId, Values,
};

struct Ops {
    me: Mutex<Option<OpId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "a" => Some(vec![(v.field("x"), Locality::Global)]),
            "p" if std::env::var("PUT_UNDECLARED").is_err() => Some(vec![(v.field("x"), Locality::Local)]),
            "p" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        match self.defs(a, v) {
            Some(d) => MayDefine::Names(d.into_iter().map(|e| e.0).collect()),
            None => MayDefine::Any,
        }
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "o" if std::env::var("EFFECT_PENDING").is_ok() => Effect::Pending,
            "o" => Effect::Scope(Brackets {
                opens: 1,
                ..Brackets::default()
            }),
            "c" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn claim(&self, a: &[ValueId], v: &Values, _f: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "o" && std::env::var("CLAIM_PENDING").is_ok() {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "a" => {
                cx.define(v.field("x"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "o" => {
                // Some work first, so readers may be picked meanwhile.
                std::thread::sleep(std::time::Duration::from_micros(200));
                cx.follow(op, vec![v.str("p")]);
                if std::env::var("CLAIM_PENDING").is_ok() {
                    cx.claim(0);
                }
                if std::env::var("EFFECT_PENDING").is_ok() {
                    cx.set_effect(Effect::Scope(Brackets {
                        opens: 1,
                        ..Brackets::default()
                    }));
                }
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.define_with(v.field("x"), v.int(5), Locality::Local);
                Ok(ValueId::UNIT)
            }
            "c" => Ok(ValueId::UNIT),
            _ => Ok(cx.use_field(v.field("x"))?.unwrap_or(ValueId::UNIT)),
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
fn readers_in_the_scope_see_the_follow_local_in_every_schedule() {
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    for i in 0..200 {
        let ops = Arc::new(Ops { me: Mutex::new(None) });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        *ops.me.lock().unwrap() = Some(op);
        let u = eng.new_unit();
        let s = |e: &Engine, t: &str| e.values().str(t);
        eng.push(u, op, vec![s(&eng, "a")]);
        eng.push(u, op, vec![s(&eng, "o")]);
        let rs: Vec<_> = (0..6).map(|_| eng.push(u, op, vec![s(&eng, "r")])).collect();
        eng.push(u, op, vec![s(&eng, "c")]);
        let z = eng.push(u, op, vec![s(&eng, "z")]);
        let r = match i % 3 {
            0 => eng.evaluate_sequential(),
            1 => eng.evaluate(
                &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
                &mut Threads,
            ),
            _ => eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut Threads),
        };
        r.expect("evaluate");
        let outs: Vec<_> = rs
            .iter()
            .chain([&z])
            .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
            .collect();
        *seen.entry(format!("{outs:?}")).or_default() += 1;
    }
    eprintln!("{seen:?}");
    assert_eq!(seen.len(), 1, "outputs differ across schedules: {seen:?}");
}
