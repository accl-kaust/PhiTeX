//! Claims among placed nodes and loop-back re-asks, under threads: the
//! interleavings a client's text pieces meet (a piece read on by its
//! siblings, a continuation placed among them, a file read back last).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use phitex_defuse::{At, Ctx, Engine, Field, FieldAt, NodeId, NodeState, OpId, Operation, Stop, ValueId};

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

/// `[f, v, ms]`: spins `ms` milliseconds, then defines f = v.
struct SlowDef;

impl Operation for SlowDef {
    fn name(&self) -> &'static str {
        "slow_def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (f, x) = (Field(cx.args()[0]), cx.args()[1]);
        let ms = cx.values().as_int(cx.args()[2]).unwrap() as u64;
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            std::hint::spin_loop();
        }
        cx.define(f, x);
        Ok(x)
    }
}

/// `[x]`: outputs x.
struct Text;

impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.args()[0])
    }
}

/// `[n]`: claims its next n siblings; outputs their first arguments.
struct Claimer;

impl Operation for Claimer {
    fn name(&self) -> &'static str {
        "claimer"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let n = cx.values().as_int(cx.args()[0]).unwrap() as usize;
        let mut xs = Vec::new();
        for i in 0..n {
            if let Some((_, args)) = cx.following(i) {
                xs.push(args[0]);
            }
        }
        cx.claim(n);
        Ok(cx.values().list(xs))
    }
}

/// `[n, k]`: places a `Peek` child, and claims its next n siblings when
/// `k` is 1 (none when 0).
struct ClaimPlace(std::sync::OnceLock<OpId>);

