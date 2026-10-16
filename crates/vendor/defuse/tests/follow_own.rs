//! A node's sibling reads do not see the follow nodes it placed: `A` reads
//! `following(0)` and follows `B`; `B` reads `following(0)` and claims 1.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use defuse::{Claim, Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: std::sync::Mutex<Option<OpId>>,
    runs_a: AtomicU64,
    b_saw: std::sync::Mutex<Vec<String>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "b" || tag(v, a) == "a" {
            Claim::Pending
        } else {
            Claim::None
        }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        match tag(v, &a).as_str() {
            "a" => {
                self.runs_a.fetch_add(1, Relaxed);
                let next = cx.following(0).map(|(_, xs)| tag(v, &xs)).unwrap_or_default();
                cx.claim(0);
                cx.follow(op, vec![v.str("b")]);
                Ok(v.str(&next))
            }
            "b" => {
                let next = cx.following(0).map(|(_, xs)| tag(v, &xs)).unwrap_or_default();
                self.b_saw.lock().unwrap().push(next.clone());
                cx.claim(1);
                Ok(v.str(&next))
            }
            _ => Ok(ValueId::UNIT),
        }
    }
}

#[test]
fn a_node_does_not_read_its_own_follows() {
    let ops = Arc::new(Ops {
        me: std::sync::Mutex::new(None),
        runs_a: AtomicU64::new(0),
        b_saw: std::sync::Mutex::new(Vec::new()),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (sa, ss) = (eng.values().str("a"), eng.values().str("s"));
    let a = eng.push(u, op, vec![sa]);
    let s = eng.push(u, op, vec![ss]);
    let s1 = {
        let x = eng.values().str("t");
        eng.push(u, op, vec![x])
    };
    eng.evaluate_sequential().unwrap();
    // An edit after them makes A's log be checked again: it holds.
    let x = eng.values().str("u");
    eng.replace(s1, op, vec![x]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(
        eng.output(a),
        Some(eng.values().str("s")),
        "A sees the original sibling"
    );
    assert_eq!(ops.runs_a.load(Relaxed), 1, "A runs once");
    assert_eq!(
        ops.b_saw.lock().unwrap().last().map(String::as_str),
        Some("s"),
        "B sees S"
    );
    assert!(eng.output(s).is_none() || eng.node_info(s).is_some());
}
