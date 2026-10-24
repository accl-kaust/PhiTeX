//! Ranks in a sequence (`Step::Rank`, `Step::Ranks`, `Step::Distinct`):
//! the place of an element among the sequence's distinct elements in the
//! order they first occur. Unit [l_0 .. l_5, p_0, p_1]: each loader `l_i`
//! contributes its font to `loads` (a sequence, duplicates kept); page
//! `p_j` takes the ranks of its two fonts and the count of distinct fonts
//! loaded before it through one view of the fold. A load that changes no
//! page's ranks or count leaves the pages standing; one that moves a
//! font's first occurrence runs them again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, SeqMonoid, Step, Stop, ValueId, Values};

struct Ops(Mutex<HashMap<String, usize>>);

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let t = tag(v, &a);
        *self.0.lock().unwrap().entry(t.clone()).or_default() += 1;
        let loads = v.field("loads");
        if t == "l" {
            cx.contribute(loads, v.seq_unit(a[1]));
            return Ok(ValueId::UNIT);
        }
        // A page: the ranks of its fonts (gathered: one read) and the
        // distinct fonts loaded before it.
        let m = cx.acc_view(loads, None, Some(cx.node()))?;
        let mut out: Vec<ValueId> = [a[1], a[2]].iter().map(|&k| cx.get(m, Step::Rank(k)).unwrap_or(v.int(0))).collect();
        out.push(cx.get(m, Step::Distinct).unwrap_or(v.int(0)));
        Ok(v.list(out))
    }
}

struct T {
    eng: Engine,
    op: OpId,
    ops: Arc<Ops>,
    l: Vec<NodeId>,
    p: Vec<NodeId>,
}

impl T {
    fn new(fonts: &[i64]) -> T {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops(Mutex::new(HashMap::new())));
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        let loads = eng.values().field("loads");
        eng.declare_accumulator(loads, Arc::new(SeqMonoid));
        let u = eng.new_unit();
        let mut l = Vec::new();
        for &f in fonts {
            let a = vec![eng.values().str("l"), eng.values().int(f)];
            l.push(eng.push(u, op, a));
        }
        let mut p = Vec::new();
        for (x, y) in [(3, 1), (2, 9)] {
            let a = vec![eng.values().str("p"), eng.values().int(x), eng.values().int(y)];
            p.push(eng.push(u, op, a));
        }
        let mut t = T { eng, op, ops, l, p };
        t.eval();
        t
    }

    fn eval(&mut self) {
        self.eng.evaluate_sequential().expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
    }

    fn page(&self, j: usize) -> Vec<i64> {
        let v = self.eng.values();
        let x = self.eng.output(self.p[j]).expect("output");
        match &*v.get(x) {
            phitex_defuse::Value::List(l) => l.iter().map(|&e| v.as_int(e).unwrap()).collect(),
            _ => Vec::new(),
        }
    }

    fn load(&mut self, i: usize, f: i64) -> usize {
        let a = vec![self.eng.values().str("l"), self.eng.values().int(f)];
        self.eng.replace(self.l[i], self.op, a);
        let before = self.ops.0.lock().unwrap().get("p").copied().unwrap_or(0);
        self.eval();
        self.ops.0.lock().unwrap().get("p").copied().unwrap_or(0) - before
    }
}

#[test]
fn ranks_count_first_occurrences() {
    let mut t = T::new(&[1, 2, 1, 3, 2, 4]);
    // Distinct in order: 1, 2, 3, 4.
    assert_eq!(t.page(0), vec![3, 1, 4]);
    assert_eq!(t.page(1), vec![2, 0, 4]);
    // A repeat replaced by another repeat: nothing moves, no page runs.
    assert_eq!(t.load(4, 1), 0);
    assert_eq!(t.page(0), vec![3, 1, 4]);
    // The last load 9 for 4: the second page's 9 is ranked now; the first
    // page's ranks and count stand, and it does not run.
    assert_eq!(t.load(5, 9), 1);
    assert_eq!(t.page(0), vec![3, 1, 4]);
    assert_eq!(t.page(1), vec![2, 4, 4]);
    // 3 loaded first (3, 2, 1, 9): the first page's ranks move; the
    // second's (2 and 9) stand.
    assert_eq!(t.load(0, 3), 1);
    assert_eq!(t.page(0), vec![1, 3, 4]);
    assert_eq!(t.page(1), vec![2, 4, 4]);
    // A new font third (3, 2, 7, 1, 9): the count moves for both, and 1's
    // rank.
    assert_eq!(t.load(2, 7), 2);
    assert_eq!(t.page(0), vec![1, 4, 5]);
    assert_eq!(t.page(1), vec![2, 5, 5]);
}

#[test]
fn ranks_on_a_long_sequence() {
    // Many loads of few fonts: ranks as of a flat walk.
    let fonts: Vec<i64> = (0..300).map(|i| (i * 7 + i / 13) % 11).collect();
    let t = T::new(&fonts);
    let mut seen: Vec<i64> = Vec::new();
    for &f in &fonts {
        if !seen.contains(&f) {
            seen.push(f);
        }
    }
    let rank = |k: i64| seen.iter().position(|&e| e == k).map_or(0, |i| i64::try_from(i).unwrap() + 1);
    assert_eq!(t.page(0), vec![rank(3), rank(1), i64::try_from(seen.len()).unwrap()]);
    assert_eq!(t.page(1), vec![rank(2), rank(9), i64::try_from(seen.len()).unwrap()]);
}
