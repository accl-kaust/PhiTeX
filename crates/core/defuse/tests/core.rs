//! The core's specification: what the engine does, measured on toy
//! operations. Every test here passes before any client code is touched.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{
    At, Ctx, Engine, Field, FieldAt, Limits, NodeId, NodeState, OpId, Operation, Report, Step, Stop, ValueId, Values,
};

/// Whether the wall-clock bounds are checked: not with `PHITEX_SKIP_TIMING`
/// set (a loaded machine; the counts are checked always).
fn timing() -> bool {
    std::env::var_os("PHITEX_SKIP_TIMING").is_none()
}

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
    assert_eq!(
        Report::by_op(&r.runs_by_op, "adv"),
        1,
        "only the edited node runs: {r:?}"
    );
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
    assert!(
        runs <= 2 * 200,
        "each routine runs at most twice (predicted, then confirmed): {runs}"
    );
    assert!(r.shaped >= 190, "pending routines were given the relation: {r:?}");
    assert!(
        !timing() || wall < Duration::from_millis(150),
        "200 x 2 ms serial is 400 ms, twice over 8 threads is 100 ms: {wall:?}"
    );
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
    assert!(
        !timing() || avg_commit_ns <= 20_000,
        "a commit is a few microseconds, not {avg_commit_ns} ns: {:?}",
        r.times
    );
    // (on the CPU: a commit's wall time counts the time its thread was
    // off the CPU, preempted or asleep outside the engine, 1 to 3 ms at
    // times on a loaded machine with 0.03 to 0.09 ms of it on the CPU,
    // mc 76840/76841; measured with `profile` alone)
    assert!(
        !timing() || !cfg!(feature = "profile") || r.times.commit_max_cpu_ns < 1_000_000,
        "no commit takes a millisecond: {:?}",
        r.times
    );
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
    assert!(
        t8 * 5 < t1,
        "8 threads at least five times as fast as 1 on independent work: {t1:?} vs {t8:?}"
    );
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
    assert!(
        xs.iter()
            .enumerate()
            .all(|(i, x)| e.values().as_int(*x) == Some(i as i64))
    );

    e.replace(cs[2500], o.contrib, vec![s, v.int(-1)]);
    let t = Instant::now();
    let rep = e.evaluate(4).unwrap();
    assert!(
        !timing() || t.elapsed() < Duration::from_millis(10),
        "a fold over segment sums, not over 5000 items: {:?}",
        t.elapsed()
    );
    let xs = e.values().seq_to_vec(e.output(r).unwrap());
    assert_eq!(e.values().as_int(xs[2500]), Some(-1));
    assert_eq!(
        rep.nodes_run, 2,
        "the contributor and the fold's reader, not {}: {rep:?}",
        rep.nodes_run
    );
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
    assert!(
        !timing() || t.elapsed() < Duration::from_millis(5),
        "an edit of two nodes among 20000 in {:?}",
        t.elapsed()
    );
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
    let out = e
        .values()
        .with_list(e.output(cl).unwrap(), <[ValueId]>::to_vec)
        .unwrap();
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
    assert_eq!(
        r.nodes_run, 10,
        "the definer and its nine readers up to the next definition: {r:?}"
    );
    assert!(
        !timing() || t.elapsed() < Duration::from_millis(2),
        "ten nodes among 100000 in {:?}",
        t.elapsed()
    );
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
    assert!(
        runs <= 501 + 10,
        "the inserted one and the 500 whose read shifted, once each: {runs}"
    );
    assert!(
        r.relative_resolved >= 500,
        "the shifted relations are recomputed in place: {r:?}"
    );
    assert!(
        !timing() || wall < Duration::from_millis(250),
        "500 x 1 ms side by side on 8 is about 63 ms, serial is 500 ms: {wall:?}"
    );
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
    assert_eq!(
        r.nodes_run, 1,
        "the first step places the same continuation: the chain joins: {r:?}"
    );
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
    assert_eq!(
        Report::by_op(&r.runs_by_op, "next_after"),
        1,
        "the reader ran once, past the quiet node still running: {r:?}"
    );
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

/// `[k, ms]`: reads k (an integer; none: 0), spins `ms` milliseconds, then
/// claims that many siblings; outputs k.
struct ClaimK;

