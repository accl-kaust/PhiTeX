//! The core's specification: what the engine does, measured on toy
//! operations. Every test here passes before any client code is touched.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{
    At, Ctx, Engine, Field, FieldAt, NodeId, NodeState, OpId, Operation, Report, Step, Stop, ValueId, Values,
};

/// `[f, v]`: defines f = v; outputs v.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }
}

/// `[f]`: outputs the definition of f reaching it (UNIT if none).
struct Get;

impl Operation for Get {
    fn name(&self) -> &'static str {
        "get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        Ok(cx.read(f)?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f, c]`: f := f + c, as a relation; never reads f.
struct Adv;

impl Operation for Adv {
    fn name(&self) -> &'static str {
        "adv"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, c) = (Field(cx.args()[0]), cx.values().as_int(cx.args()[1]).unwrap());
        cx.define_rel(f, c);
        Ok(ValueId::UNIT)
    }
}

/// `[f, ms]`: reads f, works `ms` milliseconds, then f := f + 1 as a
/// relation; outputs what it read. A page-like routine.
struct Routine(AtomicU64);

impl Operation for Routine {
    fn name(&self) -> &'static str {
        "routine"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let ms = cx.values().as_int(cx.args()[1]).unwrap() as u64;
        let x = cx.read(f)?.unwrap_or(ValueId::UNIT);
        spin(Duration::from_millis(ms));
        cx.define_rel(f, 1);
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(x)
    }
}

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (s, x) = (Field(cx.args()[0]), cx.args()[1]);
        cx.contribute(s, x);
        Ok(x)
    }
}

/// `[s]`: outputs the fold of s before it.
struct FoldR;

impl Operation for FoldR {
    fn name(&self) -> &'static str {
        "fold"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        cx.fold(s, None, None)
    }
}

/// `[n, salt]`: places n children `Def(f_i, i)`; outputs n.
struct Spawn(OpId);

impl Operation for Spawn {
    fn name(&self) -> &'static str {
        "spawn"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap();
        for i in 0..n {
            let f = v.field(&format!("child{i}"));
            cx.child(self.0, vec![f.0, v.int(i)]);
        }
        Ok(cx.args()[0])
    }
}

/// `[us]`: spins `us` microseconds.
struct Work;

impl Operation for Work {
    fn name(&self) -> &'static str {
        "work"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let us = cx.values().as_int(cx.args()[0]).unwrap() as u64;
        spin(Duration::from_micros(us));
        Ok(ValueId::UNIT)
    }
}

/// `[n]`: claims the next n siblings; outputs the list of their first args.
struct Claimer;

impl Operation for Claimer {
    fn name(&self) -> &'static str {
        "claimer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let n = cx.values().as_int(cx.args()[0]).unwrap() as usize;
        cx.claim(n);
        let mut xs = Vec::new();
        for i in 0..n {
            if let Some((_, args)) = cx.following(i) {
                xs.push(args[0]);
            }
        }
        Ok(cx.values().list(xs))
    }
}

/// `[f]`: outputs element 1 of the list defined as f.
struct Idx;

