//! `Ctx::placed_output`: a placer reads the output of a continuation it
//! placed after a later node (`follow_after_any`), as committed under the
//! text it placed it with. Unit [s, p, a]: `s` defines `x`; `p` places
//! finisher `f(x)` after `a` and outputs `f`'s output (-1 while `f` has
//! none of that text). `f` outputs `10 x` (or 7 whatever `x` is, the
//! `k` variant: a new text with the same output still re-asks `p`).
//! An edit of `s` (`x` 1 -> 2) gives `f` a new text.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

/// (`a`, the node `p` places its finisher after, once made; `p`'s runs.)
struct Ops(std::sync::OnceLock<NodeId>, std::sync::atomic::AtomicU32);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "s" => Some(vec![(v.field("x"), Locality::Global)]),
            _ => Some(Vec::new()),
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(v.field("x"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "p" => {
                self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let x = cx.use_field(v.field("x"))?.unwrap_or(v.int(0));
                let after = *self.0.get().expect("a made");
                let i = cx.follow_after_any(after, cx.op(), vec![a[1], x]);
                // ("p" "f" g: guesses `f`'s output `g x`.)
                Ok(match a.get(2).and_then(|&g| v.as_int(g)) {
                    Some(g) => cx.placed_output_or(i, v.int(g * v.as_int(x).unwrap())),
                    None => cx.placed_output(i).unwrap_or(v.int(-1)),
                })
            }
            "f" => Ok(v.int(10 * v.as_int(a[1]).unwrap())),
            "k" => Ok(v.int(7)),
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_placer_reads_its_finisher_as_committed_under_its_text() {
    for (fin, want1, want2) in [("f", 10, 20), ("k", 7, 7)] {
        for seq in [true, false] {
            let mut eng = Engine::new();
            let ops = Arc::new(Ops(std::sync::OnceLock::new(), std::sync::atomic::AtomicU32::new(0)));
            let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
            let u = eng.new_unit();
            let v = eng.values();
            let s_args = vec![v.str("s"), v.int(1)];
            let s = eng.push(u, op, s_args);
            let p_args = vec![eng.values().str("p"), eng.values().str(fin)];
            let p = eng.push(u, op, p_args);
            let a_args = vec![eng.values().str("a")];
            let a = eng.push(u, op, a_args);
            ops.0.set(a).expect("once");
            let run = |eng: &mut Engine| {
                if seq {
                    eng.evaluate_sequential().expect("evaluate");
                } else {
                    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
                }
            };
            run(&mut eng);
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(want1), "{fin} seq {seq}");
            assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
            let s_args = vec![eng.values().str("s"), eng.values().int(2)];
            eng.replace(s, op, s_args);
            run(&mut eng);
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(want2), "{fin} seq {seq} after the edit");
            assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
        }
    }
}

/// A placer that goes on with a guess of its finisher's output
/// (`Ctx::placed_output_or`) runs again only when the finisher outputs
/// something else: guessing `10 x` (right), once per evaluation; guessing
/// `0`, twice; either way it ends on the finisher's output.
#[test]
fn a_right_guess_of_a_placed_output_costs_no_run() {
    // (Attempts counted: the first evaluation's first stops on `s`.)
    for (g, runs) in [(10, 3), (0, 5)] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(std::sync::OnceLock::new(), std::sync::atomic::AtomicU32::new(0)));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        let u = eng.new_unit();
        let s_args = vec![eng.values().str("s"), eng.values().int(1)];
        let s = eng.push(u, op, s_args);
        let p_args = vec![eng.values().str("p"), eng.values().str("f"), eng.values().int(g)];
        let p = eng.push(u, op, p_args);
        let a_args = vec![eng.values().str("a")];
        let a = eng.push(u, op, a_args);
        ops.0.set(a).expect("once");
        eng.evaluate_sequential().expect("evaluate");
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(10), "guess {g}");
        let s_args = vec![eng.values().str("s"), eng.values().int(2)];
        eng.replace(s, op, s_args);
        eng.evaluate_sequential().expect("evaluate");
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(20), "guess {g} after the edit");
        assert_eq!(ops.1.load(std::sync::atomic::Ordering::Relaxed), runs, "guess {g}: runs of p");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
}
