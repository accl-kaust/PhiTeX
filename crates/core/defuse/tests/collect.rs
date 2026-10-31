//! The value store's collection (`Limits::collect_at_bytes`): what runs
//! thrown away interned is freed with every worker at the barrier, and
//! nothing a record, a reader or a run in flight holds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use phitex_defuse::{Ctx, Engine, Field, Limits, NodeId, OpId, Operation, SeqMonoid, Stop, ValueId, Values};

/// The list `[salt * 1000 + i]` for `i < k`.
fn made(v: &Values, k: i64, salt: i64) -> ValueId {
    v.list((0..k).map(|i| v.int(salt * 1000 + i)).collect())
}

/// `[f, k, salt]`: defines f := `made(k, salt)` and contributes it to
/// `acc` as a one-element sequence.
struct Make;

impl Operation for Make {
    fn name(&self) -> &'static str {
        "make"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let a = cx.args().to_vec();
        let x = made(v, v.as_int(a[1]).unwrap(), v.as_int(a[2]).unwrap());
        cx.define(Field(a[0]), x);
        cx.contribute(v.field("acc"), v.seq_unit(x));
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: f's value.
struct Read;

impl Operation for Read {
    fn name(&self) -> &'static str {
        "read"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

/// `[]`: the fold of `acc` to it.
struct FoldAll;

impl Operation for FoldAll {
    fn name(&self) -> &'static str {
        "fold_all"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = cx.values().field("acc");
        cx.fold(f, None, None)
    }
}

const NODES: i64 = 40;
const LEN: i64 = 200;

/// Makers and their readers, and a fold of all of them at the end.
fn line(e: &Engine) -> (OpId, Vec<NodeId>, Vec<NodeId>, NodeId) {
    let v = e.values();
    e.declare_accumulator(v.field("acc"), Arc::new(SeqMonoid));
    let mk = e.register(Arc::new(Make));
    let rd = e.register(Arc::new(Read));
    let fa = e.register(Arc::new(FoldAll));
    let u = e.new_unit();
    let (mut ms, mut rs) = (Vec::new(), Vec::new());
    for i in 0..NODES {
        let f = v.field(&format!("f{i}"));
        ms.push(e.push(u, mk, vec![f.0, v.int(LEN), v.int(i)]));
        rs.push(e.push(u, rd, vec![f.0]));
    }
    (mk, ms, rs, e.push(u, fa, vec![]))
}

/// Every reader's output and the fold as the makers' salts say.
fn check(e: &Engine, rs: &[NodeId], fold: NodeId, salts: &[i64]) {
    let v = e.values();
    for (i, &r) in rs.iter().enumerate() {
        let x = e.output(r).expect("ran");
        assert_eq!(x, made(v, LEN, salts[i]), "reader {i}");
        let n = v.with_list(x, <[ValueId]>::len).expect("a list, not collected");
        assert_eq!(n, usize::try_from(LEN).unwrap());
    }
    let all = v.seq_to_vec(e.output(fold).expect("ran"));
    assert_eq!(all.len(), salts.len());
    for (i, &x) in all.iter().enumerate() {
        assert_eq!(x, made(v, LEN, salts[i]), "fold element {i}");
    }
}

#[test]
fn a_collection_frees_what_thrown_away_runs_made_and_keeps_what_stands() {
    let e = Engine::new();
    e.set_limits(Limits {
        collect_at_bytes: Some(256 << 10),
        ..Limits::default()
    });
    let (mk, ms, rs, fold) = line(&e);
    let v = e.values();
    let mut salts: Vec<i64> = (0..NODES).collect();
    e.evaluate(4).unwrap();
    check(&e, &rs, fold, &salts);
    let mut collections = Vec::new();
    for round in 1..40 {
        for (i, &m) in ms.iter().enumerate() {
            salts[i] = round * 100 + i as i64;
            e.replace(m, mk, vec![v.field(&format!("f{i}")).0, v.int(LEN), v.int(salts[i])]);
        }
        let rep = e.evaluate(4).unwrap();
        collections.extend(rep.collections.iter().map(|c| c.0));
        check(&e, &rs, fold, &salts);
        assert!(e.check_invariants().is_empty());
        assert!(e.rerun_check().is_empty(), "{:?}", e.rerun_check());
    }
    assert!(!collections.is_empty(), "no collection");
    for c in &collections {
        assert!(c.bytes_after < c.bytes_before, "{c:?}");
        assert!(c.lists > 0, "{c:?}");
    }
    // (what stays is what the line holds: some 40 lists of 200 a round,
    // well under what 40 rounds made)
    assert!(v.payload_bytes() < 40 * 40 * 200 * 8, "{}", v.payload_bytes());
}

/// `[]`: holds a list of its own over a long run; says whether a
/// collection happened meanwhile.
struct Slow(Arc<AtomicBool>);

impl Operation for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let mine = made(v, 1000, 77_777);
        let before = v.collections();
        std::thread::sleep(Duration::from_millis(300));
        if v.collections() != before {
            self.0.store(true, Ordering::SeqCst);
        }
        assert_eq!(v.with_list(mine, <[ValueId]>::len), Some(1000));
        Ok(mine)
    }
}