impl Operation for ClaimPlace {
    fn name(&self) -> &'static str {
        "claim_place"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap() as usize;
        let on = v.as_int(cx.args()[1]).unwrap() != 0;
        let peek = *self.0.get().expect("registered");
        cx.child(peek, vec![v.int(0)]);
        if on {
            for i in 0..n {
                let _ = cx.following(i);
            }
            cx.claim(n);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[_]`: outputs its next sibling's first argument (UNIT if none),
/// claiming nothing: a continuation reading on.
struct Peek;

impl Operation for Peek {
    fn name(&self) -> &'static str {
        "peek"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.following(0).map_or(ValueId::UNIT, |(_, a)| a[0]))
    }
}

/// `[f]`: outputs f's last definition on the line; counts its runs.
struct FinalGet(Arc<AtomicU64>);

impl Operation for FinalGet {
    fn name(&self) -> &'static str {
        "final_get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let f = Field(cx.args()[0]);
        Ok(match cx.read_at(f, At::Final)? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        })
    }
}

struct Ops {
    def: OpId,
    slow_def: OpId,
    text: OpId,
    claimer: OpId,
    claim_place: OpId,
    final_get: OpId,
    final_runs: Arc<AtomicU64>,
}

fn engine() -> (Engine, Ops) {
    let e = Engine::new();
    let cp = Arc::new(ClaimPlace(std::sync::OnceLock::new()));
    let final_runs = Arc::new(AtomicU64::new(0));
    let ops = Ops {
        def: e.register(Arc::new(Def)),
        slow_def: e.register(Arc::new(SlowDef)),
        text: e.register(Arc::new(Text)),
        claimer: e.register(Arc::new(Claimer)),
        claim_place: e.register(cp.clone()),
        final_get: e.register(Arc::new(FinalGet(final_runs.clone()))),
        final_runs,
    };
    let _ = cp.0.set(e.register(Arc::new(Peek)));
    (e, ops)
}

fn op_nodes(e: &Engine, u: phitex_defuse::UnitId, op: &str) -> Vec<NodeId> {
    e.order(u)
        .into_iter()
        .filter(|&n| e.node_info(n).is_some_and(|i| i.op == op))
        .collect()
}

#[test]
fn a_sibling_a_later_claim_holds_is_taken_with_it() {
    // B holds C and D; A, made to claim three, takes B and with it what B
    // held, in order, not passing over C and D to E.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let a = e.push(u, o.claimer, vec![v.int(0)]);
    let b = e.push(u, o.claimer, vec![v.int(2)]);
    let c = e.push(u, o.text, vec![v.str("c")]);
    let d = e.push(u, o.text, vec![v.str("d")]);
    let x = e.push(u, o.text, vec![v.str("e")]);
    e.evaluate(2).unwrap();
    assert_eq!(e.claimed(b), vec![c, d]);
    e.replace(a, o.claimer, vec![v.int(3)]);
    e.evaluate(4).unwrap();
    let out = v.with_list(e.output(a).unwrap(), <[ValueId]>::to_vec).unwrap();
    assert_eq!(out, vec![v.int(2), v.str("c"), v.str("d")]);
    assert_eq!(e.claimed(a), vec![b, c, d]);
    for n in [b, c, d] {
        assert_eq!(e.node_info(n).unwrap().state, NodeState::Inert);
        assert_eq!(e.node_info(n).unwrap().claimer, Some(a));
    }
    assert_eq!(e.node_info(x).unwrap().state, NodeState::Done);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_sibling_released_re_asks_the_node_that_passed_it() {
    // P holds S and places K between them; K reads on past S (an earlier
    // claim holds it) to T. P made to claim nothing gives S back, and K,
    // whose answer moved though nothing it read was defined, reads S.
    for threads in [1, 4] {
        let (e, o) = engine();
        let v = e.values();
        let u = e.new_unit();
        let p = e.push(u, o.claim_place, vec![v.int(1), v.int(1)]);
        let s = e.push(u, o.text, vec![v.str("s")]);
        e.push(u, o.text, vec![v.str("t")]);
        e.evaluate(threads).unwrap();
        let k = op_nodes(&e, u, "peek")[0];
        assert_eq!(e.output(k), Some(v.str("t")), "-j{threads}");
        assert_eq!(e.node_info(s).unwrap().claimer, Some(p));
        e.replace(p, o.claim_place, vec![v.int(1), v.int(0)]);
        e.evaluate(threads).unwrap();
        let k = op_nodes(&e, u, "peek")[0];
        assert_eq!(e.output(k), Some(v.str("s")), "-j{threads}");
        assert_eq!(e.node_info(s).unwrap().state, NodeState::Done);
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn a_removed_claimer_gives_back_what_it_held() {
    // C holds S and T; C removed, both run, held by no one.
    let (e, o) = engine();
    let v = e.values();
    let u = e.new_unit();
    let c = e.push(u, o.claimer, vec![v.int(2)]);
    let s = e.push(u, o.text, vec![v.str("s")]);
    let t = e.push(u, o.text, vec![v.str("t")]);
    e.evaluate(2).unwrap();
    assert_eq!(e.claimed(c), vec![s, t]);
    e.remove(c);
    e.evaluate(2).unwrap();
    for n in [s, t] {
        let i = e.node_info(n).unwrap();
        assert_eq!((i.state, i.claimer), (NodeState::Done, None));
    }
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_loopback_reader_runs_again_once_the_line_is_quiet() {
    // R reads f at its last definition; W1 defines it at once, W2 after a
    // long run. R re-asked as W1 lands is held while W2 runs, not run on a
    // value about to move, and the evaluation does not end over it: R
    // runs at most twice and ends with W2's value.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    let (e, o) = engine();
    let v = e.values();
    let fv = v.field("f").0;
    let u = e.new_unit();
    let r = e.push(u, o.final_get, vec![fv]);
    e.push(u, o.def, vec![fv, v.int(1)]);
    e.push(u, o.slow_def, vec![fv, v.int(2), v.int(40)]);
    let rep = e.evaluate(threads).unwrap();
    assert_eq!(e.output(r), Some(v.int(2)));
    assert!(
        o.final_runs.load(Ordering::Relaxed) <= 2,
        "{} runs: {rep:?}",
        o.final_runs.load(Ordering::Relaxed)
    );
    assert!(rep.capped.is_empty());
    assert!(e.check_invariants().is_empty());
}

/// `[f]`: outputs f's definition reaching it; panics unless it is 2.
struct PanicUnless2;

impl Operation for PanicUnless2 {
    fn name(&self) -> &'static str {
        "panic_unless_2"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT);
        assert!(cx.values().as_int(x) == Some(2), "not 2");
        Ok(x)
    }
}

#[test]
fn a_panic_on_a_value_about_to_move_is_run_again_and_one_on_settled_values_fails() {
    // R panics unless f is 2; W1 defines it 1 at once, W2 2 after a long
    // run. R run before W2 lands panics on a value about to move, and runs
    // again; R after a definition of 1 that stays fails.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    for j in [1, threads] {
        let (e, o) = engine();
        let pu = e.register(Arc::new(PanicUnless2));
        let v = e.values();
        let fv = v.field("f").0;
        let u = e.new_unit();
        e.push(u, o.def, vec![fv, v.int(1)]);
        e.push(u, o.slow_def, vec![fv, v.int(2), v.int(40)]);
        let r = e.push(u, pu, vec![fv]);
        e.evaluate(j).unwrap();
        assert_eq!(e.output(r), Some(v.int(2)), "-j{j}");
        assert!(e.check_invariants().is_empty());
        let w = e.push(u, o.def, vec![fv, v.int(1)]);
        let r2 = e.push(u, pu, vec![fv]);
        let err = e.evaluate(j).unwrap_err();
        assert!(
            matches!(err, phitex_defuse::EvalError::OpPanicked { node, .. } if node == r2),
            "-j{j}: {err:?}"
        );
        e.remove(w);
        e.evaluate(j).unwrap();
        assert_eq!(e.output(r2), Some(v.int(2)), "-j{j}");
    }
}

/// `[text]`: places a `Text` child, reads its output (a guess until it
/// has run), then panics: a read of the run's own placement, which the
/// panic never commits.
struct PlacesThenPanics(OpId);

impl Operation for PlacesThenPanics {
    fn name(&self) -> &'static str {
        "places_then_panics"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let i = cx.child(self.0, vec![cx.values().int(5)]);
        let _ = cx.placed_output_or(i, ValueId::UNIT);
        panic!("always");
    }
}

#[test]
fn a_panic_after_a_guess_of_its_own_placement_fails_not_runs_forever() {
    // R, never committed, panics after reading a guess of its own
    // placement's output: no run before placed it, so a run again is
    // answered the guess again; the read holds, and the evaluation fails
    // with R's panic (it ran R again forever, the read judged against a
    // last run R never had).
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    for j in [1, threads] {
        let (e, o) = engine();
        let pp = e.register(Arc::new(PlacesThenPanics(o.text)));
        let v = e.values();
        let fv = v.field("f").0;
        let u = e.new_unit();
        e.push(u, o.def, vec![fv, v.int(1)]);
        let r = e.push(u, pp, vec![]);
        let err = e.evaluate(j).unwrap_err();
        assert!(
            matches!(err, phitex_defuse::EvalError::OpPanicked { node, .. } if node == r),
            "-j{j}: {err:?}"
        );
    }
}

/// `[f]`: outputs f's published last definition, keeping every value it
/// was answered.
struct SeenFinal(Arc<std::sync::Mutex<Vec<ValueId>>>);

impl Operation for SeenFinal {
    fn name(&self) -> &'static str {
        "seen_final"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let x = match cx.read_at(Field(cx.args()[0]), At::Final)? {
            FieldAt::Def(x, _) => x,
            _ => ValueId::UNIT,
        };
        self.0.lock().unwrap().push(x);
        Ok(x)
    }
}

#[test]
fn a_loopback_reader_never_sees_a_part_of_the_pass_being_written() {
    // W1 defines f at once, W2 again after a long run; R between them
    // reads f's last definition. Whatever runs when, R is answered what
    // the line held when it was last quiet: nothing, then W2's value,
    // never W1's, which is no whole state of the line.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    for j in [1, threads] {
        let (e, o) = engine();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sf = e.register(Arc::new(SeenFinal(seen.clone())));
        let v = e.values();
        let fv = v.field("f").0;
        let u = e.new_unit();
        e.push(u, o.def, vec![fv, v.int(1)]);
        let r = e.push(u, sf, vec![fv]);
        e.push(u, o.slow_def, vec![fv, v.int(2), v.int(40)]);
        let rep = e.evaluate(j).unwrap();
        assert_eq!(e.output(r), Some(v.int(2)), "-j{j}");
        assert_eq!(*seen.lock().unwrap(), vec![ValueId::UNIT, v.int(2)], "-j{j}");
        assert_eq!(rep.final_reruns, 1, "-j{j}: {rep:?}");
        assert!(e.check_invariants().is_empty());
    }
}

/// `[f, us]`: spins `us` microseconds, then defines f = 1.
struct SlowOne;

impl Operation for SlowOne {
    fn name(&self) -> &'static str {
        "slow_one"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let us = cx.values().as_int(cx.args()[1]).unwrap() as u64;
        let t = Instant::now();
        while t.elapsed() < Duration::from_micros(us) {
            std::hint::spin_loop();
        }
        cx.define(Field(cx.args()[0]), cx.values().int(1));
        Ok(ValueId::UNIT)
    }
}

/// `[f, us]`: places `Grow [f's value, us]`.
struct Parent(OpId);

impl Operation for Parent {
    fn name(&self) -> &'static str {
        "parent"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(v.int(0));
        cx.child(self.0, vec![x, cx.args()[1]]);
        Ok(ValueId::UNIT)
    }
}

/// `[x, us]`: spins `us` microseconds, then places three texts.
struct Grow(OpId);

impl Operation for Grow {
    fn name(&self) -> &'static str {
        "grow"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let us = cx.values().as_int(cx.args()[1]).unwrap() as u64;
        let t = Instant::now();
        while t.elapsed() < Duration::from_micros(us) {
            std::hint::spin_loop();
        }
        for k in 0..3 {
            cx.child(self.0, vec![cx.values().int(k)]);
        }
        Ok(cx.args()[0])
    }
}

#[test]
fn a_node_removed_as_it_commits_takes_its_placements_with_it() {
    // P places G on what it read of f; S defines f after a while, P runs
    // again and G gives way to another while G's own run, as long, places
    // its children: whichever comes first, no node is left on the line
    // that its placer no longer holds.
    let threads = std::thread::available_parallelism().map_or(1, usize::from).clamp(2, 8);
    for i in 0..400 {
        let (e, o) = engine();
        let grow = e.register(Arc::new(Grow(o.text)));
        let parent = e.register(Arc::new(Parent(grow)));
        let slow = e.register(Arc::new(SlowOne));
        let v = e.values();
        let f = v.field("f").0;
        let u = e.new_unit();
        e.push(u, slow, vec![f, v.int(200)]);
        e.push(u, parent, vec![f, v.int(150 + (i % 100))]);
        e.evaluate(threads).unwrap();
        let bad = e.check_invariants();
        assert!(bad.is_empty(), "round {i}: {bad:?}");
        assert_eq!(e.order(u).len(), 2 + 1 + 3, "round {i}");
    }
}

/// `[f]`: places `Text [0]` as its child and `Text [f's value]` as its
/// continuation.
struct ReadOn(OpId);

impl Operation for ReadOn {
    fn name(&self) -> &'static str {
        "read_on"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let x = cx.read(Field(cx.args()[0]))?.unwrap_or(v.int(0));
        cx.child(self.0, vec![v.int(0)]);
        cx.follow(self.0, vec![x]);
        Ok(ValueId::UNIT)
    }
}

