//! An edit that comes while a run goes on for long is taken during it:
//! the worker that asks the edit source asks it while idle, not only
//! after a run of its own or at the quiet point, which that long run
//! keeps away; what the edit changes under the running node stops the
//! run at its next check, and it runs again on the edited input.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, EditSource, Engine, Field, NodeId, OpId, Operation, Stop, ValueId};

/// How long a run on the old input spins when nothing stops it.
const SPIN: Duration = Duration::from_secs(20);

/// `[f, x]`: defines f as x.
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

/// `[f]`: reads f; on 1, spins (a TeX loop on a stale state) for
/// [`SPIN`], checking the engine (`Ctx::tick`) as a TeX run does every
/// few thousand expansions, unless it runs on `caller` (the thread that
/// asks the edit source, which cannot ask while it runs).
struct Spin {
    caller: Mutex<Option<ThreadId>>,
    /// Whether a run spins now, and the runs that spun on another thread.
    spinning: Arc<AtomicBool>,
    spun: AtomicU64,
    /// The time between two checks.
    every: Duration,
    /// The checks before it says it spins (the edit comes then), and the
    /// checks it made in its last spinning run.
    arm: u64,
    checks: AtomicU64,
}

impl Operation for Spin {
    fn name(&self) -> &'static str {
        "spin"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        if cx.values().as_int(x) != Some(1) || *self.caller.lock().unwrap() == Some(std::thread::current().id()) {
            return Ok(x);
        }
        self.spun.fetch_add(1, Ordering::Relaxed);
        let t = Instant::now();
        let mut k = 0;
        let r = loop {
            if let Err(s) = cx.tick(1) {
                break Err(s);
            }
            k += 1;
            if k == self.arm {
                self.spinning.store(true, Ordering::SeqCst);
            }
            if t.elapsed() > SPIN {
                break Ok(x);
            }
            std::thread::sleep(self.every);
        };
        self.checks.store(k, Ordering::SeqCst);
        self.spinning.store(false, Ordering::SeqCst);
        r
    }
}

/// Once a run spins: the definition replaced by `f = 2`, version 7.
struct Edit {
    node: NodeId,
    def: OpId,
    f: Field,
    spinning: Arc<AtomicBool>,
    sent: bool,
}

impl EditSource for Edit {
    fn absorb(&mut self, eng: &Engine) -> Option<u64> {
        if self.sent || !self.spinning.load(Ordering::SeqCst) {
            return None;
        }
        self.sent = true;
        let v = eng.values();
        eng.replace(self.node, self.def, vec![self.f.0, v.int(2)]);
        Some(7)
    }
}

#[test]
fn an_edit_is_taken_while_a_run_spins_on_another_worker() {
    let mut tried = 0;
    loop {
        tried += 1;
        assert!(tried <= 50, "the spinning run never ran off the caller's thread");
        let e = Engine::new();
        let def = e.register(Arc::new(Def));
        let spinning = Arc::new(AtomicBool::new(false));
        let spin = Arc::new(Spin {
            caller: Mutex::new(Some(std::thread::current().id())),
            spinning: spinning.clone(),
            spun: AtomicU64::new(0),
            every: Duration::from_millis(1),
            arm: 1,
            checks: AtomicU64::new(0),
        });
        let sp = e.register(spin.clone());
        let v = e.values();
        let f = v.field("f");
        let u = e.new_unit();
        let d = e.push(u, def, vec![f.0, v.int(1)]);
        let r = e.push(u, sp, vec![f.0]);
        let mut src = Edit {
            node: d,
            def,
            f,
            spinning,
            sent: false,
        };
        let t = Instant::now();
        let rep = e.evaluate_absorbing(2, &mut src).unwrap();
        let took = t.elapsed();
        if spin.spun.load(Ordering::Relaxed) == 0 {
            // (it ran on the caller's thread: nothing to show)
            continue;
        }
        assert!(src.sent, "the edit was not taken");
        assert_eq!(rep.absorbed.first().map(|a| a.0), Some(7), "{:?}", rep.absorbed);
        assert_eq!(e.values().as_int(e.output(r).expect("ran")), Some(2));
        assert!(
            took < SPIN / 4,
            "the edit waited for the spinning run: {took:?} (tries {tried})"
        );
        break;
    }
}

#[test]
fn a_reader_re_asked_while_it_spins_is_stopped_at_its_next_check() {
    // Its read filed by an earlier commit, a walk re-asks it while it
    // runs (left to its commit, which a spinning run never reaches): the
    // re-ask is kept for the run's next check, which finds the read moved
    // and stops it at once, not at its own next look at a long run's
    // reads (its 128th check here).
    let mut tried = 0;
    loop {
        tried += 1;
        assert!(tried <= 50, "the spinning run never ran off the caller's thread");
        let e = Engine::new();
        let def = e.register(Arc::new(Def));
        let spinning = Arc::new(AtomicBool::new(false));
        let spin = Arc::new(Spin {
            caller: Mutex::new(Some(std::thread::current().id())),
            spinning: spinning.clone(),
            spun: AtomicU64::new(0),
            every: Duration::from_millis(1),
            arm: 70,
            checks: AtomicU64::new(0),
        });
        let sp = e.register(spin.clone());
        let v = e.values();
        let f = v.field("f");
        let u = e.new_unit();
        let d = e.push(u, def, vec![f.0, v.int(3)]);
        let r = e.push(u, sp, vec![f.0]);
        e.evaluate(2).unwrap();
        assert_eq!(v.as_int(e.output(r).expect("ran")), Some(3));
        e.replace(d, def, vec![f.0, v.int(1)]);
        // (the reader queued too, its record kept: it runs on the other
        // worker as the caller's runs the definition)
        e.replace(r, sp, vec![f.0]);
        let mut src = Edit {
            node: d,
            def,
            f,
            spinning,
            sent: false,
        };
        let t = Instant::now();
        let rep = e.evaluate_absorbing(2, &mut src).unwrap();
        let took = t.elapsed();
        if spin.spun.load(Ordering::Relaxed) == 0 {
            continue;
        }
        assert!(src.sent, "the edit was not taken");
        assert_eq!(e.values().as_int(e.output(r).expect("ran")), Some(2));
        assert!(rep.stopped_stale >= 1, "{rep:?}");
        // (the edit comes at its 70th check; its own next look is the
        // 128th)
        let k = spin.checks.load(Ordering::SeqCst);
        assert!(
            k < 100,
            "the re-ask waited for the run's own look: stopped at check {k}, {took:?} (tries {tried})"
        );
        break;
    }
}
