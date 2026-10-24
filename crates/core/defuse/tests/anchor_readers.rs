//! A range read's anchors are read for where they are, not for their
//! outputs: a retext of an anchor (its output the same) re-asks the
//! reader's folds through its contribution only. Unit [p0..p19, r]: each
//! `pi` contributes its argument to `acc`; `r` folds `acc` after each.
//! Retexting `p5` (contributing as before) re-asks no reader on all its
//! reads of structure and outputs (why 18), and `r` does not run.

use std::sync::{Arc, OnceLock};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops {
    ps: OnceLock<Vec<NodeId>>,
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let acc = v.field("acc");
        if v.as_str(a[0]).as_deref() == Some("p") {
            cx.contribute(acc, v.int(v.as_int(a[2]).unwrap_or(0)));
            return Ok(ValueId::UNIT);
        }
        let mut s = 0;
        for &p in self.ps.get().expect("pushed") {
            s += v.as_int(cx.acc_range(acc, Some(p))?).unwrap_or(0);
        }
        Ok(v.int(s))
    }
}

#[test]
fn a_retext_of_an_anchor_does_not_re_ask_its_range_readers_structure() {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { ps: OnceLock::new() });
    let op = eng.register(ops.clone());
    eng.declare_accumulator(eng.values().field("acc"), Arc::new(phitex_defuse::SumMonoid));
    eng.set_run_audit(true);
    let u = eng.new_unit();
    let ps: Vec<NodeId> = (0..20)
        .map(|i| {
            let a = vec![eng.values().str("p"), eng.values().int(i), eng.values().int(1)];
            eng.push(u, op, a)
        })
        .collect();
    let _ = ops.ps.set(ps.clone());
    let a = vec![eng.values().str("r")];
    let r = eng.push(u, op, a);
    eng.add_root(r);
    eng.evaluate_sequential().expect("evaluate");
    let out = |eng: &Engine| eng.output(r).and_then(|y| eng.values().as_int(y));
    // (After `pi`: the 19 - i after it.)
    assert_eq!(out(&eng), Some((0..20).map(|i| 19 - i).sum()));
    // Another text, the same contribution and output.
    let a = vec![eng.values().str("p"), eng.values().int(105), eng.values().int(1)];
    eng.replace(ps[5], op, a);
    let rep = eng.evaluate_sequential().expect("evaluate");
    assert_eq!(out(&eng), Some((0..20).map(|i| 19 - i).sum()));
    let audit = rep.audit.expect("audited");
    assert!(!audit.rechecked_all.iter().any(|e| e.0 == 18), "re-asked on outputs: {:?}", audit.rechecked_all);
    assert_eq!(rep.runs, 1, "only the retexted node runs");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
}
