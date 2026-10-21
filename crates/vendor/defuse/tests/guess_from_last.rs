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
