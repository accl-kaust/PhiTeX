//! A run parked continuing resumes at the last of its attempts' marks
//! before its first broken read, not from its start: stopped (its kept
//! reads broken) or done (its result's). Unit [s, t, a, r]: `s` defines
//! `x`, `t` defines `w`; `a` reads `x`, defines `z` and places `d(x)` after
//! itself, which defines `y = x / 10`. `r` goes through items 0..4, before
//! each marking (its sum so far, kept per mark) and parking continuing:
//! item 0 places child `c(0)` and reads `z`, item 1 reads `y`, item 2 `w`,
//! item 3 places child `c(z)` and waits for its output. Cold, `r` stops on `y` and on `c`: three attempts.

use std::sync::{Arc, Mutex, OnceLock};

use defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

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
            "t" => vec![(v.field("w"), Locality::Global)],
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
            "t" => {
                cx.define_with(v.field("w"), a[1], Locality::Global);
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
        // Parked: on from the token (its item's child placed); resumed at a mark:
        // from that mark's sums.
        let parked = cx.parked().and_then(|t| t.downcast::<(usize, Vec<i64>)>().ok());
        let (mut i, mut sums, mut placed) = match parked {
            Some(t) => (t.0, t.1, true),
            None => {
                let at = cx.resumed().map(|(tok, k)| (tok.downcast_ref::<Vec<i64>>().expect("sums")[..=k as usize].to_vec(), k));
                self.runs.lock().unwrap().push((at.as_ref().map(|q| q.1), Vec::new()));
                match at {
                    Some((s, k)) => (k as usize, s, false),
                    None => (0, vec![0], false),
                }
            }
        };
        while i < 4 {
            let sum = *sums.last().expect("a sum");
            cx.checkpoint(u32::try_from(i).unwrap(), v.int(sum))?;
            cx.keep_after(Arc::new(sums.clone()));
            cx.park_continuing(Box::new((i, sums.clone())));
            self.runs.lock().unwrap().last_mut().expect("a run").1.push(i);
            let q = match i {
                0 => {
                    if !placed {
                        cx.child(op, vec![v.str("c"), v.int(0)]);
                    }
                    cx.use_field(z)?
                }
                1 => cx.use_field(y)?,
                2 => cx.use_field(v.field("w"))?,
                _ => {
                    if !placed {
                        let zv = cx.use_field(z)?.unwrap_or(v.int(0));
                        cx.child(op, vec![v.str("c"), zv]);
                    }
                    Some(cx.output_of(1)?)
                }
            };
            sums.push(sum + q.and_then(|q| v.as_int(q)).unwrap_or(0));
            i += 1;
            placed = false;
        }
        Ok(v.int(*sums.last().expect("a sum")))
    }
}

struct T {
    eng: Engine,
    ops: Arc<Ops>,
    op: OpId,
    s: NodeId,
    t: NodeId,
    r: NodeId,
}

impl T {
    fn new() -> T {
        let mut eng = Engine::new();
        let ops = Arc::new(Ops { op: OnceLock::new(), runs: Mutex::new(Vec::new()) });
        let op = eng.register(Arc::clone(&ops) as Arc<dyn Operation>);
        ops.op.set(op).expect("once");
        let y = eng.values().field("y");
        eng.guess_from_last(y);
        let u = eng.new_unit();
        let s_args = vec![eng.values().str("s"), eng.values().int(10)];
        let s = eng.push(u, op, s_args);
        let t_args = vec![eng.values().str("t"), eng.values().int(100)];
        let t = eng.push(u, op, t_args);
        let a_args = vec![eng.values().str("a")];
        eng.push(u, op, a_args);
        let r_args = vec![eng.values().str("r")];
        let r = eng.push(u, op, r_args);
        let mut t = T { eng, ops, op, s, t, r };
        t.eval(10 + 1 + 100 + 10);
        assert_eq!(t.runs().len(), 1);
        t
    }

    fn set(&mut self, n: NodeId, tag: &str, x: i64) {
        let args = vec![self.eng.values().str(tag), self.eng.values().int(x)];
        self.eng.replace(n, self.op, args);
    }

    /// Evaluates, `r`'s output `want`; its runs (but from its token).
    fn eval(&mut self, want: i64) -> Report {
        let rep = self.eng.evaluate(&mut defuse::Greedy::speculative(defuse::Speculation::All, 64), &mut defuse::Sequential).expect("evaluate");
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
        assert_eq!(self.eng.output(self.r).and_then(|q| self.eng.values().as_int(q)), Some(want));
        rep
    }

    fn runs(&self) -> Vec<(Option<u32>, Vec<usize>)> {
        std::mem::take(&mut *self.ops.runs.lock().unwrap())
    }
}

/// An edit of `x` changes `z`: `r` runs again, at its first mark, before
/// `d` has, guessing `y` from the last, and parks on `c`; `d` makes another
/// `y`, breaking that kept read, and `r` resumes at mark 1 (as `c` would
/// run, its placer's kept state broken).
#[test]
fn a_stopped_run_whose_kept_read_breaks_resumes_at_a_mark() {
    let mut t = T::new();
    t.set(t.s, "s", 25);
    let rep = t.eval(25 + 2 + 100 + 25);
    let runs = t.runs();
    assert_eq!(runs[0], (Some(0), vec![0, 1, 2, 3]), "{runs:?}");
    assert_eq!(runs[1], (Some(1), vec![1, 2, 3]), "{runs:?}");
    assert!(runs.iter().all(|r| r.0.is_some()), "{runs:?}");
    assert!(rep.resumed_at_mark >= 2);
}

/// An edit of `w`, read in `r`'s second attempt: it resumes at mark 2,
/// from its done result's marks over its attempts.
#[test]
fn a_done_run_whose_earlier_attempts_read_breaks_resumes_at_a_mark() {
    let mut t = T::new();
    t.set(t.t, "t", 300);
    t.eval(10 + 1 + 300 + 10);
    assert_eq!(t.runs(), vec![(Some(2), vec![2, 3])]);
}
