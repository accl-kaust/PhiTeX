//! A run's first reads are filed nowhere a walk can re-ask it by, so a
//! definition that moves one while the run is long is found only by the
//! run's own checks (`Ctx::tick`): from its 64th check on, at powers of
//! two. Driven check by check here, it is stopped at the first such check
//! after the definition committed, and runs again on it. (A TeX run
//! counts the tokens its macro calls write toward those checks,
//! `latex::a_macro_call_counts_its_tokens_toward_the_runs_checks`: a
//! recursion with a growing argument on such a read, Grail's resumes,
//! ran to "TeX capacity exceeded" between two of them.)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, Operation, Stop, ValueId};

struct Flags {
    read: AtomicBool,
    defined: AtomicBool,
    /// The check the reader made after the definition committed, and the
    /// check it was stopped at (0: none).
    after: AtomicU64,
    stopped: AtomicU64,
}

/// `[f, x]`: once the reader has read `f`, defines `f` := x.
struct Def(Arc<Flags>);

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let t = Instant::now();
        // (checking in: what waits behind it on its worker goes to another,
        // `Ctx::tick`)
        while !self.0.read.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(2) {
            cx.tick(1)?;
            std::thread::sleep(Duration::from_millis(1));
        }
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        self.0.defined.store(true, Ordering::SeqCst);
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: reads `f`; where it has none, checks in (`Ctx::tick`) up to
/// 4096 times, noting the check after the definition has had time to
/// commit and the check it is stopped at.
struct Long(Arc<Flags>);

impl Operation for Long {
    fn name(&self) -> &'static str {
        "long"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?;
        self.0.read.store(true, Ordering::SeqCst);
        if x.is_some() {
            return Ok(cx.values().int(1));
        }
        for k in 1..=4096u64 {
            if self.0.after.load(Ordering::SeqCst) == 0 && self.0.defined.load(Ordering::SeqCst) {
                // (its commit follows the run's end: given time to land)
                std::thread::sleep(Duration::from_millis(50));
                self.0.after.store(k, Ordering::SeqCst);
            }
            if let Err(s) = cx.tick(1) {
                self.0.stopped.store(k, Ordering::SeqCst);
                return Err(s);
            }
        }
        Ok(cx.values().int(0))
    }
}

#[test]
fn a_first_runs_moved_read_stops_it_at_its_next_recheck() {
    for tried in 1..=20 {
        let flags = Arc::new(Flags {
            read: AtomicBool::new(false),
            defined: AtomicBool::new(false),
            after: AtomicU64::new(0),
            stopped: AtomicU64::new(0),
        });
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def(flags.clone())));
        let long = e.register(Arc::new(Long(flags.clone())));
        let u = e.new_unit();
        let f = v.field("f");
        e.push(u, def, vec![f.0, v.int(5)]);
        let b = e.push(u, long, vec![f.0]);
        let rep = e.evaluate(4).unwrap();
        assert_eq!(e.output(b).and_then(|x| v.as_int(x)), Some(1), "{rep:?}");
        let after = flags.after.load(Ordering::SeqCst);
        if after == 0 {
            // (the reader ran after the definition, or the definer waited
            // it out: not the case asked about, again)
            assert!(tried < 20, "the reader never ran on the old value beside the definer");
            continue;
        }
        // (the first check from the 64th on that is a power of two, at or
        // after the one that followed the commit)
        let want = after.max(64).next_power_of_two();
        assert_eq!(flags.stopped.load(Ordering::SeqCst), want, "{rep:?}");
        assert!(rep.stopped_stale >= 1, "{rep:?}");
        return;
    }
}
