//! Read paths (`Ctx::use_field_path`, `Ctx::acc_fold_path`, views): a run
//! depends on what it inspected of a value, so a change elsewhere in the
//! value leaves it standing, and a change to what it inspected runs it
//! again. Unit [d, m, c1, c2, readers…]: `d` defines `rec` (a list), `m`
//! defines `map` (an association list), `c1`, `c2` contribute to `acc` (a
//! sequence); each reader inspects a part and outputs what it saw.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, SeqMonoid, Step, Stop, ValueId, Values};

struct Ops {
    runs: Arc<Mutex<HashMap<String, usize>>>,
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match v.as_str(a[0]).as_deref() {
            Some("d") => vec![(v.field("rec"), Locality::Local)],
            Some("m") => vec![(v.field("map"), Locality::Local)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let tag = v.as_str(a[0]).unwrap();
        *self.runs.lock().unwrap().entry(tag.clone()).or_default() += 1;
        let (rec, map, acc) = (v.field("rec"), v.field("map"), v.field("acc"));
        let none = || v.str("none");
        Ok(match tag.as_str() {
            "d" => {
                cx.define(rec, a[1]);
                ValueId::UNIT
            }
            "m" => {
                cx.define(map, a[1]);
                ValueId::UNIT
            }
            t if t.starts_with('c') => {
                cx.contribute(acc, v.seq_unit(a[1]));
                ValueId::UNIT
            }
            "r1" => {
                let p = cx.path(&[Step::Index(1)]);
                cx.use_field_path(rec, p)?.unwrap_or_else(none)
            }
            "rk" => {
                let p = cx.path(&[Step::Key(v.str("k"))]);
                cx.use_field_path(map, p)?.unwrap_or_else(none)
            }
            "rv" => match cx.view(rec)? {
                None => none(),
                Some(m) => {
                    let n = cx.len(m).unwrap_or(0);
                    let e0 = cx.get(m, Step::Index(0)).unwrap_or_else(none);
                    v.list(vec![v.int(n as i64), e0])
                }
            },
            "rw" => match cx.view(rec)? {
                None => none(),
                Some(m) => cx.whole(m),
            },
            "rf" => {
                let p = cx.path(&[Step::Len]);
                cx.acc_fold_path(acc, None, Some(cx.node()), p)?.unwrap_or_else(none)
            }
            "re" => {
                let len = cx.path(&[Step::Len]);
                let p = cx.path(&[Step::Each(len)]);
                let x = cx.acc_fold_path(acc, None, Some(cx.node()), p)?.unwrap_or_else(none);
                v.list(v.seq_to_vec(x))
            }
            "rfv" => {
                let m = cx.acc_view(acc, None, Some(cx.node()))?;
                cx.get(m, Step::Last).unwrap_or_else(none)
            }
            _ => ValueId::UNIT,
        })
    }
}

const READERS: [&str; 7] = ["r1", "rk", "rv", "rw", "rf", "rfv", "re"];

struct T {
    eng: Engine,
    op: defuse::OpId,
    runs: Arc<Mutex<HashMap<String, usize>>>,
    nodes: HashMap<&'static str, NodeId>,
}

impl T {
    fn new() -> T {
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let mut eng = Engine::new();
        let op = eng.register(Arc::new(Ops { runs: runs.clone() }));
        let acc = eng.values().field("acc");
        eng.declare_accumulator(acc, Arc::new(SeqMonoid));
        let u = eng.new_unit();
        let mut nodes = HashMap::new();
        let v = eng.values();
        let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
        let pair = |k: &str, x: i64| v.list(vec![v.str(k), v.int(x)]);
        let texts: Vec<(&'static str, Vec<ValueId>)> = vec![
            ("d", vec![v.str("d"), ints(&[1, 2, 3])]),
            ("m", vec![v.str("m"), v.list(vec![pair("k", 1), pair("j", 2)])]),
            ("c1", vec![v.str("c1"), v.str("x")]),
            ("c2", vec![v.str("c2"), v.str("y")]),
        ];
        for (k, a) in texts {
            nodes.insert(k, eng.push(u, op, a));
        }
        for r in READERS {
            let a = vec![eng.values().str(r)];
            nodes.insert(r, eng.push(u, op, a));
        }
        let mut t = T { eng, op, runs, nodes };
        t.eval();
        t
    }

    fn eval(&mut self) {
        self.eng.evaluate_sequential().expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
    }

