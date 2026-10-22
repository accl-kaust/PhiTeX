//! `Engine::guess_from_last`: a reader that would wait on a definer
//! declared, not yet made, takes what its result from a previous
//! evaluation read there. Unit [s, a, r_0 .. r_N]: `s` defines `x`; `a`
//! reads `x`, defines `z` and places `d(x)` after itself, which defines
//! `y = x / 10`; each `r_i` reads `z`, then `y`. An edit of `x` re-runs `a`,
//! which re-makes `d` (declared, not made) and changes `z`: the `r`s run
//! again before `d` has. Guessing `y` from the last, they need not wait, and
//! stand when `d` makes the same `y` (`x` 1 -> 2); when it makes another
//! (`x` 2 -> 15), they run again on it. A cold build is as without it. The
//! outputs are the sequential ones throughout.

use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

const N: i64 = 12;

struct Ops(OnceLock<OpId>);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![(v.field("x"), Locality::Global)],
            "a" => vec![(v.field("z"), Locality::Global)],
            "d" => vec![(v.field("y"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.0.get().unwrap();
        let (x, y, z) = (v.field("x"), v.field("y"), v.field("z"));
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(x, a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "a" => {
                let xv = cx.use_field(x)?.unwrap_or(v.int(0));
                cx.define_with(z, xv, Locality::Global);
                cx.follow(op, vec![v.str("d"), xv]);
                Ok(ValueId::UNIT)
            }
            "d" => {
                cx.define_with(y, v.int(v.as_int(a[1]).unwrap() / 10), Locality::Global);
                Ok(ValueId::UNIT)
            }
            _ => {
                let zv = cx.use_field(z)?.and_then(|q| v.as_int(q)).unwrap_or(0);
                let yv = cx.use_field(y)?.and_then(|q| v.as_int(q)).unwrap_or(-1);
                Ok(v.int(zv * 1000 + yv))
            }
        }
    }
}

fn build(guess: bool) -> (Engine, NodeId, Vec<NodeId>, OpId) {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops(OnceLock::new()));
    let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    ops.0.set(op).expect("once");
    if guess {
        let y = eng.values().field("y");
        eng.guess_from_last(y);
    }
    let u = eng.new_unit();
    let s_args = vec![eng.values().str("s"), eng.values().int(1)];
    let s = eng.push(u, op, s_args);
    let a_args = vec![eng.values().str("a")];
    eng.push(u, op, a_args);
    let rs = (0..N)
        .map(|i| {
            let r = vec![eng.values().str("r"), eng.values().int(i)];
            eng.push(u, op, r)
        })
        .collect();
    (eng, s, rs, op)
}

fn eval(eng: &mut Engine) -> Report {
    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential).expect("evaluate")
}

#[test]
fn a_reader_guesses_a_remade_definer_from_its_last_evaluation() {
    let mut reps: Vec<(u64, u64, u64)> = Vec::new();
    for guess in [false, true] {
        let (mut eng, s, rs, op) = build(guess);
        let outs = |eng: &Engine| rs.iter().map(|&r| eng.output(r).and_then(|q| eng.values().as_int(q))).collect::<Vec<_>>();
        let cold = eval(&mut eng);
        assert_eq!(outs(&eng), vec![Some(1000); N as usize], "guess {guess}: cold");
        reps.push((cold.stops, cold.nodes_run, cold.predictions_confirmed + cold.predictions_refuted));
        // `x` moves, `y` stays.
        let a = vec![eng.values().str("s"), eng.values().int(2)];
        eng.replace(s, op, a);
        let same = eval(&mut eng);
        assert_eq!(outs(&eng), vec![Some(2000); N as usize], "guess {guess}: x 2");
        reps.push((same.stops, same.nodes_run, same.predictions_confirmed));
        // `y` moves too: the guesses are refuted.
        let a = vec![eng.values().str("s"), eng.values().int(15)];
        eng.replace(s, op, a);
        let moved = eval(&mut eng);
        assert_eq!(outs(&eng), vec![Some(15001); N as usize], "guess {guess}: x 15");
        reps.push((moved.stops, moved.nodes_run, moved.predictions_refuted));
        assert!(eng.check_reads().is_empty(), "guess {guess}: {:?}", eng.check_reads());
        assert!(eng.check_invariants().is_empty(), "guess {guess}: {:?}", eng.check_invariants());
    }
    eprintln!("(stops, runs, predictions) without / with: {reps:?}");
    // (Cold, nothing is guessed: no result is from an earlier evaluation.)
    assert_eq!(reps[0], reps[3], "cold");
    // (Guessing, the readers did not stop on the re-made definer.)
    assert!(reps[4].0 < reps[1].0 && reps[4].2 > 0, "x 2: {reps:?}");
    assert!(reps[5].2 > 0, "x 15: {reps:?}");
}

/// `Pages`'s kept state: the cursor, the pieces passed, the sum of the
/// `y`s read.
struct Tok(Option<NodeId>, i64, i64);

/// A placer asked again after it committed: unit [s, P, q_0 .. q_K];
/// each `q` reads `x` and contributes to `vprof`. `P` folds the `q`s with
/// `acc_next`, reads `y` at each (defined by the finisher it placed after
/// the one before), places a finisher `fin(k, x, y)` after it, and at last
/// reads `t` past the last `q`. Finisher `k` reads `w` (the one before's
/// `k - 1`: they run one a step) and defines `y + 1` and `w = k`; the
/// first also `t = x`. An edit of `x` re-makes every finisher (`x` in its
/// text); `P` commits on the first's last `t`, and its commit asks `P`
/// again: `P` runs again with its result of this evaluation, the later
/// finishers still to run, and still guesses `y` from its last evaluation
/// (`Res::last`, carried through its commits).
struct Pages(OnceLock<OpId>, std::sync::atomic::AtomicU64);

