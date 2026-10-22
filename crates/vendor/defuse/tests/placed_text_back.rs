//! A placer that gives its placed node a text and then its earlier one
//! again: the node's last result is of that text once more, and its
//! readers are re-asked. Unit [p, a, q]: loop-back `lp` goes 0, 1, 2 over
//! three rounds (`q` publishes `lp + 1`, at most 2); `p` places final node
//! `f(lp mod 2)` after `a` and outputs `placed_output_or(f, -5)`; `f(k)`
//! outputs `10 k + 3`. In round 2 `f` is held with `f(0)`'s result, `p`
//! places `f(0)` again in round 3 having read `f` while it was `f(1)`, and
//! the final phase's `f(0)` commits the output it had.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops(std::sync::OnceLock<NodeId>);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
        tag(v, a) == "f"
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let lp = v.field("lp");
        match tag(v, &a).as_str() {
            "p" => {
                let k = cx.use_field(lp)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                let after = *self.0.get().expect("a made");
                let i = cx.follow_after_any(after, cx.op(), vec![v.str("f"), v.int(k % 2)]);
                Ok(cx.placed_output_or(i, v.int(-5)))
            }
            "q" => {
                let k = cx.use_field(lp)?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.publish(lp, v.int((k + 1).min(2)));
                Ok(ValueId::UNIT)
            }
            "f" => Ok(v.int(10 * v.as_int(a[1]).unwrap() + 3)),
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_placed_node_given_its_earlier_text_again_re_asks_its_readers() {
    for seq in [true, false] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(std::sync::OnceLock::new()));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        let lp = eng.values().field("lp");
        eng.declare_loopback(lp, eng.values().int(0));
        let u = eng.new_unit();
        let p_args = vec![eng.values().str("p")];
        let p = eng.push(u, op, p_args);
        let a_args = vec![eng.values().str("a")];
        let a = eng.push(u, op, a_args);
        ops.0.set(a).expect("once");
        let q_args = vec![eng.values().str("q")];
        let q = eng.push(u, op, q_args);
        let run = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential().expect("evaluate");
            } else {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
            }
        };
        run(&mut eng);
        assert!(eng.check_reads().is_empty(), "seq {seq}: {:?}", eng.check_reads());
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(3), "seq {seq}");
        // Again from the initial loop-back value: `f` has `f(0)`'s result
        // from the first evaluation when `p` places `f(1)`, then `f(0)`.
        for k in 1..4 {
            let q_args = vec![eng.values().str("q"), eng.values().int(k)];
            eng.replace(q, op, q_args);
            let p_args = vec![eng.values().str("p"), eng.values().int(k)];
            eng.replace(p, op, p_args);
            run(&mut eng);
            assert!(eng.check_reads().is_empty(), "seq {seq} edit {k}: {:?}", eng.check_reads());
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(3), "seq {seq} edit {k}");
        }
    }
}

/// Unit [s, p, a]: final `s` defines `x` (predicted 2, made 1); `p`
/// places final `g(x)` after `a` and outputs `placed_output_or(g, -5)`;
/// `g(k)` outputs 7. With `s` held, `p` commits on the prediction and
/// places `g(2)`; refuted, it places `g(1)` again, reading `g` while it
/// is `g(2)`, and `g(1)` commits the output it had.
mod predicted {
    use std::sync::Arc;

    use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

    pub struct Ops(pub std::sync::OnceLock<NodeId>);

    fn tag(v: &Values, a: &[ValueId]) -> String {
        v.as_str(a[0]).unwrap_or_default()
    }

    impl Operation for Ops {
        fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(if tag(v, a) == "s" { vec![(v.field("x"), Locality::Global)] } else { Vec::new() })
        }
        fn predicted_defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
            (tag(v, a) == "s").then(|| vec![(v.field("x"), v.int(2))])
        }
        fn final_only(&self, a: &[ValueId], v: &Values) -> bool {
            matches!(tag(v, a).as_str(), "s" | "g")
        }
        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let a = cx.args();
            match tag(v, &a).as_str() {
                "s" => {
                    cx.define_with(v.field("x"), v.int(1), Locality::Global);
                    Ok(ValueId::UNIT)
                }
                "p" => {
                    let x = cx.use_field(v.field("x"))?.unwrap_or(v.int(0));
                    let after = *self.0.get().expect("a made");
                    let i = cx.follow_after_any(after, cx.op(), vec![v.str("g"), x]);
                    Ok(cx.placed_output_or(i, v.int(-5)))
                }
                "g" => Ok(v.int(7)),
                _ => Ok(ValueId::UNIT),
            }
        }
    }

    #[test]
    fn a_placer_refuted_back_to_its_earlier_placement_is_re_asked() {
        for seq in [true, false] {
            let mut eng = Engine::new();
            let ops = Arc::new(Ops(std::sync::OnceLock::new()));
            let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
            let u = eng.new_unit();
            let s_args = vec![eng.values().str("s")];
            let s = eng.push(u, op, s_args);
            let p_args = vec![eng.values().str("p")];
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
            assert!(eng.check_reads().is_empty(), "seq {seq}: {:?}", eng.check_reads());
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(7), "seq {seq}");
            for k in 1..4 {
                let s_args = vec![eng.values().str("s"), eng.values().int(k)];
                eng.replace(s, op, s_args);
                let p_args = vec![eng.values().str("p"), eng.values().int(k)];
                eng.replace(p, op, p_args);
                run(&mut eng);
                assert!(eng.check_reads().is_empty(), "seq {seq} edit {k}: {:?}", eng.check_reads());
                assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(7), "seq {seq} edit {k}");
            }
        }
    }
}
