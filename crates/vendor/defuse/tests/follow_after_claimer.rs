//! A node placed after a claimer goes after the siblings it claims (a
//! claim is never split): unit [s, p, a, a, b]; `p` claims the `a`
//! siblings ahead of it; `s`, before `p`, places `f` after `p`, as a page
//! builder places a finisher after a piece. `f` comes after both `a`s,
//! and `p` keeps claiming them.

use std::sync::{Arc, OnceLock};

use defuse::{Claim, Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: OnceLock<OpId>,
    p: OnceLock<NodeId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _f: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "p" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        match tag(v, &a).as_str() {
            "s" => {
                if v.as_int(a[1]) == Some(1)
                    && let Some(&p) = self.p.get()
                {
                    cx.follow_after_any(p, op, vec![v.str("f")]);
                }
                Ok(ValueId::UNIT)
            }
            "p" => {
                let mut k = 0;
                while cx.following(k).is_some_and(|(_, xs)| tag(v, &xs) == "a") {
                    k += 1;
                }
                cx.claim(k);
                Ok(v.int(i64::try_from(k).unwrap()))
            }
            _ => Ok(v.int(1)),
        }
    }
}

#[test]
fn a_node_placed_after_a_claimer_goes_after_what_it_claims() {
    for seq in [true, false] {
        let ops = Arc::new(Ops {
            me: OnceLock::new(),
            p: OnceLock::new(),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let s = {
            let v = eng.values();
            vec![v.str("s"), v.int(0)]
        };
        let s = eng.push(u, op, s);
        let p = vec![eng.values().str("p")];
        let p = eng.push(u, op, p);
        let _ = ops.p.set(p);
        let a: Vec<NodeId> = (0..2)
            .map(|_| {
                let a = vec![eng.values().str("a")];
                eng.push(u, op, a)
            })
            .collect();
        let b = vec![eng.values().str("b")];
        let b = eng.push(u, op, b);
        let eval = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential()
            } else {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential)
            }
            .expect("evaluate")
        };
        eval(&mut eng);
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(2), "cold");
        let x = {
            let v = eng.values();
            vec![v.str("s"), v.int(1)]
        };
        eng.replace(s, op, x);
        let rep = eval(&mut eng);
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(2), "seq {seq}: {rep:?}");
        let order = eng.order(u);
        let at = |x: NodeId| order.iter().position(|&y| y == x).expect("there");
        let f = *eng.children(s).first().expect("f placed");
        assert!(at(f) > at(a[1]) && at(f) < at(b), "seq {seq}: f at {} in {order:?}, a {a:?}, b {b:?}", at(f));
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
