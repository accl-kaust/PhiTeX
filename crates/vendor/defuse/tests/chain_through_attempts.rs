//! The critical chain through a placer's attempts: `parent` places a
//! child per attempt and waits for its output (`Ctx::park_continuing`),
//! as a page builder its finishers. Each child is created by the parent's
//! attempt then, and each attempt stood on the child before: the chain
//! walks back through them, every link to an earlier run, not between the
//! parent's last attempt and its last child.

use std::sync::{Arc, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

const N: i64 = 6;

struct Ops(OnceLock<OpId>);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _a: &[ValueId], _v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.0.get().unwrap();
        match tag(v, &a).as_str() {
            "parent" => {
                let k = cx.parked().and_then(|t| t.downcast::<usize>().ok()).map_or(0, |b| *b);
                let mut sum = 0;
                for i in 0..usize::try_from(N).unwrap() {
                    if i >= k {
                        cx.child(op, vec![v.str("kid"), v.int(i64::try_from(i).unwrap())]);
                    }
                    match cx.output_of(i) {
                        Ok(x) => sum += v.as_int(x).unwrap_or(0),
                        Err(s) => {
                            cx.park_continuing(Box::new(i + 1));
                            return Err(s);
                        }
                    }
                }
                Ok(v.int(sum))
            }
            _ => Ok(a[1]),
        }
    }
}

#[test]
fn the_chain_walks_back_through_a_placers_attempts() {
    for seq in [true, false] {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(OnceLock::new()));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        ops.0.set(op).expect("once");
        let u = eng.new_unit();
        let args = vec![eng.values().str("parent")];
        let p = eng.push(u, op, args);
        let rep = if seq {
            eng.evaluate_sequential().expect("evaluate")
        } else {
            eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate")
        };
        assert_eq!(eng.output(p).and_then(|x| eng.values().as_int(x)), Some((0..N).sum()), "seq {seq}");
        let chain = eng.critical_chain_heights();
        // (One link per height, each one lower; the parent's attempts and
        // its children alternate, at most once each.)
        assert_eq!(chain.len() as u32, rep.realised_critical_path, "seq {seq}: {chain:?}");
        assert!(chain.windows(2).all(|w| w[1].1 + 1 == w[0].1), "seq {seq}: {chain:?}");
        assert!(chain.len() <= 2 * usize::try_from(N).unwrap() + 1, "seq {seq}: {chain:?}");
        let kids = chain.iter().filter(|c| c.0 != p).count();
        assert!(kids >= usize::try_from(N).unwrap() - 1, "seq {seq}: the children on the chain: {chain:?}");
    }
}
