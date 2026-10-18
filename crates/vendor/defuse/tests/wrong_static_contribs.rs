//! A wrong static contribution list costs a re-run, never a wrong result.
//! Execs `e` declare that they contribute nothing (`Some(vec![])`), so a
//! range reader passes them while they are in work; their runs contribute
//! to `v` all the same, after some work. Readers `p` fold `v` from the
//! unit start. Every schedule must end where a sequential build does,
//! with every recorded read holding.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, Operation, Stop, ValueId, Values};

struct Sum;

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0) + v.as_int(b).unwrap_or(0))
    }
}

struct Ops {
    ids: std::sync::RwLock<Vec<NodeId>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "p").then(|| vec![v.field("v")])
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "e").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "e" {
            let mut x = 0u64;
            for i in 0..30_000u64 {
                x = x.wrapping_mul(31).wrapping_add(i);
            }
            cx.contribute(v.field("v"), v.int(1 + i64::from(x == 7)));
            return Ok(ValueId::UNIT);
        }
        // Windows: from the start, after the node 7 before, and that
        // window ending 2 before this node.
        let i = usize::try_from(v.as_int(a[1]).unwrap()).unwrap();
        let ids = self.ids.read().unwrap().clone();
        match i % 3 {
            0 => cx.acc_range(v.field("v"), None),
            1 => cx.acc_range(v.field("v"), Some(ids[i.saturating_sub(7)])),
            _ => cx.acc_range_to(v.field("v"), Some(ids[i.saturating_sub(7)]), Some(ids[i - 2])),
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
        let slots: Vec<std::sync::OnceLock<defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0 {
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

fn build() -> (Engine, Vec<NodeId>) {
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("v"), Arc::new(Sum));
    let ops = Arc::new(Ops {
        ids: std::sync::RwLock::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let u = eng.new_unit();
    let mut ps = Vec::new();
    for i in 0..60 {
        let t = if i % 3 == 2 { "p" } else { "e" };
        let a = vec![eng.values().str(t), eng.values().int(i)];
        let n = eng.push(u, op, a);
        ops.ids.write().unwrap().push(n);
        if t == "p" {
            ps.push(n);
        }
    }
    (eng, ps)
}

fn outs(eng: &Engine, ps: &[NodeId]) -> Vec<Option<i64>> {
    ps.iter()
        .map(|&n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn a_wrong_empty_declaration_is_refuted_not_believed() {
    let (mut seq, sps) = build();
    seq.evaluate_sequential().expect("sequential");
    let want = outs(&seq, &sps);
    for i in 0..40 {
        let (mut eng, ps) = build();
        let r = match i % 3 {
            0 => eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(4)),
            1 => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256),
                &mut Threads(8),
            ),
            _ => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
                &mut Threads(3),
            ),
        };
        r.expect("evaluate");
        assert_eq!(outs(&eng, &ps), want, "schedule {i}");
        assert!(eng.check_reads().is_empty(), "schedule {i}: {:?}", eng.check_reads());
    }
}

/// The same with the reader shapes a client uses: readers fold `v` first,
/// then wait for a slow definer `s` of `d`, parking with their fold kept
/// (resumed, not re-run: the fold is a read of the kept attempt); half of
/// them are placed by a creator `c` with `follow_after` an earlier exec.
struct Parking {
    me: std::sync::OnceLock<defuse::OpId>,
    ids: std::sync::RwLock<Vec<NodeId>>,
}

impl Operation for Parking {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "s" {
            vec![(v.field("d"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) == "e").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let spin = |n: u64| {
            let mut x = 0u64;
            for i in 0..n {
                x = x.wrapping_mul(31).wrapping_add(i);
            }
            x
        };
        match tag(v, &a).as_str() {
            "e" => {
                let x = spin(30_000);
                cx.contribute(v.field("v"), v.int(1 + i64::from(x == 7)));
                Ok(ValueId::UNIT)
            }
            "s" => {
                let x = spin(400_000);
                cx.define(v.field("d"), v.int(100 + i64::from(x == 7)));
                Ok(ValueId::UNIT)
            }
            "c" => {
                let ids = self.ids.read().unwrap().clone();
                for k in [5usize, 11, 20, 31, 44] {
                    let at = ids[k];
                    cx.follow_after(at, *self.me.get().unwrap(), vec![v.str("p"), v.int(-1)]);
                }
                Ok(ValueId::UNIT)
            }
            _ => {
                let fold = match cx.parked() {
                    Some(t) => *t.downcast::<ValueId>().expect("fold"),
                    None => {
                        let f = cx.acc_range(v.field("v"), None)?;
                        cx.park(Box::new(f));
                        f
                    }
                };
                let d = cx.use_field(v.field("d"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                Ok(v.int(v.as_int(fold).unwrap_or(0) * 1000 + d))
            }
        }
    }
}

fn build_parking() -> (Engine, Vec<NodeId>, NodeId) {
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("v"), Arc::new(Sum));
    let ops = Arc::new(Parking {
        me: std::sync::OnceLock::new(),
        ids: std::sync::RwLock::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let mut ps = Vec::new();
    let s = vec![eng.values().str("s")];
    ops.ids.write().unwrap().push(eng.push(u, op, s));
    for i in 1..60 {
        let t = match i % 4 {
            3 => "p",
            _ => "e",
        };
        let a = vec![eng.values().str(t), eng.values().int(i)];
        let n = eng.push(u, op, a);
        ops.ids.write().unwrap().push(n);
        if t == "p" {
            ps.push(n);
        }
    }
    let c = vec![eng.values().str("c")];
    let cn = eng.push(u, op, c);
    (eng, ps, cn)
}

/// Every reader's output: the pushed ones, then those `c` placed.
fn all_readers(eng: &Engine, ps: &[NodeId], c: NodeId) -> Vec<Option<i64>> {
    ps.iter()
        .copied()
        .chain(eng.children(c))
        .map(|n| eng.output(n).and_then(|x| eng.values().as_int(x)))
        .collect()
}

#[test]
fn parked_and_placed_readers_see_undeclared_contributions() {
    let (mut seq, sps, sc) = build_parking();
    seq.evaluate_sequential().expect("sequential");
    let want = all_readers(&seq, &sps, sc);
    assert_eq!(want.len(), sps.len() + 5);
    let mut stops = 0;
    for i in 0..60 {
        let (mut eng, ps, c) = build_parking();
        let r = match i % 3 {
            0 => eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(4)),
            1 => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256),
                &mut Threads(8),
            ),
            _ => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
                &mut Threads(3),
            ),
        };
        stops += r.expect("evaluate").stops;
        assert_eq!(all_readers(&eng, &ps, c), want, "schedule {i}");
        assert!(eng.check_reads().is_empty(), "schedule {i}: {:?}", eng.check_reads());
    }
    // (The readers did park on the way: the kept folds were resumed.)
    assert!(stops > 0, "no reader parked: the test proves nothing");
}

/// Phi's shape: page readers fold `v` over a window anchored at an earlier
/// exec while pieces still run; a creator places, with `follow_after`,
/// finishers inside those windows that contribute `v` from their runs
/// (declaring nothing), many after each anchor (relabelling the text).
struct Placed {
    me: std::sync::OnceLock<defuse::OpId>,
    ids: std::sync::RwLock<Vec<NodeId>>,
}

impl Operation for Placed {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "p").then(|| vec![v.field("v")])
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        matches!(tag(v, a).as_str(), "e" | "f").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let spin = |n: u64| {
            let mut x = 0u64;
            for i in 0..n {
                x = x.wrapping_mul(31).wrapping_add(i);
            }
            x
        };
        let i = v.as_int(a[1]).unwrap();
        match tag(v, &a).as_str() {
            "e" | "f" => {
                let x = spin(20_000 + 3_000 * u64::try_from(i.rem_euclid(7)).unwrap());
                cx.contribute(v.field("v"), v.int(1 + i.rem_euclid(5) + i64::from(x == 7)));
                Ok(ValueId::UNIT)
            }
            "c" => {
                let ids = self.ids.read().unwrap().clone();
                for (k, &at) in ids.iter().enumerate() {
                    if k % 3 == 0 {
                        for j in 0..4 {
                            let n = i64::try_from(k * 10 + j).unwrap();
                            cx.follow_after(at, *self.me.get().unwrap(), vec![v.str("f"), v.int(n)]);
                        }
                    }
                }
                Ok(ValueId::UNIT)
            }
            _ => {
                let ids = self.ids.read().unwrap().clone();
                let k = usize::try_from(i).unwrap();
                let from = ids[k.saturating_sub(9)];
                cx.acc_range(v.field("v"), Some(from))
            }
        }
    }
}

