//! Shapes a real document makes that the engine must take in stride.

use std::sync::Arc;
use std::time::{Duration, Instant};

use phitex_defuse::{Ctx, Engine, Field, NodeId, OpId, Operation, Stop, ValueId};

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

/// `[f]`: outputs the definition of f reaching it.
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

/// `[n]`: places n children `Def(c_i, i)` after itself, each `child`
/// (one node placing thousands: a piece whose pages all fire in it).
struct Flood(OpId);

impl Operation for Flood {
    fn name(&self) -> &'static str {
        "flood"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap();
        for i in 0..n {
            let f = v.field(&format!("c{i}"));
            cx.child(self.0, vec![f.0, v.int(i)]);
        }
        Ok(cx.args()[0])
    }
}

/// `[f, n]`: reads f n times (a run that reads one field over and over).
struct Reader;

impl Operation for Reader {
    fn name(&self) -> &'static str {
        "reader"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let f = Field(cx.args()[0]);
        let n = cx.values().as_int(cx.args()[1]).unwrap();
        let mut last = ValueId::UNIT;
        for _ in 0..n {
            last = cx.read(f)?.unwrap_or(ValueId::UNIT);
        }
        Ok(last)
    }
}

fn int(e: &Engine, n: NodeId) -> i64 {
    e.values().as_int(e.output(n).expect("ran")).expect("int")
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

#[test]
fn one_node_placing_ten_thousand_children_fits() {
    for n in [1, threads().min(16)] {
        let e = Engine::new();
        let def = e.register(Arc::new(Def));
        let flood = e.register(Arc::new(Flood(def)));
        let get = e.register(Arc::new(Get));
        let v = e.values();
        let u = e.new_unit();
        let a = e.push(u, def, vec![v.field("a").0, v.int(1)]);
        let fl = e.push(u, flood, vec![v.int(10000)]);
        let b = e.push(u, def, vec![v.field("b").0, v.int(2)]);
        let g = e.push(u, get, vec![v.field("c9999").0]);
        let t = Instant::now();
        e.evaluate(n).unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{:?} at {n} threads",
            t.elapsed()
        );
        assert_eq!(e.children(fl).len(), 10000);
        assert_eq!(int(&e, g), 9999);
        let order = e.order(u);
        assert_eq!(order.first(), Some(&a));
        assert_eq!(order[1], fl);
        assert_eq!(order[10002], b);
        assert_eq!(order.len(), 10004);
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn a_read_of_an_unchanged_field_is_cheap() {
    let e = Engine::new();
    let def = e.register(Arc::new(Def));
    let reader = e.register(Arc::new(Reader));
    let v = e.values();
    let x = v.field("x").0;
    let u = e.new_unit();
    e.push(u, def, vec![x, v.int(1)]);
    for _ in 0..100 {
        e.push(u, def, vec![v.field("other").0, v.int(1)]);
    }
    let r = e.push(u, reader, vec![x, v.int(2_000_000)]);
    let t = Instant::now();
    e.evaluate(1).unwrap();
    let per = t.elapsed() / 2_000_000;
    assert_eq!(int(&e, r), 1);
    assert!(
        per < Duration::from_nanos(150),
        "a read of an unchanged field costs {per:?}; a hundred nanoseconds is the bar"
    );
}

/// `[n]`: places the chain's next step `[n - 1]` after itself while `n >
/// 0` (a loop's iterations, each a continuation of the last), and
/// contributes 1 to `s`.
struct Chain(std::sync::OnceLock<OpId>, Field);

impl Operation for Chain {
    fn name(&self) -> &'static str {
        "chain"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap();
        if n > 0 {
            cx.follow(*self.0.get().unwrap(), vec![v.int(n - 1)]);
        }
        cx.contribute(self.1, v.int(1));
        Ok(cx.args()[0])
    }
}

/// `[f]`: the fold of f from the unit's start to it.
struct FoldAll;

impl Operation for FoldAll {
    fn name(&self) -> &'static str {
        "fold_all"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.fold(Field(cx.args()[0]), None, None)
    }
}

