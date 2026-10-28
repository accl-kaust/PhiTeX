//! A run that resumes from a parked state rests on the reads of the run
//! that parked it. A read of the line it makes again and finds moved is
//! not a fresh answer: the state was built on the old one. The commit
//! refutes the run as for a kept read that moved, naming the read's
//! start; its record would otherwise hold the new read, which holds, and
//! the stale state would stand. The page loop met this at sixteen
//! threads: woken by a change past a piece, it resumed at a page fired
//! inside that piece and read the piece again; the piece had changed
//! while the loop ran (its re-ask left to the loop's commit), and the
//! pages fired in its old units stood beside the new ones.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{Cause, Ctx, Engine, Field, NodeId, Operation, Stop, ValueId, Values};

#[derive(Default)]
struct Gates {
    /// The folder waits, on its resumed run, until the slow contributor
    /// has run (then a little more, for its commit).
    folder_waits: AtomicBool,
    slow_done: AtomicBool,
}

fn spin_until(flag: &AtomicBool, ms: u64) {
    let t = Instant::now();
    while !flag.load(Ordering::Acquire) && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(ms));
}

/// `[s, x, ms]` or `[s, unit, ms, t]`: works `ms` milliseconds,
/// contributes x (or the value of field t) to s; one that works raises
/// `slow_done` at its end.
struct Contrib(Arc<Gates>);

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[2]).unwrap_or(0).unsigned_abs();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        let x = match cx.args().get(3) {
            Some(&t) => cx.read(Field(t))?.unwrap_or(ValueId::UNIT),
            None => cx.args()[1],
        };
        cx.contribute(Field(cx.args()[0]), x);
        if ms > 0 {
            self.0.slow_done.store(true, Ordering::Release);
        }
        Ok(x)
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        true
    }
}

/// `[s, a]`: reads the contribution to s after node `a` (b's) and the one
/// after that (c's). Its state, parked: `[b, 10 * b's value]`. A run
/// offered the state keeps it unless told the change lies at or before b
/// (as the page loop keeps the checkpoints before the site), and reads b's
/// contribution again. Output: the state's value, b's value read now, and
/// c's: `11 * b + c` when the state was built from the b it reads now.
struct Folder(Arc<Gates>, AtomicU64);

impl Operation for Folder {
    fn name(&self) -> &'static str {
        "folder"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.1.fetch_add(1, Ordering::SeqCst);
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let a = NodeId::from_raw(v.as_int(cx.args()[1]).unwrap_or(0).unsigned_abs());
        let state = cx.resumed().and_then(|st| {
            let parts = v.with_list(st, <[ValueId]>::to_vec).unwrap_or_default();
            let b = NodeId::from_raw(v.as_int(parts[0]).unwrap_or(0).unsigned_abs());
            let keep = match cx.woken_by() {
                Some(Cause::Range { by, .. }) => cx.before(b, by),
                _ => false,
            };
            keep.then(|| v.as_int(parts[1]).unwrap_or(0))
        });
        if state.is_some() && self.0.folder_waits.swap(false, Ordering::AcqRel) {
            spin_until(&self.0.slow_done, 100);
        }
        let Some((b, bx)) = cx.next_contribution(s, Some(a))? else {
            return Ok(ValueId::UNIT);
        };
        let bx = v.as_int(bx).unwrap_or(0);
        let cx_ = cx.next_contribution(s, Some(b))?.map_or(0, |(_, x)| v.as_int(x).unwrap_or(0));
        let st = state.unwrap_or(10 * bx);
        cx.park(v.list(vec![v.int(i64::try_from(b.raw()).unwrap()), v.int(st)]));
        Ok(v.int(st + bx + cx_))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[name, x]`: defines `name` := x.
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

/// Two workers, the order forced by gates: t := 1, a (1), b (t, read),
/// c (100), the folder: 11 + 100. Then t becomes 2 and c 200, and b is
/// made slow: c's change re-asks the folder, which resumes (the site, c,
/// lies after b) and waits until b has run again and committed; b's
/// re-ask finds the folder running and is left to its commit; the folder
/// reads b again, 2, on the state built on 1. It must not end clean on
/// that state (10 + 2 + 200): the read it made again moved, its commit
/// names a, the read's start, as the site, and the next run goes back
/// before b (20 + 2 + 200).
#[test]
fn a_resumed_run_that_reads_again_a_moved_answer_is_refuted() {
    let e = Engine::new();
    let g = Arc::new(Gates::default());
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let contrib = e.register(Arc::new(Contrib(g.clone())));
    let folder = Arc::new(Folder(g.clone(), AtomicU64::new(0)));
    let fop = e.register(folder.clone());
    let s = v.field("s");
    let t = v.field("t");
    let u = e.new_unit();
    let d = e.push(u, def, vec![t.0, v.int(1)]);
    let a = e.push(u, contrib, vec![s.0, v.int(1), v.int(0)]);
    let b = e.push(u, contrib, vec![s.0, ValueId::UNIT, v.int(0), t.0]);
    let c = e.push(u, contrib, vec![s.0, v.int(100), v.int(0)]);
    let n = e.push(u, fop, vec![s.0, v.int(i64::try_from(a.raw()).unwrap())]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(111), "{r:?}");
    // (b slow from now: its text, unread by the folder, set before)
    e.replace(b, contrib, vec![s.0, ValueId::UNIT, v.int(300), t.0]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(111), "{r:?}");
    g.slow_done.store(false, Ordering::Release);
    g.folder_waits.store(true, Ordering::Release);
    e.replace(d, def, vec![t.0, v.int(2)]);
    e.replace(c, contrib, vec![s.0, v.int(200), v.int(0)]);
    let r = e.evaluate(2).unwrap();
    assert!(!g.folder_waits.load(Ordering::Acquire), "the folder resumed on the state built on b = 1: {r:?}");
    assert_eq!(e.output(n).and_then(|x| v.as_int(x)), Some(222), "{r:?}");
    assert!(e.check_invariants().is_empty());
}
