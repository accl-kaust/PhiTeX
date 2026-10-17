//! Commit and check cost against a run's read and definition counts:
//! `setsize`. One unit: `src` defines R fields; `w` reads them all and
//! defines D fields; after an edit of `src` (one value changes), `w`
//! re-runs. Prints commit and check time per run of `w` for each R and D.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops;

fn n(v: &Values, a: &[ValueId], i: usize) -> usize {
    usize::try_from(v.as_int(a[i]).unwrap_or(0)).unwrap_or(0)
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let (k, cnt) = (v.as_str(a[0]).unwrap_or_default(), n(v, a, 1));
        let p = if k == "src" { "r" } else { "d" };
        Some(
            (0..cnt)
                .map(|i| (v.field(&format!("{p}{i}")), Locality::Local))
                .collect(),
        )
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if v.as_str(a[0]).unwrap_or_default() == "src" {
            for i in 0..n(v, &a, 1) {
                cx.define(v.field(&format!("r{i}")), v.int(i as i64 + v.as_int(a[2]).unwrap_or(0)));
            }
            return Ok(ValueId::UNIT);
        }
        let mut s = 0i64;
        for i in 0..n(v, &a, 2) {
            s += cx
                .use_field(v.field(&format!("r{i}")))?
                .and_then(|x| v.as_int(x))
                .unwrap_or(0);
        }
        for i in 0..n(v, &a, 1) {
            cx.define(v.field(&format!("d{i}")), v.int(s + i as i64));
        }
        Ok(v.int(s))
    }
}

fn main() {
    println!(
        "{:>6} {:>6} {:>12} {:>12} {:>12}",
        "reads", "defs", "commit µs", "check µs", "op µs"
    );
    let cases: Vec<(i64, i64)> = match std::env::var("CASE")
        .ok()
        .and_then(|s| s.split_once(',').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap())))
    {
        Some(c) => vec![c],
        None => vec![
            (10, 10),
            (100, 10),
            (1000, 10),
            (10000, 10),
            (10, 100),
            (10, 1000),
            (10, 10000),
            (1000, 1000),
        ],
    };
    for &(r, d) in &cases {
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops));
        let u = eng.new_unit();
        let vs = eng.values();
        let (srcs, w, rr, dd, z) = (vs.str("src"), vs.str("w"), vs.int(r), vs.int(d), vs.int(0));
        let sn = eng.push(u, op, vec![srcs, rr, z]);
        eng.push(u, op, vec![w, dd, rr]);
        eng.evaluate_sequential().unwrap();
        let (mut c, mut k, mut o) = (0u64, 0u64, 0u64);
        let reps = 20;
        for i in 1..=reps {
            let x = eng.values().int(i);
            eng.replace(sn, op, vec![srcs, rr, x]);
            let rep = eng.evaluate_sequential().unwrap();
            for t in &rep.round_times {
                c += t.commit_ns;
                k += t.check_ns;
                o += t.op_ns;
            }
        }
        // Two runs per edit (src and w); report per run.
        let per = |x: u64| x as f64 / 1000.0 / f64::from(reps as u32 * 2);
        println!("{r:>6} {d:>6} {:>12.1} {:>12.1} {:>12.1}", per(c), per(k), per(o));
    }
}