impl Operation for Idx {
    fn name(&self) -> &'static str {
        "idx"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let p = cx.path(&[Step::Index(1)]);
        Ok(cx.read_path(f, p)?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f, n]`: outputs the definition of f reaching node n (At::Before).
struct GetBefore;

impl Operation for GetBefore {
    fn name(&self) -> &'static str {
        "get_before"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let n = NodeId::from_raw(cx.values().as_int(cx.args()[1]).unwrap() as u64);
        Ok(match cx.read_at(f, At::Before(n))? {
            FieldAt::Def(v, _) => v,
            FieldAt::Unset | FieldAt::Gone => ValueId::UNIT,
        })
    }
}

fn spin(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

struct Ops {
    def: OpId,
    get: OpId,
    adv: OpId,
    routine: OpId,
    contrib: OpId,
    fold: OpId,
    spawn: OpId,
    work: OpId,
    claimer: OpId,
    idx: OpId,
    get_before: OpId,
    routine_runs: Arc<Routine>,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let def = e.register(Arc::new(Def));
    let routine_runs = Arc::new(Routine(AtomicU64::new(0)));
    let ops = Ops {
        def,
        get: e.register(Arc::new(Get)),
        adv: e.register(Arc::new(Adv)),
        routine: e.register(routine_runs.clone()),
        contrib: e.register(Arc::new(Contrib)),
        fold: e.register(Arc::new(FoldR)),
        spawn: e.register(Arc::new(Spawn(def))),
        work: e.register(Arc::new(Work)),
        claimer: e.register(Arc::new(Claimer)),
        idx: e.register(Arc::new(Idx)),
        get_before: e.register(Arc::new(GetBefore)),
        routine_runs,
    };
    (e, ops)
}

fn f(v: &Values, name: &str) -> ValueId {
    v.field(name).0
}

fn int(e: &Engine, n: NodeId) -> i64 {
    e.values().as_int(e.output(n).expect("ran")).expect("int")
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

#[test]
fn reaching_definitions() {
    let (e, o) = engine();
    let v = e.values();
    let (x, g) = (f(v, "x"), f(v, "g"));
    let (one, two, three) = (v.int(1), v.int(2), v.int(3));
    let u = e.new_unit();
    e.push(u, o.def, vec![x, one]);
    let a = e.push(u, o.get, vec![x]);
    let d2 = e.push(u, o.def, vec![x, two]);
    let b = e.push(u, o.get, vec![x]);
    let c = e.push(u, o.get, vec![g]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, a), 1);
    assert_eq!(int(&e, b), 2);
    assert_eq!(e.output(c), Some(ValueId::UNIT));
    assert!(e.check_invariants().is_empty());

    e.replace(d2, o.def, vec![x, three]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(int(&e, a), 1);
    assert_eq!(int(&e, b), 3);
    assert_eq!(r.nodes_run, 2, "the edited definer and the one reader after it: {r:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn relative_chain_is_additions_not_runs() {
    let (e, o) = engine();
    let v = e.values();
    let c = f(v, "c");
    let (zero, one, two) = (v.int(0), v.int(1), v.int(2));
    let u = e.new_unit();
    e.push(u, o.def, vec![c, zero]);
    let advs: Vec<NodeId> = (0..1000).map(|_| e.push(u, o.adv, vec![c, one])).collect();
    let end = e.push(u, o.get, vec![c]);
    e.evaluate(4).unwrap();
    assert_eq!(int(&e, end), 1000);

    e.replace(advs[0], o.adv, vec![c, two]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(int(&e, end), 1001);
    assert_eq!(Report::by_op(&r.runs_by_op, "adv"), 1, "only the edited node runs: {r:?}");
    assert!(r.relative_resolved >= 999, "the chain is recomputed in place: {r:?}");
    assert_eq!(Report::by_op(&r.runs_by_op, "get"), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_learned_shape_runs_the_chain_side_by_side() {
    if threads() < 8 {
        return;
    }
    let (e, o) = engine();
    let v = e.values();
    let c = f(v, "c");
    let (zero, ms) = (v.int(0), v.int(2));
    let u = e.new_unit();
    e.push(u, o.def, vec![c, zero]);
    let rs: Vec<NodeId> = (0..200).map(|_| e.push(u, o.routine, vec![c, ms])).collect();
    let t = Instant::now();
    let r = e.evaluate(8).unwrap();
    let wall = t.elapsed();
    for (i, n) in rs.iter().enumerate() {
        assert_eq!(int(&e, *n), i as i64, "routine {i} reads the count before it");
    }
    let runs = o.routine_runs.0.load(Ordering::Relaxed);
    assert!(runs <= 2 * 200, "each routine runs at most twice (predicted, then confirmed): {runs}");
    assert!(r.shaped >= 190, "pending routines were given the relation: {r:?}");
    assert!(wall < Duration::from_millis(150), "200 x 2 ms serial is 400 ms, twice over 8 threads is 100 ms: {wall:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn commits_do_not_wait_on_each_other() {
    let n = threads().min(32);
    if n < 4 {
        return;
    }
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    for i in 0..n * 1000 {
        let x = f(v, &format!("x{i}"));
        e.push(u, o.def, vec![x, v.int(i as i64)]);
        e.push(u, o.get, vec![x]);
    }
    let r = e.evaluate(n).unwrap();
    assert_eq!(r.runs, (n * 2000) as u64);
    let busy = r.times.wall_ns * n as u64;
    assert!(
        r.times.lock_wait_ns * 20 <= busy,
        "lock wait {} ns over {} worker-ns: more than 5%: {:?}",
        r.times.lock_wait_ns,
        busy,
        r.times
    );
    assert!(r.commits >= r.runs);
    let avg_commit_ns = r.times.commit_ns / r.commits;
    assert!(avg_commit_ns <= 20_000, "a commit is a few microseconds, not {avg_commit_ns} ns: {:?}", r.times);
    assert!(r.times.commit_max_ns < 1_000_000, "no commit takes a millisecond: {:?}", r.times);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn scales_with_threads() {
    if threads() < 8 {
        return;
    }
    let wall = |n: usize| {
        let (e, o) = engine();
        let v = e.values();
        let us = v.int(100);
        let u = e.new_unit();
        for _ in 0..8000 {
            e.push(u, o.work, vec![us]);
        }
        let t = Instant::now();
        e.evaluate(n).unwrap();
        t.elapsed()
    };
    let (t1, t8) = (wall(1), wall(8));
    assert!(t8 * 5 < t1, "8 threads at least five times as fast as 1 on independent work: {t1:?} vs {t8:?}");
    if threads() >= 32 {
        let t32 = wall(32);
        assert!(t32 < t8, "32 threads faster than 8: {t8:?} vs {t32:?}");
    }
}

#[test]
fn a_fold_is_a_monoid_over_segments() {
    let (e, o) = engine();
    let v = e.values();
    let s = f(v, "s");
    let u = e.new_unit();
    let cs: Vec<NodeId> = (0..5000).map(|i| e.push(u, o.contrib, vec![s, v.int(i)])).collect();
    let r = e.push(u, o.fold, vec![s]);
    let y = f(v, "y");
    e.push(u, o.def, vec![y, v.int(1)]);
    let g = e.push(u, o.get, vec![y]);
    e.evaluate(4).unwrap();
    let xs = e.values().seq_to_vec(e.output(r).unwrap());
    assert_eq!(xs.len(), 5000);
    assert!(xs.iter().enumerate().all(|(i, x)| e.values().as_int(*x) == Some(i as i64)));

    e.replace(cs[2500], o.contrib, vec![s, v.int(-1)]);
    let t = Instant::now();
    let rep = e.evaluate(4).unwrap();
    assert!(t.elapsed() < Duration::from_millis(10), "a fold over segment sums, not over 5000 items: {:?}", t.elapsed());
    let xs = e.values().seq_to_vec(e.output(r).unwrap());
    assert_eq!(e.values().as_int(xs[2500]), Some(-1));
    assert_eq!(rep.nodes_run, 2, "the contributor and the fold's reader, not {}: {rep:?}", rep.nodes_run);
    assert_eq!(int(&e, g), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn placements_are_kept_when_unchanged() {
    let (e, o) = engine();
    let v = e.values();
    let (three, salt1, salt2) = (v.int(3), v.int(1), v.int(2));
    let u = e.new_unit();
    let sp = e.push(u, o.spawn, vec![three, salt1]);
    let g = e.push(u, o.get, vec![f(v, "child2")]);
    e.evaluate(2).unwrap();
    let kids = e.children(sp);
    assert_eq!(kids.len(), 3);
    assert_eq!(int(&e, g), 2);

    e.replace(sp, o.spawn, vec![three, salt2]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.children(sp), kids, "the same placements keep their nodes");
    assert_eq!(r.nodes_run, 1, "only the spawner: {r:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_edit_costs_what_changed() {
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let mut defs = Vec::new();
    for i in 0..10000 {
        let x = f(v, &format!("x{i}"));
        defs.push(e.push(u, o.def, vec![x, v.int(i)]));
        e.push(u, o.get, vec![x]);
    }
    e.evaluate(4).unwrap();
    let x = f(v, "x5000");
    e.replace(defs[5000], o.def, vec![x, v.int(-5000)]);
    let t = Instant::now();
    let r = e.evaluate(4).unwrap();
    assert_eq!(r.nodes_run, 2, "{r:?}");
    assert!(t.elapsed() < Duration::from_millis(5), "an edit of two nodes among 20000 in {:?}", t.elapsed());
    assert!(e.check_invariants().is_empty());
}

#[test]
fn the_environment_reaches_its_readers() {
    let (e, o) = engine();
    let v = e.values();
    let en = v.field("e");
    e.set_environment([(en, v.int(5))]);
    let u = e.new_unit();
    let a = e.push(u, o.get, vec![en.0]);
    e.push(u, o.def, vec![en.0, v.int(7)]);
    let b = e.push(u, o.get, vec![en.0]);
    e.evaluate(2).unwrap();
    assert_eq!((int(&e, a), int(&e, b)), (5, 7));
    e.update_environment(&[(en, Some(v.int(6)))]);
    let r = e.evaluate(2).unwrap();
    assert_eq!((int(&e, a), int(&e, b)), (6, 7));
    assert_eq!(r.nodes_run, 1, "{r:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn claims_take_siblings() {
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let cl = e.push(u, o.claimer, vec![v.int(2)]);
    let x = e.push(u, o.get, vec![f(v, "p")]);
    let y = e.push(u, o.get, vec![f(v, "q")]);
    let z = e.push(u, o.get, vec![f(v, "r")]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(e.node_info(x).unwrap().state, NodeState::Inert);
    assert_eq!(e.node_info(y).unwrap().state, NodeState::Inert);
    assert_eq!(e.node_info(z).unwrap().state, NodeState::Done);
    let out = e.values().with_list(e.output(cl).unwrap(), <[ValueId]>::to_vec).unwrap();
    assert_eq!(out, vec![f(v, "p"), f(v, "q")]);
    assert_eq!(e.claimed(cl), vec![x, y]);
    assert_eq!(Report::by_op(&r.runs_by_op, "get"), 1);
}

#[test]
fn a_path_read_is_as_fine_as_the_reader() {
    let (e, o) = engine();
    let v = e.values();
    let l = f(v, "l");
    let u = e.new_unit();
    let d = e.push(u, o.def, vec![l, v.list(vec![v.int(1), v.int(2)])]);
    let i = e.push(u, o.idx, vec![l]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, i), 2);
    e.replace(d, o.def, vec![l, v.list(vec![v.int(9), v.int(2)])]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(r.nodes_run, 1, "element 1 is unchanged, the reader stands: {r:?}");
    e.replace(d, o.def, vec![l, v.list(vec![v.int(9), v.int(3)])]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(r.nodes_run, 2);
    assert_eq!(int(&e, i), 3);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_read_at_another_position() {
    let (e, o) = engine();
    let v = e.values();
    let x = f(v, "x");
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    let mid = e.push(u, o.get, vec![x]);
    let d2 = e.push(u, o.def, vec![x, v.int(2)]);
    let late = e.push(u, o.get_before, vec![x, v.int(mid.raw() as i64)]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, late), 1, "what reaches `mid`, not what reaches the reader");
    e.replace(d2, o.def, vec![x, v.int(5)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(int(&e, late), 1);
    assert_eq!(r.nodes_run, 1, "the late reader's anchor is before the change: {r:?}");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_refuted_run_stops_early_and_runs_again() {
    // A reader that runs before its definer commits reads the environment,
    // is re-asked by the definer's commit, and ends with the right value;
    // the engine's total is bounded by the real dependency, not by luck.
    for _ in 0..20 {
        let (e, o) = engine();
        let v = e.values();
        let x = f(v, "x");
        let u = e.new_unit();
        let slow = e.push(u, o.work, vec![v.int(200)]);
        let _ = slow;
        e.push(u, o.def, vec![x, v.int(4)]);
        let g = e.push(u, o.get, vec![x]);
        let r = e.evaluate(4).unwrap();
        assert_eq!(int(&e, g), 4);
        assert!(r.runs <= 4, "{r:?}");
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn a_commit_walks_only_to_the_next_definition() {
    let (e, o) = engine();
    let v = e.values();
    let x = f(v, "x");
    let u = e.new_unit();
    let mut defs = Vec::new();
    for i in 0..10000 {
        defs.push(e.push(u, o.def, vec![x, v.int(i)]));
        for _ in 0..9 {
            e.push(u, o.get, vec![x]);
        }
    }
    e.evaluate(4).unwrap();
    e.replace(defs[5000], o.def, vec![x, v.int(-1)]);
    let t = Instant::now();
    let r = e.evaluate(4).unwrap();
    assert_eq!(r.nodes_run, 10, "the definer and its nine readers up to the next definition: {r:?}");
    assert!(t.elapsed() < Duration::from_millis(2), "ten nodes among 100000 in {:?}", t.elapsed());
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_insert_shifts_a_counter_chain_side_by_side() {
    if threads() < 8 {
        return;
    }
    let (e, o) = engine();
    let v = e.values();
    let c = f(v, "c");
    let ms = v.int(1);
    let u = e.new_unit();
    e.push(u, o.def, vec![c, v.int(0)]);
    let rs: Vec<NodeId> = (0..1000).map(|_| e.push(u, o.routine, vec![c, ms])).collect();
    e.evaluate(8).unwrap();
    let before = o.routine_runs.0.load(Ordering::Relaxed);
    assert!(before <= 2000, "{before}");

    let t = Instant::now();
    e.insert_after(rs[499], o.routine, vec![c, ms]);
    let r = e.evaluate(8).unwrap();
    let wall = t.elapsed();
    for (i, n) in rs.iter().enumerate() {
        let want = if i < 500 { i } else { i + 1 };
        assert_eq!(int(&e, *n), want as i64, "routine {i} after the insert");
    }
    let runs = o.routine_runs.0.load(Ordering::Relaxed) - before;
    assert!(runs <= 501 + 10, "the inserted one and the 500 whose read shifted, once each: {runs}");
    assert!(r.relative_resolved >= 500, "the shifted relations are recomputed in place: {r:?}");
    assert!(wall < Duration::from_millis(250), "500 x 1 ms side by side on 8 is about 63 ms, serial is 500 ms: {wall:?}");
    assert!(e.check_invariants().is_empty());
}

/// `[k, n, salt]`: follows itself with k + 1 while k < n; outputs k.
struct Stepper(std::sync::OnceLock<OpId>);

impl Operation for Stepper {
    fn name(&self) -> &'static str {
        "stepper"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let (k, n) = (v.as_int(cx.args()[0]).unwrap(), v.as_int(cx.args()[1]).unwrap());
        if k < n {
            cx.follow(*self.0.get().unwrap(), vec![v.int(k + 1), cx.args()[1], v.int(0)]);
        }
        Ok(cx.args()[0])
    }
}

#[test]
fn a_continuation_chain_joins_on_an_unchanged_step() {
    let e = Engine::new();
    let st = Arc::new(Stepper(std::sync::OnceLock::new()));
    let op = e.register(st.clone());
    st.0.set(op).unwrap();
    let v = e.values();
    let u = e.new_unit();
    let first = e.push(u, op, vec![v.int(0), v.int(100), v.int(0)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(r.runs, 101);
    let chain = e.order(u);
    assert_eq!(chain.len(), 101, "each step placed the next after itself");
    assert_eq!(int(&e, chain[100]), 100);

    e.replace(first, op, vec![v.int(0), v.int(100), v.int(1)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(r.nodes_run, 1, "the first step places the same continuation: the chain joins: {r:?}");
    assert_eq!(e.order(u), chain);
    assert!(e.check_invariants().is_empty());
}

/// `[ms]`: spins `ms` milliseconds and contributes nothing, and says so.
struct Quiet;

impl Operation for Quiet {
    fn name(&self) -> &'static str {
        "quiet"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let ms = cx.values().as_int(cx.args()[0]).unwrap() as u64;
        spin(Duration::from_millis(ms));
        Ok(ValueId::UNIT)
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[s, n, ms]`: spins `ms` milliseconds, then outputs the first
/// contribution to s after node n.
struct NextAfter;

impl Operation for NextAfter {
    fn name(&self) -> &'static str {
        "next_after"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let s = Field(cx.args()[0]);
        let n = NodeId::from_raw(v.as_int(cx.args()[1]).unwrap() as u64);
        spin(Duration::from_millis(v.as_int(cx.args()[2]).unwrap() as u64));
        Ok(cx.next_contribution(s, Some(n))?.map_or(ValueId::UNIT, |x| x.1))
    }
}

