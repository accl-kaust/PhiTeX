//! Nodes one commit places far apart get their frames each: unit
//! [p, a, open, b, close]; `p` places, in one run, a reader of its scope
//! kind after `a` (outside the scope) and another after `b` (inside the
//! scope `open` opens, kind 7). The frames are walked once per commit
//! from its lowest placement, lazily (until the frames agree again): the
//! second placement, past where the walk agreed, must be walked too.

use std::sync::{Arc, OnceLock};

use defuse::{Brackets, Ctx, Effect, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: OnceLock<OpId>,
    anchors: OnceLock<(NodeId, NodeId)>,
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
            "p" => {
                let (x, y) = *self.anchors.get().expect("anchors");
                cx.follow_after_any(x, op, vec![v.str("k"), v.int(1)]);
                cx.follow_after_any(y, op, vec![v.str("k"), v.int(2)]);
                Ok(ValueId::UNIT)
            }
            "k" => Ok(cx.scope_kind()?.unwrap_or(v.int(-1))),
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn nodes_placed_far_apart_in_one_commit_get_their_frames() {
    for seq in [true, false] {
        let ops = Arc::new(Ops {
            me: OnceLock::new(),
            anchors: OnceLock::new(),
        });
        let mut eng = Engine::new();
        let op = eng.register(ops.clone());
        let _ = ops.me.set(op);
        let u = eng.new_unit();
        let mut ids = Vec::new();
        for t in ["p", "a", "open", "b", "close"] {
            let x = vec![eng.values().str(t)];
            ids.push(eng.push(u, op, x));
        }
        let _ = ops.anchors.set((ids[1], ids[3]));
        if seq {
            eng.evaluate_sequential()
        } else {
            eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential)
        }
        .expect("evaluate");
        assert!(eng.check_invariants().is_empty(), "seq {seq}: {:?}", eng.check_invariants());
        let order = eng.order(u);
        let kinds: Vec<Option<i64>> = order
            .iter()
            .filter(|&&x| eng.node_info(x).is_some_and(|i| eng.values().as_str(i.args[0]).as_deref() == Some("k")))
            .map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y)))
            .collect();
        assert_eq!(kinds, vec![Some(-1), Some(7)], "seq {seq}");
    }
}
