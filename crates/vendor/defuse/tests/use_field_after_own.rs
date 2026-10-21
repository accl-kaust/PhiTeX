//! A read right after a node's own definitions (`Ctx::use_field_after_own`):
//! piece `p` defines `pagepar` and has a child `b` (its lines) that
//! defines it too and contributes; `pages` places a finisher `f` after `b`
//! (inside `p`'s extent), which reads `pagepar` right after `p`'s own
//! run: `p`'s value, not `b`'s (nor anything placed in `p`'s extent).
//! An edit to `p`'s value reaches `f`.

use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, FieldAt, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: OnceLock<OpId>,
    p: OnceLock<NodeId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn pagepar(v: &Values) -> Field {
    v.field("pagepar")
}

fn vprof(v: &Values) -> Field {
    v.field("acc:vprof")
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "p" | "b" => Some(vec![(pagepar(v), Locality::Global)]),
            // (Undeclared: a pending set while in work.)
            "fin" => None,
            _ => Some(Vec::new()),
        }
    }
    fn static_contribs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        (tag(v, a) != "b").then(Vec::new)
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.me.get().unwrap();
        match tag(v, &a).as_str() {
            "pages" => {
                let mut after = None;
                while let Some((x, _)) = cx.acc_next(vprof(v), after)? {
                    cx.follow_after_any(x, op, vec![v.str("fin")]);
                    after = Some(x);
                }
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.define_with(pagepar(v), a[1], Locality::Global);
                cx.child(op, vec![v.str("b")]);
                Ok(a[1])
            }
            "b" => {
                cx.define_with(pagepar(v), v.str("lines"), Locality::Global);
                cx.contribute(vprof(v), v.int(1));
                Ok(ValueId::UNIT)
            }
            "fin" => {
                let p = *self.p.get().unwrap();
                Ok(match cx.use_field_after_own(pagepar(v), p)? {
                    FieldAt::Def(x, _) => x,
                    _ => v.str("none"),
                })
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

fn build() -> (Engine, NodeId, OpId) {
    let ops = Arc::new(Ops {
        me: OnceLock::new(),
        p: OnceLock::new(),
    });
    let mut eng = Engine::new();
    let acc = vprof(eng.values());
    eng.declare_accumulator(acc, Arc::new(defuse::SumMonoid));
    let op = eng.register(ops.clone());
    let _ = ops.me.set(op);
    let u = eng.new_unit();
    let pages = vec![eng.values().str("pages")];
    eng.push(u, op, pages);
    let p = {
        let v = eng.values();
        vec![v.str("p"), v.str("own")]
    };
    let p = eng.push(u, op, p);
    let _ = ops.p.set(p);
    (eng, p, op)
}

fn finisher(eng: &Engine, p: NodeId) -> NodeId {
    // (Pages' only placement.)
    let pages = eng.order(eng.node_info(p).expect("p").unit)[0];
    eng.children(pages)[0]
}

#[test]
fn a_read_right_after_a_nodes_own_run_sees_its_definitions_only() {
    for seq in [true, false] {
        let (mut eng, p, op) = build();
        let eval = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential()
            } else {
                eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential)
            }
        };
        let r = eval(&mut eng);
        assert!(r.is_ok(), "seq {seq}: {r:?}");
        let f = finisher(&eng, p);
        let out = |eng: &Engine| eng.output(f).and_then(|x| eng.values().as_str(x));
        assert_eq!(out(&eng).as_deref(), Some("own"), "seq {seq}");
        let a = {
            let v = eng.values();
            vec![v.str("p"), v.str("edited")]
        };
        eng.replace(p, op, a);
        let r = eval(&mut eng);
        assert!(r.is_ok(), "seq {seq}: {r:?}");
        assert_eq!(out(&eng).as_deref(), Some("edited"), "seq {seq}");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
        assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    }
}
