//! A kept state is of the text its token was parked under: unit [s, p];
//! `s` defines `x`; `p` places continuation `f(x)`; `f` waits on the
//! environment field `e`, parking a token that holds its argument, and
//! once `e` is supplied outputs `100 arg + e` from its token. An edit of
//! `s` (`x` 1 -> 2) gives `f`, still parked, a new text (same node, the placer re-makes
//! it); `e` supplied after, `f` must run on its new argument, not resume the
//! token of the old.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Stop, ValueId, Values};

struct Ops(std::sync::OnceLock<OpId>);

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
        let op = *self.0.get().unwrap();
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(v.field("x"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "p" => {
                let x = cx.use_field(v.field("x"))?.unwrap_or(v.int(0));
                cx.follow(op, vec![v.str("f"), x]);
                Ok(ValueId::UNIT)
            }
            "f" => {
                let arg = match cx.parked().and_then(|t| t.downcast::<i64>().ok()) {
                    Some(t) => *t,
                    None => v.as_int(a[1]).unwrap(),
                };
                match cx.use_env(v.field("e")) {
                    Ok(Some(e)) => Ok(v.int(arg * 100 + v.as_int(e).unwrap())),
                    Ok(None) => Ok(v.int(-1)),
                    Err(s) => {
                        cx.park(Box::new(arg));
                        Err(s)
                    }
                }
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_node_given_a_new_text_does_not_resume_the_old_ones_token() {
    for seq in [true, false] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(std::sync::OnceLock::new()));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        ops.0.set(op).expect("once");
        let u = eng.new_unit();
        let s_args = vec![eng.values().str("s"), eng.values().int(1)];
        let s = eng.push(u, op, s_args);
        let p_args = vec![eng.values().str("p")];
        eng.push(u, op, p_args);
        let run = |eng: &mut Engine| {
            if seq {
                eng.evaluate_sequential().expect("evaluate");
            } else {
                eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::All, 64), &mut phitex_defuse::Sequential).expect("evaluate");
            }
        };
        run(&mut eng);
        let f = |eng: &Engine| -> NodeId { *eng.order(u).last().expect("f placed") };
        assert_eq!(eng.output(f(&eng)), None, "seq {seq}: f waits on e");
        // (The placer gives `f` its new text while `f` stays parked.)
        let s2 = vec![eng.values().str("s"), eng.values().int(2)];
        eng.replace(s, op, s2);
        run(&mut eng);
        assert_eq!(eng.output(f(&eng)), None, "seq {seq}: f still waits on e");
        let (e, five) = (eng.values().field("e"), eng.values().int(5));
        eng.set_environment(vec![(e, five)]);
        run(&mut eng);
        assert_eq!(eng.output(f(&eng)).and_then(|x| eng.values().as_int(x)), Some(205), "seq {seq}: f on its new text");
        assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    }
}
