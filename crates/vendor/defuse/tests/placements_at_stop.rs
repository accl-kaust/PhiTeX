//! A stopped run's placements are made at its stop, not only at its end:
//! a page builder `P` folds the pieces after it one at a time
//! (`Ctx::acc_next`), parks between attempts, and places a page's
//! finisher right after the piece the page fires at
//! (`Ctx::follow_after_any`) as soon as it fires. The pieces come in one
//! by one (each places the next, as expansion does). The finisher defines
//! `page`, which the pieces after it read: placed at P's stop, it is there
//! before most of them run, so few run twice (placed only at P's end,
//! every piece after the first page would). Every schedule gives the
//! sequential values, cold and after an edit.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

const N: usize = 30;
/// A page fires after every `PER` pieces.
const PER: usize = 3;

struct Ops {
    me: OnceLock<OpId>,
    piece_runs: AtomicU64,
    /// Each piece's last answer (its committed one at the end).
    seen: std::sync::Mutex<std::collections::BTreeMap<i64, i64>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn fields(v: &Values) -> (Field, Field) {
    (v.field("acc:vprof"), v.field("page"))
}

/// P's state between attempts: the last piece folded and the pages made.
struct Kept {
    after: Option<NodeId>,
    folded: usize,
    pages: Vec<(NodeId, i64)>,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "fin" { vec![(fields(v).1, Locality::Global)] } else { Vec::new() })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        // (Pieces contribute their height; nothing else does.)
        (tag(v, a) != "piece").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (vprof, page) = fields(v);
        let op = *self.me.get().unwrap();
        match tag(v, &a).as_str() {
            "pages" => {
                let mut k = cx
                    .parked()
                    .and_then(|t| t.downcast::<Kept>().ok())
                    .map_or(Kept { after: None, folded: 0, pages: Vec::new() }, |b| *b);
                // (The pages made by earlier attempts, placed again.)
                for &(at, n) in &k.pages {
                    cx.follow_after_any(at, op, vec![v.str("fin"), v.int(n)]);
                }
                loop {
                    // (Parked before each fold: a stop resumes from here.)
                    cx.park(Box::new(Kept {
                        after: k.after,
                        folded: k.folded,
                        pages: k.pages.clone(),
                    }));
                    let Some((x, _)) = cx.acc_next(vprof, k.after)? else { break };
                    k.after = Some(x);
                    k.folded += 1;
                    if k.folded % PER == 0 {
                        let n = i64::try_from(k.folded / PER).unwrap();
                        cx.follow_after_any(x, op, vec![v.str("fin"), v.int(n)]);
                        k.pages.push((x, n));
                    }
                }
                Ok(v.int(i64::try_from(k.pages.len()).unwrap()))
            }
            "fin" => {
                cx.define(page, a[1]);
                Ok(a[1])
            }
            _ => {
                self.piece_runs.fetch_add(1, Relaxed);
                let i = v.as_int(a[2]).unwrap();
                let seen = cx.use_field(page)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.contribute(vprof, a[1]);
                if i + 1 < i64::try_from(N).unwrap() {
                    cx.follow(op, vec![v.str("piece"), v.int(1), v.int(i + 1)]);
                }
                self.seen.lock().unwrap().insert(i, seen);
                Ok(v.int(seen))
            }
        }
    }
}

fn reference() -> Vec<i64> {
    // Piece i (0-based) sees the pages fired at or before the piece
    // before it.
    (0..N).map(|i| i64::try_from(i / PER).unwrap()).collect()
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

#[test]
fn a_page_builder_parked_between_attempts_places_its_finishers_as_pages_fire() {
    for threads in [1usize, 4] {
        let ops = Arc::new(Ops {
            me: OnceLock::new(),
            piece_runs: AtomicU64::new(0),
            seen: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        });
        let mut eng = Engine::new();
        let acc = fields(eng.values()).0;
        eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let p = vec![eng.values().str("pages")];
        let p = eng.push(u, op, p);
        let a0 = {
            let v = eng.values();
            vec![v.str("piece"), v.int(1), v.int(0)]
        };
        let a0 = eng.push(u, op, a0);
        let seen = |ops: &Ops| ops.seen.lock().unwrap().values().copied().collect::<Vec<i64>>();
        // (Not speculating: P stops on each piece in work.)
        let eval = |eng: &mut Engine| {
            if threads == 1 {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Off, 0), &mut defuse::Sequential)
            } else {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Off, 0), &mut Threads(threads))
            }
            .expect("evaluate")
        };
        let r = eval(&mut eng);
        let v = eng.values();
        assert_eq!(seen(&ops), reference(), "j{threads}: {r:?}");
        assert_eq!(eng.output(p).and_then(|x| v.as_int(x)), Some(i64::try_from(N / PER).unwrap()));
        // (Per page, at most the piece running beside its firing runs
        // again, once stopping on the new finisher: the finisher is there
        // before the rest. Placed at P's end, every piece after the first
        // page would run again.)
        let runs = ops.piece_runs.load(Relaxed);
        assert!(runs <= (N + 2 * (N / PER)) as u64, "j{threads}: {runs} piece runs: {r:?}");
        // A piece taller: the pages stay (heights do not move breaks
        // here), nothing is lost or doubled.
        let a = {
            let v = eng.values();
            vec![v.str("piece"), v.int(2), v.int(0)]
        };
        eng.replace(a0, op, a);
        let r = eval(&mut eng);
        assert_eq!(seen(&ops), reference(), "j{threads} edit: {r:?}");
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
