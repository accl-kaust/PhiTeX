//! A reader that declares its reads (gated) keeps its result when the
//! definition it read comes back equal: `P [v]` defines `x = v`; `F` reads
//! `x`, defines `x = 7`; `G` (reads `[x]`) reads `x`. Editing `P` re-runs
//! `F` (or re-makes it, `remade`), which defines `x = 7` again.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    me: Mutex<Option<OpId>>,
    remade: bool,
    g_runs: AtomicU64,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "p" | "m" | "f" => vec![(v.field("x"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        Some(match tag(v, a).as_str() {
            "m" | "f" | "g" => vec![v.field("x")],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = self.me.lock().unwrap().unwrap();
        let x = v.field("x");
        match tag(v, &a).as_str() {
            "p" => {
                cx.define(x, a[1]);
                cx.follow(op, vec![v.str("m")]);
                Ok(ValueId::UNIT)
            }
            "m" => {
                let got = cx.use_field(x)?.and_then(|p| v.as_int(p)).unwrap_or(0);
                cx.define(x, v.int(got + 100));
                cx.follow(
                    op,
                    if self.remade {
                        vec![v.str("f"), v.int(got)]
                    } else {
                        vec![v.str("f")]
                    },
                );
                Ok(ValueId::UNIT)
            }
            "f" => {
                let _ = cx.use_field(x)?;
                cx.define(x, v.int(7));
                Ok(ValueId::UNIT)
            }
            _ => {
                self.g_runs.fetch_add(1, Relaxed);
                Ok(cx.use_field(x)?.unwrap_or(ValueId::UNIT))
            }
        }
    }
}

fn case(remade: bool) {
    let ops = Arc::new(Ops {
        me: Mutex::new(None),
        remade,
        g_runs: AtomicU64::new(0),
    });
    let mut eng = Engine::new();
    let op = eng.register(ops.clone());
    *ops.me.lock().unwrap() = Some(op);
    let u = eng.new_unit();
    let (p, one, g) = (eng.values().str("p"), eng.values().int(1), eng.values().str("g"));
    let pn = eng.push(u, op, vec![p, one]);
    let gn = eng.push(u, op, vec![g]);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(gn), Some(eng.values().int(7)));
    let before = ops.g_runs.load(Relaxed);
    let two = eng.values().int(2);
    eng.replace(pn, op, vec![p, two]);
    let rep = eng.evaluate_sequential().unwrap();
    assert_eq!(
        ops.g_runs.load(Relaxed),
        before,
        "remade {remade}: G stands: {:?}",
        rep.rerun_causes
    );
}

#[test]
fn a_gated_reader_keeps_an_equal_redefinition() {
    case(false);
}

#[test]
fn a_gated_reader_keeps_an_equal_redefinition_from_a_remade_node() {
    case(true);
}
