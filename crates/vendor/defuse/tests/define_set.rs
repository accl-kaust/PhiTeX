//! A record's cell defined (`Ctx::define_set`): the setter depends on the
//! cell being there, not on the rest of the record, so a change elsewhere
//! in the record is refiled without running it again. Unit [d, ds, dn,
//! rr]: `d` defines `rec` (a list whose element 2 is an association list);
//! `ds` defines `rec` as itself with element 1 set to its argument; `dn`
//! defines `rec` as itself with key "b" of element 2 set to its argument;
//! `rr` outputs `rec`. The setters output whether they set.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Step, Stop, ValueId, Values};

struct Ops {
    runs: Arc<Mutex<HashMap<String, usize>>>,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match v.as_str(a[0]).as_deref() {
            Some("d" | "ds" | "dn") => vec![(v.field("rec"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let tag = v.as_str(a[0]).unwrap();
        *self.runs.lock().unwrap().entry(tag.clone()).or_default() += 1;
        let rec = v.field("rec");
        let set = |cx: &mut Ctx<'_>, steps: &[Step]| -> Result<ValueId, Stop> {
            let p = cx.path(steps);
            let Some(m) = cx.view(rec)? else { return Ok(v.str("none")) };
            Ok(v.int(i64::from(cx.define_set(rec, m, p, a[1], Locality::Local))))
        };
        match tag.as_str() {
            "d" => {
                cx.define(rec, a[1]);
                Ok(ValueId::UNIT)
            }
            "ds" => set(cx, &[Step::Index(1)]),
            "dn" => set(cx, &[Step::Index(2), Step::Key(v.str("b"))]),
            "rr" => Ok(cx.use_field(rec)?.unwrap_or_else(|| v.str("none"))),
            _ => Ok(ValueId::UNIT),
        }
    }
}

const NODES: [&str; 4] = ["d", "ds", "dn", "rr"];

/// `d`'s record: `[x0, x1, [["a" a] ["b" b]]]`, its parts left out from
/// the end as `xs` is shorter; `b` `None`: no key "b".
#[derive(Clone, Copy)]
struct Rec {
    xs: &'static [i64],
    b: Option<i64>,
}

fn rec_value(v: &Values, r: Rec) -> ValueId {
    let mut items: Vec<ValueId> = r.xs.iter().take(2).map(|&x| v.int(x)).collect();
    if r.xs.len() > 2 {
        let mut m = vec![v.list(vec![v.str("a"), v.int(r.xs[2])])];
        if let Some(b) = r.b {
            m.push(v.list(vec![v.str("b"), v.int(b)]));
        }
        items.push(v.list(m));
    }
    v.list(items)
}

fn args(v: &Values, k: &str, r: Rec, s: i64, n: i64) -> Vec<ValueId> {
    match k {
        "d" => vec![v.str(k), rec_value(v, r)],
        "ds" => vec![v.str(k), v.int(s)],
        "dn" => vec![v.str(k), v.int(n)],
        _ => vec![v.str(k)],
    }
}

struct T {
    eng: Engine,
    op: defuse::OpId,
    runs: Arc<Mutex<HashMap<String, usize>>>,
    nodes: HashMap<&'static str, NodeId>,
    st: (Rec, i64, i64),
}

fn engine(r: Rec, s: i64, n: i64) -> (Engine, defuse::OpId, Arc<Mutex<HashMap<String, usize>>>, HashMap<&'static str, NodeId>) {
    let runs = Arc::new(Mutex::new(HashMap::new()));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Ops { runs: runs.clone() }));
    let u = eng.new_unit();
    let mut nodes = HashMap::new();
    for k in NODES {
        let a = args(eng.values(), k, r, s, n);
        nodes.insert(k, eng.push(u, op, a));
    }
    eng.evaluate_sequential().expect("evaluate");
    (eng, op, runs, nodes)
}