/// `[n]`: places `Text ["a"]` after node n and its children.
struct After(OpId);

impl Operation for After {
    fn name(&self) -> &'static str {
        "after"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = NodeId::from_raw(v.as_int(cx.args()[0]).unwrap() as u64);
        cx.follow_after(n, self.0, vec![v.str("a")]);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_continuation_made_anew_stays_after_what_was_placed_after_its_node() {
    // X places its child and its continuation C; Y places A after X and
    // its children, before C (a page fired in X). X run again makes C
    // anew: the new one takes C's place, after A, not before it.
    let (e, o) = engine();
    let read_on = e.register(Arc::new(ReadOn(o.text)));
    let after = e.register(Arc::new(After(o.text)));
    let v = e.values();
    let f = v.field("f").0;
    let u = e.new_unit();
    let s = e.push(u, o.def, vec![v.field("g").0, v.int(1)]);
    let x = e.push(u, read_on, vec![f]);
    e.push(u, after, vec![v.int(x.raw() as i64)]);
    e.evaluate(1).unwrap();
    let texts = |e: &Engine| -> Vec<ValueId> {
        e.order(u)
            .into_iter()
            .filter(|&n| e.node_info(n).is_some_and(|i| i.op == "text"))
            .map(|n| e.node_info(n).unwrap().args[0])
            .collect()
    };
    assert_eq!(texts(&e), vec![v.int(0), v.str("a"), v.int(0)]);
    e.replace(s, o.def, vec![f, v.int(2)]);
    e.evaluate(1).unwrap();
    assert_eq!(texts(&e), vec![v.int(0), v.str("a"), v.int(2)]);
    assert!(e.check_invariants().is_empty());
}

/// `[f, v]`: predicts f = v (its last build's record) and defines it.
struct PredictedDef;

impl Operation for PredictedDef {
    fn name(&self) -> &'static str {
        "predicted_def"
    }

