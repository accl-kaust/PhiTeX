//! A run resumed from its kept token continues its placements
//! (`Ctx::park_continuing`): `Pages` folds the pieces with
//! `Ctx::acc_next` and places a finisher after each, keeping its cursor
//! and count in its token; resumed, it places only the finishers after
//! pieces it had not reached. A parent places a child per attempt and
//! sums all its children's outputs (`Ctx::output_of`, the indices going
//! on from the earlier attempts'). A finisher places a child and waits for
//! it with `Ctx::park`, making it again each attempt, as before. All match
//! the contract where every attempt places everything again, cold and
//! after edits.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

const N: usize = 12;
const SLOW: usize = 6;

struct Ops {
    me: OnceLock<OpId>,
    /// The continuing contract: resumed, place only what is new.
    cont: AtomicBool,
    /// Attempts that continued.
    continued: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn vprof(v: &Values) -> Field {
    v.field("acc:vprof")
}

/// `Pages`'s kept state: the cursor, the pieces passed.
struct Tok(Option<NodeId>, Vec<NodeId>);

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        matches!(tag(v, a).as_str(), "pages" | "fin" | "parent" | "kid" | "leaf").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        let f = vprof(v);
        let cont = self.cont.load(Relaxed);
        match tag(v, &a).as_str() {
            "pages" => {
                let tok = cx.parked().and_then(|t| t.downcast::<Tok>().ok());
                let resumed = tok.is_some();
                let Tok(mut after, mut passed) = tok.map_or(Tok(None, Vec::new()), |t| *t);
                if cont && resumed {
                    self.continued.fetch_add(1, Relaxed);
                } else {
                    for (i, &x) in passed.iter().enumerate() {
                        cx.follow_after_any(x, op, vec![v.str("fin"), v.int(i64::try_from(i).unwrap())]);
                    }
                }
                loop {
                    match cx.acc_next(f, after) {
                        Ok(Some((x, _))) => {
                            cx.follow_after_any(x, op, vec![v.str("fin"), v.int(i64::try_from(passed.len()).unwrap())]);
                            passed.push(x);
                            after = Some(x);
                        }
                        Ok(None) => break,
                        Err(s) => {
                            if cont {
                                cx.park_continuing(Box::new(Tok(after, passed)));
                            } else {
                                cx.park(Box::new(Tok(after, passed)));
                            }
                            return Err(s);
                        }
                    }
                }
                Ok(v.int(i64::try_from(passed.len()).unwrap()))
            }
            "fin" => {
                let prev = cx.prev_sibling_where(&|_, args| v.as_str(args[0]).as_deref() == Some("fin"));
                let h = cx.acc_range(f, prev)?;
                // (Made again each attempt: a plain park.)
                cx.child(op, vec![v.str("leaf"), h]);
                match cx.output_of(0) {
                    Ok(x) => Ok(v.int(v.as_int(h).unwrap_or(0) * 100 + v.as_int(x).unwrap_or(0))),
                    Err(s) => {
                        cx.park(Box::new(()));
                        Err(s)
                    }
                }
            }
            "leaf" => Ok(v.int(v.as_int(a[1]).unwrap_or(0) % 7)),
            "parent" => {
                // Children 0..n, each waited for (`k`: those placed so far).
                let k = cx.parked().and_then(|t| t.downcast::<usize>().ok()).map(|b| *b);
                let resumed = k.is_some();
                let k = k.unwrap_or(0);
                let n = usize::try_from(v.as_int(a[1]).unwrap_or(0)).unwrap_or(0);
                let mut sum = 0;
                for i in 0..n {
                    if i >= k || !(cont && resumed) {
                        let j = cx.child(op, vec![v.str("kid"), v.int(i64::try_from(i).unwrap()), a[2]]);
                        assert_eq!(j, i, "a child's index goes on from the earlier attempts'");
                    }
                    match cx.output_of(i) {
                        Ok(x) => sum += v.as_int(x).unwrap_or(0),
                        Err(s) => {
                            if cont {
                                cx.park_continuing(Box::new(i + 1));
                            } else {
                                cx.park(Box::new(i + 1));
                            }
                            return Err(s);
                        }
                    }
                }
                Ok(v.int(sum))
            }
            "kid" => Ok(v.int(v.as_int(a[1]).unwrap_or(0) * v.as_int(a[2]).unwrap_or(1))),
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

/// The outputs of every node (by text order) after each edit.
fn trace(cont: bool, sp: defuse::Speculation, threads: usize) -> Vec<Vec<Option<i64>>> {
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
        cont: AtomicBool::new(cont),
        continued: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let acc = vprof(eng.values());
    eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let pa = {
        let v = eng.values();
        vec![v.str("parent"), v.int(5), v.int(1)]
    };
    let pa = eng.push(u, op, pa);
    let p = vec![eng.values().str("pages")];
    eng.push(u, op, p);
    // (Piece `SLOW` folds the pieces before it: an edit before it leaves
    // it in work when `Pages` runs, which stops on it and resumes.)
    let piece = |eng: &Engine, i: usize, h: i64| {
        let v = eng.values();
        vec![v.str(if i == SLOW { "slow" } else { "piece" }), v.int(h)]
    };
    let ids: Vec<NodeId> = (0..N)
        .map(|i| {
            let a = piece(&eng, i, i64::try_from(i).unwrap() + 1);
            eng.push(u, op, a)
        })
        .collect();
    let mut out = Vec::new();
    let mut snap = |eng: &Engine| {
        let mut o: Vec<Option<i64>> = eng.order(u).into_iter().map(|x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect();
        o.push(i64::try_from(eng.order(u).len()).ok());
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
        out.push(o);
    };
    let eval = |eng: &mut Engine| eng.evaluate(&mut defuse::Greedy::speculative(sp, 64), &mut Threads(threads)).expect("evaluate");
    eval(&mut eng);
    snap(&eng);
    for (k, h) in [(3usize, 30i64), (0, 7), (N - 1, 2)] {
        let a = piece(&eng, k, h);
        eng.replace(ids[k], op, a);
        let a = {
            let v = eng.values();
            vec![v.str("parent"), v.int(i64::try_from(k % 4).unwrap() + 3), v.int(h)]
        };
        eng.replace(pa, op, a);
        eval(&mut eng);
        snap(&eng);
    }
    assert!(!cont || ops.continued.load(Relaxed) > 0, "no attempt continued");
    out
}

#[test]
fn a_continued_run_places_what_an_attempt_placing_everything_places() {
    for sp in [defuse::Speculation::Off, defuse::Speculation::Predicted, defuse::Speculation::All] {
        for threads in [1, 4] {
            assert_eq!(trace(true, sp, threads), trace(false, sp, threads), "{sp:?} j{threads}");
        }
    }
}
