//! `Ctx::wait_settled`: an effect outside the engine (a shell command) is
//! made only at the committed frontier, every node before its node
//! settled, never on a state the line's own order never had, at any
//! number of threads.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// `[]`: runs only once the line is quiet (`Operation::final_only`), as
/// a page's PDF side does; defines nothing.
struct Writer;

impl Operation for Writer {
    fn name(&self) -> &'static str {
        "writer"
    }

    fn final_only(&self) -> bool {
        true
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }
}

/// A node of an operation that runs only once the line is quiet stands
/// before the effect, dirty until then: the effect does not wait for it
/// (nothing an evaluation needs may wait on one), it is made once and the
/// evaluation ends. Before, the effect parked on it, the quiet point let
/// the effect go, it parked on it again, for ever (a minted listing's
/// `\write18` after a page shipped as PDF, its PDF side before it: the
/// gate's latexmk_minted past 300 s, lectures and handin-doc "ran N
/// times").
#[test]
fn an_effect_does_not_wait_for_a_node_that_runs_once_the_line_is_quiet() {
    for threads in [1, 2, 8] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let e = Engine::new();
            let v = e.values();
            let def = e.register(Arc::new(SlowDef));
            let writer = e.register(Arc::new(Writer));
            let eff = Arc::new(Effect(Mutex::new(Vec::new()), AtomicU64::new(0)));
            let eop = e.register(eff.clone());
            let g = v.field("g");
            let u = e.new_unit();
            e.push(u, def, vec![g.0, v.int(5), v.int(5)]);
            e.push(u, writer, Vec::new());
            e.push(u, eop, vec![g.0]);
            let ok = e.evaluate(threads).is_ok();
            let made = eff.0.lock().unwrap().clone();
            let _ = tx.send((ok, made == vec![v.int(5)], e.check_invariants().is_empty()));
        });
        let (ok, once, inv) = rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("-j{threads}: the evaluation did not end in 60 s"));
        assert!(
            ok && once && inv,
            "-j{threads}: ended {ok}, made once on 5 {once}, invariants {inv}"
        );
    }
}

/// `[f, g]`: as `Copy`, its definition held until `read` is raised.
struct LateCopy(Arc<Gates>);

impl Operation for LateCopy {
    fn name(&self) -> &'static str {
        "late_copy"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        self.0.wait(&self.0.read);
        cx.define(Field(cx.args()[1]), x);
        self.0.copied.store(true, Ordering::SeqCst);
        Ok(x)
    }
}

#[derive(Default)]
struct Gates {
    read: AtomicBool,
    copied: AtomicBool,
}

impl Gates {
    /// (bounded: a run on one thread at a time finds no one to raise it)
    fn wait(&self, flag: &AtomicBool) {
        let t = Instant::now();
        while !flag.load(Ordering::SeqCst) && t.elapsed() < Duration::from_millis(500) {
            std::hint::spin_loop();
        }
    }
}

/// `[g]`: reads g, lets the copy define it, waits for that commit, and only
/// then for the frontier; its effect: what it read, noted.
struct LateEffect(Arc<Gates>, Mutex<Vec<ValueId>>);

impl Operation for LateEffect {
    fn name(&self) -> &'static str {
        "late_effect"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        if !self.0.read.swap(true, Ordering::SeqCst) {
            self.0.wait(&self.0.copied);
            spin(Duration::from_millis(20));
        }
        cx.wait_settled()?;
        self.1.lock().unwrap().push(x);
        Ok(x)
    }
}

/// A node before the effect commits after the effect's read and before its
/// wait: every node before it has settled, and the copy's re-ask found no
/// read of the effect's filed yet. The effect is not made on the old answer
/// (the gate's -j2 on [UNIT, 7], one in many).
#[test]
fn an_effect_is_not_made_on_a_read_moved_before_its_wait() {
    let e = Engine::new();
    let v = e.values();
    let g = Arc::new(Gates::default());
    let def = e.register(Arc::new(SlowDef));
    let copy = e.register(Arc::new(LateCopy(g.clone())));
    let eff = Arc::new(LateEffect(g, Mutex::new(Vec::new())));
    let eop = e.register(eff.clone());
    let (f, h) = (v.field("f"), v.field("g"));
    let u = e.new_unit();
    e.push(u, def, vec![f.0, v.int(7), v.int(0)]);
    e.push(u, copy, vec![f.0, h.0]);
    e.push(u, eop, vec![h.0]);
    e.evaluate(4).unwrap();
    assert_eq!(*eff.1.lock().unwrap(), vec![v.int(7)]);
    assert!(e.check_invariants().is_empty());
}
