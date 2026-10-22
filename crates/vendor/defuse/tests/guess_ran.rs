//! `Monoid::guess_ran`: a range read folds the contributors in work that
//! have a committed result as they last contributed, and waits for those
//! that never ran. Unit [s, w_0 .. w_N, r]: `s` defines `x` and `k`; each
//! `w_i` reads `x` and contributes `k`; `r` reads `x` and folds the
//! contributions. Cold, `r` waits for the `w`s (none ran). An edit of `x`
//! re-runs every `w` and `r`: guessing those that ran, `r` folds their
//! last contributions instead of waiting, and stands when they contribute
//! the same; an edit of `k` changes them, and `r` runs again on them. The
//! outputs are the sequential ones throughout.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Monoid, NodeId, Operation, Report, Stop, ValueId, Values};

const N: i64 = 12;

struct Sum(bool);

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap() + v.as_int(b).unwrap())
    }
    fn guess_ran(&self) -> bool {
        self.0
    }
}

/// (`true`: `s` predicts `x` 0, wrongly, and `k` 1.)
struct Ops(bool);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (self.0 && tag(v, a) == "s").then(|| vec![(v.field("x"), v.int(0)), (v.field("k"), v.int(1))])
    }
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "s" => Some(vec![(v.field("x"), Locality::Global), (v.field("k"), Locality::Global)]),
            "z" => Some(vec![(v.field("q"), Locality::Global)]),
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (x, k, c) = (v.field("x"), v.field("k"), v.field("c"));
        match tag(v, &a).as_str() {
            "z" => {
                cx.define_with(v.field("q"), v.int(1), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "s" => {
                cx.use_field(v.field("q"))?;
                cx.define_with(x, a[1], Locality::Global);
                cx.define_with(k, a[2], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "w" => {
                let _ = cx.use_field(x)?;
                let kv = cx.use_field(k)?.unwrap_or(v.int(0));
                cx.contribute(c, kv);
                Ok(kv)
            }
            _ => {
                let xv = cx.use_field(x)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                let sum = cx.acc_range(c, None)?;
                Ok(v.int(xv * 1000 + v.as_int(sum).unwrap()))
            }
        }
    }
}

fn build(guess: bool, mispredict: bool) -> (Engine, NodeId, NodeId, defuse::OpId) {
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("c"), Arc::new(Sum(guess)));
    let op = eng.register(Arc::new(Ops(mispredict)));
    let u = eng.new_unit();
    if mispredict {
        let z = vec![eng.values().str("z")];
        eng.push(u, op, z);
    }
    let s_args = vec![eng.values().str("s"), eng.values().int(1), eng.values().int(1)];
    let s = eng.push(u, op, s_args);
    for i in 0..N {
        let w = vec![eng.values().str("w"), eng.values().int(i)];
        eng.push(u, op, w);
    }
    let r_args = vec![eng.values().str("r")];
    let r = eng.push(u, op, r_args);
    (eng, s, r, op)
}

fn eval(eng: &mut Engine) -> Report {
    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential).expect("evaluate")
}

#[test]
fn a_reader_folds_rerunning_contributors_as_they_last_contributed() {
    let mut stops = Vec::new();
    let mut colds = Vec::new();
    for guess in [false, true] {
        let (mut eng, s, r, op) = build(guess, false);
        let out = |eng: &Engine| eng.output(r).and_then(|y| eng.values().as_int(y));
        let cold = eval(&mut eng);
        assert_eq!(out(&eng), Some(1000 + N), "guess {guess}: cold");
        colds.push((cold.stops, cold.runs));
        // `x` moves: every `w` and `r` run again; the contributions stand.
        let a = vec![eng.values().str("s"), eng.values().int(2), eng.values().int(1)];
        eng.replace(s, op, a);
        let rep = eval(&mut eng);
        assert_eq!(out(&eng), Some(2000 + N), "guess {guess}: x edited");
        stops.push(rep.stops);
        // `k` moves: the contributions change; `r` folds them again.
        let a = vec![eng.values().str("s"), eng.values().int(2), eng.values().int(3)];
        eng.replace(s, op, a);
        eval(&mut eng);
        assert_eq!(out(&eng), Some(2000 + 3 * N), "guess {guess}: k edited");
        assert!(eng.check_reads().is_empty(), "guess {guess}: {:?}", eng.check_reads());
        assert!(eng.check_invariants().is_empty(), "guess {guess}: {:?}", eng.check_invariants());
    }
    // (Cold, none ran before: as without guessing.)
    assert_eq!(colds[0], colds[1], "cold without and with guessing");
    // (Guessing those that ran, `r` did not stop on the re-running `w`s.)
    assert!(stops[1] < stops[0], "stops without and with guessing: {stops:?}");
}

/// Cold, the `w`s run on `s`'s prediction of `x` (wrong) while `s` waits
/// on `z` (placed first), and again on its definition, in work with a
/// result of this evaluation meanwhile: `r` waits for them as without
/// guessing (none ran in an earlier one).
#[test]
fn a_cold_build_folds_no_contributor_of_its_own_evaluation() {
    let mut colds = Vec::new();
    for guess in [false, true] {
        let (mut eng, _, r, _) = build(guess, true);
        let rep = eval(&mut eng);
        assert_eq!(eng.output(r).and_then(|y| eng.values().as_int(y)), Some(1000 + N), "guess {guess}");
        assert!(eng.check_reads().is_empty(), "guess {guess}: {:?}", eng.check_reads());
        colds.push((rep.stops, rep.runs, rep.mispredictions));
    }
    eprintln!("(stops, runs, mispredictions) without / with: {colds:?}");
    assert_eq!(colds[0], colds[1], "cold without and with guessing");
}
