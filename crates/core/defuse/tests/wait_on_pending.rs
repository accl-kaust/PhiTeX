//! `Engine::wait_on_pending`: readers of the field wait on a node in work
//! whose pending definition set may hold it, even when predicting. Unit
//! [z, p, r_0 .. r_{N-1}]: `z` defines `q`; `p` (definitions pending, may
//! define `cat`) reads `q`, then defines `cat = 7`; each `r` reads `cat`.
//! Predicting, the `r`s read past `p` (the environment's `cat`) and run
//! again on its definition; waiting on it, they run once. The outputs are
//! the sequential ones.

use std::sync::Arc;

use phitex_defuse::{Ctx, Engine, Field, Locality, MayDefine, NodeId, Operation, Report, Stop, ValueId, Values};

const N: i64 = 10;

struct Ops;

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "z" => Some(vec![(v.field("q"), Locality::Global)]),
            "p" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        if tag(v, a) == "p" { MayDefine::Names(vec![v.field("cat")]) } else { MayDefine::Nothing }
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "r").then(|| vec![v.field("cat")])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "z" => {
                cx.define_with(v.field("q"), v.int(1), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.use_field(v.field("q"))?;
                cx.define_with(v.field("cat"), v.int(7), Locality::Global);
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_field(v.field("cat"))?.unwrap_or_else(|| v.int(-1))),
        }
    }
}

fn build(wait: bool, seq: bool) -> (Vec<Option<i64>>, Report) {
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops));
    if wait {
        let f = eng.values().field("cat");
        eng.wait_on_pending(f);
    }
    let (f, x) = (eng.values().field("cat"), eng.values().int(0));
    eng.set_environment([(f, x)]);
    let u = eng.new_unit();
    let mut rs: Vec<NodeId> = Vec::new();
    let z = vec![eng.values().str("z")];
    eng.push(u, op, z);
    let p = vec![eng.values().str("p")];
    eng.push(u, op, p);
    for i in 0..N {
        let r = vec![eng.values().str("r"), eng.values().int(i)];
        rs.push(eng.push(u, op, r));
    }
    let rep = if seq {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256), &mut phitex_defuse::Sequential)
    }
    .expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    (rs.iter().map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect(), rep)
}

#[test]
fn readers_wait_on_a_pending_definer_of_the_field() {
    let (want, _) = build(false, true);
    let (guess, guess_rep) = build(false, false);
    let (wait, wait_rep) = build(true, false);
    assert_eq!(want, vec![Some(7); N as usize]);
    assert_eq!(guess, want, "predicting");
    assert_eq!(wait, want, "waiting");
    eprintln!("runs predicting {} waiting {}", guess_rep.runs, wait_rep.runs);
    assert!(wait_rep.runs < guess_rep.runs, "runs predicting {} waiting {}", guess_rep.runs, wait_rep.runs);
    assert_eq!(wait_rep.runs, 2 + N as u64, "each once");
}

/// `Operation::likely_defines`: readers of a field waited on wait only on
/// the pending definers that likely define it. Unit [z, p, b_0 .. b_2,
/// r_0 .. r_{N-1}, l]: `p` as above (likely); each `b` (definitions
/// pending, may define `cat`, unlikely) waits on `l`'s `late` and defines
/// nothing; `l` reads `q` and defines `late`. Waiting on all, the `r`s
/// wait for the `b`s, which wait for `l`; on the likely only, they run on
/// `p`'s `cat` at once. The outputs are the sequential ones.
struct Body {
    unlikely: bool,
    l: std::sync::OnceLock<NodeId>,
}

impl Operation for Body {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        match tag(v, a).as_str() {
            "z" => Some(vec![(v.field("q"), Locality::Global)]),
            "l" => Some(vec![(v.field("late"), Locality::Global)]),
            "p" | "b" => None,
            _ => Some(Vec::new()),
        }
    }
    fn may_define(&self, a: &[ValueId], v: &Values) -> MayDefine {
        match tag(v, a).as_str() {
            "p" | "b" => MayDefine::Names(vec![v.field("cat")]),
            _ => MayDefine::Nothing,
        }
    }
    fn likely_defines(&self, a: &[ValueId], v: &Values, _f: Field) -> bool {
        !(self.unlikely && tag(v, a) == "b")
    }
    fn reads(&self, a: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        (tag(v, a) == "r").then(|| vec![v.field("cat")])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "z" => {
                cx.define_with(v.field("q"), v.int(1), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "l" => {
                cx.use_field(v.field("q"))?;
                cx.define_with(v.field("late"), v.int(1), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "p" => {
                cx.use_field(v.field("q"))?;
                cx.define_with(v.field("cat"), v.int(7), Locality::Global);
                Ok(ValueId::UNIT)
            }
            "b" => {
                cx.use_field_after_own(v.field("late"), *self.l.get().unwrap())?;
                Ok(ValueId::UNIT)
            }
            _ => Ok(cx.use_field(v.field("cat"))?.unwrap_or_else(|| v.int(-1))),
        }
    }
}

fn build_body(unlikely: bool, seq: bool) -> (Vec<Option<i64>>, Report) {
    let mut eng = Engine::new();
    let ops = Arc::new(Body {
        unlikely,
        l: std::sync::OnceLock::new(),
    });
    let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    let f = eng.values().field("cat");
    eng.wait_on_pending(f);
    let x = eng.values().int(0);
    eng.set_environment([(f, x)]);
    let u = eng.new_unit();
    for t in ["z", "p", "b", "b", "b"] {
        let a = vec![eng.values().str(t)];
        eng.push(u, op, a);
    }
    let mut rs: Vec<NodeId> = Vec::new();
    for i in 0..N {
        let r = vec![eng.values().str("r"), eng.values().int(i)];
        rs.push(eng.push(u, op, r));
    }
    let l = vec![eng.values().str("l")];
    let l = eng.push(u, op, l);
    ops.l.set(l).expect("once");
    let rep = if seq {
        eng.evaluate_sequential()
    } else {
        eng.evaluate(&mut phitex_defuse::Greedy::speculative(phitex_defuse::Speculation::Predicted, 256), &mut phitex_defuse::Sequential)
    }
    .expect("evaluate");
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    (rs.iter().map(|&x| eng.output(x).and_then(|y| eng.values().as_int(y))).collect(), rep)
}

#[test]
fn readers_wait_only_on_the_likely_definers() {
    let (want, _) = build_body(false, true);
    let (all, all_rep) = build_body(false, false);
    let (likely, likely_rep) = build_body(true, false);
    assert_eq!(want, vec![Some(7); N as usize]);
    assert_eq!(all, want, "waiting on all");
    assert_eq!(likely, want, "waiting on the likely");
    eprintln!("steps waiting on all {} on the likely {}; stops {} / {}", all_rep.steps, likely_rep.steps, all_rep.stops, likely_rep.stops);
    assert!(likely_rep.steps < all_rep.steps, "steps {} / {}", all_rep.steps, likely_rep.steps);
}
