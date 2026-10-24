//! The frames of nodes placed while their unit is renumbered: unit
//! [pages, open, a, b, t, close, u]; `open` opens a scope of kind 7 that
//! `close` closes; `pages` places many finishers right after `a` (enough
//! that the gap after `a` runs out and the unit is renumbered while they
//! are placed; nodes removed before `a` first, so that renumbering moves
//! positions down). `t` stays in the scope (kind 7), `u` outside it.

use std::sync::{Arc, OnceLock};

use phitex_defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

/// Finishers placed after `a`: more than the gap after it holds, each
/// placed a step after the last.
const K: i64 = 1100;

struct Ops {
    me: OnceLock<OpId>,
    a: OnceLock<NodeId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn effect(&self, a: &[ValueId], v: &Values) -> Effect {
        match tag(v, a).as_str() {
            "open" => Effect::Scope(Brackets {
                opens: 1,
                kind: Some(v.int(7)),
                ..Brackets::default()
            }),
            "close" => Effect::Close,
            _ => Effect::None,
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        match tag(v, &a).as_str() {
            "pages" => {
                let k = v.as_int(a[1]).unwrap_or(0);
                if let Some(&at) = self.a.get() {
                    for i in 0..k {
                        cx.follow_after_any(at, op, vec![v.str("fin"), v.int(i)]);
                    }
                }
                Ok(v.int(k))
            }
            "t" | "u" => Ok(cx.scope_kind()?.unwrap_or(v.int(-1))),
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn nodes_placed_across_a_renumbering_keep_their_scope() {
    for seq in [true, false] {
        let ops = Arc::new(Ops {
            me: OnceLock::new(),
            a: OnceLock::new(),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let args = |eng: &Engine, t: &str| vec![eng.values().str(t)];
        let pages = {
            let v = eng.values();
            vec![v.str("pages"), v.int(0)]
        };
        let pages = eng.push(u, op, pages);
        let fill: Vec<NodeId> = (0..8)
            .map(|_| {
                let x = args(&eng, "x");
                eng.push(u, op, x)
            })
            .collect();
        for t in ["open", "a", "b", "t", "close", "u"] {
            let x = args(&eng, t);
            let id = eng.push(u, op, x);
            if t == "a" {
                let _ = ops.a.set(id);
            }
        }
        let order = eng.order(u);
        let (t, uu) = (order[12], order[14]);
        let eval = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential()
            } else {
                eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64), &mut phitex_defuse::Sequential)
            }
            .expect("evaluate")
        };
        eval(&mut eng);
        let kind = |eng: &Engine, x: NodeId| eng.output(x).and_then(|y| eng.values().as_int(y));
        assert_eq!((kind(&eng, t), kind(&eng, uu)), (Some(7), Some(-1)), "cold");
        let x = {
            let v = eng.values();
            vec![v.str("pages"), v.int(K)]
        };
        eng.replace(pages, op, x);
        for x in fill {
            eng.remove(x);
        }
        let rep = eval(&mut eng);
        assert_eq!(eng.order(u).len(), 7 + usize::try_from(K).unwrap(), "seq {seq}");
        assert_eq!((kind(&eng, t), kind(&eng, uu)), (Some(7), Some(-1)), "seq {seq}: {rep:?}");
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
