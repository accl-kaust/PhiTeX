//! The light trace (`DEFUSE_LTRACE`): committed runs with their range
//! reads and named definition reads, contribution changes and touches,
//! in sequence order, appended per evaluation.

use std::sync::Arc;

use defuse::{Ctx, Engine, Field, Locality, Monoid, Operation, Stop, ValueId, Values};

struct Sum;

impl Monoid for Sum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }
    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        v.int(v.as_int(a).unwrap_or(0) + v.as_int(b).unwrap_or(0))
    }
}

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "e" {
            vec![(v.field("d"), Locality::Local)]
        } else {
            Vec::new()
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "e" {
            cx.contribute(v.field("acc"), a[1]);
            cx.define(v.field("d"), a[1]);
            return Ok(ValueId::UNIT);
        }
        let d = cx.use_field(v.field("d"))?.unwrap_or(ValueId::UNIT);
        let s = cx.acc_range(v.field("acc"), None)?;
        Ok(v.list(vec![d, s]))
    }
}

#[test]
fn the_light_trace_records_reads_contributions_and_touches() {
    let dir = std::env::temp_dir().join(format!("defuse-ltrace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("trace.txt");
    // SAFETY: set before any thread of this test process reads it.
    unsafe {
        std::env::set_var("DEFUSE_LTRACE", &path);
        std::env::set_var("DEFUSE_LTRACE_FIELDS", "d");
    }
    let mut eng = Engine::new();
    eng.declare_accumulator(eng.values().field("acc"), Arc::new(Sum));
    let op = eng.register(Arc::new(Ops));
    let u = eng.new_unit();
    let (e, k) = (eng.values().str("e"), eng.values().int(3));
    let en = eng.push(u, op, vec![e, k]);
    let r = eng.values().str("r");
    eng.push(u, op, vec![r]);
    eng.evaluate_sequential().expect("cold");
    let k = eng.values().int(4);
    eng.replace(en, op, vec![e, k]);
    eng.evaluate_sequential().expect("edit");
    let text = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.iter().filter(|l| l.ends_with(" E")).count(), 2, "{text}");
    assert!(lines.iter().any(|l| l.contains(" K ") && l.contains(" acc ")), "{text}");
    assert!(lines.iter().any(|l| l.trim_start().starts_with("R acc")), "{text}");
    assert!(lines.iter().any(|l| l.trim_start().starts_with("D d ")), "{text}");
    assert!(lines.iter().any(|l| l.contains(" T ")), "{text}");
    // In sequence order.
    let seqs: Vec<u64> = lines
        .iter()
        .filter(|l| !l.starts_with(' '))
        .map(|l| l.split(' ').next().unwrap().parse().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{text}");
}