    /// Replaces node `k`'s argument, evaluates, and returns the readers
    /// that ran.
    fn edit(&mut self, k: &'static str, x: ValueId) -> Vec<&'static str> {
        let before = self.runs.lock().unwrap().clone();
        let a = vec![self.eng.values().str(k), x];
        self.eng.replace(self.nodes[k], self.op, a);
        self.eval();
        let after = self.runs.lock().unwrap().clone();
        READERS.iter().copied().filter(|r| after.get(*r) != before.get(*r)).collect()
    }

    fn out(&self, r: &str) -> String {
        self.eng.values().render(self.eng.output(self.nodes[r]).unwrap())
    }
}

#[test]
fn a_path_reader_runs_again_only_when_its_part_changes() {
    let mut t = T::new();
    assert_eq!((t.out("r1"), t.out("rv"), t.out("rw")), ("2".into(), "[3 1]".into(), "[1 2 3]".into()));
    let v = t.eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    // Element 0: the view's first element and the whole read see it.
    assert_eq!(t.edit("d", ints(&[9, 2, 3])), vec!["rv", "rw"]);
    assert_eq!(t.out("rv"), "[3 9]");
    // A fourth element: the length.
    let v = t.eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    assert_eq!(t.edit("d", ints(&[9, 2, 3, 4])), vec!["rv", "rw"]);
    assert_eq!(t.out("rv"), "[4 9]");
    // Element 2: only the whole read.
    let v = t.eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    assert_eq!(t.edit("d", ints(&[9, 2, 7, 4])), vec!["rw"]);
    // Element 1: the path reader.
    let v = t.eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    assert_eq!(t.edit("d", ints(&[9, 5, 7, 4])), vec!["r1", "rw"]);
    assert_eq!(t.out("r1"), "5");
    // Too short for element 1: none.
    let v = t.eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    assert_eq!(t.edit("d", ints(&[9])), vec!["r1", "rv", "rw"]);
    assert_eq!(t.out("r1"), "\"none\"");
}

#[test]
fn a_key_reader_runs_again_only_when_its_key_changes() {
    let mut t = T::new();
    assert_eq!(t.out("rk"), "1");
    let v = t.eng.values();
    let pair = |k: &str, x: i64| v.list(vec![v.str(k), v.int(x)]);
    assert_eq!(t.edit("m", v.list(vec![pair("k", 1), pair("j", 5)])), Vec::<&str>::new());
    let v = t.eng.values();
    let pair = |k: &str, x: i64| v.list(vec![v.str(k), v.int(x)]);
    assert_eq!(t.edit("m", v.list(vec![pair("j", 5), pair("k", 1)])), Vec::<&str>::new());
    let v = t.eng.values();
    let pair = |k: &str, x: i64| v.list(vec![v.str(k), v.int(x)]);
    assert_eq!(t.edit("m", v.list(vec![pair("j", 5), pair("k", 8)])), vec!["rk"]);
    assert_eq!(t.out("rk"), "8");
    let v = t.eng.values();
    let pair = |k: &str, x: i64| v.list(vec![v.str(k), v.int(x)]);
    assert_eq!(t.edit("m", v.list(vec![pair("j", 5)])), vec!["rk"]);
    assert_eq!(t.out("rk"), "\"none\"");
}

#[test]
fn a_fold_path_reader_runs_again_only_when_its_part_changes() {
    let mut t = T::new();
    assert_eq!((t.out("rf"), t.out("rfv"), t.out("re")), ("2".into(), "\"y\"".into(), "[1 1]".into()));
    // The first contribution's content: neither the length, the last, nor
    // each one's length.
    let x = t.eng.values().str("z");
    assert_eq!(t.edit("c1", x), Vec::<&str>::new());
    // Its length: each one's length.
    let x = t.eng.values().str("zz");
    assert_eq!(t.edit("c1", x), vec!["re"]);
    assert_eq!(t.out("re"), "[2 1]");
    // The last one's: the last.
    let x = t.eng.values().str("w");
    assert_eq!(t.edit("c2", x), vec!["rfv"]);
    assert_eq!(t.out("rfv"), "\"w\"");
    // One gone: the length (and not the last, still `w`).
    let c1 = t.nodes["c1"];
    t.eng.remove(c1);
    let before = t.runs.lock().unwrap().clone();
    t.eval();
    let after = t.runs.lock().unwrap().clone();
    let ran: Vec<&str> = READERS.iter().copied().filter(|r| after.get(*r) != before.get(*r)).collect();
    assert_eq!(ran, vec!["rf", "re"]);
    assert_eq!((t.out("rf"), t.out("re")), ("1".into(), "[1]".into()));
}

/// A view held in a parked token stands when the run resumes: `p` views
/// `rec`, waits for its child's output with the view in its token, and
/// then reads element 1 through it. An edit of element 0 leaves it
/// standing; of element 1, runs it again.
struct Parks {
    runs: Arc<Mutex<usize>>,
}

impl Operation for Parks {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if v.as_str(a[0]).as_deref() == Some("d") { vec![(v.field("rec"), Locality::Local)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match v.as_str(a[0]).as_deref() {
            Some("d") => {
                cx.define(v.field("rec"), a[1]);
                Ok(ValueId::UNIT)
            }
            Some("kid") => Ok(v.int(7)),
            _ => {
                *self.runs.lock().unwrap() += 1;
                let m = match cx.parked().and_then(|t| t.downcast::<defuse::View>().ok()) {
                    Some(m) => *m,
                    None => match cx.view(v.field("rec"))? {
                        Some(m) => m,
                        None => return Ok(v.str("none")),
                    },
                };
                let k = cx.child(cx.op(), vec![v.str("kid")]);
                let x = match cx.output_of(k) {
                    Ok(x) => x,
                    Err(s) => {
                        cx.park(Box::new(m));
                        return Err(s);
                    }
                };
                let e1 = cx.get(m, Step::Index(1)).unwrap_or_else(|| v.str("none"));
                Ok(v.list(vec![x, e1]))
            }
        }
    }
}

#[test]
fn a_view_held_across_a_park_stands() {
    let runs = Arc::new(Mutex::new(0));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Parks { runs: runs.clone() }));
    let u = eng.new_unit();
    let v = eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    let da = vec![v.str("d"), ints(&[1, 2, 3])];
    let d = eng.push(u, op, da);
    let p = vec![eng.values().str("p")];
    let p = eng.push(u, op, p);
    let rep = eng.evaluate_sequential().unwrap();
    assert!(rep.resumed >= 1, "{rep:?}");
    assert_eq!(eng.values().render(eng.output(p).unwrap()), "[7 2]");
    assert!(eng.check_reads().is_empty());
    let n = *runs.lock().unwrap();
    let v = eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    let da = vec![v.str("d"), ints(&[9, 2, 3])];
    eng.replace(d, op, da);
    eng.evaluate_sequential().unwrap();
    assert_eq!(*runs.lock().unwrap(), n, "element 0 ran it again");
    let v = eng.values();
    let ints = |xs: &[i64]| v.list(xs.iter().map(|&x| v.int(x)).collect());
    let da = vec![v.str("d"), ints(&[9, 5, 3])];
    eng.replace(d, op, da);
    eng.evaluate_sequential().unwrap();
    assert!(*runs.lock().unwrap() > n);
    assert_eq!(eng.values().render(eng.output(p).unwrap()), "[7 5]");
    assert!(eng.check_reads().is_empty());
}

/// `Each` on a deep sequence (made by appends, one element at a time) is
/// made without recursion.
#[test]
fn each_on_a_deep_sequence() {
    struct Deep;
    impl Operation for Deep {
        fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(if v.as_str(a[0]).as_deref() == Some("d") { vec![(v.field("s"), Locality::Local)] } else { Vec::new() })
        }
        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            if v.as_str(cx.args()[0]).as_deref() == Some("d") {
                let mut s = ValueId::NIL;
                for i in 0..200_000 {
                    s = v.seq_concat(s, v.seq_unit(v.int(i % 7)));
                }
                cx.define(v.field("s"), s);
                return Ok(ValueId::UNIT);
            }
            let present = cx.path(&[Step::Present]);
            let p = cx.path(&[Step::Each(present)]);
            let x = cx.use_field_path(v.field("s"), p)?.unwrap();
            Ok(v.int(v.seq_len(x) as i64))
        }
    }
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Deep));
    let u = eng.new_unit();
    let d = vec![eng.values().str("d")];
    eng.push(u, op, d);
    let r = vec![eng.values().str("r")];
    let r = eng.push(u, op, r);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.output(r).and_then(|x| eng.values().as_int(x)), Some(200_000));
}

