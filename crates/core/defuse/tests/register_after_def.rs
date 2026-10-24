//! A register kept as definitions and contributions (PhiTeX's counters:
//! a write inside a scope stays a definition, one outside every scope is
//! a contribution): each piece reads the register as the definition
//! reaching it plus the contributions after that definition's node
//! (`Ctx::acc_range_after_def`), then sets it (a definition) or adds to it
//! (a contribution). Every schedule gives the sequential values, cold and
//! after edits that turn an adder into a setter and back, with the fold
//! guessed (`Monoid::speculate`) or not.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, Operation, Stop, ValueId, Values};

const N: usize = 300;

struct Sum {
    guess: bool,
}

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0).wrapping_add(v.as_int(b).unwrap_or(0)))
    }
    fn speculate(&self) -> bool {
        self.guess
    }
}

/// Piece `i`'s write: `Some(Ok(x))` sets the register to `x`, `Some(Err(c))`
/// adds `c`; `flip` turns piece `flip`'s kind over.
fn write(i: usize, flip: Option<usize>) -> Option<Result<i64, i64>> {
    let k = (i * 5) % 13;
    let set = (k == 0) != (Some(i) == flip);
    let x = i64::try_from(i).unwrap();
    if set {
        Some(Ok(x * 10))
    } else if k <= 6 {
        Some(Err(x % 7 + 1))
    } else {
        None
    }
}

fn fields(v: &Values) -> (Field, Field) {
    (v.field("reg"), v.field("acc:reg"))
}

struct Ops;

fn piece(v: &Values, a: &[ValueId]) -> (usize, Option<usize>) {
    let i = usize::try_from(v.as_int(a[0]).unwrap()).unwrap();
    let flip = v.as_int(a[1]).and_then(|f| usize::try_from(f).ok());
    (i, flip)
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let (i, flip) = piece(v, a);
        Some(match write(i, flip) {
            Some(Ok(_)) => vec![(fields(v).0, Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (i, flip) = piece(v, &a);
        let (reg, acc) = fields(v);
        let (base, fold) = cx.acc_range_after_def(acc, reg)?;
        let seen = base.and_then(|b| v.as_int(b)).unwrap_or(0).wrapping_add(v.as_int(fold).unwrap_or(0));
        match write(i, flip) {
            Some(Ok(x)) => cx.define(reg, v.int(x)),
            Some(Err(c)) => cx.contribute(acc, v.int(c)),
            None => {}
        }
        Ok(v.int(seen))
    }
}

/// The register each piece sees, sequentially.
fn reference(flip: Option<usize>) -> Vec<i64> {
    let mut r = 0i64;
    let mut out = Vec::new();
    for i in 0..N {
        out.push(r);
        match write(i, flip) {
            Some(Ok(x)) => r = x,
            Some(Err(c)) => r = r.wrapping_add(c),
            None => {}
        }
    }
    out
}

fn args(eng: &Engine, i: usize, flip: Option<usize>) -> Vec<ValueId> {
    let v = eng.values();
    vec![v.int(i64::try_from(i).unwrap()), flip.map_or(ValueId::NIL, |f| v.int(i64::try_from(f).unwrap()))]
}

struct Threads(usize);

impl phitex_defuse::Executor for Threads {
    fn parallelism(&self) -> usize {
        self.0
    }
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> phitex_defuse::RunResult + Sync),
        out: &mut Vec<phitex_defuse::RunResult>,
    ) {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::OnceLock<phitex_defuse::RunResult>> = (0..n).map(|_| std::sync::OnceLock::new()).collect();
        std::thread::scope(|s| {
            for _ in 0..self.0.min(n) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

fn eval(eng: &mut Engine, threads: usize) -> phitex_defuse::Report {
    if threads == 1 {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256), &mut Threads(threads))
    }
    .expect("evaluate")
}

fn seen(eng: &Engine, ids: &[NodeId]) -> Vec<i64> {
    ids.iter().map(|&n| eng.values().as_int(eng.output(n).expect("output")).unwrap()).collect()
}

#[test]
fn a_register_is_its_reaching_definition_plus_the_contributions_after_it() {
    for guess in [false, true] {
        for threads in [1usize, 4, 8] {
            let mut eng = Engine::new();
            let acc = fields(eng.values()).1;
            eng.declare_accumulator(acc, Arc::new(Sum { guess }));
            let op = eng.register(Arc::new(Ops));
            let u = eng.new_unit();
            let ids: Vec<NodeId> = (0..N)
                .map(|i| {
                    let a = args(&eng, i, None);
                    eng.push(u, op, a)
                })
                .collect();
            eval(&mut eng, threads);
            assert_eq!(seen(&eng, &ids), reference(None), "guess {guess}, j{threads}: cold");
            // An adder becomes a setter, then a setter an adder.
            for flip in [N / 3 + 1, 0, 2 * N / 3] {
                let set_before = matches!(write(flip, None), Some(Ok(_)));
                eng.replace(ids[flip], op, args(&eng, flip, Some(flip)));
                let r = eval(&mut eng, threads);
                assert_eq!(seen(&eng, &ids), reference(Some(flip)), "guess {guess}, j{threads}: flip {flip} (set {set_before}): {r:?}");
                eng.replace(ids[flip], op, args(&eng, flip, None));
                eval(&mut eng, threads);
                assert_eq!(seen(&eng, &ids), reference(None), "guess {guess}, j{threads}: flip {flip} back");
            }
            assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        }
    }
}
