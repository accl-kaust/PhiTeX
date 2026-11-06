//! `Step::Within`: a reader that checked each element of a list it saw
//! stands while the list loses elements or keeps them, and is asked
//! again only when it gains one it never checked.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Step, Stop, ValueId};

/// `[f, x]`: defines f := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define_with(Field(cx.args()[0]), cx.args()[1], Locality::Global);
        Ok(cx.args()[1])
    }
}

/// `[f, k]`: whether the list f holds is within list k; counts its runs.
struct Checker {
    runs: AtomicU64,
}

impl Operation for Checker {
    fn name(&self) -> &'static str {
        "checker"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let p = cx.path(&[Step::Within(cx.args()[1])]);
        Ok(cx.read_path(Field(cx.args()[0]), p)?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_reader_within_a_list_stands_while_it_shrinks() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let c = Arc::new(Checker {
            runs: AtomicU64::new(0),
        });
        let checker = e.register(c.clone());
        let f = v.field("names");
        let (a, b, cc, d) = (v.str("a"), v.str("b"), v.str("c"), v.str("d"));
        let u = e.new_unit();
        let n = e.push(u, def, vec![f.0, v.list(vec![a, b])]);
        let r = e.push(u, checker, vec![f.0, v.list(vec![a, b, cc])]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.output(r), Some(v.bool(true)));
        let r0 = c.runs.load(Ordering::SeqCst);
        // (an element gone, and the others in another order: within still)
        e.replace(n, def, vec![f.0, v.list(vec![b])]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(c.runs.load(Ordering::SeqCst), r0, "-j{threads}: {rep:?}");
        e.replace(n, def, vec![f.0, v.list(vec![cc, a])]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(c.runs.load(Ordering::SeqCst), r0, "-j{threads}: {rep:?}");
        // (an element it never checked: asked again)
        e.replace(n, def, vec![f.0, v.list(vec![a, d])]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(c.runs.load(Ordering::SeqCst), r0 + 1, "-j{threads}: {rep:?}");
        assert_eq!(e.output(r), Some(v.bool(false)));
        assert!(e.check_invariants().is_empty());
    }
}
