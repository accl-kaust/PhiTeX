//! `Ctx::placed_output`: a placer reads the output of a continuation it
//! placed after a later node (`follow_after_any`), as committed under the
//! text it placed it with. Unit [s, p, a]: `s` defines `x`; `p` places
//! finisher `f(x)` after `a` and outputs `f`'s output (-1 while `f` has
//! none of that text). `f` outputs `10 x` (or 7 whatever `x` is, the
//! `k` variant: a new text with the same output still re-asks `p`).
//! An edit of `s` (`x` 1 -> 2) gives `f` a new text.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

/// (`a`, the node `p` places its finisher after, once made.)
struct Ops(std::sync::OnceLock<NodeId>);

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
                let x = cx.use_field(v.field("x"))?.unwrap_or(v.int(0));
                let after = *self.0.get().expect("a made");
                let i = cx.follow_after_any(after, cx.op(), vec![a[1], x]);
                Ok(cx.placed_output(i).unwrap_or(v.int(-1)))
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
            let ops = Arc::new(Ops(std::sync::OnceLock::new()));
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