#[test]
fn a_collection_waits_for_every_run_in_flight() {
    let e = Engine::new();
    e.set_limits(Limits {
        collect_at_bytes: Some(64 << 10),
        ..Limits::default()
    });
    let v = e.values();
    e.declare_accumulator(v.field("acc"), Arc::new(SeqMonoid));
    let crossed = Arc::new(AtomicBool::new(false));
    let mk = e.register(Arc::new(Make));
    let slow = e.register(Arc::new(Slow(crossed.clone())));
    let u = e.new_unit();
    let n = 400;
    let mut s = None;
    for i in 0..n {
        if i == n / 4 {
            s = Some(e.push(u, slow, vec![]));
        }
        e.push(u, mk, vec![v.field(&format!("g{i}")).0, v.int(LEN), v.int(i)]);
    }
    let rep = e.evaluate(4).unwrap();
    assert!(!rep.collections.is_empty(), "no collection");
    assert!(!crossed.load(Ordering::SeqCst), "a collection ran beside a run");
    let x = e.output(s.unwrap()).unwrap();
    assert_eq!(x, made(v, 1000, 77_777));
    assert!(e.check_invariants().is_empty());
}

/// `[f]`: reads f through `Doubled` (a list made by the projection: no
/// record but this read's holds it) and panics, on inputs that hold.
struct ReadsThenPanics(phitex_defuse::PathId, Arc<std::sync::atomic::AtomicU64>);

impl Operation for ReadsThenPanics {
    fn name(&self) -> &'static str {
        "reads_then_panics"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.1.fetch_add(1, Ordering::SeqCst);
        cx.read_path(Field(cx.args()[0]), self.0)?;
        panic!("a genuine failure");
    }
}

#[test]
fn a_panicked_runs_reads_are_kept_for_the_judgment_at_the_end() {
    // (its seen list freed, the judgment made the list anew, under
    // another id, took the input for moved and ran it again)
    let e = Engine::new();
    e.set_limits(Limits {
        collect_at_bytes: Some(64 << 10),
        ..Limits::default()
    });
    let v = e.values();
    e.declare_accumulator(v.field("acc"), Arc::new(SeqMonoid));
    let doubled = e.register_projection(Arc::new(|x: ValueId, v: &Values| v.list(vec![x, x])));
    let path = e.path(&[phitex_defuse::Step::Proj(doubled)]);
    let runs = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mk = e.register(Arc::new(Make));
    let bad = e.register(Arc::new(ReadsThenPanics(path, runs.clone())));
    let u = e.new_unit();
    let f = v.field("h");
    e.push(u, mk, vec![f.0, v.int(10), v.int(1)]);
    e.push(u, bad, vec![f.0]);
    for i in 0..400 {
        e.push(u, mk, vec![v.field(&format!("g{i}")).0, v.int(LEN), v.int(i)]);
    }
    let r = e.evaluate(1);
    assert!(matches!(r, Err(phitex_defuse::EvalError::OpPanicked { .. })), "{r:?}");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "run again on an answer the collection freed"
    );
}