#[test]
fn a_long_chain_in_a_pushed_run_takes_labels_from_its_neighbours() {
    // Nodes pushed many at once share a segment's labels a push's step
    // apart; a chain of twenty thousand steps placed one after another in
    // the middle of them uses those labels up many times over, and its
    // segment takes room from the segments after it (`Order::rebalance`),
    // its order, its contributions and the fold after it all kept.
    let e = Engine::new();
    let v = e.values();
    let s = v.field("s");
    e.declare_accumulator(s, Arc::new(phitex_defuse::SumMonoid));
    let def = e.register(Arc::new(Def));
    let ch = Arc::new(Chain(std::sync::OnceLock::new(), s));
    let chain = e.register(ch.clone());
    ch.0.set(chain).unwrap();
    let fold = e.register(Arc::new(FoldAll));
    let u = e.new_unit();
    let x = v.field("x");
    let ids = e.push_many(u, def, (0..100).map(|i| vec![x.0, v.int(i)]).collect(), 4);
    let n = 20_000;
    let head = e.insert_after(ids[50], chain, vec![v.int(n)]);
    let total = e.insert_after(ids[70], fold, vec![s.0]);
    let t = Instant::now();
    e.evaluate(threads().min(8)).unwrap();
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    let order = e.order(u);
    let at = order.iter().position(|&x| x == head).unwrap();
    for k in 0..=n as usize {
        assert_eq!(int(&e, order[at + k]), n - k as i64);
    }
    assert_eq!(order[at + n as usize + 1], ids[51]);
    assert_eq!(int(&e, total), n + 1);
    assert!(e.check_invariants().is_empty());
}

/// `[n]`: like `Chain`, and places a child `Def(s_n, n)` before its next
/// step (a piece whose pages fire in it, then the piece's next part).
struct ChainKids(std::sync::OnceLock<OpId>, OpId, Field);

impl Operation for ChainKids {
    fn name(&self) -> &'static str {
        "chain_kids"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[0]).unwrap();
        cx.child(self.1, vec![v.field("k").0, v.int(n)]);
        if n > 0 {
            cx.follow(*self.0.get().unwrap(), vec![v.int(n - 1)]);
        }
        cx.contribute(self.2, v.int(1));
        Ok(cx.args()[0])
    }
}

#[test]
fn long_chains_among_many_pushed_pieces_fit() {
    // A book's shape: thousands of pieces pushed at once, and a few of
    // them read on in a long chain of continuations, each placing a page
    // after itself first; the chains use up their pushed neighbours'
    // labels many times over, and the line makes room over a window of
    // segments as wide as it needs, never out of labels.
    for threads in [1, threads().min(8)] {
        let e = Engine::new();
        let v = e.values();
        let s = v.field("s");
        e.declare_accumulator(s, Arc::new(phitex_defuse::SumMonoid));
        let def = e.register(Arc::new(Def));
        let ch = Arc::new(ChainKids(std::sync::OnceLock::new(), def, s));
        let chain = e.register(ch.clone());
        ch.0.set(chain).unwrap();
        let fold = e.register(Arc::new(FoldAll));
        let u = e.new_unit();
        let x = v.field("x");
        let ids = e.push_many(u, def, (0..4000).map(|i| vec![x.0, v.int(i)]).collect(), 4);
        let n = 10_000;
        let heads: Vec<NodeId> = [100, 101, 1999, 3998]
            .iter()
            .map(|&i| e.insert_after(ids[i], chain, vec![v.int(n)]))
            .collect();
        let total = e.push(u, fold, vec![s.0]);
        let t = Instant::now();
        e.evaluate(threads).unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "{:?} at {threads} threads",
            t.elapsed()
        );
        let order = e.order(u);
        for (&h, &i) in heads.iter().zip(&[100usize, 101, 1999, 3998]) {
            let at = order.iter().position(|&x| x == h).unwrap();
            for k in 0..=n as usize {
                assert_eq!(int(&e, order[at + 2 * k]), n - k as i64);
            }
            assert_eq!(order[at + 2 * n as usize + 2], ids[i + 1]);
        }
        assert_eq!(int(&e, total), 4 * (n + 1));
        assert!(e.check_invariants().is_empty());
    }
}

