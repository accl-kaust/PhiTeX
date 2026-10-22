//! `Engine::set_claim_window`: a pending claimer without a reach holds
//! back only the siblings in the window after it; those past it run before
//! its claim is known, and are taken back when it claims them. Unit
//! [z, c, d_1 .. d_N, r]: `z` defines `q`; `c` (pending) reads `q` and
//! claims `q` siblings; `d_k` defines `x = k` and outputs `k`; `r` reads
//! `x`. Cold, `c` waits for `z`, the `d`s past the window run meanwhile.
//! The outputs are the sequential ones, the claim within the window or
//! past it.

use std::sync::Arc;

use defuse::{Claim, Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

const N: i64 = 8;

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "z" => vec![(v.field("q"), Locality::Global)],
            "d" => vec![(v.field("x"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn claim(&self, a: &[ValueId], v: &Values, _: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>) -> Claim {
        if tag(v, a) == "c" { Claim::Pending } else { Claim::None }
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "z" => {
                cx.define_with(v.field("q"), a[1], Locality::Global);
                Ok(ValueId::UNIT)
            }
            "c" => {
                // (Claims `q`, or its own count when it has one.)
                let n = cx.use_field(v.field("q"))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                let n = a.get(1).and_then(|x| v.as_int(*x)).unwrap_or(n);
                cx.claim(usize::try_from(n).unwrap());
                Ok(v.int(n))
            }
            "d" => {
                cx.define_with(v.field("x"), a[1], Locality::Global);
                Ok(a[1])
            }
            _ => Ok(cx.use_field(v.field("x"))?.unwrap_or_else(|| v.int(-1))),
        }
    }
}

/// The outputs of every node after a build claiming `k`, and its report.
fn build(k: i64, window: Option<usize>, seq: bool) -> (Vec<Option<i64>>, Report) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_claim_window(window);
    let u = eng.new_unit();
    let mut ids: Vec<NodeId> = Vec::new();
    let z = vec![eng.values().str("z"), eng.values().int(k)];
    ids.push(eng.push(u, op, z));
    let c = vec![eng.values().str("c")];
    ids.push(eng.push(u, op, c));
    for i in 1..=N {
        let d = vec![eng.values().str("d"), eng.values().int(i)];
        ids.push(eng.push(u, op, d));
    }
    let r = vec![eng.values().str("r")];
    ids.push(eng.push(u, op, r));
    let rep = if seq {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential)
    }
    .expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    (ids.iter().map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect(), rep)
}

#[test]
fn siblings_past_the_window_run_before_the_claim_and_are_taken_back() {
    for k in [0, 1, 2, 5, N] {
        let (want, _) = build(k, None, true);
        let (held, held_rep) = build(k, None, false);
        let (win, win_rep) = build(k, Some(2), false);
        assert_eq!(held, want, "claim {k}: held");
        assert_eq!(win, want, "claim {k}: window 2");
        eprintln!("claim {k}: steps held {} window {}; past reach {}", held_rep.steps, win_rep.steps, win_rep.claims_past_reach);
        assert_eq!(held_rep.claims_past_reach, 0, "claim {k}: held");
        assert_eq!(win_rep.claims_past_reach > 0, k > 2, "claim {k}: window 2");
    }
}

/// A run of pending claimers [z, c_0 .. c_5, d, r]: each `c_i` reads `q`
/// (`z` defines it) and claims `k_i`. With a window and the claim chain
/// (`Engine::set_claim_chain`), the `c`s are not held by the ones before
/// them (guessed not claimed): they run together, one a claimer before
/// takes is taken back. The outputs are the sequential ones.
fn run_of_claimers(ks: &[i64], window: Option<usize>, chain: bool, seq: bool) -> (Vec<Option<i64>>, Report) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_claim_window(window);
    eng.set_claim_chain(chain);
    let u = eng.new_unit();
    let mut ids: Vec<NodeId> = Vec::new();
    let z = vec![eng.values().str("z"), eng.values().int(0)];
    ids.push(eng.push(u, op, z));
    for &k in ks {
        // (`c` claims `q`; `q` here is 0: the count is the claimer's own.)
        let c = vec![eng.values().str("c"), eng.values().int(k)];
        ids.push(eng.push(u, op, c));
    }
    let d = vec![eng.values().str("d"), eng.values().int(9)];
    ids.push(eng.push(u, op, d));
    let r = vec![eng.values().str("r")];
    ids.push(eng.push(u, op, r));
    let rep = if seq {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential)
    }
    .expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert!(eng.check_invariants().is_empty(), "{:?}", eng.check_invariants());
    (ids.iter().map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect(), rep)
}

#[test]
fn a_run_of_pending_claimers_goes_on_together() {
    for ks in [vec![0, 0, 0, 0, 0, 0], vec![0, 1, 0, 0, 2, 0]] {
        let (want, _) = run_of_claimers(&ks, None, false, true);
        let (held, held_rep) = run_of_claimers(&ks, None, false, false);
        let (win, win_rep) = run_of_claimers(&ks, Some(2), false, false);
        let (chain, chain_rep) = run_of_claimers(&ks, Some(2), true, false);
        assert_eq!(held, want, "{ks:?}: held");
        assert_eq!(win, want, "{ks:?}: window 2");
        assert_eq!(chain, want, "{ks:?}: window 2, chained");
        eprintln!("{ks:?}: steps held {} window {} chained {}; taken after running {}", held_rep.steps, win_rep.steps, chain_rep.steps, chain_rep.claimed_after_run);
        // (Without the chain, a window frees no pending claimer.)
        assert_eq!(win_rep.steps, held_rep.steps, "{ks:?}: window without the chain");
        assert!(chain_rep.steps < held_rep.steps, "{ks:?}: steps held {} chained {}", held_rep.steps, chain_rep.steps);
    }
}

/// A hold is a link of the critical chain: unit [z, c, d_1 .. d_N, r]
/// with the claim pending and no window, the `d`s start after `c`, held
/// back by it: the chain to `r` passes `c` as the one that woke them.
#[test]
fn a_claim_hold_is_a_link_of_the_chain() {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    eng.set_heights(true);
    let u = eng.new_unit();
    let z = vec![eng.values().str("z"), eng.values().int(0)];
    eng.push(u, op, z);
    let c = vec![eng.values().str("c")];
    let c = eng.push(u, op, c);
    for i in 1..=N {
        let d = vec![eng.values().str("d"), eng.values().int(i)];
        eng.push(u, op, d);
    }
    let r = vec![eng.values().str("r")];
    eng.push(u, op, r);
    eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::Predicted, 256), &mut defuse::Sequential).expect("evaluate");
    let chain = eng.critical_chain();
    assert!(
        chain.iter().any(|(_, why)| matches!(why, defuse::Cause::Woken { by, field: None, .. } if *by == c)),
        "{chain:?}"
    );
}
