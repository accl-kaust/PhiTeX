//! A child that reads before its placer (`Ctx::read_before_placer`): a
//! child whose text stands in the middle of its placer's (a picture in a
//! figure's run) reads what reaches the placer, never what the placer
//! defines, and the placer makes the child's result its own definition.
//! Read at its own position the child would see that definition, its
//! own result, and run again on it for ever (matrix5's picture children
//! did: one child of the cli test ran 80 861 times).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId};

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

/// `[f, before]`: reads f (before its placer where `before` is 1) and
/// outputs it plus one: a counter a picture steps.
struct Step {
    runs: AtomicU64,
}

impl Operation for Step {
    fn name(&self) -> &'static str {
        "step"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        if v.as_int(cx.args()[1]) == Some(1) {
            cx.read_before_placer();
        }
        let x = cx.read(Field(cx.args()[0]))?.and_then(|x| v.as_int(x)).unwrap_or(0);
        Ok(v.int(x + 1))
    }
}

/// `[f, before, salt]`: places a step child on f and defines f := its
/// output, globally (the child's definition made where it stands in the
/// placer's text); outputs it.
struct Placer {
    step: std::sync::OnceLock<OpId>,
    runs: AtomicU64,
}

impl Operation for Placer {
    fn name(&self) -> &'static str {
        "placer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let (f, before) = (cx.args()[0], cx.args()[1]);
        let i = cx.child(*self.step.get().unwrap(), vec![f, before]);
        let x = cx.placed_output_or(i, ValueId::UNIT);
        if x != ValueId::UNIT {
            cx.define_with(Field(f), x, Locality::Global);
        }
        Ok(x)
    }
}

/// `[f]`: outputs f as it reaches it.
struct Reader;

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_child_reading_before_its_placer_sees_what_reaches_the_placer() {
    for threads in [1, 4] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let st = Arc::new(Step {
            runs: AtomicU64::new(0),
        });
        let step = e.register(st.clone());
        let p = Arc::new(Placer {
            step: std::sync::OnceLock::new(),
            runs: AtomicU64::new(0),
        });
        p.step.set(step).unwrap();
        let placer = e.register(p.clone());
        let reader = e.register(Arc::new(Reader));
        let f = v.field("count:7");
        let u = e.new_unit();
        let d = e.push(u, def, vec![f.0, v.int(10)]);
        let n = e.push(u, placer, vec![f.0, v.int(1)]);
        let r = e.push(u, reader, vec![f.0]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(11), "-j{threads}: {rep:?}");
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(11), "-j{threads}: {rep:?}");
        assert_eq!(
            st.runs.load(Ordering::SeqCst),
            1,
            "-j{threads}: the placer's definition asks the child nothing: {rep:?}"
        );
        // (a definition before the placer moves: the child runs on it,
        // the placer once more on the child's output)
        let (s0, p0) = (st.runs.load(Ordering::SeqCst), p.runs.load(Ordering::SeqCst));
        e.replace(d, def, vec![f.0, v.int(20)]);
        let rep = e.evaluate(threads).unwrap();
        assert_eq!(e.output(r).and_then(|x| v.as_int(x)), Some(21), "-j{threads}: {rep:?}");
        assert_eq!(st.runs.load(Ordering::SeqCst), s0 + 1, "-j{threads}: {rep:?}");
        assert!(p.runs.load(Ordering::SeqCst) <= p0 + 2, "-j{threads}: {rep:?}");
        assert!(e.check_invariants().is_empty());
    }
}