#[test]
fn floods_among_long_chains_fit() {
    // Pieces placing thousands of children in one commit next to chains
    // that used up their labels: a commit's placements past its
    // segment's room go after a rebalance, one by one, never out of
    // labels.
    for threads in [1, threads().min(8)] {
        let e = Engine::new();
        let v = e.values();
        let s = v.field("s");
        e.declare_accumulator(s, Arc::new(phitex_defuse::SumMonoid));
        let def = e.register(Arc::new(Def));
        let ch = Arc::new(ChainKids(std::sync::OnceLock::new(), def, s));
        let chain = e.register(ch.clone());
        ch.0.set(chain).unwrap();
        let flood = e.register(Arc::new(Flood(def)));
        let u = e.new_unit();
        let x = v.field("x");
        let ids = e.push_many(u, def, (0..2000).map(|i| vec![x.0, v.int(i)]).collect(), 4);
        let mut floods = Vec::new();
        for &i in &[10usize, 11, 500, 1000, 1999] {
            e.insert_after(ids[i], chain, vec![v.int(3000)]);
            floods.push(e.insert_after(ids[i], flood, vec![v.int(3000)]));
        }
        e.evaluate(threads).unwrap();
        for f in floods {
            assert_eq!(e.children(f).len(), 3000);
        }
        assert_eq!(e.order(u).len(), 2000 + 5 * (1 + 3000) + 5 * 2 * 3001);
        assert!(e.check_invariants().is_empty());
    }
}

/// `[n, k]`: places its chain's next step `[n - 1, k]` after itself while
/// `n > 0`; the last step places k children `Def` in its one commit.
struct ChainThenFlood(std::sync::OnceLock<OpId>, OpId);

impl Operation for ChainThenFlood {
    fn name(&self) -> &'static str {
        "chain_then_flood"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let (n, k) = (v.as_int(cx.args()[0]).unwrap(), v.as_int(cx.args()[1]).unwrap());
        if n > 0 {
            cx.follow(*self.0.get().unwrap(), vec![v.int(n - 1), v.int(k)]);
        } else {
            let f = v.field("z");
            for i in 0..k {
                cx.child(self.1, vec![f.0, v.int(i)]);
            }
        }
        Ok(cx.args()[0])
    }
}

#[test]
fn a_flood_at_the_end_of_a_long_chain_fits() {
    // A chain of steps placed one after another narrows its segments as
    // they split (the book: a chapter read on in parts); its last step
    // places thousands of children in one commit, more than its narrow
    // segment can label: the placements its segment has no room for go
    // after a rebalance, never a relabel without a gap.
    for threads in [1, threads().min(8)] {
        let e = Engine::new();
        let v = e.values();
        let def = e.register(Arc::new(Def));
        let ch = Arc::new(ChainThenFlood(std::sync::OnceLock::new(), def));
        let chain = e.register(ch.clone());
        ch.0.set(chain).unwrap();
        let u = e.new_unit();
        let x = v.field("x");
        let ids = e.push_many(u, def, (0..200).map(|i| vec![x.0, v.int(i)]).collect(), 4);
        e.insert_after(ids[100], chain, vec![v.int(5000), v.int(60000)]);
        e.evaluate(threads).unwrap();
        assert_eq!(e.order(u).len(), 200 + 5001 + 60000);
        assert!(e.check_invariants().is_empty());
    }
}
