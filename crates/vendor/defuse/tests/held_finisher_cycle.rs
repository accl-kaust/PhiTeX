//! A placer's stop does not leave it holding what it waits on: `Pages`
//! places a finisher at each page break (`Ctx::follow_after_any`: after
//! the piece whose height fills the page) defining the page number; a
//! piece places a resume after itself (`Ctx::follow`) that reads the page
//! number and the fold before it, and contributes. An edit to an earlier
//! piece's height moves the break before that piece: `Pages` (its fold),
//! the old finisher (its range) and the resume (its fold) run again. The
//! old finisher waits for `Pages` to place it again (a stale placer's
//! hold), the resume for the finisher (the page number), and `Pages`,
//! past the old finisher's piece without placing it, for the resume. The
//! old finisher goes at `Pages`'s stop: its attempt is past it (run as it
//! stands, it would run on its old arguments).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

const N: usize = 12;
/// A page's height.
const PAGE: i64 = 10;
/// The piece that places the resume.
const K: usize = 4;

struct Ops {
    me: OnceLock<OpId>,
    /// Finisher runs whose page its fold contradicts (on arguments from
    /// before).
    stale: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn vprof(v: &Values) -> Field {
    v.field("acc:vprof")
}

fn pg(v: &Values) -> Field {
    v.field("pg")
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        // (The finisher's undeclared: a reader past it waits while it is in
        // work.)
        (tag(v, a) != "fin").then(Vec::new)
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
                let (mut after, mut n, mut total) = (None, 0, 0);
                while let Some((x, h)) = cx.acc_next(f, after)? {
                    total += v.as_int(h).unwrap_or(0);
                    if total / PAGE > n {
                        n = total / PAGE;
                        cx.follow_after_any(x, op, vec![v.str("fin"), v.int(n)]);
                    }
                    after = Some(x);
                }
                Ok(v.int(n))
            }
            "fin" => {
                let prev = cx.prev_sibling_where(&|_, args| v.as_str(args[0]).as_deref() == Some("fin"));
                let h = cx.acc_range(f, prev)?;
                let all = cx.acc_range(f, None)?;
                if v.as_int(all).unwrap_or(0) / PAGE != v.as_int(a[1]).unwrap_or(0) {
                    self.stale.fetch_add(1, Relaxed);
                }
                cx.define_with(pg(v), v.list(vec![a[1], h]), Locality::Global);
                Ok(h)
            }
            "res" => {
                // (Nothing defines it: an undeclared finisher before it in
                // work may.)
                let p = cx.use_field(v.field("mark"))?.unwrap_or_else(|| v.int(-1));
                let t = cx.acc_range(f, None)?;
                cx.contribute(f, v.int(1));
                Ok(v.list(vec![p, t]))
            }
            "brk" => {
                cx.follow(op, vec![v.str("res")]);
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

#[test]
fn a_placer_stopped_past_what_it_holds_frees_it() {
    for sp in [defuse::Speculation::Off, defuse::Speculation::Predicted, defuse::Speculation::All] {
        run(sp);
    }
}

fn run(sp: defuse::Speculation) {
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
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
    // (Pieces of 2: the first page breaks after piece `K`.)
    let piece = |eng: &Engine, i: usize, h: i64| {
        let v = eng.values();
        vec![v.str(if i == K { "brk" } else { "piece" }), v.int(h)]
    };
    let ids: Vec<NodeId> = (0..N)
        .map(|i| {
            let a = piece(&eng, i, 2);
            eng.push(u, op, a)
        })
        .collect();
    let eval = |eng: &mut Engine| eng.evaluate(&mut defuse::Greedy::speculative(sp, 64), &mut Threads(4));
    eval(&mut eng).expect("evaluate");
    // (The pieces and the resume.)
    assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some((2 * N as i64 + 1) / PAGE));
    for (k, h) in [(K - 2, 4i64), (K - 2, 2), (1, 5), (1, 2), (K - 1, 1)] {
        let a = piece(&eng, k, h);
        eng.replace(ids[k], op, a);
        let r = eval(&mut eng);
        assert!(r.is_ok(), "piece {k} to {h}: {r:?}");
        assert_eq!(ops.stale.load(Relaxed), 0, "piece {k} to {h}: a finisher ran on its old arguments");
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
}
