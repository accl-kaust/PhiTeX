//! A run joins the node's last run at a mark with an equal client state
//! (`Ctx::checkpoint_join`): cut off when that run's reads after it all
//! hold, else hopping to its last mark before the first that does not.
//! Unit [x0..x11, j, r]: `xi` defines `xi`; `j` (from item `start`) marks
//! before each item `i` with state (i, carry[, its table]) and makes
//! `y_i = x_i + carry` (carry: the last `x` odd), `z = y_i` (overwritten
//! each item) and a contribution `y_i` to `acc`: at once, or (`end`) all
//! at its end, as an exec run does (its table then in its state, and
//! `y_i` of `x_i`'s tens only); with `kids`, it places a continuation
//! `c(y_i)` each item. `r`
//! reads them all. Edits keeping the parity of an `x` leave the carries
//! as they were: the run rejoins its last at the next mark.
//! CLEAN_KEPT_CHECK (set here) compares every joined run with a full run.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use phitex_defuse::{Ctx, Engine, Field, Locality, NodeId, OpId, Operation, Report, Stop, ValueId, Values};

const N: i64 = 12;

/// The client's state at a mark: its name, the carry and the table so far.
type Snap = (u32, i64, Vec<(i64, i64)>);

struct Ops {
    hops: AtomicUsize,
    op: std::sync::OnceLock<OpId>,
}

fn tag(v: &Values, a: &[ValueId]) -> String {
    v.as_str(a[0]).unwrap_or_default()
}

fn int(v: &Values, x: ValueId) -> i64 {
    v.as_int(x).unwrap_or(0)
}

impl Operation for Ops {
    fn defs(&self, a: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(match tag(v, a).as_str() {
            "x" => vec![(v.field(&format!("x{}", int(v, a[1]))), Locality::Global)],
            "j" => (0..N).map(|i| (v.field(&format!("y{i}")), Locality::Global)).chain([(v.field("z"), Locality::Global)]).collect(),
            _ => Vec::new(),
        })
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args();
        match tag(v, &a).as_str() {
            "x" => {
                cx.define_with(v.field(&format!("x{}", int(v, a[1]))), a[2], Locality::Global);
                return Ok(ValueId::UNIT);
            }
            "c" => return Ok(a[1]),
            "r" => {
                let mut s = 0;
                for i in 0..N {
                    s += (i + 1) * cx.use_field(v.field(&format!("y{i}")))?.map_or(0, |x| int(v, x));
                }
                s += 1000 * cx.use_field(v.field("z"))?.map_or(0, |x| int(v, x));
                s += 100_000 * int(v, cx.acc_range(v.field("acc"), None)?);
                return Ok(v.int(s));
            }
            _ => {}
        }
        let (start, end, kids) = (int(v, a[1]), int(v, a[2]) == 1, int(v, a[3]) == 1);
        let (acc, z) = (v.field("acc"), v.field("z"));
        let emit = |cx: &mut Ctx<'_>, i: i64, y: i64| {
            cx.define_with(v.field(&format!("y{i}")), v.int(y), Locality::Global);
            cx.define_with(z, v.int(y), Locality::Global);
            cx.contribute(acc, v.int(y));
        };
        let state = |i: u32, c: i64, t: &[(i64, i64)]| {
            let mut w = vec![u64::from(i), c as u64];
            if end {
                w.extend(t.iter().flat_map(|&(i, y)| [i as u64, y as u64]));
            }
            v.int(phitex_defuse::hash_words(7, &w) as i64 >> 2)
        };
        // Resumed at a mark: from its snapshot (outputs made again).
        let mut snaps: Vec<Snap> = Vec::new();
        let (mut i, mut c, mut table) = match cx.resumed() {
            Some((tok, k)) => {
                let t = tok.downcast_ref::<Vec<Snap>>().expect("snapshots");
                let s = t.iter().find(|s| s.0 == k).expect("the mark's").clone();
                snaps.extend(t.iter().filter(|s| s.0 < k).cloned());
                if !end {
                    for &(i, y) in &s.2 {
                        emit(cx, i, y);
                    }
                }
                (i64::from(k), s.1, s.2)
            }
            None => (start, 0, Vec::new()),
        };
        let mut marked = false;
        while i < N {
            if !marked {
                let k = u32::try_from(i).unwrap();
                snaps.push((k, c, table.clone()));
                cx.keep_after(Arc::new(snaps.clone()));
                let j = match cx.checkpoint_join(k, state(k, c, &table)) {
                    Ok(j) => j,
                    Err(e) => {
                        // Cut off: its token of the old run's marks too.
                        let h = cx.cut_join().expect("the old run's token");
                        let t = h.token.downcast_ref::<Vec<Snap>>().expect("snapshots");
                        snaps.extend(t.iter().filter(|s| s.0 > h.from).map(|s| {
                            let mut s = s.clone();
                            s.2 = table.iter().copied().chain(s.2.iter().filter(|&&(j, _)| j >= i64::from(h.from)).copied()).collect();
                            s
                        }));
                        cx.keep_after(Arc::new(snaps));
                        return Err(e);
                    }
                };
                if let Some(h) = j {
                    self.hops.fetch_add(1, Relaxed);
                    // On from the old run's snapshot at the mark hopped
                    // to: what it made from the join on is this run's.
                    let t = h.token.downcast_ref::<Vec<Snap>>().expect("snapshots");
                    let s = t.iter().find(|s| s.0 == h.to).expect("the mark's");
                    snaps.extend(t.iter().filter(|s| s.0 > h.from && s.0 <= h.to).map(|s| {
                        let mut s = s.clone();
                        s.2 = table.iter().copied().chain(s.2.iter().filter(|&&(j, _)| j >= i64::from(h.from)).copied()).collect();
                        s
                    }));
                    table.extend(s.2.iter().filter(|&&(j, _)| j >= i64::from(h.from)).copied());
                    c = s.1;
                    i = i64::from(h.to);
                    marked = true;
                    continue;
                }
            }
            marked = false;
            let x = cx.use_field(v.field(&format!("x{i}")))?.map_or(0, |x| int(v, x));
            let y = if end { x / 10 * 10 } else { x } + c;
            if !end {
                emit(cx, i, y);
            }
            if kids {
                cx.follow(self.op.get().copied().expect("registered"), vec![v.str("c"), v.int(y)]);
            }
            table.push((i, y));
            c = x % 2;
            i += 1;
        }
        cx.keep_after(Arc::new(snaps));
        if end {
            for &(i, y) in &table {
                emit(cx, i, y);
            }
        }
        Ok(v.int(c))
    }
}

