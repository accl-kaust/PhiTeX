//! A placer parked on a kept state that a change broke is stale: what it
//! placed waits for it. `Pages` folds the pieces with `Ctx::acc_next`,
//! keeping its cursor in its parked token, and places a finisher after
//! each node it passes naming it. Between two pieces stands a node that
//! contributes nothing. `Pages` stops on the piece after it (in work),
//! having placed its finisher, and the node goes (an edit absorbed at the
//! step's end): the finisher's reads break, and it must not run naming a
//! node that is gone; `Pages` starts over without it.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, FieldAt, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

const N: usize = 10;
/// The piece the node that contributes nothing follows.
const K: usize = 4;

struct Ops {
    me: OnceLock<OpId>,
    /// The cursor `Pages` last parked with.
    held: AtomicU64,
    /// Finisher runs naming a node that is gone.
    stale: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn vprof(v: &Values) -> Field {
    v.field("acc:vprof")
}

/// `Pages`'s kept state: the cursor, the total so far, the nodes passed.
struct Tok(Option<NodeId>, i64, Vec<NodeId>);

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        matches!(tag(v, a).as_str(), "pages" | "fin").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        let f = vprof(v);
        match tag(v, &a).as_str() {
            "pages" => {
                let Tok(mut after, mut total, mut passed) = cx.parked().and_then(|t| t.downcast::<Tok>().ok()).map_or(Tok(None, 0, Vec::new()), |t| *t);
                // (A resumed run places again what it placed.)
                for &x in &passed {
                    cx.follow_after_any(x, op, vec![v.str("fin"), v.int(i64::try_from(x.raw()).unwrap_or(0))]);
                }
                loop {
                    match cx.acc_next(f, after) {
                        Ok(Some((x, h))) => {
                            total += v.as_int(h).unwrap_or(0);
                            cx.follow_after_any(x, op, vec![v.str("fin"), v.int(i64::try_from(x.raw()).unwrap_or(0))]);
                            passed.push(x);
                            after = Some(x);
                        }
                        Ok(None) => break,
                        Err(s) => {
                            self.held.store(after.map_or(0, NodeId::raw), Relaxed);
                            cx.park(Box::new(Tok(after, total, passed)));
                            return Err(s);
                        }
                    }
                }
                Ok(v.int(total))
            }
            "fin" => {
                let x = NodeId::from_raw(u64::try_from(v.as_int(a[1]).unwrap_or(0)).unwrap_or(0));
                let prev = cx.prev_sibling_where(&|_, args| v.as_str(args[0]).as_deref() == Some("fin"));
                let h = cx.acc_range(f, prev)?;
                if matches!(cx.use_field_at(v.field("pg"), Some(x))?, FieldAt::Gone) {
                    self.stale.fetch_add(1, Relaxed);
                }
                Ok(h)
            }
            "res" => {
                cx.contribute(f, v.int(0));
                Ok(v.int(0))
            }
            "slow" => {
                cx.acc_range(f, None)?;
                cx.contribute(f, a[1]);
                Ok(a[1])
            }
            _ => {
                cx.contribute(f, a[1]);
                Ok(a[1])
            }
        }
    }
}

struct Threads(usize);

impl defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> defuse::RunResult + Sync),
        out: &mut Vec<defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<OnceLock<defuse::RunResult>> = (0..n).map(|_| OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        if i >= n {
                            break;
                        }
                        let _ = slots[i].set(run(i));
                    }
                });
            }
        });
        out.extend(slots.into_iter().map(|s| s.into_inner().expect("ran")));
    }
}

/// Removes node `x` at the end of the step `Pages` parked holding it.
struct RemoveHeld(Option<NodeId>, Arc<Ops>);

impl defuse::EditSource for RemoveHeld {
    fn absorb(&mut self, eng: &mut Engine) -> Option<u64> {
        if let Some(x) = self.0
            && self.1.held.load(Relaxed) == x.raw()
        {
            self.0 = None;
            eng.remove(x);
        }
        None
    }
}

#[test]
fn a_placer_whose_kept_reads_broke_holds_what_it_placed() {
    let mut held = 0;
    for sp in [defuse::Speculation::Off, defuse::Speculation::Predicted, defuse::Speculation::All] {
        for threads in [1, 4] {
            held += run(sp, threads);
        }
    }
    // (In some schedules `Pages` runs after the piece it would stop on.)
    assert!(held > 0, "`Pages` never stopped holding the node");
}

/// The edits in which `Pages` held the node when it went.
fn run(sp: defuse::Speculation, threads: usize) -> u32 {
    let mut held = 0;
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
        held: AtomicU64::new(0),
        stale: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let acc = vprof(eng.values());
    eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let p = vec![eng.values().str("pages")];
    let p = eng.push(u, op, p);
    let piece = |eng: &Engine, i: usize, h: i64| {
        let v = eng.values();
        vec![v.str(if i == K + 1 { "slow" } else { "piece" }), v.int(h)]
    };
    let mut hs: Vec<i64> = (0..N).map(|i| i64::try_from(i).unwrap() + 1).collect();
    let ids: Vec<NodeId> = (0..N)
        .map(|i| {
            let a = piece(&eng, i, hs[i]);
            eng.push(u, op, a)
        })
        .collect();
    let total = |eng: &Engine| eng.output(p).and_then(|x| eng.values().as_int(x));
    for (h1, h) in [(5i64, 20i64), (2, 3), (7, 9)] {
        let a = vec![eng.values().str("res")];
        let r = eng.insert_after(ids[K], op, a);
        eng.evaluate(&mut defuse::Greedy::speculative(sp, 64), &mut Threads(threads)).expect("evaluate");
        assert_eq!(total(&eng), Some(hs.iter().sum()));
        // An earlier piece and the one after it change; it goes once
        // `Pages` holds it.
        for (k, h) in [(1, h1), (K + 1, h)] {
            hs[k] = h;
            let a = piece(&eng, k, h);
            eng.replace(ids[k], op, a);
        }
        let mut src = RemoveHeld(Some(r), ops.clone());
        let rep = eng.evaluate_absorbing(&mut defuse::Greedy::speculative(sp, 64), &mut Threads(threads), &mut src);
        held += u32::from(src.0.is_none());
        assert!(rep.is_ok(), "{sp:?} j{threads}: {rep:?}");
        assert_eq!(ops.stale.load(Relaxed), 0, "{sp:?} j{threads} {h1} {h}: a finisher ran naming a node gone");
        assert_eq!(total(&eng), Some(hs.iter().sum()), "{sp:?} j{threads} {h1} {h}: {rep:?}");
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
    held
}
