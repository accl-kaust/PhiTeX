//! A node that opens two frames after itself: kinds per frame
//! (`Ctx::set_kinds`) and definitions at a chosen frame
//! (`Ctx::define_in_scope_at`). `X` opens A (kind k1) then B (kind k2),
//! files `f = 1` at A and `g = 2` at B. A reader inside sees f and g; the
//! first closer sees kind k2 and after it a reader sees f only; the second
//! closer sees k1 and after it f is unset too. In every schedule, and an
//! edit of `X` matches a cold build.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use phitex_defuse::{
    Brackets, Ctx, Effect, Engine, Field, Locality, MayDefine, OpId, Operation, Report, Stop, ValueId, Values,
};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "x" => Some(vec![
                (v.field("f"), Locality::ScopeAt(0)),
                (v.field("g"), Locality::ScopeAt(1)),
            ]),
            "y" => Some(vec![(v.field("h"), Locality::ScopeAt(0))]),
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(self.defs(a, v).unwrap_or_default().into_iter().map(|e| e.0).collect())
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "x" => Effect::Scope(Brackets {
                opens: 2,
                kind_pending: true,
                ..Brackets::default()
            }),
            // A continuation inside B: closes B, opens its own frame C.
            "y" => Effect::Scope(Brackets {
                closes: 1,
                opens: 1,
                kind_pending: true,
                ..Brackets::default()
            }),
            "c" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let int = |x: Option<ValueId>| x.and_then(|x| v.as_int(x)).unwrap_or(-1);
        match tag(v, &a).as_str() {
            "x" => {
                let k = v.as_int(a[1]).unwrap_or(0);
                cx.set_kinds(&[v.int(10 + k), v.int(20 + k)]);
                cx.define_in_scope_at(v.field("f"), 0, v.int(1 + k));
                cx.define_in_scope_at(v.field("g"), 1, v.int(2 + k));
                Ok(ValueId::UNIT)
            }
            "y" => {
                cx.set_kinds(&[v.int(30)]);
                cx.define_in_scope_at(v.field("h"), 0, v.int(3));
                Ok(ValueId::UNIT)
            }
            // A closer: the kind of the frame it closes.
            "c" => Ok(v.int(int(cx.scope_kind()?))),
            "h" => {
                let f = int(cx.use_field(v.field("f"))?);
                let h = int(cx.use_field(v.field("h"))?);
                let g = int(cx.use_field(v.field("g"))?);
                Ok(v.int(f * 100 + if g == -1 { h } else { 1000 + g }))
            }
            _ => {
                let f = int(cx.use_field(v.field("f"))?);
                let g = int(cx.use_field(v.field("g"))?);
                Ok(v.int(f * 100 + g))
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

fn eval(eng: &mut Engine, i: usize) -> Report {
    match i % 4 {
        0 => eng.evaluate_sequential(),
        1 => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut phitex_defuse::Sequential,
        ),
        2 => eng.evaluate(
            &mut phitex_defuse::RandomScheduler::new(i as u64, true),
            &mut phitex_defuse::Sequential,
        ),
        _ => eng.evaluate(
            &mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 256),
            &mut Threads,
        ),
    }
    .expect("evaluate")
}

fn build(k: i64) -> (Engine, Vec<phitex_defuse::NodeId>, phitex_defuse::NodeId, OpId) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let s = |e: &Engine, t: &str| e.values().str(t);
    let kv = eng.values().int(k);
    let x = eng.push(u, op, vec![s(&eng, "x"), kv]);
    let mut out = Vec::new();
    for t in ["r", "c", "r", "c", "r"] {
        out.push(eng.push(u, op, vec![s(&eng, t)]));
    }
    (eng, out, x, op)
}

fn outs(eng: &Engine, ns: &[phitex_defuse::NodeId]) -> Vec<i64> {
    ns.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)).unwrap_or(-9))
        .collect()
}

#[test]
fn frames_opened_together_have_their_own_kinds_and_locals() {
    for i in 0..80 {
        let (mut eng, ns, x, op) = build(0);
        eval(&mut eng, i);
        // reader in B, closer of B (k2), reader in A, closer of A (k1), reader outside
        assert_eq!(outs(&eng, &ns), vec![102, 20, 99, 10, -101], "cold {i}");
        let (xv, k5) = (eng.values().str("x"), eng.values().int(5));
        eng.replace(x, op, vec![xv, k5]);
        eval(&mut eng, i + 1);
        let (mut cold, cns, _, _) = build(5);
        eval(&mut cold, 0);
        assert_eq!(outs(&eng, &ns), outs(&cold, &cns), "edit {i}");
        assert_eq!(outs(&eng, &ns), vec![607, 25, 599, 15, -101]);
    }
}

/// A continuation inside X's inner frame B closes B and opens its own C
/// (kind 30, `h = 3` local to C): readers in C see f and h, not g; C's
/// closer sees kind 30; after it, f only (h unset).
#[test]
fn a_continuation_closes_an_inherited_frame_and_opens_its_own() {
    for i in 0..40 {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops));
        let u = eng.new_unit();
        let s = |e: &Engine, t: &str| e.values().str(t);
        let k0 = eng.values().int(0);
        eng.push(u, op, vec![s(&eng, "x"), k0]);
        eng.push(u, op, vec![s(&eng, "y")]);
        let ns: Vec<_> = ["h", "c", "h"]
            .iter()
            .map(|t| eng.push(u, op, vec![s(&eng, t)]))
            .collect();
        eval(&mut eng, i);
        let hs: Vec<i64> = ns
            .iter()
            .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)).unwrap_or(-9))
            .collect();
        assert_eq!(hs, vec![103, 30, 99], "{i}");
    }
}