    fn predicted_defs(&self, args: &[ValueId], _: &phitex_defuse::Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(Field(args[0]), args[1])])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[f, k]`: k = 0 defines f = 1; k = 1 defines nothing and places
/// `PredictedDef [f, 5]` as its child.
struct DefOrPlace(OpId);

impl Operation for DefOrPlace {
    fn name(&self) -> &'static str {
        "def_or_place"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let f = Field(cx.args()[0]);
        if v.as_int(cx.args()[1]) == Some(0) {
            cx.define(f, v.int(1));
        } else {
            cx.child(self.0, vec![f.0, v.int(5)]);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: outputs the definition of f reaching it.
struct GetF;

impl Operation for GetF {
    fn name(&self) -> &'static str {
        "get_f"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_commit_that_stops_defining_a_field_its_placement_predicts_keeps_its_segment_indexed() {
    // P defines f, then runs again defining nothing and placing C, whose
    // prediction defines f in the same segment: the commit's removal of
    // the segment's last definition of f and the placement's filing of
    // the first again leave the segment in f's index, and R after them
    // reads C's value.
    let (e, _) = engine();
    let pd = e.register(Arc::new(PredictedDef));
    let p_op = e.register(Arc::new(DefOrPlace(pd)));
    let get = e.register(Arc::new(GetF));
    let v = e.values();
    let f = v.field("f").0;
    let u = e.new_unit();
    let p = e.push(u, p_op, vec![f, v.int(0)]);
    let r = e.push(u, get, vec![f]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(r), Some(v.int(1)));
    e.replace(p, p_op, vec![f, v.int(1)]);
    e.evaluate(1).unwrap();
    assert_eq!(e.output(r), Some(v.int(5)));
    assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
}

/// `[x, y]`: places a `Claimer` of one sibling, then a `Text` of x and
/// one of y: a paragraph's first line reading on into the next, as a
/// document region's run places its lines.
struct Lines(std::sync::OnceLock<(OpId, OpId)>);

impl Operation for Lines {
    fn name(&self) -> &'static str {
        "lines"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let (claimer, text) = *self.0.get().expect("registered");
        let (x, y) = (cx.args()[0], cx.args()[1]);
        let one = cx.values().int(1);
        cx.child(claimer, vec![one]);
        cx.child(text, vec![x]);
        cx.child(text, vec![y]);
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_taken_placement_remade_re_asks_its_claimer() {
    // L places C (claiming one sibling), X and Y; C takes X. L's text
    // edited, its run remakes X as X' (other args) and keeps C and Y: C
    // reads X' and takes it, not keeping X's text while X' runs on its own.
    for threads in [1, 4] {
        let (e, o) = engine();
        let v = e.values();
        let lines = Arc::new(Lines(std::sync::OnceLock::new()));
        let l_op = e.register(lines.clone());
        let _ = lines.0.set((o.claimer, o.text));
        let u = e.new_unit();
        let l = e.push(u, l_op, vec![v.str("x"), v.str("y")]);
        e.evaluate(threads).unwrap();
        let c = op_nodes(&e, u, "claimer")[0];
        let out = |c| v.with_list(e.output(c).unwrap(), <[ValueId]>::to_vec).unwrap();
        assert_eq!(out(c), vec![v.str("x")], "-j{threads}");
        e.replace(l, l_op, vec![v.str("x2"), v.str("y")]);
        e.evaluate(threads).unwrap();
        assert_eq!(op_nodes(&e, u, "claimer"), vec![c], "-j{threads}");
        assert_eq!(out(c), vec![v.str("x2")], "-j{threads}");
        let texts = op_nodes(&e, u, "text");
        assert_eq!(texts.len(), 2, "-j{threads}");
        assert_eq!(e.claimed(c), vec![texts[0]], "-j{threads}");
        assert_eq!(e.node_info(texts[0]).unwrap().state, NodeState::Inert, "-j{threads}");
        assert_eq!(e.node_info(texts[1]).unwrap().state, NodeState::Done, "-j{threads}");
        assert!(e.check_invariants().is_empty());
    }
}

/// `[x, y, t, z]`: defines the fields `x`, `y`, `t` and `z` as given.
struct Def4;

impl Operation for Def4 {
    fn name(&self) -> &'static str {
        "def4"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        for (i, name) in ["x", "y", "t", "z"].into_iter().enumerate() {
            cx.define(v.field(name), cx.args()[i]);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads `x`; where it is 1, spins 300 ms and claims its next
/// sibling.
struct ClaimIfLate;

impl Operation for ClaimIfLate {
    fn name(&self) -> &'static str {
        "claim_if_late"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let x = cx.read(v.field("x"))?.and_then(|x| v.as_int(x));
        if x == Some(1) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(300) {
                std::hint::spin_loop();
            }
            let _ = cx.following(0);
            cx.claim(1);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[k]`: a node of the settling layer that runs only at a quiet point
