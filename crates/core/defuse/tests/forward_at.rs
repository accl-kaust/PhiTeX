//! A definition forwarding a part of a loop-back field's value
//! (`Ctx::define_forward_at`): unit [p, fw, r]. `p` reads `n` (a pair
//! `[value, flag]`) and publishes the next pair; `fw` reads only its
//! parts with `loop_part` and forwards the value into `out`, keeping the
//! flag; `r` reads `out`. A round that moves the value runs `r` again and
//! not `fw`; one that moves the flag runs `fw` again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use phitex_defuse::{Ctx, Engine, Field, Locality, Operation, Step, Stop, ValueId, Values};

struct Ops {
    /// The rounds' pairs `p` publishes, in turn (the last one held).
    pairs: Vec<(i64, i64)>,
    /// `fw`'s runs.
    fw_runs: AtomicU32,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if v.as_str(a[0]).as_deref() == Some("fw") { vec![(v.field("out"), Locality::Global)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let (n, out) = (v.field("n"), v.field("out"));
        let p0 = cx.path(&[Step::Index(0)]);
        let p1 = cx.path(&[Step::Index(1)]);
        match v.as_str(a[0]).as_deref() {
            Some("p") => {
                let x = cx.use_field(n)?;
                let i = self.pairs.iter().position(|&(a, b)| x == Some(v.list(vec![v.int(a), v.int(b)]))).map_or(0, |i| i + 1).min(self.pairs.len() - 1);
                let (a, b) = self.pairs[i];
                cx.publish(n, v.list(vec![v.int(a), v.int(b)]));
                Ok(ValueId::UNIT)
            }
            Some("fw") => {
                self.fw_runs.fetch_add(1, Relaxed);
                let flag = cx.loop_part(n, p1).and_then(|x| v.as_int(x)).unwrap_or(0);
                let loc = if flag == 0 { Locality::Global } else { Locality::Local };
                if !cx.define_forward_at(out, n, p0, Some(p1), loc) {
                    let x = cx.use_field(n)?;
                    if let Some(x) = x.and_then(|x| v.with_list(x, |l| l.first().copied())).flatten() {
                        cx.define_with(out, x, loc);
                    }
                }
                Ok(v.int(flag))
            }
            _ => Ok(cx.use_field(out)?.unwrap_or(ValueId::UNIT)),
        }
    }
}

fn run(pairs: Vec<(i64, i64)>, threads: bool) -> (Option<i64>, u32) {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { pairs, fw_runs: AtomicU32::new(0) });
    let op = eng.register(ops.clone());
    let n = eng.values().field("n");
    let init = eng.values().list(vec![eng.values().int(0), eng.values().int(0)]);
    eng.declare_loopback(n, init);
    let u = eng.new_unit();
    for s in ["p", "fw"] {
        let a = vec![eng.values().str(s)];
        eng.push(u, op, a);
    }
    let r = eng.push(u, op, vec![eng.values().str("r")]);
    if threads {
        eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut phitex_defuse::Sequential).expect("evaluate");
    } else {
        eng.evaluate_sequential().expect("evaluate");
    }
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    (eng.output(r).and_then(|y| eng.values().as_int(y)), ops.fw_runs.load(Relaxed))
}

#[test]
fn a_moved_value_follows_without_the_forwarder() {
    for threads in [false, true] {
        let (out, fw) = run(vec![(5, 0), (7, 0), (9, 0)], threads);
        assert_eq!(out, Some(9), "threads {threads}");
        assert_eq!(fw, 1, "threads {threads}: the forwarder ran again");
    }
}

#[test]
fn a_moved_kept_part_runs_the_forwarder_again() {
    for threads in [false, true] {
        let (out, fw) = run(vec![(5, 0), (5, 1)], threads);
        assert_eq!(out, Some(5), "threads {threads}");
        assert!(fw >= 2, "threads {threads}: the forwarder did not run again");
    }
}
