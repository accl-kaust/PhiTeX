//! Static contributions are per field: nodes `e` contribute to `lines`
//! statically (declared) and to `pos` from their runs; readers `s` fold
//! `pos` over the window before them, naming `pos` in their read hint. A
//! reader is held (not run) while an `e` before it is in work, instead of
//! running on a partial fold and stopping; the static contributions to
//! `lines` do not let it pass the `e`s. Results equal a cold build.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{Ctx, Engine, Field, Locality, Operation, Report, Stop, ValueId, Values};

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "s").then(|| vec![v.field("pos")])
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "e").then(|| vec![(v.field("lines"), v.int(1))])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "e" {
            // Some work before the contribution.
            let mut x = 0u64;
            for i in 0..20_000u64 {
                x = x.wrapping_mul(31).wrapping_add(i);
            }
            cx.contribute(v.field("lines"), v.int(1));
            cx.contribute(v.field("pos"), v.int(1 + i64::from(x == 7)));
            return Ok(ValueId::UNIT);
        }
        cx.acc_range(v.field("pos"), None)
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



fn build() -> (Engine, Vec<defuse::NodeId>) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    let (l, p) = (eng.values().field("lines"), eng.values().field("pos"));
    eng.declare_accumulator(l, Arc::new(defuse::SumMonoid));
    eng.declare_accumulator(p, Arc::new(defuse::SumMonoid));
    let u = eng.new_unit();
    let mut rs = Vec::new();
    for _ in 0..100 {
        let e = eng.values().str("e");
        eng.push(u, op, vec![e]);
        let s = eng.values().str("s");
        rs.push(eng.push(u, op, vec![s]));
    }
    (eng, rs)
}

fn outs(eng: &Engine, rs: &[defuse::NodeId]) -> Vec<Option<i64>> {
    rs.iter().map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x))).collect()
}

#[test]
fn readers_wait_for_dynamic_contributors_of_their_field() {
    let (mut cold, crs) = build();
    cold.evaluate_sequential().unwrap();
    let want = outs(&cold, &crs);
    let mut stops = 0;
    for _ in 0..5 {
        let (mut eng, rs) = build();
        let r: Report = eng
            .evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 256), &mut Threads)
            .unwrap();
        assert_eq!(outs(&eng, &rs), want);
        stops += r.stops;
    }
    assert!(stops <= 5, "stops {stops}");
}