/// (`final_only`, `read_back`): predicted to define `s` = 7 before it runs, defines it
/// so when it does.
struct QuietSettler;

impl Operation for QuietSettler {
    fn name(&self) -> &'static str {
        "quiet_settler"
    }

    fn final_only(&self) -> bool {
        true
    }

    // (a writer read back: let go at the quiet point with the runs
    // waiting for it, not after the last pass)
    fn read_back(&self) -> bool {
        true
    }

    fn settles_at_quiet(&self) -> bool {
        true
    }

    fn predicted_defs(&self, _args: &[ValueId], values: &phitex_defuse::Values) -> Option<Vec<(Field, ValueId)>> {
        Some(vec![(values.field("s"), values.int(7))])
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        cx.define(v.field("s"), v.int(7));
        Ok(ValueId::UNIT)
    }
}

/// `[]`: reads `y` and `s` (a read of the settling node's prediction
/// waits for its commit, `Ctx::read`), then defines `z` = `y`; counts
/// the runs that stopped to wait.
struct ReadsSettler(Arc<AtomicU64>);

impl Operation for ReadsSettler {
    fn name(&self) -> &'static str {
        "reads_settler"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let y = cx.read(v.field("y"))?.unwrap_or(ValueId::UNIT);
        if let Err(stop) = cx.read(v.field("s")) {
            self.0.fetch_add(1, Ordering::Relaxed);
            return Err(stop);
        }
        cx.define(v.field("z"), y);
        Ok(y)
    }
}