#[test]
fn a_node_that_never_contributes_is_no_stop() {
    let (e, o) = engine();
    let quiet = e.register(Arc::new(Quiet));
    let next = e.register(Arc::new(NextAfter));
    let v = e.values();
    let u = e.new_unit();
    let s = f(v, "stream");
    let first = e.push(u, o.contrib, vec![s, v.int(1)]);
    e.push(u, quiet, vec![v.int(200)]);
    e.push(u, o.contrib, vec![s, v.int(2)]);
    let n = e.push(u, next, vec![s, v.int(first.raw() as i64), v.int(20)]);
    let r = e.evaluate(4).unwrap();
    assert_eq!(e.output(n), Some(v.int(2)));
    assert_eq!(Report::by_op(&r.runs_by_op, "next_after"), 1, "the reader ran once, past the quiet node still running: {r:?}");
}

#[test]
fn many_pushed_at_once_are_as_pushed_one_by_one() {
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let n = 300;
    let x = f(v, "x");
    e.push(u, o.def, vec![x, v.int(-1)]);
    let defs: Vec<Vec<ValueId>> = (0..n).map(|i| vec![f(v, &format!("y{i}")), v.int(i)]).collect();
    e.push_many(u, o.def, defs, 4);
    let gets: Vec<Vec<ValueId>> = (0..n).map(|i| vec![f(v, &format!("y{i}"))]).collect();
    let ids = e.push_many(u, o.get, gets, 4);
    let last = e.push(u, o.get, vec![x]);
    let r = e.evaluate(4).unwrap();
    // (a get may run before its definer, in another block, and once more)
    assert_eq!(r.nodes_run, 2 * n as u64 + 2);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(e.output(*id), Some(v.int(i as i64)), "get {i}");
    }
    assert_eq!(e.output(last), Some(v.int(-1)));
    assert!(e.check_invariants().is_empty());
}
