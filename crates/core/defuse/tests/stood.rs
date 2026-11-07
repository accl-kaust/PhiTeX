//! A node woken by a change that is undone before it runs: every read of
//! its last run still holds, so its operation does not run and its last
//! record stands (`Ctx::last_run_stands`). Its commit files nothing again
//! (`commit::commit_stood`): the readers its last commit filed stay, and
//! a later change of what it read asks it again as before.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

/// `[name, x]`: defines field `name` = x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[n]`: the sum of fields `f0` .. `f<n-1>`, then of `x`; counts its runs.
struct Sum(AtomicU64);

impl Operation for Sum {
    fn name(&self) -> &'static str {
        "sum"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap_or(0);
        let mut s = 0;
        for i in 0..n {
            s += cx
                .read(v.field(&format!("f{i}")))?
                .and_then(|x| v.as_int(x))
                .unwrap_or(0);
        }
        s += cx.read(v.field("x"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        Ok(v.int(s))
    }
}

#[test]
fn a_stood_reader_stays_filed() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let sum = Arc::new(Sum(AtomicU64::new(0)));
        let op = e.register(sum.clone());
        let u = e.new_unit();
        let fs: Vec<_> = (0..100)
            .map(|i| e.push(u, def, vec![v.str(&format!("f{i}")), v.int(1)]))
            .collect();
        // x = 1, then a definer of y, then the reader of f0..f99 and x
        let d1 = e.push(u, def, vec![v.str("x"), v.int(1)]);
        let d2 = e.push(u, def, vec![v.str("y"), v.int(0)]);
        let r = e.push(u, op, vec![v.int(100)]);
        e.evaluate(threads).unwrap();
        assert_eq!(v.as_int(e.output(r).unwrap()), Some(101));
        let runs = || sum.0.load(Ordering::Relaxed);
        let first = runs();
        // (x moves to 2 at d1 and back to 1 at d2: the reader, asked by
        // d1's commit, finds x as it read it)
        e.replace(d1, def, vec![v.str("x"), v.int(2)]);
        e.replace(d2, def, vec![v.str("x"), v.int(1)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(v.as_int(e.output(r).unwrap()), Some(101), "-j{threads}");
        assert!(
            e.check_invariants().is_empty(),
            "-j{threads}: {:?}",
            e.check_invariants()
        );
        if threads == 1 {
            assert_eq!(runs(), first, "-j1: the reader stands, its operation not run: {rep:?}");
        }
        // (its readers stood with it: a change of x at d2 asks it again)
        let before = runs();
        e.replace(d2, def, vec![v.str("x"), v.int(5)]);
        e.evaluate(threads).unwrap();
        assert_eq!(v.as_int(e.output(r).unwrap()), Some(105), "-j{threads}");
        assert!(runs() > before, "-j{threads}");
        // (and one of the hundred fields it read)
        e.replace(fs[7], def, vec![v.str("f7"), v.int(10)]);
        e.evaluate(threads).unwrap();
        assert_eq!(v.as_int(e.output(r).unwrap()), Some(114), "-j{threads}");
        assert!(
            e.check_invariants().is_empty(),
            "-j{threads}: {:?}",
            e.check_invariants()
        );
    }
}