/// `[]`: reads `z` and `t`; while both are set and differ, goes on
/// checking whether a read moved (`Ctx::tick`): a run on a value that
/// only a re-ask ends, as a TeX loop on a stale counter.
struct UntilEqual;

impl Operation for UntilEqual {
    fn name(&self) -> &'static str {
        "until_equal"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let z = cx.read(v.field("z"))?;
        let t = cx.read(v.field("t"))?;
        if let (Some(z), Some(t)) = (z, t)
            && z != t
        {
            loop {
                cx.tick(1)?;
                std::hint::spin_loop();
            }
        }
        Ok(ValueId::UNIT)
    }
}

/// A run parked waiting for a node's commit goes on when a claim takes
/// that node, which then commits nothing. W reads the prediction of P, a
/// settling node that runs only at a quiet point, and waits for P's
/// commit; C, slow, claims P; R runs on until W's definition of `z`
/// reaches it. Left parked until a quiet point that R keeps away, W
/// never ran again and R ran for ever (csquotes -j8, mhchem -j4: a run
/// went on without end, accl 25662, mc 80815). The texts between C and
/// P put C alone at the head of the workers' first blocks.
#[test]
fn a_run_waiting_for_a_node_a_claim_takes_goes_on() {
    let e = Engine::new();
    let v = e.values();
    let def4 = e.register(Arc::new(Def4));
    let claim = e.register(Arc::new(ClaimIfLate));
    let text = e.register(Arc::new(Text));
    let p_op = e.register(Arc::new(QuietSettler));
    let waits = Arc::new(AtomicU64::new(0));
    let w_op = e.register(Arc::new(ReadsSettler(waits.clone())));
    let r_op = e.register(Arc::new(UntilEqual));
    e.set_limits(phitex_defuse::Limits {
        time: Some(Duration::from_secs(20)),
        ..Default::default()
    });
    let u = e.new_unit();
    e.push(u, def4, vec![v.int(1), v.int(1), v.int(1), v.int(0)]);
    let c = e.push(u, claim, vec![]);
    let p = e.push(u, p_op, vec![v.int(0)]);
    for _ in 0..3 {
        e.push(u, text, vec![v.int(0)]);
    }
    let w = e.push(u, w_op, vec![]);
    e.push(u, r_op, vec![]);
    let r = e.evaluate(4);
    assert!(waits.load(Ordering::Relaxed) > 0, "W never waited for P: {r:?}");
    assert!(r.is_ok(), "W left waiting for P, which the claim took: {r:?}");
    assert_eq!(e.claimed(c), vec![p]);
    assert_eq!(e.output(w), Some(v.int(1)), "W ran on y = 1");
    assert!(e.check_invariants().is_empty());
}
