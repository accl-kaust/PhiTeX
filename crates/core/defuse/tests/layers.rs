//! Layers (`Operation::layer`): a dirty node runs only while no node of a
//! lower layer is dirty or running. A loop over the lower layers'
//! contributions then runs once, over all of them, with no hole; and a
//! lower node a higher layer's commit re-asks runs before the higher
//! layers go on, so what they read of it is its final state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId, Values};

fn spin(ms: u64) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        std::hint::spin_loop();
    }
}

/// `[s, x, ms]`, layer 2: works `ms` milliseconds, contributes x to s,
/// and counts itself done at the end of its run.
struct Para {
    done: AtomicU64,
}

impl Operation for Para {
    fn name(&self) -> &'static str {
        "para"
    }

    fn layer(&self) -> u8 {
        2
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0).unsigned_abs();
        spin(ms);
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        self.done.fetch_add(1, Ordering::SeqCst);
        Ok(cx.args()[1])
    }
}

/// `[s]`, layer 4: folds s from the unit's start, contribution by
/// contribution, and defines `total`. Records how many paragraphs were
/// done when each of its runs began, and the holes it met.
struct Loop {
    paras: Arc<Para>,
    runs: AtomicU64,
    done_at_start: Mutex<Vec<u64>>,
    holes: AtomicU64,
}

impl Operation for Loop {
    fn name(&self) -> &'static str {
        "loop"
    }

    fn layer(&self) -> u8 {
        4
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.done_at_start.lock().unwrap().push(self.paras.done.load(Ordering::SeqCst));
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let mut sum = 0i64;
        let mut cur = None;
        while let Some((n, x)) = cx.next_contribution(s, cur)? {
            sum += v.as_int(x).unwrap_or(0);
            cur = Some(n);
        }
        if cx.stopped_at().is_some() {
            self.holes.fetch_add(1, Ordering::SeqCst);
        }
        cx.define(v.field("total"), v.int(sum));
        Ok(v.int(sum))
    }
}

/// `[]`, layer 5: reads `total`; records what it saw at each run.
struct Fin {
    runs: AtomicU64,
    seen: Mutex<Vec<i64>>,
}

impl Operation for Fin {
    fn name(&self) -> &'static str {
        "fin"
    }

    fn layer(&self) -> u8 {
        5
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let t = cx.read(v.field("total"))?.and_then(|x| v.as_int(x)).unwrap_or(-1);
        self.seen.lock().unwrap().push(t);
        Ok(v.int(t))
    }
}

/// Twenty-four slow paragraphs, then the loop, then the finisher. At eight
/// threads the loop's worker reaches it while the paragraphs run; without
/// the layers it would run over holes and again at each fill. With them
/// it runs once, when every paragraph is done, and the finisher once,
/// seeing the one total; the output is the same at one thread and eight.
#[test]
fn a_layer_runs_once_the_layers_below_it_are_quiet() {
    for threads in [1, 8] {
        let e = Engine::new();
        let v = e.values();
        let paras = Arc::new(Para { done: AtomicU64::new(0) });
        let para = e.register(paras.clone());
        let lp = Arc::new(Loop {
            paras: paras.clone(),
            runs: AtomicU64::new(0),
            done_at_start: Mutex::new(Vec::new()),
            holes: AtomicU64::new(0),
        });
        let lop = e.register(lp.clone());
        let fin = Arc::new(Fin {
            runs: AtomicU64::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let fop = e.register(fin.clone());
        let s = v.field("s");
        let u = e.new_unit();
        for i in 0..24 {
            e.push(u, para, vec![s.0, v.int(i + 1), v.int(30)]);
        }
        let l = e.push(u, lop, vec![s.0]);
        let f = e.push(u, fop, vec![]);
        let r = e.evaluate(threads).unwrap();
        assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(300), "-j{threads}: {r:?}");
        assert_eq!(e.output(f).and_then(|x| v.as_int(x)), Some(300), "-j{threads}: {r:?}");
        assert_eq!(lp.runs.load(Ordering::SeqCst), 1, "-j{threads}: the loop ran once, over the quiet lower layer: {r:?}");
        assert_eq!(lp.holes.load(Ordering::SeqCst), 0, "-j{threads}: it met no hole: {r:?}");
        assert_eq!(*lp.done_at_start.lock().unwrap(), vec![24], "-j{threads}: every paragraph was done when it began: {r:?}");
        assert_eq!(fin.runs.load(Ordering::SeqCst), 1, "-j{threads}: the finisher ran once: {r:?}");
        assert_eq!(*fin.seen.lock().unwrap(), vec![300], "-j{threads}: it saw the one total: {r:?}");
        if threads > 1 {
            assert!(r.layer_holds >= 1, "-j{threads}: the loop or the finisher was reached early and held: {r:?}");
        }
        assert!(e.check_invariants().is_empty(), "-j{threads}: {:?}", e.check_invariants());
    }
}

/// `[]`, layer 5: defines `page` := 7, as a finisher stepping a counter.
struct Stepper {
    runs: AtomicU64,
}

impl Operation for Stepper {
    fn name(&self) -> &'static str {
        "stepper"
    }

    fn layer(&self) -> u8 {
        5
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        cx.define(v.field("page"), v.int(7));
        Ok(ValueId::UNIT)
    }
}