impl T {
    fn new(r: Rec) -> T {
        let (eng, op, runs, nodes) = engine(r, 10, 20);
        T { eng, op, runs, nodes, st: (r, 10, 20) }
    }

    /// Sets the record and the setters' values, evaluates, checks every
    /// output against a fresh engine's, and returns the nodes that ran.
    fn edit(&mut self, r: Rec, s: i64, n: i64) -> Vec<&'static str> {
        let before = self.runs.lock().unwrap().clone();
        for (k, ch) in [("d", true), ("ds", s != self.st.1), ("dn", n != self.st.2)] {
            if ch {
                let a = args(self.eng.values(), k, r, s, n);
                self.eng.replace(self.nodes[k], self.op, a);
            }
        }
        self.st = (r, s, n);
        self.eng.evaluate_sequential().expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
        let (fresh, _, _, fnodes) = engine(r, s, n);
        for k in NODES {
            assert_eq!(self.out(k), fresh.values().render(fresh.output(fnodes[k]).unwrap()), "{k} after {:?} {s} {n}", r.xs);
        }
        let after = self.runs.lock().unwrap().clone();
        NODES.iter().copied().filter(|k| after.get(*k) != before.get(*k)).collect()
    }

    fn out(&self, k: &str) -> String {
        self.eng.values().render(self.eng.output(self.nodes[k]).unwrap())
    }
}

#[test]
fn a_cell_set_follows_the_rest_of_its_record_without_its_setter_running() {
    let mut t = T::new(Rec { xs: &[1, 2, 3], b: Some(4) });
    assert_eq!((t.out("rr"), t.out("ds"), t.out("dn")), ("[1 10 [[\"a\" 3] [\"b\" 20]]]".into(), "1".into(), "1".into()));
    // Element 0: both setters' records follow; only the reader runs.
    assert_eq!(t.edit(Rec { xs: &[5, 2, 3], b: Some(4) }, 10, 20), vec!["d", "rr"]);
    assert_eq!(t.out("rr"), "[5 10 [[\"a\" 3] [\"b\" 20]]]");
    // Key "a", beside the nested cell: the same.
    assert_eq!(t.edit(Rec { xs: &[5, 2, 7], b: Some(4) }, 10, 20), vec!["d", "rr"]);
    // The set cells themselves: what is set stands; nothing reads anew.
    assert_eq!(t.edit(Rec { xs: &[5, 9, 7], b: Some(8) }, 10, 20), vec!["d"]);
    // A setter's own value: it runs.
    let ran = t.edit(Rec { xs: &[5, 9, 7], b: Some(8) }, 11, 20);
    assert!(ran.contains(&"ds") && !ran.contains(&"dn") && ran.contains(&"rr"), "{ran:?}");
}

#[test]
fn a_setter_runs_again_when_its_cell_comes_or_goes() {
    let mut t = T::new(Rec { xs: &[1, 2, 3], b: Some(4) });
    // Key "b" gone: `dn` runs and sets nothing.
    let ran = t.edit(Rec { xs: &[1, 2, 3], b: None }, 10, 20);
    assert!(ran.contains(&"dn") && !ran.contains(&"ds"), "{ran:?}");
    assert_eq!((t.out("dn"), t.out("rr")), ("0".into(), "[1 10 [[\"a\" 3]]]".into()));
    // Elements 1 and 2 gone: both run.
    let ran = t.edit(Rec { xs: &[1], b: None }, 10, 20);
    assert!(ran.contains(&"dn") && ran.contains(&"ds"), "{ran:?}");
    assert_eq!(t.out("rr"), "[1]");
    // Back, then refiled again.
    t.edit(Rec { xs: &[1, 2, 3], b: Some(4) }, 10, 20);
    assert_eq!(t.out("rr"), "[1 10 [[\"a\" 3] [\"b\" 20]]]");
    assert_eq!(t.edit(Rec { xs: &[6, 2, 3], b: Some(4) }, 10, 20), vec!["d", "rr"]);
}
