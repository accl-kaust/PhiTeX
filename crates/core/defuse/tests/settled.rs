//! `Ctx::wait_settled`: an effect outside the engine (a shell command) is
//! made only at the committed frontier, every node before its node
//! settled, never on a state the line's own order never had, at any
//! number of threads.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

fn spin(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

/// `[f, x, ms]`: works `ms` milliseconds, then defines f = x.
struct SlowDef;

impl Operation for SlowDef {
    fn name(&self) -> &'static str {
        "slow_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0);
        spin(Duration::from_millis(ms.unsigned_abs()));
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[f, g]`: g := f (the definition reaching it, UNIT if none).
struct Copy;

impl Operation for Copy {
    fn name(&self) -> &'static str {
        "copy"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        cx.define(Field(cx.args()[1]), x);
        Ok(x)
    }
}

/// `[g]`: reads g, waits for the committed frontier, then makes its
/// effect: g's value noted.
struct Effect(Mutex<Vec<ValueId>>, AtomicU64);

impl Operation for Effect {
    fn name(&self) -> &'static str {
        "effect"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.1.fetch_add(1, Ordering::Relaxed);
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        cx.wait_settled()?;
        self.0.lock().unwrap().push(x);
        Ok(x)
    }
}

/// A slow definer, a copy of what it defines read (first) before it
/// commits, so committed on a guess and re-asked, and the effect after
/// both: it is made once, on the value the line's order gives, at -j1,
/// -j2 and -j8; an edit of the definer makes it once more, on the new
/// value.
#[test]
fn an_effect_waits_for_every_node_before_it_to_settle() {
    for threads in [1, 2, 8] {
        for _ in 0..20 {
            let e = Engine::new();
            let v = e.values();
            let def = e.register(Arc::new(SlowDef));
            let copy = e.register(Arc::new(Copy));
            let eff = Arc::new(Effect(Mutex::new(Vec::new()), AtomicU64::new(0)));
            let eop = e.register(eff.clone());
            let (f, g) = (v.field("f"), v.field("g"));
            let u = e.new_unit();
            let d = e.push(u, def, vec![f.0, v.int(7), v.int(20)]);
            e.push(u, copy, vec![f.0, g.0]);
            e.push(u, eop, vec![g.0]);
            e.evaluate(threads).unwrap();
            assert_eq!(*eff.0.lock().unwrap(), vec![v.int(7)], "-j{threads}");
            e.replace(d, def, vec![f.0, v.int(9), v.int(20)]);
            e.evaluate(threads).unwrap();
            assert_eq!(*eff.0.lock().unwrap(), vec![v.int(7), v.int(9)], "-j{threads}");
            assert!(e.check_invariants().is_empty());
        }
    }
}