impl Operation for ClaimK {
    fn name(&self) -> &'static str {
        "claim_k"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let k = Field(cx.args()[0]);
        let n = cx.read(k)?.and_then(|x| cx.values().as_int(x)).unwrap_or(0);
        spin(Duration::from_millis(cx.values().as_int(cx.args()[1]).unwrap() as u64));
        cx.claim(n as usize);
        Ok(cx.values().int(n))
    }
}

/// `[f, v, ms]`: spins `ms` milliseconds, then defines f = v; outputs v.
struct SlowDef;

impl Operation for SlowDef {
    fn name(&self) -> &'static str {
        "slow_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, x) = (Field(cx.args()[0]), cx.args()[1]);
        spin(Duration::from_millis(cx.values().as_int(cx.args()[2]).unwrap() as u64));
        cx.define(f, x);
        Ok(x)
    }
}

/// Runs `e.evaluate(n)` on a thread of its own: None when it has not
/// returned in `limit` (the thread is left behind).
fn evaluate_within(e: &Arc<Engine>, n: usize, limit: Duration) -> Option<Report> {
    let (tx, rx) = std::sync::mpsc::channel();
    let e = e.clone();
    std::thread::spawn(move || {
        let _ = tx.send(e.evaluate(n).unwrap());
    });
    rx.recv_timeout(limit).ok()
}

#[test]
fn a_sibling_released_while_it_runs_ends_the_evaluation() {
    // A claimer takes the sibling after it while that one runs, then, run
    // again on a definition that lands later, gives it back while its
    // first run is still going: the sibling runs once more, and the
    // evaluation ends.
    let (e, o) = engine();
    let e = Arc::new(e);
    let claim_k = e.register(Arc::new(ClaimK));
    let slow_def = e.register(Arc::new(SlowDef));
    let v = e.values();
    let k = f(v, "k");
    e.set_environment([(Field(k), v.int(1))]);
    let u = e.new_unit();
    e.push(u, o.work, vec![v.int(0)]);
    e.push(u, slow_def, vec![k, v.int(0), v.int(40)]);
    let c = e.push(u, claim_k, vec![k, v.int(10)]);
    let s = e.push(u, o.work, vec![v.int(150_000)]);
    let r = evaluate_within(&e, 4, Duration::from_secs(10));
    assert!(r.is_some(), "the evaluation ends");
    assert_eq!(int(&e, c), 0);
    assert_eq!(e.node_info(s).unwrap().state, NodeState::Done);
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[f, v]`: defines f = v, and says so before it runs.
struct PredDef;

impl Operation for PredDef {
    fn name(&self) -> &'static str {
        "pred_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, v) = (Field(cx.args()[0]), cx.args()[1]);
        cx.define(f, v);
        Ok(v)
    }

    fn predicted_defs(&self, args: &[ValueId], _: &Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }
}

#[test]
fn a_claimed_sibling_leaves_no_prediction() {
    // A sibling taken before it ran never runs: what it was predicted to
    // define reaches nobody, as in the sequential evaluation.
    let (e, o) = engine();
    let pred = e.register(Arc::new(PredDef));
    let v = e.values();
    let x = f(v, "x");
    let u = e.new_unit();
    let cl = e.push(u, o.claimer, vec![v.int(1)]);
    let p = e.push(u, pred, vec![x, v.int(7)]);
    let g = e.push(u, o.get, vec![x]);
    e.evaluate(1).unwrap();
    assert_eq!(e.claimed(cl), vec![p]);
    assert_eq!(
        e.output(g),
        Some(ValueId::UNIT),
        "the taken sibling's predicted x reaches the reader"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[s, n]`: outputs the first contribution to s after node n; never
/// contributes, and says so.
struct NextQuiet;

impl Operation for NextQuiet {
    fn name(&self) -> &'static str {
        "next_quiet"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        let n = NodeId::from_raw(cx.values().as_int(cx.args()[1]).unwrap() as u64);
        Ok(cx.next_contribution(s, Some(n))?.map_or(ValueId::UNIT, |x| x.1))
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

/// `[n]`: claims the next n siblings; never contributes, and says so.
struct QuietClaimer;

impl Operation for QuietClaimer {
    fn name(&self) -> &'static str {
        "quiet_claimer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let n = cx.values().as_int(cx.args()[0]).unwrap() as usize;
        cx.claim(n);
        Ok(ValueId::UNIT)
    }

    fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
        false
    }
}

