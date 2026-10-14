//! Appends N nodes to one unit with `Engine::push`, then with
//! `Engine::push_many`, and prints the time of each (into an empty unit
//! and after an evaluation; static and dynamic definition sets).
//!
//! `cargo run --release --example pushbench -- [N]`

use std::sync::Arc;
use std::time::Instant;

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Op(bool);

impl Operation for Op {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        if self.0 { None } else { Some(Vec::new()) }
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// The client's shape: node `i` reads 10-40 fields and may define as many
/// (names mostly its own), with its definition set dynamic and bounded.
/// Its answers are computed before timing, as the client caches its own.
struct Wide {
    fields: Vec<(Vec<Field>, Vec<Field>)>,
    decl: bool,
    reads: bool,
}

fn wide_index(v: &Values, a: &[ValueId]) -> usize {
    match a.get(1).map(|&x| v.get(x)).as_deref() {
        Some(defuse::Value::Int(k)) => usize::try_from(*k).unwrap_or(0),
        _ => 0,
    }
}

impl Operation for Wide {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        None
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> defuse::MayDefine {
        if !self.decl {
            return defuse::MayDefine::Any;
        }
        defuse::MayDefine::Names(self.fields[wide_index(v, a)].0.clone())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        if !self.reads {
            return None;
        }
        Some(self.fields[wide_index(v, a)].1.clone())
    }
    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

fn main() {
    if std::env::var("WIDE").is_ok() {
        let n: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5204);
        for _ in 0..3 {
            let mut eng = Engine::new();
            let v = eng.values();
            // Distinct names, mostly per node: integers stand for them (no
            // formatting in the setup, so a profile shows the engine).
            let names = |tag: i64, i: i64| -> Vec<Field> {
                (0..10 + i % 31)
                    .map(|j| Field(v.int(tag * 1_000_000 + (i % 4000) * 100 + j)))
                    .collect()
            };
            let fields = (0..n).map(|i| (names(1, i), names(2, i))).collect();
            let op = eng.register(Arc::new(Wide {
                fields,
                decl: std::env::var("NO_DECL").is_err(),
                reads: std::env::var("NO_READS").is_err(),
            }));
            let u = eng.new_unit();
            let args: Vec<Vec<ValueId>> = (0..n)
                .map(|i| vec![eng.values().str("cmd"), eng.values().int(i)])
                .collect();
            let t = Instant::now();
            eng.push_many(u, op, args);
            println!("wide push_many {n}: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
        }
        return;
    }
    if std::env::var("ONLY_MANY").is_ok() {
        let n: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5202);
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Op(false)));
        let u = eng.new_unit();
        let args: Vec<Vec<ValueId>> = (0..n)
            .map(|i| vec![eng.values().str("cmd"), eng.values().int(i)])
            .collect();
        let t = Instant::now();
        eng.push_many(u, op, args);
        println!("push_many {n}: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
        return;
    }
    let n: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5202);
    for dynamic in [false, true] {
        for warm in [false, true] {
            let mut eng = Engine::new();
            let op = eng.register(Arc::new(Op(dynamic)));
            let u = eng.new_unit();
            let args: Vec<Vec<ValueId>> = (0..n)
                .map(|i| vec![eng.values().str("cmd"), eng.values().int(i)])
                .collect();
            if warm {
                eng.push(u, op, args[0].clone());
                eng.evaluate_sequential().expect("evaluate");
            }
            let t = Instant::now();
            for a in args.clone() {
                eng.push(u, op, a);
            }
            let p = t.elapsed();
            let mut e2 = Engine::new();
            let op2 = e2.register(Arc::new(Op(dynamic)));
            let u2 = e2.new_unit();
            if warm {
                e2.push(u2, op2, args[0].clone());
                e2.evaluate_sequential().expect("evaluate");
            }
            let t = Instant::now();
            e2.push_many(u2, op2, args.clone());
            let pm = t.elapsed();
            let t = Instant::now();
            eng.evaluate_sequential().expect("evaluate");
            println!(
                "dynamic {dynamic} after an evaluation {warm}: push {n}: {:.2} ms, push_many {:.2} ms, evaluate {:.2} ms",
                p.as_secs_f64() * 1e3,
                pm.as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}