/// `[]`: says it runs, then holds until `go` (or a second).
struct Holds(Arc<AtomicBool>, Arc<AtomicBool>);

impl Operation for Holds {
    fn name(&self) -> &'static str {
        "holds"
    }

    fn run(&self, _cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.store(true, Ordering::SeqCst);
        let t = std::time::Instant::now();
        while !self.1.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(1) {
            std::hint::spin_loop();
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_collection_called_during_an_evaluation_is_refused() {
    // (its tasks read lists without a pin: a collection beside them would
    // free what they read)
    let e = Arc::new(Engine::new());
    let (running, go) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let h = e.register(Arc::new(Holds(running.clone(), go.clone())));
    let u = e.new_unit();
    e.push(u, h, vec![]);
    let ev = {
        let e = e.clone();
        std::thread::spawn(move || e.evaluate(2))
    };
    while !running.load(Ordering::SeqCst) {
        std::hint::spin_loop();
    }
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.collect()));
    go.store(true, Ordering::SeqCst);
    assert!(r.is_err(), "collected during an evaluation");
    ev.join().unwrap().unwrap();
    e.collect();
    assert!(e.check_invariants().is_empty());
}

#[test]
fn a_list_read_outside_the_evaluation_is_whole_or_gone_across_its_collections() {
    // (a thread not of the pool pins the store for each read: a list a
    // collection takes meanwhile is freed only once no reader is pinned)
    let e = Arc::new(Engine::new());
    e.set_limits(Limits {
        collect_at_bytes: Some(64 << 10),
        ..Limits::default()
    });
    let v = e.values();
    let mine = v.list((0..64).map(|i| v.int(i)).collect());
    let mk = e.register(Arc::new(Make));
    e.declare_accumulator(v.field("acc"), Arc::new(SeqMonoid));
    let u = e.new_unit();
    for i in 0..400 {
        e.push(u, mk, vec![v.field(&format!("g{i}")).0, v.int(LEN), v.int(i)]);
    }
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let (e, done) = (e.clone(), done.clone());
        std::thread::spawn(move || {
            let (mut whole, mut gone) = (0u64, 0u64);
            while !done.load(Ordering::SeqCst) {
                let v = e.values();
                match v.with_list(mine, |xs| xs.iter().map(|&x| v.as_int(x).unwrap()).sum::<i64>()) {
                    Some(s) => {
                        assert_eq!(s, 63 * 64 / 2, "a list read across its collection");
                        whole += 1;
                    }
                    None => gone += 1,
                }
            }
            (whole, gone)
        })
    };
    let rep = e.evaluate(4).unwrap();
    done.store(true, Ordering::SeqCst);
    let (whole, gone) = reader.join().unwrap();
    assert!(!rep.collections.is_empty(), "no collection");
    assert!(whole + gone > 0);
    assert!(v.with_list(mine, <[ValueId]>::len).is_none(), "nothing held it");
}

#[test]
fn a_store_counts_its_payload_only_while_it_collects() {
    // (a count per value made is a store's that collects: one that does
    // not pays none, and one that starts counts what it holds)
    let e = Engine::new();
    let v = e.values();
    let xs: Vec<ValueId> = (0..100).map(|i| made(v, 10, i)).collect();
    assert_eq!(v.payload_bytes(), 0);
    e.set_limits(Limits {
        collect_at_bytes: Some(1 << 30),
        ..Limits::default()
    });
    let held = v.payload_bytes();
    assert!(held >= 100 * 10 * 8, "{held}");
    made(v, 10, 1000);
    assert!(v.payload_bytes() > held);
    e.set_limits(Limits::default());
    assert_eq!(v.payload_bytes(), 0);
    let c = e.collect();
    assert!(c.lists >= 101 && c.bytes_before > c.bytes_after, "{c:?}");
    assert!(xs.iter().all(|&x| v.with_list(x, <[ValueId]>::len).is_none()));
}