struct Doc {
    eng: Engine,
    ops: Arc<Ops>,
    op: OpId,
    xs: Vec<NodeId>,
    j: NodeId,
    r: NodeId,
    end: bool,
    kids: bool,
}

fn doc(xs0: &[i64], end: bool, kids: bool) -> Doc {
    // SAFETY: tests in this file set the same value.
    unsafe { std::env::set_var("CLEAN_KEPT_CHECK", "1") };
    let mut eng = Engine::new();
    let ops = Arc::new(Ops { hops: AtomicUsize::new(0), op: std::sync::OnceLock::new() });
    let op = eng.register(ops.clone());
    let _ = ops.op.set(op);
    eng.declare_accumulator(eng.values().field("acc"), Arc::new(phitex_defuse::SumMonoid));
    let u = eng.new_unit();
    let xs = xs0
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let a = vec![eng.values().str("x"), eng.values().int(i as i64), eng.values().int(x)];
            eng.push(u, op, a)
        })
        .collect();
    let a = vec![eng.values().str("j"), eng.values().int(0), eng.values().int(i64::from(end)), eng.values().int(i64::from(kids))];
    let j = eng.push(u, op, a);
    let a = vec![eng.values().str("r")];
    let r = eng.push(u, op, a);
    eng.add_root(r);
    Doc { eng, ops, op, xs, j, r, end, kids }
}

/// What `r` makes of `xs` with `j` from `start`.
fn want(xs: &[i64], start: i64, end: bool) -> i64 {
    let (mut s, mut c, mut z, mut sum) = (0, 0, 0, 0);
    for i in start..N {
        let x = xs[i as usize];
        let y = if end { x / 10 * 10 } else { x } + c;
        s += (i + 1) * y;
        z = y;
        sum += y;
        c = xs[i as usize] % 2;
    }
    s + 1000 * z + 100_000 * sum
}