/// `[]`, layer 2: reads `page` and defines `saw` := what it read (-1 for
/// none), as a paragraph reading a counter a finisher after it moves.
struct Reader {
    runs: AtomicU64,
}

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn layer(&self) -> u8 {
        2
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let p = cx.read(v.field("page"))?.and_then(|x| v.as_int(x)).unwrap_or(-1);
        cx.define(v.field("saw"), v.int(p));
        Ok(v.int(p))
    }
}

/// `[]`, layer 6: reads `saw`; records what it saw at each run.
struct End {
    runs: AtomicU64,
    seen: Mutex<Vec<i64>>,
}

impl Operation for End {
    fn name(&self) -> &'static str {
        "end"
    }

    fn layer(&self) -> u8 {
        6
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let s = cx.read(v.field("saw"))?.and_then(|x| v.as_int(x)).unwrap_or(-2);
        self.seen.lock().unwrap().push(s);
        Ok(v.int(s))
    }
}

/// The stepper (layer 5) stands first on the line, the reader (layer 2)
/// after it, the end (layer 6) last. The reader runs first, with no
/// `page` yet; the stepper, let go once the reader is done, defines it
/// and re-asks the reader; the end, held while the reader is dirty
/// again, runs once the reader has its final answer, and sees 7 once.
/// Nothing waits for ever: the stepper's re-ask makes layer 2 busy again
/// and the higher layers wait for it a second time.
#[test]
fn a_lower_node_re_asked_by_a_higher_layer_runs_before_the_higher_layers_go_on() {
    for threads in [1, 8] {
        let e = Engine::new();
        let v = e.values();
        let st = Arc::new(Stepper { runs: AtomicU64::new(0) });
        let sop = e.register(st.clone());
        let rd = Arc::new(Reader { runs: AtomicU64::new(0) });
        let rop = e.register(rd.clone());
        let en = Arc::new(End {
            runs: AtomicU64::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let eop = e.register(en.clone());
        let u = e.new_unit();
        e.push(u, sop, vec![]);
        let r_ = e.push(u, rop, vec![]);
        let x = e.push(u, eop, vec![]);
        let r = e.evaluate(threads).unwrap();
        assert_eq!(e.output(r_).and_then(|y| v.as_int(y)), Some(7), "-j{threads}: {r:?}");
        assert_eq!(e.output(x).and_then(|y| v.as_int(y)), Some(7), "-j{threads}: {r:?}");
        assert_eq!(st.runs.load(Ordering::SeqCst), 1, "-j{threads}: {r:?}");
        assert_eq!(rd.runs.load(Ordering::SeqCst), 2, "-j{threads}: the reader ran before the stepper and once more after it: {r:?}");
        assert_eq!(en.runs.load(Ordering::SeqCst), 1, "-j{threads}: the end ran once, after the reader's second run: {r:?}");
        assert_eq!(*en.seen.lock().unwrap(), vec![7], "-j{threads}: {r:?}");
        assert!(r.layer_holds >= 1, "-j{threads}: the stepper and the end were held: {r:?}");
        assert!(e.check_invariants().is_empty(), "-j{threads}: {:?}", e.check_invariants());
    }
}

/// A node of layer 0 pushed after nodes of a higher layer runs without
/// waiting for them: lower layers never wait.
#[test]
fn a_lower_layer_never_waits_for_a_higher_one() {
    let e = Engine::new();
    let v = e.values();
    let paras = Arc::new(Para { done: AtomicU64::new(0) });
    let para = e.register(paras.clone());
    let en = Arc::new(End {
        runs: AtomicU64::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let eop = e.register(en.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let x = e.push(u, eop, vec![]);
    let p = e.push(u, para, vec![s.0, v.int(1), v.int(0)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(p).and_then(|y| v.as_int(y)), Some(1), "{r:?}");
    assert_eq!(en.runs.load(Ordering::SeqCst), 1, "{r:?}");
    assert!(e.output(x).is_some(), "{r:?}");
    assert!(e.check_invariants().is_empty());
}