#[test]
fn a_next_stopped_at_a_claimed_sibling_is_asked_again() {
    // The first contribution after `first` is searched while the sibling
    // that may make it has not run, and stops there; that sibling is then
    // taken by a claim and never contributes: the reader is asked again
    // and finds the one after it.
    let (e, o) = engine();
    let next = e.register(Arc::new(NextQuiet));
    let quiet = e.register(Arc::new(QuietClaimer));
    let v = e.values();
    let s = f(v, "stream");
    let u = e.new_unit();
    let first = e.push(u, o.contrib, vec![s, v.int(1)]);
    let r = e.push(u, next, vec![s, v.int(first.raw() as i64)]);
    e.push(u, quiet, vec![v.int(1)]);
    e.push(u, o.contrib, vec![s, v.int(5)]);
    e.push(u, o.contrib, vec![s, v.int(2)]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(r), Some(v.int(2)));
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[test]
fn a_reader_at_a_removed_node_is_asked_again() {
    // A read at another node's position whose node is removed: the read
    // now finds it gone, and its reader runs again.
    let (e, o) = engine();
    let v = e.values();
    let x = f(v, "x");
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    let mid = e.push(u, o.work, vec![v.int(0)]);
    let late = e.push(u, o.get_before, vec![x, v.int(mid.raw() as i64)]);
    e.evaluate(2).unwrap();
    assert_eq!(int(&e, late), 1);
    e.remove(mid);
    let r = e.evaluate(2).unwrap();
    assert_eq!(
        e.output(late),
        Some(ValueId::UNIT),
        "the reader of a gone node's position ran again: {r:?}"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[k, ms, work]`: reads k (an integer; none: 0), spins `ms`
/// milliseconds, then places k children `Work [0]`; outputs k.
struct SpawnK(OpId);

impl Operation for SpawnK {
    fn name(&self) -> &'static str {
        "spawn_k"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let k = Field(cx.args()[0]);
        let n = cx.read(k)?.and_then(|x| cx.values().as_int(x)).unwrap_or(0);
        spin(Duration::from_millis(cx.values().as_int(cx.args()[1]).unwrap() as u64));
        for _ in 0..n {
            let zero = cx.values().int(0);
            cx.child(self.0, vec![zero]);
        }
        Ok(cx.values().int(n))
    }
}

/// `[f, n, ms]`: reads the definition of f reaching node n, spins `ms`
/// milliseconds; outputs what it read (UNIT if none or gone).
struct SlowGetBefore;

impl Operation for SlowGetBefore {
    fn name(&self) -> &'static str {
        "slow_get_before"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let n = NodeId::from_raw(cx.values().as_int(cx.args()[1]).unwrap() as u64);
        let x = match cx.read_at(f, At::Before(n))? {
            FieldAt::Def(v, _) => v,
            FieldAt::Unset | FieldAt::Gone => ValueId::UNIT,
        };
        spin(Duration::from_millis(cx.values().as_int(cx.args()[2]).unwrap() as u64));
        Ok(x)
    }
}

#[test]
fn a_reader_whose_anchor_goes_before_its_commit_runs_again() {
    // The node a read is at is removed while its reader still runs: the
    // reader's commit finds it gone, so its read no longer stands.
    let (e, o) = engine();
    let spawn = e.register(Arc::new(SpawnK(o.work)));
    let get = e.register(Arc::new(SlowGetBefore));
    let v = e.values();
    let (x, k) = (f(v, "x"), f(v, "k"));
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    let kd = e.push(u, o.def, vec![k, v.int(1)]);
    let p = e.push(u, spawn, vec![k, v.int(30)]);
    e.evaluate(2).unwrap();
    let n = e.children(p)[0];
    // (the caller's thread runs the edit first and then the parent, which
    // removes `n` at 30 ms; the reader reads at once and commits at 100 ms)
    e.replace(kd, o.def, vec![k, v.int(0)]);
    let r = e.push(u, get, vec![x, v.int(n.raw() as i64), v.int(100)]);
    e.evaluate(4).unwrap();
    assert!(!e.is_live(n));
    assert_eq!(
        e.output(r),
        Some(ValueId::UNIT),
        "the reader of a node gone before its commit ran again"
    );
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[f]`: `Get` on `[f]` inline (`Ctx::apply`); outputs what it gave.
struct ApplyGet(OpId);

impl Operation for ApplyGet {
    fn name(&self) -> &'static str {
        "apply_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.apply(self.0, vec![cx.args()[0]])
    }
}

#[test]
fn an_inline_run_reads_at_its_callers_position() {
    // `Ctx::apply` runs an operation at the caller's position and its reads
    // are the caller's: two callers on either side of a definition get what
    // reaches each, and an edit of that definition re-asks the one after.
    let (e, o) = engine();
    let ag = e.register(Arc::new(ApplyGet(o.get)));
    let v = e.values();
    let x = f(v, "x");
    let u = e.new_unit();
    e.push(u, o.def, vec![x, v.int(1)]);
    let a = e.push(u, ag, vec![x]);
    let d = e.push(u, o.def, vec![x, v.int(2)]);
    let b = e.push(u, ag, vec![x]);
    e.evaluate(1).unwrap();
    assert_eq!((int(&e, a), int(&e, b)), (1, 2));
    e.replace(d, o.def, vec![x, v.int(3)]);
    e.evaluate(1).unwrap();
    assert_eq!((int(&e, a), int(&e, b)), (1, 3));
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[ms]`: its predictions take `ms` milliseconds to make, and are none;
/// runs to nothing.
struct SlowPred;

impl Operation for SlowPred {
    fn name(&self) -> &'static str {
        "slow_pred"
    }

    fn run(&self, _: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(ValueId::UNIT)
    }

    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        spin(Duration::from_millis(v.as_int(args[0]).unwrap() as u64));
        None
    }
}

/// `[f, step, ms, child]`: spins `ms` milliseconds; f := f + 1 as a
/// relation if `step` is 1; places a `SlowPred [child]` if `child` is not 0.
struct MaybeAdv(OpId);

impl Operation for MaybeAdv {
    fn name(&self) -> &'static str {
        "maybe_adv"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let (f, step) = (Field(cx.args()[0]), v.as_int(cx.args()[1]).unwrap());
        spin(Duration::from_millis(v.as_int(cx.args()[2]).unwrap() as u64));
        if step == 1 {
            cx.define_rel(f, 1);
        }
        if v.as_int(cx.args()[3]).unwrap() != 0 {
            cx.child(self.0, vec![cx.args()[3]]);
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_shape_learned_while_a_node_commits_leaves_it_nothing() {
    // A node of an operation that steps a counter, whose run did not, is
    // committing (its placement's predictions take a while) when another
    // node of that operation steps it first: the relation given to the
    // committing node ahead of its run is withdrawn by its commit, not
    // left at its label to add one more.
    let (e, o) = engine();
    let slow = e.register(Arc::new(SlowPred));
    let op = e.register(Arc::new(MaybeAdv(slow)));
    let v = e.values();
    let c = f(v, "c");
    let (zero, one) = (v.int(0), v.int(1));
    let u = e.new_unit();
    e.push(u, o.def, vec![c, zero]);
    let n = e.push(u, op, vec![c, zero, zero, v.int(60)]);
    e.push(u, op, vec![c, one, v.int(15), zero]);
    let g = e.push(u, o.get, vec![c]);
    e.evaluate(4).unwrap();
    assert_eq!(e.defs_of(n), vec![]);
    assert_eq!(int(&e, g), 1, "one step of the counter before the reader");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[test]
fn an_evaluation_that_fails_loses_no_node() {
    // An evaluation stopped by a limit leaves its queued nodes, and the one
    // it stopped at, to the next: that one runs them all.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let mut gets = Vec::new();
    for i in 0..200 {
        let x = f(v, &format!("x{i}"));
        e.push(u, o.def, vec![x, v.int(i)]);
        gets.push(e.push(u, o.get, vec![x]));
    }
    e.set_limits(Limits {
        total_runs: Some(50),
        ..Limits::default()
    });
    assert!(e.evaluate(2).is_err());
    e.set_limits(Limits::default());
    e.evaluate(2).unwrap();
    for (i, g) in gets.iter().enumerate() {
        assert_eq!(e.output(*g), Some(v.int(i as i64)), "get {i}");
    }
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[test]
fn a_claim_reads_its_siblings_in_one_pass() {
    // A claimer that reads the siblings it takes one after another, as a
    // run pulling lines does: each read goes on from the one before, not
    // from the claimer, and its commit checks them the same way.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let n = 4000;
    let cl = e.push(u, o.claimer, vec![v.int(n)]);
    for i in 0..n {
        e.push(u, o.get, vec![v.int(i)]);
    }
    let t = Instant::now();
    e.evaluate(1).unwrap();
    let wall = t.elapsed();
    assert_eq!(e.claimed(cl).len(), n as usize);
    assert!(
        !timing() || wall < Duration::from_millis(80),
        "a claim of {n} read in {wall:?}: one pass is about 15 ms, one per sibling about 250"
    );
}

/// Integer sum whose every combine takes 300 microseconds.
struct SlowSum;

impl phitex_defuse::Monoid for SlowSum {
    fn empty(&self, v: &Values) -> ValueId {
        v.int(0)
    }

    fn combine(&self, v: &Values, a: ValueId, b: ValueId) -> ValueId {
        spin(Duration::from_micros(300));
        v.int(v.as_int(a).unwrap_or(0) + v.as_int(b).unwrap_or(0))
    }
}

#[test]
fn a_fold_sums_a_segment_outside_its_lock() {
    // Folds over a segment whose sum is not cached compute it under no
    // lock: the readers and the commits of that segment do not wait for
    // the monoid.
    let (e, o) = engine();
    let v = e.values();
    let s = f(v, "s");
    e.declare_accumulator(Field(s), Arc::new(SlowSum));
    let u = e.new_unit();
    for i in 0..60 {
        e.push(u, o.contrib, vec![s, v.int(i)]);
        if i % 6 == 5 {
            e.push(u, o.fold, vec![s]);
        }
    }
    let r = e.evaluate(8).unwrap();
    assert!(
        !timing() || r.times.lock_wait_max_ns < 3_000_000,
        "a lock waited for a fold's sum: {:?}",
        r.times
    );
}

#[test]
fn a_node_after_a_long_chain_is_placed() {
    // A node inserted after the head of a continuation chain goes after
    // the chain's end, found without a frame per step.
    let e = Engine::new();
    let st = Arc::new(Stepper(std::sync::OnceLock::new()));
    let op = e.register(st.clone());
    st.0.set(op).unwrap();
    let v = e.values();
    let u = e.new_unit();
    let n = 100_000;
    let first = e.push(u, op, vec![v.int(0), v.int(n), v.int(0)]);
    e.evaluate(4).unwrap();
    let x = e.insert_after(first, op, vec![v.int(n), v.int(n), v.int(0)]);
    assert_eq!(*e.order(u).last().unwrap(), x);
}

#[test]
fn a_long_chain_cut_at_its_head_is_removed() {
    // The head of a continuation chain run again with nothing to follow:
    // its commit removes the whole chain after it, without a frame per
    // step, on a worker's thread.
    let e = Engine::new();
    let st = Arc::new(Stepper(std::sync::OnceLock::new()));
    let op = e.register(st.clone());
    st.0.set(op).unwrap();
    let v = e.values();
    let u = e.new_unit();
    let n = 100_000;
    let first = e.push(u, op, vec![v.int(0), v.int(n), v.int(0)]);
    e.evaluate(4).unwrap();
    e.replace(first, op, vec![v.int(0), v.int(0), v.int(0)]);
    e.evaluate(4).unwrap();
    assert_eq!(e.order(u), vec![first]);
    assert_eq!(e.live_nodes(), 1);
}

/// `[]`: places a `Work [0]` after itself and its placements
/// (`Ctx::follow_after` of itself).
struct FollowSelf(OpId);

impl Operation for FollowSelf {
    fn name(&self) -> &'static str {
        "follow_self"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let me = cx.node();
        let zero = cx.values().int(0);
        cx.follow_after(me, self.0, vec![zero]);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_placement_after_its_own_placer_is_placed() {
    // A node placed after a subtree that holds its own placer: the placer's
    // record names it before it is on the line, and finding the subtree's
    // end passes over it.
    let (e, o) = engine();
    let op = e.register(Arc::new(FollowSelf(o.work)));
    let u = e.new_unit();
    let p = e.push(u, op, vec![]);
    e.evaluate(1).unwrap();
    let kids = e.children(p);
    assert_eq!(kids.len(), 1);
    assert_eq!(e.order(u), vec![p, kids[0]]);
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[f]`: outputs the environment's f, whatever is defined before it
/// (UNIT if none).
struct EnvGet;

impl Operation for EnvGet {
    fn name(&self) -> &'static str {
        "env_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.env(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn an_environment_reader_outlives_a_change_that_leaves_it() {
    // The environment changed in a field to what a reader of it saw: the
    // reader stands, and is still asked at the next change that is not.
    let (e, _) = engine();
    let get = e.register(Arc::new(EnvGet));
    let v = e.values();
    let x = v.field("file:a");
    let u = e.new_unit();
    let r = e.push(u, get, vec![x.0]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(r), Some(ValueId::UNIT));
    e.update_environment(&[(x, None)]);
    let rep = e.evaluate(2).unwrap();
    assert_eq!(rep.nodes_run, 0, "{rep:?}");
    e.update_environment(&[(x, Some(v.int(5)))]);
    e.evaluate(2).unwrap();
    assert_eq!(e.output(r), Some(v.int(5)), "the reader of the environment ran again");
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

#[cfg(feature = "profile")]
#[global_allocator]
static ALLOC: phitex_defuse::obs::CountingAlloc<std::alloc::System> =
    phitex_defuse::obs::CountingAlloc(std::alloc::System);

#[test]
fn a_commit_allocates_nothing_under_its_lock() {
    // The allocator's own locks (glibc's arenas) are never taken where
    // other threads wait for a segment: what a commit files is made
    // before the lock, its scratch kept by its thread.
    if !cfg!(feature = "profile") {
        return;
    }
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    for i in 0..4000 {
        let x = f(v, &format!("x{i}"));
        e.push(u, o.def, vec![x, v.int(i)]);
        e.push(u, o.get, vec![x]);
    }
    let r = e.evaluate(4).unwrap();
    assert!(phitex_defuse::obs::counting_allocs());
    assert_eq!(
        (r.times.commit_allocs, r.times.commit_frees),
        (0, 0),
        "allocations and frees under a commit's lock over {} commits",
        r.commits
    );
}

#[test]
fn a_commit_that_places_and_contributes_allocates_nothing_under_its_lock() {
    // Placements, contributions and folds, reads at other nodes, and an
    // edit that makes placements anew and takes others back: nothing
    // under a commit's lock either.
    if !cfg!(feature = "profile") {
        return;
    }
    let (e, o) = engine();
    let v = e.values();
    let s = f(v, "s");
    e.declare_accumulator(Field(s), Arc::new(phitex_defuse::SumMonoid));
    let u = e.new_unit();
    let mut spawns = Vec::new();
    for i in 0..400 {
        spawns.push(e.push(u, o.spawn, vec![v.int(4), v.int(i)]));
        e.push(u, o.contrib, vec![s, v.int(i)]);
        e.push(u, o.fold, vec![s]);
        let x = f(v, &format!("child{}", i % 4));
        e.push(u, o.get, vec![x]);
        let d = e.push(u, o.def, vec![x, v.int(i)]);
        e.push(u, o.get_before, vec![x, v.int(i64::try_from(d.raw()).unwrap())]);
    }
    let cold = e.evaluate(4).unwrap();
    for (i, &n) in spawns.iter().enumerate().step_by(7) {
        e.replace(n, o.spawn, vec![v.int(2 + (i % 2) as i64 * 4), v.int(i as i64)]);
    }
    let edit = e.evaluate(4).unwrap();
    assert!(phitex_defuse::obs::counting_allocs());
    for (what, r) in [("cold", &cold), ("edit", &edit)] {
        assert_eq!(
            (r.times.commit_allocs, r.times.commit_frees),
            (0, 0),
            "{what}: allocations and frees under a commit's lock over {} commits",
            r.commits
        );
    }
    assert!(e.check_invariants().is_empty());
}