impl Doc {
    fn eval(&mut self) -> Report {
        self.eng.evaluate(&mut phitex_defuse::Greedy::default(), &mut phitex_defuse::Sequential).expect("evaluate")
    }
    fn out(&self) -> i64 {
        self.eng.output(self.r).and_then(|y| self.eng.values().as_int(y)).expect("an output")
    }
    fn set_x(&mut self, i: usize, x: i64) {
        let a = vec![self.eng.values().str("x"), self.eng.values().int(i as i64), self.eng.values().int(x)];
        self.eng.replace(self.xs[i], self.op, a);
    }
    fn set_start(&mut self, start: i64) {
        let a = vec![self.eng.values().str("j"), self.eng.values().int(start), self.eng.values().int(i64::from(self.end)), self.eng.values().int(i64::from(self.kids))];
        self.eng.replace(self.j, self.op, a);
    }
    fn check(&self) {
        assert!(self.eng.check_reads().is_empty(), "{:?}", self.eng.check_reads());
        assert!(self.eng.check_invariants().is_empty(), "{:?}", self.eng.check_invariants());
    }
}

fn xs0() -> Vec<i64> {
    vec![3, 4, 7, 2, 5, 8, 1, 6, 9, 2, 3, 4]
}

/// A fresh run whose start moved joins its last run where their states
/// meet (the carry the same), and is cut off there.
#[test]
fn a_fresh_run_from_a_moved_start_is_cut_off_where_it_meets_its_last() {
    let mut xs = xs0();
    // (Item 0's `x` even: from 1, the carry at 1 is the old run's.)
    xs[0] = 4;
    let mut d = doc(&xs, false, false);
    d.eval();
    assert_eq!(d.out(), want(&xs, 0, false));
    d.set_start(1);
    let rep = d.eval();
    assert_eq!(d.out(), want(&xs, 1, false), "moved start");
    assert_eq!(rep.cut_in_run, 1, "cut off at its first mark");
    d.check();
    // (Back: item 0 again, then the old run's from 1.)
    d.set_start(0);
    let rep = d.eval();
    assert_eq!(d.out(), want(&xs, 0, false), "start back");
    assert_eq!(rep.cut_in_run, 1, "cut off at its second mark");
    d.check();
}

/// Edits keeping parities at items 2, 5 and 8: resumed at mark 2, the run
/// joins at 3 and hops to 5, joins at 6 and hops to 8, and is cut off at
/// 9. Its outputs at once (spliced by segment) or at its end.
#[test]
fn a_run_hops_over_the_reads_that_hold_and_is_cut_off_after_the_last_broken() {
    for (end, kids) in [(false, false), (true, false), (false, true)] {
        let mut xs = xs0();
        let mut d = doc(&xs, end, kids);
        d.eval();
        assert_eq!(d.out(), want(&xs, 0, end), "end {end}: cold");
        for (i, x) in [(2, 9), (5, 6), (8, 7)] {
            xs[i] = x;
            d.set_x(i, x);
        }
        let hops = |d: &Doc| d.ops.hops.load(Relaxed);
        let h0 = hops(&d);
        let rep = d.eval();
        assert_eq!(d.out(), want(&xs, 0, end), "end {end}: edited");
        assert_eq!(hops(&d) - h0, 2, "end {end}: hops");
        assert_eq!(rep.cut_in_run, 1, "end {end}: cut off");
        d.check();
        // (Resumed at mark 10, joined at the cut: from the token kept
        // then, and cut off at 11.)
        xs[10] = 5;
        d.set_x(10, 5);
        let rep = d.eval();
        assert_eq!(d.out(), want(&xs, 0, end), "end {end}: after the cut");
        assert_eq!(rep.cut_in_run, 1, "end {end}: after the cut");
        d.check();
        // (A parity change at 3: the carry at 4 differs; at 5 it is the
        // last run's again, where the run is cut off.)
        xs[3] = 3;
        d.set_x(3, 3);
        let h0 = hops(&d);
        let rep = d.eval();
        assert_eq!(d.out(), want(&xs, 0, end), "end {end}: parity edited");
        // (At its end, its table in its state: from 4 on, not the last
        // run's.)
        assert_eq!((hops(&d) - h0, rep.cut_in_run), if end { (0, 0) } else { (0, 1) }, "end {end}: parity edited");
        d.check();
    }
}

/// Placements so far not the last run's at the mark (by place and
/// operation): their nodes would differ, and the run does not join.
#[test]
fn a_run_with_other_placements_so_far_does_not_join() {
    let mut xs = xs0();
    xs[0] = 4;
    let mut d = doc(&xs, false, true);
    d.eval();
    d.set_start(1);
    let rep = d.eval();
    assert_eq!(d.out(), want(&xs, 1, false), "moved start");
    assert_eq!(rep.cut_in_run, 0, "no join");
    d.check();
}
