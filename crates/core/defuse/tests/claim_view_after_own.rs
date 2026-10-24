//! A node another node places right after a claimer, or after one of its
//! continuations, is not in the claimer's view, whoever placed it: unit
//! [s, p, a, a]; `p` claims the `a` siblings ahead of it and follows `r`;
//! `s`, before `p`, places `f` after `p` (or after `r`), as a page scan
//! places a finisher after a piece. `p` keeps claiming both `a`s.

use std::sync::Arc;
use std::sync::OnceLock;

use phitex_defuse::{Claim, Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: OnceLock<OpId>,
    /// `p` and its continuation `r` (known after the cold build).
    p: OnceLock<NodeId>,
    r: OnceLock<NodeId>,
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
                let at = match v.as_str(a[1]).unwrap_or_default().as_str() {
                    "after p" => self.p.get().copied(),
                    "after r" => self.r.get().copied(),
                    _ => None,
                };
                if let Some(at) = at {
                    cx.follow_after_any(at, op, vec![v.str("f")]);
                }
                Ok(ValueId::UNIT)
            }
            "p" => {
                let mut k = 0;
                while cx.following(k).is_some_and(|(_, xs)| tag(v, &xs) == "a") {
                    k += 1;
                }
                cx.claim(k);
                cx.follow(op, vec![v.str("r")]);
                Ok(v.int(i64::try_from(k).unwrap()))
            }
            "r" => {
                let _ = self.r.set(cx.node());
                Ok(v.int(1))
            }
            _ => Ok(v.int(1)),
        }
    }
}

fn build() -> (Engine, Arc<Ops>, NodeId, NodeId, OpId) {
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
        p: OnceLock::new(),
        r: OnceLock::new(),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let s = {
        let v = eng.values();
        vec![v.str("s"), v.str("none")]
    };
    let s = eng.push(u, op, s);
    let p = vec![eng.values().str("p")];
    let p = eng.push(u, op, p);
    let _ = ops.p.set(p);
    for _ in 0..2 {
        let a = vec![eng.values().str("a")];
        eng.push(u, op, a);
    }
    (eng, ops, s, p, op)
}

#[test]
fn a_node_placed_after_a_claimer_or_its_continuation_is_not_in_its_view() {
    for at in ["after p", "after r"] {
        for seq in [true, false] {
            let (mut eng, ops, s, p, op) = build();
            let eval = |eng: &mut Engine| {
                if seq {
                    eng.evaluate_sequential()
                } else {
                    eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64), &mut phitex_defuse::Sequential)
                }
                .expect("evaluate")
            };
            eval(&mut eng);
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(2), "cold");
            assert!(ops.r.get().is_some(), "p's continuation ran");
            let a = {
                let v = eng.values();
                vec![v.str("s"), v.str(at)]
            };
            eng.replace(s, op, a);
            let rep = eval(&mut eng);
            assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some(2), "{at}, seq {seq}: {rep:?}");
            assert!(rep.runs <= 6, "{at}, seq {seq}: {rep:?}");
            assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
        }
    }
}
