//! `Ctx::define_in_scope`: an opener's run defines `x` both ways in one
//! commit: `x = 2` at its own level (seen after the scope closes) and
//! `y = 5` local to the scope it opens (seen inside, gone after). Readers
//! inside the scope see `y = 5` and `x = 2`; after the close, `y` is the
//! outer value and `x` still 2. Also with `y` defined at both levels by
//! the same run (`y = 3` after the close), and with the effect set by the
//! run. Edits of the opener's arguments reach the readers in every
//! schedule; outputs equal a cold build's.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, MayDefine, OpId, Operation, Report, Stop, ValueId, Values};

/// Bit 0: the opener defines `y` at both levels; bit 1: its effect is
/// set by the run.
static MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

struct Ops {
    me: Mutex<Option<OpId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "a" => Some(vec![(v.field("x"), Locality::Global), (v.field("y"), Locality::Global)]),
            "o" if MODE.load(Relaxed) & 1 != 0 => Some(vec![
                (v.field("x"), Locality::Local),
                (v.field("y"), Locality::Local),
                (v.field("y"), Locality::Scope),
            ]),
            "o" => Some(vec![(v.field("x"), Locality::Local), (v.field("y"), Locality::Scope)]),
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(self.defs(a, v).unwrap_or_default().into_iter().map(|e| e.0).collect())
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "o" if MODE.load(Relaxed) & 2 != 0 => Effect::Pending,
            "o" => Effect::Scope(Brackets { opens: 1, ..Brackets::default() }),
            "c" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "a" => {
                cx.define(v.field("x"), v.int(1));
                cx.define(v.field("y"), v.int(1));
                Ok(ValueId::UNIT)
            }
            "o" => {
                let k = v.as_int(a[1]).unwrap_or(0);
                cx.define(v.field("x"), v.int(2));
                if MODE.load(Relaxed) & 1 != 0 {
                    // The same field at both levels: 3 after the close.
                    cx.define(v.field("y"), v.int(3));
                }
                cx.define_in_scope(v.field("y"), v.int(5 + k));
                if MODE.load(Relaxed) & 2 != 0 {
                    cx.set_effect(Effect::Scope(Brackets { opens: 1, ..Brackets::default() }));
                }
                Ok(ValueId::UNIT)
            }
            "c" => Ok(ValueId::UNIT),
            _ => {
                let x = v.as_int(cx.use_field(v.field("x"))?.unwrap_or(ValueId::UNIT)).unwrap_or(-1);
                let y = v.as_int(cx.use_field(v.field("y"))?.unwrap_or(ValueId::UNIT)).unwrap_or(-1);
                Ok(v.int(x * 100 + y))
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



fn eval(eng: &mut Engine, i: usize) -> Report {
    match i % 4 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 256), &mut defuse::Sequential),
        2 => eng.evaluate(&mut defuse::RandomScheduler::new(i as u64, true), &mut defuse::Sequential),
        _ => eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 256), &mut Threads),
    }
    .expect("evaluate")
}

fn build(k: i64) -> (Engine, Vec<defuse::NodeId>, defuse::NodeId, OpId) {
    let ops = Arc::new(Ops { me: Mutex::new(None) });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    eng.push(u, op, vec![s(&eng, "a")]);
    let kv = eng.values().int(k);
    let o = eng.push(u, op, vec![s(&eng, "o"), kv]);
    let mut rs: Vec<_> = (0..4).map(|_| eng.push(u, op, vec![s(&eng, "r")])).collect();
    eng.push(u, op, vec![s(&eng, "c")]);
    rs.push(eng.push(u, op, vec![s(&eng, "r")]));
    (eng, rs, o, op)
}

fn outs(eng: &Engine, rs: &[defuse::NodeId]) -> Vec<Option<i64>> {
    rs.iter().map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x))).collect()
}

#[test]
fn a_scope_local_definition_is_seen_inside_only() {
    for mode in 0..4 {
        MODE.store(mode, Relaxed);
        check();
    }
}

fn check() {
    for i in 0..80 {
        let (mut eng, rs, o, op) = build(0);
        eval(&mut eng, i);
        let after = if MODE.load(Relaxed) & 1 != 0 { 203 } else { 201 };
        assert_eq!(outs(&eng, &rs), vec![Some(205); 4].into_iter().chain([Some(after)]).collect::<Vec<_>>(), "cold {i}");
        // Edit the opener: the scope local changes, the readers inside follow.
        let (ov, k3) = (eng.values().str("o"), eng.values().int(3));
        eng.replace(o, op, vec![ov, k3]);
        eval(&mut eng, i + 1);
        let (mut cold, crs, _, _) = build(3);
        eval(&mut cold, 0);
        assert_eq!(outs(&eng, &rs), outs(&cold, &crs), "edit {i}");
        assert_eq!(outs(&eng, &rs)[0], Some(208));
    }
}
