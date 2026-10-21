//! A node making many definitions (a format build's one big node): every
//! field's value through `Engine::def_value` (indexed past a few), and all
//! of them in one pass through `Engine::defs_of` (a field defined again in
//! the run: its last value), before and after the node runs again.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};

use defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

const N: i64 = 500;

struct Big {
    bump: AtomicI64,
}

fn f(v: &Values, i: i64) -> Field {
    v.field(&format!("r{i}"))
}

impl Operation for Big {
    fn defs(&self, _a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some((0..N).map(|i| (f(v, i), Locality::Global)).collect())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let b = self.bump.load(Relaxed);
        // (The same question many times: one read.)
        let mut seen = 0;
        for _ in 0..1000 {
            seen += i64::from(cx.use_field(v.field("input"))?.is_some());
        }
        for i in 0..N {
            cx.define(f(v, i), v.int(i * 10 + b));
        }
        // A field defined again: its last value stands.
        cx.define(f(v, 7), v.int(-1));
        Ok(v.int(seen))
    }
}

#[test]
fn a_big_nodes_definitions_are_found_by_field_and_in_one_pass() {
    let mut eng = Engine::new();
    let big = Arc::new(Big { bump: AtomicI64::new(0) });
    let op = eng.register(big.clone());
    let u = eng.new_unit();
    let n = eng.push(u, op, vec![]);
    for b in [0i64, 3] {
        big.bump.store(b, Relaxed);
        if b > 0 {
            eng.replace(n, op, vec![eng.values().int(b)]);
        }
        let r = eng.evaluate_sequential().expect("evaluate");
        assert_eq!(r.runs, 1);
        let v = eng.values();
        let want = |i: i64| if i == 7 { -1 } else { i * 10 + b };
        for i in 0..N {
            assert_eq!(eng.def_value(n, f(v, i)).and_then(|x| v.as_int(x)), Some(want(i)), "r{i}");
        }
        assert_eq!(eng.def_value(n, v.field("nothing")), None);
        let all = eng.defs_of(n);
        assert_eq!(all.len(), N as usize);
        for (k, &(fk, x)) in all.iter().enumerate() {
            let i = i64::try_from(k).unwrap();
            assert_eq!((fk, v.as_int(x)), (f(v, i), Some(want(i))));
        }
    }
}
