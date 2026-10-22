//! Values passed on through views (`Ctx::define_from`,
//! `Ctx::contribute_from`, `Ctx::output_from`): the passing node depends
//! on whether the part it passes is there, not on its value, so a change
//! of the value is refiled without running it again; its readers see the
//! new value. Unit [d, pd, pw, ps, pc, po, readers…]: `d` defines `rec` (a
//! list); `pd` defines `x` as `rec[1]`, `pw` defines `y` as `rec`, `ps`
//! defines `rec` as itself (soft: no definition), `pc` contributes `rec[0]`
//! to `acc` (a sum), `po` outputs `rec[2]`, `pk` defines `z` and outputs
//! a composition `["h", rec[0], ["n", rec[2]]]`; the readers output `x`,
//! `y`, `rec`, `z`, and the fold of `acc`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Part, Step, Stop, SumMonoid, ValueId, Values};

struct Ops {
    runs: Arc<Mutex<HashMap<String, usize>>>,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match v.as_str(a[0]).as_deref() {
            Some("d" | "ps") => vec![(v.field("rec"), Locality::Local)],
            Some("pd") => vec![(v.field("x"), Locality::Local)],
            Some("pw") => vec![(v.field("y"), Locality::Local)],
            Some("pk") => vec![(v.field("z"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let tag = v.as_str(a[0]).unwrap();
        *self.runs.lock().unwrap().entry(tag.clone()).or_default() += 1;
        let (rec, x, y, acc) = (v.field("rec"), v.field("x"), v.field("y"), v.field("acc"));
        let none = || v.str("none");
        Ok(match tag.as_str() {
            "d" => {
                cx.define(rec, a[1]);
                ValueId::UNIT
            }
            "pd" => {
                if let Some(m) = cx.view(rec)?
                    && let Some(e) = cx.at(m, Step::Index(1))
                {
                    cx.define_from(x, e, Locality::Local);
                }
                ValueId::UNIT
            }
            "pw" => {
                if let Some(m) = cx.view(rec)? {
                    cx.define_from(y, m, Locality::Local);
                }
                ValueId::UNIT
            }
            "ps" => {
                if let Some(m) = cx.view(rec)? {
                    cx.define_from(rec, m, Locality::Local);
                }
                ValueId::UNIT
            }
            "pc" => {
                if let Some(m) = cx.view(rec)?
                    && let Some(e) = cx.at(m, Step::Index(0))
                {
                    cx.contribute_from(acc, e);
                }
                ValueId::UNIT
            }
            "po" => {
                if let Some(m) = cx.view(rec)?
                    && let Some(e) = cx.at(m, Step::Index(2))
                {
                    cx.output_from(e);
                }
                none()
            }
            "pk" => {
                let Some(m) = cx.view(rec)? else { return Ok(none()) };
                let (Some(e0), Some(e2)) = (cx.at(m, Step::Index(0)), cx.at(m, Step::Index(2))) else { return Ok(none()) };
                let inner = cx.compose(&[Part::Val(v.str("n")), Part::View(e2)]);
                let c = cx.compose(&[Part::Val(v.str("h")), Part::View(e0), Part::View(inner)]);
                cx.define_from(v.field("z"), c, Locality::Local);
                cx.output_from(c);
                none()
            }
            "rz" => cx.use_field(v.field("z"))?.unwrap_or_else(none),
            "rx" => cx.use_field(x)?.unwrap_or_else(none),
            "ry" => cx.use_field(y)?.unwrap_or_else(none),
            "rr" => cx.use_field(rec)?.unwrap_or_else(none),
            "ra" => cx.acc_fold(acc, None, Some(cx.node()))?,
            _ => ValueId::UNIT,
        })
    }
}

const NODES: [&str; 12] = ["d", "pd", "pw", "ps", "pc", "po", "pk", "rx", "ry", "rr", "ra", "rz"];

struct T {
    eng: Engine,
    op: defuse::OpId,
    runs: Arc<Mutex<HashMap<String, usize>>>,
    nodes: HashMap<&'static str, NodeId>,
    rec: Vec<i64>,
}

fn engine(rec: &[i64]) -> (Engine, defuse::OpId, Arc<Mutex<HashMap<String, usize>>>, HashMap<&'static str, NodeId>) {
    let runs = Arc::new(Mutex::new(HashMap::new()));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { runs: runs.clone() }));
    let acc = eng.values().field("acc");
    eng.declare_accumulator(acc, Arc::new(SumMonoid));
    let u = eng.new_unit();
    let mut nodes = HashMap::new();
    for k in NODES {
        let v = eng.values();
        let mut a = vec![v.str(k)];
        if k == "d" {
            a.push(v.list(rec.iter().map(|&x| v.int(x)).collect()));
        }
        nodes.insert(k, eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("evaluate");
    (eng, op, runs, nodes)
}

impl T {
    fn new(rec: &[i64]) -> T {
        let (eng, op, runs, nodes) = engine(rec);
        T { eng, op, runs, nodes, rec: rec.to_vec() }
    }

    /// Replaces `d`'s list, evaluates, checks every output against a fresh
    /// engine's, and returns the nodes that ran.
    fn edit(&mut self, rec: &[i64]) -> Vec<&'static str> {
        let before = self.runs.lock().unwrap().clone();
        let v = self.eng.values();
        let a = vec![v.str("d"), v.list(rec.iter().map(|&x| v.int(x)).collect())];
        self.eng.replace(self.nodes["d"], self.op, a);
        self.eng.evaluate_sequential().expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
        self.rec = rec.to_vec();
        let (fresh, _, _, fnodes) = engine(rec);
        for k in NODES {
            assert_eq!(self.out(k), fresh.values().render(fresh.output(fnodes[k]).unwrap()), "{k} after {rec:?}");
        }
        let after = self.runs.lock().unwrap().clone();
        NODES.iter().copied().filter(|r| after.get(*r) != before.get(*r)).collect()
    }

    fn out(&self, r: &str) -> String {
        self.eng.values().render(self.eng.output(self.nodes[r]).unwrap())
    }
}

#[test]
fn a_passed_value_follows_without_its_passer_running() {
    let mut t = T::new(&[1, 2, 3]);
    assert_eq!((t.out("rx"), t.out("ry"), t.out("rr"), t.out("ra"), t.out("po")), ("2".into(), "[1 2 3]".into(), "[1 2 3]".into(), "1".into(), "3".into()));
    // Element 1: `x` and `y` follow; only their readers and `rec`'s run.
    assert_eq!(t.edit(&[1, 5, 3]), vec!["d", "rx", "ry", "rr"]);
    assert_eq!(t.out("rx"), "5");
    // Element 0: the contribution follows; the fold's reader runs.
    assert_eq!(t.edit(&[7, 5, 3]), vec!["d", "ry", "rr", "ra", "rz"]);
    assert_eq!(t.out("ra"), "7");
    // Element 2: the output follows.
    assert_eq!(t.edit(&[7, 5, 4]), vec!["d", "ry", "rr", "rz"]);
    assert_eq!(t.out("po"), "4");
}

#[test]
fn a_passer_runs_again_when_its_part_comes_or_goes() {
    let mut t = T::new(&[1, 2, 3]);
    // Elements 1 and 2 gone: their passers run.
    let ran = t.edit(&[1]);
    assert!(ran.contains(&"pd") && ran.contains(&"po") && !ran.contains(&"pc") && !ran.contains(&"pw"), "{ran:?}");
    assert_eq!(t.out("rx"), "\"none\"");
    // Back.
    let ran = t.edit(&[1, 8, 9]);
    assert!(ran.contains(&"pd") && ran.contains(&"po"), "{ran:?}");
    assert_eq!((t.out("rx"), t.out("po")), ("8".into(), "9".into()));
    // Then passed again.
    assert_eq!(t.edit(&[1, 6, 9]), vec!["d", "rx", "ry", "rr"]);
}

#[test]
fn passes_refiled_are_counted() {
    let mut t = T::new(&[1, 2, 3]);
    t.edit(&[1, 5, 3]);
    let v = t.eng.values();
    let a = vec![v.str("d"), v.list(vec![v.int(4), v.int(5), v.int(6)])];
    t.eng.replace(t.nodes["d"], t.op, a);
    let rep = t.eng.evaluate_sequential().expect("evaluate");
    // (`pw`, `pc`, `po` and `pk`: `pd`'s element held.)
    assert_eq!(rep.passes_refiled, 4);
}

#[test]
fn a_composition_follows_its_holes_without_its_composer_running() {
    let mut t = T::new(&[1, 2, 3]);
    assert_eq!((t.out("pk"), t.out("rz")), ("[\"h\" 1 [\"n\" 3]]".into(), "[\"h\" 1 [\"n\" 3]]".into()));
    // Element 2, nested: the composition is made again; its reader runs.
    let ran = t.edit(&[1, 2, 7]);
    assert!(!ran.contains(&"pk") && ran.contains(&"rz"), "{ran:?}");
    assert_eq!(t.out("pk"), "[\"h\" 1 [\"n\" 7]]");
    // Element 1, in none of its holes: `z` stands.
    let ran = t.edit(&[1, 9, 7]);
    assert!(!ran.contains(&"pk") && !ran.contains(&"rz"), "{ran:?}");
    // Element 2 gone: the composer runs.
    let ran = t.edit(&[1, 9]);
    assert!(ran.contains(&"pk"), "{ran:?}");
    assert_eq!(t.out("rz"), "\"none\"");
}