/// A run that only compares a value depends only on the comparison
/// (`Step::Eq`, `Step::Cmp`): "is this the last page" stands while the
/// count moves past it.
#[test]
fn a_comparison_reader_runs_again_only_when_the_comparison_changes() {
    struct Cmp {
        runs: Arc<Mutex<usize>>,
    }
    impl Operation for Cmp {
        fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
            Some(if v.as_str(a[0]).as_deref() == Some("d") { vec![(v.field("last"), Locality::Local)] } else { Vec::new() })
        }
        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let a = cx.args();
            if v.as_str(a[0]).as_deref() == Some("d") {
                cx.define(v.field("last"), a[1]);
                return Ok(ValueId::UNIT);
            }
            *self.runs.lock().unwrap() += 1;
            let mine = v.int(3);
            let eq = cx.path(&[Step::Eq(mine)]);
            let cmp = cx.path(&[Step::Cmp(mine)]);
            let is_last = cx.use_field_path(v.field("last"), eq)?.unwrap();
            let order = cx.use_field_path(v.field("last"), cmp)?.unwrap();
            Ok(v.list(vec![is_last, order]))
        }
    }
    let runs = Arc::new(Mutex::new(0));
    let mut eng = Engine::new();
    let op = eng.register(Arc::new(Cmp { runs: runs.clone() }));
    let u = eng.new_unit();
    let da = vec![eng.values().str("d"), eng.values().int(44)];
    let d = eng.push(u, op, da);
    let p = vec![eng.values().str("p")];
    let p = eng.push(u, op, p);
    eng.evaluate_sequential().unwrap();
    assert_eq!(eng.values().render(eng.output(p).unwrap()), "[false 1]");
    for (n, ran, out) in [(45, false, "[false 1]"), (3, true, "[true 0]"), (2, true, "[false -1]"), (1, false, "[false -1]")] {
        let before = *runs.lock().unwrap();
        let da = vec![eng.values().str("d"), eng.values().int(n)];
        eng.replace(d, op, da);
        eng.evaluate_sequential().unwrap();
        assert_eq!(*runs.lock().unwrap() > before, ran, "to {n}");
        assert_eq!(eng.values().render(eng.output(p).unwrap()), out);
    }
}
