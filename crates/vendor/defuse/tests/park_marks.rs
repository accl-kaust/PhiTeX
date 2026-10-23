//! A run parked continuing whose kept read breaks resumes at the last of
//! its attempts' marks before that read, not from its start. Unit
//! [s, a, r]: `s` defines `x`; `a` reads `x`, defines `z` and places `d(x)`
//! after itself, which defines `y = x / 10`. `r` goes through items 0..3,
//! before each marking (its sum so far, kept per mark) and parking
//! continuing: item 0 reads `z`, item 1 `y`, item 2 places child `c(z)` and
//! waits for its output. An edit of `x` changes `z`: `r` runs again before
//! `d` has, guessing `y` from the last, and parks on `c`; `d` makes another
//! `y`, breaking that kept read, and `r` resumes at mark 1 (as `c` would
//! run, its placer's kept state broken).

use std::sync::{Arc, Mutex, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, OpId, Operation, Stop, ValueId, Values};

struct Ops {
    op: OnceLock<OpId>,
    /// For each run of `r` not from its token: the mark it resumed at, and
    /// the items it went through (over its attempts).
    runs: Mutex<Vec<(Option<u32>, Vec<usize>)>>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "s" => vec![(v.field("x"), Locality::Global)],
            "a" => vec![(v.field("z"), Locality::Global)],
            "d" => vec![(v.field("y"), Locality::Global)],
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        let op = *self.op.get().unwrap();
        let (x, y, z) = (v.field("x"), v.field("y"), v.field("z"));
        match tag(v, &a).as_str() {
            "s" => {
                cx.define_with(x, a[1], Locality::Global);
                return Ok(ValueId::UNIT);
            }
            "a" => {
                let xv = cx.use_field(x)?.unwrap_or(v.int(0));
                cx.define_with(z, xv, Locality::Global);
                cx.follow(op, vec![v.str("d"), xv]);
                return Ok(ValueId::UNIT);
            }
            "d" => {
                cx.define_with(y, v.int(v.as_int(a[1]).unwrap() / 10), Locality::Global);
                return Ok(ValueId::UNIT);
            }
            "c" => return Ok(a[1]),
            _ => {}
        }
        // Parked: on from the token (at item 2, its child placed); resumed at a mark:
        // from that mark's sums.
        let parked = cx.parked().and_then(|t| t.downcast::<(usize, Vec<i64>)>().ok());
        let (mut i, mut sums, placed) = match parked {
            Some(t) => (t.0, t.1, t.0 == 2),
            None => {
                let at = cx.resumed().map(|(tok, k)| (tok.downcast_ref::<Vec<i64>>().expect("sums")[..=k as usize].to_vec(), k));
                self.runs.lock().unwrap().push((at.as_ref().map(|q| q.1), Vec::new()));
                match at {
                    Some((s, k)) => (k as usize, s, false),
                    None => (0, vec![0], false),
                }
            }
        };
        while i < 3 {
            let sum = *sums.last().expect("a sum");
            cx.checkpoint(u32::try_from(i).unwrap(), v.int(sum))?;
            cx.keep_after(Arc::new(sums.clone()));
            cx.park_continuing(Box::new((i, sums.clone())));
            self.runs.lock().unwrap().last_mut().expect("a run").1.push(i);
            let q = match i {
                0 => cx.use_field(z)?,
                1 => cx.use_field(y)?,
                _ => {
                    if !placed {
                        let zv = cx.use_field(z)?.unwrap_or(v.int(0));
                        cx.child(op, vec![v.str("c"), zv]);
                    }
                    Some(cx.output_of(0)?)
                }
            };
            sums.push(sum + q.and_then(|q| v.as_int(q)).unwrap_or(0));
            i += 1;
        }
        Ok(v.int(*sums.last().expect("a sum")))
    }
}

#[test]
fn a_parked_run_whose_kept_read_breaks_resumes_at_a_mark() {
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { op: OnceLock::new(), runs: Mutex::new(Vec::new()) });
    let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
    ops.op.set(op).expect("once");
    let y = eng.values().field("y");
    eng.guess_from_last(y);
    let u = eng.new_unit();
    let s_args = vec![eng.values().str("s"), eng.values().int(10)];
    let s = eng.push(u, op, s_args);
    let a_args = vec![eng.values().str("a")];
    eng.push(u, op, a_args);
    let r_args = vec![eng.values().str("r")];
    let r = eng.push(u, op, r_args);
    let spec = |eng: &mut Engine| eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
    spec(&mut eng);
    assert_eq!(eng.output(r).and_then(|q| eng.values().as_int(q)), Some(10 + 1 + 10));
    let before = ops.runs.lock().unwrap().len();
    let s_args = vec![eng.values().str("s"), eng.values().int(25)];
    eng.replace(s, op, s_args);
    let rep = spec(&mut eng);
    assert!(eng.check_reads().is_empty(), "{:?}", eng.check_reads());
    assert_eq!(eng.output(r).and_then(|q| eng.values().as_int(q)), Some(25 + 2 + 25));
    let runs = ops.runs.lock().unwrap()[before..].to_vec();
    // From the start (`z` changed: its last attempt's marks are past it),
    // then at mark 1 (`y`), and never from the start again.
    assert_eq!(runs[0], (None, vec![0, 1, 2]), "{runs:?}");
    assert_eq!(runs[1], (Some(1), vec![1, 2]), "{runs:?}");
    assert!(runs[1..].iter().all(|r| r.0.is_some()), "{runs:?}");
    assert!(rep.resumed_at_mark >= 1);
}