fn build_placed() -> (Engine, Vec<NodeId>, NodeId) {
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("v"), Arc::new(Sum));
    let ops = Arc::new(Placed {
        me: std::sync::OnceLock::new(),
        ids: std::sync::RwLock::new(Vec::new()),
    });
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    // The creator first: its placements land while the pieces run.
    let c = vec![eng.values().str("c"), eng.values().int(0)];
    let cn = eng.push(u, op, c);
    let mut ps = Vec::new();
    for i in 0..60i64 {
        let t = if i % 4 == 3 { "p" } else { "e" };
        let a = vec![eng.values().str(t), eng.values().int(i)];
        let n = eng.push(u, op, a);
        ops.ids.write().unwrap().push(n);
        if t == "p" {
            ps.push(n);
        }
    }
    (eng, ps, cn)
}

#[test]
fn readers_of_anchored_windows_see_placed_contributors() {
    let (mut seq, sps, _) = build_placed();
    seq.evaluate_sequential().expect("sequential");
    let want = outs(&seq, &sps);
    for i in 0..90 {
        let (mut eng, ps, _) = build_placed();
        let r = match i % 3 {
            0 => eng.evaluate(&mut defuse::Greedy::default(), &mut Threads(8)),
            1 => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256),
                &mut Threads(8),
            ),
            _ => eng.evaluate(
                &mut defuse::Greedy::speculative(defuse::Speculation::All, 256),
                &mut Threads(4),
            ),
        };
        r.expect("evaluate");
        assert_eq!(outs(&eng, &ps), want, "schedule {i}");
        assert!(eng.check_reads().is_empty(), "schedule {i}: {:?}", eng.check_reads());
    }
}
