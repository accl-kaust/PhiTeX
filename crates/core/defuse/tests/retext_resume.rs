//! A retexted node resumes at a mark of its last run (`Ctx::resumed_retext`,
//! `Ctx::resume_at`). Unit [s, r]: `s` defines `b`; `r`'s arguments are a
//! list of items and a flag; it sums the items in order, marking after
//! each (its state, the sum so far, kept per mark), reads `b` after its
//! second item, and outputs the sum plus `b` (times the flag). Retexted, it
//! resumes at the last mark the old and new items agree up to, when the
//! flag is the same; else it runs from the start.

use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, Operation, Stop, ValueId, Values};

struct Ops {
    /// For each run of `r`: the mark it resumed at and the items it summed.
    runs: Mutex<Vec<(Option<u32>, usize)>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn ints(v: &Values, x: ValueId) -> Vec<i64> {
    match &*v.get(x) {
        phitex_defuse::Value::List(l) => l.iter().map(|&e| v.as_int(e).unwrap()).collect(),
        _ => Vec::new(),
    }
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(if tag(v, a) == "s" { vec![(v.field("b"), Locality::Global)] } else { Vec::new() })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        if tag(v, &a) == "s" {
            cx.define_with(v.field("b"), a[1], Locality::Global);
            return Ok(ValueId::UNIT);
        }
        let items = ints(v, a[1]);
        // Resume where the old items agree with the new, the flag the same.
        let mut start = 0usize;
        let mut sums: Vec<i64> = Vec::new();
        let mut at = None;
        if let Some((tok, kmax, old)) = cx.resumed_retext() {
            let shared = ints(v, old[1]).iter().zip(&items).take_while(|(x, y)| x == y).count();
            let k = (kmax as usize).min(shared);
            if old[2] == a[2] && k > 0 && cx.resume_at(k as u32) {
                let kept = tok.downcast_ref::<Vec<i64>>().expect("sums");
                sums = kept[..k].to_vec();
                start = k;
                at = Some(k as u32);
            }
        }
        self.runs.lock().unwrap().push((at, 0));
        let mut sum = sums.last().copied().unwrap_or(0);
        let mut b = 0;
        for (i, &x) in items.iter().enumerate().skip(start) {
            self.runs.lock().unwrap().last_mut().expect("this run").1 += 1;
            sum += x;
            if i == 1 {
                b = cx.use_field(v.field("b"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
            }
            sums.push(sum);
            cx.checkpoint((i + 1) as u32, v.int(sum))?;
        }
        if start > 1 {
            // (Resumed past the read: as it read.)
            b = cx.use_field(v.field("b"))?.and_then(|y| v.as_int(y)).unwrap_or(0);
        }
        cx.keep_after(Arc::new(sums));
        Ok(v.int(sum + b * v.as_int(a[2]).unwrap()))
    }
}

fn args(v: &Values, items: &[i64], flag: i64) -> Vec<ValueId> {
    vec![v.str("r"), v.list(items.iter().map(|&x| v.int(x)).collect()), v.int(flag)]
}

struct T {
    eng: Engine,
    op: phitex_defuse::OpId,
    ops: Arc<Ops>,
    s: NodeId,
    r: NodeId,
}

impl T {
    fn new(items: &[i64]) -> T {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops { runs: Mutex::new(Vec::new()) });
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        let u = eng.new_unit();
        let s_args = vec![eng.values().str("s"), eng.values().int(100)];
        let s = eng.push(u, op, s_args);
        let r_args = args(eng.values(), items, 1);
        let r = eng.push(u, op, r_args);
        eng.evaluate_sequential().expect("evaluate");
        T { eng, op, ops, s, r }
    }

    /// Retexts `r` (and `s`'s `b`, if given); evaluates; the items its
    /// last run summed and the mark it resumed at.
    fn edit(&mut self, items: &[i64], flag: i64, b: Option<i64>) -> (usize, Option<u32>) {
        if let Some(b) = b {
            let s_args = vec![self.eng.values().str("s"), self.eng.values().int(b)];
            self.eng.replace(self.s, self.op, s_args);
        }
        let r_args = args(self.eng.values(), items, flag);
        self.eng.replace(self.r, self.op, r_args);
        self.eng.evaluate_sequential().expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
        let bv = b.unwrap_or(100);
        let want: i64 = items.iter().sum::<i64>() + bv * flag;
        assert_eq!(self.eng.output(self.r).and_then(|x| self.eng.values().as_int(x)), Some(want), "{items:?} {flag}");
        let (at, n) = *self.ops.runs.lock().unwrap().last().expect("a run");
        (n, at)
    }
}

#[test]
fn a_retexted_node_resumes_at_the_last_mark_its_texts_share() {
    let mut t = T::new(&[1, 2, 3, 4, 5, 6]);
    // The last item: resumed at mark 5, one item summed.
    assert_eq!(t.edit(&[1, 2, 3, 4, 5, 9], 1, None), (1, Some(5)));
    // The fourth: at mark 3.
    assert_eq!(t.edit(&[1, 2, 3, 7, 5, 9], 1, None), (3, Some(3)));
    // One more item at the end: at mark 6.
    assert_eq!(t.edit(&[1, 2, 3, 7, 5, 9, 4], 1, None), (1, Some(6)));
}

#[test]
fn a_retexted_node_with_another_flag_runs_from_the_start() {
    let mut t = T::new(&[1, 2, 3, 4]);
    assert_eq!(t.edit(&[1, 2, 3, 5], 2, None), (4, None));
}

#[test]
fn a_retexted_node_resumes_no_later_than_its_first_broken_read() {
    let mut t = T::new(&[1, 2, 3, 4, 5]);
    // `b` read after the second item changes: no later than mark 1.
    assert_eq!(t.edit(&[1, 2, 3, 4, 6], 1, Some(7)), (4, Some(1)));
}