const K: i64 = 10;

impl Operation for Pages {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![(v.field("x"), Locality::Global)],
            "fin" if v.as_int(a[1]) == Some(0) => vec![(v.field("y"), Locality::Global), (v.field("w"), Locality::Global), (v.field("t"), Locality::Global)],
            "fin" => vec![(v.field("y"), Locality::Global), (v.field("w"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        matches!(tag(v, a).as_str(), "s" | "pages" | "fin").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.0.get().unwrap();
        let (x, y, t, w, vp) = (v.field("x"), v.field("y"), v.field("t"), v.field("w"), v.field("acc:vprof"));
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(x, a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "pages" => {
                let Tok(mut after, mut k, mut sum) = cx.parked().and_then(|t| t.downcast::<Tok>().ok()).map_or(Tok(None, 0, 0), |t| *t);
                let xv = match cx.use_field(x) {
                    Ok(q) => q.unwrap_or(v.int(0)),
                    Err(s) => {
                        cx.park_continuing(Box::new(Tok(after, k, sum)));
                        return Err(s);
                    }
                };
                loop {
                    let q = match cx.acc_next(vp, after) {
                        Ok(Some((q, _))) => q,
                        Ok(None) => break,
                        Err(s) => {
                            cx.park_continuing(Box::new(Tok(after, k, sum)));
                            return Err(s);
                        }
                    };
                    let yv = match cx.use_field_at(y, Some(q)) {
                        Ok(defuse::FieldAt::Def(r, _)) => v.as_int(r).unwrap_or(0),
                        Ok(_) => 0,
                        Err(s) => {
                            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            cx.park_continuing(Box::new(Tok(after, k, sum)));
                            return Err(s);
                        }
                    };
                    cx.follow_after_any(q, op, vec![v.str("fin"), v.int(k), xv, v.int(yv)]);
                    (after, k, sum) = (Some(q), k + 1, sum + yv);
                }
                let tv = match cx.use_field_at(t, after) {
                    Ok(defuse::FieldAt::Def(r, _)) => v.as_int(r).unwrap_or(0),
                    Ok(_) => 0,
                    Err(s) => {
                        cx.park_continuing(Box::new(Tok(after, k, sum)));
                        return Err(s);
                    }
                };
                Ok(v.int(tv * 1000 + sum))
            }
            "fin" => {
                let k = v.as_int(a[1]).unwrap();
                if k > 0 {
                    cx.use_field(w)?;
                } else {
                    cx.define_with(t, a[2], Locality::Global);
                }
                // (Finisher 3 moves `y` when `x` is 3: one guess refuted.)
                let bump = if k == 3 && v.as_int(a[2]) == Some(3) { 50 } else { 0 };
                cx.define_with(y, v.int(k + 1 + bump), Locality::Global);
                cx.define_with(w, v.int(k), Locality::Global);
                Ok(ValueId::UNIT)
            }
            _ => {
                let xv = cx.use_field(x)?.unwrap_or(v.int(0));
                cx.contribute(vp, v.int(1));
                Ok(xv)
            }
        }
    }
}

#[test]
fn a_placer_asked_again_guesses_from_its_last_evaluation() {
    let mut reps: Vec<(u64, u64)> = Vec::new();
    for guess in [false, true] {
        let mut eng = Engine::new();
        let ops = Arc::new(Pages(OnceLock::new(), std::sync::atomic::AtomicU64::new(0)));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        ops.0.set(op).expect("once");
        let vp = eng.values().field("acc:vprof");
        eng.declare_accumulator(vp, Arc::new(defuse::SumMonoid));
        if guess {
            let y = eng.values().field("y");
            eng.guess_from_last(y);
        }
        let u = eng.new_unit();
        let s_args = vec![eng.values().str("s"), eng.values().int(1)];
        let s = eng.push(u, op, s_args);
        let p_args = vec![eng.values().str("pages")];
        let p = eng.push(u, op, p_args);
        for i in 0..K {
            let q = vec![eng.values().str("q"), eng.values().int(i)];
            eng.push(u, op, q);
        }
        let out = |eng: &Engine| eng.output(p).and_then(|q| eng.values().as_int(q));
        let ystops = || ops.1.swap(0, std::sync::atomic::Ordering::Relaxed);
        let cold = eval(&mut eng);
        assert_eq!(out(&eng), Some(1000 + K * (K - 1) / 2), "guess {guess}: cold");
        reps.push((cold.stops, ystops()));
        for xv in [2, 3, 4] {
            let a = vec![eng.values().str("s"), eng.values().int(xv)];
            eng.replace(s, op, a);
            let rep = eval(&mut eng);
            let bump = if xv == 3 { 50 } else { 0 };
            assert_eq!(out(&eng), Some(xv * 1000 + K * (K - 1) / 2 + bump), "guess {guess}: x {xv}");
            reps.push((rep.stops, ystops()));
            assert!(eng.check_reads().is_empty(), "guess {guess}: {:?}", eng.check_reads());
            assert!(eng.check_invariants().is_empty(), "guess {guess}: {:?}", eng.check_invariants());
        }
    }
    eprintln!("(stops, P's stops on y) without / with: {reps:?}");
    assert_eq!(reps[0], reps[4], "cold");
    // (Run again in the evaluation it committed in, it still guesses; a
    // wrong guess (`x` 3, and back at 4: finisher 3's `y` moves) asks it
    // again, and it guesses the others still.)
    for i in [1, 2, 3] {
        assert!(reps[i].1 > 0 && reps[4 + i].1 == 0, "edit {i}: {reps:?}");
    }
}
