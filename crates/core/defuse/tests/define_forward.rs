//! A definition forwarding a loop-back field (`Ctx::define_forward`)
//! follows the field's round value without its node running again; its
//! readers see what a relay (`use_field` then `define`) gives them. Unit
//! [p, fw, r]: `p` reads the loop-back field `n` and publishes `n + 1` up
//! to 3; `fw` defines `out` as `n` (forwarded, or relayed); `r` reads
//! `out`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Stop, ValueId, Values};

struct Ops {
    forward: bool,
    // (`p` declares `n` in its run, not the engine before.)
    late: bool,
    fw_runs: Arc<AtomicUsize>,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if v.as_str(a[0]).as_deref() == Some("fw") { vec![(v.field("out"), Locality::Local)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (n, out) = (v.field("n"), v.field("out"));
        match v.as_str(a[0]).as_deref() {
            Some("p") => {
                if self.late {
                    cx.declare_loopback(n, v.int(0));
                }
                let x = cx.use_field(n)?.and_then(|y| v.as_int(y)).unwrap_or(0);
                cx.publish(n, v.int((x + 1).min(3)));
                Ok(v.int(x))
            }
            Some("fw") => {
                self.fw_runs.fetch_add(1, Ordering::Relaxed);
                if self.forward {
                    cx.define_forward(out, n, Locality::Local)?;
                } else if let Some(x) = cx.use_field(n)? {
                    cx.define_with(out, x, Locality::Local);
                }
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_field(out)?.unwrap_or_else(|| v.int(-1))),
        }
    }
}

/// `r`'s outputs (evaluated, and evaluated again) and `fw`'s runs, for
/// one setup.
fn build(forward: bool, late: bool) -> (Vec<i64>, usize) {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { forward, late, fw_runs: runs.clone() }));
    let (n, zero) = (eng.values().field("n"), eng.values().int(0));
    if !late {
        eng.declare_loopback(n, zero);
    }
    let u = eng.new_unit();
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    let fw = vec![eng.values().str("fw")];
    eng.push(u, op, fw);
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    let mut outs = Vec::new();
    eng.evaluate_sequential().expect("evaluate");
    outs.push(eng.output(r).and_then(|y| eng.values().as_int(y)).unwrap_or(-9));
    eng.evaluate_sequential().expect("evaluate again");
    outs.push(eng.output(r).and_then(|y| eng.values().as_int(y)).unwrap_or(-9));
    (outs, runs.load(Ordering::Relaxed))
}

#[test]
fn a_forwarded_definition_follows_the_round_value_without_a_rerun() {
    let (relay, relay_runs) = build(false, false);
    let (fwd, fwd_runs) = build(true, false);
    assert_eq!(relay, vec![3, 3]);
    assert_eq!(fwd, relay);
    assert_eq!(fwd_runs, 1, "relay ran {relay_runs}");
    assert!(relay_runs > fwd_runs);
}

#[test]
fn a_field_declared_a_loop_back_later_turns_the_read_into_a_forward() {
    let (relay, relay_runs) = build(false, true);
    let (fwd, fwd_runs) = build(true, true);
    assert_eq!(fwd, relay);
    assert_eq!(relay[0], 3);
    // (Once reading, once forwarding.)
    assert!(fwd_runs <= 2 && fwd_runs < relay_runs, "forward {fwd_runs}, relay {relay_runs}");
}
